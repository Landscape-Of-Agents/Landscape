// Specification events of flow knowledge (DASHBOARD.md F39). Flow commands, the hook's agent
// channel, and the watchers note what happened (platform::notes); each note becomes one F39 event
// here, attached to the event it rode on (the command, the agent's request, or the sync's
// flow.watchers_evaluated event), so it shares that event's digest, chain position, identity, and
// session. Only the registered kinds below become events; their fields are identifiers, digests,
// counts, states, and codes, never context text, test descriptions, or incident details.

use serde_json::Value;

use super::events::Event;
use super::spec::{base, Spec};

/** Every registered F39 note kind with its action type and statement */
pub(crate) const KINDS: &[(&str, &str, &str)] = &[
    (
        "FLOW_TRACED",
        "FLOW_TRACE",
        "A flow was traced from its starting point into a draft revision (grants nothing).",
    ),
    (
        "FLOW_REWOUND",
        "FLOW_TRACE",
        "An earlier revision of a trace draft was made current again.",
    ),
    (
        "FLOW_APPROVED",
        "FLOW_APPROVE",
        "A traced flow was approved and signed into the flow knowledge.",
    ),
    (
        "FLOW_DISCARDED",
        "FLOW_APPROVE",
        "A trace draft was discarded.",
    ),
    (
        "FLOW_CHANGED",
        "FLOW_KNOWLEDGE",
        "Signed flow knowledge changed (context, priority, people, tests, limits, or models).",
    ),
    (
        "FLOW_PROPOSED",
        "FLOW_PROPOSE",
        "An agent proposed a test or context item for a flow (waits for a person).",
    ),
    (
        "FLOW_PROPOSAL_DECIDED",
        "FLOW_PROPOSE",
        "A person approved or rejected a proposal.",
    ),
    (
        "FLOW_TESTS_RUN",
        "FLOW_TEST",
        "A flow's tests were run by a person or CI.",
    ),
    (
        "FLOW_WATCHER_STARTED",
        "FLOW_WATCH",
        "A read-only watcher started on a flow, within the signed limits.",
    ),
    ("FLOW_WATCHER_STOPPED", "FLOW_WATCH", "A watcher stopped."),
    (
        "FLOW_WATCHER_EDITED",
        "FLOW_WATCH",
        "A person changed a watcher's purpose or checks; it stays read-only.",
    ),
    (
        "FLOW_WATCHERS_EVALUATED",
        "FLOW_WATCH",
        "Flow watchers ran their read-only checks without a model.",
    ),
    (
        "FLOW_INCIDENT_RAISED",
        "FLOW_INCIDENT",
        "An incident was raised on a flow; its people and the parent session were told.",
    ),
    (
        "FLOW_AGENT_REQUEST",
        "FLOW_REQUEST",
        "An agent asked Crane about a flow through its hooks; nothing was run.",
    ),
];

/** Build the F39 event of a note
 * Input
    - event: &Event - the event the note rode on
    - kind: &str - note kind
    - note: &Value - note fields
 * Output
    - Option<Spec> (None for an unregistered kind)
*/
pub(super) fn spec(event: &Event, kind: &str, note: &Value) -> Option<Spec> {
    let (_, action, statement) = KINDS.iter().find(|(name, _, _)| *name == kind)?;
    let result = note
        .get("result")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase);
    let failing = matches!(
        result.as_deref(),
        Some("FAILED" | "FIRING" | "REFUSED" | "REJECTED")
    );
    let mut spec = base(event, kind, "F39");
    spec.set("action", "action_type", *action)
        .set("resource", "resource", ["CRANE_METADATA"])
        .set(
            "resource",
            "resource_ids",
            ["flow", "watcher", "proposal", "incident"]
                .iter()
                .filter_map(|key| note.get(*key).and_then(Value::as_str).map(String::from))
                .collect::<Vec<_>>(),
        )
        .respond(
            result.as_deref().unwrap_or("OK"),
            if failing {
                result.as_deref().unwrap_or("NA")
            } else {
                "NA"
            },
            statement,
        )
        .severity(
            if kind == "FLOW_INCIDENT_RAISED" {
                "MEDIUM"
            } else if failing {
                "LOW"
            } else {
                "INFO"
            },
            "OUTCOME",
            "NONE",
        );
    for key in [
        "flow",
        "revision",
        "nodes",
        "status",
        "via",
        "digest",
        "added",
        "kind",
        "proposal",
        "decision",
        "watcher",
        "watchers",
        "by",
        "checks",
        "incident",
        "incidents",
        "code",
        "notified",
        "request",
        "change",
        "tests",
        "passed",
        "failed",
        "required",
    ] {
        if let Some(value) = note.get(key) {
            spec.set("other", key, value);
        }
    }
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::KINDS;

    /** Every flow note the code writes is registered, and every registered kind is documented in
     * DASHBOARD.md (F39)
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn flow_notes_are_registered_and_documented() {
        let sources = [
            include_str!("../flow/agent.rs"),
            include_str!("../flow/watch.rs"),
            include_str!("../cli/flow.rs"),
            include_str!("../hooks/runtime/tools.rs"),
        ]
        .concat();
        let mut used = sources
            .split('"')
            .filter(|word| {
                word.starts_with("FLOW_")
                    && word.len() > 5
                    && word.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            })
            .filter(|word| *word != "FLOW_MODEL_ROUTING")
            .collect::<Vec<_>>();
        used.sort_unstable();
        used.dedup();
        assert!(used.len() >= 10, "{used:?}");
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
                "{kind} is not documented in DASHBOARD.md F39"
            );
        }
    }
}
