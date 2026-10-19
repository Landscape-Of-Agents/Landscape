// The capability lifecycle: an explicit transition table, the role each transition needs, and
// publication eligibility (gate 6). Lifecycle state is mutable operational metadata kept beside
// the immutable definition, never inside it. Invocation outcomes are a separate record and never
// change a definition or its lifecycle state.
//
//   candidate      -> needs_evidence | validating | rejected
//   needs_evidence -> validating | rejected
//   validating     -> validated | needs_evidence | candidate | rejected
//   validated      -> published | validating | needs_evidence | deprecated | revoked | rejected
//   published      -> deprecated | revoked
//   deprecated     -> revoked
//   rejected, revoked: terminal

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use super::validate::{
    GateId, GateResult, GateStatus, RunOutcome, Severity, ValidationFinding, ValidationProfile,
    ValidationRun, GATE_VERSION,
};
use super::{CapabilityResult, Role};
use crate::capture::{Actor, ActorKind};

/** Lifecycle state of a capability version
 * Variants
    - Candidate - a draft that has not satisfied validation
    - NeedsEvidence - required evidence, references, or applicability information is missing
    - Validating - a validation run is in progress
    - Validated - the applicable validation requirements are satisfied
    - Published - an authorized publication made it eligible for discovery and use
    - Rejected - rejected, with a recorded reason
    - Deprecated - not to be selected for new use under normal policy
    - Revoked - withdrawn; never selected for new use
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LifecycleState {
    Candidate,
    NeedsEvidence,
    Validating,
    Validated,
    Published,
    Rejected,
    Deprecated,
    Revoked,
}

impl LifecycleState {
    /** Report whether another capability may depend on this version
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn selectable_for_dependencies(self) -> bool {
        !matches!(self, Self::Rejected | Self::Revoked)
    }
}

/** Report whether a transition is in the table
 * Input
    - from: LifecycleState - current state
    - to: LifecycleState - requested state
 * Output
    - bool
*/
pub(crate) fn allowed(from: LifecycleState, to: LifecycleState) -> bool {
    use LifecycleState::{
        Candidate, Deprecated, NeedsEvidence, Published, Rejected, Revoked, Validated, Validating,
    };
    matches!(
        (from, to),
        (Candidate, NeedsEvidence | Validating | Rejected)
            | (NeedsEvidence, Validating | Rejected)
            | (Validating, Validated | NeedsEvidence | Candidate | Rejected)
            | (
                Validated,
                Published | Validating | NeedsEvidence | Deprecated | Revoked | Rejected
            )
            | (Published, Deprecated | Revoked)
            | (Deprecated, Revoked)
    )
}

/** The role a transition needs
 * Input
    - to: LifecycleState - target state
 * Output
    - Role
*/
pub(crate) fn role_for(to: LifecycleState) -> Role {
    match to {
        LifecycleState::Candidate
        | LifecycleState::NeedsEvidence
        | LifecycleState::Validating
        | LifecycleState::Validated => Role::Validator,
        LifecycleState::Published
        | LifecycleState::Rejected
        | LifecycleState::Deprecated
        | LifecycleState::Revoked => Role::Publisher,
    }
}

/** An audited lifecycle transition
 * Fields
    - transition_id: String - identifier
    - capability_id: String - capability
    - version: u32 - version
    - from: Option<LifecycleState> - previous state, None at creation
    - to: LifecycleState - new state
    - actor: Actor - who caused it
    - reason: String - why
    - validation_run: Option<String> - validation run that caused it
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LifecycleTransition {
    pub(crate) transition_id: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) from: Option<LifecycleState>,
    pub(crate) to: LifecycleState,
    pub(crate) actor: Actor,
    pub(crate) reason: String,
    pub(crate) validation_run: Option<String>,
    pub(crate) at: u64,
}

/** A publication request
 * Fields
    - capability_id: String - capability
    - version: u32 - version
    - reason: String - why it is published
    - acknowledged_warnings: Vec<String> - warning codes the publisher accepts explicitly
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PublicationRequest {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) reason: String,
    pub(crate) acknowledged_warnings: Vec<String>,
}

/** The state a validation run leads to
 * Input
    - outcome: RunOutcome - run outcome
 * Output
    - LifecycleState
*/
pub(crate) fn state_after(outcome: RunOutcome) -> LifecycleState {
    match outcome {
        RunOutcome::Passed => LifecycleState::Validated,
        RunOutcome::NeedsEvidence => LifecycleState::NeedsEvidence,
        RunOutcome::StructurallyInvalid | RunOutcome::Failed => LifecycleState::Candidate,
    }
}

/** What gate 6 needs to know about a version
 * Fields
    - state: LifecycleState - current state
    - author: &'a Actor - author of the definition
    - definition_digest: &'a str - current definition digest
    - run: Option<&'a ValidationRun> - latest non-dry validation run
    - stale: bool - whether that run is stale
    - profile: &'a ValidationProfile - profile
*/
pub(crate) struct Eligibility<'a> {
    pub(crate) state: LifecycleState,
    pub(crate) author: &'a Actor,
    pub(crate) definition_digest: &'a str,
    pub(crate) run: Option<&'a ValidationRun>,
    pub(crate) stale: bool,
    pub(crate) profile: &'a ValidationProfile,
}

/** Gate 6: publication eligibility. Every mandatory gate must have passed (warnings only when
 * each is acknowledged), the validation must be current, the version validated, and the publisher
 * an authorized human other than the author when the profile requires independence
 * Input
    - eligibility: &Eligibility - version facts
    - request: &PublicationRequest - request
    - publisher: &Actor - publisher
    - authorized: CapabilityResult<()> - result of the registry's role check
    - now: u64 - Unix milliseconds
 * Output
    - GateResult (Failed with every reason when publication is not allowed)
*/
pub(crate) fn publication_gate(
    eligibility: &Eligibility,
    request: &PublicationRequest,
    publisher: &Actor,
    authorized: CapabilityResult<()>,
    now: u64,
) -> GateResult {
    let mut findings = Vec::new();
    let mut error = |code: &str, message: String| {
        findings.push(ValidationFinding {
            code: code.into(),
            severity: Severity::Error,
            message,
            evidence_refs: Vec::new(),
        })
    };
    if let Err(denied) = authorized {
        error("unauthorized", denied.message);
    }
    if publisher.kind != ActorKind::Human {
        error("unauthorized", "only a person can publish".into());
    }
    if eligibility.profile.require_independent_publisher && publisher.id == eligibility.author.id {
        error(
            "independent_publisher",
            "the author cannot publish their own capability under this profile".into(),
        );
    }
    if eligibility.state != LifecycleState::Validated {
        error(
            "not_validated",
            format!(
                "the version is {:?}; only a validated version can be published",
                eligibility.state
            ),
        );
    }
    match eligibility.run {
        None => error("not_validated", "the version has no validation run".into()),
        Some(run) => {
            if run.basis.definition_digest != eligibility.definition_digest {
                error(
                    "stale_validation",
                    "the validation run validated other content".into(),
                );
            }
            if eligibility.stale {
                error(
                    "stale_validation",
                    "a referenced version changed since validation; revalidate first".into(),
                );
            }
            for gate in run
                .gates
                .iter()
                .filter(|gate| gate.status == GateStatus::Failed)
            {
                error("gate_failed", format!("gate {:?} failed", gate.gate));
            }
            let acknowledged = request
                .acknowledged_warnings
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>();
            let unacknowledged = run
                .warning_codes()
                .difference(&acknowledged)
                .cloned()
                .collect::<Vec<_>>();
            if !unacknowledged.is_empty() {
                error(
                    "unacknowledged_warnings",
                    format!(
                        "warnings must be acknowledged explicitly: {}",
                        unacknowledged.join(", ")
                    ),
                );
            }
        }
    }
    let status = if findings.is_empty() {
        GateStatus::Passed
    } else {
        GateStatus::Failed
    };
    GateResult {
        gate: GateId::Publication,
        gate_version: GATE_VERSION,
        status,
        findings,
        evidence_refs: eligibility
            .run
            .map(|run| vec![run.run_id.clone()])
            .unwrap_or_default(),
        method: format!("capability-gates/v{GATE_VERSION}"),
        validated_at: now,
        validator: publisher.clone(),
    }
}
