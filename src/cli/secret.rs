// crane secret set | list | remove: the local credential vault (trust::vault). Environment Bricks
// name credentials; this is where a person puts their values, on each machine. A value is read
// from standard input (a pipe) or a hidden prompt, never from an argument, so it does not reach
// the shell history or the process list. Only a person runs these commands.

use std::io::{IsTerminal, Read};

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::governance::workspace::Workspace;
use crate::trust::{require_human, vault};

/** The command reference of 'crane secret' */
const USAGE: &str =
    "usage: crane secret set NAME      (the value is read from standard input or a hidden prompt)
       crane secret list
       crane secret remove NAME
  (values stay on this machine; Environment Bricks name them, Crane-run operations use them)";

/** Dispatch a secret command
 * Input
    - args: &[String] - arguments after "secret"
 * Output
    - Result<(), String>
*/
pub(crate) fn secret(args: &[String]) -> Result<(), String> {
    require_human("crane secret")?;
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let rest = |count: usize| args.get(count..).unwrap_or_default();
    match words.as_slice() {
        ["set", ..] => set(rest(1)),
        ["list", ..] => list(rest(1)),
        ["remove", ..] => remove(rest(1)),
        _ => Err(USAGE.into()),
    }
}

/** Run 'crane secret set'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn set(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane secret set NAME")?;
    let name = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    let value = if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("Value of {name} (hidden): "))
            .map_err(|error| error.to_string())?
    } else {
        let mut text = String::new();
        std::io::stdin()
            .read_to_string(&mut text)
            .map_err(crate::platform::io_error)?;
        text
    };
    let value = value.trim_end_matches(['\r', '\n']).to_string();
    let protection = vault::set(&workspace, &name, &value)?;
    let response = Response::new(
        "secret.set",
        Status::Ok,
        format!(
            "stored {name} in this machine's vault ({protection}); it never leaves this machine"
        ),
    )
    .reference("secret", name.clone())
    .data(json!({"name": name, "protection": protection}));
    finish(&args, response, &[])
}

/** Run 'crane secret list'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let workspace = Workspace::locate()?;
    let listed = vault::list(&workspace)?;
    let detail = listed
        .iter()
        .map(|entry| {
            format!(
                "  {:<32} {:<10} set by {}",
                entry.name, entry.protection, entry.set_by
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "secret.list",
        Status::Ok,
        format!("{} credential(s) in this machine's vault", listed.len()),
    )
    .data(json!({"secrets": listed}));
    finish(&args, response, &detail)
}

/** Run 'crane secret remove'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn remove(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane secret remove NAME")?;
    let name = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    let existed = vault::remove(&workspace, &name)?;
    let response = Response::new(
        "secret.remove",
        if existed {
            Status::Ok
        } else {
            Status::Unresolved
        },
        if existed {
            format!("removed {name} from this machine's vault")
        } else {
            format!("{name} was not in this machine's vault")
        },
    )
    .data(json!({"name": name, "existed": existed}));
    finish(&args, response, &[])
}
