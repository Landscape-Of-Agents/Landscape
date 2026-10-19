// Evolution commands. They read the repository's capability registry, Work Capture records, and
// invocation ledger (never written here), count DataSea lifecycle events, and keep their own
// records in the trust directory's runtime/evolution folder. Roles come from the same Crane
// grants as the capability commands. 'analyze' and 'drift' never create a candidate; 'propose'
// stores a candidate through the registry only when the gate allows it; regression runs store
// their Lab reports in the Lab's own store; 'prepare-review' writes a review packet and delivers
// nothing. A regression that is not passed, and a blocked proposal, exit with an error after
// printing the record.

use std::collections::BTreeSet;

use super::args::Args;
use super::capability::{read_json, Context};
use crate::capability::refs::Sources;
use crate::capability_lab::report::LabReport;
use crate::capability_lab::store::LabStore;
use crate::capture::Clock;
use crate::evolution::demo;
use crate::evolution::models::{
    Change, Proposal, ProposalState, RegressionRun, RunState, Signal, Thresholds,
};
use crate::evolution::outcomes::count_events;
use crate::evolution::reports::EvolutionStore;
use crate::evolution::workflow::{name, Evolution, SourceInfo};
use crate::invocation::engine::InvocationRecord;
use crate::invocation::ledger::Ledger;
use crate::telemetry::events::EventStore;

/** The command reference of 'crane evolution' */
const USAGE: &str = "usage: crane evolution demo [--json]
       crane evolution analyze ID [version N] [--json]
       crane evolution drift ID [version N] [--json]
       crane evolution signals [capability ID] [status STATUS] [--json]
       crane evolution signal inspect SIGNAL_ID [--json]
       crane evolution signal dismiss SIGNAL_ID reason TEXT [--json]
       crane evolution propose SIGNAL_ID [changes FILE] [--json]
       crane evolution proposals [capability ID] [status STATUS] [--json]
       crane evolution proposal inspect PROPOSAL_ID [--json]
       crane evolution proposal abandon PROPOSAL_ID reason TEXT [--json]
       crane evolution regression plan|run|history PROPOSAL_ID [--json]
       crane evolution regression inspect RUN_ID [--json]
       crane evolution prepare-review PROPOSAL_ID [--json]";

/** Dispatch an evolution command
 * Input
    - args: &[String] - arguments after "evolution"
 * Output
    - Result<(), String>
*/
pub(crate) fn evolution(args: &[String]) -> Result<(), String> {
    let rest = |from: usize| args.get(from..).unwrap_or_default();
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["demo", ..] => demo_command(&Args::parse(rest(1), &[], &["json"])?),
        ["analyze" | "drift", ..] => {
            analyze(words[0], &Args::parse(rest(1), &["version"], &["json"])?)
        }
        ["signals", ..] => list(
            "signals",
            &Args::parse(rest(1), &["capability", "status"], &["json"])?,
        ),
        ["proposals", ..] => list(
            "proposals",
            &Args::parse(rest(1), &["capability", "status"], &["json"])?,
        ),
        ["signal", command @ ("inspect" | "dismiss"), ..] => {
            signal(command, &Args::parse(rest(2), &["reason"], &["json"])?)
        }
        ["propose", ..] => propose(&Args::parse(rest(1), &["changes"], &["json"])?),
        ["proposal", command @ ("inspect" | "abandon"), ..] => {
            proposal(command, &Args::parse(rest(2), &["reason"], &["json"])?)
        }
        ["regression", command @ ("plan" | "run" | "inspect" | "history"), ..] => {
            regression(command, &Args::parse(rest(2), &[], &["json"])?)
        }
        ["prepare-review", ..] => prepare(&Args::parse(rest(1), &[], &["json"])?),
        _ => Err(USAGE.into()),
    }
}

/** Print a value as pretty JSON
 * Input
    - value: &impl serde::Serialize - value
 * Output
    - Result<(), String>
*/
fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

/** Run the demonstration
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane evolution demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        print_json(&report)?;
    } else {
        println!("Capability evolution demonstration\n{}\n", report.notice);
        for case in &report.cases {
            println!(
                "{:>2}. {} [{}]\n    expected: {}\n    actual:   {}",
                case.case,
                case.name,
                if case.matched {
                    "as expected"
                } else {
                    "MISMATCH"
                },
                case.expected,
                case.actual
            );
        }
    }
    if report.all_matched {
        Ok(())
    } else {
        Err("a demonstration case did not match its expectation".into())
    }
}

/** What a repository evolution command needs
 * Fields
    - context: Context - registry, capture store, versions, actor, access
    - records: Vec<InvocationRecord> - invocation ledger records (read only)
    - events: std::collections::BTreeMap<String, usize> - lifecycle events per invocation
    - info: SourceInfo - source availability
    - compilation: Vec<CompilationReport> - verified compilation reports
    - store: EvolutionStore - evolution store
    - clock: Clock - system clock
*/
struct Local {
    context: Context,
    records: Vec<InvocationRecord>,
    events: std::collections::BTreeMap<String, usize>,
    info: SourceInfo,
    compilation: Vec<crate::compiler::assemble::CompilationReport>,
    store: EvolutionStore,
    clock: Clock,
}

impl Local {
    /** Open the repository's evolution context
     * Input
        - None
     * Output
        - Result<Local, String>
    */
    fn open() -> Result<Self, String> {
        let context = Context::open()?;
        let ledger = Ledger::open(&context.runtime.join("invocation"))?;
        let events = EventStore::open(&context.runtime).read_all();
        let info = SourceInfo {
            ledger_available: true,
            events_available: events.is_ok(),
            synthetic: BTreeSet::new(),
            label: "this repository's invocation ledger (local adapters only)".into(),
        };
        Ok(Self {
            records: ledger.state.records.into_values().collect(),
            events: events
                .map(|events| count_events(&events))
                .unwrap_or_default(),
            info,
            compilation: super::invoke::compilation_reports(&context.runtime)?,
            store: EvolutionStore::open(&context.runtime.join("evolution"))?,
            clock: Clock::system(),
            context,
        })
    }

    /** The evolution operations
     * Input
        - None (uses self)
     * Output
        - Evolution
    */
    fn evolution(&mut self) -> Evolution<'_> {
        Evolution {
            registry: &mut self.context.registry,
            sources: Sources {
                capture: &self.context.capture,
                current: &self.context.current,
                reader: &self.context.reader,
            },
            invocations: &self.records,
            events: &self.events,
            info: &self.info,
            compilation_reports: &self.compilation,
            store: &mut self.store,
            clock: &self.clock,
        }
    }
}

/** The optional version argument
 * Input
    - args: &Args - arguments
 * Output
    - Result<Option<u32>, String>
*/
fn version(args: &Args) -> Result<Option<u32>, String> {
    args.value("version")
        .map(|value| {
            value
                .parse()
                .map_err(|_| "version must be a positive integer".to_string())
        })
        .transpose()
}

/** Print a signal summary
 * Input
    - signal: &Signal - signal
 * Output
    - None
*/
fn print_signal(signal: &Signal) {
    println!(
        "{} {} v{} {}: {} ({} independent, {} contradicting)",
        signal.signal_id,
        signal.capability_id,
        signal.version,
        name(&signal.kind),
        name(&signal.state),
        signal.independent_count,
        signal.contradicting.len()
    );
    println!("  context: {}", signal.context_key);
    if let Some(qualification) = &signal.qualification {
        for rule in &qualification.rules {
            println!(
                "  rule {} {}: {}",
                rule.rule,
                name(&rule.status),
                rule.detail
            );
        }
    }
    if let Some(hypothesis) = &signal.hypothesis {
        println!(
            "  hypothesis: {}\n  causality: {}\n  recommendation: {}",
            hypothesis.statement, hypothesis.causality, hypothesis.recommendation
        );
    }
    for missing in &signal.missing_evidence {
        println!("  missing evidence: {missing}");
    }
}

/** Analyze a version, or report its drift
 * Input
    - command: &str - analyze or drift
    - args: &Args - capability and version
 * Output
    - Result<(), String>
*/
fn analyze(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(1, &format!("crane evolution {command} ID [version N]"))?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let id = args.positional[0].clone();
    let version = version(args)?;
    if command == "drift" {
        let report = local.evolution().drift(&actor, &id, version)?;
        if args.flag("json") {
            return print_json(&report);
        }
        println!(
            "{} v{} (digest {}, {}): {}",
            report.capability_id,
            report.version,
            report.digest,
            name(&report.state),
            report.summary
        );
        for finding in &report.findings {
            println!(
                "  {} {} pinned {} observed {}: {} - {}",
                finding.kind,
                finding.target_id,
                finding.pinned.as_deref().unwrap_or("-"),
                finding.observed.as_deref().unwrap_or("unknown"),
                name(&finding.status),
                finding.impact
            );
        }
        return Ok(());
    }
    let analysis = local
        .evolution()
        .analyze(&actor, &id, version, Thresholds::default())?;
    if args.flag("json") {
        return print_json(&analysis);
    }
    let report = &analysis.report;
    let coverage = &report.coverage;
    println!(
        "Analysis {} of {} v{} (digest {}){}",
        report.report_id,
        report.capability_id,
        report.version,
        report.capability_digest,
        if analysis.reused {
            " - identical inputs; the stored analysis is returned"
        } else {
            ""
        }
    );
    println!(
        "  records: {} visible, {} of other requesters not read, {} other versions, {} other digests",
        coverage.records_visible, coverage.records_not_visible, coverage.other_versions, coverage.other_digest
    );
    println!(
        "  independent trials {}; resubmissions collapsed {}; retries excluded {}; events correlated {} (never counted as trials)",
        coverage.independent, coverage.duplicates_collapsed, coverage.retry_attempts_excluded, coverage.events_correlated
    );
    for (class, count) in &report.outcome_counts {
        println!("  {}: {count}", name(class));
    }
    for source in &coverage.sources {
        println!(
            "  source {}: {} ({})",
            source.source,
            if source.available {
                "available"
            } else {
                "unavailable"
            },
            source.detail
        );
    }
    println!("  drift: {}", report.drift.summary);
    for signal in &analysis.signals {
        print_signal(signal);
    }
    println!("Decision: {}", report.decision);
    for limitation in &report.limitations {
        println!("  limitation: {limitation}");
    }
    Ok(())
}

/** List signals or proposals
 * Input
    - what: &str - signals or proposals
    - args: &Args - filters
 * Output
    - Result<(), String>
*/
fn list(what: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(
        0,
        &format!("crane evolution {what} [capability ID] [status STATUS]"),
    )?;
    let local = Local::open()?;
    let capability = args.value("capability");
    let status = args.value("status");
    let keep = |id: &str, state: String| {
        capability.as_deref().is_none_or(|wanted| wanted == id)
            && status.as_deref().is_none_or(|wanted| wanted == state)
    };
    let rows = if what == "signals" {
        local.store.state.signals.values().filter(|signal| keep(&signal.capability_id, name(&signal.state))).map(|signal| serde_json::json!({"id": signal.signal_id, "capability_id": signal.capability_id, "version": signal.version, "kind": signal.kind, "state": signal.state, "independent": signal.independent_count, "updated_at": signal.updated_at})).collect::<Vec<_>>()
    } else {
        local.store.state.proposals.values().filter(|proposal| keep(&proposal.capability_id, name(&proposal.state))).map(|proposal| serde_json::json!({"id": proposal.proposal_id, "capability_id": proposal.capability_id, "parent_version": proposal.parent_version, "candidate": proposal.candidate, "state": proposal.state, "updated_at": proposal.updated_at})).collect::<Vec<_>>()
    };
    if args.flag("json") {
        return print_json(&rows);
    }
    if rows.is_empty() {
        println!("No {what}.");
    }
    for row in rows {
        println!(
            "{} {}: {}",
            row["id"].as_str().unwrap_or_default(),
            row["capability_id"].as_str().unwrap_or_default(),
            row["state"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/** Inspect or dismiss a signal
 * Input
    - command: &str - inspect or dismiss
    - args: &Args - signal id and reason
 * Output
    - Result<(), String>
*/
fn signal(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(1, &format!("crane evolution signal {command} SIGNAL_ID"))?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let id = &args.positional[0];
    let signal = if command == "inspect" {
        local.store.signal(id)?
    } else {
        let reason = args.value("reason").ok_or("'reason' is required")?;
        local.evolution().dismiss(&actor, id, &reason)?
    };
    if args.flag("json") {
        return print_json(&signal);
    }
    print_signal(&signal);
    Ok(())
}

/** Print a proposal summary
 * Input
    - proposal: &Proposal - proposal
 * Output
    - None
*/
fn print_proposal(proposal: &Proposal) {
    println!(
        "{} {} v{} (digest {}) from {}: {}",
        proposal.proposal_id,
        proposal.capability_id,
        proposal.parent_version,
        proposal.parent_digest,
        proposal.signal_id,
        name(&proposal.state)
    );
    if let Some(candidate) = &proposal.candidate {
        println!(
            "  candidate v{} (digest {}); its registry state is unchanged by evolution",
            candidate.version, candidate.digest
        );
    }
    for (index, change) in proposal.changes.iter().enumerate() {
        let decision = proposal
            .gate
            .iter()
            .find(|decision| decision.index == index);
        println!(
            "  change {} {} ({}): {}",
            index + 1,
            serde_json::to_string(&change.change).unwrap_or_default(),
            if decision.is_some_and(|decision| decision.allowed) {
                "allowed"
            } else {
                "refused"
            },
            decision
                .map(|decision| decision.reason.as_str())
                .unwrap_or_default()
        );
    }
    if let Some(transition) = proposal.transitions.last() {
        println!("  last transition: {}", transition.reason);
    }
    if let Some(packet) = &proposal.review {
        for note in &packet.notes {
            println!("  note: {note}");
        }
        for step in &packet.next_steps {
            println!("  next: {step}");
        }
    }
}

/** Propose a revision for a signal
 * Input
    - args: &Args - signal id and optional changes file
 * Output
    - Result<(), String>
*/
fn propose(args: &Args) -> Result<(), String> {
    args.expect_positional(1, "crane evolution propose SIGNAL_ID [changes FILE]")?;
    let suggestions: Vec<Change> = match args.value("changes") {
        Some(path) => serde_json::from_value(read_json(&path)?)
            .map_err(|error| format!("changes: {error}"))?,
        None => Vec::new(),
    };
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let proposal = local
        .evolution()
        .propose(&actor, &args.positional[0], suggestions)?;
    if args.flag("json") {
        print_json(&proposal)?;
    } else {
        print_proposal(&proposal);
    }
    if proposal.state == ProposalState::Blocked {
        return Err(format!("proposal {} is blocked", proposal.proposal_id));
    }
    Ok(())
}

/** Inspect or abandon a proposal
 * Input
    - command: &str - inspect or abandon
    - args: &Args - proposal id and reason
 * Output
    - Result<(), String>
*/
fn proposal(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        &format!("crane evolution proposal {command} PROPOSAL_ID"),
    )?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let id = &args.positional[0];
    let proposal = if command == "inspect" {
        local.store.proposal(id)?
    } else {
        let reason = args.value("reason").ok_or("'reason' is required")?;
        local.evolution().abandon(&actor, id, &reason)?
    };
    if args.flag("json") {
        return print_json(&proposal);
    }
    print_proposal(&proposal);
    Ok(())
}

/** Print a regression run
 * Input
    - run: &RegressionRun - run
 * Output
    - None
*/
fn print_run(run: &RegressionRun) {
    println!(
        "{} for {} ({} v{} {} against v{} {}): {}",
        run.run_id,
        run.proposal_id,
        run.capability_id,
        run.candidate_version,
        run.candidate_digest,
        run.parent_version,
        run.parent_digest,
        name(&run.state)
    );
    println!("  {}", run.reason);
    for check in &run.checks {
        println!(
            "  {} [{}] {}: {}",
            check.check_id,
            name(&check.dimension),
            name(&check.status),
            check.detail
        );
    }
    for skipped in &run.skipped {
        println!("  skipped: {skipped}");
    }
    for limitation in &run.limitations {
        println!("  limitation: {limitation}");
    }
}

/** Plan, run, inspect, or list regression runs
 * Input
    - command: &str - plan, run, inspect, or history
    - args: &Args - proposal or run id
 * Output
    - Result<(), String>
*/
fn regression(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(1, &format!("crane evolution regression {command} ID"))?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let id = args.positional[0].clone();
    let json = args.flag("json");
    match command {
        "plan" => {
            let plan = local.evolution().regression_plan(&actor, &id)?;
            if json {
                return print_json(&plan);
            }
            println!(
                "{} for {}: {} scenario(s) on candidate v{} {} against v{} {}",
                plan.plan_id,
                plan.proposal_id,
                plan.lab_plan.scenarios.len(),
                plan.candidate_version,
                plan.candidate_digest,
                plan.parent_version,
                plan.parent_digest
            );
            for scenario in &plan.lab_plan.scenarios {
                println!("  {}: {}", scenario.scenario_id, scenario.description);
            }
            for dropped in &plan.dropped {
                println!("  dropped: {dropped}");
            }
            for unsupported in &plan.unsupported {
                println!("  unsupported: {unsupported}");
            }
            Ok(())
        }
        "run" => {
            let (run, lab_reports): (RegressionRun, Vec<LabReport>) =
                local.evolution().regression_run(&actor, &id)?;
            let lab = LabStore::open(&local.context.runtime);
            for report in &lab_reports {
                lab.append(report)?;
            }
            if json {
                print_json(&run)?;
            } else {
                print_run(&run);
            }
            if run.state == RunState::Passed {
                Ok(())
            } else {
                Err(format!("regression {}: {}", name(&run.state), run.reason))
            }
        }
        "inspect" => {
            let run = local.store.run(&id)?;
            if json {
                return print_json(&run);
            }
            print_run(&run);
            Ok(())
        }
        _ => {
            let proposal = local.store.proposal(&id)?;
            let runs = proposal
                .runs
                .iter()
                .filter_map(|run| local.store.state.runs.get(run))
                .map(|run| serde_json::json!({"run_id": run.run_id, "state": run.state, "previous_run": run.previous_run, "inputs_changed": run.inputs_changed, "candidate_digest": run.candidate_digest, "created_at": run.created_at}))
                .collect::<Vec<_>>();
            if json {
                return print_json(&runs);
            }
            if runs.is_empty() {
                println!("No regression run.");
            }
            for run in runs {
                println!(
                    "{} {}",
                    run["run_id"].as_str().unwrap_or_default(),
                    run["state"].as_str().unwrap_or_default()
                );
            }
            Ok(())
        }
    }
}

/** Prepare a review packet
 * Input
    - args: &Args - proposal id
 * Output
    - Result<(), String>
*/
fn prepare(args: &Args) -> Result<(), String> {
    args.expect_positional(1, "crane evolution prepare-review PROPOSAL_ID")?;
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let proposal = local
        .evolution()
        .prepare_review(&actor, &args.positional[0])?;
    if args.flag("json") {
        return print_json(&proposal);
    }
    print_proposal(&proposal);
    Ok(())
}
