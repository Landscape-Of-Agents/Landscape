// The shared skill store (.crane/skills.json). The capability registry lives in each machine's
// trust directory, so a publication or a revocation there reached no other machine. The shared
// store carries every published version's exact definition, its digest, and its state (published,
// deprecated, revoked) in the repository, covered by the signed manifest and written only through
// governance::state::commit() by an admin running 'crane capability publish|deprecate|revoke'.
//
// It only restricts: invocation preflight treats it as authoritative for withdrawal (a version
// deprecated or revoked in the store, or published there with another digest, is blocked on every
// machine that pulled it), and a store that fails verification blocks every invocation (fail
// closed). It never makes a version eligible: local publication, validation, the lab, and the rest
// of preflight still decide that. A version missing from the store is reported, not blocked, so
// publications made before the store existed keep working until they are published again.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::CapabilityDefinition;
use crate::governance::state::{commit, integrity, State, SKILLS_FILE};
use crate::governance::workspace::Workspace;

/** Version of the shared store layout */
pub(crate) const SHARED_STORE_VERSION: u32 = 1;

/** Audit action of a shared publication */
const SKILL_PUBLISHED: &str = "skill.published";

/** Audit action of a shared withdrawal */
const SKILL_WITHDRAWN: &str = "skill.withdrawn";

/** State of a version in the shared store
 * Variants
    - Published - eligible as far as the store is concerned
    - Deprecated - kept for history; no new invocations
    - Revoked - withdrawn; no new invocations
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SharedState {
    Published,
    Deprecated,
    Revoked,
}

/** One version in the shared store
 * Fields
    - capability_id: String - capability
    - version: u32 - version
    - digest: String - definition digest
    - state: SharedState - state
    - definition: CapabilityDefinition - the exact published definition
    - published_by: String - admin who published it
    - published_at: u64 - Unix milliseconds
    - reason: String - why it was published
    - changed_by: Option<String> - admin who withdrew it
    - changed_at: Option<u64> - Unix milliseconds
    - change_reason: Option<String> - why it was withdrawn
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SharedEntry {
    pub(crate) capability_id: String,
    pub(crate) version: u32,
    pub(crate) digest: String,
    pub(crate) state: SharedState,
    pub(crate) definition: CapabilityDefinition,
    pub(crate) published_by: String,
    pub(crate) published_at: u64,
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) changed_by: Option<String>,
    #[serde(default)]
    pub(crate) changed_at: Option<u64>,
    #[serde(default)]
    pub(crate) change_reason: Option<String>,
}

/** The content of .crane/skills.json
 * Fields
    - version: u32 - SHARED_STORE_VERSION
    - skills: Vec<SharedEntry> - versions, ordered by capability and version
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SharedStore {
    pub(crate) version: u32,
    pub(crate) skills: Vec<SharedEntry>,
}

impl SharedStore {
    /** Parse the store held by a governance state (empty when there is none)
     * Input
        - state: &State - governance state
     * Output
        - Result<SharedStore, String>
        - Error if the store is malformed or of another layout version
    */
    pub(crate) fn of(state: &State) -> Result<Self, String> {
        let Some(value) = &state.skills else {
            return Ok(Self {
                version: SHARED_STORE_VERSION,
                skills: Vec::new(),
            });
        };
        let store: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!(".crane/{SKILLS_FILE} is malformed: {error}"))?;
        if store.version != SHARED_STORE_VERSION {
            return Err(format!(
                ".crane/{SKILLS_FILE} has layout version {}; this Crane reads {SHARED_STORE_VERSION}",
                store.version
            ));
        }
        Ok(store)
    }
}

/** Record a local publication in the shared store, signed through commit()
 * Input
    - workspace: &Workspace - repository
    - definition: &CapabilityDefinition - the published definition
    - actor: &str - admin publishing it
    - reason: &str - publication reason
    - at: u64 - Unix milliseconds
 * Output
    - Result<(), String>
    - Error if this machine is not an admin, the state does not verify, or the store is malformed
*/
pub(crate) fn share_publication(
    workspace: &Workspace,
    definition: &CapabilityDefinition,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<(), String> {
    let digest = definition.digest();
    commit(
        workspace,
        "crane capability publish",
        SKILL_PUBLISHED,
        json!({"capability_id": definition.capability_id, "version": definition.version, "digest": digest}),
        |state, _| {
            let mut store = SharedStore::of(state)?;
            store.skills.retain(|entry| {
                !(entry.capability_id == definition.capability_id
                    && entry.version == definition.version)
            });
            store.skills.push(SharedEntry {
                capability_id: definition.capability_id.clone(),
                version: definition.version,
                digest: digest.clone(),
                state: SharedState::Published,
                definition: definition.clone(),
                published_by: actor.into(),
                published_at: at,
                reason: reason.into(),
                changed_by: None,
                changed_at: None,
                change_reason: None,
            });
            store
                .skills
                .sort_by(|a, b| (&a.capability_id, a.version).cmp(&(&b.capability_id, b.version)));
            state.skills = Some(serde_json::to_value(&store).map_err(|error| error.to_string())?);
            Ok(())
        },
    )?;
    Ok(())
}

/** Record a local deprecation or revocation in the shared store, signed through commit()
 * Input
    - workspace: &Workspace - repository
    - capability_id: &str - capability
    - versions: &[u32] - versions withdrawn locally
    - to: SharedState - Deprecated or Revoked
    - actor: &str - admin withdrawing them
    - reason: &str - reason
    - at: u64 - Unix milliseconds
 * Output
    - Result<usize, String> - versions changed in the store (0: none were shared, nothing signed)
*/
pub(crate) fn share_withdrawal(
    workspace: &Workspace,
    capability_id: &str,
    versions: &[u32],
    to: SharedState,
    actor: &str,
    reason: &str,
    at: u64,
) -> Result<usize, String> {
    let shared = SharedStore::of(&State::load(workspace)?)?;
    let affected = shared
        .skills
        .iter()
        .filter(|entry| {
            entry.capability_id == capability_id
                && versions.contains(&entry.version)
                && entry.state != to
                && entry.state != SharedState::Revoked
        })
        .count();
    if affected == 0 {
        return Ok(0);
    }
    commit(
        workspace,
        "crane capability withdraw",
        SKILL_WITHDRAWN,
        json!({"capability_id": capability_id, "versions": versions, "to": to}),
        |state, _| {
            let mut store = SharedStore::of(state)?;
            for entry in store.skills.iter_mut().filter(|entry| {
                entry.capability_id == capability_id
                    && versions.contains(&entry.version)
                    && entry.state != SharedState::Revoked
            }) {
                entry.state = to;
                entry.changed_by = Some(actor.into());
                entry.changed_at = Some(at);
                entry.change_reason = Some(reason.into());
            }
            state.skills = Some(serde_json::to_value(&store).map_err(|error| error.to_string())?);
            Ok(())
        },
    )?;
    Ok(affected)
}

/** A version's state and digest as the verified store records it
 * Fields
    - state: SharedState - state
    - digest: String - definition digest
    - changed_by: Option<String> - who withdrew it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SharedSummary {
    pub(crate) state: SharedState,
    pub(crate) digest: String,
    pub(crate) changed_by: Option<String>,
}

/** What invocation preflight may rely on from the shared store
 * Variants
    - Absent - the repository has no shared store
    - Verified(entries) - the store verified with the signed state; "ID@vN" to its summary
    - Unverified(reason) - the store, or the state signing it, failed verification
*/
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum SharedView {
    #[default]
    Absent,
    Verified(BTreeMap<String, SharedSummary>),
    Unverified(String),
}

/** The view of a repository without a shared store (fixtures and demonstrations) */
pub(crate) static NO_SHARED_STORE: SharedView = SharedView::Absent;

/** The key of a version in a shared view
 * Input
    - capability_id: &str - capability
    - version: u32 - version
 * Output
    - String
*/
pub(crate) fn key(capability_id: &str, version: u32) -> String {
    format!("{capability_id}@v{version}")
}

/** Read the shared store and verify it with the signed state (signature, rollback anchor, and
 * every covered digest)
 * Input
    - workspace: &Workspace - repository
 * Output
    - SharedView
*/
pub(crate) fn view(workspace: &Workspace) -> SharedView {
    let state = match State::load(workspace) {
        Ok(state) => state,
        Err(error) => return SharedView::Unverified(error),
    };
    if state.skills.is_none() {
        return SharedView::Absent;
    }
    let blocking = workspace
        .trust()
        .map(|trust| {
            integrity(workspace, &state, &trust)
                .into_iter()
                .filter(crate::governance::Finding::blocking)
                .map(|finding| finding.code)
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|error| vec![error]);
    if !blocking.is_empty() {
        return SharedView::Unverified(format!(
            "the signed state does not verify ({})",
            blocking.join(", ")
        ));
    }
    match SharedStore::of(&state) {
        Ok(store) => SharedView::Verified(
            store
                .skills
                .into_iter()
                .map(|entry| {
                    (
                        key(&entry.capability_id, entry.version),
                        SharedSummary {
                            state: entry.state,
                            digest: entry.digest,
                            changed_by: entry.changed_by,
                        },
                    )
                })
                .collect(),
        ),
        Err(error) => SharedView::Unverified(error),
    }
}

/** Describe a store entry for inspection output
 * Input
    - entry: &SharedEntry - entry
 * Output
    - Value
*/
pub(crate) fn summary(entry: &SharedEntry) -> Value {
    json!({
        "capability_id": entry.capability_id,
        "version": entry.version,
        "digest": entry.digest,
        "state": entry.state,
        "published_by": entry.published_by,
        "changed_by": entry.changed_by,
        "change_reason": entry.change_reason,
    })
}
