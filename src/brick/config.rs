// Type-specific Brick configuration. A Brick stores its type and a configuration object; the
// object is parsed into the type's own schema here (unknown fields refused), so a configuration can
// never carry anything its type does not define: no scripts, no commands, no credentials. Every
// validation problem is reported with a code, never by guessing a repair.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::BrickType;
use crate::governance::zones::{Ceiling, Criticality};

/** An action class a Brick can allow, forbid, or require approval for
 * Variants
    - Read, Write, Delete, Execute, Network - the classes agent tool calls fall into
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActionClass {
    Read,
    Write,
    Delete,
    Execute,
    Network,
}

/** A deterministic check, used by preconditions and verifications
 * Variants
    - FileExists { path } - the repository file exists
    - FileAbsent { path } - the repository file does not exist
    - SelectionsResolved - every selection resolves in the working tree
    - PolicyTestsPass - every preserve and target command passes
    - SignedState - the governance state verifies against its signed manifest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Check {
    FileExists { path: String },
    FileAbsent { path: String },
    SelectionsResolved,
    PolicyTestsPass,
    SignedState,
}

/** A source a monitor can watch
 * Variants
    - GovernanceIntegrity - the signed state verifies (local)
    - SelectionsResolved - selections resolve (local)
    - PolicyTests - policy commands pass (local)
    - Denials - denied agent actions bound to the target (local events)
    - Ci, Metrics, External - no connector exists; reported as coverage gaps
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SignalSource {
    GovernanceIntegrity,
    SelectionsResolved,
    PolicyTests,
    Denials,
    Ci,
    Metrics,
    External,
}

impl SignalSource {
    /** Report whether this version has a connector for the source
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn supported(self) -> bool {
        !matches!(self, Self::Ci | Self::Metrics | Self::External)
    }
}

/** One monitored signal
 * Fields
    - source: SignalSource - what is watched
    - every_minutes: u32 - interval between evaluations (at least 5)
    - threshold: Option<u64> - for counted sources (denials), the count that raises it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Signal {
    pub(crate) source: SignalSource,
    pub(crate) every_minutes: u32,
    #[serde(default)]
    pub(crate) threshold: Option<u64>,
}

/** What a revocation Brick watches for
 * Variants
    - IntegrityFailure - the signed state stops verifying
    - VerificationContradicted - a verification check of the target fails
    - RepeatedDenials - denials in the target cross the monitor threshold
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Trigger {
    IntegrityFailure,
    VerificationContradicted,
    RepeatedDenials,
}

/** An advisory analysis
 * Variants
    - DenialPatterns - repeated denials of the same rule in the target
    - StaleContext - context unchanged while the target's code changed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Analysis {
    DenialPatterns,
    StaleContext,
}

/** What to do when a budget is exhausted or recovery fails
 * Variants
    - Stop - stop the work
    - Escalate - ask a person
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Escalation {
    Stop,
    Escalate,
}

/** A provenance record an outcome must be traceable to
 * Variants
    - SignedState - the signed manifest
    - AuditChain - the governance audit chain
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProvenanceItem {
    SignedState,
    AuditChain,
}

/** A parsed, type-specific configuration
 * Variants
    - One per Brick type, holding that type's fields (see the type catalog)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum Config {
    Context(ContextConfig),
    Identity(IdentityConfig),
    Scope(ScopeConfig),
    Permission(PermissionConfig),
    Constraint(ConstraintConfig),
    Precondition(ChecksConfig),
    Approval(ApprovalConfig),
    Isolation(IsolationConfig),
    Budget(BudgetConfig),
    Action(ActionConfig),
    Verification(ChecksConfig),
    Provenance(ProvenanceConfig),
    Recovery(RecoveryConfig),
    Revocation(RevocationConfig),
    Monitoring(MonitoringConfig),
    Intelligence(IntelligenceConfig),
}

/** CONTEXT: reviewed repository files delivered with the common context
 * Fields
    - files: Vec<String> - repository-relative files
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ContextConfig {
    pub(crate) files: Vec<String>,
}

/** IDENTITY: the agents admitted to the target
 * Fields
    - agents: Vec<String> - providers (claude, codex, copilot, gemini) or exact agent ids
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct IdentityConfig {
    pub(crate) agents: Vec<String>,
}

/** SCOPE: the files of the target agents may change
 * Fields
    - include: Vec<String> - glob patterns, repository-relative
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ScopeConfig {
    pub(crate) include: Vec<String>,
}

/** PERMISSION: the action classes allowed in the target
 * Fields
    - allow: Vec<ActionClass> - allowed classes
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PermissionConfig {
    pub(crate) allow: Vec<ActionClass>,
}

/** CONSTRAINT: hard limits
 * Fields
    - max_ceiling: Option<Ceiling> - the highest autonomy ceiling allowed
    - min_criticality: Option<Criticality> - the lowest criticality allowed
    - protected_paths: Vec<String> - glob patterns agents may not change
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ConstraintConfig {
    pub(crate) max_ceiling: Option<Ceiling>,
    pub(crate) min_criticality: Option<Criticality>,
    pub(crate) protected_paths: Vec<String>,
}

/** PRECONDITION and VERIFICATION: deterministic checks
 * Fields
    - checks: Vec<Check> - checks, all of which must hold
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ChecksConfig {
    pub(crate) checks: Vec<Check>,
}

/** APPROVAL: when a person must approve
 * Fields
    - operations: Vec<ActionClass> - classes needing approval
    - reason: String - why, shown to the person approving
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ApprovalConfig {
    pub(crate) operations: Vec<ActionClass>,
    pub(crate) reason: String,
}

/** ISOLATION: the execution boundary required
 * Fields
    - profile: String - an isolation profile id ('crane isolation profiles')
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct IsolationConfig {
    pub(crate) profile: String,
}

/** BUDGET: caps
 * Fields
    - max_tool_calls: Option<u64> - tool calls per session in the target
    - max_retries: Option<u32> - retries of a failed step
    - max_runtime_minutes: Option<u64> - running time per task
    - on_exhausted: Escalation - what happens at the cap
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct BudgetConfig {
    pub(crate) max_tool_calls: Option<u64>,
    pub(crate) max_retries: Option<u32>,
    pub(crate) max_runtime_minutes: Option<u64>,
    pub(crate) on_exhausted: Escalation,
}

impl Default for BudgetConfig {
    /** No caps; stop when exhausted
     * Input
        - None
     * Output
        - BudgetConfig
    */
    fn default() -> Self {
        Self {
            max_tool_calls: None,
            max_retries: None,
            max_runtime_minutes: None,
            on_exhausted: Escalation::Stop,
        }
    }
}

/** ACTION: a registered operation
 * Fields
    - adapter: String - registered adapter id
    - operation: String - operation id of that adapter
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ActionConfig {
    pub(crate) adapter: String,
    pub(crate) operation: String,
}

/** PROVENANCE: records an outcome must be traceable to
 * Fields
    - require: Vec<ProvenanceItem> - required records
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ProvenanceConfig {
    pub(crate) require: Vec<ProvenanceItem>,
}

/** RECOVERY: failure handling
 * Fields
    - on_failure: Escalation - stop or ask a person
    - max_retries: u32 - retries (at most 3)
    - reconcile_before_retry: bool - must be true: uncertain effects are reconciled first
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RecoveryConfig {
    pub(crate) on_failure: Escalation,
    pub(crate) max_retries: u32,
    pub(crate) reconcile_before_retry: bool,
}

impl Default for RecoveryConfig {
    /** Stop on failure, no retries, reconcile first
     * Input
        - None
     * Output
        - RecoveryConfig
    */
    fn default() -> Self {
        Self {
            on_failure: Escalation::Stop,
            max_retries: 0,
            reconcile_before_retry: true,
        }
    }
}

/** REVOCATION: when attachments should be withdrawn
 * Fields
    - triggers: Vec<Trigger> - conditions the monitors report
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RevocationConfig {
    pub(crate) triggers: Vec<Trigger>,
}

/** MONITORING: watched signals
 * Fields
    - signals: Vec<Signal> - signals
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct MonitoringConfig {
    pub(crate) signals: Vec<Signal>,
}

/** INTELLIGENCE: advisory analyses
 * Fields
    - analyses: Vec<Analysis> - analyses
    - advisory_only: bool - must be true: it proposes, never applies
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct IntelligenceConfig {
    pub(crate) analyses: Vec<Analysis>,
    pub(crate) advisory_only: bool,
}

impl Default for IntelligenceConfig {
    /** No analyses; advisory only
     * Input
        - None
     * Output
        - IntelligenceConfig
    */
    fn default() -> Self {
        Self {
            analyses: Vec::new(),
            advisory_only: true,
        }
    }
}

/** A validation problem
 * Fields
    - code: String - machine-readable code
    - message: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Problem {
    pub(crate) code: String,
    pub(crate) message: String,
}

/** Build a problem
 * Input
    - code: &str - code
    - message: impl Into<String> - explanation
 * Output
    - Problem
*/
pub(crate) fn problem(code: &str, message: impl Into<String>) -> Problem {
    Problem {
        code: code.into(),
        message: message.into(),
    }
}

/** The serialized name of an enumeration value, such as "policy_tests"
 * Input
    - value: &impl Serialize - value
 * Output
    - String
*/
pub(crate) fn serial(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/** Parse a configuration object into its type's schema
 * Input
    - brick_type: BrickType - type
    - value: &Value - configuration object (null is an empty configuration)
 * Output
    - Result<Config, Problem>
    - Error CONFIG_INVALID naming the field serde refused
*/
pub(crate) fn parse(brick_type: BrickType, value: &Value) -> Result<Config, Problem> {
    /** Parse into one schema
     * Input
        - value: &Value - object
     * Output
        - Result<T, Problem>
    */
    fn typed<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, Problem> {
        let value = if value.is_null() {
            Value::Object(serde_json::Map::new())
        } else {
            value.clone()
        };
        serde_json::from_value(value).map_err(|error| problem("CONFIG_INVALID", error.to_string()))
    }
    Ok(match brick_type {
        BrickType::Context => Config::Context(typed(value)?),
        BrickType::Identity => Config::Identity(typed(value)?),
        BrickType::Scope => Config::Scope(typed(value)?),
        BrickType::Permission => Config::Permission(typed(value)?),
        BrickType::Constraint => Config::Constraint(typed(value)?),
        BrickType::Precondition => Config::Precondition(typed(value)?),
        BrickType::Approval => Config::Approval(typed(value)?),
        BrickType::Isolation => Config::Isolation(typed(value)?),
        BrickType::Budget => Config::Budget(typed(value)?),
        BrickType::Action => Config::Action(typed(value)?),
        BrickType::Verification => Config::Verification(typed(value)?),
        BrickType::Provenance => Config::Provenance(typed(value)?),
        BrickType::Recovery => Config::Recovery(typed(value)?),
        BrickType::Revocation => Config::Revocation(typed(value)?),
        BrickType::Monitoring => Config::Monitoring(typed(value)?),
        BrickType::Intelligence => Config::Intelligence(typed(value)?),
    })
}

/** Check a repository-relative path or glob: relative, no parent segments, no backslashes, not
 * inside .git
 * Input
    - field: &str - field name, for the message
    - path: &str - path or pattern
 * Output
    - Option<Problem>
*/
pub(crate) fn path_problem(field: &str, path: &str) -> Option<Problem> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Some(problem("PATH_EMPTY", format!("{field}: an entry is empty")));
    }
    let traversal = trimmed.split('/').any(|segment| segment == "..");
    if trimmed.starts_with('/')
        || trimmed.contains('\\')
        || trimmed.contains(':')
        || traversal
        || trimmed == ".git"
        || trimmed.starts_with(".git/")
    {
        return Some(problem(
            "PATH_OUTSIDE_REPOSITORY",
            format!("{field}: '{path}' must be a repository-relative path without '..', drive letters, or backslashes"),
        ));
    }
    None
}

/** Validate a parsed configuration (beyond its schema)
 * Input
    - config: &Config - configuration
    - adapters: &[(String, String)] - registered (adapter, operation) pairs
    - profiles: &[(String, bool)] - isolation profiles and whether each is available
 * Output
    - Vec<Problem>, empty when it validates
*/
pub(crate) fn validate(
    config: &Config,
    adapters: &[(String, String)],
    profiles: &[(String, bool)],
) -> Vec<Problem> {
    let mut problems = Vec::new();
    let mut paths = |field: &str, entries: &[String]| {
        for entry in entries {
            problems.extend(path_problem(field, entry));
        }
    };
    match config {
        Config::Context(config) => paths("files", &config.files),
        Config::Scope(config) => {
            paths("include", &config.include);
            if config.include.is_empty() {
                problems.push(problem(
                    "SCOPE_EMPTY",
                    "include must name at least one pattern; an empty scope is never a wildcard",
                ));
            }
        }
        Config::Constraint(config) => {
            paths("protected_paths", &config.protected_paths);
            if config.max_ceiling.is_none()
                && config.min_criticality.is_none()
                && config.protected_paths.is_empty()
            {
                problems.push(problem(
                    "CONSTRAINT_EMPTY",
                    "a constraint needs max_ceiling, min_criticality, or protected_paths",
                ));
            }
        }
        _ => {}
    }
    match config {
        Config::Identity(config) => {
            if config.agents.is_empty() {
                problems.push(problem(
                    "IDENTITY_EMPTY",
                    "agents must name at least one provider or agent id",
                ));
            }
            for agent in &config.agents {
                if agent.trim().is_empty()
                    || !agent
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                {
                    problems.push(problem(
                        "IDENTITY_INVALID",
                        format!("'{agent}' is not a provider name or agent id"),
                    ));
                }
            }
        }
        Config::Permission(config) => {
            if config.allow.is_empty() {
                problems.push(problem(
                    "PERMISSION_EMPTY",
                    "allow must name at least one action class (an empty list would read as 'nothing', state 'read' for read-only)",
                ));
            }
        }
        Config::Precondition(config) => {
            if config.checks.is_empty() {
                problems.push(problem("CHECKS_EMPTY", "checks must name at least one check"));
            }
            for check in &config.checks {
                match check {
                    Check::FileExists { path } | Check::FileAbsent { path } => {
                        problems.extend(path_problem("checks.path", path));
                    }
                    Check::SignedState => {}
                    Check::SelectionsResolved | Check::PolicyTestsPass => problems.push(problem(
                        "PRECONDITION_TOO_COSTLY",
                        "selections_resolved and policy_tests_pass are verification checks; a precondition is evaluated before every change and may use file_exists, file_absent, or signed_state",
                    )),
                }
            }
        }
        Config::Verification(config) => {
            if config.checks.is_empty() {
                problems.push(problem("CHECKS_EMPTY", "checks must name at least one check"));
            }
            for check in &config.checks {
                if let Check::FileExists { path } | Check::FileAbsent { path } = check {
                    problems.extend(path_problem("checks.path", path));
                }
            }
        }
        Config::Approval(config) => {
            if config.operations.is_empty() {
                problems.push(problem(
                    "APPROVAL_EMPTY",
                    "operations must name at least one action class",
                ));
            }
            if config.operations.contains(&ActionClass::Read) {
                problems.push(problem(
                    "APPROVAL_READ_UNSUPPORTED",
                    "reads cannot be held for approval by the hooks; use a permission or scope Brick instead",
                ));
            }
        }
        Config::Isolation(config) => match profiles.iter().find(|(id, _)| *id == config.profile) {
            None => problems.push(problem(
                "ISOLATION_UNKNOWN_PROFILE",
                format!(
                    "'{}' is not an isolation profile; known: {}",
                    config.profile,
                    profiles
                        .iter()
                        .map(|(id, _)| id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
            Some((_, false)) => problems.push(problem(
                "ISOLATION_UNAVAILABLE",
                format!(
                    "the '{}' profile is not available in this version; attaching it makes direct agent changes observe-only",
                    config.profile
                ),
            )),
            Some(_) => {}
        },
        Config::Budget(config) => {
            if config.max_tool_calls.is_none()
                && config.max_retries.is_none()
                && config.max_runtime_minutes.is_none()
            {
                problems.push(problem("BUDGET_EMPTY", "a budget needs at least one cap"));
            }
            if config.max_tool_calls == Some(0) || config.max_runtime_minutes == Some(0) {
                problems.push(problem("BUDGET_ZERO", "a cap of 0 blocks all work; use a permission Brick to forbid it"));
            }
        }
        Config::Action(config) => {
            if config.adapter.contains(char::is_whitespace)
                || config.operation.contains(char::is_whitespace)
            {
                problems.push(problem(
                    "ACTION_NOT_REGISTERED",
                    "an action names a registered adapter and operation id, never a command line",
                ));
            } else if !adapters
                .iter()
                .any(|(adapter, operation)| *adapter == config.adapter && *operation == config.operation)
            {
                problems.push(problem(
                    "ACTION_NOT_REGISTERED",
                    format!(
                        "{}/{} is not a registered operation; registered: {}",
                        config.adapter,
                        config.operation,
                        adapters
                            .iter()
                            .map(|(adapter, operation)| format!("{adapter}/{operation}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
        }
        Config::Provenance(config) => {
            if config.require.is_empty() {
                problems.push(problem("PROVENANCE_EMPTY", "require must name at least one record"));
            }
        }
        Config::Recovery(config) => {
            if !config.reconcile_before_retry && config.max_retries > 0 {
                problems.push(problem(
                    "RECOVERY_UNSAFE_RETRY",
                    "retries require reconcile_before_retry: an uncertain effect is reconciled before it is repeated",
                ));
            }
            if config.max_retries > 3 {
                problems.push(problem("RECOVERY_TOO_MANY_RETRIES", "max_retries is at most 3"));
            }
        }
        Config::Revocation(config) => {
            if config.triggers.is_empty() {
                problems.push(problem("REVOCATION_EMPTY", "triggers must name at least one condition"));
            }
        }
        Config::Monitoring(config) => {
            if config.signals.is_empty() {
                problems.push(problem("MONITORING_EMPTY", "signals must name at least one signal"));
            }
            for signal in &config.signals {
                if signal.every_minutes < 5 {
                    problems.push(problem(
                        "MONITOR_TOO_FREQUENT",
                        "every_minutes is at least 5: monitors are cheap periodic checks, not a loop",
                    ));
                }
                if signal.source == SignalSource::Denials && signal.threshold.is_none() {
                    problems.push(problem("MONITOR_THRESHOLD_MISSING", "a denials signal needs a threshold"));
                }
            }
        }
        Config::Intelligence(config) => {
            if !config.advisory_only {
                problems.push(problem(
                    "INTELLIGENCE_NOT_ADVISORY",
                    "intelligence is advisory: it may propose, never change policy, grant authority, or approve",
                ));
            }
            if config.analyses.is_empty() {
                problems.push(problem("INTELLIGENCE_EMPTY", "analyses must name at least one analysis"));
            }
        }
        _ => {}
    }
    problems
}
