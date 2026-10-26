// The fixed Brick type taxonomy. The set is closed in code: a type is a platform change reviewed
// with the rest of Crane, never a value a person registers at runtime. Each type states, honestly,
// how far this version supports it: whether Crane enforces it (through the guard's per-file
// limits), checks it on demand, delivers it as guidance, or only validates its declaration.

use serde::{Deserialize, Serialize};

/** A functional Brick type (closed set)
 * Variants
    - Context - reviewed domain facts and guidance; never authorizes
    - Identity - which agents may act within the target
    - Scope - the resources of the target that are in scope
    - Permission - the action classes allowed within the target
    - Constraint - hard limits: ceiling, criticality, protected paths
    - Precondition - facts that must hold before work starts
    - Approval - when a person must approve
    - Isolation - the execution boundary required
    - Budget - caps on calls, retries, and time
    - Action - a registered, typed operation adapter
    - Verification - objective postconditions and their evidence
    - Provenance - the records an outcome must be traceable to
    - Recovery - handling of failed, partial, or uncertain outcomes
    - Revocation - when attachments should be withdrawn, and by whom
    - Monitoring - deterministic checks run while agents are idle
    - Intelligence - advisory analysis that may only propose
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BrickType {
    Context,
    Identity,
    Scope,
    Permission,
    Constraint,
    Precondition,
    Approval,
    Isolation,
    Budget,
    Action,
    Verification,
    Provenance,
    Recovery,
    Revocation,
    Monitoring,
    Intelligence,
}

/** How far this version supports a type
 * Variants
    - Enforced - folded into the guard's per-file limits, so the hooks and 'crane scope' decide
      with it before every agent action
    - Checked - evaluated deterministically by 'crane brick test' and the monitors
    - Delivered - assembled into the agent's context (guidance only)
    - Declared - validated and recorded; no runtime acts on it yet
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Support {
    Enforced,
    Checked,
    Delivered,
    Declared,
}

/** One row of the type catalog
 * Fields
    - brick_type: BrickType - type
    - label: &'static str - human label
    - purpose: &'static str - what it solves, in plain words
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
    pub(crate) support: Support,
    pub(crate) runtime: &'static str,
    pub(crate) fields: &'static [&'static str],
}

impl BrickType {
    /** Every type, in catalog order */
    pub(crate) const ALL: [BrickType; 16] = [
        Self::Context,
        Self::Identity,
        Self::Scope,
        Self::Permission,
        Self::Constraint,
        Self::Precondition,
        Self::Approval,
        Self::Isolation,
        Self::Budget,
        Self::Action,
        Self::Verification,
        Self::Provenance,
        Self::Recovery,
        Self::Revocation,
        Self::Monitoring,
        Self::Intelligence,
    ];

    /** The serialized name of the type, such as "constraint"
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        self.info().label_key
    }

    /** Parse a type name, case-insensitively; anything outside the fixed set is refused
     * Input
        - text: &str - name, such as "CONSTRAINT" or "constraint"
     * Output
        - Result<BrickType, String>
        - Error naming the fixed set
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let wanted = text.trim().to_ascii_lowercase();
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
            Self::Context => Info {
                label_key: "context",
                label: "Context",
                purpose: "Reviewed domain facts, procedures, and terminology agents receive while they work in the Flow or Zone.",
                support: Support::Delivered,
                runtime: "assembled into the context snapshot delivered before the agent acts in the target; never authorization",
                fields: &["files"],
            },
            Self::Identity => Info {
                label_key: "identity",
                label: "Identity",
                purpose: "Which agents (by provider or agent id) may act within the target.",
                support: Support::Checked,
                runtime: "checked by 'crane brick test' and shown per assignment; the guard does not yet decide per agent, so it is not enforced at tool-call time",
                fields: &["agents"],
            },
            Self::Scope => Info {
                label_key: "scope",
                label: "Scope",
                purpose: "The files of the target agents may change; everything else in the target becomes read-only.",
                support: Support::Enforced,
                runtime: "files of the target outside every scope pattern become observe-only; a scope that matches nothing makes the whole target observe-only",
                fields: &["include"],
            },
            Self::Permission => Info {
                label_key: "permission",
                label: "Permission",
                purpose: "The action classes allowed within the target (read, write, delete, execute, network).",
                support: Support::Enforced,
                runtime: "without write or delete, the target's files become observe-only; execute and network limits are delivered and checked, not enforced per file",
                fields: &["allow"],
            },
            Self::Constraint => Info {
                label_key: "constraint",
                label: "Constraint",
                purpose: "Hard limits no context or instruction can relax: autonomy ceiling, criticality, protected paths.",
                support: Support::Enforced,
                runtime: "lowers the target's autonomy ceiling, raises its criticality, and makes protected paths observe-only",
                fields: &["max_ceiling", "min_criticality", "protected_paths"],
            },
            Self::Precondition => Info {
                label_key: "precondition",
                label: "Precondition",
                purpose: "Facts that must hold before an agent may change the target (a file exists, a file is absent).",
                support: Support::Enforced,
                runtime: "evaluated when the limits are computed; while one fails, every change in the target needs a person's approval",
                fields: &["checks"],
            },
            Self::Approval => Info {
                label_key: "approval",
                label: "Approval",
                purpose: "When a person must approve agent changes in the target.",
                support: Support::Enforced,
                runtime: "write or delete approval makes the target's ceiling assisted; the approval is answered by the agent tool's own prompt, which Crane cannot bind to the exact change (F7)",
                fields: &["operations", "reason"],
            },
            Self::Isolation => Info {
                label_key: "isolation",
                label: "Isolation",
                purpose: "The execution boundary work in the target requires.",
                support: Support::Enforced,
                runtime: "agent tool actions run under the agent-hooks profile; requiring a stronger or unavailable profile makes direct agent changes observe-only (fail closed); no sandbox exists",
                fields: &["profile"],
            },
            Self::Budget => Info {
                label_key: "budget",
                label: "Budget",
                purpose: "Caps on tool calls, retries, and running time.",
                support: Support::Declared,
                runtime: "validated and delivered; per-Brick meters do not exist, so only the existing autonomy credits limit agents",
                fields: &["max_tool_calls", "max_retries", "max_runtime_minutes", "on_exhausted"],
            },
            Self::Action => Info {
                label_key: "action",
                label: "Action",
                purpose: "Binds work to an explicitly registered, typed operation adapter.",
                support: Support::Checked,
                runtime: "the adapter and operation must be registered; a Brick never executes anything, operations run only through 'crane skill invoke'",
                fields: &["adapter", "operation"],
            },
            Self::Verification => Info {
                label_key: "verification",
                label: "Verification",
                purpose: "Objective postconditions and the evidence that proves them.",
                support: Support::Checked,
                runtime: "checks run by 'crane brick test' and the monitors: proven, contradicted, unknown, or not run",
                fields: &["checks"],
            },
            Self::Provenance => Info {
                label_key: "provenance",
                label: "Provenance",
                purpose: "The records an outcome must be traceable to (signed state, audit chain).",
                support: Support::Checked,
                runtime: "verified against the signed manifest and the audit chain by 'crane brick test'",
                fields: &["require"],
            },
            Self::Recovery => Info {
                label_key: "recovery",
                label: "Recovery",
                purpose: "What happens after failed, partial, or uncertain outcomes.",
                support: Support::Declared,
                runtime: "validated (reconciliation before retry is required) and delivered; the invocation engine applies reconciliation to skills",
                fields: &["on_failure", "max_retries", "reconcile_before_retry"],
            },
            Self::Revocation => Info {
                label_key: "revocation",
                label: "Revocation",
                purpose: "When attachments in the target should be withdrawn.",
                support: Support::Checked,
                runtime: "the monitors report a due revocation; withdrawing is always a signed 'crane brick revoke' or 'detach' by an admin",
                fields: &["triggers"],
            },
            Self::Monitoring => Info {
                label_key: "monitoring",
                label: "Monitoring",
                purpose: "Deterministic checks run while assigned agents are idle, without model inference.",
                support: Support::Checked,
                runtime: "evaluated by every background sync and 'crane brick monitor run'; ci, metrics, and external sources are reported as coverage gaps",
                fields: &["signals"],
            },
            Self::Intelligence => Info {
                label_key: "intelligence",
                label: "Intelligence",
                purpose: "Advisory pattern analysis that may propose, never apply.",
                support: Support::Checked,
                runtime: "deterministic analyses over local events produce proposals only; nothing is changed or approved by it",
                fields: &["analyses", "advisory_only"],
            },
        }
    }
}

/** The static description of a type
 * Fields
    - label_key: &'static str - serialized name
    - label: &'static str - human label
    - purpose: &'static str - what it solves
    - support: Support - how far it is supported
    - runtime: &'static str - what really happens with it
    - fields: &'static [&'static str] - configuration fields
*/
struct Info {
    label_key: &'static str,
    label: &'static str,
    purpose: &'static str,
    support: Support,
    runtime: &'static str,
    fields: &'static [&'static str],
}
