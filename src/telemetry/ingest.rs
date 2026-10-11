// Zero-setup upload of the evidence to the Foxx ingestion service. A Crane release knows its
// service: the address is compiled in from CRANE_DEFAULT_INGEST_URL when the release is built, and
// CRANE_INGEST_URL overrides it at run time ("off" keeps everything local). The CLI never holds
// database credentials: it sends batches over HTTPS, and every request is signed with the
// repository's own Ed25519 key (created by crane init), whose public key identifies this
// installation. The service writes to MongoDB and only ever returns an installation's own records.
//
// A sync first asks the service how far each local stream already reached it (moving the local
// checkpoint back when the service holds less, for example after a restore), then sends everything
// newer in batches: events, ledger entries, governance changes, and alerts; the last batch also
// carries the current sessions, tasks, selection records, signed manifest, and metric rollups for
// the repository and for every session and tool call with new events. The outcome is kept in
// sync-state.json, which drives the periodic sync and the dashboard's health view.

use std::collections::BTreeSet;
use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::events::Event;
use super::identity::{SessionRecord, TaskRecord};
use super::ledger::Entry;
use super::{metrics, Stores};
use crate::governance::audit;
use crate::governance::state::{Manifest, State};
use crate::governance::workspace::Workspace;
use crate::platform::files::{read_lines, write_atomic, Lock};
use crate::platform::{io_error, now_millis};
use crate::selection::registry::Record;
use crate::trust::crypto::{sha512_hex, SigningIdentity};

/** Signature domain of ingestion requests */
pub(crate) const REQUEST_DOMAIN: &str = "foxx/ingest/request/v1";

/** Most records of one stream sent in one batch */
pub(crate) const BATCH: usize = 500;

/** Time limit of one request */
const TIMEOUT: Duration = Duration::from_secs(20);

/** Return the ingestion service address: CRANE_INGEST_URL, else the address compiled into this
 * release; None when uploads are off
 * Input
    - None
 * Output
    - Option<String> base URL without a trailing slash
*/
pub(crate) fn endpoint() -> Option<String> {
    let value = match std::env::var("CRANE_INGEST_URL") {
        Ok(value) => value,
        Err(_) => option_env!("CRANE_DEFAULT_INGEST_URL")?.to_string(),
    };
    let value = value.trim().trim_end_matches('/').to_string();
    match value.to_ascii_lowercase().as_str() {
        "" | "off" | "0" | "false" | "none" => None,
        _ => Some(value),
    }
}

/** Require HTTPS, except for a service on this machine
 * Input
    - url: &str - base URL
 * Output
    - Result<(), String>
*/
fn secure(url: &str) -> Result<(), String> {
    let local = ["http://127.0.0.1", "http://localhost", "http://[::1]"]
        .iter()
        .any(|prefix| {
            url.strip_prefix(prefix)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with([':', '/']))
        });
    if url.starts_with("https://") || local {
        Ok(())
    } else {
        Err(format!(
            "the ingestion service address must use https:// ({url})"
        ))
    }
}

/** Build the bytes a request signature covers: method, path from /v1/, timestamp, and the
 * SHA-512 of the body
 * Input
    - method: &str - GET or POST
    - path: &str - path starting at /v1/
    - timestamp: u64 - Unix milliseconds
    - body: &[u8] - request body
 * Output
    - Vec<u8>
*/
pub(crate) fn request_message(method: &str, path: &str, timestamp: u64, body: &[u8]) -> Vec<u8> {
    format!("{method}\n{path}\n{timestamp}\n{}", sha512_hex(body)).into_bytes()
}

/** How far each stream reached the service (also the local checkpoint)
 * Fields
    - events: u64 - last event sequence
    - ledger: u64 - last ledger sequence
    - audit: u64 - last governance audit sequence
    - alerts: u64 - number of alerts
*/
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Positions {
    pub(crate) events: u64,
    pub(crate) ledger: u64,
    pub(crate) audit: u64,
    pub(crate) alerts: u64,
}

/** A metric rollup of one scope
 * Fields
    - scope: String - repository, session, or tool_call
    - id: String - identifier within the scope
    - computed_at: u64 - Unix milliseconds
    - formula_version: u32 - metrics::FORMULA_VERSION
    - metrics: Value - computed metrics
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Rollup {
    pub(crate) scope: String,
    pub(crate) id: String,
    pub(crate) computed_at: u64,
    pub(crate) formula_version: u32,
    pub(crate) metrics: Value,
}

/** One upload; records are decoded into Crane's own types by the service, which validates them
 * Fields
    - complete: bool - the last batch of a sync, carrying the current state below
    - remote: Option<String> - repository remote
    - registry_generation: u64 - signed registry generation
    - events, ledger, audit, alerts - new records of each stream
    - sessions, tasks, selections, manifest - current state
    - rollups: Vec<Rollup> - refreshed metric rollups
*/
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Batch {
    pub(crate) complete: bool,
    pub(crate) remote: Option<String>,
    pub(crate) registry_generation: u64,
    pub(crate) events: Vec<Event>,
    pub(crate) ledger: Vec<Entry>,
    pub(crate) audit: Vec<audit::Entry>,
    pub(crate) alerts: Vec<Value>,
    pub(crate) sessions: Vec<SessionRecord>,
    pub(crate) tasks: Vec<TaskRecord>,
    pub(crate) selections: Vec<Record>,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) rollups: Vec<Rollup>,
}

/** The installation's data as the service holds it, for the dashboard
 * Fields
    - events: Vec<Event> - events in sequence order
    - ledger: Vec<Entry> - ledger entries in sequence order
    - sessions: Vec<SessionRecord> - sessions, most recent first
    - tasks: Vec<TaskRecord> - tasks, most recent first
    - alerts: Vec<Value> - alerts, newest first
*/
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Remote {
    pub(crate) events: Vec<Event>,
    pub(crate) ledger: Vec<Entry>,
    pub(crate) sessions: Vec<SessionRecord>,
    pub(crate) tasks: Vec<TaskRecord>,
    pub(crate) alerts: Vec<Value>,
}

/** The result of one sync
 * Fields
    - events, ledger, audit, alerts: usize - records sent per stream
    - rollups: usize - metric rollups refreshed
    - duplicates: usize - records the service already held (retries)
    - reset: bool - the service held less than the checkpoint, so missing records were re-sent
*/
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct Flushed {
    pub(crate) events: usize,
    pub(crate) ledger: usize,
    pub(crate) audit: usize,
    pub(crate) alerts: usize,
    pub(crate) rollups: usize,
    pub(crate) duplicates: usize,
    pub(crate) reset: bool,
}

/** The sync status kept in the trust directory
 * Fields
    - last_attempt_at: u64 - Unix milliseconds of the last attempt
    - last_success_at: Option<u64> - Unix milliseconds of the last successful sync
    - last_error: Option<String> - why the last attempt failed
    - remote: Positions - how far the service holds each stream
    - endpoint: Option<String> - service address
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SyncState {
    pub(crate) last_attempt_at: u64,
    pub(crate) last_success_at: Option<u64>,
    pub(crate) last_error: Option<String>,
    pub(crate) remote: Positions,
    pub(crate) endpoint: Option<String>,
}

/** Read the sync status
 * Input
    - stores: &Stores - runtime stores
 * Output
    - SyncState, default when never synced
*/
pub(crate) fn state(stores: &Stores) -> SyncState {
    fs::read_to_string(stores.directory.join("sync-state.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** Write the sync status
 * Input
    - stores: &Stores - runtime stores
    - state: &SyncState - status
 * Output
    - None
*/
fn save_state(stores: &Stores, state: &SyncState) {
    let _ = write_atomic(
        &stores.directory.join("sync-state.json"),
        serde_json::to_string_pretty(state)
            .unwrap_or_default()
            .as_bytes(),
    );
}

/** A signed connection to the ingestion service
 * Fields
    - base: String - service address
    - repository: String - repository identity
    - key: SigningIdentity - the repository's signing key, identifying this installation
    - agent: ureq::Agent - HTTP client
*/
pub(crate) struct Client {
    base: String,
    repository: String,
    key: SigningIdentity,
    agent: ureq::Agent,
}

impl Client {
    /** Connect when uploads are on and this machine holds the repository's signing key
     * Input
        - workspace: &Workspace - repository
     * Output
        - Result<Option<Client>, String>, None when uploads are off or there is no key yet
        - Error if the address is not HTTPS or the key is unreadable
    */
    pub(crate) fn open(workspace: &Workspace) -> Result<Option<Self>, String> {
        let Some(base) = endpoint() else {
            return Ok(None);
        };
        secure(&base)?;
        let Some(key) = workspace.trust()?.signing_key()? else {
            return Ok(None);
        };
        Ok(Some(Self {
            base,
            repository: workspace.repository_id.clone(),
            key,
            agent: ureq::AgentBuilder::new().timeout(TIMEOUT).build(),
        }))
    }

    /** Send one signed request about this repository
     * Input
        - method: &str - GET or POST
        - operation: &str - positions, batch, or snapshot
        - body: &[u8] - JSON body (empty for GET)
     * Output
        - Result<Value, String> the JSON answer
        - Error if the service is unreachable or refuses the request
    */
    fn call(&self, method: &str, operation: &str, body: &[u8]) -> Result<Value, String> {
        let path = format!("/v1/repositories/{}/{operation}", self.repository);
        let timestamp = now_millis();
        let signature = self.key.sign(
            REQUEST_DOMAIN,
            &request_message(method, &path, timestamp, body),
        );
        let request = self
            .agent
            .request(method, &format!("{}{path}", self.base))
            .set("X-Crane-Installation", &self.key.public_hex())
            .set("X-Crane-Timestamp", &timestamp.to_string())
            .set("X-Crane-Signature", &signature)
            .set("User-Agent", concat!("crane/", env!("CARGO_PKG_VERSION")));
        let response = if method == "POST" {
            request
                .set("Content-Type", "application/json")
                .send_bytes(body)
        } else {
            request.call()
        };
        match response {
            Ok(response) => response.into_json::<Value>().map_err(|error| {
                format!("the ingestion service sent an unreadable answer: {error}")
            }),
            Err(ureq::Error::Status(code, response)) => Err(format!(
                "the ingestion service answered HTTP {code}: {}",
                response
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>()
            )),
            Err(error) => Err(format!("the ingestion service is unreachable: {error}")),
        }
    }
}

/** Upload the local stores under the sync lock, recording the outcome
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
 * Output
    - Result<Flushed, String> what was sent (nothing when uploads are off)
    - Error if the upload failed (the local stores are unaffected)
*/
pub(crate) fn sync(workspace: &Workspace, stores: &Stores) -> Result<Flushed, String> {
    let _lock = Lock::acquire(&stores.directory.join("sync.lock"))?;
    let mut status = state(stores);
    status.last_attempt_at = now_millis();
    save_state(stores, &status);
    let result = Client::open(workspace).and_then(|client| match client {
        None => Ok(None),
        Some(client) => push(&client, workspace, stores, &mut status).map(Some),
    });
    match &result {
        Ok(Some(_)) => {
            status.last_success_at = Some(now_millis());
            status.last_error = None;
        }
        Ok(None) => status.last_error = Some("uploads are off".into()),
        Err(error) => status.last_error = Some(super::redact::redact(error).0),
    }
    save_state(stores, &status);
    result.map(Option::unwrap_or_default)
}

/** Read the installation's data back from the service
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Option<Remote>, String>, None when uploads are off
    - Error if the service cannot be read
*/
pub(crate) fn snapshot(workspace: &Workspace) -> Result<Option<Remote>, String> {
    let Some(client) = Client::open(workspace)? else {
        return Ok(None);
    };
    serde_json::from_value(client.call("GET", "snapshot", &[])?)
        .map(Some)
        .map_err(|error| format!("the ingestion service sent an unreadable snapshot: {error}"))
}

/** Compute the rollups of the repository and of the sessions and tool calls with new records
 * Input
    - repository: &str - repository identity
    - events: &[Event] - every event
    - ledger: &[Entry] - every ledger entry
    - records: &[SessionRecord] - every session
    - sessions: &BTreeSet<String> - sessions with new records
    - calls: &BTreeSet<String> - tool calls with new events
 * Output
    - Vec<Rollup>
*/
fn rollups(
    repository: &str,
    events: &[Event],
    ledger: &[Entry],
    records: &[SessionRecord],
    sessions: &BTreeSet<String>,
    calls: &BTreeSet<String>,
) -> Vec<Rollup> {
    let rollup = |scope: &str,
                  id: &str,
                  events: Vec<&Event>,
                  ledger: Vec<&Entry>,
                  records: Vec<&SessionRecord>| {
        let records = records
            .into_iter()
            .map(|record| serde_json::to_value(record).unwrap_or_default())
            .collect::<Vec<_>>();
        Rollup {
            scope: scope.into(),
            id: id.into(),
            computed_at: now_millis(),
            formula_version: metrics::FORMULA_VERSION,
            metrics: json!(metrics::compute(&events, &ledger, &records)),
        }
    };
    let mut rollups = vec![rollup(
        "repository",
        repository,
        events.iter().collect(),
        ledger.iter().collect(),
        records.iter().collect(),
    )];
    for session in sessions {
        rollups.push(rollup(
            "session",
            session,
            events
                .iter()
                .filter(|event| event.scope.foxx_session_id.as_ref() == Some(session))
                .collect(),
            ledger
                .iter()
                .filter(|entry| &entry.session == session)
                .collect(),
            records
                .iter()
                .filter(|record| &record.foxx_session_id == session)
                .collect(),
        ));
    }
    for call in calls {
        rollups.push(rollup(
            "tool_call",
            call,
            events
                .iter()
                .filter(|event| event.scope.tool_call_id.as_ref() == Some(call))
                .collect(),
            ledger
                .iter()
                .filter(|entry| entry.tool_call_id.as_ref() == Some(call))
                .collect(),
            Vec::new(),
        ));
    }
    rollups
}

/** Reconcile and upload: see the module comment
 * Input
    - client: &Client - service connection
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - status: &mut SyncState - sync status, updated with the remote positions
 * Output
    - Result<Flushed, String>
*/
fn push(
    client: &Client,
    workspace: &Workspace,
    stores: &Stores,
    status: &mut SyncState,
) -> Result<Flushed, String> {
    let path = stores.directory.join("sync-checkpoint.json");
    let mut checkpoint: Positions = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default();
    let remote: Positions = serde_json::from_value(client.call("GET", "positions", &[])?)
        .map_err(|error| format!("the ingestion service sent unreadable positions: {error}"))?;
    let mut flushed = Flushed::default();
    for (local, remote) in [
        (&mut checkpoint.events, remote.events),
        (&mut checkpoint.ledger, remote.ledger),
        (&mut checkpoint.audit, remote.audit),
        (&mut checkpoint.alerts, remote.alerts),
    ] {
        if remote < *local {
            *local = remote;
            flushed.reset = true;
        }
    }
    let all_events = stores.events.read_all()?;
    let ledger = stores.ledger.entries()?;
    let governance = State::load(workspace)?;
    let alerts = read_lines(&stores.directory.join("alerts.jsonl"))?;
    let events = all_events
        .iter()
        .filter(|event| event.sequence > checkpoint.events)
        .collect::<Vec<_>>();
    let entries = ledger
        .iter()
        .filter(|entry| entry.seq > checkpoint.ledger)
        .collect::<Vec<_>>();
    let changes = governance
        .audit
        .iter()
        .filter(|entry| entry.seq > checkpoint.audit)
        .collect::<Vec<_>>();
    let new_alerts = &alerts[(checkpoint.alerts as usize).min(alerts.len())..];
    let mut sessions = events
        .iter()
        .filter_map(|event| event.scope.foxx_session_id.clone())
        .collect::<BTreeSet<_>>();
    sessions.extend(entries.iter().map(|entry| entry.session.clone()));
    let calls = events
        .iter()
        .filter_map(|event| event.scope.tool_call_id.clone())
        .collect::<BTreeSet<_>>();
    let rounds = [events.len(), entries.len(), changes.len(), new_alerts.len()]
        .into_iter()
        .map(|length| length.div_ceil(BATCH))
        .max()
        .unwrap_or(0)
        .max(1);
    for round in 0..rounds {
        let range = |length: usize| (round * BATCH).min(length)..((round + 1) * BATCH).min(length);
        let mut batch = Batch {
            events: events[range(events.len())]
                .iter()
                .map(|event| (*event).clone())
                .collect(),
            ledger: entries[range(entries.len())]
                .iter()
                .map(|entry| (*entry).clone())
                .collect(),
            audit: changes[range(changes.len())]
                .iter()
                .map(|entry| (*entry).clone())
                .collect(),
            alerts: new_alerts[range(new_alerts.len())].to_vec(),
            ..Batch::default()
        };
        if round + 1 == rounds {
            let records = stores.identity.sessions();
            if !events.is_empty() || !entries.is_empty() || flushed.reset {
                batch.rollups = rollups(
                    &workspace.repository_id,
                    &all_events,
                    &ledger,
                    &records,
                    &sessions,
                    &calls,
                );
            }
            batch.complete = true;
            batch.remote = workspace.remote.clone();
            batch.registry_generation = governance.generation();
            batch.sessions = records;
            batch.tasks = stores.identity.tasks();
            batch.selections = governance.registry.selections.clone();
            batch.manifest = governance.registry.manifest.clone();
        }
        let body = serde_json::to_vec(&batch).map_err(io_error)?;
        let answer = client.call("POST", "batch", &body)?;
        flushed.duplicates += answer["duplicates"].as_u64().unwrap_or(0) as usize;
        flushed.events += batch.events.len();
        flushed.ledger += batch.ledger.len();
        flushed.audit += batch.audit.len();
        flushed.alerts += batch.alerts.len();
        flushed.rollups += batch.rollups.len();
        if let Some(last) = batch.events.last() {
            checkpoint.events = last.sequence;
        }
        if let Some(last) = batch.ledger.last() {
            checkpoint.ledger = last.seq;
        }
        if let Some(last) = batch.audit.last() {
            checkpoint.audit = last.seq;
        }
        checkpoint.alerts += batch.alerts.len() as u64;
        write_atomic(
            &path,
            serde_json::to_string(&checkpoint)
                .map_err(io_error)?
                .as_bytes(),
        )?;
    }
    status.remote = checkpoint;
    status.endpoint = Some(client.base.clone());
    Ok(flushed)
}

#[cfg(test)]
mod tests {
    use super::{request_message, secure};

    /** Check that only HTTPS or a service on this machine is accepted
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn requires_https_except_locally() {
        assert!(secure("https://ingest.example.com").is_ok());
        assert!(secure("http://127.0.0.1:8080").is_ok());
        assert!(secure("http://localhost").is_ok());
        assert!(secure("http://localhost.evil.com").is_err());
        assert!(secure("http://ingest.example.com").is_err());
    }

    /** Check that the signed message binds method, path, time, and body
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn request_message_binds_every_part() {
        let base = request_message("POST", "/v1/a", 1, b"x");
        assert_ne!(base, request_message("GET", "/v1/a", 1, b"x"));
        assert_ne!(base, request_message("POST", "/v1/b", 1, b"x"));
        assert_ne!(base, request_message("POST", "/v1/a", 2, b"x"));
        assert_ne!(base, request_message("POST", "/v1/a", 1, b"y"));
    }
}
