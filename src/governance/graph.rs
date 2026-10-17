// The repository's governance graph ('crane governance graph'): zones, flows, the zones a flow runs
// through in order (its stages), and the selections inside them, with a selection nested in another
// selection shown under its parent instead of beside it. Edges point from a container to what it
// contains and from a stage to the next one, and the graph is kept acyclic: an edge that would close
// a cycle is left out and reported. It also tells which agents recently acted in each zone and
// flow, from this machine's event log. The Landscape extension draws it.

use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::verify::{evaluate, Options};
use super::workspace::Workspace;
use super::zones::{self, Zone};
use crate::platform::{glob, now_millis};
use crate::selection::registry::Span;
use crate::telemetry::model::Decision;
use crate::telemetry::Stores;

/** How far back agent activity is collected (24 hours, in milliseconds) */
const ACTIVITY_WINDOW_MS: u64 = 24 * 60 * 60 * 1000;

/** How recent the last tool call must be for an agent to count as active (15 minutes) */
const ACTIVE_MS: u64 = 15 * 60 * 1000;

/** Find a cycle in a directed graph, depth first without recursion
 * Input
    - edges: &[(String, String)] - from and to node pairs
 * Output
    - Option<Vec<String>> - the nodes of one cycle, starting and ending with the same node, or None
      when the graph is acyclic
*/
pub(crate) fn find_cycle(edges: &[(String, String)]) -> Option<Vec<String>> {
    let mut next: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (from, to) in edges {
        next.entry(from.as_str()).or_default().push(to.as_str());
        next.entry(to.as_str()).or_default();
    }
    // 0: not visited, 1: on the current path, 2: finished
    let mut color: BTreeMap<&str, u8> = next.keys().map(|node| (*node, 0)).collect();
    for &start in next.keys() {
        if color[start] != 0 {
            continue;
        }
        color.insert(start, 1);
        let mut path: Vec<(&str, usize)> = vec![(start, 0)];
        while let Some(&(node, position)) = path.last() {
            let Some(&child) = next[node].get(position) else {
                color.insert(node, 2);
                path.pop();
                continue;
            };
            if let Some(top) = path.last_mut() {
                top.1 += 1;
            }
            match color[child] {
                0 => {
                    color.insert(child, 1);
                    path.push((child, 0));
                }
                1 => {
                    let at = path
                        .iter()
                        .position(|(item, _)| *item == child)
                        .unwrap_or(0);
                    let mut cycle = path[at..]
                        .iter()
                        .map(|(item, _)| (*item).to_string())
                        .collect::<Vec<_>>();
                    cycle.push(child.to_string());
                    return Some(cycle);
                }
                _ => {}
            }
        }
    }
    None
}

/** A graph under construction that never accepts an edge closing a cycle
 * Fields
    - nodes: Vec<Value> - nodes in insertion order
    - index: BTreeMap<String, usize> - node id to its position in nodes
    - next: BTreeMap<String, BTreeSet<String>> - accepted edges as adjacency
    - edges: Vec<Value> - accepted edges in insertion order
    - rejected: Vec<Value> - edges left out because they would close a cycle
*/
#[derive(Default)]
struct Builder {
    nodes: Vec<Value>,
    index: BTreeMap<String, usize>,
    next: BTreeMap<String, BTreeSet<String>>,
    edges: Vec<Value>,
    rejected: Vec<Value>,
}

impl Builder {
    /** Add a node unless one with its id exists
     * Input
        - node: Value - node with an "id"
     * Output
        - bool - whether it was added
    */
    fn node(&mut self, node: Value) -> bool {
        let id = node["id"].as_str().unwrap_or_default().to_string();
        if self.index.contains_key(&id) {
            return false;
        }
        self.index.insert(id, self.nodes.len());
        self.nodes.push(node);
        true
    }

    /** The node with an id
     * Input
        - id: &str - node id
     * Output
        - Option<&mut Value>
    */
    fn get(&mut self, id: &str) -> Option<&mut Value> {
        let position = *self.index.get(id)?;
        self.nodes.get_mut(position)
    }

    /** Add an edge between existing nodes, unless it repeats an edge or would close a cycle (then
     * it is recorded as rejected)
     * Input
        - from: &str - source node id
        - to: &str - target node id
        - kind: &str - stage, next, contains, nests, or covers
        - label: Option<&str> - the flow a stage edge belongs to
     * Output
        - None
    */
    fn edge(&mut self, from: &str, to: &str, kind: &str, label: Option<&str>) {
        if !self.index.contains_key(from) || !self.index.contains_key(to) {
            return;
        }
        if self
            .next
            .get(from)
            .is_some_and(|targets| targets.contains(to))
        {
            return;
        }
        if from == to || self.reaches(to, from) {
            self.rejected.push(json!({
                "from": from,
                "to": to,
                "kind": kind,
                "label": label,
                "reason": format!("{from} → {to} would close a cycle"),
            }));
            return;
        }
        self.next
            .entry(from.to_string())
            .or_default()
            .insert(to.to_string());
        self.edges
            .push(json!({"from": from, "to": to, "kind": kind, "label": label}));
    }

    /** Whether a path of accepted edges leads from one node to another
     * Input
        - from: &str - start
        - to: &str - destination
     * Output
        - bool
    */
    fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = BTreeSet::from([from]);
        let mut queue = VecDeque::from([from]);
        while let Some(node) = queue.pop_front() {
            if node == to {
                return true;
            }
            for child in self.next.get(node).into_iter().flatten() {
                if seen.insert(child.as_str()) {
                    queue.push_back(child.as_str());
                }
            }
        }
        false
    }
}

/** Where the editor opens a node
 * Input
    - path: &str - repository-relative file
    - line: usize - 1-based line
 * Output
    - Value - {path, line}
*/
fn open(path: &str, line: usize) -> Value {
    json!({"path": path, "line": line.max(1)})
}

/** The files of a flow that one of its stages selects (its paths, files, directories, and the
 * files of its symbol and selection selectors)
 * Input
    - stage: &Zone - stage
    - files: &BTreeSet<String> - the flow's covered files
    - spans: &BTreeMap<String, Option<Span>> - where selections resolve
 * Output
    - BTreeSet<String>
*/
fn stage_files(
    stage: &Zone,
    files: &BTreeSet<String>,
    spans: &BTreeMap<String, Option<Span>>,
) -> BTreeSet<String> {
    let selectors = &stage.selectors;
    let mut matched = files
        .iter()
        .filter(|path| {
            selectors
                .paths
                .iter()
                .any(|pattern| glob::matches(pattern, path))
                || selectors.files.iter().any(|file| file == *path)
                || selectors.directories.iter().any(|directory| {
                    path.starts_with(&format!("{}/", directory.trim_end_matches('/')))
                })
                || selectors.symbols.iter().any(|symbol| {
                    symbol
                        .split_once(':')
                        .is_some_and(|(file, _)| file == path.as_str())
                })
        })
        .cloned()
        .collect::<BTreeSet<_>>();
    matched.extend(
        selectors
            .selections
            .iter()
            .filter_map(|id| spans.get(id).cloned().flatten())
            .map(|span| span.path),
    );
    matched
}

/** The selection a selection is nested in: the smallest one in the same file that encloses it.
 * Selections are ordered by (start, longest first, id), and only an earlier selection can be a
 * parent, so even identical spans never make each other parents and nesting is always a tree
 * Input
    - id: &str - selection
    - span: &Span - where it resolves
    - resolved: &[(String, Span)] - every resolved selection
 * Output
    - Option<&str> - the parent's id
*/
fn parent<'a>(id: &str, span: &Span, resolved: &'a [(String, Span)]) -> Option<&'a str> {
    let key = |item: &str, at: &Span| (at.start_byte, Reverse(at.end_byte), item.to_string());
    resolved
        .iter()
        .filter(|(other, outer)| {
            other != id
                && outer.path == span.path
                && outer.start_byte <= span.start_byte
                && span.end_byte <= outer.end_byte
                && key(other, outer) < key(id, span)
        })
        .min_by_key(|(other, outer)| {
            (
                outer.end_byte - outer.start_byte,
                Reverse(key(other, outer)),
            )
        })
        .map(|(other, _)| other.as_str())
}

/** The tool calls agents made in each zone and flow over the last 24 hours, from this machine's
 * event log, busiest entity first
 * Input
    - workspace: &Workspace - repository
    - kinds: &BTreeMap<String, &str> - zone and flow ids to their kind
 * Output
    - Vec<Value> - {node, kind, id, calls, denied, last_at, active, agents[]}
*/
fn activity(workspace: &Workspace, kinds: &BTreeMap<String, &str>) -> Vec<Value> {
    let Ok(trust) = workspace.trust() else {
        return Vec::new();
    };
    let events = Stores::open(&trust.runtime())
        .events
        .read_all()
        .unwrap_or_default();
    let now = now_millis();
    // entity -> agent -> (calls, denied, last_at, provider, model, session)
    type Tally = (
        u64,
        u64,
        u64,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let mut tallies: BTreeMap<String, BTreeMap<String, Tally>> = BTreeMap::new();
    for event in events.iter().filter(|event| {
        event.decision.is_some() && now.saturating_sub(event.occurred_at) <= ACTIVITY_WINDOW_MS
    }) {
        let agent = event
            .scope
            .agent_id
            .clone()
            .or_else(|| event.scope.provider.clone())
            .unwrap_or_else(|| "unknown agent".into());
        let denied = matches!(event.decision, Some(Decision::Deny | Decision::Quarantine));
        // Hooks record the covering entities as "zone:ID" and "flow:ID"
        for entity in event.bindings.zones.iter().collect::<BTreeSet<_>>() {
            let id = entity.split_once(':').map_or(entity.as_str(), |(_, id)| id);
            let Some(kind) = kinds.get(id) else {
                continue;
            };
            let tally = tallies
                .entry(format!("{kind}:{id}"))
                .or_default()
                .entry(agent.clone())
                .or_default();
            tally.0 += 1;
            tally.1 += u64::from(denied);
            if event.occurred_at >= tally.2 {
                tally.2 = event.occurred_at;
                tally.3.clone_from(&event.scope.provider);
                tally.4.clone_from(&event.scope.model);
                tally.5.clone_from(&event.scope.foxx_session_id);
            }
        }
    }
    let mut list = tallies
        .into_iter()
        .map(|(node, agents)| {
            let (kind, id) = node.split_once(':').unwrap_or(("zone", node.as_str()));
            let last_at = agents.values().map(|tally| tally.2).max().unwrap_or(0);
            let mut agents = agents
                .iter()
                .map(|(agent, (calls, denied, last, provider, model, session))| {
                    json!({
                        "agent": agent,
                        "provider": provider,
                        "model": model,
                        "session": session,
                        "calls": calls,
                        "denied": denied,
                        "last_at": last,
                        "active": now.saturating_sub(*last) <= ACTIVE_MS,
                    })
                })
                .collect::<Vec<_>>();
            agents.sort_by_key(|agent| Reverse(agent["last_at"].as_u64().unwrap_or(0)));
            json!({
                "node": node,
                "kind": kind,
                "id": id,
                "calls": agents.iter().filter_map(|agent| agent["calls"].as_u64()).sum::<u64>(),
                "denied": agents.iter().filter_map(|agent| agent["denied"].as_u64()).sum::<u64>(),
                "last_at": last_at,
                "active": now.saturating_sub(last_at) <= ACTIVE_MS,
                "agents": agents,
            })
        })
        .collect::<Vec<_>>();
    list.sort_by_key(|entry| Reverse(entry["last_at"].as_u64().unwrap_or(0)));
    list
}

/** Build the governance graph of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Value, String> - {generation, nodes, edges, rejected, acyclic, activity, findings}
    - Error if the governance state or .crane/governance.yaml cannot be read
*/
pub(crate) fn build(workspace: &Workspace) -> Result<Value, String> {
    let (report, state) = evaluate(workspace, &Options::default())?;
    let governance = zones::load(workspace)?;
    let resolution = report.zones.clone().unwrap_or_default();
    let covered = |kind: &str, id: &str| {
        resolution
            .entities
            .iter()
            .find(|entity| entity.kind == kind && entity.id == id)
            .map(|entity| entity.files.clone())
            .unwrap_or_default()
    };
    let resolved = report
        .resolutions
        .iter()
        .filter_map(|(id, span)| span.clone().map(|span| (id.clone(), span)))
        .collect::<Vec<_>>();
    let span_of = |id: &str| report.resolutions.get(id).cloned().flatten();
    // Where a zone opens: its first selection, else its first covered file
    let zone_open = |zone: &Zone, files: &BTreeSet<String>| {
        zone.selectors
            .selections
            .iter()
            .find_map(|id| span_of(id))
            .map(|span| open(&span.path, span.start_line))
            .or_else(|| files.iter().next().map(|path| open(path, 1)))
    };
    let mut graph = Builder::default();
    let mut kinds: BTreeMap<String, &str> = BTreeMap::new();
    // Files of every zone node (top-level zones and flow stages), for placing selections
    let mut zone_files: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for zone in &governance.zones {
        let files = covered("zone", &zone.id);
        let node = format!("zone:{}", zone.id);
        graph.node(json!({
            "id": node,
            "kind": "zone",
            "name": zone.id,
            "description": zone.description,
            "status": zone.status,
            "criticality": zone.criticality,
            "ceiling": zone.autonomy_ceiling,
            "files": files.len(),
            "stage_of": [],
            "top_level": true,
            "open": zone_open(zone, &files),
        }));
        kinds.insert(zone.id.clone(), "zone");
        zone_files.insert(node, files);
    }
    for flow in &governance.flows {
        let files = covered("flow", &flow.id);
        let entry = resolution
            .entities
            .iter()
            .find(|entity| entity.kind == "flow" && entity.id == flow.id)
            .and_then(|entity| entity.graph.as_ref())
            .and_then(|graph| {
                let first = graph.entry_points.first()?;
                graph.nodes.iter().find(|node| &node.id == first)
            })
            .map(|node| open(&node.definition.path, node.definition.start_line));
        let node = format!("flow:{}", flow.id);
        graph.node(json!({
            "id": node,
            "kind": "flow",
            "name": flow.id,
            "description": flow.description,
            "status": flow.status,
            "criticality": flow.criticality,
            "ceiling": flow.autonomy_ceiling,
            "files": files.len(),
            "stages": flow.stages.iter().map(|stage| stage.id.clone()).collect::<Vec<_>>(),
            "open": entry.or_else(|| files.iter().next().map(|path| open(path, 1))),
        }));
        kinds.insert(flow.id.clone(), "flow");
        // A stage named like an existing zone is that zone: flows that share it meet there
        for stage in &flow.stages {
            let stage_node = format!("zone:{}", stage.id);
            let matched = stage_files(stage, &files, &report.resolutions);
            if graph.node(json!({
                "id": stage_node,
                "kind": "zone",
                "name": stage.id,
                "description": stage.description,
                "status": stage.status,
                "criticality": stage.criticality,
                "ceiling": stage.autonomy_ceiling,
                "files": matched.len(),
                "stage_of": [],
                "top_level": false,
                "open": zone_open(stage, &matched),
            })) {
                kinds.entry(stage.id.clone()).or_insert("zone");
            }
            if let Some(stages) = graph
                .get(&stage_node)
                .and_then(|node| node["stage_of"].as_array_mut())
            {
                stages.push(json!(flow.id));
            }
            zone_files.entry(stage_node).or_default().extend(matched);
        }
        // The order of the flow: flow → first stage → second stage → ...
        let chain = std::iter::once(node.clone())
            .chain(flow.stages.iter().map(|stage| format!("zone:{}", stage.id)))
            .collect::<Vec<_>>();
        for (index, pair) in chain.windows(2).enumerate() {
            graph.edge(
                &pair[0],
                &pair[1],
                if index == 0 { "stage" } else { "next" },
                Some(&flow.id),
            );
        }
    }
    // A flow also covers the zones whose files it reaches without listing them as stages
    for flow in &governance.flows {
        let files = covered("flow", &flow.id);
        for zone in &governance.zones {
            if flow.stages.iter().any(|stage| stage.id == zone.id) {
                continue;
            }
            if zone_files
                .get(&format!("zone:{}", zone.id))
                .is_some_and(|zone_files| !zone_files.is_disjoint(&files))
            {
                graph.edge(
                    &format!("flow:{}", flow.id),
                    &format!("zone:{}", zone.id),
                    "covers",
                    None,
                );
            }
        }
    }
    for record in &state.registry.selections {
        let span = span_of(&record.id);
        graph.node(json!({
            "id": format!("selection:{}", record.id),
            "kind": "selection",
            "name": record.name.clone().unwrap_or_else(|| record.id.clone()),
            "selection": record.id,
            "context": record.context,
            "operation": record.operation,
            "policy": record.policy,
            "status": if span.is_some() { "resolved" } else { "UNRESOLVED" },
            "lines": span.as_ref().map(|span| json!([span.start_line, span.end_line])),
            "path": span.as_ref().map(|span| span.path.clone()),
            "parent": Value::Null,
            "open": span.as_ref().map(|span| open(&span.path, span.start_line)),
        }));
    }
    for record in &state.registry.selections {
        let node = format!("selection:{}", record.id);
        let span = span_of(&record.id);
        // A nested selection hangs under its parent; only top-level selections join containers
        if let Some(outer) = span
            .as_ref()
            .and_then(|span| parent(&record.id, span, &resolved))
        {
            let outer = format!("selection:{outer}");
            if let Some(item) = graph.get(&node) {
                item["parent"] = json!(outer);
            }
            graph.edge(&outer, &node, "nests", None);
            continue;
        }
        let path = span.as_ref().map(|span| span.path.as_str());
        let named = |zone: &Zone| zone.selectors.selections.contains(&record.id);
        let mut placed = false;
        for (zone_node, files) in &zone_files {
            let zone_id = &zone_node["zone:".len()..];
            let explicit = governance
                .zones
                .iter()
                .chain(governance.flows.iter().flat_map(|flow| flow.stages.iter()))
                .any(|zone| zone.id == zone_id && named(zone));
            if explicit || path.is_some_and(|path| files.contains(path)) {
                graph.edge(zone_node, &node, "contains", None);
                placed = true;
            }
        }
        if placed {
            continue;
        }
        for flow in &governance.flows {
            if path.is_some_and(|path| covered("flow", &flow.id).contains(path)) {
                graph.edge(&format!("flow:{}", flow.id), &node, "contains", None);
            }
        }
    }
    let acyclic = graph.rejected.is_empty();
    Ok(json!({
        "generation": report.generation,
        "nodes": graph.nodes,
        "edges": graph.edges,
        "rejected": graph.rejected,
        "acyclic": acyclic,
        "activity": activity(workspace, &kinds),
        "findings": resolution.findings,
    }))
}

#[cfg(test)]
mod tests {
    use super::{find_cycle, parent, Builder};
    use crate::selection::registry::Span;
    use serde_json::json;

    /** A span in one file
     * Input
        - start: usize - first byte
        - end: usize - last byte
     * Output
        - Span
    */
    fn span(start: usize, end: usize) -> Span {
        Span {
            path: "app.py".into(),
            start_line: start,
            end_line: end,
            start_byte: start,
            end_byte: end,
        }
    }

    #[test]
    fn cycles_are_found_and_never_accepted() {
        let edges = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(from, to)| ((*from).to_string(), (*to).to_string()))
                .collect::<Vec<_>>()
        };
        assert!(find_cycle(&edges(&[("a", "b"), ("b", "c"), ("a", "c")])).is_none());
        assert_eq!(
            find_cycle(&edges(&[("a", "b"), ("b", "c"), ("c", "a")])),
            Some(vec!["a".into(), "b".into(), "c".into(), "a".into()])
        );
        assert!(find_cycle(&edges(&[("a", "a")])).is_some());
        let mut graph = Builder::default();
        for id in ["a", "b", "c"] {
            graph.node(json!({"id": id}));
        }
        assert!(!graph.node(json!({"id": "a"})), "ids are unique");
        graph.edge("a", "b", "next", None);
        graph.edge("b", "c", "next", None);
        graph.edge("c", "a", "next", Some("loop"));
        graph.edge("b", "b", "next", None);
        graph.edge("a", "b", "next", None);
        assert_eq!(graph.edges.len(), 2, "the duplicate is dropped");
        assert_eq!(
            graph.rejected.len(),
            2,
            "both cycle-closing edges are rejected"
        );
        let accepted = graph
            .edges
            .iter()
            .map(|edge| {
                (
                    edge["from"].as_str().unwrap().to_string(),
                    edge["to"].as_str().unwrap().to_string(),
                )
            })
            .collect::<Vec<_>>();
        assert!(find_cycle(&accepted).is_none());
    }

    #[test]
    fn nested_selections_form_a_tree_even_with_identical_spans() {
        let resolved = vec![
            ("OUTER000".to_string(), span(0, 100)),
            ("MIDDLE00".to_string(), span(10, 60)),
            ("INNER000".to_string(), span(20, 30)),
            ("SIBLING0".to_string(), span(70, 90)),
            ("TWIN0001".to_string(), span(200, 300)),
            ("TWIN0002".to_string(), span(200, 300)),
        ];
        let parent_of = |id: &str| {
            let (_, at) = resolved.iter().find(|(item, _)| item == id).unwrap();
            parent(id, at, &resolved).map(str::to_string)
        };
        assert_eq!(parent_of("OUTER000"), None);
        assert_eq!(parent_of("MIDDLE00").as_deref(), Some("OUTER000"));
        assert_eq!(parent_of("INNER000").as_deref(), Some("MIDDLE00"));
        assert_eq!(parent_of("SIBLING0").as_deref(), Some("OUTER000"));
        // Identical spans: exactly one is the parent of the other
        assert_eq!(parent_of("TWIN0001"), None);
        assert_eq!(parent_of("TWIN0002").as_deref(), Some("TWIN0001"));
    }
}
