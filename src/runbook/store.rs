// The runbook store (runtime/runbook/store.json): immutable runbook versions with their lifecycle
// (draft, active, retired, revoked) and run records. A version's stored digest must equal the
// digest of its definition, and every run record is sealed with a digest, so a store edited outside
// Crane is refused when it is read. Writes are atomic and made under a lock.
//
// Lifecycle rules: a draft grants nothing; activation needs a person other than the author holding
// the publisher role, a definition that validates, and every pinned skill published with its
// pinned digest; retirement stops new runs; revocation stops new runs and every run still going at
// its next step. Neither can be undone: a new version is the way back.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{Runbook, Step};
use crate::capture::Actor;
use crate::platform::files::{write_atomic, Lock};
use crate::trust::crypto::digest_json;

/** Lifecycle of a runbook version
 * Variants
    - Draft - stored, grants nothing, cannot run
    - Active - may run
    - Retired - no new runs; history kept
    - Revoked - no new runs, and running runs stop at their next step
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunbookState {
    Draft,
    Active,
    Retired,
    Revoked,
}

/** A lifecycle change
 * Fields
    - to: RunbookState - new state
    - by: Actor - who
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Change {
    pub(crate) to: RunbookState,
    pub(crate) by: Actor,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** One stored runbook version
 * Fields
    - definition: Runbook - the immutable definition
    - digest: String - its digest
    - state: RunbookState - lifecycle
    - created_by: Actor - author
    - created_at: u64 - Unix milliseconds
    - changes: Vec<Change> - lifecycle history
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunbookRecord {
    pub(crate) definition: Runbook,
    pub(crate) digest: String,
    pub(crate) state: RunbookState,
    pub(crate) created_by: Actor,
    pub(crate) created_at: u64,
    pub(crate) changes: Vec<Change>,
}

/** State of one step in a run
 * Variants
    - Pending - not started
    - AwaitingApproval - waits for a person to approve the runbook's approval point
    - AwaitingSkillApproval - its invocation waits for the skill's own approval
    - Succeeded - verified postconditions
    - Failed - failed
    - Blocked - stopped before any effect
    - NeedsReconciliation - effects are uncertain; reconcile before anything else
    - Skipped - not run (a branch did not apply, or a dependency did not succeed)
    - Cancelled - the run was cancelled before it finished
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StepState {
    Pending,
    AwaitingApproval,
    AwaitingSkillApproval,
    Succeeded,
    Failed,
    Blocked,
    NeedsReconciliation,
    Skipped,
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
            Self::Succeeded | Self::Failed | Self::Blocked | Self::Skipped | Self::Cancelled
        )
    }
}

/** State of a run
 * Variants
    - Running - steps can still start
    - AwaitingApproval - paused for a person (a runbook approval point or a skill approval)
    - NeedsReconciliation - paused: a step's effects are uncertain
    - Succeeded - every required step succeeded with verified postconditions
    - Partial - some required steps succeeded, others did not
    - Failed - no required step succeeded, or a required step failed
    - Blocked - it could not continue (budget, revocation, or a block before any effect)
    - Cancelled - a person cancelled it
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Running,
    AwaitingApproval,
    NeedsReconciliation,
    Succeeded,
    Partial,
    Failed,
    Blocked,
    Cancelled,
}

impl RunState {
    /** Report whether the run has ended
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn ended(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Partial | Self::Failed | Self::Blocked | Self::Cancelled
        )
    }
}

/** One step of a run
 * Fields
    - state: StepState - state
    - attempts: u32 - invocations submitted
    - invocation_id: Option<String> - the current attempt's invocation
    - reservation: Option<String> - the budget reservation of the current attempt
    - detail: String - why it is in this state
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepRun {
    pub(crate) state: StepState,
    pub(crate) attempts: u32,
    pub(crate) invocation_id: Option<String>,
    pub(crate) reservation: Option<String>,
    pub(crate) detail: String,
}

/** A person's approval of a runbook approval point
 * Fields
    - step: String - step
    - digest: String - the step approval digest it is bound to
    - approver: Actor - approver
    - at: u64 - Unix milliseconds
    - revoked: Option<String> - why it was revoked before it was used
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepApproval {
    pub(crate) step: String,
    pub(crate) digest: String,
    pub(crate) approver: Actor,
    pub(crate) at: u64,
    #[serde(default)]
    pub(crate) revoked: Option<String>,
}

/** A run of a runbook version
 * Fields
    - run_id: String - identifier ("rbr_...")
    - runbook_id, version, digest - the exact runbook version
    - started_by: Actor - who started it
    - inputs: BTreeMap<String, String> - run inputs (credential references, never secrets)
    - confirmed: Vec<String> - preconditions the starter confirmed
    - idempotency_key: String - key it was started with
    - state: RunState - state
    - steps: BTreeMap<String, StepRun> - per step
    - approvals: Vec<StepApproval> - approval point decisions
    - log: Vec<String> - what happened, in order
    - created_at, updated_at: u64 - Unix milliseconds
    - record_digest: String - integrity digest of the record without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RunRecord {
    pub(crate) run_id: String,
    pub(crate) runbook_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) started_by: Actor,
    pub(crate) inputs: BTreeMap<String, String>,
    pub(crate) confirmed: Vec<String>,
    pub(crate) idempotency_key: String,
    pub(crate) state: RunState,
    pub(crate) steps: BTreeMap<String, StepRun>,
    pub(crate) approvals: Vec<StepApproval>,
    pub(crate) log: Vec<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) record_digest: String,
}

impl RunRecord {
    /** The digest of the record without its digest
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn compute_digest(&self) -> String {
        let mut content = self.clone();
        content.record_digest = String::new();
        digest_json("runbook/run/v1", &json!(content))[..32].to_string()
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

    /** The digest an approval of a step is bound to: run, runbook digest, step, and the step's
     * resolved definition (skill, digest, bindings, inputs)
     * Input
        - step: &Step - step
     * Output
        - String
    */
    pub(crate) fn approval_digest(&self, step: &Step) -> String {
        digest_json(
            "runbook/approval/v1",
            &json!([self.run_id, self.digest, step, self.inputs]),
        )[..24]
            .to_string()
    }
}

/** What the store holds
 * Fields
    - runbooks: BTreeMap<String, BTreeMap<u32, RunbookRecord>> - runbook id, then version
    - runs: BTreeMap<String, RunRecord> - runs by id
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StoreState {
    pub(crate) runbooks: BTreeMap<String, BTreeMap<u32, RunbookRecord>>,
    pub(crate) runs: BTreeMap<String, RunRecord>,
}

/** The runbook store
 * Fields
    - directory: Option<PathBuf> - runtime/runbook, None in memory
    - state: StoreState - contents
*/
pub(crate) struct RunbookStore {
    directory: Option<PathBuf>,
    pub(crate) state: StoreState,
}

impl RunbookStore {
    /** An in-memory store
     * Input
        - None
     * Output
        - RunbookStore
    */
    pub(crate) fn in_memory() -> Self {
        Self {
            directory: None,
            state: StoreState::default(),
        }
    }

    /** Open the store of a runtime directory, refusing one that was altered
     * Input
        - runtime: &Path - the repository's runtime directory
     * Output
        - Result<RunbookStore, String>
    */
    pub(crate) fn open(runtime: &Path) -> Result<Self, String> {
        let directory = runtime.join("runbook");
        let path = directory.join("store.json");
        let state = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str::<StoreState>(&text)
                .map_err(|error| format!("{} is malformed: {error}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => StoreState::default(),
            Err(error) => return Err(format!("could not read {}: {error}", path.display())),
        };
        for versions in state.runbooks.values() {
            for record in versions.values() {
                if record.definition.digest() != record.digest {
                    return Err(format!(
                        "runbook {} v{} was altered outside Crane (its digest does not match)",
                        record.definition.runbook_id, record.definition.version
                    ));
                }
            }
        }
        if let Some(run) = state.runs.values().find(|run| !run.verify_digest()) {
            return Err(format!(
                "run {} was altered outside Crane (its digest does not match)",
                run.run_id
            ));
        }
        Ok(Self {
            directory: Some(directory),
            state,
        })
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
        let _lock = Lock::acquire(&directory.join("store.lock"))?;
        let text = serde_json::to_string_pretty(&self.state).map_err(|error| error.to_string())?;
        write_atomic(&directory.join("store.json"), text.as_bytes())
    }

    /** A runbook version
     * Input
        - runbook_id: &str - runbook
        - version: Option<u32> - version, None for the latest
     * Output
        - Result<&RunbookRecord, String>
    */
    pub(crate) fn runbook(
        &self,
        runbook_id: &str,
        version: Option<u32>,
    ) -> Result<&RunbookRecord, String> {
        let versions = self
            .state
            .runbooks
            .get(runbook_id)
            .ok_or_else(|| format!("runbook {runbook_id} does not exist"))?;
        match version {
            Some(version) => versions
                .get(&version)
                .ok_or_else(|| format!("runbook {runbook_id} v{version} does not exist")),
            None => versions
                .values()
                .next_back()
                .ok_or_else(|| format!("runbook {runbook_id} has no version")),
        }
    }

    /** Store a new draft version (an existing version is never replaced)
     * Input
        - definition: Runbook - definition
        - author: &Actor - author
        - at: u64 - Unix milliseconds
     * Output
        - Result<RunbookRecord, String>
    */
    pub(crate) fn create(
        &mut self,
        definition: Runbook,
        author: &Actor,
        at: u64,
    ) -> Result<RunbookRecord, String> {
        let versions = self
            .state
            .runbooks
            .entry(definition.runbook_id.clone())
            .or_default();
        if versions.contains_key(&definition.version) {
            return Err(format!(
                "runbook {} v{} already exists; versions are immutable, create v{}",
                definition.runbook_id,
                definition.version,
                versions.keys().next_back().copied().unwrap_or(0) + 1
            ));
        }
        let record = RunbookRecord {
            digest: definition.digest(),
            definition,
            state: RunbookState::Draft,
            created_by: author.clone(),
            created_at: at,
            changes: Vec::new(),
        };
        versions.insert(record.definition.version, record.clone());
        Ok(record)
    }

    /** Change a version's lifecycle state
     * Input
        - runbook_id: &str - runbook
        - version: u32 - version
        - to: RunbookState - new state
        - by: &Actor - who
        - reason: &str - why
        - at: u64 - Unix milliseconds
     * Output
        - Result<RunbookRecord, String>
        - Error when the transition is not allowed
    */
    pub(crate) fn transition(
        &mut self,
        runbook_id: &str,
        version: u32,
        to: RunbookState,
        by: &Actor,
        reason: &str,
        at: u64,
    ) -> Result<RunbookRecord, String> {
        let record = self
            .state
            .runbooks
            .get_mut(runbook_id)
            .and_then(|versions| versions.get_mut(&version))
            .ok_or_else(|| format!("runbook {runbook_id} v{version} does not exist"))?;
        let allowed = matches!(
            (record.state, to),
            (RunbookState::Draft, RunbookState::Active)
                | (
                    RunbookState::Draft | RunbookState::Active,
                    RunbookState::Retired
                )
                | (
                    RunbookState::Draft | RunbookState::Active | RunbookState::Retired,
                    RunbookState::Revoked
                )
        );
        if !allowed {
            return Err(format!(
                "runbook {runbook_id} v{version} is {:?} and cannot become {to:?}",
                record.state
            ));
        }
        if reason.trim().is_empty() {
            return Err("a reason is required".into());
        }
        record.state = to;
        record.changes.push(Change {
            to,
            by: by.clone(),
            reason: reason.into(),
            at,
        });
        Ok(record.clone())
    }

    /** A run
     * Input
        - run_id: &str - run
     * Output
        - Result<&RunRecord, String>
    */
    pub(crate) fn run(&self, run_id: &str) -> Result<&RunRecord, String> {
        self.state
            .runs
            .get(run_id)
            .ok_or_else(|| format!("run {run_id} does not exist"))
    }

    /** Store a run, sealed
     * Input
        - run: RunRecord - run
     * Output
        - RunRecord - the sealed record
    */
    pub(crate) fn put_run(&mut self, mut run: RunRecord) -> RunRecord {
        run.seal();
        self.state.runs.insert(run.run_id.clone(), run.clone());
        run
    }
}

/** Describe a runbook record for output
 * Input
    - record: &RunbookRecord - record
 * Output
    - Value
*/
pub(crate) fn summary(record: &RunbookRecord) -> Value {
    json!({
        "runbook_id": record.definition.runbook_id,
        "version": record.definition.version,
        "digest": record.digest,
        "state": record.state,
        "name": record.definition.name,
        "department": record.definition.department,
        "created_by": record.created_by.id,
        "steps": record.definition.steps.iter().map(|step| json!({
            "step_id": step.step_id,
            "skill": format!("{}@{}", step.skill_id, step.skill_version),
            "depends_on": step.depends_on,
            "when": step.when,
            "approval": step.approval,
            "max_retries": step.max_retries,
            "on_failure": step.on_failure,
            "operations": step.cost(),
        })).collect::<Vec<_>>(),
        "required": record.definition.required(),
        "budget": record.definition.budget,
    })
}
