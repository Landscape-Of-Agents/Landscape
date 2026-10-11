// Command audit: every Crane command run in an initialized repository (read-only ones included)
// is recorded as a command.executed event with its outcome, duration, and whether it ran in an
// agent environment, and then handed to the upload sync. Help, version, the ingestion service, and
// the internal hook and sync entry points are not recorded (hooks record their own events). Credentials embedded in URL
// arguments are removed, and the event store redacts known secret patterns as well.

use serde_json::json;

use crate::governance::workspace::Workspace;
use crate::platform::now_millis;
use crate::telemetry::events::{Event, EventStore, Measurement, Source};
use crate::telemetry::model::{Domain, Execution, Severity};
use crate::telemetry::sync;
use crate::trust::agent_environment;

/** Remove user information (user:password@) from URL-like arguments
 * Input
    - word: &str - argument
 * Output
    - String
*/
fn without_credentials(word: &str) -> String {
    match (word.find("://"), word.rfind('@')) {
        (Some(scheme), Some(at)) if at > scheme => {
            format!("{}://[REDACTED]@{}", &word[..scheme], &word[at + 1..])
        }
        _ => word.to_string(),
    }
}

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
        .map(|word| without_credentials(word))
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
    });
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
    sync::trigger(&workspace);
}

#[cfg(test)]
mod tests {
    use super::without_credentials;

    /** Check that URL credentials are removed and other words are kept
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn removes_url_credentials() {
        assert_eq!(
            without_credentials("https://token@github.com/a/b"),
            "https://[REDACTED]@github.com/a/b"
        );
        assert_eq!(
            without_credentials("git@github.com:a/b.git"),
            "git@github.com:a/b.git"
        );
        assert_eq!(without_credentials("app/payments.py"), "app/payments.py");
    }
}
