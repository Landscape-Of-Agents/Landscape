// Compliance evidence. Each control states its objective, enforcement rule, required evidence,
// verification method, and framework mappings, and is evaluated only from evidence Crane actually
// holds: SATISFIED needs positive evidence, missing evidence is NO_EVIDENCE, controls Crane cannot
// instrument are UNSUPPORTED, and partial coverage is PARTIAL with its gaps listed. Framework
// mappings (OWASP LLM and agentic guidance, NIST AI RMF, SOC 2, ISO/IEC 27001:2022, GDPR) show which
// requirements the evidence supports; they are not certification.

// Evaluated by the hosted dashboard; the CLI only builds the Inputs it reports
#![cfg_attr(not(feature = "server"), allow(dead_code))]

use serde::Serialize;
use serde_json::Value;

use super::events::Event;
use super::ledger::Entry;
use super::metrics::Metric;

/** A control definition
 * Fields
    - id: &'static str - control identifier
    - version: u32 - control version
    - objective: &'static str - what the control achieves
    - rule: &'static str - how Crane enforces it
    - evidence: &'static str - evidence required
    - method: &'static str - how the evidence is verified
    - mappings: &'static [&'static str] - framework references the evidence supports
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Control {
    pub(crate) id: &'static str,
    pub(crate) version: u32,
    pub(crate) objective: &'static str,
    pub(crate) rule: &'static str,
    pub(crate) evidence: &'static str,
    pub(crate) method: &'static str,
    pub(crate) mappings: &'static [&'static str],
}

/** The controls Crane reports */
pub(crate) const CONTROLS: &[Control] = &[
    Control { id: "CR-01", version: 1, objective: "Protected code stays identical to its trusted baseline", rule: "preserve commands; pre-write simulation denies changes; post-action verification detects others", evidence: "passing preserve commands with signed records", method: "content digest against the signed origin digest", mappings: &["OWASP LLM06 Excessive Agency", "NIST AI RMF MANAGE 2.4", "SOC 2 CC8.1", "ISO/IEC 27001 A.8.32"] },
    Control { id: "CR-02", version: 1, objective: "Required changes happen within their selection and are meaningful", rule: "target commands with change-type semantics; whitespace-only changes are rejected", evidence: "passing target commands", method: "lexical change classification against the checkpoint baseline", mappings: &["NIST AI RMF MEASURE 2.3", "SOC 2 CC8.1"] },
    Control { id: "CR-03", version: 1, objective: "Governance state is authentic, current, and not rolled back", rule: "Ed25519-signed manifest and records, SHA-512 digests, rollback anchor outside the repository", evidence: "no integrity findings; rollback anchor present", method: "signature, digest, and generation verification", mappings: &["OWASP Agentic T3 Privilege Compromise", "NIST AI RMF GOVERN 1.4", "SOC 2 CC6.1", "ISO/IEC 27001 A.8.24"] },
    Control { id: "CR-04", version: 1, objective: "Every agent action is authorized before it runs", rule: "pre-tool hooks for every tool; failures deny (fail closed)", evidence: "valid hooks and a decision for every tool call", method: "hook validation and decision/verification correlation", mappings: &["OWASP Agentic T2 Tool Misuse", "NIST AI RMF MANAGE 2.2", "SOC 2 CC6.1", "ISO/IEC 27001 A.5.15"] },
    Control { id: "CR-05", version: 1, objective: "Actual effects are verified after every action", rule: "post-tool verification of written files or the whole work tree", evidence: "a verification for every allowed tool call", method: "evidence completeness of tool calls", mappings: &["NIST AI RMF MEASURE 2.7", "SOC 2 CC7.2", "ISO/IEC 27001 A.8.16"] },
    Control { id: "CR-06", version: 1, objective: "Audit evidence is tamper-evident and reconstructable", rule: "hash-chained events, audit trail, and ledger; signed session attestations", evidence: "verified chains and signed attestations", method: "chain verification", mappings: &["OWASP Agentic T8 Repudiation", "SOC 2 CC7.2", "ISO/IEC 27001 A.8.15", "ISO/IEC 27001 A.5.28"] },
    Control { id: "CR-07", version: 1, objective: "Agents cannot change their own authority", rule: "denial of writes to .crane, trust material, hook settings, markers, and governance commands", evidence: "bypass attempts prevented and no confirmed bypass", method: "decision and post-action outcomes", mappings: &["OWASP Agentic T3 Privilege Compromise", "OWASP LLM06 Excessive Agency", "SOC 2 CC6.8", "ISO/IEC 27001 A.8.2"] },
    Control { id: "CR-08", version: 1, objective: "Autonomy is bounded by a reconcilable credit budget", rule: "atomic, idempotent ledger; exhausted credits require approval", evidence: "ledger chain and balances reconcile", method: "ledger verification", mappings: &["OWASP LLM10 Unbounded Consumption", "OWASP Agentic T4 Resource Overload", "NIST AI RMF MANAGE 2.4"] },
    Control { id: "CR-09", version: 1, objective: "Secrets are not persisted in evidence", rule: "redaction before persistence; raw tool inputs and prompts are never stored", evidence: "redaction applied to every stored event", method: "design (only digests of inputs and prompts are stored) and redaction counters", mappings: &["OWASP LLM02 Sensitive Information Disclosure", "GDPR Art. 25", "GDPR Art. 32", "ISO/IEC 27001 A.8.12"] },
    Control { id: "CR-10", version: 1, objective: "Network egress by agents is visible", rule: "destinations inferred from commands and tool arguments", evidence: "observed network telemetry", method: "network instrumentation (not available in hooks)", mappings: &["OWASP Agentic T2 Tool Misuse", "ISO/IEC 27001 A.8.16"] },
    Control { id: "CR-11", version: 1, objective: "Every action is attributable to a canonical identity", rule: "canonical session and task identifiers with provider provenance", evidence: "sessions with provider-reported identities", method: "identity provenance", mappings: &["OWASP Agentic T9 Identity Spoofing", "NIST AI RMF GOVERN 1.4", "SOC 2 CC6.1"] },
    Control { id: "CR-12", version: 1, objective: "Humans are alerted and can intervene", rule: "approvals, quarantine, and alert delivery", evidence: "an alert channel and recorded interventions", method: "integration configuration and alert log", mappings: &["NIST AI RMF MANAGE 4.1", "SOC 2 CC7.3", "ISO/IEC 27001 A.5.24"] },
    Control { id: "CR-13", version: 1, objective: "Instruction-integrity attacks are screened", rule: "prompt phrase screening (heuristic)", evidence: "screened prompts", method: "heuristic only; not a guarantee", mappings: &["OWASP LLM01 Prompt Injection", "OWASP Agentic T6 Intent Breaking"] },
];

/** The evaluation of one control
 * Fields
    - control: &'static Control - definition
    - outcome: &'static str - SATISFIED, PARTIAL, NOT_SATISFIED, NO_EVIDENCE, or UNSUPPORTED
    - evidence: Vec<String> - evidence found
    - gaps: Vec<String> - what is missing
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Evaluation {
    pub(crate) control: &'static Control,
    pub(crate) outcome: &'static str,
    pub(crate) evidence: Vec<String>,
    pub(crate) gaps: Vec<String>,
}

/** What the evaluation can draw on
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
}

/** Read a numeric metric
 * Input
    - metrics: &[Metric] - computed metrics
    - name: &str - metric name
 * Output
    - Option<f64>, None when unobserved
*/
fn metric(metrics: &[Metric], name: &str) -> Option<f64> {
    metrics
        .iter()
        .find(|metric| metric.name == name)
        .and_then(|metric| metric.value.as_f64())
}

/** Evaluate every control
 * Input
    - inputs: &Inputs - verification results
    - metrics: &[Metric] - repository metrics
    - events: &[&Event] - events
    - ledger: &[&Entry] - ledger entries
    - sessions: &[Value] - session records
 * Output
    - Vec<Evaluation> in control order
*/
pub(crate) fn evaluate(
    inputs: &Inputs,
    metrics: &[Metric],
    events: &[&Event],
    ledger: &[&Entry],
    sessions: &[Value],
) -> Vec<Evaluation> {
    let ratio =
        |pair: Option<(usize, usize)>, label: &str| -> (&'static str, Vec<String>, Vec<String>) {
            match pair {
                None => (
                    "NO_EVIDENCE",
                    Vec::new(),
                    vec![format!("no {label} commands have been evaluated")],
                ),
                Some((_, 0)) => (
                    "NO_EVIDENCE",
                    Vec::new(),
                    vec![format!("no {label} commands exist")],
                ),
                Some((passed, total)) if passed == total => (
                    "SATISFIED",
                    vec![format!("{passed}/{total} {label} commands pass")],
                    Vec::new(),
                ),
                Some((passed, total)) => (
                    "NOT_SATISFIED",
                    vec![format!("{passed}/{total} {label} commands pass")],
                    vec![format!("{} {label} commands fail", total - passed)],
                ),
            }
        };
    let completeness = metric(metrics, "evidence_completeness");
    let bypass_attempts = metric(metrics, "bypass_attempts");
    let confirmed = metric(metrics, "bypasses_confirmed").unwrap_or(0.0);
    let prevented = metric(metrics, "prevented_actions");
    let signed = metric(metrics, "attestations_signed").unwrap_or(0.0);
    let generated = sessions
        .iter()
        .filter(|session| session["provenance"] == "generated")
        .count();
    CONTROLS
        .iter()
        .map(|control| {
            let (outcome, evidence, gaps): (&'static str, Vec<String>, Vec<String>) = match control.id {
                "CR-01" => ratio(inputs.preserve, "preserve"),
                "CR-02" => ratio(inputs.target, "target"),
                "CR-03" => match (inputs.integrity_findings, inputs.anchor) {
                    (0, true) => ("SATISFIED", vec!["signatures, digests, and generation verify".into(), "rollback anchor present".into()], Vec::new()),
                    (0, false) => ("PARTIAL", vec!["signatures and digests verify".into()], vec!["no rollback anchor on this machine yet".into()]),
                    (count, _) => ("NOT_SATISFIED", Vec::new(), vec![format!("{count} blocking integrity findings")]),
                },
                "CR-04" => match (inputs.hooks_valid, metric(metrics, "missing_decisions").map(|missing| missing as u64)) {
                    (false, _) => ("NOT_SATISFIED", Vec::new(), vec!["no valid agent hooks are installed".into()]),
                    (true, None) => ("NO_EVIDENCE", vec!["hooks validate".into()], vec!["no agent activity recorded yet".into()]),
                    (true, Some(0)) => ("SATISFIED", vec!["hooks validate".into(), "every executed tool call had a pre-tool decision".into()], Vec::new()),
                    (true, Some(missing)) => ("PARTIAL", vec!["hooks validate".into()], vec![format!("{missing} tool calls ran without a recorded decision")]),
                },
                "CR-05" => match completeness {
                    None => ("NO_EVIDENCE", Vec::new(), vec!["no allowed tool calls recorded".into()]),
                    Some(value) if value >= 1.0 => ("SATISFIED", vec!["every allowed tool call was verified after it ran".into()], Vec::new()),
                    Some(value) => ("PARTIAL", vec![format!("{:.0}% of allowed tool calls verified", value * 100.0)], vec!["some allowed tool calls have no post-tool verification".into()]),
                },
                "CR-06" => {
                    let mut gaps = Vec::new();
                    if !inputs.events_verified { gaps.push("the event chain does not verify".to_string()); }
                    if !inputs.audit_verified { gaps.push("the governance audit chain does not verify".to_string()); }
                    if signed == 0.0 { gaps.push("no signed session attestation yet".to_string()); }
                    let outcome = if !inputs.events_verified || !inputs.audit_verified { "NOT_SATISFIED" } else if gaps.is_empty() { "SATISFIED" } else { "PARTIAL" };
                    (outcome, vec![format!("{} events, {} signed attestations", events.len(), signed)], gaps)
                }
                "CR-07" => match (bypass_attempts, confirmed > 0.0) {
                    (_, true) => ("NOT_SATISFIED", Vec::new(), vec![format!("{confirmed} confirmed bypasses")]),
                    (None, false) => ("NO_EVIDENCE", Vec::new(), vec!["no agent activity recorded yet".into()]),
                    (Some(attempts), false) => ("SATISFIED", vec![format!("{attempts} bypass attempts, {} prevented actions, no confirmed bypass", prevented.unwrap_or(0.0))], Vec::new()),
                },
                "CR-08" => match (ledger.is_empty(), inputs.ledger_verified) {
                    (true, _) => ("NO_EVIDENCE", Vec::new(), vec!["no ledger entries yet".into()]),
                    (false, true) => ("SATISFIED", vec![format!("{} ledger entries reconcile", ledger.len())], Vec::new()),
                    (false, false) => ("NOT_SATISFIED", Vec::new(), vec!["the ledger does not reconcile".into()]),
                },
                "CR-09" => ("PARTIAL", vec!["raw tool inputs and prompts are stored only as SHA-512 digests".into(), format!("{} secrets redacted", metric(metrics, "secrets_redacted").unwrap_or(0.0))], vec!["pattern redaction cannot prove that no secret remains in reasons or resource names".into()]),
                "CR-10" => ("UNSUPPORTED", vec![format!("{} destinations inferred", metrics.iter().find(|metric| metric.name == "network_destinations").and_then(|metric| metric.value.as_array().map(Vec::len)).unwrap_or(0))], vec!["Crane does not observe network traffic; destinations are parsed from commands, and unrestricted shells can reach the network unobserved".into()]),
                "CR-11" => match (sessions.len(), generated) {
                    (0, _) => ("NO_EVIDENCE", Vec::new(), vec!["no sessions yet".into()]),
                    (total, 0) => ("SATISFIED", vec![format!("{total} sessions with provider-reported identities")], Vec::new()),
                    (total, generated) => ("PARTIAL", vec![format!("{} of {total} sessions with provider identities", total - generated)], vec![format!("{generated} sessions have Crane-generated identities")]),
                },
                "CR-12" => if inputs.alerts_configured { ("SATISFIED", vec!["an alert channel is configured".into()], Vec::new()) } else { ("PARTIAL", vec!["approvals and quarantine are enforced by the hooks".into()], vec!["no Slack or WhatsApp alert channel is configured".into()]) },
                _ => ("PARTIAL", vec![format!("{} prompts flagged", metric(metrics, "suspicious_prompts").unwrap_or(0.0))], vec!["screening is heuristic and is not a defense by itself".into()]),
            };
            Evaluation { control, outcome, evidence, gaps }
        })
        .collect()
}
