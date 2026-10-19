// The discovery report and its storage. discover() authorizes and resolves the request, evaluates
// the registry's capabilities, ranks the recommended ones, and returns a versioned,
// integrity-protected report of the inputs, the resolved context with its bases, the registry
// generation and governance versions it ran against, every candidate's digest, conditions,
// lab reports, disposition, and ranking reasons, the recommendations, and what stayed unresolved.
// A refused request still yields a report (outcome blocked, nothing retrieved). Reports are
// appended to the trust directory's runtime/discovery/reports.jsonl and verified when loaded.
// Nothing in the registry changes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::evaluate::{evaluate_all, CandidateEvaluation, Disposition};
use super::resolve::{resolve, DiscoveryRequest, ResolvedContext};
use super::{DiscoveryOutcome, ENGINE_VERSION, RANKING_VERSION, REPORT_SCHEMA_VERSION};
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::CapabilityRegistry;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::capability_lab::report::LabReport;
use crate::capture::{stable_id, Actor};
use crate::platform::files::{append_line, read_lines};
use crate::trust::crypto::digest_json;

/** A recommendation
 * Fields
    - rank: usize - rank (1 is best)
    - capability_id: String - capability
    - version: u32 - version
    - digest: String - definition digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Recommendation {
    pub(crate) rank: usize,
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
}

/** A discovery report
 * Fields
    - report_id: String - stable identifier (from the digest)
    - schema_version: u32 - REPORT_SCHEMA_VERSION
    - engine_version: String - ENGINE_VERSION
    - ranking_version: String - RANKING_VERSION
    - actor: Actor - requester
    - request: DiscoveryRequest - request as received
    - context: Option<ResolvedContext> - resolved context, None when refused
    - registry_generation: u64 - registry generation evaluated
    - governance: CurrentVersions - Landscape and Crane versions evaluated against
    - candidates: Vec<CandidateEvaluation> - every candidate evaluated
    - truncated: bool - whether the candidate limit was reached
    - recommendations: Vec<Recommendation> - recommended versions in rank order
    - outcome: DiscoveryOutcome - outcome
    - reasons: Vec<String> - why
    - unresolved: Vec<String> - what stayed unknown
    - created_at: u64 - Unix milliseconds
    - digest: String - digest of the report without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DiscoveryReport {
    pub(crate) report_id: String,
    pub(crate) schema_version: u32,
    pub(crate) engine_version: String,
    pub(crate) ranking_version: String,
    pub(crate) actor: Actor,
    pub(crate) request: DiscoveryRequest,
    pub(crate) context: Option<ResolvedContext>,
    pub(crate) registry_generation: u64,
    pub(crate) governance: CurrentVersions,
    pub(crate) candidates: Vec<CandidateEvaluation>,
    pub(crate) truncated: bool,
    pub(crate) recommendations: Vec<Recommendation>,
    pub(crate) outcome: DiscoveryOutcome,
    pub(crate) reasons: Vec<String>,
    pub(crate) unresolved: Vec<String>,
    pub(crate) created_at: u64,
    pub(crate) digest: String,
}

impl DiscoveryReport {
    /** The digest of the content without the digest and identifier
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn content_digest(&self) -> String {
        let mut content = self.clone();
        content.digest = String::new();
        content.report_id = String::new();
        digest_json("discovery/report/v1", &json!(content))[..32].to_string()
    }

    /** Set the digest and identifier
     * Input
        - None (uses self)
     * Output
        - DiscoveryReport
    */
    fn seal(mut self) -> Self {
        let digest = self.content_digest();
        self.report_id = stable_id("dsc", &[&digest]);
        self.digest = digest;
        self
    }

    /** Check that the report still matches its digest and identifier
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verify_digest(&self) -> bool {
        let digest = self.content_digest();
        digest == self.digest && self.report_id == stable_id("dsc", &[&digest])
    }
}

/** Run a discovery
 * Input
    - registry: &CapabilityRegistry - Component 2 registry (read only)
    - sources: &Sources - Work Capture records, current versions, the requester's access policy
    - actor: &Actor - requester
    - request: &DiscoveryRequest - request
    - lab_reports: &[LabReport] - Component 4 reports (verified when loaded)
    - now: u64 - Unix milliseconds
 * Output
    - DiscoveryReport
*/
pub(crate) fn discover(
    registry: &CapabilityRegistry,
    sources: &Sources,
    actor: &Actor,
    request: &DiscoveryRequest,
    lab_reports: &[LabReport],
    now: u64,
) -> DiscoveryReport {
    let mut report = DiscoveryReport {
        report_id: String::new(),
        schema_version: REPORT_SCHEMA_VERSION,
        engine_version: ENGINE_VERSION.into(),
        ranking_version: RANKING_VERSION.into(),
        actor: actor.clone(),
        request: request.clone(),
        context: None,
        registry_generation: registry.state.generation,
        governance: sources.current.clone(),
        candidates: Vec::new(),
        truncated: false,
        recommendations: Vec::new(),
        outcome: DiscoveryOutcome::Blocked,
        reasons: Vec::new(),
        unresolved: Vec::new(),
        created_at: now,
        digest: String::new(),
    };
    let context = match resolve(request, &registry.grants, sources, actor) {
        Ok(context) => context,
        Err(error) => {
            report.reasons.push(format!(
                "the request was refused before any retrieval: {error}"
            ));
            return report.seal();
        }
    };
    let (candidates, truncated) = evaluate_all(&registry.state, &context, sources, lab_reports);
    let count = |disposition: Disposition| {
        candidates
            .iter()
            .filter(|candidate| candidate.disposition == disposition)
            .count()
    };
    report.recommendations = candidates
        .iter()
        .filter_map(|candidate| {
            Some(Recommendation {
                rank: candidate.rank?,
                capability_id: candidate.capability_id.clone(),
                version: candidate.version?,
                digest: candidate.digest.clone()?,
            })
        })
        .collect();
    let (outcome, reason) = if !report.recommendations.is_empty() {
        (
            DiscoveryOutcome::Recommended,
            format!(
                "{} candidate(s) passed every mandatory gate",
                report.recommendations.len()
            ),
        )
    } else if count(Disposition::Unresolved) > 0 {
        (
            DiscoveryOutcome::InsufficientContext,
            format!(
                "{} candidate(s) could apply, but unknown conditions keep them unresolved",
                count(Disposition::Unresolved)
            ),
        )
    } else if count(Disposition::BlockedByLab) + count(Disposition::BlockedByDependency) > 0 {
        (DiscoveryOutcome::Blocked, "every applicable candidate is blocked by a failed lab report or an unresolvable required dependency".to_string())
    } else if candidates.is_empty() {
        (
            DiscoveryOutcome::NoMatch,
            "the registry holds no capability".to_string(),
        )
    } else {
        (
            DiscoveryOutcome::NoMatch,
            "no published, currently eligible capability applies".to_string(),
        )
    };
    report.outcome = outcome;
    report.reasons.push(reason);
    report.unresolved = context.unresolved.clone();
    report.unresolved.extend(
        candidates
            .iter()
            .filter(|candidate| candidate.disposition == Disposition::Unresolved)
            .flat_map(|candidate| {
                candidate
                    .reasons
                    .iter()
                    .map(move |reason| format!("{}: {reason}", candidate.capability_id))
            }),
    );
    report.context = Some(context);
    report.candidates = candidates;
    report.truncated = truncated;
    report.seal()
}

/** The append-only discovery report store
 * Fields
    - path: PathBuf - reports file
*/
#[derive(Debug, Clone)]
pub(crate) struct DiscoveryStore {
    path: PathBuf,
}

impl DiscoveryStore {
    /** Open the store in a runtime directory
     * Input
        - runtime: &Path - runtime directory
     * Output
        - DiscoveryStore
    */
    pub(crate) fn open(runtime: &Path) -> Self {
        Self {
            path: runtime.join("discovery").join("reports.jsonl"),
        }
    }

    /** Append a report after checking its digest
     * Input
        - report: &DiscoveryReport - report
     * Output
        - CapabilityResult<()>
    */
    pub(crate) fn append(&self, report: &DiscoveryReport) -> CapabilityResult<()> {
        if !report.verify_digest() {
            return Err(CapabilityError::new(
                ErrorCode::IntegrityMismatch,
                "the discovery report does not match its digest",
            ));
        }
        let line = serde_json::to_string(report)
            .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error.to_string()))?;
        append_line(&self.path, &line)
            .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error))
    }

    /** Load every report, verifying each digest
     * Input
        - None (uses self)
     * Output
        - CapabilityResult<Vec<DiscoveryReport>>
        - Error naming the first unreadable or tampered report
    */
    pub(crate) fn load(&self) -> CapabilityResult<Vec<DiscoveryReport>> {
        let lines = read_lines(&self.path)
            .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error))?;
        lines
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let report: DiscoveryReport = serde_json::from_value(value).map_err(|error| {
                    CapabilityError::new(
                        ErrorCode::Persistence,
                        format!("discovery report {}: {error}", index + 1),
                    )
                })?;
                if report.verify_digest() {
                    Ok(report)
                } else {
                    Err(CapabilityError::new(
                        ErrorCode::IntegrityMismatch,
                        format!("discovery report {} does not match its digest", index + 1),
                    ))
                }
            })
            .collect()
    }
}
