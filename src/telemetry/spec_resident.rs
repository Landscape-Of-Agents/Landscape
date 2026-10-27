// Specification events of resident agents (DASHBOARD.md F38). Resident commands and the worker
// note what happened (platform::notes); each note becomes one F38 event here, attached to the event
// it rode on (the command's event, or the background sync's `resident.tick` event), so it shares
// that event's digest, chain position, identity, and session. Only the registered kinds below
// become events; their fields are identifiers, digests, counts, states, and reason codes, never
// context text, answers, or messages. Correlation ids (assignment, activation, trigger, step) are
// carried on every activity event.

use serde_json::Value;

use super::events::Event;
use super::spec::{base, Spec};

/** Every registered F38 note kind with its action type and statement */
pub(crate) const KINDS: &[(&str, &str, &str)] = &[
    (
        "RESIDENT_DEFINITION_SAVED",
        "RESIDENT_DEFINE",
        "Resident agent definition saved as a draft (grants nothing).",
    ),
    (
        "RESIDENT_DEFINITION_ACTIVATED",
        "RESIDENT_DEFINE",
        "Resident agent definition version activated.",
    ),
    (
        "RESIDENT_DEFINITION_REVOKED",
        "RESIDENT_DEFINE",
        "Resident agent definition version revoked.",
    ),
    (
        "RESIDENT_CREATED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment created as a draft.",
    ),
    (
        "RESIDENT_VALIDATED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment validated against the signed state.",
    ),
    (
        "RESIDENT_ACTIVATED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment activated; idle until a trigger arrives.",
    ),
    (
        "RESIDENT_PAUSED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment paused.",
    ),
    (
        "RESIDENT_RESUMED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment resumed.",
    ),
    (
        "RESIDENT_RENEWED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment renewed for a new period.",
    ),
    (
        "RESIDENT_EXPIRED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment lifetime ended.",
    ),
    (
        "RESIDENT_REVOKED",
        "RESIDENT_LIFECYCLE",
        "Resident assignment revoked.",
    ),
    (
        "RESIDENT_TRIGGER_RECEIVED",
        "RESIDENT_TRIGGER",
        "Trigger received and normalized.",
    ),
    (
        "RESIDENT_TRIGGER_IGNORED",
        "RESIDENT_TRIGGER",
        "Trigger woke nothing (duplicate, no match, cooldown, limit, paused).",
    ),
    (
        "RESIDENT_ACTIVATION_QUEUED",
        "RESIDENT_ACTIVATION",
        "Activation queued for a matching assignment.",
    ),
    (
        "RESIDENT_ACTIVATION_STARTED",
        "RESIDENT_ACTIVATION",
        "Activation started by the worker.",
    ),
    (
        "RESIDENT_CONTEXT_READY",
        "RESIDENT_CONTEXT",
        "Activation context compiled.",
    ),
    (
        "RESIDENT_CONTEXT_CACHE_HIT",
        "RESIDENT_CONTEXT",
        "Context pack unchanged since the last activation.",
    ),
    (
        "RESIDENT_CONTEXT_CACHE_MISS",
        "RESIDENT_CONTEXT",
        "No context pack was kept; compiled from scratch.",
    ),
    (
        "RESIDENT_CONTEXT_CACHE_INVALIDATED",
        "RESIDENT_CONTEXT",
        "Context pack rebuilt because a dependency changed.",
    ),
    (
        "RESIDENT_RCA_SAVED",
        "RESIDENT_RCA",
        "RCA graph version saved as proposed.",
    ),
    (
        "RESIDENT_RCA_PROPOSED",
        "RESIDENT_RCA",
        "Investigation produced a proposal or question instead of a known remediation.",
    ),
    (
        "RESIDENT_RCA_REVIEWED",
        "RESIDENT_RCA",
        "RCA graph version reviewed by a person.",
    ),
    (
        "RESIDENT_PLAN_CREATED",
        "RESIDENT_PLAN",
        "Typed action plan created or extended.",
    ),
    (
        "RESIDENT_APPROVAL_REQUIRED",
        "RESIDENT_APPROVAL",
        "A step waits for a person's approval bound to its digest.",
    ),
    (
        "RESIDENT_APPROVAL_DECIDED",
        "RESIDENT_APPROVAL",
        "A person approved, denied, or answered a waiting step.",
    ),
    (
        "RESIDENT_ACTION_STARTED",
        "RESIDENT_ACTION",
        "A step started after Crane allowed it.",
    ),
    (
        "RESIDENT_ACTION_DENIED",
        "RESIDENT_ACTION",
        "Crane denied a step.",
    ),
    (
        "RESIDENT_ACTION_RESULT",
        "RESIDENT_ACTION",
        "A step's external or declared result.",
    ),
    (
        "RESIDENT_VERIFICATION_RESULT",
        "RESIDENT_VERIFY",
        "A verification step evaluated its postcondition.",
    ),
    (
        "RESIDENT_RECONCILIATION_REQUIRED",
        "RESIDENT_VERIFY",
        "An external effect is uncertain; a person must reconcile it.",
    ),
    (
        "RESIDENT_ACTIVATION_COMPLETED",
        "RESIDENT_ACTIVATION",
        "Activation ended with an evidence-based outcome.",
    ),
    (
        "RESIDENT_TESTED",
        "RESIDENT_TEST",
        "Assignment tested in plan-only mode (nothing executed).",
    ),
];

/** The F38 event of a resident note
 * Input
    - event: &Event - the event the note rode on
    - kind: &str - note kind
    - note: &Value - note fields
 * Output
    - Option<Spec> - None for a kind that is not registered
*/
pub(super) fn spec(event: &Event, kind: &str, note: &Value) -> Option<Spec> {
    let (_, action, statement) = KINDS.iter().find(|(name, _, _)| *name == kind)?;
    let result = note
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase);
    let failing = matches!(
        result.as_deref(),
        Some("FAILED" | "BLOCKED" | "CONTRADICTED" | "UNKNOWN" | "ERROR")
    ) || kind == "RESIDENT_ACTION_DENIED";
    let attention = failing
        || matches!(
            kind,
            "RESIDENT_RECONCILIATION_REQUIRED"
                | "RESIDENT_APPROVAL_REQUIRED"
                | "RESIDENT_REVOKED"
                | "RESIDENT_EXPIRED"
        );
    let ids = [
        "assignment",
        "activation",
        "trigger",
        "definition",
        "rca",
        "step",
    ]
    .iter()
    .filter_map(|key| note.get(*key).and_then(Value::as_str).map(String::from))
    .collect::<Vec<_>>();
    let mut spec = base(event, kind, "F38");
    spec.set("action", "action_type", *action)
        .set("resource", "resource", ["CRANE_METADATA"])
        .set("resource", "resource_ids", ids)
        .respond(
            result.as_deref().unwrap_or("OK"),
            if failing {
                result.as_deref().unwrap_or("DENIED")
            } else {
                "NA"
            },
            statement,
        )
        .severity(
            if failing {
                "MEDIUM"
            } else if attention {
                "LOW"
            } else {
                "INFO"
            },
            "OUTCOME",
            "NONE",
        );
    for key in [
        "assignment",
        "activation",
        "trigger",
        "step",
        "step_type",
        "definition",
        "rca",
        "target",
        "agent",
        "state",
        "from",
        "mode",
        "source",
        "kind",
        "targets",
        "origin",
        "plan",
        "steps",
        "conclusion",
        "digest",
        "decision",
        "reasons",
        "approver",
        "basis",
        "pack",
        "refresh_reason",
        "missing",
        "problems",
        "warnings",
        "gaps",
        "model_calls",
        "external_calls",
        "self_reviewed",
        "expires_at",
        "schema_version",
    ] {
        if let Some(value) = note.get(key) {
            spec.set("other", key, value);
        }
    }
    match kind {
        "RESIDENT_CONTEXT_CACHE_HIT" => {
            spec.set("eval", "context_cache", "HIT");
        }
        "RESIDENT_CONTEXT_CACHE_MISS" | "RESIDENT_CONTEXT_CACHE_INVALIDATED" => {
            spec.set("eval", "context_cache", "MISS");
        }
        _ => {}
    }
    if let Some(calls) = note.get("model_calls") {
        spec.set("eval", "model_calls", calls);
    }
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::KINDS;

    /** Every resident note the code writes is registered, and every registered kind is documented
     * in DASHBOARD.md (F38)
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn resident_notes_are_registered_and_documented() {
        let sources = [
            include_str!("../resident/registry.rs"),
            include_str!("../resident/worker.rs"),
            include_str!("../resident/store.rs"),
            include_str!("../cli/resident/registry.rs"),
            include_str!("../cli/resident/activity.rs"),
        ]
        .concat();
        let mut used = sources
            .split('"')
            .filter(|word| {
                word.starts_with("RESIDENT_")
                    && word.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            })
            .collect::<Vec<_>>();
        used.sort_unstable();
        used.dedup();
        assert!(used.len() >= 30, "{used:?}");
        for kind in used {
            assert!(
                KINDS.iter().any(|(name, _, _)| *name == kind),
                "{kind} is noted but not registered"
            );
        }
        let documented = include_str!("../../DASHBOARD.md");
        for (kind, _, _) in KINDS {
            assert!(
                documented.contains(&format!("`{kind}`")),
                "{kind} is not documented in DASHBOARD.md F38"
            );
        }
    }
}
