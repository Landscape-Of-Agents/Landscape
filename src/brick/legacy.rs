// Former Brick types. Signed stores written before the twelve current kinds keep verifying: their
// entries are never rewritten in place (that would change their digests). They are parsed with
// their own former schemas (unknown fields refused, as before) and converted into the current
// kinds they stand for, so composition, enforcement, and tests see only current kinds:
//   context -> knowledge            scope, permission -> access
//   constraint -> limits (+ access for protected paths)
//   isolation -> environment        budget -> limits
//   revocation, monitoring, intelligence -> watch
// 'crane brick migrate' uses the same conversion to write current drafts an admin activates.

use serde::Deserialize;
use serde_json::{json, Value};

use super::config::{problem, ActionClass, Analysis, Problem, Signal, Trigger};
use super::types::BrickType;
use crate::governance::zones::{Ceiling, Criticality};

/** The former context schema
 * Fields
    - files: Vec<String> - repository files
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ContextV1 {
    files: Vec<String>,
}

/** The former scope schema
 * Fields
    - include: Vec<String> - files agents may change
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ScopeV1 {
    include: Vec<String>,
}

/** The former permission schema
 * Fields
    - allow: Vec<ActionClass> - allowed classes
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct PermissionV1 {
    allow: Vec<ActionClass>,
}

/** The former constraint schema
 * Fields
    - max_ceiling: Option<Ceiling> - highest ceiling
    - min_criticality: Option<Criticality> - lowest criticality
    - protected_paths: Vec<String> - paths agents may not change
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct ConstraintV1 {
    max_ceiling: Option<Ceiling>,
    min_criticality: Option<Criticality>,
    protected_paths: Vec<String>,
}

/** The former isolation schema
 * Fields
    - profile: String - isolation profile
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct IsolationV1 {
    profile: String,
}

/** The former budget schema
 * Fields
    - max_tool_calls: Option<u64> - tool calls
    - max_retries: Option<u32> - retries
    - max_runtime_minutes: Option<u64> - running time
    - on_exhausted: Option<String> - stop or escalate
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct BudgetV1 {
    max_tool_calls: Option<u64>,
    max_retries: Option<u32>,
    max_runtime_minutes: Option<u64>,
    on_exhausted: Option<String>,
}

/** The former revocation schema
 * Fields
    - triggers: Vec<Trigger> - conditions
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RevocationV1 {
    triggers: Vec<Trigger>,
}

/** The former monitoring schema
 * Fields
    - signals: Vec<Signal> - signals
*/
#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct MonitoringV1 {
    signals: Vec<Signal>,
}

/** The former intelligence schema
 * Fields
    - analyses: Vec<Analysis> - analyses
    - advisory_only: bool - must be true
*/
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct IntelligenceV1 {
    analyses: Vec<Analysis>,
    advisory_only: bool,
}

impl Default for IntelligenceV1 {
    /** No analyses; advisory only
     * Input
        - None
     * Output
        - IntelligenceV1
    */
    fn default() -> Self {
        Self {
            analyses: Vec::new(),
            advisory_only: true,
        }
    }
}

/** Parse a former configuration
 * Input
    - value: &Value - object (null is empty)
 * Output
    - Result<T, Problem>
*/
fn former<T: for<'de> Deserialize<'de>>(value: &Value) -> Result<T, Problem> {
    let value = if value.is_null() {
        json!({})
    } else {
        value.clone()
    };
    serde_json::from_value(value).map_err(|error| problem("CONFIG_INVALID", error.to_string()))
}

/** Convert a former configuration into the current types it stands for
 * Input
    - brick_type: BrickType - former type
    - value: &Value - stored configuration
 * Output
    - Result<Vec<(BrickType, Value)>, Problem> - one or two current (type, configuration) pairs
*/
pub(crate) fn convert(
    brick_type: BrickType,
    value: &Value,
) -> Result<Vec<(BrickType, Value)>, Problem> {
    Ok(match brick_type {
        BrickType::Context => {
            let old: ContextV1 = former(value)?;
            vec![(BrickType::Knowledge, json!({"files": old.files}))]
        }
        BrickType::Scope => {
            let old: ScopeV1 = former(value)?;
            if old.include.is_empty() {
                return Err(problem(
                    "SCOPE_EMPTY",
                    "include must name at least one pattern; an empty scope is never a wildcard",
                ));
            }
            vec![(BrickType::Access, json!({"include": old.include}))]
        }
        BrickType::Permission => {
            let old: PermissionV1 = former(value)?;
            if old.allow.is_empty() {
                return Err(problem("PERMISSION_EMPTY", "allow must name at least one action class (an empty list would read as 'nothing', state 'read' for read-only)"));
            }
            vec![(BrickType::Access, json!({"allow": old.allow}))]
        }
        BrickType::Constraint => {
            let old: ConstraintV1 = former(value)?;
            if old.max_ceiling.is_none()
                && old.min_criticality.is_none()
                && old.protected_paths.is_empty()
            {
                return Err(problem(
                    "CONSTRAINT_EMPTY",
                    "a constraint needs max_ceiling, min_criticality, or protected_paths",
                ));
            }
            let mut parts = Vec::new();
            if old.max_ceiling.is_some() || old.min_criticality.is_some() {
                parts.push((
                    BrickType::Limits,
                    json!({"max_ceiling": old.max_ceiling, "min_criticality": old.min_criticality}),
                ));
            }
            if !old.protected_paths.is_empty() {
                parts.push((BrickType::Access, json!({"read_only": old.protected_paths})));
            }
            parts
        }
        BrickType::Isolation => {
            let old: IsolationV1 = former(value)?;
            vec![(BrickType::Environment, json!({"profile": old.profile}))]
        }
        BrickType::Budget => {
            let old: BudgetV1 = former(value)?;
            if old.max_tool_calls.is_none()
                && old.max_retries.is_none()
                && old.max_runtime_minutes.is_none()
            {
                return Err(problem("BUDGET_EMPTY", "a budget needs at least one cap"));
            }
            let on_exhausted = match old.on_exhausted.as_deref() {
                None | Some("stop") => "stop",
                Some("escalate") => "escalate",
                Some(other) => {
                    return Err(problem(
                        "CONFIG_INVALID",
                        format!("unknown variant `{other}`, expected `stop` or `escalate`"),
                    ))
                }
            };
            vec![(
                BrickType::Limits,
                json!({"max_tool_calls": old.max_tool_calls, "max_retries": old.max_retries, "max_runtime_minutes": old.max_runtime_minutes, "on_exhausted": on_exhausted}),
            )]
        }
        BrickType::Revocation => {
            let old: RevocationV1 = former(value)?;
            if old.triggers.is_empty() {
                return Err(problem(
                    "REVOCATION_EMPTY",
                    "triggers must name at least one condition",
                ));
            }
            vec![(BrickType::Watch, json!({"triggers": old.triggers}))]
        }
        BrickType::Monitoring => {
            let old: MonitoringV1 = former(value)?;
            if old.signals.is_empty() {
                return Err(problem(
                    "MONITORING_EMPTY",
                    "signals must name at least one signal",
                ));
            }
            vec![(BrickType::Watch, json!({"signals": old.signals}))]
        }
        BrickType::Intelligence => {
            let old: IntelligenceV1 = former(value)?;
            if old.analyses.is_empty() {
                return Err(problem(
                    "INTELLIGENCE_EMPTY",
                    "analyses must name at least one analysis",
                ));
            }
            vec![(
                BrickType::Watch,
                json!({"analyses": old.analyses, "advisory_only": old.advisory_only}),
            )]
        }
        current => vec![(current, value.clone())],
    })
}
