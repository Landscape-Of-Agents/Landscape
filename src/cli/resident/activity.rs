// crane resident context | status | test | trigger | tick | activations | activation | approve |
// answer | reconcile | cancel: what residents receive and do. 'trigger' delivers one ci or webhook
// event (a person, or a CI step) and runs a tick; 'tick' is what the background sync runs and what
// an OS scheduler can run; 'test' plans without executing anything. Decisions of people resume the
// activation they belong to.

use serde_json::json;

use super::{open, open_read, read_json, reason, require_signer, signers};
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::resident::store::ActivationState;
use crate::resident::{authority, context, report, trigger, word, worker};

/** The envelope status of where an activation stopped
 * Input
    - state: ActivationState - state
 * Output
    - Status
*/
fn status_of(state: ActivationState) -> Status {
    match state {
        ActivationState::Completed => Status::Ok,
        ActivationState::AwaitingApproval => Status::ApprovalRequired,
        ActivationState::Failed => Status::Failed,
        ActivationState::Blocked | ActivationState::Cancelled => Status::Blocked,
        ActivationState::Unknown
        | ActivationState::PartiallyVerified
        | ActivationState::ReconciliationRequired => Status::Unknown,
        _ => Status::Ok,
    }
}

/** Run 'crane resident context': the pack an activation of the assignment would receive now and
 * how it compares with the kept one (nothing is written)
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn context(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane resident context ID")?;
    let id = args.positional[0].clone();
    let repo = open_read()?;
    let assignment = repo.store.assignment(&id)?.clone();
    let definition = repo
        .store
        .definition(
            &assignment.definition_id,
            Some(assignment.definition_version),
        )?
        .clone();
    let view = authority::load(&repo.workspace)?;
    let mut env = worker::live(&repo.workspace, &repo.runtime);
    let snapshot = (env.snapshot)(&assignment);
    let rca = assignment.rca.as_ref().and_then(|reference| {
        repo.store
            .rca(&reference.rca_id, Some(reference.version))
            .ok()
    });
    let pack = context::compile(&view, &definition, &assignment, &snapshot, rca, env.now);
    let cache = context::compare(&pack, env.cache.get(&id).as_ref());
    let detail = pack
        .sections
        .iter()
        .map(|section| {
            format!(
                "  [{}] {} ({}, {} bytes)",
                section.kind, section.source, section.reference, section.bytes
            )
        })
        .chain(
            pack.missing
                .iter()
                .map(|(code, message)| format!("  missing {code}: {message}")),
        )
        .collect::<Vec<_>>();
    let response = Response::new("resident.context", if pack.complete() { Status::Ok } else { Status::Unknown }, format!("context pack {} for {id}: {} section(s), cache {}", pack.key, pack.sections.len(), match &cache { context::Cache::Hit => "HIT".to_string(), context::Cache::Miss => "MISS".to_string(), context::Cache::Invalidated(changed) => format!("INVALIDATED ({})", changed.join(", ")) }))
        .reasons(pack.missing.iter().map(|(code, _)| code.clone()))
        .data(json!({"pack": pack, "cache": cache, "note": "context is guidance; Crane decides every step against the signed state when it runs"}));
    finish(&args, response, &detail)
}

/** Run 'crane resident status': idle cost and health
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn status(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let repo = open_read()?;
    let view = authority::load(&repo.workspace).ok();
    let report = report::build(&repo.store, view.as_ref(), crate::platform::now_millis());
    let counters = &report["health"]["counters"];
    let detail = vec![
        format!("  ticks {} (idle {}), model calls while idle {}, model calls in activations {}", counters["ticks"], counters["idle_ticks"], counters["idle_model_calls"], counters["model_calls"]),
        format!("  triggers received {}, rejected {}, duplicates {}, queue depth {}, failed or blocked {}", counters["triggers_received"], counters["triggers_rejected"], counters["duplicates"], report["health"]["queue_depth"], report["health"]["dead_letter"]),
        format!("  context cache: {} hit, {} miss, {} invalidated; deterministic health checks {}", counters["cache_hits"], counters["cache_misses"], counters["cache_invalidations"], counters["health_checks"]),
        format!("  tokens: {}", report["health"]["token_usage"].as_str().unwrap_or_default()),
    ];
    let response = Response::new(
        "resident.status",
        Status::Ok,
        format!(
            "{} assignment(s), {} pending approval(s), {} question(s), {} to reconcile",
            report["assignments"].as_array().map_or(0, Vec::len),
            report["inbox"]["approvals"].as_array().map_or(0, Vec::len),
            report["inbox"]["questions"].as_array().map_or(0, Vec::len),
            report["inbox"]["reconciliation"]
                .as_array()
                .map_or(0, Vec::len)
        ),
    )
    .data(json!({"health": report["health"], "inbox": report["inbox"]}));
    finish(&args, response, &detail)
}

/** Run 'crane resident test': plan an assignment's response to a fixture or replayed trigger
 * without executing anything
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["event", "replay", "replay-event"], &[])?;
    args.expect_positional(
        1,
        "crane resident test ID (--event EVENT.json | --replay ACTIVATION)",
    )?;
    let id = args.positional[0].clone();
    let mut repo = open()?;
    let now = crate::platform::now_millis();
    let fixture = match (
        args.value("event"),
        args.value("replay"),
        args.value("replay-event"),
    ) {
        (Some(file), None, None) => {
            trigger::fixture(&read_json(&file)?, &repo.workspace.repository_id, now)?
        }
        (None, Some(activation), None) => repo.store.activation(&activation)?.trigger.clone(),
        (None, None, Some(sequence)) => {
            // A recorded Crane event, read from this machine's event store
            let sequence = sequence
                .parse::<u64>()
                .map_err(|_| "--replay-event takes an event sequence number")?;
            let events = crate::telemetry::Stores::open(&repo.runtime)
                .events
                .read_all()?;
            let event = events
                .iter()
                .find(|event| event.sequence == sequence)
                .ok_or_else(|| format!("no event with sequence {sequence}"))?;
            if trigger::self_caused(event) {
                return Err(format!("event {sequence} was caused by resident agents themselves; it never becomes a trigger"));
            }
            let (found, _) = trigger::from_events(
                std::slice::from_ref(event),
                sequence.saturating_sub(1),
                &repo.workspace.repository_id,
                now,
            );
            found
                .into_iter()
                .next()
                .ok_or_else(|| format!("event {sequence} does not normalize into a trigger"))?
        }
        _ => {
            return Err(
                "give one of --event EVENT.json, --replay ACTIVATION, or --replay-event SEQUENCE"
                    .into(),
            )
        }
    };
    let mut env = worker::live(&repo.workspace, &repo.runtime);
    let report = worker::test(&mut repo.store, &mut env, &id, fixture)?;
    let result = report["result"].as_str().unwrap_or("UNKNOWN").to_string();
    let detail = report["plan"]["steps"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|step| {
            format!(
                "  {:<16} {:<20} {:<16} {}",
                step["step"].as_str().unwrap_or_default(),
                step["type"].as_str().unwrap_or_default(),
                step["decision"].as_str().unwrap_or_default(),
                step["basis"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.test",
        match result.as_str() {
            "PASSED" => Status::Passed,
            "BLOCKED" => Status::Blocked,
            _ => Status::Unknown,
        },
        format!(
            "{result}: plan only, nothing was executed ({} step(s))",
            detail.len()
        ),
    )
    .reference("assignment_ref", id)
    .data(report);
    finish(&args, response, &detail)
}

/** Run 'crane resident trigger': deliver one ci or webhook event, then tick
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn trigger(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["file"], &[])?;
    let file = args.value("file").ok_or("--file is required")?;
    let mut repo = open()?;
    let now = crate::platform::now_millis();
    let delivered = match trigger::delivered(
        &read_json(&file)?,
        &repo.author.name,
        &repo.workspace.repository_id,
        now,
    ) {
        Ok(delivered) => delivered,
        Err(error) => {
            repo.store.state.counters.triggers_rejected += 1;
            repo.store.save()?;
            return Err(format!("the trigger was rejected: {error}"));
        }
    };
    let mut env = worker::live(&repo.workspace, &repo.runtime);
    let done = worker::tick(&mut repo.store, &mut env, vec![delivered.clone()])?;
    let response = Response::new(
        "resident.trigger",
        Status::Ok,
        if done.skipped {
            "another worker is running; the trigger was not taken, deliver it again".to_string()
        } else {
            format!(
                "{} {}: {} activation(s) queued, {} resumed, {} duplicate(s)",
                delivered.source.name(),
                delivered.kind,
                done.ingested.queued.len(),
                done.ingested.resumed.len(),
                done.ingested.duplicates
            )
        },
    )
    .reference("trigger_ref", delivered.trigger_id.clone())
    .data(json!({"trigger": delivered, "tick": done}));
    finish(&args, response, &[])
}

/** Run 'crane resident tick': read the real sources and advance what can run
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn tick(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let mut repo = open()?;
    let workspace = repo.workspace.clone();
    let stores = crate::telemetry::Stores::open(&repo.runtime);
    let mut env = worker::live(&workspace, &repo.runtime);
    let triggers = worker::collect(&workspace, &stores, &mut repo.store, env.now);
    let done = worker::tick(&mut repo.store, &mut env, triggers)?;
    let detail = done
        .advanced
        .iter()
        .map(|(id, state)| format!("  {id}: {}", word(state)))
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.tick",
        Status::Ok,
        if done.skipped {
            "another worker is running; nothing was done".to_string()
        } else if done.idle {
            "idle: nothing arrived or was due; no model was called".to_string()
        } else {
            format!(
                "{} trigger(s), {} activation(s) advanced, {} model call(s)",
                done.ingested.received,
                done.advanced.len(),
                done.model_calls
            )
        },
    )
    .data(json!({"tick": done}));
    finish(&args, response, &detail)
}

/** Run 'crane resident activations'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn activations(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["assignment"], &[])?;
    let repo = open_read()?;
    let filter = args.value("assignment");
    let mut rows = repo
        .store
        .state
        .activations
        .values()
        .filter(|activation| {
            filter
                .as_ref()
                .is_none_or(|id| &activation.assignment_id == id)
        })
        .collect::<Vec<_>>();
    rows.sort_by_key(|activation| std::cmp::Reverse(activation.created_at));
    let detail = rows
        .iter()
        .map(|activation| {
            format!(
                "  {} {:<24} {:<16} {} {}",
                activation.activation_id,
                word(&activation.state),
                activation.assignment_id,
                activation.trigger.source.name(),
                activation.trigger.kind
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new("resident.activations", Status::Ok, format!("{} activation(s)", rows.len()))
        .data(json!({"activations": rows.iter().map(|activation| report::activation_summary(activation)).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}

/** Run 'crane resident activation': one activation's timeline, context, investigation, plan,
 * decisions, approvals, and usage
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn activation(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane resident activation ACTIVATION")?;
    let repo = open_read()?;
    let activation = repo.store.activation(&args.positional[0])?;
    let detail = activation
        .timeline
        .iter()
        .map(|entry| {
            format!(
                "  {} {:<24} {:<34} {}",
                entry.at,
                word(&entry.state),
                entry.code,
                entry.detail
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "resident.activation",
        status_of(activation.state),
        format!(
            "{} is {}{}",
            activation.activation_id,
            word(&activation.state),
            activation
                .outcome
                .as_ref()
                .map(|outcome| format!(": {} ({})", outcome.status, outcome.reason))
                .unwrap_or_default()
        ),
    )
    .reference("activation_ref", activation.activation_id.clone())
    .data(report::activation_summary(activation));
    finish(&args, response, &detail)
}

/** Print where an activation stopped after a person's decision
 * Input
    - args: &crate::cli::args::Args - parsed arguments
    - command: &str - command name
    - id: &str - activation
    - state: ActivationState - where it stopped
    - repo: &super::Repo - repository
 * Output
    - Result<(), String>
*/
fn resumed(
    args: &crate::cli::args::Args,
    command: &str,
    id: &str,
    state: ActivationState,
    repo: &super::Repo,
) -> Result<(), String> {
    let activation = repo.store.activation(id)?;
    let response = Response::new(
        command,
        status_of(state),
        format!(
            "{id} resumed and is now {}{}",
            word(&state),
            activation
                .outcome
                .as_ref()
                .map(|outcome| format!(": {}", outcome.status))
                .unwrap_or_default()
        ),
    )
    .reference("activation_ref", id)
    .data(report::activation_summary(activation));
    finish(args, response, &[])
}

/** Run 'crane resident approve'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn approve(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["step", "reason"], &["deny"])?;
    args.expect_positional(
        1,
        "crane resident approve ACTIVATION --step STEP --reason TEXT [--deny]",
    )?;
    let id = args.positional[0].clone();
    let step = args.value("step").ok_or("--step is required")?;
    let reason = reason(&args)?;
    let mut repo = open()?;
    require_signer(&repo, "deciding a resident approval")?;
    let keys = signers(&repo.workspace);
    let author = repo.author.clone();
    let mut env = worker::live(&repo.workspace, &repo.runtime);
    let state = worker::approve(
        &mut repo.store,
        &mut env,
        &id,
        &step,
        &author,
        worker::Via::Machine { signers: &keys },
        !args.flag("deny"),
        &reason,
    )?;
    resumed(&args, "resident.approve", &id, state, &repo)
}

/** Run 'crane resident answer'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn answer(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["step", "text"], &[])?;
    args.expect_positional(
        1,
        "crane resident answer ACTIVATION --step STEP --text TEXT",
    )?;
    let id = args.positional[0].clone();
    let step = args.value("step").ok_or("--step is required")?;
    let text = args.value("text").ok_or("--text is required")?;
    let mut repo = open()?;
    let author = repo.author.clone();
    let mut env = worker::live(&repo.workspace, &repo.runtime);
    let keys = signers(&repo.workspace);
    let state = worker::answer(
        &mut repo.store,
        &mut env,
        &id,
        &step,
        &text,
        &author,
        worker::Via::Machine { signers: &keys },
    )?;
    resumed(&args, "resident.answer", &id, state, &repo)
}

/** Run 'crane resident reconcile'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn reconcile(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["step", "happened", "reason"], &[])?;
    args.expect_positional(
        1,
        "crane resident reconcile ACTIVATION --step STEP --happened yes|no --reason TEXT",
    )?;
    let id = args.positional[0].clone();
    let step = args.value("step").ok_or("--step is required")?;
    let happened = match args.value("happened").as_deref() {
        Some("yes") => true,
        Some("no") => false,
        _ => return Err("--happened must be yes or no".into()),
    };
    let reason = reason(&args)?;
    let mut repo = open()?;
    let author = repo.author.clone();
    let mut env = worker::live(&repo.workspace, &repo.runtime);
    let state = worker::reconcile(
        &mut repo.store,
        &mut env,
        &id,
        &step,
        happened,
        &author,
        &reason,
    )?;
    resumed(&args, "resident.reconcile", &id, state, &repo)
}

/** Run 'crane resident cancel'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
pub(super) fn cancel(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane resident cancel ACTIVATION --reason TEXT")?;
    let id = args.positional[0].clone();
    let reason = reason(&args)?;
    let mut repo = open()?;
    let author = repo.author.clone();
    let state = worker::cancel(
        &mut repo.store,
        &id,
        &author,
        &reason,
        crate::platform::now_millis(),
    )?;
    resumed(&args, "resident.cancel", &id, state, &repo)
}

/** The JSON value of a status for tests of status_of
 * Input
    - state: ActivationState - state
 * Output
    - serde_json::Value
*/
#[cfg(test)]
fn status_value(state: ActivationState) -> serde_json::Value {
    json!(status_of(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Ended activations map to the shared exit statuses; nothing but a verified completion is ok
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn resident_statuses_follow_the_envelope() {
        assert_eq!(status_value(ActivationState::Completed), json!("ok"));
        assert_eq!(
            status_value(ActivationState::AwaitingApproval),
            json!("approval_required")
        );
        assert_eq!(status_value(ActivationState::Unknown), json!("unknown"));
        assert_eq!(
            status_value(ActivationState::PartiallyVerified),
            json!("unknown")
        );
        assert_eq!(status_value(ActivationState::Blocked), json!("blocked"));
        assert_eq!(status_value(ActivationState::Failed), json!("failed"));
    }
}
