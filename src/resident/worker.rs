// The resident worker: it turns triggers into activations and advances each activation through its
// lifecycle (queued -> context loading -> investigating -> plan ready -> executing -> verifying ->
// an outcome), persisting the record after every transition and before every external call. It
// runs inside a tick: from the background sync that hooks and commands already start, or from
// 'crane resident tick' (which an OS scheduler can run). There is no daemon, no polling loop, and
// no model session; an idle tick reads cursors and timers and makes no model call.
//
// Delivery is at least once, so: a trigger id seen before is dropped (store.seen), an activation's
// id derives from its assignment and trigger (a repeat finds the existing one), and an external
// step is persisted as dispatching before its call, so a crash leaves it uncertain (reconciled by a
// person) instead of repeated. A step that failed with no effect is retried within the limit.
//
// Every step is decided by authority::authorize against the signed state read immediately before
// it, never at activation start only. Approvals bind to the activation, the plan digest, the step,
// and the signed state; they expire and are used once. People's answers, approvals, and
// reconciliations resume the same activation.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::authority::{self, Authority, Decision};
use super::context::{self, Cache, PackCache};
use super::model::{Assignment, DefinitionRecord, Lifecycle};
use super::plan::{self, Effect, Plan, PlanStep, StepType};
use super::rca::{self, CheckEnv, CheckResult, Origin, RcaRecord, RcaState, ReasonInput, Reasoner};
use super::store::{
    Activation, ActivationState, Approval, ContextRecord, Ignored, Lease, Mode, Outcome,
    ResidentStore, StepRecord, StepState, Usage, LEASE_MS,
};
use super::trigger::{self, kind_matches, Trigger};
use crate::brick::context::Snapshot;
use crate::brick::model::Author;
use crate::capture::stable_id;
use crate::governance::workspace::Workspace;
use crate::platform::notes::note;

/** Most activations one tick advances */
const MAX_PER_TICK: usize = 25;

/** Longest answer kept from a person, in characters */
const MAX_ANSWER: usize = 2000;

/** What an external adapter reports
 * Variants
    - Done - it happened (reference in the external system, evidence)
    - Failed - it did not happen (whether any effect is known to have happened, detail)
    - Uncertain - the outcome is unknown (detail)
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Dispatch {
    Done {
        external_ref: String,
        evidence: String,
    },
    Failed {
        effect: bool,
        detail: String,
    },
    Uncertain {
        detail: String,
    },
}

/** An adapter that carries out one external step type */
pub(crate) trait ActionAdapter {
    /** The step type it carries out
     * Input
        - None (uses self)
     * Output
        - StepType
    */
    fn step_type(&self) -> StepType;

    /** Carry out a step
     * Input
        - text: &str - the step's message
        - key: &str - idempotency key of this step
     * Output
        - Dispatch
    */
    fn dispatch(&self, text: &str, key: &str) -> Dispatch;

    /** Read back what a step with this key really did, when the external system can say
     * Input
        - _key: &str - idempotency key of the step
     * Output
        - Option<Dispatch> - Done or Failed when established, None when it cannot be read back
    */
    fn reconcile(&self, _key: &str) -> Option<Dispatch> {
        None
    }
}

/** The Jira adapter: one issue per step in the configured project ('crane integrate jira'),
 * labelled with a digest of the step's idempotency key. Before creating, and when an outcome is
 * unknown, the label is looked up, so a retry or a crash never opens a second issue
 * Fields
    - workspace: Workspace - repository
*/
pub(crate) struct JiraAdapter {
    pub(crate) workspace: Workspace,
}

/** Turn a Jira answer into a dispatch outcome
 * Input
    - answer: crate::integrations::IssueAnswer - answer
 * Output
    - Dispatch
*/
fn jira_outcome(answer: crate::integrations::IssueAnswer) -> Dispatch {
    use crate::integrations::IssueAnswer;
    match answer {
        IssueAnswer::Created(key) => Dispatch::Done {
            external_ref: format!("jira:{key}"),
            evidence: format!("created {key}"),
        },
        IssueAnswer::Found(key) => Dispatch::Done {
            external_ref: format!("jira:{key}"),
            evidence: format!("{key} already carries this step's label; nothing new was created"),
        },
        IssueAnswer::Absent => Dispatch::Failed {
            effect: false,
            detail: "no issue carries this step's label".into(),
        },
        IssueAnswer::Refused(detail) | IssueAnswer::NotConfigured(detail) => Dispatch::Failed {
            effect: false,
            detail,
        },
        IssueAnswer::Unknown(detail) => Dispatch::Uncertain { detail },
    }
}

impl ActionAdapter for JiraAdapter {
    /** CREATE_TICKET
     * Input
        - None (uses self)
     * Output
        - StepType
    */
    fn step_type(&self) -> StepType {
        StepType::CreateTicket
    }

    /** Look the step's label up, and create the issue only when none carries it (the first line
     * of the text is the summary, the rest the description)
     * Input
        - text: &str - summary and description
        - key: &str - idempotency key
     * Output
        - Dispatch
    */
    fn dispatch(&self, text: &str, key: &str) -> Dispatch {
        let label = crate::integrations::issue_label(key);
        match crate::integrations::find_issue(&self.workspace, &label) {
            crate::integrations::IssueAnswer::Absent => {
                let (summary, description) = text.split_once('\n').unwrap_or((text, ""));
                jira_outcome(crate::integrations::create_issue(
                    &self.workspace,
                    summary,
                    description,
                    &label,
                ))
            }
            other => jira_outcome(other),
        }
    }

    /** Establish what an uncertain step did by looking its label up
     * Input
        - key: &str - idempotency key
     * Output
        - Option<Dispatch>
    */
    fn reconcile(&self, key: &str) -> Option<Dispatch> {
        match crate::integrations::find_issue(
            &self.workspace,
            &crate::integrations::issue_label(key),
        ) {
            answer @ (crate::integrations::IssueAnswer::Found(_)
            | crate::integrations::IssueAnswer::Absent) => Some(jira_outcome(answer)),
            _ => None,
        }
    }
}

/** Ask the step's adapter what an uncertain step really did and record what it establishes
 * Input
    - env: &Env - environment (adapters)
    - step: &PlanStep - the uncertain step
    - record: &mut StepRecord - its record
    - max_retries: u32 - retries allowed for a step that failed without effect
 * Output
    - bool - whether the adapter established the outcome
*/
fn reconcile_with_adapter(
    env: &Env,
    step: &PlanStep,
    record: &mut StepRecord,
    max_retries: u32,
) -> bool {
    let Some(answer) = env
        .adapters
        .iter()
        .find(|adapter| adapter.step_type() == step.step_type)
        .and_then(|adapter| adapter.reconcile(&record.idempotency_key))
    else {
        return false;
    };
    record.updated_at = env.now;
    match answer {
        Dispatch::Done {
            external_ref,
            evidence,
        } => {
            record.state = StepState::Succeeded;
            record.detail = "reconciled from the external system: it happened".into();
            record.external_ref = Some(external_ref);
            record.evidence.push(format!("reconciled: {evidence}"));
        }
        Dispatch::Failed {
            effect: false,
            detail,
        } => {
            record.evidence.push(format!("reconciled: {detail}"));
            if record.attempts <= max_retries {
                record.state = StepState::Pending;
                record.detail =
                    "reconciled from the external system: it did not happen; retrying".into();
            } else {
                record.state = StepState::Failed;
                record.detail = "reconciled from the external system: it did not happen".into();
            }
        }
        Dispatch::Failed { .. } | Dispatch::Uncertain { .. } => return false,
    }
    true
}

/** Try to resolve the uncertain steps of an activation waiting for reconciliation through their
 * adapters; when every one is established, the activation continues
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation_id: &str - activation
 * Output
    - Result<bool, String> - whether it continued
*/
pub(crate) fn auto_reconcile(
    store: &mut ResidentStore,
    env: &mut Env,
    activation_id: &str,
) -> Result<bool, String> {
    let mut activation = store.activation(activation_id)?.clone();
    if activation.state != ActivationState::ReconciliationRequired {
        return Ok(false);
    }
    let max_retries = store
        .definition(
            &activation.definition_id,
            Some(activation.definition_version),
        )
        .map(|record| record.definition.limits.max_retries)
        .unwrap_or(0);
    let plan = activation
        .plan
        .clone()
        .unwrap_or_else(|| Plan::new(Vec::new()));
    let mut resolved = 0;
    let mut open = 0;
    for step in &plan.steps {
        let Some(record) = activation.steps.get_mut(&step.step_id) else {
            continue;
        };
        if record.state != StepState::Uncertain {
            continue;
        }
        if reconcile_with_adapter(env, step, record, max_retries) {
            resolved += 1;
        } else {
            open += 1;
        }
    }
    if resolved == 0 || open > 0 {
        if resolved > 0 {
            store.put_activation(activation);
            store.save()?;
        }
        return Ok(false);
    }
    activation.record(
        ActivationState::Executing,
        "RECONCILED",
        "the external system established every uncertain step",
        env.now,
    );
    activity(
        "RESIDENT_ACTION_RESULT",
        &activation,
        json!({"result": "OK", "basis": "RECONCILED"}),
    );
    store.put_activation(activation);
    store.save()?;
    advance(store, env, activation_id)?;
    Ok(true)
}

/** The Slack adapter: the configured channel of 'crane integrate slack', waiting for Slack's answer
 * Fields
    - workspace: Workspace - repository
*/
pub(crate) struct SlackAdapter {
    pub(crate) workspace: Workspace,
}

impl ActionAdapter for SlackAdapter {
    /** NOTIFY_SLACK
     * Input
        - None (uses self)
     * Output
        - StepType
    */
    fn step_type(&self) -> StepType {
        StepType::NotifySlack
    }

    /** Post the message and classify Slack's answer: 2xx is done; 4xx and a dry run are failures
     * without effect; 5xx and timeouts are uncertain (Slack may have posted it)
     * Input
        - text: &str - message
        - _key: &str - idempotency key (Slack's incoming webhooks take none)
     * Output
        - Dispatch
    */
    fn dispatch(&self, text: &str, _key: &str) -> Dispatch {
        let result = crate::integrations::deliver_now(&self.workspace, "slack", text);
        let answer = result["result"].as_str().unwrap_or_default().to_string();
        match result["status"].as_u64() {
            Some(status) if (200..300).contains(&status) => Dispatch::Done {
                external_ref: format!("slack:HTTP {status}"),
                evidence: answer,
            },
            Some(status) if (400..500).contains(&status) => Dispatch::Failed {
                effect: false,
                detail: answer,
            },
            Some(_) => Dispatch::Uncertain { detail: answer },
            None if answer == "dry-run" => Dispatch::Failed {
                effect: false,
                detail: "CRANE_ALERTS_DRY_RUN is set: nothing was sent".into(),
            },
            None if answer.contains("timed out") || answer.contains("timeout") => {
                Dispatch::Uncertain { detail: answer }
            }
            None => Dispatch::Failed {
                effect: false,
                detail: answer,
            },
        }
    }
}

/** Everything the worker reads from outside the store (the live implementation reads the
 * repository; tests supply fixtures)
 * Fields
    - authority: Box<dyn FnMut() -> Result<Authority, String>> - the Crane view, read again on every call
    - snapshot: Box<dyn FnMut(&Assignment) -> Snapshot> - the target's Flow, Zone, and Brick context
    - checks: Box<dyn FnMut(&Assignment) -> CheckEnv> - real state for checks
    - reasoner: Box<dyn Reasoner> - the reasoner
    - adapters: Vec<Box<dyn ActionAdapter>> - external adapters
    - cache: PackCache - context pack cache
    - now: u64 - Unix milliseconds (the clock of this pass)
    - worker: String - lease holder name
*/
pub(crate) struct Env {
    pub(crate) authority: Box<dyn FnMut() -> Result<Authority, String>>,
    pub(crate) snapshot: Box<dyn FnMut(&Assignment) -> Snapshot>,
    pub(crate) checks: Box<dyn FnMut(&Assignment) -> CheckEnv>,
    pub(crate) reasoner: Box<dyn Reasoner>,
    pub(crate) adapters: Vec<Box<dyn ActionAdapter>>,
    pub(crate) cache: PackCache,
    pub(crate) now: u64,
    pub(crate) worker: String,
}

/** The live environment of a repository
 * Input
    - workspace: &Workspace - repository
    - runtime: &std::path::Path - runtime directory
 * Output
    - Env
*/
pub(crate) fn live(workspace: &Workspace, runtime: &std::path::Path) -> Env {
    let for_authority = workspace.clone();
    let for_snapshot = workspace.clone();
    let for_checks = workspace.clone();
    let runtime_dir = runtime.to_path_buf();
    Env {
        authority: Box::new(move || authority::load(&for_authority)),
        snapshot: Box::new(move |assignment| {
            let targets = std::collections::BTreeSet::from([assignment.target.key()]);
            match crate::brick::store::load(&for_snapshot) {
                Ok((state, store, _)) => {
                    let governance = crate::governance::zones::load(&for_snapshot).ok();
                    crate::brick::context::assemble(
                        &for_snapshot,
                        &state,
                        &store,
                        governance.as_ref(),
                        &targets,
                    )
                }
                Err(error) => Snapshot {
                    version: crate::brick::context::SNAPSHOT_VERSION,
                    targets: targets.into_iter().collect(),
                    digest: String::new(),
                    generation: 0,
                    store_digest: String::new(),
                    bricks: Vec::new(),
                    sections: Vec::new(),
                    conflicts: Vec::new(),
                    gaps: vec![crate::brick::config::problem(
                        "CONTEXT_FILE_MISSING",
                        format!("the signed state could not be read: {error}"),
                    )],
                },
            }
        }),
        checks: Box::new(move |_| {
            let signed = matches!(
                crate::brick::store::load(&for_checks).map(|(_, _, trust)| trust),
                Ok(crate::brick::store::Trust::Verified)
            );
            let mut monitors: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
            for row in crate::brick::runtime::monitor_status(&for_checks, &runtime_dir) {
                monitors
                    .entry(row["target"].as_str().unwrap_or_default().to_string())
                    .or_default()
                    .push((
                        row["source"].as_str().unwrap_or_default().to_string(),
                        row["result"].as_str().unwrap_or("NOT_RUN").to_string(),
                    ));
            }
            CheckEnv {
                root: Some(for_checks.root.clone()),
                signed_state: Some(signed),
                monitors,
                recent: Vec::new(),
                now: crate::platform::now_millis(),
            }
        }),
        reasoner: Box::new(rca::GraphReasoner),
        adapters: vec![
            Box::new(SlackAdapter {
                workspace: workspace.clone(),
            }),
            Box::new(JiraAdapter {
                workspace: workspace.clone(),
            }),
        ],
        cache: PackCache::open(runtime),
        now: crate::platform::now_millis(),
        worker: format!("{}:{}", crate::platform::actor(), std::process::id()),
    }
}

/** What one ingestion did
 * Fields
    - received: usize - triggers taken
    - duplicates: usize - repeated deliveries dropped
    - queued: Vec<String> - activations created
    - resumed: Vec<String> - waiting activations a trigger resumed
    - ignored: usize - triggers that woke nothing
*/
#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct Ingested {
    pub(crate) received: usize,
    pub(crate) duplicates: usize,
    pub(crate) queued: Vec<String>,
    pub(crate) resumed: Vec<String>,
    pub(crate) ignored: usize,
}

/** Take triggers: drop repeats, resume waits they satisfy, and queue one activation per matching
 * active assignment (subject to cooldown and the daily limit)
 * Input
    - store: &mut ResidentStore - store
    - triggers: Vec<Trigger> - normalized triggers
    - now: u64 - Unix milliseconds
 * Output
    - Ingested
*/
pub(crate) fn ingest(store: &mut ResidentStore, triggers: Vec<Trigger>, now: u64) -> Ingested {
    let mut done = Ingested::default();
    for trigger in triggers {
        store.state.counters.triggers_received += 1;
        store.state.counters.lag_ms = store
            .state
            .counters
            .lag_ms
            .max(now.saturating_sub(trigger.occurred_at));
        let ignored = |reason: &str, detail: String, assignment: Option<String>| Ignored {
            trigger_id: trigger.trigger_id.clone(),
            source: trigger.source.name(),
            kind: trigger.kind.clone(),
            assignment,
            reason: reason.into(),
            detail,
            at: now,
        };
        if store.state.seen.contains_key(&trigger.trigger_id) {
            store.state.counters.duplicates += 1;
            done.duplicates += 1;
            store.ignore(ignored(
                "DUPLICATE",
                format!("{} was already received", trigger.idempotency_key),
                None,
            ));
            continue;
        }
        done.received += 1;
        store.state.seen.insert(trigger.trigger_id.clone(), now);
        note(
            "RESIDENT_TRIGGER_RECEIVED",
            json!({"trigger": trigger.trigger_id, "source": trigger.source.name(), "kind": trigger.kind, "targets": trigger.targets, "origin": trigger.origin, "schema_version": trigger.schema_version}),
        );
        // Waits this trigger satisfies resume their own activation
        let waiting = store
            .state
            .activations
            .values()
            .filter(|activation| {
                activation.state == ActivationState::WaitingForEvent
                    && activation.mode == Mode::Live
            })
            .map(|activation| activation.activation_id.clone())
            .collect::<Vec<_>>();
        for id in waiting {
            let Ok(mut activation) = store.activation(&id).cloned() else {
                continue;
            };
            let Ok(assignment) = store.assignment(&activation.assignment_id).cloned() else {
                continue;
            };
            let Some(plan) = activation.plan.clone() else {
                continue;
            };
            let mut satisfied = false;
            for step in plan
                .steps
                .iter()
                .filter(|step| step.step_type == StepType::WaitForEvent)
            {
                let Some(record) = activation.steps.get_mut(&step.step_id) else {
                    continue;
                };
                if record.state != StepState::Waiting {
                    continue;
                }
                let kind = step.params.get("kind").cloned().unwrap_or_default();
                let source = step.params.get("source");
                let concerns = trigger.targets.is_empty()
                    || trigger.targets.contains(&assignment.target.key());
                // An event that happened before the activation's own trigger (delivered late,
                // out of order) is not the event the wait is for
                let later = trigger.occurred_at >= activation.trigger.occurred_at;
                if kind_matches(&kind, &trigger.kind)
                    && concerns
                    && later
                    && source.is_none_or(|source| *source == trigger.source.name())
                {
                    record.state = StepState::Succeeded;
                    record.detail =
                        format!("trigger {} ({}) arrived", trigger.trigger_id, trigger.kind);
                    record
                        .evidence
                        .push(format!("trigger:{}", trigger.trigger_id));
                    record.updated_at = now;
                    satisfied = true;
                }
            }
            if satisfied {
                activation.triggers.push(trigger.trigger_id.clone());
                activation.record(
                    ActivationState::Executing,
                    "RESIDENT_TRIGGER_RECEIVED",
                    format!("the awaited trigger {} arrived", trigger.kind),
                    now,
                );
                store.put_activation(activation);
                done.resumed.push(id);
            }
        }
        let resumed = done.resumed.len();
        let mut matched = false;
        let assignments = store
            .state
            .assignments
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for assignment in assignments {
            if !matches!(assignment.lifecycle, Lifecycle::Active | Lifecycle::Paused) {
                continue;
            }
            if trigger::assignment_matches(&assignment, &trigger).is_err() {
                continue;
            }
            matched = true;
            let id = assignment.assignment_id.clone();
            if assignment.lifecycle == Lifecycle::Paused {
                store.ignore(ignored(
                    "PAUSED",
                    "the assignment is paused".into(),
                    Some(id),
                ));
                continue;
            }
            if assignment.expired_at(now) {
                store.ignore(ignored(
                    "EXPIRED",
                    "the assignment's lifetime has ended".into(),
                    Some(id),
                ));
                continue;
            }
            let Ok(definition) = store
                .definition(
                    &assignment.definition_id,
                    Some(assignment.definition_version),
                )
                .cloned()
            else {
                store.ignore(ignored(
                    "DEFINITION_MISSING",
                    "the definition version is missing".into(),
                    Some(id),
                ));
                continue;
            };
            let previous = store
                .state
                .activations
                .values()
                .filter(|activation| {
                    activation.assignment_id == id && activation.mode == Mode::Live
                })
                .map(|activation| activation.created_at)
                .collect::<Vec<_>>();
            let cooldown = u64::from(definition.definition.limits.cooldown_minutes) * 60_000;
            if previous.iter().any(|at| at + cooldown > now) {
                store.ignore(ignored(
                    "COOLDOWN",
                    format!(
                        "an activation started within the last {} minutes",
                        definition.definition.limits.cooldown_minutes
                    ),
                    Some(id),
                ));
                continue;
            }
            let today = previous
                .iter()
                .filter(|at| **at + super::model::DAY_MS > now)
                .count();
            if today >= definition.definition.limits.max_activations_per_day as usize {
                store.ignore(ignored(
                    "RATE_LIMITED",
                    format!("{today} activations in the last day reach the limit"),
                    Some(id),
                ));
                continue;
            }
            let activation_id = stable_id("act", &[&id, &trigger.trigger_id]);
            if store.state.activations.contains_key(&activation_id) {
                continue;
            }
            let mut activation = Activation {
                activation_id: activation_id.clone(),
                assignment_id: id.clone(),
                definition_id: definition.definition.definition_id.clone(),
                definition_version: definition.definition.version,
                definition_digest: definition.digest.clone(),
                mode: Mode::Live,
                trigger: trigger.clone(),
                triggers: Vec::new(),
                state: ActivationState::Queued,
                lease: None,
                context: None,
                investigation: None,
                plan: None,
                steps: BTreeMap::new(),
                approvals: Vec::new(),
                outcome: None,
                usage: Usage::default(),
                timeline: Vec::new(),
                created_at: now,
                updated_at: now,
                record_digest: String::new(),
            };
            activation.record(
                ActivationState::Queued,
                "RESIDENT_ACTIVATION_QUEUED",
                format!("{} {} matched", trigger.source.name(), trigger.kind),
                now,
            );
            note(
                "RESIDENT_ACTIVATION_QUEUED",
                json!({"assignment": id, "activation": activation_id, "trigger": trigger.trigger_id, "kind": trigger.kind}),
            );
            store.put_activation(activation);
            done.queued.push(activation_id);
        }
        if !matched && done.resumed.len() == resumed {
            done.ignored += 1;
            store.ignore(ignored(
                "NO_MATCH",
                "no active assignment subscribes to it".into(),
                None,
            ));
        }
    }
    done
}

/** Record a note about an activation, with its correlation ids
 * Input
    - kind: &str - note kind
    - activation: &Activation - activation
    - fields: Value - more fields
 * Output
    - None
*/
fn activity(kind: &str, activation: &Activation, fields: Value) {
    let mut value = json!({
        "assignment": activation.assignment_id,
        "activation": activation.activation_id,
        "trigger": activation.trigger.trigger_id,
        "mode": activation.mode,
        "state": activation.state,
        "schema_version": super::SCHEMA_VERSION,
    });
    if let (Some(target), Some(source)) = (value.as_object_mut(), fields.as_object()) {
        for (key, item) in source {
            target.insert(key.clone(), item.clone());
        }
    }
    note(kind, value);
}

/** End an activation with an outcome
 * Input
    - activation: &mut Activation - activation
    - state: ActivationState - final state
    - status: &str - outcome status
    - reason: String - why
    - now: u64 - Unix milliseconds
 * Output
    - None
*/
fn end(
    activation: &mut Activation,
    state: ActivationState,
    status: &str,
    reason: String,
    now: u64,
) {
    activation.lease = None;
    for record in activation.steps.values_mut() {
        if !record.state.finished() && record.state != StepState::Uncertain {
            record.state = if activation.mode == Mode::DryRun {
                StepState::Simulated
            } else {
                StepState::Cancelled
            };
            record.updated_at = now;
        }
    }
    activation.outcome = Some(Outcome {
        status: status.into(),
        reason: reason.clone(),
    });
    activation.record(state, "RESIDENT_ACTIVATION_COMPLETED", reason, now);
    activity(
        "RESIDENT_ACTIVATION_COMPLETED",
        activation,
        json!({"result": status, "model_calls": activation.usage.model_calls, "external_calls": activation.usage.external_calls}),
    );
}

/** Compile the context of an activation and record it (with the cache outcome)
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation: &mut Activation - activation
    - assignment: &Assignment - assignment
    - definition: &DefinitionRecord - definition
    - authority: &Authority - Crane view
    - keep: bool - keep the pack in the cache (live) or only compare (tests)
 * Output
    - context::Pack
*/
fn load_context(
    store: &mut ResidentStore,
    env: &mut Env,
    activation: &mut Activation,
    assignment: &Assignment,
    definition: &DefinitionRecord,
    authority: &Authority,
    keep: bool,
) -> context::Pack {
    let snapshot = (env.snapshot)(assignment);
    let record = assignment.rca.as_ref().and_then(|reference| {
        store
            .rca(&reference.rca_id, Some(reference.version))
            .ok()
            .cloned()
    });
    let pack = context::compile(
        authority,
        definition,
        assignment,
        &snapshot,
        record.as_ref(),
        env.now,
    );
    let kept = env.cache.get(&assignment.assignment_id);
    let cache = context::compare(&pack, kept.as_ref());
    if keep {
        match &cache {
            Cache::Hit => store.state.counters.cache_hits += 1,
            Cache::Miss => store.state.counters.cache_misses += 1,
            Cache::Invalidated(_) => store.state.counters.cache_invalidations += 1,
        }
        if let Err(error) = env.cache.put(&assignment.assignment_id, &pack) {
            activation.record(
                activation.state,
                "CONTEXT_CACHE_WRITE_FAILED",
                error,
                env.now,
            );
        }
        let (kind, changed) = match &cache {
            Cache::Hit => ("RESIDENT_CONTEXT_CACHE_HIT", Vec::new()),
            Cache::Miss => ("RESIDENT_CONTEXT_CACHE_MISS", Vec::new()),
            Cache::Invalidated(changed) => ("RESIDENT_CONTEXT_CACHE_INVALIDATED", changed.clone()),
        };
        activity(
            kind,
            activation,
            json!({"pack": pack.key, "refresh_reason": changed}),
        );
    }
    let previous = store
        .state
        .activations
        .values()
        .filter(|other| {
            other.assignment_id == assignment.assignment_id
                && other.activation_id != activation.activation_id
                && other.state.ended()
        })
        .max_by_key(|other| other.created_at)
        .map(|other| other.activation_id.clone())
        .into_iter()
        .collect();
    activation.context = Some(ContextRecord {
        pack: pack.key.clone(),
        cache,
        dependencies: pack.dependencies.clone(),
        sections: pack
            .sections
            .iter()
            .map(|section| {
                (
                    section.kind.clone(),
                    section.source.clone(),
                    section.reference.clone(),
                    section.bytes,
                )
            })
            .collect(),
        missing: pack.missing.clone(),
        previous,
    });
    pack
}

/** The checks environment of an activation, with the kinds of recently received triggers
 * Input
    - store: &ResidentStore - store
    - env: &mut Env - environment
    - assignment: &Assignment - assignment
 * Output
    - CheckEnv
*/
fn check_env(store: &ResidentStore, env: &mut Env, assignment: &Assignment) -> CheckEnv {
    let mut checks = (env.checks)(assignment);
    checks.now = env.now;
    checks.recent = store
        .state
        .activations
        .values()
        .map(|activation| {
            (
                activation.trigger.kind.clone(),
                activation.trigger.received_at,
            )
        })
        .chain(
            store
                .state
                .ignored
                .iter()
                .map(|item| (item.kind.clone(), item.at)),
        )
        .collect();
    checks
}

/** Investigate and build the plan of an activation
 * Input
    - store: &ResidentStore - store
    - env: &mut Env - environment
    - activation: &mut Activation - activation
    - assignment: &Assignment - assignment
    - definition: &DefinitionRecord - definition
 * Output
    - Result<Vec<(String, String)>, String> plan problems (empty when valid)
*/
fn investigate(
    store: &ResidentStore,
    env: &mut Env,
    activation: &mut Activation,
    assignment: &Assignment,
    definition: &DefinitionRecord,
) -> Result<Vec<(String, String)>, String> {
    let checks = check_env(store, env, assignment);
    let record = assignment.rca.as_ref().and_then(|reference| {
        store
            .rca(&reference.rca_id, Some(reference.version))
            .ok()
            .cloned()
    });
    let before = env.reasoner.model_calls();
    let started = std::time::Instant::now();
    let investigation = env.reasoner.investigate(&ReasonInput {
        definition: &definition.definition,
        assignment,
        trigger: &activation.trigger,
        rca: record.as_ref(),
        checks: &checks,
    })?;
    activation.usage.model_calls += env.reasoner.model_calls().saturating_sub(before);
    activation.usage.model = env.reasoner.model();
    activation.usage.duration_ms += started.elapsed().as_millis() as u64;
    activation.usage.deterministic_checks += investigation
        .candidates
        .iter()
        .map(|candidate| candidate.observations.len() as u64)
        .sum::<u64>();
    let mut problems = plan::validate(&investigation.plan, &definition.definition);
    if activation.usage.model_calls > u64::from(definition.definition.limits.max_model_calls) {
        problems.push((
            "MODEL_BUDGET".to_string(),
            format!(
                "{} model calls exceed the limit of {}",
                activation.usage.model_calls, definition.definition.limits.max_model_calls
            ),
        ));
    }
    activation.plan = Some(investigation.plan.clone());
    activation.steps = investigation
        .plan
        .steps
        .iter()
        .map(|step| {
            (
                step.step_id.clone(),
                StepRecord::pending(
                    format!("{}:{}", activation.activation_id, step.step_id),
                    env.now,
                ),
            )
        })
        .collect();
    activation.investigation = Some(investigation);
    Ok(problems)
}

/** Advance an activation as far as it can go now, persisting after every transition
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation_id: &str - activation
 * Output
    - Result<ActivationState, String> the state it stopped in
*/
pub(crate) fn advance(
    store: &mut ResidentStore,
    env: &mut Env,
    activation_id: &str,
) -> Result<ActivationState, String> {
    for _ in 0..200 {
        let mut activation = store.activation(activation_id)?.clone();
        if activation.state.ended() || activation.state.waiting() {
            return Ok(activation.state);
        }
        let assignment = store.assignment(&activation.assignment_id)?.clone();
        let definition = store
            .definition(
                &activation.definition_id,
                Some(activation.definition_version),
            )
            .ok()
            .cloned();
        let now = env.now;
        match activation.state {
            ActivationState::Queued => {
                if assignment.lifecycle == Lifecycle::Paused {
                    return Ok(ActivationState::Queued);
                }
                let authority = (env.authority)()?;
                let admitted = authority::admit(&authority, &assignment, definition.as_ref(), now);
                if admitted.decision != Decision::Allow {
                    let reason = admitted
                        .reasons
                        .iter()
                        .map(|(code, message)| format!("{code}: {message}"))
                        .collect::<Vec<_>>()
                        .join("; ");
                    end(
                        &mut activation,
                        ActivationState::Blocked,
                        "BLOCKED",
                        reason,
                        now,
                    );
                } else {
                    activation.lease = Some(Lease {
                        holder: env.worker.clone(),
                        expires_at: now + LEASE_MS,
                    });
                    activation.record(
                        ActivationState::ContextLoading,
                        "RESIDENT_ACTIVATION_STARTED",
                        format!("started by {}", env.worker),
                        now,
                    );
                    activity("RESIDENT_ACTIVATION_STARTED", &activation, json!({}));
                }
            }
            ActivationState::ContextLoading => {
                let definition = definition.ok_or("the definition version is missing")?;
                let authority = (env.authority)()?;
                let pack = load_context(
                    store,
                    env,
                    &mut activation,
                    &assignment,
                    &definition,
                    &authority,
                    true,
                );
                activity(
                    "RESIDENT_CONTEXT_READY",
                    &activation,
                    json!({"pack": pack.key, "missing": pack.missing.len(), "result": if pack.complete() { "OK" } else { "UNKNOWN" }}),
                );
                if pack.complete() {
                    activation.record(
                        ActivationState::Investigating,
                        "RESIDENT_CONTEXT_READY",
                        format!(
                            "context pack {} ({} sections)",
                            pack.key,
                            pack.sections.len()
                        ),
                        now,
                    );
                } else {
                    let gaps = pack
                        .missing
                        .iter()
                        .map(|(code, message)| format!("{code}: {message}"))
                        .collect::<Vec<_>>()
                        .join("; ");
                    match definition.definition.on_missing_context {
                        super::model::MissingContext::Block => end(
                            &mut activation,
                            ActivationState::Blocked,
                            "BLOCKED",
                            format!("required context is missing: {gaps}"),
                            now,
                        ),
                        super::model::MissingContext::Ask => {
                            let question = rca::step("context-gap", StepType::AskUser, &[("question", format!("Required context is missing ({gaps}). Answer 'continue' to investigate with these gaps named, or cancel the activation."))], &[], "missing context".into());
                            activation.plan = Some(Plan::new(vec![question]));
                            let mut record =
                                StepRecord::pending(format!("{activation_id}:context-gap"), now);
                            record.state = StepState::AwaitingInput;
                            record.deadline = Some(
                                now + u64::from(definition.definition.limits.wait_minutes) * 60_000,
                            );
                            activation.steps =
                                BTreeMap::from([("context-gap".to_string(), record)]);
                            activation.lease = None;
                            activation.record(
                                ActivationState::AwaitingInput,
                                "RESIDENT_CONTEXT_READY",
                                format!("required context is missing; a person decides: {gaps}"),
                                now,
                            );
                        }
                    }
                }
            }
            ActivationState::Investigating => {
                let definition = definition.ok_or("the definition version is missing")?;
                let problems = investigate(store, env, &mut activation, &assignment, &definition)?;
                let investigation = activation.investigation.clone();
                if let Some(investigation) = &investigation {
                    activity(
                        "RESIDENT_PLAN_CREATED",
                        &activation,
                        json!({"plan": investigation.plan.digest, "steps": investigation.plan.steps.len(), "conclusion": investigation.conclusion, "rca": investigation.rca, "model_calls": activation.usage.model_calls, "problems": problems.len()}),
                    );
                    if investigation.conclusion != rca::Conclusion::Identified {
                        activity(
                            "RESIDENT_RCA_PROPOSED",
                            &activation,
                            json!({"result": investigation.conclusion, "gaps": investigation.gaps.len()}),
                        );
                    }
                }
                if problems.is_empty() {
                    activation.record(
                        ActivationState::PlanReady,
                        "RESIDENT_PLAN_CREATED",
                        format!(
                            "plan {} validated",
                            activation
                                .plan
                                .as_ref()
                                .map(|plan| plan.digest.clone())
                                .unwrap_or_default()
                        ),
                        now,
                    );
                } else {
                    let reason = problems
                        .iter()
                        .map(|(code, message)| format!("{code}: {message}"))
                        .collect::<Vec<_>>()
                        .join("; ");
                    end(
                        &mut activation,
                        ActivationState::Blocked,
                        "BLOCKED",
                        format!("the plan is not valid: {reason}"),
                        now,
                    );
                }
            }
            ActivationState::PlanReady => {
                activation.record(
                    ActivationState::Executing,
                    "EXECUTING",
                    "running steps in order",
                    now,
                );
            }
            ActivationState::Executing => {
                activation.lease = Some(Lease {
                    holder: env.worker.clone(),
                    expires_at: now + LEASE_MS,
                });
                execute_next(
                    store,
                    env,
                    &mut activation,
                    &assignment,
                    definition.as_ref(),
                )?;
            }
            ActivationState::Verifying => {
                verify(&mut activation, now);
            }
            _ => return Ok(activation.state),
        }
        store.put_activation(activation);
        store.save()?;
    }
    Err(format!("activation {activation_id} did not settle"))
}

/** The text of an external step (NOTIFY_SLACK's text, or CREATE_TICKET's summary and
 * description), identifying its source event
 * Input
    - activation: &Activation - activation
    - assignment: &Assignment - assignment
    - step: &PlanStep - step
 * Output
    - String
*/
fn message(activation: &Activation, assignment: &Assignment, step: &PlanStep) -> String {
    format!(
        "{}\n{}(resident {} on {}, activation {}, trigger {} {} {})",
        step.params
            .get("text")
            .or_else(|| step.params.get("summary"))
            .cloned()
            .unwrap_or_default(),
        step.params
            .get("description")
            .map(|description| format!("{description}\n"))
            .unwrap_or_default(),
        assignment.assignment_id,
        assignment.target.key(),
        activation.activation_id,
        activation.trigger.source.name(),
        activation.trigger.kind,
        activation.trigger.trigger_id
    )
}

/** Run the next step that can run, deciding it with Crane first; move to verifying when every
 * step has finished, or to a waiting state when one waits
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation: &mut Activation - activation
    - assignment: &Assignment - assignment
    - definition: Option<&DefinitionRecord> - definition
 * Output
    - Result<(), String>
*/
fn execute_next(
    store: &mut ResidentStore,
    env: &mut Env,
    activation: &mut Activation,
    assignment: &Assignment,
    definition: Option<&DefinitionRecord>,
) -> Result<(), String> {
    let now = env.now;
    let plan = activation
        .plan
        .clone()
        .ok_or("the activation has no plan")?;
    for step in &plan.steps {
        let record = activation
            .steps
            .get(&step.step_id)
            .cloned()
            .ok_or_else(|| format!("step {} has no record", step.step_id))?;
        match record.state {
            state if state.finished() => continue,
            StepState::Uncertain | StepState::Dispatching => {
                let max_retries = definition
                    .map(|record| record.definition.limits.max_retries)
                    .unwrap_or(0);
                let record = activation
                    .steps
                    .get_mut(&step.step_id)
                    .ok_or("step record")?;
                record.state = StepState::Uncertain;
                // The external system may be able to say what happened
                if reconcile_with_adapter(env, step, record, max_retries) {
                    return Ok(());
                }
                record.detail = "the external call started but its outcome was not recorded; establish what happened".into();
                activation.lease = None;
                activation.record(
                    ActivationState::ReconciliationRequired,
                    "RESIDENT_RECONCILIATION_REQUIRED",
                    format!("step {} is uncertain", step.step_id),
                    now,
                );
                activity(
                    "RESIDENT_RECONCILIATION_REQUIRED",
                    activation,
                    json!({"step": step.step_id}),
                );
                return Ok(());
            }
            StepState::AwaitingApproval => {
                activation.lease = None;
                activation.record(
                    ActivationState::AwaitingApproval,
                    "RESIDENT_APPROVAL_REQUIRED",
                    format!("step {} waits for approval", step.step_id),
                    now,
                );
                return Ok(());
            }
            StepState::AwaitingInput => {
                activation.lease = None;
                activation.record(
                    ActivationState::AwaitingInput,
                    "WAITING",
                    format!("step {} waits for an answer", step.step_id),
                    now,
                );
                return Ok(());
            }
            StepState::Waiting => {
                activation.lease = None;
                activation.record(
                    ActivationState::WaitingForEvent,
                    "WAITING",
                    format!("step {} waits for a trigger", step.step_id),
                    now,
                );
                return Ok(());
            }
            _ => {}
        }
        // A step whose dependency did not succeed is skipped
        if let Some(dependency) = step.depends_on.iter().find(|dependency| {
            activation.steps.get(*dependency).is_none_or(|record| {
                record.state.finished() && record.state != StepState::Succeeded
            })
        }) {
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record.state = StepState::Skipped;
            record.detail = format!("dependency {dependency} did not succeed");
            record.updated_at = now;
            return Ok(());
        }
        // Crane decides immediately before the step, from the signed state read now
        let authority = (env.authority)()?;
        let decision = authority::authorize(&authority, assignment, definition, step, now);
        let digest = activation.approval_digest(&step.step_id, &decision.authority_digest);
        let needs_person = decision.decision == Decision::RequireApproval
            || matches!(
                step.step_type,
                StepType::RequestApproval | StepType::ProposeActionPlan
            );
        {
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record.authorization = Some(decision.clone());
            record.updated_at = now;
        }
        match decision.decision {
            Decision::Deny => {
                let record = activation
                    .steps
                    .get_mut(&step.step_id)
                    .ok_or("step record")?;
                record.state = StepState::Denied;
                record.detail = decision
                    .reasons
                    .iter()
                    .map(|(code, message)| format!("{code}: {message}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                activity(
                    "RESIDENT_ACTION_DENIED",
                    activation,
                    json!({"step": step.step_id, "step_type": step.step_type, "reasons": decision.codes()}),
                );
                return Ok(());
            }
            Decision::Unavailable => {
                let record = activation
                    .steps
                    .get_mut(&step.step_id)
                    .ok_or("step record")?;
                record.state = StepState::Unavailable;
                record.detail = decision
                    .reasons
                    .iter()
                    .map(|(code, message)| format!("{code}: {message}"))
                    .collect::<Vec<_>>()
                    .join("; ");
                activity(
                    "RESIDENT_ACTION_RESULT",
                    activation,
                    json!({"step": step.step_id, "step_type": step.step_type, "result": "UNSUPPORTED"}),
                );
                return Ok(());
            }
            Decision::Allow | Decision::RequireApproval => {}
        }
        if needs_person {
            let usable = activation.approvals.iter().position(|approval| {
                approval.step == step.step_id
                    && approval.digest == digest
                    && !approval.used
                    && approval.expires_at > now
            });
            match usable {
                Some(index) if activation.approvals[index].approved => {
                    activation.approvals[index].used = true;
                }
                Some(index) => {
                    let approver = activation.approvals[index].approver.name.clone();
                    activation.approvals[index].used = true;
                    let record = activation
                        .steps
                        .get_mut(&step.step_id)
                        .ok_or("step record")?;
                    record.state = StepState::Denied;
                    record.detail = format!("denied by {approver}");
                    return Ok(());
                }
                None => {
                    let record = activation
                        .steps
                        .get_mut(&step.step_id)
                        .ok_or("step record")?;
                    record.state = StepState::AwaitingApproval;
                    record.detail = format!(
                        "approve with: crane resident approve {} --step {} (digest {digest})",
                        activation.activation_id, step.step_id
                    );
                    activation.lease = None;
                    activation.record(
                        ActivationState::AwaitingApproval,
                        "RESIDENT_APPROVAL_REQUIRED",
                        format!(
                            "step {} ({}) needs a person's approval",
                            step.step_id,
                            step.step_type.name()
                        ),
                        now,
                    );
                    activity(
                        "RESIDENT_APPROVAL_REQUIRED",
                        activation,
                        json!({"step": step.step_id, "step_type": step.step_type, "digest": digest, "reasons": decision.codes()}),
                    );
                    return Ok(());
                }
            }
        }
        activity(
            "RESIDENT_ACTION_STARTED",
            activation,
            json!({"step": step.step_id, "step_type": step.step_type, "decision": decision.decision}),
        );
        run_step(store, env, activation, assignment, definition, step)?;
        return Ok(());
    }
    activation.record(
        ActivationState::Verifying,
        "VERIFYING",
        "every step has finished",
        now,
    );
    Ok(())
}

/** Run one decided step by its type
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation: &mut Activation - activation
    - assignment: &Assignment - assignment
    - definition: Option<&DefinitionRecord> - definition
    - step: &PlanStep - step
 * Output
    - Result<(), String>
*/
fn run_step(
    store: &mut ResidentStore,
    env: &mut Env,
    activation: &mut Activation,
    assignment: &Assignment,
    definition: Option<&DefinitionRecord>,
    step: &PlanStep,
) -> Result<(), String> {
    let now = env.now;
    let limits = definition
        .map(|record| record.definition.limits.clone())
        .unwrap_or_default();
    let wait = now + u64::from(limits.wait_minutes) * 60_000;
    match step.step_type {
        StepType::ReadCheck | StepType::Verify => {
            let checks = check_env(store, env, assignment);
            let result = step
                .check
                .as_ref()
                .map(|check| {
                    rca::evaluate(
                        check,
                        &checks,
                        &activation.trigger,
                        &assignment.target.key(),
                    )
                })
                .unwrap_or_else(|| CheckResult::Unknown("no check".into()));
            activation.usage.deterministic_checks += 1;
            let text = step
                .check
                .as_ref()
                .map(rca::CheckSpec::text)
                .unwrap_or_default();
            let verify = step.step_type == StepType::Verify;
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record
                .evidence
                .push(format!("{text}: {}", rca::result_text(&result)));
            record.updated_at = now;
            let (state, detail) = match (&result, verify) {
                (CheckResult::Passed, _) => (StepState::Succeeded, "PASSED".to_string()),
                (CheckResult::Failed, false) => (StepState::Succeeded, "FAILED (read)".to_string()),
                (CheckResult::Failed, true) => (StepState::Failed, "CONTRADICTED".to_string()),
                (CheckResult::Unknown(reason), _) => {
                    (StepState::Failed, format!("UNKNOWN: {reason}"))
                }
            };
            record.state = state;
            record.detail = detail.clone();
            if verify {
                activity(
                    "RESIDENT_VERIFICATION_RESULT",
                    activation,
                    json!({"step": step.step_id, "result": detail.split(':').next().unwrap_or("UNKNOWN")}),
                );
            }
        }
        StepType::NotifySlack | StepType::CreateTicket => {
            let Some(adapter) = env
                .adapters
                .iter()
                .find(|adapter| adapter.step_type() == step.step_type)
            else {
                let record = activation
                    .steps
                    .get_mut(&step.step_id)
                    .ok_or("step record")?;
                record.state = StepState::Unavailable;
                record.detail = format!(
                    "no adapter is registered for {} in this runtime",
                    step.step_type.name()
                );
                return Ok(());
            };
            // Persist the dispatch before the call, so a crash leaves it uncertain, never repeated
            let key = {
                let record = activation
                    .steps
                    .get_mut(&step.step_id)
                    .ok_or("step record")?;
                record.state = StepState::Dispatching;
                record.attempts += 1;
                record.updated_at = now;
                record.idempotency_key.clone()
            };
            activation.usage.external_calls += 1;
            store.put_activation(activation.clone());
            store.save()?;
            let text = message(activation, assignment, step);
            let outcome = adapter.dispatch(&text, &key);
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            let (result, uncertain) = match outcome {
                Dispatch::Done {
                    external_ref,
                    evidence,
                } => {
                    record.state = StepState::Succeeded;
                    record.detail = "delivered".into();
                    record.external_ref = Some(external_ref);
                    record.evidence.push(evidence);
                    ("OK", false)
                }
                Dispatch::Failed {
                    effect: false,
                    detail,
                } if record.attempts <= limits.max_retries => {
                    record.state = StepState::Pending;
                    record.detail = format!(
                        "attempt {} failed without effect ({detail}); retrying",
                        record.attempts
                    );
                    ("FAILED", false)
                }
                Dispatch::Failed { effect, detail } => {
                    record.state = StepState::Failed;
                    record.detail = format!(
                        "{detail}{}",
                        if effect {
                            " (an effect is known to have happened)"
                        } else {
                            ""
                        }
                    );
                    ("FAILED", false)
                }
                Dispatch::Uncertain { detail } => {
                    record.state = StepState::Uncertain;
                    record.detail = format!("outcome unknown: {detail}");
                    ("UNKNOWN", true)
                }
            };
            activity(
                "RESIDENT_ACTION_RESULT",
                activation,
                json!({"step": step.step_id, "step_type": step.step_type, "result": result}),
            );
            if uncertain {
                activation.lease = None;
                activation.record(ActivationState::ReconciliationRequired, "RESIDENT_RECONCILIATION_REQUIRED", format!("the outcome of {} is unknown; establish it with 'crane resident reconcile'", step.step_id), now);
                activity(
                    "RESIDENT_RECONCILIATION_REQUIRED",
                    activation,
                    json!({"step": step.step_id}),
                );
            }
        }
        StepType::RequestApproval => {
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record.state = StepState::Succeeded;
            record.detail = "approved".into();
        }
        StepType::AskUser => {
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            if record.answer.is_some() {
                record.state = StepState::Succeeded;
            } else {
                record.state = StepState::AwaitingInput;
                record.deadline = Some(wait);
                record.detail = format!(
                    "answer with: crane resident answer {} --step {} --text TEXT",
                    activation.activation_id, step.step_id
                );
            }
        }
        StepType::WaitForEvent => {
            let minutes = step
                .params
                .get("within_minutes")
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(u64::from(limits.wait_minutes));
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record.state = StepState::Waiting;
            record.deadline = Some(now + minutes.min(u64::from(limits.wait_minutes)) * 60_000);
            record.detail = format!(
                "waiting for {} (no polling, no model)",
                step.params.get("kind").cloned().unwrap_or_default()
            );
        }
        StepType::Reconcile => {
            let target = step.params.get("step").cloned().unwrap_or_default();
            let uncertain = activation
                .steps
                .get(&target)
                .is_some_and(|record| record.state == StepState::Uncertain);
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            if uncertain {
                record.state = StepState::AwaitingInput;
                record.deadline = Some(wait);
                record.detail = format!("establish the outcome of {target}: crane resident reconcile {} --step {target} --happened yes|no", activation.activation_id);
            } else {
                record.state = StepState::Succeeded;
                record.detail = format!("{target} is not uncertain; nothing to reconcile");
            }
        }
        StepType::ProposeRca => {
            let label = step.params.get("label").cloned().unwrap_or_default();
            let rca_id = format!("{}-proposed", assignment.assignment_id);
            let version = store
                .state
                .rcas
                .get(&rca_id)
                .and_then(|versions| versions.keys().next_back().copied())
                .unwrap_or(0)
                + 1;
            let graph = rca::proposal_graph(assignment, &activation.trigger, &label, version);
            let reference = format!("{rca_id}@{version}");
            let record = RcaRecord {
                digest: graph.digest(),
                graph,
                state: RcaState::Proposed,
                origin: Origin::Activation {
                    activation_id: activation.activation_id.clone(),
                },
                author: Author {
                    name: format!("activation {}", activation.activation_id),
                    key: None,
                },
                self_reviewed: false,
                changes: vec![super::model::Change {
                    to: "proposed".into(),
                    by: Author {
                        name: env.worker.clone(),
                        key: None,
                    },
                    reason: label,
                    at: now,
                }],
            };
            if activation.mode == Mode::Live {
                store
                    .state
                    .rcas
                    .entry(rca_id)
                    .or_default()
                    .insert(version, record);
            }
            activity(
                "RESIDENT_RCA_PROPOSED",
                activation,
                json!({"rca": reference, "result": "PROPOSAL"}),
            );
            let step_record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            step_record.state = StepState::Succeeded;
            step_record.external_ref = Some(format!("rca:{reference}"));
            step_record.detail = "proposal recorded for review; nothing was published".into();
        }
        StepType::ProposeActionPlan => {
            let plan = activation
                .plan
                .as_ref()
                .ok_or("plan")?
                .accept(&step.step_id);
            for added in &plan.steps {
                if !activation.steps.contains_key(&added.step_id) {
                    let key = format!("{}:{}", activation.activation_id, added.step_id);
                    activation
                        .steps
                        .insert(added.step_id.clone(), StepRecord::pending(key, now));
                }
            }
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record.state = StepState::Succeeded;
            record.detail = format!("proposed steps accepted; plan {}", plan.digest);
            activity(
                "RESIDENT_PLAN_CREATED",
                activation,
                json!({"plan": plan.digest, "steps": plan.steps.len(), "result": "ACCEPTED"}),
            );
            activation.plan = Some(plan);
        }
        StepType::PageOncall | StepType::ExecuteProcedure => {
            let record = activation
                .steps
                .get_mut(&step.step_id)
                .ok_or("step record")?;
            record.state = StepState::Unavailable;
            record.detail = step
                .step_type
                .info()
                .available
                .unwrap_or("unavailable")
                .into();
        }
    }
    Ok(())
}

/** Classify an activation's outcome from the evidence of its steps
 * Input
    - activation: &mut Activation - activation
    - now: u64 - Unix milliseconds
 * Output
    - None
*/
fn verify(activation: &mut Activation, now: u64) {
    let plan = activation
        .plan
        .clone()
        .unwrap_or_else(|| Plan::new(Vec::new()));
    let state_of = |id: &str| {
        activation
            .steps
            .get(id)
            .map(|record| (record.state, record.detail.clone()))
    };
    if activation
        .steps
        .values()
        .any(|record| record.state == StepState::Uncertain)
    {
        activation.record(
            ActivationState::ReconciliationRequired,
            "RESIDENT_RECONCILIATION_REQUIRED",
            "an external effect is uncertain",
            now,
        );
        return;
    }
    let verifications = plan
        .steps
        .iter()
        .filter(|step| step.step_type == StepType::Verify)
        .collect::<Vec<_>>();
    let others = plan
        .steps
        .iter()
        .filter(|step| step.step_type != StepType::Verify)
        .collect::<Vec<_>>();
    let passed = verifications
        .iter()
        .filter(|step| {
            state_of(&step.step_id).is_some_and(|(state, _)| state == StepState::Succeeded)
        })
        .count();
    let contradicted = verifications
        .iter()
        .filter(|step| state_of(&step.step_id).is_some_and(|(_, detail)| detail == "CONTRADICTED"))
        .count();
    let others_ok = others.iter().all(|step| {
        state_of(&step.step_id).is_some_and(|(state, _)| state == StepState::Succeeded)
    });
    let blocked = plan.steps.iter().any(|step| {
        state_of(&step.step_id)
            .is_some_and(|(state, _)| matches!(state, StepState::Denied | StepState::Unavailable))
    });
    let failed = others
        .iter()
        .any(|step| state_of(&step.step_id).is_some_and(|(state, _)| state == StepState::Failed));
    let external = plan.steps.iter().any(|step| {
        step.step_type.info().effect == Effect::External
            && state_of(&step.step_id).is_some_and(|(state, _)| state == StepState::Succeeded)
    });
    let proposal = activation
        .investigation
        .as_ref()
        .is_some_and(|investigation| investigation.conclusion != rca::Conclusion::Identified);
    let (state, status, reason) = if contradicted > 0 {
        (
            ActivationState::Failed,
            "CONTRADICTED",
            format!("{contradicted} verification(s) were contradicted by evidence"),
        )
    } else if !verifications.is_empty() && passed == verifications.len() && others_ok {
        (
            ActivationState::Completed,
            "PROVEN",
            format!("every step succeeded and {passed} verification(s) passed"),
        )
    } else if passed > 0 {
        (
            ActivationState::PartiallyVerified,
            "PARTIALLY_VERIFIED",
            format!(
                "{passed} of {} verification(s) passed; other steps did not all succeed",
                verifications.len()
            ),
        )
    } else if blocked && !external {
        (
            ActivationState::Blocked,
            "BLOCKED",
            "Crane denied a step, or a step had no adapter".into(),
        )
    } else if failed {
        (ActivationState::Failed, "FAILED", "a step failed".into())
    } else if proposal {
        (ActivationState::Unknown, "UNKNOWN", "insufficient evidence for a known remediation; the question or proposal is recorded for review".into())
    } else {
        (
            ActivationState::Unknown,
            "UNKNOWN",
            "no verification evidence: the outcome is not proven".into(),
        )
    };
    end(activation, state, status, reason, now);
}

/** Where a person's decision comes from
 * Variants
    - Machine - a person running Crane on their machine (signers: the repository's signer keys)
    - Dashboard - a repository admin GitHub verified on the dashboard, deciding on the plan digest
      they were shown
*/
pub(crate) enum Via<'a> {
    Machine { signers: &'a [String] },
    Dashboard { plan: &'a str },
}

/** Check that a person may decide a waiting step: a signer key on a machine (for approvals), or a
 * dashboard decision the definition allows and that was made on the plan as it is now; and, when
 * the definition separates duties, not the person who activated the assignment
 * Input
    - store: &ResidentStore - store
    - activation: &Activation - activation
    - decider: &Author - who decides
    - via: &Via - where the decision comes from
    - approval: bool - an approval (true) or an answer
 * Output
    - Result<(), String>
*/
fn check_decider(
    store: &ResidentStore,
    activation: &Activation,
    decider: &Author,
    via: &Via,
    approval: bool,
) -> Result<(), String> {
    let definition = store.definition(
        &activation.definition_id,
        Some(activation.definition_version),
    )?;
    match via {
        Via::Machine { signers } => {
            if approval
                && !decider
                    .key
                    .as_ref()
                    .is_some_and(|key| signers.contains(key))
            {
                return Err("only the repository owner or an admin (a signer) can decide resident approvals".into());
            }
        }
        Via::Dashboard { plan } => {
            if !definition.definition.remote_decisions {
                return Err(format!(
                    "definition {}@{} does not take decisions from the dashboard (remote_decisions is off)",
                    definition.definition.definition_id, definition.definition.version
                ));
            }
            let current = activation
                .plan
                .as_ref()
                .map(|plan| plan.digest.as_str())
                .unwrap_or_default();
            if current != *plan {
                return Err(format!("the plan changed since the decision was made (decided on {plan}, now {current})"));
            }
        }
    }
    if approval && definition.definition.distinct_approver {
        let activator = store
            .assignment(&activation.assignment_id)?
            .changes
            .iter()
            .rev()
            .find(|change| change.to == "active")
            .map(|change| change.by.name.clone());
        if activator.as_deref() == Some(decider.name.as_str()) {
            return Err(format!(
                "separation of duties: {} activated this assignment and cannot approve its steps",
                decider.name
            ));
        }
    }
    Ok(())
}

/** Record a person's decision on a step that waits for approval, then resume the activation
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation_id: &str - activation
    - step_id: &str - step
    - approver: &Author - who
    - via: Via - a signer on a machine, or a verified admin on the dashboard
    - approved: bool - approve or deny
    - reason: &str - why
 * Output
    - Result<ActivationState, String>
*/
#[allow(clippy::too_many_arguments)]
pub(crate) fn approve(
    store: &mut ResidentStore,
    env: &mut Env,
    activation_id: &str,
    step_id: &str,
    approver: &Author,
    via: Via,
    approved: bool,
    reason: &str,
) -> Result<ActivationState, String> {
    if reason.trim().is_empty() {
        return Err("a reason is required".into());
    }
    let mut activation = store.activation(activation_id)?.clone();
    check_decider(store, &activation, approver, &via, true)?;
    let record = activation
        .steps
        .get(step_id)
        .ok_or_else(|| format!("activation {activation_id} has no step {step_id}"))?;
    if record.state != StepState::AwaitingApproval {
        return Err(format!(
            "step {step_id} does not wait for approval (it is {})",
            crate::resident::word(&record.state)
        ));
    }
    // Bind to the signed state of this moment; a change before the step runs voids the approval
    let authority = (env.authority)()?;
    let digest = activation.approval_digest(step_id, &authority.digest());
    let ttl = store
        .definition(
            &activation.definition_id,
            Some(activation.definition_version),
        )
        .map(|record| u64::from(record.definition.limits.approval_ttl_minutes) * 60_000)
        .unwrap_or(86_400_000);
    activation.approvals.push(Approval {
        step: step_id.into(),
        digest: digest.clone(),
        approved,
        approver: approver.clone(),
        reason: reason.into(),
        at: env.now,
        expires_at: env.now + ttl,
        used: false,
    });
    if let Some(record) = activation.steps.get_mut(step_id) {
        record.state = StepState::Pending;
        record.detail = format!(
            "{} by {}",
            if approved { "approved" } else { "denied" },
            approver.name
        );
    }
    activation.record(
        ActivationState::Executing,
        "RESIDENT_APPROVAL_DECIDED",
        format!(
            "{step_id} {} by {}: {reason}",
            if approved { "approved" } else { "denied" },
            approver.name
        ),
        env.now,
    );
    activity(
        "RESIDENT_APPROVAL_DECIDED",
        &activation,
        json!({"step": step_id, "result": if approved { "APPROVED" } else { "DENIED" }, "digest": digest, "approver": approver.name}),
    );
    store.put_activation(activation);
    store.save()?;
    advance(store, env, activation_id)
}

/** Record a person's answer to a waiting question (or the context gap), then resume
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation_id: &str - activation
    - step_id: &str - step
    - text: &str - answer
    - by: &Author - who
    - via: Via - a person on a machine, or a verified admin on the dashboard
 * Output
    - Result<ActivationState, String>
*/
pub(crate) fn answer(
    store: &mut ResidentStore,
    env: &mut Env,
    activation_id: &str,
    step_id: &str,
    text: &str,
    by: &Author,
    via: Via,
) -> Result<ActivationState, String> {
    let (text, secrets) = crate::telemetry::redact::redact(text);
    if text.trim().is_empty() {
        return Err("the answer is empty".into());
    }
    let mut activation = store.activation(activation_id)?.clone();
    check_decider(store, &activation, by, &via, false)?;
    let plan = activation
        .plan
        .clone()
        .ok_or("the activation has no plan")?;
    let step = plan
        .step(step_id)
        .ok_or_else(|| format!("activation {activation_id} has no step {step_id}"))?;
    let record = activation.steps.get_mut(step_id).ok_or("step record")?;
    if record.state != StepState::AwaitingInput || step.step_type == StepType::Reconcile {
        return Err(format!("step {step_id} does not wait for an answer"));
    }
    record.answer = Some(text.chars().take(MAX_ANSWER).collect());
    record.state = StepState::Succeeded;
    record.detail = format!(
        "answered by {}{}",
        by.name,
        if secrets > 0 {
            " (secret-looking values were redacted)"
        } else {
            ""
        }
    );
    record.evidence.push(format!("answer by {}", by.name));
    record.updated_at = env.now;
    if step_id == "context-gap" {
        let proceed = text.trim().eq_ignore_ascii_case("continue");
        if !proceed {
            end(
                &mut activation,
                ActivationState::Cancelled,
                "CANCELLED",
                format!("{} did not continue with missing context", by.name),
                env.now,
            );
        } else {
            activation.plan = None;
            activation.steps.clear();
            activation.record(
                ActivationState::Investigating,
                "ANSWERED",
                format!("{} chose to continue with the named context gaps", by.name),
                env.now,
            );
        }
    } else {
        activation.record(
            ActivationState::Executing,
            "ANSWERED",
            format!("{step_id} answered by {}", by.name),
            env.now,
        );
    }
    activity(
        "RESIDENT_APPROVAL_DECIDED",
        &activation,
        json!({"step": step_id, "result": "ANSWERED"}),
    );
    store.put_activation(activation);
    store.save()?;
    advance(store, env, activation_id)
}

/** Record what a person established about an uncertain step, then resume
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - activation_id: &str - activation
    - step_id: &str - the uncertain step
    - happened: bool - the effect happened
    - by: &Author - who
    - reason: &str - how it was established
 * Output
    - Result<ActivationState, String>
*/
pub(crate) fn reconcile(
    store: &mut ResidentStore,
    env: &mut Env,
    activation_id: &str,
    step_id: &str,
    happened: bool,
    by: &Author,
    reason: &str,
) -> Result<ActivationState, String> {
    if reason.trim().is_empty() {
        return Err("say how the outcome was established (--reason)".into());
    }
    let mut activation = store.activation(activation_id)?.clone();
    let max_retries = store
        .definition(
            &activation.definition_id,
            Some(activation.definition_version),
        )
        .map(|record| record.definition.limits.max_retries)
        .unwrap_or(0);
    let record = activation
        .steps
        .get_mut(step_id)
        .ok_or_else(|| format!("activation {activation_id} has no step {step_id}"))?;
    if record.state != StepState::Uncertain {
        return Err(format!("step {step_id} is not uncertain"));
    }
    record.evidence.push(format!(
        "declared by {}: the effect {} ({reason})",
        by.name,
        if happened {
            "happened"
        } else {
            "did not happen"
        }
    ));
    record.updated_at = env.now;
    if happened {
        record.state = StepState::Succeeded;
        record.detail = format!("declared by {} (not observed)", by.name);
    } else if record.attempts <= max_retries {
        record.state = StepState::Pending;
        record.detail = format!("declared without effect by {}; retrying", by.name);
    } else {
        record.state = StepState::Failed;
        record.detail = format!("declared without effect by {}", by.name);
    }
    // A reconcile step waiting on this one is answered by the same declaration
    let waiting = activation
        .plan
        .as_ref()
        .map(|plan| {
            plan.steps
                .iter()
                .filter(|step| {
                    step.step_type == StepType::Reconcile
                        && step.params.get("step").map(String::as_str) == Some(step_id)
                })
                .map(|step| step.step_id.clone())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for id in waiting {
        if let Some(record) = activation.steps.get_mut(&id) {
            if record.state == StepState::AwaitingInput {
                record.state = StepState::Succeeded;
                record.detail = format!("reconciled by {}", by.name);
            }
        }
    }
    activation.record(
        ActivationState::Executing,
        "RECONCILED",
        format!("{step_id} reconciled by {}", by.name),
        env.now,
    );
    activity(
        "RESIDENT_ACTION_RESULT",
        &activation,
        json!({"step": step_id, "result": if happened { "OK" } else { "FAILED" }, "basis": "DECLARED"}),
    );
    store.put_activation(activation);
    store.save()?;
    advance(store, env, activation_id)
}

/** Cancel an activation (a started external call stays uncertain for reconciliation)
 * Input
    - store: &mut ResidentStore - store
    - activation_id: &str - activation
    - by: &Author - who
    - reason: &str - why
    - now: u64 - Unix milliseconds
 * Output
    - Result<ActivationState, String>
*/
pub(crate) fn cancel(
    store: &mut ResidentStore,
    activation_id: &str,
    by: &Author,
    reason: &str,
    now: u64,
) -> Result<ActivationState, String> {
    if reason.trim().is_empty() {
        return Err("a reason is required".into());
    }
    let mut activation = store.activation(activation_id)?.clone();
    if activation.state.ended() {
        return Err(format!("activation {activation_id} has already ended"));
    }
    if activation
        .steps
        .values()
        .any(|record| matches!(record.state, StepState::Dispatching | StepState::Uncertain))
    {
        for record in activation
            .steps
            .values_mut()
            .filter(|record| record.state == StepState::Dispatching)
        {
            record.state = StepState::Uncertain;
        }
        activation.record(
            ActivationState::ReconciliationRequired,
            "RESIDENT_RECONCILIATION_REQUIRED",
            format!(
                "cancel requested by {}: an external effect must be reconciled first",
                by.name
            ),
            now,
        );
    } else {
        end(
            &mut activation,
            ActivationState::Cancelled,
            "CANCELLED",
            format!("cancelled by {}: {reason}", by.name),
            now,
        );
    }
    let state = activation.state;
    store.put_activation(activation);
    store.save()?;
    Ok(state)
}

/** What one tick did
 * Fields
    - skipped: bool - another worker held the lock
    - ingested: Ingested - trigger ingestion
    - advanced: Vec<(String, ActivationState)> - activations advanced and where they stopped
    - expired: Vec<String> - assignments whose lifetime ended
    - recovered: Vec<String> - activations resumed after an expired lease
    - deadlines: Vec<String> - waits that passed their deadline
    - reconciled: Vec<String> - activations whose uncertain effects an adapter established
    - model_calls: u64 - model calls made by this tick
    - idle: bool - nothing was received, advanced, or due
*/
#[derive(Debug, Clone, Default, serde::Serialize)]
pub(crate) struct Ticked {
    pub(crate) skipped: bool,
    pub(crate) ingested: Ingested,
    pub(crate) advanced: Vec<(String, ActivationState)>,
    pub(crate) expired: Vec<String>,
    pub(crate) recovered: Vec<String>,
    pub(crate) deadlines: Vec<String>,
    pub(crate) reconciled: Vec<String>,
    pub(crate) model_calls: u64,
    pub(crate) idle: bool,
}

/** One worker pass: expire lifetimes, fail waits past their deadline, take triggers, recover
 * activations whose worker's lease expired, and advance what can run; then persist
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - triggers: Vec<Trigger> - triggers collected for this pass
 * Output
    - Result<Ticked, String>
*/
pub(crate) fn tick(
    store: &mut ResidentStore,
    env: &mut Env,
    triggers: Vec<Trigger>,
) -> Result<Ticked, String> {
    let Some(_lock) = store.worker_lock()? else {
        return Ok(Ticked {
            skipped: true,
            ..Ticked::default()
        });
    };
    let now = env.now;
    let started = std::time::Instant::now();
    let calls = env.reasoner.model_calls();
    let mut done = Ticked::default();
    store.state.counters.ticks += 1;
    store.state.counters.lag_ms = 0;
    // Lifetimes
    let expired = store
        .state
        .assignments
        .values()
        .filter(|assignment| {
            matches!(assignment.lifecycle, Lifecycle::Active | Lifecycle::Paused)
                && assignment.expired_at(now)
        })
        .map(|assignment| assignment.assignment_id.clone())
        .collect::<Vec<_>>();
    store.state.counters.health_checks += store
        .state
        .assignments
        .values()
        .filter(|assignment| assignment.lifecycle == Lifecycle::Active)
        .count() as u64;
    for id in &expired {
        if let Ok(mut assignment) = store.assignment(id).cloned() {
            assignment.change(
                Lifecycle::Expired,
                &Author {
                    name: "resident worker".into(),
                    key: None,
                },
                "lifetime ended",
                now,
            );
            store.put_assignment(assignment);
            env.cache.drop_pack(id);
            super::registry::stop_work(store, id, "the assignment's lifetime ended", now);
            note(
                "RESIDENT_EXPIRED",
                json!({"assignment": id, "state": "expired"}),
            );
        }
    }
    done.expired = expired;
    // Deadlines of waits
    let waiting = store
        .state
        .activations
        .values()
        .filter(|activation| activation.state.waiting())
        .map(|activation| activation.activation_id.clone())
        .collect::<Vec<_>>();
    for id in waiting {
        let Ok(mut activation) = store.activation(&id).cloned() else {
            continue;
        };
        let mut passed = false;
        for record in activation.steps.values_mut() {
            if matches!(
                record.state,
                StepState::AwaitingInput | StepState::Waiting | StepState::AwaitingApproval
            ) && record.deadline.is_some_and(|deadline| deadline <= now)
            {
                record.state = StepState::Failed;
                record.detail = "the deadline passed without an answer or event".into();
                passed = true;
            }
        }
        if passed {
            activation.record(
                ActivationState::Executing,
                "DEADLINE_PASSED",
                "a wait passed its deadline",
                now,
            );
            store.put_activation(activation);
            done.deadlines.push(id);
        }
    }
    done.ingested = ingest(store, triggers, now);
    // Uncertain effects an adapter can read back are established without a person
    let uncertain = store
        .state
        .activations
        .values()
        .filter(|activation| {
            activation.mode == Mode::Live
                && activation.state == ActivationState::ReconciliationRequired
        })
        .map(|activation| activation.activation_id.clone())
        .collect::<Vec<_>>();
    for id in uncertain {
        if auto_reconcile(store, env, &id)? {
            done.reconciled.push(id);
        }
    }
    // Activations a crashed worker left behind
    let running = [
        ActivationState::ContextLoading,
        ActivationState::Investigating,
        ActivationState::PlanReady,
        ActivationState::Executing,
        ActivationState::Verifying,
    ];
    let mut order = store
        .state
        .activations
        .values()
        .filter(|activation| {
            activation.mode == Mode::Live
                && (activation.state == ActivationState::Queued
                    || running.contains(&activation.state))
        })
        .filter(|activation| {
            activation
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at <= now || lease.holder == env.worker)
        })
        .map(|activation| {
            (
                activation.created_at,
                activation.activation_id.clone(),
                activation.state,
                activation.lease.is_some(),
            )
        })
        .collect::<Vec<_>>();
    order.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    for (_, id, state, leased) in order.into_iter().take(MAX_PER_TICK) {
        if running.contains(&state)
            && leased
            && !done.ingested.resumed.contains(&id)
            && !done.deadlines.contains(&id)
        {
            if let Ok(mut activation) = store.activation(&id).cloned() {
                activation.record(
                    state,
                    "LEASE_RECOVERED",
                    "the previous worker's lease expired; resumed from the persisted state",
                    now,
                );
                store.put_activation(activation);
                done.recovered.push(id.clone());
            }
        }
        let stopped = advance(store, env, &id)?;
        done.advanced.push((id, stopped));
    }
    store.prune(now);
    done.model_calls = env.reasoner.model_calls().saturating_sub(calls);
    done.idle = done.ingested.received == 0
        && done.advanced.is_empty()
        && done.expired.is_empty()
        && done.deadlines.is_empty()
        && done.reconciled.is_empty();
    if done.idle {
        store.state.counters.idle_ticks += 1;
        store.state.counters.idle_model_calls += done.model_calls;
    } else {
        store.state.counters.model_calls += done.model_calls;
    }
    store.state.counters.last_tick_at = now;
    store.state.counters.last_tick_ms = started.elapsed().as_millis() as u64;
    store.save()?;
    Ok(done)
}

/** How often GitHub Actions is read for finished runs, in milliseconds */
const CI_POLL_MS: u64 = 300_000;

/** Read finished GitHub Actions runs when an active assignment subscribes to CI and the last read
 * is old enough (no model; one GitHub API call). CRANE_RESIDENT_CI_POLL=off turns it off
 * Input
    - api: &dyn crate::capture::resources::github::GitHubApi - GitHub (gh in practice)
    - label: &str - the repository as "github.com/OWNER/NAME"
    - store: &mut ResidentStore - store (cursors)
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - Vec<Trigger>
*/
pub(crate) fn poll_ci(
    api: &dyn crate::capture::resources::github::GitHubApi,
    label: &str,
    store: &mut ResidentStore,
    repository_id: &str,
    now: u64,
) -> Vec<Trigger> {
    let wanted = store.state.assignments.values().any(|assignment| {
        assignment.lifecycle == Lifecycle::Active
            && assignment
                .subscriptions
                .iter()
                .any(|subscription| subscription.source == super::model::TriggerSource::Ci)
    });
    if !wanted
        || std::env::var("CRANE_RESIDENT_CI_POLL").is_ok_and(|value| value == "off")
        || now < store.state.cursors.ci_polled_at + CI_POLL_MS
    {
        return Vec::new();
    }
    let mut parts = label.splitn(3, '/');
    let (Some(_host), Some(owner), Some(name)) = (parts.next(), parts.next(), parts.next()) else {
        return Vec::new();
    };
    let safe = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    if !safe(owner) || !safe(name) {
        return Vec::new();
    }
    store.state.cursors.ci_polled_at = now;
    match api.get(
        &format!("repos/{owner}/{name}/actions/runs?status=completed&per_page=20"),
        // Bounded well under the writers' lock timeout, which commands wait on
        Some(std::time::Instant::now() + std::time::Duration::from_secs(8)),
    ) {
        crate::capture::resources::github::ApiAnswer::Found(runs) => {
            let (found, cursor) =
                trigger::from_workflow_runs(&runs, store.state.cursors.ci_run, repository_id, now);
            store.state.cursors.ci_run = cursor;
            found
        }
        _ => {
            store.state.counters.connector_errors += 1;
            Vec::new()
        }
    }
}

/** Collect the triggers of the real sources since the cursors, advancing them
 * Input
    - workspace: &Workspace - repository
    - stores: &crate::telemetry::Stores - runtime stores
    - store: &mut ResidentStore - resident store (cursors)
    - now: u64 - Unix milliseconds
 * Output
    - Vec<Trigger> (records that do not normalize are skipped)
*/
pub(crate) fn collect(
    workspace: &Workspace,
    stores: &crate::telemetry::Stores,
    store: &mut ResidentStore,
    now: u64,
) -> Vec<Trigger> {
    let mut triggers = Vec::new();
    if let Ok(events) = stores.events.read_all() {
        let (found, cursor) = trigger::from_events(
            &events,
            store.state.cursors.event_sequence,
            &workspace.repository_id,
            now,
        );
        store.state.cursors.event_sequence = cursor;
        triggers.extend(found);
    }
    if let Ok((state, _, _)) = crate::brick::store::load(workspace) {
        let generation = state.generation();
        triggers.extend(trigger::from_generation(
            generation,
            store.state.cursors.generation,
            &workspace.repository_id,
            now,
        ));
        store.state.cursors.generation = generation;
    }
    let monitors = crate::brick::runtime::monitor_status(workspace, &stores.directory);
    let (found, cursor) = trigger::from_monitors(
        &monitors,
        store.state.cursors.monitor_run,
        &workspace.repository_id,
        now,
    );
    store.state.cursors.monitor_run = cursor;
    triggers.extend(found);
    let active = store
        .state
        .assignments
        .values()
        .filter(|assignment| assignment.lifecycle == Lifecycle::Active)
        .collect::<Vec<_>>();
    let due = trigger::from_timers(&active, &store.state.timers, &workspace.repository_id, now);
    // A timer seen for the first time records its slot so it fires one period later
    for assignment in &active {
        for timer in &assignment.timers {
            let key = format!("{}:{}", assignment.assignment_id, timer.timer_id);
            let period = u64::from(timer.every_minutes.max(5)) * 60_000;
            store.state.timers.entry(key).or_insert(now / period);
        }
    }
    for (key, slot, found) in due {
        store.state.timers.insert(key, slot);
        triggers.push(found);
    }
    // Finished GitHub Actions runs, when the repository is connected to GitHub
    if let Ok(Some(connection)) = crate::github::load(workspace) {
        triggers.extend(poll_ci(
            &crate::capture::resources::github::GhCli,
            &connection.normalized,
            store,
            &workspace.repository_id,
            now,
        ));
    }
    triggers
}

/** The resident work of a background sync: nothing at all unless an assignment is active or an
 * activation is unfinished; otherwise collect triggers, tick, and record one event the notes ride
 * on. Best effort: errors are recorded on the event
 * Input
    - workspace: &Workspace - repository
    - stores: &crate::telemetry::Stores - runtime stores
 * Output
    - None
*/
pub(crate) fn sync(workspace: &Workspace, stores: &crate::telemetry::Stores) {
    if !stores
        .directory
        .join("resident")
        .join("store.json")
        .exists()
    {
        return;
    }
    // A command or another sync is changing the store: this sync's tick waits for the next one
    let Ok(Some(mut store)) = ResidentStore::try_open_exclusive(&stores.directory) else {
        return;
    };
    let busy = store
        .state
        .assignments
        .values()
        .any(|assignment| assignment.lifecycle == Lifecycle::Active)
        || store
            .state
            .activations
            .values()
            .any(|activation| !activation.state.ended());
    if !busy {
        return;
    }
    let mut env = live(workspace, &stores.directory);
    let triggers = collect(workspace, stores, &mut store, env.now);
    let result = tick(&mut store, &mut env, triggers);
    let mut event = crate::telemetry::events::Event::new(
        "resident.tick",
        crate::telemetry::model::Domain::RecoveryOversight,
        &workspace.repository_id,
        &format!("resident-tick:{}", env.now),
    );
    event.payload = match &result {
        Ok(done) => {
            json!({"received": done.ingested.received, "queued": done.ingested.queued.len(), "advanced": done.advanced.len(), "idle": done.idle, "model_calls": done.model_calls, "trigger": "SYNC"})
        }
        Err(error) => json!({"error": error, "trigger": "SYNC"}),
    };
    if let Err(error) = result {
        event.reasons.push(error);
    }
    // An idle tick records nothing, so the event store does not grow while assignments wait
    if event.payload["idle"] == true {
        let _ = crate::platform::notes::drain();
        return;
    }
    let _ = stores.events.append(event);
}

/** Test an assignment against a trigger without running anything: match, deduplication, cooldown,
 * context and cache, investigation, plan validation, and Crane's decision for every step (as if
 * the assignment were active now). Read-only checks are evaluated; nothing external is touched,
 * nothing is persisted except the report
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - assignment_id: &str - assignment (draft, validated, or active)
    - trigger: Trigger - the test trigger (a fixture, or a replayed one)
 * Output
    - Result<Value, String> the test report
*/
pub(crate) fn test(
    store: &mut ResidentStore,
    env: &mut Env,
    assignment_id: &str,
    trigger: Trigger,
) -> Result<Value, String> {
    let now = env.now;
    let stored = store.assignment(assignment_id)?.clone();
    let mut notes = Vec::new();
    let mut assignment = stored.clone();
    if assignment.lifecycle != Lifecycle::Active {
        notes.push(format!(
            "the assignment is {}; it was tested as if it were active now",
            crate::resident::word(&assignment.lifecycle)
        ));
        assignment.lifecycle = Lifecycle::Active;
        assignment.expires_at =
            now + u64::from(assignment.lifetime_days.max(1)) * super::model::DAY_MS;
    }
    let mut definition = store
        .definition(
            &assignment.definition_id,
            Some(assignment.definition_version),
        )?
        .clone();
    if definition.state != super::model::DefinitionState::Active {
        notes.push(format!(
            "definition {}@{} is {}; it was tested as if it were active",
            definition.definition.definition_id,
            definition.definition.version,
            crate::resident::word(&definition.state)
        ));
        definition.state = super::model::DefinitionState::Active;
    }
    let matching = trigger::assignment_matches(&assignment, &trigger);
    let duplicate = store.state.seen.contains_key(&trigger.trigger_id);
    let cooldown = u64::from(definition.definition.limits.cooldown_minutes) * 60_000;
    let cooling = store.state.activations.values().any(|other| {
        other.assignment_id == assignment_id
            && other.mode == Mode::Live
            && other.created_at + cooldown > now
    });
    let authority = (env.authority)()?;
    let admitted = authority::admit(&authority, &assignment, Some(&definition), now);
    let mut activation = Activation {
        activation_id: stable_id(
            "act",
            &[assignment_id, &trigger.trigger_id, "test", &now.to_string()],
        ),
        assignment_id: assignment_id.into(),
        definition_id: definition.definition.definition_id.clone(),
        definition_version: definition.definition.version,
        definition_digest: definition.digest.clone(),
        mode: Mode::DryRun,
        trigger: trigger.clone(),
        triggers: Vec::new(),
        state: ActivationState::ContextLoading,
        lease: None,
        context: None,
        investigation: None,
        plan: None,
        steps: BTreeMap::new(),
        approvals: Vec::new(),
        outcome: None,
        usage: Usage::default(),
        timeline: Vec::new(),
        created_at: now,
        updated_at: now,
        record_digest: String::new(),
    };
    let pack = load_context(
        store,
        env,
        &mut activation,
        &assignment,
        &definition,
        &authority,
        false,
    );
    let problems = investigate(store, env, &mut activation, &assignment, &definition)?;
    let checks = check_env(store, env, &assignment);
    let plan = activation
        .plan
        .clone()
        .unwrap_or_else(|| Plan::new(Vec::new()));
    let flattened = plan
        .steps
        .iter()
        .flat_map(|step| std::iter::once(step).chain(step.proposal.iter()))
        .collect::<Vec<_>>();
    let mut decisions = Vec::new();
    for step in &flattened {
        let decision = authority::authorize(&authority, &assignment, Some(&definition), step, now);
        let needs_person = decision.decision == Decision::RequireApproval
            || matches!(
                step.step_type,
                StepType::RequestApproval | StepType::ProposeActionPlan | StepType::AskUser
            );
        let expected = match (&step.check, step.step_type) {
            (Some(check), StepType::ReadCheck | StepType::Verify) => json!(rca::evaluate(
                check,
                &checks,
                &trigger,
                &assignment.target.key()
            )),
            _ => Value::Null,
        };
        decisions.push(json!({
            "step": step.step_id,
            "type": step.step_type.name(),
            "effect": step.step_type.info().effect,
            "depends_on": step.depends_on,
            "decision": decision.decision,
            "needs_person": needs_person,
            "reasons": decision.reasons,
            "basis": step.basis,
            "check": step.check.as_ref().map(rca::CheckSpec::text),
            "expected": expected,
            "proposed": plan.steps.iter().all(|top| top.step_id != step.step_id),
        }));
    }
    let worst = decisions
        .iter()
        .filter_map(|row| serde_json::from_value::<Decision>(row["decision"].clone()).ok())
        .max();
    let investigation = activation.investigation.clone();
    let insufficient = investigation
        .as_ref()
        .is_some_and(|investigation| investigation.conclusion != rca::Conclusion::Identified);
    let result = if matching.is_err()
        || admitted.decision != Decision::Allow
        || !pack.complete()
        || !problems.is_empty()
        || worst == Some(Decision::Deny)
    {
        "BLOCKED"
    } else if insufficient || worst == Some(Decision::Unavailable) {
        "INCONCLUSIVE"
    } else {
        "PASSED"
    };
    let report = json!({
        "test_id": activation.activation_id,
        "assignment": assignment_id,
        "lifecycle": stored.lifecycle,
        "mode": "plan-only: nothing was executed (there is no sandbox; staging execution is not available)",
        "tested_at": now,
        "result": result,
        "trigger": trigger,
        "matching": match &matching { Ok(_) => json!({"matched": true}), Err(reason) => json!({"matched": false, "reason": reason}) },
        "duplicate": duplicate,
        "cooldown": cooling,
        "admission": admitted,
        "context": {"pack": pack.key, "cache": activation.context.as_ref().map(|record| record.cache.clone()), "dependencies": pack.dependencies, "sections": activation.context.as_ref().map(|record| record.sections.clone()), "missing": pack.missing, "gaps": pack.gaps},
        "investigation": investigation,
        "plan": {"digest": plan.digest, "problems": problems, "steps": decisions},
        "usage": activation.usage,
        "notes": notes,
    });
    store.keep_test(report.clone());
    note(
        "RESIDENT_TESTED",
        json!({"assignment": assignment_id, "trigger": trigger.trigger_id, "result": result, "model_calls": activation.usage.model_calls}),
    );
    store.save()?;
    Ok(report)
}
