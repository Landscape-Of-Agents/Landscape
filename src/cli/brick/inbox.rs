// crane brick inbox: the Brick change requests people made on the DataSea dashboard. The
// dashboard holds no signing key, so it only keeps requests; a person applies them here, on a
// machine, with 'crane brick inbox apply' (or one click in Landscape, which runs it). Each request
// is checked again (brick/requests.rs) and carried out by running the same 'crane brick' command a
// person would type, so every validation, separation-of-duties rule, and signature applies as if
// they had typed it; a signed change needs this machine's admin key. Requests are never applied by
// a sync or a hook. 'dismiss' declines requests without applying them. The service is told what
// happened to each one, and the dashboard shows it.

use std::process::Command;

use serde_json::{json, Value};

use crate::brick::requests;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::governance::workspace::Workspace;
use crate::telemetry::ingest::Client;
use crate::trust::require_human;

/** The usage of 'crane brick inbox' */
const USAGE: &str =
    "crane brick inbox [list] | inbox apply ID...|--all | inbox dismiss ID... --reason TEXT";

/** Run 'crane brick inbox'
 * Input
    - args: &[String] - arguments after "inbox"
 * Output
    - Result<(), String>
*/
pub(super) fn inbox(args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        None | Some("list") => list(args.get(1..).unwrap_or_default()),
        Some("apply") => apply(&args[1..]),
        Some("dismiss") => dismiss(&args[1..]),
        Some(word) if word.starts_with("--") => list(args),
        Some(_) => Err(format!("usage: {USAGE}")),
    }
}

/** Connect to the service of this repository
 * Input
    - None
 * Output
    - Result<(Workspace, Client), String>
    - Error when uploads are off (requests reach a machine through the service it uploads to)
*/
fn connect() -> Result<(Workspace, Client), String> {
    let workspace = Workspace::locate()?;
    let client = Client::open(&workspace)?.ok_or(
        "uploads are off (CRANE_INGEST_URL is 'off'), so this machine cannot receive DataSea requests",
    )?;
    Ok((workspace, client))
}

/** The requests waiting at the service
 * Input
    - client: &Client - service connection
 * Output
    - Result<Vec<Value>, String>
*/
fn pending(client: &Client) -> Result<Vec<Value>, String> {
    Ok(client.brick_requests()?["requests"]
        .as_array()
        .cloned()
        .unwrap_or_default())
}

/** One line describing a request
 * Input
    - request: &Value - request
 * Output
    - String
*/
fn line(request: &Value) -> String {
    let text = |key: &str| request[key].as_str().unwrap_or_default().to_string();
    let params = &request["params"];
    let subject = [
        "brick_id",
        "ref",
        "subject",
        "attachment",
        "agent",
        "stack_id",
        "name",
    ]
    .iter()
    .find_map(|key| params[*key].as_str().filter(|value| !value.is_empty()))
    .or_else(|| params["draft"]["name"].as_str())
    .unwrap_or("");
    format!(
        "{}  {} {}  by @{} ({}): {}",
        text("request_id"),
        text("op"),
        subject,
        text("requested_by"),
        text("role"),
        text("reason")
    )
}

/** Run 'crane brick inbox list'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let (_, client) = connect()?;
    let requests = pending(&client)?;
    let detail = requests.iter().map(line).collect::<Vec<_>>();
    let response = Response::new(
        "brick.inbox",
        Status::Ok,
        if requests.is_empty() {
            "no Brick requests from DataSea are waiting".to_string()
        } else {
            format!(
                "{} Brick request(s) from DataSea are waiting; 'crane brick inbox apply ID' or '--all' applies them",
                requests.len()
            )
        },
    )
    .data(json!({"requests": requests}));
    finish(&args, response, &detail)
}

/** Choose the requests named on the command line (or all of them)
 * Input
    - args: &crate::cli::args::Args - parsed arguments
    - requests: Vec<Value> - waiting requests
 * Output
    - Result<Vec<Value>, String>
    - Error when nothing is named or a named request is not waiting
*/
fn chosen(args: &crate::cli::args::Args, requests: Vec<Value>) -> Result<Vec<Value>, String> {
    if args.flag("all") {
        return Ok(requests);
    }
    if args.positional.is_empty() {
        return Err(format!(
            "name the requests to act on, or --all; usage: {USAGE}"
        ));
    }
    args.positional
        .iter()
        .map(|id| {
            requests
                .iter()
                .find(|request| request["request_id"].as_str() == Some(id.as_str()))
                .cloned()
                .ok_or_else(|| format!("{id} is not waiting ('crane brick inbox' lists what is)"))
        })
        .collect()
}

/** Carry out one request by running its 'crane brick' command
 * Input
    - workspace: &Workspace - repository
    - request: &Value - request
 * Output
    - Value {request_id, status: APPLIED|FAILED, detail, command}
*/
fn carry_out(workspace: &Workspace, request: &Value) -> Value {
    let id = request["request_id"].as_str().unwrap_or_default();
    let op = request["op"].as_str().unwrap_or_default();
    let failed =
        |detail: String| json!({"request_id": id, "op": op, "status": "FAILED", "detail": detail});
    let directory = match workspace.trust() {
        Ok(trust) => trust.runtime().join("brick-inbox"),
        Err(error) => return failed(error),
    };
    if let Err(error) = std::fs::create_dir_all(&directory) {
        return failed(format!(
            "could not prepare {}: {error}",
            directory.display()
        ));
    }
    let file = directory.join(format!(
        "{}.input",
        id.replace(|c: char| !c.is_ascii_alphanumeric() && c != '_', "")
    ));
    let origin = format!(
        "@{} on DataSea, {id}",
        request["requested_by"].as_str().unwrap_or("someone")
    );
    let (command, written) = match requests::command(
        op,
        &request["params"],
        request["reason"].as_str().unwrap_or_default(),
        &origin,
        &file.to_string_lossy(),
    ) {
        Ok(built) => built,
        Err(error) => return failed(format!("refused here: {error}")),
    };
    if let Some(content) = &written {
        if let Err(error) = std::fs::write(&file, content) {
            return failed(format!("could not write {}: {error}", file.display()));
        }
    }
    let output = std::env::current_exe()
        .map_err(|error| error.to_string())
        .and_then(|crane| {
            Command::new(crane)
                .args(&command)
                .current_dir(&workspace.root)
                .output()
                .map_err(|error| error.to_string())
        });
    let _ = std::fs::remove_file(&file);
    let shown = format!(
        "crane {}",
        command
            .iter()
            .filter(|word| **word != "--format" && **word != "json")
            .map(|word| if word.contains(' ') {
                format!("'{word}'")
            } else {
                word.clone()
            })
            .collect::<Vec<_>>()
            .join(" ")
    );
    let output = match output {
        Ok(output) => output,
        Err(error) => return failed(format!("could not run Crane: {error}")),
    };
    let answer = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    let status = answer["status"].as_str().unwrap_or_default();
    let summary = answer["summary"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| {
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .trim_start_matches("crane: ")
                .to_string()
        });
    let applied = output.status.success() && matches!(status, "ok" | "passed");
    json!({
        "request_id": id,
        "op": op,
        "status": if applied { "APPLIED" } else { "FAILED" },
        "detail": crate::telemetry::redact::redact(&summary).0.chars().take(300).collect::<String>(),
        "command": shown,
    })
}

/** Run 'crane brick inbox apply'
 * Input
    - args: &[String] - request ids, or --all
 * Output
    - Result<(), String>
*/
fn apply(args: &[String]) -> Result<(), String> {
    require_human("crane brick inbox apply")?;
    let args = parse(args, &[], &["all"])?;
    let (workspace, client) = connect()?;
    let chosen = chosen(&args, pending(&client)?)?;
    let results = chosen
        .iter()
        .map(|request| carry_out(&workspace, request))
        .collect::<Vec<_>>();
    let reported = if results.is_empty() {
        Ok(json!({"settled": 0}))
    } else {
        client.settle_brick_requests(&results)
    };
    let applied = results
        .iter()
        .filter(|result| result["status"] == "APPLIED")
        .count();
    let detail = results
        .iter()
        .map(|result| {
            format!(
                "{} {} {}: {}",
                result["status"].as_str().unwrap_or_default(),
                result["request_id"].as_str().unwrap_or_default(),
                result["op"].as_str().unwrap_or_default(),
                result["detail"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let mut summary = format!("applied {applied} of {} DataSea request(s)", results.len());
    if let Err(error) = &reported {
        summary.push_str(&format!(
            "; the service could not be told ({error}), so DataSea still shows them waiting"
        ));
    }
    let response = Response::new(
        "brick.inbox.apply",
        if applied == results.len() {
            Status::Ok
        } else {
            Status::Failed
        },
        summary,
    )
    .data(json!({"results": results, "reported": reported.is_ok()}));
    finish(&args, response, &detail)
}

/** Run 'crane brick inbox dismiss'
 * Input
    - args: &[String] - request ids (or --all) and --reason
 * Output
    - Result<(), String>
*/
fn dismiss(args: &[String]) -> Result<(), String> {
    require_human("crane brick inbox dismiss")?;
    let args = parse(args, &["reason"], &["all"])?;
    let reason = args
        .value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or("--reason is required; it is shown on DataSea")?;
    let (_, client) = connect()?;
    let chosen = chosen(&args, pending(&client)?)?;
    let detail = format!(
        "dismissed by {}: {}",
        crate::platform::actor(),
        crate::telemetry::redact::redact(reason.trim()).0
    );
    let results = chosen
        .iter()
        .map(|request| json!({"request_id": request["request_id"], "op": request["op"], "status": "DISMISSED", "detail": detail.chars().take(300).collect::<String>()}))
        .collect::<Vec<_>>();
    if !results.is_empty() {
        client.settle_brick_requests(&results)?;
    }
    let response = Response::new(
        "brick.inbox.dismiss",
        Status::Ok,
        format!("dismissed {} DataSea request(s)", results.len()),
    )
    .data(json!({"results": results}));
    finish(&args, response, &[])
}
