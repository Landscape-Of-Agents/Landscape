// GitHub Copilot CLI hooks, translated to the provider-neutral enforcement engine.

use super::*;
use crate::hooks::action::{FileChange, Operation, Proposed, TextEdit};
use crate::telemetry::model::Decision;
use serde_json::{json, Value};

/** Adapter for the GitHub Copilot CLI: translates its camelCase hook payloads into neutral actions and answers in Copilot's permissionDecision format
 * Fields
    - None
*/
pub(crate) struct CopilotAdapter;

impl AgentAdapter for CopilotAdapter {
    /** Report the Copilot profile
     * Input
        - None (uses self)
     * Output
        - AgentKind::Copilot
    */
    fn kind(&self) -> AgentKind {
        AgentKind::Copilot
    }

    /** Protect the Copilot file that registers Crane's hooks, so an agent cannot unhook Crane by editing it
     * Input
        - None (uses self)
     * Output
        - &'static [&'static str]
    */
    fn protected_paths(&self) -> &'static [&'static str] {
        &[".github/hooks/crane.json"]
    }

    /** Translate a Copilot hook payload; only events that carry a tool call get a neutral action
     * Input
        - event: HookEvent - lifecycle event
        - payload: &Value - Copilot hook JSON
     * Output
        - ProviderEvent
    */
    fn translate(&self, event: HookEvent, payload: &Value) -> ProviderEvent {
        provider_event(payload, event.has_action().then(|| copilot_action(payload)))
    }

    /** Describe Copilot's project hook configuration: a flat list of handlers per event in .github/hooks/crane.json, every tool matched
     * Input
        - None (uses self)
     * Output
        - Option<HookConfig>
    */
    fn hook_config(&self) -> Option<HookConfig> {
        Some(HookConfig {
            file: ".github/hooks/crane.json",
            events: &[
                (HookEvent::SessionStart, false),
                (HookEvent::UserPromptSubmit, false),
                (HookEvent::PreToolUse, false),
                (HookEvent::PostToolUse, false),
                (HookEvent::PostToolUseFailure, false),
                (HookEvent::Stop, false),
                (HookEvent::SessionEnd, false),
            ],
            matcher: "*",
            deny: &[],
            timeout: 120,
            format: HookConfigFormat::Flat,
        })
    }

    /** Map a neutral lifecycle event to the event name Copilot uses in its hook file
     * Input
        - event: HookEvent - lifecycle event
     * Output
        - &'static str
    */
    fn hook_event_name(&self, event: HookEvent) -> &'static str {
        match event {
            HookEvent::SessionStart => "sessionStart",
            HookEvent::UserPromptSubmit => "userPromptSubmitted",
            HookEvent::PreToolUse => "preToolUse",
            HookEvent::PostToolUse => "postToolUse",
            HookEvent::PostToolUseFailure => "postToolUseFailure",
            HookEvent::Stop => "agentStop",
            HookEvent::SessionEnd => "sessionEnd",
            HookEvent::PermissionRequest => "permissionRequest",
        }
    }

    /** Build the command Copilot runs for an event: the Crane hook entry point with the copilot profile
     * Input
        - event: HookEvent - lifecycle event
     * Output
        - String
    */
    fn hook_command(&self, event: HookEvent) -> String {
        format!(
            "crane agent hook --event {} --profile copilot",
            event.name()
        )
    }

    /** Answer a tool call: any message is printed first as a progress line, then the decision as allow, ask (human approval) or deny with the reasons
     * Input
        - verdict: &Verdict - the enforcement decision
     * Output
        - Result<(), String>
    */
    fn respond_action(&self, verdict: &Verdict) -> Result<(), String> {
        if let Some(message) = verdict
            .system_message
            .as_deref()
            .or(verdict.context.as_deref())
        {
            println!("{}", json!({"type": "progress", "message": message}));
        }
        let reasons = verdict.reasons.join("; ");
        let response = match verdict.decision {
            Decision::Allow => json!({"permissionDecision": "allow"}),
            Decision::RequireApproval => json!({
                "permissionDecision": "ask",
                "permissionDecisionReason": format!("Crane requires human approval: {reasons}"),
            }),
            _ => json!({
                "permissionDecision": "deny",
                "permissionDecisionReason": format!("Crane denied this tool call: {reasons}"),
            }),
        };
        println!("{response}");
        Ok(())
    }

    /** Answer after a tool ran or the agent stopped: the summary as a progress line, the detail attached to the tool result or failure, and a block on Stop when work is unfinished
     * Input
        - event: HookEvent - lifecycle event
        - feedback: &Feedback - what Crane found
     * Output
        - Result<(), String>
    */
    fn respond_feedback(&self, event: HookEvent, feedback: &Feedback) -> Result<(), String> {
        if !feedback.summary.is_empty() {
            println!(
                "{}",
                json!({"type": "progress", "message": format!("Crane: {}", feedback.summary)})
            );
        }
        let response = match event {
            HookEvent::PostToolUse if !feedback.detail.is_empty() => json!({"modifiedResult": {
                "resultType": "success",
                "textResultForLlm": feedback.detail,
            }}),
            HookEvent::PostToolUseFailure if !feedback.detail.is_empty() => {
                json!({"additionalContext": feedback.detail})
            }
            HookEvent::Stop if feedback.block => json!({
                "decision": "block",
                "reason": format!("{}\n{}", feedback.summary, feedback.detail),
            }),
            _ => json!({}),
        };
        println!("{response}");
        Ok(())
    }

    /** Give the agent extra context (for example at session start)
     * Input
        - context: &str - guidance text
     * Output
        - Result<(), String>
    */
    fn respond_context(&self, context: &str) -> Result<(), String> {
        println!("{}", json!({"additionalContext": context}));
        Ok(())
    }

    /** Answer when Crane itself failed: fail closed by denying tool calls, warn after them, block a first Stop, and refuse to start a session
     * Input
        - event: HookEvent - lifecycle event
        - error: &str - what went wrong
        - stop_hook_active: bool - whether the agent is already continuing because of a Stop block (avoids loops)
     * Output
        - Result<(), String>
        - Error if the session could not start
    */
    fn respond_failure(
        &self,
        event: HookEvent,
        error: &str,
        stop_hook_active: bool,
    ) -> Result<(), String> {
        if matches!(
            event,
            HookEvent::PreToolUse
                | HookEvent::PermissionRequest
                | HookEvent::PostToolUse
                | HookEvent::PostToolUseFailure
        ) {
            println!(
                "{}",
                json!({"type": "progress", "message": format!("Crane: {error}")})
            );
        }
        match event {
            HookEvent::PreToolUse => {
                println!(
                    "{}",
                    json!({
                        "permissionDecision": "deny",
                        "permissionDecisionReason": format!("Crane could not authorize this tool call: {error}"),
                    })
                );
            }
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
                println!(
                    "{}",
                    json!({"additionalContext": format!("Crane could not verify this tool call: {error}")})
                );
            }
            HookEvent::Stop if !stop_hook_active => {
                println!(
                    "{}",
                    json!({"decision": "block", "reason": format!("Crane could not reconcile the session: {error}")})
                );
            }
            HookEvent::SessionStart => {
                return Err(format!("Crane could not start the session: {error}"));
            }
            _ => println!("{{}}"),
        }
        Ok(())
    }
}

/** Turn a Copilot tool call into a neutral action: shell tools become Execute with their command, create/write and edit tools become Write with the proposed content or edits, anything else stays Other. Gemini reuses it after renaming its tools
 * Input
    - payload: &Value - hook JSON with toolName and toolArgs
 * Output
    - AgentAction
*/
pub(super) fn copilot_action(payload: &Value) -> AgentAction {
    let tool = payload
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input = copilot_input(payload);
    let value = &input;
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|key| value.get(*key).and_then(Value::as_str))
            .map(String::from)
    };
    let mut action = skeleton(tool, &input);
    action.arguments = strings(&input);
    let path = text(&["filePath", "file_path", "path"]);
    match tool.to_ascii_lowercase().as_str() {
        "bash" | "powershell" | "shell" | "run_command" => {
            action.operation = Operation::Execute;
            action.command = text(&["command", "cmd"]).or_else(|| input.as_str().map(String::from));
        }
        "create" | "write" | "write_file" => {
            action.operation = Operation::Write;
            if let Some(path) = path {
                action.files.push(FileChange {
                    path,
                    proposed: text(&["content", "text"])
                        .map_or(Proposed::Unknown, Proposed::Content),
                });
            } else {
                action.operation = Operation::Other;
            }
        }
        "edit" | "replace" => {
            action.operation = Operation::Write;
            if let Some(path) = path {
                let proposed = match (
                    text(&["oldString", "old_string", "old"]),
                    text(&["newString", "new_string", "new"]),
                ) {
                    (Some(old), Some(new)) => Proposed::Edits(vec![TextEdit {
                        old,
                        new,
                        all: false,
                    }]),
                    _ => Proposed::Unknown,
                };
                action.files.push(FileChange { path, proposed });
            } else {
                action.operation = Operation::Other;
            }
        }
        "view" | "read" | "read_file" | "grep" | "glob" | "search" => {
            action.operation = Operation::Read;
            action.reads.extend(path);
        }
        _ => {}
    }
    action
}

/** Read Copilot's tool arguments, which may arrive as a JSON string; a string that is not JSON is treated as a shell command
 * Input
    - payload: &Value - hook JSON
 * Output
    - Value (Null when there are no arguments)
*/
fn copilot_input(payload: &Value) -> Value {
    match payload.get("toolArgs") {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| json!({"command": text}))
        }
        Some(value) => value.clone(),
        None => Value::Null,
    }
}
