// crane admin list|add|del: the owner's signed list of admins who may change the rules.

use super::*;
use crate::governance::admins::{self, Admin, AdminStatement};
use crate::governance::state::{commit, State};
use crate::governance::workspace::Workspace;
use crate::platform::now_unix;
use crate::platform::render::iso_time;
use crate::trust::require_human;
use serde_json::json;

/** Manage the repository's admins: list them, or (owner only) add or remove one
 * Input
    - args: &[String] - list | add PUBLIC_KEY [name NAME] | del PUBLIC_KEY
 * Output
    - Result<(), String>
*/
pub(crate) fn admin(args: &[String]) -> Result<(), String> {
    let usage = "usage: crane admin list | crane admin add PUBLIC_KEY [name NAME] | crane admin del PUBLIC_KEY";
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let (adding, key, name) = match words.as_slice() {
        ["list"] => return admin_list(),
        ["add", key] => (true, *key, "admin"),
        ["add", key, "name", name] => (true, *key, *name),
        ["del", key] => (false, *key, ""),
        _ => return Err(usage.into()),
    };
    let key = key.replace(' ', "").to_ascii_lowercase();
    if key.len() != 64 || !key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("a public key is 64 hexadecimal characters (spaces are ignored)".into());
    }
    require_human(if adding {
        "crane admin add"
    } else {
        "crane admin del"
    })?;
    let workspace = Workspace::locate()?;
    let trust = workspace.trust()?;
    let identity = trust.require_signing_key()?;
    let state = State::load(&workspace)?;
    let owner = admins::owner(&state).ok_or("the repository is not initialized")?;
    if identity.public_hex() != owner {
        return Err(format!(
            "only the repository owner can change admins (owner key {})",
            grouped(&owner)
        ));
    }
    let repository_id = workspace.repository_id.clone();
    let name = name.to_string();
    let detail = json!({"public_key": key, "name": name});
    let (command, action) = if adding {
        ("crane admin add", "governance.admin_added")
    } else {
        ("crane admin del", "governance.admin_removed")
    };
    let state = commit(&workspace, command, action, detail, |state, _| {
        let current = state.config.admins.clone();
        let mut list = current
            .as_ref()
            .map(|statement| statement.admins.clone())
            .unwrap_or_default();
        let present = list.iter().any(|admin| admin.public_key == key);
        if adding {
            if key == owner {
                return Err("the owner is already an admin".into());
            }
            if present {
                return Err("this key is already an admin".into());
            }
            list.push(Admin {
                public_key: key.clone(),
                name: name.clone(),
                added_at: now_unix(),
            });
        } else {
            if !present {
                return Err("this key is not an admin".into());
            }
            list.retain(|admin| admin.public_key != key);
        }
        let epoch = current.map_or(0, |statement| statement.epoch) + 1;
        state.config.admins = Some(AdminStatement::sign(&identity, &repository_id, epoch, list));
        Ok(())
    })?;
    println!(
        "{} admin {} (admin list version {}, registry generation {}). Commit .crane so teammates receive it.",
        if adding { "Added" } else { "Removed" },
        grouped(&key),
        state.config.admins.as_ref().map_or(0, |statement| statement.epoch),
        state.generation()
    );
    Ok(())
}

/** Print the owner, the admins, and this machine's role
 * Input
    - None
 * Output
    - Result<(), String>
*/
pub(super) fn admin_list() -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let trust = workspace.trust()?;
    let state = State::load(&workspace)?;
    let owner = admins::owner(&state).ok_or("the repository is not initialized")?;
    println!("owner: {}", grouped(&owner));
    match &state.config.admins {
        Some(statement) if !statement.admins.is_empty() => {
            println!("admins (list version {}):", statement.epoch);
            for admin in &statement.admins {
                println!(
                    "  {}  {} (added {})",
                    grouped(&admin.public_key),
                    admin.name,
                    iso_time(admin.added_at * 1000)
                );
            }
        }
        _ => println!("admins: none besides the owner"),
    }
    println!("this machine: {}", role(&state, &trust)?);
    Ok(())
}
