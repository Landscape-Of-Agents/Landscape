// The invocation engine. submit() authorizes the requester (registry roles held by Crane; the
// Work Capture access policy must be the requester's own) before retrieving anything, applies
// idempotency, and walks requested -> authorizing -> preflighting -> plan_ready (or blocked), then
// awaiting_approval when the plan needs it. approve() binds an approval to the exact plan digest
// and scope; an approver must be another person of the required review level. run() repeats the
// plan build and preflight immediately before execution (a changed plan invalidates its approval)
// and executes. Before each operation it rechecks the requester's role, the deadline, the adapter,
// the operation's schema, preconditions, transaction availability, and, before an externally
// consequential operation, staleness. Dispatch intent is persisted before any state-changing
// dispatch. A known failure fails; a timeout before dispatch, or of a read-only operation, may be
// retried within the limit; any other uncertain effect requires reconciliation against the
// authoritative system, never a blind retry. reconcile() resumes execution, proceeds to
// verification, or leaves the invocation unknown for an explicit decision; abandon() records the
// remaining effects. Verification runs every bound check (fresh adapter reads that are not the
// plan's own operations, or Work Capture evidence read through the requester's access) and only
// supported postconditions count: missing or contradictory evidence never becomes success. Every
// transition is validated, persisted, and emitted as an event; the final outcome is handed to
// Component 2's invocation history, which never changes the definition or its lifecycle.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::adapter::{
    validate_params, AdapterRegistry, Compensation, DispatchOutcome, Reconciliation,
    TransactionRuntime,
};
use super::ledger::{Insertion, Ledger, LedgerDenial};
use super::lifecycle::{allowed, EffectClass, EffectStatus, InvocationState, OperationStatus};
use super::plan::{
    self, ApprovalRequirement, BindingSet, InvocationPlan, PlanLimits, VerificationMethod,
};
use super::preflight::{self, PreflightCheck, PreflightInputs, PreflightStatus, Target};
use super::{ENGINE_VERSION, RECORD_SCHEMA_VERSION};
use crate::capability::model::{CapabilityDefinition, ReviewLevel, RiskLevel};
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::{
    stale_reasons, CapabilityRegistry, InvocationRef, InvocationStatus,
};
use crate::capability::shared::SharedView;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode, Role};
use crate::capability_lab::report::LabReport;
use crate::capture::format::record::scalar;
use crate::capture::{stable_id, Actor, ActorKind, Clock};
use crate::compiler::assemble::CompilationReport;
use crate::discovery::report::DiscoveryReport;
use crate::telemetry::events::{redact_value, Event, EventStore};
use crate::telemetry::model::{Domain, Execution, Severity};
use crate::telemetry::redact::redact;
use crate::trust::crypto::digest_json;

/** An invocation request
 * Fields
    - capability_id, version, digest - exact capability
    - task_id, episode_id: Option<String> - work references
    - environment, repository: Option<String> - target environment and repository
    - confirmed_preconditions: Vec<String> - preconditions the requester confirms
    - inputs: BTreeMap<String, String> - inputs (credential references only; never secret values)
    - bindings: BindingSet - reviewed step and verification bindings
    - discovery_report: Option<String> - advisory discovery report id
    - compilation_report: Option<String> - compilation report id
    - idempotency_key: String - idempotency key
    - execute: bool - execute when no approval is needed (false: plan only)
    - limits: PlanLimits - limits
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvocationRequest {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) task_id: Option<String>,
    pub(crate) episode_id: Option<String>,
    pub(crate) environment: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) confirmed_preconditions: Vec<String>,
    pub(crate) inputs: BTreeMap<String, String>,
    pub(crate) bindings: BindingSet,
    pub(crate) discovery_report: Option<String>,
    pub(crate) compilation_report: Option<String>,
    pub(crate) idempotency_key: String,
    pub(crate) execute: bool,
    pub(crate) limits: PlanLimits,
}

impl InvocationRequest {
    /** The canonical digest of the request's content
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json("invocation/request/v1", &json!(self))[..32].to_string()
    }
}

/** One reconciliation attempt
 * Fields
    - at: u64 - Unix milliseconds
    - outcome: String - applied, not applied, or unavailable (with the reason)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReconciliationRecord {
    pub(crate) at: u64,
    pub(crate) outcome: String,
}

/** One operation's execution record
 * Fields
    - index: usize - plan position
    - step_id, adapter_id, operation_id - identity
    - effect: EffectClass - effect class
    - status: OperationStatus - status
    - effect_status: EffectStatus - what is known about its external effect
    - attempts: u32 - dispatches
    - started_at, finished_at: Option<u64> - Unix milliseconds
    - external_ref: Option<String> - external operation reference
    - result: Value - normalized, redacted, bounded result
    - error: Option<String> - error
    - reconciliations: Vec<ReconciliationRecord> - reconciliation attempts
    - compensation: Option<Compensation> - compensation reported by the transaction runtime
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OperationRecord {
    pub(crate) index: usize,
    pub(crate) step_id: String,
    pub(crate) adapter_id: String,
    pub(crate) operation_id: String,
    pub(crate) effect: EffectClass,
    pub(crate) status: OperationStatus,
    pub(crate) effect_status: EffectStatus,
    pub(crate) attempts: u32,
    pub(crate) started_at: Option<u64>,
    pub(crate) finished_at: Option<u64>,
    pub(crate) external_ref: Option<String>,
    pub(crate) result: Value,
    pub(crate) error: Option<String>,
    pub(crate) reconciliations: Vec<ReconciliationRecord>,
    pub(crate) compensation: Option<Compensation>,
}

/** The result of one postcondition check
 * Variants
    - Verified - the evidence supports it
    - Failed - the evidence shows it does not hold
    - Unresolved - no or insufficient evidence
    - Contradicted - checks disagree; no side is chosen
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationStatus {
    Verified,
    Failed,
    Unresolved,
    Contradicted,
}

/** One verification result
 * Fields
    - claim_id: String - claim
    - requirement: String - what must hold
    - required: bool - whether the claim is required
    - method: String - method used
    - evidence_refs: Vec<String> - evidence and results relied on
    - observed: Option<String> - value observed
    - result: VerificationStatus - result
    - limitations: Vec<String> - limitations
    - candidate_digest: String - definition digest
    - environment: Option<String> - environment
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VerificationResult {
    pub(crate) claim_id: String,
    pub(crate) requirement: String,
    pub(crate) required: bool,
    pub(crate) method: String,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) observed: Option<String>,
    pub(crate) result: VerificationStatus,
    pub(crate) limitations: Vec<String>,
    pub(crate) candidate_digest: String,
    pub(crate) environment: Option<String>,
}

/** An approval bound to a plan digest and scope
 * Fields
    - approver: Actor - approver
    - plan_digest: String - plan approved
    - environment, repository: Option<String> - scope approved
    - at: u64 - Unix milliseconds
    - revoked: Option<String> - why it was revoked before it was used (absent in records written
      before revocation existed, so their digests still verify)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Approval {
    pub(crate) approver: Actor,
    pub(crate) plan_digest: String,
    pub(crate) environment: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) revoked: Option<String>,
}

/** A recorded transition
 * Fields
    - sequence: u64 - event sequence within the invocation
    - from: Option<InvocationState> - previous state
    - to: InvocationState - new state
    - actor: Actor - who caused it
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TransitionRecord {
    pub(crate) sequence: u64,
    pub(crate) from: Option<InvocationState>,
    pub(crate) to: InvocationState,
    pub(crate) actor: Actor,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** An invocation record
 * Fields
    - invocation_id: String - identifier
    - schema_version: u32 - RECORD_SCHEMA_VERSION
    - engine_version: String - ENGINE_VERSION
    - idempotency_key: String - idempotency key
    - request_digest: String - canonical request digest
    - request: InvocationRequest - request
    - requester: Actor - requester
    - authorization: String - authorization decision
    - capability_id, version, digest - exact capability
    - compilation_report: Option<String> - compilation report used
    - plan: Option<InvocationPlan> - plan
    - approval: ApprovalRequirement - approval required (plan and preflight)
    - governance: CurrentVersions - Landscape and Crane versions at preflight
    - preflight: Vec<PreflightCheck> - preflight results (latest run)
    - approvals: Vec<Approval> - approvals
    - operations: Vec<OperationRecord> - operations
    - verifications: Vec<VerificationResult> - verification results
    - state: InvocationState - state
    - transitions: Vec<TransitionRecord> - transitions
    - unresolved_effects: Vec<String> - effects that are not established
    - failures: Vec<String> - failures and block reasons
    - recovery_status: String - recovery status
    - created_at, updated_at: u64 - Unix milliseconds
    - record_digest: String - integrity digest of the record without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvocationRecord {
    pub(crate) invocation_id: String,
    pub(crate) schema_version: u32,
    pub(crate) engine_version: String,
    pub(crate) idempotency_key: String,
    pub(crate) request_digest: String,
    pub(crate) request: InvocationRequest,
    pub(crate) requester: Actor,
    pub(crate) authorization: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) compilation_report: Option<String>,
    pub(crate) plan: Option<InvocationPlan>,
    pub(crate) approval: ApprovalRequirement,
    pub(crate) governance: CurrentVersions,
    pub(crate) preflight: Vec<PreflightCheck>,
    pub(crate) approvals: Vec<Approval>,
    pub(crate) operations: Vec<OperationRecord>,
    pub(crate) verifications: Vec<VerificationResult>,
    pub(crate) state: InvocationState,
    pub(crate) transitions: Vec<TransitionRecord>,
    pub(crate) unresolved_effects: Vec<String>,
    pub(crate) failures: Vec<String>,
    pub(crate) recovery_status: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) record_digest: String,
}

impl InvocationRecord {
    /** The digest of the record's content
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn compute_digest(&self) -> String {
        let mut content = self.clone();
        content.record_digest = String::new();
        digest_json("invocation/record/v1", &json!(content))[..32].to_string()
    }

    /** Seal the record with its digest
     * Input
        - None (uses self)
     * Output
        - None
    */
    pub(crate) fn seal(&mut self) {
        self.record_digest = self.compute_digest();
    }

    /** Check that the record still matches its digest
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verify_digest(&self) -> bool {
        self.compute_digest() == self.record_digest
    }

    /** Apply a validated transition (a refused one changes nothing)
     * Input
        - to: InvocationState - new state
        - actor: &Actor - who
        - reason: &str - why
        - at: u64 - Unix milliseconds
     * Output
        - CapabilityResult<TransitionRecord>
        - Error (InvalidTransition) when the table forbids it
    */
    pub(crate) fn transition(
        &mut self,
        to: InvocationState,
        actor: &Actor,
        reason: &str,
        at: u64,
    ) -> CapabilityResult<TransitionRecord> {
        if !allowed(self.state, to) {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("an invocation cannot go from {:?} to {to:?}", self.state),
            ));
        }
        let record = TransitionRecord {
            sequence: self.transitions.len() as u64 + 1,
            from: Some(self.state),
            to,
            actor: actor.clone(),
            reason: reason.into(),
            at,
        };
        self.state = to;
        self.updated_at = at;
        self.transitions.push(record.clone());
        Ok(record)
    }
}

/** Where lifecycle events go */
pub(crate) trait EventSink {
    /** Emit one transition
     * Input
        - record: &InvocationRecord - record after the transition
        - transition: &TransitionRecord - transition
     * Output
        - None
    */
    fn emit(&mut self, record: &InvocationRecord, transition: &TransitionRecord);
}

/** Events kept in memory (fixtures and tests)
 * Fields
    - events: Vec<String> - "INVOCATION:STATE" lines
*/
#[derive(Debug, Default)]
pub(crate) struct MemorySink {
    pub(crate) events: Vec<String>,
}

impl EventSink for MemorySink {
    /** Keep the event
     * Input
        - record: &InvocationRecord - record
        - transition: &TransitionRecord - transition
     * Output
        - None
    */
    fn emit(&mut self, record: &InvocationRecord, transition: &TransitionRecord) {
        self.events
            .push(format!("{}:{:?}", record.invocation_id, transition.to));
    }
}

/** Events appended to DataSea's event store with its existing conventions (redaction,
 * idempotency, hash chain)
 * Fields
    - store: EventStore - the repository's event store
    - repository_id: String - repository identity
*/
pub(crate) struct DataSeaSink {
    pub(crate) store: EventStore,
    pub(crate) repository_id: String,
}

impl EventSink for DataSeaSink {
    /** Append a "capability.invocation.STATE" event (best effort; the ledger is authoritative)
     * Input
        - record: &InvocationRecord - record
        - transition: &TransitionRecord - transition
     * Output
        - None
    */
    fn emit(&mut self, record: &InvocationRecord, transition: &TransitionRecord) {
        let state = serde_json::to_value(transition.to)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        let mut event = Event::new(
            &format!("capability.invocation.{state}"),
            Domain::AuditCompliance,
            &self.repository_id,
            &format!(
                "invocation:{}:{}",
                record.invocation_id, transition.sequence
            ),
        );
        event.occurred_at = transition.at;
        event.execution = Some(match transition.to {
            InvocationState::Succeeded => Execution::Success,
            InvocationState::Failed => Execution::Failure,
            InvocationState::PartiallyVerified => Execution::Partial,
            InvocationState::Blocked => Execution::NotExecuted,
            _ => Execution::Unknown,
        });
        event.severity = match transition.to {
            InvocationState::Failed
            | InvocationState::Unknown
            | InvocationState::ReconciliationRequired => Severity::Medium,
            InvocationState::Blocked
            | InvocationState::Abandoned
            | InvocationState::PartiallyVerified => Severity::Low,
            _ => Severity::Info,
        };
        event.reasons = vec![transition.reason.clone()];
        event.payload = json!({"invocation_id": record.invocation_id, "capability_id": record.capability_id, "version": record.version, "digest": record.digest, "from": transition.from, "to": transition.to, "actor": transition.actor.id});
        let _ = self.store.append(event);
    }
}

/** Everything an invocation needs
 * Fields
    - registry: &'a mut CapabilityRegistry - Component 2 registry
    - sources: Sources<'a> - Work Capture records, current versions, the requester's access
    - lab_reports, discovery_reports, compilation_reports - verified reports
    - adapters: &'a AdapterRegistry - adapters
    - runtime: &'a dyn TransactionRuntime - transaction runtime
    - ledger: &'a mut Ledger - invocation ledger
    - events: &'a mut dyn EventSink - event sink
    - clock: &'a Clock - clock
    - shared: &'a SharedView - the verified shared skill store
*/
pub(crate) struct Engine<'a> {
    pub(crate) registry: &'a mut CapabilityRegistry,
    pub(crate) sources: Sources<'a>,
    pub(crate) lab_reports: &'a [LabReport],
    pub(crate) discovery_reports: &'a [DiscoveryReport],
    pub(crate) compilation_reports: &'a [CompilationReport],
    pub(crate) adapters: &'a AdapterRegistry,
    pub(crate) runtime: &'a dyn TransactionRuntime,
    pub(crate) ledger: &'a mut Ledger,
    pub(crate) events: &'a mut dyn EventSink,
    pub(crate) clock: &'a Clock,
    pub(crate) shared: &'a SharedView,
}

/** Check requested limits against the engine's hard ceiling (64 operations, 1 MiB of output, 10
 * minutes, and two retries above the default); signed policy may narrow them, never exceed them
 * Input
    - limits: &PlanLimits - requested limits
 * Output
    - Result<(), String> - the limit that exceeds the ceiling
*/
pub(crate) fn within_ceiling(limits: &PlanLimits) -> Result<(), String> {
    let ceiling = PlanLimits::default();
    if limits.max_operations > 64 {
        return Err(format!(
            "max_operations {} exceeds the engine's ceiling of 64",
            limits.max_operations
        ));
    }
    if limits.max_output_bytes > 1024 * 1024 {
        return Err(format!(
            "max_output_bytes {} exceeds the engine's ceiling of 1 MiB",
            limits.max_output_bytes
        ));
    }
    if limits.deadline_ms > 600_000 {
        return Err(format!(
            "deadline_ms {} exceeds the engine's ceiling of 600000",
            limits.deadline_ms
        ));
    }
    if limits.max_retries > ceiling.max_retries + 2 {
        return Err(format!(
            "max_retries {} exceeds the engine's ceiling of {}",
            limits.max_retries,
            ceiling.max_retries + 2
        ));
    }
    Ok(())
}

/** Read a dotted field of a JSON value as text
 * Input
    - value: &Value - value
    - path: &str - dotted path
 * Output
    - Option<String>
*/
fn field(value: &Value, path: &str) -> Option<String> {
    let mut node = value;
    for part in path.split('.') {
        node = node.get(part)?;
    }
    scalar(node)
}

impl Engine<'_> {
    /** Record a refusal and return it
     * Input
        - action: &str - action
        - invocation_id: Option<&str> - invocation
        - actor: &Actor - actor
        - error: CapabilityError - refusal
     * Output
        - CapabilityError
    */
    fn refuse(
        &mut self,
        action: &str,
        invocation_id: Option<&str>,
        actor: &Actor,
        error: CapabilityError,
    ) -> CapabilityError {
        let at = self.clock.now();
        self.ledger.deny(LedgerDenial {
            action: action.into(),
            invocation_id: invocation_id.map(str::to_string),
            actor: actor.clone(),
            reason: error.to_string(),
            at,
        });
        error
    }

    /** Apply a transition, persist the record, and emit the event
     * Input
        - record: &mut InvocationRecord - record
        - to: InvocationState - new state
        - actor: &Actor - actor
        - reason: &str - why
     * Output
        - CapabilityResult<()>
    */
    fn commit(
        &mut self,
        record: &mut InvocationRecord,
        to: InvocationState,
        actor: &Actor,
        reason: &str,
    ) -> CapabilityResult<()> {
        let at = self.clock.now();
        let transition = record.transition(to, actor, reason, at)?;
        if to == InvocationState::Blocked {
            record.failures.push(reason.into());
        }
        self.persist(record)?;
        self.events.emit(record, &transition);
        Ok(())
    }

    /** Seal and persist a record
     * Input
        - record: &mut InvocationRecord - record
     * Output
        - CapabilityResult<()>
    */
    fn persist(&mut self, record: &mut InvocationRecord) -> CapabilityResult<()> {
        record.seal();
        self.ledger.save(record)
    }

    /** The compilation report a request names, or the one for the exact digest
     * Input
        - request: &InvocationRequest - request
     * Output
        - Option<&CompilationReport>
    */
    fn compilation(&self, request: &InvocationRequest) -> Option<&CompilationReport> {
        self.compilation_reports
            .iter()
            .rev()
            .find(|report| match &request.compilation_report {
                Some(id) => &report.report_id == id,
                None => report.candidate.as_ref().is_some_and(|candidate| {
                    candidate.capability_id == request.capability_id
                        && candidate.version == request.version
                        && candidate.digest == request.digest
                }),
            })
    }

    /** Build the plan and run the preflight against the current state
     * Input
        - request: &InvocationRequest - request
        - requester: &Actor - requester
     * Output
        - (Vec<PreflightCheck>, Option<CapabilityDefinition>, Result<InvocationPlan, Vec<String>>)
    */
    fn evaluate(
        &self,
        request: &InvocationRequest,
        requester: &Actor,
    ) -> (
        Vec<PreflightCheck>,
        Option<CapabilityDefinition>,
        Result<InvocationPlan, Vec<String>>,
    ) {
        let inputs = PreflightInputs {
            state: &self.registry.state,
            sources: &self.sources,
            lab_reports: self.lab_reports,
            discovery_reports: self.discovery_reports,
            adapters: self.adapters,
            runtime: self.runtime,
            shared: self.shared,
        };
        let target = Target {
            capability_id: &request.capability_id,
            version: request.version,
            digest: &request.digest,
            environment: request.environment.clone(),
            confirmed_preconditions: &request.confirmed_preconditions,
            discovery_report: request.discovery_report.as_deref(),
        };
        let (identity, definition) = preflight::identity(&inputs, &target);
        let mut checks = vec![identity];
        let Some(definition) = definition else {
            return (
                checks,
                None,
                Err(vec![
                    "the exact capability version could not be established".into(),
                ]),
            );
        };
        let plan = plan::build(
            &definition,
            self.compilation(request),
            request,
            self.adapters,
            &self.sources,
        );
        if let Ok(plan) = &plan {
            checks.extend(preflight::run(
                &inputs,
                &target,
                &definition,
                plan,
                requester,
            ));
        }
        (checks, Some(definition), plan)
    }

    /** Submit an invocation request
     * Input
        - actor: &Actor - requester
        - request: InvocationRequest - request
     * Output
        - CapabilityResult<InvocationRecord> - the new or existing invocation
        - Error for unauthorized requesters, invalid requests, idempotency conflicts, persistence
    */
    pub(crate) fn submit(
        &mut self,
        actor: &Actor,
        request: InvocationRequest,
    ) -> CapabilityResult<InvocationRecord> {
        if let Err(error) = self.registry.grants.require(actor, Role::Contributor) {
            return Err(self.refuse("submit", None, actor, error));
        }
        if self.sources.reader.actor != *actor {
            let error = CapabilityError::new(
                ErrorCode::Unauthorized,
                "the access policy belongs to another actor",
            );
            return Err(self.refuse("submit", None, actor, error));
        }
        let key_valid = !request.idempotency_key.is_empty()
            && request.idempotency_key.len() <= 128
            && request
                .idempotency_key
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "._:-".contains(character));
        if !key_valid {
            return Err(CapabilityError::new(
                ErrorCode::InvalidSchema,
                "the idempotency key must be 1-128 letters, digits, '.', '_', ':' or '-'",
            ));
        }
        if let Some((name, _)) = request.inputs.iter().find(|(_, value)| redact(value).1 > 0) {
            return Err(CapabilityError::new(ErrorCode::InvalidSchema, format!("input '{name}' holds a secret; pass a credential reference, never a secret value")));
        }
        if let Err(problem) = within_ceiling(&request.limits) {
            return Err(CapabilityError::new(ErrorCode::InvalidSchema, problem));
        }
        let now = self.clock.now();
        let request_digest = request.digest();
        let mut record = InvocationRecord {
            invocation_id: stable_id("inv", &[&request.idempotency_key, &request_digest]),
            schema_version: RECORD_SCHEMA_VERSION,
            engine_version: ENGINE_VERSION.into(),
            idempotency_key: request.idempotency_key.clone(),
            request_digest,
            requester: actor.clone(),
            authorization: format!(
                "{} holds the contributor role; the evidence access policy is the requester's own",
                actor.id
            ),
            capability_id: request.capability_id.clone(),
            version: request.version,
            digest: request.digest.clone(),
            compilation_report: None,
            plan: None,
            approval: ApprovalRequirement {
                required: false,
                level: ReviewLevel::None,
                reasons: Vec::new(),
            },
            governance: self.sources.current.clone(),
            preflight: Vec::new(),
            approvals: Vec::new(),
            operations: Vec::new(),
            verifications: Vec::new(),
            state: InvocationState::Requested,
            transitions: vec![TransitionRecord {
                sequence: 1,
                from: None,
                to: InvocationState::Requested,
                actor: actor.clone(),
                reason: "requested".into(),
                at: now,
            }],
            unresolved_effects: Vec::new(),
            failures: Vec::new(),
            recovery_status: "not needed".into(),
            created_at: now,
            updated_at: now,
            record_digest: String::new(),
            request,
        };
        record.seal();
        let mut record = match self.ledger.insert(record)? {
            Insertion::Existing(existing) => return Ok(*existing),
            Insertion::Inserted(record) => *record,
        };
        let first = record.transitions[0].clone();
        self.events.emit(&record, &first);
        let reason = record.authorization.clone();
        self.commit(&mut record, InvocationState::Authorizing, actor, &reason)?;
        self.commit(
            &mut record,
            InvocationState::Preflighting,
            actor,
            "authorized",
        )?;
        let (checks, definition, plan) = self.evaluate(&record.request, actor);
        record.preflight = checks;
        record.compilation_report = self
            .compilation(&record.request)
            .map(|report| report.report_id.clone());
        let plan = match plan {
            Ok(plan) => plan,
            Err(problems) => {
                record.failures.extend(problems);
                self.commit(
                    &mut record,
                    InvocationState::Blocked,
                    actor,
                    "the plan could not be built",
                )?;
                return Ok(record);
            }
        };
        let blocking = record
            .preflight
            .iter()
            .filter(|check| check.blocks())
            .map(|check| format!("{}: {}", check.check_id, check.detail))
            .collect::<Vec<_>>();
        let mut approval = plan.approval.clone();
        let lab_unresolved = record
            .preflight
            .iter()
            .any(|check| check.check_id == "lab" && check.status == PreflightStatus::Unresolved);
        if lab_unresolved
            && definition
                .as_ref()
                .is_some_and(|definition| definition.applicability.risk >= RiskLevel::Medium)
        {
            approval.required = true;
            approval
                .reasons
                .push("no passing lab report for this digest".into());
        }
        record.approval = approval;
        record.operations = plan
            .operations
            .iter()
            .map(|operation| OperationRecord {
                index: operation.index,
                step_id: operation.step_id.clone(),
                adapter_id: operation.adapter_id.clone(),
                operation_id: operation.operation_id.clone(),
                effect: operation.effect,
                status: OperationStatus::Pending,
                effect_status: if operation.effect == EffectClass::ReadOnly {
                    EffectStatus::None
                } else {
                    EffectStatus::NotApplied
                },
                attempts: 0,
                started_at: None,
                finished_at: None,
                external_ref: None,
                result: Value::Null,
                error: None,
                reconciliations: Vec::new(),
                compensation: None,
            })
            .collect();
        record.plan = Some(plan);
        if !blocking.is_empty() {
            record.failures.extend(blocking);
            self.commit(
                &mut record,
                InvocationState::Blocked,
                actor,
                "preflight failed",
            )?;
            return Ok(record);
        }
        self.commit(
            &mut record,
            InvocationState::PlanReady,
            actor,
            "the plan is valid and every preflight check passed",
        )?;
        if record.approval.required {
            let reason = format!(
                "approval of at least {:?} level is required: {}",
                record.approval.level,
                record.approval.reasons.join("; ")
            );
            self.commit(
                &mut record,
                InvocationState::AwaitingApproval,
                actor,
                &reason,
            )?;
        } else if record.request.execute {
            self.execute(&mut record, actor, 0)?;
        }
        Ok(record)
    }

    /** Approve an invocation's exact plan
     * Input
        - approver: &Actor - approver
        - invocation_id: &str - invocation
        - plan_digest: &str - plan digest being approved
     * Output
        - CapabilityResult<InvocationRecord>
        - Error when the approver may not approve or the digest does not match
    */
    pub(crate) fn approve(
        &mut self,
        approver: &Actor,
        invocation_id: &str,
        plan_digest: &str,
    ) -> CapabilityResult<InvocationRecord> {
        let mut record = self.ledger.get(invocation_id)?;
        let required = if record.approval.level >= ReviewLevel::Owner {
            Role::Publisher
        } else {
            Role::Contributor
        };
        let refusal = if record.state != InvocationState::AwaitingApproval {
            Some(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!(
                    "the invocation is {:?}, not awaiting approval",
                    record.state
                ),
            ))
        } else if approver.kind != ActorKind::Human || approver.id == record.requester.id {
            Some(CapabilityError::new(
                ErrorCode::Unauthorized,
                "an approver must be a person other than the requester",
            ))
        } else if let Err(error) = self.registry.grants.require(approver, required) {
            Some(error)
        } else if record.plan.as_ref().map(|plan| plan.plan_digest.as_str()) != Some(plan_digest) {
            Some(CapabilityError::new(
                ErrorCode::InvalidTransition,
                "the approval names another plan digest",
            ))
        } else {
            None
        };
        if let Some(error) = refusal {
            return Err(self.refuse("approve", Some(invocation_id), approver, error));
        }
        let at = self.clock.now();
        record.approvals.push(Approval {
            approver: approver.clone(),
            plan_digest: plan_digest.into(),
            environment: record.request.environment.clone(),
            repository: record.request.repository.clone(),
            at,
            revoked: None,
        });
        self.persist(&mut record)?;
        Ok(record)
    }

    /** Deny an invocation awaiting approval: it is blocked and can never be approved or run
     * Input
        - approver: &Actor - the person deciding
        - invocation_id: &str - invocation
        - reason: &str - why
     * Output
        - CapabilityResult<InvocationRecord>
        - Error when it is not awaiting approval, or the decider is an agent, the requester, or
          does not hold the role its approval needs
    */
    pub(crate) fn deny(
        &mut self,
        approver: &Actor,
        invocation_id: &str,
        reason: &str,
    ) -> CapabilityResult<InvocationRecord> {
        let mut record = self.ledger.get(invocation_id)?;
        let required = if record.approval.level >= ReviewLevel::Owner {
            Role::Publisher
        } else {
            Role::Contributor
        };
        let refusal = if record.state != InvocationState::AwaitingApproval {
            Some(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!(
                    "the invocation is {:?}, not awaiting approval",
                    record.state
                ),
            ))
        } else if approver.kind != ActorKind::Human || approver.id == record.requester.id {
            Some(CapabilityError::new(
                ErrorCode::Unauthorized,
                "only a person other than the requester decides its approval",
            ))
        } else if reason.trim().is_empty() {
            Some(CapabilityError::new(
                ErrorCode::InvalidSchema,
                "a reason is required",
            ))
        } else {
            self.registry.grants.require(approver, required).err()
        };
        if let Some(error) = refusal {
            return Err(self.refuse("deny", Some(invocation_id), approver, error));
        }
        self.commit(
            &mut record,
            InvocationState::Blocked,
            approver,
            &format!("approval denied by {}: {reason}", approver.id),
        )?;
        Ok(record)
    }

    /** Revoke the approvals of an invocation that has not run yet; it waits for a new approval
     * Input
        - actor: &Actor - the approver who gave it, or a publisher
        - invocation_id: &str - invocation
        - reason: &str - why
     * Output
        - CapabilityResult<InvocationRecord>
        - Error when it already ran, has no live approval, or the actor may not revoke it
    */
    pub(crate) fn revoke_approval(
        &mut self,
        actor: &Actor,
        invocation_id: &str,
        reason: &str,
    ) -> CapabilityResult<InvocationRecord> {
        let mut record = self.ledger.get(invocation_id)?;
        let refusal = if record.state != InvocationState::AwaitingApproval {
            Some(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("the invocation is {:?}; an approval that was used cannot be revoked, its effects are reconciled instead", record.state),
            ))
        } else if !record
            .approvals
            .iter()
            .any(|approval| approval.revoked.is_none())
        {
            Some(CapabilityError::new(
                ErrorCode::NotFound,
                "there is no approval to revoke",
            ))
        } else if actor.kind != ActorKind::Human {
            Some(CapabilityError::new(
                ErrorCode::Unauthorized,
                "only a person revokes an approval",
            ))
        } else if !record
            .approvals
            .iter()
            .any(|approval| approval.approver.id == actor.id)
            && self
                .registry
                .grants
                .require(actor, Role::Publisher)
                .is_err()
        {
            Some(CapabilityError::new(
                ErrorCode::Unauthorized,
                "only the approver or a publisher revokes an approval",
            ))
        } else if reason.trim().is_empty() {
            Some(CapabilityError::new(
                ErrorCode::InvalidSchema,
                "a reason is required",
            ))
        } else {
            None
        };
        if let Some(error) = refusal {
            return Err(self.refuse("revoke-approval", Some(invocation_id), actor, error));
        }
        for approval in record
            .approvals
            .iter_mut()
            .filter(|approval| approval.revoked.is_none())
        {
            approval.revoked = Some(format!("{}: {reason}", actor.id));
        }
        self.persist(&mut record)?;
        Ok(record)
    }

    /** Run an approved or approval-free plan after rechecking everything
     * Input
        - actor: &Actor - requester (or a publisher)
        - invocation_id: &str - invocation
     * Output
        - CapabilityResult<InvocationRecord>
    */
    pub(crate) fn run(
        &mut self,
        actor: &Actor,
        invocation_id: &str,
    ) -> CapabilityResult<InvocationRecord> {
        let mut record = self.ledger.get(invocation_id)?;
        if actor.id != record.requester.id
            && self
                .registry
                .grants
                .require(actor, Role::Publisher)
                .is_err()
        {
            let error = CapabilityError::new(
                ErrorCode::Unauthorized,
                "only the requester or a publisher may run an invocation",
            );
            return Err(self.refuse("run", Some(invocation_id), actor, error));
        }
        if !matches!(
            record.state,
            InvocationState::PlanReady | InvocationState::AwaitingApproval
        ) {
            let error = CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("the invocation is {:?}", record.state),
            );
            return Err(self.refuse("run", Some(invocation_id), actor, error));
        }
        let planned = record
            .plan
            .as_ref()
            .map(|plan| plan.plan_digest.clone())
            .unwrap_or_default();
        let approved = record.approvals.iter().any(|approval| {
            approval.plan_digest == planned
                && approval.environment == record.request.environment
                && approval.repository == record.request.repository
                && approval.revoked.is_none()
        });
        if record.approval.required && !approved {
            let error = CapabilityError::new(
                ErrorCode::Unauthorized,
                "the plan needs an approval bound to its digest and scope",
            );
            return Err(self.refuse("run", Some(invocation_id), actor, error));
        }
        let (checks, _, plan) = self.evaluate(&record.request, &record.requester);
        record.preflight = checks;
        match plan {
            Ok(plan) if plan.plan_digest == planned => {}
            Ok(plan) => {
                let reason = format!(
                    "the plan changed from {planned} to {}; its approval is invalidated",
                    plan.plan_digest
                );
                self.commit(&mut record, InvocationState::Blocked, actor, &reason)?;
                return Ok(record);
            }
            Err(problems) => {
                record.failures.extend(problems);
                self.commit(
                    &mut record,
                    InvocationState::Blocked,
                    actor,
                    "the plan can no longer be built",
                )?;
                return Ok(record);
            }
        }
        let blocking = record
            .preflight
            .iter()
            .filter(|check| check.blocks())
            .map(|check| format!("{}: {}", check.check_id, check.detail))
            .collect::<Vec<_>>();
        if !blocking.is_empty() {
            record.failures.extend(blocking);
            self.commit(
                &mut record,
                InvocationState::Blocked,
                actor,
                "a precondition no longer holds immediately before execution",
            )?;
            return Ok(record);
        }
        self.execute(&mut record, actor, 0)?;
        Ok(record)
    }

    /** Check one operation immediately before dispatch
     * Input
        - record: &InvocationRecord - record
        - index: usize - operation
        - started: Instant - execution start
     * Output
        - Result<(), String> - the reason it may not start
    */
    fn operation_check(
        &self,
        record: &InvocationRecord,
        index: usize,
        started: Instant,
    ) -> Result<(), String> {
        let plan = record.plan.as_ref().ok_or("no plan")?;
        let operation = &plan.operations[index];
        self.registry
            .grants
            .require(&record.requester, Role::Contributor)
            .map_err(|error| error.to_string())?;
        if started.elapsed().as_millis() as u64 > plan.limits.deadline_ms {
            return Err("the execution deadline passed".into());
        }
        let adapter = self
            .adapters
            .adapter(&operation.adapter_id)
            .ok_or("the adapter is no longer registered")?;
        if !adapter.healthy() {
            return Err(format!("{} is unavailable", operation.adapter_id));
        }
        let spec = self
            .adapters
            .operation(&operation.adapter_id, &operation.operation_id)
            .ok_or("the operation is no longer registered")?;
        if spec.effect != operation.effect {
            return Err("the operation's effect class changed since planning".into());
        }
        validate_params(&spec, &operation.params)?;
        if operation.requires_transaction && !self.runtime.available() {
            return Err(
                "a durable transaction is required and no transaction runtime is available".into(),
            );
        }
        if let Some(missing) = operation
            .preconditions
            .iter()
            .find(|condition| !record.request.confirmed_preconditions.contains(condition))
        {
            return Err(format!("precondition {missing} is not confirmed"));
        }
        if operation.effect == EffectClass::ExternallyConsequential {
            let state = &self.registry.state;
            let stale = state
                .record(&record.capability_id, record.version)
                .ok()
                .and_then(|stored| state.latest_run(stored))
                .map(|run| stale_reasons(run, state, &self.sources))
                .unwrap_or_else(|| vec!["no validation run".into()]);
            if !stale.is_empty() {
                return Err(format!(
                    "immediately before a consequential operation: {}",
                    stale.join("; ")
                ));
            }
        }
        Ok(())
    }

    /** Execute operations from an index, then verify
     * Input
        - record: &mut InvocationRecord - record
        - actor: &Actor - actor
        - from: usize - first operation to run
     * Output
        - CapabilityResult<()>
    */
    fn execute(
        &mut self,
        record: &mut InvocationRecord,
        actor: &Actor,
        from: usize,
    ) -> CapabilityResult<()> {
        self.commit(
            record,
            InvocationState::Executing,
            actor,
            "executing the plan",
        )?;
        let started = Instant::now();
        let plan = record
            .plan
            .clone()
            .ok_or_else(|| CapabilityError::new(ErrorCode::InvalidTransition, "no plan"))?;
        for index in from..plan.operations.len() {
            if record.operations[index].status == OperationStatus::Succeeded {
                continue;
            }
            if let Err(reason) = self.operation_check(record, index, started) {
                record.operations[index].status = OperationStatus::NotStarted;
                record.operations[index].error = Some(reason.clone());
                let dispatched = record
                    .operations
                    .iter()
                    .any(|operation| operation.attempts > 0);
                let to = if dispatched {
                    InvocationState::Failed
                } else {
                    InvocationState::Blocked
                };
                if dispatched {
                    record
                        .failures
                        .push(format!("{}: {reason}", plan.operations[index].step_id));
                    self.collect_effects(record, true);
                }
                return self.commit(
                    record,
                    to,
                    actor,
                    &format!(
                        "operation {} was not started: {reason}",
                        plan.operations[index].step_id
                    ),
                );
            }
            let planned = plan.operations[index].clone();
            let adapter = self
                .adapters
                .adapter(&planned.adapter_id)
                .cloned()
                .ok_or_else(|| CapabilityError::new(ErrorCode::NotFound, "adapter"))?;
            loop {
                if planned.effect != EffectClass::ReadOnly {
                    record.operations[index].status = OperationStatus::Intended;
                    self.persist(record)?;
                }
                let now = self.clock.now();
                let operation = &mut record.operations[index];
                operation.attempts += 1;
                operation.started_at.get_or_insert(now);
                let (outcome, compensation) = if planned.requires_transaction {
                    self.runtime.execute(
                        adapter.as_ref(),
                        &planned.operation_id,
                        &planned.params,
                        &planned.idempotency_key,
                    )
                } else {
                    (
                        adapter.dispatch(
                            &planned.operation_id,
                            &planned.params,
                            &planned.idempotency_key,
                        ),
                        None,
                    )
                };
                operation.compensation = compensation.or(operation.compensation.take());
                operation.finished_at = Some(self.clock.now());
                let read_only = planned.effect == EffectClass::ReadOnly;
                let retries_left = operation.attempts <= plan.limits.max_retries;
                match outcome {
                    DispatchOutcome::Completed {
                        mut result,
                        external_ref,
                    } => {
                        redact_value(&mut result);
                        operation.external_ref = external_ref;
                        operation.effect_status = if read_only {
                            EffectStatus::None
                        } else {
                            EffectStatus::Applied
                        };
                        if result.to_string().len() > planned.max_output_bytes {
                            operation.status = OperationStatus::Failed;
                            operation.error = Some(format!(
                                "the result exceeds {} bytes",
                                planned.max_output_bytes
                            ));
                            operation.result = json!({"truncated": true});
                            record
                                .failures
                                .push(format!("{}: output limit exceeded", planned.step_id));
                            self.collect_effects(record, true);
                            return self.commit(
                                record,
                                InvocationState::Failed,
                                actor,
                                "an operation exceeded its output limit",
                            );
                        }
                        operation.status = OperationStatus::Succeeded;
                        operation.result = result;
                        self.persist(record)?;
                        break;
                    }
                    DispatchOutcome::Failed { error, effect } => {
                        operation.status = OperationStatus::Failed;
                        operation.error = Some(redact(&error).0);
                        operation.effect_status = effect;
                        if effect == EffectStatus::Unknown {
                            record.recovery_status = "reconciliation required".into();
                            self.collect_effects(record, false);
                            return self.commit(
                                record,
                                InvocationState::ReconciliationRequired,
                                actor,
                                &format!("{} failed and its effect is unknown", planned.step_id),
                            );
                        }
                        record
                            .failures
                            .push(format!("{}: {error}", planned.step_id));
                        self.collect_effects(record, true);
                        return self.commit(
                            record,
                            InvocationState::Failed,
                            actor,
                            &format!("{} failed definitively", planned.step_id),
                        );
                    }
                    DispatchOutcome::TimedOut {
                        dispatched,
                        external_ref,
                    } => {
                        operation.status = OperationStatus::TimedOut;
                        operation.external_ref = external_ref;
                        let no_effect_possible = !dispatched || read_only;
                        operation.effect_status = if read_only {
                            EffectStatus::None
                        } else if dispatched {
                            EffectStatus::Unknown
                        } else {
                            EffectStatus::NotApplied
                        };
                        if no_effect_possible && retries_left {
                            self.persist(record)?;
                            continue;
                        }
                        if no_effect_possible {
                            record.failures.push(format!(
                                "{}: timed out and the retry limit is reached",
                                planned.step_id
                            ));
                            self.collect_effects(record, true);
                            return self.commit(
                                record,
                                InvocationState::Failed,
                                actor,
                                "an operation timed out with no effect and no retries left",
                            );
                        }
                        record.recovery_status = "reconciliation required".into();
                        self.collect_effects(record, false);
                        return self.commit(record, InvocationState::ReconciliationRequired, actor, &format!("{} timed out after dispatch; its effect is unknown and it is not retried blindly", planned.step_id));
                    }
                }
            }
        }
        self.collect_effects(record, false);
        self.commit(
            record,
            InvocationState::Verifying,
            actor,
            "every operation completed; adapter success is not yet proof of the outcome",
        )?;
        self.verify(record, actor)
    }

    /** Recompute the effects that need attention: unknown effects always, and applied
     * state-changing effects when the invocation stops short (they may need recovery)
     * Input
        - record: &mut InvocationRecord - record
        - include_applied: bool - whether applied effects are listed too
     * Output
        - None
    */
    fn collect_effects(&self, record: &mut InvocationRecord, include_applied: bool) {
        record.unresolved_effects = record
            .operations
            .iter()
            .filter(|operation| {
                operation.effect_status == EffectStatus::Unknown
                    || (include_applied && operation.effect_status == EffectStatus::Applied)
            })
            .map(|operation| {
                format!(
                    "{} ({}.{}): effect {:?}{}",
                    operation.step_id,
                    operation.adapter_id,
                    operation.operation_id,
                    operation.effect_status,
                    operation
                        .compensation
                        .as_ref()
                        .map(|compensation| format!(
                            "; compensation reported separately: {}",
                            compensation.outcome
                        ))
                        .unwrap_or_default()
                )
            })
            .collect();
    }

    /** Run every bound postcondition check and settle the outcome
     * Input
        - record: &mut InvocationRecord - record
        - actor: &Actor - actor
     * Output
        - CapabilityResult<()>
    */
    fn verify(&mut self, record: &mut InvocationRecord, actor: &Actor) -> CapabilityResult<()> {
        let plan = record
            .plan
            .clone()
            .ok_or_else(|| CapabilityError::new(ErrorCode::InvalidTransition, "no plan"))?;
        let mut results = Vec::new();
        for (index, planned) in plan.verifications.iter().enumerate() {
            let mut result = VerificationResult {
                claim_id: planned.claim_id.clone(),
                requirement: planned.requirement.clone(),
                required: planned.required,
                method: "none".into(),
                evidence_refs: Vec::new(),
                observed: None,
                result: VerificationStatus::Unresolved,
                limitations: Vec::new(),
                candidate_digest: record.digest.clone(),
                environment: record.request.environment.clone(),
            };
            match &planned.method {
                None => result.limitations.push(
                    "no verification method is bound; the claim cannot be established".into(),
                ),
                Some(VerificationMethod::AdapterCheck {
                    adapter_id,
                    operation_id,
                    params,
                    field: path,
                    equals,
                }) => {
                    result.method = format!("fresh read: {adapter_id}.{operation_id}");
                    let resolved = params
                        .iter()
                        .map(|(name, source)| {
                            plan::resolve(source, &record.request)
                                .map(|value| (name.clone(), value))
                        })
                        .collect::<Result<BTreeMap<_, _>, _>>();
                    match (self.adapters.adapter(adapter_id), resolved) {
                        (Some(adapter), Ok(resolved)) => {
                            let key = stable_id(
                                "ivf",
                                &[&record.invocation_id, &planned.claim_id, &index.to_string()],
                            );
                            match adapter.dispatch(operation_id, &resolved, &key) {
                                DispatchOutcome::Completed { result: value, .. } => {
                                    result
                                        .evidence_refs
                                        .push(format!("{adapter_id}.{operation_id}:{key}"));
                                    result.observed = field(&value, path);
                                    result.result = match &result.observed {
                                        Some(found) if found == equals => {
                                            VerificationStatus::Verified
                                        }
                                        Some(_) => VerificationStatus::Failed,
                                        None => VerificationStatus::Unresolved,
                                    };
                                }
                                other => result.limitations.push(format!(
                                    "the verification read did not complete: {other:?}"
                                )),
                            }
                        }
                        (None, _) => result
                            .limitations
                            .push(format!("{adapter_id} is not registered")),
                        (_, Err(problem)) => result.limitations.push(problem),
                    }
                }
                Some(VerificationMethod::WorkCaptureEvidence {
                    evidence_type,
                    field: path,
                    equals,
                }) => {
                    result.method =
                        "Work Capture evidence (reused from the episode, not fresh)".into();
                    result.limitations.push(
                        "reuses episode evidence; it does not observe this invocation".into(),
                    );
                    let mut seen = Vec::new();
                    for id in self
                        .sources
                        .capture
                        .evidence
                        .iter()
                        .filter(|(_, record)| record.evidence_type == *evidence_type)
                        .map(|(id, _)| id.clone())
                    {
                        if let (_, Some(evidence)) = self.sources.evidence(&id) {
                            if let Some(value) = evidence.field(path) {
                                seen.push((id, value));
                            }
                        }
                    }
                    result.evidence_refs = seen.iter().map(|(id, _)| id.clone()).collect();
                    let matching = seen.iter().filter(|(_, value)| value == equals).count();
                    result.observed = seen.first().map(|(_, value)| value.clone());
                    result.result = match (matching, seen.len() - matching) {
                        (0, 0) => VerificationStatus::Unresolved,
                        (_, 0) => VerificationStatus::Verified,
                        (0, _) => VerificationStatus::Failed,
                        _ => VerificationStatus::Contradicted,
                    };
                }
            }
            results.push(result);
        }
        let mut claims: BTreeMap<String, (bool, Vec<VerificationStatus>)> = BTreeMap::new();
        for result in &results {
            let entry = claims
                .entry(result.claim_id.clone())
                .or_insert((result.required, Vec::new()));
            entry.1.push(result.result);
        }
        let settle = |statuses: &[VerificationStatus]| {
            let verified = statuses.contains(&VerificationStatus::Verified);
            let failed = statuses.contains(&VerificationStatus::Failed);
            if statuses.contains(&VerificationStatus::Contradicted) || (verified && failed) {
                VerificationStatus::Contradicted
            } else if failed {
                VerificationStatus::Failed
            } else if verified
                && statuses
                    .iter()
                    .all(|status| *status == VerificationStatus::Verified)
            {
                VerificationStatus::Verified
            } else {
                VerificationStatus::Unresolved
            }
        };
        let required = claims
            .values()
            .filter(|(required, _)| *required)
            .map(|(_, statuses)| settle(statuses))
            .collect::<Vec<_>>();
        record.verifications = results;
        let (to, reason) = if required.contains(&VerificationStatus::Failed) {
            (
                InvocationState::Failed,
                "a required postcondition definitively failed",
            )
        } else if !required.is_empty()
            && required
                .iter()
                .all(|status| *status == VerificationStatus::Verified)
        {
            (
                InvocationState::Succeeded,
                "every required postcondition is supported by evidence",
            )
        } else if required.contains(&VerificationStatus::Verified) {
            (InvocationState::PartiallyVerified, "some required postconditions are established; others are unresolved or contradicted")
        } else {
            (
                InvocationState::Unknown,
                "no required postcondition could be established",
            )
        };
        if to == InvocationState::Failed {
            record.failures.push(reason.into());
        }
        self.commit(record, to, actor, reason)?;
        self.hand_off(record, actor);
        Ok(())
    }

    /** Hand the outcome to Component 2's invocation history (never changes the definition)
     * Input
        - record: &mut InvocationRecord - record
        - actor: &Actor - actor
     * Output
        - None
    */
    fn hand_off(&mut self, record: &mut InvocationRecord, actor: &Actor) {
        let status = match record.state {
            InvocationState::Succeeded => InvocationStatus::Succeeded,
            InvocationState::Failed => InvocationStatus::Failed,
            _ => InvocationStatus::Unknown,
        };
        let evidence = record
            .verifications
            .iter()
            .flat_map(|verification| verification.evidence_refs.iter().cloned())
            .collect();
        let reference = InvocationRef {
            invocation_id: record.invocation_id.clone(),
            status,
            evidence_refs: evidence,
            recorded_by: actor.clone(),
            recorded_at: self.clock.now(),
        };
        if let Err(error) = self.registry.record_invocation(
            &record.requester,
            &record.capability_id,
            record.version,
            reference,
        ) {
            record.failures.push(format!(
                "the outcome could not be handed to the capability registry: {error}"
            ));
            let _ = self.persist(record);
        }
    }

    /** Reconcile uncertain effects against the authoritative systems, then resume, verify, or stay
     * unknown
     * Input
        - actor: &Actor - requester or a publisher
        - invocation_id: &str - invocation
     * Output
        - CapabilityResult<InvocationRecord>
    */
    pub(crate) fn reconcile(
        &mut self,
        actor: &Actor,
        invocation_id: &str,
    ) -> CapabilityResult<InvocationRecord> {
        let mut record = self.ledger.get(invocation_id)?;
        if actor.id != record.requester.id
            && self
                .registry
                .grants
                .require(actor, Role::Publisher)
                .is_err()
        {
            let error = CapabilityError::new(
                ErrorCode::Unauthorized,
                "only the requester or a publisher may reconcile an invocation",
            );
            return Err(self.refuse("reconcile", Some(invocation_id), actor, error));
        }
        match record.state {
            InvocationState::ReconciliationRequired => {}
            InvocationState::Unknown => self.commit(
                &mut record,
                InvocationState::ReconciliationRequired,
                actor,
                "an explicit decision to reconcile again",
            )?,
            other => {
                let error = CapabilityError::new(
                    ErrorCode::InvalidTransition,
                    format!("the invocation is {other:?}; nothing to reconcile"),
                );
                return Err(self.refuse("reconcile", Some(invocation_id), actor, error));
            }
        }
        let plan = record
            .plan
            .clone()
            .ok_or_else(|| CapabilityError::new(ErrorCode::InvalidTransition, "no plan"))?;
        for index in 0..record.operations.len() {
            if record.operations[index].effect_status != EffectStatus::Unknown {
                continue;
            }
            let planned = &plan.operations[index];
            let outcome = match (
                planned.reconcilable,
                self.adapters.adapter(&planned.adapter_id),
            ) {
                (true, Some(adapter)) => adapter.reconcile(
                    &planned.operation_id,
                    &planned.idempotency_key,
                    record.operations[index].external_ref.as_deref(),
                ),
                _ => Reconciliation::Unavailable {
                    reason: "the operation cannot be reconciled".into(),
                },
            };
            let at = self.clock.now();
            let operation = &mut record.operations[index];
            let note = match outcome {
                Reconciliation::Applied {
                    mut result,
                    external_ref,
                } => {
                    redact_value(&mut result);
                    operation.effect_status = EffectStatus::Applied;
                    operation.status = OperationStatus::Succeeded;
                    operation.result = result;
                    operation.external_ref = external_ref.or(operation.external_ref.take());
                    "applied: the authoritative system shows the effect".to_string()
                }
                Reconciliation::NotApplied => {
                    operation.effect_status = EffectStatus::NotApplied;
                    operation.status = OperationStatus::Pending;
                    "not applied: the authoritative system shows no effect; it may be dispatched again".to_string()
                }
                Reconciliation::Unavailable { reason } => format!("unavailable: {reason}"),
            };
            operation
                .reconciliations
                .push(ReconciliationRecord { at, outcome: note });
        }
        self.collect_effects(&mut record, false);
        if record
            .operations
            .iter()
            .any(|operation| operation.effect_status == EffectStatus::Unknown)
        {
            record.recovery_status = "unresolved: an explicit, authorized decision is required (reconcile again or abandon)".into();
            self.commit(
                &mut record,
                InvocationState::Unknown,
                actor,
                "some effects could not be established",
            )?;
            return Ok(record);
        }
        record.recovery_status = "reconciled".into();
        match record
            .operations
            .iter()
            .position(|operation| operation.status != OperationStatus::Succeeded)
        {
            Some(index) => self.execute(&mut record, actor, index)?,
            None => {
                self.commit(
                    &mut record,
                    InvocationState::Verifying,
                    actor,
                    "every effect is established",
                )?;
                self.verify(&mut record, actor)?;
            }
        }
        Ok(record)
    }

    /** Abandon an invocation, recording its remaining effects
     * Input
        - actor: &Actor - requester (before any effect) or a publisher
        - invocation_id: &str - invocation
        - reason: &str - why
     * Output
        - CapabilityResult<InvocationRecord>
    */
    pub(crate) fn abandon(
        &mut self,
        actor: &Actor,
        invocation_id: &str,
        reason: &str,
    ) -> CapabilityResult<InvocationRecord> {
        let mut record = self.ledger.get(invocation_id)?;
        if record.state.terminal() {
            let error = CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("the invocation already ended {:?}", record.state),
            );
            return Err(self.refuse("abandon", Some(invocation_id), actor, error));
        }
        let effects = record.operations.iter().any(|operation| {
            matches!(
                operation.effect_status,
                EffectStatus::Applied | EffectStatus::Unknown
            )
        });
        let permitted = if effects {
            self.registry.grants.require(actor, Role::Publisher).is_ok()
                && actor.kind == ActorKind::Human
        } else {
            actor.id == record.requester.id
                || self.registry.grants.require(actor, Role::Publisher).is_ok()
        };
        if !permitted || reason.trim().is_empty() {
            let error = CapabilityError::new(
                ErrorCode::Unauthorized,
                "abandoning an invocation with effects needs a publisher and a reason",
            );
            return Err(self.refuse("abandon", Some(invocation_id), actor, error));
        }
        self.collect_effects(&mut record, true);
        record.recovery_status = if record.unresolved_effects.is_empty() {
            "nothing to recover".into()
        } else {
            "handed over: the recorded effects need recovery outside the engine".into()
        };
        if let Err(error) = self.commit(&mut record, InvocationState::Abandoned, actor, reason) {
            return Err(self.refuse("abandon", Some(invocation_id), actor, error));
        }
        Ok(record)
    }
}
