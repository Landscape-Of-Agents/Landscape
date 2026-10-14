// Agent commands: install, uninstall, and validate supported provider hooks, and the internal
// 'agent hook' entry point the installed hooks run.

use super::args::Args;
use crate::hooks::adapter::{AgentKind, HookEvent};
use crate::hooks::{install, runtime};

/** Dispatch an agent command
 * Input
    - args: &[String] - arguments after "agent"
 * Output
    - Result<(), String>
*/
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let (command, rest) = args.split_first().ok_or(
        "usage: crane agent install|uninstall|hooks --profile claude|codex|copilot|gemini",
    )?;
    match command.as_str() {
        "install" => install_hooks(&profile(rest)?),
        "uninstall" => uninstall_hooks(&profile(rest)?),
        "hooks" => validate_hooks(&profile(rest)?),
        "hook" => hook(rest),
        "sync" => sync(),
        "link" => super::link::link(rest),
        other => Err(format!(
            "unknown agent command '{other}'; use install, uninstall, hooks, or link"
        )),
    }
}

/** Read the required --profile option
 * Input
    - args: &[String] - arguments
 * Output
    - Result<AgentKind, String>
    - Error if it is missing or not claude or codex
*/
fn profile(args: &[String]) -> Result<AgentKind, String> {
    let parsed = Args::parse(args, &["profile"], &[])?;
    parsed.expect_positional(
        0,
        "crane agent install|uninstall|hooks --profile claude|codex|copilot|gemini",
    )?;
    let kind = AgentKind::parse(
        &parsed
            .value("profile")
            .ok_or("--profile claude|codex|copilot|gemini is required")?,
    )?;
    if kind == AgentKind::Generic {
        return Err("hooks are installed for --profile claude, codex, copilot, or gemini".into());
    }
    Ok(kind)
}

/** Install a provider's hooks, merging them into its project configuration
 * Input
    - kind: &AgentKind - provider
 * Output
    - Result<(), String>
*/
fn install_hooks(kind: &AgentKind) -> Result<(), String> {
    let added = install::install(*kind)?;
    if added.is_empty() {
        println!("Crane {} hooks are already installed", kind.display());
    } else {
        println!(
            "Installed Crane {} hooks for {}",
            kind.display(),
            added.join(", ")
        );
    }
    println!("Every pre- and post-tool hook now passes through Crane; check them with 'crane agent hooks --profile {}'.", kind.name());
    if *kind == AgentKind::Codex {
        println!("Codex runs project hooks only after they are trusted: open Codex in this repository and review them with /hooks.");
    }
    if *kind == AgentKind::Gemini {
        println!("Gemini CLI may ask you to trust the project hooks before running them; review the displayed hook commands.");
    }
    Ok(())
}

/** Remove a provider's Crane hooks (refused for agents)
 * Input
    - kind: &AgentKind - provider
 * Output
    - Result<(), String>
*/
fn uninstall_hooks(kind: &AgentKind) -> Result<(), String> {
    let removed = install::uninstall(*kind)?;
    if removed == 0 {
        println!(
            "Crane {} hooks are not installed; nothing changed",
            kind.display()
        );
    } else {
        println!(
            "Removed {removed} Crane hook entries for {}; other settings were kept",
            kind.display()
        );
    }
    Ok(())
}

/** Validate a provider's hooks and exit with an error when they are not valid
 * Input
    - kind: &AgentKind - provider
 * Output
    - Result<(), String>
    - Error listing the problems
*/
fn validate_hooks(kind: &AgentKind) -> Result<(), String> {
    let report = install::validate(*kind)?;
    println!(
        "{} hooks ({}): {}",
        kind.display(),
        report["file"].as_str().unwrap_or_default(),
        if report["valid"] == true {
            "valid"
        } else {
            "NOT VALID"
        }
    );
    for event in report["events"].as_array().into_iter().flatten() {
        println!(
            "  {:<18} registered {}",
            event["event"].as_str().unwrap_or_default(),
            event["registered"]
        );
    }
    for warning in report["warnings"].as_array().into_iter().flatten() {
        println!("  note: {}", warning.as_str().unwrap_or_default());
    }
    let problems = report["problems"].as_array().cloned().unwrap_or_default();
    for problem in &problems {
        println!("  problem: {}", problem.as_str().unwrap_or_default());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("{} hooks are not valid", kind.display()))
    }
}

/** Run a hook (internal: the command the installed hooks execute)
 * Input
    - args: &[String] - --event EVENT --profile PROFILE
 * Output
    - Result<(), String>
*/
fn hook(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &["event", "profile"], &[])?;
    let event = HookEvent::parse(&parsed.value("event").ok_or("--event is required")?)?;
    let kind = AgentKind::parse(&parsed.value("profile").unwrap_or_else(|| "generic".into()))?;
    runtime::run(kind, event)
}

/** Upload the local evidence to the Foxx ingestion service now (internal: started detached by
 * hooks, commands, and the periodic sync)
 * Input
    - None
 * Output
    - Result<(), String>
    - Error if uploads are off or the upload fails
*/
fn sync() -> Result<(), String> {
    let workspace = crate::governance::workspace::Workspace::locate()?;
    if !crate::telemetry::sync::configured() {
        return Err("uploads are off (CRANE_INGEST_URL is 'off')".into());
    }
    let stores = crate::telemetry::Stores::open(&workspace.trust()?.runtime());
    // Hooks and commands start background syncs (marked by the trigger); a person runs a manual one
    let trigger = if std::env::var("CRANE_SYNC_BACKGROUND").is_ok_and(|value| value == "1") {
        crate::telemetry::ingest::Trigger::Background
    } else {
        crate::telemetry::ingest::Trigger::Manual
    };
    let flushed = crate::telemetry::ingest::sync(&workspace, &stores, trigger)?;
    if flushed.skipped {
        println!("Another upload is running; this one was skipped");
        return Ok(());
    }
    println!(
        "Synchronized {} events, {} ledger entries, {} governance changes, {} alerts ({} already present{})",
        flushed.events,
        flushed.ledger,
        flushed.audit,
        flushed.alerts,
        flushed.duplicates,
        if flushed.reset { "; re-sent records missing from the service" } else { "" }
    );
    Ok(())
}
