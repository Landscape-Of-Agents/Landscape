// The governance layer: repository workspace, configuration, insert-only checkpoints, policy
// context bindings, the audit chain, the signed state, the AgentScript policy language, change
// classification, and verification of preserve and target commands.

pub(crate) mod admins;
pub(crate) mod audit;
pub(crate) mod changes;
pub(crate) mod checkpoints;
pub(crate) mod config;
pub(crate) mod context;
pub(crate) mod diff;
pub(crate) mod flows;
pub(crate) mod policy;
pub(crate) mod state;
pub(crate) mod verify;
pub(crate) mod workspace;
pub(crate) mod zones;

use serde::Serialize;

/** How serious a finding is
 * Variants
    - Low - informational; never fails a command
    - Medium - degraded assurance worth fixing; never fails a command
    - High - a check failed; the command fails
    - Critical - tampering or loss of integrity; the command fails and hooks fail closed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

/** One structured problem found by verification
 * Fields
    - code: String - machine-readable category, such as map_tampered
    - severity: Severity - how serious it is
    - message: String - explanation for humans and agents
    - selection: Option<String> - affected selection identifier
    - path: Option<String> - affected file
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    pub(crate) code: String,
    pub(crate) severity: Severity,
    pub(crate) message: String,
    pub(crate) selection: Option<String>,
    pub(crate) path: Option<String>,
}

impl Finding {
    /** Build a finding without selection or path
     * Input
        - code: &str - category
        - severity: Severity - seriousness
        - message: String - explanation
     * Output
        - Finding
    */
    pub(crate) fn new(code: &str, severity: Severity, message: String) -> Self {
        Self {
            code: code.into(),
            severity,
            message,
            selection: None,
            path: None,
        }
    }

    /** Attach the affected selection
     * Input
        - id: &str - selection identifier
     * Output
        - Finding
    */
    pub(crate) fn with_selection(mut self, id: &str) -> Self {
        self.selection = Some(id.into());
        self
    }

    /** Attach the affected file
     * Input
        - path: &str - repository-relative path
     * Output
        - Finding
    */
    pub(crate) fn with_path(mut self, path: &str) -> Self {
        self.path = Some(path.into());
        self
    }

    /** Report whether the finding fails a command
     * Input
        - None (uses self)
     * Output
        - bool, true for High and Critical
    */
    pub(crate) fn blocking(&self) -> bool {
        self.severity >= Severity::High
    }
}
