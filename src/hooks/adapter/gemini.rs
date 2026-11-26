// Gemini CLI hooks, translated to the provider-neutral enforcement engine.

use super::*;
use crate::hooks::action::Operation;
use crate::telemetry::model::Decision;
use serde_json::{json, Value};

/** Adapter for the Gemini CLI: translates its snake_case hook payloads into neutral actions and answers in Gemini's decision format
 * Fields
    - None
*/
pub(crate) struct GeminiAdapter;

impl AgentAdapter for GeminiAdapter {
    /** Report the Gemini profile
     * Input
        - None (uses self)
     * Output
        - AgentKind::Gemini
    */
    fn kind(&self) -> AgentKind {
        AgentKind::Gemini
    }

    /** Protect the Gemini settings file that registers Crane's hooks
     * Input
        - None (uses self)
     * Output
        - &'static [&'static str]
    */
    fn protected_paths(&self) -> &'static [&'static str] {
        &[".gemini/settings.json"]
    }

    /** Translate a Gemini hook payload; only events that carry a tool call get a neutral action
     * Input
        - event: HookEvent - lifecycle event
        - payload: &Value - Gemini hook JSON
     * Output
        - ProviderEvent
    */
    fn translate(&self, event: HookEvent, payload: &Value) -> ProviderEvent {
        let action = event.has_action().then(|| gemini_action(payload));
        provider_event(payload, action)
    }

    /** Describe Gemini's project hook configuration: grouped handlers in .gemini/settings.json, a regex matcher, and a timeout in milliseconds
     * Input
        - None (uses self)
     * Output
        - Option<HookConfig>
    */
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

    /** Map a neutral lifecycle event to Gemini's event name (Before/After Tool and Agent)
     * Input
        - event: HookEvent - lifecycle event
     * Output
        - &'static str
    */
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

    /** Answer a tool call. Gemini has no "ask" answer, so a required approval is a deny that says a human must approve; messages and context ride along
     * Input
        - verdict: &Verdict - the enforcement decision
     * Output
        - Result<(), String>
    */
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

    /** Answer after a tool ran or the agent stopped: deny when Crane blocks, show the summary as a system message, and attach the detail as context after tools and prompts
     * Input
        - event: HookEvent - lifecycle event
        - feedback: &Feedback - what Crane found
     * Output
        - Result<(), String>
    */
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

    /** Give the agent extra context (for example at session start)
     * Input
        - context: &str - guidance text
     * Output
        - Result<(), String>
    */
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

/** Turn a Gemini tool call into a neutral action by renaming Gemini's tools to their Copilot equivalents and reusing the Copilot translation, then restoring the real tool name
 * Input
    - payload: &Value - hook JSON with tool_name and tool_input
 * Output
    - AgentAction
*/
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
