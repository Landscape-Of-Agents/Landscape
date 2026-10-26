// crane selection inspect: a selection's signed record (operation, policy, checkpoint, digests,
// context) and where it resolves now. An identifier alone grants nothing; an unresolved selection
// stays unresolved.

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::selection::tracker::{resolve, scan_repository, Resolution};

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane selection inspect SELECTION_ID")?;
    let id = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    let state = State::load(&workspace)?;
    let record = state
        .record(&id)
        .ok_or_else(|| format!("{id} is not a registered selection"))?;
    let scan = scan_repository(&workspace)?;
    let (resolved, location) = match resolve(&id, &scan) {
        Resolution::Resolved(location) => (
            true,
            json!({"path": location.span.path, "start_line": location.span.start_line, "end_line": location.span.end_line}),
        ),
        Resolution::Unresolved { code, reason } => {
            (false, json!({"unresolved": code, "reason": reason}))
        }
    };
    let detail = vec![
        format!(
            "  {} in policy {}, checkpoint {}",
            record.operation.name(),
            record.policy,
            record.origin.checkpoint
        ),
        format!(
            "  context: {}",
            record.context.clone().unwrap_or_else(|| "none".into())
        ),
        format!(
            "  now: {}",
            if resolved {
                format!(
                    "{}:{}-{}",
                    location["path"].as_str().unwrap_or_default(),
                    location["start_line"],
                    location["end_line"]
                )
            } else {
                format!(
                    "UNRESOLVED ({})",
                    location["reason"].as_str().unwrap_or_default()
                )
            }
        ),
        format!("  content digest {}", record.origin.content_digest),
    ];
    let response = Response::new(
        "selection.inspect",
        if resolved {
            Status::Ok
        } else {
            Status::Unresolved
        },
        format!(
            "selection {id}{}",
            record
                .name
                .as_deref()
                .map(|name| format!(" ({name})"))
                .unwrap_or_default()
        ),
    )
    .reference("selection_ref", id.clone())
    .data(json!({"record": record, "location": location}));
    finish(&args, response, &detail)
}
