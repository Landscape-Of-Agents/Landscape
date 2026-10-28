// The Crane view a resident's actions are decided against. It is read from the signed state every
// time it is needed (activation start and immediately before each step), never cached: whether the
// signed state verifies, the signed Brick assignments, each target's lifecycle, autonomy ceiling,
// and criticality from .crane/governance.yaml, the Brick controls folded onto each target, and
// which action connectors are configured. Nothing an assignment, definition, RCA graph, context,
// or trigger says can loosen it.
//
// The decision of one step, strictest wins:
//   signed state not verifying, no active signed Brick assignment for the agent on the target, an
//   assignment that is not active or has expired, a definition that is not active or changed, a
//   missing or inactive target, or a step type the definition does not allow     -> DENY
//   a step type without an adapter, or a connector that is not configured          -> UNAVAILABLE
//   read-only and internal steps                                                    -> ALLOW
//   external steps: an observe ceiling                                              -> DENY
//                   an assisted ceiling, restricted criticality, a Brick approval,
//                   read-only, or fail-closed control, a runbook target (Bricks are
//                   not applied to runbooks), or a step the definition does not auto-run
//                                                                                   -> REQUIRE_APPROVAL
//                   otherwise                                                       -> ALLOW

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{Assignment, DefinitionRecord, DefinitionState, Lifecycle};
use super::plan::{Effect, PlanStep, StepType};
use crate::brick::model::{AssignmentState, TargetKind};
use crate::governance::zones::{Ceiling, Criticality, Lifecycle as TargetLifecycle};
use crate::trust::crypto::digest_json;

/** Whether the signed state vouches for the rules
 * Variants
    - Verified - signature, rollback anchor, and every covered digest verify
    - Absent - there is no signed Brick store, so no signed assignment exists
    - Unverified - it does not verify (why)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
pub(crate) enum Trust {
    Verified,
    Absent,
    Unverified(String),
}

/** What Crane says about one target
 * Fields
    - status: TargetLifecycle - proposed, active, or deprecated
    - ceiling: Ceiling - autonomy ceiling (after Brick constraints)
    - criticality: Criticality - criticality (after Brick constraints)
    - approval: bool - a Brick requires approval for changes
    - read_only: bool - a Brick makes it read-only
    - fail_closed: Vec<String> - pins Crane cannot honor
    - operations: Vec<String> - the step types Action Bricks allow ("resident/STEP"; empty: no
      Action Brick)
    - missing_credentials: Vec<String> - credentials Environment Bricks name that this machine's
      vault does not hold
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TargetFacts {
    pub(crate) status: TargetLifecycle,
    pub(crate) ceiling: Ceiling,
    pub(crate) criticality: Criticality,
    pub(crate) approval: bool,
    pub(crate) read_only: bool,
    pub(crate) fail_closed: Vec<String>,
    // Omitted when empty, so the digests of views computed before these fields do not change
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) operations: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) missing_credentials: Vec<String>,
}

/** A signed Brick assignment as the resident runtime needs it
 * Fields
    - agent: String - provider or agent id
    - targets: Vec<String> - target keys
    - active: bool - its signed state is active
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SignedAssignment {
    pub(crate) agent: String,
    pub(crate) targets: Vec<String>,
    pub(crate) active: bool,
}

/** The Crane view
 * Fields
    - trust: Trust - whether the signed state verifies
    - generation: u64 - signed generation
    - governance_digest: String - digest of governance.yaml in the signed state
    - bricks_digest: String - digest of the signed Brick store
    - assignments: Vec<SignedAssignment> - signed Brick assignments
    - targets: BTreeMap<String, TargetFacts> - Zones and Flows by key
    - connectors: BTreeMap<String, bool> - action connectors and whether they are configured
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Authority {
    pub(crate) trust: Trust,
    pub(crate) generation: u64,
    pub(crate) governance_digest: String,
    pub(crate) bricks_digest: String,
    pub(crate) assignments: Vec<SignedAssignment>,
    pub(crate) targets: BTreeMap<String, TargetFacts>,
    pub(crate) connectors: BTreeMap<String, bool>,
}

impl Authority {
    /** The digest approvals and validations are bound to: the signed generation and the digests
     * of governance and the Brick store
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json(
            "resident/authority/v1",
            &json!([self.generation, self.governance_digest, self.bricks_digest]),
        )[..24]
            .to_string()
    }
}

/** A Crane decision for a resident
 * Variants
    - Allow - may run now
    - RequireApproval - a person must approve this exact step first
    - Deny - may not run
    - Unavailable - no adapter or connector can run it
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Decision {
    Allow,
    RequireApproval,
    Unavailable,
    Deny,
}

/** A decision with its reasons
 * Fields
    - decision: Decision - decision
    - reasons: Vec<(String, String)> - codes and explanations
    - authority_digest: String - the signed state it was decided under
    - generation: u64 - signed generation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Authorization {
    pub(crate) decision: Decision,
    pub(crate) reasons: Vec<(String, String)>,
    pub(crate) authority_digest: String,
    pub(crate) generation: u64,
}

impl Authorization {
    /** Make the decision stricter with a reason
     * Input
        - decision: Decision - at least this strict
        - code: &str - reason code
        - message: String - explanation
     * Output
        - None
    */
    fn tighten(&mut self, decision: Decision, code: &str, message: String) {
        if decision > self.decision {
            self.decision = decision;
        }
        self.reasons.push((code.into(), message));
    }

    /** The reason codes
     * Input
        - None (uses self)
     * Output
        - Vec<String>
    */
    pub(crate) fn codes(&self) -> Vec<String> {
        self.reasons.iter().map(|(code, _)| code.clone()).collect()
    }
}

/** Decide whether an assignment may act at all now (the checks shared by every step)
 * Input
    - authority: &Authority - Crane view, freshly read
    - assignment: &Assignment - assignment
    - definition: Option<&DefinitionRecord> - its definition version
    - now: u64 - Unix milliseconds
 * Output
    - Authorization
*/
pub(crate) fn admit(
    authority: &Authority,
    assignment: &Assignment,
    definition: Option<&DefinitionRecord>,
    now: u64,
) -> Authorization {
    let mut answer = Authorization {
        decision: Decision::Allow,
        reasons: Vec::new(),
        authority_digest: authority.digest(),
        generation: authority.generation,
    };
    match &authority.trust {
        Trust::Verified => {}
        Trust::Absent => answer.tighten(
            Decision::Deny,
            "NO_SIGNED_STORE",
            "there is no signed Brick store, so no signed assignment authorizes this agent ('crane brick assign')".into(),
        ),
        Trust::Unverified(reason) => answer.tighten(
            Decision::Deny,
            "SIGNED_STATE_UNVERIFIED",
            format!("the signed state does not verify: {reason}"),
        ),
    }
    let target = assignment.target.key();
    let signed = authority.assignments.iter().any(|signed| {
        signed.active && signed.agent == assignment.agent && signed.targets.contains(&target)
    });
    if !signed {
        answer.tighten(
            Decision::Deny,
            "NO_SIGNED_ASSIGNMENT",
            format!(
                "no active signed Brick assignment puts {} on {target} ('crane brick assign {} --targets {target}')",
                assignment.agent, assignment.agent
            ),
        );
    }
    match assignment.lifecycle {
        Lifecycle::Active if assignment.expired_at(now) => answer.tighten(
            Decision::Deny,
            "ASSIGNMENT_EXPIRED",
            "the assignment's lifetime has ended; renew it to continue".into(),
        ),
        Lifecycle::Active => {}
        other => answer.tighten(
            Decision::Deny,
            "ASSIGNMENT_NOT_ACTIVE",
            format!("the assignment is {}", crate::resident::word(&other)),
        ),
    }
    match definition {
        None => answer.tighten(
            Decision::Deny,
            "DEFINITION_MISSING",
            format!(
                "definition {}@{} does not exist",
                assignment.definition_id, assignment.definition_version
            ),
        ),
        Some(record) if record.state != DefinitionState::Active => answer.tighten(
            Decision::Deny,
            "DEFINITION_NOT_ACTIVE",
            format!(
                "definition {}@{} is {}",
                assignment.definition_id,
                assignment.definition_version,
                crate::resident::word(&record.state)
            ),
        ),
        Some(record) if record.digest != assignment.definition_digest => answer.tighten(
            Decision::Deny,
            "DEFINITION_CHANGED",
            "the definition's digest differs from the one the assignment pinned".into(),
        ),
        Some(_) => {}
    }
    if assignment.target.kind != TargetKind::Runbook {
        match authority.targets.get(&target) {
            None => answer.tighten(
                Decision::Deny,
                "TARGET_MISSING",
                format!("{target} is not a Zone or Flow of .crane/governance.yaml"),
            ),
            Some(facts) if facts.status != TargetLifecycle::Active => answer.tighten(
                Decision::Deny,
                "TARGET_NOT_ACTIVE",
                format!("{target} is {:?}", facts.status).to_ascii_lowercase(),
            ),
            Some(_) => {}
        }
    }
    answer
}

/** The integration an external step type needs, with the command that configures it
 * Input
    - step_type: StepType - type
 * Output
    - Option<(&'static str, &'static str)> (connector name, setup command)
*/
pub(crate) fn connector(step_type: StepType) -> Option<(&'static str, &'static str)> {
    match step_type {
        StepType::NotifySlack => Some(("slack", "crane integrate slack")),
        StepType::CreateTicket => Some(("jira", "crane integrate jira")),
        _ => None,
    }
}

/** Decide one step, immediately before it would run
 * Input
    - authority: &Authority - Crane view, freshly read
    - assignment: &Assignment - assignment
    - definition: Option<&DefinitionRecord> - its definition version
    - step: &PlanStep - step
    - now: u64 - Unix milliseconds
 * Output
    - Authorization
*/
pub(crate) fn authorize(
    authority: &Authority,
    assignment: &Assignment,
    definition: Option<&DefinitionRecord>,
    step: &PlanStep,
    now: u64,
) -> Authorization {
    let mut answer = admit(authority, assignment, definition, now);
    let policy = definition.and_then(|record| record.definition.policy(step.step_type));
    if policy.is_none() {
        answer.tighten(
            Decision::Deny,
            "STEP_NOT_ALLOWED",
            format!("the definition does not allow {}", step.step_type.name()),
        );
    }
    let info = step.step_type.info();
    if let Some(reason) = info.available {
        answer.tighten(Decision::Unavailable, "STEP_UNAVAILABLE", reason.into());
    }
    if let Some((name, setup)) = connector(step.step_type) {
        if !authority.connectors.get(name).copied().unwrap_or(false) {
            answer.tighten(
                Decision::Unavailable,
                "CONNECTOR_NOT_CONFIGURED",
                format!("{name} is not configured ('{setup}')"),
            );
        }
    }
    if info.effect != Effect::External {
        return answer;
    }
    let target = assignment.target.key();
    match authority.targets.get(&target) {
        None if assignment.target.kind == TargetKind::Runbook => answer.tighten(
            Decision::RequireApproval,
            "RUNBOOK_TARGET_UNENFORCED",
            "Bricks are not applied to runbooks yet, so external steps of a runbook target need a person".into(),
        ),
        None => {}
        Some(facts) => {
            match facts.ceiling {
                Ceiling::Observe => answer.tighten(
                    Decision::Deny,
                    "ZONE_OBSERVE",
                    format!("{target} has the observe ceiling: no external effect"),
                ),
                Ceiling::Assisted => answer.tighten(
                    Decision::RequireApproval,
                    "ZONE_ASSISTED",
                    format!("{target} has the assisted ceiling: a person approves each effect"),
                ),
                Ceiling::Delegated | Ceiling::Autonomous => {}
            }
            if facts.criticality == Criticality::Restricted {
                answer.tighten(
                    Decision::RequireApproval,
                    "ZONE_RESTRICTED",
                    format!("{target} is restricted: a person approves each effect"),
                );
            }
            if facts.approval {
                answer.tighten(Decision::RequireApproval, "BRICK_APPROVAL", "an approval Brick applies to the target".into());
            }
            if facts.read_only {
                answer.tighten(Decision::RequireApproval, "BRICK_READ_ONLY", "a Brick makes the target read-only; external effects need a person".into());
            }
            if !facts.fail_closed.is_empty() {
                answer.tighten(
                    Decision::RequireApproval,
                    "BRICK_FAIL_CLOSED",
                    format!("pins Crane cannot honor: {}", facts.fail_closed.join("; ")),
                );
            }
            let wanted = format!("resident/{}", step.step_type.name());
            if !facts.operations.is_empty() && !facts.operations.contains(&wanted) {
                answer.tighten(
                    Decision::Deny,
                    "BRICK_ACTION_NOT_LISTED",
                    format!("the Action Bricks of {target} allow {}, not {wanted}", facts.operations.join(", ")),
                );
            }
            if !facts.missing_credentials.is_empty() {
                answer.tighten(
                    Decision::Unavailable,
                    "CREDENTIAL_MISSING",
                    format!("this machine's vault lacks {} ('crane secret set NAME')", facts.missing_credentials.join(", ")),
                );
            }
        }
    }
    if policy.is_some_and(|policy| !policy.auto) {
        answer.tighten(
            Decision::RequireApproval,
            "STEP_NEEDS_APPROVAL",
            format!("the definition does not auto-run {}", step.step_type.name()),
        );
    }
    answer
}

/** Read the Crane view of a repository from its signed state (every call reads it again)
 * Input
    - workspace: &crate::governance::workspace::Workspace - repository
 * Output
    - Result<Authority, String>
    - Error if the state cannot be read at all
*/
pub(crate) fn load(
    workspace: &crate::governance::workspace::Workspace,
) -> Result<Authority, String> {
    let (state, store, trust) = crate::brick::store::load(workspace)?;
    let trust = match trust {
        crate::brick::store::Trust::Verified => Trust::Verified,
        crate::brick::store::Trust::Absent => Trust::Absent,
        crate::brick::store::Trust::Unverified(reason) => Trust::Unverified(reason),
    };
    let mut targets = BTreeMap::new();
    if let Ok(governance) = crate::governance::zones::load(workspace) {
        if let Ok(policies) = crate::governance::policy::PolicySet::load(workspace) {
            if let Ok(resolution) =
                crate::governance::zones::resolve(workspace, &governance, &state, &policies)
            {
                for entity in resolution.entities {
                    let key = format!("{}:{}", entity.kind, entity.id);
                    let controls = crate::brick::compose::controls(&store, &key, &workspace.root);
                    let ceiling = controls
                        .ceiling
                        .map_or(entity.ceiling, |brick| brick.min(entity.ceiling));
                    let criticality = controls
                        .criticality
                        .map_or(entity.criticality, |brick| brick.max(entity.criticality));
                    targets.insert(
                        key,
                        TargetFacts {
                            status: entity.status,
                            ceiling,
                            criticality,
                            approval: !controls.approvals.is_empty(),
                            read_only: controls.mode(crate::brick::config::ChangeClass::Update)
                                == Some(crate::brick::config::Mode::Deny)
                                && controls.mode(crate::brick::config::ChangeClass::Create)
                                    == Some(crate::brick::config::Mode::Deny),
                            operations: controls
                                .operations
                                .iter()
                                .map(|(adapter, operation)| format!("{adapter}/{operation}"))
                                .collect(),
                            missing_credentials: controls
                                .credentials
                                .iter()
                                .filter(|name| !crate::trust::vault::exists(workspace, name))
                                .cloned()
                                .collect(),
                            fail_closed: controls.fail_closed,
                        },
                    );
                }
            }
        }
    }
    let assignments = store
        .assignments
        .iter()
        .map(|assignment| SignedAssignment {
            agent: assignment.agent.clone(),
            targets: assignment
                .targets
                .iter()
                .map(crate::brick::model::Target::key)
                .collect(),
            active: assignment.state == AssignmentState::Active,
        })
        .collect();
    let connectors = ["slack", "jira"]
        .into_iter()
        .map(|name| {
            (
                name.to_string(),
                crate::integrations::load(workspace, name).is_some(),
            )
        })
        .collect();
    Ok(Authority {
        trust,
        generation: state.generation(),
        governance_digest: state.governance_digest.clone(),
        bricks_digest: state.bricks_digest.clone(),
        assignments,
        targets,
        connectors,
    })
}
