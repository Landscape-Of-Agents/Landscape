// Removing a selection. The rule ends: its two marker comments leave the source (the code between
// them stays), its command leaves its policy, and its signed record leaves the registry through
// commit(), which refuses agents and non-admins and keeps the removal in the audit chain. A
// selection a zone names in governance.yaml is left alone, because that zone's coverage rests on it.
// If any step fails, the source and policy files are restored.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::anchors::{lines, MarkerKind};
use super::tracker::scan_repository;
use crate::governance::policy::{remove_statement, PolicySet};
use crate::governance::state::{commit, integrity, State};
use crate::governance::workspace::Workspace;
use crate::governance::zones;
use crate::platform::files::rewrite_preserving;
use crate::trust::require_human;

/** What removing a selection touches
 * Fields
    - workspace: Workspace - repository
    - id: String - selection identifier
    - operation: String - preserve or target
    - policy: String - the policy holding its command
    - binding_digest: String - its signed binding, kept in the audit entry
    - files: BTreeMap<String, Vec<usize>> - marker lines (1-based) by repository-relative file
    - named_by: Vec<String> - zones that name it in their selections selector
    - policies: Vec<(String, String, String)> - policy file path, its text, and its text without
      the selection's commands
*/
pub(crate) struct Plan {
    workspace: Workspace,
    id: String,
    pub(crate) operation: String,
    pub(crate) policy: String,
    binding_digest: String,
    pub(crate) files: BTreeMap<String, Vec<usize>>,
    pub(crate) named_by: Vec<String>,
    policies: Vec<(String, String, String)>,
}

/** Work out what removing a selection touches, without changing anything
 * Input
    - id: &str - selection identifier
 * Output
    - Result<Plan, String>
    - Error if run by an agent, the governance state does not verify, a policy file does not parse,
      or the selection is not registered
*/
pub(crate) fn plan(id: &str) -> Result<Plan, String> {
    require_human("crane selection remove")?;
    let workspace = Workspace::locate()?;
    let state = State::load(&workspace)?;
    let trust = workspace.trust()?;
    let blocking = integrity(&workspace, &state, &trust)
        .into_iter()
        .filter(crate::governance::Finding::blocking)
        .map(|finding| finding.message)
        .collect::<Vec<_>>();
    if !blocking.is_empty() {
        return Err(format!(
            "the governance state fails verification: {}; run 'crane validate'",
            blocking.join("; ")
        ));
    }
    let record = state
        .record(id)
        .ok_or_else(|| format!("{id} is not a registered selection"))?;
    let policies = PolicySet::load(&workspace)?;
    if let Some((path, error)) = policies.errors.first() {
        return Err(format!("fix the policy syntax first: {path}: {error}"));
    }
    // Later commands first, so the offsets of the earlier ones stay right
    let mut rewritten: BTreeMap<String, (String, String)> = BTreeMap::new();
    let mut references = policies.references(id);
    references.sort_by_key(|(_, _, statement)| std::cmp::Reverse(statement.start));
    for (file, _, statement) in references {
        let entry = rewritten
            .entry(file.path.clone())
            .or_insert_with(|| (file.text.clone(), file.text.clone()));
        entry.1 = remove_statement(&entry.1, statement);
    }
    let mut files: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (path, marker) in &scan_repository(&workspace)?.markers {
        if marker.kind == MarkerKind::Selection && marker.id.as_deref() == Some(id) {
            files.entry(path.clone()).or_default().push(marker.line);
        }
    }
    let governance = zones::load(&workspace)?;
    let named_by = governance
        .zones
        .iter()
        .chain(governance.flows.iter().flat_map(|flow| flow.stages.iter()))
        .filter(|zone| zone.selectors.selections.iter().any(|named| named == id))
        .map(|zone| zone.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    Ok(Plan {
        id: id.to_string(),
        operation: record.operation.name().to_string(),
        policy: record.policy.clone(),
        binding_digest: record.origin.binding_digest.clone(),
        files,
        named_by,
        policies: rewritten
            .into_iter()
            .map(|(path, (before, after))| (path, before, after))
            .collect(),
        workspace,
    })
}

/** A text without some of its lines
 * Input
    - text: &str - file content
    - dropped: &[usize] - line numbers (1-based)
 * Output
    - String
*/
fn without_lines(text: &str, dropped: &[usize]) -> String {
    lines(text)
        .iter()
        .filter(|line| !dropped.contains(&line.number))
        .map(|line| &text[line.start..line.end])
        .collect()
}

/** Put files back after a failed removal
 * Input
    - undo: &[(PathBuf, String)] - files with the text they had
 * Output
    - None (a file that cannot be written back stays as it is)
*/
fn restore(undo: &[(PathBuf, String)]) {
    for (path, text) in undo {
        let _ = rewrite_preserving(path, text.as_bytes());
    }
}

/** Remove the selection: its markers, its policy commands, and its signed record
 * Input
    - plan: &Plan - from plan()
    - reason: &str - why, kept in the audit entry
 * Output
    - Result<(), String>
    - Error if run by an agent, the machine is not an admin, a zone names the selection, or a write
      fails (the files are restored)
*/
pub(crate) fn apply(plan: &Plan, reason: &str) -> Result<(), String> {
    require_human("crane selection remove")?;
    if !plan.named_by.is_empty() {
        return Err(format!(
            "{} is named by zone(s) {}",
            plan.id,
            plan.named_by.join(", ")
        ));
    }
    let root = &plan.workspace.root;
    let mut writes = Vec::new();
    for (path, marker_lines) in &plan.files {
        let full = root.join(path);
        let text =
            fs::read_to_string(&full).map_err(|error| format!("could not read {path}: {error}"))?;
        let after = without_lines(&text, marker_lines);
        writes.push((full, text, after));
    }
    for (path, before, after) in &plan.policies {
        writes.push((root.join(path), before.clone(), after.clone()));
    }
    let mut undo = Vec::new();
    for (full, before, after) in writes {
        if let Err(error) = rewrite_preserving(&full, after.as_bytes()) {
            restore(&undo);
            return Err(error);
        }
        undo.push((full, before));
    }
    let detail = json!({
        "id": plan.id,
        "operation": plan.operation,
        "policy": plan.policy,
        "paths": plan.files.keys().collect::<Vec<_>>(),
        "binding_digest": plan.binding_digest,
        "reason": reason,
    });
    let committed = commit(
        &plan.workspace,
        "crane selection remove",
        "selection.removed",
        detail,
        |state, _| {
            state
                .registry
                .selections
                .retain(|record| record.id != plan.id);
            Ok(())
        },
    );
    if let Err(error) = committed {
        restore(&undo);
        return Err(error);
    }
    Ok(())
}
