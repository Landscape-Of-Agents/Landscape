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
            if brick.brick_type.is_legacy() {
                cases.push(case(
                    "former kind",
                    "UNKNOWN",
                    format!(
                        "read as {}; 'crane brick migrate' rewrites it",
                        parsed.iter().map(kind_of).collect::<Vec<_>>().join(" and ")
                    ),
                ));
            }
            for config in &parsed {
                test_config(&repo, &brick, config, &on, &mut cases, &mut effects)?;
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

/** The kind a configuration belongs to, such as "access"
 * Input
    - config: &Config - configuration
 * Output
    - &'static str
*/
fn kind_of(config: &Config) -> &'static str {
    match config {
        Config::Knowledge(_) => "knowledge",
        Config::Identity(_) => "identity",
        Config::Access(_) => "access",
        Config::Approval(_) => "approval",
        Config::Limits(_) => "limits",
        Config::Environment(_) => "environment",
        Config::Precondition(_) => "precondition",
        Config::Verification(_) => "verification",
        Config::Action(_) => "action",
        Config::Recovery(_) => "recovery",
        Config::Provenance(_) => "provenance",
        Config::Watch(_) => "watch",
    }
}

/** The test cases of one configuration
 * Input
    - repo: &Repo - repository
    - brick: &Brick - the Brick tested
    - config: &Config - one of its configurations
    - on: &[String] - targets to simulate on
    - cases: &mut Vec<Value> - cases found
    - effects: &mut Vec<Value> - enforcement previews
 * Output
    - Result<(), String>
*/
fn test_config(
    repo: &Repo,
    brick: &Brick,
    config: &Config,
    on: &[String],
    cases: &mut Vec<Value>,
    effects: &mut Vec<Value>,
) -> Result<(), String> {
    let per_call = matches!(
        config,
        Config::Identity(_)
            | Config::Approval(_)
            | Config::Environment(_)
            | Config::Recovery(_)
            | Config::Watch(_)
    ) || matches!(config, Config::Limits(limits) if limits.max_ceiling.is_none() && limits.min_criticality.is_none())
        || matches!(config, Config::Action(action) if action.deny_direct);
    let per_file = matches!(
        config,
        Config::Access(_) | Config::Limits(_) | Config::Precondition(_) | Config::Environment(_)
    );
    if per_file || per_call {
        if on.is_empty() {
            cases.push(case(
                "enforcement",
                "NOT_RUN",
                "give --target flow:ID or zone:ID to simulate it on real files",
            ));
        }
        for target in on {
            let preview = simulate(repo, brick, target)?;
            for gap in preview["controls"]["gaps"].as_array().into_iter().flatten() {
                cases.push(case(
                    "gap",
                    "UNKNOWN",
                    gap["message"].as_str().unwrap_or_default(),
                ));
            }
            if per_file {
                let restricted = preview["files"].as_array().map_or(0, Vec::len);
                let resolved = preview["scope_resolved"].as_bool().unwrap_or(true);
                cases.push(case(
                    &format!("per-file limits on {target}"),
                    if resolved { "PASS" } else { "FAIL" },
                    if resolved {
                        format!("{restricted} file(s) of {target} restricted by the guard")
                    } else {
                        "the scope matches no file of the target, so the whole target becomes observe-only (fail closed)".to_string()
                    },
                ));
            }
            if per_call {
                cases.push(case(
                    &format!("per-call rules on {target}"),
                    "PASS",
                    format!(
                        "decided before every tool call touching {target}: {}",
                        brick.brick_type.catalog().runtime
                    ),
                ));
            }
            effects.push(preview);
        }
    }
    match config {
        Config::Precondition(precondition) => {
            for (check, outcome, detail) in run_checks(
                &repo.workspace,
                &precondition.checks,
                on.first().map(String::as_str),
                true,
            ) {
                let holds = outcome == CheckOutcome::Proven;
                cases.push(case(
                    &format!("gate: {check}"),
                    if holds { "PASS" } else { "UNKNOWN" },
                    if holds {
                        "holds now".to_string()
                    } else {
                        format!(
                            "failing now: changes would {} ({detail})",
                            if precondition.on_fail == config::Mode::Deny {
                                "stop"
                            } else {
                                "need a person"
                            }
                        )
                    },
                ));
            }
        }
        Config::Verification(verification) => {
            let mut checks = verification.checks.clone();
            if let Some(source) = &verification.checks_from {
                let shared = repo
                    .store
                    .bricks
                    .iter()
                    .filter(|entry| {
                        entry.brick.brick_id == *source && entry.state == VersionState::Active
                    })
                    .filter_map(|entry| {
                        config::parse(entry.brick.brick_type, &entry.brick.config).ok()
                    })
                    .flatten()
                    .filter_map(|parsed| match parsed {
                        Config::Precondition(precondition) => Some(precondition.checks),
                        _ => None,
                    })
                    .next_back();
                match shared {
                    Some(found) => checks.extend(found),
                    None => cases.push(case(
                        "checks_from",
                        "FAIL",
                        format!("'{source}' is not an active precondition Brick"),
                    )),
                }
            }
            for (check, outcome, detail) in run_checks(
                &repo.workspace,
                &checks,
                on.first().map(String::as_str),
                true,
            ) {
                cases.push(case(&check, outcome_word(outcome), detail));
            }
        }
        Config::Provenance(provenance) => {
            for item in &provenance.require {
                let (status, detail) = match item {
                    ProvenanceItem::SignedState => {
                        let outcome =
                            run_checks(&repo.workspace, &[config::Check::SignedState], None, true);
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
                    ProvenanceItem::DecisionsRecorded
                    | ProvenanceItem::CommitTrailer
                    | ProvenanceItem::LinkedIssue => provenance_case(repo, provenance, *item, on),
                };
                cases.push(case(&config::serial(item), status, detail));
            }
        }
        Config::Action(action) => {
            let registered = validate::registered_operations(&repo.workspace.root);
            for rule in action.rules() {
                let known = registered.iter().any(|(adapter, operation)| {
                    *adapter == rule.adapter && *operation == rule.operation
                });
                let direct = crate::brick::guard::DIRECT
                    .iter()
                    .any(|(adapter, operation, _)| {
                        *adapter == rule.adapter && *operation == rule.operation
                    });
                cases.push(case(
                    &format!("{}/{}", rule.adapter, rule.operation),
                    if known || direct { "PASS" } else { "FAIL" },
                    if known {
                        "a registered operation Crane runs ('crane skill invoke', resident steps)"
                            .to_string()
                    } else if direct {
                        format!(
                            "a direct operation; {}",
                            if action.deny_direct {
                                "agents may not do it themselves"
                            } else {
                                "set deny_direct to keep agents from doing it themselves"
                            }
                        )
                    } else {
                        "neither registered nor a known direct operation".to_string()
                    },
                ));
            }
        }
        Config::Identity(identity) => {
            for target in on {
                for assignment in repo.store.assignments.iter().filter(|assignment| {
                    assignment.targets.iter().any(|item| item.key() == *target)
                }) {
                    let admitted = identity
                        .agents
                        .iter()
                        .any(|agent| agent.eq_ignore_ascii_case(&assignment.agent));
                    cases.push(case(
                        &format!("assignment {} on {target}", assignment.agent),
                        if admitted { "PASS" } else { "FAIL" },
                        if admitted { "admitted".to_string() } else { "assigned but not admitted by this identity Brick; its tool calls would be refused".to_string() },
                    ));
                }
            }
        }
        Config::Environment(environment) => {
            for credential in &environment.credentials {
                let present = crate::trust::vault::exists(&repo.workspace, &credential.name);
                cases.push(case(
                    &format!("credential {}", credential.name),
                    if present { "PASS" } else { "UNKNOWN" },
                    if present {
                        "in this machine's vault".to_string()
                    } else {
                        format!(
                            "not in this machine's vault; a person runs 'crane secret set {}'",
                            credential.name
                        )
                    },
                ));
            }
            for name in &environment.require_env {
                let set = std::env::var_os(name).is_some_and(|value| !value.is_empty());
                cases.push(case(
                    &format!("variable {name}"),
                    if set { "PASS" } else { "UNKNOWN" },
                    if set {
                        "set here"
                    } else {
                        "not set here; changes would need a person"
                    },
                ));
            }
        }
        Config::Watch(watch) => {
            for signal in &watch.signals {
                cases.push(case(
                    &config::serial(&signal.source),
                    if signal.source.supported() {
                        "PASS"
                    } else {
                        "UNKNOWN"
                    },
                    if signal.source.supported() {
                        format!(
                            "checked every {} minutes by the background sync, without a model",
                            signal.every_minutes
                        )
                    } else {
                        "no connector exists; reported as a coverage gap".to_string()
                    },
                ));
            }
        }
        Config::Knowledge(knowledge) => {
            for file in &knowledge.files {
                let readable = repo.workspace.root.join(file).is_file();
                cases.push(case(
                    file,
                    if readable { "PASS" } else { "FAIL" },
                    if readable { "readable" } else { "missing" },
                ));
            }
        }
        _ => {}
    }
    if !per_file
        && !per_call
        && !matches!(
            config,
            Config::Precondition(_)
                | Config::Verification(_)
                | Config::Provenance(_)
                | Config::Action(_)
                | Config::Identity(_)
                | Config::Watch(_)
                | Config::Knowledge(_)
        )
    {
        cases.push(case(
            "runtime",
            "DECLARED",
            brick.brick_type.catalog().runtime,
        ));
    }
    Ok(())
}

/** Check a lineage requirement against the recent commits touching a target's files
 * Input
    - repo: &Repo - repository
    - provenance: &config::ProvenanceConfig - the requirement
    - item: ProvenanceItem - DecisionsRecorded, CommitTrailer, or LinkedIssue
    - on: &[String] - targets
 * Output
    - (&'static str, String) status and detail
*/
fn provenance_case(
    repo: &Repo,
    provenance: &config::ProvenanceConfig,
    item: ProvenanceItem,
    on: &[String],
) -> (&'static str, String) {
    let Some(target) = on.first() else {
        return (
            "NOT_RUN",
            "give --target flow:ID or zone:ID to check its commits".into(),
        );
    };
    let Ok(all) = targets(repo) else {
        return ("UNKNOWN", "the targets could not be resolved".into());
    };
    let Some(info) = all.get(target) else {
        return ("UNKNOWN", format!("{target} is not a Zone or Flow"));
    };
    // A shared scope narrows the files whose commits are checked
    let scope = provenance.scope_from.as_ref().and_then(|id| {
        let (configs, _) = crate::brick::compose::configs_of(&repo.store, target);
        configs
            .into_iter()
            .find_map(|(_, brick_id, config)| match config {
                Config::Access(access) if brick_id == *id => Some(crate::brick::compose::PathSet {
                    include: access.include,
                    exclude: access.exclude,
                }),
                _ => None,
            })
    });
    let files = info
        .files
        .iter()
        .filter(|file| scope.as_ref().is_none_or(|scope| scope.contains(file)))
        .cloned()
        .collect::<Vec<_>>();
    if files.is_empty() {
        return ("UNKNOWN", format!("no file of {target} is in scope"));
    }
    if item == ProvenanceItem::DecisionsRecorded {
        let Ok(trust) = repo.workspace.trust() else {
            return ("UNKNOWN", "the trust directory is not available".into());
        };
        let events = crate::telemetry::Stores::open(&trust.runtime())
            .events
            .read_all()
            .unwrap_or_default();
        let decided = events
            .iter()
            .filter(|event| {
                event.decision.is_some() && event.bindings.zones.iter().any(|zone| zone == target)
            })
            .count();
        return if decided > 0 {
            (
                "PASS",
                format!("{decided} recorded decision(s) bound to {target}"),
            )
        } else {
            (
                "UNKNOWN",
                format!("no recorded decision is bound to {target} on this machine"),
            )
        };
    }
    let mut command = vec![
        "log".to_string(),
        format!("-n{}", provenance.commits),
        "--format=%H%x1f%B%x1e".to_string(),
        "--".to_string(),
    ];
    command.extend(files.iter().take(200).cloned());
    let words = command.iter().map(String::as_str).collect::<Vec<_>>();
    let Ok(log) = crate::platform::git::git_in(&repo.workspace.root, &words) else {
        return ("UNKNOWN", "git log could not run".into());
    };
    let commits = log
        .split('\u{1e}')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| entry.split_once('\u{1f}'))
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return ("UNKNOWN", format!("no commit touches {target} yet"));
    }
    let missing = commits
        .iter()
        .filter(|(_, message)| match item {
            ProvenanceItem::CommitTrailer => {
                let key = provenance
                    .trailer
                    .clone()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                !message
                    .lines()
                    .any(|line| line.to_ascii_lowercase().starts_with(&format!("{key}:")))
            }
            _ => {
                let pattern = provenance.issue_pattern.clone().unwrap_or_default();
                !message
                    .split(|c: char| c.is_whitespace() || "()[],;:".contains(c))
                    .any(|word| crate::platform::glob::matches(&pattern, word))
            }
        })
        .map(|(hash, _)| hash.chars().take(10).collect::<String>())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        (
            "PASS",
            format!(
                "all {} recent commit(s) touching {target} comply",
                commits.len()
            ),
        )
    } else {
        (
            "FAIL",
            format!(
                "{} of {} recent commit(s) touching {target} do not: {}",
                missing.len(),
                commits.len(),
                missing.join(", ")
            ),
        )
    }
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
