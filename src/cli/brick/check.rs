// crane brick validate | test. Validate checks a draft (or an activated version) against its type's
// schema, the registered operations, the isolation profiles, and the store, and records the result
// in the draft (informational: activation validates again). Test runs the type's deterministic
// checks: for enforced types it simulates the Brick on a target and shows exactly which files the
// guard would restrict (the functions enforcement uses, so the simulation cannot differ from it);
// verification and provenance checks are evaluated against the working tree and the signed state;
// declared-only types say so instead of passing.

use serde_json::{json, Value};

use super::{find, open, target_option, targets, Found, Repo};
use crate::brick::config::{self, Config, ProvenanceItem};
use crate::brick::model::{
    Attachment, AttachmentState, Brick, BrickEntry, Subject, ValidationStatus, VersionState,
};
use crate::brick::runtime::{run_checks, CheckOutcome};
use crate::brick::store;
use crate::brick::types::Support;
use crate::brick::validate;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run 'crane brick validate'
 * Input
    - args: &[String] - arguments after "validate"
 * Output
    - Result<(), String>
*/
pub(super) fn validate(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane brick validate ID[@N]")?;
    let repo = open()?;
    let found = find(&repo, &args.positional[0])?;
    let brick = match &found {
        Found::Draft(draft) => draft.brick.clone(),
        Found::Version(entry) => entry.brick.clone(),
    };
    let validation = validate::brick(&brick, &repo.store, &repo.workspace.root);
    if let Found::Draft(mut draft) = found {
        draft.validation = validation.clone();
        store::save_brick_draft(&repo.workspace, &draft)?;
    }
    crate::platform::notes::note(
        "BRICK_VALIDATED",
        json!({"brick": brick.reference().to_string(), "type": brick.brick_type, "validation": validation.status, "problems": validation.problems.len()}),
    );
    let status = match validation.status {
        ValidationStatus::Passed => Status::Passed,
        ValidationStatus::Blocked => Status::Blocked,
        ValidationStatus::Inconclusive | ValidationStatus::NotRun => Status::Unknown,
        ValidationStatus::Failed => Status::Failed,
    };
    let detail = validation
        .problems
        .iter()
        .map(|problem| format!("  {}: {}", problem.code, problem.message))
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.validate",
        status,
        format!(
            "{} {}: {}",
            brick.reference(),
            brick.brick_type.name(),
            if validation.problems.is_empty() {
                "validates".to_string()
            } else {
                format!("{} problem(s)", validation.problems.len())
            }
        ),
    )
    .reference("brick_ref", brick.reference().to_string())
    .reference("digest", brick.digest())
    .reasons(
        validation
            .problems
            .iter()
            .map(|problem| problem.code.clone()),
    )
    .data(json!({"validation": validation}));
    finish(&args, response, &detail)
}

/** A test case result
 * Input
    - name: &str - case
    - status: &str - PASS, FAIL, UNKNOWN, NOT_RUN, BLOCKED, DECLARED
    - detail: impl Into<String> - what was found
 * Output
    - Value
*/
fn case(name: &str, status: &str, detail: impl Into<String>) -> Value {
    json!({"name": name, "status": status, "detail": detail.into()})
}

/** Simulate a Brick attached to a target: the store with one more enabled attachment, then the
 * effect preview enforcement would apply
 * Input
    - repo: &Repo - repository
    - brick: &Brick - definition (a draft is simulated as if activated)
    - target: &str - target key
 * Output
    - Result<Value, String> the preview
*/
fn simulate(repo: &Repo, brick: &Brick, target: &str) -> Result<Value, String> {
    let all = targets(repo)?;
    let info = all
        .get(target)
        .ok_or_else(|| format!("{target} is not a Zone or Flow of .crane/governance.yaml"))?;
    let mut simulated = repo.store.clone();
    simulated
        .bricks
        .retain(|entry| entry.brick.reference() != brick.reference());
    simulated.bricks.push(BrickEntry {
        brick: brick.clone(),
        digest: brick.digest(),
        state: VersionState::Active,
        validation: Default::default(),
        activated_by: Default::default(),
        activated_at: 0,
        reason: "simulation".into(),
        self_reviewed: false,
        changed_by: None,
        changed_at: None,
        change_reason: None,
    });
    simulated.attachments.push(Attachment {
        attachment_id: "simulation".into(),
        target: crate::brick::model::Target::parse(target)?,
        subject: Subject::Brick {
            brick_id: brick.brick_id.clone(),
            version: brick.version,
        },
        current_context: Default::default(),
        state: AttachmentState::Enabled,
        attached_by: Default::default(),
        attached_at: 0,
    });
    Ok(crate::brick::effect::preview(
        &repo.workspace,
        &simulated,
        target,
        &info.files,
    ))
}

/** Run 'crane brick test'
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["target", "flow", "zone"], &[])?;
    args.expect_positional(1, "crane brick test ID[@N] [--target flow:ID|zone:ID]")?;
    let repo = open()?;
    let brick = match find(&repo, &args.positional[0])? {
        Found::Draft(draft) => draft.brick,
        Found::Version(entry) => entry.brick,
    };
    let validation = validate::brick(&brick, &repo.store, &repo.workspace.root);
    let mut cases = vec![case(
        "validation",
        match validation.status {
            ValidationStatus::Passed => "PASS",
            ValidationStatus::Blocked => "BLOCKED",
            _ => "FAIL",
        },
        validation
            .problems
            .iter()
            .map(|problem| format!("{}: {}", problem.code, problem.message))
            .collect::<Vec<_>>()
            .join("; "),
    )];
    // The targets to simulate on: the one given, else the ones it is attached to
    let mut on = target_option(&args)?
        .map(|target| vec![target.key()])
        .unwrap_or_default();
    if on.is_empty() {
        on = repo
            .store
            .attachments
            .iter()
            .filter(|attachment| matches!(&attachment.subject, Subject::Brick { brick_id, .. } if *brick_id == brick.brick_id))
            .map(|attachment| attachment.target.key())
            .filter(|key| !key.starts_with("runbook:"))
            .collect();
    }
    let mut effects = Vec::new();
    let info = brick.brick_type.catalog();
    match config::parse(brick.brick_type, &brick.config) {
        Err(problem) => cases.push(case("configuration", "FAIL", problem.message)),
        Ok(parsed) => {
            match &parsed {
                Config::Scope(_)
                | Config::Constraint(_)
                | Config::Permission(_)
                | Config::Approval(_)
                | Config::Isolation(_)
                | Config::Precondition(_) => {
                    if on.is_empty() {
                        cases.push(case(
                            "enforcement",
                            "NOT_RUN",
                            "give --target flow:ID or zone:ID to simulate it on real files",
                        ));
                    }
                    for target in &on {
                        let preview = simulate(&repo, &brick, target)?;
                        let restricted = preview["files"].as_array().map_or(0, Vec::len);
                        let resolved = preview["scope_resolved"].as_bool().unwrap_or(true);
                        for gap in preview["controls"]["gaps"].as_array().into_iter().flatten() {
                            cases.push(case(
                                "gap",
                                "UNKNOWN",
                                gap["message"].as_str().unwrap_or_default(),
                            ));
                        }
                        cases.push(case(
                        &format!("enforcement on {target}"),
                        if resolved { "PASS" } else { "FAIL" },
                        if !resolved {
                            "the scope matches no file of the target, so the whole target becomes observe-only (fail closed)".to_string()
                        } else {
                            format!("{restricted} file(s) of {target} restricted by the guard")
                        },
                    ));
                        effects.push(preview);
                    }
                }
                Config::Verification(checks) => {
                    for (check, outcome, detail) in run_checks(&repo.workspace, &checks.checks) {
                        cases.push(case(&check, outcome_word(outcome), detail));
                    }
                }
                Config::Provenance(provenance) => {
                    for item in &provenance.require {
                        let (status, detail) = match item {
                            ProvenanceItem::SignedState => {
                                let outcome =
                                    run_checks(&repo.workspace, &[config::Check::SignedState]);
                                (outcome_word(outcome[0].1), outcome[0].2.clone())
                            }
                            ProvenanceItem::AuditChain => {
                                match crate::governance::audit::verify(&repo.state.audit) {
                                    Ok(()) => (
                                        "PASS",
                                        format!(
                                            "{} entries chain to the signed head",
                                            repo.state.audit.len()
                                        ),
                                    ),
                                    Err(error) => ("FAIL", error),
                                }
                            }
                        };
                        cases.push(case(&config::serial(item), status, detail));
                    }
                }
                Config::Action(action) => {
                    let registered = validate::registered_operations(&repo.workspace.root)
                        .iter()
                        .any(|(adapter, operation)| {
                            *adapter == action.adapter && *operation == action.operation
                        });
                    cases.push(case(
                        "registered operation",
                        if registered { "PASS" } else { "FAIL" },
                        format!(
                            "{}/{}; a Brick never executes it, 'crane skill invoke' does",
                            action.adapter, action.operation
                        ),
                    ));
                }
                Config::Identity(identity) => {
                    for target in &on {
                        for assignment in repo.store.assignments.iter().filter(|assignment| {
                            assignment.targets.iter().any(|item| item.key() == *target)
                        }) {
                            let admitted = identity.agents.contains(&assignment.agent);
                            cases.push(case(
                                &format!("assignment {} on {target}", assignment.agent),
                                if admitted { "PASS" } else { "FAIL" },
                                if admitted {
                                    "admitted".to_string()
                                } else {
                                    "assigned but not admitted by this identity Brick".to_string()
                                },
                            ));
                        }
                    }
                    cases.push(case("enforcement", "UNKNOWN", "the guard does not decide per agent yet; identity is checked here and shown per assignment"));
                }
                Config::Monitoring(monitoring) => {
                    for signal in &monitoring.signals {
                        cases.push(case(
                        &config::serial(&signal.source),
                        if signal.source.supported() { "PASS" } else { "UNKNOWN" },
                        if signal.source.supported() {
                            format!("checked every {} minutes by the background sync, without a model", signal.every_minutes)
                        } else {
                            "no connector exists; reported as a coverage gap".to_string()
                        },
                    ));
                    }
                }
                Config::Context(context) => {
                    for file in &context.files {
                        let readable = repo.workspace.root.join(file).is_file();
                        cases.push(case(
                            file,
                            if readable { "PASS" } else { "FAIL" },
                            if readable { "readable" } else { "missing" },
                        ));
                    }
                }
                _ => cases.push(case("runtime", "DECLARED", info.runtime)),
            }
        }
    }
    let failed = cases.iter().any(|item| item["status"] == "FAIL");
    let blocked = cases.iter().any(|item| item["status"] == "BLOCKED");
    let unknown = cases
        .iter()
        .any(|item| matches!(item["status"].as_str(), Some("UNKNOWN" | "NOT_RUN")));
    let status = if failed {
        Status::Failed
    } else if blocked {
        Status::Blocked
    } else if unknown && info.support != Support::Enforced {
        Status::Unknown
    } else {
        Status::Passed
    };
    crate::platform::notes::note(
        "BRICK_TESTED",
        json!({"brick": brick.reference().to_string(), "type": brick.brick_type, "result": crate::cli::protocol::name(&status), "cases": cases.len()}),
    );
    let detail = cases
        .iter()
        .map(|item| {
            format!(
                "  [{}] {} — {}",
                item["status"].as_str().unwrap_or_default(),
                item["name"].as_str().unwrap_or_default(),
                item["detail"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.test",
        status,
        format!("{} {}: {} case(s)", brick.reference(), brick.brick_type.name(), cases.len()),
    )
    .reference("brick_ref", brick.reference().to_string())
    .data(json!({"cases": cases, "effects": effects, "support": info.support, "runtime": info.runtime}));
    finish(&args, response, &detail)
}

/** The case word of a check outcome
 * Input
    - outcome: CheckOutcome - outcome
 * Output
    - &'static str
*/
fn outcome_word(outcome: CheckOutcome) -> &'static str {
    match outcome {
        CheckOutcome::Proven => "PASS",
        CheckOutcome::Contradicted => "FAIL",
        CheckOutcome::Unknown => "UNKNOWN",
        CheckOutcome::NotRun => "NOT_RUN",
    }
}
