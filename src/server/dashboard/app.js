"use strict";
// The Foxx dashboard page: sign in with GitHub, choose an account (personal or organization), a
// repository, and optionally one team member, then browse every view. A time range and a global
// filter (facets such as severity or flow, a session, and free text) apply to every graph, list,
// and eval; each graph also has its own group-by and filter. Charts are drawn here as SVG, since
// the page loads no other script. Everything is read-only.

// ---------------------------------------------------------------- state

const NAV = [
  ["Monitor", [["overview", "Overview", "M3 3h7v7H3zM14 3h7v4h-7zM14 10h7v11h-7zM3 13h7v8H3z"], ["events", "Events", "M4 5h16M4 10h16M4 15h10M4 20h7"], ["alerts", "Alerts", "M12 3a6 6 0 0 1 6 6v4l2 3H4l2-3V9a6 6 0 0 1 6-6zM10 19a2 2 0 0 0 4 0"]]],
  ["Investigate", [["sessions", "Sessions", "M4 4h16v12H4zM8 20h8M12 16v4"], ["agents", "Agents", "M12 4a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM4 20c1-4 4-6 8-6s7 2 8 6"], ["audit", "Audit trail", "M6 3h9l4 4v14H6zM9 9h6M9 13h6M9 17h4"]]],
  ["Govern", [["governance", "Governance", "M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z"], ["compliance", "Compliance", "M5 12l4 4 10-10"]]],
  ["Organization", [["repositories", "Repositories", "M4 4h6l2 2h8v14H4z"], ["settings", "Settings", "M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2"]]],
];
const RANGES = [["15m", 15 * 60e3], ["1h", 60 * 60e3], ["4h", 4 * 3600e3], ["1d", 86400e3], ["7d", 7 * 86400e3], ["30d", 30 * 86400e3], ["All", null]];
const FACETS = ["severity", "flow", "decision", "enforcement", "alert", "origin", "agent", "source", "event", "response", "tool", "member"];
const FACET_LABELS = { severity: "Severity", flow: "Flow", decision: "Decision", enforcement: "Enforcement", alert: "Alert kind", origin: "Origin", agent: "Agent type", source: "Action source", event: "Event name", response: "Response", tool: "Tool", member: "Member" };
// The thirteen attribute groups every event carries (DASHBOARD.md)
const GROUPS = ["action", "repository", "session", "agent", "resource", "response", "assessment", "decision", "eval", "severity", "bindings", "telemetry", "other"];
// Where each facet lives in an event, so a value in the event drawer can become a filter
const FACET_FIELDS = { "severity.severity_status": "severity", "decision.decision": "decision", "decision.enforcement": "enforcement", "severity.alert_kind": "alert", "agent.agent_type": "agent", "action.action_source": "source", "response.response_status": "response", "action.action_tool": "tool" };
const PALETTE = ["#7b5fc7", "#3fa9f5", "#2fbf9c", "#f2b33d", "#ef6461", "#e573b1", "#9ccc65", "#5c6bc0", "#ff9f43"];
const NAMED = {
  INFO: "#3f8fe0", LOW: "#26b3b3", MEDIUM: "#f0a830", HIGH: "#ee6a4f", CRITICAL: "#c2185b",
  ALLOW: "#2fbf7c", DENY: "#e5484d", REQUIRE_APPROVAL: "#f0a830", QUARANTINE: "#c2185b", ERROR: "#a33a3a", NOT_REQUIRED: "#9aa0b4",
  MACHINE: "#7b5fc7", SERVICE: "#3fa9f5", DERIVED: "#2fbf9c", WORKFLOW: "#f2b33d",
  PREVENTED: "#2fbf7c", DETECTED_AFTER_EFFECT: "#ee6a4f", BYPASS_CONFIRMED: "#c2185b", COVERAGE_GAP: "#f0a830", NOT_APPLICABLE: "#9aa0b4",
  SATISFIED: "#2fbf7c", NOT_SATISFIED: "#e5484d", NO_EVIDENCE: "#f0a830", UNSUPPORTED: "#9aa0b4", OTHER: "#9aa0b4", NONE: "#9aa0b4",
};
const TONES = {
  ALLOW: "ok", COMPLIANT: "ok", SATISFIED: "ok", PASS: "ok", ACTIVE: "ok", OBSERVED: "ok", verified: "ok", current: "ok", LINKED: "ok", OK: "ok", SUCCESS: "ok", COMPLETED: "ok", SIGNED: "ok", PREVENTED: "ok", OPENED: "ok", SENT: "ok", VERIFIED: "ok", ENDED: "info",
  INFO: "info", MACHINE: "info", SERVICE: "info", DERIVED: "info", WORKFLOW: "info", LOW: "low",
  REQUIRE_APPROVAL: "warn", DEGRADED: "warn", PARTIAL: "warn", INFERRED: "warn", PENDING: "warn", SUSPICIOUS: "warn", COVERAGE_GAP: "warn", stale: "warn", STALE: "warn", NO_EVIDENCE: "warn", MEDIUM: "warn", UNLINKED: "warn", UNSIGNED: "warn", UNKNOWN: "warn", SKIPPED: "warn",
  DENY: "bad", VIOLATION: "bad", FAIL: "bad", NOT_SATISFIED: "bad", DETECTED_AFTER_EFFECT: "bad", UNRESOLVED: "bad", HIGH: "bad", ERROR: "bad", missing: "bad", broken: "bad", FAILED: "bad", FORBIDDEN: "bad", BLOCKED: "bad", BYPASS_ATTEMPT: "bad", VIOLATION_AFTER_EFFECT: "bad", REFUSED: "bad",
  QUARANTINE: "crit", QUARANTINED: "crit", BYPASS_CONFIRMED: "crit", CRITICAL: "crit", EVIDENCE_CHAIN_BROKEN: "crit",
};

const scope = { account: "", repository: "", member: "" };
const filter = { range: "7d", since: null, until: null, q: "", session: "", facets: {} };
let me = null;
let members = [];
let facetCache = null;
let lastEvals = null;
let evalQuery = "";
let approval = null;
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
storage.load("foxx-filter", filter);
if (!filter.facets || typeof filter.facets !== "object") filter.facets = {};
const saveScope = () => storage.save("foxx-scope", scope);
const saveFilter = () => storage.save("foxx-filter", filter);

// ---------------------------------------------------------------- DOM helpers

function h(tag, attributes, ...children) {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attributes || {})) {
    if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
    else if (key === "dataset") Object.assign(node.dataset, value);
    else if (value !== undefined && value !== null && value !== false) node.setAttribute(key, value === true ? "" : value);
  }
  for (const child of children.flat(Infinity)) {
    if (child === null || child === undefined || child === false) continue;
    node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  }
  return node;
}
function put(node, ...children) {
  node.replaceChildren(...children.flat(Infinity).filter((child) => child !== null && child !== undefined && child !== false));
  return node;
}
function svg(tag, attributes, ...children) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [key, value] of Object.entries(attributes || {})) {
    if (key === "style") Object.assign(node.style, value);
    else if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
    else if (value !== undefined && value !== null) node.setAttribute(key, value);
  }
  for (const child of children.flat()) if (child) node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  return node;
}
function icon(path) { return svg("svg", { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" }, svg("path", { d: path })); }
function fmt(value) {
  if (value === null || value === undefined) return "—";
  if (typeof value === "number") return Number.isInteger(value) ? value.toLocaleString() : value.toFixed(3);
  if (Array.isArray(value)) return value.length ? value.map(fmt).join(", ") : "none";
  if (typeof value === "object") return Object.entries(value).map(([key, item]) => key + ": " + fmt(item)).join("  ");
  return String(value);
}
function compact(value) {
  if (value === null || value === undefined) return "—";
  if (typeof value !== "number") return fmt(value);
  if (Math.abs(value) >= 1e6) return (value / 1e6).toFixed(1) + "M";
  if (Math.abs(value) >= 1e4) return (value / 1e3).toFixed(1) + "k";
  return Number.isInteger(value) ? value.toLocaleString() : value.toFixed(2);
}
function percent(value) { return value === null || value === undefined ? "—" : (value * 100).toFixed(1) + "%"; }
function chip(value, title) { return value === null || value === undefined || value === "" ? h("span", { class: "muted" }, "—") : h("span", { class: "chip " + (TONES[value] || ""), title }, String(value)); }
function member(value) { return value ? h("span", { class: "member", title: value.key }, value.actor + " ", h("code", { class: "muted" }, value.short)) : h("span", { class: "muted" }, "—"); }
function when(millis) { return millis ? new Date(millis).toLocaleString(undefined, { year: "numeric", month: "short", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit" }) : "—"; }
function ago(millis) {
  if (!millis) return "—";
  const seconds = Math.max(0, (Date.now() - millis) / 1000);
  if (seconds < 60) return Math.round(seconds) + "s ago";
  if (seconds < 3600) return Math.round(seconds / 60) + "m ago";
  if (seconds < 86400) return Math.round(seconds / 3600) + "h ago";
  return Math.round(seconds / 86400) + "d ago";
}
function at(spec) { return when(spec.occurred_at); }
function sessionOf(spec) { return spec.session && spec.session.session_id ? spec.session.session_id : null; }
function colorOf(key, index) { return NAMED[key] || PALETTE[index % PALETTE.length]; }
function link(text, hash) { return h("a", { href: hash }, text); }
function go(hash) { if (location.hash === hash) render(); else location.hash = hash; }
const tooltip = {
  show(event, content) {
    const box = document.getElementById("tooltip");
    put(box, content);
    box.hidden = false;
    const x = Math.min(event.clientX + 14, window.innerWidth - box.offsetWidth - 8);
    const y = Math.min(event.clientY + 14, window.innerHeight - box.offsetHeight - 8);
    box.style.left = x + "px"; box.style.top = y + "px";
  },
  hide() { document.getElementById("tooltip").hidden = true; },
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
// The global filter as query parameters; a widget's own choices are layered on top
function filterParams(extra) {
  const params = {};
  if (filter.since || filter.until) { if (filter.since) params.since = Math.floor(filter.since); if (filter.until) params.until = Math.floor(filter.until); }
  else { const range = RANGES.find(([name]) => name === filter.range); if (range && range[1]) params.since = Math.floor(Date.now() - range[1]); }
  for (const [name, values] of Object.entries(filter.facets)) if (values && values.length) params[name] = values.join(",");
  if (filter.q) params.q = filter.q;
  if (filter.session) params.session = filter.session;
  return Object.assign(params, extra || {});
}
function activeFilters() { return Object.values(filter.facets).reduce((count, values) => count + (values ? values.length : 0), 0) + (filter.q ? 1 : 0) + (filter.session ? 1 : 0) + (filter.since || filter.until ? 1 : 0); }
function setFacet(name, values) { if (values && values.length) filter.facets[name] = values; else delete filter.facets[name]; saveFilter(); }
function addFacet(name, value) { const values = new Set(filter.facets[name] || []); values.add(value); setFacet(name, [...values]); }
function toggleFacet(name, value) { const values = new Set(filter.facets[name] || []); if (values.has(value)) values.delete(value); else values.add(value); setFacet(name, [...values]); }
// Open the events of a choice (a KPI, a bar, a slice, a value in an event)
function drill(changes) {
  for (const [name, value] of Object.entries(changes.facets || {})) setFacet(name, [].concat(value));
  if (changes.since || changes.until) { filter.since = changes.since || null; filter.until = changes.until || null; }
  if (changes.session !== undefined) filter.session = changes.session;
  saveFilter();
  go("#/events");
}
async function facets() {
  if (!facetCache) facetCache = api("/api/facets", filterParams()).catch(() => ({ total: 0, facets: {} }));
  return facetCache;
}

// ---------------------------------------------------------------- charts

// The top of the axis: four whole-number steps of 1, 2, or 5 times a power of ten
function niceMax(max) {
  if (max <= 4) return 4;
  const raw = max / 4, power = Math.pow(10, Math.floor(Math.log10(raw)));
  for (const step of [1, 2, 5, 10]) if (step * power >= raw) return Math.ceil(step * power) * 4;
  return 40 * power;
}
function axisTime(millis, bucket, span) {
  const date = new Date(millis);
  const day = date.toLocaleDateString(undefined, { month: "short", day: "numeric" });
  if (bucket >= 86400e3) return day;
  const time = date.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
  return span > 86400e3 ? day + " " + time : time;
}
function legend(series, hidden, redraw, onPick) {
  return h("div", { class: "legend" }, series.map((item, index) => h("button", {
    class: hidden.has(item.key) ? "off" : "", title: "Click to show or hide; double-click to see these events",
    onclick: () => { if (hidden.has(item.key)) hidden.delete(item.key); else hidden.add(item.key); redraw(); },
    ondblclick: () => onPick && onPick({ key: item.key }),
  }, h("i", { style: "background:" + colorOf(item.key, index) }), item.label || item.key, h("span", { class: "count" }, compact(item.total)))));
}
// Events over time: stacked bars, lines, or areas, with a tooltip per bucket and a click that
// opens the events of that bucket (and series)
function timeseries(data, options) {
  const box = h("div", { class: "chart" });
  const hidden = new Set();
  const height = options.height || 200;
  let drawnWidth = 0;
  const draw = () => {
    const width = box.clientWidth;
    if (!width) return;
    drawnWidth = width;
    put(box, );
    const series = data.series || [];
    if (!series.length || !data.total) { box.append(h("div", { class: "empty" }, "No events in this range and filter.")); return; }
    const visible = series.filter((item) => !hidden.has(item.key));
    const count = data.buckets, left = 40, right = 8, top = 8, bottom = 22;
    const plotWidth = width - left - right, plotHeight = height - top - bottom, step = plotWidth / count;
    const stacked = options.type === "bars";
    let max = 0;
    for (let index = 0; index < count; index += 1) {
      if (stacked) max = Math.max(max, visible.reduce((sum, item) => sum + item.points[index], 0));
      else for (const item of visible) max = Math.max(max, item.points[index]);
    }
    max = niceMax(max);
    const y = (value) => top + plotHeight - (value / max) * plotHeight;
    const chart = svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "events over time" });
    const axis = svg("g", { class: "axis" });
    for (let tick = 0; tick <= 4; tick += 1) {
      const value = (max / 4) * tick, ty = y(value);
      axis.append(svg("line", { class: "grid-line", x1: left, x2: width - right, y1: ty, y2: ty, style: { strokeDasharray: tick ? "2 3" : "" } }), svg("text", { x: left - 6, y: ty + 3, "text-anchor": "end" }, compact(value)));
    }
    const labels = Math.max(2, Math.min(8, Math.floor(plotWidth / 110)));
    for (let label = 0; label < labels; label += 1) {
      const index = Math.round((label * (count - 1)) / Math.max(1, labels - 1));
      axis.append(svg("text", { x: left + index * step + step / 2, y: height - 6, "text-anchor": "middle" }, axisTime(data.start + index * data.bucket_ms, data.bucket_ms, data.buckets * data.bucket_ms)));
    }
    chart.append(axis);
    const describe = (index) => {
      const start = data.start + index * data.bucket_ms;
      return h("div", {}, h("div", {}, h("b", {}, when(start))), visible.filter((item) => item.points[index]).map((item) => h("div", { class: "row" }, h("i", { style: "background:" + colorOf(item.key, series.indexOf(item)) }), item.label || item.key, h("b", { style: "margin-left:auto" }, item.points[index]))), h("div", { class: "muted", style: "margin-top:4px;color:#bbb" }, "Click to see these events"));
    };
    const pick = (index, key) => options.onPick && options.onPick({ since: data.start + index * data.bucket_ms, until: data.start + (index + 1) * data.bucket_ms - 1, key });
    if (stacked) {
      for (let index = 0; index < count; index += 1) {
        let base = 0;
        for (const item of visible) {
          const value = item.points[index];
          if (!value) continue;
          const y1 = y(base + value), y0 = y(base);
          chart.append(svg("rect", {
            class: "bar", x: left + index * step + Math.min(1, step * 0.1), width: Math.max(1, step - Math.max(1, step * 0.2)), y: y1, height: Math.max(1, y0 - y1), style: { fill: colorOf(item.key, series.indexOf(item)) },
            onmousemove: (event) => tooltip.show(event, describe(index)), onmouseleave: tooltip.hide, onclick: () => { tooltip.hide(); pick(index, item.key); },
          }));
          base += value;
        }
      }
    } else {
      for (const item of visible) {
        const color = colorOf(item.key, series.indexOf(item));
        const points = item.points.map((value, index) => [left + index * step + step / 2, y(value)]);
        const line = points.map(([px, py], index) => (index ? "L" : "M") + px.toFixed(1) + " " + py.toFixed(1)).join(" ");
        if (options.type === "area") chart.append(svg("path", { d: line + " L" + points[points.length - 1][0] + " " + y(0) + " L" + points[0][0] + " " + y(0) + " Z", style: { fill: color, opacity: "0.18" } }));
        chart.append(svg("path", { d: line, style: { fill: "none", stroke: color, strokeWidth: "2" } }));
      }
      for (let index = 0; index < count; index += 1) {
        chart.append(svg("rect", { x: left + index * step, y: top, width: step, height: plotHeight, style: { fill: "transparent", cursor: "pointer" }, onmousemove: (event) => tooltip.show(event, describe(index)), onmouseleave: tooltip.hide, onclick: () => { tooltip.hide(); pick(index); } }));
      }
    }
    box.append(chart, legend(series, hidden, draw, options.onPick));
  };
  new ResizeObserver(() => { if (box.clientWidth !== drawnWidth) draw(); }).observe(box);
  return box;
}
function donut(items, options) {
  const total = items.reduce((sum, item) => sum + item.count, 0);
  if (!total) return h("div", { class: "empty" }, "No events in this range and filter.");
  const chart = svg("svg", { viewBox: "0 0 160 160", role: "img", "aria-label": options.title || "breakdown" });
  let angle = -Math.PI / 2;
  items.forEach((item, index) => {
    const sweep = (item.count / total) * Math.PI * 2;
    const end = angle + sweep, large = sweep > Math.PI ? 1 : 0;
    const point = (radius, theta) => (80 + radius * Math.cos(theta)).toFixed(2) + " " + (80 + radius * Math.sin(theta)).toFixed(2);
    const d = items.length === 1
      ? "M80 8 A72 72 0 1 1 79.99 8 L79.99 38 A42 42 0 1 0 80 38 Z"
      : "M" + point(72, angle) + " A72 72 0 " + large + " 1 " + point(72, end) + " L" + point(42, end) + " A42 42 0 " + large + " 0 " + point(42, angle) + " Z";
    chart.append(svg("path", { class: "slice", d, style: { fill: colorOf(item.value, index) },
      onmousemove: (event) => tooltip.show(event, h("div", {}, h("b", {}, item.label || item.value), " " + item.count + " (" + ((item.count / total) * 100).toFixed(1) + "%)")),
      onmouseleave: tooltip.hide, onclick: () => { tooltip.hide(); if (options.onPick) options.onPick(item.value); } }));
    angle = end;
  });
  chart.append(svg("text", { x: 80, y: 78, "text-anchor": "middle", style: { fill: "var(--ink)", fontSize: "20px", fontWeight: "700" } }, compact(total)), svg("text", { x: 80, y: 96, "text-anchor": "middle", style: { fill: "var(--muted)", fontSize: "10px" } }, "events"));
  return h("div", { class: "donut" }, chart, h("div", { class: "legend", style: "flex-direction:column;flex:1;min-width:160px" }, items.map((item, index) => h("button", { onclick: () => options.onPick && options.onPick(item.value) },
    h("i", { style: "background:" + colorOf(item.value, index) }), item.label || item.value, h("span", { class: "count" }, item.count + " · " + ((item.count / total) * 100).toFixed(0) + "%")))));
}
function toplist(items, options) {
  if (!items.length) return h("div", { class: "empty" }, "No events in this range and filter.");
  const max = Math.max(...items.map((item) => item.count));
  return h("div", { class: "toplist" }, items.map((item, index) => h("div", { class: "row", title: "See these events", onclick: () => options.onPick && options.onPick(item.value) },
    h("span", { class: "name", title: item.label || item.value }, item.label || item.value),
    h("div", { class: "track" }, h("div", { class: "fill", style: "width:" + Math.max(2, (item.count / max) * 100) + "%;background:" + colorOf(item.value, index) })),
    h("span", { class: "num" }, compact(item.count)))));
}
function sparkline(points, color) {
  if (!points || points.length < 2) return null;
  const max = Math.max(1, ...points);
  const d = points.map((value, index) => (index ? "L" : "M") + ((index / (points.length - 1)) * 72).toFixed(1) + " " + (24 - (value / max) * 22).toFixed(1)).join(" ");
  return svg("svg", { class: "spark", viewBox: "0 0 72 26", "aria-hidden": "true" }, svg("path", { d, style: { fill: "none", stroke: color, strokeWidth: "1.6" } }));
}

// ---------------------------------------------------------------- widgets

function widget(title, options, ...body) {
  const content = h("div", { class: "body" + (options.flush ? " flush" : "") }, body);
  const node = h("section", { class: "widget span-" + (options.span || 12) }, h("header", {}, h("h3", {}, title), options.controls || []), content);
  node.body = content;
  return node;
}
function select(label, value, options, onchange) {
  return h("select", { "aria-label": label, title: label, onchange: (event) => onchange(event.target.value) },
    options.map((option) => {
      if (option.group) return h("optgroup", { label: option.group }, option.options.map(([optionValue, text]) => { const node = h("option", { value: optionValue }, text); if (optionValue === value) node.selected = true; return node; }));
      const [optionValue, text] = option;
      const node = h("option", { value: optionValue }, text);
      if (optionValue === value) node.selected = true;
      return node;
    }));
}
// The choices of a graph's own filter: every value of every facet, from the current events
function widgetFilterOptions(cache) {
  const groups = FACETS.map((name) => ({ group: FACET_LABELS[name], options: ((cache.facets || {})[name] || []).slice(0, 12).map((item) => [name + "=" + item.value, (item.label || item.value) + " (" + item.count + ")"]) })).filter((group) => group.options.length);
  return [["", "All events"]].concat(groups);
}
function parseWidgetFilter(value) { if (!value) return {}; const index = value.indexOf("="); return { [value.slice(0, index)]: value.slice(index + 1) }; }
function onPickFor(by, widgetFilter) {
  return ({ since, until, key }) => {
    const facetsChosen = Object.assign({}, parseWidgetFilter(widgetFilter));
    if (by && key && key !== "OTHER") facetsChosen[by] = key;
    drill({ facets: facetsChosen, since, until });
  };
}
// A graph of events over time with its own group-by, filter, and chart type
function seriesWidget(title, options) {
  const state = { by: options.by || "", filter: options.filter || "", type: options.type || "bars" };
  const node = widget(title, { span: options.span });
  const load = async () => {
    put(node.body, h("div", { class: "skeleton" }, "Loading…"));
    const extra = Object.assign({ by: state.by }, options.params || {}, parseWidgetFilter(state.filter));
    try {
      const data = await api("/api/series", filterParams(extra));
      put(node.body, timeseries(data, { type: state.type, height: options.height, title, onPick: (pick) => onPickFor(state.by, state.filter)(pick) }));
    } catch (error) { put(node.body, h("div", { class: "notice error" }, error.message)); }
  };
  facets().then((cache) => {
    const header = node.querySelector("header");
    header.append(
      select("Group by", state.by, [["", "No grouping"]].concat(FACETS.map((name) => [name, "By " + FACET_LABELS[name].toLowerCase()])), (value) => { state.by = value; load(); }),
      select("Filter this graph", state.filter, widgetFilterOptions(cache), (value) => { state.filter = value; load(); }),
      select("Chart type", state.type, [["bars", "Bars"], ["lines", "Lines"], ["area", "Area"]], (value) => { state.type = value; load(); }));
  });
  load();
  return node;
}
// A breakdown of the events by one facet, as a donut or a top list
function breakdownWidget(title, options) {
  const state = { by: options.by || "severity", filter: options.filter || "", view: options.view || "donut" };
  const node = widget(title, { span: options.span });
  const load = async () => {
    put(node.body, h("div", { class: "skeleton" }, "Loading…"));
    try {
      const data = await api("/api/facets", filterParams(Object.assign({}, options.params || {}, parseWidgetFilter(state.filter))));
      const items = (data.facets[state.by] || []).slice(0, options.limit || 10);
      const pick = (value) => drill({ facets: Object.assign(parseWidgetFilter(state.filter), { [state.by]: value }) });
      put(node.body, state.view === "donut" ? donut(items, { title, onPick: pick }) : toplist(items, { onPick: pick }));
    } catch (error) { put(node.body, h("div", { class: "notice error" }, error.message)); }
  };
  facets().then((cache) => {
    node.querySelector("header").append(
      select("Break down by", state.by, FACETS.map((name) => [name, "By " + FACET_LABELS[name].toLowerCase()]), (value) => { state.by = value; load(); }),
      select("Filter this graph", state.filter, widgetFilterOptions(cache), (value) => { state.filter = value; load(); }),
      select("Show as", state.view, [["donut", "Donut"], ["toplist", "Top list"]], (value) => { state.view = value; load(); }));
  });
  load();
  return node;
}
// A table with search, a dropdown per filterable column, sorting by any column, and pages
function dataTable(columns, rows, options) {
  options = options || {};
  const state = { q: options.query || "", sort: options.sort === undefined ? -1 : options.sort, dir: options.dir || -1, page: 0, chosen: {} };
  const size = options.pageSize || 25;
  const holder = h("div");
  const text = (column, row) => { const value = column.get ? column.get(row) : null; return value === null || value === undefined ? "" : typeof value === "object" ? JSON.stringify(value) : String(value); };
  const tools = h("div", { class: "table-tools" });
  const searchInput = h("input", { type: "search", placeholder: options.placeholder || "Search this table…", value: state.q, "aria-label": "Search this table", oninput: (event) => { state.q = event.target.value; state.page = 0; draw(); } });
  tools.append(searchInput);
  for (const column of columns.filter((item) => item.filter)) {
    const values = [...new Set(rows.map((row) => text(column, row)).filter(Boolean))].sort();
    tools.append(select(column.label, "", [["", column.label + ": all"]].concat(values.map((value) => [value, value])), (value) => { state.chosen[column.label] = value; state.page = 0; draw(); }));
  }
  const counter = h("span", { class: "count" });
  tools.append(counter);
  const draw = () => {
    const q = state.q.trim().toLowerCase();
    let list = rows.filter((row) => (!q || columns.some((column) => text(column, row).toLowerCase().includes(q))) && columns.every((column) => !state.chosen[column.label] || text(column, row) === state.chosen[column.label]));
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
    const head = h("tr", {}, columns.map((column, index) => h("th", { class: column.get ? "sortable" : "", onclick: () => { if (!column.get) return; if (state.sort === index) state.dir = -state.dir; else { state.sort = index; state.dir = column.num ? -1 : 1; } draw(); } }, column.label, state.sort === index ? h("span", { class: "arrow" }, state.dir > 0 ? "▲" : "▼") : null)));
    const body = shown.length ? shown.map((row) => h("tr", { class: options.onRow ? "clickable" : "", onclick: options.onRow ? (event) => { if (!event.target.closest("a,button")) options.onRow(row); } : undefined },
      columns.map((column) => { const cell = column.render ? column.render(row) : text(column, row); return h("td", { class: column.num ? "num" : "" }, cell instanceof Node ? cell : fmt(cell)); })))
      : [h("tr", {}, h("td", { colspan: columns.length, class: "empty" }, rows.length ? "No rows match." : (options.empty || "Nothing recorded yet.")))];
    const pager = pages > 1 ? h("div", { class: "pager" }, "Page " + (state.page + 1) + " of " + pages,
      h("button", { class: "btn", disabled: state.page === 0 ? true : null, onclick: () => { state.page -= 1; draw(); } }, "‹ Previous"),
      h("button", { class: "btn", disabled: state.page >= pages - 1 ? true : null, onclick: () => { state.page += 1; draw(); } }, "Next ›")) : null;
    put(holder, h("div", { style: "overflow-x:auto" }, h("table", {}, h("thead", {}, head), h("tbody", {}, body))), pager);
  };
  draw();
  return h("div", {}, options.noTools ? null : tools, holder);
}
function kpi(label, value, options) {
  options = options || {};
  const node = h("div", { class: "kpi " + (options.tone || ""), dataset: options.drill ? { href: "1" } : {}, title: options.drill ? "See these events" : undefined, onclick: options.drill || undefined },
    h("div", { class: "label" }, label), h("div", { class: "value" }, options.format ? options.format(value) : compact(value)), options.note ? h("div", { class: "note" }, options.note) : null, sparkline(options.spark, options.color || "#7b5fc7"));
  return node;
}

// ---------------------------------------------------------------- the event drawer

function closeDrawer() { document.querySelectorAll(".drawer, .scrim").forEach((node) => node.remove()); }
function openDrawer(spec) {
  closeDrawer();
  const value = (group, key, item) => {
    const facetName = group ? FACET_FIELDS[group + "." + key] : { flow: "flow", origin: "origin", event_name: "event" }[key];
    const shown = fmt(item);
    if (facetName && typeof item === "string" && item) return h("a", { title: "Filter every view by this value", onclick: () => { closeDrawer(); addFacet(facetName, item); go("#/events"); } }, shown);
    if (group === "session" && key === "session_id" && item) return h("a", { href: "#/sessions/" + encodeURIComponent(item), onclick: closeDrawer }, shown);
    return shown;
  };
  const groups = GROUPS.filter((group) => spec[group] && Object.keys(spec[group]).length).map((group) => h("div", { class: "group" }, h("h4", {}, group), h("div", { class: "kv" }, Object.entries(spec[group]).map(([key, item]) => [h("div", {}, key), h("div", {}, value(group, key, item))]))));
  const envelope = h("div", { class: "group" }, h("h4", {}, "envelope"), h("div", { class: "kv" }, ["event_id", "flow", "origin", "occurred_at", "schema_version", "sequence", "digest", "previous_digest", "member"].filter((key) => spec[key] !== undefined && spec[key] !== null).map((key) => [h("div", {}, key), h("div", {}, key === "occurred_at" ? when(spec[key]) : key === "member" ? member(spec[key]) : value(null, key, spec[key]))])));
  const copy = h("button", { class: "btn", onclick: async () => { try { await navigator.clipboard.writeText(JSON.stringify(spec, null, 2)); copy.textContent = "Copied"; } catch (error) { copy.textContent = "Copy failed"; } } }, "Copy JSON");
  const drawer = h("aside", { class: "drawer", role: "dialog", "aria-label": "Event " + spec.event_name },
    h("header", {}, h("span", { class: "sev", style: "background:" + colorOf((spec.severity || {}).severity_status, 0) }), h("h3", {}, spec.event_name), copy, h("button", { class: "icon-btn", "aria-label": "Close", onclick: closeDrawer }, "✕")),
    h("div", { class: "content" }, h("p", {}, (spec.response || {}).response_statement || ""), h("p", { class: "muted" }, "Click a value to filter every view by it."), envelope, groups));
  document.body.append(h("div", { class: "scrim", onclick: closeDrawer }), drawer);
}

// ---------------------------------------------------------------- shared tables

function eventsTable(events, options) {
  return dataTable([
    { label: "", render: (row) => h("span", { class: "sev", style: "background:" + colorOf((row.severity || {}).severity_status, 0), title: (row.severity || {}).severity_status }) },
    { label: "Time", get: (row) => row.occurred_at, render: (row) => h("span", { title: at(row) }, ago(row.occurred_at)) },
    { label: "Event", get: (row) => row.event_name, render: (row) => h("div", {}, h("code", {}, row.event_name), h("div", { class: "muted" }, (row.response || {}).response_statement || "")) },
    { label: "Flow", get: (row) => row.flow, filter: true },
    { label: "Severity", get: (row) => (row.severity || {}).severity_status, render: (row) => chip((row.severity || {}).severity_status), filter: true },
    { label: "Decision", get: (row) => (row.decision || {}).decision, render: (row) => chip((row.decision || {}).decision), filter: true },
    { label: "Member", get: (row) => row.member ? row.member.actor : "", render: (row) => member(row.member) },
    { label: "Session", get: (row) => sessionOf(row), render: (row) => sessionOf(row) ? link(sessionOf(row).slice(0, 16), "#/sessions/" + encodeURIComponent(sessionOf(row))) : "—" },
    { label: "Origin", get: (row) => row.origin, render: (row) => chip(row.origin), filter: options && options.originFilter },
  ], events, Object.assign({ sort: 1, dir: -1, onRow: openDrawer, placeholder: "Search these events…" }, options || {}));
}
function alertTable(rows) {
  return dataTable([
    { label: "When", get: (row) => row.at, render: (row) => when(row.at * 1000) },
    { label: "Alert", get: (row) => row.title },
    { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true },
    { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true },
    { label: "Member", get: (row) => row.member ? row.member.actor : "", render: (row) => member(row.member), filter: true },
    { label: "Session", get: (row) => row.session, render: (row) => row.session ? link(String(row.session).slice(0, 16), "#/sessions/" + encodeURIComponent(row.session)) : "—" },
    { label: "Delivery", get: (row) => row.computed ? "derived" : row.suppressed ? "suppressed" : (row.deliveries || []).map((delivery) => delivery.channel + ": " + delivery.result).join(", "), render: (row) => row.computed ? h("span", { class: "muted", title: row.detail }, "derived by the dashboard") : row.suppressed ? "suppressed (cooldown)" : fmt((row.deliveries || []).map((delivery) => delivery.channel + ": " + delivery.result)) },
  ], rows, { sort: 0, dir: -1, placeholder: "Search alerts…" });
}

// ---------------------------------------------------------------- pages

function needRepository(main) {
  if (scope.repository) return false;
  main.append(h("div", { class: "notice" }, "Choose a repository in the top bar, or open ", link("Repositories", "#/repositories"), " for a summary of the account."));
  return true;
}
function repositoryName() { const repository = me && me.repositories.find((item) => item.id === scope.repository); return repository ? repository.remote.replace(/^github\.com\//, "") : "repository"; }

async function overview(main) {
  if (needRepository(main)) return;
  const [data, decisions, all] = await Promise.all([api("/api/overview", filterParams()), api("/api/series", filterParams({ by: "decision" })), api("/api/series", filterParams())]);
  const evals = data.evals || {};
  lastEvals = evals;
  const points = (key) => { const item = (decisions.series || []).find((series) => series.key === key); return item ? item.points : null; };
  const scopeLabel = scope.member ? members.find((row) => row.member.key === scope.member) : null;
  main.append(h("div", { class: "hero" },
    h("div", {}, h("h1", {}, repositoryName()), h("div", { class: "sub" }, scopeLabel ? "Member " + scopeLabel.member.actor : "All " + (data.members || 0) + " member(s)", " · ", rangeLabel(), activeFilters() ? " · " + activeFilters() + " filter(s)" : "")),
    h("div", { class: "facts" },
      h("div", { class: "fact" }, "Events", h("b", {}, compact(evals.events))), h("div", { class: "fact" }, "Active sessions", h("b", {}, compact(data.active_sessions))),
      h("div", { class: "fact" }, "Agents", h("b", {}, compact(data.agents))), h("div", { class: "fact" }, "Event chains", h("b", {}, data.event_chain.status)),
      h("div", { class: "fact" }, "Governance", h("b", {}, data.integrity.reported ? (data.integrity.ok ? "passing" : "failing") : "not reported")))));
  const drillTo = (facetsChosen) => () => drill({ facets: facetsChosen });
  main.append(h("div", { class: "kpis" },
    kpi("Tool calls decided", evals.tool_calls_decided, { spark: all.series && all.series[0] ? all.series[0].points : null, note: compact(evals.events) + " events in all", drill: drillTo({ flow: "F05" }) }),
    kpi("Allowed", evals.allowed, { spark: points("ALLOW"), color: NAMED.ALLOW, tone: "good", drill: drillTo({ decision: "ALLOW" }) }),
    kpi("Denied", evals.denied, { spark: points("DENY"), color: NAMED.DENY, tone: evals.denied ? "bad" : "", note: "share " + percent(evals.denial_share), drill: drillTo({ decision: "DENY" }) }),
    kpi("Approvals requested", evals.approvals, { spark: points("REQUIRE_APPROVAL"), color: NAMED.REQUIRE_APPROVAL, drill: drillTo({ decision: "REQUIRE_APPROVAL" }) }),
    kpi("Bypass attempts", evals.bypass_attempts, { tone: evals.bypass_attempts ? "bad" : "", drill: drillTo({ alert: "BYPASS_ATTEMPT" }) }),
    kpi("Confirmed bypasses", evals.bypasses_confirmed, { tone: evals.bypasses_confirmed ? "crit" : "good", note: "protected code changed outside checks", drill: drillTo({ event: "BYPASS_CONFIRMED" }) }),
    kpi("Violations after effect", evals.violations_after_effect, { tone: evals.violations_after_effect ? "bad" : "", drill: drillTo({ event: "VIOLATION_AFTER_EFFECT" }) }),
    kpi("Coverage gaps", evals.coverage_gaps, { tone: evals.coverage_gaps ? "bad" : "", note: "unobserved or undecided actions", drill: drillTo({ enforcement: "COVERAGE_GAP" }) }),
    kpi("Evidence completeness", evals.evidence_completeness, { format: percent, note: "allowed calls verified after running" }),
    kpi("Credits available", (evals.credits || {}).available, { note: compact((evals.credits || {}).consumed) + " consumed", drill: drillTo({ flow: "F27" }) }),
    kpi("Suspicious prompts", evals.suspicious_prompts, { tone: evals.suspicious_prompts ? "bad" : "", drill: drillTo({ event: "PROMPT_SUSPICIOUS" }) }),
    kpi("Signed attestations", evals.attestations_signed, { drill: drillTo({ event: "SESSION_ATTESTED" }) })));
  main.append(h("div", { class: "grid" },
    seriesWidget("Events over time", { span: 8, by: "severity", height: 230 }),
    breakdownWidget("Breakdown", { span: 4, by: "severity" })));
  main.append(h("div", { class: "grid" },
    seriesWidget("Tool call decisions", { span: 6, by: "decision", filter: "flow=F05" }),
    breakdownWidget("Top events", { span: 6, by: "event", view: "toplist", limit: 10 })));
  main.append(h("div", { class: "grid" },
    seriesWidget("Activity by agent", { span: 6, by: "agent", type: "lines" }),
    breakdownWidget("Most active members", { span: 6, by: "member", view: "toplist", limit: 8 })));
  main.append(h("div", { class: "grid" },
    widget("Recent violations", { span: 6, flush: true }, eventsTable(data.recent_violations || [], { pageSize: 8, noTools: true, empty: "No violations in this range." })),
    widget("Recent alerts", { span: 6, flush: true }, dataTable([
      { label: "When", get: (row) => row.at, render: (row) => ago(row.at * 1000) }, { label: "Alert", get: (row) => row.title }, { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity) }, { label: "Member", render: (row) => member(row.member) },
    ], data.recent_alerts || [], { noTools: true, sort: 0, pageSize: 8, empty: "No alerts." }))));
  main.append(h("div", { class: "grid" },
    widget("Integrity", { span: 5 }, h("p", {}, "Governance state ", data.integrity.reported ? chip(data.integrity.ok ? "PASS" : "FAIL") : h("span", { class: "muted" }, "not reported yet"), "   Event chains ", chip(data.event_chain.status), " ", h("span", { class: "muted" }, compact(data.event_chain.length) + " events")),
      data.integrity.findings.length ? dataTable([{ label: "Member", render: (row) => member(row.member) }, { label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "Code", get: (row) => row.code, render: (row) => h("code", {}, row.code) }, { label: "Finding", get: (row) => row.message }], data.integrity.findings, { pageSize: 6 }) : h("p", { class: "muted" }, "No integrity findings.")),
    h("section", { class: "widget span-7", id: "evals" }, h("header", {}, h("h3", {}, "All evals"), h("span", { class: "muted" }, "computed from the events in range and filter")), h("div", { class: "body flush" }, evalsTable(evals, evalQuery)))));
  if (evalQuery) { document.getElementById("evals").scrollIntoView({ behavior: "smooth" }); evalQuery = ""; }
}
function evalsTable(evals, query) {
  const rows = Object.entries(evals || {}).filter(([, value]) => value !== null && !(typeof value === "object" && !Array.isArray(value) && !Object.keys(value).length));
  return dataTable([{ label: "Eval", get: (row) => row[0], render: (row) => h("code", {}, row[0]) }, { label: "Value", get: (row) => typeof row[1] === "number" ? row[1] : fmt(row[1]), render: (row) => fmt(row[1]) }], rows, { query, sort: 0, dir: 1, pageSize: 12, placeholder: "Search evals…" });
}
function rangeLabel() {
  if (filter.since || filter.until) return when(filter.since) + " → " + (filter.until ? when(filter.until) : "now");
  return filter.range === "All" ? "all time" : "last " + filter.range;
}

async function events(main) {
  if (needRepository(main)) return;
  const cache = await facets();
  const panel = h("aside", { class: "facets", "aria-label": "Facets" }, h("div", { class: "head" }, "Facets", h("span", { class: "muted" }, compact(cache.total) + " events")),
    FACETS.map((name, index) => {
      const values = (cache.facets || {})[name] || [];
      const chosen = new Set(filter.facets[name] || []);
      return h("details", { open: index < 5 || chosen.size ? true : null }, h("summary", {}, FACET_LABELS[name], chosen.size ? h("span", { class: "chip info" }, chosen.size) : null),
        values.length ? values.map((item) => h("label", {}, h("input", { type: "checkbox", checked: chosen.has(item.value) ? true : null, onchange: () => { toggleFacet(name, item.value); render(); } }), h("span", { title: item.label || item.value }, item.label || item.value), h("em", {}, compact(item.count))))
          : h("div", { class: "muted", style: "padding:2px 14px 8px;font-size:12px" }, "none"));
    }));
  const list = widget("Event stream", { flush: true });
  const right = h("div", {}, h("div", { class: "grid" }, seriesWidget("Event volume", { span: 12, by: "severity", height: 170 })), list);
  main.append(h("div", { class: "explorer" }, panel, right));
  let cursor = 0;
  const rows = [];
  const loadMore = async () => {
    const data = await api("/api/events", filterParams({ cursor, limit: 500 }));
    rows.push(...data.events);
    cursor = data.next_cursor;
    list.querySelector("h3").textContent = "Event stream · " + compact(data.total) + " events" + (rows.length < data.total ? " (newest " + rows.length + " loaded)" : "");
    put(list.body, eventsTable(rows, { pageSize: 50, originFilter: true }), cursor ? h("div", { class: "pager" }, h("button", { class: "btn", onclick: loadMore }, "Load 500 more")) : null);
  };
  await loadMore();
}

async function sessions(main) {
  if (needRepository(main)) return;
  const data = await api("/api/sessions");
  const rows = data.sessions;
  const active = rows.filter((row) => !row.stale && row.session.status === "ACTIVE").length;
  main.append(h("div", { class: "kpis" },
    kpi("Sessions", rows.length), kpi("Active", active, { tone: "good" }), kpi("Stale", rows.filter((row) => row.stale).length, { tone: rows.some((row) => row.stale) ? "bad" : "", note: "stopped reporting without a signed end" }),
    kpi("Quarantined", rows.filter((row) => row.session.safety === "QUARANTINED").length, { tone: rows.some((row) => row.session.safety === "QUARANTINED") ? "crit" : "" }),
    kpi("Denied actions", rows.reduce((sum, row) => sum + ((row.evals || {}).denied || 0), 0), { drill: () => drill({ facets: { decision: "DENY" } }) })));
  main.append(h("div", { class: "grid" }, seriesWidget("Session activity", { span: 8, by: "agent", type: "area", height: 190 }), breakdownWidget("Events by agent type", { span: 4, by: "agent" })));
  main.append(widget("Sessions", { flush: true }, dataTable([
    { label: "Session", get: (row) => row.session.foxx_session_id, render: (row) => link(row.session.foxx_session_id, "#/sessions/" + encodeURIComponent(row.session.foxx_session_id)) },
    { label: "Member", get: (row) => row.member ? row.member.actor : "", render: (row) => member(row.member), filter: true },
    { label: "Provider", get: (row) => row.session.provider, filter: true },
    { label: "Model", get: (row) => row.session.model, filter: true },
    { label: "Status", get: (row) => row.stale ? "STALE" : row.session.status, render: (row) => chip(row.stale ? "STALE" : row.session.status), filter: true },
    { label: "Safety", get: (row) => row.session.safety, render: (row) => chip(row.session.safety), filter: true },
    { label: "Mode", get: (row) => row.session.autonomy_mode, filter: true },
    { label: "Denied", get: (row) => (row.evals || {}).denied, num: true },
    { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true },
    { label: "Credits", get: (row) => row.credits.available, render: (row) => row.credits.available + " / " + row.credits.granted, num: true },
    { label: "Last seen", get: (row) => row.session.last_seen_at, render: (row) => ago(row.session.last_seen_at) },
  ], rows, { sort: 10, dir: -1, onRow: (row) => go("#/sessions/" + encodeURIComponent(row.session.foxx_session_id)), placeholder: "Search sessions, members, models…" })));
}

function graphView(graph) {
  const nodes = graph.nodes || [];
  if (!nodes.length) return h("div", { class: "empty" }, "No events in this session.");
  const verified = new Set((graph.edges || []).filter((edge) => edge.kind === "verified_by").map((edge) => edge.to));
  const color = (node) => node.flags.includes("bypass_confirmed") ? NAMED.CRITICAL : node.flags.includes("violation") ? NAMED.HIGH : node.flags.includes("denied") ? NAMED.DENY : node.flags.includes("coverage_gap") || node.flags.includes("unresolved") ? NAMED.MEDIUM : node.kind === "verification" ? NAMED.ALLOW : node.kind === "decision" ? "#7b5fc7" : "#9aa0b4";
  return h("div", {}, h("p", { class: "legend muted" }, "Order only, no inferred causality. A dotted link joins a decision to the verification of the same tool call. Click a step for its event."),
    h("div", { class: "flow" }, nodes.map((node) => h("div", { class: "node" + (verified.has(node.id) ? " linked" : ""), onclick: () => openDrawer(node.event) },
      h("span", { class: "muted mono" }, new Date(node.event.occurred_at).toLocaleTimeString()),
      h("span", { class: "dot" + (node.kind === "decision" || node.kind === "verification" ? "" : " small"), style: "background:" + color(node) }),
      h("span", {}, h("code", {}, node.label), " ", chip(node.event.flow), " ", node.event.decision && node.event.decision.decision !== "NOT_REQUIRED" ? chip(node.event.decision.decision) : null, " ", node.flags.map((flag) => chip(flag.toUpperCase()))," ", h("span", { class: "muted" }, (node.event.response || {}).response_statement || ""))))));
}

async function session(main, id) {
  if (needRepository(main)) return;
  const [data, graph] = await Promise.all([api("/api/sessions/" + encodeURIComponent(id)), api("/api/graph/" + encodeURIComponent(id))]);
  const info = data.session, evals = data.evals || {};
  if (data.stale) main.append(h("div", { class: "notice warn" }, "This session stopped reporting: no hook activity for a long time and no signed end of session. The agent may have been stopped, its hooks removed, or its evidence is not reaching the service."));
  const download = (path, name) => async () => {
    const response = await fetch(scoped(path), { credentials: "same-origin" });
    const anchor = h("a", { href: URL.createObjectURL(await response.blob()), download: name });
    document.body.append(anchor); anchor.click(); anchor.remove();
  };
  main.append(h("div", { class: "hero" },
    h("div", {}, h("h1", {}, "Session ", h("code", {}, info.foxx_session_id)), h("div", { class: "sub" }, info.provider + " · " + (info.model || "model not reported") + " · ", data.member ? data.member.actor : "unknown member", " · started " + when(info.started_at))),
    h("div", { class: "facts" }, h("div", { class: "fact" }, "Safety", h("b", {}, info.safety)), h("div", { class: "fact" }, "Mode", h("b", {}, info.autonomy_mode)), h("div", { class: "fact" }, "Status", h("b", {}, data.stale ? "STALE" : info.status)),
      h("button", { class: "btn", onclick: download("/api/reports/session/" + info.foxx_session_id, "crane-report-" + info.foxx_session_id + ".json") }, "Report"),
      h("button", { class: "btn", onclick: download("/api/evidence/session/" + info.foxx_session_id, "crane-evidence-" + info.foxx_session_id + ".json") }, "Evidence bundle"),
      h("button", { class: "btn", onclick: () => drill({ session: info.foxx_session_id }) }, "Open in events"))));
  main.append(h("div", { class: "kpis" },
    kpi("Events", evals.events), kpi("Tool calls decided", evals.tool_calls_decided), kpi("Denied", evals.denied, { tone: evals.denied ? "bad" : "" }), kpi("Bypass attempts", evals.bypass_attempts, { tone: evals.bypass_attempts ? "bad" : "" }),
    kpi("Confirmed bypasses", evals.bypasses_confirmed, { tone: evals.bypasses_confirmed ? "crit" : "good" }), kpi("Evidence completeness", evals.evidence_completeness, { format: percent }),
    kpi("Credits available", data.credits.available, { note: data.credits.consumed + " consumed, " + data.credits.revoked + " revoked, " + data.credits.expired + " expired" })));
  main.append(h("div", { class: "grid" }, seriesWidget("Session timeline", { span: 8, by: "flow", params: { session: info.foxx_session_id, since: "" }, height: 190 }), breakdownWidget("Decisions in this session", { span: 4, by: "decision", params: { session: info.foxx_session_id, since: "" } })));
  main.append(h("div", { class: "grid" }, widget("Action graph", { span: 7 }, graphView(graph)),
    h("section", { class: "widget span-5" }, h("header", {}, h("h3", {}, "Evals")), h("div", { class: "body flush" }, evalsTable(evals)))));
  main.append(widget("Tasks", { flush: true }, dataTable([
    { label: "Task", get: (row) => row.task.foxx_task_id, render: (row) => h("code", {}, row.task.foxx_task_id) }, { label: "External", get: (row) => row.task.external_task_id },
    { label: "Status", get: (row) => row.task.status, render: (row) => chip(row.task.status), filter: true }, { label: "Tool calls", get: (row) => row.task.tool_calls, num: true },
    { label: "Denied", get: (row) => (row.evals || {}).denied, num: true }, { label: "Started", get: (row) => row.task.started_at, render: (row) => when(row.task.started_at) },
  ], data.tasks || [], { sort: 5, dir: 1 })));
}

async function agents(main) {
  if (needRepository(main)) return;
  const data = await api("/api/agents");
  const rows = data.agents;
  main.append(h("div", { class: "grid" },
    widget("Denials by agent", { span: 6 }, toplist(rows.map((row) => ({ value: row.agent_id, count: (row.evals || {}).denied || 0 })).filter((item) => item.count).sort((a, b) => b.count - a.count), { onPick: () => drill({ facets: { decision: "DENY" } }) })),
    seriesWidget("Events by agent type", { span: 6, by: "agent", type: "lines" })));
  main.append(widget("Agents (across all runs)", { flush: true }, dataTable([
    { label: "Agent", get: (row) => row.agent_id, render: (row) => h("code", {}, row.agent_id) }, { label: "Provider", get: (row) => row.provider, filter: true },
    { label: "Sessions", get: (row) => row.sessions, num: true }, { label: "Quarantined", get: (row) => row.quarantined_sessions, num: true },
    { label: "Tool calls", get: (row) => (row.evals || {}).tool_calls_decided, num: true }, { label: "Denial share", get: (row) => (row.evals || {}).denial_share, render: (row) => percent((row.evals || {}).denial_share), num: true },
    { label: "Bypass attempts", get: (row) => (row.evals || {}).bypass_attempts, num: true }, { label: "Confirmed bypasses", get: (row) => (row.evals || {}).bypasses_confirmed, num: true },
    { label: "Credits consumed", get: (row) => ((row.evals || {}).credits || {}).consumed, num: true }, { label: "Last seen", get: (row) => row.last_seen, render: (row) => fmt(row.last_seen) },
  ], rows, { sort: 2, placeholder: "Search agents…" })));
}

async function audit(main) {
  if (needRepository(main)) return;
  main.append(h("div", { class: "grid" }, seriesWidget("Crane commands over time", { span: 8, by: "event", filter: "flow=F13", height: 190 }), breakdownWidget("Commands by result", { span: 4, by: "response", filter: "flow=F13" })));
  let governance = null;
  try { governance = await api("/api/governance"); } catch (error) { if (error.status !== 404) throw error; }
  const trail = governance ? governance.audit || [] : [];
  main.append(widget("Governance audit trail", { flush: true }, governance ? dataTable([
    { label: "#", get: (row) => row.seq, num: true }, { label: "When", get: (row) => row.at },
    { label: "Actor", get: (row) => row.actor, filter: true }, { label: "Action", get: (row) => row.action, render: (row) => h("code", {}, row.action), filter: true },
    { label: "Detail", get: (row) => fmt(row.detail) },
  ], trail, { sort: 0, dir: -1, placeholder: "Search the audit trail…", empty: "No governance changes recorded." }) : h("div", { class: "notice" }, "No member has reported the repository's governance yet; it is sent with the next upload.")));
  const commands = await api("/api/events", filterParams({ flow: "F13", limit: 500 }));
  main.append(h("h2", { class: "section" }, "Command and audit events"), widget("Events of flow F13 · " + compact(commands.total), { flush: true }, eventsTable(commands.events, { pageSize: 25 })));
}

async function governanceView(main) {
  if (needRepository(main)) return;
  let data;
  try { data = await api("/api/governance"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
  const passing = data.policies.filter((policy) => policy.passed).length;
  main.append(h("div", { class: "hero" }, h("div", {}, h("h1", {}, "Governance"), h("div", { class: "sub" }, "As reported by ", data.reported_by ? data.reported_by.actor : "a member", " · ", when(data.reported_at), " · results describe that machine's working tree")),
    h("div", { class: "facts" }, h("div", { class: "fact" }, "Generation", h("b", {}, data.generation)), h("div", { class: "fact" }, "Policies passing", h("b", {}, passing + " / " + data.policies.length)), h("div", { class: "fact" }, "Autonomy", h("b", {}, data.autonomy.mode)))));
  main.append(h("div", { class: "kpis" }, kpi("Selections", data.selections.length), kpi("Unresolved selections", data.selections.filter((row) => !row.resolved).length, { tone: data.selections.some((row) => !row.resolved) ? "bad" : "good" }),
    kpi("Policies failing", data.policies.length - passing, { tone: passing < data.policies.length ? "bad" : "good" }), kpi("Checkpoints", data.checkpoints.length), kpi("Findings", data.findings.length, { tone: data.findings.length ? "bad" : "good" }),
    kpi("Admins", data.admins ? data.admins.admins.length + 1 : 1, { note: "owner included" }), kpi("Credits per session", data.autonomy.session_credits)));
  main.append(h("div", { class: "grid" },
    widget("Policies", { span: 6, flush: true }, dataTable([{ label: "Policy", get: (row) => row.name }, { label: "File", get: (row) => row.file, render: (row) => h("code", {}, row.file) }, { label: "Result", get: (row) => row.passed ? "PASS" : "FAIL", render: (row) => chip(row.passed ? "PASS" : "FAIL"), filter: true },
      { label: "Commands", get: (row) => row.commands.length, render: (row) => h("div", {}, row.commands.map((command) => h("div", {}, chip(command.outcome), " ", h("code", {}, command.operation + " " + command.id), " ", h("span", { class: "muted" }, command.message)))) }], data.policies, { sort: 0, dir: 1 })),
    widget("Selections", { span: 6, flush: true }, dataTable([{ label: "Id", get: (row) => row.id, render: (row) => h("code", {}, row.id) }, { label: "Operation", get: (row) => row.operation, filter: true }, { label: "Policy", get: (row) => row.policy, filter: true },
      { label: "Resolved at", get: (row) => row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : "UNRESOLVED", render: (row) => row.resolved ? row.resolved.path + ":" + row.resolved.start_line + "-" + row.resolved.end_line : chip("UNRESOLVED") },
      { label: "Language", get: (row) => row.language, filter: true }, { label: "Gen", get: (row) => row.generation, num: true }], data.selections, { sort: 0, dir: 1 }))));
  main.append(h("div", { class: "grid" },
    widget("Checkpoints", { span: 6, flush: true }, dataTable([{ label: "Name", get: (row) => row.name }, { label: "Commit", get: (row) => row.commit, render: (row) => h("code", {}, row.commit.slice(0, 12)) }, { label: "Branch", get: (row) => row.branch, filter: true }, { label: "Created", get: (row) => row.created_at }, { label: "By", get: (row) => row.created_by, filter: true }], data.checkpoints, { sort: 3 })),
    widget("Policy context (guidance, never authorization)", { span: 6, flush: true }, dataTable([{ label: "Policy", get: (row) => row.policy, filter: true }, { label: "File", get: (row) => row.path, render: (row) => h("code", {}, row.path) }, { label: "State", get: (row) => row.state, render: (row) => chip(row.state), filter: true }], data.contexts))));
  main.append(widget("Findings", { flush: true }, dataTable([{ label: "Severity", get: (row) => row.severity, render: (row) => chip(row.severity), filter: true }, { label: "Code", get: (row) => row.code, render: (row) => h("code", {}, row.code), filter: true }, { label: "Finding", get: (row) => row.message }], data.findings, { empty: "No findings." })));
}

async function compliance(main) {
  if (needRepository(main)) return;
  const data = await api("/api/compliance");
  const counts = {};
  for (const control of data.controls) counts[control.outcome] = (counts[control.outcome] || 0) + 1;
  const items = Object.entries(counts).map(([value, count]) => ({ value, count })).sort((a, b) => b.count - a.count);
  main.append(h("div", { class: "notice" }, data.disclaimer));
  main.append(h("div", { class: "grid" },
    widget("Control outcomes", { span: 4 }, donut(items, { title: "control outcomes" })),
    widget("Telemetry Crane cannot collect", { span: 8 }, h("p", { class: "muted" }, "Hooks cannot observe these, so no control relies on them."), h("div", {}, data.unsupported_telemetry.map((name) => chip(name))))));
  main.append(widget("Controls", { flush: true }, dataTable([
    { label: "Control", get: (row) => row.control.id, render: (row) => h("code", {}, row.control.id) }, { label: "Objective", get: (row) => row.control.objective },
    { label: "Outcome", get: (row) => row.outcome, render: (row) => chip(row.outcome), filter: true }, { label: "Evidence", get: (row) => fmt(row.evidence) }, { label: "Gaps", get: (row) => fmt(row.gaps) },
    { label: "Supports", get: (row) => row.control.mappings.join(" "), render: (row) => h("div", {}, row.control.mappings.map((mapping) => chip(mapping))) },
  ], data.controls, { sort: 0, dir: 1, placeholder: "Search controls, evidence, frameworks…" })));
}

async function alerts(main) {
  if (needRepository(main)) return;
  const data = await api("/api/alerts");
  const rows = data.alerts;
  main.append(h("div", { class: "kpis" }, kpi("Alerts", rows.length), kpi("Critical", rows.filter((row) => String(row.severity).toUpperCase() === "CRITICAL").length, { tone: "crit" }),
    kpi("High", rows.filter((row) => String(row.severity).toUpperCase() === "HIGH").length, { tone: "bad" }), kpi("Derived by the dashboard", rows.filter((row) => row.computed).length), kpi("Suppressed", rows.filter((row) => row.suppressed).length)));
  main.append(h("div", { class: "grid" }, seriesWidget("Alerts over time", { span: 8, by: "alert", filter: "flow=F14" }), breakdownWidget("Alerts by kind", { span: 4, by: "alert", filter: "flow=F14", view: "toplist" })));
  main.append(widget("Alerts", { flush: true }, alertTable(rows)));
}

async function repositories(main) {
  if (!scope.account) { main.append(h("div", { class: "notice" }, "No account selected.")); return; }
  const data = await api("/api/accounts/" + encodeURIComponent(scope.account));
  const choose = (row) => { scope.repository = row.repository; scope.member = ""; saveScope(); go("#/overview"); };
  main.append(h("div", { class: "hero" }, h("div", {}, h("h1", {}, scope.account), h("div", { class: "sub" }, data.repositories.length + " repositories with Crane")),
    h("div", { class: "facts" }, h("div", { class: "fact" }, "Events", h("b", {}, compact(data.repositories.reduce((sum, row) => sum + row.events, 0)))), h("div", { class: "fact" }, "Denied", h("b", {}, compact(data.repositories.reduce((sum, row) => sum + row.denied, 0)))), h("div", { class: "fact" }, "Alerts", h("b", {}, compact(data.repositories.reduce((sum, row) => sum + row.alerts, 0)))))));
  main.append(h("div", { class: "grid" }, widget("Events by repository", { span: 6 }, toplist(data.repositories.map((row) => ({ value: row.remote.replace(/^github\.com\//, ""), count: row.events, row })).sort((a, b) => b.count - a.count), { onPick: (value) => { const row = data.repositories.find((item) => item.remote.endsWith(value)); if (row) choose(row); } })),
    widget("Denials by repository", { span: 6 }, toplist(data.repositories.filter((row) => row.denied).map((row) => ({ value: row.remote.replace(/^github\.com\//, ""), count: row.denied })).sort((a, b) => b.count - a.count), { onPick: (value) => { const row = data.repositories.find((item) => item.remote.endsWith(value)); if (row) choose(row); } }))));
  main.append(widget("Repositories", { flush: true }, dataTable([
    { label: "Repository", get: (row) => row.remote, render: (row) => h("a", { href: "#/overview", onclick: (event) => { event.preventDefault(); choose(row); } }, row.remote.replace(/^github\.com\//, "")) },
    { label: "Members", get: (row) => row.members, num: true }, { label: "Sessions", get: (row) => row.sessions, num: true }, { label: "Events", get: (row) => row.events, num: true },
    { label: "Denied", get: (row) => row.denied, num: true }, { label: "Confirmed bypasses", get: (row) => row.bypasses_confirmed, render: (row) => row.bypasses_confirmed ? chip("BYPASS_CONFIRMED", row.bypasses_confirmed + " confirmed") : "0" },
    { label: "Alerts", get: (row) => row.alerts, num: true }, { label: "Last activity", get: (row) => row.last_activity, render: (row) => ago(row.last_activity) },
  ], data.repositories, { sort: 7, onRow: choose, placeholder: "Search repositories…", empty: "A repository appears once a member's Crane uploads from a clone of a GitHub repository owned by this account." })));
}

async function settings(main) {
  if (needRepository(main)) return;
  const [health, repo, integrations, memberData] = await Promise.all([api("/api/health"), api("/api/repo"), api("/api/integrations"), api("/api/members")]);
  const input = h("input", { type: "text", placeholder: "12-character code", "aria-label": "approval code", maxlength: "12" });
  const approve = (action) => () => { const code = input.value.trim().toUpperCase(); if (code) location.href = "/auth/approve?action=" + action + "&code=" + encodeURIComponent(code); };
  main.append(h("div", { class: "grid" },
    widget("Approvals", { span: 6 }, h("p", { class: "muted" }, "Enter the code printed by 'crane agent link' (links a machine's upload key to your GitHub account; you need access to the repository) or 'crane admin register' (confirms the owner key; you must administer the repository). GitHub checks your role once; Foxx keeps no GitHub token."),
      h("div", { class: "table-tools", style: "border:0;padding:0" }, input, h("button", { class: "btn primary", onclick: approve("link") }, "Link machine"), h("button", { class: "btn", onclick: approve("owner") }, "Confirm owner key"))),
    widget("Repository connection", { span: 6 }, repo.connection ? h("p", {}, repo.connection.repository + " via " + repo.connection.method, h("br"), h("span", { class: "muted" }, "privileges: " + fmt(repo.connection.privileges))) : h("p", { class: "muted" }, repo.remote + " (connection not reported yet)"),
      repo.owner ? h("p", {}, "Owner key ", h("code", {}, repo.owner.owner_key.slice(0, 16) + "…"), " confirmed by @" + repo.owner.confirmed_by + " (GitHub admin) on " + when(repo.owner.confirmed_at)) : h("p", { class: "muted" }, "No GitHub-confirmed owner key yet: the owner runs 'crane admin register' and confirms it here."))));
  const hidden = memberData.members.filter((row) => !row.selected && !row.linked_by).length;
  if (hidden && !scope.member) main.append(h("div", { class: "notice" }, hidden + " machine(s) nobody linked to a GitHub account are hidden; choose one in Member to see its activity."));
  main.append(widget("Members", { flush: true }, dataTable([
    { label: "Member", get: (row) => row.member.actor, render: (row) => member(row.member) },
    { label: "Linked", get: (row) => row.linked_by ? "LINKED" : "UNLINKED", render: (row) => row.linked_by ? chip("LINKED", "linked by @" + row.linked_by) : chip("UNLINKED", "run 'crane agent link' on that machine"), filter: true },
    { label: "GitHub", get: (row) => row.linked_by ? "@" + row.linked_by : "" }, { label: "Shown", get: (row) => row.selected ? "yes" : "no", filter: true },
    { label: "Remote", get: (row) => row.verified ? "verified" : "missing", render: (row) => chip(row.verified ? "verified" : "missing") },
    { label: "Events", get: (row) => row.events, num: true }, { label: "Sessions", get: (row) => row.sessions, num: true },
    { label: "Last upload", get: (row) => row.last_upload, render: (row) => ago(row.last_upload) }, { label: "Crane", get: (row) => row.crane_version, filter: true },
  ], memberData.members, { sort: 5 })));
  main.append(h("h2", { class: "section" }, "System health"), h("div", { class: "kpis" },
    kpi("Crane", health.reported_crane_version || health.crane_version, { format: String, note: "schema v" + health.schema_version + ", events v" + health.spec_version }),
    kpi("Registry generation", health.registry_generation, { format: fmt }), kpi("Events", health.events, { note: "chains " + health.event_chain.status + (health.truncated ? " (most recent shown)" : "") }),
    kpi("Specification events", health.spec_events), kpi("Ledger", health.ledger_entries, { note: health.ledger_reconciles ? "reconciles" : "DOES NOT reconcile", tone: health.ledger_reconciles ? "" : "bad" }), kpi("Mode", health.mode, { format: String })));
  main.append(h("div", { class: "grid" },
    widget("Event chains by member", { span: 6, flush: true }, dataTable([{ label: "Member", render: (row) => member(row.member) }, { label: "Chain", get: (row) => row.event_chain.status, render: (row) => chip(row.event_chain.status), filter: true }, { label: "Events", get: (row) => row.event_chain.length, num: true }, { label: "Ledger", get: (row) => row.ledger_reconciles ? "reconciles" : "FAIL", render: (row) => row.ledger_reconciles ? "reconciles" : chip("FAIL") }], health.event_chain.members, { noTools: true })),
    widget("Integrations (non-secret settings)", { span: 6, flush: true }, dataTable([{ label: "Integration", get: (row) => row.name }, { label: "Member", render: (row) => member(row.member) }, { label: "Settings", get: (row) => fmt(row.settings) }, { label: "Secrets", get: (row) => row.secret_fields.length, render: (row) => row.secret_fields.length + " on the member's machine" }, { label: "Configured", get: (row) => row.configured_at, render: (row) => when(row.configured_at * 1000) }], integrations.integrations, { noTools: true, empty: "No integrations configured." }))));
  const signIns = widget("Your sign-ins", { flush: true }, h("div", { class: "pager", style: "justify-content:flex-start" }, h("button", { class: "btn", onclick: async () => {
    const data = await api("/api/me/sign-ins");
    put(signIns.body, eventsTable(data.events, { pageSize: 10 }));
  } }, "Show my recent sign-ins")));
  main.append(signIns, h("p", { class: "muted" }, "This dashboard is read-only. Policies, checkpoints, budgets, approvals, and integrations are changed with the Crane CLI by a human."));
}

// ---------------------------------------------------------------- chrome: navigation, scope, time, filters, search

function drawNav(page) {
  put(document.getElementById("nav"), ...NAV.map(([group, pages]) => h("div", {}, h("h6", {}, group), pages.map(([id, label, path]) => h("a", { href: "#/" + id, class: page === id || (page === "sessions" && id === "sessions") ? "active" : "", title: label }, icon(path), h("span", { class: "label" }, label))))));
}
function drawTime() {
  const custom = filter.since || filter.until;
  put(document.getElementById("time"), h("legend", { class: "visually-hidden" }, "Time range"), ...RANGES.map(([name]) => h("button", { type: "button", class: !custom && filter.range === name ? "on" : "", onclick: () => { filter.range = name; filter.since = null; filter.until = null; saveFilter(); render(); } }, name)));
}
function drawFilterBar() {
  const bar = document.getElementById("filterbar");
  if (!scope.repository) { bar.hidden = true; return; }
  bar.hidden = false;
  const pills = [];
  if (filter.since || filter.until) pills.push(h("span", { class: "pill" }, h("b", {}, "time"), rangeLabel(), h("button", { "aria-label": "Remove time range", onclick: () => { filter.since = null; filter.until = null; saveFilter(); render(); } }, "×")));
  if (filter.session) pills.push(h("span", { class: "pill" }, h("b", {}, "session"), filter.session.slice(0, 18), h("button", { "aria-label": "Remove session filter", onclick: () => { filter.session = ""; saveFilter(); render(); } }, "×")));
  for (const [name, values] of Object.entries(filter.facets)) for (const value of values || []) {
    const label = name === "member" ? ((members.find((row) => row.member.key === value) || {}).member || { actor: value.slice(0, 8) }).actor : value;
    pills.push(h("span", { class: "pill" }, h("b", {}, FACET_LABELS[name] ? FACET_LABELS[name].toLowerCase() : name), label, h("button", { "aria-label": "Remove " + name + " " + value, onclick: () => { toggleFacet(name, value); render(); } }, "×")));
  }
  let typing;
  const search = h("input", { type: "search", placeholder: "Search events: text in any attribute…", value: filter.q, "aria-label": "Search events", oninput: (event) => { clearTimeout(typing); typing = setTimeout(() => { filter.q = event.target.value.trim(); saveFilter(); render(); }, 600); }, onkeydown: (event) => { if (event.key === "Enter") { clearTimeout(typing); filter.q = event.target.value.trim(); saveFilter(); render(); } } });
  const add = h("button", { class: "btn", onclick: (event) => filterMenu(event.currentTarget) }, "+ Add filter");
  put(bar, h("span", { class: "lead" }, "Filter"), search, ...pills, add, activeFilters() ? h("button", { class: "btn link", onclick: () => { filter.facets = {}; filter.q = ""; filter.session = ""; filter.since = null; filter.until = null; saveFilter(); render(); } }, "Clear all") : null,
    h("span", { class: "muted", style: "margin-left:auto;font-size:11px" }, "Applies to every graph, list, and eval"));
}
async function filterMenu(anchor) {
  document.querySelectorAll(".menu").forEach((node) => node.remove());
  const cache = await facets();
  const values = h("div", { class: "values" });
  const show = (name) => put(values, ...(((cache.facets || {})[name]) || []).map((item) => h("button", { onclick: () => { menu.remove(); addFacet(name, item.value); render(); } }, h("span", {}, item.label || item.value), h("em", {}, compact(item.count)))));
  const chooser = select("Facet", "severity", FACETS.map((name) => [name, FACET_LABELS[name]]), show);
  const menu = h("div", { class: "menu" }, h("strong", {}, "Filter by"), chooser, values);
  const box = anchor.getBoundingClientRect();
  menu.style.left = Math.min(box.left, window.innerWidth - 260) + "px"; menu.style.top = box.bottom + 6 + "px";
  document.body.append(menu);
  show("severity");
  setTimeout(() => document.addEventListener("click", function close(event) { if (!menu.contains(event.target)) { menu.remove(); document.removeEventListener("click", close); } }), 0);
}
// The search palette: pages, repositories, evals, sessions, and event names
async function openPalette() {
  if (document.querySelector(".palette")) return;
  const items = [];
  for (const [, pages] of NAV) for (const [id, label] of pages) items.push({ kind: "Page", label, run: () => go("#/" + id) });
  for (const account of (me ? me.accounts : [])) for (const repository of (me.repositories || []).filter((item) => item.account === account.name)) {
    items.push({ kind: "Repository", label: repository.remote.replace(/^github\.com\//, ""), hint: account.kind, run: () => { scope.account = account.name; scope.repository = repository.id; scope.member = ""; saveScope(); go("#/overview"); } });
  }
  const input = h("input", { type: "search", placeholder: "Search repositories, metrics, sessions, events, pages…", "aria-label": "Search" });
  const list = h("ul", { role: "listbox" });
  const scrim = h("div", { class: "scrim", onclick: () => close() });
  const palette = h("div", { class: "palette", role: "dialog", "aria-label": "Search" }, input, list);
  const close = () => { palette.remove(); scrim.remove(); };
  document.body.append(scrim, palette);
  input.focus();
  let shown = [], selected = 0;
  const draw = () => {
    const q = input.value.trim().toLowerCase();
    shown = items.filter((item) => !q || (item.kind + " " + item.label + " " + (item.hint || "")).toLowerCase().includes(q)).slice(0, 60);
    selected = Math.min(selected, Math.max(0, shown.length - 1));
    put(list, ...(shown.length ? shown.map((item, index) => h("li", { class: index === selected ? "on" : "", role: "option", onclick: () => { close(); item.run(); }, onmousemove: () => { if (selected !== index) { selected = index; draw(); } } }, h("span", { class: "kind" }, item.kind), h("span", {}, item.label), item.hint !== undefined ? h("span", { class: "hint" }, String(item.hint)) : null)) : [h("li", { class: "muted" }, "Nothing matches.")]));
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
  // Repository content arrives after the palette opens, so typing is never delayed
  const [overviewData, sessionData, cache] = await Promise.all([lastEvals ? Promise.resolve({ evals: lastEvals }) : api("/api/overview", filterParams()).catch(() => ({})), api("/api/sessions").catch(() => ({ sessions: [] })), facets()]);
  for (const [name, value] of Object.entries(overviewData.evals || {})) if (value !== null && typeof value !== "object") items.push({ kind: "Metric", label: name, hint: compact(value), run: () => { evalQuery = name; go("#/overview"); } });
  for (const row of sessionData.sessions || []) items.push({ kind: "Session", label: row.session.foxx_session_id, hint: (row.member ? row.member.actor + " · " : "") + row.session.provider, run: () => go("#/sessions/" + encodeURIComponent(row.session.foxx_session_id)) });
  for (const item of (cache.facets || {}).event || []) items.push({ kind: "Event", label: item.value, hint: compact(item.count), run: () => drill({ facets: { event: item.value } }) });
  for (const item of (cache.facets || {}).flow || []) items.push({ kind: "Flow", label: item.value, hint: compact(item.count), run: () => drill({ facets: { flow: item.value } }) });
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

const PAGES = { overview, events, sessions, agents, audit, governance: governanceView, compliance, alerts, repositories, settings };
async function render() {
  closeDrawer();
  tooltip.hide();
  document.querySelectorAll(".menu").forEach((node) => node.remove());
  const [path, search] = location.hash.replace(/^#\//, "").split("?");
  // Older links carried event filters in the address; they become the global filter
  if (search) {
    const params = new URLSearchParams(search);
    for (const [key, value] of params) { if (FACETS.includes(key)) addFacet(key, value); else if (key === "session") filter.session = value; else if (key === "type" || key === "name") addFacet("event", value); }
    saveFilter();
    history.replaceState(null, "", "#/" + path);
  }
  const [page, id] = (path || "overview").split("/");
  facetCache = null;
  drawNav(page);
  drawTime();
  const main = document.getElementById("main");
  put(main, h("p", { class: "skeleton" }, "Loading…"));
  try {
    me = await api("/api/me");
    await controls();
    drawFilterBar();
    put(main, );
    if (approval) { main.append(h("div", { class: "notice" + (approval.ok ? "" : " error") }, approval.message)); approval = null; }
    if (page === "sessions" && id) await session(main, decodeURIComponent(id));
    else await (PAGES[page] || overview)(main);
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
document.getElementById("nav").addEventListener("click", () => document.getElementById("app").classList.remove("open"));
document.getElementById("search-open").addEventListener("click", openPalette);
document.addEventListener("keydown", (event) => {
  const typing = /^(input|select|textarea)$/i.test((event.target || {}).tagName || "");
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") { event.preventDefault(); openPalette(); }
  else if (event.key === "/" && !typing) { event.preventDefault(); openPalette(); }
  else if (event.key === "Escape") { closeDrawer(); document.querySelectorAll(".menu").forEach((node) => node.remove()); }
});
window.addEventListener("hashchange", render);
render();
