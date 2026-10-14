// The Codex adapter: Codex hook payloads (including apply_patch edits) translated to neutral
// actions, its hook configuration, and its answers.

use super::*;
use crate::hooks::action::{AgentAction, FileChange, Operation, Proposed, TextEdit};
use crate::hooks::outcome::{block, blocked};
use crate::telemetry::model::Decision;
use serde_json::{json, Value};
use std::path::Path;

/** Adapter for Codex
 * Fields
    - None
*/
pub(crate) struct CodexAdapter;

impl AgentAdapter for CodexAdapter {
    /** Report the Codex profile
     * Input
        - None (uses self)
     * Output
        - AgentKind::Codex
    */
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    /** Protect the Codex files that register Crane's hooks
     * Input
        - None (uses self)
     * Output
        - &'static [&'static str]
    */
    fn protected_paths(&self) -> &'static [&'static str] {
        &[".codex/hooks.json", ".codex/config.toml"]
    }

    /** Translate a Codex hook payload
     * Input
        - event: HookEvent - lifecycle event
        - payload: &Value - Codex hook JSON
     * Output
        - ProviderEvent
    */
    fn translate(&self, event: HookEvent, payload: &Value) -> ProviderEvent {
        provider_event(payload, event.has_action().then(|| codex_action(payload)))
    }

    /** Describe Codex's project hook configuration
     * Input
        - None (uses self)
     * Output
        - Option<HookConfig>
    */
    fn hook_config(&self) -> Option<HookConfig> {
        Some(HookConfig {
            file: ".codex/hooks.json",
            events: &[
                (HookEvent::SessionStart, false),
                (HookEvent::UserPromptSubmit, false),
                (HookEvent::PreToolUse, true),
                (HookEvent::PermissionRequest, true),
                (HookEvent::PostToolUse, true),
                (HookEvent::Stop, false),
                (HookEvent::SessionEnd, false),
            ],
            matcher: "*",
            deny: &[],
            timeout: 120,
            format: HookConfigFormat::Grouped,
        })
    }

    /** Return the command Codex runs (Codex spells events as PreToolUse)
     * Input
        - event: HookEvent - lifecycle event
     * Output
        - String
    */
    fn hook_command(&self, event: HookEvent) -> String {
        format!(
            "crane agent hook --event {} --profile codex",
            event.host_name()
        )
    }

    /** Answer PreToolUse the Codex way: silence to allow, exit code 2 otherwise (Codex has no
     * "ask" in PreToolUse, and an approval must never become a silent allow)
     * Input
        - verdict: &Verdict - decision
     * Output
        - Result<(), String>
    */
    fn respond_action(&self, verdict: &Verdict) -> Result<(), String> {
        let reasons = verdict.reasons.join("; ");
        match verdict.decision {
            Decision::Allow => {
                if verdict.context.is_some() || verdict.system_message.is_some() {
                    let mut output = json!({});
                    if let Some(context) = &verdict.context {
                        output["hookSpecificOutput"] = json!({
                            "hookEventName": "PreToolUse",
                            "additionalContext": context,
                        });
                    }
                    if let Some(message) = &verdict.system_message {
                        output["systemMessage"] = json!(message);
                    }
                    println!("{output}");
                }
                Ok(())
            }
            Decision::RequireApproval => Err(block(format!(
                "Crane requires human approval for this tool call: {reasons}{}",
                verdict
                    .system_message
                    .as_deref()
                    .map(|message| format!("\n{message}"))
                    .unwrap_or_default()
            ))),
            _ => Err(block(format!(
                "Crane denied this tool call: {reasons}{}",
                verdict
                    .system_message
                    .as_deref()
                    .map(|message| format!("\n{message}"))
                    .unwrap_or_default()
            ))),
        }
    }

    /** Answer PermissionRequest the Codex way: deny with a decision object when Crane denies;
     * otherwise print nothing so the user decides (Crane never approves on the user's behalf)
     * Input
        - verdict: &Verdict - decision
     * Output
        - Result<(), String>
    */
    fn respond_permission(&self, verdict: &Verdict) -> Result<(), String> {
        if matches!(
            verdict.decision,
            Decision::Allow | Decision::RequireApproval
        ) {
            if verdict.context.is_some() || verdict.system_message.is_some() {
                let mut output = json!({});
                if let Some(context) = &verdict.context {
                    output["hookSpecificOutput"] = json!({
                        "hookEventName": "PermissionRequest",
                        "additionalContext": context,
                    });
                }
                if let Some(message) = &verdict.system_message {
                    output["systemMessage"] = json!(message);
                }
                println!("{output}");
            }
        } else {
            println!(
                "{}",
                json!({"hookSpecificOutput": {
                    "hookEventName": "PermissionRequest",
                    "decision": {"behavior": "deny", "message": format!("Crane denied this tool call: {}{}", verdict.reasons.join("; "), verdict.system_message.as_deref().map(|message| format!("\n{message}")).unwrap_or_default())},
                }})
            );
        }
        Ok(())
    }

    /** Answer after verification the Codex way: decision "block" with the reason when the agent
     * must repair, a systemMessage otherwise
     * Input
        - event: HookEvent - lifecycle event
        - feedback: &Feedback - what to tell the agent
     * Output
        - Result<(), String>
    */
    fn respond_feedback(&self, event: HookEvent, feedback: &Feedback) -> Result<(), String> {
        let output = if feedback.block {
            match event {
                HookEvent::PostToolUse => json!({
                    "decision": "block",
                    "reason": feedback.summary,
                    "hookSpecificOutput": {"hookEventName": "PostToolUse", "additionalContext": feedback.detail},
                }),
                _ => {
                    json!({"decision": "block", "reason": format!("{}\n{}", feedback.summary, feedback.detail)})
                }
            }
        } else if feedback.summary.is_empty() {
            return Ok(());
        } else {
            json!({"systemMessage": format!("Crane: {}", feedback.summary)})
        };
        println!("{output}");
        Ok(())
    }

    /** Answer an event without a decision the Codex way (fail closed)
     * Input
        - event: HookEvent - lifecycle event
        - error: &str - why
        - stop_hook_active: bool - the agent continues after a held stop
     * Output
        - Result<(), String>
    */
    fn respond_failure(
        &self,
        event: HookEvent,
        error: &str,
        stop_hook_active: bool,
    ) -> Result<(), String> {
        let error = blocked(error).unwrap_or(error);
        let output = match event {
            HookEvent::PreToolUse => {
                return Err(block(format!(
                    "Crane could not authorize this tool call, so it is denied: {error}"
                )))
            }
            HookEvent::PermissionRequest => json!({"hookSpecificOutput": {
                "hookEventName": "PermissionRequest",
                "decision": {"behavior": "deny", "message": format!("Crane could not authorize this tool call: {error}")},
            }}),
            HookEvent::PostToolUse => {
                json!({"decision": "block", "reason": format!("Crane could not verify this tool call: {error}")})
            }
            HookEvent::SessionStart => {
                return Err(format!("Crane could not start the session: {error}"))
            }
            HookEvent::Stop if !stop_hook_active => {
                json!({"decision": "block", "reason": format!("Crane could not reconcile the session: {error}")})
            }
            _ => json!({"systemMessage": format!("Crane: {error}")}),
        };
        println!("{output}");
        Ok(())
    }
}

/** Map a Codex tool call to a neutral action: apply_patch (directly or through the shell) to
 * write with the files its patch changes, shell tools to execute, read-only tools to read,
 * anything else to other
 * Input
    - payload: &Value - Codex tool event JSON
 * Output
    - AgentAction
*/
pub(super) fn codex_action(payload: &Value) -> AgentAction {
    let tool = payload
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let input = payload.get("tool_input").unwrap_or(&Value::Null);
    let joined = match input.get("command") {
        Some(Value::Array(parts)) => {
            let parts = parts.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            match parts.as_slice() {
                [shell, flag, script] if shell.ends_with("sh") && flag.starts_with('-') => {
                    Some(script.to_string())
                }
                _ => Some(parts.join(" ")),
            }
        }
        Some(Value::String(text)) => Some(text.clone()),
        _ => ["patch", "input", "cmd"]
            .iter()
            .find_map(|key| input.get(*key).and_then(Value::as_str).map(String::from)),
    };
    let command = joined.as_deref();
    let cwd = payload
        .get("cwd")
        .and_then(Value::as_str)
        .or_else(|| input.get("workdir").and_then(Value::as_str));
    let mut action = skeleton(tool, input);
    action.arguments = strings(input);
    let shell_patch = command
        .filter(|command| command.trim_start().starts_with("apply_patch"))
        .and_then(|command| {
            let start = command.find("*** Begin Patch")?;
            let end = command.find("*** End Patch")? + "*** End Patch".len();
            Some(command[start..end].to_string())
        });
    let shell = matches!(
        tool,
        "Bash" | "shell" | "local_shell" | "exec_command" | "container.exec" | "unified_exec"
    );
    match tool {
        "apply_patch" => {
            action.operation = Operation::Write;
            action.files = command
                .and_then(|patch| parse_patch(patch, cwd))
                .unwrap_or_default();
        }
        _ if shell && shell_patch.is_some() => {
            action.operation = Operation::Write;
            action.files = shell_patch
                .and_then(|patch| parse_patch(&patch, cwd))
                .unwrap_or_default();
        }
        _ if shell => {
            action.operation = Operation::Execute;
            action.command = command.map(String::from);
        }
        "update_plan" | "read_file" | "list_dir" | "grep_files" | "view_image" | "web_search" => {
            action.operation = Operation::Read;
            action
                .reads
                .extend(input.get("path").and_then(Value::as_str).map(String::from));
        }
        _ => {}
    }
    action
}

/** Parse a Codex apply_patch patch into the file changes it proposes
 * Input
    - patch: &str - text from "*** Begin Patch" to "*** End Patch"
    - cwd: Option<&str> - directory relative paths are resolved against
 * Output
    - Option<Vec<FileChange>>, None if the patch is malformed or changes nothing
*/
pub(crate) fn parse_patch(patch: &str, cwd: Option<&str>) -> Option<Vec<FileChange>> {
    let resolve = |path: &str| match cwd {
        Some(cwd) if Path::new(path).is_relative() => {
            Path::new(cwd).join(path).to_string_lossy().into_owned()
        }
        _ => path.to_string(),
    };
    let mut lines = patch
        .lines()
        .skip_while(|line| line.trim().is_empty())
        .peekable();
    if lines.next()?.trim() != "*** Begin Patch" {
        return None;
    }
    let mut changes = Vec::new();
    loop {
        let line = lines.next()?;
        if line.trim() == "*** End Patch" {
            break;
        }
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            let mut content = String::new();
            while let Some(added) = lines.peek().and_then(|next| next.strip_prefix('+')) {
                content.push_str(added);
                content.push('\n');
                lines.next();
            }
            changes.push(FileChange {
                path: resolve(path.trim()),
                proposed: Proposed::Content(content),
            });
        } else if let Some(path) = line.strip_prefix("*** Delete File: ") {
            changes.push(FileChange {
                path: resolve(path.trim()),
                proposed: Proposed::Delete,
            });
        } else if let Some(path) = line.strip_prefix("*** Update File: ") {
            let moved = lines
                .peek()
                .and_then(|next| next.strip_prefix("*** Move to: "))
                .map(|target| resolve(target.trim()));
            if moved.is_some() {
                lines.next();
            }
            let mut edits = Vec::new();
            let mut unknown = false;
            let (mut old, mut new) = (Vec::new(), Vec::new());
            let mut flush = |old: &mut Vec<&str>, new: &mut Vec<&str>| {
                if old.is_empty() && !new.is_empty() {
                    unknown = true;
                } else if !old.is_empty() {
                    edits.push(TextEdit {
                        old: old.join("\n"),
                        new: new.join("\n"),
                        all: false,
                    });
                }
                old.clear();
                new.clear();
            };
            while let Some(next) = lines.peek().copied() {
                if next.starts_with("*** ") && next.trim() != "*** End of File" {
                    break;
                }
                lines.next();
                if next.starts_with("@@") {
                    flush(&mut old, &mut new);
                } else if next.trim() == "*** End of File" {
                    continue;
                } else if let Some(removed) = next.strip_prefix('-') {
                    old.push(removed);
                } else if let Some(added) = next.strip_prefix('+') {
                    new.push(added);
                } else {
                    if !next.is_empty() && !next.starts_with(' ') {
                        return None;
                    }
                    let context = next.strip_prefix(' ').unwrap_or(next);
                    old.push(context);
                    new.push(context);
                }
            }
            flush(&mut old, &mut new);
            let proposed = if unknown || edits.is_empty() {
                Proposed::Unknown
            } else {
                Proposed::Edits(edits)
            };
            match moved {
                Some(target) => {
                    changes.push(FileChange {
                        path: resolve(path.trim()),
                        proposed: Proposed::Delete,
                    });
                    changes.push(FileChange {
                        path: target,
                        proposed: Proposed::Unknown,
                    });
                }
                None => changes.push(FileChange {
                    path: resolve(path.trim()),
                    proposed,
                }),
            }
        } else if !line.trim().is_empty() {
            return None;
        }
    }
    (!changes.is_empty()).then_some(changes)
}
