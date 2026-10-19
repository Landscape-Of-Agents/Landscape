// The deterministic evolution demonstration ('crane evolution demo'). Offline, isolated, and
// synthetic: every world is built in memory from the earlier components' fixtures, invocation
// records are produced by Component 6's real engine against fixture adapters (no external system,
// no credentials), and one record that stands for a declared-but-unverified outcome is a labelled
// synthetic fixture. Nothing touches a real registry or ledger. Each case prints what was expected
// and what happened.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::models::{
    Change, ChangeKind, OutcomeClass, ProposalState, RunState, SignalKind, SignalState, Thresholds,
};
use super::reports::{parse, EvolutionStore};
use super::workflow::{Evolution, SourceInfo};
use crate::capability::demo as capability_demo;
use crate::capability::lifecycle::{LifecycleState, PublicationRequest};
use crate::capability::model::ReviewLevel;
use crate::capability::refs::Sources;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capture::{AccessPolicy, Clock};
use crate::invocation::demo::{diagnostics_request, diagnostics_world, status_fixture, World};
use crate::invocation::engine::InvocationRecord;

/** The capability every case evolves */
const CAPABILITY: &str = "deployment-failure-diagnostics";

/** Fixed start of the evolution clock */
const EPOCH: u64 = 1_760_001_000_000;

/** A seeded world: invocation history and the evolution store
 * Fields
    - world: World - the invocation demonstration world (registry, capture, fixtures, ledger)
    - reader: AccessPolicy - the engineer's Work Capture access
    - records: Vec<InvocationRecord> - ledger records plus the synthetic one
    - events: BTreeMap<String, usize> - lifecycle events per invocation
    - info: SourceInfo - source labels
    - store: EvolutionStore - evolution store
    - clock: Clock - clock
*/
pub(crate) struct Bench {
    pub(crate) world: World,
    pub(crate) reader: AccessPolicy,
    pub(crate) records: Vec<InvocationRecord>,
    pub(crate) events: BTreeMap<String, usize>,
    pub(crate) info: SourceInfo,
    pub(crate) store: EvolutionStore,
    pub(crate) clock: Clock,
}

impl Bench {
    /** The evolution operations over this bench
     * Input
        - None (uses self)
     * Output
        - Evolution
    */
    pub(crate) fn evolution(&mut self) -> Evolution<'_> {
        Evolution {
            registry: &mut self.world.registry,
            sources: Sources {
                capture: &self.world.capture.store,
                current: &self.world.current,
                reader: &self.reader,
            },
            invocations: &self.records,
            events: &self.events,
            info: &self.info,
            compilation_reports: &self.world.compilation_reports,
            store: &mut self.store,
            clock: &self.clock,
        }
    }

    /** Re-read the ledger after new invocations
     * Input
        - None (uses self)
     * Output
        - None
    */
    fn reload(&mut self) {
        let synthetic = self
            .records
            .iter()
            .filter(|record| self.info.synthetic.contains(&record.invocation_id))
            .cloned()
            .collect::<Vec<_>>();
        self.records = self.world.ledger.state.records.values().cloned().collect();
        self.records.extend(synthetic);
        self.events.clear();
        for line in &self.world.events.events {
            if let Some((id, _)) = line.split_once(':') {
                *self.events.entry(id.to_string()).or_insert(0) += 1;
            }
        }
    }
}

/** Invoke the diagnostics capability once
 * Input
    - world: &mut World - world
    - reader: &AccessPolicy - engineer access
    - key: &str - idempotency key
    - task: &str - task id
    - deployment: &str - deployment id
    - repository: &str - repository
 * Output
    - CapabilityResult<InvocationRecord>
*/
fn invoke(
    world: &mut World,
    reader: &AccessPolicy,
    key: &str,
    task: &str,
    deployment: &str,
    repository: &str,
) -> CapabilityResult<InvocationRecord> {
    let mut request = diagnostics_request(world, key);
    request.task_id = Some(task.into());
    request.repository = Some(repository.into());
    request
        .inputs
        .insert("deployment_id".into(), deployment.into());
    let actor = world.cast.engineer.clone();
    world.engine(reader).submit(&actor, request)
}

/** Build a world with the synthetic invocation history:
 *   acme/payments    three failed log verifications, two resubmissions of the first
 *   acme/ledger      one failed log verification
 *   acme/refunds-api three failures, one verified success, one contradicted outcome
 *   acme/billing     one declared success without verification (synthetic record)
 * Input
    - None
 * Output
    - CapabilityResult<Bench>
*/
pub(crate) fn seeded() -> CapabilityResult<Bench> {
    let mut world = diagnostics_world()?;
    let reader = capability_demo::reader(&world.cast.engineer)?;
    world.set_status(status_fixture("failed", false));
    for index in 1..=3 {
        invoke(
            &mut world,
            &reader,
            &format!("pay-{index}"),
            &format!("PAY-20{index}"),
            &format!("dep-800{index}"),
            "acme/payments",
        )?;
    }
    for key in ["pay-1-retry-a", "pay-1-retry-b"] {
        invoke(
            &mut world,
            &reader,
            key,
            "PAY-201",
            "dep-8001",
            "acme/payments",
        )?;
    }
    invoke(
        &mut world,
        &reader,
        "ledger-1",
        "LED-1",
        "dep-8100",
        "acme/ledger",
    )?;
    for index in 1..=3 {
        invoke(
            &mut world,
            &reader,
            &format!("ra-{index}"),
            &format!("RA-{index}"),
            &format!("dep-830{index}"),
            "acme/refunds-api",
        )?;
    }
    world.set_status(status_fixture("failed", true));
    let success = invoke(
        &mut world,
        &reader,
        "ra-4",
        "RA-4",
        "dep-8304",
        "acme/refunds-api",
    )?;
    world.set_status(status_fixture("succeeded", true));
    invoke(
        &mut world,
        &reader,
        "ra-5",
        "RA-5",
        "dep-8305",
        "acme/refunds-api",
    )?;
    world.set_status(status_fixture("failed", true));
    let mut declared = success;
    declared.invocation_id = "inv_synthetic_declared_1".into();
    declared.idempotency_key = "synthetic-declared-1".into();
    declared.request.task_id = Some("BIL-1".into());
    declared.request.repository = Some("acme/billing".into());
    declared
        .request
        .inputs
        .insert("deployment_id".into(), "dep-8400".into());
    declared.verifications.clear();
    declared.seal();
    let mut bench = Bench {
        world,
        reader,
        records: vec![declared],
        events: BTreeMap::new(),
        info: SourceInfo {
            ledger_available: true,
            events_available: true,
            synthetic: BTreeSet::from(["inv_synthetic_declared_1".to_string()]),
            label: "SYNTHETIC: fixture adapters through Component 6's engine, plus one labelled synthetic record".into(),
        },
        store: EvolutionStore::in_memory(),
        clock: Clock::fixed(EPOCH),
    };
    bench.reload();
    Ok(bench)
}

/** One demonstrated case
 * Fields
    - case: u32 - case number
    - name: String - case
    - expected: String - expected result
    - actual: String - actual result
    - matched: bool - whether they agree
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoCase {
    pub(crate) case: u32,
    pub(crate) name: String,
    pub(crate) expected: String,
    pub(crate) actual: String,
    pub(crate) matched: bool,
}

/** What the demonstration produced
 * Fields
    - notice: String - synthetic-data notice
    - cases: Vec<DemoCase> - cases
    - all_matched: bool - every case matched its expectation
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct DemoReport {
    pub(crate) notice: String,
    pub(crate) cases: Vec<DemoCase>,
    pub(crate) all_matched: bool,
}

/** Record a case
 * Input
    - cases: &mut Vec<DemoCase> - cases
    - name: &str - case
    - expected: &str - expected result
    - actual: String - actual result
    - matched: bool - agreement
 * Output
    - None
*/
fn record(cases: &mut Vec<DemoCase>, name: &str, expected: &str, actual: String, matched: bool) {
    cases.push(DemoCase {
        case: cases.len() as u32 + 1,
        name: name.into(),
        expected: expected.into(),
        actual,
        matched,
    });
}

/** The signal of a kind in a repository
 * Input
    - signals: &[super::models::Signal] - signals
    - kind: SignalKind - kind
    - repository: &str - repository in the context key
 * Output
    - CapabilityResult<super::models::Signal>
*/
fn signal_of(
    signals: &[super::models::Signal],
    kind: SignalKind,
    repository: &str,
) -> CapabilityResult<super::models::Signal> {
    signals
        .iter()
        .find(|signal| {
            signal.kind == kind
                && signal
                    .context_key
                    .contains(&format!("repository={repository};"))
        })
        .cloned()
        .ok_or_else(|| {
            CapabilityError::new(
                ErrorCode::NotFound,
                format!("no {kind:?} signal for {repository}"),
            )
        })
}

/** Run the demonstration
 * Input
    - None
 * Output
    - CapabilityResult<DemoReport>
*/
pub(crate) fn run() -> CapabilityResult<DemoReport> {
    let mut cases = Vec::new();
    let mut bench = seeded()?;
    let engineer = bench.world.cast.engineer.clone();
    let analysis =
        bench
            .evolution()
            .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())?;
    let repeated = signal_of(
        &analysis.signals,
        SignalKind::RepeatedFailure,
        "acme/payments",
    )?;
    record(
        &mut cases,
        "Repeated verified failure",
        "a qualified repeated_failure signal (3 independent failures, rules met)",
        format!(
            "{} with {} independent failures: {:?}",
            repeated.signal_id, repeated.independent_count, repeated.state
        ),
        repeated.state == SignalState::Investigating && repeated.independent_count == 3,
    );
    let single = signal_of(
        &analysis.signals,
        SignalKind::DiagnosticFailure,
        "acme/ledger",
    )?;
    record(
        &mut cases,
        "Single failure",
        "a diagnostic signal with insufficient evidence, never a recurring pattern",
        format!("{:?} signal: {:?}", single.kind, single.state),
        single.state == SignalState::InsufficientEvidence,
    );
    let declared = analysis
        .report
        .observations
        .iter()
        .find(|observation| observation.invocation_id == "inv_synthetic_declared_1")
        .map(|observation| observation.class);
    record(
        &mut cases,
        "Declared success without verification",
        "classified declared_success_unverified, not a verified success",
        format!("{declared:?}"),
        declared == Some(OutcomeClass::DeclaredSuccessUnverified),
    );
    let payments = analysis
        .report
        .observations
        .iter()
        .filter(|observation| observation.context.repository.as_deref() == Some("acme/payments"))
        .count();
    record(
        &mut cases,
        "Duplicate retries",
        "5 payments records and many lifecycle events, but 3 independent failures",
        format!(
            "{payments} records, {} collapsed, {} events correlated, {} independent failures",
            analysis.report.coverage.duplicates_collapsed,
            analysis.report.coverage.events_correlated,
            repeated.independent_count
        ),
        payments == 5
            && analysis.report.coverage.duplicates_collapsed == 2
            && repeated.independent_count == 3,
    );
    let contradicted = signal_of(
        &analysis.signals,
        SignalKind::RepeatedFailure,
        "acme/refunds-api",
    )?;
    record(
        &mut cases,
        "Contradictory evidence",
        "both sides kept; the failure pattern is not qualified",
        format!(
            "{} supporting, {} contradicting kept: {:?}",
            contradicted.supporting.len(),
            contradicted.contradicting.len(),
            contradicted.state
        ),
        contradicted.state == SignalState::InsufficientEvidence
            && contradicted.contradicting.len() == 2,
    );
    let mut drift_bench = seeded()?;
    drift_bench
        .world
        .current
        .zones
        .insert("refunds".into(), "v4".into());
    let drift_analysis =
        drift_bench
            .evolution()
            .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())?;
    let drift = drift_analysis
        .signals
        .iter()
        .find(|signal| signal.kind == SignalKind::DependencyDrift)
        .cloned()
        .ok_or_else(|| CapabilityError::new(ErrorCode::NotFound, "no drift signal"))?;
    record(
        &mut cases,
        "Dependency drift",
        "zone refunds pinned v3, observed v4: a qualified dependency_drift signal",
        format!("{}: {:?}", drift.context_key, drift.state),
        drift.state == SignalState::Investigating
            && drift
                .drift
                .first()
                .and_then(|finding| finding.observed.as_deref())
                == Some("v4"),
    );
    let mut dark = seeded()?;
    dark.world.current.landscape_known = false;
    let unknown = dark.evolution().drift(&engineer, CAPABILITY, Some(1))?;
    let unknown_analysis =
        dark.evolution()
            .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())?;
    let unavailable = unknown_analysis.signals.iter().any(|signal| {
        signal.kind == SignalKind::ContextUnavailable
            && signal.state == SignalState::InsufficientEvidence
    });
    record(
        &mut cases,
        "Unavailable context",
        "zone and flow drift unknown, not \"no drift\"",
        unknown.summary.clone(),
        unavailable && unknown.summary.contains("unknown"),
    );
    let blocked = bench.evolution().propose(
        &engineer,
        &repeated.signal_id,
        vec![
            Change {
                change: ChangeKind::LowerReview {
                    to: ReviewLevel::None,
                },
                rationale: "the reviews slow triage".into(),
                evidence: repeated.supporting.clone(),
                origin: "suggestion".into(),
            },
            Change {
                change: ChangeKind::ChangePolicy {
                    target_id: "refund_safety".into(),
                    detail: "relax log access".into(),
                },
                rationale: "logs are often unavailable".into(),
                evidence: repeated.supporting.clone(),
                origin: "suggestion".into(),
            },
            Change {
                change: ChangeKind::AddStep {
                    description: "Retry the log query with an admin token".into(),
                },
                rationale: "might fetch the logs".into(),
                evidence: repeated.supporting.clone(),
                origin: "suggestion".into(),
            },
            Change {
                change: ChangeKind::AddEnvironment {
                    environment: "staging".into(),
                },
                rationale: "probably works there too".into(),
                evidence: repeated.supporting.clone(),
                origin: "suggestion".into(),
            },
        ],
    )?;
    let versions = bench.world.registry.state.versions[CAPABILITY].len();
    record(
        &mut cases,
        "Unsupported revision",
        "review downgrade, policy edit, speculative step, and broadened scope refused; blocked without a candidate",
        format!("{:?}; {} refused; registry versions {versions}", blocked.state, blocked.unsupported.len()),
        blocked.state == ProposalState::Blocked && blocked.unsupported.len() == 4 && blocked.candidate.is_none() && versions == 1,
    );
    let parent_before = bench.world.registry.state.record(CAPABILITY, 1)?.clone();
    let proposal = bench
        .evolution()
        .propose(&engineer, &repeated.signal_id, Vec::new())?;
    let parent_after = bench.world.registry.state.record(CAPABILITY, 1)?.clone();
    let candidate_version = proposal.candidate.as_ref().map(|link| link.version);
    record(
        &mut cases,
        "Safe revision proposal",
        "candidate v2 (state candidate) linked to v1; v1 unchanged and still published",
        format!(
            "{:?}, candidate {candidate_version:?}; parent digest unchanged: {}; parent {:?}",
            proposal.state,
            parent_before == parent_after,
            parent_after.state
        ),
        proposal.state == ProposalState::CandidateCreated
            && candidate_version == Some(2)
            && parent_before == parent_after
            && parent_after.state == LifecycleState::Published,
    );
    let again = bench
        .evolution()
        .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())?;
    let repeat = bench
        .evolution()
        .propose(&engineer, &repeated.signal_id, Vec::new())?;
    let signal_count = bench.store.state.signals.len();
    record(
        &mut cases,
        "Idempotent analysis",
        "the same report and signals; the same proposal; no new candidate",
        format!(
            "report reused: {}; {signal_count} signals stored; same proposal: {}; registry versions {}",
            again.reused,
            repeat.proposal_id == proposal.proposal_id,
            bench.world.registry.state.versions[CAPABILITY].len()
        ),
        again.reused
            && again.report.report_id == analysis.report.report_id
            && signal_count == analysis.signals.len()
            && repeat.proposal_id == proposal.proposal_id
            && bench.world.registry.state.versions[CAPABILITY].len() == 2,
    );
    let mut narrow = seeded()?;
    let narrow_analysis =
        narrow
            .evolution()
            .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())?;
    let narrow_signal = signal_of(
        &narrow_analysis.signals,
        SignalKind::RepeatedFailure,
        "acme/payments",
    )?;
    narrow.evolution().propose(
        &engineer,
        &narrow_signal.signal_id,
        vec![Change {
            change: ChangeKind::AddExclusion {
                exclusion_kind: "environment".into(),
                value: "production".into(),
                reason: "log verification fails there".into(),
            },
            rationale: "failures were observed in production".into(),
            evidence: narrow_signal.supporting.clone(),
            origin: "suggestion".into(),
        }],
    )?;
    let narrow_id = narrow
        .store
        .state
        .proposals
        .keys()
        .next()
        .cloned()
        .unwrap_or_default();
    let (failed, _) = narrow.evolution().regression_run(&engineer, &narrow_id)?;
    let counterexample = failed
        .checks
        .iter()
        .find(|check| check.check_id.contains("verified-context") && check.counterexample.is_some())
        .map(|check| check.check_id.clone());
    record(
        &mut cases,
        "Regression failure",
        "excluding production contradicts a verified production success: failed with a counterexample",
        format!("{:?}; counterexample from {counterexample:?}", failed.state),
        failed.state == RunState::Failed && counterexample.is_some(),
    );
    let mut reword = seeded()?;
    let reword_analysis =
        reword
            .evolution()
            .analyze(&engineer, CAPABILITY, Some(1), Thresholds::default())?;
    let reword_signal = signal_of(
        &reword_analysis.signals,
        SignalKind::RepeatedFailure,
        "acme/payments",
    )?;
    let reworded = reword.evolution().propose(
        &engineer,
        &reword_signal.signal_id,
        vec![Change {
            change: ChangeKind::ClarifyStep {
                step_id: "fetch-logs".into(),
                description: "Fetch error logs emitted during the deployment window; when the log source returns none, record that no logs were available".into(),
            },
            rationale: "the log source returned no logs in the failed invocations".into(),
            evidence: reword_signal.supporting.clone(),
            origin: "suggestion".into(),
        }],
    )?;
    let (inconclusive, _) = reword
        .evolution()
        .regression_run(&engineer, &reworded.proposal_id)?;
    record(
        &mut cases,
        "Regression inconclusive",
        "a reworded step cannot be executed by the Lab: inconclusive, never passed",
        format!("{:?}: {}", inconclusive.state, inconclusive.reason),
        inconclusive.state == RunState::Inconclusive
            && inconclusive.reason.contains("behavioral:procedure"),
    );
    let repin = drift_bench
        .evolution()
        .propose(&engineer, &drift.signal_id, Vec::new())?;
    drift_bench
        .world
        .current
        .zones
        .insert("refunds".into(), "v5".into());
    let (stale, _) = drift_bench
        .evolution()
        .regression_run(&engineer, &repin.proposal_id)?;
    let stored_repin = drift_bench.store.proposal(&repin.proposal_id)?;
    record(
        &mut cases,
        "Stale candidate",
        "the candidate pins refunds v4, now v5: blocked before any check ran",
        format!(
            "run {:?}, proposal {:?}: {}",
            stale.state, stored_repin.state, stale.reason
        ),
        repin.state == ProposalState::CandidateCreated
            && stale.state == RunState::Blocked
            && stored_repin.state == ProposalState::Blocked
            && stale.checks.is_empty(),
    );
    let text = serde_json::to_string(&bench.store.state)
        .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error.to_string()))?;
    let tampered = text.replacen("\"independent_count\":3", "\"independent_count\":30", 1);
    let corrupt = parse(&tampered).err();
    record(
        &mut cases,
        "Corrupt report",
        "integrity_mismatch; nothing repaired",
        corrupt
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| "accepted".into()),
        tampered != text && corrupt.is_some_and(|error| error.code == ErrorCode::IntegrityMismatch),
    );
    let (passed, _) = bench
        .evolution()
        .regression_run(&engineer, &proposal.proposal_id)?;
    let packet = bench
        .evolution()
        .prepare_review(&engineer, &proposal.proposal_id);
    let agent = bench.world.cast.agent.clone();
    let publish = bench.world.registry.publish(
        &agent,
        &PublicationRequest {
            capability_id: CAPABILITY.into(),
            version: 2,
            reason: "regression passed".into(),
            acknowledged_warnings: Vec::new(),
        },
        EPOCH,
    );
    let state = bench.world.registry.state.record(CAPABILITY, 2)?.state;
    record(
        &mut cases,
        "Publication boundary",
        "regression passed; review packet prepared; candidate v2 still a candidate; an agent cannot publish",
        format!(
            "run {:?}; packet {}; candidate {:?}; agent publish refused: {}",
            passed.state,
            packet.as_ref().map(|_| "prepared").unwrap_or("refused"),
            state,
            publish.is_err()
        ),
        passed.state == RunState::Passed
            && packet.is_ok()
            && state == LifecycleState::Candidate
            && publish.is_err(),
    );
    let all_matched = cases.iter().all(|case| case.matched);
    Ok(DemoReport {
        notice: "SYNTHETIC: every invocation, evidence record, and version comes from deterministic fixtures; nothing here is live production evidence".into(),
        cases,
        all_matched,
    })
}
