// Gemini CLI hooks, translated to the provider-neutral enforcement engine.

use super::*;
use crate::hooks::action::Operation;
use crate::telemetry::model::Decision;
use serde_json::{json, Value};

pub(crate) struct GeminiAdapter;

impl AgentAdapter for GeminiAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Gemini
    }

    fn protected_paths(&self) -> &'static [&'static str] {
        &[".gemini/settings.json"]
    }

    fn translate(&self, event: HookEvent, payload: &Value) -> ProviderEvent {
        let action = event.has_action().then(|| gemini_action(payload));
        provider_event(payload, action)
    }

    fn hook_config(&self) -> Option<HookConfig> {
        Some(HookConfig {
            file: ".gemini/settings.json",
            events: &[
                (HookEvent::SessionStart, false),
                (HookEvent::UserPromptSubmit, false),
                (HookEvent::PreToolUse, true),
                (HookEvent::PostToolUse, true),
                (HookEvent::Stop, false),
                (HookEvent::SessionEnd, false),
            ],
            matcher: ".*",
            deny: &[],
            timeout: 120_000,
            format: HookConfigFormat::Grouped,
        })
    }

    fn hook_event_name(&self, event: HookEvent) -> &'static str {
        match event {
            HookEvent::SessionStart => "SessionStart",
            HookEvent::UserPromptSubmit => "BeforeAgent",
            HookEvent::PreToolUse => "BeforeTool",
            HookEvent::PostToolUse => "AfterTool",
            HookEvent::PostToolUseFailure => "AfterTool",
            HookEvent::Stop => "AfterAgent",
            HookEvent::SessionEnd => "SessionEnd",
            HookEvent::PermissionRequest => "PermissionRequest",
        }
    }

    fn respond_action(&self, verdict: &Verdict) -> Result<(), String> {
        let reasons = verdict.reasons.join("; ");
        let mut response = match verdict.decision {
            Decision::Allow => json!({"decision": "allow"}),
            Decision::RequireApproval => json!({
                "decision": "deny",
                "reason": format!("Crane requires human approval for this tool call: {reasons}"),
            }),
            _ => json!({
                "decision": "deny",
                "reason": format!("Crane denied this tool call: {reasons}"),
            }),
        };
        if let Some(message) = &verdict.system_message {
            response["systemMessage"] = json!(message);
        }
        if let Some(context) = &verdict.context {
            response["hookSpecificOutput"] = json!({"additionalContext": context});
        }
        println!("{response}");
        Ok(())
    }

    fn respond_feedback(&self, event: HookEvent, feedback: &Feedback) -> Result<(), String> {
        let mut response = if feedback.block {
            json!({
                "decision": "deny",
                "reason": format!("{}\n{}", feedback.summary, feedback.detail),
            })
        } else {
            json!({})
        };
        if !feedback.summary.is_empty() {
            response["systemMessage"] = json!(format!("Crane: {}", feedback.summary));
        }
        if !feedback.detail.is_empty()
            && matches!(event, HookEvent::PostToolUse | HookEvent::UserPromptSubmit)
        {
            response["hookSpecificOutput"] = json!({"additionalContext": feedback.detail});
        }
        println!("{response}");
        Ok(())
    }

    fn respond_context(&self, context: &str) -> Result<(), String> {
        println!(
            "{}",
            json!({
                "hookSpecificOutput": {"additionalContext": context},
                "systemMessage": "Crane supplied repository governance context for this session.",
            })
        );
        Ok(())
    }

    fn respond_failure(
        &self,
        event: HookEvent,
        error: &str,
        stop_hook_active: bool,
    ) -> Result<(), String> {
        let response = match event {
            HookEvent::PreToolUse => json!({
                "decision": "deny",
                "reason": format!("Crane could not authorize this tool call: {error}"),
            }),
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => json!({
                "decision": "deny",
                "reason": format!("Crane could not verify this tool call: {error}"),
            }),
            HookEvent::SessionStart => {
                return Err(format!("Crane could not start the session: {error}"));
            }
            HookEvent::Stop if !stop_hook_active => json!({
                "decision": "deny",
                "reason": format!("Crane could not reconcile the session: {error}"),
            }),
            _ => json!({"systemMessage": format!("Crane: {error}")}),
        };
        println!("{response}");
        Ok(())
    }
}

fn gemini_action(payload: &Value) -> AgentAction {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input = payload.get("tool_input").unwrap_or(&Value::Null);
    let copilot_name = match tool.to_ascii_lowercase().as_str() {
        "run_shell_command" | "run_terminal_command" => "run_command",
        "write_file" | "writefile" => "write_file",
        "replace" | "edit" => "edit",
        "read_file" | "readfile" | "list_directory" | "grep_search" | "google_web_search" => {
            "read_file"
        }
        _ => tool,
    };
    let normalized = json!({"toolName": copilot_name, "toolArgs": input});
    let mut action = super::copilot::copilot_action(&normalized);
    action.tool = tool.to_string();
    if action.operation == Operation::Other && matches!(tool, "run_shell_command") {
        action.operation = Operation::Execute;
    }
    action
}
