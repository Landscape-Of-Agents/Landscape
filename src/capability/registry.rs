// The Capability Registry: the authoritative store of capability versions, intakes, validation
// runs, and lifecycle history. Persistence reuses Crane's file primitives, with stronger
// guarantees than Work Capture's per-file JSONL rewrites, because several records change together
// (a version, its state, a run, transitions): the whole registry is one versioned JSON document
// (runtime/capability/registry.json), and every mutation is a transaction that takes the
// cross-process lock, reloads the document from disk, applies the change to a copy, and replaces
// the file atomically (temporary file and rename). A failed operation writes nothing; an
// interrupted write leaves the previous document intact; concurrent writers serialize on the lock
// and never lose each other's updates. Version definitions are immutable once stored (any change
// is a new version linked to its predecessor); lifecycle state, staleness, withdrawal, and
// invocation references are separate, mutable metadata; transitions and denials are append-only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::intake::{self, CapabilityIntake, IntakeRequest};
use super::lifecycle::{
    self, publication_gate, state_after, Eligibility, LifecycleState, LifecycleTransition,
    PublicationRequest,
};
use super::model::{CapabilityDefinition, VersionConstraint};
use super::refs::{RefOutcome, Sources};
use super::validate::{self, select_profile, GateResult, GateStatus, ValidationRun};
use super::{CapabilityError, CapabilityResult, ErrorCode, Grants, Role};
use crate::capture::{stable_id, Actor};
use crate::platform::files::{write_atomic, Lock};

/** Version of the registry document layout */
pub(crate) const REGISTRY_SCHEMA_VERSION: u32 = 1;

/** File holding the registry */
const REGISTRY_FILE: &str = "registry.json";

/** Outcome of one use of a capability (recorded for the future; never changes a definition)
 * Variants
    - Succeeded - the use achieved its verified result
    - Failed - the use failed
    - Unknown - the outcome was not established
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvocationStatus {
    Succeeded,
    Failed,
    Unknown,
}

/** A reference to one use of a capability version
 * Fields
    - invocation_id: String - identifier (an episode or execution id elsewhere)
    - status: InvocationStatus - outcome of that use
    - evidence_refs: Vec<String> - evidence about it
    - recorded_by: Actor - who recorded it
    - recorded_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvocationRef {
    pub(crate) invocation_id: String,
    pub(crate) status: InvocationStatus,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) recorded_by: Actor,
    pub(crate) recorded_at: u64,
}

/** A note that a validation run no longer matches the current references
 * Fields
    - run_id: String - run that became stale
    - reasons: Vec<String> - what changed
    - detected_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StaleNote {
    pub(crate) run_id: String,
    pub(crate) reasons: Vec<String>,
    pub(crate) detected_at: u64,
}

/** A deprecation, revocation, or rejection
 * Fields
    - state: LifecycleState - deprecated, revoked, or rejected
    - reason: String - why
    - actor: Actor - who decided it
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Withdrawal {
    pub(crate) state: LifecycleState,
    pub(crate) reason: String,
    pub(crate) actor: Actor,
    pub(crate) at: u64,
}

/** One stored capability version: the immutable definition and its mutable metadata
 * Fields
    - definition: CapabilityDefinition - immutable definition
    - definition_digest: String - its digest, checked on every load
    - profile_id: String - validation profile
    - state: LifecycleState - current lifecycle state
    - created_at: u64 - Unix milliseconds of storage
    - validation_runs: Vec<String> - validation runs, oldest first
    - stale: Vec<StaleNote> - staleness detected since the latest run
    - superseded_by: Option<u32> - a later published version
    - withdrawals: Vec<Withdrawal> - deprecation, revocation, rejection history
    - invocations: Vec<InvocationRef> - uses (separate from the lifecycle state)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VersionRecord {
    pub(crate) definition: CapabilityDefinition,
    pub(crate) definition_digest: String,
    pub(crate) profile_id: String,
    pub(crate) state: LifecycleState,
    pub(crate) created_at: u64,
    pub(crate) validation_runs: Vec<String>,
    #[serde(default)]
    pub(crate) stale: Vec<StaleNote>,
    #[serde(default)]
    pub(crate) superseded_by: Option<u32>,
    #[serde(default)]
    pub(crate) withdrawals: Vec<Withdrawal>,
    #[serde(default)]
    pub(crate) invocations: Vec<InvocationRef>,
}

/** A refused privileged operation, kept for audit
 * Fields
    - action: String - publish, deprecate, revoke, reject, validate
    - capability_id: String - capability
    - version: Option<u32> - version
    - actor: Actor - who asked
    - reason: String - why it was refused
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Denial {
    pub(crate) action: String,
    pub(crate) capability_id: String,
    pub(crate) version: Option<u32>,
    pub(crate) actor: Actor,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** The registry document
 * Fields
    - schema_version: u32 - REGISTRY_SCHEMA_VERSION
    - generation: u64 - raised by every committed transaction
    - migrated_from: Option<u32> - layout it was migrated from, if any
    - intakes: BTreeMap<String, CapabilityIntake> - intakes by id
    - idempotency: BTreeMap<String, String> - idempotency key to intake id
    - versions: BTreeMap<String, BTreeMap<u32, VersionRecord>> - capability id, then version
    - runs: BTreeMap<String, ValidationRun> - validation runs by id
    - transitions: Vec<LifecycleTransition> - append-only lifecycle history
    - denials: Vec<Denial> - append-only refused privileged operations
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegistryState {
    pub(crate) schema_version: u32,
    pub(crate) generation: u64,
    #[serde(default)]
    pub(crate) migrated_from: Option<u32>,
    pub(crate) intakes: BTreeMap<String, CapabilityIntake>,
    pub(crate) idempotency: BTreeMap<String, String>,
    pub(crate) versions: BTreeMap<String, BTreeMap<u32, VersionRecord>>,
    pub(crate) runs: BTreeMap<String, ValidationRun>,
    pub(crate) transitions: Vec<LifecycleTransition>,
    #[serde(default)]
    pub(crate) denials: Vec<Denial>,
}

impl RegistryState {
    /** A version record
     * Input
        - capability_id: &str - capability
        - version: u32 - version
     * Output
        - CapabilityResult<&VersionRecord>
        - Error (NotFound) when it does not exist
    */
    pub(crate) fn record(
        &self,
        capability_id: &str,
        version: u32,
    ) -> CapabilityResult<&VersionRecord> {
        self.versions
            .get(capability_id)
            .and_then(|versions| versions.get(&version))
            .ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::NotFound,
                    format!("capability {capability_id} v{version} does not exist"),
                )
            })
    }

    /** A mutable version record
     * Input
        - capability_id: &str - capability
        - version: u32 - version
     * Output
        - CapabilityResult<&mut VersionRecord>
        - Error (NotFound) when it does not exist
    */
    fn record_mut(
        &mut self,
        capability_id: &str,
        version: u32,
    ) -> CapabilityResult<&mut VersionRecord> {
        self.versions
            .get_mut(capability_id)
            .and_then(|versions| versions.get_mut(&version))
            .ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::NotFound,
                    format!("capability {capability_id} v{version} does not exist"),
                )
            })
    }

    /** The latest non-dry validation run of a version
     * Input
        - record: &VersionRecord - version
     * Output
        - Option<&ValidationRun>
    */
    pub(crate) fn latest_run(&self, record: &VersionRecord) -> Option<&ValidationRun> {
        record
            .validation_runs
            .iter()
            .rev()
            .filter_map(|id| self.runs.get(id))
            .find(|run| !run.dry_run)
    }

    /** Apply a transition and append it to the history
     * Input
        - key: (&str, u32) - capability and version
        - to: LifecycleState - new state
        - actor: &Actor - actor
        - reason: &str - why
        - run: Option<&str> - validation run
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<()>
        - Error (InvalidTransition) when the table forbids it
    */
    fn transition(
        &mut self,
        key: (&str, u32),
        to: LifecycleState,
        actor: &Actor,
        reason: &str,
        run: Option<&str>,
        now: u64,
    ) -> CapabilityResult<()> {
        let (capability_id, version) = key;
        let record = self.record_mut(capability_id, version)?;
        let from = record.state;
        if !lifecycle::allowed(from, to) {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("{from:?} cannot become {to:?}"),
            ));
        }
        record.state = to;
        let sequence = self.transitions.len().to_string();
        self.transitions.push(LifecycleTransition {
            transition_id: stable_id("trn", &[capability_id, &version.to_string(), &sequence]),
            capability_id: capability_id.into(),
            version,
            from: Some(from),
            to,
            actor: actor.clone(),
            reason: reason.into(),
            validation_run: run.map(str::to_string),
            at: now,
        });
        Ok(())
    }
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

/** Bring a stored registry document to the current layout. Layout 0 (before versioning) had no
 * generation and no per-version profile, staleness, withdrawal, or invocation fields: the
 * missing collections start empty and the profile becomes "conservative" (the strictest, so no
 * migrated version gains permissions it did not have). Newer layouts are refused
 * Input
    - value: Value - stored document
 * Output
    - CapabilityResult<RegistryState>
    - Error (UnsupportedSchemaVersion or Persistence) for an unknown layout or an invalid record
*/
pub(crate) fn migrate(mut value: Value) -> CapabilityResult<RegistryState> {
    let layout = value
        .get("schema_version")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if layout > u64::from(REGISTRY_SCHEMA_VERSION) {
        return Err(CapabilityError::new(ErrorCode::UnsupportedSchemaVersion, format!("the capability registry has layout {layout}, newer than this Crane ({REGISTRY_SCHEMA_VERSION}); upgrade Crane")));
    }
    if layout == 0 {
        let object = value
            .as_object_mut()
            .ok_or_else(|| persistence("the registry document is not an object".into()))?;
        object.insert(
            "schema_version".into(),
            Value::from(REGISTRY_SCHEMA_VERSION),
        );
        object.entry("generation").or_insert(Value::from(0));
        object.insert("migrated_from".into(), Value::from(0));
        for key in ["intakes", "idempotency", "versions", "runs"] {
            object
                .entry(key)
                .or_insert(Value::Object(Default::default()));
        }
        object
            .entry("transitions")
            .or_insert(Value::Array(Vec::new()));
        if let Some(Value::Object(capabilities)) = object.get_mut("versions") {
            for versions in capabilities.values_mut().filter_map(Value::as_object_mut) {
                for record in versions.values_mut().filter_map(Value::as_object_mut) {
                    record
                        .entry("profile_id")
                        .or_insert(Value::from("conservative"));
                }
            }
        }
    }
    let state: RegistryState = serde_json::from_value(value)
        .map_err(|error| persistence(format!("the capability registry is invalid: {error}")))?;
    for record in state
        .versions
        .values()
        .flat_map(|versions| versions.values())
    {
        if record.definition.digest() != record.definition_digest {
            return Err(CapabilityError::new(ErrorCode::IntegrityMismatch, format!("{} v{} does not match its stored digest; the registry was edited outside Crane", record.definition.capability_id, record.definition.version)));
        }
    }
    Ok(state)
}

/** Read the registry document from a directory (empty when it does not exist yet)
 * Input
    - directory: &Path - registry directory
 * Output
    - CapabilityResult<RegistryState>
    - Error when the document is unreadable, truncated, or an unsupported layout
*/
fn load(directory: &Path) -> CapabilityResult<RegistryState> {
    match std::fs::read_to_string(directory.join(REGISTRY_FILE)) {
        Ok(text) => {
            let value: Value = serde_json::from_str(&text).map_err(|error| persistence(format!("{} is not valid JSON ({error}); it was not written by Crane's atomic writer — restore it from a backup", directory.join(REGISTRY_FILE).display())))?;
            migrate(value)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(RegistryState {
            schema_version: REGISTRY_SCHEMA_VERSION,
            ..RegistryState::default()
        }),
        Err(error) => Err(persistence(format!(
            "could not read the capability registry: {error}"
        ))),
    }
}

/** Result of an intake submission
 * Fields
    - intake: CapabilityIntake - stored intake
    - duplicate: bool - the same request had been submitted before (nothing new was stored)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IntakeSubmission {
    pub(crate) intake: CapabilityIntake,
    pub(crate) duplicate: bool,
}

/** The registry: its document, where it persists, and the roles it grants
 * Fields
    - directory: Option<PathBuf> - persistence directory, None in memory
    - state: RegistryState - the document as last committed
    - grants: Grants - roles held by actors
*/
#[derive(Debug)]
pub(crate) struct CapabilityRegistry {
    directory: Option<PathBuf>,
    pub(crate) state: RegistryState,
    pub(crate) grants: Grants,
}

impl CapabilityRegistry {
    /** An in-memory registry
     * Input
        - grants: Grants - roles
     * Output
        - CapabilityRegistry
    */
    pub(crate) fn in_memory(grants: Grants) -> Self {
        Self {
            directory: None,
            state: RegistryState {
                schema_version: REGISTRY_SCHEMA_VERSION,
                ..RegistryState::default()
            },
            grants,
        }
    }

    /** Open a persistent registry
     * Input
        - directory: &Path - registry directory
        - grants: Grants - roles
     * Output
        - CapabilityResult<CapabilityRegistry>
        - Error when the stored document cannot be loaded
    */
    pub(crate) fn open(directory: &Path, grants: Grants) -> CapabilityResult<Self> {
        let _lock = Lock::acquire(&directory.join("registry.lock")).map_err(persistence)?;
        Ok(Self {
            directory: Some(directory.to_path_buf()),
            state: load(directory)?,
            grants,
        })
    }

    /** Run a mutation as a transaction: lock, reload, apply to a copy, write atomically, and
     * commit; nothing is written when the mutation fails
     * Input
        - change: impl FnOnce(&mut RegistryState, &Grants) -> CapabilityResult<T> - mutation
     * Output
        - CapabilityResult<T>
        - Error from the mutation, or Persistence when the document cannot be written
    */
    fn transact<T>(
        &mut self,
        change: impl FnOnce(&mut RegistryState, &Grants) -> CapabilityResult<T>,
    ) -> CapabilityResult<T> {
        let _lock = match &self.directory {
            Some(directory) => {
                let lock = Lock::acquire(&directory.join("registry.lock")).map_err(persistence)?;
                self.state = load(directory)?;
                Some(lock)
            }
            None => None,
        };
        let mut next = self.state.clone();
        let result = change(&mut next, &self.grants)?;
        next.generation += 1;
        if let Some(directory) = &self.directory {
            let bytes =
                serde_json::to_vec_pretty(&next).map_err(|error| persistence(error.to_string()))?;
            write_atomic(&directory.join(REGISTRY_FILE), &bytes).map_err(persistence)?;
        }
        self.state = next;
        Ok(result)
    }

    /** Record a refused privileged operation, then return the refusal
     * Input
        - action: &str - operation
        - capability_id: &str - capability
        - version: Option<u32> - version
        - actor: &Actor - actor
        - error: CapabilityError - refusal
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityError (the refusal; recording it is best effort)
    */
    fn refuse(
        &mut self,
        action: &str,
        capability_id: &str,
        version: Option<u32>,
        actor: &Actor,
        error: CapabilityError,
        now: u64,
    ) -> CapabilityError {
        let denial = Denial {
            action: action.into(),
            capability_id: capability_id.into(),
            version,
            actor: actor.clone(),
            reason: error.to_string(),
            at: now,
        };
        let _ = self.transact(|state, _| {
            state.denials.push(denial);
            Ok(())
        });
        error
    }

    /** Submit an intake: authorize, build it from canonical records, apply idempotency, persist
     * Input
        - request: &IntakeRequest - request
        - sources: &Sources - canonical sources and the submitter's access policy
        - actor: &Actor - submitter
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<IntakeSubmission>
        - Error for authorization, schema, reference, integrity, or idempotency conflicts
    */
    pub(crate) fn submit_intake(
        &mut self,
        request: &IntakeRequest,
        sources: &Sources,
        actor: &Actor,
        now: u64,
    ) -> CapabilityResult<IntakeSubmission> {
        self.grants.require(actor, Role::Contributor)?;
        if sources.reader.actor != *actor {
            return Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                "the access policy belongs to another actor",
            ));
        }
        let built = intake::build(request, sources, actor, now)?;
        self.transact(|state, _| {
            if let Some(existing) = state
                .idempotency
                .get(&built.idempotency_key)
                .and_then(|id| state.intakes.get(id))
            {
                if existing.fingerprint == built.fingerprint {
                    return Ok(IntakeSubmission {
                        intake: existing.clone(),
                        duplicate: true,
                    });
                }
                return Err(CapabilityError::new(
                    ErrorCode::IdempotencyConflict,
                    format!(
                        "idempotency key {} was used for different content (intake {})",
                        built.idempotency_key, existing.intake_id
                    ),
                ));
            }
            state
                .idempotency
                .insert(built.idempotency_key.clone(), built.intake_id.clone());
            state.intakes.insert(built.intake_id.clone(), built.clone());
            Ok(IntakeSubmission {
                intake: built,
                duplicate: false,
            })
        })
    }

    /** Store a definition as version 1 of a new capability, in state candidate
     * Input
        - actor: &Actor - author (must match the definition's provenance)
        - definition: CapabilityDefinition - definition
        - profile: Option<&str> - validation profile, None for the default for its risk
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<VersionRecord>
        - Error when unauthorized, the id exists, or the version is not 1
    */
    pub(crate) fn create_candidate(
        &mut self,
        actor: &Actor,
        definition: CapabilityDefinition,
        profile: Option<&str>,
        now: u64,
    ) -> CapabilityResult<VersionRecord> {
        if definition.version != 1 || definition.previous_version_ref.is_some() {
            return Err(CapabilityError::new(ErrorCode::InvalidSchema, "a new capability starts at version 1 without a predecessor; use propose_version for changes"));
        }
        self.store_version(actor, definition, profile, now, "created")
    }

    /** Store a new version of an existing capability (the predecessor is never modified); it starts
     * as a candidate and inherits no validation
     * Input
        - actor: &Actor - author
        - definition: CapabilityDefinition - new definition (version = latest + 1)
        - profile: Option<&str> - validation profile
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<VersionRecord>
        - Error when the capability does not exist, the version or predecessor is wrong, or the
          change reason is missing
    */
    pub(crate) fn propose_version(
        &mut self,
        actor: &Actor,
        definition: CapabilityDefinition,
        profile: Option<&str>,
        now: u64,
    ) -> CapabilityResult<VersionRecord> {
        let latest = self
            .state
            .versions
            .get(&definition.capability_id)
            .and_then(|versions| versions.keys().next_back().copied())
            .ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::NotFound,
                    format!("capability {} does not exist", definition.capability_id),
                )
            })?;
        if definition.version != latest + 1 || definition.previous_version_ref != Some(latest) {
            return Err(CapabilityError::new(
                ErrorCode::InvalidSchema,
                format!(
                    "the next version is {} with predecessor {latest}",
                    latest + 1
                ),
            ));
        }
        if definition
            .provenance
            .change_reason
            .as_deref()
            .is_none_or(|reason| reason.trim().is_empty())
        {
            return Err(CapabilityError::new(
                ErrorCode::InvalidSchema,
                "a new version needs a change reason",
            ));
        }
        self.store_version(actor, definition, profile, now, "proposed as a new version")
    }

    /** Store a version as a candidate
     * Input
        - actor: &Actor - author
        - definition: CapabilityDefinition - definition
        - profile: Option<&str> - validation profile
        - now: u64 - Unix milliseconds
        - reason: &str - audit reason
     * Output
        - CapabilityResult<VersionRecord>
        - Error when unauthorized or the version exists
    */
    fn store_version(
        &mut self,
        actor: &Actor,
        definition: CapabilityDefinition,
        profile: Option<&str>,
        now: u64,
        reason: &str,
    ) -> CapabilityResult<VersionRecord> {
        self.grants.require(actor, Role::Contributor)?;
        if definition.provenance.created_by != *actor {
            return Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                "the definition's author must be the submitting actor",
            ));
        }
        let profile = select_profile(profile, definition.applicability.risk)?;
        let record = VersionRecord {
            definition_digest: definition.digest(),
            profile_id: profile.profile_id,
            state: LifecycleState::Candidate,
            created_at: now,
            validation_runs: Vec::new(),
            stale: Vec::new(),
            superseded_by: None,
            withdrawals: Vec::new(),
            invocations: Vec::new(),
            definition,
        };
        self.transact(|state, _| {
            let id = record.definition.capability_id.clone();
            let version = record.definition.version;
            let versions = state.versions.entry(id.clone()).or_default();
            if versions.contains_key(&version) {
                return Err(CapabilityError::new(
                    ErrorCode::IdempotencyConflict,
                    format!("{id} v{version} already exists; stored versions are immutable"),
                ));
            }
            versions.insert(version, record.clone());
            let sequence = state.transitions.len().to_string();
            state.transitions.push(LifecycleTransition {
                transition_id: stable_id("trn", &[&id, &version.to_string(), &sequence]),
                capability_id: id,
                version,
                from: None,
                to: LifecycleState::Candidate,
                actor: actor.clone(),
                reason: reason.into(),
                validation_run: None,
                at: now,
            });
            Ok(record)
        })
    }

    /** Validate a version through gates 1-5. A dry run changes nothing and may be run by any
     * contributor; a real run needs the Validator role, moves the version through validating to
     * validated, needs_evidence, or candidate, records the run, and clears staleness on success
     * Input
        - actor: &Actor - validator
        - capability_id: &str - capability
        - version: u32 - version
        - sources: &Sources - canonical sources
        - dry_run: bool - only report
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<ValidationRun>
        - Error when unauthorized, unknown, or the state cannot be validated
    */
    pub(crate) fn validate(
        &mut self,
        actor: &Actor,
        capability_id: &str,
        version: u32,
        sources: &Sources,
        dry_run: bool,
        now: u64,
    ) -> CapabilityResult<ValidationRun> {
        let role = if dry_run {
            Role::Contributor
        } else {
            lifecycle::role_for(LifecycleState::Validating)
        };
        self.grants.require(actor, role)?;
        let record = self.state.record(capability_id, version)?;
        let profile = select_profile(
            Some(&record.profile_id),
            record.definition.applicability.risk,
        )?;
        if dry_run {
            return Ok(validate::run(
                &record.definition,
                &profile,
                &self.state,
                sources,
                actor,
                true,
                now,
            ));
        }
        self.transact(|state, _| {
            let record = state.record(capability_id, version)?.clone();
            let run = validate::run(
                &record.definition,
                &profile,
                state,
                sources,
                actor,
                false,
                now,
            );
            if record.state == LifecycleState::Published {
                // Revalidating a published version never unpublishes it; a pass clears staleness
                let stored = state.record_mut(capability_id, version)?;
                stored.validation_runs.push(run.run_id.clone());
                if run.outcome == validate::RunOutcome::Passed {
                    stored.stale.clear();
                }
                state.runs.insert(run.run_id.clone(), run.clone());
                return Ok(run);
            }
            state.transition(
                (capability_id, version),
                LifecycleState::Validating,
                actor,
                "validation started",
                Some(&run.run_id),
                now,
            )?;
            let to = state_after(run.outcome);
            state.transition(
                (capability_id, version),
                to,
                actor,
                &format!("validation {:?}", run.outcome),
                Some(&run.run_id),
                now,
            )?;
            let stored = state.record_mut(capability_id, version)?;
            stored.validation_runs.push(run.run_id.clone());
            if to == LifecycleState::Validated {
                stored.stale.clear();
            }
            state.runs.insert(run.run_id.clone(), run.clone());
            Ok(run)
        })
    }

    /** Check publication eligibility (gate 6) without publishing
     * Input
        - actor: &Actor - prospective publisher
        - request: &PublicationRequest - request
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<GateResult>
        - Error when the version does not exist
    */
    pub(crate) fn eligibility(
        &self,
        actor: &Actor,
        request: &PublicationRequest,
        now: u64,
    ) -> CapabilityResult<GateResult> {
        let record = self.state.record(&request.capability_id, request.version)?;
        let profile = select_profile(
            Some(&record.profile_id),
            record.definition.applicability.risk,
        )?;
        let eligibility = Eligibility {
            state: record.state,
            author: &record.definition.provenance.created_by,
            definition_digest: &record.definition_digest,
            run: self.state.latest_run(record),
            stale: !record.stale.is_empty(),
            profile: &profile,
        };
        Ok(publication_gate(
            &eligibility,
            request,
            actor,
            self.grants.require(actor, Role::Publisher),
            now,
        ))
    }

    /** Publish a version after gate 6 passes; a refusal is recorded for audit
     * Input
        - actor: &Actor - publisher
        - request: &PublicationRequest - request
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<GateResult>
        - Error (PublicationDenied with every reason, or NotFound)
    */
    pub(crate) fn publish(
        &mut self,
        actor: &Actor,
        request: &PublicationRequest,
        now: u64,
    ) -> CapabilityResult<GateResult> {
        let gate = self.eligibility(actor, request, now)?;
        if gate.status != GateStatus::Passed {
            let reasons = gate
                .findings
                .iter()
                .map(|finding| finding.message.clone())
                .collect::<Vec<_>>()
                .join("; ");
            let error = CapabilityError::new(ErrorCode::PublicationDenied, reasons);
            return Err(self.refuse(
                "publish",
                &request.capability_id,
                Some(request.version),
                actor,
                error,
                now,
            ));
        }
        let reason = format!("published: {}", request.reason);
        let id = request.capability_id.clone();
        let version = request.version;
        self.transact(|state, _| {
            state.transition(
                (&id, version),
                LifecycleState::Published,
                actor,
                &reason,
                None,
                now,
            )?;
            if let Some(versions) = state.versions.get_mut(&id) {
                for (_, older) in versions.range_mut(..version) {
                    if older.state == LifecycleState::Published && older.superseded_by.is_none() {
                        older.superseded_by = Some(version);
                    }
                }
            }
            Ok(())
        })?;
        Ok(gate)
    }

    /** Deprecate, revoke, or reject one version, or every applicable version of a capability
         * Input
            - actor: &Actor - publisher
            - capability_id: &str - capability
            - version: Option<u32> - version, None for every version the transition applies to
            - to: LifecycleState - Deprecated, Revoked, or Rejected
            - reason: &str - why (required)
            - now: u64 - Unix milliseconds
         * Output
            - CapabilityResult<Vec<u32>> versions changed
            - Error when unauthorized, unknown, without a reason, or no version can make the transition
    */
    pub(crate) fn withdraw(
        &mut self,
        actor: &Actor,
        capability_id: &str,
        version: Option<u32>,
        to: LifecycleState,
        reason: &str,
        now: u64,
    ) -> CapabilityResult<Vec<u32>> {
        let action = format!("{to:?}").to_ascii_lowercase();
        if !matches!(
            to,
            LifecycleState::Deprecated | LifecycleState::Revoked | LifecycleState::Rejected
        ) {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("withdrawal cannot lead to {to:?}"),
            ));
        }
        if let Err(error) = self.grants.require(actor, lifecycle::role_for(to)) {
            return Err(self.refuse(&action, capability_id, version, actor, error, now));
        }
        if reason.trim().is_empty() {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                "a withdrawal needs a reason",
            ));
        }
        self.transact(|state, _| {
            let versions = state.versions.get(capability_id).ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::NotFound,
                    format!("capability {capability_id} does not exist"),
                )
            })?;
            let targets = match version {
                Some(version) => vec![version],
                None => versions
                    .iter()
                    .filter(|(_, record)| lifecycle::allowed(record.state, to))
                    .map(|(version, _)| *version)
                    .collect(),
            };
            if targets.is_empty() {
                return Err(CapabilityError::new(
                    ErrorCode::InvalidTransition,
                    format!("no version of {capability_id} can become {to:?}"),
                ));
            }
            for target in &targets {
                state.transition((capability_id, *target), to, actor, reason, None, now)?;
                state
                    .record_mut(capability_id, *target)?
                    .withdrawals
                    .push(Withdrawal {
                        state: to,
                        reason: reason.into(),
                        actor: actor.clone(),
                        at: now,
                    });
            }
            Ok(targets)
        })
    }

    /** Record a use of a version; it never changes the definition or the lifecycle state
     * Input
        - actor: &Actor - recorder
        - capability_id: &str - capability
        - version: u32 - version
        - invocation: InvocationRef - the use
     * Output
        - CapabilityResult<()>
        - Error when unauthorized or unknown
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for future invocation history; exercised by the acceptance tests
    pub(crate) fn record_invocation(
        &mut self,
        actor: &Actor,
        capability_id: &str,
        version: u32,
        invocation: InvocationRef,
    ) -> CapabilityResult<()> {
        self.grants.require(actor, Role::Contributor)?;
        self.transact(|state, _| {
            state
                .record_mut(capability_id, version)?
                .invocations
                .push(invocation);
            Ok(())
        })
    }

    /** Compare every version's latest validation basis with the current references and mark the
     * runs that no longer match (the runs themselves are never rewritten)
     * Input
        - actor: &Actor - validator
        - sources: &Sources - canonical sources and current versions
        - now: u64 - Unix milliseconds
     * Output
        - CapabilityResult<Vec<(String, u32, Vec<String>)>> versions newly marked, with reasons
        - Error when unauthorized
    */
    pub(crate) fn mark_stale(
        &mut self,
        actor: &Actor,
        sources: &Sources,
        now: u64,
    ) -> CapabilityResult<Vec<(String, u32, Vec<String>)>> {
        self.grants.require(actor, Role::Validator)?;
        self.transact(|state, _| {
            let mut marked = Vec::new();
            let mut notes = Vec::new();
            for record in state
                .versions
                .values()
                .flat_map(|versions| versions.values())
            {
                if !matches!(
                    record.state,
                    LifecycleState::Validated | LifecycleState::Published
                ) {
                    continue;
                }
                let Some(run) = state.latest_run(record) else {
                    continue;
                };
                let reasons = stale_reasons(run, state, sources);
                let already = record
                    .stale
                    .iter()
                    .any(|note| note.run_id == run.run_id && note.reasons == reasons);
                if !reasons.is_empty() && !already {
                    notes.push((
                        record.definition.capability_id.clone(),
                        record.definition.version,
                        StaleNote {
                            run_id: run.run_id.clone(),
                            reasons: reasons.clone(),
                            detected_at: now,
                        },
                    ));
                    marked.push((
                        record.definition.capability_id.clone(),
                        record.definition.version,
                        reasons,
                    ));
                }
            }
            for (id, version, note) in notes {
                state.record_mut(&id, version)?.stale.push(note);
            }
            Ok(marked)
        })
    }
}

/** Why a validation run no longer matches the current references
 * Input
    - run: &ValidationRun - run
    - state: &RegistryState - registry
    - sources: &Sources - canonical sources
 * Output
    - Vec<String> reasons, empty when current
*/
pub(crate) fn stale_reasons(
    run: &ValidationRun,
    state: &RegistryState,
    sources: &Sources,
) -> Vec<String> {
    let mut reasons = Vec::new();
    for pack in &run.basis.packs {
        match sources.capture.packs.get(&pack.pack_id) {
            Some(found) if found.manifest.digest == pack.manifest_digest => {}
            _ => reasons.push(format!(
                "Evidence Pack {} changed or is missing",
                pack.pack_id
            )),
        }
    }
    for (key, version) in &run.basis.governance {
        let (kind, id) = key.split_once(':').unwrap_or(("policy", key));
        let check = sources.current.check(kind, id, Some(version));
        if check.outcome != RefOutcome::Resolved {
            reasons.push(format!(
                "{kind} {id}@{version} is {:?}: {}",
                check.outcome, check.detail
            ));
        }
    }
    for (id, version) in &run.basis.capabilities {
        let usable = version
            .parse::<u32>()
            .ok()
            .and_then(|version| {
                state
                    .versions
                    .get(id)
                    .and_then(|versions| versions.get(&version))
            })
            .is_some_and(|record| {
                record.state.selectable_for_dependencies()
                    && VersionConstraint::Exact(version.clone())
                        .admits(&record.definition.version.to_string())
            });
        if !usable {
            reasons.push(format!(
                "dependency capability {id} v{version} is no longer usable"
            ));
        }
    }
    reasons
}
