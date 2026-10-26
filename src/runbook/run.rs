// The runbook orchestrator. A run walks its steps in dependency order; every step is one invocation
// of an exact published skill version through the invocation engine, so each step gets its own
// execution identity and the engine's full authorization, preflight, approval, per-operation
// checks, reconciliation, and verification. The runbook adds nothing to a skill's authority: it
// only decides order, branches, approval points, budget, and the run's status.
//
// Before a step starts: the runbook version must still be active (a revocation stops the run at its
// next step), its approval point must be approved by a person other than the one who started the
// run (bound to the step's approval digest), and its operations must fit the run budget: they are
// reserved atomically in runtime/budget/ledger.jsonl under the account "run:RUN_ID" and settled from
// the invocation's own record (operations that started are consumed; the rest is released; an
// uncertain outcome consumes everything until it is reconciled).
//
// Outcomes: succeeded (verified postconditions), failed, blocked (before any effect), waiting for a
// skill approval (the run pauses), or uncertain (the run pauses for reconciliation; nothing is
// retried). A retry happens only when a person resumes from a step that was blocked or failed
// without any effect, within the step's retry limit, with a new invocation identity. The run's final
// status comes from the verified outcomes of its required steps, never from anyone's claim.

use std::collections::BTreeMap;

use serde_json::Value;

use super::model::{FailurePolicy, InputSource, Outcome, Runbook, Step};
use super::store::{
    RunRecord, RunState, RunbookState, RunbookStore, StepApproval, StepRun, StepState,
};
use crate::capability::Role;
use crate::capture::{stable_id, Actor, ActorKind};
use crate::invocation::engine::{Engine, InvocationRecord, InvocationRequest};
use crate::invocation::lifecycle::{EffectStatus, InvocationState, OperationStatus};
use crate::invocation::plan::PlanLimits;
use crate::telemetry::ledger::{Kind, Ledger, Operation as LedgerOperation};

/** The budget account of a run
 * Input
    - run_id: &str - run
 * Output
    - String
*/
pub(crate) fn account(run_id: &str) -> String {
    format!("run:{run_id}")
}

/** Everything a run needs
 * Fields
    - engine: &mut Engine - the invocation engine (registry, adapters, ledger, events)
    - store: &mut RunbookStore - runbooks and runs
    - budget: &Ledger - the run budget ledger
    - actor: Actor - the person acting
    - now: u64 - Unix milliseconds
*/
pub(crate) struct Orchestrator<'e, 'a> {
    pub(crate) engine: &'e mut Engine<'a>,
    pub(crate) store: &'e mut RunbookStore,
    pub(crate) budget: &'e Ledger,
    pub(crate) actor: Actor,
    pub(crate) now: u64,
}

/** Map an engine error to a message
 * Input
    - error: crate::capability::CapabilityError - error
 * Output
    - String
*/
fn message(error: crate::capability::CapabilityError) -> String {
    error.to_string()
}

impl Orchestrator<'_, '_> {
    /** Apply a budget ledger operation
     * Input
        - run_id: &str - run
        - kind: Kind - operation
        - amount: u64 - amount
        - reason: &str - why
        - key: &str - idempotency key
     * Output
        - Result<crate::telemetry::ledger::Entry, String>
    */
    fn ledger(
        &self,
        run_id: &str,
        kind: Kind,
        amount: u64,
        reason: &str,
        key: &str,
    ) -> Result<crate::telemetry::ledger::Entry, String> {
        let account = account(run_id);
        self.budget
            .apply(LedgerOperation {
                session: &account,
                task: None,
                tool_call_id: None,
                kind,
                amount,
                reason,
                registry_generation: 0,
                idempotency_key: key,
            })
            .map(|(entry, _)| entry)
    }

    /** The available budget of a run
     * Input
        - run_id: &str - run
     * Output
        - Result<u64, String>
    */
    pub(crate) fn available(&self, run_id: &str) -> Result<u64, String> {
        Ok(Ledger::balance_of(&self.budget.entries()?, &account(run_id)).available)
    }

    /** Start a run of an active runbook version and advance it as far as it can go
     * Input
        - runbook_id: &str - runbook
        - version: u32 - exact version
        - inputs: BTreeMap<String, String> - run inputs
        - confirmed: Vec<String> - preconditions the starter confirms
        - key: &str - idempotency key (a repeated key returns the existing run)
     * Output
        - Result<RunRecord, String>
    */
    pub(crate) fn start(
        &mut self,
        runbook_id: &str,
        version: u32,
        inputs: BTreeMap<String, String>,
        confirmed: Vec<String>,
        key: &str,
    ) -> Result<RunRecord, String> {
        let record = self.store.runbook(runbook_id, Some(version))?.clone();
        let run_id = stable_id("rbr", &[runbook_id, &version.to_string(), key]);
        if self.store.state.runs.contains_key(&run_id) {
            return self.store.run(&run_id).cloned();
        }
        if record.state != RunbookState::Active {
            return Err(format!(
                "runbook {runbook_id} v{version} is {:?}; only an active version runs",
                record.state
            ));
        }
        if let Some(missing) = record
            .definition
            .inputs
            .iter()
            .find(|name| !inputs.contains_key(*name))
        {
            return Err(format!("run input '{missing}' is required"));
        }
        if let Some((name, _)) = inputs
            .iter()
            .find(|(_, value)| crate::telemetry::redact::redact(value).1 > 0)
        {
            return Err(format!("run input '{name}' holds a secret; pass a credential reference, never a secret value"));
        }
        let run = RunRecord {
            run_id: run_id.clone(),
            runbook_id: runbook_id.into(),
            version,
            digest: record.digest.clone(),
            started_by: self.actor.clone(),
            inputs,
            confirmed,
            idempotency_key: key.into(),
            state: RunState::Running,
            steps: record
                .definition
                .steps
                .iter()
                .map(|step| {
                    (
                        step.step_id.clone(),
                        StepRun {
                            state: StepState::Pending,
                            attempts: 0,
                            invocation_id: None,
                            reservation: None,
                            detail: "not started".into(),
                        },
                    )
                })
                .collect(),
            approvals: Vec::new(),
            log: vec![format!("started by {}", self.actor.id)],
            created_at: self.now,
            updated_at: self.now,
            record_digest: String::new(),
        };
        self.ledger(
            &run_id,
            Kind::Grant,
            record.definition.budget.operations,
            "run budget",
            &format!("{run_id}:grant"),
        )?;
        self.store.put_run(run);
        self.advance(&run_id)
    }

    /** Resolve a step's inputs
     * Input
        - run: &RunRecord - run
        - step: &Step - step
     * Output
        - Result<BTreeMap<String, String>, String>
    */
    fn inputs(&self, run: &RunRecord, step: &Step) -> Result<BTreeMap<String, String>, String> {
        let mut resolved = BTreeMap::new();
        for (name, source) in &step.inputs {
            let value = match source {
                InputSource::Literal { value } => value.clone(),
                InputSource::RunInput { name: input } => run
                    .inputs
                    .get(input)
                    .cloned()
                    .ok_or_else(|| format!("run input {input} is missing"))?,
                InputSource::StepOutput {
                    step: from,
                    operation,
                    field,
                } => {
                    let invocation = run
                        .steps
                        .get(from)
                        .and_then(|state| state.invocation_id.clone())
                        .ok_or_else(|| format!("step {from} has no result"))?;
                    let record = self.engine.ledger.get(&invocation).map_err(message)?;
                    record
                        .operations
                        .iter()
                        .filter(|op| &op.operation_id == operation)
                        .find_map(|op| lookup(&op.result, field))
                        .ok_or_else(|| {
                            format!("step {from} produced no {operation} field {field}")
                        })?
                }
            };
            resolved.insert(name.clone(), value);
        }
        Ok(resolved)
    }

    /** Settle a step's reservation from its invocation record
     * Input
        - run_id: &str - run
        - step_id: &str - step
        - attempt: u32 - attempt
        - record: &InvocationRecord - invocation
        - reserved: u64 - amount reserved
     * Output
        - Result<(), String>
    */
    fn settle(
        &self,
        run_id: &str,
        step_id: &str,
        attempt: u32,
        record: &InvocationRecord,
        reserved: u64,
    ) -> Result<(), String> {
        let uncertain = matches!(
            record.state,
            InvocationState::ReconciliationRequired
                | InvocationState::Unknown
                | InvocationState::PartiallyVerified
        );
        let started = record
            .operations
            .iter()
            .filter(|op| {
                !matches!(
                    op.status,
                    OperationStatus::Pending | OperationStatus::NotStarted
                )
            })
            .count() as u64;
        let consume = if uncertain {
            reserved
        } else {
            started.min(reserved)
        };
        let prefix = format!("{run_id}:{step_id}:{attempt}");
        if consume > 0 {
            self.ledger(
                run_id,
                Kind::Consume,
                consume,
                &format!("step {step_id}: operations that started"),
                &format!("{prefix}:consume"),
            )?;
        }
        if reserved > consume {
            self.ledger(
                run_id,
                Kind::Release,
                reserved - consume,
                &format!("step {step_id}: operations that never started"),
                &format!("{prefix}:release"),
            )?;
        }
        Ok(())
    }

    /** Map an invocation to a step state
     * Input
        - record: &InvocationRecord - invocation
     * Output
        - (StepState, String) state and detail
    */
    fn state_of(record: &InvocationRecord) -> (StepState, String) {
        let reason = record.failures.join("; ");
        match record.state {
            InvocationState::Succeeded => (StepState::Succeeded, "verified postconditions".into()),
            InvocationState::AwaitingApproval => (
                StepState::AwaitingSkillApproval,
                format!(
                    "invocation {} waits for the skill's approval",
                    record.invocation_id
                ),
            ),
            InvocationState::Blocked => (
                StepState::Blocked,
                format!("blocked before any effect: {reason}"),
            ),
            InvocationState::Failed => (StepState::Failed, format!("failed: {reason}")),
            InvocationState::Abandoned => (StepState::Cancelled, "abandoned".into()),
            InvocationState::PlanReady => (
                StepState::AwaitingSkillApproval,
                "planned, not executed".into(),
            ),
            _ => (
                StepState::NeedsReconciliation,
                format!(
                    "invocation {} is {:?}: reconcile it before anything else",
                    record.invocation_id, record.state
                ),
            ),
        }
    }

    /** Advance a run: start every step whose turn it is, until the run pauses or ends
     * Input
        - run_id: &str - run
     * Output
        - Result<RunRecord, String>
    */
    pub(crate) fn advance(&mut self, run_id: &str) -> Result<RunRecord, String> {
        let mut run = self.store.run(run_id)?.clone();
        if run.state.ended() {
            return Ok(run);
        }
        let record = self
            .store
            .runbook(&run.runbook_id, Some(run.version))?
            .clone();
        let runbook = record.definition.clone();
        let order = runbook.order()?;
        run.state = RunState::Running;
        let mut stopped = false;
        for step_id in &order {
            let step = runbook.step(step_id).ok_or("unknown step")?.clone();
            let current = run.steps[step_id].clone();
            if current.state.finished() || current.state == StepState::NeedsReconciliation {
                continue;
            }
            if matches!(current.state, StepState::AwaitingSkillApproval) {
                let invocation = current.invocation_id.clone().unwrap_or_default();
                let existing = self.engine.ledger.get(&invocation).map_err(message)?;
                let planned = existing
                    .plan
                    .as_ref()
                    .map(|plan| plan.plan_digest.clone())
                    .unwrap_or_default();
                let approved = existing
                    .approvals
                    .iter()
                    .any(|approval| approval.plan_digest == planned && approval.revoked.is_none());
                if !approved {
                    run.state = RunState::AwaitingApproval;
                    stopped = true;
                    break;
                }
                let done = self.engine.run(&self.actor, &invocation).map_err(message)?;
                self.finish_step(&mut run, &step, &done)?;
                if self.should_stop(&run, &runbook, &step) {
                    stopped = true;
                    break;
                }
                continue;
            }
            // Dependencies and branches
            let outcomes = step
                .depends_on
                .iter()
                .map(|dependency| run.steps[dependency].state)
                .collect::<Vec<_>>();
            if outcomes.iter().any(|state| !state.finished()) {
                continue;
            }
            let applies = match &step.when {
                Some(when) => {
                    let wanted = match when.outcome {
                        Outcome::Succeeded => StepState::Succeeded,
                        Outcome::Failed => StepState::Failed,
                        Outcome::Blocked => StepState::Blocked,
                    };
                    run.steps[&when.step].state == wanted
                        && step
                            .depends_on
                            .iter()
                            .filter(|dependency| **dependency != when.step)
                            .all(|dependency| run.steps[dependency].state == StepState::Succeeded)
                }
                None => outcomes.iter().all(|state| *state == StepState::Succeeded),
            };
            if !applies {
                let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                entry.state = StepState::Skipped;
                entry.detail = if step.when.is_some() {
                    "its branch condition did not hold".into()
                } else {
                    "a dependency did not succeed".into()
                };
                continue;
            }
            // The runbook version must still be active
            let state = self
                .store
                .runbook(&run.runbook_id, Some(run.version))?
                .state;
            if state != RunbookState::Active {
                let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                entry.state = StepState::Blocked;
                entry.detail = format!("the runbook version is {state:?}");
                run.log
                    .push(format!("{step_id}: blocked, runbook {state:?}"));
                stopped = true;
                break;
            }
            // Approval point
            if step.approval {
                let digest = run.approval_digest(&step);
                let approved = run.approvals.iter().any(|approval| {
                    approval.step == *step_id
                        && approval.digest == digest
                        && approval.revoked.is_none()
                });
                if !approved {
                    let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                    entry.state = StepState::AwaitingApproval;
                    entry.detail = format!(
                        "waits for a person to approve {}:{step_id}@{digest}",
                        run.run_id
                    );
                    run.state = RunState::AwaitingApproval;
                    stopped = true;
                    break;
                }
            }
            // Budget
            let cost = step.cost();
            let attempt = current.attempts + 1;
            if self.available(&run.run_id)? < cost {
                let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                entry.state = StepState::Blocked;
                entry.detail = format!("the run budget cannot cover its {cost} operations");
                run.log
                    .push(format!("{step_id}: blocked, budget exhausted"));
                stopped = true;
                break;
            }
            let reservation = self.ledger(
                &run.run_id,
                Kind::Reserve,
                cost,
                &format!("step {step_id} attempt {attempt}"),
                &format!("{}:{step_id}:{attempt}:reserve", run.run_id),
            )?;
            // The ledger clamps a reservation to what is left; a concurrent run may have taken part
            if reservation.amount < cost {
                if reservation.amount > 0 {
                    self.ledger(
                        &run.run_id,
                        Kind::Release,
                        reservation.amount,
                        "reservation short",
                        &format!("{}:{step_id}:{attempt}:release-short", run.run_id),
                    )?;
                }
                let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                entry.state = StepState::Blocked;
                entry.detail = format!(
                    "only {} of its {cost} operations could be reserved",
                    reservation.amount
                );
                run.log
                    .push(format!("{step_id}: blocked, budget taken concurrently"));
                stopped = true;
                break;
            }
            let inputs = match self.inputs(&run, &step) {
                Ok(inputs) => inputs,
                Err(problem) => {
                    self.ledger(
                        &run.run_id,
                        Kind::Release,
                        cost,
                        "inputs unresolved",
                        &format!("{}:{step_id}:{attempt}:release", run.run_id),
                    )?;
                    let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                    entry.state = StepState::Blocked;
                    entry.detail = problem;
                    stopped = self.should_stop(&run, &runbook, &step);
                    if stopped {
                        break;
                    }
                    continue;
                }
            };
            let request = InvocationRequest {
                capability_id: step.skill_id.clone(),
                version: step.skill_version,
                digest: step.skill_digest.clone(),
                task_id: Some(run.run_id.clone()),
                episode_id: None,
                environment: runbook.environment.clone(),
                repository: runbook.repository.clone(),
                confirmed_preconditions: step
                    .preconditions
                    .iter()
                    .filter(|id| run.confirmed.contains(id))
                    .cloned()
                    .collect(),
                inputs,
                bindings: step.bindings.clone(),
                discovery_report: None,
                compilation_report: None,
                idempotency_key: format!("{}:{step_id}:{attempt}", run.run_id),
                execute: true,
                limits: PlanLimits::default(),
            };
            {
                let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
                entry.attempts = attempt;
                entry.reservation = Some(reservation.entry_id.clone());
            }
            let invocation = self
                .engine
                .submit(&run.started_by, request)
                .map_err(message)?;
            run.steps
                .get_mut(step_id)
                .ok_or("unknown step")?
                .invocation_id = Some(invocation.invocation_id.clone());
            self.finish_step(&mut run, &step, &invocation)?;
            if matches!(
                run.steps[step_id].state,
                StepState::AwaitingSkillApproval | StepState::NeedsReconciliation
            ) || self.should_stop(&run, &runbook, &step)
            {
                stopped = true;
                break;
            }
        }
        let failed_stop = stopped
            && run
                .steps
                .values()
                .any(|step| matches!(step.state, StepState::Failed | StepState::Blocked))
            && !run.steps.values().any(|step| {
                matches!(
                    step.state,
                    StepState::AwaitingApproval
                        | StepState::AwaitingSkillApproval
                        | StepState::NeedsReconciliation
                )
            });
        if failed_stop {
            for step in run
                .steps
                .values_mut()
                .filter(|step| step.state == StepState::Pending)
            {
                step.state = StepState::Skipped;
                step.detail = "the run stopped after a step failed or was blocked".into();
            }
        }
        run.state = self.status(&run, &runbook);
        run.updated_at = self.now;
        Ok(self.store.put_run(run))
    }

    /** Record a step's invocation outcome and settle its budget (unless it is still waiting)
     * Input
        - run: &mut RunRecord - run
        - step: &Step - step
        - invocation: &InvocationRecord - invocation
     * Output
        - Result<(), String>
    */
    fn finish_step(
        &mut self,
        run: &mut RunRecord,
        step: &Step,
        invocation: &InvocationRecord,
    ) -> Result<(), String> {
        let (state, detail) = Self::state_of(invocation);
        let attempt = run.steps[&step.step_id].attempts;
        if state != StepState::AwaitingSkillApproval {
            self.settle(&run.run_id, &step.step_id, attempt, invocation, step.cost())?;
        }
        run.log
            .push(format!("{}: {state:?} ({detail})", step.step_id));
        let entry = run.steps.get_mut(&step.step_id).ok_or("unknown step")?;
        entry.state = state;
        entry.detail = detail;
        Ok(())
    }

    /** Report whether a finished step stops the run (a failure or block under the stop policy,
     * unless another step branches on it)
     * Input
        - run: &RunRecord - run
        - runbook: &Runbook - definition
        - step: &Step - step
     * Output
        - bool
    */
    fn should_stop(&self, run: &RunRecord, runbook: &Runbook, step: &Step) -> bool {
        let branched = runbook.steps.iter().any(|other| {
            other
                .when
                .as_ref()
                .is_some_and(|when| when.step == step.step_id)
        });
        matches!(
            run.steps[&step.step_id].state,
            StepState::Failed | StepState::Blocked
        ) && step.on_failure == FailurePolicy::Stop
            && !branched
    }

    /** The run's status from its steps
     * Input
        - run: &RunRecord - run
        - runbook: &Runbook - definition
     * Output
        - RunState
    */
    fn status(&self, run: &RunRecord, runbook: &Runbook) -> RunState {
        let states = run
            .steps
            .values()
            .map(|step| step.state)
            .collect::<Vec<_>>();
        if states.contains(&StepState::NeedsReconciliation) {
            return RunState::NeedsReconciliation;
        }
        if states.iter().any(|state| {
            matches!(
                state,
                StepState::AwaitingApproval | StepState::AwaitingSkillApproval
            )
        }) {
            return RunState::AwaitingApproval;
        }
        let required = runbook.required();
        let succeeded = required
            .iter()
            .filter(|id| {
                run.steps
                    .get(*id)
                    .is_some_and(|step| step.state == StepState::Succeeded)
            })
            .count();
        let pending = states.contains(&StepState::Pending);
        if pending {
            // A stop left steps unstarted: they never will start
            let failed = states
                .iter()
                .any(|state| matches!(state, StepState::Failed));
            let blocked = states
                .iter()
                .any(|state| matches!(state, StepState::Blocked));
            if failed || blocked {
                return if succeeded > 0 {
                    RunState::Partial
                } else if failed {
                    RunState::Failed
                } else {
                    RunState::Blocked
                };
            }
            return RunState::Running;
        }
        if succeeded == required.len() {
            RunState::Succeeded
        } else if succeeded > 0 {
            RunState::Partial
        } else if states.contains(&StepState::Failed) {
            RunState::Failed
        } else {
            RunState::Blocked
        }
    }

    /** Approve a runbook approval point for a run
     * Input
        - run_id: &str - run
        - step_id: &str - step
        - digest: &str - the approval digest shown to the approver
     * Output
        - Result<RunRecord, String>
        - Error for the run's starter, an agent, a non-validator, a wrong digest, or a step that is
          not waiting
    */
    pub(crate) fn approve(
        &mut self,
        run_id: &str,
        step_id: &str,
        digest: &str,
    ) -> Result<RunRecord, String> {
        let mut run = self.store.run(run_id)?.clone();
        let runbook = self
            .store
            .runbook(&run.runbook_id, Some(run.version))?
            .definition
            .clone();
        let step = runbook
            .step(step_id)
            .ok_or_else(|| format!("run {run_id} has no step {step_id}"))?;
        if run.steps[step_id].state != StepState::AwaitingApproval {
            return Err(format!(
                "step {step_id} is {:?}, not waiting for approval",
                run.steps[step_id].state
            ));
        }
        if self.actor.kind != ActorKind::Human || self.actor.id == run.started_by.id {
            return Err(
                "an approver must be a person other than the one who started the run".into(),
            );
        }
        self.engine
            .registry
            .grants
            .require(&self.actor, Role::Validator)
            .map_err(message)?;
        if run.approval_digest(step) != digest {
            return Err("the approval names another step digest; review the step again".into());
        }
        run.approvals.push(StepApproval {
            step: step_id.into(),
            digest: digest.into(),
            approver: self.actor.clone(),
            at: self.now,
            revoked: None,
        });
        run.log
            .push(format!("{step_id}: approved by {}", self.actor.id));
        let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
        entry.state = StepState::Pending;
        entry.detail = "approved; not started".into();
        self.store.put_run(run);
        self.advance(run_id)
    }

    /** Deny a runbook approval point: the step is blocked and the run follows its failure policy
     * Input
        - run_id: &str - run
        - step_id: &str - step
        - reason: &str - why
     * Output
        - Result<RunRecord, String>
    */
    pub(crate) fn deny(
        &mut self,
        run_id: &str,
        step_id: &str,
        reason: &str,
    ) -> Result<RunRecord, String> {
        let mut run = self.store.run(run_id)?.clone();
        if run.steps.get(step_id).map(|step| step.state) != Some(StepState::AwaitingApproval) {
            return Err(format!("step {step_id} is not waiting for approval"));
        }
        if self.actor.kind != ActorKind::Human || self.actor.id == run.started_by.id {
            return Err(
                "only a person other than the one who started the run decides its approvals".into(),
            );
        }
        run.log.push(format!(
            "{step_id}: approval denied by {} ({reason})",
            self.actor.id
        ));
        let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
        entry.state = StepState::Blocked;
        entry.detail = format!("approval denied: {reason}");
        self.store.put_run(run);
        self.advance(run_id)
    }

    /** Revoke a runbook approval that has not been used (its step has not started)
     * Input
        - run_id: &str - run
        - step_id: &str - step
        - reason: &str - why
     * Output
        - Result<RunRecord, String>
    */
    pub(crate) fn revoke_approval(
        &mut self,
        run_id: &str,
        step_id: &str,
        reason: &str,
    ) -> Result<RunRecord, String> {
        let mut run = self.store.run(run_id)?.clone();
        if run.steps.get(step_id).is_some_and(|step| step.attempts > 0) {
            return Err(format!("step {step_id} already started; its effects are reconciled, not undone by revoking the approval"));
        }
        let mut found = false;
        for approval in run
            .approvals
            .iter_mut()
            .filter(|approval| approval.step == step_id && approval.revoked.is_none())
        {
            approval.revoked = Some(reason.into());
            found = true;
        }
        if !found {
            return Err(format!("step {step_id} has no approval to revoke"));
        }
        run.log.push(format!(
            "{step_id}: approval revoked by {} ({reason})",
            self.actor.id
        ));
        let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
        entry.state = StepState::AwaitingApproval;
        entry.detail = "its approval was revoked".into();
        run.state = RunState::AwaitingApproval;
        Ok(self.store.put_run(run))
    }

    /** Cancel a run: unstarted steps are cancelled, a step waiting for a skill approval is abandoned,
     * and unused reservations are released; completed effects stay recorded
     * Input
        - run_id: &str - run
        - reason: &str - why
     * Output
        - Result<RunRecord, String>
    */
    pub(crate) fn cancel(&mut self, run_id: &str, reason: &str) -> Result<RunRecord, String> {
        let mut run = self.store.run(run_id)?.clone();
        if run.state.ended() {
            return Err(format!("run {run_id} already ended ({:?})", run.state));
        }
        if run
            .steps
            .values()
            .any(|step| step.state == StepState::NeedsReconciliation)
        {
            return Err("a step's effects are uncertain; reconcile it (crane recovery reconcile) before cancelling".into());
        }
        let ids = run.steps.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let step = run.steps[&id].clone();
            match step.state {
                StepState::Pending | StepState::AwaitingApproval => {
                    let entry = run.steps.get_mut(&id).ok_or("unknown step")?;
                    entry.state = StepState::Cancelled;
                    entry.detail = format!("cancelled: {reason}");
                }
                StepState::AwaitingSkillApproval => {
                    if let Some(invocation) = &step.invocation_id {
                        self.engine
                            .abandon(&self.actor, invocation, reason)
                            .map_err(message)?;
                    }
                    self.ledger(
                        run_id,
                        Kind::Release,
                        run_budget_of(&run, &id, self.store)?,
                        "cancelled before it started",
                        &format!("{run_id}:{id}:{}:release", step.attempts),
                    )?;
                    let entry = run.steps.get_mut(&id).ok_or("unknown step")?;
                    entry.state = StepState::Cancelled;
                    entry.detail = format!("cancelled while waiting for approval: {reason}");
                }
                _ => {}
            }
        }
        run.state = RunState::Cancelled;
        run.log
            .push(format!("cancelled by {} ({reason})", self.actor.id));
        run.updated_at = self.now;
        Ok(self.store.put_run(run))
    }

    /** Resume a paused run: steps whose uncertain invocations were reconciled take their outcome;
     * with a step named, that step is retried when it was blocked or failed without any effect and
     * has retries left; then the run advances
     * Input
        - run_id: &str - run
        - from_step: Option<&str> - step to retry
     * Output
        - Result<RunRecord, String>
    */
    pub(crate) fn resume(
        &mut self,
        run_id: &str,
        from_step: Option<&str>,
    ) -> Result<RunRecord, String> {
        let mut run = self.store.run(run_id)?.clone();
        if run.state == RunState::Cancelled {
            return Err(format!("run {run_id} was cancelled"));
        }
        let runbook = self
            .store
            .runbook(&run.runbook_id, Some(run.version))?
            .definition
            .clone();
        let ids = run.steps.keys().cloned().collect::<Vec<_>>();
        for id in &ids {
            let step = run.steps[id].clone();
            if step.state != StepState::NeedsReconciliation {
                continue;
            }
            let invocation = self
                .engine
                .ledger
                .get(step.invocation_id.as_deref().unwrap_or_default())
                .map_err(message)?;
            let (state, detail) = Self::state_of(&invocation);
            if state != StepState::NeedsReconciliation {
                run.log.push(format!("{id}: reconciled to {state:?}"));
                let entry = run.steps.get_mut(id).ok_or("unknown step")?;
                entry.state = state;
                entry.detail = detail;
            }
        }
        if let Some(step_id) = from_step {
            let step = runbook
                .step(step_id)
                .ok_or_else(|| format!("run {run_id} has no step {step_id}"))?;
            let state = run.steps[step_id].clone();
            if !matches!(state.state, StepState::Blocked | StepState::Failed) {
                return Err(format!(
                    "step {step_id} is {:?}; only a blocked or failed step is retried",
                    state.state
                ));
            }
            if state.attempts > step.max_retries {
                return Err(format!(
                    "step {step_id} used its {} retries",
                    step.max_retries
                ));
            }
            if let Some(invocation) = &state.invocation_id {
                let record = self.engine.ledger.get(invocation).map_err(message)?;
                if record.operations.iter().any(|op| {
                    matches!(
                        op.effect_status,
                        EffectStatus::Applied | EffectStatus::Unknown
                    )
                }) {
                    return Err(format!("step {step_id} had effects (applied or uncertain); retrying could repeat them"));
                }
            }
            run.log
                .push(format!("{step_id}: retry requested by {}", self.actor.id));
            let entry = run.steps.get_mut(step_id).ok_or("unknown step")?;
            entry.state = StepState::Pending;
            entry.detail = "retry requested".into();
            // Steps skipped because of it are decided again
            for entry in run
                .steps
                .values_mut()
                .filter(|entry| entry.state == StepState::Skipped)
            {
                entry.state = StepState::Pending;
                entry.detail = "not started".into();
            }
        }
        run.state = RunState::Running;
        self.store.put_run(run);
        self.advance(run_id)
    }
}

/** The operations reserved for a step (its cost)
 * Input
    - run: &RunRecord - run
    - step_id: &str - step
    - store: &RunbookStore - store
 * Output
    - Result<u64, String>
*/
fn run_budget_of(run: &RunRecord, step_id: &str, store: &RunbookStore) -> Result<u64, String> {
    Ok(store
        .runbook(&run.runbook_id, Some(run.version))?
        .definition
        .step(step_id)
        .map_or(0, Step::cost))
}

/** Read a dotted field of a JSON value as text
 * Input
    - value: &Value - value
    - path: &str - dotted path
 * Output
    - Option<String>
*/
fn lookup(value: &Value, path: &str) -> Option<String> {
    let mut node = value;
    for part in path.split('.') {
        node = node.get(part)?;
    }
    crate::capture::format::record::scalar(node)
}
