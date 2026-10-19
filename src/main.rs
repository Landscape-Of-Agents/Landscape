// Crane: AgentScript security contracts (signed selections, preserve and target policies),
// runtime enforcement at the agent hook boundary, and DataSea security telemetry.
//
// Layers, from the bottom: platform (Git, files, time) -> trust (keys, digests, signatures,
// rollback anchors) -> selection (anchors, signed registry, location tracker) -> governance
// (checkpoints, policies, configuration, verification) -> hooks (agent adapters and runtime
// enforcement) -> telemetry (identity, events, metrics, autonomy ledger, compliance) -> foxx
// (read-only dashboard) and integrations; cli dispatches the commands. The server feature adds the
// DataSea ingestion service, which stores every installation's uploads in MongoDB. capture is Work
// Capture & Evidence Ingestion, which reads those layers as evidence sources and never writes them.

mod capability;
mod capability_lab;
mod capture;
mod cli;
mod compiler;
mod discovery;
mod foxx;
mod github;
mod governance;
mod hooks;
mod integrations;
mod invocation;
mod platform;
mod selection;
#[cfg(feature = "server")]
mod server;
mod telemetry;
mod trust;

/** Entry point of the crane binary: run the command line and convert an error into a message
 * and an exit code: 2 for a blocking error (hooks::outcome, the blocking convention of Claude Code
 * and Codex hooks) and 1 for every other error
 * Input
    - None (arguments are read from the process environment)
 * Output
    - None
    - Exits the process with code 2 for hook blocks, 1 for other failures
*/
fn main() {
    restore_broken_pipe_default();
    if let Err(error) = cli::run() {
        let blocked = hooks::outcome::blocked(&error);
        eprintln!("crane: {}", blocked.unwrap_or(&error));
        std::process::exit(if blocked.is_some() { 2 } else { 1 });
    }
}

/** Let a closed output pipe end the process quietly, as it does for other command-line tools
 * ("crane validate | head" stops when head has enough). Rust ignores SIGPIPE, so a write into a
 * closed pipe fails and println! panics instead; this restores the default action. Windows has no
 * SIGPIPE
 * Input
    - None
 * Output
    - None
*/
#[cfg(unix)]
fn restore_broken_pipe_default() {
    extern "C" {
        /** Set the action of a signal (POSIX) */
        fn signal(signal: i32, handler: usize) -> usize;
    }
    /** SIGPIPE on Linux and macOS */
    const SIGPIPE: i32 = 13;
    /** The default action (SIG_DFL) */
    const DEFAULT: usize = 0;
    // SAFETY: signal only changes this process's action for SIGPIPE, before any other thread or
    // output exists; SIG_DFL is a valid handler value
    unsafe {
        signal(SIGPIPE, DEFAULT);
    }
}

/** Nothing to restore where there is no SIGPIPE
 * Input
    - None
 * Output
    - None
*/
#[cfg(not(unix))]
fn restore_broken_pipe_default() {}
