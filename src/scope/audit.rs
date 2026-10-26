// Scope audit: the boundary compared with what agents attempted and what really changed, read
// from this machine's hash-chained event log (tool_call.decided before each action,
// tool_call.verified after it, tool_call.unsettled for calls never verified). It reports decisions
// by rule and Crane operation domain, enforcement outcomes after the effect (bypasses, detections,
// coverage gaps), files agents were allowed to change that nothing governs, writes outside the
// repository, and decisions made under rules that have since changed. It reads only; nothing in
// the log or the rules is changed, and an absent record is reported as absent, never as zero risk.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;
use serde_json::Value;

use super::{rule, Boundary};
use crate::telemetry::events::{verify_chain, Event, EventStore};
use crate::telemetry::model::Decision;

/** A key and how often it occurred
 * Fields
    - key: String - key
    - count: usize - occurrences
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Count {
    pub(crate) key: String,
    pub(crate) count: usize,
}

/** The audit of one repository on this machine
 * Fields
    - session: Option<String> - the session audited (None: every session)
    - since: Option<u64> - earliest event considered, Unix milliseconds (None: all)
    - chain: String - verified, empty, or broken (the event chain on this machine)
    - chain_problem: Option<String> - why the chain is broken
    - events: usize - events considered
    - sessions: usize - sessions seen
    - decisions: BTreeMap<String, usize> - pre-action decisions by kind
    - rules: Vec<Count> - rule codes of decisions that were not allowed
    - domains: BTreeMap<String, usize> - those rules by Crane operation domain
    - outcomes: BTreeMap<String, usize> - enforcement outcomes after the effect
    - assessments: BTreeMap<String, usize> - assessments after the effect
    - unsettled: usize - tool calls decided but never verified
    - ungoverned_changes: Vec<Count> - files agents were allowed to change that no zone, flow, or
      selection governs now
    - outside_repository: Vec<Count> - writes outside the repository
    - older_rules: usize - decisions made under an older signed generation than the current one
    - findings: Vec<String> - what deserves attention, in plain words
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct AuditReport {
    pub(crate) session: Option<String>,
    pub(crate) since: Option<u64>,
    pub(crate) chain: String,
    pub(crate) chain_problem: Option<String>,
    pub(crate) events: usize,
    pub(crate) sessions: usize,
    pub(crate) decisions: BTreeMap<String, usize>,
    pub(crate) rules: Vec<Count>,
    pub(crate) domains: BTreeMap<String, usize>,
    pub(crate) outcomes: BTreeMap<String, usize>,
    pub(crate) assessments: BTreeMap<String, usize>,
    pub(crate) unsettled: usize,
    pub(crate) ungoverned_changes: Vec<Count>,
    pub(crate) outside_repository: Vec<Count>,
    pub(crate) older_rules: usize,
    pub(crate) findings: Vec<String>,
}

/** The serialized name of an enumeration value
 * Input
    - value: &impl Serialize - value
 * Output
    - String
*/
fn name(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "UNKNOWN".into())
}

/** Sort counts, most frequent first
 * Input
    - counts: BTreeMap<String, usize> - counts
 * Output
    - Vec<Count>
*/
fn ranked(counts: BTreeMap<String, usize>) -> Vec<Count> {
    let mut ranked = counts
        .into_iter()
        .map(|(key, count)| Count { key, count })
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.count.cmp(&a.count).then(a.key.cmp(&b.key)));
    ranked
}

/** Report whether an event belongs to a session
 * Input
    - event: &Event - event
    - session: &str - Crane or provider session id
 * Output
    - bool
*/
fn in_session(event: &Event, session: &str) -> bool {
    event.scope.foxx_session_id.as_deref() == Some(session)
        || event.scope.provider_session_id.as_deref() == Some(session)
}

/** Audit the boundary against the recorded attempts and effects
 * Input
    - boundary: &Boundary - the boundary now
    - session: Option<&str> - one session, or every session
    - since: Option<u64> - earliest event, Unix milliseconds
 * Output
    - Result<AuditReport, String>
    - Error if the event log cannot be read
*/
pub(crate) fn audit(
    boundary: &Boundary,
    session: Option<&str>,
    since: Option<u64>,
) -> Result<AuditReport, String> {
    let all = EventStore::open(&boundary.runtime).read_all()?;
    let chain = verify_chain(&all);
    let events = all
        .iter()
        .filter(|event| session.is_none_or(|session| in_session(event, session)))
        .filter(|event| since.is_none_or(|since| event.occurred_at >= since))
        .collect::<Vec<_>>();
    let governed = |path: &str| {
        boundary.enforcement.limits.contains_key(path)
            || boundary
                .state
                .registry
                .selections
                .iter()
                .any(|record| record.current.span.path == path)
    };
    let current = boundary.state.generation();
    let mut report = AuditReport {
        session: session.map(str::to_string),
        since,
        chain: chain.status.clone(),
        chain_problem: chain.problem.clone(),
        events: events.len(),
        sessions: 0,
        decisions: BTreeMap::new(),
        rules: Vec::new(),
        domains: BTreeMap::new(),
        outcomes: BTreeMap::new(),
        assessments: BTreeMap::new(),
        unsettled: 0,
        ungoverned_changes: Vec::new(),
        outside_repository: Vec::new(),
        older_rules: 0,
        findings: Vec::new(),
    };
    let mut sessions = BTreeSet::new();
    let mut rules = BTreeMap::<String, usize>::new();
    let mut ungoverned = BTreeMap::<String, usize>::new();
    let mut outside = BTreeMap::<String, usize>::new();
    for event in &events {
        if let Some(id) = &event.scope.foxx_session_id {
            sessions.insert(id.clone());
        }
        match event.event_type.as_str() {
            "tool_call.decided" => {
                let decision = event.decision.unwrap_or(Decision::Error);
                *report.decisions.entry(name(&decision)).or_default() += 1;
                if event.bindings.registry_generation < current {
                    report.older_rules += 1;
                }
                if decision != Decision::Allow {
                    for code in event.payload["rules"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                    {
                        *rules.entry(code.to_string()).or_default() += 1;
                        *report.domains.entry(rule(code).0.to_string()).or_default() += 1;
                    }
                    continue;
                }
                for resource in event.resources.iter().filter(|resource| {
                    resource.kind == "file"
                        && matches!(resource.access.as_str(), "write" | "delete")
                }) {
                    if resource.classification.as_deref() == Some("external") {
                        *outside.entry(resource.id.clone()).or_default() += 1;
                    } else if !governed(&resource.id) {
                        *ungoverned.entry(resource.id.clone()).or_default() += 1;
                    }
                }
            }
            "tool_call.verified" => {
                *report.outcomes.entry(name(&event.enforcement)).or_default() += 1;
                if let Some(assessment) = event.assessment {
                    *report.assessments.entry(name(&assessment)).or_default() += 1;
                }
            }
            "tool_call.unsettled" => report.unsettled += 1,
            _ => {}
        }
    }
    report.sessions = sessions.len();
    report.rules = ranked(rules);
    report.ungoverned_changes = ranked(ungoverned);
    report.outside_repository = ranked(outside);
    let tool_calls = events
        .iter()
        .filter(|event| event.event_type.starts_with("tool_call."))
        .count();
    let findings = &mut report.findings;
    if tool_calls == 0 {
        findings.push("no hook activity is recorded on this machine for this selection: no agent ran here, or its hooks are not installed (see 'crane scope inspect'); absence of events is not evidence of safety".into());
    }
    if report.chain == "broken" {
        findings.push(format!(
            "the event chain on this machine is broken ({}); these counts cannot be trusted",
            report.chain_problem.clone().unwrap_or_default()
        ));
    }
    let outcome = |key: &str| report.outcomes.get(key).copied().unwrap_or(0);
    if outcome("BYPASS_CONFIRMED") > 0 {
        findings.push(format!("{} actions changed protected code without passing enforcement (bypass confirmed after the effect)", outcome("BYPASS_CONFIRMED")));
    }
    if outcome("DETECTED_AFTER_EFFECT") > 0 {
        findings.push(format!(
            "{} violations were detected only after the effect",
            outcome("DETECTED_AFTER_EFFECT")
        ));
    }
    if outcome("COVERAGE_GAP") > 0 {
        findings.push(format!(
            "{} actions could not be observed or decided (coverage gaps)",
            outcome("COVERAGE_GAP")
        ));
    }
    if report.unsettled > 0 {
        findings.push(format!("{} tool calls were decided but never verified (the agent tool did not run the post-tool hook)", report.unsettled));
    }
    if !report.ungoverned_changes.is_empty() {
        findings.push(format!("agents were allowed to change {} files that no zone, flow, or selection governs (first: {})", report.ungoverned_changes.len(), report.ungoverned_changes[0].key));
    }
    if !report.outside_repository.is_empty() {
        findings.push(format!(
            "agents wrote {} paths outside the repository, which Crane does not govern",
            report.outside_repository.len()
        ));
    }
    if report.older_rules > 0 {
        findings.push(format!(
            "{} decisions were made under rules that have since been re-signed",
            report.older_rules
        ));
    }
    Ok(report)
}
