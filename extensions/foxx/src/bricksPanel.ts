// The Bricks panel: the repository's Zones and Flows as a graph with the Bricks attached to each,
// and one place to create a Brick, attach it to a Zone or Flow, and change its two contexts: the
// Common Context every attachment shares (a change starts the Brick's next version) and the
// volatile context of one attachment (Crane's Current Context; it changes in place). Crane is the
// authority: every signing step is previewed with '--dry-run' and signed only after the person
// confirms, and whatever Crane refuses is shown in plain words with its own reasons.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { AttachmentRow, BRICK_GUIDE, BRICK_TEMPLATES, BRICK_TYPES, BrickList, BrickRow, draftFile, InboxRequest, inboxSubject, LEGACY_TYPES, MockForm, mockCall, mockPreset, SnapshotData, succeeded, TypeInfo } from "./core/bricks";
import { ContextFormat, contextFromFile, friendlyError, OverlayItem, overlayGraph, problemLines } from "./core/workbench";
import { call, CANCELLED, Dialogs, OpResult, PanelDeps, refused, scratch, signed, WorkbenchHost, WorkbenchPage } from "./workbench";

/** The panel's view type. */
export const BRICKS_PANEL = "foxx.bricksPanel";

/** The section under the Bricks view with the button that opens the Bricks panel (a tree view
 * cannot show a styled button). */
export class BricksLauncher implements vscode.WebviewViewProvider {
  static readonly id = "foxx.bricksLauncher";

  /**
   * Draw the button.
   * @param view the webview view
   */
  resolveWebviewView(view: vscode.WebviewView): void {
    const nonce = Array.from({ length: 32 }, () => "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[Math.floor(Math.random() * 62)]).join("");
    view.webview.options = { enableScripts: true };
    view.webview.html = launcherHtml(view.webview.cspSource, nonce);
    view.webview.onDidReceiveMessage((message: { type: string }) => {
      if (message.type === "open") void vscode.commands.executeCommand("foxx.bricks.openPanel");
    });
  }
}

/**
 * Build the launcher's page: one button in the fox colors and a line on what it opens.
 * @param cspSource the webview's source for styles
 * @param nonce the script's nonce
 * @returns HTML
 */
export function launcherHtml(cspSource: string, nonce: string): string {
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
body { margin: 0; padding: 10px 12px 12px; color: var(--vscode-foreground); background: transparent; font: var(--vscode-font-size) / 1.45 var(--vscode-font-family); }
button { width: 100%; display: flex; align-items: center; justify-content: center; gap: 8px; padding: 7px 12px; border: 0; border-radius: 7px; cursor: pointer; font: inherit; font-weight: 700; color: #1a0d05; background: linear-gradient(90deg, #ff7a1a, #ffb347); box-shadow: 0 2px 10px rgba(255, 122, 26, .3); }
button:hover { filter: brightness(1.07); }
button:focus-visible { outline: 2px solid #ffb347; outline-offset: 2px; }
p { margin: 7px 0 0; font-size: 11.5px; color: var(--vscode-descriptionForeground); text-align: center; }
</style>
</head>
<body>
<button id="open" title="Open the Bricks panel beside the editor"><span aria-hidden="true">▦</span>Open Bricks Panel</button>
<p>The graph of Zones and Flows with their Bricks: create, attach, and edit context.</p>
<script nonce="${nonce}">
"use strict";
const vscode = acquireVsCodeApi();
document.getElementById("open").addEventListener("click", () => vscode.postMessage({ type: "open" }));
</script>
</body>
</html>`;
}

/**
 * Read a text value the page sent.
 * @param value value
 * @returns text ("" when absent)
 */
const text = (value: unknown): string => (typeof value === "string" ? value : "");

/**
 * The extension side of the Bricks panel.
 * @param deps the rest of Landscape
 * @returns host
 */
export function bricksHost(deps: PanelDeps): WorkbenchHost {
  let types: TypeInfo[] = [];
  let list: BrickList | undefined;
  /** The changes requested on DataSea, asked of the service at most once a minute */
  let inbox: { requests: InboxRequest[]; error?: string; at: number } = { requests: [], at: 0 };

  /**
   * The Brick changes people requested on DataSea for this repository.
   * @param refresh ask the service again now
   */
  async function loadInbox(refresh: boolean): Promise<void> {
    if (!refresh && Date.now() - inbox.at < 60_000) return;
    const asked = await call<{ requests: InboxRequest[] }>(deps, ["brick", "inbox"]);
    inbox = { requests: asked.envelope?.data?.requests ?? [], error: asked.envelope ? undefined : asked.error, at: Date.now() };
  }

  /** The newest Brick list, or why there is none. */
  async function load(): Promise<string | undefined> {
    const listed = await call<BrickList>(deps, ["brick", "list"]);
    list = listed.envelope?.data;
    if (list && !types.length) types = (await call<{ types: TypeInfo[] }>(deps, ["brick", "types"])).envelope?.data.types ?? [];
    return listed.error;
  }

  /**
   * The Brick an attachment pins.
   * @param row attachment
   * @returns row
   */
  const brickOf = (row: AttachmentRow): BrickRow | undefined => list?.bricks.find((brick) => `brick:${brick.ref}` === row.subject || brick.ref === row.subject);

  /** The panel's dialogs, for the operation running now. */
  let dialogs: Dialogs;

  return {
    async state(refresh) {
      const problem = await load();
      await loadInbox(Boolean(refresh));
      const graph = await deps.graph(refresh);
      const items: OverlayItem[] = (list?.attachments ?? []).map((row) => ({
        id: row.attachment_id,
        label: brickOf(row)?.name ?? row.subject.replace(/^(brick|stack):/, ""),
        sub: `${row.subject.replace(/^brick:/, "")} · ${row.issues.length ? "fails closed" : row.state}`,
        target: row.target,
        tone: row.issues.length ? "bad" : row.state === "enabled" ? "ok" : "off",
      }));
      return {
        problem,
        graphError: graph.error,
        graph: overlayGraph(graph.data, items),
        targets: deps.targets().map((target) => ({ key: `${target.kind}:${target.id}`, ...target })),
        bricks: list?.bricks ?? [],
        stacks: list?.stacks ?? [],
        attachments: list?.attachments ?? [],
        store: list?.store,
        types: types.length ? types : BRICK_TYPES.map((type) => ({ type, label: type, purpose: BRICK_GUIDE[type].purpose, unique: BRICK_GUIDE[type].unique, support: "checked", runtime: "", fields: [] })),
        templates: BRICK_TEMPLATES,
        guide: BRICK_GUIDE,
        legacy: LEGACY_TYPES,
        craneKinds: types.map((info) => info.type),
        inbox: inbox.requests.map((request) => ({ ...request, subject: inboxSubject(request) })),
      };
    },

    async run(op, values, progress, asked) {
      dialogs = asked;
      const result = await operate(op, values, progress);
      if (result.ok && !op.startsWith("load:") && op !== "test") await deps.refresh();
      return result;
    },
  };

  /**
   * Run one operation of the page.
   * @param op operation
   * @param values what the person entered
   * @param progress step reporter
   * @returns result
   */
  async function operate(op: string, values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    if (op === "create") return create(values, progress);
    if (op === "attach") {
      const subject = text(values.subject), target = text(values.target);
      if (!subject || !target) return { ok: false, message: "Choose a Brick and a Zone or Flow first.", fields: { ...(subject ? {} : { subject: "Choose a Brick." }), ...(target ? {} : { target: "Choose a Zone or Flow." }) } };
      return attach(subject, target, text(values.volatile), progress);
    }
    if (op === "load:common") {
      const shown = await call<{ definition: { common_context?: { text?: string } } }>(deps, ["brick", "show", text(values.ref)]);
      if (!shown.envelope) return { ok: false, message: shown.error ?? "Crane printed no result." };
      return { ok: true, message: "", data: shown.envelope.data.definition?.common_context?.text ?? "" };
    }
    if (op === "common") return commonContext(values, progress);
    if (op === "volatile") {
      const attachment = text(values.attachment);
      if (!attachment) return { ok: false, message: "Choose the attachment whose volatile context you want to change.", fields: { attachment: "Choose an attachment." } };
      const file = scratch("current-context.md", text(values.text));
      try {
        return await signed(deps, ["brick", "context", "set", attachment, "--file", file.file], "Update the volatile context", progress, dialogs);
      } finally {
        file.dispose();
      }
    }
    if (op === "detach") {
      const row = list?.attachments.find((item) => item.attachment_id === text(values.attachment));
      if (!row) return { ok: false, message: "That attachment is gone; refresh the panel." };
      const reason = await dialogs.reason(`Why detach ${row.subject} from ${row.target}?`);
      if (!reason) return CANCELLED;
      return signed(deps, ["brick", "detach", row.attachment_id, "--reason", reason], `Detach ${row.subject} from ${row.target}`, progress, dialogs);
    }
    if (op === "activate") {
      const id = text(values.id);
      const reason = await dialogs.reason(`Why activate ${id}? Activation is signed with an admin key.`);
      if (!reason) return CANCELLED;
      return signed(deps, ["brick", "activate", id, "--reason", reason], `Activate ${id}`, progress, dialogs);
    }
    if (op === "validate") {
      progress("Validating");
      const result = await call<{ validation?: { problems?: unknown } }>(deps, ["brick", "validate", text(values.id)]);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      const problems = problemLines(result.envelope.data?.validation?.problems);
      return succeeded(result.envelope.status) && !problems.length ? { ok: true, message: result.envelope.summary } : { ok: false, message: friendlyError(result.envelope.summary), problems };
    }
    if (op === "load:preview") {
      const result = await call<SnapshotData>(deps, ["brick", "context", "preview", "--targets", text(values.target)]);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      if (!result.envelope.data?.snapshot) return refused(result.envelope);
      return { ok: true, message: "", data: result.envelope.data };
    }
    if (op === "load:file") return contextFile(text(values.format) as ContextFormat);
    if (op === "load:brick") {
      const shown = await call<{ definition: Record<string, unknown> }>(deps, ["brick", "show", text(values.ref)]);
      if (!shown.envelope) return { ok: false, message: shown.error ?? "Crane printed no result." };
      return { ok: true, message: "", data: shown.envelope.data };
    }
    if (op === "load:test") {
      const shown = await call<{ definition: { type?: string; config?: Record<string, unknown> } }>(deps, ["brick", "show", text(values.ref)]);
      if (!shown.envelope) return { ok: false, message: shown.error ?? "Crane printed no result." };
      const definition = shown.envelope.data?.definition ?? {};
      return { ok: true, message: "", data: mockPreset(definition.type ?? "", definition.config ?? {}, text(values.file)) };
    }
    if (op === "test") return test(values, progress);
    if (op === "inbox:apply" || op === "inbox:dismiss") return inboxDecide(op === "inbox:apply", progress);
    if (op === "migrate") {
      progress("Rewriting former kinds as drafts");
      const migrated = await call<{ next?: string[] }>(deps, ["brick", "migrate"]);
      if (!migrated.envelope) return { ok: false, message: migrated.error ?? "Crane printed no result." };
      if (!succeeded(migrated.envelope.status)) return refused(migrated.envelope);
      const next = migrated.envelope.data?.next ?? [];
      return { ok: true, message: `${migrated.envelope.summary}${next.length ? " Activate each new draft from the Library, then move its attachments to it:" : ""}`, problems: next };
    }
    return { ok: false, message: `Landscape does not know the operation '${op}'.` };
  }

  /**
   * Test a Brick on a Zone or Flow: its configuration checks ('crane brick test'), or, with a mock
   * call, what the Brick rules would decide about that one agent call ('--call'). A draft is tested
   * as if it were activated and attached; nothing is signed, run, or counted.
   * @param values the form: ref, target, mock (send a mock call), and the mock call's fields
   * @param progress step reporter
   * @returns result whose data is Crane's envelope data with its status and summary
   */
  async function test(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const ref = text(values.ref), target = text(values.target);
    if (!ref) return { ok: false, message: "Choose the Brick to test.", fields: { brick: "Choose a Brick." } };
    const args = ["brick", "test", ref];
    if (target) args.push("--target", target);
    let file: { file: string; dispose(): void } | undefined;
    if (values.mock) {
      if (!target) return { ok: false, message: "Choose the Zone or Flow the call happens in.", fields: { target: "Choose a Zone or Flow." } };
      const built = mockCall(values.form as MockForm);
      if (!built.call) return { ok: false, message: built.problem ?? "Check the mock call.", fields: built.field ? { [built.field]: built.problem ?? "" } : {} };
      file = scratch("mock-call.json", JSON.stringify(built.call));
      args.push("--call", file.file);
    }
    progress(file ? "Deciding the mock call" : "Testing the configuration");
    try {
      const result = await call<Record<string, unknown>>(deps, args);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      if (file && !result.envelope.data?.trial) return refused(result.envelope);
      return { ok: true, message: "", data: { status: result.envelope.status, summary: result.envelope.summary, ...result.envelope.data } };
    } finally {
      file?.dispose();
    }
  }

  /**
   * Apply or dismiss Brick changes requested on DataSea: the person chooses which, Crane runs the
   * same 'crane brick' command for each (checking and, for a signed change, signing it with this
   * machine's admin key), and DataSea is told the result.
   * @param applying apply (else dismiss)
   * @param progress step reporter
   * @returns result
   */
  async function inboxDecide(applying: boolean, progress: (text: string) => void): Promise<OpResult> {
    await loadInbox(true);
    if (!inbox.requests.length) return { ok: false, message: inbox.error ? friendlyError(inbox.error) : "No change from DataSea is waiting any more." };
    const chosen = await dialogs.pickMany({
      title: applying ? "Apply changes requested on DataSea" : "Dismiss changes requested on DataSea",
      detail: applying ? "Each one runs here as the 'crane brick' command it names, with every check Crane makes. Signed changes use this machine's admin key; commit .crane afterwards so the team shares them." : "Dismissed requests are not applied; DataSea shows them as dismissed with your reason.",
      button: applying ? "Continue" : "Dismiss",
      items: inbox.requests.map((request) => ({ label: `${request.op} ${inboxSubject(request)}`, description: `@${request.requested_by} · needs ${request.role}`, detail: request.reason, picked: true, value: request.request_id })),
    });
    if (!chosen?.length) return CANCELLED;
    if (!applying) {
      const reason = await dialogs.reason(`Why dismiss ${chosen.length} request(s)? DataSea shows the reason.`);
      if (!reason) return CANCELLED;
      progress("Dismissing");
      const dismissed = await call(deps, ["brick", "inbox", "dismiss", ...chosen, "--reason", reason]);
      if (!dismissed.envelope) return { ok: false, message: dismissed.error ?? "Crane printed no result." };
      inbox.at = 0;
      return succeeded(dismissed.envelope.status) ? { ok: true, message: dismissed.envelope.summary } : refused(dismissed.envelope);
    }
    const signing = inbox.requests.filter((request) => chosen.includes(request.request_id) && request.role === "admin").length;
    if (!(await dialogs.confirm(`Apply ${chosen.length} change(s) from DataSea?`, signing ? `${signing} of them are signed changes: Crane signs them into .crane/bricks.json with your admin key.` : "They write drafts, which grant nothing until an admin activates them.", signing ? "Sign and apply" : "Apply"))) return CANCELLED;
    progress("Applying the changes from DataSea");
    const chosenRequests = inbox.requests.filter((request) => chosen.includes(request.request_id));
    const applied = await call<{ results: { request_id: string; op: string; status: string; detail: string }[] }>(deps, ["brick", "inbox", "apply", ...chosen]);
    inbox.at = 0;
    if (!applied.envelope) return { ok: false, message: applied.error ?? "Crane printed no result." };
    const results = applied.envelope.data?.results ?? [];
    deps.record("FOXX_BRICK_COMMAND", { command: applied.envelope.command, status: applied.envelope.status.toUpperCase(), statement: applied.envelope.summary });
    const lines = results.map((result) => `${result.status === "APPLIED" ? "Applied" : "Failed"}: ${result.op} (${result.request_id}) — ${friendlyError(result.detail)}`);
    // Connections set on DataSea carry no credentials: ask for the missing ones here, on this machine
    const connected = results
      .filter((result) => result.op === "integration-set" && result.status === "APPLIED")
      .map((result) => inbox.requests.find((request) => request.request_id === result.request_id) ?? chosenRequests.find((request) => request.request_id === result.request_id))
      .map((request) => (typeof request?.params?.name === "string" ? request.params.name : ""))
      .filter(Boolean);
    for (const name of [...new Set(connected)]) lines.push(await askCredentials(name));
    return succeeded(applied.envelope.status) ? { ok: true, message: applied.envelope.summary, problems: lines } : { ok: false, message: friendlyError(applied.envelope.summary), problems: lines };
  }

  /**
   * Ask, in password boxes, for the credentials a connection still needs, and give them to Crane on
   * its standard input ('crane integrate NAME --secrets-stdin'): they stay on this machine.
   * @param name integration
   * @returns one line about the outcome
   */
  async function askCredentials(name: string): Promise<string> {
    const status = await call<{ integrations: Record<string, { connected: boolean; missing_secrets: string[] }>; schemas: { name: string; label: string; secrets: { key: string; label: string }[] }[] }>(deps, ["integrate", "--status"]);
    const row = status.envelope?.data?.integrations?.[name];
    if (!row) return `Connection ${name}: its status could not be read (${status.error ?? "no answer"}).`;
    if (!row.missing_secrets.length) return `Connection ${name}: ${row.connected ? "connected" : "saved"}.`;
    const schema = status.envelope?.data?.schemas?.find((entry) => entry.name === name);
    const credentials: Record<string, string> = {};
    for (const key of row.missing_secrets) {
      const label = schema?.secrets.find((secret) => secret.key === key)?.label ?? key;
      const value = await vscode.window.showInputBox({ title: `Connect ${schema?.label ?? name}`, prompt: `${label}: it stays on this machine and is never sent to DataSea`, password: true, ignoreFocusOut: true });
      if (!value) return `Connection ${name}: settings saved; ${label} was not entered (add it later with 'crane integrate ${name} --secrets-stdin').`;
      credentials[key] = value;
    }
    const saved = await call(deps, ["integrate", name, "--secrets-stdin"], JSON.stringify(credentials));
    if (!saved.envelope) return `Connection ${name}: the credentials were refused: ${saved.error ?? "no answer"}`;
    return `Connection ${name}: ${friendlyError(saved.envelope.summary)}`;
  }

  /**
   * Let the person choose a text, YAML, or JSON file and read it as volatile context.
   * @param format how to read it
   * @returns the text and the file's name, for the page to show before anything is saved
   */
  async function contextFile(format: ContextFormat): Promise<OpResult> {
    const filters: Record<ContextFormat, Record<string, string[]>> = { text: { "Text files": ["txt", "md", "markdown", "text"] }, yaml: { "YAML files": ["yaml", "yml"] }, json: { "JSON files": ["json"] } };
    const picked = await vscode.window.showOpenDialog({ canSelectMany: false, openLabel: "Use as volatile context", filters: filters[format] ?? filters.text, defaultUri: vscode.workspace.workspaceFolders?.[0]?.uri });
    if (!picked?.[0]) return CANCELLED;
    const file = picked[0].fsPath;
    const name = path.basename(file);
    try {
      if (fs.statSync(file).size > 1024 * 1024) return { ok: false, message: `${name} is larger than 1 MB; volatile context holds at most 16 KB.` };
      const read = contextFromFile(name, format, fs.readFileSync(file, "utf8"));
      return read.error ? { ok: false, message: read.error } : { ok: true, message: "", data: { text: read.text, name } };
    } catch (error) {
      return { ok: false, message: `Landscape could not read ${name}: ${(error as Error).message}` };
    }
  }

  /**
   * Attach an activated Brick to a target with an optional volatile context.
   * @param subject "ID@N" or "stack:ID@N"
   * @param target "zone:ID" or "flow:ID"
   * @param volatile the attachment's volatile context
   * @param progress step reporter
   * @returns result
   */
  async function attach(subject: string, target: string, volatile: string, progress: (text: string) => void): Promise<OpResult> {
    const args = ["brick", "attach", subject.replace(/^brick:/, ""), "--target", target];
    const file = volatile.trim() ? scratch("current-context.md", volatile) : undefined;
    if (file) args.push("--context-file", file.file);
    try {
      return await signed(deps, args, `Attach ${subject} to ${target}`, progress, dialogs);
    } finally {
      file?.dispose();
    }
  }

  /**
   * Create a draft, and when asked, activate it and attach it.
   * @param values the form
   * @param progress step reporter
   * @returns result
   */
  async function create(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const type = text(values.type) as BrickRow["type"];
    let config: Record<string, unknown>;
    try {
      config = JSON.parse(text(values.config) || "{}") as Record<string, unknown>;
    } catch (error) {
      return { ok: false, message: "The configuration is not valid JSON. Check brackets, quotes, and commas.", fields: { config: (error as Error).message } };
    }
    progress("Saving the draft");
    const file = scratch("draft.json", draftFile({ type, name: text(values.name), intent: text(values.intent), context: text(values.common), config }));
    let created;
    try {
      created = await call<{ validation?: { problems?: unknown } }>(deps, ["brick", "create", "--file", file.file]);
    } finally {
      file.dispose();
    }
    if (!created.envelope) return { ok: false, message: created.error ?? "Crane printed no result." };
    if (!succeeded(created.envelope.status)) return refused(created.envelope, created.envelope.data?.validation?.problems);
    const problems = problemLines(created.envelope.data?.validation?.problems);
    const reference = created.envelope.refs.brick_ref ?? "";
    if (problems.length) return { ok: false, message: `Saved the draft ${reference}, but it does not validate yet. Fix these in the Library (Edit), then activate it.`, problems };
    if (!values.now) return { ok: true, message: `Created the draft ${reference}. Activate it from the Library to attach it.` };
    const id = reference.split("@")[0];
    const activated = await signed(deps, ["brick", "activate", id, "--reason", text(values.reason)], `Activate ${reference}`, progress, dialogs);
    if (!activated.ok) return { ...activated, message: `Created the draft ${reference}; activating it did not finish. ${activated.message}` };
    const target = text(values.target);
    if (!target) return { ok: true, message: `Created and activated ${reference}.` };
    const attached = await attach(reference, target, text(values.volatile), progress);
    return attached.ok ? { ok: true, message: `Created ${reference}, activated it, and attached it to ${target}.` } : { ...attached, message: `Created and activated ${reference}; attaching it to ${target} did not finish. ${attached.message}` };
  }

  /**
   * Save a Brick's Common Context. On an activated Brick it starts the next version's draft; when
   * asked, that version is activated and the Brick's attachments are moved to it.
   * @param values the form
   * @param progress step reporter
   * @returns result
   */
  async function commonContext(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const brick = list?.bricks.find((row) => row.ref === text(values.ref) || (row.state === "draft" && row.brick_id === text(values.ref)));
    if (!brick) return { ok: false, message: "That Brick is gone; refresh the panel.", fields: { brick: "Choose a Brick." } };
    progress("Saving the common context");
    const file = scratch("context.md", text(values.text));
    let edited;
    try {
      edited = await call(deps, ["brick", "edit", brick.brick_id, "--context-file", file.file]);
    } finally {
      file.dispose();
    }
    if (!edited.envelope) return { ok: false, message: edited.error ?? "Crane printed no result." };
    if (!succeeded(edited.envelope.status)) return refused(edited.envelope);
    const reference = edited.envelope.refs.brick_ref ?? brick.brick_id;
    if (brick.state === "draft") return { ok: true, message: `Saved the common context of the draft ${reference}.` };
    if (!values.promote) return { ok: true, message: `Saved as ${reference}, a new draft version: ${brick.ref} stays in force until ${reference} is activated and its attachments are moved to it.` };
    const activated = await signed(deps, ["brick", "activate", brick.brick_id, "--reason", text(values.reason)], `Activate ${reference}`, progress, dialogs);
    if (!activated.ok) return { ...activated, message: `Saved ${reference} as a draft; activating it did not finish. ${activated.message}` };
    const moving = (list?.attachments ?? []).filter((row) => row.subject.replace(/^brick:/, "").split("@")[0] === brick.brick_id && !row.subject.endsWith(`@${reference.split("@")[1]}`));
    if (!moving.length) return { ok: true, message: `Activated ${reference}; nothing was attached to move.` };
    if (!(await dialogs.confirm(`move ${moving.length} attachment(s) to ${reference}?`, moving.map((row) => `${row.target}: ${row.subject} → ${reference}`).join("\n") + "\n\nEach move is signed; volatile contexts stay as they are.", "Move them"))) return { ok: true, message: `Activated ${reference}; the attachments still use the older version.` };
    const failed: string[] = [];
    for (const row of moving) {
      progress(`Moving ${row.target} to ${reference}`);
      const moved = await call(deps, ["brick", "upgrade", row.attachment_id, "--to", reference, "--reason", text(values.reason)]);
      if (!moved.envelope || !succeeded(moved.envelope.status)) failed.push(`${row.target}: ${moved.error ?? friendlyError(moved.envelope?.summary ?? "")}`);
    }
    return failed.length ? { ok: false, message: `Activated ${reference}, but ${failed.length} attachment(s) could not move.`, problems: failed } : { ok: true, message: `Activated ${reference} and moved ${moving.length} attachment(s) to it.` };
  }
}

/** The Bricks panel page. */
export const BRICKS_PAGE: WorkbenchPage = {
  viewType: BRICKS_PANEL,
  title: "Landscape Bricks",
  styles: `
.create-grid { display: grid; grid-template-columns: minmax(0, 1.25fr) minmax(260px, 1fr); grid-template-areas: "form guide"; gap: 16px; align-items: start; }
.create-grid > form { grid-area: form; }
.guide { grid-area: guide; position: sticky; top: 64px; border: 1px solid color-mix(in srgb, var(--fox) 40%, transparent); border-radius: var(--radius); background: var(--bg); overflow: hidden; }
@media (max-width: 820px) { .create-grid { grid-template-columns: 1fr; grid-template-areas: "guide" "form"; } .guide { position: static; } }
.guide .top { display: flex; align-items: center; gap: 8px; padding: 10px 12px; background: var(--fox-soft); border-bottom: 1px solid var(--line); }
.guide .top b { font-size: 13.5px; text-transform: capitalize; }
.guide .part { padding: 9px 12px; border-bottom: 1px solid var(--line); }
.guide .part:last-child { border-bottom: 0; }
.guide h4 { margin: 0 0 3px; font-size: 10.5px; letter-spacing: .8px; text-transform: uppercase; color: var(--muted); }
.guide p { margin: 0; }
.guide .story { padding: 7px 9px; border-left: 3px solid var(--fox); background: var(--raised); border-radius: 0 6px 6px 0; }
.guide pre { margin: 6px 0 0; padding: 7px 9px; border-radius: 6px; background: var(--raised); font-family: var(--mono); font-size: 11.5px; white-space: pre-wrap; overflow-wrap: anywhere; }
.guide dl { margin: 0; display: grid; gap: 7px; }
.guide dt { font-family: var(--mono); font-size: 12px; font-weight: 700; }
.guide dd { margin: 1px 0 0; }
.guide dd small { display: block; color: var(--muted); font-size: 11px; }
.ctx { margin-bottom: 7px; border: 1px solid var(--line); border-left: 3px solid var(--tone, var(--line)); border-radius: 6px; background: var(--bg); }
.ctx.mine { box-shadow: 0 0 0 2px var(--fox-soft); border-color: var(--fox); border-left-color: var(--tone); }
.ctx summary { display: flex; gap: 8px; align-items: center; padding: 6px 9px; cursor: pointer; }
.ctx summary b { flex: 1; min-width: 0; font-size: 12.5px; overflow-wrap: anywhere; }
.ctx pre { margin: 0; padding: 7px 10px 9px; border-top: 1px solid var(--line); max-height: 200px; overflow: auto; white-space: pre-wrap; overflow-wrap: anywhere; font-family: var(--mono); font-size: 11.5px; }
.legend-row { display: flex; gap: 10px; flex-wrap: wrap; margin: 2px 0 10px; font-size: 11px; color: var(--muted); }
.legend-row i { display: inline-block; width: 9px; height: 9px; border-radius: 3px; margin-right: 4px; vertical-align: -1px; }
.sources { display: flex; gap: 5px; flex-wrap: wrap; align-items: center; margin-bottom: 6px; }
.loaded { margin: -2px 0 6px; padding: 3px 8px; border-radius: 6px; background: var(--fox-soft); color: var(--fg); }
.guide .part.unique, .info-line.unique { background: color-mix(in srgb, var(--fox) 7%, transparent); }
.guide .part.unique h4, .info-line.unique h4 { color: var(--fox); }
.info-btn { display: inline-grid; place-items: center; width: 18px; height: 18px; padding: 0; border-radius: 50%; border: 1px solid color-mix(in srgb, var(--fox) 55%, var(--line)); background: var(--bg); color: var(--fox); font: italic 700 11px/1 Georgia, serif; cursor: pointer; flex: none; }
.info-btn:hover, .info-btn:focus-visible { background: var(--fox); color: #1a0d05; }
.info-modal { width: min(640px, 100%); }
.guide .settings-row { display: flex; align-items: center; gap: 6px; padding: 5px 12px; }
.guide .settings-row span { font-size: 10px; letter-spacing: .8px; text-transform: uppercase; color: var(--muted); }
.info-btn.tiny { width: 15px; height: 15px; font-size: 9.5px; }
.info-modal .m-head .mark { font: italic 700 14px/1 Georgia, serif; }
.info-line { margin: 0 0 8px; padding: 7px 10px; border-radius: 7px; border: 1px solid var(--line); }
.info-line h4, .props-title { margin: 0 0 3px; font-size: 10.5px; letter-spacing: .8px; text-transform: uppercase; color: var(--muted); }
.info-line p { margin: 0; }
.props-title { margin: 12px 0 6px; }
.props { display: grid; gap: 6px; }
.prop { display: grid; grid-template-columns: minmax(110px, .8fr) minmax(0, 1fr); gap: 2px 12px; padding: 7px 9px; border: 1px solid var(--line); border-radius: 7px; background: var(--raised); }
.prop-name { font-family: var(--mono); font-size: 12px; font-weight: 700; overflow-wrap: anywhere; }
.prop-value pre, .context-text { margin: 0; padding: 5px 7px; border-radius: 5px; background: var(--bg); font-family: var(--mono); font-size: 11.5px; white-space: pre-wrap; overflow-wrap: anywhere; max-height: 180px; overflow: auto; }
.prop-value code { font-size: 12px; }
.prop-value .sub-key { margin-top: 3px; font-size: 11px; color: var(--muted); }
.prop-help { grid-column: 1 / -1; color: var(--muted); font-size: 11.5px; }
.prop-help small { display: block; font-size: 11px; }
@media (max-width: 520px) { .prop { grid-template-columns: 1fr; } }
.mock { margin: 0; padding: 10px 12px 12px; border: 1px solid var(--line); border-radius: var(--radius); }
.mock legend { padding: 0 6px; font-weight: 700; }
.grid3, .grid4 { display: grid; gap: 0 12px; align-items: end; }
.grid3 { grid-template-columns: repeat(3, minmax(0, 1fr)); }
.grid4 { grid-template-columns: repeat(4, minmax(0, 1fr)); }
@media (max-width: 720px) { .grid3, .grid4 { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
.verdict { display: flex; gap: 10px; align-items: center; margin: 14px 0 8px; padding: 10px 12px; border-radius: var(--radius); border: 1px solid var(--line); border-left: 4px solid var(--tone); }
.verdict b { font-size: 14px; }
.cases { display: grid; gap: 5px; margin-bottom: 8px; }
.case { display: grid; grid-template-columns: auto minmax(0, 1fr); gap: 2px 10px; padding: 6px 9px; border: 1px solid var(--line); border-radius: 6px; background: var(--raised); }
.case .what { font-weight: 700; overflow-wrap: anywhere; }
.case .why { grid-column: 2; color: var(--muted); font-size: 11.5px; overflow-wrap: anywhere; }
`,
  body: `<div class="shell">
<header class="top"><span class="mark" aria-hidden="true">▦</span><div class="grow"><b>Bricks</b><div class="muted small" id="summary">Loading…</div></div><button class="btn" id="refresh" title="Ask Crane again">Refresh</button></header>
<div id="notice" aria-live="polite"></div>
<div id="problem"></div>
<div class="layout">
  <section class="card"><div class="head"><b class="grow">Landscape</b><span class="muted small">Click a Zone or Flow to work on it</span></div><div class="graph" id="graph" data-item="Brick" data-glyph="◆"></div></section>
  <section class="card"><div class="head"><b class="grow" id="selected-title">What agents receive</b></div><div class="body" id="selected"></div></section>
</div>
<section class="card">
  <div class="tabs" role="tablist">
    <button data-tab="create" role="tab" aria-selected="true">Create a Brick</button>
    <button data-tab="attach" role="tab">Attach</button>
    <button data-tab="context" role="tab">Edit context</button>
    <button data-tab="test" role="tab">Test</button>
    <button data-tab="library" role="tab">Library<span class="count" id="count-library"></span></button>
  </div>
  <div class="body">
    <div data-panel="create" class="create-grid">
    <aside class="guide" id="guide" aria-live="polite" aria-label="About this kind of Brick"></aside>
    <form id="create-form" novalidate>
      <div class="grid2">
        <div class="field" data-field="type"><label for="c-type">Kind of Brick</label><select id="c-type"></select><div class="help">The guide explains each kind, with an example.</div></div>
        <div class="field" data-field="name"><label for="c-name">Name</label><input type="text" id="c-name" placeholder="Production config guard"></div>
      </div>
      <div class="field" data-field="intent"><label for="c-intent">What it solves</label><input type="text" id="c-intent" placeholder="Agents must not change production settings without a person"></div>
      <div class="field" data-field="common"><label for="c-common">Common context</label><textarea id="c-common" placeholder="What every agent should know wherever this Brick is attached…"></textarea><div class="help">Shared by every Zone and Flow this Brick is attached to. It is guidance for agents, never permission. Never put secrets here.</div></div>
      <details class="more"><summary>Configuration (optional)</summary><div class="field" data-field="config" style="margin-top:8px"><textarea class="code" id="c-config" spellcheck="false" aria-label="Configuration JSON"></textarea><div class="help">JSON for this kind; Crane checks it against the kind's schema.</div><div class="chips-row"><button type="button" class="chip-btn" id="c-reset"><span aria-hidden="true">↺</span>Reset to the example</button><button type="button" class="chip-btn" id="c-settings"><span aria-hidden="true">i</span>What each setting means</button></div></div></details>
      <label class="check"><input type="checkbox" id="c-now"><span class="grow">Activate and attach it now<small>Activating is signed with an admin key. Crane shows what changes before anything is signed.</small></span></label>
      <div id="c-now-fields" hidden style="margin-top:10px">
        <div class="grid2">
          <div class="field" data-field="target"><label for="c-target">Attach to</label><select id="c-target" data-targets></select><div class="help">Or click a Zone or Flow in the graph.</div></div>
          <div class="field" data-field="reason"><label for="c-reason">Why activate it?</label><input type="text" id="c-reason" placeholder="Recorded in the signed audit"></div>
        </div>
        <div class="field" data-field="volatile"><label for="c-volatile">Volatile context there <span class="muted">(optional)</span></label><div class="sources" data-into="c-volatile" role="group" aria-label="Where the volatile context comes from"><span class="muted small">Add it from:</span><button type="button" class="btn small" data-src="write">✎ Write text</button><button type="button" class="btn small" data-src="text">Text file</button><button type="button" class="btn small" data-src="yaml">YAML file</button><button type="button" class="btn small" data-src="json">JSON file</button></div><div class="loaded small" hidden></div><textarea id="c-volatile" placeholder="What is true there right now, e.g. a release freeze until Friday…"></textarea><div class="help">Only this Zone or Flow gets it. Change it any time without a new version.</div></div>
      </div>
      <div class="row" style="margin-top:12px"><button class="btn primary" id="c-submit" type="submit">Create Brick</button><span class="muted small">It starts as a draft and changes nothing until it is activated and attached.</span></div>
    </form>
    </div>

    <form data-panel="attach" id="attach-form" novalidate hidden>
      <div id="a-empty" class="empty-note" hidden>No Brick is activated yet. Create one, then activate it from the Library.</div>
      <div class="grid2">
        <div class="field" data-field="subject"><label for="a-subject">Brick</label><select id="a-subject"></select><div class="help">Only activated Bricks and Stacks can be attached.</div></div>
        <div class="field" data-field="target"><label for="a-target">Zone or Flow</label><select id="a-target" data-targets></select><div class="help">Or click one in the graph.</div></div>
      </div>
      <div class="field" data-field="volatile"><label for="a-volatile">Volatile context <span class="muted">(optional)</span></label><div class="sources" data-into="a-volatile" role="group" aria-label="Where the volatile context comes from"><span class="muted small">Add it from:</span><button type="button" class="btn small" data-src="write">✎ Write text</button><button type="button" class="btn small" data-src="text">Text file</button><button type="button" class="btn small" data-src="yaml">YAML file</button><button type="button" class="btn small" data-src="json">JSON file</button></div><div class="loaded small" hidden></div><textarea id="a-volatile" placeholder="What agents should know about this Zone or Flow right now…"></textarea><div class="help">This attachment only. It can add guidance but never relaxes a control.</div></div>
      <div class="row"><button class="btn primary" id="a-submit" type="submit">Attach</button><span class="muted small">You see what changes before Crane signs it.</span></div>
    </form>

    <div data-panel="context" hidden>
      <div class="grid2">
        <form id="common-form" novalidate>
          <h3 style="margin:0 0 4px">Common context</h3><p class="muted small" style="margin:0 0 10px">Shared by every attachment of the Brick.</p>
          <div class="field" data-field="brick"><label for="k-brick">Brick</label><select id="k-brick"></select></div>
          <div class="field" data-field="text"><label for="k-text">Text</label><textarea id="k-text" style="min-height:120px" placeholder="Choose a Brick to load its common context"></textarea></div>
          <div id="k-promote-box" hidden>
            <div class="banner info" style="margin-bottom:8px"><span class="icon">ℹ</span><div class="grow small">This Brick is activated, so saving starts its next version as a draft. The current version stays in force until the new one is activated.</div></div>
            <label class="check"><input type="checkbox" id="k-promote"><span class="grow">Then activate the new version and move every attachment to it<small>Signed with an admin key.</small></span></label>
            <div class="field" data-field="reason" id="k-reason-box" hidden style="margin-top:8px"><label for="k-reason">Why?</label><input type="text" id="k-reason" placeholder="Recorded in the signed audit"></div>
          </div>
          <div class="row" style="margin-top:10px"><button class="btn primary" id="k-submit" type="submit">Save common context</button></div>
        </form>
        <form id="volatile-form" novalidate>
          <h3 style="margin:0 0 4px">Volatile context</h3><p class="muted small" style="margin:0 0 10px">For one attachment: what is true on that Zone or Flow right now.</p>
          <div class="field" data-field="attachment"><label for="v-attachment">Attachment</label><select id="v-attachment"></select></div>
          <div class="field" data-field="text"><label for="v-text">Text</label><div class="sources" data-into="v-text" role="group" aria-label="Where the volatile context comes from"><span class="muted small">Add it from:</span><button type="button" class="btn small" data-src="write">✎ Write text</button><button type="button" class="btn small" data-src="text">Text file</button><button type="button" class="btn small" data-src="yaml">YAML file</button><button type="button" class="btn small" data-src="json">JSON file</button></div><div class="loaded small" hidden></div><textarea id="v-text" style="min-height:120px" placeholder="Choose an attachment to load its volatile context"></textarea></div>
          <div class="row" style="margin-top:10px"><button class="btn primary" id="v-submit" type="submit">Save volatile context</button><button class="btn danger" id="v-detach" type="button" title="Remove this Brick from its Zone or Flow">Detach this Brick</button><span class="muted small">Signed in place; no new version.</span></div>
        </form>
      </div>
    </div>

    <div data-panel="test" hidden>
      <form id="test-form" novalidate>
        <div class="grid2">
          <div class="field" data-field="brick"><label for="t-brick">Brick</label><select id="t-brick"></select><div class="help">A draft is tested as if it were activated and attached.</div></div>
          <div class="field" data-field="target"><label for="t-target">Zone or Flow</label><select id="t-target" data-targets></select><div class="help">Where the call happens. Or click one in the graph.</div></div>
        </div>
        <div class="row" style="margin:2px 0 12px"><button class="btn" id="t-check" type="button">Check the configuration</button><span class="muted small">What the Brick restricts on that Zone or Flow, file by file, and its checks.</span></div>
        <fieldset class="mock"><legend>Mock agent call</legend>
          <div id="t-note" class="banner info" hidden style="margin-bottom:8px"><span class="icon">ℹ</span><div class="grow small" id="t-note-text"></div></div>
          <div class="grid3">
            <div class="field" data-field="operation"><label for="t-operation">The agent</label><select id="t-operation"><option value="write">changes files</option><option value="read">reads files</option><option value="execute">runs a command</option><option value="other">does something else</option></select></div>
            <div class="field" data-field="agent"><label for="t-agent">Provider</label><input type="text" id="t-agent" placeholder="claude"><div class="help">Empty: a person, not an agent.</div></div>
            <div class="field" data-field="model"><label for="t-model">Model <span class="muted">(optional)</span></label><input type="text" id="t-model" placeholder="claude-opus-5-5"></div>
          </div>
          <div class="field" data-field="files"><label for="t-files">Files <span class="muted">(repository paths, one per line)</span></label><textarea id="t-files" class="code" style="min-height:52px" placeholder="src/api/handler.ts"></textarea></div>
          <div class="grid3" id="t-write">
            <div class="field" data-field="lines"><label for="t-lines">Lines changed per file</label><input type="text" id="t-lines" inputmode="numeric" placeholder="10"></div>
            <label class="check"><input type="checkbox" id="t-new"><span class="grow">They are new files</span></label>
            <label class="check"><input type="checkbox" id="t-delete"><span class="grow">The change deletes them</span></label>
          </div>
          <div class="grid2">
            <div class="field" data-field="command"><label for="t-command">Command</label><input type="text" id="t-command" class="code" placeholder="git push origin main"></div>
            <div class="field" data-field="hosts"><label for="t-hosts">Hosts it reaches <span class="muted">(comma-separated)</span></label><input type="text" id="t-hosts" placeholder="api.example.com"></div>
          </div>
          <details class="more"><summary>What the session did before this call</summary>
            <div class="grid4" style="margin-top:8px">
              <div class="field" data-field="s-tool_calls"><label for="s-tool_calls">Tool calls</label><input type="text" id="s-tool_calls" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-minutes"><label for="s-minutes">Minutes running</label><input type="text" id="s-minutes" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-files_changed"><label for="s-files_changed">Files changed</label><input type="text" id="s-files_changed" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-lines_changed"><label for="s-lines_changed">Lines changed</label><input type="text" id="s-lines_changed" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-commands"><label for="s-commands">Commands</label><input type="text" id="s-commands" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-network_calls"><label for="s-network_calls">Network calls</label><input type="text" id="s-network_calls" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-denials"><label for="s-denials">Denials</label><input type="text" id="s-denials" inputmode="numeric" placeholder="0"></div>
              <div class="field" data-field="s-failures_in_row"><label for="s-failures_in_row">Failures in a row</label><input type="text" id="s-failures_in_row" inputmode="numeric" placeholder="0"></div>
            </div>
            <label class="check"><input type="checkbox" id="t-watch"><span class="grow">A Watch Brick on this Zone or Flow is firing</span></label>
          </details>
          <div class="row" style="margin-top:12px"><button class="btn primary" id="t-run" type="submit">Run the mock call</button><button class="btn" id="t-preset" type="button" title="Start again from a call this kind of Brick decides">Reset to the example call</button><span class="muted small">Nothing runs; Crane decides the call with the same rules the hooks use.</span></div>
        </fieldset>
      </form>
      <div id="t-result" aria-live="polite"></div>
    </div>

    <div data-panel="library" hidden id="library"></div>
  </div>
</section>
</div>`,
  script: `
let S = { graph: { nodes: [], edges: [] }, targets: [], bricks: [], stacks: [], attachments: [], types: [], templates: {} };
let configTouched = false;
let commonLoaded = "";
const graph = Graph($("graph"), (node) => { showPreview(node); if (node && node.kind !== "item") setTarget(node.id); });
const subjectName = (subject) => { const brick = S.bricks.find((row) => "brick:" + row.ref === subject || row.ref === subject); return brick ? brick.name : subject.replace(/^(brick|stack):/, ""); };
const brickRef = (subject) => subject.replace(/^brick:/, "");
const stateTone = (state) => state === "active" ? "ok" : state === "draft" ? "warn" : state === "revoked" ? "bad" : "";
function options(select, items, placeholder) {
  const keep = select.value;
  select.innerHTML = (placeholder ? '<option value="">' + esc(placeholder) + "</option>" : "") + items.map((item) => '<option value="' + esc(item.value) + '">' + esc(item.label) + "</option>").join("");
  if (items.some((item) => item.value === keep)) select.value = keep;
}
function setTarget(key) { for (const select of document.querySelectorAll("[data-targets]")) if ([...select.options].some((option) => option.value === key)) select.value = key; }

function onState(state) {
  S = Object.assign(S, state);
  $("problem").innerHTML = state.problem ? '<div class="banner" role="alert"><span class="icon">⚠</span><div class="grow"><b>Crane could not list the Bricks</b><div>' + esc(state.problem) + "</div></div></div>" : state.graphError ? '<div class="banner info"><span class="icon">ℹ</span><div class="grow"><b>The graph is not available</b><div>' + esc(state.graphError) + "</div></div></div>" : "";
  const active = S.bricks.filter((row) => row.state === "active").length;
  $("summary").textContent = S.bricks.length + " Brick version(s) · " + active + " active · " + S.attachments.length + " attachment(s)" + (S.store ? " · signed store " + S.store.trust : "");
  $("count-library").textContent = String(S.bricks.length);
  graph.set(S.graph);
  // The kinds, with what each does and how far Crane enforces it
  const type = $("c-type");
  if (!type.options.length) {
    // The kinds this Landscape knows, in catalog order; Crane's labels where it has them
    type.innerHTML = Object.keys(S.guide || {}).map((kind) => {
      const info = S.types.find((item) => item.type === kind);
      return '<option value="' + esc(kind) + '">' + esc((info && info.label) || kind.charAt(0).toUpperCase() + kind.slice(1)) + "</option>";
    }).join("");
    applyType();
  }
  const targets = S.targets.map((target) => ({ value: target.key, label: target.key + (target.status && target.status !== "active" ? " (" + target.status + ")" : "") }));
  for (const select of document.querySelectorAll("[data-targets]")) options(select, targets, "Choose a Zone or Flow");
  const attachable = S.bricks.filter((row) => row.state === "active").map((row) => ({ value: row.ref, label: row.name + " · " + row.ref + " · " + row.type })).concat(S.stacks.filter((row) => row.state === "active").map((row) => ({ value: "stack:" + row.stack_id + "@" + row.version, label: row.name + " · Stack " + row.stack_id + "@" + row.version })));
  options($("a-subject"), attachable, "Choose a Brick");
  $("a-empty").hidden = attachable.length > 0;
  options($("k-brick"), S.bricks.filter((row) => row.state === "draft" || row.state === "active").map((row) => ({ value: row.state === "draft" ? row.brick_id : row.ref, label: row.name + " · " + (row.state === "draft" ? row.brick_id + " draft" : row.ref) })), "Choose a Brick");
  options($("v-attachment"), S.attachments.map((row) => ({ value: row.attachment_id, label: row.target + " · " + subjectName(row.subject) + " (" + brickRef(row.subject) + ")" })), "Choose an attachment");
  options($("t-brick"), S.bricks.filter((row) => row.state === "draft" || row.state === "active").map((row) => ({ value: row.state === "draft" ? row.brick_id : row.ref, label: row.name + " · " + (row.state === "draft" ? row.brick_id + " draft" : row.ref) + " · " + row.type })), "Choose a Brick");
  renderLibrary();
  const picked = graph.picked();
  // Crane's state changed: what agents receive may have changed too
  for (const key of Object.keys(previews)) delete previews[key];
  showPreview(picked ? S.graph.nodes.find((node) => node.id === picked) : null);
}
function onFocus(id) { graph.pick(id); }

// How far Crane acts on a kind today, in words and a tone
const SUPPORT = { enforced: ["ok", "Enforced at every tool call"], checked: ["busy", "Checked by tests and monitors"], delivered: ["fox", "Given to agents as context"], declared: ["warn", "Recorded; not enforced yet"] };
function renderGuide() {
  const type = $("c-type").value;
  if (!type) { $("guide").innerHTML = '<div class="part muted">Choose a kind to see what it does.</div>'; return; }
  const guide = guideOf(type);
  const info = S.types.find((item) => item.type === type) || {};
  const known = !S.craneKinds || !S.craneKinds.length || S.craneKinds.includes(type);
  const support = SUPPORT[info.support] || ["", ""];
  $("guide").innerHTML = '<div class="top"><span aria-hidden="true">◆</span><b class="grow">' + esc(info.label || type.charAt(0).toUpperCase() + type.slice(1)) + " Brick</b>" + '<button type="button" class="info-btn" data-kind-info="' + esc(type) + '" title="Its settings" aria-label="Settings of the ' + esc(type) + ' kind">i</button>' + (support[1] ? '<span class="tag ' + support[0] + '">' + esc(support[1]) + "</span>" : "") + "</div>" +
    (known ? "" : '<div class="part"><div class="banner" style="margin:0"><span class="icon">⚠</span><div class="grow small"><b>Your Crane does not know this kind yet</b>Update Crane to 0.5.8 or newer to create ' + esc(type) + " Bricks; the guide below already explains it.</div></div></div>") +
    '<div class="part"><h4>Purpose</h4><p>' + esc(guide.purpose) + "</p></div>" +
    '<div class="part unique"><h4>Only this kind</h4><p>' + esc(guide.unique) + "</p></div>" +
    (guide.useWhen ? '<div class="part"><h4>Use it when</h4><p>' + esc(guide.useWhen) + "</p></div>" : "") +
    (guide.example ? '<div class="part"><h4>Example</h4><p class="story">' + esc(guide.example.story) + "</p><pre>" + esc(JSON.stringify(guide.example.config, null, 2)) + '</pre><div class="row" style="margin-top:8px"><button type="button" class="btn small primary" id="g-use">Fill the form with this example</button></div></div>' : "") +
    '<div class="part settings-row"><span>Settings you can configure · ' + guide.settings.length + '</span><button type="button" class="info-btn tiny" data-kind-info="' + esc(type) + '" title="Settings you can configure" aria-label="Settings you can configure for the ' + esc(type) + ' kind">i</button></div>' +
    (info.runtime ? '<div class="part"><h4>What Crane does with it today</h4><p class="muted">' + esc(info.runtime[0].toUpperCase() + info.runtime.slice(1)) + ".</p></div>" : "");
}
function showKind(type) {
  selectTab("create");
  $("c-type").value = type;
  applyType();
  $("guide").scrollIntoView({ block: "nearest" });
}
document.addEventListener("click", (event) => {
  if (event.target.id === "g-use") {
    const guide = S.guide[$("c-type").value];
    $("c-name").value = guide.example.name;
    $("c-intent").value = guide.example.intent;
    $("c-config").value = JSON.stringify(guide.example.config, null, 2);
    configTouched = true;
    $("create-form").querySelector("details.more").open = true;
    notice("info", "The form now holds the example. Adjust the name, context, and configuration to your case before creating it.");
    $("c-name").focus();
  }
  const kind = event.target.closest("[data-kind]");
  if (kind) { event.preventDefault(); showKind(kind.dataset.kind); }
});
function applyType() {
  renderGuide();
  if (!configTouched) $("c-config").value = JSON.stringify(S.templates[$("c-type").value] || {}, null, 2);
}
$("c-type").addEventListener("change", applyType);
$("c-config").addEventListener("input", () => { configTouched = true; });
$("c-reset").addEventListener("click", (event) => { event.preventDefault(); configTouched = false; applyType(); });
$("c-settings").addEventListener("click", (event) => { event.preventDefault(); showKindInfo($("c-type").value); });
$("c-now").addEventListener("change", () => { $("c-now-fields").hidden = !$("c-now").checked; });
$("refresh").addEventListener("click", () => { notice(); vscode.postMessage({ type: "refresh" }); });

// ---------------------------------------------------------------- what the picked Zone or Flow receives
const previews = {};
let previewFor = null;
const SECTION = { enforcement: "var(--muted)", zone: "var(--zone)", flow: "var(--flow)", brick_common: "var(--ok)", brick_current: "var(--warn)" };
const size = (bytes) => bytes < 1024 ? bytes + " B" : (bytes / 1024).toFixed(1) + " KB";
function sectionTitle(section) {
  if (section.kind === "enforcement") return "Rules Crane enforces";
  if (section.kind === "zone" || section.kind === "flow") return (section.kind === "zone" ? "Zone" : "Flow") + " context · " + section.source.replace(/^[a-z]+:/, "");
  if (section.kind === "brick_common") { const ref = section.source.replace(/^brick:/, ""); return "Common context · " + subjectName("brick:" + ref) + " (" + ref + ")"; }
  if (section.kind === "brick_current") { const subject = section.reference.split(" on ")[0]; return "Volatile context · " + subjectName(subject) + " (" + brickRef(subject) + ")"; }
  return section.kind;
}
async function showPreview(node) {
  const box = $("selected");
  if (!node) {
    previewFor = null;
    $("selected-title").textContent = "What agents receive";
    box.innerHTML = '<div class="empty-note">Click a Zone or Flow in the graph to preview every context agents get there: its own context, and the common and volatile context of each attached Brick.</div>';
    return;
  }
  const row = node.kind === "item" ? S.attachments.find((item) => "item:" + item.attachment_id === node.id) : null;
  const target = row ? row.target : node.kind === "item" ? null : node.id;
  if (!target) return;
  previewFor = { target, row };
  $("selected-title").textContent = "What agents receive in " + (target.split(":")[1] || target);
  if (!previews[target]) {
    box.innerHTML = '<div class="muted"><span class="spin" aria-hidden="true"></span> Asking Crane what agents receive…</div>';
    const result = await ask("load:preview", { target });
    previews[target] = result.ok ? { data: result.data } : { error: result.message };
    if (!previewFor || previewFor.target !== target) return;
  }
  renderPreview(previews[target], target, previewFor.row);
}
function renderPreview(entry, target, row) {
  const box = $("selected");
  if (entry.error) { box.innerHTML = '<div class="banner"><span class="icon">⚠</span><div class="grow small"><b>Crane could not build the preview</b>' + esc(entry.error) + "</div></div>"; return; }
  const snapshot = entry.data.snapshot;
  const kind = target.split(":")[0];
  const own = snapshot.sections.filter((section) => section.kind !== "enforcement");
  const mine = (section) => row && (section.source === "attachment:" + row.attachment_id || section.source === "brick:" + brickRef(row.subject));
  const block = (section) => '<details class="ctx' + (mine(section) ? " mine" : "") + '"' + (section.kind === "enforcement" ? "" : " open") + ' style="--tone:' + (SECTION[section.kind] || "var(--line)") + '"><summary><b>' + esc(sectionTitle(section)) + "</b>" + (section.kind === "brick_common" ? infoButton(section.source.replace(/^brick:/, ""), sectionTitle(section)) : section.kind === "brick_current" ? infoButton(brickRef(section.reference.split(" on ")[0]), sectionTitle(section)) : "") + '<span class="muted small">' + esc(size(section.bytes)) + "</span></summary><pre>" + esc(section.text) + "</pre></details>";
  box.innerHTML = '<div class="row small" style="margin-bottom:6px"><span class="tag ' + (kind === "flow" ? "fox" : "busy") + '">' + esc(kind) + '</span><span class="muted grow">' + own.length + " context section(s)</span></div>" +
    '<div class="legend-row"><span><i style="background:var(--' + (kind === "flow" ? "flow" : "zone") + ')"></i>' + (kind === "flow" ? "Flow" : "Zone") + ' context</span><span><i style="background:var(--ok)"></i>Common</span><span><i style="background:var(--warn)"></i>Volatile</span><span><i style="background:var(--muted)"></i>Crane rules</span></div>' +
    snapshot.conflicts.map((item) => '<div class="banner"><span class="icon">⚠</span><div class="grow small"><b>Conflict</b>' + esc(item.message) + "</div></div>").join("") +
    snapshot.gaps.map((item) => '<div class="banner info"><span class="icon">ℹ</span><div class="grow small">' + esc(item.message) + "</div></div>").join("") +
    (own.length ? "" : '<div class="empty-note" style="margin-bottom:8px">No context yet: this ' + esc(kind) + " has no context of its own and no Brick attached. Attach one from the Attach tab.</div>") +
    snapshot.sections.map(block).join("") +
    (entry.data.truncated ? '<div class="small muted">Agents receive this cut at the delivery limit.</div>' : "") +
    '<div class="small muted" style="margin-top:6px">Context guides agents; it never grants anything. Crane still checks every action.</div>';
}

// ---------------------------------------------------------------- where a volatile context comes from
document.addEventListener("click", async (event) => {
  const button = event.target.closest("[data-src]");
  if (!button) return;
  const group = button.closest(".sources");
  const area = $(group.dataset.into);
  const loaded = group.nextElementSibling;
  if (button.dataset.src === "write") { loaded.hidden = true; area.focus(); return; }
  const result = await ask("load:file", { format: button.dataset.src });
  if (result.cancelled) return;
  if (!result.ok) { notice("bad", result.message); return; }
  area.value = result.data.text;
  loaded.hidden = false;
  loaded.textContent = "Loaded from " + result.data.name + ". Review or edit it below; nothing is saved until you confirm.";
  area.focus();
});
$("v-detach").addEventListener("click", (event) => {
  const attachment = $("v-attachment").value;
  if (!attachment) { fieldError("attachment", "Choose the attachment to detach."); return; }
  perform(event.currentTarget, "detach", { attachment });
});

// ---------------------------------------------------------------- a Brick's configuration, in a pop-up
const infoButton = (ref, name) => '<button type="button" class="info-btn" data-info="' + esc(ref) + '" title="Configuration and properties of ' + esc(name) + '" aria-label="Configuration and properties of ' + esc(name) + '">i</button>';
// The value of a setting in a stored configuration ("network.allow" reads a nested field)
function valueOf(config, key) {
  return key.split(".").reduce((value, part) => (value && typeof value === "object" ? value[part] : undefined), config);
}
function shown(value) {
  if (value === undefined || value === null) return '<span class="muted">not set (default)</span>';
  if (Array.isArray(value) && !value.length) return '<span class="muted">none</span>';
  if (typeof value === "object") return "<pre>" + esc(JSON.stringify(value, null, 2)) + "</pre>";
  return "<code>" + esc(String(value)) + "</code>";
}
async function showInfo(ref) {
  const result = await ask("load:brick", { ref });
  if (!result.ok) { notice("bad", result.message); return; }
  const definition = result.data.definition || {};
  const row = S.bricks.find((item) => item.ref === ref || item.brick_id === ref) || {};
  const stored = definition.type || row.type;
  const kind = (S.legacy || {})[stored] ? String(S.legacy[stored]).split(" ")[0] : stored;
  const guide = (S.guide || {})[kind] || { settings: [] };
  const config = definition.config || {};
  const covered = new Set();
  const settings = guide.settings.map((setting) => {
    const keys = setting.name.split(" / ");
    keys.forEach((key) => covered.add(key.split(".")[0]));
    const values = keys.map((key) => (keys.length > 1 ? '<div class="sub-key"><code>' + esc(key) + "</code></div>" : "") + shown(valueOf(config, key))).join("");
    return '<div class="prop"><div class="prop-name">' + esc(setting.name) + '</div><div class="prop-value">' + values + '</div><div class="prop-help">' + esc(setting.meaning) + "<small>Allowed: " + esc(setting.values) + "</small></div></div>";
  }).join("");
  const others = Object.keys(config).filter((key) => !covered.has(key));
  const context = (definition.common_context && definition.common_context.text) || "";
  const back = document.createElement("div");
  back.className = "modal-back";
  back.innerHTML = '<div class="modal info-modal" role="dialog" aria-modal="true" aria-labelledby="i-title"><div class="m-head"><span class="mark" aria-hidden="true">i</span><div class="grow"><b id="i-title">' + esc(definition.name || ref) + '</b><div class="row small" style="margin-top:3px"><span class="tag">' + esc(kind) + "</span>" + (row.state ? '<span class="tag ' + stateTone(row.state) + '">' + esc(row.state) + "</span>" : "") + '<span class="muted mono">' + esc(ref) + "</span></div></div></div>" +
    '<div class="m-body">' +
    ((S.legacy || {})[stored] ? '<div class="banner info"><span class="icon">ℹ</span><div class="grow small">Stored as the former kind <b>' + esc(stored) + "</b> and read as " + esc(S.legacy[stored]) + ". Its settings are shown as stored; 'Rewrite as current kinds' in the Library converts it.</div></div>" : "") +
    (guide.purpose ? '<div class="info-line"><h4>Purpose</h4><p>' + esc(guide.purpose) + "</p></div>" : "") +
    (guide.unique ? '<div class="info-line unique"><h4>Only this kind</h4><p>' + esc(guide.unique) + "</p></div>" : "") +
    (definition.intent ? '<div class="info-line"><h4>What this Brick solves</h4><p>' + esc(definition.intent) + "</p></div>" : "") +
    '<h4 class="props-title">Settings you can configure</h4><div class="props">' + (settings || '<div class="muted">This kind has no settings.</div>') +
    others.map((key) => '<div class="prop"><div class="prop-name">' + esc(key) + '</div><div class="prop-value">' + shown(config[key]) + '</div><div class="prop-help">A setting of the former kind.</div></div>').join("") + "</div>" +
    '<h4 class="props-title">Common context</h4>' + (context ? '<pre class="context-text">' + esc(context.length > 2000 ? context.slice(0, 2000) + " …" : context) + "</pre>" : '<p class="muted">None.</p>') +
    ((definition.dependencies || []).length ? '<h4 class="props-title">Needs alongside it</h4><p>' + definition.dependencies.map((item) => "<code>" + esc(item.brick_id + "@" + item.version) + "</code>").join(", ") + "</p>" : "") +
    '</div><div class="m-foot"><button class="btn" data-i="kind">What is a ' + esc(kind) + ' Brick?</button><span class="grow"></span><button class="btn primary" data-i="close">Close</button></div></div>';
  document.body.appendChild(back);
  const close = () => back.remove();
  back.addEventListener("click", (event) => {
    const button = event.target.closest("[data-i]");
    if (event.target === back || (button && button.dataset.i === "close")) close();
    else if (button && button.dataset.i === "kind") { close(); showKind(kind); }
  });
  back.addEventListener("keydown", (event) => { if (event.key === "Escape") { event.preventDefault(); close(); } });
  back.querySelector('[data-i="close"]').focus();
}
document.addEventListener("click", (event) => {
  const info = event.target.closest("[data-info]");
  if (info) { event.preventDefault(); event.stopPropagation(); showInfo(info.dataset.info); return; }
  const migrate = event.target.closest("[data-migrate]");
  if (migrate) { event.preventDefault(); perform(migrate, "migrate", {}); return; }
  const inbox = event.target.closest("[data-inbox]");
  if (inbox) { event.preventDefault(); perform(inbox, "inbox:" + inbox.dataset.inbox, {}); }
}, true);

// ---------------------------------------------------------------- a kind's settings, in a pop-up
// The guide of a kind: the extension's own, the one of the kind a former kind became, or what
// Crane's catalog says about it
function guideOf(type) {
  const own = (S.guide || {})[type];
  if (own) return own;
  const now = String((S.legacy || {})[type] || "").split(" ")[0];
  if ((S.guide || {})[now]) return S.guide[now];
  const info = S.types.find((item) => item.type === type) || {};
  return { purpose: info.purpose || "", unique: info.unique || "", useWhen: "", example: null, settings: (info.fields || []).map((name) => ({ name, meaning: "", values: "" })) };
}
function showKindInfo(type) {
  const guide = guideOf(type);
  const example = guide.example ? guide.example.config : (S.templates || {})[type] || {};
  const rows = guide.settings.map((setting) => {
    const keys = setting.name.split(" / ");
    const values = keys.map((key) => (keys.length > 1 ? '<div class="sub-key"><code>' + esc(key) + "</code></div>" : "") + shown(valueOf(example, key))).join("");
    return '<div class="prop"><div class="prop-name">' + esc(setting.name) + '</div><div class="prop-value"><div class="sub-key">in the example</div>' + values + '</div><div class="prop-help">' + esc(setting.meaning) + (setting.values ? "<small>Allowed: " + esc(setting.values) + "</small>" : "") + "</div></div>";
  }).join("");
  const back = document.createElement("div");
  back.className = "modal-back";
  back.innerHTML = '<div class="modal info-modal" role="dialog" aria-modal="true" aria-labelledby="k-title"><div class="m-head"><span class="mark" aria-hidden="true">i</span><div class="grow"><b id="k-title">' + esc(type.charAt(0).toUpperCase() + type.slice(1)) + ' Brick: its settings</b><div class="muted small">What you can configure, what each setting means, and the values Crane accepts.</div></div></div>' +
    '<div class="m-body">' +
    (guide.purpose ? '<div class="info-line"><h4>Purpose</h4><p>' + esc(guide.purpose) + "</p></div>" : "") +
    (guide.unique ? '<div class="info-line unique"><h4>Only this kind</h4><p>' + esc(guide.unique) + "</p></div>" : "") +
    '<h4 class="props-title">Settings you can configure</h4><div class="props">' + (rows || '<div class="muted">This kind has no settings.</div>') + "</div>" +
    '</div><div class="m-foot"><span class="grow"></span><button class="btn primary" data-k="close">Close</button></div></div>';
  document.body.appendChild(back);
  const close = () => back.remove();
  back.addEventListener("click", (event) => { if (event.target === back || event.target.closest('[data-k="close"]')) close(); });
  back.addEventListener("keydown", (event) => { if (event.key === "Escape") { event.preventDefault(); close(); } });
  back.querySelector('[data-k="close"]').focus();
}
document.addEventListener("click", (event) => {
  const button = event.target.closest("[data-kind-info]");
  if (button) { event.preventDefault(); event.stopPropagation(); showKindInfo(button.dataset.kindInfo); }
}, true);

// ---------------------------------------------------------------- the library
function renderLibrary() {
  const groups = [["Drafts", (row) => row.state === "draft"], ["Active", (row) => row.state === "active"], ["Deprecated and revoked", (row) => row.state === "deprecated" || row.state === "revoked"]];
  const html = groups.map(([title, filter]) => {
    const rows = S.bricks.filter(filter);
    if (!rows.length) return "";
    return "<h3 style=\\"margin:4px 0 8px\\">" + esc(title) + ' <span class="muted small">' + rows.length + '</span></h3><div class="list" style="margin-bottom:14px">' + rows.map((row) => {
      const where = S.attachments.filter((item) => brickRef(item.subject) === row.ref).map((item) => item.target);
      const id = row.state === "draft" ? row.brick_id : row.ref;
      const actions = row.state === "draft"
        ? '<button class="btn small primary" data-activate="' + esc(row.brick_id) + '">Activate</button><button class="btn small" data-validate="' + esc(row.brick_id) + '">Check for problems</button><button class="btn small" data-test="' + esc(id) + '">Test…</button><button class="btn small" data-common="' + esc(id) + '">Edit common context</button>'
        : row.state === "active" ? '<button class="btn small primary" data-attach-brick="' + esc(row.ref) + '">Attach…</button><button class="btn small" data-test="' + esc(id) + '">Test…</button><button class="btn small" data-common="' + esc(id) + '">Edit common context</button>' : "";
      const former = (S.legacy || {})[row.type];
      return '<div class="item"><div class="title"><b>' + esc(row.name) + "</b>" + infoButton(id, row.name) + '<span class="tag">' + esc(row.type) + "</span>" + (former ? '<span class="tag warn" title="Read as ' + esc(former) + '">former kind: now ' + esc(former) + "</span>" : "") + '<span class="tag ' + stateTone(row.state) + '">' + esc(row.state) + "</span>" + (row.validation && row.validation !== "passed" ? '<span class="tag bad">' + esc(row.validation) + "</span>" : "") + '<span class="muted small mono">' + esc(id) + "</span></div>" +
        (row.intent ? '<div class="muted small" style="margin-top:4px">' + esc(row.intent) + "</div>" : "") +
        '<div class="chips-row"><button type="button" class="chip-btn" data-kind="' + esc(former ? former.split(" ")[0] : row.type) + '"><span aria-hidden="true">?</span>What is a ' + esc(former ? former.split(" ")[0] : row.type) + " Brick?</button>" +
        (where.length ? '<span class="muted small">Attached to</span>' + where.map((key) => '<button type="button" class="chip-btn" data-goto="' + esc(key) + '" title="Show it in the graph"><span aria-hidden="true">' + (key.startsWith("flow:") ? "⇢" : "▦") + "</span>" + esc(key.replace(/^[a-z]+:/, "")) + "</button>").join("") : "") + "</div>" +
        (actions ? '<div class="actions">' + actions + "</div>" : "") + "</div>";
    }).join("") + "</div>";
  }).join("");
  const formers = S.bricks.filter((row) => row.state === "active" && (S.legacy || {})[row.type]);
  const banner = formers.length ? '<div class="banner info"><span class="icon">ℹ</span><div class="grow small"><b>' + formers.length + " Brick(s) use a former kind</b>They keep working, read as the kinds that replaced them. Rewrite them as drafts of the current kinds, then activate the drafts and move the attachments.</div><button class='btn small primary' data-migrate>Rewrite as current kinds</button></div>" : "";
  const asked = S.inbox || [];
  const fromDataSea = asked.length ? '<div class="banner info"><span class="icon">⇣</span><div class="grow small"><b>' + asked.length + " change(s) requested on DataSea</b>" + asked.slice(0, 4).map((row) => esc(row.op + " " + row.subject) + " · @" + esc(row.requested_by)).join("<br>") + (asked.length > 4 ? "<br>…" : "") + "</div><button class='btn small primary' data-inbox='apply'>Review and apply</button><button class='btn small' data-inbox='dismiss'>Dismiss…</button></div>" : "";
  $("library").innerHTML = fromDataSea + banner + (html || '<div class="empty-note">No Bricks yet. Create the first one in "Create a Brick".</div>');
}

// ---------------------------------------------------------------- buttons anywhere on the page
document.addEventListener("click", async (event) => {
  const el = event.target.closest("[data-volatile],[data-common],[data-activate],[data-validate],[data-attach-brick],[data-goto]");
  if (!el) return;
  event.preventDefault();
  const d = el.dataset;
  if (d.volatile) { selectTab("context"); $("v-attachment").value = d.volatile; loadVolatile(); $("v-text").focus(); }
  else if (d.common) { selectTab("context"); const select = $("k-brick"); const match = [...select.options].find((option) => option.value === d.common || option.value.split("@")[0] === d.common.split("@")[0]); if (match) { select.value = match.value; loadCommon(); } }
  else if (d.activate) perform(el, "activate", { id: d.activate });
  else if (d.validate) perform(el, "validate", { id: d.validate });
  else if (d.attachBrick) { selectTab("attach"); $("a-subject").value = d.attachBrick; const picked = graph.picked(); if (picked && !picked.startsWith("item:")) setTarget(picked); $("a-target").focus(); }
  else if (d.goto) { graph.pick(d.goto); $("graph").scrollIntoView({ block: "nearest" }); }
});

// ---------------------------------------------------------------- the forms
$("create-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = $("create-form");
  clearErrors(form);
  const values = { type: $("c-type").value, name: $("c-name").value.trim(), intent: $("c-intent").value.trim(), common: $("c-common").value, config: $("c-config").value, now: $("c-now").checked, target: $("c-target").value, reason: $("c-reason").value.trim(), volatile: $("c-volatile").value };
  let bad = false;
  if (!values.name) { fieldError("name", "Give the Brick a name."); bad = true; }
  try { JSON.parse(values.config || "{}"); } catch (error) { form.querySelector("details.more").open = true; fieldError("config", "Not valid JSON: " + error.message); bad = true; }
  if (values.now && !values.reason) { fieldError("reason", "Say why it is activated; it is recorded."); bad = true; }
  if (bad) { notice("bad", "Some fields need attention."); return; }
  perform($("c-submit"), "create", values, () => { for (const id of ["c-name", "c-intent", "c-common", "c-volatile", "c-reason"]) $(id).value = ""; configTouched = false; applyType(); });
});
$("attach-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("attach-form"));
  const values = { subject: $("a-subject").value, target: $("a-target").value, volatile: $("a-volatile").value };
  if (!values.subject) fieldError("subject", "Choose a Brick.");
  if (!values.target) fieldError("target", "Choose a Zone or Flow, or click one in the graph.");
  if (!values.subject || !values.target) { notice("bad", "Choose what to attach and where."); return; }
  perform($("a-submit"), "attach", values, () => { $("a-volatile").value = ""; });
});
async function loadCommon() {
  const ref = $("k-brick").value;
  const row = S.bricks.find((item) => item.ref === ref || (item.state === "draft" && item.brick_id === ref));
  $("k-promote-box").hidden = !row || row.state !== "active";
  if (!ref) { $("k-text").value = ""; return; }
  $("k-text").value = "Loading…";
  const result = await ask("load:common", { ref });
  commonLoaded = result.ok ? String(result.data || "") : "";
  $("k-text").value = commonLoaded;
  if (!result.ok) notice("bad", result.message);
}
function loadVolatile() { const row = S.attachments.find((item) => item.attachment_id === $("v-attachment").value); $("v-text").value = row ? row.current_context || "" : ""; }
$("k-brick").addEventListener("change", loadCommon);
$("v-attachment").addEventListener("change", loadVolatile);
$("k-promote").addEventListener("change", () => { $("k-reason-box").hidden = !$("k-promote").checked; });
$("common-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("common-form"));
  const values = { ref: $("k-brick").value, text: $("k-text").value, promote: !$("k-promote-box").hidden && $("k-promote").checked, reason: $("k-reason").value.trim() };
  if (!values.ref) { fieldError("brick", "Choose a Brick."); return; }
  if (values.text === commonLoaded) { notice("info", "Nothing changed in the common context."); return; }
  if (values.promote && !values.reason) { fieldError("reason", "Say why; it is recorded."); return; }
  perform($("k-submit"), "common", values, (result) => { commonLoaded = values.text; });
});
$("volatile-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("volatile-form"));
  const values = { attachment: $("v-attachment").value, text: $("v-text").value };
  if (!values.attachment) { fieldError("attachment", "Choose an attachment."); return; }
  const row = S.attachments.find((item) => item.attachment_id === values.attachment);
  if (row && (row.current_context || "") === values.text) { notice("info", "Nothing changed in the volatile context."); return; }
  perform($("v-submit"), "volatile", values);
});
// ---------------------------------------------------------------- testing a Brick
const SESSION = ["tool_calls", "minutes", "files_changed", "lines_changed", "commands", "network_calls", "denials", "failures_in_row"];
const CASE_TONE = { PASS: "ok", FAIL: "bad", BLOCKED: "bad", UNKNOWN: "warn", NOT_RUN: "warn", DECLARED: "" };
const VERDICT = { allow: ["var(--ok)", "ok", "Allowed by the Bricks"], approval: ["var(--warn)", "warn", "Held for a person"], deny: ["var(--bad)", "bad", "Refused"] };
function testForm() {
  const session = {};
  for (const key of SESSION) session[key] = $("s-" + key).value;
  return { operation: $("t-operation").value, files: $("t-files").value, lines: $("t-lines").value, newFile: $("t-new").checked, delete: $("t-delete").checked, command: $("t-command").value, hosts: $("t-hosts").value, agent: $("t-agent").value, model: $("t-model").value, watchFiring: $("t-watch").checked, session };
}
function fillTest(form) {
  $("t-operation").value = form.operation; $("t-files").value = form.files; $("t-lines").value = form.lines; $("t-new").checked = form.newFile; $("t-delete").checked = form.delete;
  $("t-command").value = form.command; $("t-hosts").value = form.hosts; $("t-agent").value = form.agent; $("t-model").value = form.model; $("t-watch").checked = form.watchFiring;
  for (const key of SESSION) $("s-" + key).value = form.session[key] || "";
  if (SESSION.some((key) => form.session[key]) || form.watchFiring) $("test-form").querySelector("details.more").open = true;
  showWrite();
}
function showWrite() { $("t-write").hidden = $("t-operation").value !== "write"; }
async function loadPreset() {
  const ref = $("t-brick").value;
  $("t-note").hidden = true;
  if (!ref) return;
  const keep = $("t-files").value.split("\\n")[0].trim();
  const result = await ask("load:test", { ref, file: keep });
  if (!result.ok) { notice("bad", result.message); return; }
  fillTest(result.data.form);
  if (result.data.note) { $("t-note-text").textContent = result.data.note; $("t-note").hidden = false; }
}
function openTest(ref) {
  selectTab("test");
  const select = $("t-brick");
  const match = [...select.options].find((option) => option.value === ref || option.value.split("@")[0] === ref.split("@")[0]);
  if (match) select.value = match.value;
  const attached = S.attachments.find((item) => brickRef(item.subject).split("@")[0] === ref.split("@")[0]);
  const picked = graph.picked();
  if (picked && !picked.startsWith("item:")) setTarget(picked); else if (attached) $("t-target").value = attached.target;
  loadPreset();
}
async function runTest(button, mock) {
  const form = $("test-form");
  clearErrors(form);
  const values = { ref: $("t-brick").value, target: $("t-target").value, mock, form: testForm() };
  if (!values.ref) { fieldError("brick", "Choose a Brick."); return; }
  if (mock && !values.target) { fieldError("target", "Choose where the call happens."); return; }
  const label = button.innerHTML;
  button.disabled = true; button.innerHTML = '<span class="spin" aria-hidden="true"></span> Testing…';
  const result = await ask("test", values);
  button.disabled = false; button.innerHTML = label;
  if (!result.ok) { $("t-result").innerHTML = ""; notice(result.cancelled ? "info" : "bad", result.message, result.problems); for (const [name, text] of Object.entries(result.fields || {})) fieldError(name, text); return; }
  notice();
  $("t-result").innerHTML = result.data.trial ? renderTrial(result.data.trial) : renderCases(result.data);
  $("t-result").scrollIntoView({ block: "nearest" });
}
function renderTrial(trial) {
  const tone = VERDICT[trial.verdict] || VERDICT.allow;
  const findings = trial.findings.map((item) => '<div class="case"><span class="tag ' + (VERDICT[item.decision] || VERDICT.allow)[1] + '">' + esc(item.decision === "approval" ? "ask" : item.decision) + '</span><span class="what">' + esc(item.reason) + '</span><span class="why">' + esc(item.rule) + " · from " + esc(item.brick) + "</span></div>").join("");
  const files = trial.files.map((row) => '<div class="case"><span class="tag ' + (VERDICT[row.decision] || VERDICT.allow)[1] + '">' + esc(row.ceiling) + '</span><span class="what mono">' + esc(row.path) + '</span><span class="why">' + (row.targets.length ? "in " + esc(row.targets.join(", ")) : "in no Zone or Flow") + (row.zone_ceiling && row.zone_ceiling !== row.ceiling ? " · the Zones alone allow " + esc(row.zone_ceiling) : "") + (row.reasons.length ? " · " + esc(row.reasons.join("; ")) : "") + (row.exists ? "" : " · new file") + "</span></div>").join("");
  return '<div class="verdict" style="--tone:' + tone[0] + '"><span class="tag ' + tone[1] + '">' + esc(trial.verdict) + '</span><b class="grow">' + esc(tone[2]) + '</b><span class="muted small">on ' + esc(trial.target) + "</span></div>" +
    '<h4 class="props-title">Rules that decided</h4><div class="cases">' + (findings || '<div class="muted small">No Brick rule made this call stricter.</div>') + "</div>" +
    (files ? '<h4 class="props-title">Files, with their autonomy ceiling</h4><div class="cases">' + files + "</div>" : "") +
    trial.notes.map((note) => '<div class="small muted">' + esc(note) + "</div>").join("");
}
function renderCases(data) {
  const cases = (data.cases || []).map((item) => '<div class="case"><span class="tag ' + (CASE_TONE[item.status] || "") + '">' + esc(item.status) + '</span><span class="what">' + esc(item.name) + '</span><span class="why">' + esc(item.detail) + "</span></div>").join("");
  const files = (data.effects || []).flatMap((effect) => (effect.files || []).map((row) => '<div class="case"><span class="tag">' + esc(row.ceiling || row.criticality || "") + '</span><span class="what mono">' + esc(row.path) + '</span><span class="why">' + esc(effect.target) + " · " + esc((row.reasons || []).join("; ")) + "</span></div>")).join("");
  return '<div class="verdict" style="--tone:var(--' + (data.status === "passed" || data.status === "ok" ? "ok" : data.status === "failed" || data.status === "blocked" ? "bad" : "warn") + ')"><b class="grow">' + esc(data.summary) + "</b></div>" +
    '<div class="cases">' + (cases || '<div class="muted small">No checks.</div>') + "</div>" +
    (files ? '<h4 class="props-title">Files it restricts</h4><div class="cases">' + files + "</div>" : "") +
    (data.runtime ? '<div class="small muted">What Crane does with this kind today: ' + esc(data.runtime) + ".</div>" : "");
}
$("t-brick").addEventListener("change", loadPreset);
$("t-operation").addEventListener("change", showWrite);
$("t-preset").addEventListener("click", (event) => { event.preventDefault(); loadPreset(); });
$("t-check").addEventListener("click", (event) => { event.preventDefault(); runTest(event.currentTarget, false); });
$("test-form").addEventListener("submit", (event) => { event.preventDefault(); runTest($("t-run"), true); });
document.addEventListener("click", (event) => {
  const button = event.target.closest("[data-test]");
  if (button) { event.preventDefault(); openTest(button.dataset.test); }
});

showPreview(null);
vscode.postMessage({ type: "ready" });
`,
};
