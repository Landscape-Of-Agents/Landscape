// Subagents of a flow: the typed, configured form of a watcher (flow/watch.rs).
//
// A subagent is created by a person or by an agent, and is of one subtype:
// - Monitor: holds the monitoring of the flow. Crane runs its checks (trace drift, required tests,
//   denials) without a model; its actions read observability metrics (Datadog, Grafana), open a
//   meeting (Google Meet), page on-call, activate the flow's Incident subagents, and tell the parent
//   session and the flow's people.
// - Incident: works an incident of the flow. Its actions read the Jira ticket, write RCA notes and
//   flow context (as proposals a person approves), prepare pull requests, code changes, and reviews
//   on the designated branch, and tell the flow's people.
//
// A flow holds at most 3 subagents (pending or active), and at most one of them created by an
// agent. A subagent is constrained to its flow: its targets are the flow and the zones it runs
// through, its autonomy ceiling is at most the flow's, and every action it may take needs no more
// than that ceiling. A configuration that asks for more fails with an error that says what to raise
// first (the flow's ceiling, in .crane/governance.yaml). Only a person's manual trigger reaches
// beyond the flow, for that one run, and it is recorded.
//
// Creating a subagent or changing its configuration waits for a person's approval (in Landscape, or
// 'crane flow subagent approve'), which checks the bounds again. A subagent has standing context and
// actions of its own but no volatile context: when it runs, the flow's common context is its
// volatile context. When its flow or one of its zones is removed, the subagent is terminated.
//
// Actions run for real where Crane has the integration: Slack messages, reading a Jira issue, and
// context or RCA proposals. Datadog, Grafana, Google Meet, on-call paging, and GitHub pull requests
// have connection settings but are not called yet: those actions are recorded as prepared (or not
// connected), never pretended.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::model::FlowBook;
use super::watch::{self, Incident, Proposal, Watcher};
use crate::governance::workspace::Workspace;
use crate::governance::zones::Ceiling;

/** The subtypes a subagent is one of */
pub(crate) const SUBTYPES: &[&str] = &["monitor", "incident"];

/** Most subagents a flow holds, pending or active */
pub(crate) const MAX_PER_FLOW: u32 = 3;

/** Most subagents of one flow an agent may have created, pending or active */
pub(crate) const MAX_AGENT_PER_FLOW: u32 = 1;

/** Most manual runs kept per subagent */
const MAX_RUNS: usize = 20;

/** Largest configuration a subagent takes (bytes of JSON) */
const MAX_CONFIG_BYTES: usize = 16 * 1024;

/** Lifecycle states */
pub(crate) const PENDING: &str = "PENDING_APPROVAL";
pub(crate) const ACTIVE: &str = "ACTIVE";
pub(crate) const STOPPED: &str = "STOPPED";
pub(crate) const TERMINATED: &str = "TERMINATED";

/** An action a subagent may take
 * Fields
    - id: &'static str - name used in configurations
    - subtype: &'static str - the subtype it belongs to
    - label: &'static str - what it does
    - integration: Option<&'static str> - the integration it needs
    - ceiling: Ceiling - the least autonomy it needs
    - runs: &'static str - "crane" (Crane does it), "integration" (calls the service when Crane
      can), or "prepared" (recorded for a person; the service is not called yet)
*/
pub(crate) struct ActionSpec {
    pub(crate) id: &'static str,
    pub(crate) subtype: &'static str,
    pub(crate) label: &'static str,
    pub(crate) integration: Option<&'static str>,
    pub(crate) ceiling: Ceiling,
    pub(crate) runs: &'static str,
}

/** Every action, by subtype */
pub(crate) static ACTIONS: &[ActionSpec] = &[
    ActionSpec {
        id: "watch_checks",
        subtype: "monitor",
        label: "Run the flow's checks (trace drift, required tests, denials) without a model",
        integration: None,
        ceiling: Ceiling::Observe,
        runs: "crane",
    },
    ActionSpec {
        id: "check_metrics",
        subtype: "monitor",
        label: "Read observability metrics and monitors (Datadog, Grafana)",
        integration: Some("datadog|grafana"),
        ceiling: Ceiling::Observe,
        runs: "prepared",
    },
    ActionSpec {
        id: "create_meeting",
        subtype: "monitor",
        label: "Open a Google Meet for the incident",
        integration: Some("google_meet"),
        ceiling: Ceiling::Assisted,
        runs: "prepared",
    },
    ActionSpec {
        id: "page_oncall",
        subtype: "monitor",
        label: "Trigger the on-call status (page the rotation)",
        integration: Some("oncall"),
        ceiling: Ceiling::Assisted,
        runs: "prepared",
    },
    ActionSpec {
        id: "activate_incident",
        subtype: "monitor",
        label: "Activate the flow's Incident subagents",
        integration: None,
        ceiling: Ceiling::Observe,
        runs: "crane",
    },
    ActionSpec {
        id: "notify_parent",
        subtype: "monitor",
        label: "Tell the parent agent session (with its next tool call)",
        integration: None,
        ceiling: Ceiling::Observe,
        runs: "crane",
    },
    ActionSpec {
        id: "notify_people",
        subtype: "monitor",
        label: "Message the flow's people on Slack",
        integration: Some("slack"),
        ceiling: Ceiling::Observe,
        runs: "integration",
    },
    ActionSpec {
        id: "read_jira_ticket",
        subtype: "incident",
        label: "Read the Jira ticket of the task",
        integration: Some("jira"),
        ceiling: Ceiling::Observe,
        runs: "integration",
    },
    ActionSpec {
        id: "write_rca",
        subtype: "incident",
        label: "Write RCA notes as flow context (a proposal a person approves)",
        integration: None,
        ceiling: Ceiling::Assisted,
        runs: "crane",
    },
    ActionSpec {
        id: "update_flow_context",
        subtype: "incident",
        label: "Update the flow's context (a proposal a person approves)",
        integration: None,
        ceiling: Ceiling::Assisted,
        runs: "crane",
    },
    ActionSpec {
        id: "open_pull_request",
        subtype: "incident",
        label: "Open a pull request on the designated branch",
        integration: Some("github"),
        ceiling: Ceiling::Delegated,
        runs: "prepared",
    },
    ActionSpec {
        id: "propose_code_change",
        subtype: "incident",
        label: "Propose a code change on the designated branch",
        integration: Some("github"),
        ceiling: Ceiling::Delegated,
        runs: "prepared",
    },
    ActionSpec {
        id: "review_edits",
        subtype: "incident",
        label: "Review edits made during the incident",
        integration: Some("github"),
        ceiling: Ceiling::Assisted,
        runs: "prepared",
    },
    ActionSpec {
        id: "notify_people",
        subtype: "incident",
        label: "Message the flow's people on Slack",
        integration: Some("slack"),
        ceiling: Ceiling::Observe,
        runs: "integration",
    },
];

/** What a subagent is set up to do
 * Fields
    - actions: Vec<String> - actions it may take (ACTIONS of its subtype)
    - context: Vec<String> - standing context of its own (not volatile; the flow's common context
      is its volatile context)
    - parameters: Map<String, Value> - values its actions use (jira_issue, metrics, branch base, ...)
    - integrations: Vec<String> - the integrations it uses
    - branch: Option<String> - the designated branch (Incident)
    - ceiling: String - its autonomy ceiling (at most the flow's)
    - targets: Vec<String> - what it acts on (the flow and its zones only)
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SubagentConfig {
    pub(crate) actions: Vec<String>,
    pub(crate) context: Vec<String>,
    pub(crate) parameters: Map<String, Value>,
    pub(crate) integrations: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) branch: Option<String>,
    pub(crate) ceiling: String,
    pub(crate) targets: Vec<String>,
}

/** A change waiting for a person's approval
 * Fields
    - kind: String - create or configure
    - purpose: Option<String> - the new purpose
    - checks: Option<Vec<String>> - the new checks
    - config: SubagentConfig - the configuration it would have
    - requested_by: String - a person's name, or agent:SESSION
    - requested_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PendingChange {
    pub(crate) kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) purpose: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) checks: Option<Vec<String>>,
    pub(crate) config: SubagentConfig,
    pub(crate) requested_by: String,
    pub(crate) requested_at: u64,
}

/** One run of a subagent's actions: triggered by a person (a manual trigger, which may reach beyond
 * the flow for this run), or by an incident
 * Fields
    - run_id: String - identifier ("run_…")
    - by: String - the person, or incident:ID
    - at: u64 - Unix milliseconds
    - reason: String - why
    - beyond: Vec<String> - targets outside the flow reached in this run only
    - outcomes: Vec<Value> - one {action, result, detail} per action
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Run {
    pub(crate) run_id: String,
    pub(crate) by: String,
    pub(crate) at: u64,
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) beyond: Vec<String>,
    pub(crate) outcomes: Vec<Value>,
}

/** A subagent's state: PENDING_APPROVAL, ACTIVE, STOPPED, or TERMINATED (a watcher written before
 * subagents had states reads as ACTIVE or STOPPED)
 * Input
    - watcher: &Watcher - subagent
 * Output
    - &str
*/
pub(crate) fn status(watcher: &Watcher) -> &str {
    match watcher.status.as_str() {
        "" if watcher.active => ACTIVE,
        "" => STOPPED,
        other => other,
    }
}

/** Report whether a subagent takes one of its flow's places (pending or active)
 * Input
    - watcher: &Watcher - subagent
 * Output
    - bool
*/
pub(crate) fn holds_place(watcher: &Watcher) -> bool {
    matches!(status(watcher), PENDING | ACTIVE)
}

/** The subtype of a subagent ("monitor" for a watcher written before subtypes)
 * Input
    - watcher: &Watcher - subagent
 * Output
    - &str
*/
pub(crate) fn subtype(watcher: &Watcher) -> &str {
    if watcher.subtype.is_empty() {
        "monitor"
    } else {
        &watcher.subtype
    }
}

/** Check a subtype name
 * Input
    - text: &str - candidate
 * Output
    - Result<String, String>
*/
pub(crate) fn parse_subtype(text: &str) -> Result<String, String> {
    let name = text.trim().to_ascii_lowercase();
    if SUBTYPES.contains(&name.as_str()) {
        Ok(name)
    } else {
        Err(format!(
            "choose the subagent's subtype: --type {} (it is required)",
            SUBTYPES.join("|")
        ))
    }
}

/** Read a ceiling name
 * Input
    - text: &str - observe, assisted, delegated, or autonomous
 * Output
    - Result<Ceiling, String>
*/
fn ceiling_of(text: &str) -> Result<Ceiling, String> {
    serde_json::from_value(json!(text.trim().to_ascii_lowercase()))
        .map_err(|_| format!("ceiling '{text}' is one of observe, assisted, delegated, autonomous"))
}

/** The actions of a subtype
 * Input
    - subtype: &str - monitor or incident
 * Output
    - impl Iterator<Item = &'static ActionSpec>
*/
fn actions_of(subtype: &str) -> impl Iterator<Item = &'static ActionSpec> + '_ {
    ACTIONS
        .iter()
        .filter(move |action| action.subtype == subtype)
}

/** Where a flow stands: its ceiling and the targets a subagent of it may have
 * Fields
    - ceiling: Ceiling - the flow's autonomy ceiling
    - targets: BTreeSet<String> - "flow:ID" and "zone:STAGE" for each of its stages
*/
pub(crate) struct FlowBounds {
    pub(crate) ceiling: Ceiling,
    pub(crate) targets: BTreeSet<String>,
}

/** Read a flow's bounds from .crane/governance.yaml
 * Input
    - workspace: &Workspace - repository
    - flow: &str - flow
 * Output
    - Result<FlowBounds, String>
    - Error when the flow is not declared
*/
pub(crate) fn bounds_of(workspace: &Workspace, flow: &str) -> Result<FlowBounds, String> {
    let governance = crate::governance::zones::load(workspace)?;
    let declared = governance
        .flows
        .iter()
        .find(|candidate| candidate.id == flow)
        .ok_or_else(|| format!("{flow} is not a flow of .crane/governance.yaml"))?;
    let mut targets = BTreeSet::from([format!("flow:{flow}")]);
    targets.extend(
        declared
            .stages
            .iter()
            .map(|stage| format!("zone:{}", stage.id)),
    );
    Ok(FlowBounds {
        ceiling: declared.autonomy_ceiling,
        targets,
    })
}

/** The configuration a new subagent of a subtype starts with: every action of the subtype its flow
 * allows, the flow as its only target, and the flow's ceiling
 * Input
    - subtype: &str - monitor or incident
    - flow: &str - flow
    - bounds: &FlowBounds - the flow's bounds
 * Output
    - SubagentConfig
*/
pub(crate) fn default_config(subtype: &str, flow: &str, bounds: &FlowBounds) -> SubagentConfig {
    let actions = actions_of(subtype)
        .filter(|action| action.ceiling <= bounds.ceiling)
        .map(|action| action.id.to_string())
        .collect::<Vec<_>>();
    let mut integrations = actions_of(subtype)
        .filter(|action| actions.iter().any(|id| id == action.id))
        .filter_map(|action| action.integration)
        .flat_map(|names| names.split('|'))
        .map(str::to_string)
        .collect::<Vec<_>>();
    integrations.sort();
    integrations.dedup();
    SubagentConfig {
        actions,
        integrations,
        ceiling: bounds.ceiling.name().to_string(),
        targets: vec![format!("flow:{flow}")],
        ..SubagentConfig::default()
    }
}

/** Check a configuration against its subtype and its flow: every way it asks for more than the
 * flow allows, so a person sees all of them at once
 * Input
    - subtype: &str - monitor or incident
    - config: &SubagentConfig - configuration
    - checks: &[String] - the Crane-run checks it has
    - bounds: &FlowBounds - the flow's bounds
 * Output
    - Result<(), String> - the problems, one per line, ending with how to resolve them
*/
pub(crate) fn check(
    subtype: &str,
    config: &SubagentConfig,
    checks: &[String],
    bounds: &FlowBounds,
) -> Result<(), String> {
    let mut problems = Vec::new();
    let ceiling = match ceiling_of(&config.ceiling) {
        Ok(ceiling) => Some(ceiling),
        Err(error) => {
            problems.push(error);
            None
        }
    };
    if let Some(ceiling) = ceiling {
        if ceiling > bounds.ceiling {
            problems.push(format!(
                "its ceiling {} is above the flow's ceiling {}",
                ceiling.name(),
                bounds.ceiling.name()
            ));
        }
    }
    let allowed = ceiling.unwrap_or(bounds.ceiling).min(bounds.ceiling);
    let mut wanted = BTreeSet::new();
    for id in &config.actions {
        match actions_of(subtype).find(|action| action.id == id) {
            None => problems.push(format!(
                "action '{id}' is not an action of a {subtype} subagent ({})",
                actions_of(subtype)
                    .map(|action| action.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
            Some(action) if action.ceiling > allowed => problems.push(format!(
                "action '{id}' needs the {} ceiling, more than the {} it may have",
                action.ceiling.name(),
                allowed.name()
            )),
            Some(action) => {
                if let Some(names) = action.integration {
                    wanted.extend(names.split('|'));
                }
            }
        }
    }
    if config.actions.is_empty() {
        problems.push("give it at least one action".into());
    }
    for name in &config.integrations {
        if !wanted.contains(name.as_str()) {
            problems.push(format!(
                "integration '{name}' is not used by any of its actions"
            ));
        }
    }
    for target in &config.targets {
        if !bounds.targets.contains(target) {
            problems.push(format!(
                "target '{target}' is outside the flow (it may act on {})",
                bounds
                    .targets
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    if config.targets.is_empty() {
        problems.push("give it the flow as a target".into());
    }
    match (subtype, &config.branch) {
        ("monitor", Some(_)) => {
            problems.push("only an Incident subagent has a designated branch".into())
        }
        (_, Some(branch))
            if branch.is_empty()
                || branch.starts_with('-')
                || branch.contains("..")
                || branch.chars().any(|character| {
                    character.is_whitespace() || "~^:?*[\\".contains(character)
                }) =>
        {
            problems.push(format!("branch '{branch}' is not a valid branch name"))
        }
        _ => {}
    }
    if subtype == "monitor"
        && checks.is_empty()
        && config.actions.iter().any(|id| id == "watch_checks")
    {
        problems.push("a Monitor that runs the flow's checks needs at least one check".into());
    }
    for item in &config.context {
        if crate::telemetry::redact::redact(item).1 > 0 {
            problems.push("a context item looks like it holds a secret".into());
        }
    }
    let size = serde_json::to_vec(config)
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    if size > MAX_CONFIG_BYTES {
        problems.push(format!(
            "the configuration is {size} bytes, above {MAX_CONFIG_BYTES}"
        ));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the subagent asks for more than its flow allows:\n- {}\nA subagent stays within its flow. To give it more, raise the flow's autonomy ceiling (or add the zone to the flow) in .crane/governance.yaml first, then configure the subagent again.",
            problems.join("\n- ")
        ))
    }
}

/** Merge a JSON patch into a configuration: a field given replaces it, null resets it, and fields
 * not given stay (parameters merge key by key)
 * Input
    - base: &SubagentConfig - current configuration
    - patch: &Value - a JSON object
 * Output
    - Result<SubagentConfig, String>
*/
pub(crate) fn merge(base: &SubagentConfig, patch: &Value) -> Result<SubagentConfig, String> {
    let patch = patch
        .as_object()
        .ok_or("the configuration must be a JSON object")?;
    let known = [
        "actions",
        "context",
        "parameters",
        "integrations",
        "branch",
        "ceiling",
        "targets",
    ];
    if let Some(unknown) = patch.keys().find(|key| !known.contains(&key.as_str())) {
        return Err(format!(
            "unknown configuration field '{unknown}' (use {})",
            known.join(", ")
        ));
    }
    let mut merged = serde_json::to_value(base).map_err(crate::platform::io_error)?;
    let object = merged
        .as_object_mut()
        .ok_or("configuration is not an object")?;
    for (key, value) in patch {
        if key == "parameters" {
            if let (Some(into), Some(from)) = (
                object.get_mut("parameters").and_then(Value::as_object_mut),
                value.as_object(),
            ) {
                for (name, item) in from {
                    if item.is_null() {
                        into.remove(name);
                    } else {
                        into.insert(name.clone(), item.clone());
                    }
                }
                continue;
            }
        }
        if value.is_null() {
            object.remove(key);
        } else {
            object.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(merged).map_err(|error| format!("invalid configuration: {error}"))
}

/** Check that a flow has room for one more subagent: at most MAX_PER_FLOW pending or active (and
 * the flow's own lower limit), at most MAX_AGENT_PER_FLOW of them created by an agent
 * Input
    - book: &FlowBook - flow knowledge (the flow's limit)
    - existing: &[Watcher] - subagents of this machine
    - flow: &str - flow
    - by_agent: bool - an agent creates it
 * Output
    - Result<(), String>
*/
pub(crate) fn has_room(
    book: &FlowBook,
    existing: &[Watcher],
    flow: &str,
    by_agent: bool,
) -> Result<(), String> {
    let holding = existing
        .iter()
        .filter(|watcher| watcher.flow == flow && holds_place(watcher))
        .collect::<Vec<_>>();
    let limit = book.watcher_limit(flow).min(MAX_PER_FLOW);
    if holding.len() as u32 >= limit {
        return Err(format!(
            "flow {flow} already has {} subagent(s) pending or active, its limit of {limit} (a flow has at most {MAX_PER_FLOW}); stop one first",
            holding.len()
        ));
    }
    if by_agent {
        let by_agents = holding
            .iter()
            .filter(|watcher| watcher.created_by == "agent")
            .count() as u32;
        if by_agents >= MAX_AGENT_PER_FLOW {
            return Err(format!(
                "flow {flow} already has a subagent created by an agent (at most {MAX_AGENT_PER_FLOW}); a person creates more"
            ));
        }
    }
    Ok(())
}

/** Approve a subagent's pending creation or configuration: the bounds are checked again (the flow
 * may have changed), then it becomes active with the new configuration
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - id: &str - subagent
    - by: &str - the person approving
 * Output
    - Result<Watcher, String>
*/
pub(crate) fn approve(
    workspace: &Workspace,
    runtime: &Path,
    id: &str,
    by: &str,
) -> Result<Watcher, String> {
    let snapshot = watch::watchers(runtime);
    let current = snapshot
        .iter()
        .find(|watcher| watcher.watcher_id == id)
        .ok_or_else(|| format!("{id} is not a subagent of this machine"))?;
    let change = current
        .pending
        .clone()
        .ok_or_else(|| format!("{id} has nothing waiting for approval"))?;
    let bounds = bounds_of(workspace, &current.flow)?;
    let checks = change
        .checks
        .clone()
        .unwrap_or_else(|| current.checks.clone());
    check(subtype(current), &change.config, &checks, &bounds)?;
    let mut approved = None;
    watch::update_watchers(runtime, |all| {
        let watcher = all
            .iter_mut()
            .find(|watcher| watcher.watcher_id == id)
            .ok_or_else(|| format!("{id} is not a subagent of this machine"))?;
        if matches!(status(watcher), STOPPED | TERMINATED) {
            return Err(format!(
                "{id} is {}; it cannot be approved",
                status(watcher)
            ));
        }
        let change = watcher
            .pending
            .take()
            .ok_or_else(|| format!("{id} has nothing waiting for approval"))?;
        if let Some(purpose) = change.purpose {
            watcher.purpose = purpose;
        }
        if let Some(checks) = change.checks {
            watcher.checks = checks;
        }
        watcher.config = change.config;
        watcher.status = ACTIVE.into();
        watcher.active = true;
        watcher.approved_by = Some(by.to_string());
        approved = Some(watcher.clone());
        Ok(())
    })?;
    let watcher = approved.ok_or("the subagent was not approved")?;
    crate::platform::notes::note(
        "FLOW_SUBAGENT_APPROVED",
        json!({"flow": watcher.flow, "watcher": id, "subtype": subtype(&watcher), "kind": change.kind, "by": "person"}),
    );
    Ok(watcher)
}

/** Reject a subagent's pending change: a pending creation ends the subagent, a pending
 * configuration is dropped and the subagent keeps the one it had
 * Input
    - runtime: &Path - runtime directory
    - id: &str - subagent
    - by: &str - the person rejecting
 * Output
    - Result<Watcher, String>
*/
pub(crate) fn reject(runtime: &Path, id: &str, by: &str) -> Result<Watcher, String> {
    let mut rejected = None;
    watch::update_watchers(runtime, |all| {
        let watcher = all
            .iter_mut()
            .find(|watcher| watcher.watcher_id == id)
            .ok_or_else(|| format!("{id} is not a subagent of this machine"))?;
        let change = watcher
            .pending
            .take()
            .ok_or_else(|| format!("{id} has nothing waiting for approval"))?;
        if change.kind == "create" {
            watcher.status = STOPPED.into();
            watcher.active = false;
            watcher.stopped_by = Some(by.to_string());
        }
        rejected = Some(watcher.clone());
        Ok(())
    })?;
    let watcher = rejected.ok_or("nothing was rejected")?;
    crate::platform::notes::note(
        "FLOW_SUBAGENT_APPROVED",
        json!({"flow": watcher.flow, "watcher": id, "subtype": subtype(&watcher), "kind": "rejected", "by": "person"}),
    );
    Ok(watcher)
}

/** Terminate the subagents whose flow, or one of whose zones, is no longer in
 * .crane/governance.yaml (after a flow or zone is removed)
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
 * Output
    - Result<Vec<(String, String)>, String> - the subagents terminated, with why
*/
pub(crate) fn terminate_orphans(
    workspace: &Workspace,
    runtime: &Path,
) -> Result<Vec<(String, String)>, String> {
    let governance = crate::governance::zones::load(workspace)?;
    let flows = governance
        .flows
        .iter()
        .map(|flow| flow.id.clone())
        .collect::<BTreeSet<_>>();
    let zones = governance
        .zones
        .iter()
        .map(|zone| zone.id.clone())
        .collect::<BTreeSet<_>>();
    if !watch::watchers(runtime)
        .iter()
        .any(|watcher| !matches!(status(watcher), STOPPED | TERMINATED))
    {
        return Ok(Vec::new());
    }
    let mut ended = Vec::new();
    watch::update_watchers(runtime, |all| {
        for watcher in all
            .iter_mut()
            .filter(|watcher| !matches!(status(watcher), STOPPED | TERMINATED))
        {
            let missing_zone = watcher
                .inherited
                .stages
                .iter()
                .chain(
                    watcher
                        .config
                        .targets
                        .iter()
                        .filter_map(|target| target.strip_prefix("zone:"))
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                        .iter(),
                )
                .find(|zone| !zones.contains(*zone))
                .cloned();
            let why = if !flows.contains(&watcher.flow) {
                Some(format!("flow {} was removed", watcher.flow))
            } else {
                missing_zone.map(|zone| format!("zone {zone} of flow {} was removed", watcher.flow))
            };
            if let Some(why) = why {
                watcher.status = TERMINATED.into();
                watcher.active = false;
                watcher.pending = None;
                watcher.firing.clear();
                watcher.ended = Some(why.clone());
                ended.push((watcher.watcher_id.clone(), why));
            }
        }
        Ok(())
    })?;
    for (id, why) in &ended {
        crate::platform::notes::note(
            "FLOW_SUBAGENT_TERMINATED",
            json!({"watcher": id, "reason": why}),
        );
    }
    Ok(ended)
}

/** Run a subagent's actions once: from a person's manual trigger (which may reach targets beyond
 * the flow for this run only) or from an incident of its flow. Actions run for real where Crane
 * has the integration; the others are recorded as prepared or not connected.
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - book: &FlowBook - flow knowledge (the flow's common context and people)
    - id: &str - subagent
    - by: &str - the person, or incident:ID
    - reason: &str - why
    - beyond: &[String] - targets outside the flow (a person's manual trigger only)
    - incident: Option<&Incident> - the incident that started it
 * Output
    - Result<Run, String>
*/
#[allow(clippy::too_many_arguments)]
pub(crate) fn run(
    workspace: &Workspace,
    runtime: &Path,
    book: &FlowBook,
    id: &str,
    by: &str,
    reason: &str,
    beyond: &[String],
    incident: Option<&Incident>,
) -> Result<Run, String> {
    let watcher = watch::watchers(runtime)
        .into_iter()
        .find(|watcher| watcher.watcher_id == id)
        .ok_or_else(|| format!("{id} is not a subagent of this machine"))?;
    if status(&watcher) != ACTIVE {
        return Err(format!(
            "{id} is {}; only an active subagent runs",
            status(&watcher)
        ));
    }
    let connected = crate::integrations::configured(workspace)
        .into_iter()
        .map(|integration| integration.name)
        .collect::<BTreeSet<_>>();
    // The flow's common context is the subagent's volatile context
    let common = book
        .flows
        .get(&watcher.flow)
        .map(|knowledge| {
            knowledge
                .context
                .iter()
                .map(|item| format!("[{}] {}: {}", item.kind.name(), item.title, item.value))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let headline = match incident {
        Some(incident) => format!(
            "{} on flow {}: {}",
            incident.code, watcher.flow, incident.summary
        ),
        None => format!(
            "subagent {id} of flow {} was triggered by {by}: {reason}",
            watcher.flow
        ),
    };
    let mut outcomes = Vec::new();
    for action in &watcher.config.actions {
        let spec = actions_of(subtype(&watcher)).find(|spec| spec.id == action);
        let needs = spec.and_then(|spec| spec.integration).unwrap_or("");
        let have = needs.split('|').find(|name| connected.contains(*name));
        let (result, detail) = match action.as_str() {
            "watch_checks" => (
                "RUNS",
                "the checks run with every background sync".to_string(),
            ),
            "notify_people" => match have {
                Some(channel) => {
                    let sent = crate::integrations::deliver_now(
                        workspace,
                        channel,
                        &format!("Crane: {headline}"),
                    );
                    (
                        "SENT",
                        sent["result"].as_str().unwrap_or_default().to_string(),
                    )
                }
                None => (
                    "NOT_CONNECTED",
                    "connect Slack with 'crane integrate slack'".to_string(),
                ),
            },
            "notify_parent" => {
                if watcher.parent.session.is_empty() {
                    (
                        "SKIPPED",
                        "a person created it; there is no parent session".to_string(),
                    )
                } else {
                    watch::notify_session(
                        runtime,
                        &watcher.parent.session,
                        &format!("Subagent {id}: {headline}"),
                    )?;
                    ("SENT", format!("session {}", watcher.parent.session))
                }
            }
            "activate_incident" => {
                let woken = watch::watchers(runtime)
                    .into_iter()
                    .filter(|other| {
                        other.flow == watcher.flow
                            && subtype(other) == "incident"
                            && status(other) == ACTIVE
                    })
                    .map(|other| other.watcher_id)
                    .collect::<Vec<_>>();
                if woken.is_empty() {
                    (
                        "SKIPPED",
                        "the flow has no active Incident subagent".to_string(),
                    )
                } else {
                    ("ACTIVATED", woken.join(", "))
                }
            }
            "read_jira_ticket" => {
                let key = watcher
                    .config
                    .parameters
                    .get("jira_issue")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                match (have, key.is_empty()) {
                    (None, _) => (
                        "NOT_CONNECTED",
                        "connect Jira with 'crane integrate jira'".to_string(),
                    ),
                    (_, true) => (
                        "SKIPPED",
                        "set parameters.jira_issue to the ticket's label".to_string(),
                    ),
                    (Some(_), false) => (
                        "READ",
                        format!("{:?}", crate::integrations::find_issue(workspace, key)),
                    ),
                }
            }
            "write_rca" | "update_flow_context" => {
                let title = if action == "write_rca" {
                    format!("RCA: {headline}")
                } else {
                    format!("Context from {id}")
                };
                let value = match incident {
                    Some(incident) => format!(
                        "{}\n\n{}\n\nFlow context at the time:\n{}",
                        incident.summary,
                        incident.detail,
                        common.join("\n")
                    ),
                    None => format!(
                        "{reason}\n\nFlow context at the time:\n{}",
                        common.join("\n")
                    ),
                };
                let proposal = Proposal {
                    proposal_id: watch::new_id("fpr")?,
                    kind: "context".into(),
                    flow: watcher.flow.clone(),
                    payload: json!({"kind": "note", "title": title.chars().take(200).collect::<String>(), "value": value.chars().take(4000).collect::<String>()}),
                    proposed_by: watch::Parent {
                        session: watcher.parent.session.clone(),
                        agent: format!("subagent {id}"),
                        agent_id: id.to_string(),
                        model: None,
                    },
                    proposed_at: crate::platform::now_millis(),
                    status: "PENDING".into(),
                    decided_by: None,
                    decided_at: None,
                    reason: None,
                };
                watch::save_proposal(workspace, &proposal)?;
                (
                    "PROPOSED",
                    format!(
                        "{} waits for a person ('crane flow proposals approve {}')",
                        proposal.proposal_id, proposal.proposal_id
                    ),
                )
            }
            _ => match have {
                Some(name) => (
                    "PREPARED",
                    format!(
                        "{} is connected but Crane does not call it yet; recorded for a person{}",
                        name,
                        watcher
                            .config
                            .branch
                            .as_ref()
                            .map(|branch| format!(" (branch {branch})"))
                            .unwrap_or_default()
                    ),
                ),
                None => (
                    "NOT_CONNECTED",
                    format!(
                        "connect {} with 'crane integrate {}'",
                        needs.replace('|', " or "),
                        needs.split('|').next().unwrap_or_default()
                    ),
                ),
            },
        };
        outcomes.push(json!({"action": action, "result": result, "detail": detail}));
    }
    let run = Run {
        run_id: watch::new_id("run")?,
        by: by.to_string(),
        at: crate::platform::now_millis(),
        reason: reason.to_string(),
        beyond: beyond.to_vec(),
        outcomes,
    };
    watch::update_watchers(runtime, |all| {
        if let Some(watcher) = all.iter_mut().find(|watcher| watcher.watcher_id == id) {
            watcher.runs.push(run.clone());
            let extra = watcher.runs.len().saturating_sub(MAX_RUNS);
            watcher.runs.drain(..extra);
            watcher.last_run = run.at;
        }
        Ok(())
    })?;
    crate::platform::notes::note(
        "FLOW_SUBAGENT_RUN",
        json!({"flow": watcher.flow, "watcher": id, "subtype": subtype(&watcher), "by": if by.starts_with("incident:") { "incident" } else { "person" }, "beyond": beyond.len(), "actions": run.outcomes.len()}),
    );
    Ok(run)
}

/** When an incident is raised on a flow: run the flow's active Monitor subagents (those with
 * actions beyond the checks), then, when one of them activates Incident subagents, the flow's
 * active Incident subagents (best effort; failures are skipped)
 * Input
    - workspace: &Workspace - repository
    - runtime: &Path - runtime directory
    - book: &FlowBook - flow knowledge
    - incident: &Incident - the incident
 * Output
    - None
*/
pub(crate) fn on_incident(
    workspace: &Workspace,
    runtime: &Path,
    book: &FlowBook,
    incident: &Incident,
) {
    let by = format!("incident:{}", incident.incident_id);
    let active = watch::watchers(runtime)
        .into_iter()
        .filter(|watcher| watcher.flow == incident.flow && status(watcher) == ACTIVE)
        .collect::<Vec<_>>();
    let monitors = active
        .iter()
        .filter(|watcher| {
            subtype(watcher) == "monitor"
                && watcher.config.actions.iter().any(|id| id != "watch_checks")
        })
        .collect::<Vec<_>>();
    for monitor in &monitors {
        let _ = run(
            workspace,
            runtime,
            book,
            &monitor.watcher_id,
            &by,
            &incident.summary,
            &[],
            Some(incident),
        );
    }
    if monitors.iter().any(|monitor| {
        monitor
            .config
            .actions
            .iter()
            .any(|id| id == "activate_incident")
    }) {
        for responder in active
            .iter()
            .filter(|watcher| subtype(watcher) == "incident")
        {
            let _ = run(
                workspace,
                runtime,
                book,
                &responder.watcher_id,
                &by,
                &incident.summary,
                &[],
                Some(incident),
            );
        }
    }
}

/** The catalog the dashboard and Landscape show: subtypes, their actions, the caps
 * Input
    - None
 * Output
    - Value
*/
pub(crate) fn catalog() -> Value {
    json!({
        "subtypes": SUBTYPES,
        "max_per_flow": MAX_PER_FLOW,
        "max_agent_per_flow": MAX_AGENT_PER_FLOW,
        "checks": watch::CHECKS,
        "ceilings": ["observe", "assisted", "delegated", "autonomous"],
        "actions": ACTIONS.iter().map(|action| json!({
            "id": action.id, "subtype": action.subtype, "label": action.label,
            "integration": action.integration, "ceiling": action.ceiling.name(), "runs": action.runs,
        })).collect::<Vec<_>>(),
    })
}

/** The integrations configured on this machine, by name, with whether each is connected
 * Input
    - workspace: &Workspace - repository
 * Output
    - BTreeMap<String, bool>
*/
pub(crate) fn connections(workspace: &Workspace) -> BTreeMap<String, bool> {
    let configured = crate::integrations::configured(workspace)
        .into_iter()
        .map(|integration| integration.name)
        .collect::<BTreeSet<_>>();
    [
        "slack",
        "jira",
        "github",
        "whatsapp",
        "datadog",
        "grafana",
        "google_meet",
        "oncall",
    ]
    .iter()
    .map(|name| (name.to_string(), configured.contains(*name)))
    .collect()
}
