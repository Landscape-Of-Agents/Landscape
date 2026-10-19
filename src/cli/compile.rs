// Compiler commands. 'crane compile demo' runs the deterministic demonstration from fixtures;
// 'crane compile episode ...' compiles a work episode of this repository (its Evidence Pack comes
// from 'crane capture investigate') into a capability candidate through Component 2, as the
// person running Crane, and appends the compilation report to the trust directory's
// runtime/compiler/reports.jsonl. Compiling never validates for the record, publishes, or
// executes anything.

use serde_json::json;

use super::args::Args;
use super::capability::{read_json, Context};
use crate::capability::intake::{EnterpriseIntent, IntakeRequest, INTAKE_SCHEMA_VERSION};
use crate::capability::model::RiskLevel;
use crate::capability::refs::Sources;
use crate::compiler::assemble::{compile as run_compile, CompilationReport};
use crate::compiler::{demo, CompileRequest};
use crate::platform::files::append_line;
use crate::platform::now_millis;

/** The command reference of 'crane compile' */
const USAGE: &str = "usage: crane compile demo [--json]
       crane compile episode EPISODE pack PACK digest DIGEST capability ID name TEXT purpose TEXT task-class CLASS
                     [intent FILE] [risk low|medium|high|critical] [profile NAME] [--json]";

/** Dispatch a compile command
 * Input
    - args: &[String] - arguments after "compile"
 * Output
    - Result<(), String>
*/
pub(crate) fn compile(args: &[String]) -> Result<(), String> {
    let rest = || args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(rest(), &[], &["json"])?),
        Some("episode") => episode(
            args.get(1).cloned(),
            &Args::parse(
                args.get(2..).unwrap_or_default(),
                &[
                    "pack",
                    "digest",
                    "capability",
                    "name",
                    "purpose",
                    "task-class",
                    "intent",
                    "risk",
                    "profile",
                ],
                &["json"],
            )?,
        ),
        _ => Err(USAGE.into()),
    }
}

/** Print a report summary
 * Input
    - report: &CompilationReport - report
 * Output
    - None
*/
fn print_report(report: &CompilationReport) {
    println!(
        "{} {}: {:?}",
        report.report_id, report.capability_id, report.outcome
    );
    for reason in &report.reasons {
        println!("  - {reason}");
    }
    if let Some(candidate) = &report.candidate {
        println!("  candidate {} v{} digest {}{} (state: candidate; validation and publication are separate steps)", candidate.capability_id, candidate.version, candidate.digest, if candidate.reused { ", reused" } else { "" });
    }
    for step in &report.step_bases {
        println!(
            "  step {} [{:?}] from {}",
            step.step_id,
            step.basis,
            step.evidence_refs.join(", ")
        );
    }
    for gap in &report.gaps {
        println!(
            "  gap [{:?}{}] {}",
            gap.kind,
            if gap.blocking { ", blocking" } else { "" },
            gap.detail
        );
    }
    for warning in &report.warnings {
        println!("  warning: {warning}");
    }
}

/** Run the demonstration
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane compile demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    println!("Capability compiler demonstration (fixtures; no live service was contacted)\n");
    print_report(&report.success);
    println!(
        "  compiling again reused the stored version: {}",
        report.recompiled_reused
    );
    println!(
        "  state after a second person validated it: {:?}\n",
        report.state_after_validation
    );
    print_report(&report.failed);
    Ok(())
}

/** Compile a work episode of this repository
 * Input
    - episode_id: Option<String> - episode
    - args: &Args - pack, digest, capability, name, purpose, task class, intent, risk, profile
 * Output
    - Result<(), String>
*/
fn episode(episode_id: Option<String>, args: &Args) -> Result<(), String> {
    let episode_id = episode_id.filter(|id| !id.starts_with('-')).ok_or(USAGE)?;
    let required = |key: &str| {
        args.value(key)
            .ok_or_else(|| format!("'{key}' is required\n{USAGE}"))
    };
    let intent: EnterpriseIntent = match args.value("intent") {
        Some(path) => {
            serde_json::from_value(read_json(&path)?).map_err(|error| format!("{path}: {error}"))?
        }
        None => EnterpriseIntent::default(),
    };
    let risk: RiskLevel = serde_json::from_value(json!(args
        .value("risk")
        .unwrap_or_else(|| "unknown".into())))
    .map_err(|_| "risk must be low, medium, high, critical, or unknown".to_string())?;
    let request = CompileRequest {
        intake: IntakeRequest {
            schema_version: INTAKE_SCHEMA_VERSION,
            episode_id,
            pack_id: required("pack")?,
            pack_digest: required("digest")?,
            intent,
            risk,
            profile: args.value("profile"),
            idempotency_key: None,
            unresolved_assumptions: Vec::new(),
            human_interventions: Vec::new(),
        },
        capability_id: required("capability")?,
        name: required("name")?,
        purpose: required("purpose")?,
        task_class: required("task-class")?,
        proposals: Vec::new(),
    };
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let sources = Sources {
        capture: &context.capture,
        current: &context.current,
        reader: &context.reader,
    };
    let report = run_compile(
        &mut context.registry,
        &sources,
        &actor,
        &request,
        now_millis(),
    );
    let line = serde_json::to_string(&report).map_err(|error| error.to_string())?;
    append_line(
        &context.runtime.join("compiler").join("reports.jsonl"),
        &line,
    )?;
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
