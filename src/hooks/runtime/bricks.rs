// Brick context in the hook runtime. Before a tool call that touches Flows or Zones with attached
// Bricks, the agent receives the snapshot of those targets (brick::context), once per session while
// it is unchanged; afterwards a one-line reference stands in for it. The restricting controls of
// the Bricks are not decided here: the guard already applied them through the per-file limits.
// Identity Bricks are checked against the acting provider and reported to the agent, because the
// guard does not decide per agent yet. Every delivery is noted for the tool call's event (F37).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::Hook;
use crate::brick::context::{assemble, deliver, index, remember, render, Delivery};
use crate::brick::runtime::identity_admits;
use crate::brick::store::BrickStore;
use crate::telemetry::identity::SessionRecord;

/** What the agent receives about Bricks for one tool call
 * Fields
    - text: String - the snapshot, or a reference to it when unchanged
    - delivered: bool - the full snapshot was sent (show it in the agent's window too)
    - live: Value - the editor's live context entry
*/
pub(super) struct BrickGuidance {
    pub(super) text: String,
    pub(super) delivered: bool,
    pub(super) live: Value,
}

impl Hook {
    /** Assemble and deliver the Brick snapshot of the targets a tool call touches
     * Input
        - session: &SessionRecord - session
        - entities: &BTreeSet<String> - "zone:ID" and "flow:ID" covering the touched files
        - files: &BTreeSet<String> - the touched files, for the editor's highlights
     * Output
        - Option<BrickGuidance> - None when no attached Brick applies
    */
    pub(super) fn brick_guidance(
        &self,
        session: &SessionRecord,
        entities: &BTreeSet<String>,
        files: &BTreeSet<String>,
    ) -> Option<BrickGuidance> {
        self.state.bricks.as_ref()?;
        let store = BrickStore::of(&self.state).ok()?;
        let attached = index(&store);
        let targets = entities
            .iter()
            .filter(|entity| attached.contains_key(*entity))
            .cloned()
            .collect::<BTreeSet<_>>();
        if targets.is_empty() {
            return None;
        }
        let snapshot = assemble(&self.workspace, &self.state, &store, None, &targets);
        if snapshot.sections.is_empty() && snapshot.conflicts.is_empty() {
            return None;
        }
        let refreshed = remember(&self.stores.directory, &snapshot);
        let (full, truncated) = render(&snapshot);
        let delivery = deliver(
            &self.stores.directory,
            &session.foxx_session_id,
            &snapshot,
            full.len(),
        );
        let short = &snapshot.digest[..12];
        let mut text = match delivery {
            Delivery::Delivered => full.clone(),
            Delivery::Reused => format!(
                "Crane Bricks for {}: unchanged since they were delivered earlier in this session (snapshot {short}); they still apply.",
                snapshot.targets.join(", ")
            ),
        };
        // Identity is enforced by the guard (brick::guard); a call that reaches this point was only
        // held for a person (on_other: ask), so say why to the agent rather than stay silent
        for target in &targets {
            let admitted = identity_admits(&store, target, &session.provider)
                .filter(|(admitted, _)| !admitted)
                .or_else(|| {
                    identity_admits(&store, target, &session.agent_id)
                        .filter(|(admitted, _)| !admitted)
                });
            if let Some((_, deciding)) = admitted {
                text.push_str(&format!(
                    "\nIdentity: {} is not admitted in {target} by {}; its calls there need a person, so leave the work to an admitted agent.",
                    session.provider,
                    deciding.join(", ")
                ));
                crate::platform::notes::note(
                    "BRICK_IDENTITY_MISMATCH",
                    json!({"target": target, "agent": session.provider, "bricks": deciding}),
                );
            }
        }
        crate::platform::notes::note(
            if delivery == Delivery::Delivered {
                "BRICK_CONTEXT_DELIVERED"
            } else {
                "BRICK_CONTEXT_REUSED"
            },
            json!({
                "targets": snapshot.targets,
                "snapshot": snapshot.digest,
                "bytes": text.len(),
                "snapshot_bytes": full.len(),
                "bricks": snapshot.bricks,
                "refresh_reason": refreshed,
                "truncated": truncated,
                "conflicts": snapshot.conflicts.len(),
                "gaps": snapshot.gaps.len(),
            }),
        );
        let live = json!({
            "entity": format!("bricks:{short}"),
            "kind": "brick",
            "id": short,
            "name": format!("Bricks of {}", snapshot.targets.join(", ")),
            "text": full,
            "locations": files.iter().map(|path| json!({"path": path})).collect::<Vec<_>>(),
        });
        Some(BrickGuidance {
            text,
            delivered: delivery == Delivery::Delivered,
            live,
        })
    }
}
