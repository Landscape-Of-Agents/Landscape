// What the Bricks and Resident Agents panels share and compute without the editor: the landscape
// graph with Bricks or agents hung on their Zones and Flows, the plain-language form of what Crane
// refused, and the resident definition a person fills in. Crane stays the authority: nothing here
// validates or decides; it only shapes what Crane reported and what Crane will be asked.
// Editor-independent, so it is unit-tested.

import { parse as parseYaml } from "yaml";

import { GraphData } from "./graph";

/** A node of a panel's graph: a Zone, a Flow, or something attached to one. */
export interface OverlayNode {
  id: string;
  kind: "zone" | "flow" | "item";
  name: string;
  sub: string;
  /** ok, busy, attention, bad, or off: the color of an attached item. */
  tone?: string;
  /** A target named by an attachment that the graph does not have. */
  missing?: boolean;
}

/** An edge of a panel's graph: a Flow's stages, or an item hung on its target. */
export interface OverlayEdge {
  from: string;
  to: string;
  kind: "stage" | "next" | "attached";
}

/** Something to hang on a Zone or Flow: a Brick attachment or a resident assignment. */
export interface OverlayItem {
  id: string;
  label: string;
  sub: string;
  /** "zone:ID" or "flow:ID". */
  target: string;
  tone?: string;
}

/**
 * Build a panel's graph: the Zones and Flows of the governance graph (no selections), their stage
 * links, and each item linked from its target. A target the graph does not know still appears,
 * marked missing, so nothing attached is hidden.
 * @param graph the governance graph, when Crane reported one
 * @param items attachments or assignments
 * @returns nodes and edges
 */
export function overlayGraph(graph: GraphData | undefined, items: OverlayItem[]): { nodes: OverlayNode[]; edges: OverlayEdge[] } {
  const nodes: OverlayNode[] = [];
  const edges: OverlayEdge[] = [];
  for (const node of graph?.nodes ?? []) {
    if (node.kind === "zone") nodes.push({ id: node.id, kind: "zone", name: node.name, sub: `zone${node.status ? " · " + node.status : ""}` });
    else if (node.kind === "flow") nodes.push({ id: node.id, kind: "flow", name: node.name, sub: `flow · ${(node.stages ?? []).length} stage(s)${node.status ? " · " + node.status : ""}` });
  }
  const known = new Set(nodes.map((node) => node.id));
  for (const edge of graph?.edges ?? []) {
    if ((edge.kind === "stage" || edge.kind === "next") && known.has(edge.from) && known.has(edge.to)) edges.push({ from: edge.from, to: edge.to, kind: edge.kind });
  }
  for (const item of items) {
    if (!known.has(item.target)) {
      const [kind, name] = item.target.split(/:(.*)/s);
      nodes.push({ id: item.target, kind: kind === "flow" ? "flow" : "zone", name: name || item.target, sub: "not in this repository's graph", missing: true });
      known.add(item.target);
    }
    const id = `item:${item.id}`;
    nodes.push({ id, kind: "item", name: item.label, sub: item.sub, tone: item.tone });
    edges.push({ from: item.target, to: id, kind: "attached" });
  }
  return { nodes, edges };
}

/** Plain-language forms of what Crane says when it refuses or fails, most specific first. */
const FRIENDLY: [RegExp, string][] = [
  [/needs the repository owner's or an admin's signing key|admin key|not an admin|no signing key/i, "Only a repository admin can do this step: it is signed with an admin's key. Ask an admin to run it, or add your key with 'crane admin'."],
  [/unknown command 'brick'/i, "This Crane has no Bricks yet. Update Crane to the latest release."],
  [/unknown command 'resident'/i, "This Crane has no resident agents yet. Update Crane to the latest release."],
  [/not set up|no \.crane|registry\.json/i, "Crane is not set up in this repository yet. Run 'Landscape: Set Up Crane in This Repository' first."],
  [/does not verify|unverified|SIGNED_STATE_UNVERIFIED/i, "The signed rules of this repository do not verify, so Crane stops every change. Run 'Validate Governance' and fix what it reports."],
  [/versions are immutable/i, "That version already exists and can no longer change. Save it as the next version instead."],
  [/already exists/i, "Something with that id already exists. Pick another id."],
  [/a type never changes/i, "A Brick's type cannot change. Create a new Brick of the other type instead."],
  [/drafts cannot be attached|is a draft|not active/i, "This is still a draft. Activate it first, then attach or assign it."],
  [/revoked/i, "This was revoked and can no longer be used. Create or activate a new version."],
  [/is not valid JSON|not JSON|expected value|EOF while parsing/i, "The configuration is not valid JSON. Check brackets, quotes, and commas."],
  [/unknown field/i, "The configuration has a field this type does not know. Remove it or check its spelling."],
  [/no (Zone|Flow|zone|flow)|unknown target|TARGET_MISSING/i, "That Zone or Flow does not exist in the signed governance. Pick one from the graph."],
  [/TARGET_NOT_ACTIVE|proposed/i, "That Zone or Flow is only proposed. Activate it in the Landscape panel first."],
  [/NO_SIGNED_ASSIGNMENT/i, "The agent is not assigned to this Zone or Flow in the signed Brick store yet."],
  [/secret-looking|DEFINITION_SECRET/i, "A field looks like it holds a secret (a key or token). Remove it: context and definitions are shared in the repository."],
  [/could not run|ENOENT|not recognized/i, "Landscape could not start Crane. Install Crane or set 'foxx.cranePath' in the settings."],
  [/timed out|timeout/i, "Crane took too long to answer. Try again; if it keeps happening, check the Landscape output."],
];

/**
 * Turn what Crane printed into a sentence a person can act on, keeping Crane's own words after it.
 * @param message Crane's message (stderr, or an envelope summary)
 * @returns a friendly message
 */
export function friendlyError(message: string): string {
  const text = message.trim().replace(/^crane: /, "");
  if (!text) return "Crane stopped without saying why. Open the Landscape output for details.";
  const found = FRIENDLY.find(([pattern]) => pattern.test(text));
  return found ? `${found[1]} (Crane: ${text.length > 240 ? text.slice(0, 239) + "…" : text})` : text[0].toUpperCase() + text.slice(1);
}

/**
 * Describe validation problems Crane reported ([code, message] or {code, message}) as lines.
 * @param problems problems
 * @returns one line per problem
 */
export function problemLines(problems: unknown): string[] {
  if (!Array.isArray(problems)) return [];
  return problems.map((problem) => {
    if (Array.isArray(problem)) return `${String(problem[1] ?? problem[0])} (${String(problem[0])})`;
    const item = problem as { code?: string; message?: string };
    return `${item.message ?? ""}${item.code ? ` (${item.code})` : ""}`;
  });
}

/** The trigger sources a definition may name, and how an assignment subscribes to each. */
export const TRIGGER_DEFAULTS: Record<string, { label: string; kinds: string; scope: "target" | "repository" }> = {
  crane_event: { label: "Crane events (decisions, denials, sessions)", kinds: "crane.*", scope: "target" },
  governance_change: { label: "Signed governance changes", kinds: "governance.signed", scope: "repository" },
  brick_monitor: { label: "Brick monitors of the target", kinds: "monitor.*", scope: "target" },
  timer: { label: "A timer", kinds: "", scope: "target" },
  ci: { label: "CI results (GitHub Actions)", kinds: "ci.failed", scope: "repository" },
  webhook: { label: "Events delivered with 'crane resident trigger'", kinds: "webhook.*", scope: "target" },
};

/** The operating limits of a definition, their defaults, and their administrative ceilings. */
export const LIMITS: { key: string; label: string; value: number; ceiling: number }[] = [
  { key: "max_activations_per_day", label: "Wake-ups per day", value: 24, ceiling: 200 },
  { key: "max_steps", label: "Steps per plan", value: 8, ceiling: 20 },
  { key: "max_retries", label: "Retries of a failed step", value: 1, ceiling: 2 },
  { key: "max_model_calls", label: "Model calls per wake-up", value: 0, ceiling: 10 },
  { key: "cooldown_minutes", label: "Minutes between wake-ups", value: 10, ceiling: 10_080 },
  { key: "step_timeout_seconds", label: "Seconds per external call", value: 30, ceiling: 300 },
  { key: "approval_ttl_minutes", label: "Minutes an approval stays usable", value: 1440, ceiling: 10_080 },
  { key: "wait_minutes", label: "Minutes to wait for an answer", value: 1440, ceiling: 20_160 },
];

/** What a person enters for a resident definition (an agent's metaproperties). */
export interface DefinitionInput {
  id: string;
  version: number;
  name: string;
  purpose: string;
  owner: string;
  bricks: string[];
  triggers: string[];
  actions: { type: string; auto: boolean }[];
  verification: boolean;
  onMissing: "block" | "ask";
  limits: Record<string, number>;
  remote: boolean;
  distinct: boolean;
}

/**
 * Check a definition before Crane sees it, so the form can point at the field. Crane checks again.
 * @param input the form
 * @returns field → problem
 */
export function checkDefinition(input: DefinitionInput): Record<string, string> {
  const problems: Record<string, string> = {};
  if (!/^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(input.id)) problems.id = "Use letters, digits, '_' or '-', starting with a letter (at most 64).";
  if (!input.name.trim() || input.name.length > 120) problems.name = "Give it a name of 1 to 120 characters.";
  if (input.purpose.length > 4000) problems.purpose = "Keep the purpose under 4000 characters.";
  if (!input.triggers.length) problems.triggers = "Choose at least one thing that wakes the agent.";
  if (!input.actions.length) problems.actions = "Allow at least one kind of step.";
  for (const limit of LIMITS) {
    const value = input.limits[limit.key];
    if (value !== undefined && (!Number.isInteger(value) || value < 0 || value > limit.ceiling)) problems[`limit:${limit.key}`] = `Use a whole number from 0 to ${limit.ceiling}.`;
  }
  return problems;
}

/**
 * Build the file 'crane resident definition save --file' reads.
 * @param input the form
 * @returns the definition
 */
export function definitionFile(input: DefinitionInput): Record<string, unknown> {
  const limits: Record<string, number> = {};
  for (const limit of LIMITS) limits[limit.key] = input.limits[limit.key] ?? limit.value;
  return {
    definition_id: input.id,
    version: input.version,
    name: input.name.trim(),
    purpose: input.purpose.trim(),
    owner: input.owner.trim(),
    bricks: input.bricks,
    triggers: input.triggers,
    actions: input.actions.map((action) => ({ step_type: action.type, auto: action.auto })),
    verification_required: input.verification,
    on_missing_context: input.onMissing,
    limits,
    remote_decisions: input.remote,
    distinct_approver: input.distinct,
  };
}

/** What a person enters to put an agent to work on a Zone or Flow. */
export interface AssignInput {
  id: string;
  definition: string;
  agent: string;
  target: string;
  /** Trigger source → comma-separated kinds; a timer source holds its period in minutes. */
  wake: Record<string, string>;
  lifetimeDays: number;
}

/**
 * Build the assignment draft 'crane resident create --file' imports: one subscription per chosen
 * source (never a bare "*"), timers apart, and a lifetime of one period (renewal starts the next).
 * @param input the form
 * @returns the draft
 */
export function assignmentFile(input: AssignInput): Record<string, unknown> {
  const [definitionId, version] = input.definition.split("@");
  const subscriptions: Record<string, unknown>[] = [];
  const timers: Record<string, unknown>[] = [];
  for (const [source, value] of Object.entries(input.wake)) {
    if (source === "timer") {
      const minutes = Number(value);
      if (minutes >= 5) timers.push({ timer_id: "every-" + minutes + "-min", every_minutes: Math.round(minutes) });
      continue;
    }
    const kinds = value.split(",").map((kind) => kind.trim()).filter((kind) => kind && kind !== "*");
    if (kinds.length) subscriptions.push({ source, kinds, attributes: {}, scope: TRIGGER_DEFAULTS[source]?.scope ?? "target" });
  }
  return {
    assignment_id: input.id,
    definition_id: definitionId,
    definition_version: Number(version || 1),
    agent: input.agent,
    target: input.target,
    rca: null,
    subscriptions,
    timers,
    lifetime_days: Math.min(90, Math.max(1, Math.round(input.lifetimeDays))),
  };
}

/**
 * The targets an agent's signed Brick assignment should cover after adding one: 'crane brick
 * assign' replaces the agent's targets, so the ones it already has are kept.
 * @param current the agent's current targets ("zone:ID" or {kind, id})
 * @param target the target to add
 * @returns targets, without duplicates
 */
export function mergeTargets(current: (string | { kind: string; id: string })[], target: string): string[] {
  const keys = current.map((item) => (typeof item === "string" ? item : `${item.kind}:${item.id}`));
  return [...new Set([...keys, target])];
}

/** The most context one attachment can hold (Crane's MAX_CONTEXT_BYTES). */
export const MAX_CONTEXT_BYTES = 16 * 1024;

/** Where a volatile context comes from. */
export type ContextFormat = "text" | "yaml" | "json";

/** The fields a YAML or JSON file may hold its context in, checked in order. */
const CONTEXT_FIELDS = ["volatile_context", "context", "text"];

/**
 * Turn a file a person chose into volatile context text. A text file is used as written. A YAML or
 * JSON file must parse; a string document, or a "volatile_context", "context", or "text" field,
 * becomes the text; any other document is kept whole in a fenced block, so agents read it as data.
 * @param name file name, for messages
 * @param format how to read it
 * @param content file content
 * @returns the text, or why the file cannot be used
 */
export function contextFromFile(name: string, format: ContextFormat, content: string): { text?: string; error?: string } {
  if (content.includes("\u0000")) return { error: `${name} looks like a binary file. Choose a text, YAML, or JSON file.` };
  const raw = content.replace(/^\uFEFF/, "").replace(/\r\n/g, "\n").trim();
  let text = raw;
  if (format !== "text") {
    let value: unknown;
    try {
      value = format === "json" ? JSON.parse(raw) : parseYaml(raw);
    } catch (error) {
      return { error: `${name} is not valid ${format === "json" ? "JSON" : "YAML"}: ${(error as Error).message.split("\n")[0]}` };
    }
    const field = value && typeof value === "object" && !Array.isArray(value) ? CONTEXT_FIELDS.map((key) => (value as Record<string, unknown>)[key]).find((item) => typeof item === "string") : undefined;
    if (typeof value === "string") text = value.trim();
    else if (typeof field === "string") text = field.trim();
    else if (value === null || value === undefined) text = "";
    else text = "```" + format + "\n" + (format === "json" ? JSON.stringify(value, null, 2) : raw) + "\n```";
  }
  if (!text) return { error: `${name} holds no text to use as context.` };
  const bytes = Buffer.byteLength(text, "utf8");
  if (bytes > MAX_CONTEXT_BYTES) return { error: `${name} gives ${Math.ceil(bytes / 1024)} KB of context; one attachment holds at most ${MAX_CONTEXT_BYTES / 1024} KB. Shorten it, or put the long part in a Context Brick's files.` };
  return { text };
}
