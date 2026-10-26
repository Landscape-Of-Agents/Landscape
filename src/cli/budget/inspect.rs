// crane budget inspect: a run's budget (granted, reserved, consumed, released, available) with its
// ledger chain verified, or a skill's invocation limits and the hard ceilings.

use serde_json::json;

use crate::cli::family::{budget, reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::plan::PlanLimits;
use crate::runbook::run::account;
use crate::telemetry::ledger::{verify, Ledger};

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "skill"], &[])?;
    args.expect_positional(0, "crane budget inspect --run RUN_ID | --skill SKILL[@N]")?;
    let local = Local::open()?;
    if let Some(run) = args.value("run") {
        let ledger = budget(&local);
        let entries = ledger.entries()?;
        let chain = verify(&entries);
        let balance = Ledger::balance_of(&entries, &account(&run));
        let mine = entries
            .iter()
            .filter(|entry| entry.session == account(&run))
            .collect::<Vec<_>>();
        if mine.is_empty() {
            return finish(
                &args,
                Response::new(
                    "budget.inspect",
                    Status::Unknown,
                    format!("no budget is recorded for {run} (absent, not zero)"),
                ),
                &[],
            );
        }
        let detail = mine
            .iter()
            .map(|entry| {
                format!(
                    "  {} {:?} {} of {} ({})",
                    entry.entry_id, entry.kind, entry.amount, entry.requested, entry.reason
                )
            })
            .collect::<Vec<_>>();
        let response = Response::new(
            "budget.inspect",
            if chain.is_ok() {
                Status::Ok
            } else {
                Status::Failed
            },
            format!(
                "{run}: granted {}, reserved {}, consumed {}, released {}, available {}{}",
                balance.granted,
                balance.reserved,
                balance.consumed,
                balance.released,
                balance.available,
                chain
                    .as_ref()
                    .err()
                    .map(|error| format!("; LEDGER BROKEN: {error}"))
                    .unwrap_or_default()
            ),
        )
        .reference("run_ref", run.clone())
        .data(json!({"balance": balance, "entries": mine, "ledger": chain.err()}));
        return finish(&args, response, &detail);
    }
    let record = skill(
        &local,
        &args
            .value("skill")
            .ok_or("'--run' or '--skill' is required")?,
    )?;
    let limits = PlanLimits::default();
    let response = Response::new("budget.inspect", Status::Ok, format!("{}: up to {} operations, {} ms, {} retries per invocation (ceilings 64, 600000, {})", reference(record), limits.max_operations, limits.deadline_ms, limits.max_retries, limits.max_retries + 2))
        .reference("skill_ref", reference(record))
        .data(json!({"limits": limits, "steps": record.definition.execution.steps.len(), "tokens": "unobserved: model token use is not measured by Crane"}));
    finish(&args, response, &[])
}
