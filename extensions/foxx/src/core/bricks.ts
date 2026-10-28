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

/** One configurable setting of a Brick kind, explained. */
export interface GuideSetting {
  name: string;
  meaning: string;
  values: string;
}

/** A Brick kind explained for people choosing one: what it is, when to use it, a worked example,
 * and its settings. The values follow Crane's schema (src/brick/config.rs); Crane validates. */
export interface BrickGuide {
  what: string;
  useWhen: string;
  example: { story: string; name: string; intent: string; config: Record<string, unknown> };
  settings: GuideSetting[];
}

/** Action classes, as several kinds name them. */
const CLASSES = "read, write, delete, execute, network";
/** Deterministic checks, as preconditions and verifications name them. */
const CHECKS = 'list of {"kind": "file_exists" | "file_absent", "path": "…"}, {"kind": "selections_resolved"}, {"kind": "policy_tests_pass"}, {"kind": "signed_state"}';

/** The guide of every Brick kind. */
export const BRICK_GUIDE: Record<BrickType, BrickGuide> = {
  context: {
    what: "Reviewed knowledge agents read before they act: domain facts, procedures, terms. It guides; it never grants anything.",
    useWhen: "Agents keep making the same mistakes in an area because they lack background they cannot see in the code.",
    example: { story: "The payments Zone has rules about rounding and currencies. Attach a Context Brick listing docs/payments.md so every agent working there reads it first.", name: "Payments domain notes", intent: "Agents should know our rounding and currency rules before editing payments", config: { files: ["docs/payments.md"] } },
    settings: [{ name: "files", meaning: "Repository files delivered with the common context.", values: "list of repository-relative paths" }],
  },
  identity: {
    what: "Which agents may work in the Zone or Flow, by provider or exact agent id.",
    useWhen: "Only some agents should touch an area, e.g. only Claude on infrastructure code.",
    example: { story: "Only Claude agents should work on the deployment Flow; Codex is used elsewhere.", name: "Claude only on deployment", intent: "Keep deployment work to one reviewed agent", config: { agents: ["claude"] } },
    settings: [{ name: "agents", meaning: "Agents admitted to the target.", values: "list of providers (claude, codex, copilot, gemini) or exact agent ids" }],
  },
  scope: {
    what: "The files inside the Zone or Flow agents may change. Everything else there becomes read-only.",
    useWhen: "A Zone is broad, but agents should only edit part of it (say, source but not generated files).",
    example: { story: "The API Zone includes generated clients. Agents may change src/api/** but never the generated code.", name: "API source only", intent: "Agents edit handwritten API code, never generated clients", config: { include: ["src/api/**"] } },
    settings: [{ name: "include", meaning: "Glob patterns of the files agents may change. If none matches, the whole target becomes read-only.", values: "list of repository-relative globs" }],
  },
  permission: {
    what: "The kinds of actions allowed in the target: read, write, delete, run commands, use the network.",
    useWhen: "An area should be readable by agents but not editable, or should allow edits but no deletions.",
    example: { story: "Agents may read and edit the docs Flow, but never delete files or run commands there.", name: "Docs: read and write only", intent: "No deletions or commands while agents edit docs", config: { allow: ["read", "write"] } },
    settings: [{ name: "allow", meaning: "Allowed action classes. Without write or delete, the files become read-only.", values: CLASSES }],
  },
  constraint: {
    what: "Hard limits no context or instruction can relax: how autonomous agents may be, how critical the area is, and paths that must never change.",
    useWhen: "Some files must never be changed by an agent, or an area must always keep a person in the loop.",
    example: { story: "In the billing Zone agents may only propose changes a person accepts, and the migrations folder is untouchable.", name: "Billing hard limits", intent: "A person accepts every billing change; migrations never change", config: { max_ceiling: "assisted", min_criticality: "critical", protected_paths: ["db/migrations/**"] } },
    settings: [
      { name: "max_ceiling", meaning: "The highest autonomy agents get.", values: "observe, assisted, delegated, autonomous (strictest first)" },
      { name: "min_criticality", meaning: "The lowest criticality the target is treated with.", values: "routine, sensitive, critical, restricted" },
      { name: "protected_paths", meaning: "Glob patterns agents may never change.", values: "list of repository-relative globs" },
    ],
  },
  precondition: {
    what: "Facts that must hold before agents may change the target. While one fails, every change needs a person's approval.",
    useWhen: "Work is only safe once something is in place, e.g. a feature flag file exists or a lock file is absent.",
    example: { story: "Agents may change the release Flow only while no release.lock file exists.", name: "No release in progress", intent: "Pause agent changes while a release is running", config: { checks: [{ kind: "file_absent", path: "release.lock" }] } },
    settings: [{ name: "checks", meaning: "Checks that must all pass.", values: CHECKS }],
  },
  approval: {
    what: "When a person must approve agent changes in the target.",
    useWhen: "Agents may work in an area, but a person should confirm edits or deletions there.",
    example: { story: "Every edit or deletion in the auth Zone is approved by a person, with the reason shown to them.", name: "Approve auth changes", intent: "A person confirms every change to authentication code", config: { operations: ["write", "delete"], reason: "Authentication changes need a second pair of eyes" } },
    settings: [
      { name: "operations", meaning: "Action classes that need approval.", values: CLASSES },
      { name: "reason", meaning: "Shown to the person who approves.", values: "text" },
    ],
  },
  isolation: {
    what: "The execution boundary agent work in the target requires.",
    useWhen: "Work must run under a specific confinement profile; if it is unavailable, direct changes stop (fail closed).",
    example: { story: "Agent work in the infra Zone must go through the agent-hooks profile, so every tool call is checked.", name: "Infra through hooks", intent: "Every agent action in infra is checked by Crane's hooks", config: { profile: "agent-hooks" } },
    settings: [{ name: "profile", meaning: "The isolation profile id. 'crane isolation profiles' lists them.", values: "a profile id, e.g. agent-hooks" }],
  },
  budget: {
    what: "Caps on tool calls, retries, and running time, and what happens at the cap.",
    useWhen: "Agents in an area should not run away with long sessions or endless retries.",
    example: { story: "In the experiments Zone a session gets at most 200 tool calls and 30 minutes, then a person is asked.", name: "Experiments budget", intent: "Keep experiment sessions short and ask before going on", config: { max_tool_calls: 200, max_retries: 1, max_runtime_minutes: 30, on_exhausted: "escalate" } },
    settings: [
      { name: "max_tool_calls", meaning: "Tool calls per session in the target.", values: "whole number (optional)" },
      { name: "max_retries", meaning: "Retries of a failed step.", values: "whole number (optional)" },
      { name: "max_runtime_minutes", meaning: "Running time per task.", values: "whole number (optional)" },
      { name: "on_exhausted", meaning: "What happens at the cap.", values: "stop, escalate (ask a person)" },
    ],
  },
  action: {
    what: "Binds work to one registered, typed operation (for example a git commit). The Brick runs nothing itself.",
    useWhen: "Work in an area should go through a known operation instead of free-form commands.",
    example: { story: "The changelog Flow ends with a git commit made through the registered git adapter.", name: "Commit through git adapter", intent: "Changelog updates are committed through a known operation", config: { adapter: "git", operation: "git.commit" } },
    settings: [
      { name: "adapter", meaning: "A registered adapter id.", values: "e.g. git" },
      { name: "operation", meaning: "An operation of that adapter.", values: "e.g. git.commit" },
    ],
  },
  verification: {
    what: "Objective checks that prove an outcome, with the evidence for each: proven, contradicted, unknown, or not run.",
    useWhen: "You want proof the area is still healthy after agents work, such as all policy tests passing.",
    example: { story: "After agents change the checkout Flow, every policy test and selection must still pass.", name: "Checkout stays green", intent: "Prove checkout still meets its rules after agent work", config: { checks: [{ kind: "policy_tests_pass" }, { kind: "selections_resolved" }] } },
    settings: [{ name: "checks", meaning: "Checks that prove the outcome.", values: CHECKS }],
  },
  provenance: {
    what: "The records every outcome must be traceable to: the signed state and the audit chain.",
    useWhen: "Auditors need to trace each change in an area back to signed rules and recorded decisions.",
    example: { story: "Changes in the compliance Zone must trace back to the signed manifest and the audit chain.", name: "Traceable compliance work", intent: "Every compliance change is traceable for audits", config: { require: ["signed_state", "audit_chain"] } },
    settings: [{ name: "require", meaning: "Records an outcome must be traceable to.", values: "signed_state, audit_chain" }],
  },
  recovery: {
    what: "What happens after a failed, partial, or uncertain outcome. Uncertain effects are always reconciled before a retry.",
    useWhen: "Failures in an area must not be retried blindly, for example steps that touch external systems.",
    example: { story: "If a step in the deploy Flow fails, retry once after checking what really happened, then ask a person.", name: "Careful deploy retries", intent: "Never retry a deploy step blindly", config: { on_failure: "escalate", max_retries: 1, reconcile_before_retry: true } },
    settings: [
      { name: "on_failure", meaning: "What happens after a failure.", values: "stop, escalate (ask a person)" },
      { name: "max_retries", meaning: "Retries allowed.", values: "0 to 3" },
      { name: "reconcile_before_retry", meaning: "Check what really happened before retrying.", values: "must be true" },
    ],
  },
  revocation: {
    what: "Conditions under which attachments in the target should be withdrawn. Monitors report it; an admin withdraws.",
    useWhen: "You want to know at once when an area's protections can no longer be trusted.",
    example: { story: "If the signed state stops verifying or verification is contradicted, the monitors flag the attachments for withdrawal.", name: "Withdraw on broken trust", intent: "Flag attachments when the area's guarantees break", config: { triggers: ["integrity_failure", "verification_contradicted"] } },
    settings: [{ name: "triggers", meaning: "Conditions the monitors report.", values: "integrity_failure, verification_contradicted, repeated_denials" }],
  },
  monitoring: {
    what: "Deterministic checks that keep running while agents are idle, without any model.",
    useWhen: "You want an area watched continuously, such as policy tests every hour or a burst of denials.",
    example: { story: "Check the payments Zone's policy tests every hour, and raise an alert after 5 denied agent actions.", name: "Watch payments", intent: "Notice broken rules or repeated denials in payments", config: { signals: [{ source: "policy_tests", every_minutes: 60 }, { source: "denials", every_minutes: 15, threshold: 5 }] } },
    settings: [{ name: "signals", meaning: "What is watched, how often (at least every 5 minutes), and for denials the count that raises it.", values: "source: governance_integrity, selections_resolved, policy_tests, denials (ci, metrics, external have no connector yet); every_minutes; threshold" }],
  },
  intelligence: {
    what: "Advisory analysis of local events that proposes improvements and never applies them.",
    useWhen: "You want hints, such as rules agents keep hitting or context that went stale while the code changed.",
    example: { story: "Spot rules agents are repeatedly denied by in the API Zone, and context that no longer matches the code.", name: "API rule hints", intent: "Suggest rule and context improvements for the API Zone", config: { analyses: ["denial_patterns", "stale_context"], advisory_only: true } },
    settings: [
      { name: "analyses", meaning: "Analyses to run.", values: "denial_patterns, stale_context" },
      { name: "advisory_only", meaning: "It proposes, never applies.", values: "must be true" },
    ],
  },
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
