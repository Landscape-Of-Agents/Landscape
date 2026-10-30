// The agent channel: how an AI agent asks Crane about flows while it works. Agents may run no Crane
// command, so the hook recognizes one shell command, 'crane-request …', before it would run,
// answers it, and stops the command (nothing is executed; it is not counted as a denial or a
// bypass). What an agent may ask is bounded: read a flow's context and routed model, start a trace
// draft, propose a test or a context item, start or stop a watcher within the signed limits, and
// report an incident. Everything an agent starts is a draft, a proposal, or a read-only watcher; a
// person approves what changes the repository's knowledge.

use serde_json::json;

use super::model::{route, ContextKind, FlowBook, Priority};
use super::trace;
use super::watch::{self, Incident, Parent, Proposal, Watcher};
use crate::governance::workspace::Workspace;

/** The program word of the channel */
pub(crate) const PROGRAM: &str = "crane-request";

/** Longest request (characters) */
const MAX_REQUEST: usize = 16 * 1024;

/** Longest single text field (characters) */
const MAX_FIELD: usize = 4 * 1024;

/** What the channel answers to 'crane-request help' */
pub(crate) const HELP: &str = "crane-request flows
crane-request flow ID                       (its context, people, tests, routed model, incidents)
crane-request route --flow ID               (the model this flow's priority routes to)
crane-request trace --from SYMBOL[,SYMBOL] [--flow ID] [--depth N] [--description TEXT]
crane-request retrace ID [--reason TEXT]
crane-request test --flow ID --file PATH --name TEXT --description TEXT [--assumptions 'A; B'] [--mock TEXT] [--command TEXT]
crane-request context --flow ID --kind query|metric|latency|file|note|link --title TEXT --value TEXT [--unit U] [--threshold N] [--file PATH] [--lines A-B]
crane-request watch --flow ID --type monitor|incident --purpose TEXT [--checks trace_drift,required_tests,denials]
  (a subagent of the flow; one per flow from agents, it waits for a person's approval)
crane-request unwatch WATCHER_ID
crane-request incident --flow ID --summary TEXT [--detail TEXT]
Traces, tests, and context are drafts and proposals a person approves; watchers are read-only.";

/** Who is asking
 * Fields
    - session: String - Crane session id
    - agent: String - provider
    - agent_id: String - agent identity
    - model: Option<String> - model
*/
#[derive(Debug, Clone)]
pub(crate) struct Asker {
    pub(crate) session: String,
    pub(crate) agent: String,
    pub(crate) agent_id: String,
    pub(crate) model: Option<String>,
}

impl Asker {
    /** The asker as the parent of what it starts
     * Input
        - None (uses self)
     * Output
        - Parent
    */
    fn parent(&self) -> Parent {
        Parent {
            session: self.session.clone(),
            agent: self.agent.clone(),
            agent_id: self.agent_id.clone(),
            model: self.model.clone(),
        }
    }
}

/** Split a request into words, keeping quoted text whole; shell operators outside quotes make it
 * not a request (so a request cannot carry another command)
 * Input
    - command: &str - the shell command
 * Output
    - Option<Result<Vec<String>, String>>: None when the command is not a request; an error when it
      is one but malformed
*/
pub(crate) fn words(command: &str) -> Option<Result<Vec<String>, String>> {
    let trimmed = command.trim();
    let first = trimmed.split_whitespace().next()?;
    if first.rsplit(['/', '\\']).next() != Some(PROGRAM) {
        return None;
    }
    if trimmed.chars().count() > MAX_REQUEST {
        return Some(Err(format!(
            "the request is longer than {MAX_REQUEST} characters"
        )));
    }
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(open) if c == open => quote = None,
            Some('"') if c == '\\' && matches!(chars.peek(), Some('"' | '\\')) => {
                current.push(chars.next().unwrap_or('\\'))
            }
            Some(_) => current.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            None if "|;&`<>".contains(c) || (c == '$' && chars.peek() == Some(&'(')) => {
                return Some(Err(
                    "a crane-request is one command: no pipes, chaining, redirection, or substitution"
                        .into(),
                ))
            }
            None => current.push(c),
        }
    }
    if quote.is_some() {
        return Some(Err("the request has an unclosed quote".into()));
    }
    if started || !current.is_empty() {
        words.push(current);
    }
    Some(Ok(words))
}

/** Options of a request (--key value) and its positional words
 * Input
    - words: &[String] - words after the verb
    - keys: &[&str] - options it takes
 * Output
    - Result<(Vec<String>, std::collections::BTreeMap<String, String>), String>
*/
fn options(
    words: &[String],
    keys: &[&str],
) -> Result<(Vec<String>, std::collections::BTreeMap<String, String>), String> {
    let mut positional = Vec::new();
    let mut values = std::collections::BTreeMap::new();
    let mut iter = words.iter();
    while let Some(word) = iter.next() {
        if let Some(key) = word.strip_prefix("--") {
            if !keys.contains(&key) {
                return Err(format!(
                    "unknown option --{key} (this request takes {})",
                    keys.iter()
                        .map(|key| format!("--{key}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let value = iter
                .next()
                .ok_or_else(|| format!("--{key} needs a value"))?;
            if value.chars().count() > MAX_FIELD {
                return Err(format!("--{key} is longer than {MAX_FIELD} characters"));
            }
            values.insert(key.to_string(), value.clone());
        } else {
            positional.push(word.clone());
        }
    }
    Ok((positional, values))
}

/** Refuse text that looks like it holds a secret
 * Input
    - what: &str - field
    - text: &str - text
 * Output
    - Result<(), String>
*/
fn no_secret(what: &str, text: &str) -> Result<(), String> {
    if crate::telemetry::redact::redact(text).1 > 0 {
        return Err(format!(
            "{what} looks like it holds a secret; secrets never go into flows"
        ));
    }
    Ok(())
}

/** A repository file named by a request: relative, inside the repository, not Crane's metadata,
 * and existing
 * Input
    - workspace: &Workspace - repository
    - raw: &str - path
 * Output
    - Result<String, String>
*/
fn repository_file(workspace: &Workspace, raw: &str) -> Result<String, String> {
    let path = raw.trim().replace('\\', "/");
    let path = path.trim_start_matches("./").to_string();
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(':')
        || path.split('/').any(|part| part == "..")
        || path == ".crane"
        || path.starts_with(".crane/")
    {
        return Err(format!("'{raw}' is not a file of the repository"));
    }
    if !workspace.root.join(&path).is_file() {
        return Err(format!("{path} does not exist; write the file first"));
    }
    Ok(path)
}

/** Answer a request
 * Input
    - workspace: &Workspace - repository
    - runtime: &std::path::Path - runtime directory
    - asker: &Asker - the asking session
    - words: &[String] - the request's words (the program first)
    - now: u64 - Unix milliseconds
 * Output
    - Result<String, String> the answer, or why the request was refused
*/
pub(crate) fn answer(
    workspace: &Workspace,
    runtime: &std::path::Path,
    asker: &Asker,
    words: &[String],
    now: u64,
) -> Result<String, String> {
    let state = crate::governance::state::State::load(workspace)?;
    let book = FlowBook::of(&state)?;
    let governance = crate::governance::zones::load(workspace)?;
    let verb = words.get(1).map(String::as_str).unwrap_or("help");
    let rest = words.get(2..).unwrap_or_default();
    let flow_exists = |id: &str| -> Result<(), String> {
        if governance.flows.iter().any(|flow| flow.id == id) {
            Ok(())
        } else {
            Err(format!(
                "{id} is not a flow of this repository ('crane-request flows' lists them)"
            ))
        }
    };
    let by = format!("agent:{}", asker.session);
    match verb {
        "help" | "--help" => Ok(HELP.to_string()),
        "flows" => {
            let lines = governance
                .flows
                .iter()
                .map(|flow| {
                    let knowledge = book.flows.get(&flow.id);
                    format!(
                        "{} [{}] {}{}",
                        flow.id,
                        knowledge
                            .and_then(|knowledge| knowledge.priority)
                            .map_or("no priority", Priority::name),
                        flow.description,
                        if knowledge.is_some_and(|knowledge| knowledge.trace.is_some()) {
                            " · traced"
                        } else {
                            ""
                        }
                    )
                })
                .collect::<Vec<_>>();
            Ok(if lines.is_empty() {
                "This repository has no flows yet. Start one with 'crane-request trace --from SYMBOL --flow ID'.".into()
            } else {
                lines.join("\n")
            })
        }
        "flow" => {
            let id = rest.first().ok_or("name the flow: crane-request flow ID")?;
            flow_exists(id)?;
            Ok(super::context::text(
                runtime,
                &book,
                &governance,
                id,
                asker.model.as_deref(),
            ))
        }
        "route" => {
            let (_, values) = options(rest, &["flow"])?;
            let id = values.get("flow").ok_or("name the flow: --flow ID")?;
            flow_exists(id)?;
            let priority = book
                .flows
                .get(id)
                .and_then(|knowledge| knowledge.priority)
                .ok_or_else(|| format!("{id} has no priority yet; a person sets it with 'crane flow priority {id} P0|P1|P2|P3'"))?;
            let (tiers, chosen) = route(&book, priority, Some(&asker.agent));
            Ok(match chosen {
                Some(entry) => format!(
                    "{id} is {}: use {} ({}, {} tier; allowed tiers: {}).",
                    priority.name(),
                    entry.model,
                    entry.provider,
                    entry.tier.name(),
                    tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>().join(", ")
                ),
                None => format!(
                    "{id} is {}, which uses the {} tier(s); the organization lists no available model of them yet.",
                    priority.name(),
                    tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>().join(", ")
                ),
            })
        }
        "trace" => {
            let (_, values) = options(rest, &["from", "flow", "depth", "description"])?;
            let from = values
                .get("from")
                .ok_or("name where the flow starts: --from SYMBOL")?;
            let depth = match values.get("depth") {
                Some(text) => text
                    .parse::<usize>()
                    .map_err(|_| "--depth is a number".to_string())?,
                None => trace::DEFAULT_DEPTH,
            };
            let starts = trace::check_starts(std::slice::from_ref(from), depth)?;
            let id = match values.get("flow") {
                Some(id) => id.clone(),
                None => slug(&starts[0]),
            };
            crate::platform::validate_name("Flow", &id)?;
            let description = values.get("description").cloned().unwrap_or_default();
            no_secret("the description", &description)?;
            let graph = trace::trace(workspace, &starts, depth)?;
            if graph.status == "UNRESOLVED" {
                return Err(format!(
                    "the trace could not start: {}",
                    graph.unresolved.join("; ")
                ));
            }
            let existing = trace::load(workspace, &id)?;
            let draft = trace::add_revision(
                existing,
                &id,
                &description,
                starts,
                depth,
                graph,
                &by,
                "agent",
                "traced by an agent",
                now,
            );
            trace::save(workspace, &draft)?;
            let revision = draft
                .current()
                .cloned()
                .ok_or("the draft has no revision")?;
            crate::platform::notes::note(
                "FLOW_TRACED",
                json!({"flow": id, "revision": revision.revision, "nodes": revision.graph.nodes.len(), "status": revision.graph.status, "via": "agent", "digest": revision.digest}),
            );
            Ok(format!(
                "Draft {id} revision {}: {} functions, {} calls, {} files ({}){}. A code owner or admin reviews and approves it ('crane flow approve {id}'); it grants nothing until then.",
                revision.revision,
                revision.graph.nodes.len(),
                revision.graph.edges.len(),
                revision.graph.files.len(),
                revision.graph.status,
                if revision.graph.unresolved.is_empty() { String::new() } else { format!("; not followed: {}", revision.graph.unresolved.iter().take(5).cloned().collect::<Vec<_>>().join("; ")) }
            ))
        }
        "retrace" => {
            let (positional, values) = options(rest, &["reason"])?;
            let id = positional
                .first()
                .ok_or("name the draft: crane-request retrace ID")?;
            let existing = trace::load(workspace, id)?
                .ok_or_else(|| format!("there is no trace draft of {id}"))?;
            let last = existing
                .current()
                .cloned()
                .ok_or("the draft has no revision")?;
            let graph = trace::trace(workspace, &last.from, last.max_depth)?;
            let changes = trace::diff(&last.graph, &graph);
            let reason = values
                .get("reason")
                .cloned()
                .unwrap_or_else(|| "retraced by an agent".into());
            no_secret("the reason", &reason)?;
            let draft = trace::add_revision(
                Some(existing),
                id,
                "",
                last.from.clone(),
                last.max_depth,
                graph,
                &by,
                "agent",
                &reason,
                now,
            );
            trace::save(workspace, &draft)?;
            crate::platform::notes::note(
                "FLOW_TRACED",
                json!({"flow": id, "revision": draft.current, "via": "agent"}),
            );
            Ok(format!(
                "Draft {id} revision {}: {}.",
                draft.current,
                if changes.is_empty() {
                    "the same functions and calls as before".to_string()
                } else {
                    format!(
                        "{} functions added, {} removed, {} calls added, {} removed",
                        changes.added_nodes.len(),
                        changes.removed_nodes.len(),
                        changes.added_edges.len(),
                        changes.removed_edges.len()
                    )
                }
            ))
        }
        "test" => {
            let (_, values) = options(
                rest,
                &[
                    "flow",
                    "file",
                    "name",
                    "description",
                    "assumptions",
                    "mock",
                    "command",
                ],
            )?;
            let flow = values.get("flow").ok_or("name the flow: --flow ID")?;
            flow_exists(flow)?;
            let path = repository_file(
                workspace,
                values
                    .get("file")
                    .ok_or("name the test file: --file PATH")?,
            )?;
            let name = values
                .get("name")
                .filter(|text| !text.trim().is_empty())
                .ok_or("say what it checks: --name TEXT")?;
            let description = values
                .get("description")
                .filter(|text| !text.trim().is_empty())
                .ok_or("explain it in plain words: --description TEXT")?;
            let assumptions = values
                .get("assumptions")
                .map(|text| {
                    text.split([';', '\n'])
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let mock = values.get("mock").cloned().unwrap_or_default();
            let command = values.get("command").cloned();
            for (what, text) in [
                ("the name", name.as_str()),
                ("the description", description.as_str()),
                ("the mock data", mock.as_str()),
                ("the command", command.as_deref().unwrap_or_default()),
            ] {
                no_secret(what, text)?;
            }
            if assumptions
                .iter()
                .any(|item| crate::telemetry::redact::redact(item).1 > 0)
            {
                return Err("an assumption looks like it holds a secret".into());
            }
            let proposal = Proposal {
                proposal_id: watch::new_id("fpr")?,
                kind: "test".into(),
                flow: flow.clone(),
                payload: json!({"path": path, "name": name.trim(), "description": description.trim(), "assumptions": assumptions, "mock_data": mock.trim(), "command": command}),
                proposed_by: asker.parent(),
                proposed_at: now,
                status: "PENDING".into(),
                decided_by: None,
                decided_at: None,
                reason: None,
            };
            watch::save_proposal(workspace, &proposal)?;
            crate::platform::notes::note(
                "FLOW_PROPOSED",
                json!({"flow": flow, "proposal": proposal.proposal_id, "kind": "test"}),
            );
            Ok(format!(
                "Proposal {} waits for a person: the test {path} for {flow}. It is attached to the flow once approved ('crane flow proposals approve {}'), and can be marked required for every release.",
                proposal.proposal_id, proposal.proposal_id
            ))
        }
        "context" => {
            let (_, values) = options(
                rest,
                &[
                    "flow",
                    "kind",
                    "title",
                    "value",
                    "unit",
                    "threshold",
                    "file",
                    "lines",
                ],
            )?;
            let flow = values.get("flow").ok_or("name the flow: --flow ID")?;
            flow_exists(flow)?;
            let kind = ContextKind::parse(
                values
                    .get("kind")
                    .ok_or("name the kind: --kind query|metric|latency|file|note|link")?,
            )?;
            let title = values
                .get("title")
                .filter(|text| !text.trim().is_empty())
                .ok_or("give it a title: --title TEXT")?;
            let value = values.get("value").cloned().unwrap_or_default();
            let file = values
                .get("file")
                .map(|raw| repository_file(workspace, raw))
                .transpose()?;
            if value.trim().is_empty() && file.is_none() {
                return Err("give a value (--value TEXT) or a file (--file PATH)".into());
            }
            let threshold = values
                .get("threshold")
                .map(|text| {
                    text.parse::<f64>()
                        .map_err(|_| "--threshold is a number".to_string())
                })
                .transpose()?;
            for (what, text) in [("the title", title.as_str()), ("the value", value.as_str())] {
                no_secret(what, text)?;
            }
            let proposal = Proposal {
                proposal_id: watch::new_id("fpr")?,
                kind: "context".into(),
                flow: flow.clone(),
                payload: json!({"kind": kind, "title": title.trim(), "value": value.trim(), "unit": values.get("unit"), "threshold": threshold, "file": file, "lines": values.get("lines")}),
                proposed_by: asker.parent(),
                proposed_at: now,
                status: "PENDING".into(),
                decided_by: None,
                decided_at: None,
                reason: None,
            };
            watch::save_proposal(workspace, &proposal)?;
            crate::platform::notes::note(
                "FLOW_PROPOSED",
                json!({"flow": flow, "proposal": proposal.proposal_id, "kind": "context"}),
            );
            Ok(format!(
                "Proposal {} waits for a person: {} context for {flow}.",
                proposal.proposal_id,
                kind.name()
            ))
        }
        "watch" => {
            let (_, values) = options(rest, &["flow", "purpose", "checks", "type"])?;
            let flow = values.get("flow").ok_or("name the flow: --flow ID")?;
            flow_exists(flow)?;
            let kind = super::subagent::parse_subtype(
                values.get("type").map(String::as_str).unwrap_or_default(),
            )?;
            let purpose = values
                .get("purpose")
                .filter(|text| !text.trim().is_empty())
                .ok_or("say why: --purpose TEXT")?;
            no_secret("the purpose", purpose)?;
            let checks = match values.get("checks") {
                Some(text) => {
                    let wanted = text
                        .split(',')
                        .map(str::trim)
                        .filter(|item| !item.is_empty())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    if let Some(bad) = wanted
                        .iter()
                        .find(|check| !watch::CHECKS.contains(&check.as_str()))
                    {
                        return Err(format!(
                            "check '{bad}' is one of {}",
                            watch::CHECKS.join(", ")
                        ));
                    }
                    wanted
                }
                None if kind == "monitor" => watch::CHECKS
                    .iter()
                    .map(|check| check.to_string())
                    .collect(),
                None => Vec::new(),
            };
            let inherited = watch::inherit(workspace, &book, flow)?;
            // An agent's subagent starts with its flow's default configuration and waits for a
            // person's approval; it can never ask for more than the flow allows
            let bounds = super::subagent::bounds_of(workspace, flow)?;
            let config = super::subagent::default_config(&kind, flow, &bounds);
            super::subagent::check(&kind, &config, &checks, &bounds)?;
            let watcher = Watcher {
                watcher_id: watch::new_id("wat")?,
                flow: flow.clone(),
                purpose: purpose.trim().to_string(),
                parent: asker.parent(),
                created_by: "agent".into(),
                created_at: now,
                active: false,
                inherited,
                checks: checks.clone(),
                subtype: kind.clone(),
                status: super::subagent::PENDING.into(),
                config: config.clone(),
                pending: Some(super::subagent::PendingChange {
                    kind: "create".into(),
                    purpose: Some(purpose.trim().to_string()),
                    checks: Some(checks),
                    config,
                    requested_by: by.clone(),
                    requested_at: now,
                }),
                ..Watcher::default()
            };
            // The limit is checked under the same lock that adds the watcher
            watch::update_watchers(runtime, |all| {
                watch::may_start(&book, all, flow, Some(&asker.session))?;
                all.push(watcher.clone());
                Ok(())
            })?;
            crate::platform::notes::note(
                "FLOW_WATCHER_STARTED",
                json!({"flow": flow, "watcher": watcher.watcher_id, "by": "agent", "checks": watcher.checks, "subtype": kind}),
            );
            Ok(format!(
                "{} subagent {} of {flow} waits for a person's approval in Landscape. It takes on this session's identity and the flow's Bricks ({}), ceiling {}, and acts only within the flow. Once active, Crane tells this session with its next tool call when it finds something.",
                if kind == "monitor" { "Monitor" } else { "Incident" },
                watcher.watcher_id,
                if watcher.inherited.bricks.is_empty() { "none".to_string() } else { watcher.inherited.bricks.join(", ") },
                watcher.inherited.ceiling
            ))
        }
        "unwatch" => {
            let id = rest
                .first()
                .ok_or("name the watcher: crane-request unwatch WATCHER_ID")?;
            watch::update_watchers(runtime, |all| {
                let watcher = all
                    .iter_mut()
                    .find(|watcher| {
                        watcher.watcher_id == *id && super::subagent::holds_place(watcher)
                    })
                    .ok_or_else(|| format!("{id} is not a pending or active subagent"))?;
                if watcher.parent.session != asker.session {
                    return Err(
                        "an agent stops only the watchers its own session started".to_string()
                    );
                }
                watcher.active = false;
                watcher.status = super::subagent::STOPPED.into();
                watcher.pending = None;
                watcher.stopped_by = Some(by.clone());
                Ok(())
            })?;
            crate::platform::notes::note(
                "FLOW_WATCHER_STOPPED",
                json!({"watcher": id, "by": "agent"}),
            );
            Ok(format!("Watcher {id} stopped."))
        }
        "incident" => {
            let (_, values) = options(rest, &["flow", "summary", "detail"])?;
            let flow = values.get("flow").ok_or("name the flow: --flow ID")?;
            flow_exists(flow)?;
            let summary = values
                .get("summary")
                .filter(|text| !text.trim().is_empty())
                .ok_or("say what happened: --summary TEXT")?;
            let detail = values.get("detail").cloned().unwrap_or_default();
            no_secret("the summary", summary)?;
            no_secret("the detail", &detail)?;
            let incident = watch::raise(
                workspace,
                runtime,
                &book,
                Incident {
                    incident_id: watch::new_id("inc")?,
                    flow: flow.clone(),
                    watcher_id: None,
                    code: "AGENT_REPORTED".into(),
                    summary: summary.trim().to_string(),
                    detail: detail.trim().to_string(),
                    raised_by: by.clone(),
                    raised_at: now,
                    notified: Vec::new(),
                },
                &[],
            )?;
            Ok(format!(
                "Incident {} raised on {flow}; told: {}.",
                incident.incident_id,
                if incident.notified.is_empty() {
                    "nobody yet (the flow has no designated people, or no integration is set up)"
                        .to_string()
                } else {
                    incident.notified.join(", ")
                }
            ))
        }
        other => Err(format!(
            "'{other}' is not a request; 'crane-request help' lists them"
        )),
    }
}

/** A flow id made from a starting point ("api/pay.py:Service.submit" becomes "service-submit")
 * Input
    - start: &str - starting point
 * Output
    - String
*/
pub(crate) fn slug(start: &str) -> String {
    let symbol = start.rsplit(':').next().unwrap_or(start);
    let mut slug = String::new();
    for character in symbol.chars() {
        if character.is_ascii_alphanumeric() {
            if character.is_ascii_uppercase() && !slug.is_empty() && !slug.ends_with('-') {
                slug.push('-');
            }
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').chars().take(60).collect::<String>();
    if slug.starts_with(|character: char| character.is_ascii_alphabetic()) {
        slug
    } else {
        format!("flow-{slug}")
    }
}
