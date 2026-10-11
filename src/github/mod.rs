// The repository connection. Crane connects to the GitHub repository a local clone came from, over
// SSH, HTTPS, or the GitHub CLI, records which read privileges were granted (file reads,
// CODEOWNERS, contributors, CI/CD, Actions, branches, insights), and checks the connection live
// (git ls-remote, or gh repo view) whenever its status matters. The record lives in the trust
// directory, not in the repository, so an agent cannot forge a connection by editing .crane.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::governance::workspace::Workspace;
use crate::platform::files::write_atomic;
use crate::platform::{actor, git, io_error, now_unix, process};
use crate::trust::{crane_home, require_human};

/** Privileges Crane can be granted, with their shorthand and meaning */
pub(crate) const PRIVILEGES: &[(&str, &str)] = &[
    ("rf", "read repository files as text (by extension)"),
    ("codeowners", "read CODEOWNERS"),
    ("contributors", "read contributors"),
    ("ci-cd", "read CI/CD configuration"),
    ("gh-actions", "read GitHub Actions workflows and runs"),
    ("gh-branches", "read branches and protection"),
    ("gh-insights", "read insights (activity and traffic)"),
];

/** Longest number of days a GitHub grant may last */
pub(crate) const MAX_GRANT_DAYS: u64 = 180;

/** Lifetime of a terminal-session grant, in seconds */
const SESSION_GRANT_SECONDS: u64 = 12 * 60 * 60;

/** How long a connection probe may take */
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/** How Crane reaches the repository
 * Variants
    - Ssh - git over SSH (git@github.com:OWNER/NAME.git)
    - Https - git over HTTPS (https://github.com/OWNER/NAME)
    - GhCli - the GitHub CLI, with OWNER/NAME
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Method {
    Ssh,
    Https,
    GhCli,
}

impl Method {
    /** Return the method's flag without dashes
     * Input
        - None (uses self)
     * Output
        - &'static str, "ssh", "https", or "gh-cli"
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ssh => "ssh",
            Self::Https => "https",
            Self::GhCli => "gh-cli",
        }
    }
}

/** A recorded connection
 * Fields
    - method: Method - how the repository is reached
    - remote: String - remote as given
    - url: String - Git URL probed
    - normalized: String - normalized "host/owner/name"
    - privileges: Vec<String> - granted privilege shorthands
    - connected_at: u64 - Unix seconds
    - connected_by: String - actor
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Connection {
    pub(crate) method: Method,
    pub(crate) remote: String,
    pub(crate) url: String,
    pub(crate) normalized: String,
    pub(crate) privileges: Vec<String>,
    pub(crate) connected_at: u64,
    pub(crate) connected_by: String,
}

/** Live connection status
 * Variants
    - Connected - the repository answered
    - NotConnected - no connection is recorded
    - Failure(String) - a connection is recorded but the repository did not answer
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Connected,
    NotConnected,
    Failure(String),
}

impl Status {
    /** Return the status keyword
     * Input
        - None (uses self)
     * Output
        - &'static str, CONNECTED, NOT_CONNECTED, or FAILURE
    */
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Connected => "CONNECTED",
            Self::NotConnected => "NOT_CONNECTED",
            Self::Failure(_) => "FAILURE",
        }
    }
}

/** Return the connection file of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<PathBuf, String>
*/
fn connection_path(workspace: &Workspace) -> Result<PathBuf, String> {
    Ok(workspace.trust()?.repo_dir.join("connection.json"))
}

/** Load the recorded connection
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Option<Connection>, String>, None when not connected
    - Error if the record is unreadable
*/
pub(crate) fn load(workspace: &Workspace) -> Result<Option<Connection>, String> {
    let path = connection_path(workspace)?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/** Check whether local remotes are allowed (CRANE_ALLOW_LOCAL_REMOTE=1, for offline mirrors and
 * tests)
 * Input
    - None
 * Output
    - bool
*/
fn local_allowed() -> bool {
    std::env::var("CRANE_ALLOW_LOCAL_REMOTE").is_ok_and(|value| value == "1")
}

/** Check that a normalized remote is on an allowed GitHub host: github.com, any host listed in
 * CRANE_GITHUB_HOSTS (comma separated, for GitHub Enterprise), or a local remote when allowed
 * Input
    - normalized: &str - normalized remote
 * Output
    - Result<(), String>
    - Error naming the host
*/
fn check_host(normalized: &str) -> Result<(), String> {
    if normalized.starts_with("local:") {
        return if local_allowed() {
            Ok(())
        } else {
            Err("local remotes are not GitHub repositories (set CRANE_ALLOW_LOCAL_REMOTE=1 for offline mirrors)".into())
        };
    }
    let mut parts = normalized.split('/');
    let host = parts.next().unwrap_or_default();
    if parts.filter(|part| !part.is_empty()).count() < 2 {
        return Err(format!("'{normalized}' does not name OWNER/REPOSITORY"));
    }
    let extra = std::env::var("CRANE_GITHUB_HOSTS").unwrap_or_default();
    if host == "github.com"
        || extra
            .split(',')
            .any(|allowed| allowed.trim().eq_ignore_ascii_case(host))
    {
        Ok(())
    } else {
        Err(format!(
            "{host} is not a GitHub host (add GitHub Enterprise hosts to CRANE_GITHUB_HOSTS)"
        ))
    }
}

/** Validate a remote for a method and derive the URL to probe and its normalized form
 * Input
    - method: Method - connection method
    - remote: &str - remote as given
 * Output
    - Result<(String, String), String> URL and normalized remote
    - Error if the remote does not fit the method or is not on a GitHub host
*/
pub(crate) fn interpret(method: Method, remote: &str) -> Result<(String, String), String> {
    let remote = remote.trim();
    let url = match method {
        Method::GhCli => {
            let parts = remote.split('/').collect::<Vec<_>>();
            if parts.len() != 2 || parts.iter().any(|part| part.is_empty()) {
                return Err(format!("--gh-cli expects OWNER/REPOSITORY, got '{remote}'"));
            }
            format!("https://github.com/{remote}")
        }
        Method::Ssh => {
            let local =
                git::normalize_remote(remote).is_some_and(|value| value.starts_with("local:"));
            let ssh = remote.starts_with("ssh://")
                || remote
                    .split_once(':')
                    .is_some_and(|(left, _)| left.contains('@') && !left.contains('/'));
            if !ssh && !local {
                return Err(format!(
                    "--ssh expects git@HOST:OWNER/REPOSITORY.git, got '{remote}'"
                ));
            }
            remote.to_string()
        }
        Method::Https => {
            let local =
                git::normalize_remote(remote).is_some_and(|value| value.starts_with("local:"));
            if !remote.starts_with("https://") && !local {
                return Err(format!(
                    "--https expects https://HOST/OWNER/REPOSITORY, got '{remote}'"
                ));
            }
            remote.to_string()
        }
    };
    let normalized = git::normalize_remote(&url)
        .ok_or_else(|| format!("cannot understand remote '{remote}'"))?;
    check_host(&normalized)?;
    Ok((url, normalized))
}

/** Probe a connection: git ls-remote for SSH and HTTPS, gh repo view for the GitHub CLI
 * Input
    - connection: &Connection - recorded connection
 * Output
    - Result<(), String>
    - Error with the tool's message
*/
fn probe(connection: &Connection) -> Result<(), String> {
    match connection.method {
        Method::Ssh | Method::Https => git::remote_reachable(&connection.url, PROBE_TIMEOUT),
        Method::GhCli => {
            let output = process::run(
                Command::new(gh_binary()).args([
                    "repo",
                    "view",
                    &connection.remote,
                    "--json",
                    "nameWithOwner",
                ]),
                PROBE_TIMEOUT,
            )?;
            if output.success {
                Ok(())
            } else {
                Err(format!("gh repo view failed: {}", output.stderr.trim()))
            }
        }
    }
}

/** Return the GitHub CLI executable: CRANE_GH when set, otherwise gh
 * Input
    - None
 * Output
    - String
*/
fn gh_binary() -> String {
    std::env::var("CRANE_GH").unwrap_or_else(|_| "gh".into())
}

/** Connect (or reconnect with new privileges): validate the remote, require that it is the
 * clone's origin, probe it, and record it in the trust directory
 * Input
    - workspace: &Workspace - repository
    - method: Method - connection method
    - remote: &str - remote as given
    - privileges: Vec<String> - privilege shorthands
 * Output
    - Result<Connection, String>
    - Error if run by an agent, the remote is invalid, is not the origin, or does not answer
*/
pub(crate) fn connect(
    workspace: &Workspace,
    method: Method,
    remote: &str,
    mut privileges: Vec<String>,
) -> Result<Connection, String> {
    require_human("crane repo")?;
    for privilege in &privileges {
        if !PRIVILEGES.iter().any(|(name, _)| name == privilege) {
            return Err(format!(
                "unknown privilege '{privilege}'; use {}",
                PRIVILEGES
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }
    privileges.sort();
    privileges.dedup();
    let (url, normalized) = interpret(method, remote)?;
    match &workspace.remote {
        None => return Err("this repository has no origin remote; clone it from GitHub first".into()),
        Some(origin) if *origin != normalized => {
            return Err(format!(
                "this clone's origin is {origin}, not {normalized}; connect the repository the clone came from"
            ))
        }
        Some(_) => {}
    }
    let connection = Connection {
        method,
        remote: remote.trim().to_string(),
        url,
        normalized,
        privileges,
        connected_at: now_unix(),
        connected_by: actor(),
    };
    probe(&connection).map_err(|error| format!("GitHub connection failed: {error}"))?;
    let text = serde_json::to_string_pretty(&connection).map_err(io_error)?;
    write_atomic(&connection_path(workspace)?, text.as_bytes())?;
    Ok(connection)
}

/** Check the live connection status
 * Input
    - workspace: &Workspace - repository
 * Output
    - Status
*/
pub(crate) fn status(workspace: &Workspace) -> Status {
    match load(workspace) {
        Err(error) => Status::Failure(error),
        Ok(None) => Status::NotConnected,
        Ok(Some(connection)) => match probe(&connection) {
            Ok(()) => Status::Connected,
            Err(error) => Status::Failure(error),
        },
    }
}

/** Require a live connection, for init and checkpoint
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Connection, String>
    - Error if not connected or the repository does not answer
*/
pub(crate) fn require_connected(workspace: &Workspace) -> Result<Connection, String> {
    let connection = load(workspace)?.ok_or(
        "Crane is not connected to the GitHub repository (repo status: NOT_CONNECTED); run 'crane repo --ssh|--https|--gh-cli REMOTE' first",
    )?;
    probe(&connection)
        .map_err(|error| format!("GitHub connection failed (repo status: FAILURE): {error}"))?;
    Ok(connection)
}

/** Delete the connection, so the status becomes NOT_CONNECTED
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<bool, String>, false when there was no connection
    - Error if run by an agent or the record cannot be removed
*/
pub(crate) fn disconnect(workspace: &Workspace) -> Result<bool, String> {
    require_human("crane repo del")?;
    match fs::remove_file(connection_path(workspace)?) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/** A grant allowing Crane to use the GitHub CLI's login for reads
 * Fields
    - mode: String - "session" (12 hours) or "days"
    - days: Option<u64> - requested days
    - granted_at: u64 - Unix seconds
    - expires_at: u64 - Unix seconds
    - granted_by: String - actor
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Grant {
    pub(crate) mode: String,
    pub(crate) days: Option<u64>,
    pub(crate) granted_at: u64,
    pub(crate) expires_at: u64,
    pub(crate) granted_by: String,
}

/** Validate a number of grant days
 * Input
    - value: &str - days as given
 * Output
    - Result<u64, String>
    - Error if not an integer between 1 and MAX_GRANT_DAYS
*/
pub(crate) fn parse_days(value: &str) -> Result<u64, String> {
    let days = value
        .parse::<i64>()
        .map_err(|_| format!("DAYS must be a whole number, got '{value}'"))?;
    if days <= 0 || days as u64 > MAX_GRANT_DAYS {
        return Err(format!(
            "DAYS must be between 1 and {MAX_GRANT_DAYS}, got {days}"
        ));
    }
    Ok(days as u64)
}

/** Run gh auth login interactively and record a read grant for the terminal session (12 hours)
 * or for a number of days
 * Input
    - days: Option<u64> - grant length in days, None for the terminal session
 * Output
    - Result<Grant, String>
    - Error if run by an agent, gh is missing, or the login fails
*/
pub(crate) fn login(days: Option<u64>) -> Result<Grant, String> {
    require_human("crane github")?;
    let status = Command::new(gh_binary())
        .args(["auth", "login", "--git-protocol", "https", "--scopes", "read:org"])
        .status()
        .map_err(|error| format!("could not run the GitHub CLI (gh): {error}; install it from https://cli.github.com"))?;
    if !status.success() {
        return Err("gh auth login failed".into());
    }
    let now = now_unix();
    let grant = Grant {
        mode: if days.is_some() { "days" } else { "session" }.into(),
        days,
        granted_at: now,
        expires_at: now + days.map_or(SESSION_GRANT_SECONDS, |days| days * 24 * 60 * 60),
        granted_by: actor(),
    };
    let text = serde_json::to_string_pretty(&grant).map_err(io_error)?;
    write_atomic(&crane_home()?.join("github-grant.json"), text.as_bytes())?;
    Ok(grant)
}

/** Load the GitHub grant if it has not expired
 * Input
    - None
 * Output
    - Option<Grant>
*/
pub(crate) fn active_grant() -> Option<Grant> {
    let text = fs::read_to_string(crane_home().ok()?.join("github-grant.json")).ok()?;
    let grant: Grant = serde_json::from_str(&text).ok()?;
    (grant.expires_at > now_unix()).then_some(grant)
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check remote interpretation per method, host restrictions, and day limits
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn interprets_remotes_and_days() {
        assert_eq!(
            interpret(Method::Ssh, "git@github.com:acme/shop.git")
                .unwrap()
                .1,
            "github.com/acme/shop"
        );
        assert_eq!(
            interpret(Method::Https, "https://github.com/acme/shop")
                .unwrap()
                .1,
            "github.com/acme/shop"
        );
        assert_eq!(
            interpret(Method::GhCli, "acme/shop").unwrap().0,
            "https://github.com/acme/shop"
        );
        assert!(interpret(Method::Ssh, "https://github.com/acme/shop").is_err());
        assert!(interpret(Method::Https, "git@github.com:acme/shop.git").is_err());
        assert!(interpret(Method::Https, "https://gitlab.com/acme/shop").is_err());
        assert!(interpret(Method::GhCli, "shop").is_err());
        assert_eq!(parse_days("180").unwrap(), 180);
        assert!(parse_days("181").is_err());
        assert!(parse_days("0").is_err());
        assert!(parse_days("-3").is_err());
        assert!(parse_days("x").is_err());
    }
}
