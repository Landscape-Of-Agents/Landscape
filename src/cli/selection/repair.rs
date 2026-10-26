// crane selection repair: for a selection whose markers went missing, show where its signed baseline
// content appears now (exact matches only), for a person to review. Crane never rebinds a selection
// by itself: applying a repair is protecting the code again at its new place (an authorized,
// signed 'crane protect'), so --proposal only checks that the proposed place is one of the
// candidates and says what to run.

use serde_json::json;

use crate::cli::protocol::{finish, parse, Response, Status};
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::selection::tracker::{candidates, resolve, scan_repository, Resolution};

/** Run the command
 * Input
    - args: &[String] - arguments after "repair"
 * Output
    - Result<(), String>
*/
pub(super) fn repair(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["proposal"], &[])?;
    args.expect_positional(
        1,
        "crane selection repair SELECTION_ID [--proposal PATH:LINE]",
    )?;
    let id = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    let state = State::load(&workspace)?;
    let record = state
        .record(&id)
        .ok_or_else(|| format!("{id} is not a registered selection"))?;
    if let Resolution::Resolved(location) = resolve(&id, &scan_repository(&workspace)?) {
        let response = Response::new(
            "selection.repair",
            Status::Ok,
            format!(
                "selection {id} resolves at {}:{}; nothing to repair",
                location.span.path, location.span.start_line
            ),
        );
        return finish(&args, response, &[]);
    }
    let found = candidates(&workspace, record);
    let lines = record.origin.span.end_line - record.origin.span.start_line;
    let command = |place: &str| {
        let (path, line) = place.rsplit_once(':').unwrap_or((place, "1"));
        let start = line.parse::<usize>().unwrap_or(1);
        format!(
            "crane protect {path} start-line {start} end-line {} context \"{}\" policy {}",
            start + lines,
            record.context.clone().unwrap_or_default().replace('"', "'"),
            record.policy
        )
    };
    if let Some(proposal) = args.value("proposal") {
        if !found.contains(&proposal) {
            let response = Response::new(
                "selection.repair",
                Status::Denied,
                format!("{proposal} is not where the signed baseline content of {id} appears"),
            )
            .reasons(["PROPOSAL_NOT_A_CANDIDATE"])
            .data(json!({"candidates": found}));
            return finish(&args, response, &[]);
        }
        let response = Response::new("selection.repair", Status::Blocked, format!("{proposal} holds the baseline content of {id}; an authorized person protects it again there (the old record stays in the audit trail)"))
            .reasons(["REPAIR_NEEDS_SIGNATURE"])
            .data(json!({"proposal": proposal, "command": command(&proposal)}));
        return finish(&args, response, &[format!("  run: {}", command(&proposal))]);
    }
    let status = if found.is_empty() {
        Status::Unresolved
    } else {
        Status::Ok
    };
    let detail = found
        .iter()
        .map(|place| format!("  candidate {place}: {}", command(place)))
        .collect::<Vec<_>>();
    let response = Response::new(
        "selection.repair",
        status,
        format!(
            "selection {id} is unresolved; {} places hold its signed baseline content",
            found.len()
        ),
    )
    .reference("selection_ref", id.clone())
    .data(json!({"candidates": found}));
    finish(&args, response, &detail)
}
