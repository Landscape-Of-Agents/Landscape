// Foxx for VS Code: create selection contracts from the editor, see where selections are and
// whether they pass, and manage zones, flows, and context packs in .crane/governance.yaml.
// Everything runs through the Crane CLI; decorations never modify source files, and governance
// edits are previewed, validated by Crane, reversible, and created as proposals.

import * as fs from "fs";
import * as path from "path";
import * as vscode from "vscode";

import { CommandResult, Finding, parseJson, runCrane, selectionArgs, selectionIdFrom, Span, TestReport, ValidateReport } from "./core/crane";
import { activate as activateEntity, addFlow, addZone, attachContextPack, attachToFlow, attachToZone, CEILINGS, checkId, CRITICALITY, entities, SELECTOR_KINDS } from "./core/governance";
import { parsePolicies, Policy } from "./core/policies";

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
 * Run Crane in the repository and log the invocation.
 * @param args arguments
 * @returns result, or undefined when there is no repository
 */
async function crane(args: string[]) {
  const directory = root();
  if (!directory) {
    vscode.window.showErrorMessage("Foxx: no folder with a .crane directory is open; run 'crane init' first.");
    return undefined;
  }
  const executable = vscode.workspace.getConfiguration("foxx").get<string>("cranePath", "crane");
  output.appendLine(`$ crane ${args.join(" ")}`);
  const result = await runCrane(executable, directory, args);
  if (result.stdout.trim()) output.appendLine(result.stdout.trimEnd());
  if (result.stderr.trim()) output.appendLine(result.stderr.trimEnd());
  return result;
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
 * Preview a governance change as a diff, write it when confirmed, validate it with Crane, and
 * restore the previous content when validation fails.
 * @param change produces the new content from the current one
 * @param summary description for the confirmation
 */
async function applyGovernance(change: (text: string) => string, summary: string): Promise<void> {
  const before = readGovernance();
  let after: string;
  try {
    after = change(before);
  } catch (error) {
    vscode.window.showErrorMessage(`Foxx: ${(error as Error).message}`);
    return;
  }
  const left = await vscode.workspace.openTextDocument({ content: before, language: "yaml" });
  const right = await vscode.workspace.openTextDocument({ content: after, language: "yaml" });
  await vscode.commands.executeCommand("vscode.diff", left.uri, right.uri, "governance.yaml: proposed change");
  const choice = await vscode.window.showInformationMessage(`Foxx: ${summary}. Write this change to .crane/governance.yaml?`, { modal: true }, "Write and validate");
  if (choice !== "Write and validate") return;
  const file = path.join(root() ?? "", ".crane", "governance.yaml");
  fs.writeFileSync(file, after, "utf8");
  lastGovernanceText = before;
  const report = await validate(true);
  const errors = report?.findings.filter((finding) => (finding.severity === "HIGH" || finding.severity === "CRITICAL") && (finding.code.startsWith("governance") || finding.code.startsWith("flow") || finding.code.startsWith("selector"))) ?? [];
  if (!report || errors.length) {
    fs.writeFileSync(file, before, "utf8");
    lastGovernanceText = undefined;
    await validate(true);
    vscode.window.showErrorMessage(`Foxx: the change was not kept: ${errors.map((finding) => finding.message).join("; ") || "validation did not run"}`);
    return;
  }
  vscode.window.showInformationMessage(`Foxx: ${summary}. Review it in Git; new zones and flows stay proposed until activated.`);
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
 * Ask for an entity id of a kind from governance.yaml.
 * @param kind zone or flow
 * @returns id, or undefined
 */
async function pickEntity(kind: "zone" | "flow"): Promise<string | undefined> {
  const ids = entities(readGovernance()).filter((entity) => entity.kind === kind).map((entity) => entity.id);
  if (!ids.length) {
    vscode.window.showWarningMessage(`Foxx: there is no ${kind} yet; create one first.`);
    return undefined;
  }
  return pick(ids, `Choose a ${kind}`);
}

/**
 * Create a preserve or target selection from the active editor's selection.
 * @param fixed operation, or undefined to ask
 */
async function createSelection(fixed?: "preserve" | "target"): Promise<void> {
  const editor = vscode.window.activeTextEditor;
  if (!editor) return;
  if (editor.document.isDirty) {
    await editor.document.save();
  }
  const operation = fixed ?? (await pick(["preserve", "target"] as const, "What must the agent do with these lines?"));
  if (!operation) return;
  const policy = await pick(policies().map((policy) => policy.name), "Policy (Esc for the default policy)");
  let changeType: string | undefined;
  if (operation === "target") {
    const chosen = await pick(["any change", "logical_bn", "logical_cn", "logical_sn", "semantic"], "Required kind of change");
    if (!chosen) return;
    changeType = chosen === "any change" ? undefined : chosen;
  }
  const name = await vscode.window.showInputBox({ prompt: "Optional selection name", validateInput: (value) => (!value || /^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(value) ? undefined : "letters, digits, '_' or '-'") });
  if (name === undefined) return;
  const { start, end } = editor.selection;
  const endLine = end.character === 0 && end.line > start.line ? end.line : end.line + 1;
  const result = await crane(selectionArgs(operation, relative(editor.document.uri.fsPath), start.line + 1, endLine, { policy, changeType, name: name || undefined }));
  if (!result) return;
  const id = selectionIdFrom(result.stdout);
  if (result.code !== 0 || !id) {
    vscode.window.showErrorMessage(`Foxx: ${(result.stderr || result.stdout).trim()}`);
    return;
  }
  vscode.window.showInformationMessage(`Foxx: ${operation} ${id} created; Crane inserted its anchor comments.`);
  await validate(true);
}

/**
 * Register the commands, views, and listeners.
 * @param context extension context
 */
export function activate(context: vscode.ExtensionContext): void {
  output = vscode.window.createOutputChannel("Foxx");
  diagnostics = vscode.languages.createDiagnosticCollection("crane");
  const register = (command: string, callback: (...args: unknown[]) => unknown) => context.subscriptions.push(vscode.commands.registerCommand(command, callback));
  register("foxx.createSelectionContract", () => createSelection());
  register("foxx.protectSelection", () => createSelection("preserve"));
  register("foxx.targetSelection", () => createSelection("target"));
  register("foxx.validate", () => validate());
  register("foxx.test", () => test());
  register("foxx.refresh", () => validate(true));
  register("foxx.openDashboard", () => {
    // The dashboard is hosted by the Foxx service; sign in there with GitHub
    const url = vscode.workspace.getConfiguration("foxx").get<string>("dashboardUrl", "https://agentscript-service.onrender.com");
    void vscode.env.openExternal(vscode.Uri.parse(url));
  });
  register("foxx.policyStatus", async () => {
    const policy = await pick(policies().map((policy) => policy.name), "Policy");
    if (!policy) return;
    await crane(["policy", policy, "status"]);
    output.show(true);
  });
  register("foxx.attachPolicyContext", async (uri?: unknown) => {
    const file = uri instanceof vscode.Uri ? uri.fsPath : vscode.window.activeTextEditor?.document.uri.fsPath;
    if (!file) return;
    const policy = await pick(policies().map((policy) => policy.name), "Attach as context of which policy?");
    if (!policy) return;
    const result = await crane(["add", "policy-context", "file", relative(file), "policy", policy]);
    if (result) (result.code === 0 ? vscode.window.showInformationMessage : vscode.window.showErrorMessage)(`Foxx: ${(result.stdout || result.stderr).trim()}`);
  });
  register("foxx.createZone", async (uri?: unknown) => {
    const id = await vscode.window.showInputBox({ prompt: "Zone id", validateInput: (value) => checkId(value) });
    if (!id) return;
    const description = (await vscode.window.showInputBox({ prompt: "Description" })) ?? "";
    const owner = await vscode.window.showInputBox({ prompt: "Owner (optional)" });
    const criticality = await pick(CRITICALITY, "Criticality");
    const ceiling = criticality && (await pick(CEILINGS, "Autonomy ceiling"));
    if (!criticality || !ceiling) return;
    const target = uri instanceof vscode.Uri ? uri.fsPath : vscode.window.activeTextEditor?.document.uri.fsPath;
    const selectors: Record<string, string[]> = {};
    if (target) {
      const isDirectory = fs.statSync(target).isDirectory();
      selectors[isDirectory ? "directories" : "files"] = [relative(target)];
    }
    await applyGovernance((text) => addZone(text, { id, description, owner: owner || undefined, criticality, autonomy_ceiling: ceiling, selectors }), `create zone ${id} (proposed)`);
  });
  register("foxx.createFlow", async () => {
    const id = await vscode.window.showInputBox({ prompt: "Flow id", validateInput: (value) => checkId(value) });
    if (!id) return;
    const editor = vscode.window.activeTextEditor;
    const word = editor ? editor.document.getText(editor.document.getWordRangeAtPosition(editor.selection.active)) : "";
    const entry = await vscode.window.showInputBox({ prompt: "Entry point (Symbol, Type.Symbol, or path:Symbol)", value: editor && word ? `${relative(editor.document.uri.fsPath)}:${word}` : "" });
    if (!entry) return;
    const description = (await vscode.window.showInputBox({ prompt: "Description" })) ?? "";
    const criticality = await pick(CRITICALITY, "Criticality");
    const ceiling = criticality && (await pick(CEILINGS, "Autonomy ceiling"));
    if (!criticality || !ceiling) return;
    await applyGovernance((text) => addFlow(text, { id, description, criticality, autonomy_ceiling: ceiling, entry_points: [entry] }), `create flow ${id} (proposed)`);
  });
  register("foxx.attachToZone", async (uri?: unknown) => {
    const zone = await pickEntity("zone");
    if (!zone) return;
    const editor = vscode.window.activeTextEditor;
    const target = uri instanceof vscode.Uri ? uri.fsPath : editor?.document.uri.fsPath;
    const kind = await pick(SELECTOR_KINDS, "Selector kind");
    if (!kind) return;
    const suggestion = kind === "symbols" && editor ? `${relative(editor.document.uri.fsPath)}:${editor.document.getText(editor.document.getWordRangeAtPosition(editor.selection.active))}` : target ? relative(target) : "";
    const value = await vscode.window.showInputBox({ prompt: `Value for ${kind}`, value: suggestion });
    if (!value) return;
    await applyGovernance((text) => attachToZone(text, zone, kind, value), `attach ${value} to zone ${zone}`);
  });
  register("foxx.attachToFlow", async () => {
    const flow = await pickEntity("flow");
    if (!flow) return;
    const field = await pick(["entry_points", "include", "exclude"] as const, "What to attach");
    if (!field) return;
    const editor = vscode.window.activeTextEditor;
    const value = await vscode.window.showInputBox({ prompt: field === "entry_points" ? "Entry point (path:Symbol)" : "Path glob", value: editor ? relative(editor.document.uri.fsPath) : "" });
    if (!value) return;
    await applyGovernance((text) => attachToFlow(text, flow, field, value), `attach ${value} to flow ${flow}`);
  });
  register("foxx.attachContextPack", async () => {
    const kind = await pick(["zone", "flow"] as const, "Attach to a zone or a flow?");
    const id = kind && (await pickEntity(kind));
    if (!kind || !id) return;
    const pack = await vscode.window.showInputBox({ prompt: "Context pack id (existing or new)", validateInput: (value) => checkId(value) });
    if (!pack) return;
    const line = await vscode.window.showInputBox({ prompt: "First guidance line for a new pack (optional)" });
    await applyGovernance((text) => attachContextPack(text, kind, id, pack, line ? [line] : []), `attach context pack ${pack} to ${kind} ${id}`);
  });
  register("foxx.activateEntity", async () => {
    const kind = await pick(["zone", "flow"] as const, "Activate a zone or a flow?");
    const id = kind && (await pickEntity(kind));
    if (!kind || !id) return;
    await applyGovernance((text) => activateEntity(text, kind, id), `activate ${kind} ${id} (it will be enforced for agents)`);
  });
  register("foxx.previewImpact", async () => {
    const report = await validate(true);
    if (!report) return;
    const governanceFindings = report.findings.filter((finding) => /^(governance|flow|selector|scope|resources|context)/.test(finding.code));
    output.appendLine("Impact preview:");
    for (const finding of governanceFindings) output.appendLine(`  ${finding.severity} [${finding.code}] ${finding.message}`);
    if (!governanceFindings.length) output.appendLine("  no zone, flow, or context findings");
    output.show(true);
  });
  register("foxx.undoGovernanceChange", async () => {
    if (lastGovernanceText === undefined) {
      vscode.window.showInformationMessage("Foxx: there is no governance change to undo.");
      return;
    }
    fs.writeFileSync(path.join(root() ?? "", ".crane", "governance.yaml"), lastGovernanceText, "utf8");
    lastGovernanceText = undefined;
    await validate(true);
    vscode.window.showInformationMessage("Foxx: the last governance change was undone.");
  });
  register("foxx.createProposalBranch", async () => {
    const name = await vscode.window.showInputBox({ prompt: "Branch for the governance proposal", value: `foxx/governance-${new Date().toISOString().slice(0, 10)}`, validateInput: (value) => (/^[A-Za-z0-9._/-]+$/.test(value) ? undefined : "invalid branch name") });
    if (!name) return;
    const confirm = await vscode.window.showWarningMessage(`Create branch ${name} and commit .crane/governance.yaml?`, { modal: true }, "Create and commit");
    if (confirm !== "Create and commit") return;
    const terminal = vscode.window.createTerminal({ name: "Foxx proposal", cwd: root() });
    terminal.sendText(`git checkout -b ${name} && git add .crane/governance.yaml && git commit -m "Foxx: governance proposal"`);
    terminal.show();
  });
  context.subscriptions.push(
    output,
    diagnostics,
    vscode.window.registerTreeDataProvider("foxx.governance", tree),
    vscode.languages.registerCodeLensProvider({ scheme: "file" }, codeLenses),
    vscode.window.onDidChangeVisibleTextEditors(() => decorateAll()),
    vscode.workspace.onDidSaveTextDocument(() => {
      if (vscode.workspace.getConfiguration("foxx").get<boolean>("validateOnSave", true)) void validate(true);
    }),
  );
  void validate(true);
}

/** Nothing to clean up beyond the disposables. */
export function deactivate(): void {}
