// crane brick assign | unassign | agents | monitor run | monitor status. An assignment is a signed
// record that an agent (a provider such as claude or codex, or an exact agent id) works in some
// Flows or Zones; it stays visible while the agent is idle. 'agents' derives each assignment's
// runtime status from local sessions, events, and monitors (no daemon, no model). Monitors are
// deterministic checks run by every background sync and by 'monitor run'.

use serde_json::json;

use super::{open, require_verified, target_list};
use crate::brick::model::{Assignment, AssignmentState};
use crate::brick::runtime;
use crate::brick::store;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run 'crane brick assign'
 * Input
    - args: &[String] - arguments after "assign"
 * Output
    - Result<(), String>
*/
pub(super) fn assign(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["targets"], &["paused"])?;
    args.expect_positional(
        1,
        "crane brick assign AGENT --targets flow:ID,zone:ID [--paused]",
    )?;
    let agent = args.positional[0].clone();
    if agent.is_empty()
        || !agent
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(format!("'{agent}' is not a provider name or agent id"));
    }
    let targets = target_list(&args.value("targets").ok_or("--targets is required")?)?;
    let repo = open()?;
    let mut notes = Vec::new();
    for target in &targets {
        notes.extend(super::check_target(&repo, target)?);
    }
    require_verified(&repo)?;
    let state = if args.flag("paused") {
        AssignmentState::Paused
    } else {
        AssignmentState::Active
    };
    let assignment = Assignment {
        agent: agent.clone(),
        targets: targets.clone(),
        state,
        assigned_by: store::author(&repo.workspace),
        assigned_at: crate::platform::now_millis(),
    };
    let keys = targets
        .iter()
        .map(crate::brick::model::Target::key)
        .collect::<Vec<_>>();
    store::change(
        &repo.workspace,
        "crane brick assign",
        "brick.assigned",
        json!({"agent": agent, "targets": keys, "state": state}),
        |store, _| {
            store.assignments.retain(|existing| existing.agent != agent);
            store.assignments.push(assignment);
            Ok(())
        },
    )?;
    crate::platform::notes::note(
        "BRICK_ASSIGNED",
        json!({"agent": agent, "targets": keys, "state": state}),
    );
    let detail = notes
        .iter()
        .map(|note| format!("  note: {note}"))
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.assign",
        Status::Ok,
        format!(
            "assigned {agent} to {} ({}); it stays listed while idle",
            keys.join(", "),
            crate::cli::protocol::name(&state)
        ),
    )
    .data(json!({"agent": agent, "targets": keys, "state": state}));
    finish(&args, response, &detail)
}

/** Run 'crane brick unassign'
 * Input
    - args: &[String] - arguments after "unassign"
 * Output
    - Result<(), String>
*/
pub(super) fn unassign(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane brick unassign AGENT --reason TEXT")?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required")?;
    let agent = args.positional[0].clone();
    let repo = open()?;
    if !repo
        .store
        .assignments
        .iter()
        .any(|assignment| assignment.agent == agent && assignment.state != AssignmentState::Revoked)
    {
        return Err(format!("{agent} has no assignment"));
    }
    require_verified(&repo)?;
    store::change(
        &repo.workspace,
        "crane brick unassign",
        "brick.unassigned",
        json!({"agent": agent, "reason": reason}),
        |store, _| {
            for assignment in store
                .assignments
                .iter_mut()
                .filter(|assignment| assignment.agent == agent)
            {
                assignment.state = AssignmentState::Revoked;
            }
            Ok(())
        },
    )?;
    crate::platform::notes::note("BRICK_UNASSIGNED", json!({"agent": agent}));
    let response = Response::new(
        "brick.unassign",
        Status::Ok,
        format!("revoked the assignment of {agent}"),
    );
    finish(&args, response, &[])
}

/** Run 'crane brick agents'
 * Input
    - args: &[String] - arguments after "agents"
 * Output
    - Result<(), String>
*/
pub(super) fn agents(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane brick agents")?;
    let repo = open()?;
    let runtime_dir = repo.workspace.trust()?.runtime();
    let rows = runtime::assignments(&repo.workspace, &runtime_dir)?;
    let detail = rows
        .iter()
        .map(|row| {
            format!(
                "  {:<12} {:<8} {:<21} {}",
                row["agent"].as_str().unwrap_or_default(),
                row["assignment_status"].as_str().unwrap_or_default(),
                row["runtime_status"].as_str().unwrap_or_default(),
                row["targets"]
                    .as_array()
                    .map(|targets| targets
                        .iter()
                        .filter_map(|target| target.as_str())
                        .collect::<Vec<_>>()
                        .join(", "))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.agents",
        Status::Ok,
        format!(
            "{} assignment(s); idle agents use no inference (monitors are deterministic)",
            rows.len()
        ),
    )
    .data(json!({"assignments": rows}));
    finish(&args, response, &detail)
}

/** Run 'crane brick monitor run'
 * Input
    - args: &[String] - arguments after "monitor run"
 * Output
    - Result<(), String>
*/
pub(super) fn monitor_run(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &["all"])?;
    args.expect_positional(0, "crane brick monitor run [--all]")?;
    let workspace = crate::governance::workspace::Workspace::locate()?;
    let runtime_dir = workspace.trust()?.runtime();
    let results = runtime::evaluate_monitors(&workspace, &runtime_dir, args.flag("all"))?;
    let attention = results
        .iter()
        .filter(|result| !matches!(result["result"].as_str(), Some("OK")))
        .count();
    let detail = results
        .iter()
        .map(|result| {
            format!(
                "  [{}] {} {} {}: {}",
                result["result"].as_str().unwrap_or_default(),
                result["target"].as_str().unwrap_or_default(),
                result["brick"].as_str().unwrap_or_default(),
                result["source"].as_str().unwrap_or_default(),
                result["detail"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.monitor.run",
        if results
            .iter()
            .any(|result| result["result"] == "UNSUPPORTED")
        {
            Status::Unknown
        } else {
            Status::Ok
        },
        format!(
            "{} monitor(s) evaluated without a model; {attention} need attention",
            results.len()
        ),
    )
    .data(json!({"results": results}));
    finish(&args, response, &detail)
}

/** Run 'crane brick monitor status'
 * Input
    - args: &[String] - arguments after "monitor status"
 * Output
    - Result<(), String>
*/
pub(super) fn monitor_status(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane brick monitor status")?;
    let workspace = crate::governance::workspace::Workspace::locate()?;
    let runtime_dir = workspace.trust()?.runtime();
    let monitors = runtime::monitor_status(&workspace, &runtime_dir);
    let gaps = monitors
        .iter()
        .filter(|monitor| monitor["supported"] == false)
        .count();
    let response = Response::new(
        "brick.monitor.status",
        Status::Ok,
        format!("{} monitor(s), {gaps} without a connector (coverage gaps); runs on every background sync and 'crane brick monitor run'", monitors.len()),
    )
    .data(json!({"monitors": monitors, "runner": "background sync (started by hooks and commands) or 'crane brick monitor run'; no daemon"}));
    finish(&args, response, &[])
}
