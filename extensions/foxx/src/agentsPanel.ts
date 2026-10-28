// The Resident Agents panel: the repository's Zones and Flows as a graph with the resident agents
// working on each, a form for an agent's definition (its metaproperties: purpose, owner, what
// wakes it, what it may do, its safety switches and limits), the agents created but not working
// yet, and the ones working now. An agent starts working when it is put on a Zone or Flow: Landscape
// signs that the agent works there (a Brick assignment, keeping its other targets), creates the
// resident assignment, has Crane check it, and activates it for one period (renewable). Crane is
// the authority for every step; definitions never change once saved, so an update saves the next
// version. Whatever Crane refuses is shown in plain words with its reasons.

import * as vscode from "vscode";

import { BrickList, succeeded } from "./core/bricks";
import { checkAssignmentId, PackData, packMarkdown, RESIDENT_CHANGES, ResidentList, residentMarkdown, ResidentRow, residentTone } from "./core/residents";
import { assignmentFile, checkDefinition, DefinitionInput, definitionFile, friendlyError, LIMITS, mergeTargets, OverlayItem, overlayGraph, problemLines, TRIGGER_DEFAULTS } from "./core/workbench";
import { call, CANCELLED, Dialogs, OpResult, PanelDeps, refused, scratch, showMarkdown, WorkbenchHost, WorkbenchPage } from "./workbench";

/** The panel's view type. */
export const AGENTS_PANEL = "foxx.agentsPanel";

/** A definition as 'crane resident list' reports it, with the fields the panel shows. */
interface DefinitionFull {
  ref: string;
  definition_id: string;
  version: number;
  name: string;
  owner?: string;
  state: "draft" | "active" | "revoked";
  triggers?: string[];
  actions?: { step_type: string; auto: boolean }[];
  bricks?: string[];
}

/** One step type of 'crane resident catalog'. */
interface StepInfo {
  type: string;
  effect: string;
  available: boolean;
  unavailable_reason?: string | null;
}

/** Reason codes an assignment's first check reports for what putting it to work fixes. */
const FIXED_BY_ASSIGNING = new Set(["DEFINITION_NOT_ACTIVE", "NO_SIGNED_ASSIGNMENT", "ASSIGNMENT_NOT_ACTIVE"]);

/**
 * Read a text value the page sent.
 * @param value value
 * @returns text ("" when absent)
 */
const text = (value: unknown): string => (typeof value === "string" ? value : "");

/**
 * The extension side of the Resident Agents panel.
 * @param deps the rest of Landscape
 * @param changed called after every change, so the sidebar list follows
 * @returns host
 */
export function agentsHost(deps: PanelDeps, changed: () => void): WorkbenchHost {
  let residents: ResidentList | undefined;
  let bricks: BrickList | undefined;
  let catalog: { steps: StepInfo[]; triggers: { source: string; supported: boolean; delivery?: string | null }[] } | undefined;

  /** The newest resident state, or why there is none. */
  async function load(): Promise<string | undefined> {
    const listed = await call<ResidentList>(deps, ["resident", "list"]);
    residents = listed.envelope?.data;
    bricks = (await call<BrickList>(deps, ["brick", "list"])).envelope?.data;
    if (residents && !catalog) catalog = (await call<NonNullable<typeof catalog>>(deps, ["resident", "catalog"])).envelope?.data;
    return listed.error;
  }

  const definitions = () => (residents?.definitions ?? []) as unknown as DefinitionFull[];

  /** The panel's dialogs, for the operation running now. */
  let dialogs: Dialogs;

  return {
    async state(refresh) {
      const problem = await load();
      const graph = await deps.graph(refresh);
      const items: OverlayItem[] = (residents?.assignments ?? [])
        .filter((row) => row.lifecycle !== "revoked")
        .map((row) => ({ id: row.assignment_id, label: row.assignment_id, sub: `${row.status.toLowerCase().replace(/_/g, " ")} · ${row.agent}`, target: row.target, tone: residentTone(row.status) }));
      return {
        problem,
        graphError: graph.error,
        graph: overlayGraph(graph.data, items),
        targets: deps.targets().map((target) => ({ key: `${target.kind}:${target.id}`, ...target })),
        definitions: definitions(),
        assignments: residents?.assignments ?? [],
        idle: residents?.health?.counters?.idle_model_calls ?? 0,
        catalog: catalog ?? { steps: [], triggers: [] },
        bricks: (bricks?.bricks ?? []).filter((row) => row.state === "active").map((row) => ({ ref: row.ref, name: row.name, type: row.type })),
        triggers: TRIGGER_DEFAULTS,
        limits: LIMITS,
      };
    },

    async run(op, values, progress, asked) {
      dialogs = asked;
      const result = await operate(op, values, progress);
      if (!op.startsWith("load:") && (result.ok || result.problems)) {
        await deps.refresh();
        changed();
      }
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
    if (op === "load:definition") {
      const shown = await call<{ record: { definition: Record<string, unknown> } }>(deps, ["resident", "definition", "show", text(values.ref)]);
      if (!shown.envelope) return { ok: false, message: shown.error ?? "Crane printed no result." };
      return { ok: true, message: "", data: shown.envelope.data.record?.definition };
    }
    if (op === "saveDefinition") return saveDefinition(values, progress);
    if (op === "activateDefinition") {
      const ref = text(values.ref);
      const reason = await dialogs.reason(`Why activate the agent ${ref}? Activation is signed with an admin key.`);
      if (!reason) return CANCELLED;
      progress(`Activating ${ref}`);
      return outcome(await call(deps, ["resident", "definition", "activate", ref, "--reason", reason]));
    }
    if (op === "assign") return assign(values, progress);
    if (op === "activateAssignment") {
      const id = text(values.id);
      const reason = await dialogs.reason(`Why activate ${id}? Activation is signed with an admin key.`);
      if (!reason) return CANCELLED;
      progress(`Checking ${id} against the signed rules`);
      const checked = await call<{ validation?: { problems?: unknown } }>(deps, ["resident", "validate", id]);
      if (!checked.envelope) return { ok: false, message: checked.error ?? "Crane printed no result." };
      if (!succeeded(checked.envelope.status)) return { ok: false, message: `${id} does not pass Crane's checks yet, so it stays inactive.`, problems: problemLines(checked.envelope.data?.validation?.problems) };
      progress(`Activating ${id}`);
      return outcome(await call(deps, ["resident", "activate", id, "--reason", reason]));
    }
    if (op === "change") {
      const id = text(values.id), action = text(values.action) as keyof typeof RESIDENT_CHANGES | "renew";
      if (!["pause", "resume", "revoke", "renew"].includes(action)) return { ok: false, message: `Landscape does not know the change '${action}'.` };
      const label = action === "renew" ? "Renew" : RESIDENT_CHANGES[action].label;
      const reason = await dialogs.reason(`Why ${label.toLowerCase()} ${id}?${action === "renew" ? " Renewing is signed with an admin key." : ""}`);
      if (!reason) return CANCELLED;
      if (action === "revoke" && !(await dialogs.confirm(`revoke ${id}?`, RESIDENT_CHANGES.revoke.detail, "Revoke", "danger"))) return CANCELLED;
      progress(`${label} ${id}`);
      return outcome(await call(deps, ["resident", action, id, "--reason", reason]));
    }
    if (op === "context") {
      const id = text(values.id);
      const result = await call<PackData>(deps, ["resident", "context", id]);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      await showMarkdown(packMarkdown(id, result.envelope.data));
      return { ok: true, message: `Opened what ${id} knows when it wakes.` };
    }
    if (op === "show") {
      const result = await call<{ assignment: ResidentRow; activations?: Record<string, unknown>[] }>(deps, ["resident", "show", text(values.id)]);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      const activations = (result.envelope.data.activations ?? []).slice(0, 10).map((item) => `- \`${String(item.activation_id)}\` ${String(item.state)}`);
      await showMarkdown(`${residentMarkdown(result.envelope.data.assignment)}\n\n## Latest activations\n\n${activations.length ? activations.join("\n") : "_none: idle until a trigger arrives_"}\n`);
      return { ok: true, message: `Opened the details of ${text(values.id)}.` };
    }
    return { ok: false, message: `Landscape does not know the operation '${op}'.` };
  }

  /**
   * The result of a command that either worked or not.
   * @param result envelope or error
   * @returns result
   */
  function outcome(result: Awaited<ReturnType<typeof call>>): OpResult {
    if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
    if (!succeeded(result.envelope.status)) return refused(result.envelope);
    deps.record("FOXX_RESIDENT_COMMAND", { command: result.envelope.command, status: result.envelope.status.toUpperCase(), statement: result.envelope.summary });
    return { ok: true, message: result.envelope.summary };
  }

  /**
   * Save an agent definition (a new agent, or the next version of one), and activate it when asked.
   * @param values the form
   * @param progress step reporter
   * @returns result
   */
  async function saveDefinition(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const input = values.definition as DefinitionInput;
    const fields = checkDefinition(input);
    const existing = definitions().filter((row) => row.definition_id === input.id);
    if (input.version === 1 && existing.length) fields.id = "An agent with this id exists. Use Update on it in 'Not working yet', or pick another id.";
    if (values.now && !text(values.reason).trim()) fields.reason = "Say why it is activated; it is recorded.";
    if (Object.keys(fields).length) return { ok: false, message: "Some fields need attention.", fields };
    const file = scratch("definition.json", JSON.stringify(definitionFile(input), null, 2));
    try {
      progress("Checking the definition");
      const checked = await call<{ problems?: unknown; warnings?: unknown }>(deps, ["resident", "definition", "save", "--file", file.file, "--dry-run"]);
      if (!checked.envelope) return { ok: false, message: checked.error ?? "Crane printed no result." };
      if (!succeeded(checked.envelope.status)) return { ok: false, message: "Crane found problems in the definition; nothing was saved.", problems: problemLines(checked.envelope.data?.problems) };
      const warnings = problemLines(checked.envelope.data?.warnings);
      progress("Saving the definition");
      const saved = await call(deps, ["resident", "definition", "save", "--file", file.file]);
      if (!saved.envelope) return { ok: false, message: saved.error ?? "Crane printed no result." };
      if (!succeeded(saved.envelope.status)) return refused(saved.envelope);
      const ref = saved.envelope.refs.definition_ref ?? `${input.id}@${input.version}`;
      if (!values.now) return { ok: true, message: `Saved ${ref}. It is listed under 'Not working yet'; an admin activates it, then you put it to work on a Zone or Flow.`, problems: warnings.length ? warnings.map((line) => `Note: ${line}`) : undefined };
      progress(`Activating ${ref}`);
      const activated = await call(deps, ["resident", "definition", "activate", ref, "--reason", text(values.reason)]);
      if (!activated.envelope || !succeeded(activated.envelope.status)) return { ok: false, message: `Saved ${ref}, but activating it did not finish: ${activated.error ?? friendlyError(activated.envelope?.summary ?? "")}` };
      return { ok: true, message: `Saved and activated ${ref}. Put it to work on a Zone or Flow from 'Not working yet'.`, problems: warnings.length ? warnings.map((line) => `Note: ${line}`) : undefined };
    } finally {
      file.dispose();
    }
  }

  /**
   * Put an agent to work on a Zone or Flow: check the assignment first (nothing changes when it
   * cannot work), confirm every step with the person, then activate the definition when it is a
   * draft, sign the agent's Brick assignment, create the resident assignment, check it, and
   * activate it.
   * @param values the form
   * @param progress step reporter
   * @returns result
   */
  async function assign(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const ref = text(values.definition), target = text(values.target), agent = text(values.agent).trim(), id = text(values.id).trim(), reason = text(values.reason).trim();
    const lifetime = Number(values.lifetime) || 30;
    const fields: Record<string, string> = {};
    if (!target) fields.target = "Choose a Zone or Flow, or click one in the graph.";
    if (!/^[A-Za-z0-9._-]+$/.test(agent)) fields.agent = "Use the provider (claude, codex) or an agent id: letters, digits, '.', '_' or '-'.";
    const idProblem = checkAssignmentId(id);
    if (idProblem) fields.id = idProblem;
    else if ((residents?.assignments ?? []).some((row) => row.assignment_id === id)) fields.id = "An assignment with this id exists. Pick another id.";
    if (!reason) fields.reason = "Say why it starts working; it is recorded.";
    const wake = (values.wake ?? {}) as Record<string, string>;
    if (!Object.values(wake).some((value) => String(value).trim())) fields.wake = "Choose at least one thing that wakes it.";
    if (Object.keys(fields).length) return { ok: false, message: "Some fields need attention.", fields };
    const definition = definitions().find((row) => row.ref === ref);
    if (!definition) return { ok: false, message: "That agent definition is gone; refresh the panel." };
    if (definition.state === "revoked") return { ok: false, message: `${ref} was revoked and can no longer work. Save a new version with Update.` };

    const file = scratch("assignment.json", JSON.stringify(assignmentFile({ id, definition: ref, agent, target, wake, lifetimeDays: lifetime }), null, 2));
    try {
      progress("Checking the assignment before anything changes");
      const preview = await call<{ validation?: { problems?: [string, string][] } }>(deps, ["resident", "create", "--file", file.file, "--dry-run"]);
      if (!preview.envelope) return { ok: false, message: preview.error ?? "Crane printed no result." };
      const blocking = (preview.envelope.data?.validation?.problems ?? []).filter(([code]) => !FIXED_BY_ASSIGNING.has(code));
      if (blocking.length) return { ok: false, message: `${id} could not work on ${target} as it is; nothing was changed.`, problems: problemLines(blocking) };

      const current = (bricks?.assignments ?? []).find((row) => row.agent === agent);
      const covered = Boolean(current && current.state === "active" && mergeTargets(current.targets, target).length === current.targets.length);
      const steps = [
        ...(definition.state === "draft" ? [`Activate the agent definition ${ref} (admin key).`] : []),
        ...(covered ? [] : [`Sign that ${agent} works in ${target}${current?.targets.length ? " (it keeps its other Zones and Flows)" : ""}.`]),
        `Create the assignment ${id} and have Crane check it.`,
        `Activate it for ${lifetime} days (admin key). It stays idle until something wakes it; no model runs while idle. Renew it before it ends to keep it running.`,
      ];
      if (!(await dialogs.confirm(`put ${definition.name} to work on ${target}?`, steps.map((step, index) => `${index + 1}. ${step}`).join("\n"), "Put to work"))) return CANCELLED;

      if (definition.state === "draft") {
        progress(`Activating the definition ${ref}`);
        const activated = await call(deps, ["resident", "definition", "activate", ref, "--reason", reason]);
        if (!activated.envelope || !succeeded(activated.envelope.status)) return { ok: false, message: `Nothing was assigned: activating ${ref} did not finish. ${activated.error ?? friendlyError(activated.envelope?.summary ?? "")}` };
      }
      if (!covered) {
        progress(`Signing that ${agent} works in ${target}`);
        const assigned = await call(deps, ["brick", "assign", agent, "--targets", mergeTargets(current?.targets ?? [], target).join(",")]);
        if (!assigned.envelope || !succeeded(assigned.envelope.status)) return { ok: false, message: `${agent} could not be assigned to ${target}. ${assigned.error ?? friendlyError(assigned.envelope?.summary ?? "")}` };
      }
      progress(`Creating ${id}`);
      const created = await call(deps, ["resident", "create", "--file", file.file]);
      if (!created.envelope || !succeeded(created.envelope.status)) return { ok: false, message: `Could not create ${id}. ${created.error ?? friendlyError(created.envelope?.summary ?? "")}` };
      progress(`Checking ${id} against the signed rules`);
      const checked = await call<{ validation?: { problems?: unknown } }>(deps, ["resident", "validate", id]);
      if (!checked.envelope) return { ok: false, message: `Created ${id}, but checking it did not finish. ${checked.error ?? ""}` };
      if (!succeeded(checked.envelope.status)) return { ok: false, message: `Created ${id}, but it does not pass Crane's checks yet, so it is not working. Fix these, then press Activate on it under 'Not working yet'.`, problems: problemLines(checked.envelope.data?.validation?.problems) };
      progress(`Activating ${id}`);
      const activated = await call(deps, ["resident", "activate", id, "--reason", reason]);
      if (!activated.envelope || !succeeded(activated.envelope.status)) return { ok: false, message: `Created and checked ${id}, but activating it did not finish; it is listed under 'Not working yet'. ${activated.error ?? friendlyError(activated.envelope?.summary ?? "")}` };
      deps.record("FOXX_RESIDENT_COMMAND", { command: "resident.activate", status: "OK", statement: activated.envelope.summary });
      return { ok: true, message: `${definition.name} now works on ${target} as ${id}, for ${lifetime} days. It stays idle until something wakes it.` };
    } finally {
      file.dispose();
    }
  }
}

/** One node of the Agents sidebar list. */
class AgentNode extends vscode.TreeItem {}

/** The Agents sidebar list: an entry that opens the panel, then the agents working and not yet. */
export class AgentsTree implements vscode.TreeDataProvider<AgentNode> {
  private readonly changed = new vscode.EventEmitter<void>();
  readonly onDidChangeTreeData = this.changed.event;
  private list: ResidentList | undefined;
  private problem: string | undefined;

  constructor(private readonly deps: PanelDeps) {}

  /** Read the agents from Crane again. */
  async load(): Promise<void> {
    const listed = await call<ResidentList>(this.deps, ["resident", "list"]);
    this.list = listed.envelope?.data;
    this.problem = listed.error;
    this.changed.fire();
  }

  /**
   * Return a node as a tree item.
   * @param node node
   * @returns the node
   */
  getTreeItem(node: AgentNode): vscode.TreeItem {
    return node;
  }

  /**
   * Return the entries (the list is flat).
   * @returns nodes
   */
  getChildren(): AgentNode[] {
    const open = new AgentNode("Open the Resident Agents panel");
    open.iconPath = new vscode.ThemeIcon("window");
    open.command = { command: "foxx.agents.openPanel", title: "Open" };
    if (!this.list) {
      const note = new AgentNode(this.problem ?? "Loading…");
      note.iconPath = new vscode.ThemeIcon(this.problem ? "warning" : "loading~spin");
      return [open, note];
    }
    const working = this.list.assignments.filter((row) => row.lifecycle === "active" || row.lifecycle === "paused");
    const busy = new Set(working.map((row) => row.definition.split("@")[0]));
    const waiting = this.list.definitions.filter((row) => row.state !== "revoked" && !busy.has(row.definition_id));
    const nodes = working.map((row) => {
      const node = new AgentNode(row.assignment_id);
      node.description = `${row.status.toLowerCase().replace(/_/g, " ")} · ${row.target}`;
      node.tooltip = new vscode.MarkdownString(residentMarkdown(row));
      node.iconPath = new vscode.ThemeIcon("robot", new vscode.ThemeColor(residentTone(row.status) === "ok" ? "testing.iconPassed" : residentTone(row.status) === "bad" ? "testing.iconFailed" : "editorWarning.foreground"));
      node.command = { command: "foxx.agents.openPanel", title: "Open", arguments: [row.target] };
      return node;
    });
    const latest = new Map<string, (typeof waiting)[number]>();
    for (const row of waiting) if ((latest.get(row.definition_id)?.version ?? 0) < row.version) latest.set(row.definition_id, row);
    for (const row of latest.values()) {
      const node = new AgentNode(row.name);
      node.description = `${row.ref} · not working yet`;
      node.iconPath = new vscode.ThemeIcon("circle-outline");
      node.command = { command: "foxx.agents.openPanel", title: "Open" };
      nodes.push(node);
    }
    return [open, ...nodes];
  }
}

/** The Resident Agents panel page. */
export const AGENTS_PAGE: WorkbenchPage = {
  viewType: AGENTS_PANEL,
  title: "Landscape Resident Agents",
  styles: `
.step { display: grid; grid-template-columns: auto minmax(0, 1fr) auto; gap: 8px; align-items: start; padding: 6px 8px; border: 1px solid var(--line); border-radius: 6px; background: var(--bg); }
.step.disabled { opacity: .6; }
.step small { display: block; color: var(--muted); font-size: 11px; }
.step .auto { font-size: 11px; color: var(--muted); white-space: nowrap; }
.assign { margin-top: 10px; padding: 10px; border: 1px dashed var(--fox); border-radius: 8px; background: var(--fox-soft); }
.chips { display: flex; gap: 4px; flex-wrap: wrap; margin-top: 6px; }
`,
  body: `<div class="shell">
<header class="top"><span class="mark" aria-hidden="true">◉</span><div class="grow"><b>Resident agents</b><div class="muted small" id="summary">Loading…</div></div><button class="btn" id="refresh" title="Ask Crane again">Refresh</button></header>
<div id="notice" aria-live="polite"></div>
<div id="problem"></div>
<div class="layout">
  <section class="card"><div class="head"><b class="grow">Landscape</b><span class="muted small">Click a Zone or Flow to see who works there</span></div><div class="graph" id="graph" data-item="Agent" data-glyph="◉"></div></section>
  <section class="card"><div class="head"><b class="grow" id="selected-title">Selected</b></div><div class="body" id="selected"></div></section>
</div>
<section class="card">
  <div class="tabs" role="tablist">
    <button data-tab="create" role="tab" aria-selected="true" id="tab-create">Create agent</button>
    <button data-tab="waiting" role="tab">Not working yet<span class="count" id="count-waiting"></span></button>
    <button data-tab="working" role="tab">Working<span class="count" id="count-working"></span></button>
  </div>
  <div class="body">
    <form data-panel="create" id="d-form" novalidate>
      <div id="d-editing" class="banner info" hidden><span class="icon">ℹ</span><div class="grow small" id="d-editing-text"></div><button type="button" class="btn small" id="d-new">Start a new agent instead</button></div>
      <div class="grid2">
        <div class="field" data-field="name"><label for="d-name">Name</label><input type="text" id="d-name" placeholder="CI failure triage"></div>
        <div class="field" data-field="id"><label for="d-id">Id</label><input type="text" id="d-id" placeholder="ci-triage" spellcheck="false"><div class="help">Letters, digits, '_' or '-'. It never changes.</div></div>
      </div>
      <div class="field" data-field="purpose"><label for="d-purpose">Purpose</label><textarea id="d-purpose" placeholder="Its mission and responsibility, in plain words. It is given to the agent as context."></textarea></div>
      <div class="field" data-field="owner"><label for="d-owner">Owner</label><input type="text" id="d-owner" placeholder="Person or team who answers for it"></div>
      <div class="field" data-field="triggers"><div class="label">What wakes it up</div><div class="checks" id="d-triggers"></div><div class="help">It stays idle (no process, no model) until one of these happens.</div></div>
      <div class="field" data-field="actions"><div class="label">What it may do</div><div class="checks" id="d-actions"></div><div class="help">"Without asking" lets a step run without a person's approval when Crane allows it; Crane can still require one.</div></div>
      <details class="more"><summary>Safety switches</summary><div class="checks" style="margin-top:8px">
        <label class="check"><input type="checkbox" id="d-verify" checked><span class="grow">Verify the outcome of every external step<small>A plan that changes another system must also check what happened.</small></span></label>
        <label class="check"><input type="checkbox" id="d-distinct"><span class="grow">The person who activates it may not approve its steps<small>Separation of duties.</small></span></label>
        <label class="check"><input type="checkbox" id="d-remote"><span class="grow">Allow approvals and answers from the DataSea dashboard<small>Only from a GitHub-verified admin of the repository.</small></span></label>
        <div class="field" data-field="onMissing" style="margin:6px 0 0"><label for="d-missing">When context it needs cannot be loaded</label><select id="d-missing"><option value="block">Stop and say why</option><option value="ask">Ask a person</option></select></div>
      </div></details>
      <details class="more"><summary>Bricks it expects on its Zone or Flow <span class="muted">(optional)</span></summary><div class="checks" id="d-bricks" style="margin-top:8px"></div></details>
      <details class="more"><summary>Limits <span class="muted">(the defaults are conservative)</span></summary><div class="grid2" id="d-limits" style="margin-top:8px"></div></details>
      <label class="check"><input type="checkbox" id="d-now"><span class="grow">Activate it now<small>Signed with an admin key. Without it, an admin activates it later from 'Not working yet'.</small></span></label>
      <div class="field" data-field="reason" id="d-reason-box" hidden style="margin-top:8px"><label for="d-reason">Why?</label><input type="text" id="d-reason" placeholder="Recorded in the signed audit"></div>
      <div class="row" style="margin-top:12px"><button class="btn primary" id="d-submit" type="submit">Create agent</button><span class="muted small">It does nothing until it is put to work on a Zone or Flow.</span></div>
    </form>
    <div data-panel="waiting" hidden>
      <label class="check" style="margin-bottom:10px"><input type="checkbox" id="w-all"><span class="grow">Also show agents that already work somewhere<small>One agent can work on several Zones and Flows.</small></span></label>
      <div class="list" id="waiting"></div>
    </div>
    <div data-panel="working" hidden><div class="list" id="working"></div></div>
  </div>
</section>
</div>`,
  script: `
let S = { graph: { nodes: [], edges: [] }, targets: [], definitions: [], assignments: [], catalog: { steps: [], triggers: [] }, bricks: [], triggers: {}, limits: [] };
let editing = null;
let assigning = null;
let presetTarget = "";
const graph = Graph($("graph"), (node) => { renderSelected(node); if (node && node.kind !== "item") { presetTarget = node.id; for (const select of document.querySelectorAll("[data-targets]")) select.value = node.id; } });
const human = (word) => String(word || "").replace(/_/g, " ").toLowerCase();
const toneOf = (status) => status === "ACTIVE_IDLE" ? "ok" : ["QUEUED", "INVESTIGATING", "EXECUTING", "WAITING_FOR_EVENT", "VALIDATED"].includes(status) ? "busy" : ["AWAITING_APPROVAL", "AWAITING_INPUT"].includes(status) ? "attention" : ["DEGRADED", "RECONCILIATION_REQUIRED", "REVOKED"].includes(status) ? "bad" : "";
const days = (at) => { if (!at) return ""; const left = Math.round((at - Date.now()) / 86400000); return left > 0 ? left + " day(s) left" : "ended"; };
const working = (row) => row.lifecycle === "active" || row.lifecycle === "paused";
const latestVersions = () => { const map = new Map(); for (const row of S.definitions) if (row.state !== "revoked" && (!map.has(row.definition_id) || map.get(row.definition_id).version < row.version)) map.set(row.definition_id, row); return [...map.values()]; };

function onState(state) {
  S = Object.assign(S, state);
  $("problem").innerHTML = state.problem ? '<div class="banner" role="alert"><span class="icon">⚠</span><div class="grow"><b>Crane could not list the resident agents</b><div>' + esc(state.problem) + "</div></div></div>" : state.graphError ? '<div class="banner info"><span class="icon">ℹ</span><div class="grow"><b>The graph is not available</b><div>' + esc(state.graphError) + "</div></div></div>" : "";
  const live = S.assignments.filter(working);
  const waiting = waitingDefinitions(false);
  $("summary").textContent = live.length + " working · " + waiting.length + " not working yet · " + (S.idle || 0) + " model call(s) while idle";
  $("count-working").textContent = String(live.length);
  $("count-waiting").textContent = String(waiting.length);
  graph.set(S.graph);
  if (!$("d-triggers").children.length && (S.catalog.triggers.length || S.catalog.steps.length)) buildForm();
  renderBricks();
  renderWaiting();
  renderWorking();
  const picked = graph.picked();
  renderSelected(picked ? S.graph.nodes.find((node) => node.id === picked) : null);
}
function onFocus(id) { graph.pick(id); }
$("refresh").addEventListener("click", () => { notice(); vscode.postMessage({ type: "refresh" }); });

// ---------------------------------------------------------------- the definition form
function buildForm() {
  $("d-triggers").innerHTML = S.catalog.triggers.map((item) => {
    const known = S.triggers[item.source];
    return '<label class="check' + (item.supported ? "" : " disabled") + '"><input type="checkbox" data-trigger="' + esc(item.source) + '"' + (item.supported ? "" : " disabled") + '><span class="grow">' + esc(known ? known.label : human(item.source)) + "<small>" + esc(item.supported ? item.delivery || "" : "Not available yet: no connector in this Crane.") + "</small></span></label>";
  }).join("");
  $("d-actions").innerHTML = S.catalog.steps.map((step) =>
    '<div class="step' + (step.available ? "" : " disabled") + '"><input type="checkbox" data-step="' + esc(step.type) + '" aria-label="' + esc(human(step.type)) + '"><span>' + esc(human(step.type)) + ' <span class="tag' + (step.effect === "external" ? " warn" : "") + '">' + esc(human(step.effect)) + "</span>" + (step.available ? "" : "<small>Not available yet: " + esc(step.unavailable_reason || "") + "</small>") + '</span><label class="auto"><input type="checkbox" data-auto="' + esc(step.type) + '"> without asking</label></div>').join("");
  $("d-limits").innerHTML = S.limits.map((limit) => '<div class="field" data-field="limit:' + esc(limit.key) + '"><label for="l-' + esc(limit.key) + '">' + esc(limit.label) + '</label><input type="number" min="0" max="' + limit.ceiling + '" step="1" id="l-' + esc(limit.key) + '" value="' + limit.value + '"><div class="help">Up to ' + limit.ceiling + "</div></div>").join("");
}
function renderBricks() {
  const box = $("d-bricks");
  const checked = new Set([...box.querySelectorAll("input:checked")].map((input) => input.dataset.brick));
  box.innerHTML = S.bricks.length ? S.bricks.map((brick) => '<label class="check"><input type="checkbox" data-brick="' + esc(brick.ref) + '"' + (checked.has(brick.ref) ? " checked" : "") + '><span class="grow">' + esc(brick.name) + "<small>" + esc(brick.ref + " · " + brick.type) + "</small></span></label>").join("") : '<div class="muted small">No activated Brick yet. Crane checks these when the agent is put to work.</div>';
}
function readForm() {
  const limits = {};
  for (const limit of S.limits) limits[limit.key] = Number($("l-" + limit.key).value);
  return {
    id: $("d-id").value.trim(), version: editing ? editing.next : 1, name: $("d-name").value.trim(), purpose: $("d-purpose").value, owner: $("d-owner").value,
    bricks: [...document.querySelectorAll("#d-bricks input:checked")].map((input) => input.dataset.brick),
    triggers: [...document.querySelectorAll("#d-triggers input:checked")].map((input) => input.dataset.trigger),
    actions: [...document.querySelectorAll("#d-actions input[data-step]:checked")].map((input) => ({ type: input.dataset.step, auto: document.querySelector('[data-auto="' + input.dataset.step + '"]').checked })),
    verification: $("d-verify").checked, onMissing: $("d-missing").value, limits, remote: $("d-remote").checked, distinct: $("d-distinct").checked,
  };
}
function fillForm(definition) {
  $("d-id").value = definition.definition_id || ""; $("d-name").value = definition.name || ""; $("d-purpose").value = definition.purpose || ""; $("d-owner").value = definition.owner || "";
  for (const input of document.querySelectorAll("#d-triggers input")) input.checked = (definition.triggers || []).includes(input.dataset.trigger);
  for (const input of document.querySelectorAll("#d-actions input[data-step]")) { const policy = (definition.actions || []).find((item) => item.step_type === input.dataset.step); input.checked = Boolean(policy); document.querySelector('[data-auto="' + input.dataset.step + '"]').checked = Boolean(policy && policy.auto); }
  for (const input of document.querySelectorAll("#d-bricks input")) input.checked = (definition.bricks || []).includes(input.dataset.brick);
  $("d-verify").checked = definition.verification_required !== false; $("d-missing").value = definition.on_missing_context || "block";
  $("d-remote").checked = Boolean(definition.remote_decisions); $("d-distinct").checked = Boolean(definition.distinct_approver);
  for (const limit of S.limits) $("l-" + limit.key).value = definition.limits && definition.limits[limit.key] != null ? definition.limits[limit.key] : limit.value;
}
function setEditing(next) {
  editing = next;
  $("d-editing").hidden = !editing;
  $("d-id").readOnly = Boolean(editing);
  $("d-submit").textContent = editing ? "Save version " + editing.next : "Create agent";
  $("tab-create").textContent = editing ? "Update agent" : "Create agent";
  if (editing) $("d-editing-text").innerHTML = "Updating <b>" + esc(editing.id) + "</b>: saving creates version " + editing.next + ". Saved versions never change, so agents already working keep version " + editing.from + " until they are put to work with the new one.";
}
$("d-new").addEventListener("click", () => { setEditing(null); fillForm({}); $("d-id").value = ""; });
$("d-now").addEventListener("change", () => { $("d-reason-box").hidden = !$("d-now").checked; });
$("d-name").addEventListener("input", () => { if (!editing && !$("d-id").dataset.touched) $("d-id").value = $("d-name").value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").replace(/^[^a-z]+/, "").slice(0, 64); });
$("d-id").addEventListener("input", () => { $("d-id").dataset.touched = "1"; });
$("d-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("d-form"));
  const definition = readForm();
  perform($("d-submit"), "saveDefinition", { definition, now: $("d-now").checked, reason: $("d-reason").value.trim() }, () => {
    setEditing(null); fillForm({}); $("d-id").value = ""; delete $("d-id").dataset.touched; $("d-now").checked = false; $("d-reason-box").hidden = true; $("d-reason").value = ""; selectTab("waiting");
  }).then((result) => { if (!result.ok && result.fields) { const first = document.querySelector("#d-form .field.error"); if (first) { const details = first.closest("details"); if (details) details.open = true; first.scrollIntoView({ block: "center" }); } } });
});

// ---------------------------------------------------------------- not working yet
function waitingDefinitions(all) {
  const busy = new Set(S.assignments.filter(working).map((row) => row.definition.split("@")[0]));
  return latestVersions().filter((row) => all || !busy.has(row.definition_id));
}
function assignForm(row) {
  const target = presetTarget || "";
  const triggers = (row.triggers || []).filter((source) => S.triggers[source]);
  const wake = triggers.map((source) => source === "timer"
    ? '<div class="field" data-field="wake"><label for="w-timer">' + esc(S.triggers[source].label) + ': every how many minutes? <span class="muted">(empty: no timer)</span></label><input type="number" min="5" id="w-timer" data-wake="timer" value="60"></div>'
    : '<div class="field"><label for="w-' + esc(source) + '">' + esc(S.triggers[source].label) + ' <span class="muted">(kinds, comma separated; empty: not this)</span></label><input type="text" id="w-' + esc(source) + '" data-wake="' + esc(source) + '" value="' + esc(S.triggers[source].kinds) + '" spellcheck="false"></div>').join("");
  const options = '<option value="">Choose a Zone or Flow</option>' + S.targets.map((item) => '<option value="' + esc(item.key) + '"' + (item.key === target ? " selected" : "") + ">" + esc(item.key + (item.status !== "active" ? " (" + item.status + ")" : "")) + "</option>").join("");
  return '<form class="assign" id="assign-form" novalidate>' +
    '<div class="grid2"><div class="field" data-field="target"><label for="w-target">Zone or Flow</label><select id="w-target" data-targets>' + options + '</select><div class="help">Or click one in the graph.</div></div>' +
    '<div class="field" data-field="agent"><label for="w-agent">Agent</label><input type="text" id="w-agent" value="claude" list="w-agents" spellcheck="false"><datalist id="w-agents"><option value="claude"><option value="codex"></datalist><div class="help">The provider or agent id that does the work.</div></div></div>' +
    '<div data-field="wake">' + (wake || '<div class="muted small">This definition names no trigger source Landscape can set up.</div>') + "</div>" +
    '<div class="grid2"><div class="field" data-field="lifetime"><label for="w-lifetime">Keep it working for</label><select id="w-lifetime"><option value="30">30 days</option><option value="60">60 days</option><option value="90" selected>90 days (the longest one period can be)</option></select><div class="help">Renew it before it ends to keep it running.</div></div>' +
    '<div class="field" data-field="id"><label for="w-id">Assignment id</label><input type="text" id="w-id" spellcheck="false" value="' + esc((row.definition_id + "-" + (target.split(":")[1] || "")).replace(/-$/, "").slice(0, 64)) + '"></div></div>' +
    '<div class="field" data-field="reason"><label for="w-reason">Why does it start working?</label><input type="text" id="w-reason" placeholder="Recorded in the signed audit"></div>' +
    '<div class="row"><button class="btn primary" id="w-submit" type="submit">Put to work</button><button class="btn" type="button" data-cancel-assign>Cancel</button><span class="muted small">You confirm every step before anything is signed.</span></div></form>';
}
function renderWaiting() {
  const rows = waitingDefinitions($("w-all").checked);
  const parked = (id) => S.assignments.filter((item) => item.definition.split("@")[0] === id && !working(item) && item.lifecycle !== "revoked");
  $("waiting").innerHTML = rows.length ? rows.map((row) => {
    const parkedRows = parked(row.definition_id);
    const live = S.assignments.filter((item) => item.definition.split("@")[0] === row.definition_id && working(item));
    return '<div class="item' + (assigning === row.ref ? " picked" : "") + '"><div class="title"><b>' + esc(row.name) + '</b><span class="muted small mono">' + esc(row.ref) + '</span><span class="tag ' + (row.state === "active" ? "ok" : "warn") + '">' + (row.state === "active" ? "ready" : "draft: an admin activates it") + "</span>" + (live.length ? '<span class="tag busy">works on ' + live.length + "</span>" : "") + "</div>" +
      (row.owner ? '<div class="muted small">Owner: ' + esc(row.owner) + "</div>" : "") +
      '<div class="chips">' + (row.triggers || []).map((source) => '<span class="tag">wakes on ' + esc(human(source)) + "</span>").join("") + (row.actions || []).map((item) => '<span class="tag' + (item.auto ? " fox" : "") + '">' + esc(human(item.step_type)) + (item.auto ? " · without asking" : "") + "</span>").join("") + "</div>" +
      parkedRows.map((item) => '<div class="row small" style="margin-top:6px"><span class="tag">' + esc(item.lifecycle) + "</span><b>" + esc(item.assignment_id) + '</b><span class="muted">on ' + esc(item.target) + '</span><span class="grow"></span>' +
        (item.lifecycle === "expired" ? '<button class="btn small primary" data-change="renew" data-id="' + esc(item.assignment_id) + '">Renew</button>' : '<button class="btn small primary" data-activate-assignment="' + esc(item.assignment_id) + '">Activate</button>') +
        '<button class="btn small danger" data-change="revoke" data-id="' + esc(item.assignment_id) + '">Revoke</button></div>').join("") +
      '<div class="actions"><button class="btn small primary" data-assign="' + esc(row.ref) + '">Put to work on a Zone or Flow</button><button class="btn small" data-update="' + esc(row.ref) + '">Update</button>' + (row.state === "draft" ? '<button class="btn small" data-activate-definition="' + esc(row.ref) + '">Activate (admin)</button>' : "") + "</div>" +
      (assigning === row.ref ? assignForm(row) : "") + "</div>";
  }).join("") : '<div class="empty-note">' + (S.definitions.length ? "Every agent is working. Tick the box above to put one on another Zone or Flow too." : 'No agents yet. Create one in "Create agent".') + "</div>";
}
$("w-all").addEventListener("change", renderWaiting);

// ---------------------------------------------------------------- working
function agentCard(row) {
  const tone = toneOf(row.status);
  return '<div class="item"><div class="title"><b>' + esc(row.assignment_id) + '</b><span class="tag ' + tone + '">' + esc(human(row.status)) + '</span><span class="muted small">on </span><button type="button" class="chip-btn" data-goto="' + esc(row.target) + '" title="Show it in the graph"><span aria-hidden="true">' + (row.target.startsWith("flow:") ? "⇢" : "▦") + "</span>" + esc(row.target) + '</button><span class="muted small">' + " · " + esc(row.agent) + " · " + esc(row.definition) + "</span></div>" +
    '<div class="muted small" style="margin-top:4px">' + esc([days(row.expires_at), row.pending_approvals ? row.pending_approvals + " approval(s) waiting" : "", row.latest_activation ? "last woke: " + human(row.latest_activation.state) : "idle until something wakes it", row.health && row.health.reasons && row.health.reasons.length ? "Crane: " + row.health.reasons.join(", ") : ""].filter(Boolean).join(" · ")) + "</div>" +
    '<div class="actions">' + (row.lifecycle === "active" ? '<button class="btn small" data-change="pause" data-id="' + esc(row.assignment_id) + '">Pause</button>' : row.lifecycle === "paused" ? '<button class="btn small primary" data-change="resume" data-id="' + esc(row.assignment_id) + '">Resume</button>' : "") +
    '<button class="btn small" data-context="' + esc(row.assignment_id) + '">What it knows</button><button class="btn small" data-show="' + esc(row.assignment_id) + '">Details</button><button class="btn small danger" data-change="revoke" data-id="' + esc(row.assignment_id) + '">Revoke</button></div></div>';
}
function renderWorking() {
  const rows = S.assignments.filter(working);
  const revoked = S.assignments.filter((row) => row.lifecycle === "revoked");
  $("working").innerHTML = (rows.length ? rows.map(agentCard).join("") : '<div class="empty-note">No agent is working yet. Put one to work from "Not working yet".</div>') +
    (revoked.length ? '<details class="more"><summary>Revoked <span class="muted">' + revoked.length + "</span></summary>" + revoked.map((row) => '<div class="small" style="padding:3px 0"><b>' + esc(row.assignment_id) + '</b> <span class="muted">on ' + esc(row.target) + "</span></div>").join("") + "</details>" : "");
}

// ---------------------------------------------------------------- the selected Zone, Flow, or agent
function renderSelected(node) {
  const box = $("selected");
  if (!node) { $("selected-title").textContent = "Selected"; box.innerHTML = '<div class="empty-note">Click a Zone or Flow in the graph to see the agents working there and put a new one to work.</div>'; return; }
  if (node.kind === "item") {
    const row = S.assignments.find((item) => "item:" + item.assignment_id === node.id);
    $("selected-title").textContent = "Agent";
    box.innerHTML = row ? agentCard(row) : '<div class="empty-note">This assignment is gone.</div>';
    return;
  }
  const rows = S.assignments.filter((row) => row.target === node.id && row.lifecycle !== "revoked");
  $("selected-title").textContent = (node.kind === "flow" ? "Flow " : "Zone ") + node.name;
  box.innerHTML = '<div class="row" style="margin-bottom:10px"><span class="tag ' + (node.kind === "flow" ? "fox" : "busy") + '">' + esc(node.kind) + '</span><span class="muted small grow">' + esc(node.sub) + "</span></div>" +
    (node.missing ? '<div class="banner"><span class="icon">⚠</span><div class="grow small">Agents are assigned to this ' + esc(node.kind) + ", but it is not in the signed governance, so they cannot act. Revoke them, or create the " + esc(node.kind) + " again.</div></div>" : "") +
    '<div class="list">' + (rows.length ? rows.map(agentCard).join("") : '<div class="empty-note">No agent works here yet.</div>') + "</div>" +
    (node.missing ? "" : '<div class="row" style="margin-top:10px"><button class="btn primary" data-work-here="' + esc(node.id) + '">Put an agent to work here</button></div>');
}

// ---------------------------------------------------------------- buttons anywhere on the page
document.addEventListener("click", async (event) => {
  const el = event.target.closest("[data-assign],[data-update],[data-activate-definition],[data-activate-assignment],[data-change],[data-context],[data-show],[data-goto],[data-work-here],[data-cancel-assign]");
  if (!el) return;
  event.preventDefault();
  const d = el.dataset;
  if (d.assign) { assigning = assigning === d.assign ? null : d.assign; renderWaiting(); const form = $("assign-form"); if (form) form.scrollIntoView({ block: "nearest" }); }
  else if (d.cancelAssign !== undefined) { assigning = null; renderWaiting(); }
  else if (d.update) {
    const row = S.definitions.find((item) => item.ref === d.update);
    const result = await ask("load:definition", { ref: d.update });
    if (!result.ok) { notice("bad", result.message); return; }
    const newest = Math.max(...S.definitions.filter((item) => item.definition_id === row.definition_id).map((item) => item.version));
    selectTab("create"); fillForm(result.data || {}); setEditing({ id: row.definition_id, from: row.version, next: newest + 1 }); $("d-name").focus();
  }
  else if (d.activateDefinition) perform(el, "activateDefinition", { ref: d.activateDefinition });
  else if (d.activateAssignment) perform(el, "activateAssignment", { id: d.activateAssignment });
  else if (d.change) perform(el, "change", { id: d.id, action: d.change });
  else if (d.context) perform(el, "context", { id: d.context });
  else if (d.show) perform(el, "show", { id: d.show });
  else if (d.goto) { graph.pick(d.goto); $("graph").scrollIntoView({ block: "nearest" }); }
  else if (d.workHere) { presetTarget = d.workHere; $("w-all").checked = true; selectTab("waiting"); renderWaiting(); notice("info", "Choose an agent below and press 'Put to work on a Zone or Flow'. " + d.workHere + " is already chosen."); }
});
document.addEventListener("change", (event) => {
  if (event.target.id === "w-target") { presetTarget = event.target.value; const id = $("w-id"); const row = S.definitions.find((item) => item.ref === assigning); if (id && row && !id.dataset.touched) id.value = (row.definition_id + "-" + (presetTarget.split(":")[1] || "")).replace(/-$/, "").slice(0, 64); }
});
document.addEventListener("input", (event) => { if (event.target.id === "w-id") event.target.dataset.touched = "1"; });
document.addEventListener("submit", (event) => {
  if (event.target.id !== "assign-form") return;
  event.preventDefault();
  clearErrors(event.target);
  const wake = {};
  for (const input of event.target.querySelectorAll("[data-wake]")) wake[input.dataset.wake] = input.value.trim();
  perform($("w-submit"), "assign", { definition: assigning, target: $("w-target").value, agent: $("w-agent").value.trim(), wake, lifetime: Number($("w-lifetime").value), id: $("w-id").value.trim(), reason: $("w-reason").value.trim() }, () => { assigning = null; selectTab("working"); });
});
renderSelected(null);
vscode.postMessage({ type: "ready" });
`,
};
