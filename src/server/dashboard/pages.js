"use strict";
// The Foxx dashboard: the pages that watch agents. Monitor (overview, event explorer, timeline),
// AI agents (agents, tool calls, sessions, with a profile for each agent, tool, and session), and
// Investigate (logs, traces, topology, distributions, texts). Every panel uses the core of app.js:
// the global filter, the funnel that says how it is filtered, the lens, and the details sidebar.

window.FOXX_PAGES = window.FOXX_PAGES || {};

const TOKEN_AGGS = [["sum", "Total tokens"], ["avg", "Average per call"], ["p95", "95th percentile"], ["max", "Largest call"]];
const TIME_AGGS = [["p95", "95th percentile"], ["p50", "Median"], ["avg", "Average"], ["max", "Slowest"]];

// ---------------------------------------------------------------- overview

async function overview(main) {
  if (needRepository(main)) return;
  const [data, decisions, all, tools] = await Promise.all([api("/api/overview", filterParams()), api("/api/series", filterParams({ by: "decision" })), api("/api/series", filterParams()), usage("tool").catch(() => ({ totals: {}, groups: [] }))]);
  const evals = data.evals || {};
  lastEvals = evals;
  const totals = tools.totals || {};
  const points = (key) => { const item = (decisions.series || []).find((series) => series.key === key); return item ? item.points : null; };
  const to = (clauses) => () => drill({ clauses });
  main.append(hero(repositoryName(), [scope.member ? "Member " + memberLabel(scope.member) : "All " + (data.members || 0) + " member(s)", rangeLabel(), activeFilters() ? activeFilters() + " filter(s)" : "no filter"].join(" · "),
    [["Events", compact(evals.events), "eval:events"], ["Tool calls", compact(totals.calls), "eval:tool_calls"], ["Tokens", tokens(totals.tokens && totals.tokens.total), "eval:model_tokens"], ["Active sessions", compact(data.active_sessions)], ["Agents", compact(data.agents)], ["Event chains", data.event_chain.status], ["Governance", data.integrity.reported ? (data.integrity.ok ? "passing" : "failing") : "not reported"]],
    [h("button", { class: "btn", type: "button", onclick: () => go("#/agents") }, "Agents"), h("button", { class: "btn", type: "button", onclick: () => go("#/tools") }, "Tool calls"), h("button", { class: "btn", type: "button", onclick: () => details.guide() }, "Guide")]));
  main.append(zone("health", "Health", "The headline numbers of the range and filter. Each tile is one eval; click it for its events.",
    h("div", { class: "kpis span-12" },
      kpi("Tool calls decided", evals.tool_calls_decided, { info: "eval:tool_calls_decided", tone: "brand", spark: all.series && all.series[0] ? all.series[0].points : null, note: compact(evals.events) + " events in all", drill: to([is("flow", "F05")]) }),
      kpi("Allowed", evals.allowed, { info: "eval:allowed", spark: points("ALLOW"), color: Foxx.STATUS.good, tone: "good", drill: to([is("decision.decision", "ALLOW")]) }),
      kpi("Denied", evals.denied, { info: "eval:denied", spark: points("DENY"), color: Foxx.STATUS.critical, tone: evals.denied ? "bad" : "", note: "share " + percent(evals.denial_share), drill: to([is("decision.decision", "DENY")]) }),
      kpi("Approvals", evals.approvals, { info: "eval:approvals", spark: points("REQUIRE_APPROVAL"), color: Foxx.STATUS.warning, tone: evals.approvals ? "warnt" : "", drill: to([is("decision.decision", "REQUIRE_APPROVAL")]) }),
      kpi("Evidence completeness", evals.evidence_completeness, { info: "eval:evidence_completeness", format: percent, tone: evals.evidence_completeness >= 0.95 ? "good" : "" }),
      kpi("Credits available", (evals.credits || {}).available, { info: "eval:credits", note: compact((evals.credits || {}).consumed) + " consumed", drill: () => go("#/credits") }))));
  main.append(zone("fleet", "Agent fleet", "What the agents did and spent: tool calls, the tokens their model turns burnt, how long tools ran, and how often calls failed.",
    h("div", { class: "kpis span-12" },
      kpi("Tool calls", totals.calls, { info: "eval:tool_calls", tone: "brand", drill: () => go("#/tools") }),
      kpi("Tokens burnt", totals.tokens && totals.tokens.total, { info: "eval:model_tokens", format: tokens, note: (totals.tokens ? fmt(totals.tokens.measured_calls) : 0) + " calls report tokens", drill: () => go("#/tools") }),
      kpi("Tokens per call", totals.tokens && totals.tokens.per_call, { info: "eval:model_tokens", format: tokens }),
      kpi("Tool time p95", totals.tool_ms && totals.tool_ms.p95, { info: "eval:tool_duration_ms", format: duration, note: "median " + duration(totals.tool_ms && totals.tool_ms.p50) }),
      kpi("Failure share", totals.failure_share, { format: percent, tone: totals.failure_share > 0.1 ? "bad" : "", note: fmt(totals.failed) + " failed calls" }),
      kpi("Agents active", totals.agents, { note: fmt(totals.sessions) + " sessions", drill: () => go("#/agents") })),
    seriesPanel("Tokens burnt over time", { span: 6, by: "agent.agent_id", metric: "eval.model_tokens", agg: "sum", metrics: TOKEN_AGGS, type: "bars", height: 200, info: "eval:model_tokens", empty: "No tool call reported tokens in this range (Claude Code reports them)." }),
    panel("Tool calls by tool", { span: 6, info: { title: "Tool calls by tool", text: "How many calls each tool received, with its token share and 95th-percentile tool time. Click a tool for its profile." } }, async (body, big, ctx) => {
      ctx.record("usage", tools, { by: "tool" });
      put(body, charts.toplist(tools.groups.map((group) => ({ value: group.key, count: group.calls })), { onPick: (value) => openTool(value), hint: "Open this tool's profile", byValue: true }));
    })));
  main.append(zone("activity", "Activity", "How many events happened over time, and how they divide.",
    seriesPanel("Events over time", { span: 8, by: "severity", height: 240, info: "page:overview" }),
    breakdownPanel("Breakdown", { span: 4, by: "severity", info: "facet:severity" })));
  main.append(zone("decisions", "Decisions", "What Crane decided before each tool call (F05).",
    seriesPanel("Tool call decisions", { span: 8, by: "decision", clauses: [is("flow", "F05")], info: "flow:F05" }),
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
    panel("Recent violations", { span: 6, flush: true, info: "value:VIOLATION", scope: { what: "The newest violations" } }, async (body, big, ctx) => { ctx.record("rows", data.recent_violations || []); put(body, eventsTable(data.recent_violations || [], { pageSize: 8, noTools: true, empty: "No violations in this range: the healthy state." })); }),
    panel("Recent alerts", { span: 6, flush: true, info: "flow:F14", scope: { kind: "state", what: "The newest alerts" } }, async (body, big, ctx) => { ctx.record("rows", data.recent_alerts || []); put(body, dataTable([
      { label: "Time", get: (row) => row.at, render: (row) => h("span", { class: "mono" }, dt(row.at * 1000)) }, { label: "Alert", get: (row) => row.title }, { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity) }, { label: "Member", render: (row) => member(row.member) },
    ], data.recent_alerts || [], { noTools: true, sort: 0, pageSize: 8, empty: "No alerts." })); })));
  main.append(zone("evals", "All evals", "Every number the dashboard computes from the events in range and filter.",
    panel("Evals", { span: 12, flush: true }, async (body, big, ctx) => { ctx.record("value", Object.fromEntries(Object.entries(evals).filter(([, value]) => typeof value === "number"))); put(body, evalsTable(evals)); })));
}

// ---------------------------------------------------------------- event explorer

async function explorer(main) {
  if (needRepository(main)) return;
  const cache = await facets();
  const side = h("aside", { class: "facets", "aria-label": "Facets" }, h("div", { class: "head" }, h("span", {}, "Facets", info("", { title: "Facets", text: "Each attribute with a count for each of its values among the events in range and filter. A facet ignores its own selection, so its other values stay visible. Tick a value to keep only it, ⊖ to exclude it; values of one facet combine with OR, facets with AND." })), h("span", { class: "muted" }, compact(cache.total) + " events")),
    FACETS.map(([name, path], index) => {
      const values = (cache.facets || {})[name] || [];
      const chosen = values.filter((item) => isChosen(path, item.value) || isChosen(path, item.value, true)).length;
      return h("details", { open: index < 6 || chosen ? true : null }, h("summary", {}, h("span", {}, FACET_LABELS[name], info("facet:" + name)), chosen ? h("span", { class: "chip brand" }, chosen) : null),
        values.length ? values.map((item) => h("div", { class: "facet-row" }, h("label", {}, h("input", { type: "checkbox", checked: isChosen(path, item.value) ? true : null, onchange: () => { toggleValue(path, item.value); render(); } }), h("span", { title: item.label || item.value, style: isChosen(path, item.value, true) ? "text-decoration:line-through" : "" }, item.label || item.value)), explained(item.value), h("em", {}, compact(item.count)),
          h("button", { type: "button", class: "exclude" + (isChosen(path, item.value, true) ? " on" : ""), title: isChosen(path, item.value, true) ? "Excluded: click to include again" : "Exclude this value", "aria-label": "Exclude " + item.value, onclick: () => { toggleValue(path, item.value, true); render(); } }, "⊖")))
          : h("div", { class: "muted none" }, "none"));
    }));
  const right = h("div", { class: "explorer-main" });
  main.append(h("div", { class: "explorer" }, side, right));
  right.append(zone("volume", "Volume", "Events over time in range and filter; drag across to zoom.", seriesPanel("Event volume", { span: 12, by: "severity", height: 180 })));
  let cursor = 0;
  const rows = [];
  const stream = panel("Event stream", { span: 12, flush: true, info: "page:explorer" }, async (body, big, ctx) => {
    const data = await api("/api/events", filterParams({ cursor, limit: 500 }));
    rows.push(...data.events);
    cursor = data.next_cursor;
    ctx.record("rows", rows, { summary: compact(data.total) + " events match; the newest " + rows.length + " are loaded." });
    put(body, h("div", { class: "muted pad" }, compact(data.total) + " matching events, newest first" + (rows.length < data.total ? " (newest " + rows.length + " loaded)" : "")), eventsTable(rows, { pageSize: 50 }), cursor ? h("div", { class: "pager" }, h("button", { class: "btn", type: "button", onclick: () => stream.refresh() }, "Load 500 more")) : null);
  });
  right.append(zone("stream", "Event stream", "Every matching event; open one for all thirteen attribute groups.", stream));
}

// ---------------------------------------------------------------- timeline

async function attributeOptions() {
  const data = await attributes();
  return (data.attributes || []).filter((attribute) => attribute.kind !== "number" && attribute.distinct > 0 && attribute.distinct < 200 && !facetName(attribute.path) && attribute.path.includes(".")).map((attribute) => [attribute.path, attribute.path]);
}
function ganttPanel(title, options) {
  const state = { rows: options.rows || "session", color: options.color || "decision" };
  const node = panel(title, { span: 12, filterable: true, clauses: options.clauses, info: { title: "Gantt chart", text: "One bar per trace: a tool call from its first event (credit reservation or decision) to its last (verification, credit settlement), extended by the measured tool time; or a command, an alert, or any single event. Rows group bars by the chosen attribute; colors show another. A red outline marks a failed trace. Each session row is shaded from the session's start to its end. Drag across to zoom; click a bar for its trace." },
    scope: () => ({ what: "Traces with an event", notes: ["Rows by " + state.rows + ", colored by " + state.color + "; the newest 1,500 traces."] }) }, async (body, big, ctx) => {
    const data = await api("/api/gantt", filterParams({ rows: state.rows, color: state.color, clauses: ctx.clauses(), ignoreTime: options.ignoreTime }));
    ctx.record("gantt", data, { rows: state.rows, color: state.color });
    put(body, data.truncated ? h("div", { class: "notice warn" }, "Showing the newest 1,500 of " + compact(data.total) + " traces; narrow the range or filter.") : null, h("div", { class: big ? "tall" : "" }, charts.gantt(data, { title, onZoom: zoomTo, onPick: (bar) => openTrace(bar.id) })));
  });
  attributeOptions().then((attributeChoices) => {
    const rowChoices = [["session", "Rows: session"], ["agent", "Rows: agent type"], ["agent.agent_id", "Rows: agent"], ["member", "Rows: member"], ["tool", "Rows: tool"], ["flow", "Rows: flow"], ["decision", "Rows: decision"], ["origin", "Rows: origin"]].concat(attributeChoices.filter(([path]) => path !== "agent.agent_id").map(([path, label]) => [path, "Rows: " + label]));
    const colorChoices = [["decision", "Color: decision"], ["severity", "Color: severity"], ["tool", "Color: tool"], ["flow", "Color: flow"], ["enforcement", "Color: enforcement"], ["response", "Color: response"], ["origin", "Color: origin"]].concat(attributeChoices.map(([path, label]) => [path, "Color: " + label]));
    node.controls.append(select("Rows", state.rows, rowChoices, (value) => { state.rows = value; node.refresh(); }), select("Color", state.color, colorChoices, (value) => { state.color = value; node.refresh(); }));
  });
  return node;
}
async function timeline(main) {
  if (needRepository(main)) return;
  main.append(zone("gantt", "Gantt chart", "The exact time span of every agent action, grouped and colored by any attribute.", ganttPanel("Gantt chart of agent actions", {})));
  const lanes = { lanes: "flow", color: "severity" };
  const lanePanel = panel("Event timeline", { span: 12, filterable: true, info: { title: "Event timeline", text: "Every event as a dot at its exact time, on a lane per value of the chosen attribute (flows by default, labelled with their names), colored by another. Drag across to zoom; click a dot for the event." }, scope: () => ({ notes: ["Lanes by " + lanes.lanes + ", colored by " + lanes.color + "; the newest 5,000 events."] }) }, async (body, big, ctx) => {
    const data = await api("/api/timeline", filterParams({ lanes: lanes.lanes, color: lanes.color, clauses: ctx.clauses() }));
    ctx.record("lanes", data, { lanes: lanes.lanes, color: lanes.color });
    put(body, data.truncated ? h("div", { class: "notice warn" }, "Showing the newest 5,000 of " + compact(data.total) + " events.") : null, charts.swimlanes(data, { onZoom: zoomTo, onPick: (point) => openEventById(point.id), laneLabel: (key) => (lanes.lanes === "flow" && G.flows[key] ? key + " " + G.flows[key][0].slice(0, 22) : key), laneTitle: (key) => meaning(key) || key }));
  });
  lanePanel.controls.append(select("Lanes", lanes.lanes, AXES.map(([key, label]) => [key, "Lanes: " + label.toLowerCase()]), (value) => { lanes.lanes = value; lanePanel.refresh(); }), select("Color", lanes.color, AXES.map(([key, label]) => [key, "Color: " + label.toLowerCase()]), (value) => { lanes.color = value; lanePanel.refresh(); }));
  main.append(zone("swimlanes", "Event timeline", "Every event at its exact time.", lanePanel));
  main.append(zone("rhythm", "Activity rhythm", "When agents work: the days of the week and hours of the day (your time zone), and every day in range.",
    panel("Day and hour heatmap", { span: 12, filterable: true, info: { title: "Activity rhythm", text: "Each cell counts the events of one hour of one weekday in your local time; darker is more. Below, every day in range; click a day to zoom to it." } }, async (body, big, ctx) => {
      const data = await api("/api/calendar", filterParams({ clauses: ctx.clauses(), tz_offset: new Date().getTimezoneOffset() }));
      ctx.record("matrix", { rows: ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"].map((day) => ({ key: day })), cols: Array.from({ length: 24 }, (_, hour) => ({ key: hour + ":00" })), cells: data.week, max: data.max_hour, rows_by: "weekday", cols_by: "hour" }, { timezone_offset_minutes: data.tz_offset });
      put(body, charts.calendar(data, { onDay: (day) => zoomTo(day, day + 86400000 - 1), onPick: (weekday, hour) => details.open("Weekday " + ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"][Number(weekday)] + " at " + hour + ":00", h("p", {}, fmt(data.week[weekday][hour]) + " events happened in this hour of this weekday over the range."), { kind: "Rhythm" }) }));
    })));
  main.append(zone("concurrency", "Concurrency", "How many agent sessions and agents were active at once (distinct values per time bucket).",
    seriesPanel("Active sessions", { span: 6, by: "agent", metric: "session.session_id", agg: "distinct", type: "area", height: 200, info: { title: "Active sessions", text: "The number of different sessions that produced an event in each time bucket, by agent type." } }),
    seriesPanel("Active agents", { span: 6, metric: "agent.agent_id", agg: "distinct", type: "lines", height: 200, info: { title: "Active agents", text: "The number of different agent identities that produced an event in each time bucket." } })));
  main.append(zone("latency", "Latency and tokens", "The cost of each call over time: how long tools ran, how long Crane's hook took to decide, and the tokens burnt.",
    seriesPanel("Tool time", { span: 4, by: "tool", metric: "eval.tool_duration_ms", agg: "p95", metrics: TIME_AGGS, type: "lines", height: 200, format: duration, info: "eval:tool_duration_ms" }),
    seriesPanel("Hook latency", { span: 4, metric: "eval.latency_ms", agg: "p95", metrics: TIME_AGGS, type: "lines", height: 200, format: duration, info: "eval:latency_ms" }),
    seriesPanel("Tokens", { span: 4, by: "tool", metric: "eval.model_tokens", agg: "sum", metrics: TOKEN_AGGS, type: "bars", height: 200, info: "eval:model_tokens" })));
}

// ---------------------------------------------------------------- logs and traces

async function logs(main) {
  if (needRepository(main)) return;
  const data = await attributes();
  const info2 = await retention();
  let saved = [];
  try { saved = JSON.parse(sessionStorage.getItem("foxx-log-columns") || "[]"); } catch (error) { saved = []; }
  const extra = new Set(saved);
  let cursor = 0;
  const rows = [];
  const read = (spec, path) => { let node = spec; for (const part of path.split(".")) node = node === null || node === undefined ? undefined : node[part]; return Array.isArray(node) ? node.join(", ") : node; };
  const table = panel("Log table", { span: 12, flush: true, info: "page:logs" }, async (body, big, ctx) => {
    const page = await api("/api/events", filterParams({ cursor, limit: 500 }));
    rows.push(...page.events);
    cursor = page.next_cursor;
    ctx.record("rows", rows, { summary: compact(page.total) + " log lines match; " + rows.length + " loaded." });
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
      { label: "Tokens", get: (row) => (row.eval || {}).model_tokens, render: (row) => ((row.eval || {}).model_tokens !== undefined ? tokens(row.eval.model_tokens) : ""), num: true, info: "eval:model_tokens" },
      { label: "Response", get: (row) => (row.response || {}).response_status, render: (row) => chip((row.response || {}).response_status), filter: true, info: "facet:response" },
      { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
      { label: "Session", get: (row) => sessionOf(row), render: (row) => sessionLink(sessionOf(row)) },
      { label: "Data", get: (row) => (staleAt(row.occurred_at, info2) !== null && row.origin !== "DERIVED" ? "STALE_DATA" : "CURRENT"), render: (row) => (staleAt(row.occurred_at, info2) !== null && row.origin !== "DERIVED" ? chip("STALE_DATA") : ""), filter: true, info: "value:STALE_DATA" },
      { label: "Origin", get: (row) => row.origin, render: (row) => chip(row.origin), filter: true, info: "facet:origin", hidden: true },
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
  main.append(zone("log-table", "Log table", "Every matching event as a log line. The global filter (top) narrows what is loaded: any attribute, any operator, AND, OR, and NOT. Column filters and search narrow the loaded lines.", table));
}

async function traces(main) {
  if (needRepository(main)) return;
  const state = { kind: "", outcome: "", min_ms: "", max_ms: "", min_spans: "" };
  let tablePanel = null;
  main.append(zone("trace-durations", "Durations", "How long traces took, from their first event to their last.",
    panel("Trace durations", { span: 12, info: { title: "Trace durations", text: "A histogram of trace durations: a tool call from its decision to its verification and credit settlement, a command, an alert. The median (p50) and 95th percentile (p95) are marked; click a bar to list those traces." } }, async (body, big, ctx) => {
      const data = await api("/api/histogram", filterParams({ field: "trace_duration_ms", bins: big ? 40 : 24 }));
      ctx.record("histogram", data, { field: "trace_duration_ms", format: duration });
      put(body, charts.histogram(data, { height: big ? 420 : 200, format: duration, onPick: (bin) => { state.min_ms = String(Math.floor(bin.from)); state.max_ms = String(Math.ceil(bin.to)); if (tablePanel) tablePanel.refresh(); scrollToZone("trace-table"); } }));
    })));
  tablePanel = panel("Trace table", { span: 12, flush: true, info: "page:traces", scope: () => ({ what: "Traces with at least one event", notes: [[state.kind && "kind " + state.kind, state.outcome && "outcome " + state.outcome, state.min_ms && "at least " + duration(Number(state.min_ms)), state.max_ms && "at most " + duration(Number(state.max_ms)), state.min_spans && "at least " + state.min_spans + " events"].filter(Boolean).join(", ")].filter(Boolean).map((text) => "Trace filters: " + text + ".") }) }, async (body, big, ctx) => {
    const data = await api("/api/traces", filterParams(Object.assign({ limit: 500 }, state)));
    ctx.record("calls", data.traces, { total: data.total });
    const kinds = select("Kind", state.kind, [["", "Kind: all"]].concat((data.kinds || []).map((kind) => [kind, kind])), (value) => { state.kind = value; tablePanel.refresh(); });
    const numberInput = (key, placeholder) => h("input", { type: "number", min: "0", placeholder, value: state[key], "aria-label": placeholder, onchange: (event) => { state[key] = event.target.value; tablePanel.refresh(); } });
    const filters = h("div", { class: "table-tools" }, h("strong", {}, "Trace filters"), kinds, select("Outcome", state.outcome, [["", "Outcome: all"], ["OK", "OK"], ["FAILED", "FAILED"]], (value) => { state.outcome = value; tablePanel.refresh(); }),
      numberInput("min_ms", "min duration ms"), numberInput("max_ms", "max duration ms"), numberInput("min_spans", "min events"),
      h("button", { class: "btn link", type: "button", onclick: () => { Object.keys(state).forEach((key) => { state[key] = ""; }); tablePanel.refresh(); } }, "Reset"), h("span", { class: "count" }, compact(data.total) + " traces"));
    put(body, filters, dataTable([
      { label: "", render: (row) => h("span", { class: "sev", style: "background:" + color(row.severity, 0) }) },
      { label: "Start", get: (row) => row.start, render: (row) => h("span", { class: "mono" }, dt(row.start)), csv: (row) => dt(row.start) },
      { label: "Duration", get: (row) => row.duration_ms, render: (row) => duration(row.duration_ms), num: true, bar: true, format: duration },
      { label: "Kind", get: (row) => row.kind, render: (row) => chip(row.kind), filter: true },
      { label: "Trace", get: (row) => row.root, render: (row) => h("div", {}, h("code", {}, row.root), h("div", { class: "muted" }, row.events.join(" → "))) },
      { label: "Tool", get: (row) => row.tool, filter: true, info: "facet:tool" },
      { label: "Tokens", get: (row) => (row.tokens ? row.tokens.total : null), render: (row) => (row.tokens ? tokens(row.tokens.total) : "—"), num: true, info: "eval:model_tokens" },
      { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true },
      { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
      { label: "Decision", get: (row) => row.decision, render: (row) => chip(row.decision), filter: true },
      { label: "Events", get: (row) => row.spans, num: true },
      { label: "Session", get: (row) => row.session, render: (row) => sessionLink(row.session) },
      { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
      { label: "Flows", get: (row) => row.flows.join(" "), render: (row) => h("span", {}, row.flows.map((flow) => h("span", { class: "tag", title: meaning(flow) }, flow))) },
    ], data.traces, { sort: 1, dir: -1, pageSize: 40, onRow: (row) => openTrace(row.id), name: "foxx-traces", placeholder: "Search traces…" }));
  });
  main.append(zone("trace-table", "Trace table", "Every trace with at least one event matching the global filter; open one for its waterfall.", tablePanel));
}

// ---------------------------------------------------------------- AI agents: tool calls

/** A tool's profile in the details sidebar: its metrics, decisions, recent calls, and waterfall */
async function openTool(tool) {
  const clauses = [is("action.action_tool", tool)];
  const [stats, calls, decisions] = await Promise.all([usage("tool", { clauses }), toolCalls({ clauses }), api("/api/facets", filterParams({ clauses: clauses.concat([is("flow", "F05")]) }))]);
  const group = stats.groups.find((item) => item.key === tool) || stats.totals;
  details.open(tool, h("div", {},
    h("p", { class: "sentence" }, filterSentence({ what: "Tool calls of " + tool })),
    h("div", { class: "metric-grid", style: "margin-top:10px" }, [["calls", fmt(group.calls)], ["denied", fmt(group.denied)], ["approvals", fmt(group.approvals)], ["failed", fmt(group.failed) + " (" + percent(group.failure_share) + ")"], ["tool time p50", duration(group.tool_ms.p50)], ["tool time p95", duration(group.tool_ms.p95)], ["hook latency p95", duration(group.latency_ms.p95)], ["tokens", tokens(group.tokens.total)], ["tokens per call", tokens(group.tokens.per_call)], ["sessions", fmt(group.sessions)], ["agents", fmt(group.agents)], ["last used", dt(group.last)]].map(([label, value]) => h("div", {}, h("span", {}, label), h("b", {}, value)))),
    h("h4", {}, "Decisions"), charts.donut((decisions.facets.decision || []).map((item) => ({ value: item.value, count: item.count })), { unit: "decisions", explain: explained, onPick: (value) => { details.close(); drill({ clauses: clauses.concat([is("decision.decision", value)]) }); } }),
    h("h4", {}, "Waterfall of its newest calls"), charts.waterfall(calls.slice(0, 40), { packed: true, onPick: (call) => openTrace(call.id) }),
    h("h4", {}, "Calls"), callsTable(calls, { pageSize: 10, compact: true }),
    h("div", { class: "row-actions", style: "margin-top:10px" }, h("button", { class: "btn", type: "button", onclick: () => { details.close(); drill({ clauses }); } }, "Open its events"), h("button", { class: "btn", type: "button", onclick: () => { details.close(); setClause("action.action_tool", "is", [tool]); render(); } }, "Filter every view by it"))), { wide: true, kind: "Tool" });
}
function waterfallPanel(title, options) {
  const state = { limit: "60", session: options.session || "", axis: "packed" };
  const node = panel(title, { span: 12, filterable: true, clauses: options.clauses, info: { title: "Waterfall of tool calls", text: "One row per tool call, oldest at the top. On the packed axis each bar starts where the call before it ended, so durations compare even when calls are hours apart (Σ is the running total of tool time); on the real-time axis each bar starts when Crane decided. A bar lasts as long as the tool ran (pre-tool to post-tool hook); its color is Crane's decision (red outline: the call failed). On the right, the tokens of the model turn that issued it (its equal share). Hover a row for every metric; click it for the trace." },
    scope: () => ({ what: "Tool calls", notes: ["The newest " + state.limit + (state.session ? " of session " + state.session : "") + ", drawn oldest first."] }) }, async (body, big, ctx) => {
    const extra = state.session ? [is("session.session_id", state.session)] : [];
    const calls = (await toolCalls({ clauses: ctx.clauses().concat(extra) })).slice(0, Number(state.limit));
    ctx.record("calls", calls, { limit: state.limit, session: state.session || "any" });
    put(body, charts.waterfall(calls, { packed: state.axis === "packed", onPick: (call) => openTrace(call.id) }));
  });
  node.controls.append(segmented("Time axis", state.axis, [["packed", "Packed"], ["real", "Real time"]], (value) => { state.axis = value; node.refresh(); }), select("Calls", state.limit, [["30", "Newest 30"], ["60", "Newest 60"], ["120", "Newest 120"], ["250", "Newest 250"]], (value) => { state.limit = value; node.refresh(); }));
  if (options.sessions) api("/api/sessions").then((data) => node.controls.prepend(select("Session", state.session, [["", "All sessions"]].concat(data.sessions.map((row) => [row.session.foxx_session_id, "Session " + shortIdentity(row.session.foxx_session_id) + " · " + (row.member ? row.member.actor : "")])), (value) => { state.session = value; node.refresh(); }))).catch(() => {});
  return node;
}
async function tools(main) {
  if (needRepository(main)) return;
  const [stats, calls, ledger] = await Promise.all([usage("tool"), toolCalls(), api("/api/ledger")]);
  const totals = stats.totals;
  main.append(hero("Tool calls", "Every call an agent made through Crane's hooks: what it was, what Crane decided, how long it ran, and the tokens its model turn burnt.",
    [["Calls", compact(totals.calls), "eval:tool_calls"], ["Tools", compact(stats.groups.length)], ["Tokens", tokens(totals.tokens.total), "eval:model_tokens"], ["Tool time p95", duration(totals.tool_ms.p95), "eval:tool_duration_ms"], ["Failure share", percent(totals.failure_share)]]));
  main.append(zone("tool-kpis", "Summary", "The calls in range and filter.",
    h("div", { class: "kpis span-12" },
      kpi("Tool calls", totals.calls, { tone: "brand", info: "eval:tool_calls" }),
      kpi("Denied", totals.denied, { tone: totals.denied ? "bad" : "good", note: percent(totals.denied / Math.max(1, totals.calls)) + " of calls", drill: () => drill({ clauses: [is("decision.decision", "DENY")] }) }),
      kpi("Needed approval", totals.approvals, { tone: totals.approvals ? "warnt" : "", drill: () => drill({ clauses: [is("decision.decision", "REQUIRE_APPROVAL")] }) }),
      kpi("Failed", totals.failed, { tone: totals.failed ? "bad" : "", note: percent(totals.failure_share) }),
      kpi("Tokens burnt", totals.tokens.total, { format: tokens, info: "eval:model_tokens", note: fmt(totals.tokens.measured_calls) + " of " + fmt(totals.calls) + " calls report tokens" }),
      kpi("Tokens per call", totals.tokens.per_call, { format: tokens, info: "eval:model_tokens" }),
      kpi("Cache read share", totals.tokens.total ? totals.tokens.cache_read / totals.tokens.total : null, { format: percent, info: "eval:cache_read_tokens" }),
      kpi("Tool time p50 / p95", totals.tool_ms.p50, { format: duration, note: "p95 " + duration(totals.tool_ms.p95), info: "eval:tool_duration_ms" }),
      kpi("Hook latency p95", totals.latency_ms.p95, { format: duration, info: "eval:latency_ms" }))));
  main.append(zone("tool-types", "Types of tool calls", "What kind of calls agents make: by tool, by the resource they touch, and by Crane's decision.",
    panel("Calls by tool", { span: 4, info: { title: "Calls by tool", text: "How many calls each tool received. Click a slice for the tool's profile." } }, async (body, big, ctx) => { const items = stats.groups.map((group) => ({ value: group.key, count: group.calls })); ctx.record("items", items, { unit: "calls" }); put(body, charts.donut(items.slice(0, 8), { unit: "calls", onPick: openTool, hint: "Click: the tool's profile" })); }),
    breakdownPanel("By resource kind", { span: 4, by: "resource.resource", clauses: [is("flow", "F05")], info: { title: "By resource kind", text: "What the calls touched (file reads and writes, network, git, processes, …), counted on Crane's decisions." } }),
    breakdownPanel("By decision", { span: 4, by: "decision", clauses: [is("flow", "F05")], info: "facet:decision" })));
  main.append(zone("tool-metrics", "Metrics per tool", "Every tool with its calls, outcomes, time, and tokens. Click a row for the tool's profile.",
    panel("Tool metrics", { span: 12, flush: true, info: { title: "Tool metrics", text: "Per tool: calls; denied (DENY, QUARANTINE, ERROR); approvals; failed (refused or the tool reported a failure); tool time percentiles (pre-tool to post-tool hook); hook latency (Crane's decision time); tokens (the shares of the model turns that issued the calls); sessions and agents that used it." } }, async (body, big, ctx) => {
      ctx.record("usage", stats, { by: "tool" });
      put(body, dataTable([
        { label: "Tool", get: (row) => row.key, render: (row) => h("b", {}, row.key) },
        { label: "Calls", get: (row) => row.calls, num: true, bar: true },
        { label: "Denied", get: (row) => row.denied, num: true },
        { label: "Approvals", get: (row) => row.approvals, num: true },
        { label: "Failed", get: (row) => row.failure_share, render: (row) => fmt(row.failed) + " · " + percent(row.failure_share), num: true },
        { label: "Time p50", get: (row) => row.tool_ms.p50, render: (row) => duration(row.tool_ms.p50), num: true, info: "eval:tool_duration_ms" },
        { label: "Time p95", get: (row) => row.tool_ms.p95, render: (row) => duration(row.tool_ms.p95), num: true, bar: true, format: duration },
        { label: "Total time", get: (row) => row.tool_ms.total, render: (row) => duration(row.tool_ms.total), num: true, hidden: true },
        { label: "Latency p95", get: (row) => row.latency_ms.p95, render: (row) => duration(row.latency_ms.p95), num: true, info: "eval:latency_ms" },
        { label: "Tokens", get: (row) => row.tokens.total, render: (row) => tokens(row.tokens.total), num: true, bar: true, format: tokens, info: "eval:model_tokens" },
        { label: "Per call", get: (row) => row.tokens.per_call, render: (row) => tokens(row.tokens.per_call), num: true },
        { label: "Output", get: (row) => row.tokens.output, render: (row) => compact(row.tokens.output), num: true, hidden: true },
        { label: "Cache read", get: (row) => row.tokens.cache_read, render: (row) => compact(row.tokens.cache_read), num: true, hidden: true },
        { label: "Sessions", get: (row) => row.sessions, num: true },
        { label: "Agents", get: (row) => row.agents, num: true },
        { label: "Last used", get: (row) => row.last, render: (row) => h("span", { class: "mono" }, dt(row.last)) },
      ], stats.groups, { sort: 1, onRow: (row) => openTool(row.key), name: "foxx-tool-metrics", placeholder: "Search tools…" }));
    }),
    panel("Decisions per tool", { span: 6, info: { title: "Decisions per tool", text: "For each tool, how Crane's decisions divide: allowed, needed approval, denied. Click a segment for those events." } }, async (body, big, ctx) => {
      const data = await api("/api/matrix", filterParams({ rows: "tool", cols: "decision", clauses: [is("flow", "F05")], hide_none: "1", limit: 15 }));
      ctx.record("matrix", data, { rows: "tool", columns: "decision" });
      put(body, charts.stacked(data.rows.map((row, r) => ({ label: row.label, key: row.key, parts: Object.fromEntries(data.cols.map((col, c) => [col.key, data.cells[r][c]])) })), { keys: data.cols.map((col) => col.key), unit: "decisions", onPick: (row, key) => drill({ clauses: [is("flow", "F05"), is("action.action_tool", row.key), is("decision.decision", key)] }) }));
    }),
    matrixPanel("Tools by resource", { span: 6, rows: "tool", cols: "resource.resource_ids", clauses: [is("flow", "F05")] })));
  main.append(zone("tool-waterfall", "Waterfall", "The time each call took, one after another, with the tokens its model turn burnt.", waterfallPanel("Waterfall of tool calls", { sessions: true })));
  main.append(zone("tool-credits", "Actual credit consumption", "Credits actually settled after calls ran, net of refunds. Tool attribution requires a matching tool-call ID in both the signed ledger and the trace.",
    panel("Consumed credits by tool", { span: 7, info: { title: "Actual credits consumed by tool", text: "This uses CONSUME ledger entries minus REFUND entries, joined to tool traces by the internal tool-call ID. Entries without a matching trace are grouped as Unattributed; grants and reservations are not counted as consumption." } }, async (body, big, ctx) => {
      const items = consumedCreditsBy(ledger.entries, calls, "tool").filter((item) => item.count > 0);
      ctx.record("items", items, { unit: "credits", source: "autonomy ledger" });
      put(body, charts.toplist(items, { unit: "credits", format: fmt, onPick: openTool, byValue: true, empty: "No settled tool-call credits in the current range." }));
    }),
    panel("Consumption by session", { span: 5, info: { title: "Actual credits consumed by session", text: "Net consumed credits from the autonomy ledger. Click a session to inspect its activity and remaining balance." }, scope: { kind: "state", what: "Ledger consumption" } }, async (body, big, ctx) => {
      const items = consumedCreditsBy(ledger.entries, [], "session").filter((item) => item.count > 0).slice(0, 12);
      ctx.record("items", items, { unit: "credits" });
      put(body, charts.toplist(items, { unit: "credits", format: fmt, onPick: (id) => go("#/sessions/" + encodeURIComponent(id)), byValue: true, empty: "No consumed credits in the ledger." }));
    })));
  main.append(zone("tool-tokens", "Tokens per call", "How many tokens each call's model turn burnt (Claude Code transcripts; shared equally among the calls of one turn).",
    panel("Tokens per call", { span: 6, info: "eval:model_tokens" }, async (body, big, ctx) => {
      const data = await api("/api/histogram", filterParams({ field: "eval.model_tokens", bins: big ? 40 : 20 }));
      ctx.record("histogram", data, { field: "eval.model_tokens" });
      put(body, charts.histogram(data, { height: big ? 420 : 220, empty: "No tool call reported tokens in this range (Claude Code reports them through its transcript).", onPick: (bin) => drill({ clauses: [{ path: "eval.model_tokens", op: "between", values: [String(bin.from), String(bin.to)] }] }) }));
    }),
    panel("Time against tokens", { span: 6, info: { title: "Time against tokens", text: "Each dot is one tool call: how long the tool ran (across) against the tokens of its model turn (up), colored by tool. Calls up and to the right are slow and expensive." } }, async (body, big, ctx) => {
      const points = calls.filter((call) => call.tokens && call.tool_ms !== null && call.tool_ms !== undefined).map((call) => ({ x: call.tool_ms, y: call.tokens.total, key: call.tool, label: call.tool + " · " + dt(call.start), id: call.id, rows: [["decision", call.decision], ["session", call.session]] }));
      ctx.record("calls", calls.filter((call) => call.tokens), {});
      put(body, charts.scatter(points, { height: big ? 420 : 240, xLabel: "tool time", yLabel: "tokens", xFormat: duration, yFormat: compact, keyLabel: "tool", empty: "No call has both a tool time and tokens.", onPick: (point) => openTrace(point.id) }));
    }),
    seriesPanel("Tokens by tool over time", { span: 12, by: "tool", metric: "eval.model_tokens", agg: "sum", metrics: TOKEN_AGGS, height: 200, info: "eval:model_tokens" })));
  main.append(zone("tool-time", "Time per call", "How long tools ran.",
    panel("Tool time", { span: 6, info: "eval:tool_duration_ms" }, async (body, big, ctx) => {
      const data = await api("/api/histogram", filterParams({ field: "eval.tool_duration_ms", bins: big ? 40 : 20 }));
      ctx.record("histogram", data, { field: "eval.tool_duration_ms", format: duration });
      put(body, charts.histogram(data, { height: big ? 420 : 220, format: duration, onPick: (bin) => drill({ clauses: [{ path: "eval.tool_duration_ms", op: "between", values: [String(bin.from), String(bin.to)] }] }) }));
    }),
    seriesPanel("Tool time by tool", { span: 6, by: "tool", metric: "eval.tool_duration_ms", agg: "p95", metrics: TIME_AGGS, type: "lines", height: 220, format: duration, info: "eval:tool_duration_ms" })));
  main.append(zone("tool-browser", "Browse tool calls", "Every call (the newest 500 in range and filter); open one for its trace, tokens, and events.",
    panel("Tool calls", { span: 12, flush: true, info: { title: "Tool calls", text: "Each row is one tool call (a trace with a tool). Columns can be added (output tokens, cache reads, member), filtered, sorted, and exported." } }, async (body, big, ctx) => { ctx.record("calls", calls, {}); put(body, callsTable(calls)); })));
}

// ---------------------------------------------------------------- AI agents: agents

/** An agent's profile in the details sidebar */
async function openAgent(id) {
  const clauses = [is("agent.agent_id", id)];
  const [byTool, calls, sessionData, decisions] = await Promise.all([usage("tool", { clauses }), toolCalls({ clauses }), api("/api/sessions").catch(() => ({ sessions: [] })), api("/api/facets", filterParams({ clauses: clauses.concat([is("flow", "F05")]) }))]);
  const totals = byTool.totals;
  const sessions = sessionData.sessions.filter((row) => row.session.agent_id === id);
  const provider = sessions[0] && sessions[0].session.provider;
  rememberAgents([{ agent_id: id, provider }]);
  details.open(agentName(id, provider), h("div", {},
    h("p", { class: "sentence" }, filterSentence({ what: "Events of agent " + id })),
    h("div", { class: "metric-grid", style: "margin-top:10px" }, [["tool calls", fmt(totals.calls)], ["tools used", fmt(totals.tools)], ["denied", fmt(totals.denied)], ["failure share", percent(totals.failure_share)], ["tokens", tokens(totals.tokens.total)], ["tokens per call", tokens(totals.tokens.per_call)], ["tool time p95", duration(totals.tool_ms.p95)], ["sessions", fmt(sessions.length)], ["last active", dt(totals.last)]].map(([label, value]) => h("div", {}, h("span", {}, label), h("b", {}, value)))),
    h("h4", {}, "Tool mix"), charts.toplist(byTool.groups.map((group) => ({ value: group.key, count: group.calls })), { onPick: openTool, hint: "The tool's profile", byValue: true }),
    h("h4", {}, "Decisions"), charts.donut((decisions.facets.decision || []).map((item) => ({ value: item.value, count: item.count })), { unit: "decisions", explain: explained }),
    h("h4", {}, "Waterfall of its newest calls"), charts.waterfall(calls.slice(0, 40), { packed: true, onPick: (call) => openTrace(call.id) }),
    h("h4", {}, "Sessions"), dataTable([
      { label: "Session", get: (row) => row.session.foxx_session_id, render: (row) => sessionLink(row.session.foxx_session_id, row.session) },
      { label: "Status", get: (row) => (row.stale ? "STALE" : row.session.status), render: (row) => chip(row.stale ? "STALE" : row.session.status) },
      { label: "Safety", get: (row) => row.session.safety, render: (row) => chip(row.session.safety) },
      { label: "Mode", get: (row) => row.session.autonomy_mode },
      { label: "Started", get: (row) => row.session.started_at, render: (row) => h("span", { class: "mono" }, dt(row.session.started_at)) },
    ], sessions, { noTools: true, sort: 4, pageSize: 8, empty: "No session of this agent is stored." }),
    h("div", { class: "row-actions", style: "margin-top:10px" }, h("button", { class: "btn", type: "button", onclick: () => { details.close(); drill({ clauses }); } }, "Open its events"), h("button", { class: "btn", type: "button", onclick: () => { details.close(); setClause("agent.agent_id", "is", [id]); render(); } }, "Filter every view by it"))), { wide: true, kind: "Agent" });
}
async function agents(main) {
  if (needRepository(main)) return;
  const [data, byAgent, sessionData] = await Promise.all([api("/api/agents"), usage("agent_id"), api("/api/sessions")]);
  const rows = data.agents;
  rememberAgents(rows);
  const sessions = sessionData.sessions;
  const stats = new Map(byAgent.groups.map((group) => [group.key, group]));
  const active = sessions.filter((row) => !row.stale && row.session.status === "ACTIVE").length;
  const totals = byAgent.totals;
  main.append(hero("AI agents", "Every agent identity that worked in this repository, what it did, what it spent, and how often Crane had to stop it.",
    [["Agents", rows.length], ["Sessions", sessions.length], ["Active now", active], ["Tool calls", compact(totals.calls)], ["Tokens", tokens(totals.tokens.total), "eval:model_tokens"], ["Quarantined", rows.reduce((sum, row) => sum + row.quarantined_sessions, 0), "value:QUARANTINED"]]));
  main.append(zone("fleet-kpis", "Fleet", "The agents' activity in range and filter (sessions over all time).",
    h("div", { class: "kpis span-12" },
      kpi("Agents active", totals.agents, { tone: "brand", note: rows.length + " known" }),
      kpi("Sessions", sessions.length, { note: active + " active" }),
      kpi("Tool calls", totals.calls, { info: "eval:tool_calls", drill: () => go("#/tools") }),
      kpi("Denial share", totals.denied / Math.max(1, totals.calls), { format: percent, tone: totals.denied ? "bad" : "good", info: "eval:denial_share" }),
      kpi("Tokens", totals.tokens.total, { format: tokens, info: "eval:model_tokens" }),
      kpi("Tokens per call", totals.tokens.per_call, { format: tokens }),
      kpi("Tool time p95", totals.tool_ms.p95, { format: duration, info: "eval:tool_duration_ms" }),
      kpi("Stale sessions", sessions.filter((row) => row.stale).length, { tone: sessions.some((row) => row.stale) ? "warnt" : "", info: "value:STALE" }))));
  main.append(zone("leaderboard", "Leaderboard", "Every agent identity ranked by tool calls, with its time, tokens, failures, and risk. Click a row for its profile.",
    panel("Agent leaderboard", { span: 12, flush: true, info: { title: "Agent leaderboard", text: "Per agent identity: tool calls, tools used, denial and failure shares, tool time, tokens, and sessions (all time) with quarantines and bypass attempts." } }, async (body, big, ctx) => {
      ctx.record("usage", byAgent, { by: "agent identity" });
      const list = rows.map((row) => Object.assign({}, row, { usage: stats.get(row.agent_id) || null }));
      for (const group of byAgent.groups) if (!rows.some((row) => row.agent_id === group.key) && group.key !== "NONE") list.push({ agent_id: group.key, provider: "", sessions: 0, quarantined_sessions: 0, evals: {}, usage: group });
      put(body, dataTable([
        { label: "Agent", get: (row) => row.agent_id, render: (row) => agentLink(row.agent_id, row.provider) },
        { label: "Provider", get: (row) => row.provider, filter: true },
        { label: "Tool calls", get: (row) => (row.usage ? row.usage.calls : 0), num: true, bar: true },
        { label: "Tools", get: (row) => (row.usage ? row.usage.tools : 0), num: true },
        { label: "Denied", get: (row) => (row.usage ? row.usage.denied : 0), num: true },
        { label: "Denial share", get: (row) => (row.evals || {}).denial_share, render: (row) => percent((row.evals || {}).denial_share), num: true, info: "eval:denial_share" },
        { label: "Failure share", get: (row) => (row.usage ? row.usage.failure_share : null), render: (row) => percent(row.usage && row.usage.failure_share), num: true },
        { label: "Time p95", get: (row) => (row.usage ? row.usage.tool_ms.p95 : null), render: (row) => duration(row.usage && row.usage.tool_ms.p95), num: true },
        { label: "Tokens", get: (row) => (row.usage ? row.usage.tokens.total : 0), render: (row) => tokens(row.usage && row.usage.tokens.total), num: true, bar: true, format: tokens },
        { label: "Sessions", get: (row) => row.sessions, num: true },
        { label: "Quarantined", get: (row) => row.quarantined_sessions, num: true },
        { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true, info: "eval:bypass_attempts" },
        { label: "Last seen", get: (row) => (row.usage ? row.usage.last : null), render: (row) => h("span", { class: "mono" }, row.usage ? dt(row.usage.last) : fmt(row.last_seen)) },
      ], list, { sort: 2, onRow: (row) => openAgent(row.agent_id), name: "foxx-agent-leaderboard", placeholder: "Search agents…" }));
    })));
  main.append(zone("agent-activity", "Activity", "When each agent worked.",
    seriesPanel("Events by agent", { span: 12, by: "agent.agent_id", type: "bars", height: 200, info: { title: "Events by agent", text: "Events over time, one series per agent identity (the 8 most active)." } }),
    ganttPanel("Actions by agent", { rows: "agent.agent_id", color: "tool" })));
  main.append(zone("agent-tools", "Tools per agent", "Which tools each agent calls, and how often.",
    matrixPanel("Agent by tool", { span: 7, rows: "agent.agent_id", cols: "tool", clauses: [is("flow", "F05")] }),
    breakdownPanel("Tool mix", { span: 5, by: "tool", view: "toplist", clauses: [is("flow", "F05")], info: "facet:tool" })));
  main.append(zone("agent-decisions", "Decisions and risk", "How Crane's decisions divide per agent, and which agents trip the protections.",
    matrixPanel("Agent by decision", { span: 6, rows: "agent.agent_id", cols: "decision", clauses: [is("flow", "F05")] }),
    panel("Agent reliability comparison", { span: 6, info: { title: "Agent reliability comparison", text: "Each dot compares an agent's call volume (across) with its failed-call share (up); the dot grows with denied calls. Hover for denial share, bypass attempts, and quarantined sessions. Click a dot for its profile." } }, async (body, big, ctx) => {
      const points = rows.map((row) => { const group = stats.get(row.agent_id); return { x: group ? group.calls : 0, y: (group ? group.failure_share : 0) * 100, key: agentName(row.agent_id, row.provider), label: agentName(row.agent_id, row.provider), id: row.agent_id, r: 4 + Math.min(8, group ? group.denied : 0), rows: [["denial share", percent((row.evals || {}).denial_share)], ["bypass attempts", fmt((row.evals || {}).bypass_attempts)], ["quarantined sessions", fmt(row.quarantined_sessions)]] }; });
      ctx.record("value", { agents: rows.length, "busiest calls": Math.max(0, ...points.map((point) => point.x)), "highest denial share %": Math.max(0, ...points.map((point) => point.y)) }, { lines: ["Denial share is the share of the agent's decided tool calls that Crane denied."] });
      put(body, charts.scatter(points, { height: big ? 420 : 240, xLabel: "tool calls", yLabel: "failed-call share %", keyLabel: "agent", yFormat: (value) => value.toFixed(0) + "%", onPick: (point) => openAgent(point.id) }));
    }),
    breakdownPanel("Bypass attempts by agent", { span: 6, by: "agent.agent_id", view: "toplist", clauses: [is("severity.alert_kind", "BYPASS_ATTEMPT")], info: "eval:bypass_attempts" }),
    breakdownPanel("Denials by agent", { span: 6, by: "agent.agent_id", view: "toplist", clauses: [is("decision.decision", "DENY")], info: "eval:denied" })));
  main.append(zone("agent-tokens", "Tokens", "What each agent's model turns burnt.",
    seriesPanel("Tokens by agent", { span: 8, by: "agent.agent_id", metric: "eval.model_tokens", agg: "sum", metrics: TOKEN_AGGS, height: 220, info: "eval:model_tokens" }),
    panel("Tokens per call by agent", { span: 4, info: { title: "Tokens per call by agent", text: "The average tokens a tool call of each agent cost (its share of the issuing turn)." } }, async (body, big, ctx) => {
      const items = byAgent.groups.filter((group) => group.tokens.per_call !== null).map((group) => ({ value: group.key, count: Math.round(group.tokens.per_call) }));
      ctx.record("items", items, { unit: "tokens per call" });
      put(body, charts.toplist(items, { format: tokens, onPick: openAgent, hint: "The agent's profile", byValue: true, empty: "No agent reported tokens." }));
    })));
  main.append(zone("agent-models", "Models and autonomy", "Which models the agents ran, and in which autonomy mode their sessions worked.",
    breakdownPanel("Models", { span: 4, by: "agent.model", info: { title: "Models", text: "The model each event's agent reported." } }),
    panel("Autonomy modes", { span: 4, info: { title: "Autonomy modes", text: "How the sessions were configured: how much an agent may do without approval. Sessions over all time." }, scope: { kind: "state", what: "Sessions" } }, async (body, big, ctx) => {
      const counts = {};
      for (const row of sessions) counts[row.session.autonomy_mode] = (counts[row.session.autonomy_mode] || 0) + 1;
      const items = Object.entries(counts).map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
      ctx.record("items", items, { unit: "sessions" });
      put(body, charts.donut(items, { unit: "sessions", explain: explained }));
    }),
    panel("Session safety", { span: 4, info: { title: "Session safety", text: "Each session's safety state: normal, degraded (a rule broke after an action), or quarantined (protected code was changed around the checks)." }, scope: { kind: "state", what: "Sessions" } }, async (body, big, ctx) => {
      const counts = {};
      for (const row of sessions) counts[row.session.safety] = (counts[row.session.safety] || 0) + 1;
      const items = Object.entries(counts).map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
      ctx.record("items", items, { unit: "sessions" });
      put(body, charts.donut(items, { unit: "sessions", explain: explained }));
    })));
  main.append(zone("agent-list", "Agent identities", "Every agent identity across all its sessions (all time).", panel("Agents", { span: 12, flush: true, scope: { kind: "state", what: "Agent identities" } }, async (body, big, ctx) => { ctx.record("rows", rows); put(body, dataTable([
    { label: "Agent", get: (row) => row.agent_id, render: (row) => agentLink(row.agent_id, row.provider) }, { label: "Provider", get: (row) => row.provider, filter: true },
    { label: "Sessions", get: (row) => row.sessions, num: true }, { label: "Quarantined", get: (row) => row.quarantined_sessions, num: true },
    { label: "Tool calls decided", get: (row) => (row.evals || {}).tool_calls_decided, num: true, info: "eval:tool_calls_decided" }, { label: "Denial share", get: (row) => (row.evals || {}).denial_share, render: (row) => percent((row.evals || {}).denial_share), num: true, info: "eval:denial_share" },
    { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true, info: "eval:bypass_attempts" }, { label: "Confirmed bypasses", get: (row) => (row.evals || {}).bypasses_confirmed, num: true, info: "eval:bypasses_confirmed" },
    { label: "Credits consumed", get: (row) => ((row.evals || {}).credits || {}).consumed, num: true }, { label: "Last seen", get: (row) => row.last_seen, render: (row) => fmt(row.last_seen) },
  ], rows, { sort: 2, onRow: (row) => openAgent(row.agent_id), placeholder: "Search agents…", name: "foxx-agents" })); })));
}

// ---------------------------------------------------------------- AI agents: sessions

async function sessions(main) {
  if (needRepository(main)) return;
  const [data, bySession] = await Promise.all([api("/api/sessions"), usage("session")]);
  const rows = data.sessions;
  rememberAgents(rows.map((row) => ({ agent_id: row.session.agent_id, provider: row.session.provider })));
  const stats = new Map(bySession.groups.map((group) => [group.key, group]));
  const active = rows.filter((row) => !row.stale && row.session.status === "ACTIVE").length;
  main.append(zone("session-kpis", "Summary", "Sessions are counted over all time; the charts below follow the range and filter.", h("div", { class: "kpis span-12" },
    kpi("Sessions", rows.length, { tone: "brand" }), kpi("Active", active, { tone: "good" }), kpi("Stale", rows.filter((row) => row.stale).length, { tone: rows.some((row) => row.stale) ? "bad" : "", info: "value:STALE" }),
    kpi("Quarantined", rows.filter((row) => row.session.safety === "QUARANTINED").length, { tone: rows.some((row) => row.session.safety === "QUARANTINED") ? "crit" : "", info: "value:QUARANTINED" }),
    kpi("Tool calls", bySession.totals.calls, { info: "eval:tool_calls" }),
    kpi("Tokens", bySession.totals.tokens.total, { format: tokens, info: "eval:model_tokens" }),
    kpi("Denied actions", rows.reduce((sum, row) => sum + ((row.evals || {}).denied || 0), 0), { info: "eval:denied", drill: () => drill({ clauses: [is("decision.decision", "DENY")] }) }))));
  main.append(zone("session-activity", "Activity", "Session events over time, grouped by agent type, and the number of sessions active in each time bucket.", seriesPanel("Session activity", { span: 8, by: "agent", type: "area", height: 200 }), breakdownPanel("By agent type", { span: 4, by: "agent", info: "facet:agent" }),
    seriesPanel("Active sessions", { span: 12, metric: "session.session_id", agg: "distinct", type: "lines", height: 160, info: { title: "Active sessions", text: "How many different sessions produced an event in each time bucket." } })));
  main.append(zone("session-leaderboard", "Leaderboard", "Sessions ranked by tool calls in range and filter, with their time and tokens.",
    panel("Session leaderboard", { span: 12, flush: true, info: { title: "Session leaderboard", text: "Per session: tool calls, denials, failures, tool time, and tokens (in range and filter)." } }, async (body, big, ctx) => {
      ctx.record("usage", bySession, { by: "session" });
      put(body, dataTable([
        { label: "Session", get: (row) => row.key, render: (row) => sessionLink(row.key, rows.find((item) => item.session.foxx_session_id === row.key)?.session) },
        { label: "Calls", get: (row) => row.calls, num: true, bar: true },
        { label: "Tools", get: (row) => row.tools, num: true },
        { label: "Denied", get: (row) => row.denied, num: true },
        { label: "Failed", get: (row) => row.failed, num: true },
        { label: "Time total", get: (row) => row.tool_ms.total, render: (row) => duration(row.tool_ms.total), num: true },
        { label: "Time p95", get: (row) => row.tool_ms.p95, render: (row) => duration(row.tool_ms.p95), num: true },
        { label: "Tokens", get: (row) => row.tokens.total, render: (row) => tokens(row.tokens.total), num: true, bar: true, format: tokens },
        { label: "Per call", get: (row) => row.tokens.per_call, render: (row) => tokens(row.tokens.per_call), num: true },
        { label: "First", get: (row) => row.first, render: (row) => h("span", { class: "mono" }, dt(row.first)) },
        { label: "Last", get: (row) => row.last, render: (row) => h("span", { class: "mono" }, dt(row.last)) },
      ], bySession.groups.filter((group) => group.key !== "NONE"), { sort: 1, onRow: (row) => go("#/sessions/" + encodeURIComponent(row.key)), name: "foxx-session-leaderboard", placeholder: "Search sessions…" }));
    })));
  main.append(zone("session-waterfall", "Waterfall", "The tool calls of one session (or all), one after another.", waterfallPanel("Session waterfall", { sessions: true })));
  main.append(zone("session-list", "Sessions", "", panel("Sessions", { span: 12, flush: true, scope: { kind: "state", what: "Sessions" } }, async (body, big, ctx) => { ctx.record("rows", rows); put(body, dataTable([
    { label: "Session", get: (row) => row.session.foxx_session_id, render: (row) => sessionLink(row.session.foxx_session_id, row.session) },
    { label: "Member", get: (row) => (row.member ? row.member.actor : ""), render: (row) => member(row.member), filter: true },
    { label: "Agent", get: (row) => row.session.agent_id, render: (row) => agentLink(row.session.agent_id, row.session.provider), filter: true },
    { label: "Provider", get: (row) => row.session.provider, filter: true }, { label: "Model", get: (row) => row.session.model, filter: true },
    { label: "Status", get: (row) => (row.stale ? "STALE" : row.session.status), render: (row) => chip(row.stale ? "STALE" : row.session.status), filter: true },
    { label: "Safety", get: (row) => row.session.safety, render: (row) => chip(row.session.safety), filter: true }, { label: "Mode", get: (row) => row.session.autonomy_mode, filter: true },
    { label: "Tool calls", get: (row) => (stats.get(row.session.foxx_session_id) || {}).calls || 0, num: true, bar: true },
    { label: "Tokens", get: (row) => ((stats.get(row.session.foxx_session_id) || {}).tokens || {}).total || 0, render: (row) => tokens(((stats.get(row.session.foxx_session_id) || {}).tokens || {}).total), num: true, info: "eval:model_tokens" },
    { label: "Denied", get: (row) => (row.evals || {}).denied, num: true, info: "eval:denied" }, { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true, info: "eval:bypass_attempts", hidden: true },
    { label: "Credits", get: (row) => row.credits.available, render: (row) => row.credits.available + " / " + row.credits.granted, num: true, info: "eval:credits" },
    { label: "Started", get: (row) => row.session.started_at, render: (row) => h("span", { class: "mono" }, dt(row.session.started_at)) },
    { label: "Last seen", get: (row) => row.session.last_seen_at, render: (row) => h("span", { class: "mono" }, dt(row.session.last_seen_at)) },
  ], rows, { sort: 14, dir: -1, onRow: (row) => go("#/sessions/" + encodeURIComponent(row.session.foxx_session_id)), placeholder: "Search sessions, members, models…", name: "foxx-sessions" })); })));
}
function graphView(graph) {
  const nodes = graph.nodes || [];
  if (!nodes.length) return h("div", { class: "empty" }, "No events in this session.");
  const verified = new Set((graph.edges || []).filter((edge) => edge.kind === "verified_by").map((edge) => edge.to));
  const tone = (node) => (node.flags.includes("bypass_confirmed") ? "#9c1c4c" : node.flags.includes("violation") ? Foxx.STATUS.serious : node.flags.includes("denied") ? Foxx.STATUS.critical : node.flags.includes("coverage_gap") || node.flags.includes("unresolved") ? Foxx.STATUS.warning : node.kind === "verification" ? Foxx.STATUS.good : node.kind === "decision" ? Foxx.PALETTE[0] : Foxx.STATUS.neutral);
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
  const stats = await usage("tool", { clauses: only, ignoreTime: true });
  if (data.stale) main.append(h("div", { class: "notice warn" }, "This session stopped reporting: no hook activity for a long time and no signed end of session. The agent may have been stopped, its hooks removed, or its evidence is not reaching the service."));
  const download = (path, name) => async () => {
    const response = await fetch(scoped(path), { credentials: "same-origin" });
    const anchor = h("a", { href: URL.createObjectURL(await response.blob()), download: name });
    document.body.append(anchor); anchor.click(); anchor.remove();
  };
  main.append(hero(h("span", { class: "identity-link" }, "Session " + shortIdentity(sessionInfo.foxx_session_id), info("", { title: "Session " + shortIdentity(sessionInfo.foxx_session_id), text: [sessionInfo.provider, sessionInfo.model || "model not reported", "safety " + sessionInfo.safety, sessionInfo.autonomy_mode + " mode", data.member ? data.member.actor : "unknown member", "started " + dt(sessionInfo.started_at), sessionInfo.ended_at ? "ended " + dt(sessionInfo.ended_at) : "open"].join(" · ") })), [sessionInfo.provider, sessionInfo.model || "model not reported", data.member ? data.member.actor : "unknown member", "started " + dt(sessionInfo.started_at), sessionInfo.ended_at ? "ended " + dt(sessionInfo.ended_at) : "open"].join(" · "),
    [["Safety", sessionInfo.safety, "value:" + sessionInfo.safety], ["Mode", sessionInfo.autonomy_mode], ["Status", data.stale ? "STALE" : sessionInfo.status], ["Tool calls", compact(stats.totals.calls)], ["Tokens", tokens(stats.totals.tokens.total), "eval:model_tokens"], ["Credits", data.credits.available + " / " + data.credits.granted, "eval:credits"]],
    [h("button", { class: "btn", type: "button", onclick: download("/api/reports/session/" + sessionInfo.foxx_session_id, "crane-report-" + sessionInfo.foxx_session_id + ".json") }, "Report"),
      h("button", { class: "btn", type: "button", onclick: download("/api/evidence/session/" + sessionInfo.foxx_session_id, "crane-evidence-" + sessionInfo.foxx_session_id + ".json") }, "Evidence bundle"),
      h("button", { class: "btn", type: "button", onclick: () => drill({ clauses: only }) }, "Open in events"), sessionInfo.agent_id ? h("button", { class: "btn", type: "button", onclick: () => openAgent(sessionInfo.agent_id) }, "Agent profile") : null, h("button", { class: "btn", type: "button", onclick: () => go("#/credits") }, "Grant credits")]));
  main.append(h("div", { class: "kpis" },
    kpi("Events", evals.events, { info: "eval:events" }), kpi("Tool calls decided", evals.tool_calls_decided, { info: "eval:tool_calls_decided" }), kpi("Denied", evals.denied, { tone: evals.denied ? "bad" : "", info: "eval:denied" }), kpi("Bypass attempts", evals.bypass_attempts, { tone: evals.bypass_attempts ? "bad" : "", info: "eval:bypass_attempts" }),
    kpi("Confirmed bypasses", evals.bypasses_confirmed, { tone: evals.bypasses_confirmed ? "crit" : "good", info: "eval:bypasses_confirmed" }), kpi("Evidence completeness", evals.evidence_completeness, { format: percent, info: "eval:evidence_completeness" }),
    kpi("Tool time total", stats.totals.tool_ms.total, { format: duration, info: "eval:tool_duration_ms" }), kpi("Tokens per call", stats.totals.tokens.per_call, { format: tokens, info: "eval:model_tokens" })));
  main.append(h("div", { class: "grid" }, waterfallPanel("Waterfall of this session's tool calls", { clauses: only }),
    panel("Tool mix", { span: 4, info: { title: "Tool mix", text: "How many calls of each tool this session made." }, scope: { what: "Tool calls of this session", ignoreTime: true } }, async (body, big, ctx) => { ctx.record("usage", stats, { by: "tool" }); put(body, charts.toplist(stats.groups.map((group) => ({ value: group.key, count: group.calls })), { onPick: openTool, byValue: true, empty: "No tool calls." })); }),
    seriesPanel("Tokens over the session", { span: 8, by: "tool", metric: "eval.model_tokens", agg: "sum", metrics: TOKEN_AGGS, clauses: only, params: { since: "", until: "" }, height: 200, info: "eval:model_tokens" })));
  const sessionGantt = ganttPanel("Actions of this session", { rows: "tool", color: "decision", clauses: only, ignoreTime: true });
  main.append(h("div", { class: "grid" }, sessionGantt, seriesPanel("Session timeline", { span: 8, by: "flow", clauses: only, params: { since: "", until: "" }, height: 200 }), breakdownPanel("Decisions in this session", { span: 4, by: "decision", clauses: only })));
  main.append(h("div", { class: "grid" }, panel("Action graph", { span: 7, scope: { kind: "state", what: "The session's events" } }, async (body, big, ctx) => { ctx.record("rows", graph.nodes || []); put(body, graphView(graph)); }), panel("Evals", { span: 5, flush: true, scope: { kind: "state", what: "The session's evals" } }, async (body, big, ctx) => { ctx.record("value", Object.fromEntries(Object.entries(evals).filter(([, value]) => typeof value === "number"))); put(body, evalsTable(evals)); })));
  main.append(h("div", { class: "grid" }, panel("Tasks", { span: 12, flush: true, scope: { kind: "state", what: "The session's tasks" } }, async (body, big, ctx) => { ctx.record("rows", data.tasks || []); put(body, dataTable([
    { label: "Task", get: (row) => row.task.foxx_task_id, render: (row) => h("code", {}, row.task.foxx_task_id) }, { label: "External", get: (row) => row.task.external_task_id },
    { label: "Status", get: (row) => row.task.status, render: (row) => chip(row.task.status), filter: true }, { label: "Tool calls", get: (row) => row.task.tool_calls, num: true },
    { label: "Denied", get: (row) => (row.evals || {}).denied, num: true }, { label: "Started", get: (row) => row.task.started_at, render: (row) => h("span", { class: "mono" }, dt(row.task.started_at)) },
  ], data.tasks || [], { sort: 5, dir: 1, noColumns: true })); })));
}

// ---------------------------------------------------------------- topology

const LAYER_PRESETS = [["member,agent,tool,resource.resource", "Members → agents → tools → resources"], ["agent.agent_id,tool,decision", "Agents → tools → decisions"], ["agent.agent_id,agent.model,tool", "Agents → models → tools"], ["tool,decision,decision.decision_rule", "Tools → decisions → rules"], ["member,agent.agent_id,session.session_id", "Members → agents → sessions"], ["flow,event,severity", "Flows → events → severities"], ["tool,resource.resource_ids", "Tools → resources"]];
async function topologyPage(main) {
  if (needRepository(main)) return;
  const state = { layers: LAYER_PRESETS[0][0] };
  const topo = panel("Topology", { span: 12, filterable: true, info: "page:topology", scope: () => ({ notes: ["Layers: " + (LAYER_PRESETS.find(([key]) => key === state.layers) || [0, state.layers])[1] + ". The 14 largest nodes of each layer."] }) }, async (body, big, ctx) => {
    const data = await api("/api/topology", filterParams({ clauses: ctx.clauses(), layers: state.layers }));
    ctx.record("topology", data, { layers: state.layers });
    const keys = data.layer_keys || state.layers.split(",");
    put(body, charts.topology(data, { onPick: (node) => {
      const path = facetPath(keys[node.layer]);
      const value = path === "member_key" ? (members.find((row) => node.label.startsWith(row.member.actor + " (" + row.member.short)) || { member: { key: node.label } }).member.key : node.label;
      if (path === "agent.agent_id") { openAgent(value); return; }
      if (path === "action.action_tool") { openTool(value); return; }
      details.open(node.label, h("div", {}, h("div", { class: "metric-grid" }, [["layer", data.layers[node.layer]], ["events", fmt(node.count)], ["denied", fmt(node.denied)], ["denied share", percent(node.denied / Math.max(1, node.count))]].map(([label, text]) => h("div", {}, h("span", {}, label), h("b", {}, text)))),
        h("h4", {}, "Connected to"), h("ul", {}, data.edges.filter((edge) => edge.from === node.id || edge.to === node.id).sort((a, b) => b.count - a.count).slice(0, 12).map((edge) => h("li", {}, (edge.from === node.id ? "→ " + edge.to : "← " + edge.from).replace(/\d:/, "") + " · " + fmt(edge.count) + " events" + (edge.denied ? ", " + fmt(edge.denied) + " denied" : "")))),
        h("div", { class: "row-actions" }, h("button", { class: "btn", type: "button", onclick: () => { details.close(); drill({ clauses: [is(path, value)] }); } }, "Open its events"), h("button", { class: "btn", type: "button", onclick: () => { details.close(); setClause(path, "is", [value]); render(); } }, "Filter every view by it"))), { kind: "Node · " + data.layers[node.layer] });
    } }));
  });
  topo.controls.append(select("Layers", state.layers, LAYER_PRESETS, (value) => { state.layers = value; topo.refresh(); }));
  main.append(zone("who", "Who used what", "Any chain of attributes: each event walks the layers; edge width is how many events joined two nodes, red the share denied.", topo));
  main.append(zone("matrices", "Matrices", "Any two attributes against each other.",
    matrixPanel("Member by agent", { span: 6, rows: "member", cols: "agent.agent_id" }),
    matrixPanel("Tool by decision", { span: 6, rows: "tool", cols: "decision", clauses: [is("flow", "F05")] }),
    matrixPanel("Flow by severity", { span: 12, rows: "flow", cols: "severity", limit: 20 })));
  main.append(zone("hotspots", "Hotspots", "The resources agents touch most, and those where Crane stopped them.",
    breakdownPanel("Most touched resources", { span: 6, by: "resource.resource_ids", view: "toplist", limit: 15, clauses: [is("flow", "F05")], info: { title: "Most touched resources", text: "Files, hosts, and other resources named by tool calls, by how many decisions named them." } }),
    breakdownPanel("Denied resources", { span: 6, by: "resource.resource_ids", view: "toplist", limit: 15, clauses: [is("decision.decision", "DENY")], info: { title: "Denied resources", text: "Resources named by tool calls Crane denied." } })));
  main.append(zone("dependencies", "Flow dependencies", "Within each session, which flow's event tends to follow which (order only, never a cause).",
    panel("Flow dependency graph", { span: 12, filterable: true, info: { title: "Flow dependency graph", text: "Each circle is a flow sized by its events; each arrow is how often an event of one flow was followed by an event of another in the same session. Hover a flow to follow its arrows; click it to filter by it." } }, async (body, big, ctx) => {
      const data = await api("/api/dependencies", filterParams({ clauses: ctx.clauses() }));
      ctx.record("topology", { layers: ["flows"], nodes: data.nodes.map((node) => ({ id: "0:" + node.id, label: node.id, count: node.count, denied: 0 })), edges: data.edges.map((edge) => ({ from: "0:" + edge.from, to: "0:" + edge.to, count: edge.count, denied: 0 })) }, {});
      put(body, charts.dependency(data, { title_of: (flow) => (G.flows[flow] ? G.flows[flow][0] : ""), onPick: (flow) => { setClause("flow", "is", [flow]); render(); } }), dataTable([
        { label: "From", get: (row) => row.from, render: (row) => h("span", {}, row.from, info("flow:" + row.from)) }, { label: "To", get: (row) => row.to, render: (row) => h("span", {}, row.to, info("flow:" + row.to)) }, { label: "Times", get: (row) => row.count, num: true, bar: true },
      ], data.edges, { sort: 2, pageSize: 10, noColumns: true, name: "foxx-flow-dependencies" }));
    })));
  let governance = null;
  try { governance = await api("/api/governance"); } catch (error) { governance = null; }
  main.append(zone("code", "Code topology", "The repository's zones and flows (governance.yaml) as a member's machine resolved them: what code each covers, and the call graph of each flow.", ...codeTopology(governance)));
}

// ---------------------------------------------------------------- distributions and texts

async function distributions(main) {
  if (needRepository(main)) return;
  const state = { field: "trace_duration_ms", bins: "24" };
  const labels = { trace_duration_ms: "Trace duration", session_gap_ms: "Gap between a session's events", events_per_session: "Events per session", "eval.model_tokens": "Tokens per call", "eval.tool_duration_ms": "Tool time", "eval.latency_ms": "Hook latency" };
  let ready = false;
  const hist = panel("Histogram", { span: 12, info: { title: "Histogram", text: "How a number is spread over the matching events: trace durations, the gap between a session's consecutive events, events per session, tokens per call, or any numeric eval attribute. The median (p50) and 95th percentile (p95) are marked. For an eval attribute, click a bar to filter by its range." }, scope: () => ({ notes: ["The values of " + (labels[state.field] || state.field) + " in " + state.bins + " bins."] }) }, async (body, big, ctx) => {
    const data = await api("/api/histogram", filterParams({ field: state.field, bins: state.bins }));
    if (!ready) {
      ready = true;
      hist.controls.append(select("Field", state.field, [...new Set(Object.keys(labels).concat(data.fields || []))].map((field) => [field, labels[field] || field]), (value) => { state.field = value; hist.refresh(); }), select("Bins", state.bins, [["12", "12 bins"], ["24", "24 bins"], ["48", "48 bins"]], (value) => { state.bins = value; hist.refresh(); }));
    }
    const millis = /_ms$/.test(state.field);
    ctx.record("histogram", data, { field: state.field, format: millis ? duration : compact });
    put(body, charts.histogram(data, { height: big ? 420 : 240, format: millis ? duration : compact, onPick: state.field.startsWith("eval.") ? (bin) => drill({ clauses: [{ path: state.field, op: "between", values: [String(bin.from), String(bin.to)] }] }) : null }));
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
    panel("Statement words", { span: 12 }, async (body, big, ctx) => { ctx.record("items", data.words.map((word) => ({ value: word.value, count: word.count })), { unit: "uses" }); put(body, data.words.length ? h("div", { class: "cloud" }, data.words.map((word) => h("button", { type: "button", class: "word", style: "font-size:" + (11 + (word.count / max) * 18).toFixed(0) + "px", title: word.count + " times", onclick: () => { filter.q = word.value; saveFilter(); go("#/logs"); } }, word.value))) : h("div", { class: "empty" }, "No statements.")); })));
  const group = (title, key, explainText) => panel(title, { span: 6, info: { title, text: explainText } }, async (body, big, ctx) => {
    const items = (data.groups[key] || []).map((item) => ({ value: item.value, count: item.count }));
    const path = ((data.groups[key] || [])[0] || {}).path;
    ctx.record("items", items, {});
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
    panel("Log lines", { span: 12, flush: true }, async (body, big, ctx) => { ctx.record("rows", data.lines); put(body, h("div", { class: "loglines" }, data.lines.map((line) => h("div", { class: "logline", role: "button", tabindex: "0", onclick: () => openEventById(line.id), onkeydown: (event) => { if (event.key === "Enter") openEventById(line.id); } },
      h("span", { class: "mono muted" }, dt(line.t)), h("span", { class: "chip " + (TONES[line.severity] || "") }, line.severity || "—"), h("span", { class: "mono" }, line.flow), h("code", {}, line.name), h("span", {}, line.text || ""), line.reasons && line.reasons.length ? h("span", { class: "muted" }, "· " + line.reasons.join("; ")) : null)))); })));
}

Object.assign(window.FOXX_PAGES, { overview, explorer, timeline, tools, agents, sessions, session, logs, traces, topology: topologyPage, distributions, semantics });
