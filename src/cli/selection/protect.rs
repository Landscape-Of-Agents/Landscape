// crane selection protect: the same command as 'crane protect' (a signed selection that must stay as
// it was at its checkpoint); both spellings call one handler.

use crate::selection::registry::Operation;

/** Run the command
 * Input
    - args: &[String] - arguments after "protect"
 * Output
    - Result<(), String>
*/
pub(super) fn protect(args: &[String]) -> Result<(), String> {
    crate::cli::governance::select(Operation::Preserve, args)
}
