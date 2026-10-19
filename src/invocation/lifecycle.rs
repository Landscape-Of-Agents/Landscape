// The invocation lifecycle. Three status dimensions are kept apart: the invocation's state (this
// table), each operation's status, and each operation's external-effect status, so an interrupted
// invocation can still record that an external operation already succeeded.
//
//   requested               -> authorizing | blocked
//   authorizing             -> preflighting | blocked
//   preflighting            -> plan_ready | blocked
//   plan_ready              -> awaiting_approval | executing | blocked | abandoned
//   awaiting_approval       -> executing | blocked | abandoned
//   executing               -> verifying | failed | reconciliation_required | blocked | abandoned
//   verifying               -> succeeded | failed | partially_verified | unknown
//   reconciliation_required -> executing | verifying | failed | unknown | abandoned
//   unknown                 -> reconciliation_required | abandoned
//   succeeded, failed, partially_verified, blocked, abandoned: terminal
//
// executing -> blocked is allowed only before any operation was dispatched (the engine enforces
// it); abandoned requires the remaining effects to be recorded.

use serde::{Deserialize, Serialize};

/** The state of an invocation
 * Variants
    - Requested, Authorizing, Preflighting, PlanReady, AwaitingApproval, Executing, Verifying -
      the forward stages
    - ReconciliationRequired - an operation's external effect is uncertain
    - Succeeded - every mandatory postcondition is supported by sufficient evidence
    - Failed - a required operation or postcondition definitively failed, and that is understood
    - PartiallyVerified - some required outcomes are established, others remain unresolved
    - Unknown - the external effect or final state cannot yet be determined
    - Blocked - execution was prevented before the relevant operation started
    - Abandoned - deliberately stopped, with unresolved effects recorded
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InvocationState {
    Requested,
    Authorizing,
    Preflighting,
    PlanReady,
    AwaitingApproval,
    Executing,
    Verifying,
    ReconciliationRequired,
    Succeeded,
    Failed,
    PartiallyVerified,
    Unknown,
    Blocked,
    Abandoned,
}

impl InvocationState {
    /** Report whether the state is terminal
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded
                | Self::Failed
                | Self::PartiallyVerified
                | Self::Blocked
                | Self::Abandoned
        )
    }
}

/** Every state, for exhaustive transition tests */
#[cfg_attr(not(test), allow(dead_code))] // enumerates the table for the transition tests
pub(crate) const ALL_STATES: [InvocationState; 14] = [
    InvocationState::Requested,
    InvocationState::Authorizing,
    InvocationState::Preflighting,
    InvocationState::PlanReady,
    InvocationState::AwaitingApproval,
    InvocationState::Executing,
    InvocationState::Verifying,
    InvocationState::ReconciliationRequired,
    InvocationState::Succeeded,
    InvocationState::Failed,
    InvocationState::PartiallyVerified,
    InvocationState::Unknown,
    InvocationState::Blocked,
    InvocationState::Abandoned,
];

/** Report whether a transition is in the table
 * Input
    - from: InvocationState - current state
    - to: InvocationState - requested state
 * Output
    - bool
*/
pub(crate) fn allowed(from: InvocationState, to: InvocationState) -> bool {
    use InvocationState::{
        Abandoned, Authorizing, AwaitingApproval, Blocked, Executing, Failed, PartiallyVerified,
        PlanReady, Preflighting, ReconciliationRequired, Requested, Succeeded, Unknown, Verifying,
    };
    matches!(
        (from, to),
        (Requested, Authorizing | Blocked)
            | (Authorizing, Preflighting | Blocked)
            | (Preflighting, PlanReady | Blocked)
            | (
                PlanReady,
                AwaitingApproval | Executing | Blocked | Abandoned
            )
            | (AwaitingApproval, Executing | Blocked | Abandoned)
            | (
                Executing,
                Verifying | Failed | ReconciliationRequired | Blocked | Abandoned
            )
            | (Verifying, Succeeded | Failed | PartiallyVerified | Unknown)
            | (
                ReconciliationRequired,
                Executing | Verifying | Failed | Unknown | Abandoned
            )
            | (Unknown, ReconciliationRequired | Abandoned)
    )
}

/** The status of one planned operation
 * Variants
    - Pending - not reached yet
    - Intended - dispatch intent persisted, not yet dispatched
    - Succeeded - the adapter reported success
    - Failed - the adapter reported a definite failure
    - TimedOut - no answer in time (see the effect status for what is known)
    - NotStarted - prevented before dispatch (precondition, limit, authorization)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationStatus {
    Pending,
    Intended,
    Succeeded,
    Failed,
    TimedOut,
    NotStarted,
}

/** What is known about an operation's external effect
 * Variants
    - None - the operation is read-only
    - NotApplied - established not to have happened
    - Applied - established to have happened
    - Unknown - not established (never assumed either way)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EffectStatus {
    None,
    NotApplied,
    Applied,
    Unknown,
}

/** The effect class of an operation
 * Variants
    - ReadOnly - reads only
    - StateChanging - changes state that the adapter can reconcile
    - ExternallyConsequential - changes external systems with consequences beyond the repository
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EffectClass {
    ReadOnly,
    StateChanging,
    ExternallyConsequential,
}
