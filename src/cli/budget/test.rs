// crane budget test: the budget ledger holds under pressure, on a temporary ledger (never a live
// one): concurrent reservations never take more than was granted, a repeated settlement is applied
// once, an exhausted budget is detected rather than clamped silently, and the chain verifies.

use std::sync::Arc;

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::telemetry::ledger::{verify, Kind, Ledger, Operation};

/** Apply one ledger operation on the test account
 * Input
    - ledger: &Ledger - ledger
    - kind: Kind - operation
    - amount: u64 - amount
    - key: &str - idempotency key
 * Output
    - Result<(u64, bool), String> applied amount and whether it was new
*/
fn apply(ledger: &Ledger, kind: Kind, amount: u64, key: &str) -> Result<(u64, bool), String> {
    ledger
        .apply(Operation {
            session: "run:budget-test",
            task: None,
            tool_call_id: None,
            kind,
            amount,
            reason: "budget test",
            registry_generation: 0,
            idempotency_key: key,
        })
        .map(|(entry, new)| (entry.amount, new))
}

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane budget test")?;
    let directory = std::env::temp_dir().join(format!(
        "crane-budget-test-{}-{}",
        std::process::id(),
        crate::platform::now_millis()
    ));
    let ledger = Arc::new(Ledger::open(&directory));
    apply(&ledger, Kind::Grant, 10, "grant")?;
    let handles = (0..8)
        .map(|index| {
            let ledger = Arc::clone(&ledger);
            std::thread::spawn(move || {
                apply(&ledger, Kind::Reserve, 4, &format!("reserve-{index}"))
                    .map(|(amount, _)| amount)
                    .unwrap_or(0)
            })
        })
        .collect::<Vec<_>>();
    let reserved = handles
        .into_iter()
        .map(|handle| handle.join().unwrap_or(0))
        .sum::<u64>();
    let entries = ledger.entries()?;
    let balance = Ledger::balance_of(&entries, "run:budget-test");
    let first = apply(&ledger, Kind::Consume, 4, "settle-1")?;
    let again = apply(&ledger, Kind::Consume, 4, "settle-1")?;
    let short = apply(&ledger, Kind::Reserve, 100, "too-much")?;
    let chain = verify(&ledger.entries()?);
    let _ = std::fs::remove_dir_all(&directory);
    let cases = [
        (
            "eight concurrent reservations never take more than the 10 granted",
            reserved <= 10 && balance.reserved <= 10 && balance.available + balance.reserved == 10,
        ),
        ("a repeated settlement is applied once", first.1 && !again.1),
        (
            "a reservation beyond what is left is detected (applied less than requested)",
            short.0 < 100,
        ),
        (
            "the ledger chain verifies after concurrent writes",
            chain.is_ok(),
        ),
    ];
    let failed = cases.iter().filter(|(_, passed)| !passed).count();
    let detail = cases
        .iter()
        .map(|(name, passed)| format!("  {:<5} {name}", if *passed { "PASS" } else { "FAIL" }))
        .collect::<Vec<_>>();
    let response = Response::new("budget.test", if failed == 0 { Status::Passed } else { Status::Failed }, format!("{} ledger cases on a temporary ledger, {failed} failed", cases.len()))
        .data(json!({"cases": cases.iter().map(|(name, passed)| json!({"case": name, "status": if *passed { "PASS" } else { "FAIL" }})).collect::<Vec<_>>(), "reserved": reserved, "chain": chain.err()}));
    finish(&args, response, &detail)
}
