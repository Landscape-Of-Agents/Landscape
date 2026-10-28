// The fixed Brick type taxonomy. The set is closed in code: a type is a platform change reviewed
// with the rest of Crane, never a value a person registers at runtime. Each of the twelve kinds
// answers one question no other kind answers (who, which files and operations, when a person
// decides, how much, with what environment, what must hold before, what proves the outcome, which
// operations, what after a failure, what an outcome traces to, what keeps watching, what agents
// know), and states honestly how far this version supports it.
//
// Nine former kinds (context, scope, permission, constraint, isolation, budget, revocation,
// monitoring, intelligence) stay readable so signed stores keep verifying: their entries keep
// their exact serialization (and so their digests), are read as the kinds that replaced them
// (brick::legacy), and are rewritten by 'crane brick migrate'. New Bricks cannot use them.

use serde::{Deserialize, Serialize};

/** A functional Brick type (closed set)
 * Variants
    - Knowledge - what agents know: reviewed documents, terms, and references
    - Identity - who may act: the providers and models admitted to the target
    - Access - which files and operations: in and out of scope, read-only, unreadable, and
      auto/ask/deny per operation
    - Approval - when a person decides: kinds of change (dependency, config, push, large diff,
      ...) that need a person, wherever access allows them
    - Limits - how much and how long: tool calls, runtime, files and lines changed, commands,
      network calls, denials per session, and the autonomy ceiling
    - Environment - what the work runs with: required and hidden environment variables,
      credential references, and network reach
    - Precondition - what must hold before acting: gates evaluated before every change
    - Verification - what proves the outcome: checks with evidence after work
    - Action - which side-effecting operations: registered operations Crane runs for the target
    - Recovery - what happens after a failure: retries, reconciliation, and pausing after
      repeated failures
    - Provenance - what an outcome must trace to: signed state, audit chain, commit trailers,
      linked issues
    - Watch - what keeps checking while agents are idle, and how it reacts
    - Context, Scope, Permission, Constraint, Isolation, Budget, Revocation, Monitoring,
      Intelligence - former kinds, read through brick::legacy only
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrickType {
    Knowledge,
    Identity,
    Access,
    Approval,
    Limits,
    Environment,
    Precondition,
    Verification,
    Action,
    Recovery,
    Provenance,
    Watch,
    Context,
    Scope,
    Permission,
    Constraint,
    Isolation,
    Budget,
    Revocation,
    Monitoring,
    Intelligence,
}

/** How far this version supports a type
 * Variants
    - Enforced - decided before every agent tool call (the guard's per-file limits and the Brick
      rules), so the hooks and 'crane scope' apply it
    - Checked - evaluated deterministically by 'crane brick test' and the monitors
    - Delivered - assembled into the agent's context (guidance only)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Support {
    Enforced,
    Checked,
    Delivered,
}

/** One row of the type catalog
 * Fields
    - brick_type: BrickType - type
    - label: &'static str - human label
    - purpose: &'static str - what it is for, in one line
    - unique: &'static str - what it does that no other kind does, in one line
    - support: Support - how far it is supported
    - runtime: &'static str - what really happens with it today
    - fields: &'static [&'static str] - its configuration fields
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypeInfo {
    #[serde(rename = "type")]
    pub(crate) brick_type: BrickType,
    pub(crate) label: &'static str,
    pub(crate) purpose: &'static str,
    pub(crate) unique: &'static str,
    pub(crate) support: Support,
    pub(crate) runtime: &'static str,
    pub(crate) fields: &'static [&'static str],
}

impl BrickType {
    /** Every current type, in catalog order */
    pub(crate) const ALL: [BrickType; 12] = [
        Self::Knowledge,
        Self::Identity,
        Self::Access,
        Self::Approval,
        Self::Limits,
        Self::Environment,
        Self::Precondition,
        Self::Verification,
        Self::Action,
        Self::Recovery,
        Self::Provenance,
        Self::Watch,
    ];

    /** The former types, still readable from signed stores */
    pub(crate) const LEGACY: [BrickType; 9] = [
        Self::Context,
        Self::Scope,
        Self::Permission,
        Self::Constraint,
        Self::Isolation,
        Self::Budget,
        Self::Revocation,
        Self::Monitoring,
        Self::Intelligence,
    ];

    /** The serialized name of the type, such as "access"
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        self.info().label_key
    }

    /** Report whether this is a former type (readable, not creatable)
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn is_legacy(self) -> bool {
        Self::LEGACY.contains(&self)
    }

    /** The current types a former type is read as (a current type maps to itself)
     * Input
        - None (uses self)
     * Output
        - &'static [BrickType]
    */
    pub(crate) fn successors(self) -> &'static [BrickType] {
        match self {
            Self::Context => &[Self::Knowledge],
            Self::Scope | Self::Permission => &[Self::Access],
            Self::Constraint => &[Self::Limits, Self::Access],
            Self::Isolation => &[Self::Environment],
            Self::Budget => &[Self::Limits],
            Self::Revocation | Self::Monitoring | Self::Intelligence => &[Self::Watch],
            Self::Knowledge => &[Self::Knowledge],
            Self::Identity => &[Self::Identity],
            Self::Access => &[Self::Access],
            Self::Approval => &[Self::Approval],
            Self::Limits => &[Self::Limits],
            Self::Environment => &[Self::Environment],
            Self::Precondition => &[Self::Precondition],
            Self::Verification => &[Self::Verification],
            Self::Action => &[Self::Action],
            Self::Recovery => &[Self::Recovery],
            Self::Provenance => &[Self::Provenance],
            Self::Watch => &[Self::Watch],
        }
    }

    /** Parse the name of a current type, case-insensitively; a former type is refused with the
     * type that replaced it, anything else with the fixed set
     * Input
        - text: &str - name, such as "ACCESS" or "access"
     * Output
        - Result<BrickType, String>
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let wanted = text.trim().to_ascii_lowercase();
        if let Some(former) = Self::LEGACY
            .into_iter()
            .find(|candidate| candidate.name() == wanted)
        {
            return Err(format!(
                "'{wanted}' is a former Brick type; use {} instead ('crane brick migrate' converts existing ones)",
                former
                    .successors()
                    .iter()
                    .map(|kind| kind.name())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ));
        }
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.name() == wanted)
            .ok_or_else(|| {
                format!(
                    "unknown Brick type '{text}'; the set is fixed: {}",
                    Self::ALL.map(Self::name).join(", ")
                )
            })
    }

    /** Parse a current or former type name (for filters over stored Bricks)
     * Input
        - text: &str - name
     * Output
        - Result<BrickType, String>
    */
    pub(crate) fn parse_any(text: &str) -> Result<Self, String> {
        let wanted = text.trim().to_ascii_lowercase();
        match Self::LEGACY
            .into_iter()
            .find(|candidate| candidate.name() == wanted)
        {
            Some(former) => Ok(former),
            None => Self::parse(text),
        }
    }

    /** The catalog row of the type
     * Input
        - None (uses self)
     * Output
        - TypeInfo
    */
    pub(crate) fn catalog(self) -> TypeInfo {
        let info = self.info();
        TypeInfo {
            brick_type: self,
            label: info.label,
            purpose: info.purpose,
            unique: info.unique,
            support: info.support,
            runtime: info.runtime,
            fields: info.fields,
        }
    }

    /** The static description of the type
     * Input
        - None (uses self)
     * Output
        - Info
    */
    fn info(self) -> Info {
        match self {
            Self::Knowledge => Info {
                label_key: "knowledge",
                label: "Knowledge",
                purpose: "Gives agents the reviewed documents, terms, and references they need to work in the target.",
                unique: "The only kind that puts repository documents into an agent's context; it informs and never restricts.",
                support: Support::Delivered,
                runtime: "assembled into the context snapshot delivered before the agent acts in the target; never authorization",
                fields: &["files", "glossary", "references"],
            },
            Self::Identity => Info {
                label_key: "identity",
                label: "Identity",
                purpose: "Says which agent providers (and models) may work in the target at all.",
                unique: "The only kind that decides by who is calling, whatever they try to do.",
                support: Support::Enforced,
                runtime: "every tool call touching the target's files from another provider or model is denied (or needs a person with on_other: ask)",
                fields: &["agents", "models", "on_other"],
            },
            Self::Access => Info {
                label_key: "access",
                label: "Access",
                purpose: "Says which files of the target agents may read or change, and how each operation is treated.",
                unique: "The only kind with per-path, per-operation rules: in or out of scope, read-only, unreadable, and auto/ask/deny for create, update, delete, commands, and network.",
                support: Support::Enforced,
                runtime: "decided before every tool call: unreadable paths deny reads, read-only and out-of-scope paths deny changes, ask needs a person, deny refuses",
                fields: &["include", "exclude", "read_only", "deny_read", "write", "create", "update", "delete", "execute", "network", "allow"],
            },
            Self::Approval => Info {
                label_key: "approval",
                label: "Approval",
                purpose: "Names the kinds of change a person must approve, even where access allows them.",
                unique: "The only kind that recognizes kinds of change (dependency, config, migration, push, package install, large diff) rather than files or operations.",
                support: Support::Enforced,
                runtime: "a matching change needs a person; the agent tool's own prompt answers it, so approvers and quorum apply to resident agents only",
                fields: &["operations", "paths", "except", "min_lines_changed", "reason", "approvers", "min_approvals", "distinct_approver", "expires_minutes"],
            },
            Self::Limits => Info {
                label_key: "limits",
                label: "Limits",
                purpose: "Caps how much and how long agents work in the target in one session.",
                unique: "The only kind that counts: tool calls, minutes, files and lines changed, commands, network calls, denials, and the autonomy ceiling.",
                support: Support::Enforced,
                runtime: "counted per session and target before every tool call; at a cap the call stops or needs a person (on_exhausted)",
                fields: &["max_tool_calls", "max_runtime_minutes", "max_files_changed", "max_lines_changed", "max_commands", "max_network_calls", "max_denials", "max_retries", "max_ceiling", "min_criticality", "on_exhausted", "scope_from"],
            },
            Self::Environment => Info {
                label_key: "environment",
                label: "Environment",
                purpose: "Defines what the work runs with: environment variables, credentials, and network reach.",
                unique: "The only kind that governs the surroundings of the work: variables that must exist or stay hidden, credentials by reference, and hosts it may reach.",
                support: Support::Enforced,
                runtime: "hidden variables and unlisted hosts are denied before every command; a missing required variable makes changes need a person; credentials go only to Crane-run operations",
                fields: &["profile", "require_env", "hide_env", "credentials", "network"],
            },
            Self::Precondition => Info {
                label_key: "precondition",
                label: "Precondition",
                purpose: "Gates changes on facts that must hold first: files, branch, clean tree, time window, variables.",
                unique: "The only kind evaluated before every change; while a gate fails, changes need a person or stop.",
                support: Support::Enforced,
                runtime: "the checks run when limits are computed; a failing check makes every change in the target need a person (on_fail: ask) or stops it (on_fail: deny)",
                fields: &["checks", "on_fail"],
            },
            Self::Verification => Info {
                label_key: "verification",
                label: "Verification",
                purpose: "Proves the outcome after work: policy tests, files, network endpoints, metrics, and commands.",
                unique: "The only kind that produces evidence after the fact: proven, contradicted, unknown, or not run.",
                support: Support::Checked,
                runtime: "checks run by 'crane brick test' and the monitors (commands only by 'crane brick test', run by a person)",
                fields: &["checks", "checks_from"],
            },
            Self::Action => Info {
                label_key: "action",
                label: "Action",
                purpose: "Lists the side-effecting operations Crane may run for the target, and routes them through Crane.",
                unique: "The only kind about operations in other systems: an allow-list with daily caps, dry-run first, and agents kept from doing them directly.",
                support: Support::Enforced,
                runtime: "resident steps may use only the listed operations; with deny_direct, agent shell commands doing the same operation are denied",
                fields: &["operations", "deny_direct", "adapter", "operation"],
            },
            Self::Recovery => Info {
                label_key: "recovery",
                label: "Recovery",
                purpose: "Decides what happens after a failed, partial, or uncertain outcome.",
                unique: "The only kind that reacts to failure: retries with reconciliation first, and a pause after repeated agent failures.",
                support: Support::Enforced,
                runtime: "after max_consecutive_failures failed tool calls in the target, changes need a person; retries and reconciliation apply to resident agents and skills",
                fields: &["on_failure", "max_retries", "reconcile_before_retry", "backoff_seconds", "max_consecutive_failures", "on_uncertain"],
            },
            Self::Provenance => Info {
                label_key: "provenance",
                label: "Provenance",
                purpose: "Requires every outcome to be traceable: signed state, audit chain, commit trailers, linked issues.",
                unique: "The only kind about lineage: it checks where changes came from, not whether they work.",
                support: Support::Checked,
                runtime: "verified by 'crane brick test' and the monitors against the signed manifest, the audit chain, and recent commits touching the target",
                fields: &["require", "trailer", "issue_pattern", "commits", "scope_from"],
            },
            Self::Watch => Info {
                label_key: "watch",
                label: "Watch",
                purpose: "Keeps checking the target while agents are idle and reacts when a signal fires.",
                unique: "The only kind that runs on its own schedule: signals with reactions (alert, pause, revoke due) and advisory analyses.",
                support: Support::Checked,
                runtime: "evaluated by every background sync and 'crane brick monitor run' without a model; pause makes changes need a person while it fires",
                fields: &["signals", "on_fire", "triggers", "analyses", "advisory_only", "scope_from"],
            },
            Self::Context => former("context", "Context", "knowledge", &["files"]),
            Self::Scope => former("scope", "Scope", "access", &["include"]),
            Self::Permission => former("permission", "Permission", "access", &["allow"]),
            Self::Constraint => former("constraint", "Constraint", "limits and access", &["max_ceiling", "min_criticality", "protected_paths"]),
            Self::Isolation => former("isolation", "Isolation", "environment", &["profile"]),
            Self::Budget => former("budget", "Budget", "limits", &["max_tool_calls", "max_retries", "max_runtime_minutes", "on_exhausted"]),
            Self::Revocation => former("revocation", "Revocation", "watch", &["triggers"]),
            Self::Monitoring => former("monitoring", "Monitoring", "watch", &["signals"]),
            Self::Intelligence => former("intelligence", "Intelligence", "watch", &["analyses", "advisory_only"]),
        }
    }
}

/** The description of a former type
 * Input
    - key: &'static str - serialized name
    - label: &'static str - human label
    - now: &'static str - the types that replaced it
    - fields: &'static [&'static str] - its configuration fields
 * Output
    - Info
*/
fn former(
    key: &'static str,
    label: &'static str,
    now: &'static str,
    fields: &'static [&'static str],
) -> Info {
    Info {
        label_key: key,
        label,
        purpose: "A former kind, kept so signed Bricks still verify.",
        unique: now,
        support: Support::Enforced,
        runtime: "read as its replacement kind; 'crane brick migrate' rewrites it",
        fields,
    }
}

/** The static description of a type
 * Fields
    - label_key: &'static str - serialized name
    - label: &'static str - human label
    - purpose: &'static str - what it is for
    - unique: &'static str - what only it does (for a former type: what replaced it)
    - support: Support - how far it is supported
    - runtime: &'static str - what really happens with it
    - fields: &'static [&'static str] - configuration fields
*/
struct Info {
    label_key: &'static str,
    label: &'static str,
    purpose: &'static str,
    unique: &'static str,
    support: Support,
    runtime: &'static str,
    fields: &'static [&'static str],
}
