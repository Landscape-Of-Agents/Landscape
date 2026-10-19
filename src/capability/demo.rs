// The deterministic end-to-end demonstration ('crane capability demo'). It builds on the Work
// Capture demonstration (the failed refund-worker deployment, from fixtures; no credentials, no
// live service), then:
//
//   1. submits two intakes referencing the real Evidence Pack, its digest, and its episode: one
//      under the conservative profile (business objective, criteria, invariant; it stays
//      incomplete because the test report and metrics are missing) and one under the low-risk
//      technical profile (ready); a repeated submission returns the same intake
//   2. stores two candidate definitions from fixtures (src/capability/fixtures), whose "@" fields
//      are bound to the real pack, episode, intake, evidence, and assertion identifiers
//   3. validates the high-risk refund recovery capability: CI evidence cannot verify the business
//      invariant and the unknown test result cannot verify the test claim, so it needs evidence
//      and its publication is denied
//   4. validates and publishes the low-risk diagnostics capability (author and publisher are
//      different people), proposes version 2 (a candidate that inherits no validation), and
//      traces version 1 to its original evidence
//   5. shows that an agent hypothesis cannot verify a claim, and that a changed zone version
//      makes the published validation stale and withdraws it from eligibility

use serde::Serialize;
use serde_json::{json, Value};

use super::intake::{
    BusinessRef, EnterpriseIntent, IntakeRequest, IntakeStatus, INTAKE_SCHEMA_VERSION,
};
use super::lifecycle::{LifecycleState, PublicationRequest};
use super::model::{parse_definition, CapabilityDefinition, ClaimStatus, MappingStatus, RiskLevel};
use super::query::{self, ProvenanceTrace, VersionHistory};
use super::refs::{CurrentVersions, RefOutcome, Sources};
use super::registry::CapabilityRegistry;
use super::validate::{effective_claim, RunOutcome};
use super::{CapabilityError, CapabilityResult, ErrorCode, Grants, Role};
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::correlate::assertions::{hypothesis, Claim};
use crate::capture::correlate::model::{entity_id, EntityKind};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::pack::EvidencePack;
use crate::capture::{demo as capture_demo, AccessPolicy, Actor, ActorKind};

/** The low-risk diagnostics fixture */
const DIAGNOSTICS: &str = include_str!("fixtures/deployment_diagnostics.json");

/** The high-risk refund recovery fixture */
const RECOVERY: &str = include_str!("fixtures/refund_recovery.json");

/** The demonstration's people and agent
 * Fields
    - engineer: Actor - on-call engineer (contributor, validator)
    - lead: Actor - engineering lead (validator, publisher)
    - agent: Actor - the agent that did the work (contributor)
*/
pub(crate) struct Cast {
    pub(crate) engineer: Actor,
    pub(crate) lead: Actor,
    pub(crate) agent: Actor,
}

/** The demonstration's cast
 * Input
    - None
 * Output
    - Cast
*/
pub(crate) fn cast() -> Cast {
    Cast {
        engineer: Actor::new("oncall@acme.example", ActorKind::Human),
        lead: Actor::new("lead@acme.example", ActorKind::Human),
        agent: Actor::new("claude@ses_demo01", ActorKind::Agent),
    }
}

/** The roles of the demonstration
 * Input
    - cast: &Cast - cast
 * Output
    - Grants
*/
pub(crate) fn grants(cast: &Cast) -> Grants {
    Grants::default()
        .with(&cast.engineer.id, &[Role::Contributor, Role::Validator])
        .with(
            &cast.lead.id,
            &[Role::Contributor, Role::Validator, Role::Publisher],
        )
        .with(&cast.agent.id, &[Role::Contributor])
}

/** The current Landscape and Crane versions of the scenario
 * Input
    - None
 * Output
    - CurrentVersions
*/
pub(crate) fn current_versions() -> CurrentVersions {
    CurrentVersions {
        landscape_known: true,
        crane_known: true,
        zones: [("refunds".to_string(), "v3".to_string())]
            .into_iter()
            .collect(),
        flows: [("refund-flow".to_string(), "v2".to_string())]
            .into_iter()
            .collect(),
        policies: [("refund_safety".to_string(), "c41d0e9a3b2f7781".to_string())]
            .into_iter()
            .collect(),
    }
}

/** A Work Capture access policy for an actor, as broad as the scenario's engineer
 * Input
    - actor: &Actor - actor
 * Output
    - CapabilityResult<AccessPolicy>
    - Error if the capture fixture cannot be loaded
*/
pub(crate) fn reader(actor: &Actor) -> CapabilityResult<AccessPolicy> {
    let scenario = capture_demo::scenario().map_err(|error| {
        CapabilityError::new(ErrorCode::MissingSourceReference, error.to_string())
    })?;
    Ok(AccessPolicy {
        actor: actor.clone(),
        ..capture_demo::engineer(&scenario)
    })
}

/** Run the Work Capture investigation the demonstration starts from
 * Input
    - None
 * Output
    - CapabilityResult<(EvidenceQueryBroker, EvidencePack)>
    - Error if the investigation fails
*/
pub(crate) fn captured() -> CapabilityResult<(EvidenceQueryBroker, EvidencePack)> {
    let failed = |error: crate::capture::CaptureError| {
        CapabilityError::new(ErrorCode::MissingSourceReference, error.to_string())
    };
    let scenario = capture_demo::scenario().map_err(failed)?;
    let mut broker = capture_demo::broker(
        &scenario,
        crate::capture::Clock::fixed(capture_demo::DEMO_EPOCH),
    )
    .map_err(failed)?;
    let result = broker
        .investigate(
            capture_demo::request(&scenario),
            &capture_demo::engineer(&scenario),
        )
        .map_err(failed)?;
    Ok((broker, result.pack))
}

/** A business reference
 * Input
    - id: &str - id
    - statement: &str - statement
    - authority: Option<&str> - authority
    - mapping: MappingStatus - mapping
    - confirmed_by: Option<&str> - confirmer
 * Output
    - BusinessRef
*/
fn business(
    id: &str,
    statement: &str,
    authority: Option<&str>,
    mapping: MappingStatus,
    confirmed_by: Option<&str>,
) -> BusinessRef {
    BusinessRef {
        id: id.into(),
        statement: statement.into(),
        authority: authority.map(str::to_string),
        mapping,
        confirmed_by: confirmed_by.map(str::to_string),
    }
}

/** The conservative intake request: full enterprise intent for the refund domain
 * Input
    - pack: &EvidencePack - Evidence Pack
 * Output
    - IntakeRequest
*/
pub(crate) fn refund_intake(pack: &EvidencePack) -> IntakeRequest {
    IntakeRequest {
        schema_version: INTAKE_SCHEMA_VERSION,
        episode_id: pack.episode_id.clone().unwrap_or_default(),
        pack_id: pack.pack_id.clone(),
        pack_digest: pack.manifest.digest.clone(),
        intent: EnterpriseIntent {
            objective: Some(business(
                "obj-idempotent-refunds",
                "Refunds are idempotent",
                Some("payments-spec#refund-idempotency"),
                MappingStatus::ExplicitlyMapped,
                None,
            )),
            process_domain: Some("payments/refunds".into()),
            acceptance_criteria: vec![
                business(
                    "ac-1",
                    "A retried refund request never refunds twice",
                    Some("tracker:PAY-142"),
                    MappingStatus::ExplicitlyMapped,
                    None,
                ),
                business(
                    "ac-2",
                    "Existing refunds keep working after the migration",
                    Some("tracker:PAY-142"),
                    MappingStatus::ExplicitlyMapped,
                    None,
                ),
            ],
            invariants: vec![business(
                "inv-no-double-refund",
                "A retried refund request never refunds twice",
                Some("payments-spec#INV-7"),
                MappingStatus::ExplicitlyMapped,
                None,
            )],
            outcome_measures: vec![business(
                "kpi-refund-errors",
                "Refund error rate stays below 0.1%",
                None,
                MappingStatus::InferredUnverified,
                None,
            )],
            owner: Some("team:payments".into()),
            authoritative_systems: vec!["refunds-ledger".into()],
        },
        risk: RiskLevel::High,
        profile: Some("conservative".into()),
        idempotency_key: None,
        unresolved_assumptions: vec![
            "Queued job keys can be derived from the original request ids".into(),
        ],
        human_interventions: vec!["lead@acme.example approved PAY-142".into()],
    }
}

/** The low-risk intake request: technical acceptance criteria only
 * Input
    - pack: &EvidencePack - Evidence Pack
 * Output
    - IntakeRequest
*/
pub(crate) fn diagnostics_intake(pack: &EvidencePack) -> IntakeRequest {
    IntakeRequest {
        schema_version: INTAKE_SCHEMA_VERSION,
        episode_id: pack.episode_id.clone().unwrap_or_default(),
        pack_id: pack.pack_id.clone(),
        pack_digest: pack.manifest.digest.clone(),
        intent: EnterpriseIntent {
            objective: None,
            process_domain: None,
            acceptance_criteria: vec![business(
                "ac-diagnostics",
                "A failed deployment's outcome, checks, and logs are collected with references",
                None,
                MappingStatus::HumanConfirmed,
                Some("lead@acme.example"),
            )],
            invariants: Vec::new(),
            outcome_measures: Vec::new(),
            owner: Some("team:platform-reliability".into()),
            authoritative_systems: Vec::new(),
        },
        risk: RiskLevel::Low,
        profile: Some("low-risk-technical".into()),
        idempotency_key: None,
        unresolved_assumptions: Vec::new(),
        human_interventions: Vec::new(),
    }
}

/** Bind a fixture's "@" placeholders to real identifiers: @intake, @episode, @pack, @digest,
 * @evidence:TYPE (the pack's first evidence of that type), @assertion:PREDICATE
 * Input
    - fixture: &str - JSON fixture
    - pack: &EvidencePack - Evidence Pack
    - intake_id: &str - intake
 * Output
    - CapabilityResult<CapabilityDefinition>
    - Error when a placeholder cannot be bound or the definition does not parse
*/
pub(crate) fn bind(
    fixture: &str,
    pack: &EvidencePack,
    intake_id: &str,
) -> CapabilityResult<CapabilityDefinition> {
    let mut value: Value = serde_json::from_str(fixture)
        .map_err(|error| CapabilityError::new(ErrorCode::InvalidSchema, error.to_string()))?;
    let mut problem = None;
    bind_value(&mut value, pack, intake_id, &mut problem);
    if let Some(placeholder) = problem {
        return Err(CapabilityError::new(
            ErrorCode::MissingSourceReference,
            format!("fixture placeholder {placeholder} has no matching record"),
        ));
    }
    parse_definition(&value)
}

/** Replace placeholders in a JSON value
 * Input
    - value: &mut Value - value
    - pack: &EvidencePack - Evidence Pack
    - intake_id: &str - intake
    - problem: &mut Option<String> - the first placeholder that could not be bound
 * Output
    - None
*/
fn bind_value(
    value: &mut Value,
    pack: &EvidencePack,
    intake_id: &str,
    problem: &mut Option<String>,
) {
    match value {
        Value::String(text) if text.starts_with('@') => {
            let bound = match text.as_str() {
                "@intake" => Some(intake_id.to_string()),
                "@episode" => pack.episode_id.clone(),
                "@pack" => Some(pack.pack_id.clone()),
                "@digest" => Some(pack.manifest.digest.clone()),
                other => match other.split_once(':') {
                    Some(("@evidence", kind)) => {
                        let kind = serde_json::from_value::<EvidenceType>(json!(kind)).ok();
                        pack.evidence
                            .iter()
                            .find(|item| Some(item.evidence_type) == kind)
                            .map(|item| item.evidence_id.clone())
                    }
                    Some(("@assertion", predicate)) => pack
                        .assertions
                        .iter()
                        .find(|assertion| assertion.claim.predicate == predicate)
                        .map(|assertion| assertion.assertion_id.clone()),
                    _ => None,
                },
            };
            match bound {
                Some(bound) => *text = bound,
                None => *problem = problem.take().or(Some(text.clone())),
            }
        }
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| bind_value(item, pack, intake_id, problem)),
        Value::Object(map) => map
            .values_mut()
            .for_each(|item| bind_value(item, pack, intake_id, problem)),
        _ => {}
    }
}

/** What the demonstration produced
 * Fields
    - pack_id: String - Work Capture Evidence Pack
    - pack_digest: String - its manifest digest
    - intakes: Vec<Value> - intake ids, statuses, and error findings
    - duplicate_submission: bool - the repeated submission returned the existing intake
    - recovery_outcome: RunOutcome - validation outcome of the high-risk capability
    - recovery_findings: Vec<String> - why it needs evidence
    - recovery_publication: String - why its publication was denied
    - diagnostics_outcome: RunOutcome - validation outcome of the low-risk capability
    - hypothesis_claim: (ClaimStatus, Vec<String>) - a claim citing an agent hypothesis
    - trace: ProvenanceTrace - version 1 traced to its evidence
    - history: VersionHistory - diagnostics version history
    - eligible_before_change: Option<u32> - latest eligible version before the zone changed
    - stale: Vec<(String, u32, Vec<String>)> - validations made stale by the zone change
    - eligible_after_change: Option<u32> - latest eligible version after it
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) pack_id: String,
    pub(crate) pack_digest: String,
    pub(crate) intakes: Vec<Value>,
    pub(crate) duplicate_submission: bool,
    pub(crate) recovery_outcome: RunOutcome,
    pub(crate) recovery_findings: Vec<String>,
    pub(crate) recovery_publication: String,
    pub(crate) diagnostics_outcome: RunOutcome,
    pub(crate) hypothesis_claim: (ClaimStatus, Vec<String>),
    pub(crate) trace: ProvenanceTrace,
    pub(crate) history: VersionHistory,
    pub(crate) eligible_before_change: Option<u32>,
    pub(crate) stale: Vec<(String, u32, Vec<String>)>,
    pub(crate) eligible_after_change: Option<u32>,
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CapabilityResult<DemoReport>
    - Error if any stage fails unexpectedly
*/
pub(crate) fn run() -> CapabilityResult<DemoReport> {
    let cast = cast();
    let (mut capture, pack) = captured()?;
    let mut registry = CapabilityRegistry::in_memory(grants(&cast));
    let mut current = current_versions();
    let engineer_reader = reader(&cast.engineer)?;
    let lead_reader = reader(&cast.lead)?;
    let mut clock = 1_760_000_200_000u64;
    let mut tick = || {
        clock += 1;
        clock
    };
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &engineer_reader,
    };
    let refund = registry.submit_intake(&refund_intake(&pack), &sources, &cast.engineer, tick())?;
    let diagnostics =
        registry.submit_intake(&diagnostics_intake(&pack), &sources, &cast.engineer, tick())?;
    let again =
        registry.submit_intake(&diagnostics_intake(&pack), &sources, &cast.engineer, tick())?;
    let intakes = [&refund.intake, &diagnostics.intake]
        .iter()
        .map(|intake| {
            json!({
                "intake_id": intake.intake_id,
                "profile": intake.validation.profile_id,
                "status": intake.status,
                "errors": intake.validation.findings.iter().map(|finding| format!("{}: {}", finding.code, finding.message)).collect::<Vec<_>>(),
                "missing_evidence": intake.completeness.missing.iter().map(|missing| format!("{:?} ({:?})", missing.evidence_type, missing.state)).collect::<Vec<_>>(),
                "unresolved_mappings": intake.completeness.unresolved_mappings,
            })
        })
        .collect::<Vec<_>>();
    if diagnostics.intake.status != IntakeStatus::Ready {
        return Err(CapabilityError::new(
            ErrorCode::IncompleteEvidence,
            "the low-risk intake should be ready",
        ));
    }
    let recovery = bind(RECOVERY, &pack, &refund.intake.intake_id)?;
    registry.create_candidate(&cast.engineer, recovery, Some("conservative"), tick())?;
    let recovery_run = registry.validate(
        &cast.engineer,
        "refund-worker-recovery",
        1,
        &sources,
        false,
        tick(),
    )?;
    let recovery_findings = recovery_run
        .gates
        .iter()
        .flat_map(|gate| gate.findings.iter())
        .filter(|finding| finding.severity == super::validate::Severity::Error)
        .map(|finding| format!("{}: {}", finding.code, finding.message))
        .collect();
    let recovery_publication = match registry.publish(
        &cast.lead,
        &PublicationRequest {
            capability_id: "refund-worker-recovery".into(),
            version: 1,
            reason: "restore refunds".into(),
            acknowledged_warnings: Vec::new(),
        },
        tick(),
    ) {
        Err(error) => error.to_string(),
        Ok(_) => {
            return Err(CapabilityError::new(
                ErrorCode::PublicationDenied,
                "the unverified high-risk capability must not be publishable",
            ))
        }
    };
    let diagnostics_definition = bind(DIAGNOSTICS, &pack, &diagnostics.intake.intake_id)?;
    registry.create_candidate(
        &cast.engineer,
        diagnostics_definition.clone(),
        Some("low-risk-technical"),
        tick(),
    )?;
    let diagnostics_run = registry.validate(
        &cast.engineer,
        "deployment-failure-diagnostics",
        1,
        &sources,
        false,
        tick(),
    )?;
    let lead_sources = Sources {
        reader: &lead_reader,
        ..sources
    };
    registry.publish(
        &cast.lead,
        &PublicationRequest {
            capability_id: "deployment-failure-diagnostics".into(),
            version: 1,
            reason: "triage helper for on-call".into(),
            acknowledged_warnings: diagnostics_run.warning_codes().into_iter().collect(),
        },
        tick(),
    )?;
    let mut next = diagnostics_definition.clone();
    next.version = 2;
    next.previous_version_ref = Some(1);
    next.provenance.change_reason =
        Some("Include runtime metrics once a metrics source is available".into());
    next.applicability
        .limitations
        .retain(|limitation| !limitation.contains("Runtime metrics"));
    registry.propose_version(&cast.engineer, next, Some("low-risk-technical"), tick())?;
    let trace = query::trace(
        &registry.state,
        "deployment-failure-diagnostics",
        1,
        &lead_sources,
    )?;
    if trace
        .references
        .iter()
        .any(|reference| reference.outcome != RefOutcome::Resolved)
    {
        return Err(CapabilityError::new(
            ErrorCode::MissingSourceReference,
            "the published version must trace to its evidence",
        ));
    }
    let deployment = entity_id(EntityKind::Deployment, "dep-7731");
    let guess = hypothesis(
        &cast.agent,
        Claim {
            subject: deployment,
            subject_kind: EntityKind::Deployment,
            subject_key: "dep-7731".into(),
            predicate: "invariant_held".into(),
            object: json!("yes"),
        },
        "No refund was issued twice",
        Vec::new(),
        None,
        tick(),
    );
    let guess_id = capture
        .store
        .add_assertion(guess)
        .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error.to_string()))?;
    let mut hypothetical = diagnostics_definition;
    hypothetical.verification.claims[0].assertion_refs = vec![guess_id];
    hypothetical.verification.claims[0].evidence_refs.clear();
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &engineer_reader,
    };
    let (status, reasons, _) = effective_claim(
        &hypothetical,
        &hypothetical.verification.claims[0],
        &sources,
    );
    let eligible_before_change =
        query::latest_eligible(&registry.state, "deployment-failure-diagnostics")
            .map(|record| record.definition.version);
    current.zones.insert("refunds".into(), "v4".into());
    let sources = Sources {
        capture: &capture.store,
        current: &current,
        reader: &engineer_reader,
    };
    let stale = registry.mark_stale(&cast.engineer, &sources, tick())?;
    let eligible_after_change =
        query::latest_eligible(&registry.state, "deployment-failure-diagnostics")
            .map(|record| record.definition.version);
    debug_assert_eq!(
        registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .map(|record| record.state)
            .ok(),
        Some(LifecycleState::Published)
    );
    Ok(DemoReport {
        pack_id: pack.pack_id.clone(),
        pack_digest: pack.manifest.digest.clone(),
        intakes,
        duplicate_submission: again.duplicate
            && again.intake.intake_id == diagnostics.intake.intake_id,
        recovery_outcome: recovery_run.outcome,
        recovery_findings,
        recovery_publication,
        diagnostics_outcome: diagnostics_run.outcome,
        hypothesis_claim: (status, reasons),
        trace,
        history: query::history(&registry.state, "deployment-failure-diagnostics")?,
        eligible_before_change,
        stale,
        eligible_after_change,
    })
}
