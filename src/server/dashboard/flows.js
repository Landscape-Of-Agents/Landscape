"use strict";
// The DataSea Flows section: the repository's flows and zones as one quiet map you can pan and
// zoom, with the knowledge of each one a click away. Nodes show a name, a priority color, a red dot
// for incidents, and an agent badge when a watcher or an assigned agent monitors them; selecting
// one highlights it with the zones or flows it is attached to and zooms to them. The inspector under
// the map holds the selection, sorted by filter chips (overview, zones, context, tests, agents,
// trace, ...): clicking an item opens it in the smaller detail pane beside the list. A flow's trace
// reads as a stack (by call depth) or a waterfall (in call order). The Subagents section lists the
// watchers that govern flows, with their behavior, triggers, and what they raised; code owners
// change them. Every change is a request a person applies on a machine ('crane brick inbox apply',
// or one click in Landscape), where Crane checks it and, for signed knowledge, signs it.
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
.fx-node { cursor: pointer; outline: none; }
.fx-node rect.card { fill: var(--panel); stroke: var(--line-2); stroke-width: 1; transition: stroke .12s, fill .12s; }
.fx-node:hover rect.card { stroke: var(--accent); }
.fx-node.selected rect.card { stroke: var(--accent); stroke-width: 2.4; fill: var(--accent-soft); }
.fx-node.related rect.card { stroke: var(--accent); stroke-width: 1.6; }
.fx-node.dim { opacity: .25; }
.fx-node.draft rect.card { stroke-dasharray: 4 3; }
.fx-node text.name { fill: var(--ink); font-size: 11.5px; font-weight: 600; }
.fx-edge { fill: none; stroke: var(--accent); stroke-width: 1.6; opacity: .8; }
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
        if (options.validate) { const problem = options.validate(params); if (problem) { error.textContent = problem; return; } }
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

  const NODE = { w: 136, h: 30, gapX: 14, gapY: 14, band: 28, pad: 28 };

  /** Lay out bands of nodes in rows that fit the width; returns positions, band labels, and size */
  function layout(bands, width) {
    const columns = Math.max(2, Math.floor((width - NODE.pad * 2 + NODE.gapX) / (NODE.w + NODE.gapX)));
    const positions = new Map();
    const labels = [];
    let y = NODE.pad;
    for (const band of bands) {
      if (!band.items.length) continue;
      labels.push({ text: band.label + " · " + band.items.length, y: y + 10 });
      y += NODE.band;
      band.items.forEach((item, index) => {
        const column = index % columns, row = Math.floor(index / columns);
        positions.set(item.key, { x: NODE.pad + column * (NODE.w + NODE.gapX), y: y + row * (NODE.h + NODE.gapY), item });
      });
      y += Math.ceil(band.items.length / columns) * (NODE.h + NODE.gapY) + 8;
    }
    const used = Math.min(columns, Math.max(1, ...bands.map((band) => band.items.length)));
    return { positions, labels, height: y + NODE.pad, width: NODE.pad * 2 + used * (NODE.w + NODE.gapX) - NODE.gapX };
  }

  /** One node: a name, a priority color, a red dot for incidents, an agent badge when monitored */
  function node(position, onPick) {
    const { x, y, item } = position;
    const name = item.name.length > 17 ? item.name.slice(0, 16) + "…" : item.name;
    const color = item.kind === "zone" ? "var(--low)" : item.draft ? "var(--muted)" : item.priority === "P0" ? "var(--bad)" : item.priority === "P1" ? "var(--warn)" : "var(--info)";
    const label = (item.kind === "zone" ? "Zone " : item.draft ? "Draft flow " : "Flow ") + item.name + (item.monitors.length ? ", monitored by " + item.monitors.join(", ") : "") + (item.incidents ? ", " + item.incidents + " incident(s)" : "");
    return svgNode("g", { class: "fx-node" + (item.draft ? " draft" : ""), tabindex: "0", role: "button", "aria-label": label, "data-key": item.key, transform: `translate(${x},${y})`, onclick: () => onPick(item.key), onkeydown: (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onPick(item.key); } } },
      svgNode("title", {}, item.name + (item.title && item.title !== item.name ? " — " + item.title : "") + (item.monitors.length ? "\nMonitored by " + item.monitors.join(", ") : "")),
      svgNode("rect", { class: "card", width: NODE.w, height: NODE.h, rx: 8 }),
      svgNode("rect", { x: 0, y: 7, width: 3, height: NODE.h - 14, rx: 1.5, fill: color }),
      svgNode("text", { class: "name", x: 12, y: NODE.h / 2 + 4 }, name),
      item.incidents ? svgNode("circle", { cx: 2, cy: 2, r: 4, fill: "var(--bad)", stroke: "var(--panel)", "stroke-width": 1.5 }) : null,
      item.monitors.length ? agentBadge(NODE.w - 9, -7) : null);
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
    const bands = [
      ...["P0", "P1", "P2", "P3"].map((priority) => ({ label: "Flows · " + priority, items: items.filter((item) => item.kind === "flow" && item.priority === priority) })),
      { label: "Flows · no priority", items: items.filter((item) => item.kind === "flow" && !item.priority) },
      { label: "Drafts", items: items.filter((item) => item.kind === "draft") },
      { label: "Zones", items: items.filter((item) => item.kind === "zone").sort((a, b) => (critical[zoneCriticality.get(a.id)] ?? 9) - (critical[zoneCriticality.get(b.id)] ?? 9) || a.name.localeCompare(b.name)) },
    ];
    for (const band of bands) if (band.label.startsWith("Flows")) band.items.sort((a, b) => (rank[a.priority] ?? 9) - (rank[b.priority] ?? 9) || a.name.localeCompare(b.name));

    const state = { selected: null, filter: null, peek: null, nodes: new Map(), view: { x: 0, y: 0, k: 1 }, traceMode: recall("fx-trace-mode") || "waterfall", traceSource: null, subFilter: "all" };
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
    const svg = svgNode("svg", { role: "group", "aria-label": "Map of flows and zones: drag to pan, scroll to zoom" }, viewport);
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

    function drawMap(keepView) {
      geometry = layout(bands, Math.max(560, size().width));
      state.nodes = new Map();
      if (!items.length) {
        canvas.append(h("div", { class: "fx-empty" }, "No flows or zones yet. Trace a flow from its starting point; once a code owner approves it, it appears here."));
        return;
      }
      put(nodeLayer,
        geometry.labels.map((label) => svgNode("text", { class: "fx-band", x: NODE.pad, y: label.y }, label.text)),
        [...geometry.positions.values()].map((position) => { const group = node(position, pick); state.nodes.set(position.item.key, group); return group; }));
      highlight();
      if (!keepView) { if (state.selected) focusSelection(false); else fit(false); }
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
      for (const [key, group] of state.nodes) {
        group.classList.toggle("selected", key === state.selected);
        group.classList.toggle("related", near.has(key));
        group.classList.toggle("dim", Boolean(state.selected) && key !== state.selected && !near.has(key));
      }
      focusButton.disabled = !state.selected;
      clearButton.disabled = !state.selected;
      const lines = [];
      if (state.selected && geometry) {
        const from = geometry.positions.get(state.selected);
        for (const key of near) {
          const to = geometry.positions.get(key);
          if (!from || !to) continue;
          const [a, b] = from.y <= to.y ? [from, to] : [to, from];
          const x1 = a.x + NODE.w / 2, y1 = a.y + NODE.h, x2 = b.x + NODE.w / 2, y2 = b.y;
          lines.push(svgNode("path", { class: "fx-edge", d: `M${x1} ${y1} C ${x1} ${(y1 + y2) / 2}, ${x2} ${(y1 + y2) / 2}, ${x2} ${y2}` }));
        }
      }
      put(edgeLayer, lines);
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
      const nav = chips([["overview", "Overview"], ["proposals", "Proposals", proposals.length], ["incidents", "Incidents", data.incidents.length], ["requests", "Requests", data.requests.length]]);
      let pane, peek = null;
      if (state.filter === "proposals") pane = list(proposals.map((proposal) => itemRow({ type: "proposal", id: proposal.proposal_id }, proposalTitle(proposal), proposal.flow + " · " + dt(proposal.proposed_at), pill(proposal.kind))), "No proposals from agents are waiting.");
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
      const nav = chips([["overview", "Overview"], ["zones", "Zones", (flow.stages || []).length], ["context", "Context", (flow.context || []).length], ["tests", "Tests", (flow.tests || []).length], ["agents", "Agents", watchers.length + agents.length], ["trace", "Trace", flow.trace ? flow.trace.nodes : draft ? "draft" : 0], ["people", "People", (flow.people || []).length], ["proposals", "Proposals", proposals.length], ["bricks", "Bricks", (flow.bricks || []).length], ["agent-lens", "For agents"]]);
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
        h("div", { class: "fx-section" }, h("h3", {}, "Subagents (watchers)"), list(watchers.map((watcher) => itemRow({ type: "watcher", id: watcher.watcher_id }, watcher.purpose, watcher.watcher_id + " · " + watcher.checks.length + " check(s)", [watcher.active ? pill("active", "ok") : pill("stopped"), ...(watcher.firing || []).map((code) => pill(code, "bad"))])), "No watcher yet. Agents start one with 'crane-request watch --flow " + flow.id + "' up to the limit.")),
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
      else if (state.filter === "people") pane = [h("div", { class: "fx-acts" }, personAdd(flow)), list((flow.people || []).map((person) => itemRow({ type: "person", id: person.id, flow: flow.id }, person.name, [person.github ? "@" + person.github : "", person.jira || "", person.slack || ""].filter(Boolean).join(" · ") || "from " + person.source, pill(person.role))), "Nobody is designated. 'crane flow people suggest " + flow.id + "' lists the Git authors of its files.")];
      else if (state.filter === "proposals") pane = list(proposals.map((proposal) => itemRow({ type: "proposal", id: proposal.proposal_id }, proposalTitle(proposal), "by " + ((proposal.proposed_by || {}).agent || "an agent") + " · " + dt(proposal.proposed_at), pill(proposal.kind))), "No proposals from agents are waiting for this flow.");
      else if (state.filter === "bricks") pane = bricksList(flow.bricks || []);
      else if (state.filter === "agent-lens") pane = agentLens(flow, watchers);
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

    function personAdd(flow) {
      return h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-person-add", "Designate a person on " + flow.id, { params: { flow: flow.id }, effect: "They are told when a watcher raises an incident (Slack, and the Jira issue that names them).", fields: [{ key: "name", label: "Name", required: true, max: 120 }, { key: "role", label: "Role", kind: "select", value: "owner", options: [["author", "Author (wrote the code)"], ["owner", "Owner"], ["reviewer", "Reviewer"], ["oncall", "On call"]] }, { key: "github", label: "GitHub login" }, { key: "jira", label: "Jira account or issue (PAY-12)" }, { key: "slack", label: "Slack member id (U…)" }] }) }, "Designate…");
    }

    // ---------------------------------------------------------------- the trace: stack or waterfall

    function traceView(graph, diff) {
      if (!graph || !(graph.nodes || []).length) return note("The trace found no functions.");
      const rows = callRows(graph);
      const total = Math.max(1, rows.length);
      const added = new Set(((diff || {}).added_nodes) || []);
      const files = [...new Set((graph.nodes || []).map((entry) => entry.path))];
      const shade = (path) => FILE_TONES[files.indexOf(path) % FILE_TONES.length];
      const pickNode = (row) => { state.peek = { type: "function", id: row.node.id, row: row.start }; drawInspector(); };
      const isOn = (row) => state.peek && state.peek.type === "function" && state.peek.row === row.start;
      const bar = (row, extra) => h("div", { class: "fx-bar" + (isOn(row) ? " on" : "") + (added.has(row.node.id) ? " added" : ""), role: "button", tabindex: "0", title: row.node.symbol + " — " + row.node.path + ":" + row.node.start_line + (row.again ? " (reached again)" : ""),
        style: Object.assign({ left: (row.start / total) * 100 + "%", width: "calc(" + (row.size / total) * 100 + "% - 1px)", background: "color-mix(in srgb, " + shade(row.node.path) + " " + (row.again ? 14 : 30) + "%, var(--panel))" }, extra || {}),
        onclick: (event) => { event.stopPropagation(); pickNode(row); }, onkeydown: (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); pickNode(row); } } }, row.node.symbol);
      const mode = h("div", { class: "fx-seg", role: "group", "aria-label": "Trace view" }, [["stack", "Stack"], ["waterfall", "Waterfall"]].map(([id, label]) => h("button", { type: "button", "aria-pressed": String(state.traceMode === id), onclick: () => { state.traceMode = id; remember("fx-trace-mode", id); drawInspector(); } }, label)));
      let body;
      if (state.traceMode === "stack") {
        const depth = Math.max(...rows.map((row) => row.depth)) + 1;
        body = h("div", { class: "fx-stack", style: { height: depth * 26 + 6 + "px" }, role: "group", "aria-label": "Call stack: one row per call depth; a box spans the calls it makes" }, rows.map((row) => bar(row, { top: 3 + row.depth * 26 + "px" })));
      } else {
        body = h("div", { class: "fx-waterfall", role: "list", "aria-label": "Calls in order" }, rows.map((row) => h("div", { class: "fx-wf-row" + (isOn(row) ? " on" : ""), role: "listitem", onclick: () => pickNode(row) },
          h("div", { class: "fx-wf-name", style: { paddingLeft: 8 + row.depth * 14 + "px" }, title: row.node.path + ":" + row.node.start_line }, (row.depth ? "└ " : "") + row.node.symbol, row.again ? h("span", { class: "again" }, " (above)") : null),
          h("div", { class: "fx-wf-track" }, bar(row)))));
      }
      return h("div", { class: "fx-section" },
        h("div", { class: "fx-trace-bar" }, mode, h("span", { class: "fx-note" }, state.traceMode === "stack" ? "Rows are call depth; each box spans the calls under it." : "Rows are calls in the order the trace follows them; each bar spans the calls under it."), h("span", { class: "fx-spacer" }), diff ? h("span", { class: "fx-note" }, "+" + diff.added_nodes.length + " / −" + diff.removed_nodes.length + " functions since the revision before (green edge)") : null),
        body,
        h("div", { class: "fx-axis" }, h("span", {}, "first call"), h("span", {}, "static call order, not timing · colors are files"), h("span", {}, rows.length + (graph.truncated || rows.length >= 400 ? "+ calls" : " calls"))),
        (graph.unresolved || []).length ? h("details", {}, h("summary", { class: "fx-note" }, graph.unresolved.length + " call(s) not followed (external, dynamic, or ambiguous)"), h("div", { class: "fx-code" }, graph.unresolved.join("\n"))) : null);
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
        return [peekHead("Person · " + person.role, person.name), field("GitHub", person.github ? "@" + person.github : ""), field("Jira", person.jira), field("Slack", person.slack), field("Source", person.source),
          h("div", { class: "fx-acts" }, h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-person-remove", "Remove " + person.name, { params: { flow: flow.id, person: person.id } }) }, "Remove…"))];
      }
      if (peek.type === "watcher") {
        const watcher = data.watchers.find((row) => row.watcher_id === peek.id);
        return watcher ? [peekHead("Subagent · watcher", watcher.purpose, watcher.active ? pill("active", "ok") : pill("stopped"), ...(watcher.firing || []).map((code) => pill(code, "bad"))), watcherProps(watcher), watcherActions(watcher)] : null;
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

    /** What an agent receives about the flow, and what it may ask */
    function agentLens(flow, watchers) {
      const lines = ["Flow " + flow.id + ": " + (flow.description || "no description") + " (criticality " + flow.criticality + ", autonomy ceiling " + flow.ceiling + ", " + flow.status + ")", "Starts at: " + (flow.entry_points || []).join(", ")];
      if (flow.priority) lines.push("Priority " + flow.priority + ": model tiers " + ((flow.routed || {}).tiers || []).join(", ") + ((flow.routed || {}).model ? " (routed to " + flow.routed.model + " by " + flow.routed.provider + ")" : ""));
      if (flow.trace) lines.push("Approved trace (revision " + flow.trace.revision + ", " + flow.trace.status + "): " + flow.trace.nodes + " functions, " + flow.trace.edges + " calls in " + flow.trace.files.length + " files");
      if ((flow.people || []).length) lines.push("People: " + flow.people.map((person) => person.name + " (" + person.role + ")").join("; "));
      for (const entry of flow.context || []) lines.push("Context [" + entry.kind + "] " + entry.title + ": " + plain(entry.value) + (entry.unit ? " " + entry.unit : "") + (!empty(entry.threshold) ? " (keep within " + entry.threshold + ")" : ""));
      const tests = flow.tests || [];
      if (tests.length) lines.push("Tests: " + tests.length + " approved, " + tests.filter((test) => test.required).length + " required for every release");
      lines.push("Watchers: " + watchers.filter((watcher) => watcher.active).length + " active of " + ((flow.watchers || {}).limit || 0) + " allowed");
      lines.push("This is guidance; it never authorizes anything. Crane decides every action.");
      const asks = [
        "crane-request flow " + flow.id,
        "crane-request route --flow " + flow.id,
        "crane-request test --flow " + flow.id + " --file PATH --name TEXT --description TEXT --assumptions 'A; B' --mock TEXT",
        "crane-request context --flow " + flow.id + " --kind latency --title TEXT --value N --unit ms",
        "crane-request watch --flow " + flow.id + " --purpose TEXT",
        "crane-request incident --flow " + flow.id + " --summary TEXT",
      ];
      const copy = (text) => async (event) => { try { await navigator.clipboard.writeText(text); event.target.textContent = "Copied"; } catch (error) { event.target.textContent = "Select and copy"; } };
      return [
        note("What an agent receives when it asks for this flow (and, shorter, with each tool call that touches it). Rebuilt here from the last report; the machine's answer is the authority."),
        h("div", { class: "fx-code" }, lines.join("\n")),
        h("div", { class: "fx-section" }, h("h3", {}, "What an agent may ask"), h("div", { class: "fx-items" }, asks.map((ask) => h("div", { class: "fx-item", style: "cursor:default" }, h("code", { class: "t" }, ask), h("span", { class: "r" }, h("button", { class: "fx-btn small ghost", type: "button", onclick: copy(ask) }, "Copy")))))),
        note("The hook answers these and stops the command: nothing runs. Traces, tests, and context are drafts and proposals a person approves; watchers are read-only and capped by the limits only a person changes."),
      ];
    }

    // ---------------------------------------------------------------- subagents

    /** A watcher's properties: behavior, triggers, what it governs and inherits, and what it raised */
    function watcherProps(watcher) {
      const raised = data.incidents.filter((incident) => incident.watcher_id === watcher.watcher_id);
      const started = watcher.created_by === "agent" ? "agent " + ((watcher.parent || {}).agent || "?") + " (session " + ((watcher.parent || {}).session || "?") + ((watcher.parent || {}).model ? ", " + watcher.parent.model : "") + ")" : watcher.created_by;
      const inherited = watcher.inherited || {};
      return h("dl", { class: "fx-props" },
        h("dt", {}, "Governs"), h("dd", {}, h("button", { class: "fx-link", type: "button", onclick: () => pick("flow:" + watcher.flow, { scroll: true }) }, watcher.flow)),
        h("dt", {}, "Behavior"), h("dd", {}, "Read-only checks, no model; raises incidents to the flow's people and its parent session"),
        h("dt", {}, "Triggers"), h("dd", {}, (watcher.checks || []).map((check) => { const known = CHECKS.find(([key]) => key === check); return h("div", {}, known ? known[1] : check, known ? h("span", { class: "muted" }, " · " + known[2]) : null); })),
        h("dt", {}, "Inherits"), h("dd", {}, ((inherited.bricks || []).length ? inherited.bricks.join(", ") : "no Bricks") + " · ceiling " + (inherited.ceiling || "—")),
        h("dt", {}, "Started by"), h("dd", {}, started + " · " + dt(watcher.created_at)),
        h("dt", {}, "Last ran"), h("dd", {}, watcher.last_run ? dt(watcher.last_run) : "not yet"),
        h("dt", {}, "Insights"), h("dd", {}, raised.length ? raised.length + " incident(s) raised, last " + dt(Math.max(...raised.map((incident) => incident.raised_at))) + (watcher.firing && watcher.firing.length ? "; firing now: " + watcher.firing.join(", ") : "") : watcher.firing && watcher.firing.length ? "Firing now: " + watcher.firing.join(", ") : "Quiet: nothing raised"),
        watcher.stopped_by ? h("dt", {}, "Stopped by") : null, watcher.stopped_by ? h("dd", {}, watcher.stopped_by) : null);
    }
    function watcherActions(watcher) {
      if (!watcher.active) return null;
      return h("div", { class: "fx-acts" },
        h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-watch-edit", "Change " + watcher.watcher_id, { params: { watcher: watcher.watcher_id }, effect: "Changes what the subagent is for and what it checks. It stays read-only and keeps the Bricks and ceiling it inherited.", fields: [{ key: "purpose", label: "Why it watches", value: watcher.purpose, required: true }, ...checkFields(watcher.checks)], shape: shapeChecks, validate: (params) => (params.checks.length ? null : "Choose at least one check.") }) }, "Edit…"),
        h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-watch-stop", "Stop " + watcher.watcher_id, { params: { watcher: watcher.watcher_id } }) }, "Stop…"));
    }

    function drawSubagents() {
      const selected = state.selected && state.selected.startsWith("flow:") ? state.selected.slice(5) : null;
      const filters = [["all", "All", data.watchers.length], ["active", "Active", data.watchers.filter((watcher) => watcher.active).length], ["firing", "Firing", data.watchers.filter((watcher) => (watcher.firing || []).length).length]];
      if (selected) filters.push(["flow", "On " + selected, watchersOf(selected).length]);
      if (!filters.some(([id]) => id === state.subFilter)) state.subFilter = "all";
      const shown = data.watchers.filter((watcher) => state.subFilter === "active" ? watcher.active : state.subFilter === "firing" ? (watcher.firing || []).length : state.subFilter === "flow" ? watcher.flow === selected : true)
        .sort((a, b) => Number(b.active) - Number(a.active) || (b.firing || []).length - (a.firing || []).length || a.flow.localeCompare(b.flow));
      const flow = selected ? data.flows.find((row) => row.id === selected) : null;
      const limit = flow ? (flow.watchers || {}).limit || 0 : 0;
      const running = flow ? watchersOf(flow.id).filter((watcher) => watcher.active).length : 0;
      put(subagents,
        h("div", { class: "fx-subs-head" },
          h("h2", {}, "Subagents"),
          h("span", { class: "fx-note" }, "Watchers that govern flows: read-only, no model, capped by the signed limits. Code owners of .crane/flows.json change them."),
          h("span", { class: "fx-spacer" }),
          flow ? h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-limit", "Watcher limit of " + flow.id, { params: { flow: flow.id }, effect: "Only a person changes it; a flow cannot go above the repository's limit.", fields: [{ key: "watchers", label: "Watchers on this flow", kind: "number", value: limit }] }) }, "Limit " + running + "/" + limit + "…") : null,
          flow ? h("button", { class: "fx-btn small primary", type: "button", disabled: running >= limit, title: running >= limit ? "At the limit; raise it first" : undefined, onclick: () => request("flow-watch-add", "Start a subagent on " + flow.id, { params: { flow: flow.id }, effect: "A read-only watcher runs Crane's checks of this flow without a model. It takes on the flow's Bricks and ceiling, and raises an incident to the designated people.", fields: [{ key: "purpose", label: "Why it watches", required: true }, ...checkFields(null)], shape: shapeChecks }) }, "Start a subagent…") : null),
        h("div", { class: "fx-chips", style: "padding:0;border:0", role: "toolbar", "aria-label": "Show subagents" }, filters.map(([id, label, count]) => h("button", { class: "fx-chip", type: "button", "aria-pressed": String(id === state.subFilter), onclick: () => { state.subFilter = id; drawSubagents(); } }, label, h("span", { class: "n" }, String(count))))),
        shown.length ? h("div", { class: "fx-cards" }, shown.map((watcher) => h("article", { class: "fx-card" + ((watcher.firing || []).length ? " firing" : "") + (watcher.active ? "" : " stopped") },
          h("div", { class: "fx-card-head" }, h("span", { class: "fx-avatar" }, icon(ROBOT)), h("div", { class: "who" }, h("span", { class: "name" }, watcher.purpose), h("span", { class: "id" }, watcher.watcher_id)), h("div", { class: "fx-acts" }, watcher.active ? pill("active", "ok") : pill("stopped"), ...(watcher.firing || []).map((code) => pill(code, "bad")))),
          watcherProps(watcher),
          watcherActions(watcher))))
          : note(data.watchers.length ? "No subagent matches this filter." : "No subagents yet. Select a flow and start one, or let agents start them with 'crane-request watch --flow ID' up to the limit."));
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
