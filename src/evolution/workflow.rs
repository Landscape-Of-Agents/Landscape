// The evolution operations. Authorization comes first, from the registry's grants (held by
// Crane, never from caller metadata): any registry role may analyze, propose, and run
// regressions (agents included, as contributors); dismissing a signal needs the Validator role
// (people only); abandoning a proposal needs its proposer or a validator. An actor sees only the
// invocations it may see in Component 6 (its own; all for validators), and evidence is read
// through its own Work Capture access, so restricted records are counted, never retrieved.
//
//   analyze          observations -> drift -> signals -> qualification -> hypothesis; it stores an
//                    analysis report and its signals and never creates a candidate. Identical
//                    inputs return the stored report and signals; a changed evidence set gives new
//                    signals and supersedes the open ones of the same kind and context.
//   propose          a minimal change set from a qualified signal (plus optional typed
//                    suggestions), gated; the exact candidate definition is persisted in the
//                    draft before Component 2's propose_version stores it, so a crash in between
//                    is recovered by finding that exact digest instead of creating a duplicate.
//   regression_plan / regression_run    see regression.rs; every run is a new immutable record.
//   prepare_review   a review packet for a passed run that still matches the current context;
//                    nothing is delivered, the candidate keeps its registry state.
//   abandon, dismiss close a proposal or a signal with a recorded reason.
//
// Nothing here publishes, validates, approves, deprecates, revokes, or rejects a capability.

use std::collections::{BTreeMap, BTreeSet};

use super::drift;
use super::models::{
    advance, digest_of, AnalysisReport, CandidateLink, Change, ChangeKind, Coverage, DriftReport,
    EvidenceAccess, Lifecycle, Observation, Proposal, ProposalState, RegressionPlan, RegressionRun,
    ReviewPacket, RunState, Sealed, Signal, SignalKind, SignalState, SourceCoverage, Thresholds,
    Transition,
};
use super::outcomes::{deduplicate, normalize, pinned_governance};
use super::patterns;
use super::proposals::{self, GateContext};
use super::regression;
use super::reports::{put_forward, put_once, EvolutionStore};
use super::{DETECTOR_VERSION, EVOLUTION_SCHEMA_VERSION};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::refs::Sources;
use crate::capability::registry::{CapabilityRegistry, VersionRecord};
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode, Role};
use crate::capability_lab::report::LabReport;
use crate::capture::{stable_id, Actor, Clock};
use crate::compiler::assemble::CompilationReport;
use crate::invocation::engine::{InvocationRecord, VerificationStatus};

/** Where the observations come from
 * Fields
    - ledger_available: bool - the invocation ledger could be read
    - events_available: bool - DataSea's event log could be read
    - synthetic: BTreeSet<String> - invocation ids that come from labelled fixtures
    - label: String - how the records were produced
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct SourceInfo {
    pub(crate) ledger_available: bool,
    pub(crate) events_available: bool,
    pub(crate) synthetic: BTreeSet<String>,
    pub(crate) label: String,
}

/** Everything the operations need
 * Fields
    - registry: &'a mut CapabilityRegistry - Component 2 registry
    - sources: Sources<'a> - Work Capture records, current versions, the actor's access
    - invocations: &'a [InvocationRecord] - Component 6 records (read only)
    - events: &'a BTreeMap<String, usize> - DataSea lifecycle events per invocation
    - info: &'a SourceInfo - source availability and labels
    - compilation_reports: &'a [CompilationReport] - verified compilation reports
    - store: &'a mut EvolutionStore - evolution store
    - clock: &'a Clock - clock
*/
pub(crate) struct Evolution<'a> {
    pub(crate) registry: &'a mut CapabilityRegistry,
    pub(crate) sources: Sources<'a>,
    pub(crate) invocations: &'a [InvocationRecord],
    pub(crate) events: &'a BTreeMap<String, usize>,
    pub(crate) info: &'a SourceInfo,
    pub(crate) compilation_reports: &'a [CompilationReport],
    pub(crate) store: &'a mut EvolutionStore,
    pub(crate) clock: &'a Clock,
}

/** The result of an analysis
 * Fields
    - report: AnalysisReport - the report
    - signals: Vec<Signal> - its signals
    - reused: bool - an identical analysis was already stored (nothing new written)
*/
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct Analysis {
    pub(crate) report: AnalysisReport,
    pub(crate) signals: Vec<Signal>,
    pub(crate) reused: bool,
}

/** The snake_case name of a serializable unit value
 * Input
    - value: &impl serde::Serialize - value
 * Output
    - String
*/
pub(crate) fn name(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

impl Evolution<'_> {
    /** The version analyzed when none is given: the latest published, else the latest
     * Input
        - capability_id: &str - capability
        - version: Option<u32> - requested version
     * Output
        - CapabilityResult<VersionRecord>
    */
    fn target(&self, capability_id: &str, version: Option<u32>) -> CapabilityResult<VersionRecord> {
        let versions = self
            .registry
            .state
            .versions
            .get(capability_id)
            .ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::NotFound,
                    format!("capability {capability_id} does not exist"),
                )
            })?;
        let chosen = match version {
            Some(version) => versions.get(&version),
            None => versions
                .values()
                .rev()
                .find(|record| record.state == LifecycleState::Published)
                .or_else(|| versions.values().next_back()),
        };
        chosen.cloned().ok_or_else(|| {
            CapabilityError::new(
                ErrorCode::NotFound,
                format!(
                    "{capability_id} v{} does not exist",
                    version.unwrap_or_default()
                ),
            )
        })
    }

    /** Read and normalize the invocations of an exact version the actor may see
     * Input
        - actor: &Actor - actor (already authorized)
        - record: &VersionRecord - analyzed version
     * Output
        - CapabilityResult<(Vec<Observation>, Coverage)>
        - Error (IntegrityMismatch) when a record does not match its digest
    */
    fn observe(
        &self,
        actor: &Actor,
        record: &VersionRecord,
    ) -> CapabilityResult<(Vec<Observation>, Coverage)> {
        let id = &record.definition.capability_id;
        let version = record.definition.version;
        let validator = self.registry.grants.require(actor, Role::Validator).is_ok();
        let mut coverage = Coverage::default();
        let mut observations = Vec::new();
        for invocation in self
            .invocations
            .iter()
            .filter(|invocation| &invocation.capability_id == id)
        {
            coverage.records_for_capability += 1;
            if invocation.version != version {
                coverage.other_versions += 1;
                continue;
            }
            if !validator && invocation.requester.id != actor.id {
                coverage.records_not_visible += 1;
                continue;
            }
            if !invocation.verify_digest() {
                return Err(CapabilityError::new(
                    ErrorCode::IntegrityMismatch,
                    format!(
                        "invocation {} does not match its digest; nothing is analyzed",
                        invocation.invocation_id
                    ),
                ));
            }
            coverage.records_visible += 1;
            if invocation.digest != record.definition_digest {
                coverage.other_digest += 1;
                continue;
            }
            coverage.retry_attempts_excluded += invocation
                .operations
                .iter()
                .map(|operation| operation.attempts.saturating_sub(1))
                .sum::<u32>();
            let events = self
                .events
                .get(&invocation.invocation_id)
                .copied()
                .unwrap_or(0);
            coverage.events_correlated += events;
            observations.push(normalize(
                invocation,
                &record.definition,
                &self.sources,
                events,
                self.info.synthetic.contains(&invocation.invocation_id),
            ));
        }
        coverage.duplicates_collapsed = deduplicate(&mut observations);
        coverage.independent = observations
            .iter()
            .filter(|observation| observation.duplicate_of.is_none() && observation.class.trial())
            .count();
        for reference in observations
            .iter()
            .flat_map(|observation| &observation.evidence)
        {
            match reference.access {
                EvidenceAccess::Restricted => coverage.restricted_evidence += 1,
                EvidenceAccess::Missing => coverage.missing_evidence += 1,
                _ => {}
            }
        }
        let any_synthetic = observations.iter().any(|observation| observation.synthetic);
        coverage.sources = vec![
            SourceCoverage {
                source: "invocation ledger (Component 6)".into(),
                available: self.info.ledger_available,
                synthetic: any_synthetic,
                detail: self.info.label.clone(),
            },
            SourceCoverage {
                source: "DataSea lifecycle events".into(),
                available: self.info.events_available,
                synthetic: false,
                detail: "correlated per invocation; never counted as trials".into(),
            },
            SourceCoverage {
                source: "Work Capture evidence".into(),
                available: true,
                synthetic: false,
                detail: "read through the actor's own access policy".into(),
            },
            SourceCoverage {
                source: "Landscape (zones, flows)".into(),
                available: self.sources.current.landscape_known,
                synthetic: false,
                detail: if self.sources.current.landscape_known {
                    "read".into()
                } else {
                    "unavailable: zone and flow drift is unknown".into()
                },
            },
            SourceCoverage {
                source: "Crane policies".into(),
                available: self.sources.current.crane_known,
                synthetic: false,
                detail: if self.sources.current.crane_known {
                    "read".into()
                } else {
                    "unavailable: policy drift is unknown".into()
                },
            },
        ];
        Ok((observations, coverage))
    }

    /** Report drift of a version (read only; nothing is stored)
     * Input
        - actor: &Actor - actor (needs a registry role)
        - capability_id: &str - capability
        - version: Option<u32> - version, None for the latest published
     * Output
        - CapabilityResult<DriftReport>
    */
    pub(crate) fn drift(
        &self,
        actor: &Actor,
        capability_id: &str,
        version: Option<u32>,
    ) -> CapabilityResult<DriftReport> {
        self.registry.grants.require(actor, Role::Contributor)?;
        let record = self.target(capability_id, version)?;
        let (observations, _) = self.observe(actor, &record)?;
        drift::detect(
            &self.registry.state,
            capability_id,
            record.definition.version,
            &self.sources,
            &observations,
        )
    }

    /** Analyze a version's outcomes and context, storing the report and its signals
     * Input
        - actor: &Actor - actor (needs a registry role; checked before anything is read)
        - capability_id: &str - capability
        - version: Option<u32> - version, None for the latest published
        - thresholds: Thresholds - qualification thresholds
     * Output
        - CapabilityResult<Analysis>
    */
    pub(crate) fn analyze(
        &mut self,
        actor: &Actor,
        capability_id: &str,
        version: Option<u32>,
        thresholds: Thresholds,
    ) -> CapabilityResult<Analysis> {
        self.registry.grants.require(actor, Role::Contributor)?;
        let record = self.target(capability_id, version)?;
        let definition = &record.definition;
        let version = definition.version;
        let digest = record.definition_digest.clone();
        let (observations, coverage) = self.observe(actor, &record)?;
        let drift = drift::detect(
            &self.registry.state,
            capability_id,
            version,
            &self.sources,
            &observations,
        )?;
        let thresholds_digest = digest_of("evolution/thresholds/v1", &thresholds);
        let observation_set = observations
            .iter()
            .map(|observation| {
                (
                    &observation.invocation_id,
                    &observation.record_digest,
                    observation.events,
                )
            })
            .collect::<Vec<_>>();
        let report_id = stable_id(
            "evr",
            &[
                capability_id,
                &version.to_string(),
                &digest,
                DETECTOR_VERSION,
                &thresholds_digest,
                &digest_of("evolution/observation-set/v1", &observation_set),
                &digest_of("evolution/governance/v1", self.sources.current),
                &name(&record.state),
            ],
        );
        if let Some(report) = self.store.state.analyses.get(&report_id) {
            let signals = report
                .signals
                .iter()
                .filter_map(|(id, _, _)| self.store.state.signals.get(id).cloned())
                .collect();
            return Ok(Analysis {
                report: report.clone(),
                signals,
                reused: true,
            });
        }
        let now = self.clock.now();
        let pins = pinned_governance(definition);
        let mut signals = Vec::new();
        for detected in patterns::detect(&observations, &digest, &drift) {
            let records = observations
                .iter()
                .filter(|observation| {
                    detected.supporting.contains(&observation.invocation_id)
                        || detected.contradicting.contains(&observation.invocation_id)
                })
                .map(|observation| (&observation.invocation_id, &observation.record_digest))
                .collect::<Vec<_>>();
            let evidence_set_digest = digest_of(
                "evolution/evidence-set/v1",
                &(&records, &detected.collapsed, &detected.drift),
            );
            let signal_id = stable_id(
                "sig",
                &[
                    capability_id,
                    &version.to_string(),
                    &digest,
                    &name(&detected.kind),
                    &detected.context_key,
                    DETECTOR_VERSION,
                    &evidence_set_digest,
                    &thresholds_digest,
                ],
            );
            let qualification =
                patterns::qualify(&detected, definition.applicability.risk, &pins, thresholds);
            let missing = detected
                .evidence
                .iter()
                .filter(|reference| {
                    matches!(
                        reference.access,
                        EvidenceAccess::Missing | EvidenceAccess::Restricted
                    )
                })
                .map(|reference| format!("{} ({:?})", reference.ref_id, reference.access))
                .collect();
            let mut signal = Signal {
                signal_id,
                schema_version: EVOLUTION_SCHEMA_VERSION,
                detector_version: DETECTOR_VERSION.into(),
                capability_id: capability_id.into(),
                version,
                capability_digest: digest.clone(),
                kind: detected.kind,
                context_key: detected.context_key.clone(),
                state: SignalState::SignalDetected,
                transitions: vec![Transition {
                    sequence: 1,
                    from: None,
                    to: SignalState::SignalDetected,
                    actor: actor.clone(),
                    reason: format!("detected by detector v{DETECTOR_VERSION}"),
                    at: now,
                }],
                supporting: detected.supporting.clone(),
                contradicting: detected.contradicting.clone(),
                collapsed: detected.collapsed.clone(),
                independent_count: detected.supporting.len(),
                affected_claims: detected.affected_claims.clone(),
                affected_steps: detected.affected_steps.clone(),
                evidence: detected.evidence.clone(),
                missing_evidence: missing,
                drift: detected.drift.clone(),
                qualification: None,
                hypothesis: None,
                evidence_set_digest,
                analysis_id: report_id.clone(),
                superseded_by: None,
                limitations: Vec::new(),
                created_at: now,
                updated_at: now,
                digest: String::new(),
            };
            if detected.supporting.len() == 1 && detected.kind == SignalKind::DiagnosticFailure {
                signal.limitations.push("a single invocation is diagnostic only; it is not presented as a recurring pattern".into());
            }
            advance(&mut signal.state, &mut signal.transitions, SignalState::Qualifying, actor, "checking independence, comparability, completeness, provenance, and contradictions", now)?;
            let qualified = qualification.qualified;
            let explanation = qualification.explanation.clone();
            signal.qualification = Some(qualification);
            if !qualified {
                advance(
                    &mut signal.state,
                    &mut signal.transitions,
                    SignalState::InsufficientEvidence,
                    actor,
                    &explanation,
                    now,
                )?;
            } else {
                advance(
                    &mut signal.state,
                    &mut signal.transitions,
                    SignalState::Qualified,
                    actor,
                    &explanation,
                    now,
                )?;
                let (hypothesis, proposable) =
                    patterns::hypothesis(&detected, (capability_id, version));
                let recommendation = hypothesis.recommendation.clone();
                signal.hypothesis = Some(hypothesis);
                advance(
                    &mut signal.state,
                    &mut signal.transitions,
                    SignalState::Investigating,
                    actor,
                    "context and counterevidence assembled; hypothesis recorded",
                    now,
                )?;
                if !proposable {
                    advance(
                        &mut signal.state,
                        &mut signal.transitions,
                        SignalState::NoChangeRecommended,
                        actor,
                        &recommendation,
                        now,
                    )?;
                }
            }
            signal.seal();
            signals.push(signal);
        }
        let mut outcome_counts = BTreeMap::new();
        for observation in observations
            .iter()
            .filter(|observation| observation.duplicate_of.is_none())
        {
            *outcome_counts.entry(observation.class).or_insert(0) += 1;
        }
        let partitions = patterns::summaries(&patterns::partition(&observations, &digest));
        let qualified = signals
            .iter()
            .filter(|signal| matches!(signal.state, SignalState::Investigating))
            .count();
        let insufficient = signals
            .iter()
            .filter(|signal| signal.state == SignalState::InsufficientEvidence)
            .count();
        let no_change = signals
            .iter()
            .filter(|signal| signal.state == SignalState::NoChangeRecommended)
            .count();
        let decision = if signals.is_empty() {
            format!(
                "no signal: {} independent trial(s); nothing indicates a revision",
                coverage.independent
            )
        } else {
            format!("{} signal(s): {qualified} qualified and open for a proposal, {insufficient} with insufficient evidence, {no_change} with no change recommended; no candidate was created", signals.len())
        };
        let mut limitations = vec![
            "Qualification thresholds are deterministic decision rules, not statistical confidence.".to_string(),
            "Causality is never inferred from the order of events.".into(),
        ];
        if observations.iter().any(|observation| observation.synthetic) {
            limitations.push("Some observations come from labelled synthetic fixtures, not live production outcomes.".into());
        }
        if coverage.records_not_visible > 0 {
            limitations.push(format!(
                "{} invocation(s) by other requesters were not read (only validators see them).",
                coverage.records_not_visible
            ));
        }
        if coverage.restricted_evidence + coverage.missing_evidence > 0 {
            limitations.push(format!(
                "{} evidence reference(s) were restricted or missing and were not read.",
                coverage.restricted_evidence + coverage.missing_evidence
            ));
        }
        let mut report = AnalysisReport {
            report_id: report_id.clone(),
            schema_version: EVOLUTION_SCHEMA_VERSION,
            detector_version: DETECTOR_VERSION.into(),
            capability_id: capability_id.into(),
            version,
            capability_digest: digest,
            risk: definition.applicability.risk,
            thresholds,
            coverage,
            outcome_counts,
            partitions,
            observations,
            drift,
            signals: signals
                .iter()
                .map(|signal| (signal.signal_id.clone(), signal.kind, signal.state))
                .collect(),
            decision,
            limitations,
            actor: actor.clone(),
            created_at: now,
            digest: String::new(),
        };
        report.seal();
        let stored = self.store.transact(|state| {
            let mut stored = Vec::new();
            for signal in &signals {
                let current = state.signals.get(&signal.signal_id).cloned();
                let keep = match current {
                    Some(existing) => existing,
                    None => {
                        let stale = state
                            .signals
                            .values()
                            .filter(|other| {
                                other.capability_id == signal.capability_id
                                    && other.version == signal.version
                                    && other.kind == signal.kind
                                    && other.context_key == signal.context_key
                                    && !other.state.terminal()
                            })
                            .map(|other| other.signal_id.clone())
                            .collect::<Vec<_>>();
                        for id in stale {
                            if let Some(mut old) = state.signals.get(&id).cloned() {
                                if advance(
                                    &mut old.state,
                                    &mut old.transitions,
                                    SignalState::Superseded,
                                    actor,
                                    &format!(
                                        "superseded by {} (a different evidence set)",
                                        signal.signal_id
                                    ),
                                    now,
                                )
                                .is_ok()
                                {
                                    old.superseded_by = Some(signal.signal_id.clone());
                                    old.updated_at = now;
                                    old.seal();
                                    state.signals.insert(id, old);
                                }
                            }
                        }
                        state
                            .signals
                            .insert(signal.signal_id.clone(), signal.clone());
                        signal.clone()
                    }
                };
                stored.push(keep);
            }
            put_once(
                &mut state.analyses,
                report.report_id.clone(),
                report.clone(),
            )?;
            Ok(stored)
        })?;
        Ok(Analysis {
            report,
            signals: stored,
            reused: false,
        })
    }

    /** The verification methods observed verifying an outcome of a capability
     * Input
        - capability_id: &str - capability
     * Output
        - BTreeSet<String>
    */
    fn observed_methods(&self, capability_id: &str) -> BTreeSet<String> {
        self.invocations
            .iter()
            .filter(|invocation| {
                invocation.capability_id == capability_id && invocation.verify_digest()
            })
            .flat_map(|invocation| invocation.verifications.iter())
            .filter(|verification| verification.result == VerificationStatus::Verified)
            .map(|verification| verification.method.clone())
            .collect()
    }

    /** Persist a proposal (and optionally a signal) in one transaction
     * Input
        - proposal: &mut Proposal - proposal (sealed here)
        - signal: Option<&mut Signal> - signal update (sealed here)
     * Output
        - CapabilityResult<()>
    */
    fn persist(
        &mut self,
        proposal: &mut Proposal,
        signal: Option<&mut Signal>,
    ) -> CapabilityResult<()> {
        proposal.updated_at = self.clock.now();
        proposal.seal();
        let signal = signal.map(|signal| {
            signal.updated_at = proposal.updated_at;
            signal.seal();
            signal.clone()
        });
        let proposal = proposal.clone();
        self.store.transact(|state| {
            if let Some(signal) = signal {
                put_forward(
                    &mut state.signals,
                    signal.signal_id.clone(),
                    signal,
                    |record| record.transitions.len(),
                )?;
            }
            put_forward(
                &mut state.proposals,
                proposal.proposal_id.clone(),
                proposal,
                |record| record.transitions.len(),
            )
        })
    }

    /** Move a proposal and persist it
     * Input
        - proposal: &mut Proposal - proposal
        - to: ProposalState - new state
        - actor: &Actor - actor
        - reason: &str - reason
     * Output
        - CapabilityResult<()>
    */
    fn move_to(
        &mut self,
        proposal: &mut Proposal,
        to: ProposalState,
        actor: &Actor,
        reason: &str,
    ) -> CapabilityResult<()> {
        let now = self.clock.now();
        advance(
            &mut proposal.state,
            &mut proposal.transitions,
            to,
            actor,
            reason,
            now,
        )?;
        self.persist(proposal, None)
    }

    /** Store the draft's candidate through Component 2, or find it when an earlier attempt
     * stored it before being interrupted
     * Input
        - proposal: &mut Proposal - draft with an intended definition
        - actor: &Actor - proposer
     * Output
        - CapabilityResult<()>
    */
    fn create_candidate(&mut self, proposal: &mut Proposal, actor: &Actor) -> CapabilityResult<()> {
        let Some(intended) = proposal.intended.clone() else {
            return self.move_to(
                proposal,
                ProposalState::Blocked,
                actor,
                "the draft has no candidate definition",
            );
        };
        let digest = intended.digest();
        let existing = self
            .registry
            .state
            .versions
            .get(&proposal.capability_id)
            .and_then(|versions| {
                versions
                    .values()
                    .find(|record| record.definition_digest == digest)
            })
            .map(|record| record.definition.version);
        let version = match existing {
            Some(version) => Ok(version),
            None => {
                let profile = self
                    .registry
                    .state
                    .record(&proposal.capability_id, proposal.parent_version)?
                    .profile_id
                    .clone();
                self.registry
                    .propose_version(actor, intended.clone(), Some(&profile), self.clock.now())
                    .map(|record| record.definition.version)
            }
        };
        match version {
            Ok(version) => {
                proposal.candidate = Some(CandidateLink {
                    version,
                    digest,
                    previous_version: proposal.parent_version,
                });
                let reason = if existing.is_some() {
                    format!("recovered: candidate v{version} with the intended digest was already stored")
                } else {
                    format!("candidate v{version} stored by the registry's propose_version")
                };
                self.move_to(proposal, ProposalState::CandidateCreated, actor, &reason)
            }
            Err(error) => self.move_to(
                proposal,
                ProposalState::Blocked,
                actor,
                &format!("the registry refused the candidate: {error}"),
            ),
        }
    }

    /** Propose a revision for a qualified signal (idempotent for identical inputs)
     * Input
        - actor: &Actor - proposer (needs a registry role; becomes the candidate's author)
        - signal_id: &str - signal
        - suggestions: Vec<Change> - optional typed suggestions (hypotheses; gated like any change)
     * Output
        - CapabilityResult<Proposal> (a blocked proposal is an outcome, not an error)
        - Error when unauthorized, the signal is unknown, or it is not open for a proposal
    */
    pub(crate) fn propose(
        &mut self,
        actor: &Actor,
        signal_id: &str,
        suggestions: Vec<Change>,
    ) -> CapabilityResult<Proposal> {
        self.registry.grants.require(actor, Role::Contributor)?;
        let mut signal = self.store.signal(signal_id)?;
        if !matches!(
            signal.state,
            SignalState::Investigating | SignalState::ProposalCreated
        ) {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!("signal {signal_id} is {}; only qualified signals under investigation lead to proposals", name(&signal.state)),
            ));
        }
        let mut changes = proposals::changes_for(&signal);
        changes.extend(suggestions);
        if changes.is_empty() {
            return Err(CapabilityError::new(
                ErrorCode::InvalidSchema,
                format!(
                    "no supported change follows from a {} signal; record more evidence instead",
                    name(&signal.kind)
                ),
            ));
        }
        let change_set_digest = digest_of("evolution/change-set/v1", &changes);
        let proposal_id = stable_id("prp", &[signal_id, &change_set_digest]);
        if let Ok(mut existing) = self.store.proposal(&proposal_id) {
            if existing.state == ProposalState::Draft {
                self.create_candidate(&mut existing, actor)?;
            }
            return self.refresh(&proposal_id, actor);
        }
        let parent = self
            .registry
            .state
            .record(&signal.capability_id, signal.version)?
            .clone();
        let now = self.clock.now();
        let methods = self.observed_methods(&signal.capability_id);
        let gate = proposals::gate(
            &changes,
            &GateContext {
                parent: &parent.definition,
                signal: &signal,
                current: self.sources.current,
                observed_methods: &methods,
            },
        );
        let unsupported = gate
            .iter()
            .filter(|decision| !decision.allowed)
            .map(|decision| format!("change {}: {}", decision.index + 1, decision.reason))
            .collect::<Vec<_>>();
        let mut affected_claims = signal.affected_claims.clone();
        let mut affected_steps = signal.affected_steps.clone();
        for change in &changes {
            match &change.change {
                ChangeKind::AddVerificationRequirement { claim_id, .. }
                | ChangeKind::RemoveClaim { claim_id }
                | ChangeKind::MarkClaimVerified { claim_id } => {
                    affected_claims.push(claim_id.clone())
                }
                ChangeKind::ClarifyStep { step_id, .. } => affected_steps.push(step_id.clone()),
                _ => {}
            }
        }
        affected_claims.sort();
        affected_claims.dedup();
        affected_steps.sort();
        affected_steps.dedup();
        let mut risks = vec!["The candidate must be validated and published by authorized people; this proposal authorizes nothing.".to_string()];
        if changes
            .iter()
            .any(|change| matches!(change.change, ChangeKind::UpdateDependency { .. }))
        {
            risks.push(
                "A re-pinned dependency needs revalidation of every claim under the new version."
                    .into(),
            );
        }
        if changes
            .iter()
            .any(|change| matches!(change.change, ChangeKind::ClarifyStep { .. }))
        {
            risks.push("A reworded step changes the procedure text; its behavior cannot be compared by the Lab.".into());
        }
        let mut proposal = Proposal {
            proposal_id: proposal_id.clone(),
            schema_version: EVOLUTION_SCHEMA_VERSION,
            signal_id: signal_id.into(),
            capability_id: signal.capability_id.clone(),
            parent_version: signal.version,
            parent_digest: signal.capability_digest.clone(),
            hypothesis: signal.hypothesis.clone().unwrap_or_else(|| {
                patterns::hypothesis(
                    &patterns::Detected {
                        kind: signal.kind,
                        context_key: signal.context_key.clone(),
                        supporting: signal.supporting.clone(),
                        contradicting: signal.contradicting.clone(),
                        collapsed: signal.collapsed.clone(),
                        affected_claims: signal.affected_claims.clone(),
                        affected_steps: signal.affected_steps.clone(),
                        evidence: Vec::new(),
                        failures: 0,
                        successes: 0,
                        infrastructure: false,
                        context_governance: BTreeMap::new(),
                        drift: signal.drift.clone(),
                    },
                    (&signal.capability_id, signal.version),
                )
                .0
            }),
            changes: changes.clone(),
            change_set_digest,
            gate,
            affected_claims,
            affected_steps,
            risks,
            unsupported: unsupported.clone(),
            intended: None,
            candidate: None,
            state: ProposalState::Draft,
            transitions: vec![Transition {
                sequence: 1,
                from: None,
                to: ProposalState::Draft,
                actor: actor.clone(),
                reason: format!("hypothesis from signal {signal_id}"),
                at: now,
            }],
            proposer: actor.clone(),
            runs: Vec::new(),
            plan_id: None,
            review: None,
            created_at: now,
            updated_at: now,
            digest: String::new(),
        };
        if signal.state == SignalState::Investigating {
            advance(
                &mut signal.state,
                &mut signal.transitions,
                SignalState::ProposalCreated,
                actor,
                &format!("proposal {proposal_id}"),
                now,
            )?;
        }
        let latest = self
            .registry
            .state
            .versions
            .get(&signal.capability_id)
            .and_then(|versions| versions.keys().next_back().copied())
            .unwrap_or(signal.version);
        let mut blocked = unsupported.clone();
        if parent.definition_digest != signal.capability_digest {
            blocked.push("the parent no longer matches the analyzed digest".into());
        }
        if latest != signal.version {
            blocked.push(format!("a newer version (v{latest}) exists; the registry links a new version to the latest one, so analyze and propose against v{latest}"));
        }
        if matches!(
            parent.state,
            LifecycleState::Revoked | LifecycleState::Rejected | LifecycleState::Deprecated
        ) {
            blocked.push(format!(
                "the parent is {}; a withdrawn version is not revised",
                name(&parent.state)
            ));
        }
        let allowed = changes
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                proposal
                    .gate
                    .iter()
                    .any(|decision| decision.index == *index && decision.allowed)
            })
            .map(|(_, change)| change.clone())
            .collect::<Vec<_>>();
        if blocked.is_empty() {
            let reason = format!(
                "evolution proposal {proposal_id} from signal {signal_id} ({}): {}",
                name(&signal.kind),
                proposal.hypothesis.recommendation
            );
            let intended = proposals::candidate(&parent.definition, &allowed, actor, &reason, now);
            let weakened = proposals::weakenings(&parent.definition, &intended);
            if weakened.is_empty() {
                proposal.intended = Some(intended);
            } else {
                blocked.extend(weakened);
            }
        }
        if !blocked.is_empty() {
            advance(
                &mut proposal.state,
                &mut proposal.transitions,
                ProposalState::Blocked,
                actor,
                &blocked.join("; "),
                now,
            )?;
            self.persist(&mut proposal, Some(&mut signal))?;
            return Ok(proposal);
        }
        self.persist(&mut proposal, Some(&mut signal))?;
        self.create_candidate(&mut proposal, actor)?;
        Ok(proposal)
    }

    /** Reload a proposal and reflect Component 2's state of its candidate and parent: a rejected
     * candidate rejects the proposal; a newer version or a withdrawn parent supersedes it
     * Input
        - proposal_id: &str - proposal
        - actor: &Actor - actor recorded on the transition
     * Output
        - CapabilityResult<Proposal>
    */
    pub(crate) fn refresh(
        &mut self,
        proposal_id: &str,
        actor: &Actor,
    ) -> CapabilityResult<Proposal> {
        let mut proposal = self.store.proposal(proposal_id)?;
        if proposal.state.terminal() {
            return Ok(proposal);
        }
        let Some(link) = proposal.candidate.clone() else {
            return Ok(proposal);
        };
        let candidate = self
            .registry
            .state
            .record(&proposal.capability_id, link.version)
            .ok()
            .map(|record| record.state);
        let latest = self
            .registry
            .state
            .versions
            .get(&proposal.capability_id)
            .and_then(|versions| versions.keys().next_back().copied())
            .unwrap_or(link.version);
        let parent = self
            .registry
            .state
            .record(&proposal.capability_id, proposal.parent_version)
            .ok()
            .map(|record| record.state);
        if candidate == Some(LifecycleState::Rejected) {
            self.move_to(
                &mut proposal,
                ProposalState::Rejected,
                actor,
                "the candidate was rejected through Component 2",
            )?;
        } else if latest > link.version {
            self.move_to(
                &mut proposal,
                ProposalState::Superseded,
                actor,
                &format!("a newer version v{latest} exists"),
            )?;
        } else if matches!(parent, Some(LifecycleState::Revoked) | None) {
            self.move_to(
                &mut proposal,
                ProposalState::Superseded,
                actor,
                "the parent was withdrawn",
            )?;
        }
        Ok(proposal)
    }

    /** Build and store the regression plan of a proposal, moving it to regression_pending
     * Input
        - actor: &Actor - actor (needs a registry role)
        - proposal_id: &str - proposal
     * Output
        - CapabilityResult<RegressionPlan>
    */
    pub(crate) fn regression_plan(
        &mut self,
        actor: &Actor,
        proposal_id: &str,
    ) -> CapabilityResult<RegressionPlan> {
        self.registry.grants.require(actor, Role::Contributor)?;
        let mut proposal = self.refresh(proposal_id, actor)?;
        if proposal.state.terminal() || proposal.candidate.is_none() {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                format!(
                    "proposal {proposal_id} is {} without a usable candidate",
                    name(&proposal.state)
                ),
            ));
        }
        let signal = self.store.signal(&proposal.signal_id)?;
        let analysis = self.store.state.analyses.get(&signal.analysis_id).cloned();
        let plan = regression::build_plan(
            &self.registry.state,
            &self.sources,
            &proposal,
            analysis.as_ref(),
            self.clock.now(),
        )?;
        self.store
            .transact(|state| put_once(&mut state.plans, plan.plan_id.clone(), plan.clone()))?;
        let stored = self
            .store
            .state
            .plans
            .get(&plan.plan_id)
            .cloned()
            .unwrap_or(plan);
        proposal.plan_id = Some(stored.plan_id.clone());
        if proposal.state == ProposalState::RegressionPending {
            self.persist(&mut proposal, None)?;
        } else {
            self.move_to(
                &mut proposal,
                ProposalState::RegressionPending,
                actor,
                &format!("regression plan {}", stored.plan_id),
            )?;
        }
        Ok(stored)
    }

    /** Run a regression (a new run every time; earlier runs are never changed)
     * Input
        - actor: &Actor - actor (needs a registry role)
        - proposal_id: &str - proposal
     * Output
        - CapabilityResult<(RegressionRun, Vec<LabReport>)> the run and the Lab reports it produced
    */
    pub(crate) fn regression_run(
        &mut self,
        actor: &Actor,
        proposal_id: &str,
    ) -> CapabilityResult<(RegressionRun, Vec<LabReport>)> {
        let plan = self.regression_plan(actor, proposal_id)?;
        let mut proposal = self.store.proposal(proposal_id)?;
        let signal = self.store.signal(&proposal.signal_id)?;
        let previous = proposal
            .runs
            .last()
            .and_then(|id| self.store.state.runs.get(id).cloned());
        let (run, lab_reports) = regression::execute(
            self.registry,
            &self.sources,
            self.compilation_reports,
            &proposal,
            &plan,
            &signal,
            previous.as_ref(),
            actor,
            self.clock.now(),
        )?;
        let inserted = self
            .store
            .transact(|state| put_once(&mut state.runs, run.run_id.clone(), run.clone()))?;
        if !inserted {
            return Err(CapabilityError::new(
                ErrorCode::IdempotencyConflict,
                format!("regression run {} already exists", run.run_id),
            ));
        }
        proposal.runs.push(run.run_id.clone());
        let to = match run.state {
            RunState::Passed => ProposalState::RegressionPassed,
            RunState::Failed => ProposalState::RegressionFailed,
            RunState::Blocked => ProposalState::Blocked,
            _ => ProposalState::Inconclusive,
        };
        self.move_to(
            &mut proposal,
            to,
            actor,
            &format!("regression run {}: {}", run.run_id, run.reason),
        )?;
        Ok((run, lab_reports))
    }

    /** Prepare a review packet for a proposal whose latest run passed and still matches the
     * current context; nothing is delivered and the candidate's registry state is unchanged
     * Input
        - actor: &Actor - actor (needs a registry role)
        - proposal_id: &str - proposal
     * Output
        - CapabilityResult<Proposal>
    */
    pub(crate) fn prepare_review(
        &mut self,
        actor: &Actor,
        proposal_id: &str,
    ) -> CapabilityResult<Proposal> {
        self.registry.grants.require(actor, Role::Contributor)?;
        let mut proposal = self.refresh(proposal_id, actor)?;
        let refuse =
            |message: String| Err(CapabilityError::new(ErrorCode::InvalidTransition, message));
        if proposal.state != ProposalState::RegressionPassed {
            return refuse(format!(
                "proposal {proposal_id} is {}; a review packet needs a passed regression run",
                name(&proposal.state)
            ));
        }
        let Some(run) = proposal
            .runs
            .last()
            .and_then(|id| self.store.state.runs.get(id).cloned())
        else {
            return refuse("no regression run".into());
        };
        let Some(link) = proposal.candidate.clone() else {
            return refuse("no candidate".into());
        };
        let record = self
            .registry
            .state
            .record(&proposal.capability_id, link.version)?;
        if run.state != RunState::Passed
            || run.candidate_digest != record.definition_digest
            || run.governance != *self.sources.current
        {
            return refuse(format!("run {} no longer matches the current candidate or governance versions; run the regression again", run.run_id));
        }
        let packet = ReviewPacket {
            prepared_by: actor.clone(),
            prepared_at: self.clock.now(),
            run_id: run.run_id.clone(),
            candidate_state: record.state,
            next_steps: vec![
                format!("crane capability inspect {} version {}", proposal.capability_id, link.version),
                format!("crane capability validate {} version {}", proposal.capability_id, link.version),
                format!("crane capability publish {} version {} reason TEXT   (an authorized publisher only)", proposal.capability_id, link.version),
            ],
            notes: vec![
                "Crane has no review-request mechanism: nothing was delivered and nobody was notified.".into(),
                format!("The candidate stays {} in the registry; this packet changes no lifecycle state.", name(&record.state)),
                "A passed regression is not publication approval.".into(),
            ],
        };
        proposal.review = Some(packet);
        self.persist(&mut proposal, None)?;
        Ok(proposal)
    }

    /** Abandon a proposal with a reason (the candidate, if any, stays in the registry unchanged)
     * Input
        - actor: &Actor - the proposer or a validator
        - proposal_id: &str - proposal
        - reason: &str - why
     * Output
        - CapabilityResult<Proposal>
    */
    pub(crate) fn abandon(
        &mut self,
        actor: &Actor,
        proposal_id: &str,
        reason: &str,
    ) -> CapabilityResult<Proposal> {
        let mut proposal = self.store.proposal(proposal_id)?;
        if proposal.proposer.id != actor.id
            && self
                .registry
                .grants
                .require(actor, Role::Validator)
                .is_err()
        {
            return Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                "only the proposer or a validator may abandon a proposal",
            ));
        }
        if reason.trim().is_empty() {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                "abandoning needs a reason",
            ));
        }
        self.move_to(
            &mut proposal,
            ProposalState::Abandoned,
            actor,
            &format!("abandoned: {reason}; any candidate stays in the registry unchanged"),
        )?;
        Ok(proposal)
    }

    /** Dismiss a signal with a reason
     * Input
        - actor: &Actor - a validator (people only)
        - signal_id: &str - signal
        - reason: &str - why
     * Output
        - CapabilityResult<Signal>
    */
    pub(crate) fn dismiss(
        &mut self,
        actor: &Actor,
        signal_id: &str,
        reason: &str,
    ) -> CapabilityResult<Signal> {
        self.registry.grants.require(actor, Role::Validator)?;
        if reason.trim().is_empty() {
            return Err(CapabilityError::new(
                ErrorCode::InvalidTransition,
                "dismissing needs a reason",
            ));
        }
        let mut signal = self.store.signal(signal_id)?;
        let now = self.clock.now();
        advance(
            &mut signal.state,
            &mut signal.transitions,
            SignalState::Dismissed,
            actor,
            &format!("dismissed: {reason}"),
            now,
        )?;
        signal.updated_at = now;
        signal.seal();
        let stored = signal.clone();
        self.store.transact(|state| {
            put_forward(
                &mut state.signals,
                stored.signal_id.clone(),
                stored,
                |record| record.transitions.len(),
            )
        })?;
        Ok(signal)
    }
}
