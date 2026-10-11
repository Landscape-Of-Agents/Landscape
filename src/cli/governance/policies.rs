// Policy commands: create a policy, move a statement between policies, attach and view policy
// context, and policy status (the policy's commands plus an enforcement self-test and CI checks).

use super::*;
use crate::cli::args::Args;
use crate::governance::changes::ChangeType;
use crate::governance::context::{Binding, MAX_CONTEXT_BYTES};
use crate::governance::policy::{
    insert_statement, parse as parse_policy, remove_statement, render_statement, PolicySet,
    EXTENSION,
};
use crate::governance::state::{commit, State};
use crate::governance::verify::{evaluate, Options, Outcome};
use crate::governance::workspace::Workspace;
use crate::governance::Finding;
use crate::platform::files::{is_text, rewrite_preserving, write_atomic};
use crate::platform::{actor, now_unix, validate_name};
use crate::trust::crypto::{is_selection_id, sha512_hex};
use crate::trust::require_human;
use serde_json::json;
use std::fs;

/** Bind a text file to a policy as context supplied to agents with that policy
 * Input
    - args: &[String] - FILE and optional "policy NAME" (default policy when absent)
 * Output
    - Result<(), String>
    - Error if run by an agent, the file is missing, not text, or too large, or the policy does
      not exist
*/
pub(crate) fn add_context(args: &[String]) -> Result<(), String> {
    require_human("crane add policy-context")?;
    let parsed = Args::parse(args, &["policy"], &[])?;
    parsed.expect_positional(1, "crane add policy-context file FILE [policy NAME]")?;
    let workspace = Workspace::locate()?;
    let path = workspace.resolve_file(&parsed.positional[0])?;
    let bytes = fs::read(workspace.root.join(&path))
        .map_err(|error| format!("could not read {path}: {error}"))?;
    if !is_text(&bytes) {
        return Err(format!("{path} is not a text file"));
    }
    if bytes.len() > MAX_CONTEXT_BYTES {
        return Err(format!(
            "{path} is larger than {} KB",
            MAX_CONTEXT_BYTES / 1024
        ));
    }
    let state = State::load(&workspace)?;
    let policy = parsed
        .value("policy")
        .or(state.config.default_policy)
        .ok_or("no policy was given and no default policy is set")?;
    if PolicySet::load(&workspace)?.find(&policy).is_none() {
        return Err(format!("policy '{policy}' does not exist"));
    }
    let binding = Binding {
        policy: policy.clone(),
        path: path.clone(),
        digest: sha512_hex(&bytes),
        bound_at: now_unix(),
        bound_by: actor(),
    };
    let mut replaced = false;
    commit(
        &workspace,
        "crane add policy-context",
        "context.bound",
        json!({"policy": policy, "path": path, "digest": binding.digest}),
        |state, _| {
            replaced = state.context.bind(binding.clone());
            Ok(())
        },
    )?;
    println!(
        "{} {path} as context of policy {policy}; agents receive it with the policy (context is guidance, never authorization)",
        if replaced { "Refreshed" } else { "Bound" }
    );
    Ok(())
}

/** Show the context files bound to a policy, with their text
 * Input
    - args: &[String] - POLICY and --view
 * Output
    - Result<(), String>
    - Error if --view is missing or the policy does not exist
*/
pub(crate) fn view_context(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["view"])?;
    if parsed.positional.len() != 1 || !parsed.flag("view") {
        return Err("usage: crane policy-context NAME --view".into());
    }
    let policy = &parsed.positional[0];
    let workspace = Workspace::locate()?;
    if PolicySet::load(&workspace)?.find(policy).is_none() {
        return Err(format!("policy '{policy}' does not exist"));
    }
    let state = State::load(&workspace)?;
    let bindings = state.context.of(policy);
    if bindings.is_empty() {
        println!("Policy {policy} has no context files; add one with 'crane add policy-context file FILE policy {policy}'");
        return Ok(());
    }
    for binding in bindings {
        let current = fs::read(workspace.root.join(&binding.path)).ok();
        let freshness = match &current {
            None => "MISSING",
            Some(bytes) if sha512_hex(bytes) == binding.digest => "current",
            Some(_) => "STALE (changed since it was bound)",
        };
        println!("== {} ({freshness})", binding.path);
        if let Some(bytes) = current {
            println!("{}", String::from_utf8_lossy(&bytes).trim_end());
        }
    }
    Ok(())
}

/** Create a policy in .crane/policies/FILENAME.crane (appended when the file exists)
 * Input
    - args: &[String] - POLICY FILENAME
 * Output
    - Result<(), String>
    - Error if run by an agent, FILENAME has the .crane extension or is invalid, or the policy
      already exists
*/
pub(crate) fn create_policy(args: &[String]) -> Result<(), String> {
    require_human("crane create policy")?;
    let parsed = Args::parse(args, &[], &[])?;
    parsed.expect_positional(2, "crane create policy NAME FILENAME")?;
    let (name, file_name) = (&parsed.positional[0], &parsed.positional[1]);
    validate_name("policy", name)?;
    if file_name.to_ascii_lowercase().ends_with(".crane") {
        return Err(format!(
            "FILENAME must not include the .crane extension (use '{}')",
            &file_name[..file_name.len() - 6]
        ));
    }
    validate_name("file", file_name)?;
    let workspace = Workspace::locate()?;
    let policies = PolicySet::load(&workspace)?;
    if let Some((path, error)) = policies.errors.first() {
        return Err(format!("fix the policy syntax first: {path}: {error}"));
    }
    if let Some((file, block)) = policies
        .blocks()
        .into_iter()
        .find(|(_, block)| block.name.eq_ignore_ascii_case(name))
    {
        return Err(format!(
            "policy '{}' already exists in {}",
            block.name, file.path
        ));
    }
    let path = workspace
        .policies_dir()
        .join(format!("{file_name}.{EXTENSION}"));
    let original = fs::read_to_string(&path).ok();
    let text = match &original {
        Some(text) if !text.trim().is_empty() => {
            format!("{}\n\n{}", text.trim_end(), empty_block(name))
        }
        _ => empty_block(name),
    };
    write_atomic(&path, text.as_bytes())?;
    let relative = format!(".crane/policies/{file_name}.{EXTENSION}");
    if let Err(error) = commit(
        &workspace,
        "crane create policy",
        "policy.created",
        json!({"policy": name, "file": relative}),
        |_, _| Ok(()),
    ) {
        match original {
            Some(text) => write_atomic(&path, text.as_bytes())?,
            None => {
                let _ = fs::remove_file(&path);
            }
        }
        return Err(error);
    }
    println!("Created policy {name} in {relative}");
    Ok(())
}

/** Move a selection's command into a policy (removing it from the policy that held it), so
 * 'crane add policy default marker ID' returns a command to the default policy
 * Input
    - args: &[String] - POLICY "marker" MARKER
 * Output
    - Result<(), String>
    - Error if run by an agent, the marker is not registered, or the policy does not exist
*/
pub(crate) fn move_marker(args: &[String]) -> Result<(), String> {
    require_human("crane add policy")?;
    let parsed = Args::parse(args, &["marker"], &[])?;
    parsed.expect_positional(1, "crane add policy NAME marker MARKER")?;
    let target = parsed.positional[0].clone();
    let marker = parsed
        .value("marker")
        .ok_or("usage: crane add policy NAME marker MARKER")?;
    if !is_selection_id(&marker) {
        return Err(format!(
            "'{marker}' is not a selection marker (8 characters A-Z, 0-9)"
        ));
    }
    let workspace = Workspace::locate()?;
    let state = State::load(&workspace)?;
    let record = state
        .record(&marker)
        .cloned()
        .ok_or_else(|| format!("marker {marker} is not a registered selection"))?;
    let policies = PolicySet::load(&workspace)?;
    if let Some((path, error)) = policies.errors.first() {
        return Err(format!("fix the policy syntax first: {path}: {error}"));
    }
    policies
        .find(&target)
        .ok_or_else(|| format!("policy '{target}' does not exist"))?;
    let references = policies.references(&marker);
    if record.policy == target && references.len() == 1 && references[0].1.name == target {
        println!("Selection {marker} is already in policy {target}; nothing changed");
        return Ok(());
    }
    let change_type = record
        .change_type
        .as_deref()
        .map(ChangeType::parse)
        .transpose()?;
    let statement = render_statement(record.operation, &marker, change_type);
    // Remove every existing reference, file by file, then insert into the target block
    let mut originals = Vec::new();
    let mut texts = std::collections::BTreeMap::new();
    for file in &policies.files {
        texts.insert(file.path.clone(), file.text.clone());
        originals.push((file.path.clone(), file.text.clone()));
    }
    loop {
        let mut changed = false;
        for (path, text) in texts.iter_mut() {
            let file = parse_policy(path, text).map_err(|error| format!("{path}: {error}"))?;
            if let Some(statement) = file
                .blocks
                .iter()
                .flat_map(|block| block.statements.iter())
                .find(|statement| statement.id == marker)
            {
                *text = remove_statement(text, statement);
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    let target_path = policies
        .find(&target)
        .map(|(file, _)| file.path.clone())
        .unwrap_or_default();
    let target_text = texts.get(&target_path).cloned().unwrap_or_default();
    let parsed_target = parse_policy(&target_path, &target_text)
        .map_err(|error| format!("{target_path}: {error}"))?;
    let block = parsed_target
        .blocks
        .iter()
        .find(|block| block.name == target)
        .ok_or("internal error: target policy vanished")?;
    texts.insert(
        target_path,
        insert_statement(&target_text, block, &statement),
    );
    for (path, text) in &texts {
        if originals
            .iter()
            .any(|(original_path, original)| original_path == path && original != text)
        {
            rewrite_preserving(&workspace.root.join(path), text.as_bytes())?;
        }
    }
    let from = record.policy;
    let result = commit(
        &workspace,
        "crane add policy",
        "selection.policy_changed",
        json!({"id": marker, "from": from, "to": target}),
        |state, _| {
            let record = state
                .registry
                .selections
                .iter_mut()
                .find(|record| record.id == marker)
                .ok_or("internal error: record vanished")?;
            record.policy = target.clone();
            record.generation += 1;
            Ok(())
        },
    );
    if let Err(error) = result {
        for (path, text) in &originals {
            let _ = rewrite_preserving(&workspace.root.join(path), text.as_bytes());
        }
        return Err(error);
    }
    println!("Moved {statement} from policy {from} to policy {target}");
    Ok(())
}

/** One line of the policy status report
 * Fields
    - check: String - what was checked
    - status: &'static str - PASS, FAIL, or WARN
    - detail: String - explanation
*/
pub(super) struct Check {
    check: String,
    status: &'static str,
    detail: String,
}

/** Validate and execute one policy end to end: syntax, every command's test, the enforcement
 * boundary (simulated pre-agent write and read decisions at the hook for each selection, the map,
 * and protected commands), the installed hooks of Claude Code and Codex, the post-write
 * verification path, and whether CI/CD pipelines run Crane
 * Input
    - args: &[String] - POLICY and optional --json
 * Output
    - Result<(), String>
    - Error if the policy does not exist or any check fails
*/
pub(crate) fn policy_status(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["json"])?;
    parsed.expect_positional(1, "crane policy NAME status [--json]")?;
    let name = parsed.positional[0].clone();
    let workspace = Workspace::locate()?;
    let policies = PolicySet::load(&workspace)?;
    let mut checks = Vec::new();
    let found = policies.find(&name);
    if found.is_none() && policies.errors.is_empty() {
        return Err(format!("policy '{name}' does not exist"));
    }
    match found {
        Some((file, _)) => checks.push(Check {
            check: "syntax".into(),
            status: "PASS",
            detail: format!("{} parses", file.path),
        }),
        None => {
            for (path, error) in &policies.errors {
                checks.push(Check {
                    check: "syntax".into(),
                    status: "FAIL",
                    detail: format!("{path}: {error}"),
                });
            }
        }
    }
    let (report, state) = evaluate(&workspace, &Options::default())?;
    let relevant = |finding: &&Finding| {
        finding.blocking()
            && (finding
                .selection
                .as_ref()
                .is_none_or(|id| state.record(id).is_some_and(|record| record.policy == name))
                || finding.selection.is_none())
    };
    for finding in report.findings.iter().filter(relevant) {
        checks.push(Check {
            check: format!("integrity: {}", finding.code),
            status: "FAIL",
            detail: finding.message.clone(),
        });
    }
    if let Some(policy) = report.policies.iter().find(|policy| policy.name == name) {
        for command in &policy.commands {
            checks.push(Check {
                check: format!(
                    "test: {}",
                    render_statement(command.operation, &command.id, command.change_type)
                        .trim_end_matches(';')
                ),
                status: if command.outcome == Outcome::Pass {
                    "PASS"
                } else {
                    "FAIL"
                },
                detail: command.message.clone(),
            });
        }
        if policy.commands.is_empty() {
            checks.push(Check {
                check: "test".into(),
                status: "PASS",
                detail: "the policy has no commands yet".into(),
            });
        }
    }
    for (check, status, detail) in crate::hooks::enforce::self_test(&workspace, &state, &name) {
        checks.push(Check {
            check,
            status,
            detail,
        });
    }
    for (check, status, detail) in crate::hooks::install::status_checks() {
        checks.push(Check {
            check,
            status,
            detail,
        });
    }
    checks.push(ci_check(&workspace));
    let failed = checks.iter().any(|check| check.status == "FAIL");
    if parsed.flag("json") {
        let rows = checks.iter().map(|check| json!({"check": check.check, "status": check.status, "detail": check.detail})).collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"policy": name, "passed": !failed, "checks": rows})
            )
            .map_err(|error| error.to_string())?
        );
    } else {
        println!("Policy {name} status");
        for check in &checks {
            println!("  {:<4} {}: {}", check.status, check.check, check.detail);
        }
        println!("Policy {name}: {}", if failed { "FAIL" } else { "PASS" });
    }
    if failed {
        Err(format!("policy {name} failed one or more checks"))
    } else {
        Ok(())
    }
}

/** Check whether a CI/CD pipeline in .github/workflows runs Crane
 * Input
    - workspace: &Workspace - repository
 * Output
    - Check, WARN when no workflow runs crane test or crane validate
*/
pub(super) fn ci_check(workspace: &Workspace) -> Check {
    let directory = workspace.root.join(".github").join("workflows");
    let runs_crane = fs::read_dir(&directory)
        .map(|entries| {
            entries.filter_map(Result::ok).any(|entry| {
                fs::read_to_string(entry.path()).is_ok_and(|text| {
                    text.contains("crane test") || text.contains("crane validate")
                })
            })
        })
        .unwrap_or(false);
    if runs_crane {
        Check {
            check: "ci/cd".into(),
            status: "PASS",
            detail: "a workflow in .github/workflows runs Crane".into(),
        }
    } else {
        Check {
            check: "ci/cd".into(),
            status: "WARN",
            detail: "no workflow in .github/workflows runs 'crane test .' or 'crane validate'; changes made outside agent hooks are only caught where Crane runs".into(),
        }
    }
}
