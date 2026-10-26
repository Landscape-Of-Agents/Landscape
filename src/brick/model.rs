// The Brick model: a definition (one fixed type, its typed configuration, and its Common Context),
// immutable activated versions, Brick Stacks (version-pinned compositions, never a new type),
// attachments of a Brick or Stack to a Flow, Zone, or (later) Runbook, each carrying the Current
// Context of that one attachment, and agent assignments. References are always exact versions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::types::BrickType;
use crate::trust::crypto::digest_json;

/** Digest domain of a Brick definition */
const BRICK_DOMAIN: &str = "bricks/definition/v1";

/** Digest domain of a Stack definition */
const STACK_DOMAIN: &str = "bricks/stack/v1";

/** Digest domain of a context text */
pub(crate) const CONTEXT_DOMAIN: &str = "bricks/context/v1";

/** How sensitive a context is
 * Variants
    - Internal - for the repository's people and agents (the default)
    - Restricted - shown to agents working in the target; summarized elsewhere
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Sensitivity {
    #[default]
    Internal,
    Restricted,
}

/** The reusable context that travels with a Brick version
 * Fields
    - text: String - guidance (never authorization)
    - sensitivity: Sensitivity - who may read it outside the agent's context
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct CommonContext {
    pub(crate) text: String,
    pub(crate) sensitivity: Sensitivity,
}

/** Who did something
 * Fields
    - name: String - Git user name or system user
    - key: Option<String> - this machine's signing public key, when it has one
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Author {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) key: Option<String>,
}

/** A Brick definition (a draft, or the content of an activated version)
 * Fields
    - brick_id: String - stable identifier, independent of version
    - version: u32 - version number
    - brick_type: BrickType - fixed functional type
    - name: String - human name
    - description: String - what it is
    - intent: String - the question or problem it solves
    - owner: Option<String> - owning person or team
    - departments: Vec<String> - department or domain tags
    - config: Value - type-specific configuration (parsed by config::parse)
    - common_context: CommonContext - reusable guidance
    - dependencies: Vec<BrickRef> - exact versions it needs alongside it
    - created_by: Author - author
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Brick {
    pub(crate) brick_id: String,
    pub(crate) version: u32,
    #[serde(rename = "type")]
    pub(crate) brick_type: BrickType,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) intent: String,
    #[serde(default)]
    pub(crate) owner: Option<String>,
    #[serde(default)]
    pub(crate) departments: Vec<String>,
    #[serde(default)]
    pub(crate) config: Value,
    #[serde(default)]
    pub(crate) common_context: CommonContext,
    #[serde(default)]
    pub(crate) dependencies: Vec<BrickRef>,
    #[serde(default)]
    pub(crate) created_by: Author,
    #[serde(default)]
    pub(crate) created_at: u64,
}

impl Brick {
    /** The content digest of the definition (every field: a changed field is a new version)
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json(
            BRICK_DOMAIN,
            &serde_json::to_value(self).unwrap_or(Value::Null),
        )
    }

    /** The exact reference of this version
     * Input
        - None (uses self)
     * Output
        - BrickRef
    */
    pub(crate) fn reference(&self) -> BrickRef {
        BrickRef {
            brick_id: self.brick_id.clone(),
            version: self.version,
        }
    }
}

/** An exact Brick version
 * Fields
    - brick_id: String - Brick
    - version: u32 - version
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrickRef {
    pub(crate) brick_id: String,
    pub(crate) version: u32,
}

impl std::fmt::Display for BrickRef {
    /** Write "ID@N"
     * Input
        - formatter: &mut Formatter - output
     * Output
        - fmt::Result
    */
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}@{}", self.brick_id, self.version)
    }
}

/** The state of an activated version
 * Variants
    - Active - usable in attachments
    - Deprecated - kept; existing pins keep working, new attachments are refused
    - Revoked - withdrawn; pins to it fail closed until an admin detaches or upgrades them
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VersionState {
    Active,
    Deprecated,
    Revoked,
}

/** A validation outcome
 * Variants
    - NotRun - never validated
    - Passed - every check passed
    - Failed - a check failed
    - Blocked - it needs something unavailable
    - Inconclusive - required facts are missing
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValidationStatus {
    #[default]
    NotRun,
    Passed,
    Failed,
    Blocked,
    Inconclusive,
}

/** A validation summary
 * Fields
    - status: ValidationStatus - outcome
    - at: u64 - Unix milliseconds
    - problems: Vec<super::config::Problem> - problems found
    - digest: String - digest of the definition validated
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Validation {
    pub(crate) status: ValidationStatus,
    pub(crate) at: u64,
    pub(crate) problems: Vec<super::config::Problem>,
    pub(crate) digest: String,
}

/** An activated, immutable version in the signed store
 * Fields
    - brick: Brick - the exact definition
    - digest: String - its digest
    - state: VersionState - state
    - validation: Validation - the validation run at activation
    - activated_by: Author - admin who activated it
    - activated_at: u64 - Unix milliseconds
    - reason: String - why it was activated
    - self_reviewed: bool - activated by its author (only when the repository has one admin)
    - changed_by: Option<Author> - who deprecated or revoked it
    - changed_at: Option<u64> - when
    - change_reason: Option<String> - why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrickEntry {
    pub(crate) brick: Brick,
    pub(crate) digest: String,
    pub(crate) state: VersionState,
    pub(crate) validation: Validation,
    pub(crate) activated_by: Author,
    pub(crate) activated_at: u64,
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) self_reviewed: bool,
    #[serde(default)]
    pub(crate) changed_by: Option<Author>,
    #[serde(default)]
    pub(crate) changed_at: Option<u64>,
    #[serde(default)]
    pub(crate) change_reason: Option<String>,
}

/** A Brick Stack definition: a named composition of exact Brick versions (one level; a Stack never
 * contains a Stack, and is never a Brick type)
 * Fields
    - stack_id: String - stable identifier
    - version: u32 - version
    - name: String - human name
    - purpose: String - the scenario it supports
    - owner: Option<String> - owner
    - members: Vec<BrickRef> - exact members
    - created_by: Author - author
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stack {
    pub(crate) stack_id: String,
    pub(crate) version: u32,
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) purpose: String,
    #[serde(default)]
    pub(crate) owner: Option<String>,
    pub(crate) members: Vec<BrickRef>,
    #[serde(default)]
    pub(crate) created_by: Author,
    #[serde(default)]
    pub(crate) created_at: u64,
}

impl Stack {
    /** The content digest of the definition
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json(
            STACK_DOMAIN,
            &serde_json::to_value(self).unwrap_or(Value::Null),
        )
    }
}

/** An activated Stack version in the signed store
 * Fields
    - stack: Stack - the exact definition
    - digest: String - its digest
    - state: VersionState - state
    - activated_by: Author - admin
    - activated_at: u64 - Unix milliseconds
    - reason: String - why
    - changed_by, changed_at, change_reason - deprecation or revocation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StackEntry {
    pub(crate) stack: Stack,
    pub(crate) digest: String,
    pub(crate) state: VersionState,
    pub(crate) activated_by: Author,
    pub(crate) activated_at: u64,
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) changed_by: Option<Author>,
    #[serde(default)]
    pub(crate) changed_at: Option<u64>,
    #[serde(default)]
    pub(crate) change_reason: Option<String>,
}

/** What an attachment binds
 * Variants
    - Brick(BrickRef) - one exact Brick version
    - Stack { stack_id, version } - one exact Stack version
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Subject {
    Brick { brick_id: String, version: u32 },
    Stack { stack_id: String, version: u32 },
}

impl Subject {
    /** Parse "ID@N" (a Brick) or "stack:ID@N"
     * Input
        - text: &str - reference
     * Output
        - Result<Subject, String>
        - Error without an exact version
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let (stack, rest) = match text.strip_prefix("stack:") {
            Some(rest) => (true, rest),
            None => (false, text.strip_prefix("brick:").unwrap_or(text)),
        };
        let (id, version) = rest
            .split_once('@')
            .and_then(|(id, version)| {
                version
                    .trim_start_matches('v')
                    .parse::<u32>()
                    .ok()
                    .filter(|version| *version > 0 && !id.is_empty())
                    .map(|version| (id.to_string(), version))
            })
            .ok_or_else(|| format!("'{text}' must be an exact version: ID@N or stack:ID@N"))?;
        Ok(if stack {
            Self::Stack {
                stack_id: id,
                version,
            }
        } else {
            Self::Brick {
                brick_id: id,
                version,
            }
        })
    }

    /** The reference as text: "brick:ID@N" or "stack:ID@N"
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn text(&self) -> String {
        match self {
            Self::Brick { brick_id, version } => format!("brick:{brick_id}@{version}"),
            Self::Stack { stack_id, version } => format!("stack:{stack_id}@{version}"),
        }
    }
}

/** What an attachment or assignment points at
 * Variants
    - Flow - a Flow of .crane/governance.yaml
    - Zone - a Zone of .crane/governance.yaml
    - Runbook - a runbook (recorded for future use; not resolved or enforced yet)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetKind {
    Flow,
    Zone,
    Runbook,
}

/** A target
 * Fields
    - kind: TargetKind - kind
    - id: String - identifier
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub(crate) kind: TargetKind,
    pub(crate) id: String,
}

impl Target {
    /** The entity key the governance code uses: "flow:ID", "zone:ID", "runbook:ID"
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn key(&self) -> String {
        format!(
            "{}:{}",
            match self.kind {
                TargetKind::Flow => "flow",
                TargetKind::Zone => "zone",
                TargetKind::Runbook => "runbook",
            },
            self.id
        )
    }

    /** Parse "flow:ID", "zone:ID", or "runbook:ID"
     * Input
        - text: &str - key
     * Output
        - Result<Target, String>
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let (kind, id) = text
            .split_once(':')
            .ok_or_else(|| format!("'{text}' must be flow:ID, zone:ID, or runbook:ID"))?;
        let kind = match kind {
            "flow" => TargetKind::Flow,
            "zone" => TargetKind::Zone,
            "runbook" => TargetKind::Runbook,
            _ => return Err(format!("'{text}' must be flow:ID, zone:ID, or runbook:ID")),
        };
        crate::platform::validate_name("target", id)?;
        Ok(Self {
            kind,
            id: id.into(),
        })
    }
}

/** The context of one attachment (it changes only that attachment)
 * Fields
    - text: String - guidance for this target
    - updated_by: Author - who wrote it
    - updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CurrentContext {
    pub(crate) text: String,
    pub(crate) updated_by: Author,
    pub(crate) updated_at: u64,
}

/** Whether an attachment applies
 * Variants
    - Enabled - applies
    - Disabled - kept, not applied (disabling a restricting Brick is a signed widening)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AttachmentState {
    #[default]
    Enabled,
    Disabled,
}

/** A Brick or Stack attached to a target
 * Fields
    - attachment_id: String - identifier (stable across upgrades)
    - target: Target - where
    - subject: Subject - what, at an exact version
    - current_context: CurrentContext - this attachment's own context
    - state: AttachmentState - enabled or disabled
    - attached_by: Author - admin
    - attached_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attachment {
    pub(crate) attachment_id: String,
    pub(crate) target: Target,
    pub(crate) subject: Subject,
    #[serde(default)]
    pub(crate) current_context: CurrentContext,
    #[serde(default)]
    pub(crate) state: AttachmentState,
    pub(crate) attached_by: Author,
    pub(crate) attached_at: u64,
}

/** The state of an agent assignment
 * Variants
    - Active - assigned
    - Paused - kept, the agent is not expected to work
    - Revoked - withdrawn
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssignmentState {
    Active,
    Paused,
    Revoked,
}

/** An agent assigned to targets
 * Fields
    - agent: String - provider (claude, codex, copilot, gemini) or exact agent id
    - targets: Vec<Target> - assigned Flows, Zones, or Runbooks
    - state: AssignmentState - state
    - assigned_by: Author - admin
    - assigned_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Assignment {
    pub(crate) agent: String,
    pub(crate) targets: Vec<Target>,
    pub(crate) state: AssignmentState,
    pub(crate) assigned_by: Author,
    pub(crate) assigned_at: u64,
}

/** Summarize a Brick entry for listings
 * Input
    - entry: &BrickEntry - entry
 * Output
    - Value
*/
pub(crate) fn entry_summary(entry: &BrickEntry) -> Value {
    json!({
        "ref": entry.brick.reference().to_string(),
        "brick_id": entry.brick.brick_id,
        "version": entry.brick.version,
        "type": entry.brick.brick_type,
        "name": entry.brick.name,
        "intent": entry.brick.intent,
        "owner": entry.brick.owner,
        "departments": entry.brick.departments,
        "state": entry.state,
        "digest": entry.digest,
        "validation": entry.validation.status,
        "activated_by": entry.activated_by.name,
        "activated_at": entry.activated_at,
        "self_reviewed": entry.self_reviewed,
        "change_reason": entry.change_reason,
    })
}
