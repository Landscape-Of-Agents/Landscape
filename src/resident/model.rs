// The resident model: reusable agent definitions (immutable versions), assignments that activate a
// definition for one target and a bounded lifetime, trigger subscriptions, and operating limits.
// Definitions are declarative: no credential, script, URL, or authority grant can be expressed, and
// every limit has an administrative ceiling that validation enforces. An assignment's lifecycle
// (draft, validated, active, paused, expired, revoked) is kept apart from its activations and from
// its derived runtime status.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::plan::StepType;
use crate::brick::model::{Author, Target};
use crate::trust::crypto::digest_json;

/** The longest lifetime of one assignment period, in days (renewal starts a new period) */
pub(crate) const MAX_LIFETIME_DAYS: u64 = 90;

/** Problems or warnings: reason codes with explanations */
pub(crate) type Problems = Vec<(String, String)>;

/** One day in milliseconds */
pub(crate) const DAY_MS: u64 = 86_400_000;

/** Where a trigger comes from (closed; unsupported sources are listed so the UI can say so)
 * Variants
    - CraneEvent - an event in the local Crane event store (decisions, denials, sessions)
    - GovernanceChange - the signed governance generation changed
    - BrickMonitor - a Brick monitor of the target reported FIRING, FAILING, or REVOCATION_DUE
    - Timer - a timer of the assignment fell due
    - Ci - a CI result, delivered by a CI step running 'crane resident trigger'
    - Webhook - a typed event a person or a CI step delivers with 'crane resident trigger'
    - Metric - a metric or alert threshold (no connector: unsupported)
    - Deployment - a deployment result (no connector: unsupported)
    - Slack - a Slack message (no inbound connector: unsupported)
    - Ticket - a ticket change (no inbound connector: unsupported)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TriggerSource {
    CraneEvent,
    GovernanceChange,
    BrickMonitor,
    Timer,
    Ci,
    Webhook,
    Metric,
    Deployment,
    Slack,
    Ticket,
}

impl TriggerSource {
    /** Every source, in catalog order */
    pub(crate) const ALL: [Self; 10] = [
        Self::CraneEvent,
        Self::GovernanceChange,
        Self::BrickMonitor,
        Self::Timer,
        Self::Ci,
        Self::Webhook,
        Self::Metric,
        Self::Deployment,
        Self::Slack,
        Self::Ticket,
    ];

    /** The source's name as written in files
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn name(self) -> String {
        crate::resident::word(&self)
    }

    /** How this source reaches Crane today, or None when it has no connector
     * Input
        - None (uses self)
     * Output
        - Option<&'static str>
    */
    pub(crate) fn delivery(self) -> Option<&'static str> {
        match self {
            Self::CraneEvent => Some("read from the local event store by every tick"),
            Self::GovernanceChange => Some("the signed generation, compared by every tick"),
            Self::BrickMonitor => Some("Brick monitor results, read by every tick"),
            Self::Timer => {
                Some("evaluated by every tick (background sync or 'crane resident tick')")
            }
            Self::Ci => Some("a CI step runs 'crane resident trigger --file EVENT.json'"),
            Self::Webhook => Some("'crane resident trigger --file EVENT.json' (no HTTP listener)"),
            Self::Metric | Self::Deployment | Self::Slack | Self::Ticket => None,
        }
    }
}

/** Whether a subscription also takes repository-wide triggers
 * Variants
    - Target - only triggers that name the assignment's target (the default)
    - Repository - also triggers that name no target (governance changes, CI of the repository)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SubscriptionScope {
    #[default]
    Target,
    Repository,
}

/** A trigger subscription: a source, the kinds it takes, and attribute filters
 * Fields
    - source: TriggerSource - source
    - kinds: Vec<String> - exact kinds or "prefix.*" patterns (never "*")
    - attributes: BTreeMap<String, String> - attributes that must be equal
    - scope: SubscriptionScope - target-only or repository-wide
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Subscription {
    pub(crate) source: TriggerSource,
    pub(crate) kinds: Vec<String>,
    #[serde(default)]
    pub(crate) attributes: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) scope: SubscriptionScope,
}

/** A timer of an assignment (a deterministic schedule; it never prompts a model)
 * Fields
    - timer_id: String - identifier
    - every_minutes: u32 - period (at least 5)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Timer {
    pub(crate) timer_id: String,
    pub(crate) every_minutes: u32,
}

/** How one action-step type may be used by a definition
 * Fields
    - step_type: StepType - type
    - auto: bool - may run without a per-activation approval when Crane allows it (Crane can
      still require one; it can never be skipped where Crane requires it)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepPolicy {
    pub(crate) step_type: StepType,
    #[serde(default)]
    pub(crate) auto: bool,
}

/** What an activation does when required context cannot be loaded
 * Variants
    - Block - the activation is blocked with the reason
    - Ask - the activation waits for a person's answer
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MissingContext {
    #[default]
    Block,
    Ask,
}

/** Operating limits (each bounded by an administrative ceiling)
 * Fields
    - max_activations_per_day: u32 - activations created per assignment per day
    - max_steps: u32 - steps in one plan
    - max_retries: u32 - retries of a step that failed without effect
    - max_model_calls: u32 - model calls per activation (0: deterministic reasoning only)
    - cooldown_minutes: u32 - minimum time between two activations of one assignment
    - step_timeout_seconds: u32 - longest external call
    - approval_ttl_minutes: u32 - how long an approval stays usable
    - wait_minutes: u32 - longest wait for an answer or an event before the step fails
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Limits {
    pub(crate) max_activations_per_day: u32,
    pub(crate) max_steps: u32,
    pub(crate) max_retries: u32,
    pub(crate) max_model_calls: u32,
    pub(crate) cooldown_minutes: u32,
    pub(crate) step_timeout_seconds: u32,
    pub(crate) approval_ttl_minutes: u32,
    pub(crate) wait_minutes: u32,
}

impl Default for Limits {
    /** Conservative defaults
     * Input
        - None
     * Output
        - Limits
    */
    fn default() -> Self {
        Self {
            max_activations_per_day: 24,
            max_steps: 8,
            max_retries: 1,
            max_model_calls: 0,
            cooldown_minutes: 10,
            step_timeout_seconds: 30,
            approval_ttl_minutes: 1440,
            wait_minutes: 1440,
        }
    }
}

/** The administrative ceilings of Limits
 * Input
    - limits: &Limits - limits
 * Output
    - [(&str, u32, u32); 8] (name, value, ceiling)
*/
pub(crate) fn limit_ceilings(limits: &Limits) -> [(&'static str, u32, u32); 8] {
    [
        (
            "max_activations_per_day",
            limits.max_activations_per_day,
            200,
        ),
        ("max_steps", limits.max_steps, 20),
        ("max_retries", limits.max_retries, 2),
        ("max_model_calls", limits.max_model_calls, 10),
        ("cooldown_minutes", limits.cooldown_minutes, 10_080),
        ("step_timeout_seconds", limits.step_timeout_seconds, 300),
        ("approval_ttl_minutes", limits.approval_ttl_minutes, 10_080),
        ("wait_minutes", limits.wait_minutes, 20_160),
    ]
}

/** A reusable agent definition (one immutable version)
 * Fields
    - definition_id: String - stable identifier
    - version: u32 - version (an activated version never changes)
    - name: String - human-readable name
    - purpose: String - mission and responsibility, delivered as agent context
    - owner: String - owning person or team
    - bricks: Vec<String> - Brick versions ("ID@N") expected on the target, checked at validation
    - triggers: Vec<TriggerSource> - sources assignments of it may subscribe to
    - actions: Vec<StepPolicy> - action-step types it may plan
    - verification_required: bool - a plan with an external step must also verify its outcome
      (an activation without verification evidence is never completed either way)
    - on_missing_context: MissingContext - what a context gap does
    - limits: Limits - operating limits
    - created_by: Author - author
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Definition {
    pub(crate) definition_id: String,
    #[serde(default = "first")]
    pub(crate) version: u32,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) purpose: String,
    #[serde(default)]
    pub(crate) owner: String,
    #[serde(default)]
    pub(crate) bricks: Vec<String>,
    pub(crate) triggers: Vec<TriggerSource>,
    pub(crate) actions: Vec<StepPolicy>,
    #[serde(default = "yes")]
    pub(crate) verification_required: bool,
    #[serde(default)]
    pub(crate) on_missing_context: MissingContext,
    #[serde(default)]
    pub(crate) limits: Limits,
    #[serde(default)]
    pub(crate) created_by: Author,
    #[serde(default)]
    pub(crate) created_at: u64,
}

/** The default version of a new definition
 * Input
    - None
 * Output
    - u32 1
*/
fn first() -> u32 {
    1
}

/** The default of a flag that is on unless turned off
 * Input
    - None
 * Output
    - bool true
*/
fn yes() -> bool {
    true
}

impl Definition {
    /** The digest of the definition (what an assignment pins)
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json("resident/definition/v1", &json!(self))[..32].to_string()
    }

    /** The policy of a step type, when the definition allows it
     * Input
        - step_type: StepType - type
     * Output
        - Option<&StepPolicy>
    */
    pub(crate) fn policy(&self, step_type: StepType) -> Option<&StepPolicy> {
        self.actions
            .iter()
            .find(|policy| policy.step_type == step_type)
    }
}

/** Lifecycle of a definition version
 * Variants
    - Draft - stored, cannot be assigned
    - Active - may be assigned
    - Revoked - withdrawn; every assignment of it stops at its next step
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DefinitionState {
    Draft,
    Active,
    Revoked,
}

/** A lifecycle change of a definition, assignment, or RCA graph
 * Fields
    - to: String - new state
    - by: Author - who
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Change {
    pub(crate) to: String,
    pub(crate) by: Author,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** One stored definition version
 * Fields
    - definition: Definition - content
    - digest: String - its digest
    - state: DefinitionState - lifecycle
    - self_reviewed: bool - the author activated it
    - changes: Vec<Change> - lifecycle history
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DefinitionRecord {
    pub(crate) definition: Definition,
    pub(crate) digest: String,
    pub(crate) state: DefinitionState,
    #[serde(default)]
    pub(crate) self_reviewed: bool,
    pub(crate) changes: Vec<Change>,
}

/** Lifecycle of an assignment (kept apart from activations and from the derived status)
 * Variants
    - Draft - stored, receives no triggers
    - Validated - passed validation against the current signed state; not yet active
    - Active - receives triggers (idle unless an activation runs)
    - Paused - receives no triggers; queued work waits
    - Expired - its lifetime ended (renewal starts a new period)
    - Revoked - withdrawn; never undone
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Lifecycle {
    Draft,
    Validated,
    Active,
    Paused,
    Expired,
    Revoked,
}

/** A reference to an RCA graph version
 * Fields
    - rca_id: String - graph
    - version: u32 - exact version
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RcaRef {
    pub(crate) rca_id: String,
    pub(crate) version: u32,
}

/** A validation of an assignment
 * Fields
    - passed: bool - no blocking problem
    - problems: Vec<(String, String)> - blocking problems (codes and explanations)
    - warnings: Vec<(String, String)> - what will not work, without blocking (missing connectors,
      unavailable step types, a proposed RCA graph)
    - authority_digest: String - the signed state it was validated against
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Validation {
    pub(crate) passed: bool,
    pub(crate) problems: Vec<(String, String)>,
    #[serde(default)]
    pub(crate) warnings: Vec<(String, String)>,
    pub(crate) authority_digest: String,
    pub(crate) at: u64,
}

/** A resident assignment: a definition version activated for one target and a lifetime
 * Fields
    - assignment_id: String - identifier
    - definition_id: String - definition
    - definition_version: u32 - exact version
    - definition_digest: String - its digest when the assignment was created
    - agent: String - the agent of the signed Brick assignment it acts under
    - target: Target - Flow, Zone, or Runbook
    - rca: Option<RcaRef> - the RCA graph it investigates with
    - subscriptions: Vec<Subscription> - trigger subscriptions
    - timers: Vec<Timer> - timers
    - lifetime_days: u32 - length of one period (at most MAX_LIFETIME_DAYS)
    - starts_at: u64 - start of the current period (Unix milliseconds; 0 until activated)
    - expires_at: u64 - end of the current period (0 until activated)
    - lifecycle: Lifecycle - lifecycle
    - validation: Validation - last validation
    - created_by: Author - author
    - created_at: u64 - Unix milliseconds
    - changes: Vec<Change> - lifecycle history
    - record_digest: String - integrity digest of the record without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Assignment {
    pub(crate) assignment_id: String,
    pub(crate) definition_id: String,
    pub(crate) definition_version: u32,
    #[serde(default)]
    pub(crate) definition_digest: String,
    pub(crate) agent: String,
    pub(crate) target: Target,
    #[serde(default)]
    pub(crate) rca: Option<RcaRef>,
    pub(crate) subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub(crate) timers: Vec<Timer>,
    pub(crate) lifetime_days: u32,
    #[serde(default)]
    pub(crate) starts_at: u64,
    #[serde(default)]
    pub(crate) expires_at: u64,
    pub(crate) lifecycle: Lifecycle,
    #[serde(default)]
    pub(crate) validation: Validation,
    #[serde(default)]
    pub(crate) created_by: Author,
    #[serde(default)]
    pub(crate) created_at: u64,
    #[serde(default)]
    pub(crate) changes: Vec<Change>,
    #[serde(default)]
    pub(crate) record_digest: String,
}

/** The fields of an assignment a person writes in a draft file
 * Fields
    - assignment_id, definition_id, definition_version, agent, target, rca, subscriptions, timers,
      lifetime_days - see Assignment
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssignmentDraft {
    pub(crate) assignment_id: String,
    pub(crate) definition_id: String,
    pub(crate) definition_version: u32,
    pub(crate) agent: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) rca: Option<RcaRef>,
    pub(crate) subscriptions: Vec<Subscription>,
    #[serde(default)]
    pub(crate) timers: Vec<Timer>,
    #[serde(default = "default_lifetime")]
    pub(crate) lifetime_days: u32,
}

/** The default lifetime of an assignment period
 * Input
    - None
 * Output
    - u32 14 days
*/
fn default_lifetime() -> u32 {
    14
}

impl Assignment {
    /** The digest of the record without its digest
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn compute_digest(&self) -> String {
        let mut content = self.clone();
        content.record_digest = String::new();
        digest_json("resident/assignment/v1", &json!(content))[..32].to_string()
    }

    /** Seal the record
     * Input
        - None (uses self)
     * Output
        - None
    */
    pub(crate) fn seal(&mut self) {
        self.record_digest = self.compute_digest();
    }

    /** Check that the record matches its digest
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verify_digest(&self) -> bool {
        self.compute_digest() == self.record_digest
    }

    /** The digest of what the assignment says (without lifecycle history and seal), part of the
     * context cache key
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn content_digest(&self) -> String {
        digest_json(
            "resident/assignment/content/v1",
            &json!([
                self.definition_id,
                self.definition_version,
                self.definition_digest,
                self.agent,
                self.target,
                self.rca,
                self.subscriptions,
                self.timers,
                self.lifecycle,
                self.expires_at
            ]),
        )[..32]
            .to_string()
    }

    /** Report whether the current period has ended
     * Input
        - now: u64 - Unix milliseconds
     * Output
        - bool
    */
    pub(crate) fn expired_at(&self, now: u64) -> bool {
        self.expires_at > 0 && now >= self.expires_at
    }

    /** Record a lifecycle change
     * Input
        - to: Lifecycle - new state
        - by: &Author - who
        - reason: &str - why
        - at: u64 - Unix milliseconds
     * Output
        - None
    */
    pub(crate) fn change(&mut self, to: Lifecycle, by: &Author, reason: &str, at: u64) {
        self.lifecycle = to;
        self.changes.push(Change {
            to: crate::resident::word(&to),
            by: by.clone(),
            reason: reason.into(),
            at,
        });
    }
}

/** Check whether a lifecycle transition of an assignment is allowed
 * Input
    - from: Lifecycle - current state
    - to: Lifecycle - requested state
 * Output
    - bool
*/
pub(crate) fn transition_allowed(from: Lifecycle, to: Lifecycle) -> bool {
    use Lifecycle::*;
    matches!(
        (from, to),
        (Draft | Validated, Validated)
            | (Validated, Active)
            | (Active, Paused)
            | (Paused, Active)
            | (Active | Paused, Expired)
            | (Expired, Active)
            | (Draft | Validated | Active | Paused | Expired, Revoked)
    )
}

/** Check that a kind pattern is well formed: dot-separated lowercase segments, optionally ending
 * in ".*"; a bare "*" is refused (no "any event" subscription)
 * Input
    - pattern: &str - kind or pattern
 * Output
    - Result<(), String>
*/
pub(crate) fn check_kind(pattern: &str) -> Result<(), String> {
    let body = pattern.strip_suffix(".*").unwrap_or(pattern);
    let valid = !body.is_empty()
        && pattern.len() <= 96
        && body.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_-".contains(c))
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "'{pattern}' is not a trigger kind: use dot-separated lowercase words such as ci.failed or crane.*, never a bare *"
        ))
    }
}
