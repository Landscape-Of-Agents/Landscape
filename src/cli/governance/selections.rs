// Commands that record trusted state and selections: checkpoint, protect and target (anchor the
// lines, sign the record, add the statement to a policy), the default policy and checkpoint, and
// parse file (show the anchors a file contains).

use crate::cli::args::Args;
use crate::github;
use crate::governance::changes::ChangeType;
use crate::governance::policy::{parse as parse_policy, render_statement, PolicySet, EXTENSION};
use crate::governance::state::{commit, State};
use crate::governance::workspace::Workspace;
use crate::platform::{actor, git, now_unix, validate_name};
use crate::selection::create::{create, Request};
use crate::selection::registry::Operation;
use crate::trust::require_human;
use serde_json::json;
use std::fs;
use std::path::Path;

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
        Operation::Preserve => &[
            "policy",
            "checkpoint",
            "start-line",
            "end-line",
            "name",
            "context",
        ],
        Operation::Target => &[
            "policy",
            "checkpoint",
            "start-line",
            "end-line",
            "name",
            "change-type",
            "context",
        ],
    };
    let parsed = Args::parse(args, keys, &[])?;
    parsed.expect_positional(
        1,
        &format!(
            "crane {command} FILE context TEXT [policy NAME] [checkpoint NAME] [start-line N] [end-line N]"
        ),
    )?;
    // Every selection tells the agent why it exists and what to keep in mind there
    let context = parsed
        .value("context")
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
        .ok_or(format!(
            "a selection needs context for the agents that work in it: add context \"why these lines matter\" (crane {command} FILE context TEXT ...)"
        ))?;
    if context.chars().count() > 2000 {
        return Err("selection context must be at most 2000 characters".into());
    }
    let request = Request {
        operation,
        file: parsed.positional[0].clone(),
        policy: parsed.value("policy"),
        checkpoint: parsed.value("checkpoint"),
        start_line: parsed.line("start-line")?,
        end_line: parsed.line("end-line")?,
        name: parsed.value("name"),
        context: Some(context),
        change_type: parsed
            .value("change-type")
            .map(|value| ChangeType::parse(&value))
            .transpose()?,
    };
    let record = create(&request)?;
    crate::platform::notes::note("SELECTION_ID", json!({"id": record.id}));
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
    // The boundary as signed, for 'crane scope diff' (best effort; never undoes the selection)
    if let Ok(workspace) = crate::governance::workspace::Workspace::locate() {
        let _ = crate::scope::snapshot::record_current(&workspace);
    }
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
pub(super) fn find_policy_file(argument: &str) -> Result<(String, String), String> {
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
