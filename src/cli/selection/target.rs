// crane selection target: the same command as 'crane target' (a signed selection that must change,
// in a stated way); both spellings call one handler.

use crate::selection::registry::Operation;

/** Run the command
 * Input
    - args: &[String] - arguments after "target"
 * Output
    - Result<(), String>
*/
pub(super) fn target(args: &[String]) -> Result<(), String> {
    crate::cli::governance::select(Operation::Target, args)
}
