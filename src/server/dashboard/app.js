"use strict";
// The DataSea dashboard: the core. Sign in with GitHub, choose an account, a repository, and optionally
// one machine, then browse pages grouped in the left rail; each page is split into zones (listed
// under it in the rail), and each zone into panels focused on one thing. A time range and a global
// filter (any attribute of DASHBOARD.md, any operator, AND, OR groups, and NOT, saved as views and
// shared as links) apply to every event view; any panel can add its own conditions. Every panel says
// in words what it shows and how it is filtered (the funnel icon), and can be refreshed, opened in
// the lens (a right sidebar with the chart, its metrics, properties, explanation, and an
// interpretation of what it shows), expanded, shown in monitor mode, or collapsed. Every time is
// written as YYYY:MM:DD:HH:MM:SS:MMM. Charts come from charts.js, explanations from glossary.js, pages
// from pages.js and govern.js. The only writes are approvals and credit grants, which GitHub
// authorizes.

const { h, svg, put, dt, duration, fmt, compact, tokens, color, hashColor, tooltip, tip, charts } = window.Foxx;
const G = window.FOXX_GLOSSARY || { flows: {}, groups: {}, values: {}, evals: {}, facets: {} };

// Explanations the specification does not generate: agent metrics and dashboard states
Object.assign(G.evals, {
  model_tokens: "Tokens of the model turn that issued this tool call, shared equally by the calls of that turn (input + output + cache read + cache creation), read from the agent's transcript when the provider exposes usage. Unsupported providers are not estimated.",
  model_input_tokens: "Input tokens of the issuing turn (not read from the prompt cache), this call's share.",
  model_output_tokens: "Output tokens the model generated in the issuing turn, this call's share.",
  cache_read_tokens: "Input tokens read from the prompt cache in the issuing turn, this call's share. Cheap and fast: a high share means the context was reused.",
  cache_creation_tokens: "Input tokens written to the prompt cache in the issuing turn, this call's share.",
  turn_tokens: "Every token of the model turn that issued the call (not shared).",
  turn_tool_calls: "How many tool calls the issuing model turn made; its tokens are shared among them.",
  session_tokens: "Every token of the session, from its transcript when the session ended.",
  tool_duration_ms: "How long the tool ran: from the pre-tool hook (Crane's decision) to the post-tool hook.",
  latency_ms: "How long Crane's hook took to decide (the overhead Crane adds to a tool call).",
  tool_calls: "Tool calls (traces with a tool) in range and filter.",
});
Object.assign(G.values, {
  STALE_DATA: "Older than the retention period less 7 days: MongoDB deletes it within a week (30 days after its time by default). Kept for ever once the service runs with CRANE_MONGODB_RETENTION_DAYS=persist.",
  EXPIRES: "Telemetry records expire after the retention period.",
  PERSIST: "Every record is kept for ever.",
  TOOL_CALL: "One call of a tool by an agent: Crane's decision before it, the tool running, and the verification after it.",
});

// ---------------------------------------------------------------- pages and their zones

const ICONS = {
  overview: "M3 3h7v7H3zM14 3h7v4h-7zM14 10h7v11h-7zM3 13h7v8H3z", explorer: "M4 5h16M4 10h16M4 15h10M4 20h7", timeline: "M4 6h10M8 12h12M4 18h7M4 3v18",
  logs: "M5 4h14v16H5zM8 8h8M8 12h8M8 16h5", traces: "M4 6h6v4H4zM10 8h4M14 6h6v4h-6zM7 10v6h6", topology: "M5 5a2 2 0 1 0 0 .1M19 5a2 2 0 1 0 0 .1M12 19a2 2 0 1 0 0 .1M6 6l5 11M18 6l-5 11",
  distributions: "M4 20V12M9 20V6M14 20V10M19 20V15M3 20h18", semantics: "M4 5h16v10H9l-5 4zM8 9h8M8 12h5", sessions: "M4 4h16v12H4zM8 20h8M12 16v4",
  agents: "M12 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM4 20c1-4 4-6 8-6s7 2 8 6", tools: "M14 6l4 4-8 8H6v-4zM16 4l4 4", alerting: "M12 3a6 6 0 0 1 6 6v4l2 3H4l2-3V9a6 6 0 0 1 6-6zM10 19a2 2 0 0 0 4 0",
  credits: "M12 3v18M17 7H9.5a3 3 0 0 0 0 6h5a3 3 0 0 1 0 6H6", audit: "M6 3h9l4 4v14H6zM9 9h6M9 13h6M9 17h4", governance: "M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z",
  compliance: "M5 12l4 4 10-10", repositories: "M4 4h6l2 2h8v14H4z", settings: "M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2",
};
const NAV = [
  ["Monitor", [
    ["overview", "Overview", [["health", "Health"], ["fleet", "Agent fleet"], ["activity", "Activity"], ["decisions", "Decisions"], ["security", "Security"], ["people", "Agents and members"], ["recent", "Recent findings"], ["evals", "All evals"]]],
    ["explorer", "Event explorer", [["volume", "Volume"], ["stream", "Event stream"]]],
    ["timeline", "Timeline", [["gantt", "Gantt chart"], ["swimlanes", "Event timeline"], ["rhythm", "Activity rhythm"], ["concurrency", "Concurrency"], ["latency", "Latency and tokens"]]],
  ]],
  ["AI agents", [
    ["agents", "Agents", [["fleet-kpis", "Fleet"], ["leaderboard", "Leaderboard"], ["agent-activity", "Activity"], ["agent-tools", "Tools per agent"], ["agent-decisions", "Decisions and risk"], ["agent-tokens", "Tokens"], ["agent-models", "Models and autonomy"], ["agent-list", "Agent identities"]]],
    ["dashboard-agents", "Dashboard agents", [["da-agents", "Agents"], ["da-states", "Action States"], ["da-incidents", "Incidents and 3A"]]],
    ["tools", "Tool calls", [["tool-kpis", "Summary"], ["tool-types", "Types of tool calls"], ["tool-metrics", "Metrics per tool"], ["tool-waterfall", "Waterfall"], ["tool-tokens", "Tokens per call"], ["tool-time", "Time per call"], ["tool-browser", "Browse tool calls"]]],
    ["sessions", "Sessions", [["session-kpis", "Summary"], ["session-activity", "Activity"], ["session-leaderboard", "Leaderboard"], ["session-waterfall", "Waterfall"], ["session-list", "Sessions"]]],
  ]],
  ["Investigate", [
    ["logs", "Logs", [["log-table", "Log table"]]],
    ["traces", "Traces", [["trace-durations", "Durations"], ["trace-table", "Trace table"]]],
    ["topology", "Topology", [["who", "Who used what"], ["matrices", "Matrices"], ["hotspots", "Hotspots"], ["dependencies", "Flow dependencies"], ["code", "Code topology"]]],
    ["distributions", "Distributions", [["histogram", "Distribution shapes"], ["breakdowns", "Breakdowns"]]],
    ["semantics", "Texts and reasons", [["words", "Words"], ["texts", "Statements and reasons"], ["lines", "Log lines"]]],
  ]],
  ["Operate", [
    ["alerting", "Alerting", [["rules", "Rules"], ["rule-values", "Rule values"], ["firings", "Firings"], ["deliveries", "Deliveries"], ["alert-events", "Alert events"], ["hook-alerts", "Hook alerts"], ["build", "Build a rule"]]],
    ["credits", "Credits", [["credit-summary", "Summary"], ["balances", "Balances"], ["grants", "Grants"]]],
  ]],
  ["Govern", [
    ["audit", "Audit trail", [["audit-summary", "Summary"], ["audit-activity", "Activity"], ["audit-matrix", "Who changed what"], ["audit-timeline", "Change timeline"], ["audit-trail", "Governance changes"], ["command-events", "Commands"], ["dashboard-actions", "Dashboard actions"]]],
    ["governance", "Governance", [["gov-state", "State"], ["gov-zones", "Zones and flows"], ["gov-graphs", "Flow call graphs"], ["gov-rules", "Policies and selections"], ["gov-map", "Selection map"], ["gov-more", "Checkpoints and context"], ["gov-integrity", "Integrity"], ["gov-events", "Governance events"], ["gov-findings", "Findings"]]],
    ["bricks", "Bricks", [["brick-state", "State"], ["brick-library", "Library"], ["brick-attachments", "Attachments and Stacks"], ["brick-agents", "Agents and monitors"], ["brick-context", "Context snapshots"], ["brick-history", "History and events"], ["brick-build", "Build a draft"]]],
    ["compliance", "Compliance", [["posture", "Posture"], ["frameworks", "Frameworks"], ["controls", "Controls"], ["evidence", "Evidence"], ["gaps", "Gaps and limitations"], ["inferred", "Inferred observability"], ["unobservable", "Not observable"]]],
  ]],
  ["Organization", [
    ["repositories", "Repositories", []],
    ["settings", "Settings", [["approvals", "Approvals"], ["members", "Members"], ["health-zone", "System health"], ["retention", "Data retention"], ["integrations", "Integrations"], ["signins", "Your sign-ins"]]],
  ]],
];
const PAGE_GUIDE = {
  overview: "The repository at a glance, for the time range and filter: how many tool calls Crane decided and how, what the agents did and spent, security outcomes, who was active, and the newest violations and alerts. Every number comes from the event stream of DASHBOARD.md; click one to see the events behind it.",
  explorer: "Every event, newest first, with a count for each value of each facet on the left. Tick values to filter every view (⊖ excludes a value); open an event to see all thirteen attribute groups.",
  timeline: "When things happened. The Gantt chart draws each traced action as a bar; the event timeline draws every event as a dot; the rhythm heatmap shows the days and hours agents work; concurrency counts the sessions active at once; latency and tokens follow the cost of each call over time. Drag across a time chart to zoom in.",
  agents: "Every AI agent: its identities, sessions, tool calls, decisions, tokens, models, and risk signals. Click an agent anywhere for its profile.",
  "dashboard-agents": "Create and edit monitor-only agents for flows, zones, selections, or the whole repository. The service evaluates enabled agents after telemetry uploads, at startup, and on a 60-second recovery sweep. Review their counters and activity log, inspect incidents and the chronological Agent Action Analysis (3A), then add human analysis or mitigation notes. Quarantine, prediction models, and external messaging are not executed by this monitor-only dashboard.",
  tools: "Every tool call agents made: what kind of calls, how each tool performs (calls, denials, failures, time, tokens), the waterfall of the time each call took and the tokens its model turn burnt, and a browser of every call with its trace.",
  logs: "A detailed log table of the events matching the global filter. Add any attribute as a column, filter columns, sort, search, and export CSV.",
  traces: "A trace is every event of one tool call, command, or alert (joined by action.action_id). The table lists traces with their duration, outcome, tokens, and events; open one for its waterfall.",
  topology: "Who used what (any layers: members, agents, tools, resources, flows, decisions, …), matrices of any two attributes, the resources agents touch most, which flow follows which, and the code topology: the repository's zones and flows with their call graphs.",
  distributions: "How numbers are spread: trace durations, gaps between a session's events, events per session, and every numeric eval attribute (hook latency, tool duration, tokens, …).",
  semantics: "The texts events carry: response statements, decision reasons and rules, failure states, assessments, end reasons, and the words used most often.",
  sessions: "Agent sessions with their status, safety, tool calls, tokens, and evals. Open one for its timeline, waterfall, Gantt chart, action graph, tasks, and downloadable report and evidence.",
  alerting: "Alert rules are built on a member's machine with 'crane alert add' (never here): a metric, a threshold, a window, a filter, and Slack or WhatsApp. Each background sync evaluates them; this page shows every rule, its value against its threshold, every firing, and every delivery with its HTTP result.",
  credits: "Autonomy credits bound what an agent may do on its own. Admins and owners of the repository can grant credits to an agent's session here; GitHub confirms the role, and the session's machine applies the grant at its next sync.",
  audit: "Who changed the rules and when: the signed governance history, who changed what, a timeline of changes, every Crane command, and what was done on this dashboard (sign-ins, approvals, grants).",
  governance: "The governance state as last reported by a member's machine: zones and flows with their criticality, autonomy ceilings, and call graphs; policies and their results; where every selection is; checkpoints; context; integrity by member; and findings.",
  compliance: "Each control is evaluated only from evidence Crane holds. The posture gauge is the share of applicable controls satisfied; frameworks show which requirements the evidence supports; gaps can be fixed; limitations are inherent to the method and never lower an outcome.",
  repositories: "Every repository of the account where a member's Crane uploaded and proved its GitHub remote.",
  settings: "Approvals, members and their machines, system health, how long data is kept, integrations, and your own sign-ins.",
};

// ---------------------------------------------------------------- state

const RANGES = [["15m", 15 * 60e3], ["1h", 3600e3], ["4h", 4 * 3600e3], ["1d", 86400e3], ["7d", 7 * 86400e3], ["30d", 30 * 86400e3], ["All", null]];
const FACETS = [["severity", "severity.severity_status"], ["flow", "flow"], ["decision", "decision.decision"], ["enforcement", "decision.enforcement"], ["alert", "severity.alert_kind"], ["origin", "origin"], ["agent", "agent.agent_type"], ["source", "action.action_source"], ["event", "event_name"], ["response", "response.response_status"], ["tool", "action.action_tool"], ["member", "member_key"]];
const FACET_LABELS = { severity: "Severity", flow: "Flow", decision: "Decision", enforcement: "Enforcement", alert: "Alert kind", origin: "Origin", agent: "Agent type", source: "Action source", event: "Event name", response: "Response", tool: "Tool", member: "Member" };
const OPS = { is: "is", not: "is not", contains: "contains", not_contains: "does not contain", prefix: "starts with", suffix: "ends with", like: "matches", gt: ">", gte: "≥", lt: "<", lte: "≤", between: "is between", exists: "exists", missing: "is missing" };
const OPS_BY_KIND = { text: ["is", "not", "contains", "not_contains", "prefix", "suffix", "like", "exists", "missing"], number: ["is", "not", "gt", "gte", "lt", "lte", "between", "exists", "missing"], boolean: ["is", "not", "exists", "missing"], list: ["is", "not", "contains", "not_contains", "prefix", "suffix", "like", "exists", "missing"] };
const NO_VALUE_OPS = ["exists", "missing"];
const TONES = {
  ALLOW: "ok", COMPLIANT: "ok", SATISFIED: "ok", PASS: "ok", ACTIVE: "ok", OBSERVED: "ok", verified: "ok", LINKED: "ok", OK: "ok", SUCCESS: "ok", COMPLETED: "ok", SIGNED: "ok", PREVENTED: "ok", DELIVERED: "ok", APPLIED: "ok", RESOLVED: "ok", RESOLVED_GRAPH: "ok", EXPIRES: "info", PERSIST: "ok",
  INFO: "info", MACHINE: "info", SERVICE: "info", DERIVED: "info", WORKFLOW: "info", QUEUED: "info", ENDED: "info", LOW: "low", TOOL_CALL: "brand",
  REQUIRE_APPROVAL: "warn", DEGRADED: "warn", PARTIAL: "warn", INFERRED: "warn", PENDING: "warn", SUSPICIOUS: "warn", COVERAGE_GAP: "warn", STALE: "warn", STALE_DATA: "warn", NO_EVIDENCE: "warn", MEDIUM: "warn", UNLINKED: "warn", UNSIGNED: "warn", UNKNOWN: "warn", DRY_RUN: "warn", NOT_APPLICABLE: "", proposed: "warn", sensitive: "warn",
  DENY: "bad", VIOLATION: "bad", FAIL: "bad", NOT_SATISFIED: "bad", DETECTED_AFTER_EFFECT: "bad", UNRESOLVED: "bad", HIGH: "bad", ERROR: "bad", missing: "bad", broken: "bad", FAILED: "bad", FORBIDDEN: "bad", BLOCKED: "bad", BYPASS_ATTEMPT: "bad", REFUSED: "bad", FIRING: "bad", REJECTED: "bad", critical: "bad", restricted: "crit",
  QUARANTINE: "crit", QUARANTINED: "crit", BYPASS_CONFIRMED: "crit", CRITICAL: "crit",
};
const scope = { account: "", repository: "", member: "" };
window.FOXX_REPOSITORY_ID = () => scope.repository;
const filter = { range: "7d", since: null, until: null, q: "", clauses: [] };
let me = null;
let members = [];
let catalog = null;
let facetCache = null;
let lastEvals = null;
let approval = null;
let live = null;
let retentionInfo = null;
{
  const params = new URLSearchParams(location.search);
  if (params.has("approval")) {
    approval = { ok: params.get("ok") === "1", message: params.get("approval") };
    history.replaceState(null, "", location.pathname + location.hash);
  }
}
const storage = {
  load(key, into, area) { try { Object.assign(into, JSON.parse((area || sessionStorage).getItem(key) || "{}")); } catch (error) { /* storage unavailable */ } },
  save(key, value, area) { try { (area || sessionStorage).setItem(key, JSON.stringify(value)); } catch (error) { /* storage unavailable */ } },
  read(key, fallback, area) { try { const text = (area || localStorage).getItem(key); return text ? JSON.parse(text) : fallback; } catch (error) { return fallback; } },
};
storage.load("foxx-scope", scope);
storage.load("foxx-filter-v2", filter);
if (!Array.isArray(filter.clauses)) filter.clauses = [];
const areaFilters = storage.read("foxx-area-filters", { pages: {}, zones: {} });
if (!areaFilters.pages || !areaFilters.zones) Object.assign(areaFilters, { pages: {}, zones: {} });
let currentDashboardPage = "overview";
const saveScope = () => storage.save("foxx-scope", scope);
const saveFilter = () => storage.save("foxx-filter-v2", filter);

// ---------------------------------------------------------------- small helpers

function chip(value, title) { return value === null || value === undefined || value === "" ? h("span", { class: "muted" }, "—") : h("span", { class: "chip " + (TONES[value] || ""), title: title || meaning(value) || undefined }, String(value)); }
function member(value) { return value ? h("span", { class: "member", title: value.key }, value.actor + " ", h("code", { class: "muted" }, value.short)) : h("span", { class: "muted" }, "—"); }
function percent(value) { return value === null || value === undefined || Number.isNaN(value) ? "—" : (value * 100).toFixed(1) + "%"; }
function link(text, hash) { return h("a", { href: hash }, text); }
function go(hash) { if (location.hash === hash) render(); else location.hash = hash; }
function sessionOf(spec) { return spec.session && spec.session.session_id ? spec.session.session_id : null; }
function shortIdentity(id) {
  const value = String(id || "").replace(/^[a-z]+_/, "");
  return value.slice(0, 4) || "????";
}
function compactIdentity(id, type) {
  if (!id) return "—";
  const value = String(id);
  return value.length > 12 ? type + "_" + shortIdentity(value) : value;
}
const agentProviders = new Map();
function rememberAgents(rows) {
  for (const row of rows || []) if (row.agent_id && row.provider) agentProviders.set(row.agent_id, row.provider);
}
function agentName(id, provider) {
  const kind = String(provider || agentProviders.get(id) || "").toLowerCase();
  if (kind.includes("claude")) return "Claude";
  if (kind.includes("codex")) return "Codex";
  if (kind.includes("copilot")) return "Copilot";
  if (kind.includes("gemini")) return "Gemini";
  return compactIdentity(id, "agent");
}
function facetPath(name) { const found = FACETS.find(([facet]) => facet === name); return found ? found[1] : name; }
function facetName(path) { const found = FACETS.find(([, known]) => known === path); return found ? found[0] : null; }
function attrLabel(path) { const facet = facetName(path); return facet ? FACET_LABELS[facet].toLowerCase() : path; }
function memberLabel(key) { const row = members.find((item) => item.member.key === key); return row ? row.member.actor + " (" + row.member.short + ")" : String(key).slice(0, 8); }
function valueLabel(path, value) {
  if (path === "member_key") return memberLabel(value);
  if (path === "agent.agent_id") return agentName(value);
  if (path === "session.session_id") return compactIdentity(value, "session");
  return value;
}
function sessionLink(id, session) {
  if (!id) return h("span", { class: "muted" }, "—");
  const label = compactIdentity(id, "session");
  const summary = session ? [
    session.provider,
    session.model,
    session.safety && "safety " + session.safety,
    session.autonomy_mode && session.autonomy_mode + " mode",
    session.started_at && "started " + dt(session.started_at),
    session.ended_at && "ended " + dt(session.ended_at),
  ].filter(Boolean).join(" · ") : "Internal session identity. Open the session to see its activity, safety state, model, and credit history.";
  return h("span", { class: "identity-link" }, link(label, "#/sessions/" + encodeURIComponent(id)),
    info("", { title: label, text: summary }));
}
function agentLink(id, provider) {
  if (!id) return h("span", { class: "muted" }, "—");
  const label = agentName(id, provider);
  return h("span", { class: "identity-link" }, link(label, "#/agents"),
    info("", { title: compactIdentity(id, "agent"), text: "Stable internal agent profile identity " + compactIdentity(id, "agent") + ". Provider: " + (provider || agentProviders.get(id) || "unknown") + ". The full identity is retained internally for filtering and trace correlation." }));
}
function consumedCreditsBy(entries, calls, dimension) {
  const toolsByCall = new Map((calls || []).filter((call) => call.tool_call_id && call.tool).map((call) => [call.tool_call_id, call.tool]));
  const totals = new Map();
  for (const entry of entries || []) {
    const kind = String(entry.kind || "").toUpperCase();
    const delta = kind === "CONSUME" ? Number(entry.amount || 0) : kind === "REFUND" ? -Number(entry.amount || 0) : 0;
    if (!delta) continue;
    if (dimension === "tool" && entry.tool_call_id && !toolsByCall.has(entry.tool_call_id)) continue;
    const key = dimension === "tool"
      ? (toolsByCall.get(entry.tool_call_id) || "Unattributed")
      : (entry.session || "Unknown session");
    totals.set(key, (totals.get(key) || 0) + delta);
  }
  return [...totals].map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
}
const is = (path, value) => ({ path, op: "is", values: [value] });
const SVG_ICON = (d) => svg("svg", { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" }, svg("path", { d }));
const FUNNEL = "M3 5h18l-7 8v6l-4 2v-8z", LENS = "M11 4a7 7 0 1 1 0 14 7 7 0 0 1 0-14zM20 20l-4-4M8 11h6M11 8v6";

// ---------------------------------------------------------------- explanations (info icons)

/** What a value means, from the glossary */
function meaning(value) {
  if (value === null || value === undefined) return "";
  const text = String(value);
  if (/^F\d\d$/.test(text) && G.flows[text]) return text + " · " + G.flows[text][0] + ". " + G.flows[text][1];
  if (/^F\d\d\.\d+$/.test(text) && G.flows[text.slice(0, 3)]) return text + " · a step of " + text.slice(0, 3) + " (" + G.flows[text.slice(0, 3)][0] + ").";
  return G.values[text] || "";
}
/** Look up an explanation by key: flow:F05, value:HIGH, eval:denied, facet:tool, group:decision, attr:decision.decision, or page:NAME */
function explain(key) {
  const [kind, rest] = [key.slice(0, key.indexOf(":")), key.slice(key.indexOf(":") + 1)];
  if (kind === "flow" && G.flows[rest]) return { title: rest + " · " + G.flows[rest][0], text: G.flows[rest][1] };
  if (kind === "value") return { title: rest, text: meaning(rest) || "A value of this attribute." };
  if (kind === "eval") return { title: rest, text: G.evals[rest] || G.evals[rest.replace(/^eval\./, "")] || G.evals[rest.split(".")[0]] || "An eval computed from the events." };
  if (kind === "facet") return { title: FACET_LABELS[rest] || rest, text: G.facets[rest] || "" };
  if (kind === "group" && G.groups[rest]) return { title: rest + " group", text: "The " + rest + " group: " + G.groups[rest].title + "." };
  if (kind === "attr") {
    const [group, sub] = rest.split(".");
    const facet = facetName(rest);
    const definition = sub && G.groups[group] && G.groups[group].keys[sub];
    const evalText = group === "eval" && sub ? G.evals[sub] : "";
    return { title: rest, text: (facet ? G.facets[facet] + " " : "") + (evalText || (definition ? "Values: " + definition : sub ? "A " + group + " attribute specific to some flows." : G.values[rest] || "An envelope attribute.")) };
  }
  if (kind === "page") return { title: rest, text: PAGE_GUIDE[rest] || "" };
  return { title: key, text: rest };
}
/** An info icon: hover for a short explanation, click to read it in the sidebar */
function info(key, custom) {
  const detail = custom || explain(key);
  if (!detail || !detail.text) return null;
  return h("button", { type: "button", class: "info-icon", "aria-label": "About " + detail.title,
    onmouseenter: (event) => tooltip.show(event, tip(detail.title, [], detail.text)), onmousemove: (event) => tooltip.show(event, tip(detail.title, [], detail.text)), onmouseleave: tooltip.hide, onfocus: (event) => { const box = event.target.getBoundingClientRect(); tooltip.show({ clientX: box.right, clientY: box.bottom }, tip(detail.title, [], detail.text)); }, onblur: tooltip.hide,
    onclick: (event) => { event.stopPropagation(); tooltip.hide(); details.open(detail.title, h("div", { class: "explain" }, h("p", {}, detail.text), key.startsWith("flow:") ? h("p", {}, h("button", { class: "btn", onclick: () => { details.close(); setClause("flow", "is", [key.slice(5)]); go("#/explorer"); } }, "Show the events of " + key.slice(5))) : null), { kind: "Explanation" }); } }, "i");
}
/** A value with its info icon, when the glossary explains it */
function explained(value) { return meaning(value) ? info("value:" + value) : null; }

// ---------------------------------------------------------------- the filter in words

function rangeText() {
  if (filter.since || filter.until) return "from " + dt(filter.since) + " to " + (filter.until ? dt(filter.until) : "now");
  if (filter.range === "All") return "of all time";
  const range = RANGES.find(([name]) => name === filter.range);
  return "of the last " + filter.range + (range && range[1] ? " (since " + dt(Date.now() - range[1]) + ")" : "");
}
function clauseText(clause) {
  if (!clause) return "";
  const values = (clause.values || []).map((value) => valueLabel(clause.path, value));
  const body = clause.op === "between" ? attrLabel(clause.path) + " is between " + values.join(" and ") : [attrLabel(clause.path), OPS[clause.op] || clause.op, NO_VALUE_OPS.includes(clause.op) ? "" : values.join(" or ")].filter(Boolean).join(" ");
  return (clause.exclude ? "not (" + body + ")" : body);
}
/** Clauses in words: plain clauses joined with and, OR groups in brackets */
function clausesText(list) {
  const groups = new Map();
  list.filter((clause) => !clause.off).forEach((clause, index) => { const key = clause.group ? "g" + clause.group : "c" + index; if (!groups.has(key)) groups.set(key, []); groups.get(key).push(clause); });
  return [...groups.values()].map((items) => (items.length > 1 ? "(" + items.map(clauseText).join(" or ") + ")" : clauseText(items[0]))).join(", and ");
}
/** What a panel shows and how it is filtered, in words
 * spec: {kind: "events" (default) | "report" | "state" | "all-time" | "custom", what, clauses, notes, ignoreTime} */
function filterSentence(spec) {
  spec = spec || {};
  const parts = [];
  const what = spec.what || "Events";
  if (spec.kind === "report") return (spec.what || "This panel") + " comes from the newest status report a member's machine sent" + (spec.reported ? " (" + dt(spec.reported) + ")" : "") + ". It shows the repository's current state, so the time range and the event filter do not apply." + (spec.notes && spec.notes.length ? " " + spec.notes.join(" ") : "");
  if (spec.kind === "state") return (spec.what || "This panel") + " lists records over all time (not narrowed by the time range or the event filter)" + (scope.member ? ", for the chosen member only" : "") + "." + (spec.notes && spec.notes.length ? " " + spec.notes.join(" ") : "");
  if (spec.kind === "custom") return spec.text;
  parts.push(what + " " + (spec.ignoreTime ? "of any time" : rangeText()));
  const global = filter.clauses.filter((clause) => !clause.off);
  if (global.length) parts.push("where " + clausesText(global));
  if (filter.q) parts.push("containing the text “" + filter.q + "”");
  const local = (spec.clauses || []).filter(Boolean);
  if (local.length) parts.push((global.length || filter.q ? "and, for this panel only, " : "where, for this panel only, ") + clausesText(local));
  if (scope.member) parts.push("from member " + memberLabel(scope.member));
  let sentence = parts.join(" ") + ".";
  if (!global.length && !filter.q && !local.length) sentence += " No filter narrows it.";
  if (spec.notes && spec.notes.length) sentence += " " + spec.notes.filter(Boolean).join(" ");
  return sentence;
}
/** The funnel icon of a panel: hover for its filter in words */
function scopeIcon(getSpec, onClick) {
  const text = () => filterSentence(typeof getSpec === "function" ? getSpec() : getSpec);
  const active = () => { const spec = typeof getSpec === "function" ? getSpec() : getSpec; return (!spec || !spec.kind || spec.kind === "events") && (activeFilters() > 0 || (spec && spec.clauses && spec.clauses.length)); };
  const node = h("button", { type: "button", class: "scope-icon", "aria-label": "What this panel shows and how it is filtered",
    onmouseenter: (event) => { node.classList.toggle("active", Boolean(active())); tooltip.show(event, tip("Filter applied", [], text())); }, onmousemove: (event) => tooltip.show(event, tip("Filter applied", [], text())), onmouseleave: tooltip.hide,
    onfocus: (event) => { const box = event.target.getBoundingClientRect(); tooltip.show({ clientX: box.right, clientY: box.bottom }, tip("Filter applied", [], text())); }, onblur: tooltip.hide,
    onclick: (event) => { event.stopPropagation(); tooltip.hide(); if (onClick) onClick(); else details.open("Filter applied", h("p", { class: "sentence" }, text()), { kind: "Filter" }); } }, SVG_ICON(FUNNEL));
  if (active()) node.classList.add("active");
  return node;
}

// ---------------------------------------------------------------- the details sidebar

const details = {
  open(title, content, options) {
    options = options || {};
    const pane = document.getElementById("details");
    put(pane, h("header", {}, h("div", { style: "flex:1;min-width:0" }, options.kind ? h("div", { class: "kind" }, options.kind) : null, h("h3", {}, title)), options.actions || null, h("button", { class: "icon-btn", "aria-label": "Close details", title: "Close (Esc)", onclick: () => details.close() }, "✕")), h("div", { class: "content" }, content));
    pane.classList.toggle("wide", Boolean(options.wide));
    const app = document.getElementById("app");
    app.classList.add("details-open");
    app.classList.toggle("details-wide", Boolean(options.wide));
    pane.hidden = false;
    pane.querySelector("button.icon-btn").focus();
  },
  close() {
    document.getElementById("details").hidden = true;
    document.getElementById("app").classList.remove("details-open", "details-wide");
  },
  guide() {
    const page = currentPage();
    const entry = NAV.flatMap(([, items]) => items).find(([id]) => id === page);
    details.open("About this page", h("div", { class: "explain" }, h("p", {}, PAGE_GUIDE[page] || ""), entry && entry[2].length ? h("div", {}, h("h4", {}, "Zones"), h("ul", {}, entry[2].map(([id, label]) => h("li", {}, h("a", { href: "#", onclick: (event) => { event.preventDefault(); scrollToZone(id); } }, label))))) : null,
      h("h4", {}, "The current filter"), h("p", { class: "sentence" }, filterSentence({})),
      h("h4", {}, "Reading the dashboard"), h("ul", {}, h("li", {}, "Hover any ", h("span", { class: "info-icon inline" }, "i"), " for what a number, value, or flow means; click it to read more here."), h("li", {}, "Hover a panel's funnel for how it is filtered, in words; click its lens for the chart with its metrics, properties, and an interpretation."), h("li", {}, "Click a bar, slice, dot, cell, or KPI to see the events behind it; drag across a time chart to zoom in."), h("li", {}, "Every panel can be refreshed, opened in the lens, expanded, shown in monitor mode (full screen, live), or collapsed from its header."), h("li", {}, "Times are YYYY:MM:DD:HH:MM:SS:MMM in your local time."), h("li", {}, "Keys: Ctrl K or / to search, ? for this guide, Esc to close."))), { kind: "Guide" });
  },
};

// ---------------------------------------------------------------- API and the global filter

class SignedOut extends Error {}
function scoped(path) {
  const url = new URL(path, location.origin);
  if (!url.pathname.startsWith("/api/me") && !url.pathname.startsWith("/api/accounts/")) {
    url.searchParams.set("repository", scope.repository);
    if (scope.member) url.searchParams.set("members", scope.member);
  }
  return url;
}
async function api(path, params) {
  const url = scoped(path);
  for (const [key, value] of Object.entries(params || {})) if (value !== undefined && value !== null && value !== "") url.searchParams.set(key, value);
  const response = await fetch(url, { credentials: "same-origin" });
  const body = await response.json().catch(() => ({ error: "invalid response" }));
  if (response.status === 401) throw new SignedOut(body.error);
  if (!response.ok) { const error = new Error(body.error || response.statusText); error.status = response.status; throw error; }
  return body;
}
async function apiWrite(path, payload) {
  const response = await fetch(scoped(path), {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const body = await response.json().catch(() => ({ error: "invalid response" }));
  if (response.status === 401) throw new SignedOut(body.error);
  if (!response.ok) { const error = new Error(body.error || response.statusText); error.status = response.status; throw error; }
  return body;
}
function cleanClause({ path, op, values, group, exclude }) { const clause = { path, op, values: values || [] }; if (group) clause.group = group; if (exclude) clause.exclude = true; return clause; }
/** The global filter as request parameters; a panel's own clauses and settings are added */
function filterParams(extra) {
  extra = Object.assign({}, extra || {});
  const params = {};
  if (filter.since || filter.until) { if (filter.since) params.since = Math.floor(filter.since); if (filter.until) params.until = Math.floor(filter.until); }
  else { const range = RANGES.find(([name]) => name === filter.range); if (range && range[1]) params.since = Math.floor(Date.now() - range[1]); }
  if (filter.q) params.q = filter.q;
  // A panel's own OR groups are numbered apart from the global ones
  const offset = Math.max(0, ...filter.clauses.map((clause) => clause.group || 0));
  const local = (extra.clauses || []).map((clause) => cleanClause(Object.assign({}, clause, clause.group ? { group: clause.group + offset + 100 } : {})));
  const clauses = filter.clauses.filter((clause) => !clause.off).map(cleanClause).concat(local);
  delete extra.clauses;
  if (extra.ignoreTime) { delete params.since; delete params.until; }
  delete extra.ignoreTime;
  if (clauses.length) params.filter = JSON.stringify(clauses);
  return Object.assign(params, extra);
}
function activeFilters() { return filter.clauses.filter((clause) => !clause.off).length + (filter.q ? 1 : 0) + (filter.since || filter.until ? 1 : 0); }
/** Set (or with no values, remove) the plain clause of an attribute and operator */
function setClause(path, op, values, exclude) {
  path = facetPath(path);
  filter.clauses = filter.clauses.filter((clause) => !(clause.path === path && clause.op === op && !clause.group && Boolean(clause.exclude) === Boolean(exclude)));
  if (values && values.length) filter.clauses.push(cleanClause({ path, op, values, exclude }));
  saveFilter();
}
function toggleValue(path, value, exclude) {
  path = facetPath(path);
  const existing = filter.clauses.find((clause) => clause.path === path && clause.op === "is" && !clause.group && Boolean(clause.exclude) === Boolean(exclude));
  const values = new Set(existing ? existing.values : []);
  if (values.has(value)) values.delete(value); else values.add(value);
  setClause(path, "is", [...values], exclude);
}
function isChosen(path, value, exclude) { return filter.clauses.some((clause) => clause.path === facetPath(path) && clause.op === "is" && !clause.off && Boolean(clause.exclude) === Boolean(exclude) && clause.values.includes(value)); }
/** Open the events of a choice (a KPI, bar, slice, node, or value) */
function drill(changes) {
  for (const clause of changes.clauses || []) filter.clauses.push(cleanClause(clause));
  if (changes.since || changes.until) { filter.since = changes.since || null; filter.until = changes.until || null; }
  saveFilter();
  go(changes.to || "#/explorer");
}
function zoomTo(since, until) { filter.since = since; filter.until = until; saveFilter(); render(); }
async function facets() {
  if (!facetCache) facetCache = api("/api/facets", filterParams()).catch(() => ({ total: 0, facets: {} }));
  return facetCache;
}
async function attributes() {
  if (!catalog) catalog = api("/api/attributes", filterParams()).catch(() => ({ attributes: [] }));
  return catalog;
}
async function retention() {
  if (!retentionInfo) retentionInfo = api("/api/retention").catch(() => null);
  return retentionInfo;
}
/** Whether a time is in its last days before deletion (stale data) */
function staleAt(millis, info) {
  if (!info || info.mode !== "EXPIRES" || !millis) return null;
  const age = (Date.now() - millis) / 86400000;
  if (age < info.stale_after_days) return null;
  return Math.max(0, info.days - age);
}

// ---------------------------------------------------------------- panels and zones

let zoneObserver = null;
function persistAreaFilters() { storage.save("foxx-area-filters", areaFilters, localStorage); }
function areaConditions(page, zoneId) {
  return (areaFilters.pages[page] || []).concat(zoneId ? areaFilters.zones[page + ":" + zoneId] || [] : []);
}
function scopeFilterRow(list, title, note, apply) {
  const row = h("div", { class: "area-filter" });
  const target = { list, title, note, apply: () => { persistAreaFilters(); draw(); apply(); } };
  const draw = () => put(row, h("button", { class: "btn small", type: "button", onclick: (event) => builder(event.currentTarget, -1, target) }, "+ Add filter"),
    list.map((clause, index) => h("span", { class: "pill" + (clause.exclude ? " not" : "") },
      h("button", { type: "button", class: "pill-text", title: "Edit this scoped filter", onclick: (event) => builder(event.currentTarget, index, target) }, clauseText(clause)),
      h("button", { type: "button", "aria-label": "Remove " + clauseText(clause), onclick: () => { list.splice(index, 1); persistAreaFilters(); draw(); apply(); } }, "×"))));
  draw();
  return row;
}
function annotatePanel(node, target) {
  const area = node.dashboardArea || { page: currentDashboardPage, zone: "" };
  const spec = node.scopeSpec();
  const metadata = {
    section: area.page,
    subsection: area.zone,
    component: node.title,
    information: typeof node.options.info === "string" ? explain(node.options.info).text : node.options.info && node.options.info.text,
    filter: spec,
  };
  for (const component of target.querySelectorAll("svg.chart, .table-scroll table")) {
    component.dataset.section = metadata.section;
    component.dataset.subsection = metadata.subsection;
    component.dataset.component = metadata.component;
    component.dataset.filter = JSON.stringify(spec);
    component.dataset.context = metadata.information || "";
    if (component.matches("table")) component.dataset.columns = JSON.stringify([...component.querySelectorAll("thead th")].map((cell) => cell.textContent.trim()));
  }
}
async function openAgentContext(node) {
  const area = node.dashboardArea || { page: currentDashboardPage, zone: "" };
  const spec = node.scopeSpec();
  const componentNode = node.body.querySelector("svg.chart, .table-scroll table");
  const information = typeof node.options.info === "string" ? explain(node.options.info).text : node.options.info && node.options.info.text;
  const params = filterParams({
    clauses: node.queryClauses(),
    section: area.page,
    subsection: area.zone,
    component: node.title,
    kind: componentNode && componentNode.matches("table") ? "table" : "graph",
    context: information || "",
  });
  const data = await api("/api/agent-context", params);
  const text = JSON.stringify(data);
  const download = h("button", { class: "btn", type: "button", onclick: () => {
    const url = URL.createObjectURL(new Blob([text], { type: "application/json" }));
    const anchor = h("a", { href: url, download: "foxx-agent-context.json" });
    document.body.append(anchor); anchor.click(); anchor.remove(); URL.revokeObjectURL(url);
  } }, "Download compact JSON");
  const url = scoped("/api/agent-context");
  for (const [key, value] of Object.entries(params)) url.searchParams.set(key, value);
  const actions = h("div", { class: "row-gap" },
    h("button", { class: "btn small", type: "button", onclick: () => {
      const button = document.activeElement;
      navigator.clipboard.writeText(url.toString()).then(() => { button.textContent = "Copied"; }).catch(() => { button.textContent = "Copy failed"; });
    } }, "Copy API URL"),
    h("button", { class: "btn small", type: "button", onclick: () => {
      const button = document.activeElement;
      navigator.clipboard.writeText(text).then(() => { button.textContent = "Copied"; }).catch(() => { button.textContent = "Copy failed"; });
    } }, "Copy JSON"),
    download);
  details.open("Agent context · " + node.title, h("div", {},
    h("p", { class: "sentence" }, filterSentence(spec)),
    h("p", { class: "muted small" }, "Authenticated API URL for this component's filtered context:"),
    h("div", { class: "code-line" }, h("code", {}, url.toString())),
    h("pre", { class: "api-preview" }, text)), { wide: true, kind: "foxx-agent-context/v1", actions });
}
/** A zone: one subsection of a page, listed in the rail */
function zone(id, title, guide, ...children) {
  const local = areaFilters.zones[currentDashboardPage + ":" + id] || (areaFilters.zones[currentDashboardPage + ":" + id] = []);
  const section = h("section", { class: "zone", id: "zone-" + id, "aria-labelledby": "zone-title-" + id });
  const grid = h("div", { class: "grid" }, children);
  children.filter((child) => child.classList && child.classList.contains("panel")).forEach((child) => {
    child.dashboardArea = { page: currentDashboardPage, zone: id };
  });
  const rerun = () => section.querySelectorAll(".panel").forEach((node) => node.refresh && node.refresh());
  put(section,
    h("div", { class: "zone-heading" }, h("h2", { class: "zone-title", id: "zone-title-" + id }, title, info("", { title, text: guide || title + " groups related dashboard panels. Use each panel's controls and info icon to explore the data." })),
      scopeFilterRow(local, "Filter subsection " + title, "Applies only to charts and tables in this subsection.", rerun)),
    grid);
  return section;
}
function scrollToZone(id) { const node = document.getElementById("zone-" + id); if (node) node.scrollIntoView({ behavior: "smooth", block: "start" }); }
/** A panel focused on one thing. Its header: the title and explanation, the funnel (its filter in
 * words), its own controls, and refresh, lens, expand, monitor, and collapse. draw(body, big, ctx)
 * fills it and runs again on refresh; ctx.clauses() are the panel's own conditions (options.filterable)
 * and ctx.record(kind, data, properties) keeps what it drew for the lens
 * options: span, info, flush, noExpand, filterable, scope (filterSentence spec or a function giving one), clauses (fixed panel clauses), interpret */
function panel(title, options, draw) {
  options = options || {};
  const body = h("div", { class: "body" + (options.flush ? " flush" : "") });
  const controls = h("div", { class: "controls" });
  const node = h("section", { class: "panel span-" + (options.span || 12) });
  const local = [];
  const filterRow = h("div", { class: "panel-filter" });
  node.local = local;
  node.recorded = null;
  node.title = title;
  node.options = options;
  node.queryClauses = () => areaConditions(node.dashboardArea ? node.dashboardArea.page : currentDashboardPage, node.dashboardArea && node.dashboardArea.zone).concat(fixed(), local);
  const fixed = () => (typeof options.clauses === "function" ? options.clauses() : options.clauses || []);
  const ctx = (target) => ({
    clauses: node.queryClauses,
    record: (kind, data, properties) => { if (target === body) node.recorded = { kind, data, properties: properties || {}, at: Date.now() }; else target.recorded = { kind, data, properties: properties || {}, at: Date.now() }; },
  });
  const run = async (target, big) => {
    if (target.childNodes.length && target === body) target.classList.add("loading"); else put(target, h("div", { class: "skeleton" }, "Loading…"));
    try { await draw(target, big || false, ctx(target)); annotatePanel(node, target); } catch (error) { if (error instanceof SignedOut) { signIn(); return; } put(target, h("div", { class: "notice error" }, error.message)); }
    target.classList.remove("loading");
  };
  node.refresh = () => run(body);
  node.run = run;
  node.scopeSpec = () => {
    const spec = typeof options.scope === "function" ? options.scope() : Object.assign({}, options.scope || {});
    if (!spec.kind || spec.kind === "events") spec.clauses = (spec.clauses || []).concat(areaConditions(node.dashboardArea ? node.dashboardArea.page : currentDashboardPage, node.dashboardArea && node.dashboardArea.zone), fixed(), local);
    return spec;
  };
  const drawFilterRow = () => put(filterRow, local.map((clause, index) => h("span", { class: "pill" + (clause.exclude ? " not" : "") }, h("button", { type: "button", class: "pill-text", title: "Edit this panel condition", onclick: (event) => builder(event.currentTarget, index, panelTarget) }, clauseText(clause)), h("button", { type: "button", "aria-label": "Remove " + clauseText(clause), onclick: () => { local.splice(index, 1); drawFilterRow(); node.refresh(); } }, "×"))));
  const panelTarget = { list: local, title: "Condition for this panel only", note: "Added to the global filter for this panel; other panels are not affected.", apply: () => { drawFilterRow(); node.refresh(); } };
  const tools = h("div", { class: "tools" },
    scopeIcon(() => node.scopeSpec(), () => lens(node, "properties")),
    h("button", { class: "icon-btn", type: "button", title: "Open this component's filtered context for an agent", "aria-label": "Agent context for " + title, onclick: () => openAgentContext(node).catch((error) => { if (error instanceof SignedOut) signIn(); else details.open("Agent context unavailable", h("div", { class: "notice error" }, error.message), { kind: "API error" }); }) }, "AI"),
    options.filterable ? h("button", { class: "icon-btn", type: "button", title: "Add a condition for this panel only", "aria-label": "Filter " + title, onclick: (event) => builder(event.currentTarget, -1, panelTarget) }, "+ filter") : null,
    h("button", { class: "icon-btn", type: "button", title: "Refresh", "aria-label": "Refresh " + title, onclick: () => node.refresh() }, "↻"),
    options.noExpand ? null : h("button", { class: "icon-btn", type: "button", title: "Lens: the chart with its metrics, properties, and interpretation", "aria-label": "Lens on " + title, onclick: () => lens(node) }, SVG_ICON(LENS)),
    options.noExpand ? null : h("button", { class: "icon-btn", type: "button", title: "Expand", "aria-label": "Expand " + title, onclick: () => overlay(title, (target) => run(target, true), false) }, "⤢"),
    options.noExpand ? null : h("button", { class: "icon-btn", type: "button", title: "Monitor view: full screen, refreshed every 30 seconds", "aria-label": "Monitor " + title, onclick: () => overlay(title, (target) => run(target, true), true) }, "▣"),
    h("button", { class: "icon-btn", type: "button", title: "Collapse", "aria-label": "Collapse " + title, "aria-expanded": "true", onclick: (event) => { const collapsed = node.classList.toggle("collapsed"); event.currentTarget.setAttribute("aria-expanded", String(!collapsed)); event.currentTarget.textContent = collapsed ? "▸" : "▾"; } }, "▾"));
  put(node, h("header", {}, h("h3", {}, title, options.info ? (typeof options.info === "string" ? info(options.info) : info("", options.info)) : null), controls, tools), filterRow, body);
  node.controls = controls;
  node.body = body;
  queueMicrotask(() => run(body));
  return node;
}
/** Show a panel large: expanded, or in monitor mode (full screen, refreshed every 30 seconds) */
function overlay(title, draw, monitor) {
  const body = h("div", { class: "overlay-body" });
  const stamp = h("span", { class: "muted" });
  let timer = null;
  const refresh = async () => { await draw(body); stamp.textContent = monitor ? "Live · refreshed " + dt(Date.now()) + " · every 30 s" : ""; };
  const close = () => { clearInterval(timer); if (document.fullscreenElement) document.exitFullscreen().catch(() => {}); shell.remove(); document.removeEventListener("keydown", escape); };
  const escape = (event) => { if (event.key === "Escape") close(); };
  const shell = h("div", { class: "overlay" + (monitor ? " monitor" : ""), role: "dialog", "aria-label": title },
    h("header", {}, h("h3", {}, (monitor ? "Monitor · " : "") + title), stamp, h("button", { class: "btn", type: "button", onclick: refresh }, "↻ Refresh"), h("button", { class: "btn", type: "button", onclick: close }, "✕ Close")), body);
  document.body.append(shell);
  document.addEventListener("keydown", escape);
  if (monitor) { if (shell.requestFullscreen) shell.requestFullscreen().catch(() => {}); timer = setInterval(refresh, 30000); }
  refresh();
}

// ---------------------------------------------------------------- the lens: a chart explained

/** Numbers that describe what a panel drew, and sentences that interpret them */
const READ = {
  timeseries(data, properties) {
    const format = properties.format || compact;
    const series = data.series || [];
    const metrics = [["buckets", data.buckets], ["bucket size", duration(data.bucket_ms)], ["events counted", fmt(data.total)], ["series", series.length]];
    const lines = [];
    if (!data.total) return { metrics, lines: ["Nothing matched in this range and filter; widen the time range or remove a condition."] };
    const isCount = !data.agg || data.agg === "count";
    const sum = (points) => points.reduce((acc, value) => acc + (value || 0), 0);
    const totals = Array.from({ length: data.buckets }, (_, index) => series.reduce((acc, item) => acc + (item.points[index] || 0), 0));
    if (isCount || data.agg === "sum") {
      const peak = totals.indexOf(Math.max(...totals));
      metrics.push(["peak bucket", format(totals[peak])], ["average per bucket", format(sum(totals) / Math.max(1, data.buckets))], ["empty buckets", totals.filter((value) => !value).length]);
      lines.push("The busiest moment was " + dt(data.start + peak * data.bucket_ms) + " with " + format(totals[peak]) + (isCount ? " events" : "") + " in one " + duration(data.bucket_ms) + " bucket.");
      const half = Math.floor(data.buckets / 2), first = sum(totals.slice(0, half)), second = sum(totals.slice(half));
      if (first + second > 0 && data.buckets >= 4) {
        const change = first ? ((second - first) / first) * 100 : 100;
        lines.push(Math.abs(change) < 10 ? "Activity was steady: the second half of the range is within 10% of the first." : "Activity is " + (change > 0 ? "rising" : "falling") + ": the second half of the range has " + Math.abs(change).toFixed(0) + "% " + (change > 0 ? "more" : "less") + " than the first.");
      }
      const empties = totals.filter((value) => !value).length;
      if (empties / data.buckets > 0.5) lines.push("Activity is bursty: " + empties + " of " + data.buckets + " buckets are empty.");
    } else {
      const values = series.flatMap((item) => item.points).filter((value) => value !== null && value !== undefined);
      if (values.length) {
        metrics.push(["lowest", format(Math.min(...values))], ["highest", format(Math.max(...values))], ["mean of buckets", format(sum(values) / values.length)]);
        const all = series.flatMap((item) => item.points.map((value, index) => [value, index, item])).filter(([value]) => value !== null && value !== undefined);
        const [top, index, item] = all.sort((a, b) => b[0] - a[0])[0];
        lines.push("The highest " + data.agg + " of " + (data.metric || "the metric") + " was " + format(top) + " at " + dt(data.start + index * data.bucket_ms) + (series.length > 1 ? " (" + (item.label || item.key) + ")" : "") + ".");
      }
      lines.push("Each point is the " + data.agg + " of " + data.metric + " over the events of one " + duration(data.bucket_ms) + " bucket; a gap means no event carried it there.");
    }
    if (series.length > 1) {
      const ranked = series.slice().sort((a, b) => (b.events || b.total || 0) - (a.events || a.total || 0));
      const all = sum(ranked.map((item) => item.events || item.total || 0));
      lines.push((ranked[0].label || ranked[0].key) + " leads with " + ((100 * (ranked[0].events || ranked[0].total || 0)) / Math.max(1, all)).toFixed(0) + "% of the events" + (ranked[1] ? ", then " + (ranked[1].label || ranked[1].key) : "") + ".");
      for (const key of ["DENY", "CRITICAL", "HIGH", "FAILED", "BYPASS_ATTEMPT"]) { const item = series.find((entry) => entry.key === key); if (item && item.total) lines.push(key + " accounts for " + format(item.total) + " — click its legend entry twice to open those events."); }
    }
    return { metrics, lines };
  },
  items(data, properties) {
    const items = data || [];
    const total = items.reduce((acc, item) => acc + item.count, 0);
    const metrics = [["values", items.length], ["total", fmt(total)]];
    if (!total) return { metrics, lines: ["Nothing to break down in this range and filter."] };
    const top = items[0];
    const three = items.slice(0, 3).reduce((acc, item) => acc + item.count, 0);
    metrics.push(["largest", (top.label || top.value)], ["largest share", percent(top.count / total)], ["top 3 share", percent(three / total)]);
    const lines = [(top.label || top.value) + " is the largest with " + percent(top.count / total) + " of " + (properties.unit || "the events") + "."];
    lines.push(three / total > 0.8 ? "Concentrated: the top three values make up " + percent(three / total) + "." : "Spread out: the top three values make up only " + percent(three / total) + ".");
    for (const key of ["DENY", "QUARANTINE", "CRITICAL", "HIGH", "FAILED", "NOT_SATISFIED"]) { const item = items.find((entry) => entry.value === key); if (item) lines.push(key + " is " + percent(item.count / total) + " (" + fmt(item.count) + ")" + (meaning(key) ? ": " + meaning(key) : ".")); }
    return { metrics, lines };
  },
  matrix(data) {
    const cells = [];
    (data.rows || []).forEach((row, r) => (data.cols || []).forEach((col, c) => cells.push({ row: row.label || row.key, col: col.label || col.key, count: data.cells[r][c], denied: data.denied ? data.denied[r][c] : 0 })));
    const filled = cells.filter((cell) => cell.count);
    const metrics = [["rows", (data.rows || []).length], ["columns", (data.cols || []).length], ["filled cells", filled.length + " of " + cells.length], ["busiest cell", fmt(data.max)]];
    const lines = [];
    if (!filled.length) return { metrics, lines: ["No pair of values occurred in this range and filter."] };
    const top = filled.sort((a, b) => b.count - a.count)[0];
    lines.push("The most common pair is " + top.row + " × " + top.col + " with " + fmt(top.count) + " events.");
    const risky = cells.filter((cell) => cell.denied).sort((a, b) => b.denied / b.count - a.denied / a.count)[0];
    if (risky) lines.push("The highest denial share is " + risky.row + " × " + risky.col + ": " + fmt(risky.denied) + " of " + fmt(risky.count) + " denied (" + percent(risky.denied / risky.count) + ").");
    lines.push(filled.length / cells.length < 0.3 ? "Sparse: most pairs never happen, so each " + (data.rows_by || "row") + " sticks to a few " + (data.cols_by || "columns") + "." : "Dense: most pairs happen, so " + (data.rows_by || "rows") + " and " + (data.cols_by || "columns") + " mix freely.");
    return { metrics, lines };
  },
  histogram(data, properties) {
    const format = properties.format || compact;
    if (!data.count) return { metrics: [["values", 0]], lines: ["No values for this field in range and filter."] };
    const metrics = [["values", fmt(data.count)], ["min", format(data.min)], ["p50", format(data.p50)], ["mean", format(data.mean)], ["p95", format(data.p95)], ["max", format(data.max)]];
    const lines = ["Half of the values are at most " + format(data.p50) + "; 95% are at most " + format(data.p95) + "."];
    if (data.p50 > 0 && data.p95 / data.p50 > 5) lines.push("Long tail: the 95th percentile is " + (data.p95 / data.p50).toFixed(1) + "× the median, so a few values are far above the typical one.");
    else lines.push("Tight: the 95th percentile is within " + (data.p50 ? (data.p95 / data.p50).toFixed(1) + "×" : "range of") + " the median.");
    if (data.mean > data.p50 * 1.5) lines.push("The mean (" + format(data.mean) + ") is well above the median: outliers pull it up.");
    return { metrics, lines };
  },
  gantt(data) {
    const bars = (data.rows || []).flatMap((row) => row.bars);
    const failed = bars.filter((bar) => bar.outcome === "FAILED");
    const longest = bars.slice().sort((a, b) => (b.end - b.start) - (a.end - a.start))[0];
    const metrics = [["rows", (data.rows || []).length], ["bars", bars.length], ["failed", failed.length], ["span", duration((data.end || 0) - (data.start || 0))]];
    const lines = [];
    if (longest) lines.push("The longest action is " + longest.label + " (" + duration(longest.end - longest.start) + ", from " + dt(longest.start) + ").");
    if (bars.length) lines.push(failed.length ? percent(failed.length / bars.length) + " of the actions failed (outlined in red)." : "No action failed.");
    const busiest = (data.rows || []).slice().sort((a, b) => b.bars.length - a.bars.length)[0];
    if (busiest) lines.push("The busiest row is " + busiest.label + " with " + busiest.bars.length + " actions.");
    return { metrics, lines };
  },
  lanes(data) {
    const lanes = (data.lanes || []).slice().sort((a, b) => b.count - a.count);
    const metrics = [["lanes", lanes.length], ["events", fmt(data.total)], ["span", duration((data.end || 0) - (data.start || 0))]];
    const lines = lanes.length ? [(lanes[0].label || lanes[0].key) + " is the busiest lane with " + fmt(lanes[0].count) + " events (" + percent(lanes[0].count / Math.max(1, data.total)) + ")."] : ["No events."];
    return { metrics, lines };
  },
  topology(data) {
    const nodes = data.nodes || [], edges = data.edges || [];
    const metrics = [["layers", (data.layers || []).length], ["nodes", nodes.length], ["edges", edges.length]];
    const lines = [];
    const heavy = edges.slice().sort((a, b) => b.count - a.count)[0];
    if (heavy) lines.push("The heaviest path is " + heavy.from.split(":").slice(1).join(":") + " → " + heavy.to.split(":").slice(1).join(":") + " (" + fmt(heavy.count) + " events).");
    const risky = nodes.filter((node) => node.denied).sort((a, b) => b.denied / b.count - a.denied / a.count)[0];
    if (risky) lines.push(risky.label + " has the highest denial share: " + fmt(risky.denied) + " of " + fmt(risky.count) + " denied.");
    else if (nodes.length) lines.push("Nothing on these paths was denied.");
    return { metrics, lines };
  },
  usage(data) {
    const groups = data.groups || [];
    const totals = data.totals || {};
    const metrics = [["groups", groups.length], ["tool calls", fmt(totals.calls)], ["tokens", tokens(totals.tokens && totals.tokens.total)], ["p95 tool time", duration(totals.tool_ms && totals.tool_ms.p95)], ["failure share", percent(totals.failure_share)]];
    const lines = [];
    if (!groups.length) return { metrics, lines: ["No tool calls in this range and filter."] };
    lines.push(groups[0].key + " is used most: " + fmt(groups[0].calls) + " calls (" + percent(groups[0].calls / Math.max(1, totals.calls)) + ").");
    const byTokens = groups.slice().sort((a, b) => b.tokens.total - a.tokens.total)[0];
    if (byTokens && byTokens.tokens.total) lines.push(byTokens.key + " burnt the most tokens: " + tokens(byTokens.tokens.total) + " (" + percent(byTokens.tokens.total / Math.max(1, totals.tokens.total)) + ").");
    const slow = groups.filter((group) => group.tool_ms.p95 !== null).sort((a, b) => b.tool_ms.p95 - a.tool_ms.p95)[0];
    if (slow) lines.push(slow.key + " is the slowest at the 95th percentile: " + duration(slow.tool_ms.p95) + ".");
    const fail = groups.filter((group) => group.calls >= 3).sort((a, b) => b.failure_share - a.failure_share)[0];
    if (fail && fail.failure_share) lines.push(fail.key + " fails most often: " + percent(fail.failure_share) + " of its calls.");
    if (totals.tokens && totals.tokens.measured_calls < totals.calls) lines.push(fmt(totals.calls - totals.tokens.measured_calls) + " calls report no tokens (agents other than Claude Code, or no transcript).");
    return { metrics, lines };
  },
  calls(data) {
    const list = data || [];
    const measured = list.filter((call) => call.tokens);
    const tokenSum = measured.reduce((acc, call) => acc + call.tokens.total, 0);
    const timed = list.filter((call) => call.tool_ms !== null && call.tool_ms !== undefined);
    const metrics = [["calls", list.length], ["tokens", tokens(tokenSum)], ["with tokens", measured.length], ["tool time", duration(timed.reduce((acc, call) => acc + call.tool_ms, 0))]];
    const lines = [];
    const slow = timed.slice().sort((a, b) => b.tool_ms - a.tool_ms)[0];
    if (slow) lines.push("The slowest call is " + slow.tool + " at " + dt(slow.start) + ": " + duration(slow.tool_ms) + ".");
    const costly = measured.slice().sort((a, b) => b.tokens.total - a.tokens.total)[0];
    if (costly) lines.push("The most expensive call is " + costly.tool + " at " + dt(costly.start) + ": " + tokens(costly.tokens.total) + " (" + percent(costly.tokens.total / Math.max(1, tokenSum)) + " of these calls).");
    const denied = list.filter((call) => ["DENY", "QUARANTINE"].includes(call.decision)).length;
    if (list.length) lines.push(denied ? fmt(denied) + " of these calls were denied." : "None of these calls was denied.");
    if (list.length > 1) { const span = list[list.length - 1].start - list[0].start; lines.push("They span " + duration(span) + "; on average a call started every " + duration(span / (list.length - 1)) + "."); }
    return { metrics, lines };
  },
  rows(data, properties) {
    return { metrics: [["rows", (data || []).length]], lines: [properties.summary || "A table: search, sort, filter its columns, and export it as CSV."] };
  },
  value(data, properties) { return { metrics: Object.entries(data || {}).map(([key, value]) => [key, typeof value === "number" ? (properties.format || compact)(value) : fmt(value)]), lines: properties.lines || [] }; },
};
/** Open a panel in the lens: the chart large, its metrics, its interpretation, its properties, and
 * what it is, in the right sidebar */
function lens(node, tab) {
  const chartBox = h("div", { class: "lens-chart" });
  const recorded = () => chartBox.recorded || node.recorded;
  const tabs = ["Interpretation", "Metrics", "Properties", "About"];
  let current = tab === "properties" ? "Properties" : "Interpretation";
  const pane = h("div");
  const drawTab = () => {
    const record = recorded();
    const reader = record && READ[record.kind];
    const read = reader ? reader(record.data, record.properties || {}) : { metrics: [], lines: ["This panel lists records; use its search, column filters, and sort."] };
    const spec = node.scopeSpec();
    const views = {
      Interpretation: () => h("div", {}, read.lines.length ? h("ul", { class: "insights" }, read.lines.map((line) => h("li", {}, line))) : h("p", { class: "muted" }, "Nothing to interpret yet."), h("p", { class: "muted small" }, "Generated from the data drawn above; it describes, never claims a cause.")),
      Metrics: () => read.metrics.length ? h("div", { class: "metric-grid" }, read.metrics.map(([label, value]) => h("div", {}, h("span", {}, label), h("b", {}, value === null || value === undefined ? "—" : String(value))))) : h("p", { class: "muted" }, "No metrics for this panel."),
      Properties: () => h("div", {}, h("p", { class: "sentence" }, filterSentence(spec)), h("div", { class: "kv", style: "margin-top:10px" }, [["chart", record ? record.kind : "table"], ["drawn at", record ? dt(record.at) : "—"], ["time range", spec.kind && spec.kind !== "events" ? "not applied" : rangeText()], ["global conditions", filter.clauses.filter((clause) => !clause.off).length], ["panel conditions", (spec.clauses || []).length], ["member", scope.member ? memberLabel(scope.member) : "all selected members"]].concat(Object.entries((record && record.properties) || {}).filter(([key, value]) => key !== "format" && value !== undefined && value !== null && typeof value !== "function").map(([key, value]) => [key, typeof value === "object" ? JSON.stringify(value) : String(value)])).map(([key, value]) => [h("div", {}, key), h("div", {}, fmt(value))]))),
      About: () => { const about = typeof node.options.info === "string" ? explain(node.options.info) : node.options.info; return h("div", { class: "explain" }, about && about.text ? h("p", {}, about.text) : h("p", { class: "muted" }, "No further explanation."), h("h4", {}, "How to read it"), h("ul", {}, h("li", {}, "Hover any mark for its exact values; click it to open the events behind it."), h("li", {}, "Drag across a time axis to zoom; the time range then applies to every panel."), h("li", {}, "The funnel says in words which events are counted."))); },
    };
    put(pane, h("div", { class: "lens-tabs", role: "tablist" }, tabs.map((name) => h("button", { type: "button", role: "tab", "aria-selected": String(name === current), class: name === current ? "on" : "", onclick: () => { current = name; drawTab(); } }, name))), h("div", { style: "padding-top:10px" }, views[current]()));
  };
  details.open(node.title, h("div", {}, chartBox, pane), { wide: true, kind: "Lens", actions: h("button", { class: "btn small", type: "button", onclick: () => overlay(node.title, (target) => node.run(target, true), false) }, "⤢ Full screen") });
  node.run(chartBox, true).then(drawTab);
  drawTab();
}

// ---------------------------------------------------------------- controls, KPIs, and standard panels

function select(label, value, choices, onchange) {
  return h("select", { "aria-label": label, title: label, onchange: (event) => onchange(event.target.value) },
    choices.map((choice) => {
      if (choice.group) return h("optgroup", { label: choice.group }, choice.options.map(([optionValue, text]) => { const node = h("option", { value: optionValue }, text); if (optionValue === value) node.selected = true; return node; }));
      const node = h("option", { value: choice[0] }, choice[1]);
      if (choice[0] === value) node.selected = true;
      return node;
    }));
}
function segmented(label, value, choices, onchange) {
  const group = h("div", { class: "seg", role: "group", "aria-label": label });
  const draw = (current) => put(group, choices.map(([key, text]) => h("button", { type: "button", class: key === current ? "on" : "", "aria-pressed": String(key === current), onclick: () => { draw(key); onchange(key); } }, text)));
  draw(value);
  return group;
}
const GROUP_CHOICES = [["", "No grouping"]].concat(FACETS.map(([name]) => [name, "By " + FACET_LABELS[name].toLowerCase()]), [["agent.agent_id", "By agent identity"], ["agent.model", "By model"], ["session.session_id", "By session"], ["resource.resource", "By resource"]]);
const AXES = [["agent", "Agent type"], ["agent.agent_id", "Agent identity"], ["agent.model", "Model"], ["tool", "Tool"], ["decision", "Decision"], ["severity", "Severity"], ["flow", "Flow"], ["event", "Event"], ["member", "Member"], ["session.session_id", "Session"], ["resource.resource", "Resource kind"], ["resource.resource_ids", "Resource"], ["enforcement", "Enforcement"], ["response", "Response"], ["origin", "Origin"], ["decision.decision_rule", "Decision rule"]];
function kpi(label, value, options) {
  options = options || {};
  return h("div", { class: "kpi " + (options.tone || ""), role: options.drill ? "button" : null, tabindex: options.drill ? "0" : null, title: options.drill ? "See these events" : undefined, onclick: options.drill || undefined, onkeydown: options.drill ? (event) => { if (event.key === "Enter") options.drill(); } : undefined },
    h("div", { class: "label" }, label, options.info ? info(options.info) : null), h("div", { class: "value" }, options.format ? options.format(value) : compact(value)), options.note ? h("div", { class: "note" }, options.note) : null, charts.sparkline(options.spark, options.color));
}
/** Events (or a metric) over time with its own group-by, metric, and chart type */
function seriesPanel(title, options) {
  const state = { by: options.by || "", type: options.type || "bars", agg: options.agg || "", metric: options.metric || "" };
  const node = panel(title, { span: options.span, info: options.info, filterable: true, clauses: options.clauses, scope: () => ({ what: state.metric ? state.agg + " of " + state.metric + " over events" : "Events", ignoreTime: options.params && options.params.since === "", notes: [state.by ? "Grouped by " + (GROUP_CHOICES.find(([key]) => key === state.by) || [0, state.by])[1].replace("By ", "") + " (the 8 largest groups, the rest as OTHER)." : ""] }) }, async (body, big, ctx) => {
    const data = await api("/api/series", filterParams(Object.assign({ by: state.by, clauses: ctx.clauses(), metric: state.metric, agg: state.metric ? state.agg : "" }, options.params || {})));
    const format = options.format || (/_ms$/.test(state.metric) ? duration : /tokens/.test(state.metric) ? compact : compact);
    ctx.record("timeseries", data, { endpoint: "/api/series", by: state.by || "none", metric: state.metric || "event count", aggregation: data.agg, format });
    put(body, charts.timeseries(data, { type: state.type, height: big ? 460 : options.height, title, format, onZoom: zoomTo, empty: options.empty,
      onPick: ({ since, until, key }) => { const clauses = ctx.clauses().slice(); if (state.by && key && key !== "OTHER" && key !== "NONE") clauses.push(is(facetPath(state.by), key)); drill({ clauses, since, until }); } }));
  });
  node.controls.append(select("Group by", state.by, GROUP_CHOICES, (value) => { state.by = value; node.refresh(); }));
  if (options.metrics) node.controls.append(select("Aggregation", state.agg, options.metrics, (value) => { state.agg = value; node.refresh(); }));
  node.controls.append(select("Chart type", state.type, [["bars", "Bars"], ["lines", "Lines"], ["area", "Area"]], (value) => { state.type = value; node.refresh(); }));
  return node;
}
/** A breakdown of the events by one facet or attribute, as a donut or a top list */
function breakdownPanel(title, options) {
  const state = { by: options.by || "severity", view: options.view || "donut" };
  const node = panel(title, { span: options.span, info: options.info, filterable: true, clauses: options.clauses, scope: () => ({ notes: ["Counted by " + attrLabel(facetPath(state.by)) + "; the " + (options.limit || 10) + " largest values are shown."] }) }, async (body, big, ctx) => {
    const isFacet = FACETS.some(([name]) => name === state.by);
    let items;
    if (isFacet) {
      const data = await api("/api/facets", filterParams({ clauses: ctx.clauses() }));
      items = (data.facets[state.by] || []).slice(0, big ? 25 : options.limit || 10);
    } else {
      const data = await api("/api/matrix", filterParams({ clauses: ctx.clauses(), rows: state.by, cols: "origin", limit: big ? 25 : options.limit || 10, hide_none: "1" }));
      items = data.rows.map((row) => ({ value: row.key, label: row.label, count: row.total })).filter((item) => item.value !== "OTHER");
    }
    ctx.record("items", items, { by: state.by, view: state.view });
    const pick = (value) => drill({ clauses: ctx.clauses().concat([is(facetPath(state.by), value)]) });
    put(body, state.view === "donut" ? charts.donut(items, { title, onPick: pick, explain: explained, unit: options.unit }) : charts.toplist(items, { onPick: pick, explain: explained, byValue: true }));
  });
  node.controls.append(select("Break down by", state.by, AXES.concat(FACETS.filter(([name]) => !AXES.some(([key]) => key === name)).map(([name]) => [name, FACET_LABELS[name]])).map(([key, label]) => [key, "By " + label.toLowerCase()]), (value) => { state.by = value; node.refresh(); }),
    select("Show as", state.view, [["donut", "Donut"], ["toplist", "Top list"]], (value) => { state.view = value; node.refresh(); }));
  return node;
}
/** A heatmap of two attributes, with axis choosers */
function matrixPanel(title, options) {
  const state = { rows: options.rows || "agent", cols: options.cols || "tool" };
  const label = (key) => (AXES.find(([value]) => value === key) || [key, key])[1].toLowerCase();
  const node = panel(title, { span: options.span, info: options.info || { title, text: "Each cell counts the events that carry both values; darker is more. A red dot marks cells with denials. Click a cell to open its events." }, filterable: true, clauses: options.clauses, scope: () => ({ notes: ["Rows are " + label(state.rows) + ", columns " + label(state.cols) + "; the 12 largest of each (the rest as OTHER); events without a value are left out."] }) }, async (body, big, ctx) => {
    const data = await api("/api/matrix", filterParams({ clauses: ctx.clauses(), rows: state.rows, cols: state.cols, limit: big ? 30 : options.limit || 12, hide_none: "1" }));
    ctx.record("matrix", data, { rows: state.rows, columns: state.cols, endpoint: "/api/matrix" });
    put(body, charts.heatmap(data, { title, unit: "events", onPick: (row, col) => drill({ clauses: ctx.clauses().concat([row, col].map((value, index) => (value === "OTHER" ? null : is(facetPath(index ? state.cols : state.rows), value))).filter(Boolean)) }) }));
  });
  node.controls.append(select("Rows", state.rows, AXES.map(([key, text]) => [key, "Rows: " + text.toLowerCase()]), (value) => { state.rows = value; node.refresh(); }), select("Columns", state.cols, AXES.map(([key, text]) => [key, "Columns: " + text.toLowerCase()]), (value) => { state.cols = value; node.refresh(); }));
  return node;
}

// ---------------------------------------------------------------- tables

/** A table: search, a dropdown per filterable column, sorting, pages, column choice, CSV export */
function dataTable(columns, rows, options) {
  options = options || {};
  const state = { q: options.query || "", sort: options.sort === undefined ? -1 : options.sort, dir: options.dir || -1, page: 0, chosen: {}, hidden: new Set(columns.map((column, index) => (column.hidden ? index : -1)).filter((index) => index >= 0)) };
  const size = options.pageSize || 25;
  const holder = h("div");
  const text = (column, row) => { const value = column.get ? column.get(row) : null; return value === null || value === undefined ? "" : typeof value === "object" ? JSON.stringify(value) : String(value); };
  const tools = h("div", { class: "table-tools" });
  tools.append(h("input", { type: "search", placeholder: options.placeholder || "Search this table…", value: state.q, "aria-label": "Search this table", oninput: (event) => { state.q = event.target.value; state.page = 0; draw(); } }));
  for (const column of columns.filter((item) => item.filter)) {
    const values = [...new Set(rows.map((row) => text(column, row)).filter(Boolean))].sort();
    tools.append(select(column.label, "", [["", column.label + ": all"]].concat(values.map((value) => [value, value])), (value) => { state.chosen[column.label] = value; state.page = 0; draw(); }));
  }
  const counter = h("span", { class: "count" });
  const visible = () => rows.filter((row) => {
    const q = state.q.trim().toLowerCase();
    return (!q || columns.some((column) => text(column, row).toLowerCase().includes(q))) && columns.every((column) => !state.chosen[column.label] || text(column, row) === state.chosen[column.label]);
  });
  const exportCsv = () => {
    const shown = columns.filter((column, index) => !state.hidden.has(index) && column.get);
    const quote = (value) => '"' + String(value).replace(/"/g, '""') + '"';
    const lines = [shown.map((column) => quote(column.label || "")).join(",")].concat(visible().map((row) => shown.map((column) => quote(column.csv ? column.csv(row) : text(column, row))).join(",")));
    const anchor = h("a", { href: URL.createObjectURL(new Blob([lines.join("\n")], { type: "text/csv" })), download: (options.name || "foxx-export") + ".csv" });
    document.body.append(anchor); anchor.click(); anchor.remove();
  };
  if (!options.noColumns) tools.append(h("button", { class: "btn", type: "button", onclick: (event) => {
    menu(event.currentTarget, h("div", { class: "checks" }, h("strong", {}, "Columns"), columns.map((column, index) => (column.label ? h("label", {}, h("input", { type: "checkbox", checked: state.hidden.has(index) ? null : true, onchange: () => { if (state.hidden.has(index)) state.hidden.delete(index); else state.hidden.add(index); draw(); } }), column.label) : null))));
  } }, "Columns"));
  if (!options.noExport) tools.append(h("button", { class: "btn", type: "button", onclick: exportCsv }, "Export CSV"));
  tools.append(counter);
  // Columns drawn as bars are scaled to their largest value
  const maxima = new Map(columns.filter((column) => column.bar).map((column) => [column, Math.max(1e-9, ...rows.map((row) => Number(column.get(row)) || 0))]));
  const draw = () => {
    let list = visible();
    if (state.sort >= 0) {
      const column = columns[state.sort];
      list = list.slice().sort((a, b) => {
        const x = column.get ? column.get(a) : null, y = column.get ? column.get(b) : null;
        if (typeof x === "number" || typeof y === "number") return ((x ?? -Infinity) - (y ?? -Infinity)) * state.dir;
        return String(x ?? "").localeCompare(String(y ?? "")) * state.dir;
      });
    }
    counter.textContent = list.length === rows.length ? rows.length + " rows" : list.length + " of " + rows.length + " rows";
    const pages = Math.max(1, Math.ceil(list.length / size));
    state.page = Math.min(state.page, pages - 1);
    const shown = list.slice(state.page * size, state.page * size + size);
    const cols = columns.map((column, index) => [column, index]).filter(([, index]) => !state.hidden.has(index));
    const head = h("tr", {}, cols.map(([column, index]) => h("th", { class: column.get ? "sortable" : "", scope: "col", "aria-sort": state.sort === index ? (state.dir > 0 ? "ascending" : "descending") : null, onclick: (event) => { if (!column.get || event.target.closest(".info-icon")) return; if (state.sort === index) state.dir = -state.dir; else { state.sort = index; state.dir = column.num ? -1 : 1; } draw(); } }, column.label, column.info ? info(column.info) : null, state.sort === index ? h("span", { class: "arrow" }, state.dir > 0 ? "▲" : "▼") : null)));
    const body = shown.length ? shown.map((row) => h("tr", { class: options.onRow ? "clickable" : "", tabindex: options.onRow ? "0" : null, onclick: options.onRow ? (event) => { if (!event.target.closest("a,button,input,select")) options.onRow(row); } : undefined, onkeydown: options.onRow ? (event) => { if (event.key === "Enter") options.onRow(row); } : undefined },
      cols.map(([column]) => {
        if (column.bar) { const value = Number(column.get(row)) || 0; return h("td", { class: "num" }, h("div", { class: "bar-cell" }, h("i", { style: "width:" + Math.round((value / maxima.get(column)) * 60) + "px" }), column.render ? column.render(row) : (column.format || compact)(value))); }
        const cell = column.render ? column.render(row) : text(column, row);
        return h("td", { class: column.num ? "num" : "" }, cell instanceof Node ? cell : fmt(cell));
      })))
      : [h("tr", {}, h("td", { colspan: cols.length, class: "empty" }, rows.length ? "No rows match." : (options.empty || "Nothing recorded yet.")))];
    const pager = pages > 1 ? h("div", { class: "pager" }, "Page " + (state.page + 1) + " of " + pages,
      h("button", { class: "btn", type: "button", disabled: state.page === 0 ? true : null, onclick: () => { state.page -= 1; draw(); } }, "‹ Previous"),
      h("button", { class: "btn", type: "button", disabled: state.page >= pages - 1 ? true : null, onclick: () => { state.page += 1; draw(); } }, "Next ›")) : null;
    put(holder, h("div", { class: "table-scroll" }, h("table", {}, h("thead", {}, head), h("tbody", {}, body))), pager);
  };
  draw();
  return h("div", {}, options.noTools ? null : tools, holder);
}
/** A small popover menu next to an element */
function menu(anchor, content) {
  document.querySelectorAll(".menu").forEach((node) => node.remove());
  const node = h("div", { class: "menu", role: "dialog" }, content);
  document.body.append(node);
  const box = anchor.getBoundingClientRect();
  node.style.left = Math.max(8, Math.min(box.left, window.innerWidth - node.offsetWidth - 8)) + "px";
  node.style.top = Math.max(8, Math.min(box.bottom + 6, window.innerHeight - node.offsetHeight - 8)) + "px";
  setTimeout(() => document.addEventListener("mousedown", function close(event) { if (!node.contains(event.target)) { node.remove(); document.removeEventListener("mousedown", close); } }), 0);
  return node;
}
const sevBar = (row) => h("span", { class: "sev", style: "background:" + color((row.severity || {}).severity_status, 0), title: (row.severity || {}).severity_status });
function eventsTable(events, options) {
  return dataTable([
    { label: "", render: sevBar },
    { label: "Time", get: (row) => row.occurred_at, render: (row) => h("span", { class: "mono" }, dt(row.occurred_at)), csv: (row) => dt(row.occurred_at) },
    { label: "Event", get: (row) => row.event_name, render: (row) => h("div", {}, h("code", {}, row.event_name), h("div", { class: "muted" }, (row.response || {}).response_statement || "")) },
    { label: "Flow", get: (row) => row.flow, render: (row) => h("span", {}, row.flow, info("flow:" + row.flow)), filter: true, info: "facet:flow" },
    { label: "Severity", get: (row) => (row.severity || {}).severity_status, render: (row) => chip((row.severity || {}).severity_status), filter: true, info: "facet:severity" },
    { label: "Decision", get: (row) => (row.decision || {}).decision, render: (row) => chip((row.decision || {}).decision), filter: true, info: "facet:decision" },
    { label: "Tool", get: (row) => (row.action || {}).action_tool, filter: true, info: "facet:tool" },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) },
    { label: "Session", get: (row) => sessionOf(row), render: (row) => sessionLink(sessionOf(row)) },
    { label: "Origin", get: (row) => row.origin, render: (row) => chip(row.origin), filter: true, info: "facet:origin", hidden: true },
  ], events, Object.assign({ sort: 1, dir: -1, onRow: openEvent, placeholder: "Search these events…", name: "foxx-events" }, options || {}));
}

// ---------------------------------------------------------------- event and trace details

async function openEventById(id) {
  const data = await api("/api/events", { filter: JSON.stringify([{ path: "event_id", op: "is", values: [id] }]), limit: 1 });
  if (data.events[0]) openEvent(data.events[0]);
}
/** A value of an event, with the filter actions: only this, exclude this */
function valueActions(path, item, shown) {
  return h("span", {}, h("a", { href: "#", title: "Filter every view by this value", onclick: (event) => { event.preventDefault(); details.close(); setClause(path, "is", [item]); go("#/explorer"); } }, shown),
    h("button", { type: "button", class: "icon-btn", title: "Exclude this value from every view", "aria-label": "Exclude " + item, onclick: () => { details.close(); toggleValue(path, item, true); render(); } }, "⊖"), explained(item));
}
async function openEvent(spec) {
  const info2 = await retention();
  const value = (path, item) => {
    const shown = fmt(item);
    if (path === "session.session_id" && item) return h("a", { href: "#/sessions/" + encodeURIComponent(item), onclick: () => details.close() }, shown);
    if ((typeof item === "string" && item || typeof item === "number" && !/_at$|sequence/.test(path)) && !/digest|signature|event_id/.test(path)) return valueActions(path, String(item), shown);
    return shown;
  };
  const groups = ["action", "repository", "session", "agent", "resource", "response", "assessment", "decision", "eval", "severity", "bindings", "telemetry", "other"].filter((group) => spec[group] && Object.keys(spec[group]).length).map((group) => h("div", { class: "group" }, h("h4", {}, group, info("group:" + group)), h("div", { class: "kv" }, Object.entries(spec[group]).map(([key, item]) => [h("div", {}, key, info("attr:" + group + "." + key)), h("div", {}, value(group + "." + key, item))]))));
  const stale = staleAt(spec.occurred_at, info2);
  const envelope = h("div", { class: "group" }, h("h4", {}, "envelope"), h("div", { class: "kv" }, ["event_id", "flow", "origin", "occurred_at", "schema_version", "sequence", "digest", "previous_digest"].filter((key) => spec[key] !== undefined && spec[key] !== null).map((key) => [h("div", {}, key), h("div", {}, key === "occurred_at" ? dt(spec[key]) : value(key, spec[key]))]).concat(spec.member ? [[h("div", {}, "member"), h("div", {}, member(spec.member))]] : [], info2 && info2.mode === "EXPIRES" && spec.origin !== "DERIVED" ? [[h("div", {}, "retention"), h("div", {}, stale !== null ? h("span", {}, chip("STALE_DATA"), " deleted in about " + stale.toFixed(1) + " days") : "kept until " + dt(spec.occurred_at + info2.days * 86400000))]] : [])));
  const copy = h("button", { class: "btn", type: "button", onclick: async () => { try { await navigator.clipboard.writeText(JSON.stringify(spec, null, 2)); copy.textContent = "Copied"; } catch (error) { copy.textContent = "Copy failed"; } } }, "Copy JSON");
  const action = (spec.action || {}).action_id;
  details.open(spec.event_name, h("div", {}, h("p", { class: "statement" }, (spec.response || {}).response_statement || ""), h("div", { class: "row-actions" }, copy, action ? h("button", { class: "btn", type: "button", onclick: () => openTrace(action) }, "Open trace") : null, sessionOf(spec) ? h("button", { class: "btn", type: "button", onclick: () => { details.close(); go("#/sessions/" + encodeURIComponent(sessionOf(spec))); } }, "Open session") : null),
    h("p", { class: "muted" }, "Click a value to filter every view by it, ⊖ to exclude it; hover ", h("span", { class: "info-icon inline" }, "i"), " for what it means."), envelope, groups), { kind: "Event · " + spec.flow });
}
async function openTrace(id) {
  const data = await api("/api/traces/" + encodeURIComponent(id));
  const trace = data.trace, spans = data.spans;
  const start = trace.start, total = Math.max(1, trace.end - trace.start);
  const waterfall = h("div", { class: "waterfall" }, spans.map((spec) => {
    const offset = ((spec.occurred_at - start) / total) * 100;
    return h("div", { class: "wf-row", role: "button", tabindex: "0", onclick: () => openEvent(spec), onkeydown: (event) => { if (event.key === "Enter") openEvent(spec); } },
      h("div", { class: "wf-label" }, sevBar(spec), h("code", {}, spec.event_name), info("flow:" + spec.flow)),
      h("div", { class: "wf-track" }, h("div", { class: "wf-mark", style: "left:" + Math.min(99, offset) + "%;background:" + color((spec.severity || {}).severity_status, 0) })),
      h("div", { class: "wf-time mono" }, "+" + duration(spec.occurred_at - start)));
  }));
  const t = trace.tokens;
  details.open((trace.tool_call_id ? compactIdentity(trace.tool_call_id, "tool") : compactIdentity(id, "tool")), h("div", {},
    h("div", { class: "metric-grid" }, [["duration", duration(trace.duration_ms)], ["tool time", duration(trace.tool_ms)], ["hook latency", duration(trace.latency_ms)], ["tokens", t ? tokens(t.total) : "not reported"], ["events", trace.spans]].map(([label, valueText]) => h("div", {}, h("span", {}, label), h("b", {}, valueText)))),
    h("h4", {}, "Call"), h("div", { class: "kv" }, [["kind", chip(trace.kind)], ["tool", trace.tool || "—"], ["outcome", chip(trace.outcome)], ["severity", chip(trace.severity)], ["decision", chip(trace.decision)], ["start", dt(trace.start)], ["end", dt(trace.end)], ["agent", trace.agent ? agentName(trace.agent_id, trace.agent) : "—"], ["agent identity", trace.agent_id ? agentName(trace.agent_id, trace.agent) : "—"], ["model", trace.model || "—"], ["session", trace.session ? sessionLink(trace.session) : "—"], ["member", member(trace.member)], ["resources", (trace.resources || []).join(", ") || "—"], ["targets", (trace.targets || []).join(", ") || "—"], ["flows", h("span", {}, trace.flows.map((flow) => h("span", {}, flow, info("flow:" + flow), " ")))]].map(([key, item]) => [h("div", {}, key), h("div", {}, item instanceof Node ? item : fmt(item))])),
    t ? h("div", {}, h("h4", {}, "Tokens of the issuing model turn", info("eval:model_tokens")), h("div", { class: "kv" }, [["this call's share", tokens(t.total)], ["input", tokens(t.input)], ["output", tokens(t.output)], ["cache read", tokens(t.cache_read)], ["cache creation", tokens(t.cache_creation)], ["whole turn", tokens(t.turn)], ["calls in the turn", fmt(t.turn_calls)]].map(([key, item]) => [h("div", {}, key), h("div", {}, item)]))) : null,
    h("h4", {}, "Waterfall"), h("p", { class: "muted" }, "Every event of the trace at its time after the first; click one for its attributes."), waterfall), { wide: true, kind: "Trace" });
}

// ---------------------------------------------------------------- the condition builder

/** The condition builder: attribute (any path, searchable), operator (by the attribute's kind),
 * values (with counts, or typed), NOT, and how it combines (AND, or OR with another condition).
 * target: the global filter (default) or a panel's own list */
async function builder(anchor, index, target) {
  const list = target ? target.list : filter.clauses;
  const editing = index >= 0 ? list[index] : null;
  const data = await attributes();
  const all = (data.attributes || []).slice().sort((a, b) => (facetName(b.path) ? 1 : 0) - (facetName(a.path) ? 1 : 0) || a.path.localeCompare(b.path));
  const state = { path: editing ? editing.path : "severity.severity_status", op: editing ? editing.op : "is", values: new Set(editing ? editing.values : []), combine: editing && editing.group ? "g" + editing.group : "and", exclude: Boolean(editing && editing.exclude) };
  const body = h("div", { class: "builder" });
  const draw = () => {
    const attribute = all.find((item) => item.path === state.path) || { path: state.path, kind: "text", values: [] };
    const ops = OPS_BY_KIND[attribute.kind] || OPS_BY_KIND.text;
    if (!ops.includes(state.op)) state.op = ops[0];
    const listBox = h("div", { class: "attr-list", role: "listbox", "aria-label": "Attributes" }, all.map((item) => h("button", { type: "button", role: "option", "aria-selected": String(item.path === state.path), class: item.path === state.path ? "on" : "", "data-path": item.path, onclick: () => { state.path = item.path; state.values = new Set(); draw(); } }, h("span", {}, facetName(item.path) ? FACET_LABELS[facetName(item.path)] + " · " : "", item.path), h("em", {}, item.kind + " · " + compact(item.present)))));
    const search = h("input", { type: "search", placeholder: "Find an attribute…", "aria-label": "Find an attribute", oninput: (event) => { const q = event.target.value.toLowerCase(); listBox.querySelectorAll("button").forEach((node) => { node.hidden = !node.dataset.path.toLowerCase().includes(q); }); } });
    const numeric = ["gt", "gte", "lt", "lte"].includes(state.op);
    const range = state.op === "between";
    const listed = [...state.values].filter((value) => !(attribute.values || []).some((item) => item.value === value));
    const typed = h("input", { type: "text", placeholder: range ? "Low, high" + (attribute.min !== undefined && attribute.min !== null ? " (" + attribute.min + " to " + attribute.max + ")" : "") : numeric ? "A number" + (attribute.min !== undefined && attribute.min !== null ? " (" + attribute.min + " to " + attribute.max + ")" : "") : state.op === "like" ? "Patterns with * and ?, comma separated" : "Or type values, comma separated", "aria-label": "Values", value: listed.join(", ") });
    const valueList = NO_VALUE_OPS.includes(state.op) ? h("p", { class: "muted" }, "No value needed.") : h("div", {},
      numeric || range || state.op === "like" ? null : h("div", { class: "value-list" }, (attribute.values || []).length ? attribute.values.map((item) => h("label", {}, h("input", { type: "checkbox", checked: state.values.has(item.value) ? true : null, onchange: () => { if (state.values.has(item.value)) state.values.delete(item.value); else state.values.add(item.value); } }), h("span", {}, item.label || item.value), explained(item.value), h("em", {}, compact(item.count)))) : h("p", { class: "muted" }, "No listed values; type them below.")),
      typed);
    const groupsUsed = [...new Set(list.map((clause, position) => (clause.group ? "g" + clause.group : position !== index ? "c" + position : null)).filter(Boolean))];
    const combine = select("Combine", state.combine, [["and", "AND: a new condition"]].concat(groupsUsed.map((key) => { const sample = key.startsWith("g") ? list.find((clause) => "g" + clause.group === key) : list[Number(key.slice(1))]; return [key, "OR with: " + clauseText(sample)]; })), (value) => { state.combine = value; });
    const preview = h("span", { class: "preview" });
    const update = () => { preview.textContent = "→ " + clauseText({ path: state.path, op: state.op, values: [...state.values].concat(typed.value.split(",").map((item) => item.trim()).filter(Boolean)), exclude: state.exclude }); };
    typed.addEventListener("input", update);
    body.addEventListener("change", update);
    const submit = () => {
      const values = new Set(numeric || range || state.op === "like" ? [] : state.values);
      for (const item of typed.value.split(",").map((part) => part.trim()).filter(Boolean)) values.add(item);
      if (!NO_VALUE_OPS.includes(state.op) && !values.size) { typed.focus(); typed.classList.add("invalid"); return; }
      if (range && (values.size !== 2 || [...values].some((item) => Number.isNaN(Number(item))))) { typed.focus(); typed.classList.add("invalid"); return; }
      if (numeric && Number.isNaN(Number([...values][0]))) { typed.focus(); typed.classList.add("invalid"); return; }
      const clause = cleanClause({ path: state.path, op: state.op, values: NO_VALUE_OPS.includes(state.op) ? [] : [...values], exclude: state.exclude });
      if (state.combine !== "and") {
        let group;
        if (state.combine.startsWith("g")) group = Number(state.combine.slice(1));
        else { group = Math.max(0, ...list.map((item) => item.group || 0)) + 1; list[Number(state.combine.slice(1))].group = group; }
        clause.group = group;
      }
      if (editing) list[index] = clause; else list.push(clause);
      document.querySelectorAll(".menu").forEach((node) => node.remove());
      if (target) target.apply(); else { saveFilter(); render(); }
    };
    put(body, target ? h("p", { class: "scope-note" }, h("b", {}, target.title), " · ", target.note) : null,
      h("div", { class: "builder-cols" }, h("div", {}, h("strong", {}, "1. Attribute ", info("attr:" + state.path)), search, listBox), h("div", {}, h("strong", {}, "2. Operator"), select("Operator", state.op, ops.map((op) => [op, OPS[op]]), (value) => { state.op = value; state.values = new Set(); draw(); }),
        h("label", { class: "row-gap", style: "font-size:12px" }, h("input", { type: "checkbox", checked: state.exclude ? true : null, onchange: (event) => { state.exclude = event.target.checked; update(); } }), "NOT: keep the events where this does not hold", info("", { title: "NOT", text: "Negates the condition: 'not (tool is READ)' keeps every event whose tool is not READ, including events without a tool. With AND, OR groups, and NOT, any combination of conditions can be written." })),
        h("strong", {}, "3. Values"), valueList, h("strong", {}, "4. Combine ", info("", { title: "Combining conditions", text: "AND adds a condition every event must meet. OR joins this condition with another one, so an event that meets either passes (shown in brackets)." })), combine)),
      h("div", { class: "builder-actions" }, preview, h("button", { class: "btn", type: "button", onclick: () => document.querySelectorAll(".menu").forEach((node) => node.remove()) }, "Cancel"), h("button", { class: "btn primary", type: "button", onclick: submit }, editing ? "Update condition" : "Add condition")));
    update();
    const current = listBox.querySelector("button.on");
    if (current) current.scrollIntoView({ block: "nearest" });
  };
  draw();
  menu(anchor, body).classList.add("wide");
}

// ---------------------------------------------------------------- chrome: rail, crumbs, scope, time, filter bar, search

function currentPage() { return (location.hash.replace(/^#\//, "") || "overview").split("?")[0].split("/")[0]; }
const closedGroups = new Set(storage.read("foxx-nav-closed", []));
const badges = {};
function drawNav(page) {
  const q = (document.getElementById("rail-filter").value || "").trim().toLowerCase();
  put(document.getElementById("nav"), NAV.map(([group, items]) => {
    const shown = items.filter(([, label, zones]) => !q || label.toLowerCase().includes(q) || zones.some(([, zoneLabel]) => zoneLabel.toLowerCase().includes(q)));
    if (!shown.length) return null;
    const closed = !q && closedGroups.has(group) && !shown.some(([id]) => id === page);
    return h("div", { class: "nav-group" + (closed ? " closed" : "") }, h("button", { type: "button", class: "group-head", "aria-expanded": String(!closed), onclick: () => { if (closedGroups.has(group)) closedGroups.delete(group); else closedGroups.add(group); storage.save("foxx-nav-closed", [...closedGroups], localStorage); drawNav(currentPage()); } }, group, h("span", { class: "caret" }, "▾")),
      h("div", { class: "nav-items" }, shown.map(([id, label, zones]) => {
        const active = page === id;
        const badge = badges[id];
        return h("div", {}, h("a", { href: "#/" + id, class: "page-link" + (active ? " active" : ""), title: label, "aria-current": active ? "page" : null }, SVG_ICON(ICONS[id] || ICONS.overview), h("span", { class: "label" }, label), badge ? h("span", { class: "badge " + (badge.tone || ""), title: badge.title }, badge.text) : null),
          (active || q) && zones.length ? h("div", { class: "zones" }, zones.filter(([, zoneLabel]) => !q || active || zoneLabel.toLowerCase().includes(q) || label.toLowerCase().includes(q)).map(([zoneId, zoneLabel]) => h("a", { href: "#/" + id, class: "zone-link", "data-zone": zoneId, onclick: (event) => { if (active) { event.preventDefault(); scrollToZone(zoneId); } else setTimeout(() => scrollToZone(zoneId), 600); } }, zoneLabel))) : null);
      })));
  }));
}
function setBadge(page, text, tone, title) { badges[page] = text ? { text: String(text), tone, title } : null; drawNav(currentPage()); }
function drawCrumbs(page, extra) {
  const entry = NAV.flatMap(([group, items]) => items.map((item) => [group, item])).find(([, [id]]) => id === page);
  const repository = me && me.repositories.find((item) => item.id === scope.repository);
  put(document.getElementById("crumbs"), [scope.account, repository ? repository.remote.replace(/^github\.com\/[^/]+\//, "") : null, entry ? entry[0] : null, entry ? h("b", {}, entry[1][1]) : null, extra ? h("b", {}, extra) : null].filter(Boolean).flatMap((part, position) => (position ? [h("span", { class: "sep" }, "›"), part] : [part])));
}
function watchZones() {
  if (zoneObserver) zoneObserver.disconnect();
  zoneObserver = new IntersectionObserver((entries) => {
    for (const entry of entries) if (entry.isIntersecting) {
      const id = entry.target.id.replace(/^zone-/, "");
      document.querySelectorAll(".zone-link").forEach((node) => node.classList.toggle("current", node.dataset.zone === id));
    }
  }, { rootMargin: "-120px 0px -60% 0px" });
  document.querySelectorAll(".zone").forEach((node) => zoneObserver.observe(node));
}
function drawTime() {
  const custom = filter.since || filter.until;
  const local = (millis) => { if (!millis) return ""; const date = new Date(millis); return new Date(millis - date.getTimezoneOffset() * 60000).toISOString().slice(0, 19); };
  put(document.getElementById("time"), h("legend", { class: "visually-hidden" }, "Time range"), RANGES.map(([name]) => h("button", { type: "button", class: !custom && filter.range === name ? "on" : "", "aria-pressed": String(!custom && filter.range === name), onclick: () => { filter.range = name; filter.since = null; filter.until = null; saveFilter(); render(); } }, name)),
    h("button", { type: "button", class: custom ? "on" : "", title: "A custom range", onclick: (event) => {
      const from = h("input", { type: "datetime-local", step: "0.001", value: local(filter.since || Date.now() - 86400e3), "aria-label": "From" });
      const to = h("input", { type: "datetime-local", step: "0.001", value: local(filter.until || Date.now()), "aria-label": "To" });
      menu(event.currentTarget, h("div", { class: "form" }, h("strong", {}, "Custom range (your local time)"), h("label", {}, "From", from), h("label", {}, "To", to), h("div", { class: "builder-actions" }, h("button", { class: "btn primary", type: "button", onclick: () => { const since = Date.parse(from.value), until = Date.parse(to.value); if (!(since < until)) { to.classList.add("invalid"); return; } filter.since = since; filter.until = until; saveFilter(); document.querySelectorAll(".menu").forEach((node) => node.remove()); render(); } }, "Apply"))));
    } }, "⋯"));
}
function encodeView(value) { return btoa(unescape(encodeURIComponent(JSON.stringify(value)))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, ""); }
function decodeView(text) { try { return JSON.parse(decodeURIComponent(escape(atob(text.replace(/-/g, "+").replace(/_/g, "/"))))); } catch (error) { return null; } }
function drawFilterBar() {
  const bar = document.getElementById("filterbar");
  if (!scope.repository) { bar.hidden = true; return; }
  bar.hidden = false;
  const pills = [];
  if (filter.since || filter.until) {
    pills.push(h("span", { class: "pill time" }, h("b", {}, "time"), " " + rangeText(), h("button", { type: "button", "aria-label": "Zoom out", title: "Zoom out (double the range)", onclick: () => { const end = filter.until || Date.now(), start = filter.since || end - 3600e3, span = end - start; filter.since = Math.floor(start - span / 2); filter.until = Math.min(Date.now(), Math.floor(end + span / 2)); saveFilter(); render(); } }, "⇲"), h("button", { type: "button", "aria-label": "Remove time range", onclick: () => { filter.since = null; filter.until = null; saveFilter(); render(); } }, "×")));
  }
  const groups = new Map();
  filter.clauses.forEach((clause, index) => { const key = clause.group ? "g" + clause.group : "c" + index; if (!groups.has(key)) groups.set(key, []); groups.get(key).push([clause, index]); });
  for (const [key, list] of groups) {
    const pillsOf = list.map(([clause, index]) => h("span", { class: "pill" + (clause.off ? " off" : "") + (clause.exclude ? " not" : ""), title: clause.off ? "Disabled: click ◌ to apply it again" : "" },
      h("button", { type: "button", class: "pill-text", title: "Edit this condition", onclick: (event) => builder(event.currentTarget, index) }, clause.exclude ? h("b", {}, "NOT ") : null, h("b", {}, attrLabel(clause.path)), " " + (OPS[clause.op] || clause.op) + " ", NO_VALUE_OPS.includes(clause.op) ? "" : (clause.values || []).map((value) => valueLabel(clause.path, value)).join(clause.op === "between" ? " and " : ", ")),
      h("button", { type: "button", title: clause.exclude ? "Include instead (remove NOT)" : "Negate (NOT)", "aria-label": "Negate " + clauseText(clause), onclick: () => { clause.exclude = !clause.exclude; saveFilter(); render(); } }, "¬"),
      h("button", { type: "button", "aria-label": clause.off ? "Enable " + clauseText(clause) : "Disable " + clauseText(clause), title: clause.off ? "Enable" : "Disable without removing", onclick: () => { clause.off = !clause.off; saveFilter(); render(); } }, clause.off ? "◌" : "◉"),
      h("button", { type: "button", "aria-label": "Remove " + clauseText(clause), onclick: () => { filter.clauses.splice(index, 1); saveFilter(); render(); } }, "×")));
    if (key.startsWith("g") && list.length > 1) pills.push(h("span", { class: "or-group", title: "Any of these (OR)" }, "(", pillsOf.flatMap((pill, position) => (position ? [h("span", { class: "or" }, "or"), pill] : [pill])), ")"));
    else pills.push(...pillsOf);
  }
  let typing;
  const apply = (value) => { filter.q = value.trim(); saveFilter(); render(); };
  const search = h("input", { type: "search", placeholder: "Search text in any attribute…", value: filter.q, "aria-label": "Search events", oninput: (event) => { clearTimeout(typing); typing = setTimeout(() => apply(event.target.value), 700); }, onkeydown: (event) => { if (event.key === "Enter") { clearTimeout(typing); apply(event.target.value); } } });
  const views = h("button", { class: "btn", type: "button", title: "Saved views and share links", onclick: (event) => {
    const saved = storage.read("foxx-views", []);
    const name = h("input", { type: "text", placeholder: "Name this view", "aria-label": "View name", maxlength: "60" });
    const link2 = location.origin + location.pathname + "#/" + currentPage() + "?view=" + encodeView({ range: filter.range, since: filter.since, until: filter.until, q: filter.q, clauses: filter.clauses });
    menu(event.currentTarget, h("div", { class: "form" }, h("strong", {}, "Saved views"), saved.length ? saved.map((view, position) => h("div", { class: "row-gap" }, h("button", { type: "button", class: "menu-item", style: "flex:1", title: clausesText(view.filter.clauses || []) || "no conditions", onclick: () => { Object.assign(filter, { range: "7d", since: null, until: null, q: "", clauses: [] }, view.filter); saveFilter(); document.querySelectorAll(".menu").forEach((node) => node.remove()); render(); } }, "★ " + view.name), h("button", { type: "button", class: "icon-btn", "aria-label": "Delete view " + view.name, onclick: () => { saved.splice(position, 1); storage.save("foxx-views", saved, localStorage); event.currentTarget.click(); } }, "×"))) : h("p", { class: "muted" }, "No saved views yet."),
      h("div", { class: "row-gap" }, name, h("button", { class: "btn primary", type: "button", onclick: () => { if (!name.value.trim()) { name.classList.add("invalid"); return; } saved.push({ name: name.value.trim(), filter: { range: filter.range, since: filter.since, until: filter.until, q: filter.q, clauses: filter.clauses } }); storage.save("foxx-views", saved, localStorage); document.querySelectorAll(".menu").forEach((node) => node.remove()); } }, "Save current")),
      h("strong", {}, "Share this filter"), h("div", { class: "code-line" }, h("code", {}, link2), h("button", { class: "btn small", type: "button", onclick: (copyEvent) => { const button = copyEvent.currentTarget; navigator.clipboard.writeText(link2).then(() => { button.textContent = "Copied"; }).catch(() => {}); } }, "Copy")), h("p", { class: "muted small" }, "The link carries the time range and every condition; whoever opens it still needs access to the repository.")));
  } }, "★ Views");
  put(bar, h("span", { class: "lead" }, SVG_ICON(FUNNEL), "Filter", info("", { title: "The global filter", text: "Applies to every event view, graph, list, and eval. Add a condition on any attribute of DASHBOARD.md with any operator (is, is not, contains, starts or ends with, matches a * ? pattern, >, ≥, <, ≤, between, exists, is missing), negate any condition (¬), and join conditions with OR (brackets). Conditions combine with AND. Click a condition to edit it, ◉ to disable it, × to remove it. Panels can add their own conditions (+⏚)." })),
    search, pills, h("button", { class: "btn", type: "button", onclick: (event) => builder(event.currentTarget, -1) }, "+ Add condition"), views,
    activeFilters() ? h("button", { class: "btn link", type: "button", onclick: () => { filter.clauses = []; filter.q = ""; filter.since = null; filter.until = null; saveFilter(); render(); } }, "Clear all") : null,
    h("span", { class: "muted right", title: filterSentence({}) }, activeFilters() ? activeFilters() + " active · hover a panel's funnel for its filter in words" : "No filter: every event in range"));
}
/** The search palette: pages and zones, repositories, agents, tools, metrics, sessions, flows, and event names */
async function openPalette() {
  if (document.querySelector(".palette")) return;
  const items = [];
  for (const [, pages] of NAV) for (const [id, label, zones] of pages) {
    items.push({ kind: "Page", label, run: () => go("#/" + id) });
    for (const [zoneId, zoneLabel] of zones) items.push({ kind: "Zone", label: label + " › " + zoneLabel, run: () => { go("#/" + id); setTimeout(() => scrollToZone(zoneId), 600); } });
  }
  for (const account of me ? me.accounts : []) for (const repository of (me.repositories || []).filter((item) => item.account === account.name)) {
    items.push({ kind: "Repository", label: repository.remote.replace(/^github\.com\//, ""), hint: account.kind, run: () => { scope.account = account.name; scope.repository = repository.id; scope.member = ""; saveScope(); go("#/overview"); } });
  }
  for (const [flow, [title]] of Object.entries(G.flows)) items.push({ kind: "Flow", label: flow + " " + title, run: () => drill({ clauses: [is("flow", flow)] }) });
  const input = h("input", { type: "search", placeholder: "Search pages, zones, repositories, agents, tools, sessions, metrics, flows, events…", "aria-label": "Search" });
  const list = h("ul", { role: "listbox" });
  const scrim = h("div", { class: "scrim", onclick: () => close() });
  const palette = h("div", { class: "palette", role: "dialog", "aria-label": "Search" }, input, list);
  const close = () => { palette.remove(); scrim.remove(); };
  document.body.append(scrim, palette);
  input.focus();
  let shown = [], selected = 0;
  const draw = () => {
    const q = input.value.trim().toLowerCase();
    shown = items.filter((item) => !q || (item.kind + " " + item.label + " " + (item.hint || "")).toLowerCase().includes(q)).slice(0, 80);
    selected = Math.min(selected, Math.max(0, shown.length - 1));
    put(list, shown.length ? shown.map((item, index) => h("li", { class: index === selected ? "on" : "", role: "option", "aria-selected": String(index === selected), onclick: () => { close(); item.run(); }, onmousemove: () => { if (selected !== index) { selected = index; draw(); } } }, h("span", { class: "kind" }, item.kind), h("span", {}, item.label), item.hint !== undefined ? h("span", { class: "hint" }, String(item.hint)) : null)) : h("li", { class: "muted" }, "Nothing matches."));
  };
  input.addEventListener("input", () => { selected = 0; draw(); });
  input.addEventListener("keydown", (event) => {
    if (event.key === "Escape") close();
    else if (event.key === "ArrowDown") { selected = Math.min(shown.length - 1, selected + 1); draw(); event.preventDefault(); }
    else if (event.key === "ArrowUp") { selected = Math.max(0, selected - 1); draw(); event.preventDefault(); }
    else if (event.key === "Enter" && shown[selected]) { close(); shown[selected].run(); }
  });
  draw();
  if (!scope.repository) return;
  const [overviewData, sessionData, cache, agentData] = await Promise.all([lastEvals ? Promise.resolve({ evals: lastEvals }) : api("/api/overview", filterParams()).catch(() => ({})), api("/api/sessions").catch(() => ({ sessions: [] })), facets(), api("/api/agents").catch(() => ({ agents: [] }))]);
  for (const [name, value] of Object.entries(overviewData.evals || {})) if (value !== null && typeof value !== "object") items.push({ kind: "Metric", label: name, hint: compact(value), run: () => { go("#/overview"); setTimeout(() => scrollToZone("evals"), 600); } });
  for (const row of sessionData.sessions || []) items.push({ kind: "Session", label: "Session " + shortIdentity(row.session.foxx_session_id), hint: (row.member ? row.member.actor + " · " : "") + row.session.provider, run: () => go("#/sessions/" + encodeURIComponent(row.session.foxx_session_id)) });
  for (const row of agentData.agents || []) items.push({ kind: "Agent", label: agentName(row.agent_id, row.provider), hint: row.provider, run: () => { go("#/agents"); setTimeout(() => openAgent(row.agent_id), 600); } });
  for (const item of (cache.facets || {}).tool || []) items.push({ kind: "Tool", label: item.value, hint: compact(item.count) + " events", run: () => { go("#/tools"); setTimeout(() => openTool(item.value), 600); } });
  for (const item of (cache.facets || {}).event || []) items.push({ kind: "Event", label: item.value, hint: compact(item.count), run: () => drill({ clauses: [is("event_name", item.value)] }) });
  if (document.body.contains(palette)) draw();
}
function pageFilterBar(page) {
  const local = areaFilters.pages[page] || (areaFilters.pages[page] = []);
  return h("div", { class: "page-filter" },
    h("strong", {}, "This section only", info("", { title: "Section filter", text: "Conditions here apply only to this dashboard section. They do not change the global filter or any other section." })),
    scopeFilterRow(local, "Filter section " + page, "Applies only to this dashboard section.", () => {
      document.querySelectorAll(".zone .panel").forEach((node) => node.refresh && node.refresh());
    }));
}
async function controls() {
  if (!me.accounts.some((account) => account.name === scope.account)) {
    const withRepositories = me.accounts.find((account) => me.repositories.some((repository) => repository.account === account.name));
    scope.account = (withRepositories || me.accounts[0]).name;
  }
  const owned = me.repositories.filter((repository) => repository.account === scope.account);
  if (!owned.some((repository) => repository.id === scope.repository)) { scope.repository = owned.length ? owned[0].id : ""; scope.member = ""; }
  members = [];
  if (scope.repository) { try { members = (await api("/api/members")).members; } catch (error) { members = []; } }
  if (scope.member && !members.some((row) => row.member.key === scope.member)) scope.member = "";
  saveScope();
  const labelled = (label, control) => h("label", {}, label, control);
  const changeScope = (changes) => { Object.assign(scope, changes); retentionInfo = null; saveScope(); render(); };
  put(document.getElementById("scope"),
    labelled("Account", select("Account", scope.account, me.accounts.map((account) => [account.name, (account.kind === "personal" ? "Personal · " : "Org · ") + account.name]), (value) => changeScope({ account: value, repository: "", member: "" }))),
    labelled("Repo", select("Repository", scope.repository, owned.length ? owned.map((repository) => [repository.id, repository.remote.replace(/^github\.com\//, "")]) : [["", "no repositories yet"]], (value) => changeScope({ repository: value, member: "" }))),
    labelled("Member", select("Member", scope.member, [["", members.some((row) => row.linked_by) ? "All linked" : "All members"]].concat(members.map((row) => [row.member.key, row.member.actor + " (" + row.member.short + ")" + (row.linked_by ? " @" + row.linked_by : " · unlinked")])), (value) => changeScope({ member: value }))));
  put(document.getElementById("viewer"), h("div", { class: "who" }, h("span", { class: "avatar", "aria-hidden": "true" }, (me.login || "?").slice(0, 1).toUpperCase()), h("span", {}, "Signed in as ", h("b", {}, me.login))), h("a", { href: "/auth/logout" }, "Sign out"));
}
function signIn(message) {
  for (const id of ["scope", "viewer", "nav", "time", "filterbar", "crumbs"]) document.getElementById(id).replaceChildren();
  document.getElementById("filterbar").hidden = true;
  put(document.getElementById("main"), h("div", { class: "signin" },
    h("img", { src: "/logo.png?v=2", alt: "DataSea", width: "120", height: "120" }),
    h("h2", {}, "Observability for your AI agents"),
    h("p", { class: "muted" }, "Sign in with GitHub to see every tool call your agents make: Crane's decisions, the tokens each call burnt, sessions, risks, and the rules they follow. Personal accounts see their own repositories; organization members see the organization's repositories with a filter for each teammate."),
    message ? h("p", { class: "notice error" }, message) : null,
    h("a", { class: "btn primary", href: "/auth/login" }, "Sign in with GitHub")));
}

// ---------------------------------------------------------------- shared views used by several pages

function needRepository(main) {
  if (scope.repository) return false;
  main.append(h("div", { class: "notice" }, "Choose a repository in the top bar, or open ", link("Repositories", "#/repositories"), " for a summary of the account."));
  return true;
}
function repositoryName() { const repository = me && me.repositories.find((item) => item.id === scope.repository); return repository ? repository.remote.replace(/^github\.com\//, "") : "repository"; }
function rangeLabel() {
  if (filter.since || filter.until) return dt(filter.since) + " → " + (filter.until ? dt(filter.until) : "now");
  return filter.range === "All" ? "all time" : "last " + filter.range;
}
function hero(title, subtitle, facts, actions) {
  return h("div", { class: "hero" }, h("div", { style: "position:relative;z-index:1" }, h("h1", {}, title), h("div", { class: "sub" }, subtitle)), h("div", { class: "facts" }, facts.map(([label, value, key]) => h("div", { class: "fact" }, h("span", {}, label, key ? info(key) : null), h("b", {}, value)))), actions ? h("div", { class: "hero-actions" }, actions) : null);
}
function evalsTable(evals) {
  const rows = Object.entries(evals || {}).filter(([, value]) => value !== null && !(typeof value === "object" && !Array.isArray(value) && !Object.keys(value).length));
  return dataTable([{ label: "Eval", get: (row) => row[0], render: (row) => h("span", {}, h("code", {}, row[0]), info("eval:" + row[0])) }, { label: "Value", get: (row) => (typeof row[1] === "number" ? row[1] : fmt(row[1])), render: (row) => fmt(row[1]) }], rows, { sort: 0, dir: 1, pageSize: 15, placeholder: "Search evals…", noColumns: true, name: "foxx-evals" });
}
/** The tool call table: every call with its tool, decision, time, tokens, and trace */
function callsTable(calls, options) {
  return dataTable([
    { label: "", render: (row) => h("span", { class: "sev", style: "background:" + color(row.severity, 0) }) },
    { label: "Start", get: (row) => row.start, render: (row) => h("span", { class: "mono" }, dt(row.start)), csv: (row) => dt(row.start) },
    { label: "Tool", get: (row) => row.tool, render: (row) => h("code", {}, row.tool || "—"), filter: true, info: "facet:tool" },
    { label: "Decision", get: (row) => row.decision, render: (row) => chip(row.decision), filter: true, info: "facet:decision" },
    { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true },
    { label: "Tool time", get: (row) => row.tool_ms, render: (row) => duration(row.tool_ms), num: true, bar: true, format: duration, info: "eval:tool_duration_ms" },
    { label: "Hook latency", get: (row) => row.latency_ms, render: (row) => duration(row.latency_ms), num: true, info: "eval:latency_ms", hidden: options && options.compact },
    { label: "Tokens", get: (row) => (row.tokens ? row.tokens.total : null), render: (row) => (row.tokens ? tokens(row.tokens.total) : h("span", { class: "muted" }, "—")), num: true, bar: true, format: tokens, info: "eval:model_tokens" },
    { label: "Output tok", get: (row) => (row.tokens ? row.tokens.output : null), render: (row) => (row.tokens ? compact(row.tokens.output) : "—"), num: true, hidden: true },
    { label: "Cache read", get: (row) => (row.tokens ? row.tokens.cache_read : null), render: (row) => (row.tokens ? compact(row.tokens.cache_read) : "—"), num: true, hidden: true },
    { label: "Agent", get: (row) => row.agent_id || row.agent, render: (row) => agentName(row.agent_id, row.agent), filter: true },
    { label: "Model", get: (row) => row.model, filter: true, hidden: options && options.compact },
    { label: "Targets", get: (row) => (row.targets || []).join(", "), render: (row) => h("span", { class: "muted clip" }, (row.targets || []).join(", ") || "—") },
    { label: "Session", get: (row) => row.session, render: (row) => sessionLink(row.session), hidden: options && options.compact },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true, hidden: true },
  ], calls, Object.assign({ sort: 1, dir: -1, pageSize: 25, onRow: (row) => openTrace(row.id), name: "foxx-tool-calls", placeholder: "Search tool calls, targets, agents…" }, options || {}));
}
/** The tool calls in range and filter (traces that are tool calls), newest first */
async function toolCalls(extra) {
  const data = await api("/api/traces", filterParams(Object.assign({ kind: "TOOL_CALL", limit: 500 }, extra || {})));
  return data.traces.filter((trace) => trace.tool);
}
/** The usage metrics (calls, time, tokens) grouped by tool, agent, session, model, or member */
async function usage(by, extra) { return api("/api/usage", filterParams(Object.assign({ by }, extra || {}))); }

// ---------------------------------------------------------------- render

async function render() {
  tooltip.hide();
  document.querySelectorAll(".menu").forEach((node) => node.remove());
  const [path, search] = location.hash.replace(/^#\//, "").split("?");
  if (search) {
    const params = new URLSearchParams(search);
    for (const [key, value] of params) {
      const facet = FACETS.find(([name]) => name === key);
      if (facet) toggleValue(facet[1], value);
      else if (key === "session") setClause("session.session_id", "is", [value]);
      else if (key === "view") { const view = decodeView(value); if (view) Object.assign(filter, { range: "7d", since: null, until: null, q: "", clauses: [] }, view); }
    }
    saveFilter();
    history.replaceState(null, "", "#/" + path);
  }
  const [page, id] = (path || "overview").split("/");
  currentDashboardPage = page;
  if (page === "events") { go("#/explorer"); return; }
  facetCache = null;
  catalog = null;
  drawNav(page);
  drawTime();
  const main = document.getElementById("main");
  put(main, h("p", { class: "skeleton" }, "Loading…"));
  try {
    me = await api("/api/me");
    await controls();
    drawCrumbs(page, page === "sessions" && id ? decodeURIComponent(id) : null);
    drawFilterBar();
    put(main);
    if (scope.repository && page !== "dashboard-agents") main.append(pageFilterBar(page));
    if (approval) { main.append(h("div", { class: "notice" + (approval.ok ? " ok" : " error"), role: "status" }, approval.message)); approval = null; }
    const pages = Object.assign({}, window.FOXX_PAGES || {});
    if (page === "sessions" && id) await pages.session(main, decodeURIComponent(id));
    else await (pages[page] || pages.overview)(main);
    watchZones();
    refreshBadges();
  } catch (error) {
    if (error instanceof SignedOut) signIn();
    else if (error.status === 503) signIn(error.message);
    else put(main, h("div", { class: "notice error" }, "Could not load: " + error.message));
  }
}
/** The rail's badges: firing alert rules, controls not satisfied, and failing policies */
let badgeStamp = 0;
async function refreshBadges() {
  if (!scope.repository || Date.now() - badgeStamp < 60000) return;
  badgeStamp = Date.now();
  const [alerting, compliance] = await Promise.all([api("/api/alerting").catch(() => null), api("/api/compliance").catch(() => null)]);
  if (alerting) { const firing = alerting.rules.filter((rule) => rule.state === "FIRING").length; badges.alerting = firing ? { text: firing, tone: "hot", title: firing + " rule(s) firing" } : alerting.rules.length ? { text: alerting.rules.length, title: alerting.rules.length + " rule(s), none firing" } : null; }
  if (compliance) { const bad = (compliance.outcomes || {}).NOT_SATISFIED || 0, partial = (compliance.outcomes || {}).PARTIAL || 0; badges.compliance = bad ? { text: bad, tone: "hot", title: bad + " control(s) not satisfied" } : partial ? { text: partial, tone: "warm", title: partial + " control(s) partial" } : null; }
  drawNav(currentPage());
}

/** Apply the chosen theme (system, light, or dark) */
function applyTheme() {
  const theme = storage.read("foxx-theme", "system");
  if (theme === "system") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = theme;
  const button = document.getElementById("theme");
  button.textContent = theme === "dark" ? "☾" : theme === "light" ? "☀" : "◐";
  button.title = "Theme: " + theme + " (click to change)";
}
applyTheme();
