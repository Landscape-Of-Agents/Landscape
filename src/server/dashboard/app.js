"use strict";
// The Foxx dashboard page: sign in with GitHub, choose an account (personal or organization), a
// repository, and optionally one team member, then browse every view. Everything is read-only.
const PAGES = [["repositories", "Repositories"], ["overview", "Overview"], ["agents", "Agents & Autonomy"], ["events", "Observability"], ["governance", "Governance"], ["compliance", "Compliance"], ["alerts", "Alerts"], ["settings", "Settings"]];
const scope = { account: "", repository: "", member: "" };
let me = null;
let members = [];
try { Object.assign(scope, JSON.parse(sessionStorage.getItem("foxx-scope") || "{}")); } catch (error) { /* storage unavailable */ }
function saveScope() { try { sessionStorage.setItem("foxx-scope", JSON.stringify(scope)); } catch (error) { /* storage unavailable */ } }

function h(tag, attributes, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attributes || {})) {
    if (key === "onclick") node.addEventListener("click", value);
    else if (key === "onchange") node.addEventListener("change", value);
    else if (value !== undefined && value !== null) node.setAttribute(key, value);
  }
  for (const child of children.flat()) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}
class SignedOut extends Error {}
function scoped(path) {
  const url = new URL(path, location.origin);
  if (!url.pathname.startsWith("/api/me") && !url.pathname.startsWith("/api/accounts")) {
    url.searchParams.set("repository", scope.repository);
    if (scope.member) url.searchParams.set("members", scope.member);
  }
  return url;
}
async function api(path) {
  const response = await fetch(scoped(path), { credentials: "same-origin" });
  const body = await response.json().catch(() => ({ error: "invalid response" }));
  if (response.status === 401) throw new SignedOut(body.error);
  if (!response.ok) { const error = new Error(body.error || response.statusText); error.status = response.status; throw error; }
  return body;
}
function fmt(value) {
  if (value === null || value === undefined) return "—";
  if (typeof value === "number") return Number.isInteger(value) ? String(value) : value.toFixed(3);
  if (Array.isArray(value)) return value.length ? value.map(fmt).join(", ") : "none";
  if (typeof value === "object") return Object.entries(value).map(([key, item]) => key + ": " + fmt(item)).join("  ");
  return String(value);
}
const TONES = { ALLOW: "ok", COMPLIANT: "ok", SATISFIED: "ok", PASS: "ok", ACTIVE: "ok", OBSERVED: "ok", verified: "ok", current: "ok",
  REQUIRE_APPROVAL: "warn", DEGRADED: "warn", PARTIAL: "warn", INFERRED: "warn", PENDING: "warn", SUSPICIOUS: "warn", COVERAGE_GAP: "warn", stale: "warn", NO_EVIDENCE: "warn", Medium: "warn", MEDIUM: "warn",
  DENY: "bad", VIOLATION: "bad", FAIL: "bad", NOT_SATISFIED: "bad", DETECTED_AFTER_EFFECT: "bad", UNRESOLVED: "bad", HIGH: "bad", ERROR: "bad", missing: "bad", broken: "bad",
  QUARANTINE: "crit", QUARANTINED: "crit", BYPASS_CONFIRMED: "crit", CRITICAL: "crit" };
function chip(value, title) { return value === null || value === undefined ? h("span", { class: "muted" }, "—") : h("span", { class: "chip " + (TONES[value] || ""), title }, String(value)); }
function status(metric) { return metric ? chip(metric.status, "how this value is known") : null; }
function metricValue(metrics, name) { const metric = metrics && metrics[name]; return metric ? metric.value : null; }
function member(value) { return value ? h("span", { class: "member", title: value.key }, value.actor + " ", h("code", { class: "muted" }, value.short)) : h("span", { class: "muted" }, "—"); }
function when(millis) { return millis ? new Date(millis).toISOString().replace(".000", "") : "—"; }
function tile(label, value, note) { return h("div", { class: "tile" }, h("div", { class: "label" }, label), h("div", { class: "value" }, fmt(value)), note ? h("div", { class: "note" }, note) : null); }
function table(columns, rows) {
  return h("div", { class: "panel" }, h("table", {}, h("thead", {}, h("tr", {}, columns.map((column) => h("th", {}, column[0])))),
    h("tbody", {}, rows.length ? rows.map((row) => h("tr", {}, columns.map((column) => { const cell = column[1](row); return h("td", { class: column[2] || "" }, cell instanceof Node ? cell : fmt(cell)); }))) : h("tr", {}, h("td", { colspan: columns.length, class: "muted" }, "Nothing recorded yet.")))));
}
function link(text, hash) { return h("a", { href: hash }, text); }
function metricsTable(metrics) {
  const rows = Array.isArray(metrics) ? metrics.map((metric) => [metric.name, metric]) : Object.entries(metrics || {});
  return table([["Metric", (row) => h("span", { class: "mono" }, row[0])], ["Value", (row) => fmt(row[1].value)], ["Unit", (row) => row[1].unit], ["Known as", (row) => status(row[1])], ["Domain", (row) => row[1].domain]], rows);
}
function repositoriesOf(account) { return me.repositories.filter((repository) => repository.account === account); }
function needRepository(main) {
  if (scope.repository) return false;
  main.append(h("p", { class: "muted" }, "Choose a repository above, or open ", link("Repositories", "#/repositories"), " for a summary of the account."));
  return true;
}

async function repositories(main) {
  if (!scope.account) { main.append(h("p", { class: "muted" }, "No account selected.")); return; }
  const data = await api("/api/accounts/" + encodeURIComponent(scope.account));
  main.append(h("h2", {}, "Repositories of " + scope.account), table([
    ["Repository", (row) => h("a", { href: "#/overview", onclick: () => { scope.repository = row.repository; scope.member = ""; saveScope(); } }, row.remote)],
    ["Members", (row) => row.members, "num"], ["Sessions", (row) => row.sessions, "num"], ["Events", (row) => row.events, "num"], ["Denied", (row) => row.denied ? chip(String(row.denied)) : "0", "num"],
    ["Confirmed bypasses", (row) => row.bypasses_confirmed ? chip("BYPASS_CONFIRMED", row.bypasses_confirmed + " confirmed") : "0"], ["Alerts", (row) => row.alerts, "num"], ["Last activity", (row) => when(row.last_activity)]], data.repositories));
  if (!data.repositories.length) main.append(h("p", { class: "muted" }, "A repository appears once a member's Crane uploads from a clone of a GitHub repository owned by this account."));
}

async function overview(main) {
  if (needRepository(main)) return;
  const data = await api("/api/overview");
  const metrics = data.metrics;
  main.append(h("h2", {}, "Activity"), h("div", { class: "grid" },
    tile("Members shown", data.members), tile("Active sessions", data.active_sessions, data.sessions + " total"), tile("Tasks", data.tasks, fmt(data.tasks_by_status)), tile("Agents", data.agents),
    tile("Actions allowed", metricValue(metrics, "actions_allowed")), tile("Actions denied", metricValue(metrics, "actions_denied"), "prevented before running"),
    tile("Approvals requested", metricValue(metrics, "actions_requiring_approval"))));
  main.append(h("h2", {}, "Security outcomes"), h("div", { class: "grid" },
    tile("Bypass attempts", metricValue(metrics, "bypass_attempts"), "denied attempts"), tile("Confirmed bypasses", metricValue(metrics, "bypasses_confirmed"), "protected code changed outside checks"),
    tile("Violations after effect", metricValue(metrics, "violations_after_effect")), tile("Coverage gaps", metricValue(metrics, "coverage_gaps"), "unobserved or undecided actions"),
    tile("Evidence completeness", metricValue(metrics, "evidence_completeness"), "allowed calls verified after running"), tile("Credits available", metricValue(metrics, "credits_available"), "risk-bearing authority")));
  main.append(h("h2", {}, "Integrity"), h("div", { class: "panel" },
    h("p", {}, "Governance state: ", data.integrity.reported ? chip(data.integrity.ok ? "PASS" : "FAIL") : h("span", { class: "muted" }, "not reported yet"), "  Event chains: ", chip(data.event_chain.status), " ", h("span", { class: "muted" }, data.event_chain.length + " events")),
    data.integrity.findings.length ? table([["Member", (row) => member(row.member)], ["Severity", (row) => chip(row.severity)], ["Code", (row) => h("code", {}, row.code)], ["Finding", (row) => row.message]], data.integrity.findings) : h("p", { class: "muted" }, "No integrity findings.")));
  main.append(h("h2", {}, "Recent violations"), table([["When", (row) => row.at], ["Member", (row) => member(row.member)], ["Session", (row) => link(row.session || "—", "#/sessions/" + row.session)], ["Enforcement", (row) => chip(row.enforcement)], ["Reason", (row) => fmt(row.reasons)]], data.recent_violations));
  main.append(h("h2", {}, "Recent alerts"), alertTable(data.recent_alerts));
}

function alertTable(rows) {
  return table([["When", (row) => new Date(row.at * 1000).toISOString()], ["Member", (row) => member(row.member)], ["Alert", (row) => row.title], ["Severity", (row) => chip(row.severity)], ["Session", (row) => row.session ? link(row.session, "#/sessions/" + row.session) : "—"], ["State", (row) => chip(row.state)],
    ["Delivery", (row) => row.suppressed ? "suppressed (cooldown)" : fmt((row.deliveries || []).map((delivery) => delivery.channel + ": " + delivery.result))]], rows);
}

async function agents(main) {
  if (needRepository(main)) return;
  const [agentData, sessionData] = await Promise.all([api("/api/agents"), api("/api/sessions")]);
  main.append(h("h2", {}, "Agents (across all runs)"), table([
    ["Agent", (row) => h("code", {}, row.agent_id)], ["Provider", (row) => row.provider], ["Sessions", (row) => row.sessions, "num"], ["Quarantined", (row) => row.quarantined_sessions, "num"],
    ["Denial rate", (row) => metricValue(row.metrics, "denial_rate"), "num"], ["Bypass attempts", (row) => metricValue(row.metrics, "bypass_attempts"), "num"], ["Confirmed bypasses", (row) => metricValue(row.metrics, "bypasses_confirmed"), "num"],
    ["Credits consumed", (row) => metricValue(row.metrics, "credits_consumed"), "num"], ["Last seen", (row) => row.last_seen]], agentData.agents));
  main.append(h("h2", {}, "Sessions"), table([
    ["Session", (row) => link(row.session.foxx_session_id, "#/sessions/" + row.session.foxx_session_id)], ["Member", (row) => member(row.member)], ["Provider id", (row) => row.session.provider_session_id === null ? h("span", { class: "muted" }, "not reported (" + row.session.id_provenance + ")") : row.session.provider_session_id],
    ["Model", (row) => row.session.model], ["Status", (row) => chip(row.session.status)], ["Mode", (row) => row.session.autonomy_mode], ["Safety", (row) => chip(row.session.safety)],
    ["Credits", (row) => row.credits.available + " / " + row.credits.granted, "num"], ["Started", (row) => row.session.started_at]], sessionData.sessions));
}

function graphView(graph) {
  const nodes = graph.nodes, width = 760, row = 26, height = Math.max(60, nodes.length * row + 20);
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 " + width + " " + height); svg.setAttribute("width", "100%"); svg.setAttribute("role", "img"); svg.setAttribute("aria-label", "action graph");
  const index = {}; nodes.forEach((node, position) => { index[node.id] = position; });
  const color = (node) => node.flags.includes("bypass_confirmed") ? "var(--crit)" : node.flags.includes("violation") ? "var(--bad)" : node.flags.includes("denied") ? "var(--warn)" : node.flags.includes("coverage_gap") || node.flags.includes("unresolved") ? "var(--warn)" : node.kind === "verification" ? "var(--ok)" : "var(--accent)";
  for (const edge of graph.edges.filter((edge) => edge.kind === "verified_by")) {
    const from = index[edge.from], to = index[edge.to]; if (from === undefined || to === undefined) continue;
    const path = document.createElementNS(svg.namespaceURI, "path"); const y1 = 20 + from * row, y2 = 20 + to * row, x = 30 + (to - from) * 6;
    path.setAttribute("d", "M20 " + y1 + " C" + (20 - x) + " " + y1 + " " + (20 - x) + " " + y2 + " 20 " + y2); path.setAttribute("fill", "none"); path.setAttribute("stroke", "var(--muted)"); path.setAttribute("stroke-dasharray", "3 3");
    svg.append(path);
  }
  nodes.forEach((node, position) => {
    const y = 20 + position * row;
    const circle = document.createElementNS(svg.namespaceURI, "circle"); circle.setAttribute("cx", "20"); circle.setAttribute("cy", String(y)); circle.setAttribute("r", node.kind === "decision" || node.kind === "verification" ? "6" : "4"); circle.setAttribute("fill", color(node));
    const label = document.createElementNS(svg.namespaceURI, "text"); label.setAttribute("x", "36"); label.setAttribute("y", String(y + 4));
    label.textContent = node.event.at.slice(11, 19) + "  " + node.event.type + "  " + node.label + (node.event.decision ? "  [" + node.event.decision + "]" : "") + (node.flags.length ? "  {" + node.flags.join(", ") + "}" : "");
    const title = document.createElementNS(svg.namespaceURI, "title"); title.textContent = (node.event.reasons || []).join("; ");
    circle.append(title); svg.append(circle, label);
  });
  return h("div", { class: "panel graph" }, h("p", { class: "legend muted" }, h("span", {}, "● decision / verification"), h("span", {}, "dashed: verified by the same tool call"), h("span", {}, "order only, no inferred causality")), svg);
}

async function session(main, id) {
  if (needRepository(main)) return;
  const data = await api("/api/sessions/" + encodeURIComponent(id));
  const graph = await api("/api/graph/" + encodeURIComponent(id));
  const info = data.session;
  main.append(h("h2", {}, "Session ", h("code", {}, info.foxx_session_id), " ", member(data.member)), h("div", { class: "grid" },
    tile("Provider", info.provider, info.provider_session_id === null ? "provider id not reported" : "provider id " + info.provider_session_id), tile("Model", info.model),
    tile("Safety", info.safety, info.safety_reason || ""), tile("Autonomy mode", info.autonomy_mode), tile("Credits available", data.credits.available, data.credits.consumed + " consumed, " + data.credits.revoked + " revoked, " + data.credits.expired + " expired"),
    tile("Bypass attempts", info.bypass_attempts, info.violations + " violations")));
  const download = (path, name) => async () => {
    const response = await fetch(scoped(path), { credentials: "same-origin" });
    const anchor = h("a", { href: URL.createObjectURL(await response.blob()), download: name });
    document.body.append(anchor); anchor.click(); anchor.remove();
  };
  main.append(h("p", {}, h("button", { onclick: download("/api/reports/session/" + info.foxx_session_id, "crane-report-" + info.foxx_session_id + ".json") }, "Download session report (JSON)"), " ",
    h("button", { onclick: download("/api/evidence/session/" + info.foxx_session_id, "crane-evidence-" + info.foxx_session_id + ".json") }, "Download evidence bundle")));
  main.append(h("h2", {}, "Action graph"), graphView(graph));
  main.append(h("h2", {}, "Tasks"), table([["Task", (row) => h("code", {}, row.task.foxx_task_id)], ["External", (row) => row.task.external_task_id], ["Status", (row) => chip(row.task.status)], ["Tool calls", (row) => row.task.tool_calls, "num"], ["Denied", (row) => metricValue(row.metrics, "actions_denied"), "num"], ["Started", (row) => row.task.started_at]], data.tasks || []));
  main.append(h("h2", {}, "Metrics"), h("details", {}, h("summary", {}, "All metrics with how each value is known"), metricsTable(data.metrics)));
}

async function events(main) {
  if (needRepository(main)) return;
  const params = new URLSearchParams(location.hash.split("?")[1] || "");
  const form = h("div", { class: "filters" });
  const fields = {};
  for (const [name, placeholder] of [["session", "session id"], ["task", "task id"], ["type", "event type prefix"], ["decision", "decision"], ["severity", "severity"], ["domain", "domain"], ["enforcement", "enforcement"], ["policy", "policy"], ["selection", "selection id"]]) {
    fields[name] = h("input", { placeholder, value: params.get(name) || "", "aria-label": placeholder }); form.append(fields[name]);
  }
  form.append(h("button", { onclick: () => { const query = new URLSearchParams(); for (const [name, input] of Object.entries(fields)) if (input.value) query.set(name, input.value); location.hash = "#/events?" + query; } }, "Filter"));
  main.append(h("h2", {}, "Event timeline"), form);
  const holder = h("div"); main.append(holder);
  let cursor = 0;
  const load = async () => {
    const query = new URLSearchParams(params); query.set("cursor", cursor); query.set("limit", "100");
    const data = await api("/api/events?" + query);
    holder.append(table([["When", (row) => row.at], ["Member", (row) => member(row.member)], ["Type", (row) => h("code", {}, row.type)], ["Session", (row) => row.session ? link(row.session, "#/sessions/" + row.session) : "—"],
      ["Decision", (row) => chip(row.decision)], ["Assessment", (row) => chip(row.assessment)], ["Enforcement", (row) => chip(row.enforcement)], ["Known as", (row) => chip(row.telemetry)], ["Reasons", (row) => fmt(row.reasons)]], data.events));
    if (data.next_cursor) { cursor = data.next_cursor; const more = h("button", { onclick: () => { more.remove(); load(); } }, "Load more"); holder.append(more); }
  };
  await load();
}

async function governance(main) {
  if (needRepository(main)) return;
  let data;
  try { data = await api("/api/governance"); } catch (error) { if (error.status === 404) { main.append(h("p", { class: "muted" }, error.message)); return; } throw error; }
  main.append(h("p", { class: "muted" }, "As reported by ", member(data.reported_by), " at ", when(data.reported_at), ". Results describe that machine's working tree."));
  main.append(h("h2", {}, "Governance state"), h("div", { class: "grid" }, tile("Registry generation", data.generation), tile("Default policy", data.defaults.policy), tile("Default checkpoint", data.defaults.checkpoint),
    tile("Selections", data.selections.length), tile("Policies passing", data.policies.filter((policy) => policy.passed).length + " / " + data.policies.length), tile("Autonomy", data.autonomy.mode, data.autonomy.session_credits + " credits per session"),
    tile("Admins", data.admins ? data.admins.admins.length + 1 : 1, "owner included")));
  main.append(h("h2", {}, "Policies"), table([["Policy", (row) => row.name], ["File", (row) => h("code", {}, row.file)], ["Result", (row) => chip(row.passed ? "PASS" : "FAIL")],
    ["Commands", (row) => h("div", {}, row.commands.map((command) => h("div", {}, chip(command.outcome), " ", h("code", {}, command.operation + " " + command.id), " ", h("span", { class: "muted" }, command.message))))]], data.policies));
  main.append(h("h2", {}, "Selections"), table([["Id", (row) => h("code", {}, row.id)], ["Operation", (row) => row.operation], ["Policy", (row) => row.policy], ["Resolved at", (row) => row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : chip("UNRESOLVED")],
    ["Origin", (row) => row.origin.path + ":" + row.origin.lines.join("-") + " @ " + row.origin.checkpoint], ["Language", (row) => row.language], ["Generation", (row) => row.generation, "num"]], data.selections));
  main.append(h("h2", {}, "Checkpoints"), table([["Name", (row) => row.name], ["Commit", (row) => h("code", {}, row.commit.slice(0, 12))], ["Branch", (row) => row.branch], ["Created", (row) => row.created_at], ["By", (row) => row.created_by]], data.checkpoints));
  main.append(h("h2", {}, "Policy context (guidance, never authorization)"), table([["Policy", (row) => row.policy], ["File", (row) => h("code", {}, row.path)], ["State", (row) => chip(row.state)]], data.contexts));
  main.append(h("h2", {}, "Findings"), table([["Severity", (row) => chip(row.severity)], ["Code", (row) => h("code", {}, row.code)], ["Finding", (row) => row.message]], data.findings));
  main.append(h("h2", {}, "Governance audit trail"), table([["#", (row) => row.seq, "num"], ["When", (row) => row.at], ["Actor", (row) => row.actor], ["Action", (row) => h("code", {}, row.action)], ["Detail", (row) => fmt(row.detail)]], data.audit));
}

async function compliance(main) {
  if (needRepository(main)) return;
  const data = await api("/api/compliance");
  main.append(h("p", { class: "muted" }, data.disclaimer), h("h2", {}, "Controls"), table([["Control", (row) => h("code", {}, row.control.id)], ["Objective", (row) => row.control.objective], ["Outcome", (row) => chip(row.outcome)],
    ["Evidence", (row) => fmt(row.evidence)], ["Gaps", (row) => fmt(row.gaps)], ["Supports", (row) => h("div", {}, row.control.mappings.map((mapping) => chip(mapping)))]], data.controls));
  main.append(h("h2", {}, "Telemetry Crane cannot collect"), h("div", { class: "panel" }, data.unsupported_telemetry.map((name) => chip(name))));
}

async function alerts(main) {
  if (needRepository(main)) return;
  const data = await api("/api/alerts");
  main.append(h("h2", {}, "Alerts"), alertTable(data.alerts));
}

async function settings(main) {
  if (needRepository(main)) return;
  const [health, repo, integrations, memberData] = await Promise.all([api("/api/health"), api("/api/repo"), api("/api/integrations"), api("/api/members")]);
  main.append(h("h2", {}, "Members"), table([["Member", (row) => member(row.member)], ["Shown", (row) => row.selected ? "yes" : "no"], ["Remote verified", (row) => chip(row.verified ? "verified" : "missing")], ["Events", (row) => row.events, "num"], ["Sessions", (row) => row.sessions, "num"],
    ["Last upload", (row) => when(row.last_upload)], ["Status reported", (row) => when(row.status_reported_at)], ["Crane", (row) => row.crane_version]], memberData.members));
  main.append(h("h2", {}, "System health"), h("div", { class: "grid" }, tile("Crane", health.reported_crane_version || health.crane_version, "schema v" + health.schema_version + ", metrics v" + health.metric_formula_version), tile("Registry generation", health.registry_generation),
    tile("Events", health.events, "chains " + health.event_chain.status + (health.truncated ? " (most recent shown)" : "")), tile("Ledger", health.ledger_entries, health.ledger_reconciles ? "reconciles" : "DOES NOT reconcile"), tile("Storage", "MongoDB", health.storage.service), tile("Mode", health.mode)));
  main.append(h("h2", {}, "Event chains by member"), table([["Member", (row) => member(row.member)], ["Chain", (row) => chip(row.event_chain.status)], ["Events", (row) => row.event_chain.length, "num"], ["Ledger", (row) => row.ledger_reconciles ? "reconciles" : chip("FAIL")]], health.event_chain.members));
  main.append(h("h2", {}, "Repository connection"), h("div", { class: "panel" }, repo.connection ? h("p", {}, repo.connection.repository + " via " + repo.connection.method + "  ·  privileges: " + fmt(repo.connection.privileges)) : h("p", { class: "muted" }, repo.remote + " (connection not reported yet)")));
  main.append(h("h2", {}, "Integrations (non-secret settings)"), table([["Integration", (row) => row.name], ["Member", (row) => member(row.member)], ["Settings", (row) => fmt(row.settings)], ["Secrets", (row) => row.secret_fields.length + " stored on the member's machine"], ["Configured", (row) => new Date(row.configured_at * 1000).toISOString()]], integrations.integrations));
  main.append(h("p", { class: "muted" }, "This dashboard is read-only. Policies, checkpoints, budgets, approvals, and integrations are changed with the Crane CLI by a human."));
}

function signIn(message) {
  document.getElementById("scope").replaceChildren();
  document.getElementById("viewer").replaceChildren();
  document.getElementById("nav").replaceChildren();
  document.getElementById("main").replaceChildren(h("div", { class: "signin" },
    h("h2", {}, "Crane security observability"),
    h("p", { class: "muted" }, "Sign in with GitHub to see your agents' sessions, actions, and metrics. Personal accounts see their own repositories; organization members see the organization's repositories with a filter for each teammate."),
    message ? h("p", { class: "error" }, message) : null,
    h("a", { class: "button", href: "/auth/login" }, "Sign in with GitHub")));
}

async function controls() {
  if (!me.accounts.some((account) => account.name === scope.account)) {
    const withRepositories = me.accounts.find((account) => repositoriesOf(account.name).length);
    scope.account = (withRepositories || me.accounts[0]).name;
  }
  const owned = repositoriesOf(scope.account);
  if (!owned.some((repository) => repository.id === scope.repository)) { scope.repository = owned.length ? owned[0].id : ""; scope.member = ""; }
  members = [];
  if (scope.repository) { try { members = (await api("/api/members")).members; } catch (error) { members = []; } }
  if (scope.member && !members.some((row) => row.member.key === scope.member)) scope.member = "";
  saveScope();
  const select = (label, value, options, change) => h("label", {}, label, h("select", { "aria-label": label, onchange: (event) => change(event.target.value) },
    options.map(([optionValue, text]) => { const option = h("option", { value: optionValue }, text); if (optionValue === value) option.selected = true; return option; })));
  document.getElementById("scope").replaceChildren(
    select("Account", scope.account, me.accounts.map((account) => [account.name, (account.kind === "personal" ? "Personal · " : "Organization · ") + account.name]), (value) => { scope.account = value; scope.repository = ""; scope.member = ""; saveScope(); render(); }),
    select("Repository", scope.repository, owned.length ? owned.map((repository) => [repository.id, repository.remote.replace(/^github\.com\//, "")]) : [["", "no repositories yet"]], (value) => { scope.repository = value; scope.member = ""; saveScope(); render(); }),
    select("Member", scope.member, [["", "All members"]].concat(members.map((row) => [row.member.key, row.member.actor + " (" + row.member.short + ")"])), (value) => { scope.member = value; saveScope(); render(); }));
  document.getElementById("viewer").replaceChildren("Signed in as " + me.login + " · ", h("a", { href: "/auth/logout" }, "Sign out"));
}

async function render() {
  const route = (location.hash.replace(/^#\//, "") || "overview").split("?")[0];
  const main = document.getElementById("main"); main.replaceChildren(h("p", { class: "muted" }, "Loading…"));
  try {
    me = await api("/api/me");
    await controls();
    const nav = document.getElementById("nav"); nav.replaceChildren(...PAGES.map(([id, label]) => h("a", { href: "#/" + id, class: route.split("/")[0] === id ? "active" : "" }, label)));
    main.replaceChildren();
    const [page, id] = route.split("/");
    if (page === "sessions" && id) await session(main, decodeURIComponent(id));
    else await ({ repositories, overview, agents, events, governance, compliance, alerts, settings }[page] || overview)(main);
  } catch (error) {
    if (error instanceof SignedOut) signIn();
    else if (error.status === 503) signIn(error.message);
    else main.replaceChildren(h("p", { class: "error" }, "Could not load: " + error.message));
  }
}
window.addEventListener("hashchange", render);
render();
