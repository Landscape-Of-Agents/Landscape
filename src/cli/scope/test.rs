// crane scope test: positive and adversarial probes of the boundary through the hooks' guard
// (traversal, absolute, case, and symlink spellings of protected files, marker removal, shell
// changes, Crane metadata and commands, zone ceilings, session states, and documented gaps).
// Fails when a probe shows the boundary does not hold as the rules say; gaps are reported, not
// failed. With --skill, every zone and flow the skill depends on must also resolve.

use super::{name, print_json};
use crate::cli::args::Args;
use crate::scope::probe::{run, ProbeStatus};

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
    - Error when a probe fails
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = Args::parse(args, &["skill", "format", "request-id"], &["json"])?;
    args.expect_positional(0, "crane scope test [--skill SKILL[@N]] [--json]")?;
    let boundary = super::boundary()?;
    let report = run(&boundary);
    let failed = report.count(ProbeStatus::Fail);
    // With a skill: every zone and flow it depends on must still cover something
    let unresolved = match args.value("skill") {
        Some(reference) => super::skill::unresolved(&boundary, &reference)?,
        None => Vec::new(),
    };
    if crate::cli::protocol::wants_json(&args)? {
        print_json(&report)?;
    } else {
        if !report.integrity_ok {
            println!("Warning: the signed rules fail verification, so every agent change is denied now; the probes below test the rules as signed");
            for problem in &report.integrity {
                println!("  - {problem}");
            }
        }
        if let Some(error) = &report.zone_error {
            println!("Warning: zones and flows cannot be resolved ({error}); every agent change is denied now");
        }
        let mut category = "";
        for probe in &report.probes {
            if probe.category != category {
                category = probe.category;
                println!("\n{category}");
            }
            let decision = probe
                .decision
                .as_ref()
                .map(name)
                .unwrap_or_else(|| "-".into());
            println!("  {:<7} {} -> {decision}", name(&probe.status), probe.name);
            if probe.status != ProbeStatus::Pass {
                println!("          {}", probe.detail);
            }
        }
        println!(
            "\n{} passed, {failed} failed, {} gaps (limits of repository scope, not sandboxed), {} not run",
            report.count(ProbeStatus::Pass),
            report.count(ProbeStatus::Gap),
            report.count(ProbeStatus::NotRun)
        );
    }
    for entity in &unresolved {
        eprintln!("crane: the skill depends on {entity}, which covers nothing now (unresolved scope is never allowed)");
    }
    if failed > 0 {
        return Err(format!(
            "{failed} scope probes failed: the boundary does not hold as the rules say"
        ));
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "{} of the skill's zone and flow dependencies are unresolved",
            unresolved.len()
        ));
    }
    Ok(())
}
