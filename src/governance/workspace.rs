// The repository Crane works in: its Git top level, its .crane directory, and its identity. The
// repository identity is a SHA-512 digest of the root commits and the normalized origin remote,
// so it is stable across SSH and HTTPS spellings and binds every signed record to one repository.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::files::write_atomic;
use crate::platform::{git, now_millis};
use crate::trust::crypto::digest;
use crate::trust::{crane_home, TrustStore};

/** Name of the metadata directory at the repository top level */
pub(crate) const CRANE_DIR: &str = ".crane";

/** Digest domain of the repository identity */
const IDENTITY_DOMAIN: &str = "repository/identity/v1";

/** How long a cached repository identity is reused, in milliseconds */
const IDENTITY_TTL_MS: u64 = 10 * 60 * 1000;

/** A repository identity remembered between Crane runs, so a hook does not start Git three times
 * (one of them walking the whole history) on every tool call. It is kept in the trust directory,
 * which agents can neither read nor write, never in .git, where an agent could point Crane at
 * another repository's identity.
 * Fields
    - root: String - Git top level as Git reports it
    - remote: Option<String> - normalized origin remote
    - root_commits: Vec<String> - root commits
    - config: String - digest of .git/config when cached (a changed remote invalidates the entry)
    - at: u64 - Unix milliseconds when cached
    - user: Option<String> - Git user.name
    - email: Option<String> - Git user.email
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedIdentity {
    root: String,
    remote: Option<String>,
    root_commits: Vec<String>,
    config: String,
    at: u64,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    email: Option<String>,
}

/** Find the cache entry of the repository containing the current directory: the nearest ancestor with a .git directory, unless Git is redirected by GIT_DIR or GIT_WORK_TREE (then no cache)
 * Input
    - None
 * Output
    - Option<(PathBuf, String)> cache file and the digest of .git/config
*/
fn identity_cache() -> Option<(PathBuf, String)> {
    if std::env::var_os("GIT_DIR").is_some() || std::env::var_os("GIT_WORK_TREE").is_some() {
        return None;
    }
    let current = std::env::current_dir().ok()?;
    let repository = current
        .ancestors()
        .find(|directory| directory.join(".git").is_dir())?;
    let config = std::fs::read(repository.join(".git").join("config")).ok()?;
    let key = digest(
        "repository/identity-cache/v1",
        &[repository.to_string_lossy().as_bytes()],
    );
    Some((
        crane_home()
            .ok()?
            .join("workspaces")
            .join(format!("{}.json", &key[..32])),
        digest("repository/git-config/v1", &[&config]),
    ))
}

/** A repository Crane operates on
 * Fields
    - root: PathBuf - Git top-level directory
    - crane: PathBuf - root/.crane
    - repository_id: String - repository identity digest
    - remote: Option<String> - normalized origin remote, if any
    - root_commits: Vec<String> - the root commits the identity is computed from
    - user: Option<String> - Git user.name (who acts on this machine)
    - email: Option<String> - Git user.email
    - identity_cache: &'static str - how the identity was found: HIT (cached), MISS (computed
      and cached), REFRESHED (cache expired or Git settings changed), NONE (no cache possible)
*/
#[derive(Debug, Clone)]
pub(crate) struct Workspace {
    pub(crate) root: PathBuf,
    pub(crate) crane: PathBuf,
    pub(crate) repository_id: String,
    pub(crate) remote: Option<String>,
    pub(crate) root_commits: Vec<String>,
    pub(crate) user: Option<String>,
    pub(crate) email: Option<String>,
    pub(crate) identity_cache: &'static str,
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
    /** Locate the repository from the current directory without requiring .crane, computing its identity from Git (or reusing it from the trust directory for IDENTITY_TTL_MS while .git/config is unchanged)
    * Input
        - None
    * Output
        - Result<Workspace, String>
        - Error if not inside a Git repository with at least one commit
    */
    pub(crate) fn discover() -> Result<Self, String> {
        let cache = identity_cache();
        let previous = cache.as_ref().and_then(|(file, _)| {
            serde_json::from_slice::<CachedIdentity>(&std::fs::read(file).ok()?).ok()
        });
        let cached = previous
            .clone()
            .zip(cache.as_ref())
            .and_then(|(entry, (_, config))| {
                (entry.config == *config && now_millis().saturating_sub(entry.at) < IDENTITY_TTL_MS)
                    .then_some(entry)
            });
        if let Some(entry) = cached {
            let root = PathBuf::from(&entry.root);
            return Ok(Self {
                crane: root.join(CRANE_DIR),
                repository_id: identity(&entry.root_commits, entry.remote.as_deref()),
                root,
                remote: entry.remote,
                root_commits: entry.root_commits,
                user: entry.user,
                email: entry.email,
                identity_cache: "HIT",
            });
        }
        let root = git::toplevel()?;
        let remote = git::origin_url(&root).and_then(|url| git::normalize_remote(&url));
        let roots = git::root_commits(&root).map_err(|_| {
            "the repository has no commits yet; commit before using Crane".to_string()
        })?;
        let repository_id = identity(&roots, remote.as_deref());
        let setting = |key: &str| {
            git::git_in(&root, &["config", key])
                .ok()
                .filter(|value| !value.is_empty())
        };
        let (user, email) = (setting("user.name"), setting("user.email"));
        let identity_cache = match (&cache, &previous) {
            (None, _) => "NONE",
            (Some(_), None) => "MISS",
            (Some(_), Some(_)) => "REFRESHED",
        };
        if let Some((file, config)) = cache {
            let entry = CachedIdentity {
                root: root.to_string_lossy().into_owned(),
                remote: remote.clone(),
                root_commits: roots.clone(),
                config,
                at: now_millis(),
                user: user.clone(),
                email: email.clone(),
            };
            if let Some(parent) = file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(text) = serde_json::to_vec(&entry) {
                let _ = write_atomic(&file, &text);
            }
        }
        Ok(Self {
            crane: root.join(CRANE_DIR),
            root,
            repository_id,
            remote,
            root_commits: roots,
            user,
            email,
            identity_cache,
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
