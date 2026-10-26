// crane budget: the operation budget of runbook runs, kept in runtime/budget/ledger.jsonl (the same
// atomic, idempotent, hash-chained ledger as the autonomy credits) under the account "run:RUN_ID",
// and a skill's invocation limits. The orchestrator reserves and settles automatically; these
// commands inspect, evaluate, and handle reservations by hand. Usage is never taken from the caller:
// settlement reads the run's own invocation records, under the same idempotency keys the
// orchestrator uses, so nothing can be settled twice. One file per command: inspect, evaluate,
// reserve, settle, release, test.

mod evaluate;
mod inspect;
mod release;
mod reserve;
mod settle;
mod test;

use crate::telemetry::ledger::{Entry, Kind, Ledger};

/** The command reference of 'crane budget' */
const USAGE: &str = "usage: crane budget inspect --run RUN_ID | --skill SKILL[@N]
       crane budget evaluate --run RUN_ID --operation STEP
       crane budget reserve --run RUN_ID --operation STEP --idempotency-key KEY
       crane budget settle --reservation ENTRY_ID --outcome success|failure|partial|unknown
       crane budget release --reservation ENTRY_ID --reason TEXT
       crane budget test
  (every command takes --format json)";

/** Find a reservation entry by id
 * Input
    - ledger: &Ledger - budget ledger
    - id: &str - entry id
 * Output
    - Result<Entry, String>
*/
pub(super) fn reservation(ledger: &Ledger, id: &str) -> Result<Entry, String> {
    ledger
        .entries()?
        .into_iter()
        .find(|entry| entry.entry_id == id && entry.kind == Kind::Reserve)
        .ok_or_else(|| format!("no budget reservation {id}"))
}

/** Split a reservation's idempotency key into run, step, and attempt (or hold key)
 * Input
    - key: &str - "RUN:STEP:ATTEMPT:reserve" or "RUN:STEP:hold:KEY"
 * Output
    - Option<(String, String, String)>
*/
pub(super) fn parts(key: &str) -> Option<(String, String, String)> {
    let pieces = key.split(':').collect::<Vec<_>>();
    match pieces.as_slice() {
        [run, step, attempt, "reserve"] => {
            Some((run.to_string(), step.to_string(), attempt.to_string()))
        }
        [run, step, "hold", hold] => {
            Some((run.to_string(), step.to_string(), format!("hold:{hold}")))
        }
        _ => None,
    }
}

/** Dispatch a budget command
 * Input
    - args: &[String] - arguments after "budget"
 * Output
    - Result<(), String>
*/
pub(crate) fn budget(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("inspect") => inspect::inspect(rest),
        Some("evaluate") => evaluate::evaluate(rest),
        Some("reserve") => reserve::reserve(rest),
        Some("settle") => settle::settle(rest),
        Some("release") => release::release(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::parts;

    /** Reservation keys split into run, step, and attempt or hold
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn reservation_keys_split() {
        assert_eq!(
            parts("rbr_x:refund:2:reserve"),
            Some(("rbr_x".into(), "refund".into(), "2".into()))
        );
        assert_eq!(
            parts("rbr_x:refund:hold:k1"),
            Some(("rbr_x".into(), "refund".into(), "hold:k1".into()))
        );
        assert_eq!(parts("other"), None);
    }
}
