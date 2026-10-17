// Rebinding a repository that moved ('crane repo rebind OLD_REMOTE'), for example after a transfer
// to an organization or a rename. A repository's identity is computed from its root commits and its
// origin remote, and everything Crane signs is bound to it: the manifest, every selection record,
// and the owner's admin list. This machine's trust data (pinned keys, rollback anchor, evidence,
// integration settings, and the signing key) is kept under the identity too.
//
// Rebinding proves the move instead of trusting it: the old remote must reproduce the identity the
// state was signed for (same root commits, so the same history), and the signed state must verify
// completely under that old identity. Only then is this machine's trust data carried over, and, on
// the owner's machine, every record, the admin list, and the manifest are signed again for the new
// identity. Teammates run the same command after the owner commits the re-signed .crane; for them it
// only carries their trust data over.

use std::fs;
use std::path::Path;

use serde_json::json;

use super::admins::AdminStatement;
use super::state::{commit, integrity, State, REPOSITORY_REBOUND};
use super::workspace::{identity, Workspace};
use super::Severity;
use crate::platform::git;
use crate::selection::registry::binding_digest;
use crate::trust::{require_human, TrustStore};

/** What a rebind did
 * Fields
    - from: String - the old normalized remote
    - to: String - the new normalized remote
    - copied: usize - trust files carried over to the new identity
    - resigned: Option<usize> - selections signed again (None when this machine only carried
      its trust data over)
    - generation: Option<u64> - the new registry generation when the state was signed again
*/
pub(crate) struct Rebound {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) copied: usize,
    pub(crate) resigned: Option<usize>,
    pub(crate) generation: Option<u64>,
}

/** Copy every file of a directory tree into another, keeping files that already exist there (lock
 * files are skipped)
 * Input
    - from: &Path - source directory
    - to: &Path - destination directory
 * Output
    - Result<usize, String> - files copied
    - Error if a file cannot be read or written
*/
fn copy_missing(from: &Path, to: &Path) -> Result<usize, String> {
    let Ok(entries) = fs::read_dir(from) else {
        return Ok(0);
    };
    fs::create_dir_all(to)
        .map_err(|error| format!("could not create {}: {error}", to.display()))?;
    let mut copied = 0;
    for entry in entries.flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() {
            copied += copy_missing(&source, &target)?;
        } else if !target.exists()
            && source
                .extension()
                .is_none_or(|extension| extension != "lock")
        {
            fs::copy(&source, &target).map_err(|error| {
                format!(
                    "could not copy {} to {}: {error}",
                    source.display(),
                    target.display()
                )
            })?;
            copied += 1;
        }
    }
    Ok(copied)
}

/** Rebind the repository to its current origin remote after a move
 * Input
    - workspace: &Workspace - the repository as it is now (new remote)
    - old_remote: &str - the remote the state was signed for, in any form Git accepts
 * Output
    - Result<Rebound, String>
    - Error if run by an agent, the old remote does not reproduce the signed identity, the state
      does not verify under it, or this machine cannot sign the change it needs
*/
pub(crate) fn rebind(workspace: &Workspace, old_remote: &str) -> Result<Rebound, String> {
    require_human("crane repo rebind")?;
    let from = git::normalize_remote(old_remote)
        .ok_or_else(|| format!("'{old_remote}' is not a remote Crane understands (use the old GitHub URL, for example https://github.com/OWNER/REPO)"))?;
    let to = workspace.remote.clone().ok_or(
        "this clone has no origin remote; point it at the new repository first ('git remote set-url origin URL')",
    )?;
    if from == to {
        return Err(format!(
            "the old remote is the current one ({to}); point origin at the new repository first ('git remote set-url origin URL')"
        ));
    }
    // The same history under the old remote must give the identity the state was signed for
    let old_id = identity(&workspace.root_commits, Some(&from));
    let state = State::load(workspace)?;
    let manifest = state
        .registry
        .manifest
        .as_ref()
        .ok_or("this repository has no signed Crane state to rebind; run 'crane init'")?;
    if manifest.repository_id != old_id && manifest.repository_id != workspace.repository_id {
        return Err(format!(
            ".crane was not signed for {from} with this history; check the old remote (Crane rebinds only a repository that moved, never a copied .crane)"
        ));
    }
    let old_trust = TrustStore::open(&old_id)?;
    let new_trust = workspace.trust()?;
    let mut copied = copy_missing(&old_trust.repo_dir, &new_trust.repo_dir)?;
    if old_trust.key_path.exists() && !new_trust.key_path.exists() {
        if let Some(parent) = new_trust.key_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        fs::copy(&old_trust.key_path, &new_trust.key_path)
            .map_err(|error| format!("could not carry the signing key over: {error}"))?;
        copied += 1;
    }
    // Already rebound by the owner (a teammate's machine): carrying the trust data over is all
    if manifest.repository_id == workspace.repository_id {
        return Ok(Rebound {
            from,
            to,
            copied,
            resigned: None,
            generation: None,
        });
    }
    // The owner's machine signs the state again for the new identity
    let mut old_view = workspace.clone();
    old_view.repository_id = old_id.clone();
    let blocking = integrity(&old_view, &state, &old_trust)
        .into_iter()
        .filter(|finding| finding.severity >= Severity::High)
        .map(|finding| finding.message)
        .collect::<Vec<_>>();
    if !blocking.is_empty() {
        return Err(format!(
            "the signed state does not verify for {from}, so it is not rebound: {}",
            blocking.join("; ")
        ));
    }
    let Some(_) = new_trust.signing_public_key()? else {
        return Err(format!(
            "this machine's trust data was carried over to {to}, but it holds no signing key: the owner must run 'crane repo rebind {from}' and commit .crane; then run it here again"
        ));
    };
    let signer = new_trust.require_signing_key()?;
    if let Some(statement) = &state.config.admins {
        if statement.owner != signer.public_hex() {
            return Err(format!(
                "only the owner can rebind: the admin list is signed by the owner's key ({}...); the owner runs 'crane repo rebind {from}' and commits .crane",
                &statement.owner[..statement.owner.len().min(16)]
            ));
        }
    }
    let new_id = workspace.repository_id.clone();
    let count = state.registry.selections.len();
    let committed = commit(
        workspace,
        "crane repo rebind",
        REPOSITORY_REBOUND,
        json!({"from": from, "to": to, "selections": count}),
        |state, _| {
            for record in &mut state.registry.selections {
                record.repository_id = new_id.clone();
                record.origin.binding_digest = binding_digest(&new_id, &record.origin);
            }
            if let Some(statement) = &state.config.admins {
                state.config.admins = Some(AdminStatement::sign(
                    &signer,
                    &new_id,
                    statement.epoch,
                    statement.admins.clone(),
                ));
            }
            Ok(())
        },
    )?;
    Ok(Rebound {
        from,
        to,
        copied,
        resigned: Some(count),
        generation: Some(committed.generation()),
    })
}
