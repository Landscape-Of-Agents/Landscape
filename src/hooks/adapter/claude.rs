// The Claude Code adapter: Claude Code's hook payloads translated to neutral actions, its hook
// configuration, and its answers (permission decisions, blocking feedback, added context).

use super::*;
use crate::hooks::action::{AgentAction, FileChange, Operation, Proposed, TextEdit};
use crate::hooks::outcome::block;
use crate::telemetry::model::Decision;
use serde_json::{json, Value};

/** Adapter for Claude Code
 * Fields
    - None
*/
pub(crate) struct ClaudeAdapter;

impl AgentAdapter for ClaudeAdapter {
    /** Report the Claude profile
     * Input
        - None (uses self)
     * Output
        - AgentKind::Claude
    */
    fn kind(&self) -> AgentKind {
        AgentKind::Claude
    }

    /** Protect the Claude Code settings files that register Crane's hooks
     * Input
        - None (uses self)
     * Output
        - &'static [&'static str]
    */
    fn protected_paths(&self) -> &'static [&'static str] {
        &[".claude/settings.json", ".claude/settings.local.json"]
    }

    /** Translate a Claude Code hook payload
     * Input
        - event: HookEvent - lifecycle event
        - payload: &Value - Claude Code hook JSON
     * Output
        - ProviderEvent
    */
    fn translate(&self, event: HookEvent, payload: &Value) -> ProviderEvent {
        provider_event(payload, event.has_action().then(|| claude_action(payload)))
    }

    /** Describe Claude Code's project-local hook configuration
     * Input
        - None (uses self)
     * Output
        - Option<HookConfig>
    */
    fn hook_config(&self) -> Option<HookConfig> {
        Some(HookConfig {
            file: ".claude/settings.local.json",
            events: &[
                (HookEvent::SessionStart, false),
                (HookEvent::UserPromptSubmit, false),
                (HookEvent::PreToolUse, true),
                (HookEvent::PostToolUse, true),
                (HookEvent::Stop, false),
                (HookEvent::SessionEnd, false),
            ],
            deny: &[
                "Edit(./.crane/**)",
                "Write(./.crane/**)",
                "Edit(./.claude/settings.json)",
                "Edit(./.claude/settings.local.json)",
            ],
            timeout: 120,
        })
    }

    /** Answer PreToolUse the Claude Code way: silence to allow (or, when the call touches active
     * zones or flows, their guidance as additionalContext), exit code 2 with the reason to
     * deny (a denial is final; "ask" is never offered for a denied action), and permissionDecision
     * "ask" when approval is required
     * Input
        - verdict: &Verdict - decision
     * Output
        - Result<(), String>
    */
    fn respond_action(&self, verdict: &Verdict) -> Result<(), String> {
        let reasons = verdict.reasons.join("; ");
        match verdict.decision {
            Decision::Allow => {
                // Allowed calls stay silent unless there is zone or flow guidance to give
                if let Some(context) = &verdict.context {
                    println!(
                        "{}",
                        json!({"hookSpecificOutput": {"hookEventName": "PreToolUse", "additionalContext": context}})
                    );
                }
                Ok(())
            }
            Decision::RequireApproval => {
                let mut output = json!({"hookSpecificOutput": {
                    "hookEventName": "PreToolUse",
                    "permissionDecision": "ask",
                    "permissionDecisionReason": format!("Crane requires approval: {reasons}"),
                }});
                if let Some(context) = &verdict.context {
                    output["hookSpecificOutput"]["additionalContext"] = json!(context);
                }
                println!("{output}");
                Ok(())
            }
            _ => Err(block(format!("Crane denied this tool call: {reasons}"))),
        }
    }

    /** Answer after verification the Claude Code way: exit code 2 with the detail to block, a
     * systemMessage (and additionalContext where supported) otherwise
     * Input
        - event: HookEvent - lifecycle event
        - feedback: &Feedback - what to tell the agent
     * Output
        - Result<(), String>
    */
    fn respond_feedback(&self, event: HookEvent, feedback: &Feedback) -> Result<(), String> {
        if feedback.block {
            return Err(block(format!("{}\n{}", feedback.summary, feedback.detail)));
        }
        if feedback.summary.is_empty() {
            return Ok(());
        }
        let mut output = json!({"systemMessage": format!("Crane: {}", feedback.summary)});
        if matches!(event, HookEvent::PostToolUse | HookEvent::UserPromptSubmit)
            && !feedback.detail.is_empty()
        {
            output["hookSpecificOutput"] =
                json!({"hookEventName": event.host_name(), "additionalContext": feedback.detail});
        }
        println!("{output}");
        Ok(())
    }

    /** Supply session context as SessionStart additionalContext
     * Input
        - context: &str - context text
     * Output
        - Result<(), String>
    */
    fn respond_context(&self, context: &str) -> Result<(), String> {
        println!(
            "{}",
            json!({"hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": context}})
        );
        Ok(())
    }
}

/** Map a Claude Code tool call to a neutral action: read tools to read (keeping the path or URL
 * read), Write, Edit, MultiEdit, and NotebookEdit to write with the proposed content or edits (a
 * write without a path becomes other), Bash and PowerShell to execute, anything else to other
 * Input
    - payload: &Value - Claude Code tool event JSON
 * Output
    - AgentAction
*/
pub(super) fn claude_action(payload: &Value) -> AgentAction {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input = payload.get("tool_input").unwrap_or(&Value::Null);
    let text = |value: &Value, key: &str| value.get(key).and_then(Value::as_str).map(String::from);
    let edit = |value: &Value| {
        Some(TextEdit {
            old: text(value, "old_string")?,
            new: text(value, "new_string")?,
            all: value
                .get("replace_all")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    };
    let path = ["file_path", "notebook_path", "path"]
        .iter()
        .find_map(|key| text(input, key));
    let mut action = skeleton(tool, input);
    action.arguments = strings(input);
    match (tool, path) {
        ("Read" | "NotebookRead" | "Glob" | "Grep" | "LS", path) => {
            action.operation = Operation::Read;
            action.reads.extend(path);
            action
                .reads
                .extend(text(input, "pattern").filter(|_| tool == "Glob"));
        }
        ("WebFetch" | "WebSearch", _) => {
            action.operation = Operation::Read;
            action
                .reads
                .extend(text(input, "url").or_else(|| text(input, "query")));
        }
        ("TodoWrite" | "Task" | "Agent" | "ExitPlanMode", _) => action.operation = Operation::Read,
        ("Write" | "Edit" | "MultiEdit" | "NotebookEdit", Some(path)) => {
            let proposed = match tool {
                "Write" => text(input, "content").map_or(Proposed::Unknown, Proposed::Content),
                "Edit" => edit(input).map_or(Proposed::Unknown, |edit| Proposed::Edits(vec![edit])),
                "MultiEdit" => input
                    .get("edits")
                    .and_then(Value::as_array)
                    .and_then(|edits| edits.iter().map(edit).collect::<Option<Vec<_>>>())
                    .map_or(Proposed::Unknown, Proposed::Edits),
                _ => Proposed::Unknown,
            };
            action.operation = Operation::Write;
            action.files.push(FileChange { path, proposed });
        }
        ("Bash" | "PowerShell", _) => {
            action.operation = Operation::Execute;
            action.command = text(input, "command");
        }
        _ => {}
    }
    action
}
