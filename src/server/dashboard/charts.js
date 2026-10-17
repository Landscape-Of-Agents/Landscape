"use strict";
// The dashboard's chart library and shared helpers, drawn as SVG (the page loads no other script):
// events over time (bars, lines, areas, any aggregated metric) with drag-to-zoom, donut, top list,
// sparkline, Gantt chart, event swimlanes, topology, flow dependencies, histograms, a rule against
// its threshold, heatmaps (two attributes, or day by hour), a gauge, stacked bars, the waterfall
// of tool calls (time and tokens), a scatter plot, and a layered graph (flow call graphs). Every
// chart has a tooltip, a click that drills into what it shows, and redraws when its box changes.
// Colors: a categorical order led by the brand orange and validated for color-blind separation in
// both themes; status colors (good, warning, serious, critical) are reserved for states. Every
// time is written as YYYY:MM:DD:HH:MM:SS:MMM (local time).

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
  /** A time as YYYY:MM:DD:HH:MM:SS:MMM (local) */
  Foxx.dt = function dt(millis) {
    if (millis === null || millis === undefined || millis === "") return "—";
    const value = typeof millis === "string" && !/^-?\d+(?:\.\d+)?$/.test(millis) ? Date.parse(millis) : Number(millis);
    if (!Number.isFinite(value)) return "—";
    const date = new Date(value);
    if (Number.isNaN(date.getTime())) return "—";
    return pad(date.getFullYear(), 4) + ":" + pad(date.getMonth() + 1) + ":" + pad(date.getDate()) + ":" + pad(date.getHours()) + ":" + pad(date.getMinutes()) + ":" + pad(date.getSeconds()) + ":" + pad(date.getMilliseconds(), 3);
  };
  /** A time as [YYYY:MM:DD:, HH:MM:SS:MMM] for two-line axis labels */
  Foxx.dtParts = function dtParts(millis) {
    const text = Foxx.dt(millis);
    return [text.slice(0, 11), text.slice(11)];
  };
  /** A duration in words: 850 ms, 12.4 s, 3 min 5 s, 2 h 10 min */
  Foxx.duration = function duration(millis) {
    if (millis === null || millis === undefined || Number.isNaN(Number(millis))) return "—";
    const ms = Number(millis);
    if (ms < 1000) return Math.round(ms) + " ms";
    if (ms < 60000) return (ms / 1000).toFixed(1) + " s";
    if (ms < 3600000) return Math.floor(ms / 60000) + " min " + Math.round((ms % 60000) / 1000) + " s";
    if (ms < 86400000) return Math.floor(ms / 3600000) + " h " + Math.round((ms % 3600000) / 60000) + " min";
    return Math.floor(ms / 86400000) + " d " + Math.round((ms % 86400000) / 3600000) + " h";
  };
  Foxx.fmt = function fmt(value) {
    if (value === null || value === undefined) return "—";
    if (typeof value === "number") return Number.isInteger(value) ? value.toLocaleString() : value.toLocaleString(undefined, { maximumFractionDigits: 3 });
    if (Array.isArray(value)) return value.length ? value.map(Foxx.fmt).join(", ") : "none";
    if (typeof value === "object") return Object.entries(value).map(([key, item]) => key + ": " + Foxx.fmt(item)).join("  ");
    return String(value);
  };
  Foxx.compact = function compact(value) {
    if (value === null || value === undefined || Number.isNaN(Number(value))) return "—";
    if (typeof value !== "number") return Foxx.fmt(value);
    const abs = Math.abs(value);
    if (abs >= 1e9) return (value / 1e9).toFixed(1) + "B";
    if (abs >= 1e6) return (value / 1e6).toFixed(1) + "M";
    if (abs >= 1e4) return (value / 1e3).toFixed(1) + "k";
    if (Number.isInteger(value)) return value.toLocaleString();
    return abs >= 100 ? Math.round(value).toLocaleString() : value.toFixed(abs >= 10 ? 1 : 2);
  };
  /** Tokens in words: 1.2k tokens */
  Foxx.tokens = function tokens(value) {
    if (value === null || value === undefined || Number.isNaN(Number(value))) return "—";
    return Foxx.compact(Math.max(0, Math.round(Number(value)))) + " tok";
  };

  // ---------------------------------------------------------------- colors and tooltip

  /** Whether the page is drawn dark */
  Foxx.isDark = function isDark() {
    const theme = document.documentElement.dataset.theme;
    if (theme === "dark") return true;
    if (theme === "light") return false;
    return window.matchMedia && window.matchMedia("(prefers-color-scheme: dark)").matches;
  };
  // Categorical slots, in a fixed order (validated: adjacent color-blind separation ≥ 8 ΔE, normal
  // vision ≥ 15, both themes): brand orange, aqua, yellow, magenta, green, violet, red, blue
  const CATEGORICAL = {
    light: ["#eb6834", "#1baf7a", "#eda100", "#e87ba4", "#008300", "#6250d6", "#e34948", "#2a78d6"],
    dark: ["#d95926", "#199e70", "#c98500", "#d55181", "#008300", "#9085e9", "#e66767", "#3987e5"],
  };
  // Status colors are reserved for states (never a series): good, warning, serious, critical
  const STATUS = { good: "#0ca30c", warning: "#fab219", serious: "#ec835a", critical: "#d03b3b", neutral: "#898781", info: "#3987e5" };
  Foxx.STATUS = STATUS;
  Object.defineProperty(Foxx, "PALETTE", { get: () => CATEGORICAL[Foxx.isDark() ? "dark" : "light"] });
  Foxx.NAMED = {
    INFO: STATUS.info, LOW: "#5aa9a0", MEDIUM: STATUS.warning, HIGH: STATUS.serious, CRITICAL: STATUS.critical,
    ALLOW: STATUS.good, DENY: STATUS.critical, REQUIRE_APPROVAL: STATUS.warning, QUARANTINE: "#9c1c4c", ERROR: "#a33a3a", NOT_REQUIRED: STATUS.neutral,
    PREVENTED: STATUS.good, DETECTED_AFTER_EFFECT: STATUS.serious, BYPASS_CONFIRMED: "#9c1c4c", COVERAGE_GAP: STATUS.warning, NOT_APPLICABLE: STATUS.neutral,
    SATISFIED: STATUS.good, PARTIAL: STATUS.warning, NOT_SATISFIED: STATUS.critical, NO_EVIDENCE: STATUS.neutral, OK: STATUS.good, FAILED: STATUS.critical, SUCCESS: STATUS.good, FAILURE: STATUS.critical, BLOCKED: STATUS.critical,
    FIRING: STATUS.critical, RESOLVED: STATUS.good, DELIVERED: STATUS.good, QUEUED: STATUS.info, DRY_RUN: STATUS.neutral, PENDING: STATUS.warning, APPLIED: STATUS.good, REJECTED: STATUS.critical,
    STALE_DATA: STATUS.warning, OTHER: STATUS.neutral, NONE: STATUS.neutral, UNKNOWN: STATUS.neutral,
  };
  /** The color of a value: its status color when it names a state, else its categorical slot */
  Foxx.color = function color(key, index) { return Foxx.NAMED[key] || Foxx.PALETTE[Math.abs(index || 0) % 8]; };
  /** A stable categorical color for any text (the same entity keeps its color across charts) */
  Foxx.hashColor = function hashColor(text) {
    if (Foxx.NAMED[text]) return Foxx.NAMED[text];
    let hash = 0;
    for (const character of String(text)) hash = (hash * 31 + character.charCodeAt(0)) | 0;
    return Foxx.PALETTE[Math.abs(hash) % 8];
  };
  /** A sequential color (one hue, the brand orange, light to dark) for a share 0..1 */
  Foxx.ramp = function ramp(share) {
    const steps = Foxx.isDark() ? ["#3a2418", "#5c2f17", "#7f3a16", "#a54515", "#cc5418", "#ee6a2a", "#ff8a4c"] : ["#fdeee4", "#fbd5bd", "#f7b48d", "#f28f5c", "#e76a31", "#c94f1c", "#9a3a12"];
    if (!share || share <= 0) return "transparent";
    return steps[Math.min(steps.length - 1, Math.floor(Math.sqrt(Math.min(1, share)) * (steps.length - 0.001)))];
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
  const tip = (title, rows, hint) => Foxx.h("div", {}, Foxx.h("div", { class: "tip-title" }, title), (rows || []).filter((row) => row && row[1] !== undefined && row[1] !== null && row[1] !== "").map(([label, value, color]) => Foxx.h("div", { class: "row" }, color ? Foxx.h("i", { style: "background:" + color }) : null, Foxx.h("span", {}, label), Foxx.h("b", {}, value))), hint ? Foxx.h("div", { class: "tip-hint" }, hint) : null);
  Foxx.tip = tip;
  const hover = (content) => ({ onmousemove: (event) => Foxx.tooltip.show(event, typeof content === "function" ? content() : content), onmouseleave: Foxx.tooltip.hide });

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
  Foxx.responsive = responsive;
  /** The top of an axis: four steps of 1, 2, or 5 times a power of ten */
  function niceMax(max) {
    if (!(max > 0)) return 4;
    if (max <= 4 && Number.isInteger(max)) return 4;
    const raw = max / 4, power = Math.pow(10, Math.floor(Math.log10(raw)));
    for (const step of [1, 2, 2.5, 5, 10]) if (step * power >= raw) return step * power * 4;
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
  /** Horizontal gridlines with value labels */
  function valueAxis(group, max, top, height, left, right, format) {
    for (let tick = 0; tick <= 4; tick += 1) {
      const value = (max / 4) * tick, y = top + height - (value / max) * height;
      group.append(Foxx.svg("line", { class: tick ? "grid-line" : "base-line", x1: left, x2: right, y1: y, y2: y }), Foxx.svg("text", { x: left - 6, y: y + 3, "text-anchor": "end" }, (format || Foxx.compact)(value)));
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
      const moved = Math.abs(to - from), start = from;
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
  function legend(series, hidden, redraw, onPick, format) {
    return Foxx.h("div", { class: "legend" }, series.map((item, index) => Foxx.h("button", {
      type: "button", class: hidden.has(item.key) ? "off" : "", title: "Click to show or hide · double-click to open these events", "aria-pressed": String(!hidden.has(item.key)),
      onclick: () => { if (hidden.has(item.key)) hidden.delete(item.key); else hidden.add(item.key); redraw(); },
      ondblclick: () => onPick && onPick({ key: item.key }),
    }, Foxx.h("i", { style: "background:" + Foxx.color(item.key, index) }), item.label || item.key, Foxx.h("span", { class: "count" }, (format || Foxx.compact)(item.total)))));
  }
  const empty = (text) => Foxx.h("div", { class: "empty" }, text || "No events in this range and filter.");
  Foxx.empty = empty;

  const charts = (Foxx.charts = {});

  // ---------------------------------------------------------------- events over time

  /** Events (or any aggregated metric) over time: stacked bars, lines, or areas; hover a bucket for
   * its values, click it to open its events, drag across to zoom into a range */
  charts.timeseries = function timeseries(data, options) {
    const hidden = new Set();
    const height = options.height || 220;
    const format = options.format || Foxx.compact;
    const box = responsive((box, width) => {
      const series = data.series || [];
      if (!series.length || !data.total) { Foxx.put(box, empty(options.empty)); return; }
      const visible = series.filter((item) => !hidden.has(item.key));
      const count = data.buckets, left = 52, right = 10, top = 8, bottom = 30;
      const plotWidth = Math.max(40, width - left - right), plotHeight = height - top - bottom, step = plotWidth / count;
      const stacked = options.type === "bars" && (!data.agg || data.agg === "count" || data.agg === "sum");
      let max = 0;
      for (let index = 0; index < count; index += 1) {
        if (stacked) max = Math.max(max, visible.reduce((sum, item) => sum + (item.points[index] || 0), 0));
        else for (const item of visible) max = Math.max(max, item.points[index] || 0);
      }
      max = niceMax(max);
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const end = data.start + count * data.bucket_ms;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "over time", class: "zoomable" });
      const axis = Foxx.svg("g", { class: "axis" });
      valueAxis(axis, max, top, plotHeight, left, width - right, format);
      timeAxis(axis, data.start, end, left, plotWidth, height - 17);
      chart.append(axis);
      const describe = (index) => tip(Foxx.dt(data.start + index * data.bucket_ms) + " → " + Foxx.dt(data.start + (index + 1) * data.bucket_ms - 1),
        visible.filter((item) => item.points[index] !== null && item.points[index] !== undefined && (item.points[index] || !stacked)).map((item) => [item.label || item.key, format(item.points[index]), Foxx.color(item.key, series.indexOf(item))]), "Click: open these events · drag: zoom in");
      const pick = (index, key) => { if (!dragged(chart) && options.onPick) options.onPick({ since: data.start + index * data.bucket_ms, until: data.start + (index + 1) * data.bucket_ms - 1, key }); };
      if (stacked) {
        const gap = step > 6 ? 2 : step > 3 ? 1 : 0;
        for (let index = 0; index < count; index += 1) {
          let base = 0;
          const shown = visible.filter((item) => item.points[index]);
          shown.forEach((item, position) => {
            const value = item.points[index];
            const y1 = y(base + value), y0 = y(base);
            const isTop = position === shown.length - 1;
            const barHeight = Math.max(1, y0 - y1 - (isTop ? 0 : 2));
            chart.append(Foxx.svg(isTop && step > 6 ? "path" : "rect", Object.assign(isTop && step > 6 ? { d: roundedTop(left + index * step + gap / 2, y1, Math.max(1, step - gap), barHeight, Math.min(4, (step - gap) / 2, barHeight)) } : { x: left + index * step + gap / 2, width: Math.max(1, step - gap), y: y1, height: barHeight }, { class: "bar", style: { fill: Foxx.color(item.key, series.indexOf(item)) },
              onmousemove: (event) => Foxx.tooltip.show(event, describe(index)), onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); pick(index, item.key); } })));
            base += value;
          });
        }
      } else {
        for (const item of visible) {
          const color = Foxx.color(item.key, series.indexOf(item));
          // A line breaks where a bucket has no value (a metric without events there)
          let path = "", area = "", run = [];
          const flush = () => {
            if (!run.length) return;
            path += run.map(([px, py], index) => (index ? "L" : "M") + px.toFixed(1) + " " + py.toFixed(1)).join(" ") + " ";
            if (options.type === "area") area += "M" + run[0][0] + " " + y(0) + " " + run.map(([px, py]) => "L" + px.toFixed(1) + " " + py.toFixed(1)).join(" ") + " L" + run[run.length - 1][0] + " " + y(0) + " Z ";
            run = [];
          };
          item.points.forEach((value, index) => { if (value === null || value === undefined) flush(); else run.push([left + index * step + step / 2, y(value)]); });
          flush();
          if (area) chart.append(Foxx.svg("path", { d: area, style: { fill: color, opacity: "0.16" } }));
          chart.append(Foxx.svg("path", { d: path, class: "line", style: { stroke: color } }));
          if (count <= 40) item.points.forEach((value, index) => { if (value !== null && value !== undefined) chart.append(Foxx.svg("circle", { cx: left + index * step + step / 2, cy: y(value), r: 2.5, class: "point", style: { fill: color } })); });
        }
        const crosshair = Foxx.svg("line", { class: "crosshair", x1: 0, x2: 0, y1: top, y2: top + plotHeight, visibility: "hidden" });
        chart.append(crosshair);
        for (let index = 0; index < count; index += 1) {
          const cx = left + index * step + step / 2;
          chart.append(Foxx.svg("rect", { x: left + index * step, y: top, width: step, height: plotHeight, style: { fill: "transparent", cursor: "pointer" },
            onmousemove: (event) => { crosshair.setAttribute("x1", cx); crosshair.setAttribute("x2", cx); crosshair.setAttribute("visibility", "visible"); Foxx.tooltip.show(event, describe(index)); },
            onmouseleave: () => { crosshair.setAttribute("visibility", "hidden"); Foxx.tooltip.hide(); }, onclick: () => { Foxx.tooltip.hide(); pick(index); } }));
        }
      }
      brush(chart, { left, right: left + plotWidth, top, height: plotHeight, total: width }, (x) => data.start + ((x - left) / plotWidth) * (end - data.start), options.onZoom);
      Foxx.put(box, chart, series.length > 1 || options.legend ? legend(series, hidden, box.redraw, options.onPick, format) : null);
    });
    return box;
  };
  /** A rectangle with its top corners rounded (a bar's data end) */
  function roundedTop(x, y, width, height, radius) {
    const r = Math.max(0, radius);
    return "M" + x + " " + (y + height) + " L" + x + " " + (y + r) + " Q" + x + " " + y + " " + (x + r) + " " + y + " L" + (x + width - r) + " " + y + " Q" + (x + width) + " " + y + " " + (x + width) + " " + (y + r) + " L" + (x + width) + " " + (y + height) + " Z";
  }

  // ---------------------------------------------------------------- donut, top list, sparkline

  charts.donut = function donut(items, options) {
    const total = items.reduce((sum, item) => sum + item.count, 0);
    if (!total) return empty(options.empty);
    const chart = Foxx.svg("svg", { viewBox: "0 0 160 160", role: "img", "aria-label": options.title || "breakdown" });
    let angle = -Math.PI / 2;
    items.forEach((item, index) => {
      const sweep = (item.count / total) * Math.PI * 2;
      const end = angle + sweep, large = sweep > Math.PI ? 1 : 0;
      const point = (radius, theta) => (80 + radius * Math.cos(theta)).toFixed(2) + " " + (80 + radius * Math.sin(theta)).toFixed(2);
      const d = items.length === 1
        ? "M80 8 A72 72 0 1 1 79.99 8 L79.99 36 A44 44 0 1 0 80 36 Z"
        : "M" + point(72, angle) + " A72 72 0 " + large + " 1 " + point(72, end) + " L" + point(44, end) + " A44 44 0 " + large + " 0 " + point(44, angle) + " Z";
      chart.append(Foxx.svg("path", { class: "slice", d, style: { fill: Foxx.color(item.value, index) },
        onmousemove: (event) => Foxx.tooltip.show(event, tip(item.label || item.value, [[options.unit || "events", Foxx.fmt(item.count) + " (" + ((item.count / total) * 100).toFixed(1) + "%)"]], options.hint || "Click: open these events")),
        onmouseleave: Foxx.tooltip.hide, onclick: () => { Foxx.tooltip.hide(); if (options.onPick) options.onPick(item.value); } }));
      angle = end;
    });
    chart.append(Foxx.svg("text", { x: 80, y: 80, "text-anchor": "middle", class: "donut-total" }, Foxx.compact(total)), Foxx.svg("text", { x: 80, y: 97, "text-anchor": "middle", class: "donut-unit" }, options.unit || "events"));
    return Foxx.h("div", { class: "donut" }, chart, Foxx.h("div", { class: "legend vertical" }, items.map((item, index) => Foxx.h("button", { type: "button", onclick: () => options.onPick && options.onPick(item.value) },
      Foxx.h("i", { style: "background:" + Foxx.color(item.value, index) }), Foxx.h("span", { class: "legend-label" }, item.label || item.value), options.explain ? options.explain(item.value) : null, Foxx.h("span", { class: "count" }, Foxx.compact(item.count) + " · " + ((item.count / total) * 100).toFixed(0) + "%")))));
  };
  charts.toplist = function toplist(items, options) {
    options = options || {};
    if (!items.length) return empty(options.empty);
    const max = Math.max(...items.map((item) => item.count), 1e-9);
    const format = options.format || Foxx.compact;
    return Foxx.h("div", { class: "toplist" }, items.map((item, index) => Foxx.h("div", { class: "row", role: "button", tabindex: "0", title: options.hint || "Open these events", onclick: () => options.onPick && options.onPick(item.value), onkeydown: (event) => { if (event.key === "Enter" && options.onPick) options.onPick(item.value); } },
      Foxx.h("span", { class: "name", title: item.label || item.value }, item.label || item.value, options.explain ? options.explain(item.value) : null),
      Foxx.h("div", { class: "track" }, Foxx.h("div", { class: "fill", style: "width:" + Math.max(2, (item.count / max) * 100) + "%;background:" + (options.color || (options.byValue ? Foxx.hashColor(item.value) : Foxx.PALETTE[0])) })),
      Foxx.h("span", { class: "num" }, format(item.count)))));
  };
  charts.sparkline = function sparkline(points, color) {
    const list = (points || []).map((value) => value || 0);
    if (list.length < 2) return null;
    const max = Math.max(1e-9, ...list);
    const d = list.map((value, index) => (index ? "L" : "M") + ((index / (list.length - 1)) * 72).toFixed(1) + " " + (24 - (value / max) * 22).toFixed(1)).join(" ");
    return Foxx.svg("svg", { class: "spark", viewBox: "0 0 72 26", "aria-hidden": "true" }, Foxx.svg("path", { d: d + " L72 26 L0 26 Z", style: { fill: color || Foxx.PALETTE[0], opacity: "0.12" } }), Foxx.svg("path", { d, style: { fill: "none", stroke: color || Foxx.PALETTE[0], strokeWidth: "1.6" } }));
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
    if (!lanes.length) return empty(options.empty);
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

  /** Who used what: nodes in layers (members → agents → tools → resources by default), edge width by
   * events, red by the share denied; hover a node to follow its edges, click to pick it */
  charts.topology = function topology(data, options) {
    if (!data.nodes || !data.nodes.length) return empty(options.empty);
    const box = responsive((box, width) => {
      const layers = data.layers.map((_, layer) => data.nodes.filter((node) => node.layer === layer).sort((a, b) => b.count - a.count).slice(0, 14));
      const shown = new Set(layers.flat().map((node) => node.id));
      const nodeHeight = 28, gap = 10, top = 28;
      const height = top + Math.max(1, ...layers.map((layer) => layer.length)) * (nodeHeight + gap) + 10;
      const nodeWidth = Math.max(70, Math.min(180, (width - 40) / Math.max(1, layers.length) - 40));
      const column = (layer) => 10 + layer * ((width - nodeWidth - 20) / Math.max(1, layers.length - 1));
      const position = new Map();
      layers.forEach((list, layer) => list.forEach((node, index) => position.set(node.id, { x: column(layer), y: top + index * (nodeHeight + gap) })));
      const maxEdge = Math.max(1, ...data.edges.map((edge) => edge.count));
      const maxNode = Math.max(1, ...data.nodes.map((node) => node.count));
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "topology" });
      data.layers.forEach((name, layer) => chart.append(Foxx.svg("text", { x: column(layer), y: 14, class: "layer-title" }, name)));
      const edges = [];
      for (const edge of data.edges) {
        if (!shown.has(edge.from) || !shown.has(edge.to)) continue;
        const a = position.get(edge.from), b = position.get(edge.to);
        const x1 = a.x + nodeWidth, y1 = a.y + nodeHeight / 2, x2 = b.x, y2 = b.y + nodeHeight / 2, mid = (x1 + x2) / 2;
        const share = edge.denied / Math.max(1, edge.count);
        const path = Foxx.svg("path", Object.assign({ class: "edge", d: "M" + x1 + " " + y1 + " C" + mid + " " + y1 + " " + mid + " " + y2 + " " + x2 + " " + y2, style: { strokeWidth: String(1 + (edge.count / maxEdge) * 12), stroke: share >= 0.1 ? "rgba(208,59,59," + (0.3 + share * 0.7) + ")" : "" } },
          hover(() => tip(edge.from.split(":").slice(1).join(":") + " → " + edge.to.split(":").slice(1).join(":"), [["events", Foxx.fmt(edge.count)], ["denied", Foxx.fmt(edge.denied)], ["denied share", ((edge.denied / Math.max(1, edge.count)) * 100).toFixed(1) + "%"]]))));
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
          onmousemove: (event) => Foxx.tooltip.show(event, tip(node.label, [["layer", data.layers[node.layer]], ["events", Foxx.fmt(node.count)], ["denied", Foxx.fmt(node.denied)]], options.hint || "Click: details and filter")),
          onclick: () => options.onPick && options.onPick(node), onkeydown: (event) => { if (event.key === "Enter" && options.onPick) options.onPick(node); } });
        const warn = node.denied / Math.max(1, node.count) >= 0.1;
        group.append(Foxx.svg("rect", { width: nodeWidth, height: nodeHeight, rx: 6, class: warn ? "node-box warn" : "node-box" }), Foxx.svg("rect", { width: Math.max(3, (node.count / maxNode) * nodeWidth), height: 3, y: nodeHeight - 3, rx: 1.5, style: { fill: Foxx.hashColor(data.layers[node.layer]) } }), Foxx.svg("text", { x: 8, y: 18 }, node.label.length > 22 ? node.label.slice(0, 21) + "…" : node.label), Foxx.svg("text", { x: nodeWidth - 6, y: 18, "text-anchor": "end", class: "node-count" }, Foxx.compact(node.count)));
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
    if (!data.nodes || !data.nodes.length) return empty(options.empty);
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
      const marker = "arrow-" + Math.random().toString(36).slice(2, 8);
      chart.append(Foxx.svg("defs", {}, Foxx.svg("marker", { id: marker, viewBox: "0 0 10 10", refX: "10", refY: "5", markerWidth: "9", markerHeight: "9", markerUnits: "userSpaceOnUse", orient: "auto" }, Foxx.svg("path", { d: "M0 0 L10 5 L0 10 z", class: "arrow-head" }))));
      const lines = [];
      for (const edge of data.edges) {
        const a = place.get(edge.from), b = place.get(edge.to);
        if (!a || !b) continue;
        const dx = b.x - a.x, dy = b.y - a.y, length = Math.max(1, Math.hypot(dx, dy));
        const mx = (a.x + b.x) / 2 - (dy / length) * 34, my = (a.y + b.y) / 2 + (dx / length) * 34;
        const target = nodes.find((node) => node.id === edge.to);
        const r = 12 + ((target ? target.count : 0) / maxCount) * 16;
        const ex = b.x - ((b.x - mx) / Math.max(1, Math.hypot(b.x - mx, b.y - my))) * r, ey = b.y - ((b.y - my) / Math.max(1, Math.hypot(b.x - mx, b.y - my))) * r;
        const line = Foxx.svg("path", Object.assign({ class: "edge", d: "M" + a.x + " " + a.y + " Q" + mx + " " + my + " " + ex + " " + ey, "marker-end": "url(#" + marker + ")", style: { strokeWidth: String(1 + (edge.count / maxEdge) * 8) } }, hover(tip(edge.from + " → " + edge.to, [["times", Foxx.fmt(edge.count)]], "Order only, never a claimed cause"))));
        line.dataset.from = edge.from;
        line.dataset.to = edge.to;
        lines.push(line);
        chart.append(line);
      }
      for (const node of nodes) {
        const at = place.get(node.id);
        const r = 10 + (node.count / maxCount) * 16;
        const group = Foxx.svg("g", { class: "node", tabindex: "0",
          onmouseenter: () => lines.forEach((line) => line.classList.toggle("dim", line.dataset.from !== node.id && line.dataset.to !== node.id)),
          onmouseleave: () => { lines.forEach((line) => line.classList.remove("dim")); Foxx.tooltip.hide(); },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(node.id + (options.title_of ? " · " + options.title_of(node.id) : ""), [["events", Foxx.fmt(node.count)]], "Click: filter by this flow")),
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
    if (!data.count) return empty(options.empty || "No values for this field in this range and filter.");
    const height = options.height || 220;
    const box = responsive((box, width) => {
      const left = 52, right = 10, top = 14, bottom = 34;
      const plotWidth = Math.max(40, width - left - right), plotHeight = height - top - bottom;
      const bins = data.bins, step = plotWidth / bins.length;
      const max = niceMax(Math.max(...bins.map((bin) => bin.count)));
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const x = (value) => left + ((value - data.min) / Math.max(1e-9, data.max - data.min)) * plotWidth;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "histogram of " + data.field });
      const axis = Foxx.svg("g", { class: "axis" });
      valueAxis(axis, max, top, plotHeight, left, width - right);
      const unit = options.format || Foxx.compact;
      for (let tick = 0; tick <= 4; tick += 1) {
        const value = data.min + ((data.max - data.min) * tick) / 4;
        axis.append(Foxx.svg("text", { x: left + (plotWidth * tick) / 4, y: height - 18, "text-anchor": tick === 0 ? "start" : tick === 4 ? "end" : "middle" }, unit(value)));
      }
      chart.append(axis);
      bins.forEach((bin, index) => {
        if (!bin.count) return;
        const barHeight = top + plotHeight - y(bin.count);
        chart.append(Foxx.svg("path", Object.assign({ class: "bar", d: roundedTop(left + index * step + 1, y(bin.count), Math.max(1, step - 2), barHeight, Math.min(4, (step - 2) / 2, barHeight)), style: { fill: options.color || Foxx.PALETTE[0] }, onclick: () => { Foxx.tooltip.hide(); if (options.onPick) options.onPick(bin); } },
          hover(tip(unit(bin.from) + " – " + unit(bin.to), [["count", Foxx.fmt(bin.count)], ["share", ((bin.count / data.count) * 100).toFixed(1) + "%"]], options.onPick ? "Click: filter to this range" : "")))));
      });
      for (const [label, value, cls] of [["p50", data.p50, "marker"], ["p95", data.p95, "marker warn"]]) {
        const px = x(value);
        chart.append(Foxx.svg("line", { x1: px, x2: px, y1: top, y2: top + plotHeight, class: cls }), Foxx.svg("text", { x: Math.min(px + 3, width - 60), y: top - 3, class: "marker-label" }, label + " " + unit(value)));
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
      const left = 52, right = 10, top = 10, bottom = 30;
      const plotWidth = Math.max(40, width - left - right), plotHeight = height - top - bottom;
      const start = points[0].at, end = Math.max(points[points.length - 1].at, start + 1);
      const values = points.map((point) => point.value).filter((value) => value !== null && value !== undefined);
      const max = niceMax(Math.max(options.threshold * 1.2, ...values, 1));
      const x = (t) => left + ((t - start) / (end - start)) * plotWidth;
      const y = (value) => top + plotHeight - (value / max) * plotHeight;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "rule value against its threshold" });
      const axis = Foxx.svg("g", { class: "axis" });
      valueAxis(axis, max, top, plotHeight, left, width - right);
      timeAxis(axis, start, end, left, plotWidth, height - 17, 6);
      chart.append(axis);
      chart.append(Foxx.svg("line", { x1: left, x2: width - right, y1: y(options.threshold), y2: y(options.threshold), class: "threshold" }), Foxx.svg("text", { x: width - right, y: y(options.threshold) - 4, "text-anchor": "end", class: "marker-label" }, "threshold " + options.symbol + " " + options.threshold));
      const known = points.filter((point) => point.value !== null && point.value !== undefined);
      chart.append(Foxx.svg("path", { class: "line", d: known.map((point, index) => (index ? "L" : "M") + x(point.at).toFixed(1) + " " + y(point.value).toFixed(1)).join(" "), style: { stroke: Foxx.PALETTE[7] } }));
      for (const point of known) {
        const firing = point.transition === "FIRING" || point.transition === "REMINDER";
        chart.append(Foxx.svg("circle", Object.assign({ cx: x(point.at), cy: y(point.value), r: point.transition ? 5 : 2.5, class: firing ? "fire" : point.transition === "RESOLVED" ? "resolve" : "sample" }, hover(tip(Foxx.dt(point.at), [["value", Foxx.fmt(point.value)], ["state", point.state], ["change", point.transition || "none"], ["events counted", point.events]])))));
      }
      Foxx.put(box, chart);
    });
    return box;
  };

  // ---------------------------------------------------------------- heatmaps

  /** A heatmap of counts by two attributes: one hue, darker for more; hover a cell for its count and
   * denied share, click it to open those events */
  charts.heatmap = function heatmap(data, options) {
    options = options || {};
    const rows = data.rows || [], cols = data.cols || [];
    if (!rows.length || !cols.length || !data.max) return empty(options.empty);
    const label = options.label || ((item) => item.label || item.key);
    const rowWidth = Math.min(220, Math.max(70, ...rows.map((row) => String(label(row, "row")).length * 6.6 + 14)));
    const cell = 30, headHeight = Math.min(130, Math.max(40, ...cols.map((col) => String(label(col, "col")).length * 5.6 + 12)));
    const width = rowWidth + cols.length * cell + 64, height = headHeight + rows.length * cell + 8;
    const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, width, height, role: "img", "aria-label": options.title || "heatmap", class: "heatmap" });
    cols.forEach((col, c) => {
      const x = rowWidth + c * cell + cell / 2;
      const text = String(label(col, "col"));
      chart.append(Foxx.svg("text", { x, y: headHeight - 6, transform: "rotate(-45 " + x + " " + (headHeight - 6) + ")", class: "head-label" }, text.length > 20 ? text.slice(0, 19) + "…" : text, Foxx.svg("title", {}, text)));
    });
    rows.forEach((row, r) => {
      const y = headHeight + r * cell;
      const text = String(label(row, "row"));
      chart.append(Foxx.svg("text", { x: rowWidth - 8, y: y + cell / 2 + 4, "text-anchor": "end", class: "row-label" }, text.length > 30 ? text.slice(0, 29) + "…" : text, Foxx.svg("title", {}, text)));
      cols.forEach((col, c) => {
        const value = data.cells[r][c];
        const denied = data.denied ? data.denied[r][c] : 0;
        const share = value / data.max;
        const g = Foxx.svg("g", { class: "cell", tabindex: value ? "0" : null, onclick: () => { Foxx.tooltip.hide(); if (value && options.onPick) options.onPick(row.key, col.key); },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(label(row, "row") + " × " + label(col, "col"), [[options.unit || "events", Foxx.fmt(value)], data.denied ? ["denied", Foxx.fmt(denied) + (value ? " (" + ((denied / value) * 100).toFixed(0) + "%)" : "")] : null, ["share of the busiest cell", (share * 100).toFixed(0) + "%"]], value && options.onPick ? "Click: open these events" : "")),
          onmouseleave: Foxx.tooltip.hide });
        g.append(Foxx.svg("rect", { x: rowWidth + c * cell + 1, y: y + 1, width: cell - 2, height: cell - 2, rx: 4, class: value ? "cell-box" : "cell-box zero", style: value ? { fill: Foxx.ramp(share) } : {} }));
        if (value) g.append(Foxx.svg("text", { x: rowWidth + c * cell + cell / 2, y: y + cell / 2 + 4, "text-anchor": "middle", class: share > 0.45 ? "cell-text strong" : "cell-text" }, Foxx.compact(value)));
        if (denied && value) g.append(Foxx.svg("circle", { cx: rowWidth + c * cell + cell - 6, cy: y + 6, r: 2.5, class: "cell-denied" }));
        chart.append(g);
      });
      chart.append(Foxx.svg("text", { x: rowWidth + cols.length * cell + 8, y: y + cell / 2 + 4, class: "row-total" }, Foxx.compact(row.total)));
    });
    const scale = Foxx.h("div", { class: "scale" }, Foxx.h("span", {}, "0"), [0.05, 0.2, 0.4, 0.6, 0.8, 1].map((share) => Foxx.h("i", { style: "background:" + Foxx.ramp(share) })), Foxx.h("span", {}, Foxx.compact(data.max) + " " + (options.unit || "events")), data.denied ? Foxx.h("span", { class: "denied-key" }, Foxx.h("b", {}), "has denials") : null);
    return Foxx.h("div", { class: "chart heat" }, Foxx.h("div", { class: "scroll-x" }, chart), scale);
  };

  /** When events happen: day of the week by hour of the day, and a strip of every day */
  charts.calendar = function calendar(data, options) {
    options = options || {};
    if (!data.total) return empty(options.empty);
    const days = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const week = {
      rows: days.map((name, index) => ({ key: String(index), label: name, total: data.week[index].reduce((a, b) => a + b, 0) })),
      cols: Array.from({ length: 24 }, (_, hour) => ({ key: String(hour), label: pad(hour) + ":00", total: 0 })),
      cells: data.week, max: data.max_hour,
    };
    const strip = responsive((box, width) => {
      const list = data.days || [];
      if (!list.length) { Foxx.put(box); return; }
      const first = list[0].day, last = list[list.length - 1].day, span = Math.round((last - first) / 86400000) + 1;
      const step = Math.max(3, Math.min(26, (width - 8) / span));
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + Math.max(width, span * step) + " 44", height: 44, role: "img", "aria-label": "events per day" });
      for (const item of list) {
        const x = Math.round((item.day - first) / 86400000) * step;
        chart.append(Foxx.svg("rect", Object.assign({ x: x + 1, y: 2, width: Math.max(2, step - 2), height: 24, rx: 3, class: "cell-box", style: { fill: Foxx.ramp(item.count / Math.max(1, data.max_day)) }, onclick: () => options.onDay && options.onDay(item.day) }, hover(tip(Foxx.dt(item.day).slice(0, 10), [["events", Foxx.fmt(item.count)]], options.onDay ? "Click: zoom to this day" : "")))));
      }
      chart.append(Foxx.svg("text", { x: 0, y: 40, class: "axis-text" }, Foxx.dt(first).slice(0, 10)), Foxx.svg("text", { x: Math.max(width, span * step) - 2, y: 40, "text-anchor": "end", class: "axis-text" }, Foxx.dt(last).slice(0, 10)));
      Foxx.put(box, chart);
    });
    return Foxx.h("div", {}, charts.heatmap(week, { unit: "events", title: "events by day and hour", onPick: options.onPick }), Foxx.h("div", { class: "muted small" }, "Every day in range"), strip);
  };

  // ---------------------------------------------------------------- gauge and stacked bars

  /** A gauge: a share 0..1 on a half circle, with its label and a tone (good, warning, critical) */
  charts.gauge = function gauge(share, options) {
    options = options || {};
    if (share === null || share === undefined || Number.isNaN(share)) return empty(options.empty || "Nothing to measure yet.");
    const value = Math.max(0, Math.min(1, share));
    const arc = (from, to, r) => {
      const point = (t) => [100 + r * Math.cos(Math.PI * (1 - t)), 100 - r * Math.sin(Math.PI * (1 - t))];
      const [x1, y1] = point(from), [x2, y2] = point(to);
      return "M" + x1.toFixed(2) + " " + y1.toFixed(2) + " A" + r + " " + r + " 0 0 1 " + x2.toFixed(2) + " " + y2.toFixed(2);
    };
    const tone = options.tone || (value >= 0.8 ? STATUS.good : value >= 0.5 ? STATUS.warning : STATUS.critical);
    return Foxx.h("div", { class: "gauge" }, Foxx.svg("svg", { viewBox: "0 0 200 116", role: "img", "aria-label": (options.label || "") + " " + (value * 100).toFixed(0) + "%" },
      Foxx.svg("path", { d: arc(0, 1, 80), class: "gauge-track" }),
      value > 0 ? Foxx.svg("path", { d: arc(0, Math.max(0.005, value), 80), class: "gauge-value", style: { stroke: tone } }) : null,
      Foxx.svg("text", { x: 100, y: 92, "text-anchor": "middle", class: "gauge-number" }, options.text || (value * 100).toFixed(0) + "%"),
      Foxx.svg("text", { x: 100, y: 110, "text-anchor": "middle", class: "gauge-label" }, options.label || "")));
  };

  /** Horizontal stacked bars: a row per item, a segment per part (status colors for states); hover
   * a segment for its count, click it to pick it */
  charts.stacked = function stacked(rows, options) {
    options = options || {};
    if (!rows.length) return empty(options.empty);
    const keys = options.keys || [...new Set(rows.flatMap((row) => Object.keys(row.parts)))];
    const max = Math.max(1, ...rows.map((row) => keys.reduce((sum, key) => sum + (row.parts[key] || 0), 0)));
    return Foxx.h("div", { class: "stacked" }, rows.map((row) => {
      const total = keys.reduce((sum, key) => sum + (row.parts[key] || 0), 0);
      return Foxx.h("div", { class: "stacked-row" }, Foxx.h("span", { class: "name", title: row.label }, row.label),
        Foxx.h("div", { class: "stacked-track" }, keys.filter((key) => row.parts[key]).map((key, index) => Foxx.h("div", Object.assign({ class: "segment", style: "width:" + ((row.parts[key] / max) * 100) + "%;background:" + Foxx.color(key, keys.indexOf(key)), role: options.onPick ? "button" : null, tabindex: options.onPick ? "0" : null, onclick: () => options.onPick && options.onPick(row, key) }, hover(tip(row.label + " · " + key, [[options.unit || "count", Foxx.fmt(row.parts[key])], ["share of the row", ((row.parts[key] / Math.max(1, total)) * 100).toFixed(0) + "%"]])))))),
        Foxx.h("span", { class: "num" }, Foxx.compact(total)));
    }), Foxx.h("div", { class: "legend" }, keys.map((key, index) => Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { style: "background:" + Foxx.color(key, index) }), key))));
  };

  // ---------------------------------------------------------------- the waterfall of tool calls

  /** The waterfall of tool calls: a row per call, a bar from its start (relative to the first) for
   * the time it took, colored by its decision, with the tokens of its model turn beside it; hover
   * for every metric, click to open the call */
  charts.waterfall = function waterfall(calls, options) {
    options = options || {};
    if (!calls.length) return empty(options.empty || "No tool calls in this range and filter.");
    const list = calls.slice().sort((a, b) => a.start - b.start);
    const length = (call) => Math.max(call.tool_ms || 0, call.duration_ms || 0, 1);
    // Packed: each call starts where the one before it ended, so idle time does not hide durations
    const offsets = new Map();
    let cursor = 0;
    for (const call of list) { offsets.set(call, cursor); cursor += length(call); }
    const at = (call) => (options.packed ? list[0].start + offsets.get(call) : call.start);
    const start = list[0].start, end = options.packed ? start + Math.max(1, cursor) : Math.max(...list.map((call) => call.start + length(call)), start + 1);
    const maxTokens = Math.max(1, ...list.map((call) => (call.tokens && call.tokens.total) || 0));
    const rowHeight = 22, top = 24, labelWidth = 210, tokenWidth = 120;
    const box = responsive((box, width) => {
      const plotWidth = Math.max(80, width - labelWidth - tokenWidth - 24);
      const height = top + list.length * rowHeight + 6;
      const x = (t) => labelWidth + ((t - start) / (end - start)) * plotWidth;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": "waterfall of tool calls" });
      const axis = Foxx.svg("g", { class: "axis" });
      for (let tick = 0; tick <= 4; tick += 1) {
        const t = start + ((end - start) * tick) / 4, px = x(t);
        axis.append(Foxx.svg("line", { class: "grid-line", x1: px, x2: px, y1: top - 4, y2: height }), Foxx.svg("text", { x: px, y: 12, "text-anchor": tick === 0 ? "start" : tick === 4 ? "end" : "middle" }, (options.packed ? "Σ " : "+") + Foxx.duration(t - start)));
      }
      axis.append(Foxx.svg("text", { x: labelWidth + plotWidth + 16, y: 12 }, "tokens"));
      chart.append(axis);
      list.forEach((call, index) => {
        const y = top + index * rowHeight;
        const color = Foxx.color(call.decision && call.decision !== "NOT_REQUIRED" ? call.decision : call.outcome === "FAILED" ? "FAILED" : "ALLOW", 0);
        const tool = call.tool_ms || Math.max(call.duration_ms || 0, 1);
        const rows = [["tool", call.tool], ["started", Foxx.dt(call.start)], ["offset", "+" + Foxx.duration(call.start - start)], ["tool time", call.tool_ms !== null && call.tool_ms !== undefined ? Foxx.duration(call.tool_ms) : "not measured"], ["trace span", Foxx.duration(call.duration_ms)], ["hook latency", call.latency_ms !== null && call.latency_ms !== undefined ? Foxx.duration(call.latency_ms) : null], ["decision", call.decision, Foxx.color(call.decision, 0)], ["outcome", call.outcome], ["tokens", call.tokens ? Foxx.fmt(Math.round(call.tokens.total)) : "not reported"], ["input / output", call.tokens ? Foxx.compact(call.tokens.input) + " / " + Foxx.compact(call.tokens.output) : null], ["cache read", call.tokens ? Foxx.compact(call.tokens.cache_read) : null], ["model", call.model], ["session", call.session], ["targets", (call.targets || []).join(", ")]];
        const g = Foxx.svg("g", Object.assign({ class: "wf", tabindex: "0", onclick: () => { Foxx.tooltip.hide(); if (options.onPick) options.onPick(call); }, onkeydown: (event) => { if (event.key === "Enter" && options.onPick) options.onPick(call); } }, hover(() => tip((call.tool || call.kind) + " · " + call.id, rows, "Click: open the call"))));
        g.append(Foxx.svg("rect", { x: 0, y, width, height: rowHeight, class: index % 2 ? "stripe" : "stripe odd" }));
        g.append(Foxx.svg("text", { x: 6, y: y + 15, class: "row-label" }, String(call.tool || call.kind).slice(0, 14), Foxx.svg("tspan", { class: "muted-text", dx: 6 }, Foxx.dt(call.start).slice(11))));
        const bx = x(at(call)), bw = Math.max(3, x(at(call) + tool) - bx);
        g.append(Foxx.svg("rect", { x: bx, y: y + 5, width: bw, height: rowHeight - 10, rx: 3, style: { fill: color }, class: call.outcome === "FAILED" ? "wf-bar failed" : "wf-bar" }));
        g.append(Foxx.svg("text", { x: Math.min(bx + bw + 4, labelWidth + plotWidth - 4), y: y + 15, class: "wf-time", "text-anchor": bx + bw + 60 > labelWidth + plotWidth ? "end" : "start" }, call.tool_ms !== null && call.tool_ms !== undefined ? Foxx.duration(call.tool_ms) : ""));
        if (call.tokens && call.tokens.total) {
          const tw = Math.max(2, (call.tokens.total / maxTokens) * (tokenWidth - 44));
          g.append(Foxx.svg("rect", { x: labelWidth + plotWidth + 16, y: y + 6, width: tw, height: rowHeight - 12, rx: 2, style: { fill: Foxx.PALETTE[5] } }), Foxx.svg("text", { x: labelWidth + plotWidth + 20 + tw, y: y + 15, class: "wf-time" }, Foxx.compact(Math.round(call.tokens.total))));
        } else g.append(Foxx.svg("text", { x: labelWidth + plotWidth + 16, y: y + 15, class: "wf-time" }, "—"));
        chart.append(g);
      });
      Foxx.put(box, Foxx.h("div", { class: "scroll-y" }, chart), Foxx.h("div", { class: "legend" }, ["ALLOW", "REQUIRE_APPROVAL", "DENY", "FAILED"].map((key) => Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { style: "background:" + Foxx.color(key, 0) }), key === "ALLOW" ? "allowed / verified" : key.toLowerCase().replace("_", " "))), Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { style: "background:" + Foxx.PALETTE[5] }), "tokens of the call's model turn (its share)")));
    }, "waterfall");
    return box;
  };

  // ---------------------------------------------------------------- scatter

  /** A scatter plot: one dot per item at (x, y), colored by a key; hover for its values, click to
   * open it. Dots have a 24px hit area */
  charts.scatter = function scatter(points, options) {
    options = options || {};
    const list = points.filter((point) => Number.isFinite(point.x) && Number.isFinite(point.y));
    if (!list.length) return empty(options.empty);
    const height = options.height || 260;
    const xf = options.xFormat || Foxx.compact, yf = options.yFormat || Foxx.compact;
    const box = responsive((box, width) => {
      const left = 56, right = 14, top = 10, bottom = 40;
      const plotWidth = Math.max(40, width - left - right), plotHeight = height - top - bottom;
      const xmax = niceMax(Math.max(...list.map((point) => point.x))), ymax = niceMax(Math.max(...list.map((point) => point.y)));
      const x = (value) => left + (value / xmax) * plotWidth, y = (value) => top + plotHeight - (value / ymax) * plotHeight;
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "scatter" });
      const axis = Foxx.svg("g", { class: "axis" });
      valueAxis(axis, ymax, top, plotHeight, left, width - right, yf);
      for (let tick = 0; tick <= 4; tick += 1) axis.append(Foxx.svg("text", { x: left + (plotWidth * tick) / 4, y: height - 22, "text-anchor": tick === 0 ? "start" : tick === 4 ? "end" : "middle" }, xf((xmax / 4) * tick)));
      axis.append(Foxx.svg("text", { x: left + plotWidth / 2, y: height - 4, "text-anchor": "middle", class: "axis-title" }, options.xLabel || "x"), Foxx.svg("text", { x: 12, y: top + plotHeight / 2, transform: "rotate(-90 12 " + (top + plotHeight / 2) + ")", "text-anchor": "middle", class: "axis-title" }, options.yLabel || "y"));
      chart.append(axis);
      const keys = [...new Set(list.map((point) => point.key))].slice(0, 8);
      for (const point of list) {
        const color = point.color || Foxx.color(point.key, Math.max(0, keys.indexOf(point.key)));
        const g = Foxx.svg("g", Object.assign({ class: "scatter-point", onclick: () => { Foxx.tooltip.hide(); if (options.onPick) options.onPick(point); } }, hover(() => tip(point.label, [[options.xLabel || "x", xf(point.x)], [options.yLabel || "y", yf(point.y)], [options.keyLabel || "group", point.key, color]].concat(point.rows || []), options.onPick ? "Click: open it" : ""))));
        g.append(Foxx.svg("circle", { cx: x(point.x), cy: y(point.y), r: 12, class: "hit" }), Foxx.svg("circle", { cx: x(point.x), cy: y(point.y), r: point.r || 4.5, class: "dot", style: { fill: color } }));
        chart.append(g);
      }
      Foxx.put(box, chart, keys.length > 1 ? Foxx.h("div", { class: "legend" }, keys.map((key, index) => Foxx.h("span", { class: "legend-item" }, Foxx.h("i", { style: "background:" + Foxx.color(key, index) }), key))) : null);
    });
    return box;
  };

  // ---------------------------------------------------------------- layered graph

  /** A layered graph (a flow's call graph): nodes in columns by depth from the entry points, an edge
   * per call; hover a node to follow its calls, click it for details */
  charts.layered = function layered(graph, options) {
    options = options || {};
    const nodes = graph.nodes || [];
    if (!nodes.length) return empty(options.empty || "No call graph.");
    const box = responsive((box, width) => {
      const depthOf = (node) => node.depth || 0;
      const depths = Math.max(...nodes.map(depthOf)) + 1;
      const columns = Array.from({ length: depths }, (_, depth) => nodes.filter((node) => depthOf(node) === depth).slice(0, 18));
      const nodeHeight = 26, gap = 8, top = 24;
      const nodeWidth = Math.max(90, Math.min(220, (width - 30) / depths - 30));
      const height = top + Math.max(...columns.map((column) => column.length)) * (nodeHeight + gap) + 8;
      const step = depths > 1 ? (width - nodeWidth - 20) / (depths - 1) : 0;
      const place = new Map();
      columns.forEach((column, depth) => column.forEach((node, index) => place.set(node.id, { x: 10 + depth * step, y: top + index * (nodeHeight + gap) })));
      const chart = Foxx.svg("svg", { viewBox: "0 0 " + width + " " + height, height, role: "img", "aria-label": options.title || "call graph" });
      columns.forEach((column, depth) => chart.append(Foxx.svg("text", { x: 10 + depth * step, y: 14, class: "layer-title" }, depth === 0 ? "entry" : "depth " + depth)));
      const lines = [];
      for (const [from, to, how] of graph.edges || []) {
        const a = place.get(from), b = place.get(to);
        if (!a || !b) continue;
        const x1 = a.x + nodeWidth, y1 = a.y + nodeHeight / 2, x2 = b.x, y2 = b.y + nodeHeight / 2, mid = (x1 + x2) / 2;
        const line = Foxx.svg("path", Object.assign({ class: "edge", d: x2 > x1 ? "M" + x1 + " " + y1 + " C" + mid + " " + y1 + " " + mid + " " + y2 + " " + x2 + " " + y2 : "M" + (a.x + nodeWidth / 2) + " " + (a.y + nodeHeight) + " Q" + (a.x - 30) + " " + ((y1 + y2) / 2) + " " + (b.x + nodeWidth / 2) + " " + b.y, style: { strokeWidth: "1.5" } }, hover(tip(from + " → " + to, [["resolved by", how]]))));
        line.dataset.from = from;
        line.dataset.to = to;
        lines.push(line);
        chart.append(line);
      }
      for (const [id, at] of place) {
        const node = nodes.find((item) => item.id === id);
        const name = (node.definition && (node.definition.qualified || node.definition.name)) || id.split(":").pop();
        const entry = (graph.entry_points || []).includes(id);
        const g = Foxx.svg("g", { class: "node", transform: "translate(" + at.x + "," + at.y + ")", tabindex: "0",
          onmouseenter: () => lines.forEach((line) => line.classList.toggle("dim", line.dataset.from !== id && line.dataset.to !== id)),
          onmouseleave: () => { lines.forEach((line) => line.classList.remove("dim")); Foxx.tooltip.hide(); },
          onmousemove: (event) => Foxx.tooltip.show(event, tip(name, [["file", id.split(":")[0]], ["depth", depthOf(node)], ["entry point", entry ? "yes" : "no"]])),
          onclick: () => options.onPick && options.onPick(node) });
        g.append(Foxx.svg("rect", { width: nodeWidth, height: nodeHeight, rx: 6, class: entry ? "node-box entry" : "node-box" }), Foxx.svg("text", { x: 8, y: 17 }, name.length > 28 ? name.slice(0, 27) + "…" : name));
        chart.append(g);
      }
      Foxx.put(box, Foxx.h("div", { class: "scroll-y" }, chart));
    });
    return box;
  };
})();
