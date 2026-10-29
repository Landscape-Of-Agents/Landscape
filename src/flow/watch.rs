// Watchers, incidents, and proposals: the parts of a flow that agents may start by themselves.
//
// A watcher is a monitoring subagent of one flow. An agent starts it through its hooks
// (flow/agent.rs), or a person with 'crane flow watch add'. It takes on its parent's properties:
// the parent session, provider, and model; the flow as its only target; and the Bricks, autonomy
// ceiling, and criticality of that flow, as they stand. It is read-only by construction: Crane runs
// its checks (no model is called, nothing is written but its own results), and they read only the
// flow. The checks: the flow's call graph drifted from the approved trace, a required test is
// failing or missing, and denials in the flow. When a check starts firing, Crane raises an
// incident: it notifies the flow's designated people through the integrations (Slack, the Jira
// issue that names them) and leaves a notice the parent session receives with its next tool call.
// How many watchers agents start is capped by the signed limits (flow/model.rs); only a person
// changes them.
//
// A proposal is a test or a context item an agent offers for a flow. It waits in
// .crane/drafts/flow-proposals (so it travels with the repository) until a person approves it
// ('crane flow proposals approve'), which adds it to the signed flow knowledge, or rejects it.
//
// Watchers and incidents are this machine's runtime (the trust directory, which agents cannot
// read); the dashboard sees them in the status report.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::FlowBook;
use crate::governance::workspace::Workspace;
use crate::platform::files::{append_line, write_atomic, Lock};
use crate::platform::io_error;

/** Most incidents kept per machine (the oldest beyond this are dropped from the file) */
pub(crate) const MAX_INCIDENTS: usize = 500;

/** Most notices waiting for one session */
pub(crate) const MAX_NOTICES: usize = 20;

/** How far back denials are counted (one hour, milliseconds) */
pub(crate) const DENIAL_WINDOW_MS: u64 = 60 * 60 * 1000;

/** Denials in the window that make the denials check fire */
pub(crate) const DENIAL_THRESHOLD: u64 = 5;

/** The checks a watcher runs */
pub(crate) const CHECKS: &[&str] = &["trace_drift", "required_tests", "denials"];

/** The agent session that started a watcher, whose properties it takes on
 * Fields
    - session: String - Crane session id
    - agent: String - provider (claude, codex, ...)
    - agent_id: String - agent identity
    - model: Option<String> - model
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Parent {
    pub(crate) session: String,
    pub(crate) agent: String,
    pub(crate) agent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
}

/** What a watcher takes on from its parent and its flow, at the time it started
 * Fields
    - targets: Vec<String> - the only target ("flow:ID")
    - bricks: Vec<String> - Bricks attached to the flow ("brick:ID@N")
    - ceiling: String - the flow's autonomy ceiling
    - criticality: String - the flow's criticality
    - priority: Option<String> - the flow's priority
    - mode: String - always read_only
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Inherited {
    pub(crate) targets: Vec<String>,
    pub(crate) bricks: Vec<String>,
    pub(crate) ceiling: String,
    pub(crate) criticality: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) priority: Option<String>,
    pub(crate) mode: String,
}

/** A watcher
 * Fields
    - watcher_id: String - identifier ("wat_…")
    - flow: String - the flow it watches
    - purpose: String - why it was started
    - parent: Parent - the session that started it (empty for a person)
    - created_by: String - "agent" or the person's name
    - created_at: u64 - Unix milliseconds
    - active: bool - it runs
    - stopped_by: Option<String> - who stopped it
    - inherited: Inherited - what it took on
    - checks: Vec<String> - the checks it runs
    - firing: Vec<String> - checks firing now
    - last_run: u64 - Unix milliseconds of its last run (0: never)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Watcher {
    pub(crate) watcher_id: String,
    pub(crate) flow: String,
    pub(crate) purpose: String,
    pub(crate) parent: Parent,
    pub(crate) created_by: String,
    pub(crate) created_at: u64,
    pub(crate) active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) stopped_by: Option<String>,
    pub(crate) inherited: Inherited,
    pub(crate) checks: Vec<String>,
    #[serde(default)]
    pub(crate) firing: Vec<String>,
    #[serde(default)]
    pub(crate) last_run: u64,
}

/** An incident a watcher or an agent raised
 * Fields
    - incident_id: String - identifier ("inc_…")
    - flow: String - flow
    - watcher_id: Option<String> - the watcher that raised it
    - code: String - TRACE_DRIFT, REQUIRED_TEST_FAILING, REQUIRED_TEST_MISSING, DENIALS, or
      AGENT_REPORTED
    - summary: String - one line
    - detail: String - what was found
    - raised_by: String - watcher, or agent:SESSION
    - raised_at: u64 - Unix milliseconds
    - notified: Vec<String> - who was told (people ids, parent:SESSION)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Incident {
    pub(crate) incident_id: String,
    pub(crate) flow: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) watcher_id: Option<String>,
    pub(crate) code: String,
    pub(crate) summary: String,
    pub(crate) detail: String,
    pub(crate) raised_by: String,
    pub(crate) raised_at: u64,
    pub(crate) notified: Vec<String>,
}

/** A test or context item an agent proposed for a flow
 * Fields
    - proposal_id: String - identifier ("fpr_…")
    - kind: String - test or context
    - flow: String - flow
    - payload: Value - the test ({path, name, description, assumptions, mock_data, command}) or
      the context item ({kind, title, value, unit, threshold, file, lines})
    - proposed_by: Parent - the session that proposed it
    - proposed_at: u64 - Unix milliseconds
    - status: String - PENDING, APPROVED, or REJECTED
    - decided_by: Option<String> - who decided
    - decided_at: Option<u64> - when
    - reason: Option<String> - why it was rejected
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub(crate) proposal_id: String,
    pub(crate) kind: String,
    pub(crate) flow: String,
    pub(crate) payload: Value,
    pub(crate) proposed_by: Parent,
    pub(crate) proposed_at: u64,
    pub(crate) status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) decided_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) decided_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

/** The runtime folder of flows
 * Input
    - runtime: &Path - runtime directory
 * Output
    - PathBuf
*/
pub(crate) fn folder(runtime: &Path) -> PathBuf {
    runtime.join("flows")
}

/** A new identifier with a prefix
 * Input
    - prefix: &str - "wat", "inc", or "fpr"
 * Output
    - Result<String, String>
*/
pub(crate) fn new_id(prefix: &str) -> Result<String, String> {
    Ok(format!(
        "{prefix}_{}{}",
        crate::trust::crypto::random_id()?,
        crate::trust::crypto::random_id()?
    ))
}

/** Every watcher of this machine
 * Input
    - runtime: &Path - runtime directory
 * Output
    - Vec<Watcher> (empty when there is none or the file does not read)
*/
pub(crate) fn watchers(runtime: &Path) -> Vec<Watcher> {
    fs::read_to_string(folder(runtime).join("watchers.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** Save every watcher of this machine
 * Input
    - runtime: &Path - runtime directory
    - watchers: &[Watcher] - watchers
 * Output
    - Result<(), String>
*/
pub(crate) fn save_watchers(runtime: &Path, watchers: &[Watcher]) -> Result<(), String> {
    fs::create_dir_all(folder(runtime)).map_err(io_error)?;
    let text = serde_json::to_string_pretty(watchers).map_err(io_error)? + "\n";
    write_atomic(&folder(runtime).join("watchers.json"), text.as_bytes())
}

/** Change the watchers of this machine under a lock, so a background run and an agent's request
 * never overwrite each other's changes
 * Input
    - runtime: &Path - runtime directory
    - change: impl FnOnce(&mut Vec<Watcher>) -> Result<T, String> - the change
 * Output
    - Result<T, String>
*/
pub(crate) fn update_watchers<T>(
    runtime: &Path,
    change: impl FnOnce(&mut Vec<Watcher>) -> Result<T, String>,
) -> Result<T, String> {
    fs::create_dir_all(folder(runtime)).map_err(io_error)?;
    let _lock = Lock::acquire(&folder(runtime).join("watchers.lock"))?;
    let mut all = watchers(runtime);
    let value = change(&mut all)?;
    save_watchers(runtime, &all)?;
    Ok(value)
}

/** Check that one more watcher may start: the session's and the flow's limits
 * Input
    - book: &FlowBook - flow knowledge (the signed limits)
    - existing: &[Watcher] - watchers of this machine
    - flow: &str - flow
    - session: Option<&str> - the starting agent session (None for a person, who is not counted
      against the session limit)
 * Output
    - Result<(), String> why it may not start
*/
pub(crate) fn may_start(
    book: &FlowBook,
    existing: &[Watcher],
    flow: &str,
    session: Option<&str>,
) -> Result<(), String> {
    let on_flow = existing
        .iter()
        .filter(|watcher| watcher.active && watcher.flow == flow)
        .count() as u32;
    let limit = book.watcher_limit(flow);
    if on_flow >= limit {
        return Err(format!(
            "flow {flow} already has {on_flow} active watcher(s), its limit; a person raises it with 'crane flow limit {flow} --watchers N' or stops one"
        ));
    }
    if let Some(session) = session {
        let started = existing
            .iter()
            .filter(|watcher| watcher.active && watcher.parent.session == session)
            .count() as u32;
        if started >= book.limits.max_watchers_per_session {
            return Err(format!(
                "this session already started {started} watcher(s), the limit agents may start; a person raises it with 'crane flow limit --session N'"
            ));
        }
    }
    Ok(())
}

/** What a watcher of a flow takes on: the flow as its only target, the Bricks attached to it, its
 * ceiling, criticality, and priority
 * Input
    - workspace: &Workspace - repository
    - book: &FlowBook - flow knowledge
    - flow: &str - flow
 * Output
    - Result<Inherited, String>
    - Error when the flow is not in .crane/governance.yaml
*/
pub(crate) fn inherit(
    workspace: &Workspace,
    book: &FlowBook,
    flow: &str,
) -> Result<Inherited, String> {
    let governance = crate::governance::zones::load(workspace)?;
    let declared = governance
        .flows
        .iter()
        .find(|candidate| candidate.id == flow)
        .ok_or_else(|| format!("{flow} is not a flow of .crane/governance.yaml"))?;
    let target = format!("flow:{flow}");
    let bricks = crate::governance::state::State::load(workspace)
        .ok()
        .and_then(|state| crate::brick::store::BrickStore::of(&state).ok())
        .map(|store| {
            store
                .attachments
                .iter()
                .filter(|attachment| attachment.target.key() == target)
                .map(|attachment| attachment.subject.text())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(Inherited {
        targets: vec![target],
        bricks,
        ceiling: declared.autonomy_ceiling.name().to_string(),
        criticality: serde_json::to_value(declared.criticality)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default(),
        priority: book
            .flows
            .get(flow)
            .and_then(|knowledge| knowledge.priority)
            .map(|priority| priority.name().to_string()),
        mode: "read_only".into(),
    })
}

/** Every incident of this machine, newest first
 * Input
    - runtime: &Path - runtime directory
 * Output
    - Vec<Incident>
*/
pub(crate) fn incidents(runtime: &Path) -> Vec<Incident> {
    let mut found = fs::read_to_string(folder(runtime).join("incidents.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str::<Incident>(line).ok())
        .collect::<Vec<_>>();
    found.reverse();
    found.truncate(MAX_INCIDENTS);
    found
}

/** Raise an incident: keep it, notify the flow's people through the integrations, and leave a
 * notice for the parent session
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - book: &FlowBook - flow knowledge (the people)
    - incident: Incident - the incident (notified is filled in here)
    - parents: &[String] - the parent sessions to tell
 * Output
    - Result<Incident, String> the incident as kept
*/
pub(crate) fn raise(
    workspace: &Workspace,
    runtime: &Path,
    book: &FlowBook,
    mut incident: Incident,
    parents: &[String],
) -> Result<Incident, String> {
    let people = book
        .flows
        .get(&incident.flow)
        .map(|knowledge| knowledge.people.clone())
        .unwrap_or_default();
    // The people on call, else the owners, else everyone named
    let called = ["oncall", "owner"]
        .iter()
        .map(|role| {
            people
                .iter()
                .filter(|person| {
                    serde_json::to_value(person.role)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .as_deref()
                        == Some(*role)
                })
                .collect::<Vec<_>>()
        })
        .find(|found| !found.is_empty())
        .unwrap_or_else(|| people.iter().collect());
    let mentions = called
        .iter()
        .map(|person| match &person.slack {
            Some(slack) if slack.starts_with('U') || slack.starts_with('W') => {
                format!("<@{slack}>")
            }
            _ => person.name.clone(),
        })
        .collect::<Vec<_>>();
    let text = format!(
        "Crane incident on flow {}: {}{}\n{}",
        incident.flow,
        incident.summary,
        if mentions.is_empty() {
            String::new()
        } else {
            format!(" · {}", mentions.join(", "))
        },
        incident.detail
    );
    let issue = called.iter().find_map(|person| {
        person
            .jira
            .as_deref()
            .filter(|key| key.contains('-') && key.chars().any(|c| c.is_ascii_digit()))
    });
    let delivered = crate::integrations::deliver(
        workspace,
        &crate::integrations::Alert {
            text: &text,
            severity: 2,
            issue,
        },
    );
    if !delivered.is_empty() {
        incident
            .notified
            .extend(called.iter().map(|person| person.id.clone()));
    }
    for session in parents.iter().filter(|session| !session.is_empty()) {
        notify_session(
            runtime,
            session,
            &format!("{} ({})", text, incident.incident_id),
        )?;
        incident.notified.push(format!("parent:{session}"));
    }
    fs::create_dir_all(folder(runtime)).map_err(io_error)?;
    append_line(
        &folder(runtime).join("incidents.jsonl"),
        &serde_json::to_string(&incident).map_err(io_error)?,
    )?;
    crate::platform::notes::note(
        "FLOW_INCIDENT_RAISED",
        json!({"flow": incident.flow, "incident": incident.incident_id, "code": incident.code, "watcher": incident.watcher_id, "notified": incident.notified.len(), "result": "FIRING"}),
    );
    Ok(incident)
}

/** The file of the notices waiting for a session
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
 * Output
    - PathBuf
*/
fn notices_file(runtime: &Path, session: &str) -> PathBuf {
    let safe = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    folder(runtime).join("notices").join(format!("{safe}.json"))
}

/** Leave a notice for a session (it receives it with its next tool call)
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
    - text: &str - notice
 * Output
    - Result<(), String>
*/
pub(crate) fn notify_session(runtime: &Path, session: &str, text: &str) -> Result<(), String> {
    let path = notices_file(runtime, session);
    let mut notices: Vec<String> = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    notices.push(text.chars().take(2000).collect());
    let keep = notices.len().saturating_sub(MAX_NOTICES);
    notices.drain(..keep);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    write_atomic(
        &path,
        serde_json::to_string(&notices)
            .map_err(io_error)?
            .as_bytes(),
    )
}

/** Take the notices waiting for a session
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
 * Output
    - Vec<String> (empty when none; they are removed)
*/
pub(crate) fn take_notices(runtime: &Path, session: &str) -> Vec<String> {
    let path = notices_file(runtime, session);
    let notices: Vec<String> = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    if !notices.is_empty() {
        let _ = fs::remove_file(&path);
    }
    notices
}

/** The results of the last test run, by flow and test id
 * Input
    - runtime: &Path - runtime directory
 * Output
    - BTreeMap<String, BTreeMap<String, Value>> {flow: {test: {status, at, detail}}}
*/
pub(crate) fn test_results(runtime: &Path) -> BTreeMap<String, BTreeMap<String, Value>> {
    fs::read_to_string(folder(runtime).join("test-results.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** Save the results of a test run
 * Input
    - runtime: &Path - runtime directory
    - results: &BTreeMap<String, BTreeMap<String, Value>> - results by flow and test
 * Output
    - Result<(), String>
*/
pub(crate) fn save_test_results(
    runtime: &Path,
    results: &BTreeMap<String, BTreeMap<String, Value>>,
) -> Result<(), String> {
    fs::create_dir_all(folder(runtime)).map_err(io_error)?;
    let text = serde_json::to_string_pretty(results).map_err(io_error)? + "\n";
    write_atomic(&folder(runtime).join("test-results.json"), text.as_bytes())
}

/** One finding of a watcher check
 * Fields
    - code: String - TRACE_DRIFT, REQUIRED_TEST_FAILING, REQUIRED_TEST_MISSING, or DENIALS
    - summary: String - one line
    - detail: String - what was found
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) code: String,
    pub(crate) summary: String,
    pub(crate) detail: String,
}

/** Run the checks of one flow, reading only
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - book: &FlowBook - flow knowledge
    - flow: &str - flow
    - index: &mut Option<crate::governance::flows::Index> - the call-graph index, built once per run
    - now: u64 - Unix milliseconds
 * Output
    - Vec<Finding>
*/
pub(crate) fn check_flow(
    workspace: &Workspace,
    runtime: &Path,
    book: &FlowBook,
    flow: &str,
    index: &mut Option<crate::governance::flows::Index>,
    now: u64,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(knowledge) = book.flows.get(flow) else {
        return findings;
    };
    if let Some(approved) = &knowledge.trace {
        if index.is_none() {
            *index = super::trace::index(workspace).ok();
        }
        if let Some(built) = index.as_ref() {
            let now_graph = super::trace::trace_index(built, &approved.from, approved.max_depth);
            if now_graph.digest() != approved.digest {
                let added = now_graph
                    .nodes
                    .iter()
                    .filter(|node| !approved.files.contains(&node.path))
                    .count();
                findings.push(Finding {
                    code: "TRACE_DRIFT".into(),
                    summary: format!("the call graph of {flow} changed since its trace was approved"),
                    detail: format!(
                        "approved: {} functions, {} calls; now: {} functions, {} calls ({} in files outside the approved trace); retrace it with 'crane flow retrace {flow}' and approve the new revision",
                        approved.nodes, approved.edges, now_graph.nodes.len(), now_graph.edges.len(), added
                    ),
                });
            }
        }
    }
    let results = test_results(runtime);
    for test in knowledge.tests.iter().filter(|test| test.required) {
        if !workspace.root.join(&test.path).is_file() {
            findings.push(Finding {
                code: "REQUIRED_TEST_MISSING".into(),
                summary: format!("required test {} of {flow} is missing", test.id),
                detail: format!("{} no longer exists", test.path),
            });
        } else if results
            .get(flow)
            .and_then(|tests| tests.get(&test.id))
            .is_some_and(|result| result["status"] == "FAILED")
        {
            findings.push(Finding {
                code: "REQUIRED_TEST_FAILING".into(),
                summary: format!("required test {} of {flow} failed its last run", test.id),
                detail: format!("{}: {}", test.path, test.name),
            });
        }
    }
    let events = crate::telemetry::Stores::open(runtime)
        .events
        .read_all()
        .unwrap_or_default();
    let (denied, rules) = crate::brick::runtime::denials(
        &events,
        &format!("flow:{flow}"),
        now.saturating_sub(DENIAL_WINDOW_MS),
    );
    if denied >= DENIAL_THRESHOLD {
        findings.push(Finding {
            code: "DENIALS".into(),
            summary: format!("{denied} agent actions were denied in {flow} in the last hour"),
            detail: format!(
                "rules: {}",
                rules
                    .iter()
                    .map(|(rule, count)| format!("{rule} ×{count}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        });
    }
    findings
}

/** Run every active watcher: a check that starts firing raises one incident (notified once); one
 * that stops firing is cleared
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - book: &FlowBook - flow knowledge
    - now: u64 - Unix milliseconds
 * Output
    - Result<Vec<Incident>, String> the incidents raised
*/
pub(crate) fn run(
    workspace: &Workspace,
    runtime: &Path,
    book: &FlowBook,
    now: u64,
) -> Result<Vec<Incident>, String> {
    let snapshot = watchers(runtime);
    if !snapshot.iter().any(|watcher| watcher.active) {
        return Ok(Vec::new());
    }
    // The checks read the repository (slow); they run outside the lock
    let mut index = None;
    let mut by_flow: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    for watcher in snapshot.iter().filter(|watcher| watcher.active) {
        if !by_flow.contains_key(&watcher.flow) {
            let findings = check_flow(workspace, runtime, book, &watcher.flow, &mut index, now);
            by_flow.insert(watcher.flow.clone(), findings);
        }
    }
    // Under the lock: the watchers as they are now (an agent may have started or stopped one)
    let (raising, active) = update_watchers(runtime, |all| {
        let mut raising = Vec::new();
        for watcher in all.iter_mut().filter(|watcher| watcher.active) {
            let Some(findings) = by_flow.get(&watcher.flow) else {
                continue;
            };
            let firing = findings
                .iter()
                .filter(|finding| {
                    watcher
                        .checks
                        .contains(&check_of(&finding.code).to_string())
                })
                .collect::<Vec<_>>();
            let before = watcher.firing.iter().cloned().collect::<BTreeSet<_>>();
            for finding in &firing {
                if !before.contains(&finding.code) {
                    raising.push((watcher.clone(), (*finding).clone()));
                }
            }
            watcher.firing = firing.iter().map(|finding| finding.code.clone()).collect();
            watcher.last_run = now;
        }
        Ok((raising, all.iter().filter(|watcher| watcher.active).count()))
    })?;
    // One incident per flow and problem, however many watchers saw it: its people are told once,
    // and every parent session of those watchers gets the notice
    let mut grouped: BTreeMap<(String, String), (Watcher, Finding, BTreeSet<String>)> =
        BTreeMap::new();
    for (watcher, finding) in raising {
        let entry = grouped
            .entry((watcher.flow.clone(), finding.code.clone()))
            .or_insert_with(|| (watcher.clone(), finding.clone(), BTreeSet::new()));
        if !watcher.parent.session.is_empty() {
            entry.2.insert(watcher.parent.session.clone());
        }
    }
    let mut raised = Vec::new();
    for (_, (watcher, finding, sessions)) in grouped {
        let parents = sessions.into_iter().collect::<Vec<_>>();
        raised.push(raise(
            workspace,
            runtime,
            book,
            Incident {
                incident_id: new_id("inc")?,
                flow: watcher.flow.clone(),
                watcher_id: Some(watcher.watcher_id.clone()),
                code: finding.code,
                summary: finding.summary,
                detail: finding.detail,
                raised_by: "watcher".into(),
                raised_at: now,
                notified: Vec::new(),
            },
            &parents,
        )?);
    }
    crate::platform::notes::note(
        "FLOW_WATCHERS_EVALUATED",
        json!({"watchers": active, "incidents": raised.len(), "result": if raised.is_empty() { "OK" } else { "FIRING" }}),
    );
    Ok(raised)
}

/** The check a finding code belongs to
 * Input
    - code: &str - finding code
 * Output
    - &'static str
*/
fn check_of(code: &str) -> &'static str {
    match code {
        "TRACE_DRIFT" => "trace_drift",
        "REQUIRED_TEST_FAILING" | "REQUIRED_TEST_MISSING" => "required_tests",
        _ => "denials",
    }
}

/** Run the watchers from the background sync (best effort; failures are ignored)
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
 * Output
    - None
*/
pub(crate) fn sync(workspace: &Workspace, runtime: &Path) {
    if !workspace
        .file(crate::governance::state::FLOWS_FILE)
        .exists()
    {
        return;
    }
    let Ok(state) = crate::governance::state::State::load(workspace) else {
        return;
    };
    let Ok(book) = FlowBook::of(&state) else {
        return;
    };
    if !watchers(runtime).iter().any(|watcher| watcher.active) {
        return;
    }
    let now = crate::platform::now_millis();
    let outcome = run(workspace, runtime, &book, now);
    // The event the watchers' notes ride on (F39), as the Brick monitors record theirs
    let mut event = crate::telemetry::events::Event::new(
        "flow.watchers_evaluated",
        crate::telemetry::model::Domain::RecoveryOversight,
        &workspace.repository_id,
        &format!("flow-watchers:{now}"),
    );
    event.payload = match &outcome {
        Ok(raised) => json!({"incidents": raised.len(), "trigger": "SYNC", "inference": "NONE"}),
        Err(error) => {
            json!({"incidents": 0, "trigger": "SYNC", "inference": "NONE", "error": error})
        }
    };
    if let Err(error) = outcome {
        event.reasons.push(error);
    }
    let _ = crate::telemetry::Stores::open(runtime).events.append(event);
}

/** The folder of proposals
 * Input
    - workspace: &Workspace - repository
 * Output
    - PathBuf
*/
fn proposals_folder(workspace: &Workspace) -> PathBuf {
    workspace.crane.join("drafts").join("flow-proposals")
}

/** Save a proposal
 * Input
    - workspace: &Workspace - repository
    - proposal: &Proposal - proposal
 * Output
    - Result<(), String>
*/
pub(crate) fn save_proposal(workspace: &Workspace, proposal: &Proposal) -> Result<(), String> {
    crate::platform::validate_name("proposal", &proposal.proposal_id)?;
    fs::create_dir_all(proposals_folder(workspace)).map_err(io_error)?;
    let text = serde_json::to_string_pretty(proposal).map_err(io_error)? + "\n";
    write_atomic(
        &proposals_folder(workspace).join(format!("{}.json", proposal.proposal_id)),
        text.as_bytes(),
    )
}

/** Every proposal, oldest first
 * Input
    - workspace: &Workspace - repository
 * Output
    - Vec<Proposal>
*/
pub(crate) fn proposals(workspace: &Workspace) -> Vec<Proposal> {
    let Ok(entries) = fs::read_dir(proposals_folder(workspace)) else {
        return Vec::new();
    };
    let mut found = entries
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Proposal>(&text).ok())
        .collect::<Vec<_>>();
    found.sort_by_key(|proposal| (proposal.proposed_at, proposal.proposal_id.clone()));
    found
}
