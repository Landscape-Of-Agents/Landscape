// Installing, validating, and removing Crane's hooks in provider project configurations.
// Installation is additive and idempotent; removal touches only Crane's handlers and permission
// rules; validation fails when any required event is missing, duplicated, or narrowly matched.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::adapter::{adapter, AgentKind, HookConfig, HookConfigFormat};
use crate::platform::files::write_atomic;
use crate::platform::git;
use crate::trust::require_human;

/** Read a provider's hook configuration: path, JSON document ({} when missing), and whether it
 * existed; a file that is not a JSON object is an error and is never overwritten
 * Input
    - config: &HookConfig - provider configuration
 * Output
    - Result<(PathBuf, Value, bool), String>
*/
fn read_config(config: &HookConfig) -> Result<(PathBuf, Value, bool), String> {
    let root = git::toplevel()?;
    let path = root.join(config.file);
    let (document, exists) = match fs::read_to_string(&path) {
        Ok(content) => (
            serde_json::from_str::<Value>(&content).map_err(|error| {
                format!(
                    "{} is not valid JSON ({error}); fix it before changing Crane hooks",
                    path.display()
                )
            })?,
            true,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (json!({}), false),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    if !document.is_object() {
        return Err(format!("{} must contain a JSON object", path.display()));
    }
    if !document.get("hooks").is_none_or(Value::is_object) {
        return Err(format!("\"hooks\" in {} must be an object", path.display()));
    }
    Ok((path, document, exists))
}

/** Write a configuration file atomically, pretty-printed
 * Input
    - path: &Path - file
    - document: &Value - JSON
 * Output
    - Result<(), String>
*/
fn write_config(path: &Path, document: &Value) -> Result<(), String> {
    let text = serde_json::to_string_pretty(document).map_err(|error| error.to_string())? + "\n";
    write_atomic(path, text.as_bytes())
}

/** Return configuration that registers hooks inline (Codex config.toml next to hooks.json)
 * Input
    - path: &Path - hook configuration file
 * Output
    - String, empty when there is none
*/
fn inline_config(path: &Path) -> String {
    path.parent()
        .map(|folder| fs::read_to_string(folder.join("config.toml")).unwrap_or_default())
        .unwrap_or_default()
}

/** Count the handlers of an event's groups that run a command
 * Input
    - groups: &[Value] - matcher groups of one event
    - command: &str - command
 * Output
    - usize
*/
fn has_command(handler: &Value, command: &str, format: HookConfigFormat) -> bool {
    match format {
        HookConfigFormat::Grouped => handler["command"] == command,
        HookConfigFormat::Flat => handler["bash"] == command || handler["powershell"] == command,
    }
}

fn registrations(groups: &[Value], command: &str, format: HookConfigFormat) -> usize {
    match format {
        HookConfigFormat::Grouped => groups
            .iter()
            .flat_map(|group| group["hooks"].as_array().cloned().unwrap_or_default())
            .filter(|handler| has_command(handler, command, format))
            .count(),
        HookConfigFormat::Flat => groups
            .iter()
            .filter(|handler| has_command(handler, command, format))
            .count(),
    }
}

/** Install a provider's hooks additively and idempotently: every event Crane needs gets one
 * Crane handler group, Crane's permission rules are added when missing, and nothing else changes
 * Input
    - kind: AgentKind - provider
 * Output
    - Result<Vec<String>, String> events added (empty when already installed)
    - Error if run by an agent, the provider is unsupported, or the file is invalid
*/
pub(crate) fn install(kind: AgentKind) -> Result<Vec<String>, String> {
    require_human("crane agent install")?;
    let adapter = adapter(kind);
    let config = adapter
        .hook_config()
        .ok_or("this profile does not support hook installation")?;
    let (path, mut document, _) = read_config(&config)?;
    let mut changed = false;
    if config.format == HookConfigFormat::Flat {
        match document.get("version") {
            Some(version) if version == 1 => {}
            Some(_) => return Err(format!("{} must use hooks version 1", path.display())),
            None => {
                document["version"] = json!(1);
                changed = true;
            }
        }
    }
    let inline = inline_config(&path);
    let mut added = Vec::new();
    {
        let hooks = document
            .as_object_mut()
            .ok_or("invalid configuration")?
            .entry("hooks")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or("invalid configuration")?;
        for (event, with_matcher) in config.events {
            let name = adapter.hook_event_name(*event);
            let command = adapter.hook_command(*event);
            if inline.contains(&command) {
                continue;
            }
            let groups = hooks
                .entry(name)
                .or_insert_with(|| json!([]))
                .as_array_mut()
                .ok_or_else(|| format!("hooks.{name} in {} must be an array", path.display()))?;
            if registrations(groups, &command, config.format) > 0 {
                continue;
            }
            if config.format == HookConfigFormat::Flat {
                groups.push(json!({
                    "type": "command",
                    "bash": command,
                    "powershell": command,
                    "timeoutSec": config.timeout,
                }));
                added.push(name.to_string());
                continue;
            }
            let mut handler =
                json!({"type": "command", "command": command, "timeout": config.timeout});
            if kind == AgentKind::Codex {
                handler["statusMessage"] = json!(format!("Crane: {}", event.name()));
            }
            let mut group = json!({"hooks": [handler]});
            if *with_matcher {
                group["matcher"] = json!(config.matcher);
            }
            groups.push(group);
            added.push(name.to_string());
        }
    }
    let mut denied = 0;
    if !config.deny.is_empty() {
        let rules = document
            .as_object_mut()
            .ok_or("invalid configuration")?
            .entry("permissions")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| format!("\"permissions\" in {} must be an object", path.display()))?
            .entry("deny")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .ok_or_else(|| format!("permissions.deny in {} must be an array", path.display()))?;
        for rule in config.deny {
            if !rules.iter().any(|existing| existing == rule) {
                rules.push(json!(rule));
                denied += 1;
            }
        }
    }
    if changed || !added.is_empty() || denied > 0 {
        write_config(&path, &document)?;
    }
    Ok(added)
}

/** Find the Crane executable the hooks run: crane on PATH
 * Input
    - None
 * Output
    - Option<PathBuf>
*/
fn crane_on_path() -> Option<PathBuf> {
    let names: &[&str] = if cfg!(windows) {
        &["crane.exe", "crane"]
    } else {
        &["crane"]
    };
    env::split_paths(&env::var_os("PATH")?)
        .flat_map(|folder| names.iter().map(move |name| folder.join(name)))
        .find(|path| path.is_file())
}

/** Validate a provider's hook configuration
 * Input
    - kind: AgentKind - provider
 * Output
    - Result<Value, String> {profile, file, exists, events, problems, warnings, valid}
    - Error if the provider is unsupported
*/
pub(crate) fn validate(kind: AgentKind) -> Result<Value, String> {
    let adapter = adapter(kind);
    let config = adapter
        .hook_config()
        .ok_or("this profile does not support hook validation")?;
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let (path, document, exists) = match read_config(&config) {
        Ok(read) => read,
        Err(error) => {
            return Ok(
                json!({"profile": kind.name(), "file": config.file, "exists": true, "valid": false, "problems": [error], "warnings": [], "events": []}),
            )
        }
    };
    let inline = inline_config(&path);
    if !exists && inline.is_empty() {
        problems.push(format!(
            "{} does not exist; run 'crane agent install --profile {}'",
            config.file,
            kind.name()
        ));
    }
    if config.format == HookConfigFormat::Flat && document["version"] != 1 {
        problems.push(format!("{} must use hooks version 1", config.file));
    }
    let mut events = Vec::new();
    for (event, with_matcher) in config.events {
        let name = adapter.hook_event_name(*event);
        let command = adapter.hook_command(*event);
        let groups = document["hooks"][name]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let count = registrations(&groups, &command, config.format)
            + usize::from(inline.contains(&command));
        let matchers = if config.format == HookConfigFormat::Grouped {
            groups
                .iter()
                .filter(|group| {
                    registrations(std::slice::from_ref(*group), &command, config.format) > 0
                })
                .map(|group| group["matcher"].as_str().unwrap_or("").to_string())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        match count {
            0 => problems.push(format!("{name} is not registered, so Crane never sees it")),
            1 => {}
            many => problems.push(format!(
                "{name} runs Crane {many} times; remove the duplicates"
            )),
        }
        if *with_matcher && config.format == HookConfigFormat::Grouped {
            for matcher in &matchers {
                if !matches!(matcher.as_str(), "" | "*" | ".*") {
                    problems.push(format!("{name} is registered with matcher '{matcher}', so other tools run without Crane"));
                }
            }
        }
        events.push(
            json!({"event": name, "command": command, "registered": count, "matchers": matchers}),
        );
    }
    let missing = config
        .deny
        .iter()
        .filter(|rule| {
            !document["permissions"]["deny"]
                .as_array()
                .is_some_and(|rules| rules.iter().any(|existing| existing == **rule))
        })
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        problems.push(format!(
            "permission rules missing: {}",
            missing
                .iter()
                .map(std::string::ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let executable = crane_on_path();
    if executable.is_none() {
        problems.push("crane is not on PATH: the provider cannot run the hooks, and a hook that cannot run does not stop a tool call".into());
    }
    if crate::governance::workspace::Workspace::locate().is_err() {
        problems.push("Crane is not initialized in this repository; run 'crane init'".into());
    }
    if kind == AgentKind::Codex {
        warnings.push("Codex runs project hooks only after they are trusted; review them with /hooks in Codex".to_string());
    }
    Ok(json!({
        "profile": kind.name(),
        "file": config.file,
        "exists": exists,
        "events": events,
        "executable": executable.map(|path| path.to_string_lossy().into_owned()),
        "problems": problems,
        "warnings": warnings,
        "valid": problems.is_empty(),
    }))
}

/** Remove a provider's Crane hooks, refused for agents: only Crane's handlers and permission
 * rules are removed, emptied groups and events are dropped, and a file that held only Crane's
 * configuration is deleted
 * Input
    - kind: AgentKind - provider
 * Output
    - Result<usize, String> handlers and rules removed
    - Error if run by an agent or the file is invalid
*/
pub(crate) fn uninstall(kind: AgentKind) -> Result<usize, String> {
    require_human("crane agent uninstall")?;
    let adapter = adapter(kind);
    let config = adapter
        .hook_config()
        .ok_or("hooks cannot be removed for this profile")?;
    let (path, mut document, exists) = read_config(&config)?;
    if !exists {
        return Ok(0);
    }
    let commands = config
        .events
        .iter()
        .map(|(event, _)| adapter.hook_command(*event))
        .collect::<Vec<_>>();
    let mut removed = 0;
    if let Some(hooks) = document.get_mut("hooks").and_then(Value::as_object_mut) {
        for entries in hooks.values_mut() {
            if let Some(entries) = entries.as_array_mut() {
                match config.format {
                    HookConfigFormat::Flat => {
                        let before = entries.len();
                        entries.retain(|entry| {
                            !commands
                                .iter()
                                .any(|command| has_command(entry, command, config.format))
                        });
                        removed += before - entries.len();
                    }
                    HookConfigFormat::Grouped => {
                        for group in entries.iter_mut() {
                            if let Some(handlers) =
                                group.get_mut("hooks").and_then(Value::as_array_mut)
                            {
                                let before = handlers.len();
                                handlers.retain(|handler| {
                                    !commands
                                        .iter()
                                        .any(|command| has_command(handler, command, config.format))
                                });
                                removed += before - handlers.len();
                            }
                        }
                        entries.retain(|group| {
                            group["hooks"]
                                .as_array()
                                .is_none_or(|handlers| !handlers.is_empty())
                        });
                    }
                }
            }
        }
        hooks.retain(|_, entries| entries.as_array().is_none_or(|entries| !entries.is_empty()));
    }
    if let Some(deny) = document
        .pointer_mut("/permissions/deny")
        .and_then(Value::as_array_mut)
    {
        let before = deny.len();
        deny.retain(|rule| !config.deny.iter().any(|ours| rule == ours));
        removed += before - deny.len();
    }
    for (parent, key) in [("/permissions", "deny"), ("", "permissions"), ("", "hooks")] {
        let empty = document
            .pointer(&format!("{parent}/{key}"))
            .is_some_and(|value| {
                value.as_array().is_some_and(Vec::is_empty)
                    || value.as_object().is_some_and(serde_json::Map::is_empty)
            });
        if empty {
            let container = if parent.is_empty() {
                Some(&mut document)
            } else {
                document.pointer_mut(parent)
            };
            if let Some(object) = container.and_then(Value::as_object_mut) {
                object.remove(key);
            }
        }
    }
    if document.as_object().is_some_and(serde_json::Map::is_empty)
        || (config.format == HookConfigFormat::Flat
            && document.as_object().is_some_and(|object| {
                object.len() == 1 && object.get("version") == Some(&json!(1))
            }))
    {
        fs::remove_file(&path).map_err(|error| error.to_string())?;
    } else {
        write_config(&path, &document)?;
    }
    Ok(removed)
}

/** Report the hook installation of every provider for policy status: PASS for valid installed
 * hooks, FAIL for installed but invalid ones, WARN when no provider has hooks
 * Input
    - None
 * Output
    - Vec<(String, &'static str, String)> check, status, detail
*/
pub(crate) fn status_checks() -> Vec<(String, &'static str, String)> {
    let mut checks = Vec::new();
    for kind in [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Copilot,
        AgentKind::Gemini,
    ] {
        let Ok(report) = validate(kind) else { continue };
        let installed = report["exists"] == true
            || report["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event["registered"].as_u64() > Some(0))
            });
        if !installed {
            continue;
        }
        let problems = report["problems"].as_array().cloned().unwrap_or_default();
        checks.push((
            format!("hooks: {}", kind.display()),
            if problems.is_empty() { "PASS" } else { "FAIL" },
            if problems.is_empty() {
                "pre-tool-use authorization and post-tool-use verification are registered for every tool".into()
            } else {
                problems.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")
            },
        ));
    }
    if checks.is_empty() {
        checks.push((
            "hooks".into(),
            "WARN",
            "no agent hooks are installed; run 'crane agent install --profile claude|codex|copilot|gemini' so agent actions pass through Crane".into(),
        ));
    }
    checks
}
