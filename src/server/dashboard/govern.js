"use strict";
// The Foxx dashboard: the pages that operate and govern. Operate (alerting, credits), Govern (audit
// trail, governance with zones, flows, and call graphs, compliance with posture, frameworks, and
// evidence), and Organization (repositories, settings with data retention); then the page starts.

window.FOXX_PAGES = window.FOXX_PAGES || {};

// ---------------------------------------------------------------- alerting

const SYMBOLS = { above: ">", at_least: "≥", below: "<", at_most: "≤", equals: "=" };
async function alerting(main) {
  if (needRepository(main)) return;
  const data = await api("/api/alerting");
  const rules = data.rules;
  const firing = rules.filter((rule) => rule.state === "FIRING").length;
  const failed = data.deliveries.filter((delivery) => delivery.outcome === "FAILED").length;
  const report = { kind: "report", what: "The alert rules and their history" };
  main.append(hero("Alerting", "Rules are built on a member's machine with 'crane alert add' and evaluated by every background sync; this page tracks them.",
    [["Rules", rules.length], ["Firing", firing, "value:FIRING"], ["Firings", data.firings.length], ["Deliveries", data.deliveries.length], ["Failed deliveries", failed, "value:FAILED"]]));
  if (!data.reported) main.append(h("div", { class: "notice" }, "No member has sent a status report yet, so rules are not visible. Run 'crane agent sync' on a machine with rules."));
  main.append(zone("rules", "Rules", "Every rule, its state, and its last value.", panel("Alert rules", { span: 12, flush: true, scope: report, info: { title: "Alert rules", text: "A rule watches one metric (an eval) over a time window, counting only events that match its filter, and compares it with a threshold. When it crosses, it fires: its channels are notified, again after its cooldown while still firing, and once more when it resolves." } }, async (body, big, ctx) => { ctx.record("rows", rules); put(body, dataTable([
    { label: "Rule", get: (row) => row.name, render: (row) => h("b", {}, row.name) },
    { label: "State", get: (row) => row.state || "NEW", render: (row) => chip(row.state || "NEW"), filter: true },
    { label: "Condition", get: (row) => row.description, render: (row) => h("span", {}, h("code", {}, row.metric), info("eval:" + row.metric), " " + (SYMBOLS[row.op] || row.op) + " " + row.threshold + " over " + row.window_minutes + " min", row.filter && row.filter.length ? h("div", { class: "muted" }, "counting " + clausesText(row.filter)) : null) },
    { label: "Last value", get: (row) => row.last_value, render: (row) => fmt(row.last_value), num: true },
    { label: "Since", get: (row) => row.since, render: (row) => h("span", { class: "mono" }, dt(row.since)) },
    { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
    { label: "Channels", get: (row) => (row.channels || []).join(", "), filter: true },
    { label: "Cooldown", get: (row) => row.cooldown_minutes, render: (row) => row.cooldown_minutes + " min", num: true },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
    { label: "Created", get: (row) => row.created_at, render: (row) => h("span", { class: "mono" }, dt(row.created_at)) },
  ], rules, { sort: 1, dir: 1, empty: "No alert rules yet. Build one on a member's machine (see Build a rule below).", name: "foxx-alert-rules" })); })));
  const ruleNames = [...new Set(data.evaluations.map((evaluation) => evaluation.rule))];
  const chosen = { rule: ruleNames[0] || "" };
  const valuePanel = panel("Rule value against its threshold", { span: 12, scope: report, info: { title: "Rule value against its threshold", text: "Each evaluation of the rule (by every background sync and 'crane alert check'), the value it measured, and its threshold (dashed). Big red dots are firings and reminders; green dots are resolutions." } }, async (body, big, ctx) => {
    const rule = rules.find((item) => item.name === chosen.rule) || {};
    const points = data.evaluations.filter((evaluation) => evaluation.rule === chosen.rule);
    const values = points.map((point) => point.value).filter((value) => value !== null && value !== undefined);
    ctx.record("value", { evaluations: points.length, firings: points.filter((point) => point.transition === "FIRING").length, resolutions: points.filter((point) => point.transition === "RESOLVED").length, "highest value": values.length ? Math.max(...values) : null, threshold: rule.threshold }, { lines: points.length ? ["The rule was evaluated " + points.length + " times; it " + (rule.state === "FIRING" ? "is firing now (since " + dt(rule.since) + ")." : "is not firing now.")] : [] });
    put(body, chosen.rule ? charts.threshold(points, { threshold: Number(rule.threshold !== undefined ? rule.threshold : (points[0] || {}).threshold || 0), symbol: SYMBOLS[rule.op || (points[0] || {}).op] || "", height: big ? 420 : 200 }) : h("div", { class: "empty" }, "No rule has been evaluated yet."));
  });
  if (ruleNames.length) valuePanel.controls.append(select("Rule", chosen.rule, ruleNames.map((name) => [name, name]), (value) => { chosen.rule = value; valuePanel.refresh(); }));
  main.append(zone("rule-values", "Rule values", "", valuePanel));
  main.append(zone("firings", "Firings", "Every notification a rule sent: firing, still firing (reminder), resolved, or test.", panel("Firings", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.firings); put(body, dataTable([
    { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at)), csv: (row) => dt(row.at) },
    { label: "Rule", get: (row) => row.rule, filter: true },
    { label: "State", get: (row) => (row.reminder ? "REMINDER" : row.state), render: (row) => chip(row.reminder ? "REMINDER" : row.state), filter: true },
    { label: "Value", get: (row) => row.value, render: (row) => fmt(row.value) + " (threshold " + fmt(row.threshold) + ")" },
    { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
    { label: "Deliveries", get: (row) => (row.deliveries || []).map((delivery) => delivery.channel + " " + delivery.result).join("; ") },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) },
  ], data.firings, { sort: 0, dir: -1, empty: "No rule has fired yet.", name: "foxx-alert-firings" })); })));
  const outcomes = {};
  for (const delivery of data.deliveries) outcomes[delivery.outcome] = (outcomes[delivery.outcome] || 0) + 1;
  main.append(zone("deliveries", "Deliveries", "Every message sent to Slack or WhatsApp, with the channel's answer.",
    panel("Delivery outcomes", { span: 4, scope: report }, async (body, big, ctx) => { const items = Object.entries(outcomes).map(([value, count]) => ({ value, count })); ctx.record("items", items, { unit: "deliveries" }); put(body, charts.donut(items, { unit: "deliveries", explain: explained, hint: "" })); }),
    panel("Delivery log", { span: 8, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.deliveries); put(body, dataTable([
      { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at)), csv: (row) => dt(row.at) },
      { label: "Rule", get: (row) => row.rule, filter: true }, { label: "Channel", get: (row) => row.channel, filter: true },
      { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true }, { label: "HTTP", get: (row) => row.status, num: true }, { label: "Took", get: (row) => row.ms, render: (row) => (row.ms === null || row.ms === undefined ? "—" : duration(row.ms)), num: true },
      { label: "Answer", get: (row) => row.result },
    ], data.deliveries, { sort: 0, dir: -1, empty: "Nothing delivered yet.", name: "foxx-alert-deliveries" })); }),
    panel("Channels", { span: 12, flush: true, scope: report, info: { title: "Channels", text: "Whether each machine has Slack or WhatsApp configured ('crane integrate slack|whatsapp'). A rule's notifications fail on a machine without its channel." } }, async (body, big, ctx) => { ctx.record("rows", data.channels); put(body, dataTable([
      { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) }, { label: "Channel", get: (row) => row.channel }, { label: "Configured", get: (row) => (row.configured ? "yes" : "no"), render: (row) => chip(row.configured ? "OK" : "PENDING") },
    ], data.channels, { noTools: true })); })));
  main.append(zone("alert-events", "Alert events", "The alert events of the stream (F14): raised, suppressed, delivered, failed, rule firings and resolutions.",
    seriesPanel("Alert events over time", { span: 8, by: "event", clauses: [is("flow", "F14")], info: "flow:F14" }),
    breakdownPanel("Alert events", { span: 4, by: "event", view: "toplist", clauses: [is("flow", "F14")] })));
  main.append(zone("hook-alerts", "Hook alerts", "Alerts the hooks raise on their own (bypass attempts, quarantine, coverage gaps, low credits, suspicious prompts) and stale sessions.",
    panel("Hook alerts", { span: 12, flush: true, info: "flow:F14", scope: { kind: "state", what: "Hook alerts" } }, async (body, big, ctx) => {
      const hooks = await api("/api/alerts");
      const list = hooks.alerts.filter((alert) => alert.kind !== "rule");
      ctx.record("rows", list);
      put(body, dataTable([
        { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at * 1000)) }, { label: "Alert", get: (row) => row.title }, { label: "Kind", get: (row) => row.kind, filter: true },
        { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
        { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
        { label: "Session", get: (row) => row.session, render: (row) => sessionLink(row.session) },
        { label: "Delivery", get: (row) => (row.computed ? "derived by the dashboard" : row.suppressed ? "suppressed (cooldown)" : (row.deliveries || []).map((delivery) => delivery.channel + ": " + delivery.result).join(", ")) },
      ], list, { sort: 0, dir: -1, name: "foxx-hook-alerts" }));
    })));
  const example = (text) => h("div", { class: "code-line" }, h("code", {}, text), h("button", { class: "btn small", type: "button", onclick: (event) => { const button = event.currentTarget; navigator.clipboard.writeText(text).then(() => { button.textContent = "Copied"; }).catch(() => {}); } }, "Copy"));
  main.append(zone("build", "Build a rule", "Rules are built from the command line on a member's machine, by a person (agents are refused).", panel("Build a rule with the CLI", { span: 12, noExpand: true, scope: { kind: "custom", text: "Instructions: no data is shown." } }, async (body) => put(body,
    h("p", {}, "1. Configure a channel once on the machine: ", h("code", {}, "crane integrate slack"), " or ", h("code", {}, "crane integrate whatsapp"), "."),
    h("p", {}, "2. Add a rule: a metric, a comparison, a threshold, a window, channels, and optional conditions on the events counted (any attribute, any operator; except negates):"),
    example("crane alert add too-many-denials metric denied at_least 5 window 15m channels slack,whatsapp severity high"),
    example("crane alert add bypass-watch metric bypass_attempts above 0 window 1h channels whatsapp severity critical cooldown 60"),
    example("crane alert add slow-hooks metric latency_ms.p95 above 800 window 30m channels slack where flow is F05"),
    example("crane alert add token-burn metric events at_least 20 window 10m channels slack where eval.model_tokens gt 20000"),
    example("crane alert add risky-bash metric events at_least 3 window 10m channels slack where tool is BASH where decision is DENY,QUARANTINE except event like *VERIFIED*"),
    h("p", {}, "3. Check it now, send a test message, and read its log: ", h("code", {}, "crane alert check"), ", ", h("code", {}, "crane alert test NAME"), ", ", h("code", {}, "crane alert log"), ". ", h("code", {}, "crane alert metrics"), " lists every metric."),
    h("p", { class: "muted" }, "Every background sync evaluates the rules; firings and deliveries are uploaded with it and appear here within about a minute.")))));
}

// ---------------------------------------------------------------- credits

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
      h("button", { class: "btn primary", type: "submit" }, "Continue with GitHub")), { kind: "Credits" });
    amount.focus();
  };
  main.append(zone("balances", "Balances", "Each session's credits from its ledger.", panel("Session balances", { span: 12, flush: true, info: "eval:credits", scope: { kind: "state", what: "Session balances" } }, async (body, big, ctx) => { ctx.record("rows", data.sessions); put(body, dataTable([
    { label: "Session", get: (row) => row.session, render: (row) => sessionLink(row.session) },
    { label: "Agent", get: (row) => row.agent, filter: true }, { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
    { label: "Active", get: (row) => (row.active ? "yes" : "no"), render: (row) => chip(row.active ? "ACTIVE" : "ENDED"), filter: true },
    { label: "Available", get: (row) => row.balance.available, num: true, bar: true }, { label: "Granted", get: (row) => row.balance.granted, num: true }, { label: "Consumed", get: (row) => row.balance.consumed, num: true },
    { label: "Reserved", get: (row) => row.balance.reserved, num: true }, { label: "Revoked", get: (row) => row.balance.revoked, num: true },
    { label: "Last seen", get: (row) => row.last_seen_at, render: (row) => h("span", { class: "mono" }, dt(row.last_seen_at)) },
    { label: "", render: (row) => h("button", { class: "btn primary small", type: "button", onclick: () => grantForm(row) }, "Grant credits") },
  ], data.sessions, { sort: 3, dir: -1, name: "foxx-credit-balances", empty: "No sessions yet." })); })));
  main.append(zone("consumption", "Actual consumption", "Net credits consumed after calls ran, from the autonomy ledger. Credits are fungible within a session, so a specific spend cannot be assigned to one dashboard grant.",
    panel("Consumed credits by session", { span: 12, info: { title: "Consumed credits by session", text: "Ledger-backed consumption (CONSUME minus REFUND) for each session. A dashboard grant becomes spendable only after the agent machine applies it; because credits share one session balance, the ledger does not claim which individual grant funded a later call." } }, async (body, big, ctx) => {
      const items = data.sessions.map((row) => ({ value: row.session, count: row.balance.consumed })).filter((row) => row.count > 0).sort((a, b) => b.count - a.count);
      ctx.record("items", items, { unit: "credits", source: "autonomy ledger" });
      put(body, charts.toplist(items, { unit: "credits", format: fmt, onPick: (id) => go("#/sessions/" + encodeURIComponent(id)), byValue: true, empty: "No session has consumed credits yet." }));
    })));
  main.append(zone("grants", "Grants", "Every grant approved on the dashboard and what the machine did with it.", panel("Grants", { span: 12, flush: true, scope: { kind: "state", what: "Credit grants" } }, async (body, big, ctx) => { ctx.record("rows", data.grants); put(body, dataTable([
    { label: "Approved", get: (row) => row.approved_at, render: (row) => h("span", { class: "mono" }, dt(row.approved_at)), csv: (row) => dt(row.approved_at) },
    { label: "By", get: (row) => "@" + row.approved_by, filter: true }, { label: "Session", get: (row) => row.session }, { label: "Credits", get: (row) => row.amount, num: true },
    { label: "Reason", get: (row) => row.reason }, { label: "Status", get: (row) => row.status, render: (row) => chip(row.status), filter: true },
    { label: "Settled", get: (row) => row.settled_at, render: (row) => h("span", { class: "mono" }, dt(row.settled_at)) }, { label: "Machine said", get: (row) => row.detail },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member) },
  ], data.grants, { sort: 0, dir: -1, empty: "No credits have been granted on the dashboard.", name: "foxx-credit-grants" })); })));
}

// ---------------------------------------------------------------- audit trail

async function audit(main) {
  if (needRepository(main)) return;
  let governance = null;
  try { governance = await api("/api/governance"); } catch (error) { if (error.status !== 404) throw error; }
  const trail = governance ? governance.audit || [] : [];
  const when = (entry) => entry.at_ms || Date.parse(entry.at);
  const actors = [...new Set(trail.map((entry) => entry.actor))];
  const actions = [...new Set(trail.map((entry) => entry.action))];
  const report = { kind: "report", what: "The governance audit trail", reported: governance && governance.reported_at };
  const newest = trail.length ? Math.max(...trail.map(when)) : null;
  main.append(hero("Audit trail", "Who changed the rules and when: the signed governance history, every Crane command, and what was done on this dashboard.", [["Governance changes", trail.length], ["Actors", actors.length], ["Kinds of change", actions.length], ["Newest change", newest ? dt(newest) : "—"]]));
  const dayMs = 86400000;
  const last7 = trail.filter((entry) => Date.now() - when(entry) < 7 * dayMs).length;
  main.append(zone("audit-summary", "Summary", "The signed governance history in numbers (all time; it is not filtered).",
    h("div", { class: "kpis span-12" },
      kpi("Changes", trail.length, { tone: "brand", info: "flow:F13" }),
      kpi("In the last 7 days", last7),
      kpi("Actors", actors.length, { note: actors.slice(0, 3).join(", ") }),
      kpi("Signatures", trail.filter((entry) => /sign/.test(entry.action)).length, { note: "governance sign" }),
      kpi("Checkpoints", trail.filter((entry) => /checkpoint/.test(entry.action)).length),
      kpi("Protect / target", trail.filter((entry) => /protect|target/.test(entry.action)).length))));
  main.append(zone("audit-activity", "Activity", "Governance changes per day by actor, and Crane commands over time.",
    panel("Governance changes per day", { span: 6, scope: report, info: { title: "Governance changes per day", text: "Each signed change to the governance (init, protect, target, checkpoint, sign, …) per day, by who made it." } }, async (body, big, ctx) => {
      const byDay = new Map();
      for (const entry of trail) { const day = Math.floor(when(entry) / dayMs) * dayMs; if (!byDay.has(day)) byDay.set(day, {}); byDay.get(day)[entry.actor] = (byDay.get(day)[entry.actor] || 0) + 1; }
      const days = [...byDay.keys()].sort((a, b) => a - b);
      if (!days.length) { put(body, h("div", { class: "empty" }, "No governance changes reported.")); return; }
      const start = days[0], buckets = Math.round((days[days.length - 1] - start) / dayMs) + 1;
      const series = actors.map((actor) => ({ key: actor, label: actor, total: trail.filter((entry) => entry.actor === actor).length, points: Array.from({ length: buckets }, (_, index) => (byDay.get(start + index * dayMs) || {})[actor] || 0) }));
      const data = { start, bucket_ms: dayMs, buckets, series, total: trail.length };
      ctx.record("timeseries", data, { bucket: "1 day" });
      put(body, charts.timeseries(data, { type: "bars", height: big ? 420 : 200, legend: true }));
    }),
    seriesPanel("Crane commands over time", { span: 6, by: "event", clauses: [is("flow", "F13")], info: "flow:F13" })));
  main.append(zone("audit-matrix", "Who changed what", "Each actor against each kind of change.",
    panel("Actor by action", { span: 7, scope: report, info: { title: "Actor by action", text: "How many changes of each kind each actor made (all time). Darker is more." } }, async (body, big, ctx) => {
      const data = { rows: actors.map((actor) => ({ key: actor, label: actor, total: trail.filter((entry) => entry.actor === actor).length })), cols: actions.map((action) => ({ key: action, label: action, total: 0 })), cells: actors.map((actor) => actions.map((action) => trail.filter((entry) => entry.actor === actor && entry.action === action).length)), rows_by: "actor", cols_by: "action" };
      data.max = Math.max(0, ...data.cells.flat());
      ctx.record("matrix", data, {});
      put(body, charts.heatmap(data, { unit: "changes", onPick: (actor, action) => details.open(actor + " · " + action, dataTable([{ label: "#", get: (row) => row.seq, num: true }, { label: "Time", get: when, render: (row) => h("span", { class: "mono" }, dt(when(row))) }, { label: "Detail", get: (row) => JSON.stringify(row.detail) }], trail.filter((entry) => entry.actor === actor && entry.action === action), { noTools: true, sort: 1 }), { kind: "Audit" }) }));
    }),
    panel("Kinds of change", { span: 5, scope: report }, async (body, big, ctx) => {
      const items = actions.map((action) => ({ value: action, count: trail.filter((entry) => entry.action === action).length })).sort((a, b) => b.count - a.count);
      ctx.record("items", items, { unit: "changes" });
      put(body, charts.donut(items.slice(0, 8), { unit: "changes" }));
    })));
  main.append(zone("audit-timeline", "Change timeline", "Every governance change at its exact time, on a lane per actor, colored by kind.",
    panel("Change timeline", { span: 12, scope: report }, async (body, big, ctx) => {
      if (!trail.length) { put(body, h("div", { class: "empty" }, "No governance changes reported.")); return; }
      const times = trail.map(when);
      const data = { start: Math.min(...times), end: Math.max(...times) + 60000, color_by: "action", total: trail.length, lanes: actors.map((actor) => ({ key: actor, label: actor, count: trail.filter((entry) => entry.actor === actor).length })), points: trail.map((entry) => ({ t: when(entry), lane: entry.actor, color: entry.action, name: "#" + entry.seq + " " + entry.action, id: entry.seq, statement: JSON.stringify(entry.detail).slice(0, 120) })) };
      ctx.record("lanes", data, {});
      put(body, charts.swimlanes(data, { onPick: (point) => { const entry = trail.find((item) => item.seq === point.id); if (entry) openChange(entry); } }));
    })));
  const openChange = (row) => details.open("Governance change #" + row.seq, h("div", {}, h("div", { class: "kv" }, [["Time", dt(when(row))], ["Actor", row.actor], ["Action", row.action]].map(([key, value]) => [h("div", {}, key), h("div", {}, value)])), h("h4", {}, "Detail"), h("pre", { class: "json" }, JSON.stringify(row.detail, null, 2))), { kind: "Audit" });
  main.append(zone("audit-trail", "Governance changes", "Every signed change, newest first; open one for its detail.", panel("Governance audit trail", { span: 12, flush: true, info: "flow:F13", scope: report }, async (body, big, ctx) => { ctx.record("rows", trail); put(body, governance ? dataTable([
    { label: "#", get: (row) => row.seq, num: true },
    { label: "Time", get: when, render: (row) => h("span", { class: "mono" }, dt(when(row))), csv: (row) => dt(when(row)) },
    { label: "Actor", get: (row) => row.actor, filter: true }, { label: "Action", get: (row) => row.action, render: (row) => h("code", {}, row.action), filter: true },
    { label: "Detail", get: (row) => JSON.stringify(row.detail), render: (row) => h("span", { class: "muted clip" }, fmt(row.detail)) },
  ], trail, { sort: 0, dir: -1, placeholder: "Search actors, actions, details…", empty: "No governance changes recorded.", name: "foxx-governance-audit", onRow: openChange })
    : h("div", { class: "notice" }, "No member has reported the repository's governance yet; it is sent with the next upload.")); })));
  main.append(zone("command-events", "Commands", "Every Crane command (F13) in range and filter, and how they ended.",
    breakdownPanel("Command outcomes", { span: 4, by: "response", clauses: [is("flow", "F13")], info: "facet:response" }),
    panel("Command events", { span: 8, flush: true, info: "flow:F13", clauses: [is("flow", "F13")], filterable: true }, async (body, big, ctx) => {
      const commands = await api("/api/events", filterParams({ clauses: ctx.clauses(), limit: 500 }));
      ctx.record("rows", commands.events, { summary: compact(commands.total) + " command events." });
      put(body, eventsTable(commands.events, { pageSize: 15, name: "foxx-command-events" }));
    })));
  main.append(zone("dashboard-actions", "Dashboard actions", "What people did on this dashboard and the service: approvals, member links, owner keys, credit grants, and views (service events).",
    seriesPanel("Service events over time", { span: 8, by: "event", clauses: [is("origin", "SERVICE")], info: { title: "Service events", text: "Events the ingestion service records itself: uploads, status reports, approvals, credit grants, and dashboard views." } }),
    breakdownPanel("Service events", { span: 4, by: "event", view: "toplist", clauses: [is("origin", "SERVICE")] })));
}

// ---------------------------------------------------------------- governance

/** The zones and flows of the repository and the call graphs of its flows (shared with Topology) */
function codeTopology(gov) {
  if (!gov) return [h("div", { class: "notice span-12" }, "No member has reported the repository's governance yet; zones and flows appear with the next upload.")];
  const resolution = gov.zones_and_flows || { entities: [], findings: [], uncovered: [] };
  const config = gov.governance_config || { zones: [], flows: [], context_packs: [] };
  const entities = resolution.entities || [];
  const report = { kind: "report", what: "The zones and flows", reported: gov.reported_at };
  const described = (entity) => (config[entity.kind === "zone" ? "zones" : "flows"] || []).find((item) => item.id === entity.id) || {};
  const openEntity = (entity) => {
    const definition = described(entity);
    details.open(entity.kind + " " + entity.id, h("div", {}, h("p", {}, definition.description || ""), h("div", { class: "kv" }, [["status", chip(entity.status)], ["criticality", chip(entity.criticality)], ["autonomy ceiling", entity.ceiling], ["owner", definition.owner || "—"], ["policies", (definition.policies || []).join(", ") || "—"], ["context packs", (definition.context_packs || []).join(", ") || "—"], ["entry points", (definition.entry_points || []).join(", ") || "—"], ["files covered", (entity.files || []).length]].map(([key, value]) => [h("div", {}, key), h("div", {}, value instanceof Node ? value : fmt(value))])),
      entity.graph ? h("div", {}, h("h4", {}, "Call graph · " + entity.graph.status), charts.layered(entity.graph, { title: entity.id }), entity.graph.unresolved && entity.graph.unresolved.length ? h("div", {}, h("h4", {}, "Unresolved"), h("ul", {}, entity.graph.unresolved.map((item) => h("li", {}, item)))) : null) : null,
      h("h4", {}, "Files"), h("ul", {}, (entity.files || []).slice(0, 60).map((file) => h("li", {}, h("code", {}, file)))),
      entity.kind === "zone" ? h("div", { class: "row-actions" }, h("button", { class: "btn", type: "button", onclick: () => { details.close(); drill({ clauses: [is("other.zones", entity.id)] }); } }, "Open the events in this zone")) : null), { wide: true, kind: entity.kind === "zone" ? "Zone" : "Flow" });
  };
  const counts = (key) => { const map = {}; for (const entity of entities) map[entity[key]] = (map[entity[key]] || 0) + 1; return Object.entries(map).map(([value, count]) => ({ value, count })); };
  return [
    h("div", { class: "kpis span-12" },
      kpi("Zones", entities.filter((entity) => entity.kind === "zone").length, { tone: "brand" }),
      kpi("Flows", entities.filter((entity) => entity.kind === "flow").length),
      kpi("Proposed (not enforced)", entities.filter((entity) => entity.status === "proposed").length, { tone: entities.some((entity) => entity.status === "proposed") ? "warnt" : "" }),
      kpi("Uncovered files", (resolution.uncovered || []).length, { note: "in no active zone or flow" }),
      kpi("Files analyzed", resolution.analyzed, { note: fmt(resolution.unsupported) + " unsupported" }),
      kpi("Findings", (resolution.findings || []).length, { tone: (resolution.findings || []).length ? "bad" : "good" })),
    panel("Zones and flows", { span: 8, flush: true, scope: report, info: { title: "Zones and flows", text: "Zones mark risky code and flows paths through it (from an entry point along the calls). Each has a criticality and an autonomy ceiling (the most an agent may do there); proposed ones are not enforced yet. Click one for its files and call graph." } }, async (body, big, ctx) => { ctx.record("rows", entities); put(body, dataTable([
      { label: "Kind", get: (row) => row.kind, render: (row) => chip(row.kind === "zone" ? "ZONE" : "FLOW"), filter: true },
      { label: "Id", get: (row) => row.id, render: (row) => h("b", {}, row.id) },
      { label: "Status", get: (row) => row.status, render: (row) => chip(row.status), filter: true },
      { label: "Criticality", get: (row) => row.criticality, render: (row) => chip(row.criticality), filter: true },
      { label: "Ceiling", get: (row) => row.ceiling, filter: true },
      { label: "Files", get: (row) => (row.files || []).length, num: true, bar: true },
      { label: "Graph", get: (row) => (row.graph ? row.graph.status + " · " + row.graph.nodes.length + " functions" : ""), render: (row) => (row.graph ? h("span", {}, chip(row.graph.status), " " + row.graph.nodes.length + " functions") : h("span", { class: "muted" }, "—")) },
      { label: "Description", get: (row) => described(row).description || "", render: (row) => h("span", { class: "muted clip" }, described(row).description || "") },
    ], entities, { sort: 1, dir: 1, onRow: openEntity, name: "foxx-zones-flows", empty: "No zones or flows: add them in .crane/governance.yaml or from Foxx in the editor." })); }),
    panel("Criticality and ceilings", { span: 4, scope: report, info: { title: "Criticality and ceilings", text: "How the zones and flows divide by criticality (routine, sensitive, critical, restricted) and autonomy ceiling (observe, assisted, delegated, autonomous)." } }, async (body, big, ctx) => {
      const criticality = counts("criticality"), ceiling = counts("ceiling");
      ctx.record("items", criticality, { unit: "zones and flows" });
      put(body, h("div", {}, h("h4", { class: "muted small" }, "Criticality"), charts.toplist(criticality, { byValue: true, empty: "None." }), h("h4", { class: "muted small" }, "Autonomy ceiling"), charts.toplist(ceiling, { byValue: true, empty: "None." })));
    }),
    panel("Flow call graphs", { span: 12, scope: report, info: { title: "Flow call graphs", text: "Each flow's call graph as the machine resolved it: entry points on the left, the functions they reach by depth to the right. Hover a function to follow its calls." } }, async (body, big, ctx) => {
      const flows = entities.filter((entity) => entity.graph && entity.graph.nodes && entity.graph.nodes.length);
      ctx.record("value", { flows: flows.length, functions: flows.reduce((sum, entity) => sum + entity.graph.nodes.length, 0), calls: flows.reduce((sum, entity) => sum + (entity.graph.edges || []).length, 0) }, {});
      if (!flows.length) { put(body, h("div", { class: "empty" }, "No flow with a resolved call graph.")); return; }
      const state = { flow: flows[0].id };
      const holder = h("div");
      const draw = () => { const entity = flows.find((item) => item.id === state.flow); put(holder, h("p", { class: "muted" }, entity.graph.status + " · " + entity.graph.nodes.length + " functions · " + (entity.graph.edges || []).length + " calls · " + entity.graph.files.length + " files"), charts.layered(entity.graph, { title: entity.id })); };
      put(body, select("Flow", state.flow, flows.map((entity) => [entity.id, entity.id]), (value) => { state.flow = value; draw(); }), holder);
      draw();
    }),
  ];
}
async function governanceView(main) {
  if (needRepository(main)) return;
  let data;
  try { data = await api("/api/governance"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
  const [health, overviewData] = await Promise.all([api("/api/health").catch(() => null), api("/api/overview", filterParams()).catch(() => null)]);
  const passing = data.policies.filter((policy) => policy.passed).length;
  const report = { kind: "report", what: "The governance state", reported: data.reported_at };
  main.append(hero("Governance", ["As reported by", data.reported_by ? data.reported_by.actor : "a member", "at", dt(data.reported_at), "· results describe that machine's working tree"].join(" "), [["Generation", data.generation], ["Policies passing", passing + " / " + data.policies.length], ["Selections", data.selections.length], ["Autonomy", data.autonomy.mode], ["Integrity", overviewData ? (overviewData.integrity.ok ? "passing" : "failing") : "—"]]));
  main.append(zone("gov-state", "State", "", h("div", { class: "kpis span-12" }, kpi("Selections", data.selections.length, { tone: "brand" }), kpi("Unresolved selections", data.selections.filter((row) => !row.resolved).length, { tone: data.selections.some((row) => !row.resolved) ? "bad" : "good", info: "value:UNRESOLVED" }),
    kpi("Policies failing", data.policies.length - passing, { tone: passing < data.policies.length ? "bad" : "good" }), kpi("Checkpoints", data.checkpoints.length), kpi("Findings", data.findings.length, { tone: data.findings.length ? "bad" : "good" }),
    kpi("Relocations", (data.relocations || []).length, { note: "selections that moved" }),
    kpi("Admins", data.admins ? data.admins.admins.length + 1 : 1, { note: "owner included" }), kpi("Credits per session", data.autonomy.session_credits, { info: "eval:credits" }))));
  const topo = codeTopology(data);
  main.append(zone("gov-zones", "Zones and flows", "The risky parts of the code and the paths through them, with their criticality and autonomy ceiling.", ...topo.slice(0, 3)));
  main.append(zone("gov-graphs", "Flow call graphs", "", topo[3]));
  main.append(zone("gov-rules", "Policies and selections", "",
    panel("Policies", { span: 6, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.policies); put(body, dataTable([{ label: "Policy", get: (row) => row.name }, { label: "File", get: (row) => row.file, render: (row) => h("code", {}, row.file) }, { label: "Result", get: (row) => (row.passed ? "PASS" : "FAIL"), render: (row) => chip(row.passed ? "PASS" : "FAIL"), filter: true },
      { label: "Commands", get: (row) => row.commands.length, render: (row) => h("div", {}, row.commands.map((command) => h("div", {}, chip(command.outcome), " ", h("code", {}, command.operation + " " + command.id), " ", h("span", { class: "muted" }, command.message)))) }], data.policies, { sort: 0, dir: 1, noColumns: true })); }),
    panel("Selections", { span: 6, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.selections); put(body, dataTable([{ label: "Id", get: (row) => row.id, render: (row) => h("code", {}, row.id) }, { label: "Name", get: (row) => row.name }, { label: "Operation", get: (row) => row.operation, filter: true }, { label: "Policy", get: (row) => row.policy, filter: true },
      { label: "Resolved at", get: (row) => (row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : "UNRESOLVED"), render: (row) => (row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : chip("UNRESOLVED")) },
      { label: "Language", get: (row) => row.language, filter: true }, { label: "Gen", get: (row) => row.generation, num: true }], data.selections, { sort: 0, dir: 1, noColumns: true })); }),
    panel("Command outcomes per policy", { span: 12, scope: report, info: { title: "Command outcomes per policy", text: "For each policy, how its preserve and target commands ended on the reporting machine: PASS, FAIL, or UNRESOLVED." } }, async (body, big, ctx) => {
      const rows = data.policies.map((policy) => { const parts = {}; for (const command of policy.commands) parts[command.outcome] = (parts[command.outcome] || 0) + 1; return { label: policy.name, key: policy.name, parts }; });
      ctx.record("rows", rows);
      put(body, charts.stacked(rows, { keys: ["PASS", "FAIL", "UNRESOLVED"], unit: "commands", empty: "No policies." }));
    })));
  main.append(zone("gov-map", "Selection map", "Where the protected and targeted code is.",
    panel("Selections by file", { span: 6, scope: report }, async (body, big, ctx) => { const map = {}; for (const row of data.selections) { const file = row.resolved ? row.resolved.path : "UNRESOLVED"; map[file] = (map[file] || 0) + 1; } const items = Object.entries(map).map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count); ctx.record("items", items, { unit: "selections" }); put(body, charts.toplist(items, { empty: "No selections." })); }),
    panel("By operation and language", { span: 6, scope: report }, async (body, big, ctx) => {
      const tally = (key) => { const map = {}; for (const row of data.selections) map[row[key] || "unknown"] = (map[row[key] || "unknown"] || 0) + 1; return Object.entries(map).map(([value, count]) => ({ value, count })); };
      const operations = tally("operation");
      ctx.record("items", operations, { unit: "selections" });
      put(body, h("div", { class: "grid", style: "margin:0" }, h("div", { class: "span-6" }, charts.donut(operations, { unit: "selections" })), h("div", { class: "span-6" }, charts.toplist(tally("language"), { empty: "None." }))));
    }),
    panel("Relocations", { span: 12, flush: true, scope: report, info: { title: "Relocations", text: "Selections whose anchors moved since they were recorded (the code around them changed); Crane follows them." } }, async (body, big, ctx) => { ctx.record("rows", data.relocations || []); put(body, dataTable([{ label: "Relocation", get: (row) => JSON.stringify(row), render: (row) => h("code", {}, typeof row === "string" ? row : JSON.stringify(row)) }], data.relocations || [], { noTools: true, empty: "No selection moved." })); })));
  main.append(zone("gov-more", "Checkpoints and context", "",
    panel("Checkpoints", { span: 6, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.checkpoints); put(body, dataTable([{ label: "Name", get: (row) => row.name }, { label: "Commit", get: (row) => row.commit, render: (row) => h("code", {}, row.commit.slice(0, 12)) }, { label: "Branch", get: (row) => row.branch, filter: true }, { label: "Created", get: (row) => row.created_at }, { label: "By", get: (row) => row.created_by, filter: true }], data.checkpoints, { sort: 3, noColumns: true })); }),
    panel("Policy context (guidance, never authorization)", { span: 6, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.contexts); put(body, dataTable([{ label: "Policy", get: (row) => row.policy, filter: true }, { label: "File", get: (row) => row.path, render: (row) => h("code", {}, row.path) }, { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true }], data.contexts, { noColumns: true })); })));
  main.append(zone("gov-integrity", "Integrity", "Whether each machine's copy of the rules verifies, and whether its evidence chain is whole.",
    panel("Rules integrity by member", { span: 6, flush: true, scope: { kind: "report", what: "Each member's integrity check" } }, async (body, big, ctx) => {
      const findings = overviewData ? overviewData.integrity.findings || [] : [];
      ctx.record("rows", findings);
      put(body, findings.length ? dataTable([{ label: "Member", render: (row) => member(row.member) }, { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "Code", get: (row) => row.code, render: (row) => h("code", {}, row.code) }, { label: "Finding", get: (row) => row.message }], findings, { noTools: true }) : h("div", { class: "empty" }, overviewData && overviewData.integrity.reported ? "Every member's rules verify." : "No member reported its integrity yet."));
    }),
    panel("Evidence chains", { span: 6, flush: true, info: "flow:F09", scope: { kind: "state", what: "Each member's event chain" } }, async (body, big, ctx) => { const list = health ? health.event_chain.members : []; ctx.record("rows", list); put(body, dataTable([{ label: "Member", render: (row) => member(row.member) }, { label: "Chain", get: (row) => row.event_chain.status, render: (row) => chip(row.event_chain.status), filter: true }, { label: "Events", get: (row) => row.event_chain.length, num: true }, { label: "Expired before", get: (row) => row.event_chain.expired_before, render: (row) => (row.event_chain.expired_before ? "#" + row.event_chain.expired_before : "—"), num: true }, { label: "Ledger", get: (row) => (row.ledger_reconciles ? "reconciles" : "FAIL"), render: (row) => (row.ledger_reconciles ? "reconciles" : chip("FAIL")) }], list, { noTools: true })); })));
  main.append(zone("gov-events", "Governance events", "Governance activity in the event stream: rules changed, signed, verified, and integrity checks.",
    seriesPanel("Governance events over time", { span: 8, by: "event", clauses: [{ path: "flow", op: "is", values: ["F01", "F12", "F13", "F18", "F19", "F20", "F28"] }], info: { title: "Governance events", text: "Events of the governance flows: setup (F01), integrity (F12), commands (F13), and rule changes and verification (F18 to F20, F28)." } }),
    breakdownPanel("By flow", { span: 4, by: "flow", clauses: [{ path: "flow", op: "is", values: ["F01", "F12", "F13", "F18", "F19", "F20", "F28"] }], info: "facet:flow" })));
  main.append(zone("gov-findings", "Findings", "", panel("Findings", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.findings); put(body, dataTable([{ label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "Code", get: (row) => row.code, render: (row) => h("code", {}, row.code), filter: true }, { label: "Finding", get: (row) => row.message }], data.findings, { empty: "No findings.", noColumns: true })); })));
}

// ---------------------------------------------------------------- compliance

const OUTCOME_ORDER = ["SATISFIED", "PARTIAL", "NOT_SATISFIED", "NO_EVIDENCE", "NOT_APPLICABLE"];
async function compliance(main) {
  if (needRepository(main)) return;
  const data = await api("/api/compliance");
  const outcomes = OUTCOME_ORDER.filter((outcome) => (data.outcomes || {})[outcome]).map((value) => ({ value, count: data.outcomes[value] }));
  const applicable = data.controls.filter((control) => control.outcome !== "NOT_APPLICABLE");
  const satisfied = applicable.filter((control) => control.outcome === "SATISFIED").length;
  const score = applicable.length ? (satisfied + 0.5 * applicable.filter((control) => control.outcome === "PARTIAL").length) / applicable.length : null;
  const state = { outcome: "", q: "", view: "cards" };
  const evaluated = { kind: "custom", text: "Controls are evaluated from all the evidence the repository holds (every event, the ledger, and the members' latest local checks) at " + dt(data.evaluated_at) + "; the time range and event filter do not apply." };
  let controlsPanel = null;
  const focus = (outcome) => { state.outcome = outcome; if (controlsPanel) controlsPanel.refresh(); scrollToZone("controls"); };
  main.append(h("div", { class: "notice" }, data.disclaimer, " Evaluated at ", dt(data.evaluated_at), data.inputs_reported ? "." : ". No machine has reported its local checks yet, so some controls show no evidence."));
  main.append(zone("posture", "Posture", "How the controls stand. Gaps are fixable; limitations are inherent to the method and never lower an outcome.",
    panel("Compliance posture", { span: 4, scope: evaluated, info: { title: "Compliance posture", text: "The share of applicable controls satisfied (a partial control counts half). Controls with nothing to apply to yet are left out." } }, async (body, big, ctx) => { ctx.record("value", { applicable: applicable.length, satisfied, score: score === null ? null : Math.round(score * 100) + "%" }, { lines: [score === null ? "No control applies yet." : satisfied + " of " + applicable.length + " applicable controls are satisfied."] }); put(body, charts.gauge(score, { label: satisfied + " of " + applicable.length + " applicable satisfied" })); }),
    panel("Control outcomes", { span: 4, scope: evaluated, info: { title: "Control outcomes", text: "SATISFIED: positive evidence. PARTIAL: some evidence, with fixable gaps. NOT_SATISFIED: evidence of failure. NO_EVIDENCE: nothing exercised it yet (not a failure). NOT_APPLICABLE: nothing for it to apply to yet." } }, async (body, big, ctx) => { ctx.record("items", outcomes, { unit: "controls" }); put(body, charts.donut(outcomes, { unit: "controls", explain: explained, onPick: focus, hint: "Click: show these controls" })); }),
    h("div", { class: "kpis span-4", style: "grid-template-columns:repeat(2,minmax(0,1fr))" }, OUTCOME_ORDER.map((outcome) => kpi(outcome.replace(/_/g, " ").toLowerCase(), (data.outcomes || {})[outcome] || 0, { info: "value:" + outcome, tone: outcome === "SATISFIED" ? "good" : outcome === "NOT_SATISFIED" ? "bad" : outcome === "PARTIAL" ? "warnt" : "", drill: () => focus(outcome) })))));
  const FRAMEWORKS = ["OWASP LLM", "OWASP Agentic", "NIST AI RMF", "SOC 2", "ISO/IEC 27001", "GDPR", "EU AI Act"];
  const frameworkOf = (mapping) => FRAMEWORKS.find((name) => mapping.startsWith(name)) || mapping.split(" ")[0];
  const frameworks = {};
  for (const control of data.controls) for (const mapping of control.control.mappings) { const name = frameworkOf(mapping); frameworks[name] = frameworks[name] || {}; frameworks[name][control.outcome] = (frameworks[name][control.outcome] || 0) + 1; }
  main.append(zone("frameworks", "Frameworks", "Which requirements the evidence supports, per framework (mappings, not a certification).",
    panel("Outcomes per framework", { span: 7, scope: evaluated, info: { title: "Outcomes per framework", text: "For each framework a control maps to, how those controls stand. A control mapped to several requirements of one framework counts once per requirement." } }, async (body, big, ctx) => {
      const rows = Object.entries(frameworks).map(([name, parts]) => ({ label: name, key: name, parts })).sort((a, b) => Object.values(b.parts).reduce((x, y) => x + y, 0) - Object.values(a.parts).reduce((x, y) => x + y, 0));
      ctx.record("rows", rows);
      put(body, charts.stacked(rows, { keys: OUTCOME_ORDER, unit: "mappings", onPick: (row) => { state.q = row.key; focus(""); } }));
    }),
    panel("Requirements", { span: 5, flush: true, scope: evaluated }, async (body, big, ctx) => {
      const rows = data.controls.flatMap((control) => control.control.mappings.map((mapping) => ({ mapping, framework: frameworkOf(mapping), control: control.control.id, outcome: control.outcome })));
      ctx.record("rows", rows);
      put(body, dataTable([{ label: "Requirement", get: (row) => row.mapping }, { label: "Framework", get: (row) => row.framework, filter: true }, { label: "Control", get: (row) => row.control, render: (row) => h("code", {}, row.control) }, { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true }], rows, { sort: 0, dir: 1, pageSize: 12, noColumns: true, name: "foxx-requirements" }));
    })));
  controlsPanel = panel("Controls", { span: 12, scope: evaluated }, async (body, big, ctx) => {
    const list = data.controls.filter((control) => (!state.outcome || control.outcome === state.outcome) && (!state.q || JSON.stringify(control).toLowerCase().includes(state.q.toLowerCase())));
    ctx.record("items", OUTCOME_ORDER.map((outcome) => ({ value: outcome, count: list.filter((control) => control.outcome === outcome).length })).filter((item) => item.count), { unit: "controls" });
    const tools = h("div", { class: "table-tools" }, h("input", { type: "search", placeholder: "Search controls, evidence, frameworks…", value: state.q, "aria-label": "Search controls", oninput: (event) => { state.q = event.target.value; clearTimeout(controlsPanel.typing); controlsPanel.typing = setTimeout(() => { controlsPanel.refresh(); }, 400); } }),
      select("Outcome", state.outcome, [["", "Outcome: all"]].concat(OUTCOME_ORDER.map((outcome) => [outcome, outcome])), (value) => { state.outcome = value; controlsPanel.refresh(); }), segmented("View", state.view, [["cards", "Cards"], ["table", "Table"]], (value) => { state.view = value; controlsPanel.refresh(); }), h("span", { class: "count" }, list.length + " of " + data.controls.length + " controls"));
    const cards = h("div", { class: "controls-grid" }, list.length ? list.map((control) => h("article", { class: "control " + (TONES[control.outcome] || "") },
      h("header", {}, h("code", {}, control.control.id), chip(control.outcome), info("value:" + control.outcome)), h("h4", {}, control.control.objective),
      h("dl", {}, h("dt", {}, "Enforced by"), h("dd", {}, control.control.rule), h("dt", {}, "Verified by"), h("dd", {}, control.control.method)),
      control.evidence.length ? h("div", { class: "evidence" }, h("b", {}, "Evidence"), h("ul", {}, control.evidence.map((item) => h("li", {}, item)))) : null,
      control.gaps.length ? h("div", { class: "gaps" }, h("b", {}, "Gaps to fix"), h("ul", {}, control.gaps.map((item) => h("li", {}, item)))) : null,
      control.limitations && control.limitations.length ? h("div", { class: "limits" }, h("b", {}, "Limitations ", info("", { title: "Limitations", text: "What the method cannot prove. They are disclosed, not counted against the control." })), h("ul", {}, control.limitations.map((item) => h("li", {}, item)))) : null,
      h("div", { class: "fix" }, h("b", {}, "How to satisfy: "), control.control.fix),
      h("div", { class: "flows" }, h("b", {}, "Evidence flows: "), control.control.flows.map((flow) => h("button", { type: "button", class: "tag link", title: meaning(flow) + " Click: open these events.", onclick: () => drill({ clauses: [is("flow", flow)] }) }, flow))),
      h("div", { class: "maps" }, control.control.mappings.map((mapping) => h("span", { class: "chip" }, mapping))))) : h("div", { class: "empty" }, "No control matches."));
    const table = dataTable([{ label: "Control", get: (row) => row.control.id, render: (row) => h("code", {}, row.control.id) }, { label: "Objective", get: (row) => row.control.objective }, { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true }, { label: "Evidence", get: (row) => row.evidence.length, num: true }, { label: "Gaps", get: (row) => row.gaps.length, num: true }, { label: "Flows", get: (row) => row.control.flows.join(" ") }, { label: "Frameworks", get: (row) => row.control.mappings.join("; ") }], list, { noTools: true, sort: 0, dir: 1, pageSize: 30 });
    put(body, tools, state.view === "cards" ? cards : table);
  });
  main.append(zone("controls", "Controls", "Each control with its evidence, gaps, limitations, how to satisfy it, and the flows its evidence comes from.", controlsPanel));
  const flowsOf = [...new Set(data.controls.flatMap((control) => control.control.flows))];
  main.append(zone("evidence", "Evidence", "How much evidence each control's flows produced, in range and filter.",
    panel("Evidence per control and flow", { span: 12, info: { title: "Evidence per control and flow", text: "Each cell counts the events of one flow (column) that a control (row) is evaluated from, in range and filter. An empty row means its control had nothing to evaluate in this range." } }, async (body, big, ctx) => {
      const counts = await api("/api/facets", filterParams({ clauses: [{ path: "flow", op: "is", values: flowsOf }] }));
      const byFlow = Object.fromEntries(((counts.facets || {}).flow || []).map((item) => [item.value, item.count]));
      const matrix = { rows: data.controls.map((control) => ({ key: control.control.id, label: control.control.id + " · " + control.outcome, total: control.control.flows.reduce((sum, flow) => sum + (byFlow[flow] || 0), 0) })), cols: flowsOf.sort().map((flow) => ({ key: flow, label: flow, total: byFlow[flow] || 0 })), rows_by: "control", cols_by: "flow" };
      matrix.cells = data.controls.map((control) => matrix.cols.map((col) => (control.control.flows.includes(col.key) ? byFlow[col.key] || 0 : 0)));
      matrix.max = Math.max(0, ...matrix.cells.flat());
      ctx.record("matrix", matrix, { flows: flowsOf.length });
      put(body, charts.heatmap(matrix, { unit: "events", empty: "No evidence events in this range.", onPick: (control, flow) => drill({ clauses: [is("flow", flow)] }) }));
    }),
    seriesPanel("Evidence events over time", { span: 12, by: "flow", clauses: [{ path: "flow", op: "is", values: flowsOf }], height: 200, info: { title: "Evidence events over time", text: "The events of every flow a control is evaluated from, by flow." } })));
  const gapRows = data.controls.flatMap((control) => control.gaps.map((text) => ({ kind: "GAP", control: control.control.id, outcome: control.outcome, text, fix: control.control.fix })).concat((control.limitations || []).map((text) => ({ kind: "LIMITATION", control: control.control.id, outcome: control.outcome, text, fix: "Inherent to the method; disclosed, never counted against the control." }))));
  main.append(zone("gaps", "Gaps and limitations", "Every gap to fix and every limitation of the method, in one list.",
    panel("Gaps and limitations", { span: 12, flush: true, scope: evaluated }, async (body, big, ctx) => { ctx.record("rows", gapRows, { summary: gapRows.filter((row) => row.kind === "GAP").length + " gaps can be fixed; " + gapRows.filter((row) => row.kind === "LIMITATION").length + " limitations are disclosed." }); put(body, dataTable([{ label: "Kind", get: (row) => row.kind, render: (row) => chip(row.kind === "GAP" ? "PARTIAL" : "NOT_APPLICABLE", row.kind), filter: true }, { label: "Control", get: (row) => row.control, render: (row) => h("code", {}, row.control), filter: true }, { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome) }, { label: "What", get: (row) => row.text }, { label: "How to fix", get: (row) => row.fix, render: (row) => h("span", { class: "muted" }, row.fix) }], gapRows, { sort: 0, dir: 1, empty: "No gaps and no limitations." })); })));
  main.append(zone("inferred", "Inferred observability", "What hooks can only infer is shown as observability, not as a control.", (data.inferred || []).map((item) => panel("Network destinations", { span: 12, info: { title: "Network destinations", text: item.meaning }, scope: { kind: "custom", text: "Destinations inferred from every NETWORK_DESTINATION_INFERRED event the repository holds (not narrowed by the time range or filter)." } }, async (body, big, ctx) => { const items = item.values.map((entry) => ({ value: entry.value, count: entry.count })); ctx.record("items", items, { unit: "inferences" }); put(body, h("p", { class: "muted" }, item.meaning), charts.toplist(items, { onPick: (value) => drill({ clauses: [is("event_name", "NETWORK_DESTINATION_INFERRED"), is("resource.resource_ids", value)] }), empty: "No destinations inferred in the recorded events." })); }))));
  main.append(zone("unobservable", "Not observable", "Telemetry hooks cannot collect; no control relies on it.", panel("Not observable from hooks", { span: 12, noExpand: true, scope: { kind: "custom", text: "A fixed list: what Crane's hooks cannot measure." } }, async (body) => put(body, h("div", {}, data.unsupported_telemetry.map((name) => chip(name))), h("p", { class: "muted" }, "Hooks see each tool call before and after it runs, not the process underneath: bytes sent, CPU time, and memory are outside their view. Model tokens are read from the agent's transcript when it reports them (Claude Code).")))));
}

// ---------------------------------------------------------------- organization

async function repositories(main) {
  if (!scope.account) { main.append(h("div", { class: "notice" }, "No account selected.")); return; }
  const data = await api("/api/accounts/" + encodeURIComponent(scope.account));
  const choose = (row) => { scope.repository = row.repository; scope.member = ""; saveScope(); go("#/overview"); };
  const all = { kind: "state", what: "Every repository of the account" };
  main.append(hero(scope.account, data.repositories.length + " repositories with Crane", [["Events", compact(data.repositories.reduce((sum, row) => sum + row.events, 0))], ["Denied", compact(data.repositories.reduce((sum, row) => sum + row.denied, 0)), "eval:denied"], ["Alerts", compact(data.repositories.reduce((sum, row) => sum + row.alerts, 0))]]));
  const byName = (value) => data.repositories.find((item) => item.remote.endsWith(value));
  main.append(h("div", { class: "grid" }, panel("Events by repository", { span: 6, scope: all }, async (body, big, ctx) => { const items = data.repositories.map((row) => ({ value: row.remote.replace(/^github\.com\//, ""), count: row.events })).sort((a, b) => b.count - a.count); ctx.record("items", items, {}); put(body, charts.toplist(items, { onPick: (value) => { const row = byName(value); if (row) choose(row); }, hint: "Open this repository" })); }),
    panel("Denials by repository", { span: 6, scope: all }, async (body, big, ctx) => { const items = data.repositories.filter((row) => row.denied).map((row) => ({ value: row.remote.replace(/^github\.com\//, ""), count: row.denied })).sort((a, b) => b.count - a.count); ctx.record("items", items, {}); put(body, charts.toplist(items, { onPick: (value) => { const row = byName(value); if (row) choose(row); }, empty: "No denials.", hint: "Open this repository", color: Foxx.STATUS.critical })); })));
  main.append(h("div", { class: "grid" }, panel("Repositories", { span: 12, flush: true, scope: all }, async (body, big, ctx) => { ctx.record("rows", data.repositories); put(body, dataTable([
    { label: "Repository", get: (row) => row.remote, render: (row) => h("a", { href: "#/overview", onclick: (event) => { event.preventDefault(); choose(row); } }, row.remote.replace(/^github\.com\//, "")) },
    { label: "Members", get: (row) => row.members, num: true }, { label: "Sessions", get: (row) => row.sessions, num: true }, { label: "Events", get: (row) => row.events, num: true, bar: true },
    { label: "Denied", get: (row) => row.denied, num: true }, { label: "Confirmed bypasses", get: (row) => row.bypasses_confirmed, render: (row) => (row.bypasses_confirmed ? chip("BYPASS_CONFIRMED", row.bypasses_confirmed + " confirmed") : "0") },
    { label: "Alerts", get: (row) => row.alerts, num: true }, { label: "Last activity", get: (row) => row.last_activity, render: (row) => h("span", { class: "mono" }, dt(row.last_activity)) },
  ], data.repositories, { sort: 7, onRow: choose, placeholder: "Search repositories…", empty: "A repository appears once a member's Crane uploads from a clone of a GitHub repository owned by this account.", name: "foxx-repositories" })); })));
}

async function settings(main) {
  if (needRepository(main)) return;
  const [health, repo, integrations, memberData, kept] = await Promise.all([api("/api/health"), api("/api/repo"), api("/api/integrations"), api("/api/members"), retention()]);
  const state = { kind: "state", what: "This setting" };
  const input = h("input", { type: "text", placeholder: "12-character code", "aria-label": "approval code", maxlength: "12" });
  const approve = (action) => () => { const code = input.value.trim().toUpperCase(); if (code) location.href = "/auth/approve?action=" + action + "&code=" + encodeURIComponent(code); };
  main.append(zone("approvals", "Approvals", "",
    panel("Approvals", { span: 6, noExpand: true, scope: { kind: "custom", text: "An action: nothing is shown or filtered." }, info: { title: "Approvals", text: "'crane agent link' prints a code that links a machine's upload key to your GitHub account (you need access to the repository); 'crane admin register' prints one that confirms the owner key (you must administer the repository). GitHub checks your role once; Foxx keeps no GitHub token." } }, async (body) => put(body, h("div", { class: "table-tools plain" }, input, h("button", { class: "btn primary", type: "button", onclick: approve("link") }, "Link machine"), h("button", { class: "btn", type: "button", onclick: approve("owner") }, "Confirm owner key")))),
    panel("Repository connection", { span: 6, info: "flow:F21", noExpand: true, scope: { kind: "report", what: "The repository connection" } }, async (body) => put(body, repo.connection ? h("p", {}, repo.connection.repository + " via " + repo.connection.method, h("br"), h("span", { class: "muted" }, "privileges: " + fmt(repo.connection.privileges))) : h("p", { class: "muted" }, repo.remote + " (connection not reported yet)"),
      repo.owner && repo.owner.owner_key ? h("p", {}, "Owner key ", h("code", {}, repo.owner.owner_key.slice(0, 16) + "…"), " confirmed by @" + repo.owner.confirmed_by + " (GitHub admin) at " + dt(repo.owner.confirmed_at)) : h("p", { class: "muted" }, "No GitHub-confirmed owner key yet: the owner runs 'crane admin register' and confirms it here.")))));
  const hidden = memberData.members.filter((row) => !row.selected && !row.linked_by).length;
  main.append(zone("members", "Members", hidden && !scope.member ? hidden + " machine(s) nobody linked to a GitHub account are hidden; choose one in Member to see its activity." : "", panel("Members", { span: 12, flush: true, info: "flow:F02", scope: Object.assign({}, state, { what: "Members and their machines" }) }, async (body, big, ctx) => { ctx.record("rows", memberData.members); put(body, dataTable([
    { label: "Member", get: (row) => row.member.actor, render: (row) => member(row.member) },
    { label: "Linked", get: (row) => (row.linked_by ? "LINKED" : "UNLINKED"), render: (row) => (row.linked_by ? chip("LINKED", "linked by @" + row.linked_by) : chip("UNLINKED", "run 'crane agent link' on that machine")), filter: true },
    { label: "GitHub", get: (row) => (row.linked_by ? "@" + row.linked_by : "") }, { label: "Shown", get: (row) => (row.selected ? "yes" : "no"), filter: true },
    { label: "Remote", get: (row) => (row.verified ? "verified" : "missing"), render: (row) => chip(row.verified ? "verified" : "missing") },
    { label: "Events", get: (row) => row.events, num: true, bar: true }, { label: "Sessions", get: (row) => row.sessions, num: true },
    { label: "Last upload", get: (row) => row.last_upload, render: (row) => h("span", { class: "mono" }, dt(row.last_upload)) }, { label: "Crane", get: (row) => row.crane_version, filter: true },
  ], memberData.members, { sort: 5, name: "foxx-members" })); })));
  main.append(zone("health-zone", "System health", "", h("div", { class: "kpis span-12" },
    kpi("Crane", health.reported_crane_version || health.crane_version, { format: String, note: "schema v" + health.schema_version + ", events v" + health.spec_version }),
    kpi("Registry generation", health.registry_generation, { format: fmt }), kpi("Events", health.events, { note: "chains " + health.event_chain.status + (health.truncated ? " (most recent shown)" : "") }),
    kpi("Specification events", health.spec_events), kpi("Ledger", health.ledger_entries, { note: health.ledger_reconciles ? "reconciles" : "DOES NOT reconcile", tone: health.ledger_reconciles ? "" : "bad" }), kpi("Mode", health.mode, { format: String })),
    panel("Event chains by member", { span: 12, flush: true, info: "flow:F09", scope: Object.assign({}, state, { what: "Each member's event chain" }) }, async (body, big, ctx) => { ctx.record("rows", health.event_chain.members); put(body, dataTable([{ label: "Member", render: (row) => member(row.member) }, { label: "Chain", get: (row) => row.event_chain.status, render: (row) => chip(row.event_chain.status), filter: true }, { label: "Events", get: (row) => row.event_chain.length, num: true }, { label: "Expired before", get: (row) => row.event_chain.expired_before, render: (row) => (row.event_chain.expired_before ? "#" + row.event_chain.expired_before + " (expired, retention)" : "—") }, { label: "Ledger", get: (row) => (row.ledger_reconciles ? "reconciles" : "FAIL"), render: (row) => (row.ledger_reconciles ? "reconciles" : chip("FAIL")) }], health.event_chain.members, { noTools: true })); })));
  main.append(zone("retention", "Data retention", "How long MongoDB keeps the repository's records. Telemetry expires after the retention period; records in their last 7 days are stale data.", ...retentionPanels(kept)));
  main.append(zone("integrations", "Integrations", "Non-secret settings; secrets stay on the member's machine.", panel("Integrations", { span: 12, flush: true, info: "flow:F22", scope: { kind: "report", what: "The members' integrations" } }, async (body, big, ctx) => { ctx.record("rows", integrations.integrations); put(body, dataTable([{ label: "Integration", get: (row) => row.name }, { label: "Member", render: (row) => member(row.member) }, { label: "Settings", get: (row) => fmt(row.settings) }, { label: "Secrets", get: (row) => row.secret_fields.length, render: (row) => row.secret_fields.length + " on the member's machine" }, { label: "Configured", get: (row) => row.configured_at, render: (row) => h("span", { class: "mono" }, dt(row.configured_at * 1000)) }], integrations.integrations, { noTools: true, empty: "No integrations configured." })); })));
  const signIns = panel("Your sign-ins", { span: 12, flush: true, info: "flow:F00", noExpand: true, scope: { kind: "state", what: "Your own sign-ins" } }, async (body) => put(body, h("div", { class: "pager left" }, h("button", { class: "btn", type: "button", onclick: async () => { const data = await api("/api/me/sign-ins"); put(signIns.body, eventsTable(data.events, { pageSize: 10 })); } }, "Show my recent sign-ins"))));
  main.append(zone("signins", "Your sign-ins", "", signIns), h("p", { class: "muted" }, "This dashboard only reads, except approvals and credit grants, which GitHub authorizes. Policies, checkpoints, budgets, alert rules, and integrations are changed with the Crane CLI by a person."));
}
/** The data retention panels: the policy, and each kind of record with how much is stale */
function retentionPanels(kept) {
  if (!kept) return [h("div", { class: "notice span-12" }, "The retention policy could not be read.")];
  const expires = kept.mode === "EXPIRES";
  const scopeText = { kind: "custom", text: "Every record of this repository the service holds, whatever its age (the time range and filter do not apply)." };
  return [
    h("div", { class: "kpis span-12" },
      kpi("Retention", expires ? fmt(Math.round(kept.days * 100) / 100) + " days" : "kept for ever", { format: String, tone: expires ? "brand" : "good", info: "value:" + kept.mode, note: expires ? "then MongoDB deletes them" : "CRANE_MONGODB_RETENTION_DAYS=persist" }),
      kpi("Stale after", expires ? fmt(Math.round(kept.stale_after_days * 100) / 100) + " days" : "never", { format: String, info: "value:STALE_DATA" }),
      kpi("Stale records", kept.collections.reduce((sum, item) => sum + item.stale, 0), { tone: kept.collections.some((item) => item.stale) ? "warnt" : "", note: "deleted within 7 days" }),
      kpi("Deleted within a day", kept.collections.reduce((sum, item) => sum + item.expiring_24h, 0), { tone: kept.collections.some((item) => item.expiring_24h) ? "bad" : "" })),
    panel("Records by kind", { span: 8, flush: true, scope: scopeText, info: { title: "Records by kind", text: "Each kind of record that expires: how many the repository has, the oldest and newest, how many are stale (in their last 7 days), and how many are deleted within a day. Derived events (credits, alerts on the dashboard) are rebuilt from other records and are not stored." } }, async (body, big, ctx) => {
      ctx.record("rows", kept.collections, { summary: expires ? "Records expire " + kept.days + " days after their time." : "Every record is kept." });
      put(body, dataTable([
        { label: "Records", get: (row) => row.label },
        { label: "Held", get: (row) => row.documents, num: true, bar: true },
        { label: "Stale", get: (row) => row.stale, render: (row) => (row.stale ? h("span", {}, chip("STALE_DATA"), " " + fmt(row.stale)) : "0"), num: true },
        { label: "Within a day", get: (row) => row.expiring_24h, num: true },
        { label: "Age", get: (row) => row.documents, render: (row) => { const fresh = row.documents - row.stale; return h("div", { class: "retention-bar", title: fmt(fresh) + " current, " + fmt(row.stale) + " stale" }, h("i", { style: "width:" + (row.documents ? (fresh / row.documents) * 100 : 0) + "%;background:" + Foxx.STATUS.good }), h("i", { style: "width:" + (row.documents ? (row.stale / row.documents) * 100 : 0) + "%;background:" + Foxx.STATUS.warning })); } },
        { label: "Oldest", get: (row) => row.oldest, render: (row) => h("span", { class: "mono" }, dt(row.oldest)) },
        { label: "Deleted around", get: (row) => (row.oldest && expires ? row.oldest + kept.days * 86400000 : null), render: (row) => h("span", { class: "mono" }, row.oldest && expires ? dt(row.oldest + kept.days * 86400000) : "never") },
        { label: "Newest", get: (row) => row.newest, render: (row) => h("span", { class: "mono" }, dt(row.newest)) },
      ], kept.collections, { noTools: true, noColumns: true }));
    }),
    panel("Kept for ever", { span: 4, noExpand: true, scope: scopeText, info: { title: "Kept for ever", text: "State and history never expire: they are small, and evidence, balances, and the audit trail depend on them." } }, async (body) => put(body, h("div", {}, kept.kept.map((name) => h("span", { class: "tag" }, name))), h("p", { class: "muted" }, "When the service moves to a larger cluster, start it with ", h("code", {}, "CRANE_MONGODB_RETENTION_DAYS=persist"), ": it removes every expiry already set and keeps all new records."))),
  ];
}

Object.assign(window.FOXX_PAGES, { alerting, credits, audit, governance: governanceView, compliance, repositories, settings });

// ---------------------------------------------------------------- start

document.getElementById("toggle").addEventListener("click", () => {
  const app = document.getElementById("app");
  if (window.innerWidth <= 760) app.classList.toggle("open"); else app.classList.toggle("collapsed");
});
document.getElementById("details-toggle").addEventListener("click", () => { if (document.getElementById("details").hidden) details.guide(); else details.close(); });
document.getElementById("theme").addEventListener("click", () => {
  const order = ["system", "light", "dark"];
  const next = order[(order.indexOf(storage.read("foxx-theme", "system")) + 1) % order.length];
  storage.save("foxx-theme", next, localStorage);
  applyTheme();
  render();
});
document.getElementById("live").addEventListener("click", (event) => {
  const button = event.currentTarget;
  if (live) { clearInterval(live); live = null; button.classList.remove("on"); button.setAttribute("aria-pressed", "false"); button.textContent = "● Live"; }
  else { live = setInterval(() => { if (!document.querySelector(".overlay, .menu, .palette") && document.getElementById("details").hidden) render(); }, 30000); button.classList.add("on"); button.setAttribute("aria-pressed", "true"); button.textContent = "● Live 30 s"; }
});
document.getElementById("nav").addEventListener("click", (event) => { if (event.target.closest("a.page-link")) document.getElementById("app").classList.remove("open"); });
document.getElementById("rail-filter").addEventListener("input", () => drawNav(currentPage()));
document.getElementById("search-open").addEventListener("click", openPalette);
document.addEventListener("keydown", (event) => {
  const typing = /^(input|select|textarea)$/i.test((event.target || {}).tagName || "");
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); openPalette(); }
  else if (event.key === "/" && !typing) { event.preventDefault(); openPalette(); }
  else if (event.key === "?" && !typing) { event.preventDefault(); details.guide(); }
  else if (event.key === "Escape") { details.close(); document.querySelectorAll(".menu").forEach((node) => node.remove()); }
});
if (window.matchMedia) window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => { if (storage.read("foxx-theme", "system") === "system") render(); });
window.addEventListener("hashchange", render);
render();
