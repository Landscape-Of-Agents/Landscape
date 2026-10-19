// The evolution store: one versioned JSON document (runtime/evolution/state.json) holding
// analyses, signals, proposals, regression plans, and regression runs. It follows the invocation
// ledger's and the capability registry's conventions: every change is a transaction (take the
// cross-process lock, reload, apply to a copy, replace the file atomically), so concurrent writers
// never lose each other's records and an interrupted write leaves the previous document intact.
// Every record carries a digest verified on load; a record that does not match fails the load
// with integrity_mismatch and nothing is repaired or guessed. Analyses, plans, and runs are
// immutable once stored (an identical analysis returns the stored one; a run id is never
// reused); signals and proposals only move forward through their lifecycles. The store is
// separate from the registry and the ledger and is never authoritative capability state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::models::{AnalysisReport, Proposal, RegressionPlan, RegressionRun, Sealed, Signal};
use super::EVOLUTION_SCHEMA_VERSION;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::platform::files::{write_atomic, Lock};

/** The store file */
const STATE_FILE: &str = "state.json";

/** The store document
 * Fields
    - schema_version: u32 - EVOLUTION_SCHEMA_VERSION
    - generation: u64 - raised by every committed transaction
    - analyses: BTreeMap<String, AnalysisReport> - analysis reports by id
    - signals: BTreeMap<String, Signal> - signals by id
    - proposals: BTreeMap<String, Proposal> - proposals by id
    - plans: BTreeMap<String, RegressionPlan> - regression plans by id
    - runs: BTreeMap<String, RegressionRun> - regression runs by id
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct EvolutionState {
    pub(crate) schema_version: u32,
    pub(crate) generation: u64,
    pub(crate) analyses: BTreeMap<String, AnalysisReport>,
    pub(crate) signals: BTreeMap<String, Signal>,
    pub(crate) proposals: BTreeMap<String, Proposal>,
    pub(crate) plans: BTreeMap<String, RegressionPlan>,
    pub(crate) runs: BTreeMap<String, RegressionRun>,
}

/** Build an integrity error
 * Input
    - what: &str - record kind
    - id: &str - record id
 * Output
    - CapabilityError
*/
fn tampered(what: &str, id: &str) -> CapabilityError {
    CapabilityError::new(
        ErrorCode::IntegrityMismatch,
        format!("{what} {id} does not match its digest; the evolution store was changed outside Crane and is not repaired"),
    )
}

/** Check every record of a document against its digest and its key
 * Input
    - state: &EvolutionState - document
 * Output
    - CapabilityResult<()>
*/
fn verify_all(state: &EvolutionState) -> CapabilityResult<()> {
    /** Check one map
     * Input
        - what: &str - record kind
        - map: &BTreeMap<String, T> - records
        - id: impl Fn(&T) -> &str - record id
     * Output
        - CapabilityResult<()>
    */
    fn check<T: Sealed>(
        what: &str,
        map: &BTreeMap<String, T>,
        id: impl Fn(&T) -> &str,
    ) -> CapabilityResult<()> {
        for (key, record) in map {
            if !record.verify() || id(record) != key {
                return Err(tampered(what, key));
            }
        }
        Ok(())
    }
    check("analysis", &state.analyses, |record| &record.report_id)?;
    check("signal", &state.signals, |record| &record.signal_id)?;
    check("proposal", &state.proposals, |record| &record.proposal_id)?;
    check("regression plan", &state.plans, |record| &record.plan_id)?;
    check("regression run", &state.runs, |record| &record.run_id)
}

/** Parse and verify a store document
 * Input
    - text: &str - JSON document
 * Output
    - CapabilityResult<EvolutionState>
    - Error for invalid JSON, another layout, or any record that does not match its digest
*/
pub(crate) fn parse(text: &str) -> CapabilityResult<EvolutionState> {
    let value: Value = serde_json::from_str(text).map_err(|error| {
        CapabilityError::new(
            ErrorCode::Persistence,
            format!("the evolution store is not valid JSON ({error}); restore it from a backup"),
        )
    })?;
    let layout = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if layout != u64::from(EVOLUTION_SCHEMA_VERSION) {
        return Err(CapabilityError::new(
            ErrorCode::UnsupportedSchemaVersion,
            format!("the evolution store has layout {layout}; this Crane reads {EVOLUTION_SCHEMA_VERSION}"),
        ));
    }
    let state: EvolutionState = serde_json::from_value(value).map_err(|error| {
        CapabilityError::new(
            ErrorCode::Persistence,
            format!("the evolution store is invalid: {error}"),
        )
    })?;
    verify_all(&state)?;
    Ok(state)
}

/** Load the document from a directory (empty when absent)
 * Input
    - directory: &Path - store directory
 * Output
    - CapabilityResult<EvolutionState>
*/
fn load(directory: &Path) -> CapabilityResult<EvolutionState> {
    match std::fs::read_to_string(directory.join(STATE_FILE)) {
        Ok(text) => parse(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(EvolutionState {
            schema_version: EVOLUTION_SCHEMA_VERSION,
            ..EvolutionState::default()
        }),
        Err(error) => Err(CapabilityError::new(
            ErrorCode::Persistence,
            format!("could not read the evolution store: {error}"),
        )),
    }
}

/** The evolution store
 * Fields
    - directory: Option<PathBuf> - persistence directory, None in memory
    - state: EvolutionState - the document as last committed
*/
#[derive(Debug)]
pub(crate) struct EvolutionStore {
    directory: Option<PathBuf>,
    pub(crate) state: EvolutionState,
}

impl EvolutionStore {
    /** An in-memory store
     * Input
        - None
     * Output
        - EvolutionStore
    */
    pub(crate) fn in_memory() -> Self {
        Self {
            directory: None,
            state: EvolutionState {
                schema_version: EVOLUTION_SCHEMA_VERSION,
                ..EvolutionState::default()
            },
        }
    }

    /** Open a persistent store
     * Input
        - directory: &Path - store directory
     * Output
        - CapabilityResult<EvolutionStore>
    */
    pub(crate) fn open(directory: &Path) -> CapabilityResult<Self> {
        let _lock = Lock::acquire(&directory.join("state.lock"))
            .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error))?;
        Ok(Self {
            directory: Some(directory.to_path_buf()),
            state: load(directory)?,
        })
    }

    /** Run a transaction: lock, reload, apply to a copy, verify, write atomically, commit
     * Input
        - change: impl FnOnce(&mut EvolutionState) -> CapabilityResult<T> - change
     * Output
        - CapabilityResult<T>
    */
    pub(crate) fn transact<T>(
        &mut self,
        change: impl FnOnce(&mut EvolutionState) -> CapabilityResult<T>,
    ) -> CapabilityResult<T> {
        let _lock = match &self.directory {
            Some(directory) => {
                let lock = Lock::acquire(&directory.join("state.lock"))
                    .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error))?;
                self.state = load(directory)?;
                Some(lock)
            }
            None => None,
        };
        let mut next = self.state.clone();
        let result = change(&mut next)?;
        verify_all(&next)?;
        next.generation += 1;
        if let Some(directory) = &self.directory {
            let bytes = serde_json::to_vec_pretty(&next)
                .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error.to_string()))?;
            write_atomic(&directory.join(STATE_FILE), &bytes)
                .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error))?;
        }
        self.state = next;
        Ok(result)
    }

    /** Find a signal
     * Input
        - id: &str - signal id
     * Output
        - CapabilityResult<Signal>
    */
    pub(crate) fn signal(&self, id: &str) -> CapabilityResult<Signal> {
        self.state.signals.get(id).cloned().ok_or_else(|| {
            CapabilityError::new(ErrorCode::NotFound, format!("signal {id} does not exist"))
        })
    }

    /** Find a proposal
     * Input
        - id: &str - proposal id
     * Output
        - CapabilityResult<Proposal>
    */
    pub(crate) fn proposal(&self, id: &str) -> CapabilityResult<Proposal> {
        self.state.proposals.get(id).cloned().ok_or_else(|| {
            CapabilityError::new(ErrorCode::NotFound, format!("proposal {id} does not exist"))
        })
    }

    /** Find a regression run
     * Input
        - id: &str - run id
     * Output
        - CapabilityResult<RegressionRun>
    */
    pub(crate) fn run(&self, id: &str) -> CapabilityResult<RegressionRun> {
        self.state.runs.get(id).cloned().ok_or_else(|| {
            CapabilityError::new(
                ErrorCode::NotFound,
                format!("regression run {id} does not exist"),
            )
        })
    }
}

/** Store a signal or proposal update, refusing to overwrite a newer stored history
 * Input
    - map: &mut BTreeMap<String, T> - records
    - id: String - record id
    - record: T - sealed record
    - history: impl Fn(&T) -> usize - its transition count
 * Output
    - CapabilityResult<()>
*/
pub(crate) fn put_forward<T: Sealed>(
    map: &mut BTreeMap<String, T>,
    id: String,
    record: T,
    history: impl Fn(&T) -> usize,
) -> CapabilityResult<()> {
    if map
        .get(&id)
        .is_some_and(|stored| history(stored) > history(&record))
    {
        return Err(CapabilityError::new(
            ErrorCode::InvalidTransition,
            format!("{id} has a newer stored history; reload it instead of overwriting"),
        ));
    }
    map.insert(id, record);
    Ok(())
}

/** Store an immutable record once; an existing record with the same id is never replaced
 * Input
    - map: &mut BTreeMap<String, T> - records
    - id: String - record id
    - record: T - sealed record
 * Output
    - CapabilityResult<bool> whether it was inserted (false: already stored)
*/
pub(crate) fn put_once<T: Sealed>(
    map: &mut BTreeMap<String, T>,
    id: String,
    record: T,
) -> CapabilityResult<bool> {
    if map.contains_key(&id) {
        return Ok(false);
    }
    map.insert(id, record);
    Ok(true)
}
