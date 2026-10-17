// The Foxx graph: the repository's zones, flows, and selections as Crane reports them ('crane
// governance graph'), on a canvas beside the editor. Zones can be dragged anywhere (positions are
// remembered per workspace), and linked by dragging from one zone's connector onto another: the
// link becomes a flow's next stage in .crane/governance.yaml, signed by Crane. Every link is
// checked here first (no self links, no cycles, a flow stays one ordered path), then again by the
// extension and by Crane. Hover a node for what it is and who is working there, click it to open
// its code, filter the kinds from one menu.

import * as vscode from "vscode";

import { findCycle } from "./core/governance";
import { GraphData, layout } from "./core/graph";
import { RunResult } from "./panel";

/** A node's place on the canvas. */
export type Positions = Record<string, { x: number; y: number }>;

/** The extension side the graph talks to. */
export interface GraphHost {
  /** The latest graph, or why there is none. */
  load(refresh: boolean): Promise<{ data?: GraphData; error?: string }>;
  /** Where the nodes were left on the canvas. */
  positions(): Positions;
  /** Remember where the nodes are. */
  savePositions(positions: Positions): void;
  /** Open a node's code in the editor. */
  open(id: string): Promise<void>;
  /** Link one zone to the next (a flow's next stage). */
  link(from: string, to: string): Promise<RunResult>;
  /** Remove the link between two consecutive zones of a flow. */
  unlink(from: string, to: string): Promise<RunResult>;
  /** Run a command of the extension (fix Crane, open output). */
  command(id: string): Promise<void>;
}

/** The governance graph panel (one per window). */
export class FoxxGraph {
  private static current: FoxxGraph | undefined;

  /**
   * Open the graph beside the editor, or bring it forward.
   * @param extensionUri the extension's folder
   * @param host extension side
   * @param connect show how to link zones
   */
  static show(extensionUri: vscode.Uri, host: GraphHost, connect = false): void {
    if (FoxxGraph.current) {
      FoxxGraph.current.panel.reveal(vscode.ViewColumn.Beside);
      if (connect) void FoxxGraph.current.panel.webview.postMessage({ type: "hint" });
      return;
    }
    const panel = vscode.window.createWebviewPanel("foxx.graph", "Foxx Graph", { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false }, {
      enableScripts: true,
      retainContextWhenHidden: true,
      localResourceRoots: [vscode.Uri.joinPath(extensionUri, "images")],
    });
    panel.iconPath = vscode.Uri.joinPath(extensionUri, "images", "icon.png");
    FoxxGraph.current = new FoxxGraph(panel, host, connect);
  }

  /** Whether the graph is open. */
  static get open(): boolean {
    return FoxxGraph.current !== undefined;
  }

  /**
   * Send new graph data to an open panel.
   * @param data graph, or undefined with an error
   * @param error why there is no graph
   */
  static update(data: GraphData | undefined, error?: string): void {
    const current = FoxxGraph.current;
    if (current) void current.panel.webview.postMessage({ type: "data", data, error, positions: current.host.positions() });
  }

  /**
   * Create the panel's controller.
   * @param panel webview panel
   * @param host extension side
   * @param connect show how to link zones
   */
  private constructor(private readonly panel: vscode.WebviewPanel, private readonly host: GraphHost, connect: boolean) {
    panel.webview.html = html(panel.webview.cspSource);
    panel.onDidDispose(() => (FoxxGraph.current = undefined));
    panel.webview.onDidReceiveMessage(async (message: { type: string; id?: string; from?: string; to?: string; positions?: Positions }) => {
      if (message.type === "ready" || message.type === "refresh") {
        if (message.type === "ready" && connect) void panel.webview.postMessage({ type: "hint" });
        const { data, error } = await this.host.load(message.type === "refresh");
        FoxxGraph.update(data, error);
      } else if (message.type === "open" && message.id) await this.host.open(message.id);
      else if (message.type === "positions" && message.positions) this.host.savePositions(message.positions);
      else if (message.type === "command" && message.id) await this.host.command(message.id);
      else if ((message.type === "link" || message.type === "unlink") && message.from && message.to) {
        const result = await (message.type === "link" ? this.host.link(message.from, message.to) : this.host.unlink(message.from, message.to));
        void panel.webview.postMessage({ type: "result", op: message.type, from: message.from, to: message.to, result });
        const { data, error } = await this.host.load(true);
        FoxxGraph.update(data, error);
      }
    });
  }
}

/**
 * Build the graph's page: a strict content security policy (only this page's script and inline
 * styles), the layout and cycle check of core/ embedded as source, and the canvas editor.
 * @param cspSource the webview's source for styles
 * @returns HTML
 */
function html(cspSource: string): string {
  const nonce = Array.from({ length: 32 }, () => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 62)]).join("");
  const shared = `const layout = ${layout.toString()};\nconst findCycle = ${findCycle.toString()};`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Foxx Graph</title>
<style>
:root {
  --fox: #ff7a1a; --fox-soft: color-mix(in srgb, var(--fox) 16%, transparent);
  --bg: var(--vscode-editor-background); --fg: var(--vscode-foreground); --muted: var(--vscode-descriptionForeground);
  --line: var(--vscode-panel-border, color-mix(in srgb, var(--fg) 16%, transparent)); --raised: color-mix(in srgb, var(--fg) 5%, var(--bg));
  --node: color-mix(in srgb, var(--fg) 6%, var(--bg));
  --zone: var(--vscode-charts-blue, #3794ff); --flow: var(--fox); --preserve: var(--vscode-charts-green, #3fb950); --target: var(--vscode-charts-purple, #b180d7);
  --ok: var(--vscode-testing-iconPassed, #3fb950); --bad: var(--vscode-errorForeground, #f14c4c); --warn: var(--vscode-editorWarning-foreground, #cca700);
}
* { box-sizing: border-box; }
/* The hidden attribute always wins over display rules such as grid */
[hidden] { display: none !important; }
html, body { margin: 0; height: 100%; }
body { display: flex; flex-direction: column; color: var(--fg); background: var(--bg); font: var(--vscode-font-size) / 1.45 var(--vscode-font-family); overflow: hidden; }
button, input { font: inherit; color: inherit; }
:focus-visible { outline: 1px solid var(--fox); outline-offset: 1px; }
.bar { position: relative; z-index: 10; display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding: 8px 12px; border-bottom: 1px solid var(--line); background: color-mix(in srgb, var(--bg) 94%, transparent); }
.bar b { margin-right: 2px; }
.muted { color: var(--muted); }
.grow { flex: 1; }
input[type=search] { width: 190px; min-width: 0; padding: 4px 8px; border-radius: 6px; border: 1px solid var(--vscode-input-border, var(--line)); background: var(--vscode-input-background); color: var(--vscode-input-foreground); }
.btn { display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--line); background: var(--raised); padding: 3px 9px; border-radius: 6px; cursor: pointer; white-space: nowrap; }
.btn:hover, .btn[aria-expanded=true] { border-color: var(--fox); }
.btn svg { width: 15px; height: 15px; }
.btn .count { min-width: 16px; padding: 0 4px; border-radius: 8px; background: var(--fox); color: #1a0d05; font-size: 10px; font-weight: 700; text-align: center; }
.menu { position: absolute; top: calc(100% - 2px); z-index: 5; min-width: 190px; padding: 6px; border: 1px solid var(--line); border-radius: 8px; background: var(--vscode-menu-background, var(--bg)); box-shadow: 0 10px 28px rgba(0, 0, 0, .35); }
.menu label { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 5px; cursor: pointer; }
.menu label:hover { background: var(--vscode-list-hoverBackground, var(--raised)); }
.menu i { width: 10px; height: 10px; border-radius: 3px; }
.menu small { margin-left: auto; color: var(--muted); }
.hint { display: flex; align-items: center; gap: 8px; padding: 6px 12px; border-bottom: 1px solid var(--line); font-size: 12px; color: var(--muted); }
.hint .dot { width: 9px; height: 9px; border-radius: 50%; background: var(--zone); box-shadow: 0 0 0 2px var(--bg), 0 0 0 3px var(--zone); flex: none; }
.hint.lit { color: var(--fg); background: var(--fox-soft); }
.notices { position: absolute; z-index: 4; left: 12px; right: 12px; top: 10px; display: grid; gap: 6px; pointer-events: none; }
.notice { pointer-events: auto; padding: 7px 10px; border-radius: 7px; border: 1px solid color-mix(in srgb, var(--warn) 50%, transparent); background: color-mix(in srgb, var(--warn) 14%, var(--bg)); font-size: 12px; box-shadow: 0 6px 18px rgba(0, 0, 0, .25); }
.notice.bad { border-color: color-mix(in srgb, var(--bad) 55%, transparent); background: color-mix(in srgb, var(--bad) 14%, var(--bg)); }
.notice.ok { border-color: color-mix(in srgb, var(--ok) 55%, transparent); background: color-mix(in srgb, var(--ok) 14%, var(--bg)); }
#stage { position: relative; flex: 1; min-height: 0; overflow: hidden; background-color: var(--bg);
  background-image: radial-gradient(circle, color-mix(in srgb, var(--fg) 16%, transparent) 1px, transparent 1.4px); }
svg#canvas { position: absolute; inset: 0; width: 100%; height: 100%; display: block; cursor: grab; user-select: none; touch-action: none; }
svg#canvas.panning { cursor: grabbing; }
svg#canvas.linking { cursor: crosshair; }
.edge { fill: none; stroke: color-mix(in srgb, var(--fg) 34%, transparent); stroke-width: 1.4; pointer-events: none; }
.edge.stage, .edge.next { stroke: var(--flow); stroke-width: 2.2; }
.edge.nests { stroke-dasharray: 5 4; }
.edge.covers { stroke-dasharray: 2 4; }
.edge.pending { stroke-dasharray: 6 5; animation: march .6s linear infinite; opacity: .8; }
@keyframes march { to { stroke-dashoffset: -11; } }
.hit { fill: none; stroke: transparent; stroke-width: 14; cursor: pointer; }
.edge-g.removable:hover .edge, .edge-g.selected .edge { stroke: var(--fox); stroke-width: 3.2; }
.edge-g.selected .edge { filter: drop-shadow(0 0 4px var(--fox)); }
.remove { cursor: pointer; }
.remove circle { fill: var(--bad); stroke: var(--bg); stroke-width: 2; }
.remove path { stroke: #fff; stroke-width: 1.8; stroke-linecap: round; }
#temp { fill: none; stroke: var(--fox); stroke-width: 2.2; stroke-dasharray: 6 5; pointer-events: none; }
#temp.valid { stroke: var(--ok); stroke-dasharray: none; }
#temp.invalid { stroke: var(--bad); }
.node { cursor: grab; }
.node.dragging { cursor: grabbing; }
.node rect.box { fill: var(--node); stroke: var(--line); stroke-width: 1.2; rx: 9; transition: stroke .12s, fill .12s; }
.node text { fill: var(--fg); font-size: 12px; pointer-events: none; }
.node text.sub { fill: var(--muted); font-size: 10.5px; }
.node.proposed rect.box { stroke-dasharray: 4 3; }
.node.unresolved rect.box { stroke: var(--bad); }
.node:hover rect.box, .node.focus rect.box { stroke: var(--fox); stroke-width: 2; }
.node.match rect.box { stroke: var(--fox); stroke-width: 2; fill: var(--fox-soft); }
.node.target-ok rect.box { stroke: var(--ok); stroke-width: 2.6; fill: color-mix(in srgb, var(--ok) 14%, var(--node)); }
.node.target-bad rect.box { stroke: var(--bad); stroke-width: 2.6; fill: color-mix(in srgb, var(--bad) 12%, var(--node)); }
.node.source rect.box { stroke: var(--fox); stroke-width: 2.6; }
.port { fill: var(--bg); stroke: var(--zone); stroke-width: 2; cursor: crosshair; transition: r .12s; }
.port.in { pointer-events: none; opacity: .55; }
.node:hover .port.out, .port.out:focus { r: 7.5; fill: var(--zone); }
svg.linking .port.in { opacity: 1; }
.pulse { fill: var(--ok); animation: pulse 1.6s ease-in-out infinite; }
@keyframes pulse { 50% { opacity: .35; } }
@media (prefers-reduced-motion: reduce) { .pulse, .edge.pending { animation: none; } .node rect.box, .port { transition: none; } }
svg.dim .node:not(.lit):not(.focus) { opacity: .28; }
svg.dim .edge-g:not(.lit) { opacity: .15; }
.edge-g.lit .edge { stroke: var(--fox); }
#tip { position: fixed; z-index: 6; pointer-events: none; max-width: 340px; padding: 8px 10px; border-radius: 8px; border: 1px solid var(--line); background: var(--vscode-editorHoverWidget-background, var(--bg)); color: var(--vscode-editorHoverWidget-foreground, var(--fg)); box-shadow: 0 8px 24px rgba(0, 0, 0, .3); font-size: 12px; display: none; }
#tip b { display: block; font-size: 13px; margin-bottom: 2px; overflow-wrap: anywhere; }
#tip .kind { font-size: 10px; letter-spacing: .7px; text-transform: uppercase; color: var(--muted); }
#tip .kv { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 1px 10px; margin-top: 5px; }
#tip .kv span:nth-child(odd) { color: var(--muted); }
#tip .agents { margin: 6px 0 0; padding-left: 14px; }
#tip .say { margin-top: 6px; color: var(--muted); font-size: 11px; }
#tip.verdict { border-color: var(--bad); } #tip.verdict.ok { border-color: var(--ok); }
.empty { position: absolute; inset: 0; display: grid; place-items: center; text-align: center; color: var(--muted); padding: 20px; pointer-events: none; }
.empty b { display: block; color: var(--fg); margin-bottom: 4px; }
.zoom { position: absolute; z-index: 3; right: 12px; bottom: 12px; display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; background: var(--raised); }
.zoom button { width: 30px; height: 28px; border: 0; background: none; cursor: pointer; }
.zoom button + button { border-top: 1px solid var(--line); }
.zoom button:hover { background: var(--fox-soft); }
</style>
</head>
<body>
<div class="bar" role="toolbar" aria-label="Graph tools">
  <b>Governance graph</b><span class="muted" id="summary"></span>
  <span class="grow"></span>
  <input type="search" id="search" placeholder="Find a zone, flow, selection…" aria-label="Find a node">
  <button class="btn" id="filter" aria-haspopup="true" aria-expanded="false" aria-controls="filter-menu" title="Choose what the graph shows">
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M4 5h16l-6 7.5V19l-4 1.5v-8z"/></svg><span>Filter</span><span class="count" id="filter-count" hidden></span>
  </button>
  <div class="menu" id="filter-menu" role="menu" hidden>
    <label role="menuitemcheckbox"><input type="checkbox" data-kind="zone" checked><i style="background:var(--zone)"></i>Zones<small id="count-zone"></small></label>
    <label role="menuitemcheckbox"><input type="checkbox" data-kind="flow" checked><i style="background:var(--flow)"></i>Flows<small id="count-flow"></small></label>
    <label role="menuitemcheckbox"><input type="checkbox" data-kind="selection" checked><i style="background:var(--preserve)"></i>Selections<small id="count-selection"></small></label>
  </div>
  <button class="btn" id="arrange" title="Lay the graph out again, left to right">Arrange</button>
  <button class="btn" id="refresh" title="Ask Crane for the graph again">Refresh</button>
</div>
<div class="hint" id="hint"><span class="dot" aria-hidden="true"></span><span id="hint-text">Drag from a zone's right handle onto another zone to link them into a flow. Click a link, then press Delete to remove it. Drag zones to move them.</span></div>
<div id="stage">
  <div class="notices" id="notices" aria-live="polite"></div>
  <svg id="canvas" role="application" aria-label="Governance graph canvas"><defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="context-stroke"/></marker>
  </defs><g id="viewport"><g id="edges"></g><path id="temp" d="" hidden></path><g id="nodes"></g></g></svg>
  <div class="empty" id="empty" hidden></div>
  <div class="zoom"><button id="zoom-in" title="Zoom in" aria-label="Zoom in">+</button><button id="zoom-out" title="Zoom out" aria-label="Zoom out">−</button><button id="fit" title="Fit the graph" aria-label="Fit the graph">⤢</button></div>
</div>
<div id="tip" role="tooltip"></div>
<script nonce="${nonce}">
"use strict";
const vscode = acquireVsCodeApi();
${shared}
const W = 184, H = 44, GRID = 24;
const saved = vscode.getState() || {};
let data = null, error = "";
let kinds = Object.assign({ zone: true, flow: true, selection: true }, saved.kinds || {});
let query = "";
let pos = {};
let view = saved.view || { scale: 1, x: 40, y: 40 };
let gesture = null;
let selectedEdge = null;
let keyLink = null;
let pending = [];
const $ = (id) => document.getElementById(id);
const esc = (text) => String(text == null ? "" : text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const clip = (text, n) => (text.length > n ? text.slice(0, n - 1) + "…" : text);
const ago = (at) => { const s = Math.max(0, Math.round((Date.now() - at) / 1000)); return s < 60 ? "just now" : s < 3600 ? Math.round(s / 60) + " min ago" : Math.round(s / 3600) + " h ago"; };
const color = (node) => node.kind === "zone" ? "var(--zone)" : node.kind === "flow" ? "var(--flow)" : node.operation === "target" ? "var(--target)" : "var(--preserve)";
const glyph = (node) => node.kind === "zone" ? "▦" : node.kind === "flow" ? "⇢" : node.operation === "target" ? "◎" : "⛨";
const nodeById = (id) => data && data.nodes.find((node) => node.id === id);
const zoneName = (id) => id.replace(/^zone:/, "");
const activityOf = (id) => (data && data.activity || []).find((entry) => entry.node === id);
const persist = () => vscode.setState({ kinds, view });

// ---------------------------------------------------------------- flows, links, and their rules
// Where a zone sits: a free zone, or a stage of a flow at a position
function placeOf(name) {
  for (const flow of data.nodes.filter((node) => node.kind === "flow")) {
    const index = (flow.stages || []).indexOf(name);
    if (index >= 0) return { flow: flow.name, index, stages: flow.stages };
  }
  return { flow: null, index: -1, stages: [] };
}
// The same rules the extension and Crane apply, so a drag says at once whether it can link
function verdict(from, to) {
  if (from === to) return { ok: false, text: "A zone cannot follow itself." };
  const a = placeOf(from), b = placeOf(to);
  if (a.flow && a.flow === b.flow) return { ok: false, text: b.index === a.index + 1 ? from + " already leads to " + to + "." : from + " and " + to + " are already in flow " + a.flow + ", which runs each zone once." };
  const pairs = data.edges.filter((edge) => edge.kind === "next").map((edge) => [zoneName(edge.from), zoneName(edge.to)]);
  pairs.push([from, to]);
  const cycle = findCycle(pairs);
  if (cycle) return { ok: false, text: "That would make a cycle: " + cycle.join(" → ") + "." };
  if (a.flow && a.index !== a.stages.length - 1) return { ok: false, text: from + " already leads to " + a.stages[a.index + 1] + " in flow " + a.flow + ". A flow is one ordered path: link from its last zone." };
  if (b.flow && b.index !== 0) return { ok: false, text: to + " already follows " + b.stages[b.index - 1] + " in flow " + b.flow + ". Link to its first zone." };
  if (!a.flow && !b.flow) return { ok: true, text: "Creates a flow: " + from + " → " + to + "." };
  if (a.flow && !b.flow) return { ok: true, text: "Adds " + to + " after " + from + " in flow " + a.flow + "." };
  if (!a.flow && b.flow) return { ok: true, text: "Adds " + from + " before " + to + " in flow " + b.flow + "." };
  return { ok: true, text: "Merges flows " + a.flow + " and " + b.flow + "." };
}

// ---------------------------------------------------------------- drawing
function visible() {
  const nodes = data.nodes.filter((node) => kinds[node.kind]);
  const ids = new Set(nodes.map((node) => node.id));
  return { nodes, edges: data.edges.filter((edge) => ids.has(edge.from) && ids.has(edge.to)) };
}
// New nodes get the layered layout's place, shifted beside nodes that were already placed
function place(nodes, edges, all) {
  const missing = nodes.filter((node) => all || !pos[node.id]);
  if (!missing.length) return;
  const drawn = layout(nodes, edges, W + 70, H + 26);
  const shift = all ? 0 : Math.max(0, ...Object.values(pos).map((point) => point.x + W + 90));
  const lowest = Math.min(...missing.map((node) => drawn.placed[node.id].x));
  for (const node of missing) pos[node.id] = { x: Math.round((drawn.placed[node.id].x - (all ? 0 : lowest) + shift) / 4) * 4, y: Math.round(drawn.placed[node.id].y / 4) * 4 };
}
function anchor(id, side) { const p = pos[id]; return side === "out" ? { x: p.x + W, y: p.y + H / 2 } : { x: p.x, y: p.y + H / 2 }; }
// Forward links bend gently; a link back to the left arcs under the nodes instead of through them
function curve(a, b) {
  if (b.x >= a.x + 20) { const bend = Math.max(40, (b.x - a.x) / 2); return "M" + a.x + " " + a.y + " C" + (a.x + bend) + " " + a.y + " " + (b.x - bend) + " " + b.y + " " + b.x + " " + b.y; }
  const drop = Math.max(a.y, b.y) + H + 40 + Math.min(120, (a.x - b.x) * 0.08);
  return "M" + a.x + " " + a.y + " C" + (a.x + 90) + " " + a.y + " " + (a.x + 90) + " " + drop + " " + (a.x + b.x) / 2 + " " + drop + " S" + (b.x - 90) + " " + b.y + " " + b.x + " " + b.y;
}
function edgeKey(edge) { return edge.from + "|" + edge.to; }

function draw() {
  notices();
  const empty = $("empty");
  for (const kind of ["zone", "flow", "selection"]) $("count-" + kind).textContent = data ? String(data.nodes.filter((node) => node.kind === kind).length) : "";
  const hidden = Object.values(kinds).filter((on) => !on).length;
  $("filter-count").hidden = !hidden;
  $("filter-count").textContent = String(hidden);
  if (!data) { $("nodes").innerHTML = ""; $("edges").innerHTML = ""; empty.hidden = false; empty.innerHTML = error ? "" : "<div><b>Loading the graph…</b>Crane resolves the zones, flows, and selections.</div>"; return; }
  const { nodes, edges } = visible();
  const count = (kind) => data.nodes.filter((node) => node.kind === kind).length;
  $("summary").textContent = "· " + count("zone") + " zones · " + count("flow") + " flows · " + count("selection") + " selections" + (data.acyclic ? " · acyclic" : "");
  if (!nodes.length) { $("nodes").innerHTML = ""; $("edges").innerHTML = ""; empty.hidden = false; empty.innerHTML = data.nodes.length ? "<div><b>Everything is filtered out</b>Turn a kind back on from the filter.</div>" : "<div><b>No zones, flows, or selections yet</b>Select code and press Ctrl+Alt+F to make it a zone.</div>"; return; }
  empty.hidden = true;
  place(nodes, edges, false);
  const q = query.trim().toLowerCase();
  const all = edges.map((edge) => Object.assign({}, edge, { pending: false })).concat(pending.filter((item) => item.op === "link").map((item) => ({ from: "zone:" + item.from, to: "zone:" + item.to, kind: "next", pending: true })));
  $("edges").innerHTML = all.filter((edge) => pos[edge.from] && pos[edge.to]).map((edge) => {
    const removable = edge.kind === "next" && !edge.pending;
    const selected = selectedEdge && selectedEdge.from === edge.from && selectedEdge.to === edge.to;
    const d = curve(anchor(edge.from, "out"), anchor(edge.to, "in"));
    const unlinking = pending.some((item) => item.op === "unlink" && "zone:" + item.from === edge.from && "zone:" + item.to === edge.to);
    return '<g class="edge-g' + (removable ? " removable" : "") + (selected ? " selected" : "") + '" data-from="' + esc(edge.from) + '" data-to="' + esc(edge.to) + '" data-kind="' + edge.kind + '">' +
      (removable ? '<path class="hit" d="' + d + '"><title>' + esc(zoneName(edge.from) + " → " + zoneName(edge.to) + (edge.label ? " · flow " + edge.label : "") + " · click, then Delete to remove") + "</title></path>" : "") +
      '<path class="edge ' + edge.kind + (edge.pending || unlinking ? " pending" : "") + '" d="' + d + '" marker-end="url(#arrow)"></path>' +
      (selected ? removeButton(edge) : "") + "</g>";
  }).join("");
  $("nodes").innerHTML = nodes.map((node) => {
    const at = pos[node.id];
    const busy = activityOf(node.id);
    const classes = ["node", node.status === "proposed" ? "proposed" : "", node.status === "UNRESOLVED" ? "unresolved" : "", q && (node.name + " " + node.id + " " + (node.description || "")).toLowerCase().includes(q) ? "match" : "", keyLink && keyLink === node.name && node.kind === "zone" ? "source" : ""].filter(Boolean).join(" ");
    const where = node.kind === "zone" ? placeOf(node.name) : null;
    const sub = node.kind === "selection" ? (node.operation || "") + (node.lines ? " · L" + node.lines[0] + "–" + node.lines[1] : " · unresolved") : node.kind === "zone" ? (where.flow ? "stage " + (where.index + 1) + " of " + where.flow : "zone") + (node.status ? " · " + node.status : "") : "flow · " + (node.stages || []).length + " stage" + ((node.stages || []).length === 1 ? "" : "s") + (node.status ? " · " + node.status : "");
    return '<g class="' + classes + '" tabindex="0" role="button" data-id="' + esc(node.id) + '" aria-label="' + esc(node.kind + " " + node.name + (node.kind === "zone" ? ". Press C to link it, Enter to open." : ". Enter to open.")) + '" transform="translate(' + at.x + " " + at.y + ')">' +
      '<rect class="box" width="' + W + '" height="' + H + '"></rect><rect x="0" y="6" width="3.5" height="' + (H - 12) + '" rx="1.75" fill="' + color(node) + '"></rect>' +
      '<text x="14" y="19">' + esc(glyph(node) + " " + clip(node.name, 21)) + '</text><text class="sub" x="14" y="34">' + esc(clip(sub, 30)) + "</text>" +
      (busy && busy.active ? '<circle class="pulse" cx="' + (W - 14) + '" cy="12" r="4"><title>agent active now</title></circle>' : "") +
      (node.kind === "zone" ? '<circle class="port in" cx="0" cy="' + H / 2 + '" r="6"></circle><circle class="port out" tabindex="-1" data-port="' + esc(node.name) + '" cx="' + W + '" cy="' + H / 2 + '" r="6"><title>Drag onto another zone to link</title></circle>' : "") + "</g>";
  }).join("");
  apply();
}
function removeButton(edge) {
  const a = anchor(edge.from, "out"), b = anchor(edge.to, "in");
  const x = (a.x + b.x) / 2, y = (a.y + b.y) / 2;
  return '<g class="remove" data-unlink="1" transform="translate(' + x + " " + y + ')" role="button" aria-label="Remove this link"><circle r="10"></circle><path d="M-3.5 -3.5L3.5 3.5M3.5 -3.5L-3.5 3.5"></path><title>Remove this link (Delete)</title></g>';
}
// Move one node without redrawing everything: its transform and the edges that touch it
function moveNode(id) {
  const group = document.querySelector('.node[data-id="' + CSS.escape(id) + '"]');
  if (group) group.setAttribute("transform", "translate(" + pos[id].x + " " + pos[id].y + ")");
  for (const edge of document.querySelectorAll(".edge-g")) {
    if (edge.dataset.from !== id && edge.dataset.to !== id) continue;
    const d = curve(anchor(edge.dataset.from, "out"), anchor(edge.dataset.to, "in"));
    for (const path of edge.querySelectorAll("path.edge, path.hit")) path.setAttribute("d", d);
    const remove = edge.querySelector(".remove");
    if (remove) { const a = anchor(edge.dataset.from, "out"), b = anchor(edge.dataset.to, "in"); remove.setAttribute("transform", "translate(" + (a.x + b.x) / 2 + " " + (a.y + b.y) / 2 + ")"); }
  }
}
function apply() {
  $("viewport").setAttribute("transform", "translate(" + view.x + " " + view.y + ") scale(" + view.scale + ")");
  const stage = $("stage");
  stage.style.backgroundSize = GRID * view.scale + "px " + GRID * view.scale + "px";
  stage.style.backgroundPosition = view.x + "px " + view.y + "px";
}
function fit() {
  if (!data) return;
  const ids = visible().nodes.map((node) => node.id).filter((id) => pos[id]);
  if (!ids.length) return;
  const box = $("stage").getBoundingClientRect();
  const xs = ids.map((id) => pos[id].x), ys = ids.map((id) => pos[id].y);
  const left = Math.min(...xs) - 30, top = Math.min(...ys) - 30, width = Math.max(...xs) + W + 30 - left, height = Math.max(...ys) + H + 30 - top;
  view.scale = Math.max(0.25, Math.min(1.3, Math.min(box.width / width, box.height / height)));
  view.x = (box.width - width * view.scale) / 2 - left * view.scale;
  view.y = (box.height - height * view.scale) / 2 - top * view.scale;
  apply();
  persist();
}
function zoomBy(factor, cx, cy) {
  const box = $("stage").getBoundingClientRect();
  const px = cx == null ? box.width / 2 : cx - box.left, py = cy == null ? box.height / 2 : cy - box.top;
  const scale = Math.max(0.2, Math.min(3, view.scale * factor));
  view.x = px - ((px - view.x) * scale) / view.scale; view.y = py - ((py - view.y) * scale) / view.scale; view.scale = scale;
  apply();
  persist();
}
function toGraph(clientX, clientY) { const box = $("stage").getBoundingClientRect(); return { x: (clientX - box.left - view.x) / view.scale, y: (clientY - box.top - view.y) / view.scale }; }

// ---------------------------------------------------------------- notices and the tooltip
let flash = [];
function notices() {
  const list = [];
  if (error) list.push('<div class="notice bad" role="alert">' + esc(error) + ' <button class="btn" data-command="crane:download">Download latest Crane</button> <button class="btn" data-command="open:output">Show Crane output</button></div>');
  if (data && data.rejected && data.rejected.length) list.push('<div class="notice bad" role="alert"><b>Crane left out ' + data.rejected.length + " link" + (data.rejected.length === 1 ? "" : "s") + " that would close a cycle.</b> " + data.rejected.slice(0, 3).map((edge) => esc(edge.reason)).join("; ") + ".</div>");
  $("notices").innerHTML = list.concat(flash.map((item) => '<div class="notice ' + item.tone + '" role="status">' + esc(item.text) + "</div>")).join("");
}
function say(tone, text, ms) { const item = { tone, text }; flash.push(item); notices(); setTimeout(() => { flash = flash.filter((other) => other !== item); notices(); }, ms || 5000); }
function details(node) {
  const rows = [];
  const add = (key, value) => { if (value !== undefined && value !== null && value !== "") rows.push("<span>" + esc(key) + "</span><span>" + esc(value) + "</span>"); };
  if (node.kind === "selection") { add("operation", node.operation); add("policy", node.policy); add("where", node.path ? node.path + " lines " + node.lines[0] + "–" + node.lines[1] : "not found in the code"); add("nested in", node.parent ? node.parent.replace("selection:", "") : ""); }
  else { add("status", node.status); add("criticality", node.criticality); add("autonomy ceiling", node.ceiling); add("files", node.files); if (node.stages && node.stages.length) add("stages", node.stages.join(" → ")); if (node.stage_of && node.stage_of.length) add("stage of", node.stage_of.join(", ")); }
  const busy = activityOf(node.id);
  const agents = busy ? '<div class="kind" style="margin-top:6px">Agents · last 24 h</div><ul class="agents">' + busy.agents.slice(0, 5).map((agent) => "<li>" + esc(agent.agent) + (agent.model ? " (" + esc(agent.model) + ")" : "") + " · " + agent.calls + " call" + (agent.calls === 1 ? "" : "s") + (agent.denied ? ", " + agent.denied + " denied" : "") + " · " + (agent.active ? "active now" : ago(agent.last_at)) + "</li>").join("") + "</ul>" : "";
  const help = node.kind === "zone" ? "Drag its right handle onto another zone to link. Click to open " + (node.open ? node.open.path : "its code") + "." : node.open ? "Click to open " + node.open.path + ":" + node.open.line : "No source location";
  return '<div class="kind">' + esc(node.kind) + '</div><b>' + esc(node.name) + "</b>" + (node.description ? '<div class="muted">' + esc(node.description) + "</div>" : "") + '<div class="kv">' + rows.join("") + "</div>" + agents + '<div class="say">' + esc(help) + "</div>";
}
function tipAt(x, y, html, cls) {
  const tip = $("tip");
  tip.className = cls || "";
  tip.innerHTML = html;
  tip.style.display = "block";
  tip.style.left = Math.max(8, Math.min(window.innerWidth - tip.offsetWidth - 8, x + 14)) + "px";
  tip.style.top = Math.max(8, Math.min(window.innerHeight - tip.offsetHeight - 8, y + 14)) + "px";
}
function hideTip() { $("tip").style.display = "none"; }
function focusNode(id) {
  const svg = $("canvas");
  svg.classList.add("dim");
  const near = new Set([id]);
  for (const edge of svg.querySelectorAll(".edge-g")) { const lit = edge.dataset.from === id || edge.dataset.to === id; edge.classList.toggle("lit", lit); if (lit) { near.add(edge.dataset.from); near.add(edge.dataset.to); } }
  for (const group of svg.querySelectorAll(".node")) { group.classList.toggle("lit", near.has(group.dataset.id)); group.classList.toggle("focus", group.dataset.id === id); }
}
function blur() { const svg = $("canvas"); svg.classList.remove("dim"); for (const item of svg.querySelectorAll(".lit, .focus")) item.classList.remove("lit", "focus"); hideTip(); }

// ---------------------------------------------------------------- linking and unlinking
function link(from, to) {
  const ruling = verdict(from, to);
  if (!ruling.ok) { say("bad", ruling.text); return; }
  pending.push({ op: "link", from, to });
  say("ok", "Linking " + from + " → " + to + "… " + ruling.text, 2500);
  draw();
  vscode.postMessage({ type: "link", from, to });
}
function unlink(edge) {
  const from = zoneName(edge.from), to = zoneName(edge.to);
  pending.push({ op: "unlink", from, to });
  selectedEdge = null;
  draw();
  vscode.postMessage({ type: "unlink", from, to });
}
function setHint(text, lit) { $("hint-text").textContent = text; $("hint").classList.toggle("lit", Boolean(lit)); }
const DEFAULT_HINT = $("hint-text").textContent;

// ---------------------------------------------------------------- pointer gestures on the canvas
const canvas = $("canvas");
const nodeOf = (target) => { const group = target && target.closest && target.closest(".node"); return group && data ? nodeById(group.dataset.id) : null; };
canvas.addEventListener("pointerdown", (event) => {
  if (event.button !== 0 || !data) return;
  hideTip();
  const port = event.target.closest("[data-port]");
  const node = nodeOf(event.target);
  const remove = event.target.closest("[data-unlink]");
  if (remove && selectedEdge) { event.preventDefault(); unlink(selectedEdge); return; }
  const edge = event.target.closest(".edge-g.removable");
  if (port) {
    gesture = { kind: "link", from: port.dataset.port, start: anchor("zone:" + port.dataset.port, "out"), target: null };
    canvas.classList.add("linking");
    setHint("Drop on a zone to link " + port.dataset.port + " to it. Esc cancels.", true);
  } else if (node) {
    const point = toGraph(event.clientX, event.clientY);
    gesture = { kind: "move", id: node.id, dx: point.x - pos[node.id].x, dy: point.y - pos[node.id].y, x: event.clientX, y: event.clientY, moved: false };
  } else if (edge) {
    selectedEdge = { from: edge.dataset.from, to: edge.dataset.to };
    draw();
    return;
  } else {
    if (selectedEdge) { selectedEdge = null; draw(); }
    gesture = { kind: "pan", x: event.clientX, y: event.clientY, vx: view.x, vy: view.y };
    canvas.classList.add("panning");
  }
  canvas.setPointerCapture(event.pointerId);
});
canvas.addEventListener("pointermove", (event) => {
  if (!gesture) {
    const node = nodeOf(event.target);
    if (node) { focusNode(node.id); tipAt(event.clientX, event.clientY, details(node)); } else if (!event.target.closest(".edge-g")) blur();
    return;
  }
  if (gesture.kind === "pan") { view.x = gesture.vx + event.clientX - gesture.x; view.y = gesture.vy + event.clientY - gesture.y; apply(); }
  else if (gesture.kind === "move") {
    if (Math.abs(event.clientX - gesture.x) + Math.abs(event.clientY - gesture.y) > 4) gesture.moved = true;
    if (!gesture.moved) return;
    const point = toGraph(event.clientX, event.clientY);
    pos[gesture.id] = { x: Math.round((point.x - gesture.dx) / 4) * 4, y: Math.round((point.y - gesture.dy) / 4) * 4 };
    const group = document.querySelector('.node[data-id="' + CSS.escape(gesture.id) + '"]');
    if (group) group.classList.add("dragging");
    moveNode(gesture.id);
  } else if (gesture.kind === "link") {
    const point = toGraph(event.clientX, event.clientY);
    const under = document.elementFromPoint(event.clientX, event.clientY);
    const node = nodeOf(under);
    for (const group of document.querySelectorAll(".target-ok, .target-bad")) group.classList.remove("target-ok", "target-bad");
    const temp = $("temp");
    temp.hidden = false;
    gesture.target = null;
    if (node && node.kind === "zone" && node.name !== gesture.from) {
      const ruling = verdict(gesture.from, node.name);
      const group = document.querySelector('.node[data-id="' + CSS.escape(node.id) + '"]');
      if (group) group.classList.add(ruling.ok ? "target-ok" : "target-bad");
      temp.setAttribute("class", ruling.ok ? "valid" : "invalid");
      temp.setAttribute("d", curve(gesture.start, anchor(node.id, "in")));
      gesture.target = ruling.ok ? node.name : null;
      tipAt(event.clientX, event.clientY, "<b>" + esc(gesture.from + " → " + node.name) + "</b>" + esc(ruling.text), "verdict" + (ruling.ok ? " ok" : ""));
    } else {
      temp.setAttribute("class", "");
      temp.setAttribute("d", curve(gesture.start, point));
      if (node && node.kind !== "zone") tipAt(event.clientX, event.clientY, esc("Only zones can be linked into a flow."), "verdict"); else hideTip();
    }
  }
});
function endGesture(event) {
  if (!gesture) return;
  const done = gesture;
  gesture = null;
  canvas.classList.remove("panning", "linking");
  if (done.kind === "pan") persist();
  else if (done.kind === "move") {
    const group = document.querySelector(".node.dragging");
    if (group) group.classList.remove("dragging");
    if (done.moved) vscode.postMessage({ type: "positions", positions: pos });
    else if (event && event.type === "pointerup") { const node = nodeById(done.id); if (node && node.open) vscode.postMessage({ type: "open", id: node.id }); }
  } else if (done.kind === "link") {
    $("temp").hidden = true;
    for (const group of document.querySelectorAll(".target-ok, .target-bad")) group.classList.remove("target-ok", "target-bad");
    hideTip();
    setHint(DEFAULT_HINT, false);
    if (done.target && event && event.type === "pointerup") link(done.from, done.target);
  }
}
canvas.addEventListener("pointerup", endGesture);
canvas.addEventListener("pointercancel", endGesture);
canvas.addEventListener("pointerleave", () => { if (!gesture) blur(); });
canvas.addEventListener("wheel", (event) => { event.preventDefault(); zoomBy(Math.exp(-event.deltaY * 0.0015), event.clientX, event.clientY); }, { passive: false });

// ---------------------------------------------------------------- keyboard
canvas.addEventListener("keydown", (event) => {
  const node = nodeOf(event.target);
  if (event.key === "Escape") { keyLink = null; selectedEdge = null; if (gesture && gesture.kind === "link") endGesture(null); setHint(DEFAULT_HINT, false); draw(); return; }
  if ((event.key === "Delete" || event.key === "Backspace") && selectedEdge) { event.preventDefault(); unlink(selectedEdge); return; }
  if (!node) return;
  if ((event.key === "c" || event.key === "C") && node.kind === "zone") { keyLink = node.name; setHint("Linking from " + node.name + ": focus another zone and press Enter. Esc cancels.", true); draw(); focusAgain(node.id); return; }
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    if (keyLink && node.kind === "zone") { const from = keyLink; keyLink = null; setHint(DEFAULT_HINT, false); link(from, node.name); return; }
    if (node.open) vscode.postMessage({ type: "open", id: node.id });
  }
});
canvas.addEventListener("focusin", (event) => { const node = nodeOf(event.target); if (node) { focusNode(node.id); const box = event.target.getBoundingClientRect(); tipAt(box.right, box.top, details(node)); } });
canvas.addEventListener("focusout", () => blur());
function focusAgain(id) { const group = document.querySelector('.node[data-id="' + CSS.escape(id) + '"]'); if (group) group.focus(); }
document.addEventListener("keydown", (event) => { if ((event.key === "Delete" || event.key === "Backspace") && selectedEdge && document.activeElement === document.body) { event.preventDefault(); unlink(selectedEdge); } });

// ---------------------------------------------------------------- toolbar
const filterButton = $("filter"), filterMenu = $("filter-menu");
const setMenu = (open) => { filterMenu.hidden = !open; filterButton.setAttribute("aria-expanded", String(open)); if (open) { const box = filterButton.getBoundingClientRect(); filterMenu.style.left = Math.max(8, Math.min(window.innerWidth - 200, box.left)) + "px"; } };
filterButton.addEventListener("click", (event) => { event.stopPropagation(); setMenu(filterMenu.hidden); });
document.addEventListener("click", (event) => { if (!filterMenu.hidden && !filterMenu.contains(event.target)) setMenu(false); });
document.addEventListener("keydown", (event) => { if (event.key === "Escape" && !filterMenu.hidden) { setMenu(false); filterButton.focus(); } });
for (const box of filterMenu.querySelectorAll("input[data-kind]")) {
  box.checked = kinds[box.dataset.kind];
  box.addEventListener("change", () => { kinds[box.dataset.kind] = box.checked; persist(); draw(); });
}
$("search").addEventListener("input", (event) => { query = event.target.value; draw(); });
$("arrange").addEventListener("click", () => { if (!data) return; const { nodes, edges } = visible(); place(nodes, edges, true); vscode.postMessage({ type: "positions", positions: pos }); draw(); fit(); });
$("refresh").addEventListener("click", () => vscode.postMessage({ type: "refresh" }));
$("zoom-in").addEventListener("click", () => zoomBy(1.2));
$("zoom-out").addEventListener("click", () => zoomBy(1 / 1.2));
$("fit").addEventListener("click", fit);
document.addEventListener("click", (event) => { const command = event.target.closest("[data-command]"); if (command) vscode.postMessage({ type: "command", id: command.dataset.command }); });

window.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "data") {
    const first = !data;
    data = message.data || null;
    error = message.error || "";
    pos = Object.assign({}, message.positions || {}, pos);
    if (data) { const ids = new Set(data.nodes.map((node) => node.id)); for (const id of Object.keys(pos)) if (!ids.has(id)) delete pos[id]; }
    draw();
    if (first && data && !saved.view) fit();
  } else if (message.type === "hint") setHint("Drag from the handle on a zone's right edge onto another zone to link them. No typing needed.", true);
  else if (message.type === "result") {
    pending = pending.filter((item) => !(item.op === message.op && item.from === message.from && item.to === message.to));
    const result = message.result;
    say(result.ok ? "ok" : "bad", result.message + (result.hint && !result.ok ? " " + result.hint : ""), result.ok ? 4000 : 9000);
    draw();
  }
});
apply();
draw();
vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
}
