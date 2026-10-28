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

/** The fixed Brick kinds, as 'crane brick types' lists them: each answers one question no other kind answers. */
export const BRICK_TYPES = ["knowledge", "identity", "access", "approval", "limits", "environment", "precondition", "verification", "action", "recovery", "provenance", "watch"] as const;

/** Former kinds: still read from signed stores (as the kind that replaced them), never created. */
export const LEGACY_TYPES: Record<string, string> = {
  context: "knowledge",
  scope: "access",
  permission: "access",
  constraint: "limits and access",
  isolation: "environment",
  budget: "limits",
  revocation: "watch",
  monitoring: "watch",
  intelligence: "watch",
};

/** A current Brick kind. */
export type CurrentType = (typeof BRICK_TYPES)[number];

/** A stored Brick kind: current, or former. */
export type BrickType = CurrentType | keyof typeof LEGACY_TYPES;

/** One kind of the catalog. */
export interface TypeInfo {
  type: CurrentType;
  label: string;
  purpose: string;
  unique?: string;
  support: "enforced" | "checked" | "delivered";
  runtime: string;
  fields: string[];
}

/** Starting configurations offered when a person creates a Brick; Crane validates whatever is kept. */
export const BRICK_TEMPLATES: Record<CurrentType, Record<string, unknown>> = {
  knowledge: { files: [], glossary: {}, references: [] },
  identity: { agents: ["claude"], models: [], on_other: "deny" },
  access: { include: ["src/**"], exclude: [], read_only: [], deny_read: [".env*"], write: "auto", delete: "ask" },
  approval: { operations: ["dependency_change", "git_push"], paths: [], reason: "" },
  limits: { max_tool_calls: 200, max_runtime_minutes: 60, on_exhausted: "escalate" },
  environment: { profile: "agent-hooks", require_env: [], hide_env: ["*_TOKEN", "*_SECRET"], credentials: [], network: { allow: [], deny: [] } },
  precondition: { checks: [{ kind: "branch_not", patterns: ["main"] }], on_fail: "ask" },
  verification: { checks: [{ kind: "policy_tests_pass" }] },
  action: { operations: [{ adapter: "git", operation: "git.push" }], deny_direct: true },
  recovery: { on_failure: "escalate", max_retries: 1, reconcile_before_retry: true, max_consecutive_failures: 3 },
  provenance: { require: ["signed_state", "audit_chain"], commits: 20 },
  watch: { signals: [{ source: "policy_tests", every_minutes: 60 }], on_fire: ["alert"] },
};

/** One configurable setting of a Brick kind, explained. */
export interface GuideSetting {
  name: string;
  meaning: string;
  values: string;
}

/** A Brick kind explained for people choosing one: its purpose and what only it does (one line
 * each), when to use it, a worked example, and its settings. The values follow Crane's schema
 * (src/brick/config.rs); Crane validates. */
export interface BrickGuide {
  purpose: string;
  unique: string;
  useWhen: string;
  example: { story: string; name: string; intent: string; config: Record<string, unknown> };
  settings: GuideSetting[];
}

/** A mode of an operation. */
const MODES = "auto (allowed), ask (a person approves), deny (refused)";
/** Gate checks, which a precondition runs before every change. */
const GATES = 'file_exists / file_absent {path}, file_contains {path, text}, env_set {name}, branch_is / branch_not {patterns}, git_clean, time_window {days, from, to, utc_offset_minutes}, command_available {name}, credential_present {name}, signed_state';
/** Checks a verification may also run. */
const PROOFS = 'every gate check, plus policy_tests_pass, selections_resolved, http_ok {url, status}, json_value {path, pointer, op: at_least|at_most|equals, value}, local_metric {metric: denials|tool_calls, window_hours, max}, command_succeeds {program, args} (run only by a person)';
/** The field that shares an Access Brick's scope. */
const SCOPE_FROM: GuideSetting = { name: "scope_from", meaning: "Take the files from an Access Brick attached to the same Zone or Flow (its id): one scope, defined once.", values: "a Brick id (optional)" };

/** The guide of every current Brick kind. */
export const BRICK_GUIDE: Record<CurrentType, BrickGuide> = {
  knowledge: {
    purpose: "Gives agents the reviewed documents, terms, and references they need in this Zone or Flow.",
    unique: "The only kind that puts repository documents into an agent's context; it informs and never restricts.",
    useWhen: "Agents keep making mistakes in an area because they lack background the code does not show.",
    example: { story: "The payments Zone has rounding and currency rules. A Knowledge Brick delivers docs/payments.md and the team's terms to every agent before it acts there.", name: "Payments domain notes", intent: "Agents should know our rounding and currency rules before editing payments", config: { files: ["docs/payments.md"], glossary: { "minor units": "amounts are integers in cents, never floats" }, references: ["Runbook: wiki/payments/refunds"] } },
    settings: [
      { name: "files", meaning: "Repository files delivered with the common context.", values: "list of repository-relative paths" },
      { name: "glossary", meaning: "Terms and what they mean in this target.", values: "map of term → meaning" },
      { name: "references", meaning: "Other sources agents should know about, one line each.", values: "list of text lines" },
    ],
  },
  identity: {
    purpose: "Says which agent providers and models may work in this Zone or Flow at all.",
    unique: "The only kind that decides by who is calling, whatever they try to do.",
    useWhen: "Only some agents (or some models) should touch an area, e.g. only Claude Opus on infrastructure.",
    example: { story: "Only Claude agents on an Opus model may work in the deployment Flow; any other agent's tool calls there are refused.", name: "Claude Opus only on deployment", intent: "Keep deployment work to one reviewed agent and model", config: { agents: ["claude"], models: ["claude-opus-*"], on_other: "deny" } },
    settings: [
      { name: "agents", meaning: "Providers or exact agent ids admitted.", values: "claude, codex, copilot, gemini, or an agent id" },
      { name: "models", meaning: "Model name patterns admitted (empty: any model).", values: "list such as claude-opus-*" },
      { name: "on_other", meaning: "What anyone else's calls get.", values: "deny (refused) or ask (a person approves)" },
    ],
  },
  access: {
    purpose: "Says which files agents may read or change here, and how each operation is treated.",
    unique: "The only kind with per-path, per-operation rules: in or out of scope, read-only, unreadable, and auto/ask/deny per operation.",
    useWhen: "An area is broad but agents should edit only part of it, never read secrets, or ask before deleting.",
    example: { story: "In the API Zone agents edit src/api/** but not the generated clients, never read .env files, need a person to delete, and may not run commands on these files.", name: "API source only", intent: "Agents edit handwritten API code, never generated clients or secrets", config: { include: ["src/api/**"], exclude: ["src/api/generated/**"], deny_read: [".env*", "secrets/**"], write: "auto", delete: "ask", execute: "deny" } },
    settings: [
      { name: "include", meaning: "Files agents may change (empty: the whole target); the rest is read-only.", values: "list of globs" },
      { name: "exclude", meaning: "Files carved out of include; they stay read-only.", values: "list of globs" },
      { name: "read_only", meaning: "Files agents may read but never change.", values: "list of globs" },
      { name: "deny_read", meaning: "Files agents may not even read.", values: "list of globs" },
      { name: "write", meaning: "Creating and updating files (shorthand for create + update).", values: MODES },
      { name: "create / update / delete", meaning: "Each operation on its own.", values: MODES },
      { name: "execute", meaning: "Shell commands that name this target's files.", values: MODES },
      { name: "network", meaning: "Network calls while working in this target.", values: MODES },
      { name: "allow", meaning: "The older permission list, still accepted: the operations allowed, everything else denied. Use it or the modes above, not both.", values: "list of read, write, delete, execute, network" },
    ],
  },
  approval: {
    purpose: "Names the kinds of change a person must approve, even where access allows them.",
    unique: "The only kind that recognizes kinds of change (dependency, config, migration, push, package install, large diff) instead of files or operations.",
    useWhen: "Most work is fine for agents, but some kinds of change always need a person's eyes.",
    example: { story: "Agents work freely in the checkout Flow, but dependency or migration changes, pushes, and any edit over 200 lines wait for a person.", name: "Human checks on risky changes", intent: "A person sees dependency, migration, push, and large changes first", config: { operations: ["dependency_change", "migration_change", "git_push", "large_change"], min_lines_changed: 200, reason: "Release owners review these" } },
    settings: [
      { name: "operations", meaning: "Kinds of change that need a person.", values: "write, create, update, delete, execute, network, dependency_change, config_change, migration_change, test_change, git_commit, git_push, package_install, large_change" },
      { name: "paths / except", meaning: "Only changes to these files, minus the exceptions (empty: the whole target).", values: "lists of globs" },
      { name: "min_lines_changed", meaning: "From how many changed lines a change is a large_change.", values: "whole number" },
      { name: "reason", meaning: "Shown to the person who approves.", values: "text" },
      { name: "approvers / min_approvals / distinct_approver / expires_minutes", meaning: "Who approves, how many, not the requester, and how long an approval lasts (resident agents).", values: "list of logins, 0-5, true/false, minutes" },
      SCOPE_FROM,
    ],
  },
  limits: {
    purpose: "Caps how much and how long agents work in this Zone or Flow in one session.",
    unique: "The only kind that counts: tool calls, minutes, files and lines changed, commands, network calls, denials, and the autonomy ceiling.",
    useWhen: "Agents in an area should not run away with long sessions, sweeping edits, or endless retries.",
    example: { story: "In the experiments Zone a session gets 200 tool calls, 30 minutes, and 10 changed files; after 5 denials it stops; a person decides whether to go on.", name: "Experiments budget", intent: "Keep experiment sessions small and short", config: { max_tool_calls: 200, max_runtime_minutes: 30, max_files_changed: 10, max_denials: 5, on_exhausted: "escalate" } },
    settings: [
      { name: "max_tool_calls / max_commands / max_network_calls", meaning: "Calls touching the target per session.", values: "whole numbers" },
      { name: "max_runtime_minutes", meaning: "Minutes since the session started.", values: "whole number" },
      { name: "max_files_changed / max_lines_changed", meaning: "Distinct files and total lines changed per session.", values: "whole numbers" },
      { name: "max_denials", meaning: "Denied calls before the session stops in the target.", values: "whole number" },
      { name: "max_retries", meaning: "Retries of a failed step (resident agents and skills).", values: "whole number" },
      { name: "max_ceiling / min_criticality", meaning: "The highest autonomy, and the lowest criticality, of the target.", values: "observe|assisted|delegated|autonomous; routine|sensitive|critical|restricted" },
      { name: "on_exhausted", meaning: "What happens at a cap.", values: "stop (refused) or escalate (a person approves)" },
      SCOPE_FROM,
    ],
  },
  environment: {
    purpose: "Defines what the work runs with: environment variables, credentials, and network reach.",
    unique: "The only kind that governs the surroundings of the work: variables that must exist or stay hidden, credentials by name, and reachable hosts.",
    useWhen: "Work needs certain variables or credentials, must not leak secrets, or may only call certain hosts.",
    example: { story: "Agents in the billing Zone may call only api.stripe.com, never read AWS_* variables, and need STRIPE_MODE set; Crane-run Jira steps use the jira-billing credential from each machine's vault.", name: "Billing environment", intent: "Billing work reaches only Stripe and never sees cloud secrets", config: { require_env: ["STRIPE_MODE"], hide_env: ["AWS_*", "*_SECRET"], credentials: [{ name: "jira-billing", for: ["jira"] }], network: { allow: ["api.stripe.com"], deny: [] } } },
    settings: [
      { name: "profile", meaning: "The isolation profile ('crane isolation profiles').", values: "agent-hooks (default)" },
      { name: "require_env", meaning: "Variables that must be set where agents run; if missing, changes need a person.", values: "list of names" },
      { name: "hide_env", meaning: "Variables agents may not read ($NAME, printenv, env dumps are refused).", values: "list of names or patterns such as AWS_*" },
      { name: "credentials", meaning: "Credentials Crane-run operations use, by vault name; values stay on each machine ('crane secret set NAME').", values: "list of {name, for: [integrations or operations]}" },
      { name: "network.allow / network.deny", meaning: "Hosts calls may reach while working here, and hosts never reached.", values: "host names such as api.example.com or *.example.com" },
    ],
  },
  precondition: {
    purpose: "Gates changes on facts that must hold first: files, branch, clean tree, time window, variables.",
    unique: "The only kind evaluated before every change; while a gate fails, changes need a person or stop.",
    useWhen: "Work is only safe at certain times or states: not on main, during business hours, with no release lock.",
    example: { story: "Agents may change the release Flow only off main, on weekdays 09:00-18:00 IST, with a clean tree and no release.lock file.", name: "Safe release window", intent: "No agent changes on main, after hours, or during a release", config: { checks: [{ kind: "branch_not", patterns: ["main", "release/*"] }, { kind: "time_window", days: ["mon", "tue", "wed", "thu", "fri"], from: "09:00", to: "18:00", utc_offset_minutes: 330 }, { kind: "git_clean" }, { kind: "file_absent", path: "release.lock" }], on_fail: "ask" } },
    settings: [
      { name: "checks", meaning: "Gates that must all hold before a change.", values: GATES },
      { name: "on_fail", meaning: "What a failing gate does.", values: "ask (changes need a person) or deny (changes stop)" },
    ],
  },
  verification: {
    purpose: "Proves the outcome after work: policy tests, files, endpoints, metrics, and commands.",
    unique: "The only kind that produces evidence after the fact: proven, contradicted, unknown, or not run.",
    useWhen: "You want proof the area still works after agents changed it, not just that rules were followed.",
    example: { story: "After agents change the checkout Flow: policy tests pass, coverage stays at least 80 %, the health endpoint answers 200, the test suite succeeds, and the release gates still hold.", name: "Checkout stays healthy", intent: "Prove checkout still works after agent work", config: { checks: [{ kind: "policy_tests_pass" }, { kind: "json_value", path: "coverage/coverage-summary.json", pointer: "/total/lines/pct", op: "at_least", value: 80 }, { kind: "http_ok", url: "https://staging.example.com/health", status: 200 }, { kind: "command_succeeds", program: "npm", args: ["test"] }], checks_from: "safe-release-window" } },
    settings: [
      { name: "checks", meaning: "Checks that prove the outcome.", values: PROOFS },
      { name: "checks_from", meaning: "Also re-run a Precondition Brick's gates after the work (its id), so the facts are proven afterwards too.", values: "a Brick id (optional)" },
    ],
  },
  action: {
    purpose: "Lists the side-effecting operations Crane may run here, and keeps agents from doing them directly.",
    unique: "The only kind about operations in other systems: an allow-list with daily caps, dry runs first, and no direct agent shortcut.",
    useWhen: "Pushes, releases, deployments, or tickets must go through a reviewed path, never an agent's shell.",
    example: { story: "In the release Flow, agents may not run git push or gh release create themselves; resident agents may only create tickets, at most 5 a day.", name: "Releases through Crane", intent: "Pushes and releases never come straight from an agent", config: { operations: [{ adapter: "git", operation: "git.push" }, { adapter: "github", operation: "github.release_create" }, { adapter: "resident", operation: "create_ticket", max_per_day: 5, dry_run_first: true }], deny_direct: true } },
    settings: [
      { name: "operations", meaning: "Operations Crane may run for the target.", values: "list of {adapter, operation, max_per_day, dry_run_first, approval}; resident steps are resident/STEP, direct ones git.commit|push|tag|merge|rebase|reset, github.pr_create|pr_merge|release_create|workflow_run, packages.install, kubectl.apply, terraform.apply, docker.push" },
      { name: "deny_direct", meaning: "Agents may not do these operations themselves (their shell commands are refused).", values: "true or false" },
      { name: "adapter / operation", meaning: "The short form for a single operation, instead of the operations list.", values: "an adapter and one of its operations, e.g. git and git.push" },
    ],
  },
  recovery: {
    purpose: "Decides what happens after a failed, partial, or uncertain outcome.",
    unique: "The only kind that reacts to failure: retries with reconciliation first, and a pause after repeated agent failures.",
    useWhen: "Failures must not be retried blindly, or an agent stuck failing should stop and get help.",
    example: { story: "In the deploy Flow, a failed step is retried once after checking what really happened; after 3 failed agent tool calls in a row, changes wait for a person.", name: "Careful deploy recovery", intent: "Never retry blindly; stop a struggling agent", config: { on_failure: "retry", max_retries: 1, backoff_seconds: 30, reconcile_before_retry: true, max_consecutive_failures: 3, on_uncertain: "reconcile" } },
    settings: [
      { name: "on_failure", meaning: "What a failure leads to.", values: "stop, escalate, or retry (within max_retries)" },
      { name: "max_retries / backoff_seconds", meaning: "How often and how far apart retries happen.", values: "0-3; 0-3600 seconds" },
      { name: "reconcile_before_retry", meaning: "Check what really happened before a retry.", values: "must be true" },
      { name: "max_consecutive_failures", meaning: "Failed agent tool calls in a row before changes need a person.", values: "whole number" },
      { name: "on_uncertain", meaning: "An outcome that may or may not have happened.", values: "reconcile or escalate" },
    ],
  },
  provenance: {
    purpose: "Requires every outcome to be traceable: signed state, audit chain, commit trailers, linked issues.",
    unique: "The only kind about lineage: it checks where changes came from, not whether they work.",
    useWhen: "Auditors or reviewers need each change traced to signed rules, a recorded decision, and an issue.",
    example: { story: "The last 20 commits touching the compliance Zone's in-scope files must carry an Agent-Session trailer and name a COMP-* issue; each change must have a recorded guard decision.", name: "Traceable compliance work", intent: "Every compliance change is traceable for audits", config: { require: ["signed_state", "audit_chain", "decisions_recorded", "commit_trailer", "linked_issue"], trailer: "Agent-Session", issue_pattern: "COMP-*", commits: 20, scope_from: "compliance-access" } },
    settings: [
      { name: "require", meaning: "Records every outcome must trace to.", values: "signed_state, audit_chain, decisions_recorded, commit_trailer, linked_issue" },
      { name: "trailer", meaning: "The commit trailer key commit_trailer looks for.", values: "e.g. Agent-Session" },
      { name: "issue_pattern", meaning: "The issue id linked_issue looks for in commit messages.", values: "e.g. PROJ-*" },
      { name: "commits", meaning: "How many recent commits touching the target are checked.", values: "1-500" },
      SCOPE_FROM,
    ],
  },
  watch: {
    purpose: "Keeps checking this Zone or Flow while agents are idle, and reacts when a signal fires.",
    unique: "The only kind that runs on its own schedule: signals with reactions (alert, pause, revoke due) and advisory analyses.",
    useWhen: "You want an area watched continuously and want to hear about it, or stop agents, when something breaks.",
    example: { story: "Every hour Crane checks the payments Zone's policy tests and preconditions, and every 15 minutes counts denials; when one fires it alerts on Slack and pauses agent changes until it clears.", name: "Watch payments", intent: "Notice broken rules fast and stop agents until fixed", config: { signals: [{ source: "policy_tests", every_minutes: 60 }, { source: "preconditions", every_minutes: 60 }, { source: "denials", every_minutes: 15, threshold: 5 }], on_fire: ["alert", "pause"], triggers: ["integrity_failure"], analyses: ["denial_patterns"] } },
    settings: [
      { name: "signals", meaning: "What is checked, how often (at least every 5 minutes), and for denials the count that fires.", values: "source: governance_integrity, selections_resolved, policy_tests, denials, preconditions, verification (ci, metrics, external have no connector yet); every_minutes; threshold" },
      { name: "on_fire", meaning: "What a firing signal does.", values: "alert (integrations), pause (changes need a person while it fires), revoke_due, propose" },
      { name: "triggers", meaning: "When attachments are due for withdrawal.", values: "integrity_failure, verification_contradicted, repeated_denials" },
      { name: "analyses", meaning: "Advisory analyses that only propose.", values: "denial_patterns, stale_context" },
      { name: "advisory_only", meaning: "Analyses propose and never change rules, grant anything, or approve.", values: "must be true (the default)" },
      SCOPE_FROM,
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
