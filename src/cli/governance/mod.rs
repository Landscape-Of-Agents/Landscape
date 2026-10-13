// Governance commands, one file per responsibility: init and joining (init.rs), admins (admin.rs),
// checkpoints and selections (selections.rs), verification (checks.rs), policies (policies.rs), and
// signing zones and flows (zones.rs).
// The helpers they share (policy block text, key fingerprints, this machine's role) live here.

mod admin;
mod checks;
mod init;
mod policies;
mod selections;
mod zones;

pub(crate) use admin::admin;
pub(crate) use checks::{test, validate};
pub(crate) use init::init;
pub(crate) use policies::{add_context, create_policy, move_marker, policy_status, view_context};
pub(crate) use selections::{
    checkpoint, parse, select, set_default_checkpoint, set_default_policy,
};
pub(crate) use zones::governance;

use crate::governance::admins::{self};
use crate::governance::state::State;
use crate::trust::TrustStore;

/** Text of a new policy block
 * Input
    - name: &str - policy name
 * Output
    - String
*/
fn empty_block(name: &str) -> String {
    format!("policy {name} {{\n}}\n")
}

/** Format a public key in groups of four characters, for comparing fingerprints by eye
 * Input
    - key: &str - public key in hex
 * Output
    - String
*/
fn grouped(key: &str) -> String {
    key.as_bytes()
        .chunks(4)
        .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

/** Describe this machine's role in the repository
 * Input
    - state: &State - governance state
    - trust: &TrustStore - trust directory
 * Output
    - Result<String, String> owner, admin, or member, with this machine's key
*/
fn role(state: &State, trust: &TrustStore) -> Result<String, String> {
    let Some(key) = trust.signing_public_key()? else {
        return Ok("member (this machine has no key yet)".into());
    };
    let role = if admins::owner(state).as_deref() == Some(key.as_str()) {
        "owner"
    } else if admins::signers(state).contains(&key) {
        "admin"
    } else {
        "member"
    };
    Ok(format!("{role} (key {})", grouped(&key)))
}
