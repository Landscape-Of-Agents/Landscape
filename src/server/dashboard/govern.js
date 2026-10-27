"use strict";
// The DataSea dashboard: the pages that operate and govern. Operate (alerting, credits), Govern (audit
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
    ], entities, { sort: 1, dir: 1, onRow: openEntity, name: "foxx-zones-flows", empty: "No zones or flows: add them in .crane/governance.yaml or from Landscape in the editor." })); }),
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
    panel("Approvals", { span: 6, noExpand: true, scope: { kind: "custom", text: "An action: nothing is shown or filtered." }, info: { title: "Approvals", text: "'crane agent link' prints a code that links a machine's upload key to your GitHub account (you need access to the repository); 'crane admin register' prints one that confirms the owner key (you must administer the repository). GitHub checks your role once; DataSea keeps no GitHub token." } }, async (body) => put(body, h("div", { class: "table-tools plain" }, input, h("button", { class: "btn primary", type: "button", onclick: approve("link") }, "Link machine"), h("button", { class: "btn", type: "button", onclick: approve("owner") }, "Confirm owner key")))),
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

// ---------------------------------------------------------------- bricks

Object.assign(TONES, { active: "ok", passed: "ok", draft: "info", not_run: "warn", deprecated: "warn", revoked: "crit", failed: "bad", blocked: "bad", inconclusive: "warn", enabled: "ok", disabled: "warn",
  IDLE: "info", RUNNING: "ok", WAITING_FOR_APPROVAL: "warn", PAUSED: "warn", REVOKED: "crit", FAILING: "bad", UNSUPPORTED: "warn", REVOCATION_DUE: "bad", PROPOSAL: "info", NOT_RUN: "warn",
  FIRST_ASSEMBLY: "info", BRICK_STORE_CHANGED: "warn", GOVERNANCE_CHANGED: "warn", CONTEXT_CHANGED: "info",
  enforced: "ok", checked: "info", delivered: "info", declared: "warn", paused: "warn" });

/** Starting configurations per type for the draft builder; Crane validates whatever is written */
const BRICK_TEMPLATES = {
  context: { files: [] }, identity: { agents: ["claude"] }, scope: { include: ["src/**"] }, permission: { allow: ["read", "write"] },
  constraint: { max_ceiling: "assisted", protected_paths: [] }, precondition: { checks: [{ kind: "file_exists", path: "README.md" }] },
  approval: { operations: ["write", "delete"], reason: "" }, isolation: { profile: "agent-hooks" }, budget: { max_tool_calls: 200, on_exhausted: "escalate" },
  action: { adapter: "git", operation: "git.commit" }, verification: { checks: [{ kind: "policy_tests_pass" }] }, provenance: { require: ["signed_state", "audit_chain"] },
  recovery: { on_failure: "escalate", max_retries: 0, reconcile_before_retry: true }, revocation: { triggers: ["verification_contradicted"] },
  monitoring: { signals: [{ source: "policy_tests", every_minutes: 60 }] }, intelligence: { analyses: ["denial_patterns"], advisory_only: true },
};

/** The draft builder: a fixed type, plain-language intent, Common Context, and the type's
 * configuration, written as the JSON 'crane brick create --file' imports. The dashboard holds no
 * signing key and does not validate: Crane does, on a member's machine. */
function brickBuilder(types) {
  const state = { type: (types[0] || {}).type || "context", name: "", intent: "", owner: "", departments: "", context: "", config: JSON.stringify(BRICK_TEMPLATES.context, null, 2) };
  const out = h("pre", { class: "mono", style: "white-space:pre-wrap;max-height:260px;overflow:auto" });
  const help = h("div", { class: "muted" });
  const draft = () => {
    let config;
    try { config = JSON.parse(state.config || "{}"); } catch (error) { return { error: "The configuration is not JSON: " + error.message }; }
    return { type: state.type, name: state.name, intent: state.intent, owner: state.owner || null, departments: state.departments.split(",").map((item) => item.trim()).filter(Boolean), config, common_context: { text: state.context, sensitivity: "internal" } };
  };
  const refresh = () => {
    const value = draft();
    const info = types.find((item) => item.type === state.type) || {};
    put(help, h("b", {}, info.label || state.type), " — ", info.purpose || "", h("div", {}, "Support today: ", chip(info.support), " ", info.runtime || ""));
    put(out, value.error ? value.error : JSON.stringify(value, null, 2));
  };
  const field = (label, key, area) => h("label", { class: "field", style: "display:block;margin:6px 0" }, h("div", { class: "muted" }, label),
    h(area ? "textarea" : "input", { rows: area ? 4 : undefined, style: "width:100%", value: state[key], oninput: (event) => { state[key] = event.target.value; refresh(); } }, area ? state[key] : null));
  const configArea = h("textarea", { rows: 6, style: "width:100%;font-family:monospace", oninput: (event) => { state.config = event.target.value; refresh(); } }, state.config);
  const download = h("button", { type: "button", class: "btn", onclick: () => {
    const value = draft();
    if (value.error || !value.name) { put(out, value.error || "Give the Brick a name first."); return; }
    const anchor = h("a", { href: URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: "application/json" })), download: "brick-draft.json" });
    anchor.click();
  } }, "Download draft");
  const node = h("div", {},
    h("p", {}, "Choose one of the fixed types, say in plain words what it solves, and add its reviewed context. The dashboard cannot sign or validate: download the draft and import it on a member's machine (or build it in Landscape for VS Code). Crane validates it there, and an admin activates it."),
    h("div", { style: "display:flex;gap:8px;flex-wrap:wrap" }, select("Type", state.type, types.map((item) => [item.type, item.label + " (" + item.support + ")"]), (value) => { state.type = value; state.config = JSON.stringify(BRICK_TEMPLATES[value] || {}, null, 2); configArea.value = state.config; refresh(); })),
    help, field("Name", "name"), field("What it solves (intent, in plain words)", "intent"), field("Owner", "owner"), field("Departments (comma separated)", "departments"),
    field("Common context (guidance every agent in an attached Flow or Zone receives; never authorization; no secrets)", "context", true),
    h("div", { class: "muted" }, "Type configuration (JSON, checked against the type's schema by Crane)"), configArea,
    h("div", { style: "margin:8px 0" }, download),
    h("div", { class: "muted" }, "Then on a member's machine: crane brick create --file brick-draft.json · crane brick test ID --target flow:ID · an admin runs crane brick activate ID --reason TEXT"), out);
  refresh();
  return node;
}

async function bricksView(main) {
  if (needRepository(main)) return;
  let data;
  try { data = await api("/api/bricks"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
  const report = { kind: "report", what: "The Bricks state", reported: data.reported_at };
  const active = data.bricks.filter((row) => row.state === "active").length;
  const drafts = data.bricks.filter((row) => row.state === "draft").length;
  const attention = data.monitors.filter((row) => !["OK", "NOT_RUN"].includes(row.result)).length;
  const pinIssues = data.attachments.filter((row) => (row.issues || []).length).length;
  main.append(hero("Bricks", ["Reusable modules of a fixed type, attached to Flows and Zones. As reported by", data.reported_by ? data.reported_by.actor : "a member", "at", dt(data.reported_at), "· store", data.trust].join(" "),
    [["Active versions", active], ["Drafts", drafts], ["Stacks", data.stacks.length], ["Attachments", data.attachments.length], ["Assigned agents", data.assignments.length], ["Monitors needing attention", attention]]));
  main.append(zone("brick-state", "State", "", h("div", { class: "kpis span-12" },
    kpi("Active versions", active, { tone: "brand" }), kpi("Drafts (grant nothing)", drafts),
    kpi("Revoked or deprecated", data.bricks.filter((row) => ["revoked", "deprecated"].includes(row.state)).length, { tone: data.bricks.some((row) => row.state === "revoked") ? "bad" : "" }),
    kpi("Pins Crane cannot honor", pinIssues, { tone: pinIssues ? "bad" : "good", note: "these fail closed" }),
    kpi("Snapshots delivered", data.context.deliveries), kpi("Unchanged, not resent", data.context.reused, { note: "provider caching not measured" }))));
  main.append(zone("brick-library", "Library", "Drafts and activated versions. A published version never changes; editing starts the next version.",
    panel("Bricks", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.bricks); put(body, dataTable([
      { label: "Brick", get: (row) => row.ref, render: (row) => h("code", {}, row.ref) },
      { label: "Name", get: (row) => row.name, render: (row) => h("b", {}, row.name) },
      { label: "Type", get: (row) => row.type, filter: true },
      { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
      { label: "Validation", get: (row) => row.validation, render: (row) => chip(row.validation), filter: true },
      { label: "Intent", get: (row) => row.intent },
      { label: "Owner", get: (row) => row.owner || "", filter: true },
      { label: "Departments", get: (row) => (row.departments || []).join(", "), filter: true },
      { label: "Context", get: (row) => row.common_context_bytes, render: (row) => (row.common_context_bytes ? row.common_context_bytes + " B" : "—"), num: true },
      { label: "Digest", get: (row) => row.digest, render: (row) => h("code", { class: "muted" }, String(row.digest || "").slice(0, 12)) },
      { label: "Activated by", get: (row) => row.activated_by || row.updated_by || "", render: (row) => (row.activated_by || row.updated_by || "—") + (row.self_reviewed ? " (self-reviewed)" : "") },
    ], data.bricks, { sort: 0, dir: 1, empty: "No Bricks yet. Build a draft below, or run 'crane brick create' on a member's machine.", name: "foxx-bricks" })); }),
    panel("Fixed types and what each does today", { span: 12, flush: true }, async (body, big, ctx) => { ctx.record("rows", data.types); put(body, dataTable([
      { label: "Type", get: (row) => row.label }, { label: "Support", get: (row) => row.support, render: (row) => chip(row.support), filter: true },
      { label: "Purpose", get: (row) => row.purpose }, { label: "What really happens", get: (row) => row.runtime }, { label: "Fields", get: (row) => (row.fields || []).join(", ") },
    ], data.types, { noColumns: true, pageSize: 16 })); })));
  main.append(zone("brick-attachments", "Attachments and Stacks", "Where each Brick applies. Current Context belongs to one attachment and changes nothing else; only its digest and size leave the machine.",
    panel("Attachments", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.attachments); put(body, dataTable([
      { label: "Attachment", get: (row) => row.attachment_id, render: (row) => h("code", {}, row.attachment_id) },
      { label: "Target", get: (row) => row.target, filter: true },
      { label: "Brick or Stack", get: (row) => row.subject, render: (row) => h("code", {}, row.subject) },
      { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
      { label: "Current context", get: (row) => row.current_context_bytes, render: (row) => (row.current_context_bytes ? row.current_context_bytes + " B · " + String(row.current_context_digest).slice(0, 10) : "—") },
      { label: "Restricts target", get: (row) => (row.target_restricted ? "yes" : "no"), filter: true },
      { label: "Issues", get: (row) => (row.issues || []).join("; "), render: (row) => ((row.issues || []).length ? h("span", {}, chip("FAIL"), " ", row.issues.join("; ")) : "—") },
      { label: "Attached by", get: (row) => row.attached_by, filter: true },
    ], data.attachments, { empty: "Nothing attached yet ('crane brick attach ID@N --target flow:ID').", name: "foxx-brick-attachments" })); }),
    panel("Stacks", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.stacks); put(body, dataTable([
      { label: "Stack", get: (row) => row.ref, render: (row) => h("code", {}, row.ref) }, { label: "Name", get: (row) => row.name }, { label: "Purpose", get: (row) => row.purpose },
      { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true }, { label: "Members", get: (row) => row.members.join(", ") },
    ], data.stacks, { noColumns: true, empty: "No Stacks. A Stack combines exact Brick versions; it is never a new type." })); })));
  main.append(zone("brick-agents", "Agents and monitors", "Assignment status (signed) and runtime status (derived on the reporting machine) are separate. Idle agents use no inference: monitors are deterministic.",
    panel("Assigned agents", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.assignments); put(body, dataTable([
      { label: "Agent", get: (row) => row.agent, render: (row) => h("b", {}, row.agent) },
      { label: "Assignment", get: (row) => row.assignment_status, render: (row) => chip(row.assignment_status), filter: true },
      { label: "Runtime", get: (row) => row.runtime_status, render: (row) => chip(row.runtime_status), filter: true },
      { label: "Targets", get: (row) => (row.targets || []).join(", ") },
      { label: "Last seen", get: (row) => row.last_seen_at, render: (row) => h("span", { class: "mono" }, row.last_seen_at ? dt(row.last_seen_at) : "—") },
      { label: "Problems", get: (row) => (row.problems || []).join("; ") },
      { label: "Inference", get: (row) => (row.inference ? row.inference.status : ""), render: () => h("span", { class: "muted" }, "not measured") },
    ], data.assignments, { empty: "No agent is assigned ('crane brick assign claude --targets flow:ID').", name: "foxx-brick-agents" })); }),
    panel("Monitors", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.monitors); put(body, dataTable([
      { label: "Target", get: (row) => row.target, filter: true }, { label: "Brick", get: (row) => row.brick, render: (row) => h("code", {}, row.brick) },
      { label: "Kind", get: (row) => row.monitor, filter: true }, { label: "Source", get: (row) => row.source, filter: true },
      { label: "Result", get: (row) => row.result, render: (row) => chip(row.result), filter: true }, { label: "Detail", get: (row) => row.detail || "" },
      { label: "Every", get: (row) => row.every_minutes, render: (row) => row.every_minutes + " min", num: true },
      { label: "Last run", get: (row) => row.last_run, render: (row) => h("span", { class: "mono" }, row.last_run ? dt(row.last_run) : "—") },
    ], data.monitors, { empty: "No monitoring, revocation, or intelligence Brick is attached.", name: "foxx-brick-monitors" })); })));
  main.append(zone("brick-context", "Context snapshots", "What agents received: each snapshot's targets, digest, size, and why it was last refreshed. Unchanged snapshots are referenced, not resent.",
    panel("Snapshots", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { const rows = data.context.snapshots || []; ctx.record("rows", rows); put(body, dataTable([
      { label: "Targets", get: (row) => (row.targets || []).join(", ") },
      { label: "Snapshot", get: (row) => row.digest, render: (row) => h("code", {}, String(row.digest).slice(0, 12)) },
      { label: "Bytes", get: (row) => row.bytes, num: true }, { label: "Bricks", get: (row) => (row.bricks || []).join(", ") },
      { label: "Refreshed because", get: (row) => row.refresh_reason, render: (row) => chip(row.refresh_reason), filter: true },
      { label: "Assembled", get: (row) => row.assembled_at, render: (row) => h("span", { class: "mono" }, dt(row.assembled_at)) },
      { label: "Conflicts", get: (row) => row.conflicts, num: true },
    ], rows, { empty: "No snapshot was delivered yet (an agent must touch an attached Flow or Zone).", name: "foxx-brick-snapshots" })); })));
  main.append(zone("brick-history", "History and events", "Signed changes from the audit trail, and the F37 events in the stream.",
    panel("Signed Brick changes", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.history); put(body, dataTable([
      { label: "#", get: (row) => row.seq, num: true }, { label: "Action", get: (row) => row.action, filter: true }, { label: "Actor", get: (row) => row.actor, filter: true },
      { label: "Detail", get: (row) => JSON.stringify(row.detail), render: (row) => h("code", {}, JSON.stringify(row.detail)) },
    ], data.history, { sort: 0, dir: -1, noColumns: true, empty: "No signed Brick change yet." })); }),
    seriesPanel("Brick events over time", { span: 8, by: "event", clauses: [{ path: "flow", op: "is", values: ["F37"] }], info: { title: "Brick events", text: "F37: drafts, validation, activation, attachment, context delivery (MISS sent, HIT referenced), assignments, and monitor runs." } }),
    breakdownPanel("By event", { span: 4, by: "event", clauses: [{ path: "flow", op: "is", values: ["F37"] }] })));
  main.append(zone("brick-build", "Build a draft", "", panel("Draft builder", { span: 12 }, async (body) => put(body, brickBuilder(data.types || [])))));
}

// ---------------------------------------------------------------- resident agents

Object.assign(TONES, { ACTIVE_IDLE: "ok", INVESTIGATING: "info", EXECUTING: "info", AWAITING_APPROVAL: "warn", AWAITING_INPUT: "warn", WAITING_FOR_EVENT: "info", RECONCILIATION_REQUIRED: "bad", DEGRADED: "bad", VALIDATED: "info", QUEUED: "info", EXPIRED: "warn",
  COMPLETED: "ok", PROVEN: "ok", PARTIALLY_VERIFIED: "warn", CONTRADICTED: "bad", UNKNOWN: "warn", CANCELLED: "warn", SIMULATED: "info", INCONCLUSIVE: "warn", HIT: "ok", MISS: "info", INVALIDATED: "warn",
  ALLOW: "ok", REQUIRE_APPROVAL: "warn", DENY: "bad", UNAVAILABLE: "warn", SUCCEEDED: "ok", DENIED: "bad", SKIPPED: "warn", UNCERTAIN: "bad", DISPATCHING: "info", PENDING: "info", WAITING: "info",
  IDENTIFIED: "ok", AMBIGUOUS: "warn", INSUFFICIENT_EVIDENCE: "warn", SUPPORTED: "ok", read_only: "ok", internal: "info", external: "warn", reviewed: "ok", proposed: "info", rejected: "bad", retired: "warn", expired: "warn", validated: "info" });

/** One-line view of a cache outcome ({cache, changed}) */
function cacheText(cache) {
  if (!cache) return "—";
  return cache.cache + (cache.changed && cache.changed.length ? " (" + cache.changed.join(", ") + ")" : "");
}

/** The detail of one activation: trigger, context, investigation, plan with Crane's decisions,
 * approvals, usage, and the timeline. Everything is as reported; nothing here acts. */
function activationDetail(row) {
  if (!row) return h("div", { class: "muted" }, "Choose an activation above.");
  const investigation = row.investigation || {};
  const steps = (row.plan && row.plan.steps) || [];
  const section = (title, ...children) => h("div", { style: "margin:10px 0" }, h("h3", { style: "margin:4px 0" }, title), ...children);
  return h("div", {},
    h("div", { style: "display:flex;gap:8px;flex-wrap:wrap;align-items:center" }, h("code", {}, row.activation_id), chip(row.state), row.outcome ? chip(row.outcome.status) : null, h("span", { class: "muted" }, row.outcome ? row.outcome.reason : "not ended"), chip(row.mode)),
    section("Trigger", h("div", {}, h("code", {}, row.trigger.source + " " + row.trigger.kind), " · ", h("code", { class: "muted" }, row.trigger.id), " · occurred ", dt(row.trigger.occurred_at), " · received ", dt(row.trigger.received_at), " · from ", row.trigger.origin, row.later_triggers && row.later_triggers.length ? " · later: " + row.later_triggers.join(", ") : "")),
    section("Context supplied", row.context ? h("div", {},
      h("div", {}, "Pack ", h("code", {}, row.context.pack), " · cache ", chip((row.context.cache || {}).cache), " ", cacheText(row.context.cache), " · signed generation ", String((row.context.dependencies || {}).generation)),
      dataTable([{ label: "Kind", get: (item) => item.kind, filter: true }, { label: "Source", get: (item) => item.source, render: (item) => h("code", {}, item.source) }, { label: "Version", get: (item) => item.reference }, { label: "Bytes", get: (item) => item.bytes, num: true }], row.context.sections || [], { noColumns: true, pageSize: 12, empty: "No section." }),
      (row.context.missing || []).length ? h("div", { class: "notice" }, "Missing required context: ", row.context.missing.map((item) => item[0] + " — " + item[1]).join("; ")) : null) : h("div", { class: "muted" }, "Context was not loaded.")),
    section("Investigation", row.investigation ? h("div", {},
      h("div", {}, "Graph ", h("code", {}, investigation.rca || "none"), " ", chip(investigation.rca_state), " · conclusion ", chip(investigation.conclusion), investigation.cause ? h("span", {}, " · cause ", h("code", {}, investigation.cause)) : null, " · ", h("span", { class: "muted" }, investigation.reasoner)),
      dataTable([
        { label: "Candidate cause", get: (item) => item.node, render: (item) => h("span", {}, h("code", {}, item.node), " ", item.label) },
        { label: "Graph says", get: (item) => item.prior, render: (item) => chip(item.prior) },
        { label: "Evidence says", get: (item) => item.verdict, render: (item) => chip(item.verdict), filter: true },
        { label: "Checks", get: (item) => (item.observations || []).length, render: (item) => h("div", {}, (item.observations || []).map((observation) => h("div", {}, h("code", {}, observation.relation), " ", observation.check, " → ", chip(observation.result.result), observation.result.reason ? " " + observation.result.reason : ""))) },
      ], investigation.candidates || [], { noColumns: true, empty: "No candidate cause (no signal of the graph matched)." }),
      (investigation.gaps || []).length ? h("div", { class: "notice" }, "Gaps: ", investigation.gaps.join("; ")) : null,
      h("div", { class: "muted" }, "Consulted: ", (investigation.consulted || []).join(", ") || "—"),
      h("div", { class: "muted" }, "A supported cause is supported by this activation's checks; it is not a confirmed cause. Co-occurrence is never shown as cause.")) : h("div", { class: "muted" }, "Not investigated.")),
    section("Plan and Crane's decisions", row.plan ? h("div", {}, h("div", {}, "Plan digest ", h("code", {}, row.plan.digest), " (approvals bind to it)"),
      dataTable([
        { label: "Step", get: (item) => item.step, render: (item) => h("code", {}, item.step) },
        { label: "Type", get: (item) => item.type, filter: true },
        { label: "Effect", get: (item) => item.effect, render: (item) => chip(item.effect), filter: true },
        { label: "Depends on", get: (item) => (item.depends_on || []).join(", ") },
        { label: "Crane", get: (item) => (item.decision ? item.decision.decision : ""), render: (item) => (item.decision ? h("span", {}, chip(item.decision.decision), " ", h("span", { class: "muted" }, (item.decision.reasons || []).join(", "))) : "—") },
        { label: "State", get: (item) => item.state, render: (item) => chip(item.state), filter: true },
        { label: "Detail", get: (item) => item.detail || "" },
        { label: "Evidence", get: (item) => (item.evidence || []).join("; ") },
        { label: "External ref", get: (item) => item.external_ref || "", render: (item) => (item.external_ref ? h("code", {}, item.external_ref) : "—") },
        { label: "Basis", get: (item) => item.basis || "" },
      ], steps, { noColumns: true, empty: "No step." })) : h("div", { class: "muted" }, "No plan.")),
    section("Approvals", dataTable([{ label: "Step", get: (item) => item.step }, { label: "Decision", get: (item) => (item.approved ? "APPROVED" : "DENIED"), render: (item) => chip(item.approved ? "ALLOW" : "DENY") }, { label: "By", get: (item) => item.approver }, { label: "Digest", get: (item) => item.digest, render: (item) => h("code", {}, item.digest) }, { label: "Used", get: (item) => (item.used ? "yes" : "no") }, { label: "Expires", get: (item) => item.expires_at, render: (item) => dt(item.expires_at) }, { label: "Reason", get: (item) => item.reason }], row.approvals || [], { noColumns: true, empty: "No approval was given or needed." })),
    section("Usage", h("div", {}, "Model calls ", h("b", {}, String(row.usage.model_calls)), row.usage.model ? " (" + row.usage.model + ")" : " (deterministic reasoning)", " · tokens ", row.usage.tokens === null || row.usage.tokens === undefined ? h("span", { class: "muted" }, "unmeasured") : String(row.usage.tokens), " · deterministic checks ", String(row.usage.deterministic_checks), " · external calls ", String(row.usage.external_calls), " · worker time ", String(row.usage.duration_ms), " ms")),
    section("Timeline", dataTable([{ label: "At", get: (item) => item.at, render: (item) => h("span", { class: "mono" }, dt(item.at)) }, { label: "State", get: (item) => item.state, render: (item) => chip(item.state), filter: true }, { label: "What", get: (item) => item.code, render: (item) => h("code", {}, item.code) }, { label: "Detail", get: (item) => item.detail }], row.timeline || [], { noColumns: true, sort: 0, dir: 1, pageSize: 30 })));
}

/** Decide, in the browser, whether a typed event would match an assignment's subscriptions: the
 * same rule as resident/trigger.rs (exact kind or "prefix.*", equal attributes, target scope).
 * A preview only: the machine decides again when the event really arrives. */
function wouldMatch(assignment, event) {
  if (event.source === "timer") return { matched: false, reason: "timers are evaluated by the machine's tick" };
  const target = assignment.target;
  for (const subscription of assignment.subscriptions || []) {
    if (subscription.source !== event.source) continue;
    const kindOk = (subscription.kinds || []).some((pattern) => (pattern.endsWith(".*") ? event.kind.startsWith(pattern.slice(0, -1)) : pattern === event.kind));
    if (!kindOk) continue;
    const attributesOk = Object.entries(subscription.attributes || {}).every(([key, value]) => (event.attributes || {})[key] === value);
    if (!attributesOk) continue;
    const targets = event.targets || [];
    if (targets.includes(target) || (!targets.length && subscription.scope === "repository")) return { matched: true, reason: "subscription " + subscription.source + " " + subscription.kinds.join(",") };
  }
  return { matched: false, reason: "no subscription takes " + event.source + " " + event.kind + " for " + target };
}

/** The fixture builder: a canonical event file for 'crane resident test ID --event', with a
 * preview of which assignments it would wake. Nothing is sent anywhere. */
function residentFixture(data) {
  const supported = (data.catalog.triggers || []).filter((item) => item.supported).map((item) => item.source);
  const state = { source: supported.includes("ci") ? "ci" : supported[0], kind: "ci.failed", target: ((data.assignments || [])[0] || {}).target || "", attributes: "job=tests", key: "test-" + Date.now() };
  const out = h("pre", { class: "mono", style: "white-space:pre-wrap;max-height:220px;overflow:auto" });
  const matches = h("div", {});
  const event = () => ({ source: state.source, kind: state.kind, targets: state.target ? [state.target] : [], attributes: Object.fromEntries(state.attributes.split(",").map((pair) => pair.split("=").map((part) => part.trim())).filter((pair) => pair[0] && pair.length === 2)), idempotency_key: state.key });
  const refresh = () => {
    const value = event();
    put(out, JSON.stringify(value, null, 2));
    put(matches, dataTable([{ label: "Assignment", get: (row) => row.assignment_id }, { label: "Status", get: (row) => row.status, render: (row) => chip(row.status) }, { label: "Would match", get: (row) => (wouldMatch(row, value).matched ? "yes" : "no"), render: (row) => chip(wouldMatch(row, value).matched ? "ALLOW" : "SKIPPED") }, { label: "Why", get: (row) => wouldMatch(row, value).reason }], data.assignments || [], { noColumns: true, empty: "No assignment yet." }));
  };
  const input = (label, key) => h("label", { class: "field", style: "display:block;margin:6px 0" }, h("div", { class: "muted" }, label), h("input", { style: "width:100%", value: state[key], oninput: (e) => { state[key] = e.target.value; refresh(); } }));
  const download = h("button", { type: "button", class: "btn", onclick: () => { h("a", { href: URL.createObjectURL(new Blob([JSON.stringify(event(), null, 2)], { type: "application/json" })), download: "resident-event.json" }).click(); } }, "Download event file");
  const node = h("div", {},
    h("p", {}, "Build a typed test event. It is checked on a member's machine with 'crane resident test ID --event resident-event.json' (plan only: nothing is executed, no trigger is consumed). The preview below applies the same matching rule as Crane; the machine decides again."),
    select("Source", state.source, (data.catalog.triggers || []).map((item) => [item.source, item.source + (item.supported ? "" : " (unsupported: no connector)")]), (value) => { state.source = value; refresh(); }),
    input("Kind (dot-separated lowercase, such as ci.failed)", "kind"), input("Target (flow:ID or zone:ID; empty for repository-wide)", "target"), input("Attributes (key=value, comma separated; never secrets)", "attributes"), input("Idempotency key (a redelivery uses the same key)", "key"),
    h("div", { style: "margin:8px 0" }, download), out, h("h3", {}, "Would it wake an assignment?"), matches,
    h("div", { class: "muted" }, "Replay a recorded trigger instead: crane resident test ID --replay ACTIVATION."));
  refresh();
  return node;
}

/** The creation wizard: identity and goal, target, context preview, triggers, actions and
 * boundaries, RCA behavior, verification, and review. It writes the two draft files Crane imports
 * ('crane resident definition save', 'crane resident create'); the dashboard cannot sign, validate,
 * or activate. Defaults are least privilege: nothing auto-runs and nothing external is allowed until
 * a person ticks it. */
function residentWizard(data, bricks) {
  const crane = data.crane || { targets: [], signed_assignments: [], connectors: {} };
  const steps = data.catalog.steps || [];
  const triggers = data.catalog.triggers || [];
  const state = {
    id: "", name: "", purpose: "", owner: "", agent: ((crane.signed_assignments || [])[0] || {}).agent || "claude",
    target: ((crane.targets || []).find((row) => row.status === "active") || {}).target || "",
    sources: { ci: "ci.failed" }, actions: { READ_CHECK: true, VERIFY: true, ASK_USER: true, PROPOSE_RCA: true }, auto: { READ_CHECK: true, VERIFY: true },
    rca: "", lifetime: 14, cooldown: 10, perDay: 24, maxSteps: 8, retries: 1, approvalTtl: 1440, verification: true, missing: "block", bricks: "", remote: false, distinct: false,
  };
  const out = h("pre", { class: "mono", style: "white-space:pre-wrap;max-height:300px;overflow:auto" });
  const review = h("div", {});
  const contextBox = h("div", {});
  const slug = (text) => text.toLowerCase().replace(/[^a-z0-9_-]+/g, "-").replace(/^[^a-z]+/, "").slice(0, 60);
  const definition = () => ({
    definition_id: slug(state.id || state.name) || "resident", version: 1, name: state.name, purpose: state.purpose, owner: state.owner,
    bricks: state.bricks.split(",").map((item) => item.trim()).filter(Boolean),
    triggers: Object.keys(state.sources).filter((source) => state.sources[source] !== undefined),
    actions: Object.keys(state.actions).filter((type) => state.actions[type]).map((type) => ({ step_type: type, auto: !!state.auto[type] })),
    verification_required: state.verification, on_missing_context: state.missing, remote_decisions: state.remote, distinct_approver: state.distinct,
    limits: { max_activations_per_day: Number(state.perDay), max_steps: Number(state.maxSteps), max_retries: Number(state.retries), max_model_calls: 0, cooldown_minutes: Number(state.cooldown), step_timeout_seconds: 30, approval_ttl_minutes: Number(state.approvalTtl), wait_minutes: 1440 },
  });
  const assignment = () => {
    const def = definition();
    const rca = state.rca ? state.rca.split("@") : null;
    return {
      assignment_id: (slug(state.id || state.name) || "resident") + "-" + (state.target.split(":")[1] || "target"), definition_id: def.definition_id, definition_version: 1, agent: state.agent, target: state.target,
      rca: rca ? { rca_id: rca[0], version: Number(rca[1]) } : null,
      subscriptions: Object.entries(state.sources).filter(([source]) => source !== "timer").map(([source, kinds]) => ({ source, kinds: String(kinds).split(",").map((item) => item.trim()).filter(Boolean), attributes: {}, scope: source === "ci" ? "repository" : "target" })),
      timers: state.sources.timer !== undefined ? [{ timer_id: "scheduled", every_minutes: Math.max(5, Number(state.sources.timer) || 60) }] : [], lifetime_days: Number(state.lifetime),
    };
  };
  const refresh = () => {
    const def = definition();
    const asg = assignment();
    const risks = [];
    const signed = (crane.signed_assignments || []).some((row) => row.active && row.agent === state.agent && (row.targets || []).includes(state.target));
    if (!signed) risks.push(["bad", "No active signed Brick assignment puts " + state.agent + " on " + (state.target || "a target") + ": activation will be refused until an admin runs 'crane brick assign " + state.agent + " --targets " + (state.target || "flow:ID") + "'."]);
    const targetRow = (crane.targets || []).find((row) => row.target === state.target);
    if (targetRow && targetRow.status !== "active") risks.push(["bad", state.target + " is " + targetRow.status + ": unavailable until it is approved and active."]);
    if (targetRow && targetRow.ceiling === "observe") risks.push(["warn", state.target + " has the observe ceiling: external steps will be denied."]);
    if (targetRow && (targetRow.ceiling === "assisted" || targetRow.criticality === "restricted")) risks.push(["warn", state.target + " needs a person for every external step, whatever 'auto' says."]);
    for (const type of Object.keys(state.actions).filter((key) => state.actions[key])) {
      const info = steps.find((row) => row.type === type) || {};
      if (!info.available) risks.push(["warn", type + " is unavailable: " + (info.unavailable_reason || "no adapter") + ". It will never run or count as done."]);
      if (info.effect === "external") risks.push(["warn", type + " has external side effects" + (state.auto[type] ? " and is set to auto-run where Crane allows" : "; each run needs a person's approval") + "."]);
    }
    if (state.actions.NOTIFY_SLACK && !(crane.connectors || {}).slack) risks.push(["warn", "Slack is not configured on the reporting machine ('crane integrate slack'); NOTIFY_SLACK steps will be unavailable."]);
    if (!state.rca) risks.push(["info", "No RCA graph: every activation ends in a question or a proposal for review."]);
    for (const source of Object.keys(state.sources)) { const info = triggers.find((row) => row.source === source); if (info && !info.supported) risks.push(["bad", source + " has no connector and cannot be subscribed."]); }
    if (Number(state.lifetime) > 90 || Number(state.lifetime) < 1) risks.push(["bad", "The lifetime must be 1-90 days."]);
    put(review, h("div", {}, h("div", {}, "Triggers: ", asg.subscriptions.map((row) => row.source + " " + row.kinds.join(",")).concat(asg.timers.map((row) => "timer every " + row.every_minutes + " min")).join(" · ") || "none"),
      h("div", {}, "Actions: ", def.actions.map((row) => row.step_type + (row.auto ? " (auto)" : "")).join(", ") || "none"),
      h("div", {}, "Lifetime ", String(state.lifetime), " days · cooldown ", String(state.cooldown), " min · at most ", String(state.perDay), " activations a day · no model calls (no provider is configured)"),
      risks.map(([tone, text]) => h("div", { class: "notice " + tone }, text))));
    put(out, "# resident-definition.json\n" + JSON.stringify(def, null, 2) + "\n\n# resident-assignment.json\n" + JSON.stringify(asg, null, 2));
    const attachments = ((bricks && bricks.attachments) || []).filter((row) => row.target === state.target && row.state === "enabled");
    put(contextBox, h("div", {}, h("div", { class: "muted" }, "What an activation will receive for ", h("code", {}, state.target || "—"), " (sources and versions; text stays on the machine). Preview the full pack with 'crane resident context ID'."),
      dataTable([{ label: "Source", get: (row) => row.kind }, { label: "Reference", get: (row) => row.reference, render: (row) => h("code", {}, row.reference) }, { label: "Bytes", get: (row) => row.bytes, num: true }],
        [{ kind: "agent mission (this definition)", reference: "definition:" + def.definition_id + "@1", bytes: state.purpose.length }, { kind: "Flow or Zone context", reference: "governance.yaml (signed generation " + crane.generation + ")", bytes: null }]
          .concat(attachments.map((row) => ({ kind: "Brick Common + Current Context", reference: row.subject + " via " + row.attachment_id, bytes: row.current_context_bytes || 0 })))
          .concat(state.rca ? [{ kind: "RCA graph", reference: "rca:" + state.rca, bytes: null }] : []), { noColumns: true })));
  };
  const field = (label, key, type) => h("label", { class: "field", style: "display:block;margin:6px 0" }, h("div", { class: "muted" }, label),
    h(type === "area" ? "textarea" : "input", { rows: type === "area" ? 3 : undefined, type: type === "number" ? "number" : undefined, style: "width:100%", value: state[key], oninput: (e) => { state[key] = e.target.value; refresh(); } }, type === "area" ? state[key] : null));
  const step = (number, title, ...children) => h("details", { open: number <= 2, style: "margin:8px 0;border:1px solid var(--line, #8883);border-radius:8px;padding:6px 10px" }, h("summary", {}, h("b", {}, number + ". " + title)), ...children);
  const targetChoices = (crane.targets || []).map((row) => [row.target, row.target + " (" + row.status + ", " + row.ceiling + ", " + row.criticality + ")" + (row.status === "active" ? "" : " — unavailable until approved")]);
  const sourceRows = h("div", {}, triggers.map((row) => h("div", { style: "display:flex;gap:8px;align-items:center;margin:4px 0" },
    h("input", { type: "checkbox", disabled: !row.supported, checked: state.sources[row.source] !== undefined, onchange: (e) => { if (e.target.checked) state.sources[row.source] = row.source === "timer" ? "60" : row.source + ".*"; else delete state.sources[row.source]; refresh(); } }),
    h("code", { style: "min-width:140px" }, row.source),
    row.supported ? h("input", { placeholder: row.source === "timer" ? "every N minutes (at least 5)" : "kinds, comma separated (no bare *)", value: state.sources[row.source] || "", oninput: (e) => { if (state.sources[row.source] !== undefined) { state.sources[row.source] = e.target.value; refresh(); } } }) : null,
    h("span", { class: "muted" }, row.supported ? row.delivery : "unsupported: no connector"))));
  const actionRows = h("div", {}, steps.map((row) => h("div", { style: "display:flex;gap:8px;align-items:center;margin:4px 0" },
    h("input", { type: "checkbox", disabled: !row.available, checked: !!state.actions[row.type], onchange: (e) => { state.actions[row.type] = e.target.checked; refresh(); } }),
    h("code", { style: "min-width:170px" }, row.type), chip(row.effect),
    h("label", { class: "muted" }, h("input", { type: "checkbox", disabled: !row.available, checked: !!state.auto[row.type], onchange: (e) => { state.auto[row.type] = e.target.checked; refresh(); } }), " auto-run where Crane allows"),
    h("span", { class: "muted" }, row.available ? row.adapter : "unavailable: " + row.unavailable_reason))));
  const rcaChoices = [["", "None (activations end in a question or a proposal)"]].concat((data.rcas || []).filter((row) => !state.target || row.target === state.target).map((row) => [row.rca, row.rca + " (" + row.state + ")"]));
  const download = (name, value) => h("a", { href: URL.createObjectURL(new Blob([JSON.stringify(value, null, 2)], { type: "application/json" })), download: name }).click();
  const node = h("div", {},
    h("p", {}, "Build a resident agent step by step. The dashboard cannot sign, validate, or activate: download the two drafts and import them on a member's machine (or in Landscape for VS Code). Crane validates them there against the signed rules; an admin activates them. A test never grants authority."),
    step(1, "Identity and goal", field("Name", "name"), field("Identifier (letters, digits, - and _)", "id"), field("Mission: what it is responsible for and the outcome you want (delivered as context; never authority; no secrets)", "purpose", "area"), field("Owner or team", "owner")),
    step(2, "Assignment target", select("Target", state.target, targetChoices.length ? targetChoices : [["", "No Zone or Flow was reported"]], (value) => { state.target = value; refresh(); }), field("Agent of the signed Brick assignment (provider or agent id)", "agent"),
      h("div", { class: "muted" }, "Signed assignments: ", (crane.signed_assignments || []).map((row) => row.agent + " → " + (row.targets || []).join(", ") + (row.active ? "" : " (not active)")).join(" · ") || "none")),
    step(3, "Context", contextBox, field("Brick versions the definition expects on the target (ID@N, comma separated; a missing one is reported as missing context)", "bricks"), select("When required context is missing", state.missing, [["block", "Block the activation"], ["ask", "Ask a person"]], (value) => { state.missing = value; refresh(); })),
    step(4, "Triggers", h("div", { class: "muted" }, "Choose real sources only. Unsupported sources are listed so the gap is visible."), sourceRows),
    step(5, "Actions and boundaries", h("div", { class: "muted" }, "Least privilege: nothing auto-runs unless ticked, and Crane can still require a person (assisted ceilings, restricted zones, approval Bricks)."), actionRows,
      h("label", { style: "display:block;margin:6px 0" }, h("input", { type: "checkbox", checked: state.remote, onchange: (e) => { state.remote = e.target.checked; refresh(); } }), " Admins may approve and answer from the dashboard (GitHub-verified; off by default)"),
      h("label", { style: "display:block;margin:6px 0" }, h("input", { type: "checkbox", checked: state.distinct, onchange: (e) => { state.distinct = e.target.checked; refresh(); } }), " Separation of duties: whoever activates the assignment cannot approve its steps"),
      field("Lifetime (days, at most 90)", "lifetime", "number"), field("Cooldown between activations (minutes)", "cooldown", "number"), field("Activations per day (at most 200)", "perDay", "number"), field("Steps per plan (at most 20)", "maxSteps", "number"), field("Retries of a step that failed without effect (at most 2)", "retries", "number"), field("Approval lifetime (minutes)", "approvalTtl", "number")),
    step(6, "RCA behavior", select("RCA graph", state.rca, rcaChoices, (value) => { state.rca = value; refresh(); }), h("div", { class: "muted" }, "A reviewed graph's remediations may run (each step still decided by Crane); a proposed graph's are proposed to a person. Without evidence the agent asks or proposes; it never guesses.")),
    step(7, "Verification", h("label", {}, h("input", { type: "checkbox", checked: state.verification, onchange: (e) => { state.verification = e.target.checked; refresh(); } }), " Plans with an external step must verify their outcome"), h("div", { class: "muted" }, "Either way, an activation is COMPLETED only when verification evidence proves it; otherwise it ends UNKNOWN.")),
    step(8, "Review and validate", review, h("div", { style: "display:flex;gap:8px;margin:8px 0" }, h("button", { type: "button", class: "btn", onclick: () => download("resident-definition.json", definition()) }, "Download definition"), h("button", { type: "button", class: "btn", onclick: () => download("resident-assignment.json", assignment()) }, "Download assignment")),
      h("div", { class: "muted" }, "Then on a member's machine: crane resident definition save --file resident-definition.json · crane resident create --file resident-assignment.json --dry-run · crane resident validate ID · crane resident test ID --event resident-event.json · an admin: crane resident definition activate ID@1 --reason TEXT and crane resident activate ID --reason TEXT"), out));
  refresh();
  return node;
}

/** What each dashboard decision means, for the form */
const RESIDENT_ACTIONS = {
  approve: ["Approve step", "The step runs when the machine applies this, if Crane still allows it then."],
  deny: ["Deny step", "The step is denied; independent steps may continue."],
  answer: ["Answer question", "The activation resumes with your answer (secret-looking values are removed)."],
  cancel: ["Cancel activation", "Queued and waiting work stops; a started external call stays uncertain until reconciled."],
  pause: ["Pause assignment", "It takes no trigger until a signer resumes it on a machine."],
  revoke: ["Revoke assignment", "Final: it never acts again. A new assignment is the way back."],
};

/** Decide for a resident agent on the dashboard: GitHub confirms that you administer the
 * repository, the service keeps the decision for the machine that holds the activation, and that
 * machine applies it at its next sync with Crane's own checks. The dashboard can always make
 * things stricter (pause, revoke, cancel); approvals and answers count only when the definition
 * allows dashboard decisions, and only for the plan digest shown here. */
function residentDecide(action, row) {
  const [title, effect] = RESIDENT_ACTIONS[action];
  const reason = h("input", { type: "text", maxlength: "300", placeholder: "Why (recorded with the decision)", "aria-label": "Reason", required: true });
  const text = action === "answer" ? h("textarea", { rows: 4, maxlength: "2000", placeholder: "Your answer (never secrets)", "aria-label": "Answer", required: true }) : null;
  const error = h("div", { class: "error-text", role: "alert" });
  const facts = [["Machine", row.member ? (row.member.actor || row.installation) : row.installation], ["Assignment", row.assignment || row.assignment_id], ["Activation", row.activation || row.activation_id || "—"], ["Step", row.step || "—"], ["Plan", row.plan || "—"]];
  details.open(title, h("form", { class: "form", onsubmit: (event) => {
    event.preventDefault();
    if (!reason.value.trim()) { error.textContent = "Say why; it is recorded."; return; }
    if (text && !text.value.trim()) { error.textContent = "Write the answer."; return; }
    const query = new URLSearchParams({ repository: scope.repository, installation: row.installation || "", action, activation: row.activation || row.activation_id || "", assignment: row.assignment || row.assignment_id || "", step: row.step || "", plan: row.plan || "", text: text ? text.value.trim() : "", reason: reason.value.trim() });
    location.href = "/auth/resident?" + query.toString();
  } },
    h("div", { class: "kv" }, facts.map(([key, value]) => [h("div", {}, key), h("div", {}, h("code", {}, String(value)))])),
    h("p", {}, effect), text ? h("label", {}, "Answer", text) : null, h("label", {}, "Reason", reason), error,
    h("p", { class: "muted" }, "You will sign in with GitHub; only an admin or owner of the repository can decide, and anyone else is refused. The machine applies it at its next sync, with every check Crane makes there" + (["approve", "deny", "answer"].includes(action) ? "; it counts only if the definition allows dashboard decisions (remote_decisions) and the plan is still the one shown" : "") + ". Resuming and activating need a signer on a machine."),
    h("button", { class: "btn primary", type: "submit" }, "Continue with GitHub")), { kind: "Resident agents" });
  reason.focus();
}

async function residentsView(main) {
  if (needRepository(main)) return;
  let data;
  try { data = await api("/api/residents"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
  let bricks = null;
  try { bricks = await api("/api/bricks"); } catch (error) { bricks = null; }
  data.assignments = data.assignments || []; data.definitions = data.definitions || []; data.activations = data.activations || []; data.rcas = data.rcas || []; data.tests = data.tests || [];
  data.inbox = data.inbox || { approvals: [], questions: [], reconciliation: [], rca_proposals: [], assignments: [] };
  data.health = data.health || { counters: {}, ignored: {}, outcomes: {}, cache: {} };
  const report = { kind: "report", what: "The resident agents state", reported: data.reported_at };
  const counters = data.health.counters || {};
  const count = (status) => data.assignments.filter((row) => row.status === status).length;
  const pending = data.inbox.approvals.length + data.inbox.questions.length + data.inbox.reconciliation.length;
  main.append(hero("Resident agents", ["Agent assignments that stay responsible for a Flow, Zone, or Runbook and wake only on a trigger or timer. As reported by", data.reported_by ? data.reported_by.actor : "a member", "at", dt(data.reported_at)].join(" "),
    [["Assignments", data.assignments.length], ["Idle and healthy", count("ACTIVE_IDLE")], ["Waiting for people", pending], ["Degraded", count("DEGRADED")], ["Model calls while idle", counters.idle_model_calls || 0]]));
  main.append(zone("res-state", "State", "Assignment lifecycle (draft, validated, active, paused, expired, revoked) and runtime status (derived from activations and from Crane now) are kept apart.", h("div", { class: "kpis span-12" },
    kpi("Active and idle", count("ACTIVE_IDLE"), { tone: "good", note: "no model, no process" }), kpi("Investigating or executing", count("INVESTIGATING") + count("EXECUTING")),
    kpi("Awaiting approval", count("AWAITING_APPROVAL"), { tone: count("AWAITING_APPROVAL") ? "warn" : "" }), kpi("Degraded", count("DEGRADED"), { tone: count("DEGRADED") ? "bad" : "good", note: "Crane would not admit it now" }),
    kpi("Model calls while idle", counters.idle_model_calls || 0, { tone: counters.idle_model_calls ? "bad" : "good", note: "should stay 0" }), kpi("Queue depth", data.health.queue_depth || 0)),
    h("div", { class: "muted span-12" }, "Each member's machine keeps its own resident store; this page merges every machine's latest report: ", (data.members || []).map((row) => (row.member ? row.member.actor : row.installation) + " (" + row.assignments + " assignment(s), " + dt(row.reported_at) + ")").join(" · ") || "—")));
  main.append(zone("res-registry", "Registry", "Every assignment with its target, definition version, lifetime, triggers, connector support, latest activation, last verification, and most recent failure.",
    panel("Assignments", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.assignments); put(body, dataTable([
      { label: "Assignment", get: (row) => row.assignment_id, render: (row) => h("b", {}, row.assignment_id) },
      { label: "Status", get: (row) => row.status, render: (row) => chip(row.status), filter: true },
      { label: "Lifecycle", get: (row) => row.lifecycle, render: (row) => chip(row.lifecycle), filter: true },
      { label: "Target", get: (row) => row.target, filter: true },
      { label: "Definition", get: (row) => row.definition, render: (row) => h("code", {}, row.definition), filter: true },
      { label: "Agent", get: (row) => row.agent, filter: true },
      { label: "RCA", get: (row) => row.rca || "", render: (row) => (row.rca ? h("code", {}, row.rca) : "—") },
      { label: "Triggers", get: (row) => (row.connectors || []).map((item) => item.source).join(", "), render: (row) => h("div", {}, (row.connectors || []).map((item) => h("span", {}, chip(item.supported ? item.source : "UNSUPPORTED", item.delivery || "no connector"), " ")), (row.timers || []).map((timer) => h("span", { class: "muted" }, "timer " + timer.every_minutes + " min "))) },
      { label: "Health", get: (row) => (row.health ? row.health.decision : "unknown"), render: (row) => (row.health ? h("span", {}, chip(row.health.decision), " ", h("span", { class: "muted" }, (row.health.reasons || []).join(", "))) : h("span", { class: "muted" }, "unknown")) },
      { label: "Expires", get: (row) => row.expires_at, render: (row) => h("span", { class: "mono" }, row.expires_at ? dt(row.expires_at) : "—") },
      { label: "Last event", get: (row) => row.last_event_at, render: (row) => h("span", { class: "mono" }, row.last_event_at ? dt(row.last_event_at) : "—") },
      { label: "Latest activation", get: (row) => (row.latest_activation ? row.latest_activation.state : ""), render: (row) => (row.latest_activation ? h("span", {}, chip(row.latest_activation.state), " ", h("code", { class: "muted" }, row.latest_activation.id)) : "—") },
      { label: "Last verified", get: (row) => row.last_verified_at, render: (row) => h("span", { class: "mono" }, row.last_verified_at ? dt(row.last_verified_at) : "—") },
      { label: "Last failure", get: (row) => (row.last_failure ? row.last_failure.reason : ""), render: (row) => (row.last_failure ? h("span", {}, chip(row.last_failure.state), " ", row.last_failure.reason || "") : "—") },
      { label: "Model calls", get: (row) => row.model_calls, num: true },
      { label: "Machine", get: (row) => (row.member ? row.member.actor || row.installation : row.installation || ""), filter: true },
      { label: "Act", get: () => "", render: (row) => (["active", "paused", "validated", "draft", "expired"].includes(row.lifecycle) ? h("div", {}, row.lifecycle === "active" ? h("button", { type: "button", class: "btn", onclick: () => residentDecide("pause", row) }, "Pause…") : null, " ", h("button", { type: "button", class: "btn", onclick: () => residentDecide("revoke", row) }, "Revoke…")) : "—") },
    ], data.assignments, { empty: "No resident assignment yet. Build one below, or run 'crane resident create' on a member's machine.", name: "foxx-residents" })); }),
    panel("Definitions", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.definitions); put(body, dataTable([
      { label: "Definition", get: (row) => row.ref, render: (row) => h("code", {}, row.ref) }, { label: "Name", get: (row) => row.name }, { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
      { label: "Owner", get: (row) => row.owner || "", filter: true }, { label: "Triggers", get: (row) => (row.triggers || []).join(", ") },
      { label: "Actions", get: (row) => (row.actions || []).map((item) => item.step_type + (item.auto ? " (auto)" : "")).join(", ") },
      { label: "Limits", get: (row) => JSON.stringify(row.limits), render: (row) => h("span", { class: "muted" }, "cooldown " + row.limits.cooldown_minutes + " min · " + row.limits.max_activations_per_day + "/day · " + row.limits.max_steps + " steps") },
      { label: "Digest", get: (row) => row.digest, render: (row) => h("code", { class: "muted" }, String(row.digest).slice(0, 12)) },
      { label: "Activated", get: (row) => (row.self_reviewed ? "self-reviewed" : ""), render: (row) => (row.self_reviewed ? chip("self-reviewed") : "—") },
    ], data.definitions, { noColumns: true, empty: "No definition yet." })); }),
    panel("RCA graphs", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.rcas); put(body, dataTable([
      { label: "Graph", get: (row) => row.rca, render: (row) => h("code", {}, row.rca) }, { label: "Name", get: (row) => row.name }, { label: "Target", get: (row) => row.target, filter: true },
      { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true }, { label: "Origin", get: (row) => row.origin.by, filter: true },
      { label: "Nodes", get: (row) => row.nodes.length, render: (row) => h("div", {}, row.nodes.map((node) => h("div", {}, h("code", {}, node.id), " ", node.kind, " ", chip(node.status), " ", h("span", { class: "muted" }, node.basis)))) },
      { label: "Edges", get: (row) => row.edges.length, render: (row) => h("div", { class: "muted" }, row.edges.map((edge) => edge.from + " " + edge.relation + " " + edge.to).join("; ")) },
    ], data.rcas, { noColumns: true, empty: "No RCA graph ('crane resident rca save --file GRAPH.json')." })); })));
  const command = (text) => h("code", { style: "user-select:all" }, text);
  main.append(zone("res-inbox", "Review inbox", "What waits for a person. Decisions are made on a member's machine with a signing key and resume the same activation; every decision is audited with who, when, the digest, and why.",
    panel("Pending approvals", { span: 12, flush: true, scope: report }, async (body) => put(body, dataTable([
      { label: "Activation", get: (row) => row.activation, render: (row) => h("code", {}, row.activation) }, { label: "Assignment", get: (row) => row.assignment, filter: true }, { label: "Step", get: (row) => row.step + " " + row.type },
      { label: "What", get: (row) => row.summary || "", render: (row) => h("div", {}, row.summary || "", (row.proposed_steps || []).length ? h("div", { class: "muted" }, "proposes: " + row.proposed_steps.map((item) => item.step + " " + item.type).join(", ")) : null) },
      { label: "Why Crane asks", get: (row) => (row.reasons || []).join(", ") }, { label: "Plan", get: (row) => row.plan, render: (row) => h("code", { class: "muted" }, row.plan) },
      { label: "Decide", get: () => "", render: (row) => h("div", {}, h("button", { type: "button", class: "btn", onclick: () => residentDecide("approve", row) }, "Approve…"), " ", h("button", { type: "button", class: "btn", onclick: () => residentDecide("deny", row) }, "Deny…"), h("div", { class: "muted" }, "or on a machine: ", command("crane resident approve " + row.activation + " --step " + row.step + " --reason TEXT [--deny]"))) },
    ], data.inbox.approvals, { noColumns: true, empty: "Nothing waits for approval." }))),
    panel("Questions", { span: 6, flush: true, scope: report }, async (body) => put(body, dataTable([
      { label: "Activation", get: (row) => row.activation, render: (row) => h("code", {}, row.activation) }, { label: "Question", get: (row) => row.summary || row.detail },
      { label: "Deadline", get: (row) => row.deadline, render: (row) => (row.deadline ? dt(row.deadline) : "—") }, { label: "Answer", get: () => "", render: (row) => h("div", {}, h("button", { type: "button", class: "btn", onclick: () => residentDecide("answer", row) }, "Answer…"), h("div", { class: "muted" }, "or: ", command("crane resident answer " + row.activation + " --step " + row.step + " --text TEXT"))) },
    ], data.inbox.questions, { noColumns: true, empty: "No open question." }))),
    panel("Uncertain effects to reconcile", { span: 6, flush: true, scope: report }, async (body) => put(body, dataTable([
      { label: "Activation", get: (row) => row.activation, render: (row) => h("code", {}, row.activation) }, { label: "Step", get: (row) => row.step + " " + row.type }, { label: "Detail", get: (row) => row.detail },
      { label: "Establish on a machine", get: () => "", render: (row) => h("div", {}, command("crane resident reconcile " + row.activation + " --step " + row.step + " --happened yes|no --reason TEXT"), h("div", { class: "muted" }, "Declared evidence comes from a person at the machine; adapters that can read back (Jira) are reconciled automatically")) },
    ], data.inbox.reconciliation, { noColumns: true, empty: "No uncertain effect." }))),
    panel("Proposed RCA graphs", { span: 6, flush: true, scope: report }, async (body) => put(body, dataTable([
      { label: "Graph", get: (row) => row.rca, render: (row) => h("code", {}, row.rca) }, { label: "Target", get: (row) => row.target }, { label: "Origin", get: (row) => row.origin.by + (row.origin.activation_id ? " " + row.origin.activation_id : "") },
      { label: "Size", get: (row) => row.nodes, render: (row) => row.nodes + " nodes, " + row.edges + " edges" }, { label: "Review on a machine", get: () => "", render: (row) => command("crane resident rca review " + row.rca + " --accept|--reject --reason TEXT") },
    ], data.inbox.rca_proposals, { noColumns: true, empty: "No proposal." }))),
    panel("Expired or unhealthy assignments", { span: 6, flush: true, scope: report }, async (body) => put(body, dataTable([
      { label: "Assignment", get: (row) => row.assignment }, { label: "Status", get: (row) => row.status, render: (row) => chip(row.status) },
      { label: "Next", get: () => "", render: (row) => command(row.status === "EXPIRED" ? "crane resident renew " + row.assignment + " --reason TEXT" : "crane resident show " + row.assignment) },
    ], data.inbox.assignments, { noColumns: true, empty: "Every assignment is healthy." })))));
  const detail = h("div", {});
  main.append(zone("res-timeline", "Activation timeline", "Every activation from trigger to outcome: matching, context, investigation, plan, Crane's decision for each step, approvals, actions, verification, and usage. Model calls are shown only when they happened.",
    panel("Activations", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.activations); put(body, dataTable([
      { label: "Created", get: (row) => row.created_at, render: (row) => h("span", { class: "mono" }, dt(row.created_at)) },
      { label: "Activation", get: (row) => row.activation_id, render: (row) => h("a", { href: "javascript:void 0", onclick: () => put(detail, activationDetail(row)) }, h("code", {}, row.activation_id)) },
      { label: "Assignment", get: (row) => row.assignment, filter: true },
      { label: "Trigger", get: (row) => row.trigger.source + " " + row.trigger.kind, filter: true },
      { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
      { label: "Outcome", get: (row) => (row.outcome ? row.outcome.status : ""), render: (row) => (row.outcome ? chip(row.outcome.status) : "—"), filter: true },
      { label: "Conclusion", get: (row) => (row.investigation ? row.investigation.conclusion : ""), render: (row) => (row.investigation ? chip(row.investigation.conclusion) : "—"), filter: true },
      { label: "Cache", get: (row) => (row.context ? (row.context.cache || {}).cache : ""), render: (row) => (row.context ? chip((row.context.cache || {}).cache) : "—"), filter: true },
      { label: "Steps", get: (row) => (row.plan ? row.plan.steps.length : 0), num: true },
      { label: "Model calls", get: (row) => row.usage.model_calls, num: true },
      { label: "Act", get: () => "", render: (row) => (["COMPLETED", "PARTIALLY_VERIFIED", "FAILED", "BLOCKED", "CANCELLED", "UNKNOWN", "SIMULATED"].includes(row.state) || row.mode !== "live" ? "—" : h("button", { type: "button", class: "btn", onclick: () => residentDecide("cancel", row) }, "Cancel…")) },
    ], data.activations, { sort: 0, dir: -1, empty: "No activation yet: assignments are idle until a trigger arrives.", name: "foxx-resident-activations" })); }),
    panel("Activation detail", { span: 12 }, async (body) => { put(body, detail); put(detail, activationDetail(data.activations[0])); })));
  const ignoredRows = Object.entries(data.health.ignored || {}).map(([reason, total]) => ({ reason, total }));
  const outcomeRows = Object.entries(data.health.outcomes || {}).map(([state, total]) => ({ state, total }));
  main.append(zone("res-health", "Idle cost and health", "Idleness made visible. Event processing and deterministic checks are counted apart from model calls; tokens are unmeasured when no provider reports them (never zero by assumption).",
    h("div", { class: "kpis span-12" },
      kpi("Ticks", counters.ticks || 0, { note: (counters.idle_ticks || 0) + " idle" }), kpi("Model calls while idle", counters.idle_model_calls || 0, { tone: counters.idle_model_calls ? "bad" : "good" }),
      kpi("Model calls in activations", counters.model_calls || 0), kpi("Deterministic health checks", counters.health_checks || 0, { note: "no model" }),
      kpi("Triggers received", counters.triggers_received || 0, { note: (counters.duplicates || 0) + " duplicates dropped, " + (counters.triggers_rejected || 0) + " rejected" }),
      kpi("Ingestion lag", counters.lag_ms || 0, { format: (value) => Math.round(value / 1000) + " s", note: "last tick, longest" }),
      kpi("Failed or blocked", data.health.dead_letter || 0, { tone: data.health.dead_letter ? "warn" : "" }),
      kpi("Context cache", (data.health.cache || {}).hits || 0, { note: "hits · " + ((data.health.cache || {}).misses || 0) + " misses · " + ((data.health.cache || {}).invalidations || 0) + " invalidated" }),
      kpi("Tokens", "unmeasured", { format: (value) => value, note: data.health.token_usage })),
    panel("Triggers that woke nothing", { span: 4, flush: true, scope: report }, async (body) => put(body, dataTable([{ label: "Reason", get: (row) => row.reason, render: (row) => chip(row.reason) }, { label: "Count", get: (row) => row.total, num: true }], ignoredRows, { noColumns: true, empty: "None." }))),
    panel("Outcomes", { span: 4, flush: true, scope: report }, async (body) => put(body, dataTable([{ label: "Outcome", get: (row) => row.state, render: (row) => chip(row.state) }, { label: "Count", get: (row) => row.total, num: true }], outcomeRows, { noColumns: true, empty: "None.", }))),
    breakdownPanel("Resident events", { span: 4, by: "event", clauses: [{ path: "flow", op: "is", values: ["F38"] }] }),
    seriesPanel("Resident events over time", { span: 12, by: "event", clauses: [{ path: "flow", op: "is", values: ["F38"] }], info: { title: "Resident events", text: "F38: lifecycle, triggers received or ignored, activations, context cache, plans, approvals, actions, verification, reconciliation, and tests." } })));
  main.append(zone("res-test", "Test and simulate", "Plan-only tests: matching, deduplication, cooldown, context and cache, RCA path, the plan, and Crane's decision for every step. Nothing is executed; there is no sandbox, so no run here is labelled one.",
    panel("Test reports", { span: 12, flush: true, scope: report }, async (body, big, ctx) => { ctx.record("rows", data.tests); put(body, dataTable([
      { label: "Tested", get: (row) => row.tested_at, render: (row) => h("span", { class: "mono" }, dt(row.tested_at)) }, { label: "Assignment", get: (row) => row.assignment, filter: true },
      { label: "Result", get: (row) => row.result, render: (row) => chip(row.result), filter: true }, { label: "Matched", get: (row) => (row.matching && row.matching.matched ? "yes" : "no") },
      { label: "Context", get: (row) => (row.context && row.context.cache ? row.context.cache.cache : ""), render: (row) => h("span", {}, chip(row.context && row.context.cache ? row.context.cache.cache : null), (row.context && (row.context.missing || []).length) ? " missing: " + row.context.missing.map((item) => item[0]).join(", ") : "") },
      { label: "Conclusion", get: (row) => row.investigation || "", render: (row) => chip(row.investigation) },
      { label: "Decisions", get: (row) => ((row.plan || {}).steps || []).map((step) => step.type + ":" + step.decision).join(", "), render: (row) => h("div", {}, ((row.plan || {}).steps || []).map((step) => h("div", {}, h("code", {}, step.step), " ", step.type, " ", chip(step.decision), step.needs_person ? " needs a person" : "", step.expected ? " expected " + step.expected.result : ""))) },
      { label: "Notes", get: (row) => (row.notes || []).join("; ") },
    ], data.tests, { sort: 0, dir: -1, empty: "No test yet ('crane resident test ID --event EVENT.json').", name: "foxx-resident-tests" })); }),
    panel("Build a test event", { span: 6 }, async (body) => put(body, residentFixture(data))),
    panel("What can run today", { span: 6, flush: true }, async (body) => put(body, h("div", {},
      dataTable([{ label: "Step type", get: (row) => row.type, render: (row) => h("code", {}, row.type) }, { label: "Effect", get: (row) => row.effect, render: (row) => chip(row.effect) }, { label: "Available", get: (row) => (row.available ? "yes" : "no"), render: (row) => chip(row.available ? "ALLOW" : "UNAVAILABLE") }, { label: "Adapter or reason", get: (row) => row.available ? row.adapter : row.unavailable_reason }], data.catalog.steps || [], { noColumns: true, pageSize: 12 }),
      dataTable([{ label: "Trigger source", get: (row) => row.source, render: (row) => h("code", {}, row.source) }, { label: "Supported", get: (row) => (row.supported ? "yes" : "no"), render: (row) => chip(row.supported ? "ALLOW" : "UNSUPPORTED") }, { label: "How it arrives", get: (row) => row.delivery || "no connector" }], data.catalog.triggers || [], { noColumns: true, pageSize: 12 }))))));
  main.append(zone("res-create", "Create", "", panel("Creation wizard", { span: 12 }, async (body) => put(body, residentWizard(data, bricks)))));
}

Object.assign(window.FOXX_PAGES, { alerting, credits, audit, governance: governanceView, bricks: bricksView, residents: residentsView, compliance, repositories, settings });

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
