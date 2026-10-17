"use strict";
// The dashboard's glossary: every flow of DASHBOARD.md (title and what happens), the 13 attribute
// groups and their sub-attributes, the meaning of every value the dashboard shows (severities,
// decisions, enforcement, origins, alert kinds, compliance outcomes, trace and rule states), every
// eval, and every facet. The info icons read it. Generated from DASHBOARD.md and kept in step with it.
window.FOXX_GLOSSARY = {
 "flows": {
  "F00": [
   "Signing in and choosing a scope on the dashboard",
   "A person opens the dashboard and is asked to sign in with GitHub."
  ],
  "F01": [
   "Setting up a repository (owner)",
   "The owner connects their clone to its GitHub repository and initializes Crane."
  ],
  "F02": [
   "A teammate joins, a machine is linked, the owner key is confirmed",
   "A teammate clones the repository and runs setup."
  ],
  "F03": [
   "An agent session starts",
   "When an agent starts, Crane finds the repository, checks the signed rules, and finds or creates a session with its own identity."
  ],
  "F04": [
   "A prompt is submitted",
   "Each prompt opens a new task inside the session, optionally linked to an external tracker task."
  ],
  "F05": [
   "Before each tool call: the decision",
   "Before the agent's tool runs, Crane works out what it would do (read, edit, run a command, or something else), checks the rules are intact, and applies, in order: the session's safety state, protection of Crane's own files, shell command an"
  ],
  "F06": [
   "After each tool call: verification",
   "After the tool ran, Crane pairs it with its decision (or records that there was none), charges or releases the credits, attributes the files a shell or other tool changed, and re-checks the protected blocks."
  ],
  "F07": [
   "Stop: the agent says it is done",
   "Crane first compares the work tree with the session's starting state and reports, once per file, changes nobody recorded."
  ],
  "F08": [
   "The session ends",
   "Crane releases credits reserved for actions that never reported back (recording each as a gap), expires unused credits, closes the session and task, and signs a digest of all the session's events with the machine's upload key, so the sessio"
  ],
  "F09": [
   "Evidence upload",
   "After actions, at stop and session end, after Crane commands, and otherwise every minute (F34), a background process uploads what is new."
  ],
  "F10": [
   "A session stops reporting",
   "A session that never ended, has no signed end, and that no hook has seen for the stale timeout (120 minutes by default) is treated as stale: the agent was stopped abruptly, its hooks were removed, or its evidence is not arriving."
  ],
  "F11": [
   "Crane fails during a hook",
   "If Crane hits an error while deciding or verifying, it records the failure and denies the action: it never lets an action through because it could not check it."
  ],
  "F12": [
   "The rules are tampered with, rolled back, or forked",
   "On every check Crane verifies the signed rules: signatures and trusted signers, the digest of every rules file, the audit and checkpoint chains, the admin list version, and this machine's memory of the newest rules version."
  ],
  "F13": [
   "A Crane command runs",
   "Every Crane command in a governed repository is recorded with its outcome, duration, and whether an agent tried to run it; secrets in arguments are removed."
  ],
  "F14": [
   "Alerts are delivered",
   "When an event raises an alert (its `alert_kind` is not `NONE`), Crane records it with its trigger, the value seen, the threshold, and the action taken; repeats within a cooldown are suppressed; it is delivered to the configured channels (Sl"
  ],
  "F15": [
   "Installing, checking, and removing agent hooks",
   "A person installs Crane's hooks into Claude Code's or Codex's settings (additive and idempotent), validates them (each lifecycle event registered or missing), or removes them."
  ],
  "F16": [
   "The agent tool asks the person for permission",
   "Some agent tools ask the person before running a tool."
  ],
  "F17": [
   "Session resumed, compacted, or cleared",
   "An agent session can be resumed, compacted, or cleared."
  ],
  "F18": [
   "Protected blocks move, break, or go missing",
   "Crane follows each block by its markers."
  ],
  "F19": [
   "Policy tests and the enforcement self-test",
   "A person or CI runs the policy tests: every rule is checked against the trusted checkpoint, then any project test commands configured in the rules run."
  ],
  "F20": [
   "Policy context files",
   "A person attaches a file to a policy as guidance for the agent."
  ],
  "F21": [
   "Repository connection and GitHub access",
   "A person connects the clone over SSH, HTTPS, or the GitHub CLI and chooses which read privileges Crane may use."
  ],
  "F22": [
   "Setting up integrations (Slack, WhatsApp, Jira, GitHub)",
   "A person configures an integration on their machine."
  ],
  "F23": [
   "Two admins change the rules at the same time",
   "Merging two admins' rule changes from different branches conflicts in the rules folder, since each side carries its own signed version."
  ],
  "F24": [
   "Checks in CI or other headless machines",
   "On a build server, the owner key is supplied by configuration, so Crane verifies the rules and runs the policy tests without a person."
  ],
  "F25": [
   "Keys: signing keys and the upload key",
   "A repository signing key is created sealed to the person's Windows login (or under a passphrase typed on the console); older plain keys are sealed at the next signing command."
  ],
  "F26": [
   "Local speed caches",
   "To keep each tool call fast, Crane remembers the repository identity for ten minutes (refreshed when the Git settings change) and remembers checkpoint commits it already verified."
  ],
  "F27": [
   "The credit budget",
   "Every session's autonomy budget moves through fixed steps: granted at start, reserved when an action is allowed, consumed when it ran or released when it failed or never reported back, revoked as a penalty after a violation, and expired whe"
  ],
  "F28": [
   "Machine status reports",
   "Alongside uploads, each machine sends a status report at most every two minutes: whether the rules verify (with findings), policy results, where each block is, checkpoints, admins, autonomy settings, its Crane version, its GitHub connection"
  ],
  "F29": [
   "Running the hosted service",
   "At start the service upgrades its database layout (versioned, safe to repeat), then serves uploads and the dashboard."
  ],
  "F30": [
   "Browsing the dashboard, downloads, and approval edge cases",
   "A signed-in person moves between tabs, filters the event timeline, pages through it, opens session pages, and downloads a session report or evidence bundle."
  ],
  "F31": [
   "The Landscape editor extension",
   "Inside VS Code, Landscape sets up Crane, opens a panel for the selected code (or a whole file from the Explorer), and runs actions from it: protect, target, zone, flow, context, policy, checkpoint, move, hooks."
  ],
  "F32": [
   "Pull request notifications",
   "Outside Crane's runtime, a repository workflow reacts to pull requests being opened, reopened, marked ready, or closed, posting to Slack and commenting on the Jira issue named in the title or branch."
  ],
  "F33": [
   "Building, releasing, and publishing",
   "Every push runs formatting, lints, all tests, a smoke test of the real binary, and the extension tests."
  ],
  "F34": [
   "Background upload timing",
   "Uploads never hold up the agent."
  ],
  "F35": [
   "Read-only inspection commands",
   "People (and agents, which may run only these) inspect state without changing it: validate, parse a file's markers, view the current task or session, view admins, view the repository connection, view a policy's context, view hook validity."
  ],
  "F36": [
   "Agent network and external tool use",
   "Within F05, some calls reach outside the repository: web fetches and searches, shell commands that use the network, and tools from external (MCP) servers."
  ]
 },
 "groups": {
  "action": {
   "title": "what was done and by whom",
   "keys": {
    "action_type": "flow-specific (for example SIGN_IN, INIT, TOOL_CALL)",
    "action_source": "DASHBOARD, CLI, HOOK, EXTENSION, SERVICE, CI, GITHUB_WORKFLOW, LINK (an address opened from a printed link), AGENT (an agent ran the command)",
    "action_user": "GitHub login, else Git user.name, else OS user",
    "action_email": "Git user.email or the GitHub public email",
    "action_id": "the operation's own identifier (tool-call id, command id, request id)"
   }
  },
  "repository": {
   "title": "the repository it concerns",
   "keys": {
    "repo": "PERSONAL, ORGANIZATION, UNKNOWN",
    "repo_id": "repository identity",
    "repo_remote": "normalized remote, for example github.com/acme/payments",
    "repo_remote_verified": "the remote was proven by root commits",
    "registry_generation": "signed rules version in effect"
   }
  },
  "session": {
   "title": "the agent session or chat window",
   "keys": {
    "session_type": "NOT_IN_SESSION, AGENT_SESSION, DASHBOARD_SESSION",
    "session_id": "Crane session id (ses_…) or dashboard session reference",
    "provider_session_id": "the id the agent tool reported",
    "id_provenance": "PROVIDER, ENVIRONMENT, GENERATED, NA",
    "task_id": "Crane task id (tsk_…)",
    "external_task_id": "tracker task (for example a Jira key)"
   }
  },
  "agent": {
   "title": "the agent acting",
   "keys": {
    "agent_type": "NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC, MULTI_AGENT (a sub-agent inside a session)",
    "agent_id": "Crane agent id (agt_…)",
    "model": "model reported by the agent tool",
    "autonomy_mode": "OBSERVE, ASSISTED, DELEGATED, AUTONOMOUS, NA"
   }
  },
  "resource": {
   "title": "what was used or touched",
   "keys": {
    "resource": "NOT_CONSUMED, FILE_READ, FILE_WRITE, FILE_DELETE, COMMAND, SCRIPT, NETWORK, TOOL, CRANE_METADATA, TRUST_DIRECTORY, HOOK_SETTINGS, SELECTION, CREDITS, GIT, DATABASE, SIGNING_KEY, UPLOAD_KEY",
    "resource_ids": "paths, selection ids, hosts, tool names (secrets redacted)",
    "resource_classification": "NONE, PROTECTED, TARGETED, ZONED, METADATA, TRUST, EXTERNAL, UNATTRIBUTED"
   }
  },
  "response": {
   "title": "what the operation answered or produced",
   "keys": {
    "response_status": "flow-specific (for example OK, FAILED, DENIED)",
    "failure_state": "NA when it succeeded; otherwise flow-specific",
    "response_statement": "at most 10 words of context, written by the agent when an agent acted, otherwise by Crane"
   }
  },
  "assessment": {
   "title": "the pre-check of the action or plan",
   "keys": {
    "assessment": "NOT_REQUIRED, COMPLIANT, VIOLATION, SUSPICIOUS, UNRESOLVED, PENDING",
    "assessment_checks": "checks that ran (flow-specific, for example INTEGRITY, SIMULATION, SCRIPT_SCAN, ZONE)"
   }
  },
  "decision": {
   "title": "what was decided",
   "keys": {
    "decision": "NOT_REQUIRED, ALLOW, REQUIRE_APPROVAL, DENY, QUARANTINE, ERROR",
    "decision_rule": "which rule decided (flow-specific)",
    "decision_reasons": "plain-language reasons",
    "enforcement": "NOT_APPLICABLE, PREVENTED, DETECTED_AFTER_EFFECT, BYPASS_CONFIRMED, COVERAGE_GAP"
   }
  },
  "eval": {
   "title": "numbers",
   "keys": {
    "eval": "NOT_REQUIRED, MEASURED"
   }
  },
  "severity": {
   "title": "the joined verdict",
   "keys": {
    "severity_status": "INFO, LOW, MEDIUM, HIGH, CRITICAL",
    "severity_basis": "OUTCOME (from decision, assessment, and response), FAILURE_STREAK (rule S1), THRESHOLD (an eval crossed a limit), FIXED (always this value for this event)",
    "alert_kind": "the alert this event raises, or NONE (see F14)"
   }
  },
  "bindings": {
   "title": "third parties and calls outside the codebase",
   "keys": {
    "third_party": "NONE, GITHUB, GITHUB_CLI, GIT_REMOTE, FOXX_SERVICE, MONGODB, SLACK, WHATSAPP, JIRA, CLAUDE_CODE, CODEX, VSCODE, VSCODE_MARKETPLACE, OPEN_VSX, RENDER, GITHUB_ACTIONS, WINDOWS_DPAPI",
    "binding_result": "NA, OK, FAILED, TIMEOUT, SKIPPED"
   }
  },
  "telemetry": {
   "title": "current state and how facts are known",
   "keys": {
    "agent_state": "INACTIVE (no agent involved), ACTIVE, DEGRADED, QUARANTINED, STALE, ENDED, ERROR",
    "known_as": "OBSERVED, INFERRED, ESTIMATED, UNSUPPORTED, REDACTED, UNOBSERVED, LOST",
    "crane_version": "version of the emitting Crane",
    "hook_event": "NA, SESSION_START, PROMPT_SUBMIT, PRE_TOOL_USE, PERMISSION_REQUEST, POST_TOOL_USE, STOP, SESSION_END"
   }
  },
  "other": {
   "title": "flow-specific details that fit no other group",
   "keys": {
    "notes": "facts found deep in Crane's code, attached to the next event",
    "rule_name": "the alert rule that fired (F14)"
   }
  }
 },
 "values": {
  "INFO": "Routine: something normal happened and was recorded.",
  "LOW": "Minor: worth knowing, nothing to act on.",
  "MEDIUM": "Needs attention: a refusal, a failed step, or a degraded state.",
  "HIGH": "Serious: a rule was broken or an agent was stopped from breaking one.",
  "CRITICAL": "Severe: protected code changed around the checks, or evidence was tampered with.",
  "ALLOW": "The tool call was allowed to run.",
  "DENY": "The tool call was refused before it ran.",
  "REQUIRE_APPROVAL": "The agent tool must ask the person before running the call.",
  "QUARANTINE": "The session is quarantined; every mutating call is refused.",
  "ERROR": "Crane failed while deciding and refused the call (fail closed).",
  "NOT_REQUIRED": "No decision applies to this event.",
  "PREVENTED": "Stopped before it could happen.",
  "DETECTED_AFTER_EFFECT": "Found after the action ran; the session was degraded and asked to repair.",
  "BYPASS_CONFIRMED": "Protected code changed outside the checks; the session was quarantined.",
  "COVERAGE_GAP": "An action happened without a decision or an observation.",
  "NOT_APPLICABLE": "Nothing for the control to apply to yet (for example, no preserve rules are defined).",
  "MACHINE": "Recorded by Crane on a member's machine (hooks, commands, the Landscape extension), hash-chained and signed.",
  "SERVICE": "Recorded by the ingestion service (uploads, status reports, sign-ins, dashboard views).",
  "DERIVED": "Computed by the dashboard from other records (credits, alerts, the audit trail, stale sessions).",
  "WORKFLOW": "Printed by a GitHub workflow (pull requests, CI, releases); not uploaded.",
  "BYPASS_ATTEMPT": "An agent tried to change Crane's own protections; the attempt was refused.",
  "VIOLATION": "An allowed action broke a rule, found after it ran.",
  "LOW_CREDITS": "A session's autonomy credits are running low.",
  "PROMPT_INJECTION": "A prompt matched prompt-injection phrases (heuristic).",
  "UNATTRIBUTED_CHANGE": "Files changed that no recorded action explains.",
  "VERIFICATION_FAILED": "A task ended but its rules did not pass.",
  "EVIDENCE_CHAIN_BROKEN": "A machine's evidence history was rewritten or removed.",
  "THRESHOLD": "An alert rule's metric crossed its threshold.",
  "NONE": "No value: the attribute does not apply.",
  "OTHER": "Every value outside the largest groups, summed.",
  "COMPLIANT": "Every check passed.",
  "UNRESOLVED": "A check could not be completed (a selection could not be found).",
  "SUSPICIOUS": "The evidence looks tampered with or inconsistent.",
  "SATISFIED": "Positive evidence shows the control works.",
  "PARTIAL": "Some evidence, with gaps a person can fix.",
  "NOT_SATISFIED": "The evidence shows the control failing.",
  "NO_EVIDENCE": "Nothing has exercised the control yet; not a failure.",
  "OK": "Finished without a refusal or a failure.",
  "FAILED": "At least one event was refused, failed, or broke a rule.",
  "FIRING": "The rule's metric is past its threshold; its channels were notified.",
  "RESOLVED": "The metric returned within the threshold.",
  "DISABLED": "The rule exists but is not evaluated.",
  "DELIVERED": "The channel accepted the message (HTTP 2xx).",
  "QUEUED": "Sent in the background without waiting; delivery not confirmed.",
  "DRY_RUN": "Delivery was skipped on purpose (CRANE_ALERTS_DRY_RUN=1).",
  "PENDING": "The grant was approved on the dashboard and is waiting for the agent's machine to sync and add it to that session's credit ledger. It normally becomes APPLIED at the machine's next sync (about a minute while the agent is active); until then, the credits are not available to spend.",
  "APPLIED": "The agent's machine added it to its ledger.",
  "REJECTED": "The machine could not apply it (for example, the session is not on that machine).",
  "CLAUDE_CODE": "Anthropic's Claude Code agent.",
  "CODEX": "OpenAI's Codex agent.",
  "NOT_AN_AGENT": "A person, the service, or a workflow, not an AI agent.",
  "STALE": "The session stopped reporting: no hook activity and no signed end.",
  "QUARANTINED": "Every mutating action of the session is refused.",
  "DEGRADED": "The session broke a rule and was asked to repair."
 },
 "evals": {
  "events": "Events matching the time range and filter.",
  "tool_calls_decided": "Tool calls the pre-tool hook decided (F05).",
  "allowed": "Tool calls allowed to run.",
  "denied": "Tool calls denied, quarantined, or refused after an error.",
  "approvals": "Tool calls that had to ask the person first.",
  "denial_share": "Denied ÷ decided, from 0 to 1.",
  "bypass_attempts": "Denied calls aimed at Crane's own protections.",
  "bypasses_confirmed": "Protected code that changed outside the checks. Should stay 0.",
  "violations_after_effect": "Rules broken by calls that were allowed, found afterwards.",
  "coverage_gaps": "Actions with no decision or no observation.",
  "missing_decisions": "Tool calls that ran without a pre-tool decision.",
  "unattributed_changes": "Changes no recorded action explains.",
  "scripts_blocked": "Risky scripts blocked (downloads, encoded commands, mass deletion).",
  "quarantines": "Sessions quarantined.",
  "verified": "Tool calls verified after running (F06).",
  "evidence_completeness": "Share of allowed calls that also got their post-call verification.",
  "tasks_verified": "Tasks whose rules all passed at stop.",
  "tasks_failed": "Tasks that ended unverified or failed.",
  "suspicious_prompts": "Prompts matching prompt-injection phrases.",
  "attestations_signed": "Sessions whose evidence Crane signed at their end.",
  "secrets_redacted": "Secrets removed from evidence before it was stored.",
  "latency_ms": "Hook latency percentiles in milliseconds.",
  "tool_duration_ms": "How long tools ran, in milliseconds.",
  "credits": "Autonomy credits: granted, consumed, reserved, revoked, expired, available.",
  "models": "Models the agents reported.",
  "identity_provenance": "Where session identities came from (provider, environment, generated).",
  "network_destinations": "Hosts parsed from commands and tool arguments (inferred, not observed).",
  "by_event": "Count of each event name.",
  "by_flow": "Count of each flow.",
  "by_decision": "Count of each decision.",
  "by_severity": "Count of each severity.",
  "by_enforcement": "Count of each enforcement outcome.",
  "by_alert": "Count of each alert kind."
 },
 "facets": {
  "severity": "How serious the event is: INFO, LOW, MEDIUM, HIGH, CRITICAL.",
  "flow": "Which of the 37 flows (F00 to F36) recorded the event.",
  "decision": "What Crane decided about a tool call.",
  "enforcement": "What enforcement achieved: prevented, detected after the effect, a confirmed bypass, a coverage gap.",
  "alert": "The alert the event raised, if any.",
  "origin": "Who recorded the event: a machine, the service, the dashboard, or a workflow.",
  "agent": "The kind of agent behind the event.",
  "source": "Where the action came from: a hook, the CLI, the extension, the service, the dashboard.",
  "event": "The event's name.",
  "response": "How the action ended.",
  "tool": "The agent tool called (BASH, EDIT, WRITE, READ, WEB_FETCH, …).",
  "member": "The machine (member) that recorded the event."
 }
};
