// crane init: create .crane and the owner's signing key in a new repository, or join one that is
// already set up (trust the owner's key after a human compares its fingerprint, or accept another
// admin's signed rules after a merge).

use super::*;
use crate::github;
use crate::governance::admins::{self};
use crate::governance::config::DEFAULT_POLICY;
use crate::governance::policy::{PolicySet, DEFAULT_FILE, EXTENSION};
use crate::governance::state::{commit, integrity, State};
use crate::governance::workspace::Workspace;
use crate::governance::Finding;
use crate::integrations::prompt::Prompter;
use crate::platform::files::write_atomic;
use crate::platform::render::iso_time;
use crate::trust::{require_human, TrustStore};
use serde_json::json;
use std::fs;

/** Initialize Crane: require a clone with a live GitHub connection, create .crane with the
 * default policy, configuration, empty checkpoint log, registry, map, and audit trail, create the
 * signing key in the trust directory, and sign generation 1; in a repository that is already
 * initialized, join it instead (see join)
 * Input
    - args: &[String] - no arguments are accepted
 * Output
    - Result<(), String>
    - Error if not a clone, not connected, or the state fails verification
*/
pub(crate) fn init(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: crane init".into());
    }
    let workspace = Workspace::discover()?;
    if workspace.remote.is_none() {
        return Err(
            "this repository has no origin remote; Crane works on a clone of a GitHub repository"
                .into(),
        );
    }
    let connection = github::require_connected(&workspace)?;
    println!(
        "Repository status: CONNECTED ({} via {})",
        connection.normalized,
        connection.method.name()
    );
    let state = State::load(&workspace)?;
    if state.registry.manifest.is_some() {
        return join(&workspace, &state);
    }
    require_human("crane init")?;
    fs::create_dir_all(workspace.policies_dir()).map_err(|error| error.to_string())?;
    let policies = PolicySet::load(&workspace)?;
    let default_file = workspace
        .policies_dir()
        .join(format!("{DEFAULT_FILE}.{EXTENSION}"));
    let created_default = policies.find(DEFAULT_POLICY).is_none() && !default_file.exists();
    if created_default {
        write_atomic(&default_file, empty_block(DEFAULT_POLICY).as_bytes())?;
    }
    let detail = json!({"remote": connection.normalized, "repository_id": workspace.repository_id});
    let committed = commit(
        &workspace,
        "crane init",
        "governance.initialized",
        detail,
        |_, _| Ok(()),
    );
    let state = match committed {
        Ok(state) => state,
        Err(error) => {
            if created_default {
                let _ = fs::remove_file(&default_file);
            }
            return Err(error);
        }
    };
    let trust = workspace.trust()?;
    let manifest = state
        .registry
        .manifest
        .as_ref()
        .ok_or("internal error: no manifest")?;
    println!(
        "Initialized {} (registry generation {})",
        workspace.crane.display(),
        manifest.generation
    );
    println!(
        "  default policy: {DEFAULT_POLICY} ({})",
        default_file
            .strip_prefix(&workspace.root)
            .unwrap_or(&default_file)
            .display()
    );
    println!(
        "  signing key:    {} (outside the repository; never commit or share it)",
        trust.key_path.display()
    );
    println!("  owner key:      {}", grouped(&manifest.public_key));
    println!("                  (teammates compare this when their 'crane init' asks to trust it)");
    // Best effort: let a GitHub admin vouch for the owner key, so joining teammates see it
    if let Ok(identity) = trust.require_signing_key() {
        match crate::cli::link::register_owner(&workspace, &state, &identity) {
            Ok(url) => println!("  GitHub check:   confirm the owner key as a GitHub admin of the repository (within 10 minutes): {url}"),
            Err(error) if crate::telemetry::ingest::endpoint().is_some() => println!(
                "  GitHub check:   not registered ({error}); run 'crane admin register' later"
            ),
            Err(_) => {}
        }
    }
    match crate::telemetry::ingest::endpoint() {
        Some(url) => println!(
            "  telemetry:      events and metrics are uploaded to {url}, signed with this machine's upload key (set CRANE_INGEST_URL=off to keep them local)"
        ),
        None => println!("  telemetry:      local only (no ingestion service in this build)"),
    }
    println!("Next: 'crane checkpoint NAME' to record a trusted baseline, then 'crane protect FILE ...'.");
    Ok(())
}

/** Join an initialized repository, or re-check it. When the state verifies, nothing changes. When
 * it would verify except that this machine does not trust the owner's key yet (a teammate's first
 * run), show the owner key's fingerprint and the last signed change, and pin the key once the human
 * confirms it; this machine's own key is created for rollback detection and for becoming an admin.
 * When the only problem is a fork (another admin's change was kept after a merge), show the signed
 * state and move this machine's rollback anchor to it once the human confirms. Anything else fails.
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
 * Output
    - Result<(), String>
    - Error if the state fails verification for any other reason, or the human declines
*/
pub(super) fn join(workspace: &Workspace, state: &State) -> Result<(), String> {
    let trust = workspace.trust()?;
    let manifest = state
        .registry
        .manifest
        .as_ref()
        .ok_or("internal error: no manifest")?;
    let owner = admins::owner(state).ok_or("internal error: no owner")?;
    let blocking = |trust: &TrustStore| {
        integrity(workspace, state, trust)
            .into_iter()
            .filter(Finding::blocking)
            .collect::<Vec<_>>()
    };
    let found = blocking(&trust);
    if found.is_empty() {
        println!(
            "Crane is already initialized in {} (registry generation {}); nothing changed",
            workspace.crane.display(),
            state.generation()
        );
        println!("  owner key: {}", grouped(&owner));
        println!("  this machine: {}", role(state, &trust)?);
        return Ok(());
    }
    let describe = || {
        println!(
            "  last change: generation {} by {} at {}",
            manifest.generation,
            manifest.updated_by,
            iso_time(manifest.updated_at * 1000)
        );
        if let Some(statement) = &state.config.admins {
            let names = statement
                .admins
                .iter()
                .map(|admin| admin.name.as_str())
                .collect::<Vec<_>>();
            println!("  admins besides the owner: {}", names.join(", "));
        }
    };
    let mut candidate = trust.clone();
    candidate.candidates.push(owner.clone());
    if !trust.trusted_public_keys()?.contains(&owner) && blocking(&candidate).is_empty() {
        require_human("trusting a repository owner key")?;
        println!(
            "This repository's rules are signed, but this machine does not trust their owner yet."
        );
        println!("  owner key:   {}", grouped(&owner));
        describe();
        println!("Ask the repository owner for their key ('crane admin list' shows it) and compare every group.");
        let mismatch = crate::cli::link::show_owner_confirmation(workspace, &owner);
        if mismatch {
            println!("Answer 'n' unless the owner confirmed to you that their key changed.");
        }
        if !Prompter::new().confirm("Trust this owner key for this repository?", false)? {
            return Err("the owner key was not trusted; nothing changed, and agent changes stay blocked until it is".into());
        }
        trust.pin(&owner)?;
        let identity = trust.signing_key_or_create()?;
        trust.set_anchor(&identity, manifest.generation, &manifest.head)?;
        println!("Trusted. Crane now verifies this repository's rules on this machine.");
        println!(
            "  your key: {} (to become an admin, send it to the owner: crane admin add KEY name NAME)",
            grouped(&identity.public_hex())
        );
        return Ok(());
    }
    if found.iter().all(|finding| finding.code == "registry_fork") {
        require_human("accepting another admin's signed rules")?;
        println!("The signed rules differ from the version this machine last signed at the same generation. This is expected after merging another admin's change (keeping their side of .crane).");
        describe();
        if !Prompter::new().confirm("Accept the current signed rules?", false)? {
            return Err("not accepted; nothing changed".into());
        }
        let identity = trust.require_signing_key()?;
        trust.set_anchor(&identity, manifest.generation, &manifest.head)?;
        println!(
            "Accepted generation {}. Re-apply your own change with crane to sign it on top.",
            manifest.generation
        );
        return Ok(());
    }
    Err(format!(
        "Crane is initialized but its state fails verification: {}",
        found
            .into_iter()
            .map(|finding| finding.message)
            .collect::<Vec<_>>()
            .join("; ")
    ))
}
