// Resource resolution: what a target an agent names really is, decided from the file system and
// Git, never from the agent's description. A path is made absolute (from the current directory),
// resolved through symlinks to a repository path, and classified: Crane metadata, the trust
// directory, hook settings, another worktree of the same repository, outside the repository, or a
// repository file that is tracked, untracked, ignored, missing, or inside a nested repository. The
// content digest identifies the file as it is now, so a decision can be recognized as stale when
// the file changes between the decision and the action (time of check versus time of use).

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{Access, Boundary, Request};
use crate::hooks::enforce::Guard;
use crate::platform::git;
use crate::trust::crypto::sha512_hex;

/** Where a target is
 * Variants
    - Repository - a file of the repository Crane governs
    - CraneMetadata - Crane's signed rules (.crane)
    - TrustDirectory - Crane's trust directory (keys, secrets, runtime evidence)
    - HookSettings - an agent tool's hook settings
    - OtherWorktree - another worktree of the same repository
    - OutsideRepository - anywhere else on the machine
    - Url - a network resource
    - Command - a shell command
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Location {
    Repository,
    CraneMetadata,
    TrustDirectory,
    HookSettings,
    OtherWorktree,
    OutsideRepository,
    Url,
    Command,
}

/** How Git knows a repository file
 * Variants
    - Tracked - in the index
    - Untracked - not in the index, not ignored (zones and flows cover it)
    - Ignored - ignored by Git (zones and flows do not cover it)
    - Missing - does not exist yet
    - NestedRepository - inside a nested repository or submodule
    - NotApplicable - not a repository file
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Tracking {
    Tracked,
    Untracked,
    Ignored,
    Missing,
    NestedRepository,
    NotApplicable,
}

/** A resolved target
 * Fields
    - requested: String - the target as given
    - location: Location - where it is
    - path: Option<String> - repository-relative path (repository files and Crane metadata)
    - absolute: Option<String> - absolute path after resolving symlinks (files)
    - via_symlink: bool - the path as given passes through a symlink
    - tracking: Tracking - how Git knows it
    - content_digest: Option<String> - digest of its content now (existing files)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Resource {
    pub(crate) requested: String,
    pub(crate) location: Location,
    pub(crate) path: Option<String>,
    pub(crate) absolute: Option<String>,
    pub(crate) via_symlink: bool,
    pub(crate) tracking: Tracking,
    pub(crate) content_digest: Option<String>,
}

/** The digest that identifies a file's content now
 * Input
    - path: &Path - file
 * Output
    - Option<String>, None when it cannot be read
*/
pub(crate) fn content_digest(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| sha512_hex(&bytes)[..32].to_string())
}

/** Make a path absolute and remove "." and ".." without touching the file system
 * Input
    - base: &Path - directory relative paths start from
    - raw: &str - path as given
 * Output
    - PathBuf
*/
fn lexical_absolute(base: &Path, raw: &str) -> PathBuf {
    let joined = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        base.join(raw)
    };
    let mut normal = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            other => normal.push(other.as_os_str()),
        }
    }
    normal
}

/** Report whether a path, or any directory on the way to it, is a symlink
 * Input
    - path: &Path - absolute path
 * Output
    - bool
*/
fn passes_symlink(path: &Path) -> bool {
    path.ancestors().any(|ancestor| {
        std::fs::symlink_metadata(ancestor).is_ok_and(|metadata| metadata.file_type().is_symlink())
    })
}

/** Classify how Git knows a repository file
 * Input
    - root: &Path - repository top level
    - path: &str - repository-relative path
 * Output
    - Tracking
*/
fn tracking(root: &Path, path: &str) -> Tracking {
    let full = root.join(path);
    let nested = full
        .parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .take_while(|directory| *directory != root && directory.starts_with(root))
        .any(|directory| directory.join(".git").exists());
    if nested {
        return Tracking::NestedRepository;
    }
    if !full.exists() {
        return Tracking::Missing;
    }
    if git::git_in(root, &["ls-files", "--", path]).is_ok_and(|listed| !listed.is_empty()) {
        return Tracking::Tracked;
    }
    // check-ignore exits 0 and prints the path when it is ignored, and fails otherwise
    if git::git_in(root, &["check-ignore", "--", path]).is_ok_and(|listed| !listed.is_empty()) {
        Tracking::Ignored
    } else {
        Tracking::Untracked
    }
}

/** Resolve a request's target
 * Input
    - boundary: &Boundary - the boundary
    - guard: &Guard - guard (protected names and repository paths)
    - request: &Request - question
 * Output
    - Resource
*/
pub(crate) fn resolve(boundary: &Boundary, guard: &Guard, request: &Request) -> Resource {
    let mut resource = Resource {
        requested: request.target.clone(),
        location: Location::Command,
        path: None,
        absolute: None,
        via_symlink: false,
        tracking: Tracking::NotApplicable,
        content_digest: None,
    };
    if request.access == Access::Execute {
        return resource;
    }
    if request.target.contains("://") {
        resource.location = Location::Url;
        return resource;
    }
    let base = guard
        .cwd
        .clone()
        .unwrap_or_else(|| boundary.workspace.root.clone());
    let lexical = lexical_absolute(&base, &request.target);
    let resolved = git::canonical_lenient(&lexical);
    resource.via_symlink = passes_symlink(&lexical);
    resource.absolute = Some(resolved.to_string_lossy().into_owned());
    resource.content_digest = resolved
        .is_file()
        .then(|| content_digest(&resolved))
        .flatten();
    let in_trust = boundary
        .trust_home
        .as_ref()
        .is_some_and(|home| resolved.starts_with(git::canonical_lenient(home)));
    if in_trust {
        resource.location = Location::TrustDirectory;
        return resource;
    }
    match guard.relative(&request.target) {
        Some(path) => {
            resource.location = if path == ".crane" || path.starts_with(".crane/") {
                Location::CraneMetadata
            } else if boundary
                .protected
                .iter()
                .any(|protected| *protected == path)
            {
                Location::HookSettings
            } else {
                Location::Repository
            };
            if resource.location == Location::Repository {
                resource.tracking = tracking(&boundary.workspace.root, &path);
            }
            resource.path = Some(path);
        }
        None => {
            resource.location = if git::other_worktrees(&boundary.workspace.root)
                .iter()
                .any(|worktree| resolved.starts_with(worktree))
            {
                Location::OtherWorktree
            } else {
                Location::OutsideRepository
            };
        }
    }
    resource
}

#[cfg(test)]
mod tests {
    use super::lexical_absolute;
    use std::path::Path;

    /** Lexical normalization removes "." and ".." and keeps absolute paths
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn lexical_paths_are_normalized() {
        let base = Path::new("/repo/web");
        assert_eq!(
            lexical_absolute(base, "../app/./pay.py"),
            Path::new("/repo/app/pay.py")
        );
        assert_eq!(lexical_absolute(base, "x.js"), Path::new("/repo/web/x.js"));
    }
}
