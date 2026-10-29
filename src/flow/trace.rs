// Tracing a flow from a starting point. Given a function, method, RPC handler, or any symbol
// (SYMBOL, Type.SYMBOL, or path:SYMBOL), Crane follows the calls through the repository with the
// lexical call-graph index of governance::flows: the same input always gives the same graph, an
// ambiguous or external call is reported instead of guessed, and nothing runs. The result is a
// draft in .crane/drafts/flows/ID.json that keeps every revision: retracing adds a revision (after
// the code or the parameters changed), rewinding makes an earlier revision current again (nothing
// is deleted), and a person reviews the current one and approves it (cli/flow), which adds the flow
// to .crane/governance.yaml and records the trace in the signed flow knowledge. A draft grants
// nothing. Agents start drafts through their hooks (flow/agent.rs); people with 'crane flow trace'.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::governance::flows::{self, Index};
use crate::governance::workspace::Workspace;
use crate::platform::files::write_atomic;
use crate::platform::{git, io_error};
use crate::trust::crypto::digest_json;

/** Layout version of a trace draft */
pub(crate) const DRAFT_FORMAT: &str = "crane-flow-draft/v1";

/** Deepest call chain followed when none is given */
pub(crate) const DEFAULT_DEPTH: usize = 8;

/** Deepest call chain a trace may follow */
pub(crate) const MAX_DEPTH: usize = 24;

/** Most revisions one draft keeps (the oldest beyond this are dropped) */
pub(crate) const MAX_REVISIONS: usize = 50;

/** Most starting points one trace takes */
pub(crate) const MAX_STARTS: usize = 16;

/** A function in a traced flow
 * Fields
    - id: String - path:qualified
    - path: String - file
    - symbol: String - Type.name or name
    - start_line, end_line: usize - lines (1-based)
    - depth: usize - calls from the nearest starting point
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TraceNode {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) symbol: String,
    pub(crate) start_line: usize,
    pub(crate) end_line: usize,
    pub(crate) depth: usize,
}

/** A call between two functions of a traced flow
 * Fields
    - from: String - caller id
    - to: String - callee id
    - how: String - how the call was resolved (unique name, same file)
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct TraceEdge {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) how: String,
}

/** A traced flow
 * Fields
    - entry_points: Vec<String> - resolved starting points
    - nodes: Vec<TraceNode> - functions, in the order they were reached
    - edges: Vec<TraceEdge> - calls
    - unresolved: Vec<String> - starting points or calls that could not be followed, with why
    - files: Vec<String> - files holding the functions
    - status: String - RESOLVED, PARTIAL (some calls not followed), or UNRESOLVED
    - analyzed: usize - files the index read
    - unsupported: usize - source files in languages the index does not read
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TraceGraph {
    pub(crate) entry_points: Vec<String>,
    pub(crate) nodes: Vec<TraceNode>,
    pub(crate) edges: Vec<TraceEdge>,
    pub(crate) unresolved: Vec<String>,
    pub(crate) files: Vec<String>,
    pub(crate) status: String,
    pub(crate) analyzed: usize,
    pub(crate) unsupported: usize,
}

impl TraceGraph {
    /** The digest of what the trace found (the functions and calls, not when it ran)
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        let mut edges = self.edges.clone();
        edges.sort();
        digest_json(
            "flows/trace/v1",
            &json!({
                "entry_points": self.entry_points,
                "nodes": self.nodes.iter().map(|node| json!([node.id, node.start_line, node.end_line])).collect::<Vec<_>>(),
                "edges": edges,
                "unresolved": self.unresolved,
            }),
        )
    }
}

/** One revision of a trace draft
 * Fields
    - revision: u32 - number (1, 2, ...)
    - from: Vec<String> - starting points
    - max_depth: usize - deepest call chain followed
    - graph: TraceGraph - what was found
    - digest: String - the graph's digest
    - traced_at: u64 - Unix milliseconds
    - by: String - who asked (a person, or "agent:SESSION")
    - reason: String - why it was traced again
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Revision {
    pub(crate) revision: u32,
    pub(crate) from: Vec<String>,
    pub(crate) max_depth: usize,
    pub(crate) graph: TraceGraph,
    pub(crate) digest: String,
    pub(crate) traced_at: u64,
    pub(crate) by: String,
    pub(crate) reason: String,
}

/** A trace draft
 * Fields
    - format: String - layout version
    - flow_id: String - the flow it becomes
    - description: String - what the flow does
    - revisions: Vec<Revision> - every revision kept, oldest first
    - current: u32 - the revision under review
    - requested_by: String - who started it
    - via: String - cli, agent, or dashboard
    - created_at, updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TraceDraft {
    pub(crate) format: String,
    pub(crate) flow_id: String,
    pub(crate) description: String,
    pub(crate) revisions: Vec<Revision>,
    pub(crate) current: u32,
    pub(crate) requested_by: String,
    pub(crate) via: String,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

impl TraceDraft {
    /** The revision under review
     * Input
        - None (uses self)
     * Output
        - Option<&Revision>
    */
    pub(crate) fn current(&self) -> Option<&Revision> {
        self.revisions
            .iter()
            .find(|revision| revision.revision == self.current)
    }
}

/** What changed between two traces
 * Fields
    - added_nodes, removed_nodes: Vec<String> - functions that appeared or disappeared
    - added_edges, removed_edges: Vec<String> - calls ("from -> to") that appeared or disappeared
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct TraceDiff {
    pub(crate) added_nodes: Vec<String>,
    pub(crate) removed_nodes: Vec<String>,
    pub(crate) added_edges: Vec<String>,
    pub(crate) removed_edges: Vec<String>,
}

impl TraceDiff {
    /** Report whether nothing changed
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn is_empty(&self) -> bool {
        self.added_nodes.is_empty()
            && self.removed_nodes.is_empty()
            && self.added_edges.is_empty()
            && self.removed_edges.is_empty()
    }
}

/** Compare two traces
 * Input
    - before: &TraceGraph - earlier trace
    - after: &TraceGraph - later trace
 * Output
    - TraceDiff
*/
pub(crate) fn diff(before: &TraceGraph, after: &TraceGraph) -> TraceDiff {
    let nodes = |graph: &TraceGraph| {
        graph
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect::<BTreeSet<_>>()
    };
    let edges = |graph: &TraceGraph| {
        graph
            .edges
            .iter()
            .map(|edge| format!("{} -> {}", edge.from, edge.to))
            .collect::<BTreeSet<_>>()
    };
    let (old_nodes, new_nodes) = (nodes(before), nodes(after));
    let (old_edges, new_edges) = (edges(before), edges(after));
    TraceDiff {
        added_nodes: new_nodes.difference(&old_nodes).cloned().collect(),
        removed_nodes: old_nodes.difference(&new_nodes).cloned().collect(),
        added_edges: new_edges.difference(&old_edges).cloned().collect(),
        removed_edges: old_edges.difference(&new_edges).cloned().collect(),
    }
}

/** Check the starting points and depth of a trace
 * Input
    - from: &[String] - starting points
    - max_depth: usize - deepest call chain
 * Output
    - Result<Vec<String>, String> the starting points, trimmed
*/
pub(crate) fn check_starts(from: &[String], max_depth: usize) -> Result<Vec<String>, String> {
    let starts = from
        .iter()
        .flat_map(|item| item.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if starts.is_empty() || starts.len() > MAX_STARTS {
        return Err(format!(
            "name 1 to {MAX_STARTS} starting points (SYMBOL, Type.SYMBOL, or path:SYMBOL)"
        ));
    }
    if let Some(bad) = starts.iter().find(|start| {
        start.len() > 300
            || start.contains("..")
            || start.starts_with('/')
            || start.chars().any(|c| c.is_whitespace() || c.is_control())
    }) {
        return Err(format!("'{bad}' is not a symbol of this repository"));
    }
    if max_depth == 0 || max_depth > MAX_DEPTH {
        return Err(format!("the depth is 1 to {MAX_DEPTH}"));
    }
    Ok(starts)
}

/** The call-graph index of a repository's tracked files
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Index, String>
*/
pub(crate) fn index(workspace: &Workspace) -> Result<Index, String> {
    let files = git::list_files(&workspace.root)?;
    Ok(Index::build(
        files
            .into_iter()
            .filter(|path| flows::supported(path))
            .filter_map(|path| {
                fs::read_to_string(workspace.root.join(&path))
                    .ok()
                    .map(|text| (path, text))
            }),
    ))
}

/** Trace a flow from its starting points over an index
 * Input
    - index: &Index - call-graph index
    - from: &[String] - starting points
    - max_depth: usize - deepest call chain followed
 * Output
    - TraceGraph
*/
pub(crate) fn trace_index(index: &Index, from: &[String], max_depth: usize) -> TraceGraph {
    let graph = index.discover(from, max_depth);
    TraceGraph {
        entry_points: graph.entry_points,
        nodes: graph
            .nodes
            .into_iter()
            .map(|node| TraceNode {
                path: node.definition.path,
                symbol: node.definition.qualified,
                start_line: node.definition.start_line,
                end_line: node.definition.end_line,
                depth: node.depth,
                id: node.id,
            })
            .collect(),
        edges: graph
            .edges
            .into_iter()
            .map(|(from, to, how)| TraceEdge { from, to, how })
            .collect(),
        unresolved: graph.unresolved,
        files: graph.files.into_iter().collect(),
        status: graph.status.to_string(),
        analyzed: index.analyzed,
        unsupported: index.unsupported,
    }
}

/** Trace a flow in a repository
 * Input
    - workspace: &Workspace - repository
    - from: &[String] - starting points
    - max_depth: usize - deepest call chain followed
 * Output
    - Result<TraceGraph, String>
*/
pub(crate) fn trace(
    workspace: &Workspace,
    from: &[String],
    max_depth: usize,
) -> Result<TraceGraph, String> {
    let starts = check_starts(from, max_depth)?;
    Ok(trace_index(&index(workspace)?, &starts, max_depth))
}

/** The folder of trace drafts
 * Input
    - workspace: &Workspace - repository
 * Output
    - PathBuf
*/
fn folder(workspace: &Workspace) -> PathBuf {
    workspace.crane.join("drafts").join("flows")
}

/** The file of a trace draft
 * Input
    - workspace: &Workspace - repository
    - id: &str - flow id (validated as a name)
 * Output
    - Result<PathBuf, String>
*/
fn file(workspace: &Workspace, id: &str) -> Result<PathBuf, String> {
    crate::platform::validate_name("Flow", id)?;
    Ok(folder(workspace).join(format!("{id}.json")))
}

/** Read a trace draft
 * Input
    - workspace: &Workspace - repository
    - id: &str - flow id
 * Output
    - Result<Option<TraceDraft>, String>
*/
pub(crate) fn load(workspace: &Workspace, id: &str) -> Result<Option<TraceDraft>, String> {
    let path = file(workspace, id)?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{} is malformed: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/** Save a trace draft
 * Input
    - workspace: &Workspace - repository
    - draft: &TraceDraft - draft
 * Output
    - Result<(), String>
*/
pub(crate) fn save(workspace: &Workspace, draft: &TraceDraft) -> Result<(), String> {
    let path = file(workspace, &draft.flow_id)?;
    fs::create_dir_all(folder(workspace)).map_err(io_error)?;
    let text = serde_json::to_string_pretty(draft).map_err(io_error)? + "\n";
    write_atomic(&path, text.as_bytes())
}

/** Remove a trace draft (after it was approved or discarded)
 * Input
    - workspace: &Workspace - repository
    - id: &str - flow id
 * Output
    - Result<(), String>
*/
pub(crate) fn remove(workspace: &Workspace, id: &str) -> Result<(), String> {
    match fs::remove_file(file(workspace, id)?) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

/** Every trace draft, by flow id
 * Input
    - workspace: &Workspace - repository
 * Output
    - (Vec<TraceDraft>, Vec<String>) drafts and unreadable files
*/
pub(crate) fn drafts(workspace: &Workspace) -> (Vec<TraceDraft>, Vec<String>) {
    let mut found = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = fs::read_dir(folder(workspace)) else {
        return (found, problems);
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        match fs::read_to_string(&path)
            .map_err(io_error)
            .and_then(|text| serde_json::from_str::<TraceDraft>(&text).map_err(io_error))
        {
            Ok(draft) => found.push(draft),
            Err(error) => problems.push(format!("{}: {error}", path.display())),
        }
    }
    (found, problems)
}

/** Add a revision to a draft (a new draft when there is none), and make it current
 * Input
    - draft: Option<TraceDraft> - the existing draft
    - id: &str - flow id
    - description: &str - what the flow does (kept from the draft when empty)
    - from: Vec<String> - starting points
    - max_depth: usize - deepest call chain followed
    - graph: TraceGraph - what was found
    - by: &str - who asked
    - via: &str - cli, agent, or dashboard
    - reason: &str - why
    - now: u64 - Unix milliseconds
 * Output
    - TraceDraft
*/
#[allow(clippy::too_many_arguments)]
pub(crate) fn add_revision(
    draft: Option<TraceDraft>,
    id: &str,
    description: &str,
    from: Vec<String>,
    max_depth: usize,
    graph: TraceGraph,
    by: &str,
    via: &str,
    reason: &str,
    now: u64,
) -> TraceDraft {
    let mut draft = draft.unwrap_or_else(|| TraceDraft {
        format: DRAFT_FORMAT.into(),
        flow_id: id.to_string(),
        description: String::new(),
        revisions: Vec::new(),
        current: 0,
        requested_by: by.to_string(),
        via: via.to_string(),
        created_at: now,
        updated_at: now,
    });
    if !description.trim().is_empty() {
        draft.description = description.trim().to_string();
    }
    let number = draft
        .revisions
        .iter()
        .map(|revision| revision.revision)
        .max()
        .unwrap_or(0)
        + 1;
    draft.revisions.push(Revision {
        revision: number,
        from,
        max_depth,
        digest: graph.digest(),
        graph,
        traced_at: now,
        by: by.to_string(),
        reason: reason.trim().to_string(),
    });
    if draft.revisions.len() > MAX_REVISIONS {
        let drop = draft.revisions.len() - MAX_REVISIONS;
        draft.revisions.drain(..drop);
    }
    draft.current = number;
    draft.updated_at = now;
    draft
}

/** Make an earlier revision current again (nothing is deleted)
 * Input
    - draft: &mut TraceDraft - draft
    - revision: u32 - revision to review
    - now: u64 - Unix milliseconds
 * Output
    - Result<(), String>
    - Error when the draft holds no such revision
*/
pub(crate) fn rewind(draft: &mut TraceDraft, revision: u32, now: u64) -> Result<(), String> {
    if !draft.revisions.iter().any(|kept| kept.revision == revision) {
        return Err(format!(
            "{} has no revision {revision} (it keeps {})",
            draft.flow_id,
            draft
                .revisions
                .iter()
                .map(|kept| kept.revision.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    draft.current = revision;
    draft.updated_at = now;
    Ok(())
}
