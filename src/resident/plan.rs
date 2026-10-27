// Typed action plans. The step types are a closed, versioned registry: a definition chooses from
// it, a plan is a list of steps of those types with dependencies on earlier steps, and the plan
// digest binds approvals to its exact content. A type without a real adapter is listed as
// unavailable with the reason; it can be planned (so people see what is missing) but never runs and
// never counts as a success.
//
// Effect classes: read_only (reads evidence), internal (changes only Crane's own records: an RCA
// proposal, a wait, a question), external (a side effect in another system). Only external steps
// can need Crane's approval; waits for people are their own step types.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::Definition;
use super::rca::CheckSpec;
use crate::trust::crypto::digest_json;

/** Version of the step-type registry */
pub(crate) const REGISTRY_VERSION: u32 = 1;

/** Longest text parameter of a step, in characters */
const MAX_TEXT: usize = 1500;

/** The closed set of action-step types
 * Variants
    - ReadCheck - read an authorized state or evidence snapshot
    - CreateTicket - create a ticket (no adapter: unavailable)
    - NotifySlack - post to the configured Slack channel
    - PageOncall - page on-call (no adapter: unavailable)
    - ExecuteProcedure - run a versioned runbook step (not wired to residents: unavailable)
    - RequestApproval - pause for a person's approval bound to the plan digest
    - AskUser - pause for a person's answer
    - WaitForEvent - wait for a matching trigger until a deadline
    - Verify - evaluate an evidence-backed postcondition
    - Reconcile - establish what an uncertain external step really did
    - ProposeRca - record an RCA proposal for review (never published by itself)
    - ProposeActionPlan - submit steps for a person to accept into the plan
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum StepType {
    ReadCheck,
    CreateTicket,
    NotifySlack,
    PageOncall,
    ExecuteProcedure,
    RequestApproval,
    AskUser,
    WaitForEvent,
    Verify,
    Reconcile,
    ProposeRca,
    ProposeActionPlan,
}

/** What a step does to the world
 * Variants
    - ReadOnly - reads only
    - Internal - changes only Crane's own records
    - External - a side effect in another system
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Effect {
    ReadOnly,
    Internal,
    External,
}

/** The registry entry of a step type
 * Fields
    - step_type: StepType - type
    - effect: Effect - effect class
    - available: Option<&'static str> - None when runnable, else why it is unavailable
    - adapter: &'static str - what carries it out
    - idempotent: bool - a repeat cannot act twice
    - reconcilable: bool - its real outcome can be read back automatically
    - params: &'static [(&'static str, bool)] - parameters (name, required)
    - check: bool - it needs a check
    - evidence: &'static str - the evidence it emits
    - failure: &'static str - what a failure means
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct TypeInfo {
    pub(crate) step_type: StepType,
    pub(crate) effect: Effect,
    pub(crate) available: Option<&'static str>,
    pub(crate) adapter: &'static str,
    pub(crate) idempotent: bool,
    pub(crate) reconcilable: bool,
    pub(crate) params: &'static [(&'static str, bool)],
    pub(crate) check: bool,
    pub(crate) evidence: &'static str,
    pub(crate) failure: &'static str,
}

impl StepType {
    /** Every type, in registry order */
    pub(crate) const ALL: [Self; 12] = [
        Self::ReadCheck,
        Self::CreateTicket,
        Self::NotifySlack,
        Self::PageOncall,
        Self::ExecuteProcedure,
        Self::RequestApproval,
        Self::AskUser,
        Self::WaitForEvent,
        Self::Verify,
        Self::Reconcile,
        Self::ProposeRca,
        Self::ProposeActionPlan,
    ];

    /** The type's name as written in files
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn name(self) -> String {
        crate::resident::word(&self)
    }

    /** The registry entry of the type
     * Input
        - None (uses self)
     * Output
        - TypeInfo
    */
    pub(crate) fn info(self) -> TypeInfo {
        let entry = |effect,
                     available,
                     adapter,
                     idempotent,
                     reconcilable,
                     params,
                     check,
                     evidence,
                     failure| TypeInfo {
            step_type: self,
            effect,
            available,
            adapter,
            idempotent,
            reconcilable,
            params,
            check,
            evidence,
            failure,
        };
        match self {
            Self::ReadCheck => entry(Effect::ReadOnly, None, "deterministic checks (rca::CheckSpec)", true, true, &[], true, "the check's result and what it read", "the evidence could not be read: unknown, never passed"),
            Self::CreateTicket => entry(Effect::External, None, "the Jira integration ('crane integrate jira'): an issue in the configured project, labelled with the step's key", true, true, &[("summary", true), ("description", false)], false, "the issue key", "known failure: retried within the limit; unknown outcome: the label is looked up before anything is repeated"),
            Self::NotifySlack => entry(Effect::External, None, "the Slack integration ('crane integrate slack'), the configured channel only", false, false, &[("text", true)], false, "the HTTP status Slack answered", "known failure: retried within the limit; unknown outcome: reconciliation by a person"),
            Self::PageOncall => entry(Effect::External, Some("no on-call adapter is registered"), "none", false, false, &[("summary", true)], false, "none", "unavailable"),
            Self::ExecuteProcedure => entry(Effect::External, Some("residents cannot start runbooks yet; a person runs 'crane runbook run'"), "none", false, false, &[("runbook", true)], false, "none", "unavailable"),
            Self::RequestApproval => entry(Effect::Internal, None, "a person ('crane resident approve')", true, true, &[("question", true)], false, "the approver, decision, and digest", "denied: the activation is blocked"),
            Self::AskUser => entry(Effect::Internal, None, "a person ('crane resident answer')", true, true, &[("question", true)], false, "the answer (redacted)", "no answer before the deadline: the step fails"),
            Self::WaitForEvent => entry(Effect::Internal, None, "later triggers (no polling, no model)", true, true, &[("kind", true)], false, "the trigger that arrived", "deadline passed: the step fails"),
            Self::Verify => entry(Effect::ReadOnly, None, "deterministic checks (rca::CheckSpec)", true, true, &[], true, "the postcondition's result and what it read", "contradicted: the activation fails; unknown: never completed"),
            Self::Reconcile => entry(Effect::Internal, None, "a person establishes the real outcome ('crane resident reconcile')", true, true, &[("step", true)], false, "the declared outcome", "unresolved: the activation stays in reconciliation"),
            Self::ProposeRca => entry(Effect::Internal, None, "the resident store (a proposal, never published by itself)", true, true, &[("label", true)], false, "the proposal reference", "none"),
            Self::ProposeActionPlan => entry(Effect::Internal, None, "a person accepts the proposed steps ('crane resident approve')", true, true, &[("summary", true)], false, "the accepted steps and the new plan digest", "rejected: the activation is blocked"),
        }
    }
}

/** One step of a plan
 * Fields
    - step_id: String - identifier, unique in the plan
    - step_type: StepType - type
    - params: BTreeMap<String, String> - typed, bounded parameters
    - check: Option<CheckSpec> - the check of a READ_CHECK or VERIFY step
    - depends_on: Vec<String> - earlier steps that must succeed first
    - proposal: Vec<PlanStep> - the steps a PROPOSE_ACTION_PLAN step submits
    - basis: String - why the step is in the plan (RCA node, investigation result)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlanStep {
    pub(crate) step_id: String,
    pub(crate) step_type: StepType,
    #[serde(default)]
    pub(crate) params: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) check: Option<CheckSpec>,
    #[serde(default)]
    pub(crate) depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) proposal: Vec<PlanStep>,
    #[serde(default)]
    pub(crate) basis: String,
}

/** A typed plan
 * Fields
    - registry_version: u32 - REGISTRY_VERSION
    - steps: Vec<PlanStep> - steps, dependencies before dependents
    - digest: String - digest of the steps (approvals bind to it)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Plan {
    pub(crate) registry_version: u32,
    pub(crate) steps: Vec<PlanStep>,
    pub(crate) digest: String,
}

impl Plan {
    /** Build a plan and compute its digest
     * Input
        - steps: Vec<PlanStep> - steps
     * Output
        - Plan
    */
    pub(crate) fn new(steps: Vec<PlanStep>) -> Self {
        let digest = digest_json("resident/plan/v1", &json!(steps))[..32].to_string();
        Self {
            registry_version: REGISTRY_VERSION,
            steps,
            digest,
        }
    }

    /** A step by id
     * Input
        - step_id: &str - step
     * Output
        - Option<&PlanStep>
    */
    pub(crate) fn step(&self, step_id: &str) -> Option<&PlanStep> {
        self.steps.iter().find(|step| step.step_id == step_id)
    }

    /** The plan with the accepted steps of a proposal inserted after the proposing step (ids
     * prefixed by the proposing step, dependencies rewritten), and a new digest
     * Input
        - step_id: &str - the PROPOSE_ACTION_PLAN step
     * Output
        - Plan
    */
    pub(crate) fn accept(&self, step_id: &str) -> Self {
        let mut steps = Vec::new();
        for step in &self.steps {
            let mut kept = step.clone();
            let proposal = std::mem::take(&mut kept.proposal);
            steps.push(kept);
            if step.step_id == step_id {
                let local = proposal
                    .iter()
                    .map(|item| item.step_id.clone())
                    .collect::<BTreeSet<_>>();
                for mut item in proposal {
                    item.step_id = format!("{step_id}-{}", item.step_id);
                    item.depends_on = std::iter::once(step_id.to_string())
                        .chain(item.depends_on.iter().map(|dependency| {
                            if local.contains(dependency) {
                                format!("{step_id}-{dependency}")
                            } else {
                                dependency.clone()
                            }
                        }))
                        .collect();
                    item.basis = format!("accepted from {step_id}: {}", item.basis);
                    steps.push(item);
                }
            }
        }
        Self::new(steps)
    }
}

/** Validate a plan against the registry and a definition: unique ids, dependencies on earlier
 * steps only (so the plan is acyclic), allowed types, required parameters, bounded text without
 * secrets, checks where needed, and no nested proposals
 * Input
    - plan: &Plan - plan
    - definition: &Definition - definition it runs under
 * Output
    - Vec<(String, String)> problems (codes and explanations); empty when valid
*/
pub(crate) fn validate(plan: &Plan, definition: &Definition) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let mut push = |code: &str, message: String| problems.push((code.to_string(), message));
    if plan.steps.is_empty() {
        push("PLAN_EMPTY", "the plan has no step".into());
    }
    if plan.steps.len() > definition.limits.max_steps as usize {
        push(
            "PLAN_TOO_LONG",
            format!(
                "{} steps exceed the definition's limit of {}",
                plan.steps.len(),
                definition.limits.max_steps
            ),
        );
    }
    if plan.registry_version != REGISTRY_VERSION {
        push(
            "PLAN_REGISTRY_VERSION",
            format!("the plan uses step registry v{}", plan.registry_version),
        );
    }
    let mut seen = BTreeSet::new();
    for step in &plan.steps {
        check_step(step, definition, &seen, false, &mut push);
        seen.insert(step.step_id.clone());
    }
    let all = plan
        .steps
        .iter()
        .flat_map(|step| std::iter::once(step).chain(step.proposal.iter()))
        .collect::<Vec<_>>();
    if definition.verification_required
        && all
            .iter()
            .any(|step| step.step_type.info().effect == Effect::External)
        && !all.iter().any(|step| step.step_type == StepType::Verify)
    {
        push(
            "PLAN_UNVERIFIED",
            "the plan has an external step but no VERIFY step, and the definition requires verification".into(),
        );
    }
    problems
}

/** Validate one step
 * Input
    - step: &PlanStep - step
    - definition: &Definition - definition
    - earlier: &BTreeSet<String> - ids of earlier steps
    - nested: bool - the step is inside a proposal
    - push: &mut impl FnMut(&str, String) - problem sink
 * Output
    - None
*/
fn check_step(
    step: &PlanStep,
    definition: &Definition,
    earlier: &BTreeSet<String>,
    nested: bool,
    push: &mut impl FnMut(&str, String),
) {
    let id = &step.step_id;
    if crate::platform::validate_name("step", id).is_err() {
        push(
            "PLAN_STEP_ID",
            format!("step id '{id}' must be letters, digits, '_' or '-'"),
        );
    }
    if earlier.contains(id) {
        push("PLAN_DUPLICATE_STEP", format!("step {id} appears twice"));
    }
    for dependency in &step.depends_on {
        if !earlier.contains(dependency) {
            push(
                "PLAN_BAD_DEPENDENCY",
                format!("step {id} depends on {dependency}, which is not an earlier step"),
            );
        }
    }
    let info = step.step_type.info();
    if definition.policy(step.step_type).is_none() {
        push(
            "PLAN_STEP_NOT_ALLOWED",
            format!(
                "step {id} is {}, which the definition does not allow",
                step.step_type.name()
            ),
        );
    }
    for (name, required) in info.params {
        if *required
            && step
                .params
                .get(*name)
                .is_none_or(|value| value.trim().is_empty())
        {
            push(
                "PLAN_MISSING_PARAMETER",
                format!("step {id} needs parameter '{name}'"),
            );
        }
    }
    for (name, value) in &step.params {
        if !info.params.iter().any(|(known, _)| known == name)
            && name != "within_minutes"
            && name != "source"
        {
            push(
                "PLAN_UNKNOWN_PARAMETER",
                format!("step {id} has unknown parameter '{name}'"),
            );
        }
        if value.chars().count() > MAX_TEXT || value.chars().any(|c| c.is_control() && c != '\n') {
            push(
                "PLAN_PARAMETER_INVALID",
                format!("parameter '{name}' of step {id} is too long or has control characters"),
            );
        }
        if crate::telemetry::redact::redact(value).1 > 0 {
            push(
                "PLAN_SECRET",
                format!("parameter '{name}' of step {id} holds a secret-looking value"),
            );
        }
    }
    if info.check && step.check.is_none() {
        push(
            "PLAN_MISSING_CHECK",
            format!("step {id} ({}) needs a check", step.step_type.name()),
        );
    }
    if let Some(check) = &step.check {
        if let Err(error) = check.validate() {
            push("PLAN_BAD_CHECK", format!("step {id}: {error}"));
        }
    }
    if !step.proposal.is_empty() && step.step_type != StepType::ProposeActionPlan {
        push(
            "PLAN_UNEXPECTED_PROPOSAL",
            format!("step {id} carries proposed steps but is not PROPOSE_ACTION_PLAN"),
        );
    }
    if step.step_type == StepType::ProposeActionPlan {
        if nested {
            push(
                "PLAN_NESTED_PROPOSAL",
                format!("step {id}: a proposal cannot contain another proposal"),
            );
        }
        let mut local = earlier.clone();
        local.insert(id.clone());
        // A proposed step may depend on earlier plan steps, the proposing step, and earlier
        // proposed steps
        for item in &step.proposal {
            check_step(item, definition, &local, true, push);
            local.insert(item.step_id.clone());
        }
    }
}

/** Describe the registry for listings and the dashboard
 * Input
    - definition: Option<&Definition> - mark the types a definition allows
 * Output
    - Vec<Value>
*/
pub(crate) fn catalog(definition: Option<&Definition>) -> Vec<Value> {
    StepType::ALL
        .iter()
        .map(|step_type| {
            let info = step_type.info();
            json!({
                "type": step_type.name(),
                "effect": info.effect,
                "available": info.available.is_none(),
                "unavailable_reason": info.available,
                "adapter": info.adapter,
                "idempotent": info.idempotent,
                "reconcilable": info.reconcilable,
                "params": info.params.iter().map(|(name, required)| json!({"name": name, "required": required})).collect::<Vec<_>>(),
                "needs_check": info.check,
                "evidence": info.evidence,
                "failure": info.failure,
                "allowed": definition.map(|definition| definition.policy(*step_type).is_some()),
                "auto": definition.and_then(|definition| definition.policy(*step_type)).map(|policy| policy.auto),
            })
        })
        .collect()
}
