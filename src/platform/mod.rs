// Platform helpers shared by every layer: time, error conversion, atomic files, cross-process
// locks, and Git access. Nothing here knows about selections, policies, or agents.

pub(crate) mod files;
pub(crate) mod git;
pub(crate) mod glob;
pub(crate) mod http;
pub(crate) mod notes;
pub(crate) mod probe;
pub(crate) mod process;
pub(crate) mod render;

use std::fmt::Display;
use std::time::{SystemTime, UNIX_EPOCH};

/** Convert any displayable error into Crane's String error type, by calling to_string on it so it
 * can be used with map_err
 * Input
    - error: E - any error implementing Display
 * Output
    - String containing the error message
*/
pub(crate) fn io_error<E: Display>(error: E) -> String {
    error.to_string()
}

/** Get the current Unix time in seconds, by measuring the system clock against UNIX_EPOCH and
 * falling back to 0 if the clock is before the epoch
 * Input
    - None
 * Output
    - u64 seconds since the Unix epoch
*/
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

/** Get the current Unix time in milliseconds, used for event ordering and latency measurements
 * Input
    - None
 * Output
    - u64 milliseconds since the Unix epoch
*/
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/** Validate a policy, checkpoint, or integration name, by rejecting empty values, values longer
 * than 64 characters, values not starting with a letter, and any character other than ASCII
 * letters, digits, underscore, or hyphen
 * Input
    - kind: &str - what the name identifies, used in the error message
    - value: &str - candidate name
 * Output
    - Result<(), String>
    - Error if the name is empty, too long, or contains an unsafe character
*/
pub(crate) fn validate_name(kind: &str, value: &str) -> Result<(), String> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.starts_with(|character: char| character.is_ascii_alphabetic())
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || character == '_' || character == '-'
        });
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid {kind} name '{value}'; use 1-64 letters, digits, '_' or '-', starting with a letter"
        ))
    }
}

/** Return the name of the human or process running Crane, for audit records, by reading the Git
 * user name and falling back to the operating-system user
 * Input
    - None
 * Output
    - String actor name, "unknown" when nothing identifies the user
*/
pub(crate) fn actor() -> String {
    git::git(&["config", "user.name"])
        .ok()
        .filter(|name| !name.is_empty())
        .or_else(|| std::env::var("USERNAME").ok())
        .or_else(|| std::env::var("USER").ok())
        .unwrap_or_else(|| "unknown".into())
}
