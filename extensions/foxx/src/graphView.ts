// The Landscape graph: the repository's zones, flows, and selections as Crane reports them ('crane
// governance graph'), on a canvas beside the editor. Zones can be dragged anywhere (positions are
// remembered per workspace), and linked by dragging from one zone's connector onto another: the
// link becomes a flow's next stage in .crane/governance.yaml, signed by Crane. A zone or flow can
// be removed from its × handle after a confirmation that the removal is permanent (a flow's zones
// stay as zones; the subagents acting on it are terminated). Every link is checked here first (no
// self links, no cycles, a flow stays one ordered path), then again by the extension and by Crane.
// Hover a node for what it is and who is working there, click a zone (or double-click a flow) to
// open its code, filter the kinds from one menu. Clicking a flow opens a drawer under the graph: its
// test scenarios as a searchable carousel of cards a person edits, accepts (one by one or all at
// once), saves to .crane/scenarios, or downloads as text; and its subagents, whose pending
// creations and configurations a person approves or rejects here.

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
  /** Remove a zone, or a flow (its zones stay); the subagents acting on it are terminated. */
  remove(kind: "zone" | "flow", id: string): Promise<RunResult>;
  /** Run a command of the extension (fix Crane, open output). */
  command(id: string): Promise<void>;
  /** A flow's test scenarios: written from its knowledge (generate) or the saved ones (show). */
  scenarios(flow: string, action: "generate" | "show"): Promise<{ scenarios?: unknown[]; error?: string }>;
  /** Save reviewed scenarios to .crane/scenarios/FLOW.json. */
  saveScenarios(flow: string, scenarios: unknown[]): Promise<RunResult>;
  /** Ask where to save a text file, and write it. */
  saveText(name: string, text: string): Promise<RunResult>;
  /** A flow's subagents (all states) and the subagent catalog. */
  subagents(flow: string): Promise<{ subagents?: unknown[]; catalog?: unknown; error?: string }>;
  /** Approve or reject a subagent's pending change, or stop it. */
  decide(id: string, decision: "approve" | "reject" | "stop"): Promise<RunResult>;
}

/** A message from the graph page. */
interface GraphMessage {
  type: string;
  id?: string;
  from?: string;
  to?: string;
  kind?: string;
  flow?: string;
  name?: string;
  text?: string;
  decision?: string;
  scenarios?: unknown[];
  positions?: Positions;
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
    const panel = vscode.window.createWebviewPanel("foxx.graph", "Landscape Graph", { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false }, {
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
    panel.webview.onDidReceiveMessage(async (message: GraphMessage) => {
      const post = (reply: Record<string, unknown>) => void panel.webview.postMessage(reply);
      if (message.type === "flow" && message.flow) {
        const [tests, agents] = await Promise.all([this.host.scenarios(message.flow, "show"), this.host.subagents(message.flow)]);
        post({ type: "flowData", flow: message.flow, scenarios: tests.scenarios ?? [], testsError: tests.error, subagents: agents.subagents ?? [], catalog: agents.catalog, agentsError: agents.error });
        return;
      }
      if (message.type === "generate" && message.flow) {
        const tests = await this.host.scenarios(message.flow, "generate");
        post({ type: "scenarios", flow: message.flow, scenarios: tests.scenarios ?? [], error: tests.error });
        return;
      }
      if (message.type === "saveScenarios" && message.flow && Array.isArray(message.scenarios)) {
        post({ type: "result", op: "save", from: message.flow, to: "", result: await this.host.saveScenarios(message.flow, message.scenarios) });
        return;
      }
      if (message.type === "saveText" && message.name && typeof message.text === "string") {
        post({ type: "result", op: "download", from: message.name, to: "", result: await this.host.saveText(message.name, message.text) });
        return;
      }
      if (message.type === "decide" && message.id && message.flow && (message.decision === "approve" || message.decision === "reject" || message.decision === "stop")) {
        const result = await this.host.decide(message.id, message.decision);
        const agents = await this.host.subagents(message.flow);
        post({ type: "decided", flow: message.flow, id: message.id, decision: message.decision, result, subagents: agents.subagents ?? [], catalog: agents.catalog });
        return;
      }
      if (message.type === "ready" || message.type === "refresh") {
        if (message.type === "ready" && connect) void panel.webview.postMessage({ type: "hint" });
        const { data, error } = await this.host.load(message.type === "refresh");
        FoxxGraph.update(data, error);
      } else if (message.type === "open" && message.id) await this.host.open(message.id);
      else if (message.type === "positions" && message.positions) this.host.savePositions(message.positions);
      else if (message.type === "command" && message.id) await this.host.command(message.id);
      else if (message.type === "remove" && message.id && (message.kind === "zone" || message.kind === "flow")) {
        const result = await this.host.remove(message.kind, message.id);
        void panel.webview.postMessage({ type: "result", op: "remove", from: message.id, to: "", result });
        const { data, error } = await this.host.load(true);
        FoxxGraph.update(data, error);
      }
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
<title>Landscape Graph</title>
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
.edge-g:not(.removable):hover .edge { stroke: color-mix(in srgb, var(--fg) 60%, transparent); }
.edge-menu { position: fixed; z-index: 8; width: 270px; padding: 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--vscode-menu-background, var(--bg)); box-shadow: 0 12px 32px rgba(0, 0, 0, .4); font-size: 12px; }
.edge-menu .kind { font-size: 10px; letter-spacing: .7px; text-transform: uppercase; color: var(--muted); }
.edge-menu b { display: block; margin: 2px 0 6px; font-size: 13px; overflow-wrap: anywhere; }
.edge-menu p { margin: 0 0 10px; color: var(--muted); line-height: 1.45; }
.edge-menu .row { display: flex; gap: 6px; justify-content: flex-end; }
.btn.danger { border-color: transparent; background: var(--bad); color: #fff; font-weight: 600; }
.btn.danger:hover { filter: brightness(1.1); border-color: transparent; }
.btn.danger svg { width: 14px; height: 14px; }
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
.remove { opacity: 0; cursor: pointer; transition: opacity .12s; }
.remove circle { fill: var(--bg); stroke: var(--bad); stroke-width: 1.4; }
.remove path { stroke: var(--bad); stroke-width: 1.6; stroke-linecap: round; }
.node:hover .remove, .node.focus .remove, .remove:focus { opacity: 1; }
.remove:hover circle { fill: var(--bad); } .remove:hover path { stroke: #fff; }
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
/* The picked flow, and the drawer under the graph: its test scenarios and subagents */
.node.picked rect.box { stroke: var(--fox); stroke-width: 2.6; fill: color-mix(in srgb, var(--fox) 22%, var(--node)); }
.node.picked text { font-weight: 600; }
.drawer { flex: none; height: clamp(280px, 46vh, 520px); display: flex; flex-direction: column; border-top: 2px solid var(--fox); background: var(--bg); min-height: 0; }
.drawer-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 7px 12px; border-bottom: 1px solid var(--line); }
.drawer-head .kind, .tcard .src, .scard .kind { font-size: 10px; letter-spacing: .7px; text-transform: uppercase; color: var(--muted); }
.tabs { display: inline-flex; gap: 2px; margin-left: 8px; }
.tab { border: 0; border-bottom: 2px solid transparent; background: none; padding: 4px 9px; cursor: pointer; color: var(--muted); }
.tab[aria-selected=true] { color: var(--fg); border-bottom-color: var(--fox); }
.tab .count { margin-left: 4px; }
.drawer-body { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; }
.ttools { display: flex; flex-wrap: wrap; align-items: center; gap: 6px; padding: 7px 12px; }
.ttools input[type=search] { width: 170px; }
.ttools .pos { font-size: 11px; color: var(--muted); min-width: 52px; text-align: center; }
.btn.primary { background: var(--fox); border-color: var(--fox); color: #1a0d05; font-weight: 600; }
.btn:disabled { opacity: .5; cursor: default; }
.track { flex: 1; min-height: 0; display: flex; gap: 10px; padding: 2px 12px 12px; overflow-x: auto; overflow-y: hidden; scroll-snap-type: x mandatory; scroll-behavior: smooth; scrollbar-width: thin; }
.tcard { flex: 0 0 min(420px, 86%); scroll-snap-align: start; display: flex; flex-direction: column; gap: 6px; padding: 10px 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--raised); overflow-y: auto; }
.tcard.accepted { border-color: color-mix(in srgb, var(--ok) 60%, var(--line)); }
.tcard header { display: flex; align-items: center; gap: 6px; }
.tcard .ttitle { flex: 1; min-width: 0; font-weight: 600; padding: 3px 6px; border-radius: 5px; border: 1px solid transparent; background: transparent; }
.tcard .ttitle:hover, .tcard .ttitle:focus { border-color: var(--line); background: var(--vscode-input-background); }
.accept { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; white-space: nowrap; cursor: pointer; }
.ttype { font-size: 10px; font-weight: 700; text-transform: uppercase; letter-spacing: .5px; padding: 2px 6px; border-radius: 999px; background: var(--fox-soft); color: var(--fox); white-space: nowrap; }
.ttype.negative { background: color-mix(in srgb, var(--bad) 16%, transparent); color: var(--bad); }
.ttype.performance, .ttype.regression { background: color-mix(in srgb, var(--warn) 16%, transparent); color: var(--warn); }
.ttype.integration, .ttype.contract { background: color-mix(in srgb, var(--zone) 18%, transparent); color: var(--zone); }
.ttype.mocked, .ttype.smoke { background: color-mix(in srgb, var(--ok) 16%, transparent); color: var(--ok); }
.tdesc { margin: 0; font-size: 12px; color: var(--muted); }
.tgrid { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 6px; }
.tgrid label { display: grid; gap: 2px; font-size: 10.5px; color: var(--muted); min-width: 0; }
.tgrid .wide { grid-column: 1 / -1; }
.tgrid input, .tgrid textarea, .tgrid select, .mjson { width: 100%; min-width: 0; padding: 4px 6px; border-radius: 5px; border: 1px solid var(--vscode-input-border, var(--line)); background: var(--vscode-input-background); color: var(--vscode-input-foreground); font: 12px var(--vscode-editor-font-family, monospace); }
.tgrid textarea { resize: vertical; min-height: 34px; }
.tgrid .bad { border-color: var(--bad); }
.tgrid .why { color: var(--bad); font-size: 10.5px; }
.tempty { flex: 1; display: grid; place-items: center; text-align: center; color: var(--muted); padding: 16px; }
.slist { flex: 1; min-height: 0; overflow: auto; display: grid; grid-template-columns: repeat(auto-fill, minmax(280px, 1fr)); gap: 10px; padding: 8px 12px 12px; align-content: start; }
.scard { display: grid; gap: 6px; padding: 10px 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--raised); font-size: 12px; }
.scard.pending { border-color: color-mix(in srgb, var(--warn) 60%, var(--line)); }
.scard b { font-size: 13px; }
.scard .pills { display: flex; flex-wrap: wrap; gap: 4px; }
.pill { font-size: 10.5px; padding: 1px 7px; border-radius: 999px; border: 1px solid var(--line); color: var(--muted); }
.pill.ok { color: var(--ok); border-color: color-mix(in srgb, var(--ok) 50%, var(--line)); }
.pill.warn { color: var(--warn); border-color: color-mix(in srgb, var(--warn) 50%, var(--line)); }
.pill.bad { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 50%, var(--line)); }
.scard pre { margin: 0; max-height: 140px; overflow: auto; padding: 6px 8px; border-radius: 6px; background: var(--bg); border: 1px solid var(--line); font: 11px var(--vscode-editor-font-family, monospace); white-space: pre-wrap; }
.scard .row, .modal .row { display: flex; gap: 6px; justify-content: flex-end; flex-wrap: wrap; }
.scaps { padding: 6px 12px 0; font-size: 12px; color: var(--muted); }
.modal { position: fixed; inset: 0; z-index: 20; display: grid; place-items: center; padding: 16px; background: rgba(0, 0, 0, .45); }
.modal .box { width: min(520px, 100%); max-height: calc(100vh - 32px); overflow: auto; display: grid; gap: 10px; padding: 16px; border-radius: 12px; border: 1px solid var(--line); background: var(--vscode-editorWidget-background, var(--bg)); box-shadow: 0 16px 40px rgba(0, 0, 0, .45); }
.modal h2 { margin: 0; font-size: 15px; }
.modal p { margin: 0; color: var(--muted); }
.modal .err { white-space: pre-wrap; color: var(--fg); padding: 8px 10px; border-radius: 7px; border: 1px solid color-mix(in srgb, var(--bad) 55%, transparent); background: color-mix(in srgb, var(--bad) 12%, var(--bg)); font-size: 12px; }
.confirm { display: flex; align-items: flex-start; gap: 6px; font-size: 12px; margin: 0 0 10px; cursor: pointer; }
@media (max-width: 640px) { .tgrid { grid-template-columns: 1fr 1fr; } }
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
<div class="hint" id="hint"><span class="dot" aria-hidden="true"></span><span id="hint-text">Drag from a zone's right handle onto another zone to link them into a flow. Click a link to delete it. Hover a zone or flow and press × (or Delete) to remove it. Drag zones to move them. Click a flow for its tests and subagents.</span></div>
<div id="stage">
  <div class="notices" id="notices" aria-live="polite"></div>
  <svg id="canvas" role="application" aria-label="Governance graph canvas"><defs>
    <marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="context-stroke"/></marker>
  </defs><g id="viewport"><g id="edges"></g><path id="temp" d="" hidden></path><g id="nodes"></g></g></svg>
  <div class="empty" id="empty" hidden></div>
  <div class="zoom"><button id="zoom-in" title="Zoom in" aria-label="Zoom in">+</button><button id="zoom-out" title="Zoom out" aria-label="Zoom out">−</button><button id="fit" title="Fit the graph" aria-label="Fit the graph">⤢</button></div>
</div>
<section class="drawer" id="drawer" aria-label="Flow tests and subagents" hidden>
  <div class="drawer-head">
    <span class="kind">Flow</span><b id="drawer-title"></b>
    <div class="tabs" role="tablist" aria-label="Show">
      <button class="tab" role="tab" data-tab="tests" aria-selected="true">Tests <span class="count" id="test-count" hidden></span></button>
      <button class="tab" role="tab" data-tab="subagents" aria-selected="false">Subagents <span class="count" id="sub-count" hidden></span></button>
    </div>
    <span class="grow"></span>
    <button class="btn" id="drawer-open" title="Open the flow's code">Open code</button>
    <button class="btn" id="drawer-close" aria-label="Close the drawer" title="Close">×</button>
  </div>
  <div class="drawer-body" id="drawer-body"></div>
</section>
<div class="modal" id="modal" role="dialog" aria-modal="true" aria-labelledby="modal-title" hidden></div>
<div id="tip" role="tooltip"></div>
<div class="edge-menu" id="edge-menu" role="dialog" aria-labelledby="edge-menu-title" hidden></div>
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
    return '<g class="edge-g' + (removable ? " removable" : "") + (selected ? " selected" : "") + '" data-from="' + esc(edge.from) + '" data-to="' + esc(edge.to) + '" data-kind="' + edge.kind + '" data-label="' + esc(edge.label || "") + '">' +
      (edge.pending ? "" : '<path class="hit" d="' + d + '"><title>' + esc(edgeTitle(edge) + (removable ? " · click to delete" : " · click for details")) + "</title></path>") +
      '<path class="edge ' + edge.kind + (edge.pending || unlinking ? " pending" : "") + '" d="' + d + '" marker-end="url(#arrow)"></path>' + "</g>";
  }).join("");
  $("nodes").innerHTML = nodes.map((node) => {
    const at = pos[node.id];
    const busy = activityOf(node.id);
    const classes = ["node", node.kind === "flow" && node.name === picked ? "picked" : "", node.status === "proposed" ? "proposed" : "", node.status === "UNRESOLVED" ? "unresolved" : "", q && (node.name + " " + node.id + " " + (node.description || "")).toLowerCase().includes(q) ? "match" : "", keyLink && keyLink === node.name && node.kind === "zone" ? "source" : ""].filter(Boolean).join(" ");
    const where = node.kind === "zone" ? placeOf(node.name) : null;
    const sub = node.kind === "selection" ? (node.operation || "") + (node.lines ? " · L" + node.lines[0] + "–" + node.lines[1] : " · unresolved") : node.kind === "zone" ? (where.flow ? "stage " + (where.index + 1) + " of " + where.flow : "zone") + (node.status ? " · " + node.status : "") : "flow · " + (node.stages || []).length + " stage" + ((node.stages || []).length === 1 ? "" : "s") + (node.status ? " · " + node.status : "");
    return '<g class="' + classes + '" tabindex="0" role="button" data-id="' + esc(node.id) + '" aria-label="' + esc(node.kind + " " + node.name + (node.kind === "zone" ? ". Press C to link it, Enter to open." : ". Enter to open.")) + '" transform="translate(' + at.x + " " + at.y + ')">' +
      '<rect class="box" width="' + W + '" height="' + H + '"></rect><rect x="0" y="6" width="3.5" height="' + (H - 12) + '" rx="1.75" fill="' + color(node) + '"></rect>' +
      '<text x="14" y="19">' + esc(glyph(node) + " " + clip(node.name, 21)) + '</text><text class="sub" x="14" y="34">' + esc(clip(sub, 30)) + "</text>" +
      (busy && busy.active ? '<circle class="pulse" cx="' + (W - 14) + '" cy="12" r="4"><title>agent active now</title></circle>' : "") +
      (node.kind === "zone" ? '<circle class="port in" cx="0" cy="' + H / 2 + '" r="6"></circle><circle class="port out" tabindex="-1" data-port="' + esc(node.name) + '" cx="' + W + '" cy="' + H / 2 + '" r="6"><title>Drag onto another zone to link</title></circle>' : "") +
      (node.kind === "zone" || node.kind === "flow" ? '<g class="remove" data-remove="' + esc(node.id) + '" transform="translate(' + (W - 10) + ' -6)"><title>Remove this ' + node.kind + '</title><circle r="7.5"></circle><path d="M-3 -3L3 3M3 -3L-3 3"></path></g>' : "") + "</g>";
  }).join("");
  apply();
}
const nameOf = (id) => { const node = nodeById(id); return node ? node.name : id.replace(/^[a-z]+:/, ""); };
function edgeTitle(edge) { return nameOf(edge.from) + " → " + nameOf(edge.to); }
// What each kind of edge means, and how to change it when it cannot be deleted from here
function edgeDetails(edge) {
  const from = nameOf(edge.from), to = nameOf(edge.to);
  if (edge.kind === "next") return { kind: "Link in flow " + (edge.label || placeOf(to).flow || ""), title: from + " → " + to, text: "Deleting it splits the flow here. A zone left on its own becomes a free zone again; Crane signs the change.", removable: true };
  if (edge.kind === "stage") return { kind: "Flow start", title: "Flow " + from + " starts at " + to, text: "This shows where the flow begins. To change it, delete the link after " + to + ", or link another zone before it." };
  if (edge.kind === "contains") return { kind: "Contains", title: from + " contains " + to, text: "The selection lies in the files this " + (edge.from.startsWith("flow:") ? "flow" : "zone") + " covers. It follows the code: move or remove the selection to change it." };
  if (edge.kind === "nests") return { kind: "Nested selection", title: to + " is inside " + from, text: "A nested selection keeps its own rule inside its parent's lines." };
  return { kind: "Reaches", title: "Flow " + from + " reaches " + to, text: "The flow's calls reach this zone's files without it being a stage. Link it into the flow to make it a stage." };
}
const TRASH = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/></svg>';
function openEdgeMenu(edge, x, y) {
  const menu = $("edge-menu");
  const info = edgeDetails(edge);
  menu.innerHTML = '<div class="kind">' + esc(info.kind) + '</div><b id="edge-menu-title">' + esc(info.title) + "</b><p>" + esc(info.text) + '</p><div class="row">' +
    (info.removable ? '<button class="btn danger" id="edge-delete">' + TRASH + "Delete link</button>" : "") + '<button class="btn" id="edge-close">' + (info.removable ? "Cancel" : "Close") + "</button></div>";
  menu.hidden = false;
  menu.style.left = Math.max(8, Math.min(window.innerWidth - menu.offsetWidth - 8, x + 10)) + "px";
  menu.style.top = Math.max(8, Math.min(window.innerHeight - menu.offsetHeight - 8, y + 10)) + "px";
  const remove = $("edge-delete");
  if (remove) remove.addEventListener("click", () => { closeEdgeMenu(); unlink(edge); });
  $("edge-close").addEventListener("click", () => { closeEdgeMenu(); selectedEdge = null; draw(); });
  (remove || $("edge-close")).focus();
}
function closeEdgeMenu() { $("edge-menu").hidden = true; }
// What removing a zone or flow does, said before it is done
function openRemoveMenu(node, x, y) {
  const menu = $("edge-menu");
  const where = node.kind === "zone" ? placeOf(node.name) : null;
  const text = node.kind === "flow"
    ? "Its " + (node.stages || []).length + " zone(s) stay as zones of their own. Bricks and resident agents attached to the flow stop applying, and every subagent of the flow is terminated."
    : (where && where.flow ? "Flow " + where.flow + " splits at this zone; the zones around it stay, and the subagents of flow " + where.flow + " are terminated. " : "") + "Its own context pack goes with it. Bricks and resident agents attached to the zone stop applying.";
  menu.innerHTML = '<div class="kind">Remove ' + esc(node.kind) + ' permanently</div><b id="edge-menu-title">' + esc(node.name) + "</b><p>" + esc(text) + " Crane validates and signs the change. Terminated subagents are not restarted, even if the governance change is undone.</p>" +
    '<label class="confirm"><input type="checkbox" id="node-confirm"> I understand this removal is permanent and stops its subagents.</label>' +
    '<div class="row"><button class="btn danger" id="node-remove" disabled>' + TRASH + "Remove " + esc(node.kind) + '</button><button class="btn" id="node-cancel">Cancel</button></div>';
  $("node-confirm").addEventListener("change", (event) => { $("node-remove").disabled = !event.target.checked; });
  menu.hidden = false;
  menu.style.left = Math.max(8, Math.min(window.innerWidth - menu.offsetWidth - 8, x + 10)) + "px";
  menu.style.top = Math.max(8, Math.min(window.innerHeight - menu.offsetHeight - 8, y + 10)) + "px";
  $("node-remove").addEventListener("click", () => {
    closeEdgeMenu();
    say("ok", "Removing " + node.kind + " " + node.name + "…", 2500);
    vscode.postMessage({ type: "remove", kind: node.kind, id: node.name });
  });
  $("node-cancel").addEventListener("click", closeEdgeMenu);
  $("node-cancel").focus();
}
// Move one node without redrawing everything: its transform and the edges that touch it
function moveNode(id) {
  const group = document.querySelector('.node[data-id="' + CSS.escape(id) + '"]');
  if (group) group.setAttribute("transform", "translate(" + pos[id].x + " " + pos[id].y + ")");
  for (const edge of document.querySelectorAll(".edge-g")) {
    if (edge.dataset.from !== id && edge.dataset.to !== id) continue;
    const d = curve(anchor(edge.dataset.from, "out"), anchor(edge.dataset.to, "in"));
    for (const path of edge.querySelectorAll("path.edge, path.hit")) path.setAttribute("d", d);
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
  const removing = event.target.closest("[data-remove]");
  if (removing) { closeEdgeMenu(); const node = nodeById(removing.dataset.remove); if (node) openRemoveMenu(node, event.clientX, event.clientY); return; }
  const port = event.target.closest("[data-port]");
  const node = nodeOf(event.target);
  closeEdgeMenu();
  const edge = event.target.closest(".edge-g") && event.target.closest("path.hit") ? event.target.closest(".edge-g") : null;
  if (port) {
    gesture = { kind: "link", from: port.dataset.port, start: anchor("zone:" + port.dataset.port, "out"), target: null };
    canvas.classList.add("linking");
    setHint("Drop on a zone to link " + port.dataset.port + " to it. Esc cancels.", true);
  } else if (node) {
    const point = toGraph(event.clientX, event.clientY);
    gesture = { kind: "move", id: node.id, dx: point.x - pos[node.id].x, dy: point.y - pos[node.id].y, x: event.clientX, y: event.clientY, moved: false };
  } else if (edge) {
    selectedEdge = { from: edge.dataset.from, to: edge.dataset.to, kind: edge.dataset.kind, label: edge.dataset.label };
    draw();
    openEdgeMenu(selectedEdge, event.clientX, event.clientY);
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
    else if (event && event.type === "pointerup") {
      // A flow opens its tests and subagents (double-click opens its code); a zone opens its code
      const node = nodeById(done.id);
      if (node && node.kind === "flow") pickFlow(node.name);
      else if (node && node.open) vscode.postMessage({ type: "open", id: node.id });
    }
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
  if (event.key === "Escape") { closeEdgeMenu(); keyLink = null; selectedEdge = null; if (gesture && gesture.kind === "link") endGesture(null); setHint(DEFAULT_HINT, false); draw(); return; }
  if ((event.key === "Delete" || event.key === "Backspace") && selectedEdge && selectedEdge.kind === "next") { event.preventDefault(); closeEdgeMenu(); unlink(selectedEdge); return; }
  if (!node) return;
  if ((event.key === "Delete" || event.key === "Backspace") && (node.kind === "zone" || node.kind === "flow")) { event.preventDefault(); const box = event.target.getBoundingClientRect(); openRemoveMenu(node, box.left, box.bottom); return; }
  if ((event.key === "c" || event.key === "C") && node.kind === "zone") { keyLink = node.name; setHint("Linking from " + node.name + ": focus another zone and press Enter. Esc cancels.", true); draw(); focusAgain(node.id); return; }
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    if (keyLink && node.kind === "zone") { const from = keyLink; keyLink = null; setHint(DEFAULT_HINT, false); link(from, node.name); return; }
    if (node.kind === "flow") { pickFlow(node.name); return; }
    if (node.open) vscode.postMessage({ type: "open", id: node.id });
  }
});
canvas.addEventListener("focusin", (event) => { const node = nodeOf(event.target); if (node) { focusNode(node.id); const box = event.target.getBoundingClientRect(); tipAt(box.right, box.top, details(node)); } });
canvas.addEventListener("focusout", () => blur());
function focusAgain(id) { const group = document.querySelector('.node[data-id="' + CSS.escape(id) + '"]'); if (group) group.focus(); }
document.addEventListener("keydown", (event) => {
  if (event.key === "Escape" && !$("edge-menu").hidden) { closeEdgeMenu(); selectedEdge = null; draw(); return; }
  if ((event.key === "Delete" || event.key === "Backspace") && selectedEdge && selectedEdge.kind === "next" && !event.target.closest("input")) { event.preventDefault(); closeEdgeMenu(); unlink(selectedEdge); }
});

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

// ---------------------------------------------------------------- the drawer: a flow's tests and subagents
// Click a flow to pick it. Its test scenarios (written by Crane from the flow's context, Bricks,
// and zones) show as a carousel of cards a person edits and accepts, then saves to the repository
// or downloads as text. Its subagents show with their pending changes to approve or reject.
const NL = String.fromCharCode(10);
let picked = null;
let tab = "tests";
let tests = [];
let testQuery = "";
let subagents = [];
let catalog = null;
let dirty = false;
const TYPES = ["smoke", "mocked", "integration", "performance", "negative", "contract", "regression"];
function pickFlow(name) {
  if (picked !== name && dirty && !window.confirm("Leave the unsaved test edits of " + picked + "?")) return;
  picked = name;
  tests = []; subagents = []; catalog = null; dirty = false; testQuery = "";
  $("drawer").hidden = false;
  $("drawer-title").textContent = name;
  $("drawer-body").innerHTML = '<div class="tempty">Loading ' + esc(name) + '…</div>';
  draw();
  vscode.postMessage({ type: "flow", flow: name });
}
function closeDrawer() {
  if (dirty && !window.confirm("Leave the unsaved test edits of " + picked + "?")) return;
  picked = null; dirty = false; $("drawer").hidden = true; draw();
}
function setTab(name) {
  tab = name;
  for (const button of document.querySelectorAll(".tab")) button.setAttribute("aria-selected", String(button.dataset.tab === name));
  drawDrawer();
}
function counts() {
  $("test-count").hidden = !tests.length; $("test-count").textContent = String(tests.length);
  const waiting = subagents.filter((agent) => agent.status === "PENDING_APPROVAL" || agent.pending).length;
  $("sub-count").hidden = !waiting; $("sub-count").textContent = String(waiting);
}
function drawDrawer() {
  if (!picked) return;
  counts();
  if (tab === "tests") drawTests(); else drawSubagents();
}
// -------- tests
const pretty = (value) => (value === null || value === undefined || (typeof value === "object" && !Array.isArray(value) && !Object.keys(value).length) ? "" : JSON.stringify(value, null, 2));
function shown() {
  const q = testQuery.trim().toLowerCase();
  return tests.filter((item) => !q || [item.type, item.title, item.description, item.request && item.request.url, item.source].join(" ").toLowerCase().includes(q));
}
function drawTests() {
  const list = shown();
  const accepted = tests.filter((item) => item.accepted).length;
  const body = $("drawer-body");
  const tools = '<div class="ttools" role="toolbar" aria-label="Test scenarios">' +
    '<input type="search" id="test-search" placeholder="Search tests…" aria-label="Search tests" value="' + esc(testQuery) + '">' +
    '<button class="btn" id="test-prev" aria-label="Previous test" title="Previous">‹</button><span class="pos" id="test-pos" aria-live="polite"></span><button class="btn" id="test-next" aria-label="Next test" title="Next">›</button>' +
    '<span class="muted" style="font-size:11px">' + accepted + " of " + tests.length + ' accepted' + (dirty ? " · unsaved" : "") + "</span>" +
    '<span class="grow"></span>' +
    '<button class="btn" id="test-generate" title="Write scenarios from the flow\\'s context, Bricks, and zones (saved edits are kept)">' + (tests.length ? "Generate again" : "Generate tests") + "</button>" +
    '<button class="btn" id="test-accept-all"' + (tests.length && accepted < tests.length ? "" : " disabled") + ">I accept all…</button>" +
    '<button class="btn primary" id="test-save"' + (tests.length ? "" : " disabled") + ' title="Save to .crane/scenarios/' + esc(picked) + '.json">Save to repository</button>' +
    '<button class="btn" id="test-download"' + (tests.length ? "" : " disabled") + ">Download .txt</button></div>";
  const cards = list.length ? '<div class="track" id="track">' + list.map(card).join("") + "</div>"
    : '<div class="tempty"><div>' + (tests.length ? "No test matches the search." : "<b>No test scenarios yet</b><br>Generate them from the flow's starting points, context, Bricks, and zones; then review the values, accept them, and save.") + "</div></div>";
  body.innerHTML = tools + cards;
  wireTests();
}
function card(item) {
  const request = item.request || {}, expect = item.expect || {};
  const field = (label, key, value, kind, wide) => '<label' + (wide ? ' class="wide"' : "") + ">" + esc(label) +
    (kind === "area" ? '<textarea data-f="' + key + '" rows="2" spellcheck="false">' + esc(value) + "</textarea>" : '<input data-f="' + key + '"' + (kind === "number" ? ' type="number"' : "") + ' value="' + esc(value) + '" spellcheck="false">') + '<span class="why" hidden></span></label>';
  const outcome = '<label>Expect<select data-f="outcome">' + ["success", "blocked", "within"].map((name) => '<option value="' + name + '"' + (expect.outcome === name ? " selected" : "") + ">" + name + "</option>").join("") + "</select></label>";
  return '<article class="tcard' + (item.accepted ? " accepted" : "") + '" data-id="' + esc(item.id) + '" aria-label="' + esc(item.type + " test " + item.title) + '">' +
    '<header><select data-f="type" class="ttype ' + esc(item.type) + '" aria-label="Type">' + TYPES.map((name) => '<option value="' + name + '"' + (item.type === name ? " selected" : "") + ">" + name + "</option>").join("") + "</select>" +
    '<input class="ttitle" data-f="title" value="' + esc(item.title) + '" aria-label="Title">' +
    '<label class="accept" title="Accept this test"><input type="checkbox" data-f="accepted"' + (item.accepted ? " checked" : "") + "> Accepted</label></header>" +
    (item.description ? '<p class="tdesc">' + esc(item.description) + "</p>" : "") +
    '<div class="tgrid">' + field("Method", "method", request.method || "", "text") + field("URL or function", "url", request.url || "", "text") + outcome +
    field("Parameters (JSON)", "params", pretty(request.params), "area", true) + field("Body (JSON)", "body", pretty(request.body), "area", true) +
    field("Mocks (comma separated)", "mocks", (item.mocks || []).join(", "), "text", true) +
    field("HTTP status", "status", expect.status == null ? "" : String(expect.status), "number") + field("Within ms", "latency", expect.latency_ms == null ? "" : String(expect.latency_ms), "number") + "<span></span>" +
    field("Assertions (one per line)", "assertions", (expect.assertions || []).join(NL), "area", true) + "</div>" +
    '<div class="src">from ' + esc(item.source || "a person") + " · " + esc(item.id) + (item.reviewed_by ? " · reviewed by " + esc(item.reviewed_by) : "") + "</div></article>";
}
function parseJsonField(text) {
  if (!text.trim()) return { ok: true, value: null };
  try { return { ok: true, value: JSON.parse(text) }; } catch (problem) { return { ok: false, why: "Not valid JSON: " + problem.message }; }
}
function wireTests() {
  const search = $("test-search");
  search.addEventListener("input", () => { testQuery = search.value; const at = search.selectionStart; drawTests(); const again = $("test-search"); again.focus(); again.setSelectionRange(at, at); });
  $("test-generate").addEventListener("click", () => { $("test-generate").disabled = true; say("ok", "Writing test scenarios for " + picked + "…", 2000); vscode.postMessage({ type: "generate", flow: picked }); });
  $("test-accept-all").addEventListener("click", () => openModal("Accept all test scenarios?", "All " + tests.length + " scenarios of " + picked + " will be marked accepted, with you as the reviewer once saved. Review the values first; you can still uncheck any of them before saving.", "I accept all", () => { for (const item of tests) item.accepted = true; dirty = true; drawTests(); }));
  $("test-save").addEventListener("click", () => {
    const broken = document.querySelector(".tgrid .bad");
    if (broken) { openModal("Fix the values first", "A JSON field is not valid; it is outlined in red.", null, null); broken.focus(); return; }
    vscode.postMessage({ type: "saveScenarios", flow: picked, scenarios: tests });
  });
  $("test-download").addEventListener("click", () => vscode.postMessage({ type: "saveText", name: picked + "-tests.txt", text: asText() }));
  const track = $("track");
  const step = (direction) => { if (!track) return; const width = track.querySelector(".tcard") ? track.querySelector(".tcard").offsetWidth + 10 : 300; track.scrollBy({ left: direction * width }); };
  $("test-prev").addEventListener("click", () => step(-1));
  $("test-next").addEventListener("click", () => step(1));
  const position = () => { const list = shown(); if (!track || !list.length) { $("test-pos").textContent = list.length ? "" : "0 / 0"; return; } const width = (track.querySelector(".tcard").offsetWidth || 1) + 10; $("test-pos").textContent = Math.min(list.length, Math.round(track.scrollLeft / width) + 1) + " / " + list.length; };
  if (track) { track.addEventListener("scroll", position, { passive: true }); track.addEventListener("keydown", (event) => { if (event.target === track && event.key === "ArrowRight") step(1); if (event.target === track && event.key === "ArrowLeft") step(-1); }); }
  position();
  for (const article of document.querySelectorAll(".tcard")) {
    const item = tests.find((candidate) => candidate.id === article.dataset.id);
    if (!item) continue;
    for (const control of article.querySelectorAll("[data-f]")) {
      const update = () => {
        const key = control.dataset.f;
        item.request = item.request || {}; item.expect = item.expect || {};
        const why = control.parentElement.querySelector(".why");
        control.classList.remove("bad"); if (why) why.hidden = true;
        if (key === "accepted") { item.accepted = control.checked; article.classList.toggle("accepted", control.checked); if (!control.checked) item.reviewed_by = undefined; }
        else if (key === "title" || key === "type") { item[key] = control.value; if (key === "type") control.className = "ttype " + control.value; }
        else if (key === "method" || key === "url") item.request[key] = control.value.trim();
        else if (key === "params" || key === "body") {
          const parsed = parseJsonField(control.value);
          if (!parsed.ok) { control.classList.add("bad"); if (why) { why.textContent = parsed.why; why.hidden = false; } return; }
          if (key === "params" && parsed.value !== null && (typeof parsed.value !== "object" || Array.isArray(parsed.value))) { control.classList.add("bad"); if (why) { why.textContent = "Parameters must be a JSON object."; why.hidden = false; } return; }
          item.request[key] = key === "params" ? (parsed.value || {}) : parsed.value;
        }
        else if (key === "mocks") item.mocks = control.value.split(",").map((text) => text.trim()).filter(Boolean);
        else if (key === "outcome") item.expect.outcome = control.value;
        else if (key === "status") item.expect.status = control.value === "" ? undefined : Number(control.value);
        else if (key === "latency") item.expect.latency_ms = control.value === "" ? undefined : Number(control.value);
        else if (key === "assertions") item.expect.assertions = control.value.split(NL).map((text) => text.trim()).filter(Boolean);
        dirty = true;
        const note = document.querySelector(".ttools .muted");
        const accepted = tests.filter((test) => test.accepted).length;
        if (note) note.textContent = accepted + " of " + tests.length + " accepted · unsaved";
        $("test-accept-all").disabled = accepted === tests.length;
      };
      control.addEventListener(control.tagName === "SELECT" || control.type === "checkbox" ? "change" : "input", update);
    }
  }
}
function asText() {
  const lines = ["Test scenarios of flow " + picked + " (" + tests.length + " total, " + tests.filter((item) => item.accepted).length + " accepted)", ""];
  tests.forEach((item, index) => {
    const request = item.request || {}, expect = item.expect || {};
    lines.push((index + 1) + ". [" + item.type + "] " + item.title + (item.accepted ? " (accepted)" : ""));
    lines.push("   id: " + item.id + " · from: " + (item.source || "a person"));
    if (item.description) lines.push("   " + item.description);
    if (request.method || request.url) lines.push("   request: " + (request.method || "") + " " + (request.url || ""));
    if (request.params && Object.keys(request.params).length) lines.push("   params: " + JSON.stringify(request.params));
    if (request.body !== null && request.body !== undefined) lines.push("   body: " + JSON.stringify(request.body));
    if ((item.mocks || []).length) lines.push("   mocks: " + item.mocks.join(", "));
    lines.push("   expect: " + (expect.outcome || "success") + (expect.status ? ", HTTP " + expect.status : "") + (expect.latency_ms ? ", within " + expect.latency_ms + " ms" : ""));
    for (const assertion of expect.assertions || []) lines.push("   - " + assertion);
    lines.push("");
  });
  return lines.join(NL);
}
// -------- subagents
function drawSubagents() {
  const body = $("drawer-body");
  const holding = subagents.filter((agent) => agent.status === "PENDING_APPROVAL" || agent.status === "ACTIVE" || (!agent.status && agent.active));
  const byAgent = holding.filter((agent) => agent.created_by === "agent").length;
  const max = catalog ? catalog.max_per_flow : 3, maxAgent = catalog ? catalog.max_agent_per_flow : 1;
  const caps = '<div class="scaps">' + holding.length + " of " + max + " places taken · " + byAgent + " of " + maxAgent + " created by an agent. Changes made on DataSea or the CLI wait here for your approval; Crane checks them against the flow's bounds again.</div>";
  if (!subagents.length) { body.innerHTML = caps + '<div class="tempty">No subagents on this flow. Create one with \\'crane flow subagent add ' + esc(picked) + ' --type monitor|incident\\', on DataSea, or let an agent ask for one.</div>'; return; }
  const label = (id) => { const action = catalog && catalog.actions.find((entry) => entry.id === id); return action ? action.label : id; };
  body.innerHTML = caps + '<div class="slist">' + subagents.map((agent) => {
    const status = agent.status || (agent.active ? "ACTIVE" : "STOPPED");
    const tone = status === "ACTIVE" ? "ok" : status === "PENDING_APPROVAL" ? "warn" : status === "TERMINATED" ? "bad" : "";
    const config = agent.config || {};
    const pending = agent.pending;
    return '<article class="scard' + (pending ? " pending" : "") + '"><div class="kind">' + esc((agent.subtype || "monitor") + " subagent · " + (agent.created_by === "agent" ? "created by an agent" : "created by " + agent.created_by)) + "</div>" +
      "<b>" + esc(agent.purpose) + '</b><div class="pills"><span class="pill ' + tone + '">' + esc(status.replace("_", " ").toLowerCase()) + '</span><span class="pill">ceiling ' + esc(config.ceiling || (agent.inherited || {}).ceiling || "—") + "</span>" + (config.branch ? '<span class="pill">branch ' + esc(config.branch) + "</span>" : "") + ((agent.firing || []).map((code) => '<span class="pill bad">' + esc(code) + "</span>").join("")) + "</div>" +
      '<div class="muted">' + esc((config.actions || []).map(label).join(" · ") || "no actions yet") + "</div>" +
      (agent.ended ? '<div class="muted">Ended: ' + esc(agent.ended) + "</div>" : "") +
      (pending ? "<div>Waiting for approval: <b>" + esc(pending.kind) + "</b> by " + esc(pending.requested_by) + "</div><pre>" + esc(JSON.stringify(Object.assign({}, pending.purpose ? { purpose: pending.purpose } : {}, pending.checks ? { checks: pending.checks } : {}, { config: pending.config }), null, 2)) + "</pre>" : "") +
      '<div class="row">' + (pending ? '<button class="btn" data-decide="reject" data-id="' + esc(agent.watcher_id) + '">Reject</button><button class="btn primary" data-decide="approve" data-id="' + esc(agent.watcher_id) + '">Approve</button>' : "") +
      (status === "ACTIVE" || status === "PENDING_APPROVAL" ? '<button class="btn" data-decide="stop" data-id="' + esc(agent.watcher_id) + '">Stop</button>' : "") + "</div></article>";
  }).join("") + "</div>";
  for (const button of body.querySelectorAll("[data-decide]")) button.addEventListener("click", () => {
    const decision = button.dataset.decide, id = button.dataset.id;
    const send = () => { for (const other of body.querySelectorAll("[data-decide]")) other.disabled = true; vscode.postMessage({ type: "decide", flow: picked, id, decision }); };
    if (decision === "stop") openModal("Stop " + id + "?", "It stops acting on " + picked + " and gives its place back. Stopping cannot be undone; create a new one later if needed.", "Stop it", send); else send();
  });
}
// -------- the modal
function openModal(title, text, confirmLabel, onConfirm, error) {
  const modal = $("modal");
  modal.innerHTML = '<div class="box"><h2 id="modal-title">' + esc(title) + "</h2>" + (text ? "<p>" + esc(text) + "</p>" : "") + (error ? '<div class="err">' + esc(error) + "</div>" : "") +
    '<div class="row"><button class="btn" id="modal-cancel">' + (confirmLabel ? "Cancel" : "Close") + "</button>" + (confirmLabel ? '<button class="btn primary" id="modal-ok">' + esc(confirmLabel) + "</button>" : "") + "</div></div>";
  modal.hidden = false;
  const close = () => { modal.hidden = true; modal.innerHTML = ""; };
  $("modal-cancel").addEventListener("click", close);
  if (confirmLabel) $("modal-ok").addEventListener("click", () => { close(); onConfirm(); });
  modal.onclick = (event) => { if (event.target === modal) close(); };
  modal.onkeydown = (event) => { if (event.key === "Escape") close(); };
  (confirmLabel ? $("modal-ok") : $("modal-cancel")).focus();
}
$("drawer-close").addEventListener("click", closeDrawer);
$("drawer-open").addEventListener("click", () => { const node = picked && nodeById("flow:" + picked); if (node && node.open) vscode.postMessage({ type: "open", id: node.id }); else say("bad", "No source location is known for " + picked + ".", 4000); });
for (const button of document.querySelectorAll(".tab")) button.addEventListener("click", () => setTab(button.dataset.tab));
canvas.addEventListener("dblclick", (event) => { const node = nodeOf(event.target); if (node && node.open) vscode.postMessage({ type: "open", id: node.id }); });

window.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "flowData" && message.flow === picked) {
    tests = message.scenarios || []; subagents = message.subagents || []; catalog = message.catalog || null;
    if (message.testsError) say("bad", message.testsError, 8000);
    if (message.agentsError) say("bad", message.agentsError, 8000);
    drawDrawer();
    return;
  }
  if (message.type === "scenarios" && message.flow === picked) {
    if (message.error) say("bad", message.error, 9000);
    else { tests = message.scenarios || []; dirty = true; say("ok", tests.length + " scenario(s) written from the flow's knowledge. Review and accept them, then save.", 5000); }
    drawDrawer();
    return;
  }
  if (message.type === "decided" && message.flow === picked) {
    subagents = message.subagents || subagents; catalog = message.catalog || catalog;
    const result = message.result;
    if (result.ok) say("ok", result.message, 5000);
    else openModal(message.decision === "approve" ? "The subagent was not approved" : "That did not work", message.decision === "approve" ? "A subagent always stays within its flow. To give it more, first raise the flow's ceiling (or add the zone to the flow) in .crane/governance.yaml, then configure the subagent again." : "", null, null, result.message + (result.hint ? NL + result.hint : ""));
    drawDrawer();
    return;
  }
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
    if (message.op === "save" && result.ok && message.from === picked) { dirty = false; vscode.postMessage({ type: "flow", flow: picked }); }
    if (message.op === "remove" && result.ok && message.from === picked) { dirty = false; closeDrawer(); }
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
