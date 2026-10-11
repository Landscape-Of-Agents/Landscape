// The blocking convention between the hook adapters and the process entry point. Claude Code and
// Codex treat exit code 2 as "deny, and show stderr to the agent". A hook signals that by
// returning an error built with block(); main() recognizes it with blocked() and exits with 2,
// printing the message without the marker. Every other error exits with 1. The marker lives only
// here, so the convention cannot drift between the places that produce and consume it.

use std::fmt::Display;

/** Marker at the start of an error message that must block the agent */
const PREFIX: &str = "HOOK_BLOCK:";

/** Build an error that blocks the agent with a message
 * Input
    - message: impl Display - what the agent is told
 * Output
    - String error carrying the blocking marker
*/
pub(crate) fn block(message: impl Display) -> String {
    format!("{PREFIX}{message}")
}

/** Read the message of a blocking error
 * Input
    - error: &str - error message
 * Output
    - Option<&str> the message without the marker, None for an ordinary error
*/
pub(crate) fn blocked(error: &str) -> Option<&str> {
    error.strip_prefix(PREFIX)
}

#[cfg(test)]
mod tests {
    use super::{block, blocked};

    /** Check that a blocking error round-trips and an ordinary error is not mistaken for one
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn blocking_errors_round_trip() {
        assert_eq!(blocked(&block("denied: x")), Some("denied: x"));
        assert_eq!(blocked("could not read file"), None);
        assert_eq!(blocked(&block(format!("{}", 2))), Some("2"));
    }
}
