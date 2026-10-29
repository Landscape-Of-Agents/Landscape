// A mock agent call decided by the Brick rules, so a person can see what a Brick would do before
// (or after) it is attached: 'crane brick test ID --target flow:ID --call JSON', and the Test
// button in Landscape. The call is described by its parameters (operation, files, command, hosts,
// who calls, and what the session already did) and decided by the functions the hooks use before
// every real tool call: the per-file limits (compose::file_limit, folded the way effect::apply
// folds them) and the per-call rules (guard::decide). Nothing runs: no command is executed, no
// host is reached, nothing is counted or recorded as a decision. Gates (preconditions) are
// evaluated against the working tree as it is now. Policies, the autonomy mode, credits, and the
// safety state are not part of the test; the guard applies them too, and they can only make a
// decision stricter.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use super::compose::{controls, file_limit, scope_resolved};
use super::guard::{decide, targets_of, Call, Change, Rules};
use super::store::BrickStore;
use super::usage::{TargetUsage, Usage};
use crate::governance::workspace::Workspace;
use crate::governance::zones::{Ceiling, Criticality, Limit, Limits};
use crate::hooks::action::Operation;
use crate::telemetry::model::Decision;

/** The fields a mock call may have */
pub(crate) const FIELDS: &[&str] = &[
    "operation",
    "files",
    "new_file",
    "delete",
    "lines",
    "command",
    "hosts",
    "text",
    "agent",
    "agent_id",
    "model",
    "session",
    "watch_firing",
];

/** The fields of the session part of a mock call (what the session did before this call) */
pub(crate) const SESSION_FIELDS: &[&str] = &[
    "tool_calls",
    "minutes",
    "files_changed",
    "lines_changed",
    "commands",
    "network_calls",
    "denials",
    "failures_in_row",
];

/** Most entries one list of a mock call may hold */
const MAX_ITEMS: usize = 50;

/** Longest string of a mock call (characters) */
const MAX_TEXT: usize = 4096;

/** Highest count a mock call may state */
const MAX_COUNT: u64 = 1_000_000;

/** A list of strings of a mock call (a string counts as a list of one)
 * Input
    - call: &Value - mock call
    - key: &str - field
 * Output
    - Result<Vec<String>, String>
    - Error when the field is not a string or a list of strings, or too long
*/
fn strings(call: &Value, key: &str) -> Result<Vec<String>, String> {
    let list = match &call[key] {
        Value::Null => Vec::new(),
        Value::String(text) => vec![text.clone()],
        Value::Array(items) if items.len() <= MAX_ITEMS => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| format!("'{key}' holds only strings"))
            })
            .collect::<Result<Vec<_>, _>>()?,
        Value::Array(_) => return Err(format!("'{key}' holds at most {MAX_ITEMS} entries")),
        _ => return Err(format!("'{key}' is a string or a list of strings")),
    };
    if list.iter().any(|text| text.chars().count() > MAX_TEXT) {
        return Err(format!(
            "an entry of '{key}' is longer than {MAX_TEXT} characters"
        ));
    }
    Ok(list
        .into_iter()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .collect())
}

/** A repository-relative file of a mock call
 * Input
    - raw: &str - file as given
 * Output
    - Result<String, String>
    - Error when it is absolute or leaves the repository
*/
fn file(raw: &str) -> Result<String, String> {
    let path = raw.trim().replace('\\', "/");
    let path = path.trim_start_matches("./");
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(':')
        || path.split('/').any(|part| part == "..")
    {
        return Err(format!("'{raw}' is not a file inside the repository"));
    }
    Ok(path.to_string())
}

/** A count of a mock call
 * Input
    - part: &Value - object holding the count
    - key: &str - field
 * Output
    - Result<u64, String> (0 when absent)
*/
fn count(part: &Value, key: &str) -> Result<u64, String> {
    match &part[key] {
        Value::Null => Ok(0),
        value => value
            .as_u64()
            .filter(|number| *number <= MAX_COUNT)
            .ok_or_else(|| format!("'{key}' is a whole number up to {MAX_COUNT}")),
    }
}

/** An optional text of a mock call
 * Input
    - call: &Value - mock call
    - key: &str - field
 * Output
    - Result<Option<String>, String> (None when absent or blank)
*/
fn text(call: &Value, key: &str) -> Result<Option<String>, String> {
    match &call[key] {
        Value::Null => Ok(None),
        Value::String(text) if text.chars().count() <= MAX_TEXT => {
            Ok(Some(text.trim().to_string()).filter(|text| !text.is_empty()))
        }
        Value::String(_) => Err(format!("'{key}' is longer than {MAX_TEXT} characters")),
        _ => Err(format!("'{key}' is a string")),
    }
}

/** The word of a decision in a test result
 * Input
    - decision: Decision - decision
 * Output
    - &'static str: "allow", "approval", or "deny"
*/
fn word(decision: Decision) -> &'static str {
    match decision {
        Decision::Allow => "allow",
        Decision::RequireApproval => "approval",
        _ => "deny",
    }
}

/** How strict a decision word is (higher is stricter)
 * Input
    - word: &str - "allow", "approval", or "deny"
 * Output
    - u8
*/
fn strictness(word: &str) -> u8 {
    match word {
        "deny" => 2,
        "approval" => 1,
        _ => 0,
    }
}

/** Decide a mock call against the Brick rules of a store
 * Input
    - workspace: &Workspace - repository (gates read its working tree)
    - store: &BrickStore - Brick store, usually with the tested Brick attached (cli/brick/check.rs)
    - members: &BTreeMap<String, BTreeSet<String>> - the files of the active Zones and Flows (and
      the tested target), by key
    - base: Option<&Limits> - the per-file limits of the Zones and Flows (None: only target
      membership is known, so only the Bricks decide)
    - target: &str - the target under test; the session part of the call counts there
    - call: &Value - the mock call: {operation (read|write|execute|other; default execute with a
      command, else write), files, new_file, delete, lines (per changed file, default 10), command,
      hosts, text (other arguments), agent (default "claude"; "" for a person, whom identity rules
      do not decide), agent_id, model, session {tool_calls, minutes, files_changed, lines_changed,
      commands, network_calls, denials, failures_in_row}, watch_firing}
    - now: u64 - Unix milliseconds
 * Output
    - Result<Value, String> {verdict, findings, files, counted, call, notes}
    - Error when the mock call is malformed
*/
pub(crate) fn run(
    workspace: &Workspace,
    store: &BrickStore,
    members: &BTreeMap<String, BTreeSet<String>>,
    base: Option<&Limits>,
    target: &str,
    call: &Value,
    now: u64,
) -> Result<Value, String> {
    let Some(fields) = call.as_object() else {
        return Err("the mock call is a JSON object".into());
    };
    if let Some(key) = fields.keys().find(|key| !FIELDS.contains(&key.as_str())) {
        return Err(format!(
            "'{key}' is not a field of a mock call (fields: {})",
            FIELDS.join(", ")
        ));
    }
    let session = &call["session"];
    if !session.is_null() && !session.is_object() {
        return Err("'session' is an object of counts".into());
    }
    if let Some(key) = session.as_object().and_then(|part| {
        part.keys()
            .find(|key| !SESSION_FIELDS.contains(&key.as_str()))
    }) {
        return Err(format!(
            "'{key}' is not a session count (counts: {})",
            SESSION_FIELDS.join(", ")
        ));
    }
    let files = strings(call, "files")?
        .iter()
        .map(|raw| file(raw))
        .collect::<Result<Vec<_>, _>>()?;
    let command = text(call, "command")?;
    let operation = match call["operation"].as_str() {
        Some("read") => Operation::Read,
        Some("write") => Operation::Write,
        Some("execute") => Operation::Execute,
        Some("other") => Operation::Other,
        None if command.is_some() => Operation::Execute,
        None => Operation::Write,
        Some(other) => {
            return Err(format!(
                "operation '{other}' is read, write, execute, or other"
            ))
        }
    };
    if operation == Operation::Execute && command.is_none() {
        return Err("an execute call needs a command".into());
    }
    if matches!(operation, Operation::Read | Operation::Write) && files.is_empty() {
        return Err(format!(
            "a {} call names at least one file",
            operation.name()
        ));
    }
    let hosts = strings(call, "hosts")?;
    let other = strings(call, "text")?;
    let lines = match &call["lines"] {
        Value::Null => 10,
        _ => count(call, "lines")?,
    };
    let deleted = call["delete"].as_bool().unwrap_or(false);
    let new_file = call["new_file"].as_bool();
    let agent = match &call["agent"] {
        Value::Null => Some("claude".to_string()),
        _ => text(call, "agent")?,
    };
    let agent_id = text(call, "agent_id")?;
    let model = text(call, "model")?;

    // What the session did before this call, counted in the target under test
    let mut usage = Usage {
        session: "trial".into(),
        ..Usage::default()
    };
    usage.targets.insert(
        target.to_string(),
        TargetUsage {
            calls: count(session, "tool_calls")?,
            commands: count(session, "commands")?,
            network: count(session, "network_calls")?,
            denials: count(session, "denials")?,
            files: (1..=count(session, "files_changed")?.min(10_000))
                .map(|number| format!("(earlier file {number})"))
                .collect(),
            lines: count(session, "lines_changed")?,
            failures_in_row: u32::try_from(count(session, "failures_in_row")?).unwrap_or(u32::MAX),
        },
    );
    let minutes = count(session, "minutes")?;

    // Target membership as the per-file limits record it: the real limits when known, and always
    // the target under test (so a target that is not active yet is tested as if it were)
    let mut limits = base.cloned().unwrap_or_default();
    let blank = || Limit {
        ceiling: Ceiling::Autonomous,
        criticality: Criticality::Routine,
        entities: Vec::new(),
        scopes: Vec::new(),
        bricks: Vec::new(),
    };
    for (key, covered) in members {
        if base.is_some() && key != target {
            continue;
        }
        for path in covered {
            let limit = limits.entry(path.clone()).or_insert_with(blank);
            if !limit.entities.contains(key) {
                limit.entities.push(key.clone());
            }
        }
    }
    let mut rules = Rules::default();
    for key in store
        .attachments
        .iter()
        .map(|attachment| attachment.target.key())
        .collect::<BTreeSet<_>>()
    {
        let combined = controls(store, &key, &workspace.root);
        if combined.decides_calls() {
            rules.targets.insert(key, combined);
        }
    }
    if call["watch_firing"].as_bool() == Some(true)
        && rules
            .targets
            .get(target)
            .is_some_and(|combined| !combined.watch_pause.is_empty())
    {
        rules.paused.insert(target.to_string());
    }

    let reads = if operation == Operation::Read {
        files.clone()
    } else {
        Vec::new()
    };
    let changes = if operation == Operation::Write {
        files
            .iter()
            .map(|path| Change {
                existed: new_file.map_or_else(|| workspace.root.join(path).is_file(), |new| !new),
                deleted,
                lines,
                path: path.clone(),
            })
            .collect()
    } else {
        Vec::new()
    };
    let named = if operation == Operation::Execute {
        command
            .as_deref()
            .map(crate::hooks::shell::words)
            .unwrap_or_default()
            .iter()
            .filter_map(|word| file(word).ok())
            .filter(|path| workspace.root.join(path).is_file())
            .chain(files.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };
    let arguments = command
        .iter()
        .cloned()
        .chain(files.iter().cloned())
        .chain(other)
        .collect::<Vec<_>>();
    let mock = Call {
        operation,
        reads,
        changes,
        named,
        command: command.as_deref(),
        arguments: &arguments,
        network: &hosts,
        agent: agent.as_deref(),
        agent_id: agent_id.as_deref(),
        model: model.as_deref(),
        started_at: Some(now.saturating_sub(minutes.saturating_mul(60_000))),
        usage: Some(&usage),
        now,
    };
    let outcome = decide(&rules, &limits, workspace, &mock);

    // The per-file limits of each changed file, folded as effect::apply folds them
    let mut verdict = "allow";
    let mut rows = Vec::new();
    for path in &files {
        let entities = targets_of(&limits, path);
        let changed = mock.changes.iter().any(|change| change.path == *path);
        let known = limits.get(path);
        let mut ceiling = known.map_or(Ceiling::Autonomous, |limit| limit.ceiling);
        let mut criticality = known.map_or(Criticality::Routine, |limit| limit.criticality);
        let mut reasons = Vec::new();
        for key in &entities {
            let combined = controls(store, key, &workspace.root);
            if !combined.restricts() {
                continue;
            }
            let covered = members.get(key).cloned().unwrap_or_default();
            let (lower, higher, why) =
                file_limit(&combined, path, scope_resolved(&combined, &covered));
            if let Some(lower) = lower {
                ceiling = ceiling.min(lower);
            }
            if let Some(higher) = higher {
                criticality = criticality.max(higher);
            }
            reasons.extend(why.into_iter().map(|reason| format!("{key}: {reason}")));
        }
        let decision = match (changed, ceiling, criticality) {
            (false, _, _) => "allow",
            (true, Ceiling::Observe, _) => "deny",
            (true, Ceiling::Assisted, _) | (true, _, Criticality::Restricted) => "approval",
            _ => "allow",
        };
        if strictness(decision) > strictness(verdict) {
            verdict = decision;
        }
        rows.push(json!({
            "path": path,
            "targets": entities,
            "changed": changed,
            "exists": workspace.root.join(path).is_file(),
            "ceiling": ceiling.name(),
            "zone_ceiling": base.and_then(|limits| limits.get(path)).map(|limit| limit.ceiling.name()),
            "criticality": criticality,
            "decision": decision,
            "reasons": reasons,
        }));
    }
    let findings = outcome
        .findings
        .iter()
        .map(|finding| {
            let decision = word(finding.decision);
            if strictness(decision) > strictness(verdict) {
                verdict = decision;
            }
            json!({"decision": decision, "rule": finding.rule, "reason": finding.reason, "brick": finding.brick})
        })
        .collect::<Vec<_>>();
    let mut notes = vec![
        "Nothing ran: no command was executed, no host was reached, and nothing was counted.".to_string(),
        "Policies, the autonomy mode, credits, and the safety state also decide real calls; they can only make the decision stricter.".to_string(),
    ];
    if !rules.targets.is_empty()
        && rules
            .targets
            .values()
            .any(|combined| !combined.gates.is_empty())
    {
        notes.push("Gates were evaluated against this working tree as it is now.".into());
    }
    if agent.is_none() {
        notes
            .push("No agent calls (a person's question), so Identity Bricks do not decide.".into());
    }
    if files
        .iter()
        .chain(mock.named.iter())
        .all(|path| targets_of(&limits, path).is_empty())
        && !files.is_empty()
    {
        notes.push(format!(
            "None of the files belongs to {target} or any other Zone or Flow, so only repository-wide rules apply."
        ));
    }
    Ok(json!({
        "target": target,
        "verdict": verdict,
        "findings": findings,
        "files": rows,
        "counted": {
            "targets": outcome.counted.targets,
            "command": outcome.counted.command,
            "network": outcome.counted.network,
            "changes": outcome.counted.changes.iter().map(|(target, path, lines)| json!({"target": target, "path": path, "lines": lines})).collect::<Vec<_>>(),
        },
        "call": {
            "operation": operation.name(),
            "files": files,
            "command": command,
            "hosts": hosts,
            "agent": agent,
            "model": model,
            "lines": lines,
            "delete": deleted,
            "session": SESSION_FIELDS.iter().map(|key| (key.to_string(), json!(count(session, key).unwrap_or(0)))).collect::<serde_json::Map<_, _>>(),
            "watch_firing": call["watch_firing"].as_bool().unwrap_or(false),
        },
        "notes": notes,
    }))
}
