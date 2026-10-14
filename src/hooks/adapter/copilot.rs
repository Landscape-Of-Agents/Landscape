// GitHub Copilot CLI hooks, translated to the provider-neutral enforcement engine.

use super::*;
use crate::hooks::action::{FileChange, Operation, Proposed, TextEdit};
use crate::telemetry::model::Decision;
use serde_json::{json, Value};

pub(crate) struct CopilotAdapter;

impl AgentAdapter for CopilotAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Copilot
    }

    fn protected_paths(&self) -> &'static [&'static str] {
        &[".github/hooks/crane.json"]
    }

    fn translate(&self, event: HookEvent, payload: &Value) -> ProviderEvent {
        provider_event(payload, event.has_action().then(|| copilot_action(payload)))
    }

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

    fn hook_command(&self, event: HookEvent) -> String {
        format!(
            "crane agent hook --event {} --profile copilot",
            event.name()
        )
    }

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

    fn respond_context(&self, context: &str) -> Result<(), String> {
        println!("{}", json!({"additionalContext": context}));
        Ok(())
    }

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

fn copilot_input(payload: &Value) -> Value {
    match payload.get("toolArgs") {
        Some(Value::String(text)) => {
            serde_json::from_str(text).unwrap_or_else(|_| json!({"command": text}))
        }
        Some(value) => value.clone(),
        None => Value::Null,
    }
}
