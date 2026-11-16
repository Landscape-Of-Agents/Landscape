// The Brick part of the repository status a machine reports to DataSea (foxx::status): what the
// Flows section names on its zones and flows. Each Brick in the library (activated versions and
// drafts) is sent by reference, name, type, state, and intent, with the agent assignments and
// their runtime status. Configuration and context text stay on this machine.

use serde_json::{json, Value};

use super::store;
use crate::governance::workspace::Workspace;

/** Collect the Brick report of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value {bricks: [{ref, name, type, state, intent}], assignments} (an {error} object when the
      store cannot be read)
*/
pub(crate) fn collect(workspace: &Workspace) -> Value {
    let (_, store, _) = match store::load(workspace) {
        Ok(loaded) => loaded,
        Err(error) => return json!({"error": error}),
    };
    let (drafts, _) = store::brick_drafts(workspace);
    let library = store
        .bricks
        .iter()
        .map(|entry| {
            json!({
                "ref": entry.brick.reference().to_string(), "name": entry.brick.name,
                "type": entry.brick.brick_type, "state": entry.state, "intent": entry.brick.intent,
            })
        })
        .chain(drafts.iter().map(|draft| {
            json!({
                "ref": draft.brick.brick_id, "name": draft.brick.name,
                "type": draft.brick.brick_type, "state": "draft", "intent": draft.brick.intent,
            })
        }))
        .collect::<Vec<_>>();
    let assignments = workspace
        .trust()
        .ok()
        .and_then(|trust| super::runtime::assignments(workspace, &trust.runtime()).ok())
        .unwrap_or_default();
    json!({"bricks": library, "assignments": assignments})
}
