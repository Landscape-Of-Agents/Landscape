// The Foxx panel: an agent-style workspace for the selected code. It shows what it acts on (the
// selection, with line numbers), lets the person ask for an action (search, a quick chip, or the
// grouped action menu behind its toggle),
// configures it with an explicit plan of every step Foxx will take, then runs it with a live
// timeline of those steps and a result with follow-up buttons (undo, open the file, fix Crane).
// The panel holds no state of its own beyond this session's activity: the extension sends the
// selection, the repository's governance, and Crane's status, and runs what the person chose.

import * as vscode from "vscode";

import { ACTIONS, groupActions, search, SETUP, words } from "./core/actions";
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

/** What the panel shows: the selection, Crane, and the repository's governance. */
export interface PanelState {
  ready: boolean;
  repository?: string;
  crane?: { state: "ok" | "missing" | "outdated"; version?: string; message?: string };
  generation?: number;
  issues?: number;
  undo?: boolean;
  selection?: SelectionInfo;
  policies: string[];
  entities: { kind: "zone" | "flow"; id: string; status: string }[];
  selections: { id: string; label: string; policy: string }[];
}

/** One step of a running action. */
export interface StepUpdate {
  id: string;
  label: string;
  state: "running" | "done" | "failed";
  detail?: string;
  ms?: number;
}

/** Reports a step's progress to the panel. */
export type Progress = (step: StepUpdate) => void;

/** A follow-up button of a result: run another action, or a command of the extension. */
export interface ResultAction {
  id: string;
  label: string;
  kind: "run" | "command";
}

/** The outcome of an action. */
export interface RunResult {
  ok: boolean;
  message: string;
  hint?: string;
  actions?: ResultAction[];
}

/** The extension side the panel talks to. */
export interface PanelHost {
  state(): PanelState;
  run(action: string, values: Record<string, string | boolean>, selection: SelectionInfo | undefined, progress: Progress): Promise<RunResult>;
  command(id: string): Promise<void>;
}

/** The Foxx panel (one per window). */
export class FoxxPanel {
  private static current: FoxxPanel | undefined;
  private selection: SelectionInfo | undefined;
  private running = false;

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
   * Tell an open panel about a new selection (ignored while an action runs, so the result stays
   * about the code it acted on).
   * @param selection selected code
   */
  static select(selection: SelectionInfo | undefined): void {
    if (!FoxxPanel.current || FoxxPanel.current.running) return;
    FoxxPanel.current.selection = selection;
    FoxxPanel.current.refresh(false);
  }

  /**
   * Refresh an open panel's view of Crane and the repository.
   */
  static update(): void {
    FoxxPanel.current?.refresh(false);
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
    panel.webview.onDidReceiveMessage(async (message: { type: string; action?: string; id?: string; values?: Record<string, string | boolean> }) => {
      if (message.type === "ready") this.refresh(true);
      else if (message.type === "command" && message.id) {
        await this.host.command(message.id);
        this.refresh(false);
      } else if (message.type === "run" && message.action && !this.running) {
        this.running = true;
        const progress: Progress = (step) => void this.panel.webview.postMessage({ type: "progress", step });
        try {
          const result = await this.host.run(message.action, message.values ?? {}, this.selection, progress);
          void this.panel.webview.postMessage({ type: "result", result });
        } finally {
          this.running = false;
          this.refresh(false);
        }
      }
    });
  }

  /**
   * Send the current state to the panel.
   * @param focus move the cursor to the composer
   */
  private refresh(focus: boolean): void {
    void this.panel.webview.postMessage({ type: "state", state: { ...this.host.state(), selection: this.selection }, focus });
  }
}

/**
 * Build the panel's page: a strict content security policy (only this page's script, inline
 * styles, and the extension's images), and the script that draws every view. The action search is
 * the extension's own (core/actions), so the panel and the tests agree.
 * @param cspSource the webview's source for images and styles
 * @param logo the Foxx logo URI
 * @returns HTML
 */
function html(cspSource: string, logo: string): string {
  const nonce = Array.from({ length: 32 }, () => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 62)]).join("");
  const data = JSON.stringify({ actions: ACTIONS, setup: SETUP, criticality: CRITICALITY, ceilings: CEILINGS }).replace(/</g, "\\u003c");
  const searchSource = `const words = ${words.toString()};\nconst searchActions = ${search.toString()};\nconst groupActions = ${groupActions.toString()};\nconst searchWords = words;`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src ${cspSource}; style-src ${cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Foxx</title>
<style>
:root {
  --fox: #ff7a1a; --fox-2: #ffb347; --fox-soft: color-mix(in srgb, var(--fox) 14%, transparent);
  --bg: var(--vscode-editor-background); --surface: var(--vscode-sideBar-background, var(--vscode-editor-background));
  --raised: color-mix(in srgb, var(--vscode-foreground) 4%, var(--bg)); --line: var(--vscode-panel-border, color-mix(in srgb, var(--vscode-foreground) 14%, transparent));
  --muted: var(--vscode-descriptionForeground); --ok: var(--vscode-testing-iconPassed, #3fb950); --bad: var(--vscode-errorForeground, #f14c4c); --warn: var(--vscode-editorWarning-foreground, #cca700);
  --mono: var(--vscode-editor-font-family, ui-monospace, Consolas, monospace); --radius: 10px; --width: 760px;
}
* { box-sizing: border-box; }
html, body { margin: 0; padding: 0; }
body { color: var(--vscode-foreground); background: var(--bg); font: var(--vscode-font-size) / 1.5 var(--vscode-font-family); }
button, input, select, textarea { font: inherit; color: inherit; }
:focus-visible { outline: 1px solid var(--fox); outline-offset: 1px; }
.shell { max-width: var(--width); margin: 0 auto; padding: 0 18px 40px; }
code, .mono { font-family: var(--mono); font-size: 0.92em; }
.muted { color: var(--muted); }
.eyebrow { font-size: 10.5px; font-weight: 700; letter-spacing: .9px; text-transform: uppercase; color: var(--muted); }

/* Header */
header.top { position: sticky; top: 0; z-index: 3; display: flex; align-items: center; gap: 10px; padding: 12px 18px; margin: 0 -18px 14px; background: color-mix(in srgb, var(--bg) 88%, transparent); backdrop-filter: blur(6px); border-bottom: 1px solid var(--line); }
header.top img { width: 26px; height: 26px; filter: drop-shadow(0 0 6px rgba(255, 122, 26, .45)); }
header.top .title { display: flex; flex-direction: column; line-height: 1.15; }
header.top .title b { font-size: 14px; letter-spacing: .2px; }
header.top .title span { font-size: 11px; color: var(--muted); }
.badges { margin-left: auto; display: flex; gap: 6px; flex-wrap: wrap; justify-content: flex-end; }
.badge { display: inline-flex; align-items: center; gap: 6px; padding: 2px 9px; border-radius: 999px; border: 1px solid var(--line); font-size: 11px; white-space: nowrap; background: var(--raised); }
.badge i { width: 7px; height: 7px; border-radius: 50%; background: var(--muted); display: inline-block; }
.badge.ok i { background: var(--ok); box-shadow: 0 0 6px var(--ok); } .badge.bad { border-color: color-mix(in srgb, var(--bad) 50%, transparent); color: var(--bad); } .badge.bad i { background: var(--bad); }
.badge.warn { border-color: color-mix(in srgb, var(--warn) 50%, transparent); } .badge.warn i { background: var(--warn); }

/* Cards */
.card { border: 1px solid var(--line); border-radius: var(--radius); background: var(--raised); margin-bottom: 12px; overflow: hidden; }
.card > .head { display: flex; align-items: center; gap: 8px; padding: 9px 12px; border-bottom: 1px solid var(--line); }
.card > .head .grow { flex: 1; min-width: 0; }
.card > .body { padding: 12px; }
.banner { display: flex; gap: 12px; align-items: flex-start; padding: 12px 14px; border-radius: var(--radius); margin-bottom: 12px; border: 1px solid color-mix(in srgb, var(--bad) 45%, transparent); background: color-mix(in srgb, var(--bad) 9%, transparent); }
.banner.info { border-color: color-mix(in srgb, var(--fox) 45%, transparent); background: var(--fox-soft); }
.banner .icon { font-size: 16px; line-height: 1.2; }
.banner b { display: block; margin-bottom: 2px; }
.banner .row { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }

/* Selection */
.file { display: flex; align-items: center; gap: 8px; flex-wrap: wrap; min-width: 0; }
.file .path { font-family: var(--mono); font-size: 12px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
.tag { display: inline-flex; align-items: center; gap: 4px; padding: 0 7px; border-radius: 5px; font-size: 11px; font-weight: 600; border: 1px solid var(--line); background: var(--bg); white-space: nowrap; }
.tag.fox { color: var(--fox); border-color: color-mix(in srgb, var(--fox) 40%, transparent); background: var(--fox-soft); }
pre.code { margin: 0; max-height: 190px; overflow: auto; font-family: var(--mono); font-size: 12px; line-height: 1.55; background: var(--vscode-textCodeBlock-background, var(--bg)); counter-reset: line var(--first); }
pre.code span.l { display: block; padding: 0 12px 0 0; white-space: pre; }
pre.code span.l::before { counter-increment: line; content: counter(line); display: inline-block; width: 46px; padding-right: 12px; margin-right: 10px; text-align: right; color: var(--muted); opacity: .7; border-right: 1px solid var(--line); }
.empty-state { padding: 18px 14px; text-align: center; color: var(--muted); }
.empty-state b { color: var(--vscode-foreground); display: block; margin-bottom: 4px; }
.icon-btn { border: 0; background: none; cursor: pointer; color: var(--muted); padding: 2px 6px; border-radius: 6px; }
.icon-btn:hover { color: var(--vscode-foreground); background: var(--fox-soft); }

/* Composer */
.composer { border: 1px solid var(--line); border-radius: 12px; background: var(--vscode-input-background); padding: 10px 10px 8px; transition: border-color .15s, box-shadow .15s; }
.composer:focus-within { border-color: var(--fox); box-shadow: 0 0 0 3px var(--fox-soft); }
.composer textarea { width: 100%; border: 0; outline: 0; resize: none; background: transparent; height: 44px; padding: 2px 4px; color: var(--vscode-input-foreground); }
.composer .tools { display: flex; align-items: center; gap: 6px; flex-wrap: wrap; }
.chip { border: 1px solid var(--line); background: var(--bg); border-radius: 999px; padding: 2px 10px; font-size: 12px; cursor: pointer; display: inline-flex; gap: 6px; align-items: center; }
.chip:hover { border-color: var(--fox); color: var(--fox); }
.chip[disabled] { opacity: .45; cursor: not-allowed; }
.kbd { font-family: var(--mono); font-size: 10.5px; padding: 0 5px; border: 1px solid var(--line); border-bottom-width: 2px; border-radius: 4px; color: var(--muted); }
.composer .tools .kbd { margin-left: auto; }
.actions { margin-top: 10px; display: grid; gap: 10px; }
.group { display: grid; gap: 6px; }
.group-head { display: flex; align-items: center; gap: 6px; font-size: 10.5px; font-weight: 700; letter-spacing: .9px; text-transform: uppercase; color: var(--muted); padding: 0 2px; }
.group-head::after { content: ""; flex: 1; border-top: 1px solid var(--line); }
.group-head span { font-weight: 400; letter-spacing: 0; }
.menu-toggle { border: 1px solid var(--line); background: var(--bg); border-radius: 999px; padding: 2px 10px; font-size: 12px; cursor: pointer; display: inline-flex; gap: 6px; align-items: center; }
.menu-toggle:hover, .menu-toggle[aria-expanded=true] { border-color: var(--fox); color: var(--fox); }
.menu-toggle svg { width: 13px; height: 13px; }
.menu-toggle .chev { transition: transform .15s; }
.menu-toggle[aria-expanded=true] .chev { transform: rotate(180deg); }
@media (prefers-reduced-motion: reduce) { .menu-toggle .chev { transition: none; } }
.action { display: grid; grid-template-columns: 34px minmax(0, 1fr) auto; gap: 10px; align-items: center; padding: 9px 10px; border: 1px solid var(--line); border-radius: 9px; background: var(--raised); cursor: pointer; text-align: left; width: 100%; }
.action:hover, .action.active { border-color: var(--fox); background: var(--fox-soft); }
.action.disabled { opacity: .5; cursor: not-allowed; }
.action .glyph { width: 34px; height: 34px; border-radius: 8px; display: grid; place-items: center; background: var(--bg); border: 1px solid var(--line); color: var(--fox); }
.action .glyph svg { width: 18px; height: 18px; }
.action b { display: block; font-weight: 600; }
.action small { color: var(--muted); display: block; overflow: hidden; text-overflow: ellipsis; }
.action mark { background: none; color: var(--fox); font-weight: 700; }
.action .need { font-size: 10.5px; color: var(--warn); white-space: nowrap; }

/* Configure */
.steps { display: flex; gap: 6px; align-items: center; margin-bottom: 12px; font-size: 12px; color: var(--muted); }
.steps span.on { color: var(--vscode-foreground); font-weight: 600; }
.steps .sep { opacity: .5; }
.section { padding: 12px; border-top: 1px solid var(--line); }
.section:first-child { border-top: 0; }
.section h4 { margin: 0 0 8px; font-size: 11px; letter-spacing: .8px; text-transform: uppercase; color: var(--muted); }
.field { margin-bottom: 10px; }
.field:last-child { margin-bottom: 0; }
.field label { display: block; font-weight: 600; margin-bottom: 4px; }
.field .help { color: var(--muted); font-size: 11.5px; margin-top: 3px; }
.field.error input { border-color: var(--bad); }
.field .error-text { color: var(--bad); font-size: 11.5px; margin-top: 3px; }
input[type=text], select, textarea.context { width: 100%; padding: 6px 9px; background: var(--vscode-input-background); color: var(--vscode-input-foreground); border: 1px solid var(--vscode-input-border, var(--line)); border-radius: 7px; }
textarea.context { height: 120px; resize: vertical; font-family: var(--vscode-font-family); }
.grid2 { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 10px; }
.segmented { display: grid; grid-template-columns: repeat(auto-fit, minmax(120px, 1fr)); gap: 6px; }
.segmented label { margin: 0; font-weight: 400; display: flex; flex-direction: column; gap: 2px; padding: 8px 10px; border: 1px solid var(--line); border-radius: 8px; cursor: pointer; background: var(--bg); }
.segmented label b { font-weight: 600; overflow-wrap: anywhere; }
.chip svg { width: 13px; height: 13px; color: var(--fox); }
.segmented label small { color: var(--muted); font-size: 11px; }
.segmented input { position: absolute; opacity: 0; pointer-events: none; }
.segmented label:has(input:checked) { border-color: var(--fox); background: var(--fox-soft); }
.segmented label:has(input:focus-visible) { outline: 1px solid var(--fox); }
.switch { display: flex; align-items: center; gap: 10px; cursor: pointer; font-weight: 600; }
.switch input { width: 34px; height: 18px; appearance: none; border-radius: 999px; background: var(--line); position: relative; cursor: pointer; margin: 0; flex: none; }
.switch input::after { content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px; border-radius: 50%; background: var(--vscode-foreground); transition: left .15s; }
.switch input:checked { background: var(--fox); }
.switch input:checked::after { left: 18px; background: #fff; }
.plan { margin: 0; padding: 0; list-style: none; counter-reset: plan; }
.plan li { counter-increment: plan; display: grid; grid-template-columns: 22px minmax(0, 1fr); gap: 8px; padding: 4px 0; }
.plan li::before { content: counter(plan); width: 20px; height: 20px; border-radius: 50%; display: grid; place-items: center; font-size: 11px; font-weight: 700; background: var(--fox-soft); color: var(--fox); }
.footer { display: flex; align-items: center; gap: 8px; padding: 10px 12px; border-top: 1px solid var(--line); background: var(--bg); }
.footer .grow { flex: 1; }
.btn { border: 1px solid var(--line); background: var(--bg); padding: 6px 14px; border-radius: 8px; cursor: pointer; display: inline-flex; align-items: center; gap: 6px; }
.btn:hover { border-color: var(--fox); }
.btn.primary { border-color: transparent; color: #1a0d05; font-weight: 700; background: linear-gradient(90deg, var(--fox), var(--fox-2)); }
.btn.primary:hover { filter: brightness(1.06); }
.btn.primary[disabled] { opacity: .5; cursor: not-allowed; filter: none; }
.btn.ghost { border-color: transparent; color: var(--muted); }

/* Run */
.timeline { list-style: none; margin: 0; padding: 4px 0; }
.timeline li { display: grid; grid-template-columns: 22px minmax(0, 1fr) auto; gap: 10px; padding: 7px 0; position: relative; }
.timeline li:not(:last-child)::after { content: ""; position: absolute; left: 10px; top: 28px; bottom: -6px; border-left: 1px dashed var(--line); }
.timeline .dot { width: 20px; height: 20px; border-radius: 50%; display: grid; place-items: center; font-size: 11px; font-weight: 800; border: 1px solid var(--line); background: var(--bg); }
.timeline .running .dot { border-color: var(--fox); border-top-color: transparent; animation: spin .8s linear infinite; }
.timeline .done .dot { background: var(--ok); border-color: var(--ok); color: #fff; }
.timeline .failed .dot { background: var(--bad); border-color: var(--bad); color: #fff; }
.timeline .label { font-weight: 600; overflow-wrap: anywhere; }
.timeline .detail { color: var(--muted); font-size: 12px; overflow-wrap: anywhere; }
.timeline .failed .detail { color: var(--bad); }
.timeline .ms { color: var(--muted); font-size: 11px; font-variant-numeric: tabular-nums; }
@keyframes spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .timeline .running .dot { animation: none; } }
.result { padding: 12px 14px; border-top: 1px solid var(--line); display: grid; grid-template-columns: 26px minmax(0, 1fr); gap: 10px; }
.result .mark { width: 24px; height: 24px; border-radius: 50%; display: grid; place-items: center; color: #fff; font-weight: 800; }
.result.ok .mark { background: var(--ok); } .result.bad .mark { background: var(--bad); }
.result b { display: block; overflow-wrap: anywhere; }
.result .hint { color: var(--muted); font-size: 12px; margin-top: 2px; }
.result .row { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 10px; }

/* Activity */
details.activity summary { cursor: pointer; list-style: none; display: flex; align-items: center; gap: 8px; padding: 9px 12px; }
details.activity summary::-webkit-details-marker { display: none; }
details.activity summary::after { content: "▸"; margin-left: auto; color: var(--muted); }
details.activity[open] summary::after { content: "▾"; }
.log { list-style: none; margin: 0; padding: 0 12px 10px; }
.log li { display: grid; grid-template-columns: 54px 12px minmax(0, 1fr); gap: 8px; align-items: baseline; padding: 3px 0; font-size: 12px; }
.log .when { color: var(--muted); font-variant-numeric: tabular-nums; }
.log .ok { color: var(--ok); } .log .bad { color: var(--bad); }
@media (max-width: 520px) { .grid2 { grid-template-columns: 1fr; } .badges { display: none; } }
</style>
</head>
<body>
<div class="shell">
  <header class="top">
    <img src="${logo}" alt="">
    <div class="title"><b>Foxx</b><span id="subtitle">Governance agent for Crane</span></div>
    <div class="badges" id="badges"></div>
  </header>
  <div id="banner"></div>
  <section class="card" id="selection-card" aria-label="Selected code"></section>
  <main id="stage"></main>
  <details class="card activity" id="activity" hidden><summary><span class="eyebrow">Activity</span><span class="muted" id="activity-count"></span></summary><ul class="log" id="log"></ul></details>
</div>
<script nonce="${nonce}">
"use strict";
const vscode = acquireVsCodeApi();
const DATA = ${data};
${searchSource}
let state = { ready: false, policies: [], entities: [], selections: [] };
let view = "compose";
let query = "";
let active = 0;
let shown = [];
let current = null;
let draft = {};
let run = null;
let codeOpen = true;
// The grouped action list stays behind its toggle until opened (remembered) or something is typed
let menuOpen = Boolean((vscode.getState() || {}).menuOpen);
const activity = [];
const $ = (id) => document.getElementById(id);
const esc = (text) => String(text == null ? "" : text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const selected = () => state.selection && !state.selection.empty;
const usable = () => !state.crane || state.crane.state === "ok";

const ICONS = {
  preserve: "M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z M9 12l2 2 4-4",
  target: "M12 3v4M12 17v4M3 12h4M17 12h4M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8z",
  zone: "M4 4h16v16H4z M4 9h16M9 9v11",
  flow: "M5 5h4v4H5zM15 15h4v4h-4zM9 7h4a2 2 0 0 1 2 2v6",
  context: "M4 5h16v11H8l-4 4z M8 9h8M8 12h5",
  zone_attach: "M4 4h10v10H4z M14 10h6v10H10v-6 M17 13v4M15 15h4",
  policy_context: "M6 3h9l4 4v14H6z M9 12h6M9 16h4",
  policy: "M5 4h14v16H5z M9 8h6M9 12h6M9 16h3",
  move: "M4 12h14M14 7l5 5-5 5",
  checkpoint: "M6 3v18M6 4h11l-2 4 2 4H6",
  validate: "M5 12l4 4 10-10",
  test: "M9 3h6M10 3v6l-5 9a2 2 0 0 0 2 3h10a2 2 0 0 0 2-3l-5-9V3",
  status: "M4 19V9M10 19V5M16 19v-7M22 19H2",
  hooks: "M8 3v6a4 4 0 0 0 8 0V3M12 13v8",
  connect: "M5 6h4v4H5zM15 14h4v4h-4zM9 8h2a2 2 0 0 1 2 2v4a2 2 0 0 0 2 2",
  graph: "M6 5a2 2 0 1 0 0 .01M18 12a2 2 0 1 0 0 .01M6 19a2 2 0 1 0 0 .01M8 6l8 5M8 18l8-5",
  menu: "M4 6h16M4 12h16M4 18h10",
  setup: "M12 3l9 5-9 5-9-5z M3 13l9 5 9-5",
};
const glyph = (id) => '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="' + (ICONS[id] || ICONS.validate) + '"/></svg>';
const QUICK = [["preserve", "Preserve"], ["target", "Target"], ["zone", "Zone"], ["flow", "Flow"], ["context", "Context"], ["checkpoint", "Checkpoint"]];
const CEILING_HELP = { observe: "Agent may only read", assisted: "Every change needs approval", delegated: "Changes allowed within policies", autonomous: "No limit beyond policies" };
const CRITICALITY_HELP = { routine: "Ordinary code", sensitive: "Mistakes are costly", critical: "Money, security, or data integrity", restricted: "Changes always need a human" };
const title = (word) => word.charAt(0).toUpperCase() + word.slice(1);

// ------------------------------------------------------------------ header, banner, context

function renderHeader() {
  const crane = state.crane;
  const badges = [];
  if (crane) badges.push('<span class="badge ' + (crane.state === "ok" ? "ok" : "bad") + '" title="' + esc(crane.message || "") + '"><i></i>' + (crane.state === "ok" ? "Crane " + esc(crane.version) : crane.state === "missing" ? "Crane not found" : "Crane " + esc(crane.version || "?") + " · update") + "</span>");
  if (state.ready) {
    badges.push('<span class="badge"><i></i>' + esc(state.repository || "repository") + (state.generation != null ? " · gen " + state.generation : "") + "</span>");
    if (state.issues != null) badges.push('<span class="badge ' + (state.issues ? "warn" : "ok") + '"><i></i>' + (state.issues ? state.issues + " issue" + (state.issues === 1 ? "" : "s") : "Valid") + "</span>");
  } else badges.push('<span class="badge warn"><i></i>Not set up</span>');
  $("badges").innerHTML = badges.join("");
  $("subtitle").textContent = state.ready ? state.policies.length + " polic" + (state.policies.length === 1 ? "y" : "ies") + " · " + state.selections.length + " selection" + (state.selections.length === 1 ? "" : "s") + " · " + state.entities.length + " zone/flow" + (state.entities.length === 1 ? "" : "s") : "Governance agent for Crane";
  let banner = "";
  if (crane && crane.state !== "ok") {
    banner = '<div class="banner" role="alert"><div class="icon">⛔</div><div><b>' + (crane.state === "missing" ? "Crane was not found" : "This Crane is too old for Foxx") + "</b>" + esc(crane.message) +
      '<div class="row"><button class="btn primary" data-command="crane:download">Download latest Crane</button><button class="btn" data-command="crane:path">Set Crane path</button><button class="btn ghost" data-command="crane:recheck">Check again</button></div></div></div>';
  } else if (!state.ready) {
    banner = '<div class="banner info"><div class="icon">🦊</div><div><b>Crane is not set up in this repository</b>Set it up to protect code, create zones and flows, and check every agent tool call.<div class="row"><button class="btn primary" data-run="setup">Set up Crane</button></div></div></div>';
  }
  $("banner").innerHTML = banner;
}

function renderContext() {
  const s = state.selection;
  if (!s) {
    $("selection-card").innerHTML = '<div class="empty-state"><b>No code selected</b>Select lines in an editor, or right-click a file in the Explorer and choose Foxx.</div>';
    return;
  }
  const count = s.end - s.start + 1;
  const lines = String(s.preview || "").split(/\\r?\\n/);
  const head = '<div class="head"><span class="eyebrow">Context</span><div class="file grow"><span class="path" title="' + esc(s.file) + '">' + esc(s.file) + "</span>" +
    '<span class="tag fox">' + (s.empty ? "L" + s.start : "L" + s.start + "–" + s.end) + "</span>" + (s.empty ? "" : '<span class="tag">' + count + " line" + (count === 1 ? "" : "s") + "</span>") +
    (s.symbol ? '<span class="tag" title="Enclosing function">ƒ ' + esc(s.symbol) + "</span>" : "") + "</div>" +
    (s.empty ? "" : '<button class="icon-btn" id="toggle-code" aria-expanded="' + codeOpen + '" title="' + (codeOpen ? "Hide" : "Show") + ' code">' + (codeOpen ? "▾" : "▸") + "</button>") + "</div>";
  const code = !s.empty && codeOpen ? '<pre class="code" style="--first:' + (s.start - 1) + '">' + lines.map((line) => '<span class="l">' + (esc(line) || " ") + "</span>").join("") + "</pre>" : "";
  $("selection-card").innerHTML = head + code;
  const toggle = $("toggle-code");
  if (toggle) toggle.addEventListener("click", () => { codeOpen = !codeOpen; renderContext(); });
}

// ------------------------------------------------------------------ compose

function highlight(text) {
  const typed = searchWords(query);
  return esc(text).replace(/[A-Za-z0-9]+/g, (word) => {
    const hit = typed.find((t) => word.toLowerCase().startsWith(t));
    return hit ? "<mark>" + word.slice(0, hit.length) + "</mark>" + word.slice(hit.length) : word;
  });
}
function pool() { return state.ready ? DATA.actions : [DATA.setup]; }
function blocked(action) { return !usable() ? "Fix Crane first" : action.needsSelection && !selected() ? "Select code first" : ""; }

function setMenu(open) { menuOpen = open; vscode.setState(Object.assign({}, vscode.getState() || {}, { menuOpen })); }
function renderCompose(focus) {
  // With nothing typed and the menu closed, only the search bar and the quick chips show
  const listing = menuOpen || Boolean(query.trim()) || !state.ready;
  const groups = groupActions(searchActions(query, pool()));
  shown = listing ? groups.flatMap((group) => group.actions) : [];
  if (active >= shown.length) active = 0;
  const chips = state.ready ? QUICK.map(([id, label]) => { const action = DATA.actions.find((a) => a.id === id); return '<button class="chip" data-pick="' + id + '" title="' + esc(blocked(action) || action.detail) + '"' + (blocked(action) ? " disabled" : "") + ">" + glyph(id) + esc(label) + "</button>"; }).join("") : "";
  const toggle = state.ready ? '<button type="button" class="menu-toggle" id="menu-toggle" aria-expanded="' + listing + '" aria-controls="actions" title="' + (listing ? "Hide" : "Show") + ' every action, by group">' + glyph("menu") + "Actions" + '<svg class="chev" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="M6 9l6 6 6-6"/></svg></button>' : "";
  let index = -1;
  const item = (action) => {
    index += 1;
    const why = blocked(action);
    return '<button type="button" role="option" class="action' + (index === active ? " active" : "") + (why ? " disabled" : "") + '" data-index="' + index + '" aria-selected="' + (index === active) + '">' +
      '<span class="glyph">' + glyph(action.id) + "</span><span><b>" + highlight(action.label) + "</b><small>" + esc(action.detail) + "</small></span>" + (why ? '<span class="need">' + esc(why) + "</span>" : '<span class="kbd">↵</span>') + "</button>";
  };
  const list = !listing ? "" : '<div class="actions" id="actions" role="listbox" aria-label="Actions">' + (shown.length ? groups.map((group) =>
    '<div class="group" role="group" aria-label="' + esc(group.group) + '"><div class="group-head">' + esc(group.group) + " <span>" + group.actions.length + "</span></div>" + group.actions.map(item).join("") + "</div>").join("")
    : '<div class="empty-state"><b>No action matches</b>Try "zone", "protect", "context", or "graph".</div>') + "</div>";
  $("stage").innerHTML =
    '<div class="composer"><textarea id="composer" rows="2" placeholder="' + (state.ready ? "What should Foxx do with this code? e.g. zone, protect, add context…" : "Type setup, or press Enter") + '" aria-label="Search actions" spellcheck="false">' + esc(query) + "</textarea>" +
    '<div class="tools">' + chips + toggle + '<span class="kbd">' + (listing ? "↑↓ choose · Enter open" : "Type to search · ↓ all actions") + "</span></div></div>" + list;
  const menu = $("menu-toggle");
  if (menu) menu.addEventListener("click", () => { if (listing) { setMenu(false); query = ""; } else setMenu(true); active = 0; renderCompose(true); });
  const box = $("composer");
  box.addEventListener("input", () => { query = box.value; active = 0; renderList(); });
  box.addEventListener("keydown", (event) => {
    if (event.key === "ArrowDown" && !shown.length && !query.trim()) { event.preventDefault(); setMenu(true); renderList(); }
    else if (event.key === "ArrowDown") { event.preventDefault(); active = Math.min(active + 1, shown.length - 1); renderList(); }
    else if (event.key === "ArrowUp") { event.preventDefault(); active = Math.max(active - 1, 0); renderList(); }
    else if (event.key === "Enter" && !event.shiftKey) { event.preventDefault(); open(shown[active]); }
    else if (event.key === "Escape") { if (!query && menuOpen) setMenu(false); query = ""; box.value = ""; active = 0; renderList(); }
  });
  if (focus) { box.focus(); box.setSelectionRange(box.value.length, box.value.length); }
}
// Redraw only the list, so typing never loses the cursor
function renderList() {
  const box = $("composer");
  const at = box.selectionStart;
  renderCompose(false);
  const again = $("composer");
  again.focus();
  again.setSelectionRange(at, at);
}

// ------------------------------------------------------------------ configure

function slug(text) { return (text || "").replace(/[^A-Za-z0-9]+/g, "-").replace(/^-+|-+$/g, "").toLowerCase().replace(/^([^a-z])/, "x-$1").slice(0, 40); }
function option(value, label, chosen) { return '<option value="' + esc(value) + '"' + (value === chosen ? " selected" : "") + ">" + esc(label) + "</option>"; }
function field(id, label, inner, help) { return '<div class="field" id="field-' + id + '"><label for="' + id + '">' + esc(label) + "</label>" + inner + (help ? '<div class="help">' + help + "</div>" : "") + "</div>"; }
function text(id, label, help, placeholder) { return field(id, label, '<input type="text" id="' + id + '" value="' + esc(draft[id] || "") + '" placeholder="' + esc(placeholder || "") + '">', help); }
function choose(id, label, options, help) { return field(id, label, '<select id="' + id + '">' + options.join("") + "</select>", help); }
function segmented(name, label, items, help) {
  return '<div class="field"><label>' + esc(label) + '</label><div class="segmented" role="radiogroup" aria-label="' + esc(label) + '">' + items.map(([value, title, note]) =>
    '<label><input type="radio" name="' + name + '" value="' + esc(value) + '"' + (draft[name] === value ? " checked" : "") + "><b>" + esc(title) + "</b>" + (note ? "<small>" + esc(note) + "</small>" : "") + "</label>").join("") + "</div>" + (help ? '<div class="help">' + help + "</div>" : "") + "</div>";
}
function contextBox(label, help) { return field("context", label, '<textarea id="context" class="context" placeholder="One instruction per line, e.g.\\nCharges must be idempotent.\\nNever retry a capture." spellcheck="true">' + esc(draft.context || "") + "</textarea>", help); }
function toggle(id, label, help) { return '<div class="field"><label class="switch"><input type="checkbox" id="' + id + '"' + (draft[id] ? " checked" : "") + "> " + esc(label) + '</label><div class="help">' + help + "</div></div>"; }
function policyChoice() { return choose("policy", "Policy", [option("", "Default policy", draft.policy)].concat(state.policies.map((p) => option(p, p, draft.policy))), "The policy whose rules include this selection."); }
function coverChoice() {
  const s = state.selection;
  return s && s.symbol ? segmented("cover", "Covers", [["symbol", "Function " + s.symbol, "Follows the function if it moves"], ["file", "Whole file", s.file]], "") : '<div class="field"><label>Covers</label><div class="help">The whole file <code>' + esc(s ? s.file : "") + "</code>: no enclosing function was found.</div></div>";
}
function governance() {
  return '<div class="section"><h4>Limits</h4>' + segmented("criticality", "Criticality", DATA.criticality.map((c) => [c, title(c), CRITICALITY_HELP[c] || ""]), "") +
    segmented("ceiling", "Autonomy ceiling", DATA.ceilings.map((c) => [c, title(c), CEILING_HELP[c] || ""]), "Where zones and flows overlap, the strictest ceiling wins.") + "</div>";
}

function defaults(action) {
  const s = state.selection;
  const base = s ? slug((s.symbol || s.file.split("/").pop()).split(/[.:]/).pop()) : "";
  const first = state.entities[0];
  return {
    zone: { id: base ? base + "-zone" : "", cover: s && s.symbol ? "symbol" : "file", criticality: "sensitive", ceiling: "assisted", enforce: true },
    flow: { id: base ? base + "-flow" : "", entry: s && s.symbol ? s.file + ":" + s.symbol : "", criticality: "sensitive", ceiling: "assisted", enforce: true },
    zone_attach: { cover: s && s.symbol ? "symbol" : "file", zone: (state.entities.find((e) => e.kind === "zone") || {}).id || "" },
    context: { target: first ? first.kind + ":" + first.id : "" },
    checkpoint: { name: "checkpoint-" + new Date().toISOString().slice(0, 10) },
    hooks: { profile: "claude" },
    move: { selection: (state.selections[0] || {}).id || "", policy: state.policies[0] || "" },
    target: { changeType: "" },
  }[action.form] || {};
}

function formFor(action) {
  switch (action.form) {
    case "preserve": return '<div class="section"><h4>Rule</h4>' + policyChoice() + text("name", "Name", "Optional: shown in reports and the governance tree.", "e.g. payment capture") + "</div>" +
      '<div class="section"><h4>Agent guidance</h4>' + contextBox("Context (required)", "Why these lines matter. Agents receive it whenever they touch them; Crane signs it into the selection.") + "</div>";
    case "target": return '<div class="section"><h4>Rule</h4>' + policyChoice() + choose("changeType", "Required kind of change", [option("", "Any real change", draft.changeType), option("logical_bn", "Business logic", draft.changeType), option("logical_cn", "Loops or branches", draft.changeType), option("logical_sn", "Structure", draft.changeType), option("semantic", "Names or comments", draft.changeType)], "The task passes only when the agent makes this kind of change here.") + text("name", "Name", "Optional.", "") + "</div>" +
      '<div class="section"><h4>Agent guidance</h4>' + contextBox("Context (required)", "What the change should achieve. Agents receive it whenever they touch these lines.") + "</div>";
    case "zone": return '<div class="section"><h4>Zone</h4>' + text("id", "Name", "Letters, digits, '-' or '_'.", "") + text("description", "Description", "What this code does and why it is guarded.", "e.g. Card charging and refunds") + coverChoice() + "</div>" + governance() +
      '<div class="section"><h4>Agent guidance</h4>' + contextBox("Context", "Given to the agent whenever a tool call touches this zone. Guidance, never authorization.") + toggle("enforce", "Enforce now", "Off: created as a proposal to review first.") + "</div>";
    case "flow": return '<div class="section"><h4>Flow</h4>' + text("id", "Name", "Letters, digits, '-' or '_'.", "") + text("entry", "Entry point", "<code>path:function</code>. The flow covers it and everything it calls.", "src/app.py:handler") + text("description", "Description", "", "") + "</div>" + governance() +
      '<div class="section"><h4>Agent guidance</h4>' + contextBox("Context (required)", "Given to the agent whenever a tool call touches this flow. Every flow needs context.") + toggle("enforce", "Enforce now", "Off: created as a proposal to review first.") + "</div>";
    case "context":
      return '<div class="section"><h4>Where</h4>' + (state.entities.length ? choose("target", "Zone or flow", state.entities.map((e) => option(e.kind + ":" + e.id, e.kind + " · " + e.id + (e.status === "active" ? "" : " (" + e.status + ")"), draft.target)), "") : '<div class="help">No zones or flows yet. Create one first.</div>') + "</div>" +
        '<div class="section"><h4>Guidance</h4>' + contextBox("Context", "Added to any context it already has.") + "</div>";
    case "zone_attach": {
      const zones = state.entities.filter((e) => e.kind === "zone");
      return '<div class="section"><h4>Zone</h4>' + (zones.length ? choose("zone", "Add to zone", zones.map((e) => option(e.id, e.id, draft.zone)), "") : '<div class="help">No zones yet.</div>') + coverChoice() + "</div>";
    }
    case "policy_context": return '<div class="section"><h4>Policy</h4>' + policyChoice() + '<div class="help">Attaches <code>' + esc(state.selection ? state.selection.file : "the open file") + "</code> as guidance for the policy.</div></div>";
    case "policy": return '<div class="section"><h4>Policy</h4>' + text("name", "Name", "", "e.g. payments") + text("file", "File in .crane/policies", "Without .crane. Empty uses the name.", "") + "</div>";
    case "move": return '<div class="section"><h4>Move</h4>' + choose("selection", "Selection", state.selections.map((r) => option(r.id, r.id + " · " + r.label + " (in " + r.policy + ")", draft.selection)), "") + choose("policy", "To policy", state.policies.map((p) => option(p, p, draft.policy)), "") + "</div>";
    case "checkpoint": return '<div class="section"><h4>Checkpoint</h4>' + text("name", "Name", "Records the current commit as trusted.", "") + "</div>";
    case "hooks": return '<div class="section"><h4>Agent</h4>' + segmented("profile", "Install hooks for", [["claude", "Claude Code", "PreToolUse, PostToolUse, Stop…"], ["codex", "Codex", "Codex hook profile"], ["copilot", "Copilot CLI", "GitHub Copilot CLI hooks"], ["gemini", "Gemini CLI", "Gemini CLI hooks"]], "") + "</div>";
    default: return "";
  }
}

// What Foxx will do, in order, from the current values
function plan(action) {
  const s = state.selection;
  const where = s ? "<code>" + esc(s.file) + "</code> lines " + s.start + "–" + s.end : "the selection";
  const covers = () => draft.cover === "symbol" && s && s.symbol ? "function <code>" + esc(s.symbol) + "</code>" : "file <code>" + esc(s ? s.file : "") + "</code>";
  const lines = String(draft.context || "").split(/\\n/).filter((line) => line.trim()).length;
  const sign = ["Validate governance with <code>crane validate</code>", "Sign <code>.crane/governance.yaml</code> with your admin key", "If either step fails, restore the previous signed file"];
  switch (action.id) {
    case "preserve": return ["Save the file if it has unsaved changes", "Protect " + where + " in " + (draft.policy ? "policy <b>" + esc(draft.policy) + "</b>" : "the default policy"), "Crane inserts anchor comments around the lines", "Agents are then denied any edit that changes them"];
    case "target": return ["Save the file if it has unsaved changes", "Target " + where + " in " + (draft.policy ? "policy <b>" + esc(draft.policy) + "</b>" : "the default policy"), "A task passes only once the agent changes these lines"];
    case "zone": return ["Add zone <b>" + esc(draft.id || "…") + "</b> to <code>.crane/governance.yaml</code> covering the " + covers(), "Criticality <b>" + esc(draft.criticality) + "</b>, autonomy ceiling <b>" + esc(draft.ceiling) + "</b> (" + esc((CEILING_HELP[draft.ceiling] || "").toLowerCase()) + ")"].concat(lines ? ["Attach " + lines + " line" + (lines === 1 ? "" : "s") + " of agent context"] : [], [draft.enforce ? "Enforce it immediately" : "Keep it as a proposal"], sign);
    case "flow": return ["Add flow <b>" + esc(draft.id || "…") + "</b> from <code>" + esc(draft.entry || "path:function") + "</code> and everything it calls", "Criticality <b>" + esc(draft.criticality) + "</b>, autonomy ceiling <b>" + esc(draft.ceiling) + "</b>"].concat(lines ? ["Attach " + lines + " line" + (lines === 1 ? "" : "s") + " of agent context"] : [], [draft.enforce ? "Enforce it immediately" : "Keep it as a proposal"], sign);
    case "context": return ["Add " + (lines || "your") + " line" + (lines === 1 ? "" : "s") + " of guidance to <b>" + esc((draft.target || "").replace(":", " ")) + "</b>"].concat(sign);
    case "zone_attach": return ["Add the " + covers() + " to zone <b>" + esc(draft.zone || "…") + "</b>"].concat(sign);
    case "policy_context": return ["Attach <code>" + esc(s ? s.file : "") + "</code> to " + (draft.policy ? "policy <b>" + esc(draft.policy) + "</b>" : "the first policy"), "Refresh selections"];
    case "policy": return ["Create policy <b>" + esc(draft.name || "…") + "</b> in <code>.crane/policies/" + esc(draft.file || (draft.name || "").toLowerCase() || "…") + ".crane</code>"];
    case "move": return ["Move <b>" + esc(draft.selection || "…") + "</b> to policy <b>" + esc(draft.policy || "…") + "</b>"];
    case "checkpoint": return ["Record the current commit as checkpoint <b>" + esc(draft.name || "…") + "</b>", "Later checks compare against it"];
    case "hooks": return ["Install Crane's hooks for " + (draft.profile || "claude"), "Every tool call is decided before it runs and verified after"];
    default: return [];
  }
}

function open(action) {
  if (!action) return;
  if (blocked(action)) return;
  current = action;
  draft = defaults(action);
  if (action.form === "none") { start(action.id, {}); return; }
  view = "configure";
  renderConfigure();
}

function renderConfigure() {
  const action = current;
  $("stage").innerHTML = '<div class="steps"><span>Choose</span><span class="sep">›</span><span class="on">Configure</span><span class="sep">›</span><span>Run</span></div>' +
    '<form class="card" id="form" novalidate><div class="head"><span class="glyph" style="color:var(--fox);display:grid;width:22px;height:22px">' + glyph(action.id) + '</span><div class="grow"><b>' + esc(action.label) + '</b><div class="muted" style="font-size:12px">' + esc(action.detail) + "</div></div></div>" +
    formFor(action) + '<div class="section"><h4>Foxx will</h4><ol class="plan" id="plan"></ol></div>' +
    '<div class="footer"><button type="button" class="btn ghost" id="back">← Back</button><span class="grow"></span><span class="kbd">Ctrl ↵</span><button type="submit" class="btn primary" id="go">' + esc(action.label) + "</button></div></form>";
  const form = $("form");
  const sync = () => { readDraft(); $("plan").innerHTML = plan(action).map((step) => "<li><span>" + step + "</span></li>").join(""); };
  form.addEventListener("input", sync);
  form.addEventListener("change", sync);
  form.addEventListener("submit", (event) => { event.preventDefault(); submit(); });
  form.addEventListener("keydown", (event) => { if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) { event.preventDefault(); submit(); } else if (event.key === "Escape") back(); });
  $("back").addEventListener("click", back);
  sync();
  const first = form.querySelector("input[type=text], select, textarea");
  if (first) first.focus();
}
function readDraft() {
  for (const element of $("form").querySelectorAll("input, select, textarea")) {
    if (element.type === "radio") { if (element.checked) draft[element.name] = element.value; }
    else if (element.type === "checkbox") draft[element.id] = element.checked;
    else if (element.id) draft[element.id] = element.value;
  }
}
// Required fields, checked before anything runs
function problems(action) {
  const out = {};
  const name = /^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$/;
  if ((action.id === "zone" || action.id === "flow") && !name.test(draft.id || "")) out.id = "Use letters, digits, '-' or '_' (starting with a letter or digit).";
  if (action.id === "flow" && !/.+:.+/.test(draft.entry || "")) out.entry = "Write path:function, for example src/app.py:handler.";
  if (["context", "preserve", "target", "flow"].includes(action.id) && !String(draft.context || "").trim()) out.context = "Write at least one line of guidance for the agent.";
  if ((action.id === "policy" || action.id === "checkpoint") && !name.test(draft.name || "")) out.name = "Use letters, digits, '-' or '_'.";
  return out;
}
function submit() {
  readDraft();
  const found = problems(current);
  for (const node of document.querySelectorAll(".field.error")) { node.classList.remove("error"); const text = node.querySelector(".error-text"); if (text) text.remove(); }
  const ids = Object.keys(found);
  if (ids.length) {
    for (const id of ids) { const node = $("field-" + id); if (node) { node.classList.add("error"); node.insertAdjacentHTML("beforeend", '<div class="error-text">' + esc(found[id]) + "</div>"); } }
    const first = $(ids[0]); if (first) first.focus();
    return;
  }
  start(current.id, Object.assign({}, draft));
}
function back() { view = "compose"; current = null; renderStage(true); }

// ------------------------------------------------------------------ run

function start(id, values) {
  const action = DATA.actions.concat([DATA.setup, { id: "undo", label: "Undo the last governance change", detail: "" }]).find((a) => a.id === id) || { id, label: id };
  run = { action, steps: [], result: null, started: Date.now() };
  view = "run";
  renderRun();
  vscode.postMessage({ type: "run", action: id, values });
}
function renderRun() {
  const r = run;
  const elapsed = ((Date.now() - r.started) / 1000).toFixed(1);
  const steps = r.steps.map((step) => '<li class="' + step.state + '"><span class="dot">' + (step.state === "done" ? "✓" : step.state === "failed" ? "✕" : "") + '</span><span><span class="label">' + esc(step.label) + "</span>" + (step.detail ? '<div class="detail">' + esc(step.detail) + "</div>" : "") + '</span><span class="ms">' + (step.ms != null ? (step.ms < 1000 ? step.ms + " ms" : (step.ms / 1000).toFixed(1) + " s") : "") + "</span></li>").join("");
  const result = r.result ? '<div class="result ' + (r.result.ok ? "ok" : "bad") + '"><span class="mark">' + (r.result.ok ? "✓" : "!") + "</span><div><b>" + esc(r.result.message) + "</b>" + (r.result.hint ? '<div class="hint">' + esc(r.result.hint) + "</div>" : "") +
    '<div class="row">' + (r.result.actions || []).map((item) => '<button class="btn' + (item.id === "undo" ? "" : "") + '" ' + (item.kind === "run" ? 'data-run="' + esc(item.id) + '"' : 'data-command="' + esc(item.id) + '"') + ">" + esc(item.label) + "</button>").join("") +
    '<span style="flex:1"></span><button class="btn primary" id="again">New action</button></div></div></div>' : "";
  $("stage").innerHTML = '<div class="steps"><span>Choose</span><span class="sep">›</span><span>Configure</span><span class="sep">›</span><span class="on">Run</span></div>' +
    '<section class="card" aria-live="polite"><div class="head"><span class="glyph" style="color:var(--fox);display:grid;width:22px;height:22px">' + glyph(r.action.id) + '</span><div class="grow"><b>' + esc(r.action.label) + '</b></div><span class="muted mono" id="elapsed">' + (r.result ? elapsed + " s" : "running · " + elapsed + " s") + "</span></div>" +
    '<div class="body"><ol class="timeline">' + (steps || '<li class="running"><span class="dot"></span><span class="label">Starting…</span><span></span></li>') + "</ol></div>" + result + "</section>";
  const again = $("again");
  if (again) { again.addEventListener("click", () => { query = ""; back(); }); again.focus(); }
}
const ticker = setInterval(() => { if (view === "run" && run && !run.result) { const node = $("elapsed"); if (node) node.textContent = "running · " + ((Date.now() - run.started) / 1000).toFixed(1) + " s"; } }, 200);
function logRun() {
  activity.unshift({ at: new Date(), label: run.action.label, ok: run.result.ok, message: run.result.message });
  $("activity").hidden = false;
  $("activity-count").textContent = activity.length + " this session";
  $("log").innerHTML = activity.slice(0, 30).map((item) => '<li><span class="when">' + item.at.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) + '</span><span class="' + (item.ok ? "ok" : "bad") + '">' + (item.ok ? "●" : "●") + "</span><span><b>" + esc(item.label) + '</b> <span class="muted">' + esc(item.message) + "</span></span></li>").join("");
}

// ------------------------------------------------------------------ wiring

function renderStage(focus) {
  if (view === "compose") renderCompose(focus);
  else if (view === "configure") renderConfigure();
  else renderRun();
}
document.addEventListener("click", (event) => {
  const command = event.target.closest("[data-command]");
  if (command) { vscode.postMessage({ type: "command", id: command.dataset.command }); return; }
  const runner = event.target.closest("[data-run]");
  if (runner) { start(runner.dataset.run, {}); return; }
  const pick = event.target.closest("[data-pick]");
  if (pick && !pick.disabled) { open(DATA.actions.find((a) => a.id === pick.dataset.pick)); return; }
  const item = event.target.closest(".action[data-index]");
  if (item) { active = Number(item.dataset.index); open(shown[active]); }
});
window.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "state") {
    state = message.state;
    renderHeader();
    renderContext();
    // A new selection never interrupts a form being filled in or a run in progress
    if (view === "compose") {
      const box = $("composer");
      if (box && document.activeElement === box && !message.focus) renderList(); else renderCompose(message.focus);
    }
  } else if (message.type === "progress" && run) {
    const index = run.steps.findIndex((step) => step.id === message.step.id);
    if (index >= 0) run.steps[index] = message.step; else run.steps.push(message.step);
    renderRun();
  } else if (message.type === "result" && run) {
    run.result = message.result;
    renderRun();
    logRun();
  }
});
vscode.postMessage({ type: "ready" });
</script>
</body>
</html>`;
}
