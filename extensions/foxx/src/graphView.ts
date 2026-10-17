// The Foxx graph: the repository's zones, flows, and selections as Crane reports them ('crane
// governance graph'), drawn as a directed acyclic graph in its own panel beside the editor. Hover a
// node for what it is (and which agents are working there), click it to open its code, and connect
// zones into a flow in execution order; the order is checked for cycles here, by the extension, and
// again by Crane before it is signed.

import * as vscode from "vscode";

import { findCycle } from "./core/governance";
import { GraphData, layout } from "./core/graph";
import { RunResult } from "./panel";

/** The extension side the graph talks to. */
export interface GraphHost {
  /** The latest graph, or why there is none. */
  load(refresh: boolean): Promise<{ data?: GraphData; error?: string }>;
  /** Open a node's code in the editor. */
  open(id: string): Promise<void>;
  /** Connect zones, in order, into a new flow. */
  connect(zones: string[], id: string, description: string): Promise<RunResult>;
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
   * @param connect start in connect mode
   */
  static show(extensionUri: vscode.Uri, host: GraphHost, connect = false): void {
    if (FoxxGraph.current) {
      FoxxGraph.current.panel.reveal(vscode.ViewColumn.Beside);
      if (connect) void FoxxGraph.current.panel.webview.postMessage({ type: "connect" });
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
    void FoxxGraph.current?.panel.webview.postMessage({ type: "data", data, error });
  }

  /**
   * Create the panel's controller.
   * @param panel webview panel
   * @param host extension side
   * @param connect start in connect mode
   */
  private constructor(private readonly panel: vscode.WebviewPanel, private readonly host: GraphHost, connect: boolean) {
    panel.webview.html = html(panel.webview.cspSource);
    panel.onDidDispose(() => (FoxxGraph.current = undefined));
    panel.webview.onDidReceiveMessage(async (message: { type: string; id?: string; zones?: string[]; description?: string }) => {
      if (message.type === "ready" || message.type === "refresh") {
        if (message.type === "ready" && connect) void panel.webview.postMessage({ type: "connect" });
        const { data, error } = await this.host.load(message.type === "refresh");
        FoxxGraph.update(data, error);
      } else if (message.type === "open" && message.id) await this.host.open(message.id);
      else if (message.type === "command" && message.id) await this.host.command(message.id);
      else if (message.type === "connect" && message.zones && message.id) {
        const result = await this.host.connect(message.zones, message.id, message.description ?? "");
        void panel.webview.postMessage({ type: "result", result });
        if (result.ok) {
          const { data, error } = await this.host.load(true);
          FoxxGraph.update(data, error);
        }
      }
    });
  }
}

/**
 * Build the graph's page: a strict content security policy (only this page's script and inline
 * styles), the layout and cycle check of core/ embedded as source, and the script that draws it.
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
  --zone: var(--vscode-charts-blue, #3794ff); --flow: var(--fox); --preserve: var(--vscode-charts-green, #3fb950); --target: var(--vscode-charts-purple, #b180d7);
  --ok: var(--vscode-testing-iconPassed, #3fb950); --bad: var(--vscode-errorForeground, #f14c4c); --warn: var(--vscode-editorWarning-foreground, #cca700);
}
* { box-sizing: border-box; }
html, body { margin: 0; height: 100%; }
body { display: flex; flex-direction: column; color: var(--fg); background: var(--bg); font: var(--vscode-font-size) / 1.45 var(--vscode-font-family); overflow: hidden; }
button, input { font: inherit; color: inherit; }
:focus-visible { outline: 1px solid var(--fox); outline-offset: 1px; }
.bar { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding: 8px 12px; border-bottom: 1px solid var(--line); background: color-mix(in srgb, var(--bg) 92%, transparent); }
.bar b { margin-right: 4px; }
.bar .grow { flex: 1; }
input[type=search], input[type=text] { min-width: 0; padding: 4px 8px; border-radius: 6px; border: 1px solid var(--vscode-input-border, var(--line)); background: var(--vscode-input-background); color: var(--vscode-input-foreground); }
input[type=search] { width: 200px; }
.btn { border: 1px solid var(--line); background: var(--raised); padding: 3px 10px; border-radius: 6px; cursor: pointer; white-space: nowrap; }
.btn:hover { border-color: var(--fox); }
.btn.on { border-color: var(--fox); color: var(--fox); background: var(--fox-soft); }
.btn.primary { border-color: transparent; color: #1a0d05; font-weight: 700; background: linear-gradient(90deg, var(--fox), #ffb347); }
.btn[disabled] { opacity: .5; cursor: not-allowed; }
.chip { display: inline-flex; align-items: center; gap: 5px; }
.chip i { width: 9px; height: 9px; border-radius: 3px; display: inline-block; }
.notice { margin: 8px 12px 0; padding: 7px 10px; border-radius: 7px; border: 1px solid color-mix(in srgb, var(--warn) 50%, transparent); background: color-mix(in srgb, var(--warn) 10%, transparent); font-size: 12px; }
.notice.bad { border-color: color-mix(in srgb, var(--bad) 50%, transparent); background: color-mix(in srgb, var(--bad) 10%, transparent); }
.notice.ok { border-color: color-mix(in srgb, var(--ok) 50%, transparent); background: color-mix(in srgb, var(--ok) 10%, transparent); }
.connect { display: none; flex-wrap: wrap; align-items: center; gap: 6px; padding: 8px 12px; border-bottom: 1px solid var(--line); background: var(--fox-soft); }
.connect.open { display: flex; }
.order { font-family: var(--vscode-editor-font-family, monospace); font-size: 12px; }
#stage { position: relative; flex: 1; min-height: 0; }
svg { width: 100%; height: 100%; display: block; cursor: grab; user-select: none; }
svg.panning { cursor: grabbing; }
.edge { fill: none; stroke: color-mix(in srgb, var(--fg) 34%, transparent); stroke-width: 1.4; transition: opacity .12s; }
.edge.stage, .edge.next { stroke: var(--flow); stroke-width: 2; }
.edge.nests { stroke-dasharray: 5 4; }
.edge.covers { stroke-dasharray: 2 4; }
.node { cursor: pointer; transition: opacity .12s; }
.node rect.box { fill: var(--raised); stroke: var(--line); stroke-width: 1.2; rx: 8; transition: stroke .12s, fill .12s; }
.node .bar-kind { rx: 2; }
.node text { fill: var(--fg); font-size: 12px; pointer-events: none; }
.node text.sub { fill: var(--muted); font-size: 10.5px; }
.node.proposed rect.box { stroke-dasharray: 4 3; }
.node.unresolved rect.box { stroke: var(--bad); }
.node:hover rect.box, .node.focus rect.box { stroke: var(--fox); fill: var(--fox-soft); stroke-width: 2; }
.node.match rect.box { stroke: var(--fox); stroke-width: 2; }
.node.picked rect.box { stroke: var(--fox); stroke-width: 2.4; fill: var(--fox-soft); }
.node.unpickable { opacity: .45; }
.pick { fill: var(--fox); } .pick-text { fill: #1a0d05; font-size: 10px; font-weight: 700; }
.pulse { fill: var(--ok); animation: pulse 1.6s ease-in-out infinite; }
@keyframes pulse { 50% { opacity: .35; } }
@media (prefers-reduced-motion: reduce) { .pulse { animation: none; } .node, .edge { transition: none; } }
svg.dim .node:not(.lit):not(.focus) { opacity: .25; }
svg.dim .edge:not(.lit) { opacity: .12; }
.edge.lit { stroke: var(--fox); stroke-width: 2.2; }
#tip { position: fixed; z-index: 5; pointer-events: none; max-width: 340px; padding: 8px 10px; border-radius: 8px; border: 1px solid var(--line); background: var(--vscode-editorHoverWidget-background, var(--bg)); color: var(--vscode-editorHoverWidget-foreground, var(--fg)); box-shadow: 0 8px 24px rgba(0, 0, 0, .3); font-size: 12px; display: none; }
#tip b { display: block; font-size: 13px; margin-bottom: 2px; overflow-wrap: anywhere; }
#tip .kind { font-size: 10px; letter-spacing: .7px; text-transform: uppercase; color: var(--muted); }
#tip .kv { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 1px 10px; margin-top: 5px; }
#tip .kv span:nth-child(odd) { color: var(--muted); }
#tip .agents { margin: 6px 0 0; padding-left: 14px; }
#tip .hint { margin-top: 6px; color: var(--muted); font-size: 11px; }
.empty { position: absolute; inset: 0; display: grid; place-items: center; text-align: center; color: var(--muted); padding: 20px; }
.empty b { display: block; color: var(--fg); margin-bottom: 4px; }
.legend { display: flex; flex-wrap: wrap; gap: 12px; padding: 6px 12px; border-top: 1px solid var(--line); font-size: 11px; color: var(--muted); }
.legend svg { width: 26px; height: 8px; display: inline-block; vertical-align: middle; cursor: default; }
</style>
</head>
<body>
<div class="bar" role="toolbar" aria-label="Graph tools">
  <b>Governance graph</b><span class="muted" id="summary"></span>
  <span class="grow"></span>
  <input type="search" id="search" placeholder="Find a zone, flow, selection…" aria-label="Find a node">
  <button class="btn chip on" data-kind="zone" aria-pressed="true"><i style="background:var(--zone)"></i>Zones</button>
  <button class="btn chip on" data-kind="flow" aria-pressed="true"><i style="background:var(--flow)"></i>Flows</button>
  <button class="btn chip on" data-kind="selection" aria-pressed="true"><i style="background:var(--preserve)"></i>Selections</button>
  <button class="btn" id="connect-toggle" title="Click zones in execution order to join them into a flow">Connect zones</button>
  <button class="btn" id="fit" title="Fit the graph to the panel">Fit</button>
  <button class="btn" id="refresh" title="Ask Crane for the graph again">Refresh</button>
</div>
<div class="connect" id="connect" aria-live="polite">
  <span>Click zones in execution order:</span><span class="order" id="order">none yet</span>
  <input type="text" id="flow-id" placeholder="flow id" aria-label="Flow identifier" size="16">
  <input type="text" id="flow-description" placeholder="why this flow exists" aria-label="Flow description" size="26">
  <button class="btn primary" id="create" disabled>Create flow</button>
  <button class="btn" id="clear">Clear</button>
  <button class="btn" id="cancel">Cancel</button>
</div>
<div id="notices"></div>
<div id="stage"><svg id="canvas" role="img" aria-label="Governance graph"><defs>
  <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="context-stroke"/></marker>
</defs><g id="viewport"></g></svg><div class="empty" id="empty" hidden></div></div>
<div class="legend">
  <span><svg viewBox="0 0 26 8"><path d="M0 4H26" stroke="var(--flow)" stroke-width="2"/></svg> stage order</span>
  <span><svg viewBox="0 0 26 8"><path d="M0 4H26" stroke="var(--muted)" stroke-width="1.4"/></svg> contains</span>
  <span><svg viewBox="0 0 26 8"><path d="M0 4H26" stroke="var(--muted)" stroke-width="1.4" stroke-dasharray="5 4"/></svg> nested selection</span>
  <span><svg viewBox="0 0 26 8"><path d="M0 4H26" stroke="var(--muted)" stroke-width="1.4" stroke-dasharray="2 4"/></svg> flow reaches zone</span>
  <span>dashed box: proposed · green dot: agent active now</span>
</div>
<div id="tip" role="tooltip"></div>
<script nonce="${nonce}">
"use strict";
const vscode = acquireVsCodeApi();
${shared}
const W = 176, H = 40;
const saved = vscode.getState() || {};
let data = null;
let error = "";
let kinds = Object.assign({ zone: true, flow: true, selection: true }, saved.kinds || {});
let query = "";
let picking = null;
let view = { scale: 1, x: 24, y: 24 };
let placed = {};
let size = { width: 0, height: 0 };
const $ = (id) => document.getElementById(id);
const esc = (text) => String(text == null ? "" : text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const clip = (text, n) => (text.length > n ? text.slice(0, n - 1) + "…" : text);
const ago = (at) => { const s = Math.max(0, Math.round((Date.now() - at) / 1000)); return s < 60 ? "just now" : s < 3600 ? Math.round(s / 60) + " min ago" : Math.round(s / 3600) + " h ago"; };
const color = (node) => node.kind === "zone" ? "var(--zone)" : node.kind === "flow" ? "var(--flow)" : node.operation === "target" ? "var(--target)" : "var(--preserve)";
const glyph = (node) => node.kind === "zone" ? "▦" : node.kind === "flow" ? "⇢" : node.operation === "target" ? "◎" : "⛨";
const activityOf = (id) => (data && data.activity || []).find((entry) => entry.node === id);
const persist = () => vscode.setState({ kinds });

function visible() {
  const nodes = data.nodes.filter((node) => kinds[node.kind]);
  const ids = new Set(nodes.map((node) => node.id));
  return { nodes, edges: data.edges.filter((edge) => ids.has(edge.from) && ids.has(edge.to)) };
}

function notices() {
  const list = [];
  if (error) list.push('<div class="notice bad" role="alert">' + esc(error) + ' <button class="btn" data-command="crane:download">Download latest Crane</button> <button class="btn" data-command="open:output">Show Crane output</button></div>');
  if (data && data.rejected && data.rejected.length) list.push('<div class="notice bad" role="alert"><b>Crane left out ' + data.rejected.length + " connection" + (data.rejected.length === 1 ? "" : "s") + " that would close a cycle.</b> " + data.rejected.slice(0, 3).map((edge) => esc(edge.reason)).join("; ") + ". Fix the stage order in .crane/governance.yaml; Crane will not sign it until the flows are acyclic.</div>");
  $("notices").innerHTML = list.join("");
}

function draw() {
  notices();
  const empty = $("empty");
  if (!data) { $("viewport").innerHTML = ""; empty.hidden = false; empty.innerHTML = error ? "" : "<div><b>Loading the graph…</b>Crane resolves the zones, flows, and selections.</div>"; return; }
  const { nodes, edges } = visible();
  const zones = data.nodes.filter((node) => node.kind === "zone").length, flows = data.nodes.filter((node) => node.kind === "flow").length, selections = data.nodes.filter((node) => node.kind === "selection").length;
  $("summary").textContent = "· " + zones + " zone" + (zones === 1 ? "" : "s") + " · " + flows + " flow" + (flows === 1 ? "" : "s") + " · " + selections + " selection" + (selections === 1 ? "" : "s") + " · generation " + data.generation + (data.acyclic ? " · acyclic" : "");
  if (!nodes.length) { $("viewport").innerHTML = ""; empty.hidden = false; empty.innerHTML = data.nodes.length ? "<div><b>Everything is filtered out</b>Turn a kind back on in the toolbar.</div>" : "<div><b>No zones, flows, or selections yet</b>Select code and press Ctrl+Alt+F to preserve it, or make it a zone.</div>"; return; }
  empty.hidden = true;
  const result = layout(nodes, edges, W + 64, H + 22);
  placed = result.placed;
  size = { width: result.width, height: result.height };
  const q = query.trim().toLowerCase();
  const dropped = new Set(result.dropped.map((edge) => edge.from + "|" + edge.to));
  const paths = edges.filter((edge) => !dropped.has(edge.from + "|" + edge.to)).map((edge) => {
    const a = placed[edge.from], b = placed[edge.to];
    const x1 = a.x + W, y1 = a.y + H / 2, x2 = b.x, y2 = b.y + H / 2, bend = Math.max(30, (x2 - x1) / 2);
    return '<path class="edge ' + edge.kind + '" data-from="' + esc(edge.from) + '" data-to="' + esc(edge.to) + '" d="M' + x1 + " " + y1 + " C" + (x1 + bend) + " " + y1 + " " + (x2 - bend) + " " + y2 + " " + x2 + " " + y2 + '" marker-end="url(#arrow)"><title>' + esc(edge.kind + (edge.label ? " · " + edge.label : "")) + "</title></path>";
  }).join("");
  const boxes = nodes.map((node) => {
    const at = placed[node.id];
    const busy = activityOf(node.id);
    const pickable = node.kind === "zone" && node.top_level;
    const pickIndex = picking ? picking.indexOf(node.name) : -1;
    const classes = ["node", node.status === "proposed" ? "proposed" : "", node.status === "UNRESOLVED" ? "unresolved" : "", q && (node.name + " " + node.id + " " + (node.description || "")).toLowerCase().includes(q) ? "match" : "", picking && !pickable ? "unpickable" : "", pickIndex >= 0 ? "picked" : ""].filter(Boolean).join(" ");
    const sub = node.kind === "selection" ? (node.operation || "") + (node.lines ? " · L" + node.lines[0] + "–" + node.lines[1] : " · unresolved") : node.kind + (node.status ? " · " + node.status : "") + (node.files != null ? " · " + node.files + " file" + (node.files === 1 ? "" : "s") : "");
    return '<g class="' + classes + '" tabindex="0" role="button" data-id="' + esc(node.id) + '" aria-label="' + esc(node.kind + " " + node.name) + '" transform="translate(' + at.x + " " + at.y + ')">' +
      '<rect class="box" width="' + W + '" height="' + H + '"></rect><rect class="bar-kind" x="0" y="0" width="4" height="' + H + '" fill="' + color(node) + '"></rect>' +
      '<text x="12" y="17">' + esc(glyph(node) + " " + clip(node.name, 20)) + '</text><text class="sub" x="12" y="31">' + esc(clip(sub, 28)) + "</text>" +
      (busy && busy.active ? '<circle class="pulse" cx="' + (W - 10) + '" cy="10" r="4"><title>agent active now</title></circle>' : "") +
      (pickIndex >= 0 ? '<circle class="pick" cx="' + (W - 12) + '" cy="' + (H - 12) + '" r="8"></circle><text class="pick-text" x="' + (W - 12) + '" y="' + (H - 8.5) + '" text-anchor="middle">' + (pickIndex + 1) + "</text>" : "") + "</g>";
  }).join("");
  $("viewport").innerHTML = paths + boxes;
  apply();
}

function apply() { $("viewport").setAttribute("transform", "translate(" + view.x + " " + view.y + ") scale(" + view.scale + ")"); }
function fit() {
  const box = $("stage").getBoundingClientRect();
  const width = size.width + W, height = size.height + H;
  view.scale = Math.max(0.2, Math.min(1.4, Math.min((box.width - 48) / width, (box.height - 48) / height)));
  view.x = (box.width - width * view.scale) / 2;
  view.y = Math.max(16, (box.height - height * view.scale) / 2);
  apply();
}

function details(node) {
  const rows = [];
  const add = (key, value) => { if (value !== undefined && value !== null && value !== "") rows.push("<span>" + esc(key) + "</span><span>" + esc(value) + "</span>"); };
  if (node.kind === "selection") { add("operation", node.operation); add("policy", node.policy); add("where", node.path ? node.path + " lines " + node.lines[0] + "–" + node.lines[1] : "not found in the code"); add("nested in", node.parent ? node.parent.replace("selection:", "") : ""); }
  else { add("status", node.status); add("criticality", node.criticality); add("autonomy ceiling", node.ceiling); add("files", node.files); if (node.stages && node.stages.length) add("stages", node.stages.join(" → ")); if (node.stage_of && node.stage_of.length) add("stage of", node.stage_of.join(", ")); }
  const busy = activityOf(node.id);
  const agents = busy ? '<div class="kind" style="margin-top:6px">Agents · last 24 h</div><ul class="agents">' + busy.agents.slice(0, 5).map((agent) => "<li>" + esc(agent.agent) + (agent.model ? " (" + esc(agent.model) + ")" : "") + " · " + agent.calls + " call" + (agent.calls === 1 ? "" : "s") + (agent.denied ? ", " + agent.denied + " denied" : "") + " · " + (agent.active ? "active now" : ago(agent.last_at)) + "</li>").join("") + "</ul>" : "";
  const hint = picking ? (node.kind === "zone" && node.top_level ? "Click to " + (picking.includes(node.name) ? "remove it from" : "add it to") + " the flow" : "Only zones that are not yet stages of a flow can be connected") : node.open ? "Click to open " + node.open.path + ":" + node.open.line : "No source location";
  return '<div class="kind">' + esc(node.kind) + '</div><b>' + esc(node.name) + "</b>" + (node.description ? '<div class="muted">' + esc(node.description) + "</div>" : "") + '<div class="kv">' + rows.join("") + "</div>" + agents + '<div class="hint">' + esc(hint) + "</div>";
}
function showTip(event, node) {
  const tip = $("tip");
  tip.innerHTML = details(node);
  tip.style.display = "block";
  const x = Math.min(window.innerWidth - tip.offsetWidth - 8, event.clientX + 14), y = Math.min(window.innerHeight - tip.offsetHeight - 8, event.clientY + 14);
  tip.style.left = Math.max(8, x) + "px"; tip.style.top = Math.max(8, y) + "px";
}
function focusNode(id) {
  const svg = $("canvas");
  svg.classList.add("dim");
  const near = new Set([id]);
  for (const path of svg.querySelectorAll(".edge")) {
    const lit = path.dataset.from === id || path.dataset.to === id;
    path.classList.toggle("lit", lit);
    if (lit) { near.add(path.dataset.from); near.add(path.dataset.to); }
  }
  for (const group of svg.querySelectorAll(".node")) { group.classList.toggle("lit", near.has(group.dataset.id)); group.classList.toggle("focus", group.dataset.id === id); }
}
function blur() {
  const svg = $("canvas");
  svg.classList.remove("dim");
  for (const item of svg.querySelectorAll(".lit, .focus")) item.classList.remove("lit", "focus");
  $("tip").style.display = "none";
}

// ---------------------------------------------------------------- connect zones
function connectBar() {
  $("connect").classList.toggle("open", Boolean(picking));
  $("connect-toggle").classList.toggle("on", Boolean(picking));
  if (!picking) return;
  $("order").textContent = picking.length ? picking.join(" → ") : "none yet";
  if (!$("flow-id").dataset.touched) $("flow-id").value = picking.join("-").slice(0, 64);
  $("create").disabled = picking.length < 2 || !/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test($("flow-id").value);
}
// The stage order that would result, checked like Crane does: no zone twice, no loop with other flows
function cycleWith(order) {
  const pairs = data.edges.filter((edge) => edge.kind === "next").map((edge) => [edge.from.replace(/^zone:/, ""), edge.to.replace(/^zone:/, "")]);
  for (let index = 1; index < order.length; index += 1) pairs.push([order[index - 1], order[index]]);
  return findCycle(pairs);
}
function pick(node) {
  if (!(node.kind === "zone" && node.top_level)) return;
  const at = picking.indexOf(node.name);
  if (at >= 0) picking.splice(at, 1);
  else {
    const cycle = cycleWith(picking.concat([node.name]));
    if (cycle) { say("bad", "Adding " + node.name + " would make the flows cyclic (" + cycle.join(" → ") + ")."); return; }
    picking.push(node.name);
  }
  connectBar();
  draw();
}
function say(tone, text) { $("notices").insertAdjacentHTML("afterbegin", '<div class="notice ' + tone + '" role="status">' + esc(text) + "</div>"); setTimeout(() => notices(), 6000); }
function startConnect() { picking = []; $("flow-id").dataset.touched = ""; $("flow-id").value = ""; $("flow-description").value = ""; kinds.zone = true; syncKinds(); connectBar(); draw(); }
function stopConnect() { picking = null; connectBar(); draw(); }

// ---------------------------------------------------------------- wiring
function syncKinds() { for (const button of document.querySelectorAll("[data-kind]")) { const on = kinds[button.dataset.kind]; button.classList.toggle("on", on); button.setAttribute("aria-pressed", String(on)); } }
const nodeOf = (target) => { const group = target.closest && target.closest(".node"); return group && data ? data.nodes.find((node) => node.id === group.dataset.id) : null; };
const canvas = $("canvas");
canvas.addEventListener("mouseover", (event) => { const node = nodeOf(event.target); if (node) { focusNode(node.id); showTip(event, node); } });
canvas.addEventListener("mousemove", (event) => { const node = nodeOf(event.target); if (node) showTip(event, node); });
canvas.addEventListener("mouseout", (event) => { if (nodeOf(event.target) && !nodeOf(event.relatedTarget || document.body)) blur(); });
canvas.addEventListener("focusin", (event) => { const node = nodeOf(event.target); if (node) { focusNode(node.id); const box = event.target.getBoundingClientRect(); showTip({ clientX: box.right, clientY: box.top }, node); } });
canvas.addEventListener("focusout", blur);
function activate(node) { if (picking) pick(node); else if (node.open) vscode.postMessage({ type: "open", id: node.id }); }
canvas.addEventListener("click", (event) => { if (moved) return; const node = nodeOf(event.target); if (node) activate(node); });
canvas.addEventListener("keydown", (event) => { const node = nodeOf(event.target); if (node && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); activate(node); } });
let drag = null, moved = false;
canvas.addEventListener("pointerdown", (event) => { moved = false; if (nodeOf(event.target)) return; drag = { x: event.clientX, y: event.clientY, vx: view.x, vy: view.y }; canvas.classList.add("panning"); canvas.setPointerCapture(event.pointerId); });
canvas.addEventListener("pointermove", (event) => { if (!drag) return; const dx = event.clientX - drag.x, dy = event.clientY - drag.y; if (Math.abs(dx) + Math.abs(dy) > 3) moved = true; view.x = drag.vx + dx; view.y = drag.vy + dy; apply(); });
canvas.addEventListener("pointerup", () => { drag = null; canvas.classList.remove("panning"); setTimeout(() => (moved = false), 0); });
canvas.addEventListener("wheel", (event) => {
  event.preventDefault();
  const box = canvas.getBoundingClientRect();
  const factor = Math.exp(-event.deltaY * 0.0015);
  const scale = Math.max(0.15, Math.min(3, view.scale * factor));
  const px = event.clientX - box.left, py = event.clientY - box.top;
  view.x = px - ((px - view.x) * scale) / view.scale; view.y = py - ((py - view.y) * scale) / view.scale; view.scale = scale;
  apply();
}, { passive: false });
for (const button of document.querySelectorAll("[data-kind]")) button.addEventListener("click", () => { kinds[button.dataset.kind] = !kinds[button.dataset.kind]; persist(); syncKinds(); draw(); fit(); });
$("search").addEventListener("input", (event) => { query = event.target.value; draw(); });
$("fit").addEventListener("click", fit);
$("refresh").addEventListener("click", () => vscode.postMessage({ type: "refresh" }));
$("connect-toggle").addEventListener("click", () => (picking ? stopConnect() : startConnect()));
$("cancel").addEventListener("click", stopConnect);
$("clear").addEventListener("click", () => { picking = []; connectBar(); draw(); });
$("flow-id").addEventListener("input", () => { $("flow-id").dataset.touched = "1"; connectBar(); });
$("create").addEventListener("click", () => {
  const cycle = cycleWith(picking);
  if (cycle) { say("bad", "This order would make the flows cyclic (" + cycle.join(" → ") + ")."); return; }
  $("create").disabled = true;
  vscode.postMessage({ type: "connect", zones: picking.slice(), id: $("flow-id").value.trim(), description: $("flow-description").value.trim() || picking.join(" → ") });
});
document.addEventListener("click", (event) => { const command = event.target.closest("[data-command]"); if (command) vscode.postMessage({ type: "command", id: command.dataset.command }); });
window.addEventListener("resize", () => { if (data) fit(); });
window.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "data") { const first = !data; data = message.data || null; error = message.error || ""; draw(); if (first || !data) fit(); }
  else if (message.type === "connect") startConnect();
  else if (message.type === "result") {
    say(message.result.ok ? "ok" : "bad", message.result.message + (message.result.hint ? " " + message.result.hint : ""));
    if (message.result.ok) stopConnect(); else connectBar();
  }
});
syncKinds();
draw();
vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
}
