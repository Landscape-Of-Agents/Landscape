// The Foxx sidebar (activity bar → Foxx → Overview): the repository's state at a glance. It shows
// Crane's status (with how to fix an unusable Crane), health counts, one-click actions, every
// selection with where it is and whether it passes (searchable, filterable, click to reveal, open in
// the Foxx panel), zones and flows, the most serious findings, and this window's activity. It holds
// no state: the extension sends a snapshot whenever something changes.

import * as vscode from "vscode";

/** One selection as the sidebar lists it. */
export interface SidebarSelection {
  id: string;
  name: string | null;
  operation: "preserve" | "target";
  policy: string;
  location: string | null;
  outcome: string;
}

/** What the sidebar shows. */
export interface SidebarState {
  ready: boolean;
  repository: string;
  crane?: { state: "ok" | "missing" | "outdated"; version?: string; message?: string };
  generation?: number;
  valid?: boolean;
  policies: number;
  selections: SidebarSelection[];
  entities: { kind: "zone" | "flow"; id: string; status: string }[];
  findings: { severity: string; code: string; message: string }[];
  activity: { at: number; label: string; ok: boolean; message: string }[];
}

/** The extension side of the sidebar. */
export interface SidebarHost {
  state(): SidebarState;
  command(id: string, argument?: string): Promise<void>;
}

/** The Overview view of the Foxx activity bar container. */
export class FoxxSidebar implements vscode.WebviewViewProvider {
  static readonly id = "foxx.sidebar";
  private static current: FoxxSidebar | undefined;
  private view: vscode.WebviewView | undefined;

  /**
   * Create the provider.
   * @param extensionUri the extension's folder
   * @param host extension side
   */
  constructor(private readonly extensionUri: vscode.Uri, private readonly host: SidebarHost) {
    FoxxSidebar.current = this;
  }

  /** Send a fresh snapshot to the sidebar, when it is visible. */
  static update(): void {
    FoxxSidebar.current?.refresh();
  }

  /**
   * Fill the view when VS Code shows it.
   * @param view the webview view
   */
  resolveWebviewView(view: vscode.WebviewView): void {
    this.view = view;
    view.webview.options = { enableScripts: true, localResourceRoots: [vscode.Uri.joinPath(this.extensionUri, "images")] };
    view.webview.html = html(view.webview.cspSource, view.webview.asWebviewUri(vscode.Uri.joinPath(this.extensionUri, "images", "icon.png")).toString());
    view.webview.onDidReceiveMessage(async (message: { type: string; id?: string; argument?: string }) => {
      if (message.type === "ready") this.refresh();
      else if (message.type === "command" && message.id) {
        await this.host.command(message.id, message.argument);
        this.refresh();
      }
    });
    view.onDidChangeVisibility(() => { if (view.visible) this.refresh(); });
  }

  /** Post the current state. */
  private refresh(): void {
    if (this.view?.visible) void this.view.webview.postMessage({ type: "state", state: this.host.state() });
  }
}

/**
 * Build the sidebar's page: a strict content security policy (only this page's script, inline
 * styles, and the extension's images).
 * @param cspSource the webview's source for images and styles
 * @param logo the Foxx logo URI
 * @returns HTML
 */
function html(cspSource: string, logo: string): string {
  const nonce = Array.from({ length: 32 }, () => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 62)]).join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${cspSource}; style-src ${cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
:root { --fox: #ff7a1a; --fox-2: #ffb347; --fox-soft: color-mix(in srgb, var(--fox) 14%, transparent); --line: var(--vscode-panel-border, color-mix(in srgb, var(--vscode-foreground) 14%, transparent)); --muted: var(--vscode-descriptionForeground); --raised: color-mix(in srgb, var(--vscode-foreground) 4%, var(--vscode-sideBar-background)); --ok: var(--vscode-testing-iconPassed, #3fb950); --bad: var(--vscode-errorForeground, #f14c4c); --warn: var(--vscode-editorWarning-foreground, #cca700); --mono: var(--vscode-editor-font-family, monospace); }
* { box-sizing: border-box; }
body { margin: 0; padding: 10px 12px 24px; color: var(--vscode-foreground); background: var(--vscode-sideBar-background); font: var(--vscode-font-size) / 1.45 var(--vscode-font-family); }
button { font: inherit; color: inherit; }
:focus-visible { outline: 1px solid var(--fox); outline-offset: 1px; }
.top { display: flex; align-items: center; gap: 8px; margin-bottom: 10px; }
.top img { width: 22px; height: 22px; }
.top b { font-size: 13px; }
.top .repo { color: var(--muted); font-size: 11px; display: block; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.pill { margin-left: auto; display: inline-flex; align-items: center; gap: 5px; padding: 1px 8px; border-radius: 999px; border: 1px solid var(--line); font-size: 11px; white-space: nowrap; }
.pill i { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.pill.ok i { background: var(--ok); } .pill.bad { color: var(--bad); } .pill.bad i { background: var(--bad); }
.banner { border: 1px solid color-mix(in srgb, var(--bad) 45%, transparent); background: color-mix(in srgb, var(--bad) 9%, transparent); border-radius: 8px; padding: 8px 10px; margin-bottom: 10px; font-size: 12px; }
.banner.info { border-color: color-mix(in srgb, var(--fox) 45%, transparent); background: var(--fox-soft); }
.banner b { display: block; margin-bottom: 2px; }
.banner .row { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }
h3 { margin: 14px 0 6px; font-size: 11px; letter-spacing: .8px; text-transform: uppercase; color: var(--muted); display: flex; align-items: center; gap: 6px; }
h3 .count { margin-left: auto; font-weight: 400; text-transform: none; letter-spacing: 0; }
.stats { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 6px; }
.stat { border: 1px solid var(--line); border-radius: 8px; padding: 6px 8px; background: var(--raised); cursor: pointer; text-align: left; }
.stat:hover { border-color: var(--fox); }
.stat .n { font-size: 17px; font-weight: 700; font-variant-numeric: tabular-nums; }
.stat .l { font-size: 10.5px; color: var(--muted); text-transform: uppercase; letter-spacing: .4px; }
.stat.ok .n { color: var(--ok); } .stat.bad .n { color: var(--bad); } .stat.warn .n { color: var(--warn); }
.actions { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 6px; }
.btn { border: 1px solid var(--line); background: var(--raised); border-radius: 7px; padding: 6px 8px; cursor: pointer; display: flex; align-items: center; gap: 6px; text-align: left; font-size: 12px; }
.btn:hover { border-color: var(--fox); }
.btn.primary { border-color: transparent; color: #1a0d05; font-weight: 700; background: linear-gradient(90deg, var(--fox), var(--fox-2)); justify-content: center; }
.btn .g { color: var(--fox); width: 14px; text-align: center; }
.btn.primary .g { color: inherit; }
.search { width: 100%; padding: 5px 8px; border-radius: 6px; border: 1px solid var(--vscode-input-border, var(--line)); background: var(--vscode-input-background); color: var(--vscode-input-foreground); margin-bottom: 6px; }
.chips { display: flex; gap: 4px; flex-wrap: wrap; margin-bottom: 6px; }
.chip { border: 1px solid var(--line); background: none; border-radius: 999px; padding: 0 8px; font-size: 11px; cursor: pointer; }
.chip.on { border-color: var(--fox); color: var(--fox); background: var(--fox-soft); }
.list { display: flex; flex-direction: column; gap: 3px; }
.item { display: grid; grid-template-columns: 16px minmax(0, 1fr) auto; gap: 6px; align-items: center; padding: 5px 6px; border-radius: 6px; border: 1px solid transparent; cursor: pointer; text-align: left; background: none; width: 100%; }
.item:hover { background: var(--vscode-list-hoverBackground); border-color: var(--line); }
.item .t { font-family: var(--mono); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.item .s { display: block; color: var(--muted); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: var(--vscode-font-family); }
.badge { font-size: 10px; font-weight: 700; padding: 0 6px; border-radius: 4px; border: 1px solid var(--line); white-space: nowrap; }
.badge.PASS, .badge.active, .badge.SATISFIED { color: var(--ok); border-color: color-mix(in srgb, var(--ok) 50%, transparent); }
.badge.FAIL, .badge.UNRESOLVED, .badge.HIGH, .badge.CRITICAL { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 50%, transparent); }
.badge.PENDING, .badge.proposed, .badge.MEDIUM { color: var(--warn); border-color: color-mix(in srgb, var(--warn) 50%, transparent); }
.op { width: 16px; text-align: center; color: var(--fox); }
.empty { color: var(--muted); font-size: 12px; padding: 6px 2px; }
.log { list-style: none; padding: 0; margin: 0; font-size: 11.5px; }
.log li { display: grid; grid-template-columns: 54px 10px minmax(0, 1fr); gap: 6px; padding: 2px 0; }
.log .ok { color: var(--ok); } .log .bad { color: var(--bad); }
.finding { font-size: 12px; padding: 4px 0; border-bottom: 1px solid var(--line); display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 6px; }
.more { background: none; border: 0; color: var(--fox); cursor: pointer; padding: 4px 0; font-size: 12px; }
</style>
</head>
<body>
<div class="top"><img src="${logo}" alt=""><div style="min-width:0"><b>Foxx</b><span class="repo" id="repo"></span></div><span class="pill" id="pill"><i></i>…</span></div>
<div id="banner"></div>
<div id="body"></div>
<script nonce="${nonce}">
"use strict";
const vscode = acquireVsCodeApi();
let state = null;
let view = { q: "", filter: "all", showAll: false };
const esc = (text) => String(text == null ? "" : text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const post = (id, argument) => vscode.postMessage({ type: "command", id, argument });
function draw() {
  if (!state) return;
  const s = state;
  document.getElementById("repo").textContent = s.repository ? s.repository + (s.generation != null ? " · gen " + s.generation : "") : "No repository";
  const crane = s.crane;
  const pill = document.getElementById("pill");
  pill.className = "pill " + (crane && crane.state !== "ok" ? "bad" : crane ? "ok" : "");
  pill.innerHTML = "<i></i>" + (crane ? (crane.state === "ok" ? "Crane " + esc(crane.version) : crane.state === "missing" ? "Crane not found" : "Crane " + esc(crane.version || "?") + " · update") : "Checking Crane…");
  let banner = "";
  if (crane && crane.state !== "ok") banner = '<div class="banner" role="alert"><b>' + (crane.state === "missing" ? "Crane was not found" : "This Crane is too old") + "</b>" + esc(crane.message) + '<div class="row"><button class="btn primary" data-id="crane:download">Download Crane</button><button class="btn" data-id="crane:path">Set path</button><button class="btn" data-id="crane:recheck">Check again</button></div></div>';
  else if (!s.ready) banner = '<div class="banner info"><b>Crane is not set up here</b>Protect code, add zones and flows, and check every agent tool call.<div class="row"><button class="btn primary" data-id="foxx.setup">Set up Crane</button></div></div>';
  document.getElementById("banner").innerHTML = banner;
  if (!s.ready) { document.getElementById("body").innerHTML = ""; return; }
  const sel = s.selections;
  const passing = sel.filter((x) => x.outcome === "PASS").length, failing = sel.filter((x) => x.outcome === "FAIL").length, unresolved = sel.filter((x) => x.outcome === "UNRESOLVED").length;
  const zones = s.entities.filter((e) => e.kind === "zone").length, flows = s.entities.filter((e) => e.kind === "flow").length;
  const stat = (n, label, tone, filter) => '<button class="stat ' + tone + '" data-filter="' + filter + '" title="Show ' + esc(label.toLowerCase()) + '"><div class="n">' + n + '</div><div class="l">' + esc(label) + "</div></button>";
  const action = (id, glyph, label, title) => '<button class="btn" data-id="' + id + '" title="' + esc(title) + '"><span class="g">' + glyph + "</span>" + esc(label) + "</button>";
  const q = view.q.toLowerCase();
  const shown = sel.filter((x) => (view.filter === "all" || x.outcome === view.filter || (view.filter === "preserve" || view.filter === "target") && x.operation === view.filter) && (!q || (x.id + " " + (x.name || "") + " " + x.policy + " " + (x.location || "")).toLowerCase().includes(q)));
  const visible = view.showAll ? shown : shown.slice(0, 12);
  const chips = [["all", "All"], ["PASS", "Passing"], ["FAIL", "Failing"], ["UNRESOLVED", "Unresolved"], ["preserve", "Preserved"], ["target", "Targeted"]].map(([key, label]) => '<button class="chip' + (view.filter === key ? " on" : "") + '" data-filter="' + key + '">' + label + "</button>").join("");
  const items = visible.map((x) => '<button class="item" data-reveal="' + esc(x.id) + '" title="Reveal in the editor · ' + esc(x.operation + " " + x.id + " · policy " + x.policy) + '"><span class="op">' + (x.operation === "preserve" ? "⛨" : "◎") + '</span><span><span class="t">' + esc(x.name || x.id) + '</span><span class="s">' + esc((x.location || "not found") + " · " + x.policy) + '</span></span><span class="badge ' + esc(x.outcome) + '">' + esc(x.outcome) + "</span></button>").join("");
  const entities = s.entities.map((e) => '<div class="item" style="cursor:default"><span class="op">' + (e.kind === "zone" ? "▦" : "⇢") + '</span><span class="t">' + esc(e.id) + '<span class="s">' + e.kind + '</span></span><span class="badge ' + esc(e.status) + '">' + esc(e.status) + "</span></div>").join("");
  const findings = s.findings.map((f) => '<div class="finding"><span class="badge ' + esc(f.severity) + '">' + esc(f.severity) + "</span><span>" + esc(f.message) + "</span></div>").join("");
  const log = s.activity.map((a) => '<li><span>' + new Date(a.at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) + '</span><span class="' + (a.ok ? "ok" : "bad") + '">●</span><span><b>' + esc(a.label) + "</b> " + esc(a.message) + "</span></li>").join("");
  document.getElementById("body").innerHTML =
    '<div class="stats">' + stat(passing, "Passing", passing ? "ok" : "", "PASS") + stat(failing, "Failing", failing ? "bad" : "", "FAIL") + stat(unresolved, "Unresolved", unresolved ? "warn" : "", "UNRESOLVED") + stat(s.policies, "Policies", "", "all") + stat(zones, "Zones", "", "all") + stat(flows, "Flows", "", "all") + "</div>" +
    "<h3>Actions</h3>" + '<div class="actions">' + action("foxx.menu", "✦", "Open panel", "Act on the selected code (Ctrl+Alt+F)") + action("foxx.test", "▶", "Test policies", "Run every preserve and target rule") + action("foxx.validate", "✓", "Validate", "Check integrity, syntax, and markers") + action("foxx.createCheckpoint", "⚑", "Checkpoint", "Record the current commit as trusted") + action("foxx.installHooks", "⚓", "Agent hooks", "Install Crane's hooks for Claude Code or Codex") + action("foxx.undoGovernanceChange", "↶", "Undo change", "Undo the last governance change made here") + "</div>" +
    '<h3>Selections <span class="count">' + shown.length + " of " + sel.length + "</span></h3>" +
    '<input class="search" id="search" type="search" placeholder="Search selections…" value="' + esc(view.q) + '" aria-label="Search selections">' + '<div class="chips">' + chips + "</div>" +
    '<div class="list">' + (items || '<div class="empty">' + (sel.length ? "No selection matches." : "No selections yet. Select code and press Ctrl+Alt+F to preserve or target it.") + "</div>") + "</div>" +
    (shown.length > visible.length ? '<button class="more" id="more">Show all ' + shown.length + "</button>" : "") +
    '<h3>Zones and flows <span class="count">' + s.entities.length + "</span></h3>" + '<div class="list">' + (entities || '<div class="empty">None yet. Select code and choose "Make these lines a zone".</div>') + "</div>" +
    (findings ? '<h3>Findings <span class="count">' + s.findings.length + '</span></h3><div>' + findings + "</div>" : "") +
    '<h3>Activity</h3><ul class="log">' + (log || '<li class="empty" style="display:block">Nothing yet in this window.</li>') + "</ul>";
  const search = document.getElementById("search");
  if (search) search.addEventListener("input", () => { view.q = search.value; const at = search.selectionStart; draw(); const again = document.getElementById("search"); again.focus(); again.setSelectionRange(at, at); });
  const more = document.getElementById("more");
  if (more) more.addEventListener("click", () => { view.showAll = true; draw(); });
}
document.addEventListener("click", (event) => {
  const command = event.target.closest("[data-id]");
  if (command) { post(command.dataset.id); return; }
  const filter = event.target.closest("[data-filter]");
  if (filter) { view.filter = filter.dataset.filter; view.showAll = false; draw(); return; }
  const reveal = event.target.closest("[data-reveal]");
  if (reveal) post("foxx.revealSelection", reveal.dataset.reveal);
});
window.addEventListener("message", (event) => { if (event.data.type === "state") { state = event.data.state; draw(); } });
vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
}
