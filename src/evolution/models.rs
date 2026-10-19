// Typed, versioned evolution records and their three lifecycles. Signals, proposals, and
// regression runs each have their own state table; none of them is a capability lifecycle state
// (Component 2 stays authoritative for candidates). Every persisted record carries its schema
// version and a digest over its content, verified whenever the evolution store is loaded.
//
// Signal:     signal_detected -> qualifying | dismissed | superseded
//             qualifying -> insufficient_evidence | qualified | dismissed
//             insufficient_evidence -> superseded | dismissed
//             qualified -> investigating | dismissed | superseded
//             investigating -> proposal_created | no_change_recommended | dismissed | superseded
//             proposal_created -> superseded
//             no_change_recommended -> superseded | dismissed
//             dismissed, superseded: terminal
//
// Proposal:   draft -> candidate_created | blocked | abandoned | superseded
//             candidate_created -> regression_pending | blocked | abandoned | superseded
//             regression_pending -> regression_passed | regression_failed | inconclusive | blocked
//                                   | abandoned | superseded
//             regression_passed -> regression_pending | submitted_for_review | blocked | abandoned
//                                  | superseded
//             regression_failed, inconclusive, blocked -> regression_pending | abandoned | superseded
//             submitted_for_review -> rejected | abandoned | superseded
//             any state with a candidate -> rejected (when Component 2 rejected the candidate)
//             rejected, abandoned, superseded: terminal
//
// Regression: created -> preflight | cancelled
//             preflight -> running | blocked | cancelled
//             running -> passed | failed | inconclusive | cancelled
//             passed, failed, inconclusive, blocked, cancelled: terminal

use std::collections::BTreeMap;
use std::fmt::Debug;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::{CapabilityDefinition, DependencyKind, ReviewLevel, RiskLevel};
use crate::capability::refs::CurrentVersions;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capability_lab::scenarios::TestPlan;
use crate::capability_lab::CheckStatus;
use crate::capture::Actor;
use crate::invocation::lifecycle::InvocationState;
use crate::trust::crypto::digest_json;

/** A state table: which transitions are allowed and which states are terminal */
pub(crate) trait Lifecycle: Copy + PartialEq + Debug {
    /** Report whether a transition is in the table
     * Input
        - to: Self - requested state
     * Output
        - bool
    */
    fn allows(self, to: Self) -> bool;

    /** Report whether the state is terminal
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn terminal(self) -> bool;
}

/** One recorded lifecycle transition
 * Fields
    - sequence: u64 - 1, 2, ... within the record
    - from: Option<S> - previous state, None for the first
    - to: S - new state
    - actor: Actor - who caused it
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Transition<S> {
    pub(crate) sequence: u64,
    pub(crate) from: Option<S>,
    pub(crate) to: S,
    pub(crate) actor: Actor,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** Apply a validated transition; a refused one changes nothing
 * Input
    - state: &mut S - current state
    - history: &mut Vec<Transition<S>> - transitions
    - to: S - requested state
    - actor: &Actor - who
    - reason: &str - why
    - at: u64 - Unix milliseconds
 * Output
    - CapabilityResult<()>
    - Error (InvalidTransition) when the table forbids it
*/
pub(crate) fn advance<S: Lifecycle>(
    state: &mut S,
    history: &mut Vec<Transition<S>>,
    to: S,
    actor: &Actor,
    reason: &str,
    at: u64,
) -> CapabilityResult<()> {
    if !state.allows(to) {
        return Err(CapabilityError::new(
            ErrorCode::InvalidTransition,
            format!("cannot go from {state:?} to {to:?}"),
        ));
    }
    history.push(Transition {
        sequence: history.len() as u64 + 1,
        from: Some(*state),
        to,
        actor: actor.clone(),
        reason: reason.into(),
        at,
    });
    *state = to;
    Ok(())
}

/** The state of an evolution signal
 * Variants
    - SignalDetected - a deterministic detector found a potential pattern or drift
    - Qualifying - independence, comparability, completeness, provenance, contradictions checked
    - InsufficientEvidence - the observations cannot support a defensible conclusion
    - Qualified - the deterministic qualification rules are met
    - Investigating - context and counterevidence assembled; a hypothesis exists
    - ProposalCreated - a revision proposal was created from it
    - NoChangeRecommended - the evidence indicates no revision should be proposed
    - Dismissed - closed by an authorized person with a recorded reason
    - Superseded - replaced by a newer analysis, history kept
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SignalState {
    SignalDetected,
    Qualifying,
    InsufficientEvidence,
    Qualified,
    Investigating,
    ProposalCreated,
    NoChangeRecommended,
    Dismissed,
    Superseded,
}

impl Lifecycle for SignalState {
    /** Report whether a signal transition is in the table
     * Input
        - to: SignalState - requested state
     * Output
        - bool
    */
    fn allows(self, to: Self) -> bool {
        use SignalState::{
            Dismissed, InsufficientEvidence, Investigating, NoChangeRecommended, ProposalCreated,
            Qualified, Qualifying, SignalDetected, Superseded,
        };
        matches!(
            (self, to),
            (SignalDetected, Qualifying | Dismissed | Superseded)
                | (Qualifying, InsufficientEvidence | Qualified | Dismissed)
                | (InsufficientEvidence, Superseded | Dismissed)
                | (Qualified, Investigating | Dismissed | Superseded)
                | (
                    Investigating,
                    ProposalCreated | NoChangeRecommended | Dismissed | Superseded
                )
                | (ProposalCreated, Superseded)
                | (NoChangeRecommended, Superseded | Dismissed)
        )
    }

    /** Report whether the signal state is terminal
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn terminal(self) -> bool {
        matches!(self, Self::Dismissed | Self::Superseded)
    }
}

/** The state of a revision proposal (the evolution workflow's record, not the candidate's state)
 * Variants
    - Draft - a hypothesis and change set exist, no candidate yet
    - CandidateCreated - a candidate version was stored through Component 2
    - RegressionPending - a regression plan exists, no completed run
    - RegressionPassed - every required supported check passed (not publication approval)
    - RegressionFailed - a supported check found a regression or violated invariant
    - Inconclusive - required comparisons cannot be established
    - Blocked - authorization, integrity, staleness, or an unsupported change prevents progress
    - SubmittedForReview - routed to a review workflow (reserved: Crane has no review-request
      mechanism, so nothing reaches this state today)
    - Rejected - the candidate was rejected through Component 2
    - Superseded - the parent or candidate changed, so the proposal is no longer current
    - Abandoned - closed without publication
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposalState {
    Draft,
    CandidateCreated,
    RegressionPending,
    RegressionPassed,
    RegressionFailed,
    Inconclusive,
    Blocked,
    #[cfg_attr(not(test), allow(dead_code))] // reserved: no review-request mechanism exists
    SubmittedForReview,
    Rejected,
    Superseded,
    Abandoned,
}

impl Lifecycle for ProposalState {
    /** Report whether a proposal transition is in the table
     * Input
        - to: ProposalState - requested state
     * Output
        - bool
    */
    fn allows(self, to: Self) -> bool {
        use ProposalState::{
            Abandoned, Blocked, CandidateCreated, Draft, Inconclusive, RegressionFailed,
            RegressionPassed, RegressionPending, Rejected, SubmittedForReview, Superseded,
        };
        matches!(
            (self, to),
            (Draft, CandidateCreated | Blocked | Abandoned | Superseded)
                | (
                    CandidateCreated,
                    RegressionPending | Blocked | Abandoned | Superseded
                )
                | (
                    RegressionPending,
                    RegressionPassed
                        | RegressionFailed
                        | Inconclusive
                        | Blocked
                        | Abandoned
                        | Superseded
                )
                | (
                    RegressionPassed,
                    RegressionPending | SubmittedForReview | Blocked | Abandoned | Superseded
                )
                | (
                    RegressionFailed | Inconclusive | Blocked,
                    RegressionPending | Abandoned | Superseded
                )
                | (SubmittedForReview, Rejected | Abandoned | Superseded)
                | (
                    CandidateCreated
                        | RegressionPending
                        | RegressionPassed
                        | RegressionFailed
                        | Inconclusive
                        | Blocked,
                    Rejected
                )
        )
    }

    /** Report whether the proposal state is terminal
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn terminal(self) -> bool {
        matches!(self, Self::Rejected | Self::Abandoned | Self::Superseded)
    }
}

/** The state of a regression run
 * Variants
    - Created - the run exists
    - Preflight - identities, digests, staleness, integrity, and the plan are checked
    - Running - supported checks are evaluated
    - Passed - every required supported check passed
    - Failed - a supported check found a regression
    - Inconclusive - a required guarantee cannot be established
    - Blocked - preflight or authorization prevented the checks
    - Cancelled - stopped before completion
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunState {
    Created,
    Preflight,
    Running,
    Passed,
    Failed,
    Inconclusive,
    Blocked,
    #[cfg_attr(not(test), allow(dead_code))] // runs complete in one step; kept for the table
    Cancelled,
}

impl Lifecycle for RunState {
    /** Report whether a run transition is in the table
     * Input
        - to: RunState - requested state
     * Output
        - bool
    */
    fn allows(self, to: Self) -> bool {
        use RunState::{
            Blocked, Cancelled, Created, Failed, Inconclusive, Passed, Preflight, Running,
        };
        matches!(
            (self, to),
            (Created, Preflight | Cancelled)
                | (Preflight, Running | Blocked | Cancelled)
                | (Running, Passed | Failed | Inconclusive | Cancelled)
        )
    }

    /** Report whether the run state is terminal
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn terminal(self) -> bool {
        !matches!(self, Self::Created | Self::Preflight | Self::Running)
    }
}

/** The evolution classification of one invocation outcome (the source record is never changed)
 * Variants
    - VerifiedSuccess - succeeded and every required postcondition was verified
    - DeclaredSuccessUnverified - recorded as succeeded without sufficient verification
    - VerifiedFailure - an operation failed definitively, as reported by its system of record
    - FailedVerification - an independent postcondition check failed
    - PartiallyVerified - some required outcomes established, others unresolved
    - Contradicted - checks of one postcondition disagree; no side is chosen
    - Unknown - the outcome was not established
    - ReconciliationUnresolved - an effect is uncertain and awaits reconciliation
    - Abandoned - deliberately stopped
    - BlockedBeforeExecution - prevented before anything ran (not a capability trial)
    - InProgress - not finished
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OutcomeClass {
    VerifiedSuccess,
    DeclaredSuccessUnverified,
    VerifiedFailure,
    FailedVerification,
    PartiallyVerified,
    Contradicted,
    Unknown,
    ReconciliationUnresolved,
    Abandoned,
    BlockedBeforeExecution,
    InProgress,
}

impl OutcomeClass {
    /** Report whether the class is an established (evidence-backed) failure
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn established_failure(self) -> bool {
        matches!(self, Self::VerifiedFailure | Self::FailedVerification)
    }

    /** Report whether the class is a verification gap (outcome not established)
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn gap(self) -> bool {
        matches!(
            self,
            Self::DeclaredSuccessUnverified
                | Self::PartiallyVerified
                | Self::Unknown
                | Self::ReconciliationUnresolved
        )
    }

    /** Report whether the invocation was a capability trial (it got past preflight and finished)
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn trial(self) -> bool {
        !matches!(
            self,
            Self::BlockedBeforeExecution | Self::InProgress | Self::Abandoned
        )
    }
}

/** Where an evidence reference comes from
 * Variants
    - WorkCapture - a Work Capture evidence record
    - InvocationVerification - a verification result recorded by Component 6
    - DataSeaEvent - a DataSea lifecycle event (correlation only, never a trial)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceSource {
    WorkCapture,
    InvocationVerification,
    DataSeaEvent,
}

/** Whether a reference could be read
 * Variants
    - Readable - resolved and readable by the actor (Work Capture checks its content hash)
    - Restricted - exists but the actor may not read it (never retrieved)
    - Missing - does not resolve
    - RecordedByEngine - a result inside an integrity-checked invocation record
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceAccess {
    Readable,
    Restricted,
    Missing,
    RecordedByEngine,
}

/** One evidence reference and its provenance
 * Fields
    - ref_id: String - identifier
    - source: EvidenceSource - where it comes from
    - access: EvidenceAccess - whether it could be read
    - invocation_id: String - invocation it belongs to
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct EvidenceRef {
    pub(crate) ref_id: String,
    pub(crate) source: EvidenceSource,
    pub(crate) access: EvidenceAccess,
    pub(crate) invocation_id: String,
}

/** The context an invocation ran in
 * Fields
    - environment: Option<String> - environment, None when unknown
    - repository: Option<String> - repository, None when unknown
    - governance: BTreeMap<String, String> - "zone:ID" etc. to the version current at invocation
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct ObservationContext {
    pub(crate) environment: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) governance: BTreeMap<String, String>,
}

impl ObservationContext {
    /** A stable key for comparable contexts
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn key(&self) -> String {
        let governance = self
            .governance
            .iter()
            .map(|(key, version)| format!("{key}@{version}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "environment={};repository={};governance=[{governance}]",
            self.environment.as_deref().unwrap_or("unknown"),
            self.repository.as_deref().unwrap_or("unknown")
        )
    }
}

/** One normalized invocation observation
 * Fields
    - invocation_id: String - Component 6 invocation
    - record_digest: String - the invocation record's digest (verified)
    - capability_id, version, capability_digest - exact capability invoked
    - plan_digest: Option<String> - plan digest
    - independence_key: String - same key means the same trial resubmitted
    - state: InvocationState - recorded state
    - class: OutcomeClass - evolution classification
    - context: ObservationContext - context
    - verified_claims, failed_claims, unresolved_claims, contradicted_claims: Vec<String> - claims
    - failed_steps: Vec<String> - steps whose operation failed or never started
    - infrastructure: bool - the failure names an unavailable or unhealthy system
    - attempts: u32 - dispatch attempts (retries inside one trial)
    - events: usize - DataSea lifecycle events correlated (never counted as trials)
    - evidence: Vec<EvidenceRef> - evidence and its access
    - synthetic: bool - from a labelled fixture, not a live ledger
    - duplicate_of: Option<String> - representative observation when collapsed
    - at: u64 - Unix milliseconds of the last update
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Observation {
    pub(crate) invocation_id: String,
    pub(crate) record_digest: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) capability_digest: String,
    pub(crate) plan_digest: Option<String>,
    pub(crate) independence_key: String,
    pub(crate) state: InvocationState,
    pub(crate) class: OutcomeClass,
    pub(crate) context: ObservationContext,
    pub(crate) verified_claims: Vec<String>,
    pub(crate) failed_claims: Vec<String>,
    pub(crate) unresolved_claims: Vec<String>,
    pub(crate) contradicted_claims: Vec<String>,
    pub(crate) failed_steps: Vec<String>,
    pub(crate) infrastructure: bool,
    pub(crate) attempts: u32,
    pub(crate) events: usize,
    pub(crate) evidence: Vec<EvidenceRef>,
    pub(crate) synthetic: bool,
    pub(crate) duplicate_of: Option<String>,
    pub(crate) at: u64,
}

/** The availability of one source
 * Fields
    - source: String - source name
    - available: bool - whether it could be read
    - synthetic: bool - fixture data, not live production evidence
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourceCoverage {
    pub(crate) source: String,
    pub(crate) available: bool,
    pub(crate) synthetic: bool,
    pub(crate) detail: String,
}

/** What an analysis could read and count
 * Fields
    - records_for_capability: usize - ledger records of the capability (any version)
    - records_visible: usize - of the analyzed version, visible to the actor
    - records_not_visible: usize - of the analyzed version, other requesters' (not read)
    - other_versions: usize - records of other versions (excluded)
    - other_digest: usize - records invoking a different digest (excluded)
    - independent: usize - independent trials after collapsing resubmissions
    - duplicates_collapsed: usize - resubmissions of the same trial
    - retry_attempts_excluded: u32 - dispatch retries inside trials (never counted)
    - events_correlated: usize - DataSea lifecycle events correlated (never counted as trials)
    - restricted_evidence: usize - evidence references the actor may not read
    - missing_evidence: usize - evidence references that do not resolve
    - sources: Vec<SourceCoverage> - sources and their availability
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Coverage {
    pub(crate) records_for_capability: usize,
    pub(crate) records_visible: usize,
    pub(crate) records_not_visible: usize,
    pub(crate) other_versions: usize,
    pub(crate) other_digest: usize,
    pub(crate) independent: usize,
    pub(crate) duplicates_collapsed: usize,
    pub(crate) retry_attempts_excluded: u32,
    pub(crate) events_correlated: usize,
    pub(crate) restricted_evidence: usize,
    pub(crate) missing_evidence: usize,
    pub(crate) sources: Vec<SourceCoverage>,
}

/** What is known about one dependency's drift
 * Variants
    - Unchanged - the pinned version is current
    - Changed - observed: the current version differs
    - Missing - observed: the target no longer exists
    - Unknown - the current version cannot be observed (never "no drift")
    - SuspectedAtInvocation - some invocations ran under another version than the pin
    - NoLongerEligible - the capability version itself is withdrawn; it is not revised
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DriftStatus {
    Unchanged,
    Changed,
    Missing,
    Unknown,
    SuspectedAtInvocation,
    NoLongerEligible,
}

/** One drift finding
 * Fields
    - kind: String - zone, flow, policy, capability, evidence_pack, lifecycle
    - target_id: String - target
    - required: bool - whether the dependency is required
    - pinned: Option<String> - pinned version
    - observed: Option<String> - currently observed version, None when unknown
    - status: DriftStatus - status
    - source: String - where the current value was read
    - impact: String - consequence
    - invocations: Vec<String> - invocations that ran under another version (suspected drift)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DriftFinding {
    pub(crate) kind: String,
    pub(crate) target_id: String,
    pub(crate) required: bool,
    pub(crate) pinned: Option<String>,
    pub(crate) observed: Option<String>,
    pub(crate) status: DriftStatus,
    pub(crate) source: String,
    pub(crate) impact: String,
    pub(crate) invocations: Vec<String>,
}

/** A drift report (computed on demand; never rewrites constraints)
 * Fields
    - capability_id, version, digest - exact version
    - state: LifecycleState - its lifecycle state
    - landscape_known, crane_known: bool - whether current versions could be read
    - findings: Vec<DriftFinding> - findings
    - summary: String - decision
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DriftReport {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) state: LifecycleState,
    pub(crate) landscape_known: bool,
    pub(crate) crane_known: bool,
    pub(crate) findings: Vec<DriftFinding>,
    pub(crate) summary: String,
}

/** What a signal is about
 * Variants
    - RepeatedFailure - several independent established failures in one context
    - DiagnosticFailure - one established failure (never a recurring pattern)
    - VerificationGap - outcomes repeatedly not established
    - ContradictoryOutcomes - checks disagree about outcomes
    - VerifiedSuccessPattern - repeated verified successes (no change implied)
    - DependencyDrift - a pinned dependency changed or disappeared
    - ContextUnavailable - the current context cannot be observed
    - NoLongerEligible - the version is withdrawn; it is not revised
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SignalKind {
    RepeatedFailure,
    DiagnosticFailure,
    VerificationGap,
    ContradictoryOutcomes,
    VerifiedSuccessPattern,
    DependencyDrift,
    ContextUnavailable,
    NoLongerEligible,
}

/** The status of one qualification rule
 * Variants
    - Met - the rule holds
    - NotMet - the rule does not hold
    - Unknown - the rule cannot be evaluated (treated as not met)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RuleStatus {
    Met,
    NotMet,
    Unknown,
}

/** One evaluated qualification rule
 * Fields
    - rule: String - rule name
    - status: RuleStatus - status
    - detail: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuleResult {
    pub(crate) rule: String,
    pub(crate) status: RuleStatus,
    pub(crate) detail: String,
}

/** Configurable, deterministic qualification thresholds (conservative defaults; they are
 * decision rules, not statistical confidence)
 * Fields
    - min_independent: u32 - independent observations for a pattern (low and medium risk)
    - min_independent_elevated: u32 - for high, critical, or unknown risk
    - min_failure_share_percent: u32 - failures among established outcomes in the context
    - min_gap_observations: u32 - independent verification gaps for a gap pattern
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Thresholds {
    pub(crate) min_independent: u32,
    pub(crate) min_independent_elevated: u32,
    pub(crate) min_failure_share_percent: u32,
    pub(crate) min_gap_observations: u32,
}

impl Default for Thresholds {
    /** The conservative defaults
     * Input
        - None
     * Output
        - Thresholds
    */
    fn default() -> Self {
        Self {
            min_independent: 3,
            min_independent_elevated: 5,
            min_failure_share_percent: 75,
            min_gap_observations: 3,
        }
    }
}

/** The qualification result of a signal
 * Fields
    - qualified: bool - every rule met
    - rules: Vec<RuleResult> - evaluated rules
    - thresholds: Thresholds - thresholds applied
    - explanation: String - decision
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Qualification {
    pub(crate) qualified: bool,
    pub(crate) rules: Vec<RuleResult>,
    pub(crate) thresholds: Thresholds,
    pub(crate) explanation: String,
}

/** A bounded revision hypothesis (it never asserts causality)
 * Fields
    - statement: String - what was observed, without a causal claim
    - causality: String - what is and is not established about cause
    - assumptions: Vec<String> - assumptions
    - questions: Vec<String> - unresolved questions
    - recommendation: String - what the engine recommends
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Hypothesis {
    pub(crate) statement: String,
    pub(crate) causality: String,
    pub(crate) assumptions: Vec<String>,
    pub(crate) questions: Vec<String>,
    pub(crate) recommendation: String,
}

/** An evolution signal
 * Fields
    - signal_id: String - identity (capability, version, digest, kind, context, detector version,
      evidence set)
    - schema_version: u32 - EVOLUTION_SCHEMA_VERSION
    - detector_version: String - DETECTOR_VERSION
    - capability_id, version, capability_digest - exact version
    - kind: SignalKind - kind
    - context_key: String - comparable context
    - state: SignalState - state
    - transitions: Vec<Transition<SignalState>> - history
    - supporting: Vec<String> - supporting invocation ids (independent)
    - contradicting: Vec<String> - contradicting invocation ids
    - collapsed: Vec<String> - resubmissions collapsed into the supporting ones
    - independent_count: usize - independent supporting observations
    - affected_claims, affected_steps: Vec<String> - claims and steps involved
    - evidence: Vec<EvidenceRef> - evidence
    - missing_evidence: Vec<String> - evidence that is missing or restricted
    - drift: Vec<DriftFinding> - drift findings (drift signals)
    - qualification: Option<Qualification> - qualification
    - hypothesis: Option<Hypothesis> - hypothesis (qualified signals)
    - evidence_set_digest: String - digest of the supporting and contradicting records
    - analysis_id: String - analysis that detected it
    - superseded_by: Option<String> - newer signal
    - limitations: Vec<String> - limitations
    - created_at, updated_at: u64 - Unix milliseconds
    - digest: String - content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Signal {
    pub(crate) signal_id: String,
    pub(crate) schema_version: u32,
    pub(crate) detector_version: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) capability_digest: String,
    pub(crate) kind: SignalKind,
    pub(crate) context_key: String,
    pub(crate) state: SignalState,
    pub(crate) transitions: Vec<Transition<SignalState>>,
    pub(crate) supporting: Vec<String>,
    pub(crate) contradicting: Vec<String>,
    pub(crate) collapsed: Vec<String>,
    pub(crate) independent_count: usize,
    pub(crate) affected_claims: Vec<String>,
    pub(crate) affected_steps: Vec<String>,
    pub(crate) evidence: Vec<EvidenceRef>,
    pub(crate) missing_evidence: Vec<String>,
    pub(crate) drift: Vec<DriftFinding>,
    pub(crate) qualification: Option<Qualification>,
    pub(crate) hypothesis: Option<Hypothesis>,
    pub(crate) evidence_set_digest: String,
    pub(crate) analysis_id: String,
    pub(crate) superseded_by: Option<String>,
    pub(crate) limitations: Vec<String>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) digest: String,
}

/** One typed change. The first group is supported (subject to the gate); the second group is
 * always refused, and exists so a suggestion that asks for it is recorded and blocked
 * Variants
    - AddKnownFailureCase - record an observed failure (known failure case and negative case)
    - AddLimitation - add a limitation
    - AddUnresolvedAssumption - add an open question or evidence requirement
    - AddExclusion - tighten applicability (task_class, environment, or entity)
    - AddVerificationRequirement - add a required claim (unresolved until validated) whose method
      was observed working in invocation verification
    - UpdateDependency - re-pin a zone or flow dependency to the observed current version
    - ClarifyStep - reword a step's description from observed evidence (kind unchanged)
    - RaiseRisk - raise the risk level
    - ChangePolicy - edit or re-pin a policy (refused: governance)
    - LowerRisk - lower the risk level (refused)
    - LowerReview - lower the review level (refused)
    - RemoveClaim - remove a verification claim (refused)
    - MarkClaimVerified - set a claim's status to verified (refused: only evidence verifies)
    - AddEnvironment - broaden applicability to an environment (refused without evidence)
    - AddTaskClass - broaden applicability to a task class (refused without evidence)
    - AddStep - add a procedure step (refused: speculative)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ChangeKind {
    AddKnownFailureCase {
        text: String,
    },
    AddLimitation {
        text: String,
    },
    AddUnresolvedAssumption {
        text: String,
    },
    AddExclusion {
        exclusion_kind: String,
        value: String,
        reason: String,
    },
    AddVerificationRequirement {
        claim_id: String,
        statement: String,
        expected_result: String,
        method: String,
    },
    UpdateDependency {
        dependency: DependencyKind,
        target_id: String,
        from: String,
        to: String,
    },
    ClarifyStep {
        step_id: String,
        description: String,
    },
    RaiseRisk {
        to: RiskLevel,
    },
    ChangePolicy {
        target_id: String,
        detail: String,
    },
    LowerRisk {
        to: RiskLevel,
    },
    LowerReview {
        to: ReviewLevel,
    },
    RemoveClaim {
        claim_id: String,
    },
    MarkClaimVerified {
        claim_id: String,
    },
    AddEnvironment {
        environment: String,
    },
    AddTaskClass {
        task_class: String,
    },
    AddStep {
        description: String,
    },
}

/** One proposed change with its justification
 * Fields
    - change: ChangeKind - the change
    - rationale: String - why
    - evidence: Vec<String> - invocation ids and evidence references supporting it
    - origin: String - "detector" (deterministic) or "suggestion" (an external analyzer or a person;
      a hypothesis only)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Change {
    pub(crate) change: ChangeKind,
    pub(crate) rationale: String,
    pub(crate) evidence: Vec<String>,
    #[serde(default = "suggestion")]
    pub(crate) origin: String,
}

/** The default origin of a change read from a file
 * Input
    - None
 * Output
    - String
*/
fn suggestion() -> String {
    "suggestion".into()
}

/** The gate's decision on one change
 * Fields
    - index: usize - change position
    - allowed: bool - whether it may be applied
    - reason: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct GateDecision {
    pub(crate) index: usize,
    pub(crate) allowed: bool,
    pub(crate) reason: String,
}

/** A candidate version linked to a proposal
 * Fields
    - version: u32 - candidate version
    - digest: String - its definition digest
    - previous_version: u32 - its registry predecessor (the parent)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CandidateLink {
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) previous_version: u32,
}

/** A prepared review packet (an export: nothing is delivered and nobody is notified)
 * Fields
    - prepared_by: Actor - who prepared it
    - prepared_at: u64 - Unix milliseconds
    - run_id: String - the passed regression run it relies on
    - candidate_state: LifecycleState - the candidate's registry state (unchanged)
    - next_steps: Vec<String> - the existing authorized commands that remain to be run
    - notes: Vec<String> - what the packet does not do
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReviewPacket {
    pub(crate) prepared_by: Actor,
    pub(crate) prepared_at: u64,
    pub(crate) run_id: String,
    pub(crate) candidate_state: LifecycleState,
    pub(crate) next_steps: Vec<String>,
    pub(crate) notes: Vec<String>,
}

/** A revision proposal
 * Fields
    - proposal_id: String - identity (signal and change set)
    - schema_version: u32 - EVOLUTION_SCHEMA_VERSION
    - signal_id: String - motivating signal
    - capability_id, parent_version, parent_digest - exact parent
    - hypothesis: Hypothesis - bounded hypothesis
    - changes: Vec<Change> - change set
    - change_set_digest: String - digest of the change set
    - gate: Vec<GateDecision> - gate decisions
    - affected_claims, affected_steps: Vec<String> - what the revision touches
    - risks: Vec<String> - risks of the revision
    - unsupported: Vec<String> - refused or unsupported changes
    - intended: Option<CapabilityDefinition> - the exact definition to store (kept for recovery)
    - candidate: Option<CandidateLink> - the stored candidate
    - state: ProposalState - state
    - transitions: Vec<Transition<ProposalState>> - history
    - proposer: Actor - who proposed it
    - runs: Vec<String> - regression runs, oldest first
    - plan_id: Option<String> - current regression plan
    - review: Option<ReviewPacket> - prepared review packet
    - created_at, updated_at: u64 - Unix milliseconds
    - digest: String - content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub(crate) proposal_id: String,
    pub(crate) schema_version: u32,
    pub(crate) signal_id: String,
    pub(crate) capability_id: String,
    pub(crate) parent_version: u32,
    pub(crate) parent_digest: String,
    pub(crate) hypothesis: Hypothesis,
    pub(crate) changes: Vec<Change>,
    pub(crate) change_set_digest: String,
    pub(crate) gate: Vec<GateDecision>,
    pub(crate) affected_claims: Vec<String>,
    pub(crate) affected_steps: Vec<String>,
    pub(crate) risks: Vec<String>,
    pub(crate) unsupported: Vec<String>,
    pub(crate) intended: Option<CapabilityDefinition>,
    pub(crate) candidate: Option<CandidateLink>,
    pub(crate) state: ProposalState,
    pub(crate) transitions: Vec<Transition<ProposalState>>,
    pub(crate) proposer: Actor,
    pub(crate) runs: Vec<String>,
    pub(crate) plan_id: Option<String>,
    pub(crate) review: Option<ReviewPacket>,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
    pub(crate) digest: String,
}

/** A regression dimension
 * Variants
    - Structural - schema, identity, parent linkage, registry dry-run gates
    - ClaimIntegrity - claims preserved, recomputed, never upgraded by authorship
    - Applicability - scope explicit; verified contexts still apply; unknown never matches
    - VerificationCoverage - required verification kept; new requirements supported
    - HistoricalFailure - observed failures and negative cases represented
    - Governance - risk, review, policy, and dependencies not weakened
    - Behavioral - parent-versus-candidate behavior (only identity of procedures is decidable)
    - Lab - Lab checks compared between parent and candidate
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Dimension {
    Structural,
    ClaimIntegrity,
    Applicability,
    VerificationCoverage,
    HistoricalFailure,
    Governance,
    Behavioral,
    Lab,
}

/** One regression check result
 * Fields
    - check_id: String - identifier
    - dimension: Dimension - dimension
    - required: bool - whether the aggregate depends on it
    - status: CheckStatus - passed, failed, not_run, blocked, inconclusive
    - parent_status: Option<CheckStatus> - the same check on the parent, when compared
    - detail: String - what was checked and found
    - counterexample: Option<Value> - what contradicts it
    - evidence_refs: Vec<String> - evidence relied on
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegressionCheck {
    pub(crate) check_id: String,
    pub(crate) dimension: Dimension,
    pub(crate) required: bool,
    pub(crate) status: CheckStatus,
    pub(crate) parent_status: Option<CheckStatus>,
    pub(crate) detail: String,
    pub(crate) counterexample: Option<Value>,
    pub(crate) evidence_refs: Vec<String>,
}

/** A versioned regression plan
 * Fields
    - plan_id: String - identity (proposal, parent, candidate, plan content)
    - schema_version: u32 - EVOLUTION_SCHEMA_VERSION
    - regression_version: String - REGRESSION_VERSION
    - proposal_id: String - proposal
    - capability_id: String - capability
    - parent_version, parent_digest - exact parent
    - candidate_version, candidate_digest - exact candidate
    - lab_plan: TestPlan - the Lab test plan (supported scenario kinds only)
    - historical_contexts: Vec<String> - contexts with verified successes kept applicable
    - dropped: Vec<String> - generated scenarios whose expectation does not hold on the parent
    - dimensions: Vec<Dimension> - dimensions evaluated
    - unsupported: Vec<String> - comparisons the available mechanisms cannot make
    - created_at: u64 - Unix milliseconds
    - digest: String - content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegressionPlan {
    pub(crate) plan_id: String,
    pub(crate) schema_version: u32,
    pub(crate) regression_version: String,
    pub(crate) proposal_id: String,
    pub(crate) capability_id: String,
    pub(crate) parent_version: u32,
    pub(crate) parent_digest: String,
    pub(crate) candidate_version: u32,
    pub(crate) candidate_digest: String,
    pub(crate) lab_plan: TestPlan,
    pub(crate) historical_contexts: Vec<String>,
    pub(crate) dropped: Vec<String>,
    pub(crate) dimensions: Vec<Dimension>,
    pub(crate) unsupported: Vec<String>,
    pub(crate) created_at: u64,
    pub(crate) digest: String,
}

/** A regression run report (immutable once stored; a retry is a new run)
 * Fields
    - run_id: String - identity
    - schema_version: u32 - EVOLUTION_SCHEMA_VERSION
    - regression_version: String - REGRESSION_VERSION
    - proposal_id, plan_id, plan_digest - what ran
    - capability_id: String - capability
    - parent_version, parent_digest, candidate_version, candidate_digest - exact inputs
    - governance: CurrentVersions - versions current during the run
    - inputs_digest: String - digest of every input identity
    - previous_run: Option<String> - the run this one retries
    - inputs_changed: Option<bool> - whether the inputs differ from the previous run's
    - state: RunState - final state
    - transitions: Vec<Transition<RunState>> - history
    - checks: Vec<RegressionCheck> - check matrix
    - lab_reports: Vec<(String, String)> - Lab reports (id, digest): parent, then candidate
    - skipped: Vec<String> - checks not run and why
    - counterexamples: Vec<Value> - counterexamples
    - limitations: Vec<String> - limitations
    - reason: String - summary of the outcome
    - actor: Actor - who ran it
    - created_at: u64 - Unix milliseconds
    - digest: String - content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RegressionRun {
    pub(crate) run_id: String,
    pub(crate) schema_version: u32,
    pub(crate) regression_version: String,
    pub(crate) proposal_id: String,
    pub(crate) plan_id: String,
    pub(crate) plan_digest: String,
    pub(crate) capability_id: String,
    pub(crate) parent_version: u32,
    pub(crate) parent_digest: String,
    pub(crate) candidate_version: u32,
    pub(crate) candidate_digest: String,
    pub(crate) governance: CurrentVersions,
    pub(crate) inputs_digest: String,
    pub(crate) previous_run: Option<String>,
    pub(crate) inputs_changed: Option<bool>,
    pub(crate) state: RunState,
    pub(crate) transitions: Vec<Transition<RunState>>,
    pub(crate) checks: Vec<RegressionCheck>,
    pub(crate) lab_reports: Vec<(String, String)>,
    pub(crate) skipped: Vec<String>,
    pub(crate) counterexamples: Vec<Value>,
    pub(crate) limitations: Vec<String>,
    pub(crate) reason: String,
    pub(crate) actor: Actor,
    pub(crate) created_at: u64,
    pub(crate) digest: String,
}

/** A summary of one comparable context
 * Fields
    - context_key: String - context
    - independent: usize - independent trials
    - counts: BTreeMap<OutcomeClass, usize> - independent trials by class
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PartitionSummary {
    pub(crate) context_key: String,
    pub(crate) independent: usize,
    pub(crate) counts: BTreeMap<OutcomeClass, usize>,
}

/** An evolution analysis report (immutable; identical inputs give the same report)
 * Fields
    - report_id: String - identity (version, digest, detector, thresholds, observation set,
      governance)
    - schema_version: u32 - EVOLUTION_SCHEMA_VERSION
    - detector_version: String - DETECTOR_VERSION
    - capability_id, version, capability_digest - exact version
    - risk: RiskLevel - its risk
    - thresholds: Thresholds - thresholds
    - coverage: Coverage - what was read and counted
    - outcome_counts: BTreeMap<OutcomeClass, usize> - independent trials by class
    - partitions: Vec<PartitionSummary> - comparable contexts
    - observations: Vec<Observation> - observations (duplicates marked)
    - drift: DriftReport - drift
    - signals: Vec<(String, SignalKind, SignalState)> - signals and their state after analysis
    - decision: String - overall decision
    - limitations: Vec<String> - limitations
    - actor: Actor - who analyzed
    - created_at: u64 - Unix milliseconds
    - digest: String - content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AnalysisReport {
    pub(crate) report_id: String,
    pub(crate) schema_version: u32,
    pub(crate) detector_version: String,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) capability_digest: String,
    pub(crate) risk: RiskLevel,
    pub(crate) thresholds: Thresholds,
    pub(crate) coverage: Coverage,
    pub(crate) outcome_counts: BTreeMap<OutcomeClass, usize>,
    pub(crate) partitions: Vec<PartitionSummary>,
    pub(crate) observations: Vec<Observation>,
    pub(crate) drift: DriftReport,
    pub(crate) signals: Vec<(String, SignalKind, SignalState)>,
    pub(crate) decision: String,
    pub(crate) limitations: Vec<String>,
    pub(crate) actor: Actor,
    pub(crate) created_at: u64,
    pub(crate) digest: String,
}

/** A record sealed with a digest over its content (the digest field itself excluded) */
pub(crate) trait Sealed: Serialize + Clone {
    /** The digest domain
     * Input
        - None
     * Output
        - &'static str
    */
    fn domain() -> &'static str;

    /** The digest field
     * Input
        - None (uses self)
     * Output
        - &mut String
    */
    fn digest_field(&mut self) -> &mut String;

    /** Compute the content digest
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn compute(&self) -> String {
        let mut content = self.clone();
        content.digest_field().clear();
        digest_json(Self::domain(), &json!(content))[..32].to_string()
    }

    /** Seal the record
     * Input
        - None (uses self)
     * Output
        - None
    */
    fn seal(&mut self) {
        let digest = self.compute();
        *self.digest_field() = digest;
    }

    /** Check the record against its digest
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn verify(&self) -> bool {
        let mut copy = self.clone();
        let stored = copy.digest_field().clone();
        stored == self.compute()
    }
}

/** Implement Sealed for a record with a "digest" field
 * Input
    - $type: a record type
    - $domain: its digest domain
 * Output
    - an impl of Sealed
*/
macro_rules! sealed {
    ($type:ty, $domain:literal) => {
        impl Sealed for $type {
            /** The digest domain
             * Input
                - None
             * Output
                - &'static str
            */
            fn domain() -> &'static str {
                $domain
            }

            /** The digest field
             * Input
                - None (uses self)
             * Output
                - &mut String
            */
            fn digest_field(&mut self) -> &mut String {
                &mut self.digest
            }
        }
    };
}

sealed!(Signal, "evolution/signal/v1");
sealed!(Proposal, "evolution/proposal/v1");
sealed!(RegressionPlan, "evolution/regression-plan/v1");
sealed!(RegressionRun, "evolution/regression-run/v1");
sealed!(AnalysisReport, "evolution/analysis/v1");

/** A content digest of any serializable value
 * Input
    - domain: &str - digest domain
    - value: &impl Serialize - value
 * Output
    - String
*/
pub(crate) fn digest_of(domain: &str, value: &impl Serialize) -> String {
    digest_json(domain, &json!(value))[..32].to_string()
}
