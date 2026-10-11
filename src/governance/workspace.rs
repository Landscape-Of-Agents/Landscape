// The repository Crane works in: its Git top level, its .crane directory, and its identity. The
// repository identity is a SHA-512 digest of the root commits and the normalized origin remote,
// so it is stable across SSH and HTTPS spellings and binds every signed record to one repository.

use std::path::{Path, PathBuf};

use crate::platform::git;
use crate::trust::crypto::digest;
use crate::trust::TrustStore;

/** Name of the metadata directory at the repository top level */
pub(crate) const CRANE_DIR: &str = ".crane";

/** Digest domain of the repository identity */
const IDENTITY_DOMAIN: &str = "repository/identity/v1";

/** A repository Crane operates on
 * Fields
    - root: PathBuf - Git top-level directory
    - crane: PathBuf - root/.crane
    - repository_id: String - repository identity digest
    - remote: Option<String> - normalized origin remote, if any
    - root_commits: Vec<String> - the root commits the identity is computed from
*/
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) crane: PathBuf,
    pub(crate) repository_id: String,
    pub(crate) remote: Option<String>,
    pub(crate) root_commits: Vec<String>,
}

/** Compute a repository identity from its root commits and normalized remote (the ingestion
 * service recomputes it to check which repository an upload really belongs to)
 * Input
    - roots: &[String] - root commits
    - remote: Option<&str> - normalized origin remote
 * Output
    - String of 128 hex characters
*/
pub(crate) fn identity(roots: &[String], remote: Option<&str>) -> String {
    digest(
        IDENTITY_DOMAIN,
        &[
            roots.join("\n").as_bytes(),
            remote.unwrap_or_default().as_bytes(),
        ],
    )
}

impl Workspace {
    /** Locate the repository from the current directory without requiring .crane, computing its
     * identity from Git
     * Input
        - None
     * Output
        - Result<Workspace, String>
        - Error if not inside a Git repository with at least one commit
    */
    pub(crate) fn discover() -> Result<Self, String> {
        let root = git::toplevel()?;
        let remote = git::origin_url(&root).and_then(|url| git::normalize_remote(&url));
        let roots = git::root_commits(&root).map_err(|_| {
            "the repository has no commits yet; commit before using Crane".to_string()
        })?;
        let repository_id = identity(&roots, remote.as_deref());
        Ok(Self {
            crane: root.join(CRANE_DIR),
            root,
            repository_id,
            remote,
            root_commits: roots,
        })
    }

    /** Locate an initialized repository
     * Input
        - None
     * Output
        - Result<Workspace, String>
        - Error if not in a repository or .crane does not exist
    */
    pub(crate) fn locate() -> Result<Self, String> {
        let workspace = Self::discover()?;
        if !workspace.crane.is_dir() {
            return Err("Crane is not initialized in this repository; run 'crane init'".into());
        }
        Ok(workspace)
    }

    /** Open the trust directory of this repository
     * Input
        - None (uses self)
     * Output
        - Result<TrustStore, String>
    */
    pub(crate) fn trust(&self) -> Result<TrustStore, String> {
        TrustStore::open(&self.repository_id)
    }

    /** Return the path of a file inside .crane
     * Input
        - name: &str - relative name such as "registry.json"
     * Output
        - PathBuf
    */
    pub(crate) fn file(&self, name: &str) -> PathBuf {
        self.crane.join(name)
    }

    /** Return the policies directory
     * Input
        - None (uses self)
     * Output
        - PathBuf of .crane/policies
    */
    pub(crate) fn policies_dir(&self) -> PathBuf {
        self.crane.join("policies")
    }

    /** Resolve a file argument: a path (absolute or relative to the current directory) inside the
     * repository, or else a file name or path suffix that matches exactly one file Git knows
     * Input
        - argument: &str - path or unique file name
     * Output
        - Result<String, String> repository-relative path
        - Error if nothing or more than one file matches, or the path is outside the repository
    */
    pub(crate) fn resolve_file(&self, argument: &str) -> Result<String, String> {
        let candidate = Path::new(argument);
        if candidate.is_file() {
            return git::relative(&self.root, candidate)
                .ok_or_else(|| format!("{argument} is outside the repository"));
        }
        let wanted = argument.replace('\\', "/");
        let wanted = wanted.trim_start_matches("./");
        let matches = git::list_files(&self.root)?
            .into_iter()
            .filter(|path| path == wanted || path.ends_with(&format!("/{wanted}")))
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [single] => Ok(single.clone()),
            [] => Err(format!("no file named '{argument}' in the repository")),
            many => Err(format!(
                "'{argument}' matches {} files ({}); give a longer path",
                many.len(),
                many.iter().take(5).cloned().collect::<Vec<_>>().join(", ")
            )),
        }
    }
}
