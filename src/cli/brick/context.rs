// crane brick context preview | status. Preview assembles exactly the snapshot agents working in
// the targets receive (plus the Flow and Zone context the hooks deliver with each tool call), with
// every section's source, reference, and digest, and the enforced controls; status shows the
// snapshots the hooks assembled, why each was refreshed, and what every session received.

use std::collections::BTreeSet;

use serde_json::json;

use super::{open, target_list};
use crate::brick::context::{assemble, render, status as delivery_status};
use crate::brick::model::TargetKind;
use crate::cli::protocol::{finish, parse, Response, Status};

/** Run 'crane brick context preview'
 * Input
    - args: &[String] - arguments after "context preview"
 * Output
    - Result<(), String>
*/
pub(super) fn preview(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["targets", "target", "flow", "zone"], &[])?;
    args.expect_positional(0, "crane brick context preview --targets flow:ID,zone:ID")?;
    let mut targets = match args.value("targets") {
        Some(text) => target_list(&text)?,
        None => super::target_option(&args)?.into_iter().collect(),
    };
    targets.retain(|target| target.kind != TargetKind::Runbook);
    if targets.is_empty() {
        return Err("name the Flows or Zones to preview: --targets flow:ID,zone:ID".into());
    }
    let repo = open()?;
    let known = super::targets(&repo)?;
    let mut notes = Vec::new();
    for target in &targets {
        match known.get(&target.key()) {
            None => {
                return Err(format!(
                    "{} is not a Zone or Flow of .crane/governance.yaml",
                    target.key()
                ))
            }
            Some(info) if info.status != crate::governance::zones::Lifecycle::Active => {
                notes.push(format!(
                    "{} is not active, so agents receive nothing for it yet",
                    target.key()
                ))
            }
            Some(_) => {}
        }
    }
    let keys = targets
        .iter()
        .map(crate::brick::model::Target::key)
        .collect::<BTreeSet<_>>();
    let governance = crate::governance::zones::load(&repo.workspace)?;
    let snapshot = assemble(
        &repo.workspace,
        &repo.state,
        &repo.store,
        Some(&governance),
        &keys,
    );
    let (text, truncated) = render(&snapshot);
    let effects = keys
        .iter()
        .map(|key| {
            let files = known
                .get(key)
                .map(|info| info.files.clone())
                .unwrap_or_default();
            crate::brick::effect::preview(&repo.workspace, &repo.store, key, &files)
        })
        .collect::<Vec<_>>();
    crate::platform::notes::note(
        "BRICK_CONTEXT_PREVIEWED",
        json!({"targets": snapshot.targets, "snapshot": snapshot.digest, "bytes": snapshot.bytes(), "bricks": snapshot.bricks.len(), "conflicts": snapshot.conflicts.len()}),
    );
    let mut detail = snapshot
        .sections
        .iter()
        .map(|section| {
            format!(
                "  [{}] {} {} ({} bytes, {})",
                section.kind,
                section.source,
                section.reference,
                section.bytes,
                &section.digest[..12]
            )
        })
        .collect::<Vec<_>>();
    detail.extend(
        snapshot
            .conflicts
            .iter()
            .map(|problem| format!("  conflict {}: {}", problem.code, problem.message)),
    );
    detail.extend(
        snapshot
            .gaps
            .iter()
            .map(|problem| format!("  gap {}: {}", problem.code, problem.message)),
    );
    detail.extend(notes.iter().map(|note| format!("  note: {note}")));
    detail.push(String::new());
    detail.push(text.clone());
    let response = Response::new(
        "brick.context.preview",
        if snapshot.conflicts.is_empty() { Status::Ok } else { Status::Unresolved },
        format!(
            "snapshot {} for {}: {} section(s), {} bytes, {} Brick(s){}",
            &snapshot.digest[..12],
            snapshot.targets.join(", "),
            snapshot.sections.len(),
            snapshot.bytes(),
            snapshot.bricks.len(),
            if snapshot.conflicts.is_empty() { String::new() } else { format!(", {} conflict(s)", snapshot.conflicts.len()) }
        ),
    )
    .reference("snapshot_digest", snapshot.digest.clone())
    .reference("store_digest", repo.state.bricks_digest.clone())
    .data(json!({"snapshot": snapshot, "rendered": text, "truncated": truncated, "effects": effects, "notes": notes}));
    finish(&args, response, &detail)
}

/** Run 'crane brick context status'
 * Input
    - args: &[String] - arguments after "context status"
 * Output
    - Result<(), String>
*/
pub(super) fn status(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(0, "crane brick context status")?;
    let repo = open()?;
    let runtime = repo.workspace.trust()?.runtime();
    let status = delivery_status(&runtime);
    let snapshots = status["snapshots"].as_array().map_or(0, Vec::len);
    let sessions = status["sessions"].as_array().map_or(0, Vec::len);
    let (delivered, reused) = status["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|session| {
            session["deliveries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .fold((0u64, 0u64), |(delivered, reused), item| {
            (delivered + 1, reused + item["reused"].as_u64().unwrap_or(0))
        });
    let detail = status["snapshots"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|snapshot| {
            format!(
                "  {} {} {} bytes, refreshed: {}",
                snapshot["targets"]
                    .as_array()
                    .map(|targets| targets
                        .iter()
                        .filter_map(|target| target.as_str())
                        .collect::<Vec<_>>()
                        .join(","))
                    .unwrap_or_default(),
                snapshot["digest"]
                    .as_str()
                    .map(|digest| &digest[..12.min(digest.len())])
                    .unwrap_or_default(),
                snapshot["bytes"],
                snapshot["refresh_reason"].as_str().unwrap_or("?")
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.context.status",
        Status::Ok,
        format!("{snapshots} snapshot(s); {sessions} session(s) received {delivered} snapshot(s) and {reused} unchanged reference(s) instead of resends"),
    )
    .data(json!({
        "cache": status,
        "measured": {"deliveries": delivered, "reused": reused},
        "provider_caching": "not measured: Crane sends context through the agent hooks; whether a provider caches prompts is not observable here, so no token savings are claimed",
    }));
    finish(&args, response, &detail)
}
