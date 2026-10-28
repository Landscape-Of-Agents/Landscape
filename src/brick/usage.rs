// What one agent session has done in each Zone and Flow, for the caps of Limits Bricks and the
// pause of Recovery Bricks: tool calls, commands, network calls, denials, distinct files and lines
// changed, and failures in a row. Kept per session in the runtime directory (never signed, never
// uploaded), written by the hooks after each decision and outcome, and read before the next one.
// A new session starts from zero; a session that ended long ago is pruned on the next write.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::context::folder;
use crate::platform::files::write_atomic;

/** Usage files untouched for this long are removed, in milliseconds (7 days) */
const STALE_MS: u64 = 7 * 86_400_000;

/** What a session did in one target
 * Fields
    - calls: u64 - tool calls touching the target
    - commands: u64 - shell commands
    - network: u64 - calls reaching the network
    - denials: u64 - denied calls
    - files: BTreeSet<String> - distinct files changed
    - lines: u64 - lines changed
    - failures_in_row: u32 - failed tool calls since the last success
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct TargetUsage {
    pub(crate) calls: u64,
    pub(crate) commands: u64,
    pub(crate) network: u64,
    pub(crate) denials: u64,
    pub(crate) files: BTreeSet<String>,
    pub(crate) lines: u64,
    pub(crate) failures_in_row: u32,
}

/** What a session did, by target
 * Fields
    - session: String - Crane session id
    - targets: BTreeMap<String, TargetUsage> - usage by target key
    - pending: BTreeMap<String, Vec<String>> - targets of calls whose outcome is not known yet,
      by tool call id
    - updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Usage {
    pub(crate) session: String,
    pub(crate) targets: BTreeMap<String, TargetUsage>,
    pub(crate) pending: BTreeMap<String, Vec<String>>,
    pub(crate) updated_at: u64,
}

/** One decided call, as it counts
 * Fields
    - targets: BTreeSet<String> - targets it touched
    - command: bool - it ran a shell command
    - network: bool - it reached the network
    - denied: bool - it was denied
    - changes: Vec<(String, String, u64)> - (target, file, lines) it changed
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct Counted {
    pub(crate) targets: BTreeSet<String>,
    pub(crate) command: bool,
    pub(crate) network: bool,
    pub(crate) denied: bool,
    pub(crate) changes: Vec<(String, String, u64)>,
}

/** The usage file of a session
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
 * Output
    - PathBuf
*/
fn path(runtime: &Path, session: &str) -> PathBuf {
    let safe = session
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect::<String>();
    folder(runtime).join("usage").join(format!("{safe}.json"))
}

/** Read a session's usage (empty when none)
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
 * Output
    - Usage
*/
pub(crate) fn load(runtime: &Path, session: &str) -> Usage {
    fs::read_to_string(path(runtime, session))
        .ok()
        .and_then(|text| serde_json::from_str::<Usage>(&text).ok())
        .filter(|usage| usage.session == session)
        .unwrap_or_else(|| Usage {
            session: session.into(),
            ..Usage::default()
        })
}

/** Write a session's usage, and prune stale usage files of other sessions
 * Input
    - runtime: &Path - runtime directory
    - usage: &mut Usage - usage
 * Output
    - Result<(), String>
*/
fn save(runtime: &Path, usage: &mut Usage) -> Result<(), String> {
    let now = crate::platform::now_millis();
    usage.updated_at = now;
    let file = path(runtime, &usage.session);
    if let Some(directory) = file.parent() {
        fs::create_dir_all(directory).map_err(crate::platform::io_error)?;
        for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
            let old = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.elapsed().ok())
                .is_some_and(|age| age.as_millis() as u64 > STALE_MS);
            if old && entry.path() != file {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    write_atomic(
        &file,
        serde_json::to_string(usage)
            .map_err(crate::platform::io_error)?
            .as_bytes(),
    )
}

/** Count a decided call; its outcome follows with outcome()
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
    - call_id: &str - tool call id
    - counted: &Counted - what it counts
 * Output
    - Result<(), String>
*/
pub(crate) fn record(
    runtime: &Path,
    session: &str,
    call_id: &str,
    counted: &Counted,
) -> Result<(), String> {
    if counted.targets.is_empty() {
        return Ok(());
    }
    let mut usage = load(runtime, session);
    for target in &counted.targets {
        let entry = usage.targets.entry(target.clone()).or_default();
        entry.calls += 1;
        entry.commands += u64::from(counted.command);
        entry.network += u64::from(counted.network);
        entry.denials += u64::from(counted.denied);
    }
    if !counted.denied {
        for (target, file, lines) in &counted.changes {
            let entry = usage.targets.entry(target.clone()).or_default();
            entry.files.insert(file.clone());
            entry.lines += lines;
        }
        usage
            .pending
            .insert(call_id.into(), counted.targets.iter().cloned().collect());
        // Calls whose outcome never arrived (a crashed tool) are not kept forever
        while usage.pending.len() > 64 {
            let first = usage.pending.keys().next().cloned().unwrap_or_default();
            usage.pending.remove(&first);
        }
    }
    save(runtime, &mut usage)
}

/** Record the outcome of a call (failures in a row per target)
 * Input
    - runtime: &Path - runtime directory
    - session: &str - session id
    - call_id: &str - tool call id
    - failed: bool - the tool reported a failure
 * Output
    - Result<(), String>
*/
pub(crate) fn outcome(
    runtime: &Path,
    session: &str,
    call_id: &str,
    failed: bool,
) -> Result<(), String> {
    let mut usage = load(runtime, session);
    let Some(targets) = usage.pending.remove(call_id) else {
        return Ok(());
    };
    for target in targets {
        let entry = usage.targets.entry(target).or_default();
        entry.failures_in_row = if failed {
            entry.failures_in_row.saturating_add(1)
        } else {
            0
        };
    }
    save(runtime, &mut usage)
}
