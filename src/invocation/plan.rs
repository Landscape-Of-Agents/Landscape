// The invocation plan (PLAN_VERSION): a separate, versioned, restricted linear plan built from the
// exact published definition and a reviewed binding set. The capability schema describes steps; it
// does not name operations, so each step is bound explicitly to a registered, typed operation, and
// each required claim to a verification method. Recorded tool calls are evidence and are never
// executed. A plan is refused (the invocation is blocked) when:
//
//   - a step has no binding to a registered operation, or a binding names an unknown step
//   - the operation does not implement the step's kind, or its effect class does not match the
//     step (a state-changing step needs a state-changing operation and a read-only step a read-only one)
//   - a parameter is unknown, missing, mistyped, unbounded, or touches Crane policy
//   - a required capability input is missing
//   - the candidate came from the compiler and its report shows a hypothesis step, a step whose kind
//     was only assumed (an unrecognized command), or a procedure branch, which the schema cannot
//     express; ambiguous control flow is never invented or silently linearized
//   - a verification method is not read-only, or is one of the plan's own operations
//   - the plan exceeds its operation limit
//
// Approval requirements follow the existing risk and review semantics: any operation that is not
// read-only, any medium or higher risk, and any approval step require an approver of at least the
// capability's review level (owner for state-changing operations).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::adapter::{validate_params, AdapterRegistry};
use super::engine::InvocationRequest;
use super::lifecycle::EffectClass;
use super::PLAN_VERSION;
use crate::capability::model::{
    CapabilityDefinition, ClaimStatus, CreationMethod, ReviewLevel, RiskLevel, StepKind,
};
use crate::capability::refs::Sources;
use crate::capability::validate::touches_policy;
use crate::capture::format::record::EvidenceType;
use crate::capture::stable_id;
use crate::compiler::assemble::CompilationReport;
use crate::compiler::extract::recognized;
use crate::compiler::Basis;
use crate::telemetry::redact::redact;
use crate::trust::crypto::digest_json;

/** Where a parameter value comes from
 * Variants
    - Literal - a fixed value in the reviewed binding set
    - Input - a named input of the invocation request
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "from", rename_all = "snake_case")]
pub(crate) enum ParamSource {
    Literal { value: String },
    Input { name: String },
}

/** A step bound to a registered operation
 * Fields
    - step_id: String - procedure step
    - adapter_id: String - adapter
    - operation_id: String - operation
    - params: BTreeMap<String, ParamSource> - parameter sources
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct StepBinding {
    pub(crate) step_id: String,
    pub(crate) adapter_id: String,
    pub(crate) operation_id: String,
    pub(crate) params: BTreeMap<String, ParamSource>,
}

/** How a claim is verified
 * Variants
    - AdapterCheck - a fresh read through a read-only operation that is not one of the plan's own,
      whose result field must equal a value
    - WorkCaptureEvidence - an existing Work Capture record of a type whose field equals a value,
      read through the requester's access policy (reused episode evidence, not fresh)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub(crate) enum VerificationMethod {
    AdapterCheck {
        adapter_id: String,
        operation_id: String,
        params: BTreeMap<String, ParamSource>,
        field: String,
        equals: String,
    },
    WorkCaptureEvidence {
        evidence_type: EvidenceType,
        field: String,
        equals: String,
    },
}

/** A claim bound to a verification method
 * Fields
    - claim_id: String - claim
    - method: VerificationMethod - method
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct VerificationBinding {
    pub(crate) claim_id: String,
    pub(crate) method: VerificationMethod,
}

/** A reviewed binding set for one exact capability version
 * Fields
    - binding_id: String - identifier
    - version: u32 - version
    - capability_id: String - capability
    - capability_version: u32 - version it binds
    - steps: Vec<StepBinding> - step bindings
    - verifications: Vec<VerificationBinding> - verification bindings
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BindingSet {
    pub(crate) binding_id: String,
    pub(crate) version: u32,
    pub(crate) capability_id: String,
    pub(crate) capability_version: u32,
    pub(crate) steps: Vec<StepBinding>,
    pub(crate) verifications: Vec<VerificationBinding>,
}

/** Execution limits
 * Fields
    - max_operations: usize - most operations
    - max_output_bytes: usize - largest normalized result per operation
    - deadline_ms: u64 - latest wall-clock time after execution starts
    - max_retries: u32 - retries allowed per operation, only when no effect can have happened
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlanLimits {
    pub(crate) max_operations: usize,
    pub(crate) max_output_bytes: usize,
    pub(crate) deadline_ms: u64,
    pub(crate) max_retries: u32,
}

impl Default for PlanLimits {
    /** 16 operations, 16 KiB per result, 60 seconds, one retry
     * Input
        - None
     * Output
        - PlanLimits
    */
    fn default() -> Self {
        Self {
            max_operations: 16,
            max_output_bytes: 16 * 1024,
            deadline_ms: 60_000,
            max_retries: 1,
        }
    }
}

/** A resolved input and its provenance
 * Fields
    - name: String - input
    - value: String - value (redacted)
    - provenance: String - where it came from
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InputBinding {
    pub(crate) name: String,
    pub(crate) value: String,
    pub(crate) provenance: String,
}

/** One planned operation
 * Fields
    - index: usize - position (0-based)
    - step_id: String - procedure step
    - step_kind: StepKind - its kind
    - adapter_id: String - adapter
    - operation_id: String - operation
    - effect: EffectClass - effect class
    - params: BTreeMap<String, String> - resolved parameters (redacted)
    - preconditions: Vec<String> - preconditions to hold before it starts
    - verifies: Vec<String> - claims it contributes to
    - required_permissions: Vec<String> - permissions it needs
    - idempotency_key: String - operation idempotency key
    - idempotent: bool - idempotent
    - reconcilable: bool - reconcilable
    - requires_transaction: bool - needs a durable transaction
    - max_output_bytes: usize - output limit
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlannedOperation {
    pub(crate) index: usize,
    pub(crate) step_id: String,
    pub(crate) step_kind: StepKind,
    pub(crate) adapter_id: String,
    pub(crate) operation_id: String,
    pub(crate) effect: EffectClass,
    pub(crate) params: BTreeMap<String, String>,
    pub(crate) preconditions: Vec<String>,
    pub(crate) verifies: Vec<String>,
    pub(crate) required_permissions: Vec<String>,
    pub(crate) idempotency_key: String,
    pub(crate) idempotent: bool,
    pub(crate) reconcilable: bool,
    pub(crate) requires_transaction: bool,
    pub(crate) max_output_bytes: usize,
}

/** One planned postcondition check
 * Fields
    - claim_id: String - claim
    - requirement: String - what must hold
    - required: bool - whether the claim is required
    - method: Option<VerificationMethod> - method, None when no method is bound (never verified)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PlannedVerification {
    pub(crate) claim_id: String,
    pub(crate) requirement: String,
    pub(crate) required: bool,
    pub(crate) method: Option<VerificationMethod>,
}

/** The approval a plan needs
 * Fields
    - required: bool - whether approval is needed
    - level: ReviewLevel - minimum approver level
    - reasons: Vec<String> - why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApprovalRequirement {
    pub(crate) required: bool,
    pub(crate) level: ReviewLevel,
    pub(crate) reasons: Vec<String>,
}

/** An invocation plan
 * Fields
    - plan_version: u32 - PLAN_VERSION
    - capability_id, version, digest - exact capability
    - binding_id: String - binding set
    - binding_version: u32 - its version
    - task_id, episode_id, environment, repository: Option<String> - context references
    - inputs: Vec<InputBinding> - inputs with provenance
    - operations: Vec<PlannedOperation> - ordered operations
    - verifications: Vec<PlannedVerification> - postcondition checks
    - dependencies: Vec<String> - dependencies and pinned versions
    - risk: RiskLevel - capability risk
    - approval: ApprovalRequirement - approval requirement
    - unresolved_assumptions: Vec<String> - unresolved assumptions carried from the definition
    - limits: PlanLimits - limits
    - plan_digest: String - digest of the plan without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InvocationPlan {
    pub(crate) plan_version: u32,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) binding_id: String,
    pub(crate) binding_version: u32,
    pub(crate) task_id: Option<String>,
    pub(crate) episode_id: Option<String>,
    pub(crate) environment: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) inputs: Vec<InputBinding>,
    pub(crate) operations: Vec<PlannedOperation>,
    pub(crate) verifications: Vec<PlannedVerification>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) risk: RiskLevel,
    pub(crate) approval: ApprovalRequirement,
    pub(crate) unresolved_assumptions: Vec<String>,
    pub(crate) limits: PlanLimits,
    pub(crate) plan_digest: String,
}

impl InvocationPlan {
    /** The digest of the plan's content
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn compute_digest(&self) -> String {
        let mut content = self.clone();
        content.plan_digest = String::new();
        digest_json("invocation/plan/v1", &json!(content))[..32].to_string()
    }
}

/** Resolve a parameter source from the request's inputs
 * Input
    - source: &ParamSource - source
    - request: &InvocationRequest - request
 * Output
    - Result<String, String>
*/
pub(crate) fn resolve(source: &ParamSource, request: &InvocationRequest) -> Result<String, String> {
    match source {
        ParamSource::Literal { value } => Ok(value.clone()),
        ParamSource::Input { name } => request
            .inputs
            .get(name)
            .cloned()
            .ok_or_else(|| format!("input '{name}' is missing")),
    }
}

/** Resolve and validate a parameter map
 * Input
    - sources: &BTreeMap<String, ParamSource> - sources
    - request: &InvocationRequest - request
    - spec: &super::adapter::OperationSpec - operation schema
 * Output
    - Result<BTreeMap<String, String>, String>
*/
fn resolve_params(
    sources: &BTreeMap<String, ParamSource>,
    request: &InvocationRequest,
    spec: &super::adapter::OperationSpec,
) -> Result<BTreeMap<String, String>, String> {
    let mut params = BTreeMap::new();
    for (name, source) in sources {
        params.insert(name.clone(), resolve(source, request)?);
    }
    validate_params(spec, &params)?;
    if let Some((name, _)) = params.iter().find(|(_, value)| touches_policy(value)) {
        return Err(format!("parameter '{name}' refers to Crane policy or governance; policy changes go through Crane's signed workflow"));
    }
    Ok(params)
}

/** Check a compiler-created candidate's steps against its compilation report
 * Input
    - definition: &CapabilityDefinition - definition
    - compilation: Option<&CompilationReport> - report
    - sources: &Sources - Work Capture records
    - problems: &mut Vec<String> - problems to extend
 * Output
    - None
*/
fn check_compilation(
    definition: &CapabilityDefinition,
    compilation: Option<&CompilationReport>,
    sources: &Sources,
    problems: &mut Vec<String>,
) {
    if definition.provenance.creation_method != CreationMethod::Compiler {
        return;
    }
    let Some(report) = compilation.filter(|report| {
        report.verify_digest()
            && report
                .candidate
                .as_ref()
                .is_some_and(|candidate| candidate.digest == definition.digest())
    }) else {
        problems.push("the candidate was compiled, but no intact compilation report for this exact digest was given; its steps cannot be shown to be safe to bind".into());
        return;
    };
    for basis in &report.step_bases {
        match basis.basis {
            Basis::Hypothesis => problems.push(format!(
                "step {} is an unverified proposal and cannot be executed",
                basis.step_id
            )),
            Basis::Observed => {
                let assumed = basis
                    .evidence_refs
                    .first()
                    .and_then(|id| sources.capture.evidence.get(id))
                    .is_some_and(|record| !recognized(record));
                if assumed {
                    problems.push(format!(
                        "step {} has an ambiguous effect: its command was not recognized",
                        basis.step_id
                    ));
                }
            }
            _ => {}
        }
    }
    for branch in report
        .branches
        .iter()
        .filter(|branch| branch.basis == Basis::Inferred)
    {
        problems.push(format!(
            "unresolved procedure branch \"{}\": the plan is linear and cannot express it",
            branch.condition
        ));
    }
}

/** Build and validate a plan
 * Input
    - definition: &CapabilityDefinition - exact published definition
    - compilation: Option<&CompilationReport> - its compilation report
    - request: &InvocationRequest - request
    - adapters: &AdapterRegistry - registered adapters
    - sources: &Sources - Work Capture records
 * Output
    - Result<InvocationPlan, Vec<String>> - the plan, or every problem that blocks it
*/
pub(crate) fn build(
    definition: &CapabilityDefinition,
    compilation: Option<&CompilationReport>,
    request: &InvocationRequest,
    adapters: &AdapterRegistry,
    sources: &Sources,
) -> Result<InvocationPlan, Vec<String>> {
    let mut problems = Vec::new();
    let bindings = &request.bindings;
    if bindings.capability_id != definition.capability_id
        || bindings.capability_version != definition.version
    {
        problems.push(format!(
            "the binding set binds {} v{}, not {} v{}",
            bindings.capability_id,
            bindings.capability_version,
            definition.capability_id,
            definition.version
        ));
    }
    check_compilation(definition, compilation, sources, &mut problems);
    for input in definition
        .execution
        .inputs
        .iter()
        .filter(|input| input.required)
    {
        if !request.inputs.contains_key(&input.name) {
            problems.push(format!("required input '{}' is missing", input.name));
        }
    }
    let step_ids = definition
        .execution
        .steps
        .iter()
        .map(|step| step.step_id.as_str())
        .collect::<BTreeSet<_>>();
    let mut bound = BTreeSet::new();
    for binding in &bindings.steps {
        if !step_ids.contains(binding.step_id.as_str()) || !bound.insert(binding.step_id.clone()) {
            problems.push(format!(
                "binding for step '{}' does not match a unique step",
                binding.step_id
            ));
        }
    }
    let mut approval = ApprovalRequirement {
        required: false,
        level: definition.applicability.review_level,
        reasons: Vec::new(),
    };
    let mut operations = Vec::new();
    for step in &definition.execution.steps {
        if step.kind == StepKind::Approval {
            approval.required = true;
            approval
                .reasons
                .push(format!("step {} is an approval", step.step_id));
            continue;
        }
        let Some(binding) = bindings
            .steps
            .iter()
            .find(|binding| binding.step_id == step.step_id)
        else {
            problems.push(format!(
                "step {} ({:?}) has no registered operation; it is not supported",
                step.step_id, step.kind
            ));
            continue;
        };
        let Some(spec) = adapters.operation(&binding.adapter_id, &binding.operation_id) else {
            problems.push(format!(
                "step {}: {}.{} is not a registered operation",
                step.step_id, binding.adapter_id, binding.operation_id
            ));
            continue;
        };
        if !spec.step_kinds.contains(&step.kind) {
            problems.push(format!(
                "step {}: {}.{} does not implement {:?} steps",
                step.step_id, spec.adapter_id, spec.operation_id, step.kind
            ));
        }
        if step.kind.mutating() != (spec.effect != EffectClass::ReadOnly) {
            problems.push(format!(
                "step {}: the {:?} step and the {:?} operation disagree about effects",
                step.step_id, step.kind, spec.effect
            ));
        }
        let params = match resolve_params(&binding.params, request, &spec) {
            Ok(params) => params,
            Err(problem) => {
                problems.push(format!("step {}: {problem}", step.step_id));
                continue;
            }
        };
        if spec.effect != EffectClass::ReadOnly {
            approval.required = true;
            approval.level = approval.level.max(ReviewLevel::Owner);
            approval
                .reasons
                .push(format!("step {} is {:?}", step.step_id, spec.effect));
        }
        let permissions = vec![
            format!("{}:{}", spec.adapter_id, spec.operation_id),
            format!("effect:{:?}", spec.effect).to_ascii_lowercase(),
        ];
        operations.push(PlannedOperation {
            index: operations.len(),
            step_id: step.step_id.clone(),
            step_kind: step.kind,
            adapter_id: spec.adapter_id.clone(),
            operation_id: spec.operation_id.clone(),
            effect: spec.effect,
            params: params
                .into_iter()
                .map(|(name, value)| (name, redact(&value).0))
                .collect(),
            preconditions: step.preconditions.clone(),
            verifies: step.verifies.clone(),
            required_permissions: permissions,
            idempotency_key: stable_id(
                "iop",
                &[
                    &request.idempotency_key,
                    &definition.digest(),
                    &step.step_id,
                ],
            ),
            idempotent: spec.idempotent,
            reconcilable: spec.reconcilable,
            requires_transaction: spec.requires_transaction,
            max_output_bytes: spec.max_output_bytes.min(request.limits.max_output_bytes),
        });
    }
    if operations.len() > request.limits.max_operations {
        problems.push(format!(
            "the plan has {} operations; the limit is {}",
            operations.len(),
            request.limits.max_operations
        ));
    }
    let used = operations
        .iter()
        .map(|operation| (operation.adapter_id.clone(), operation.operation_id.clone()))
        .collect::<BTreeSet<_>>();
    let mut verifications = Vec::new();
    for claim in &definition.verification.claims {
        let bound = bindings
            .verifications
            .iter()
            .filter(|binding| binding.claim_id == claim.claim_id)
            .collect::<Vec<_>>();
        for binding in &bound {
            if let VerificationMethod::AdapterCheck {
                adapter_id,
                operation_id,
                params,
                ..
            } = &binding.method
            {
                match adapters.operation(adapter_id, operation_id) {
                    None => problems.push(format!("verification of {}: {adapter_id}.{operation_id} is not registered", claim.claim_id)),
                    Some(spec) if spec.effect != EffectClass::ReadOnly => problems.push(format!("verification of {} must be read-only", claim.claim_id)),
                    Some(_) if used.contains(&(adapter_id.clone(), operation_id.clone())) => problems.push(format!("verification of {} reuses one of the plan's own operations; it would not be independent", claim.claim_id)),
                    Some(spec) => {
                        if let Err(problem) = resolve_params(params, request, &spec) {
                            problems.push(format!("verification of {}: {problem}", claim.claim_id));
                        }
                    }
                }
            }
        }
        let requirement = format!("{} (expected: {})", claim.statement, claim.expected_result);
        if bound.is_empty() {
            verifications.push(PlannedVerification {
                claim_id: claim.claim_id.clone(),
                requirement,
                required: claim.required,
                method: None,
            });
        } else {
            verifications.extend(bound.into_iter().map(|binding| PlannedVerification {
                claim_id: claim.claim_id.clone(),
                requirement: requirement.clone(),
                required: claim.required,
                method: Some(binding.method.clone()),
            }));
        }
    }
    for binding in &bindings.verifications {
        if !definition
            .verification
            .claims
            .iter()
            .any(|claim| claim.claim_id == binding.claim_id)
        {
            problems.push(format!(
                "verification binding names unknown claim '{}'",
                binding.claim_id
            ));
        }
    }
    let risk = definition.applicability.risk;
    if risk >= RiskLevel::Medium {
        approval.required = true;
        approval
            .reasons
            .push(format!("the capability's risk is {risk:?}"));
    }
    if !problems.is_empty() {
        return Err(problems);
    }
    let mut unresolved = definition
        .applicability
        .assumptions
        .iter()
        .filter(|assumption| assumption.status != ClaimStatus::Verified)
        .map(|assumption| format!("{}: {}", assumption.id, assumption.description))
        .collect::<Vec<_>>();
    unresolved.extend(
        definition
            .verification
            .unresolved_assumptions
            .iter()
            .cloned(),
    );
    unresolved.extend(
        verifications
            .iter()
            .filter(|verification| verification.required && verification.method.is_none())
            .map(|verification| {
                format!(
                    "required claim {} has no verification method; the invocation cannot succeed",
                    verification.claim_id
                )
            }),
    );
    let mut plan = InvocationPlan {
        plan_version: PLAN_VERSION,
        capability_id: definition.capability_id.clone(),
        version: definition.version,
        digest: definition.digest(),
        binding_id: bindings.binding_id.clone(),
        binding_version: bindings.version,
        task_id: request.task_id.clone(),
        episode_id: request.episode_id.clone(),
        environment: request.environment.clone(),
        repository: request.repository.clone(),
        inputs: request
            .inputs
            .iter()
            .map(|(name, value)| InputBinding {
                name: name.clone(),
                value: redact(value).0,
                provenance: "invocation request".into(),
            })
            .collect(),
        operations,
        verifications,
        dependencies: definition
            .dependencies
            .iter()
            .map(|dependency| {
                format!(
                    "{:?} {} {:?}{}",
                    dependency.kind,
                    dependency.target_id,
                    dependency.constraint,
                    if dependency.required {
                        ""
                    } else {
                        " (optional)"
                    }
                )
            })
            .collect(),
        risk,
        approval,
        unresolved_assumptions: unresolved,
        limits: request.limits,
        plan_digest: String::new(),
    };
    plan.plan_digest = plan.compute_digest();
    Ok(plan)
}
