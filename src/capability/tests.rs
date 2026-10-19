// Acceptance tests of the Capability Model & Registry, numbered after the specification's list
// (schema 1-8, intake 9-18, registry 19-27, lifecycle and verification 28-38, integration 39-43;
// 44 and 45 are the binary tests in tests/capability.rs and the full suite). They build on the
// Work Capture demonstration's real Evidence Pack.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::demo::{self, Cast};
use super::intake::{BusinessRef, IntakeRequest, IntakeStatus};
use super::lifecycle::{allowed, LifecycleState, PublicationRequest};
use super::model::{
    parse_definition, CapabilityDefinition, ClaimKind, ClaimStatus, DependencyKind,
    DependencyReference, MappingStatus, VersionConstraint,
};
use super::query::{self, ApplicabilityMatcher, ApplicabilityQuery, ExactMatcher, RegistryQuery};
use super::refs::{CurrentVersions, RefOutcome, Sources};
use super::registry::{migrate, CapabilityRegistry, InvocationRef, InvocationStatus};
use super::validate::{effective_claim, GateId, GateStatus, RunOutcome, Severity, ValidationRun};
use super::{ErrorCode, Grants, Role};
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::correlate::assertions::{hypothesis, Claim};
use crate::capture::correlate::model::{entity_id, EntityKind};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::pack::EvidencePack;
use crate::capture::{AccessPolicy, Actor, ActorKind, Truth};

/** The diagnostics fixture */
const DIAGNOSTICS: &str = include_str!("fixtures/deployment_diagnostics.json");

/** The refund recovery fixture */
const RECOVERY: &str = include_str!("fixtures/refund_recovery.json");

/** A test world: Work Capture with its pack, a registry, current versions, people, and readers
 * Fields
    - capture: EvidenceQueryBroker - Work Capture (its store holds the pack)
    - pack: EvidencePack - the Evidence Pack
    - registry: CapabilityRegistry - in-memory registry
    - current: CurrentVersions - current governance versions
    - cast: Cast - people and agent
    - engineer: AccessPolicy - the engineer's evidence access
    - lead: AccessPolicy - the lead's evidence access
*/
struct World {
    capture: EvidenceQueryBroker,
    pack: EvidencePack,
    registry: CapabilityRegistry,
    current: CurrentVersions,
    cast: Cast,
    engineer: AccessPolicy,
    lead: AccessPolicy,
}

impl World {
    /** Build the world
     * Input
        - None
     * Output
        - World
    */
    fn new() -> Self {
        let cast = demo::cast();
        let (capture, pack) = demo::captured().unwrap();
        Self {
            registry: CapabilityRegistry::in_memory(demo::grants(&cast)),
            current: demo::current_versions(),
            engineer: demo::reader(&cast.engineer).unwrap(),
            lead: demo::reader(&cast.lead).unwrap(),
            capture,
            pack,
            cast,
        }
    }

    /** Sources read by the engineer
     * Input
        - None (uses self)
     * Output
        - Sources
    */
    fn sources(&self) -> Sources<'_> {
        Sources {
            capture: &self.capture.store,
            current: &self.current,
            reader: &self.engineer,
        }
    }

    /** Submit a request as the engineer
     * Input
        - request: &IntakeRequest - request
     * Output
        - super::CapabilityResult<super::registry::IntakeSubmission>
    */
    fn submit(
        &mut self,
        request: &IntakeRequest,
    ) -> super::CapabilityResult<super::registry::IntakeSubmission> {
        let sources = Sources {
            capture: &self.capture.store,
            current: &self.current,
            reader: &self.engineer,
        };
        self.registry
            .submit_intake(request, &sources, &self.cast.engineer, 10)
    }

    /** Submit the ready low-risk intake and bind the diagnostics definition to it
     * Input
        - None (uses self)
     * Output
        - CapabilityDefinition
    */
    fn diagnostics(&mut self) -> CapabilityDefinition {
        let intake = self
            .submit(&demo::diagnostics_intake(&self.pack))
            .unwrap()
            .intake;
        assert_eq!(
            intake.status,
            IntakeStatus::Ready,
            "{:?}",
            intake.validation.findings
        );
        demo::bind(DIAGNOSTICS, &self.pack, &intake.intake_id).unwrap()
    }

    /** Submit the conservative intake and bind the refund recovery definition to it
     * Input
        - None (uses self)
     * Output
        - CapabilityDefinition
    */
    fn recovery(&mut self) -> CapabilityDefinition {
        let intake = self
            .submit(&demo::refund_intake(&self.pack))
            .unwrap()
            .intake;
        demo::bind(RECOVERY, &self.pack, &intake.intake_id).unwrap()
    }

    /** Validate a stored version as the engineer
     * Input
        - id: &str - capability
        - version: u32 - version
     * Output
        - ValidationRun
    */
    fn validate(&mut self, id: &str, version: u32) -> ValidationRun {
        let sources = Sources {
            capture: &self.capture.store,
            current: &self.current,
            reader: &self.engineer,
        };
        self.registry
            .validate(&self.cast.engineer, id, version, &sources, false, 20)
            .unwrap()
    }

    /** Store, validate, and publish the diagnostics capability (author engineer, publisher lead)
     * Input
        - None (uses self)
     * Output
        - CapabilityDefinition the published definition
    */
    fn published(&mut self) -> CapabilityDefinition {
        let definition = self.diagnostics();
        self.registry
            .create_candidate(
                &self.cast.engineer,
                definition.clone(),
                Some("low-risk-technical"),
                11,
            )
            .unwrap();
        let run = self.validate("deployment-failure-diagnostics", 1);
        assert_eq!(run.outcome, RunOutcome::Passed, "{:#?}", run.gates);
        self.registry
            .publish(
                &self.cast.lead,
                &publication("deployment-failure-diagnostics", 1),
                30,
            )
            .unwrap();
        definition
    }
}

/** A publication request without acknowledged warnings
 * Input
    - id: &str - capability
    - version: u32 - version
 * Output
    - PublicationRequest
*/
fn publication(id: &str, version: u32) -> PublicationRequest {
    PublicationRequest {
        capability_id: id.into(),
        version,
        reason: "test".into(),
        acknowledged_warnings: Vec::new(),
    }
}

/** The error findings of a run
 * Input
    - run: &ValidationRun - run
 * Output
    - BTreeSet<String> codes
*/
fn errors(run: &ValidationRun) -> BTreeSet<String> {
    run.gates
        .iter()
        .flat_map(|gate| gate.findings.iter())
        .filter(|finding| finding.severity == Severity::Error)
        .map(|finding| finding.code.clone())
        .collect()
}

/** A dry run of a definition with the low-risk profile
 * Input
    - world: &World - world
    - definition: &CapabilityDefinition - definition
 * Output
    - ValidationRun
*/
fn dry(world: &World, definition: &CapabilityDefinition) -> ValidationRun {
    let profile =
        super::validate::select_profile(Some("low-risk-technical"), definition.applicability.risk)
            .unwrap();
    super::validate::run(
        definition,
        &profile,
        &world.registry.state,
        &world.sources(),
        &world.cast.engineer,
        true,
        5,
    )
}

/** 1. A valid definition is stored as a candidate with its digest
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn creates_valid_capabilities() {
    let mut world = World::new();
    let definition = world.diagnostics();
    let record = world
        .registry
        .create_candidate(
            &world.cast.engineer,
            definition.clone(),
            Some("low-risk-technical"),
            11,
        )
        .unwrap();
    assert_eq!(
        (record.state, record.definition_digest.clone()),
        (LifecycleState::Candidate, definition.digest())
    );
    assert_eq!(errors(&dry(&world, &definition)), BTreeSet::new());
}

/** 2. Missing required fields and invalid types are rejected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_missing_fields_and_invalid_types() {
    let mut world = World::new();
    let definition = world.diagnostics();
    let mut value = json!(definition);
    value.as_object_mut().unwrap().remove("name");
    assert_eq!(
        parse_definition(&value).unwrap_err().code,
        ErrorCode::InvalidSchema
    );
    let mut value = json!(definition);
    value["version"] = json!("one");
    assert_eq!(
        parse_definition(&value).unwrap_err().code,
        ErrorCode::InvalidSchema
    );
    let mut value = json!(definition);
    value["unexpected"] = json!(true);
    assert_eq!(
        parse_definition(&value).unwrap_err().code,
        ErrorCode::InvalidSchema
    );
    let mut empty = definition;
    empty.name = " ".into();
    assert!(errors(&dry(&world, &empty)).contains("missing_field"));
}

/** 3. Unsupported schema versions are refused, never reinterpreted
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn refuses_unsupported_schema_versions() {
    let mut world = World::new();
    let mut value = json!(world.diagnostics());
    value["schema_version"] = json!(2);
    assert_eq!(
        parse_definition(&value).unwrap_err().code,
        ErrorCode::UnsupportedSchemaVersion
    );
    let mut request = demo::diagnostics_intake(&world.pack);
    request.schema_version = 7;
    assert_eq!(
        world.submit(&request).unwrap_err().code,
        ErrorCode::UnsupportedSchemaVersion
    );
    assert_eq!(
        migrate(json!({"schema_version": 9})).unwrap_err().code,
        ErrorCode::UnsupportedSchemaVersion
    );
}

/** 4. Unknown enumeration values are rejected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_unknown_enumeration_values() {
    let mut world = World::new();
    let definition = world.diagnostics();
    for (path, bad) in [
        (&["applicability", "risk"][..], "extreme"),
        (&["provenance", "creation_method"][..], "telepathy"),
        (&["verification", "claims", "0", "status"][..], "probably"),
    ] {
        let mut value = json!(definition);
        let mut node = &mut value;
        for part in path {
            node = match part.parse::<usize>() {
                Ok(index) => &mut node[index],
                Err(_) => &mut node[*part],
            };
        }
        *node = json!(bad);
        assert_eq!(
            parse_definition(&value).unwrap_err().code,
            ErrorCode::InvalidSchema,
            "{bad}"
        );
    }
}

/** 5. An invalid procedure fails structural validation and stays a candidate
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_invalid_procedures() {
    let mut world = World::new();
    let mut definition = world.diagnostics();
    definition.execution.steps[1].step_id = definition.execution.steps[0].step_id.clone();
    definition.execution.steps[2]
        .inputs
        .push("undeclared_input".into());
    definition.execution.steps[3]
        .verifies
        .push("no-such-claim".into());
    definition
        .execution
        .checkpoints
        .push("missing-checkpoint".into());
    let run = dry(&world, &definition);
    assert_eq!(run.outcome, RunOutcome::StructurallyInvalid);
    let structural = run
        .gates
        .iter()
        .find(|gate| gate.gate == GateId::Structural)
        .unwrap();
    assert_eq!(structural.status, GateStatus::Failed);
    assert!(structural.findings.len() >= 4);
    world
        .registry
        .create_candidate(
            &world.cast.engineer,
            definition,
            Some("low-risk-technical"),
            11,
        )
        .unwrap();
    world.validate("deployment-failure-diagnostics", 1);
    assert_eq!(
        world
            .registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .state,
        LifecycleState::Candidate
    );
}

/** 6. Invalid dependency references are rejected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_invalid_dependency_references() {
    let mut world = World::new();
    let base = world.diagnostics();
    let dependency = |kind, target: &str, constraint| DependencyReference {
        kind,
        target_id: target.into(),
        required: true,
        constraint,
        reason: "test".into(),
        applicability: Vec::new(),
    };
    for (extra, code) in [
        (
            dependency(DependencyKind::Service, "*", VersionConstraint::Unpinned),
            "invalid_dependency",
        ),
        (
            dependency(
                DependencyKind::Capability,
                "deployment-failure-diagnostics",
                VersionConstraint::AtLeast(1),
            ),
            "invalid_dependency",
        ),
        (
            dependency(
                DependencyKind::Zone,
                "refunds",
                VersionConstraint::Exact("v3".into()),
            ),
            "invalid_dependency",
        ),
        (
            dependency(
                DependencyKind::Flow,
                "payments-flow",
                VersionConstraint::Unpinned,
            ),
            "unpinned_dependency",
        ),
    ] {
        let mut definition = base.clone();
        definition.dependencies.push(extra);
        assert!(errors(&dry(&world, &definition)).contains(code), "{code}");
    }
}

/** 7. Missing or universal applicability is rejected; unverified environments are not claimed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_missing_applicability() {
    let mut world = World::new();
    let base = world.diagnostics();
    let mut empty = base.clone();
    empty.applicability.environments.clear();
    assert!(errors(&dry(&world, &empty)).contains("missing_applicability"));
    let mut universal = base.clone();
    universal.applicability.task_classes = vec!["any".into()];
    assert!(errors(&dry(&world, &universal)).contains("universal_applicability"));
    let mut beyond = base;
    beyond.applicability.environments.push("staging".into());
    assert!(errors(&dry(&world, &beyond)).contains("applicability_beyond_verification"));
}

/** 8. Invalid verification claims are rejected or left unresolved
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_invalid_verification_claims() {
    let mut world = World::new();
    let base = world.diagnostics();
    let mut orphan = base.clone();
    orphan.verification.claims[0].kind = ClaimKind::BusinessInvariant;
    assert!(errors(&dry(&world, &orphan)).contains("invalid_claim"));
    let mut bare = base;
    bare.verification.claims[1].method = None;
    bare.verification.claims[1].evidence_refs.clear();
    let run = dry(&world, &bare);
    assert!(errors(&run).contains("unverified_claim"));
    let (status, reasons, _) =
        effective_claim(&bare, &bare.verification.claims[1], &world.sources());
    assert_eq!(status, ClaimStatus::Unresolved);
    assert!(reasons
        .iter()
        .any(|reason| reason.contains("no verification method")));
}

/** 9. A Work Capture intake is validated and ready under its profile
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn accepts_valid_work_capture_intakes() {
    let mut world = World::new();
    let intake = world
        .submit(&demo::diagnostics_intake(&world.pack))
        .unwrap()
        .intake;
    assert_eq!(intake.status, IntakeStatus::Ready);
    assert!(intake.validation.digest_verified);
    assert_eq!(intake.work.pack_digest, world.pack.manifest.digest);
    assert_eq!(intake.work.task_id.as_deref(), Some("PAY-142"));
    assert!(
        intake
            .validation
            .references
            .iter()
            .all(|reference| reference.outcome == RefOutcome::Resolved),
        "{:#?}",
        intake.validation.references
    );
    assert!(!intake.work.snapshots.is_empty());
    assert_eq!(
        intake.completeness.missing.len(),
        2,
        "missing evidence is preserved even when the profile tolerates it"
    );
}

/** 10. A missing or inaccessible episode or pack is an error, never inferred
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_missing_episodes_and_packs() {
    let mut world = World::new();
    let mut request = demo::diagnostics_intake(&world.pack);
    request.pack_id = "pck_missing".into();
    assert_eq!(
        world.submit(&request).unwrap_err().code,
        ErrorCode::MissingSourceReference
    );
    let mut request = demo::diagnostics_intake(&world.pack);
    request.episode_id = "epi_missing".into();
    assert_eq!(
        world.submit(&request).unwrap_err().code,
        ErrorCode::MissingSourceReference
    );
    assert!(world.registry.state.intakes.is_empty());
}

/** 11. A digest mismatch, or pack content changed after sealing, is an integrity error
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn detects_digest_mismatches() {
    let mut world = World::new();
    let mut request = demo::diagnostics_intake(&world.pack);
    request.pack_digest = "0".repeat(32);
    assert_eq!(
        world.submit(&request).unwrap_err().code,
        ErrorCode::IntegrityMismatch
    );
    let pack_id = world.pack.pack_id.clone();
    world
        .capture
        .store
        .packs
        .get_mut(&pack_id)
        .unwrap()
        .evidence[0]
        .excerpt = json!("tampered");
    let error = world
        .submit(&demo::diagnostics_intake(&world.pack))
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::IntegrityMismatch);
    assert!(error.message.contains("does not match"));
}

/** 12. Repeating the same submission returns the existing intake
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn submissions_are_idempotent() {
    let mut world = World::new();
    let first = world
        .submit(&demo::diagnostics_intake(&world.pack))
        .unwrap();
    let second = world
        .submit(&demo::diagnostics_intake(&world.pack))
        .unwrap();
    assert!(!first.duplicate && second.duplicate);
    assert_eq!(first.intake.intake_id, second.intake.intake_id);
    assert_eq!(world.registry.state.intakes.len(), 1);
}

/** 13. Reusing an idempotency key with different content is a conflict
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_idempotency_key_reuse() {
    let mut world = World::new();
    let mut request = demo::diagnostics_intake(&world.pack);
    request.idempotency_key = Some("key-1".into());
    world.submit(&request).unwrap();
    request.intent.owner = Some("team:other".into());
    assert_eq!(
        world.submit(&request).unwrap_err().code,
        ErrorCode::IdempotencyConflict
    );
}

/** 14. Inferred or unsupported business mappings never become authoritative
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn flags_incomplete_business_mappings() {
    let mut world = World::new();
    let mut request = demo::refund_intake(&world.pack);
    let objective = request.intent.objective.as_mut().unwrap();
    objective.mapping = MappingStatus::InferredUnverified;
    request.intent.acceptance_criteria[0] = BusinessRef {
        authority: None,
        ..request.intent.acceptance_criteria[0].clone()
    };
    let intake = world.submit(&request).unwrap().intake;
    let codes = intake
        .validation
        .findings
        .iter()
        .map(|finding| finding.code.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        codes.contains("unresolved_business_mapping")
            && codes.contains("unsupported_mapping_claim"),
        "{codes:?}"
    );
    assert!(intake
        .completeness
        .unresolved_mappings
        .contains(&"obj-idempotent-refunds".to_string()));
    assert!(intake
        .completeness
        .unresolved_mappings
        .contains(&"ac-1".to_string()));
    assert_eq!(
        intake.intent.objective.unwrap().mapping,
        MappingStatus::InferredUnverified,
        "the submitted intent is preserved, not upgraded"
    );
    assert_eq!(intake.status, IntakeStatus::Incomplete);
}

/** 15. Missing business invariants block an intake under a profile that requires them
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn requires_invariants_when_the_profile_does() {
    let mut world = World::new();
    let mut request = demo::refund_intake(&world.pack);
    request.intent.invariants.clear();
    let intake = world.submit(&request).unwrap().intake;
    assert!(intake
        .validation
        .findings
        .iter()
        .any(|finding| finding.code == "missing_business_invariants"));
    let mut low = demo::diagnostics_intake(&world.pack);
    low.risk = super::model::RiskLevel::High;
    assert_eq!(
        world.submit(&low).unwrap_err().code,
        ErrorCode::InvalidSchema,
        "a weak profile cannot govern high risk"
    );
}

/** 16. Contradictory source assertions are surfaced, not resolved silently
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn surfaces_contradictory_assertions() {
    let mut scenario = crate::capture::demo::scenario().unwrap();
    let deploy = scenario
        .resources
        .iter_mut()
        .find(|resource| resource.resource_id == "deploy")
        .unwrap();
    let mut canary = deploy.records[1].clone();
    canary.record_id = "check-canary".into();
    canary.content = json!({"deployment": "dep-7731", "check": "canary", "healthy": true, "environment": "production"});
    deploy.records.push(canary);
    let mut capture = crate::capture::demo::broker(
        &scenario,
        crate::capture::Clock::fixed(crate::capture::demo::DEMO_EPOCH),
    )
    .unwrap();
    let pack = capture
        .investigate(
            crate::capture::demo::request(&scenario),
            &crate::capture::demo::engineer(&scenario),
        )
        .unwrap()
        .pack;
    let cast = demo::cast();
    let mut registry = CapabilityRegistry::in_memory(demo::grants(&cast));
    let reader = demo::reader(&cast.engineer).unwrap();
    let current = demo::current_versions();
    let intake = registry
        .submit_intake(
            &demo::diagnostics_intake(&pack),
            &Sources {
                capture: &capture.store,
                current: &current,
                reader: &reader,
            },
            &cast.engineer,
            1,
        )
        .unwrap()
        .intake;
    assert_eq!(intake.completeness.conflicting_assertions.len(), 1);
    assert_eq!(
        intake.observed.contradicted_claims,
        intake.completeness.conflicting_assertions
    );
    assert!(intake
        .validation
        .findings
        .iter()
        .any(|finding| finding.code == "conflicting_assertions"));
    assert_eq!(intake.status, IntakeStatus::Incomplete);
}

/** 17. Stale or unverifiable zone, flow, and policy versions are recorded
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn records_stale_revisions() {
    let mut world = World::new();
    world.current.zones.insert("refunds".into(), "v4".into());
    world
        .current
        .policies
        .insert("refund_safety".into(), "changed".into());
    let intake = world
        .submit(&demo::diagnostics_intake(&world.pack))
        .unwrap()
        .intake;
    let stale = intake
        .completeness
        .stale_revisions
        .iter()
        .map(|check| (check.kind.as_str(), check.outcome))
        .collect::<Vec<_>>();
    assert!(
        stale.contains(&("zone", RefOutcome::Stale))
            && stale.contains(&("policy", RefOutcome::Stale)),
        "{stale:?}"
    );
    assert_eq!(intake.status, IntakeStatus::Incomplete);
    world.current.crane_known = false;
    let mut request = demo::diagnostics_intake(&world.pack);
    request.idempotency_key = Some("unknown-crane".into());
    let unknown = world.submit(&request).unwrap().intake;
    assert!(unknown
        .completeness
        .stale_revisions
        .iter()
        .any(|check| check.outcome == RefOutcome::Unverifiable));
}

/** 18. Unauthorized submitters and unreadable references are refused or recorded
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn enforces_intake_authorization() {
    let mut world = World::new();
    let stranger = Actor::new("stranger@acme.example", ActorKind::Human);
    let reader = AccessPolicy {
        actor: stranger.clone(),
        ..world.engineer.clone()
    };
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &reader,
    };
    assert_eq!(
        world
            .registry
            .submit_intake(
                &demo::diagnostics_intake(&world.pack),
                &sources,
                &stranger,
                1
            )
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &world.lead,
    };
    assert_eq!(
        world
            .registry
            .submit_intake(
                &demo::diagnostics_intake(&world.pack),
                &sources,
                &world.cast.engineer,
                1
            )
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized,
        "the access policy must be the submitter's own"
    );
    let mut narrow = world.engineer.clone();
    narrow.resources.remove("logs");
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &narrow,
    };
    let intake = world
        .registry
        .submit_intake(
            &demo::diagnostics_intake(&world.pack),
            &sources,
            &world.cast.engineer,
            1,
        )
        .unwrap()
        .intake;
    assert!(intake
        .completeness
        .unauthorized_sources
        .iter()
        .any(|source| source.starts_with("logs:")));
    assert!(intake
        .validation
        .references
        .iter()
        .any(|reference| reference.outcome == RefOutcome::Unauthorized));
    assert!(
        intake
            .observed
            .runtime
            .iter()
            .all(|reference| reference.evidence_type != EvidenceType::RuntimeLog),
        "unreadable evidence is not referenced as observed"
    );
    let mut elsewhere = world.engineer.clone();
    elsewhere.repositories = BTreeSet::from(["acme/ledger".to_string()]);
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &elsewhere,
    };
    let mut request = demo::diagnostics_intake(&world.pack);
    request.idempotency_key = Some("other-repo".into());
    assert_eq!(
        world
            .registry
            .submit_intake(&request, &sources, &world.cast.engineer, 1)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );
}

/** 19. Create, inspect, list, and retrieve exact versions
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn creates_lists_and_retrieves_versions() {
    let mut world = World::new();
    let diagnostics = world.diagnostics();
    let recovery = world.recovery();
    world
        .registry
        .create_candidate(
            &world.cast.engineer,
            diagnostics.clone(),
            Some("low-risk-technical"),
            1,
        )
        .unwrap();
    world
        .registry
        .create_candidate(&world.cast.engineer, recovery, None, 2)
        .unwrap();
    let state = &world.registry.state;
    assert_eq!(
        state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .definition,
        diagnostics
    );
    assert_eq!(
        state
            .record("deployment-failure-diagnostics", 9)
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(query::list(state, &RegistryQuery::default()).len(), 2);
    let triage = query::list(
        state,
        &RegistryQuery {
            task_class: Some("deployment-failure-triage".into()),
            ..RegistryQuery::default()
        },
    );
    assert_eq!(triage.len(), 1);
    assert_eq!(triage[0].latest_eligible, None);
    assert_eq!(
        query::list(
            state,
            &RegistryQuery {
                environment: Some("staging".into()),
                ..RegistryQuery::default()
            }
        )
        .len(),
        0
    );
    assert_eq!(
        state
            .record("refund-worker-recovery", 1)
            .unwrap()
            .profile_id,
        "conservative"
    );
}

/** 20. Stored versions are immutable
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn versions_are_immutable() {
    let mut world = World::new();
    let definition = world.diagnostics();
    world
        .registry
        .create_candidate(
            &world.cast.engineer,
            definition.clone(),
            Some("low-risk-technical"),
            1,
        )
        .unwrap();
    let mut changed = definition.clone();
    changed.description = "rewritten".into();
    assert_eq!(
        world
            .registry
            .create_candidate(
                &world.cast.engineer,
                changed.clone(),
                Some("low-risk-technical"),
                2
            )
            .unwrap_err()
            .code,
        ErrorCode::IdempotencyConflict
    );
    assert_eq!(
        world
            .registry
            .propose_version(&world.cast.engineer, changed, Some("low-risk-technical"), 2)
            .unwrap_err()
            .code,
        ErrorCode::InvalidSchema
    );
    assert_eq!(
        world
            .registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .definition,
        definition
    );
    let mut forged = definition;
    forged.capability_id = "forged".into();
    forged.provenance.created_by = world.cast.lead.clone();
    assert_eq!(
        world
            .registry
            .create_candidate(&world.cast.engineer, forged, Some("low-risk-technical"), 3)
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized,
        "authorship comes from the caller, not the metadata"
    );
    let mut document = json!(world.registry.state);
    document["versions"]["deployment-failure-diagnostics"]["1"]["definition"]["description"] =
        json!("edited on disk");
    assert_eq!(
        migrate(document).unwrap_err().code,
        ErrorCode::IntegrityMismatch
    );
}

/** 21. A new version links to its predecessor; publishing it supersedes the older published one
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn links_new_versions_to_predecessors() {
    let mut world = World::new();
    let definition = world.published();
    let mut next = definition;
    next.version = 2;
    next.previous_version_ref = Some(1);
    assert_eq!(
        world
            .registry
            .propose_version(
                &world.cast.engineer,
                next.clone(),
                Some("low-risk-technical"),
                40
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidSchema,
        "a change reason is required"
    );
    next.provenance.change_reason = Some("clarify the bundle".into());
    next.description = "Read-only collection with clearer bundle references.".into();
    world
        .registry
        .propose_version(&world.cast.engineer, next, Some("low-risk-technical"), 40)
        .unwrap();
    let history = query::history(&world.registry.state, "deployment-failure-diagnostics").unwrap();
    assert_eq!(history.versions[1].1, Some(1));
    assert_eq!(
        world.validate("deployment-failure-diagnostics", 2).outcome,
        RunOutcome::Passed
    );
    world
        .registry
        .publish(
            &world.cast.lead,
            &publication("deployment-failure-diagnostics", 2),
            50,
        )
        .unwrap();
    assert_eq!(
        world
            .registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .superseded_by,
        Some(2)
    );
    assert_eq!(
        query::latest_eligible(&world.registry.state, "deployment-failure-diagnostics")
            .unwrap()
            .definition
            .version,
        2
    );
}

/** 22. The registry survives a restart with everything it held
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn persists_across_restarts() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = World::new();
    world.registry = CapabilityRegistry::open(directory.path(), demo::grants(&world.cast)).unwrap();
    world.published();
    let reopened = CapabilityRegistry::open(directory.path(), demo::grants(&world.cast)).unwrap();
    assert_eq!(reopened.state, world.registry.state);
    assert_eq!(
        query::latest_eligible(&reopened.state, "deployment-failure-diagnostics")
            .unwrap()
            .definition
            .version,
        1
    );
}

/** 23. Concurrent writers lose nothing; interrupted and failed writes leave the last committed
 * document intact; a corrupted document fails loudly
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn survives_concurrent_and_interrupted_writes() {
    let directory = tempfile::tempdir().unwrap();
    let mut world = World::new();
    let base = world.diagnostics();
    let path = directory.path().to_path_buf();
    let workers = (0..2)
        .map(|worker| {
            let path = path.clone();
            let base = base.clone();
            let engineer = world.cast.engineer.clone();
            let grants = demo::grants(&world.cast);
            std::thread::spawn(move || {
                let mut registry = CapabilityRegistry::open(&path, grants).unwrap();
                for index in 0..5 {
                    let mut definition = base.clone();
                    definition.capability_id = format!("worker-{worker}-{index}");
                    registry
                        .create_candidate(&engineer, definition, Some("low-risk-technical"), 1)
                        .unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().unwrap();
    }
    let mut registry = CapabilityRegistry::open(&path, demo::grants(&world.cast)).unwrap();
    assert_eq!(registry.state.versions.len(), 10);
    assert_eq!(registry.state.generation, 10);
    std::fs::write(
        path.join(".registry.json.crane-tmp-99999"),
        "{ half written",
    )
    .unwrap();
    let generation = CapabilityRegistry::open(&path, demo::grants(&world.cast))
        .unwrap()
        .state
        .generation;
    assert_eq!(
        generation, 10,
        "a leftover temporary file from an interrupted write is ignored"
    );
    assert!(registry
        .create_candidate(
            &world.cast.engineer,
            base.clone(),
            Some("unknown-profile"),
            2
        )
        .is_err());
    assert_eq!(
        CapabilityRegistry::open(&path, demo::grants(&world.cast))
            .unwrap()
            .state
            .generation,
        10,
        "a failed operation writes nothing"
    );
    std::fs::write(
        path.join("registry.json"),
        "{\"schema_version\": 1, \"gener",
    )
    .unwrap();
    let error = CapabilityRegistry::open(&path, demo::grants(&world.cast)).unwrap_err();
    assert_eq!(error.code, ErrorCode::Persistence);
    assert!(error.message.contains("restore it from a backup"));
}

/** 24. An older layout is migrated without losing data; the strictest profile is assumed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn migrates_older_layouts() {
    let mut world = World::new();
    let definition = world.diagnostics();
    world
        .registry
        .create_candidate(
            &world.cast.engineer,
            definition,
            Some("low-risk-technical"),
            1,
        )
        .unwrap();
    let mut legacy = json!(world.registry.state);
    let object = legacy.as_object_mut().unwrap();
    object.remove("schema_version");
    object.remove("generation");
    object.remove("denials");
    let record = &mut object["versions"]["deployment-failure-diagnostics"]["1"];
    for field in [
        "profile_id",
        "stale",
        "superseded_by",
        "withdrawals",
        "invocations",
    ] {
        record.as_object_mut().unwrap().remove(field);
    }
    let migrated = migrate(legacy).unwrap();
    assert_eq!(
        (migrated.schema_version, migrated.migrated_from),
        (1, Some(0))
    );
    let record = migrated
        .record("deployment-failure-diagnostics", 1)
        .unwrap();
    assert_eq!(record.profile_id, "conservative");
    assert_eq!(
        record.definition,
        world
            .registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .definition
    );
    assert_eq!(migrated.intakes, world.registry.state.intakes);
}

/** 25. Dependencies are inspected, and a missing required dependency blocks validation
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn detects_missing_dependencies() {
    let mut world = World::new();
    let mut definition = world.diagnostics();
    definition.dependencies.push(DependencyReference {
        kind: DependencyKind::Capability,
        target_id: "log-fetcher".into(),
        required: true,
        constraint: VersionConstraint::AtLeast(2),
        reason: "fetches logs".into(),
        applicability: Vec::new(),
    });
    definition.dependencies.push(DependencyReference {
        kind: DependencyKind::Zone,
        target_id: "ledger".into(),
        required: false,
        constraint: VersionConstraint::Exact("v1".into()),
        reason: "sometimes involved".into(),
        applicability: Vec::new(),
    });
    world
        .registry
        .create_candidate(
            &world.cast.engineer,
            definition,
            Some("low-risk-technical"),
            1,
        )
        .unwrap();
    let statuses = query::dependencies(
        &world.registry.state,
        "deployment-failure-diagnostics",
        1,
        &world.sources(),
    )
    .unwrap();
    let outcome = |target: &str| {
        statuses
            .iter()
            .find(|status| status.dependency.target_id == target)
            .unwrap()
            .outcome
    };
    assert_eq!(
        (
            outcome("refunds"),
            outcome("log-fetcher"),
            outcome("ledger")
        ),
        (
            RefOutcome::Resolved,
            RefOutcome::Missing,
            RefOutcome::Missing
        )
    );
    let run = world.validate("deployment-failure-diagnostics", 1);
    assert_eq!(run.outcome, RunOutcome::NeedsEvidence);
    let applicability = run
        .gates
        .iter()
        .find(|gate| gate.gate == GateId::Applicability)
        .unwrap();
    assert!(
        applicability
            .findings
            .iter()
            .any(|finding| finding.code == "missing_dependency"
                && finding.severity == Severity::Error)
    );
    assert!(applicability
        .findings
        .iter()
        .any(|finding| finding.code == "missing_dependency"
            && finding.severity == Severity::Warning
            && finding.message.starts_with("optional")));
}

/** 26. Required capability dependencies must be acyclic
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_dependency_cycles() {
    let mut world = World::new();
    let base = world.diagnostics();
    let depend_on = |target: &str| DependencyReference {
        kind: DependencyKind::Capability,
        target_id: target.into(),
        required: true,
        constraint: VersionConstraint::AtLeast(1),
        reason: "needs it".into(),
        applicability: Vec::new(),
    };
    let mut first = base.clone();
    first.capability_id = "alpha".into();
    first.dependencies.push(depend_on("beta"));
    let mut second = base;
    second.capability_id = "beta".into();
    second.dependencies.push(depend_on("alpha"));
    world
        .registry
        .create_candidate(&world.cast.engineer, first, Some("low-risk-technical"), 1)
        .unwrap();
    let run = dry(&world, &second);
    let cycle = run
        .gates
        .iter()
        .flat_map(|gate| gate.findings.iter())
        .find(|finding| finding.code == "dependency_cycle")
        .expect("cycle detected");
    assert_eq!(cycle.evidence_refs, vec!["beta", "alpha", "beta"]);
    let mut optional = second.clone();
    optional.dependencies.last_mut().unwrap().required = false;
    assert!(
        !errors(&dry(&world, &optional)).contains("dependency_cycle"),
        "optional references are not resolved recursively"
    );
}

/** 27. A changed reference marks the validation stale without rewriting it, and blocks use
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn detects_stale_validation() {
    let mut world = World::new();
    world.published();
    let run_before = world.registry.state.runs.values().next().unwrap().clone();
    world
        .current
        .flows
        .insert("refund-flow".into(), "v3".into());
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &world.engineer,
    };
    let marked = world
        .registry
        .mark_stale(&world.cast.engineer, &sources, 60)
        .unwrap();
    assert_eq!(marked.len(), 1);
    assert!(marked[0].2[0].contains("flow refund-flow@v2"));
    assert_eq!(
        world.registry.state.runs[&run_before.run_id], run_before,
        "the run is never rewritten"
    );
    assert!(
        query::latest_eligible(&world.registry.state, "deployment-failure-diagnostics").is_none()
    );
    assert!(
        world
            .registry
            .mark_stale(&world.cast.engineer, &sources, 61)
            .unwrap()
            .is_empty(),
        "marking is idempotent"
    );
    world
        .current
        .flows
        .insert("refund-flow".into(), "v2".into());
    let revalidated = world.validate("deployment-failure-diagnostics", 1);
    assert_eq!(revalidated.outcome, RunOutcome::Passed);
    assert_eq!(
        world
            .registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .state,
        LifecycleState::Published
    );
    assert!(
        query::latest_eligible(&world.registry.state, "deployment-failure-diagnostics").is_some()
    );
}

/** 28. The permitted forward path works and every step is audited
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn follows_valid_transitions() {
    let mut world = World::new();
    world.published();
    let path = world
        .registry
        .state
        .transitions
        .iter()
        .map(|transition| transition.to)
        .collect::<Vec<_>>();
    assert_eq!(
        path,
        vec![
            LifecycleState::Candidate,
            LifecycleState::Validating,
            LifecycleState::Validated,
            LifecycleState::Published
        ]
    );
    assert!(
        allowed(LifecycleState::Candidate, LifecycleState::NeedsEvidence)
            && allowed(LifecycleState::NeedsEvidence, LifecycleState::Validating)
    );
    assert!(world
        .registry
        .state
        .transitions
        .iter()
        .all(|transition| !transition.reason.is_empty()));
}

/** 29. Transitions outside the table are rejected
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn rejects_invalid_transitions() {
    for (from, to) in [
        (LifecycleState::Candidate, LifecycleState::Published),
        (LifecycleState::NeedsEvidence, LifecycleState::Validated),
        (LifecycleState::Published, LifecycleState::Validating),
        (LifecycleState::Revoked, LifecycleState::Published),
        (LifecycleState::Rejected, LifecycleState::Candidate),
    ] {
        assert!(!allowed(from, to), "{from:?} -> {to:?}");
    }
    let mut world = World::new();
    world.published();
    assert_eq!(
        world
            .registry
            .withdraw(
                &world.cast.lead,
                "deployment-failure-diagnostics",
                Some(1),
                LifecycleState::Rejected,
                "late",
                70
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
    world
        .registry
        .withdraw(
            &world.cast.lead,
            "deployment-failure-diagnostics",
            Some(1),
            LifecycleState::Revoked,
            "withdrawn",
            71,
        )
        .unwrap();
    let sources = Sources {
        capture: &world.capture.store,
        current: &world.current,
        reader: &world.engineer,
    };
    assert_eq!(
        world
            .registry
            .validate(
                &world.cast.engineer,
                "deployment-failure-diagnostics",
                1,
                &sources,
                false,
                72
            )
            .unwrap_err()
            .code,
        ErrorCode::InvalidTransition
    );
}

/** 30. Publication is denied, and audited, when required gates fail
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn blocks_publication_when_gates_fail() {
    let mut world = World::new();
    let recovery = world.recovery();
    world
        .registry
        .create_candidate(&world.cast.engineer, recovery, None, 1)
        .unwrap();
    assert_eq!(
        world.validate("refund-worker-recovery", 1).outcome,
        RunOutcome::NeedsEvidence
    );
    let error = world
        .registry
        .publish(
            &world.cast.lead,
            &publication("refund-worker-recovery", 1),
            2,
        )
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::PublicationDenied);
    assert!(error.message.contains("gate Verification failed"));
    assert_eq!(world.registry.state.denials.len(), 1);
    assert_eq!(
        world
            .registry
            .state
            .record("refund-worker-recovery", 1)
            .unwrap()
            .state,
        LifecycleState::NeedsEvidence
    );
}

/** 31. Publication, deprecation, and revocation need an authorized, independent person
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn refuses_unauthorized_privileged_transitions() {
    let mut world = World::new();
    let definition = world.diagnostics();
    world
        .registry
        .create_candidate(
            &world.cast.engineer,
            definition,
            Some("low-risk-technical"),
            1,
        )
        .unwrap();
    world.validate("deployment-failure-diagnostics", 1);
    let denied = world
        .registry
        .publish(
            &world.cast.engineer,
            &publication("deployment-failure-diagnostics", 1),
            2,
        )
        .unwrap_err();
    assert!(
        denied.message.contains("Publisher") && denied.message.contains("cannot publish their own"),
        "{denied}"
    );
    world.registry.grants = world
        .registry
        .grants
        .clone()
        .with(&world.cast.agent.id, &[Role::Publisher, Role::Validator]);
    let agent = world
        .registry
        .publish(
            &world.cast.agent,
            &publication("deployment-failure-diagnostics", 1),
            3,
        )
        .unwrap_err();
    assert!(
        agent.message.contains("an agent cannot act as Publisher"),
        "{agent}"
    );
    world
        .registry
        .publish(
            &world.cast.lead,
            &publication("deployment-failure-diagnostics", 1),
            4,
        )
        .unwrap();
    for state in [LifecycleState::Deprecated, LifecycleState::Revoked] {
        assert_eq!(
            world
                .registry
                .withdraw(
                    &world.cast.engineer,
                    "deployment-failure-diagnostics",
                    None,
                    state,
                    "no",
                    5
                )
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            world
                .registry
                .withdraw(
                    &world.cast.agent,
                    "deployment-failure-diagnostics",
                    None,
                    state,
                    "no",
                    5
                )
                .unwrap_err()
                .code,
            ErrorCode::Unauthorized
        );
    }
    assert_eq!(
        world.registry.state.denials.len(),
        6,
        "every refused privileged operation is audited"
    );
    assert_eq!(
        world
            .registry
            .state
            .record("deployment-failure-diagnostics", 1)
            .unwrap()
            .state,
        LifecycleState::Published
    );
}

/** 32. Revoked versions are never eligible for new use
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn excludes_revoked_versions() {
    let mut world = World::new();
    world.published();
    world
        .registry
        .withdraw(
            &world.cast.lead,
            "deployment-failure-diagnostics",
            None,
            LifecycleState::Revoked,
            "unsafe log access",
            80,
        )
        .unwrap();
    assert!(
        query::latest_eligible(&world.registry.state, "deployment-failure-diagnostics").is_none()
    );
    assert!(query::list(&world.registry.state, &RegistryQuery::default()).is_empty());
    let all = query::list(
        &world.registry.state,
        &RegistryQuery {
            include_withdrawn: true,
            ..RegistryQuery::default()
        },
    );
    assert_eq!(all[0].versions, vec![(1, LifecycleState::Revoked)]);
}

/** 33. Deprecation keeps the definition and its history
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn preserves_deprecation_history() {
    let mut world = World::new();
    let definition = world.published();
    world
        .registry
        .withdraw(
            &world.cast.lead,
            "deployment-failure-diagnostics",
            Some(1),
            LifecycleState::Deprecated,
            "superseded by a dashboard",
            90,
        )
        .unwrap();
    let record = world
        .registry
        .state
        .record("deployment-failure-diagnostics", 1)
        .unwrap();
    assert_eq!(record.definition, definition);
    assert_eq!(record.withdrawals[0].reason, "superseded by a dashboard");
    assert!(world
        .registry
        .state
        .runs
        .contains_key(&record.validation_runs[0]));
    let history = query::history(&world.registry.state, "deployment-failure-diagnostics").unwrap();
    assert_eq!(
        history.transitions.last().unwrap().to,
        LifecycleState::Deprecated
    );
    assert!(
        query::latest_eligible(&world.registry.state, "deployment-failure-diagnostics").is_none()
    );
}

/** 34. A new version does not inherit its predecessor's validation
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn new_versions_do_not_inherit_validation() {
    let mut world = World::new();
    let mut next = world.published();
    next.version = 2;
    next.previous_version_ref = Some(1);
    next.provenance.change_reason = Some("tweak".into());
    next.name = "Collect failed-deployment diagnostics (v2)".into();
    let record = world
        .registry
        .propose_version(&world.cast.engineer, next, Some("low-risk-technical"), 100)
        .unwrap();
    assert_eq!(record.state, LifecycleState::Candidate);
    assert!(record.validation_runs.is_empty());
    let gate = world
        .registry
        .eligibility(
            &world.cast.lead,
            &publication("deployment-failure-diagnostics", 2),
            101,
        )
        .unwrap();
    assert_eq!(gate.status, GateStatus::Failed);
    assert!(gate
        .findings
        .iter()
        .any(|finding| finding.code == "not_validated"));
}

/** 35. A hypothesis cannot verify a claim, however it is labeled
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn hypotheses_cannot_verify_claims() {
    let mut world = World::new();
    let mut definition = world.diagnostics();
    let guess = hypothesis(
        &world.cast.agent,
        Claim {
            subject: entity_id(EntityKind::Deployment, "dep-7731"),
            subject_kind: EntityKind::Deployment,
            subject_key: "dep-7731".into(),
            predicate: "recovered".into(),
            object: json!("yes"),
        },
        "it recovered",
        Vec::new(),
        None,
        1,
    );
    let id = world.capture.store.add_assertion(guess).unwrap();
    definition.verification.claims[0].assertion_refs = vec![id];
    definition.verification.claims[0].evidence_refs.clear();
    definition.verification.claims[0].status = ClaimStatus::Verified;
    let (status, reasons, _) = effective_claim(
        &definition,
        &definition.verification.claims[0],
        &world.sources(),
    );
    assert_eq!(status, ClaimStatus::Unresolved);
    assert!(reasons[0].contains("hypothesis"));
    assert_eq!(dry(&world, &definition).outcome, RunOutcome::NeedsEvidence);
}

/** 36. CI success cannot verify a missing business invariant
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn ci_success_does_not_verify_business_invariants() {
    let mut world = World::new();
    let recovery = world.recovery();
    let claim = recovery
        .verification
        .claims
        .iter()
        .find(|claim| claim.kind == ClaimKind::BusinessInvariant)
        .unwrap();
    let ci = world
        .capture
        .store
        .evidence
        .get(&claim.evidence_refs[0])
        .unwrap();
    assert_eq!(
        (ci.evidence_type, ci.field("status").as_deref()),
        (EvidenceType::CiRun, Some("success"))
    );
    let (status, reasons, _) = effective_claim(&recovery, claim, &world.sources());
    assert_eq!(status, ClaimStatus::Unresolved);
    assert!(reasons
        .iter()
        .any(|reason| reason.contains("does not verify a business invariant")));
    let ci_claim = recovery
        .verification
        .claims
        .iter()
        .find(|claim| claim.claim_id == "ci-passed")
        .unwrap();
    assert_eq!(
        effective_claim(&recovery, ci_claim, &world.sources()).0,
        ClaimStatus::Verified,
        "the same evidence does verify the technical claim"
    );
}

/** 37. Invocation outcomes never change the definition or its lifecycle state
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn invocations_are_separate_from_lifecycle() {
    let mut world = World::new();
    let definition = world.published();
    let failure = InvocationRef {
        invocation_id: "epi_next".into(),
        status: InvocationStatus::Failed,
        evidence_refs: vec!["evd_x".into()],
        recorded_by: world.cast.agent.clone(),
        recorded_at: 110,
    };
    world
        .registry
        .record_invocation(
            &world.cast.agent,
            "deployment-failure-diagnostics",
            1,
            failure,
        )
        .unwrap();
    let record = world
        .registry
        .state
        .record("deployment-failure-diagnostics", 1)
        .unwrap();
    assert_eq!(
        (record.state, &record.definition, record.invocations.len()),
        (LifecycleState::Published, &definition, 1)
    );
    assert_eq!(record.definition_digest, definition.digest());
}

/** 38. Findings keep their evidence references and reasons
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn findings_keep_evidence_and_reasons() {
    let mut world = World::new();
    let recovery = world.recovery();
    let ci_evidence = recovery.verification.claims[2].evidence_refs[0].clone();
    world
        .registry
        .create_candidate(&world.cast.engineer, recovery, None, 1)
        .unwrap();
    let run = world.validate("refund-worker-recovery", 1);
    let finding = run
        .gates
        .iter()
        .flat_map(|gate| gate.findings.iter())
        .find(|finding| {
            finding.code == "unverified_claim" && finding.message.contains("no-double-refund")
        })
        .unwrap();
    assert_eq!(finding.evidence_refs, vec![ci_evidence]);
    assert!(!finding.message.is_empty());
    let stored = &world.registry.state.runs[&run.run_id];
    assert_eq!(stored, &run);
    assert!(stored.gates.iter().all(|gate| gate.gate_version == 1
        && !gate.method.is_empty()
        && gate.validator == world.cast.engineer));
}

/** 39. An Evidence Pack becomes an intake by reference, without copying raw artifacts
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn intakes_reference_rather_than_copy() {
    let mut world = World::new();
    let intake = world
        .submit(&demo::refund_intake(&world.pack))
        .unwrap()
        .intake;
    let text = serde_json::to_string(&intake).unwrap();
    for blob in world.capture.store.blobs.values() {
        for fragment in [
            "KeyError",
            "Make refunds idempotent",
            "pytest tests/test_refunds.py",
        ] {
            if blob.to_string().contains(fragment) {
                assert!(
                    !text.contains(fragment),
                    "raw content '{fragment}' was copied into the intake"
                );
            }
        }
    }
    assert!(!text.contains("excerpt") && !text.contains("diff"));
    assert!(text.contains(&world.pack.evidence[0].evidence_id));
}

/** 40. Every intake reference resolves to its canonical Work Capture record
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn intake_references_resolve() {
    let mut world = World::new();
    let intake = world
        .submit(&demo::refund_intake(&world.pack))
        .unwrap()
        .intake;
    let observed = &intake.observed;
    let evidence = observed
        .code_changes
        .iter()
        .chain(&observed.tool_calls)
        .chain(&observed.ci_and_tests)
        .chain(&observed.deployments)
        .chain(&observed.runtime)
        .chain(&observed.recovery)
        .collect::<Vec<_>>();
    assert!(evidence.len() >= 8);
    for reference in evidence {
        let record = &world.capture.store.evidence[&reference.evidence_id];
        assert_eq!(
            (record.evidence_type, &record.source_ref.artifact_id),
            (reference.evidence_type, &reference.artifact_id)
        );
        assert_eq!(
            world.capture.store.artifacts[&reference.artifact_id].content_hash,
            reference.content_hash
        );
    }
    for assertion in &observed.assertions {
        assert_eq!(
            world.capture.store.assertions[&assertion.assertion_id].status,
            assertion.status
        );
    }
    assert!(world
        .capture
        .store
        .episodes
        .contains_key(&intake.work.episode_id));
    assert!(world.capture.store.audit(&intake.work.query_id).is_some());
    assert_eq!(
        observed.verified_claims.len()
            + observed.unresolved_claims.len()
            + observed.contradicted_claims.len(),
        observed.assertions.len()
    );
    assert!(observed
        .unresolved_claims
        .iter()
        .any(|id| world.capture.store.assertions[id].claim.predicate == "tests_passed"));
}

/** 41. Landscape and Crane references keep their versions and are never written
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn governance_references_keep_versions() {
    let mut world = World::new();
    let before = world.current.clone();
    let intake = world
        .submit(&demo::refund_intake(&world.pack))
        .unwrap()
        .intake;
    assert_eq!(intake.governance.zones[0].version.as_deref(), Some("v3"));
    assert_eq!(intake.governance.flows[0].version.as_deref(), Some("v2"));
    assert_eq!(
        intake.governance.policies[0].version.as_deref(),
        Some("c41d0e9a3b2f7781")
    );
    assert!(intake.governance.approval_level >= super::model::ReviewLevel::Admin);
    let mut weakening = world.diagnostics();
    weakening.execution.steps[0].description =
        "Edit .crane/policies/refunds.crane to relax the refund rule".into();
    assert!(
        errors(&dry(&world, &weakening)).contains("policy_change"),
        "policy changes stay in Crane's signed workflow"
    );
    assert_eq!(world.current, before);
}

/** 42. DataSea agent execution evidence stays traceable
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn datasea_references_stay_traceable() {
    let mut world = World::new();
    let intake = world
        .submit(&demo::refund_intake(&world.pack))
        .unwrap()
        .intake;
    assert_eq!(intake.observed.tool_calls.len(), 2);
    for reference in &intake.observed.tool_calls {
        let record = &world.capture.store.evidence[&reference.evidence_id];
        assert_eq!(record.source_ref.resource_id, "datasea");
        assert_eq!(record.refs.session.as_deref(), Some("ses_demo01"));
        assert_eq!(reference.revision, record.source_ref.revision);
    }
}

/** 43. The component adds no dependency on the excluded product, no crate, no service
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn adds_no_excluded_dependency() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let excluded = ["so", "il"].concat();
    let mut pending = vec![
        root.join("src").join("capability"),
        root.join("src").join("cli"),
    ];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap_or_default()
                .to_ascii_lowercase();
            let words = text
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .collect::<BTreeSet<_>>();
            assert!(
                !words.contains(excluded.as_str()),
                "{} references the excluded product",
                path.display()
            );
            if path.starts_with(root.join("src").join("capability"))
                && path.file_name().is_some_and(|name| name != "tests.rs")
            {
                assert!(
                    !text.contains("mongodb") && !text.contains("crate::server"),
                    "{}",
                    path.display()
                );
            }
        }
    }
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(!manifest.to_ascii_lowercase().contains(&excluded));
}

/** Applicability is decided from exact declared values; unknown inputs never match
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn evaluates_applicability_exactly() {
    let mut world = World::new();
    let definition = world.diagnostics();
    let situation = |task: Option<&str>, environment: Option<&str>, preconditions: &[&str]| {
        ApplicabilityQuery {
            task_class: task.map(str::to_string),
            environment: environment.map(str::to_string),
            satisfied_preconditions: preconditions.iter().map(|id| id.to_string()).collect(),
            entities: Vec::new(),
        }
    };
    let decide = |query: ApplicabilityQuery| ExactMatcher.evaluate(&definition, &query).applicable;
    assert_eq!(
        decide(situation(
            Some("deployment-failure-triage"),
            Some("production"),
            &["deployment-known", "read-access"]
        )),
        Truth::Yes
    );
    assert_eq!(
        decide(situation(
            Some("deployment-failure-triage"),
            None,
            &["deployment-known", "read-access"]
        )),
        Truth::Unknown
    );
    assert_eq!(
        decide(situation(
            Some("deployment-failure-triage"),
            Some("production"),
            &["deployment-known"]
        )),
        Truth::Unknown
    );
    assert_eq!(
        decide(situation(
            Some("deployment-failure-triage"),
            Some("staging"),
            &["deployment-known", "read-access"]
        )),
        Truth::No
    );
    assert_eq!(
        decide(situation(
            Some("deployment-rollback"),
            Some("production"),
            &["deployment-known", "read-access"]
        )),
        Truth::No
    );
}

/** The deterministic demonstration covers the full flow
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn runs_the_demonstration() {
    let report = demo::run().unwrap();
    assert_eq!(report.intakes[0]["status"], json!("incomplete"));
    assert_eq!(report.intakes[1]["status"], json!("ready"));
    assert!(report.duplicate_submission);
    assert_eq!(
        (report.recovery_outcome, report.diagnostics_outcome),
        (RunOutcome::NeedsEvidence, RunOutcome::Passed)
    );
    assert!(report
        .recovery_publication
        .starts_with("publication_denied"));
    assert_eq!(report.hypothesis_claim.0, ClaimStatus::Unresolved);
    assert_eq!(
        (report.eligible_before_change, report.eligible_after_change),
        (Some(1), None)
    );
    assert_eq!(report.history.versions.len(), 2);
    let again: Value = serde_json::to_value(demo::run().unwrap()).unwrap();
    assert_eq!(
        again,
        serde_json::to_value(&report).unwrap(),
        "the demonstration is deterministic"
    );
    let unknown = Grants::default().with("x", &[Role::Contributor]);
    assert!(unknown
        .require(&Actor::new("x", ActorKind::Human), Role::Validator)
        .is_err());
}
