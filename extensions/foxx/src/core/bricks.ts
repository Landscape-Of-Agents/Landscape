// Bricks and Crane's command families (controls) as Landscape uses them. Crane is the authority:
// Landscape builds arguments, reads the shared JSON envelope every family command prints
// ('--format json'), and shows what Crane decided. Nothing here validates a Brick or decides a
// control; it never reads keys, the trust directory, or secrets. Editor-independent, so it is
// unit-tested.

/** The outcome classes of the shared envelope (cli/protocol.rs). */
export type EnvelopeStatus = "ok" | "passed" | "failed" | "denied" | "blocked" | "approval_required" | "unknown" | "unresolved";

/** The result every family command prints with '--format json'. */
export interface Envelope<T = Record<string, unknown>> {
  schema_version: number;
  command: string;
  request_id: string;
  status: EnvelopeStatus;
  decision?: string;
  reason_codes: string[];
  summary: string;
  refs: Record<string, string>;
  data: T;
  created_at: number;
}

/**
 * Parse the envelope a family command printed.
 * @param stdout command output
 * @returns the envelope, or undefined when it is not one (an older Crane, or a usage error)
 */
export function parseEnvelope<T = Record<string, unknown>>(stdout: string): Envelope<T> | undefined {
  try {
    const value = JSON.parse(stdout) as Envelope<T>;
    return value && typeof value.status === "string" && typeof value.command === "string" ? value : undefined;
  } catch {
    return undefined;
  }
}

/**
 * Whether an envelope status means success (exit code 0).
 * @param status status
 * @returns whether it succeeded
 */
export function succeeded(status: EnvelopeStatus): boolean {
  return status === "ok" || status === "passed";
}

/** The fixed Brick types, as 'crane brick types' lists them. */
export const BRICK_TYPES = ["context", "identity", "scope", "permission", "constraint", "precondition", "approval", "isolation", "budget", "action", "verification", "provenance", "recovery", "revocation", "monitoring", "intelligence"] as const;

/** A fixed Brick type. */
export type BrickType = (typeof BRICK_TYPES)[number];

/** One type of the catalog. */
export interface TypeInfo {
  type: BrickType;
  label: string;
  purpose: string;
  support: "enforced" | "checked" | "delivered" | "declared";
  runtime: string;
  fields: string[];
}

/** Starting configurations offered when a person creates a Brick; Crane validates whatever is kept. */
export const BRICK_TEMPLATES: Record<BrickType, Record<string, unknown>> = {
  context: { files: [] },
  identity: { agents: ["claude"] },
  scope: { include: ["src/**"] },
  permission: { allow: ["read", "write"] },
  constraint: { max_ceiling: "assisted", protected_paths: [] },
  precondition: { checks: [{ kind: "file_exists", path: "README.md" }] },
  approval: { operations: ["write", "delete"], reason: "" },
  isolation: { profile: "agent-hooks" },
  budget: { max_tool_calls: 200, on_exhausted: "escalate" },
  action: { adapter: "git", operation: "git.commit" },
  verification: { checks: [{ kind: "policy_tests_pass" }] },
  provenance: { require: ["signed_state", "audit_chain"] },
  recovery: { on_failure: "escalate", max_retries: 0, reconcile_before_retry: true },
  revocation: { triggers: ["verification_contradicted"] },
  monitoring: { signals: [{ source: "policy_tests", every_minutes: 60 }] },
  intelligence: { analyses: ["denial_patterns"], advisory_only: true },
};

/** A Brick as 'crane brick list' reports it (drafts and activated versions). */
export interface BrickRow {
  ref: string;
  brick_id: string;
  version: number;
  type: BrickType;
  name: string;
  intent?: string;
  state: "draft" | "active" | "deprecated" | "revoked";
  validation?: string;
  digest?: string;
}

/** A Stack as 'crane brick list' reports it. */
export interface StackRow {
  ref: string;
  stack_id: string;
  version: number;
  name: string;
  state: string;
  members: string[];
}

/** An attachment as 'crane brick list' reports it. */
export interface AttachmentRow {
  attachment_id: string;
  target: string;
  subject: string;
  state: "enabled" | "disabled";
  current_context: string;
  issues: string[];
}

/** The data of 'crane brick list --format json'. */
export interface BrickList {
  bricks: BrickRow[];
  stacks: StackRow[];
  attachments: AttachmentRow[];
  assignments: { agent: string; targets: { kind: string; id: string }[]; state: string }[];
  store: { trust: string; digest: string; generation: number };
  unreadable_drafts: string[];
}

/** The draft a person fills in, written as the file 'crane brick create --file' reads. */
export interface DraftInput {
  type: BrickType;
  name: string;
  intent: string;
  context: string;
  config: Record<string, unknown>;
  owner?: string;
  departments?: string[];
}

/**
 * Build the JSON of a draft file.
 * @param input what the person entered
 * @returns file content
 */
export function draftFile(input: DraftInput): string {
  return JSON.stringify(
    {
      type: input.type,
      name: input.name.trim(),
      intent: input.intent.trim(),
      owner: input.owner?.trim() || null,
      departments: input.departments ?? [],
      config: input.config,
      common_context: { text: input.context, sensitivity: "internal" },
    },
    null,
    2,
  );
}

/**
 * Make an exact subject reference for attachments: "ID@N" for a Brick, "stack:ID@N" for a Stack.
 * @param row Brick or Stack row
 * @returns reference, or undefined for a draft (drafts cannot be attached)
 */
export function subjectOf(row: BrickRow | StackRow): string | undefined {
  if (row.state === "draft") return undefined;
  return "stack_id" in row ? `stack:${row.stack_id}@${row.version}` : `${row.brick_id}@${row.version}`;
}

/**
 * Build the arguments of 'crane brick attach'.
 * @param subject "ID@N" or "stack:ID@N"
 * @param target "flow:ID" or "zone:ID"
 * @param contextFile file holding the attachment's Current Context, when there is one
 * @param dryRun preview only
 * @returns arguments
 */
export function attachArgs(subject: string, target: string, contextFile: string | undefined, dryRun: boolean): string[] {
  const args = ["brick", "attach", subject, "--target", target];
  if (contextFile) args.push("--context-file", contextFile);
  if (dryRun) args.push("--dry-run");
  return args.concat(["--format", "json"]);
}

/** The preview a signing brick command returns with '--dry-run' (and after applying). */
export interface ChangePreview {
  target: string;
  files_in_target?: number;
  widens?: boolean;
  note?: string;
  before?: { files: { path: string; ceiling: string | null; reasons: string[] }[]; restricts: boolean };
  after?: { files: { path: string; ceiling: string | null; reasons: string[] }[]; restricts: boolean; scope_resolved?: boolean };
  snapshot?: { digest: string; bytes: number; sections: { kind: string; source: string; reference: string; digest: string; bytes: number }[]; conflicts: { code: string; message: string }[]; gaps: { code: string; message: string }[] };
}

/**
 * Describe a change preview for a confirmation: what the guard will restrict before and after, and
 * what agents will receive.
 * @param preview preview from Crane
 * @returns lines
 */
export function describePreview(preview: ChangePreview | undefined): string[] {
  if (!preview) return [];
  if (preview.note) return [preview.note];
  const before = preview.before?.files.length ?? 0;
  const after = preview.after?.files.length ?? 0;
  const lines = [`${preview.target}: ${after} of ${preview.files_in_target ?? "?"} file(s) restricted by Bricks after this change (${before} before).`];
  if (preview.widens) lines.push("This loosens what agents may do in the target.");
  if (preview.after && preview.after.scope_resolved === false) lines.push("A scope matches no file of the target, so the whole target becomes read-only (fail closed).");
  const changed = preview.after?.files.filter((file) => !preview.before?.files.some((old) => old.path === file.path && old.ceiling === file.ceiling)) ?? [];
  for (const file of changed.slice(0, 8)) lines.push(`  ${file.path} → ${file.ceiling ?? "unchanged"}: ${file.reasons.join("; ")}`);
  if (changed.length > 8) lines.push(`  … and ${changed.length - 8} more`);
  if (preview.snapshot) {
    lines.push(`Agents in the target receive snapshot ${preview.snapshot.digest.slice(0, 12)} (${preview.snapshot.bytes} bytes, ${preview.snapshot.sections.length} section(s)).`);
    for (const conflict of preview.snapshot.conflicts) lines.push(`Conflict ${conflict.code}: ${conflict.message}`);
    for (const gap of preview.snapshot.gaps) lines.push(`Gap ${gap.code}: ${gap.message}`);
  }
  return lines;
}

/** The snapshot 'crane brick context preview' returns. */
export interface SnapshotData {
  snapshot: {
    targets: string[];
    digest: string;
    generation: number;
    bricks: string[];
    sections: { kind: string; source: string; reference: string; digest: string; bytes: number; text: string }[];
    conflicts: { code: string; message: string }[];
    gaps: { code: string; message: string }[];
  };
  rendered: string;
  truncated: boolean;
  notes: string[];
}

/**
 * Render a context preview as Markdown: sources with digests first, then the exact text agents get.
 * @param data preview data
 * @returns Markdown
 */
export function snapshotMarkdown(data: SnapshotData): string {
  const snapshot = data.snapshot;
  const rows = snapshot.sections.map((section) => `| ${section.kind} | \`${section.source}\` | ${section.reference} | \`${section.digest.slice(0, 12)}\` | ${section.bytes} |`);
  return [
    `# Effective context for ${snapshot.targets.join(", ")}`,
    "",
    `Snapshot \`${snapshot.digest.slice(0, 16)}\` · signed generation ${snapshot.generation} · ${snapshot.bricks.length} Brick(s). Context is guidance only; Crane enforces restrictions at every action.`,
    "",
    "| Kind | Source | Reference | Digest | Bytes |",
    "|---|---|---|---|---|",
    ...rows,
    "",
    ...snapshot.conflicts.map((item) => `- **Conflict ${item.code}**: ${item.message}`),
    ...snapshot.gaps.map((item) => `- Gap ${item.code}: ${item.message}`),
    ...data.notes.map((note) => `- Note: ${note}`),
    "",
    "## What agents receive",
    "",
    "```text",
    data.rendered.trimEnd(),
    "```",
    data.truncated ? "\n_The delivered text is truncated at the delivery limit._" : "",
  ].join("\n");
}

/** How a control asks for a value. */
export interface ControlInput {
  /** The option, such as "--skill"; empty for a positional value. */
  flag: string;
  prompt: string;
  placeholder?: string;
  optional?: boolean;
}

/** One command of Crane's command families that Landscape can run. */
export interface Control {
  id: string;
  family: string;
  label: string;
  detail: string;
  words: string[];
  inputs: ControlInput[];
  /** It changes state (approvals, budgets, runs, revocations): confirm first. */
  changes: boolean;
  /** Arguments always passed after the entered values. */
  fixed?: string[];
}

/** Ask for a skill reference. */
const SKILL: ControlInput = { flag: "--skill", prompt: "Skill (ID or ID@N)", placeholder: "look-up-a-commit" };
/** Ask for an invocation or run id. */
const RUN: ControlInput = { flag: "--run", prompt: "Invocation or run id" };
/** Ask for an approval reference. */
const APPROVAL: ControlInput = { flag: "", prompt: "Approval reference (INVOCATION_ID@PLAN_DIGEST or RUN_ID:STEP@DIGEST)" };
/** Ask for a reason. */
const REASON: ControlInput = { flag: "--reason", prompt: "Reason (recorded in the audit)" };
/** Ask for a selection id. */
const SELECTION: ControlInput = { flag: "", prompt: "Selection id (8 characters)" };

/**
 * Build a control entry.
 * @param family command family
 * @param command command words after the family
 * @param detail what it does
 * @param inputs values it asks for
 * @param changes whether it changes state
 * @returns control
 */
function control(family: string, command: string, detail: string, inputs: ControlInput[] = [], changes = false): Control {
  return { id: `${family}.${command.replace(/ /g, ".")}`, family, label: `${family} ${command}`, detail, words: command.split(/[^a-z]+/), inputs, changes };
}

/** The commands of the families (scope, permission, precondition, constraint, approval, isolation,
 * verification, provenance, budget, recovery, revocation, selection, skill, runbook, brick) that
 * Landscape offers, with the arguments each requires ('crane FAMILY' prints the same forms).
 * Commands that change state ask for confirmation; Crane still decides and refuses agents. */
export const CONTROLS: Control[] = [
  control("scope", "inspect", "The effective boundary, its enforcement, and what is unresolved"),
  control("scope", "diff", "The boundary against the last signed baseline; fails when it widened"),
  control("scope", "test", "Positive and adversarial probes of the boundary"),
  control("scope", "audit", "The boundary against recorded attempts"),
  control("permission", "inspect", "What a skill may do, by resource namespace", [SKILL]),
  control("permission", "check", "Decide one operation on one resource", [{ flag: "--operation", prompt: "Operation (ADAPTER/OPERATION)", placeholder: "git/git.commit" }, { flag: "--resource", prompt: "Resource (repo:PATH or NAMESPACE:ID)" }]),
  control("permission", "test", "A skill's permission tests", [SKILL]),
  control("precondition", "inspect", "A skill's preconditions and how each is checked", [SKILL]),
  control("precondition", "test", "A skill's precondition tests", [SKILL]),
  control("constraint", "inspect", "A skill's limits and applicability", [SKILL]),
  control("constraint", "test", "A skill's constraint tests", [SKILL]),
  control("approval", "status", "Where an approval stands", [{ flag: "", prompt: "Approval reference or invocation/run id" }]),
  control("approval", "approve", "Approve an exact pending operation (a person other than the requester)", [APPROVAL, { flag: "--comment", prompt: "Comment (optional)", optional: true }], true),
  control("approval", "deny", "Deny a pending operation", [APPROVAL, REASON], true),
  control("approval", "test", "The approval rule tests (binding, self-approval)"),
  control("isolation", "profiles", "What each execution path really confines; none is a sandbox"),
  control("isolation", "test", "Test an isolation profile", [{ flag: "--profile", prompt: "Profile", placeholder: "agent-hooks" }]),
  control("verification", "evaluate", "Judge a skill's claims from their evidence now", [SKILL]),
  control("verification", "report", "The verification result of an invocation", [{ flag: "--run", prompt: "Invocation id" }]),
  control("verification", "test", "A skill's verification tests", [SKILL]),
  control("provenance", "verify", "Verify the lineage of an invocation or run", [RUN]),
  control("provenance", "test", "The provenance rule tests (chain tampering)"),
  control("budget", "inspect", "A skill's budget, or a run's reservations", [SKILL]),
  control("budget", "test", "The budget rule tests (exact limit, exceeded, retries)"),
  control("recovery", "inspect", "An uncertain or failed operation and how to reconcile it", [RUN]),
  control("recovery", "reconcile", "Reconcile an uncertain operation before any retry", [RUN], true),
  control("recovery", "test", "The recovery rule tests (reconcile before retry)"),
  control("revocation", "inspect", "Whether a skill is revoked and where it still applies", [SKILL]),
  control("revocation", "test", "The revocation rule tests"),
  control("selection", "inspect", "One preserved or targeted selection and its state", [SELECTION]),
  control("selection", "validate", "Selection markers and records", [{ flag: "", prompt: "Selection id (empty for all)", optional: true }]),
  control("selection", "repair", "Candidates for an unresolved selection (applying one is re-protecting)", [SELECTION]),
  control("skill", "inspect", "A skill's definition, state, and evidence", [{ flag: "", prompt: "Skill (ID or ID@N)" }]),
  { ...control("skill", "validate", "Validate a skill without recording it (dry run)", [{ flag: "", prompt: "Skill (ID or ID@N)" }]), fixed: ["--dry-run"] },
  control("runbook", "inspect", "A runbook's steps, approvals, and budget", [{ flag: "", prompt: "Runbook (ID or ID@N)" }]),
  control("runbook", "status", "Where a runbook run stands", [{ flag: "", prompt: "Run id" }]),
  control("runbook", "cancel", "Cancel a runbook run (never claims a rollback)", [{ flag: "", prompt: "Run id" }, REASON], true),
  control("brick", "agents", "Assigned agents: assignment and runtime status"),
  control("brick", "monitor run", "Run the due Brick monitors now (deterministic, no model)"),
  control("brick", "monitor status", "Brick monitors, their last result, and coverage gaps"),
  control("brick", "context status", "Context snapshots delivered and reused per session"),
  control("brick", "history", "Signed Brick changes"),
];

/**
 * Build the arguments of a control from the values the person entered.
 * @param item control
 * @param values one value per input, in order ("" for a skipped optional one)
 * @returns arguments, ending with '--format json'
 */
export function controlArgs(item: Control, values: string[]): string[] {
  const args = [item.family, ...item.label.split(" ").slice(1)];
  item.inputs.forEach((input, index) => {
    const value = (values[index] ?? "").trim();
    if (!value) return;
    if (input.flag) args.push(input.flag, value);
    else args.push(value);
  });
  return args.concat(item.fixed ?? [], ["--format", "json"]);
}

/**
 * Describe an envelope in one line for a notification.
 * @param envelope envelope
 * @returns text
 */
export function envelopeLine(envelope: Envelope<unknown>): string {
  const reasons = envelope.reason_codes.length ? ` (${envelope.reason_codes.slice(0, 4).join(", ")})` : "";
  return `${envelope.status.replace("_", " ").toUpperCase()}${envelope.decision ? " " + envelope.decision : ""}: ${envelope.summary}${reasons}`;
}
