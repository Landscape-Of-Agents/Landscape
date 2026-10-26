// Where Bricks live. Two places, one authority:
//   - .crane/bricks.json, the signed Brick store: activated (immutable) Brick and Stack versions,
//     attachments with their Current Context, and agent assignments. It is covered by the signed
//     manifest (bricks_digest) and written only through governance::state::commit() by an admin, so
//     every machine that pulls the repository enforces the same Bricks, and an edit nobody signed
//     fails integrity (which already blocks every agent change).
//   - .crane/drafts/{bricks,stacks}/ID.json, unsigned drafts. A draft grants nothing: nothing reads
//     it for enforcement or context, and activation always validates it again instead of trusting
//     the result stored in it. Agents cannot write either place (the hooks deny .crane).
// Runtime observations (snapshots, deliveries, monitor results) live in the trust directory's
// runtime folder and in the event store, never here.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{
    Assignment, Attachment, Author, Brick, BrickEntry, BrickRef, Stack, StackEntry, Validation,
};
use crate::governance::state::{commit, integrity, State, BRICKS_FILE};
use crate::governance::workspace::Workspace;
use crate::platform::files::write_atomic;
use crate::platform::io_error;

/** Version of the store layout */
pub(crate) const STORE_VERSION: u32 = 1;

/** The content of .crane/bricks.json
 * Fields
    - version: u32 - STORE_VERSION
    - bricks: Vec<BrickEntry> - activated Brick versions, by id and version
    - stacks: Vec<StackEntry> - activated Stack versions, by id and version
    - attachments: Vec<Attachment> - attachments, by id
    - assignments: Vec<Assignment> - agent assignments, by agent
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrickStore {
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) bricks: Vec<BrickEntry>,
    #[serde(default)]
    pub(crate) stacks: Vec<StackEntry>,
    #[serde(default)]
    pub(crate) attachments: Vec<Attachment>,
    #[serde(default)]
    pub(crate) assignments: Vec<Assignment>,
}

impl Default for BrickStore {
    /** An empty store of the current layout
     * Input
        - None
     * Output
        - BrickStore
    */
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            bricks: Vec::new(),
            stacks: Vec::new(),
            attachments: Vec::new(),
            assignments: Vec::new(),
        }
    }
}

impl BrickStore {
    /** Parse the store a governance state holds (empty when there is none)
     * Input
        - state: &State - governance state
     * Output
        - Result<BrickStore, String>
        - Error if the store is malformed or of another layout version
    */
    pub(crate) fn of(state: &State) -> Result<Self, String> {
        let Some(value) = &state.bricks else {
            return Ok(Self::default());
        };
        let store: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!(".crane/{BRICKS_FILE} is malformed: {error}"))?;
        if store.version != STORE_VERSION {
            return Err(format!(
                ".crane/{BRICKS_FILE} has layout version {}; this Crane reads {STORE_VERSION}",
                store.version
            ));
        }
        Ok(store)
    }

    /** Put the store into a governance state, in canonical order
     * Input
        - state: &mut State - state being committed
     * Output
        - Result<(), String>
    */
    pub(crate) fn save_into(mut self, state: &mut State) -> Result<(), String> {
        self.bricks.sort_by(|a, b| {
            (&a.brick.brick_id, a.brick.version).cmp(&(&b.brick.brick_id, b.brick.version))
        });
        self.stacks.sort_by(|a, b| {
            (&a.stack.stack_id, a.stack.version).cmp(&(&b.stack.stack_id, b.stack.version))
        });
        self.attachments
            .sort_by(|a, b| a.attachment_id.cmp(&b.attachment_id));
        self.assignments.sort_by(|a, b| a.agent.cmp(&b.agent));
        state.bricks = Some(serde_json::to_value(&self).map_err(io_error)?);
        Ok(())
    }

    /** Find an activated Brick version
     * Input
        - reference: &BrickRef - exact version
     * Output
        - Option<&BrickEntry>
    */
    pub(crate) fn brick(&self, reference: &BrickRef) -> Option<&BrickEntry> {
        self.bricks.iter().find(|entry| {
            entry.brick.brick_id == reference.brick_id && entry.brick.version == reference.version
        })
    }

    /** Every activated version of a Brick, oldest first
     * Input
        - brick_id: &str - Brick
     * Output
        - Vec<&BrickEntry>
    */
    pub(crate) fn versions(&self, brick_id: &str) -> Vec<&BrickEntry> {
        self.bricks
            .iter()
            .filter(|entry| entry.brick.brick_id == brick_id)
            .collect()
    }

    /** Find an activated Stack version
     * Input
        - stack_id: &str - Stack
        - version: u32 - version
     * Output
        - Option<&StackEntry>
    */
    pub(crate) fn stack(&self, stack_id: &str, version: u32) -> Option<&StackEntry> {
        self.stacks
            .iter()
            .find(|entry| entry.stack.stack_id == stack_id && entry.stack.version == version)
    }

    /** The next version number of a Brick or Stack id (activated versions and the draft considered)
     * Input
        - id: &str - identifier
        - stack: bool - a Stack id
     * Output
        - u32
    */
    pub(crate) fn next_version(&self, id: &str, stack: bool) -> u32 {
        let highest = if stack {
            self.stacks
                .iter()
                .filter(|entry| entry.stack.stack_id == id)
                .map(|entry| entry.stack.version)
                .max()
        } else {
            self.versions(id)
                .iter()
                .map(|entry| entry.brick.version)
                .max()
        };
        highest.unwrap_or(0) + 1
    }
}

/** Whether the signed state vouches for the store
 * Variants
    - Absent - the repository has no Brick store
    - Verified - the store verified with the signed state
    - Unverified(reason) - the store or the state signing it failed verification
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Trust {
    Absent,
    Verified,
    Unverified(String),
}

/** Load the store and whether the signed state vouches for it (signature, rollback anchor, and
 * every covered digest)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<(State, BrickStore, Trust), String>
    - Error if the state cannot be read or the store is malformed
*/
pub(crate) fn load(workspace: &Workspace) -> Result<(State, BrickStore, Trust), String> {
    let state = State::load(workspace)?;
    let store = BrickStore::of(&state)?;
    let trust = trust_of(workspace, &state);
    Ok((state, store, trust))
}

/** Decide whether the signed state vouches for the store of a loaded state
 * Input
    - workspace: &Workspace - repository
    - state: &State - loaded state
 * Output
    - Trust
*/
pub(crate) fn trust_of(workspace: &Workspace, state: &State) -> Trust {
    if state.bricks.is_none() {
        return Trust::Absent;
    }
    let blocking = workspace
        .trust()
        .map(|trust| {
            integrity(workspace, state, &trust)
                .into_iter()
                .filter(crate::governance::Finding::blocking)
                .map(|finding| finding.code)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|error| vec![error]);
    if blocking.is_empty() {
        Trust::Verified
    } else {
        Trust::Unverified(format!(
            "the signed state does not verify ({})",
            blocking.join(", ")
        ))
    }
}

/** Change the signed store through commit(): verify, mutate, audit, sign
 * Input
    - workspace: &Workspace - repository
    - command: &str - command, for the agent refusal
    - action: &str - audit action, such as "brick.activated"
    - detail: Value - audit detail (identifiers and digests, never context text)
    - change: impl FnOnce(&mut BrickStore, &State) -> Result<(), String> - the change
 * Output
    - Result<BrickStore, String> the committed store
    - Error if run by an agent, the machine is not an admin, the state does not verify, or the
      change fails
*/
pub(crate) fn change(
    workspace: &Workspace,
    command: &str,
    action: &str,
    detail: Value,
    change: impl FnOnce(&mut BrickStore, &State) -> Result<(), String>,
) -> Result<BrickStore, String> {
    let mut committed = None;
    commit(workspace, command, action, detail, |state, _| {
        let mut store = BrickStore::of(state)?;
        change(&mut store, state)?;
        committed = Some(store.clone());
        store.save_into(state)
    })?;
    committed.ok_or_else(|| "the Brick store was not changed".into())
}

/** A Brick draft (unsigned; grants nothing)
 * Fields
    - brick: Brick - definition
    - validation: Validation - the last validation (informational; activation validates again)
    - updated_by: Author - who saved it last
    - updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BrickDraft {
    pub(crate) brick: Brick,
    #[serde(default)]
    pub(crate) validation: Validation,
    pub(crate) updated_by: Author,
    pub(crate) updated_at: u64,
}

/** A Stack draft (unsigned; grants nothing)
 * Fields
    - stack: Stack - definition
    - updated_by: Author - who saved it last
    - updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StackDraft {
    pub(crate) stack: Stack,
    pub(crate) updated_by: Author,
    pub(crate) updated_at: u64,
}

/** The directory of drafts of one kind
 * Input
    - workspace: &Workspace - repository
    - kind: &str - "bricks" or "stacks"
 * Output
    - PathBuf
*/
fn drafts_dir(workspace: &Workspace, kind: &str) -> PathBuf {
    workspace.crane.join("drafts").join(kind)
}

/** The file of a draft
 * Input
    - workspace: &Workspace - repository
    - kind: &str - "bricks" or "stacks"
    - id: &str - identifier (validated as a name)
 * Output
    - Result<PathBuf, String>
*/
fn draft_file(workspace: &Workspace, kind: &str, id: &str) -> Result<PathBuf, String> {
    crate::platform::validate_name("Brick", id)?;
    Ok(drafts_dir(workspace, kind).join(format!("{id}.json")))
}

/** Read a draft
 * Input
    - workspace: &Workspace - repository
    - kind: &str - "bricks" or "stacks"
    - id: &str - identifier
 * Output
    - Result<Option<T>, String>
*/
fn read_draft<T: for<'de> Deserialize<'de>>(
    workspace: &Workspace,
    kind: &str,
    id: &str,
) -> Result<Option<T>, String> {
    let path = draft_file(workspace, kind, id)?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{} is malformed: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/** Write a draft atomically
 * Input
    - workspace: &Workspace - repository
    - kind: &str - "bricks" or "stacks"
    - id: &str - identifier
    - draft: &impl Serialize - draft
 * Output
    - Result<(), String>
*/
fn write_draft(
    workspace: &Workspace,
    kind: &str,
    id: &str,
    draft: &impl Serialize,
) -> Result<(), String> {
    crate::trust::require_human("crane brick")?;
    let path = draft_file(workspace, kind, id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(io_error)?;
    }
    let text = serde_json::to_string_pretty(draft).map_err(io_error)? + "\n";
    write_atomic(&path, text.as_bytes())
}

/** Read a Brick draft
 * Input
    - workspace: &Workspace - repository
    - id: &str - Brick id
 * Output
    - Result<Option<BrickDraft>, String>
*/
pub(crate) fn brick_draft(workspace: &Workspace, id: &str) -> Result<Option<BrickDraft>, String> {
    read_draft(workspace, "bricks", id)
}

/** Save a Brick draft
 * Input
    - workspace: &Workspace - repository
    - draft: &BrickDraft - draft
 * Output
    - Result<(), String>
*/
pub(crate) fn save_brick_draft(workspace: &Workspace, draft: &BrickDraft) -> Result<(), String> {
    write_draft(workspace, "bricks", &draft.brick.brick_id, draft)
}

/** Read a Stack draft
 * Input
    - workspace: &Workspace - repository
    - id: &str - Stack id
 * Output
    - Result<Option<StackDraft>, String>
*/
pub(crate) fn stack_draft(workspace: &Workspace, id: &str) -> Result<Option<StackDraft>, String> {
    read_draft(workspace, "stacks", id)
}

/** Save a Stack draft
 * Input
    - workspace: &Workspace - repository
    - draft: &StackDraft - draft
 * Output
    - Result<(), String>
*/
pub(crate) fn save_stack_draft(workspace: &Workspace, draft: &StackDraft) -> Result<(), String> {
    write_draft(workspace, "stacks", &draft.stack.stack_id, draft)
}

/** Remove a draft after it was activated (best effort: a leftover draft grants nothing)
 * Input
    - workspace: &Workspace - repository
    - kind: &str - "bricks" or "stacks"
    - id: &str - identifier
 * Output
    - None
*/
pub(crate) fn remove_draft(workspace: &Workspace, kind: &str, id: &str) {
    if let Ok(path) = draft_file(workspace, kind, id) {
        let _ = fs::remove_file(path);
    }
}

/** Every draft of a kind, by id (unreadable drafts are reported, not skipped silently)
 * Input
    - workspace: &Workspace - repository
    - kind: &str - "bricks" or "stacks"
 * Output
    - (Vec<T>, Vec<String>) drafts and problems
*/
fn drafts<T: for<'de> Deserialize<'de>>(
    workspace: &Workspace,
    kind: &str,
) -> (Vec<T>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = fs::read_dir(drafts_dir(workspace, kind)) else {
        return (found, problems);
    };
    let mut names = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    names.sort();
    for path in names {
        match fs::read_to_string(&path)
            .map_err(io_error)
            .and_then(|text| serde_json::from_str::<T>(&text).map_err(io_error))
        {
            Ok(draft) => found.push(draft),
            Err(error) => problems.push(format!("{}: {error}", path.display())),
        }
    }
    (found, problems)
}

/** Every Brick draft
 * Input
    - workspace: &Workspace - repository
 * Output
    - (Vec<BrickDraft>, Vec<String>) drafts and unreadable files
*/
pub(crate) fn brick_drafts(workspace: &Workspace) -> (Vec<BrickDraft>, Vec<String>) {
    drafts(workspace, "bricks")
}

/** Every Stack draft
 * Input
    - workspace: &Workspace - repository
 * Output
    - (Vec<StackDraft>, Vec<String>) drafts and unreadable files
*/
pub(crate) fn stack_drafts(workspace: &Workspace) -> (Vec<StackDraft>, Vec<String>) {
    drafts(workspace, "stacks")
}

/** The person running Crane, with this machine's signing key when it has one
 * Input
    - workspace: &Workspace - repository
 * Output
    - Author
*/
pub(crate) fn author(workspace: &Workspace) -> Author {
    Author {
        name: crate::platform::actor(),
        key: workspace
            .trust()
            .ok()
            .and_then(|trust| trust.signing_public_key().ok().flatten()),
    }
}

/** Build an unsaved Brick definition template for a type
 * Input
    - brick_id: &str - identifier
    - version: u32 - version
    - brick_type: super::types::BrickType - type
    - name: &str - name
    - by: Author - author
 * Output
    - Brick
*/
pub(crate) fn new_brick(
    brick_id: &str,
    version: u32,
    brick_type: super::types::BrickType,
    name: &str,
    by: Author,
) -> Brick {
    Brick {
        brick_id: brick_id.into(),
        version,
        brick_type,
        name: name.into(),
        description: String::new(),
        intent: String::new(),
        owner: None,
        departments: Vec::new(),
        config: Value::Object(serde_json::Map::new()),
        common_context: Default::default(),
        dependencies: Vec::new(),
        created_by: by,
        created_at: crate::platform::now_millis(),
    }
}
