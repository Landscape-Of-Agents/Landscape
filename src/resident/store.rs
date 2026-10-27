// The resident store (runtime/resident/store.json in the trust directory): definition versions,
// assignments, RCA graph versions, activations, the trigger deduplication set, ignored triggers,
// source cursors, timer slots, test reports, and idle-cost counters. Definitions and graphs are
// checked against their digests and assignments and activations are sealed, so a store edited
// outside Crane is refused when it is read. Writes are atomic. Every writer (a command that
// changes it, the background sync's tick, applying dashboard decisions) opens it exclusively: one
// lock (resident/worker.lock) is held from the read to the last save, so two processes never
// overwrite each other's changes. Read-only commands read without it.
//
// This store is the authoritative transactional record of resident work. DataSea receives events
// about it (F38) and the status report; it never holds the durable state.

use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::authority::Authorization;
use super::context::Cache;
use super::model::{Assignment, DefinitionRecord};
use super::plan::Plan;
use super::rca::{Investigation, RcaRecord};
use super::trigger::Trigger;
use crate::brick::model::Author;
use crate::platform::files::{write_atomic, Lock};
use crate::trust::crypto::digest_json;

/** How long a trigger's idempotency key is remembered (14 days), in milliseconds */
pub(crate) const DEDUP_RETENTION_MS: u64 = 14 * super::model::DAY_MS;

/** Most ignored triggers kept for inspection */
const MAX_IGNORED: usize = 300;

/** Most test reports kept */
const MAX_TESTS: usize = 50;

/** How long a worker's lease on an activation lasts (5 minutes), in milliseconds */
pub(crate) const LEASE_MS: u64 = 300_000;

/** Whether an activation acts or only plans
 * Variants
    - Live - steps run (each decided by Crane first)
    - DryRun - investigation, plan, and Crane decisions only; nothing runs
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    Live,
    DryRun,
}

/** Lifecycle of an activation
 * Variants
    - Queued - created, waiting for the worker
    - ContextLoading - compiling the context pack
    - Investigating - traversing the RCA graph
    - PlanReady - the plan is validated
    - AwaitingApproval - a step waits for a person's approval
    - AwaitingInput - a step waits for a person's answer
    - WaitingForEvent - a step waits for a matching trigger
    - Executing - running steps
    - Verifying - classifying the outcome from evidence
    - Completed - every step succeeded and verification evidence proves the outcome
    - PartiallyVerified - some verifications passed, others did not or some steps did not run
    - Failed - a verification was contradicted, or a step failed
    - Blocked - Crane denied it, or required context was missing
    - Cancelled - a person cancelled it, or its assignment was revoked or expired
    - ReconciliationRequired - an external effect is uncertain; a person establishes it
    - Unknown - it ended without evidence that proves an outcome (no verification, a proposal)
    - Simulated - a dry run ended (plan and decisions only)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum ActivationState {
    Queued,
    ContextLoading,
    Investigating,
    PlanReady,
    AwaitingApproval,
    AwaitingInput,
    WaitingForEvent,
    Executing,
    Verifying,
    Completed,
    PartiallyVerified,
    Failed,
    Blocked,
    Cancelled,
    ReconciliationRequired,
    Unknown,
    Simulated,
}

impl ActivationState {
    /** Report whether the activation has ended
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn ended(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::PartiallyVerified
                | Self::Failed
                | Self::Blocked
                | Self::Cancelled
                | Self::Unknown
                | Self::Simulated
        )
    }

    /** Report whether the activation waits for a person or an event (no worker runs it)
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn waiting(self) -> bool {
        matches!(
            self,
            Self::AwaitingApproval
                | Self::AwaitingInput
                | Self::WaitingForEvent
                | Self::ReconciliationRequired
        )
    }
}

/** State of one step
 * Variants
    - Pending - not started
    - AwaitingApproval - waits for an approval bound to its digest
    - AwaitingInput - waits for an answer
    - Waiting - waits for a matching trigger
    - Dispatching - its external call was started (persisted before the call)
    - Succeeded - done, with evidence
    - Failed - failed (the detail says whether any effect happened)
    - Denied - Crane denied it
    - Unavailable - no adapter or connector can run it (never a success)
    - Skipped - a dependency did not succeed
    - Uncertain - its effect is unknown; a person must reconcile it
    - Simulated - a dry run decided it without running it
    - Cancelled - the activation ended first
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum StepState {
    Pending,
    AwaitingApproval,
    AwaitingInput,
    Waiting,
    Dispatching,
    Succeeded,
    Failed,
    Denied,
    Unavailable,
    Skipped,
    Uncertain,
    Simulated,
    Cancelled,
}

impl StepState {
    /** Report whether the step has finished
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn finished(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::Denied
                | Self::Unavailable
                | Self::Skipped
                | Self::Simulated
                | Self::Cancelled
        )
    }
}

/** One step of an activation
 * Fields
    - state: StepState - state
    - attempts: u32 - dispatches started
    - detail: String - why it is in this state
    - authorization: Option<Authorization> - the last Crane decision
    - evidence: Vec<String> - evidence it produced
    - external_ref: Option<String> - reference in the external system
    - idempotency_key: String - key of its external call
    - answer: Option<String> - a person's answer (redacted)
    - deadline: Option<u64> - when a wait ends (Unix milliseconds)
    - updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepRecord {
    pub(crate) state: StepState,
    pub(crate) attempts: u32,
    pub(crate) detail: String,
    pub(crate) authorization: Option<Authorization>,
    pub(crate) evidence: Vec<String>,
    pub(crate) external_ref: Option<String>,
    pub(crate) idempotency_key: String,
    pub(crate) answer: Option<String>,
    pub(crate) deadline: Option<u64>,
    pub(crate) updated_at: u64,
}

impl StepRecord {
    /** A new pending step
     * Input
        - key: String - idempotency key of its external call
        - at: u64 - Unix milliseconds
     * Output
        - StepRecord
    */
    pub(crate) fn pending(key: String, at: u64) -> Self {
        Self {
            state: StepState::Pending,
            attempts: 0,
            detail: "not started".into(),
            authorization: None,
            evidence: Vec::new(),
            external_ref: None,
            idempotency_key: key,
            answer: None,
            deadline: None,
            updated_at: at,
        }
    }
}

/** A person's decision on a step
 * Fields
    - step: String - step
    - digest: String - the step approval digest it is bound to
    - approved: bool - approved or denied
    - approver: Author - who (identity of the machine's signer)
    - reason: String - why
    - at: u64 - Unix milliseconds
    - expires_at: u64 - when it can no longer be used
    - used: bool - consumed by the step it approved
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Approval {
    pub(crate) step: String,
    pub(crate) digest: String,
    pub(crate) approved: bool,
    pub(crate) approver: Author,
    pub(crate) reason: String,
    pub(crate) at: u64,
    pub(crate) expires_at: u64,
    pub(crate) used: bool,
}

/** The context an activation received
 * Fields
    - pack: String - context pack key
    - cache: Cache - hit, miss, or invalidated
    - dependencies: super::context::Dependencies - versions it was built from
    - sections: Vec<(String, String, String, usize)> - kind, source, reference, bytes of each section
    - missing: Vec<(String, String)> - required items that could not be loaded
    - previous: Vec<String> - prior activations of the assignment given as outcomes
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextRecord {
    pub(crate) pack: String,
    pub(crate) cache: Cache,
    pub(crate) dependencies: super::context::Dependencies,
    pub(crate) sections: Vec<(String, String, String, usize)>,
    pub(crate) missing: Vec<(String, String)>,
    pub(crate) previous: Vec<String>,
}

/** Usage of one activation
 * Fields
    - model_calls: u64 - model invocations actually made
    - model: Option<String> - provider and model, when one was called
    - tokens: Option<u64> - tokens reported by the provider (None: unmeasured)
    - deterministic_checks: u64 - checks evaluated without a model
    - external_calls: u64 - external dispatches started
    - duration_ms: u64 - worker time spent
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Usage {
    pub(crate) model_calls: u64,
    pub(crate) model: Option<String>,
    pub(crate) tokens: Option<u64>,
    pub(crate) deterministic_checks: u64,
    pub(crate) external_calls: u64,
    pub(crate) duration_ms: u64,
}

/** One timeline entry
 * Fields
    - at: u64 - Unix milliseconds
    - state: ActivationState - state after the entry
    - code: String - what happened
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub(crate) at: u64,
    pub(crate) state: ActivationState,
    pub(crate) code: String,
    pub(crate) detail: String,
}

/** A worker's lease on an activation
 * Fields
    - holder: String - worker
    - expires_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Lease {
    pub(crate) holder: String,
    pub(crate) expires_at: u64,
}

/** The outcome of an ended activation
 * Fields
    - status: String - PROVEN, CONTRADICTED, PARTIALLY_VERIFIED, UNKNOWN, BLOCKED, FAILED, CANCELLED
    - reason: String - why, from evidence
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Outcome {
    pub(crate) status: String,
    pub(crate) reason: String,
}

/** One activation
 * Fields
    - activation_id: String - "act_..." from assignment and trigger
    - assignment_id: String - assignment
    - definition_id, definition_version, definition_digest - exact definition version
    - mode: Mode - live or dry run
    - trigger: Trigger - the trigger that started it
    - triggers: Vec<String> - later triggers it received (waits)
    - state: ActivationState - state
    - lease: Option<Lease> - the worker's lease
    - context: Option<ContextRecord> - context received
    - investigation: Option<Investigation> - investigation
    - plan: Option<Plan> - plan (its digest binds approvals)
    - steps: BTreeMap<String, StepRecord> - per step
    - approvals: Vec<Approval> - decisions of people
    - outcome: Option<Outcome> - outcome once ended
    - usage: Usage - usage
    - timeline: Vec<Entry> - what happened, in order
    - created_at, updated_at: u64 - Unix milliseconds
    - record_digest: String - integrity digest of the record without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Activation {
    pub(crate) activation_id: String,
    pub(crate) assignment_id: String,
    pub(crate) definition_id: String,
    pub(crate) definition_version: u32,
    pub(crate) definition_digest: String,
    pub(crate) mode: Mode,
    pub(crate) trigger: Trigger,
    pub(crate) triggers: Vec<String>,
    pub(crate) state: ActivationState,
    pub(crate) lease: Option<Lease>,
    pub(crate) context: Option<ContextRecord>,
    pub(crate) investigation: Option<Investigation>,
    pub(crate) plan: Option<Plan>,
    pub(crate) steps: BTreeMap<String, StepRecord>,
    pub(crate) approvals: Vec<Approval>,
    pub(crate) outcome: Option<Outcome>,
    pub(crate) usage: Usage,
    pub(crate) timeline: Vec<Entry>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) record_digest: String,
}

impl Activation {
    /** The digest of the record without its digest
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn compute_digest(&self) -> String {
        let mut content = self.clone();
        content.record_digest = String::new();
        digest_json("resident/activation/v1", &json!(content))[..32].to_string()
    }

    /** Seal the record
     * Input
        - None (uses self)
     * Output
        - None
    */
    pub(crate) fn seal(&mut self) {
        self.record_digest = self.compute_digest();
    }

    /** Check that the record matches its digest
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verify_digest(&self) -> bool {
        self.compute_digest() == self.record_digest
    }

    /** Move to a state and record why
     * Input
        - state: ActivationState - new state
        - code: &str - what happened
        - detail: impl Into<String> - explanation
        - at: u64 - Unix milliseconds
     * Output
        - None
    */
    pub(crate) fn record(
        &mut self,
        state: ActivationState,
        code: &str,
        detail: impl Into<String>,
        at: u64,
    ) {
        self.state = state;
        self.updated_at = at;
        self.timeline.push(Entry {
            at,
            state,
            code: code.into(),
            detail: detail.into(),
        });
    }

    /** The digest an approval of a step is bound to: the activation, its plan digest, the step's
     * content, and the signed state the decision was made under
     * Input
        - step_id: &str - step
        - authority_digest: &str - the signed state
     * Output
        - String
    */
    pub(crate) fn approval_digest(&self, step_id: &str, authority_digest: &str) -> String {
        let plan = self.plan.as_ref();
        digest_json(
            "resident/approval/v1",
            &json!([
                self.activation_id,
                plan.map(|plan| plan.digest.clone()),
                plan.and_then(|plan| plan.step(step_id)),
                authority_digest
            ]),
        )[..24]
            .to_string()
    }
}

/** A trigger that woke nothing, with the reason
 * Fields
    - trigger_id: String - trigger
    - source: String - source
    - kind: String - kind
    - assignment: Option<String> - the assignment it was considered for
    - reason: String - DUPLICATE, NO_MATCH, COOLDOWN, RATE_LIMITED, PAUSED, INVALID, ...
    - detail: String - explanation
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Ignored {
    pub(crate) trigger_id: String,
    pub(crate) source: String,
    pub(crate) kind: String,
    pub(crate) assignment: Option<String>,
    pub(crate) reason: String,
    pub(crate) detail: String,
    pub(crate) at: u64,
}

/** Where the readers of each source stopped
 * Fields
    - event_sequence: u64 - last event store sequence read
    - generation: u64 - signed generation last seen
    - monitor_run: u64 - last Brick monitor run read
    - ci_run: u64 - newest GitHub Actions run id read
    - ci_polled_at: u64 - when GitHub Actions was last read (Unix milliseconds)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Cursors {
    pub(crate) event_sequence: u64,
    pub(crate) generation: u64,
    pub(crate) monitor_run: u64,
    #[serde(default)]
    pub(crate) ci_run: u64,
    #[serde(default)]
    pub(crate) ci_polled_at: u64,
}

/** Idle cost and health counters
 * Fields
    - ticks: u64 - ticks run
    - idle_ticks: u64 - ticks that started and advanced nothing
    - idle_model_calls: u64 - model calls during idle ticks (should stay 0)
    - model_calls: u64 - model calls during activations
    - triggers_received: u64 - triggers normalized
    - triggers_rejected: u64 - triggers refused as malformed, stale, or foreign
    - duplicates: u64 - repeated deliveries dropped
    - health_checks: u64 - deterministic checks (connectors, assignments) run without a model
    - last_tick_at: u64 - Unix milliseconds
    - last_tick_ms: u64 - duration of the last tick
    - lag_ms: u64 - the longest time from a trigger's occurrence to its receipt in the last tick
    - cache_hits, cache_misses, cache_invalidations: u64 - context pack cache outcomes
    - connector_errors: u64 - reads of a trigger source that failed (GitHub Actions)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Counters {
    pub(crate) ticks: u64,
    pub(crate) idle_ticks: u64,
    pub(crate) idle_model_calls: u64,
    pub(crate) model_calls: u64,
    pub(crate) triggers_received: u64,
    pub(crate) triggers_rejected: u64,
    pub(crate) duplicates: u64,
    pub(crate) health_checks: u64,
    pub(crate) last_tick_at: u64,
    pub(crate) last_tick_ms: u64,
    pub(crate) lag_ms: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
    pub(crate) cache_invalidations: u64,
    #[serde(default)]
    pub(crate) connector_errors: u64,
}

/** What the store holds
 * Fields
    - definitions: BTreeMap<String, BTreeMap<u32, DefinitionRecord>> - by id, then version
    - assignments: BTreeMap<String, Assignment> - by id
    - rcas: BTreeMap<String, BTreeMap<u32, RcaRecord>> - by id, then version
    - activations: BTreeMap<String, Activation> - by id
    - seen: BTreeMap<String, u64> - trigger ids already received, with when
    - ignored: VecDeque<Ignored> - recent triggers that woke nothing
    - cursors: Cursors - source cursors
    - timers: BTreeMap<String, u64> - "assignment:timer" to the last slot fired
    - tests: VecDeque<Value> - recent test reports
    - counters: Counters - idle cost and health
    - remote: BTreeMap<String, u64> - dashboard decisions already applied, with when
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoreState {
    pub(crate) definitions: BTreeMap<String, BTreeMap<u32, DefinitionRecord>>,
    pub(crate) assignments: BTreeMap<String, Assignment>,
    pub(crate) rcas: BTreeMap<String, BTreeMap<u32, RcaRecord>>,
    pub(crate) activations: BTreeMap<String, Activation>,
    pub(crate) seen: BTreeMap<String, u64>,
    pub(crate) ignored: VecDeque<Ignored>,
    pub(crate) cursors: Cursors,
    pub(crate) timers: BTreeMap<String, u64>,
    pub(crate) tests: VecDeque<Value>,
    pub(crate) counters: Counters,
    #[serde(default)]
    pub(crate) remote: BTreeMap<String, u64>,
}

/** The resident store
 * Fields
    - directory: Option<PathBuf> - runtime/resident, None in memory
    - state: StoreState - contents
    - held: Option<Lock> - the writers' lock, held from the read until the store is dropped
*/
pub(crate) struct ResidentStore {
    directory: Option<PathBuf>,
    pub(crate) state: StoreState,
    held: Option<Lock>,
}

impl ResidentStore {
    /** An in-memory store
     * Input
        - None
     * Output
        - ResidentStore
    */
    pub(crate) fn in_memory() -> Self {
        Self {
            directory: None,
            state: StoreState::default(),
            held: None,
        }
    }

    /** The writers' lock file of a runtime directory
     * Input
        - runtime: &Path - runtime directory
     * Output
        - PathBuf
    */
    fn lock_path(runtime: &Path) -> PathBuf {
        runtime.join("resident").join("worker.lock")
    }

    /** Open the store to change it: wait for the writers' lock, then read, and keep the lock until
     * the store is dropped (commands)
     * Input
        - runtime: &Path - runtime directory
     * Output
        - Result<ResidentStore, String>
        - Error if the lock is not free within the lock timeout, or the store was altered
    */
    pub(crate) fn open_exclusive(runtime: &Path) -> Result<Self, String> {
        let lock = Lock::acquire(&Self::lock_path(runtime))?;
        let mut store = Self::open(runtime)?;
        store.held = Some(lock);
        Ok(store)
    }

    /** Open the store to change it only if no other writer holds it (background work, which
     * simply runs again at the next sync)
     * Input
        - runtime: &Path - runtime directory
     * Output
        - Result<Option<ResidentStore>, String> - None when another writer holds it
    */
    pub(crate) fn try_open_exclusive(runtime: &Path) -> Result<Option<Self>, String> {
        let Some(lock) = Lock::try_acquire(&Self::lock_path(runtime))? else {
            return Ok(None);
        };
        let mut store = Self::open(runtime)?;
        store.held = Some(lock);
        Ok(Some(store))
    }

    /** Open the store of a runtime directory, refusing one that was altered
     * Input
        - runtime: &Path - the repository's runtime directory
     * Output
        - Result<ResidentStore, String>
    */
    pub(crate) fn open(runtime: &Path) -> Result<Self, String> {
        let directory = runtime.join("resident");
        let path = directory.join("store.json");
        let state = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<StoreState>(&text)
                .map_err(|error| format!("{} is malformed: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreState::default(),
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        };
        let store = Self {
            directory: Some(directory),
            state,
            held: None,
        };
        store.verify()?;
        Ok(store)
    }

    /** Check every digest of the contents
     * Input
        - None (uses self)
     * Output
        - Result<(), String>
        - Error naming the first record altered outside Crane
    */
    pub(crate) fn verify(&self) -> Result<(), String> {
        for versions in self.state.definitions.values() {
            for record in versions.values() {
                if record.definition.digest() != record.digest {
                    return Err(format!(
                        "definition {}@{} was altered outside Crane (its digest does not match)",
                        record.definition.definition_id, record.definition.version
                    ));
                }
            }
        }
        for versions in self.state.rcas.values() {
            for record in versions.values() {
                if record.graph.digest() != record.digest {
                    return Err(format!(
                        "RCA graph {}@{} was altered outside Crane",
                        record.graph.rca_id, record.graph.version
                    ));
                }
            }
        }
        if let Some(assignment) = self
            .state
            .assignments
            .values()
            .find(|assignment| !assignment.verify_digest())
        {
            return Err(format!(
                "assignment {} was altered outside Crane",
                assignment.assignment_id
            ));
        }
        if let Some(activation) = self
            .state
            .activations
            .values()
            .find(|activation| !activation.verify_digest())
        {
            return Err(format!(
                "activation {} was altered outside Crane",
                activation.activation_id
            ));
        }
        Ok(())
    }

    /** Persist the store atomically under a lock (in memory: nothing)
     * Input
        - None (uses self)
     * Output
        - Result<(), String>
    */
    pub(crate) fn save(&self) -> Result<(), String> {
        let Some(directory) = &self.directory else {
            return Ok(());
        };
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        let _lock = Lock::acquire(&directory.join("store.lock"))?;
        let text = serde_json::to_string_pretty(&self.state).map_err(|error| error.to_string())?;
        write_atomic(&directory.join("store.json"), text.as_bytes())
    }

    /** Take the worker lock of the store, or None when another worker holds it (a store opened
     * exclusively already holds it; in memory a lock is never needed)
     * Input
        - None (uses self)
     * Output
        - Result<Option<Option<Lock>>, String> Some(lock) when taken, None when busy
    */
    pub(crate) fn worker_lock(&self) -> Result<Option<Option<Lock>>, String> {
        let Some(directory) = &self.directory else {
            return Ok(Some(None));
        };
        if self.held.is_some() {
            return Ok(Some(None));
        }
        std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
        Ok(Lock::try_acquire(&directory.join("worker.lock"))?.map(Some))
    }

    /** A definition version
     * Input
        - definition_id: &str - definition
        - version: Option<u32> - version, None for the latest
     * Output
        - Result<&DefinitionRecord, String>
    */
    pub(crate) fn definition(
        &self,
        definition_id: &str,
        version: Option<u32>,
    ) -> Result<&DefinitionRecord, String> {
        let versions = self
            .state
            .definitions
            .get(definition_id)
            .ok_or_else(|| format!("definition {definition_id} does not exist"))?;
        match version {
            Some(version) => versions
                .get(&version)
                .ok_or_else(|| format!("definition {definition_id}@{version} does not exist")),
            None => versions
                .values()
                .next_back()
                .ok_or_else(|| format!("definition {definition_id} has no version")),
        }
    }

    /** An RCA graph version
     * Input
        - rca_id: &str - graph
        - version: Option<u32> - version, None for the latest
     * Output
        - Result<&RcaRecord, String>
    */
    pub(crate) fn rca(&self, rca_id: &str, version: Option<u32>) -> Result<&RcaRecord, String> {
        let versions = self
            .state
            .rcas
            .get(rca_id)
            .ok_or_else(|| format!("RCA graph {rca_id} does not exist"))?;
        match version {
            Some(version) => versions
                .get(&version)
                .ok_or_else(|| format!("RCA graph {rca_id}@{version} does not exist")),
            None => versions
                .values()
                .next_back()
                .ok_or_else(|| format!("RCA graph {rca_id} has no version")),
        }
    }

    /** An assignment
     * Input
        - assignment_id: &str - assignment
     * Output
        - Result<&Assignment, String>
    */
    pub(crate) fn assignment(&self, assignment_id: &str) -> Result<&Assignment, String> {
        self.state
            .assignments
            .get(assignment_id)
            .ok_or_else(|| format!("assignment {assignment_id} does not exist"))
    }

    /** Store an assignment, sealed
     * Input
        - assignment: Assignment - assignment
     * Output
        - None
    */
    pub(crate) fn put_assignment(&mut self, mut assignment: Assignment) {
        assignment.seal();
        self.state
            .assignments
            .insert(assignment.assignment_id.clone(), assignment);
    }

    /** An activation
     * Input
        - activation_id: &str - activation
     * Output
        - Result<&Activation, String>
    */
    pub(crate) fn activation(&self, activation_id: &str) -> Result<&Activation, String> {
        self.state
            .activations
            .get(activation_id)
            .ok_or_else(|| format!("activation {activation_id} does not exist"))
    }

    /** Store an activation, sealed
     * Input
        - activation: Activation - activation
     * Output
        - None
    */
    pub(crate) fn put_activation(&mut self, mut activation: Activation) {
        activation.seal();
        self.state
            .activations
            .insert(activation.activation_id.clone(), activation);
    }

    /** Record a trigger that woke nothing (bounded)
     * Input
        - ignored: Ignored - entry
     * Output
        - None
    */
    pub(crate) fn ignore(&mut self, ignored: Ignored) {
        crate::platform::notes::note(
            "RESIDENT_TRIGGER_IGNORED",
            json!({"trigger": ignored.trigger_id, "source": ignored.source, "kind": ignored.kind, "assignment": ignored.assignment, "result": ignored.reason}),
        );
        self.state.ignored.push_back(ignored);
        while self.state.ignored.len() > MAX_IGNORED {
            self.state.ignored.pop_front();
        }
    }

    /** Keep a test report (bounded)
     * Input
        - report: Value - report
     * Output
        - None
    */
    pub(crate) fn keep_test(&mut self, report: Value) {
        self.state.tests.push_back(report);
        while self.state.tests.len() > MAX_TESTS {
            self.state.tests.pop_front();
        }
    }

    /** Forget deduplication keys older than the retention
     * Input
        - now: u64 - Unix milliseconds
     * Output
        - None
    */
    pub(crate) fn prune(&mut self, now: u64) {
        self.state
            .seen
            .retain(|_, at| *at + DEDUP_RETENTION_MS > now);
    }
}
