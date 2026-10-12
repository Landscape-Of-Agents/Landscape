// The Foxx panel: one place to act on the selected code. A fixed-size search box suggests actions
// as you type (prefix search); choosing one shows its form (policies, zones and flows with their
// autonomy ceiling and criticality, and fixed-size, scrollable context boxes), and the extension
// runs it through the Crane CLI. The panel holds no state of its own: the extension sends the
// selection, policies, zones, and flows, and the panel sends back what the person chose.

import * as vscode from "vscode";

import { ACTIONS, search, SETUP, words } from "./core/actions";
import { CEILINGS, CRITICALITY } from "./core/governance";

/** The code the panel acts on. */
export interface SelectionInfo {
  uri: string;
  file: string;
  start: number;
  end: number;
  empty: boolean;
  symbol?: string;
  preview: string;
}

/** What the panel shows: the selection and the repository's governance. */
export interface PanelState {
  ready: boolean;
  selection?: SelectionInfo;
  policies: string[];
  entities: { kind: "zone" | "flow"; id: string; status: string }[];
  selections: { id: string; label: string; policy: string }[];
}

/** The extension side the panel talks to. */
export interface PanelHost {
  state(): PanelState;
  run(action: string, values: Record<string, string | boolean>, selection?: SelectionInfo): Promise<{ ok: boolean; message: string }>;
}

/** The Foxx panel (one per window). */
export class FoxxPanel {
  private static current: FoxxPanel | undefined;
  private selection: SelectionInfo | undefined;

  /**
   * Open the panel beside the editor, or bring it forward, focused on a selection.
   * @param extensionUri the extension's folder
   * @param host extension side
   * @param selection selected code, when any
   */
  static show(extensionUri: vscode.Uri, host: PanelHost, selection: SelectionInfo | undefined): void {
    if (FoxxPanel.current) {
      FoxxPanel.current.selection = selection;
      FoxxPanel.current.panel.reveal(vscode.ViewColumn.Beside);
      FoxxPanel.current.refresh(true);
      return;
    }
    const panel = vscode.window.createWebviewPanel("foxx.panel", "Foxx", { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false }, {
      enableScripts: true,
      retainContextWhenHidden: true,
      localResourceRoots: [vscode.Uri.joinPath(extensionUri, "images")],
    });
    panel.iconPath = vscode.Uri.joinPath(extensionUri, "images", "icon.png");
    FoxxPanel.current = new FoxxPanel(panel, extensionUri, host, selection);
  }

  /**
   * Tell an open panel about a new selection.
   * @param selection selected code
   */
  static select(selection: SelectionInfo | undefined): void {
    if (!FoxxPanel.current) return;
    FoxxPanel.current.selection = selection;
    FoxxPanel.current.refresh(false);
  }

  /**
   * Create the panel's controller.
   * @param panel webview panel
   * @param extensionUri the extension's folder
   * @param host extension side
   * @param selection selected code
   */
  private constructor(private readonly panel: vscode.WebviewPanel, extensionUri: vscode.Uri, private readonly host: PanelHost, selection: SelectionInfo | undefined) {
    this.selection = selection;
    const logo = panel.webview.asWebviewUri(vscode.Uri.joinPath(extensionUri, "images", "icon.png"));
    panel.webview.html = html(panel.webview.cspSource, logo.toString());
    panel.onDidDispose(() => (FoxxPanel.current = undefined));
    panel.webview.onDidReceiveMessage(async (message: { type: string; action?: string; values?: Record<string, string | boolean> }) => {
      if (message.type === "ready") this.refresh(true);
      if (message.type === "run" && message.action) {
        const result = await this.host.run(message.action, message.values ?? {}, this.selection);
        void this.panel.webview.postMessage({ type: "result", ...result });
        this.refresh(false);
      }
    });
  }

  /**
   * Send the current state to the panel.
   * @param focus move the cursor to the search box
   */
  private refresh(focus: boolean): void {
    void this.panel.webview.postMessage({ type: "state", state: { ...this.host.state(), selection: this.selection }, focus });
  }
}

/**
 * Build the panel's page: a strict content security policy (only this page's script, styles, and
 * the extension's images), the search box, the suggestion list, and the form area. The search
 * function is the extension's own (core/actions), so the panel and the tests agree.
 * @param cspSource the webview's source for images and styles
 * @param logo the Foxx logo URI
 * @returns HTML
 */
function html(cspSource: string, logo: string): string {
  const nonce = Array.from({ length: 32 }, () => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 62)]).join("");
  const data = JSON.stringify({ actions: ACTIONS, setup: SETUP, criticality: CRITICALITY, ceilings: CEILINGS }).replace(/</g, "\\u003c");
  // The panel runs the extension's own search code, so typing finds the same actions everywhere
  const searchSource = `const words = ${words.toString()};\nconst searchActions = ${search.toString()};\nconst searchWords = words;`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${cspSource}; style-src ${cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Foxx</title>
<style>
:root { --fox: #ff7a1a; --fox-2: #ffb347; --width: 720px; }
* { box-sizing: border-box; }
body { margin: 0; padding: 16px 20px 32px; color: var(--vscode-foreground); background: var(--vscode-editor-background); font: var(--vscode-font-size) / 1.5 var(--vscode-font-family); }
header { display: flex; align-items: center; gap: 10px; max-width: var(--width); }
header img { width: 32px; height: 32px; filter: drop-shadow(0 0 6px rgba(255, 122, 26, .45)); }
header h1 { font-size: 16px; margin: 0; background: linear-gradient(90deg, var(--fox), var(--fox-2)); -webkit-background-clip: text; background-clip: text; color: transparent; }
.where { max-width: var(--width); margin: 8px 0 12px; padding: 8px 10px; border-left: 3px solid var(--fox); background: var(--vscode-textBlockQuote-background); font-size: 12px; }
.where code { font-family: var(--vscode-editor-font-family); }
.where pre { margin: 6px 0 0; max-height: 72px; overflow-y: auto; white-space: pre-wrap; font-family: var(--vscode-editor-font-family); font-size: 12px; color: var(--vscode-descriptionForeground); }
label.title { display: block; font-weight: 600; margin: 12px 0 4px; }
.hint { color: var(--vscode-descriptionForeground); font-size: 12px; margin: 2px 0 0; }
/* Fixed-size boxes that scroll vertically instead of growing */
textarea.search, textarea.context { display: block; width: 100%; max-width: var(--width); resize: none; overflow-y: auto; padding: 8px 10px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-panel-border)); border-radius: 6px; font: inherit; }
textarea.search { height: 64px; }
textarea.context { height: 160px; }
textarea:focus, input:focus, select:focus { outline: 1px solid var(--fox); border-color: var(--fox); }
ul.suggestions { list-style: none; margin: 6px 0 0; padding: 4px; max-width: var(--width); height: 236px; overflow-y: auto; border: 1px solid var(--vscode-panel-border); border-radius: 6px; }
ul.suggestions li { padding: 6px 8px; border-radius: 4px; cursor: pointer; }
ul.suggestions li .detail { display: block; font-size: 12px; color: var(--vscode-descriptionForeground); }
ul.suggestions li.active { background: var(--vscode-list-activeSelectionBackground); color: var(--vscode-list-activeSelectionForeground); }
ul.suggestions li.active .detail { color: inherit; opacity: .85; }
ul.suggestions li.disabled { opacity: .5; cursor: default; }
ul.suggestions mark { background: none; color: var(--fox); font-weight: 700; }
ul.suggestions li.active mark { color: inherit; text-decoration: underline; }
form { max-width: var(--width); margin-top: 14px; padding: 12px 14px; border: 1px solid var(--vscode-panel-border); border-radius: 8px; }
form h2 { margin: 0 0 4px; font-size: 14px; }
input[type=text], select { width: 100%; padding: 6px 8px; color: var(--vscode-input-foreground); background: var(--vscode-input-background); border: 1px solid var(--vscode-input-border, var(--vscode-panel-border)); border-radius: 6px; font: inherit; }
/* Criticality takes a compact column so the autonomy ceiling beside it has room; long option text
   is cut with an ellipsis instead of widening its column */
.row { display: grid; grid-template-columns: minmax(0, 11rem) minmax(0, 1fr); gap: 12px; }
.row > div { min-width: 0; }
select { text-overflow: ellipsis; }
.choice { display: flex; gap: 14px; flex-wrap: wrap; }
.buttons { display: flex; gap: 8px; margin-top: 14px; }
button { padding: 6px 14px; border-radius: 6px; border: 1px solid transparent; font: inherit; cursor: pointer; color: var(--vscode-button-secondaryForeground); background: var(--vscode-button-secondaryBackground); }
button.primary { color: #1a0d05; font-weight: 700; background: linear-gradient(90deg, var(--fox), var(--fox-2)); }
.result { max-width: var(--width); margin-top: 12px; padding: 8px 10px; border-radius: 6px; white-space: pre-wrap; }
.result.ok { border: 1px solid var(--vscode-testing-iconPassed, #3fb950); }
.result.error { border: 1px solid var(--vscode-errorForeground); color: var(--vscode-errorForeground); }
@media (max-width: 480px) { .row { grid-template-columns: 1fr; } }
</style>
</head>
<body>
<header><img src="${logo}" alt=""><h1>Foxx</h1></header>
<div class="where" id="where">No code selected.</div>
<label class="title" for="search">What do you want to do?</label>
<textarea id="search" class="search" placeholder="Type to search, e.g. preserve, zone, context, autonomy, policy…" aria-describedby="search-hint" spellcheck="false"></textarea>
<p class="hint" id="search-hint">↑ ↓ to choose · Enter to open · Esc to clear</p>
<ul id="suggestions" class="suggestions" role="listbox" aria-label="Matching actions"></ul>
<div id="form-area"></div>
<div id="result" role="status"></div>
<script nonce="${nonce}">
"use strict";
const vscode = acquireVsCodeApi();
const DATA = ${data};
${searchSource}
let state = { ready: false, policies: [], entities: [], selections: [] };
let shown = [];
let active = 0;
const $ = (id) => document.getElementById(id);
const escape = (text) => String(text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const selected = () => state.selection && !state.selection.empty;

function highlight(text, query) {
  const typed = searchWords(query);
  return escape(text).replace(/[A-Za-z0-9]+/g, (word) => {
    const hit = typed.find((t) => word.toLowerCase().startsWith(t));
    return hit ? "<mark>" + word.slice(0, hit.length) + "</mark>" + word.slice(hit.length) : word;
  });
}

function renderWhere() {
  const s = state.selection;
  if (!state.ready) { $("where").textContent = "Crane is not set up in this repository yet."; return; }
  if (!s) { $("where").textContent = "No file open. Select code in an editor to act on it."; return; }
  const span = s.empty ? "line " + s.start : "lines " + s.start + "–" + s.end;
  $("where").innerHTML = "<code>" + escape(s.file) + "</code> · " + span + (s.symbol ? " · in <code>" + escape(s.symbol) + "</code>" : "") + (s.empty ? "" : "<pre>" + escape(s.preview) + "</pre>");
}

function renderSuggestions() {
  const query = $("search").value;
  const pool = state.ready ? DATA.actions : [DATA.setup];
  shown = searchActions(query, pool);
  if (active >= shown.length) active = 0;
  $("suggestions").innerHTML = shown.length
    ? shown.map((action, index) => {
        const off = action.needsSelection && !selected();
        return '<li role="option" data-index="' + index + '" class="' + (index === active ? "active " : "") + (off ? "disabled" : "") + '" aria-selected="' + (index === active) + '">' + highlight(action.label, query) +
          '<span class="detail">' + escape(off ? "Select code in the editor first" : action.detail) + "</span></li>";
      }).join("")
    : '<li class="disabled">No action starts with that. Try "zone", "context", or "preserve".</li>';
  const current = $("suggestions").querySelector("li.active");
  if (current) current.scrollIntoView({ block: "nearest" });
}

function option(value, label, chosen) { return '<option value="' + escape(value) + '"' + (value === chosen ? " selected" : "") + ">" + escape(label) + "</option>"; }
function field(id, label, inner, hint) { return '<label class="title" for="' + id + '">' + escape(label) + "</label>" + inner + (hint ? '<p class="hint">' + escape(hint) + "</p>" : ""); }
function text(id, label, value, hint) { return field(id, label, '<input type="text" id="' + id + '" value="' + escape(value || "") + '">', hint); }
function select(id, label, options, hint) { return field(id, label, '<select id="' + id + '">' + options.join("") + "</select>", hint); }
function policySelect() { return select("policy", "Policy", [option("", "Default policy")].concat(state.policies.map((p) => option(p, p)))); }
function context(label, hint) { return field("context", label, '<textarea id="context" class="context" placeholder="One guidance line per line, e.g. Charges must be idempotent; never retry a capture." spellcheck="true"></textarea>', hint); }
const CEILING_HELP = { observe: "the agent may only read this code", assisted: "every change needs your approval", delegated: "changes are allowed within the policies", autonomous: "no limit beyond the policies" };
function autonomy(chosen) { return select("ceiling", "Autonomy ceiling", DATA.ceilings.map((c) => option(c, c + " — " + CEILING_HELP[c], chosen)), "The strictest ceiling wins where zones and flows overlap."); }
function criticality(chosen) { return select("criticality", "Criticality", DATA.criticality.map((c) => option(c, c, chosen))); }
function coverChoice() {
  const s = state.selection;
  const fn = s && s.symbol ? '<label><input type="radio" name="cover" value="symbol" checked> the function <code>' + escape(s.symbol) + "</code></label>" : "";
  return field("cover", "Covers", '<div class="choice">' + fn + '<label><input type="radio" name="cover" value="file"' + (fn ? "" : " checked") + "> the whole file</label></div>", fn ? "" : "No enclosing function was found, so the zone covers the file.");
}
function slug(text) { return (text || "").replace(/[^A-Za-z0-9]+/g, "-").replace(/^-+|-+$/g, "").toLowerCase().replace(/^([^a-z])/, "x-$1").slice(0, 40); }

function formFor(action) {
  const s = state.selection;
  const base = s ? slug((s.symbol || s.file.split("/").pop()).split(/[.:]/).pop()) : "";
  switch (action.form) {
    case "preserve": return policySelect() + text("name", "Name (optional)", "");
    case "target": return policySelect() + select("changeType", "Required kind of change", [option("", "Any real change"), option("logical_bn", "Business logic"), option("logical_cn", "Loops or branches"), option("logical_sn", "Structure"), option("semantic", "Names or comments")]) + text("name", "Name (optional)", "");
    case "zone": return text("id", "Zone name", base ? base + "-zone" : "", "Letters, digits, '-' or '_'.") + text("description", "Description", "") + coverChoice() + '<div class="row"><div>' + criticality("sensitive") + "</div><div>" + autonomy("assisted") + "</div></div>" + context("Context for the agent", "Given to the agent whenever a tool call touches this zone.") + enforce();
    case "flow": return text("id", "Flow name", base ? base + "-flow" : "", "Letters, digits, '-' or '_'.") + text("entry", "Entry point", s && s.symbol ? s.file + ":" + s.symbol : "", "path:function — the flow covers it and everything it calls.") + text("description", "Description", "") + '<div class="row"><div>' + criticality("sensitive") + "</div><div>" + autonomy("assisted") + "</div></div>" + context("Context for the agent", "Given to the agent whenever a tool call touches this flow.") + enforce();
    case "context": {
      const options = state.entities.map((e) => option(e.kind + ":" + e.id, e.kind + " · " + e.id + (e.status === "active" ? "" : " (" + e.status + ")")));
      return (options.length ? select("target", "Zone or flow", options) : '<p class="hint">No zones or flows yet. Create one first ("Make these lines a zone").</p>') + context("Context", "Given to the agent whenever a tool call touches code the zone or flow covers. Added to any context it already has.");
    }
    case "zone_attach": {
      const zones = state.entities.filter((e) => e.kind === "zone").map((e) => option(e.id, e.id));
      return (zones.length ? select("zone", "Zone", zones) : '<p class="hint">No zones yet.</p>') + coverChoice();
    }
    case "policy_context": return policySelect() + '<p class="hint">Attaches <code>' + escape(s ? s.file : "the open file") + "</code> as guidance for the policy.</p>";
    case "policy": return text("name", "Policy name", "") + text("file", "File in .crane/policies (without .crane)", "", "Leave empty to use the policy name.");
    case "move": return select("selection", "Selection", state.selections.map((r) => option(r.id, r.id + " · " + r.label + " (in " + r.policy + ")"))) + select("policy", "Move to policy", state.policies.map((p) => option(p, p)));
    case "checkpoint": return text("name", "Checkpoint name", "checkpoint-" + new Date().toISOString().slice(0, 10), "Records the current commit as trusted.");
    case "hooks": return field("profile", "Agent", '<div class="choice"><label><input type="radio" name="profile" value="claude" checked> Claude Code</label><label><input type="radio" name="profile" value="codex"> Codex</label></div>');
    default: return "";
  }
}
function enforce() { return '<label class="title"><input type="checkbox" id="enforce" checked> Enforce now</label><p class="hint">Unchecked, it is created as a proposal to review first.</p>'; }

function choose(index) {
  const action = shown[index];
  if (!action || (action.needsSelection && !selected())) return;
  $("result").textContent = "";
  $("result").className = "";
  if (action.form === "none") { run(action.id, {}); return; }
  $("form-area").innerHTML = '<form id="form"><h2>' + escape(action.label) + '</h2><p class="hint">' + escape(action.detail) + "</p>" + formFor(action) +
    '<div class="buttons"><button type="submit" class="primary">' + escape(action.label) + '</button><button type="button" id="cancel">Cancel</button></div></form>';
  $("form").addEventListener("submit", (event) => { event.preventDefault(); run(action.id, values()); });
  $("cancel").addEventListener("click", () => { $("form-area").innerHTML = ""; $("search").focus(); });
  const first = $("form").querySelector("input[type=text], select, textarea");
  if (first) first.focus();
}

function values() {
  const out = {};
  for (const element of $("form").querySelectorAll("input, select, textarea")) {
    if (element.type === "radio") { if (element.checked) out[element.name] = element.value; }
    else if (element.type === "checkbox") out[element.id] = element.checked;
    else if (element.id) out[element.id] = element.value;
  }
  return out;
}

function run(action, payload) {
  $("result").className = "result";
  $("result").textContent = "Working…";
  vscode.postMessage({ type: "run", action, values: payload });
}

$("search").addEventListener("input", () => { active = 0; renderSuggestions(); });
$("search").addEventListener("keydown", (event) => {
  if (event.key === "ArrowDown") { event.preventDefault(); active = Math.min(active + 1, shown.length - 1); renderSuggestions(); }
  else if (event.key === "ArrowUp") { event.preventDefault(); active = Math.max(active - 1, 0); renderSuggestions(); }
  else if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); choose(active); }
  else if (event.key === "Escape") { $("search").value = ""; active = 0; renderSuggestions(); }
});
$("suggestions").addEventListener("click", (event) => {
  const item = event.target.closest("li[data-index]");
  if (item) { active = Number(item.dataset.index); renderSuggestions(); choose(active); }
});
window.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "state") {
    state = message.state;
    renderWhere();
    renderSuggestions();
    if (message.focus) $("search").focus();
  } else if (message.type === "result") {
    $("result").className = "result " + (message.ok ? "ok" : "error");
    $("result").textContent = message.message;
    if (message.ok) $("form-area").innerHTML = "";
  }
});
vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
}
