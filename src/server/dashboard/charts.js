"use strict";
// The dashboard's chart library and shared helpers, drawn as SVG (the page loads no other script):
// events over time (bars, lines, areas) with drag-to-zoom, donut, top list, sparkline, Gantt chart
// of agent actions, event swimlanes, topology (members, agents, tools, resources), the dependency
// graph between flows, histograms, and an alert rule's value against its threshold. Every chart has
// a tooltip, a click that drills into the events behind it, and redraws when its box changes size.
// Every time is written as DD:MM:YYYY:HH:MM:SS (local time).

(function () {
  const Foxx = (window.Foxx = window.Foxx || {});

  // ---------------------------------------------------------------- DOM and formatting

  /** Create an element with attributes, listeners (on…), and children */
  Foxx.h = function h(tag, attributes, ...children) {
    const node = document.createElement(tag);
    for (const [key, value] of Object.entries(attributes || {})) {
      if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
      else if (key === "dataset") Object.assign(node.dataset, value);
      else if (key === "style" && typeof value === "object") Object.assign(node.style, value);
      else if (value !== undefined && value !== null && value !== false) node.setAttribute(key, value === true ? "" : value);
    }
    for (const child of children.flat(Infinity)) {
      if (child === null || child === undefined || child === false) continue;
      node.append(child instanceof Node ? child : document.createTextNode(String(child)));
    }
    return node;
  };
  /** Create an SVG element */
  Foxx.svg = function svg(tag, attributes, ...children) {
    const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
    for (const [key, value] of Object.entries(attributes || {})) {
      if (key === "style") Object.assign(node.style, value);
      else if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
      else if (value !== undefined && value !== null) node.setAttribute(key, value);
    }
    for (const child of children.flat(Infinity)) if (child !== null && child !== undefined && child !== false) node.append(child instanceof Node ? child : document.createTextNode(String(child)));
    return node;
  };
  /** Replace a node's children, skipping empty ones */
  Foxx.put = function put(node, ...children) {
    node.replaceChildren(...children.flat(Infinity).filter((child) => child !== null && child !== undefined && child !== false));
    return node;
  };
  const pad = (number, width) => String(number).padStart(width || 2, "0");
  /** A time as DD:MM:YYYY:HH:MM:SS (local) */
  Foxx.dt = function dt(millis) {
    if (millis === null || millis === undefined || millis === "" || Number.isNaN(Number(millis))) return "—";
    const date = new Date(Number(millis));
    return pad(date.getDate()) + ":" + pad(date.getMonth() + 1) + ":" + date.getFullYear() + ":" + pad(date.getHours()) + ":" + pad(date.getMinutes()) + ":" + pad(date.getSeconds());
  };
  /** A time as [DD:MM:YYYY:, HH:MM:SS] for two-line axis labels (read together, the full stamp) */
  Foxx.dtParts = function dtParts(millis) {
    const text = Foxx.dt(millis);
    return [text.slice(0, 11), text.slice(11)];
  };
  /** A duration in words: 850 ms, 12.4 s, 3 min 5 s, 2 h 10 min */
  Foxx.duration = function duration(millis) {
    if (millis === null || millis === undefined) return "—";
    const ms = Number(millis);
    if (ms < 1000) return Math.round(ms) + " ms";
    if (ms < 60000) return (ms / 1000).toFixed(1) + " s";
    if (ms < 3600000) return Math.floor(ms / 60000) + " min " + Math.round((ms % 60000) / 1000) + " s";
    return Math.floor(ms / 3600000) + " h " + Math.round((ms % 3600000) / 60000) + " min";
  };
  Foxx.fmt = function fmt(value) {
    if (value === null || value === undefined) return "—";
    if (typeof value === "number") return Number.isInteger(value) ? value.toLocaleString() : value.toFixed(3);
    if (Array.isArray(value)) return value.length ? value.map(Foxx.fmt).join(", ") : "none";
    if (typeof value === "object") return Object.entries(value).map(([key, item]) => key + ": " + Foxx.fmt(item)).join("  ");
    return String(value);
  };
  Foxx.compact = function compact(value) {
    if (value === null || value === undefined) return "—";
    if (typeof value !== "number") return Foxx.fmt(value);
    if (Math.abs(value) >= 1e6) return (value / 1e6).toFixed(1) + "M";
    if (Math.abs(value) >= 1e4) return (value / 1e3).toFixed(1) + "k";
    return Number.isInteger(value) ? value.toLocaleString() : value.toFixed(2);
  };

  // ---------------------------------------------------------------- colors and tooltip

  Foxx.PALETTE = ["#7b5fc7", "#3fa9f5", "#2fbf9c", "#f2b33d", "#ef6461", "#e573b1", "#9ccc65", "#5c6bc0", "#ff9f43", "#26c6da", "#a1887f", "#d4e157"];
  Foxx.NAMED = {
    INFO: "#3f8fe0", LOW: "#26b3b3", MEDIUM: "#f0a830", HIGH: "#ee6a4f", CRITICAL: "#c2185b",
    ALLOW: "#2fbf7c", DENY: "#e5484d", REQUIRE_APPROVAL: "#f0a830", QUARANTINE: "#c2185b", ERROR: "#a33a3a", NOT_REQUIRED: "#9aa0b4",
    MACHINE: "#7b5fc7", SERVICE: "#3fa9f5", DERIVED: "#2fbf9c", WORKFLOW: "#f2b33d",
    PREVENTED: "#2fbf7c", DETECTED_AFTER_EFFECT: "#ee6a4f", BYPASS_CONFIRMED: "#c2185b", COVERAGE_GAP: "#f0a830", NOT_APPLICABLE: "#9aa0b4",
    SATISFIED: "#2fbf7c", PARTIAL: "#f0a830", NOT_SATISFIED: "#e5484d", NO_EVIDENCE: "#9aa0b4", OK: "#2fbf7c", FAILED: "#e5484d",
    FIRING: "#e5484d", RESOLVED: "#2fbf7c", DELIVERED: "#2fbf7c", QUEUED: "#3fa9f5", DRY_RUN: "#9aa0b4", PENDING: "#f0a830", APPLIED: "#2fbf7c", REJECTED: "#e5484d",
    OTHER: "#9aa0b4", NONE: "#9aa0b4",
  };
  /** The color of a value: its own when it has one, else the palette's */
  Foxx.color = function color(key, index) { return Foxx.NAMED[key] || Foxx.PALETTE[Math.abs(index || 0) % Foxx.PALETTE.length]; };
  /** A stable palette color for any text */
  Foxx.hashColor = function hashColor(text) {
    if (Foxx.NAMED[text]) return Foxx.NAMED[text];
    let hash = 0;
    for (const character of String(text)) hash = (hash * 31 + character.charCodeAt(0)) | 0;
    return Foxx.PALETTE[Math.abs(hash) % Foxx.PALETTE.length];
  };
  Foxx.tooltip = {
    show(event, content) {
      const box = document.getElementById("tooltip");
      if (!box) return;
      Foxx.put(box, content);
      box.hidden = false;
      const x = Math.min(event.clientX + 14, window.innerWidth - box.offsetWidth - 8);
      const y = Math.min(event.clientY + 14, window.innerHeight - box.offsetHeight - 8);
      box.style.left = Math.max(4, x) + "px";
      box.style.top = Math.max(4, y) + "px";
    },
    hide() { const box = document.getElementById("tooltip"); if (box) box.hidden = true; },
  };
  const tip = (title, rows, hint) => Foxx.h("div", {}, Foxx.h("div", { class: "tip-title" }, title), (rows || []).map(([label, value, color]) => Foxx.h("div", { class: "row" }, color ? Foxx.h("i", { style: "background:" + color }) : null, Foxx.h("span", {}, label), Foxx.h("b", {}, value))), hint ? Foxx.h("div", { class: "tip-hint" }, hint) : null);
  Foxx.tip = tip;

  // ---------------------------------------------------------------- shared chart pieces

  /** A box that draws its chart whenever its width changes */
  function responsive(draw, className) {
    const box = Foxx.h("div", { class: "chart " + (className || "") });
    let drawnWidth = 0;
    const redraw = () => { const width = box.clientWidth; if (!width) return; drawnWidth = width; draw(box, width); };
    box.redraw = redraw;
    new ResizeObserver(() => { if (box.clientWidth !== drawnWidth) redraw(); }).observe(box);
    return box;
  }
  /** The top of an axis: four whole-number steps of 1, 2, or 5 times a power of ten */
  function niceMax(max) {
    if (max <= 4) return 4;
    const raw = max / 4, power = Math.pow(10, Math.floor(Math.log10(raw)));
    for (const step of [1, 2, 5, 10]) if (step * power >= raw) return Math.ceil(step * power) * 4;
    return 40 * power;
  }
  /** Time labels along an axis, two lines each (date, time) */
  function timeAxis(group, start, end, left, width, y, count) {
    const labels = Math.max(2, Math.min(count || 8, Math.floor(width / 120)));
    for (let index = 0; index < labels; index += 1) {
      const t = start + ((end - start) * index) / (labels - 1);
      const x = left + (width * index) / (labels - 1);
      const [date, time] = Foxx.dtParts(t);
      const anchor = index === 0 ? "start" : index === labels - 1 ? "end" : "middle";
      group.append(Foxx.svg("text", { x, y, "text-anchor": anchor }, date), Foxx.svg("text", { x, y: y + 11, "text-anchor": anchor }, time));
    }
  }
  /** Drag across a time axis to zoom: calls onZoom(since, until); a plain click passes through */
  function brush(chart, plot, scale, onZoom) {
    if (!onZoom) return;
    let from = null, band = null;
    const point = (event) => { const box = chart.getBoundingClientRect(); return Math.min(plot.right, Math.max(plot.left, ((event.clientX - box.left) / box.width) * plot.total)); };
    chart.addEventListener("mousedown", (event) => {
      if (event.button !== 0) return;
      from = point(event);
      band = Foxx.svg("rect", { class: "brush", x: from, y: plot.top, width: 0, height: plot.height });
      chart.append(band);
    });
    chart.addEventListener("mousemove", (event) => {
      if (from === null || !band) return;
      const to = point(event);
      band.setAttribute("x", Math.min(from, to));
      band.setAttribute("width", Math.abs(to - from));
    });
    const finish = (event) => {
      if (from === null) return;
      const to = point(event);
      if (band) band.remove();
      const moved = Math.abs(to - from);
      const start = from;
      from = null;
      band = null;
      if (moved > 6) {
        chart.dataset.dragged = "1";
        setTimeout(() => { delete chart.dataset.dragged; }, 0);
        onZoom(Math.floor(scale(Math.min(start, to))), Math.ceil(scale(Math.max(start, to))));
      }
    };
    chart.addEventListener("mouseup", finish);
    chart.addEventListener("mouseleave", (event) => { if (from !== null) finish(event); });
  }
  const dragged = (chart) => chart.dataset.dragged === "1";
  /** A legend whose entries hide and show their series */
  function legend(series, hidden, redraw, onPick) {
    return Foxx.h("div", { class: "legend" }, series.map((item, index) => Foxx.h("button", {
      type: "button", class: hidden.has(item.key) ? "off" : "", title: "Click to show or hide · double-click to open these events",
      onclick: () => { if (hidden.has(item.key)) hidden.delete(item.key); else hidden.add(item.key); redraw(); },
      ondblclick: () => onPick && onPick({ key: item.key }),
    }, Foxx.h("i", { style: "background:" + Foxx.color(item.key, index) }), item.label || item.key, Foxx.h("span", { class: "count" }, Foxx.compact(item.total)))));
  }
  const empty = (text) => Foxx.h("div", { class: "empty" }, text || "No events in this range and filter.");

  const charts = (Foxx.charts = {});

  // ---------------------------------------------------------------- events over time

  /** Events over time: stacked bars, lines, or areas; hover a bucket for its counts, click it to
   * open its events, drag across to zoom into a range */
  charts.timeseries = function timeseries(data, options) {
    const hidden = new Set();
    const height = options.height || 220;
    const box = responsive((box, width) => {
      const series = data.series || [];
      if (!series.length || !data.total) { Foxx.put(box, empty()); return; }
      const visible = series.filter((item) => !hidden.has(item.key));
      const count = data.buckets, left = 44, right = 10, top = 8, bottom = 30;
      const plotWidth = width - left - right, plotHeight = height - top - bottom, step = plotWidth / count;
      const stacked = options.type === "bars";
      let max = 0;
      for (let index = 0; index < count; index += 1) {
        if (stacked) max = Math.max(max, visible.reduce((sum, item) => sum + item.points[index], 0));
        else for (const item of visible) max = Math.max(max, item.points[index]);
      }
      max = niceMax(max);
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const end = data.start + count * data.bucket_ms;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "events over time", class: "zoomable" });
      const axis = Foxx.svg("g", { class: "axis" });
      for (let tick = 0; tick <= 4; tick += 1) {
        const value = (max / 4) * tick, ty = y(value);
        axis.append(Foxx.svg("line", { class: "grid-line", x1: left, x2: width - right, y1: ty, y2: ty, style: { strokeDasharray: tick ? "2 3" : "" } }), Foxx.svg("text", { x: left - 6, y: ty + 3, "text-anchor": "end" }, Foxx.compact(value)));
      }
      timeAxis(axis, data.start, end, left, plotWidth, height - 17);
      chart.append(axis);
      const describe = (index) => tip(Foxx.dt(data.start + index * data.bucket_ms) + " → " + Foxx.dt(data.start + (index + 1) * data.bucket_ms - 1),
        visible.filter((item) => item.points[index]).map((item) => [item.label || item.key, item.points[index], Foxx.color(item.key, series.indexOf(item))]), "Click: open these events · drag: zoom in");
      const pick = (index, key) => { if (!dragged(chart) && options.onPick) options.onPick({ since: data.start + index * data.bucket_ms, until: data.start + (index + 1) * data.bucket_ms - 1, key }); };
      if (stacked) {
        for (let index = 0; index < count; index += 1) {
          let base = 0;
          for (const item of visible) {
            const value = item.points[index];
            if (!value) continue;
            const y1 = y(base + value), y0 = y(base);
            chart.append(Foxx.svg("rect", { class: "bar", x: left + index * step + Math.min(1, step * 0.1), width: Math.max(1, step - Math.max(1, step * 0.2)), y: y1, height: Math.max(1, y0 - y1), style: { fill: Foxx.color(item.key, series.indexOf(item)) },
              onmousemove: (event) => Foxx.tooltip.show(event, describe(index)), onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); pick(index, item.key); } }));
            base += value;
          }
        }
      } else {
        for (const item of visible) {
          const color = Foxx.color(item.key, series.indexOf(item));
          const points = item.points.map((value, index) => [left + index * step + step / 2, y(value)]);
          const line = points.map(([px, py], index) => (index ? "L" : "M") + px.toFixed(1) + " " + py.toFixed(1)).join(" ");
          if (options.type === "area") chart.append(Foxx.svg("path", { d: line + " L" + points[points.length - 1][0] + " " + y(0) + " L" + points[0][0] + " " + y(0) + " Z", style: { fill: color, opacity: "0.18" } }));
          chart.append(Foxx.svg("path", { d: line, style: { fill: "none", stroke: color, strokeWidth: "2" } }));
        }
        for (let index = 0; index < count; index += 1) {
          chart.append(Foxx.svg("rect", { x: left + index * step, y: top, width: step, height: plotHeight, style: { fill: "transparent", cursor: "pointer" }, onmousemove: (event) => Foxx.tooltip.show(event, describe(index)), onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); pick(index); } }));
        }
      }
      brush(chart, { left, right: left + plotWidth, top, height: plotHeight, total: width }, (x) => data.start + ((x - left) / plotWidth) * (end - data.start), options.onZoom);
      Foxx.put(box, chart, legend(series, hidden, box.redraw, options.onPick));
    });
    return box;
  };

  // ---------------------------------------------------------------- donut, top list, sparkline

  charts.donut = function donut(items, options) {
    const total = items.reduce((sum, item) => sum + item.count, 0);
    if (!total) return empty();
    const chart = Foxx.svg("svg", { viewBox: "0 0 160 160", role: "img", "aria-label": options.title || "breakdown" });
    let angle = -Math.PI / 2;
    items.forEach((item, index) => {
      const sweep = (item.count / total) * Math.PI * 2;
      const end = angle + sweep, large = sweep > Math.PI ? 1 : 0;
      const point = (radius, theta) => (80 + radius * Math.cos(theta)).toFixed(2) + " " + (80 + radius * Math.sin(theta)).toFixed(2);
      const d = items.length === 1
        ? "M80 8 A72 72 0 1 1 79.99 8 L79.99 38 A42 42 0 1 0 80 38 Z"
        : "M" + point(72, angle) + " A72 72 0 " + large + " 1 " + point(72, end) + " L" + point(42, end) + " A42 42 0 " + large + " 0 " + point(42, angle) + " Z";
      chart.append(Foxx.svg("path", { class: "slice", d, style: { fill: Foxx.color(item.value, index) },
        onmousemove: (event) => Foxx.tooltip.show(event, tip(item.label || item.value, [["events", item.count + " (" + ((item.count / total) * 100).toFixed(1) + "%)"]], options.hint || "Click: open these events")),
        onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); if (options.onPick) options.onPick(item.value); } }));
      angle = end;
    });
    chart.append(Foxx.svg("text", { x: 80, y: 78, "text-anchor": "middle", class: "donut-total" }, Foxx.compact(total)), Foxx.svg("text", { x: 80, y: 96, "text-anchor": "middle", class: "donut-unit" }, options.unit || "events"));
    return Foxx.h("div", { class: "donut" }, chart, Foxx.h("div", { class: "legend vertical" }, items.map((item, index) => Foxx.h("button", { type: "button", onclick: () => options.onPick && options.onPick(item.value) },
      Foxx.h("i", { style: "background:" + Foxx.color(item.value, index) }), item.label || item.value, options.explain ? options.explain(item.value) : null, Foxx.h("span", { class: "count" }, item.count + " · " + ((item.count / total) * 100).toFixed(0) + "%")))));
  };
  charts.toplist = function toplist(items, options) {
    if (!items.length) return empty(options && options.empty);
    const max = Math.max(...items.map((item) => item.count));
    return Foxx.h("div", { class: "toplist" }, items.map((item, index) => Foxx.h("div", { class: "row", role: "button", tabindex: "0", title: (options && options.hint) || "Open these events", onclick: () => options && options.onPick && options.onPick(item.value), onkeydown: (event) => { if (event.key === "Enter" && options && options.onPick) options.onPick(item.value); } },
      Foxx.h("span", { class: "name", title: item.label || item.value }, item.label || item.value, options && options.explain ? options.explain(item.value) : null),
      Foxx.h("div", { class: "track" }, Foxx.h("div", { class: "fill", style: "width:" + Math.max(2, (item.count / max) * 100) + "%;background:" + Foxx.color(item.value, index) })),
      Foxx.h("span", { class: "num" }, Foxx.compact(item.count)))));
  };
  charts.sparkline = function sparkline(points, color) {
    if (!points || points.length < 2) return null;
    const max = Math.max(1, ...points);
    const d = points.map((value, index) => (index ? "L" : "M") + ((index / (points.length - 1)) * 72).toFixed(1) + " " + (24 - (value / max) * 22).toFixed(1)).join(" ");
    return Foxx.svg("svg", { class: "spark", viewBox: "0 0 72 26", "aria-hidden": "true" }, Foxx.svg("path", { d, style: { fill: "none", stroke: color, strokeWidth: "1.6" } }));
  };

  // ---------------------------------------------------------------- Gantt chart

  /** The Gantt chart: a row per session (or agent, member, tool, flow, …), a bar per trace from
   * its first event to its last (and its measured tool time), colored by a chosen attribute; a
   * failed trace is outlined; hover for timestamps, click for the trace, drag to zoom */
  charts.gantt = function gantt(data, options) {
    const rows = data.rows || [];
    if (!rows.length) return empty("No actions in this range and filter.");
    const rowHeight = 24, labelWidth = Math.min(220, Math.max(120, ...rows.map((row) => String(row.label).length * 6.5))), top = 34, bottom = 6;
    const box = responsive((box, width) => {
      if (!Number.isFinite(data.start) || !Number.isFinite(data.end)) { Foxx.put(box, empty("No actions in this range and filter.")); return; }
      const start = data.start, end = Math.max(data.end, data.start + 1);
      const plotWidth = Math.max(80, width - labelWidth - 12);
      const height = top + rows.length * rowHeight + bottom;
      const x = (t) => labelWidth + ((t - start) / (end - start)) * plotWidth;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "Gantt chart of agent actions", class: "zoomable" });
      const axis = Foxx.svg("g", { class: "axis" });
      timeAxis(axis, start, end, labelWidth, plotWidth, 11, 6);
      chart.append(axis);
      rows.forEach((row, index) => {
        const y = top + index * rowHeight;
        chart.append(Foxx.svg("rect", { x: 0, y, width, height: rowHeight, class: index % 2 ? "stripe" : "stripe odd" }));
        const label = Foxx.svg("text", { x: 6, y: y + 16, class: "row-label" }, String(row.label).length > 32 ? String(row.label).slice(0, 31) + "…" : row.label);
        label.append(Foxx.svg("title", {}, row.label));
        chart.append(label);
        if (row.span && row.span.start) {
          const sx = Math.max(labelWidth, x(row.span.start)), ex = Math.min(labelWidth + plotWidth, x(row.span.end || end));
          if (ex > sx) chart.append(Foxx.svg("rect", { x: sx, y: y + 3, width: ex - sx, height: rowHeight - 6, class: "span", rx: 3 }));
        }
        for (const bar of row.bars) {
          const bx = Math.max(labelWidth, x(bar.start));
          const bw = Math.max(3, Math.min(labelWidth + plotWidth, x(bar.end)) - bx);
          const failed = bar.outcome === "FAILED";
          chart.append(Foxx.svg("rect", { class: "gantt-bar" + (failed ? " failed" : ""), x: bx, y: y + 5, width: bw, height: rowHeight - 10, rx: 2, style: { fill: Foxx.hashColor(bar.color) },
            onmousemove: (event) => Foxx.tooltip.show(event, tip(bar.label, [["start", Foxx.dt(bar.start)], ["end", Foxx.dt(bar.end)], ["duration", Foxx.duration(bar.end - bar.start)], [data.color_by || "color", bar.color, Foxx.hashColor(bar.color)], ["outcome", bar.outcome], ["severity", bar.severity], ["events", (bar.events || []).join(" → ")]], "Click: open the trace · drag: zoom")),
            onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); if (!dragged(chart) && options.onPick) options.onPick(bar); } }));
        }
      });
      brush(chart, { left: labelWidth, right: labelWidth + plotWidth, top, height: rows.length * rowHeight, total: width }, (px) => start + ((px - labelWidth) / plotWidth) * (end - start), options.onZoom);
      const colors = [...new Set(rows.flatMap((row) => row.bars.map((bar) => bar.color)))].slice(0, 16);
      Foxx.put(box, Foxx.h("div", { class: "scroll-y" }, chart), Foxx.h("div", { class: "legend" }, colors.map((color) => Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { style: "background:" + Foxx.hashColor(color) }), color)), Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { class: "outline" }), "failed trace")));
    }, "gantt");
    return box;
  };

  // ---------------------------------------------------------------- event swimlanes

  /** The event timeline: a lane per flow (or any attribute), every event a dot at its exact time,
   * colored by severity (or any attribute); hover for the event, click to open it, drag to zoom */
  charts.swimlanes = function swimlanes(data, options) {
    const lanes = data.lanes || [];
    if (!lanes.length) return empty();
    const laneText = (lane) => (options.laneLabel ? options.laneLabel(lane.key) : lane.label) + " (" + Foxx.compact(lane.count) + ")";
    const laneHeight = 26, labelWidth = Math.min(280, Math.max(120, ...lanes.map((lane) => laneText(lane).length * 6.6 + 12))), top = 34;
    const box = responsive((box, width) => {
      const start = data.start, end = Math.max(data.end, data.start + 1);
      const plotWidth = Math.max(80, width - labelWidth - 12);
      const height = top + lanes.length * laneHeight + 8;
      const x = (t) => labelWidth + ((t - start) / (end - start)) * plotWidth;
      const index = new Map(lanes.map((lane, position) => [lane.key, position]));
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "event timeline", class: "zoomable" });
      const axis = Foxx.svg("g", { class: "axis" });
      timeAxis(axis, start, end, labelWidth, plotWidth, 11, 7);
      chart.append(axis);
      lanes.forEach((lane, position) => {
        const y = top + position * laneHeight;
        chart.append(Foxx.svg("rect", { x: 0, y, width, height: laneHeight, class: position % 2 ? "stripe" : "stripe odd" }));
        const label = Foxx.svg("text", { x: 6, y: y + 17, class: "row-label" }, laneText(lane));
        label.append(Foxx.svg("title", {}, options.laneTitle ? options.laneTitle(lane.key) : lane.label));
        chart.append(label);
      });
      for (const point of data.points) {
        const position = index.get(point.lane);
        if (position === undefined) continue;
        chart.append(Foxx.svg("circle", { class: "dot", cx: x(point.t), cy: top + position * laneHeight + laneHeight / 2, r: 4, style: { fill: Foxx.hashColor(point.color) },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(point.name, [["time", Foxx.dt(point.t)], [data.color_by || "color", point.color, Foxx.hashColor(point.color)], ["session", point.session || "—"], ["", point.statement || ""]], "Click: open the event · drag: zoom")),
          onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); if (!dragged(chart) && options.onPick) options.onPick(point); } }));
      }
      brush(chart, { left: labelWidth, right: labelWidth + plotWidth, top, height: lanes.length * laneHeight, total: width }, (px) => start + ((px - labelWidth) / plotWidth) * (end - start), options.onZoom);
      const colors = [...new Set(data.points.map((point) => point.color))].slice(0, 16);
      Foxx.put(box, Foxx.h("div", { class: "scroll-y" }, chart), Foxx.h("div", { class: "legend" }, colors.map((color) => Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { style: "background:" + Foxx.hashColor(color) }), color))));
    });
    return box;
  };

  // ---------------------------------------------------------------- topology

  /** Who used what: members → agents → tools → resources, edge width by events, red by the share
   * denied; hover a node to follow its edges, click to filter by it */
  charts.topology = function topology(data, options) {
    if (!data.nodes || !data.nodes.length) return empty();
    const box = responsive((box, width) => {
      const layers = data.layers.map((_, layer) => data.nodes.filter((node) => node.layer === layer).sort((a, b) => b.count - a.count).slice(0, 14));
      const shown = new Set(layers.flat().map((node) => node.id));
      const nodeHeight = 26, gap = 10, top = 28;
      const height = top + Math.max(...layers.map((layer) => layer.length)) * (nodeHeight + gap) + 10;
      const nodeWidth = Math.max(70, Math.min(170, (width - 40) / Math.max(1, layers.length) - 40));
      const column = (layer) => 10 + layer * ((width - nodeWidth - 20) / Math.max(1, layers.length - 1));
      const position = new Map();
      layers.forEach((list, layer) => list.forEach((node, index) => position.set(node.id, { x: column(layer), y: top + index * (nodeHeight + gap) })));
      const maxEdge = Math.max(1, ...data.edges.map((edge) => edge.count));
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "topology" });
      data.layers.forEach((name, layer) => chart.append(Foxx.svg("text", { x: column(layer), y: 14, class: "layer-title" }, name)));
      const edges = [];
      for (const edge of data.edges) {
        if (!shown.has(edge.from) || !shown.has(edge.to)) continue;
        const a = position.get(edge.from), b = position.get(edge.to);
        const x1 = a.x + nodeWidth, y1 = a.y + nodeHeight / 2, x2 = b.x, y2 = b.y + nodeHeight / 2, mid = (x1 + x2) / 2;
        const share = edge.denied / Math.max(1, edge.count);
        const path = Foxx.svg("path", { class: "edge", d: "M" + x1 + " " + y1 + " C" + mid + " " + y1 + " " + mid + " " + y2 + " " + x2 + " " + y2, style: { strokeWidth: String(1 + (edge.count / maxEdge) * 12), stroke: share >= 0.1 ? "rgba(229,72,77," + (0.3 + share * 0.7) + ")" : "" },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(edge.from.split(":").slice(1).join(":") + " → " + edge.to.split(":").slice(1).join(":"), [["events", edge.count], ["denied", edge.denied]])), onmouseleave: Foxx.tooltip.hide });
        path.dataset.from = edge.from;
        path.dataset.to = edge.to;
        edges.push(path);
        chart.append(path);
      }
      for (const [id, place] of position) {
        const node = data.nodes.find((item) => item.id === id);
        const group = Foxx.svg("g", { class: "node", transform: "translate(" + place.x + "," + place.y + ")", tabindex: "0",
          onmouseenter: () => edges.forEach((edge) => edge.classList.toggle("dim", edge.dataset.from !== id && edge.dataset.to !== id)),
          onmouseleave: () => { edges.forEach((edge) => edge.classList.remove("dim")); Foxx.tooltip.hide(); },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(node.label, [["layer", data.layers[node.layer]], ["events", node.count], ["denied", node.denied]], "Click: filter every view by it")),
          onclick: () => options.onPick && options.onPick(node) });
        group.append(Foxx.svg("rect", { width: nodeWidth, height: nodeHeight, rx: 6, class: node.denied / Math.max(1, node.count) >= 0.1 ? "node-box warn" : "node-box" }), Foxx.svg("text", { x: 8, y: 17 }, node.label.length > 22 ? node.label.slice(0, 21) + "…" : node.label), Foxx.svg("text", { x: nodeWidth - 6, y: 17, "text-anchor": "end", class: "node-count" }, Foxx.compact(node.count)));
        chart.append(group);
      }
      Foxx.put(box, chart);
    });
    return box;
  };

  // ---------------------------------------------------------------- dependency graph

  /** Which flow follows which within a session: flows on a circle, an arrow per transition, width
   * by how often it happened (order only, never a claimed cause) */
  charts.dependency = function dependency(data, options) {
    if (!data.nodes || !data.nodes.length) return empty();
    const box = responsive((box, width) => {
      const size = Math.min(width, 560), center = size / 2, radius = size / 2 - 60;
      const nodes = data.nodes.slice().sort((a, b) => a.id.localeCompare(b.id));
      const place = new Map(nodes.map((node, index) => {
        const angle = (index / nodes.length) * Math.PI * 2 - Math.PI / 2;
        return [node.id, { x: center + radius * Math.cos(angle), y: center + radius * Math.sin(angle) }];
      }));
      const maxCount = Math.max(1, ...nodes.map((node) => node.count));
      const maxEdge = Math.max(1, ...data.edges.map((edge) => edge.count));
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + size + " " + size, width: size, height: size, role: "img", "aria-label": "flow dependency graph" });
      chart.append(Foxx.svg("defs", {}, Foxx.svg("marker", { id: "arrow", viewBox: "0 0 10 10", refX: "10", refY: "5", markerWidth: "9", markerHeight: "9", markerUnits: "userSpaceOnUse", orient: "auto" }, Foxx.svg("path", { d: "M0 0 L10 5 L0 10 z", class: "arrow-head" }))));
      const lines = [];
      for (const edge of data.edges) {
        const a = place.get(edge.from), b = place.get(edge.to);
        if (!a || !b) continue;
        // Bend each arrow to the left of its direction, so A → B and B → A stay apart, and stop it
        // at the edge of the target circle
        const dx = b.x - a.x, dy = b.y - a.y, length = Math.max(1, Math.hypot(dx, dy));
        const mx = (a.x + b.x) / 2 - (dy / length) * 34, my = (a.y + b.y) / 2 + (dx / length) * 34;
        const target = nodes.find((node) => node.id === edge.to);
        const radius = 12 + ((target ? target.count : 0) / maxCount) * 16;
        const ex = b.x - ((b.x - mx) / Math.max(1, Math.hypot(b.x - mx, b.y - my))) * radius, ey = b.y - ((b.y - my) / Math.max(1, Math.hypot(b.x - mx, b.y - my))) * radius;
        const line = Foxx.svg("path", { class: "edge", d: "M" + a.x + " " + a.y + " Q" + mx + " " + my + " " + ex + " " + ey, "marker-end": "url(#arrow)", style: { strokeWidth: String(1 + (edge.count / maxEdge) * 8) },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(edge.from + " → " + edge.to, [["times", edge.count]], "Order only, never a claimed cause")), onmouseleave: Foxx.tooltip.hide });
        line.dataset.from = edge.from;
        line.dataset.to = edge.to;
        lines.push(line);
        chart.append(line);
      }
      for (const node of nodes) {
        const at = place.get(node.id);
        const r = 10 + (node.count / maxCount) * 16; // edges stop 2px outside this
        const group = Foxx.svg("g", { class: "node", tabindex: "0",
          onmouseenter: () => lines.forEach((line) => line.classList.toggle("dim", line.dataset.from !== node.id && line.dataset.to !== node.id)),
          onmouseleave: () => { lines.forEach((line) => line.classList.remove("dim")); Foxx.tooltip.hide(); },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(node.id + (options.title_of ? " · " + options.title_of(node.id) : ""), [["events", node.count]], "Click: filter by this flow")),
          onclick: () => options.onPick && options.onPick(node.id) });
        group.append(Foxx.svg("circle", { cx: at.x, cy: at.y, r, class: "flow-node", style: { fill: Foxx.hashColor(node.id) } }), Foxx.svg("text", { x: at.x, y: at.y + 4, "text-anchor": "middle", class: "flow-label" }, node.id));
        chart.append(group);
      }
      Foxx.put(box, Foxx.h("div", { class: "center" }, chart));
    });
    return box;
  };

  // ---------------------------------------------------------------- histogram

  /** A histogram with its median and 95th percentile marked; click a bar to filter by its range */
  charts.histogram = function histogram(data, options) {
    if (!data.count) return empty("No values for this field in this range and filter.");
    const height = options.height || 220;
    const box = responsive((box, width) => {
      const left = 44, right = 10, top = 10, bottom = 34;
      const plotWidth = width - left - right, plotHeight = height - top - bottom;
      const bins = data.bins, step = plotWidth / bins.length;
      const max = niceMax(Math.max(...bins.map((bin) => bin.count)));
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const x = (value) => left + ((value - data.min) / Math.max(1e-9, data.max - data.min)) * plotWidth;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "histogram of " + data.field });
      const axis = Foxx.svg("g", { class: "axis" });
      for (let tick = 0; tick <= 4; tick += 1) {
        const value = (max / 4) * tick;
        axis.append(Foxx.svg("line", { class: "grid-line", x1: left, x2: width - right, y1: y(value), y2: y(value), style: { strokeDasharray: tick ? "2 3" : "" } }), Foxx.svg("text", { x: left - 6, y: y(value) + 3, "text-anchor": "end" }, Foxx.compact(value)));
      }
      const unit = options.format || Foxx.compact;
      for (let tick = 0; tick <= 4; tick += 1) {
        const value = data.min + ((data.max - data.min) * tick) / 4;
        axis.append(Foxx.svg("text", { x: left + (plotWidth * tick) / 4, y: height - 18, "text-anchor": tick === 0 ? "start" : tick === 4 ? "end" : "middle" }, unit(value)));
      }
      chart.append(axis);
      bins.forEach((bin, index) => {
        if (!bin.count) return;
        chart.append(Foxx.svg("rect", { class: "bar", x: left + index * step + 1, width: Math.max(1, step - 2), y: y(bin.count), height: top + plotHeight - y(bin.count), style: { fill: "#7b5fc7" },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(unit(bin.from) + " – " + unit(bin.to), [["count", bin.count]], options.onPick ? "Click: filter to this range" : "")),
          onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); if (options.onPick) options.onPick(bin); } }));
      });
      for (const [label, value, cls] of [["p50", data.p50, "marker"], ["p95", data.p95, "marker warn"]]) {
        const px = x(value);
        chart.append(Foxx.svg("line", { x1: px, x2: px, y1: top, y2: top + plotHeight, class: cls }), Foxx.svg("text", { x: px + 3, y: top + 10, class: "marker-label" }, label + " " + unit(value)));
      }
      Foxx.put(box, chart, Foxx.h("div", { class: "stats" }, ["count " + Foxx.compact(data.count), "min " + unit(data.min), "mean " + unit(data.mean), "p50 " + unit(data.p50), "p95 " + unit(data.p95), "max " + unit(data.max)].map((text) => Foxx.h("span", {}, text))));
    });
    return box;
  };

  // ---------------------------------------------------------------- an alert rule against its threshold

  /** An alert rule's measured value over time against its threshold, with each firing marked */
  charts.threshold = function threshold(points, options) {
    if (!points.length) return empty("This rule has not been evaluated yet.");
    const height = options.height || 180;
    const box = responsive((box, width) => {
      const left = 44, right = 10, top = 10, bottom = 30;
      const plotWidth = width - left - right, plotHeight = height - top - bottom;
      const start = points[0].at, end = Math.max(points[points.length - 1].at, start + 1);
      const values = points.map((point) => point.value).filter((value) => value !== null && value !== undefined);
      const max = niceMax(Math.max(options.threshold * 1.2, ...values, 1));
      const x = (t) => left + ((t - start) / (end - start)) * plotWidth;
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "rule value against its threshold" });
      const axis = Foxx.svg("g", { class: "axis" });
      for (let tick = 0; tick <= 4; tick += 1) axis.append(Foxx.svg("line", { class: "grid-line", x1: left, x2: width - right, y1: y((max / 4) * tick), y2: y((max / 4) * tick), style: { strokeDasharray: tick ? "2 3" : "" } }), Foxx.svg("text", { x: left - 6, y: y((max / 4) * tick) + 3, "text-anchor": "end" }, Foxx.compact((max / 4) * tick)));
      timeAxis(axis, start, end, left, plotWidth, height - 17, 6);
      chart.append(axis);
      chart.append(Foxx.svg("line", { x1: left, x2: width - right, y1: y(options.threshold), y2: y(options.threshold), class: "threshold" }), Foxx.svg("text", { x: width - right, y: y(options.threshold) - 4, "text-anchor": "end", class: "marker-label" }, "threshold " + options.symbol + " " + options.threshold));
      const known = points.filter((point) => point.value !== null && point.value !== undefined);
      chart.append(Foxx.svg("path", { d: known.map((point, index) => (index ? "L" : "M") + x(point.at).toFixed(1) + " " + y(point.value).toFixed(1)).join(" "), style: { fill: "none", stroke: "#3fa9f5", strokeWidth: "2" } }));
      for (const point of known) {
        const firing = point.transition === "FIRING" || point.transition === "REMINDER";
        chart.append(Foxx.svg("circle", { cx: x(point.at), cy: y(point.value), r: point.transition ? 5 : 2.5, class: firing ? "fire" : point.transition === "RESOLVED" ? "resolve" : "sample",
          onmousemove: (event) => Foxx.tooltip.show(event, tip(Foxx.dt(point.at), [["value", Foxx.fmt(point.value)], ["state", point.state], ["change", point.transition || "none"], ["events counted", point.events]])), onmouseleave: Foxx.tooltip.hide }));
      }
      Foxx.put(box, chart);
    });
    return box;
  };
})();
