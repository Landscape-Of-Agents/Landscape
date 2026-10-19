// Discovery commands, under the capability group ('crane discover' was a removed 0.3 command and
// stays unknown). 'crane capability discover demo' runs the deterministic demonstration;
// 'crane capability discover task-class CLASS ...' finds this repository's published capabilities
// for a task (with optional Work Capture context: an Evidence Pack and its digest), reading the
// registry and the verified lab reports, and appends the discovery report to
// runtime/discovery/reports.jsonl; 'crane capability discover reports' lists stored reports.
// Nothing in the registry changes.

use super::args::Args;
use super::capability::Context;
use crate::capability::refs::Sources;
use crate::capability_lab::store::LabStore;
use crate::discovery::demo;
use crate::discovery::evaluate::Disposition;
use crate::discovery::report::{discover, DiscoveryReport, DiscoveryStore};
use crate::discovery::resolve::{ContextRef, DiscoveryRequest};
use crate::platform::now_millis;

/** The command reference of 'crane capability discover' */
const USAGE: &str = "usage: crane capability discover demo [--json]
       crane capability discover task-class CLASS [environment ENV] [repository REPO] [preconditions ID,ID]
                      [entities ID,ID] [pack PACK digest DIGEST] [--json]
       crane capability discover reports [--json]";

/** Dispatch a discover command
 * Input
    - args: &[String] - arguments after "discover"
 * Output
    - Result<(), String>
*/
pub(crate) fn discover_command(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(&args[1..], &[], &["json"])?),
        Some("reports") => reports(&Args::parse(&args[1..], &[], &["json"])?),
        Some("task-class" | "--task-class" | "task_class") => task(&Args::parse(
            args,
            &[
                "task-class",
                "environment",
                "repository",
                "preconditions",
                "entities",
                "pack",
                "digest",
            ],
            &["json"],
        )?),
        _ => Err(USAGE.into()),
    }
}

/** Split a comma-separated list
 * Input
    - value: Option<String> - list
 * Output
    - Vec<String>
*/
fn list(value: Option<String>) -> Vec<String> {
    value
        .map(|value| {
            value
                .split(',')
                .map(|item| item.trim().to_string())
                .filter(|item| !item.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/** Print a report summary
 * Input
    - report: &DiscoveryReport - report
 * Output
    - None
*/
fn print_report(report: &DiscoveryReport) {
    println!(
        "{} task {}: {:?}",
        report.report_id, report.request.task_class, report.outcome
    );
    for reason in &report.reasons {
        println!("  - {reason}");
    }
    for candidate in &report.candidates {
        let version = candidate
            .version
            .map_or("-".to_string(), |version| format!("v{version}"));
        match (candidate.disposition, candidate.rank) {
            (Disposition::Recommended, Some(rank)) => println!(
                "  #{rank} {} {version} (digest {})",
                candidate.capability_id,
                candidate.digest.clone().unwrap_or_default()
            ),
            (disposition, _) => println!(
                "  {:?}: {} {version}: {}",
                disposition,
                candidate.capability_id,
                candidate.reasons.join("; ")
            ),
        }
        for lab in &candidate.lab {
            println!(
                "      lab report {} {:?}: {}",
                lab.report_id, lab.overall, lab.note
            );
        }
    }
    for unresolved in &report.unresolved {
        println!("  unresolved: {unresolved}");
    }
}

/** Run the demonstration
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability discover demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    println!("Capability discovery demonstration (fixtures; the registry is not changed)\n");
    for (label, discovery) in [
        ("With the task's Work Capture context", &report.with_context),
        (
            "Without context or environment",
            &report.unknown_environment,
        ),
        ("In staging", &report.staging),
        ("After the refunds zone moved to v4", &report.stale),
    ] {
        println!("{label}:");
        print_report(discovery);
        println!();
    }
    Ok(())
}

/** Discover capabilities for a task in this repository
 * Input
    - args: &Args - task and context
 * Output
    - Result<(), String>
*/
fn task(args: &Args) -> Result<(), String> {
    args.expect_positional(0, USAGE)?;
    let context = match (args.value("pack"), args.value("digest")) {
        (Some(pack_id), Some(pack_digest)) => Some(ContextRef {
            pack_id,
            pack_digest,
        }),
        (None, None) => None,
        _ => return Err("'pack' and 'digest' go together".into()),
    };
    let request = DiscoveryRequest {
        task_class: args.value("task-class").ok_or(USAGE)?,
        environment: args.value("environment"),
        repository: args.value("repository"),
        confirmed_preconditions: list(args.value("preconditions")),
        entities: list(args.value("entities")),
        context,
    };
    let repository = Context::open()?;
    let lab_reports = LabStore::open(&repository.runtime).load()?;
    let sources = Sources {
        capture: &repository.capture,
        current: &repository.current,
        reader: &repository.reader,
    };
    let report = discover(
        &repository.registry,
        &sources,
        &repository.actor,
        &request,
        &lab_reports,
        now_millis(),
    );
    DiscoveryStore::open(&repository.runtime).append(&report)?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print_report(&report);
    }
    Ok(())
}

/** List stored discovery reports (each verified)
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn reports(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability discover reports [--json]")?;
    let repository = Context::open()?;
    let reports = DiscoveryStore::open(&repository.runtime).load()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&reports).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if reports.is_empty() {
        println!("No discovery report.");
    }
    for report in reports {
        println!(
            "{} task {}: {:?} ({} recommended)",
            report.report_id,
            report.request.task_class,
            report.outcome,
            report.recommendations.len()
        );
    }
    Ok(())
}
