// How Bricks reach enforcement. The guard decides every agent action from per-file limits (the
// strictest autonomy ceiling and highest criticality of the active Zones and Flows covering a
// file). After those limits are computed, apply() folds the combined controls of each active
// target's attached Bricks into the limits of that target's files. It can only lower a ceiling or
// raise a criticality, so Bricks never widen what an agent may do, and because the hooks and
// 'crane scope' read the same limits, both decide with the Bricks without a second decision path.
// Only enabled attachments in the signed store count; drafts never do.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::compose::{controls, file_limit, scope_resolved};
use super::store::BrickStore;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::{Lifecycle, Limits, Resolved};

/** Fold the Bricks attached to active Zones and Flows into the per-file limits
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state (its signed Brick store)
    - entities: &[Resolved] - resolved Zones and Flows with their files
    - limits: &mut Limits - limits by file, computed from the Zones and Flows
 * Output
    - None (a malformed store is left to integrity(), which already stops every agent change)
*/
pub(crate) fn apply(
    workspace: &Workspace,
    state: &State,
    entities: &[Resolved],
    limits: &mut Limits,
) {
    let Ok(store) = BrickStore::of(state) else {
        return;
    };
    if store.attachments.is_empty() {
        return;
    }
    for entity in entities
        .iter()
        .filter(|entity| entity.status == Lifecycle::Active)
    {
        let key = format!("{}:{}", entity.kind, entity.id);
        let combined = controls(&store, &key, &workspace.root);
        if !combined.restricts() {
            continue;
        }
        let resolved = scope_resolved(&combined, &entity.files);
        for file in &entity.files {
            let (ceiling, criticality, _) = file_limit(&combined, file, resolved);
            let Some(limit) = limits.get_mut(file) else {
                continue;
            };
            let mut tightened = false;
            if let Some(ceiling) = ceiling.filter(|ceiling| *ceiling < limit.ceiling) {
                limit.ceiling = ceiling;
                tightened = true;
            }
            if let Some(criticality) = criticality.filter(|value| *value > limit.criticality) {
                limit.criticality = criticality;
                tightened = true;
            }
            if tightened || ceiling.is_some() {
                limit.bricks.extend(
                    combined
                        .sources
                        .iter()
                        .map(|reference| format!("brick:{reference}")),
                );
                limit.bricks.sort();
                limit.bricks.dedup();
            }
        }
    }
}

/** Preview the effect of a target's Bricks on its files, for people (the same functions apply()
 * uses, so the preview cannot differ from enforcement)
 * Input
    - workspace: &Workspace - repository
    - store: &BrickStore - signed store
    - target_key: &str - "flow:ID" or "zone:ID"
    - files: &BTreeSet<String> - the target's files
 * Output
    - Value {controls, scope_resolved, files: [{path, ceiling, criticality, reasons}]}
*/
pub(crate) fn preview(
    workspace: &Workspace,
    store: &BrickStore,
    target_key: &str,
    files: &BTreeSet<String>,
) -> Value {
    let combined = controls(store, target_key, &workspace.root);
    let resolved = scope_resolved(&combined, files);
    let rows = files
        .iter()
        .filter_map(|file| {
            let (ceiling, criticality, reasons) = file_limit(&combined, file, resolved);
            (ceiling.is_some() || criticality.is_some()).then(|| {
                json!({
                    "path": file,
                    "ceiling": ceiling.map(crate::governance::zones::Ceiling::name),
                    "criticality": criticality,
                    "reasons": reasons,
                })
            })
        })
        .collect::<Vec<_>>();
    json!({
        "target": target_key,
        "controls": combined,
        "restricts": combined.restricts(),
        "scope_resolved": resolved,
        "files": rows,
    })
}
