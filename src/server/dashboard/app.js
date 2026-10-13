"use strict";
// The Foxx dashboard page. Sign in with GitHub, choose an account, a repository, and optionally one
// machine, then browse pages grouped in the left rail; each page is split into zones (listed under
// it in the rail), and each zone into panels focused on one thing. A time range and a global filter
// (any attribute of DASHBOARD.md, any operator, combined with AND and OR) apply to every event view.
// Every panel can be explained (info icon), refreshed, expanded, shown in monitor mode (full screen,
// refreshed every 30 seconds), or collapsed; details open in the right-hand sidebar. Every time is
// written as DD:MM:YYYY:HH:MM:SS. Charts and helpers come from charts.js, explanations from
// glossary.js. The only writes are approvals and credit grants, which GitHub authorizes.

const { h, svg, put, dt, duration, fmt, compact, color, hashColor, tooltip, tip, charts } = window.Foxx;
const G = window.FOXX_GLOSSARY || { flows: {}, groups: {}, values: {}, evals: {}, facets: {} };

// ---------------------------------------------------------------- pages and their zones

const ICONS = {
  overview: "M3 3h7v7H3zM14 3h7v4h-7zM14 10h7v11h-7zM3 13h7v8H3z", explorer: "M4 5h16M4 10h16M4 15h10M4 20h7", timeline: "M4 6h10M8 12h12M4 18h7M4 3v18",
  logs: "M5 4h14v16H5zM8 8h8M8 12h8M8 16h5", traces: "M4 6h6v4H4zM10 8h4M14 6h6v4h-6zM7 10v6h6", topology: "M5 5a2 2 0 1 0 0 .1M19 5a2 2 0 1 0 0 .1M12 19a2 2 0 1 0 0 .1M6 6l5 11M18 6l-5 11",
  distributions: "M4 20V12M9 20V6M14 20V10M19 20V15M3 20h18", semantics: "M4 5h16v10H9l-5 4zM8 9h8M8 12h5", sessions: "M4 4h16v12H4zM8 20h8M12 16v4",
  agents: "M12 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM4 20c1-4 4-6 8-6s7 2 8 6", alerting: "M12 3a6 6 0 0 1 6 6v4l2 3H4l2-3V9a6 6 0 0 1 6-6zM10 19a2 2 0 0 0 4 0",
  credits: "M12 3v18M17 7H9.5a3 3 0 0 0 0 6h5a3 3 0 0 1 0 6H6", audit: "M6 3h9l4 4v14H6zM9 9h6M9 13h6M9 17h4", governance: "M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z",
  compliance: "M5 12l4 4 10-10", repositories: "M4 4h6l2 2h8v14H4z", settings: "M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2",
};
const NAV = [
  ["Monitor", [
    ["overview", "Overview", [["health", "Health"], ["activity", "Activity"], ["decisions", "Decisions"], ["security", "Security"], ["people", "Agents and members"], ["recent", "Recent findings"], ["evals", "All evals"]]],
    ["explorer", "Event explorer", [["volume", "Volume"], ["stream", "Event stream"]]],
    ["timeline", "Timeline", [["gantt", "Gantt chart"], ["swimlanes", "Event timeline"]]],
  ]],
  ["Investigate", [
    ["logs", "Logs", [["log-table", "Log table"]]],
    ["traces", "Traces", [["trace-durations", "Durations"], ["trace-table", "Trace table"]]],
    ["topology", "Topology", [["who", "Who used what"], ["dependencies", "Flow dependencies"]]],
    ["distributions", "Distributions", [["histogram", "Histogram"], ["breakdowns", "Breakdowns"]]],
    ["semantics", "Texts and reasons", [["words", "Words"], ["texts", "Statements and reasons"], ["lines", "Log lines"]]],
    ["sessions", "Sessions", [["session-kpis", "Summary"], ["session-activity", "Activity"], ["session-list", "Sessions"]]],
    ["agents", "Agents", [["agent-charts", "Charts"], ["agent-list", "Agents"]]],
  ]],
  ["Operate", [
    ["alerting", "Alerting", [["rules", "Rules"], ["rule-values", "Rule values"], ["firings", "Firings"], ["deliveries", "Deliveries"], ["alert-events", "Alert events"], ["hook-alerts", "Hook alerts"], ["build", "Build a rule"]]],
    ["credits", "Credits", [["credit-summary", "Summary"], ["balances", "Balances"], ["grants", "Grants"]]],
  ]],
  ["Govern", [
    ["audit", "Audit trail", [["audit-activity", "Activity"], ["audit-trail", "Governance changes"], ["command-events", "Command events"]]],
    ["governance", "Governance", [["gov-state", "State"], ["gov-rules", "Policies and selections"], ["gov-more", "Checkpoints and context"], ["gov-findings", "Findings"]]],
    ["compliance", "Compliance", [["outcomes", "Outcomes"], ["controls", "Controls"], ["inferred", "Inferred observability"], ["unobservable", "Not observable"]]],
  ]],
  ["Organization", [
    ["repositories", "Repositories", []],
    ["settings", "Settings", [["approvals", "Approvals"], ["members", "Members"], ["health-zone", "System health"], ["integrations", "Integrations"], ["signins", "Your sign-ins"]]],
  ]],
];
const PAGE_GUIDE = {
  overview: "The repository at a glance, for the time range and filter: how many tool calls Crane decided and how, security outcomes, who was active, and the newest violations and alerts. Every number comes from the event stream of DASHBOARD.md; click one to see the events behind it.",
  explorer: "Every event, newest first, with a count for each value of each facet on the left. Tick values to filter every view; open an event to see all thirteen attribute groups.",
  timeline: "When things happened. The Gantt chart draws each traced action (a tool call from its decision to its verification and credit settlement, a command, an alert) as a bar on a row per session or any attribute; the event timeline draws every event as a dot on a lane per flow or any attribute. Drag across either to zoom in.",
  logs: "A detailed log table of the events matching the global filter. Add any attribute as a column, filter columns, sort, search, and export CSV.",
  traces: "A trace is every event of one tool call, command, or alert (joined by action.action_id). The table lists traces with their duration, outcome, and events; open one for its waterfall.",
  topology: "Who used what: members, the agents they ran, the tools those agents called, and the resources those tools touched; and which flow tends to follow which within a session.",
  distributions: "How numbers are spread: trace durations, gaps between a session's events, events per session, and every numeric eval attribute (hook latency, tool duration, …).",
  semantics: "The texts events carry: response statements, decision reasons and rules, failure states, assessments, end reasons, and the words used most often.",
  sessions: "Agent sessions with their status, safety, and evals. Open one for its timeline, Gantt chart, action graph, tasks, and downloadable report and evidence.",
  agents: "Every agent identity across all its sessions.",
  alerting: "Alert rules are built on a member's machine with 'crane alert add' (never here): a metric, a threshold, a window, a filter, and Slack or WhatsApp. Each background sync evaluates them; this page shows every rule, its value against its threshold, every firing, and every delivery with its HTTP result.",
  credits: "Autonomy credits bound what an agent may do on its own. Admins and owners of the repository can grant credits to an agent's session here; GitHub confirms the role, and the session's machine applies the grant at its next sync.",
  audit: "The signed governance history (who changed which rule, when) and every Crane command run in the repository.",
  governance: "The governance state as last reported by a member's machine: policies and their results, selections and where they are, checkpoints, policy context, and findings.",
  compliance: "Each control is evaluated only from evidence Crane holds. Gaps can be fixed; limitations are inherent to the method and do not lower an outcome. Network egress is not a control (hooks cannot see traffic), so it is shown as inferred observability.",
  repositories: "Every repository of the account where a member's Crane uploaded and proved its GitHub remote.",
  settings: "Approvals, members and their machines, system health, integrations, and your own sign-ins.",
};

// ---------------------------------------------------------------- state

const RANGES = [["15m", 15 * 60e3], ["1h", 3600e3], ["4h", 4 * 3600e3], ["1d", 86400e3], ["7d", 7 * 86400e3], ["30d", 30 * 86400e3], ["All", null]];
const FACETS = [["severity", "severity.severity_status"], ["flow", "flow"], ["decision", "decision.decision"], ["enforcement", "decision.enforcement"], ["alert", "severity.alert_kind"], ["origin", "origin"], ["agent", "agent.agent_type"], ["source", "action.action_source"], ["event", "event_name"], ["response", "response.response_status"], ["tool", "action.action_tool"], ["member", "member_key"]];
const FACET_LABELS = { severity: "Severity", flow: "Flow", decision: "Decision", enforcement: "Enforcement", alert: "Alert kind", origin: "Origin", agent: "Agent type", source: "Action source", event: "Event name", response: "Response", tool: "Tool", member: "Member" };
const OPS = { is: "is", not: "is not", contains: "contains", not_contains: "does not contain", prefix: "starts with", gt: ">", gte: "≥", lt: "<", lte: "≤", exists: "exists", missing: "is missing" };
const OPS_BY_KIND = { text: ["is", "not", "contains", "not_contains", "prefix", "exists", "missing"], number: ["is", "not", "gt", "gte", "lt", "lte", "exists", "missing"], boolean: ["is", "not", "exists", "missing"], list: ["is", "not", "contains", "not_contains", "exists", "missing"] };
const TONES = {
  ALLOW: "ok", COMPLIANT: "ok", SATISFIED: "ok", PASS: "ok", ACTIVE: "ok", OBSERVED: "ok", verified: "ok", LINKED: "ok", OK: "ok", SUCCESS: "ok", COMPLETED: "ok", SIGNED: "ok", PREVENTED: "ok", DELIVERED: "ok", APPLIED: "ok", RESOLVED: "ok",
  INFO: "info", MACHINE: "info", SERVICE: "info", DERIVED: "info", WORKFLOW: "info", QUEUED: "info", ENDED: "info", LOW: "low",
  REQUIRE_APPROVAL: "warn", DEGRADED: "warn", PARTIAL: "warn", INFERRED: "warn", PENDING: "warn", SUSPICIOUS: "warn", COVERAGE_GAP: "warn", STALE: "warn", NO_EVIDENCE: "warn", MEDIUM: "warn", UNLINKED: "warn", UNSIGNED: "warn", UNKNOWN: "warn", DRY_RUN: "warn", NOT_APPLICABLE: "",
  DENY: "bad", VIOLATION: "bad", FAIL: "bad", NOT_SATISFIED: "bad", DETECTED_AFTER_EFFECT: "bad", UNRESOLVED: "bad", HIGH: "bad", ERROR: "bad", missing: "bad", broken: "bad", FAILED: "bad", FORBIDDEN: "bad", BLOCKED: "bad", BYPASS_ATTEMPT: "bad", REFUSED: "bad", FIRING: "bad", REJECTED: "bad",
  QUARANTINE: "crit", QUARANTINED: "crit", BYPASS_CONFIRMED: "crit", CRITICAL: "crit",
};
const scope = { account: "", repository: "", member: "" };
const filter = { range: "7d", since: null, until: null, q: "", clauses: [] };
let me = null;
let members = [];
let catalog = null;
let facetCache = null;
let lastEvals = null;
let approval = null;
let live = null;
{
  const params = new URLSearchParams(location.search);
  if (params.has("approval")) {
    approval = { ok: params.get("ok") === "1", message: params.get("approval") };
    history.replaceState(null, "", location.pathname + location.hash);
  }
}
const storage = {
  load(key, into) { try { Object.assign(into, JSON.parse(sessionStorage.getItem(key) || "{}")); } catch (error) { /* storage unavailable */ } },
  save(key, value) { try { sessionStorage.setItem(key, JSON.stringify(value)); } catch (error) { /* storage unavailable */ } },
};
storage.load("foxx-scope", scope);
storage.load("foxx-filter-v2", filter);
if (!Array.isArray(filter.clauses)) filter.clauses = [];
const saveScope = () => storage.save("foxx-scope", scope);
const saveFilter = () => storage.save("foxx-filter-v2", filter);

// ---------------------------------------------------------------- small helpers

function chip(value, title) { return value === null || value === undefined || value === "" ? h("span", { class: "muted" }, "—") : h("span", { class: "chip " + (TONES[value] || ""), title: title || meaning(value) || undefined }, String(value)); }
function member(value) { return value ? h("span", { class: "member", title: value.key }, value.actor + " ", h("code", { class: "muted" }, value.short)) : h("span", { class: "muted" }, "—"); }
function percent(value) { return value === null || value === undefined ? "—" : (value * 100).toFixed(1) + "%"; }
function link(text, hash) { return h("a", { href: hash }, text); }
function go(hash) { if (location.hash === hash) render(); else location.hash = hash; }
function sessionOf(spec) { return spec.session && spec.session.session_id ? spec.session.session_id : null; }
function facetPath(name) { const found = FACETS.find(([facet]) => facet === name); return found ? found[1] : name; }
function facetName(path) { const found = FACETS.find(([, known]) => known === path); return found ? found[0] : null; }
function attrLabel(path) { const facet = facetName(path); return facet ? FACET_LABELS[facet].toLowerCase() : path; }
function memberLabel(key) { const row = members.find((item) => item.member.key === key); return row ? row.member.actor + " (" + row.member.short + ")" : String(key).slice(0, 8); }
function valueLabel(path, value) { return path === "member_key" ? memberLabel(value) : value; }

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
  if (kind === "eval") return { title: rest, text: G.evals[rest] || G.evals[rest.split(".")[0]] || "An eval computed from the events." };
  if (kind === "facet") return { title: FACET_LABELS[rest] || rest, text: G.facets[rest] || "" };
  if (kind === "group" && G.groups[rest]) return { title: rest + " group", text: "The " + rest + " group: " + G.groups[rest].title + "." };
  if (kind === "attr") {
    const [group, sub] = rest.split(".");
    const facet = facetName(rest);
    const definition = sub && G.groups[group] && G.groups[group].keys[sub];
    return { title: rest, text: (facet ? G.facets[facet] + " " : "") + (definition ? "Values: " + definition : sub ? "A " + group + " attribute specific to some flows." : G.values[rest] || "An envelope attribute.") };
  }
  if (kind === "page") return { title: rest, text: PAGE_GUIDE[rest] || "" };
  return { title: key, text: rest };
}
/** An info icon: hover for a short explanation, click to read it in the sidebar */
function info(key, custom) {
  const detail = custom || explain(key);
  if (!detail.text) return null;
  return h("button", { type: "button", class: "info-icon", "aria-label": "About " + detail.title,
    onmouseenter: (event) => tooltip.show(event, tip(detail.title, [], detail.text)), onmousemove: (event) => tooltip.show(event, tip(detail.title, [], detail.text)), onmouseleave: tooltip.hide, onfocus: (event) => { const box = event.target.getBoundingClientRect(); tooltip.show({ clientX: box.right, clientY: box.bottom }, tip(detail.title, [], detail.text)); }, onblur: tooltip.hide,
    onclick: (event) => { event.stopPropagation(); tooltip.hide(); details.open(detail.title, h("div", { class: "explain" }, h("p", {}, detail.text), key.startsWith("flow:") ? h("p", {}, h("button", { class: "btn", onclick: () => { details.close(); setClause("flow", "is", [key.slice(5)]); go("#/explorer"); } }, "Show the events of " + key.slice(5))) : null)); } }, "i");
}
/** A value with its info icon, when the glossary explains it */
function explained(value) { return meaning(value) ? info("value:" + value) : null; }

// ---------------------------------------------------------------- the details sidebar

const details = {
  open(title, content, wide) {
    const pane = document.getElementById("details");
    put(pane, h("header", {}, h("h3", {}, title), h("button", { class: "icon-btn", "aria-label": "Close details", onclick: () => details.close() }, "✕")), h("div", { class: "content" }, content));
    pane.classList.toggle("wide", Boolean(wide));
    document.getElementById("app").classList.add("details-open");
    pane.hidden = false;
    pane.querySelector("button").focus();
  },
  close() {
    document.getElementById("details").hidden = true;
    document.getElementById("app").classList.remove("details-open");
  },
  guide() {
    const page = currentPage();
    const entry = NAV.flatMap(([, items]) => items).find(([id]) => id === page);
    details.open("About this page", h("div", { class: "explain" }, h("p", {}, PAGE_GUIDE[page] || ""), entry && entry[2].length ? h("div", {}, h("h4", {}, "Zones"), h("ul", {}, entry[2].map(([id, label]) => h("li", {}, h("a", { href: "#", onclick: (event) => { event.preventDefault(); scrollToZone(id); } }, label))))) : null,
      h("h4", {}, "Reading the dashboard"), h("ul", {}, h("li", {}, "Hover any ", h("span", { class: "info-icon inline" }, "i"), " for what a number, value, or flow means; click it to read more here."), h("li", {}, "Click a bar, slice, dot, or KPI to see the events behind it; drag across a time chart to zoom in."), h("li", {}, "Every panel can be refreshed, expanded, shown in monitor mode (full screen, live), or collapsed from its header."), h("li", {}, "Times are DD:MM:YYYY:HH:MM:SS in your local time."))));
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
/** The global filter as request parameters; a panel's own clauses and settings are added */
function filterParams(extra) {
  extra = Object.assign({}, extra || {});
  const params = {};
  if (filter.since || filter.until) { if (filter.since) params.since = Math.floor(filter.since); if (filter.until) params.until = Math.floor(filter.until); }
  else { const range = RANGES.find(([name]) => name === filter.range); if (range && range[1]) params.since = Math.floor(Date.now() - range[1]); }
  if (filter.q) params.q = filter.q;
  const clauses = filter.clauses.filter((clause) => !clause.off).map(({ path, op, values, group }) => (group ? { path, op, values, group } : { path, op, values })).concat(extra.clauses || []);
  delete extra.clauses;
  if (extra.ignoreTime) { delete params.since; delete params.until; }
  delete extra.ignoreTime;
  if (clauses.length) params.filter = JSON.stringify(clauses);
  return Object.assign(params, extra);
}
function activeFilters() { return filter.clauses.filter((clause) => !clause.off).length + (filter.q ? 1 : 0) + (filter.since || filter.until ? 1 : 0); }
/** Set (or with no values, remove) the plain clause of an attribute and operator */
function setClause(path, op, values) {
  path = facetPath(path);
  filter.clauses = filter.clauses.filter((clause) => !(clause.path === path && clause.op === op && !clause.group));
  if (values && values.length) filter.clauses.push({ path, op, values });
  saveFilter();
}
function toggleValue(path, value) {
  path = facetPath(path);
  const existing = filter.clauses.find((clause) => clause.path === path && clause.op === "is" && !clause.group);
  const values = new Set(existing ? existing.values : []);
  if (values.has(value)) values.delete(value); else values.add(value);
  setClause(path, "is", [...values]);
}
function isChosen(path, value) { return filter.clauses.some((clause) => clause.path === facetPath(path) && clause.op === "is" && !clause.off && clause.values.includes(value)); }
/** Open the events of a choice (a KPI, bar, slice, node, or value) */
function drill(changes) {
  for (const clause of changes.clauses || []) filter.clauses.push(clause);
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

// ---------------------------------------------------------------- panels and zones

let zoneObserver = null;
/** A zone: one subsection of a page, listed in the rail */
function zone(id, title, guide, ...children) {
  return h("section", { class: "zone", id: "zone-" + id, "aria-labelledby": "zone-title-" + id }, h("h2", { class: "zone-title", id: "zone-title-" + id }, title, guide ? info("", { title, text: guide }) : null), h("div", { class: "grid" }, children));
}
function scrollToZone(id) { const node = document.getElementById("zone-" + id); if (node) node.scrollIntoView({ behavior: "smooth", block: "start" }); }
/** A panel focused on one thing: header with title, explanation, its own controls, refresh,
 * expand, monitor, and collapse; draw(body, big) fills it and runs again on refresh */
function panel(title, options, draw) {
  options = options || {};
  const body = h("div", { class: "body" + (options.flush ? " flush" : "") });
  const controls = h("div", { class: "controls" });
  const node = h("section", { class: "panel span-" + (options.span || 12) });
  const run = async (target, big) => {
    put(target, h("div", { class: "skeleton" }, "Loading…"));
    try { await draw(target, big || false); } catch (error) { if (error instanceof SignedOut) { signIn(); return; } put(target, h("div", { class: "notice error" }, error.message)); }
  };
  node.refresh = () => run(body);
  const tools = h("div", { class: "tools" },
    h("button", { class: "icon-btn", type: "button", title: "Refresh", "aria-label": "Refresh " + title, onclick: () => node.refresh() }, "↻"),
    options.noExpand ? null : h("button", { class: "icon-btn", type: "button", title: "Expand", "aria-label": "Expand " + title, onclick: () => overlay(title, (target) => run(target, true), false) }, "⤢"),
    options.noExpand ? null : h("button", { class: "icon-btn", type: "button", title: "Monitor view: full screen, refreshed every 30 seconds", "aria-label": "Monitor " + title, onclick: () => overlay(title, (target) => run(target, true), true) }, "▣"),
    h("button", { class: "icon-btn", type: "button", title: "Collapse", "aria-label": "Collapse " + title, "aria-expanded": "true", onclick: (event) => { const collapsed = node.classList.toggle("collapsed"); event.currentTarget.setAttribute("aria-expanded", String(!collapsed)); event.currentTarget.textContent = collapsed ? "▸" : "▾"; } }, "▾"));
  put(node, h("header", {}, h("h3", {}, title, options.info ? (typeof options.info === "string" ? info(options.info) : info("", options.info)) : null), controls, tools), body);
  node.controls = controls;
  node.body = body;
  run(body);
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
function select(label, value, choices, onchange) {
  return h("select", { "aria-label": label, title: label, onchange: (event) => onchange(event.target.value) },
    choices.map((choice) => {
      if (choice.group) return h("optgroup", { label: choice.group }, choice.options.map(([optionValue, text]) => { const node = h("option", { value: optionValue }, text); if (optionValue === value) node.selected = true; return node; }));
      const node = h("option", { value: choice[0] }, choice[1]);
      if (choice[0] === value) node.selected = true;
      return node;
    }));
}
/** The choices of a panel's own filter: each value of each facet */
function facetChoices(cache) {
  return [["", "All events"]].concat(FACETS.map(([name, path]) => ({ group: FACET_LABELS[name], options: ((cache.facets || {})[name] || []).slice(0, 12).map((item) => [path + "=" + item.value, (item.label || item.value) + " (" + item.count + ")"]) })).filter((group) => group.options.length));
}
function panelClause(value) { if (!value) return []; const index = value.indexOf("="); return [{ path: value.slice(0, index), op: "is", values: [value.slice(index + 1)] }]; }
const GROUP_CHOICES = [["", "No grouping"]].concat(FACETS.map(([name]) => [name, "By " + FACET_LABELS[name].toLowerCase()]));
function kpi(label, value, options) {
  options = options || {};
  return h("div", { class: "kpi " + (options.tone || ""), role: options.drill ? "button" : null, tabindex: options.drill ? "0" : null, title: options.drill ? "See these events" : undefined, onclick: options.drill || undefined, onkeydown: options.drill ? (event) => { if (event.key === "Enter") options.drill(); } : undefined },
    h("div", { class: "label" }, label, options.info ? info(options.info) : null), h("div", { class: "value" }, options.format ? options.format(value) : compact(value)), options.note ? h("div", { class: "note" }, options.note) : null, charts.sparkline(options.spark, options.color || "#7b5fc7"));
}
/** Events over time with its own group-by, filter, and chart type */
function seriesPanel(title, options) {
  const state = { by: options.by || "", filter: options.filter || "", type: options.type || "bars" };
  const node = panel(title, { span: options.span, info: options.info }, async (body, big) => {
    const data = await api("/api/series", filterParams(Object.assign({ by: state.by, clauses: panelClause(state.filter).concat(options.clauses || []) }, options.params || {})));
    put(body, charts.timeseries(data, { type: state.type, height: big ? 520 : options.height, title, onZoom: zoomTo,
      onPick: ({ since, until, key }) => { const clauses = panelClause(state.filter); if (state.by && key && key !== "OTHER" && key !== "NONE") clauses.push({ path: facetPath(state.by), op: "is", values: [key] }); drill({ clauses: clauses.concat(options.clauses || []), since, until }); } }));
  });
  facets().then((cache) => node.controls.append(
    select("Group by", state.by, GROUP_CHOICES, (value) => { state.by = value; node.refresh(); }),
    select("Filter this chart", state.filter, facetChoices(cache), (value) => { state.filter = value; node.refresh(); }),
    select("Chart type", state.type, [["bars", "Bars"], ["lines", "Lines"], ["area", "Area"]], (value) => { state.type = value; node.refresh(); })));
  return node;
}
/** A breakdown of the events by one facet, as a donut or a top list */
function breakdownPanel(title, options) {
  const state = { by: options.by || "severity", filter: options.filter || "", view: options.view || "donut" };
  const node = panel(title, { span: options.span, info: options.info }, async (body) => {
    const data = await api("/api/facets", filterParams(Object.assign({ clauses: panelClause(state.filter).concat(options.clauses || []) }, options.params || {})));
    const items = (data.facets[state.by] || []).slice(0, options.limit || 10);
    const pick = (value) => drill({ clauses: panelClause(state.filter).concat(options.clauses || []).concat([{ path: facetPath(state.by), op: "is", values: [value] }]) });
    put(body, state.view === "donut" ? charts.donut(items, { title, onPick: pick, explain: explained }) : charts.toplist(items, { onPick: pick, explain: explained }));
  });
  facets().then((cache) => node.controls.append(
    select("Break down by", state.by, FACETS.map(([name]) => [name, "By " + FACET_LABELS[name].toLowerCase()]), (value) => { state.by = value; node.refresh(); }),
    select("Filter this chart", state.filter, facetChoices(cache), (value) => { state.filter = value; node.refresh(); }),
    select("Show as", state.view, [["donut", "Donut"], ["toplist", "Top list"]], (value) => { state.view = value; node.refresh(); })));
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
      cols.map(([column]) => { const cell = column.render ? column.render(row) : text(column, row); return h("td", { class: column.num ? "num" : "" }, cell instanceof Node ? cell : fmt(cell)); })))
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
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) },
    { label: "Session", get: (row) => sessionOf(row), render: (row) => (sessionOf(row) ? link(sessionOf(row).slice(0, 18), "#/sessions/" + encodeURIComponent(sessionOf(row))) : "—") },
    { label: "Origin", get: (row) => row.origin, render: (row) => chip(row.origin), filter: true, info: "facet:origin" },
  ], events, Object.assign({ sort: 1, dir: -1, onRow: openEvent, placeholder: "Search these events…", name: "foxx-events" }, options || {}));
}

// ---------------------------------------------------------------- event and trace details

async function openEventById(id) {
  const data = await api("/api/events", { filter: JSON.stringify([{ path: "event_id", op: "is", values: [id] }]), limit: 1 });
  if (data.events[0]) openEvent(data.events[0]);
}
function openEvent(spec) {
  const value = (path, item) => {
    const shown = fmt(item);
    if (path === "session.session_id" && item) return h("a", { href: "#/sessions/" + encodeURIComponent(item), onclick: () => details.close() }, shown);
    if (typeof item === "string" && item && !/digest|signature|event_id/.test(path)) return h("span", {}, h("a", { href: "#", title: "Filter every view by this value", onclick: (event) => { event.preventDefault(); details.close(); setClause(path, "is", [item]); go("#/explorer"); } }, shown), explained(item));
    return shown;
  };
  const groups = ["action", "repository", "session", "agent", "resource", "response", "assessment", "decision", "eval", "severity", "bindings", "telemetry", "other"].filter((group) => spec[group] && Object.keys(spec[group]).length).map((group) => h("div", { class: "group" }, h("h4", {}, group, info("group:" + group)), h("div", { class: "kv" }, Object.entries(spec[group]).map(([key, item]) => [h("div", {}, key, info("attr:" + group + "." + key)), h("div", {}, value(group + "." + key, item))]))));
  const envelope = h("div", { class: "group" }, h("h4", {}, "envelope"), h("div", { class: "kv" }, ["event_id", "flow", "origin", "occurred_at", "schema_version", "sequence", "digest", "previous_digest"].filter((key) => spec[key] !== undefined && spec[key] !== null).map((key) => [h("div", {}, key), h("div", {}, key === "occurred_at" ? dt(spec[key]) : value(key, spec[key]))]).concat(spec.member ? [[h("div", {}, "member"), h("div", {}, member(spec.member))]] : [])));
  const copy = h("button", { class: "btn", type: "button", onclick: async () => { try { await navigator.clipboard.writeText(JSON.stringify(spec, null, 2)); copy.textContent = "Copied"; } catch (error) { copy.textContent = "Copy failed"; } } }, "Copy JSON");
  const action = (spec.action || {}).action_id;
  details.open(spec.event_name, h("div", {}, h("p", { class: "statement" }, (spec.response || {}).response_statement || ""), h("div", { class: "row-actions" }, copy, action ? h("button", { class: "btn", type: "button", onclick: () => openTrace(action) }, "Open trace") : null, sessionOf(spec) ? h("button", { class: "btn", type: "button", onclick: () => { details.close(); go("#/sessions/" + encodeURIComponent(sessionOf(spec))); } }, "Open session") : null),
    h("p", { class: "muted" }, "Click a value to filter every view by it; hover ", h("span", { class: "info-icon inline" }, "i"), " for what it means."), envelope, groups));
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
  details.open("Trace " + id, h("div", {},
    h("div", { class: "kv" }, [["kind", trace.kind], ["outcome", chip(trace.outcome)], ["severity", chip(trace.severity)], ["decision", chip(trace.decision)], ["start", dt(trace.start)], ["end", dt(trace.end)], ["duration", duration(trace.duration_ms)], ["tool", trace.tool || "—"], ["session", trace.session || "—"], ["member", member(trace.member)], ["flows", h("span", {}, trace.flows.map((flow) => h("span", {}, flow, info("flow:" + flow), " ")))]].map(([key, item]) => [h("div", {}, key), h("div", {}, item instanceof Node ? item : fmt(item))])),
    h("h4", {}, "Waterfall"), h("p", { class: "muted" }, "Every event of the trace at its time after the first; click one for its attributes."), waterfall), true);
}

// ---------------------------------------------------------------- pages

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
  return h("div", { class: "hero" }, h("div", {}, h("h1", {}, title), h("div", { class: "sub" }, subtitle)), h("div", { class: "facts" }, facts.map(([label, value, key]) => h("div", { class: "fact" }, h("span", {}, label, key ? info(key) : null), h("b", {}, value)))), actions ? h("div", { class: "hero-actions" }, actions) : null);
}
const is = (path, value) => ({ path, op: "is", values: [value] });

async function overview(main) {
  if (needRepository(main)) return;
  const [data, decisions, all] = await Promise.all([api("/api/overview", filterParams()), api("/api/series", filterParams({ by: "decision" })), api("/api/series", filterParams())]);
  const evals = data.evals || {};
  lastEvals = evals;
  const points = (key) => { const item = (decisions.series || []).find((series) => series.key === key); return item ? item.points : null; };
  const to = (clauses) => () => drill({ clauses });
  main.append(hero(repositoryName(), [scope.member ? "Member " + memberLabel(scope.member) : "All " + (data.members || 0) + " member(s)", rangeLabel(), activeFilters() ? activeFilters() + " filter(s)" : "no filter"].join(" · "),
    [["Events", compact(evals.events), "eval:events"], ["Active sessions", compact(data.active_sessions)], ["Agents", compact(data.agents)], ["Event chains", data.event_chain.status], ["Governance", data.integrity.reported ? (data.integrity.ok ? "passing" : "failing") : "not reported"]]));
  main.append(zone("health", "Health", "The headline numbers of the range and filter. Each tile is one eval; click it for its events.",
    h("div", { class: "kpis span-12" },
      kpi("Tool calls decided", evals.tool_calls_decided, { info: "eval:tool_calls_decided", spark: all.series && all.series[0] ? all.series[0].points : null, note: compact(evals.events) + " events in all", drill: to([is("flow", "F05")]) }),
      kpi("Allowed", evals.allowed, { info: "eval:allowed", spark: points("ALLOW"), color: "#2fbf7c", tone: "good", drill: to([is("decision.decision", "ALLOW")]) }),
      kpi("Denied", evals.denied, { info: "eval:denied", spark: points("DENY"), color: "#e5484d", tone: evals.denied ? "bad" : "", note: "share " + percent(evals.denial_share), drill: to([is("decision.decision", "DENY")]) }),
      kpi("Approvals", evals.approvals, { info: "eval:approvals", spark: points("REQUIRE_APPROVAL"), color: "#f0a830", drill: to([is("decision.decision", "REQUIRE_APPROVAL")]) }),
      kpi("Evidence completeness", evals.evidence_completeness, { info: "eval:evidence_completeness", format: percent }),
      kpi("Credits available", (evals.credits || {}).available, { info: "eval:credits", note: compact((evals.credits || {}).consumed) + " consumed", drill: () => go("#/credits") }))));
  main.append(zone("activity", "Activity", "How many events happened over time, and how they divide.",
    seriesPanel("Events over time", { span: 8, by: "severity", height: 240, info: "page:overview" }),
    breakdownPanel("Breakdown", { span: 4, by: "severity", info: "facet:severity" })));
  main.append(zone("decisions", "Decisions", "What Crane decided before each tool call (F05).",
    seriesPanel("Tool call decisions", { span: 8, by: "decision", filter: "flow=F05", info: "flow:F05" }),
    breakdownPanel("Decisions", { span: 4, by: "decision", clauses: [is("flow", "F05")], info: "facet:decision" })));
  main.append(zone("security", "Security", "Attempts to get around Crane, what got through, and what went unobserved. Confirmed bypasses should stay 0.",
    h("div", { class: "kpis span-12" },
      kpi("Bypass attempts", evals.bypass_attempts, { info: "eval:bypass_attempts", tone: evals.bypass_attempts ? "bad" : "", drill: to([is("severity.alert_kind", "BYPASS_ATTEMPT")]) }),
      kpi("Confirmed bypasses", evals.bypasses_confirmed, { info: "eval:bypasses_confirmed", tone: evals.bypasses_confirmed ? "crit" : "good", drill: to([is("event_name", "BYPASS_CONFIRMED")]) }),
      kpi("Violations after effect", evals.violations_after_effect, { info: "eval:violations_after_effect", tone: evals.violations_after_effect ? "bad" : "", drill: to([is("event_name", "VIOLATION_AFTER_EFFECT")]) }),
      kpi("Coverage gaps", evals.coverage_gaps, { info: "eval:coverage_gaps", tone: evals.coverage_gaps ? "bad" : "", drill: to([is("decision.enforcement", "COVERAGE_GAP")]) }),
      kpi("Scripts blocked", evals.scripts_blocked, { info: "eval:scripts_blocked", drill: to([is("event_name", "SCRIPT_BLOCKED")]) }),
      kpi("Suspicious prompts", evals.suspicious_prompts, { info: "eval:suspicious_prompts", tone: evals.suspicious_prompts ? "bad" : "", drill: to([is("event_name", "PROMPT_SUSPICIOUS")]) }),
      kpi("Quarantines", evals.quarantines, { info: "eval:quarantines", tone: evals.quarantines ? "crit" : "" }),
      kpi("Signed attestations", evals.attestations_signed, { info: "eval:attestations_signed", drill: to([is("event_name", "SESSION_ATTESTED")]) })),
    seriesPanel("Security signals", { span: 12, by: "alert", clauses: [{ path: "severity.alert_kind", op: "not", values: ["NONE"] }], info: "facet:alert" })));
  main.append(zone("people", "Agents and members", "Who was active.",
    seriesPanel("Activity by agent type", { span: 6, by: "agent", type: "lines", info: "facet:agent" }),
    breakdownPanel("Most active members", { span: 6, by: "member", view: "toplist", limit: 8, info: "facet:member" })));
  main.append(zone("recent", "Recent findings", "The newest violations (confirmed bypasses and violations after effect) and alerts.",
    panel("Recent violations", { span: 6, flush: true, info: "value:VIOLATION" }, async (body) => put(body, eventsTable(data.recent_violations || [], { pageSize: 8, noTools: true, empty: "No violations in this range: the healthy state." }))),
    panel("Recent alerts", { span: 6, flush: true, info: "flow:F14" }, async (body) => put(body, dataTable([
      { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at * 1000)) }, { label: "Alert", get: (row) => row.title }, { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity) }, { label: "Member", render: (row) => member(row.member) },
    ], data.recent_alerts || [], { noTools: true, sort: 0, pageSize: 8, empty: "No alerts." })))));
  main.append(zone("evals", "All evals", "Every number the dashboard computes from the events in range and filter.",
    panel("Evals", { span: 12, flush: true }, async (body) => put(body, evalsTable(evals)))));
}
function evalsTable(evals) {
  const rows = Object.entries(evals || {}).filter(([, value]) => value !== null && !(typeof value === "object" && !Array.isArray(value) && !Object.keys(value).length));
  return dataTable([{ label: "Eval", get: (row) => row[0], render: (row) => h("span", {}, h("code", {}, row[0]), info("eval:" + row[0])) }, { label: "Value", get: (row) => (typeof row[1] === "number" ? row[1] : fmt(row[1])), render: (row) => fmt(row[1]) }], rows, { sort: 0, dir: 1, pageSize: 15, placeholder: "Search evals…", noColumns: true, name: "foxx-evals" });
}

async function explorer(main) {
  if (needRepository(main)) return;
  const cache = await facets();
  const side = h("aside", { class: "facets", "aria-label": "Facets" }, h("div", { class: "head" }, h("span", {}, "Facets", info("", { title: "Facets", text: "Each attribute with a count for each of its values among the events in range and filter. A facet ignores its own selection, so its other values stay visible. Ticking values filters every view (values of one facet combine with OR, facets combine with AND)." })), h("span", { class: "muted" }, compact(cache.total) + " events")),
    FACETS.map(([name, path], index) => {
      const values = (cache.facets || {})[name] || [];
      const chosen = values.filter((item) => isChosen(path, item.value)).length;
      return h("details", { open: index < 6 || chosen ? true : null }, h("summary", {}, h("span", {}, FACET_LABELS[name], info("facet:" + name)), chosen ? h("span", { class: "chip info" }, chosen) : null),
        values.length ? values.map((item) => h("label", {}, h("input", { type: "checkbox", checked: isChosen(path, item.value) ? true : null, onchange: () => { toggleValue(path, item.value); render(); } }), h("span", { title: item.label || item.value }, item.label || item.value), explained(item.value), h("em", {}, compact(item.count))))
          : h("div", { class: "muted none" }, "none"));
    }));
  const right = h("div", { class: "explorer-main" });
  main.append(h("div", { class: "explorer" }, side, right));
  right.append(zone("volume", "Volume", "Events over time in range and filter; drag across to zoom.", seriesPanel("Event volume", { span: 12, by: "severity", height: 180 })));
  let cursor = 0;
  const rows = [];
  const stream = panel("Event stream", { span: 12, flush: true, info: "page:explorer" }, async (body) => {
    const data = await api("/api/events", filterParams({ cursor, limit: 500 }));
    rows.push(...data.events);
    cursor = data.next_cursor;
    put(body, h("div", { class: "muted pad" }, compact(data.total) + " matching events, newest first" + (rows.length < data.total ? " (newest " + rows.length + " loaded)" : "")), eventsTable(rows, { pageSize: 50 }), cursor ? h("div", { class: "pager" }, h("button", { class: "btn", type: "button", onclick: () => stream.refresh() }, "Load 500 more")) : null);
  });
  right.append(zone("stream", "Event stream", "Every matching event; open one for all thirteen attribute groups.", stream));
}

async function attributeOptions() {
  const data = await attributes();
  return (data.attributes || []).filter((attribute) => attribute.kind !== "number" && attribute.distinct > 0 && attribute.distinct < 200 && !facetName(attribute.path) && attribute.path.includes(".")).map((attribute) => [attribute.path, attribute.path]);
}
async function timeline(main) {
  if (needRepository(main)) return;
  const attributeChoices = await attributeOptions();
  const rowChoices = [["session", "Rows: session"], ["agent", "Rows: agent type"], ["agent.agent_id", "Rows: agent"], ["member", "Rows: member"], ["tool", "Rows: tool"], ["flow", "Rows: flow"], ["decision", "Rows: decision"], ["origin", "Rows: origin"]].concat(attributeChoices.filter(([path]) => path !== "agent.agent_id").map(([path, label]) => [path, "Rows: " + label]));
  const colorChoices = [["decision", "Color: decision"], ["severity", "Color: severity"], ["tool", "Color: tool"], ["flow", "Color: flow"], ["enforcement", "Color: enforcement"], ["response", "Color: response"], ["origin", "Color: origin"]].concat(attributeChoices.map(([path, label]) => [path, "Color: " + label]));
  const gantt = { rows: "session", color: "decision", filter: "" };
  const ganttPanel = panel("Gantt chart of agent actions", { span: 12, info: { title: "Gantt chart", text: "One bar per trace: a tool call from its first event (credit reservation or decision) to its last (verification, credit settlement), extended by the measured tool time; or a command, an alert, or any single event. Rows group bars by the chosen attribute; colors show another. A red outline marks a failed trace (refused, failed, or a broken rule). Each session row is shaded from the session's start to its end. Drag across to zoom; click a bar for its trace." } }, async (body, big) => {
    const data = await api("/api/gantt", filterParams({ rows: gantt.rows, color: gantt.color, clauses: panelClause(gantt.filter) }));
    put(body, data.truncated ? h("div", { class: "notice warn" }, "Showing the newest 1,500 of " + compact(data.total) + " traces; narrow the range or filter.") : null, h("div", { class: big ? "tall" : "" }, charts.gantt(data, { title: "Gantt chart", onZoom: zoomTo, onPick: (bar) => openTrace(bar.id) })));
  });
  facets().then((cache) => ganttPanel.controls.append(
    select("Rows", gantt.rows, rowChoices, (value) => { gantt.rows = value; ganttPanel.refresh(); }),
    select("Color", gantt.color, colorChoices, (value) => { gantt.color = value; ganttPanel.refresh(); }),
    select("Filter this chart", gantt.filter, facetChoices(cache), (value) => { gantt.filter = value; ganttPanel.refresh(); })));
  main.append(zone("gantt", "Gantt chart", "The exact time span of every agent action, grouped and colored by any attribute.", ganttPanel));
  const lanes = { lanes: "flow", color: "severity", filter: "" };
  const lanePanel = panel("Event timeline", { span: 12, info: { title: "Event timeline", text: "Every event as a dot at its exact time, on a lane per value of the chosen attribute (flows by default, labelled with their names), colored by another. Drag across to zoom; click a dot for the event." } }, async (body) => {
    const data = await api("/api/timeline", filterParams({ lanes: lanes.lanes, color: lanes.color, clauses: panelClause(lanes.filter) }));
    put(body, data.truncated ? h("div", { class: "notice warn" }, "Showing the newest 5,000 of " + compact(data.total) + " events.") : null, charts.swimlanes(data, { onZoom: zoomTo, onPick: (point) => openEventById(point.id), laneLabel: (key) => (lanes.lanes === "flow" && G.flows[key] ? key + " " + G.flows[key][0].slice(0, 22) : key), laneTitle: (key) => meaning(key) || key }));
  });
  facets().then((cache) => lanePanel.controls.append(
    select("Lanes", lanes.lanes, rowChoices.filter(([value]) => value !== "session").map(([value, label]) => [value, label.replace("Rows", "Lanes")]), (value) => { lanes.lanes = value; lanePanel.refresh(); }),
    select("Color", lanes.color, colorChoices, (value) => { lanes.color = value; lanePanel.refresh(); }),
    select("Filter this chart", lanes.filter, facetChoices(cache), (value) => { lanes.filter = value; lanePanel.refresh(); })));
  main.append(zone("swimlanes", "Event timeline", "Every event at its exact time.", lanePanel));
}

async function logs(main) {
  if (needRepository(main)) return;
  const data = await attributes();
  let saved = [];
  try { saved = JSON.parse(sessionStorage.getItem("foxx-log-columns") || "[]"); } catch (error) { saved = []; }
  const extra = new Set(saved);
  let cursor = 0;
  const rows = [];
  const read = (spec, path) => { let node = spec; for (const part of path.split(".")) node = node === null || node === undefined ? undefined : node[part]; return Array.isArray(node) ? node.join(", ") : node; };
  const table = panel("Log table", { span: 12, flush: true, info: "page:logs" }, async (body) => {
    const page = await api("/api/events", filterParams({ cursor, limit: 500 }));
    rows.push(...page.events);
    cursor = page.next_cursor;
    const columns = [
      { label: "", render: sevBar },
      { label: "Time", get: (row) => row.occurred_at, render: (row) => h("span", { class: "mono" }, dt(row.occurred_at)), csv: (row) => dt(row.occurred_at) },
      { label: "Severity", get: (row) => (row.severity || {}).severity_status, render: (row) => chip((row.severity || {}).severity_status), filter: true, info: "facet:severity" },
      { label: "Flow", get: (row) => row.flow, render: (row) => h("span", {}, row.flow, info("flow:" + row.flow)), filter: true, info: "facet:flow" },
      { label: "Event", get: (row) => row.event_name, render: (row) => h("code", {}, row.event_name), filter: true, info: "facet:event" },
      { label: "Statement", get: (row) => (row.response || {}).response_statement },
      { label: "Decision", get: (row) => (row.decision || {}).decision, render: (row) => chip((row.decision || {}).decision), filter: true, info: "facet:decision" },
      { label: "Rule", get: (row) => (row.decision || {}).decision_rule, filter: true, info: "attr:decision.decision_rule" },
      { label: "Tool", get: (row) => (row.action || {}).action_tool, filter: true, info: "facet:tool" },
      { label: "Response", get: (row) => (row.response || {}).response_status, render: (row) => chip((row.response || {}).response_status), filter: true, info: "facet:response" },
      { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
      { label: "Session", get: (row) => sessionOf(row), render: (row) => (sessionOf(row) ? link(sessionOf(row).slice(0, 18), "#/sessions/" + encodeURIComponent(sessionOf(row))) : "—") },
      { label: "Origin", get: (row) => row.origin, render: (row) => chip(row.origin), filter: true, info: "facet:origin" },
      { label: "Enforcement", get: (row) => (row.decision || {}).enforcement, filter: true, hidden: true, info: "facet:enforcement" },
      { label: "Agent", get: (row) => (row.agent || {}).agent_type, filter: true, hidden: true, info: "facet:agent" },
      { label: "Source", get: (row) => (row.action || {}).action_source, filter: true, hidden: true, info: "facet:source" },
      { label: "Event id", get: (row) => row.event_id, hidden: true },
    ].concat([...extra].map((path) => ({ label: path, get: (row) => read(row, path), filter: true, info: "attr:" + path })));
    put(body, h("div", { class: "pad muted" }, compact(page.total) + " matching log lines, newest first" + (rows.length < page.total ? " · " + rows.length + " loaded" : "")), dataTable(columns, rows, { sort: 1, dir: -1, pageSize: 50, onRow: openEvent, name: "foxx-logs", placeholder: "Search the loaded lines…" }), cursor ? h("div", { class: "pager" }, h("button", { class: "btn", type: "button", onclick: () => table.refresh() }, "Load 500 more")) : null);
  });
  table.controls.append(h("button", { class: "btn", type: "button", onclick: (event) => {
    const list = h("div", { class: "checks scroll" }, (data.attributes || []).filter((attribute) => attribute.path.includes(".")).map((attribute) => h("label", {}, h("input", { type: "checkbox", checked: extra.has(attribute.path) ? true : null, onchange: () => { if (extra.has(attribute.path)) extra.delete(attribute.path); else extra.add(attribute.path); try { sessionStorage.setItem("foxx-log-columns", JSON.stringify([...extra])); } catch (error) { /* storage unavailable */ } } }), attribute.path, h("em", {}, compact(attribute.present)))));
    menu(event.currentTarget, h("div", {}, h("strong", {}, "Add any attribute as a column"), list, h("button", { class: "btn primary", type: "button", onclick: () => { document.querySelectorAll(".menu").forEach((node) => node.remove()); rows.length = 0; cursor = 0; table.refresh(); } }, "Apply")));
  } }, "Attribute columns"));
  main.append(zone("log-table", "Log table", "Every matching event as a log line. The global filter (top) narrows what is loaded: any attribute, any operator, AND and OR. Column filters and search narrow the loaded lines.", table));
}

async function traces(main) {
  if (needRepository(main)) return;
  const state = { kind: "", outcome: "", min_ms: "", max_ms: "", min_spans: "" };
  let tablePanel = null;
  main.append(zone("trace-durations", "Durations", "How long traces took, from their first event to their last.",
    panel("Trace durations", { span: 12, info: { title: "Trace durations", text: "A histogram of trace durations: a tool call from its decision to its verification and credit settlement, a command, an alert. The median (p50) and 95th percentile (p95) are marked; click a bar to list those traces." } }, async (body, big) => {
      const data = await api("/api/histogram", filterParams({ field: "trace_duration_ms", bins: big ? 40 : 24 }));
      put(body, charts.histogram(data, { height: big ? 480 : 200, format: duration, onPick: (bin) => { state.min_ms = String(Math.floor(bin.from)); state.max_ms = String(Math.ceil(bin.to)); if (tablePanel) tablePanel.refresh(); scrollToZone("trace-table"); } }));
    })));
  tablePanel = panel("Trace table", { span: 12, flush: true, info: "page:traces" }, async (body) => {
    const data = await api("/api/traces", filterParams(Object.assign({ limit: 500 }, state)));
    const kinds = select("Kind", state.kind, [["", "Kind: all"]].concat((data.kinds || []).map((kind) => [kind, kind])), (value) => { state.kind = value; tablePanel.refresh(); });
    const numberInput = (key, placeholder) => h("input", { type: "number", min: "0", placeholder, value: state[key], "aria-label": placeholder, onchange: (event) => { state[key] = event.target.value; tablePanel.refresh(); } });
    const filters = h("div", { class: "table-tools" }, h("strong", {}, "Trace filters"), kinds, select("Outcome", state.outcome, [["", "Outcome: all"], ["OK", "OK"], ["FAILED", "FAILED"]], (value) => { state.outcome = value; tablePanel.refresh(); }),
      numberInput("min_ms", "min duration ms"), numberInput("max_ms", "max duration ms"), numberInput("min_spans", "min events"),
      h("button", { class: "btn link", type: "button", onclick: () => { Object.keys(state).forEach((key) => { state[key] = ""; }); tablePanel.refresh(); } }, "Reset"), h("span", { class: "count" }, compact(data.total) + " traces"));
    put(body, filters, dataTable([
      { label: "", render: (row) => h("span", { class: "sev", style: "background:" + color(row.severity, 0) }) },
      { label: "Start", get: (row) => row.start, render: (row) => h("span", { class: "mono" }, dt(row.start)), csv: (row) => dt(row.start) },
      { label: "Duration", get: (row) => row.duration_ms, render: (row) => duration(row.duration_ms), num: true },
      { label: "Kind", get: (row) => row.kind, filter: true },
      { label: "Trace", get: (row) => row.root, render: (row) => h("div", {}, h("code", {}, row.root), h("div", { class: "muted" }, row.events.join(" → "))) },
      { label: "Tool", get: (row) => row.tool, filter: true, info: "facet:tool" },
      { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true },
      { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
      { label: "Decision", get: (row) => row.decision, render: (row) => chip(row.decision), filter: true },
      { label: "Events", get: (row) => row.spans, num: true },
      { label: "Session", get: (row) => row.session, render: (row) => (row.session ? link(row.session.slice(0, 18), "#/sessions/" + encodeURIComponent(row.session)) : "—") },
      { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
      { label: "Flows", get: (row) => row.flows.join(" "), render: (row) => h("span", {}, row.flows.map((flow) => h("span", { class: "tag", title: meaning(flow) }, flow))) },
    ], data.traces, { sort: 1, dir: -1, pageSize: 40, onRow: (row) => openTrace(row.id), name: "foxx-traces", placeholder: "Search traces…" }));
  });
  main.append(zone("trace-table", "Trace table", "Every trace with at least one event matching the global filter; open one for its waterfall.", tablePanel));
}

async function topologyPage(main) {
  if (needRepository(main)) return;
  const layerPath = ["member_key", "agent.agent_type", "action.action_tool", "resource.resource"];
  main.append(zone("who", "Who used what", "Members → the agents they ran → the tools those agents called → the resources those tools touched. Edge width is how many events joined two nodes; red is the share denied.",
    panel("Topology", { span: 12, info: "page:topology" }, async (body) => {
      const data = await api("/api/topology", filterParams());
      put(body, charts.topology(data, { onPick: (node) => {
        const value = node.layer === 0 ? (members.find((row) => node.label.startsWith(row.member.actor + " (" + row.member.short)) || { member: { key: node.label } }).member.key : node.label;
        setClause(layerPath[node.layer], "is", [value]);
        render();
      } }));
    })));
  main.append(zone("dependencies", "Flow dependencies", "Within each session, which flow's event tends to follow which (order only, never a cause).",
    panel("Flow dependency graph", { span: 12, info: { title: "Flow dependency graph", text: "Each circle is a flow sized by its events; each arrow is how often an event of one flow was followed by an event of another in the same session. Hover a flow to follow its arrows; click it to filter by it." } }, async (body) => {
      const data = await api("/api/dependencies", filterParams());
      put(body, charts.dependency(data, { title_of: (flow) => (G.flows[flow] ? G.flows[flow][0] : ""), onPick: (flow) => { setClause("flow", "is", [flow]); render(); } }), dataTable([
        { label: "From", get: (row) => row.from, render: (row) => h("span", {}, row.from, info("flow:" + row.from)) }, { label: "To", get: (row) => row.to, render: (row) => h("span", {}, row.to, info("flow:" + row.to)) }, { label: "Times", get: (row) => row.count, num: true },
      ], data.edges, { sort: 2, pageSize: 10, noColumns: true, name: "foxx-flow-dependencies" }));
    })));
}

async function distributions(main) {
  if (needRepository(main)) return;
  const state = { field: "trace_duration_ms", bins: "24" };
  const labels = { trace_duration_ms: "Trace duration", session_gap_ms: "Gap between a session's events", events_per_session: "Events per session" };
  let ready = false;
  const hist = panel("Histogram", { span: 12, info: { title: "Histogram", text: "How a number is spread over the matching events: trace durations, the gap between a session's consecutive events, events per session, or any numeric eval attribute. The median (p50) and 95th percentile (p95) are marked. For an eval attribute, click a bar to filter by its range." } }, async (body, big) => {
    const data = await api("/api/histogram", filterParams({ field: state.field, bins: state.bins }));
    if (!ready) {
      ready = true;
      hist.controls.append(select("Field", state.field, (data.fields || []).map((field) => [field, labels[field] || field]), (value) => { state.field = value; hist.refresh(); }), select("Bins", state.bins, [["12", "12 bins"], ["24", "24 bins"], ["48", "48 bins"]], (value) => { state.bins = value; hist.refresh(); }));
    }
    const millis = /_ms$/.test(state.field);
    put(body, charts.histogram(data, { height: big ? 480 : 240, format: millis ? duration : compact, onPick: state.field.startsWith("eval.") ? (bin) => drill({ clauses: [{ path: state.field, op: "gte", values: [String(bin.from)] }, { path: state.field, op: "lt", values: [String(bin.to)] }] }) : null }));
  });
  main.append(zone("histogram", "Histogram", "The distribution of any number.", hist));
  main.append(zone("breakdowns", "Breakdowns", "How events divide by each facet.",
    breakdownPanel("By severity", { span: 4, by: "severity", info: "facet:severity" }),
    breakdownPanel("By origin", { span: 4, by: "origin", info: "facet:origin" }),
    breakdownPanel("By response", { span: 4, by: "response", info: "facet:response" }),
    breakdownPanel("Top tools", { span: 6, by: "tool", view: "toplist", info: "facet:tool" }),
    breakdownPanel("Top flows", { span: 6, by: "flow", view: "toplist", info: "facet:flow" })));
}

async function semantics(main) {
  if (needRepository(main)) return;
  const data = await api("/api/semantics", filterParams());
  const max = Math.max(1, ...data.words.map((word) => word.count));
  main.append(zone("words", "Words", "The words response statements use most, sized by frequency; click one to search for it.",
    panel("Statement words", { span: 12 }, async (body) => put(body, data.words.length ? h("div", { class: "cloud" }, data.words.map((word) => h("button", { type: "button", class: "word", style: "font-size:" + (11 + (word.count / max) * 18).toFixed(0) + "px", title: word.count + " times", onclick: () => { filter.q = word.value; saveFilter(); go("#/logs"); } }, word.value))) : h("div", { class: "empty" }, "No statements.")))));
  const group = (title, key, explainText) => panel(title, { span: 6, info: { title, text: explainText } }, async (body) => {
    const items = (data.groups[key] || []).map((item) => ({ value: item.value, count: item.count }));
    const path = ((data.groups[key] || [])[0] || {}).path;
    put(body, charts.toplist(items, { onPick: (value) => { if (path) drill({ clauses: [{ path, op: "is", values: [value] }], to: "#/logs" }); }, explain: explained, empty: "None in this range and filter." }));
  });
  main.append(zone("texts", "Statements and reasons", "What events said, and why decisions were made.",
    group("Response statements", "statements", "The short sentence Crane writes for each event (at most ten words)."),
    group("Decision reasons", "reasons", "Why a decision was made, in Crane's words."),
    group("Decision rules", "rules", "The rule that decided (for a denial, the first rule that refused)."),
    group("Failure states", "failure_states", "How a failed response failed."),
    group("Assessments", "assessments", "What the checks concluded."),
    group("Session end reasons", "end_reasons", "Why sessions ended.")));
  main.append(zone("lines", "Log lines", "The newest 300 matching events as text.",
    panel("Log lines", { span: 12, flush: true }, async (body) => put(body, h("div", { class: "loglines" }, data.lines.map((line) => h("div", { class: "logline", role: "button", tabindex: "0", onclick: () => openEventById(line.id), onkeydown: (event) => { if (event.key === "Enter") openEventById(line.id); } },
      h("span", { class: "mono muted" }, dt(line.t)), h("span", { class: "chip " + (TONES[line.severity] || "") }, line.severity || "—"), h("span", { class: "mono" }, line.flow), h("code", {}, line.name), h("span", {}, line.text || ""), line.reasons && line.reasons.length ? h("span", { class: "muted" }, "· " + line.reasons.join("; ")) : null)))))));
}

async function sessions(main) {
  if (needRepository(main)) return;
  const data = await api("/api/sessions");
  const rows = data.sessions;
  const active = rows.filter((row) => !row.stale && row.session.status === "ACTIVE").length;
  main.append(zone("session-kpis", "Summary", "Sessions are counted over all time; the charts below follow the range and filter.", h("div", { class: "kpis span-12" },
    kpi("Sessions", rows.length), kpi("Active", active, { tone: "good" }), kpi("Stale", rows.filter((row) => row.stale).length, { tone: rows.some((row) => row.stale) ? "bad" : "", info: "value:STALE" }),
    kpi("Quarantined", rows.filter((row) => row.session.safety === "QUARANTINED").length, { tone: rows.some((row) => row.session.safety === "QUARANTINED") ? "crit" : "", info: "value:QUARANTINED" }),
    kpi("Denied actions", rows.reduce((sum, row) => sum + ((row.evals || {}).denied || 0), 0), { info: "eval:denied", drill: () => drill({ clauses: [is("decision.decision", "DENY")] }) }))));
  main.append(zone("session-activity", "Activity", "", seriesPanel("Session activity", { span: 8, by: "agent", type: "area", height: 200 }), breakdownPanel("By agent type", { span: 4, by: "agent", info: "facet:agent" })));
  main.append(zone("session-list", "Sessions", "", panel("Sessions", { span: 12, flush: true }, async (body) => put(body, dataTable([
    { label: "Session", get: (row) => row.session.foxx_session_id, render: (row) => link(row.session.foxx_session_id, "#/sessions/" + encodeURIComponent(row.session.foxx_session_id)) },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
    { label: "Provider", get: (row) => row.session.provider, filter: true }, { label: "Model", get: (row) => row.session.model, filter: true },
    { label: "Status", get: (row) => (row.stale ? "STALE" : row.session.status), render: (row) => chip(row.stale ? "STALE" : row.session.status), filter: true },
    { label: "Safety", get: (row) => row.session.safety, render: (row) => chip(row.session.safety), filter: true }, { label: "Mode", get: (row) => row.session.autonomy_mode, filter: true },
    { label: "Denied", get: (row) => (row.evals || {}).denied, num: true, info: "eval:denied" }, { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true, info: "eval:bypass_attempts" },
    { label: "Credits", get: (row) => row.credits.available, render: (row) => row.credits.available + " / " + row.credits.granted, num: true, info: "eval:credits" },
    { label: "Started", get: (row) => row.session.started_at, render: (row) => h("span", { class: "mono" }, dt(row.session.started_at)) },
    { label: "Last seen", get: (row) => row.session.last_seen_at, render: (row) => h("span", { class: "mono" }, dt(row.session.last_seen_at)) },
  ], rows, { sort: 11, dir: -1, onRow: (row) => go("#/sessions/" + encodeURIComponent(row.session.foxx_session_id)), placeholder: "Search sessions, members, models…", name: "foxx-sessions" })))));
}
function graphView(graph) {
  const nodes = graph.nodes || [];
  if (!nodes.length) return h("div", { class: "empty" }, "No events in this session.");
  const verified = new Set((graph.edges || []).filter((edge) => edge.kind === "verified_by").map((edge) => edge.to));
  const tone = (node) => (node.flags.includes("bypass_confirmed") ? "#c2185b" : node.flags.includes("violation") ? "#ee6a4f" : node.flags.includes("denied") ? "#e5484d" : node.flags.includes("coverage_gap") || node.flags.includes("unresolved") ? "#f0a830" : node.kind === "verification" ? "#2fbf7c" : node.kind === "decision" ? "#7b5fc7" : "#9aa0b4");
  return h("div", {}, h("p", { class: "muted" }, "Order only, no inferred causality. A dotted link joins a decision to the verification of the same tool call. Click a step for its event."),
    h("div", { class: "flow" }, nodes.map((node) => h("div", { class: "node" + (verified.has(node.id) ? " linked" : ""), role: "button", tabindex: "0", onclick: () => openEvent(node.event), onkeydown: (event) => { if (event.key === "Enter") openEvent(node.event); } },
      h("span", { class: "muted mono" }, dt(node.event.occurred_at).slice(11)), h("span", { class: "dot" + (node.kind === "decision" || node.kind === "verification" ? "" : " small"), style: "background:" + tone(node) }),
      h("span", {}, h("code", {}, node.label), " ", h("span", { class: "tag", title: meaning(node.event.flow) }, node.event.flow), " ", node.event.decision && node.event.decision.decision !== "NOT_REQUIRED" ? chip(node.event.decision.decision) : null, " ", node.flags.map((flag) => chip(flag.toUpperCase())), " ", h("span", { class: "muted" }, (node.event.response || {}).response_statement || ""))))));
}
async function session(main, id) {
  if (needRepository(main)) return;
  const [data, graph] = await Promise.all([api("/api/sessions/" + encodeURIComponent(id)), api("/api/graph/" + encodeURIComponent(id))]);
  const sessionInfo = data.session, evals = data.evals || {};
  const only = [is("session.session_id", sessionInfo.foxx_session_id)];
  if (data.stale) main.append(h("div", { class: "notice warn" }, "This session stopped reporting: no hook activity for a long time and no signed end of session. The agent may have been stopped, its hooks removed, or its evidence is not reaching the service."));
  const download = (path, name) => async () => {
    const response = await fetch(scoped(path), { credentials: "same-origin" });
    const anchor = h("a", { href: URL.createObjectURL(await response.blob()), download: name });
    document.body.append(anchor); anchor.click(); anchor.remove();
  };
  main.append(hero("Session " + sessionInfo.foxx_session_id, [sessionInfo.provider, sessionInfo.model || "model not reported", data.member ? data.member.actor : "unknown member", "started " + dt(sessionInfo.started_at)].join(" · "),
    [["Safety", sessionInfo.safety, "value:" + sessionInfo.safety], ["Mode", sessionInfo.autonomy_mode], ["Status", data.stale ? "STALE" : sessionInfo.status], ["Credits", data.credits.available + " / " + data.credits.granted, "eval:credits"]],
    [h("button", { class: "btn", type: "button", onclick: download("/api/reports/session/" + sessionInfo.foxx_session_id, "crane-report-" + sessionInfo.foxx_session_id + ".json") }, "Report"),
      h("button", { class: "btn", type: "button", onclick: download("/api/evidence/session/" + sessionInfo.foxx_session_id, "crane-evidence-" + sessionInfo.foxx_session_id + ".json") }, "Evidence bundle"),
      h("button", { class: "btn", type: "button", onclick: () => drill({ clauses: only }) }, "Open in events"), h("button", { class: "btn", type: "button", onclick: () => go("#/credits") }, "Grant credits")]));
  main.append(h("div", { class: "kpis" },
    kpi("Events", evals.events, { info: "eval:events" }), kpi("Tool calls decided", evals.tool_calls_decided, { info: "eval:tool_calls_decided" }), kpi("Denied", evals.denied, { tone: evals.denied ? "bad" : "", info: "eval:denied" }), kpi("Bypass attempts", evals.bypass_attempts, { tone: evals.bypass_attempts ? "bad" : "", info: "eval:bypass_attempts" }),
    kpi("Confirmed bypasses", evals.bypasses_confirmed, { tone: evals.bypasses_confirmed ? "crit" : "good", info: "eval:bypasses_confirmed" }), kpi("Evidence completeness", evals.evidence_completeness, { format: percent, info: "eval:evidence_completeness" })));
  const ganttState = { color: "decision" };
  const sessionGantt = panel("Actions of this session", { span: 12, info: { title: "Actions of this session", text: "Every traced action of the session as a bar from its first to its last event, on a row per tool, colored by decision. Drag to zoom; click a bar for its trace." } }, async (body, big) => {
    const ganttData = await api("/api/gantt", filterParams({ rows: "tool", color: ganttState.color, clauses: only, ignoreTime: true }));
    put(body, h("div", { class: big ? "tall" : "" }, charts.gantt(ganttData, { onZoom: zoomTo, onPick: (bar) => openTrace(bar.id) })));
  });
  sessionGantt.controls.append(select("Color", ganttState.color, [["decision", "Color: decision"], ["severity", "Color: severity"], ["flow", "Color: flow"]], (value) => { ganttState.color = value; sessionGantt.refresh(); }));
  main.append(h("div", { class: "grid" }, sessionGantt, seriesPanel("Session timeline", { span: 8, by: "flow", clauses: only, params: { since: "", until: "" }, height: 200 }), breakdownPanel("Decisions in this session", { span: 4, by: "decision", clauses: only, params: { since: "", until: "" } })));
  main.append(h("div", { class: "grid" }, panel("Action graph", { span: 7 }, async (body) => put(body, graphView(graph))), panel("Evals", { span: 5, flush: true }, async (body) => put(body, evalsTable(evals)))));
  main.append(h("div", { class: "grid" }, panel("Tasks", { span: 12, flush: true }, async (body) => put(body, dataTable([
    { label: "Task", get: (row) => row.task.foxx_task_id, render: (row) => h("code", {}, row.task.foxx_task_id) }, { label: "External", get: (row) => row.task.external_task_id },
    { label: "Status", get: (row) => row.task.status, render: (row) => chip(row.task.status), filter: true }, { label: "Tool calls", get: (row) => row.task.tool_calls, num: true },
    { label: "Denied", get: (row) => (row.evals || {}).denied, num: true }, { label: "Started", get: (row) => row.task.started_at, render: (row) => h("span", { class: "mono" }, dt(row.task.started_at)) },
  ], data.tasks || [], { sort: 5, dir: 1, noColumns: true })))));
}

async function agents(main) {
  if (needRepository(main)) return;
  const data = await api("/api/agents");
  const rows = data.agents;
  main.append(zone("agent-charts", "Charts", "",
    panel("Denials by agent", { span: 6, info: "eval:denied" }, async (body) => put(body, charts.toplist(rows.map((row) => ({ value: row.agent_id, count: (row.evals || {}).denied || 0 })).filter((item) => item.count).sort((a, b) => b.count - a.count), { onPick: (value) => drill({ clauses: [is("agent.agent_id", value), is("decision.decision", "DENY")] }), empty: "No agent was denied." }))),
    seriesPanel("Events by agent type", { span: 6, by: "agent", type: "lines", info: "facet:agent" })));
  main.append(zone("agent-list", "Agents", "Every agent identity across all its sessions.", panel("Agents", { span: 12, flush: true }, async (body) => put(body, dataTable([
    { label: "Agent", get: (row) => row.agent_id, render: (row) => h("code", {}, row.agent_id) }, { label: "Provider", get: (row) => row.provider, filter: true },
    { label: "Sessions", get: (row) => row.sessions, num: true }, { label: "Quarantined", get: (row) => row.quarantined_sessions, num: true },
    { label: "Tool calls", get: (row) => (row.evals || {}).tool_calls_decided, num: true, info: "eval:tool_calls_decided" }, { label: "Denial share", get: (row) => (row.evals || {}).denial_share, render: (row) => percent((row.evals || {}).denial_share), num: true, info: "eval:denial_share" },
    { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true, info: "eval:bypass_attempts" }, { label: "Confirmed bypasses", get: (row) => (row.evals || {}).bypasses_confirmed, num: true, info: "eval:bypasses_confirmed" },
    { label: "Credits consumed", get: (row) => ((row.evals || {}).credits || {}).consumed, num: true }, { label: "Last seen", get: (row) => row.last_seen, render: (row) => fmt(row.last_seen) },
  ], rows, { sort: 2, placeholder: "Search agents…", name: "foxx-agents" })))));
}

const SYMBOLS = { above: ">", at_least: "≥", below: "<", at_most: "≤", equals: "=" };
async function alerting(main) {
  if (needRepository(main)) return;
  const data = await api("/api/alerting");
  const rules = data.rules;
  const firing = rules.filter((rule) => rule.state === "FIRING").length;
  const failed = data.deliveries.filter((delivery) => delivery.outcome === "FAILED").length;
  main.append(hero("Alerting", "Rules are built on a member's machine with 'crane alert add' and evaluated by every background sync; this page tracks them.",
    [["Rules", rules.length], ["Firing", firing, "value:FIRING"], ["Firings", data.firings.length], ["Deliveries", data.deliveries.length], ["Failed deliveries", failed, "value:FAILED"]]));
  if (!data.reported) main.append(h("div", { class: "notice" }, "No member has sent a status report yet, so rules are not visible. Run 'crane agent sync' on a machine with rules."));
  main.append(zone("rules", "Rules", "Every rule, its state, and its last value.", panel("Alert rules", { span: 12, flush: true, info: { title: "Alert rules", text: "A rule watches one metric (an eval) over a time window, counting only events that match its filter, and compares it with a threshold. When it crosses, it fires: its channels are notified, again after its cooldown while still firing, and once more when it resolves." } }, async (body) => put(body, dataTable([
    { label: "Rule", get: (row) => row.name, render: (row) => h("b", {}, row.name) },
    { label: "State", get: (row) => row.state || "NEW", render: (row) => chip(row.state || "NEW"), filter: true },
    { label: "Condition", get: (row) => row.description, render: (row) => h("span", {}, h("code", {}, row.metric), info("eval:" + row.metric), " " + (SYMBOLS[row.op] || row.op) + " " + row.threshold + " over " + row.window_minutes + " min", row.filter && row.filter.length ? h("div", { class: "muted" }, "counting " + row.filter.map((clause) => clauseText(clause)).join(" and ")) : null) },
    { label: "Last value", get: (row) => row.last_value, render: (row) => fmt(row.last_value), num: true },
    { label: "Since", get: (row) => row.since, render: (row) => h("span", { class: "mono" }, dt(row.since)) },
    { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
    { label: "Channels", get: (row) => (row.channels || []).join(", "), filter: true },
    { label: "Cooldown", get: (row) => row.cooldown_minutes, render: (row) => row.cooldown_minutes + " min", num: true },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
    { label: "Created", get: (row) => row.created_at, render: (row) => h("span", { class: "mono" }, dt(row.created_at)) },
  ], rules, { sort: 1, dir: 1, empty: "No alert rules yet. Build one on a member's machine (see Build a rule below).", name: "foxx-alert-rules" })))));
  const ruleNames = [...new Set(data.evaluations.map((evaluation) => evaluation.rule))];
  const chosen = { rule: ruleNames[0] || "" };
  const valuePanel = panel("Rule value against its threshold", { span: 12, info: { title: "Rule value against its threshold", text: "Each evaluation of the rule (by every background sync and 'crane alert check'), the value it measured, and its threshold (dashed). Big red dots are firings and reminders; green dots are resolutions." } }, async (body, big) => {
    const rule = rules.find((item) => item.name === chosen.rule) || {};
    const points = data.evaluations.filter((evaluation) => evaluation.rule === chosen.rule);
    put(body, chosen.rule ? charts.threshold(points, { threshold: Number(rule.threshold !== undefined ? rule.threshold : (points[0] || {}).threshold || 0), symbol: SYMBOLS[rule.op || (points[0] || {}).op] || "", height: big ? 460 : 200 }) : h("div", { class: "empty" }, "No rule has been evaluated yet."));
  });
  if (ruleNames.length) valuePanel.controls.append(select("Rule", chosen.rule, ruleNames.map((name) => [name, name]), (value) => { chosen.rule = value; valuePanel.refresh(); }));
  main.append(zone("rule-values", "Rule values", "", valuePanel));
  main.append(zone("firings", "Firings", "Every notification a rule sent: firing, still firing (reminder), resolved, or test.", panel("Firings", { span: 12, flush: true }, async (body) => put(body, dataTable([
    { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at)), csv: (row) => dt(row.at) },
    { label: "Rule", get: (row) => row.rule, filter: true },
    { label: "State", get: (row) => (row.reminder ? "REMINDER" : row.state), render: (row) => chip(row.reminder ? "REMINDER" : row.state), filter: true },
    { label: "Value", get: (row) => row.value, render: (row) => fmt(row.value) + " (threshold " + fmt(row.threshold) + ")" },
    { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
    { label: "Deliveries", get: (row) => (row.deliveries || []).map((delivery) => delivery.channel + " " + delivery.result).join("; ") },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) },
  ], data.firings, { sort: 0, dir: -1, empty: "No rule has fired yet.", name: "foxx-alert-firings" })))));
  const outcomes = {};
  for (const delivery of data.deliveries) outcomes[delivery.outcome] = (outcomes[delivery.outcome] || 0) + 1;
  main.append(zone("deliveries", "Deliveries", "Every message sent to Slack or WhatsApp, with the channel's answer.",
    panel("Delivery outcomes", { span: 4 }, async (body) => put(body, charts.donut(Object.entries(outcomes).map(([value, count]) => ({ value, count })), { unit: "deliveries", explain: explained, hint: "" }))),
    panel("Delivery log", { span: 8, flush: true }, async (body) => put(body, dataTable([
      { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at)), csv: (row) => dt(row.at) },
      { label: "Rule", get: (row) => row.rule, filter: true }, { label: "Channel", get: (row) => row.channel, filter: true },
      { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true }, { label: "HTTP", get: (row) => row.status, num: true }, { label: "Took", get: (row) => row.ms, render: (row) => (row.ms === null || row.ms === undefined ? "—" : duration(row.ms)), num: true },
      { label: "Answer", get: (row) => row.result },
    ], data.deliveries, { sort: 0, dir: -1, empty: "Nothing delivered yet.", name: "foxx-alert-deliveries" }))),
    panel("Channels", { span: 12, flush: true, info: { title: "Channels", text: "Whether each machine has Slack or WhatsApp configured ('crane integrate slack|whatsapp'). A rule's notifications fail on a machine without its channel." } }, async (body) => put(body, dataTable([
      { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) }, { label: "Channel", get: (row) => row.channel }, { label: "Configured", get: (row) => (row.configured ? "yes" : "no"), render: (row) => chip(row.configured ? "OK" : "PENDING") },
    ], data.channels, { noTools: true })))));
  main.append(zone("alert-events", "Alert events", "The alert events of the stream (F14): raised, suppressed, delivered, failed, rule firings and resolutions.",
    seriesPanel("Alert events over time", { span: 8, by: "event", clauses: [is("flow", "F14")], info: "flow:F14" }),
    breakdownPanel("Alert events", { span: 4, by: "event", view: "toplist", clauses: [is("flow", "F14")] })));
  main.append(zone("hook-alerts", "Hook alerts", "Alerts the hooks raise on their own (bypass attempts, quarantine, coverage gaps, low credits, suspicious prompts) and stale sessions.",
    panel("Hook alerts", { span: 12, flush: true, info: "flow:F14" }, async (body) => {
      const hooks = await api("/api/alerts");
      put(body, dataTable([
        { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at * 1000)) }, { label: "Alert", get: (row) => row.title }, { label: "Kind", get: (row) => row.kind, filter: true },
        { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
        { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
        { label: "Session", get: (row) => row.session, render: (row) => (row.session ? link(String(row.session).slice(0, 18), "#/sessions/" + encodeURIComponent(row.session)) : "—") },
        { label: "Delivery", get: (row) => (row.computed ? "derived by the dashboard" : row.suppressed ? "suppressed (cooldown)" : (row.deliveries || []).map((delivery) => delivery.channel + ": " + delivery.result).join(", ")) },
      ], hooks.alerts.filter((alert) => alert.kind !== "rule"), { sort: 0, dir: -1, name: "foxx-hook-alerts" }));
    })));
  const example = (text) => h("div", { class: "code-line" }, h("code", {}, text), h("button", { class: "btn small", type: "button", onclick: (event) => { const button = event.currentTarget; navigator.clipboard.writeText(text).then(() => { button.textContent = "Copied"; }).catch(() => {}); } }, "Copy"));
  main.append(zone("build", "Build a rule", "Rules are built from the command line on a member's machine, by a person (agents are refused).", panel("Build a rule with the CLI", { span: 12 }, async (body) => put(body,
    h("p", {}, "1. Configure a channel once on the machine: ", h("code", {}, "crane integrate slack"), " or ", h("code", {}, "crane integrate whatsapp"), "."),
    h("p", {}, "2. Add a rule: a metric, a comparison, a threshold, a window, channels, and optional conditions on the events counted (any attribute, any operator):"),
    example("crane alert add too-many-denials metric denied at_least 5 window 15m channels slack,whatsapp severity high"),
    example("crane alert add bypass-watch metric bypass_attempts above 0 window 1h channels whatsapp severity critical cooldown 60"),
    example("crane alert add slow-hooks metric latency_ms.p95 above 800 window 30m channels slack where flow is F05"),
    example("crane alert add risky-bash metric events at_least 3 window 10m channels slack where tool is BASH where decision is DENY,QUARANTINE"),
    h("p", {}, "3. Check it now, send a test message, and read its log: ", h("code", {}, "crane alert check"), ", ", h("code", {}, "crane alert test NAME"), ", ", h("code", {}, "crane alert log"), ". ", h("code", {}, "crane alert metrics"), " lists every metric."),
    h("p", { class: "muted" }, "Every background sync evaluates the rules; firings and deliveries are uploaded with it and appear here within about a minute.")))));
}

async function credits(main) {
  if (needRepository(main)) return;
  const data = await api("/api/grants");
  const active = data.sessions.filter((row) => row.active);
  main.append(hero("Credits", "Autonomy credits bound what an agent may do on its own. Admins and owners can grant more to a session; GitHub confirms the role.",
    [["Sessions", data.sessions.length], ["Active", active.length], ["Available (all)", compact(data.sessions.reduce((sum, row) => sum + row.balance.available, 0)), "eval:credits"], ["Grants pending", (data.by_status || {}).PENDING || 0, "value:PENDING"], ["Grants applied", (data.by_status || {}).APPLIED || 0, "value:APPLIED"]]));
  main.append(zone("credit-summary", "Summary", "Credit events over time (F27).", seriesPanel("Credit events", { span: 8, by: "event", clauses: [is("flow", "F27")], info: "flow:F27" }), breakdownPanel("Credit events", { span: 4, by: "event", view: "toplist", clauses: [is("flow", "F27")] })));
  const grantForm = (row) => {
    const amount = h("input", { type: "number", min: "1", max: "1000", value: "25", "aria-label": "Credits", required: true });
    const reason = h("input", { type: "text", maxlength: "200", placeholder: "Why (shown in the ledger)", "aria-label": "Reason", required: true });
    const error = h("div", { class: "error-text", role: "alert" });
    details.open("Grant credits", h("form", { class: "form", onsubmit: (event) => {
      event.preventDefault();
      const value = Number(amount.value);
      if (!(value >= 1 && value <= 1000)) { error.textContent = "Grant 1 to 1000 credits."; return; }
      if (!reason.value.trim()) { error.textContent = "Say why; it is recorded in the session's ledger."; return; }
      location.href = "/auth/grant?repository=" + encodeURIComponent(scope.repository) + "&session=" + encodeURIComponent(row.session) + "&amount=" + value + "&reason=" + encodeURIComponent(reason.value.trim());
    } },
      h("div", { class: "kv" }, [["Session", row.session], ["Agent", row.agent], ["Member", member(row.member)], ["Available now", row.balance.available + " of " + row.balance.granted + " granted"]].map(([key, value]) => [h("div", {}, key), h("div", {}, value instanceof Node ? value : fmt(value))])),
      h("label", {}, "Credits to grant", amount), h("label", {}, "Reason", reason), error,
      h("p", { class: "muted" }, "You will sign in with GitHub. Only an admin or owner of the repository can grant; anyone else is refused and nothing is recorded. The session's machine adds the credits to its own ledger at its next sync (within about a minute while the agent works) and reports back here."),
      h("button", { class: "btn primary", type: "submit" }, "Continue with GitHub")));
    amount.focus();
  };
  main.append(zone("balances", "Balances", "Each session's credits from its ledger.", panel("Session balances", { span: 12, flush: true, info: "eval:credits" }, async (body) => put(body, dataTable([
    { label: "Session", get: (row) => row.session, render: (row) => link(row.session, "#/sessions/" + encodeURIComponent(row.session)) },
    { label: "Agent", get: (row) => row.agent, filter: true }, { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
    { label: "Active", get: (row) => (row.active ? "yes" : "no"), render: (row) => chip(row.active ? "ACTIVE" : "ENDED"), filter: true },
    { label: "Available", get: (row) => row.balance.available, num: true }, { label: "Granted", get: (row) => row.balance.granted, num: true }, { label: "Consumed", get: (row) => row.balance.consumed, num: true },
    { label: "Reserved", get: (row) => row.balance.reserved, num: true }, { label: "Revoked", get: (row) => row.balance.revoked, num: true },
    { label: "Last seen", get: (row) => row.last_seen_at, render: (row) => h("span", { class: "mono" }, dt(row.last_seen_at)) },
    { label: "", render: (row) => h("button", { class: "btn primary small", type: "button", onclick: () => grantForm(row) }, "Grant credits") },
  ], data.sessions, { sort: 3, dir: -1, name: "foxx-credit-balances", empty: "No sessions yet." })))));
  main.append(zone("grants", "Grants", "Every grant approved on the dashboard and what the machine did with it.", panel("Grants", { span: 12, flush: true }, async (body) => put(body, dataTable([
    { label: "Approved", get: (row) => row.approved_at, render: (row) => h("span", { class: "mono" }, dt(row.approved_at)), csv: (row) => dt(row.approved_at) },
    { label: "By", get: (row) => "@" + row.approved_by, filter: true }, { label: "Session", get: (row) => row.session }, { label: "Credits", get: (row) => row.amount, num: true },
    { label: "Reason", get: (row) => row.reason }, { label: "Status", get: (row) => row.status, render: (row) => chip(row.status), filter: true },
    { label: "Settled", get: (row) => row.settled_at, render: (row) => h("span", { class: "mono" }, dt(row.settled_at)) }, { label: "Machine said", get: (row) => row.detail },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) },
  ], data.grants, { sort: 0, dir: -1, empty: "No credits have been granted on the dashboard.", name: "foxx-credit-grants" })))));
}

async function audit(main) {
  if (needRepository(main)) return;
  let governance = null;
  try { governance = await api("/api/governance"); } catch (error) { if (error.status !== 404) throw error; }
  const trail = governance ? governance.audit || [] : [];
  const when = (entry) => entry.at_ms || Date.parse(entry.at);
  const actors = [...new Set(trail.map((entry) => entry.actor))];
  main.append(hero("Audit trail", "The signed governance history and every Crane command run in the repository.", [["Governance changes", trail.length], ["Actors", actors.length], ["Newest change", trail[0] ? dt(when(trail[0])) : "—"]]));
  main.append(zone("audit-activity", "Activity", "Governance changes per day by actor, and commands over time.",
    panel("Governance changes per day", { span: 6, info: { title: "Governance changes per day", text: "Each signed change to the governance (init, protect, target, checkpoint, sign, …) per day, by who made it." } }, async (body, big) => {
      const byDay = new Map();
      for (const entry of trail) { const day = Math.floor(when(entry) / 86400000) * 86400000; if (!byDay.has(day)) byDay.set(day, {}); byDay.get(day)[entry.actor] = (byDay.get(day)[entry.actor] || 0) + 1; }
      const days = [...byDay.keys()].sort((a, b) => a - b);
      if (!days.length) { put(body, h("div", { class: "empty" }, "No governance changes reported.")); return; }
      const start = days[0], buckets = Math.round((days[days.length - 1] - start) / 86400000) + 1;
      const series = actors.map((actor) => ({ key: actor, label: actor, total: trail.filter((entry) => entry.actor === actor).length, points: Array.from({ length: buckets }, (_, index) => (byDay.get(start + index * 86400000) || {})[actor] || 0) }));
      put(body, charts.timeseries({ start, bucket_ms: 86400000, buckets, series, total: trail.length }, { type: "bars", height: big ? 480 : 200 }));
    }),
    seriesPanel("Crane commands over time", { span: 6, by: "event", clauses: [is("flow", "F13")], info: "flow:F13" })));
  main.append(zone("audit-trail", "Governance changes", "Every signed change, newest first; open one for its detail.", panel("Governance audit trail", { span: 12, flush: true, info: "flow:F13" }, async (body) => put(body, governance ? dataTable([
    { label: "#", get: (row) => row.seq, num: true },
    { label: "Time", get: when, render: (row) => h("span", { class: "mono" }, dt(when(row))), csv: (row) => dt(when(row)) },
    { label: "Actor", get: (row) => row.actor, filter: true }, { label: "Action", get: (row) => row.action, render: (row) => h("code", {}, row.action), filter: true },
    { label: "Detail", get: (row) => JSON.stringify(row.detail), render: (row) => h("span", { class: "muted clip" }, fmt(row.detail)) },
  ], trail, { sort: 0, dir: -1, placeholder: "Search actors, actions, details…", empty: "No governance changes recorded.", name: "foxx-governance-audit", onRow: (row) => details.open("Governance change #" + row.seq, h("div", {}, h("div", { class: "kv" }, [["Time", dt(when(row))], ["Actor", row.actor], ["Action", row.action]].map(([key, value]) => [h("div", {}, key), h("div", {}, value)])), h("h4", {}, "Detail"), h("pre", { class: "json" }, JSON.stringify(row.detail, null, 2)))) })
    : h("div", { class: "notice" }, "No member has reported the repository's governance yet; it is sent with the next upload.")))));
  main.append(zone("command-events", "Command events", "Every Crane command (F13) in range and filter.", panel("Command events", { span: 12, flush: true, info: "flow:F13" }, async (body) => {
    const commands = await api("/api/events", filterParams({ clauses: [is("flow", "F13")], limit: 500 }));
    put(body, eventsTable(commands.events, { pageSize: 25, name: "foxx-command-events" }));
  })));
}

async function governanceView(main) {
  if (needRepository(main)) return;
  let data;
  try { data = await api("/api/governance"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
  const passing = data.policies.filter((policy) => policy.passed).length;
  main.append(hero("Governance", ["As reported by", data.reported_by ? data.reported_by.actor : "a member", "at", dt(data.reported_at), "· results describe that machine's working tree"].join(" "), [["Generation", data.generation], ["Policies passing", passing + " / " + data.policies.length], ["Autonomy", data.autonomy.mode]]));
  main.append(zone("gov-state", "State", "", h("div", { class: "kpis span-12" }, kpi("Selections", data.selections.length), kpi("Unresolved selections", data.selections.filter((row) => !row.resolved).length, { tone: data.selections.some((row) => !row.resolved) ? "bad" : "good", info: "value:UNRESOLVED" }),
    kpi("Policies failing", data.policies.length - passing, { tone: passing < data.policies.length ? "bad" : "good" }), kpi("Checkpoints", data.checkpoints.length), kpi("Findings", data.findings.length, { tone: data.findings.length ? "bad" : "good" }),
    kpi("Admins", data.admins ? data.admins.admins.length + 1 : 1, { note: "owner included" }), kpi("Credits per session", data.autonomy.session_credits, { info: "eval:credits" }))));
  main.append(zone("gov-rules", "Policies and selections", "",
    panel("Policies", { span: 6, flush: true }, async (body) => put(body, dataTable([{ label: "Policy", get: (row) => row.name }, { label: "File", get: (row) => row.file, render: (row) => h("code", {}, row.file) }, { label: "Result", get: (row) => (row.passed ? "PASS" : "FAIL"), render: (row) => chip(row.passed ? "PASS" : "FAIL"), filter: true },
      { label: "Commands", get: (row) => row.commands.length, render: (row) => h("div", {}, row.commands.map((command) => h("div", {}, chip(command.outcome), " ", h("code", {}, command.operation + " " + command.id), " ", h("span", { class: "muted" }, command.message)))) }], data.policies, { sort: 0, dir: 1, noColumns: true }))),
    panel("Selections", { span: 6, flush: true }, async (body) => put(body, dataTable([{ label: "Id", get: (row) => row.id, render: (row) => h("code", {}, row.id) }, { label: "Operation", get: (row) => row.operation, filter: true }, { label: "Policy", get: (row) => row.policy, filter: true },
      { label: "Resolved at", get: (row) => (row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : "UNRESOLVED"), render: (row) => (row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : chip("UNRESOLVED")) },
      { label: "Language", get: (row) => row.language, filter: true }, { label: "Gen", get: (row) => row.generation, num: true }], data.selections, { sort: 0, dir: 1, noColumns: true })))));
  main.append(zone("gov-more", "Checkpoints and context", "",
    panel("Checkpoints", { span: 6, flush: true }, async (body) => put(body, dataTable([{ label: "Name", get: (row) => row.name }, { label: "Commit", get: (row) => row.commit, render: (row) => h("code", {}, row.commit.slice(0, 12)) }, { label: "Branch", get: (row) => row.branch, filter: true }, { label: "Created", get: (row) => row.created_at }, { label: "By", get: (row) => row.created_by, filter: true }], data.checkpoints, { sort: 3, noColumns: true }))),
    panel("Policy context (guidance, never authorization)", { span: 6, flush: true }, async (body) => put(body, dataTable([{ label: "Policy", get: (row) => row.policy, filter: true }, { label: "File", get: (row) => row.path, render: (row) => h("code", {}, row.path) }, { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true }], data.contexts, { noColumns: true })))));
  main.append(zone("gov-findings", "Findings", "", panel("Findings", { span: 12, flush: true }, async (body) => put(body, dataTable([{ label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "Code", get: (row) => row.code, render: (row) => h("code", {}, row.code), filter: true }, { label: "Finding", get: (row) => row.message }], data.findings, { empty: "No findings.", noColumns: true })))));
}

async function compliance(main) {
  if (needRepository(main)) return;
  const data = await api("/api/compliance");
  const outcomes = Object.entries(data.outcomes || {}).map(([value, count]) => ({ value, count }));
  const state = { outcome: "", q: "" };
  let controlsPanel = null;
  const focus = (outcome) => { state.outcome = outcome; if (controlsPanel) controlsPanel.refresh(); scrollToZone("controls"); };
  main.append(h("div", { class: "notice" }, data.disclaimer, " Evaluated at ", dt(data.evaluated_at), data.inputs_reported ? "." : ". No machine has reported its local checks yet, so some controls show no evidence."));
  main.append(zone("outcomes", "Outcomes", "How the controls stand. Gaps are fixable; limitations are inherent to the method and never lower an outcome.",
    panel("Control outcomes", { span: 5, info: { title: "Control outcomes", text: "SATISFIED: positive evidence. PARTIAL: some evidence, with fixable gaps. NOT_SATISFIED: evidence of failure. NO_EVIDENCE: nothing exercised it yet (not a failure). NOT_APPLICABLE: nothing for it to apply to yet." } }, async (body) => put(body, charts.donut(outcomes, { unit: "controls", explain: explained, onPick: focus, hint: "Click: show these controls" }))),
    h("div", { class: "kpis span-7" }, ["SATISFIED", "PARTIAL", "NOT_SATISFIED", "NO_EVIDENCE", "NOT_APPLICABLE"].map((outcome) => kpi(outcome.replace(/_/g, " ").toLowerCase(), (data.outcomes || {})[outcome] || 0, { info: "value:" + outcome, tone: outcome === "SATISFIED" ? "good" : outcome === "NOT_SATISFIED" ? "bad" : "", drill: () => focus(outcome) })))));
  controlsPanel = panel("Controls", { span: 12 }, async (body) => {
    const list = data.controls.filter((control) => (!state.outcome || control.outcome === state.outcome) && (!state.q || JSON.stringify(control).toLowerCase().includes(state.q.toLowerCase())));
    put(body, h("div", { class: "table-tools" }, h("input", { type: "search", placeholder: "Search controls, evidence, frameworks…", value: state.q, "aria-label": "Search controls", oninput: (event) => { state.q = event.target.value; clearTimeout(controlsPanel.typing); controlsPanel.typing = setTimeout(() => { controlsPanel.refresh(); }, 400); } }),
      select("Outcome", state.outcome, [["", "Outcome: all"]].concat(["SATISFIED", "PARTIAL", "NOT_SATISFIED", "NO_EVIDENCE", "NOT_APPLICABLE"].map((outcome) => [outcome, outcome])), (value) => { state.outcome = value; controlsPanel.refresh(); }), h("span", { class: "count" }, list.length + " of " + data.controls.length + " controls")),
    h("div", { class: "controls-grid" }, list.length ? list.map((control) => h("article", { class: "control " + (TONES[control.outcome] || "") },
      h("header", {}, h("code", {}, control.control.id), chip(control.outcome), info("value:" + control.outcome)), h("h4", {}, control.control.objective),
      h("dl", {}, h("dt", {}, "Enforced by"), h("dd", {}, control.control.rule), h("dt", {}, "Verified by"), h("dd", {}, control.control.method)),
      control.evidence.length ? h("div", { class: "evidence" }, h("b", {}, "Evidence"), h("ul", {}, control.evidence.map((item) => h("li", {}, item)))) : null,
      control.gaps.length ? h("div", { class: "gaps" }, h("b", {}, "Gaps to fix"), h("ul", {}, control.gaps.map((item) => h("li", {}, item)))) : null,
      control.limitations && control.limitations.length ? h("div", { class: "limits" }, h("b", {}, "Limitations ", info("", { title: "Limitations", text: "What the method cannot prove. They are disclosed, not counted against the control." })), h("ul", {}, control.limitations.map((item) => h("li", {}, item)))) : null,
      h("div", { class: "fix" }, h("b", {}, "How to satisfy: "), control.control.fix),
      h("div", { class: "flows" }, h("b", {}, "Evidence flows: "), control.control.flows.map((flow) => h("button", { type: "button", class: "tag link", title: meaning(flow) + " Click: open these events.", onclick: () => drill({ clauses: [is("flow", flow)] }) }, flow))),
      h("div", { class: "maps" }, control.control.mappings.map((mapping) => h("span", { class: "chip" }, mapping))))) : h("div", { class: "empty" }, "No control matches.")));
  });
  main.append(zone("controls", "Controls", "Each control with its evidence, gaps, limitations, how to satisfy it, and the flows its evidence comes from.", controlsPanel));
  main.append(zone("inferred", "Inferred observability", "What hooks can only infer is shown as observability, not as a control.", (data.inferred || []).map((item) => panel("Network destinations", { span: 12, info: { title: "Network destinations", text: item.meaning } }, async (body) => put(body, h("p", { class: "muted" }, item.meaning), charts.toplist(item.values.map((entry) => ({ value: entry.value, count: entry.count })), { onPick: (value) => drill({ clauses: [is("event_name", "NETWORK_DESTINATION_INFERRED"), is("resource.resource_ids", value)] }), empty: "No destinations inferred in the recorded events." }))))));
  main.append(zone("unobservable", "Not observable", "Telemetry hooks cannot collect; no control relies on it.", panel("Not observable from hooks", { span: 12 }, async (body) => put(body, h("div", {}, data.unsupported_telemetry.map((name) => chip(name))), h("p", { class: "muted" }, "Hooks see each tool call before and after it runs, not the process underneath: bytes sent, CPU time, and model tokens are outside their view.")))));
}

async function repositories(main) {
  if (!scope.account) { main.append(h("div", { class: "notice" }, "No account selected.")); return; }
  const data = await api("/api/accounts/" + encodeURIComponent(scope.account));
  const choose = (row) => { scope.repository = row.repository; scope.member = ""; saveScope(); go("#/overview"); };
  main.append(hero(scope.account, data.repositories.length + " repositories with Crane", [["Events", compact(data.repositories.reduce((sum, row) => sum + row.events, 0))], ["Denied", compact(data.repositories.reduce((sum, row) => sum + row.denied, 0)), "eval:denied"], ["Alerts", compact(data.repositories.reduce((sum, row) => sum + row.alerts, 0))]]));
  const byName = (value) => data.repositories.find((item) => item.remote.endsWith(value));
  main.append(h("div", { class: "grid" }, panel("Events by repository", { span: 6 }, async (body) => put(body, charts.toplist(data.repositories.map((row) => ({ value: row.remote.replace(/^github\.com\//, ""), count: row.events })).sort((a, b) => b.count - a.count), { onPick: (value) => { const row = byName(value); if (row) choose(row); }, hint: "Open this repository" }))),
    panel("Denials by repository", { span: 6 }, async (body) => put(body, charts.toplist(data.repositories.filter((row) => row.denied).map((row) => ({ value: row.remote.replace(/^github\.com\//, ""), count: row.denied })).sort((a, b) => b.count - a.count), { onPick: (value) => { const row = byName(value); if (row) choose(row); }, empty: "No denials.", hint: "Open this repository" })))));
  main.append(h("div", { class: "grid" }, panel("Repositories", { span: 12, flush: true }, async (body) => put(body, dataTable([
    { label: "Repository", get: (row) => row.remote, render: (row) => h("a", { href: "#/overview", onclick: (event) => { event.preventDefault(); choose(row); } }, row.remote.replace(/^github\.com\//, "")) },
    { label: "Members", get: (row) => row.members, num: true }, { label: "Sessions", get: (row) => row.sessions, num: true }, { label: "Events", get: (row) => row.events, num: true },
    { label: "Denied", get: (row) => row.denied, num: true }, { label: "Confirmed bypasses", get: (row) => row.bypasses_confirmed, render: (row) => (row.bypasses_confirmed ? chip("BYPASS_CONFIRMED", row.bypasses_confirmed + " confirmed") : "0") },
    { label: "Alerts", get: (row) => row.alerts, num: true }, { label: "Last activity", get: (row) => row.last_activity, render: (row) => h("span", { class: "mono" }, dt(row.last_activity)) },
  ], data.repositories, { sort: 7, onRow: choose, placeholder: "Search repositories…", empty: "A repository appears once a member's Crane uploads from a clone of a GitHub repository owned by this account.", name: "foxx-repositories" })))));
}

async function settings(main) {
  if (needRepository(main)) return;
  const [health, repo, integrations, memberData] = await Promise.all([api("/api/health"), api("/api/repo"), api("/api/integrations"), api("/api/members")]);
  const input = h("input", { type: "text", placeholder: "12-character code", "aria-label": "approval code", maxlength: "12" });
  const approve = (action) => () => { const code = input.value.trim().toUpperCase(); if (code) location.href = "/auth/approve?action=" + action + "&code=" + encodeURIComponent(code); };
  main.append(zone("approvals", "Approvals", "",
    panel("Approvals", { span: 6, info: { title: "Approvals", text: "'crane agent link' prints a code that links a machine's upload key to your GitHub account (you need access to the repository); 'crane admin register' prints one that confirms the owner key (you must administer the repository). GitHub checks your role once; Foxx keeps no GitHub token." } }, async (body) => put(body, h("div", { class: "table-tools plain" }, input, h("button", { class: "btn primary", type: "button", onclick: approve("link") }, "Link machine"), h("button", { class: "btn", type: "button", onclick: approve("owner") }, "Confirm owner key")))),
    panel("Repository connection", { span: 6, info: "flow:F21" }, async (body) => put(body, repo.connection ? h("p", {}, repo.connection.repository + " via " + repo.connection.method, h("br"), h("span", { class: "muted" }, "privileges: " + fmt(repo.connection.privileges))) : h("p", { class: "muted" }, repo.remote + " (connection not reported yet)"),
      repo.owner && repo.owner.owner_key ? h("p", {}, "Owner key ", h("code", {}, repo.owner.owner_key.slice(0, 16) + "…"), " confirmed by @" + repo.owner.confirmed_by + " (GitHub admin) at " + dt(repo.owner.confirmed_at)) : h("p", { class: "muted" }, "No GitHub-confirmed owner key yet: the owner runs 'crane admin register' and confirms it here.")))));
  const hidden = memberData.members.filter((row) => !row.selected && !row.linked_by).length;
  main.append(zone("members", "Members", hidden && !scope.member ? hidden + " machine(s) nobody linked to a GitHub account are hidden; choose one in Member to see its activity." : "", panel("Members", { span: 12, flush: true, info: "flow:F02" }, async (body) => put(body, dataTable([
    { label: "Member", get: (row) => row.member.actor, render: (row) => member(row.member) },
    { label: "Linked", get: (row) => (row.linked_by ? "LINKED" : "UNLINKED"), render: (row) => (row.linked_by ? chip("LINKED", "linked by @" + row.linked_by) : chip("UNLINKED", "run 'crane agent link' on that machine")), filter: true },
    { label: "GitHub", get: (row) => (row.linked_by ? "@" + row.linked_by : "") }, { label: "Shown", get: (row) => (row.selected ? "yes" : "no"), filter: true },
    { label: "Remote", get: (row) => (row.verified ? "verified" : "missing"), render: (row) => chip(row.verified ? "verified" : "missing") },
    { label: "Events", get: (row) => row.events, num: true }, { label: "Sessions", get: (row) => row.sessions, num: true },
    { label: "Last upload", get: (row) => row.last_upload, render: (row) => h("span", { class: "mono" }, dt(row.last_upload)) }, { label: "Crane", get: (row) => row.crane_version, filter: true },
  ], memberData.members, { sort: 5, name: "foxx-members" })))));
  main.append(zone("health-zone", "System health", "", h("div", { class: "kpis span-12" },
    kpi("Crane", health.reported_crane_version || health.crane_version, { format: String, note: "schema v" + health.schema_version + ", events v" + health.spec_version }),
    kpi("Registry generation", health.registry_generation, { format: fmt }), kpi("Events", health.events, { note: "chains " + health.event_chain.status + (health.truncated ? " (most recent shown)" : "") }),
    kpi("Specification events", health.spec_events), kpi("Ledger", health.ledger_entries, { note: health.ledger_reconciles ? "reconciles" : "DOES NOT reconcile", tone: health.ledger_reconciles ? "" : "bad" }), kpi("Mode", health.mode, { format: String })),
    panel("Event chains by member", { span: 12, flush: true, info: "flow:F09" }, async (body) => put(body, dataTable([{ label: "Member", render: (row) => member(row.member) }, { label: "Chain", get: (row) => row.event_chain.status, render: (row) => chip(row.event_chain.status), filter: true }, { label: "Events", get: (row) => row.event_chain.length, num: true }, { label: "Ledger", get: (row) => (row.ledger_reconciles ? "reconciles" : "FAIL"), render: (row) => (row.ledger_reconciles ? "reconciles" : chip("FAIL")) }], health.event_chain.members, { noTools: true })))));
  main.append(zone("integrations", "Integrations", "Non-secret settings; secrets stay on the member's machine.", panel("Integrations", { span: 12, flush: true, info: "flow:F22" }, async (body) => put(body, dataTable([{ label: "Integration", get: (row) => row.name }, { label: "Member", render: (row) => member(row.member) }, { label: "Settings", get: (row) => fmt(row.settings) }, { label: "Secrets", get: (row) => row.secret_fields.length, render: (row) => row.secret_fields.length + " on the member's machine" }, { label: "Configured", get: (row) => row.configured_at, render: (row) => h("span", { class: "mono" }, dt(row.configured_at * 1000)) }], integrations.integrations, { noTools: true, empty: "No integrations configured." })))));
  const signIns = panel("Your sign-ins", { span: 12, flush: true, info: "flow:F00" }, async (body) => put(body, h("div", { class: "pager left" }, h("button", { class: "btn", type: "button", onclick: async () => { const data = await api("/api/me/sign-ins"); put(signIns.body, eventsTable(data.events, { pageSize: 10 })); } }, "Show my recent sign-ins"))));
  main.append(zone("signins", "Your sign-ins", "", signIns), h("p", { class: "muted" }, "This dashboard only reads, except approvals and credit grants, which GitHub authorizes. Policies, checkpoints, budgets, alert rules, and integrations are changed with the Crane CLI by a person."));
}

// ---------------------------------------------------------------- chrome: rail, scope, time, filter builder, search

function currentPage() { return (location.hash.replace(/^#\//, "") || "overview").split("?")[0].split("/")[0]; }
function drawNav(page) {
  put(document.getElementById("nav"), NAV.map(([group, items]) => h("div", { class: "nav-group" }, h("h6", {}, group), items.map(([id, label, zones]) => {
    const active = page === id;
    return h("div", {}, h("a", { href: "#/" + id, class: active ? "active" : "", title: label, "aria-current": active ? "page" : null }, svg("svg", { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" }, svg("path", { d: ICONS[id] || ICONS.overview })), h("span", { class: "label" }, label)),
      active && zones.length ? h("div", { class: "zones" }, zones.map(([zoneId, zoneLabel]) => h("a", { href: "#", class: "zone-link", "data-zone": zoneId, onclick: (event) => { event.preventDefault(); scrollToZone(zoneId); } }, zoneLabel))) : null);
  }))));
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
  put(document.getElementById("time"), h("legend", { class: "visually-hidden" }, "Time range"), RANGES.map(([name]) => h("button", { type: "button", class: !custom && filter.range === name ? "on" : "", "aria-pressed": String(!custom && filter.range === name), onclick: () => { filter.range = name; filter.since = null; filter.until = null; saveFilter(); render(); } }, name)));
}
function clauseText(clause) {
  if (!clause) return "";
  const values = (clause.values || []).map((value) => valueLabel(clause.path, value));
  return [attrLabel(clause.path), OPS[clause.op] || clause.op, ["exists", "missing"].includes(clause.op) ? "" : values.join(", ")].filter(Boolean).join(" ");
}
function drawFilterBar() {
  const bar = document.getElementById("filterbar");
  if (!scope.repository) { bar.hidden = true; return; }
  bar.hidden = false;
  const pills = [];
  if (filter.since || filter.until) {
    pills.push(h("span", { class: "pill time" }, h("b", {}, "time"), " " + rangeLabel(), h("button", { type: "button", "aria-label": "Zoom out", title: "Zoom out (double the range)", onclick: () => { const end = filter.until || Date.now(), start = filter.since || end - 3600e3, span = end - start; filter.since = Math.floor(start - span / 2); filter.until = Math.min(Date.now(), Math.floor(end + span / 2)); saveFilter(); render(); } }, "⇲"), h("button", { type: "button", "aria-label": "Remove time range", onclick: () => { filter.since = null; filter.until = null; saveFilter(); render(); } }, "×")));
  }
  const groups = new Map();
  filter.clauses.forEach((clause, index) => { const key = clause.group ? "g" + clause.group : "c" + index; if (!groups.has(key)) groups.set(key, []); groups.get(key).push([clause, index]); });
  for (const [key, list] of groups) {
    const pillsOf = list.map(([clause, index]) => h("span", { class: "pill" + (clause.off ? " off" : ""), title: clause.off ? "Disabled: click ◌ to apply it again" : "" },
      h("button", { type: "button", class: "pill-text", title: "Edit this condition", onclick: (event) => builder(event.currentTarget, index) }, h("b", {}, attrLabel(clause.path)), " " + (OPS[clause.op] || clause.op) + " ", ["exists", "missing"].includes(clause.op) ? "" : (clause.values || []).map((value) => valueLabel(clause.path, value)).join(", ")),
      h("button", { type: "button", "aria-label": clause.off ? "Enable " + clauseText(clause) : "Disable " + clauseText(clause), title: clause.off ? "Enable" : "Disable without removing", onclick: () => { clause.off = !clause.off; saveFilter(); render(); } }, clause.off ? "◌" : "◉"),
      h("button", { type: "button", "aria-label": "Remove " + clauseText(clause), onclick: () => { filter.clauses.splice(index, 1); saveFilter(); render(); } }, "×")));
    if (key.startsWith("g") && list.length > 1) pills.push(h("span", { class: "or-group", title: "Any of these (OR)" }, "(", pillsOf.flatMap((pill, position) => (position ? [h("span", { class: "or" }, "or"), pill] : [pill])), ")"));
    else pills.push(...pillsOf);
  }
  let typing;
  const apply = (value) => { filter.q = value.trim(); saveFilter(); render(); };
  const search = h("input", { type: "search", placeholder: "Search text in any attribute…", value: filter.q, "aria-label": "Search events", oninput: (event) => { clearTimeout(typing); typing = setTimeout(() => apply(event.target.value), 700); }, onkeydown: (event) => { if (event.key === "Enter") { clearTimeout(typing); apply(event.target.value); } } });
  put(bar, h("span", { class: "lead" }, "Filter", info("", { title: "The global filter", text: "Applies to every event view, graph, list, and eval. Add a condition on any attribute of DASHBOARD.md with any operator (is, is not, contains, starts with, >, ≥, <, ≤, exists, is missing). Conditions combine with AND; conditions in one group (shown in brackets) combine with OR. Click a condition to edit it, ◉ to disable it, × to remove it." })),
    search, pills, h("button", { class: "btn", type: "button", onclick: (event) => builder(event.currentTarget, -1) }, "+ Add condition"),
    activeFilters() ? h("button", { class: "btn link", type: "button", onclick: () => { filter.clauses = []; filter.q = ""; filter.since = null; filter.until = null; saveFilter(); render(); } }, "Clear all") : null,
    h("span", { class: "muted right" }, "Applies to every event view, graph, and eval"));
}
/** The condition builder: attribute (any path, searchable), operator (by the attribute's kind),
 * values (with counts, or typed), and how it combines (AND, or OR with another condition) */
async function builder(anchor, index) {
  const editing = index >= 0 ? filter.clauses[index] : null;
  const data = await attributes();
  const all = (data.attributes || []).slice().sort((a, b) => (facetName(b.path) ? 1 : 0) - (facetName(a.path) ? 1 : 0) || a.path.localeCompare(b.path));
  const state = { path: editing ? editing.path : "severity.severity_status", op: editing ? editing.op : "is", values: new Set(editing ? editing.values : []), combine: editing && editing.group ? "g" + editing.group : "and" };
  const body = h("div", { class: "builder" });
  const draw = () => {
    const attribute = all.find((item) => item.path === state.path) || { path: state.path, kind: "text", values: [] };
    const ops = OPS_BY_KIND[attribute.kind] || OPS_BY_KIND.text;
    if (!ops.includes(state.op)) state.op = ops[0];
    const list = h("div", { class: "attr-list", role: "listbox", "aria-label": "Attributes" }, all.map((item) => h("button", { type: "button", role: "option", "aria-selected": String(item.path === state.path), class: item.path === state.path ? "on" : "", "data-path": item.path, onclick: () => { state.path = item.path; state.values = new Set(); draw(); } }, h("span", {}, facetName(item.path) ? FACET_LABELS[facetName(item.path)] + " · " : "", item.path), h("em", {}, item.kind + " · " + compact(item.present)))));
    const search = h("input", { type: "search", placeholder: "Find an attribute…", "aria-label": "Find an attribute", oninput: (event) => { const q = event.target.value.toLowerCase(); list.querySelectorAll("button").forEach((node) => { node.hidden = !node.dataset.path.toLowerCase().includes(q); }); } });
    const numeric = ["gt", "gte", "lt", "lte"].includes(state.op);
    const typed = h("input", { type: "text", placeholder: numeric ? "A number" + (attribute.min !== undefined && attribute.min !== null ? " (" + attribute.min + " to " + attribute.max + ")" : "") : "Or type values, comma separated", "aria-label": "Values", value: [...state.values].filter((value) => !(attribute.values || []).some((item) => item.value === value)).join(", ") });
    const valueList = ["exists", "missing"].includes(state.op) ? h("p", { class: "muted" }, "No value needed.") : h("div", {},
      numeric ? null : h("div", { class: "value-list" }, (attribute.values || []).length ? attribute.values.map((item) => h("label", {}, h("input", { type: "checkbox", checked: state.values.has(item.value) ? true : null, onchange: () => { if (state.values.has(item.value)) state.values.delete(item.value); else state.values.add(item.value); } }), h("span", {}, item.label || item.value), explained(item.value), h("em", {}, compact(item.count)))) : h("p", { class: "muted" }, "No listed values; type them below.")),
      typed);
    const groupsUsed = [...new Set(filter.clauses.map((clause, position) => (clause.group ? "g" + clause.group : position !== index ? "c" + position : null)).filter(Boolean))];
    const combine = select("Combine", state.combine, [["and", "AND: a new condition"]].concat(groupsUsed.map((key) => { const sample = key.startsWith("g") ? filter.clauses.find((clause) => "g" + clause.group === key) : filter.clauses[Number(key.slice(1))]; return [key, "OR with: " + clauseText(sample)]; })), (value) => { state.combine = value; });
    const submit = () => {
      for (const value of typed.value.split(",").map((item) => item.trim()).filter(Boolean)) state.values.add(value);
      if (!["exists", "missing"].includes(state.op) && !state.values.size) { typed.focus(); typed.classList.add("invalid"); return; }
      const clause = { path: state.path, op: state.op, values: ["exists", "missing"].includes(state.op) ? [] : [...state.values] };
      if (state.combine !== "and") {
        let group;
        if (state.combine.startsWith("g")) group = Number(state.combine.slice(1));
        else { group = Math.max(0, ...filter.clauses.map((item) => item.group || 0)) + 1; filter.clauses[Number(state.combine.slice(1))].group = group; }
        clause.group = group;
      }
      if (editing) filter.clauses[index] = clause; else filter.clauses.push(clause);
      saveFilter();
      document.querySelectorAll(".menu").forEach((node) => node.remove());
      render();
    };
    put(body, h("div", { class: "builder-cols" }, h("div", {}, h("strong", {}, "1. Attribute ", info("attr:" + state.path)), search, list), h("div", {}, h("strong", {}, "2. Operator"), select("Operator", state.op, ops.map((op) => [op, OPS[op]]), (value) => { state.op = value; draw(); }), h("strong", {}, "3. Values"), valueList, h("strong", {}, "4. Combine ", info("", { title: "Combining conditions", text: "AND adds a condition every event must meet. OR joins this condition with another one, so an event that meets either passes (shown in brackets in the filter bar)." })), combine)),
      h("div", { class: "builder-actions" }, h("button", { class: "btn", type: "button", onclick: () => document.querySelectorAll(".menu").forEach((node) => node.remove()) }, "Cancel"), h("button", { class: "btn primary", type: "button", onclick: submit }, editing ? "Update condition" : "Add condition")));
    const current = list.querySelector("button.on");
    if (current) current.scrollIntoView({ block: "nearest" });
  };
  draw();
  menu(anchor, body).classList.add("wide");
}
/** The search palette: pages and zones, repositories, metrics, sessions, flows, and event names */
async function openPalette() {
  if (document.querySelector(".palette")) return;
  const items = [];
  for (const [, pages] of NAV) for (const [id, label, zones] of pages) {
    items.push({ kind: "Page", label, run: () => go("#/" + id) });
    for (const [zoneId, zoneLabel] of zones) items.push({ kind: "Zone", label: label + " › " + zoneLabel, run: () => { go("#/" + id); setTimeout(() => scrollToZone(zoneId), 500); } });
  }
  for (const account of me ? me.accounts : []) for (const repository of (me.repositories || []).filter((item) => item.account === account.name)) {
    items.push({ kind: "Repository", label: repository.remote.replace(/^github\.com\//, ""), hint: account.kind, run: () => { scope.account = account.name; scope.repository = repository.id; scope.member = ""; saveScope(); go("#/overview"); } });
  }
  for (const [flow, [title]] of Object.entries(G.flows)) items.push({ kind: "Flow", label: flow + " " + title, run: () => drill({ clauses: [is("flow", flow)] }) });
  const input = h("input", { type: "search", placeholder: "Search pages, zones, repositories, metrics, sessions, flows, events…", "aria-label": "Search" });
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
  const [overviewData, sessionData, cache] = await Promise.all([lastEvals ? Promise.resolve({ evals: lastEvals }) : api("/api/overview", filterParams()).catch(() => ({})), api("/api/sessions").catch(() => ({ sessions: [] })), facets()]);
  for (const [name, value] of Object.entries(overviewData.evals || {})) if (value !== null && typeof value !== "object") items.push({ kind: "Metric", label: name, hint: compact(value), run: () => { go("#/overview"); setTimeout(() => scrollToZone("evals"), 500); } });
  for (const row of sessionData.sessions || []) items.push({ kind: "Session", label: row.session.foxx_session_id, hint: (row.member ? row.member.actor + " · " : "") + row.session.provider, run: () => go("#/sessions/" + encodeURIComponent(row.session.foxx_session_id)) });
  for (const item of (cache.facets || {}).event || []) items.push({ kind: "Event", label: item.value, hint: compact(item.count), run: () => drill({ clauses: [is("event_name", item.value)] }) });
  if (document.body.contains(palette)) draw();
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
  const changeScope = (changes) => { Object.assign(scope, changes); saveScope(); render(); };
  put(document.getElementById("scope"),
    labelled("Account", select("Account", scope.account, me.accounts.map((account) => [account.name, (account.kind === "personal" ? "Personal · " : "Org · ") + account.name]), (value) => changeScope({ account: value, repository: "", member: "" }))),
    labelled("Repo", select("Repository", scope.repository, owned.length ? owned.map((repository) => [repository.id, repository.remote.replace(/^github\.com\//, "")]) : [["", "no repositories yet"]], (value) => changeScope({ repository: value, member: "" }))),
    labelled("Member", select("Member", scope.member, [["", members.some((row) => row.linked_by) ? "All linked" : "All members"]].concat(members.map((row) => [row.member.key, row.member.actor + " (" + row.member.short + ")" + (row.linked_by ? " @" + row.linked_by : " · unlinked")])), (value) => changeScope({ member: value }))));
  put(document.getElementById("viewer"), "Signed in as ", h("b", {}, me.login), h("br"), h("a", { href: "/auth/logout" }, "Sign out"));
}
function signIn(message) {
  for (const id of ["scope", "viewer", "nav", "time", "filterbar"]) document.getElementById(id).replaceChildren();
  document.getElementById("filterbar").hidden = true;
  put(document.getElementById("main"), h("div", { class: "signin" },
    h("img", { src: "/logo.png", alt: "Foxx", width: "120", height: "120" }),
    h("h2", {}, "Crane security observability"),
    h("p", { class: "muted" }, "Sign in with GitHub to see your agents' sessions, actions, and events. Personal accounts see their own repositories; organization members see the organization's repositories with a filter for each teammate."),
    message ? h("p", { class: "notice error" }, message) : null,
    h("a", { class: "btn primary", href: "/auth/login" }, "Sign in with GitHub")));
}

const PAGES = { overview, explorer, timeline, logs, traces, topology: topologyPage, distributions, semantics, sessions, agents, alerting, credits, audit, governance: governanceView, compliance, repositories, settings };
async function render() {
  tooltip.hide();
  document.querySelectorAll(".menu").forEach((node) => node.remove());
  const [path, search] = location.hash.replace(/^#\//, "").split("?");
  if (search) {
    const params = new URLSearchParams(search);
    for (const [key, value] of params) { const facet = FACETS.find(([name]) => name === key); if (facet) toggleValue(facet[1], value); else if (key === "session") setClause("session.session_id", "is", [value]); }
    saveFilter();
    history.replaceState(null, "", "#/" + path);
  }
  const [page, id] = (path || "overview").split("/");
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
    drawFilterBar();
    put(main);
    if (approval) { main.append(h("div", { class: "notice" + (approval.ok ? " ok" : " error"), role: "status" }, approval.message)); approval = null; }
    if (page === "sessions" && id) await session(main, decodeURIComponent(id));
    else await (PAGES[page] || overview)(main);
    watchZones();
  } catch (error) {
    if (error instanceof SignedOut) signIn();
    else if (error.status === 503) signIn(error.message);
    else put(main, h("div", { class: "notice error" }, "Could not load: " + error.message));
  }
}

document.getElementById("toggle").addEventListener("click", () => {
  const app = document.getElementById("app");
  if (window.innerWidth <= 760) app.classList.toggle("open"); else app.classList.toggle("collapsed");
});
document.getElementById("details-toggle").addEventListener("click", () => { if (document.getElementById("details").hidden) details.guide(); else details.close(); });
document.getElementById("live").addEventListener("click", (event) => {
  const button = event.currentTarget;
  if (live) { clearInterval(live); live = null; button.classList.remove("on"); button.setAttribute("aria-pressed", "false"); button.textContent = "● Live"; }
  else { live = setInterval(() => { if (!document.querySelector(".overlay, .menu, .palette")) render(); }, 30000); button.classList.add("on"); button.setAttribute("aria-pressed", "true"); button.textContent = "● Live 30 s"; }
});
document.getElementById("nav").addEventListener("click", (event) => { if (event.target.closest("a:not(.zone-link)")) document.getElementById("app").classList.remove("open"); });
document.getElementById("search-open").addEventListener("click", openPalette);
document.addEventListener("keydown", (event) => {
  const typing = /^(input|select|textarea)$/i.test((event.target || {}).tagName || "");
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); openPalette(); }
  else if (event.key === "/" && !typing) { event.preventDefault(); openPalette(); }
  else if (event.key === "?" && !typing) { event.preventDefault(); details.guide(); }
  else if (event.key === "Escape") { details.close(); document.querySelectorAll(".menu").forEach((node) => node.remove()); }
});
window.addEventListener("hashchange", render);
render();
