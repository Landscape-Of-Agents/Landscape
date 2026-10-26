// Helpers the family commands share: the repository context (the invocation context: registry,
// evidence, adapters, invocation ledger, shared skill store), skill and run references, the
// runbook store, the run budget ledger, and the orchestrator. They hold no rules of their own:
// every decision is made by the services they open.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::capability::read_json;
use super::invoke::{with_engine, Local};
use super::protocol::versioned;
use crate::capability::lifecycle::LifecycleState;
use crate::capability::registry::VersionRecord;
use crate::runbook::run::Orchestrator;
use crate::runbook::store::{RunRecord, RunbookStore};
use crate::telemetry::ledger::Ledger;

/** Resolve a skill reference ("ID", "ID@N") to a stored version: an exact version, or else the
 * latest published version, or else the latest version
 * Input
    - local: &Local - repository context
    - reference: &str - skill reference
 * Output
    - Result<&VersionRecord, String>
*/
pub(super) fn skill<'a>(local: &'a Local, reference: &str) -> Result<&'a VersionRecord, String> {
    let (id, version) = versioned(reference)?;
    let versions = local
        .context
        .registry
        .state
        .versions
        .get(&id)
        .ok_or_else(|| format!("skill {id} does not exist"))?;
    match version {
        Some(version) => versions
            .get(&version)
            .ok_or_else(|| format!("skill {id} v{version} does not exist")),
        None => versions
            .values()
            .rev()
            .find(|record| record.state == LifecycleState::Published)
            .or_else(|| versions.values().next_back())
            .ok_or_else(|| format!("skill {id} has no version")),
    }
}

/** The reference of a stored skill version
 * Input
    - record: &VersionRecord - version
 * Output
    - String "ID@N"
*/
pub(super) fn reference(record: &VersionRecord) -> String {
    format!(
        "{}@{}",
        record.definition.capability_id, record.definition.version
    )
}

/** Require an exact version in a reference
 * Input
    - reference: &str - "ID@N"
    - what: &str - what it is, for the error
 * Output
    - Result<(String, u32), String>
*/
pub(super) fn exact(reference: &str, what: &str) -> Result<(String, u32), String> {
    match versioned(reference)? {
        (id, Some(version)) => Ok((id, version)),
        (id, None) => Err(format!("{what} needs an exact version: {id}@N")),
    }
}

/** Read a JSON object of string values (inputs) from a file
 * Input
    - path: Option<String> - file, None for no inputs
 * Output
    - Result<BTreeMap<String, String>, String>
*/
pub(super) fn inputs(path: Option<String>) -> Result<BTreeMap<String, String>, String> {
    match path {
        None => Ok(BTreeMap::new()),
        Some(path) => serde_json::from_value(read_json(&path)?)
            .map_err(|error| format!("{path}: inputs must be an object of text values ({error})")),
    }
}

/** Split a comma-separated list
 * Input
    - value: Option<String> - list
 * Output
    - Vec<String>
*/
pub(super) fn list(value: Option<String>) -> Vec<String> {
    value
        .map(|ids| {
            ids.split(',')
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/** Open the runbook store of the repository
 * Input
    - local: &Local - repository context
 * Output
    - Result<RunbookStore, String>
*/
pub(super) fn runbooks(local: &Local) -> Result<RunbookStore, String> {
    RunbookStore::open(&local.context.runtime)
}

/** The run budget ledger of the repository
 * Input
    - local: &Local - repository context
 * Output
    - Ledger
*/
pub(super) fn budget(local: &Local) -> Ledger {
    Ledger::open(&local.context.runtime.join("budget"))
}

/** Run work with the orchestrator, then persist the runbook store
 * Input
    - local: &mut Local - repository context
    - store: &mut RunbookStore - runbook store
    - work: impl FnOnce(&mut Orchestrator) -> Result<RunRecord, String> - work
 * Output
    - Result<RunRecord, String>
*/
pub(super) fn orchestrate(
    local: &mut Local,
    store: &mut RunbookStore,
    work: impl FnOnce(&mut Orchestrator) -> Result<RunRecord, String>,
) -> Result<RunRecord, String> {
    let budget = budget(local);
    let actor = local.context.actor.clone();
    let result = with_engine(local, |engine| {
        let mut orchestrator = Orchestrator {
            engine,
            store,
            budget: &budget,
            actor,
            now: crate::platform::now_millis(),
        };
        work(&mut orchestrator)
    });
    store.save()?;
    result
}

/** Describe a run for output
 * Input
    - run: &RunRecord - run
 * Output
    - Value
*/
pub(super) fn run_view(run: &RunRecord) -> Value {
    json!({
        "run_id": run.run_id,
        "runbook": format!("{}@{}", run.runbook_id, run.version),
        "digest": run.digest,
        "state": run.state,
        "started_by": run.started_by.id,
        "steps": run.steps,
        "approvals": run.approvals,
        "log": run.log,
        "record_digest": run.record_digest,
    })
}

/** Human lines for a run's steps
 * Input
    - run: &RunRecord - run
 * Output
    - Vec<String>
*/
pub(super) fn run_lines(run: &RunRecord) -> Vec<String> {
    run.steps
        .iter()
        .map(|(id, step)| {
            format!(
                "  {id}: {} (attempts {}{}) - {}",
                super::protocol::name(&step.state),
                step.attempts,
                step.invocation_id
                    .as_deref()
                    .map(|id| format!(", invocation {id}"))
                    .unwrap_or_default(),
                step.detail
            )
        })
        .collect()
}

/** The command status an invocation state implies
 * Input
    - state: crate::invocation::lifecycle::InvocationState - invocation state
 * Output
    - super::protocol::Status
*/
pub(super) fn invocation_status(
    state: crate::invocation::lifecycle::InvocationState,
) -> super::protocol::Status {
    use super::protocol::Status;
    use crate::invocation::lifecycle::InvocationState as State;
    match state {
        State::Succeeded => Status::Passed,
        State::PlanReady | State::Requested | State::Authorizing | State::Preflighting => {
            Status::Ok
        }
        State::AwaitingApproval => Status::ApprovalRequired,
        State::Failed => Status::Failed,
        State::Blocked | State::Abandoned => Status::Blocked,
        _ => Status::Unknown,
    }
}

/** Describe an invocation for output
 * Input
    - record: &crate::invocation::engine::InvocationRecord - invocation
 * Output
    - Value
*/
pub(super) fn invocation_view(record: &crate::invocation::engine::InvocationRecord) -> Value {
    json!({
        "invocation_id": record.invocation_id,
        "skill": format!("{}@{}", record.capability_id, record.version),
        "digest": record.digest,
        "state": record.state,
        "requester": record.requester.id,
        "plan_digest": record.plan.as_ref().map(|plan| plan.plan_digest.clone()),
        "approval": record.approval,
        "approvals": record.approvals,
        "preflight": record.preflight,
        "operations": record.operations.iter().map(|op| json!({"step": op.step_id, "operation": format!("{}.{}", op.adapter_id, op.operation_id), "effect": op.effect, "status": op.status, "effect_status": op.effect_status, "attempts": op.attempts})).collect::<Vec<_>>(),
        "verifications": record.verifications,
        "unresolved_effects": record.unresolved_effects,
        "failures": record.failures,
        "record_digest": record.record_digest,
    })
}

/** The command status a run state implies
 * Input
    - state: crate::runbook::store::RunState - run state
 * Output
    - super::protocol::Status
*/
pub(super) fn run_status(state: crate::runbook::store::RunState) -> super::protocol::Status {
    use super::protocol::Status;
    use crate::runbook::store::RunState;
    match state {
        RunState::Succeeded => Status::Passed,
        RunState::Running => Status::Ok,
        RunState::AwaitingApproval => Status::ApprovalRequired,
        RunState::NeedsReconciliation => Status::Unknown,
        RunState::Partial | RunState::Failed => Status::Failed,
        RunState::Blocked | RunState::Cancelled => Status::Blocked,
    }
}

/** Check a runbook against the registry, adapters, and shared store: every pinned skill exists
 * with its pinned digest and is published (not withdrawn in the shared store), every bound
 * operation is registered, and every step can run under the typed-adapters profile
 * Input
    - local: &Local - repository context
    - runbook: &crate::runbook::model::Runbook - runbook
 * Output
    - Vec<Value> findings {code, message}
*/
pub(super) fn registry_findings(
    local: &Local,
    runbook: &crate::runbook::model::Runbook,
) -> Vec<Value> {
    use crate::capability::shared::{key, SharedState, SharedView};
    let mut findings = Vec::new();
    let mut push =
        |code: &str, message: String| findings.push(json!({"code": code, "message": message}));
    if let SharedView::Unverified(reason) = &local.shared {
        push(
            "shared_store_unverified",
            format!(
                "the shared skill store cannot be trusted ({reason}); every invocation is blocked"
            ),
        );
    }
    for step in &runbook.steps {
        let id = &step.step_id;
        let Ok(record) = local
            .context
            .registry
            .state
            .record(&step.skill_id, step.skill_version)
        else {
            push(
                "skill_missing",
                format!(
                    "step {id}: skill {}@{} does not exist on this machine",
                    step.skill_id, step.skill_version
                ),
            );
            continue;
        };
        if record.definition_digest != step.skill_digest {
            push(
                "skill_digest_changed",
                format!(
                    "step {id}: {}@{} has digest {}, not the pinned {}",
                    step.skill_id, step.skill_version, record.definition_digest, step.skill_digest
                ),
            );
        }
        if record.state != LifecycleState::Published {
            push(
                "skill_not_published",
                format!(
                    "step {id}: {}@{} is {:?}; only published skills run",
                    step.skill_id, step.skill_version, record.state
                ),
            );
        }
        if let SharedView::Verified(entries) = &local.shared {
            if let Some(entry) = entries.get(&key(&step.skill_id, step.skill_version)) {
                if entry.state != SharedState::Published {
                    push(
                        "skill_withdrawn",
                        format!(
                            "step {id}: the shared skill store records {}@{} as {:?}",
                            step.skill_id, step.skill_version, entry.state
                        ),
                    );
                }
            }
        }
        for binding in &step.bindings.steps {
            if local
                .adapters
                .operation(&binding.adapter_id, &binding.operation_id)
                .is_none()
            {
                push(
                    "operation_unregistered",
                    format!(
                        "step {id}: {}.{} is not a registered operation",
                        binding.adapter_id, binding.operation_id
                    ),
                );
            }
        }
        for problem in crate::invocation::isolation::check(
            &record.definition,
            &local.adapters,
            "typed-adapters",
        ) {
            push("isolation_unsupported", format!("step {id}: {problem}"));
        }
    }
    findings
}
