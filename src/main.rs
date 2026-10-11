// Crane: AgentScript security contracts (signed selections, preserve and target policies),
// runtime enforcement at the agent hook boundary, and Foxx security telemetry.
//
// Layers, from the bottom: platform (Git, files, time) -> trust (keys, digests, signatures,
// rollback anchors) -> selection (anchors, signed registry, location tracker) -> governance
// (checkpoints, policies, configuration, verification) -> hooks (agent adapters and runtime
// enforcement) -> telemetry (identity, events, metrics, autonomy ledger, compliance) -> foxx
// (read-only dashboard) and integrations; cli dispatches the commands. The server feature adds the
// Foxx ingestion service, which stores every installation's uploads in MongoDB.

mod cli;
mod foxx;
mod github;
mod governance;
mod hooks;
mod integrations;
mod platform;
mod selection;
#[cfg(feature = "server")]
mod server;
mod telemetry;
mod trust;

/** Entry point of the crane binary: run the command line and convert an error into a message
 * and an exit code, 2 for errors prefixed with HOOK_BLOCK: (the blocking convention of Claude Code
 * and Codex hooks, with the prefix left out of the message) and 1 for every other error
 * Input
    - None (arguments are read from the process environment)
 * Output
    - None
    - Exits the process with code 2 for hook blocks, 1 for other failures
*/
fn main() {
    if let Err(error) = cli::run() {
        eprintln!(
            "crane: {}",
            error.strip_prefix("HOOK_BLOCK:").unwrap_or(&error)
        );
        std::process::exit(if error.starts_with("HOOK_BLOCK:") {
            2
        } else {
            1
        });
    }
}
