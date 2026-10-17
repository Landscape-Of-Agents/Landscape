// The actions the Landscape panel offers, grouped under menu headings, and how the search box finds them.
// Typing filters actions whose words start with what was typed (prefix search, every typed word must
// match), ranked by how early the match is; the matches stay under their headings. Editor-independent,
// so it is unit-tested.

/** The form an action needs. */
export type Form =
  | "preserve"
  | "target"
  | "zone"
  | "flow"
  | "context"
  | "zone_attach"
  | "policy_context"
  | "policy"
  | "checkpoint"
  | "move"
  | "hooks"
  | "none";

/** The menu headings, in the order the menu shows them. */
export const GROUPS = ["Preserve", "Target", "Zones", "Flows", "Context", "Policies", "Checks", "Agents", "Setup"] as const;

/** A menu heading. */
export type Group = (typeof GROUPS)[number];

/** One action of the panel. */
export interface Action {
  id: string;
  label: string;
  detail: string;
  keywords: string[];
  form: Form;
  needsSelection: boolean;
  group: Group;
}

/** Every action, grouped and in the order shown when the search box is empty. */
export const ACTIONS: Action[] = [
  { id: "preserve", label: "Preserve these lines", detail: "The agent must not change them (crane protect)", keywords: ["protect", "keep", "lock", "freeze"], form: "preserve", needsSelection: true, group: "Preserve" },
  { id: "target", label: "Target these lines", detail: "The agent must change them (crane target)", keywords: ["change", "modify", "fix", "rewrite"], form: "target", needsSelection: true, group: "Target" },
  { id: "zone", label: "Make these lines a zone", detail: "A boundary with its own autonomy ceiling, criticality, and context", keywords: ["boundary", "autonomy", "ceiling", "criticality", "area"], form: "zone", needsSelection: true, group: "Zones" },
  { id: "zone_attach", label: "Add these lines to an existing zone", detail: "Extend a zone to cover this function or file", keywords: ["zone", "extend", "include", "cover"], form: "zone_attach", needsSelection: true, group: "Zones" },
  { id: "flow", label: "Make this function a flow", detail: "The function and everything it calls, with its own autonomy and context", keywords: ["entry", "path", "calls", "autonomy", "execution"], form: "flow", needsSelection: true, group: "Flows" },
  { id: "connect", label: "Build a flow from zones", detail: "Connect two or more zones, in order, into one flow (kept acyclic)", keywords: ["connect", "link", "join", "chain", "order", "zones", "stages"], form: "none", needsSelection: false, group: "Flows" },
  { id: "graph", label: "Show the governance graph", detail: "Zones, flows, and selections of the repository as a graph", keywords: ["dag", "map", "diagram", "visualize", "connections", "tree"], form: "none", needsSelection: false, group: "Flows" },
  { id: "context", label: "Add context to a zone or flow", detail: "Guidance the agent receives whenever it touches that code", keywords: ["guidance", "note", "instruction", "explain", "describe", "zone", "flow"], form: "context", needsSelection: false, group: "Context" },
  { id: "policy_context", label: "Attach this file as policy context", detail: "The whole file becomes guidance for a policy", keywords: ["context", "guidance", "document", "file", "policy"], form: "policy_context", needsSelection: false, group: "Context" },
  { id: "policy", label: "Create a policy", detail: "A named group of preserve and target rules", keywords: ["new", "rules", "group"], form: "policy", needsSelection: false, group: "Policies" },
  { id: "move", label: "Move a selection to another policy", detail: "Reassign a preserved or targeted block", keywords: ["reassign", "policy", "selection"], form: "move", needsSelection: false, group: "Policies" },
  { id: "status", label: "Policy status", detail: "A policy's rules and an enforcement self-test", keywords: ["report", "rules"], form: "none", needsSelection: false, group: "Policies" },
  { id: "checkpoint", label: "Create a checkpoint", detail: "Record the current commit as trusted", keywords: ["baseline", "snapshot", "commit", "trust"], form: "checkpoint", needsSelection: false, group: "Checks" },
  { id: "validate", label: "Validate governance", detail: "Check integrity, syntax, and markers", keywords: ["check", "verify", "integrity"], form: "none", needsSelection: false, group: "Checks" },
  { id: "test", label: "Test policies", detail: "Run every preserve and target rule", keywords: ["run", "check", "rules"], form: "none", needsSelection: false, group: "Checks" },
  { id: "hooks", label: "Install agent hooks", detail: "Make Crane check every tool call of Claude Code, Codex, Copilot CLI, or Gemini CLI", keywords: ["claude", "codex", "copilot", "gemini", "agent", "install", "enforce"], form: "hooks", needsSelection: false, group: "Agents" },
];

/** The action offered before Crane is set up. */
export const SETUP: Action = { id: "setup", label: "Set up Crane in this repository", detail: "Connect to GitHub and create or join its signed rules", keywords: ["init", "start", "connect", "join"], form: "none", needsSelection: false, group: "Setup" };

/**
 * Split text into lowercase words.
 * @param text text
 * @returns words
 */
export function words(text: string): string[] {
  return text.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
}

/**
 * Find the actions matching a query: every typed word must be the start of a word of the action's
 * label or keywords. An empty query returns every action. Results are ranked by whether the label
 * itself matches and by how early its first matching word is.
 * @param query what was typed
 * @param actions actions to search
 * @returns matching actions, best first
 */
export function search(query: string, actions: Action[]): Action[] {
  const typed = words(query);
  if (!typed.length) return actions;
  const scored = actions
    .map((action, order) => {
      const label = words(action.label);
      const all = label.concat(action.keywords.flatMap(words));
      if (!typed.every((word) => all.some((candidate) => candidate.startsWith(word)))) return undefined;
      const position = label.findIndex((candidate) => candidate.startsWith(typed[0]));
      return { action, rank: (position < 0 ? 100 : position) * 100 + order };
    })
    .filter((entry): entry is { action: Action; rank: number } => entry !== undefined);
  return scored.sort((left, right) => left.rank - right.rank).map((entry) => entry.action);
}

/**
 * Put actions under their menu headings. Headings appear in the order of their first action, so a
 * ranked search result keeps its best match first, and actions keep their order within a heading.
 * @param actions actions, already ordered (for example by search rank)
 * @returns headings with their actions, without empty headings
 */
export function groupActions(actions: Action[]): { group: Group; actions: Action[] }[] {
  const groups: { group: Group; actions: Action[] }[] = [];
  for (const action of actions) {
    const existing = groups.find((entry) => entry.group === action.group);
    if (existing) existing.actions.push(action);
    else groups.push({ group: action.group, actions: [action] });
  }
  return groups;
}
