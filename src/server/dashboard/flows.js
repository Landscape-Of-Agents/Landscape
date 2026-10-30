"use strict";
// The DataSea Flows section: the repository's flows and zones as one map you can pan and zoom, laid
// out in columns (flows, then each stage of zones, then zones in no flow) with each flow's path drawn
// left to right. Nodes show a name, a priority stripe, a red dot for incidents, and an agent badge
// when a subagent or an assigned agent monitors them; selecting one fills it with the accent, tints
// its path, fades the rest, and zooms to them. The inspector under the map holds the selection,
// sorted by filter chips (overview, zones, context, tests, agents, trace, people, ...): clicking an
// item opens it in the smaller detail pane beside the list. A flow's trace reads from its first call
// to its last as a call stack (names, left) beside a waterfall (right). People show whether Slack,
// Jira, and GitHub are connected. The Subagents section shows each Monitor or Incident subagent as a
// small tile; its info button opens its properties, actions, parameters, context, Crane's
// governance, and a JSON configuration that is merged and checked against the flow's bounds before
// it is requested (Landscape approves it). Every change is a request a person applies on a machine
// ('crane brick inbox apply', or one click in Landscape), where Crane checks it and, for signed
// knowledge, signs it.
// Data: GET /api/flows (the last status a member's machine reported); changes: POST /api/flows.

window.FOXX_PAGES = window.FOXX_PAGES || {};

(function () {
  const CSS = `
.fx { display: grid; gap: 14px; }
.fx-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 14px; }
.fx-title { font-size: 18px; font-weight: 750; letter-spacing: -.01em; margin: 0; }
.fx-meta { color: var(--muted); font-size: 12px; }
.fx-stats { display: flex; flex-wrap: wrap; gap: 6px; }
.fx-stat { font: 600 11.5px/1 var(--mono); padding: 5px 8px; border: 1px solid var(--line); border-radius: 999px; background: var(--panel); color: var(--ink-2); white-space: nowrap; }
.fx-stat b { color: var(--ink); }
.fx-stat.alert { border-color: color-mix(in srgb, var(--bad) 45%, var(--line)); color: var(--bad); }
.fx-spacer { flex: 1; }
.fx-btn { font: 600 12.5px/1 inherit; padding: 8px 12px; border-radius: 8px; border: 1px solid var(--line-2); background: var(--panel); color: var(--ink); cursor: pointer; }
.fx-btn:hover { background: var(--hover); }
.fx-btn.primary { background: var(--accent); border-color: var(--accent); color: var(--accent-ink); }
.fx-btn.ghost { border-color: transparent; background: transparent; color: var(--ink-2); }
.fx-btn.danger { color: var(--bad); }
.fx-btn.small { padding: 5px 9px; font-size: 12px; }
.fx-btn:disabled { opacity: .5; cursor: not-allowed; }
.fx-btn:focus-visible, .fx-node:focus-visible, .fx-chip:focus-visible, .fx-item:focus-visible, .fx-tool:focus-visible, .fx-bar:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }

/* The map: fills the width and most of the screen; no scrollbars, drag to pan, wheel to zoom */
.fx-canvas { position: relative; height: clamp(460px, 68vh, 920px); border: 1px solid var(--line); border-radius: 14px; overflow: hidden; background:
  radial-gradient(circle at 1px 1px, color-mix(in srgb, var(--axis) 55%, transparent) 1px, transparent 1.2px) 0 0 / 18px 18px, var(--panel-2); }
.fx-canvas svg { display: block; width: 100%; height: 100%; touch-action: none; cursor: grab; user-select: none; }
.fx-canvas.dragging svg { cursor: grabbing; }
.fx-viewport { transform-origin: 0 0; }
.fx-viewport.glide { transition: transform .28s ease; }
.fx-tools { position: absolute; top: 10px; right: 10px; display: flex; flex-direction: column; gap: 2px; padding: 3px; border: 1px solid var(--line); border-radius: 10px; background: color-mix(in srgb, var(--panel) 92%, transparent); box-shadow: var(--shadow); }
.fx-tool { display: grid; place-items: center; width: 30px; height: 30px; border: 0; border-radius: 7px; background: none; color: var(--ink-2); cursor: pointer; padding: 0; }
.fx-tool:hover { background: var(--hover); color: var(--ink); }
.fx-tool:disabled { opacity: .4; cursor: default; background: none; }
.fx-tool svg { width: 16px; height: 16px; }
.fx-tools hr { border: 0; border-top: 1px solid var(--line); margin: 2px 4px; }
.fx-zoom { font: 600 10px/1 var(--mono); color: var(--muted); text-align: center; padding: 3px 0 2px; }
.fx-legend { position: absolute; left: 10px; bottom: 10px; display: flex; flex-wrap: wrap; gap: 10px; padding: 6px 10px; border: 1px solid var(--line); border-radius: 9px; background: color-mix(in srgb, var(--panel) 92%, transparent); font-size: 11px; color: var(--muted); max-width: calc(100% - 80px); }
.fx-legend i { display: inline-block; width: 8px; height: 8px; border-radius: 2px; margin-right: 5px; vertical-align: 0; }
.fx-hint { position: absolute; right: 12px; bottom: 10px; font-size: 11px; color: var(--muted); pointer-events: none; }
.fx-band { font: 700 10px/1 var(--mono); letter-spacing: .12em; text-transform: uppercase; fill: var(--muted); }
.fx-node { cursor: pointer; outline: none; transition: opacity .15s; }
.fx-node rect.card { fill: var(--panel); stroke: var(--line-2); stroke-width: 1.2; filter: drop-shadow(0 1px 2px rgba(40, 20, 10, .10)); transition: stroke .12s, fill .12s; }
.fx-node:hover rect.card, .fx-node:focus-visible rect.card { stroke: var(--accent-2); stroke-width: 1.8; }
.fx-node text.name { fill: var(--ink); font-size: 12.5px; font-weight: 600; }
.fx-node text.glyph { fill: var(--muted); font-size: 12px; }
.fx-node.zone text.glyph { fill: var(--low); }
/* Selected: filled with the accent; along its path: tinted; everything else steps back */
.fx-node.selected rect.card { fill: var(--accent); stroke: var(--ember); stroke-width: 2; filter: drop-shadow(0 3px 8px color-mix(in srgb, var(--accent) 45%, transparent)); }
.fx-node.selected text.name, .fx-node.selected text.glyph { fill: var(--accent-ink); }
.fx-node.selected rect.stripe { fill: var(--accent-ink); opacity: .85; }
.fx-node.related rect.card { fill: var(--accent-soft); stroke: var(--accent); stroke-width: 1.8; }
.fx-node.related text.name { fill: var(--ember); }
.fx-node.dim { opacity: .3; }
.fx-node.draft rect.card { stroke-dasharray: 4 3; }
.fx-edge { fill: none; stroke: color-mix(in srgb, var(--axis) 80%, var(--muted)); stroke-width: 1.5; opacity: .75; transition: opacity .15s, stroke .15s; }
.fx-edge.lit { stroke: var(--accent); stroke-width: 2.6; opacity: 1; }
.fx-edge.faded { opacity: .12; }
.fx-empty { position: absolute; inset: 0; display: grid; place-items: center; padding: 24px; text-align: center; color: var(--muted); }

/* The inspector: filter chips, the list or view of the chosen filter, and the smaller detail pane */
.fx-inspect { border: 1px solid var(--line); border-radius: 14px; background: var(--panel); box-shadow: var(--shadow); overflow: hidden; }
.fx-inspect-head { display: flex; flex-wrap: wrap; align-items: center; gap: 6px 10px; padding: 12px 16px 0; }
.fx-kind { font: 700 10px/1 var(--mono); letter-spacing: .12em; text-transform: uppercase; color: var(--accent); }
.fx-inspect-title { font-size: 16px; font-weight: 750; margin: 0; }
.fx-chips { display: flex; flex-wrap: wrap; gap: 6px; padding: 10px 16px 12px; border-bottom: 1px solid var(--line); }
.fx-chip { font: 600 12px/1 inherit; padding: 6px 10px; border-radius: 999px; border: 1px solid var(--line-2); background: var(--panel); color: var(--ink-2); cursor: pointer; white-space: nowrap; }
.fx-chip:hover { border-color: var(--accent-2); }
.fx-chip[aria-pressed="true"] { background: var(--accent); border-color: var(--accent); color: var(--accent-ink); }
.fx-chip .n { font: 600 10.5px/1 var(--mono); margin-left: 5px; opacity: .8; }
.fx-inspect-body { display: grid; grid-template-columns: minmax(0, 1fr) minmax(260px, 340px); }
.fx-pane { padding: 14px 16px 18px; display: grid; gap: 14px; align-content: start; min-width: 0; }
.fx-peek { border-left: 1px solid var(--line); background: var(--panel-2); padding: 14px; display: grid; gap: 10px; align-content: start; min-width: 0; }
.fx-peek-head { display: grid; gap: 4px; }
.fx-peek-title { font-size: 14px; font-weight: 700; margin: 0; overflow-wrap: anywhere; }
.fx-peek .fx-field { padding: 6px 0; border-top: 1px solid var(--line); }
.fx-peek .fx-field:first-of-type { border-top: 0; }
.fx-items { display: grid; gap: 6px; }
.fx-item { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 2px 10px; align-items: center; text-align: left; padding: 9px 11px; border: 1px solid var(--line); border-radius: 9px; background: var(--panel); color: var(--ink); cursor: pointer; font: inherit; width: 100%; }
.fx-item:hover { border-color: var(--accent-2); }
.fx-item.on { border-color: var(--accent); background: var(--accent-soft); }
.fx-item .t { font-weight: 650; font-size: 13px; overflow-wrap: anywhere; }
.fx-item .m { grid-column: 1; color: var(--muted); font-size: 11.5px; overflow-wrap: anywhere; }
.fx-item .r { grid-row: 1 / span 2; grid-column: 2; display: flex; gap: 4px; align-items: center; }
.fx-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(170px, 1fr)); gap: 10px; }
.fx-field { display: grid; gap: 4px; min-width: 0; }
.fx-label { font: 700 10px/1 var(--mono); letter-spacing: .1em; text-transform: uppercase; color: var(--muted); }
.fx-value { font-size: 13px; color: var(--ink); overflow-wrap: anywhere; }
.fx-value code, .fx-code { font: 12px/1.5 var(--mono); }
.fx-code { display: block; padding: 10px 12px; border-radius: 9px; background: var(--panel-2); border: 1px solid var(--line); white-space: pre-wrap; overflow-wrap: anywhere; max-height: 300px; overflow: auto; color: var(--ink); }
.fx-peek .fx-code { background: var(--panel); }
.fx-pill { display: inline-flex; align-items: center; gap: 5px; font: 650 10.5px/1 var(--mono); padding: 4px 7px; border-radius: 999px; border: 1px solid var(--line-2); color: var(--ink-2); background: var(--panel-2); white-space: nowrap; }
.fx-pill.p0 { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 40%, var(--line)); }
.fx-pill.p1 { color: var(--warn); border-color: color-mix(in srgb, var(--warn) 40%, var(--line)); }
.fx-pill.ok { color: var(--ok); border-color: color-mix(in srgb, var(--ok) 40%, var(--line)); }
.fx-pill.bad { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 40%, var(--line)); }
.fx-pill.accent { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 45%, var(--line)); }
.fx-section { display: grid; gap: 10px; }
.fx-section > h3 { margin: 0; font-size: 13px; font-weight: 700; display: flex; align-items: center; gap: 10px; }
.fx-note { font-size: 12.5px; color: var(--muted); margin: 0; }
.fx-acts { display: flex; flex-wrap: wrap; gap: 6px; }
.fx-meter { height: 6px; border-radius: 999px; background: var(--panel-2); border: 1px solid var(--line); overflow: hidden; }
.fx-meter i { display: block; height: 100%; background: var(--accent); }

/* The trace: a stack (one row per call depth) or a waterfall (one row per call, in order) */
.fx-trace-bar { display: flex; flex-wrap: wrap; align-items: center; gap: 8px; }
.fx-seg { display: inline-flex; border: 1px solid var(--line-2); border-radius: 8px; overflow: hidden; }
.fx-seg button { font: 600 12px/1 inherit; padding: 6px 10px; border: 0; background: var(--panel); color: var(--muted); cursor: pointer; }
.fx-seg button + button { border-left: 1px solid var(--line); }
.fx-seg button[aria-pressed="true"] { background: var(--accent); color: var(--accent-ink); }
.fx-stack { position: relative; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); overflow: hidden; }
.fx-waterfall { display: grid; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); max-height: 440px; overflow: auto; }
.fx-wf-row { display: grid; grid-template-columns: minmax(160px, 38%) minmax(0, 1fr); align-items: center; min-height: 24px; border-bottom: 1px solid color-mix(in srgb, var(--line) 60%, transparent); cursor: pointer; }
.fx-wf-row:hover, .fx-wf-row.on { background: var(--accent-soft); }
.fx-wf-name { font: 11.5px/1.2 var(--mono); padding: 4px 8px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; color: var(--ink); }
.fx-wf-name .again { color: var(--muted); }
.fx-wf-track { position: relative; height: 24px; margin-right: 8px; }
.fx-bar { position: absolute; top: 4px; height: 16px; border-radius: 4px; min-width: 3px; overflow: hidden; font: 600 10.5px/16px var(--mono); padding: 0 5px; white-space: nowrap; text-overflow: ellipsis; color: var(--ink); cursor: pointer; border: 1px solid color-mix(in srgb, var(--ink) 12%, transparent); }
.fx-stack .fx-bar { height: 22px; line-height: 22px; top: auto; }
.fx-bar.on { outline: 2px solid var(--accent); outline-offset: 0; }
.fx-bar.added { box-shadow: inset 0 -3px 0 var(--ok); }
.fx-axis { display: flex; justify-content: space-between; font: 600 10px/1 var(--mono); color: var(--muted); padding: 0 2px; }
.fx-trace { border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); overflow: hidden; }
.fx-trace-head { display: grid; grid-template-columns: minmax(200px, 40%) minmax(0, 1fr); align-items: end; border-bottom: 1px solid var(--line); background: var(--panel); font: 700 10px/1 var(--mono); letter-spacing: .1em; text-transform: uppercase; color: var(--muted); }
.fx-trace-head > span { padding: 8px 10px; border-right: 1px solid var(--line); }
.fx-ticks { position: relative; height: 26px; margin-right: 10px; }
.fx-ticks span { position: absolute; bottom: 6px; transform: translateX(-50%); font-size: 10px; letter-spacing: 0; text-transform: none; }
.fx-ticks span:first-child { transform: none; } .fx-ticks span:last-child { transform: translateX(-100%); }
.fx-trace-body { max-height: 460px; overflow: auto; }
.fx-trace-row { display: grid; grid-template-columns: minmax(200px, 40%) minmax(0, 1fr); align-items: center; min-height: 26px; border-bottom: 1px solid color-mix(in srgb, var(--line) 55%, transparent); cursor: pointer; outline: none; }
.fx-trace-row:hover, .fx-trace-row.on, .fx-trace-row:focus-visible { background: var(--accent-soft); }
.fx-trace-row.added .sym { color: var(--ok); }
.fx-stack-name { display: flex; align-items: center; gap: 5px; min-width: 0; padding: 4px 8px; border-right: 1px solid var(--line); font: 12px/1.2 var(--mono); color: var(--ink); white-space: nowrap; }
.fx-stack-name .sym { overflow: hidden; text-overflow: ellipsis; }
.fx-stack-name .loc { margin-left: auto; padding-left: 8px; color: var(--muted); font-size: 10.5px; flex: none; }
.fx-stack-name .again, .fx-guide { color: var(--muted); }
.fx-swatch { flex: none; width: 8px; height: 8px; border-radius: 2px; }
.fx-trace .fx-bar { top: 5px; height: 16px; line-height: 14px; border-width: 1px; border-style: solid; font-size: 10px; color: var(--ink-2); }

/* Subagents as small tiles; the info button opens everything about one */
.fx-tiles { display: grid; grid-template-columns: repeat(auto-fill, minmax(230px, 1fr)); gap: 8px; }
.fx-tile { display: flex; align-items: center; gap: 9px; padding: 8px 8px 8px 10px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); min-width: 0; }
.fx-tile.firing { border-color: color-mix(in srgb, var(--bad) 55%, var(--line)); }
.fx-tile.stopped { opacity: .6; }
.fx-tile .who { display: grid; gap: 2px; min-width: 0; flex: 1; }
.fx-tile .name { font-weight: 650; font-size: 12.5px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.fx-tile .meta { display: flex; align-items: center; gap: 5px; font-size: 11px; color: var(--muted); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.fx-avatar.small { width: 26px; height: 26px; border-radius: 8px; }
.fx-avatar.small svg { width: 15px; height: 15px; }
.fx-avatar.incident { background: color-mix(in srgb, var(--bad) 14%, var(--panel)); color: var(--bad); }
.fx-dot { flex: none; width: 7px; height: 7px; border-radius: 50%; background: var(--muted); }
.fx-dot.active { background: var(--ok); } .fx-dot.pending_approval { background: var(--warn); } .fx-dot.terminated { background: var(--bad); }
.fx-dialog form.fx-info { width: min(720px, 100%); }
.fx-info h2 { margin: 2px 0; font-size: 16px; }
.fx-json { min-height: 180px; font: 12px/1.5 var(--mono) !important; }
.fx-verdict { font-size: 12.5px; min-height: 1.2em; } .fx-verdict .bad { color: var(--bad); } .fx-verdict .ok { color: var(--ok); }
.fx-errors { margin: 0; padding-left: 18px; display: grid; gap: 4px; font-size: 12.5px; color: var(--bad); }
.fx-conn { display: flex; flex-wrap: wrap; gap: 6px; }

/* Subagents */
.fx-subs { display: grid; gap: 12px; }
.fx-subs-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 12px; }
.fx-subs-head h2 { margin: 0; font-size: 16px; font-weight: 750; }
.fx-cards { display: grid; grid-template-columns: repeat(auto-fill, minmax(290px, 1fr)); gap: 12px; }
.fx-card { display: grid; gap: 10px; align-content: start; padding: 14px; border: 1px solid var(--line); border-radius: 12px; background: var(--panel); box-shadow: var(--shadow); }
.fx-card.firing { border-color: color-mix(in srgb, var(--bad) 50%, var(--line)); }
.fx-card.stopped { opacity: .65; }
.fx-card-head { display: flex; align-items: flex-start; gap: 10px; }
.fx-card-head .who { display: grid; gap: 3px; min-width: 0; flex: 1; }
.fx-card-head .name { font-weight: 700; font-size: 13.5px; overflow-wrap: anywhere; }
.fx-card-head .id { font: 11px/1 var(--mono); color: var(--muted); }
.fx-avatar { flex: none; display: grid; place-items: center; width: 30px; height: 30px; border-radius: 9px; background: var(--accent-soft); color: var(--accent); }
.fx-avatar svg { width: 18px; height: 18px; }
.fx-props { display: grid; grid-template-columns: 92px minmax(0, 1fr); gap: 6px 10px; font-size: 12.5px; }
.fx-props dt { color: var(--muted); font: 700 10px/1.6 var(--mono); letter-spacing: .08em; text-transform: uppercase; }
.fx-props dd { margin: 0; overflow-wrap: anywhere; }
.fx-link { border: 0; background: none; padding: 0; color: var(--accent); cursor: pointer; font: inherit; text-decoration: underline; text-underline-offset: 2px; }

.fx-dialog { position: fixed; inset: 0; z-index: 60; display: grid; place-items: center; padding: 16px; background: color-mix(in srgb, var(--rail) 45%, transparent); }
.fx-dialog form { width: min(560px, 100%); max-height: calc(100vh - 32px); overflow: auto; display: grid; gap: 12px; padding: 18px; border-radius: 14px; background: var(--panel); border: 1px solid var(--line); box-shadow: var(--shadow); }
.fx-dialog h2 { margin: 0; font-size: 16px; }
.fx-dialog label { display: grid; gap: 5px; font-size: 12.5px; font-weight: 600; color: var(--ink-2); }
.fx-dialog input, .fx-dialog select, .fx-dialog textarea { font: 13.5px/1.4 inherit; padding: 8px 10px; border-radius: 8px; border: 1px solid var(--line-2); background: var(--panel); color: var(--ink); width: 100%; }
.fx-dialog textarea { min-height: 88px; font-family: var(--mono); font-size: 12.5px; }
.fx-dialog .check { display: flex; align-items: center; gap: 8px; font-weight: 500; }
.fx-dialog .check input { width: auto; }
.fx-dialog .error { color: var(--bad); font-size: 12.5px; min-height: 1em; }
.fx-dialog .acts { display: flex; justify-content: flex-end; gap: 8px; }
@media (max-width: 900px) { .fx-inspect-body { grid-template-columns: 1fr; } .fx-peek { border-left: 0; border-top: 1px solid var(--line); } }
@media (max-width: 600px) { .fx-canvas { height: 62vh; } .fx-hint { display: none; } .fx-wf-row { grid-template-columns: minmax(120px, 45%) minmax(0, 1fr); } }
@media (prefers-reduced-motion: reduce) { .fx-node rect.card { transition: none; } .fx-viewport.glide { transition: none; } }
`;

  const svgNode = (tag, attributes, ...children) => window.Foxx.svg(tag, attributes, ...children);
  const pill = (text, tone, title) => h("span", { class: "fx-pill" + (tone ? " " + tone : ""), title: title || undefined }, text);
  const empty = (value) => value === null || value === undefined || value === "";
  const field = (label, value) => h("div", { class: "fx-field" }, h("span", { class: "fx-label" }, label), h("span", { class: "fx-value" }, empty(value) ? h("span", { class: "muted" }, "—") : value));
  const shared = (value) => (!value ? "" : value.withheld ? h("span", { class: "muted" }, "Not shown: it looks like it holds a secret.") : value.text + (value.truncated ? " …" : ""));
  const plain = (value) => (!value ? "" : value.withheld ? "(withheld)" : value.text || "");
  const tone = (priority) => (priority === "P0" ? "p0" : priority === "P1" ? "p1" : "");
  const note = (text) => h("p", { class: "fx-note" }, text);
  const SIGNED = new Set(["flow-approve", "flow-discard", "flow-priority", "flow-context-add", "flow-context-remove", "flow-person-add", "flow-person-remove", "flow-test-require", "flow-proposal-approve", "flow-proposal-reject", "flow-limit"]);
  // Code owners of .crane/flows.json ask for these; only SIGNED ones change signed knowledge
  const OWNERS = new Set([...SIGNED, "flow-watch-edit"]);
  const CHECKS = [
    ["trace_drift", "The call graph drifts from the approved trace", "TRACE_DRIFT"],
    ["required_tests", "A required test fails or goes missing", "REQUIRED_TEST_FAILING / MISSING"],
    ["denials", "Agents are denied repeatedly", "DENIALS"],
  ];
  const ROBOT = "M12 3v3M7 8h10a3 3 0 0 1 3 3v5a3 3 0 0 1-3 3H7a3 3 0 0 1-3-3v-5a3 3 0 0 1 3-3zM9 13h.01M15 13h.01M2 12v3M22 12v3";
  const icon = (d) => svgNode("svg", { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": "1.9", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" }, svgNode("path", { d }));
  const ICONS = {
    zoomIn: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM21 21l-5-5M11 8v6M8 11h6",
    zoomOut: "M11 4a7 7 0 1 0 0 14 7 7 0 0 0 0-14zM21 21l-5-5M8 11h6",
    fit: "M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5",
    reset: "M3 12a9 9 0 1 0 3-6.7M3 4v5h5",
    focus: "M12 9a3 3 0 1 0 0 6 3 3 0 0 0 0-6zM12 2v3M12 19v3M2 12h3M19 12h3",
    clear: "M6 6l12 12M18 6L6 18",
  };
  const remember = (key, value) => { try { if (value === null) sessionStorage.removeItem(key); else sessionStorage.setItem(key, value); } catch (error) { /* storage unavailable */ } };
  const recall = (key) => { try { return sessionStorage.getItem(key); } catch (error) { return null; } };

  function injectStyle() {
    if (document.getElementById("fx-style")) document.getElementById("fx-style").remove();
    document.head.append(h("style", { id: "fx-style" }, CSS));
  }

  // ---------------------------------------------------------------- requests (applied on a machine)

  /** Ask for a change: the person gives a reason, GitHub confirms the role, and a person applies it
   * on a machine. fields: [{key, label, kind: text|textarea|select|number|check, options, value, hint}] */
  function request(op, title, options) {
    options = options || {};
    const inputs = {};
    const controls = (options.fields || []).map((spec) => {
      let control;
      if (spec.kind === "select") control = h("select", { id: "fx-" + spec.key }, spec.options.map(([value, label]) => h("option", { value, selected: value === spec.value ? "" : undefined }, label)));
      else if (spec.kind === "textarea") control = h("textarea", { id: "fx-" + spec.key, placeholder: spec.hint || "" }, spec.value || "");
      else if (spec.kind === "check") control = h("input", { id: "fx-" + spec.key, type: "checkbox", checked: spec.value ? "" : undefined });
      else control = h("input", { id: "fx-" + spec.key, type: spec.kind === "number" ? "number" : "text", value: spec.value === undefined || spec.value === null ? "" : String(spec.value), placeholder: spec.hint || "", maxlength: spec.kind === "number" ? undefined : String(spec.max || 300) });
      inputs[spec.key] = { control, spec };
      return spec.kind === "check" ? h("label", { class: "check" }, control, spec.label) : h("label", {}, spec.label, control);
    });
    const reason = h("input", { id: "fx-reason", type: "text", maxlength: "300", placeholder: "Recorded with the change" });
    const error = h("div", { class: "error", role: "alert" });
    const submit = h("button", { class: "fx-btn primary", type: "submit" }, "Continue with GitHub");
    const close = () => back.remove();
    const back = h("div", { class: "fx-dialog", role: "dialog", "aria-modal": "true", "aria-label": title, onclick: (event) => { if (event.target === back) close(); }, onkeydown: (event) => { if (event.key === "Escape") close(); } },
      h("form", { onsubmit: async (event) => {
        event.preventDefault();
        error.textContent = "";
        const params = Object.assign({}, options.params || {});
        for (const [key, { control, spec }] of Object.entries(inputs)) {
          if (spec.kind === "check") params[key] = control.checked;
          else if (spec.kind === "number") params[key] = control.value === "" ? null : Number(control.value);
          else params[key] = control.value.trim();
          if (spec.required && (params[key] === "" || params[key] === null)) { error.textContent = spec.label + " is required."; control.focus(); return; }
        }
        if (options.shape) params.checks = options.shape(params);
        if (options.validate) {
          submit.disabled = true;
          const problem = await options.validate(params);
          submit.disabled = false;
          if (problem) { error.textContent = problem; return; }
        }
        if (!reason.value.trim()) { error.textContent = "Say why; it is recorded with the change."; reason.focus(); return; }
        submit.disabled = true;
        try {
          const staged = await apiWrite("/api/flows", { action: "stage", op, params, reason: reason.value.trim() });
          location.href = staged.continue + "&page=flows";
        } catch (problem) { submit.disabled = false; error.textContent = problem.message; }
      } },
        h("div", { class: "fx-kind" }, SIGNED.has(op) ? "Signed change · request" : OWNERS.has(op) ? "Code owners · request" : "Request"),
        h("h2", {}, title),
        options.effect ? note(options.effect) : null,
        ...controls,
        h("label", {}, "Why", reason),
        error,
        note(SIGNED.has(op)
          ? "GitHub confirms you are a code owner of the signed file (or a repository admin when CODEOWNERS does not cover it). Nothing changes yet: a person applies it on a machine with an admin key, where Crane checks and signs it."
          : OWNERS.has(op)
            ? "GitHub confirms you are a code owner of .crane/flows.json (or a repository admin when CODEOWNERS does not cover it). A person applies it on a machine ('crane brick inbox apply', or one click in Landscape)."
            : "GitHub confirms you can write to the repository. A person applies it on a machine ('crane brick inbox apply', or one click in Landscape)."),
        h("div", { class: "acts" }, h("button", { class: "fx-btn ghost", type: "button", onclick: close }, "Cancel"), submit)));
    document.body.append(back);
    const first = Object.values(inputs)[0];
    (first ? first.control : reason).focus();
  }

  /** The check boxes of a watcher's checks, and how to turn them back into a list */
  const checkFields = (chosen) => CHECKS.map(([key, label]) => ({ key, label, kind: "check", value: chosen ? chosen.includes(key) : true }));
  const shapeChecks = (params) => { const checks = CHECKS.map(([key]) => key).filter((key) => params[key]); for (const [key] of CHECKS) delete params[key]; return checks; };

  // ---------------------------------------------------------------- the map's layout

  const NODE = { w: 168, h: 38, colGap: 88, rowGap: 16, pad: 32, head: 34 };

  /** Lay out the map in columns, left to right: flows (and drafts), then each stage column of
   * zones, then the zones in no flow. Shorter columns are centered against the tallest one.
   * columns: [{label, items}] in order; returns positions, column headers, and the size */
  function layout(columns) {
    const positions = new Map();
    const headers = [];
    const tallest = Math.max(1, ...columns.map((column) => column.items.length));
    columns.forEach((column, index) => {
      const x = NODE.pad + index * (NODE.w + NODE.colGap);
      const offset = ((tallest - column.items.length) * (NODE.h + NODE.rowGap)) / 2;
      headers.push({ x, text: column.label + ' · ' + column.items.length });
      column.items.forEach((item, row) => positions.set(item.key, { x, y: NODE.pad + NODE.head + offset + row * (NODE.h + NODE.rowGap), item }));
    });
    return { positions, headers, width: NODE.pad * 2 + columns.length * (NODE.w + NODE.colGap) - NODE.colGap, height: NODE.pad * 2 + NODE.head + tallest * (NODE.h + NODE.rowGap) - NODE.rowGap };
  }

  /** A curve from the right side of one node to the left side of the next; a link back to the left
   * (or within a column) arcs under the nodes instead of through them */
  function curve(a, b) {
    const x1 = a.x + NODE.w, y1 = a.y + NODE.h / 2, x2 = b.x, y2 = b.y + NODE.h / 2;
    if (x2 > x1 + 10) { const bend = Math.max(36, (x2 - x1) / 2); return 'M' + x1 + ' ' + y1 + ' C' + (x1 + bend) + ' ' + y1 + ' ' + (x2 - bend) + ' ' + y2 + ' ' + x2 + ' ' + y2; }
    const drop = Math.max(a.y, b.y) + NODE.h + 28;
    return 'M' + x1 + ' ' + y1 + ' C' + (x1 + 60) + ' ' + y1 + ' ' + (x1 + 60) + ' ' + drop + ' ' + (x1 + x2) / 2 + ' ' + drop + ' S' + (x2 - 60) + ' ' + y2 + ' ' + x2 + ' ' + y2;
  }

  /** One node: a kind stripe, the name, a red dot for incidents, an agent badge when monitored */
  function node(position, onPick) {
    const { x, y, item } = position;
    const name = item.name.length > 19 ? item.name.slice(0, 18) + '…' : item.name;
    const color = item.kind === 'zone' ? 'var(--low)' : item.draft ? 'var(--muted)' : item.priority === 'P0' ? 'var(--bad)' : item.priority === 'P1' ? 'var(--warn)' : 'var(--info)';
    const label = (item.kind === 'zone' ? 'Zone ' : item.draft ? 'Draft flow ' : 'Flow ') + item.name + (item.priority ? ', ' + item.priority : '') + (item.monitors.length ? ', monitored by ' + item.monitors.join(', ') : '') + (item.incidents ? ', ' + item.incidents + ' incident(s)' : '');
    return svgNode('g', { class: 'fx-node ' + item.kind + (item.draft ? ' draft' : ''), tabindex: '0', role: 'button', 'aria-label': label, 'data-key': item.key, transform: 'translate(' + x + ',' + y + ')', onclick: () => onPick(item.key), onkeydown: (event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); onPick(item.key); } } },
      svgNode('title', {}, item.name + (item.title && item.title !== item.name ? ' — ' + item.title : '') + (item.monitors.length ? ' · monitored by ' + item.monitors.join(', ') : '')),
      svgNode('rect', { class: 'card', width: NODE.w, height: NODE.h, rx: 10 }),
      svgNode('rect', { class: 'stripe', x: 0, y: 0, width: 5, height: NODE.h, rx: 2.5, fill: color }),
      svgNode('text', { class: 'glyph', x: 15, y: NODE.h / 2 + 4 }, item.kind === 'zone' ? '▦' : '⇢'),
      svgNode('text', { class: 'name', x: 31, y: NODE.h / 2 + 4.5 }, name),
      item.incidents ? svgNode('circle', { cx: 4, cy: 4, r: 4.5, fill: 'var(--bad)', stroke: 'var(--panel)', 'stroke-width': 1.5 }) : null,
      item.monitors.length ? agentBadge(NODE.w - 10, -8) : null);
  }

  /** A small agent badge over a node's corner */
  function agentBadge(x, y) {
    return svgNode("g", { transform: `translate(${x},${y})`, "aria-hidden": "true" },
      svgNode("circle", { cx: 8, cy: 8, r: 8.5, fill: "var(--accent)", stroke: "var(--panel)", "stroke-width": 1.5 }),
      svgNode("rect", { x: 3.6, y: 5.6, width: 8.8, height: 6.6, rx: 2, fill: "var(--accent-ink)" }),
      svgNode("line", { x1: 8, y1: 5.6, x2: 8, y2: 3.4, stroke: "var(--accent-ink)", "stroke-width": 1.3, "stroke-linecap": "round" }),
      svgNode("circle", { cx: 6.4, cy: 8.9, r: 0.95, fill: "var(--accent)" }),
      svgNode("circle", { cx: 9.6, cy: 8.9, r: 0.95, fill: "var(--accent)" }));
  }

  // ---------------------------------------------------------------- call trees

  /** Walk a call graph from its starting points: one row per call in order, with its depth, its
   * place, and how many calls it covers (a function reached again is shown once more, not opened) */
  function callRows(graph) {
    const children = new Map();
    for (const edge of graph.edges || []) { if (!children.has(edge.from)) children.set(edge.from, []); children.get(edge.from).push(edge.to); }
    const byId = new Map((graph.nodes || []).map((entry) => [entry.id, entry]));
    const rows = [];
    const seen = new Set();
    const walk = (id, depth) => {
      const entry = byId.get(id);
      if (!entry || rows.length >= 400) return 0;
      const again = seen.has(id);
      seen.add(id);
      const row = { node: entry, depth, start: rows.length, size: 1, again };
      rows.push(row);
      if (!again) for (const next of children.get(id) || []) row.size += walk(next, depth + 1);
      return row.size;
    };
    for (const entry of graph.entry_points || []) walk(entry, 0);
    return rows;
  }
  const FILE_TONES = ["var(--info)", "var(--low)", "var(--accent)", "var(--warn)", "var(--crit)", "var(--ok)"];

  // ---------------------------------------------------------------- the page

  async function flowsView(main) {
    if (needRepository(main)) return;
    injectStyle();
    let data;
    try { data = await api("/api/flows"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
    for (const key of ["flows", "zones", "drafts", "proposals", "watchers", "incidents", "requests", "bricks", "assignments"]) data[key] = data[key] || [];

    // Who monitors a target: its active watchers and the agents assigned to it
    const watchersOf = (flowId) => data.watchers.filter((watcher) => watcher.flow === flowId);
    const assignedTo = (key) => data.assignments.filter((assignment) => (assignment.targets || []).includes(key));
    const monitorsOf = (key, flowId) => [...(flowId ? watchersOf(flowId).filter((watcher) => watcher.active).map((watcher) => "watcher " + watcher.watcher_id) : []), ...assignedTo(key).map((assignment) => assignment.agent)];

    const items = [];
    for (const flow of data.flows) {
      const key = "flow:" + flow.id;
      items.push({ key, kind: "flow", id: flow.id, name: flow.id, priority: flow.priority, draft: false, incidents: flow.incidents, title: flow.description || "", zones: flow.stages || [], monitors: monitorsOf(key, flow.id) });
    }
    for (const draft of data.drafts.filter((candidate) => !candidate.exists)) {
      items.push({ key: "draft:" + draft.flow_id, kind: "draft", id: draft.flow_id, name: draft.flow_id, draft: true, incidents: 0, title: plain(draft.description), zones: [], monitors: [] });
    }
    for (const zone of data.zones) {
      const key = "zone:" + zone.id;
      items.push({ key, kind: "zone", id: zone.id, name: zone.id, draft: false, incidents: 0, title: zone.description || "", zones: [], monitors: monitorsOf(key, null) });
    }
    const byKey = new Map(items.map((item) => [item.key, item]));
    const rank = { P0: 0, P1: 1, P2: 2, P3: 3 };
    const critical = { restricted: 0, critical: 1, sensitive: 2, routine: 3 };
    const zoneCriticality = new Map(data.zones.map((zone) => [zone.id, zone.criticality]));
    // Columns: flows by priority (then drafts); each zone in the column of the furthest stage it
    // holds in any flow, ordered by the rows of what leads into it; zones in no flow last
    const firstColumn = [
      ...items.filter((item) => item.kind === "flow").sort((a, b) => (rank[a.priority] ?? 9) - (rank[b.priority] ?? 9) || a.name.localeCompare(b.name)),
      ...items.filter((item) => item.kind === "draft"),
    ];
    const stageColumn = new Map();
    for (const flow of data.flows) (flow.stages || []).forEach((stage, index) => stageColumn.set(stage, Math.max(stageColumn.get(stage) || 0, index + 1)));
    const rowOf = new Map(firstColumn.map((item, index) => [item.key, index]));
    const columns = [{ label: "Flows", items: firstColumn }];
    const deepestStage = Math.max(0, ...stageColumn.values());
    for (let column = 1; column <= deepestStage; column += 1) {
      const zones = items.filter((item) => item.kind === "zone" && stageColumn.get(item.id) === column);
      // The average row of what leads into each zone (the flow, or the stage before it)
      const lead = (zone) => {
        const rows = [];
        for (const flow of data.flows) {
          const index = (flow.stages || []).indexOf(zone.id);
          if (index < 0) continue;
          const before = index === 0 ? "flow:" + flow.id : "zone:" + flow.stages[index - 1];
          if (rowOf.has(before)) rows.push(rowOf.get(before));
        }
        return rows.length ? rows.reduce((sum, row) => sum + row, 0) / rows.length : Infinity;
      };
      zones.sort((a, b) => lead(a) - lead(b) || a.name.localeCompare(b.name));
      zones.forEach((zone, index) => rowOf.set(zone.key, index));
      if (zones.length) columns.push({ label: "Stage " + column, items: zones });
    }
    const loose = items.filter((item) => item.kind === "zone" && !stageColumn.has(item.id)).sort((a, b) => (critical[zoneCriticality.get(a.id)] ?? 9) - (critical[zoneCriticality.get(b.id)] ?? 9) || a.name.localeCompare(b.name));
    if (loose.length) columns.push({ label: "Not in a flow", items: loose });
    // Links: each flow's path, flow → first stage → next stage …, with the flows that take it
    const links = new Map();
    for (const flow of data.flows) {
      const path = ["flow:" + flow.id, ...(flow.stages || []).map((stage) => "zone:" + stage)];
      for (let index = 0; index + 1 < path.length; index += 1) {
        const id = path[index] + "|" + path[index + 1];
        if (!links.has(id)) links.set(id, { from: path[index], to: path[index + 1], flows: new Set() });
        links.get(id).flows.add(flow.id);
      }
    }

    const state = { selected: null, filter: null, peek: null, nodes: new Map(), view: { x: 0, y: 0, k: 1 }, traceSource: null, subFilter: "all" };
    const saved = recall("fx-selected");
    if (saved && byKey.has(saved)) state.selected = saved;

    // Header
    const openIncidents = data.incidents.length;
    const active = data.watchers.filter((watcher) => watcher.active).length;
    const pending = data.proposals.filter((proposal) => proposal.status === "PENDING").length;
    const stat = (label, value, alert) => h("span", { class: "fx-stat" + (alert ? " alert" : "") }, h("b", {}, String(value)), " " + label);
    const root = h("div", { class: "fx" });
    root.append(h("div", { class: "fx-head" },
      h("div", {}, h("h1", { class: "fx-title" }, "Flows"), h("div", { class: "fx-meta" }, "Reported by ", data.reported_by ? data.reported_by.actor : "a member", " · ", dt(data.reported_at))),
      h("div", { class: "fx-spacer" }),
      h("div", { class: "fx-stats" }, stat("flows", data.flows.length), stat("zones", data.zones.length), stat("subagents", active), pending ? stat("proposals", pending, true) : null, stat("incidents", openIncidents, openIncidents > 0)),
      h("button", { class: "fx-btn primary", type: "button", onclick: () => request("flow-trace", "Trace a new flow", { effect: "Crane follows the calls from a starting point through the repository and saves the result as a draft. The same code always gives the same trace; a code owner or admin approves it.", fields: [
        { key: "from", label: "Starting point (function, RPC handler, or path:Symbol)", hint: "api/pay.py:CheckoutService.submit", required: true },
        { key: "flow", label: "Flow id (made from the starting point when empty)", hint: "checkout" },
        { key: "depth", label: "Deepest call chain", kind: "number", value: 8 },
        { key: "description", label: "What the flow does", hint: "Pay for an order" }] }) }, "Trace a flow")));

    // ---------------------------------------------------------------- the map: pan, zoom, select

    const viewport = svgNode("g", { class: "fx-viewport" });
    const edgeLayer = svgNode("g", { class: "fx-edges" });
    const nodeLayer = svgNode("g");
    viewport.append(edgeLayer, nodeLayer);
    const svg = svgNode("svg", { role: "group", "aria-label": "Map of flows and zones: drag to pan, scroll to zoom" },
      svgNode("defs", {}, svgNode("marker", { id: "fx-arrow", viewBox: "0 0 10 10", refX: 9, refY: 5, markerWidth: 7, markerHeight: 7, orient: "auto-start-reverse" }, svgNode("path", { d: "M0 0L10 5L0 10z", fill: "var(--accent)" }))),
      viewport);
    const zoomText = h("div", { class: "fx-zoom", "aria-live": "polite" }, "100%");
    const tool = (name, label, run) => h("button", { class: "fx-tool", type: "button", title: label, "aria-label": label, onclick: run }, icon(ICONS[name]));
    const focusButton = tool("focus", "Zoom to the selection", () => focusSelection(true));
    const clearButton = tool("clear", "Clear the selection", () => pick(null));
    const canvas = h("section", { class: "fx-canvas", "aria-label": "Map of flows and zones" },
      svg,
      h("div", { class: "fx-tools", role: "toolbar", "aria-label": "Map controls" },
        tool("zoomIn", "Zoom in", () => zoomBy(1.25)), tool("zoomOut", "Zoom out", () => zoomBy(0.8)), zoomText, h("hr"),
        tool("fit", "Fit everything", () => fit(true)), tool("reset", "Reset to 100%", () => setView({ x: 0, y: 0, k: 1 }, true)), focusButton, clearButton),
      h("div", { class: "fx-legend" }, h("span", {}, h("i", { style: "background:var(--bad)" }), "P0"), h("span", {}, h("i", { style: "background:var(--warn)" }), "P1"), h("span", {}, h("i", { style: "background:var(--info)" }), "flow"), h("span", {}, h("i", { style: "background:var(--low)" }), "zone"), h("span", {}, h("i", { style: "background:var(--accent);border-radius:50%" }), "monitored by an agent"), h("span", {}, h("i", { style: "background:var(--bad);border-radius:50%" }), "incident")),
      h("div", { class: "fx-hint" }, "Drag to pan · scroll to zoom"));
    root.append(canvas);
    const inspect = h("section", { class: "fx-inspect", "aria-label": "Inspector" });
    const subagents = h("section", { class: "fx-subs", "aria-label": "Subagents" });
    root.append(inspect, subagents);
    main.append(root);

    let geometry = null;
    const size = () => ({ width: canvas.clientWidth || 960, height: canvas.clientHeight || 560 });
    function setView(view, glide) {
      state.view = { x: view.x, y: view.y, k: Math.min(3, Math.max(0.2, view.k)) };
      viewport.classList.toggle("glide", Boolean(glide));
      viewport.style.transform = `translate(${state.view.x}px, ${state.view.y}px) scale(${state.view.k})`;
      zoomText.textContent = Math.round(state.view.k * 100) + "%";
    }
    function zoomBy(factor, at) {
      const { width, height } = size();
      const point = at || { x: width / 2, y: height / 2 };
      const k = Math.min(3, Math.max(0.2, state.view.k * factor));
      const ratio = k / state.view.k;
      setView({ x: point.x - (point.x - state.view.x) * ratio, y: point.y - (point.y - state.view.y) * ratio, k }, !at);
    }
    /** Fit a box (in map units) into the canvas */
    function frame(box, maxK, glide) {
      const { width, height } = size();
      const k = Math.min(maxK, (width - 48) / Math.max(1, box.width), (height - 48) / Math.max(1, box.height));
      setView({ x: (width - box.width * k) / 2 - box.x * k, y: (height - box.height * k) / 2 - box.y * k, k }, glide);
    }
    function fit(glide) { if (geometry) frame({ x: 0, y: 0, width: geometry.width, height: geometry.height }, 1.2, glide); }
    function focusSelection(glide) {
      if (!state.selected || !geometry) return;
      const keys = [state.selected, ...related(state.selected)].filter((key) => geometry.positions.has(key));
      if (!keys.length) return;
      const boxes = keys.map((key) => geometry.positions.get(key));
      const x = Math.min(...boxes.map((box) => box.x)) - 30, y = Math.min(...boxes.map((box) => box.y)) - 30;
      const right = Math.max(...boxes.map((box) => box.x + NODE.w)) + 30, bottom = Math.max(...boxes.map((box) => box.y + NODE.h)) + 30;
      frame({ x, y, width: right - x, height: bottom - y }, 1.6, glide);
    }

    // Drag to pan (a short move is still a click), wheel to zoom around the pointer
    let drag = null, suppressClick = false;
    svg.addEventListener("pointerdown", (event) => { if (event.button !== 0) return; drag = { id: event.pointerId, x: event.clientX, y: event.clientY, view: Object.assign({}, state.view), moved: false }; });
    svg.addEventListener("pointermove", (event) => {
      if (!drag || event.pointerId !== drag.id) return;
      const dx = event.clientX - drag.x, dy = event.clientY - drag.y;
      if (!drag.moved && Math.hypot(dx, dy) < 4) return;
      if (!drag.moved) { drag.moved = true; svg.setPointerCapture(event.pointerId); canvas.classList.add("dragging"); }
      setView({ x: drag.view.x + dx, y: drag.view.y + dy, k: drag.view.k });
    });
    const endDrag = (event) => { if (!drag || event.pointerId !== drag.id) return; if (drag.moved) { suppressClick = true; setTimeout(() => { suppressClick = false; }, 0); } canvas.classList.remove("dragging"); drag = null; };
    svg.addEventListener("pointerup", endDrag);
    svg.addEventListener("pointercancel", endDrag);
    svg.addEventListener("click", (event) => { if (suppressClick) { event.stopPropagation(); return; } if (event.target === svg) pick(null); }, true);
    svg.addEventListener("wheel", (event) => {
      event.preventDefault();
      const box = svg.getBoundingClientRect();
      zoomBy(Math.exp(-event.deltaY * (event.deltaMode === 1 ? 0.05 : 0.0016)), { x: event.clientX - box.left, y: event.clientY - box.top });
    }, { passive: false });
    canvas.addEventListener("keydown", (event) => {
      if (["INPUT", "TEXTAREA", "SELECT"].includes(event.target.tagName)) return;
      if (event.key === "+" || event.key === "=") { zoomBy(1.25); event.preventDefault(); }
      else if (event.key === "-") { zoomBy(0.8); event.preventDefault(); }
      else if (event.key === "0") { fit(true); event.preventDefault(); }
      else if (event.key === "Escape") pick(null);
    });

    const linkPaths = new Map();
    function drawMap(keepView) {
      geometry = layout(columns);
      state.nodes = new Map();
      if (!items.length) {
        canvas.append(h("div", { class: "fx-empty" }, "No flows or zones yet. Trace a flow from its starting point; once a code owner approves it, it appears here."));
        return;
      }
      linkPaths.clear();
      put(edgeLayer, [...links.entries()].map(([id, link]) => {
        const a = geometry.positions.get(link.from), b = geometry.positions.get(link.to);
        if (!a || !b) return null;
        const path = svgNode("path", { class: "fx-edge", d: curve(a, b) }, svgNode("title", {}, link.from.replace(/^\w+:/, "") + " → " + link.to.replace(/^\w+:/, "") + " (" + [...link.flows].join(", ") + ")"));
        linkPaths.set(id, path);
        return path;
      }));
      put(nodeLayer,
        geometry.headers.map((header) => svgNode("text", { class: "fx-band", x: header.x + 2, y: NODE.pad + 12 }, header.text)),
        [...geometry.positions.values()].map((position) => { const group = node(position, pick); state.nodes.set(position.item.key, group); return group; }));
      highlight();
      if (!keepView) { if (state.selected) focusSelection(false); else fit(false); }
    }

    /** The flows a selection is about: the flow itself, or the flows through a zone */
    function flowsOf(key) {
      const item = byKey.get(key);
      if (!item) return new Set();
      if (item.kind === "flow") return new Set([item.id]);
      if (item.kind === "zone") return new Set(items.filter((other) => other.kind === "flow" && other.zones.includes(item.id)).map((other) => other.id));
      return new Set();
    }
    function related(key) {
      const item = byKey.get(key);
      if (!item) return new Set();
      if (item.kind === "flow") return new Set(item.zones.map((zone) => "zone:" + zone));
      if (item.kind === "zone") return new Set(items.filter((other) => other.kind === "flow" && other.zones.includes(item.id)).map((other) => other.key));
      return new Set();
    }

    function highlight() {
      const near = state.selected ? related(state.selected) : new Set();
      const lit = state.selected ? flowsOf(state.selected) : new Set();
      // Along the selected flow's path (or the flows through the selected zone), every zone lights up
      const path = new Set();
      for (const link of links.values()) if ([...link.flows].some((flow) => lit.has(flow))) { path.add(link.from); path.add(link.to); }
      for (const [key, group] of state.nodes) {
        group.classList.toggle("selected", key === state.selected);
        group.classList.toggle("related", key !== state.selected && (near.has(key) || path.has(key)));
        group.classList.toggle("dim", Boolean(state.selected) && key !== state.selected && !near.has(key) && !path.has(key));
      }
      for (const [id, element] of linkPaths) {
        const on = [...links.get(id).flows].some((flow) => lit.has(flow));
        element.classList.toggle("lit", on);
        element.classList.toggle("faded", Boolean(state.selected) && !on);
        element.setAttribute("marker-end", on ? "url(#fx-arrow)" : "");
        if (on) edgeLayer.append(element);
      }
      focusButton.disabled = !state.selected;
      clearButton.disabled = !state.selected;
    }

    function pick(key, options) {
      state.selected = key && byKey.has(key) ? key : null;
      state.filter = (options && options.filter) || null;
      state.peek = null;
      state.traceSource = null;
      remember("fx-selected", state.selected);
      highlight();
      if (state.selected) focusSelection(true);
      drawInspector();
      drawSubagents();
      if (options && options.scroll) canvas.scrollIntoView({ behavior: "smooth", block: "start" });
    }

    // ---------------------------------------------------------------- the inspector

    /** Filter chips: [id, label, count]; the chosen one is kept while the selection stays */
    function chips(list) {
      if (!list.some(([id]) => id === state.filter)) state.filter = list[0][0];
      return h("div", { class: "fx-chips", role: "toolbar", "aria-label": "Show" }, list.map(([id, label, count]) => h("button", { class: "fx-chip", type: "button", "aria-pressed": String(id === state.filter), onclick: () => { state.filter = id; state.peek = null; drawInspector(); } }, label, count !== undefined && count !== null ? h("span", { class: "n" }, String(count)) : null)));
    }
    /** A clickable list row; clicking opens the item in the detail pane */
    function itemRow(peek, title, meta, right) {
      const on = state.peek && state.peek.type === peek.type && state.peek.id === peek.id;
      return h("button", { class: "fx-item" + (on ? " on" : ""), type: "button", "aria-pressed": String(Boolean(on)), onclick: () => { state.peek = peek; drawInspector(); } },
        h("span", { class: "t" }, title), meta ? h("span", { class: "m" }, meta) : null, right ? h("span", { class: "r" }, right) : null);
    }
    const list = (rows, none) => (rows.length ? h("div", { class: "fx-items" }, rows) : note(none));
    const peekHead = (kind, title, ...pills) => h("div", { class: "fx-peek-head" }, h("span", { class: "fx-kind" }, kind), h("h3", { class: "fx-peek-title" }, title), pills.length ? h("div", { class: "fx-acts" }, pills) : null);

    function drawInspector() {
      const item = state.selected ? byKey.get(state.selected) : null;
      let view;
      if (!item) view = repositoryView();
      else if (item.kind === "zone") view = zoneView(data.zones.find((zone) => zone.id === item.id));
      else if (item.kind === "draft") view = draftView(data.drafts.find((draft) => draft.flow_id === item.id));
      else view = flowView(data.flows.find((flow) => flow.id === item.id));
      put(inspect, view.head, view.chips, h("div", { class: "fx-inspect-body" }, h("div", { class: "fx-pane" }, view.pane), h("aside", { class: "fx-peek", "aria-label": "Details of the item clicked", "aria-live": "polite" }, view.peek || note("Click an item to see it here."))));
    }

    // At rest: the repository's routing, limits, proposals, incidents, and requests
    function repositoryView() {
      const proposals = data.proposals.filter((proposal) => proposal.status === "PENDING");
      const linked = Object.values(data.integrations || {}).filter((row) => row.connected).length;
      const nav = chips([["overview", "Overview"], ["connections", "Connections", linked + "/" + (data.integration_schemas || []).length], ["proposals", "Proposals", proposals.length], ["incidents", "Incidents", data.incidents.length], ["requests", "Requests", data.requests.length]]);
      let pane, peek = null;
      if (state.filter === "connections") pane = connectionsPane();
      else if (state.filter === "proposals") pane = list(proposals.map((proposal) => itemRow({ type: "proposal", id: proposal.proposal_id }, proposalTitle(proposal), proposal.flow + " · " + dt(proposal.proposed_at), pill(proposal.kind))), "No proposals from agents are waiting.");
      else if (state.filter === "incidents") pane = list(data.incidents.slice(0, 50).map((incident) => itemRow({ type: "incident", id: incident.incident_id }, incident.summary, incident.flow + " · " + dt(incident.raised_at), pill(incident.code, "bad"))), "No incidents.");
      else if (state.filter === "requests") pane = list(data.requests.slice(0, 50).map((row, index) => itemRow({ type: "request", id: index }, requestTitle(row), "@" + row.requested_by + " · " + dt(row.requested_at), chip(requestStatus(row)))), "No flow requests were made here.");
      else pane = [
        note("Select a flow or zone on the map. Agents read the same knowledge through their hooks ('crane-request flow ID'); nothing an agent asks is ever run."),
        h("div", { class: "fx-section" }, h("h3", {}, "Model routing"), h("div", { class: "fx-grid" }, (data.routing || []).map((route) => field(route.priority + " → " + route.tiers.join(", "), route.model || h("span", { class: "muted" }, "no available model"))))),
        h("div", { class: "fx-section" }, h("h3", {}, "Limits agents work within"),
          h("div", { class: "fx-grid" }, field("Subagents per agent session", String(data.limits ? data.limits.max_watchers_per_session : "—")), field("Subagents per flow", String(data.limits ? data.limits.max_watchers_per_flow : "—")), field("Provider models", String((data.models || []).length))),
          h("div", {}, h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-limit", "Change the limits agents work within", { effect: "Only a person changes these. Agents start watchers by themselves only up to them.", fields: [{ key: "session", label: "Watchers one agent session may start", kind: "number", value: data.limits && data.limits.max_watchers_per_session }, { key: "per_flow", label: "Watchers per flow", kind: "number", value: data.limits && data.limits.max_watchers_per_flow }] }) }, "Change limits…"))),
      ];
      if (state.peek) peek = sharedPeek(state.peek);
      return { head: h("div", { class: "fx-inspect-head" }, h("span", { class: "fx-kind" }, "Repository"), h("h2", { class: "fx-inspect-title" }, repositoryName())), chips: nav, pane, peek };
    }

    function zoneView(zone) {
      const through = data.flows.filter((flow) => (flow.stages || []).includes(zone.id));
      const agents = assignedTo("zone:" + zone.id);
      const head = h("div", { class: "fx-inspect-head" }, h("span", { class: "fx-kind" }, "Zone"), h("h2", { class: "fx-inspect-title" }, zone.id), pill(zone.status), pill(zone.criticality, zone.criticality === "critical" || zone.criticality === "restricted" ? "bad" : ""), agents.length ? pill("monitored", "accent") : null);
      const nav = chips([["overview", "Overview"], ["flows", "Flows", through.length], ["agents", "Agents", agents.length], ["bricks", "Bricks", (zone.bricks || []).length]]);
      let pane;
      if (state.filter === "flows") pane = list(through.map((flow) => itemRow({ type: "flow", id: flow.id }, flow.id, flow.description || "", flow.priority ? pill(flow.priority, tone(flow.priority)) : null)), "No flow lists this zone among its stages.");
      else if (state.filter === "agents") pane = list(agents.map((assignment, index) => itemRow({ type: "assignment", id: index, key: "zone:" + zone.id }, assignment.agent, (assignment.targets || []).join(", "), chip(assignment.runtime_status || assignment.assignment_status))), "No agent is assigned to this zone (assign one on the Bricks page).");
      else if (state.filter === "bricks") pane = bricksList(zone.bricks || []);
      else pane = h("div", { class: "fx-grid" }, field("Description", zone.description), field("Criticality", zone.criticality), field("Autonomy ceiling", zone.ceiling), field("Status", zone.status), field("Files covered", empty(zone.files) ? "" : String(zone.files)), field("Owner", zone.owner), field("Flows through it", String(through.length)));
      return { head, chips: nav, pane, peek: state.peek ? sharedPeek(state.peek) : null };
    }

    function draftView(draft) {
      const head = h("div", { class: "fx-inspect-head" }, h("span", { class: "fx-kind" }, "Draft flow"), h("h2", { class: "fx-inspect-title" }, draft.flow_id), pill("revision " + draft.current + " of " + draft.revisions.length), pill("by " + draft.via));
      const nav = chips([["trace", "Trace"], ["revisions", "Revisions", draft.revisions.length]]);
      const pane = state.filter === "revisions" ? revisions(draft) : [draftFacts(draft), traceView(draft.graph, draft.diff), draftActions(draft)];
      return { head, chips: nav, pane, peek: state.peek ? sharedPeek(state.peek, { graph: draft.graph, diff: draft.diff }) : null };
    }

    function flowView(flow) {
      const draft = data.drafts.find((candidate) => candidate.flow_id === flow.id);
      const watchers = watchersOf(flow.id);
      const agents = assignedTo("flow:" + flow.id);
      const proposals = data.proposals.filter((proposal) => proposal.flow === flow.id && proposal.status === "PENDING");
      const monitored = watchers.some((watcher) => watcher.active) || agents.length;
      const head = h("div", { class: "fx-inspect-head" }, h("span", { class: "fx-kind" }, "Flow"), h("h2", { class: "fx-inspect-title" }, flow.id),
        flow.priority ? pill(flow.priority, tone(flow.priority)) : pill("no priority"), pill(flow.status), flow.trace ? pill("traced r" + flow.trace.revision, "ok") : pill("not traced"), draft ? pill("draft r" + draft.current, "accent") : null, monitored ? pill("monitored", "accent") : null, flow.incidents ? pill(flow.incidents + " incident(s)", "bad") : null);
      const nav = chips([["overview", "Overview"], ["zones", "Zones", (flow.stages || []).length], ["context", "Context", (flow.context || []).length], ["tests", "Tests", (flow.tests || []).length], ["agents", "Agents", watchers.length + agents.length], ["trace", "Trace", flow.trace ? flow.trace.nodes : draft ? "draft" : 0], ["people", "People", (flow.people || []).length], ["proposals", "Proposals", proposals.length], ["bricks", "Bricks", (flow.bricks || []).length]]);
      let pane;
      const graphs = { approved: flow.graph || null, draft: draft ? draft.graph : null };
      if (!state.traceSource || !graphs[state.traceSource]) state.traceSource = graphs.approved ? "approved" : graphs.draft ? "draft" : "approved";
      const graph = graphs[state.traceSource];
      const diff = state.traceSource === "draft" && draft ? draft.diff : null;
      if (state.filter === "zones") pane = list((flow.stages || []).map((stage) => { const zone = data.zones.find((row) => row.id === stage); return itemRow({ type: "zone", id: stage }, stage, zone ? zone.description || "" : "not declared", zone ? pill(zone.criticality) : null); }), "The flow lists no zones among its stages.");
      else if (state.filter === "context") pane = [h("div", { class: "fx-acts" }, contextAdd(flow)), list((flow.context || []).map((entry) => itemRow({ type: "context", id: entry.id, flow: flow.id }, entry.title, plain(entry.value).slice(0, 90), pill(entry.kind, "accent"))), "No context yet: queries, metrics, latency budgets, files, notes, and links agents should know.")];
      else if (state.filter === "tests") {
        const tests = flow.tests || [];
        pane = [note(tests.filter((test) => test.required).length + " of " + tests.length + " required in every release. Agents propose tests with their assumptions and mock data; a person approves each one."),
          list(tests.map((test) => itemRow({ type: "test", id: test.id, flow: flow.id }, test.name, test.path + (test.exists ? "" : " · missing") + (test.last_run ? " · ran " + dt(test.last_run) : ""), [test.required ? pill("required", "accent") : null, test.last_result ? pill(test.last_result, test.last_result === "PASSED" ? "ok" : test.last_result === "FAILED" ? "bad" : "") : pill("not run")])), "No tests yet.")];
      }
      else if (state.filter === "agents") pane = [
        h("div", { class: "fx-section" }, h("h3", {}, "Subagents"), list(watchers.map((watcher) => { const status = statusOf(watcher); return itemRow({ type: "watcher", id: watcher.watcher_id }, watcher.purpose, subtypeOf(watcher) + " · " + (watcher.created_by === "agent" ? "created by an agent" : "created by " + watcher.created_by) + " · " + watcher.watcher_id, [pill(status.replace("_", " ").toLowerCase(), status === "ACTIVE" ? "ok" : status === "PENDING_APPROVAL" ? "accent" : status === "TERMINATED" ? "bad" : ""), ...(watcher.firing || []).map((code) => pill(code, "bad"))]); }), "No subagent yet. Add one below, or let an agent ask for one ('crane-request watch --flow " + flow.id + " --type monitor').")),
        h("div", { class: "fx-section" }, h("h3", {}, "Agents assigned"), list(agents.map((assignment, index) => itemRow({ type: "assignment", id: index, key: "flow:" + flow.id }, assignment.agent, (assignment.targets || []).join(", "), chip(assignment.runtime_status || assignment.assignment_status))), "No agent is assigned to this flow (assign one on the Bricks page).")),
      ];
      else if (state.filter === "trace") pane = [
        h("div", { class: "fx-trace-bar" },
          graphs.approved && graphs.draft ? h("div", { class: "fx-seg", role: "group", "aria-label": "Which trace" }, [["approved", "Approved r" + flow.trace.revision], ["draft", "Draft r" + draft.current]].map(([id, label]) => h("button", { type: "button", "aria-pressed": String(state.traceSource === id), onclick: () => { state.traceSource = id; state.peek = null; drawInspector(); } }, label))) : null,
          flow.trace ? h("span", { class: "fx-note" }, flow.trace.nodes + " functions · " + flow.trace.edges + " calls · " + flow.trace.files.length + " files · " + flow.trace.status) : null,
          h("span", { class: "fx-spacer" }),
          draft ? null : h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-retrace", "Trace " + flow.id + " again", { params: { flow: flow.id }, effect: "Starts a draft revision from its approved starting points. A watcher with the trace_drift check tells you when the code moved." }) }, "Retrace…")),
        graph ? traceView(graph, diff) : note(flow.trace ? "This flow was approved before Crane kept call graphs. Retrace it and approve the draft to keep its functions and calls here." : "No trace yet. Retrace the flow from its starting points to see its functions and calls."),
        state.traceSource === "draft" && draft ? draftActions(draft) : null,
      ];
      else if (state.filter === "people") pane = [
        peopleConnections(),
        h("div", { class: "fx-acts" }, personAdd(flow)),
        list((flow.people || []).map((person) => itemRow({ type: "person", id: person.id, flow: flow.id }, person.name, [person.github ? "@" + person.github : "", person.jira || "", person.slack || ""].filter(Boolean).join(" · ") || "from " + person.source, [pill(person.role), ...reach(person).map(([name, ok]) => pill(name + (ok ? " ✓" : " ✗"), ok ? "ok" : "bad", ok ? name + " is connected and this person has an id there" : name + " id set, but " + name + " is not connected"))])), "Nobody is designated. 'crane flow people suggest " + flow.id + "' lists the Git authors of its files."),
      ];
      else if (state.filter === "proposals") pane = list(proposals.map((proposal) => itemRow({ type: "proposal", id: proposal.proposal_id }, proposalTitle(proposal), "by " + ((proposal.proposed_by || {}).agent || "an agent") + " · " + dt(proposal.proposed_at), pill(proposal.kind))), "No proposals from agents are waiting for this flow.");
      else if (state.filter === "bricks") pane = bricksList(flow.bricks || []);
      else pane = overviewPane(flow, watchers, agents);
      let peek = state.peek ? sharedPeek(state.peek, { graph, diff }) : null;
      if (!peek && state.filter === "overview") peek = [peekHead("At a glance", flow.id), field("Monitored by", monitorsOf("flow:" + flow.id, flow.id).join(", ") || "nobody"), field("Zones", (flow.stages || []).join(" → ")), field("Required tests", String((flow.tests || []).filter((test) => test.required).length)), field("Incidents", String(flow.incidents || 0))];
      return { head, chips: nav, pane, peek };
    }

    function overviewPane(flow) {
      const routed = flow.routed || {};
      return [
        h("div", { class: "fx-grid" }, field("Description", flow.description), field("Starts at", (flow.entry_points || []).join(", ")), field("Criticality · ceiling", flow.criticality + " · " + flow.ceiling), field("Files covered", empty(flow.files) ? "" : String(flow.files)), field("Owner", flow.owner), field("Deepest call chain", String(flow.max_depth || ""))),
        h("div", { class: "fx-section" }, h("h3", {}, "Priority and model", h("span", { class: "fx-spacer" }), h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-priority", "Priority of " + flow.id, { params: { flow: flow.id }, effect: "The priority routes agents' work to the organization's model tiers. Enforced, a change from a model outside them needs a person.", fields: [{ key: "priority", label: "Priority", kind: "select", value: flow.priority || "none", options: [["P0", "P0 · critical"], ["P1", "P1 · high"], ["P2", "P2 · normal"], ["P3", "P3 · low"], ["none", "No priority"]] }, { key: "enforce_model", label: "Model routing", kind: "select", value: flow.enforce_model ? "on" : "off", options: [["off", "Guidance only"], ["on", "Enforced: other models need a person"]] }] }) }, "Change…")),
          h("div", { class: "fx-grid" }, field("Priority", flow.priority ? pill(flow.priority, tone(flow.priority)) : ""), field("Routed to", routed.model ? routed.model + " (" + routed.provider + ")" : flow.priority ? "no available model of its tiers" : ""), field("Tiers", (routed.tiers || []).join(", ")), field("Routing", flow.enforce_model ? "enforced" : "guidance only"))),
        requestList((row) => row.params && row.params.flow === flow.id),
      ];
    }

    function requestList(filter) {
      const rows = data.requests.map((row, index) => [row, index]).filter(([row]) => filter(row)).slice(0, 12);
      if (!rows.length) return null;
      return h("div", { class: "fx-section" }, h("h3", {}, "Requests made here"), h("div", { class: "fx-items" }, rows.map(([row, index]) => itemRow({ type: "request", id: index }, requestTitle(row), "@" + row.requested_by + " · " + dt(row.requested_at), chip(requestStatus(row))))));
    }
    const requestTitle = (row) => row.op.replace(/^flow-/, "").replace(/-/g, " ") + " " + ((row.params && (row.params.flow || row.params.proposal || row.params.watcher)) || "");
    const requestStatus = (row) => (row.status === "PENDING" && row.expires_at && row.expires_at < Date.now() ? "EXPIRED" : row.status);
    const proposalTitle = (proposal) => (proposal.kind === "test" ? plain((proposal.payload || {}).name) : plain((proposal.payload || {}).title)) || proposal.proposal_id;

    function bricksList(refs) {
      if (!refs.length) return note("No Brick is attached. Attach one on the Bricks page.");
      return h("div", { class: "fx-items" }, refs.map((ref) => {
        const brick = data.bricks.find((row) => "brick:" + row.ref === ref || row.ref === ref.replace(/^brick:/, ""));
        return itemRow({ type: "brick", id: ref }, brick ? brick.name : ref.replace(/^(brick|stack):/, ""), (brick && brick.intent) || ref, [brick ? pill(brick.type) : pill(ref.startsWith("stack:") ? "stack" : "brick"), brick ? chip(brick.state) : null]);
      }));
    }

    function contextAdd(flow) {
      const kinds = [["query", "Query"], ["metric", "Metric"], ["latency", "Latency"], ["file", "File"], ["note", "Note"], ["link", "Link"]];
      return h("button", { class: "fx-btn small primary", type: "button", onclick: () => request("flow-context-add", "Add context to " + flow.id, { params: { flow: flow.id }, effect: "Guidance agents receive when they work in this flow. It never authorizes anything; secrets are refused.", fields: [{ key: "kind", label: "Kind", kind: "select", value: "note", options: kinds }, { key: "title", label: "Title", required: true, max: 200 }, { key: "value", label: "Value (the query, number, note, or address)", kind: "textarea" }, { key: "unit", label: "Unit (ms for latency)", max: 20 }, { key: "threshold", label: "Keep within (a budget or limit)", kind: "number" }, { key: "file", label: "Repository file (optional)", hint: "app/pay.py" }, { key: "lines", label: "Lines (A-B)", max: 20 }] }) }, "Add context…");
    }

    /** Which services incidents can reach people through, as the reporting machine has them */
    function peopleConnections() {
      const names = [["slack", "Slack"], ["jira", "Jira"], ["github", "GitHub"], ["whatsapp", "WhatsApp"]];
      return h("div", { class: "fx-section" },
        h("div", { class: "fx-conn" }, names.map(([key, label]) => schemaOf(key)
          ? h("button", { class: "fx-btn small" + (connections[key] ? "" : " ghost"), type: "button", title: connections[key] ? "Change the " + label + " connection" : "Connect " + label, onclick: () => connectDialog(key) }, pill(label + (connections[key] ? " · connected" : " · not connected"), connections[key] ? "ok" : ""))
          : pill(label + (connections[key] ? " · connected" : " · not connected"), connections[key] ? "ok" : "", connections[key] ? label + " is connected on the reporting machine" : "Run 'crane integrate " + key + "' to connect " + label))),
        data.connections ? null : note("The reporting machine runs a Crane that does not report its connections; update Crane there."));
    }
    /** The services a person has an id in, each with whether that service is connected */
    const reach = (person) => [["Slack", person.slack, "slack"], ["Jira", person.jira, "jira"], ["GitHub", person.github, "github"]].filter(([, id]) => id).map(([name, , key]) => [name, Boolean(connections[key])]);

    function personAdd(flow) {
      return h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-person-add", "Designate a person on " + flow.id, { params: { flow: flow.id }, effect: "They are told when a watcher raises an incident (Slack, and the Jira issue that names them).", fields: [{ key: "name", label: "Name", required: true, max: 120 }, { key: "role", label: "Role", kind: "select", value: "owner", options: [["author", "Author (wrote the code)"], ["owner", "Owner"], ["reviewer", "Reviewer"], ["oncall", "On call"]] }, { key: "github", label: "GitHub login" }, { key: "jira", label: "Jira account or issue (PAY-12)" }, { key: "slack", label: "Slack member id (U…)" }] }) }, "Designate…");
    }

    // ---------------------------------------------------------------- the trace: call stack and waterfall

    /** The whole flow from its first call to its last: the call stack (function names, indented by
     * depth) on the left, and the waterfall (one bar per call, spanning the calls under it) on the
     * right. It is the static call order the trace follows, not timing. */
    function traceView(graph, diff) {
      if (!graph || !(graph.nodes || []).length) return note('The trace found no functions.');
      const rows = callRows(graph);
      const total = Math.max(1, rows.length);
      const deepest = Math.max(...rows.map((row) => row.depth)) + 1;
      const added = new Set(((diff || {}).added_nodes) || []);
      const files = [...new Set((graph.nodes || []).map((entry) => entry.path))];
      const shade = (path) => FILE_TONES[files.indexOf(path) % FILE_TONES.length];
      const pickNode = (row) => { state.peek = { type: 'function', id: row.node.id, row: row.start }; drawInspector(); };
      const isOn = (row) => state.peek && state.peek.type === 'function' && state.peek.row === row.start;
      const onKey = (row) => (event) => { if (event.key === 'Enter' || event.key === ' ') { event.preventDefault(); pickNode(row); } };
      const ticks = [0, 0.25, 0.5, 0.75, 1].map((at) => h('span', { style: { left: at * 100 + '%' } }, String(Math.round(at * total))));
      return h('div', { class: 'fx-section' },
        h('div', { class: 'fx-trace-bar' }, h('span', { class: 'fx-note' }, rows.length + ' calls through ' + files.length + ' file(s), ' + deepest + ' level(s) deep, from ' + ((graph.entry_points || []).join(', ') || 'its start') + ' to its last call. Click a call for where it is and what it calls.'), h('span', { class: 'fx-spacer' }), diff ? h('span', { class: 'fx-note' }, '+' + diff.added_nodes.length + ' / −' + diff.removed_nodes.length + ' functions since the revision before (green edge)') : null),
        h('div', { class: 'fx-trace', role: 'table', 'aria-label': 'Call stack and waterfall of the flow' },
          h('div', { class: 'fx-trace-head', role: 'row' }, h('span', { role: 'columnheader' }, 'Call stack'), h('div', { class: 'fx-ticks', role: 'columnheader', 'aria-label': 'Waterfall: calls in order' }, ticks)),
          h('div', { class: 'fx-trace-body' }, rows.map((row) => h('div', { class: 'fx-trace-row' + (isOn(row) ? ' on' : '') + (added.has(row.node.id) ? ' added' : ''), role: 'row', tabindex: '0', title: row.node.symbol + ' — ' + row.node.path + ':' + row.node.start_line + '-' + row.node.end_line + (row.again ? ' (reached again; shown once above)' : ''), onclick: () => pickNode(row), onkeydown: onKey(row) },
            h('div', { class: 'fx-stack-name', role: 'cell', style: { paddingLeft: 8 + row.depth * 14 + 'px' } },
              row.depth ? h('span', { class: 'fx-guide' }, '└ ') : null,
              h('i', { class: 'fx-swatch', style: { background: shade(row.node.path) } }),
              h('span', { class: 'sym' }, row.node.symbol), row.again ? h('span', { class: 'again' }, ' ↺') : null,
              h('span', { class: 'loc' }, row.node.path.split('/').pop() + ':' + row.node.start_line)),
            h('div', { class: 'fx-wf-track', role: 'cell' }, h('div', { class: 'fx-bar' + (isOn(row) ? ' on' : ''), style: { left: (row.start / total) * 100 + '%', width: 'calc(' + (row.size / total) * 100 + '% - 1px)', background: 'color-mix(in srgb, ' + shade(row.node.path) + ' ' + (row.again ? 16 : 34) + '%, var(--panel))', borderColor: shade(row.node.path) } }, row.size > 1 ? row.size + ' calls' : '')))))),
        h('div', { class: 'fx-axis' }, h('span', {}, 'first call'), h('span', {}, 'static call order, not timing · colors are files'), h('span', {}, rows.length + (graph.truncated || rows.length >= 400 ? '+ calls' : ' calls'))),
        (graph.unresolved || []).length ? h('details', {}, h('summary', { class: 'fx-note' }, graph.unresolved.length + ' call(s) not followed (external, dynamic, or ambiguous)'), h('div', { class: 'fx-code' }, graph.unresolved.join('\n'))) : null);
    }

    function draftFacts(draft) {
      const current = draft.revisions.find((revision) => revision.revision === draft.current) || {};
      return h("div", { class: "fx-grid" }, field("Starts at", (current.from || []).join(", ")), field("Status", current.status), field("Functions · calls · files", [current.nodes, current.edges, current.files].join(" · ")), field("Digest", h("code", {}, String(current.digest || "").slice(0, 16))), field("Traced", current.traced_at ? dt(current.traced_at) + " by " + current.by : ""), field("Description", shared(draft.description)));
    }
    function revisions(draft) {
      return h("div", { class: "fx-items" }, draft.revisions.slice().reverse().map((revision) => h("div", { class: "fx-item", style: "cursor:default" },
        h("span", { class: "t" }, "r" + revision.revision + (revision.revision === draft.current ? " · under review" : "")), h("span", { class: "m" }, revision.nodes + " functions · " + revision.edges + " calls · " + revision.status + " · " + dt(revision.traced_at) + " · " + (revision.reason || "")),
        h("span", { class: "r" }, revision.revision === draft.current ? null : h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-rewind", "Review revision " + revision.revision + " of " + draft.flow_id, { params: { flow: draft.flow_id, revision: revision.revision }, effect: "Makes this revision the one under review. Later revisions are kept." }) }, "Rewind here…")))));
    }
    function draftActions(draft) {
      return h("div", { class: "fx-acts" },
        h("button", { class: "fx-btn primary", type: "button", onclick: () => request("flow-approve", "Approve " + draft.flow_id, { params: { flow: draft.flow_id }, effect: draft.exists ? "Records this trace in the flow's signed knowledge and keeps its call graph." : "Adds the flow to .crane/governance.yaml with a context pack written from its trace, and records the trace in the signed knowledge.", fields: draft.exists ? [] : [{ key: "proposed", label: "Add it as proposed (not enforced yet)", kind: "check" }] }) }, "Approve…"),
        h("button", { class: "fx-btn", type: "button", onclick: () => request("flow-retrace", "Trace " + draft.flow_id + " again", { params: { flow: draft.flow_id }, effect: "A new revision from the same starting points, after the code changed." }) }, "Retrace…"),
        h("button", { class: "fx-btn danger", type: "button", onclick: () => request("flow-discard", "Discard the draft of " + draft.flow_id, { params: { flow: draft.flow_id } }) }, "Discard…"));
    }

    // ---------------------------------------------------------------- the detail pane

    /** The smaller view of the item clicked: a zone, flow, test, context item, person, agent,
     * function, proposal, incident, request, or Brick */
    function sharedPeek(peek, extra) {
      const flowOf = (id) => data.flows.find((flow) => flow.id === id);
      if (peek.type === "zone") {
        const zone = data.zones.find((row) => row.id === peek.id);
        if (!zone) return [peekHead("Zone", peek.id), note("This zone is not declared in governance.yaml.")];
        return [peekHead("Zone", zone.id, pill(zone.criticality), pill(zone.status)), field("Description", zone.description), field("Autonomy ceiling", zone.ceiling), field("Files covered", empty(zone.files) ? "" : String(zone.files)), field("Owner", zone.owner), field("Bricks", (zone.bricks || []).join(", ")), field("Agents", assignedTo("zone:" + zone.id).map((row) => row.agent).join(", ")),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small", type: "button", onclick: () => pick("zone:" + zone.id) }, "Open zone"))];
      }
      if (peek.type === "flow") {
        const flow = flowOf(peek.id);
        if (!flow) return null;
        return [peekHead("Flow", flow.id, flow.priority ? pill(flow.priority, tone(flow.priority)) : null, pill(flow.status)), field("Description", flow.description), field("Stages", (flow.stages || []).join(" → ")), field("Monitored by", monitorsOf("flow:" + flow.id, flow.id).join(", ") || "nobody"), field("Tests", (flow.tests || []).length + " (" + (flow.tests || []).filter((test) => test.required).length + " required)"),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small", type: "button", onclick: () => pick("flow:" + flow.id) }, "Open flow"))];
      }
      if (peek.type === "test") {
        const flow = flowOf(peek.flow), test = flow && (flow.tests || []).find((row) => row.id === peek.id);
        if (!test) return null;
        return [peekHead("Test", test.name, test.required ? pill("required", "accent") : null, test.last_result ? pill(test.last_result, test.last_result === "PASSED" ? "ok" : test.last_result === "FAILED" ? "bad" : "") : pill("not run")),
          field("File", h("code", {}, test.path + (test.exists ? "" : " (missing)"))), field("Last run", test.last_run ? dt(test.last_run) : ""), field("What it checks", shared(test.description)), field("Assumes", (test.assumptions || []).map(plain).join("; ")), field("Mock data", shared(test.mock_data)), field("Command", test.command ? h("code", {}, plain(test.command)) : ""), field("Written · approved", test.proposed_by + " · " + test.approved_by + " · " + dt(test.approved_at)),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-test-require", (test.required ? "Stop requiring " : "Require ") + test.name, { params: { flow: flow.id, test: test.id, required: !test.required }, effect: test.required ? "Releases no longer fail on it." : "'crane flow tests run --required' fails while it fails, so a release step stops." }) }, test.required ? "Not required…" : "Require…"))];
      }
      if (peek.type === "context") {
        const flow = flowOf(peek.flow), entry = flow && (flow.context || []).find((row) => row.id === peek.id);
        if (!entry) return null;
        return [peekHead("Context · " + entry.kind, entry.title), h("div", { class: "fx-code" }, shared(entry.value), entry.unit ? " " + entry.unit : ""), field("Keep within", empty(entry.threshold) ? "" : String(entry.threshold) + (entry.unit ? " " + entry.unit : "")), field("File", entry.file ? h("code", {}, entry.file + (entry.lines ? ":" + entry.lines : "")) : ""), field("Added", entry.added_by + " · " + dt(entry.added_at)),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-context-remove", "Remove " + entry.title, { params: { flow: flow.id, item: entry.id } }) }, "Remove…"))];
      }
      if (peek.type === "person") {
        const flow = flowOf(peek.flow), person = flow && (flow.people || []).find((row) => row.id === peek.id);
        if (!person) return null;
        const linked = (name, value) => (value ? h("span", {}, value, " ", connections[name] ? pill("connected", "ok") : pill("not connected", "bad", "Run 'crane integrate " + name + "' on the reporting machine")) : "");
        return [peekHead("Person · " + person.role, person.name), field("GitHub", linked("github", person.github ? "@" + person.github : "")), field("Jira", linked("jira", person.jira)), field("Slack", linked("slack", person.slack)), field("Source", person.source),
          note(connections.slack || connections.jira ? "Incidents reach this person through the connected services." : "Neither Slack nor Jira is connected on the reporting machine: incidents cannot reach this person yet."),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-person-remove", "Remove " + person.name, { params: { flow: flow.id, person: person.id } }) }, "Remove…"))];
      }
      if (peek.type === "watcher") {
        const watcher = data.watchers.find((row) => row.watcher_id === peek.id);
        if (!watcher) return null;
        const status = watcher.status || (watcher.active ? "ACTIVE" : "STOPPED");
        return [peekHead((watcher.subtype || "monitor") + " subagent", watcher.purpose, pill(status.replace("_", " ").toLowerCase(), status === "ACTIVE" ? "ok" : status === "PENDING_APPROVAL" ? "accent" : ""), ...(watcher.firing || []).map((code) => pill(code, "bad"))),
          field("Created by", watcher.created_by === "agent" ? "an agent" : watcher.created_by), field("Ceiling", ((watcher.config || {}).ceiling) || (watcher.inherited || {}).ceiling), field("Actions", ((watcher.config || {}).actions || []).join(", ")), field("Last ran", watcher.last_run ? dt(watcher.last_run) : "not yet"),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small", type: "button", onclick: () => subagentInfo(watcher) }, "Properties and configuration…"))];
      }
      if (peek.type === "assignment") {
        const assignment = assignedTo(peek.key)[peek.id];
        if (!assignment) return null;
        return [peekHead("Agent", assignment.agent), field("Assignment", chip(assignment.assignment_status)), field("Runtime", chip(assignment.runtime_status)), field("Targets", (assignment.targets || []).join(", ")), note("Assigned on the Bricks page; it acts only inside its signed Brick assignment.")];
      }
      if (peek.type === "function") {
        const graph = (extra && extra.graph) || { nodes: [], edges: [] };
        const entry = (graph.nodes || []).find((row) => row.id === peek.id);
        if (!entry) return null;
        const callers = (graph.edges || []).filter((edge) => edge.to === entry.id);
        const callees = (graph.edges || []).filter((edge) => edge.from === entry.id);
        const name = (id) => { const found = (graph.nodes || []).find((row) => row.id === id); return found ? found.symbol : id; };
        const added = extra && extra.diff && (extra.diff.added_nodes || []).includes(entry.id);
        return [peekHead("Function", entry.symbol, pill("depth " + entry.depth), added ? pill("new in this revision", "ok") : null, (graph.entry_points || []).includes(entry.id) ? pill("starting point", "accent") : null),
          field("Where", h("code", {}, entry.path + ":" + entry.start_line + "-" + entry.end_line)),
          field("Called by", callers.length ? h("div", { class: "fx-items" }, callers.map((edge) => h("span", {}, h("code", {}, name(edge.from)), h("span", { class: "muted" }, " · " + edge.how)))) : "a starting point"),
          field("Calls", callees.length ? h("div", { class: "fx-items" }, callees.map((edge) => h("span", {}, h("code", {}, name(edge.to)), h("span", { class: "muted" }, " · " + edge.how)))) : "nothing it traced")];
      }
      if (peek.type === "proposal") {
        const proposal = data.proposals.find((row) => row.proposal_id === peek.id);
        if (!proposal) return null;
        const payload = proposal.payload || {}, test = proposal.kind === "test";
        return [peekHead("Proposal · " + proposal.kind, proposalTitle(proposal), pill(proposal.status)), field("Flow", proposal.flow),
          test ? field("File", h("code", {}, plain(payload.path))) : field("Kind", plain(payload.kind)), field(test ? "What it checks" : "Value", test ? shared(payload.description) : shared(payload.value)),
          test && (payload.assumptions || []).length ? field("Assumes", payload.assumptions.map(plain).join("; ")) : null, test && plain(payload.mock_data) ? field("Mock data", shared(payload.mock_data)) : null,
          field("Proposed by", ((proposal.proposed_by || {}).agent || "an agent") + " · session " + ((proposal.proposed_by || {}).session || "?") + " · " + dt(proposal.proposed_at)),
          proposal.status === "PENDING" ? h("div", { class: "fx-acts" },
            h("button", { class: "fx-btn small primary", type: "button", onclick: () => request("flow-proposal-approve", "Approve " + proposal.proposal_id, { params: { proposal: proposal.proposal_id }, effect: test ? "Attaches the test to the flow's signed knowledge." : "Adds the context to the flow's signed knowledge.", fields: test ? [{ key: "required", label: "Require it in every release ('crane flow tests run --required')", kind: "check" }] : [] }) }, "Approve…"),
            h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-proposal-reject", "Reject " + proposal.proposal_id, { params: { proposal: proposal.proposal_id }, effect: "The agent's session is told why with its next tool call." }) }, "Reject…")) : null];
      }
      if (peek.type === "incident") {
        const incident = data.incidents.find((row) => row.incident_id === peek.id);
        if (!incident) return null;
        return [peekHead("Incident", incident.summary, pill(incident.code, "bad")), field("Flow", h("button", { class: "fx-link", type: "button", onclick: () => pick("flow:" + incident.flow) }, incident.flow)), field("Raised by", incident.raised_by + (incident.watcher_id ? " (" + incident.watcher_id + ")" : "")), field("When", dt(incident.raised_at)), field("Told", (incident.notified || []).join(", ") || "nobody yet"), incident.detail ? h("div", { class: "fx-code" }, incident.detail) : null, field("Id", h("code", {}, incident.incident_id))];
      }
      if (peek.type === "request") {
        const row = data.requests[peek.id];
        if (!row) return null;
        return [peekHead("Request", requestTitle(row), chip(requestStatus(row))), field("Asked by", "@" + row.requested_by + " · " + dt(row.requested_at)), field("Why", row.reason), field("Detail", row.detail), h("div", { class: "fx-code" }, JSON.stringify(row.params || {}, null, 2))];
      }
      if (peek.type === "brick") {
        const brick = data.bricks.find((row) => "brick:" + row.ref === peek.id || row.ref === peek.id.replace(/^brick:/, ""));
        return [peekHead("Brick", brick ? brick.name : peek.id), field("Type", brick ? brick.type : ""), field("State", brick ? chip(brick.state) : ""), field("Intent", brick ? brick.intent : ""), field("Reference", h("code", {}, peek.id))];
      }
      return null;
    }

    // ---------------------------------------------------------------- subagents

    const catalog = data.subagent_catalog || { subtypes: ["monitor", "incident"], max_per_flow: 3, max_agent_per_flow: 1, actions: [], ceilings: ["observe", "assisted", "delegated", "autonomous"] };
    const connections = data.connections || {};
    const statusOf = (watcher) => watcher.status || (watcher.active ? "ACTIVE" : "STOPPED");
    const holdsPlace = (watcher) => ["ACTIVE", "PENDING_APPROVAL"].includes(statusOf(watcher));
    const subtypeOf = (watcher) => watcher.subtype || "monitor";
    const actionSpec = (subtype, id) => (catalog.actions || []).find((action) => action.subtype === subtype && action.id === id);
    const INFO = "M12 3a9 9 0 1 0 0 18 9 9 0 0 0 0-18zM12 11v5M12 8h.01";

    /** A flow's bounds as the report states them: its ceiling and the targets a subagent may have */
    function boundsOf(flowId) {
      const flow = data.flows.find((row) => row.id === flowId);
      return { ceiling: flow ? flow.ceiling : "observe", targets: flow ? ["flow:" + flow.id, ...(flow.stages || []).map((stage) => "zone:" + stage)] : [] };
    }
    /** Merge a JSON patch into a configuration, as Crane does: given fields replace, null resets,
     * parameters merge key by key, unknown fields are refused */
    function mergeConfig(base, patch) {
      if (!patch || typeof patch !== "object" || Array.isArray(patch)) throw new Error("The configuration must be a JSON object.");
      const known = ["actions", "context", "parameters", "integrations", "branch", "ceiling", "targets"];
      const unknown = Object.keys(patch).find((key) => !known.includes(key));
      if (unknown) throw new Error("Unknown configuration field '" + unknown + "' (use " + known.join(", ") + ").");
      const merged = JSON.parse(JSON.stringify(base || {}));
      for (const [key, value] of Object.entries(patch)) {
        if (key === "parameters" && value && typeof value === "object" && !Array.isArray(value)) {
          merged.parameters = merged.parameters || {};
          for (const [name, item] of Object.entries(value)) { if (item === null) delete merged.parameters[name]; else merged.parameters[name] = item; }
        } else if (value === null) delete merged[key];
        else merged[key] = value;
      }
      return merged;
    }
    /** Every way a configuration asks for more than its flow allows (empty when within bounds) */
    function boundsProblems(subtype, config, flowId) {
      const bounds = boundsOf(flowId);
      const order = catalog.ceilings || ["observe", "assisted", "delegated", "autonomous"];
      const rank = (name) => order.indexOf(name);
      const problems = [];
      const ceiling = config.ceiling || bounds.ceiling;
      if (rank(ceiling) < 0) problems.push("ceiling '" + ceiling + "' is one of " + order.join(", "));
      else if (rank(ceiling) > rank(bounds.ceiling)) problems.push("its ceiling " + ceiling + " is above the flow's ceiling " + bounds.ceiling);
      const allowed = Math.min(rank(ceiling) < 0 ? rank(bounds.ceiling) : rank(ceiling), rank(bounds.ceiling));
      for (const id of config.actions || []) {
        const spec = actionSpec(subtype, id);
        if (!spec) problems.push("action '" + id + "' is not an action of a " + subtype + " subagent");
        else if (rank(spec.ceiling) > allowed) problems.push("action '" + id + "' needs the " + spec.ceiling + " ceiling, more than the " + order[allowed] + " it may have");
      }
      if (!(config.actions || []).length) problems.push("give it at least one action");
      for (const target of config.targets || []) if (!bounds.targets.includes(target)) problems.push("target '" + target + "' is outside the flow (it may act on " + bounds.targets.join(", ") + ")");
      if (subtype === "monitor" && config.branch) problems.push("only an Incident subagent has a designated branch");
      return problems;
    }
    const RESOLVE = "A subagent stays within its flow. To give it more, raise the flow's autonomy ceiling (or add the zone to the flow) in .crane/governance.yaml first, then configure the subagent again.";
    /** A modal error that lists the problems and how to resolve them */
    function boundsError(problems) {
      const close = () => back.remove();
      const back = h("div", { class: "fx-dialog", role: "alertdialog", "aria-modal": "true", "aria-label": "Outside the flow's bounds", onclick: (event) => { if (event.target === back) close(); }, onkeydown: (event) => { if (event.key === "Escape") close(); } },
        h("form", { onsubmit: (event) => { event.preventDefault(); close(); } },
          h("div", { class: "fx-kind", style: "color:var(--bad)" }, "Not within bounds"),
          h("h2", {}, "The subagent asks for more than its flow allows"),
          h("ul", { class: "fx-errors" }, problems.map((problem) => h("li", {}, problem))),
          note(RESOLVE),
          h("div", { class: "acts" }, h("button", { class: "fx-btn primary", type: "submit" }, "Got it"))));
      document.body.append(back);
      back.querySelector("button").focus();
    }

    // ---------------------------------------------------------------- connections

    const schemas = data.integration_schemas || [];
    const integrations = data.integrations || {};
    const schemaOf = (name) => schemas.find((schema) => schema.name === name);
    /** Every address in a value (strings that start with http:// or https://) */
    function urlsIn(value) {
      const found = new Set();
      const walk = (item) => {
        if (typeof item === "string" && /^https?:\/\//.test(item.trim())) found.add(item.trim());
        else if (Array.isArray(item)) item.forEach(walk);
        else if (item && typeof item === "object") Object.values(item).forEach(walk);
      };
      walk(value);
      return [...found];
    }
    /** Ask DataSea whether an address points to a service ({ok, private, message}) */
    async function probeUrl(url) {
      try { return await apiWrite("/api/flows", { action: "probe", url }); } catch (error) { return { ok: false, private: false, message: error.message }; }
    }
    /** Check every address; returns the ones that failed, the private ones (checked by the machine), and the ones that answered */
    async function checkUrls(urls) {
      const results = await Promise.all(urls.map((url) => probeUrl(url).then((answer) => Object.assign({ url }, answer))));
      return { failed: results.filter((row) => !row.ok), later: results.filter((row) => row.ok && row.private), answered: results.filter((row) => row.ok && !row.private) };
    }
    /** A modal that lists addresses that do not point to a service */
    function urlError(failed) {
      const close = () => back.remove();
      const back = h("div", { class: "fx-dialog", role: "alertdialog", "aria-modal": "true", "aria-label": "Addresses that do not answer", onclick: (event) => { if (event.target === back) close(); }, onkeydown: (event) => { if (event.key === "Escape") close(); } },
        h("form", { onsubmit: (event) => { event.preventDefault(); close(); } },
          h("div", { class: "fx-kind", style: "color:var(--bad)" }, "Not kept"),
          h("h2", {}, "An address does not point to a service"),
          h("ul", { class: "fx-errors" }, failed.map((row) => h("li", {}, row.message))),
          note("Nothing is attached until every address answers. Fix the address, or check that the service is up."),
          h("div", { class: "acts" }, h("button", { class: "fx-btn primary", type: "submit" }, "Got it"))));
      document.body.append(back);
      back.querySelector("button").focus();
    }

    /** Connect (or change) an integration: its non-secret settings, each address checked before it
     * is sent; the machine checks the addresses again and keeps the settings, and credentials are
     * entered there (Landscape asks for them when it applies the request) */
    function connectDialog(name) {
      const schema = schemaOf(name);
      if (!schema) { boundsError(["The reporting machine runs a Crane that cannot set " + name + " from DataSea; update Crane there."]); return; }
      const current = (integrations[name] || {}).settings || {};
      const values = {};
      for (const setting of schema.settings) values[setting.key] = current[setting.key] !== undefined ? String(current[setting.key]) : (setting.default || "");
      const applies = (when) => !when || when.values.includes(values[when.key]);
      const rows = h("div", { class: "fx-section" });
      const secrets = h("div");
      const error = h("div", { class: "error", role: "alert" });
      const reason = h("input", { type: "text", maxlength: "300", placeholder: "Recorded with the change", "aria-label": "Why" });
      const submit = h("button", { class: "fx-btn primary", type: "submit" }, "Check and continue with GitHub");
      const draw = () => {
        put(rows, schema.settings.filter((setting) => applies(setting.when)).map((setting) => {
          const verdict = h("span", { class: "fx-verdict" });
          let control;
          if (setting.kind === "choice") control = h("select", { onchange: (event) => { values[setting.key] = event.target.value; draw(); } }, setting.choices.map((choice) => h("option", { value: choice, selected: values[setting.key] === choice ? "" : undefined }, choice)));
          else control = h("input", { type: setting.kind === "url" ? "url" : "text", value: values[setting.key], placeholder: setting.default || "", spellcheck: "false", oninput: (event) => { values[setting.key] = event.target.value.trim(); put(verdict); } });
          const probe = setting.kind === "url" ? h("button", { class: "fx-btn small", type: "button", onclick: async () => {
            if (!values[setting.key]) { put(verdict, h("span", { class: "bad" }, "Enter the address first.")); return; }
            put(verdict, h("span", { class: "muted" }, "Checking…"));
            const answer = await probeUrl(values[setting.key]);
            put(verdict, h("span", { class: answer.ok ? "ok" : "bad" }, (answer.ok ? (answer.private ? "◌ " : "✓ ") : "✗ ") + answer.message));
          } }, "Check") : null;
          return h("label", {}, setting.label + (setting.required ? "" : " (optional)"), probe ? h("div", { class: "fx-acts", style: "flex-wrap:nowrap" }, control, probe) : control, verdict);
        }));
        const needed = schema.secrets.filter((secret) => !secret.optional && applies(secret.when));
        const missing = (integrations[name] || {}).missing_secrets || [];
        put(secrets, needed.length ? h("div", { class: "fx-note" }, "Credentials are never sent to DataSea. ", h("b", {}, needed.map((secret) => secret.label).join(", ")), (integrations[name] || {}).configured && !missing.length ? " are already on the machine." : " are entered on the machine: Landscape asks for them when it applies this request (or run 'crane integrate " + name + " --secrets-stdin').") : null);
      };
      draw();
      const close = () => back.remove();
      const back = h("div", { class: "fx-dialog", role: "dialog", "aria-modal": "true", "aria-label": "Connect " + schema.label, onclick: (event) => { if (event.target === back) close(); }, onkeydown: (event) => { if (event.key === "Escape") close(); } },
        h("form", { onsubmit: async (event) => {
          event.preventDefault();
          error.textContent = "";
          const settings = {};
          for (const setting of schema.settings) {
            if (!applies(setting.when)) continue;
            if (setting.required && !values[setting.key]) { error.textContent = setting.label + " is required."; return; }
            if (values[setting.key] !== (current[setting.key] === undefined ? "" : String(current[setting.key]))) settings[setting.key] = values[setting.key];
          }
          if (!Object.keys(settings).length) { error.textContent = "Nothing changed."; return; }
          if (!reason.value.trim()) { error.textContent = "Say why; it is recorded with the change."; reason.focus(); return; }
          submit.disabled = true;
          submit.textContent = "Checking the addresses…";
          const { failed } = await checkUrls(urlsIn(settings));
          if (failed.length) { submit.disabled = false; submit.textContent = "Check and continue with GitHub"; urlError(failed); return; }
          try {
            const staged = await apiWrite("/api/flows", { action: "stage", op: "integration-set", params: { name, settings }, reason: reason.value.trim() });
            location.href = staged.continue + "&page=flows";
          } catch (problem) { submit.disabled = false; submit.textContent = "Check and continue with GitHub"; error.textContent = problem.message; }
        } },
          h("div", { class: "fx-kind" }, "Code owners · request"),
          h("h2", {}, ((integrations[name] || {}).configured ? "Change " : "Connect ") + schema.label),
          note(schema.purpose + ". Every address is checked before it is sent, and again by the machine before it keeps it; nothing is attached until it answers."),
          rows, secrets,
          h("label", {}, "Why", reason),
          error,
          note("GitHub confirms you are a code owner of .crane/flows.json (or a repository admin). A person applies it on a machine ('crane brick inbox apply', or one click in Landscape)."),
          h("div", { class: "acts" }, h("button", { class: "fx-btn ghost", type: "button", onclick: close }, "Cancel"), submit)));
      document.body.append(back);
      (back.querySelector("input, select") || reason).focus();
    }

    /** Every integration subagents use, with its status and a way to connect it */
    function connectionsPane() {
      if (!schemas.length) return note("The reporting machine runs a Crane that does not report its connections; update Crane there.");
      return [
        note("Connections subagents use for their actions. Settings are set here; credentials stay on the machine."),
        h("div", { class: "fx-items" }, schemas.map((schema) => {
          const row = integrations[schema.name] || {};
          const tone = row.connected ? "ok" : row.configured ? "accent" : "";
          const label = row.connected ? "connected" : row.configured ? "needs " + (row.missing_secrets || []).join(", ") : "not connected";
          return h("div", { class: "fx-item", style: "cursor:default" },
            h("span", { class: "t" }, schema.label), h("span", { class: "m" }, schema.purpose),
            h("span", { class: "r" }, pill(label, tone), h("button", { class: "fx-btn small", type: "button", onclick: () => connectDialog(schema.name) }, row.configured ? "Change…" : "Connect…")));
        })),
      ];
    }

    /** The full view of a subagent: properties, actions, parameters, context, Crane's governance,
     * runs, and a JSON configuration that is merged, checked against the flow, and requested */
    function subagentInfo(watcher) {
      const subtype = subtypeOf(watcher);
      const status = statusOf(watcher);
      const config = (watcher.pending && watcher.pending.config) || watcher.config || {};
      const flow = data.flows.find((row) => row.id === watcher.flow);
      const bounds = boundsOf(watcher.flow);
      const onFlow = data.watchers.filter((other) => other.flow === watcher.flow && holdsPlace(other));
      const common = flow ? (flow.context || []).length : 0;
      const raised = data.incidents.filter((incident) => incident.watcher_id === watcher.watcher_id);
      const started = watcher.created_by === "agent" ? "agent " + ((watcher.parent || {}).agent || "?") + " (session " + ((watcher.parent || {}).session || "?") + ((watcher.parent || {}).model ? ", " + watcher.parent.model : "") + ")" : "person · " + watcher.created_by;
      const section = (title, ...children) => h("div", { class: "fx-section" }, h("h3", {}, title), ...children);
      const props = (rows) => h("dl", { class: "fx-props" }, rows.filter(Boolean).flatMap(([key, value]) => [h("dt", {}, key), h("dd", {}, empty(value) ? h("span", { class: "muted" }, "—") : value)]));
      const editor = h("textarea", { class: "fx-json", spellcheck: "false", "aria-label": "Configuration (JSON)" }, JSON.stringify(config, null, 2));
      const verdict = h("div", { class: "fx-verdict", role: "status" });
      const check = async () => {
        let patch;
        try { patch = JSON.parse(editor.value); } catch (error) { put(verdict, h("span", { class: "bad" }, "Not valid JSON: " + error.message)); return null; }
        let merged;
        try { merged = mergeConfig(watcher.config || {}, patch); } catch (error) { put(verdict, h("span", { class: "bad" }, error.message)); return null; }
        const problems = boundsProblems(subtype, merged, watcher.flow);
        if (problems.length) { put(verdict, h("span", { class: "bad" }, problems.length + " problem(s): outside the flow's bounds")); boundsError(problems); return null; }
        // Every address it carries must point to a service before it is attached
        const urls = urlsIn(patch);
        if (urls.length) {
          put(verdict, h("span", { class: "muted" }, "Checking " + urls.length + " address(es)…"));
          const { failed, later } = await checkUrls(urls);
          if (failed.length) { put(verdict, h("span", { class: "bad" }, failed.length + " address(es) do not answer")); urlError(failed); return null; }
          put(verdict, h("span", { class: "ok" }, "Within the flow's bounds; " + (urls.length - later.length) + " address(es) answered" + (later.length ? ", " + later.length + " private one(s) are checked by the machine" : "") + "."));
          return patch;
        }
        put(verdict, h("span", { class: "ok" }, "Within the flow's bounds. Crane checks it again on the machine and when it is approved."));
        return patch;
      };
      const close = () => back.remove();
      const back = h("div", { class: "fx-dialog", role: "dialog", "aria-modal": "true", "aria-label": "Subagent " + watcher.watcher_id, onclick: (event) => { if (event.target === back) close(); }, onkeydown: (event) => { if (event.key === "Escape" && document.activeElement !== editor) close(); } },
        h("form", { class: "fx-info", onsubmit: (event) => event.preventDefault() },
          h("div", { class: "fx-card-head" }, h("span", { class: "fx-avatar" }, icon(ROBOT)), h("div", { class: "who" }, h("span", { class: "fx-kind" }, subtype + " subagent"), h("h2", {}, watcher.purpose), h("span", { class: "id" }, watcher.watcher_id)), h("button", { class: "fx-tool", type: "button", "aria-label": "Close", onclick: close }, icon(ICONS.clear))),
          h("div", { class: "fx-acts" }, pill(status.replace("_", " ").toLowerCase(), status === "ACTIVE" ? "ok" : status === "PENDING_APPROVAL" ? "accent" : status === "TERMINATED" ? "bad" : ""), pill(watcher.created_by === "agent" ? "created by an agent" : "created by a person"), ...(watcher.firing || []).map((code) => pill(code, "bad"))),
          section("Properties", props([
            ["Subtype", subtype === "monitor" ? "Monitor: holds the monitoring, reads observability, opens a meeting, pages on-call, activates Incident subagents" : "Incident: reads the Jira ticket, writes RCA and flow context, prepares pull requests and reviews on the designated branch"],
            ["Flow", h("button", { class: "fx-link", type: "button", onclick: () => { close(); pick("flow:" + watcher.flow, { scroll: true }); } }, watcher.flow)],
            ["Started by", started + " · " + dt(watcher.created_at)],
            ["Approved by", watcher.approved_by],
            ["Ceiling", (config.ceiling || bounds.ceiling) + " (flow: " + bounds.ceiling + ")"],
            ["Targets", (config.targets || []).join(", ")],
            subtype === "incident" ? ["Branch", config.branch] : null,
            ["Checks", (watcher.checks || []).join(", ")],
            ["Last ran", watcher.last_run ? dt(watcher.last_run) : "not yet"],
            watcher.ended ? ["Ended", watcher.ended] : null,
          ])),
          section("Actions", (config.actions || []).length ? h("div", { class: "fx-items" }, (config.actions || []).map((id) => {
            const spec = actionSpec(subtype, id) || {};
            const needs = spec.integration ? spec.integration.split("|") : [];
            const linked = needs.some((name) => connections[name]);
            return h("div", { class: "fx-item", style: "cursor:default" }, h("span", { class: "t" }, spec.label || id), h("span", { class: "m" }, id + " · needs " + (spec.ceiling || "?") + (needs.length ? " · " + needs.join(" or ") : "") + (spec.runs === "integration" ? " · calls the service" : " · Crane does it")),
              h("span", { class: "r" }, needs.length ? pill(linked ? "connected" : "not connected", linked ? "ok" : "") : pill("crane"), needs.length && !linked ? h("button", { class: "fx-btn small", type: "button", onclick: () => connectDialog(needs[0]) }, "Connect…") : null));
          })) : note("No actions.")),
          section("Parameters", Object.keys(config.parameters || {}).length ? props(Object.entries(config.parameters).map(([key, value]) => [key, h("code", {}, typeof value === "string" ? value : JSON.stringify(value))])) : note("None. Incident subagents read parameters.jira_issue for the ticket.")),
          section("Context", (config.context || []).length ? h("ul", { class: "fx-errors", style: "color:var(--ink)" }, config.context.map((item) => h("li", {}, item))) : note("No standing context of its own."), note("No volatile context of its own: when it runs, the flow's common context (" + common + " item" + (common === 1 ? "" : "s") + ") is its volatile context.")),
          section("Governance via Crane", props([
            ["Bounds", "Constrained to " + watcher.flow + ": ceiling at most " + bounds.ceiling + ", targets " + bounds.targets.join(", ")],
            ["Places", onFlow.length + " of " + catalog.max_per_flow + " on this flow · " + onFlow.filter((other) => other.created_by === "agent").length + " of " + catalog.max_agent_per_flow + " created by an agent"],
            ["Inherited", (((watcher.inherited || {}).bricks || []).join(", ") || "no Bricks") + " · ceiling " + ((watcher.inherited || {}).ceiling || "—") + " · " + ((watcher.inherited || {}).mode || "read_only")],
            ["Approval", "Creating or configuring waits for a person's approval in Landscape, where the bounds are checked again"],
            ["Beyond the flow", "Only a person's manual trigger ('crane flow subagent trigger … --beyond'), for that run only, recorded"],
            ["Insights", raised.length ? raised.length + " incident(s) raised, last " + dt(Math.max(...raised.map((incident) => incident.raised_at))) : "nothing raised"],
          ])),
          (watcher.runs || []).length ? section("Latest runs", h("div", { class: "fx-items" }, watcher.runs.slice(-5).reverse().map((run) => h("div", { class: "fx-item", style: "cursor:default" }, h("span", { class: "t" }, run.reason || run.by), h("span", { class: "m" }, dt(run.at) + " · by " + run.by + (run.beyond && run.beyond.length ? " · beyond: " + run.beyond.join(", ") : "") + " · " + run.outcomes.map((outcome) => outcome.action + " " + outcome.result).join(", ")))))) : null,
          watcher.pending ? section("Waiting for approval", note(watcher.pending.kind + " requested by " + watcher.pending.requested_by + " · " + dt(watcher.pending.requested_at) + ". A person approves it in Landscape.")) : null,
          holdsPlace(watcher) ? section("Configuration",
            note("Edit the JSON and merge it into the current configuration: fields you give replace theirs, null resets one, parameters merge key by key. Fields: actions, context, parameters, integrations, branch, ceiling, targets."),
            editor, verdict,
            h("div", { class: "fx-acts" },
              h("button", { class: "fx-btn small", type: "button", onclick: check }, "Check bounds"),
              h("button", { class: "fx-btn small primary", type: "button", onclick: async () => { const patch = await check(); if (!patch) return; close(); request("flow-watch-edit", "Change " + watcher.watcher_id, { params: { watcher: watcher.watcher_id, config: patch }, effect: "Merges the configuration into " + watcher.watcher_id + ". Crane checks the flow's bounds on the machine, and a person approves it in Landscape.", fields: [{ key: "purpose", label: "Purpose (leave as is to keep it)", value: watcher.purpose }] }); } }, "Merge and request…"),
              h("span", { class: "fx-spacer" }),
              h("button", { class: "fx-btn small danger", type: "button", onclick: () => { close(); request("flow-watch-stop", "Stop " + watcher.watcher_id, { params: { watcher: watcher.watcher_id } }); } }, "Stop…"))) : null,
          statusOf(watcher) === "ACTIVE" ? section("Run now",
            note("A person's manual trigger: the subagent runs its actions once, through the connections the machine has, and records what each service answered. It may reach targets beyond its flow for this run only."),
            h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small primary", type: "button", onclick: () => { close(); request("flow-subagent-trigger", "Run " + watcher.watcher_id + " now", { params: { watcher: watcher.watcher_id }, effect: "Runs its actions once on the machine that applies the request: " + ((watcher.config || {}).actions || []).join(", ") + ".", fields: [{ key: "beyond", label: "Also reach, for this run only (zone:ID or flow:ID, comma separated; optional)", hint: "zone:ledger" }], validate: (params) => { const targets = String(params.beyond || "").split(",").map((text) => text.trim()).filter(Boolean); const bad = targets.find((target) => !/^(zone|flow):[A-Za-z0-9_.-]+$/.test(target)); if (bad) return "'" + bad + "' must be zone:ID or flow:ID."; params.beyond = targets; return null; } }); } }, "Run now…"))) : null));
      document.body.append(back);
      back.querySelector("button").focus();
    }

    /** Ask for a new subagent on a flow: the subtype is required, the configuration optional */
    function addSubagent(flow) {
      const defaults = h("span");
      request("flow-watch-add", "Add a subagent to " + flow.id, {
        params: { flow: flow.id },
        effect: "A subagent is constrained to its flow. Crane checks the configuration against the flow's bounds, and a person approves it in Landscape before it acts.",
        fields: [
          { key: "subtype", label: "Subtype (required)", kind: "select", value: "", options: [["", "Choose a subtype…"], ["monitor", "Monitor: monitoring, observability, meeting, on-call, activates Incident"], ["incident", "Incident: Jira ticket, RCA, context, pull requests on a branch"]] },
          { key: "purpose", label: "What it is for", required: true },
          ...checkFields(null),
          { key: "config", label: "Configuration (JSON, optional; the flow's defaults otherwise)", kind: "textarea", hint: "{\"parameters\": {\"jira_issue\": \"PAY-12\"}, \"branch\": \"incident/checkout\"}" },
        ],
        shape: shapeChecks,
        validate: async (params) => {
          if (!params.subtype) return "Choose the subtype: Monitor or Incident.";
          if (params.subtype === "incident") params.checks = [];
          else if (!params.checks.length) return "A Monitor needs at least one check.";
          if (!params.config) { delete params.config; return null; }
          let patch;
          try { patch = JSON.parse(params.config); } catch (error) { return "The configuration is not valid JSON: " + error.message; }
          const problems = (() => { try { return boundsProblems(params.subtype, mergeConfig({ ceiling: flow.ceiling, targets: ["flow:" + flow.id], actions: (catalog.actions || []).filter((action) => action.subtype === params.subtype && catalog.ceilings.indexOf(action.ceiling) <= catalog.ceilings.indexOf(flow.ceiling)).map((action) => action.id) }, patch), flow.id); } catch (error) { return [error.message]; } })();
          if (problems.length) { boundsError(problems); return "Outside the flow's bounds; see the list."; }
          const { failed } = await checkUrls(urlsIn(patch));
          if (failed.length) { urlError(failed); return "An address does not answer; see the list."; }
          params.config = patch;
          return null;
        },
      });
      return defaults;
    }

    function drawSubagents() {
      const selected = state.selected && state.selected.startsWith("flow:") ? state.selected.slice(5) : null;
      const filters = [["all", "All", data.watchers.length], ["active", "Active", data.watchers.filter((watcher) => statusOf(watcher) === "ACTIVE").length], ["pending", "Waiting", data.watchers.filter((watcher) => statusOf(watcher) === "PENDING_APPROVAL").length], ["firing", "Firing", data.watchers.filter((watcher) => (watcher.firing || []).length).length]];
      if (selected) filters.push(["flow", "On " + selected, watchersOf(selected).length]);
      if (!filters.some(([id]) => id === state.subFilter)) state.subFilter = "all";
      const shown = data.watchers.filter((watcher) => state.subFilter === "active" ? statusOf(watcher) === "ACTIVE" : state.subFilter === "pending" ? statusOf(watcher) === "PENDING_APPROVAL" : state.subFilter === "firing" ? (watcher.firing || []).length : state.subFilter === "flow" ? watcher.flow === selected : true)
        .sort((a, b) => Number(holdsPlace(b)) - Number(holdsPlace(a)) || (b.firing || []).length - (a.firing || []).length || a.flow.localeCompare(b.flow));
      const flow = selected ? data.flows.find((row) => row.id === selected) : null;
      const taken = flow ? watchersOf(flow.id).filter(holdsPlace).length : 0;
      put(subagents,
        h("div", { class: "fx-subs-head" },
          h("h2", {}, "Subagents"),
          h("span", { class: "fx-note" }, "Monitor and Incident subagents of each flow: at most " + catalog.max_per_flow + " per flow, " + catalog.max_agent_per_flow + " created by an agent. They act only within their flow; Landscape approves every change."),
          h("span", { class: "fx-spacer" }),
          flow ? h("button", { class: "fx-btn small primary", type: "button", disabled: taken >= catalog.max_per_flow, title: taken >= catalog.max_per_flow ? "The flow has its " + catalog.max_per_flow + " subagents" : undefined, onclick: () => addSubagent(flow) }, "Add subagent (" + taken + "/" + catalog.max_per_flow + ")…") : null),
        h("div", { class: "fx-chips", style: "padding:0;border:0", role: "toolbar", "aria-label": "Show subagents" }, filters.map(([id, label, count]) => h("button", { class: "fx-chip", type: "button", "aria-pressed": String(id === state.subFilter), onclick: () => { state.subFilter = id; drawSubagents(); } }, label, h("span", { class: "n" }, String(count))))),
        shown.length ? h("div", { class: "fx-tiles" }, shown.map((watcher) => {
          const status = statusOf(watcher);
          const firing = (watcher.firing || []).length;
          return h("div", { class: "fx-tile" + (firing ? " firing" : "") + (holdsPlace(watcher) ? "" : " stopped") },
            h("span", { class: "fx-avatar small" + (subtypeOf(watcher) === "incident" ? " incident" : "") }, icon(ROBOT)),
            h("div", { class: "who" },
              h("span", { class: "name", title: watcher.purpose }, watcher.purpose),
              h("span", { class: "meta" }, h("i", { class: "fx-dot " + status.toLowerCase() }), subtypeOf(watcher) + " · " + watcher.flow + (watcher.created_by === "agent" ? " · by agent" : "") + (firing ? " · firing" : ""))),
            h("button", { class: "fx-tool", type: "button", title: "Properties and configuration of " + watcher.watcher_id, "aria-label": "Properties and configuration of " + watcher.watcher_id, onclick: () => subagentInfo(watcher) }, icon(INFO)));
        }))
          : note(data.watchers.length ? "No subagent matches this filter." : "No subagents yet. Select a flow and add one, or let an agent ask for one ('crane-request watch --flow ID --type monitor')."));
    }

    drawMap(false);
    drawInspector();
    drawSubagents();
    let lastWidth = size().width;
    const resize = () => {
      if (!document.body.contains(root)) { window.removeEventListener("resize", resize); return; }
      if (Math.abs(size().width - lastWidth) < 40) return;
      lastWidth = size().width;
      drawMap(false);
    };
    window.addEventListener("resize", resize);
  }

  Object.assign(window.FOXX_PAGES, { flows: flowsView });
})();
