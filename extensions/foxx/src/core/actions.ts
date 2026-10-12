// The actions the Foxx panel offers and how the search box finds them. Typing filters actions whose
// words start with what was typed (prefix search, every typed word must match), ranked by how early
// the match is. Editor-independent, so it is unit-tested.

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

/** One action of the panel. */
export interface Action {
  id: string;
  label: string;
  detail: string;
  keywords: string[];
  form: Form;
  needsSelection: boolean;
}

/** Every action, in the order shown when the search box is empty. */
export const ACTIONS: Action[] = [
  { id: "preserve", label: "Preserve these lines", detail: "The agent must not change them (crane protect)", keywords: ["protect", "keep", "lock", "freeze"], form: "preserve", needsSelection: true },
  { id: "target", label: "Target these lines", detail: "The agent must change them (crane target)", keywords: ["change", "modify", "fix", "rewrite"], form: "target", needsSelection: true },
  { id: "zone", label: "Make these lines a zone", detail: "A boundary with its own autonomy ceiling, criticality, and context", keywords: ["boundary", "autonomy", "ceiling", "criticality", "area"], form: "zone", needsSelection: true },
  { id: "flow", label: "Make this function a flow", detail: "The function and everything it calls, with its own autonomy and context", keywords: ["entry", "path", "calls", "autonomy", "execution"], form: "flow", needsSelection: true },
  { id: "context", label: "Add context to a zone or flow", detail: "Guidance the agent receives whenever it touches that code", keywords: ["guidance", "note", "instruction", "explain", "describe", "zone", "flow"], form: "context", needsSelection: false },
  { id: "zone_attach", label: "Add these lines to an existing zone", detail: "Extend a zone to cover this function or file", keywords: ["zone", "extend", "include", "cover"], form: "zone_attach", needsSelection: true },
  { id: "policy_context", label: "Attach this file as policy context", detail: "The whole file becomes guidance for a policy", keywords: ["context", "guidance", "document", "file", "policy"], form: "policy_context", needsSelection: false },
  { id: "policy", label: "Create a policy", detail: "A named group of preserve and target rules", keywords: ["new", "rules", "group"], form: "policy", needsSelection: false },
  { id: "move", label: "Move a selection to another policy", detail: "Reassign a preserved or targeted block", keywords: ["reassign", "policy", "selection"], form: "move", needsSelection: false },
  { id: "checkpoint", label: "Create a checkpoint", detail: "Record the current commit as trusted", keywords: ["baseline", "snapshot", "commit", "trust"], form: "checkpoint", needsSelection: false },
  { id: "validate", label: "Validate governance", detail: "Check integrity, syntax, and markers", keywords: ["check", "verify", "integrity"], form: "none", needsSelection: false },
  { id: "test", label: "Test policies", detail: "Run every preserve and target rule", keywords: ["run", "check", "rules"], form: "none", needsSelection: false },
  { id: "status", label: "Policy status", detail: "A policy's rules and an enforcement self-test", keywords: ["report", "rules"], form: "none", needsSelection: false },
  { id: "hooks", label: "Install agent hooks", detail: "Make Crane check every tool call of Claude Code or Codex", keywords: ["claude", "codex", "agent", "install", "enforce"], form: "hooks", needsSelection: false },
];

/** The action offered before Crane is set up. */
export const SETUP: Action = { id: "setup", label: "Set up Crane in this repository", detail: "Connect to GitHub and create or join its signed rules", keywords: ["init", "start", "connect", "join"], form: "none", needsSelection: false };

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
