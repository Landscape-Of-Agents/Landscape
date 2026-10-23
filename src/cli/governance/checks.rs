// Verification commands: validate (integrity, syntax, and markers) and test (run every preserve and target command of every policy), with their human-readable and JSON reports.

use crate::cli::args::Args;
use crate::governance::policy::render_statement;
use crate::governance::state::commit;
use crate::governance::verify::{evaluate, Options, Outcome, Report};
use crate::governance::workspace::Workspace;
use crate::governance::{Finding, Severity};
use crate::platform::process;
use crate::trust::agent_environment;
use serde_json::json;
use std::time::Duration;

/** Print findings with severity tags
 * Input
    - findings: &[Finding] - findings to print
 * Output
    - None (writes to stdout)
*/
pub(super) fn print_findings(findings: &[Finding]) {
    for finding in findings {
        let tag = match finding.severity {
            Severity::Critical => "CRITICAL",
            Severity::High => "ERROR",
            Severity::Medium => "WARNING",
            Severity::Low => "NOTE",
        };
        println!("  {tag} [{}] {}", finding.code, finding.message);
    }
}

/** Validate the policies and governance state: syntax, policy files outside .crane/policies,
 * checkpoints, the signed registry and map, defaults, policy-registry consistency, and anchor
 * resolution; a human run also records selections that moved to another file as a new registry
 * generation
 * Input
    - args: &[String] - optional --json
 * Output
    - Result<(), String>
    - Error if any check fails
*/
pub(crate) fn validate(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["json"])?;
    parsed.expect_positional(0, "crane validate [--json]")?;
    let workspace = Workspace::locate()?;
    let (report, state) = evaluate(&workspace, &Options::default())?;
    let blocking = report.has_blocking_findings();
    let mut recorded = Vec::new();
    if !blocking
        && !report.relocations.is_empty()
        && agent_environment().is_none()
        && workspace.trust()?.signing_public_key()?.is_some()
        // A passphrase key needs a console; editors run validate without one
        && (!workspace.trust()?.signing_key_prompts()?
            || std::io::IsTerminal::is_terminal(&std::io::stdin()))
    {
        let relocations = report.relocations.clone();
        let detail = json!({"relocations": relocations});
        commit(
            &workspace,
            "crane validate",
            "selection.relocated",
            detail,
            |state, generation| {
                for relocation in &relocations {
                    if let Some(record) = state
                        .registry
                        .selections
                        .iter_mut()
                        .find(|record| record.id == relocation.id)
                    {
                        if let Some(Some(span)) = report.resolutions.get(&record.id) {
                            record.current.span = span.clone();
                            record.current.resolved_at_generation = generation;
                            record.generation += 1;
                        }
                    }
                }
                Ok(())
            },
        )?;
        recorded = report.relocations.clone();
    }
    if parsed.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "valid": !blocking,
                "generation": report.generation,
                "findings": report.findings,
                "selections": report.resolutions,
                "relocations_recorded": recorded,
                "files_scanned": report.files_scanned,
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        let policies = report.policies.len();
        let commands = report
            .policies
            .iter()
            .map(|policy| policy.commands.len())
            .sum::<usize>();
        println!(
            "Crane validate: {policies} policies, {commands} commands, {} selections, {} checkpoints, registry generation {} ({} files scanned)",
            state.registry.selections.len(),
            state.checkpoints.checkpoints.len(),
            report.generation,
            report.files_scanned
        );
        print_findings(&report.findings);
        for relocation in &recorded {
            println!(
                "  recorded relocation of {}: {} -> {}",
                relocation.id, relocation.from, relocation.to
            );
        }
        if !blocking && !report.relocations.is_empty() && recorded.is_empty() {
            for relocation in &report.relocations {
                println!(
                    "  NOTE selection {} moved {} -> {} (a human run of crane validate records it)",
                    relocation.id, relocation.from, relocation.to
                );
            }
        }
        println!("Crane validate: {}", if blocking { "FAIL" } else { "PASS" });
    }
    if blocking {
        Err("validation failed".into())
    } else {
        Ok(())
    }
}

/** Print a report's policy results
 * Input
    - report: &Report - evaluation
    - only: Option<&str> - limit to one policy
 * Output
    - None (writes to stdout)
*/
pub(super) fn print_policies(report: &Report, only: Option<&str>) {
    for policy in report
        .policies
        .iter()
        .filter(|policy| only.is_none_or(|name| name == policy.name))
    {
        println!(
            "{} policy {} ({}, {} commands)",
            if policy.passed { "PASS" } else { "FAIL" },
            policy.name,
            policy.file,
            policy.commands.len()
        );
        for command in &policy.commands {
            let tag = match command.outcome {
                Outcome::Pass => "PASS",
                Outcome::Fail => "FAIL",
                Outcome::Unresolved => "UNRESOLVED",
                Outcome::Pending => "PENDING",
            };
            println!(
                "  {tag} {}: {}",
                render_statement(command.operation, &command.id, command.change_type)
                    .trim_end_matches(';'),
                command.message
            );
        }
    }
}

/** Run the smoke tests: evaluate every policy (a policy fails if any command fails) and then run
 * the commands listed under "tests" in .crane/config.json
 * Input
    - args: &[String] - "." and optional --json
 * Output
    - Result<(), String>
    - Error if any policy, check, or test command fails
*/
pub(crate) fn test(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["json"])?;
    if parsed.positional != ["."] {
        return Err("usage: crane test . [--json]".into());
    }
    let workspace = Workspace::locate()?;
    let (report, state) = evaluate(&workspace, &Options::default())?;
    let mut tests = Vec::new();
    if report.passed {
        for test in &state.config.tests {
            let output = process::run(
                process::shell(&test.command).current_dir(&workspace.root),
                Duration::from_secs(test.timeout_seconds),
            )?;
            tests.push(json!({
                "name": test.name,
                "command": test.command,
                "passed": output.success,
                "timed_out": output.timed_out,
                "exit_code": output.code,
                "duration_ms": output.duration_ms,
            }));
        }
    }
    let tests_passed = tests.iter().all(|test| test["passed"] == true);
    let passed = report.passed && tests_passed;
    // Each rule's and each test command's result, for the command's event (DASHBOARD.md F19)
    for policy in &report.policies {
        for command in &policy.commands {
            crate::platform::notes::note(
                "POLICY_RULE_RESULT",
                json!({
                    "policy": policy.name,
                    "id": command.id,
                    "operation": command.operation.name(),
                    "change_type": command.change_type,
                    "outcome": command.outcome,
                    "message": command.message,
                }),
            );
        }
    }
    for test in &tests {
        crate::platform::notes::note(
            "PROJECT_TEST_RESULT",
            json!({
                "name": test["name"],
                "outcome": if test["timed_out"] == true { "TIMEOUT" } else if test["passed"] == true { "PASS" } else { "FAIL" },
                "exit_code": test["exit_code"],
                "duration_ms": test["duration_ms"],
            }),
        );
    }
    if parsed.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "passed": passed,
                "generation": report.generation,
                "findings": report.findings,
                "policies": report.policies,
                "tests": tests,
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        print_findings(&report.findings);
        print_policies(&report, None);
        for test in &tests {
            println!(
                "{} test {} ({} ms)",
                if test["passed"] == true {
                    "PASS"
                } else {
                    "FAIL"
                },
                test["name"].as_str().unwrap_or_default(),
                test["duration_ms"]
            );
        }
        if !report.passed && !state.config.tests.is_empty() {
            println!("  test commands were not run because the policies failed");
        }
        println!("Crane test: {}", if passed { "PASS" } else { "FAIL" });
    }
    if passed {
        Ok(())
    } else {
        Err("one or more Crane policies or tests failed".into())
    }
}
