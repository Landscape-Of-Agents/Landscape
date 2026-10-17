// The governance graph as 'crane governance graph --json' reports it (zones, flows in stage order,
// selections with nested ones under their parent, and agent activity), and its layout: a layered
// drawing of a directed acyclic graph. Crane already leaves out edges that would close a cycle; the
// layout drops any that remain, so a drawing is always acyclic. Editor-independent and
// self-contained (the webview embeds layout() as source), so it is unit-tested.

/** Where the editor opens a node. */
export interface OpenTarget {
  path: string;
  line: number;
}

/** A node of the governance graph. */
export interface GraphNode {
  id: string;
  kind: "zone" | "flow" | "selection";
  name: string;
  description?: string;
  status?: string;
  criticality?: string;
  ceiling?: string;
  files?: number;
  stage_of?: string[];
  stages?: string[];
  top_level?: boolean;
  selection?: string;
  operation?: "preserve" | "target";
  policy?: string;
  lines?: [number, number] | null;
  path?: string | null;
  parent?: string | null;
  open?: OpenTarget | null;
}

/** An edge: flow → first stage ("stage"), stage → next ("next"), container → selection
 * ("contains"), selection → nested selection ("nests"), flow → zone it reaches ("covers"). */
export interface GraphEdge {
  from: string;
  to: string;
  kind: "stage" | "next" | "contains" | "nests" | "covers";
  label?: string | null;
}

/** One agent's recent tool calls in a zone or flow. */
export interface AgentActivity {
  agent: string;
  provider?: string | null;
  model?: string | null;
  session?: string | null;
  calls: number;
  denied: number;
  last_at: number;
  active: boolean;
}

/** The recent tool calls in one zone or flow (the last 24 hours, on this machine). */
export interface ActivityEntry {
  node: string;
  kind: "zone" | "flow";
  id: string;
  calls: number;
  denied: number;
  last_at: number;
  active: boolean;
  agents: AgentActivity[];
}

/** The JSON printed by 'crane governance graph --json'. */
export interface GraphData {
  generation: number;
  nodes: GraphNode[];
  edges: GraphEdge[];
  rejected: (GraphEdge & { reason: string })[];
  acyclic: boolean;
  activity: ActivityEntry[];
  findings: { code: string; severity: string; message: string }[];
}

/** A node's place in the drawing. */
export interface Placement {
  layer: number;
  order: number;
  x: number;
  y: number;
}

/** A layered drawing. */
export interface Layout {
  placed: Record<string, Placement>;
  width: number;
  height: number;
  /** Edges left out because they would close a cycle (or name a missing node). */
  dropped: { from: string; to: string }[];
}

/**
 * Lay out a directed graph in layers, left to right. Kahn's algorithm orders the nodes; when only
 * nodes on a cycle remain, the incoming edges of one of them are dropped, so the result is acyclic.
 * A node's layer is the length of the longest path reaching it, and a few barycenter sweeps order
 * each layer to shorten edges. Self-contained: it uses no other function.
 * @param nodes nodes, in a stable order
 * @param edges edges between them
 * @param column distance between layers
 * @param row distance between nodes of a layer
 * @returns positions, size, and dropped edges
 */
export function layout(nodes: { id: string }[], edges: { from: string; to: string }[], column = 230, row = 58): Layout {
  const ids = nodes.map((node) => node.id);
  const known = new Set(ids);
  const dropped: { from: string; to: string }[] = [];
  const kept: { from: string; to: string }[] = [];
  const seen = new Set<string>();
  for (const edge of edges) {
    const key = edge.from + "\u0000" + edge.to;
    if (!known.has(edge.from) || !known.has(edge.to) || edge.from === edge.to) dropped.push({ from: edge.from, to: edge.to });
    else if (!seen.has(key)) {
      seen.add(key);
      kept.push({ from: edge.from, to: edge.to });
    }
  }
  // Topological order; nodes stuck on a cycle lose their remaining incoming edges
  const incoming = new Map(ids.map((id) => [id, [] as { from: string; to: string }[]]));
  const outgoing = new Map(ids.map((id) => [id, [] as { from: string; to: string }[]]));
  for (const edge of kept) {
    incoming.get(edge.to)!.push(edge);
    outgoing.get(edge.from)!.push(edge);
  }
  const live = new Set(kept);
  const degree = new Map(ids.map((id) => [id, incoming.get(id)!.length]));
  const order: string[] = [];
  const done = new Set<string>();
  while (order.length < ids.length) {
    let next = ids.find((id) => !done.has(id) && degree.get(id) === 0);
    if (next === undefined) {
      next = ids.filter((id) => !done.has(id)).sort((a, b) => degree.get(a)! - degree.get(b)!)[0];
      for (const edge of incoming.get(next)!) {
        if (live.has(edge) && !done.has(edge.from)) {
          live.delete(edge);
          dropped.push(edge);
        }
      }
      degree.set(next, 0);
    }
    done.add(next);
    order.push(next);
    for (const edge of outgoing.get(next)!) if (live.has(edge)) degree.set(edge.to, degree.get(edge.to)! - 1);
  }
  const acyclic = kept.filter((edge) => live.has(edge));
  // Longest-path layers
  const layer = new Map(ids.map((id) => [id, 0]));
  for (const id of order) for (const edge of outgoing.get(id)!) if (live.has(edge)) layer.set(edge.to, Math.max(layer.get(edge.to)!, layer.get(id)! + 1));
  const layers: string[][] = [];
  for (const id of ids) (layers[layer.get(id)!] ??= []).push(id);
  for (let index = 0; index < layers.length; index += 1) layers[index] ??= [];
  // Barycenter sweeps: down by predecessors, up by successors
  const position = new Map<string, number>();
  const index = () => layers.forEach((items) => items.forEach((id, at) => position.set(id, at)));
  index();
  const neighbors = (id: string, down: boolean) => acyclic.filter((edge) => (down ? edge.to === id : edge.from === id)).map((edge) => position.get(down ? edge.from : edge.to)!);
  for (let sweep = 0; sweep < 6; sweep += 1) {
    const down = sweep % 2 === 0;
    const range = down ? layers.slice(1) : layers.slice(0, -1).reverse();
    for (const items of range) {
      const center = new Map(items.map((id) => {
        const near = neighbors(id, down);
        return [id, near.length ? near.reduce((sum, value) => sum + value, 0) / near.length : position.get(id)!];
      }));
      items.sort((a, b) => center.get(a)! - center.get(b)! || position.get(a)! - position.get(b)!);
      items.forEach((id, at) => position.set(id, at));
    }
  }
  const tallest = Math.max(1, ...layers.map((items) => items.length));
  const placed: Record<string, Placement> = {};
  layers.forEach((items, at) => {
    const offset = ((tallest - items.length) * row) / 2;
    items.forEach((id, slot) => (placed[id] = { layer: at, order: slot, x: at * column, y: offset + slot * row }));
  });
  return { placed, width: Math.max(1, layers.length) * column, height: tallest * row, dropped };
}

/**
 * Summarize who is working where: active entities first, then the most recent.
 * @param activity the graph's activity
 * @param limit how many entries to keep
 * @returns entries, flows before zones when equally recent
 */
export function busiest(activity: ActivityEntry[], limit = 6): ActivityEntry[] {
  return [...activity]
    .sort((a, b) => Number(b.active) - Number(a.active) || b.last_at - a.last_at || (a.kind === b.kind ? 0 : a.kind === "flow" ? -1 : 1))
    .slice(0, limit);
}
