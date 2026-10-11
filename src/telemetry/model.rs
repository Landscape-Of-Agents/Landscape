// The security taxonomy shared by events, metrics, findings, and the dashboard. The vocabularies
// are deliberately separate: an authority decision (what Crane allowed), an execution outcome
// (what happened), a security assessment (whether it complied), an enforcement outcome (how the
// control performed), and a telemetry status (how the fact is known). A denied attack is never a
// breach, a missing measurement is never zero, and absence of evidence is never evidence of absence.

use serde::{Deserialize, Serialize};

/** Authority decision for an action
 * Variants
    - Allow - the action may run
    - Deny - the action must not run
    - RequireApproval - a human must approve it first
    - Quarantine - the session is quarantined; the action must not run
    - Error - no decision could be made; treated as a denial (fail closed)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Decision {
    Allow,
    Deny,
    RequireApproval,
    Quarantine,
    Error,
}

/** Execution outcome of an action
 * Variants
    - Success - it ran and succeeded
    - Failure - it ran and failed
    - Partial - it partly ran
    - NotExecuted - it did not run (denied, or never reported)
    - Unknown - it is not known whether it ran
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Execution {
    Success,
    Failure,
    Partial,
    NotExecuted,
    Unknown,
}

/** Security assessment of an action or state
 * Variants
    - Compliant - verified to comply
    - Violation - verified to violate a policy
    - Suspicious - an attempt or pattern worth review (such as a denied bypass attempt)
    - Unresolved - compliance could not be established
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Assessment {
    Compliant,
    Violation,
    Suspicious,
    Unresolved,
}

/** How the enforcement control performed
 * Variants
    - Prevented - a prohibited action was stopped before it ran
    - DetectedAfterEffect - a violation was found only after the effect happened
    - BypassConfirmed - a protected resource changed without passing the enforcement boundary
    - CoverageGap - the action was not (or could not be) observed or decided
    - NotApplicable - no enforcement was involved
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Enforcement {
    Prevented,
    DetectedAfterEffect,
    BypassConfirmed,
    CoverageGap,
    NotApplicable,
}

/** How a fact or measurement is known
 * Variants
    - Observed - measured directly
    - Inferred - derived from other observations
    - Estimated - approximated
    - Unsupported - the source cannot provide it
    - Redacted - known but withheld
    - Unobserved - not measured (it may have happened)
    - Lost - measured but the record was lost
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Telemetry {
    Observed,
    Inferred,
    Estimated,
    Unsupported,
    Redacted,
    Unobserved,
    Lost,
}

/** The twelve security domains every event and metric belongs to
 * Variants
    - IdentityProvenance - who and what acted, and where identifiers came from
    - AuthenticationDelegation - credentials and delegated authority
    - Authorization - allow, deny, and approval decisions
    - DataAccessPrivacy - reads of data and personal information
    - SecretsCredentials - secrets found in actions or outputs
    - RuntimeIntegrity - processes and executables
    - NetworkExfiltration - network destinations and transfers
    - PromptInjection - instruction integrity
    - ResourceAbuse - availability, budgets, and autonomy credits
    - PolicyIntegrity - policies, registry, markers, and checkpoints
    - RecoveryOversight - quarantine, recovery, and human intervention
    - AuditCompliance - evidence, coverage, and attestations
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Domain {
    IdentityProvenance,
    AuthenticationDelegation,
    Authorization,
    DataAccessPrivacy,
    SecretsCredentials,
    RuntimeIntegrity,
    NetworkExfiltration,
    PromptInjection,
    ResourceAbuse,
    PolicyIntegrity,
    RecoveryOversight,
    AuditCompliance,
}

impl Domain {
    /** Every domain, in reporting order */
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
    pub(crate) const ALL: [Domain; 12] = [
        Self::IdentityProvenance,
        Self::AuthenticationDelegation,
        Self::Authorization,
        Self::DataAccessPrivacy,
        Self::SecretsCredentials,
        Self::RuntimeIntegrity,
        Self::NetworkExfiltration,
        Self::PromptInjection,
        Self::ResourceAbuse,
        Self::PolicyIntegrity,
        Self::RecoveryOversight,
        Self::AuditCompliance,
    ];
}

/** Event severity
 * Variants
    - Info - routine activity
    - Low - minor issue
    - Medium - worth attention
    - High - a violation or failure
    - Critical - tampering, confirmed bypass, or quarantine
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Severity {
    Info,
    Low,
    Medium,
    High,
    Critical,
}

/** Session safety state, separate from the autonomy mode and the credit balance
 * Variants
    - Active - normal operation
    - Degraded - a violation was detected; mutating actions need approval
    - Quarantined - a bypass or repeated bypass attempts; mutating actions are denied
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Safety {
    Active,
    Degraded,
    Quarantined,
}
