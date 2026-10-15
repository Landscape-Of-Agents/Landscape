// Foxx for VS Code: set up Crane in a repository, create policies, checkpoints, and selection
// contracts from the editor, see where selections are and whether they pass, and manage zones,
// flows, and context packs in .crane/governance.yaml.
// Everything runs through the Crane CLI; decorations never modify source files, and governance
// edits are validated and signed by Crane (only admins can sign) and reversible.

import { execFile } from "child_process";
import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { CommandResult, CraneStatus, craneStatus, Finding, parseJson, RELEASES_URL, runCrane, selectionArgs, selectionIdFrom, Span, TestReport, ValidateReport } from "./core/crane";
import { activate as activateEntity, addContext, addFlow, addZone, attachToZone, checkId, connectZones, entities, sourcePaths, ZoneInput } from "./core/governance";
import { parsePolicies, Policy } from "./core/policies";
import { checkName, confirmation, ownerKey, repoArgs } from "./core/setup";
import { FoxxPanel, PanelHost, PanelState, Progress, ResultAction, RunResult, SelectionInfo } from "./panel";
import { FoxxSidebar, SidebarHost, SidebarState } from "./sidebar";

/** A registry record as stored in .crane/registry.json (only the fields Foxx shows). */
interface RegistryRecord {
  id: string;
  name: string | null;
  operation: "preserve" | "target";
  change_type: string | null;
  policy: string;
}

let output: vscode.OutputChannel;
let diagnostics: vscode.DiagnosticCollection;
let lastGovernanceText: string | undefined;
const outcomes = new Map<string, CommandResult["outcome"]>();
let spans: Record<string, Span | null> = {};
let records: RegistryRecord[] = [];
let status: vscode.StatusBarItem;
let extensionUri: vscode.Uri;
let lastSelection: SelectionInfo | undefined;
let extensionVersion = "";
/** What was done in this window, newest first (the sidebar's Activity). */
const activity: SidebarState["activity"] = [];

/**
 * Remember an action and its outcome for the sidebar's Activity list.
 * @param label what was done
 * @param ok whether it worked
 * @param message what the person was told
 */
function remember(label: string, ok: boolean, message: string): void {
  activity.unshift({ at: Date.now(), label, ok, message: message.length > 160 ? `${message.slice(0, 160)}…` : message });
  activity.length = Math.min(activity.length, 15);
  FoxxSidebar.update();
}

/**
 * Where a selection stands: its last test outcome, UNRESOLVED when Crane cannot find it, else
 * PENDING (not tested in this window).
 * @param id selection id
 * @returns outcome
 */
function outcomeOf(id: string): string {
  return outcomes.get(id) ?? (spans[id] === null ? "UNRESOLVED" : "PENDING");
}

/** The configured Crane executable. */
function executable(): string {
  return vscode.workspace.getConfiguration("foxx").get<string>("cranePath", "crane");
}

/** The check of the configured Crane, run once (and again when "foxx.cranePath" changes). */
let craneChecked: Promise<CraneStatus> | undefined;
/** Its result, once known. */
let craneKnown: CraneStatus | undefined;
/** Whether the person was already told that Crane cannot be used. */
let craneWarned = false;

/**
 * Check that the configured Crane exists and is new enough for every command Foxx runs. An old
 * Crane (before 'crane governance sign') would make every zone, flow, and context change fail and
 * be reverted, so nothing runs until this passes.
 * @returns status
 */
function checkCrane(): Promise<CraneStatus> {
  if (!craneChecked) {
    const configured = executable();
    craneChecked = runCrane(configured, root() ?? gitFolder() ?? os.homedir(), ["--version"]).then(async (result) => {
      let status = craneStatus(configured, result);
      if (status.state === "outdated") status = craneStatus(configured, result, await locate(configured));
      craneKnown = status;
      return status;
    });
  }
  return craneChecked;
}

/**
 * Find where a bare executable name resolves, so the person knows which Crane is too old.
 * @param name executable name or path
 * @returns full path, or undefined
 */
function locate(name: string): Promise<string | undefined> {
  if (path.isAbsolute(name)) return Promise.resolve(name);
  return new Promise((resolve) => {
    execFile(process.platform === "win32" ? "where" : "which", [name], { windowsHide: true }, (error, stdout) => resolve(error ? undefined : String(stdout).split(/\r?\n/)[0].trim() || undefined));
  });
}

/**
 * Tell the person once that Crane cannot be used, with ways to fix it.
 * @param status the failed check
 */
async function warnCrane(status: CraneStatus): Promise<void> {
  if (craneWarned || status.state === "ok") return;
  craneWarned = true;
  updateStatus(lastReport);
  const choice = await vscode.window.showErrorMessage(`Foxx: ${status.message}`, "Download Crane", "Set Crane Path");
  if (choice) craneFix(choice === "Download Crane" ? "crane:download" : "crane:path");
}

/**
 * Open the way to fix an unusable Crane.
 * @param id crane:download (the release page) or crane:path (the setting)
 */
function craneFix(id: string): void {
  if (id === "crane:download") void vscode.env.openExternal(vscode.Uri.parse(RELEASES_URL));
  else void vscode.commands.executeCommand("workbench.action.openSettings", "foxx.cranePath");
}

/** Where the panel was opened from (DASHBOARD.md F31.2). */
type OpenedFrom = "TITLE_BUTTON" | "KEYBINDING" | "EXPLORER" | "STATUS_BAR" | "SIDEBAR" | "CODE_LENS";

/**
 * Record an event of the editor side (DASHBOARD.md F31) with 'crane foxx event'. It runs in the
 * background, writes nothing to the Foxx output, and never changes or delays what the person did.
 * @param name event name (FOXX_...)
 * @param fields attributes as key=value; empty values are left out
 */
function record(name: string, fields: Record<string, string | number | boolean | undefined>): void {
  const directory = root();
  // Older Crane releases do not record editor events; nothing is started for them
  if (!directory || !craneKnown?.events) return;
  const pairs = Object.entries({ ...fields, extension_version: extensionVersion })
    .filter(([, value]) => value !== undefined && value !== "")
    .map(([key, value]) => `${key}=${String(value).replace(/\s+/g, " ").slice(0, 200)}`);
  void runCrane(executable(), directory, ["foxx", "event", name, ...pairs]);
}

/**
 * Name why a Crane command or governance change failed, for the event (DASHBOARD.md F31.3).
 * @param message what the person was told
 * @returns failure_state
 */
function failureOf(message: string): string {
  const text = message.toLowerCase();
  if (text.includes("admin")) return "NOT_ADMIN";
  if (text.includes("sign")) return "SIGN_FAILED";
  if (text.includes("not kept")) return "VALIDATION_FAILED";
  if (text.startsWith("select") || text.startsWith("choose") || text.startsWith("give") || text.includes("name:")) return "INVALID_INPUT";
  return "CRANE_ERROR";
}

/**
 * Describe an editor's selection for the panel: file, lines, the enclosing function (from the
 * language's document symbols), and a short preview.
 * @param editor text editor
 * @returns the selection, or undefined for a non-file editor
 */
async function describeSelection(editor: vscode.TextEditor | undefined): Promise<SelectionInfo | undefined> {
  if (!editor || editor.document.uri.scheme !== "file") return undefined;
  const { selection, document } = editor;
  const start = selection.start.line + 1;
  const end = selection.end.character === 0 && selection.end.line > selection.start.line ? selection.end.line : selection.end.line + 1;
  let symbol: string | undefined;
  try {
    const symbols = (await vscode.commands.executeCommand<vscode.DocumentSymbol[]>("vscode.executeDocumentSymbolProvider", document.uri)) ?? [];
    const functions = [vscode.SymbolKind.Function, vscode.SymbolKind.Method, vscode.SymbolKind.Constructor];
    // The innermost function or method containing the start of the selection
    const visit = (list: vscode.DocumentSymbol[]): void => {
      for (const item of list) {
        if (!item.range.contains(selection.start)) continue;
        if (functions.includes(item.kind)) symbol = item.name.replace(/\(.*$/, "");
        visit(item.children ?? []);
      }
    };
    visit(symbols);
  } catch {
    symbol = undefined;
  }
  const text = document.getText(selection.isEmpty ? document.lineAt(selection.start.line).range : selection);
  return { uri: document.uri.toString(), file: relative(document.uri.fsPath), start, end, empty: selection.isEmpty, symbol, preview: text.length > 400 ? `${text.slice(0, 400)}…` : text };
}

/**
 * Describe a whole file for the panel, as if every line of it were selected.
 * @param uri file
 * @returns the file as a selection, or undefined when it cannot be opened as text
 */
async function describeFile(uri: vscode.Uri): Promise<SelectionInfo | undefined> {
  try {
    const document = await vscode.workspace.openTextDocument(uri);
    const text = document.getText();
    return { uri: uri.toString(), file: relative(uri.fsPath), start: 1, end: Math.max(1, document.lineCount), empty: false, preview: text.length > 400 ? `${text.slice(0, 400)}…` : text };
  } catch {
    return undefined;
  }
}

/**
 * Open the Foxx panel (the editor title button, Ctrl+Alt+F, and the menus). It works on a block of
 * code: the active editor's selection, or a whole file chosen in the Explorer; with neither, it
 * asks for a selection instead.
 * @param file file chosen in the Explorer, if any
 */
async function openPanel(file: vscode.Uri | undefined, from: OpenedFrom): Promise<void> {
  const refused = (failure: string, statement: string) =>
    record("FOXX_PANEL_REFUSED", { status: "REFUSED", failure_state: failure, statement, opened_from: from });
  if (file?.scheme === "file") {
    const whole = await describeFile(file);
    if (!whole) {
      refused("FILE_UNREADABLE", "File cannot be opened as text.");
      void vscode.window.showErrorMessage("Foxx: this file cannot be opened as text.");
      return;
    }
    lastSelection = whole;
  } else {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.uri.scheme !== "file" || editor.selection.isEmpty) {
      refused(editor && editor.document.uri.scheme !== "file" ? "NOT_A_FILE" : "NO_SELECTION", "No code block was selected.");
      void vscode.window.showErrorMessage("Please select a code block first.");
      return;
    }
    lastSelection = await describeSelection(editor);
  }
  record("FOXX_PANEL_OPENED", {
    status: "OPENED",
    statement: "Foxx panel opened for the selected code.",
    opened_from: from,
    selected_lines: lastSelection ? lastSelection.end - lastSelection.start + 1 : undefined,
    enclosing_function: lastSelection?.symbol,
  });
  FoxxPanel.show(extensionUri, panelHost, lastSelection);
}

/** A step of a running action, reported to the panel as it starts and ends. */
type Step = <T>(id: string, label: string, work: () => Promise<T>, judge?: (value: T) => { ok: boolean; detail?: string }) => Promise<T>;

/**
 * Make a step runner that reports each step to the panel.
 * @param progress the panel's progress callback
 * @returns step runner
 */
function stepper(progress: Progress): Step {
  return async (id, label, work, judge) => {
    const started = Date.now();
    progress({ id, label, state: "running" });
    try {
      const value = await work();
      const verdict = judge ? judge(value) : { ok: true };
      progress({ id, label, state: verdict.ok ? "done" : "failed", detail: verdict.detail, ms: Date.now() - started });
      return value;
    } catch (error) {
      progress({ id, label, state: "failed", detail: (error as Error).message, ms: Date.now() - started });
      throw error;
    }
  };
}

/** A step runner that reports nothing (commands run outside the panel). */
const quietStep: Step = (_id, _label, work) => work();

/** Buttons that fix an unusable Crane. */
const CRANE_FIXES: ResultAction[] = [
  { id: "crane:download", label: "Download Crane", kind: "command" },
  { id: "crane:path", label: "Set Crane path", kind: "command" },
  { id: "crane:recheck", label: "Check again", kind: "command" },
];

/** The latest validation, for the panel's header. */
let lastReport: ValidateReport | undefined;

/** The extension side of the Foxx panel. */
const panelHost: PanelHost = {
  state(): PanelState {
    const ready = Boolean(root());
    const report = lastReport;
    return {
      ready,
      repository: path.basename(root() ?? gitFolder() ?? ""),
      crane: craneKnown ? { state: craneKnown.state, version: craneKnown.version, message: craneKnown.message } : undefined,
      generation: report?.generation,
      issues: report ? report.findings.filter((finding) => finding.severity === "HIGH" || finding.severity === "CRITICAL").length : undefined,
      undo: lastGovernanceText !== undefined,
      policies: ready ? policies().map((policy) => policy.name) : [],
      entities: ready
        ? entities(readGovernance())
            .filter((entity) => entity.kind !== "context_pack")
            .map((entity) => ({ kind: entity.kind as "zone" | "flow", id: entity.id, status: entity.status, description: entity.description }))
        : [],
      selections: ready ? loadRecords().map((record) => ({ id: record.id, label: `${record.operation}${record.name ? ` ${record.name}` : ""}`, policy: record.policy })) : [],
    };
  },
  run: async (action, values, selection, progress) => {
    const started = Date.now();
    let result: RunResult;
    try {
      result = await runAction(action, values, selection, stepper(progress));
    } catch (error) {
      result = { ok: false, message: (error as Error).message, actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] };
    }
    remember(action.replace(/_/g, " "), result.ok, result.message);
    record("FOXX_ACTION_RUN", {
      status: result.ok ? "OK" : "FAILED",
      failure_state: result.ok ? "NA" : failureOf(result.message),
      statement: result.ok ? "Panel action finished." : "Panel action failed.",
      panel_action: action.toUpperCase(),
      enforce_now: typeof values.enforce === "boolean" ? values.enforce : undefined,
      duration_ms: Date.now() - started,
    });
    return result;
  },
  command: async (id) => {
    if (id === "crane:download" || id === "crane:path") craneFix(id);
    else if (id === "crane:recheck") {
      craneChecked = undefined;
      craneWarned = false;
      const checked = await checkCrane();
      if (checked.state === "ok" && root()) await validate(true);
      updateStatus(lastReport);
    } else if (id === "open:governance") {
      const file = path.join(root() ?? "", ".crane", "governance.yaml");
      if (fs.existsSync(file)) await vscode.window.showTextDocument(vscode.Uri.file(file), { preview: false, viewColumn: vscode.ViewColumn.One });
    } else if (id === "open:output") output.show(true);
    else if (id === "open:problems") await vscode.commands.executeCommand("workbench.actions.view.problems");
  },
};

/** The extension side of the Foxx sidebar. */
const sidebarHost: SidebarHost = {
  state(): SidebarState {
    const ready = Boolean(root());
    let governed: ReturnType<typeof entities> = [];
    try {
      governed = ready ? entities(readGovernance()) : [];
    } catch {
      governed = [];
    }
    const rank: Record<string, number> = { CRITICAL: 0, HIGH: 1, MEDIUM: 2, LOW: 3 };
    return {
      ready,
      repository: path.basename(root() ?? gitFolder() ?? ""),
      crane: craneKnown ? { state: craneKnown.state, version: craneKnown.version, message: craneKnown.message } : undefined,
      generation: lastReport?.generation,
      valid: lastReport?.valid,
      policies: ready ? policies().map((policy) => policy.name) : [],
      selections: ready
        ? loadRecords().map((record) => {
            const span = spans[record.id];
            return { id: record.id, name: record.name, operation: record.operation, policy: record.policy, location: span ? `${span.path}:${span.start_line}-${span.end_line}` : null, outcome: outcomeOf(record.id) };
          })
        : [],
      entities: governed.filter((entity) => entity.kind !== "context_pack").map((entity) => ({ kind: entity.kind as "zone" | "flow", id: entity.id, status: entity.status, description: entity.description })),
      findings: (lastReport?.findings ?? [])
        .filter((finding) => finding.severity !== "LOW")
        .sort((a, b) => (rank[a.severity] ?? 9) - (rank[b.severity] ?? 9))
        .slice(0, 8)
        .map((finding) => ({ severity: finding.severity, code: finding.code, message: finding.message })),
      activity,
    };
  },
  command: async (id, argument) => {
    if (id.startsWith("crane:")) await panelHost.command(id);
    else if (id === "foxx.menu") await vscode.commands.executeCommand("foxx.menu", "SIDEBAR");
    else await vscode.commands.executeCommand(id, argument);
  },
};

/**
 * Select a selection's lines in the editor, and optionally open the Foxx panel on them.
 * @param id selection id
 * @param open open the panel too
 */
async function revealSelection(id: string, open = false): Promise<void> {
  const span = spans[id];
  if (!span) {
    void vscode.window.showWarningMessage(`Foxx: Crane cannot find selection ${id} in the code (it is UNRESOLVED). Run 'Foxx: Validate Governance' for details.`);
    return;
  }
  const editor = await vscode.window.showTextDocument(vscode.Uri.file(path.join(root() ?? "", span.path)), { preview: false, viewColumn: vscode.ViewColumn.One });
  const end = Math.min(editor.document.lineCount - 1, Math.max(span.start_line - 1, span.end_line - 1));
  editor.selection = new vscode.Selection(span.start_line - 1, 0, end, editor.document.lineAt(end).text.length);
  editor.revealRange(editor.selection, vscode.TextEditorRevealType.InCenterIfOutsideViewport);
  if (open) await openPanel(undefined, "CODE_LENS");
}

/**
 * Run an action chosen in the Foxx panel, step by step: check Crane, then the action's own steps.
 * Nothing is written before Crane is known to support every step.
 * @param action action id (core/actions)
 * @param values the form's values
 * @param selection the selected code, when any
 * @param step step runner that reports progress
 * @returns whether it worked, what to tell the person, and follow-up buttons
 */
async function runAction(action: string, values: Record<string, string | boolean>, selection: SelectionInfo | undefined, step: Step = quietStep): Promise<RunResult> {
  const value = (key: string) => (typeof values[key] === "string" ? (values[key] as string).trim() : "");
  const lines = value("context").split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const failed = (result: { stdout: string; stderr: string }) => (result.stderr || result.stdout).trim().replace(/^crane: /, "");
  const checked = await step("crane", "Check Crane", () => checkCrane(), (status) => ({ ok: status.state === "ok", detail: status.state === "ok" ? `Crane ${status.version}${status.location ? " · " + status.location : ""}` : status.message }));
  if (checked.state !== "ok") return { ok: false, message: checked.message ?? "Crane cannot be used.", actions: CRANE_FIXES };
  const outcome = async (args: string[], success: string): Promise<RunResult> => {
    const result = await step("run", `crane ${args.join(" ")}`, () => crane(args), (result) => ({ ok: Boolean(result) && result!.code === 0, detail: result && result.code !== 0 ? failed(result) : undefined }));
    if (!result) return { ok: false, message: "Crane is not set up in this repository.", actions: [{ id: "setup", label: "Set up Crane", kind: "run" }] };
    if (result.code !== 0) return { ok: false, message: failed(result), actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] };
    await step("refresh", "Refresh selections and decorations", () => validate(true));
    return { ok: true, message: success };
  };
  const needSelection = (): RunResult | undefined => (!selection || selection.empty ? { ok: false, message: "Select the lines in the editor first." } : undefined);
  // What a zone or flow covers: the enclosing function when chosen and known, else the file
  const cover = (): Partial<Record<"symbols" | "files", string[]>> =>
    values.cover === "symbol" && selection?.symbol ? { symbols: [`${selection.file}:${selection.symbol}`] } : { files: [selection?.file ?? ""] };
  const governed = (change: (text: string) => string, summary: string) => writeGovernance(change, summary, step);
  switch (action) {
    case "preserve":
    case "target": {
      const missing = needSelection();
      if (missing || !selection) return missing ?? { ok: false, message: "" };
      const document = vscode.workspace.textDocuments.find((open) => open.uri.toString() === selection.uri);
      if (document?.isDirty) await step("save", `Save ${selection.file}`, () => Promise.resolve(document.save()));
      const operation = action === "preserve" ? "preserve" : "target";
      const args = selectionArgs(operation, selection.file, selection.start, selection.end, { policy: value("policy") || undefined, changeType: value("changeType") || undefined, name: value("name") || undefined });
      const result = await step("run", `crane ${args[0]} ${selection.file} lines ${selection.start}-${selection.end}`, () => crane(args), (result) => ({ ok: Boolean(result) && result!.code === 0 && Boolean(selectionIdFrom(result!.stdout)), detail: result && selectionIdFrom(result.stdout) ? "selection " + selectionIdFrom(result.stdout) : result ? failed(result) : undefined }));
      if (!result) return { ok: false, message: "Crane is not set up in this repository." };
      const id = selectionIdFrom(result.stdout);
      if (result.code !== 0 || !id) return { ok: false, message: failed(result), actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] };
      await step("refresh", "Refresh selections and decorations", () => validate(true));
      return { ok: true, message: `${operation === "preserve" ? "Preserved" : "Targeted"} ${selection.file} lines ${selection.start}-${selection.end} as ${id}. Crane inserted its anchor comments; commit them with .crane.` };
    }
    case "zone": {
      const missing = needSelection();
      if (missing) return missing;
      const id = value("id");
      const zone: ZoneInput = { id, description: value("description"), criticality: value("criticality") as ZoneInput["criticality"], autonomy_ceiling: value("ceiling") as ZoneInput["autonomy_ceiling"], selectors: cover() };
      return governed((text) => {
        let next = addZone(text, zone);
        if (lines.length) next = addContext(next, "zone", id, lines);
        return values.enforce ? activateEntity(next, "zone", id) : next;
      }, `zone ${id} created${values.enforce ? " and enforced" : " as a proposal"}, autonomy ceiling ${zone.autonomy_ceiling}${lines.length ? `, with ${lines.length} line${lines.length === 1 ? "" : "s"} of context` : ""}`);
    }
    case "flow": {
      const id = value("id");
      const entry = value("entry");
      if (!entry) return { ok: false, message: "Give the flow an entry point (path:function)." };
      const ceiling = value("ceiling") as ZoneInput["autonomy_ceiling"];
      return governed((text) => {
        let next = addFlow(text, { id, description: value("description"), criticality: value("criticality") as ZoneInput["criticality"], autonomy_ceiling: ceiling, entry_points: [entry] });
        if (lines.length) next = addContext(next, "flow", id, lines);
        return values.enforce ? activateEntity(next, "flow", id) : next;
      }, `flow ${id} created from ${entry}${values.enforce ? " and enforced" : " as a proposal"}, autonomy ceiling ${ceiling}`);
    }
    case "context": {
      const [kind, id] = value("target").split(":");
      if (!id) return { ok: false, message: "Choose a zone or flow." };
      if (!lines.length) return { ok: false, message: "Write at least one line of context." };
      return governed((text) => addContext(text, kind as "zone" | "flow", id, lines), `context added to ${kind} ${id}; the agent receives it whenever a tool call touches that code`);
    }
    case "zone_attach": {
      const missing = needSelection();
      if (missing) return missing;
      const zone = value("zone");
      if (!zone) return { ok: false, message: "Choose a zone." };
      const [kind, target] = Object.entries(cover())[0] as ["symbols" | "files", string[]];
      return governed((text) => attachToZone(text, zone, kind, target[0]), `${target[0]} added to zone ${zone}`);
    }
    case "undo": {
      const previous = lastGovernanceText;
      if (previous === undefined) {
        record("FOXX_GOVERNANCE_UNDONE", { status: "NOTHING_TO_UNDO", statement: "No governance change to undo." });
        return { ok: false, message: "There is no governance change to undo in this window." };
      }
      const result = await governed(() => previous, "the last governance change was undone");
      if (result.ok) {
        record("FOXX_GOVERNANCE_UNDONE", { status: "UNDONE", statement: "Last governance change undone and signed." });
        lastGovernanceText = undefined;
        return { ...result, actions: [{ id: "open:governance", label: "Open governance.yaml", kind: "command" }] };
      }
      return result;
    }
    case "policy_context": {
      const file = selection?.file;
      if (!file) return { ok: false, message: "Open the file to attach first." };
      const policy = value("policy") || policies()[0]?.name;
      if (!policy) return { ok: false, message: "Create a policy first." };
      return outcome(["add", "policy-context", "file", file, "policy", policy], `${file} attached as context of policy ${policy}.`);
    }
    case "policy": {
      const name = value("name");
      const error = checkName(name);
      if (error) return { ok: false, message: `Policy name: ${error}` };
      const file = value("file") || name.toLowerCase();
      return outcome(["create", "policy", name, file], `Policy ${name} created in .crane/policies/${file}.crane.`);
    }
    case "move":
      if (!value("selection") || !value("policy")) return { ok: false, message: "Choose a selection and the policy to move it to." };
      return outcome(["add", "policy", value("policy"), "marker", value("selection")], `${value("selection")} moved to policy ${value("policy")}.`);
    case "checkpoint": {
      const error = checkName(value("name"));
      if (error) return { ok: false, message: `Checkpoint name: ${error}` };
      return outcome(["checkpoint", value("name")], `Checkpoint ${value("name")} recorded: the current commit is now trusted.`);
    }
    case "hooks":
      return outcome(["agent", "install", "--profile", value("profile") || "claude"], `Hooks installed for ${value("profile") === "codex" ? "Codex" : "Claude Code"}; Crane now checks every tool call.`);
    case "setup":
      await step("run", "Connect to GitHub and run crane init", () => setUp());
      return root() ? { ok: true, message: "Crane is set up in this repository." } : { ok: false, message: "Setup did not finish; see the Foxx output.", actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] };
    case "validate": {
      const report = await step("run", "crane validate", () => validate(true), (report) => ({ ok: Boolean(report), detail: report ? `generation ${report.generation}` : undefined }));
      if (!report) return { ok: false, message: "Validation did not run; see the Foxx output.", actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] };
      const errors = report.findings.filter((finding) => finding.severity === "HIGH" || finding.severity === "CRITICAL");
      return { ok: errors.length === 0, message: errors.length ? `${errors.length} validation error${errors.length === 1 ? "" : "s"}: ${errors.slice(0, 3).map((finding) => finding.message).join("; ")}` : `Governance is valid (generation ${report.generation}, ${report.findings.length} finding${report.findings.length === 1 ? "" : "s"}).`, actions: report.findings.length ? [{ id: "open:problems", label: "Open Problems", kind: "command" }] : undefined };
    }
    case "test": {
      const passed = await step("run", "crane test .", () => test());
      return passed === undefined ? { ok: false, message: "crane test did not produce a report; see the Foxx output.", actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] }
        : { ok: passed, message: passed ? "Every policy passes." : "Some policies fail; failures are shown as problems.", actions: passed ? undefined : [{ id: "open:problems", label: "Open Problems", kind: "command" }] };
    }
    case "status":
      await step("run", "crane policy status", () => Promise.resolve(vscode.commands.executeCommand("foxx.policyStatus")));
      return { ok: true, message: "Policy status is in the Foxx output.", actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }] };
    default:
      return { ok: false, message: `Unknown action ${action}.` };
  }
}

/**
 * Show the repository's state in the status bar.
 * @param report the latest validation, when there is one
 */
function updateStatus(report?: ValidateReport): void {
  if (craneKnown && craneKnown.state !== "ok") {
    status.text = craneKnown.state === "missing" ? "$(error) Foxx: Crane not found" : `$(error) Foxx: update Crane${craneKnown.version ? " (" + craneKnown.version + ")" : ""}`;
    status.tooltip = craneKnown.message;
  } else if (!root()) {
    status.text = "$(shield) Foxx: set up";
    status.tooltip = "Crane is not set up in this repository. Click to set it up.";
  } else if (!report) {
    status.text = "$(shield) Foxx";
    status.tooltip = "Foxx actions";
  } else {
    const errors = report.findings.filter((finding) => finding.severity === "HIGH" || finding.severity === "CRITICAL").length;
    const ids = Object.keys(report.selections);
    const failing = ids.filter((id) => outcomeOf(id) === "FAIL" || outcomeOf(id) === "UNRESOLVED").length;
    const passing = ids.filter((id) => outcomeOf(id) === "PASS").length;
    status.text = errors || failing ? `$(warning) Foxx${failing ? ` $(error) ${failing}` : ""}${errors ? ` $(warning) ${errors}` : ""}` : `$(shield) Foxx${passing ? ` $(pass) ${passing}` : ""}`;
    const tooltip = new vscode.MarkdownString(
      `**Foxx** · generation ${report.generation}\n\n` +
        `| | |\n|---|---|\n| Selections | ${ids.length} |\n| Passing | ${passing} |\n| Failing or unresolved | ${failing} |\n| Validation errors | ${errors} |\n\n` +
        "[Open the overview](command:foxx.openSidebar) · [Test policies](command:foxx.test) · [Validate](command:foxx.validate)",
    );
    tooltip.isTrusted = true;
    status.tooltip = tooltip;
  }
  status.show();
  FoxxPanel.update();
  FoxxSidebar.update();
}


const preserveDecoration = vscode.window.createTextEditorDecorationType({
  backgroundColor: new vscode.ThemeColor("diffEditor.unchangedRegionBackground"),
  isWholeLine: true,
  overviewRulerColor: new vscode.ThemeColor("charts.green"),
  overviewRulerLane: vscode.OverviewRulerLane.Left,
});
const targetDecoration = vscode.window.createTextEditorDecorationType({
  backgroundColor: new vscode.ThemeColor("editor.wordHighlightBackground"),
  isWholeLine: true,
  overviewRulerColor: new vscode.ThemeColor("charts.yellow"),
  overviewRulerLane: vscode.OverviewRulerLane.Left,
});

/**
 * Return the repository root (the first workspace folder containing .crane).
 * @returns root path, or undefined
 */
function root(): string | undefined {
  return vscode.workspace.workspaceFolders?.map((folder) => folder.uri.fsPath).find((folder) => fs.existsSync(path.join(folder, ".crane")));
}

/**
 * Return the first open workspace folder that is a Git repository (for setting Crane up).
 * @returns folder path, or undefined
 */
function gitFolder(): string | undefined {
  return vscode.workspace.workspaceFolders?.map((folder) => folder.uri.fsPath).find((folder) => fs.existsSync(path.join(folder, ".git")));
}

/**
 * Run Crane in a directory and log the invocation and its output.
 * @param directory working directory
 * @param args arguments
 * @param input answers to Crane's questions, one per line
 * @returns result
 */
async function craneIn(directory: string, args: string[], input = "") {
  output.appendLine(`$ crane ${args.join(" ")}`);
  const result = await runCrane(executable(), directory, args, input);
  if (result.stdout.trim()) output.appendLine(result.stdout.trimEnd());
  if (result.stderr.trim()) output.appendLine(result.stderr.trimEnd());
  return result;
}

/**
 * Run Crane in the repository.
 * @param args arguments
 * @returns result (code 126 when the configured Crane cannot be used), or undefined when there is
 *   no repository
 */
async function crane(args: string[]) {
  const directory = root();
  if (!directory) {
    vscode.window.showErrorMessage("Foxx: Crane is not set up in this folder; run 'Foxx: Set Up Crane in This Repository'.");
    return undefined;
  }
  const checked = await checkCrane();
  if (checked.state !== "ok") {
    void warnCrane(checked);
    return { code: 126, stdout: "", stderr: checked.message ?? "Crane cannot be used." };
  }
  return craneIn(directory, args);
}

/**
 * Run a Crane command that changes governance, report its outcome, and refresh the views.
 * @param args arguments
 * @param success message shown when it succeeds
 * @returns whether it succeeded
 */
async function change(args: string[], success: string): Promise<boolean> {
  const result = await crane(args);
  if (!result || result.code === 126) return false;
  if (result.code !== 0) {
    const message = (result.stderr || result.stdout).trim().replace(/^crane: /, "");
    remember(`crane ${args[0]}`, false, message);
    vscode.window.showErrorMessage(`Foxx: ${message}`);
    return false;
  }
  remember(`crane ${args[0]}`, true, success);
  vscode.window.showInformationMessage(`Foxx: ${success}`);
  await validate(true);
  return true;
}

/**
 * Read the clone's origin remote.
 * @param directory repository
 * @returns the remote URL, or undefined
 */
function originRemote(directory: string): Promise<string | undefined> {
  return new Promise((resolve) => {
    execFile("git", ["remote", "get-url", "origin"], { cwd: directory, windowsHide: true }, (error, stdout) => resolve(error ? undefined : String(stdout).trim()));
  });
}

/**
 * Set Crane up in the open repository: connect the clone to its GitHub remote, then 'crane init'.
 * A new repository gets its owner key and a "baseline" checkpoint; joining a repository that is
 * already set up shows the owner key's fingerprint and trusts it only after the person confirms
 * (the same for accepting another admin's signed rules after a merge). Then offers the agent hooks.
 */
async function setUp(): Promise<void> {
  const checked = await checkCrane();
  if (checked.state !== "ok") {
    void warnCrane(checked);
    return;
  }
  const directory = root() ?? gitFolder();
  if (!directory) {
    vscode.window.showErrorMessage("Foxx: open the folder of a Git repository (a clone of a GitHub repository) first.");
    return;
  }
  const connect = repoArgs((await originRemote(directory)) ?? "");
  if (!connect) {
    record("FOXX_SETUP", { status: "FAILED", failure_state: "NO_REMOTE", statement: "No GitHub origin remote." });
    vscode.window.showErrorMessage("Foxx: this repository has no GitHub 'origin' remote (HTTPS or SSH). Add one with 'git remote add origin URL'.");
    return;
  }
  const fresh = !fs.existsSync(path.join(directory, ".crane", "registry.json"));
  const fail = (result: { code: number; stdout: string; stderr: string }) => {
    record("FOXX_SETUP", { status: "FAILED", failure_state: result.code === 127 ? "CRANE_MISSING" : "GIT", statement: "Crane setup failed.", role: fresh ? "OWNER" : "MEMBER" });
    return vscode.window.showErrorMessage(`Foxx: ${(result.stderr || result.stdout).trim().replace(/^crane: /, "")}`);
  };
  const connected = await craneIn(directory, connect);
  if (connected.code !== 0) return void fail(connected);
  let initialized = await craneIn(directory, ["init"], "n\n");
  const asked = confirmation(initialized.stdout + initialized.stderr);
  if (asked) {
    const message =
      asked === "trust_owner"
        ? `This repository's rules are signed by the owner key\n\n${ownerKey(initialized.stdout) ?? "(see the Foxx output)"}\n\nTrust it only if it matches the key the owner sees with 'crane admin list'. Until it is trusted, agent changes stay blocked.`
        : "The signed rules differ from the version this machine last signed. This is expected after merging another admin's change. Accept the current signed rules?";
    const answer = await vscode.window.showWarningMessage(message, { modal: true }, asked === "trust_owner" ? "Trust owner key" : "Accept rules");
    if (!answer) {
      record("FOXX_SETUP", { status: "CANCELLED", failure_state: "OWNER_DECLINED", statement: "Person declined the signed rules.", role: "MEMBER" });
      vscode.window.showInformationMessage("Foxx: nothing changed.");
      return;
    }
    initialized = await craneIn(directory, ["init"], "y\n");
  }
  if (initialized.code !== 0) return void fail(initialized);
  if (fresh) {
    const checkpoint = await craneIn(directory, ["checkpoint", "baseline"]);
    if (checkpoint.code !== 0) return void fail(checkpoint);
  }
  await vscode.commands.executeCommand("setContext", "foxx.ready", true);
  record("FOXX_SETUP", { status: "OK", statement: fresh ? "Crane set up; owner key created." : "Joined the repository's signed rules.", role: fresh ? "OWNER" : "MEMBER" });
  await validate(true);
  tree.refresh();
  const hooks = await vscode.window.showInformationMessage(
    fresh ? "Foxx: Crane is set up (owner key created, baseline checkpoint recorded). Commit the .crane folder, then install the agent hooks:" : "Foxx: this machine now verifies the repository's rules. Install the agent hooks:",
    "Claude Code",
    "Codex",
    "Copilot CLI",
    "Gemini CLI",
  );
  if (hooks) {
    const profiles: Record<string, "claude" | "codex" | "copilot" | "gemini"> = {
      "Claude Code": "claude",
      Codex: "codex",
      "Copilot CLI": "copilot",
      "Gemini CLI": "gemini",
    };
    const profile = profiles[hooks];
    if (profile) await installHooks(profile);
  }
}

/**
 * Install the Crane hooks for an agent.
 * @param kind agent profile, or undefined to ask
 */
async function installHooks(kind?: "claude" | "codex" | "copilot" | "gemini"): Promise<void> {
  const profile = kind ?? (await pick(["claude", "codex", "copilot", "gemini"] as const, "Which agent?"));
  if (!profile) return;
  const name = { claude: "Claude Code", codex: "Codex", copilot: "GitHub Copilot CLI", gemini: "Gemini CLI" }[profile];
  await change(["agent", "install", "--profile", profile], `hooks installed for ${name}; Crane now checks every tool call.`);
}

/** Record HEAD as a trusted checkpoint. */
async function createCheckpoint(): Promise<void> {
  const name = await vscode.window.showInputBox({ prompt: "Checkpoint name (records the current commit as trusted)", value: `checkpoint-${new Date().toISOString().slice(0, 10)}`, validateInput: (value) => checkName(value) });
  if (!name) return;
  await change(["checkpoint", name], `checkpoint ${name} recorded.`);
}

/** Convert a deliberate zone ordering into one proposed flow. */
async function connectZonesFlow(): Promise<void> {
  const zones = entities(readGovernance()).filter((entity) => entity.kind === "zone");
  if (zones.length < 2) {
    void vscode.window.showInformationMessage("Foxx: create at least two zones before connecting them into a flow.");
    return;
  }
  const ordered: string[] = [];
  while (ordered.length < zones.length) {
    const remaining = zones.filter((zone) => !ordered.includes(zone.id));
    const choices = remaining.map((zone) => ({ label: zone.id, description: zone.description || zone.status, id: zone.id }));
    if (ordered.length >= 2) choices.unshift({ label: "Finish flow", description: `Keep this order: ${ordered.join(" → ")}`, id: "" });
    const choice = await vscode.window.showQuickPick(choices, {
      placeHolder: ordered.length ? `Choose the next zone (${ordered.length} selected in order)` : "Choose the first zone in the flow",
      title: "Connect zones into an ordered flow",
    });
    if (!choice) return;
    if (!choice.id) break;
    ordered.push(choice.id);
  }
  if (ordered.length < 2) {
    void vscode.window.showInformationMessage("Foxx: select at least two zones to create a flow.");
    return;
  }
  const id = await vscode.window.showInputBox({
    prompt: "Flow identifier",
    value: ordered.join("-").slice(0, 64),
    validateInput: (value) => checkId(value) ?? (entities(readGovernance()).some((entity) => entity.id === value) ? "this identifier is already used" : undefined),
  });
  if (!id) return;
  const description = await vscode.window.showInputBox({ prompt: "Why does this ordered flow exist?", value: ordered.join(" → ") });
  if (description === undefined) return;
  const result = await writeGovernance(
    (text) => connectZones(text, id, description, ordered),
    `flow ${id} created in order ${ordered.join(" → ")}; the original zones are preserved as embedded stages`,
  );
  (result.ok ? vscode.window.showInformationMessage : vscode.window.showErrorMessage)(`Foxx: ${result.message}`);
}

/** Open the first governed source file for a zone or flow in the editor. */
async function openGovernanceEntity(kind: "zone" | "flow", id: string): Promise<void> {
  const directory = root();
  if (!directory) return;
  try {
    const candidates = sourcePaths(readGovernance(), kind, id);
    for (const candidate of candidates) {
      const absolute = path.resolve(directory, candidate);
      const relative = path.relative(directory, absolute);
      if (relative.startsWith("..") || path.isAbsolute(relative)) continue;
      const patterns = candidate.includes("*") || candidate.includes("?") || candidate.includes("{")
        ? [candidate]
        : [candidate, `${candidate.replace(/[\\/]+$/, "")}/**`];
      let uri: vscode.Uri | undefined;
      for (const pattern of patterns) {
        const matches = await vscode.workspace.findFiles(new vscode.RelativePattern(directory, pattern), undefined, 1);
        if (matches[0]) { uri = matches[0]; break; }
      }
      uri = uri ?? (fs.existsSync(absolute) && fs.statSync(absolute).isFile() ? vscode.Uri.file(absolute) : undefined);
      if (uri) {
        await vscode.window.showTextDocument(uri, { preview: false, viewColumn: vscode.ViewColumn.One });
        return;
      }
    }
    const governance = vscode.Uri.file(path.join(directory, ".crane", "governance.yaml"));
    if (fs.existsSync(governance.fsPath)) {
      await vscode.window.showTextDocument(governance, { preview: false, viewColumn: vscode.ViewColumn.One });
      void vscode.window.showInformationMessage(`Foxx: ${kind} '${id}' has no resolvable source file; opened its governance definition.`);
    }
  } catch (error) {
    void vscode.window.showErrorMessage(`Foxx: could not open ${kind} '${id}': ${(error as Error).message}`);
  }
}

/** Create a policy in a new or existing policy file. */
async function createPolicy(): Promise<void> {
  const existing = policies().map((policy) => policy.name.toLowerCase());
  const name = await vscode.window.showInputBox({
    prompt: "Policy name",
    validateInput: (value) => checkName(value) ?? (existing.includes(value.toLowerCase()) ? "a policy with this name exists" : undefined),
  });
  if (!name) return;
  const file = await vscode.window.showInputBox({ prompt: "Policy file in .crane/policies (without .crane)", value: name.toLowerCase(), validateInput: (value) => checkName(value) });
  if (!file) return;
  await change(["create", "policy", name, file], `policy ${name} created in .crane/policies/${file}.crane. Add lines to it with 'Foxx: Create Selection Contract'.`);
}

/** Move a selection's statement to another policy. */
async function moveToPolicy(): Promise<void> {
  const choice = await vscode.window.showQuickPick(
    loadRecords().map((record) => ({ label: record.id, description: `${record.operation}${record.name ? ` · ${record.name}` : ""}`, detail: `in policy ${record.policy}`, record })),
    { placeHolder: "Selection to move" },
  );
  if (!choice) return;
  const target = await pick(policies().map((policy) => policy.name).filter((name) => name !== choice.record.policy), `Move ${choice.label} to which policy?`);
  if (!target) return;
  await change(["add", "policy", target, "marker", choice.label], `${choice.label} moved to policy ${target}.`);
}

/**
 * Convert an absolute path to a repository-relative one with '/' separators.
 * @param file absolute path
 * @returns relative path
 */
function relative(file: string): string {
  return path.relative(root() ?? "", file).split(path.sep).join("/");
}

/**
 * Load the policies of .crane/policies.
 * @returns policies
 */
function policies(): Policy[] {
  const directory = path.join(root() ?? "", ".crane", "policies");
  if (!fs.existsSync(directory)) return [];
  return fs
    .readdirSync(directory)
    .filter((name) => name.endsWith(".crane"))
    .flatMap((name) => parsePolicies(`.crane/policies/${name}`, fs.readFileSync(path.join(directory, name), "utf8")));
}

/**
 * Load the selection records of the registry.
 * @returns records
 */
function loadRecords(): RegistryRecord[] {
  try {
    const registry = JSON.parse(fs.readFileSync(path.join(root() ?? "", ".crane", "registry.json"), "utf8"));
    return registry.selections ?? [];
  } catch {
    return [];
  }
}

/**
 * Turn findings and failing commands into editor diagnostics.
 * @param findings findings
 * @param commands command results
 */
function publish(findings: Finding[], commands: CommandResult[]): void {
  diagnostics.clear();
  const byFile = new Map<string, vscode.Diagnostic[]>();
  const add = (file: string, range: vscode.Range, message: string, severity: vscode.DiagnosticSeverity, code: string) => {
    const diagnostic = new vscode.Diagnostic(range, message, severity);
    diagnostic.source = "crane";
    diagnostic.code = code;
    byFile.set(file, [...(byFile.get(file) ?? []), diagnostic]);
  };
  const rangeOf = (span: Span | null | undefined) => (span ? new vscode.Range(Math.max(0, span.start_line - 2), 0, Math.max(0, span.end_line), 0) : new vscode.Range(0, 0, 0, 0));
  for (const finding of findings.filter((finding) => finding.severity !== "LOW")) {
    const span = finding.selection ? spans[finding.selection] : undefined;
    const file = span?.path ?? finding.path ?? ".crane/registry.json";
    const severity = finding.severity === "MEDIUM" ? vscode.DiagnosticSeverity.Warning : vscode.DiagnosticSeverity.Error;
    add(file, rangeOf(span), finding.message, severity, finding.code);
  }
  for (const command of commands.filter((command) => command.outcome === "FAIL" || command.outcome === "UNRESOLVED")) {
    const file = command.location?.path ?? command.file;
    add(file, rangeOf(command.location), `${command.operation} ${command.id} (${command.policy}): ${command.message}`, vscode.DiagnosticSeverity.Error, command.outcome);
  }
  for (const [file, list] of byFile) diagnostics.set(vscode.Uri.file(path.join(root() ?? "", file)), list);
}

/**
 * Validate with Crane and refresh selections, decorations, and diagnostics.
 * @param quiet do not show a message
 * @returns the report, or undefined
 */
async function validate(quiet = false): Promise<ValidateReport | undefined> {
  const result = await crane(["validate", "--json"]);
  if (!result) return undefined;
  // Crane cannot be used: the person was told once, and the status bar says so
  if (result.code === 126) return undefined;
  const report = parseJson<ValidateReport>(result);
  if (!report) {
    vscode.window.showErrorMessage(`Foxx: crane validate failed: ${result.stderr.trim() || "no JSON output"}`);
    return undefined;
  }
  lastReport = report;
  spans = report.selections;
  records = loadRecords();
  updateStatus(report);
  publish(report.findings, []);
  decorateAll();
  tree.refresh();
  if (!quiet) {
    vscode.window.showInformationMessage(report.valid ? `Foxx: governance valid (generation ${report.generation})` : `Foxx: ${report.findings.filter((finding) => finding.severity === "HIGH" || finding.severity === "CRITICAL").length} validation errors`);
  }
  return report;
}

/**
 * Run the policy tests and show failures as diagnostics.
 */
async function test(): Promise<boolean | undefined> {
  const result = await crane(["test", ".", "--json"]);
  if (result?.code === 126) return undefined;
  const report = result && parseJson<TestReport>(result);
  if (!report) {
    vscode.window.showErrorMessage("Foxx: crane test did not produce a report; see the Foxx output.");
    return undefined;
  }
  const commands = report.policies.flatMap((policy) => policy.commands);
  outcomes.clear();
  for (const command of commands) outcomes.set(command.id, command.outcome);
  publish(report.findings, commands);
  codeLenses.refresh();
  tree.refresh();
  updateStatus(lastReport);
  remember("test policies", report.passed, report.passed ? "Every policy passes." : `${report.policies.filter((policy) => !policy.passed).length} policies fail.`);
  vscode.window.showInformationMessage(`Foxx: ${report.passed ? "all policies pass" : `${report.policies.filter((policy) => !policy.passed).length} policies fail`}`);
  return report.passed;
}

/**
 * Decorate the selections of every visible editor (decorations never change the file).
 */
function decorateAll(): void {
  for (const editor of vscode.window.visibleTextEditors) {
    const file = relative(editor.document.uri.fsPath);
    const ranges = (operation: "preserve" | "target") =>
      records
        .filter((record) => record.operation === operation && spans[record.id]?.path === file)
        .map((record) => {
          const span = spans[record.id] as Span;
          return {
            range: new vscode.Range(span.start_line - 1, 0, Math.max(span.start_line - 1, span.end_line - 1), 0),
            hoverMessage: `${operation} ${record.id}${record.name ? ` "${record.name}"` : ""} · policy ${record.policy}${record.change_type ? ` · ${record.change_type}` : ""}`,
          };
        });
    editor.setDecorations(preserveDecoration, ranges("preserve"));
    editor.setDecorations(targetDecoration, ranges("target"));
  }
}

/** CodeLens above each selection with its operation, policy, and last test outcome. */
class SelectionLenses implements vscode.CodeLensProvider {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeCodeLenses = this.changed.event;

  /** Ask VS Code to recompute the lenses. */
  refresh(): void {
    this.changed.fire();
  }

  /**
   * Provide lenses for a document.
   * @param document document
   * @returns lenses
   */
  provideCodeLenses(document: vscode.TextDocument): vscode.CodeLens[] {
    if (!vscode.workspace.getConfiguration("foxx").get<boolean>("codeLens", true)) return [];
    const file = relative(document.uri.fsPath);
    return records
      .filter((record) => spans[record.id]?.path === file)
      .map((record) => {
        const span = spans[record.id] as Span;
        const outcome = outcomes.get(record.id);
        const at = new vscode.Range(Math.max(0, span.start_line - 2), 0, Math.max(0, span.start_line - 2), 0);
        const glyph = outcome === "PASS" ? "$(pass-filled)" : outcome === "FAIL" || outcome === "UNRESOLVED" ? "$(error)" : record.operation === "preserve" ? "$(shield)" : "$(target)";
        return [
          new vscode.CodeLens(at, { title: `${glyph} ${record.operation}${record.name ? ` "${record.name}"` : ""} · ${record.id} · ${record.policy}${outcome ? ` · ${outcome}` : " · not tested"}`, command: "foxx.test", tooltip: "Run every policy's tests" }),
          new vscode.CodeLens(at, { title: "Open in Foxx", command: "foxx.revealSelection", arguments: [record.id, true], tooltip: "Select these lines and open the Foxx panel" }),
        ];
      })
      .flat();
  }
}

/** Hover over a selection's lines: what it is, where, its last outcome, and links to act on it. */
class SelectionHovers implements vscode.HoverProvider {
  /**
   * Describe the selection under the pointer.
   * @param document document
   * @param position pointer
   * @returns hover, or undefined outside every selection
   */
  provideHover(document: vscode.TextDocument, position: vscode.Position): vscode.Hover | undefined {
    const file = relative(document.uri.fsPath);
    const line = position.line + 1;
    const record = records.find((record) => {
      const span = spans[record.id];
      return span?.path === file && line >= span.start_line && line <= span.end_line;
    });
    if (!record) return undefined;
    const span = spans[record.id] as Span;
    const outcome = outcomeOf(record.id);
    const argument = encodeURIComponent(JSON.stringify([record.id, true]));
    const text = new vscode.MarkdownString(
      `$(${record.operation === "preserve" ? "shield" : "target"}) **Foxx · ${record.operation === "preserve" ? "Preserved" : "Targeted"} code**${record.name ? ` — ${record.name}` : ""}\n\n` +
        `| | |\n|---|---|\n| Selection | \`${record.id}\` |\n| Policy | ${record.policy} |\n| Lines | ${span.start_line}–${span.end_line} |\n${record.change_type ? `| Change type | ${record.change_type} |\n` : ""}| Last outcome | ${outcome} |\n\n` +
        (record.operation === "preserve" ? "Agents may not change these lines; Crane denies the tool call and records it.\n\n" : "Agents are expected to change these lines as the policy describes.\n\n") +
        `[Open in Foxx](command:foxx.revealSelection?${argument}) · [Test policies](command:foxx.test) · [Overview](command:foxx.openSidebar)`,
      true,
    );
    text.isTrusted = { enabledCommands: ["foxx.revealSelection", "foxx.test", "foxx.openSidebar"] };
    return new vscode.Hover(text);
  }
}

/** A node of the governance tree. */
class Node extends vscode.TreeItem {
  constructor(label: string, readonly children: Node[] = [], description?: string, tooltip?: string | vscode.MarkdownString, icon?: vscode.ThemeIcon, command?: vscode.Command) {
    super(label, children.length ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None);
    this.description = description;
    this.tooltip = tooltip;
    this.iconPath = icon;
    this.command = command;
  }
}

/**
 * A theme icon colored by outcome or status.
 * @param name codicon
 * @param state outcome or status
 * @returns icon
 */
function stateIcon(name: string, state: string | undefined): vscode.ThemeIcon {
  const color = state === "PASS" || state === "active" ? "testing.iconPassed" : state === "FAIL" || state === "UNRESOLVED" ? "testing.iconFailed" : state === "proposed" ? "editorWarning.foreground" : undefined;
  return new vscode.ThemeIcon(name, color ? new vscode.ThemeColor(color) : undefined);
}

/** The Foxx Governance view: policies with their commands, zones, flows, and context packs. */
class GovernanceTree implements vscode.TreeDataProvider<Node> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;

  /** Ask VS Code to rebuild the tree. */
  refresh(): void {
    this.changed.fire();
  }

  /**
   * Return a node as a tree item.
   * @param node node
   * @returns the node
   */
  getTreeItem(node: Node): vscode.TreeItem {
    return node;
  }

  /**
   * Return the children of a node, or the root sections.
   * @param node parent
   * @returns children
   */
  getChildren(node?: Node): Node[] {
    if (node) return node.children;
    const policyNodes = policies().map((policy) => {
      const children = policy.commands.map((command) => {
        const span = spans[command.id];
        const outcome = outcomeOf(command.id);
        const tooltip = new vscode.MarkdownString(`**${command.operation} \`${command.id}\`**\n\nPolicy ${policy.name} · ${span ? `${span.path} lines ${span.start_line}–${span.end_line}` : "not found in the code"}\n\nLast outcome: **${outcome}**`);
        return new Node(`${command.operation} ${command.id}`, [], span ? `${span.path}:${span.start_line}-${span.end_line} · ${outcome}` : "UNRESOLVED", tooltip, stateIcon(command.operation === "preserve" ? "shield" : "target", outcome), { command: "foxx.revealSelection", title: "Reveal", arguments: [command.id] });
      });
      const failing = policy.commands.filter((command) => ["FAIL", "UNRESOLVED"].includes(outcomeOf(command.id))).length;
      return new Node(policy.name, children, `${policy.commands.length} · ${policy.file}`, `Policy ${policy.name} in ${policy.file}`, stateIcon("law", failing ? "FAIL" : undefined));
    });
    let governed: ReturnType<typeof entities> = [];
    try {
      governed = entities(readGovernance());
    } catch (error) {
      return [new Node("Policies", policyNodes, String(policyNodes.length), undefined, new vscode.ThemeIcon("law")), new Node("governance.yaml is invalid", [], String(error), String(error), new vscode.ThemeIcon("error", new vscode.ThemeColor("testing.iconFailed")))];
    }
    const glyphs: Record<string, string> = { zone: "symbol-namespace", flow: "git-merge", context_pack: "book" };
    const section = (kind: string, label: string) => {
      const items = governed.filter((entity) => entity.kind === kind);
      return new Node(label, items.map((entity) => new Node(entity.id, [], entity.status, new vscode.MarkdownString(`**${kind.replace("_", " ")} \`${entity.id}\`** · ${entity.status}\n\n${entity.description ?? ""}`), stateIcon(glyphs[kind], entity.status), kind === "zone" || kind === "flow" ? { command: "foxx.openEntity", title: "Open source", arguments: [kind, entity.id] } : undefined)), String(items.length), undefined, new vscode.ThemeIcon(glyphs[kind]));
    };
    return [new Node("Policies", policyNodes, String(policyNodes.length), undefined, new vscode.ThemeIcon("law")), section("zone", "Zones"), section("flow", "Flows"), section("context_pack", "Context Packs")];
  }
}

const tree = new GovernanceTree();
const codeLenses = new SelectionLenses();

/**
 * Read .crane/governance.yaml ("" when missing).
 * @returns content
 */
function readGovernance(): string {
  const file = path.join(root() ?? "", ".crane", "governance.yaml");
  return fs.existsSync(file) ? fs.readFileSync(file, "utf8") : "";
}

/**
 * Write a governance change made in the Foxx panel (the person already filled in every value),
 * validate it with Crane, and sign it with 'crane governance sign' (only admins can; an unsigned
 * edit stops agents, since Crane cannot tell it from tampering). The previous file is restored
 * when validation or signing fails; a kept change can be undone with "Foxx: Undo Last Governance
 * Change".
 * @param change edit of the current text
 * @param summary what changed, for the person
 * @returns whether it was kept, and what to tell the person
 */
async function writeGovernance(change: (text: string) => string, summary: string, step: Step = quietStep): Promise<RunResult> {
  const before = readGovernance();
  let after: string;
  try {
    after = change(before);
  } catch (error) {
    return { ok: false, message: (error as Error).message };
  }
  const file = path.join(root() ?? "", ".crane", "governance.yaml");
  const restore = async (reason: string, failure: "VALIDATION_FAILED" | "SIGN_FAILED"): Promise<RunResult> => {
    await step("revert", "Restore the previous governance.yaml", async () => {
      fs.writeFileSync(file, before, "utf8");
      await validate(true);
    });
    record("FOXX_GOVERNANCE_REVERTED", { status: "REVERTED", failure_state: failure, statement: "Governance change reverted; it did not pass." });
    const admin = /admin|owner|not allowed|signing key/i.test(reason);
    return {
      ok: false,
      message: `The change was not kept: ${reason}`,
      hint: failure === "SIGN_FAILED" ? (admin ? "Only an admin of this repository can sign governance. Ask the owner to add your key with 'crane admin add'." : "Signing failed, so governance.yaml was restored to its signed version.") : "governance.yaml was restored to its signed version.",
      actions: [{ id: "open:output", label: "Show Crane output", kind: "command" }],
    };
  };
  const relative = ".crane/governance.yaml";
  await step("write", `Write ${relative}`, async () => fs.writeFileSync(file, after, "utf8"));
  // The file is unsigned until signed below, so that one finding is expected here
  const blocking = (report: ValidateReport | undefined) => report?.findings.filter((finding) => (finding.severity === "HIGH" || finding.severity === "CRITICAL") && finding.code !== "governance_yaml_changed" && (finding.code.startsWith("governance") || finding.code.startsWith("flow") || finding.code.startsWith("selector"))) ?? [];
  const report = await step("validate", "Validate governance (crane validate)", () => validate(true), (report) => ({ ok: Boolean(report) && blocking(report).length === 0, detail: report ? blocking(report).map((finding) => finding.message).join("; ") || undefined : "validation did not run" }));
  const errors = blocking(report);
  if (!report || errors.length) return restore(errors.map((finding) => finding.message).join("; ") || "validation did not run", "VALIDATION_FAILED");
  const signed = await step("sign", "Sign with your admin key (crane governance sign)", () => crane(["governance", "sign"]), (signed) => ({ ok: Boolean(signed) && signed!.code === 0, detail: signed && signed.code !== 0 ? (signed.stderr || signed.stdout).trim().replace(/^crane: /, "") : undefined }));
  if (!signed || signed.code !== 0) return restore((signed?.stderr || signed?.stdout || "crane governance sign did not run").trim().replace(/^crane: /, ""), "SIGN_FAILED");
  lastGovernanceText = before;
  await step("refresh", "Refresh selections and decorations", () => validate(true));
  tree.refresh();
  return {
    ok: true,
    message: `${summary[0].toUpperCase()}${summary.slice(1)}.`,
    hint: "Signed and saved in .crane/governance.yaml. Commit .crane to share it with your team.",
    actions: [{ id: "undo", label: "Undo", kind: "run" }, { id: "open:governance", label: "Open governance.yaml", kind: "command" }],
  };
}

/**
 * Ask for one of a list of values.
 * @param values values
 * @param placeHolder prompt
 * @returns the value, or undefined when cancelled
 */
async function pick<T extends string>(values: readonly T[], placeHolder: string): Promise<T | undefined> {
  return (await vscode.window.showQuickPick([...values], { placeHolder })) as T | undefined;
}

/**
 * Register the commands, views, and listeners.
 * @param context extension context
 */
export function activate(context: vscode.ExtensionContext): void {
  output = vscode.window.createOutputChannel("Foxx");
  diagnostics = vscode.languages.createDiagnosticCollection("crane");
  const register = (command: string, callback: (...args: unknown[]) => unknown) => context.subscriptions.push(vscode.commands.registerCommand(command, callback));
  // The title button passes the editor's file, so only the Explorer entry opens a whole file
  extensionVersion = String(context.extension.packageJSON.version ?? "");
  // The title button passes the editor's file, the status bar passes STATUS_BAR, a keybinding nothing
  register("foxx.menu", (arg) => openPanel(undefined, arg instanceof vscode.Uri ? "TITLE_BUTTON" : arg === "STATUS_BAR" || arg === "SIDEBAR" ? arg : "KEYBINDING"));
  register("foxx.openFile", (uri) => openPanel(uri instanceof vscode.Uri ? uri : undefined, "EXPLORER"));
  register("foxx.setup", () => setUp());
  register("foxx.createPolicy", () => createPolicy());
  register("foxx.connectZonesFlow", () => connectZonesFlow());
  register("foxx.openEntity", (kind, id) => {
    if ((kind === "zone" || kind === "flow") && typeof id === "string") return openGovernanceEntity(kind, id);
    return undefined;
  });
  register("foxx.createCheckpoint", () => createCheckpoint());
  register("foxx.moveToPolicy", () => moveToPolicy());
  register("foxx.installHooks", () => installHooks());
  register("foxx.validate", () => validate());
  register("foxx.test", () => test());
  register("foxx.refresh", () => validate(true));
  register("foxx.revealSelection", (id, open) => revealSelection(String(id), open === true));
  register("foxx.openSidebar", () => vscode.commands.executeCommand("foxx.sidebar.focus"));
  register("foxx.policyStatus", async () => {
    const policy = await pick(policies().map((policy) => policy.name), "Policy");
    if (!policy) return;
    await crane(["policy", policy, "status"]);
    output.show(true);
  });
  register("foxx.undoGovernanceChange", async () => {
    const result = await runAction("undo", {}, undefined);
    (result.ok ? vscode.window.showInformationMessage : vscode.window.showErrorMessage)(`Foxx: ${result.message}`);
  });
  context.subscriptions.push(
    output,
    diagnostics,
    vscode.window.registerTreeDataProvider("foxx.governance", tree),
    vscode.window.registerWebviewViewProvider(FoxxSidebar.id, new FoxxSidebar(context.extensionUri, sidebarHost)),
    vscode.languages.registerCodeLensProvider({ scheme: "file" }, codeLenses),
    vscode.languages.registerHoverProvider({ scheme: "file" }, new SelectionHovers()),
    vscode.window.onDidChangeVisibleTextEditors(() => decorateAll()),
    vscode.workspace.onDidSaveTextDocument(async (document) => {
      if (!vscode.workspace.getConfiguration("foxx").get<boolean>("validateOnSave", true)) return;
      const report = await validate(true);
      if (!report) return;
      const failing = Object.values(report.selections).filter((span) => span === null).length;
      const serious = report.findings.some((finding) => finding.severity === "HIGH" || finding.severity === "CRITICAL");
      record("FOXX_VALIDATED_ON_SAVE", {
        status: report.findings.length ? "FINDINGS" : "PASS",
        statement: report.findings.length ? "Validation on save found problems." : "Validation on save passed.",
        assessment: failing ? "UNRESOLVED" : serious ? "VIOLATION" : "COMPLIANT",
        diagnostics: report.findings.length,
        selections_failing: failing,
        saved_file: document.uri.scheme === "file" ? relative(document.uri.fsPath) : undefined,
      });
    }),
  );
  extensionUri = context.extensionUri;
  context.subscriptions.push(
    vscode.window.onDidChangeTextEditorSelection(async (event) => {
      if (event.textEditor.document.uri.scheme !== "file") return;
      lastSelection = await describeSelection(event.textEditor);
      FoxxPanel.select(lastSelection);
    }),
  );
  status = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left, 50);
  status.command = { command: "foxx.menu", title: "Foxx", arguments: ["STATUS_BAR"] };
  context.subscriptions.push(status);
  updateStatus();
  void vscode.commands.executeCommand("setContext", "foxx.ready", Boolean(root()));
  void checkCrane().then((checked) => {
    updateStatus();
    if (checked.state !== "ok") void warnCrane(checked);
    else if (root()) void validate(true);
  });
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration((event) => {
      if (!event.affectsConfiguration("foxx.cranePath")) return;
      craneChecked = undefined;
      craneKnown = undefined;
      craneWarned = false;
      void checkCrane().then((checked) => {
        updateStatus();
        if (checked.state !== "ok") void warnCrane(checked);
        else if (root()) void validate(true);
      });
    }),
  );
}

/** Nothing to clean up beyond the disposables. */
export function deactivate(): void {}
