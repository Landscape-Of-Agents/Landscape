// Downloadable evidence: the session report (outcome, timeline, graph, effects, autonomy,
// verification, coverage, integrity, remediation, limitations) and the evidence bundle with the
// SHA-512 digest of every part.

use super::*;
use crate::foxx::views::session_view;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::server::dashboard::data::Context;
use crate::telemetry::model::Enforcement;
use crate::trust::crypto::{canonical_json, digest};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/** Build a session report: summary and outcome, timeline, action graph, resource effects,
 * decisions and violations, autonomy accounting, verification results, coverage, evidence and
 * integrity, remediation, and the telemetry's limitations
 * Input
    - context: &Context - repository data
    - id: &str - canonical session
 * Output
    - Value
*/
pub(super) fn session_report(context: &Context, id: &str) -> Value {
    let snapshot = &context.snapshot;
    let Some(session) = snapshot.session(id) else {
        return json!({"error": "no such session"});
    };
    let events = snapshot.session_events(id);
    let view = session_view(snapshot, session);
    let metric = |name: &str| view["metrics"][name]["value"].as_f64().unwrap_or(0.0);
    let violations = events
        .iter()
        .filter(|event| {
            matches!(
                event.enforcement,
                Enforcement::BypassConfirmed | Enforcement::DetectedAfterEffect
            )
        })
        .map(|event| compact(context, event))
        .collect::<Vec<_>>();
    let gaps = events
        .iter()
        .filter(|event| event.enforcement == Enforcement::CoverageGap)
        .map(|event| compact(context, event))
        .collect::<Vec<_>>();
    let outcome = if metric("bypasses_confirmed") > 0.0 {
        "BYPASS_CONFIRMED"
    } else if !violations.is_empty() {
        "VIOLATION"
    } else if !gaps.is_empty() {
        "INCOMPLETE_EVIDENCE"
    } else if metric("tasks_failed_verification") > 0.0 && metric("tasks_verified") == 0.0 {
        "NOT_VERIFIED"
    } else if metric("tasks_verified") > 0.0 {
        "COMPLIANT"
    } else {
        "NO_VERIFICATION_RECORDED"
    };
    let mut remediation = Vec::new();
    if metric("bypasses_confirmed") > 0.0 {
        remediation.push("Review the quarantined session's shell commands, restore the protected code from the checkpoint, and start a new session.");
    }
    if !violations.is_empty() {
        remediation.push("Run 'crane test .' and restore every failing preserved selection.");
    }
    if !gaps.is_empty() {
        remediation.push("Validate the hooks with 'crane agent hooks --profile ...'; actions without decisions are unobserved.");
    }
    if metric("bypass_attempts") > 0.0 {
        remediation.push("Review the bypass attempts; repeated attempts quarantine the session.");
    }
    let mut effects: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for event in &events {
        for resource in &event.resources {
            effects
                .entry(resource.access.clone())
                .or_default()
                .insert(resource.id.clone());
        }
    }
    let member = context.session_member.get(id);
    let chain = context
        .chains
        .iter()
        .find(|chain| member.is_some_and(|member| chain["member"] == member.as_str()))
        .cloned()
        .unwrap_or(Value::Null);
    json!({
        "report": "session",
        "generated_at": iso_time(now_millis()),
        "repository_id": context.repository,
        "remote": context.remote,
        "member": context.member(member),
        "summary": {
            "outcome": outcome,
            "session": view["session"],
            "credits": view["credits"],
            "decisions": {"allowed": metric("actions_allowed"), "denied": metric("actions_denied"), "approval": metric("actions_requiring_approval")},
            "bypass_attempts": metric("bypass_attempts"),
            "bypasses_confirmed": metric("bypasses_confirmed"),
            "violations_after_effect": metric("violations_after_effect"),
        },
        "timeline": events.iter().map(|event| compact(context, event)).collect::<Vec<_>>(),
        "action_graph": graph(context, &events),
        "resource_effects": effects,
        "violations": violations,
        "autonomy": snapshot.ledger.iter().filter(|entry| entry.session == id).collect::<Vec<_>>(),
        "verification": events.iter().filter(|event| event.event_type.starts_with("task.verif")).map(|event| compact(context, event)).collect::<Vec<_>>(),
        "coverage": {
            "gaps": gaps,
            "evidence_completeness": view["metrics"]["evidence_completeness"],
        },
        "metrics": view["metrics"],
        "integrity": {
            "event_chain": chain["event_chain"],
            "ledger_reconciles": chain["ledger_reconciles"],
            "attestation": events.iter().rev().find(|event| event.event_type == "session.attested").map(|event| event.payload["attestation"].clone()),
            "registry_generations": events.iter().map(|event| event.bindings.registry_generation).collect::<BTreeSet<_>>(),
        },
        "evidence_references": events.iter().flat_map(|event| event.evidence.clone()).collect::<Vec<_>>(),
        "remediation": remediation,
        "limitations": [
            "Facts marked INFERRED (network destinations, generated identifiers) were parsed, not observed.",
            "Network bytes, CPU, memory, and model tokens are UNSUPPORTED by hook instrumentation.",
            "Changes made outside agent hooks (other terminals, editors) are detected only when Crane next verifies.",
            "An agent with an unrestricted shell running as the same user can bypass local checks; detection after the effect is then the control.",
        ],
    })
}

/** Build a downloadable evidence bundle for a session: the report, the session's events and
 * ledger entries, and a manifest with the SHA-512 of every part and of the bundle
 * Input
    - context: &Context - repository data
    - id: &str - canonical session
 * Output
    - Value
*/
pub(super) fn evidence_bundle(context: &Context, id: &str) -> Value {
    let snapshot = &context.snapshot;
    let report = session_report(context, id);
    let events = snapshot
        .session_events(id)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let ledger = snapshot
        .ledger
        .iter()
        .filter(|entry| entry.session == id)
        .cloned()
        .collect::<Vec<_>>();
    let parts = [
        ("report", json!(report)),
        ("events", json!(events)),
        ("ledger", json!(ledger)),
    ];
    let files = parts
        .iter()
        .map(|(name, value)| json!({"name": name, "sha512": digest("evidence/part/v1", &[canonical_json(value).as_bytes()])}))
        .collect::<Vec<_>>();
    let manifest = json!({
        "format": 1,
        "created_at": iso_time(now_millis()),
        "repository_id": context.repository,
        "session": id,
        "parts": files,
        "verification": "recompute SHA-512 (domain evidence/part/v1) over the canonical JSON of each part; each event digest is SHA-512 over its canonical JSON and chains through previous_digest",
    });
    let bundle_digest = digest(
        "evidence/bundle/v1",
        &[canonical_json(&manifest).as_bytes()],
    );
    json!({
        "manifest": manifest,
        "bundle_digest": bundle_digest,
        "report": parts[0].1,
        "events": parts[1].1,
        "ledger": parts[2].1,
    })
}
