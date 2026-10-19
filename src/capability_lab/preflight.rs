// Preflight: the lab examines one exact candidate version. The stored definition must match the
// digest the requester names and its own recomputed digest; rejected and revoked versions are not
// examined. A compiler-created candidate needs its compilation report: the report must be intact
// (its digest recomputes) and must name this exact capability, version, and candidate digest.
// Blocking gaps the compiler recorded are carried into the lab result as blocked checks; the lab
// never turns them into passes.

use super::{CheckCategory, CheckStatus, LabCheck};
use crate::capability::lifecycle::LifecycleState;
use crate::capability::model::{CapabilityDefinition, CreationMethod};
use crate::capability::registry::RegistryState;
use crate::compiler::assemble::CompilationReport;
use crate::compiler::CompilationOutcome;

/** The exact version to examine
 * Fields
    - capability_id: String - capability
    - version: u32 - version
    - expected_digest: String - definition digest the requester expects
*/
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct Target {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) expected_digest: String,
}

/** The preflight result
 * Fields
    - checks: Vec<LabCheck> - preflight checks
    - definition: Option<CapabilityDefinition> - the definition, when it may be examined
    - state: Option<LifecycleState> - its lifecycle state
    - compilation_usable: bool - the compilation report is intact and matches
*/
#[derive(Debug, Clone)]
pub(crate) struct Preflight {
    pub(crate) checks: Vec<LabCheck>,
    pub(crate) definition: Option<CapabilityDefinition>,
    pub(crate) state: Option<LifecycleState>,
    pub(crate) compilation_usable: bool,
}

/** Build a preflight check
 * Input
    - name: &str - check name
    - status: CheckStatus - status
    - detail: impl Into<String> - explanation
 * Output
    - LabCheck
*/
fn check(name: &str, status: CheckStatus, detail: impl Into<String>) -> LabCheck {
    LabCheck::new(
        format!("preflight:{name}"),
        CheckCategory::Preflight,
        status,
        detail,
    )
}

/** Run the preflight
 * Input
    - state: &RegistryState - registry
    - target: &Target - exact version
    - compilation: Option<&CompilationReport> - its compilation report
 * Output
    - Preflight
*/
pub(crate) fn preflight(
    state: &RegistryState,
    target: &Target,
    compilation: Option<&CompilationReport>,
) -> Preflight {
    let mut result = Preflight {
        checks: Vec::new(),
        definition: None,
        state: None,
        compilation_usable: false,
    };
    let Ok(record) = state.record(&target.capability_id, target.version) else {
        result.checks.push(check(
            "candidate",
            CheckStatus::Blocked,
            format!(
                "{} v{} does not exist in the registry",
                target.capability_id, target.version
            ),
        ));
        return result;
    };
    result.state = Some(record.state);
    let recomputed = record.definition.digest();
    if record.definition_digest != target.expected_digest || recomputed != record.definition_digest
    {
        result.checks.push(check("candidate-digest", CheckStatus::Failed, format!("expected digest {}, stored {}, recomputed {}", target.expected_digest, record.definition_digest, recomputed)).with_counterexample(serde_json::json!({"expected": target.expected_digest, "stored": record.definition_digest, "recomputed": recomputed})));
        return result;
    }
    result.checks.push(check(
        "candidate-digest",
        CheckStatus::Passed,
        format!(
            "{} v{} has digest {}",
            target.capability_id, target.version, recomputed
        ),
    ));
    if matches!(
        record.state,
        LifecycleState::Rejected | LifecycleState::Revoked
    ) {
        result.checks.push(check(
            "candidate-state",
            CheckStatus::Blocked,
            format!(
                "the version is {:?}; withdrawn versions are not examined",
                record.state
            ),
        ));
        return result;
    }
    result.checks.push(check(
        "candidate-state",
        CheckStatus::Passed,
        format!(
            "the version is {:?}; the lab changes no lifecycle state",
            record.state
        ),
    ));
    let definition = record.definition.clone();
    if definition.provenance.creation_method != CreationMethod::Compiler {
        result.checks.push(check(
            "compilation-report",
            CheckStatus::NotRun,
            format!(
                "created {:?}, not by the compiler; step conformance cannot be checked",
                definition.provenance.creation_method
            ),
        ));
        result.definition = Some(definition);
        return result;
    }
    match compilation {
        None => result.checks.push(check(
            "compilation-report",
            CheckStatus::Blocked,
            "a compiler-created candidate needs its compilation report",
        )),
        Some(report) if !report.verify_digest() => result.checks.push(check(
            "compilation-report",
            CheckStatus::Failed,
            "the compilation report does not match its digest; it was changed after it was written",
        )),
        Some(report) => {
            let candidate = report.candidate.as_ref();
            let matches = candidate.is_some_and(|candidate| {
                candidate.capability_id == target.capability_id
                    && candidate.version == target.version
                    && candidate.digest == recomputed
            });
            if !matches
                || !matches!(
                    report.outcome,
                    CompilationOutcome::CandidateGenerated | CompilationOutcome::NeedsMoreEvidence
                )
            {
                result.checks.push(check("compilation-report", CheckStatus::Failed, format!("compilation report {} ({:?}) does not describe {} v{} with digest {recomputed}", report.report_id, report.outcome, target.capability_id, target.version)).with_refs(vec![report.report_id.clone()]));
            } else {
                result.compilation_usable = true;
                result.checks.push(check("compilation-report", CheckStatus::Passed, format!("compilation report {} ({:?}) is intact and describes this exact version", report.report_id, report.outcome)).with_refs(vec![report.report_id.clone()]));
                let blocking = report
                    .gaps
                    .iter()
                    .filter(|gap| gap.blocking)
                    .collect::<Vec<_>>();
                if blocking.is_empty() {
                    result.checks.push(check(
                        "blocking-gaps",
                        CheckStatus::Passed,
                        "the compiler recorded no blocking gap",
                    ));
                } else {
                    for (index, gap) in blocking.iter().enumerate() {
                        result.checks.push(
                            LabCheck::new(
                                format!("preflight:blocking-gap-{}", index + 1),
                                CheckCategory::Preflight,
                                CheckStatus::Blocked,
                                format!("{:?}: {}", gap.kind, gap.detail),
                            )
                            .with_refs(gap.refs.clone()),
                        );
                    }
                }
            }
        }
    }
    result.definition = Some(definition);
    result
}
