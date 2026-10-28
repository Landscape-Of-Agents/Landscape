// The local credential vault. Environment Bricks name credentials (committed and shared); their
// values live only here, on each machine, in the repository's trust directory: vault/NAME.json,
// sealed with the Windows Data Protection API on Windows (a copied file or another account cannot
// read it) and owner-only elsewhere. A value is set by a person ('crane secret set NAME', read from
// standard input or a hidden prompt, never from an argument), handed only to Crane-run operations
// (resident agents and skills), and never to an AI agent's tool calls, events, logs, or the
// dashboard. Agents cannot reach it either: the guard denies every access to the trust directory.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::crypto::{hex, unhex};
use crate::governance::workspace::Workspace;
use crate::platform::files::{restrict_to_owner, write_atomic};

/** Layout version of vault entries */
const FORMAT: &str = "crane-vault/v1";

/** The largest value the vault keeps, in bytes */
const MAX_VALUE: usize = 16 * 1024;

/** One stored credential
 * Fields
    - format: String - layout version
    - name: String - vault name
    - protection: String - "dpapi" or "owner-only"
    - data: String - the sealed (dpapi) or plain (owner-only) value in hex
    - set_by: String - who set it
    - set_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    format: String,
    name: String,
    protection: String,
    data: String,
    set_by: String,
    set_at: u64,
}

/** What a listing shows of an entry (never the value)
 * Fields
    - name: String - vault name
    - protection: String - how it is kept
    - set_by: String - who set it
    - set_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Listed {
    pub(crate) name: String,
    pub(crate) protection: String,
    pub(crate) set_by: String,
    pub(crate) set_at: u64,
}

/** The vault directory of the repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<PathBuf, String>
*/
fn directory(workspace: &Workspace) -> Result<PathBuf, String> {
    Ok(workspace.trust()?.repo_dir.join("vault"))
}

/** Check a vault name
 * Input
    - name: &str - name
 * Output
    - Result<(), String>
*/
fn check_name(name: &str) -> Result<(), String> {
    crate::platform::validate_name("credential", name)
}

/** Report whether the vault holds a credential
 * Input
    - workspace: &Workspace - repository
    - name: &str - vault name
 * Output
    - bool
*/
pub(crate) fn exists(workspace: &Workspace, name: &str) -> bool {
    check_name(name).is_ok()
        && directory(workspace)
            .is_ok_and(|directory| directory.join(format!("{name}.json")).is_file())
}

/** Store a credential, replacing an earlier value
 * Input
    - workspace: &Workspace - repository
    - name: &str - vault name
    - value: &str - the credential
 * Output
    - Result<String, String> how it is kept
*/
pub(crate) fn set(workspace: &Workspace, name: &str, value: &str) -> Result<String, String> {
    check_name(name)?;
    if value.is_empty() {
        return Err("the credential is empty".into());
    }
    if value.len() > MAX_VALUE {
        return Err(format!("the credential is larger than {MAX_VALUE} bytes"));
    }
    let (protection, data) = if cfg!(windows) {
        ("dpapi", super::keys::dpapi::protect(value.as_bytes())?)
    } else {
        ("owner-only", value.as_bytes().to_vec())
    };
    let entry = Entry {
        format: FORMAT.into(),
        name: name.into(),
        protection: protection.into(),
        data: hex(&data),
        set_by: crate::platform::actor(),
        set_at: crate::platform::now_millis(),
    };
    let directory = directory(workspace)?;
    fs::create_dir_all(&directory).map_err(crate::platform::io_error)?;
    let path = directory.join(format!("{name}.json"));
    write_atomic(
        &path,
        serde_json::to_string(&entry)
            .map_err(crate::platform::io_error)?
            .as_bytes(),
    )?;
    restrict_to_owner(&path)?;
    Ok(protection.into())
}

/** Read a credential (for a Crane-run operation only)
 * Input
    - workspace: &Workspace - repository
    - name: &str - vault name
 * Output
    - Result<Option<String>, String> None when it is not stored
*/
pub(crate) fn get(workspace: &Workspace, name: &str) -> Result<Option<String>, String> {
    check_name(name)?;
    let path = directory(workspace)?.join(format!("{name}.json"));
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(None);
    };
    let entry: Entry = serde_json::from_str(&text)
        .map_err(|error| format!("vault entry {name} is unreadable: {error}"))?;
    if entry.format != FORMAT {
        return Err(format!("vault entry {name} has an unknown format"));
    }
    let data = unhex(&entry.data)?;
    let plain = match entry.protection.as_str() {
        "dpapi" => super::keys::dpapi::unprotect(&data)?,
        "owner-only" => data,
        other => {
            return Err(format!(
                "vault entry {name} uses an unknown protection '{other}'"
            ))
        }
    };
    String::from_utf8(plain)
        .map(Some)
        .map_err(|_| format!("vault entry {name} is not text"))
}

/** The credential Environment Bricks name for an operation or integration ("jira"), when this
 * machine's vault holds it: the first by target key, then by Brick, so the choice is stable
 * Input
    - workspace: &Workspace - repository
    - operation: &str - integration name or operation id
 * Output
    - Option<String> the value
*/
pub(crate) fn for_operation(workspace: &Workspace, operation: &str) -> Option<String> {
    let (_, store, _) = crate::brick::store::load(workspace).ok()?;
    let targets = store
        .attachments
        .iter()
        .map(|attachment| attachment.target.key())
        .collect::<std::collections::BTreeSet<_>>();
    for target in targets {
        let (configs, _) = crate::brick::compose::configs_of(&store, &target);
        for (_, _, config) in configs {
            let crate::brick::config::Config::Environment(environment) = config else {
                continue;
            };
            for credential in environment.credentials {
                if credential
                    .for_operations
                    .iter()
                    .any(|name| name == operation)
                {
                    if let Ok(Some(value)) = get(workspace, &credential.name) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}

/** List the stored credentials (names and metadata, never values)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Vec<Listed>, String>
*/
pub(crate) fn list(workspace: &Workspace) -> Result<Vec<Listed>, String> {
    let directory = directory(workspace)?;
    let Ok(entries) = fs::read_dir(&directory) else {
        return Ok(Vec::new());
    };
    let mut listed = entries
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Entry>(&text).ok())
        .map(|entry| Listed {
            name: entry.name,
            protection: entry.protection,
            set_by: entry.set_by,
            set_at: entry.set_at,
        })
        .collect::<Vec<_>>();
    listed.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(listed)
}

/** Remove a credential
 * Input
    - workspace: &Workspace - repository
    - name: &str - vault name
 * Output
    - Result<bool, String> whether it existed
*/
pub(crate) fn remove(workspace: &Workspace, name: &str) -> Result<bool, String> {
    check_name(name)?;
    let path = directory(workspace)?.join(format!("{name}.json"));
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(crate::platform::io_error(error)),
    }
}
