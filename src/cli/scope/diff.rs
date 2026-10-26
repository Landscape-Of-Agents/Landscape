// crane scope diff: the boundary now against the baseline recorded when the rules were last signed
// (or with --record). Fails when the boundary widened, so it can run as a check: repository
// structure that moved governed code out of every zone, flow, or selection is reported even though
// nobody signed anything.

use serde_json::json;

use super::print_json;
use crate::cli::args::Args;
use crate::scope::snapshot::{
    capture, diff as compare, load_baseline, record_baseline, ChangeKind,
};

/** Run the command
 * Input
    - args: &[String] - arguments after "diff"
 * Output
    - Result<(), String>
    - Error when the boundary widened since the baseline (unless --record) or no baseline exists
*/
pub(super) fn diff(args: &[String]) -> Result<(), String> {
    let args = Args::parse(
        args,
        &["skill", "against", "format", "request-id"],
        &["record", "json"],
    )?;
    args.expect_positional(
        0,
        "crane scope diff [--record] [--json] | --skill SKILL@N [--against SKILL@N|current]",
    )?;
    if let Some(skill) = args.value("skill") {
        return super::skill::diff(
            &args,
            &skill,
            &args.value("against").unwrap_or_else(|| "current".into()),
        );
    }
    let boundary = super::boundary()?;
    let now = capture(&boundary)?;
    let record = args.flag("record");
    let Some(baseline) = load_baseline(&boundary.runtime)? else {
        if record {
            record_baseline(&boundary.runtime, &now)?;
            println!(
                "Recorded the current boundary as the baseline ({} governed files)",
                now.files.len()
            );
            return Ok(());
        }
        return Err("no boundary baseline is recorded on this machine; 'crane governance sign', 'crane protect', and 'crane target' record one, or run 'crane scope diff --record'".into());
    };
    let difference = compare(&baseline, &now);
    if record {
        record_baseline(&boundary.runtime, &now)?;
    }
    if crate::cli::protocol::wants_json(&args)? {
        print_json(&json!({
            "baseline_recorded_at": baseline.recorded_at,
            "baseline_generation": baseline.versions.registry_generation,
            "generation": now.versions.registry_generation,
            "widened": difference.widened(),
            "notes": difference.notes,
            "changes": difference.changes,
            "recorded": record,
        }))?;
    } else {
        println!(
            "Boundary now (generation {}) against the baseline (generation {})",
            now.versions.registry_generation, baseline.versions.registry_generation
        );
        for note in &difference.notes {
            println!("  note: {note}");
        }
        if difference.changes.is_empty() {
            println!("  no change");
        }
        for change in &difference.changes {
            let label = match change.kind {
                ChangeKind::Widened => "WIDENED ",
                ChangeKind::Narrowed => "narrowed",
                ChangeKind::Changed => "changed ",
            };
            println!("  {label} {}: {}", change.path, change.detail);
        }
        if record {
            println!("Recorded the current boundary as the new baseline");
        }
    }
    if difference.widened() && !record {
        return Err("the boundary widened since the baseline; review the changes, then sign the rules again or run 'crane scope diff --record' to accept them".into());
    }
    Ok(())
}
