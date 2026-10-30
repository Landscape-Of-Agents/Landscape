# Foxx Dashboard: Event Specification

This document defines every event Crane, the Foxx service, the Foxx dashboard, and the Foxx editor
extension emit, flow by flow. It replaces the earlier metric set: dashboard telemetry charts,
tables, and alerts are derived from these events. Dashboard Agent configurations and incident
journals are separately persisted, repository-scoped monitor state; their measurements are computed
from the same event evidence.

- Part 1 defines the event envelope, the 13 attribute groups, their compulsory sub-attributes,
  the shared value lists, and the severity rules.
- Part 2 defines every flow (F00 to F36). Each flow has a description, its common attributes, and
  every event with all 13 groups.
- Part 3 maps the events to the dashboard tabs.

Notation: `name (VALUE_A, VALUE_B) (type)`. Types are `enum`, `list of enums`, `string`, `integer`,
`number`, `boolean`, `timestamp` (ISO 8601, UTC), `duration_ms` (integer milliseconds), `digest`
(128 hex characters, SHA-512), `key` (64 hex characters, Ed25519 public key), `null` (allowed
when the value is unknown). `(common)` means the event uses its flow's common value unchanged.

---

## Part 1: The event

### 1.1 Envelope (on every event)

| Field | Type | Meaning |
|---|---|---|
| `event_id` | string (`evt_…`) | unique, never reused |
| `event_name` | enum | the event, as named in Part 2 (for example `TOOL_CALL_ALLOWED`) |
| `flow` | enum (`F00` … `F36`) | the flow that emitted it |
| `occurred_at` | timestamp | when it happened |
| `schema_version` | integer | version of this specification |
| `sequence` | integer | position in the emitting machine's event chain (`null` for dashboard and service events) |
| `previous_digest` | digest | digest of the previous event in the chain (`null` as above) |
| `digest` | digest | digest of this event (`null` as above) |
| `origin` | enum (`MACHINE`, `SERVICE`, `DERIVED`, `WORKFLOW`) | which producer wrote it (Part 4) |

### 1.2 The 13 attribute groups

Every event carries all 13 groups. Each group has compulsory sub-attributes (present on every
event, using `NOT_REQUIRED`, `NA`, or `null` when they do not apply) and flow-specific
sub-attributes (defined per event in Part 2).

#### Action: what was done and by whom
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `action_type` | flow-specific (for example `SIGN_IN`, `INIT`, `TOOL_CALL`) | enum | yes |
| `action_source` | `DASHBOARD`, `CLI`, `HOOK`, `EXTENSION`, `SERVICE`, `CI`, `GITHUB_WORKFLOW`, `LINK` (an address opened from a printed link), `AGENT` (an agent ran the command) | enum | yes |
| `action_user` | GitHub login, else Git `user.name`, else OS user | string, null | yes |
| `action_email` | Git `user.email` or the GitHub public email | string, null | yes |
| `action_id` | the operation's own identifier (tool-call id, command id, request id) | string, null | yes |

#### Repository: the repository it concerns
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `repo` | `PERSONAL`, `ORGANIZATION`, `UNKNOWN` | enum | yes |
| `repo_id` | repository identity | digest, null | yes |
| `repo_remote` | normalized remote, for example `github.com/acme/payments` | string, null | yes |
| `repo_remote_verified` | the remote was proven by root commits | boolean | yes |
| `registry_generation` | signed rules version in effect | integer, null | yes |

#### Session: the agent session or chat window
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `session_type` | `NOT_IN_SESSION`, `AGENT_SESSION`, `DASHBOARD_SESSION` | enum | yes |
| `session_id` | Crane session id (`ses_…`) or dashboard session reference | string, null | yes |
| `provider_session_id` | the id the agent tool reported | string, null | yes |
| `id_provenance` | `PROVIDER`, `ENVIRONMENT`, `GENERATED`, `NA` | enum | yes |
| `task_id` | Crane task id (`tsk_…`) | string, null | yes |
| `external_task_id` | tracker task (for example a Jira key) | string, null | yes |

#### Agent: the agent acting
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `agent_type` | `NOT_AN_AGENT`, `CLAUDE_CODE`, `CODEX`, `GENERIC`, `MULTI_AGENT` (a sub-agent inside a session) | enum | yes |
| `agent_id` | Crane agent id (`agt_…`) | string, null | yes |
| `model` | model reported by the agent tool | string, null | yes |
| `autonomy_mode` | `OBSERVE`, `ASSISTED`, `DELEGATED`, `AUTONOMOUS`, `NA` | enum | yes |

#### Resource: what was used or touched
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `resource` | `NOT_CONSUMED`, `FILE_READ`, `FILE_WRITE`, `FILE_DELETE`, `COMMAND`, `SCRIPT`, `NETWORK`, `TOOL`, `CRANE_METADATA`, `TRUST_DIRECTORY`, `HOOK_SETTINGS`, `SELECTION`, `CREDITS`, `GIT`, `DATABASE`, `SIGNING_KEY`, `UPLOAD_KEY` | list of enums | yes |
| `resource_ids` | paths, selection ids, hosts, tool names (secrets redacted) | list of strings | yes (empty when `NOT_CONSUMED`) |
| `resource_classification` | `NONE`, `PROTECTED`, `TARGETED`, `ZONED`, `METADATA`, `TRUST`, `EXTERNAL`, `UNATTRIBUTED` | list of enums | yes |

#### Response: what the operation answered or produced
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `response_status` | flow-specific (for example `OK`, `FAILED`, `DENIED`) | enum | yes |
| `failure_state` | `NA` when it succeeded; otherwise flow-specific | enum | yes |
| `response_statement` | at most 10 words of context, written by the agent when an agent acted, otherwise by Crane | string | yes |

#### Assessment: the pre-check of the action or plan
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `assessment` | `NOT_REQUIRED`, `COMPLIANT`, `VIOLATION`, `SUSPICIOUS`, `UNRESOLVED`, `PENDING` | enum | yes |
| `assessment_checks` | checks that ran (flow-specific, for example `INTEGRITY`, `SIMULATION`, `SCRIPT_SCAN`, `ZONE`) | list of enums | yes (empty when `NOT_REQUIRED`) |

#### Decision: what was decided
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `decision` | `NOT_REQUIRED`, `ALLOW`, `REQUIRE_APPROVAL`, `DENY`, `QUARANTINE`, `ERROR` | enum | yes |
| `decision_rule` | which rule decided (flow-specific) | enum, null | yes |
| `decision_reasons` | plain-language reasons | list of strings | yes |
| `enforcement` | `NOT_APPLICABLE`, `PREVENTED`, `DETECTED_AFTER_EFFECT`, `BYPASS_CONFIRMED`, `COVERAGE_GAP` | enum | yes |

#### Eval: numbers
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `eval` | `NOT_REQUIRED`, `MEASURED` | enum | yes |
| flow-specific measures | for example `latency_ms`, `credits_cost`, `credits_available` | number, integer, duration_ms | when `MEASURED` |

#### Severity: the joined verdict
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `severity_status` | `INFO`, `LOW`, `MEDIUM`, `HIGH`, `CRITICAL` | enum | yes |
| `severity_basis` | `OUTCOME` (from decision, assessment, and response), `FAILURE_STREAK` (rule S1), `THRESHOLD` (an eval crossed a limit), `FIXED` (always this value for this event) | enum | yes |
| `alert_kind` | the alert this event raises, or `NONE` (see F14) | enum | yes |

#### Bindings: third parties and calls outside the codebase
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `third_party` | `NONE`, `GITHUB`, `GITHUB_CLI`, `GIT_REMOTE`, `FOXX_SERVICE`, `MONGODB`, `SLACK`, `WHATSAPP`, `JIRA`, `CLAUDE_CODE`, `CODEX`, `VSCODE`, `VSCODE_MARKETPLACE`, `OPEN_VSX`, `RENDER`, `GITHUB_ACTIONS`, `WINDOWS_DPAPI` | list of enums | yes |
| `binding_result` | `NA`, `OK`, `FAILED`, `TIMEOUT`, `SKIPPED` | enum | yes |

#### Telemetry: current state and how facts are known
| Sub-attribute | Values | Type | Compulsory |
|---|---|---|---|
| `agent_state` | `INACTIVE` (no agent involved), `ACTIVE`, `DEGRADED`, `QUARANTINED`, `STALE`, `ENDED`, `ERROR` | enum | yes |
| `known_as` | `OBSERVED`, `INFERRED`, `ESTIMATED`, `UNSUPPORTED`, `REDACTED`, `UNOBSERVED`, `LOST` | enum | yes |
| `crane_version` | version of the emitting Crane | string | yes |
| `hook_event` | `NA`, `SESSION_START`, `PROMPT_SUBMIT`, `PRE_TOOL_USE`, `PERMISSION_REQUEST`, `POST_TOOL_USE`, `STOP`, `SESSION_END` | enum | yes |

#### Other: flow-specific attributes
Defined per event. Empty list `[]` when the event has none.

### 1.3 Severity rules

| Rule | Applies to | Definition |
|---|---|---|
| **S1 Failure streak** | person-driven and service flows (dashboard, CLI setup, approvals, uploads) | `LOW` while the last attempt was `OK`; `MEDIUM` after 1 non-OK response; `HIGH` after 3 consecutive non-OK responses from the same `action_user` (or machine) within 10 minutes; back to `LOW` at the next `OK` |
| **S2 Outcome** | agent flows | from the decision and assessment: `ALLOW` and `COMPLIANT` = `INFO`; `REQUIRE_APPROVAL` = `LOW`; `DENY` for a rule = `MEDIUM`; `DENY` for an attempt on Crane = `HIGH`; `VIOLATION` detected after effect = `HIGH`; `BYPASS_CONFIRMED`, quarantine, broken evidence chain = `CRITICAL` |
| **S3 Threshold** | eval-driven events | the event names its threshold (for example credits at or below 20) |
| **S4 Fixed** | events whose severity never varies | stated per event |

### 1.4 Response statement

`response_statement` is at most 10 words. When an agent acted, the agent's own one-line
explanation is stored (redacted); otherwise Crane writes it, for example `Preserved selection
K7M2P9RX would change; edit refused.`

---

## Part 2: Flows

### F00. Signing in and choosing a scope on the dashboard

A person opens the dashboard and is asked to sign in with GitHub. After GitHub confirms who they
are, the dashboard learns their personal account and organizations and builds three choices: an
account, one of that account's repositories (only those whose GitHub remote an upload proved), and
optionally one machine. Everything shown afterwards is filtered by these choices. Sessions last
eight hours.

Common attributes
- Action [action_source (DASHBOARD) (enum), action_user (GitHub login) (string, null), action_email (GitHub public email) (string, null), action_id (request id) (string)]
- Repository [repo (PERSONAL, ORGANIZATION, UNKNOWN) (enum), repo_id (null until chosen) (digest, null), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (null) (integer, null)]
- Session [session_type (DASHBOARD_SESSION) (enum), session_id (dashboard session reference) (string, null), provider_session_id (null), id_provenance (NA) (enum), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT) (enum), agent_id (null), model (null), autonomy_mode (NA) (enum)]
- Resource [resource ([NOT_CONSUMED]) (list of enums), resource_ids ([]) (list), resource_classification ([NONE]) (list of enums)]
- Assessment [assessment (NOT_REQUIRED) (enum), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED) (enum), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE) (enum)]
- Eval [eval (NOT_REQUIRED) (enum)]
- Severity [severity_basis (FAILURE_STREAK) (enum), alert_kind (NONE) (enum)]
- Bindings [third_party ([GITHUB]) (list of enums)]
- Telemetry [agent_state (INACTIVE) (enum), known_as (OBSERVED) (enum), hook_event (NA) (enum)]

#### F00.1 `DASH_OPENED_SIGNED_OUT`
The dashboard was opened without a valid session (none, expired, or forged cookie).
- Action [action_type (OPEN_DASHBOARD) (enum), action_user (null)]
- Repository [repo (UNKNOWN)]
- Session [(common), session_id (null)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SIGN_IN_REQUIRED, SESSION_EXPIRED, SIGN_IN_NOT_CONFIGURED) (enum), failure_state (NA, NO_COOKIE, EXPIRED_COOKIE, FORGED_COOKIE, OAUTH_NOT_CONFIGURED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW) (enum), severity_basis (FIXED)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [(common)]
- Other []

#### F00.2 `DASH_SIGN_IN_STARTED`
The person clicked "Sign in with GitHub" and was redirected with a fresh state.
- Action [action_type (SIGN_IN) (enum), action_user (null)]
- Repository [repo (UNKNOWN)]
- Session [(common), session_id (null)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (REDIRECTED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW), severity_basis (FIXED)]
- Bindings [third_party ([GITHUB]), binding_result (NA)]
- Telemetry [(common)]
- Other [oauth_scope (READ_ORG, READ_ORG_AND_REPO) (enum)]

#### F00.3 `DASH_SIGN_IN_COMPLETED`
GitHub returned the person's login and organizations; an 8-hour session started.
- Action [action_type (SIGN_IN) (enum), action_user (GitHub login) (string), action_email (string, null)]
- Repository [repo (PERSONAL, ORGANIZATION) (enum)]
- Session [(common), session_id (string)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), organizations_count (integer), visible_repositories_count (integer)]
- Severity [severity_status (LOW), severity_basis (FAILURE_STREAK)]
- Bindings [third_party ([GITHUB]), binding_result (OK)]
- Telemetry [(common)]
- Other [session_expires_at (timestamp)]

#### F00.4 `DASH_SIGN_IN_FAILED`
The sign-in did not complete.
- Action [action_type (SIGN_IN) (enum), action_user (null)]
- Repository [repo (UNKNOWN)]
- Session [(common), session_id (null)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (FAIL, INVALID) (enum), failure_state (STATE_MISMATCH, NO_CODE, GITHUB_DENIED, GITHUB_UNREACHABLE, TOKEN_NOT_GRANTED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (MEDIUM, HIGH) (enum), severity_basis (FAILURE_STREAK)]
- Bindings [third_party ([GITHUB]), binding_result (FAILED, TIMEOUT) (enum)]
- Telemetry [agent_state (ERROR)]
- Other []

#### F00.5 `DASH_SCOPE_ACCOUNT_SELECTED`
- Action [action_type (SELECT_ACCOUNT) (enum)]
- Repository [repo (PERSONAL, ORGANIZATION) (enum), repo_id (null)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FORBIDDEN) (enum), failure_state (NA, NOT_YOUR_ACCOUNT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), repositories_in_account (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH) (enum), severity_basis (FAILURE_STREAK)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [account_name (string), account_kind (PERSONAL, ORGANIZATION) (enum)]

#### F00.6 `DASH_SCOPE_REPOSITORY_SELECTED`
- Action [action_type (SELECT_REPOSITORY) (enum)]
- Repository [repo (PERSONAL, ORGANIZATION), repo_id (digest), repo_remote (string), repo_remote_verified (true)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FORBIDDEN, BAD_REQUEST) (enum), failure_state (NA, NOT_OWNED_BY_VIEWER, MALFORMED_ID) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), members_count (integer), linked_members_count (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other []

#### F00.7 `DASH_SCOPE_MEMBER_SELECTED`
- Action [action_type (SELECT_MEMBER, CLEAR_MEMBER) (enum)]
- Repository [(common, with repo_id)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW), severity_basis (FIXED)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [member_key (key, null), member_linked (boolean, null), default_selection (ALL_MEMBERS, ALL_LINKED_MEMBERS) (enum)]

#### F00.8 `DASH_SIGNED_OUT`
- Action [action_type (SIGN_OUT) (enum)]
- Repository [repo (UNKNOWN), repo_id (null)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), session_duration_ms (duration_ms)]
- Severity [severity_status (INFO), severity_basis (FIXED)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other []

#### F00.9 `DASH_SERVICE_UNAVAILABLE`
Sign-in or data could not be served (database unavailable, sign-in not configured).
- Action [action_type (OPEN_DASHBOARD, LOAD_VIEW) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (UNAVAILABLE) (enum), failure_state (DATABASE_UNAVAILABLE, SIGN_IN_NOT_CONFIGURED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (HIGH), severity_basis (FIXED)]
- Bindings [third_party ([MONGODB, RENDER]), binding_result (FAILED)]
- Telemetry [agent_state (ERROR)]
- Other []

---

### F01. Setting up a repository (owner)

The owner connects their clone to its GitHub repository and initializes Crane. This creates their
private signing key, writes the signed rules folder, and records the first rules version. They then
record a trusted checkpoint (a known-good commit) and mark code to protect or to change. Each
marked block gets an id, marker comments, and a signed record, and goes into a named policy. They
may also define zones and flows with their criticality, autonomy ceiling, and context, which are
signed as well. Every change raises the rules version and adds an audit entry. Governance commands
refuse to run under an agent.

Common attributes
- Action [action_source (CLI, EXTENSION) (enum), action_user (Git user.name) (string), action_email (Git user.email) (string, null), action_id (command id) (string)]
- Repository [repo (PERSONAL, ORGANIZATION, UNKNOWN) (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (NOT_IN_SESSION) (enum), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT) (enum), agent_id (null), model (null), autonomy_mode (NA)]
- Assessment [assessment (NOT_REQUIRED) (enum), assessment_checks ([INTEGRITY]) (list of enums)]
- Decision [decision (NOT_REQUIRED) (enum), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Severity [severity_basis (FAILURE_STREAK) (enum), alert_kind (NONE)]
- Bindings [third_party ([GIT_REMOTE]) (list of enums)]
- Telemetry [agent_state (INACTIVE) (enum), known_as (OBSERVED), hook_event (NA)]
- Response failure_state values for this flow (enum): NA, CHECKPOINT, POLICY, SYNTAX, GIT, SECURITY, NOT_ADMIN, AGENT_REFUSED, VERIFICATION_FAILED, CONFLICT, KEY_UNAVAILABLE

#### F01.1 `REPOSITORY_CONNECTED`
The clone was connected to its GitHub repository (see F21 for checks and grants).
- Action [action_type (CONNECT) (enum)]
- Repository [(common), registry_generation (null)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([GIT]) (list of enums), resource_ids ([remote url, redacted]) (list), resource_classification ([EXTERNAL])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, GIT, SECURITY) (enum), response_statement (string)]
- Assessment [(common), assessment_checks ([REMOTE_REACHABLE, HOST_ALLOWED])]
- Decision [(common)]
- Eval [eval (MEASURED), probe_latency_ms (duration_ms)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([GITHUB, GIT_REMOTE, GITHUB_CLI]), binding_result (OK, FAILED, TIMEOUT)]
- Telemetry [(common)]
- Other [method (SSH, HTTPS, GH_CLI) (enum), privileges ([RF, CODEOWNERS, CONTRIBUTORS, CI_CD, GH_ACTIONS, GH_BRANCHES, GH_INSIGHTS]) (list of enums)]

#### F01.2 `CRANE_INITIALIZED`
The rules folder was created and generation 1 signed.
- Action [action_type (INIT) (enum)]
- Repository [(common), registry_generation (1)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA, SIGNING_KEY]), resource_ids ([.crane/]), resource_classification ([METADATA, TRUST])]
- Response [response_status (OK, FAILED, ALREADY_INITIALIZED) (enum), failure_state (NA, GIT, SECURITY, AGENT_REFUSED, KEY_UNAVAILABLE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE, FOXX_SERVICE]), binding_result (NA, OK)]
- Telemetry [(common)]
- Other [owner_key (key), default_policy (string), uploads_enabled (boolean)]

#### F01.3 `CHECKPOINT_CREATED`
- Action [action_type (CHECKPOINT) (enum)]
- Repository [(common), registry_generation (integer)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([GIT, CRANE_METADATA]), resource_ids ([commit sha]), resource_classification ([METADATA])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, CHECKPOINT, GIT, NOT_ADMIN, AGENT_REFUSED, VERIFICATION_FAILED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [checkpoint_name (string), commit (string), branch (string, null)]

#### F01.4 `POLICY_CREATED`
- Action [action_type (CREATE_POLICY) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA]), resource_ids ([.crane/policies/NAME.crane])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, POLICY, SYNTAX, NOT_ADMIN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [policy_name (string), policy_file (string)]

#### F01.5 `SELECTION_CREATED`
A block was marked as preserved or targeted (`crane protect` / `crane target`, or the Foxx panel).
- Action [action_type (PRESERVE, TARGET) (enum)]
- Repository [(common), registry_generation (integer)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([FILE_WRITE, SELECTION, CRANE_METADATA]), resource_ids ([path, selection id]), resource_classification ([PROTECTED, TARGETED])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, CHECKPOINT, POLICY, SYNTAX, GIT, NOT_ADMIN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([INTEGRITY, LINES_UNCHANGED_SINCE_CHECKPOINT])]
- Decision [(common)]
- Eval [eval (MEASURED), selected_lines (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [selection_id (string, 8 characters), policy (string), change_type (ANY, LOGICAL_BN, LOGICAL_CN, LOGICAL_SN, SEMANTIC, NA) (enum), language (string), path (string), start_line (integer), end_line (integer), checkpoint (string), name (string, null)]

#### F01.6 `SELECTION_POLICY_CHANGED`
A block was moved to another policy.
- Action [action_type (MODIFY) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION, CRANE_METADATA]), resource_ids ([selection id])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, POLICY, NOT_ADMIN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [selection_id (string), from_policy (string), to_policy (string)]

#### F01.7 `DEFAULT_CHANGED`
- Action [action_type (SET_DEFAULT) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, POLICY, CHECKPOINT, NOT_ADMIN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [default_kind (POLICY, CHECKPOINT) (enum), value (string)]

#### F01.8 `GOVERNANCE_SIGNED`
Zones, flows, and context packs were signed (`crane governance sign`, or automatically by Foxx).
- Action [action_type (SIGN_GOVERNANCE) (enum), action_source (CLI, EXTENSION)]
- Repository [(common), registry_generation (integer)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA]), resource_ids ([.crane/governance.yaml])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, SYNTAX, POLICY, NOT_ADMIN, AGENT_REFUSED, VERIFICATION_FAILED) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([YAML_VALID, REFERENCES_VALID, IDS_UNIQUE, FLOWS_RESOLVED])]
- Decision [(common)]
- Eval [eval (MEASURED), zones (integer), zones_active (integer), flows (integer), flows_active (integer), context_packs (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [zone_changes ([{id, criticality (ROUTINE, SENSITIVE, CRITICAL, RESTRICTED), ceiling (OBSERVE, ASSISTED, DELEGATED, AUTONOMOUS), status (PROPOSED, ACTIVE)}]) (list)]

#### F01.9 `GOVERNANCE_CHANGE_REFUSED`
Any F01 change was refused before anything was written.
- Action [action_type (INIT, CHECKPOINT, PRESERVE, TARGET, CREATE_POLICY, MODIFY, SET_DEFAULT, SIGN_GOVERNANCE) (enum), action_source (CLI, EXTENSION, AGENT)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum)]
- Resource [resource ([CRANE_METADATA])]
- Response [response_status (REFUSED) (enum), failure_state (AGENT_REFUSED, NOT_ADMIN, VERIFICATION_FAILED, CONFLICT, KEY_UNAVAILABLE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (DENY) (enum), decision_rule (HUMAN_ONLY, ADMIN_ONLY, STATE_MUST_VERIFY) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common)]
- Severity [severity_status (MEDIUM, HIGH) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other []

---

### F02. A teammate joins, a machine is linked, the owner key is confirmed

A teammate clones the repository and runs setup. Crane shows the owner's key fingerprint and asks
the service whether a GitHub admin has confirmed that key. The person compares the fingerprint with
the owner and trusts it, which also creates their own key. Separately, the owner can ask for the
owner key to be confirmed: a one-time code is created, a GitHub admin of the repository opens it on
the dashboard, and GitHub's answer decides. Any teammate can link their machine to their GitHub
account the same way; GitHub must show they have access. After that, machines nobody linked are
hidden by default. The owner can also add or remove admins.

Common attributes
- Action [action_source (CLI, DASHBOARD, LINK) (enum), action_user (string), action_email (string, null), action_id (string)]
- Repository [repo (PERSONAL, ORGANIZATION) (enum), repo_id (digest), repo_remote (string), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, DASHBOARD_SESSION) (enum), session_id (null, string), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT) (enum), agent_id (null), model (null), autonomy_mode (NA)]
- Assessment [assessment (NOT_REQUIRED) (enum), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED) (enum), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Severity [severity_basis (FAILURE_STREAK) (enum), alert_kind (NONE)]
- Telemetry [agent_state (INACTIVE) (enum), known_as (OBSERVED), hook_event (NA)]
- Response failure_state values for this flow (enum): NA, SECURITY, CODE_EXPIRED, CODE_USED, CODE_UNKNOWN, NOT_ADMIN, NO_ACCESS, NOT_GITHUB_REMOTE, UPLOADS_OFF, SIGNATURE_INVALID, REMOTE_NOT_PROVEN, AGENT_REFUSED, NOT_OWNER

#### F02.1 `JOIN_STARTED`
- Action [action_type (JOIN) (enum), action_source (CLI)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA]), resource_ids ([.crane/])]
- Response [response_status (OWNER_UNTRUSTED, ALREADY_JOINED, FORK_TO_ACCEPT, FAILED) (enum), failure_state (NA, SECURITY) (enum), response_statement (string)]
- Assessment [assessment (PENDING), assessment_checks ([SIGNATURES_UNDER_CANDIDATE_OWNER])]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [owner_key (key), last_change_generation (integer), last_change_by (string)]

#### F02.2 `OWNER_KEY_LOOKUP`
Crane asked the service for the GitHub-confirmed owner key.
- Action [action_type (OWNER_LOOKUP) (enum), action_source (CLI)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([NETWORK]), resource_ids ([service host])]
- Response [response_status (MATCH, MISMATCH, NONE_CONFIRMED, UNAVAILABLE) (enum), failure_state (NA, UPLOADS_OFF) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, SUSPICIOUS, UNRESOLVED) (enum), assessment_checks ([GITHUB_CONFIRMATION])]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (INFO for MATCH, LOW for NONE_CONFIRMED or UNAVAILABLE, HIGH for MISMATCH) (enum), severity_basis (OUTCOME)]
- Bindings [third_party ([FOXX_SERVICE]), binding_result (OK, FAILED, TIMEOUT)]
- Telemetry [(common)]
- Other [confirmed_by (GitHub login) (string, null), confirmed_at (timestamp, null), confirmed_key (key, null)]

#### F02.3 `OWNER_KEY_TRUSTED` / `OWNER_KEY_DECLINED`
- Action [action_type (TRUST_OWNER, DECLINE_OWNER) (enum), action_source (CLI)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([TRUST_DIRECTORY, SIGNING_KEY]), resource_ids ([trusted_keys])]
- Response [response_status (OK, DECLINED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (ALLOW for trusted, DENY for declined) (enum), decision_rule (HUMAN_CONFIRMATION) (enum), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM) (enum), severity_basis (OUTCOME)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [owner_key (key), own_key_created (boolean), lookup_result (MATCH, MISMATCH, NONE_CONFIRMED, UNAVAILABLE) (enum)]

#### F02.4 `OWNER_REGISTRATION_REQUESTED`
The owner's machine left a pending owner-key confirmation at the service (`crane admin register`, or at `crane init`).
- Action [action_type (REGISTER_OWNER) (enum), action_source (CLI)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([SIGNING_KEY, UPLOAD_KEY, NETWORK])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, NOT_OWNER, NOT_GITHUB_REMOTE, UPLOADS_OFF, SIGNATURE_INVALID, REMOTE_NOT_PROVEN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common), assessment_checks ([OWNER_SIGNATURE, REMOTE_PROVEN])]
- Decision [(common)]
- Eval [eval (MEASURED), code_lifetime_ms (duration_ms, 600000)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([FOXX_SERVICE]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [owner_key (key), code_digest (digest)]

#### F02.5 `OWNER_KEY_CONFIRMED` / `OWNER_KEY_CONFIRMATION_REFUSED`
A person opened the approval link and GitHub decided.
- Action [action_type (CONFIRM_OWNER) (enum), action_source (LINK, DASHBOARD), action_user (GitHub login)]
- Repository [(common)]
- Session [session_type (DASHBOARD_SESSION)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (OK, REFUSED) (enum), failure_state (NA, NOT_ADMIN, CODE_EXPIRED, CODE_USED, CODE_UNKNOWN, NO_ACCESS) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION) (enum), assessment_checks ([GITHUB_ADMIN_ROLE, CODE_VALID])]
- Decision [decision (ALLOW, DENY) (enum), decision_rule (GITHUB_ADMIN_REQUIRED) (enum), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([GITHUB]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [github_role (ADMIN, WRITE, READ, NONE) (enum), private_repository_retry (boolean)]

#### F02.6 `MACHINE_LINK_REQUESTED`
- Action [action_type (REQUEST_LINK) (enum), action_source (CLI)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([UPLOAD_KEY, NETWORK])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, NOT_GITHUB_REMOTE, UPLOADS_OFF, REMOTE_NOT_PROVEN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common), assessment_checks ([REMOTE_PROVEN])]
- Decision [(common)]
- Eval [eval (MEASURED), code_lifetime_ms (duration_ms)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([FOXX_SERVICE]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [machine_key (key), code_digest (digest)]

#### F02.7 `MACHINE_LINKED` / `MACHINE_LINK_REFUSED`
- Action [action_type (CONFIRM_LINK) (enum), action_source (LINK, DASHBOARD), action_user (GitHub login)]
- Repository [(common)]
- Session [session_type (DASHBOARD_SESSION)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (OK, REFUSED) (enum), failure_state (NA, NO_ACCESS, CODE_EXPIRED, CODE_USED, CODE_UNKNOWN) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION), assessment_checks ([GITHUB_READ_ACCESS, CODE_VALID])]
- Decision [decision (ALLOW, DENY), decision_rule (GITHUB_ACCESS_REQUIRED), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), linked_members_after (integer), hidden_members_after (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([GITHUB]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [machine_key (key), linked_to (GitHub login) (string, null), github_role (ADMIN, WRITE, READ, NONE) (enum)]

#### F02.8 `ADMIN_ADDED` / `ADMIN_REMOVED`
- Action [action_type (ADD_ADMIN, REMOVE_ADMIN) (enum), action_source (CLI)]
- Repository [(common), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA, SIGNING_KEY])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, NOT_OWNER, AGENT_REFUSED, SECURITY) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), admin_list_version (integer), admins_count (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [admin_key (key), admin_name (string)]

#### F02.9 `MACHINE_FIRST_SEEN`
The service received the first upload from a machine for this repository.
- Action [action_type (FIRST_UPLOAD) (enum), action_source (SERVICE), action_user (Git user.name of the machine)]
- Repository [(common)]
- Session [session_type (NOT_IN_SESSION)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common), assessment_checks ([REMOTE_PROVEN])]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (FIXED)]
- Bindings [third_party ([MONGODB])]
- Telemetry [(common)]
- Other [machine_key (key), linked (false) (boolean)]

---

### F03. An agent session starts

When an agent starts, Crane finds the repository, checks the signed rules, and finds or creates a
session with its own identity. It records whether the provider supplied its own session id or Crane
generated one. The session gets an autonomy budget (credits, see F27) for a limited time, and Crane
notes which files were already modified so later changes can be attributed (F07). The agent
receives the rules, the protected and required blocks, and any context and zone guidance.

Common attributes
- Action [action_type (SESSION_START) (enum), action_source (HOOK) (enum), action_user (Git user.name) (string), action_email (string, null), action_id (hook invocation id) (string)]
- Repository [repo (PERSONAL, ORGANIZATION, UNKNOWN) (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (AGENT_SESSION) (enum), session_id (ses_…) (string), provider_session_id (string, null), id_provenance (PROVIDER, ENVIRONMENT, GENERATED) (enum), task_id (null), external_task_id (string, null)]
- Agent [agent_type (CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (agt_…) (string), model (string, null), autonomy_mode (OBSERVE, ASSISTED, DELEGATED, AUTONOMOUS) (enum)]
- Assessment [assessment (NOT_REQUIRED) (enum), assessment_checks ([INTEGRITY])]
- Decision [decision (NOT_REQUIRED) (enum), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Severity [severity_basis (OUTCOME) (enum), alert_kind (NONE)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]) (list of enums), binding_result (NA)]
- Telemetry [agent_state (ACTIVE) (enum), known_as (OBSERVED when provider_session_id is present, else INFERRED) (enum), hook_event (SESSION_START) (enum)]

#### F03.1 `SESSION_CREATED`
First time this session is seen.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NOT_CONSUMED])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (FIXED)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [environment (LOCAL, CI) (enum), cwd (string, redacted to repository-relative)]

#### F03.2 `SESSION_STARTED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CREDITS, FILE_READ]) (list of enums), resource_ids ([context files])]
- Response [response_status (OK, CONTEXT_SUPPLIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), credits_granted (integer), context_files_supplied (integer), latency_ms (duration_ms)]
- Severity [severity_status (INFO), severity_basis (FIXED)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [start_source (STARTUP, RESUME, COMPACT, CLEAR) (enum)]

#### F03.3 `WORKTREE_BASELINE_RECORDED`
- Action [action_type (BASELINE) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([GIT]), resource_ids ([already modified files])]
- Response [response_status (OK, SKIPPED) (enum), failure_state (NA, GIT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), modified_files_at_start (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other []

#### F03.4 `SESSION_CONTEXT_SUPPLIED`
- Action [action_type (SUPPLY_CONTEXT) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([FILE_READ, SELECTION]), resource_ids ([policy context files, selection ids, zone ids])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), policies (integer), selections (integer), context_packs (integer), context_bytes (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [context_packs ([{id, version}]) (list)]

#### F03.5 `SESSION_START_UNVERIFIED`
The rules failed verification at start; the session starts but every change will be denied.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CRANE_METADATA])]
- Response [response_status (DEGRADED_START) (enum), failure_state (VERIFICATION_FAILED) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION), assessment_checks ([INTEGRITY])]
- Decision [decision (DENY), decision_rule (FAIL_CLOSED), decision_reasons (list of findings), enforcement (PREVENTED)]
- Eval [eval (MEASURED), blocking_findings (integer)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME)]
- Bindings [(common)]
- Telemetry [agent_state (ERROR)]
- Other [finding_codes (list of enums, see F12)]

---

### F04. A prompt is submitted

Each prompt opens a new task inside the session, optionally linked to an external tracker task.
Crane screens the prompt for phrases typical of instruction attacks; a match is recorded and alerted
but never changes decisions. The prompt text is never stored, only its digest.

Common attributes
- Action [action_type (PROMPT) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (hook invocation id)]
- Repository [(as F03)]
- Session [session_type (AGENT_SESSION), session_id (string), provider_session_id (string, null), id_provenance (enum), task_id (tsk_…) (string), external_task_id (string, null)]
- Agent [(as F03)]
- Resource [resource ([NOT_CONSUMED]), resource_ids ([]), resource_classification ([NONE])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED) (enum), known_as (OBSERVED), hook_event (PROMPT_SUBMIT)]

#### F04.1 `TASK_STARTED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [(common)]
- Eval [eval (MEASURED), prompt_length (integer), prompts_in_session (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [prompt_digest (digest), task_name (string, null), previous_task_status (RUNNING, COMPLETED, BLOCKED, ENDED, NA) (enum)]

#### F04.2 `PROMPT_SUSPICIOUS`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (RECORDED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([PROMPT_SCREEN])]
- Decision [(common)]
- Eval [eval (MEASURED), matched_phrases (integer)]
- Severity [severity_status (MEDIUM), severity_basis (FIXED), alert_kind (PROMPT_INJECTION)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [phrase_classes ([IGNORE_INSTRUCTIONS, DISABLE_CRANE, REVEAL_SYSTEM_PROMPT, OTHER]) (list of enums), prompt_digest (digest)]

---
### F05. Before each tool call: the decision

Before the agent's tool runs, Crane works out what it would do (read, edit, run a command, or
something else), checks the rules are intact, and applies, in order: the session's safety state,
protection of Crane's own files, shell command analysis, the script pre-check, an in-memory
simulation of file edits against protected blocks and markers, zone and flow limits, autonomy mode,
and the credit budget. The result is allow, ask a human, or deny. Credits are reserved for allowed
actions (F27). Attempts against Crane itself count towards quarantine, and the agent is told the
reason for any refusal. Every F05 event carries the same tool-call id as its F06 verification.

Common attributes
- Action [action_type (TOOL_CALL) (enum), action_source (HOOK) (enum), action_user (string), action_email (string, null), action_id (tool-call id) (string), action_tool (READ, WRITE, EDIT, MULTI_EDIT, NOTEBOOK_EDIT, APPLY_PATCH, BASH, POWERSHELL, WEB_FETCH, WEB_SEARCH, GLOB, GREP, LS, TASK, AGENT, MCP, OTHER) (enum), action_operation (READ, WRITE, EXECUTE, OTHER) (enum), action_input_digest (digest)]
- Repository [repo (PERSONAL, ORGANIZATION, UNKNOWN), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (AGENT_SESSION), session_id (string), provider_session_id (string, null), id_provenance (enum), task_id (string), external_task_id (string, null)]
- Agent [agent_type (CLAUDE_CODE, CODEX, GENERIC, MULTI_AGENT) (enum), agent_id (string), model (string, null), autonomy_mode (OBSERVE, ASSISTED, DELEGATED, AUTONOMOUS) (enum)]
- Resource [resource (list of enums from 1.2), resource_ids (paths, selection ids, hosts, redacted command) (list), resource_classification (list of enums)]
- Assessment [assessment_checks ([INTEGRITY, SAFETY_STATE, METADATA_PROTECTION, SHELL_ANALYSIS, SCRIPT_SCAN, SIMULATION, MARKERS, ZONE, MODE, CREDITS]) (list of enums, the checks that ran)]
- Eval [eval (MEASURED), latency_ms (duration_ms), credits_cost (integer), credits_available (integer), simulation_files (integer)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED) (enum), known_as (OBSERVED when the provider gave a tool-call id, else INFERRED), hook_event (PRE_TOOL_USE, PERMISSION_REQUEST) (enum)]
- Other [zones (zone:ID, flow:ID) (list of strings), policies (list of strings), selections (list of strings), network_destinations (list of strings)]

Decision rules used in F05 (`decision_rule`, enum):
`ALLOWED`, `MODE_ASSISTED`, `MODE_OBSERVE`, `SAFETY_DEGRADED`, `SAFETY_QUARANTINED`,
`INTEGRITY_FAILED`, `ZONE_CONFIG_UNRESOLVED`, `ZONE_OBSERVE`, `ZONE_ASSISTED`,
`ZONE_RESTRICTED`, `PRESERVED_SELECTION`, `MARKER_CHANGED`, `MARKER_FORGED`,
`UNREGISTERED_MARKER`, `SELECTION_FILE_DELETE`, `EDIT_NOT_SIMULATABLE`, `WRITE_NAMES_NO_FILE`,
`COMMAND_NOT_STATED`, `SHELL_MODIFIES_PRESERVED_FILE`, `CRANE_METADATA_WRITE`,
`TRUST_DIRECTORY_ACCESS`, `HOOK_SETTINGS_WRITE`, `SHELL_NAMES_METADATA`,
`TOOL_NAMES_METADATA`, `GOVERNANCE_COMMAND`, `SCRIPT_NAMES_METADATA`,
`SCRIPT_MODIFIES_PRESERVED_FILE`, `SCRIPT_DOWNLOAD_EXECUTE`, `SCRIPT_DECODE_EXECUTE`,
`ENCODED_POWERSHELL`, `SCRIPT_SECRETS_NETWORK`, `SCRIPT_HISTORY_REWRITE`,
`SCRIPT_TREE_DELETION`, `SCRIPT_DOWNLOAD_EVALUATE`, `CREDITS_EXHAUSTED`, `CREDITS_SHORT`,
`HOOK_ERROR`.

#### F05.1 `TOOL_CALL_ALLOWED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (ALLOWED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT) (enum), (common)]
- Decision [decision (ALLOW), decision_rule (ALLOWED), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), guidance_supplied (boolean)]

#### F05.2 `TOOL_CALL_APPROVAL_REQUIRED`
A human must approve before the tool runs.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (ASK) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (VIOLATION, PENDING) (enum), (common)]
- Decision [decision (REQUIRE_APPROVAL), decision_rule (MODE_ASSISTED, SAFETY_DEGRADED, ZONE_ASSISTED, ZONE_RESTRICTED, CREDITS_EXHAUSTED, CREDITS_SHORT, SCRIPT_SECRETS_NETWORK, SCRIPT_HISTORY_REWRITE, SCRIPT_TREE_DELETION, SCRIPT_DOWNLOAD_EVALUATE) (enum), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (LOW), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

#### F05.3 `TOOL_CALL_DENIED_RULE`
Denied by a governance rule (not an attack on Crane).
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource_classification ([PROTECTED, ZONED, NONE])]
- Response [response_status (DENIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (VIOLATION) (enum), (common)]
- Decision [decision (DENY), decision_rule (MODE_OBSERVE, ZONE_OBSERVE, PRESERVED_SELECTION, MARKER_CHANGED, SELECTION_FILE_DELETE, EDIT_NOT_SIMULATABLE, WRITE_NAMES_NO_FILE, COMMAND_NOT_STATED, SHELL_MODIFIES_PRESERVED_FILE) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common)]
- Severity [severity_status (MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), simulated_change (PRESERVED_BYTES_CHANGED, MARKERS_CHANGED, FILE_DELETED, NOT_SIMULATED, NA) (enum)]

#### F05.4 `TOOL_CALL_DENIED_BYPASS_ATTEMPT`
Denied as an attempt on Crane itself; counts towards quarantine.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource ([CRANE_METADATA, TRUST_DIRECTORY, HOOK_SETTINGS, COMMAND]), resource_classification ([METADATA, TRUST])]
- Response [response_status (DENIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS) (enum), (common)]
- Decision [decision (DENY), decision_rule (CRANE_METADATA_WRITE, TRUST_DIRECTORY_ACCESS, HOOK_SETTINGS_WRITE, SHELL_NAMES_METADATA, TOOL_NAMES_METADATA, GOVERNANCE_COMMAND, MARKER_FORGED, UNREGISTERED_MARKER, SELECTION_FILE_DELETE) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common), bypass_attempts_in_session (integer), quarantine_threshold (integer, 3)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (BYPASS_ATTEMPT)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

#### F05.5 `TOOL_CALL_DENIED_QUARANTINE`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (DENIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([SAFETY_STATE])]
- Decision [decision (QUARANTINE), decision_rule (SAFETY_QUARANTINED), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [agent_state (QUARANTINED)]
- Other [(common), safety_reason (string)]

#### F05.6 `TOOL_CALL_DENIED_FAIL_CLOSED`
Denied because Crane could not verify its rules or zone configuration.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (DENIED) (enum), failure_state (VERIFICATION_FAILED, ZONE_CONFIG_UNRESOLVED) (enum), response_statement (string)]
- Assessment [assessment (UNRESOLVED), assessment_checks ([INTEGRITY, ZONE])]
- Decision [decision (DENY), decision_rule (INTEGRITY_FAILED, ZONE_CONFIG_UNRESOLVED) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [agent_state (ERROR)]
- Other [(common), finding_codes (list of enums, see F12)]

#### F05.7 `SCRIPT_BLOCKED`
The script pre-check read a script or inline code and denied it before it ran.
- Action [(common), action_tool (BASH, POWERSHELL), action_operation (EXECUTE)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([COMMAND, SCRIPT]), resource_ids ([script path or inline origin]), resource_classification ([PROTECTED, METADATA, EXTERNAL])]
- Response [response_status (DENIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (VIOLATION, SUSPICIOUS) (enum), assessment_checks ([SCRIPT_SCAN])]
- Decision [decision (DENY), decision_rule (SCRIPT_NAMES_METADATA, SCRIPT_MODIFIES_PRESERVED_FILE, SCRIPT_DOWNLOAD_EXECUTE, SCRIPT_DECODE_EXECUTE, ENCODED_POWERSHELL) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common), script_bytes (integer), scripts_scanned (integer)]
- Severity [severity_status (MEDIUM, HIGH when SCRIPT_NAMES_METADATA) (enum), severity_basis (OUTCOME), alert_kind (NONE, BYPASS_ATTEMPT)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [script_source (FILE, INLINE, PACKAGE_SCRIPT, COMMAND_LINE) (enum), interpreter (PYTHON, NODE, DENO, BUN, BASH, SH, ZSH, PWSH, POWERSHELL, CMD, RUBY, PERL, PHP, DIRECT, NPM, PNPM, YARN) (enum)]

#### F05.8 `SCRIPT_APPROVAL_REQUIRED`
- Action [(common), action_tool (BASH, POWERSHELL), action_operation (EXECUTE)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([COMMAND, SCRIPT, NETWORK]), resource_ids (list), resource_classification ([EXTERNAL, NONE])]
- Response [response_status (ASK) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([SCRIPT_SCAN])]
- Decision [decision (REQUIRE_APPROVAL), decision_rule (SCRIPT_SECRETS_NETWORK, SCRIPT_HISTORY_REWRITE, SCRIPT_TREE_DELETION, SCRIPT_DOWNLOAD_EVALUATE) (enum), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common), script_bytes (integer)]
- Severity [severity_status (LOW), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [script_source (FILE, INLINE, PACKAGE_SCRIPT) (enum), interpreter (enum as F05.7)]

#### F05.9 `SCRIPT_WRITE_APPROVAL_REQUIRED`
The agent is writing a script whose content is risky.
- Action [(common), action_tool (WRITE, EDIT, MULTI_EDIT, APPLY_PATCH), action_operation (WRITE)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([FILE_WRITE, SCRIPT]), resource_ids ([script path])]
- Response [response_status (ASK) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([SCRIPT_SCAN, SIMULATION])]
- Decision [decision (REQUIRE_APPROVAL), decision_rule (SCRIPT_NAMES_METADATA, SCRIPT_MODIFIES_PRESERVED_FILE, SCRIPT_DOWNLOAD_EXECUTE, SCRIPT_DECODE_EXECUTE, SCRIPT_SECRETS_NETWORK, SCRIPT_HISTORY_REWRITE, SCRIPT_TREE_DELETION) (enum), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (LOW), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [script_extension (PY, JS, MJS, CJS, TS, SH, BASH, ZSH, PS1, PSM1, BAT, CMD, RB, PL, PHP) (enum)]

#### F05.10 `SESSION_QUARANTINED_ATTEMPTS`
The bypass-attempt threshold was reached; the session is quarantined from now on.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NOT_CONSUMED])]
- Response [response_status (QUARANTINED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([SAFETY_STATE])]
- Decision [decision (QUARANTINE), decision_rule (SAFETY_QUARANTINED), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), bypass_attempts_in_session (integer), quarantine_threshold (integer)]
- Severity [severity_status (CRITICAL), severity_basis (THRESHOLD), alert_kind (QUARANTINE)]
- Bindings [(common)]
- Telemetry [agent_state (QUARANTINED)]
- Other [safety_reason (string)]

#### F05.11 `ZONE_GUIDANCE_SUPPLIED`
An allowed or approval-required call touched active zones or flows; their guidance went with it.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([FILE_READ, FILE_WRITE]), resource_classification ([ZONED])]
- Response [response_status (CONTEXT_SUPPLIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([ZONE])]
- Decision [decision (ALLOW, REQUIRE_APPROVAL) (enum), decision_rule (ALLOWED, ZONE_ASSISTED, ZONE_RESTRICTED), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), guidance_bytes (integer), zones_touched (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [zones (list), criticality (ROUTINE, SENSITIVE, CRITICAL, RESTRICTED) (enum), ceiling (OBSERVE, ASSISTED, DELEGATED, AUTONOMOUS) (enum)]

#### F05.12 `SUBAGENT_DISPATCHED`
The agent started a sub-agent (Task or Agent tool). Treated as a read; the sub-agent's own tool calls
come through F05 and F06 with `agent_type = MULTI_AGENT`.
- Action [(common), action_tool (TASK, AGENT), action_operation (READ)]
- Repository [(common)]
- Session [(common)]
- Agent [(common), agent_type (CLAUDE_CODE, CODEX)]
- Resource [resource ([TOOL]), resource_ids ([sub-agent type])]
- Response [response_status (ALLOWED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT), assessment_checks ([INTEGRITY])]
- Decision [decision (ALLOW), decision_rule (ALLOWED), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [subagent_type (string, null)]

Credit events emitted during F05 (reserve, short, low, exhausted) are defined in F27.

---

### F06. After each tool call: verification

After the tool ran, Crane pairs it with its decision (or records that there was none), charges or
releases the credits, attributes the files a shell or other tool changed, and re-checks the
protected blocks. A violation by a file edit degrades the session and revokes credits; protected
code changed by a shell or other tool is a confirmed bypass and quarantines the session. The agent
is told to repair or restore.

Common attributes
- Action [action_type (TOOL_CALL_RESULT) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (tool-call id), action_tool (enum as F05), action_operation (READ, WRITE, EXECUTE, OTHER) (enum)]
- Repository [(as F05)]
- Session [(as F05)]
- Agent [(as F05)]
- Resource [resource (list of enums), resource_ids (written files, or files changed during a shell command), resource_classification (list)]
- Assessment [assessment_checks ([PAIRING, SELECTIONS_RECHECK, WORKTREE_DIFF]) (list of enums)]
- Eval [eval (MEASURED), tool_duration_ms (duration_ms), latency_ms (duration_ms), files_checked (integer), model_tokens (number, tokens), model_input_tokens (number), model_output_tokens (number), cache_read_tokens (number), cache_creation_tokens (number), turn_tokens (number), turn_tool_calls (integer)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED), known_as (OBSERVED, INFERRED), hook_event (POST_TOOL_USE)]
- Other [tool_failed (boolean, null), scan_mode (PATHS, FULL, NONE) (enum)]

#### F06.1 `TOOL_CALL_VERIFIED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SUCCESS, FAILURE, PARTIAL, UNKNOWN) (enum, the tool's execution outcome), failure_state (NA, TOOL_FAILED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT) (enum), (common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

#### F06.2 `TOOL_CALL_VERIFICATION_UNRESOLVED`
Compliance could not be established (a selection could not be resolved after the call).
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource_classification ([PROTECTED, TARGETED])]
- Response [response_status (SUCCESS, FAILURE, UNKNOWN) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (UNRESOLVED), (common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), unresolved_selections (list of strings)]

#### F06.3 `TOOL_CALL_MISSING_DECISION`
A tool ran without a recorded decision (hook missing, failed, or bypassed).
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SUCCESS, FAILURE, UNKNOWN) (enum), failure_state (NO_DECISION) (enum), response_statement (string)]
- Assessment [assessment (UNRESOLVED), assessment_checks ([PAIRING])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (COVERAGE_GAP)]
- Eval [(common)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (COVERAGE_GAP)]
- Bindings [(common)]
- Telemetry [(common), known_as (UNOBSERVED)]
- Other [(common)]

#### F06.4 `VIOLATION_AFTER_EFFECT`
A file-edit tool's result broke a rule; the session is degraded and credits revoked.
- Action [(common), action_operation (WRITE)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource_classification ([PROTECTED, TARGETED])]
- Response [response_status (SUCCESS) (enum), failure_state (POLICY_VIOLATED) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION), (common)]
- Decision [decision (NOT_REQUIRED), decision_rule (PRESERVED_SELECTION, MARKER_CHANGED, UNREGISTERED_MARKER) (enum), decision_reasons (list), enforcement (DETECTED_AFTER_EFFECT)]
- Eval [(common), credits_revoked (integer, 10 by default), violations_in_session (integer)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (VIOLATION)]
- Bindings [(common)]
- Telemetry [agent_state (DEGRADED)]
- Other [(common), violated_selections (list of strings), repair_instructions_sent (true) (boolean)]

#### F06.5 `BYPASS_CONFIRMED`
A shell or other tool changed protected code around the pre-check; the session is quarantined.
- Action [(common), action_operation (EXECUTE, OTHER)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource_classification ([PROTECTED])]
- Response [response_status (SUCCESS, FAILURE, UNKNOWN) (enum), failure_state (POLICY_VIOLATED) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION), (common)]
- Decision [decision (NOT_REQUIRED), decision_rule (PRESERVED_SELECTION, MARKER_CHANGED) (enum), decision_reasons (list), enforcement (BYPASS_CONFIRMED)]
- Eval [(common), credits_revoked (integer)]
- Severity [severity_status (CRITICAL), severity_basis (OUTCOME), alert_kind (BYPASS_CONFIRMED)]
- Bindings [(common)]
- Telemetry [agent_state (QUARANTINED)]
- Other [(common), violated_selections (list of strings), restore_instructions_sent (true) (boolean)]

#### F06.6 `SHELL_EFFECT_ATTRIBUTED`
Files that changed while a shell or other tool ran are attributed to that call (used by F07).
- Action [(common), action_operation (EXECUTE, OTHER)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([FILE_WRITE, FILE_DELETE, GIT]), resource_ids (changed files)]
- Response [response_status (OK, SKIPPED) (enum), failure_state (NA, GIT) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([WORKTREE_DIFF])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), files_changed (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other []

Credit events emitted during F06 (consume, release, revoke) are defined in F27.

---

### F07. Stop: the agent says it is done

Crane first compares the work tree with the session's starting state and reports, once per file,
changes nobody recorded. It then checks every policy completely: protected blocks unchanged,
required blocks changed the required way, every block found. A failure refuses the first stop and
tells the agent what to repair; a second stop is let through, recorded as unverified.

Common attributes
- Action [action_type (STOP) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (hook invocation id)]
- Repository [(as F05)]
- Session [(as F05)]
- Agent [(as F05)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED), known_as (OBSERVED), hook_event (STOP)]

#### F07.1 `UNATTRIBUTED_CHANGES_FOUND`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([FILE_WRITE, FILE_DELETE]), resource_ids (files), resource_classification ([UNATTRIBUTED])]
- Response [response_status (RECORDED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([WORKTREE_DIFF, ATTRIBUTION])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (COVERAGE_GAP)]
- Eval [eval (MEASURED), unattributed_files (integer), attributed_files (integer), modified_files_at_start (integer)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (UNATTRIBUTED_CHANGE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [likely_cause (UNHOOKED_TOOL, EXTERNAL_PROCESS, CONCURRENT_HUMAN_EDIT, UNKNOWN) (enum, UNKNOWN unless the person classifies it)]

#### F07.2 `TASK_VERIFIED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION]), resource_ids (selection ids)]
- Response [response_status (COMPLETED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT), assessment_checks ([PRESERVES_UNCHANGED, TARGETS_CHANGED, SELECTIONS_RESOLVED])]
- Decision [decision (ALLOW), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), policies_passed (integer), policies_failed (0), tool_calls_in_task (integer), task_duration_ms (duration_ms)]
- Severity [severity_status (INFO), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [task_status (COMPLETED) (enum)]

#### F07.3 `TASK_VERIFICATION_FAILED`
The first failing stop is refused; the agent gets the failures and repair instructions.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION]), resource_ids (failing selection ids), resource_classification ([PROTECTED, TARGETED])]
- Response [response_status (BLOCKED) (enum), failure_state (PRESERVE_CHANGED, TARGET_NOT_CHANGED, TARGET_WRONG_KIND, SELECTION_UNRESOLVED, BLOCKING_FINDING) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION, UNRESOLVED) (enum), assessment_checks ([PRESERVES_UNCHANGED, TARGETS_CHANGED, SELECTIONS_RESOLVED])]
- Decision [decision (DENY), decision_rule (STOP_BLOCKED) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), policies_passed (integer), policies_failed (integer)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (VERIFICATION_FAILED)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [task_status (BLOCKED) (enum), repair_instructions_sent (true)]

#### F07.4 `TASK_ENDED_UNVERIFIED`
A second stop after a refusal is let through so the agent cannot loop.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION])]
- Response [response_status (ENDED) (enum), failure_state (PRESERVE_CHANGED, TARGET_NOT_CHANGED, TARGET_WRONG_KIND, SELECTION_UNRESOLVED, BLOCKING_FINDING) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION, UNRESOLVED), assessment_checks (as F07.3)]
- Decision [decision (ALLOW), decision_rule (STOP_LOOP_GUARD) (enum), decision_reasons (list), enforcement (DETECTED_AFTER_EFFECT)]
- Eval [eval (MEASURED), policies_passed (integer), policies_failed (integer)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (VERIFICATION_FAILED)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [task_status (ENDED) (enum)]

---

### F08. The session ends

Crane releases credits reserved for actions that never reported back (recording each as a gap),
expires unused credits, closes the session and task, and signs a digest of all the session's events
with the machine's upload key, so the session's evidence can later be proven unaltered.

Common attributes
- Action [action_type (SESSION_END) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (hook invocation id)]
- Repository [(as F05)]
- Session [(as F05)]
- Agent [(as F05)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ENDED) (enum), known_as (OBSERVED), hook_event (SESSION_END)]

#### F08.1 `TOOL_CALL_UNSETTLED`
- Action [(common), action_id (tool-call id of the unsettled call)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([CREDITS])]
- Response [response_status (RELEASED) (enum), failure_state (NO_POST_TOOL_REPORT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (COVERAGE_GAP)]
- Eval [eval (MEASURED), credits_released (integer)]
- Severity [severity_status (MEDIUM), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), known_as (UNOBSERVED)]
- Other [tool (enum as F05)]

#### F08.2 `SESSION_CLOSED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NOT_CONSUMED])]
- Response [response_status (ENDED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), session_duration_ms (duration_ms), tasks (integer), tool_calls (integer), denied (integer), approvals (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [end_reason (EXIT, CLEAR, LOGOUT, PROMPT_INPUT_EXIT, OTHER) (enum, as reported by the agent tool), final_safety (ACTIVE, DEGRADED, QUARANTINED) (enum)]

#### F08.3 `SESSION_ATTESTED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([UPLOAD_KEY])]
- Response [response_status (SIGNED, UNSIGNED) (enum), failure_state (NA, KEY_UNAVAILABLE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), events_attested (integer), credits_expired (integer)]
- Severity [severity_status (INFO, MEDIUM when UNSIGNED) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [attestation_digest (digest), signer (INSTALLATION) (enum), signer_key (key), signature (string)]

Credit events emitted during F08 (release, expire) are defined in F27.

---
### F09. Evidence upload

After actions, at stop and session end, after Crane commands, and otherwise every minute (F34), a
background process uploads what is new. It first asks the service how far each record stream got
and re-sends what the service lost. Every request is signed by the machine's upload key, stamped
with the time, and size- and rate-limited. The service checks that the repository is really the one
claimed and that the machine's event history continues exactly where it left off; a gap, rewrite,
or fork raises a critical alert. Routine records expire after the retention period; security
evidence is kept.

Common attributes
- Action [action_type (SYNC) (enum), action_source (CLI, SERVICE) (enum), action_user (Git user.name of the machine) (string), action_email (string, null), action_id (request id) (string)]
- Repository [repo (PERSONAL, ORGANIZATION, UNKNOWN), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION) (enum), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([FOXX_SERVICE, MONGODB]), binding_result (OK, FAILED, TIMEOUT)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [machine_key (key)]

#### F09.1 `SYNC_STARTED`
- Action [(common), action_source (CLI)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NETWORK, UPLOAD_KEY])]
- Response [response_status (STARTED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), pending_events (integer), pending_ledger_entries (integer), pending_audit_entries (integer), pending_alerts (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common), binding_result (NA)]
- Telemetry [(common)]
- Other [trigger (POST_TOOL, STOP, SESSION_END, COMMAND, INTERVAL, MANUAL) (enum)]

#### F09.2 `SYNC_COMPLETED`
- Action [(common), action_source (CLI)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NETWORK, DATABASE])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), batches (integer), events_sent (integer), duplicates_skipped (integer), duration_ms (duration_ms)]
- Severity [severity_status (LOW), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common), binding_result (OK)]
- Telemetry [(common)]
- Other [status_report_included (boolean)]

#### F09.3 `SYNC_RESENT`
The service held fewer records than the machine (for example after a restore); the missing records were sent again.
- Action [(common), action_source (CLI)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NETWORK, DATABASE])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), records_resent (integer)]
- Severity [severity_status (MEDIUM), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [streams ([EVENTS, LEDGER, AUDIT, ALERTS]) (list of enums)]

#### F09.4 `SYNC_REFUSED`
- Action [(common), action_source (SERVICE)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NETWORK])]
- Response [response_status (UNAUTHORIZED, RATE_LIMITED, TOO_LARGE, QUOTA_REACHED, STORAGE_FULL, BAD_REQUEST, SERVER_ERROR, UNREACHABLE) (enum), failure_state (SIGNATURE_INVALID, TIMESTAMP_SKEW, MISSING_HEADERS, MACHINE_RATE, ADDRESS_RATE, BATCH_TOO_LARGE, EVENT_QUOTA, REPOSITORY_QUOTA, DATABASE_SIZE, MALFORMED_BATCH, STORE_FAILED, NETWORK) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (DENY), decision_rule (UPLOAD_LIMIT, UPLOAD_AUTH) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), retry_after_s (integer, null)]
- Severity [severity_status (MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common), binding_result (FAILED, TIMEOUT)]
- Telemetry [(common), agent_state (ERROR)]
- Other [request_id (string)]

#### F09.5 `REMOTE_VERIFIED`
- Action [(common), action_source (SERVICE)]
- Repository [(common), repo_remote_verified (true, false) (boolean)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (VERIFIED, NOT_PROVEN) (enum), failure_state (NA, REMOTE_NOT_PROVEN) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, UNRESOLVED), assessment_checks ([ROOT_COMMITS_AND_REMOTE])]
- Decision [(common)]
- Eval [(common)]
- Severity [severity_status (INFO, MEDIUM) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other []

#### F09.6 `EVIDENCE_CHAIN_BROKEN`
- Action [(common), action_source (SERVICE)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (STORED_WITH_ALERT) (enum), failure_state (GAP, REWRITE, FORK) (enum), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([CHAIN_CONTINUITY])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (DETECTED_AFTER_EFFECT)]
- Eval [eval (MEASURED), head_sequence (integer), received_sequence (integer), missing_events (integer)]
- Severity [severity_status (CRITICAL), severity_basis (FIXED), alert_kind (EVIDENCE_CHAIN_BROKEN)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [head_digest (digest), received_previous_digest (digest)]

#### F09.7 `ROUTINE_RECORDS_EXPIRED`
- Action [action_type (EXPIRE) (enum), action_source (SERVICE), action_user (null)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([DATABASE])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), events_expired (integer), retention_days (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [third_party ([MONGODB])]
- Telemetry [(common), known_as (INFERRED)]
- Other [expired_severities ([INFO, LOW, MEDIUM]) (list of enums)]

---

### F10. A session stops reporting

A session that never ended, has no signed end, and that no hook has seen for the stale timeout
(120 minutes by default) is treated as stale: the agent was stopped abruptly, its hooks were
removed, or its evidence is not arriving. The dashboard derives this on every read.

Common attributes
- Action [action_type (STALENESS_CHECK) (enum), action_source (DASHBOARD), action_user (null), action_email (null), action_id (null)]
- Repository [(as F05)]
- Session [session_type (AGENT_SESSION), session_id (string), provider_session_id (string, null), id_provenance (enum), task_id (string, null), external_task_id (string, null)]
- Agent [agent_type (CLAUDE_CODE, CODEX, GENERIC), agent_id (string), model (string, null), autonomy_mode (enum)]
- Resource [resource ([NOT_CONSUMED]), resource_ids ([]), resource_classification ([NONE])]
- Assessment [assessment (UNRESOLVED), assessment_checks ([LAST_SEEN, ATTESTATION_PRESENT, ENDED])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (COVERAGE_GAP)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [known_as (INFERRED), hook_event (NA)]

#### F10.1 `SESSION_STALE`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (STALE) (enum), failure_state (NO_ACTIVITY) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), minutes_since_last_seen (integer), stale_after_minutes (integer, 120)]
- Severity [severity_status (HIGH), severity_basis (THRESHOLD), alert_kind (STALE_SESSION)]
- Bindings [(common)]
- Telemetry [agent_state (STALE), (common)]
- Other [last_seen_at (timestamp)]

#### F10.2 `SESSION_REVIVED`
A stale session reported again (a later hook arrived).
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (ACTIVE) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([LAST_SEEN])]
- Decision [(common), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), minutes_silent (integer)]
- Severity [severity_status (LOW), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [agent_state (ACTIVE), (common)]
- Other []

---

### F11. Crane fails during a hook

If Crane hits an error while deciding or verifying, it records the failure and denies the action:
it never lets an action through because it could not check it.

#### F11.1 `HOOK_FAILED`
- Action [action_type (HOOK) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (hook invocation id)]
- Repository [repo (enum), repo_id (digest, null), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (AGENT_SESSION), session_id (string, null), provider_session_id (string, null), id_provenance (enum), task_id (string, null), external_task_id (string, null)]
- Agent [agent_type (CLAUDE_CODE, CODEX, GENERIC), agent_id (string, null), model (string, null), autonomy_mode (enum)]
- Resource [resource ([NOT_CONSUMED]), resource_ids ([]), resource_classification ([NONE])]
- Response [response_status (ERROR) (enum), failure_state (NOT_A_REPOSITORY, NOT_INITIALIZED, PAYLOAD_UNREADABLE, STORE_UNREADABLE, LOCK_TIMEOUT, TRUST_UNAVAILABLE, INTERNAL) (enum), response_statement (string)]
- Assessment [assessment (UNRESOLVED), assessment_checks ([])]
- Decision [decision (ERROR), decision_rule (HOOK_ERROR), decision_reasons ([error message]), enforcement (PREVENTED)]
- Eval [eval (MEASURED), latency_ms (duration_ms)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ERROR), known_as (OBSERVED), hook_event (SESSION_START, PROMPT_SUBMIT, PRE_TOOL_USE, PERMISSION_REQUEST, POST_TOOL_USE, STOP, SESSION_END) (enum)]
- Other []

---

### F12. The rules are tampered with, rolled back, or fork

On every check Crane verifies the signed rules: signatures and trusted signers, the digest of every
rules file, the audit and checkpoint chains, the admin list version, and this machine's memory of
the newest rules version. A serious finding blocks all agent changes until a person repairs it.

Common attributes
- Action [action_type (VERIFY_RULES) (enum), action_source (HOOK, CLI, EXTENSION, CI) (enum), action_user (string), action_email (string, null), action_id (string)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum), session_id (string, null), provider_session_id (null, string), id_provenance (enum), task_id (null, string), external_task_id (null, string)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (string, null), model (string, null), autonomy_mode (enum)]
- Resource [resource ([CRANE_METADATA, TRUST_DIRECTORY]), resource_ids (affected files), resource_classification ([METADATA, TRUST])]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [known_as (OBSERVED), hook_event (NA, SESSION_START, PRE_TOOL_USE) (enum)]

Finding codes (`finding_code`, enum): `REGISTRY_MISSING`, `REGISTRY_WRONG_REPOSITORY`,
`TRUST_UNAVAILABLE`, `UNTRUSTED_OWNER`, `ADMINS_TAMPERED`, `ADMINS_ROLLBACK`, `NO_TRUSTED_KEY`,
`UNTRUSTED_SIGNER`, `MANIFEST_TAMPERED`, `MANIFEST_SIGNATURE_INVALID`,
`GOVERNANCE_FILE_TAMPERED`, `GOVERNANCE_YAML_CHANGED`, `GOVERNANCE_YAML_UNSIGNED`,
`GOVERNANCE_YAML_INVALID`, `SELECTION_DUPLICATED`, `SELECTION_RECORD_INVALID`, `MAP_MISSING`,
`MAP_TAMPERED`, `CHECKPOINTS_TAMPERED`, `CHECKPOINT_COMMIT_MISSING`, `AUDIT_TAMPERED`,
`ANCHOR_UNREADABLE`, `ANCHOR_INVALID`, `ANCHOR_FIRST_USE`, `REGISTRY_ROLLBACK`, `REGISTRY_FORK`,
`CONFLICTED_RULES`, `POLICY_SYNTAX`, `POLICY_DUPLICATE`, `POLICY_OUTSIDE_FOLDER`,
`POLICY_UNREGISTERED_SELECTION`.

#### F12.1 `RULES_INTEGRITY_FAILED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (FAILED) (enum), failure_state (SECURITY, SYNTAX, CONFLICT, GIT) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION), assessment_checks ([SIGNATURES, SIGNERS, FILE_DIGESTS, CHAINS, ADMIN_EPOCH, ROLLBACK_ANCHOR])]
- Decision [decision (DENY for agent changes), decision_rule (INTEGRITY_FAILED), decision_reasons (finding messages), enforcement (PREVENTED)]
- Eval [eval (MEASURED), blocking_findings (integer), findings (integer), anchor_generation (integer, null)]
- Severity [severity_status (HIGH, CRITICAL) (enum, CRITICAL for tampering, rollback, fork, untrusted signer), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [agent_state (INACTIVE, ERROR)]
- Other [finding_codes (list of enums)]

#### F12.2 `RULES_INTEGRITY_WARNING`
Non-blocking findings (for example first use on this machine, unsigned zones file from an older version).
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (WARNING) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT), assessment_checks (as F12.1)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), findings (integer)]
- Severity [severity_status (LOW, MEDIUM) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), agent_state (INACTIVE, ACTIVE)]
- Other [finding_codes ([ANCHOR_FIRST_USE, GOVERNANCE_YAML_UNSIGNED, CHECKPOINT_COMMIT_MISSING]) (list of enums)]

#### F12.3 `RULES_INTEGRITY_RESTORED`
A run after a failure verified again.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT), assessment_checks (as F12.1)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), minutes_failing (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), agent_state (INACTIVE, ACTIVE)]
- Other [repaired_by (RESTORE, SIGN, ACCEPT_FORK, TRUST_OWNER) (enum, null)]

---

### F13. A Crane command runs

Every Crane command in a governed repository is recorded with its outcome, duration, and whether an
agent tried to run it; secrets in arguments are removed. Commands that change rules also add an
entry to the signed audit trail.

Common attributes
- Action [action_type (COMMAND) (enum), action_source (CLI, EXTENSION, CI, AGENT) (enum), action_user (string), action_email (string, null), action_id (command id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum), session_id (string, null), provider_session_id (string, null), id_provenance (enum), task_id (string, null), external_task_id (string, null)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (string, null), model (null), autonomy_mode (NA)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE, ACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [command (INIT, CHECKPOINT, PROTECT, TARGET, SET_DEFAULT, PARSE, VALIDATE, TEST, ADD_CONTEXT, VIEW_CONTEXT, CREATE_POLICY, MOVE_SELECTION, POLICY_STATUS, REPO, GITHUB, TASK, SESSION, AGENT, INTEGRATE, ADMIN, GOVERNANCE) (enum), arguments (redacted) (list of strings)]

#### F13.1 `COMMAND_EXECUTED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource (depends on the command) (list of enums), resource_ids (list), resource_classification (list)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, CHECKPOINT, POLICY, SYNTAX, GIT, SECURITY, NOT_ADMIN, VERIFICATION_FAILED, USAGE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), duration_ms (duration_ms), exit_code (integer)]
- Severity [severity_status (INFO, LOW, MEDIUM, HIGH) (enum), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), read_only (boolean)]

#### F13.2 `COMMAND_REFUSED_FOR_AGENT`
- Action [(common), action_source (AGENT)]
- Repository [(common)]
- Session [(common), session_type (AGENT_SESSION)]
- Agent [(common), agent_type (CLAUDE_CODE, CODEX, GENERIC)]
- Resource [resource ([CRANE_METADATA, COMMAND])]
- Response [response_status (REFUSED) (enum), failure_state (AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([AGENT_ENVIRONMENT])]
- Decision [decision (DENY), decision_rule (HUMAN_ONLY) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), duration_ms (duration_ms)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), agent_state (ACTIVE)]
- Other [(common), agent_marker (CLAUDECODE, CODEX, OTHER) (enum)]

#### F13.3 `AUDIT_ENTRY_APPENDED`
- Action [(common), action_source (CLI, EXTENSION)]
- Repository [(common), registry_generation (integer, the new generation)]
- Session [(common), session_type (NOT_IN_SESSION)]
- Agent [(common), agent_type (NOT_AN_AGENT)]
- Resource [resource ([CRANE_METADATA]), resource_ids ([.crane/audit.jsonl])]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), audit_sequence (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [audit_action (GOVERNANCE_INITIALIZED, CHECKPOINT_CREATED, SELECTION_CREATED, SELECTION_RELOCATED, SELECTION_POLICY_CHANGED, POLICY_CREATED, DEFAULT_POLICY_SET, DEFAULT_CHECKPOINT_SET, CONTEXT_ADDED, GOVERNANCE_ADMIN_ADDED, GOVERNANCE_ADMIN_REMOVED, GOVERNANCE_SIGNED) (enum), audit_head (digest)]

---

### F14. Alerts are delivered

When an event raises an alert (its `alert_kind` is not `NONE`), Crane records it with its trigger,
the value seen, the threshold, and the action taken; repeats within a cooldown are suppressed; it is
delivered to the configured channels (Slack, WhatsApp, the linked Jira issue) and the result is
uploaded. Service-raised (`EVIDENCE_CHAIN_BROKEN`) and dashboard-derived (`STALE_SESSION`) alerts
are not delivered to channels.

Alert kinds (`alert_kind`, enum): `NONE`, `LOW_CREDITS`, `BYPASS_ATTEMPT`, `BYPASS_CONFIRMED`,
`VIOLATION`, `QUARANTINE`, `COVERAGE_GAP`, `VERIFICATION_FAILED`, `PROMPT_INJECTION`,
`UNATTRIBUTED_CHANGE`, `EVIDENCE_CHAIN_BROKEN`, `STALE_SESSION`.

Common attributes
- Action [action_type (ALERT) (enum), action_source (HOOK, SERVICE, DASHBOARD) (enum), action_user (string, null), action_email (null), action_id (alert id)]
- Repository [(as the triggering event)]
- Session [(as the triggering event)]
- Agent [(as the triggering event)]
- Resource [resource ([NOT_CONSUMED]), resource_ids ([]), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Telemetry [agent_state (as the triggering event), known_as (OBSERVED), hook_event (as the triggering event)]
- Other [triggering_event_id (string), alert_key (string)]

#### F14.1 `ALERT_RAISED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OPEN) (enum), failure_state (NA), response_statement (action taken, string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), observed_value (number, null), threshold (number, null)]
- Severity [severity_status (MEDIUM, HIGH, CRITICAL) (enum, from the triggering event), severity_basis (OUTCOME), alert_kind (enum)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [(common), alert_title (string), trigger (string)]

#### F14.2 `ALERT_SUPPRESSED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SUPPRESSED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), cooldown_seconds (integer), seconds_since_last (integer)]
- Severity [severity_status (LOW), severity_basis (FIXED), alert_kind (enum)]
- Bindings [third_party ([NONE])]
- Telemetry [(common)]
- Other [(common)]

#### F14.3 `ALERT_DELIVERED` / `ALERT_DELIVERY_FAILED`
One event per channel.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NETWORK])]
- Response [response_status (DELIVERED, FAILED, DRY_RUN) (enum), failure_state (NA, HTTP_ERROR, UNREACHABLE, NOT_CONFIGURED, BELOW_MIN_SEVERITY) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), http_status (integer, null)]
- Severity [severity_status (INFO for delivered, MEDIUM for failed) (enum), severity_basis (OUTCOME), alert_kind (enum)]
- Bindings [third_party ([SLACK, WHATSAPP, JIRA]), binding_result (OK, FAILED, TIMEOUT, SKIPPED)]
- Telemetry [(common)]
- Other [(common), channel (SLACK, WHATSAPP, JIRA) (enum), jira_issue (string, null)]

---

### F15. Installing, checking, and removing agent hooks

A person installs Crane's hooks into Claude Code's or Codex's settings (additive and idempotent),
validates them (each lifecycle event registered or missing), or removes them. Removal is for humans
only, and agents are blocked from editing hook settings. Without hooks no agent action is governed.

Common attributes
- Action [action_type (HOOKS) (enum), action_source (CLI, EXTENSION, AGENT) (enum), action_user (string), action_email (string, null), action_id (command id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([HOOK_SETTINGS]), resource_ids (settings file paths), resource_classification ([METADATA])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [profile (CLAUDE, CODEX) (enum)]

#### F15.1 `HOOKS_INSTALLED`
- Action [(common), action_source (CLI, EXTENSION)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (INSTALLED, UNCHANGED, FAILED) (enum), failure_state (NA, SETTINGS_UNWRITABLE, SETTINGS_MALFORMED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), events_registered (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), events ([SESSION_START, PROMPT_SUBMIT, PRE_TOOL_USE, PERMISSION_REQUEST, POST_TOOL_USE, STOP, SESSION_END]) (list of enums)]

#### F15.2 `HOOKS_VALIDATED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (VALID, INVALID) (enum), failure_state (NA, EVENTS_MISSING, WRONG_COMMAND, SETTINGS_MALFORMED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION), assessment_checks ([EVENTS_REGISTERED, COMMAND_PATH])]
- Decision [(common)]
- Eval [eval (MEASURED), events_registered (integer), events_missing (integer), warnings (integer)]
- Severity [severity_status (INFO, HIGH) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), missing_events (list of enums)]

#### F15.3 `HOOKS_REMOVED` / `HOOKS_REMOVAL_REFUSED`
- Action [(common), action_source (CLI, AGENT)]
- Repository [(common)]
- Session [(common), session_type (NOT_IN_SESSION, AGENT_SESSION)]
- Agent [(common), agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX)]
- Resource [(common)]
- Response [response_status (REMOVED, REFUSED) (enum), failure_state (NA, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED, DENY) (enum), decision_rule (null, HUMAN_ONLY), decision_reasons (list), enforcement (NOT_APPLICABLE, PREVENTED)]
- Eval [eval (MEASURED), entries_removed (integer)]
- Severity [severity_status (MEDIUM for removed, HIGH for refused) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

---

### F16. The agent tool asks the person for permission

Some agent tools ask the person before running a tool. Crane hooks that moment, makes the same
decision as F05, and answers the dialog: refused outright when Crane would deny, otherwise left to
the person. Crane does not see the person's click; it learns the outcome from whether the action
later reports back (F06).

Common attributes
- Action [action_type (PERMISSION) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (tool-call id), action_tool (enum as F05)]
- Repository [(as F05)]
- Session [(as F05)]
- Agent [(as F05)]
- Resource [(as F05)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED), known_as (OBSERVED), hook_event (PERMISSION_REQUEST)]

#### F16.1 `PERMISSION_PRE_DENIED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (DENIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (VIOLATION, SUSPICIOUS) (enum), assessment_checks (as F05)]
- Decision [decision (DENY, QUARANTINE) (enum), decision_rule (enum as F05), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), latency_ms (duration_ms)]
- Severity [severity_status (MEDIUM, HIGH), severity_basis (OUTCOME), alert_kind (NONE, BYPASS_ATTEMPT)]
- Bindings [(common)]
- Telemetry [(common)]
- Other []

#### F16.2 `PERMISSION_LEFT_TO_PERSON`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (ASK_PERSON) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT, PENDING) (enum), assessment_checks (as F05)]
- Decision [decision (ALLOW, REQUIRE_APPROVAL) (enum), decision_rule (enum as F05), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), latency_ms (duration_ms)]
- Severity [severity_status (INFO, LOW), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other []

#### F16.3 `PERMISSION_OUTCOME_INFERRED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (APPROVED_BY_PERSON, DECLINED_BY_PERSON, UNKNOWN) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([PAIRING])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), seconds_to_outcome (integer, null)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), known_as (INFERRED)]
- Other [inferred_from (POST_TOOL_REPORT, SESSION_END_WITHOUT_REPORT) (enum)]

---

### F17. Session resumed, compacted, or cleared

An agent session can be resumed, compacted, or cleared. Each time, Crane receives a start signal
with the reason, keeps the same session identity where it can, re-supplies the rules and context,
and continues the same credit ledger.

Common attributes
- Action [action_type (SESSION_START) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (hook invocation id)]
- Repository [(as F03)]
- Session [(as F03)]
- Agent [(as F03)]
- Resource [resource ([FILE_READ, SELECTION]), resource_ids (context files), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([INTEGRITY])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([CLAUDE_CODE, CODEX]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED), known_as (OBSERVED), hook_event (SESSION_START)]

#### F17.1 `SESSION_RESUMED` / `SESSION_COMPACTED` / `SESSION_CLEARED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (CONTEXT_SUPPLIED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), context_files_supplied (integer), credits_available (integer), minutes_since_last_seen (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [start_source (RESUME, COMPACT, CLEAR) (enum), reopened_after_end (boolean)]

#### F17.2 `SESSION_IDENTITY_CONFLICT`
The provider reported an identity that differs from what Crane recorded.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NOT_CONSUMED])]
- Response [response_status (RECORDED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([IDENTITY])]
- Decision [(common)]
- Eval [eval (MEASURED), identity_conflicts (integer)]
- Severity [severity_status (MEDIUM), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [conflicting_field (PROVIDER_SESSION_ID, MODEL, PROVIDER) (enum)]

---

### F18. Protected blocks move, break, or go missing

Crane follows each block by its markers. A block moved within a file or into a renamed file still
resolves; when a person validates on a machine holding a signing key, the new location is recorded
and signed. Missing, duplicated, malformed, out-of-order, or overlapping markers make the block
unresolved; if the original code is found elsewhere it is reported as a candidate, never adopted.

Common attributes
- Action [action_type (TRACK_SELECTION) (enum), action_source (CLI, HOOK, EXTENSION, CI) (enum), action_user (string), action_email (string, null), action_id (string)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum), session_id (string, null), provider_session_id (string, null), id_provenance (enum), task_id (string, null), external_task_id (string, null)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (string, null), model (string, null), autonomy_mode (enum)]
- Resource [resource ([SELECTION, FILE_READ]), resource_ids ([selection id, path]), resource_classification ([PROTECTED, TARGETED])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE, ACTIVE), known_as (OBSERVED), hook_event (NA, POST_TOOL_USE, STOP) (enum)]
- Other [selection_id (string), operation (PRESERVE, TARGET) (enum), policy (string)]

#### F18.1 `SELECTION_RELOCATED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (RESOLVED) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (COMPLIANT), assessment_checks ([MARKERS])]
- Decision [(common)]
- Eval [eval (MEASURED), line_shift (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), from_path (string), to_path (string), relocation_kind (WITHIN_FILE, RENAMED_FILE) (enum)]

#### F18.2 `SELECTION_RELOCATION_RECORDED`
- Action [(common), action_source (CLI)]
- Repository [(common), registry_generation (integer, the new generation)]
- Session [(common), session_type (NOT_IN_SESSION)]
- Agent [(common), agent_type (NOT_AN_AGENT)]
- Resource [(common), resource ([SELECTION, CRANE_METADATA])]
- Response [response_status (OK, SKIPPED) (enum), failure_state (NA, KEY_UNAVAILABLE, NO_CONSOLE) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [(common)]
- Eval [eval (MEASURED), relocations_recorded (integer)]
- Severity [severity_status (INFO, LOW), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

#### F18.3 `SELECTION_UNRESOLVED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (UNRESOLVED) (enum), failure_state (MARKERS_MISSING, MARKERS_DUPLICATED, MARKER_MALFORMED, MARKERS_OUT_OF_ORDER, MARKERS_OVERLAP, MARKERS_SPLIT_ACROSS_FILES) (enum), response_statement (string)]
- Assessment [assessment (UNRESOLVED), assessment_checks ([MARKERS])]
- Decision [(common)]
- Eval [(common), eval (NOT_REQUIRED)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), last_known_path (string)]

#### F18.4 `SELECTION_CANDIDATE_FOUND`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (CANDIDATE) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (PENDING), assessment_checks ([BASELINE_CONTENT_SEARCH])]
- Decision [(common)]
- Eval [eval (MEASURED), candidates (integer)]
- Severity [severity_status (MEDIUM), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), known_as (INFERRED)]
- Other [(common), candidate_locations (list of path:start-end)]

#### F18.5 `STRAY_MARKERS_FOUND`
Markers that are not in the signed rules (copied, forged, or left behind).
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource_classification ([METADATA])]
- Response [response_status (FOUND) (enum), failure_state (UNREGISTERED_MARKER) (enum), response_statement (string)]
- Assessment [assessment (SUSPICIOUS), assessment_checks ([MARKERS])]
- Decision [(common)]
- Eval [eval (MEASURED), stray_markers (integer)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [marker_ids (list of strings), paths (list of strings)]

---

### F19. Policy tests and the enforcement self-test

A person or CI runs the policy tests: every rule is checked against the trusted checkpoint, then any
project test commands configured in the rules run. Policy status runs a self-test that simulates the
enforcement boundary for one policy and reports whether enforcement actually works.

Common attributes
- Action [action_type (TEST) (enum), action_source (CLI, CI, EXTENSION) (enum), action_user (string), action_email (string, null), action_id (run id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]

#### F19.1 `POLICY_TEST_RUN`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION, FILE_READ, GIT])]
- Response [response_status (PASS, FAIL) (enum), failure_state (NA, POLICY, CHECKPOINT, SELECTION_UNRESOLVED, VERIFICATION_FAILED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION, UNRESOLVED), assessment_checks ([PRESERVES_UNCHANGED, TARGETS_CHANGED, SELECTIONS_RESOLVED, PROJECT_TESTS])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), rules_passed (integer), rules_failed (integer), rules_unresolved (integer), duration_ms (duration_ms)]
- Severity [severity_status (INFO, HIGH) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [policies (list of strings)]

#### F19.2 `POLICY_RULE_RESULT`
One per preserve or target rule.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION]), resource_ids ([selection id]), resource_classification ([PROTECTED, TARGETED])]
- Response [response_status (PASS, FAIL, UNRESOLVED) (enum), failure_state (NA, PRESERVE_CHANGED, TARGET_NOT_CHANGED, TARGET_WRONG_KIND, SELECTION_UNRESOLVED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION, UNRESOLVED), assessment_checks ([CONTENT_DIGEST, CHANGE_CLASSIFICATION])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), changed_lines (integer, null)]
- Severity [severity_status (INFO, HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [policy (string), operation (PRESERVE, TARGET) (enum), change_type_required (ANY, LOGICAL_BN, LOGICAL_CN, LOGICAL_SN, SEMANTIC, NA) (enum), change_type_found (NONE, WHITESPACE_ONLY, LOGICAL_BN, LOGICAL_CN, LOGICAL_SN, SEMANTIC, NA) (enum)]

#### F19.3 `PROJECT_TEST_RESULT`
One per test command configured in the rules.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([COMMAND]), resource_ids ([test name])]
- Response [response_status (PASS, FAIL, TIMEOUT) (enum), failure_state (NA, EXIT_CODE, TIMEOUT, NOT_STARTED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION), assessment_checks ([PROJECT_TESTS])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), exit_code (integer, null), duration_ms (duration_ms)]
- Severity [severity_status (INFO, HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [test_name (string)]

#### F19.4 `ENFORCEMENT_SELF_TEST`
- Action [(common), action_type (SELF_TEST)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([SELECTION, CRANE_METADATA])]
- Response [response_status (PASS, GAP_FOUND) (enum), failure_state (NA, PRESERVE_WRITE_ALLOWED, TARGET_WRITE_DENIED, METADATA_WRITE_ALLOWED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION), assessment_checks ([SIMULATED_PRESERVE_WRITE, SIMULATED_TARGET_WRITE, SIMULATED_METADATA_WRITE])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), checks_passed (integer), checks_failed (integer)]
- Severity [severity_status (INFO, CRITICAL) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [policy (string)]

---

### F20. Policy context files

A person attaches a file to a policy as guidance for the agent. It is supplied at session start
(F03) and is never treated as permission. Crane keeps its digest; later it may be current, changed,
or missing.

Common attributes
- Action [action_type (CONTEXT) (enum), action_source (CLI, EXTENSION) (enum), action_user (string), action_email (string, null), action_id (string)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([FILE_READ, CRANE_METADATA]), resource_ids ([context file]), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([CONTEXT_DIGEST])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [policy (string), path (string)]

#### F20.1 `CONTEXT_ATTACHED`
- Action [(common)]
- Repository [(common), registry_generation (integer, the new generation)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, FILE_MISSING, POLICY, NOT_ADMIN, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), file_bytes (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), file_digest (digest)]

#### F20.2 `CONTEXT_VIEWED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

#### F20.3 `CONTEXT_STATE_CHANGED`
- Action [(common), action_source (CLI, HOOK)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (CURRENT, STALE, MISSING) (enum), failure_state (NA, FILE_CHANGED, FILE_MISSING) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, UNRESOLVED), assessment_checks ([CONTEXT_DIGEST])]
- Decision [(common)]
- Eval [(common), eval (NOT_REQUIRED)]
- Severity [severity_status (INFO, MEDIUM) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

---
### F21. Repository connection and GitHub access

A person connects the clone over SSH, HTTPS, or the GitHub CLI and chooses which read privileges
Crane may use. With the GitHub CLI, a person grants Crane use of their login for a limited number of
days. Crane checks the connection live when it matters; the record lives on the machine, so an
agent cannot forge it. A person can view or delete the connection. (Creating it is F01.1.)

Common attributes
- Action [action_type (CONNECTION) (enum), action_source (CLI, EXTENSION) (enum), action_user (string), action_email (string, null), action_id (command id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([GIT, TRUST_DIRECTORY]), resource_ids ([remote url, redacted]), resource_classification ([EXTERNAL])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [method (SSH, HTTPS, GH_CLI) (enum)]

#### F21.1 `CONNECTION_CHECKED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (CONNECTED, DISCONNECTED, NOT_CONFIGURED) (enum), failure_state (NA, UNREACHABLE, AUTH_FAILED, TIMEOUT, GH_NOT_LOGGED_IN, GRANT_EXPIRED) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, UNRESOLVED), assessment_checks ([REMOTE_REACHABLE])]
- Decision [(common)]
- Eval [eval (MEASURED), probe_latency_ms (duration_ms)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [third_party ([GITHUB, GIT_REMOTE, GITHUB_CLI]), binding_result (OK, FAILED, TIMEOUT)]
- Telemetry [(common)]
- Other [(common), privileges (list of enums as F01.1)]

#### F21.2 `GITHUB_GRANT_CREATED` / `GITHUB_GRANT_EXPIRED`
- Action [(common), action_type (GITHUB_GRANT)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([TRUST_DIRECTORY])]
- Response [response_status (GRANTED, EXPIRED, FAILED) (enum), failure_state (NA, GH_MISSING, LOGIN_FAILED, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), grant_days (integer), days_remaining (integer)]
- Severity [severity_status (LOW, MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([GITHUB_CLI, GITHUB]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [(common), grant_scope (TERMINAL_SESSION, DAYS) (enum), expires_at (timestamp)]

#### F21.3 `CONNECTION_VIEWED` / `CONNECTION_DELETED`
- Action [(common), action_type (VIEW_CONNECTION, DELETE_CONNECTION)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, NOT_CONFIGURED, REFUSED) (enum), failure_state (NA, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (INFO for viewed, MEDIUM for deleted) (enum), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [(common)]
- Other [(common)]

---

### F22. Setting up integrations (Slack, WhatsApp, Jira, GitHub)

A person configures an integration on their machine. Secrets stay on the machine; only non-secret
settings (channel, minimum severity, cooldown) are uploaded and shown. The connection can be tested
at once. F14 then uses these settings to deliver alerts.

Common attributes
- Action [action_type (INTEGRATION) (enum), action_source (CLI) (enum), action_user (string), action_email (string, null), action_id (command id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([TRUST_DIRECTORY, NETWORK]), resource_ids ([integration name]), resource_classification ([EXTERNAL])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [integration (SLACK, WHATSAPP, JIRA, GITHUB) (enum)]

#### F22.1 `INTEGRATION_CONFIGURED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED, CANCELLED) (enum), failure_state (NA, INVALID_SETTING, AGENT_REFUSED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), secret_fields (integer), cooldown_seconds (integer, null)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [third_party ([SLACK, WHATSAPP, JIRA, GITHUB]), binding_result (NA)]
- Telemetry [(common)]
- Other [(common), min_severity (LOW, MEDIUM, HIGH, CRITICAL) (enum, null), reconfigured (boolean)]

#### F22.2 `INTEGRATION_TESTED`
- Action [(common), action_type (TEST_INTEGRATION)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, HTTP_ERROR, UNREACHABLE, AUTH_FAILED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), http_status (integer, null), latency_ms (duration_ms)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [third_party ([SLACK, WHATSAPP, JIRA, GITHUB]), binding_result (OK, FAILED, TIMEOUT)]
- Telemetry [(common)]
- Other [(common)]

---

### F23. Two admins change the rules at the same time

Merging two admins' rule changes from different branches conflicts in the rules folder, since each
side carries its own signed version. Crane refuses conflicted rules and explains the repair: keep one
side for the whole folder, accept that signed state on each machine, then redo the other change.

Common attributes
- Action [action_type (RULES_MERGE) (enum), action_source (CLI, HOOK) (enum), action_user (string), action_email (string, null), action_id (string)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION), session_id (string, null), provider_session_id (null, string), id_provenance (enum), task_id (null, string), external_task_id (null, string)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC), agent_id (string, null), model (string, null), autonomy_mode (enum)]
- Resource [resource ([CRANE_METADATA, GIT]), resource_ids (conflicted files), resource_classification ([METADATA])]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [known_as (OBSERVED), hook_event (NA, PRE_TOOL_USE)]

#### F23.1 `RULES_CONFLICT_DETECTED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (REFUSED) (enum), failure_state (CONFLICT) (enum), response_statement (string)]
- Assessment [assessment (UNRESOLVED), assessment_checks ([CONFLICT_MARKERS])]
- Decision [decision (DENY for agent changes), decision_rule (INTEGRITY_FAILED), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), conflicted_files (integer)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [agent_state (INACTIVE, ERROR)]
- Other []

#### F23.2 `FOREIGN_RULES_ACCEPTED`
Another admin's signed rules were accepted on this machine (the rollback memory moves to them).
- Action [(common), action_source (CLI)]
- Repository [(common)]
- Session [(common), session_type (NOT_IN_SESSION)]
- Agent [(common), agent_type (NOT_AN_AGENT)]
- Resource [resource ([CRANE_METADATA, TRUST_DIRECTORY])]
- Response [response_status (ACCEPTED, DECLINED) (enum), failure_state (NA) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT), assessment_checks ([SIGNATURES, SIGNERS])]
- Decision [decision (ALLOW, DENY), decision_rule (HUMAN_CONFIRMATION), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), accepted_generation (integer)]
- Severity [severity_status (LOW), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [agent_state (INACTIVE)]
- Other [signed_by (key)]

---

### F24. Checks in CI or other headless machines

On a build server, the owner key is supplied by configuration, so Crane verifies the rules and runs
the policy tests without a person. Nothing is signed there; any failure fails the build.

#### F24.1 `HEADLESS_VERIFICATION`
- Action [action_type (VERIFY_RULES, TEST) (enum), action_source (CI) (enum), action_user (CI actor) (string), action_email (null), action_id (CI run id) (string)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([CRANE_METADATA, SELECTION, GIT]), resource_ids ([]), resource_classification ([METADATA])]
- Response [response_status (PASS, FAIL) (enum), failure_state (NA, VERIFICATION_FAILED, POLICY, NO_TRUSTED_KEY) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION), assessment_checks ([INTEGRITY, POLICIES])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), duration_ms (duration_ms), findings (integer), rules_failed (integer)]
- Severity [severity_status (INFO, HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([GITHUB_ACTIONS]), binding_result (OK, FAILED)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [owner_key_source (CONFIGURED, PINNED) (enum)]

---

### F25. Keys: signing keys and the upload key

A repository signing key is created sealed to the person's Windows login (or under a passphrase
typed on the console); older plain keys are sealed at the next signing command. In passphrase mode
every signing command asks for it. The upload key identifies the machine to the service, signs
uploads and session attestations, and is created on first use. Hooks and verification only read
public halves.

Common attributes
- Action [action_type (KEY) (enum), action_source (CLI, HOOK, EXTENSION) (enum), action_user (string), action_email (string, null), action_id (string)]
- Repository [repo (enum), repo_id (digest, null), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([SIGNING_KEY, UPLOAD_KEY, TRUST_DIRECTORY]), resource_ids ([key file name]), resource_classification ([TRUST])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [key_kind (SIGNING, UPLOAD) (enum), public_key (key)]

#### F25.1 `KEY_CREATED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, AGENT_REFUSED, UNWRITABLE, NO_CONSOLE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (INFO, HIGH) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([WINDOWS_DPAPI, NONE]), binding_result (OK, FAILED, NA)]
- Telemetry [(common)]
- Other [(common), protection (PLAIN, DPAPI, PASSPHRASE) (enum)]

#### F25.2 `KEY_PROTECTION_UPGRADED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, UNWRITABLE, NO_CONSOLE, PASSPHRASES_DIFFER, PASSPHRASE_TOO_SHORT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (INFO, MEDIUM) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([WINDOWS_DPAPI, NONE]), binding_result (OK, FAILED, NA)]
- Telemetry [(common)]
- Other [(common), from_protection (PLAIN, DPAPI) (enum), to_protection (DPAPI, PASSPHRASE) (enum)]

#### F25.3 `KEY_UNLOCKED` / `KEY_UNLOCK_FAILED`
- Action [(common), action_source (CLI, EXTENSION)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, WRONG_PASSPHRASE, OTHER_WINDOWS_USER, NO_CONSOLE, MALFORMED_KEY_FILE, KEY_MISSING) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), unlock_ms (duration_ms)]
- Severity [severity_status (INFO, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [third_party ([WINDOWS_DPAPI, NONE]), binding_result (OK, FAILED, NA)]
- Telemetry [(common)]
- Other [(common), protection (PLAIN, DPAPI, PASSPHRASE) (enum), purpose (COMMIT_RULES, SET_ANCHOR, REGISTER_OWNER, ADMIN_CHANGE, RECORD_RELOCATION) (enum)]

---

### F26. Local speed caches

To keep each tool call fast, Crane remembers the repository identity for ten minutes (refreshed when
the Git settings change) and remembers checkpoint commits it already verified. Both live in the
private trust folder.

Common attributes
- Action [action_type (CACHE) (enum), action_source (HOOK, CLI) (enum), action_user (string), action_email (null), action_id (null)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum), session_id (string, null), provider_session_id (null, string), id_provenance (enum), task_id (null, string), external_task_id (null, string)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (string, null), model (null, string), autonomy_mode (enum)]
- Resource [resource ([TRUST_DIRECTORY, GIT]), resource_ids ([]), resource_classification ([TRUST])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE, ACTIVE), known_as (OBSERVED), hook_event (enum)]

#### F26.1 `IDENTITY_CACHE_USED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (HIT, MISS, REFRESHED) (enum), failure_state (NA, EXPIRED, GIT_CONFIG_CHANGED, GIT_REDIRECTED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), lookup_ms (duration_ms), cache_age_ms (duration_ms, null)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other []

#### F26.2 `CHECKPOINT_COMMIT_CHECKED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (REMEMBERED, VERIFIED_AND_REMEMBERED, MISSING) (enum), failure_state (NA, COMMIT_NOT_FETCHED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), check_ms (duration_ms)]
- Severity [severity_status (INFO, HIGH for MISSING) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [checkpoint_name (string), commit (string)]

---

### F27. The credit budget

Every session's autonomy budget moves through fixed steps: granted at start, reserved when an action
is allowed, consumed when it ran or released when it failed or never reported back, revoked as a
penalty after a violation, and expired when the session ends. Low balances raise an alert; an empty
balance turns allowed actions into approval requests. Refund and regeneration are defined in the
ledger for future use and are not triggered today.

Common attributes
- Action [action_type (CREDITS) (enum), action_source (HOOK) (enum), action_user (string), action_email (string, null), action_id (tool-call id, null)]
- Repository [(as F05)]
- Session [(as F05)]
- Agent [(as F05)]
- Resource [resource ([CREDITS]), resource_ids ([]), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED, ENDED), known_as (OBSERVED), hook_event (SESSION_START, PRE_TOOL_USE, POST_TOOL_USE, SESSION_END) (enum)]
- Eval [eval (MEASURED), amount (integer), granted (integer), reserved (integer), consumed (integer), revoked (integer), expired (integer), available (integer)]
- Other [ledger_kind (GRANT, RESERVE, CONSUME, RELEASE, REVOKE, EXPIRE, REFUND, REGENERATE) (enum), ledger_sequence (integer), idempotency_key (string)]

#### F27.1 `CREDITS_GRANTED`
- Action [(common), action_id (null)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common), credit_ttl_seconds (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (SESSION_START)]
- Other [(common), ledger_kind (GRANT)]

#### F27.2 `CREDITS_RESERVED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (ALLOW), decision_rule (ALLOWED), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common), cost_breakdown ({operation: integer, selection_file: integer, network: integer, criticality: integer}) (object)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (PRE_TOOL_USE)]
- Other [(common), ledger_kind (RESERVE)]

#### F27.3 `CREDITS_SHORT`
Concurrent actions used up the balance between the decision and the reservation.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (RELEASED) (enum), failure_state (INSUFFICIENT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (REQUIRE_APPROVAL), decision_rule (CREDITS_SHORT), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (LOW), severity_basis (THRESHOLD), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (PRE_TOOL_USE)]
- Other [(common), ledger_kind (RELEASE)]

#### F27.4 `CREDITS_LOW`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (RECORDED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common), low_credit_threshold (integer, 20)]
- Severity [severity_status (MEDIUM), severity_basis (THRESHOLD), alert_kind (LOW_CREDITS)]
- Bindings [(common)]
- Telemetry [(common), hook_event (PRE_TOOL_USE)]
- Other [(common), ledger_kind (RESERVE)]

#### F27.5 `CREDITS_EXHAUSTED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (APPROVAL_REQUIRED) (enum), failure_state (INSUFFICIENT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (REQUIRE_APPROVAL), decision_rule (CREDITS_EXHAUSTED), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (LOW), severity_basis (THRESHOLD), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (PRE_TOOL_USE)]
- Other [(common)]

#### F27.6 `CREDITS_CONSUMED` / `CREDITS_RELEASED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA, TOOL_FAILED, NO_POST_TOOL_REPORT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (POST_TOOL_USE, SESSION_END)]
- Other [(common), ledger_kind (CONSUME, RELEASE)]

#### F27.7 `CREDITS_REVOKED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [assessment (VIOLATION), assessment_checks ([SELECTIONS_RECHECK])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (DETECTED_AFTER_EFFECT, BYPASS_CONFIRMED) (enum)]
- Eval [(common), violation_penalty (integer, 10)]
- Severity [severity_status (HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (POST_TOOL_USE)]
- Other [(common), ledger_kind (REVOKE)]

#### F27.8 `CREDITS_EXPIRED`
- Action [(common), action_id (null)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), hook_event (SESSION_END)]
- Other [(common), ledger_kind (EXPIRE), expire_reason (SESSION_END, TTL_ELAPSED) (enum)]

#### F27.9 `CREDITS_REFUNDED` / `CREDITS_REGENERATED` (reserved)
Defined for future controlled events; not emitted by the current code.
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [(common)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), ledger_kind (REFUND, REGENERATE), issuer (string)]

#### F27.10 `LEDGER_RECONCILIATION_FAILED`
The ledger chain or balances do not add up (detected by the dashboard or `crane validate`).
- Action [(common), action_source (DASHBOARD, CLI), action_id (null)]
- Repository [(common)]
- Session [(common), session_type (AGENT_SESSION, NOT_IN_SESSION)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (FAILED) (enum), failure_state (CHAIN_BROKEN, BALANCE_MISMATCH) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION), assessment_checks ([LEDGER_CHAIN, BALANCES])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (DETECTED_AFTER_EFFECT)]
- Eval [(common), entries (integer)]
- Severity [severity_status (HIGH), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), known_as (INFERRED)]
- Other [(common)]

---

### F28. Machine status reports

Alongside uploads, each machine sends a status report at most every two minutes: whether the rules
verify (with findings), policy results, where each block is, checkpoints, admins, autonomy settings,
its Crane version, its GitHub connection, and its integrations' non-secret settings. The dashboard's
Governance and Settings pages show the newest report.

#### F28.1 `STATUS_REPORTED`
- Action [action_type (STATUS_REPORT) (enum), action_source (CLI) (enum), action_user (Git user.name) (string), action_email (string, null), action_id (request id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (OBSERVE, ASSISTED, DELEGATED, AUTONOMOUS) (enum, the configured mode)]
- Resource [resource ([CRANE_METADATA, SELECTION]), resource_ids ([]), resource_classification ([METADATA])]
- Response [response_status (RULES_PASS, RULES_FAIL, NOT_INITIALIZED) (enum), failure_state (NA, VERIFICATION_FAILED, POLICY) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION, UNRESOLVED), assessment_checks ([INTEGRITY, POLICIES, SELECTIONS, CONTEXT, CONNECTION])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), selections (integer), selections_unresolved (integer), policies_passing (integer), policies_total (integer), checkpoints (integer), admins (integer), session_credits (integer), findings (integer)]
- Severity [severity_status (INFO, HIGH) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([FOXX_SERVICE]), binding_result (OK, FAILED)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [crane_version_changed (boolean), previous_crane_version (string, null), default_policy (string, null), default_checkpoint (string, null), connection_method (SSH, HTTPS, GH_CLI, NONE) (enum)]

---

### F29. Running the hosted service

At start the service upgrades its database layout (versioned, safe to repeat), then serves uploads
and the dashboard. It answers health checks, rate-limits per machine and per network address, caps
batch sizes and per-machine quotas, refuses uploads near the database size limit (the dashboard keeps
working), expires routine records, and logs every request with an id it also returns.

Common attributes
- Action [action_source (SERVICE) (enum), action_user (null), action_email (null), action_id (request id) (string, null)]
- Repository [repo (UNKNOWN, PERSONAL, ORGANIZATION) (enum), repo_id (digest, null), repo_remote (null, string), repo_remote_verified (boolean), registry_generation (null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([DATABASE]), resource_ids ([]), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Bindings [third_party ([MONGODB, RENDER]), binding_result (OK, FAILED, TIMEOUT)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]

#### F29.1 `SERVICE_STARTED`
- Action [(common), action_type (SERVICE_START) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, DATABASE_UNREACHABLE, MIGRATION_FAILED, CONFIG_INVALID) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), schema_version (integer), workers (integer)]
- Severity [severity_status (INFO, CRITICAL) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [migration_applied (boolean), sign_in_configured (boolean)]

#### F29.2 `HEALTH_CHECKED`
- Action [(common), action_type (HEALTH)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, UNAVAILABLE) (enum), failure_state (NA, DATABASE_UNREACHABLE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), ping_ms (duration_ms)]
- Severity [severity_status (INFO, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other []

#### F29.3 `REQUEST_LIMITED`
- Action [(common), action_type (UPLOAD, DASHBOARD_REQUEST) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (RATE_LIMITED, TOO_LARGE, QUOTA_REACHED, STORAGE_FULL) (enum), failure_state (MACHINE_RATE, ADDRESS_RATE, BATCH_TOO_LARGE, BODY_TOO_LARGE, EVENT_QUOTA, REPOSITORY_QUOTA, DATABASE_SIZE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (DENY), decision_rule (UPLOAD_LIMIT) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [eval (MEASURED), limit (integer), observed (integer), retry_after_s (integer, null)]
- Severity [severity_status (MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [machine_key (key, null), client_address_digest (digest, null)]

#### F29.4 `REQUEST_FAILED`
- Action [(common), action_type (UPLOAD, DASHBOARD_REQUEST) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SERVER_ERROR, NOT_FOUND, METHOD_NOT_ALLOWED) (enum), failure_state (STORE_FAILED, ROUTE_UNKNOWN, WRITE_TO_DASHBOARD) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons (list), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), duration_ms (duration_ms), http_status (integer)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common), agent_state (ERROR)]
- Other [path (string), method (GET, POST, PUT, DELETE, PATCH, OTHER) (enum)]

(Upload refusals as seen by the machine are F09.4; record expiry is F09.7.)

---

### F30. Browsing the dashboard, downloads, and approval edge cases

A signed-in person moves between tabs, filters the event timeline, pages through it, opens session
pages, and downloads a session report or evidence bundle. Approvals for private repositories may
need a second sign-in that also grants repository access; invalid codes are refused.

Common attributes
- Action [action_source (DASHBOARD) (enum), action_user (GitHub login) (string), action_email (string, null), action_id (request id)]
- Repository [repo (PERSONAL, ORGANIZATION) (enum), repo_id (digest), repo_remote (string), repo_remote_verified (true), registry_generation (integer, null)]
- Session [session_type (DASHBOARD_SESSION), session_id (string), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([DATABASE]), resource_ids ([]), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([MONGODB]), binding_result (OK, FAILED)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [member_filter (key, null)]

#### F30.1 `DASH_VIEW_OPENED`
- Action [(common), action_type (OPEN_VIEW) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, EMPTY, FAILED) (enum), failure_state (NA, DATABASE_UNAVAILABLE, NOT_FOUND) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), load_ms (duration_ms), cached (boolean)]
- Severity [severity_status (INFO, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), view (REPOSITORIES, OVERVIEW, AGENTS, OBSERVABILITY, GOVERNANCE, COMPLIANCE, ALERTS, SETTINGS, SESSION) (enum), session_viewed (string, null)]

#### F30.2 `DASH_EVENTS_FILTERED` / `DASH_EVENTS_PAGED`
- Action [(common), action_type (FILTER_EVENTS, LOAD_MORE_EVENTS) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, EMPTY) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), results (integer), cursor (integer), limit (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), filters ({session, task, type, decision, severity, domain, enforcement, policy, selection}) (object of strings)]

#### F30.3 `DASH_DOWNLOAD`
- Action [(common), action_type (DOWNLOAD_REPORT, DOWNLOAD_EVIDENCE) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, NOT_FOUND) (enum), failure_state (NA, SESSION_UNKNOWN) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), bytes (integer), parts (integer)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), agent_session_id (string), bundle_digest (digest, null)]

#### F30.4 `APPROVAL_CODE_REJECTED`
- Action [(common), action_type (CONFIRM_LINK, CONFIRM_OWNER) (enum), action_source (DASHBOARD, LINK)]
- Repository [(common), repo_id (digest, null)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (REFUSED, BAD_REQUEST) (enum), failure_state (CODE_MALFORMED, CODE_EXPIRED, CODE_USED, CODE_UNKNOWN, UNKNOWN_ACTION) (enum), response_statement (string)]
- Assessment [assessment (VIOLATION), assessment_checks ([CODE_VALID])]
- Decision [decision (DENY), decision_rule (CODE_INVALID) (enum), decision_reasons (list), enforcement (PREVENTED)]
- Eval [(common), eval (NOT_REQUIRED)]
- Severity [severity_status (MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [third_party ([GITHUB, MONGODB]), binding_result (NA)]
- Telemetry [(common)]
- Other []

#### F30.5 `APPROVAL_PRIVATE_RETRY_OFFERED`
GitHub did not show the repository with the basic sign-in; a retry asking for repository access is offered.
- Action [(common), action_type (CONFIRM_LINK, CONFIRM_OWNER) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (RETRY_OFFERED) (enum), failure_state (NO_ACCESS) (enum), response_statement (string)]
- Assessment [assessment (UNRESOLVED), assessment_checks ([GITHUB_READ_ACCESS])]
- Decision [(common)]
- Eval [(common), eval (NOT_REQUIRED)]
- Severity [severity_status (LOW), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [third_party ([GITHUB]), binding_result (OK)]
- Telemetry [(common)]
- Other [oauth_scope_next (READ_ORG_AND_REPO) (enum)]

---

### F31. The Foxx editor extension

Inside VS Code, Foxx sets up Crane, opens a panel for the selected code (or a whole file from the
Explorer), and runs actions from it: protect, target, zone, flow, context, policy, checkpoint, move,
hooks. Zone, flow, and context changes are validated and signed, and reverted if either step fails;
the last change can be undone. Foxx validates on save, shows findings as problems, shows each
block's result above it, and shows the repository's state in the status bar. The actions themselves
emit their own events (F01, F15, F20, F25); F31 records the editor side.

Common attributes
- Action [action_source (EXTENSION) (enum), action_user (Git user.name) (string), action_email (string, null), action_id (string)]
- Repository [repo (enum), repo_id (digest, null), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer, null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([FILE_READ, CRANE_METADATA]), resource_ids (file paths), resource_classification ([NONE, PROTECTED, TARGETED, ZONED])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Bindings [third_party ([VSCODE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [extension_version (string)]

#### F31.1 `FOXX_SETUP`
- Action [(common), action_type (SETUP) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED, CANCELLED) (enum), failure_state (NA, NO_REMOTE, OWNER_DECLINED, CRANE_MISSING, GIT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [third_party ([VSCODE, GITHUB]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [(common), role (OWNER, MEMBER) (enum)]

#### F31.2 `FOXX_PANEL_OPENED` / `FOXX_PANEL_REFUSED`
- Action [(common), action_type (OPEN_PANEL) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OPENED, REFUSED) (enum), failure_state (NA, NO_SELECTION, NOT_A_FILE, FILE_UNREADABLE) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), selected_lines (integer, null)]
- Severity [severity_status (INFO, LOW), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), opened_from (TITLE_BUTTON, KEYBINDING, EXPLORER, CONTEXT_MENU, STATUS_BAR) (enum), enclosing_function (string, null)]

#### F31.3 `FOXX_ACTION_RUN`
- Action [(common), action_type (PANEL_ACTION) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, VALIDATION_FAILED, SIGN_FAILED, NOT_ADMIN, CRANE_ERROR, INVALID_INPUT) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), duration_ms (duration_ms)]
- Severity [severity_status (LOW, MEDIUM, HIGH), severity_basis (FAILURE_STREAK), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), panel_action (PRESERVE, TARGET, ZONE, FLOW, CONTEXT, ZONE_ATTACH, POLICY_CONTEXT, POLICY, MOVE, CHECKPOINT, VALIDATE, TEST, STATUS, HOOKS, SETUP) (enum), search_query (string, null), enforce_now (boolean, null)]

#### F31.4 `FOXX_GOVERNANCE_REVERTED` / `FOXX_GOVERNANCE_UNDONE`
- Action [(common), action_type (REVERT_GOVERNANCE, UNDO_GOVERNANCE) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common), resource_ids ([.crane/governance.yaml])]
- Response [response_status (REVERTED, UNDONE, NOTHING_TO_UNDO) (enum), failure_state (NA, VALIDATION_FAILED, SIGN_FAILED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (LOW, MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common)]

#### F31.5 `FOXX_VALIDATED_ON_SAVE`
- Action [(common), action_type (VALIDATE_ON_SAVE) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (PASS, FINDINGS) (enum), failure_state (NA) (enum), response_statement (string)]
- Assessment [assessment (COMPLIANT, VIOLATION, UNRESOLVED), assessment_checks ([INTEGRITY, POLICIES, SELECTIONS])]
- Decision [(common)]
- Eval [eval (MEASURED), diagnostics (integer), selections_failing (integer)]
- Severity [severity_status (INFO, MEDIUM, HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [(common), saved_file (string)]

---

### F32. Pull request notifications

Outside Crane's runtime, a repository workflow reacts to pull requests being opened, reopened,
marked ready, or closed, posting to Slack and commenting on the Jira issue named in the title or
branch. A channel without credentials is skipped; pull requests from forks never receive
credentials.

#### F32.1 `PULL_REQUEST_NOTIFIED`
One per channel.
- Action [action_type (PULL_REQUEST) (enum), action_source (GITHUB_WORKFLOW) (enum), action_user (GitHub login of the author) (string), action_email (null), action_id (workflow run id) (string)]
- Repository [repo (PERSONAL, ORGANIZATION) (enum), repo_id (null), repo_remote (string), repo_remote_verified (false), registry_generation (null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (Jira key) (string, null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([NETWORK]), resource_ids ([pull request number]), resource_classification ([EXTERNAL])]
- Response [response_status (SENT, SKIPPED, FAILED) (enum), failure_state (NA, NO_CREDENTIALS, FORK, NO_ISSUE_KEY, HTTP_ERROR) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (INFO, MEDIUM) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([GITHUB_ACTIONS, SLACK, JIRA]), binding_result (OK, SKIPPED, FAILED)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [pr_event (OPENED, REOPENED, READY_FOR_REVIEW, CLOSED, MERGED) (enum), channel (SLACK, JIRA) (enum)]

---

### F33. Building, releasing, and publishing

Every push runs formatting, lints, all tests, a smoke test of the real binary, and the extension
tests. A version tag builds the binaries for Windows, macOS, and Linux, publishes the release, and
publishes the extension to the Marketplace (and Open VSX) when it changed.

Common attributes
- Action [action_source (CI) (enum), action_user (GitHub login that pushed or tagged) (string), action_email (null), action_id (workflow run id) (string)]
- Repository [repo (PERSONAL, ORGANIZATION) (enum), repo_id (null), repo_remote (string), repo_remote_verified (false), registry_generation (null)]
- Session [session_type (NOT_IN_SESSION), session_id (null), provider_session_id (null), id_provenance (NA), task_id (null), external_task_id (null)]
- Agent [agent_type (NOT_AN_AGENT), agent_id (null), model (null), autonomy_mode (NA)]
- Resource [resource ([GIT]), resource_ids ([commit sha, tag]), resource_classification ([NONE])]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Telemetry [agent_state (INACTIVE), known_as (OBSERVED), hook_event (NA)]

#### F33.1 `CI_CHECKS_COMPLETED`
- Action [(common), action_type (CI) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SUCCESS, FAILURE) (enum), failure_state (NA, FORMAT, LINT, TESTS, SMOKE_TEST, EXTENSION) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), duration_ms (duration_ms), tests_passed (integer), tests_failed (integer)]
- Severity [severity_status (INFO, HIGH) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([GITHUB_ACTIONS]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [job (CORE, EXTENSION) (enum)]

#### F33.2 `RELEASE_PUBLISHED`
- Action [(common), action_type (RELEASE) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (SUCCESS, FAILURE) (enum), failure_state (NA, VERSION_MISMATCH, BUILD, UPLOAD) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (MEASURED), assets (integer), duration_ms (duration_ms)]
- Severity [severity_status (INFO, HIGH), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([GITHUB_ACTIONS, GITHUB]), binding_result (OK, FAILED)]
- Telemetry [(common)]
- Other [version (string), targets ([X86_64_LINUX_GNU, X86_64_WINDOWS_MSVC, X86_64_APPLE_DARWIN, AARCH64_APPLE_DARWIN]) (list of enums)]

#### F33.3 `EXTENSION_PUBLISHED`
- Action [(common), action_type (PUBLISH_EXTENSION) (enum)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [(common)]
- Response [response_status (PUBLISHED, SKIPPED, FAILED) (enum), failure_state (NA, UNCHANGED, NO_TOKEN, REJECTED) (enum), response_statement (string)]
- Assessment [(common)]
- Decision [(common)]
- Eval [eval (NOT_REQUIRED)]
- Severity [severity_status (INFO, MEDIUM) (enum), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [third_party ([VSCODE_MARKETPLACE, OPEN_VSX]), binding_result (OK, SKIPPED, FAILED)]
- Telemetry [(common)]
- Other [version (string), registry (MARKETPLACE, OPEN_VSX) (enum)]

---

### F34. Background upload timing

Uploads never hold up the agent. After an action, stop, session end, or a Crane command, a separate
background process (hidden window) starts an upload; otherwise any Crane activity starts one once the
last attempt is older than the interval (60 seconds by default). An unreachable service is simply
retried at the next trigger.

#### F34.1 `SYNC_TRIGGER_EVALUATED`
- Action [action_type (SYNC_TRIGGER) (enum), action_source (HOOK, CLI) (enum), action_user (string), action_email (null), action_id (null)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum), session_id (string, null), provider_session_id (string, null), id_provenance (enum), task_id (string, null), external_task_id (string, null)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (string, null), model (string, null), autonomy_mode (enum)]
- Resource [resource ([NOT_CONSUMED]), resource_ids ([]), resource_classification ([NONE])]
- Response [response_status (STARTED, NOT_DUE, UPLOADS_OFF, SPAWN_FAILED) (enum), failure_state (NA) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED), assessment_checks ([])]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), seconds_since_last_attempt (integer), interval_seconds (integer, 60)]
- Severity [severity_status (INFO, LOW) (enum), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (INACTIVE, ACTIVE), known_as (OBSERVED), hook_event (enum)]
- Other [trigger (POST_TOOL, STOP, SESSION_END, COMMAND, INTERVAL) (enum)]

---

### F35. Read-only inspection commands

People (and agents, which may run only these) inspect state without changing it: validate, parse a
file's markers, view the current task or session, view admins, view the repository connection, view
a policy's context, view hook validity. These are recorded by F13 as well; F35 adds what each showed.

#### F35.1 `STATE_INSPECTED`
- Action [action_type (INSPECT) (enum), action_source (CLI, EXTENSION, AGENT) (enum), action_user (string), action_email (string, null), action_id (command id)]
- Repository [repo (enum), repo_id (digest), repo_remote (string, null), repo_remote_verified (boolean), registry_generation (integer)]
- Session [session_type (NOT_IN_SESSION, AGENT_SESSION) (enum), session_id (string, null), provider_session_id (string, null), id_provenance (enum), task_id (string, null), external_task_id (string, null)]
- Agent [agent_type (NOT_AN_AGENT, CLAUDE_CODE, CODEX, GENERIC) (enum), agent_id (string, null), model (string, null), autonomy_mode (enum)]
- Resource [resource ([CRANE_METADATA, FILE_READ]), resource_ids (list), resource_classification ([METADATA, NONE])]
- Response [response_status (OK, FAILED) (enum), failure_state (NA, NOT_INITIALIZED, USAGE) (enum), response_statement (string)]
- Assessment [assessment (NOT_REQUIRED, COMPLIANT, VIOLATION) (enum), assessment_checks (list, for VALIDATE)]
- Decision [decision (NOT_REQUIRED), decision_rule (null), decision_reasons ([]), enforcement (NOT_APPLICABLE)]
- Eval [eval (MEASURED), duration_ms (duration_ms), findings (integer, null)]
- Severity [severity_status (INFO), severity_basis (FIXED), alert_kind (NONE)]
- Bindings [third_party ([NONE, GIT_REMOTE]), binding_result (NA, OK, FAILED)]
- Telemetry [agent_state (INACTIVE, ACTIVE), known_as (OBSERVED), hook_event (NA)]
- Other [inspection (VALIDATE, PARSE_FILE, TASK_CURRENT, TASK_LIST, SESSION_CURRENT, SESSION_LIST, ADMIN_LIST, REPO_STATUS, REPO_VIEW, VIEW_CONTEXT, HOOKS_VALIDATE, VERSION, HELP) (enum), output_format (TEXT, JSON) (enum)]

---

### F36. Agent network and external tool use

Within F05, some calls reach outside the repository: web fetches and searches, shell commands that
use the network, and tools from external (MCP) servers. Crane records the destinations it can infer
(never observed traffic) and charges extra credits for network use.

Common attributes
- Action [action_type (TOOL_CALL) (enum), action_source (HOOK), action_user (string), action_email (string, null), action_id (tool-call id), action_tool (WEB_FETCH, WEB_SEARCH, BASH, POWERSHELL, MCP, OTHER) (enum)]
- Repository [(as F05)]
- Session [(as F05)]
- Agent [(as F05)]
- Assessment [assessment (NOT_REQUIRED, COMPLIANT) (enum), assessment_checks ([NETWORK_INFERENCE, METADATA_PROTECTION])]
- Bindings [third_party ([NONE]), binding_result (NA)]
- Telemetry [agent_state (ACTIVE, DEGRADED, QUARANTINED), known_as (INFERRED), hook_event (PRE_TOOL_USE)]

#### F36.1 `NETWORK_DESTINATION_INFERRED`
- Action [(common)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([NETWORK]), resource_ids (hosts, query strings removed), resource_classification ([EXTERNAL])]
- Response [response_status (RECORDED) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (ALLOW, REQUIRE_APPROVAL, DENY) (enum, from F05), decision_rule (enum as F05), decision_reasons (list), enforcement (NOT_APPLICABLE, PREVENTED)]
- Eval [eval (MEASURED), destinations (integer), network_credit_surcharge (integer, 5)]
- Severity [severity_status (INFO, LOW, MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Bindings [(common)]
- Telemetry [(common)]
- Other [destination_source (URL_ARGUMENT, COMMAND_TEXT, NETWORK_TOOL_WITHOUT_HOST) (enum), unknown_destination (boolean)]

#### F36.2 `EXTERNAL_TOOL_CALLED`
A tool from an external (MCP) server or a tool Crane does not know.
- Action [(common), action_tool (MCP, OTHER)]
- Repository [(common)]
- Session [(common)]
- Agent [(common)]
- Resource [resource ([TOOL]), resource_ids ([tool name]), resource_classification ([EXTERNAL])]
- Response [response_status (ALLOWED, DENIED, ASK) (enum), failure_state (NA), response_statement (string)]
- Assessment [(common)]
- Decision [decision (ALLOW, REQUIRE_APPROVAL, DENY) (enum), decision_rule (ALLOWED, TOOL_NAMES_METADATA, MODE_ASSISTED, MODE_OBSERVE, CREDITS_EXHAUSTED) (enum), decision_reasons (list), enforcement (NOT_APPLICABLE, PREVENTED)]
- Eval [eval (MEASURED), credits_cost (integer)]
- Severity [severity_status (INFO, LOW, MEDIUM, HIGH), severity_basis (OUTCOME), alert_kind (NONE, BYPASS_ATTEMPT)]
- Bindings [(common), third_party ([NONE])]
- Telemetry [(common), known_as (OBSERVED)]
- Other [mcp_server (string, null), effect_known (false) (boolean)]

### F37. Bricks

Bricks are reusable modules of one fixed kind (knowledge, identity, access, approval, limits,
environment, precondition, verification, action, recovery, provenance, watch; the former kinds
context, scope, permission, constraint, isolation, budget, revocation, monitoring, and intelligence
are read as the kinds that replaced them) attached to Flows and Zones. Their lifecycle commands (`crane brick ...`),
the context the hooks deliver before an action, and the monitors the background sync runs are each
noted on the event they belong to (the command's F13 event, the tool call's F05 event, or the sync's
`brick.monitors_evaluated` event), and every note becomes one F37 event. Fields are identifiers,
digests, counts, and states; context text is never recorded. A decision the guard made because of a
Brick also carries `brick:ID@N` in Bindings (zones), next to the Zones and Flows.

Common attributes
- Action [action_type (BRICK_DRAFT, BRICK_VALIDATE, BRICK_TEST, BRICK_ACTIVATE, BRICK_DEPRECATE, BRICK_REVOKE, BRICK_STACK_DRAFT, BRICK_STACK_RESOLVE, BRICK_STACK_ACTIVATE, BRICK_STACK_REVOKE, BRICK_ATTACH, BRICK_DETACH, BRICK_UPGRADE, BRICK_CONTEXT, SUPPLY_CONTEXT, BRICK_IDENTITY, BRICK_ASSIGN, BRICK_MONITOR, BRICK_MIGRATE) (enum), action_source (CLI, EXTENSION, HOOK, SYNC), action_user (string), action_email (string, null), action_id (id of the carrying event)]
- Repository [(as F13)]
- Session [(as F13 for commands; as F05 for delivered context)]
- Agent [(as F13 for commands; as F05 for delivered context)]
- Resource [resource ([CRANE_METADATA]), resource_ids (Brick, Stack, and attachment references), resource_classification ([METADATA])]
- Response [response_status (OK, PASSED, FAILED, BLOCKED, UNKNOWN, FIRING, FAILING, UNSUPPORTED, REVOCATION_DUE, PROPOSAL, NOT_RUN) (enum), failure_state (NA or the failing status), response_statement (string)]
- Severity [severity_status (INFO, LOW, MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Other [brick, stack, attachment, target, targets, type, digest, snapshot, bytes, snapshot_bytes, bricks, refresh_reason (FIRST_ASSEMBLY, BRICK_STORE_CHANGED, GOVERNANCE_CHANGED, CONTEXT_CHANGED), truncated, conflicts, gaps, agent, state, monitor, source, inference (NONE), self_reviewed, pinned_attachments, context_bytes, from, members, problems, cases (each when it applies)]

#### F37.1 `BRICK_DRAFT_SAVED`, `BRICK_VALIDATED`, `BRICK_TESTED`
A draft was saved (it grants nothing), validated against its type's schema, or tested.
- Response [response_status (OK, PASSED, FAILED, BLOCKED, UNKNOWN)]
- Other [brick, type, problems or cases]

#### F37.2 `BRICK_ACTIVATED`, `BRICK_DEPRECATED`, `BRICK_REVOKED`
An admin signed a version into `.crane/bricks.json`, deprecated it, or revoked it (attachments pinned
to a revoked version fail closed: changes in the target need approval until it is detached or
upgraded).
- Other [brick, type, digest, self_reviewed, pinned_attachments]

#### F37.3 `BRICK_STACK_SAVED`, `BRICK_STACK_RESOLVED`, `BRICK_STACK_ACTIVATED`, `BRICK_STACK_REVOKED`
A Brick Stack (a version-pinned composition, never a type) was drafted, resolved, activated, or revoked.
- Other [stack, members, digest]

#### F37.4 `BRICK_ATTACHED`, `BRICK_DETACHED`, `BRICK_UPGRADED`, `BRICK_CONTEXT_UPDATED`
An admin attached a Brick or Stack to a Flow, Zone, or Runbook, detached it (the history stays in the
audit trail), pinned another version, or changed the attachment's Current Context.
- Other [attachment, target, brick, from, context_bytes]

#### F37.5 `BRICK_CONTEXT_PREVIEWED`, `BRICK_CONTEXT_DELIVERED`, `BRICK_CONTEXT_REUSED`
The effective context of targets was previewed by a person, or delivered to an agent before a tool
call (MISS: the full snapshot was sent; HIT: the session already had this snapshot, so a one-line
reference was sent instead). Provider-side prompt caching is not observed.
- Eval [context_cache (HIT, MISS)]
- Other [targets, snapshot, bytes, snapshot_bytes, bricks, refresh_reason, truncated, conflicts, gaps]

#### F37.6 `BRICK_IDENTITY_MISMATCH`
The acting agent is not admitted by an identity Brick of the target it touched. The guard refuses its
calls there (rule `BRICK_IDENTITY`), or holds them for a person when the Brick says `on_other: ask`;
this note is the explanation sent to the agent in the second case.
- Other [target, agent, bricks]

#### F37.7 `BRICK_ASSIGNED`, `BRICK_UNASSIGNED`
An admin assigned an agent to Flows and Zones, or revoked the assignment.
- Other [agent, targets, state]

#### F37.8 `BRICK_MONITOR_EVALUATED`
A monitoring signal, revocation trigger, or advisory analysis of an attached Brick was evaluated
deterministically (no model). Sources without a connector (ci, metrics, external) are UNSUPPORTED.
- Response [response_status (OK, FIRING, FAILING, UNSUPPORTED, REVOCATION_DUE, PROPOSAL, ERROR)]
- Other [target, brick, monitor (signal, trigger, analysis), source, inference (NONE)]

#### F37.9 `BRICK_WATCH_REACTED`
A watch signal of an attached Brick started firing (or failing) and one of its reactions ran: an
alert through the configured integrations, a pause (changes in the target need a person while it
fires, rule `BRICK_WATCH_PAUSE`), a revocation due, or a proposal. Noted once per start of firing.
- Response [response_status (FIRING, FAILING)]
- Other [target, brick, source, reaction (alert, pause, revoke_due, propose)]

#### F37.10 `BRICK_MIGRATION_DRAFTED`
A person ran `crane brick migrate`: active Bricks of former kinds were rewritten as drafts of the
current kinds (nothing is signed; an admin activates the drafts and moves the attachments).
- Other [bricks (count), dry_run]

#### F37.11 `BRICK_REQUEST_RECORDED`, `BRICK_REQUEST_REFUSED` (service)
Someone asked for a Brick change on the dashboard, and GitHub's answer at sign-in decided it. The
request is kept for a machine when they have the role (write access for drafts, admin for signed
changes). Otherwise it is refused, as is a request that waited more than ten minutes for sign-in.
Nothing changes until a person applies it on a machine.
- Action [action_type (BRICK_REQUEST), action_source (DASHBOARD), action_user (GitHub login)]
- Assessment [assessment_checks ([GITHUB_REPOSITORY_ROLE])]
- Decision [decision (ALLOW, DENY), decision_rule (GITHUB_WRITE_FOR_DRAFTS_ADMIN_FOR_SIGNED, CODEOWNERS_OF_BRICKS_FOR_SIGNED: the CODEOWNERS line covering .crane/bricks.json names who may request signed changes)]
- Response [response_status (OK, REFUSED), failure_state (NA, ROLE_MISSING)]
- Other [request]

#### F37.12 `BRICK_REQUEST_SETTLED` (service)
A machine reported what happened to a dashboard request: a person there applied it (`crane brick
inbox apply`, or Landscape), it failed with Crane's reason, or they dismissed it. One event per
request. The command it ran notes its own F37 event on that machine.
- Action [action_type (BRICK_REQUEST), action_source (SERVICE)]
- Response [response_status (APPLIED, FAILED, DISMISSED), failure_state (NA, CRANE_REFUSED)]
- Other [request, machine_key]

#### F37.13 `BRICK_PUBLISHED` (service)
An admin's machine published an activated Brick version to the organization catalog ('crane brick
publish'). The service checked the digest and the publisher's signature, and that nothing looks
like a secret. Other repositories import it as a draft.
- Action [action_type (BRICK_CATALOG), action_source (SERVICE)]
- Response [response_status (OK), failure_state (NA)]
- Other [account, brick (ID@N), machine_key]

#### F37.14 `BRICK_PUBLISHED`, `BRICK_IMPORTED` (machine)
On the publishing machine, `BRICK_PUBLISHED` records that 'crane brick publish' sent a version to
the organization catalog (`stored` is false when the same digest was already there). On an
importing machine, `BRICK_IMPORTED` records each draft 'crane brick import' wrote, where it came
from, and its validation. An import grants nothing until an admin there activates it.
- Action [action_type (BRICK_CATALOG), action_source (CLI)]
- Response [response_status (OK), failure_state (NA)]
- Other [brick (ID@N), type, account, stored, from, validation]

### F38. Resident agents

A resident agent is an assignment that stays responsible for one Flow, Zone, or Runbook for days or
weeks while using no model and no process when idle. Its lifecycle commands (`crane resident ...`)
and the worker's activations are noted on the event they belong to (the command's F13 event, or the
`resident.tick` event the background sync stores when a tick did something; idle ticks store
nothing), and every note becomes one F38 event. The durable state lives in the machine's resident
store (`runtime/resident/store.json`); these events are the record of it, never its source. Fields
are identifiers, digests, counts, states, and reason codes; context text, answers, and messages are
never recorded. Every activity event carries `assignment`, `activation`, `trigger`, and (for steps)
`step` as correlation ids.

Common attributes
- Action [action_type (RESIDENT_DEFINE, RESIDENT_LIFECYCLE, RESIDENT_TRIGGER, RESIDENT_ACTIVATION, RESIDENT_CONTEXT, RESIDENT_RCA, RESIDENT_PLAN, RESIDENT_APPROVAL, RESIDENT_ACTION, RESIDENT_VERIFY, RESIDENT_TEST) (enum), action_source (CLI, SYNC, DASHBOARD), action_user (string), action_id (id of the carrying event)]
- Repository [(as F13)]
- Resource [resource ([CRANE_METADATA]), resource_ids (assignment, activation, trigger, definition, RCA graph, and step ids), resource_classification ([METADATA])]
- Response [response_status (OK, PASSED, FAILED, BLOCKED, UNKNOWN, CONTRADICTED, PROPOSAL, APPROVED, DENIED, ANSWERED, ACCEPTED, UNSUPPORTED, DUPLICATE, NO_MATCH, COOLDOWN, RATE_LIMITED, PAUSED, EXPIRED, DEFINITION_MISSING, REVIEWED, REJECTED, APPLIED, REFUSED, PROVEN, PARTIALLY_VERIFIED, CANCELLED, AMBIGUOUS, INSUFFICIENT_EVIDENCE) (enum), failure_state (NA or the failing status), response_statement (string)]
- Eval [context_cache (HIT, MISS), model_calls (integer: model invocations actually made; never inferred)]
- Severity [severity_status (INFO, LOW, MEDIUM), severity_basis (OUTCOME), alert_kind (NONE)]
- Other [assignment, activation, trigger, step, step_type, definition, rca, target, agent, state, from, mode (live, dry_run), source, kind, targets, origin, plan (plan digest), steps, conclusion (IDENTIFIED, AMBIGUOUS, INSUFFICIENT_EVIDENCE), digest, decision (ALLOW, REQUIRE_APPROVAL, UNAVAILABLE, DENY), reasons, approver, basis (DECLARED), pack, refresh_reason, missing, problems, warnings, gaps, model_calls, external_calls, self_reviewed, expires_at, schema_version (each when it applies)]

#### F38.1 `RESIDENT_DEFINITION_SAVED`, `RESIDENT_DEFINITION_ACTIVATED`, `RESIDENT_DEFINITION_REVOKED`
A reusable agent definition version was saved as a draft (it grants nothing), activated by an
admin, or revoked (every assignment of it stops at its next step).
- Other [definition, digest, self_reviewed]

#### F38.2 `RESIDENT_CREATED`, `RESIDENT_VALIDATED`, `RESIDENT_ACTIVATED`, `RESIDENT_PAUSED`, `RESIDENT_RESUMED`, `RESIDENT_RENEWED`, `RESIDENT_EXPIRED`, `RESIDENT_REVOKED`
The assignment lifecycle. Activation and renewal validate again against the signed state of that
moment, and an assignment only acts inside an active signed Brick assignment of its agent.
- Response [response_status (OK, PASSED, FAILED)]
- Other [assignment, definition, target, agent, state, from, expires_at, problems, warnings]

#### F38.3 `RESIDENT_TRIGGER_RECEIVED`, `RESIDENT_TRIGGER_IGNORED`
A trigger was normalized (a local Crane event, a signed generation change, a Brick monitor result, a
timer, or a delivered ci or webhook event), or woke nothing, with the reason.
- Response [response_status (OK, DUPLICATE, NO_MATCH, COOLDOWN, RATE_LIMITED, PAUSED, EXPIRED, DEFINITION_MISSING)]
- Other [trigger, source, kind, targets, origin, assignment, schema_version]

#### F38.4 `RESIDENT_ACTIVATION_QUEUED`, `RESIDENT_ACTIVATION_STARTED`, `RESIDENT_ACTIVATION_COMPLETED`
An activation was created for a matching assignment (one per assignment and trigger), started by the
worker, or ended. Its outcome comes from verification evidence: PROVEN only when every step
succeeded and every verification passed.
- Response [response_status (OK, PROVEN, PARTIALLY_VERIFIED, CONTRADICTED, FAILED, BLOCKED, CANCELLED, UNKNOWN)]
- Eval [model_calls]
- Other [assignment, activation, trigger, kind, mode, state, model_calls, external_calls]

#### F38.5 `RESIDENT_CONTEXT_READY`, `RESIDENT_CONTEXT_CACHE_HIT`, `RESIDENT_CONTEXT_CACHE_MISS`, `RESIDENT_CONTEXT_CACHE_INVALIDATED`
The activation's context pack (agent mission, Flow and Zone context, Brick Common and Current
Context, RCA graph version) was compiled, and how it compares with the pack kept for the
assignment. Provider-side prompt caching is not observed; the cache never authorizes anything.
- Eval [context_cache (HIT, MISS)]
- Other [pack, refresh_reason (definition, assignment, generation, governance, bricks, context, rca), missing]

#### F38.6 `RESIDENT_RCA_SAVED`, `RESIDENT_RCA_PROPOSED`, `RESIDENT_RCA_REVIEWED`
An RCA graph version was saved (proposed until reviewed), an investigation ended in a proposal or a
question instead of a known remediation (insufficient or ambiguous evidence), or a person reviewed a
graph. Review never turns hypotheses into confirmed causes.
- Response [response_status (OK, PROPOSAL, REVIEWED, REJECTED, AMBIGUOUS, INSUFFICIENT_EVIDENCE)]
- Other [rca, target, digest, gaps, self_reviewed]

#### F38.7 `RESIDENT_PLAN_CREATED`
A typed plan was created from the investigation, or extended when a person accepted proposed steps.
- Response [response_status (OK, ACCEPTED)]
- Other [plan, steps, conclusion, rca, problems, model_calls]

#### F38.8 `RESIDENT_APPROVAL_REQUIRED`, `RESIDENT_APPROVAL_DECIDED`
A step waits for a person (an approval bound to the activation, plan digest, step, and signed state;
single use; it expires), or a person approved, denied, or answered it.
- Response [response_status (OK, APPROVED, DENIED, ANSWERED)]
- Other [step, step_type, digest, reasons, approver]

#### F38.9 `RESIDENT_ACTION_STARTED`, `RESIDENT_ACTION_DENIED`, `RESIDENT_ACTION_RESULT`
Crane decided a step immediately before it ran (from the signed state read at that moment), denied
it, or the step reported its result. A step type without an adapter is UNSUPPORTED, never OK.
- Response [response_status (OK, FAILED, UNKNOWN, UNSUPPORTED)]
- Other [step, step_type, decision, reasons, basis (DECLARED when a person reconciled it)]

#### F38.10 `RESIDENT_VERIFICATION_RESULT`, `RESIDENT_RECONCILIATION_REQUIRED`
A verification step evaluated its postcondition (PASSED, CONTRADICTED, or UNKNOWN), or an external
effect is uncertain and waits for a person to establish what happened (it is never retried blindly).
- Response [response_status (PASSED, CONTRADICTED, UNKNOWN)]
- Other [step]

#### F38.12 `RESIDENT_DECISION_APPLIED`
A decision a repository admin made on the dashboard (GitHub verified it when it was made) reached the
machine that holds the activation and was applied there through the same checks as the CLI, or was
refused (the definition does not take dashboard decisions, the plan changed since, separation of
duties, an action the dashboard may not take). Pausing, revoking, and cancelling are always
accepted from the dashboard; approvals and answers only when the definition allows it.
- Response [response_status (APPLIED, REJECTED)]
- Other [decision, action (approve, deny, answer, cancel, pause, revoke), activation, assignment, step, approver, source (DASHBOARD)]

#### F38.13 `RESIDENT_DECISION_RECORDED`, `RESIDENT_DECISION_REFUSED` (service)
The dashboard recorded a resident decision for the machine that holds the activation or assignment
(GitHub confirmed the person administers the repository), or refused it because they do not. Recorded
by the service itself; the machine later notes `RESIDENT_DECISION_APPLIED` when it applies or refuses
it. Decisions wait at most one day.
- Action [action_type (RESIDENT_APPROVAL), action_source (DASHBOARD), action_user (GitHub login)]
- Assessment [assessment_checks ([GITHUB_ADMIN_ROLE])]
- Decision [decision (ALLOW, DENY), decision_rule (GITHUB_ADMIN_REQUIRED)]
- Response [response_status (OK, REFUSED), failure_state (NA, NOT_ADMIN)]
- Other [action, activation, assignment, step, plan]

#### F38.11 `RESIDENT_TESTED`
An assignment was tested against a fixture or replayed trigger in plan-only mode: matching,
deduplication, cooldown, context, investigation, and Crane's decision for every step. Nothing was
executed (there is no sandbox).
- Response [response_status (PASSED, BLOCKED, INCONCLUSIVE)]
- Other [assignment, trigger, model_calls]

---

### F39. Flows as a knowledge base

A flow's knowledge (its approved trace, typed context, priority and model routing, designated
people, tests, and watcher limits) is signed in `.crane/flows.json`. Flow commands
(`crane flow ...`), the hook's agent channel (`crane-request ...`, answered by the hook and never
run), and the read-only watchers note what happened on the event they belong to: the command's F13
event, the agent's `flow.agent_request` event, or the `flow.watchers_evaluated` event the background
sync stores when watchers ran. Every note becomes one F39 event. Fields are identifiers, digests,
counts, states, and codes. Context text, test descriptions, assumptions, mock data, and incident
details are never recorded.

Common attributes
- Action [action_type (FLOW_TRACE, FLOW_APPROVE, FLOW_KNOWLEDGE, FLOW_PROPOSE, FLOW_TEST, FLOW_WATCH, FLOW_INCIDENT, FLOW_REQUEST) (enum), action_source (CLI, HOOK, SYNC), action_id (id of the carrying event)]
- Resource [resource (CRANE_METADATA), resource_ids (flow, watcher, proposal, incident)]
- Other [flow, and the fields of each event below]

#### F39.1 `FLOW_TRACED`, `FLOW_REWOUND`
A flow was traced from its starting points into a draft revision ('crane flow trace|retrace', or an
agent's 'crane-request trace|retrace'), or an earlier revision was made current again. The trace is
deterministic: the same code gives the same digest. A draft grants nothing.
- Response [response_status (OK)]
- Other [revision, nodes, status (RESOLVED, PARTIAL), via (cli, agent), digest]

#### F39.2 `FLOW_APPROVED`, `FLOW_DISCARDED`
A person approved the current revision of a draft. A new flow is added to `.crane/governance.yaml`
with a context pack written from its trace, and the trace is signed into the flow knowledge in the
same generation. Or the person discarded the draft.
- Response [response_status (OK)]
- Other [revision, digest, added, nodes]

#### F39.3 `FLOW_CHANGED`
Signed flow knowledge changed: a priority, a context item, a person, a test, a limit, a model, or
the routing.
- Response [response_status (OK)]
- Other [change (flow.priority, flow.context.add, flow.person.add, flow.test.require, flow.limit, flow.models.add, flow.routing, ...)]

#### F39.4 `FLOW_PROPOSED`, `FLOW_PROPOSAL_DECIDED`
An agent proposed a test (with its assumptions and mock data, explained in plain words) or a context
item for a flow. A person approved it (it becomes signed knowledge, a test optionally required in
every release) or rejected it (the agent's session is told why).
- Response [response_status (OK, REJECTED)]
- Other [proposal, kind (test, context), decision (APPROVED, REJECTED)]

#### F39.5 `FLOW_TESTS_RUN`
A person or CI ran a flow's tests ('crane flow tests run [--required]'). A failing required test
fails the command, so a release step stops.
- Response [response_status (OK, FAILED)]
- Other [tests, passed, failed, required (required tests that did not pass)]

#### F39.6 `FLOW_WATCHER_STARTED`, `FLOW_WATCHER_STOPPED`, `FLOW_WATCHER_EDITED`, `FLOW_WATCHERS_EVALUATED`, `FLOW_SUBAGENT_APPROVED`, `FLOW_SUBAGENT_RUN`, `FLOW_SUBAGENT_TERMINATED`
A subagent of a flow (a Monitor or an Incident subagent; at most 3 per flow, one created by an
agent) was created by an agent or a person and waits for approval. Or it stopped; a person asked to
change its purpose, checks, or configuration ('crane flow subagent configure', requested on DataSea
by a code owner of .crane/flows.json); a person approved or rejected a pending change in Landscape
(the bounds are checked again); it ran its actions from a manual trigger or an incident; it was
terminated because its flow or zone was removed; or the subagents ran their checks (trace drift,
required tests, denials) without a model.
- Response [response_status (OK, FIRING)]
- Other [watcher, by (agent, person), checks, watchers, incidents]

#### F39.7 `FLOW_INCIDENT_RAISED`
A watcher's check started firing, or an agent reported an incident. The flow's designated people
were notified through the integrations (Slack, the Jira issue that names them), and the parent
session receives a notice with its next tool call.
- Response [response_status (FIRING)]
- Severity [severity_status (MEDIUM)]
- Other [incident, code (TRACE_DRIFT, REQUIRED_TEST_FAILING, REQUIRED_TEST_MISSING, DENIALS, AGENT_REPORTED), watcher, notified (count)]

#### F39.8 `FLOW_AGENT_REQUEST`
An agent asked Crane through its hook ('crane-request ...'). The hook answered and stopped the
command, so nothing ran. This is not a denial and not a bypass attempt.
- Response [response_status (OK, REFUSED)]
- Other [request (flows, flow, route, trace, retrace, test, context, watch, unwatch, incident, help)]

## Part 3: Where events appear on the dashboard

Pages are grouped in the left rail (Monitor, AI agents, Investigate, Operate, Govern, Organization).
Groups fold, the rail has its own search, and pages that need attention carry a badge (firing alert
rules, controls not satisfied). Each page is split into zones, one metric or table per panel, and
the zones are listed under the page in the rail. Every panel has:

- an info icon (hover for the explanation, click for the details sidebar);
- a funnel that says in words which events it counts: the time range, every global condition, its
  own conditions, its grouping, or that it shows state that no filter narrows;
- its own conditions (`+ filter`), added to the global filter for that panel only;
- refresh, the lens (a right sidebar with the chart large, its metrics, its properties and filter in
  words, what it is, and an interpretation generated from what it drew), expand, monitor view (full
  screen, refreshed every 30 seconds), and collapse.

Every time is shown as `YYYY:MM:DD:HH:MM:SS:MMM` (local time). Colors follow the Foxx logo (fox orange, ember, cream on
warm charcoal); series use a categorical order led by the Foxx orange that was checked for
color-blind separation in both themes, and states use reserved status colors. The theme is the
system's, or light or dark when chosen.

| Page | Built from |
|---|---|
| **Header and scope** | F00 (sign-in, account, repository, member), F02.7 (linked members) |
| **Repositories** | per repository: count of F02.9 machines, F03 sessions, all events, F05.3 to F05.7 denials, F06.5 bypasses, F14.1 alerts, newest `occurred_at` |
| **Overview** | Health: F05 evals; Agent fleet: tool calls (F05 and F06 traces), tokens (F06 `eval.model_tokens`), tool time; Activity: F03, F04, F05, F10; Security outcomes: F05.4, F06.3 to F06.5, F07.1, F27; Recent violations: F06.4, F06.5; Recent alerts: F14; All evals |
| **Event explorer** | every event, as volume over time (split by any attribute) and as a stream; facets include and exclude values |
| **Timeline** | Gantt chart of traces; event timeline (every event as a dot in lanes); activity rhythm (day of week by hour, and per day); concurrency (distinct sessions and agents per time bucket); tool time, hook latency, and tokens over time |
| **Agents** | fleet numbers; a leaderboard per agent identity (calls, tools, denial and failure shares, tool time, tokens, sessions, quarantines, bypass attempts); events and Gantt by agent; agent by tool and agent by decision matrices; a risk map (calls against denial share); bypass attempts and denials by agent; tokens by agent; models, autonomy modes, and session safety; a profile per agent |
| **Dashboard Agents** | separately stored monitoring rules, action states, incidents, quarantine requests/releases, and flow-context proposals; evaluated after telemetry uploads, at startup recovery, and every 60 seconds; incidents filtered by status, severity, and text; threshold evidence, evaluation metrics, action logs, and chronological 3A notes/resolution. Repository-admin-approved quarantines are checked by local hooks before every tool call. |
| **Tool calls** | every traced tool call (F05 decision to F06 verification): summary, types (by tool, resource kind, decision), metrics per tool, decisions per tool, tools by resource, the waterfall (time per call, packed or in real time, with the tokens of its turn), tokens per call, time against tokens, tool time, and a browser of every call; a profile per tool |
| **Sessions** | summary, activity, active sessions, a leaderboard (calls, time, tokens), the waterfall of one session's calls, and the session list |
| **Session page** | tiles: F03, F05.10, F06.4, F06.5, F27; the waterfall of its tool calls; tool mix; tokens over the session; Gantt; action graph: every event with that `session_id` in `sequence` order; tasks: F04, F07; downloads: F30.3 |
| **Logs** | every event as a log line, with its severity, flow, statement, tokens, stale data, and any attribute as a column |
| **Traces** | events grouped by `action.action_id`; duration histogram and a table filtered by kind, outcome, duration, and span count; a trace view with its tokens and waterfall |
| **Topology** | Who used what (any chain of 2 to 5 attributes); member by agent, tool by decision, and flow by severity matrices; most touched and denied resources; flow dependencies; the code topology (zones and flows of governance.yaml with their call graphs, F28) |
| **Distributions** | histograms of trace duration, gap between session events, events per session, tokens per call, tool time, hook latency, and any numeric `eval.*` attribute; breakdowns by any attribute |
| **Texts and reasons** | the words of `statement`, `reason`, and `response_statement`, the distinct texts, and log lines |
| **Alerting** | rules built with `crane alert` (from the newest status report), rule values over time, firings, deliveries, alert events (F14), hook alerts |
| **Credits** | balances per session (F27), grants made on the dashboard and their outcome |
| **Audit trail** | summary; changes per day by actor; actor by action; the change timeline; the governance changes (F13.3, F18 to F20, F28); command outcomes and events (F13); dashboard actions (service events) |
| **Governance** | state; zones and flows (criticality, autonomy ceiling, files, call graph); flow call graphs; policies with command outcomes; the selection map (by file, operation, language) and relocations; checkpoints and context; integrity by member and evidence chains; governance events (F01, F12, F13, F18 to F20, F28); findings |
| **Compliance** | posture gauge (applicable controls satisfied, partial counting half); outcomes; outcomes per framework (OWASP LLM, OWASP Agentic, NIST AI RMF, SOC 2, ISO/IEC 27001, GDPR) and each requirement; controls as cards or a table; evidence per control and flow; gaps and limitations; inferred observability; what Crane cannot observe |
| **Settings** | Approvals: F02.4 to F02.7, F30.4, F30.5; Members: F02.7, F02.9, F09, F28; Health: F09, F12, F27.10, F29; Data retention; Connection: F21, F02.5; Integrations: F22 |

### Action graph encoding (session page)

| Node | Event |
|---|---|
| large orange dot | F05.1, F05.2, F05.11, F05.12 |
| large amber dot | F05.3, F05.5, F05.6, F05.7, F05.8, F05.9, F16.1 |
| large amber dot with `{bypass_attempt}` | F05.4 |
| large green dot | F06.1 |
| large red dot | F06.4 |
| large pink dot | F06.5, F05.10 |
| small amber dot (gap) | F06.3, F08.1, F11.1, F07.1 |
| small dot (task) | F04.1, F07.2, F07.3, F07.4 |
| small dot (session) | F03.1, F03.2, F08.2, F08.3, F17.1 |
| small dot (signal) | F04.2, F27.4 |
| dashed curve | from an F05 event to the F06 event with the same `action_id` |

---

## Part 4: Implementation notes

### Where each event comes from

Dashboard Agent definitions, incident journals, activity records, evaluation leases, quarantines,
and incident-flow context are stored in `dashboard_agents`, `dashboard_incidents`,
`dashboard_agent_activity`, `dashboard_agent_leases`, `dashboard_quarantines`, and
`dashboard_flow_context`. Activity records expire after 30 days; quarantine and approved context
records persist. Evaluation reads repository-scoped event evidence and writes evaluation counters,
its action record, and any resulting incident and 3A timeline; a repository lease prevents
concurrent evaluation by multiple service instances.

Each local pre-tool hook makes a signed
`GET /v1/repositories/{repository_id}/agent-policy` request before evaluating a tool call. It
receives only active quarantine rules (never telemetry); matching agent/session/tool targets become
a `QUARANTINE` decision, and network, HTTP, or malformed-policy errors deny the call. This is
intentionally online and fail-closed: `CRANE_INGEST_URL=off`, service outages, or a cold-start
timeout prevent tool calls until the policy endpoint responds. Tool quarantines match the exact
provider tool name within the incident's Crane session; session and agent quarantines match stable
Crane session/agent IDs.

Any signed-in dashboard user may request a quarantine or release, but a request remains pending
until the short-lived GitHub OAuth approval flow confirms that the approving account has repository
admin permission. A release request does not deactivate a quarantine; the quarantine remains
enforced until an admin approves its release. The OAuth token is used once for GitHub's permission
check and is not persisted. Incident context proposals contain the incident reason, event IDs, the
3A timeline, and the related response-action audit trail captured at proposal time. They are
returned through the Dashboard Agents API only after the same admin check approves them. Linked
quarantine and context approval/rejection decisions appear in the incident's 3A timeline. These are
service-side context attachments, not edits to `.crane/governance.yaml`.

New incidents attempt Slack and email delivery using server-side configuration. Delivery outcomes
are recorded in service events, never include credentials, and do not roll back incident creation.
The service does not read developer-machine integration secrets. Configure the following in the
hosted service environment (for Render, **Render → Environment**):

| Variable | Meaning |
| --- | --- |
| `CRANE_DASHBOARD_AGENT_SLACK_WEBHOOK_URL` | Slack incoming webhook URL |
| `CRANE_DASHBOARD_AGENT_RESEND_API_KEY` | Resend API key |
| `CRANE_DASHBOARD_AGENT_EMAIL_FROM` | Verified sender address |
| `CRANE_DASHBOARD_AGENT_EMAIL_TO` | Comma-separated recipient list; route to the responsible agent creators/repository owners or their team list |

`GET /api/dashboard-agents?repository=REPOSITORY_ID` returns agent definitions, incidents,
pending/active/released quarantines, activity, and pending/approved flow-context attachments.
Bounded JSON `POST /api/dashboard-agents?repository=...` actions include `save-agent`,
`set-state`, `evaluate`, `open-incident`, `annotate-incident`, `resolve-incident`,
`request-quarantine`, `request-quarantine-release`, and `propose-flow-context`. Admin approvals are
initiated at `/auth/agent-action` and can approve or reject quarantine, release, and flow-context
requests; GitHub must confirm repository-admin access before application.

The envelope's `origin` says which producer wrote an event:

| `origin` | Producer | Stored in | Notes |
|---|---|---|---|
| `MACHINE` | Crane on a member's machine (hooks, commands, the Foxx extension) | `events.spec`, next to the internal event | Computed when the event is appended and covered by its digest, so it chains like the rest of the evidence. Events stored before this specification are projected when they are read. |
| `SERVICE` | The ingestion service (uploads, status reports, sign-in, dashboard views) | `service_events` (schema version 6) | Expire with the retention period, like every telemetry record (see Data retention). |
| `DERIVED` | The dashboard, from other records | nothing (built on every read) | F27 from the credit ledger, F14 from alerts, F13.3 from the governance audit trail, F10 stale and revived sessions, permission outcomes (F16), and ledgers that do not reconcile. |
| `WORKFLOW` | GitHub workflows (F32, F33) | the workflow log | One JSON line per event, prefixed with `crane-event ` (`scripts/workflow_event.py`, or `jq` in the pull request workflow). These are not uploaded. |

### Evals replace the earlier metrics

Every number on the dashboard is an eval computed from the events it covers (`evals.summarize`):
counts by event, flow, decision, severity, enforcement, and alert kind; tool calls decided,
allowed, approvals, denied, and the denial share; bypass attempts and confirmed bypasses, violations
after the effect, coverage gaps, missing decisions, unattributed changes, blocked scripts, and
quarantines; verified calls and evidence completeness; verified and failed tasks; suspicious
prompts, signed attestations, and redacted secrets; latency and tool duration percentiles; credits
(granted, consumed, reserved, revoked, expired, available); models, identity provenance, and inferred
network destinations. An eval that has no events behind it is `null`, never zero.

The metric formulas, the metric rollups each upload carried, the `metric_rollups` collection
(dropped by the schema migration), and the `/api/metrics` route are gone. `/api/definitions`
returns the specification version and the 13 groups.

### Differences from Part 2

- **F26:** a cache `HIT` is not recorded, because it happens on every hook call. Only `MISS` and
  `REFRESHED` are.
- **F09.7:** expiry is not recorded, because MongoDB's TTL index deletes documents without notice.
- **`failure_state`:** a command failure that fits none of the listed values is `OTHER`.
- **`response_statement`:** Crane writes it from the outcome (at most ten words). Hooks do not
  carry the agent's own words.
- **`repository.repo`:** a machine cannot tell a personal account from an organization. Machine
  events are given `PERSONAL` or `ORGANIZATION` when the dashboard serves them, from the
  viewer's account.
- **`session_type` `MULTI_AGENT`:** not detected yet.
- **F34:** only triggers that started an upload are recorded. The service records them from the
  upload itself. `NOT_DUE` would be recorded on every hook call, and `UPLOADS_OFF` and
  `SPAWN_FAILED` never reach the service.
- **F31:** events are recorded only in a folder where Crane is set up. A setup that fails before
  that (for example `NO_REMOTE`) leaves no event.
- **F32:** `FORK` is never recorded, because pull requests from forks do not run the notification
  job.
- **F33.1:** `tests_passed` and `tests_failed` are not reported.
- **F33.2:** a failed build in the build matrix stops the publish job, so the release records no
  event. The build job shows the failure.
- **F00:** sign-in events belong to no repository. A person sees their own on the Settings page
  ("Your sign-ins").
- **Notes:** facts found deep in the code (keys, checkpoint checks, owner keys, policy results) are
  attached to the next event the same Crane process stores. If that process stores no event, the
  facts are not recorded.

### Filters

Every page and every API route takes the same filter, applied to the event stream before anything
is counted (`telemetry/query.rs`; the alert rules use the same code).

- `filter`: a JSON list of clauses `{"path": "...", "op": "...", "values": [...], "group": N}`. The
  path is any attribute (`decision.decision`, `actor.agent_id`, `eval.latency_ms`, ...) or a facet
  name (`flow`, `severity`, `decision`, `agent`, `session`, `tool`, `status`, `origin`, `event`,
  `failure_state`, `enforcement`, `alert_kind`).
- Operators: `is`, `not`, `contains`, `not_contains`, `prefix`, `suffix`, `like` (a pattern where
  `*` is any text and `?` one character, matching the whole value), `gt`, `gte`, `lt`, `lte`,
  `between` (two numbers, both included, in either order), `exists`, `missing`. Comparisons are
  numeric when both sides are numbers, else by text, ignoring case.
- `"exclude": true` negates a clause: an event passes when the condition does not hold (an event
  without the attribute passes `not (tool is READ)`).
- Combination: the values of one clause are OR-ed; clauses are AND-ed; clauses that share a `group`
  number are OR-ed with each other. With exclusion, every combination of conditions can be written
  (as an AND of OR groups). Any mix of these can be active at once.
- `since` and `until` (Unix milliseconds), `q` (text anywhere in the event), and the shortcuts
  `session`, `task`, `name`, and one parameter per facet (`?flow=F05,F06`) are AND-ed with the
  list.
- `/api/attributes` lists every attribute present in the filtered events: its kind, how many events
  have it, its distinct values (identifiers excepted), and the range of numbers.

On the dashboard, the filter bar shows each clause as a pill that can be edited, negated (¬),
switched off, or removed. The condition builder chooses the attribute, an operator that fits its
kind, the values, NOT, and whether it is AND-ed or OR-ed with another condition, and previews the
condition in words. Facet values can be included or excluded (⊖), and any value in an event can be
kept or excluded from every view. A panel can add conditions for itself only. Filters can be saved
as named views (in the browser) and shared as a link that carries the time range and every
condition. A custom time range takes two exact times. The filter is kept for the browser tab.

### New API routes

`/api/gantt`, `/api/timeline`, `/api/traces`, `/api/traces/{action_id}`, `/api/topology`,
`/api/dependencies`, `/api/histogram`, `/api/semantics`, `/api/attributes`, `/api/alerting`,
`/api/grants`, `/api/matrix`, `/api/calendar`, `/api/usage`, and `/api/retention`. They are
read-only and take the filter above.

- `/api/series` takes `by` (a facet or a path) and, for a metric, `metric` (an attribute path) and
  `agg`: `count`, `sum`, `avg`, `min`, `max`, `p50`, `p95`, `p99`, or `distinct` (how many
  different values: distinct `session.session_id` is how many sessions were active). A bucket
  without a value is `null`; a group without any value is left out.
- `/api/matrix` counts events by two attributes (`rows`, `cols`; `limit`; `hide_none=1`), with the
  denied events of each cell.
- `/api/calendar` counts events by day of the week and hour (`tz_offset`, as the browser reports
  it), and per day.
- `/api/usage` gives the metrics of tool calls grouped by `by` (`tool`, `agent_id`, `session`,
  `model`, `member`): calls, denied, approvals, failed, sessions, agents, tool time and hook latency
  percentiles, and tokens.
- `/api/topology` takes `layers` (2 to 5 facets or paths). Traces carry their agent, model,
  resources, targets, tool time, hook latency, and tokens.
- The Gantt chart and the event timeline take `rows`, `lanes`, and `color` (a facet or a path), and
  the histogram takes `field` and `bins`.

### Model tokens

Claude Code passes every hook the path of its transcript. After a tool runs, Crane reads the end of
the transcript and finds the model turn whose `tool_use` block has the call's id; it keeps only the
numbers of that turn's usage (input, output, cache read, and cache creation tokens) and the model,
never a prompt, reply, or tool content. A turn that issued several tool calls is shared equally among
them (`eval.model_tokens` and the four parts are the call's share; `turn_tokens` and
`turn_tool_calls` describe the whole turn), so the shares of a session add up to the tokens of its
tool-issuing turns. When the session ends, `SESSION_ATTESTED` carries the totals of the whole
transcript (`session_tokens`, `session_input_tokens`, `session_output_tokens`,
`session_cache_read_tokens`, `session_cache_creation_tokens`, `session_model_turns`,
`session_tool_calls_issued`). Codex does not report a transcript in this format, so its calls carry
no tokens (the measurement is recorded as unsupported). Tokens are no longer listed as unobservable
in compliance; memory is.

`SHELL_EFFECT_ATTRIBUTED`, the companion of a shell call's verification, now carries only its own
number (`files_changed`), so tool time and tokens are counted once per call.

### Data retention

MongoDB keeps telemetry for the retention period, 30 days by default
(`CRANE_MONGODB_RETENTION_DAYS`, fractions allowed; `CRANE_MONGODB_EVENT_RETENTION_DAYS` is still
read): events, service events, and alerts from their time; sessions, agents, and tasks from their
last activity. A TTL index on `expires_at` deletes them. Records older than the period less 7 days
are stale data: the dashboard marks them (`STALE_DATA`) on events and in the log table, and Settings
shows each kind of record with how many are stale and when the oldest goes. State and history never
expire: repositories, chain heads, the autonomy ledger, the selection registry, policy versions, the
governance audit trail, status reports, approvals, owner keys, and credit grants.

At every start the service brings stored records in line: records without an expiry get one, and
all are recomputed when the period changed. `CRANE_MONGODB_RETENTION_DAYS=persist` (or `forever`,
`off`, `0`) keeps everything, for a larger cluster, and removes every expiry already set.

Expiry never looks like tampering: the dashboard verifies each member's chain from the oldest event
it still holds (`expired_before` says how many came before), and a gap after it is still a break.
A machine does not upload expired records again: events older than the period are not stored, and
the last of them is remembered (`chain_heads.expired_through`), as is the count of alerts uploaded,
so the upload positions never go back. Recent events that are lost are still re-sent.

### Alert rules (`crane alert`)

Rules are built from the command line, never on the dashboard, and only by a person (an agent is
refused):

```
crane alert add NAME metric METRIC above|at_least|below|at_most|equals N [window 15m]
                [channels slack,whatsapp] [severity high] [cooldown 30]
                [where PATH OPERATOR VALUES]... [except PATH OPERATOR VALUES]... [quiet-resolve]
crane alert list|check [--json]
crane alert remove|enable|disable|test NAME
crane alert log [--json]
crane alert metrics
```

- A metric is any number of the evals (`crane alert metrics` lists them), computed over the events
  of the window that match the rule's `where` clauses and none of its `except` clauses. Operators:
  `is`, `not`, `contains`, `not-contains`, `prefix`, `suffix`, `like`, `gt`, `gte`, `lt`, `lte`,
  `between LOW,HIGH`, `exists`, `missing`.
- Rules are kept in the trust directory (`alert-rules.json`). The background sync evaluates them
  (and `crane alert check` does so at once). A rule is `OK` or `FIRING`. A ratio or percentile with no events in the window has no value, and the state stays as it was. When it starts
  firing, Crane sends the message to the rule's channels (`crane integrate slack|whatsapp` holds the
  destination) and records the HTTP status and time taken. Crane sends a reminder after each
  cooldown while the rule keeps firing, and a resolved message when it stops, unless
  `quiet-resolve` is given.
- Each evaluation is logged (`alert-evaluations.jsonl`, the newest 1000), and each message is
  recorded as an F14 alert of kind `rule`. The newest status report carries the rules, their
  state, and the history to the dashboard.

New F14 event names: `ALERT_RULE_FIRED`, `ALERT_RULE_RESOLVED`, `ALERT_RULE_TESTED`. Their `other`
group carries `rule_name`, `rule_metric`, `rule_op`, `rule_threshold`, `rule_window_minutes`,
`rule_channels`, and `rule_filter`. Their `eval` group carries `rule_value`, `rule_threshold`, `http_status`, and
`delivery_ms`. A delivery that was sent without waiting is `QUEUED` (no longer counted as failed).

### Credit grants

An admin or owner of the repository (GitHub admin access, checked again when the grant is made) can
grant credits to a session on the Credits page. The grant goes through the same sign-in approval as
other dashboard changes. It is stored as `PENDING` in `credit_grants` (schema version 7) and
expires after 7 days. The service records `CREDITS_GRANT_APPROVED` or `CREDITS_GRANT_REFUSED`
(F27). The next upload of the machine that owns the session applies the grant to the credit ledger.
The ledger entry reads "granted on the dashboard by @login". The grant becomes `APPLIED`, or
`REJECTED` when the session is not on that machine. Amounts are 1 to 1000.

### Brick requests

People change Bricks from the Bricks page as requests (schema version 12, `brick_requests`).

- **Staging.** `POST /api/bricks {action: stage, op, params, reason}` checks the request with the
  rules the machine checks again (`src/brick/requests.rs`) and keeps it as `STAGED`. Then
  `/auth/bricks` signs the person in with GitHub. Write access confirms a draft and admin access a
  signed change, and the request becomes `PENDING` for 14 days, or `REFUSED` (F37.11).
- **Applying.** Any machine of the repository reads waiting requests with `GET brick-requests`.
  A person applies them there with `crane brick inbox apply` or Landscape. The result goes back
  with `POST brick-requests` (`APPLIED`, `FAILED`, or `DISMISSED`, F37.12).
- **Withdrawing.** `POST /api/bricks {action: cancel}` withdraws your own waiting request.
- **Retention.** Settled requests are kept for 90 days, and unconfirmed ones for an hour.
- **What the page shows.** The Bricks status now carries each Brick's configuration and Common
  Context text, each attachment's Current Context text, and the Zones and Flows. Text or a
  configuration that looks like a secret is withheld, and text over 16 KB is cut.

### Compliance

- **CR-10** is removed: it repeated CR-09.
- A control whose inputs do not exist is `NOT_APPLICABLE`, not failed. Example: alerting controls
  when no rule or integration is set up.
- Each control lists the flows it is evaluated from, how to fix it, and its limitations. Example:
  CR-13 can only screen prompts that pass through the hooks.
- **Inferred observability** lists network destinations inferred from tool calls
  (`NETWORK_DESTINATION_INFERRED`). They are inferred, not observed.
