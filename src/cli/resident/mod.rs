// crane resident: long-lived agent assignments that stay idle (no model, no process) until a
// trigger or timer wakes them. The commands only parse and print; every rule is in src/resident.
// Who may do what: anyone running Crane (a person; agents never run it) drafts, validates, tests,
// pauses, revokes, cancels, and answers; activating a definition or an assignment, renewing it,
// deciding an approval, and reviewing an RCA graph need the repository owner's or an admin's key.
// A resident only ever acts inside an active signed Brick assignment ('crane brick assign').
// Every command takes --format json (the shared envelope of cli::protocol). One file per area:
// registry (definitions, assignments, RCA graphs) and activity (triggers, ticks, tests,
// activations, and people's decisions).

mod activity;
mod registry;

use crate::brick::model::Author;
use crate::governance::workspace::Workspace;
use crate::resident::store::ResidentStore;

/** The command reference of 'crane resident' */
const USAGE: &str = "usage: crane resident catalog
       crane resident definition save --file DEFINITION.json [--dry-run]
       crane resident definition activate|revoke ID@N --reason TEXT
       crane resident definition list | show ID[@N]
       crane resident create --file ASSIGNMENT.json [--dry-run]
       crane resident duplicate ID --as NEW_ID [--target flow:ID|zone:ID]
       crane resident validate ID
       crane resident activate|renew ID --reason TEXT [--dry-run]
       crane resident pause|resume|revoke ID --reason TEXT
       crane resident list | show ID | context ID | status
       crane resident test ID (--event EVENT.json | --replay ACTIVATION | --replay-event SEQUENCE)
       crane resident trigger --file EVENT.json
       crane resident tick
       crane resident activations [--assignment ID] | activation ACTIVATION
       crane resident approve ACTIVATION --step STEP --reason TEXT [--deny]
       crane resident answer ACTIVATION --step STEP --text TEXT
       crane resident reconcile ACTIVATION --step STEP --happened yes|no --reason TEXT
       crane resident cancel ACTIVATION --reason TEXT
       crane resident rca save --file GRAPH.json
       crane resident rca review ID@N (--accept | --reject) --reason TEXT
       crane resident rca list | show ID[@N]
  (activation, renewal, approvals, and RCA review need an admin key; every command takes --format json)";

/** Dispatch a resident command
 * Input
    - args: &[String] - arguments after "resident"
 * Output
    - Result<(), String>
*/
pub(crate) fn resident(args: &[String]) -> Result<(), String> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let rest = |count: usize| args.get(count..).unwrap_or_default();
    match words.as_slice() {
        ["catalog", ..] => registry::catalog(rest(1)),
        ["definition", "save", ..] => registry::save_definition(rest(2)),
        ["definition", "activate", ..] => registry::change_definition(rest(2), true),
        ["definition", "revoke", ..] => registry::change_definition(rest(2), false),
        ["definition", "list", ..] => registry::definitions(rest(2)),
        ["definition", "show", ..] => registry::show_definition(rest(2)),
        ["create", ..] => registry::create(rest(1)),
        ["duplicate", ..] => registry::duplicate(rest(1)),
        ["validate", ..] => registry::validate(rest(1)),
        ["activate", ..] => registry::lifecycle(rest(1), "activate"),
        ["renew", ..] => registry::lifecycle(rest(1), "renew"),
        ["pause", ..] => registry::lifecycle(rest(1), "pause"),
        ["resume", ..] => registry::lifecycle(rest(1), "resume"),
        ["revoke", ..] => registry::lifecycle(rest(1), "revoke"),
        ["list", ..] => registry::list(rest(1)),
        ["show", ..] => registry::show(rest(1)),
        ["context", ..] => activity::context(rest(1)),
        ["status", ..] => activity::status(rest(1)),
        ["test", ..] => activity::test(rest(1)),
        ["trigger", ..] => activity::trigger(rest(1)),
        ["tick", ..] => activity::tick(rest(1)),
        ["activations", ..] => activity::activations(rest(1)),
        ["activation", ..] => activity::activation(rest(1)),
        ["approve", ..] => activity::approve(rest(1)),
        ["answer", ..] => activity::answer(rest(1)),
        ["reconcile", ..] => activity::reconcile(rest(1)),
        ["cancel", ..] => activity::cancel(rest(1)),
        ["rca", "save", ..] => registry::save_rca(rest(2)),
        ["rca", "review", ..] => registry::review_rca(rest(2)),
        ["rca", "list", ..] => registry::rcas(rest(2)),
        ["rca", "show", ..] => registry::show_rca(rest(2)),
        _ => Err(USAGE.into()),
    }
}

/** The repository as the resident commands see it
 * Fields
    - workspace: Workspace - repository
    - runtime: std::path::PathBuf - runtime directory in the trust directory
    - store: ResidentStore - resident store
    - author: Author - the person running the command
*/
pub(super) struct Repo {
    pub(super) workspace: Workspace,
    pub(super) runtime: std::path::PathBuf,
    pub(super) store: ResidentStore,
    pub(super) author: Author,
}

/** Open the repository and its resident store
 * Input
    - None
 * Output
    - Result<Repo, String>
*/
pub(super) fn open() -> Result<Repo, String> {
    let workspace = Workspace::locate()?;
    let runtime = workspace.trust()?.runtime();
    let store = ResidentStore::open(&runtime)?;
    let author = crate::brick::store::author(&workspace);
    Ok(Repo {
        workspace,
        runtime,
        store,
        author,
    })
}

/** The public keys allowed to sign the repository's rules (owner and admins)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Vec<String>
*/
pub(super) fn signers(workspace: &Workspace) -> Vec<String> {
    crate::governance::state::State::load(workspace)
        .map(|state| crate::governance::admins::signers(&state))
        .unwrap_or_default()
}

/** Refuse a person whose key is not a signer of the repository
 * Input
    - repo: &Repo - repository
    - what: &str - what they tried
 * Output
    - Result<(), String>
*/
pub(super) fn require_signer(repo: &Repo, what: &str) -> Result<(), String> {
    let allowed = repo
        .author
        .key
        .as_ref()
        .is_some_and(|key| signers(&repo.workspace).contains(key));
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "{what} needs the repository owner's or an admin's signing key ('crane admin ...')"
        ))
    }
}

/** Read a JSON file
 * Input
    - path: &str - file
 * Output
    - Result<serde_json::Value, String>
*/
pub(super) fn read_json(path: &str) -> Result<serde_json::Value, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("could not read {path}: {error}"))?;
    if text.len() > 512 * 1024 {
        return Err(format!("{path} is larger than 512 KiB"));
    }
    serde_json::from_str(&text).map_err(|error| format!("{path} is not valid JSON: {error}"))
}

/** Read a required, non-empty --reason
 * Input
    - args: &crate::cli::args::Args - parsed arguments
 * Output
    - Result<String, String>
*/
pub(super) fn reason(args: &crate::cli::args::Args) -> Result<String, String> {
    args.value("reason")
        .filter(|reason| !reason.trim().is_empty())
        .ok_or_else(|| "--reason is required".into())
}

/** Split "ID@N" into an id and a version (the latest when it is absent)
 * Input
    - text: &str - reference
 * Output
    - Result<(String, Option<u32>), String>
*/
pub(super) fn versioned(text: &str) -> Result<(String, Option<u32>), String> {
    crate::cli::protocol::versioned(text)
}
