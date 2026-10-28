// Specification events of Bricks (DASHBOARD.md F37). Brick commands and the hook runtime note what
// happened (platform::notes); each note becomes one F37 event here, attached to the event it rode
// on (the command, the tool call, or the sync's monitor run), so it shares that event's digest,
// chain position, identity, and session. Only the registered kinds below become events; their
// fields are identifiers, digests, counts, and states, never context text.

use serde_json::Value;

use super::events::Event;
use super::spec::{base, Spec};

/** Every registered F37 note kind with its action type and statement */
pub(crate) const KINDS: &[(&str, &str, &str)] = &[
    (
        "BRICK_DRAFT_SAVED",
        "BRICK_DRAFT",
        "Brick draft saved (grants nothing).",
    ),
    ("BRICK_VALIDATED", "BRICK_VALIDATE", "Brick validated."),
    ("BRICK_TESTED", "BRICK_TEST", "Brick tested."),
    (
        "BRICK_ACTIVATED",
        "BRICK_ACTIVATE",
        "Brick version activated and signed.",
    ),
    (
        "BRICK_DEPRECATED",
        "BRICK_DEPRECATE",
        "Brick version deprecated.",
    ),
    (
        "BRICK_REVOKED",
        "BRICK_REVOKE",
        "Brick version revoked; pins to it fail closed.",
    ),
    (
        "BRICK_STACK_SAVED",
        "BRICK_STACK_DRAFT",
        "Brick Stack draft saved.",
    ),
    (
        "BRICK_STACK_RESOLVED",
        "BRICK_STACK_RESOLVE",
        "Brick Stack resolved.",
    ),
    (
        "BRICK_STACK_ACTIVATED",
        "BRICK_STACK_ACTIVATE",
        "Brick Stack version activated and signed.",
    ),
    (
        "BRICK_STACK_REVOKED",
        "BRICK_STACK_REVOKE",
        "Brick Stack version revoked.",
    ),
    (
        "BRICK_ATTACHED",
        "BRICK_ATTACH",
        "Brick attached to a Flow or Zone.",
    ),
    (
        "BRICK_DETACHED",
        "BRICK_DETACH",
        "Brick detached; history kept.",
    ),
    (
        "BRICK_UPGRADED",
        "BRICK_UPGRADE",
        "Attachment pinned to another version.",
    ),
    (
        "BRICK_CONTEXT_UPDATED",
        "BRICK_CONTEXT",
        "Current Context of one attachment updated.",
    ),
    (
        "BRICK_CONTEXT_PREVIEWED",
        "BRICK_CONTEXT",
        "Effective context previewed.",
    ),
    (
        "BRICK_CONTEXT_DELIVERED",
        "SUPPLY_CONTEXT",
        "Brick context delivered before the action.",
    ),
    (
        "BRICK_CONTEXT_REUSED",
        "SUPPLY_CONTEXT",
        "Brick context unchanged; a reference was sent.",
    ),
    (
        "BRICK_IDENTITY_MISMATCH",
        "BRICK_IDENTITY",
        "Agent not admitted by an identity Brick; its calls there are refused or need a person.",
    ),
    (
        "BRICK_ASSIGNED",
        "BRICK_ASSIGN",
        "Agent assigned to Flows or Zones.",
    ),
    (
        "BRICK_UNASSIGNED",
        "BRICK_ASSIGN",
        "Agent assignment revoked.",
    ),
    (
        "BRICK_MONITOR_EVALUATED",
        "BRICK_MONITOR",
        "Brick monitor evaluated without a model.",
    ),
    (
        "BRICK_WATCH_REACTED",
        "BRICK_MONITOR",
        "A watch signal started firing and its reaction ran.",
    ),
    (
        "BRICK_MIGRATION_DRAFTED",
        "BRICK_MIGRATE",
        "Bricks of former kinds rewritten as drafts of current kinds.",
    ),
];

/** The F37 event of a Brick note
 * Input
    - event: &Event - the event the note rode on
    - kind: &str - note kind
    - note: &Value - note fields
 * Output
    - Option<Spec> - None for a kind that is not registered
*/
pub(super) fn spec(event: &Event, kind: &str, note: &Value) -> Option<Spec> {
    let (_, action, statement) = KINDS.iter().find(|(name, _, _)| *name == kind)?;
    let text = |key: &str| note.get(key).cloned().unwrap_or(Value::Null);
    let result = note
        .get("result")
        .or_else(|| note.get("validation"))
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase);
    let failing = matches!(
        result.as_deref(),
        Some("FAILED" | "FAILING" | "BLOCKED" | "ERROR" | "REVOCATION_DUE" | "FIRING")
    );
    let attention = failing
        || matches!(
            kind,
            "BRICK_REVOKED" | "BRICK_IDENTITY_MISMATCH" | "BRICK_DETACHED"
        )
        || note["conflicts"].as_u64().is_some_and(|count| count > 0);
    let mut spec = base(event, kind, "F37");
    spec.set("action", "action_type", *action)
        .set("resource", "resource", ["CRANE_METADATA"])
        .set(
            "resource",
            "resource_ids",
            [text("brick"), text("stack"), text("attachment")]
                .into_iter()
                .filter_map(|value| value.as_str().map(String::from))
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
        "brick",
        "stack",
        "attachment",
        "target",
        "targets",
        "type",
        "digest",
        "snapshot",
        "bytes",
        "snapshot_bytes",
        "bricks",
        "refresh_reason",
        "truncated",
        "conflicts",
        "gaps",
        "agent",
        "state",
        "monitor",
        "source",
        "inference",
        "self_reviewed",
        "pinned_attachments",
        "context_bytes",
        "from",
        "members",
        "problems",
        "cases",
    ] {
        if let Some(value) = note.get(key) {
            spec.set("other", key, value);
        }
    }
    if kind == "BRICK_CONTEXT_REUSED" {
        spec.set("eval", "context_cache", "HIT");
    } else if kind == "BRICK_CONTEXT_DELIVERED" {
        spec.set("eval", "context_cache", "MISS");
    }
    Some(spec)
}

#[cfg(test)]
mod tests {
    use super::KINDS;

    /** Every Brick note the code writes is registered, and every registered kind is documented in
     * DASHBOARD.md (F37)
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn brick_notes_are_registered_and_documented() {
        let sources = [
            include_str!("../cli/brick/draft.rs"),
            include_str!("../cli/brick/check.rs"),
            include_str!("../cli/brick/lifecycle.rs"),
            include_str!("../cli/brick/stack.rs"),
            include_str!("../cli/brick/attach.rs"),
            include_str!("../cli/brick/context.rs"),
            include_str!("../cli/brick/agents.rs"),
            include_str!("../hooks/runtime/bricks.rs"),
            include_str!("../brick/runtime.rs"),
        ]
        .concat();
        let mut used = sources
            .split('"')
            .filter(|word| {
                word.starts_with("BRICK_")
                    && word.chars().all(|c| c.is_ascii_uppercase() || c == '_')
            })
            .collect::<Vec<_>>();
        used.sort_unstable();
        used.dedup();
        assert!(used.len() >= 15, "{used:?}");
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
                "{kind} is not documented in DASHBOARD.md F37"
            );
        }
    }
}
