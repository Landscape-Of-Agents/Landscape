// Repository commands (connect, status, view, delete), GitHub CLI login, and integrations.

use crate::github::{self, Method, Status, PRIVILEGES};
use crate::governance::workspace::Workspace;
use crate::integrations;
use crate::platform::render::iso_time;

/** Dispatch a repo command
 * Input
    - args: &[String] - arguments after "repo"
 * Output
    - Result<(), String>
*/
pub(crate) fn repo(args: &[String]) -> Result<(), String> {
    let usage = "usage: crane repo --ssh|--https|--gh-cli REMOTE [PRIVILEGES...] | crane repo status | crane repo --view | crane repo del";
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => status(),
        Some("--view") if args.len() == 1 => view(),
        Some("del") if args.len() == 1 => delete(),
        Some(flag @ ("--ssh" | "--https" | "--gh-cli")) => {
            let method = match flag {
                "--ssh" => Method::Ssh,
                "--https" => Method::Https,
                _ => Method::GhCli,
            };
            let remote = args.get(1).ok_or(usage)?;
            connect(method, remote, args[2..].to_vec())
        }
        _ => Err(usage.into()),
    }
}

/** Connect the repository with a method and privileges
 * Input
    - method: Method - connection method
    - remote: &str - remote
    - privileges: Vec<String> - privilege shorthands
 * Output
    - Result<(), String>
*/
fn connect(method: Method, remote: &str, privileges: Vec<String>) -> Result<(), String> {
    let workspace = Workspace::discover()?;
    let replaced = github::load(&workspace)?.is_some();
    let connection = github::connect(&workspace, method, remote, privileges)?;
    println!(
        "{} {} via {}: CONNECTED",
        if replaced { "Reconnected" } else { "Connected" },
        connection.normalized,
        method.name()
    );
    if connection.privileges.is_empty() {
        println!("  privileges: none (add rf, codeowners, contributors, ci-cd, gh-actions, gh-branches, gh-insights)");
    } else {
        println!("  privileges: {}", connection.privileges.join(", "));
    }
    Ok(())
}

/** Print the live connection status: CONNECTED, NOT_CONNECTED, or FAILURE with the error
 * Input
    - None
 * Output
    - Result<(), String>
    - Error when the status is FAILURE
*/
fn status() -> Result<(), String> {
    let workspace = Workspace::discover()?;
    match github::status(&workspace) {
        Status::Failure(error) => {
            println!("FAILURE: {error}");
            Err("the GitHub connection failed".into())
        }
        status => {
            println!("{}", status.name());
            Ok(())
        }
    }
}

/** Print the privileges granted to Crane and the GitHub grant
 * Input
    - None
 * Output
    - Result<(), String>
*/
fn view() -> Result<(), String> {
    let workspace = Workspace::discover()?;
    let Some(connection) = github::load(&workspace)? else {
        println!("NOT_CONNECTED: no privileges are granted; connect with 'crane repo --ssh|--https|--gh-cli REMOTE'");
        return Ok(());
    };
    println!("repository: {}", connection.normalized);
    println!(
        "method: {} ({})",
        connection.method.name(),
        connection.remote
    );
    println!(
        "connected: {} by {}",
        iso_time(connection.connected_at * 1000),
        connection.connected_by
    );
    println!("privileges:");
    for (name, description) in PRIVILEGES {
        let granted = connection.privileges.iter().any(|granted| granted == name);
        println!(
            "  [{}] {name:<13} {description}",
            if granted { "x" } else { " " }
        );
    }
    match github::active_grant() {
        Some(grant) => println!(
            "github login grant: {} until {}",
            grant.mode,
            iso_time(grant.expires_at * 1000)
        ),
        None => {
            println!("github login grant: none (run 'crane github' or 'crane github days DAYS')")
        }
    }
    Ok(())
}

/** Delete the connection
 * Input
    - None
 * Output
    - Result<(), String>
*/
fn delete() -> Result<(), String> {
    let workspace = Workspace::discover()?;
    if github::disconnect(&workspace)? {
        println!("Deleted the connection; crane repo status is now NOT_CONNECTED");
    } else {
        println!("There was no connection; crane repo status is NOT_CONNECTED");
    }
    Ok(())
}

/** Run gh auth login and record a read grant for the terminal session or a number of days
 * Input
    - args: &[String] - nothing, or "days DAYS"
 * Output
    - Result<(), String>
    - Error if DAYS is outside 1-180 or the login fails
*/
pub(crate) fn github(args: &[String]) -> Result<(), String> {
    let days = match args {
        [] => None,
        [word, days] if word == "days" => Some(github::parse_days(days)?),
        _ => return Err("usage: crane github | crane github days DAYS".into()),
    };
    let grant = github::login(days)?;
    println!(
        "GitHub read access granted to Crane until {} ({})",
        iso_time(grant.expires_at * 1000),
        if grant.mode == "session" {
            "this terminal session, at most 12 hours".to_string()
        } else {
            format!("{} days", grant.days.unwrap_or_default())
        }
    );
    Ok(())
}

/** Configure an integration interactively
 * Input
    - args: &[String] - jira, whatsapp, slack, or github
 * Output
    - Result<(), String>
*/
pub(crate) fn integrate(args: &[String]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: crane integrate jira|whatsapp|slack|github".into());
    };
    let integration = integrations::configure(name)?;
    println!(
        "Saved the {} integration ({} settings, {} secrets stored outside the repository)",
        integration.name,
        integration.settings.len(),
        integration.secret_fields.len()
    );
    Ok(())
}
