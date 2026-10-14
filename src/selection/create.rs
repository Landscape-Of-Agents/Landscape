// Creating a selection (crane protect and crane target). The given line range is used only once,
// here: it is resolved against the trusted checkpoint (every selected line must be unchanged since
// the checkpoint, found with a line diff that ignores existing anchors), a unique identifier is
// generated, anchors are inserted around the range, the command is appended to its policy, and a
// signed record is committed. Source files change only by the two inserted comment lines; if any
// step fails, the source and policy files are restored.

use std::collections::BTreeSet;
use std::fs;

use serde_json::json;

use super::anchors::{find_markers, insert, language_for, lines, Boundary, MarkerKind};
use super::registry::{
    binding_digest, content_digest, Current, Operation, Origin, Record, Span, Status,
};
use super::tracker::{normalize_content, resolve, scan_repository, Resolution, Scan};
use crate::governance::changes::ChangeType;
use crate::governance::diff::map_lines;
use crate::governance::policy::{insert_statement, render_statement, PolicySet};
use crate::governance::state::{commit, integrity, State};
use crate::governance::workspace::Workspace;
use crate::platform::files::rewrite_preserving;
use crate::platform::{actor, git, now_unix, validate_name};
use crate::trust::crypto::{is_selection_id, random_id};
use crate::trust::require_human;

/** Attempts at drawing an unused identifier before giving up */
const ID_ATTEMPTS: usize = 32;

/** What to select and how to bind it
 * Fields
    - operation: Operation - preserve (crane protect) or target (crane target)
    - file: String - path or unique file name
    - policy: Option<String> - policy to add the command to, the default policy when None
    - checkpoint: Option<String> - checkpoint to resolve against, the default when None
    - start_line: Option<usize> - first line (inclusive), the first line of the file when None
    - end_line: Option<usize> - last line (inclusive), the last line of the file when None
    - name: Option<String> - optional selection name
    - change_type: Option<ChangeType> - required change kind (target only)
*/
#[derive(Debug, Clone)]
pub(crate) struct Request {
    pub(crate) operation: Operation,
    pub(crate) file: String,
    pub(crate) policy: Option<String>,
    pub(crate) checkpoint: Option<String>,
    pub(crate) start_line: Option<usize>,
    pub(crate) end_line: Option<usize>,
    pub(crate) name: Option<String>,
    pub(crate) change_type: Option<ChangeType>,
}

/** Draw a fresh identifier that is neither registered nor written in any marker of the work
 * tree, regenerating on collision; debug builds can take identifiers from
 * CRANE_TEST_SELECTION_IDS (comma separated) so collisions can be tested deterministically
 * Input
    - state: &State - governance state
    - scan: &Scan - markers in the work tree
 * Output
    - Result<String, String>
    - Error if no unused identifier was found
*/
fn fresh_id(state: &State, scan: &Scan) -> Result<String, String> {
    let taken = state
        .registry
        .selections
        .iter()
        .map(|record| record.id.clone())
        .chain(
            scan.markers
                .iter()
                .filter_map(|(_, marker)| marker.id.clone()),
        )
        .collect::<BTreeSet<_>>();
    #[cfg(debug_assertions)]
    let mut scripted = std::env::var("CRANE_TEST_SELECTION_IDS")
        .ok()
        .map(|list| list.split(',').map(str::to_string).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter();
    for _ in 0..ID_ATTEMPTS {
        #[cfg(debug_assertions)]
        let candidate = match scripted.next() {
            Some(id) => id,
            None => random_id()?,
        };
        #[cfg(not(debug_assertions))]
        let candidate = random_id()?;
        if is_selection_id(&candidate) && !taken.contains(&candidate) {
            return Ok(candidate);
        }
        eprintln!("crane: selection id {candidate} is already in use; regenerating");
    }
    Err("could not generate an unused selection identifier".into())
}

/** Create a selection: validate the request, resolve the range against the checkpoint, insert the
 * anchors, add the command to its policy, and commit the signed record
 * Input
    - request: &Request - what to select
 * Output
    - Result<Record, String> the committed record
    - Error if run by an agent, the governance state does not verify, the policy or checkpoint
      does not exist, the file type has no comment syntax, the range is invalid, overlaps another
      selection, or differs from the checkpoint, or a write fails
*/
pub(crate) fn create(request: &Request) -> Result<Record, String> {
    let command = match request.operation {
        Operation::Preserve => "crane protect",
        Operation::Target => "crane target",
    };
    require_human(command)?;
    let workspace = Workspace::locate()?;
    let state = State::load(&workspace)?;
    let trust = workspace.trust()?;
    let blocking = integrity(&workspace, &state, &trust)
        .into_iter()
        .filter(super::super::governance::Finding::blocking)
        .map(|finding| finding.message)
        .collect::<Vec<_>>();
    if !blocking.is_empty() {
        return Err(format!(
            "the governance state fails verification: {}; run 'crane validate'",
            blocking.join("; ")
        ));
    }
    let policies = PolicySet::load(&workspace)?;
    if let Some((path, error)) = policies.errors.first() {
        return Err(format!("fix the policy syntax first: {path}: {error}"));
    }
    let policy = request
        .policy
        .clone()
        .or_else(|| state.config.default_policy.clone())
        .ok_or("no policy was given and no default policy is set; run 'crane --set default policy NAME'")?;
    let (policy_file, block) = policies.find(&policy).ok_or_else(|| {
        format!("policy '{policy}' does not exist; create it with 'crane create policy {policy} FILENAME'")
    })?;
    let checkpoint_name = request
        .checkpoint
        .clone()
        .or_else(|| state.config.default_checkpoint.clone())
        .ok_or(
            "no checkpoint was given and no default checkpoint is set; run 'crane checkpoint NAME'",
        )?;
    let checkpoint = state
        .checkpoints
        .find(&checkpoint_name)
        .cloned()
        .ok_or_else(|| format!("checkpoint '{checkpoint_name}' does not exist"))?;
    if !git::commit_exists(&workspace.root, &checkpoint.commit) {
        return Err(format!(
            "checkpoint '{}' commit {} is not available locally",
            checkpoint.name, checkpoint.commit
        ));
    }
    if let Some(name) = &request.name {
        validate_name("selection", name)?;
    }
    if request.operation == Operation::Preserve && request.change_type.is_some() {
        return Err("change_type applies only to crane target".into());
    }
    let path = workspace.resolve_file(&request.file)?;
    if path == ".crane" || path.starts_with(".crane/") {
        return Err("Crane metadata cannot be selected".into());
    }
    let language = language_for(&path).ok_or_else(|| {
        format!(
            "{path}: this file type has no supported comment syntax, so anchors cannot be placed"
        )
    })?;
    let full = workspace.root.join(&path);
    let text = String::from_utf8(
        fs::read(&full).map_err(|error| format!("could not read {path}: {error}"))?,
    )
    .map_err(|_| format!("{path} is not UTF-8 text"))?;
    let markers = find_markers(&path, &text);
    if let Some(marker) = markers.iter().find(|marker| marker.problem.is_some()) {
        return Err(format!(
            "{path}:{}: {}; repair the existing markers first",
            marker.line,
            marker.problem.clone().unwrap_or_default()
        ));
    }
    let all = lines(&text);
    if all.is_empty() {
        return Err(format!("{path} is empty; there is nothing to select"));
    }
    let first_line = match all.first() {
        Some(line)
            if request.start_line.is_none()
                && (line.text.starts_with("#!") || line.text.starts_with("<?xml"))
                && all.len() > 1 =>
        {
            2
        }
        _ => 1,
    };
    let start = request.start_line.unwrap_or(first_line);
    let end = request.end_line.unwrap_or(all.len());
    if start == 0 || end < start || end > all.len() {
        return Err(format!(
            "invalid line range {start}-{end}: {path} has {} lines (lines are 1-based and inclusive)",
            all.len()
        ));
    }
    // Existing selections in the file: their marker lines are taken and their ranges are closed
    let mut ranges: Vec<(String, usize, usize)> = Vec::new();
    for marker in markers.iter().filter(|marker| {
        marker.kind == MarkerKind::Selection && marker.boundary == Some(Boundary::Start)
    }) {
        let id = marker.id.clone().unwrap_or_default();
        let end_marker = markers
            .iter()
            .find(|other| {
                other.kind == MarkerKind::Selection
                    && other.id.as_deref() == Some(id.as_str())
                    && other.boundary == Some(Boundary::End)
            })
            .map_or(all.len(), |other| other.line);
        ranges.push((id, marker.line, end_marker));
    }
    if let Some((id, first, last)) = ranges.iter().find(|(_, first, last)| {
        let source_first = *first + 1;
        let source_last = last.saturating_sub(1);
        let intersects = source_first <= source_last && start <= source_last && end >= source_first;
        let candidate_contains = start <= source_first && end >= source_last;
        let existing_contains = source_first <= start && source_last >= end;
        let touches_marker = (start..=end).contains(first) || (start..=end).contains(last);
        (intersects && !candidate_contains && !existing_contains)
            || (touches_marker && !candidate_contains)
    }) {
        return Err(format!(
            "lines {start}-{end} overlap selection {id} (lines {first}-{last} including its markers); selections cannot overlap"
        ));
    }
    let marker_lines = markers
        .iter()
        .map(|marker| marker.line)
        .collect::<BTreeSet<_>>();
    let base_bytes = git::show(&workspace.root, &checkpoint.commit, &path)?.ok_or_else(|| {
        format!(
            "{path} does not exist in checkpoint '{}'; commit it and create a new checkpoint",
            checkpoint.name
        )
    })?;
    let base_text = String::from_utf8(base_bytes)
        .map_err(|_| format!("the checkpoint version of {path} is not UTF-8 text"))?;
    let base_lines = lines(&base_text);
    let logical = all
        .iter()
        .filter(|line| !marker_lines.contains(&line.number))
        .collect::<Vec<_>>();
    let mapping = map_lines(
        &base_lines.iter().map(|line| line.text).collect::<Vec<_>>(),
        &logical.iter().map(|line| line.text).collect::<Vec<_>>(),
    );
    let mapped = logical
        .iter()
        .enumerate()
        .filter(|(_, line)| line.number >= start && line.number <= end)
        .map(|(index, _)| mapping[index])
        .collect::<Vec<_>>();
    let unchanged = mapped.first().copied().flatten().is_some_and(|first| {
        mapped
            .iter()
            .enumerate()
            .all(|(offset, line)| *line == Some(first + offset))
    });
    if !unchanged {
        return Err(format!(
            "lines {start}-{end} of {path} differ from checkpoint '{}'; commit the change and create a new checkpoint, or select unchanged lines",
            checkpoint.name
        ));
    }
    let base_first = mapped[0].unwrap_or_default();
    let base_last = base_first + mapped.len() - 1;
    let origin_span = Span {
        path: path.clone(),
        start_line: base_first + 1,
        end_line: base_last + 1,
        start_byte: base_lines[base_first].start,
        end_byte: base_lines[base_last].end,
    };
    let origin_content =
        normalize_content(&base_text[origin_span.start_byte..origin_span.end_byte]);
    let existing = scan_repository(&workspace)?;
    let id = fresh_id(&state, &existing)?;
    let new_text = insert(&text, language.syntax, &id, start, end)?;
    let scan = Scan::single(&path, &new_text);
    let Resolution::Resolved(location) = resolve(&id, &scan) else {
        return Err("internal error: the inserted markers do not resolve".into());
    };
    let digest = content_digest(&origin_content);
    if content_digest(&location.content) != digest {
        return Err(format!(
            "lines {start}-{end} of {path} differ from checkpoint '{}' (line endings or trailing content); commit and create a new checkpoint",
            checkpoint.name
        ));
    }
    let blob = git::blob_id(&workspace.root, &checkpoint.commit, &path)
        .ok_or_else(|| format!("could not read the blob of {path} at the checkpoint"))?;
    let mut origin = Origin {
        checkpoint: checkpoint.name.clone(),
        commit: checkpoint.commit.clone(),
        blob,
        span: origin_span,
        content_digest: digest.clone(),
        binding_digest: String::new(),
    };
    origin.binding_digest = binding_digest(&workspace.repository_id, &origin);
    let mut record = Record {
        id: id.clone(),
        name: request.name.clone(),
        repository_id: workspace.repository_id.clone(),
        operation: request.operation,
        change_type: request
            .change_type
            .map(|change_type| change_type.name().to_string()),
        policy: policy.clone(),
        language: language.name.to_string(),
        origin,
        current: Current {
            span: location.span,
            content_digest: digest,
            resolved_at_generation: 0,
        },
        status: Status::Active,
        generation: 1,
        created_at: now_unix(),
        created_by: actor(),
        record_digest: String::new(),
        signature: String::new(),
    };
    let policy_path = workspace.root.join(&policy_file.path);
    let policy_text = insert_statement(
        &policy_file.text,
        block,
        &render_statement(request.operation, &id, request.change_type),
    );
    let original_policy = policy_file.text.clone();
    rewrite_preserving(&full, new_text.as_bytes())?;
    let restore = || {
        let _ = rewrite_preserving(&full, text.as_bytes());
        let _ = rewrite_preserving(&policy_path, original_policy.as_bytes());
    };
    if let Err(error) = rewrite_preserving(&policy_path, policy_text.as_bytes()) {
        restore();
        return Err(error);
    }
    let detail = json!({
        "id": id,
        "operation": request.operation.name(),
        "policy": policy,
        "path": path,
        "lines": [start, end],
        "checkpoint": checkpoint.name,
        "binding_digest": record.origin.binding_digest,
    });
    let committed = commit(
        &workspace,
        command,
        "selection.created",
        detail,
        |state, generation| {
            record.current.resolved_at_generation = generation;
            state.registry.selections.push(record.clone());
            Ok(())
        },
    );
    match committed {
        Ok(state) => state
            .record(&id)
            .cloned()
            .ok_or_else(|| "internal error: the committed record is missing".into()),
        Err(error) => {
            restore();
            Err(error)
        }
    }
}
