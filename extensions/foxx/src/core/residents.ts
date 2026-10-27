// Resident agents as Landscape shows them. Crane is the authority: Landscape reads the envelope of
// 'crane resident list|show|context --format json', builds draft files for 'crane resident
// create --file', and shows what Crane decided. Nothing here validates, authorizes, or runs an
// assignment; it never reads keys, the trust directory, or the resident store. Editor-independent,
// so it is unit-tested.

/** An assignment as 'crane resident list' reports it (the same record the dashboard shows). */
export interface ResidentRow {
  assignment_id: string;
  definition: string;
  agent: string;
  target: string;
  rca?: string | null;
  lifecycle: "draft" | "validated" | "active" | "paused" | "expired" | "revoked";
  status: string;
  health?: { decision: string; reasons: string[]; generation: number } | null;
  subscriptions?: { source: string; kinds: string[] }[];
  timers?: { timer_id: string; every_minutes: number }[];
  expires_at?: number;
  latest_activation?: { id: string; state: string; at: number } | null;
  last_verified_at?: number | null;
  pending_approvals?: number;
  model_calls?: number;
}

/** A definition as 'crane resident list' reports it. */
export interface DefinitionRow {
  ref: string;
  definition_id: string;
  version: number;
  name: string;
  state: "draft" | "active" | "revoked";
}

/** The data of 'crane resident list --format json'. */
export interface ResidentList {
  assignments: ResidentRow[];
  definitions: DefinitionRow[];
  health?: { counters?: Record<string, number> };
}

/** A context pack section as 'crane resident context' reports it. */
export interface PackSection {
  kind: string;
  source: string;
  reference: string;
  digest: string;
  bytes: number;
}

/** The data of 'crane resident context ID --format json'. */
export interface PackData {
  pack: { key: string; sections: PackSection[]; missing: [string, string][]; dependencies: Record<string, unknown> };
  cache: { cache: string; changed?: string[] };
}

/** The statuses an assignment can show (lifecycle when not active, else derived). */
export const RESIDENT_STATUSES = ["DRAFT", "VALIDATED", "ACTIVE_IDLE", "QUEUED", "INVESTIGATING", "EXECUTING", "AWAITING_APPROVAL", "AWAITING_INPUT", "WAITING_FOR_EVENT", "RECONCILIATION_REQUIRED", "DEGRADED", "PAUSED", "EXPIRED", "REVOKED"] as const;

/**
 * The tone of a status: ok (idle and healthy), busy, attention (a person is needed), bad, or off.
 * @param status derived status
 * @returns tone
 */
export function residentTone(status: string): "ok" | "busy" | "attention" | "bad" | "off" {
  if (status === "ACTIVE_IDLE") return "ok";
  if (["QUEUED", "INVESTIGATING", "EXECUTING", "WAITING_FOR_EVENT", "VALIDATED"].includes(status)) return "busy";
  if (["AWAITING_APPROVAL", "AWAITING_INPUT"].includes(status)) return "attention";
  if (["DEGRADED", "RECONCILIATION_REQUIRED", "REVOKED"].includes(status)) return "bad";
  return "off";
}

/**
 * Group assignments by target, keeping the workspace's own Zones and Flows first.
 * @param rows assignments
 * @param targets target keys of the workspace ("flow:ID", "zone:ID")
 * @returns [target, rows, in this workspace] in order
 */
export function residentsByTarget(rows: ResidentRow[], targets: string[]): [string, ResidentRow[], boolean][] {
  const keys = [...new Set(rows.map((row) => row.target))];
  const local = new Set(targets);
  keys.sort((a, b) => Number(local.has(b)) - Number(local.has(a)) || a.localeCompare(b));
  return keys.map((key) => [key, rows.filter((row) => row.target === key).sort((a, b) => a.assignment_id.localeCompare(b.assignment_id)), local.has(key)]);
}

/**
 * Build the assignment draft 'crane resident create --file' imports. It names its subscriptions
 * explicitly (never an "any event" pattern) and stays a draft until validated and activated.
 * @param input the person's choices
 * @returns the draft
 */
export function assignmentDraft(input: { id: string; definition: string; agent: string; target: string; kinds: string; source?: string; rca?: string; lifetimeDays?: number }): Record<string, unknown> {
  const [definitionId, version] = input.definition.split("@");
  const kinds = input.kinds.split(",").map((kind) => kind.trim()).filter((kind) => kind && kind !== "*");
  const rca = input.rca ? input.rca.split("@") : undefined;
  return {
    assignment_id: input.id,
    definition_id: definitionId,
    definition_version: Number(version || 1),
    agent: input.agent,
    target: input.target,
    rca: rca ? { rca_id: rca[0], version: Number(rca[1] || 1) } : null,
    // CI runs pulled from GitHub Actions name no target, so CI subscriptions take the repository scope
    subscriptions: kinds.length ? [{ source: input.source ?? "ci", kinds, attributes: {}, scope: (input.source ?? "ci") === "ci" ? "repository" : "target" }] : [],
    timers: [],
    lifetime_days: Math.min(90, Math.max(1, input.lifetimeDays ?? 14)),
  };
}

/** The lifecycle changes Landscape offers, what each needs, and what it means. */
export const RESIDENT_CHANGES = {
  pause: { from: ["active"], label: "Pause", detail: "It takes no trigger until it is resumed; queued work waits." },
  resume: { from: ["paused"], label: "Resume", detail: "It takes triggers again." },
  revoke: { from: ["draft", "validated", "active", "paused", "expired"], label: "Revoke", detail: "Final: it never acts again. Queued and waiting work is cancelled; an external call already started stays uncertain until reconciled." },
} as const;

/**
 * The lifecycle changes an assignment can take now.
 * @param row assignment
 * @returns change names
 */
export function residentChanges(row: ResidentRow): (keyof typeof RESIDENT_CHANGES)[] {
  return (Object.keys(RESIDENT_CHANGES) as (keyof typeof RESIDENT_CHANGES)[]).filter((name) => (RESIDENT_CHANGES[name].from as readonly string[]).includes(row.lifecycle));
}

/**
 * Check an assignment id the way Crane does (letters, digits, '_' or '-', starting with a letter).
 * @param id id
 * @returns a message, or undefined when valid
 */
export function checkAssignmentId(id: string): string | undefined {
  return /^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(id) ? undefined : "Use letters, digits, '_' or '-', starting with a letter (at most 64)";
}

/**
 * Describe an assignment for a tooltip or a preview (Markdown).
 * @param row assignment
 * @returns markdown
 */
export function residentMarkdown(row: ResidentRow): string {
  const triggers = (row.subscriptions ?? []).map((item) => `\`${item.source}\` ${item.kinds.map((kind) => `\`${kind}\``).join(", ")}`).concat((row.timers ?? []).map((timer) => `timer every ${timer.every_minutes} min`));
  const lines = [
    `**${row.assignment_id}** · ${row.status} (lifecycle ${row.lifecycle})`,
    "",
    `- Target: \`${row.target}\` · agent \`${row.agent}\` · definition \`${row.definition}\`${row.rca ? ` · RCA \`${row.rca}\`` : ""}`,
    `- Triggers: ${triggers.length ? triggers.join("; ") : "_none_"}`,
    `- Crane now: ${row.health ? `${row.health.decision}${row.health.reasons.length ? ` (${row.health.reasons.join(", ")})` : ""} at signed generation ${row.health.generation}` : "_unknown_"}`,
    `- Latest activation: ${row.latest_activation ? `\`${row.latest_activation.id}\` ${row.latest_activation.state}` : "_none: idle until a trigger arrives_"}`,
    `- Pending approvals: ${row.pending_approvals ?? 0} · model calls so far: ${row.model_calls ?? 0} (none while idle)`,
    "",
    "_The full activation timeline is on the DataSea dashboard under Resident agents. Crane decides every step against the signed rules; this view only shows what it reported._",
  ];
  return lines.join("\n");
}

/**
 * Describe a context pack (Markdown): the sources and versions an activation receives.
 * @param id assignment
 * @param data pack data
 * @returns markdown
 */
export function packMarkdown(id: string, data: PackData): string {
  const cache = data.cache.cache + (data.cache.changed && data.cache.changed.length ? ` (changed: ${data.cache.changed.join(", ")})` : "");
  const rows = data.pack.sections.map((section) => `| ${section.kind} | \`${section.source}\` | ${section.reference} | ${section.bytes} |`);
  const missing = data.pack.missing.map(([code, message]) => `- **${code}**: ${message}`);
  return [
    `# Context for ${id}`,
    "",
    `Pack \`${data.pack.key}\` · cache ${cache}`,
    "",
    "| Kind | Source | Version | Bytes |",
    "|---|---|---|---|",
    ...rows,
    "",
    missing.length ? "## Missing required context\n\n" + missing.join("\n") : "Every required item was loaded.",
    "",
    "_Context is guidance: Crane decides every step against the signed state when it runs, whatever the context says._",
  ].join("\n");
}
