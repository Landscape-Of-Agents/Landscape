// The live agent context Foxx shows while an agent works: when a tool call touches a selection,
// zone, or flow that has context, the hook writes that context (with the agent, the tool, and where
// in the code it applies) to one small file in the repository's runtime directory, replacing the
// previous one. Foxx asks 'crane foxx live --json' where the file is, then watches it, so the
// editor can highlight the context next to the code the agent is changing. The file holds only
// guidance that the agent already receives; nothing secret.

use serde_json::Value;
use std::path::PathBuf;

use crate::governance::workspace::Workspace;
use crate::platform::files::write_atomic;

/** The file name of the live context in the runtime directory */
const FILE: &str = "live_context.json";

/** Where the live context of a repository is written
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<PathBuf, String>
    - Error if the repository's trust directory cannot be located
*/
pub(crate) fn path(workspace: &Workspace) -> Result<PathBuf, String> {
    Ok(workspace.trust()?.runtime().join(FILE))
}

/** Replace the live context (best effort: enforcement never depends on it)
 * Input
    - workspace: &Workspace - repository
    - live: &Value - {at, agent, session, tool, operation, decision, contexts[]}
 * Output
    - Result<(), String>
    - Error if the file cannot be written
*/
pub(crate) fn publish(workspace: &Workspace, live: &Value) -> Result<(), String> {
    let path = path(workspace)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = serde_json::to_vec(live).map_err(|error| error.to_string())?;
    write_atomic(&path, &text)
}
