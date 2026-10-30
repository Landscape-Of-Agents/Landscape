// The shell the Bricks and Resident Agents panels share: one webview panel per kind beside the
// editor, a request/response channel (the page asks for an operation, the extension runs Crane and
// answers with a result in plain words), the panel styles of the Landscape panel, and a small
// landscape graph (Zones, Flows, and what hangs on them) that pans, zooms, and picks a target.
// The pages hold no state of their own beyond the form being filled in.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { ChangePreview, describePreview, Envelope, parseEnvelope, succeeded } from "./core/bricks";
import { CraneResult } from "./core/crane";
import { GraphData, layout } from "./core/graph";
import { friendlyError, problemLines } from "./core/workbench";

/** What an operation ended with, as the page shows it. */
export interface OpResult {
  ok: boolean;
  message: string;
  /** Problems Crane reported, one per line. */
  problems?: string[];
  /** Data the page asked for (a load). */
  data?: unknown;
  /** The person cancelled; nothing changed. */
  cancelled?: boolean;
  /** Problems of single form fields (field → text). */
  fields?: Record<string, string>;
}

/** What the panels need from the rest of Landscape. */
export interface PanelDeps {
  /** Run Crane in the repository (undefined without one; code 126 when Crane cannot be used);
   * input is its standard input (credentials go only this way, never in arguments or logs). */
  crane(args: string[], input?: string): Promise<CraneResult | undefined>;
  /** The governance graph, or why there is none. */
  graph(refresh: boolean): Promise<{ data?: GraphData; error?: string }>;
  /** The Zones and Flows of .crane/governance.yaml. */
  targets(): { kind: "zone" | "flow"; id: string; status: string }[];
  /** Refresh the other views after a change. */
  refresh(): Promise<void>;
  /** Record an editor event (DASHBOARD.md F31). */
  record(name: string, fields: Record<string, string | number | boolean | undefined>): void;
}

/**
 * Run a Crane command with '--format json' and read its envelope, turning every failure into
 * words a person can act on.
 * @param deps the extension side
 * @param args arguments after "crane"
 * @param input standard input (credentials)
 * @returns the envelope, or the error
 */
export async function call<T = Record<string, unknown>>(deps: PanelDeps, args: string[], input?: string): Promise<{ envelope?: Envelope<T>; error?: string }> {
  const full = args.includes("--format") ? args : args.concat(["--format", "json"]);
  const result = await deps.crane(full, input);
  if (!result) return { error: "Crane is not set up in this folder. Run 'Landscape: Set Up Crane in This Repository' first." };
  if (result.code === 126) return { error: friendlyError(result.stderr || "Crane cannot be used.") };
  const envelope = parseEnvelope<T>(result.stdout);
  if (!envelope) return { error: friendlyError(result.stderr || result.stdout) };
  return { envelope };
}

/**
 * The result of a refused or failed envelope: Crane's summary in plain words and its problems.
 * @param envelope envelope
 * @param problems problems from the envelope's data, when it has them
 * @returns result
 */
export function refused(envelope: Envelope<unknown>, problems: unknown = undefined): OpResult {
  const lines = problemLines(problems);
  return { ok: false, message: friendlyError(envelope.summary), problems: lines.length ? lines : envelope.reason_codes.map((code) => `Reason: ${code}`) };
}

/**
 * Write text to a temporary file Crane reads (never inside the repository).
 * @param name file name
 * @param text content
 * @returns the path, and a cleanup
 */
export function scratch(name: string, text: string): { file: string; dispose(): void } {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), "landscape-panel-"));
  const file = path.join(directory, name);
  fs.writeFileSync(file, text, "utf8");
  return { file, dispose: () => fs.rmSync(directory, { recursive: true, force: true }) };
}

/** The result of a change the person cancelled. */
export const CANCELLED: OpResult = { ok: false, cancelled: true, message: "Cancelled. Nothing was changed." };

/**
 * Preview a signing brick command with '--dry-run', show what it changes, and sign it only after
 * the person confirms.
 * @param deps the extension side
 * @param args the command's arguments (without --dry-run and --format)
 * @param label what it does, for the confirmation
 * @param progress step reporter
 * @param dialogs asks the person to confirm
 * @returns result
 */
export async function signed(deps: PanelDeps, args: string[], label: string, progress: (text: string) => void, dialogs: Dialogs): Promise<OpResult> {
  progress(`Checking what "${label}" would change`);
  const preview = await call<{ preview?: ChangePreview; validation?: { problems?: unknown } }>(deps, args.concat(["--dry-run"]));
  if (!preview.envelope) return { ok: false, message: preview.error ?? "Crane printed no result." };
  if (!succeeded(preview.envelope.status)) return refused(preview.envelope, preview.envelope.data?.validation?.problems);
  const detail = [preview.envelope.summary, ...describePreview(preview.envelope.data?.preview), "", "Crane signs this into .crane/bricks.json with your admin key; commit .crane afterwards so the team shares it."].join("\n");
  if (!(await dialogs.confirm(`${label}?`, detail, "Sign and apply"))) return CANCELLED;
  progress(`Signing "${label}"`);
  const result = await call(deps, args);
  if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
  if (!succeeded(result.envelope.status)) return refused(result.envelope);
  deps.record("FOXX_BRICK_COMMAND", { command: result.envelope.command, status: result.envelope.status.toUpperCase(), statement: result.envelope.summary });
  return { ok: true, message: result.envelope.summary };
}

/**
 * Open Markdown in a preview beside the panel.
 * @param text Markdown
 */
export async function showMarkdown(text: string): Promise<void> {
  const document = await vscode.workspace.openTextDocument({ content: text, language: "markdown" });
  await vscode.commands.executeCommand("markdown.showPreviewToSide", document.uri).then(undefined, () => vscode.window.showTextDocument(document, { preview: true }));
}

/** One choice of a pick dialog. */
export interface PickItem<T> {
  label: string;
  description?: string;
  detail?: string;
  picked?: boolean;
  value: T;
}

/** A one-line question. */
export interface InputOptions {
  title: string;
  prompt?: string;
  value?: string;
  placeholder?: string;
  help?: string;
  button?: string;
  /** A problem with the value (shown under the field, asked again), or undefined when valid. */
  validate?: (value: string) => string | undefined;
}

/** Questions Landscape asks the person, shown as Landscape dialogs (never native pop-ups). */
export interface Dialogs {
  /** Ask to confirm a change; tone "danger" for one that cannot be undone. */
  confirm(title: string, detail: string, button: string, tone?: "primary" | "danger"): Promise<boolean>;
  /** Ask for the reason a change is recorded with (undefined when cancelled). */
  reason(title: string): Promise<string | undefined>;
  /** Ask for one line of text, validated in place (undefined when cancelled). */
  input(options: InputOptions): Promise<string | undefined>;
  /** Ask for several lines of text (undefined when cancelled). */
  text(options: { title: string; prompt?: string; value?: string; help?: string; button?: string; detail?: string }): Promise<string | undefined>;
  /** Ask to choose one item (undefined when cancelled). */
  pick<T>(options: { title: string; items: PickItem<T>[]; detail?: string; button?: string }): Promise<T | undefined>;
  /** Ask to choose any number of items (undefined when cancelled). */
  pickMany<T>(options: { title: string; items: PickItem<T>[]; detail?: string; button?: string }): Promise<T[] | undefined>;
}

/** A question as the page shows it. */
interface Question {
  kind: "confirm" | "reason" | "input" | "text" | "pick";
  title: string;
  detail?: string;
  button: string;
  tone?: string;
  prompt?: string;
  value?: string;
  placeholder?: string;
  help?: string;
  error?: string;
  many?: boolean;
  items?: { label: string; description?: string; detail?: string; picked?: boolean }[];
}

/** An answer from the page. */
interface Answer {
  ok: boolean;
  value?: string;
  picked?: number[];
}

/**
 * Turn a way of showing one question into the dialogs operations use.
 * @param ask show a question and wait for its answer
 * @returns dialogs
 */
function dialogsOf(ask: (question: Question) => Promise<Answer>): Dialogs {
  const list = <T>(items: PickItem<T>[]) => items.map(({ label, description, detail, picked }) => ({ label, description, detail, picked }));
  return {
    confirm: async (title, detail, button, tone = "primary") => (await ask({ kind: "confirm", title, detail, button, tone })).ok,
    reason: async (title) => {
      const answer = await ask({ kind: "reason", title, button: "Continue" });
      return answer.ok && answer.value?.trim() ? answer.value.trim() : undefined;
    },
    input: async (options) => {
      let value = options.value ?? "";
      let error: string | undefined;
      for (;;) {
        const answer = await ask({ kind: "input", title: options.title, prompt: options.prompt, value, placeholder: options.placeholder, help: options.help, button: options.button ?? "OK", error });
        if (!answer.ok) return undefined;
        value = answer.value ?? "";
        error = options.validate?.(value);
        if (!error) return value;
      }
    },
    text: async (options) => {
      const answer = await ask({ kind: "text", title: options.title, prompt: options.prompt, value: options.value, help: options.help, detail: options.detail, button: options.button ?? "Save" });
      return answer.ok ? (answer.value ?? "") : undefined;
    },
    pick: async (options) => {
      const answer = await ask({ kind: "pick", title: options.title, detail: options.detail, items: list(options.items), button: options.button ?? "Choose" });
      const index = answer.picked?.[0];
      return answer.ok && index !== undefined ? options.items[index]?.value : undefined;
    },
    pickMany: async (options) => {
      const answer = await ask({ kind: "pick", many: true, title: options.title, detail: options.detail, items: list(options.items), button: options.button ?? "Choose" });
      return answer.ok ? (answer.picked ?? []).map((index) => options.items[index].value) : undefined;
    },
  };
}

/** The page that only shows a question when no Landscape panel is open. */
const QUESTION_PAGE: WorkbenchPage = {
  viewType: "foxx.question",
  title: "Landscape",
  styles: "",
  body: '<div class="shell"><header class="top"><span class="mark" aria-hidden="true">▦</span><div class="grow"><b>Landscape</b><div class="muted small">Answer the question to go on; closing this tab cancels it.</div></div></header><div id="notice"></div></div>',
  script: 'vscode.postMessage({ type: "ready" });',
};

/** The extension side of the question page. */
const QUESTION_HOST: WorkbenchHost = {
  state: async () => ({}),
  run: async () => ({ ok: false, message: "This page only asks questions." }),
};

/** The extension side of a panel. */
export interface WorkbenchHost {
  /** What the page shows; refresh asks Crane again. */
  state(refresh: boolean): Promise<unknown>;
  /** Run an operation the page asked for, reporting steps as they go. */
  run(op: string, values: Record<string, unknown>, progress: (text: string) => void, dialogs: Dialogs): Promise<OpResult>;
}

/** A panel kind: its view type, title, and page. */
export interface WorkbenchPage {
  viewType: string;
  title: string;
  /** The page's own markup, styles, and script (the shell adds the rest). */
  body: string;
  styles: string;
  script: string;
}

/** The Bricks and Resident Agents panels (one per kind and window). */
export class Workbench {
  private static readonly open = new Map<string, Workbench>();

  /**
   * Open a panel beside the editor, or bring it forward.
   * @param extensionUri the extension's folder
   * @param page the panel kind
   * @param host extension side
   * @param focus a Zone or Flow ("zone:ID") to select once it shows
   */
  static show(extensionUri: vscode.Uri, page: WorkbenchPage, host: WorkbenchHost, focus?: string): void {
    const current = Workbench.open.get(page.viewType);
    if (current) {
      current.panel.reveal(vscode.ViewColumn.Beside);
      if (focus) void current.panel.webview.postMessage({ type: "focus", id: focus });
      return;
    }
    const panel = vscode.window.createWebviewPanel(page.viewType, page.title, { viewColumn: vscode.ViewColumn.Beside, preserveFocus: false }, {
      enableScripts: true,
      retainContextWhenHidden: true,
      localResourceRoots: [vscode.Uri.joinPath(extensionUri, "images")],
    });
    panel.iconPath = vscode.Uri.joinPath(extensionUri, "images", "icon.png");
    Workbench.open.set(page.viewType, new Workbench(panel, page, host, focus));
  }

  /**
   * Send fresh state to an open panel of a kind.
   * @param viewType panel kind
   */
  static update(viewType: string): void {
    void Workbench.open.get(viewType)?.send(false);
  }

  /** Whether a panel of a kind is open. */
  static isOpen(viewType: string): boolean {
    return Workbench.open.has(viewType);
  }

  /**
   * The dialogs Landscape asks through from anywhere (a command, a tree view, a setup step): in
   * the Landscape panel that is in front, or else in a small Landscape tab that closes once it is
   * answered. Never a native pop-up.
   * @param extensionUri the extension's folder
   * @returns dialogs
   */
  static dialogs(extensionUri: vscode.Uri): Dialogs {
    return dialogsOf(async (question) => {
      const panels = [...Workbench.open.values()];
      const shown = panels.find((item) => item.panel.active) ?? panels.find((item) => item.panel.visible && item.page.viewType !== QUESTION_PAGE.viewType);
      if (shown) {
        shown.panel.reveal(undefined, false);
        await shown.ready;
        return shown.ask(question);
      }
      Workbench.show(extensionUri, QUESTION_PAGE, QUESTION_HOST);
      const asking = Workbench.open.get(QUESTION_PAGE.viewType)!;
      await asking.ready;
      try {
        return await asking.ask(question);
      } finally {
        if (!asking.questions.size) asking.panel.dispose();
      }
    });
  }

  /**
   * Create the panel's controller.
   * @param panel webview panel
   * @param page the panel kind
   * @param host extension side
   * @param focus a target to select once it shows
   */
  private constructor(private readonly panel: vscode.WebviewPanel, private readonly page: WorkbenchPage, private readonly host: WorkbenchHost, focus: string | undefined) {
    this.ready = new Promise((resolve) => (this.markReady = resolve));
    panel.webview.html = html(panel.webview.cspSource, page);
    panel.onDidDispose(() => {
      Workbench.open.delete(page.viewType);
      // A closed panel answers every open question with "no"
      for (const answer of this.questions.values()) answer({ ok: false });
      this.questions.clear();
    });
    panel.webview.onDidReceiveMessage(async (message: { type: string; op?: string; values?: Record<string, unknown>; request?: number; dialog?: number; ok?: boolean; value?: string; picked?: number[] }) => {
      if (message.type === "dialogResult" && message.dialog !== undefined) {
        const answer = this.questions.get(message.dialog);
        this.questions.delete(message.dialog);
        answer?.({ ok: Boolean(message.ok), value: message.value, picked: message.picked });
      } else if (message.type === "ready" || message.type === "refresh") {
        if (message.type === "ready") this.markReady();
        await this.send(message.type === "refresh");
        if (message.type === "ready" && focus) void panel.webview.postMessage({ type: "focus", id: focus });
      } else if (message.type === "run" && message.op) {
        const request = message.request;
        const progress = (text: string) => void panel.webview.postMessage({ type: "progress", request, text });
        let result: OpResult;
        try {
          result = await this.host.run(message.op, message.values ?? {}, progress, this.dialogs);
        } catch (error) {
          result = { ok: false, message: `Something went wrong in Landscape: ${(error as Error).message}. Open the Landscape output for details.` };
        }
        void panel.webview.postMessage({ type: "result", request, result });
        if (!message.op.startsWith("load:")) await this.send(false);
      }
    });
  }

  /**
   * Send the current state to the page.
   * @param refresh ask Crane again
   */
  private async send(refresh: boolean): Promise<void> {
    // A quiet update while state is being read would only read it again
    if (this.sending && !refresh) return;
    this.sending = true;
    try {
      const state = await this.host.state(refresh);
      void this.panel.webview.postMessage({ type: "state", state });
    } catch (error) {
      void this.panel.webview.postMessage({ type: "state", state: { problem: `Landscape could not read the state: ${(error as Error).message}` } });
    } finally {
      this.sending = false;
    }
  }

  /** State is being read for the page. */
  private sending = false;

  /** Dialogs waiting for the person's answer, by id. */
  private readonly questions = new Map<number, (answer: Answer) => void>();
  private asked = 0;

  /** The page has loaded and can show dialogs. */
  private readonly ready: Promise<void>;
  private markReady: () => void = () => undefined;

  /**
   * Show a dialog in the page and wait for the answer.
   * @param question what to show
   * @returns the answer
   */
  private ask(question: Question): Promise<Answer> {
    const id = ++this.asked;
    return new Promise((resolve) => {
      this.questions.set(id, resolve);
      void this.panel.webview.postMessage({ type: "dialog", id, ...question });
    });
  }

  /** The dialogs this panel's operations ask through. */
  private readonly dialogs: Dialogs = dialogsOf((question) => this.ask(question));
}

/**
 * Build a panel's page: a strict content security policy (only this page's script and inline
 * styles), the shared styles and graph, then the page's own.
 * @param cspSource the webview's source for styles
 * @param page the panel kind
 * @returns HTML
 */
function html(cspSource: string, page: WorkbenchPage): string {
  const nonce = Array.from({ length: 32 }, () => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 62)]).join("");
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${page.title}</title>
<style>${STYLES}${page.styles}</style>
</head>
<body>
${page.body}
<script nonce="${nonce}">
"use strict";
const layout = ${layout.toString()};
${CLIENT}
${page.script}
</script>
</body>
</html>`;
}

/** The styles of the Landscape panel, plus forms, tabs, lists, and the graph. */
const STYLES = `
:root {
  --fox: #ff7a1a; --fox-2: #ffb347; --fox-soft: color-mix(in srgb, var(--fox) 14%, transparent);
  --bg: var(--vscode-editor-background); --fg: var(--vscode-foreground);
  --raised: color-mix(in srgb, var(--fg) 4%, var(--bg)); --line: var(--vscode-panel-border, color-mix(in srgb, var(--fg) 14%, transparent));
  --muted: var(--vscode-descriptionForeground); --ok: var(--vscode-testing-iconPassed, #3fb950); --bad: var(--vscode-errorForeground, #f14c4c); --warn: var(--vscode-editorWarning-foreground, #cca700);
  --zone: var(--vscode-charts-blue, #3794ff); --flow: var(--fox); --busy: var(--vscode-charts-blue, #3794ff); --off: var(--muted);
  --mono: var(--vscode-editor-font-family, ui-monospace, Consolas, monospace); --radius: 10px;
}
* { box-sizing: border-box; }
[hidden] { display: none !important; }
html, body { margin: 0; padding: 0; }
body { color: var(--fg); background: var(--bg); font: var(--vscode-font-size) / 1.5 var(--vscode-font-family); }
button, input, select, textarea { font: inherit; color: inherit; }
:focus-visible { outline: 1px solid var(--fox); outline-offset: 1px; }
.shell { max-width: 1180px; margin: 0 auto; padding: 0 18px 40px; }
code, .mono { font-family: var(--mono); font-size: 0.92em; }
.muted { color: var(--muted); }
.small { font-size: 11.5px; }
.grow { flex: 1; min-width: 0; }
.row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
header.top { position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 10px; padding: 12px 18px; margin: 0 -18px 14px; background: color-mix(in srgb, var(--bg) 90%, transparent); backdrop-filter: blur(6px); border-bottom: 1px solid var(--line); }
header.top .mark { width: 28px; height: 28px; border-radius: 8px; display: grid; place-items: center; background: var(--fox-soft); color: var(--fox); font-size: 15px; }
header.top b { font-size: 14px; }
.card { border: 1px solid var(--line); border-radius: var(--radius); background: var(--raised); margin-bottom: 12px; overflow: hidden; }
.card > .head { display: flex; align-items: center; gap: 8px; padding: 9px 12px; border-bottom: 1px solid var(--line); }
.card > .body { padding: 12px; }
.layout { display: grid; grid-template-columns: minmax(0, 1.6fr) minmax(280px, 1fr); gap: 12px; align-items: start; }
@media (max-width: 820px) { .layout { grid-template-columns: 1fr; } }
.banner { display: flex; gap: 10px; align-items: flex-start; padding: 10px 12px; border-radius: var(--radius); margin-bottom: 12px; border: 1px solid color-mix(in srgb, var(--bad) 45%, transparent); background: color-mix(in srgb, var(--bad) 9%, transparent); }
.banner.ok { border-color: color-mix(in srgb, var(--ok) 45%, transparent); background: color-mix(in srgb, var(--ok) 9%, transparent); }
.banner.info { border-color: color-mix(in srgb, var(--fox) 45%, transparent); background: var(--fox-soft); }
.banner .icon { font-size: 15px; line-height: 1.3; }
.banner ul { margin: 6px 0 0; padding-left: 18px; }
.banner .close { margin-left: auto; border: 0; background: none; cursor: pointer; color: var(--muted); }
.tag { display: inline-flex; align-items: center; gap: 4px; padding: 0 7px; border-radius: 5px; font-size: 11px; font-weight: 600; border: 1px solid var(--line); background: var(--bg); white-space: nowrap; }
.tag.ok { color: var(--ok); border-color: color-mix(in srgb, var(--ok) 40%, transparent); }
.tag.bad { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 40%, transparent); }
.tag.warn, .tag.attention { color: var(--warn); border-color: color-mix(in srgb, var(--warn) 40%, transparent); }
.tag.busy { color: var(--busy); border-color: color-mix(in srgb, var(--busy) 40%, transparent); }
.tag.fox { color: var(--fox); border-color: color-mix(in srgb, var(--fox) 40%, transparent); background: var(--fox-soft); }
.btn { display: inline-flex; align-items: center; gap: 6px; border: 1px solid var(--line); background: var(--bg); padding: 4px 11px; border-radius: 7px; cursor: pointer; white-space: nowrap; }
.btn:hover { border-color: var(--fox); }
.btn:disabled { opacity: .5; cursor: default; border-color: var(--line); }
.btn.primary { border-color: transparent; color: #1a0d05; font-weight: 700; background: linear-gradient(90deg, var(--fox), var(--fox-2)); }
.btn.danger { color: var(--bad); }
.btn.small { padding: 1px 8px; font-size: 11.5px; }
/* A small rounded button where a text link would be */
.chip-btn { display: inline-flex; align-items: center; gap: 5px; padding: 1px 9px 1px 7px; border-radius: 999px; border: 1px solid var(--line); background: var(--raised); color: var(--fg); font: inherit; font-size: 11.5px; line-height: 1.6; cursor: pointer; white-space: nowrap; vertical-align: middle; }
.chip-btn:hover, .chip-btn:focus-visible { border-color: var(--fox); color: var(--fox); background: var(--fox-soft); }
.chip-btn > span[aria-hidden] { color: var(--fox); font-weight: 700; }
.chips-row { display: flex; gap: 5px; flex-wrap: wrap; align-items: center; margin-top: 6px; }
.tabs { display: flex; gap: 2px; padding: 0 8px; border-bottom: 1px solid var(--line); overflow-x: auto; }
.tabs button { border: 0; background: none; padding: 9px 12px; cursor: pointer; color: var(--muted); border-bottom: 2px solid transparent; white-space: nowrap; }
.tabs button[aria-selected=true] { color: var(--fg); border-bottom-color: var(--fox); font-weight: 600; }
.tabs .count { margin-left: 4px; padding: 0 5px; border-radius: 8px; background: var(--fox-soft); color: var(--fox); font-size: 10.5px; font-weight: 700; }
.field { margin-bottom: 12px; }
.field > label, .field > .label { display: block; font-weight: 600; margin-bottom: 4px; }
.field .help { color: var(--muted); font-size: 11.5px; margin-top: 3px; }
.field .error-text { color: var(--bad); font-size: 11.5px; margin-top: 3px; }
.field.error input, .field.error textarea, .field.error select { border-color: var(--bad); }
input[type=text], input[type=number], input[type=search], select, textarea { width: 100%; padding: 5px 8px; border-radius: 6px; border: 1px solid var(--vscode-input-border, var(--line)); background: var(--vscode-input-background); color: var(--vscode-input-foreground); }
textarea { resize: vertical; min-height: 64px; }
textarea.code { font-family: var(--mono); font-size: 12px; min-height: 110px; }
.grid2 { display: grid; grid-template-columns: 1fr 1fr; gap: 0 12px; }
@media (max-width: 560px) { .grid2 { grid-template-columns: 1fr; } }
.checks { display: grid; gap: 4px; }
.check { display: flex; align-items: flex-start; gap: 8px; padding: 5px 8px; border-radius: 6px; border: 1px solid var(--line); background: var(--bg); }
.check input { margin-top: 3px; }
.check.disabled { opacity: .55; }
.check .grow small { display: block; color: var(--muted); font-size: 11px; }
details.more { margin-bottom: 12px; border: 1px solid var(--line); border-radius: 8px; padding: 6px 10px; background: var(--bg); }
details.more summary { cursor: pointer; font-weight: 600; }
.list { display: grid; gap: 8px; }
.item { border: 1px solid var(--line); border-radius: 8px; background: var(--bg); padding: 9px 11px; }
.item.picked { border-color: var(--fox); box-shadow: 0 0 0 2px var(--fox-soft); }
.item .title { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
.item .title b { overflow-wrap: anywhere; }
.item .actions { display: flex; gap: 6px; flex-wrap: wrap; margin-top: 8px; }
.item .quote { margin-top: 6px; padding: 6px 8px; border-left: 3px solid var(--line); color: var(--muted); white-space: pre-wrap; font-size: 12px; max-height: 120px; overflow: auto; }
.empty-note { color: var(--muted); padding: 14px; text-align: center; border: 1px dashed var(--line); border-radius: 8px; }
.steps { margin: 8px 0 0; padding: 0; list-style: none; font-size: 12px; }
.steps li { padding: 2px 0; color: var(--muted); }
.steps li.now { color: var(--fg); }
.steps li.now::before { content: "◌ "; color: var(--fox); }
.steps li.done::before { content: "✓ "; color: var(--ok); }
.spin { display: inline-block; width: 12px; height: 12px; border: 2px solid var(--fox); border-top-color: transparent; border-radius: 50%; animation: spin .8s linear infinite; vertical-align: -2px; }
@keyframes spin { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .spin { animation: none; } }
/* The landscape graph */
.graph { position: relative; height: 360px; overflow: hidden; background-color: var(--bg); background-image: radial-gradient(circle, color-mix(in srgb, var(--fg) 14%, transparent) 1px, transparent 1.4px); background-size: 22px 22px; }
.graph svg { position: absolute; inset: 0; width: 100%; height: 100%; cursor: grab; user-select: none; touch-action: none; }
.graph svg.panning { cursor: grabbing; }
.graph .edge { fill: none; stroke: color-mix(in srgb, var(--fg) 34%, transparent); stroke-width: 1.4; }
.graph .edge.stage, .graph .edge.next { stroke: var(--flow); stroke-width: 2; }
.graph .edge.attached { stroke-dasharray: 4 4; }
.graph .node { cursor: pointer; }
.graph .node rect.box { fill: color-mix(in srgb, var(--fg) 6%, var(--bg)); stroke: var(--line); stroke-width: 1.2; }
.graph .node.item rect.box { fill: var(--raised); }
.graph .node.missing rect.box { stroke: var(--bad); stroke-dasharray: 4 3; }
.graph .node:hover rect.box { stroke: var(--fox); }
.graph .node.picked rect.box { stroke: var(--fox); stroke-width: 2.4; fill: var(--fox-soft); }
.graph .node text { fill: var(--fg); font-size: 12px; pointer-events: none; }
.graph .node text.sub { fill: var(--muted); font-size: 10.5px; }
.graph .legend { position: absolute; left: 10px; bottom: 8px; display: flex; gap: 10px; font-size: 11px; color: var(--muted); background: color-mix(in srgb, var(--bg) 85%, transparent); padding: 2px 8px; border-radius: 6px; }
.graph .legend i { display: inline-block; width: 9px; height: 9px; border-radius: 3px; margin-right: 4px; vertical-align: -1px; }
.graph .zoom { position: absolute; right: 10px; bottom: 10px; display: flex; flex-direction: column; border: 1px solid var(--line); border-radius: 8px; overflow: hidden; background: var(--raised); }
.graph .zoom button { width: 28px; height: 26px; border: 0; background: none; cursor: pointer; }
.graph .zoom button + button { border-top: 1px solid var(--line); }
.graph .placeholder { position: absolute; inset: 0; display: grid; place-items: center; text-align: center; color: var(--muted); padding: 20px; pointer-events: none; }
.graph .placeholder b { display: block; color: var(--fg); }
/* Dialogs: the panel's own confirmations and questions */
.modal-back { position: fixed; inset: 0; z-index: 50; display: grid; place-items: center; padding: 16px; background: rgba(0, 0, 0, .45); backdrop-filter: blur(2px); animation: fade .12s ease-out; }
.modal { width: min(520px, 100%); max-height: calc(100vh - 32px); display: flex; flex-direction: column; border: 1px solid color-mix(in srgb, var(--fox) 45%, var(--line)); border-radius: 12px; background: var(--vscode-editorWidget-background, var(--bg)); box-shadow: 0 18px 48px rgba(0, 0, 0, .45); overflow: hidden; animation: rise .14s ease-out; }
.modal .m-head { display: flex; align-items: center; gap: 10px; padding: 12px 14px; border-bottom: 1px solid var(--line); background: linear-gradient(90deg, var(--fox-soft), transparent); }
.modal .m-head .mark { width: 26px; height: 26px; border-radius: 8px; display: grid; place-items: center; background: linear-gradient(135deg, var(--fox), var(--fox-2)); color: #1a0d05; font-weight: 800; flex: none; }
.modal.danger .m-head { background: linear-gradient(90deg, color-mix(in srgb, var(--bad) 14%, transparent), transparent); }
.modal.danger .m-head .mark { background: var(--bad); color: #fff; }
.modal .m-head b { font-size: 13.5px; overflow-wrap: anywhere; }
.modal .m-body { padding: 12px 14px; overflow: auto; }
.modal .m-detail { margin: 0; white-space: pre-wrap; overflow-wrap: anywhere; font-size: 12.5px; line-height: 1.55; }
.modal .m-detail .sub { display: block; padding-left: 12px; font-family: var(--mono); font-size: 11.5px; color: var(--muted); }
.modal .m-detail .note { display: block; margin-top: 8px; color: var(--muted); }
.modal .m-foot { display: flex; justify-content: flex-end; gap: 8px; padding: 10px 14px; border-top: 1px solid var(--line); background: var(--raised); }
.modal .btn.danger-fill { border-color: transparent; background: var(--bad); color: #fff; font-weight: 700; }
.modal .m-list { display: grid; gap: 4px; max-height: 46vh; overflow: auto; }
.modal .m-item { display: flex; gap: 9px; align-items: flex-start; padding: 7px 9px; border: 1px solid var(--line); border-radius: 7px; background: var(--bg); cursor: pointer; }
.modal .m-item:hover, .modal .m-item:has(input:checked) { border-color: var(--fox); background: var(--fox-soft); }
.modal .m-item input { margin-top: 3px; accent-color: var(--fox); }
.modal .m-item small { display: block; color: var(--muted); font-size: 11.5px; }
.modal textarea { width: 100%; }
@keyframes fade { from { opacity: 0; } }
@keyframes rise { from { transform: translateY(8px); opacity: 0; } }
@media (prefers-reduced-motion: reduce) { .modal-back, .modal { animation: none; } }
`;

/** The page side every panel shares: messaging, notices, tabs, and the graph. */
const CLIENT = `
const vscode = acquireVsCodeApi();
const $ = (id) => document.getElementById(id);
const esc = (text) => String(text == null ? "" : text).replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const clip = (text, n) => { text = String(text || ""); return text.length > n ? text.slice(0, n - 1) + "…" : text; };
let requests = 0;
const waiting = new Map();
// Ask the extension for an operation; the promise settles with its result
function ask(op, values, onProgress) {
  const request = ++requests;
  return new Promise((resolve) => { waiting.set(request, { resolve, onProgress }); vscode.postMessage({ type: "run", op, values: values || {}, request }); });
}
// A notice at the top of the page: tone ok, info, or bad; problems become a list
function notice(tone, title, problems) {
  const box = $("notice");
  if (!title) { box.innerHTML = ""; return; }
  const icon = tone === "ok" ? "✓" : tone === "info" ? "ℹ" : "⚠";
  box.innerHTML = '<div class="banner ' + tone + '" role="' + (tone === "bad" ? "alert" : "status") + '"><span class="icon" aria-hidden="true">' + icon + '</span><div class="grow"><b>' + esc(title) + "</b>" +
    (problems && problems.length ? "<ul>" + problems.map((line) => "<li>" + esc(line) + "</li>").join("") + "</ul>" : "") + '</div><button class="close" aria-label="Dismiss" data-dismiss>✕</button></div>';
  box.scrollIntoView({ block: "nearest" });
}
document.addEventListener("click", (event) => { if (event.target.closest("[data-dismiss]")) notice(); });
// Run an operation from a button: it shows progress, then the result in plain words
async function perform(button, op, values, success) {
  const label = button ? button.innerHTML : "";
  const steps = [];
  if (button) { button.disabled = true; button.innerHTML = '<span class="spin" aria-hidden="true"></span> Working…'; }
  notice("info", "Working…");
  const result = await ask(op, values, (text) => {
    steps.push(text);
    $("notice").innerHTML = '<div class="banner info" role="status"><span class="spin" aria-hidden="true"></span><div class="grow"><b>Working…</b><ul class="steps">' + steps.map((step, index) => '<li class="' + (index === steps.length - 1 ? "now" : "done") + '">' + esc(step) + "</li>").join("") + "</ul></div></div>";
  });
  if (button) { button.disabled = false; button.innerHTML = label; }
  notice(result.ok ? "ok" : result.cancelled ? "info" : "bad", result.message, result.problems);
  for (const [name, text] of Object.entries(result.fields || {})) fieldError(name, text);
  if (result.ok && success) success(result);
  return result;
}
// Tabs: buttons with data-tab="name" show the panel with id "tab-name"
function selectTab(name) {
  for (const button of document.querySelectorAll(".tabs [data-tab]")) button.setAttribute("aria-selected", String(button.dataset.tab === name));
  for (const panel of document.querySelectorAll("[data-panel]")) panel.hidden = panel.dataset.panel !== name;
}
document.addEventListener("click", (event) => { const tab = event.target.closest(".tabs [data-tab]"); if (tab) selectTab(tab.dataset.tab); });
// A field's problem under it (or cleared)
function fieldError(name, text) {
  const field = document.querySelector('[data-field="' + name + '"]');
  if (!field) return;
  field.classList.toggle("error", Boolean(text));
  let line = field.querySelector(".error-text");
  if (!line) { line = document.createElement("div"); line.className = "error-text"; field.appendChild(line); }
  line.textContent = text || "";
}
function clearErrors(form) { for (const field of form.querySelectorAll(".field.error")) fieldError(field.dataset.field, ""); }

// ---------------------------------------------------------------- the landscape graph
// Zones and Flows with what hangs on them; it pans (drag), zooms (wheel), and picks a node (click)
function Graph(host, onPick) {
  const W = 172, H = 42;
  host.innerHTML = '<svg role="application" aria-label="Zones, flows, and what is attached to them"><defs><marker id="arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 0L10 5L0 10z" fill="context-stroke"/></marker></defs><g class="viewport"><g class="edges"></g><g class="nodes"></g></g></svg>' +
    '<div class="placeholder" hidden></div><div class="legend"><span><i style="background:var(--zone)"></i>Zone</span><span><i style="background:var(--flow)"></i>Flow</span><span><i style="background:var(--ok)"></i>' + esc(host.dataset.item || "Attached") + '</span></div>' +
    '<div class="zoom"><button data-z="in" title="Zoom in" aria-label="Zoom in">+</button><button data-z="out" title="Zoom out" aria-label="Zoom out">−</button><button data-z="fit" title="Fit" aria-label="Fit the graph">⤢</button></div>';
  const svg = host.querySelector("svg"), viewport = host.querySelector(".viewport"), placeholder = host.querySelector(".placeholder");
  let model = { nodes: [], edges: [] }, pos = {}, view = { scale: 1, x: 20, y: 20 }, picked = null, gesture = null, fitted = false;
  const tone = (node) => node.kind === "zone" ? "var(--zone)" : node.kind === "flow" ? "var(--flow)" : node.tone === "bad" ? "var(--bad)" : node.tone === "attention" ? "var(--warn)" : node.tone === "busy" ? "var(--busy)" : node.tone === "off" ? "var(--off)" : "var(--ok)";
  const glyph = (node) => node.kind === "zone" ? "▦ " : node.kind === "flow" ? "⇢ " : (host.dataset.glyph || "◆") + " ";
  const apply = () => viewport.setAttribute("transform", "translate(" + view.x + " " + view.y + ") scale(" + view.scale + ")");
  function fit() {
    const ids = Object.keys(pos);
    if (!ids.length) return;
    const box = host.getBoundingClientRect();
    const xs = ids.map((id) => pos[id].x), ys = ids.map((id) => pos[id].y);
    const left = Math.min(...xs) - 20, top = Math.min(...ys) - 20, width = Math.max(...xs) + W + 20 - left, height = Math.max(...ys) + H + 20 - top;
    view.scale = Math.max(0.3, Math.min(1.2, Math.min(box.width / width, box.height / height)));
    view.x = (box.width - width * view.scale) / 2 - left * view.scale;
    view.y = (box.height - height * view.scale) / 2 - top * view.scale;
    apply();
  }
  function curve(a, b) { const bend = Math.max(36, Math.abs(b.x - a.x) / 2); return "M" + a.x + " " + a.y + " C" + (a.x + bend) + " " + a.y + " " + (b.x - bend) + " " + b.y + " " + b.x + " " + b.y; }
  function draw() {
    placeholder.hidden = model.nodes.length > 0;
    if (!model.nodes.length) placeholder.innerHTML = host.dataset.emptyText || "<div><b>No zones or flows yet</b>Select code and press Ctrl+Alt+F to make a zone.</div>";
    host.querySelector(".edges").innerHTML = model.edges.filter((edge) => pos[edge.from] && pos[edge.to]).map((edge) =>
      '<path class="edge ' + edge.kind + '" d="' + curve({ x: pos[edge.from].x + W, y: pos[edge.from].y + H / 2 }, { x: pos[edge.to].x, y: pos[edge.to].y + H / 2 }) + '" marker-end="url(#arrow)"></path>').join("");
    host.querySelector(".nodes").innerHTML = model.nodes.map((node) => {
      const at = pos[node.id];
      return '<g class="node ' + node.kind + (node.missing ? " missing" : "") + (picked === node.id ? " picked" : "") + '" tabindex="0" role="button" data-id="' + esc(node.id) + '" aria-label="' + esc(node.kind === "item" ? node.name : node.kind + " " + node.name) + '" transform="translate(' + at.x + " " + at.y + ')">' +
        '<title>' + esc(node.name + " · " + node.sub) + '</title><rect class="box" width="' + W + '" height="' + H + '" rx="8"></rect><rect x="0" y="6" width="3.5" height="' + (H - 12) + '" rx="1.75" fill="' + tone(node) + '"></rect>' +
        '<text x="13" y="18">' + esc(glyph(node) + clip(node.name, 20)) + '</text><text class="sub" x="13" y="33">' + esc(clip(node.sub, 28)) + "</text></g>";
    }).join("");
    apply();
  }
  function set(next) {
    model = next || { nodes: [], edges: [] };
    const placed = layout(model.nodes, model.edges, W + 56, H + 18).placed;
    pos = {};
    for (const node of model.nodes) pos[node.id] = { x: placed[node.id].x, y: placed[node.id].y };
    if (picked && !pos[picked]) picked = null;
    draw();
    if (!fitted && model.nodes.length) { fitted = true; requestAnimationFrame(fit); }
  }
  function pick(id, silent) {
    picked = id && pos[id] ? id : null;
    draw();
    if (!silent) onPick(picked ? model.nodes.find((node) => node.id === picked) : null);
  }
  svg.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    const group = event.target.closest(".node");
    gesture = { x: event.clientX, y: event.clientY, vx: view.x, vy: view.y, id: group ? group.dataset.id : null, moved: false };
    svg.setPointerCapture(event.pointerId);
  });
  svg.addEventListener("pointermove", (event) => {
    if (!gesture) return;
    if (Math.abs(event.clientX - gesture.x) + Math.abs(event.clientY - gesture.y) > 4) { gesture.moved = true; svg.classList.add("panning"); }
    if (gesture.moved) { view.x = gesture.vx + event.clientX - gesture.x; view.y = gesture.vy + event.clientY - gesture.y; apply(); }
  });
  svg.addEventListener("pointerup", () => { const done = gesture; gesture = null; svg.classList.remove("panning"); if (done && !done.moved) pick(done.id); });
  svg.addEventListener("pointercancel", () => { gesture = null; svg.classList.remove("panning"); });
  svg.addEventListener("keydown", (event) => { const group = event.target.closest(".node"); if (group && (event.key === "Enter" || event.key === " ")) { event.preventDefault(); pick(group.dataset.id); } });
  svg.addEventListener("wheel", (event) => {
    event.preventDefault();
    const box = host.getBoundingClientRect(), px = event.clientX - box.left, py = event.clientY - box.top;
    const scale = Math.max(0.25, Math.min(2.5, view.scale * Math.exp(-event.deltaY * 0.0015)));
    view.x = px - ((px - view.x) * scale) / view.scale; view.y = py - ((py - view.y) * scale) / view.scale; view.scale = scale; apply();
  }, { passive: false });
  host.querySelector(".zoom").addEventListener("click", (event) => {
    const button = event.target.closest("[data-z]");
    if (!button) return;
    if (button.dataset.z === "fit") return fit();
    const factor = button.dataset.z === "in" ? 1.2 : 1 / 1.2, box = host.getBoundingClientRect(), px = box.width / 2, py = box.height / 2;
    const scale = Math.max(0.25, Math.min(2.5, view.scale * factor));
    view.x = px - ((px - view.x) * scale) / view.scale; view.y = py - ((py - view.y) * scale) / view.scale; view.scale = scale; apply();
  });
  return { set, pick, fit, picked: () => picked };
}

// ---------------------------------------------------------------- dialogs the extension asks for
// confirm, reason, input (one line, validated by the extension), text (several lines), and pick
// (one or several of a list, with a search box)
const NEWLINE = String.fromCharCode(10);
function dialog(message) {
  const back = document.createElement("div");
  back.className = "modal-back";
  const lines = String(message.detail || "").split(NEWLINE).map((line) => {
    if (!line.trim()) return "";
    if (line.startsWith("  ")) return '<span class="sub">' + esc(line.trim()) + "</span>";
    if (/^Crane signs|^Each move|^It stays idle/.test(line)) return '<span class="note">' + esc(line) + "</span>";
    return esc(line);
  }).join(NEWLINE);
  const kind = message.kind;
  const items = message.items || [];
  let field = "";
  if (kind === "reason" || kind === "input") {
    field = '<div class="field' + (message.error ? " error" : "") + '" style="margin:' + (lines ? "10px" : "0") + ' 0 0">' + (message.prompt || kind === "reason" ? '<label for="m-input">' + esc(message.prompt || "Reason") + "</label>" : "") +
      '<input type="text" id="m-input" spellcheck="false" placeholder="' + esc(message.placeholder || (kind === "reason" ? "Recorded in the signed audit" : "")) + '" value="' + esc(message.value || "") + '">' +
      (message.error ? '<div class="error-text">' + esc(message.error) + "</div>" : '<div class="help">' + (kind === "reason" ? "Required. It is kept with the change." : esc(message.help || "")) + "</div>") + "</div>";
  } else if (kind === "text") {
    field = '<div class="field" style="margin:' + (lines ? "10px" : "0") + ' 0 0">' + (message.prompt ? '<label for="m-text">' + esc(message.prompt) + "</label>" : "") + '<textarea id="m-text" style="min-height:180px">' + esc(message.value || "") + "</textarea>" + (message.help ? '<div class="help">' + esc(message.help) + "</div>" : "") + "</div>";
  } else if (kind === "pick") {
    field = (items.length > 7 ? '<input type="search" id="m-search" placeholder="Search…" aria-label="Search the list" style="margin:' + (lines ? "10px" : "0") + ' 0 8px">' : "") +
      '<div class="m-list" role="listbox" aria-multiselectable="' + Boolean(message.many) + '">' + items.map((item, index) =>
        '<label class="m-item" data-index="' + index + '"><input type="' + (message.many ? "checkbox" : "radio") + '" name="m-pick" value="' + index + '"' + (item.picked || (!message.many && index === 0) ? " checked" : "") + '><span class="grow"><b>' + esc(item.label) + "</b>" + (item.description ? ' <span class="muted small">' + esc(item.description) + "</span>" : "") + (item.detail ? "<small>" + esc(item.detail) + "</small>" : "") + "</span></label>").join("") +
      (items.length ? "" : '<div class="muted">Nothing to choose from.</div>') + "</div>";
  }
  const tone = message.tone === "danger" ? "danger" : "";
  back.innerHTML = '<div class="modal ' + tone + '" role="dialog" aria-modal="true" aria-labelledby="m-title"><div class="m-head"><span class="mark" aria-hidden="true">' + (tone ? "!" : "▦") + '</span><b id="m-title" class="grow">' + esc(message.title.charAt(0).toUpperCase() + message.title.slice(1)) + "</b></div>" +
    '<div class="m-body">' + (lines ? '<p class="m-detail">' + lines + "</p>" : "") + field + "</div>" +
    '<div class="m-foot"><button class="btn" data-m="no">Cancel</button><button class="btn ' + (tone ? "danger-fill" : "primary") + '" data-m="yes"' + (kind === "pick" && !items.length ? " disabled" : "") + ">" + esc(message.button || "OK") + "</button></div></div>";
  document.body.appendChild(back);
  const input = back.querySelector("#m-input"), area = back.querySelector("#m-text"), search = back.querySelector("#m-search");
  const answer = (ok) => {
    if (ok && kind === "reason" && !input.value.trim()) { input.closest(".field").classList.add("error"); input.focus(); return; }
    const picked = [...back.querySelectorAll('input[name="m-pick"]:checked')].map((box) => Number(box.value));
    if (ok && kind === "pick" && !picked.length) return;
    back.remove();
    vscode.postMessage({ type: "dialogResult", dialog: message.id, ok, value: input ? input.value : area ? area.value : undefined, picked });
  };
  back.addEventListener("click", (event) => { const button = event.target.closest("[data-m]"); if (button) answer(button.dataset.m === "yes"); else if (event.target === back) answer(false); });
  back.addEventListener("dblclick", (event) => { if (kind === "pick" && !message.many && event.target.closest(".m-item")) answer(true); });
  if (search) search.addEventListener("input", () => {
    const query = search.value.trim().toLowerCase();
    for (const row of back.querySelectorAll(".m-item")) row.hidden = Boolean(query) && !row.textContent.toLowerCase().includes(query);
  });
  back.addEventListener("keydown", (event) => {
    if (event.key === "Escape") { event.preventDefault(); answer(false); }
    else if (event.key === "Enter" && (event.target === input || event.target === search || (kind === "pick" && event.target.closest(".m-item")))) {
      event.preventDefault();
      if (event.target === search) { const first = [...back.querySelectorAll(".m-item")].find((row) => !row.hidden); if (first && !message.many) first.querySelector("input").checked = true; }
      answer(true);
    }
    else if (event.key === "Enter" && (event.ctrlKey || event.metaKey) && event.target === area) { event.preventDefault(); answer(true); }
    else if (event.key === "Tab") { const all = [...back.querySelectorAll("input, textarea, button")].filter((item) => !item.disabled && !item.closest("[hidden]")); const at = all.indexOf(document.activeElement); if (event.shiftKey && at <= 0) { event.preventDefault(); all[all.length - 1].focus(); } else if (!event.shiftKey && at === all.length - 1) { event.preventDefault(); all[0].focus(); } }
  });
  const first = input || area || search || back.querySelector('input[name="m-pick"]:checked') || back.querySelector('[data-m="yes"]');
  first.focus();
  if (input) input.select();
}

window.addEventListener("message", (event) => {
  const message = event.data;
  if (message.type === "dialog") { dialog(message); return; }
  if (message.type === "progress" && waiting.has(message.request)) { const entry = waiting.get(message.request); if (entry.onProgress) entry.onProgress(message.text); }
  else if (message.type === "result" && waiting.has(message.request)) { const entry = waiting.get(message.request); waiting.delete(message.request); entry.resolve(message.result); }
  else if (message.type === "state" && typeof onState === "function") onState(message.state || {});
  else if (message.type === "focus" && typeof onFocus === "function") onFocus(message.id);
});
`;
