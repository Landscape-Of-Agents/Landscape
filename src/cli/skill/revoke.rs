// crane skill revoke: revoke an exact version; it is blocked by invocation preflight on this
// machine and on every machine that pulls the signed shared skill store. Running invocations meet
// the revocation at their next preflight. A revocation is never undone; a new version is.

use crate::capability::lifecycle::LifecycleState;

/** Run the command
 * Input
    - args: &[String] - arguments after "revoke"
 * Output
    - Result<(), String>
*/
pub(super) fn revoke(args: &[String]) -> Result<(), String> {
    super::retire::withdraw(args, "revoke", LifecycleState::Revoked)
}
