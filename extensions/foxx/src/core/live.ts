// The live agent context Crane publishes while an agent works ('crane foxx live'): which selection,
// zone, or flow with context the agent's latest tool call touched, the context itself, and where
// in the code it applies. Editor-independent, so it is unit-tested.

/** How long a published context stays on screen without a newer one (two minutes). */
export const LIVE_FOR_MS = 120_000;

/** One context the agent received. */
export interface AgentContext {
  entity: string;
  kind: "selection" | "zone" | "flow" | "brick";
  id: string;
  name?: string | null;
  text: string;
  locations: { path: string; start_line?: number; end_line?: number }[];
}

/** What Crane published for the agent's latest tool call. */
export interface LiveContext {
  at: number;
  agent: string;
  session?: string;
  tool: string;
  operation?: string;
  decision?: string;
  contexts: AgentContext[];
}

/**
 * Whether a published context is recent enough to show.
 * @param live published context
 * @param now current time in milliseconds
 * @returns whether it is fresh
 */
export function liveFresh(live: LiveContext | undefined, now: number): boolean {
  return Boolean(live && Array.isArray(live.contexts) && live.contexts.length && now - live.at <= LIVE_FOR_MS && live.at <= now + 5_000);
}

/**
 * The guidance a context adds, without Crane's heading lines: the lines that say what to do.
 * @param text context text as Crane wrote it
 * @returns guidance lines
 */
export function guidanceLines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((line) => line.trim().replace(/^-\s*/, ""))
    .filter((line) => line && !/^Crane (guidance|selection guidance)/.test(line));
}

/**
 * The one-line label shown after the code the agent is in: who, where, and the first guidance.
 * @param context the context
 * @param agent the agent's name
 * @returns label text (at most about 140 characters)
 */
export function contextLabel(context: AgentContext, agent?: string): string {
  const lines = guidanceLines(context.text);
  // The first line names the zone or selection; the next is the first instruction when there is one
  const first = lines.find((line) => !/^(Zone|Flow|Selection) /.test(line)) ?? lines[0] ?? "";
  const label = `◆ ${agent ? `${agent} · ` : ""}${context.kind} ${context.name ?? context.id}${first ? ` — ${first}` : ""}`;
  return label.length > 140 ? `${label.slice(0, 139)}…` : label;
}
