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
    let usage = "usage: crane repo --ssh|--https|--gh-cli REMOTE [PRIVILEGES...] | crane repo status | crane repo --view | crane repo del | crane repo rebind OLD_REMOTE";
    match args.first().map(String::as_str) {
        Some("status") if args.len() == 1 => status(),
        Some("--view") if args.len() == 1 => view(),
        Some("del") if args.len() == 1 => delete(),
        Some("rebind") if args.len() == 2 => rebind(&args[1]),
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

/** Rebind the repository to its new origin remote after a move (transfer or rename)
 * Input
    - old_remote: &str - the remote the governance state was signed for
 * Output
    - Result<(), String>
    - Error if the move cannot be proven or this machine cannot sign the change it needs
*/
fn rebind(old_remote: &str) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let rebound = crate::governance::rebind::rebind(&workspace, old_remote)?;
    match (rebound.resigned, rebound.generation) {
        (Some(count), Some(generation)) => {
            println!(
                "Rebound {} -> {} at generation {generation}: {count} selection(s), the admin list, and the manifest signed again",
                rebound.from, rebound.to
            );
            println!("  carried {} trust file(s) of this machine over", rebound.copied);
            println!("Commit .crane and push; each teammate then runs 'crane repo rebind {}' once.", rebound.from);
        }
        _ => println!(
            "{} -> {}: carried {} trust file(s) of this machine over; the signed state already belongs to {}",
            rebound.from, rebound.to, rebound.copied, rebound.to
        ),
    }
    println!(
        "Update the connection too: crane repo --https https://{}",
        rebound.to
    );
    Ok(())
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

/** Run 'crane integrate NAME --settings JSON [--secrets-stdin]' or 'crane integrate --status':
 * set an integration without prompts (every address must answer before anything is kept;
 * credentials come only from this machine's standard input), or list every integration's status
 * Input
    - args: &[String] - arguments after "integrate"
 * Output
    - Result<(), String>
*/
fn integrate_without_prompts(args: &[String]) -> Result<(), String> {
    use crate::cli::protocol::{finish, parse, Response, Status as Outcome};
    use serde_json::{json, Map, Value};
    let args = parse(args, &["settings"], &["secrets-stdin", "status"])?;
    let workspace = Workspace::discover()?;
    if args.flag("status") {
        let status = integrations::status(&workspace);
        let detail = status
            .as_object()
            .into_iter()
            .flatten()
            .map(|(name, row)| {
                format!(
                    "  {name}: {}",
                    if row["connected"] == true {
                        "connected".to_string()
                    } else if row["configured"] == true {
                        format!("missing {}", row["missing_secrets"])
                    } else {
                        "not set".to_string()
                    }
                )
            })
            .collect::<Vec<_>>();
        let response = Response::new("integrate.status", Outcome::Ok, "integration status")
            .data(json!({"integrations": status, "schemas": integrations::schema::schemas_json()}));
        return finish(&args, response, &detail);
    }
    crate::trust::require_human("crane integrate")?;
    args.expect_positional(1, "crane integrate NAME --settings JSON [--secrets-stdin]")?;
    let name = args.positional[0].to_ascii_lowercase();
    let given: Map<String, Value> = match args.value("settings") {
        Some(text) => serde_json::from_str::<Value>(&text)
            .map_err(|error| format!("--settings is not valid JSON: {error}"))?
            .as_object()
            .cloned()
            .ok_or("--settings must be a JSON object")?,
        None => Map::new(),
    };
    let credentials: Map<String, Value> = if args.flag("secrets-stdin") {
        let mut text = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)
            .map_err(crate::platform::io_error)?;
        if text.trim().is_empty() {
            Map::new()
        } else {
            serde_json::from_str::<Value>(&text)
                .map_err(|_| "the credentials on standard input are not a JSON object".to_string())?
                .as_object()
                .cloned()
                .ok_or("the credentials on standard input must be a JSON object")?
        }
    } else {
        Map::new()
    };
    if given.is_empty() && credentials.is_empty() {
        return Err(
            "give --settings JSON, credentials on standard input (--secrets-stdin), or both".into(),
        );
    }
    let saved = integrations::set_settings(&workspace, &name, &given, &credentials)?;
    let missing = saved["missing_secrets"]
        .as_array()
        .map(Vec::len)
        .unwrap_or(0);
    let mut detail = saved["checked"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|line| format!("  checked: {line}"))
        .collect::<Vec<_>>();
    if let Some(verified) = saved["verified"].as_object() {
        detail.push(format!(
            "  {}: {}",
            if verified["ok"] == true {
                "verified"
            } else {
                "not verified"
            },
            verified["message"].as_str().unwrap_or_default()
        ));
    }
    let summary = if missing > 0 {
        format!(
            "saved the {name} settings (every address answered); add {} on this machine: crane integrate {name} --secrets-stdin, or approve it in Landscape",
            saved["missing_secrets"].as_array().into_iter().flatten().filter_map(|secret| secret["label"].as_str()).collect::<Vec<_>>().join(", ")
        )
    } else {
        format!("saved the {name} integration; it is connected")
    };
    finish(
        &args,
        Response::new("integrate.set", Outcome::Ok, summary).data(saved),
        &detail,
    )
}

/** Configure an integration interactively
 * Input
    - args: &[String] - jira, whatsapp, slack, or github
 * Output
    - Result<(), String>
*/
pub(crate) fn integrate(args: &[String]) -> Result<(), String> {
    if args.len() > 1 || args.first().is_some_and(|arg| arg.starts_with("--")) {
        return integrate_without_prompts(args);
    }
    let [name] = args else {
        return Err("usage: crane integrate jira|whatsapp|slack|github|datadog|grafana|google_meet|oncall [--settings JSON] [--secrets-stdin] [--status]".into());
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
