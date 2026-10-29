// Changing the signed flow knowledge. Every change is one signed generation through
// governance::state::commit (a person on a machine with an admin key). Approving a trace draft
// also adds the flow to .crane/governance.yaml with a context pack written from its trace (every
// flow needs guidance for agents): both entry blocks are appended as text (comments and layout of
// the rest of the file are kept), the file is read back and must equal the old one plus exactly
// this flow and pack, and the file and the knowledge are signed in the same generation, with the
// checks 'crane governance sign' makes. If signing fails, the file is put back.

use serde_json::{json, Value};

use super::model::{ApprovedTrace, FlowBook};
use super::trace::TraceDraft;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::{self, Governance};
use crate::platform::files::write_atomic;

/** The audit action of flow knowledge changes */
pub(crate) const FLOWS_CHANGED: &str = "flows.changed";

/** Sign a change of the flow knowledge
 * Input
    - workspace: &Workspace - repository
    - command: &str - the command (audited)
    - detail: Value - what changed, for the audit entry
    - change: impl FnOnce(&mut FlowBook) -> Result<(), String> - the change
 * Output
    - Result<FlowBook, String> the book as signed
*/
pub(crate) fn change(
    workspace: &Workspace,
    command: &str,
    detail: Value,
    change: impl FnOnce(&mut FlowBook) -> Result<(), String>,
) -> Result<FlowBook, String> {
    let mut signed = None;
    crate::governance::state::commit(workspace, command, FLOWS_CHANGED, detail, |state, _| {
        let mut book = FlowBook::of(state)?;
        change(&mut book)?;
        state.flows = Some(book.value());
        signed = Some(book);
        Ok(())
    })?;
    signed.ok_or_else(|| "the change was not applied".to_string())
}

/** A YAML double-quoted scalar (a JSON string is one)
 * Input
    - text: &str - text
 * Output
    - String
*/
fn quoted(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
}

/** A flow added to .crane/governance.yaml
 * Fields
    - id: String - flow id
    - description: String - what it does
    - entry_points: Vec<String> - starting points
    - max_depth: usize - deepest call chain
    - status: String - active or proposed
    - pack: String - the context pack it references
*/
#[derive(Debug, Clone)]
pub(crate) struct NewFlow {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) entry_points: Vec<String>,
    pub(crate) max_depth: usize,
    pub(crate) status: String,
    pub(crate) pack: String,
}

/** The context pack written with a new flow (every flow needs guidance for agents)
 * Fields
    - id: String - pack id
    - description: String - what it is
    - content: Vec<String> - guidance lines
*/
#[derive(Debug, Clone)]
pub(crate) struct NewPack {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) content: Vec<String>,
}

/** The entry block of a flow, at an item indentation
 * Input
    - indent: &str - indentation of the item's "- " marker
    - flow: &NewFlow - the flow
 * Output
    - String (ending with a newline)
*/
fn flow_block(indent: &str, flow: &NewFlow) -> String {
    let inner = format!("{indent}  ");
    let mut block = format!("{indent}- id: {}\n", quoted(&flow.id));
    if !flow.description.is_empty() {
        block.push_str(&format!(
            "{inner}description: {}\n",
            quoted(&flow.description)
        ));
    }
    block.push_str(&format!("{inner}entry_points:\n"));
    for entry in &flow.entry_points {
        block.push_str(&format!("{inner}  - {}\n", quoted(entry)));
    }
    block.push_str(&format!("{inner}max_depth: {}\n", flow.max_depth));
    block.push_str(&format!("{inner}context_packs: [{}]\n", quoted(&flow.pack)));
    block.push_str(&format!("{inner}status: {}\n", flow.status));
    block
}

/** The entry block of a context pack, at an item indentation
 * Input
    - indent: &str - indentation of the item's "- " marker
    - pack: &NewPack - the pack
 * Output
    - String (ending with a newline)
*/
fn pack_block(indent: &str, pack: &NewPack) -> String {
    let inner = format!("{indent}  ");
    let mut block = format!("{indent}- id: {}\n", quoted(&pack.id));
    block.push_str(&format!("{inner}version: 1\n"));
    block.push_str(&format!(
        "{inner}description: {}\n",
        quoted(&pack.description)
    ));
    block.push_str(&format!("{inner}content:\n"));
    for line in &pack.content {
        block.push_str(&format!("{inner}  - {}\n", quoted(line)));
    }
    block
}

/** Append one item to a top-level list of a YAML text, in the list's own style; the list is
 * created when the key is missing, and an inline empty list becomes a block list
 * Input
    - lines: Vec<String> - the text's lines
    - key: &str - top-level key ("flows" or "context_packs")
    - render: impl Fn(&str) -> String - the item at an indentation
 * Output
    - Result<Vec<String>, String> the new lines
    - Error when the list is written inline with items
*/
fn append_item(
    lines: Vec<String>,
    key: &str,
    render: impl Fn(&str) -> String,
) -> Result<Vec<String>, String> {
    let prefix = format!("{key}:");
    let Some(at) = lines.iter().position(|line| line.starts_with(&prefix)) else {
        let mut out = lines;
        while out.last().is_some_and(|line| line.trim().is_empty()) {
            out.pop();
        }
        out.push(prefix);
        out.extend(render("  ").lines().map(str::to_string));
        return Ok(out);
    };
    let mut lines = lines;
    let inline = lines[at][prefix.len()..].trim().to_string();
    if inline == "[]" {
        lines[at] = prefix.clone();
    } else if !inline.is_empty() && !inline.starts_with('#') {
        return Err(format!("the {key} list of .crane/governance.yaml is written inline; write it as a block list to add to it"));
    }
    // The list ends at the next top-level key
    let mut end = at + 1;
    let mut indent = None;
    while end < lines.len() {
        let line = &lines[end];
        let trimmed = line.trim_start();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            let width = line.len() - trimmed.len();
            if trimmed.starts_with("- ") && indent.is_none() {
                indent = Some(line[..width].to_string());
            }
            if width == 0 && !(trimmed.starts_with("- ") && indent.as_deref() == Some("")) {
                break;
            }
        }
        end += 1;
    }
    while end > at + 1 && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    let mut out = lines[..end].to_vec();
    out.extend(
        render(indent.as_deref().unwrap_or("  "))
            .lines()
            .map(str::to_string),
    );
    out.extend(lines[end..].iter().cloned());
    Ok(out)
}

/** Append a flow and its context pack to the text of .crane/governance.yaml, keeping the rest of
 * it as it is (comments, layout, line endings)
 * Input
    - text: &str - the file's text ("" when it does not exist)
    - flow: &NewFlow - the flow
    - pack: &NewPack - its context pack
 * Output
    - Result<String, String> the new text
    - Error when the flow or pack exists, or the result does not read as the old file plus exactly
      this flow and pack
*/
pub(crate) fn append_flow(text: &str, flow: &NewFlow, pack: &NewPack) -> Result<String, String> {
    let before: Governance = if text.trim().is_empty() {
        Governance::default()
    } else {
        serde_yaml::from_str(text).map_err(|error| format!(".crane/governance.yaml: {error}"))?
    };
    if before.flows.iter().any(|existing| existing.id == flow.id) {
        return Err(format!(
            "{} is already a flow of .crane/governance.yaml",
            flow.id
        ));
    }
    if before
        .context_packs
        .iter()
        .any(|existing| existing.id == pack.id)
    {
        return Err(format!(
            "{} is already a context pack of .crane/governance.yaml",
            pack.id
        ));
    }
    let crlf = text.contains("\r\n");
    let mut lines = text.lines().map(str::to_string).collect::<Vec<_>>();
    if lines
        .iter()
        .all(|line| line.trim().is_empty() || line.trim_start().starts_with('#'))
    {
        lines.push("version: 1".into());
    }
    let lines = append_item(lines, "flows", |indent| flow_block(indent, flow))?;
    let lines = append_item(lines, "context_packs", |indent| pack_block(indent, pack))?;
    let mut result = lines.join("\n") + "\n";
    if crlf {
        result = result.replace('\n', "\r\n");
    }
    let after: Governance = serde_yaml::from_str(&result).map_err(|error| {
        format!(
            "adding {} would break .crane/governance.yaml: {error}",
            flow.id
        )
    })?;
    let mut expected = before.clone();
    let added = after
        .flows
        .iter()
        .find(|candidate| candidate.id == flow.id)
        .cloned();
    let packed = after
        .context_packs
        .iter()
        .find(|candidate| candidate.id == pack.id)
        .cloned();
    let (Some(added), Some(packed)) = (added, packed) else {
        return Err(format!(
            "adding {} to .crane/governance.yaml did not take",
            flow.id
        ));
    };
    expected.flows.push(added.clone());
    expected.context_packs.push(packed.clone());
    if expected.version == 0 {
        expected.version = after.version;
    }
    if after != expected
        || added.entry_points != flow.entry_points
        || packed.content != pack.content
    {
        return Err(format!(
            "adding {} to .crane/governance.yaml would change more than this flow; add it by hand",
            flow.id
        ));
    }
    Ok(result)
}

/** The context pack of a traced flow: what it does, where it starts, what the trace reached, and
 * how agents ask for the rest
 * Input
    - governance: &Governance - zones and flows (for a free pack id)
    - draft: &TraceDraft - the draft
    - revision: &super::trace::Revision - the revision approved
 * Output
    - NewPack
*/
pub(crate) fn trace_pack(
    governance: &Governance,
    draft: &TraceDraft,
    revision: &super::trace::Revision,
) -> NewPack {
    let base = format!("{}-flow", draft.flow_id)
        .chars()
        .take(60)
        .collect::<String>();
    let mut id = base.clone();
    let mut number = 2;
    while governance.context_packs.iter().any(|pack| pack.id == id) {
        id = format!("{base}-{number}");
        number += 1;
    }
    let mut content = Vec::new();
    if !draft.description.is_empty() {
        content.push(draft.description.clone());
    }
    content.push(format!(
        "Starts at {}. Crane traced {} functions and {} calls across {} files (revision {}, {}).",
        revision.from.join(", "),
        revision.graph.nodes.len(),
        revision.graph.edges.len(),
        revision.graph.files.len(),
        revision.revision,
        revision.graph.status
    ));
    content.push(format!(
        "Files: {}{}",
        revision
            .graph
            .files
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join(", "),
        if revision.graph.files.len() > 20 {
            ", …"
        } else {
            ""
        }
    ));
    content.push(format!(
        "For its priority, context, people, and tests, ask Crane: crane-request flow {}",
        draft.flow_id
    ));
    NewPack {
        id,
        description: format!(
            "Guidance for the {} flow, written when its trace was approved",
            draft.flow_id
        ),
        content,
    }
}

/** Approve the current revision of a trace draft: add the flow to .crane/governance.yaml when it
 * is new, record the trace in the flow knowledge, and sign both in one generation
 * Input
    - workspace: &Workspace - repository
    - draft: &TraceDraft - the draft
    - status: &str - active or proposed (for a new flow)
    - reason: &str - why (audited)
 * Output
    - Result<(State, bool), String> the signed state, and whether the flow was added to
      .crane/governance.yaml
*/
pub(crate) fn approve(
    workspace: &Workspace,
    draft: &TraceDraft,
    status: &str,
    reason: &str,
) -> Result<(State, bool), String> {
    let revision = draft
        .current()
        .cloned()
        .ok_or("the draft has no revision to approve")?;
    if revision.graph.status == "UNRESOLVED" {
        return Err(format!(
            "revision {} of {} did not resolve its starting points; retrace it first",
            revision.revision, draft.flow_id
        ));
    }
    let id = draft.flow_id.clone();
    let path = workspace.file(zones::FILE);
    let original = std::fs::read_to_string(&path).unwrap_or_default();
    let governance = zones::load(workspace)?;
    let added = match governance.flows.iter().find(|flow| flow.id == id) {
        Some(existing) => {
            if existing.entry_points != revision.from {
                return Err(format!(
                    "flow {id} already starts at {} in .crane/governance.yaml, not at {}; trace from its entry points, or change them there first",
                    existing.entry_points.join(", "),
                    revision.from.join(", ")
                ));
            }
            false
        }
        None => {
            let pack = trace_pack(&governance, draft, &revision);
            let text = append_flow(
                &original,
                &NewFlow {
                    id: id.clone(),
                    description: draft.description.clone(),
                    entry_points: revision.from.clone(),
                    max_depth: revision.max_depth,
                    status: status.to_string(),
                    pack: pack.id.clone(),
                },
                &pack,
            )?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(crate::platform::io_error)?;
            }
            write_atomic(&path, text.as_bytes())?;
            true
        }
    };
    let approved = ApprovedTrace {
        from: revision.from.clone(),
        max_depth: revision.max_depth,
        revision: revision.revision,
        digest: revision.digest.clone(),
        nodes: revision.graph.nodes.len(),
        edges: revision.graph.edges.len(),
        files: revision.graph.files.clone(),
        status: revision.graph.status.clone(),
        approved_by: crate::platform::actor(),
        approved_at: crate::platform::now_millis(),
    };
    let signed = crate::governance::sign::sign_with(
        workspace,
        "crane flow approve",
        json!({"flow": id, "revision": revision.revision, "digest": revision.digest, "added": added, "reason": reason}),
        |state| {
            let mut book = FlowBook::of(state)?;
            book.flows.entry(id.clone()).or_default().trace = Some(approved);
            state.flows = Some(book.value());
            Ok(())
        },
    );
    match signed {
        Ok((state, _, _)) => Ok((state, added)),
        Err(error) => {
            if added {
                let _ = write_atomic(&path, original.as_bytes());
            }
            Err(error)
        }
    }
}
