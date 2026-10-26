// crane runbook test: what a runbook must guarantee, checked without running anything: it
// validates (structure and registry); every failing or blocked step has a defined outcome (the run
// stops, or a step branches on it); the budget covers every operation including retries; and the
// orchestrator refuses a draft or revoked version, missing inputs, and secret inputs. Each refusal
// is checked on an in-memory copy of the store, before any invocation could start.

use std::collections::BTreeMap;

use serde_json::json;

use crate::cli::family::{registry_findings, runbooks};
use crate::cli::invoke::{with_engine, Local};
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::runbook::model;
use crate::runbook::run::Orchestrator;
use crate::runbook::store::{RunbookState, RunbookStore};

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane runbook test RUNBOOK[@N]")?;
    let (id, version) = versioned(&args.positional[0])?;
    let mut local = Local::open()?;
    let store = runbooks(&local)?;
    let record = store.runbook(&id, version)?.clone();
    let definition = record.definition.clone();
    let mut cases = Vec::new();
    let mut case = |name: &str, passed: bool, detail: String| {
        cases.push(
            json!({"case": name, "status": if passed { "PASS" } else { "FAIL" }, "detail": detail}),
        )
    };
    let mut findings = model::validate(&definition);
    findings.extend(registry_findings(&local, &definition));
    case(
        "validates",
        findings.is_empty(),
        findings
            .iter()
            .filter_map(|finding| finding["message"].as_str())
            .collect::<Vec<_>>()
            .join("; "),
    );
    for step in &definition.steps {
        let branched = definition.steps.iter().any(|other| {
            other
                .when
                .as_ref()
                .is_some_and(|when| when.step == step.step_id)
        });
        case(
            &format!("{} failure has a defined outcome", step.step_id),
            step.on_failure == model::FailurePolicy::Stop
                || branched
                || definition
                    .steps
                    .iter()
                    .all(|other| !other.depends_on.contains(&step.step_id)),
            format!(
                "on failure {:?}{}",
                step.on_failure,
                if branched {
                    ", and a step branches on it"
                } else {
                    ""
                }
            )
            .to_lowercase(),
        );
    }
    let total: u64 = definition
        .steps
        .iter()
        .map(|step| step.cost() * (1 + u64::from(step.max_retries)))
        .sum();
    case(
        "budget covers every operation and retry",
        total <= definition.budget.operations,
        format!(
            "{total} planned of {} budgeted",
            definition.budget.operations
        ),
    );
    // Refusals, on in-memory copies: none of them reaches an invocation
    let refusal = |local: &mut Local,
                   state: Option<RunbookState>,
                   inputs: BTreeMap<String, String>|
     -> Result<String, String> {
        let mut copy = RunbookStore::in_memory();
        let mut definition = definition.clone();
        definition.version = 1;
        copy.create(definition, &record.created_by, 0)?;
        if let Some(state) = state {
            copy.transition(
                &id,
                1,
                RunbookState::Active,
                &local.context.actor,
                "test",
                0,
            )?;
            if state != RunbookState::Active {
                copy.transition(&id, 1, state, &local.context.actor, "test", 0)?;
            }
        }
        let directory =
            std::env::temp_dir().join(format!("crane-runbook-test-{}", std::process::id()));
        let budget = crate::telemetry::ledger::Ledger::open(&directory);
        let actor = local.context.actor.clone();
        let result = with_engine(local, |engine| {
            let mut orchestrator = Orchestrator {
                engine,
                store: &mut copy,
                budget: &budget,
                actor,
                now: 0,
            };
            orchestrator
                .start(&id, 1, inputs, Vec::new(), "runbook-test")
                .map(|run| format!("{:?}", run.state))
        });
        let _ = std::fs::remove_dir_all(&directory);
        match result {
            Ok(state) => Err(format!("a run started ({state})")),
            Err(error) => Ok(error),
        }
    };
    let full = definition
        .inputs
        .iter()
        .map(|name| (name.clone(), "x".to_string()))
        .collect::<BTreeMap<_, _>>();
    for (name, state, inputs) in [
        ("a draft cannot run", None, full.clone()),
        (
            "a revoked version cannot run",
            Some(RunbookState::Revoked),
            full.clone(),
        ),
        (
            "a retired version cannot run",
            Some(RunbookState::Retired),
            full.clone(),
        ),
    ] {
        let outcome = refusal(&mut local, state, inputs);
        case(name, outcome.is_ok(), outcome.unwrap_or_else(|error| error));
    }
    if let Some(first) = definition.inputs.first() {
        let mut missing = full.clone();
        missing.remove(first);
        let outcome = refusal(&mut local, Some(RunbookState::Active), missing);
        case(
            "a missing input is refused",
            outcome
                .as_ref()
                .is_ok_and(|error| error.contains("required")),
            outcome.unwrap_or_else(|error| error),
        );
        let mut secret = full.clone();
        secret.insert(
            first.clone(),
            "ghp_abcdefghijklmnopqrstuvwxyz0123456789".into(),
        );
        let outcome = refusal(&mut local, Some(RunbookState::Active), secret);
        case(
            "a secret input is refused",
            outcome.as_ref().is_ok_and(|error| error.contains("secret")),
            outcome.unwrap_or_else(|error| error),
        );
    }
    let failed = cases.iter().filter(|case| case["status"] == "FAIL").count();
    let detail = cases
        .iter()
        .map(|case| {
            format!(
                "  {:<5} {} ({})",
                case["status"].as_str().unwrap_or_default(),
                case["case"].as_str().unwrap_or_default(),
                case["detail"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "runbook.test",
        if failed == 0 {
            Status::Passed
        } else {
            Status::Failed
        },
        format!(
            "runbook {id}@{}: {} cases, {failed} failed; nothing was executed",
            definition.version,
            cases.len()
        ),
    )
    .reference("runbook_ref", format!("{id}@{}", definition.version))
    .reference("digest", record.digest.clone())
    .data(json!({"cases": cases}));
    finish(&args, response, &detail)
}
