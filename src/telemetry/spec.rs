// The Foxx event specification (DASHBOARD.md). Every event the dashboard shows has the same 13
// attribute groups (action, repository, session, agent, resource, response, assessment, decision,
// eval, severity, bindings, telemetry, other), each with compulsory sub-attributes and flow-specific
// ones. Three producers feed them:
//   - machines: every event Crane stores carries its specification events (`Event.spec`), built
//     here by `project` when the event is appended, so they are covered by the event's digest;
//   - the service: sign-in, approvals, upload checks, status-report changes, and its own operation
//     (server/service_events.rs, using `Spec`);
//   - the dashboard: events derived from other records (credit ledger, alerts, audit trail, stale
//     sessions, inferred permission outcomes), built here as well.
// The envelope (`envelope`) adds the identifiers, time, and chain position when they are served.

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::events::{Event, Source};
use super::ledger::{Entry, Kind};
use super::model::{Assessment, Decision, Enforcement, Telemetry};

/** Version of the event specification */
pub(crate) const SPEC_VERSION: u32 = 1;

/** The 13 attribute groups, in order */
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) const GROUPS: [&str; 13] = [
    "action",
    "repository",
    "session",
    "agent",
    "resource",
    "response",
    "assessment",
    "decision",
    "eval",
    "severity",
    "bindings",
    "telemetry",
    "other",
];

/** Rule codes that make a denial a script block (F05.7) */
const SCRIPT_DENY_RULES: &[&str] = &[
    "SCRIPT_NAMES_METADATA",
    "SCRIPT_MODIFIES_PRESERVED_FILE",
    "SCRIPT_DOWNLOAD_EXECUTE",
    "SCRIPT_DECODE_EXECUTE",
    "ENCODED_POWERSHELL",
];

/** Rule codes of the script pre-check that ask a human (F05.8, F05.9) */
const SCRIPT_ASK_RULES: &[&str] = &[
    "SCRIPT_SECRETS_NETWORK",
    "SCRIPT_HISTORY_REWRITE",
    "SCRIPT_TREE_DELETION",
    "SCRIPT_DOWNLOAD_EVALUATE",
    "SCRIPT_NAMES_METADATA",
    "SCRIPT_MODIFIES_PRESERVED_FILE",
    "SCRIPT_DOWNLOAD_EXECUTE",
    "SCRIPT_DECODE_EXECUTE",
];

/** One specification event: its name, flow, and the 13 groups
 * Fields
    - 0: Map<String, Value> - event_name, flow, and one object per group
*/
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spec(Map<String, Value>);

impl Spec {
    /** Start an event with every compulsory sub-attribute at its neutral value
     * Input
        - name: &str - event name, such as TOOL_CALL_ALLOWED
        - flow: &str - flow, such as F05
     * Output
        - Spec
    */
    pub(crate) fn new(name: &str, flow: &str) -> Self {
        let defaults = json!({
            "event_name": name,
            "flow": flow,
            "action": {"action_type": "NA", "action_source": "CLI", "action_user": null, "action_email": null, "action_id": null},
            "repository": {"repo": "UNKNOWN", "repo_id": null, "repo_remote": null, "repo_remote_verified": false, "registry_generation": null},
            "session": {"session_type": "NOT_IN_SESSION", "session_id": null, "provider_session_id": null, "id_provenance": "NA", "task_id": null, "external_task_id": null},
            "agent": {"agent_type": "NOT_AN_AGENT", "agent_id": null, "model": null, "autonomy_mode": "NA"},
            "resource": {"resource": ["NOT_CONSUMED"], "resource_ids": [], "resource_classification": ["NONE"]},
            "response": {"response_status": "OK", "failure_state": "NA", "response_statement": ""},
            "assessment": {"assessment": "NOT_REQUIRED", "assessment_checks": []},
            "decision": {"decision": "NOT_REQUIRED", "decision_rule": null, "decision_reasons": [], "enforcement": "NOT_APPLICABLE"},
            "eval": {"eval": "NOT_REQUIRED"},
            "severity": {"severity_status": "INFO", "severity_basis": "FIXED", "alert_kind": "NONE"},
            "bindings": {"third_party": ["NONE"], "binding_result": "NA"},
            "telemetry": {"agent_state": "INACTIVE", "known_as": "OBSERVED", "crane_version": env!("CARGO_PKG_VERSION"), "hook_event": "NA"},
            "other": {},
        });
        Self(defaults.as_object().cloned().unwrap_or_default())
    }

    /** Set one sub-attribute
     * Input
        - group: &str - group name
        - key: &str - sub-attribute
        - value: impl Serialize - value
     * Output
        - &mut Spec, for chaining
    */
    pub(crate) fn set(&mut self, group: &str, key: &str, value: impl Serialize) -> &mut Self {
        let value = serde_json::to_value(value).unwrap_or(Value::Null);
        if let Some(object) = self.0.get_mut(group).and_then(Value::as_object_mut) {
            object.insert(key.into(), value);
        }
        if group == "eval" && key != "eval" {
            self.set_raw("eval", "eval", json!("MEASURED"));
        }
        self
    }

    /** Set a value without side effects
     * Input
        - group: &str - group
        - key: &str - sub-attribute
        - value: Value - value
     * Output
        - None
    */
    fn set_raw(&mut self, group: &str, key: &str, value: Value) {
        if let Some(object) = self.0.get_mut(group).and_then(Value::as_object_mut) {
            object.insert(key.into(), value);
        }
    }

    /** Read one sub-attribute
     * Input
        - group: &str - group
        - key: &str - sub-attribute
     * Output
        - &Value (Null when absent)
    */
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the service
    pub(crate) fn get(&self, group: &str, key: &str) -> &Value {
        self.0
            .get(group)
            .and_then(|object| object.get(key))
            .unwrap_or(&Value::Null)
    }

    /** Set the response, with a statement of at most ten words
     * Input
        - status: &str - response_status
        - failure: &str - failure_state
        - statement: &str - plain words
     * Output
        - &mut Spec
    */
    pub(crate) fn respond(&mut self, status: &str, failure: &str, statement: &str) -> &mut Self {
        let words = statement
            .split_whitespace()
            .take(10)
            .collect::<Vec<_>>()
            .join(" ");
        self.set("response", "response_status", status)
            .set("response", "failure_state", failure)
            .set("response", "response_statement", words)
    }

    /** Set the severity
     * Input
        - status: &str - severity_status
        - basis: &str - severity_basis
        - alert: &str - alert_kind
     * Output
        - &mut Spec
    */
    pub(crate) fn severity(&mut self, status: &str, basis: &str, alert: &str) -> &mut Self {
        self.set("severity", "severity_status", status)
            .set("severity", "severity_basis", basis)
            .set("severity", "alert_kind", alert)
    }

    /** Turn into JSON
     * Input
        - None (consumes self)
     * Output
        - Value
    */
    pub(crate) fn into_value(self) -> Value {
        Value::Object(self.0)
    }

    /** Read back from JSON
     * Input
        - value: &Value - a specification event
     * Output
        - Option<Spec>
    */
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
    pub(crate) fn from_value(value: &Value) -> Option<Self> {
        value.as_object().cloned().map(Self)
    }
}

/** Upper-case a serde enum name (for example REQUIRE_APPROVAL), or a fallback
 * Input
    - value: impl Serialize - enum value
    - fallback: &str - when it cannot be named
 * Output
    - String
*/
fn upper(value: impl Serialize, fallback: &str) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_ascii_uppercase))
        .unwrap_or_else(|| fallback.into())
}

/** Map a hook event name (session-start) to its specification value (SESSION_START)
 * Input
    - hook: &str - hook event name
 * Output
    - &'static str
*/
fn hook_event(hook: &str) -> &'static str {
    match hook {
        "session-start" => "SESSION_START",
        "user-prompt-submit" => "PROMPT_SUBMIT",
        "pre-tool-use" => "PRE_TOOL_USE",
        "permission-request" => "PERMISSION_REQUEST",
        "post-tool-use" => "POST_TOOL_USE",
        "stop" => "STOP",
        "session-end" => "SESSION_END",
        _ => "NA",
    }
}

/** Map a provider name to an agent type
 * Input
    - provider: Option<&str> - claude, codex, generic, or none
 * Output
    - &'static str
*/
fn agent_type(provider: Option<&str>) -> &'static str {
    match provider {
        Some("claude") => "CLAUDE_CODE",
        Some("codex") => "CODEX",
        Some(_) => "GENERIC",
        None => "NOT_AN_AGENT",
    }
}

/** Map a tool name to the specification's tool list
 * Input
    - tool: &str - tool name as the agent reported it
 * Output
    - &'static str
*/
fn tool(tool: &str) -> &'static str {
    match tool {
        "Read" | "read_file" => "READ",
        "Write" => "WRITE",
        "Edit" => "EDIT",
        "MultiEdit" => "MULTI_EDIT",
        "NotebookEdit" => "NOTEBOOK_EDIT",
        "apply_patch" => "APPLY_PATCH",
        "Bash" | "shell" | "local_shell" | "exec_command" | "container.exec" | "unified_exec" => {
            "BASH"
        }
        "PowerShell" => "POWERSHELL",
        "WebFetch" => "WEB_FETCH",
        "WebSearch" => "WEB_SEARCH",
        "Glob" => "GLOB",
        "Grep" => "GREP",
        "LS" => "LS",
        "Task" => "TASK",
        "Agent" => "AGENT",
        name if name.starts_with("mcp__") => "MCP",
        _ => "OTHER",
    }
}

/** Fill the groups every machine event shares, from the stored event
 * Input
    - event: &Event - stored event
    - name: &str - event name
    - flow: &str - flow
 * Output
    - Spec
*/
pub(super) fn base(event: &Event, name: &str, flow: &str) -> Spec {
    let payload = &event.payload;
    let text = |key: &str| payload.get(key).and_then(Value::as_str).map(String::from);
    let mut spec = Spec::new(name, flow);
    let agent_environment = payload
        .get("agent_environment")
        .is_some_and(|value| !value.is_null());
    let source = match event.source {
        Source::ProviderAdapter => "HOOK",
        Source::FoxxExtension => "EXTENSION",
        Source::Dashboard => "DASHBOARD",
        Source::Ingestion | Source::Integration => "SERVICE",
        Source::Crane if agent_environment => "AGENT",
        Source::Crane if event.environment == "ci" => "CI",
        Source::Crane => "CLI",
    };
    spec.set("action", "action_source", source)
        .set("action", "action_user", text("user"))
        .set("action", "action_email", text("email"))
        .set(
            "action",
            "action_id",
            event
                .scope
                .tool_call_id
                .clone()
                .or_else(|| event.scope.hook_event_id.clone())
                .unwrap_or_else(|| event.idempotency_key.clone()),
        )
        .set("repository", "repo_id", &event.repository_id)
        .set(
            "repository",
            "registry_generation",
            event.bindings.registry_generation,
        );
    if let Some(session) = &event.scope.foxx_session_id {
        let provenance = text("identity_provenance")
            .map(|value| value.to_ascii_uppercase())
            .unwrap_or_else(|| "NA".into());
        spec.set("session", "session_type", "AGENT_SESSION")
            .set("session", "session_id", session)
            .set(
                "session",
                "provider_session_id",
                &event.scope.provider_session_id,
            )
            .set("session", "id_provenance", provenance)
            .set("session", "task_id", &event.scope.foxx_task_id)
            .set("session", "external_task_id", &event.scope.external_task_id);
    } else {
        spec.set("session", "external_task_id", &event.scope.external_task_id);
    }
    let provider = event.scope.provider.as_deref();
    spec.set("agent", "agent_type", agent_type(provider))
        .set("agent", "agent_id", &event.scope.agent_instance_id)
        .set("agent", "model", &event.scope.model)
        .set(
            "agent",
            "autonomy_mode",
            event
                .bindings
                .autonomy_mode
                .as_deref()
                .map_or_else(|| "NA".to_string(), str::to_ascii_uppercase),
        );
    resources(&mut spec, event);
    if let Some(assessment) = event.assessment {
        spec.set(
            "assessment",
            "assessment",
            upper(assessment, "NOT_REQUIRED"),
        );
    }
    if let Some(decision) = event.decision {
        spec.set("decision", "decision", upper(decision, "NOT_REQUIRED"));
    }
    spec.set("decision", "decision_rule", rule(event))
        .set("decision", "decision_reasons", &event.reasons)
        .set(
            "decision",
            "enforcement",
            upper(event.enforcement, "NOT_APPLICABLE"),
        );
    for measurement in &event.measurements {
        if let Some(value) = measurement.value {
            let key = match measurement.name.as_str() {
                "hook_latency_ms" => "latency_ms",
                "command_duration_ms" => "duration_ms",
                other => other,
            };
            spec.set("eval", key, value);
        }
    }
    spec.severity(
        &upper(event.severity, "INFO"),
        "OUTCOME",
        &text("alert")
            .map(|kind| kind.to_ascii_uppercase())
            .unwrap_or_else(|| "NONE".into()),
    );
    let third_party = match provider {
        Some("claude") => vec!["CLAUDE_CODE"],
        Some("codex") => vec!["CODEX"],
        _ => vec!["NONE"],
    };
    spec.set("bindings", "third_party", third_party).set(
        "telemetry",
        "known_as",
        upper(event.telemetry, "OBSERVED"),
    );
    if event.scope.foxx_session_id.is_some() {
        spec.set(
            "telemetry",
            "agent_state",
            event
                .bindings
                .safety
                .clone()
                .unwrap_or_else(|| "ACTIVE".into()),
        );
    }
    spec.set(
        "telemetry",
        "hook_event",
        hook_event(text("hook_event").as_deref().unwrap_or_default()),
    );
    if !event.bindings.zones.is_empty() {
        spec.set("other", "zones", &event.bindings.zones);
    }
    if !event.bindings.policies.is_empty() {
        spec.set("other", "policies", &event.bindings.policies);
    }
    if !event.bindings.selections.is_empty() {
        spec.set("other", "selections", &event.bindings.selections);
    }
    spec
}

/** The decision rule of an event: the first rule of a denial, otherwise the last one recorded
 * Input
    - event: &Event - event
 * Output
    - Option<String>
*/
fn rule(event: &Event) -> Option<String> {
    let rules = event.payload.get("rules").and_then(Value::as_array)?;
    let pick = if event.decision == Some(Decision::Deny) {
        rules.first()
    } else {
        rules.last()
    };
    pick.and_then(Value::as_str).map(String::from)
}

/** Fill the resource group from the event's resources
 * Input
    - spec: &mut Spec - event being built
    - event: &Event - event
 * Output
    - None
*/
fn resources(spec: &mut Spec, event: &Event) {
    if event.resources.is_empty() {
        return;
    }
    let mut kinds = Vec::new();
    let mut classes = Vec::new();
    for resource in &event.resources {
        let kind = match (resource.kind.as_str(), resource.access.as_str()) {
            ("metadata", _) => "CRANE_METADATA",
            ("file", "read") => "FILE_READ",
            ("file", "delete") => "FILE_DELETE",
            ("file", _) => "FILE_WRITE",
            ("command", _) => "COMMAND",
            ("tool", _) => "TOOL",
            ("url" | "host", _) => "NETWORK",
            ("selection", _) => "SELECTION",
            _ => "TOOL",
        };
        if !kinds.contains(&kind) {
            kinds.push(kind);
        }
        let class = match resource.classification.as_deref() {
            Some("selection") => "PROTECTED",
            Some("metadata") => "METADATA",
            Some("trust") => "TRUST",
            Some("external") => "EXTERNAL",
            Some("unattributed") => "UNATTRIBUTED",
            _ => "NONE",
        };
        if !classes.contains(&class) {
            classes.push(class);
        }
    }
    if !event.bindings.zones.is_empty() && !classes.contains(&"ZONED") {
        classes.push("ZONED");
    }
    if classes.len() > 1 {
        classes.retain(|class| *class != "NONE");
    }
    spec.set("resource", "resource", kinds)
        .set(
            "resource",
            "resource_ids",
            event
                .resources
                .iter()
                .map(|resource| resource.id.clone())
                .collect::<Vec<_>>(),
        )
        .set("resource", "resource_classification", classes);
}

/** Project a stored event into its specification events. A single stored event may stand for
 * several (a session start is also its baseline and its context; an allowed call may also carry
 * zone guidance and reach the network)
 * Input
    - event: &Event - event (redacted, sequence assigned)
 * Output
    - Vec<Value> specification events without envelope
*/
pub(crate) fn project(event: &Event) -> Vec<Value> {
    let mut specs = match event.event_type.as_str() {
        "session.created" => session_created(event),
        "session.started" => session_started(event),
        "task.started" => vec![task_started(event)],
        "prompt.suspicious" => vec![prompt_suspicious(event)],
        "tool_call.decided" => decided(event),
        "session.quarantined" => vec![quarantined(event)],
        "credits.low" => vec![credits_low(event)],
        "tool_call.verified" => verified(event),
        "hook.missing_decision" => vec![missing_decision(event)],
        "change.unattributed" => vec![unattributed(event)],
        "task.verified" | "task.verification_failed" => vec![task_result(event)],
        "tool_call.unsettled" => vec![unsettled(event)],
        "session.attested" => attested(event),
        "hook.failed" => vec![hook_failed(event)],
        "command.executed" => super::spec_commands::command(event),
        "foxx.event" => super::spec_commands::foxx(event),
        _ => Vec::new(),
    };
    specs.extend(super::spec_commands::notes(event));
    specs.into_iter().map(Spec::into_value).collect()
}

/** F03.1 */
fn session_created(event: &Event) -> Vec<Spec> {
    let mut spec = base(event, "SESSION_CREATED", "F03");
    spec.set("action", "action_type", "SESSION_START")
        .respond("OK", "NA", "Agent session created.")
        .severity("INFO", "FIXED", "NONE")
        .set(
            "other",
            "environment",
            event.environment.to_ascii_uppercase(),
        );
    vec![spec]
}

/** F03.2 to F03.5 and F17.1 */
fn session_started(event: &Event) -> Vec<Spec> {
    let payload = &event.payload;
    let source = payload
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("startup")
        .to_ascii_uppercase();
    let context = payload
        .get("context")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut specs = Vec::new();
    let mut started = base(event, "SESSION_STARTED", "F03");
    started
        .set("action", "action_type", "SESSION_START")
        .set("resource", "resource", ["CREDITS", "FILE_READ"])
        .set(
            "resource",
            "resource_ids",
            context
                .iter()
                .filter_map(|item| item["path"].as_str().map(String::from))
                .collect::<Vec<_>>(),
        )
        .respond("OK", "NA", "Agent session started with rules and context.")
        .severity("INFO", "FIXED", "NONE")
        .set("other", "start_source", &source);
    specs.push(started);
    if let Some(files) = payload.get("baseline_files").and_then(Value::as_u64) {
        let mut baseline = base(event, "WORKTREE_BASELINE_RECORDED", "F03");
        baseline
            .set("action", "action_type", "BASELINE")
            .set("resource", "resource", ["GIT"])
            .respond("OK", "NA", "Files already modified at start recorded.")
            .severity("INFO", "FIXED", "NONE")
            .set("eval", "modified_files_at_start", files);
        specs.push(baseline);
    }
    let mut supplied = base(event, "SESSION_CONTEXT_SUPPLIED", "F03");
    supplied
        .set("action", "action_type", "SUPPLY_CONTEXT")
        .set("resource", "resource", ["FILE_READ", "SELECTION"])
        .respond("OK", "NA", "Rules, selections, and context given to agent.")
        .severity("INFO", "FIXED", "NONE")
        .set("eval", "context_files_supplied", context.len())
        .set(
            "eval",
            "policies",
            payload.get("policies").and_then(Value::as_u64).unwrap_or(0),
        )
        .set(
            "eval",
            "selections",
            payload
                .get("selections")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
    specs.push(supplied);
    if payload.get("integrity_ok") == Some(&Value::Bool(false)) {
        let mut unverified = base(event, "SESSION_START_UNVERIFIED", "F03");
        unverified
            .set("action", "action_type", "SESSION_START")
            .set("resource", "resource", ["CRANE_METADATA"])
            .respond(
                "DEGRADED_START",
                "VERIFICATION_FAILED",
                "Rules failed verification; changes will be denied.",
            )
            .set("assessment", "assessment", "VIOLATION")
            .set("assessment", "assessment_checks", ["INTEGRITY"])
            .set("decision", "decision", "DENY")
            .set("decision", "decision_rule", "FAIL_CLOSED")
            .set("decision", "enforcement", "PREVENTED")
            .severity("HIGH", "OUTCOME", "NONE")
            .set("telemetry", "agent_state", "ERROR")
            .set("other", "finding_codes", payload.get("finding_codes"));
        specs.push(unverified);
    }
    if matches!(source.as_str(), "RESUME" | "COMPACT" | "CLEAR") {
        let name = match source.as_str() {
            "RESUME" => "SESSION_RESUMED",
            "COMPACT" => "SESSION_COMPACTED",
            _ => "SESSION_CLEARED",
        };
        let mut again = base(event, name, "F17");
        again
            .set("action", "action_type", "SESSION_START")
            .set("resource", "resource", ["FILE_READ", "SELECTION"])
            .respond(
                "CONTEXT_SUPPLIED",
                "NA",
                "Rules and context supplied again.",
            )
            .severity("INFO", "FIXED", "NONE")
            .set("eval", "context_files_supplied", context.len())
            .set("other", "start_source", &source);
        specs.push(again);
    }
    specs
}

/** F04.1 */
fn task_started(event: &Event) -> Spec {
    let mut spec = base(event, "TASK_STARTED", "F04");
    spec.set("action", "action_type", "PROMPT")
        .respond("OK", "NA", "Task opened for the prompt.")
        .severity("INFO", "FIXED", "NONE")
        .set(
            "eval",
            "prompt_length",
            event
                .payload
                .get("prompt_chars")
                .cloned()
                .unwrap_or(json!(0)),
        )
        .set("other", "prompt_digest", event.payload.get("prompt_digest"));
    spec
}

/** F04.2 */
fn prompt_suspicious(event: &Event) -> Spec {
    let mut spec = base(event, "PROMPT_SUSPICIOUS", "F04");
    spec.set("action", "action_type", "PROMPT")
        .respond("RECORDED", "NA", "Prompt resembles an instruction attack.")
        .set("assessment", "assessment", "SUSPICIOUS")
        .set("assessment", "assessment_checks", ["PROMPT_SCREEN"])
        .severity("MEDIUM", "FIXED", "PROMPT_INJECTION")
        .set("other", "prompt_digest", event.payload.get("prompt_digest"));
    spec
}

/** The checks F05 runs, in order */
const PRE_CHECKS: [&str; 10] = [
    "INTEGRITY",
    "SAFETY_STATE",
    "METADATA_PROTECTION",
    "SHELL_ANALYSIS",
    "SCRIPT_SCAN",
    "SIMULATION",
    "MARKERS",
    "ZONE",
    "MODE",
    "CREDITS",
];

/** Fill the action group of a tool call
 * Input
    - spec: &mut Spec - event being built
    - event: &Event - event
    - action_type: &str - action type
 * Output
    - None
*/
fn tool_call(spec: &mut Spec, event: &Event, action_type: &str) {
    let payload = &event.payload;
    let name = payload
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default();
    spec.set("action", "action_type", action_type)
        .set("action", "action_tool", tool(name))
        .set(
            "action",
            "action_operation",
            payload
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("other")
                .to_ascii_uppercase(),
        )
        .set("action", "action_input_digest", payload.get("input_digest"))
        .set("assessment", "assessment_checks", PRE_CHECKS);
    if let Some(network) = payload.get("network").and_then(Value::as_array) {
        if !network.is_empty() {
            spec.set("other", "network_destinations", network);
        }
    }
}

/** F05 (and F16, F27.3, F27.5, F36): the decision before a tool call */
fn decided(event: &Event) -> Vec<Spec> {
    let payload = &event.payload;
    let decision = event.decision.unwrap_or(Decision::Allow);
    let rule = rule(event).unwrap_or_default();
    let operation = payload
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_name = payload
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let bypass = event.measurement("bypass_attempt") == Some(1.0);
    let permission =
        payload.get("hook_event").and_then(Value::as_str) == Some("permission-request");
    let mut specs = Vec::new();
    let make = |name: &str, flow: &str| {
        let mut spec = base(event, name, flow);
        tool_call(
            &mut spec,
            event,
            if permission {
                "PERMISSION"
            } else {
                "TOOL_CALL"
            },
        );
        spec
    };
    if permission {
        let refused = matches!(decision, Decision::Deny | Decision::Quarantine);
        let mut spec = make(
            if refused {
                "PERMISSION_PRE_DENIED"
            } else {
                "PERMISSION_LEFT_TO_PERSON"
            },
            "F16",
        );
        if refused {
            spec.respond("DENIED", "NA", "Crane refused before the person was asked.")
                .severity(
                    if bypass { "HIGH" } else { "MEDIUM" },
                    "OUTCOME",
                    if bypass { "BYPASS_ATTEMPT" } else { "NONE" },
                );
        } else {
            spec.respond("ASK_PERSON", "NA", "Left to the person to approve.")
                .set(
                    "assessment",
                    "assessment",
                    if decision == Decision::Allow {
                        "COMPLIANT"
                    } else {
                        "PENDING"
                    },
                )
                .severity(
                    if decision == Decision::Allow {
                        "INFO"
                    } else {
                        "LOW"
                    },
                    "OUTCOME",
                    "NONE",
                );
        }
        return vec![spec];
    }
    match decision {
        Decision::Allow => {
            let mut spec = make("TOOL_CALL_ALLOWED", "F05");
            spec.respond("ALLOWED", "NA", "Tool call allowed.")
                .set("assessment", "assessment", "COMPLIANT")
                .set("decision", "decision_rule", "ALLOWED")
                .severity("INFO", "OUTCOME", "NONE")
                .set(
                    "other",
                    "guidance_supplied",
                    payload
                        .get("guidance_bytes")
                        .and_then(Value::as_u64)
                        .unwrap_or(0)
                        > 0,
                );
            specs.push(spec);
        }
        Decision::RequireApproval => {
            let script = SCRIPT_ASK_RULES.contains(&rule.as_str());
            let (name, statement) = if script && operation == "write" {
                (
                    "SCRIPT_WRITE_APPROVAL_REQUIRED",
                    "Risky script being written needs approval.",
                )
            } else if script {
                (
                    "SCRIPT_APPROVAL_REQUIRED",
                    "Script needs human approval before it runs.",
                )
            } else {
                (
                    "TOOL_CALL_APPROVAL_REQUIRED",
                    "A human must approve this tool call.",
                )
            };
            let mut spec = make(name, "F05");
            spec.respond("ASK", "NA", statement)
                .set(
                    "assessment",
                    "assessment",
                    if script { "SUSPICIOUS" } else { "PENDING" },
                )
                .severity("LOW", "OUTCOME", "NONE");
            if script {
                spec.set("other", "scripts", payload.get("scripts"));
            }
            specs.push(spec);
            if rule == "CREDITS_EXHAUSTED" || rule == "CREDITS_SHORT" {
                let name = if rule == "CREDITS_SHORT" {
                    "CREDITS_SHORT"
                } else {
                    "CREDITS_EXHAUSTED"
                };
                let mut credits = make(name, "F27");
                credits
                    .set("action", "action_type", "CREDITS")
                    .set("resource", "resource", ["CREDITS"])
                    .respond(
                        if rule == "CREDITS_SHORT" {
                            "RELEASED"
                        } else {
                            "APPROVAL_REQUIRED"
                        },
                        "INSUFFICIENT",
                        "Autonomy credits ran out; approval required.",
                    )
                    .severity("LOW", "THRESHOLD", "NONE")
                    .set(
                        "other",
                        "ledger_kind",
                        if rule == "CREDITS_SHORT" {
                            "RELEASE"
                        } else {
                            "NA"
                        },
                    );
                specs.push(credits);
            }
        }
        Decision::Quarantine => {
            let mut spec = make("TOOL_CALL_DENIED_QUARANTINE", "F05");
            spec.respond("DENIED", "NA", "Session quarantined; changes are denied.")
                .set("assessment", "assessment_checks", ["SAFETY_STATE"])
                .severity("HIGH", "OUTCOME", "NONE")
                .set("telemetry", "agent_state", "QUARANTINED");
            specs.push(spec);
        }
        Decision::Deny | Decision::Error => {
            if SCRIPT_DENY_RULES.contains(&rule.as_str()) {
                let mut spec = make("SCRIPT_BLOCKED", "F05");
                spec.respond("DENIED", "NA", "Script blocked before it ran.")
                    .set(
                        "assessment",
                        "assessment",
                        if bypass { "SUSPICIOUS" } else { "VIOLATION" },
                    )
                    .set("assessment", "assessment_checks", ["SCRIPT_SCAN"])
                    .severity(
                        if bypass { "HIGH" } else { "MEDIUM" },
                        "OUTCOME",
                        if bypass { "BYPASS_ATTEMPT" } else { "NONE" },
                    )
                    .set("other", "scripts", payload.get("scripts"));
                specs.push(spec);
            } else if rule == "INTEGRITY_FAILED" || rule == "ZONE_CONFIG_UNRESOLVED" {
                let mut spec = make("TOOL_CALL_DENIED_FAIL_CLOSED", "F05");
                spec.respond(
                    "DENIED",
                    if rule == "INTEGRITY_FAILED" {
                        "VERIFICATION_FAILED"
                    } else {
                        "ZONE_CONFIG_UNRESOLVED"
                    },
                    "Rules could not be verified; change denied.",
                )
                .set("assessment", "assessment", "UNRESOLVED")
                .severity("HIGH", "OUTCOME", "NONE")
                .set("telemetry", "agent_state", "ERROR");
                specs.push(spec);
            } else if bypass {
                let mut spec = make("TOOL_CALL_DENIED_BYPASS_ATTEMPT", "F05");
                spec.respond("DENIED", "NA", "Attempt on Crane itself denied.")
                    .set("assessment", "assessment", "SUSPICIOUS")
                    .severity("HIGH", "OUTCOME", "BYPASS_ATTEMPT");
                specs.push(spec);
            } else {
                let simulated = match rule.as_str() {
                    "PRESERVED_SELECTION" => "PRESERVED_BYTES_CHANGED",
                    "MARKER_CHANGED" => "MARKERS_CHANGED",
                    "SELECTION_FILE_DELETE" => "FILE_DELETED",
                    "EDIT_NOT_SIMULATABLE" => "NOT_SIMULATED",
                    _ => "NA",
                };
                let mut spec = make("TOOL_CALL_DENIED_RULE", "F05");
                spec.respond("DENIED", "NA", "Tool call denied by a governance rule.")
                    .set("assessment", "assessment", "VIOLATION")
                    .severity("MEDIUM", "OUTCOME", "NONE")
                    .set("other", "simulated_change", simulated);
                specs.push(spec);
            }
        }
    }
    if decision != Decision::Deny && decision != Decision::Quarantine {
        if let Some(bytes) = payload
            .get("guidance_bytes")
            .and_then(Value::as_u64)
            .filter(|bytes| *bytes > 0)
        {
            let mut spec = make("ZONE_GUIDANCE_SUPPLIED", "F05");
            spec.respond(
                "CONTEXT_SUPPLIED",
                "NA",
                "Zone guidance given with the call.",
            )
            .set("assessment", "assessment_checks", ["ZONE"])
            .set("resource", "resource_classification", ["ZONED"])
            .severity("INFO", "FIXED", "NONE")
            .set("eval", "guidance_bytes", bytes)
            .set(
                "other",
                "criticality",
                payload
                    .get("criticality")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_uppercase),
            )
            .set(
                "other",
                "ceiling",
                payload
                    .get("ceiling")
                    .and_then(Value::as_str)
                    .map(str::to_ascii_uppercase),
            );
            specs.push(spec);
        }
        if matches!(tool(tool_name), "TASK" | "AGENT") {
            let mut spec = make("SUBAGENT_DISPATCHED", "F05");
            spec.respond(
                "ALLOWED",
                "NA",
                "Sub-agent started; its calls are governed too.",
            )
            .set("assessment", "assessment", "COMPLIANT")
            .severity("INFO", "FIXED", "NONE");
            specs.push(spec);
        }
    }
    if let Some(network) = payload
        .get("network")
        .and_then(Value::as_array)
        .filter(|network| !network.is_empty())
    {
        let mut spec = make("NETWORK_DESTINATION_INFERRED", "F36");
        spec.respond(
            "RECORDED",
            "NA",
            "Network destinations inferred from the call.",
        )
        .set(
            "assessment",
            "assessment_checks",
            ["NETWORK_INFERENCE", "METADATA_PROTECTION"],
        )
        .set("resource", "resource", ["NETWORK"])
        .set("resource", "resource_ids", network)
        .set("resource", "resource_classification", ["EXTERNAL"])
        .set("telemetry", "known_as", "INFERRED")
        .set("eval", "destinations", network.len())
        .set(
            "other",
            "unknown_destination",
            network.iter().any(|host| host == "unknown-destination"),
        );
        specs.push(spec);
    }
    if matches!(tool(tool_name), "MCP" | "OTHER") && operation == "other" {
        let status = match decision {
            Decision::Allow => "ALLOWED",
            Decision::RequireApproval => "ASK",
            _ => "DENIED",
        };
        let mut spec = make("EXTERNAL_TOOL_CALLED", "F36");
        spec.respond(status, "NA", "External or unknown tool called.")
            .set("resource", "resource", ["TOOL"])
            .set("resource", "resource_ids", [tool_name])
            .set("resource", "resource_classification", ["EXTERNAL"])
            .set(
                "other",
                "mcp_server",
                tool_name
                    .strip_prefix("mcp__")
                    .and_then(|rest| rest.split("__").next()),
            )
            .set("other", "effect_known", false);
        specs.push(spec);
    }
    specs
}

/** F05.10 */
fn quarantined(event: &Event) -> Spec {
    let mut spec = base(event, "SESSION_QUARANTINED_ATTEMPTS", "F05");
    spec.set("action", "action_type", "TOOL_CALL")
        .respond(
            "QUARANTINED",
            "NA",
            "Session quarantined after repeated attempts on Crane.",
        )
        .set("assessment", "assessment", "SUSPICIOUS")
        .set("assessment", "assessment_checks", ["SAFETY_STATE"])
        .set("decision", "decision", "QUARANTINE")
        .set("decision", "decision_rule", "SAFETY_QUARANTINED")
        .set("decision", "enforcement", "PREVENTED")
        .severity("CRITICAL", "THRESHOLD", "QUARANTINE")
        .set("telemetry", "agent_state", "QUARANTINED")
        .set("other", "safety_reason", event.reasons.first());
    spec
}

/** F27.4 */
fn credits_low(event: &Event) -> Spec {
    let mut spec = base(event, "CREDITS_LOW", "F27");
    spec.set("action", "action_type", "CREDITS")
        .set("resource", "resource", ["CREDITS"])
        .respond("RECORDED", "NA", "Autonomy credits are running low.")
        .severity("MEDIUM", "THRESHOLD", "LOW_CREDITS")
        .set("other", "ledger_kind", "RESERVE");
    if let Some(available) = event.measurement("credits_available") {
        spec.set("eval", "available", available);
    }
    spec
}

/** F06.1, F06.2, F06.4, F06.5, F06.6 */
fn verified(event: &Event) -> Vec<Spec> {
    let payload = &event.payload;
    let (name, status, statement, severity, alert, state) =
        match (event.assessment, event.enforcement) {
            (_, Enforcement::BypassConfirmed) => (
                "BYPASS_CONFIRMED",
                "SUCCESS",
                "Protected code changed around the checks; quarantined.",
                "CRITICAL",
                "BYPASS_CONFIRMED",
                Some("QUARANTINED"),
            ),
            (_, Enforcement::DetectedAfterEffect) => (
                "VIOLATION_AFTER_EFFECT",
                "SUCCESS",
                "Edit broke a rule; session degraded, repair required.",
                "HIGH",
                "VIOLATION",
                Some("DEGRADED"),
            ),
            (Some(Assessment::Unresolved), _) => (
                "TOOL_CALL_VERIFICATION_UNRESOLVED",
                "UNKNOWN",
                "Compliance could not be established after the call.",
                "MEDIUM",
                "NONE",
                None,
            ),
            _ => (
                "TOOL_CALL_VERIFIED",
                "SUCCESS",
                "Tool call verified compliant.",
                "INFO",
                "NONE",
                None,
            ),
        };
    let execution = event
        .execution
        .map_or_else(|| status.to_string(), |execution| upper(execution, status));
    let failure = if name == "BYPASS_CONFIRMED" || name == "VIOLATION_AFTER_EFFECT" {
        "POLICY_VIOLATED"
    } else if execution == "FAILURE" {
        "TOOL_FAILED"
    } else {
        "NA"
    };
    let mut spec = base(event, name, "F06");
    tool_call(&mut spec, event, "TOOL_CALL_RESULT");
    spec.set(
        "assessment",
        "assessment_checks",
        ["PAIRING", "SELECTIONS_RECHECK", "WORKTREE_DIFF"],
    )
    .respond(&execution, failure, statement)
    .severity(severity, "OUTCOME", alert)
    .set("other", "tool_failed", payload.get("tool_failed"))
    .set("other", "scan_mode", payload.get("scan_mode"));
    if let Some(state) = state {
        spec.set("telemetry", "agent_state", state);
    }
    let mut specs = vec![spec];
    if let Some(files) = payload.get("attributed_files").and_then(Value::as_u64) {
        let mut attributed = base(event, "SHELL_EFFECT_ATTRIBUTED", "F06");
        tool_call(&mut attributed, event, "TOOL_CALL_RESULT");
        attributed
            .set("assessment", "assessment_checks", ["WORKTREE_DIFF"])
            .set("resource", "resource", ["FILE_WRITE", "FILE_DELETE", "GIT"])
            .respond("OK", "NA", "Files changed by the command attributed to it.")
            .severity("INFO", "FIXED", "NONE")
            .set("decision", "decision", "NOT_REQUIRED")
            .set("decision", "enforcement", "NOT_APPLICABLE")
            .set("eval", "files_changed", files);
        specs.push(attributed);
    }
    specs
}

/** F06.3 */
fn missing_decision(event: &Event) -> Spec {
    let mut spec = base(event, "TOOL_CALL_MISSING_DECISION", "F06");
    tool_call(&mut spec, event, "TOOL_CALL_RESULT");
    spec.set("assessment", "assessment", "UNRESOLVED")
        .set("assessment", "assessment_checks", ["PAIRING"])
        .respond(
            "UNKNOWN",
            "NO_DECISION",
            "Tool ran without a recorded decision.",
        )
        .severity("HIGH", "FIXED", "COVERAGE_GAP")
        .set("telemetry", "known_as", "UNOBSERVED");
    spec
}

/** F07.1 */
fn unattributed(event: &Event) -> Spec {
    let mut spec = base(event, "UNATTRIBUTED_CHANGES_FOUND", "F07");
    spec.set("action", "action_type", "STOP")
        .set(
            "assessment",
            "assessment_checks",
            ["WORKTREE_DIFF", "ATTRIBUTION"],
        )
        .respond(
            "RECORDED",
            "NA",
            "Files changed outside the recorded tool calls.",
        )
        .severity("HIGH", "FIXED", "UNATTRIBUTED_CHANGE")
        .set("other", "likely_cause", "UNKNOWN");
    spec
}

/** F07.2 to F07.4 */
fn task_result(event: &Event) -> Spec {
    let status = event
        .payload
        .get("task_status")
        .and_then(Value::as_str)
        .unwrap_or("COMPLETED");
    let (name, response, decision, rule, enforcement, statement) =
        match (event.event_type.as_str(), status) {
            ("task.verified", _) => (
                "TASK_VERIFIED",
                "COMPLETED",
                "ALLOW",
                Value::Null,
                "NOT_APPLICABLE",
                "Task verified: every policy passes.",
            ),
            (_, "BLOCKED") => (
                "TASK_VERIFICATION_FAILED",
                "BLOCKED",
                "DENY",
                json!("STOP_BLOCKED"),
                "PREVENTED",
                "Stop refused; policies fail, repair required.",
            ),
            _ => (
                "TASK_ENDED_UNVERIFIED",
                "ENDED",
                "ALLOW",
                json!("STOP_LOOP_GUARD"),
                "DETECTED_AFTER_EFFECT",
                "Task ended unverified after a refused stop.",
            ),
        };
    let failure = if name == "TASK_VERIFIED" {
        "NA"
    } else if event.assessment == Some(Assessment::Unresolved) {
        "SELECTION_UNRESOLVED"
    } else {
        "PRESERVE_CHANGED"
    };
    let mut spec = base(event, name, "F07");
    spec.set("action", "action_type", "STOP")
        .set("resource", "resource", ["SELECTION"])
        .set(
            "assessment",
            "assessment_checks",
            [
                "PRESERVES_UNCHANGED",
                "TARGETS_CHANGED",
                "SELECTIONS_RESOLVED",
            ],
        )
        .respond(response, failure, statement)
        .set("decision", "decision", decision)
        .set("decision", "decision_rule", rule)
        .set("decision", "enforcement", enforcement)
        .severity(
            if name == "TASK_VERIFIED" {
                "INFO"
            } else {
                "HIGH"
            },
            "OUTCOME",
            if name == "TASK_VERIFIED" {
                "NONE"
            } else {
                "VERIFICATION_FAILED"
            },
        )
        .set("other", "task_status", status);
    spec
}

/** F08.1 */
fn unsettled(event: &Event) -> Spec {
    let mut spec = base(event, "TOOL_CALL_UNSETTLED", "F08");
    spec.set("action", "action_type", "SESSION_END")
        .set("resource", "resource", ["CREDITS"])
        .respond(
            "RELEASED",
            "NO_POST_TOOL_REPORT",
            "Action never reported back; credits released.",
        )
        .set("decision", "enforcement", "COVERAGE_GAP")
        .severity("MEDIUM", "FIXED", "NONE")
        .set("telemetry", "known_as", "UNOBSERVED")
        .set("telemetry", "agent_state", "ENDED");
    spec
}

/** F08.2 and F08.3 */
fn attested(event: &Event) -> Vec<Spec> {
    let payload = &event.payload;
    let attestation = payload.get("attestation").cloned().unwrap_or(Value::Null);
    let signed = attestation["signed"] == true;
    let mut closed = base(event, "SESSION_CLOSED", "F08");
    closed
        .set("action", "action_type", "SESSION_END")
        .respond("ENDED", "NA", "Agent session closed.")
        .severity("INFO", "FIXED", "NONE")
        .set("telemetry", "agent_state", "ENDED")
        .set(
            "other",
            "end_reason",
            payload
                .get("end_reason")
                .and_then(Value::as_str)
                .map_or_else(|| "OTHER".to_string(), str::to_ascii_uppercase),
        )
        .set("other", "final_safety", payload.get("final_safety"));
    if let Some(duration) = payload.get("session_duration_ms").and_then(Value::as_u64) {
        closed.set("eval", "session_duration_ms", duration);
    }
    let mut spec = base(event, "SESSION_ATTESTED", "F08");
    spec.set("action", "action_type", "SESSION_END")
        .set("resource", "resource", ["UPLOAD_KEY"])
        .respond(
            if signed { "SIGNED" } else { "UNSIGNED" },
            if signed { "NA" } else { "KEY_UNAVAILABLE" },
            if signed {
                "Session evidence signed."
            } else {
                "Session evidence could not be signed."
            },
        )
        .severity(if signed { "INFO" } else { "MEDIUM" }, "OUTCOME", "NONE")
        .set("telemetry", "agent_state", "ENDED")
        .set(
            "eval",
            "events_attested",
            attestation["events"].as_u64().unwrap_or(0),
        )
        .set("other", "attestation_digest", &attestation["digest"])
        .set(
            "other",
            "signer",
            attestation
                .get("signer")
                .cloned()
                .unwrap_or(json!("INSTALLATION")),
        )
        .set("other", "signer_key", &attestation["public_key"])
        .set("other", "signature", &attestation["signature"]);
    vec![closed, spec]
}

/** F11.1 */
fn hook_failed(event: &Event) -> Spec {
    let reason = event
        .reasons
        .first()
        .cloned()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let failure = if reason.contains("not inside a git repository") {
        "NOT_A_REPOSITORY"
    } else if reason.contains("not initialized") {
        "NOT_INITIALIZED"
    } else if reason.contains("payload") {
        "PAYLOAD_UNREADABLE"
    } else if reason.contains("lock") {
        "LOCK_TIMEOUT"
    } else if reason.contains("trust") {
        "TRUST_UNAVAILABLE"
    } else if reason.contains("malformed") {
        "STORE_UNREADABLE"
    } else {
        "INTERNAL"
    };
    let mut spec = base(event, "HOOK_FAILED", "F11");
    spec.set("action", "action_type", "HOOK")
        .set("assessment", "assessment", "UNRESOLVED")
        .respond("ERROR", failure, "Crane failed; the action was denied.")
        .set("decision", "decision", "ERROR")
        .set("decision", "decision_rule", "HOOK_ERROR")
        .set("decision", "enforcement", "PREVENTED")
        .severity("HIGH", "FIXED", "NONE")
        .set("telemetry", "agent_state", "ERROR");
    spec
}

/** Add the envelope to a specification event served from a stored event
 * Input
    - event: &Event - the stored event
    - spec: &Value - one of its specification events
    - index: usize - its position among them
 * Output
    - Value
*/
pub(crate) fn envelope(event: &Event, spec: &Value, index: usize) -> Value {
    let mut value = spec.clone();
    if let Some(object) = value.as_object_mut() {
        let id = if index == 0 {
            event.event_id.clone()
        } else {
            format!("{}-{index}", event.event_id)
        };
        object.insert("event_id".into(), json!(id));
        object.insert("occurred_at".into(), json!(event.occurred_at));
        object.insert("schema_version".into(), json!(SPEC_VERSION));
        object.insert("sequence".into(), json!(event.sequence));
        object.insert("previous_digest".into(), json!(event.previous_digest));
        object.insert("digest".into(), json!(event.digest));
        object.insert("origin".into(), json!("MACHINE"));
    }
    value
}

/** Add the envelope to a derived or service event (no chain position)
 * Input
    - spec: Spec - event
    - id: &str - stable identifier
    - at: u64 - Unix milliseconds
    - origin: &str - SERVICE or DERIVED
 * Output
    - Value
*/
pub(crate) fn envelope_detached(spec: Spec, id: &str, at: u64, origin: &str) -> Value {
    let mut value = spec.into_value();
    if let Some(object) = value.as_object_mut() {
        object.insert("event_id".into(), json!(id));
        object.insert("occurred_at".into(), json!(at));
        object.insert("schema_version".into(), json!(SPEC_VERSION));
        object.insert("sequence".into(), Value::Null);
        object.insert("previous_digest".into(), Value::Null);
        object.insert("digest".into(), Value::Null);
        object.insert("origin".into(), json!(origin));
    }
    value
}

/** F27: specification events of a credit ledger entry (derived by the dashboard)
 * Input
    - entry: &Entry - ledger entry
    - repository: &str - repository identity
 * Output
    - Option<Value> the event with its envelope (None for kinds that are not shown on their own)
*/
pub(crate) fn from_ledger(entry: &Entry, repository: &str) -> Option<Value> {
    let (name, statement, severity, ledger_kind) = match entry.kind {
        Kind::Grant => (
            "CREDITS_GRANTED",
            "Autonomy credits granted to the session.",
            "INFO",
            "GRANT",
        ),
        Kind::Reserve => (
            "CREDITS_RESERVED",
            "Credits reserved for an allowed action.",
            "INFO",
            "RESERVE",
        ),
        Kind::Consume => (
            "CREDITS_CONSUMED",
            "Reserved credits charged; the action ran.",
            "INFO",
            "CONSUME",
        ),
        Kind::Release => (
            "CREDITS_RELEASED",
            "Reserved credits released; the action did not run.",
            "INFO",
            "RELEASE",
        ),
        Kind::Revoke => (
            "CREDITS_REVOKED",
            "Credits revoked as a violation penalty.",
            "HIGH",
            "REVOKE",
        ),
        Kind::Expire => (
            "CREDITS_EXPIRED",
            "Unused credits expired.",
            "INFO",
            "EXPIRE",
        ),
        Kind::Refund => (
            "CREDITS_REFUNDED",
            "Consumed credits refunded.",
            "INFO",
            "REFUND",
        ),
        Kind::Regenerate => (
            "CREDITS_REGENERATED",
            "Credits regenerated.",
            "INFO",
            "REGENERATE",
        ),
        Kind::Override => (
            "CREDITS_OVERRIDDEN",
            "A person changed the credit balance.",
            "MEDIUM",
            "OVERRIDE",
        ),
    };
    let mut spec = Spec::new(name, "F27");
    spec.set("action", "action_type", "CREDITS")
        .set("action", "action_source", "HOOK")
        .set("action", "action_id", &entry.tool_call_id)
        .set("repository", "repo_id", repository)
        .set(
            "repository",
            "registry_generation",
            entry.registry_generation,
        )
        .set("session", "session_type", "AGENT_SESSION")
        .set("session", "session_id", &entry.session)
        .set("session", "task_id", &entry.task)
        .set("resource", "resource", ["CREDITS"])
        .respond("OK", "NA", statement)
        .severity(severity, "FIXED", "NONE")
        .set(
            "telemetry",
            "agent_state",
            if entry.kind == Kind::Expire {
                "ENDED"
            } else {
                "ACTIVE"
            },
        )
        .set("eval", "amount", entry.amount)
        .set("eval", "granted", entry.after.granted)
        .set("eval", "reserved", entry.after.reserved)
        .set("eval", "consumed", entry.after.consumed)
        .set("eval", "revoked", entry.after.revoked)
        .set("eval", "expired", entry.after.expired)
        .set("eval", "available", entry.after.available)
        .set("other", "ledger_kind", ledger_kind)
        .set("other", "ledger_sequence", entry.seq)
        .set("other", "idempotency_key", &entry.idempotency_key)
        .set("other", "reason", &entry.reason);
    if entry.kind == Kind::Revoke {
        spec.set("assessment", "assessment", "VIOLATION").set(
            "decision",
            "enforcement",
            "DETECTED_AFTER_EFFECT",
        );
    }
    Some(envelope_detached(
        spec,
        &entry.entry_id,
        entry.at,
        "DERIVED",
    ))
}

/** F13.3: the specification event of a governance audit entry (derived by the dashboard)
 * Input
    - entry: &Value - audit entry as uploaded (seq, at seconds, actor, action, detail, digest)
    - repository: &str - repository identity
 * Output
    - Value
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn from_audit(entry: &Value, repository: &str) -> Value {
    let action = entry["action"].as_str().unwrap_or_default();
    let mut spec = Spec::new("AUDIT_ENTRY_APPENDED", "F13");
    spec.set("action", "action_type", "COMMAND")
        .set("action", "action_source", "CLI")
        .set("action", "action_user", &entry["actor"])
        .set("repository", "repo_id", repository)
        .set("resource", "resource", ["CRANE_METADATA"])
        .set("resource", "resource_ids", [".crane/audit.jsonl"])
        .set("resource", "resource_classification", ["METADATA"])
        .respond(
            "OK",
            "NA",
            "Rules change recorded in the signed audit trail.",
        )
        .severity("INFO", "FIXED", "NONE")
        .set("eval", "audit_sequence", &entry["seq"])
        .set(
            "other",
            "audit_action",
            action.replace('.', "_").to_ascii_uppercase(),
        )
        .set("other", "detail", &entry["detail"])
        .set("other", "audit_head", &entry["digest"]);
    let at = entry["at"].as_u64().unwrap_or(0) * 1000;
    let id = format!("aud_{}", entry["seq"].as_u64().unwrap_or(0));
    envelope_detached(spec, &id, at, "DERIVED")
}

/** F14: specification events of an alert record (raised, suppressed, and one per delivery)
 * Input
    - alert: &Value - alert as stored (alert_id, kind, title, severity, at seconds, session, state, suppressed, deliveries)
    - repository: &str - repository identity
 * Output
    - Vec<Value>
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn from_alert(alert: &Value, repository: &str) -> Vec<Value> {
    let id = alert["alert_id"].as_str().unwrap_or_default().to_string();
    let at = alert["at"].as_u64().unwrap_or(0) * 1000;
    let kind = alert["kind"]
        .as_str()
        .unwrap_or("NONE")
        .to_ascii_uppercase();
    let severity = alert["severity"]
        .as_str()
        .unwrap_or("MEDIUM")
        .to_ascii_uppercase();
    let source = match alert["source"].as_str() {
        Some("service") => "SERVICE",
        _ if alert["computed"] == true => "DASHBOARD",
        _ => "HOOK",
    };
    let common = |name: &str| {
        let mut spec = Spec::new(name, "F14");
        spec.set("action", "action_type", "ALERT")
            .set("action", "action_source", source)
            .set("action", "action_id", &id)
            .set("repository", "repo_id", repository);
        if let Some(session) = alert["session"].as_str() {
            spec.set("session", "session_type", "AGENT_SESSION").set(
                "session",
                "session_id",
                session,
            );
        }
        spec.set("other", "triggering_event_id", &alert["event_id"])
            .set("other", "alert_key", &alert["key"])
            .set("other", "alert_title", &alert["title"]);
        // A threshold rule built with 'crane alert add' (DASHBOARD.md F14, Part 4)
        if let Some(rule) = alert["rule"].as_object() {
            for key in [
                "name",
                "metric",
                "op",
                "threshold",
                "window_minutes",
                "channels",
                "filter",
            ] {
                if let Some(value) = rule.get(key) {
                    spec.set("other", &format!("rule_{key}"), value);
                }
            }
            spec.set("eval", "rule_value", &alert["value"]).set(
                "eval",
                "rule_threshold",
                &rule["threshold"],
            );
        }
        spec
    };
    let mut out = Vec::new();
    if alert["suppressed"] == true {
        let mut spec = common("ALERT_SUPPRESSED");
        spec.respond(
            "SUPPRESSED",
            "NA",
            "Repeat alert suppressed during its cooldown.",
        )
        .severity("LOW", "FIXED", &kind);
        out.push(envelope_detached(
            spec,
            &format!("{id}-suppressed"),
            at,
            "DERIVED",
        ));
        return out;
    }
    let rule_state = (alert["kind"] == "rule").then(|| alert["state"].as_str().unwrap_or("FIRING"));
    let mut raised = common(match rule_state {
        Some("RESOLVED") => "ALERT_RULE_RESOLVED",
        Some("TEST") => "ALERT_RULE_TESTED",
        Some(_) => "ALERT_RULE_FIRED",
        None => "ALERT_RAISED",
    });
    raised
        .respond(
            alert["state"].as_str().unwrap_or("OPEN"),
            "NA",
            alert["title"].as_str().unwrap_or("Alert raised."),
        )
        .severity(&severity, "OUTCOME", &kind)
        .set("other", "detail", &alert["detail"]);
    out.push(envelope_detached(raised, &id, at, "DERIVED"));
    for (index, delivery) in alert["deliveries"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        let channel = delivery["channel"]
            .as_str()
            .unwrap_or("UNKNOWN")
            .to_ascii_uppercase();
        let result = delivery["result"]
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let delivered = result.starts_with("sent")
            || result.starts_with("ok")
            || result.starts_with("delivered");
        // Sent in the background without waiting for the answer: not a failure, not confirmed
        let queued = result.starts_with("queued");
        let dry = result.contains("dry") || queued;
        let mut spec = common(if delivered || dry {
            "ALERT_DELIVERED"
        } else {
            "ALERT_DELIVERY_FAILED"
        });
        spec.respond(
            if queued {
                "QUEUED"
            } else if dry {
                "DRY_RUN"
            } else if delivered {
                "DELIVERED"
            } else {
                "FAILED"
            },
            if delivered || dry { "NA" } else { "HTTP_ERROR" },
            if delivered {
                "Alert delivered."
            } else if queued {
                "Alert sent; delivery not confirmed."
            } else if dry {
                "Alert delivery skipped (dry run)."
            } else {
                "Alert delivery failed."
            },
        )
        .severity(
            if delivered || dry { "INFO" } else { "MEDIUM" },
            "OUTCOME",
            &kind,
        )
        .set("bindings", "third_party", [channel.as_str()])
        .set(
            "bindings",
            "binding_result",
            if delivered || dry { "OK" } else { "FAILED" },
        )
        .set("other", "channel", &channel)
        .set("other", "result", &delivery["result"]);
        if let Some(status) = delivery["status"].as_u64() {
            spec.set("eval", "http_status", status);
        }
        if let Some(ms) = delivery["ms"].as_u64() {
            spec.set("eval", "delivery_ms", ms);
        }
        out.push(envelope_detached(
            spec,
            &format!("{id}-d{index}"),
            at,
            "DERIVED",
        ));
    }
    out
}

/** F10.1 or F10.2: a stale or revived session (derived by the dashboard)
 * Input
    - session: &Value - session record (JSON)
    - repository: &str - repository identity
    - revived: bool - the session reported again after a silence
    - at: u64 - Unix milliseconds of the change
    - silent_minutes: u64 - how long it was silent
 * Output
    - Value
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn staleness(
    session: &Value,
    repository: &str,
    revived: bool,
    at: u64,
    silent_minutes: u64,
) -> Value {
    let id = session["foxx_session_id"].as_str().unwrap_or_default();
    let mut spec = Spec::new(
        if revived {
            "SESSION_REVIVED"
        } else {
            "SESSION_STALE"
        },
        "F10",
    );
    spec.set("action", "action_type", "STALENESS_CHECK")
        .set("action", "action_source", "DASHBOARD")
        .set("repository", "repo_id", repository)
        .set("session", "session_type", "AGENT_SESSION")
        .set("session", "session_id", id)
        .set(
            "session",
            "provider_session_id",
            &session["provider_session_id"],
        )
        .set(
            "agent",
            "agent_type",
            agent_type(session["provider"].as_str()),
        )
        .set("agent", "agent_id", &session["agent_instance_id"])
        .set("agent", "model", &session["model"])
        .set(
            "assessment",
            "assessment",
            if revived {
                "NOT_REQUIRED"
            } else {
                "UNRESOLVED"
            },
        )
        .set(
            "assessment",
            "assessment_checks",
            ["LAST_SEEN", "ATTESTATION_PRESENT", "ENDED"],
        )
        .set(
            "decision",
            "enforcement",
            if revived {
                "NOT_APPLICABLE"
            } else {
                "COVERAGE_GAP"
            },
        )
        .respond(
            if revived { "ACTIVE" } else { "STALE" },
            if revived { "NA" } else { "NO_ACTIVITY" },
            if revived {
                "Stale session reported again."
            } else {
                "Session stopped reporting without a signed end."
            },
        )
        .severity(
            if revived { "LOW" } else { "HIGH" },
            if revived { "FIXED" } else { "THRESHOLD" },
            if revived { "NONE" } else { "STALE_SESSION" },
        )
        .set(
            "telemetry",
            "agent_state",
            if revived { "ACTIVE" } else { "STALE" },
        )
        .set("telemetry", "known_as", "INFERRED")
        .set(
            "eval",
            if revived {
                "minutes_silent"
            } else {
                "minutes_since_last_seen"
            },
            silent_minutes,
        )
        .set("other", "last_seen_at", &session["last_seen_at"]);
    let name = if revived { "revived" } else { "stale" };
    envelope_detached(spec, &format!("{name}_{id}_{at}"), at, "DERIVED")
}

/** F16.3: the inferred outcome of a permission dialog (derived by the dashboard)
 * Input
    - decision: &Value - the PERMISSION_LEFT_TO_PERSON event (with envelope)
    - ran: Option<bool> - Some(true) when the action reported back, Some(false) when the session
      ended without it, None when unknown yet
    - at: u64 - Unix milliseconds of the evidence
 * Output
    - Value
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn permission_outcome(decision: &Value, ran: Option<bool>, at: u64) -> Value {
    let mut spec = Spec::from_value(decision).unwrap_or_else(|| Spec::new("", "F16"));
    spec.0
        .insert("event_name".into(), json!("PERMISSION_OUTCOME_INFERRED"));
    for key in [
        "event_id",
        "occurred_at",
        "schema_version",
        "sequence",
        "previous_digest",
        "digest",
        "origin",
    ] {
        spec.0.remove(key);
    }
    let status = match ran {
        Some(true) => "APPROVED_BY_PERSON",
        Some(false) => "DECLINED_BY_PERSON",
        None => "UNKNOWN",
    };
    spec.respond(
        status,
        "NA",
        "Person's choice inferred from what happened next.",
    )
    .set("assessment", "assessment", "NOT_REQUIRED")
    .set("assessment", "assessment_checks", ["PAIRING"])
    .set("decision", "decision", "NOT_REQUIRED")
    .set("decision", "decision_rule", Value::Null)
    .set("decision", "decision_reasons", Vec::<String>::new())
    .severity("INFO", "FIXED", "NONE")
    .set("telemetry", "known_as", "INFERRED")
    .set(
        "other",
        "inferred_from",
        match ran {
            Some(true) => "POST_TOOL_REPORT",
            _ => "SESSION_END_WITHOUT_REPORT",
        },
    );
    let id = format!(
        "{}-outcome",
        decision["event_id"].as_str().unwrap_or_default()
    );
    envelope_detached(spec, &id, at, "DERIVED")
}

/** F27.10: the ledger of a member does not reconcile (derived by the dashboard)
 * Input
    - repository: &str - repository identity
    - member: &str - member key
    - entries: usize - ledger entries checked
    - at: u64 - Unix milliseconds
 * Output
    - Value
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn ledger_failed(repository: &str, member: &str, entries: usize, at: u64) -> Value {
    let mut spec = Spec::new("LEDGER_RECONCILIATION_FAILED", "F27");
    spec.set("action", "action_type", "CREDITS")
        .set("action", "action_source", "DASHBOARD")
        .set("repository", "repo_id", repository)
        .set("resource", "resource", ["CREDITS"])
        .respond(
            "FAILED",
            "CHAIN_BROKEN",
            "Credit ledger does not reconcile.",
        )
        .set("assessment", "assessment", "VIOLATION")
        .set(
            "assessment",
            "assessment_checks",
            ["LEDGER_CHAIN", "BALANCES"],
        )
        .set("decision", "enforcement", "DETECTED_AFTER_EFFECT")
        .severity("HIGH", "FIXED", "NONE")
        .set("telemetry", "known_as", "INFERRED")
        .set("eval", "entries", entries)
        .set("other", "member", member);
    envelope_detached(spec, &format!("ledger_{member}"), at, "DERIVED")
}

/** Map a stored telemetry status to the specification's known_as (for derived events)
 * Input
    - telemetry: Telemetry - status
 * Output
    - String
*/
#[allow(dead_code)]
pub(crate) fn known_as(telemetry: Telemetry) -> String {
    upper(telemetry, "OBSERVED")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::events::{Measurement, Resource};
    use crate::telemetry::model::Domain;

    /** Check that every event carries the 13 groups with their compulsory sub-attributes
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn every_event_has_the_thirteen_groups() {
        let spec = Spec::new("TOOL_CALL_ALLOWED", "F05").into_value();
        for group in GROUPS {
            assert!(spec[group].is_object(), "{group}");
        }
        for (group, key) in [
            ("action", "action_type"),
            ("action", "action_source"),
            ("action", "action_user"),
            ("action", "action_email"),
            ("action", "action_id"),
            ("repository", "repo"),
            ("repository", "repo_id"),
            ("session", "session_type"),
            ("agent", "agent_type"),
            ("resource", "resource"),
            ("response", "response_status"),
            ("response", "failure_state"),
            ("response", "response_statement"),
            ("assessment", "assessment"),
            ("decision", "decision"),
            ("decision", "enforcement"),
            ("eval", "eval"),
            ("severity", "severity_status"),
            ("severity", "alert_kind"),
            ("bindings", "third_party"),
            ("telemetry", "agent_state"),
            ("telemetry", "known_as"),
        ] {
            assert!(spec[group].get(key).is_some(), "{group}.{key}");
        }
    }

    /** Check that a denied write to a preserved block, an allowed call with guidance and network
     * use, and a confirmed bypass are projected to their specification events
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn decisions_are_projected() {
        let mut deny = Event::new("tool_call.decided", Domain::Authorization, "repo", "k1");
        deny.source = Source::ProviderAdapter;
        deny.decision = Some(Decision::Deny);
        deny.enforcement = Enforcement::Prevented;
        deny.scope.provider = Some("claude".into());
        deny.scope.foxx_session_id = Some("ses_1".into());
        deny.payload = json!({"hook_event": "pre-tool-use", "tool": "Edit", "operation": "write", "rules": ["PRESERVED_SELECTION"], "user": "Dev", "email": "dev@example.com"});
        deny.resources.push(Resource {
            kind: "file".into(),
            id: "app/payments.py".into(),
            access: "write".into(),
            classification: Some("selection".into()),
        });
        let specs = project(&deny);
        assert_eq!(specs[0]["event_name"], "TOOL_CALL_DENIED_RULE");
        assert_eq!(specs[0]["decision"]["decision_rule"], "PRESERVED_SELECTION");
        assert_eq!(
            specs[0]["other"]["simulated_change"],
            "PRESERVED_BYTES_CHANGED"
        );
        assert_eq!(specs[0]["action"]["action_tool"], "EDIT");
        assert_eq!(specs[0]["action"]["action_email"], "dev@example.com");
        assert_eq!(specs[0]["agent"]["agent_type"], "CLAUDE_CODE");
        assert_eq!(specs[0]["resource"]["resource"], json!(["FILE_WRITE"]));
        let mut allow = deny.clone();
        allow.decision = Some(Decision::Allow);
        allow.enforcement = Enforcement::NotApplicable;
        allow.payload = json!({"hook_event": "pre-tool-use", "tool": "Bash", "operation": "execute", "rules": [], "network": ["example.com"], "guidance_bytes": 120});
        let names = project(&allow)
            .iter()
            .map(|spec| spec["event_name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "TOOL_CALL_ALLOWED",
                "ZONE_GUIDANCE_SUPPLIED",
                "NETWORK_DESTINATION_INFERRED"
            ]
        );
        let mut bypass = Event::new("tool_call.verified", Domain::PolicyIntegrity, "repo", "k2");
        bypass.enforcement = Enforcement::BypassConfirmed;
        bypass.assessment = Some(Assessment::Violation);
        bypass.payload = json!({"tool": "Bash", "operation": "execute"});
        bypass
            .measurements
            .push(Measurement::observed("tool_duration_ms", 12.0, "ms", "t"));
        let specs = project(&bypass);
        assert_eq!(specs[0]["event_name"], "BYPASS_CONFIRMED");
        assert_eq!(specs[0]["severity"]["severity_status"], "CRITICAL");
        assert_eq!(specs[0]["eval"]["tool_duration_ms"], 12.0);
        assert_eq!(specs[0]["eval"]["eval"], "MEASURED");
    }

    /** Check that response statements never exceed ten words
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn statements_are_short() {
        let mut spec = Spec::new("X", "F00");
        spec.respond(
            "OK",
            "NA",
            "one two three four five six seven eight nine ten eleven twelve",
        );
        assert_eq!(
            spec.get("response", "response_statement")
                .as_str()
                .unwrap()
                .split(' ')
                .count(),
            10
        );
    }
}
