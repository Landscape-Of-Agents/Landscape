// Git access without a shell: every call spawns git directly with an argument vector, so paths and
// arguments are never interpolated. Git is the authority for repository membership, commits,
// blobs, and the file list Crane scans for selection anchors.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/** Run a Git command in the current directory and return trimmed stdout
 * Input
    - args: &[&str] - arguments passed to git
 * Output
    - Result<String, String>
    - Error with trimmed stderr if git cannot run or exits with a failure status
*/
pub(crate) fn git(args: &[&str]) -> Result<String, String> {
    git_bytes(None, args).map(|output| String::from_utf8_lossy(&output).trim().to_string())
}

/** Run a Git command in a directory and return trimmed stdout
 * Input
    - directory: &Path - working directory for git
    - args: &[&str] - arguments passed to git
 * Output
    - Result<String, String>
    - Error with trimmed stderr if git cannot run or exits with a failure status
*/
pub(crate) fn git_in(directory: &Path, args: &[&str]) -> Result<String, String> {
    git_bytes(Some(directory), args)
        .map(|output| String::from_utf8_lossy(&output).trim().to_string())
}

/** Run a Git command and keep stdout byte for byte, so file contents and NUL-separated listings
 * are not altered
 * Input
    - directory: Option<&Path> - working directory, None for the current one
    - args: &[&str] - arguments passed to git
 * Output
    - Result<Vec<u8>, String>
    - Error with trimmed stderr if git cannot run or exits with a failure status
*/
pub(crate) fn git_bytes(directory: Option<&Path>, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    if let Some(directory) = directory {
        command.current_dir(directory);
    }
    let output = super::process::hidden(&mut command)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("failed to execute git: {error}"))?;
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!("git {} failed", args.first().copied().unwrap_or_default())
        } else {
            message
        });
    }
    Ok(output.stdout)
}

/** Return the top-level directory of the Git work tree containing the current directory
 * Input
    - None
 * Output
    - Result<PathBuf, String>
    - Error if the current directory is not inside a Git work tree
*/
pub(crate) fn toplevel() -> Result<PathBuf, String> {
    git(&["rev-parse", "--show-toplevel"])
        .map(PathBuf::from)
        .map_err(|_| "not inside a Git repository; clone the repository first".to_string())
}

/** Return the full SHA of HEAD
 * Input
    - root: &Path - repository top level
 * Output
    - Result<String, String>
    - Error if the repository has no commits
*/
pub(crate) fn head(root: &Path) -> Result<String, String> {
    git_in(root, &["rev-parse", "--verify", "HEAD^{commit}"])
        .map_err(|_| "the repository has no commits yet; commit before using Crane".to_string())
}

/** Return the current branch name, or DETACHED when HEAD is not on a branch
 * Input
    - root: &Path - repository top level
 * Output
    - String branch name
*/
pub(crate) fn branch(root: &Path) -> String {
    git_in(root, &["branch", "--show-current"])
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "DETACHED".into())
}

/** Return the root commits reachable from HEAD, sorted, used as the stable part of the
 * repository identity
 * Input
    - root: &Path - repository top level
 * Output
    - Result<Vec<String>, String>
    - Error if the repository has no commits
*/
pub(crate) fn root_commits(root: &Path) -> Result<Vec<String>, String> {
    let mut commits = git_in(root, &["rev-list", "--max-parents=0", "HEAD"])?
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    commits.sort();
    Ok(commits)
}

/** Return the URL of the origin remote, if one is configured
 * Input
    - root: &Path - repository top level
 * Output
    - Option<String>
*/
pub(crate) fn origin_url(root: &Path) -> Option<String> {
    git_in(root, &["remote", "get-url", "origin"])
        .ok()
        .filter(|url| !url.is_empty())
}

/** Normalize a remote URL to "host/owner/name", so SSH and HTTPS spellings of the same repository
 * compare equal; credentials, ports, and a trailing .git are dropped, and local paths are kept as
 * "local:PATH"
 * Input
    - url: &str - remote URL such as git@github.com:o/n.git or https://github.com/o/n
 * Output
    - Option<String>, None when the URL cannot be understood
*/
pub(crate) fn normalize_remote(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if let Some(path) = url.strip_prefix("file://") {
        return Some(format!("local:{}", path.replace('\\', "/")));
    }
    let (host, path) = if let Some(rest) = url.split_once("://").map(|(_, rest)| rest) {
        let rest = rest.rsplit_once('@').map_or(rest, |(_, after)| after);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?.to_string(), path.to_string())
    } else if let Some((user_host, path)) = url
        .split_once(':')
        .filter(|(left, _)| left.contains('@') || (left.len() > 1 && left.contains('.')))
    {
        let host = user_host
            .rsplit_once('@')
            .map_or(user_host, |(_, host)| host);
        (host.to_string(), path.to_string())
    } else {
        return Some(format!("local:{}", url.replace('\\', "/")));
    };
    let path = path
        .trim_matches('/')
        .trim_end_matches(".git")
        .trim_matches('/');
    if host.is_empty() || path.is_empty() {
        return None;
    }
    Some(format!("{}/{}", host.to_ascii_lowercase(), path))
}

/** Read a file as it exists in a commit
 * Input
    - root: &Path - repository top level
    - commit: &str - commit SHA
    - path: &str - repository-relative path with "/" separators
 * Output
    - Result<Option<Vec<u8>>, String>, None if the path does not exist in the commit
    - Error if git cannot run
*/
pub(crate) fn show(root: &Path, commit: &str, path: &str) -> Result<Option<Vec<u8>>, String> {
    let spec = format!("{commit}:{path}");
    match git_bytes(Some(root), &["cat-file", "-e", &spec]) {
        Ok(_) => git_bytes(Some(root), &["cat-file", "blob", &spec]).map(Some),
        Err(_) => Ok(None),
    }
}

/** Return the blob id of a file in a commit
 * Input
    - root: &Path - repository top level
    - commit: &str - commit SHA
    - path: &str - repository-relative path
 * Output
    - Option<String>, None if the path is not a file in the commit
*/
pub(crate) fn blob_id(root: &Path, commit: &str, path: &str) -> Option<String> {
    git_in(
        root,
        &["rev-parse", "--verify", &format!("{commit}:{path}")],
    )
    .ok()
}

/** Confirm a commit exists locally
 * Input
    - root: &Path - repository top level
    - commit: &str - commit SHA
 * Output
    - bool
*/
pub(crate) fn commit_exists(root: &Path, commit: &str) -> bool {
    !commit.trim().is_empty()
        && git_in(
            root,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("{commit}^{{commit}}"),
            ],
        )
        .is_ok()
}

/** List the files Crane scans: tracked files plus untracked files that are not ignored, as
 * repository-relative paths with "/" separators, excluding files deleted from the work tree
 * Input
    - root: &Path - repository top level
 * Output
    - Result<Vec<String>, String>
    - Error if git cannot run
*/
pub(crate) fn list_files(root: &Path) -> Result<Vec<String>, String> {
    let output = git_bytes(
        Some(root),
        &[
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ],
    )?;
    let mut files = output
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| String::from_utf8_lossy(entry).into_owned())
        .filter(|path| root.join(path).is_file())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

/** List the files that differ from HEAD in the work tree or index, including untracked files that
 * are not ignored, as repository-relative paths with "/" separators (renames count as a deletion
 * and an addition)
 * Input
    - root: &Path - repository top level
 * Output
    - Result<Vec<String>, String>
    - Error if git cannot run
*/
pub(crate) fn changed_files(root: &Path) -> Result<Vec<String>, String> {
    let output = git_bytes(
        Some(root),
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
    )?;
    let mut files = output
        .split(|byte| *byte == 0)
        .filter(|entry| entry.len() > 3)
        .map(|entry| String::from_utf8_lossy(&entry[3..]).into_owned())
        .collect::<Vec<_>>();
    files.sort();
    files.dedup();
    Ok(files)
}

/** Check that a remote answers, by running git ls-remote against it with a timeout, so a dead
 * network cannot hang a command
 * Input
    - url: &str - remote URL
    - timeout: Duration - how long to wait
 * Output
    - Result<(), String>
    - Error with git's message, or a timeout message
*/
pub(crate) fn remote_reachable(url: &str, timeout: Duration) -> Result<(), String> {
    let mut child = super::process::hidden(&mut Command::new("git"))
        .args(["ls-remote", "--heads", url])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to execute git: {error}"))?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
            if status.success() {
                return Ok(());
            }
            let mut message = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                let _ = stderr.read_to_string(&mut message);
            }
            return Err(if message.trim().is_empty() {
                "git ls-remote failed".into()
            } else {
                message.trim().to_string()
            });
        }
        if started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "no answer from {url} within {} seconds",
                timeout.as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/** Convert a path given on the command line (absolute or relative to the current directory) to a
 * repository-relative path with "/" separators
 * Input
    - root: &Path - repository top level
    - path: &Path - path to convert
 * Output
    - Option<String>, None if the path lies outside the repository
*/
pub(crate) fn relative(root: &Path, path: &Path) -> Option<String> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().ok()?.join(path)
    };
    let canonical_root = dunce(root.canonicalize().ok()?);
    let canonical = match absolute.canonicalize() {
        Ok(path) => dunce(path),
        Err(_) => {
            // A path that does not exist yet: canonicalize its parent
            let parent = dunce(absolute.parent()?.canonicalize().ok()?);
            parent.join(absolute.file_name()?)
        }
    };
    let relative = canonical.strip_prefix(&canonical_root).ok()?;
    let text = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    (!text.is_empty()).then_some(text)
}

/** Canonicalize a path that may not exist yet: its nearest existing ancestor is canonicalized
 * (symlinks resolved) and the missing parts are appended
 * Input
    - path: &Path - absolute path
 * Output
    - PathBuf, the path unchanged when no ancestor exists
*/
pub(crate) fn canonical_lenient(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut existing = path.to_path_buf();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            let mut result = dunce(resolved);
            for part in missing.iter().rev() {
                result.push(part);
            }
            return result;
        }
        match (
            existing.file_name().map(std::ffi::OsStr::to_os_string),
            existing.parent(),
        ) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                existing = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/** List the other worktrees of the repository a worktree belongs to (absolute, canonical)
 * Input
    - root: &Path - this worktree's top level
 * Output
    - Vec<PathBuf>, empty when Git lists none or cannot run
*/
pub(crate) fn other_worktrees(root: &Path) -> Vec<PathBuf> {
    let here = canonical_lenient(root);
    git_in(root, &["worktree", "list", "--porcelain"])
        .unwrap_or_default()
        .lines()
        .filter_map(|line| line.strip_prefix("worktree "))
        .map(|path| canonical_lenient(Path::new(path)))
        .filter(|path| *path != here)
        .collect()
}

/** Remove the Windows verbatim prefix (\\?\) that canonicalize adds, so paths compare with the
 * ones Git prints
 * Input
    - path: PathBuf - canonical path
 * Output
    - PathBuf without the verbatim prefix
*/
fn dunce(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_remote;

    /** Check that SSH, HTTPS, and credentialed spellings normalize to the same identity
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn remotes_normalize_across_protocols() {
        for url in [
            "git@github.com:Acme/Shop.git",
            "https://github.com/Acme/Shop",
            "https://token@github.com/Acme/Shop.git",
            "ssh://git@github.com:22/Acme/Shop.git",
        ] {
            assert_eq!(
                normalize_remote(url).as_deref(),
                Some("github.com/Acme/Shop"),
                "{url}"
            );
        }
        assert_eq!(
            normalize_remote("C:\\repos\\shop.git").as_deref(),
            Some("local:C:/repos/shop.git")
        );
    }
}
