// Command audit: every Crane command run in an initialized repository (read-only ones included)
// is recorded as a command.executed event with its outcome, duration, and whether it ran in an
// agent environment, and then handed to the upload sync. Help, version, the ingestion service, and
// the internal hook and sync entry points are not recorded (hooks record their own events). Credentials embedded in URL
// arguments are removed, and the event store redacts known secret patterns as well.

use serde_json::{json, Value};

use super::args::Args;
use crate::governance::workspace::Workspace;
use crate::platform::now_millis;
use crate::telemetry::events::{Event, EventStore, Measurement, Source};
use crate::telemetry::model::{Domain, Execution, Severity};
use crate::telemetry::redact::strip_url_credentials;
use crate::telemetry::sync;
use crate::trust::agent_environment;

/** Record a finished command and trigger a sync
 * Input
    - words: &[String] - command-line arguments after "crane"
    - result: &Result<(), String> - outcome
    - started: u64 - Unix milliseconds when the command started
 * Output
    - None (recording failures never change the command's outcome)
*/
pub(crate) fn record(words: &[String], result: &Result<(), String>, started: u64) {
    let first = words.first().map(String::as_str).unwrap_or_default();
    let second = words.get(1).map(String::as_str).unwrap_or_default();
    if matches!(
        first,
        "" | "help" | "--help" | "-h" | "--version" | "-V" | "server"
    ) || (first == "agent" && matches!(second, "hook" | "sync"))
        || first == "foxx"
    {
        return;
    }
    let Ok(workspace) = Workspace::locate() else {
        return;
    };
    let Ok(trust) = workspace.trust() else {
        return;
    };
    let command = words
        .iter()
        .map(|word| strip_url_credentials(word))
        .collect::<Vec<_>>()
        .join(" ");
    let mut event = Event::new(
        "command.executed",
        Domain::AuditCompliance,
        &workspace.repository_id,
        &format!("command:{}:{started}", std::process::id()),
    );
    event.source = Source::Crane;
    event.execution = Some(if result.is_ok() {
        Execution::Success
    } else {
        Execution::Failure
    });
    event.severity = if result.is_ok() {
        Severity::Info
    } else {
        Severity::Low
    };
    event.occurred_at = started;
    if let Ok(state) = crate::governance::state::State::load(&workspace) {
        event.bindings.registry_generation = state.generation();
        event.bindings.registry_head = state.registry.manifest.map(|manifest| manifest.head);
        event.bindings.checkpoint = state.config.default_checkpoint;
    }
    event.payload = json!({
        "command": command.chars().take(300).collect::<String>(),
        "name": format!("{first} {second}").trim().to_string(),
        "agent_environment": agent_environment(),
        "user": workspace.user,
        "email": workspace.email,
    });
    if workspace.identity_cache != "HIT" {
        event.payload["identity_cache"] = json!(workspace.identity_cache);
    }
    if let Err(error) = result {
        event.reasons.push(error.chars().take(500).collect());
    }
    event.measurements.push(Measurement::observed(
        "command_duration_ms",
        now_millis().saturating_sub(started) as f64,
        "ms",
        "command wall clock",
    ));
    let _ = EventStore::open(&trust.runtime()).append(event);
    sync::trigger(&workspace, "COMMAND");
}

/** Names the Landscape extension may record (DASHBOARD.md F31) */
const FOXX_EVENTS: &[&str] = &[
    "FOXX_SETUP",
    "FOXX_PANEL_OPENED",
    "FOXX_PANEL_REFUSED",
    "FOXX_ACTION_RUN",
    "FOXX_GOVERNANCE_REVERTED",
    "FOXX_GOVERNANCE_UNDONE",
    "FOXX_VALIDATED_ON_SAVE",
];

/** crane foxx event NAME [key=value ...]: record an event of the Landscape editor extension (refused to
 * agents; only the F31 names are accepted, and values are short text)
 * Input
    - args: &[String] - NAME and key=value pairs
 * Output
    - Result<(), String>
*/
pub(crate) fn foxx_event(args: &[String]) -> Result<(), String> {
    crate::trust::require_human("crane foxx event")?;
    let (name, pairs) = args
        .split_first()
        .ok_or("usage: crane foxx event NAME [key=value ...]")?;
    if !FOXX_EVENTS.contains(&name.as_str()) {
        return Err(format!("unknown Landscape event '{name}'"));
    }
    let workspace = Workspace::locate()?;
    let trust = workspace.trust()?;
    let mut payload = json!({"name": name, "user": workspace.user, "email": workspace.email});
    for pair in pairs {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("'{pair}' is not key=value"))?;
        let key = key.trim();
        if key.is_empty()
            || !key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(format!("'{key}' is not a valid key"));
        }
        let value: String = value.chars().take(200).collect();
        payload[key] = match value.as_str() {
            "true" => json!(true),
            "false" => json!(false),
            _ => value
                .parse::<f64>()
                .ok()
                .filter(|_| {
                    key.ends_with("_lines")
                        || key.ends_with("_ms")
                        || key == "diagnostics"
                        || key == "selections_failing"
                })
                .map_or_else(|| json!(value), |number| json!(number)),
        };
    }
    let mut event = Event::new(
        "foxx.event",
        Domain::AuditCompliance,
        &workspace.repository_id,
        &format!("foxx:{name}:{}:{}", std::process::id(), now_millis()),
    );
    event.source = Source::FoxxExtension;
    event.payload = payload;
    EventStore::open(&trust.runtime()).append(event)?;
    Ok(())
}

/** Print where the live context is and its current content: crane foxx live [--json]
 * Input
    - args: &[String] - optional --json
 * Output
    - Result<(), String>
    - Error if the repository cannot be located
*/
pub(crate) fn foxx_live(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["json"])?;
    parsed.expect_positional(0, "crane foxx live [--json]")?;
    let path = crate::foxx::live::path(&Workspace::locate()?)?;
    let current = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    if parsed.flag("json") {
        println!("{}", json!({"path": path, "live": current}));
    } else {
        println!("{}", path.display());
        match current {
            Some(live) => println!(
                "{}",
                serde_json::to_string_pretty(&live).map_err(|error| error.to_string())?
            ),
            None => println!("No agent context yet."),
        }
    }
    Ok(())
}
