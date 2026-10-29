// crane brick: reusable Bricks of a fixed type, Brick Stacks, their attachments to Flows and Zones
// (with per-attachment Current Context), agent assignments, context previews, and monitors. The
// commands only parse and print; every rule is in src/brick. Drafts are unsigned files that grant
// nothing; activation, attachment, context changes, revocation, and assignment are signed into
// .crane/bricks.json by an admin. Every command takes --format json (the shared envelope and exit
// codes of cli::protocol) and accepts --dry-run where it would sign, so the extension can preview
// the exact result before applying it. One file per area: catalog (types, list, show, history),
// draft (create, edit), check (validate, test), lifecycle (activate, deprecate, revoke), stack,
// attach (attach, detach, upgrade, context set), context (preview, status), agents (assign,
// unassign, agents, monitor), inbox (the changes people requested on DataSea, applied here).

mod agents;
mod attach;
mod catalog;
mod check;
mod context;
mod draft;
mod inbox;
mod lifecycle;
mod migrate;
mod stack;

use std::collections::{BTreeMap, BTreeSet};

use crate::brick::model::{BrickEntry, Target, TargetKind};
use crate::brick::store::{self, BrickDraft, BrickStore, Trust};
use crate::cli::args::Args;
use crate::governance::policy::PolicySet;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::{self, Lifecycle};

/** The command reference of 'crane brick' */
const USAGE: &str = "usage: crane brick types
       crane brick list [--type TYPE] [--state draft|active|deprecated|revoked] [--target flow:ID]
       crane brick show ID[@N]
       crane brick history [ID]
       crane brick create (--file DRAFT.json | --type TYPE --name NAME [--id ID] [--intent TEXT]
                          [--description TEXT] [--owner NAME] [--departments A,B] [--config JSON|FILE]
                          [--context TEXT | --context-file FILE] [--sensitivity internal|restricted]
                          [--depends ID@N,ID@N])
       crane brick edit ID [the options of create, without --type]
       crane brick migrate [--dry-run]
       crane brick validate ID
       crane brick test ID[@N] [--target flow:ID|zone:ID]
       crane brick activate ID --reason TEXT [--dry-run]
       crane brick deprecate ID@N --reason TEXT
       crane brick revoke ID[@N] --reason TEXT
       crane brick stack create --name NAME --members ID@N,ID@N [--id ID] [--purpose TEXT]
       crane brick stack show|resolve ID[@N]
       crane brick stack activate ID --reason TEXT [--dry-run]
       crane brick stack revoke ID@N --reason TEXT
       crane brick attach ID@N|stack:ID@N --target flow:ID|zone:ID|runbook:ID
                          [--context TEXT | --context-file FILE] [--dry-run]
       crane brick detach ATTACHMENT --reason TEXT [--dry-run]
       crane brick upgrade ATTACHMENT --to ID@N|stack:ID@N --reason TEXT [--dry-run]
       crane brick context set ATTACHMENT (--text TEXT | --file FILE) [--dry-run]
       crane brick context preview --targets flow:ID,zone:ID
       crane brick context status
       crane brick assign AGENT --targets flow:ID,zone:ID [--paused]
       crane brick unassign AGENT --reason TEXT
       crane brick agents
       crane brick monitor run [--all] | monitor status
       crane brick inbox [list] | inbox apply ID...|--all | inbox dismiss ID...|--all --reason TEXT
  (types are fixed; every command takes --format json; signing commands need an admin key)";

/** Dispatch a brick command
 * Input
    - args: &[String] - arguments after "brick"
 * Output
    - Result<(), String>
*/
pub(crate) fn brick(args: &[String]) -> Result<(), String> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let rest = |count: usize| args.get(count..).unwrap_or_default();
    match words.as_slice() {
        ["types", ..] => catalog::types(rest(1)),
        ["list", ..] => catalog::list(rest(1)),
        ["show", ..] => catalog::show(rest(1)),
        ["history", ..] => catalog::history(rest(1)),
        ["create", ..] => draft::create(rest(1)),
        ["edit", ..] => draft::edit(rest(1)),
        ["migrate", ..] => migrate::migrate(rest(1)),
        ["validate", ..] => check::validate(rest(1)),
        ["test", ..] => check::test(rest(1)),
        ["activate", ..] => lifecycle::activate(rest(1)),
        ["deprecate", ..] => lifecycle::withdraw(rest(1), false),
        ["revoke", ..] => lifecycle::withdraw(rest(1), true),
        ["stack", "create", ..] => stack::create(rest(2)),
        ["stack", "show" | "resolve", ..] => stack::show(rest(2)),
        ["stack", "activate", ..] => stack::activate(rest(2)),
        ["stack", "revoke", ..] => stack::revoke(rest(2)),
        ["attach", ..] => attach::attach(rest(1)),
        ["detach", ..] => attach::detach(rest(1)),
        ["upgrade", ..] => attach::upgrade(rest(1)),
        ["context", "set", ..] => attach::set_context(rest(2)),
        ["context", "preview", ..] => context::preview(rest(2)),
        ["context", "status", ..] => context::status(rest(2)),
        ["assign", ..] => agents::assign(rest(1)),
        ["unassign", ..] => agents::unassign(rest(1)),
        ["agents", ..] => agents::agents(rest(1)),
        ["monitor", "run", ..] => agents::monitor_run(rest(2)),
        ["monitor", "status", ..] => agents::monitor_status(rest(2)),
        ["inbox", ..] => inbox::inbox(rest(1)),
        _ => Err(USAGE.into()),
    }
}

/** The repository as the brick commands see it
 * Fields
    - workspace: Workspace - repository
    - state: State - governance state
    - store: BrickStore - signed Brick store
    - trust: Trust - whether the signed state vouches for the store
*/
pub(super) struct Repo {
    pub(super) workspace: Workspace,
    pub(super) state: State,
    pub(super) store: BrickStore,
    pub(super) trust: Trust,
}

/** Open the repository
 * Input
    - None
 * Output
    - Result<Repo, String>
*/
pub(super) fn open() -> Result<Repo, String> {
    let workspace = Workspace::locate()?;
    let (state, store, trust) = store::load(&workspace)?;
    Ok(Repo {
        workspace,
        state,
        store,
        trust,
    })
}

/** A Zone or Flow as attachments see it
 * Fields
    - status: Lifecycle - proposed, active, or deprecated
    - files: BTreeSet<String> - files it covers now
*/
pub(super) struct TargetInfo {
    pub(super) status: Lifecycle,
    pub(super) files: BTreeSet<String>,
}

/** Resolve the Zones and Flows of the repository with their files
 * Input
    - repo: &Repo - repository
 * Output
    - Result<BTreeMap<String, TargetInfo>, String> by "zone:ID" / "flow:ID"
*/
pub(super) fn targets(repo: &Repo) -> Result<BTreeMap<String, TargetInfo>, String> {
    let governance = zones::load(&repo.workspace)?;
    let policies = PolicySet::load(&repo.workspace)?;
    let resolution = zones::resolve(&repo.workspace, &governance, &repo.state, &policies)?;
    Ok(resolution
        .entities
        .into_iter()
        .map(|entity| {
            (
                format!("{}:{}", entity.kind, entity.id),
                TargetInfo {
                    status: entity.status,
                    files: entity.files,
                },
            )
        })
        .collect())
}

/** Read a target option (--target flow:ID, or --flow ID / --zone ID / --runbook ID)
 * Input
    - args: &Args - parsed arguments
 * Output
    - Result<Option<Target>, String>
*/
pub(super) fn target_option(args: &Args) -> Result<Option<Target>, String> {
    let given = [
        args.value("target"),
        args.value("flow").map(|id| format!("flow:{id}")),
        args.value("zone").map(|id| format!("zone:{id}")),
        args.value("runbook").map(|id| format!("runbook:{id}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    match given.as_slice() {
        [] => Ok(None),
        [one] => Target::parse(one).map(Some),
        _ => Err("give one target: --target, --flow, --zone, or --runbook".into()),
    }
}

/** Read a list of targets ("flow:a,zone:b")
 * Input
    - text: &str - list
 * Output
    - Result<Vec<Target>, String>
*/
pub(super) fn target_list(text: &str) -> Result<Vec<Target>, String> {
    let targets = text
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(Target::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if targets.is_empty() {
        return Err("name at least one target (flow:ID, zone:ID, or runbook:ID)".into());
    }
    Ok(targets)
}

/** Check that a target exists (Flows and Zones in governance.yaml; runbooks are only recorded)
 * Input
    - repo: &Repo - repository
    - target: &Target - target
 * Output
    - Result<Vec<String>, String> notes (not active, runbook not enforced)
    - Error if a Flow or Zone does not exist
*/
pub(super) fn check_target(repo: &Repo, target: &Target) -> Result<Vec<String>, String> {
    if target.kind == TargetKind::Runbook {
        return Ok(vec![format!(
            "{} is recorded for runbooks; the runbook runtime does not apply Bricks yet",
            target.key()
        )]);
    }
    let all = targets(repo)?;
    match all.get(&target.key()) {
        None => Err(format!(
            "{} is not a Zone or Flow of .crane/governance.yaml",
            target.key()
        )),
        Some(info) if info.status != Lifecycle::Active => Ok(vec![format!(
            "{} is {:?}; its Bricks apply once it is active",
            target.key(),
            info.status
        )
        .to_ascii_lowercase()]),
        Some(_) => Ok(Vec::new()),
    }
}

/** Read text from --KEY or --KEY-file
 * Input
    - args: &Args - parsed arguments
    - text_key: &str - inline key
    - file_key: &str - file key
 * Output
    - Result<Option<String>, String>
*/
pub(super) fn text_option(
    args: &Args,
    text_key: &str,
    file_key: &str,
) -> Result<Option<String>, String> {
    match (args.value(text_key), args.value(file_key)) {
        (Some(_), Some(_)) => Err(format!("give --{text_key} or --{file_key}, not both")),
        (Some(text), None) => Ok(Some(text)),
        (None, Some(path)) => std::fs::read_to_string(&path)
            .map(Some)
            .map_err(|error| format!("could not read {path}: {error}")),
        (None, None) => Ok(None),
    }
}

/** A Brick as a person names it: its draft, or an activated version
 * Variants
    - Draft(BrickDraft) - the unsigned draft
    - Version(BrickEntry) - an activated version
*/
pub(super) enum Found {
    Draft(BrickDraft),
    Version(BrickEntry),
}

/** Find a Brick: "ID@N" is that activated version; "ID" is the draft, else the newest version
 * Input
    - repo: &Repo - repository
    - reference: &str - reference
 * Output
    - Result<Found, String>
*/
pub(super) fn find(repo: &Repo, reference: &str) -> Result<Found, String> {
    let (id, version) = crate::cli::protocol::versioned(reference)?;
    if let Some(version) = version {
        return repo
            .store
            .brick(&crate::brick::model::BrickRef {
                brick_id: id.clone(),
                version,
            })
            .cloned()
            .map(Found::Version)
            .ok_or_else(|| format!("{id}@{version} is not an activated Brick version"));
    }
    if let Some(draft) = store::brick_draft(&repo.workspace, &id)? {
        return Ok(Found::Draft(draft));
    }
    repo.store
        .versions(&id)
        .last()
        .map(|entry| Found::Version((*entry).clone()))
        .ok_or_else(|| format!("no Brick '{id}' (neither a draft nor an activated version)"))
}

/** Refuse to sign when the store does not verify (commit() refuses too; this says why first)
 * Input
    - repo: &Repo - repository
 * Output
    - Result<(), String>
*/
pub(super) fn require_verified(repo: &Repo) -> Result<(), String> {
    match &repo.trust {
        Trust::Unverified(reason) => Err(format!(
            "the Brick store cannot be changed while {reason}; run 'crane validate'"
        )),
        _ => Ok(()),
    }
}

/** The trust state as a word
 * Input
    - trust: &Trust - trust
 * Output
    - String
*/
pub(super) fn trust_word(trust: &Trust) -> String {
    match trust {
        Trust::Absent => "absent".into(),
        Trust::Verified => "verified".into(),
        Trust::Unverified(reason) => format!("unverified: {reason}"),
    }
}
