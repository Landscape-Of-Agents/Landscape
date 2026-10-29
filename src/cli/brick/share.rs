// crane brick publish | catalog | import: share Bricks across the repositories of an organization
// without copying files. 'publish' sends an activated version to the org catalog on DataSea, signed
// with this machine's admin key (brick/share.rs); 'catalog' lists what the account's repositories
// published; 'import' turns a catalog entry, a downloaded draft, or a downloaded library into
// drafts here. An import grants nothing: this repository's admin reviews and activates each draft,
// so the repository stays the authority for what applies in it.

use serde_json::{json, Value};

use super::draft::{draft_of, file_of};
use super::{find, open, require_verified, Found};
use crate::brick::model::VersionState;
use crate::brick::share;
use crate::brick::store::{self, BrickDraft};
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::governance::workspace::Workspace;
use crate::telemetry::ingest::Client;
use crate::trust::require_human;

/** Connect to the service of this repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Client, String>
    - Error when uploads are off (the catalog lives on the service this machine uploads to)
*/
fn connect(workspace: &Workspace) -> Result<Client, String> {
    Client::open(workspace)?.ok_or_else(|| {
        "uploads are off (CRANE_INGEST_URL is 'off'), so the org catalog cannot be reached"
            .to_string()
    })
}

/** Run 'crane brick publish ID@N --reason TEXT'
 * Input
    - args: &[String] - arguments after "publish"
 * Output
    - Result<(), String>
*/
pub(super) fn publish(args: &[String]) -> Result<(), String> {
    require_human("crane brick publish")?;
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane brick publish ID@N --reason TEXT")?;
    let reason = args
        .value("reason")
        .map(|reason| reason.trim().to_string())
        .filter(|reason| !reason.is_empty())
        .ok_or("--reason is required; the catalog shows it")?;
    let repo = open()?;
    require_verified(&repo)?;
    let entry = match find(&repo, &args.positional[0])? {
        Found::Version(entry) => entry,
        Found::Draft(_) => {
            return Err("a draft is not published; an admin activates it first".into())
        }
    };
    if entry.state != VersionState::Active {
        return Err(format!(
            "{} is {}; only active versions are published",
            entry.brick.reference(),
            crate::cli::protocol::name(&entry.state)
        ));
    }
    let identity = repo
        .workspace
        .trust()?
        .signing_key()?
        .ok_or("this machine holds no signing key; publish from an admin's machine")?;
    let public = identity.public_hex();
    if !crate::governance::admins::signers(&repo.state).contains(&public) {
        return Err(
            "this machine's signing key is not an admin key of this repository; publish from an admin's machine"
                .into(),
        );
    }
    let reference = entry.brick.reference().to_string();
    let signature = identity.sign(
        share::DOMAIN,
        &share::message(&reference, &entry.brick.digest()),
    );
    let value = share::entry(
        &entry.brick,
        &public,
        signature,
        &crate::platform::actor(),
        &reason,
    );
    // Refuse here what the service refuses, so nothing secret-looking leaves the machine
    share::verify(&value)?;
    let answer = connect(&repo.workspace)?.publish_brick(&value)?;
    let account = answer["account"].as_str().unwrap_or("the organization");
    crate::platform::notes::note(
        "BRICK_PUBLISHED",
        json!({"brick": reference, "type": entry.brick.brick_type, "account": account, "stored": answer["stored"]}),
    );
    let response = Response::new(
        "brick.publish",
        Status::Ok,
        if answer["stored"] == true {
            format!("published {reference} to the catalog of {account}; its repositories import it with 'crane brick import org:{reference}'")
        } else {
            format!("{reference} was already in the catalog of {account} with the same digest")
        },
    )
    .reference("brick_ref", reference)
    .reference("digest", entry.brick.digest())
    .data(answer);
    finish(&args, response, &[])
}

/** Run 'crane brick catalog'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
pub(super) fn catalog(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let workspace = Workspace::locate()?;
    let answer = connect(&workspace)?.brick_catalog()?;
    let entries = answer["entries"].as_array().cloned().unwrap_or_default();
    let detail = entries
        .iter()
        .map(|entry| {
            let text = |key: &str| entry[key].as_str().unwrap_or_default().to_string();
            format!(
                "  org:{}  {} ({})  from {} by {}: {}",
                text("ref"),
                text("name"),
                text("type"),
                text("source_remote"),
                text("published_by"),
                text("reason")
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.catalog",
        Status::Ok,
        format!(
            "{} Brick version(s) in the catalog of {}; 'crane brick import org:ID@N' makes a draft of one here",
            entries.len(),
            answer["account"].as_str().unwrap_or("the organization")
        ),
    )
    .data(answer);
    finish(&args, response, &detail)
}

/** Read what 'crane brick import' was given: a catalog reference, or a file (a draft, a catalog
 * entry, or a library the dashboard downloads)
 * Input
    - workspace: &Workspace - repository
    - source: &str - "org:ID@N" or a file
 * Output
    - Result<Vec<(Value, String)>, String> draft-file contents with where each came from
*/
fn sources(workspace: &Workspace, source: &str) -> Result<Vec<(Value, String)>, String> {
    let from_entry = |entry: &Value| -> Result<(Value, String), String> {
        let brick = share::verify(entry)?;
        Ok((
            file_of(&brick),
            format!(
                "org:{} (digest {}, published by {}, key {})",
                brick.reference(),
                &brick.digest()[..16],
                entry["published_by"].as_str().unwrap_or("someone"),
                &entry["publisher_key"].as_str().unwrap_or_default()
                    [..16.min(entry["publisher_key"].as_str().unwrap_or_default().len())]
            ),
        ))
    };
    if source.starts_with("org:") {
        let (id, version) = share::reference(source)?;
        let answer = connect(workspace)?.brick_catalog_entry(&format!("{id}@{version}"))?;
        return Ok(vec![from_entry(&answer["entry"])?]);
    }
    let text = std::fs::read_to_string(source)
        .map_err(|error| format!("could not read {source}: {error}"))?;
    let value =
        serde_json::from_str::<Value>(&text).map_err(|error| format!("{source}: {error}"))?;
    match value["format"].as_str() {
        Some(share::FORMAT) => Ok(vec![from_entry(&value)?]),
        Some(share::LIBRARY_FORMAT) => value["bricks"]
            .as_array()
            .ok_or_else(|| format!("{source} holds no bricks"))?
            .iter()
            .map(|item| {
                let mut file = item.clone();
                let origin = format!("{source} ({})", item["ref"].as_str().unwrap_or("a Brick"));
                if let Some(object) = file.as_object_mut() {
                    object.remove("ref");
                    object.remove("state");
                    object.remove("sensitivity");
                }
                Ok((file, origin))
            })
            .collect(),
        Some(other) => Err(format!(
            "{source} is a '{other}' file, not a Brick draft, catalog entry, or library"
        )),
        None => Ok(vec![(value, source.to_string())]),
    }
}

/** Run 'crane brick import org:ID@N | FILE [--as ID]'
 * Input
    - args: &[String] - arguments after "import"
 * Output
    - Result<(), String>
*/
pub(super) fn import(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["as"], &[])?;
    args.expect_positional(1, "crane brick import org:ID@N | FILE [--as ID]")?;
    let repo = open()?;
    let found = sources(&repo.workspace, &args.positional[0])?;
    if found.is_empty() {
        return Err("there is nothing to import".into());
    }
    let id = args.value("as");
    if id.is_some() && found.len() > 1 {
        return Err("--as names one Brick; this source holds several".into());
    }
    // Every draft is checked before any is saved: an import is all or nothing
    let drafts = found
        .iter()
        .map(|(file, origin)| {
            draft_of(&repo, file, id.clone())
                .map(|draft| (draft, origin.clone()))
                .map_err(|error| format!("{origin}: {error}"))
        })
        .collect::<Result<Vec<(BrickDraft, String)>, String>>()?;
    let mut ids = drafts
        .iter()
        .map(|(draft, _)| draft.brick.brick_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    if ids.len() != drafts.len() {
        return Err(
            "the source holds the same Brick id twice; import them one at a time with --as".into(),
        );
    }
    let mut rows = Vec::new();
    for (draft, origin) in &drafts {
        store::save_brick_draft(&repo.workspace, draft)?;
        let reference = draft.brick.reference().to_string();
        crate::platform::notes::note(
            "BRICK_IMPORTED",
            json!({"brick": reference, "type": draft.brick.brick_type, "from": origin, "validation": draft.validation.status}),
        );
        rows.push(json!({
            "brick_ref": reference,
            "type": draft.brick.brick_type,
            "from": origin,
            "validation": draft.validation.status,
            "problems": draft.validation.problems,
        }));
    }
    let detail = drafts
        .iter()
        .map(|(draft, origin)| {
            format!(
                "  draft {} ({}) from {origin}: validation {}{}",
                draft.brick.reference(),
                draft.brick.brick_type.name(),
                crate::cli::protocol::name(&draft.validation.status),
                draft
                    .validation
                    .problems
                    .iter()
                    .map(|problem| format!("; {}", problem.message))
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "brick.import",
        Status::Ok,
        format!(
            "imported {} draft(s); they grant nothing until an admin here activates them ('crane brick activate ID --reason TEXT')",
            drafts.len()
        ),
    )
    .data(json!({"drafts": rows}));
    finish(&args, response, &detail)
}
