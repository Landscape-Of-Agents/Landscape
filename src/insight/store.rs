// The kept insight reports: one JSON line per report in the trust directory's runtime folder
// (runtime/insight/reports.jsonl), outside the repository and apart from Work Capture's store.
// Reports are only appended; a report that does not match its digest is refused when read.

use std::path::Path;

use super::InsightReport;
use crate::platform::files::{append_line, read_lines, Lock};

/** Keep a report
 * Input
    - directory: &Path - insight directory
    - report: &InsightReport - report
 * Output
    - Result<(), String>
    - Error when the report cannot be written
*/
pub(crate) fn keep(directory: &Path, report: &InsightReport) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let _lock = Lock::acquire(&directory.join("insight.lock"))?;
    let line = serde_json::to_string(report).map_err(|error| error.to_string())?;
    append_line(&directory.join("reports.jsonl"), &line)
}

/** The kept reports, oldest first
 * Input
    - directory: &Path - insight directory
 * Output
    - Result<Vec<InsightReport>, String>
    - Error when the file is unreadable or a report does not match its digest
*/
pub(crate) fn reports(directory: &Path) -> Result<Vec<InsightReport>, String> {
    read_lines(&directory.join("reports.jsonl"))?
        .into_iter()
        .map(|value| {
            let report = serde_json::from_value::<InsightReport>(value)
                .map_err(|error| format!("reports.jsonl: {error}"))?;
            if report.verifies() {
                Ok(report)
            } else {
                Err(format!(
                    "insight report {} does not match its digest",
                    report.report_id
                ))
            }
        })
        .collect()
}
