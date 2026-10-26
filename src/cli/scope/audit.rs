// crane scope audit: the boundary compared with what agents attempted and what really changed on
// this machine, from the hash-chained event log.

use super::print_json;
use crate::cli::args::Args;
use crate::platform::now_millis;
use crate::scope::audit::audit as run;

/** Run the command
 * Input
    - args: &[String] - arguments after "audit"
 * Output
    - Result<(), String>
*/
pub(super) fn audit(args: &[String]) -> Result<(), String> {
    let args = Args::parse(
        args,
        &[
            "session",
            "days",
            "since",
            "skill",
            "run",
            "format",
            "request-id",
        ],
        &["json"],
    )?;
    args.expect_positional(
        0,
        "crane scope audit [session SESSION_ID] [days N] [--json]",
    )?;
    let since = args
        .value("days")
        .map(|days| {
            days.parse::<u64>()
                .ok()
                .filter(|days| *days > 0)
                .map(|days| now_millis().saturating_sub(days * 86_400_000))
                .ok_or_else(|| format!("days must be a positive number, got '{days}'"))
        })
        .transpose()?;
    let since = match args.value("since") {
        Some(value) => Some(super::skill::since(&value)?),
        None => since,
    };
    if args.value("skill").is_some() || args.value("run").is_some() {
        return super::skill::audit(&args, since);
    }
    let boundary = super::boundary()?;
    let report = run(&boundary, args.value("session").as_deref(), since)?;
    if crate::cli::protocol::wants_json(&args)? {
        return print_json(&report);
    }
    println!(
        "Scope audit: {} events in {} sessions (event chain {})",
        report.events, report.sessions, report.chain
    );
    let line = |label: &str, map: &std::collections::BTreeMap<String, usize>| {
        if !map.is_empty() {
            let parts = map
                .iter()
                .map(|(key, count)| format!("{key} {count}"))
                .collect::<Vec<_>>();
            println!("  {label}: {}", parts.join(", "));
        }
    };
    line("Decisions before the action", &report.decisions);
    line("Stopped or escalated, by domain", &report.domains);
    line("Enforcement after the effect", &report.outcomes);
    line("Assessments after the effect", &report.assessments);
    if !report.rules.is_empty() {
        println!("  Rules that stopped or escalated actions:");
        for count in report.rules.iter().take(10) {
            println!(
                "    {} x{}: {}",
                count.key,
                count.count,
                crate::scope::rule(&count.key).1
            );
        }
    }
    if !report.ungoverned_changes.is_empty() {
        println!("  Ungoverned files agents were allowed to change:");
        for count in report.ungoverned_changes.iter().take(10) {
            println!("    {} x{}", count.key, count.count);
        }
    }
    if report.findings.is_empty() {
        println!("Nothing in the record needs attention");
    } else {
        println!("Findings:");
        for finding in &report.findings {
            println!("  - {finding}");
        }
    }
    Ok(())
}
