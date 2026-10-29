// Specification events (DASHBOARD.md) of Crane commands (command.executed), of the Landscape editor
// extension (foxx.event), and of the notes attached to any stored event. Commands map to the flows
// they belong to: setup and rule changes (F01), joining, registering, linking, and admins (F02),
// hooks (F15), tests (F19), context (F20), the GitHub connection (F21), integrations (F22), merge
// conflicts (F23), headless runs (F24), and read-only inspection (F35); every command is also a
// COMMAND_EXECUTED or COMMAND_REFUSED_FOR_AGENT event (F13).

use serde_json::Value;

use super::events::Event;
use super::spec::{base, Spec};

/** The words of a recorded command line
 * Input
    - event: &Event - command.executed event
 * Output
    - Vec<String>
*/
fn words(event: &Event) -> Vec<String> {
    event
        .payload
        .get("command")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .map(String::from)
        .collect()
}

/** The value after a keyword argument (for example "policy NAME")
 * Input
    - words: &[String] - command words
    - key: &str - keyword
 * Output
    - Option<String>
*/
fn argument(words: &[String], key: &str) -> Option<String> {
    words
        .iter()
        .position(|word| word == key)
        .and_then(|index| words.get(index + 1))
        .cloned()
}

/** Classify a command's error into a failure state
 * Input
    - error: &str - error message
 * Output
    - &'static str
*/
fn failure(error: &str) -> &'static str {
    let error = error.to_ascii_lowercase();
    if error.contains("cannot be executed by an ai agent") || error.contains("a human must run") {
        "AGENT_REFUSED"
    } else if error.contains("merge conflict") {
        "CONFLICT"
    } else if error.contains("only the repository owner") {
        "NOT_OWNER"
    } else if error.contains("not an admin") {
        "NOT_ADMIN"
    } else if error.contains("fails verification") || error.contains("refusing to sign") {
        "VERIFICATION_FAILED"
    } else if error.contains("signing key")
        || error.contains("passphrase")
        || error.contains("unprotect")
    {
        "KEY_UNAVAILABLE"
    } else if error.contains("checkpoint") {
        "CHECKPOINT"
    } else if error.contains("policy") {
        "POLICY"
    } else if error.contains("yaml")
        || error.contains("syntax")
        || error.contains("malformed")
        || error.contains("usage:")
    {
        "SYNTAX"
    } else if error.contains("git") || error.contains("remote") || error.contains("commit") {
        "GIT"
    } else if error.contains("trust") || error.contains("signature") || error.contains("key") {
        "SECURITY"
    } else {
        "OTHER"
    }
}

/** Specification events of a Crane command
 * Input
    - event: &Event - command.executed event
 * Output
    - Vec<Spec>
*/
pub(super) fn command(event: &Event) -> Vec<Spec> {
    let words = words(event);
    let word = |index: usize| words.get(index).map(String::as_str).unwrap_or_default();
    let ok = event.execution != Some(super::model::Execution::Failure);
    let error = event.reasons.first().cloned().unwrap_or_default();
    let failed = if ok { "NA" } else { failure(&error) };
    let agent = event
        .payload
        .get("agent_environment")
        .is_some_and(|value| !value.is_null());
    let status = if ok { "OK" } else { "FAILED" };
    let streak = if ok { "LOW" } else { "MEDIUM" };
    let mut specs = Vec::new();
    // F13.1 or F13.2: every command
    let command_name = match (word(0), word(1)) {
        ("--set", _) => "SET_DEFAULT".to_string(),
        ("create", "policy") => "CREATE_POLICY".to_string(),
        ("add", "policy-context") => "ADD_CONTEXT".to_string(),
        ("add", "policy") => "MOVE_SELECTION".to_string(),
        ("policy-context", _) => "VIEW_CONTEXT".to_string(),
        ("policy", _) => "POLICY_STATUS".to_string(),
        (first, _) => first.trim_start_matches('-').to_ascii_uppercase(),
    };
    let refused = agent && failed == "AGENT_REFUSED";
    let mut executed = base(
        event,
        if refused {
            "COMMAND_REFUSED_FOR_AGENT"
        } else {
            "COMMAND_EXECUTED"
        },
        "F13",
    );
    executed
        .set("action", "action_type", "COMMAND")
        .respond(
            if refused { "REFUSED" } else { status },
            failed,
            if refused {
                "Governance command refused: an agent ran it."
            } else if ok {
                "Command finished."
            } else {
                "Command failed."
            },
        )
        .severity(
            if refused {
                "HIGH"
            } else if ok {
                "INFO"
            } else {
                "MEDIUM"
            },
            if refused { "FIXED" } else { "FAILURE_STREAK" },
            "NONE",
        )
        .set("other", "command", &command_name)
        .set("other", "arguments", &words);
    if refused {
        executed
            .set("assessment", "assessment", "SUSPICIOUS")
            .set("assessment", "assessment_checks", ["AGENT_ENVIRONMENT"])
            .set("decision", "decision", "DENY")
            .set("decision", "decision_rule", "HUMAN_ONLY")
            .set("decision", "enforcement", "PREVENTED")
            .set(
                "other",
                "agent_marker",
                event.payload.get("agent_environment"),
            );
    }
    specs.push(executed);
    let flow_event = |name: &str, flow: &str, action_type: &str, statement: &str| {
        let mut spec = base(event, name, flow);
        spec.set("action", "action_type", action_type)
            .respond(
                status,
                failed,
                if ok { statement } else { "Command failed." },
            )
            .severity(streak, "FAILURE_STREAK", "NONE");
        spec
    };
    let governance_change = matches!(
        (word(0), word(1)),
        ("init", _)
            | ("checkpoint", _)
            | ("protect", _)
            | ("target", _)
            | ("create", "policy")
            | ("add", "policy")
            | ("--set", _)
            | ("governance", "sign")
    );
    match (word(0), word(1)) {
        ("repo", "--https" | "--ssh" | "--gh-cli") => {
            let mut spec = flow_event(
                "REPOSITORY_CONNECTED",
                "F01",
                "CONNECT",
                "Clone connected to its GitHub repository.",
            );
            spec.set("resource", "resource", ["GIT"])
                .set(
                    "assessment",
                    "assessment_checks",
                    ["REMOTE_REACHABLE", "HOST_ALLOWED"],
                )
                .set("bindings", "third_party", ["GITHUB", "GIT_REMOTE"])
                .set(
                    "bindings",
                    "binding_result",
                    if ok { "OK" } else { "FAILED" },
                )
                .set(
                    "other",
                    "method",
                    word(1)
                        .trim_start_matches('-')
                        .replace('-', "_")
                        .to_ascii_uppercase(),
                )
                .set(
                    "other",
                    "privileges",
                    words
                        .iter()
                        .skip(3)
                        .map(|word| word.replace('-', "_").to_ascii_uppercase())
                        .collect::<Vec<_>>(),
                );
            specs.push(spec);
        }
        ("repo", "status") => {
            let mut spec = flow_event(
                "CONNECTION_CHECKED",
                "F21",
                "CONNECTION",
                "Repository connection checked.",
            );
            spec.respond(
                if ok { "CONNECTED" } else { "DISCONNECTED" },
                if ok { "NA" } else { "UNREACHABLE" },
                "Repository connection checked.",
            )
            .set("bindings", "third_party", ["GITHUB", "GIT_REMOTE"])
            .set(
                "bindings",
                "binding_result",
                if ok { "OK" } else { "FAILED" },
            );
            specs.push(spec);
        }
        ("repo", "--view") => specs.push(flow_event(
            "CONNECTION_VIEWED",
            "F21",
            "VIEW_CONNECTION",
            "Repository connection viewed.",
        )),
        ("repo", "del") => {
            let mut spec = flow_event(
                "CONNECTION_DELETED",
                "F21",
                "DELETE_CONNECTION",
                "Repository connection deleted.",
            );
            spec.severity(if ok { "MEDIUM" } else { "HIGH" }, "FIXED", "NONE");
            specs.push(spec);
        }
        ("github", _) => {
            let mut spec = flow_event(
                "GITHUB_GRANT_CREATED",
                "F21",
                "GITHUB_GRANT",
                "GitHub CLI login granted to Crane.",
            );
            spec.set("bindings", "third_party", ["GITHUB_CLI", "GITHUB"])
                .set("other", "grant_days", argument(&words, "days"));
            specs.push(spec);
        }
        ("init", _) => {
            let joined = event
                .payload
                .get("notes")
                .and_then(Value::as_array)
                .is_some_and(|notes| {
                    notes
                        .iter()
                        .any(|note| note["kind"] == "OWNER_KEY_LOOKUP" || note["kind"] == "JOIN")
                });
            if joined {
                let mut spec = flow_event(
                    "JOIN_STARTED",
                    "F02",
                    "JOIN",
                    "Teammate joined the repository's signed rules.",
                );
                spec.set("resource", "resource", ["CRANE_METADATA"]);
                specs.push(spec);
            } else {
                let mut spec = flow_event(
                    "CRANE_INITIALIZED",
                    "F01",
                    "INIT",
                    "Crane initialized; first rules version signed.",
                );
                spec.set("resource", "resource", ["CRANE_METADATA", "SIGNING_KEY"])
                    .set("resource", "resource_classification", ["METADATA", "TRUST"]);
                specs.push(spec);
            }
        }
        ("checkpoint", _) => {
            let mut spec = flow_event(
                "CHECKPOINT_CREATED",
                "F01",
                "CHECKPOINT",
                "Trusted checkpoint recorded.",
            );
            spec.set("resource", "resource", ["GIT", "CRANE_METADATA"])
                .set(
                    "other",
                    "checkpoint_name",
                    words.get(1).cloned().unwrap_or_else(|| "baseline".into()),
                );
            specs.push(spec);
        }
        ("create", "policy") => {
            let mut spec = flow_event("POLICY_CREATED", "F01", "CREATE_POLICY", "Policy created.");
            spec.set("resource", "resource", ["CRANE_METADATA"])
                .set("other", "policy_name", words.get(2))
                .set("other", "policy_file", words.get(3));
            specs.push(spec);
        }
        ("protect" | "target", _) => {
            let preserve = word(0) == "protect";
            let mut spec = flow_event(
                "SELECTION_CREATED",
                "F01",
                if preserve { "PRESERVE" } else { "TARGET" },
                if preserve {
                    "Lines marked as preserved."
                } else {
                    "Lines marked as targeted."
                },
            );
            spec.set(
                "resource",
                "resource",
                ["FILE_WRITE", "SELECTION", "CRANE_METADATA"],
            )
            .set("resource", "resource_ids", [word(1)])
            .set(
                "resource",
                "resource_classification",
                [if preserve { "PROTECTED" } else { "TARGETED" }],
            )
            .set(
                "assessment",
                "assessment_checks",
                ["INTEGRITY", "LINES_UNCHANGED_SINCE_CHECKPOINT"],
            )
            .set("other", "path", word(1))
            .set("other", "policy", argument(&words, "policy"))
            .set("other", "start_line", argument(&words, "start-line"))
            .set("other", "end_line", argument(&words, "end-line"))
            .set("other", "checkpoint", argument(&words, "checkpoint"))
            .set("other", "name", argument(&words, "name"))
            .set(
                "other",
                "change_type",
                argument(&words, "change_type").map_or_else(
                    || if preserve { "NA".into() } else { "ANY".into() },
                    |kind| kind.to_ascii_uppercase(),
                ),
            );
            if let Some(id) = event
                .payload
                .get("notes")
                .and_then(Value::as_array)
                .and_then(|notes| notes.iter().find(|note| note["kind"] == "SELECTION_ID"))
            {
                spec.set("other", "selection_id", &id["id"]);
            }
            specs.push(spec);
        }
        ("add", "policy") => {
            let mut spec = flow_event(
                "SELECTION_POLICY_CHANGED",
                "F01",
                "MODIFY",
                "Selection moved to another policy.",
            );
            spec.set("resource", "resource", ["SELECTION", "CRANE_METADATA"])
                .set("other", "to_policy", words.get(2))
                .set("other", "selection_id", argument(&words, "marker"));
            specs.push(spec);
        }
        ("--set", _) => {
            let mut spec = flow_event("DEFAULT_CHANGED", "F01", "SET_DEFAULT", "Default changed.");
            spec.set("other", "default_kind", word(2).to_ascii_uppercase())
                .set("other", "value", words.get(3));
            specs.push(spec);
        }
        ("governance", "sign") => {
            let mut spec = flow_event(
                "GOVERNANCE_SIGNED",
                "F01",
                "SIGN_GOVERNANCE",
                "Zones and flows signed.",
            );
            spec.set("resource", "resource", ["CRANE_METADATA"])
                .set("resource", "resource_ids", [".crane/governance.yaml"])
                .set(
                    "assessment",
                    "assessment_checks",
                    [
                        "YAML_VALID",
                        "REFERENCES_VALID",
                        "IDS_UNIQUE",
                        "FLOWS_RESOLVED",
                    ],
                );
            specs.push(spec);
        }
        ("add", "policy-context") => {
            let mut spec = flow_event(
                "CONTEXT_ATTACHED",
                "F20",
                "CONTEXT",
                "Context file attached to a policy.",
            );
            spec.set("resource", "resource", ["FILE_READ", "CRANE_METADATA"])
                .set("other", "path", words.get(3))
                .set("other", "policy", argument(&words, "policy"));
            specs.push(spec);
        }
        ("policy-context", _) => {
            let mut spec = flow_event("CONTEXT_VIEWED", "F20", "CONTEXT", "Policy context viewed.");
            spec.severity("INFO", "FIXED", "NONE")
                .set("other", "policy", words.get(1));
            specs.push(spec);
        }
        ("admin", "add" | "del") => {
            let add = word(1) == "add";
            let mut spec = flow_event(
                if add { "ADMIN_ADDED" } else { "ADMIN_REMOVED" },
                "F02",
                if add { "ADD_ADMIN" } else { "REMOVE_ADMIN" },
                if add {
                    "Admin added to the signed list."
                } else {
                    "Admin removed from the signed list."
                },
            );
            spec.set("resource", "resource", ["CRANE_METADATA", "SIGNING_KEY"])
                .set("other", "admin_key", words.get(2))
                .set("other", "admin_name", argument(&words, "name"));
            specs.push(spec);
        }
        ("admin", "register") => {
            let mut spec = flow_event(
                "OWNER_REGISTRATION_REQUESTED",
                "F02",
                "REGISTER_OWNER",
                "Owner key confirmation requested from GitHub admin.",
            );
            spec.set(
                "resource",
                "resource",
                ["SIGNING_KEY", "UPLOAD_KEY", "NETWORK"],
            )
            .set("bindings", "third_party", ["FOXX_SERVICE"])
            .set(
                "bindings",
                "binding_result",
                if ok { "OK" } else { "FAILED" },
            );
            specs.push(spec);
        }
        ("agent", "link") => {
            let mut spec = flow_event(
                "MACHINE_LINK_REQUESTED",
                "F02",
                "REQUEST_LINK",
                "Machine link to a GitHub account requested.",
            );
            spec.set("resource", "resource", ["UPLOAD_KEY", "NETWORK"])
                .set("bindings", "third_party", ["FOXX_SERVICE"])
                .set(
                    "bindings",
                    "binding_result",
                    if ok { "OK" } else { "FAILED" },
                );
            specs.push(spec);
        }
        ("agent", "install" | "uninstall" | "hooks") => {
            let (name, action, statement) = match word(1) {
                "install" => ("HOOKS_INSTALLED", "HOOKS", "Agent hooks installed."),
                "uninstall" => ("HOOKS_REMOVED", "HOOKS", "Agent hooks removed."),
                _ => ("HOOKS_VALIDATED", "HOOKS", "Agent hooks validated."),
            };
            let mut spec = flow_event(name, "F15", action, statement);
            spec.set("resource", "resource", ["HOOK_SETTINGS"])
                .set("resource", "resource_classification", ["METADATA"])
                .set("bindings", "third_party", ["CLAUDE_CODE", "CODEX"])
                .set(
                    "other",
                    "profile",
                    argument(&words, "--profile").map(|profile| profile.to_ascii_uppercase()),
                );
            if word(1) == "hooks" {
                spec.respond(
                    if ok { "VALID" } else { "INVALID" },
                    if ok { "NA" } else { "EVENTS_MISSING" },
                    statement,
                );
            }
            if word(1) == "uninstall" && refused {
                spec.respond(
                    "REFUSED",
                    "AGENT_REFUSED",
                    "Hook removal refused: an agent tried it.",
                );
            }
            specs.push(spec);
        }
        ("test", _) => {
            let mut spec = flow_event("POLICY_TEST_RUN", "F19", "TEST", "Policy tests run.");
            spec.respond(
                if ok { "PASS" } else { "FAIL" },
                if ok { "NA" } else { "POLICY" },
                "Policy tests run.",
            )
            .set(
                "assessment",
                "assessment",
                if ok { "COMPLIANT" } else { "VIOLATION" },
            )
            .set(
                "assessment",
                "assessment_checks",
                [
                    "PRESERVES_UNCHANGED",
                    "TARGETS_CHANGED",
                    "SELECTIONS_RESOLVED",
                    "PROJECT_TESTS",
                ],
            )
            .severity(if ok { "INFO" } else { "HIGH" }, "OUTCOME", "NONE");
            specs.push(spec);
        }
        ("integrate", _) => {
            let mut spec = flow_event(
                "INTEGRATION_CONFIGURED",
                "F22",
                "INTEGRATION",
                "Integration configured; secrets kept on this machine.",
            );
            spec.set("resource", "resource", ["TRUST_DIRECTORY", "NETWORK"])
                .set("bindings", "third_party", [word(1).to_ascii_uppercase()])
                .set("other", "integration", word(1).to_ascii_uppercase());
            specs.push(spec);
        }
        ("validate" | "parse" | "task" | "session", _) | ("admin", "list") => {
            let inspection = match (word(0), word(1)) {
                ("validate", _) => "VALIDATE",
                ("parse", _) => "PARSE_FILE",
                ("task", "current") => "TASK_CURRENT",
                ("task", _) => "TASK_LIST",
                ("session", "current") => "SESSION_CURRENT",
                ("session", _) => "SESSION_LIST",
                _ => "ADMIN_LIST",
            };
            let mut spec = flow_event("STATE_INSPECTED", "F35", "INSPECT", "State inspected.");
            spec.severity("INFO", "FIXED", "NONE")
                .set("resource", "resource", ["CRANE_METADATA", "FILE_READ"])
                .set("other", "inspection", inspection)
                .set(
                    "other",
                    "output_format",
                    if words.iter().any(|word| word == "--json") {
                        "JSON"
                    } else {
                        "TEXT"
                    },
                );
            specs.push(spec);
        }
        _ => {}
    }
    if word(0) == "policy" && word(2) == "status" {
        let mut spec = flow_event(
            "ENFORCEMENT_SELF_TEST",
            "F19",
            "SELF_TEST",
            "Enforcement self-test run.",
        );
        spec.respond(
            if ok { "PASS" } else { "GAP_FOUND" },
            if ok { "NA" } else { "PRESERVE_WRITE_ALLOWED" },
            "Enforcement self-test run.",
        )
        .set(
            "assessment",
            "assessment",
            if ok { "COMPLIANT" } else { "VIOLATION" },
        )
        .set(
            "assessment",
            "assessment_checks",
            [
                "SIMULATED_PRESERVE_WRITE",
                "SIMULATED_TARGET_WRITE",
                "SIMULATED_METADATA_WRITE",
            ],
        )
        .severity(if ok { "INFO" } else { "CRITICAL" }, "OUTCOME", "NONE")
        .set("other", "policy", word(1));
        specs.push(spec);
    }
    if governance_change
        && !ok
        && matches!(
            failed,
            "AGENT_REFUSED"
                | "NOT_ADMIN"
                | "VERIFICATION_FAILED"
                | "CONFLICT"
                | "KEY_UNAVAILABLE"
                | "NOT_OWNER"
        )
    {
        let mut spec = base(event, "GOVERNANCE_CHANGE_REFUSED", "F01");
        spec.set("action", "action_type", &command_name)
            .respond(
                "REFUSED",
                failed,
                "Rules change refused before anything was written.",
            )
            .set("decision", "decision", "DENY")
            .set(
                "decision",
                "decision_rule",
                match failed {
                    "AGENT_REFUSED" => "HUMAN_ONLY",
                    "NOT_ADMIN" | "NOT_OWNER" => "ADMIN_ONLY",
                    _ => "STATE_MUST_VERIFY",
                },
            )
            .set("decision", "enforcement", "PREVENTED")
            .severity(
                if failed == "AGENT_REFUSED" {
                    "HIGH"
                } else {
                    "MEDIUM"
                },
                "OUTCOME",
                "NONE",
            );
        specs.push(spec);
    }
    if failed == "CONFLICT" {
        let mut spec = base(event, "RULES_CONFLICT_DETECTED", "F23");
        spec.set("action", "action_type", "RULES_MERGE")
            .set("resource", "resource", ["CRANE_METADATA", "GIT"])
            .respond(
                "REFUSED",
                "CONFLICT",
                "Rules conflict after merging two admins' changes.",
            )
            .set("assessment", "assessment", "UNRESOLVED")
            .set("assessment", "assessment_checks", ["CONFLICT_MARKERS"])
            .set("decision", "decision", "DENY")
            .set("decision", "decision_rule", "INTEGRITY_FAILED")
            .set("decision", "enforcement", "PREVENTED")
            .severity("HIGH", "FIXED", "NONE");
        specs.push(spec);
    }
    if event.environment == "ci" && matches!(word(0), "validate" | "test") {
        let mut spec = base(event, "HEADLESS_VERIFICATION", "F24");
        spec.set(
            "action",
            "action_type",
            if word(0) == "test" {
                "TEST"
            } else {
                "VERIFY_RULES"
            },
        )
        .set("action", "action_source", "CI")
        .set(
            "resource",
            "resource",
            ["CRANE_METADATA", "SELECTION", "GIT"],
        )
        .respond(
            if ok { "PASS" } else { "FAIL" },
            if ok { "NA" } else { "VERIFICATION_FAILED" },
            "Rules checked on a build server.",
        )
        .set(
            "assessment",
            "assessment",
            if ok { "COMPLIANT" } else { "VIOLATION" },
        )
        .set("assessment", "assessment_checks", ["INTEGRITY", "POLICIES"])
        .severity(if ok { "INFO" } else { "HIGH" }, "OUTCOME", "NONE")
        .set("bindings", "third_party", ["GITHUB_ACTIONS"])
        .set(
            "other",
            "owner_key_source",
            if std::env::var_os("CRANE_TRUSTED_PUBLIC_KEYS").is_some() {
                "CONFIGURED"
            } else {
                "PINNED"
            },
        );
        specs.push(spec);
    }
    specs
}

/** Specification events the Landscape extension asked Crane to record (F31)
 * Input
    - event: &Event - foxx.event event (payload: name, and the event's facts)
 * Output
    - Vec<Spec>
*/
pub(super) fn foxx(event: &Event) -> Vec<Spec> {
    let payload = &event.payload;
    let name = payload
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let action_type = match name {
        "FOXX_SETUP" => "SETUP",
        "FOXX_PANEL_OPENED" | "FOXX_PANEL_REFUSED" => "OPEN_PANEL",
        "FOXX_ACTION_RUN" => "PANEL_ACTION",
        "FOXX_GOVERNANCE_REVERTED" => "REVERT_GOVERNANCE",
        "FOXX_GOVERNANCE_UNDONE" => "UNDO_GOVERNANCE",
        "FOXX_VALIDATED_ON_SAVE" => "VALIDATE_ON_SAVE",
        _ => return Vec::new(),
    };
    let status = payload
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("OK");
    let failure_state = payload
        .get("failure_state")
        .and_then(Value::as_str)
        .unwrap_or("NA");
    let ok = failure_state == "NA";
    let mut spec = base(event, name, "F31");
    spec.set("action", "action_type", action_type)
        .set("action", "action_source", "EXTENSION")
        .respond(
            status,
            failure_state,
            payload
                .get("statement")
                .and_then(Value::as_str)
                .unwrap_or("Landscape editor action."),
        )
        .severity(if ok { "LOW" } else { "MEDIUM" }, "FAILURE_STREAK", "NONE")
        .set("bindings", "third_party", ["VSCODE"])
        .set("telemetry", "agent_state", "INACTIVE");
    match name {
        "FOXX_SETUP" => {
            spec.set("bindings", "third_party", ["VSCODE", "GITHUB"])
                .set(
                    "bindings",
                    "binding_result",
                    if status == "OK" { "OK" } else { "FAILED" },
                );
        }
        "FOXX_GOVERNANCE_REVERTED" | "FOXX_GOVERNANCE_UNDONE" => {
            spec.set("resource", "resource", ["CRANE_METADATA"])
                .set("resource", "resource_ids", [".crane/governance.yaml"])
                .severity(if ok { "LOW" } else { "MEDIUM" }, "OUTCOME", "NONE");
        }
        "FOXX_VALIDATED_ON_SAVE" => {
            let assessment = payload
                .get("assessment")
                .and_then(Value::as_str)
                .unwrap_or("COMPLIANT");
            spec.set("assessment", "assessment", assessment)
                .set(
                    "assessment",
                    "assessment_checks",
                    ["INTEGRITY", "POLICIES", "SELECTIONS"],
                )
                .severity(
                    match assessment {
                        "COMPLIANT" => "INFO",
                        "VIOLATION" => "HIGH",
                        _ => "MEDIUM",
                    },
                    "OUTCOME",
                    "NONE",
                );
        }
        _ => {}
    }
    for key in [
        "opened_from",
        "enclosing_function",
        "panel_action",
        "search_query",
        "enforce_now",
        "role",
        "saved_file",
        "extension_version",
    ] {
        if let Some(value) = payload.get(key) {
            spec.set("other", key, value);
        }
    }
    for key in [
        "selected_lines",
        "diagnostics",
        "selections_failing",
        "duration_ms",
    ] {
        if let Some(value) = payload.get(key) {
            spec.set("eval", key, value);
        }
    }
    vec![spec]
}

/** Specification events of the notes attached to an event (F02, F19, F23, F25, F26)
 * Input
    - event: &Event - any stored event
 * Output
    - Vec<Spec>
*/
pub(super) fn notes(event: &Event) -> Vec<Spec> {
    let mut specs = Vec::new();
    if let Some(cache) = event.payload.get("identity_cache").and_then(Value::as_str) {
        if matches!(cache, "MISS" | "REFRESHED") {
            let mut spec = base(event, "IDENTITY_CACHE_USED", "F26");
            spec.set("action", "action_type", "CACHE")
                .set("resource", "resource", ["TRUST_DIRECTORY", "GIT"])
                .respond(
                    cache,
                    if cache == "REFRESHED" {
                        "EXPIRED"
                    } else {
                        "NA"
                    },
                    "Repository identity computed and remembered.",
                )
                .severity("INFO", "FIXED", "NONE");
            specs.push(spec);
        }
    }
    for note in event
        .payload
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = note["kind"].as_str().unwrap_or_default();
        let text = |key: &str| note.get(key).cloned().unwrap_or(Value::Null);
        let spec = match kind {
            "KEY_CREATED" | "KEY_PROTECTION_UPGRADED" | "KEY_UNLOCKED" | "KEY_UNLOCK_FAILED" => {
                let ok = kind != "KEY_UNLOCK_FAILED";
                let mut spec = base(event, kind, "F25");
                spec.set("action", "action_type", "KEY")
                    .set("resource", "resource", ["SIGNING_KEY", "TRUST_DIRECTORY"])
                    .set("resource", "resource_classification", ["TRUST"])
                    .respond(
                        if ok { "OK" } else { "FAILED" },
                        note["failure_state"].as_str().unwrap_or("NA"),
                        match kind {
                            "KEY_CREATED" => "Signing key created and sealed.",
                            "KEY_PROTECTION_UPGRADED" => {
                                "Signing key sealed with stronger protection."
                            }
                            "KEY_UNLOCKED" => "Signing key unlocked for a signing command.",
                            _ => "Signing key could not be unlocked.",
                        },
                    )
                    .severity(
                        if ok { "INFO" } else { "MEDIUM" },
                        if ok { "FIXED" } else { "FAILURE_STREAK" },
                        "NONE",
                    )
                    .set(
                        "bindings",
                        "third_party",
                        [
                            if note["protection"] == "DPAPI" || note["to_protection"] == "DPAPI" {
                                "WINDOWS_DPAPI"
                            } else {
                                "NONE"
                            },
                        ],
                    )
                    .set("other", "key_kind", text("key_kind"))
                    .set("other", "protection", text("protection"))
                    .set("other", "from_protection", text("from_protection"))
                    .set("other", "to_protection", text("to_protection"))
                    .set("other", "public_key", text("public_key"));
                spec
            }
            "CHECKPOINT_COMMIT_CHECKED" => {
                let missing = note["result"] == "MISSING";
                let mut spec = base(event, kind, "F26");
                spec.set("action", "action_type", "CACHE")
                    .set("resource", "resource", ["TRUST_DIRECTORY", "GIT"])
                    .respond(
                        note["result"].as_str().unwrap_or("VERIFIED_AND_REMEMBERED"),
                        if missing { "COMMIT_NOT_FETCHED" } else { "NA" },
                        if missing {
                            "Checkpoint commit is missing locally."
                        } else {
                            "Checkpoint commit verified and remembered."
                        },
                    )
                    .severity(if missing { "HIGH" } else { "INFO" }, "OUTCOME", "NONE")
                    .set("other", "checkpoint_name", text("checkpoint"))
                    .set("other", "commit", text("commit"));
                spec
            }
            "OWNER_KEY_LOOKUP" => {
                let result = note["result"].as_str().unwrap_or("UNAVAILABLE");
                let mut spec = base(event, kind, "F02");
                spec.set("action", "action_type", "OWNER_LOOKUP")
                    .set("resource", "resource", ["NETWORK"])
                    .respond(
                        result,
                        "NA",
                        match result {
                            "MATCH" => "Owner key matches the GitHub-confirmed key.",
                            "MISMATCH" => "Owner key differs from the GitHub-confirmed key.",
                            "NONE_CONFIRMED" => "No GitHub-confirmed owner key yet.",
                            _ => "Service could not be asked.",
                        },
                    )
                    .set(
                        "assessment",
                        "assessment",
                        match result {
                            "MATCH" => "COMPLIANT",
                            "MISMATCH" => "SUSPICIOUS",
                            _ => "UNRESOLVED",
                        },
                    )
                    .set("assessment", "assessment_checks", ["GITHUB_CONFIRMATION"])
                    .severity(
                        match result {
                            "MATCH" => "INFO",
                            "MISMATCH" => "HIGH",
                            _ => "LOW",
                        },
                        "OUTCOME",
                        "NONE",
                    )
                    .set("bindings", "third_party", ["FOXX_SERVICE"])
                    .set("other", "confirmed_by", text("confirmed_by"))
                    .set("other", "confirmed_key", text("confirmed_key"));
                spec
            }
            "OWNER_KEY_TRUSTED" | "OWNER_KEY_DECLINED" => {
                let trusted = kind == "OWNER_KEY_TRUSTED";
                let mut spec = base(event, kind, "F02");
                spec.set(
                    "action",
                    "action_type",
                    if trusted {
                        "TRUST_OWNER"
                    } else {
                        "DECLINE_OWNER"
                    },
                )
                .set("resource", "resource", ["TRUST_DIRECTORY", "SIGNING_KEY"])
                .respond(
                    if trusted { "OK" } else { "DECLINED" },
                    "NA",
                    if trusted {
                        "Owner key trusted after comparing fingerprints."
                    } else {
                        "Owner key declined; agent changes stay blocked."
                    },
                )
                .set(
                    "decision",
                    "decision",
                    if trusted { "ALLOW" } else { "DENY" },
                )
                .set("decision", "decision_rule", "HUMAN_CONFIRMATION")
                .severity(if trusted { "LOW" } else { "MEDIUM" }, "OUTCOME", "NONE")
                .set("other", "owner_key", text("owner_key"))
                .set("other", "lookup_result", text("lookup_result"));
                spec
            }
            "FOREIGN_RULES_ACCEPTED" => {
                let mut spec = base(event, kind, "F23");
                spec.set("action", "action_type", "RULES_MERGE")
                    .set(
                        "resource",
                        "resource",
                        ["CRANE_METADATA", "TRUST_DIRECTORY"],
                    )
                    .respond(
                        "ACCEPTED",
                        "NA",
                        "Another admin's signed rules accepted on this machine.",
                    )
                    .set("assessment", "assessment", "COMPLIANT")
                    .set("assessment", "assessment_checks", ["SIGNATURES", "SIGNERS"])
                    .set("decision", "decision", "ALLOW")
                    .set("decision", "decision_rule", "HUMAN_CONFIRMATION")
                    .severity("LOW", "OUTCOME", "NONE")
                    .set("eval", "accepted_generation", text("generation"));
                spec
            }
            "POLICY_RULE_RESULT" => {
                let outcome = note["outcome"].as_str().unwrap_or("UNRESOLVED");
                let pass = outcome == "PASS";
                let mut spec = base(event, kind, "F19");
                spec.set("action", "action_type", "TEST")
                    .set("resource", "resource", ["SELECTION"])
                    .set(
                        "resource",
                        "resource_ids",
                        [note["id"].as_str().unwrap_or_default()],
                    )
                    .set(
                        "resource",
                        "resource_classification",
                        [if note["operation"] == "target" {
                            "TARGETED"
                        } else {
                            "PROTECTED"
                        }],
                    )
                    .respond(
                        outcome,
                        if pass {
                            "NA"
                        } else if outcome == "UNRESOLVED" {
                            "SELECTION_UNRESOLVED"
                        } else if note["operation"] == "target" {
                            "TARGET_NOT_CHANGED"
                        } else {
                            "PRESERVE_CHANGED"
                        },
                        note["message"].as_str().unwrap_or("Policy rule checked."),
                    )
                    .set(
                        "assessment",
                        "assessment",
                        if pass {
                            "COMPLIANT"
                        } else if outcome == "UNRESOLVED" {
                            "UNRESOLVED"
                        } else {
                            "VIOLATION"
                        },
                    )
                    .set(
                        "assessment",
                        "assessment_checks",
                        ["CONTENT_DIGEST", "CHANGE_CLASSIFICATION"],
                    )
                    .severity(if pass { "INFO" } else { "HIGH" }, "OUTCOME", "NONE")
                    .set("other", "policy", text("policy"))
                    .set(
                        "other",
                        "operation",
                        note["operation"].as_str().map(str::to_ascii_uppercase),
                    )
                    .set(
                        "other",
                        "change_type_required",
                        note["change_type"]
                            .as_str()
                            .map_or_else(|| "NA".to_string(), str::to_ascii_uppercase),
                    );
                spec
            }
            "PROJECT_TEST_RESULT" => {
                let pass = note["outcome"] == "PASS";
                let mut spec = base(event, kind, "F19");
                spec.set("action", "action_type", "TEST")
                    .set("resource", "resource", ["COMMAND"])
                    .set(
                        "resource",
                        "resource_ids",
                        [note["name"].as_str().unwrap_or_default()],
                    )
                    .respond(
                        note["outcome"].as_str().unwrap_or("FAIL"),
                        if pass {
                            "NA"
                        } else if note["outcome"] == "TIMEOUT" {
                            "TIMEOUT"
                        } else {
                            "EXIT_CODE"
                        },
                        "Project test command run.",
                    )
                    .set(
                        "assessment",
                        "assessment",
                        if pass { "COMPLIANT" } else { "VIOLATION" },
                    )
                    .set("assessment", "assessment_checks", ["PROJECT_TESTS"])
                    .severity(if pass { "INFO" } else { "HIGH" }, "OUTCOME", "NONE")
                    .set("eval", "exit_code", text("exit_code"))
                    .set("eval", "duration_ms", text("duration_ms"))
                    .set("other", "test_name", text("name"));
                spec
            }
            "INTEGRATION_TESTED" => {
                let pass = note["result"] == "OK";
                let mut spec = base(event, kind, "F22");
                spec.set("action", "action_type", "TEST_INTEGRATION")
                    .set("resource", "resource", ["NETWORK"])
                    .respond(
                        if pass { "OK" } else { "FAILED" },
                        if pass { "NA" } else { "HTTP_ERROR" },
                        if pass {
                            "Integration test message delivered."
                        } else {
                            "Integration test failed."
                        },
                    )
                    .severity(
                        if pass { "LOW" } else { "MEDIUM" },
                        "FAILURE_STREAK",
                        "NONE",
                    )
                    .set(
                        "bindings",
                        "third_party",
                        [note["integration"]
                            .as_str()
                            .unwrap_or("NONE")
                            .to_ascii_uppercase()],
                    )
                    .set(
                        "bindings",
                        "binding_result",
                        if pass { "OK" } else { "FAILED" },
                    )
                    .set(
                        "other",
                        "integration",
                        note["integration"].as_str().map(str::to_ascii_uppercase),
                    );
                spec
            }
            brick if brick.starts_with("BRICK_") => {
                match super::spec_bricks::spec(event, brick, note) {
                    Some(spec) => spec,
                    None => continue,
                }
            }
            flow if flow.starts_with("FLOW_") => match super::spec_flows::spec(event, flow, note) {
                Some(spec) => spec,
                None => continue,
            },
            resident if resident.starts_with("RESIDENT_") => {
                match super::spec_resident::spec(event, resident, note) {
                    Some(spec) => spec,
                    None => continue,
                }
            }
            _ => continue,
        };
        specs.push(spec);
    }
    specs
}
