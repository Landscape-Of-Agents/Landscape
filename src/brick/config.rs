// Type-specific Brick configuration. A Brick stores its type and a configuration object; the
// object is parsed into the type's own schema here (unknown fields refused), so a configuration can
// never carry anything its type does not define: no scripts, no credentials (only their names),
// no command lines except the verification checks a person runs. A former type is converted into
// the current types first (brick::legacy), so one stored Brick parses into one or more current
// configurations. Every validation problem is reported with a code, never by guessing a repair.
//
// Sharing: a Limits, Approval, Provenance, or Watch Brick may take its files from an Access Brick
// attached to the same target ('scope_from': that Brick's id), and a Verification Brick may
// re-run a Precondition Brick's checks after the work ('checks_from'). Composition resolves them
// (brick::compose); one that does not resolve fails closed.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

use super::types::BrickType;
use crate::governance::zones::{Ceiling, Criticality};

/** An action class of the former permission and approval types
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

/** How an operation is treated
 * Variants
    - Auto - allowed without asking (the guard's other rules still apply)
    - Ask - a person must approve it
    - Deny - refused
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Mode {
    Auto,
    Ask,
    Deny,
}

/** A kind of change an Approval Brick can hold for a person
 * Variants
    - Read - reads (refused: the hooks cannot hold a read for approval)
    - Write - creating or updating a file
    - Create - creating a file that does not exist
    - Update - changing an existing file
    - Delete - deleting a file (or emptying it)
    - Execute - running a shell command
    - Network - a call that reaches the network
    - DependencyChange - a change to a dependency manifest or lock file
    - ConfigChange - a change to a configuration file (yaml, toml, ini, .env, json config)
    - MigrationChange - a change under a migrations folder
    - TestChange - a change to tests (only tests changing, or tests removed)
    - GitCommit - a git commit
    - GitPush - a git push (or force push)
    - PackageInstall - installing or adding packages (npm, pip, cargo, go, ...)
    - LargeChange - a change of more lines than min_lines_changed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeClass {
    Read,
    Write,
    Create,
    Update,
    Delete,
    Execute,
    Network,
    DependencyChange,
    ConfigChange,
    MigrationChange,
    TestChange,
    GitCommit,
    GitPush,
    PackageInstall,
    LargeChange,
}

/** How a JSON value is compared
 * Variants
    - AtLeast, AtMost, Equals - the value is at least, at most, or equal to the expected number
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Comparison {
    AtLeast,
    AtMost,
    Equals,
}

/** A local measurement a check can bound
 * Variants
    - Denials - denied agent actions in the target
    - ToolCalls - agent tool calls in the target
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalMetric {
    Denials,
    ToolCalls,
}

/** A deterministic check, used by preconditions and verifications
 * Variants
    - FileExists { path } - the repository file exists
    - FileAbsent { path } - the repository file does not exist
    - FileContains { path, text } - the file exists and contains the text
    - EnvSet { name } - the environment variable is set (where Crane runs)
    - BranchIs { patterns } - the current branch matches one pattern
    - BranchNot { patterns } - the current branch matches none of the patterns
    - GitClean - the working tree has no uncommitted changes
    - TimeWindow { days, from, to, utc_offset_minutes } - now is inside the window
    - CommandAvailable { name } - the program is on PATH
    - CredentialPresent { name } - this machine's vault holds the credential
    - SignedState - the governance state verifies against its signed manifest
    - SelectionsResolved - every selection resolves in the working tree (verification)
    - PolicyTestsPass - every preserve and target command passes (verification)
    - HttpOk { url, status, timeout_seconds } - an HTTPS endpoint answers with the status
      (verification)
    - JsonValue { path, pointer, op, value } - a number in a JSON file (a coverage or metrics
      report) compares as expected (verification)
    - LocalMetric { metric, window_hours, max } - a local measurement of the target stays under a
      maximum (verification)
    - CommandSucceeds { program, args, timeout_seconds } - a program exits with 0 (verification,
      run only by a person's 'crane brick test')
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Check {
    FileExists {
        path: String,
    },
    FileAbsent {
        path: String,
    },
    FileContains {
        path: String,
        text: String,
    },
    EnvSet {
        name: String,
    },
    BranchIs {
        patterns: Vec<String>,
    },
    BranchNot {
        patterns: Vec<String>,
    },
    GitClean,
    TimeWindow {
        #[serde(default)]
        days: Vec<String>,
        from: String,
        to: String,
        #[serde(default)]
        utc_offset_minutes: i32,
    },
    CommandAvailable {
        name: String,
    },
    CredentialPresent {
        name: String,
    },
    SignedState,
    SelectionsResolved,
    PolicyTestsPass,
    HttpOk {
        url: String,
        #[serde(default)]
        status: Option<u16>,
        #[serde(default)]
        timeout_seconds: Option<u32>,
    },
    JsonValue {
        path: String,
        pointer: String,
        op: Comparison,
        value: Number,
    },
    LocalMetric {
        metric: LocalMetric,
        #[serde(default = "day")]
        window_hours: u32,
        max: u64,
    },
    CommandSucceeds {
        program: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        timeout_seconds: Option<u32>,
    },
}

/** A window of 24 hours
 * Input
    - None
 * Output
    - u32 24
*/
fn day() -> u32 {
    24
}

impl Check {
    /** Report whether the check is cheap enough to run before every change (a precondition)
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn is_gate(&self) -> bool {
        !matches!(
            self,
            Self::SelectionsResolved
                | Self::PolicyTestsPass
                | Self::HttpOk { .. }
                | Self::JsonValue { .. }
                | Self::LocalMetric { .. }
                | Self::CommandSucceeds { .. }
        )
    }
}

/** A source a monitor can watch
 * Variants
    - GovernanceIntegrity - the signed state verifies (local)
    - SelectionsResolved - selections resolve (local)
    - PolicyTests - policy commands pass (local)
    - Denials - denied agent actions bound to the target (local events)
    - Preconditions - the target's preconditions hold (local)
    - Verification - the target's verification checks hold (local, no commands)
    - Ci, Metrics, External - no connector exists; reported as coverage gaps
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SignalSource {
    GovernanceIntegrity,
    SelectionsResolved,
    PolicyTests,
    Denials,
    Preconditions,
    Verification,
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

/** A condition under which attachments are due for withdrawal
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

/** What a firing watch does
 * Variants
    - Alert - deliver an alert through the configured integrations
    - Pause - changes in the target need a person while the signal fires
    - RevokeDue - report the attachments of the target as due for withdrawal
    - Propose - record an advisory proposal (never applied)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Reaction {
    Alert,
    Pause,
    RevokeDue,
    Propose,
}

/** What to do at a cap or after a failure
 * Variants
    - Stop - stop the work (refuse further calls)
    - Escalate - ask a person
    - Retry - retry within max_retries (recovery only; reconciled first)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Escalation {
    Stop,
    Escalate,
    Retry,
}

/** What to do with an outcome that is uncertain (it may or may not have happened)
 * Variants
    - Reconcile - read the real outcome back before anything else
    - Escalate - ask a person
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Uncertain {
    #[default]
    Reconcile,
    Escalate,
}

/** A record an outcome must be traceable to
 * Variants
    - SignedState - the signed manifest
    - AuditChain - the governance audit chain
    - DecisionsRecorded - every agent change in the target has a recorded guard decision
    - CommitTrailer - recent commits touching the target carry the trailer
    - LinkedIssue - recent commits touching the target name an issue matching issue_pattern
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProvenanceItem {
    SignedState,
    AuditChain,
    DecisionsRecorded,
    CommitTrailer,
    LinkedIssue,
}

/** A parsed, type-specific configuration
 * Variants
    - One per current Brick type, holding that type's fields (see the type catalog)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum Config {
    Knowledge(KnowledgeConfig),
    Identity(IdentityConfig),
    Access(AccessConfig),
    Approval(ApprovalConfig),
    Limits(LimitsConfig),
    Environment(EnvironmentConfig),
    Precondition(PreconditionConfig),
    Verification(VerificationConfig),
    Action(ActionConfig),
    Recovery(RecoveryConfig),
    Provenance(ProvenanceConfig),
    Watch(WatchConfig),
}

/** KNOWLEDGE: what agents know
 * Fields
    - files: Vec<String> - repository files delivered with the common context
    - glossary: BTreeMap<String, String> - terms and their meaning in this target
    - references: Vec<String> - other sources, one line each (title and link)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct KnowledgeConfig {
    pub(crate) files: Vec<String>,
    pub(crate) glossary: BTreeMap<String, String>,
    pub(crate) references: Vec<String>,
}

/** IDENTITY: who may act
 * Fields
    - agents: Vec<String> - providers (claude, codex, copilot, gemini) or exact agent ids
    - models: Vec<String> - model name patterns ("claude-opus-*"); empty admits every model
    - on_other: Mode - what a call by anyone else gets (ask or deny; auto is refused)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct IdentityConfig {
    pub(crate) agents: Vec<String>,
    pub(crate) models: Vec<String>,
    pub(crate) on_other: Mode,
}

impl Default for IdentityConfig {
    /** Nobody listed; others are denied
     * Input
        - None
     * Output
        - IdentityConfig
    */
    fn default() -> Self {
        Self {
            agents: Vec::new(),
            models: Vec::new(),
            on_other: Mode::Deny,
        }
    }
}

/** ACCESS: which files and operations
 * Fields
    - include: Vec<String> - files agents may change (empty: every file of the target); the rest of
      the target is read-only
    - exclude: Vec<String> - files carved out of include (read-only)
    - read_only: Vec<String> - files agents may read but never change
    - deny_read: Vec<String> - files agents may not even read (secrets, customer data)
    - write: Option<Mode> - creating and updating files (shorthand for create and update)
    - create: Option<Mode> - creating a file that does not exist
    - update: Option<Mode> - changing an existing file
    - delete: Option<Mode> - deleting (or emptying) a file
    - execute: Option<Mode> - shell commands that name the target's files
    - network: Option<Mode> - network calls made while working in the target
    - allow: Vec<ActionClass> - the former permission list (read as modes; do not combine)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct AccessConfig {
    pub(crate) include: Vec<String>,
    pub(crate) exclude: Vec<String>,
    pub(crate) read_only: Vec<String>,
    pub(crate) deny_read: Vec<String>,
    pub(crate) write: Option<Mode>,
    pub(crate) create: Option<Mode>,
    pub(crate) update: Option<Mode>,
    pub(crate) delete: Option<Mode>,
    pub(crate) execute: Option<Mode>,
    pub(crate) network: Option<Mode>,
    pub(crate) allow: Vec<ActionClass>,
}

impl AccessConfig {
    /** The effective mode of an operation: the former allow list first, then the specific field,
     * then the write shorthand
     * Input
        - class: ChangeClass - Create, Update, Delete, Execute, or Network
     * Output
        - Option<Mode>
    */
    pub(crate) fn mode(&self, class: ChangeClass) -> Option<Mode> {
        if !self.allow.is_empty() {
            let allowed = |action: ActionClass| self.allow.contains(&action);
            let mode = match class {
                ChangeClass::Create | ChangeClass::Update => allowed(ActionClass::Write),
                ChangeClass::Delete => allowed(ActionClass::Delete) && allowed(ActionClass::Write),
                ChangeClass::Execute => allowed(ActionClass::Execute),
                ChangeClass::Network => allowed(ActionClass::Network),
                _ => true,
            };
            return Some(if mode { Mode::Auto } else { Mode::Deny });
        }
        match class {
            ChangeClass::Create => self.create.or(self.write),
            ChangeClass::Update => self.update.or(self.write),
            ChangeClass::Delete => self.delete,
            ChangeClass::Execute => self.execute,
            ChangeClass::Network => self.network,
            _ => None,
        }
    }
}

/** APPROVAL: when a person decides
 * Fields
    - operations: Vec<ChangeClass> - kinds of change that need a person
    - paths: Vec<String> - only changes to these files (empty: the whole target)
    - except: Vec<String> - files never held by this Brick
    - min_lines_changed: Option<u32> - the size from which a change is a large_change
    - reason: String - why, shown to the person approving
    - approvers: Vec<String> - who may approve (resident agents; the agent tool asks its user)
    - min_approvals: u32 - how many approvals (resident agents)
    - distinct_approver: bool - the requester may not approve (resident agents)
    - expires_minutes: Option<u32> - how long an approval stays usable (resident agents)
    - scope_from: Option<String> - take paths from an Access Brick on the same target
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ApprovalConfig {
    pub(crate) operations: Vec<ChangeClass>,
    pub(crate) paths: Vec<String>,
    pub(crate) except: Vec<String>,
    pub(crate) min_lines_changed: Option<u32>,
    pub(crate) reason: String,
    pub(crate) approvers: Vec<String>,
    pub(crate) min_approvals: u32,
    pub(crate) distinct_approver: bool,
    pub(crate) expires_minutes: Option<u32>,
    pub(crate) scope_from: Option<String>,
}

/** LIMITS: how much and how long (per agent session, in the target)
 * Fields
    - max_tool_calls: Option<u64> - tool calls touching the target
    - max_runtime_minutes: Option<u64> - minutes since the session started
    - max_files_changed: Option<u64> - distinct files changed
    - max_lines_changed: Option<u64> - lines changed in total
    - max_commands: Option<u64> - shell commands
    - max_network_calls: Option<u64> - calls reaching the network
    - max_denials: Option<u64> - denied calls before the session must stop in the target
    - max_retries: Option<u32> - retries of a failed step (resident agents and skills)
    - max_ceiling: Option<Ceiling> - the highest autonomy agents get in the target
    - min_criticality: Option<Criticality> - the lowest criticality the target is treated with
    - on_exhausted: Escalation - stop or escalate at a cap
    - scope_from: Option<String> - count only files of an Access Brick on the same target
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LimitsConfig {
    pub(crate) max_tool_calls: Option<u64>,
    pub(crate) max_runtime_minutes: Option<u64>,
    pub(crate) max_files_changed: Option<u64>,
    pub(crate) max_lines_changed: Option<u64>,
    pub(crate) max_commands: Option<u64>,
    pub(crate) max_network_calls: Option<u64>,
    pub(crate) max_denials: Option<u64>,
    pub(crate) max_retries: Option<u32>,
    pub(crate) max_ceiling: Option<Ceiling>,
    pub(crate) min_criticality: Option<Criticality>,
    pub(crate) on_exhausted: Escalation,
    pub(crate) scope_from: Option<String>,
}

impl Default for LimitsConfig {
    /** No caps; stop when exhausted
     * Input
        - None
     * Output
        - LimitsConfig
    */
    fn default() -> Self {
        Self {
            max_tool_calls: None,
            max_runtime_minutes: None,
            max_files_changed: None,
            max_lines_changed: None,
            max_commands: None,
            max_network_calls: None,
            max_denials: None,
            max_retries: None,
            max_ceiling: None,
            min_criticality: None,
            on_exhausted: Escalation::Stop,
            scope_from: None,
        }
    }
}

/** A credential the target's Crane-run operations use, by name only
 * Fields
    - name: String - the vault entry ('crane secret set NAME'); the value never enters the repository
    - for_operations: Vec<String> - the adapters or operations it is for ("jira", "git.push")
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CredentialRef {
    pub(crate) name: String,
    #[serde(default, rename = "for")]
    pub(crate) for_operations: Vec<String>,
}

/** Network reach
 * Fields
    - allow: Vec<String> - hosts (or "*.example.com") calls may reach; empty: any not denied
    - deny: Vec<String> - hosts calls may never reach
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct NetworkRules {
    pub(crate) allow: Vec<String>,
    pub(crate) deny: Vec<String>,
}

/** ENVIRONMENT: what the work runs with
 * Fields
    - profile: String - the isolation profile ('crane isolation profiles'); agent-hooks by default
    - require_env: Vec<String> - variables that must be set where agents run
    - hide_env: Vec<String> - variables (or "AWS_*") agents may not read
    - credentials: Vec<CredentialRef> - credentials Crane-run operations use, by vault name
    - network: NetworkRules - hosts calls may and may not reach
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct EnvironmentConfig {
    pub(crate) profile: String,
    pub(crate) require_env: Vec<String>,
    pub(crate) hide_env: Vec<String>,
    pub(crate) credentials: Vec<CredentialRef>,
    pub(crate) network: NetworkRules,
}

impl Default for EnvironmentConfig {
    /** The agent-hooks profile and nothing else
     * Input
        - None
     * Output
        - EnvironmentConfig
    */
    fn default() -> Self {
        Self {
            profile: "agent-hooks".into(),
            require_env: Vec::new(),
            hide_env: Vec::new(),
            credentials: Vec::new(),
            network: NetworkRules::default(),
        }
    }
}

/** PRECONDITION: gates before acting
 * Fields
    - checks: Vec<Check> - checks that must all hold (gate checks only)
    - on_fail: Mode - ask (changes need a person) or deny (changes stop) while one fails
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct PreconditionConfig {
    pub(crate) checks: Vec<Check>,
    pub(crate) on_fail: Mode,
}

impl Default for PreconditionConfig {
    /** No checks; a failure asks a person
     * Input
        - None
     * Output
        - PreconditionConfig
    */
    fn default() -> Self {
        Self {
            checks: Vec::new(),
            on_fail: Mode::Ask,
        }
    }
}

/** VERIFICATION: proof after work
 * Fields
    - checks: Vec<Check> - checks that prove the outcome
    - checks_from: Option<String> - also re-run a Precondition Brick's checks on the same target
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct VerificationConfig {
    pub(crate) checks: Vec<Check>,
    pub(crate) checks_from: Option<String>,
}

/** One operation an Action Brick allows
 * Fields
    - adapter: String - registered adapter id
    - operation: String - operation id of that adapter
    - max_per_day: Option<u32> - runs per day for the target (resident agents and skills)
    - dry_run_first: bool - a dry run precedes every real run (resident agents and skills)
    - approval: bool - every run needs a person
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OperationRule {
    pub(crate) adapter: String,
    pub(crate) operation: String,
    #[serde(default)]
    pub(crate) max_per_day: Option<u32>,
    #[serde(default)]
    pub(crate) dry_run_first: bool,
    #[serde(default)]
    pub(crate) approval: bool,
}

/** ACTION: which side-effecting operations
 * Fields
    - operations: Vec<OperationRule> - operations Crane may run for the target
    - deny_direct: bool - agents may not do these operations themselves (e.g. 'git push' in a shell)
    - adapter: String - the former single-operation form (with operation)
    - operation: String - the former single-operation form (with adapter)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ActionConfig {
    pub(crate) operations: Vec<OperationRule>,
    pub(crate) deny_direct: bool,
    pub(crate) adapter: String,
    pub(crate) operation: String,
}

impl ActionConfig {
    /** Every operation, including the former single-operation form
     * Input
        - None (uses self)
     * Output
        - Vec<OperationRule>
    */
    pub(crate) fn rules(&self) -> Vec<OperationRule> {
        let mut rules = self.operations.clone();
        if !self.adapter.is_empty() || !self.operation.is_empty() {
            rules.push(OperationRule {
                adapter: self.adapter.clone(),
                operation: self.operation.clone(),
                max_per_day: None,
                dry_run_first: false,
                approval: false,
            });
        }
        rules
    }
}

/** RECOVERY: what happens after a failure
 * Fields
    - on_failure: Escalation - stop, escalate, or retry (within max_retries)
    - max_retries: u32 - retries (at most 3)
    - reconcile_before_retry: bool - must be true: uncertain effects are reconciled first
    - backoff_seconds: u32 - wait between retries
    - max_consecutive_failures: Option<u32> - failed agent tool calls in a row in the target before
      changes need a person
    - on_uncertain: Uncertain - reconcile or escalate an outcome that may or may not have happened
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RecoveryConfig {
    pub(crate) on_failure: Escalation,
    pub(crate) max_retries: u32,
    pub(crate) reconcile_before_retry: bool,
    pub(crate) backoff_seconds: u32,
    pub(crate) max_consecutive_failures: Option<u32>,
    pub(crate) on_uncertain: Uncertain,
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
            backoff_seconds: 0,
            max_consecutive_failures: None,
            on_uncertain: Uncertain::Reconcile,
        }
    }
}

/** PROVENANCE: what an outcome traces to
 * Fields
    - require: Vec<ProvenanceItem> - required records
    - trailer: Option<String> - the commit trailer key commit_trailer looks for ("Agent-Session")
    - issue_pattern: Option<String> - the issue pattern linked_issue looks for ("PROJ-*")
    - commits: u32 - how many recent commits touching the target are checked
    - scope_from: Option<String> - check only commits touching files of an Access Brick on the
      same target
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ProvenanceConfig {
    pub(crate) require: Vec<ProvenanceItem>,
    pub(crate) trailer: Option<String>,
    pub(crate) issue_pattern: Option<String>,
    pub(crate) commits: u32,
    pub(crate) scope_from: Option<String>,
}

impl Default for ProvenanceConfig {
    /** Nothing required; twenty commits
     * Input
        - None
     * Output
        - ProvenanceConfig
    */
    fn default() -> Self {
        Self {
            require: Vec::new(),
            trailer: None,
            issue_pattern: None,
            commits: 20,
            scope_from: None,
        }
    }
}

/** WATCH: what keeps checking while agents are idle
 * Fields
    - signals: Vec<Signal> - watched signals
    - on_fire: Vec<Reaction> - what a firing signal does
    - triggers: Vec<Trigger> - conditions that make the target's attachments due for withdrawal
    - analyses: Vec<Analysis> - advisory analyses
    - advisory_only: bool - must be true: analyses propose, never apply
    - scope_from: Option<String> - watch only files of an Access Brick on the same target
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct WatchConfig {
    pub(crate) signals: Vec<Signal>,
    pub(crate) on_fire: Vec<Reaction>,
    pub(crate) triggers: Vec<Trigger>,
    pub(crate) analyses: Vec<Analysis>,
    pub(crate) advisory_only: bool,
    pub(crate) scope_from: Option<String>,
}

impl Default for WatchConfig {
    /** Nothing watched; advisory only
     * Input
        - None
     * Output
        - WatchConfig
    */
    fn default() -> Self {
        Self {
            signals: Vec::new(),
            on_fire: Vec::new(),
            triggers: Vec::new(),
            analyses: Vec::new(),
            advisory_only: true,
            scope_from: None,
        }
    }
}

impl Config {
    /** The Brick id this configuration shares from ("scope_from" or "checks_from"), if any
     * Input
        - None (uses self)
     * Output
        - Option<&str>
    */
    pub(crate) fn shares_from(&self) -> Option<&str> {
        match self {
            Self::Approval(config) => config.scope_from.as_deref(),
            Self::Limits(config) => config.scope_from.as_deref(),
            Self::Provenance(config) => config.scope_from.as_deref(),
            Self::Watch(config) => config.scope_from.as_deref(),
            Self::Verification(config) => config.checks_from.as_deref(),
            _ => None,
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

/** Parse a configuration object into one current type's schema
 * Input
    - brick_type: BrickType - a current type
    - value: &Value - configuration object (null is an empty configuration)
 * Output
    - Result<Config, Problem>
    - Error CONFIG_INVALID naming the field serde refused
*/
fn parse_current(brick_type: BrickType, value: &Value) -> Result<Config, Problem> {
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
        BrickType::Knowledge => Config::Knowledge(typed(value)?),
        BrickType::Identity => Config::Identity(typed(value)?),
        BrickType::Access => Config::Access(typed(value)?),
        BrickType::Approval => Config::Approval(typed(value)?),
        BrickType::Limits => Config::Limits(typed(value)?),
        BrickType::Environment => Config::Environment(typed(value)?),
        BrickType::Precondition => Config::Precondition(typed(value)?),
        BrickType::Verification => Config::Verification(typed(value)?),
        BrickType::Action => Config::Action(typed(value)?),
        BrickType::Recovery => Config::Recovery(typed(value)?),
        BrickType::Provenance => Config::Provenance(typed(value)?),
        BrickType::Watch => Config::Watch(typed(value)?),
        former => {
            return Err(problem(
                "CONFIG_INVALID",
                format!(
                    "{} is a former type and is read through its replacement",
                    former.name()
                ),
            ))
        }
    })
}

/** Parse a stored configuration into the current types it stands for: one for a current type,
 * one or two for a former type (converted by brick::legacy)
 * Input
    - brick_type: BrickType - stored type
    - value: &Value - configuration object
 * Output
    - Result<Vec<Config>, Problem>
*/
pub(crate) fn parse(brick_type: BrickType, value: &Value) -> Result<Vec<Config>, Problem> {
    if !brick_type.is_legacy() {
        return Ok(vec![parse_current(brick_type, value)?]);
    }
    super::legacy::convert(brick_type, value)?
        .into_iter()
        .map(|(kind, converted)| parse_current(kind, &converted))
        .collect()
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

/** Report whether a name is a plain identifier (letters, digits, '-', '_', '.', and '*' when
 * patterns are allowed)
 * Input
    - text: &str - name
    - pattern: bool - '*' is allowed
 * Output
    - bool
*/
fn plain(text: &str, pattern: bool) -> bool {
    !text.trim().is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c) || (pattern && c == '*'))
}

/** Validate one check
 * Input
    - field: &str - field name, for messages
    - check: &Check - check
    - gate: bool - it runs before every change (a precondition)
    - problems: &mut Vec<Problem> - problems found
 * Output
    - None
*/
fn check_problems(field: &str, check: &Check, gate: bool, problems: &mut Vec<Problem>) {
    if gate && !check.is_gate() {
        problems.push(problem(
            "PRECONDITION_TOO_COSTLY",
            format!("{field}: {} is a verification check; a precondition runs before every change and may use file, env, branch, git_clean, time_window, command_available, credential_present, or signed_state checks", serial_kind(check)),
        ));
    }
    match check {
        Check::FileExists { path }
        | Check::FileAbsent { path }
        | Check::FileContains { path, .. } => {
            problems.extend(path_problem(&format!("{field}.path"), path));
        }
        Check::JsonValue { path, pointer, .. } => {
            problems.extend(path_problem(&format!("{field}.path"), path));
            if !pointer.starts_with('/') {
                problems.push(problem(
                    "JSON_POINTER_INVALID",
                    format!("{field}: pointer is a JSON pointer such as /total/lines/pct"),
                ));
            }
        }
        Check::EnvSet { name }
        | Check::CommandAvailable { name }
        | Check::CredentialPresent { name }
            if !plain(name, false) =>
        {
            problems.push(problem(
                "CHECK_NAME_INVALID",
                format!("{field}: '{name}' is not a plain name"),
            ));
        }
        Check::BranchIs { patterns } | Check::BranchNot { patterns } if patterns.is_empty() => {
            problems.push(problem(
                "CHECK_EMPTY",
                format!("{field}: name at least one branch pattern"),
            ));
        }
        Check::TimeWindow {
            days,
            from,
            to,
            utc_offset_minutes,
        } => {
            if minutes_of(from).is_none() || minutes_of(to).is_none() {
                problems.push(problem(
                    "TIME_WINDOW_INVALID",
                    format!("{field}: from and to are HH:MM"),
                ));
            }
            if let Some(day) = days.iter().find(|day| weekday(day).is_none()) {
                problems.push(problem(
                    "TIME_WINDOW_INVALID",
                    format!("{field}: '{day}' is not a weekday (mon..sun)"),
                ));
            }
            if utc_offset_minutes.abs() > 14 * 60 {
                problems.push(problem(
                    "TIME_WINDOW_INVALID",
                    format!("{field}: utc_offset_minutes is between -840 and 840"),
                ));
            }
        }
        Check::HttpOk {
            url,
            timeout_seconds,
            ..
        } => {
            if !url.starts_with("https://") {
                problems.push(problem(
                    "HTTP_CHECK_NOT_HTTPS",
                    format!("{field}: '{url}' must be an https:// URL"),
                ));
            }
            if timeout_seconds.is_some_and(|seconds| seconds == 0 || seconds > 60) {
                problems.push(problem(
                    "CHECK_TIMEOUT_INVALID",
                    format!("{field}: timeout_seconds is 1 to 60"),
                ));
            }
        }
        Check::LocalMetric { window_hours, .. }
            if *window_hours == 0 || *window_hours > 24 * 30 =>
        {
            problems.push(problem(
                "CHECK_WINDOW_INVALID",
                format!("{field}: window_hours is 1 to 720"),
            ));
        }
        Check::CommandSucceeds {
            program,
            timeout_seconds,
            ..
        } => {
            if !plain(program, false) {
                problems.push(problem(
                    "CHECK_PROGRAM_INVALID",
                    format!("{field}: program is a program name on PATH (no paths, no shell)"),
                ));
            }
            if timeout_seconds.is_some_and(|seconds| seconds == 0 || seconds > 600) {
                problems.push(problem(
                    "CHECK_TIMEOUT_INVALID",
                    format!("{field}: timeout_seconds is 1 to 600"),
                ));
            }
        }
        _ => {}
    }
}

/** The serialized kind of a check, such as "http_ok"
 * Input
    - check: &Check - check
 * Output
    - String
*/
pub(crate) fn serial_kind(check: &Check) -> String {
    serde_json::to_value(check)
        .ok()
        .and_then(|value| {
            value
                .get("kind")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_default()
}

/** The minutes after midnight of "HH:MM"
 * Input
    - text: &str - time
 * Output
    - Option<u32>
*/
pub(crate) fn minutes_of(text: &str) -> Option<u32> {
    let (hours, minutes) = text.trim().split_once(':')?;
    let (hours, minutes) = (hours.parse::<u32>().ok()?, minutes.parse::<u32>().ok()?);
    (hours < 24 && minutes < 60).then_some(hours * 60 + minutes)
}

/** The weekday number (0 = Monday) of a short name
 * Input
    - text: &str - "mon" .. "sun" (any case, or the full name)
 * Output
    - Option<u32>
*/
pub(crate) fn weekday(text: &str) -> Option<u32> {
    let lower = text.trim().to_ascii_lowercase();
    ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
        .iter()
        .position(|day| lower.starts_with(day))
        .map(|index| index as u32)
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
    let paths = |problems: &mut Vec<Problem>, field: &str, entries: &[String]| {
        for entry in entries {
            problems.extend(path_problem(field, entry));
        }
    };
    if let Some(shared) = config.shares_from() {
        if crate::platform::validate_name("brick", shared).is_err() {
            problems.push(problem(
                "SHARE_INVALID",
                format!("'{shared}' is not a Brick id"),
            ));
        }
    }
    match config {
        Config::Knowledge(config) => {
            paths(&mut problems, "files", &config.files);
            for (term, meaning) in &config.glossary {
                if term.trim().is_empty() || meaning.trim().is_empty() {
                    problems.push(problem(
                        "GLOSSARY_EMPTY_ENTRY",
                        "every glossary term needs a meaning",
                    ));
                }
            }
        }
        Config::Identity(config) => {
            if config.agents.is_empty() {
                problems.push(problem(
                    "IDENTITY_EMPTY",
                    "agents must name at least one provider or agent id",
                ));
            }
            for agent in &config.agents {
                if !plain(agent, false) {
                    problems.push(problem(
                        "IDENTITY_INVALID",
                        format!("'{agent}' is not a provider name or agent id"),
                    ));
                }
            }
            for model in &config.models {
                if !plain(model, true) {
                    problems.push(problem(
                        "IDENTITY_INVALID",
                        format!("'{model}' is not a model name or pattern"),
                    ));
                }
            }
            if config.on_other == Mode::Auto {
                problems.push(problem(
                    "IDENTITY_ADMITS_ALL",
                    "on_other is ask or deny; auto would admit everyone",
                ));
            }
        }
        Config::Access(config) => {
            for (field, entries) in [
                ("include", &config.include),
                ("exclude", &config.exclude),
                ("read_only", &config.read_only),
                ("deny_read", &config.deny_read),
            ] {
                paths(&mut problems, field, entries);
            }
            let modes = [
                config.write,
                config.create,
                config.update,
                config.delete,
                config.execute,
                config.network,
            ];
            if !config.allow.is_empty() && modes.iter().any(Option::is_some) {
                problems.push(problem(
                    "ACCESS_MIXED",
                    "use either the former allow list or the per-operation modes, not both",
                ));
            }
            if config.include.is_empty()
                && config.exclude.is_empty()
                && config.read_only.is_empty()
                && config.deny_read.is_empty()
                && config.allow.is_empty()
                && modes.iter().all(Option::is_none)
            {
                problems.push(problem(
                    "ACCESS_EMPTY",
                    "an access Brick needs at least one path list or operation mode",
                ));
            }
        }
        Config::Approval(config) => {
            paths(&mut problems, "paths", &config.paths);
            paths(&mut problems, "except", &config.except);
            if config.operations.is_empty() {
                problems.push(problem(
                    "APPROVAL_EMPTY",
                    "operations must name at least one kind of change",
                ));
            }
            if config.operations.contains(&ChangeClass::Read) {
                problems.push(problem("APPROVAL_READ_UNSUPPORTED", "reads cannot be held for approval by the hooks; use an access Brick's deny_read instead"));
            }
            if config.operations.contains(&ChangeClass::LargeChange)
                && config.min_lines_changed.is_none_or(|lines| lines == 0)
            {
                problems.push(problem(
                    "APPROVAL_SIZE_MISSING",
                    "large_change needs min_lines_changed (at least 1)",
                ));
            }
            if config.min_approvals > 5 {
                problems.push(problem(
                    "APPROVAL_QUORUM_INVALID",
                    "min_approvals is at most 5",
                ));
            }
        }
        Config::Limits(config) => {
            let caps = [
                config.max_tool_calls,
                config.max_runtime_minutes,
                config.max_files_changed,
                config.max_lines_changed,
                config.max_commands,
                config.max_network_calls,
                config.max_denials,
            ];
            if caps.iter().all(Option::is_none)
                && config.max_retries.is_none()
                && config.max_ceiling.is_none()
                && config.min_criticality.is_none()
            {
                problems.push(problem(
                    "LIMITS_EMPTY",
                    "limits need at least one cap, max_ceiling, or min_criticality",
                ));
            }
            if caps.contains(&Some(0)) && config.max_denials != Some(0) {
                problems.push(problem(
                    "LIMITS_ZERO",
                    "a cap of 0 blocks all work; use an access Brick to forbid it",
                ));
            }
            if config.on_exhausted == Escalation::Retry {
                problems.push(problem("LIMITS_RETRY", "on_exhausted is stop or escalate"));
            }
        }
        Config::Environment(config) => {
            match profiles.iter().find(|(id, _)| *id == config.profile) {
                None => problems.push(problem(
                    "ISOLATION_UNKNOWN_PROFILE",
                    format!("'{}' is not an isolation profile; known: {}", config.profile, profiles.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>().join(", ")),
                )),
                Some((_, false)) => problems.push(problem(
                    "ISOLATION_UNAVAILABLE",
                    format!("the '{}' profile is not available in this version; attaching it makes direct agent changes observe-only", config.profile),
                )),
                Some(_) => {}
            }
            for name in config.require_env.iter().chain(&config.hide_env) {
                if !plain(name, true) {
                    problems.push(problem(
                        "ENV_NAME_INVALID",
                        format!("'{name}' is not an environment variable name or pattern"),
                    ));
                }
            }
            for credential in &config.credentials {
                if crate::platform::validate_name("credential", &credential.name).is_err() {
                    problems.push(problem(
                        "CREDENTIAL_NAME_INVALID",
                        format!(
                            "'{}' is not a vault name (letters, digits, '_' or '-')",
                            credential.name
                        ),
                    ));
                }
            }
            for host in config.network.allow.iter().chain(&config.network.deny) {
                if host.contains('/') || host.contains(':') || !plain(host, true) {
                    problems.push(problem(
                        "NETWORK_HOST_INVALID",
                        format!("'{host}' is a host name such as api.example.com or *.example.com"),
                    ));
                }
            }
        }
        Config::Precondition(config) => {
            if config.checks.is_empty() {
                problems.push(problem(
                    "CHECKS_EMPTY",
                    "checks must name at least one check",
                ));
            }
            if config.on_fail == Mode::Auto {
                problems.push(problem(
                    "PRECONDITION_IGNORED",
                    "on_fail is ask or deny; auto would ignore the gate",
                ));
            }
            for check in &config.checks {
                check_problems("checks", check, true, &mut problems);
            }
        }
        Config::Verification(config) => {
            if config.checks.is_empty() && config.checks_from.is_none() {
                problems.push(problem(
                    "CHECKS_EMPTY",
                    "checks must name at least one check (or checks_from a precondition Brick)",
                ));
            }
            for check in &config.checks {
                check_problems("checks", check, false, &mut problems);
            }
        }
        Config::Action(config) => {
            let rules = config.rules();
            if rules.is_empty() {
                problems.push(problem(
                    "ACTION_EMPTY",
                    "operations must name at least one registered operation",
                ));
            }
            for rule in &rules {
                if rule.adapter.contains(char::is_whitespace)
                    || rule.operation.contains(char::is_whitespace)
                {
                    problems.push(problem("ACTION_NOT_REGISTERED", "an action names a registered adapter and operation id, never a command line"));
                } else if !adapters.iter().any(|(adapter, operation)| {
                    *adapter == rule.adapter && *operation == rule.operation
                }) && !super::guard::DIRECT.iter().any(|(adapter, operation, _)| {
                    *adapter == rule.adapter && *operation == rule.operation
                }) {
                    problems.push(problem(
                        "ACTION_NOT_REGISTERED",
                        format!(
                            "{}/{} is neither a registered operation nor a known direct one; registered: {}; direct: {}",
                            rule.adapter,
                            rule.operation,
                            adapters.iter().map(|(adapter, operation)| format!("{adapter}/{operation}")).collect::<Vec<_>>().join(", "),
                            super::guard::DIRECT.iter().map(|(adapter, operation, _)| format!("{adapter}/{operation}")).collect::<Vec<_>>().join(", ")
                        ),
                    ));
                }
            }
        }
        Config::Recovery(config) => {
            if !config.reconcile_before_retry && config.max_retries > 0 {
                problems.push(problem("RECOVERY_UNSAFE_RETRY", "retries require reconcile_before_retry: an uncertain effect is reconciled before it is repeated"));
            }
            if config.max_retries > 3 {
                problems.push(problem(
                    "RECOVERY_TOO_MANY_RETRIES",
                    "max_retries is at most 3",
                ));
            }
            if config.on_failure == Escalation::Retry && config.max_retries == 0 {
                problems.push(problem(
                    "RECOVERY_RETRY_WITHOUT_RETRIES",
                    "on_failure retry needs max_retries of at least 1",
                ));
            }
            if config.backoff_seconds > 3600 {
                problems.push(problem(
                    "RECOVERY_BACKOFF_TOO_LONG",
                    "backoff_seconds is at most 3600",
                ));
            }
            if config.max_consecutive_failures == Some(0) {
                problems.push(problem(
                    "RECOVERY_ZERO",
                    "max_consecutive_failures is at least 1",
                ));
            }
        }
        Config::Provenance(config) => {
            if config.require.is_empty() {
                problems.push(problem(
                    "PROVENANCE_EMPTY",
                    "require must name at least one record",
                ));
            }
            if config.require.contains(&ProvenanceItem::CommitTrailer)
                && config
                    .trailer
                    .as_deref()
                    .is_none_or(|key| !plain(key, false))
            {
                problems.push(problem(
                    "PROVENANCE_TRAILER_MISSING",
                    "commit_trailer needs trailer, a key such as Agent-Session",
                ));
            }
            if config.require.contains(&ProvenanceItem::LinkedIssue)
                && config
                    .issue_pattern
                    .as_deref()
                    .is_none_or(|pattern| pattern.trim().is_empty())
            {
                problems.push(problem(
                    "PROVENANCE_PATTERN_MISSING",
                    "linked_issue needs issue_pattern, such as PROJ-*",
                ));
            }
            if config.commits == 0 || config.commits > 500 {
                problems.push(problem("PROVENANCE_COMMITS_INVALID", "commits is 1 to 500"));
            }
        }
        Config::Watch(config) => {
            if config.signals.is_empty() && config.triggers.is_empty() && config.analyses.is_empty()
            {
                problems.push(problem(
                    "WATCH_EMPTY",
                    "a watch needs signals, triggers, or analyses",
                ));
            }
            for signal in &config.signals {
                if signal.every_minutes < 5 {
                    problems.push(problem("MONITOR_TOO_FREQUENT", "every_minutes is at least 5: monitors are cheap periodic checks, not a loop"));
                }
                if signal.source == SignalSource::Denials && signal.threshold.is_none() {
                    problems.push(problem(
                        "MONITOR_THRESHOLD_MISSING",
                        "a denials signal needs a threshold",
                    ));
                }
            }
            if !config.on_fire.is_empty() && config.signals.is_empty() {
                problems.push(problem(
                    "WATCH_REACTION_WITHOUT_SIGNAL",
                    "on_fire reacts to signals; name at least one signal",
                ));
            }
            if !config.advisory_only {
                problems.push(problem("INTELLIGENCE_NOT_ADVISORY", "analyses are advisory: they may propose, never change policy, grant authority, or approve"));
            }
        }
    }
    problems
}
