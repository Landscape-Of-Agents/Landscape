// Downloadable evidence: the session report (outcome, timeline, graph, effects, autonomy,
// verification, coverage, integrity, remediation, limitations), built from the session's events
// (DASHBOARD.md) and their evals, and the evidence bundle with the SHA-512 digest of every part.

use super::*;
use crate::foxx::views::session_view_with;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::server::dashboard::data::Context;
use crate::telemetry::evals::field;
use crate::trust::crypto::{canonical_json, digest};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/** Build a session report: summary and outcome, timeline, action graph, resource effects,
 * decisions and violations, autonomy accounting, verification results, coverage, evidence and
 * integrity, remediation, and the telemetry's limitations
 * Input
    - context: &Context - repository data
    - id: &str - canonical session
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value
*/
pub(super) fn session_report(context: &Context, id: &str, kind: &str) -> Value {
    let snapshot = &context.snapshot;
    let Some(session) = snapshot.session(id) else {
        return json!({"error": "no such session"});
    };
    let specs = of_session(context, id);
    let view = session_view_with(snapshot, session, &context.specs);
    let evals = &view["evals"];
    let count = |name: &str| evals[name].as_f64().unwrap_or(0.0);
    let pick = |wanted: &dyn Fn(&Value) -> bool| {
        specs
            .iter()
            .filter(|spec| wanted(spec))
            .map(|spec| served(context, spec, kind))
            .collect::<Vec<_>>()
    };
    let violations = pick(&|spec| {
        matches!(
            spec["event_name"].as_str(),
            Some("BYPASS_CONFIRMED" | "VIOLATION_AFTER_EFFECT")
        )
    });
    let gaps = pick(&|spec| {
        field(spec, "decision", "enforcement") == "COVERAGE_GAP" || spec["flow"] == "F11"
    });
    let outcome = if count("bypasses_confirmed") > 0.0 {
        "BYPASS_CONFIRMED"
    } else if !violations.is_empty() {
        "VIOLATION"
    } else if !gaps.is_empty() {
        "INCOMPLETE_EVIDENCE"
    } else if count("tasks_failed") > 0.0 && count("tasks_verified") == 0.0 {
        "NOT_VERIFIED"
    } else if count("tasks_verified") > 0.0 {
        "COMPLIANT"
    } else {
        "NO_VERIFICATION_RECORDED"
    };
    let mut remediation = Vec::new();
    if count("bypasses_confirmed") > 0.0 {
        remediation.push("Review the quarantined session's shell commands, restore the protected code from the checkpoint, and start a new session.");
    }
    if !violations.is_empty() {
        remediation.push("Run 'crane test .' and restore every failing preserved selection.");
    }
    if !gaps.is_empty() {
        remediation.push("Validate the hooks with 'crane agent hooks --profile ...'; actions without decisions are unobserved.");
    }
    if count("bypass_attempts") > 0.0 {
        remediation.push("Review the bypass attempts; repeated attempts quarantine the session.");
    }
    let mut effects: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for spec in &specs {
        let kinds = spec["resource"]["resource"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let ids = spec["resource"]["resource_ids"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for kind in kinds
            .iter()
            .filter_map(Value::as_str)
            .filter(|kind| *kind != "NOT_CONSUMED")
        {
            let entry = effects.entry(kind.to_string()).or_default();
            entry.extend(ids.iter().filter_map(Value::as_str).map(String::from));
        }
    }
    let member = context.session_member.get(id);
    let chain = context
        .chains
        .iter()
        .find(|chain| member.is_some_and(|member| chain["member"] == member.as_str()))
        .cloned()
        .unwrap_or(Value::Null);
    let attestation = specs
        .iter()
        .rev()
        .find(|spec| spec["event_name"] == "SESSION_ATTESTED")
        .map(|spec| served(context, spec, kind));
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
            "decisions": {"allowed": evals["allowed"], "denied": evals["denied"], "approval": evals["approvals"]},
            "bypass_attempts": evals["bypass_attempts"],
            "bypasses_confirmed": evals["bypasses_confirmed"],
            "violations_after_effect": evals["violations_after_effect"],
        },
        "timeline": specs.iter().map(|spec| served(context, spec, kind)).collect::<Vec<_>>(),
        "action_graph": graph(context, id, kind),
        "resource_effects": effects,
        "violations": violations,
        "autonomy": pick(&|spec| spec["flow"] == "F27"),
        "verification": pick(&|spec| spec["flow"] == "F07"),
        "coverage": {
            "gaps": gaps,
            "evidence_completeness": evals["evidence_completeness"],
        },
        "evals": evals,
        "integrity": {
            "event_chain": chain["event_chain"],
            "ledger_reconciles": chain["ledger_reconciles"],
            "attestation": attestation,
            "registry_generations": specs.iter().filter_map(|spec| spec["bindings"]["registry_generation"].as_u64()).collect::<BTreeSet<_>>(),
        },
        "evidence_references": snapshot.session_events(id).iter().flat_map(|event| event.evidence.clone()).collect::<Vec<_>>(),
        "remediation": remediation,
        "limitations": [
            "Facts marked INFERRED (network destinations, generated identifiers) were parsed, not observed.",
            "Network bytes, CPU, memory, and model tokens are UNSUPPORTED by hook instrumentation.",
            "Changes made outside agent hooks (other terminals, editors) are detected only when Crane next verifies.",
            "An agent with an unrestricted shell running as the same user can bypass local checks; detection after the effect is then the control.",
            "Response statements are written by Crane from the outcome; hooks do not carry the agent's own words.",
        ],
    })
}

/** Build a downloadable evidence bundle for a session: the report, the session's stored events,
 * its specification events, and ledger entries, and a manifest with the SHA-512 of every part and
 * of the bundle
 * Input
    - context: &Context - repository data
    - id: &str - canonical session
    - kind: &str - PERSONAL or ORGANIZATION
 * Output
    - Value
*/
pub(super) fn evidence_bundle(context: &Context, id: &str, kind: &str) -> Value {
    let snapshot = &context.snapshot;
    let report = session_report(context, id, kind);
    let events = snapshot
        .session_events(id)
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    let specs = of_session(context, id)
        .into_iter()
        .map(|spec| served(context, spec, kind))
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
        ("spec_events", json!(specs)),
        ("ledger", json!(ledger)),
    ];
    let files = parts
        .iter()
        .map(|(name, value)| json!({"name": name, "sha512": digest("evidence/part/v1", &[canonical_json(value).as_bytes()])}))
        .collect::<Vec<_>>();
    let manifest = json!({
        "format": 2,
        "created_at": iso_time(now_millis()),
        "repository_id": context.repository,
        "session": id,
        "parts": files,
        "verification": "recompute SHA-512 (domain evidence/part/v1) over the canonical JSON of each part; each stored event digest is SHA-512 over its canonical JSON (including its specification events) and chains through previous_digest",
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
        "spec_events": parts[2].1,
        "ledger": parts[3].1,
    })
}
