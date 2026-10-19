// Validation Lab commands. 'crane lab demo' runs the deterministic demonstration; 'crane lab run'
// checks one exact capability version of this repository against its compilation report (from
// runtime/compiler/reports.jsonl) and an approved test plan file, and appends the lab report to
// runtime/capability_lab/reports.jsonl; 'crane lab reports' lists the lab reports of a version.
// Nothing is executed, and no lifecycle state or eligibility changes. Like 'crane test', 'lab run'
// exits with an error when a check failed (the report is printed and stored first).

use super::args::Args;
use super::capability::{read_json, Context};
use crate::capability::refs::Sources;
use crate::capability_lab::preflight::Target;
use crate::capability_lab::report::{run_lab, LabReport};
use crate::capability_lab::scenarios::TestPlan;
use crate::capability_lab::store::LabStore;
use crate::capability_lab::{demo, CheckStatus};
use crate::compiler::assemble::CompilationReport;
use crate::platform::files::read_lines;
use crate::platform::now_millis;

/** The command reference of 'crane lab' */
const USAGE: &str = "usage: crane lab demo [--json]
       crane lab run ID [version N] [digest DIGEST] plan FILE [compilation REPORT_ID] [--json]
       crane lab reports ID [version N] [--json]";

/** Dispatch a lab command
 * Input
    - args: &[String] - arguments after "lab"
 * Output
    - Result<(), String>
*/
pub(crate) fn lab(args: &[String]) -> Result<(), String> {
    let rest = || args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(rest(), &[], &["json"])?),
        Some("run") => run(&Args::parse(
            rest(),
            &["version", "digest", "plan", "compilation"],
            &["json"],
        )?),
        Some("reports") => reports(&Args::parse(rest(), &["version"], &["json"])?),
        _ => Err(USAGE.into()),
    }
}

/** Print a lab report summary
 * Input
    - report: &LabReport - report
 * Output
    - None
*/
fn print_report(report: &LabReport) {
    let summary = &report.summary;
    println!(
        "{} {} v{} (digest {}): {:?}",
        report.report_id,
        report.capability_id,
        report.version,
        report.candidate_digest,
        report.overall
    );
    println!(
        "  passed {}, failed {}, blocked {}, inconclusive {}, not run {}",
        summary.passed, summary.failed, summary.blocked, summary.inconclusive, summary.not_run
    );
    for check in report
        .checks
        .iter()
        .filter(|check| check.status != CheckStatus::Passed)
    {
        println!(
            "  [{:?}] {}: {}",
            check.status, check.check_id, check.detail
        );
    }
    for limitation in &report.limitations {
        println!("  limitation: {limitation}");
    }
    println!("  {}", report.meaning);
}

/** Run the demonstration
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane lab demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    println!("Capability Validation Lab demonstration (fixtures; nothing executed)\n");
    for (label, lab) in [
        ("Compiled candidate", &report.compiled),
        ("Compiled without its test report", &report.needs_evidence),
        ("Altered compilation report", &report.tampered),
        ("Refunds zone moved to v4", &report.stale),
    ] {
        println!("{label}:");
        print_report(lab);
        println!();
    }
    Ok(())
}

/** Find the compilation report of an exact candidate in this repository's compiler reports
 * Input
    - context: &Context - repository context
    - target: &Target - exact version
    - wanted: Option<String> - a specific report id
 * Output
    - Result<Option<CompilationReport>, String>
*/
fn compilation(
    context: &Context,
    target: &Target,
    wanted: Option<String>,
) -> Result<Option<CompilationReport>, String> {
    let lines = read_lines(&context.runtime.join("compiler").join("reports.jsonl"))?;
    let mut found = None;
    for value in lines {
        let report: CompilationReport = serde_json::from_value(value)
            .map_err(|error| format!("a compilation report is malformed: {error}"))?;
        let matches = match &wanted {
            Some(id) => &report.report_id == id,
            None => report.candidate.as_ref().is_some_and(|candidate| {
                candidate.capability_id == target.capability_id
                    && candidate.version == target.version
                    && candidate.digest == target.expected_digest
            }),
        };
        if matches {
            found = Some(report);
        }
    }
    if wanted.is_some() && found.is_none() {
        return Err("no compilation report has that id".into());
    }
    Ok(found)
}

/** Run the lab on an exact version
 * Input
    - args: &Args - id, version, digest, plan, compilation
 * Output
    - Result<(), String>
*/
fn run(args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        "crane lab run ID [version N] [digest DIGEST] plan FILE [compilation REPORT_ID]",
    )?;
    let plan_path = args
        .value("plan")
        .ok_or_else(|| format!("'plan' is required\n{USAGE}"))?;
    let plan = TestPlan::parse(&read_json(&plan_path)?.to_string())?;
    let context = Context::open()?;
    let id = args.positional[0].clone();
    let version = match args.value("version") {
        Some(version) => version
            .parse::<u32>()
            .map_err(|_| "version must be a positive integer".to_string())?,
        None => context
            .registry
            .state
            .versions
            .get(&id)
            .and_then(|versions| versions.keys().next_back().copied())
            .ok_or_else(|| format!("capability {id} does not exist"))?,
    };
    let stored = context
        .registry
        .state
        .record(&id, version)
        .map(|record| record.definition_digest.clone())
        .unwrap_or_default();
    let target = Target {
        capability_id: id,
        version,
        expected_digest: args.value("digest").unwrap_or(stored),
    };
    let report_of_compilation = compilation(&context, &target, args.value("compilation"))?;
    let sources = Sources {
        capture: &context.capture,
        current: &context.current,
        reader: &context.reader,
    };
    let report = run_lab(
        &context.registry,
        &sources,
        &context.actor,
        &target,
        report_of_compilation.as_ref(),
        &plan,
        now_millis(),
    )?;
    LabStore::open(&context.runtime).append(&report)?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print_report(&report);
    }
    if report.overall == CheckStatus::Failed {
        return Err(format!(
            "lab checks failed (report {} was stored)",
            report.report_id
        ));
    }
    Ok(())
}

/** List the lab reports of a version
 * Input
    - args: &Args - id and version
 * Output
    - Result<(), String>
*/
fn reports(args: &Args) -> Result<(), String> {
    args.expect_positional(1, "crane lab reports ID [version N]")?;
    let context = Context::open()?;
    let id = &args.positional[0];
    let version = match args.value("version") {
        Some(version) => version
            .parse::<u32>()
            .map_err(|_| "version must be a positive integer".to_string())?,
        None => context
            .registry
            .state
            .versions
            .get(id)
            .and_then(|versions| versions.keys().next_back().copied())
            .ok_or_else(|| format!("capability {id} does not exist"))?,
    };
    let references = LabStore::open(&context.runtime).references(id, version)?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&references).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if references.is_empty() {
        println!("No lab report for {id} v{version}.");
    }
    for reference in references {
        println!(
            "{} {:?} (candidate digest {})",
            reference.report_id, reference.overall, reference.candidate_digest
        );
    }
    Ok(())
}
