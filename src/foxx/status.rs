// The repository status a machine reports to DataSea: what the Flows section shows (flows, zones,
// drafts, watchers, incidents, and the Bricks and assignments on them), which the service cannot
// see. The upload sync sends it at most every two minutes, or sooner when the registry generation
// changes, and the service keeps the latest one per machine and repository. Nothing else about the
// machine leaves it: events, the ledger, governance details, and alerts stay local.

use serde_json::{json, Value};

use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::platform::now_millis;

/** Collect this machine's status of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value {collected_at, crane_version, registry_generation, flows, bricks}
*/
pub(crate) fn collect(workspace: &Workspace) -> Value {
    json!({
        "collected_at": now_millis(),
        "crane_version": env!("CARGO_PKG_VERSION"),
        "registry_generation": State::load(workspace).ok().as_ref().map(State::generation),
        "flows": crate::flow::report::collect(workspace),
        "bricks": crate::brick::report::collect(workspace),
    })
}
