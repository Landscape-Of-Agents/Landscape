// Compliance inputs: what a machine verified locally (preserve and target commands, integrity
// findings, the rollback anchor, hooks, the event, audit, and ledger chains, and alerting), sent
// with its repository status.

/** What a machine reports for compliance
 * Fields
    - preserve: Option<(usize, usize)> - preserve commands (passed, total) from the latest test
    - target: Option<(usize, usize)> - target commands (passed, total)
    - integrity_findings: usize - blocking integrity findings
    - anchor: bool - a rollback anchor exists
    - hooks_valid: bool - at least one provider's hooks validate
    - events_verified: bool - the event chain verifies
    - audit_verified: bool - the governance audit chain verifies
    - ledger_verified: bool - the ledger reconciles
    - alerts_configured: bool - Slack or WhatsApp is configured
    - alert_rules: usize - enabled alert rules ('crane alert add')
*/
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub(crate) struct Inputs {
    pub(crate) preserve: Option<(usize, usize)>,
    pub(crate) target: Option<(usize, usize)>,
    pub(crate) integrity_findings: usize,
    pub(crate) anchor: bool,
    pub(crate) hooks_valid: bool,
    pub(crate) events_verified: bool,
    pub(crate) audit_verified: bool,
    pub(crate) ledger_verified: bool,
    pub(crate) alerts_configured: bool,
    pub(crate) alert_rules: usize,
}
