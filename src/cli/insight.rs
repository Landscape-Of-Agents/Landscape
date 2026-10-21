// Insight commands: 'crane insight report' finds patterns across the work Work Capture kept, under
// the access of the person running Crane, and keeps the report; 'reports' lists the kept reports;
// 'propose' turns a pattern into a proposal (a recipe proposal, or system-derived context for a
// zone) that stays proposed until a person reviews it; 'recipes' lists recipe proposals; and
// 'review' accepts or rejects one (owner and admins only), which publishes an accepted recipe as a
// new version.

use std::collections::BTreeSet;

use serde_json::json;

use super::args::Args;
use super::capture::{local_broker, local_policy};
use crate::capture::broker::proposals::ProposalStatus;
use crate::capture::broker::run::EvidenceQueryBroker;
use crate::capture::{AccessPolicy, Clock};
use crate::governance::workspace::Workspace;
use crate::insight::propose::{propose as propose_pattern, Proposed};
use crate::insight::{self, Destination, InsightReport, Pattern, MIN_SUPPORT};

/** Dispatch an insight command
 * Input
    - args: &[String] - arguments after "insight"
 * Output
    - Result<(), String>
*/
pub(crate) fn insight(args: &[String]) -> Result<(), String> {
    let usage = "usage: crane insight report [--json] | reports [--json] | propose PATTERN_ID | recipes [--json] | review PROPOSAL_ID accept|reject reason TEXT";
    let id = |position: usize| {
        args.get(position)
            .filter(|word| !word.starts_with('-'))
            .cloned()
            .ok_or_else(|| usage.to_string())
    };
    match args.first().map(String::as_str) {
        Some("report") => report(&Args::parse(&args[1..], &[], &["json"])?),
        Some("reports") => reports(&Args::parse(&args[1..], &[], &["json"])?),
        Some("propose") => propose(&id(1)?),
        Some("recipes") => recipes(&Args::parse(&args[1..], &[], &["json"])?),
        Some("review") => {
            let decision = id(2)?;
            review(
                &id(1)?,
                &decision,
                &Args::parse(&args[3..], &["reason"], &[])?,
            )
        }
        _ => Err(usage.into()),
    }
}

/** The question classes the broker's recipes serve
 * Input
    - broker: &EvidenceQueryBroker - broker
 * Output
    - BTreeSet<String>
*/
fn served(broker: &EvidenceQueryBroker) -> BTreeSet<String> {
    broker
        .recipes
        .all()
        .flat_map(|recipe| recipe.investigation_classes.iter().cloned())
        .collect()
}

/** Mine the store as it is now, under the person's access
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<(EvidenceQueryBroker, AccessPolicy, InsightReport), String>
*/
fn current(
    workspace: &Workspace,
) -> Result<(EvidenceQueryBroker, AccessPolicy, InsightReport), String> {
    let broker = local_broker(workspace, Clock::system())?;
    let policy = local_policy(workspace, &broker)?;
    let report = insight::report(
        &broker.store,
        &policy,
        &served(&broker),
        crate::platform::now_millis(),
    );
    Ok((broker, policy, report))
}

/** Print a value as pretty JSON
 * Input
    - value: &serde_json::Value - value
 * Output
    - Result<(), String>
*/
fn print_json(value: &serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

/** Where a person can take a pattern, in words
 * Input
    - pattern: &Pattern - pattern
 * Output
    - String
*/
fn next_step(pattern: &Pattern) -> String {
    match &pattern.destination {
        Destination::Context { zone, .. } => format!(
            "propose it as context for zone {zone}: crane insight propose {}",
            pattern.pattern_id
        ),
        Destination::Recipe { class } => format!(
            "propose a recipe for '{class}': crane insight propose {}",
            pattern.pattern_id
        ),
        Destination::Capability { episode } => format!(
            "compile a capability candidate from work episode {episode} (crane compile episode)"
        ),
        Destination::Governance => {
            "for the owner or an admin: the zone's agent permissions could be reviewed".into()
        }
        Destination::Report => "for people to act on".into(),
    }
}

/** Find patterns now, keep the report, and print it
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn report(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane insight report [--json]")?;
    let workspace = Workspace::locate()?;
    let (_, _, report) = current(&workspace)?;
    insight::store::keep(&workspace.trust()?.runtime().join("insight"), &report)?;
    crate::platform::notes::note(
        "INSIGHT_REPORT",
        json!({
            "report_id": report.report_id,
            "patterns": report.patterns.len(),
            "suppressed": report.suppressed,
            "considered": report.considered,
            "miners_version": report.miners_version,
        }),
    );
    if args.flag("json") {
        return print_json(&json!(report));
    }
    println!(
        "Insight report {} ({} patterns; {} smaller candidates below the minimum support of {MIN_SUPPORT} are not shown)",
        report.report_id,
        report.patterns.len(),
        report.suppressed
    );
    println!(
        "  read: {} Evidence Packs, {} investigations, {} work episodes you may see",
        report.considered.packs, report.considered.audits, report.considered.episodes
    );
    for pattern in &report.patterns {
        println!("\n[{:?}] {}", pattern.kind, pattern.pattern_id);
        println!("  {}", pattern.statement);
        println!("  next: {}", next_step(pattern));
    }
    Ok(())
}

/** List the kept reports
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn reports(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane insight reports [--json]")?;
    let workspace = Workspace::locate()?;
    let kept = insight::store::reports(&workspace.trust()?.runtime().join("insight"))?;
    if args.flag("json") {
        return print_json(&json!(kept));
    }
    for report in &kept {
        println!(
            "{}  {} patterns  {} suppressed  by {} at {}",
            report.report_id,
            report.patterns.len(),
            report.suppressed,
            report.viewer.id,
            report.generated_at
        );
    }
    Ok(())
}

/** Turn a pattern of the current report into a proposal
 * Input
    - pattern_id: &str - pattern
 * Output
    - Result<(), String>
*/
fn propose(pattern_id: &str) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let (mut broker, _, report) = current(&workspace)?;
    let pattern = report
        .patterns
        .iter()
        .find(|pattern| pattern.pattern_id == pattern_id)
        .ok_or_else(|| {
            format!("no pattern '{pattern_id}' in the current report; run 'crane insight report' again (a pattern's id changes when its counts change)")
        })?;
    let proposed = propose_pattern(&mut broker, pattern, crate::platform::now_millis())?;
    broker.store.save()?;
    match proposed {
        Proposed::Recipe(id) => println!(
            "Proposed recipe {id}; it is used only after the owner or an admin accepts it (crane insight review {id} accept reason TEXT)"
        ),
        Proposed::Context(id) => println!(
            "Proposed context {id}; it stays proposed until a person reviews it (crane capture review {id} accept|reject reason TEXT)"
        ),
    }
    Ok(())
}

/** List recipe proposals, and accepted recipes the RecipeBook could not take
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn recipes(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane insight recipes [--json]")?;
    let workspace = Workspace::locate()?;
    let broker = local_broker(&workspace, Clock::system())?;
    let mut proposals = broker.store.recipe_proposals.values().collect::<Vec<_>>();
    proposals.sort_by_key(|proposal| (proposal.created_at, proposal.proposal_id.clone()));
    if args.flag("json") {
        return print_json(&json!({"proposals": proposals, "shadowed": broker.shadowed}));
    }
    for proposal in &proposals {
        println!(
            "{}  {} v{}  {:?}\n  {}",
            proposal.proposal_id,
            proposal.recipe.recipe_id,
            proposal.recipe.version,
            proposal.status,
            proposal.rationale
        );
    }
    for shadowed in &broker.shadowed {
        println!("not in use: {shadowed}");
    }
    Ok(())
}

/** Accept or reject a recipe proposal: only the repository owner or an admin, never the proposer
 * Input
    - id: &str - proposal
    - decision: &str - accept or reject
    - args: &Args - reason
 * Output
    - Result<(), String>
*/
fn review(id: &str, decision: &str, args: &Args) -> Result<(), String> {
    let usage = "crane insight review PROPOSAL_ID accept|reject reason TEXT";
    args.expect_positional(0, usage)?;
    let decision = match decision {
        "accept" => ProposalStatus::Accepted,
        "reject" => ProposalStatus::Rejected,
        _ => return Err(format!("usage: {usage}")),
    };
    let reason = args
        .value("reason")
        .ok_or_else(|| format!("usage: {usage}"))?;
    let workspace = Workspace::locate()?;
    let mut broker = local_broker(&workspace, Clock::system())?;
    let policy = local_policy(&workspace, &broker)?;
    broker.review_recipe(&policy, id, decision, &reason)?;
    broker.store.save()?;
    println!("Recipe proposal {id}: {decision:?}");
    Ok(())
}
