// Foxx for VS Code: set up Crane in a repository, create policies, checkpoints, and selection
// contracts from the editor, see where selections are and whether they pass, and manage zones,
// flows, and context packs in .crane/governance.yaml.
// Everything runs through the Crane CLI; decorations never modify source files, and governance
// edits are validated and signed by Crane (only admins can sign) and reversible.

import { execFile } from "child_process";
import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { CommandResult, Finding, parseJson, runCrane, selectionArgs, selectionIdFrom, Span, TestReport, ValidateReport } from "./core/crane";
import { activate as activateEntity, addContext, addFlow, addZone, attachToZone, entities, ZoneInput } from "./core/governance";
import { parsePolicies, Policy } from "./core/policies";
import { checkName, confirmation, ownerKey, repoArgs } from "./core/setup";
import { FoxxPanel, PanelHost, PanelState, SelectionInfo } from "./panel";

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

/** Where the panel was opened from (DASHBOARD.md F31.2). */
type OpenedFrom = "TITLE_BUTTON" | "KEYBINDING" | "EXPLORER" | "STATUS_BAR";

/**
 * Record an event of the editor side (DASHBOARD.md F31) with 'crane foxx event'. It runs in the
 * background, writes nothing to the Foxx output, and never changes or delays what the person did.
 * @param name event name (FOXX_...)
 * @param fields attributes as key=value; empty values are left out
 */
function record(name: string, fields: Record<string, string | number | boolean | undefined>): void {
  const directory = root();
  if (!directory) return;
  const executable = vscode.workspace.getConfiguration("foxx").get<string>("cranePath", "crane");
  const pairs = Object.entries({ ...fields, extension_version: extensionVersion })
    .filter(([, value]) => value !== undefined && value !== "")
    .map(([key, value]) => `${key}=${String(value).replace(/\s+/g, " ").slice(0, 200)}`);
  void runCrane(executable, directory, ["foxx", "event", name, ...pairs]);
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

/** The extension side of the Foxx panel. */
const panelHost: PanelHost = {
  state(): PanelState {
    const ready = Boolean(root());
    return {
      ready,
      policies: ready ? policies().map((policy) => policy.name) : [],
      entities: ready
        ? entities(readGovernance())
            .filter((entity) => entity.kind !== "context_pack")
            .map((entity) => ({ kind: entity.kind as "zone" | "flow", id: entity.id, status: entity.status }))
        : [],
      selections: ready ? loadRecords().map((record) => ({ id: record.id, label: `${record.operation}${record.name ? ` ${record.name}` : ""}`, policy: record.policy })) : [],
    };
  },
  run: async (action, values, selection) => {
    const started = Date.now();
    const result = await runAction(action, values, selection);
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
};

/**
 * Run an action chosen in the Foxx panel.
 * @param action action id (core/actions)
 * @param values the form's values
 * @param selection the selected code, when any
 * @returns whether it worked, and what to tell the person
 */
async function runAction(action: string, values: Record<string, string | boolean>, selection: SelectionInfo | undefined): Promise<{ ok: boolean; message: string }> {
  const value = (key: string) => (typeof values[key] === "string" ? (values[key] as string).trim() : "");
  const lines = value("context").split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const outcome = async (args: string[], success: string) => {
    const result = await crane(args);
    if (!result) return { ok: false, message: "Crane is not set up in this repository." };
    if (result.code !== 0) return { ok: false, message: (result.stderr || result.stdout).trim().replace(/^crane: /, "") };
    await validate(true);
    return { ok: true, message: success };
  };
  const needSelection = () => (!selection || selection.empty ? { ok: false, message: "Select the lines in the editor first." } : undefined);
  // What a zone or flow covers: the enclosing function when chosen and known, else the file
  const cover = (): Partial<Record<"symbols" | "files", string[]>> =>
    values.cover === "symbol" && selection?.symbol ? { symbols: [`${selection.file}:${selection.symbol}`] } : { files: [selection?.file ?? ""] };
  switch (action) {
    case "preserve":
    case "target": {
      const missing = needSelection();
      if (missing || !selection) return missing ?? { ok: false, message: "" };
      const document = vscode.workspace.textDocuments.find((open) => open.uri.toString() === selection.uri);
      if (document?.isDirty) await document.save();
      const operation = action === "preserve" ? "preserve" : "target";
      const result = await crane(selectionArgs(operation, selection.file, selection.start, selection.end, { policy: value("policy") || undefined, changeType: value("changeType") || undefined, name: value("name") || undefined }));
      if (!result) return { ok: false, message: "Crane is not set up in this repository." };
      const id = selectionIdFrom(result.stdout);
      if (result.code !== 0 || !id) return { ok: false, message: (result.stderr || result.stdout).trim().replace(/^crane: /, "") };
      await validate(true);
      return { ok: true, message: `${operation === "preserve" ? "Preserved" : "Targeted"} ${selection.file} lines ${selection.start}-${selection.end} as ${id}. Crane inserted its anchor comments.` };
    }
    case "zone": {
      const missing = needSelection();
      if (missing) return missing;
      const id = value("id");
      const zone: ZoneInput = { id, description: value("description"), criticality: value("criticality") as ZoneInput["criticality"], autonomy_ceiling: value("ceiling") as ZoneInput["autonomy_ceiling"], selectors: cover() };
      return writeGovernance((text) => {
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
      return writeGovernance((text) => {
        let next = addFlow(text, { id, description: value("description"), criticality: value("criticality") as ZoneInput["criticality"], autonomy_ceiling: ceiling, entry_points: [entry] });
        if (lines.length) next = addContext(next, "flow", id, lines);
        return values.enforce ? activateEntity(next, "flow", id) : next;
      }, `flow ${id} created from ${entry}${values.enforce ? " and enforced" : " as a proposal"}, autonomy ceiling ${ceiling}`);
    }
    case "context": {
      const [kind, id] = value("target").split(":");
      if (!id) return { ok: false, message: "Choose a zone or flow." };
      return writeGovernance((text) => addContext(text, kind as "zone" | "flow", id, lines), `context added to ${kind} ${id}; the agent receives it whenever a tool call touches that code`);
    }
    case "zone_attach": {
      const missing = needSelection();
      if (missing) return missing;
      const zone = value("zone");
      if (!zone) return { ok: false, message: "Choose a zone." };
      const [kind, target] = Object.entries(cover())[0] as ["symbols" | "files", string[]];
      return writeGovernance((text) => attachToZone(text, zone, kind, target[0]), `${target[0]} added to zone ${zone}`);
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
      return outcome(["add", "policy", value("policy"), "marker", value("selection")], `${value("selection")} moved to policy ${value("policy")}.`);
    case "checkpoint": {
      const error = checkName(value("name"));
      if (error) return { ok: false, message: `Checkpoint name: ${error}` };
      return outcome(["checkpoint", value("name")], `Checkpoint ${value("name")} recorded.`);
    }
    case "hooks":
      return outcome(["agent", "install", "--profile", value("profile") || "claude"], "Hooks installed; Crane now checks every tool call.");
    case "setup":
      await setUp();
      return { ok: true, message: root() ? "Crane is set up." : "Setup did not finish; see the Foxx output." };
    case "validate":
      await validate(false);
      return { ok: true, message: "Validation finished; findings are shown as problems." };
    case "test":
      await test();
      return { ok: true, message: "Policy tests finished; failures are shown as problems." };
    case "status":
      await vscode.commands.executeCommand("foxx.policyStatus");
      return { ok: true, message: "Policy status is in the Foxx output." };
    default:
      return { ok: false, message: `Unknown action ${action}.` };
  }
}

/**
 * Show the repository's state in the status bar.
 * @param report the latest validation, when there is one
 */
function updateStatus(report?: ValidateReport): void {
  if (!root()) {
    status.text = "$(shield) Foxx: set up";
    status.tooltip = "Crane is not set up in this repository. Click to set it up.";
  } else if (!report) {
    status.text = "$(shield) Foxx";
    status.tooltip = "Foxx actions";
  } else {
    const errors = report.findings.filter((finding) => finding.severity === "HIGH" || finding.severity === "CRITICAL").length;
    status.text = errors ? `$(warning) Foxx: ${errors} issue${errors === 1 ? "" : "s"}` : "$(shield) Foxx";
    status.tooltip = `${Object.keys(report.selections).length} selections, registry generation ${report.generation}. Click for Foxx actions.`;
  }
  status.show();
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
  const executable = vscode.workspace.getConfiguration("foxx").get<string>("cranePath", "crane");
  output.appendLine(`$ crane ${args.join(" ")}`);
  const result = await runCrane(executable, directory, args, input);
  if (result.stdout.trim()) output.appendLine(result.stdout.trimEnd());
  if (result.stderr.trim()) output.appendLine(result.stderr.trimEnd());
  return result;
}

/**
 * Run Crane in the repository.
 * @param args arguments
 * @returns result, or undefined when there is no repository
 */
async function crane(args: string[]) {
  const directory = root();
  if (!directory) {
    vscode.window.showErrorMessage("Foxx: Crane is not set up in this folder; run 'Foxx: Set Up Crane in This Repository'.");
    return undefined;
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
  if (!result) return false;
  if (result.code !== 0) {
    vscode.window.showErrorMessage(`Foxx: ${(result.stderr || result.stdout).trim().replace(/^crane: /, "")}`);
    return false;
  }
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
  );
  if (hooks) await installHooks(hooks === "Codex" ? "codex" : "claude");
}

/**
 * Install the Crane hooks for an agent.
 * @param kind agent profile, or undefined to ask
 */
async function installHooks(kind?: "claude" | "codex"): Promise<void> {
  const profile = kind ?? (await pick(["claude", "codex"] as const, "Which agent?"));
  if (!profile) return;
  await change(["agent", "install", "--profile", profile], `hooks installed for ${profile === "claude" ? "Claude Code" : "Codex"}; Crane now checks every tool call.`);
}

/** Record HEAD as a trusted checkpoint. */
async function createCheckpoint(): Promise<void> {
  const name = await vscode.window.showInputBox({ prompt: "Checkpoint name (records the current commit as trusted)", value: `checkpoint-${new Date().toISOString().slice(0, 10)}`, validateInput: (value) => checkName(value) });
  if (!name) return;
  await change(["checkpoint", name], `checkpoint ${name} recorded.`);
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
  const report = parseJson<ValidateReport>(result);
  if (!report) {
    vscode.window.showErrorMessage(`Foxx: crane validate failed: ${result.stderr.trim() || "no JSON output"}`);
    return undefined;
  }
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
async function test(): Promise<void> {
  const result = await crane(["test", ".", "--json"]);
  const report = result && parseJson<TestReport>(result);
  if (!report) {
    vscode.window.showErrorMessage("Foxx: crane test did not produce a report; see the Foxx output.");
    return;
  }
  const commands = report.policies.flatMap((policy) => policy.commands);
  outcomes.clear();
  for (const command of commands) outcomes.set(command.id, command.outcome);
  publish(report.findings, commands);
  codeLenses.refresh();
  tree.refresh();
  vscode.window.showInformationMessage(`Foxx: ${report.passed ? "all policies pass" : `${report.policies.filter((policy) => !policy.passed).length} policies fail`}`);
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
    const file = relative(document.uri.fsPath);
    return records
      .filter((record) => spans[record.id]?.path === file)
      .map((record) => {
        const span = spans[record.id] as Span;
        const outcome = outcomes.get(record.id);
        return new vscode.CodeLens(new vscode.Range(Math.max(0, span.start_line - 2), 0, Math.max(0, span.start_line - 2), 0), {
          title: `${record.operation} ${record.id} · ${record.policy}${outcome ? ` · ${outcome}` : ""}`,
          command: "foxx.test",
        });
      });
  }
}

/** A node of the governance tree. */
class Node extends vscode.TreeItem {
  constructor(label: string, readonly children: Node[] = [], description?: string, tooltip?: string) {
    super(label, children.length ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None);
    this.description = description;
    this.tooltip = tooltip;
  }
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
    const policyNodes = policies().map(
      (policy) =>
        new Node(
          policy.name,
          policy.commands.map((command) => {
            const span = spans[command.id];
            return new Node(`${command.operation} ${command.id}`, [], span ? `${span.path}:${span.start_line}-${span.end_line}` : "UNRESOLVED", outcomes.get(command.id));
          }),
          policy.file,
        ),
    );
    let governed: ReturnType<typeof entities> = [];
    try {
      governed = entities(readGovernance());
    } catch (error) {
      return [new Node("Policies", policyNodes), new Node("governance.yaml is invalid", [], String(error))];
    }
    const section = (kind: string, label: string) =>
      new Node(label, governed.filter((entity) => entity.kind === kind).map((entity) => new Node(entity.id, [], entity.status, entity.description)));
    return [new Node("Policies", policyNodes), section("zone", "Zones"), section("flow", "Flows"), section("context_pack", "Context Packs")];
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
async function writeGovernance(change: (text: string) => string, summary: string): Promise<{ ok: boolean; message: string }> {
  const before = readGovernance();
  let after: string;
  try {
    after = change(before);
  } catch (error) {
    return { ok: false, message: (error as Error).message };
  }
  const file = path.join(root() ?? "", ".crane", "governance.yaml");
  const restore = async (reason: string, failure: "VALIDATION_FAILED" | "SIGN_FAILED") => {
    fs.writeFileSync(file, before, "utf8");
    record("FOXX_GOVERNANCE_REVERTED", { status: "REVERTED", failure_state: failure, statement: "Governance change reverted; it did not pass." });
    await validate(true);
    return { ok: false, message: `The change was not kept: ${reason}` };
  };
  fs.writeFileSync(file, after, "utf8");
  // The file is unsigned until signed below, so that one finding is expected here
  const report = await validate(true);
  const errors = report?.findings.filter((finding) => (finding.severity === "HIGH" || finding.severity === "CRITICAL") && finding.code !== "governance_yaml_changed" && (finding.code.startsWith("governance") || finding.code.startsWith("flow") || finding.code.startsWith("selector"))) ?? [];
  if (!report || errors.length) return restore(errors.map((finding) => finding.message).join("; ") || "validation did not run", "VALIDATION_FAILED");
  const signed = await crane(["governance", "sign"]);
  if (!signed || signed.code !== 0) return restore((signed?.stderr || signed?.stdout || "crane governance sign did not run").trim().replace(/^crane: /, ""), "SIGN_FAILED");
  lastGovernanceText = before;
  await validate(true);
  tree.refresh();
  return { ok: true, message: `${summary[0].toUpperCase()}${summary.slice(1)}. Signed and saved in .crane/governance.yaml; commit .crane to share it.` };
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
  register("foxx.menu", (arg) => openPanel(undefined, arg instanceof vscode.Uri ? "TITLE_BUTTON" : arg === "STATUS_BAR" ? "STATUS_BAR" : "KEYBINDING"));
  register("foxx.openFile", (uri) => openPanel(uri instanceof vscode.Uri ? uri : undefined, "EXPLORER"));
  register("foxx.setup", () => setUp());
  register("foxx.createPolicy", () => createPolicy());
  register("foxx.createCheckpoint", () => createCheckpoint());
  register("foxx.moveToPolicy", () => moveToPolicy());
  register("foxx.installHooks", () => installHooks());
  register("foxx.validate", () => validate());
  register("foxx.test", () => test());
  register("foxx.refresh", () => validate(true));
  register("foxx.policyStatus", async () => {
    const policy = await pick(policies().map((policy) => policy.name), "Policy");
    if (!policy) return;
    await crane(["policy", policy, "status"]);
    output.show(true);
  });
  register("foxx.undoGovernanceChange", async () => {
    const previous = lastGovernanceText;
    if (previous === undefined) {
      record("FOXX_GOVERNANCE_UNDONE", { status: "NOTHING_TO_UNDO", statement: "No governance change to undo." });
      vscode.window.showInformationMessage("Foxx: there is no governance change to undo.");
      return;
    }
    const result = await writeGovernance(() => previous, "the last governance change was undone");
    if (result.ok) record("FOXX_GOVERNANCE_UNDONE", { status: "UNDONE", statement: "Last governance change undone and signed." });
    lastGovernanceText = undefined;
    (result.ok ? vscode.window.showInformationMessage : vscode.window.showErrorMessage)(`Foxx: ${result.message}`);
  });
  context.subscriptions.push(
    output,
    diagnostics,
    vscode.window.registerTreeDataProvider("foxx.governance", tree),
    vscode.languages.registerCodeLensProvider({ scheme: "file" }, codeLenses),
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
  if (root()) void validate(true);
}

/** Nothing to clean up beyond the disposables. */
export function deactivate(): void {}
