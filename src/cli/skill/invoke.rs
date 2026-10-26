// crane skill invoke: request an invocation of an exact published version through the invocation
// engine; it executes only when the plan needs no approval, and succeeds only on verified
// postconditions. Shares the request path with 'skill plan'.

/** Run the command
 * Input
    - args: &[String] - arguments after "invoke"
 * Output
    - Result<(), String>
*/
pub(super) fn invoke(args: &[String]) -> Result<(), String> {
    super::plan::submit(args, true)
}
