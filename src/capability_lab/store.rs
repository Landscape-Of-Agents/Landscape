// Lab report storage and the report-reference interface. Reports are appended, one JSON line
// each, to the trust directory's runtime/capability_lab/reports.jsonl (the same append-only
// convention as compilation reports); they hold references, never copies of evidence. Loading
// verifies every report's digest and fails on any that does not match, so a tampered or truncated
// record is reported instead of skipped. references() is the bounded interface other components
// may use to find the lab reports of an exact capability version: it informs, and grants nothing
// (Component 2 has no interface that consumes lab results, and none is added here).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::report::LabReport;
use super::CheckStatus;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode};
use crate::platform::files::{append_line, read_lines};

/** A reference to a stored lab report
 * Fields
    - report_id: String - report
    - digest: String - its digest
    - candidate_digest: String - definition digest it examined
    - overall: CheckStatus - its overall status
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LabReportRef {
    pub(crate) report_id: String,
    pub(crate) digest: String,
    pub(crate) candidate_digest: String,
    pub(crate) overall: CheckStatus,
    pub(crate) created_at: u64,
}

/** The append-only lab report store
 * Fields
    - path: PathBuf - the reports file
*/
#[derive(Debug, Clone)]
pub(crate) struct LabStore {
    path: PathBuf,
}

impl LabStore {
    /** Open the store in a runtime directory
     * Input
        - runtime: &Path - the repository's runtime directory
     * Output
        - LabStore
    */
    pub(crate) fn open(runtime: &Path) -> Self {
        Self {
            path: runtime.join("capability_lab").join("reports.jsonl"),
        }
    }

    /** Append a report after checking its digest
     * Input
        - report: &LabReport - report
     * Output
        - CapabilityResult<()>
        - Error when the report does not match its digest or cannot be written
    */
    pub(crate) fn append(&self, report: &LabReport) -> CapabilityResult<()> {
        if !report.verify_digest() {
            return Err(CapabilityError::new(
                ErrorCode::IntegrityMismatch,
                "the lab report does not match its digest",
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
        - CapabilityResult<Vec<LabReport>>
        - Error naming the first unreadable or tampered report
    */
    pub(crate) fn load(&self) -> CapabilityResult<Vec<LabReport>> {
        let lines = read_lines(&self.path)
            .map_err(|error| CapabilityError::new(ErrorCode::Persistence, error))?;
        lines
            .into_iter()
            .enumerate()
            .map(|(index, value)| {
                let report: LabReport = serde_json::from_value(value).map_err(|error| {
                    CapabilityError::new(
                        ErrorCode::Persistence,
                        format!("lab report {}: {error}", index + 1),
                    )
                })?;
                if report.verify_digest() {
                    Ok(report)
                } else {
                    Err(CapabilityError::new(
                        ErrorCode::IntegrityMismatch,
                        format!(
                            "lab report {} ({}) does not match its digest",
                            index + 1,
                            report.report_id
                        ),
                    ))
                }
            })
            .collect()
    }

    /** References to the lab reports of an exact capability version
     * Input
        - capability_id: &str - capability
        - version: u32 - version
     * Output
        - CapabilityResult<Vec<LabReportRef>>
    */
    pub(crate) fn references(
        &self,
        capability_id: &str,
        version: u32,
    ) -> CapabilityResult<Vec<LabReportRef>> {
        Ok(self
            .load()?
            .into_iter()
            .filter(|report| report.capability_id == capability_id && report.version == version)
            .map(|report| LabReportRef {
                report_id: report.report_id,
                digest: report.digest,
                candidate_digest: report.candidate_digest,
                overall: report.overall,
                created_at: report.created_at,
            })
            .collect())
    }
}
