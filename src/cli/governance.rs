// Governance commands: init, checkpoint, protect, target, defaults, parse, validate, test, policy
// contexts, policy creation, moving commands between policies, and policy status.

use std::fs;
use std::path::Path;
use std::time::Duration;

use serde_json::json;

use super::args::Args;
use crate::github;
use crate::governance::changes::ChangeType;
use crate::governance::config::DEFAULT_POLICY;
use crate::governance::context::{Binding, MAX_CONTEXT_BYTES};
use crate::governance::policy::{
    insert_statement, parse as parse_policy, remove_statement, render_statement, PolicySet,
    DEFAULT_FILE, EXTENSION,
};
use crate::governance::state::{commit, integrity, State};
use crate::governance::verify::{evaluate, Options, Outcome, Report};
use crate::governance::workspace::Workspace;
use crate::governance::{Finding, Severity};
use crate::platform::files::{is_text, rewrite_preserving, write_atomic};
use crate::platform::{actor, git, now_unix, process, validate_name};
use crate::selection::create::{create, Request};
use crate::selection::registry::Operation;
use crate::trust::crypto::{is_selection_id, sha512_hex};
use crate::trust::{agent_environment, require_human};

/** Text of a new policy block
 * Input
    - name: &str - policy name
 * Output
    - String
*/
fn empty_block(name: &str) -> String {
    format!("policy {name} {{\n}}\n")
}

/** Initialize Crane: require a clone with a live GitHub connection, create .crane with the
 * default policy, configuration, empty checkpoint log, registry, map, and audit trail, create the
 * signing key in the trust directory, and sign generation 1; running it again verifies and changes
 * nothing
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
        let trust = workspace.trust()?;
        let blocking = integrity(&workspace, &state, &trust)
            .into_iter()
            .filter(Finding::blocking)
            .map(|finding| finding.message)
            .collect::<Vec<_>>();
        if !blocking.is_empty() {
            return Err(format!(
                "Crane is initialized but its state fails verification: {}",
                blocking.join("; ")
            ));
        }
        println!(
            "Crane is already initialized in {} (registry generation {}); nothing changed",
            workspace.crane.display(),
            state.generation()
        );
        return Ok(());
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
    println!("  public key:     {}", manifest.public_key);
    match crate::telemetry::ingest::endpoint() {
        Some(url) => println!(
            "  telemetry:      events and metrics are uploaded to {url}, signed with this machine's upload key (set CRANE_INGEST_URL=off to keep them local)"
        ),
        None => println!("  telemetry:      local only (no ingestion service in this build)"),
    }
    println!("Next: 'crane checkpoint NAME' to record a trusted baseline, then 'crane protect FILE ...'.");
    Ok(())
}

/** Record HEAD as a trusted checkpoint (insert-only); the first checkpoint becomes the default
 * Input
    - args: &[String] - optional NAME
 * Output
    - Result<(), String>
    - Error if run by an agent, not connected, the name is taken or invalid, or HEAD is missing
*/
pub(crate) fn checkpoint(args: &[String]) -> Result<(), String> {
    require_human("crane checkpoint")?;
    let parsed = Args::parse(args, &[], &[])?;
    if parsed.positional.len() > 1 {
        return Err("usage: crane checkpoint [NAME]".into());
    }
    let workspace = Workspace::locate()?;
    github::require_connected(&workspace)?;
    let state = State::load(&workspace)?;
    let name = parsed.positional.first().cloned().unwrap_or_else(|| {
        if state.checkpoints.checkpoints.is_empty() {
            "baseline".into()
        } else {
            format!("checkpoint-{}", state.checkpoints.checkpoints.len() + 1)
        }
    });
    validate_name("checkpoint", &name)?;
    let commit_sha = git::head(&workspace.root)?;
    let tree = git::git_in(
        &workspace.root,
        &["rev-parse", &format!("{commit_sha}^{{tree}}")],
    )?;
    let branch = git::branch(&workspace.root);
    let dirty = !git::git_in(
        &workspace.root,
        &["status", "--porcelain", "--untracked-files=no"],
    )?
    .is_empty();
    let mut became_default = false;
    let detail = json!({"name": name, "commit": commit_sha, "branch": branch});
    commit(
        &workspace,
        "crane checkpoint",
        "checkpoint.created",
        detail,
        |state, _| {
            state
                .checkpoints
                .append(&name, &commit_sha, &tree, &branch, now_unix(), &actor())?;
            if state.config.default_checkpoint.is_none() {
                state.config.default_checkpoint = Some(name.clone());
                became_default = true;
            }
            Ok(())
        },
    )?;
    println!("Created checkpoint '{name}' at {commit_sha} (branch {branch})");
    if became_default {
        println!("'{name}' is now the default checkpoint");
    }
    if dirty {
        println!(
            "note: the working tree has uncommitted changes; the checkpoint records HEAD only"
        );
    }
    Ok(())
}

/** Create a selection with crane protect (preserve) or crane target
 * Input
    - operation: Operation - preserve or target
    - args: &[String] - FILE and keyword options
 * Output
    - Result<(), String>
*/
pub(crate) fn select(operation: Operation, args: &[String]) -> Result<(), String> {
    let command = match operation {
        Operation::Preserve => "protect",
        Operation::Target => "target",
    };
    require_human(&format!("crane {command}"))?;
    let keys: &[&str] = match operation {
        Operation::Preserve => &["policy", "checkpoint", "start-line", "end-line", "name"],
        Operation::Target => &[
            "policy",
            "checkpoint",
            "start-line",
            "end-line",
            "name",
            "change-type",
        ],
    };
    let parsed = Args::parse(args, keys, &[])?;
    parsed.expect_positional(
        1,
        &format!(
            "crane {command} FILE [policy NAME] [checkpoint NAME] [start-line N] [end-line N]"
        ),
    )?;
    let request = Request {
        operation,
        file: parsed.positional[0].clone(),
        policy: parsed.value("policy"),
        checkpoint: parsed.value("checkpoint"),
        start_line: parsed.line("start-line")?,
        end_line: parsed.line("end-line")?,
        name: parsed.value("name"),
        change_type: parsed
            .value("change-type")
            .map(|value| ChangeType::parse(&value))
            .transpose()?,
    };
    let record = create(&request)?;
    println!(
        "{} {} lines {}-{} as selection {} (policy {}, checkpoint {})",
        match operation {
            Operation::Preserve => "Protected",
            Operation::Target => "Targeted",
        },
        record.current.span.path,
        record.origin.span.start_line,
        record.origin.span.end_line,
        record.id,
        record.policy,
        record.origin.checkpoint
    );
    println!(
        "  marker:         @crane:selection:{}:start / :end (in {} comment syntax)",
        record.id, record.language
    );
    println!(
        "  origin digest:  {} (SHA-512)",
        record.origin.binding_digest
    );
    println!(
        "  added to policy {}: {}",
        record.policy,
        render_statement(record.operation, &record.id, request.change_type)
    );
    println!(
        "The anchor comments modify {} and will appear in its Git diff.",
        record.current.span.path
    );
    Ok(())
}

/** Set the default policy
 * Input
    - args: &[String] - POLICY
 * Output
    - Result<(), String>
    - Error if run by an agent or the policy does not exist
*/
pub(crate) fn set_default_policy(args: &[String]) -> Result<(), String> {
    require_human("crane --set default policy")?;
    let parsed = Args::parse(args, &[], &[])?;
    parsed.expect_positional(1, "crane --set default policy NAME")?;
    let name = parsed.positional[0].clone();
    let workspace = Workspace::locate()?;
    if PolicySet::load(&workspace)?.find(&name).is_none() {
        return Err(format!("policy '{name}' does not exist"));
    }
    commit(
        &workspace,
        "crane --set default policy",
        "default.policy_set",
        json!({"policy": name}),
        |state, _| {
            state.config.default_policy = Some(name.clone());
            Ok(())
        },
    )?;
    println!("Default policy: {name}");
    Ok(())
}

/** Set the default checkpoint
 * Input
    - args: &[String] - CHECKPOINT
 * Output
    - Result<(), String>
    - Error if run by an agent or the checkpoint does not exist
*/
pub(crate) fn set_default_checkpoint(args: &[String]) -> Result<(), String> {
    require_human("crane --set default checkpoint")?;
    let parsed = Args::parse(args, &[], &[])?;
    parsed.expect_positional(1, "crane --set default checkpoint NAME")?;
    let name = parsed.positional[0].clone();
    let workspace = Workspace::locate()?;
    if State::load(&workspace)?.checkpoints.find(&name).is_none() {
        return Err(format!("checkpoint '{name}' does not exist"));
    }
    commit(
        &workspace,
        "crane --set default checkpoint",
        "default.checkpoint_set",
        json!({"checkpoint": name}),
        |state, _| {
            state.config.default_checkpoint = Some(name.clone());
            Ok(())
        },
    )?;
    println!("Default checkpoint: {name}");
    Ok(())
}

/** Find a policy file from a relative path or a unique file name in .crane/policies
 * Input
    - argument: &str - path or name, with or without .crane
 * Output
    - Result<(String, String), String> display path and content
    - Error if no unique .crane file matches
*/
fn find_policy_file(argument: &str) -> Result<(String, String), String> {
    let direct = Path::new(argument);
    if direct.is_file() {
        if direct
            .extension()
            .is_none_or(|extension| extension != EXTENSION)
        {
            return Err(format!("{argument} is not a .crane file"));
        }
        let text = fs::read_to_string(direct)
            .map_err(|error| format!("could not read {argument}: {error}"))?;
        return Ok((argument.replace('\\', "/"), text));
    }
    let workspace = Workspace::locate()?;
    let wanted = argument.trim_end_matches(".crane").replace('\\', "/");
    let wanted = wanted.rsplit('/').next().unwrap_or_default().to_string();
    let path = workspace
        .policies_dir()
        .join(format!("{wanted}.{EXTENSION}"));
    let text = fs::read_to_string(&path)
        .map_err(|_| format!("no policy file '{argument}' (looked in .crane/policies)"))?;
    Ok((format!(".crane/policies/{wanted}.{EXTENSION}"), text))
}

/** Parse a .crane file and print its policies, commands, and their selections as a tree
 * Input
    - args: &[String] - FILE
 * Output
    - Result<(), String>
    - Error if the file is missing or has a syntax error
*/
pub(crate) fn parse(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &[])?;
    parsed.expect_positional(1, "crane parse file FILE")?;
    let (path, text) = find_policy_file(&parsed.positional[0])?;
    let file = parse_policy(&path, &text).map_err(|error| format!("{path}: {error}"))?;
    let state = Workspace::locate()
        .ok()
        .and_then(|workspace| State::load(&workspace).ok());
    println!("{path}");
    let total = file.blocks.len();
    for (index, block) in file.blocks.iter().enumerate() {
        let last_block = index + 1 == total;
        let (branch, indent) = if last_block {
            ("└─", "   ")
        } else {
            ("├─", "│  ")
        };
        println!(
            "{branch} policy {} (line {}, {} command{})",
            block.name,
            block.line,
            block.statements.len(),
            if block.statements.len() == 1 { "" } else { "s" }
        );
        for (position, statement) in block.statements.iter().enumerate() {
            let last = position + 1 == block.statements.len();
            let (twig, inner) = if last {
                ("└─", "   ")
            } else {
                ("├─", "│  ")
            };
            println!(
                "{indent}{twig} {} (line {})",
                statement.render().trim_end_matches(';'),
                statement.line
            );
            let mut facts = Vec::new();
            match state
                .as_ref()
                .and_then(|state| state.record(&statement.id).cloned())
            {
                Some(record) => {
                    facts.push(format!(
                        "selection: {} lines {}-{} [{:?}]{}",
                        record.current.span.path,
                        record.current.span.start_line,
                        record.current.span.end_line,
                        record.status,
                        record
                            .name
                            .map(|name| format!(" \"{name}\""))
                            .unwrap_or_default()
                    ));
                    facts.push(format!(
                        "checkpoint: {} ({})",
                        record.origin.checkpoint,
                        &record.origin.commit[..12.min(record.origin.commit.len())]
                    ));
                    facts.push(format!(
                        "origin digest: {}...{} (SHA-512)",
                        &record.origin.binding_digest[..16],
                        &record.origin.binding_digest[112..]
                    ));
                    facts.push(format!("record generation: {}, signed", record.generation));
                }
                None => {
                    facts.push("selection: not registered (crane validate will report it)".into())
                }
            }
            for (number, fact) in facts.iter().enumerate() {
                let leaf = if number + 1 == facts.len() {
                    "└─"
                } else {
                    "├─"
                };
                println!("{indent}{inner}{leaf} {fact}");
            }
        }
    }
    if total == 0 {
        println!("└─ (no policies)");
    }
    Ok(())
}

/** Print findings with severity tags
 * Input
    - findings: &[Finding] - findings to print
 * Output
    - None (writes to stdout)
*/
fn print_findings(findings: &[Finding]) {
    for finding in findings {
        let tag = match finding.severity {
            Severity::Critical => "CRITICAL",
            Severity::High => "ERROR",
            Severity::Medium => "WARNING",
            Severity::Low => "NOTE",
        };
        println!("  {tag} [{}] {}", finding.code, finding.message);
    }
}

/** Validate the policies and governance state: syntax, policy files outside .crane/policies,
 * checkpoints, the signed registry and map, defaults, policy-registry consistency, and anchor
 * resolution; a human run also records selections that moved to another file as a new registry
 * generation
 * Input
    - args: &[String] - optional --json
 * Output
    - Result<(), String>
    - Error if any check fails
*/
pub(crate) fn validate(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["json"])?;
    parsed.expect_positional(0, "crane validate [--json]")?;
    let workspace = Workspace::locate()?;
    let (report, state) = evaluate(&workspace, &Options::default())?;
    let blocking = report.has_blocking_findings();
    let mut recorded = Vec::new();
    if !blocking
        && !report.relocations.is_empty()
        && agent_environment().is_none()
        && workspace.trust()?.signing_key()?.is_some()
    {
        let relocations = report.relocations.clone();
        let detail = json!({"relocations": relocations});
        commit(
            &workspace,
            "crane validate",
            "selection.relocated",
            detail,
            |state, generation| {
                for relocation in &relocations {
                    if let Some(record) = state
                        .registry
                        .selections
                        .iter_mut()
                        .find(|record| record.id == relocation.id)
                    {
                        if let Some(Some(span)) = report.resolutions.get(&record.id) {
                            record.current.span = span.clone();
                            record.current.resolved_at_generation = generation;
                            record.generation += 1;
                        }
                    }
                }
                Ok(())
            },
        )?;
        recorded = report.relocations.clone();
    }
    if parsed.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "valid": !blocking,
                "generation": report.generation,
                "findings": report.findings,
                "selections": report.resolutions,
                "relocations_recorded": recorded,
                "files_scanned": report.files_scanned,
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        let policies = report.policies.len();
        let commands = report
            .policies
            .iter()
            .map(|policy| policy.commands.len())
            .sum::<usize>();
        println!(
            "Crane validate: {policies} policies, {commands} commands, {} selections, {} checkpoints, registry generation {} ({} files scanned)",
            state.registry.selections.len(),
            state.checkpoints.checkpoints.len(),
            report.generation,
            report.files_scanned
        );
        print_findings(&report.findings);
        for relocation in &recorded {
            println!(
                "  recorded relocation of {}: {} -> {}",
                relocation.id, relocation.from, relocation.to
            );
        }
        if !blocking && !report.relocations.is_empty() && recorded.is_empty() {
            for relocation in &report.relocations {
                println!(
                    "  NOTE selection {} moved {} -> {} (a human run of crane validate records it)",
                    relocation.id, relocation.from, relocation.to
                );
            }
        }
        println!("Crane validate: {}", if blocking { "FAIL" } else { "PASS" });
    }
    if blocking {
        Err("validation failed".into())
    } else {
        Ok(())
    }
}

/** Print a report's policy results
 * Input
    - report: &Report - evaluation
    - only: Option<&str> - limit to one policy
 * Output
    - None (writes to stdout)
*/
fn print_policies(report: &Report, only: Option<&str>) {
    for policy in report
        .policies
        .iter()
        .filter(|policy| only.is_none_or(|name| name == policy.name))
    {
        println!(
            "{} policy {} ({}, {} commands)",
            if policy.passed { "PASS" } else { "FAIL" },
            policy.name,
            policy.file,
            policy.commands.len()
        );
        for command in &policy.commands {
            let tag = match command.outcome {
                Outcome::Pass => "PASS",
                Outcome::Fail => "FAIL",
                Outcome::Unresolved => "UNRESOLVED",
                Outcome::Pending => "PENDING",
            };
            println!(
                "  {tag} {}: {}",
                render_statement(command.operation, &command.id, command.change_type)
                    .trim_end_matches(';'),
                command.message
            );
        }
    }
}

/** Run the smoke tests: evaluate every policy (a policy fails if any command fails) and then run
 * the commands listed under "tests" in .crane/config.json
 * Input
    - args: &[String] - "." and optional --json
 * Output
    - Result<(), String>
    - Error if any policy, check, or test command fails
*/
pub(crate) fn test(args: &[String]) -> Result<(), String> {
    let parsed = Args::parse(args, &[], &["json"])?;
    if parsed.positional != ["."] {
        return Err("usage: crane test . [--json]".into());
    }
    let workspace = Workspace::locate()?;
    let (report, state) = evaluate(&workspace, &Options::default())?;
    let mut tests = Vec::new();
    if report.passed {
        for test in &state.config.tests {
            let output = process::run(
                process::shell(&test.command).current_dir(&workspace.root),
                Duration::from_secs(test.timeout_seconds),
            )?;
            tests.push(json!({
                "name": test.name,
                "command": test.command,
                "passed": output.success,
                "timed_out": output.timed_out,
                "exit_code": output.code,
                "duration_ms": output.duration_ms,
            }));
        }
    }
    let tests_passed = tests.iter().all(|test| test["passed"] == true);
    let passed = report.passed && tests_passed;
    if parsed.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "passed": passed,
                "generation": report.generation,
                "findings": report.findings,
                "policies": report.policies,
                "tests": tests,
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        print_findings(&report.findings);
        print_policies(&report, None);
        for test in &tests {
            println!(
                "{} test {} ({} ms)",
                if test["passed"] == true {
                    "PASS"
                } else {
                    "FAIL"
                },
                test["name"].as_str().unwrap_or_default(),
                test["duration_ms"]
            );
        }
        if !report.passed && !state.config.tests.is_empty() {
            println!("  test commands were not run because the policies failed");
        }
        println!("Crane test: {}", if passed { "PASS" } else { "FAIL" });
    }
    if passed {
        Ok(())
    } else {
        Err("one or more Crane policies or tests failed".into())
    }
}

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
struct Check {
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
fn ci_check(workspace: &Workspace) -> Check {
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
