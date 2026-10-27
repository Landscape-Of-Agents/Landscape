// Bricks and Crane's command families in Landscape: the Bricks view (drafts, activated versions,
// Stacks, attachments with their Current Context, assigned agents), the commands that create,
// edit, validate, test, activate, attach, detach, upgrade, and revoke Bricks, the effective context
// preview, "Run a Crane Control" for the command families, and the resident agents attached to the
// workspace's Zones and Flows (the same assignments the dashboard shows, read from Crane). Crane is the authority: every
// action runs the CLI, every signing action is previewed first with '--dry-run' (what the guard
// will restrict before and after, and the snapshot agents will receive), and nothing is signed
// until the person confirms. Problems Crane reports become diagnostics on the files they concern.

import * as fs from "fs";
import * as os from "os";
import * as path from "path";
import * as vscode from "vscode";

import { CraneResult } from "./core/crane";
import {
  AttachmentRow,
  BRICK_TEMPLATES,
  BRICK_TYPES,
  BrickList,
  BrickRow,
  BrickType,
  ChangePreview,
  CONTROLS,
  controlArgs,
  describePreview,
  draftFile,
  Envelope,
  envelopeLine,
  parseEnvelope,
  SnapshotData,
  snapshotMarkdown,
  StackRow,
  subjectOf,
  succeeded,
  TypeInfo,
} from "./core/bricks";
import { assignmentDraft, checkAssignmentId, PackData, packMarkdown, RESIDENT_CHANGES, residentChanges, ResidentList, residentMarkdown, ResidentRow, residentsByTarget, residentTone } from "./core/residents";

/** What the Bricks view needs from the rest of Landscape. */
export interface BricksHost {
  /** Run Crane in the repository (undefined without one; code 126 when Crane cannot be used). */
  crane(args: string[]): Promise<CraneResult | undefined>;
  /** The repository root. */
  root(): string | undefined;
  /** The Zones and Flows of .crane/governance.yaml. */
  targets(): { kind: "zone" | "flow"; id: string; status: string }[];
  /** Record an editor event (DASHBOARD.md F31). */
  record(name: string, fields: Record<string, string | number | boolean | undefined>): void;
  /** Refresh validation, decorations, and the governance view after a signed change. */
  refresh(): Promise<void>;
  /** The Landscape output channel. */
  output: vscode.OutputChannel;
}

/** One node of the Bricks view. */
class BrickNode extends vscode.TreeItem {
  constructor(
    label: string,
    readonly children: BrickNode[],
    options: { description?: string; tooltip?: string | vscode.MarkdownString; icon?: vscode.ThemeIcon; contextValue?: string; data?: unknown; command?: vscode.Command } = {},
  ) {
    super(label, children.length ? vscode.TreeItemCollapsibleState.Collapsed : vscode.TreeItemCollapsibleState.None);
    this.description = options.description;
    this.tooltip = options.tooltip;
    this.iconPath = options.icon;
    this.contextValue = options.contextValue;
    this.data = options.data;
    this.command = options.command;
  }

  /** The row the node shows (a Brick, Stack, attachment, or agent). */
  data: unknown;
}

/**
 * A theme icon colored by a Brick state.
 * @param name codicon
 * @param state state
 * @returns icon
 */
function icon(name: string, state: string | undefined): vscode.ThemeIcon {
  const color =
    state === "active" || state === "passed" || state === "enabled" || state === "IDLE" || state === "RUNNING"
      ? "testing.iconPassed"
      : state === "revoked" || state === "failed" || state === "ERROR" || state === "REVOKED"
        ? "testing.iconFailed"
        : state === "draft" || state === "deprecated" || state === "not_run" || state === "DEGRADED" || state === "WAITING_FOR_APPROVAL" || state === "PAUSED"
          ? "editorWarning.foreground"
          : undefined;
  return new vscode.ThemeIcon(name, color ? new vscode.ThemeColor(color) : undefined);
}

/** Codicons per Brick type. */
const TYPE_ICONS: Record<BrickType, string> = {
  context: "book", identity: "person", scope: "list-tree", permission: "key", constraint: "lock", precondition: "checklist", approval: "verified",
  isolation: "vm", budget: "dashboard", action: "zap", verification: "pass", provenance: "history", recovery: "debug-restart", revocation: "circle-slash",
  monitoring: "pulse", intelligence: "lightbulb",
};

/** The Bricks view and its commands. */
export class Bricks implements vscode.TreeDataProvider<BrickNode> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  private list: BrickList | undefined;
  private agents: Record<string, unknown>[] = [];
  private residents: ResidentList | undefined;
  private problem: string | undefined;
  private types: TypeInfo[] = [];
  private readonly diagnostics = vscode.languages.createDiagnosticCollection("crane-bricks");

  constructor(private readonly host: BricksHost) {}

  /**
   * Register the view and the commands.
   * @param context extension context
   */
  register(context: vscode.ExtensionContext): void {
    const register = (command: string, callback: (...args: unknown[]) => unknown) => context.subscriptions.push(vscode.commands.registerCommand(command, callback));
    const view = vscode.window.createTreeView("foxx.bricks", { treeDataProvider: this });
    // Showing the Bricks view opens the Bricks panel beside the editor
    context.subscriptions.push(view, this.diagnostics, view.onDidChangeVisibility((event) => {
      if (event.visible) void vscode.commands.executeCommand("foxx.bricks.openPanel");
    }));
    register("foxx.bricks.refresh", () => this.load());
    register("foxx.bricks.create", () => this.create());
    register("foxx.bricks.edit", (node) => this.edit(node as BrickNode | undefined));
    register("foxx.bricks.validate", (node) => this.validate(node as BrickNode | undefined));
    register("foxx.bricks.test", (node) => this.test(node as BrickNode | undefined));
    register("foxx.bricks.show", (node, reference) => this.show(node as BrickNode | undefined, typeof reference === "string" ? reference : undefined));
    register("foxx.bricks.activate", (node) => this.activate(node as BrickNode | undefined));
    register("foxx.bricks.attach", (node, target) => this.attach(node as BrickNode | undefined, typeof target === "string" ? target : undefined));
    register("foxx.bricks.detach", (node) => this.detach(node as BrickNode | undefined));
    register("foxx.bricks.upgrade", (node) => this.upgrade(node as BrickNode | undefined));
    register("foxx.bricks.editContext", (node) => this.editContext(node as BrickNode | undefined));
    register("foxx.bricks.revoke", (node) => this.revoke(node as BrickNode | undefined));
    register("foxx.bricks.preview", (node) => this.preview(node as BrickNode | undefined));
    register("foxx.runControl", (family) => this.runControl(typeof family === "string" ? family : undefined));
    register("foxx.residents.show", (node) => this.showResident(node as BrickNode | undefined));
    register("foxx.residents.context", (node) => this.residentContext(node as BrickNode | undefined));
    register("foxx.residents.create", (node) => this.createResident(node as BrickNode | undefined));
    register("foxx.residents.validate", (node) => this.validateResident(node as BrickNode | undefined));
    register("foxx.residents.change", (node) => this.changeResident(node as BrickNode | undefined));
  }

  /**
   * Run a brick command with '--format json' and read its envelope.
   * @param args arguments after "crane"
   * @returns envelope, or undefined (the reason was shown)
   */
  private async envelope<T>(args: string[]): Promise<Envelope<T> | undefined> {
    const full = args.includes("--format") ? args : args.concat(["--format", "json"]);
    const result = await this.host.crane(full);
    if (!result || result.code === 126) return undefined;
    const envelope = parseEnvelope<T>(result.stdout);
    if (!envelope) {
      const message = (result.stderr || result.stdout).trim().replace(/^crane: /, "");
      const old = /unknown command 'brick'/.test(message);
      vscode.window.showErrorMessage(`Landscape: ${old ? "this Crane has no Bricks yet; install a newer Crane" : message || "Crane printed no result"}`);
      return undefined;
    }
    return envelope;
  }

  /** Reload the view from Crane. */
  async load(): Promise<void> {
    if (!this.host.root()) {
      this.list = undefined;
      this.changed.fire();
      return;
    }
    const result = await this.host.crane(["brick", "list", "--format", "json"]);
    const envelope = result ? parseEnvelope<BrickList>(result.stdout) : undefined;
    this.problem = envelope ? undefined : result && result.code !== 126 ? ((result.stderr || result.stdout).includes("unknown command 'brick'") ? "This Crane has no Bricks; install a newer Crane." : (result.stderr || result.stdout).trim().replace(/^crane: /, "")) : "Crane cannot be used.";
    this.list = envelope?.data;
    if (envelope && !this.types.length) {
      const types = await this.host.crane(["brick", "types", "--format", "json"]);
      this.types = (types && parseEnvelope<{ types: TypeInfo[] }>(types.stdout)?.data.types) || [];
    }
    if (envelope) {
      const agents = await this.host.crane(["brick", "agents", "--format", "json"]);
      this.agents = (agents && parseEnvelope<{ assignments: Record<string, unknown>[] }>(agents.stdout)?.data.assignments) || [];
      // An older Crane has no resident agents: the group then says so instead of failing the view
      const residents = await this.host.crane(["resident", "list", "--format", "json"]);
      this.residents = (residents && parseEnvelope<ResidentList>(residents.stdout)?.data) || undefined;
    }
    this.diagnose();
    this.changed.fire();
  }

  /** Turn what Crane reported into diagnostics: drafts that do not validate, pins Crane cannot honor, unreadable drafts. */
  private diagnose(): void {
    this.diagnostics.clear();
    const root = this.host.root();
    if (!root || !this.list) return;
    const byFile = new Map<string, vscode.Diagnostic[]>();
    const add = (file: string, message: string, severity: vscode.DiagnosticSeverity) => {
      const list = byFile.get(file) ?? [];
      const diagnostic = new vscode.Diagnostic(new vscode.Range(0, 0, 0, 1), message, severity);
      diagnostic.source = "crane brick";
      list.push(diagnostic);
      byFile.set(file, list);
    };
    for (const row of this.list.bricks) {
      if (row.state === "draft" && row.validation && row.validation !== "passed") add(path.join(root, ".crane", "drafts", "bricks", `${row.brick_id}.json`), `Draft ${row.brick_id} does not validate (${row.validation}); run 'Validate Brick' for the problems.`, vscode.DiagnosticSeverity.Warning);
      if (row.state === "revoked" && this.list.attachments.some((attachment) => attachment.subject === `brick:${row.ref}`)) add(path.join(root, ".crane", "bricks.json"), `${row.ref} is revoked but still attached; changes in its targets need approval until it is detached or upgraded.`, vscode.DiagnosticSeverity.Error);
    }
    for (const attachment of this.list.attachments) {
      for (const issue of attachment.issues) add(path.join(root, ".crane", "bricks.json"), `Attachment ${attachment.attachment_id} on ${attachment.target}: ${issue} (fails closed)`, vscode.DiagnosticSeverity.Error);
    }
    for (const problem of this.list.unreadable_drafts) add(path.join(root, ".crane", "drafts"), `Unreadable draft: ${problem}`, vscode.DiagnosticSeverity.Warning);
    if (this.list.store.trust.startsWith("unverified")) add(path.join(root, ".crane", "bricks.json"), `The Brick store does not verify (${this.list.store.trust}); every agent change is stopped until it does.`, vscode.DiagnosticSeverity.Error);
    for (const [file, list] of byFile) this.diagnostics.set(vscode.Uri.file(file), list);
  }

  /**
   * Return a node as a tree item.
   * @param node node
   * @returns the node
   */
  getTreeItem(node: BrickNode): vscode.TreeItem {
    return node;
  }

  /**
   * Return the children of a node, or the sections.
   * @param node parent
   * @returns children
   */
  getChildren(node?: BrickNode): BrickNode[] {
    if (node) return node.children;
    if (!this.host.root()) return [new BrickNode("Set up Crane to use Bricks", [], { icon: new vscode.ThemeIcon("info") })];
    if (!this.list) return [new BrickNode(this.problem ?? "Loading…", [], { icon: new vscode.ThemeIcon(this.problem ? "warning" : "loading~spin") })];
    const list = this.list;
    const brickNode = (row: BrickRow) => {
      const tooltip = new vscode.MarkdownString(`**${row.name}** · \`${row.ref}\` · ${row.type}\n\n${row.intent ?? ""}\n\nState **${row.state}**${row.validation ? ` · validation **${row.validation}**` : ""}`);
      return new BrickNode(row.name, [], { description: `${row.state === "draft" ? row.brick_id + " draft" : row.ref} · ${row.validation ?? ""}`, tooltip, icon: icon(TYPE_ICONS[row.type] ?? "package", row.state === "draft" ? "draft" : row.state), contextValue: `brick-${row.state}`, data: row, command: { command: "foxx.bricks.show", title: "Show", arguments: [undefined, row.ref] } });
    };
    const byType = BRICK_TYPES.map((type) => ({ type, rows: list.bricks.filter((row) => row.type === type) })).filter((entry) => entry.rows.length);
    const library = new BrickNode("Library", byType.map((entry) => new BrickNode(entry.type, entry.rows.map(brickNode), { description: String(entry.rows.length), icon: new vscode.ThemeIcon(TYPE_ICONS[entry.type]) })), { description: `${list.bricks.length}`, icon: new vscode.ThemeIcon("library") });
    const stacks = new BrickNode("Stacks", list.stacks.map((row: StackRow) => new BrickNode(row.name, [], { description: `${row.ref} · ${row.members.length} member(s)`, tooltip: row.members.join(", "), icon: icon("layers", row.state), contextValue: `stack-${row.state}`, data: row })), { description: String(list.stacks.length), icon: new vscode.ThemeIcon("layers") });
    const targets = [...new Set(list.attachments.map((row) => row.target))].sort();
    const attachments = new BrickNode("Attachments", targets.map((target) => new BrickNode(target, list.attachments.filter((row) => row.target === target).map((row: AttachmentRow) => {
      const tooltip = new vscode.MarkdownString(`**${row.subject}** on **${row.target}** (\`${row.attachment_id}\`)\n\nCurrent Context (this attachment only):\n\n${row.current_context ? row.current_context.split(/\r?\n/).map((line) => `> ${line}`).join("\n") : "_none_"}${row.issues.length ? `\n\n**Fails closed:** ${row.issues.join("; ")}` : ""}`);
      return new BrickNode(row.subject, [], { description: `${row.state}${row.current_context ? " · context" : ""}${row.issues.length ? " · " + row.issues.length + " issue(s)" : ""}`, tooltip, icon: icon(row.issues.length ? "error" : "link", row.issues.length ? "failed" : row.state), contextValue: "attachment", data: row });
    }), { icon: new vscode.ThemeIcon(target.startsWith("flow:") ? "git-merge" : "symbol-namespace"), contextValue: "target", data: target })), { description: String(list.attachments.length), icon: new vscode.ThemeIcon("link") });
    const agents = new BrickNode("Assigned agents", this.agents.map((row) => new BrickNode(String(row.agent), [], { description: `${String(row.assignment_status)} · ${String(row.runtime_status)}`, tooltip: `${(row.targets as string[] | undefined)?.join(", ") ?? ""}\n${((row.problems as string[] | undefined) ?? []).join("\n")}\nInference while idle: none (monitors are deterministic); tokens are not measured.`, icon: icon("robot", String(row.runtime_status)), data: row })), { description: String(this.agents.length), icon: new vscode.ThemeIcon("robot") });
    const store = new BrickNode("Signed store", [], { description: `${list.store.trust} · generation ${list.store.generation}`, icon: icon("shield", list.store.trust === "verified" || list.store.trust === "absent" ? "active" : "failed") });
    const residents = this.residentGroup();
    const panel = new BrickNode("Open the Bricks panel", [], { icon: new vscode.ThemeIcon("window"), command: { command: "foxx.bricks.openPanel", title: "Open" } });
    return [panel, library, stacks, attachments, agents, residents, store];
  }

  /**
   * The resident agents group: assignments by target, the workspace's own Zones and Flows first.
   * @returns node
   */
  private residentGroup(): BrickNode {
    if (!this.residents) return new BrickNode("Resident agents", [new BrickNode("This Crane has no resident agents; install a newer Crane", [], { icon: new vscode.ThemeIcon("info") })], { icon: new vscode.ThemeIcon("watch") });
    const local = this.host.targets().map((target) => `${target.kind}:${target.id}`);
    const tones: Record<string, string> = { ok: "active", busy: "draft", attention: "paused", bad: "failed", off: "deprecated" };
    const groups = residentsByTarget(this.residents.assignments, local).map(([target, rows, inWorkspace]) => new BrickNode(target, rows.map((row: ResidentRow) => {
      const child = new BrickNode(row.assignment_id, [], {
        description: `${row.status} · ${row.definition}`,
        tooltip: new vscode.MarkdownString(residentMarkdown(row)),
        icon: icon("watch", tones[residentTone(row.status)]),
        contextValue: `resident-${row.lifecycle}`,
        data: row,
      });
      child.command = { command: "foxx.residents.show", title: "Show", arguments: [child] };
      return child;
    }), { description: inWorkspace ? String(rows.length) : `${rows.length} · not in this workspace`, icon: new vscode.ThemeIcon(target.startsWith("flow:") ? "git-merge" : "symbol-namespace"), contextValue: "resident-target", data: target }));
    const idle = this.residents.health?.counters?.idle_model_calls ?? 0;
    return new BrickNode("Resident agents", groups.length ? groups : [new BrickNode("No resident assignment yet", [], { icon: new vscode.ThemeIcon("info"), command: { command: "foxx.residents.create", title: "Create" } })], { description: `${this.residents.assignments.length} · ${idle} model call(s) while idle`, icon: new vscode.ThemeIcon("watch"), contextValue: "resident-group" });
  }

  /**
   * The assignment a node shows, or one the person chooses.
   * @param node node
   * @returns assignment
   */
  private async residentOf(node: BrickNode | undefined): Promise<ResidentRow | undefined> {
    const row = node?.data as ResidentRow | undefined;
    if (row && typeof row === "object" && "assignment_id" in row) return row;
    if (!this.residents) await this.load();
    const rows = this.residents?.assignments ?? [];
    if (!rows.length) {
      vscode.window.showInformationMessage("Landscape: there is no resident assignment yet.");
      return undefined;
    }
    const chosen = await vscode.window.showQuickPick(rows.map((item) => ({ label: item.assignment_id, description: `${item.status} · ${item.target}`, item })), { placeHolder: "Which resident assignment?" });
    return chosen?.item;
  }

  /**
   * Show an assignment: what Crane reports about it, and its latest activations.
   * @param node node
   */
  async showResident(node?: BrickNode): Promise<void> {
    const row = await this.residentOf(node);
    if (!row) return;
    const result = await this.envelope<{ assignment: ResidentRow; activations: Record<string, unknown>[] }>(["resident", "show", row.assignment_id]);
    if (!result) return;
    const activations = (result.data.activations ?? []).slice(0, 10).map((activation) => `- \`${String(activation.activation_id)}\` ${String(activation.state)} · ${String(((activation.trigger as Record<string, unknown>) ?? {}).kind ?? "")}`);
    const text = `${residentMarkdown(result.data.assignment)}\n\n## Latest activations\n\n${activations.length ? activations.join("\n") : "_none: idle until a trigger arrives_"}\n`;
    const document = await vscode.workspace.openTextDocument({ content: text, language: "markdown" });
    await vscode.commands.executeCommand("markdown.showPreviewToSide", document.uri).then(undefined, () => undefined);
  }

  /**
   * Preview the context pack an activation of an assignment would receive now (nothing is written).
   * @param node node
   */
  async residentContext(node?: BrickNode): Promise<void> {
    const row = await this.residentOf(node);
    if (!row) return;
    const result = await this.envelope<PackData>(["resident", "context", row.assignment_id]);
    if (!result) return;
    const document = await vscode.workspace.openTextDocument({ content: packMarkdown(row.assignment_id, result.data), language: "markdown" });
    await vscode.commands.executeCommand("markdown.showPreviewToSide", document.uri).then(undefined, () => undefined);
  }

  /**
   * Create a resident assignment draft for a Zone or Flow of this workspace: build the draft, let
   * the person edit it, preview Crane's validation with '--dry-run', and create it only after they
   * confirm. Activation stays an admin's separate step.
   * @param node a target node, when started from one
   */
  async createResident(node?: BrickNode): Promise<void> {
    if (!this.residents) await this.load();
    const definitions = (this.residents?.definitions ?? []).filter((row) => row.state === "active");
    if (!definitions.length) {
      vscode.window.showWarningMessage("Landscape: no active resident definition; save one with 'crane resident definition save --file' and have an admin activate it.");
      return;
    }
    const fromNode = typeof node?.data === "string" ? node.data : undefined;
    const targets = this.host.targets().filter((target) => target.status === "active").map((target) => `${target.kind}:${target.id}`);
    const target = fromNode ?? (await vscode.window.showQuickPick(targets, { placeHolder: "Which active Zone or Flow is it responsible for?" }));
    if (!target) return;
    const definition = await vscode.window.showQuickPick(definitions.map((row) => ({ label: row.ref, description: row.name })), { placeHolder: "Which definition version?" });
    if (!definition) return;
    const id = await vscode.window.showInputBox({ prompt: "Assignment id", value: `${definition.label.split("@")[0]}-${target.split(":")[1]}`, validateInput: checkAssignmentId });
    if (!id) return;
    const agent = await vscode.window.showInputBox({ prompt: "Agent of the signed Brick assignment (provider or agent id)", value: "claude" });
    if (!agent) return;
    const kinds = await vscode.window.showInputBox({ prompt: "CI trigger kinds it wakes on (comma separated; no bare *)", value: "ci.failed" });
    if (kinds === undefined) return;
    const text = await this.writeInEditor("resident assignment draft (JSON; Crane validates it)", JSON.stringify(assignmentDraft({ id, definition: definition.label, agent, target, kinds }), null, 2), "json");
    if (text === undefined) return;
    try {
      JSON.parse(text);
    } catch (error) {
      vscode.window.showErrorMessage(`Landscape: the draft is not JSON: ${(error as Error).message}`);
      return;
    }
    const file = this.temporary("resident.json", text);
    try {
      const preview = await this.envelope<{ validation: { problems: [string, string][]; warnings: [string, string][] } }>(["resident", "create", "--file", file, "--dry-run"]);
      if (!preview) return;
      const problems = (preview.data.validation?.problems ?? []).map(([code, message]) => `problem ${code}: ${message}`);
      const warnings = (preview.data.validation?.warnings ?? []).map(([code, message]) => `warning ${code}: ${message}`);
      const detail = [preview.summary, ...problems, ...warnings, "", "It is created as a draft: it receives no trigger until it is validated and an admin activates it."].join("\n");
      const go = await vscode.window.showWarningMessage(`Landscape: create resident ${id} on ${target}?`, { modal: true, detail }, "Create draft");
      if (go !== "Create draft") return;
      const result = await this.envelope(["resident", "create", "--file", file]);
      if (result) await this.report(result, "Create resident", false);
    } finally {
      fs.rmSync(path.dirname(file), { recursive: true, force: true });
    }
  }

  /**
   * Pause, resume, or revoke an assignment, after the person confirms (Crane decides whether it is
   * allowed; resuming may need a signer).
   * @param node node
   */
  async changeResident(node?: BrickNode): Promise<void> {
    const row = await this.residentOf(node);
    if (!row) return;
    const changes = residentChanges(row);
    if (!changes.length) {
      vscode.window.showInformationMessage(`Landscape: ${row.assignment_id} is ${row.lifecycle}; it cannot be paused, resumed, or revoked now.`);
      return;
    }
    const chosen = await vscode.window.showQuickPick(changes.map((name) => ({ label: RESIDENT_CHANGES[name].label, detail: RESIDENT_CHANGES[name].detail, name })), { placeHolder: `Change ${row.assignment_id} (${row.lifecycle})` });
    if (!chosen) return;
    const reason = await vscode.window.showInputBox({ prompt: `Why ${chosen.label.toLowerCase()} ${row.assignment_id}? (recorded)`, validateInput: (value) => (value.trim() ? undefined : "A reason is required") });
    if (!reason) return;
    const go = await vscode.window.showWarningMessage(`Landscape: ${chosen.label.toLowerCase()} ${row.assignment_id}?`, { modal: true, detail: chosen.detail }, chosen.label);
    if (go !== chosen.label) return;
    const result = await this.envelope(["resident", chosen.name, row.assignment_id, "--reason", reason]);
    if (result) await this.report(result, `${chosen.label} resident`, false);
  }

  /**
   * Validate an assignment against the current signed state.
   * @param node node
   */
  async validateResident(node?: BrickNode): Promise<void> {
    const row = await this.residentOf(node);
    if (!row) return;
    const result = await this.envelope<{ validation: { problems: [string, string][]; warnings: [string, string][] } }>(["resident", "validate", row.assignment_id]);
    if (!result) return;
    for (const [code, message] of result.data.validation?.problems ?? []) this.host.output.appendLine(`  problem ${code}: ${message}`);
    for (const [code, message] of result.data.validation?.warnings ?? []) this.host.output.appendLine(`  warning ${code}: ${message}`);
    await this.report(result, "Validate resident", false);
  }

  /**
   * Choose a Brick from the list (when no node was given).
   * @param filter which rows qualify
   * @param placeHolder prompt
   * @returns row
   */
  private async chooseBrick(filter: (row: BrickRow) => boolean, placeHolder: string): Promise<BrickRow | undefined> {
    if (!this.list) await this.load();
    const rows = (this.list?.bricks ?? []).filter(filter);
    if (!rows.length) {
      vscode.window.showInformationMessage("Landscape: no Brick qualifies; create one first.");
      return undefined;
    }
    const chosen = await vscode.window.showQuickPick(rows.map((row) => ({ label: row.name, description: `${row.ref} · ${row.type} · ${row.state}`, row })), { placeHolder });
    return chosen?.row;
  }

  /**
   * The Brick a node shows, or one the person chooses.
   * @param node node
   * @param filter which rows qualify
   * @param placeHolder prompt
   * @returns row
   */
  private async brickOf(node: BrickNode | undefined, filter: (row: BrickRow) => boolean, placeHolder: string): Promise<BrickRow | undefined> {
    const row = node?.data as BrickRow | undefined;
    if (row && "brick_id" in row && filter(row)) return row;
    return this.chooseBrick(filter, placeHolder);
  }

  /**
   * Choose a Flow or Zone.
   * @param placeHolder prompt
   * @returns "flow:ID" or "zone:ID"
   */
  private async chooseTarget(placeHolder: string): Promise<string | undefined> {
    const targets = this.host.targets();
    if (!targets.length) {
      vscode.window.showInformationMessage("Landscape: there are no Zones or Flows yet; create one from the Landscape panel first.");
      return undefined;
    }
    const chosen = await vscode.window.showQuickPick(targets.map((target) => ({ label: `${target.kind}:${target.id}`, description: target.status })), { placeHolder });
    return chosen?.label;
  }

  /**
   * Write text to a temporary file Crane reads (never inside the repository).
   * @param name file name
   * @param text content
   * @returns path
   */
  private temporary(name: string, text: string): string {
    const directory = fs.mkdtempSync(path.join(os.tmpdir(), "landscape-brick-"));
    const file = path.join(directory, name);
    fs.writeFileSync(file, text, "utf8");
    return file;
  }

  /**
   * Let the person write multi-line text in an editor, then continue or cancel.
   * @param title what is written
   * @param initial starting text
   * @param language editor language
   * @returns the saved text, or undefined when cancelled
   */
  private async writeInEditor(title: string, initial: string, language = "markdown"): Promise<string | undefined> {
    const document = await vscode.workspace.openTextDocument({ content: initial, language });
    await vscode.window.showTextDocument(document, { preview: false });
    const choice = await vscode.window.showInformationMessage(`Landscape: write the ${title} in the editor that opened, then continue. It is guidance for agents, never authorization; never put secrets in it.`, { modal: false }, "Continue", "Cancel");
    const text = document.getText();
    // Close only this scratch document, never another editor the person switched to
    if (vscode.window.activeTextEditor?.document === document) await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor");
    return choice === "Continue" ? text : undefined;
  }

  /**
   * Show the outcome of a command and refresh.
   * @param envelope envelope
   * @param label what was done
   * @param changed whether signed state changed (refresh governance too)
   */
  private async report(envelope: Envelope<unknown>, label: string, changed: boolean): Promise<void> {
    this.host.output.appendLine(`${label}: ${envelopeLine(envelope)}`);
    const ok = succeeded(envelope.status);
    (ok ? vscode.window.showInformationMessage : vscode.window.showWarningMessage)(`Landscape: ${envelope.summary}`, "Show details").then((choice) => {
      if (choice) this.host.output.show(true);
    });
    this.host.record("FOXX_BRICK_COMMAND", { command: envelope.command, status: envelope.status.toUpperCase(), statement: envelope.summary });
    await this.load();
    if (changed) await this.host.refresh();
  }

  /**
   * Preview a signing change with '--dry-run', ask for confirmation, then sign it.
   * @param args the command's arguments (without --dry-run and --format)
   * @param label what it does, for the confirmation
   */
  private async signed(args: string[], label: string): Promise<void> {
    const preview = await this.envelope<{ preview?: ChangePreview; entry?: unknown }>(args.concat(["--dry-run"]));
    if (!preview) return;
    if (!succeeded(preview.status)) return this.report(preview, label, false);
    const lines = describePreview(preview.data.preview);
    const detail = [preview.summary, ...lines, "", "Crane signs this into .crane/bricks.json with your admin key; commit .crane afterwards so the team shares it."].join("\n");
    const go = await vscode.window.showWarningMessage(`Landscape: ${label}?`, { modal: true, detail }, "Sign and apply");
    if (go !== "Sign and apply") return;
    const result = await this.envelope(args);
    if (result) await this.report(result, label, true);
  }

  /** Create a Brick draft: fixed type, plain-language intent, Common Context, and configuration. */
  async create(): Promise<void> {
    if (!this.types.length) await this.load();
    const types = this.types.length ? this.types : BRICK_TYPES.map((type) => ({ type, label: type, purpose: "", support: "declared", runtime: "", fields: [] }) as TypeInfo);
    const type = await vscode.window.showQuickPick(types.map((info) => ({ label: info.label, description: info.support, detail: `${info.purpose} — ${info.runtime}`, type: info.type })), { placeHolder: "Which kind of Brick? (the types are fixed)", matchOnDetail: true });
    if (!type) return;
    const name = await vscode.window.showInputBox({ prompt: "Name", placeHolder: "Production config guard", validateInput: (value) => (value.trim() ? undefined : "A name is required") });
    if (!name) return;
    const intent = await vscode.window.showInputBox({ prompt: "What does it solve, in plain words?", placeHolder: "Agents must not change production settings without a person" });
    if (intent === undefined) return;
    const context = await this.writeInEditor("Common Context (it travels with every version of this Brick)", "");
    if (context === undefined) return;
    const config = await this.writeInEditor(`${type.label} configuration (JSON; Crane checks it against the type's schema)`, JSON.stringify(BRICK_TEMPLATES[type.type], null, 2), "json");
    if (config === undefined) return;
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(config || "{}") as Record<string, unknown>;
    } catch (error) {
      vscode.window.showErrorMessage(`Landscape: the configuration is not JSON: ${(error as Error).message}`);
      return;
    }
    const file = this.temporary("draft.json", draftFile({ type: type.type, name, intent, context, config: parsed }));
    const result = await this.envelope<{ validation: { status: string; problems: { code: string; message: string }[] } }>(["brick", "create", "--file", file]);
    fs.rmSync(path.dirname(file), { recursive: true, force: true });
    if (!result) return;
    for (const problem of result.data.validation?.problems ?? []) this.host.output.appendLine(`  ${problem.code}: ${problem.message}`);
    await this.report(result, "Create Brick", false);
  }

  /**
   * Edit a draft (or start the next version of an activated Brick).
   * @param node the Brick
   */
  async edit(node?: BrickNode): Promise<void> {
    const row = await this.brickOf(node, (item) => item.state === "draft" || item.state === "active", "Which Brick? (editing an activated one starts its next version)");
    if (!row) return;
    const field = await vscode.window.showQuickPick(["Intent", "Common Context", "Configuration", "Name"], { placeHolder: `Edit ${row.brick_id}` });
    if (!field) return;
    const show = await this.envelope<{ definition: { name: string; intent: string; config: unknown; common_context: { text: string } } }>(["brick", "show", row.state === "draft" ? row.brick_id : row.ref]);
    if (!show) return;
    const definition = show.data.definition;
    let args: string[] | undefined;
    if (field === "Intent" || field === "Name") {
      const value = await vscode.window.showInputBox({ prompt: field, value: field === "Intent" ? definition.intent : definition.name });
      if (value !== undefined) args = ["brick", "edit", row.brick_id, field === "Intent" ? "--intent" : "--name", value];
    } else {
      const text = await this.writeInEditor(field, field === "Configuration" ? JSON.stringify(definition.config ?? {}, null, 2) : definition.common_context.text, field === "Configuration" ? "json" : "markdown");
      if (text !== undefined) args = ["brick", "edit", row.brick_id, field === "Configuration" ? "--config" : "--context-file", this.temporary(field === "Configuration" ? "config.json" : "context.md", text)];
    }
    if (!args) return;
    const result = await this.envelope(args);
    if (args.length > 4 && args[3] !== "--intent" && args[3] !== "--name") fs.rmSync(path.dirname(args[4]), { recursive: true, force: true });
    if (result) await this.report(result, "Edit Brick", false);
  }

  /**
   * Validate a Brick.
   * @param node the Brick
   */
  async validate(node?: BrickNode): Promise<void> {
    const row = await this.brickOf(node, () => true, "Validate which Brick?");
    if (!row) return;
    const result = await this.envelope<{ validation: { problems: { code: string; message: string }[] } }>(["brick", "validate", row.state === "draft" ? row.brick_id : row.ref]);
    if (!result) return;
    for (const problem of result.data.validation?.problems ?? []) this.host.output.appendLine(`  ${problem.code}: ${problem.message}`);
    await this.report(result, "Validate Brick", false);
  }

  /**
   * Test a Brick, simulated on a Flow or Zone when the person picks one.
   * @param node the Brick
   */
  async test(node?: BrickNode): Promise<void> {
    const row = await this.brickOf(node, () => true, "Test which Brick?");
    if (!row) return;
    const target = await vscode.window.showQuickPick(["(the targets it is attached to)", ...this.host.targets().map((item) => `${item.kind}:${item.id}`)], { placeHolder: "Simulate it on which Flow or Zone?" });
    if (target === undefined) return;
    const args = ["brick", "test", row.state === "draft" ? row.brick_id : row.ref];
    if (!target.startsWith("(")) args.push("--target", target);
    const result = await this.envelope<{ cases: { name: string; status: string; detail: string }[] }>(args);
    if (!result) return;
    for (const item of result.data.cases ?? []) this.host.output.appendLine(`  [${item.status}] ${item.name} — ${item.detail}`);
    await this.report(result, "Test Brick", false);
  }

  /**
   * Show a Brick's detail as JSON.
   * @param node the Brick
   * @param reference a reference given by a tree item's click
   */
  async show(node?: BrickNode, reference?: string): Promise<void> {
    const row = reference ? undefined : await this.brickOf(node, () => true, "Show which Brick?");
    const ref = reference ?? (row ? (row.state === "draft" ? row.brick_id : row.ref) : undefined);
    if (!ref) return;
    const result = await this.envelope(["brick", "show", ref]);
    if (!result) return;
    const document = await vscode.workspace.openTextDocument({ content: JSON.stringify(result.data, null, 2), language: "json" });
    await vscode.window.showTextDocument(document, { preview: true });
  }

  /**
   * Activate a draft (an admin signs it; the author cannot when another admin can review).
   * @param node the draft
   */
  async activate(node?: BrickNode): Promise<void> {
    const row = await this.brickOf(node, (item) => item.state === "draft", "Activate which draft?");
    if (!row) return;
    const reason = await vscode.window.showInputBox({ prompt: `Why activate ${row.brick_id}? (recorded in the signed audit)`, validateInput: (value) => (value.trim() ? undefined : "A reason is required") });
    if (!reason) return;
    await this.signed(["brick", "activate", row.brick_id, "--reason", reason], `Activate ${row.brick_id}@${row.version}`);
  }

  /**
   * Attach an activated Brick (or Stack) to a Flow or Zone, with this attachment's Current Context.
   * @param node the Brick, Stack, or target
   * @param given a target passed by another view ("flow:ID")
   */
  async attach(node?: BrickNode, given?: string): Promise<void> {
    let subject: string | undefined;
    let target = given ?? (node?.contextValue === "target" ? String(node.data) : undefined);
    const data = node?.data as BrickRow | StackRow | undefined;
    if (data && typeof data === "object" && "ref" in data) subject = subjectOf(data);
    if (!subject) {
      if (!this.list) await this.load();
      const choices = [...(this.list?.bricks ?? []).filter((row) => row.state === "active"), ...(this.list?.stacks ?? []).filter((row) => row.state === "active")];
      const chosen = await vscode.window.showQuickPick(choices.map((row) => ({ label: row.name, description: subjectOf(row), subject: subjectOf(row) })), { placeHolder: "Attach which active Brick or Stack?" });
      subject = chosen?.subject;
    }
    if (!subject) return;
    target = target ?? (await this.chooseTarget(`Attach ${subject} to which Flow or Zone?`));
    if (!target) return;
    const context = await this.writeInEditor(`Current Context of ${subject} on ${target} (this attachment only)`, "");
    if (context === undefined) return;
    const args = ["brick", "attach", subject, "--target", target];
    let file: string | undefined;
    if (context.trim()) {
      file = this.temporary("current-context.md", context);
      args.push("--context-file", file);
    }
    await this.signed(args, `Attach ${subject} to ${target}`);
    if (file) fs.rmSync(path.dirname(file), { recursive: true, force: true });
  }

  /**
   * The attachment a node shows, or one the person chooses.
   * @param node node
   * @returns attachment
   */
  private async attachmentOf(node?: BrickNode): Promise<AttachmentRow | undefined> {
    if (node?.contextValue === "attachment") return node.data as AttachmentRow;
    if (!this.list) await this.load();
    const chosen = await vscode.window.showQuickPick((this.list?.attachments ?? []).map((row) => ({ label: row.subject, description: `${row.target} · ${row.attachment_id}`, row })), { placeHolder: "Which attachment?" });
    return chosen?.row;
  }

  /**
   * Detach an attachment (the history stays in the signed audit).
   * @param node the attachment
   */
  async detach(node?: BrickNode): Promise<void> {
    const row = await this.attachmentOf(node);
    if (!row) return;
    const reason = await vscode.window.showInputBox({ prompt: `Why detach ${row.subject} from ${row.target}?`, validateInput: (value) => (value.trim() ? undefined : "A reason is required") });
    if (!reason) return;
    await this.signed(["brick", "detach", row.attachment_id, "--reason", reason], `Detach ${row.subject} from ${row.target}`);
  }

  /**
   * Pin an attachment to another active version of the same Brick.
   * @param node the attachment
   */
  async upgrade(node?: BrickNode): Promise<void> {
    const row = await this.attachmentOf(node);
    if (!row) return;
    const id = row.subject.replace(/^(brick|stack):/, "").split("@")[0];
    const versions = row.subject.startsWith("stack:")
      ? (this.list?.stacks ?? []).filter((item) => item.stack_id === id && item.state === "active").map((item) => `stack:${item.stack_id}@${item.version}`)
      : (this.list?.bricks ?? []).filter((item) => item.brick_id === id && item.state === "active").map((item) => `${item.brick_id}@${item.version}`);
    const to = await vscode.window.showQuickPick(versions.filter((version) => !row.subject.endsWith(version)), { placeHolder: `Pin ${row.attachment_id} to which version?` });
    if (!to) return;
    const reason = await vscode.window.showInputBox({ prompt: "Why?", validateInput: (value) => (value.trim() ? undefined : "A reason is required") });
    if (!reason) return;
    await this.signed(["brick", "upgrade", row.attachment_id, "--to", to, "--reason", reason], `Pin ${row.attachment_id} to ${to}`);
  }

  /**
   * Edit one attachment's Current Context (it changes only this attachment).
   * @param node the attachment
   */
  async editContext(node?: BrickNode): Promise<void> {
    const row = await this.attachmentOf(node);
    if (!row) return;
    const text = await this.writeInEditor(`Current Context of ${row.subject} on ${row.target} (this attachment only; it cannot relax any control)`, row.current_context);
    if (text === undefined) return;
    const file = this.temporary("current-context.md", text);
    await this.signed(["brick", "context", "set", row.attachment_id, "--file", file], `Update the Current Context of ${row.subject} on ${row.target}`);
    fs.rmSync(path.dirname(file), { recursive: true, force: true });
  }

  /**
   * Revoke an activated Brick version (pins to it fail closed until detached or upgraded).
   * @param node the Brick
   */
  async revoke(node?: BrickNode): Promise<void> {
    const row = await this.brickOf(node, (item) => item.state === "active" || item.state === "deprecated", "Revoke which version?");
    if (!row) return;
    const reason = await vscode.window.showInputBox({ prompt: `Why revoke ${row.ref}? (attachments pinned to it fail closed until an admin detaches or upgrades them)`, validateInput: (value) => (value.trim() ? undefined : "A reason is required") });
    if (!reason) return;
    const go = await vscode.window.showWarningMessage(`Landscape: revoke ${row.ref}?`, { modal: true, detail: "Revocation is signed and cannot be undone; activate a new version instead to replace it." }, "Revoke");
    if (go !== "Revoke") return;
    const result = await this.envelope(["brick", "revoke", row.ref, "--reason", reason]);
    if (result) await this.report(result, "Revoke Brick", true);
  }

  /**
   * Preview the effective context of Flows and Zones as Markdown: every source with its digest,
   * then the exact text agents receive.
   * @param node a target or attachment node, when invoked from the view
   */
  async preview(node?: BrickNode): Promise<void> {
    let targets: string[] = [];
    if (node?.contextValue === "target") targets = [String(node.data)];
    else if (node?.contextValue === "attachment") targets = [(node.data as AttachmentRow).target];
    if (!targets.length) {
      const chosen = await vscode.window.showQuickPick(this.host.targets().map((item) => ({ label: `${item.kind}:${item.id}`, description: item.status })), { placeHolder: "Preview the context of which Flows and Zones?", canPickMany: true });
      targets = (chosen ?? []).map((item) => item.label);
    }
    if (!targets.length) return;
    const result = await this.envelope<SnapshotData>(["brick", "context", "preview", "--targets", targets.join(",")]);
    if (!result) return;
    this.host.record("FOXX_BRICK_CONTEXT_PREVIEWED", { status: result.status.toUpperCase(), statement: result.summary, targets: targets.join(",") });
    const document = await vscode.workspace.openTextDocument({ content: snapshotMarkdown(result.data), language: "markdown" });
    await vscode.window.showTextDocument(document, { preview: true });
    await vscode.commands.executeCommand("markdown.showPreviewToSide", document.uri).then(undefined, () => undefined);
  }

  /**
   * Run one command of Crane's command families and show its result.
   * @param family limit the choice to one family
   */
  async runControl(family?: string): Promise<void> {
    const items = CONTROLS.filter((item) => !family || item.family === family);
    const chosen = await vscode.window.showQuickPick(items.map((item) => ({ label: `crane ${item.label}`, description: item.changes ? "changes state" : "", detail: item.detail, item })), { placeHolder: "Run which Crane control? (the same command families as the CLI)", matchOnDetail: true });
    if (!chosen) return;
    const values: string[] = [];
    for (const input of chosen.item.inputs) {
      const value = await vscode.window.showInputBox({ prompt: input.prompt, placeHolder: input.placeholder, validateInput: (text) => (input.optional || text.trim() ? undefined : "Required") });
      if (value === undefined) return;
      values.push(value);
    }
    const args = controlArgs(chosen.item, values);
    if (chosen.item.changes) {
      const go = await vscode.window.showWarningMessage(`Landscape: run crane ${args.slice(0, -2).join(" ")}?`, { modal: true, detail: "This changes state; Crane records it and decides whether it is allowed." }, "Run");
      if (go !== "Run") return;
    }
    const result = await this.host.crane(args);
    if (!result || result.code === 126) return;
    const envelope = parseEnvelope(result.stdout);
    this.host.record("FOXX_CONTROL_RUN", { command: chosen.item.id, status: envelope ? envelope.status.toUpperCase() : "FAILED", exit_code: result.code });
    if (!envelope) {
      vscode.window.showErrorMessage(`Landscape: ${(result.stderr || result.stdout).trim().replace(/^crane: /, "") || "Crane printed no result"}`);
      return;
    }
    this.host.output.appendLine(JSON.stringify(envelope.data, null, 2));
    const message = `Landscape: ${envelopeLine(envelope)}`;
    const show = (succeeded(envelope.status) ? vscode.window.showInformationMessage : envelope.status === "approval_required" || envelope.status === "unknown" || envelope.status === "unresolved" ? vscode.window.showWarningMessage : vscode.window.showErrorMessage)(message, "Show details");
    if (await show) this.host.output.show(true);
    if (chosen.item.changes || chosen.item.family === "brick") await this.load();
  }
}
