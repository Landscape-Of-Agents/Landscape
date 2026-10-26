// crane selection: the signed code selections under one family name, one file per command.
// protect and target are aliases of 'crane protect' and 'crane target' (the same handler, records,
// and signatures); inspect, validate, and repair read the existing selection services (registry,
// location tracker, verifier). Nothing about the .crane records changes.

mod inspect;
mod protect;
mod repair;
mod target;
mod validate;

/** The command reference of 'crane selection' */
const USAGE: &str = "usage: crane selection protect FILE context TEXT [start-line N] [end-line N] ...   (same as crane protect)
       crane selection target FILE context TEXT [start-line N] [end-line N] [change_type TYPE] ...   (same as crane target)
       crane selection inspect SELECTION_ID
       crane selection validate [SELECTION_ID]
       crane selection repair SELECTION_ID [--proposal PATH:LINE]
  (inspect, validate, and repair take --format json)";

/** Dispatch a selection command
 * Input
    - args: &[String] - arguments after "selection"
 * Output
    - Result<(), String>
*/
pub(crate) fn selection(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("protect") => protect::protect(rest),
        Some("target") => target::target(rest),
        Some("inspect") => inspect::inspect(rest),
        Some("validate") => validate::validate(rest),
        Some("repair") => repair::repair(rest),
        _ => Err(USAGE.into()),
    }
}
