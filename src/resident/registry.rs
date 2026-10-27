// Lifecycle changes of definitions, assignments, and RCA graphs. Every change is made by a person
// (the CLI refuses agents), is validated against the current signed state where it gives an
// assignment the ability to act, and is recorded with who, when, and why. Versions of definitions
// and graphs are immutable: an edit is a new version.
//
// Assignment lifecycle: draft -> validated -> active <-> paused; active or paused -> expired
// (lifetime) -> active (renew, after validation again); any -> revoked (never undone). Activation
// and renewal re-validate, so a test or an earlier validation never grants anything by itself.

use serde_json::json;

use super::authority::{admit, Authority, Decision};
use super::context::PackCache;
use super::model::{
    limit_ceilings, transition_allowed, Assignment, AssignmentDraft, Change, Definition,
    DefinitionRecord, DefinitionState, Lifecycle, Problems, Validation, DAY_MS, MAX_LIFETIME_DAYS,
};
use super::plan::StepType;
use super::rca::{self, Graph, Origin, RcaRecord, RcaState};
use super::store::{ActivationState, ResidentStore, StepState};
use crate::brick::model::{Author, Subject, Target};
use crate::platform::notes::note;

/** Validate a definition: identifiers, text without secrets, supported trigger sources, allowed
 * actions without duplicates, limits within their ceilings, exact Brick references
 * Input
    - definition: &Definition - definition
 * Output
    - (Problems, Problems) blocking problems and warnings
*/
pub(crate) fn check_definition(definition: &Definition) -> (Problems, Problems) {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    if let Err(error) = crate::platform::validate_name("definition", &definition.definition_id) {
        problems.push(("DEFINITION_ID".to_string(), error));
    }
    if definition.version == 0 {
        problems.push((
            "DEFINITION_VERSION".to_string(),
            "versions start at 1".into(),
        ));
    }
    if definition.name.trim().is_empty() || definition.name.len() > 120 {
        problems.push((
            "DEFINITION_NAME".to_string(),
            "a name of 1-120 characters is required".into(),
        ));
    }
    if definition.purpose.len() > 4000 {
        problems.push((
            "DEFINITION_PURPOSE".to_string(),
            "the purpose is longer than 4000 characters".into(),
        ));
    }
    for (field, text) in [
        ("name", &definition.name),
        ("purpose", &definition.purpose),
        ("owner", &definition.owner),
    ] {
        if crate::telemetry::redact::redact(text).1 > 0 {
            problems.push((
                "DEFINITION_SECRET".to_string(),
                format!("the {field} holds a secret-looking value"),
            ));
        }
    }
    if definition.triggers.is_empty() {
        problems.push((
            "DEFINITION_NO_TRIGGER".to_string(),
            "name at least one trigger source".into(),
        ));
    }
    for source in &definition.triggers {
        if source.delivery().is_none() {
            problems.push((
                "TRIGGER_UNSUPPORTED".to_string(),
                format!(
                    "the {} source has no connector in this version",
                    source.name()
                ),
            ));
        }
    }
    if definition.actions.is_empty() {
        problems.push((
            "DEFINITION_NO_ACTION".to_string(),
            "allow at least one action-step type".into(),
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for policy in &definition.actions {
        if !seen.insert(policy.step_type) {
            problems.push((
                "DEFINITION_DUPLICATE_ACTION".to_string(),
                format!("{} is listed twice", policy.step_type.name()),
            ));
        }
        if let Some(reason) = policy.step_type.info().available {
            warnings.push((
                "STEP_UNAVAILABLE".to_string(),
                format!("{}: {reason}", policy.step_type.name()),
            ));
        }
    }
    for (name, value, ceiling) in limit_ceilings(&definition.limits) {
        if value > ceiling {
            problems.push((
                "LIMIT_ABOVE_CEILING".to_string(),
                format!("{name} is {value}; the ceiling is {ceiling}"),
            ));
        }
    }
    if definition.limits.max_steps == 0 {
        problems.push((
            "LIMIT_INVALID".to_string(),
            "max_steps must be at least 1".into(),
        ));
    }
    if definition.limits.max_model_calls > 0 {
        warnings.push((
            "NO_MODEL_PROVIDER".to_string(),
            "no model provider is configured in this repository; reasoning is deterministic and makes no model call".into(),
        ));
    }
    for brick in &definition.bricks {
        if !matches!(Subject::parse(brick), Ok(Subject::Brick { .. })) {
            problems.push((
                "BRICK_REFERENCE".to_string(),
                format!("'{brick}' must be an exact Brick version ID@N"),
            ));
        }
    }
    (problems, warnings)
}

/** Store a new definition version as a draft (versions are immutable)
 * Input
    - store: &mut ResidentStore - store
    - definition: Definition - content
    - author: &Author - author
    - now: u64 - Unix milliseconds
 * Output
    - Result<DefinitionRecord, String>
*/
pub(crate) fn save_definition(
    store: &mut ResidentStore,
    mut definition: Definition,
    author: &Author,
    now: u64,
) -> Result<DefinitionRecord, String> {
    let (problems, _) = check_definition(&definition);
    if let Some((code, message)) = problems.first() {
        return Err(format!("{code}: {message}"));
    }
    let versions = store
        .state
        .definitions
        .entry(definition.definition_id.clone())
        .or_default();
    if versions.contains_key(&definition.version) {
        return Err(format!(
            "definition {}@{} already exists; versions are immutable, save version {}",
            definition.definition_id,
            definition.version,
            versions.keys().next_back().copied().unwrap_or(0) + 1
        ));
    }
    definition.created_by = author.clone();
    definition.created_at = now;
    let record = DefinitionRecord {
        digest: definition.digest(),
        definition,
        state: DefinitionState::Draft,
        self_reviewed: false,
        changes: Vec::new(),
    };
    versions.insert(record.definition.version, record.clone());
    note(
        "RESIDENT_DEFINITION_SAVED",
        json!({"definition": format!("{}@{}", record.definition.definition_id, record.definition.version), "digest": record.digest}),
    );
    Ok(record)
}

/** Activate or revoke a definition version
 * Input
    - store: &mut ResidentStore - store
    - definition_id: &str - definition
    - version: u32 - version
    - to: DefinitionState - Active or Revoked
    - by: &Author - who
    - reason: &str - why
    - now: u64 - Unix milliseconds
 * Output
    - Result<DefinitionRecord, String>
*/
pub(crate) fn change_definition(
    store: &mut ResidentStore,
    definition_id: &str,
    version: u32,
    to: DefinitionState,
    by: &Author,
    reason: &str,
    now: u64,
) -> Result<DefinitionRecord, String> {
    if reason.trim().is_empty() {
        return Err("a reason is required".into());
    }
    let record = store
        .state
        .definitions
        .get_mut(definition_id)
        .and_then(|versions| versions.get_mut(&version))
        .ok_or_else(|| format!("definition {definition_id}@{version} does not exist"))?;
    let allowed = matches!(
        (record.state, to),
        (DefinitionState::Draft, DefinitionState::Active)
            | (
                DefinitionState::Draft | DefinitionState::Active,
                DefinitionState::Revoked
            )
    );
    if !allowed {
        return Err(format!(
            "definition {definition_id}@{version} is {} and cannot become {}",
            crate::resident::word(&record.state),
            crate::resident::word(&to)
        ));
    }
    if to == DefinitionState::Active {
        let (problems, _) = check_definition(&record.definition);
        if let Some((code, message)) = problems.first() {
            return Err(format!("{code}: {message}"));
        }
        record.self_reviewed = record.definition.created_by.name == by.name;
    }
    record.state = to;
    record.changes.push(Change {
        to: crate::resident::word(&to),
        by: by.clone(),
        reason: reason.into(),
        at: now,
    });
    let record = record.clone();
    note(
        if to == DefinitionState::Active {
            "RESIDENT_DEFINITION_ACTIVATED"
        } else {
            "RESIDENT_DEFINITION_REVOKED"
        },
        json!({"definition": format!("{definition_id}@{version}"), "digest": record.digest, "self_reviewed": record.self_reviewed}),
    );
    Ok(record)
}

/** Store a new assignment as a draft
 * Input
    - store: &mut ResidentStore - store
    - draft: AssignmentDraft - what the person wrote
    - author: &Author - author
    - now: u64 - Unix milliseconds
 * Output
    - Result<Assignment, String>
*/
pub(crate) fn create_assignment(
    store: &mut ResidentStore,
    draft: AssignmentDraft,
    author: &Author,
    now: u64,
) -> Result<Assignment, String> {
    crate::platform::validate_name("assignment", &draft.assignment_id)?;
    if store.state.assignments.contains_key(&draft.assignment_id) {
        return Err(format!("assignment {} already exists", draft.assignment_id));
    }
    let agent_valid = !draft.agent.is_empty()
        && draft
            .agent
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !agent_valid {
        return Err(format!(
            "'{}' is not a provider name or agent id",
            draft.agent
        ));
    }
    let target = Target::parse(&draft.target)?;
    let definition = store
        .definition(&draft.definition_id, Some(draft.definition_version))?
        .clone();
    let mut assignment = Assignment {
        assignment_id: draft.assignment_id,
        definition_id: draft.definition_id,
        definition_version: draft.definition_version,
        definition_digest: definition.digest,
        agent: draft.agent,
        target,
        rca: draft.rca,
        subscriptions: draft.subscriptions,
        timers: draft.timers,
        lifetime_days: draft.lifetime_days,
        starts_at: 0,
        expires_at: 0,
        lifecycle: Lifecycle::Draft,
        validation: Validation::default(),
        created_by: author.clone(),
        created_at: now,
        changes: Vec::new(),
        record_digest: String::new(),
    };
    assignment.change(Lifecycle::Draft, author, "created", now);
    store.put_assignment(assignment.clone());
    note(
        "RESIDENT_CREATED",
        json!({"assignment": assignment.assignment_id, "definition": format!("{}@{}", assignment.definition_id, assignment.definition_version), "target": assignment.target.key(), "agent": assignment.agent, "state": "draft"}),
    );
    Ok(assignment)
}

/** Validate an assignment against its definition and the current signed state, as if it were
 * activated now
 * Input
    - store: &ResidentStore - store
    - authority: &Authority - Crane view, freshly read
    - assignment: &Assignment - assignment
    - now: u64 - Unix milliseconds
 * Output
    - Validation
*/
pub(crate) fn check_assignment(
    store: &ResidentStore,
    authority: &Authority,
    assignment: &Assignment,
    now: u64,
) -> Validation {
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let definition = store
        .definition(
            &assignment.definition_id,
            Some(assignment.definition_version),
        )
        .ok();
    if assignment.lifetime_days == 0 || u64::from(assignment.lifetime_days) > MAX_LIFETIME_DAYS {
        problems.push((
            "LIFETIME".to_string(),
            format!("lifetime_days must be 1-{MAX_LIFETIME_DAYS}"),
        ));
    }
    // The checks that decide whether it may act, as if it were active for a new period
    let mut simulated = assignment.clone();
    simulated.lifecycle = Lifecycle::Active;
    simulated.expires_at = now + u64::from(assignment.lifetime_days.max(1)) * DAY_MS;
    let admitted = admit(authority, &simulated, definition, now);
    if admitted.decision != Decision::Allow {
        problems.extend(admitted.reasons.iter().cloned());
    }
    if assignment.subscriptions.is_empty() && assignment.timers.is_empty() {
        problems.push((
            "NO_TRIGGER".to_string(),
            "subscribe to at least one trigger source or add a timer".into(),
        ));
    }
    let allowed_sources = definition
        .map(|record| record.definition.triggers.clone())
        .unwrap_or_default();
    for subscription in &assignment.subscriptions {
        if !allowed_sources.contains(&subscription.source) {
            problems.push((
                "TRIGGER_NOT_ALLOWED".to_string(),
                format!(
                    "the definition does not allow the {} source",
                    subscription.source.name()
                ),
            ));
        }
        if subscription.source.delivery().is_none() {
            problems.push((
                "TRIGGER_UNSUPPORTED".to_string(),
                format!("the {} source has no connector", subscription.source.name()),
            ));
        }
        if subscription.source == super::model::TriggerSource::Timer {
            problems.push((
                "TIMER_SUBSCRIPTION".to_string(),
                "timers are declared under 'timers', not as a subscription".into(),
            ));
        }
        if subscription.kinds.is_empty() {
            problems.push((
                "TRIGGER_NO_KIND".to_string(),
                format!(
                    "the {} subscription names no kind",
                    subscription.source.name()
                ),
            ));
        }
        for kind in &subscription.kinds {
            if let Err(error) = super::model::check_kind(kind) {
                problems.push(("TRIGGER_KIND".to_string(), error));
            }
        }
        if subscription.attributes.len() > 16 {
            problems.push((
                "TRIGGER_FILTER".to_string(),
                "at most 16 attribute filters".into(),
            ));
        }
    }
    if !assignment.timers.is_empty()
        && !allowed_sources.contains(&super::model::TriggerSource::Timer)
    {
        problems.push((
            "TRIGGER_NOT_ALLOWED".to_string(),
            "the definition does not allow timers".into(),
        ));
    }
    for timer in &assignment.timers {
        if crate::platform::validate_name("timer", &timer.timer_id).is_err()
            || timer.every_minutes < 5
            || timer.every_minutes > 10_080
        {
            problems.push((
                "TIMER".to_string(),
                format!(
                    "timer '{}' needs a valid id and 5-10080 minutes",
                    timer.timer_id
                ),
            ));
        }
    }
    match &assignment.rca {
        None => warnings.push((
            "NO_RCA".to_string(),
            "without an RCA graph every activation ends in a question or a proposal".into(),
        )),
        Some(reference) => match store.rca(&reference.rca_id, Some(reference.version)) {
            Err(error) => problems.push(("RCA_MISSING".to_string(), error)),
            Ok(record) => {
                if record.graph.target != assignment.target.key() {
                    problems.push((
                        "RCA_TARGET".to_string(),
                        format!(
                            "the RCA graph explains {}, not {}",
                            record.graph.target,
                            assignment.target.key()
                        ),
                    ));
                }
                match record.state {
                    RcaState::Reviewed => {}
                    RcaState::Proposed => warnings.push((
                        "RCA_PROPOSED".to_string(),
                        "the RCA graph is not reviewed: its remediations are proposed to a person, never run directly".into(),
                    )),
                    other => problems.push(("RCA_NOT_USABLE".to_string(), format!("the RCA graph is {}", crate::resident::word(&other)))),
                }
            }
        },
    }
    if let Some(record) = definition {
        if record.definition.policy(StepType::NotifySlack).is_some()
            && !authority.connectors.get("slack").copied().unwrap_or(false)
        {
            warnings.push(("CONNECTOR_NOT_CONFIGURED".to_string(), "NOTIFY_SLACK is allowed but Slack is not configured; those steps will be unavailable".into()));
        }
        for policy in &record.definition.actions {
            if let Some(reason) = policy.step_type.info().available {
                warnings.push((
                    "STEP_UNAVAILABLE".to_string(),
                    format!("{}: {reason}", policy.step_type.name()),
                ));
            }
        }
    }
    Validation {
        passed: problems.is_empty(),
        problems,
        warnings,
        authority_digest: authority.digest(),
        at: now,
    }
}

/** Validate an assignment and record the result (a passing draft becomes validated)
 * Input
    - store: &mut ResidentStore - store
    - authority: &Authority - Crane view
    - assignment_id: &str - assignment
    - by: &Author - who
    - now: u64 - Unix milliseconds
 * Output
    - Result<Validation, String>
*/
pub(crate) fn validate_assignment(
    store: &mut ResidentStore,
    authority: &Authority,
    assignment_id: &str,
    by: &Author,
    now: u64,
) -> Result<Validation, String> {
    let mut assignment = store.assignment(assignment_id)?.clone();
    let validation = check_assignment(store, authority, &assignment, now);
    assignment.validation = validation.clone();
    if validation.passed
        && matches!(
            assignment.lifecycle,
            Lifecycle::Draft | Lifecycle::Validated
        )
    {
        assignment.change(Lifecycle::Validated, by, "validation passed", now);
    }
    store.put_assignment(assignment);
    note(
        "RESIDENT_VALIDATED",
        json!({"assignment": assignment_id, "result": if validation.passed { "PASSED" } else { "FAILED" }, "problems": validation.problems.len(), "warnings": validation.warnings.len()}),
    );
    Ok(validation)
}

/** Change an assignment's lifecycle: activate (validated or expired, after validating again),
 * pause, resume, expire, renew (a new period), or revoke; revocation and expiry cancel the work
 * that has not started an effect
 * Input
    - store: &mut ResidentStore - store
    - cache: &mut PackCache - context pack cache
    - authority: &Authority - Crane view, freshly read
    - assignment_id: &str - assignment
    - to: Lifecycle - Active, Paused, Expired, or Revoked
    - by: &Author - who
    - reason: &str - why
    - now: u64 - Unix milliseconds
 * Output
    - Result<Assignment, String>
*/
#[allow(clippy::too_many_arguments)]
pub(crate) fn change_assignment(
    store: &mut ResidentStore,
    cache: &mut PackCache,
    authority: &Authority,
    assignment_id: &str,
    to: Lifecycle,
    by: &Author,
    reason: &str,
    now: u64,
) -> Result<Assignment, String> {
    if reason.trim().is_empty() {
        return Err("a reason is required".into());
    }
    let mut assignment = store.assignment(assignment_id)?.clone();
    let from = assignment.lifecycle;
    if !transition_allowed(from, to) {
        return Err(format!(
            "assignment {assignment_id} is {} and cannot become {}",
            crate::resident::word(&from),
            crate::resident::word(&to)
        ));
    }
    let kind = match (from, to) {
        (Lifecycle::Validated | Lifecycle::Expired, Lifecycle::Active) => {
            // Activation and renewal validate again against the signed state of this moment
            let validation = check_assignment(store, authority, &assignment, now);
            assignment.validation = validation.clone();
            if !validation.passed {
                let (code, message) = validation.problems.first().cloned().unwrap_or_default();
                return Err(format!("the assignment does not validate now ({code}: {message}); nothing was activated"));
            }
            assignment.starts_at = now;
            assignment.expires_at = now + u64::from(assignment.lifetime_days) * DAY_MS;
            if from == Lifecycle::Expired {
                "RESIDENT_RENEWED"
            } else {
                "RESIDENT_ACTIVATED"
            }
        }
        (Lifecycle::Paused, Lifecycle::Active) => "RESIDENT_RESUMED",
        (_, Lifecycle::Paused) => "RESIDENT_PAUSED",
        (_, Lifecycle::Expired) => "RESIDENT_EXPIRED",
        (_, Lifecycle::Revoked) => "RESIDENT_REVOKED",
        _ => "RESIDENT_VALIDATED",
    };
    assignment.change(to, by, reason, now);
    store.put_assignment(assignment.clone());
    if matches!(to, Lifecycle::Revoked | Lifecycle::Expired) {
        cache.drop_pack(assignment_id);
        stop_work(
            store,
            assignment_id,
            &format!("the assignment is {}", crate::resident::word(&to)),
            now,
        );
    }
    note(
        kind,
        json!({"assignment": assignment_id, "state": to, "from": from, "target": assignment.target.key(), "expires_at": assignment.expires_at}),
    );
    Ok(assignment)
}

/** Stop the unfinished activations of an assignment: queued and waiting work is cancelled; a step
 * that started an external call keeps its uncertainty for reconciliation
 * Input
    - store: &mut ResidentStore - store
    - assignment_id: &str - assignment
    - reason: &str - why
    - now: u64 - Unix milliseconds
 * Output
    - None
*/
pub(crate) fn stop_work(store: &mut ResidentStore, assignment_id: &str, reason: &str, now: u64) {
    let ids = store
        .state
        .activations
        .values()
        .filter(|activation| activation.assignment_id == assignment_id && !activation.state.ended())
        .map(|activation| activation.activation_id.clone())
        .collect::<Vec<_>>();
    for id in ids {
        let Ok(mut activation) = store.activation(&id).cloned() else {
            continue;
        };
        let mut uncertain = false;
        for record in activation.steps.values_mut() {
            match record.state {
                StepState::Dispatching | StepState::Uncertain => {
                    record.state = StepState::Uncertain;
                    uncertain = true;
                }
                state if !state.finished() => {
                    record.state = StepState::Cancelled;
                    record.detail = reason.into();
                    record.updated_at = now;
                }
                _ => {}
            }
        }
        activation.lease = None;
        if uncertain {
            activation.record(
                ActivationState::ReconciliationRequired,
                "RESIDENT_RECONCILIATION_REQUIRED",
                format!("{reason}; an external effect is uncertain and must be reconciled"),
                now,
            );
        } else {
            activation.record(
                ActivationState::Cancelled,
                "RESIDENT_ACTIVATION_COMPLETED",
                reason,
                now,
            );
            activation.outcome = Some(super::store::Outcome {
                status: "CANCELLED".into(),
                reason: reason.into(),
            });
        }
        store.put_activation(activation);
    }
}

/** Store a person's RCA graph version (proposed until reviewed)
 * Input
    - store: &mut ResidentStore - store
    - graph: Graph - content
    - author: &Author - author
    - now: u64 - Unix milliseconds
 * Output
    - Result<RcaRecord, String>
*/
pub(crate) fn save_rca(
    store: &mut ResidentStore,
    graph: Graph,
    author: &Author,
    now: u64,
) -> Result<RcaRecord, String> {
    let problems = rca::validate(&graph);
    if let Some((code, message)) = problems.first() {
        return Err(format!("{code}: {message}"));
    }
    let versions = store.state.rcas.entry(graph.rca_id.clone()).or_default();
    if versions.contains_key(&graph.version) {
        return Err(format!(
            "RCA graph {}@{} already exists; save version {}",
            graph.rca_id,
            graph.version,
            versions.keys().next_back().copied().unwrap_or(0) + 1
        ));
    }
    let record = RcaRecord {
        digest: graph.digest(),
        graph,
        state: RcaState::Proposed,
        origin: Origin::Person,
        author: author.clone(),
        self_reviewed: false,
        changes: vec![Change {
            to: "proposed".into(),
            by: author.clone(),
            reason: "saved".into(),
            at: now,
        }],
    };
    versions.insert(record.graph.version, record.clone());
    note(
        "RESIDENT_RCA_SAVED",
        json!({"rca": format!("{}@{}", record.graph.rca_id, record.graph.version), "digest": record.digest, "target": record.graph.target, "state": "proposed"}),
    );
    Ok(record)
}

/** Review an RCA graph version: accept (reviewed) or reject. Review does not change any node's
 * basis: hypotheses stay hypotheses until a person writes a new version that says otherwise
 * Input
    - store: &mut ResidentStore - store
    - rca_id: &str - graph
    - version: u32 - version
    - accept: bool - accept or reject
    - by: &Author - reviewer
    - reason: &str - why
    - now: u64 - Unix milliseconds
 * Output
    - Result<RcaRecord, String>
*/
pub(crate) fn review_rca(
    store: &mut ResidentStore,
    rca_id: &str,
    version: u32,
    accept: bool,
    by: &Author,
    reason: &str,
    now: u64,
) -> Result<RcaRecord, String> {
    if reason.trim().is_empty() {
        return Err("a reason is required".into());
    }
    let record = store
        .state
        .rcas
        .get_mut(rca_id)
        .and_then(|versions| versions.get_mut(&version))
        .ok_or_else(|| format!("RCA graph {rca_id}@{version} does not exist"))?;
    if record.state != RcaState::Proposed {
        return Err(format!(
            "RCA graph {rca_id}@{version} is {} and was already decided",
            crate::resident::word(&record.state)
        ));
    }
    if accept {
        let problems = rca::validate(&record.graph);
        if let Some((code, message)) = problems.first() {
            return Err(format!("{code}: {message}"));
        }
    }
    record.self_reviewed = record.author.name == by.name && matches!(record.origin, Origin::Person);
    rca::change(
        record,
        if accept {
            RcaState::Reviewed
        } else {
            RcaState::Rejected
        },
        by,
        reason,
        now,
    );
    let record = record.clone();
    note(
        "RESIDENT_RCA_REVIEWED",
        json!({"rca": format!("{rca_id}@{version}"), "result": if accept { "REVIEWED" } else { "REJECTED" }, "self_reviewed": record.self_reviewed}),
    );
    Ok(record)
}
