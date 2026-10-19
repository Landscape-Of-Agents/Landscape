// The invocation ledger: one versioned JSON document (runtime/invocation/ledger.json) holding
// every invocation record, the idempotency index, and refused actions. Like the capability
// registry, every change is a transaction: take the cross-process lock, reload the document,
// apply the change to a copy, and replace the file atomically, so concurrent invocations never
// lose each other's records and an interrupted write leaves the previous document intact. Each
// record carries a digest that is verified whenever the ledger is loaded; a record that does not
// match fails the load, and nothing is silently repaired or overwritten. Idempotency is decided
// inside the transaction, so two concurrent requests with the same key yield one invocation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::engine::InvocationRecord;
use super::RECORD_SCHEMA_VERSION;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::Actor;
use crate::platform::files::{write_atomic, Lock};

/** The ledger file */
const LEDGER_FILE: &str = "ledger.json";

/** A refused action, kept for audit
 * Fields
    - action: String - what was refused
    - invocation_id: Option<String> - invocation
    - actor: Actor - who asked
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LedgerDenial {
    pub(crate) action: String,
    pub(crate) invocation_id: Option<String>,
    pub(crate) actor: Actor,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** The ledger document
 * Fields
    - schema_version: u32 - RECORD_SCHEMA_VERSION
    - generation: u64 - raised by every committed transaction
    - records: BTreeMap<String, InvocationRecord> - records by invocation id
    - idempotency: BTreeMap<String, (String, String)> - key to (invocation id, request digest)
    - denials: Vec<LedgerDenial> - refused actions
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LedgerState {
    pub(crate) schema_version: u32,
    pub(crate) generation: u64,
    pub(crate) records: BTreeMap<String, InvocationRecord>,
    pub(crate) idempotency: BTreeMap<String, (String, String)>,
    pub(crate) denials: Vec<LedgerDenial>,
}

/** Build a persistence error
 * Input
    - message: String - explanation
 * Output
    - CapabilityError
*/
fn persistence(message: String) -> CapabilityError {
    CapabilityError::new(ErrorCode::Persistence, message)
}

/** Load and verify a ledger document
 * Input
    - directory: &Path - ledger directory
 * Output
    - CapabilityResult<LedgerState>
    - Error for unreadable or truncated documents, newer layouts, or tampered records
*/
fn load(directory: &Path) -> CapabilityResult<LedgerState> {
    let text = match std::fs::read_to_string(directory.join(LEDGER_FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LedgerState {
                schema_version: RECORD_SCHEMA_VERSION,
                ..LedgerState::default()
            })
        }
        Err(error) => {
            return Err(persistence(format!(
                "could not read the invocation ledger: {error}"
            )))
        }
    };
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        persistence(format!(
            "the invocation ledger is not valid JSON ({error}); restore it from a backup"
        ))
    })?;
    let layout = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if layout != u64::from(RECORD_SCHEMA_VERSION) {
        return Err(CapabilityError::new(ErrorCode::UnsupportedSchemaVersion, format!("the invocation ledger has layout {layout}; this Crane reads {RECORD_SCHEMA_VERSION}")));
    }
    let state: LedgerState = serde_json::from_value(value)
        .map_err(|error| persistence(format!("the invocation ledger is invalid: {error}")))?;
    for record in state.records.values() {
        if !record.verify_digest() {
            return Err(CapabilityError::new(ErrorCode::IntegrityMismatch, format!("invocation {} does not match its digest; the ledger was changed outside Crane and is not repaired", record.invocation_id)));
        }
    }
    Ok(state)
}

/** Whether an insertion created a record or found an existing one
 * Variants
    - Inserted - the record was stored
    - Existing - the same request was already stored under the key
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Insertion {
    Inserted(Box<InvocationRecord>),
    Existing(Box<InvocationRecord>),
}

/** The invocation ledger
 * Fields
    - directory: Option<PathBuf> - persistence directory, None in memory
    - state: LedgerState - the document as last committed
*/
#[derive(Debug)]
pub(crate) struct Ledger {
    directory: Option<PathBuf>,
    pub(crate) state: LedgerState,
}

impl Ledger {
    /** An in-memory ledger
     * Input
        - None
     * Output
        - Ledger
    */
    pub(crate) fn in_memory() -> Self {
        Self {
            directory: None,
            state: LedgerState {
                schema_version: RECORD_SCHEMA_VERSION,
                ..LedgerState::default()
            },
        }
    }

    /** Open a persistent ledger
     * Input
        - directory: &Path - ledger directory
     * Output
        - CapabilityResult<Ledger>
    */
    pub(crate) fn open(directory: &Path) -> CapabilityResult<Self> {
        let _lock = Lock::acquire(&directory.join("ledger.lock")).map_err(persistence)?;
        Ok(Self {
            directory: Some(directory.to_path_buf()),
            state: load(directory)?,
        })
    }

    /** Run a transaction: lock, reload, apply to a copy, write atomically, commit
     * Input
        - change: impl FnOnce(&mut LedgerState) -> CapabilityResult<T> - change
     * Output
        - CapabilityResult<T>
    */
    fn transact<T>(
        &mut self,
        change: impl FnOnce(&mut LedgerState) -> CapabilityResult<T>,
    ) -> CapabilityResult<T> {
        let _lock = match &self.directory {
            Some(directory) => {
                let lock = Lock::acquire(&directory.join("ledger.lock")).map_err(persistence)?;
                self.state = load(directory)?;
                Some(lock)
            }
            None => None,
        };
        let mut next = self.state.clone();
        let result = change(&mut next)?;
        next.generation += 1;
        if let Some(directory) = &self.directory {
            let bytes =
                serde_json::to_vec_pretty(&next).map_err(|error| persistence(error.to_string()))?;
            write_atomic(&directory.join(LEDGER_FILE), &bytes).map_err(persistence)?;
        }
        self.state = next;
        Ok(result)
    }

    /** Store a new record unless its idempotency key is taken: the same request returns the
     * existing record, a different request is a conflict
     * Input
        - record: InvocationRecord - new record (sealed)
     * Output
        - CapabilityResult<Insertion>
        - Error (IdempotencyConflict) when the key was used for a different request
    */
    pub(crate) fn insert(&mut self, record: InvocationRecord) -> CapabilityResult<Insertion> {
        self.transact(|state| {
            if let Some((id, digest)) = state.idempotency.get(&record.idempotency_key) {
                if *digest != record.request_digest {
                    return Err(CapabilityError::new(
                        ErrorCode::IdempotencyConflict,
                        format!(
                            "idempotency key {} was used for a different request (invocation {id})",
                            record.idempotency_key
                        ),
                    ));
                }
                let existing = state.records.get(id).cloned().ok_or_else(|| {
                    persistence(format!(
                        "the idempotency index names a missing invocation {id}"
                    ))
                })?;
                return Ok(Insertion::Existing(Box::new(existing)));
            }
            state.idempotency.insert(
                record.idempotency_key.clone(),
                (record.invocation_id.clone(), record.request_digest.clone()),
            );
            state
                .records
                .insert(record.invocation_id.clone(), record.clone());
            Ok(Insertion::Inserted(Box::new(record)))
        })
    }

    /** Replace an existing record (its digest must be sealed and its generation must not regress)
     * Input
        - record: &InvocationRecord - record
     * Output
        - CapabilityResult<()>
    */
    pub(crate) fn save(&mut self, record: &InvocationRecord) -> CapabilityResult<()> {
        if !record.verify_digest() {
            return Err(CapabilityError::new(
                ErrorCode::IntegrityMismatch,
                "the invocation record is not sealed",
            ));
        }
        self.transact(|state| {
            match state.records.get(&record.invocation_id) {
                None => {
                    return Err(CapabilityError::new(
                        ErrorCode::NotFound,
                        format!("invocation {} does not exist", record.invocation_id),
                    ))
                }
                Some(stored) if stored.transitions.len() > record.transitions.len() => {
                    return Err(CapabilityError::new(
                        ErrorCode::InvalidTransition,
                        "the stored invocation is newer; reload it instead of overwriting history",
                    ));
                }
                _ => {}
            }
            state
                .records
                .insert(record.invocation_id.clone(), record.clone());
            Ok(())
        })
    }

    /** Record a refused action (best effort; the refusal itself is returned by the caller)
     * Input
        - denial: LedgerDenial - refusal
     * Output
        - None
    */
    pub(crate) fn deny(&mut self, denial: LedgerDenial) {
        let _ = self.transact(|state| {
            state.denials.push(denial);
            Ok(())
        });
    }

    /** Find a record
     * Input
        - invocation_id: &str - invocation
     * Output
        - CapabilityResult<InvocationRecord>
    */
    pub(crate) fn get(&self, invocation_id: &str) -> CapabilityResult<InvocationRecord> {
        self.state
            .records
            .get(invocation_id)
            .cloned()
            .ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::NotFound,
                    format!("invocation {invocation_id} does not exist"),
                )
            })
    }
}
