// The trust boundary. Authoritative signing material and rollback anchors live in the Crane trust
// directory outside every repository (CRANE_HOME, by default ~/.crane-trust), never in .crane/,
// which is treated as untrusted until its signatures verify. Agents are refused here (environment
// markers) and at the hook boundary (tool calls naming .crane or the trust directory).
//
// Limitation, stated plainly: an agent with an unrestricted shell running as the same operating
// system user can read files the user can read. The hooks deny every tool call that names these
// paths, but only an execution boundary that mediates all file access (a sandbox, a container, or
// a separate user) can make that guarantee.

pub(crate) mod crypto;

use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform::files::{restrict_to_owner, write_atomic};
use crate::platform::{io_error, now_unix};
use crypto::{verify_signature, SigningIdentity};

/** Environment variables set by agent hosts (Claude Code, Codex) or by an adapter for agent tool
 * processes; their presence means a command is running on behalf of an agent */
pub(crate) const AGENT_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "CRANE_AGENT",
];

/** Return the first agent marker set in the environment
 * Input
    - None
 * Output
    - Option<&'static str>, None when no agent marker is set
*/
pub(crate) fn agent_environment() -> Option<&'static str> {
    AGENT_MARKERS
        .iter()
        .copied()
        .find(|marker| env::var_os(marker).is_some_and(|value| !value.is_empty()))
}

/** Refuse a command that only a human may run when it runs on behalf of an agent
 * Input
    - command: &str - command name for the message, such as "crane checkpoint"
 * Output
    - Result<(), String>
    - Error naming the agent marker found
*/
pub(crate) fn require_human(command: &str) -> Result<(), String> {
    match agent_environment() {
        Some(marker) => Err(format!(
            "{command} cannot be executed by an AI agent ({marker} is set); a human must run it"
        )),
        None => Ok(()),
    }
}

/** Return the Crane trust directory: CRANE_HOME when set, otherwise .crane-trust in the user's
 * home directory (deliberately not named .crane, so it can never be mistaken for a repository's
 * metadata directory)
 * Input
    - None
 * Output
    - Result<PathBuf, String>
    - Error if no home directory can be determined
*/
pub(crate) fn crane_home() -> Result<PathBuf, String> {
    if let Some(home) = env::var_os("CRANE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(home));
    }
    env::var_os("USERPROFILE")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("HOME").filter(|value| !value.is_empty()))
        .map(|home| PathBuf::from(home).join(".crane-trust"))
        .ok_or_else(|| "cannot determine the home directory; set CRANE_HOME".into())
}

/** File of this machine's upload key in the trust directory */
const INSTALLATION_KEY: &str = "installation.key";

/** Load this machine's upload key, creating it (owner-only) on first use. The key identifies the
 * machine to the Foxx ingestion service and grants no authority over any repository, so unlike a
 * repository signing key it is created for agents' hooks as well, and every teammate's machine
 * uploads without running crane init
 * Input
    - None
 * Output
    - Result<SigningIdentity, String>
    - Error if the trust directory cannot be determined or the key cannot be read or written
*/
pub(crate) fn installation_key() -> Result<SigningIdentity, String> {
    installation_key_in(&crane_home()?)
}

/** Load or create the upload key in a trust directory. Concurrent first uses agree on one key:
 * each writes a complete temporary file and links it into place only when no key exists yet, so a
 * reader never sees a partial key and a key is never replaced
 * Input
    - home: &Path - trust directory
 * Output
    - Result<SigningIdentity, String>
*/
fn installation_key_in(home: &Path) -> Result<SigningIdentity, String> {
    let path = home.join(INSTALLATION_KEY);
    let read = || {
        let seed = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        SigningIdentity::from_seed_hex(&seed)
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    match fs::metadata(&path) {
        Ok(_) => return read(),
        Err(error) if error.kind() != ErrorKind::NotFound => {
            return Err(format!("could not read {}: {error}", path.display()))
        }
        Err(_) => {}
    }
    fs::create_dir_all(home).map_err(io_error)?;
    let identity = SigningIdentity::generate()?;
    let temporary = home.join(format!(
        ".{INSTALLATION_KEY}.crane-tmp-{}",
        crypto::hex(&crypto::random_bytes(8)?)
    ));
    fs::write(&temporary, format!("{}\n", identity.seed_hex())).map_err(io_error)?;
    restrict_to_owner(&temporary)?;
    let linked = fs::hard_link(&temporary, &path);
    let _ = fs::remove_file(&temporary);
    match linked {
        Ok(()) => Ok(identity),
        Err(error) if error.kind() == ErrorKind::AlreadyExists => read(),
        Err(error) => Err(format!("could not create {}: {error}", path.display())),
    }
}

/** The latest registry generation this machine has seen, used to detect rollback of .crane
 * Fields
    - generation: u64 - registry generation
    - head: String - registry manifest head digest at that generation
    - updated_at: u64 - Unix seconds
    - signature: String - Ed25519 signature over generation and head
*/
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub(crate) generation: u64,
    pub(crate) head: String,
    pub(crate) updated_at: u64,
    pub(crate) signature: String,
}

/** Domain under which anchors are signed */
const ANCHOR_DOMAIN: &str = "trust/anchor/v1";

/** Per-repository view of the trust directory
 * Fields
    - home: PathBuf - the trust directory
    - repo_dir: PathBuf - home/repos/REPO_KEY: runtime evidence, anchors, integration secrets
    - key_path: PathBuf - home/keys/REPO_KEY.key: the repository's signing key seed
*/
#[derive(Debug, Clone)]
pub(crate) struct TrustStore {
    pub(crate) home: PathBuf,
    pub(crate) repo_dir: PathBuf,
    pub(crate) key_path: PathBuf,
}

impl TrustStore {
    /** Open the trust directory for a repository, keyed by the first 32 hex characters of its
     * repository identity digest
     * Input
        - repository_id: &str - repository identity digest
     * Output
        - Result<TrustStore, String>
        - Error if the trust directory cannot be determined
    */
    pub(crate) fn open(repository_id: &str) -> Result<Self, String> {
        let home = crane_home()?;
        let key = &repository_id[..repository_id.len().min(32)];
        Ok(Self {
            repo_dir: home.join("repos").join(key),
            key_path: home.join("keys").join(format!("{key}.key")),
            home,
        })
    }

    /** Return the runtime evidence directory (events, identities, ledger) of the repository
     * Input
        - None (uses self)
     * Output
        - PathBuf
    */
    pub(crate) fn runtime(&self) -> PathBuf {
        self.repo_dir.join("runtime")
    }

    /** Load the repository's signing key, if this machine holds it
     * Input
        - None (uses self)
     * Output
        - Result<Option<SigningIdentity>, String>, None when the key file does not exist
        - Error if the key file cannot be read or is malformed
    */
    pub(crate) fn signing_key(&self) -> Result<Option<SigningIdentity>, String> {
        match fs::read_to_string(&self.key_path) {
            Ok(seed) => SigningIdentity::from_seed_hex(&seed)
                .map(Some)
                .map_err(|error| format!("{}: {error}", self.key_path.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!(
                "could not read {}: {error}",
                self.key_path.display()
            )),
        }
    }

    /** Load the signing key, creating it (owner-only) when this machine has none; refused for
     * agents, since only a human may create signing authority
     * Input
        - None (uses self)
     * Output
        - Result<SigningIdentity, String>
        - Error if run by an agent or the key cannot be written
    */
    pub(crate) fn signing_key_or_create(&self) -> Result<SigningIdentity, String> {
        if let Some(identity) = self.signing_key()? {
            return Ok(identity);
        }
        require_human("creating a Crane signing key")?;
        let identity = SigningIdentity::generate()?;
        write_atomic(
            &self.key_path,
            format!("{}\n", identity.seed_hex()).as_bytes(),
        )?;
        restrict_to_owner(&self.key_path)?;
        Ok(identity)
    }

    /** Return the signing key, failing when this machine has none
     * Input
        - None (uses self)
     * Output
        - Result<SigningIdentity, String>
        - Error explaining that only the machine holding the key can change governance
    */
    pub(crate) fn require_signing_key(&self) -> Result<SigningIdentity, String> {
        self.signing_key()?.ok_or_else(|| {
            format!(
                "this machine has no Crane signing key for the repository ({} is missing); governance changes must be made where the key is held, or run 'crane init' to create one",
                self.key_path.display()
            )
        })
    }

    /** List the public keys trusted to sign this repository's registry: this machine's own key,
     * the comma-separated CRANE_TRUSTED_PUBLIC_KEYS, and the lines of home/trusted_keys (blank
     * lines and # comments ignored)
     * Input
        - None (uses self)
     * Output
        - Result<Vec<String>, String> lowercase hex public keys
        - Error if the own key file is malformed
    */
    pub(crate) fn trusted_public_keys(&self) -> Result<Vec<String>, String> {
        let mut keys = Vec::new();
        if let Some(identity) = self.signing_key()? {
            keys.push(identity.public_hex());
        }
        if let Ok(list) = env::var("CRANE_TRUSTED_PUBLIC_KEYS") {
            keys.extend(list.split(',').map(|key| key.trim().to_ascii_lowercase()));
        }
        if let Ok(list) = fs::read_to_string(self.home.join("trusted_keys")) {
            keys.extend(
                list.lines()
                    .map(|line| line.trim().to_ascii_lowercase())
                    .filter(|line| !line.is_empty() && !line.starts_with('#')),
            );
        }
        keys.retain(|key| key.len() == 64);
        keys.sort();
        keys.dedup();
        Ok(keys)
    }

    /** Read this machine's rollback anchor for the repository
     * Input
        - None (uses self)
     * Output
        - Result<Option<Anchor>, String>, None when the repository was never seen here
        - Error if the anchor file is unreadable or malformed
    */
    pub(crate) fn anchor(&self) -> Result<Option<Anchor>, String> {
        let path = self.repo_dir.join("anchor.json");
        match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("could not read {}: {error}", path.display())),
        }
    }

    /** Verify an anchor's signature against the trusted keys
     * Input
        - anchor: &Anchor - anchor to verify
     * Output
        - Result<(), String>
        - Error if no trusted key verifies it
    */
    pub(crate) fn verify_anchor(&self, anchor: &Anchor) -> Result<(), String> {
        let message = format!("{}:{}", anchor.generation, anchor.head);
        let keys = self.trusted_public_keys()?;
        if keys.iter().any(|key| {
            verify_signature(key, ANCHOR_DOMAIN, message.as_bytes(), &anchor.signature).is_ok()
        }) {
            Ok(())
        } else {
            Err("the rollback anchor in the trust directory has an invalid signature".into())
        }
    }

    /** Record a new rollback anchor, signed with the repository key
     * Input
        - identity: &SigningIdentity - signing key
        - generation: u64 - registry generation
        - head: &str - registry manifest head digest
     * Output
        - Result<(), String>
        - Error if the anchor cannot be written
    */
    pub(crate) fn set_anchor(
        &self,
        identity: &SigningIdentity,
        generation: u64,
        head: &str,
    ) -> Result<(), String> {
        let anchor = Anchor {
            generation,
            head: head.to_string(),
            updated_at: now_unix(),
            signature: identity.sign(ANCHOR_DOMAIN, format!("{generation}:{head}").as_bytes()),
        };
        let text = serde_json::to_string_pretty(&anchor).map_err(io_error)?;
        write_atomic(&self.repo_dir.join("anchor.json"), text.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::installation_key_in;

    /** Check that concurrent first uses create one upload key and later uses load it
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn installation_key_is_created_once() {
        let home = tempfile::tempdir().unwrap();
        let keys = std::thread::scope(|scope| {
            let handles = (0..8)
                .map(|_| scope.spawn(|| installation_key_in(home.path()).unwrap().public_hex()))
                .collect::<Vec<_>>();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect::<std::collections::BTreeSet<_>>()
        });
        assert_eq!(keys.len(), 1, "{keys:?}");
        let again = installation_key_in(home.path()).unwrap().public_hex();
        assert!(keys.contains(&again));
        let leftovers = std::fs::read_dir(home.path()).unwrap().count();
        assert_eq!(leftovers, 1, "only installation.key remains");
    }
}
