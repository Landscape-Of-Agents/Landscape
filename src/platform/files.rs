// Durable file primitives: atomic replacement through a temporary file, appending JSON lines, and
// a cross-process lock built on exclusive file creation (hooks run as separate processes and may
// run concurrently for parallel tool calls).

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use super::{io_error, now_unix};

/** Replace a file atomically, by writing the content to a temporary sibling and renaming it over
 * the destination, so readers never observe a partially written file
 * Input
    - path: &Path - destination file
    - content: &[u8] - complete new content
 * Output
    - Result<(), String>
    - Error if the directory cannot be created or the file cannot be written or renamed
*/
pub(crate) fn write_atomic(path: &Path, content: &[u8]) -> Result<(), String> {
    let directory = path.parent().ok_or("invalid file path")?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
    let temporary = directory.join(format!(
        ".{}.crane-tmp-{}",
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        std::process::id()
    ));
    fs::write(&temporary, content)
        .map_err(|error| format!("could not write {}: {error}", temporary.display()))?;
    // Windows refuses to rename over a file another process holds open; retry briefly
    let mut attempts = 0;
    loop {
        match fs::rename(&temporary, path) {
            Ok(()) => return Ok(()),
            Err(_) if attempts < 20 => {
                attempts += 1;
                thread::sleep(Duration::from_millis(25));
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(format!("could not write {}: {error}", path.display()));
            }
        }
    }
}

/** Replace an existing file atomically while keeping its permissions (an executable script stays
 * executable), used when Crane writes anchor comments into source files
 * Input
    - path: &Path - existing file
    - content: &[u8] - complete new content
 * Output
    - Result<(), String>
    - Error if the file cannot be written
*/
pub(crate) fn rewrite_preserving(path: &Path, content: &[u8]) -> Result<(), String> {
    let permissions = fs::metadata(path)
        .map(|metadata| metadata.permissions())
        .ok();
    write_atomic(path, content)?;
    if let Some(permissions) = permissions {
        fs::set_permissions(path, permissions).map_err(io_error)?;
    }
    Ok(())
}

/** Restrict a secret file to its owner, by setting mode 0600 on Unix; on Windows files under the
 * user profile already inherit an owner-only ACL, so nothing is changed there
 * Input
    - path: &Path - file holding secret material
 * Output
    - Result<(), String>
    - Error if the permissions cannot be changed
*/
pub(crate) fn restrict_to_owner(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_error)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/** Append one line to a file, creating it and its directory when missing
 * Input
    - path: &Path - file to append to
    - line: &str - text without the trailing newline
 * Output
    - Result<(), String>
    - Error if the file cannot be opened or written
*/
pub(crate) fn append_line(path: &Path, line: &str) -> Result<(), String> {
    if let Some(directory) = path.parent() {
        fs::create_dir_all(directory).map_err(io_error)?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    writeln!(file, "{line}").map_err(io_error)?;
    file.sync_data().map_err(io_error)
}

/** Read a JSON-lines file into values, skipping blank lines; a missing file is empty
 * Input
    - path: &Path - file to read
 * Output
    - Result<Vec<serde_json::Value>, String>
    - Error naming the line if a line is not valid JSON
*/
pub(crate) fn read_lines(path: &Path) -> Result<Vec<serde_json::Value>, String> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    content
        .lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(number, line)| {
            serde_json::from_str(line).map_err(|error| {
                format!(
                    "{} line {} is not valid JSON: {error}",
                    path.display(),
                    number + 1
                )
            })
        })
        .collect()
}

/** An exclusive cross-process lock held while the value lives
 * Fields
    - path: PathBuf - the lock file, removed on drop
*/
pub(crate) struct Lock {
    path: PathBuf,
}

/** How long a lock may be held before another process treats it as abandoned, in seconds */
const STALE_LOCK_SECONDS: u64 = 30;

/** How long to wait for a lock before failing, in milliseconds */
const LOCK_TIMEOUT_MILLIS: u64 = 10_000;

/** The outcome of one attempt to take a lock
 * Variants
    - Taken(Lock) - the lock is now held
    - Busy - another process holds it
*/
enum Attempt {
    Taken(Lock),
    Busy,
}

impl Lock {
    /** Try once to take the lock: create the lock file exclusively, and break a lock whose
     * recorded creation time is older than STALE_LOCK_SECONDS (a crashed process), so it cannot
     * block every later one
     * Input
        - path: &Path - lock file path (its directory exists)
     * Output
        - Result<Attempt, String>
        - Error if the lock file cannot be created for another reason
    */
    fn attempt(path: &Path) -> Result<Attempt, String> {
        match OpenOptions::new().write(true).create_new(true).open(path) {
            Ok(mut file) => {
                let _ = write!(file, "{} {}", std::process::id(), now_unix());
                Ok(Attempt::Taken(Self {
                    path: path.to_path_buf(),
                }))
            }
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                let stale = fs::read_to_string(path)
                    .ok()
                    .and_then(|text| text.split_whitespace().nth(1)?.parse::<u64>().ok())
                    .is_some_and(|created| now_unix().saturating_sub(created) > STALE_LOCK_SECONDS);
                if stale && fs::remove_file(path).is_ok() {
                    return Self::attempt(path);
                }
                Ok(Attempt::Busy)
            }
            // Windows reports access denied while another process is deleting the lock file
            Err(error) if cfg!(windows) && error.kind() == ErrorKind::PermissionDenied => {
                Ok(Attempt::Busy)
            }
            Err(error) => Err(format!("could not create lock {}: {error}", path.display())),
        }
    }

    /** Acquire the lock, waiting while another process holds it
     * Input
        - path: &Path - lock file path
     * Output
        - Result<Lock, String>
        - Error if the lock cannot be acquired within LOCK_TIMEOUT_MILLIS
    */
    pub(crate) fn acquire(path: &Path) -> Result<Self, String> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).map_err(io_error)?;
        }
        let started = Instant::now();
        loop {
            match Self::attempt(path)? {
                Attempt::Taken(lock) => return Ok(lock),
                Attempt::Busy if started.elapsed() > Duration::from_millis(LOCK_TIMEOUT_MILLIS) => {
                    return Err(format!(
                        "timed out waiting for lock {}; remove it if no Crane process is running",
                        path.display()
                    ))
                }
                Attempt::Busy => thread::sleep(Duration::from_millis(15)),
            }
        }
    }

    /** Take the lock only if no other process holds it
     * Input
        - path: &Path - lock file path
     * Output
        - Result<Option<Lock>, String>, None when another process holds it
    */
    pub(crate) fn try_acquire(path: &Path) -> Result<Option<Self>, String> {
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory).map_err(io_error)?;
        }
        Ok(match Self::attempt(path)? {
            Attempt::Taken(lock) => Some(lock),
            Attempt::Busy => None,
        })
    }
}

impl Drop for Lock {
    /** Release the lock by removing its file
     * Input
        - None (uses self)
     * Output
        - None
    */
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/** Check whether bytes look like text, by requiring valid UTF-8 without NUL bytes
 * Input
    - bytes: &[u8] - file content
 * Output
    - bool, true for text
*/
pub(crate) fn is_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

#[cfg(test)]
mod tests {
    use super::Lock;

    /** Check that a held lock is busy for another taker and free again once released
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn locks_exclude_and_release() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("x.lock");
        let held = Lock::try_acquire(&path).unwrap().expect("free at first");
        assert!(
            Lock::try_acquire(&path).unwrap().is_none(),
            "busy while held"
        );
        drop(held);
        assert!(
            Lock::try_acquire(&path).unwrap().is_some(),
            "free after release"
        );
        std::fs::write(&path, "1 0").unwrap();
        assert!(Lock::acquire(&path).is_ok(), "a stale lock is broken");
    }
}
