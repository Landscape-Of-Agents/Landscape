// Zero-setup upload of the evidence to the Foxx ingestion service. Every build knows its service:
// DEFAULT_INGEST_URL, or CRANE_DEFAULT_INGEST_URL when it is set at build time, and CRANE_INGEST_URL
// overrides it at run time ("off" keeps everything local). The CLI never holds database
// credentials: it sends batches over HTTPS, and every request is signed with this machine's
// Ed25519 upload key (created automatically on first use, separate from repository signing keys),
// whose public key identifies the installation. Every teammate's machine therefore uploads, not
// only the one that ran crane init. The service writes to MongoDB; a signed policy request may
// read active quarantine rules, but no machine route returns telemetry or incident evidence.
// People read evidence on the hosted dashboard.
//
// A sync first asks the service how far each local stream already reached it (moving the local
// checkpoint back when the service holds less, for example after a restore), then sends everything
// newer in batches: events, ledger entries, governance changes, and alerts; the last batch also
// carries the current sessions, tasks, selection records, the signed manifest, and how the sync
// went (the service records it as DASHBOARD.md F09 events). The outcome is kept in
// sync-state.json, which drives the periodic sync. The last batch also carries this machine's
// repository status (foxx/status.rs) when it is due.

use std::collections::BTreeSet;
use std::fs;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::events::Event;
use super::identity::{SessionRecord, TaskRecord};
use super::ledger::Entry;
use super::Stores;
use crate::governance::audit;
use crate::governance::state::{Manifest, State};
use crate::governance::workspace::Workspace;
use crate::platform::files::{read_lines, write_atomic, Lock};
use crate::platform::{io_error, now_millis};
use crate::selection::registry::Record;
use crate::trust::crypto::{hex, random_bytes, sha512_hex, SigningIdentity};

/** Signature domain of ingestion requests */
pub(crate) const REQUEST_DOMAIN: &str = "foxx/ingest/request/v1";

/** Header naming the encoding of a request body ("base64"; absent for plain JSON) */
pub(crate) const BODY_ENCODING: &str = "X-Crane-Body-Encoding";

/** Most records of one stream sent in one batch */
pub(crate) const BATCH: usize = 500;

/** The Foxx ingestion service every build uploads to unless told otherwise */
pub(crate) const DEFAULT_INGEST_URL: &str = "https://agentscript-service.onrender.com";

/** Signature domain of an owner key registration */
pub(crate) const OWNER_DOMAIN: &str = "foxx/owner/v1";

/** Digest domain of approval codes (the service stores only their digests) */
pub(crate) const CODE_DOMAIN: &str = "foxx/approval-code/v1";

/** How long an approval code can be used, in milliseconds */
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the service
pub(crate) const CODE_LIFETIME_MILLIS: u64 = 10 * 60 * 1000;

/** A request the dashboard must approve after a GitHub check: linking this machine's upload key
 * to a GitHub user, or registering the repository owner's key
 * Fields
    - code: String - one-time code the person enters (or opens) on the dashboard
    - remote: Option<String> - repository remote
    - root_commits: Vec<String> - root commits (prove the remote, as in a batch)
    - owner_key: Option<String> - the owner's public key (owner registration only)
    - signature: Option<String> - the owner key's signature over owner_message (owner registration)
*/
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ApprovalRequest {
    pub(crate) code: String,
    pub(crate) remote: Option<String>,
    pub(crate) root_commits: Vec<String>,
    pub(crate) owner_key: Option<String>,
    pub(crate) signature: Option<String>,
}

/** The message an owner key signs to register itself, binding the repository, the key, the
 * uploading machine, and the one-time code
 * Input
    - repository: &str - repository identity
    - owner_key: &str - owner public key
    - installation: &str - uploading machine's key
    - code_digest: &str - digest of the code
 * Output
    - Vec<u8>
*/
pub(crate) fn owner_message(
    repository: &str,
    owner_key: &str,
    installation: &str,
    code_digest: &str,
) -> Vec<u8> {
    format!("{repository}\n{owner_key}\n{installation}\n{code_digest}").into_bytes()
}

/** Generate a one-time approval code: 12 characters from the selection identifier alphabet
 * Input
    - None
 * Output
    - Result<String, String>
*/
pub(crate) fn approval_code() -> Result<String, String> {
    let first = crate::trust::crypto::random_id()?;
    let second = crate::trust::crypto::random_id()?;
    Ok(format!("{first}{}", &second[..4]))
}

/** Report whether text is an approval code
 * Input
    - code: &str - text
 * Output
    - bool
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the service
pub(crate) fn is_approval_code(code: &str) -> bool {
    code.len() == 12
        && code
            .bytes()
            .all(|byte| crate::trust::crypto::ID_ALPHABET.contains(&byte))
}

/** How often the repository status is sent, in milliseconds */
const STATUS_EVERY: u64 = 120_000;

/** Time limit of one request */
const TIMEOUT: Duration = Duration::from_secs(20);

/** Return the ingestion service address: CRANE_INGEST_URL, else CRANE_DEFAULT_INGEST_URL from the
 * build, else DEFAULT_INGEST_URL; None when uploads are off
 * Input
    - None
 * Output
    - Option<String> base URL without a trailing slash
*/
pub(crate) fn endpoint() -> Option<String> {
    let value = match std::env::var("CRANE_INGEST_URL") {
        Ok(value) => value,
        Err(_) => option_env!("CRANE_DEFAULT_INGEST_URL")
            .filter(|url| !url.trim().is_empty())
            .unwrap_or(DEFAULT_INGEST_URL)
            .to_string(),
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

/** One upload; records are decoded into Crane's own types by the service, which validates them
 * Fields
    - complete: bool - the last batch of a sync, carrying the current state below
    - remote: Option<String> - repository remote
    - root_commits: Vec<String> - root commits, so the service can check that the repository
      identity really belongs to the remote (the dashboard relies on it)
    - actor: Option<String> - this machine's Git user name, shown on the dashboard
    - registry_generation: u64 - signed registry generation
    - events, ledger, audit, alerts - new records of each stream
    - sessions, tasks - sessions with new records and their tasks (all of them after a reset)
    - selections, manifest - current state
    - status: Option<Value> - this machine's repository status, when due
    - sync: Option<Value> - on the last batch: how this upload went (trigger, started_at, resent,
      previous_error, events), so the service records it (DASHBOARD.md F09) without the machine
      creating events about its own uploads
*/
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Batch {
    pub(crate) complete: bool,
    pub(crate) remote: Option<String>,
    pub(crate) root_commits: Vec<String>,
    pub(crate) actor: Option<String>,
    pub(crate) registry_generation: u64,
    pub(crate) events: Vec<Event>,
    pub(crate) ledger: Vec<Entry>,
    pub(crate) audit: Vec<audit::Entry>,
    pub(crate) alerts: Vec<Value>,
    pub(crate) sessions: Vec<SessionRecord>,
    pub(crate) tasks: Vec<TaskRecord>,
    pub(crate) selections: Vec<Record>,
    pub(crate) manifest: Option<Manifest>,
    pub(crate) status: Option<Value>,
    pub(crate) sync: Option<Value>,
}

/** The result of one sync
 * Fields
    - events, ledger, audit, alerts: usize - records sent per stream
    - duplicates: usize - records the service already held (retries)
    - reset: bool - the service held less than the checkpoint, so missing records were re-sent
    - skipped: bool - another sync was running, so this background sync did nothing
*/
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub(crate) struct Flushed {
    pub(crate) events: usize,
    pub(crate) ledger: usize,
    pub(crate) audit: usize,
    pub(crate) alerts: usize,
    pub(crate) duplicates: usize,
    pub(crate) reset: bool,
    pub(crate) skipped: bool,
}

/** The sync status kept in the trust directory
 * Fields
    - last_attempt_at: u64 - Unix milliseconds of the last attempt
    - last_success_at: Option<u64> - Unix milliseconds of the last successful sync
    - last_error: Option<String> - why the last attempt failed
    - remote: Positions - how far the service holds each stream
    - endpoint: Option<String> - service address
    - status_at: u64 - Unix milliseconds of the last repository status sent
    - status_generation: u64 - registry generation of the last status sent
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct SyncState {
    pub(crate) last_attempt_at: u64,
    pub(crate) last_success_at: Option<u64>,
    pub(crate) last_error: Option<String>,
    pub(crate) remote: Positions,
    pub(crate) endpoint: Option<String>,
    pub(crate) status_at: u64,
    pub(crate) status_generation: u64,
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
    - key: SigningIdentity - this machine's upload key, identifying the installation
    - agent: ureq::Agent - HTTP client
*/
pub(crate) struct Client {
    base: String,
    repository: String,
    key: SigningIdentity,
    agent: ureq::Agent,
    policy_agent: ureq::Agent,
}

impl Client {
    /** Connect when uploads are on, creating this machine's upload key on first use
     * Input
        - workspace: &Workspace - repository
     * Output
        - Result<Option<Client>, String>, None when uploads are off
        - Error if the address is not HTTPS or the upload key cannot be loaded or created
    */
    pub(crate) fn open(workspace: &Workspace) -> Result<Option<Self>, String> {
        let Some(base) = endpoint() else {
            return Ok(None);
        };
        secure(&base)?;
        let key = crate::trust::installation_key()?;
        Ok(Some(Self {
            base,
            repository: workspace.repository_id.clone(),
            key,
            agent: ureq::AgentBuilder::new().timeout(TIMEOUT).build(),
            policy_agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(5))
                .build(),
        }))
    }

    /** Return the service's address
     * Input
        - None (uses self)
     * Output
        - &str
    */
    pub(crate) fn base(&self) -> &str {
        &self.base
    }

    /** Return this machine's upload key
     * Input
        - None (uses self)
     * Output
        - String public key in hex
    */
    pub(crate) fn installation(&self) -> String {
        self.key.public_hex()
    }

    /** Ask the service to hold an approval request (link or owner) until the code is used
     * Input
        - operation: &str - link or owner
        - request: &ApprovalRequest - the request
     * Output
        - Result<Value, String>
    */
    pub(crate) fn request_approval(
        &self,
        operation: &str,
        request: &ApprovalRequest,
    ) -> Result<Value, String> {
        self.call(
            "POST",
            operation,
            &serde_json::to_vec(request).map_err(io_error)?,
        )
    }

    /** Ask the service for the repository's GitHub-confirmed owner key
     * Input
        - None (uses self)
     * Output
        - Result<Value, String> {owner_key, confirmed_by, confirmed_at, remote}, owner_key null
          when none is confirmed
    */
    pub(crate) fn confirmed_owner(&self) -> Result<Value, String> {
        self.call("GET", "owner", &[])
    }

    /** Fetch the Brick change requests people made on the dashboard for this repository, still
     * waiting for a machine (cli/brick/inbox.rs applies them when a person says so)
     * Input
        - None (uses self)
     * Output
        - Result<Value, String> {requests: [{request_id, op, params, reason, role, requested_by,
          requested_at}]}
    */
    pub(crate) fn brick_requests(&self) -> Result<Value, String> {
        self.call("GET", "brick-requests", &[])
    }

    /** Tell the service what happened to Brick change requests
     * Input
        - results: &[Value] - [{request_id, status: APPLIED|FAILED|DISMISSED, detail}]
     * Output
        - Result<Value, String> {settled}
    */
    pub(crate) fn settle_brick_requests(&self, results: &[Value]) -> Result<Value, String> {
        let body = serde_json::to_vec(&json!({"results": results})).map_err(io_error)?;
        self.call("POST", "brick-requests", &body)
    }

    /** Fetch repository-scoped Dashboard Agent enforcement rules before a tool call. */
    pub(crate) fn dashboard_agent_policy(&self) -> Result<Value, String> {
        self.call("GET", "agent-policy", &[])
    }

    /** Request deletion of this repository's hosted records. The service checks the GitHub token
     * against the default-branch CODEOWNERS file and never stores the token.
     */
    pub(crate) fn clear_repository(
        &self,
        remote: &str,
        github_token: &str,
    ) -> Result<Value, String> {
        let body = serde_json::to_vec(&json!({
            "remote": remote,
            "github_token": github_token,
        }))
        .map_err(io_error)?;
        self.call("POST", "clear", &body)
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
        // Correlation identifier: the service logs it and returns it, so a failure reported here
        // can be found in the service's logs
        let request_id = random_bytes(8).map(|bytes| hex(&bytes)).unwrap_or_default();
        let timestamp = now_millis();
        // POST bodies travel base64-encoded: the hosting edge firewall reads raw shell commands in
        // telemetry (pipes, quotes, '$?') as injection and blocks the request before the service
        // sees it. The signature covers the bytes as sent
        let encoded = (method == "POST").then(|| crate::platform::http::base64(body));
        let body = encoded.as_deref().map_or(body, str::as_bytes);
        let signature = self.key.sign(
            REQUEST_DOMAIN,
            &request_message(method, &path, timestamp, body),
        );
        let agent = if method == "GET" && operation == "agent-policy" {
            &self.policy_agent
        } else {
            &self.agent
        };
        let request = agent
            .request(method, &format!("{}{path}", self.base))
            .set("X-Crane-Installation", &self.key.public_hex())
            .set("X-Crane-Timestamp", &timestamp.to_string())
            .set("X-Crane-Signature", &signature)
            .set("X-Request-Id", &request_id)
            .set("User-Agent", concat!("crane/", env!("CARGO_PKG_VERSION")));
        let response = if method == "POST" {
            request
                .set("Content-Type", "text/plain")
                .set(BODY_ENCODING, "base64")
                .send_bytes(body)
        } else {
            request.call()
        };
        match response {
            Ok(response) => response.into_json::<Value>().map_err(|error| {
                format!("the ingestion service sent an unreadable answer: {error} (request {request_id})")
            }),
            Err(ureq::Error::Status(code, response)) => Err(format!(
                "the ingestion service answered HTTP {code}: {} (request {request_id})",
                response
                    .into_string()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect::<String>()
            )),
            Err(error) => Err(format!(
                "the ingestion service is unreachable: {error} (request {request_id})"
            )),
        }
    }
}

/** Most upload passes one sync makes while records keep arriving */
const MAX_PASSES: usize = 5;

/** What started a sync
 * Variants
    - Background - a hook or command; while another sync runs it leaves a pending flag instead,
      and that sync makes one more pass, so the records that started it are never left behind;
      the repository status is sent only when due
    - Manual - a person ran 'crane agent sync'; waits for a running sync and always refreshes the
      repository status
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Trigger {
    Background,
    Manual,
}

/** Upload the local stores under the sync lock, recording the outcome
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - trigger: Trigger - what started the sync
 * Output
    - Result<Flushed, String> what was sent (nothing when uploads are off)
    - Error if the upload failed (the local stores are unaffected)
*/
pub(crate) fn sync(
    workspace: &Workspace,
    stores: &Stores,
    trigger: Trigger,
) -> Result<Flushed, String> {
    let manual = trigger == Trigger::Manual;
    let first = passes(
        &stores.directory.join("sync.lock"),
        &stores.directory.join("sync.pending"),
        manual,
        |pass| upload(workspace, stores, manual && pass == 0),
    )?;
    first.unwrap_or_else(|| {
        Ok(Flushed {
            skipped: true,
            ..Flushed::default()
        })
    })
}

/** Run upload passes under the sync lock until no other sync asked for one more. A background
 * sync that finds the lock taken writes the pending flag (and tries the lock once more, in case
 * the holder finished before it could see the flag); the holder checks the flag after each pass,
 * once it has released the lock, so a request is served either by another pass or by its writer
 * Input
    - lock_path: &Path - the sync lock
    - pending: &Path - the pending flag
    - wait: bool - wait for the lock (a person asked) instead of leaving the flag
    - pass: impl FnMut(usize) -> R - one pass, given its number
 * Output
    - Result<Option<R>, String> the first pass's result, None when another sync will run instead
*/
fn passes<R>(
    lock_path: &std::path::Path,
    pending: &std::path::Path,
    wait: bool,
    mut pass: impl FnMut(usize) -> R,
) -> Result<Option<R>, String> {
    let mut first = None;
    for number in 0..MAX_PASSES {
        let lock = match (wait, number) {
            (true, 0) => Some(Lock::acquire(lock_path)?),
            (false, 0) => match Lock::try_acquire(lock_path)? {
                Some(lock) => Some(lock),
                None => {
                    let _ = fs::write(pending, now_millis().to_string());
                    Lock::try_acquire(lock_path)?
                }
            },
            _ => Lock::try_acquire(lock_path)?,
        };
        // Another sync holds the lock: it sees the pending flag after its pass
        let Some(lock) = lock else {
            break;
        };
        let _ = fs::remove_file(pending);
        let result = pass(number);
        drop(lock);
        if first.is_none() {
            first = Some(result);
        }
        if !pending.exists() {
            break;
        }
    }
    Ok(first)
}

/** One upload pass, under the sync lock, recording the outcome
 * Input
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - refresh: bool - always send the repository status (a person asked)
 * Output
    - Result<Flushed, String> what was sent (nothing when uploads are off)
*/
fn upload(workspace: &Workspace, stores: &Stores, refresh: bool) -> Result<Flushed, String> {
    // Alert rules first, so their notifications are uploaded by this sync (telemetry/rules.rs)
    if let Err(error) = super::rules::evaluate(workspace, stores, "SYNC") {
        crate::platform::notes::note("ALERT_RULES_FAILED", json!({"error": error}));
    }
    // Brick monitors: deterministic checks while assigned agents are idle (brick/runtime.rs)
    crate::brick::runtime::sync_monitors(workspace, stores);
    // Resident agents: one tick (triggers, timers, waits); idle assignments make no model call
    crate::resident::worker::sync(workspace, stores);
    let mut status = state(stores);
    let previous_attempt = status.last_attempt_at;
    status.last_attempt_at = now_millis();
    save_state(stores, &status);
    let result = Client::open(workspace).and_then(|client| match client {
        None => Ok(None),
        Some(client) => push(
            &client,
            workspace,
            stores,
            &mut status,
            refresh,
            previous_attempt,
        )
        .map(Some),
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

/** Apply the credit grants an admin approved on the dashboard for this machine: each grant is a
 * Grant entry in the session's ledger, idempotent by its grant id (a retried sync never credits
 * twice), and rejected when the session is not on this machine; the service is told the outcome
 * Input
    - client: &Client - service connection
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
 * Output
    - Result<usize, String> grants settled
*/
fn apply_grants(client: &Client, workspace: &Workspace, stores: &Stores) -> Result<usize, String> {
    let answer = client.call("GET", "grants", &[])?;
    let grants = answer["grants"].as_array().cloned().unwrap_or_default();
    if grants.is_empty() {
        return Ok(0);
    }
    let sessions = stores.identity.sessions();
    let generation = State::load(workspace)
        .map(|state| state.generation())
        .unwrap_or(0);
    let mut results = Vec::new();
    for grant in &grants {
        let id = grant["grant_id"].as_str().unwrap_or_default();
        let session = grant["session"].as_str().unwrap_or_default();
        let amount = grant["amount"].as_u64().unwrap_or(0).min(1000);
        if id.is_empty() || amount == 0 {
            continue;
        }
        if !sessions
            .iter()
            .any(|record| record.foxx_session_id == session)
        {
            results.push(json!({"grant_id": id, "status": "REJECTED", "detail": "the session is not on this machine"}));
            continue;
        }
        let reason = format!(
            "granted on the dashboard by @{}: {}",
            grant["approved_by"].as_str().unwrap_or("an admin"),
            grant["reason"].as_str().unwrap_or_default()
        );
        let key = format!("grant:{id}");
        match stores.ledger.apply(super::ledger::Operation {
            session,
            task: None,
            tool_call_id: None,
            kind: super::ledger::Kind::Grant,
            amount,
            reason: &reason,
            registry_generation: generation,
            idempotency_key: &key,
        }) {
            Ok((entry, _)) => results.push(json!({"grant_id": id, "status": "APPLIED", "entry_id": entry.entry_id, "detail": format!("{} credits available now", entry.after.available)})),
            Err(error) => results.push(json!({"grant_id": id, "status": "REJECTED", "detail": error})),
        }
    }
    let body = serde_json::to_vec(&json!({"results": results})).map_err(io_error)?;
    client.call("POST", "grants", &body)?;
    Ok(results.len())
}

/** Apply the resident-agent decisions an admin made on the dashboard for this machine (approve,
 * deny, answer, cancel, pause, revoke; resident/remote.rs applies them with the CLI's checks, each
 * at most once), then tell the service the outcome
 * Input
    - client: &Client - service connection
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
 * Output
    - Result<usize, String> decisions settled
*/
fn apply_resident_decisions(
    client: &Client,
    workspace: &Workspace,
    stores: &Stores,
) -> Result<usize, String> {
    let answer = client.call("GET", "resident-decisions", &[])?;
    let decisions = answer["decisions"].as_array().cloned().unwrap_or_default();
    if decisions.is_empty() {
        return Ok(0);
    }
    // Busy with a command or a tick: the decisions stay pending at the service until the next sync
    let Some(mut store) =
        crate::resident::store::ResidentStore::try_open_exclusive(&stores.directory)?
    else {
        return Ok(0);
    };
    let mut env = crate::resident::worker::live(workspace, &stores.directory);
    let results = crate::resident::remote::apply(&mut store, &mut env, &decisions);
    let body = serde_json::to_vec(&json!({"results": results})).map_err(io_error)?;
    client.call("POST", "resident-decisions", &body)?;
    Ok(results.len())
}

/** Reconcile and upload: see the module comment
 * Input
    - client: &Client - service connection
    - workspace: &Workspace - repository
    - stores: &Stores - runtime stores
    - status: &mut SyncState - sync status, updated with the remote positions
    - refresh: bool - send the repository status even when it is not due
    - previous_attempt: u64 - when the sync before this one started (Unix milliseconds, 0 if never)
 * Output
    - Result<Flushed, String>
*/
fn push(
    client: &Client,
    workspace: &Workspace,
    stores: &Stores,
    status: &mut SyncState,
    refresh: bool,
    previous_attempt: u64,
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
    // Credits an admin granted on the dashboard: applied before the ledger is read, so the new
    // entries are uploaded by this sync (a failure only delays them to the next one)
    if let Err(error) = apply_grants(client, workspace, stores) {
        crate::platform::notes::note("CREDIT_GRANTS_FAILED", json!({"error": error}));
    }
    // Resident-agent decisions made on the dashboard, applied before the events are read so their
    // notes are uploaded by this sync
    if let Err(error) = apply_resident_decisions(client, workspace, stores) {
        crate::platform::notes::note("RESIDENT_DECISIONS_FAILED", json!({"error": error}));
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
    let rounds = [events.len(), entries.len(), changes.len(), new_alerts.len()]
        .into_iter()
        .map(|length| length.div_ceil(BATCH))
        .max()
        .unwrap_or(0)
        .max(1);
    let mut sent_status = false;
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
            let touched = |session: &str| flushed.reset || sessions.contains(session);
            batch.complete = true;
            batch.sync = Some(json!({
                "trigger": if refresh { "MANUAL" } else { "BACKGROUND" },
                "started_at": status.last_attempt_at,
                "resent": flushed.reset,
                "previous_error": status.last_error,
                "events": flushed.events + events.len(),
                // What started a background sync (F34): POST_TOOL, STOP, SESSION_END, COMMAND, INTERVAL
                "trigger_reason": if refresh { None } else { std::env::var("CRANE_SYNC_TRIGGER").ok() },
                "seconds_since_last_attempt": (previous_attempt > 0).then(|| status.last_attempt_at.saturating_sub(previous_attempt) / 1000),
                "interval_seconds": super::sync::interval_seconds(),
            }));
            batch.remote = workspace.remote.clone();
            batch.root_commits = workspace.root_commits.clone();
            batch.actor = Some(crate::platform::actor());
            batch.registry_generation = governance.generation();
            batch.sessions = records
                .into_iter()
                .filter(|record| touched(&record.foxx_session_id))
                .collect();
            batch.tasks = stores
                .identity
                .tasks()
                .into_iter()
                .filter(|task| touched(&task.foxx_session_id))
                .collect();
            batch.selections = governance.registry.selections.clone();
            batch.manifest = governance.registry.manifest.clone();
            if refresh
                || now_millis().saturating_sub(status.status_at) >= STATUS_EVERY
                || status.status_generation != governance.generation()
            {
                batch.status = Some(crate::foxx::status::collect(workspace));
            }
        }
        sent_status |= batch.status.is_some();
        let body = serde_json::to_vec(&batch).map_err(io_error)?;
        let answer = client.call("POST", "batch", &body)?;
        flushed.duplicates += answer["duplicates"].as_u64().unwrap_or(0) as usize;
        flushed.events += batch.events.len();
        flushed.ledger += batch.ledger.len();
        flushed.audit += batch.audit.len();
        flushed.alerts += batch.alerts.len();
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
    if sent_status {
        status.status_at = now_millis();
        status.status_generation = governance.generation();
    }
    status.endpoint = Some(client.base.clone());
    Ok(flushed)
}

#[cfg(test)]
mod tests {
    /** A sync never leaves records behind: one that finds another running asks it for one more
     * pass, a request made during a pass gets another pass, the passes are bounded, and a free
     * lock means a single pass
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn a_busy_sync_hands_its_records_to_the_running_one() {
        let directory = tempfile::tempdir().unwrap();
        let lock = directory.path().join("sync.lock");
        let pending = directory.path().join("sync.pending");
        // Nothing else runs: one pass
        let mut count = 0;
        assert_eq!(
            passes(&lock, &pending, false, |_| count += 1).unwrap(),
            Some(())
        );
        assert_eq!(count, 1);
        assert!(!lock.exists() && !pending.exists());
        // Another sync holds the lock: this one leaves the flag and runs nothing
        std::fs::write(&lock, "held").unwrap();
        let mut count = 0;
        assert_eq!(
            passes(&lock, &pending, false, |_| count += 1).unwrap(),
            None
        );
        assert_eq!(count, 0);
        assert!(
            pending.exists(),
            "the running sync is asked for one more pass"
        );
        // The running sync then serves that request with a second pass
        std::fs::remove_file(&lock).unwrap();
        let mut numbers = Vec::new();
        let first = passes(&lock, &pending, false, |number| {
            numbers.push(number);
            if number == 0 {
                // A trigger arrives while this pass runs
                std::fs::write(&pending, "1").unwrap();
            }
            number
        })
        .unwrap();
        assert_eq!(numbers, [0, 1]);
        assert_eq!(first, Some(0), "the first pass's result is returned");
        assert!(!pending.exists());
        // Triggers that never stop are served by a bounded number of passes
        let mut count = 0;
        passes(&lock, &pending, false, |_| {
            count += 1;
            std::fs::write(&pending, "1").unwrap();
        })
        .unwrap();
        assert_eq!(count, MAX_PASSES);
        // A person waits for the lock and always runs
        let mut count = 0;
        passes(&lock, &pending, true, |_| count += 1).unwrap();
        assert_eq!(count, 1);
    }

    use super::{passes, request_message, secure, MAX_PASSES};

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
