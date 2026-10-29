"use strict";
// The DataSea Flows section: the repository's flows and zones as one quiet map, with the knowledge
// of each flow one click away. The map shows only names and a few signals (priority, drafts,
// incidents, watchers); a flow's lines to its zones appear when it is selected, so the map stays
// readable with many flows. The lookout under the map holds everything about the selection: its
// trace and drafts, typed context, tests, people, agents and watchers, Bricks, and the exact text
// agents receive. Every change is a request a person applies on a machine ('crane brick inbox
// apply', or one click in Landscape), where Crane checks it and, for signed knowledge, signs it.
// Data: GET /api/flows (the last status a member's machine reported); changes: POST /api/flows.

window.FOXX_PAGES = window.FOXX_PAGES || {};

(function () {
  const CSS = `
.fx { --fx-gap: 14px; display: grid; gap: var(--fx-gap); }
.fx-head { display: flex; flex-wrap: wrap; align-items: center; gap: 10px 18px; padding: 2px 2px 0; }
.fx-title { font-size: 20px; font-weight: 750; letter-spacing: -.01em; margin: 0; }
.fx-meta { color: var(--muted); font-size: 12.5px; }
.fx-stats { display: flex; flex-wrap: wrap; gap: 6px; }
.fx-stat { font: 600 11.5px/1 var(--mono); padding: 6px 9px; border: 1px solid var(--line); border-radius: 999px; background: var(--panel); color: var(--ink-2); white-space: nowrap; }
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
.fx-btn:focus-visible, .fx-node:focus-visible, .fx-tab:focus-visible, .fx-search input:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
.fx-canvas { position: relative; border: 1px solid var(--line); border-radius: 14px; background:
  radial-gradient(circle at 1px 1px, color-mix(in srgb, var(--axis) 55%, transparent) 1px, transparent 1.2px) 0 0 / 18px 18px, var(--panel-2); overflow: hidden; }
.fx-toolbar { position: sticky; top: 0; z-index: 2; display: flex; flex-wrap: wrap; align-items: center; gap: 8px; padding: 10px 12px; background: color-mix(in srgb, var(--panel) 88%, transparent); backdrop-filter: blur(6px); border-bottom: 1px solid var(--line); }
.fx-search { display: flex; align-items: center; gap: 8px; flex: 1 1 260px; max-width: 460px; padding: 0 10px; border: 1px solid var(--line-2); border-radius: 9px; background: var(--panel); }
.fx-search input { flex: 1; min-width: 0; border: 0; background: transparent; color: var(--ink); font: 13.5px/1 inherit; padding: 9px 0; outline: none; }
.fx-search kbd { margin: 0; }
.fx-legend { display: flex; flex-wrap: wrap; gap: 12px; font-size: 11.5px; color: var(--muted); }
.fx-legend i { display: inline-block; width: 9px; height: 9px; border-radius: 3px; margin-right: 5px; vertical-align: -1px; }
.fx-map { overflow: auto; max-height: 62vh; }
.fx-map svg { display: block; }
.fx-band { font: 700 10.5px/1 var(--mono); letter-spacing: .12em; text-transform: uppercase; fill: var(--muted); }
.fx-node rect.card { fill: var(--panel); stroke: var(--line-2); stroke-width: 1.2; transition: stroke .12s; }
.fx-node { cursor: pointer; outline: none; }
.fx-node:hover rect.card { stroke: var(--accent); }
.fx-node.selected rect.card { stroke: var(--accent); stroke-width: 2.2; }
.fx-node.related rect.card { stroke: color-mix(in srgb, var(--accent) 60%, var(--line-2)); }
.fx-node.dim { opacity: .28; }
.fx-node.draft rect.card { stroke-dasharray: 5 4; }
.fx-node text.name { fill: var(--ink); font-size: 12.5px; font-weight: 650; }
.fx-node text.sub { fill: var(--muted); font-size: 10.5px; font-family: var(--mono); }
.fx-edge { fill: none; stroke: var(--accent); stroke-width: 1.6; opacity: .75; }
.fx-empty { padding: 36px 18px; text-align: center; color: var(--muted); }
.fx-look { border: 1px solid var(--line); border-radius: 14px; background: var(--panel); box-shadow: var(--shadow); overflow: hidden; }
.fx-look-head { display: flex; flex-wrap: wrap; align-items: center; gap: 8px 14px; padding: 14px 16px 0; }
.fx-kind { font: 700 10.5px/1 var(--mono); letter-spacing: .12em; text-transform: uppercase; color: var(--accent); }
.fx-look-title { font-size: 17px; font-weight: 750; margin: 0; }
.fx-tabs { display: flex; gap: 2px; padding: 10px 12px 0; border-bottom: 1px solid var(--line); overflow-x: auto; }
.fx-tab { font: 600 12.5px/1 inherit; padding: 9px 11px; border: 0; border-bottom: 2px solid transparent; background: none; color: var(--muted); cursor: pointer; white-space: nowrap; }
.fx-tab[aria-selected="true"] { color: var(--ink); border-bottom-color: var(--accent); }
.fx-tab .n { font: 600 10.5px/1 var(--mono); margin-left: 5px; color: var(--muted); }
.fx-body { padding: 14px 16px 18px; display: grid; gap: 14px; }
.fx-grid { display: grid; grid-template-columns: repeat(auto-fill, minmax(180px, 1fr)); gap: 10px; }
.fx-field { display: grid; gap: 4px; min-width: 0; }
.fx-label { font: 700 10px/1 var(--mono); letter-spacing: .1em; text-transform: uppercase; color: var(--muted); }
.fx-value { font-size: 13.5px; color: var(--ink); overflow-wrap: anywhere; }
.fx-value code, .fx-code { font: 12px/1.5 var(--mono); }
.fx-code { display: block; padding: 10px 12px; border-radius: 9px; background: var(--panel-2); border: 1px solid var(--line); white-space: pre-wrap; overflow-wrap: anywhere; max-height: 320px; overflow: auto; color: var(--ink); }
.fx-row { display: grid; grid-template-columns: minmax(0, 1fr) auto; gap: 6px 12px; align-items: start; padding: 10px 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel); }
.fx-row + .fx-row { margin-top: -6px; }
.fx-row .main { min-width: 0; display: grid; gap: 4px; }
.fx-row .title { font-weight: 650; font-size: 13.5px; overflow-wrap: anywhere; }
.fx-row .meta { color: var(--muted); font-size: 12px; overflow-wrap: anywhere; }
.fx-row .acts { display: flex; flex-wrap: wrap; gap: 6px; justify-content: flex-end; }
.fx-list { display: grid; gap: 12px; }
.fx-pill { display: inline-flex; align-items: center; gap: 5px; font: 650 10.5px/1 var(--mono); padding: 4px 7px; border-radius: 999px; border: 1px solid var(--line-2); color: var(--ink-2); background: var(--panel-2); white-space: nowrap; }
.fx-pill.p0 { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 40%, var(--line)); }
.fx-pill.p1 { color: var(--warn); border-color: color-mix(in srgb, var(--warn) 40%, var(--line)); }
.fx-pill.ok { color: var(--ok); border-color: color-mix(in srgb, var(--ok) 40%, var(--line)); }
.fx-pill.bad { color: var(--bad); border-color: color-mix(in srgb, var(--bad) 40%, var(--line)); }
.fx-pill.accent { color: var(--accent); border-color: color-mix(in srgb, var(--accent) 45%, var(--line)); }
.fx-section { display: grid; gap: 10px; }
.fx-section > h3 { margin: 0; font-size: 13px; font-weight: 700; display: flex; align-items: center; gap: 10px; }
.fx-section > h3 .fx-spacer { flex: 1; }
.fx-meter { height: 6px; border-radius: 999px; background: var(--panel-2); border: 1px solid var(--line); overflow: hidden; }
.fx-meter i { display: block; height: 100%; background: var(--accent); }
.fx-note { font-size: 12.5px; color: var(--muted); }
.fx-tree { font: 12px/1.65 var(--mono); padding: 10px 12px; border: 1px solid var(--line); border-radius: 10px; background: var(--panel-2); max-height: 360px; overflow: auto; }
.fx-tree .add { color: var(--ok); }
.fx-tree .file { color: var(--muted); }
.fx-split { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 14px; }
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
@media (max-width: 760px) { .fx-split { grid-template-columns: 1fr; } .fx-row { grid-template-columns: 1fr; } .fx-row .acts { justify-content: flex-start; } }
@media (prefers-reduced-motion: reduce) { .fx-node rect.card { transition: none; } }
`;

  const svgNode = (tag, attributes, ...children) => window.Foxx.svg(tag, attributes, ...children);
  const pill = (text, tone, title) => h("span", { class: "fx-pill" + (tone ? " " + tone : ""), title: title || undefined }, text);
  const field = (label, value) => h("div", { class: "fx-field" }, h("span", { class: "fx-label" }, label), h("span", { class: "fx-value" }, value === null || value === undefined || value === "" ? h("span", { class: "muted" }, "—") : value));
  const shared = (value) => (!value ? "" : value.withheld ? h("span", { class: "muted" }, "Not shown: it looks like it holds a secret.") : value.text + (value.truncated ? " …" : ""));
  const plain = (value) => (!value ? "" : value.withheld ? "(withheld)" : value.text || "");
  const tone = (priority) => (priority === "P0" ? "p0" : priority === "P1" ? "p1" : "");
  const SIGNED = new Set(["flow-approve", "flow-discard", "flow-priority", "flow-context-add", "flow-context-remove", "flow-person-add", "flow-person-remove", "flow-test-require", "flow-proposal-approve", "flow-proposal-reject", "flow-limit"]);

  function injectStyle() {
    if (document.getElementById("fx-style")) return;
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
      else control = h("input", { id: "fx-" + spec.key, type: spec.kind === "number" ? "number" : "text", value: spec.value === undefined ? "" : String(spec.value), placeholder: spec.hint || "", maxlength: spec.kind === "number" ? undefined : String(spec.max || 300) });
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
        if (!reason.value.trim()) { error.textContent = "Say why; it is recorded with the change."; reason.focus(); return; }
        submit.disabled = true;
        try {
          const staged = await apiWrite("/api/flows", { action: "stage", op, params, reason: reason.value.trim() });
          location.href = staged.continue + "&page=flows";
        } catch (problem) { submit.disabled = false; error.textContent = problem.message; }
      } },
        h("div", { class: "fx-kind" }, SIGNED.has(op) ? "Signed change · request" : "Request"),
        h("h2", {}, title),
        options.effect ? h("p", { class: "fx-note", style: "margin:0" }, options.effect) : null,
        ...controls,
        h("label", {}, "Why", reason),
        error,
        h("p", { class: "fx-note", style: "margin:0" }, SIGNED.has(op)
          ? "GitHub confirms you are a code owner of the signed file (or a repository admin when CODEOWNERS does not cover it). Nothing changes yet: a person applies it on a machine with an admin key, where Crane checks and signs it."
          : "GitHub confirms you can write to the repository. A person applies it on a machine ('crane brick inbox apply', or one click in Landscape)."),
        h("div", { class: "acts" }, h("button", { class: "fx-btn ghost", type: "button", onclick: close }, "Cancel"), submit)));
    document.body.append(back);
    const first = Object.values(inputs)[0];
    (first ? first.control : reason).focus();
  }

  // ---------------------------------------------------------------- the map

  const NODE = { w: 176, h: 46, gapX: 14, gapY: 14, band: 34, pad: 18 };

  /** Lay out bands of nodes in rows that fit the width; returns positions and the height */
  function layout(bands, width) {
    const columns = Math.max(1, Math.floor((width - NODE.pad * 2 + NODE.gapX) / (NODE.w + NODE.gapX)));
    const positions = new Map();
    const labels = [];
    let y = NODE.pad;
    for (const band of bands) {
      if (!band.items.length) continue;
      labels.push({ text: band.label + " · " + band.items.length, y: y + 12 });
      y += NODE.band;
      band.items.forEach((item, index) => {
        const column = index % columns, row = Math.floor(index / columns);
        positions.set(item.key, { x: NODE.pad + column * (NODE.w + NODE.gapX), y: y + row * (NODE.h + NODE.gapY), item });
      });
      y += Math.ceil(band.items.length / columns) * (NODE.h + NODE.gapY) + 6;
    }
    return { positions, labels, height: y + NODE.pad, width: Math.max(width, NODE.pad * 2 + columns * (NODE.w + NODE.gapX)) };
  }

  /** One node: a name and a single line of signals */
  function node(position, state, onPick) {
    const { x, y, item } = position;
    const name = item.name.length > 22 ? item.name.slice(0, 21) + "…" : item.name;
    const color = item.kind === "zone" ? "var(--low)" : item.draft ? "var(--muted)" : item.priority === "P0" ? "var(--bad)" : item.priority === "P1" ? "var(--warn)" : "var(--info)";
    const group = svgNode("g", { class: "fx-node" + (item.draft ? " draft" : ""), tabindex: "0", role: "button", "aria-label": (item.kind === "zone" ? "Zone " : item.draft ? "Draft flow " : "Flow ") + item.name, transform: `translate(${x},${y})`, onclick: () => onPick(item.key), onkeydown: (event) => { if (event.key === "Enter" || event.key === " ") { event.preventDefault(); onPick(item.key); } } },
      svgNode("title", {}, item.title || item.name),
      svgNode("rect", { class: "card", width: NODE.w, height: NODE.h, rx: 10 }),
      svgNode("rect", { x: 0, y: 8, width: 3, height: NODE.h - 16, rx: 1.5, fill: color }),
      svgNode("text", { class: "name", x: 14, y: 19 }, name),
      svgNode("text", { class: "sub", x: 14, y: 35 }, item.signals),
      item.incidents ? svgNode("circle", { cx: NODE.w - 12, cy: 12, r: 4.5, fill: "var(--bad)" }) : null,
      item.watchers ? svgNode("text", { class: "sub", x: NODE.w - (item.incidents ? 22 : 10), y: 16, "text-anchor": "end" }, "◉" + item.watchers) : null);
    state.nodes.set(item.key, group);
    return group;
  }

  // ---------------------------------------------------------------- the page

  async function flowsView(main) {
    if (needRepository(main)) return;
    injectStyle();
    let data;
    try { data = await api("/api/flows"); } catch (error) { if (error.status === 404) { main.append(h("div", { class: "notice" }, error.message)); return; } throw error; }
    data.flows = data.flows || []; data.zones = data.zones || []; data.drafts = data.drafts || []; data.proposals = data.proposals || [];
    data.watchers = data.watchers || []; data.incidents = data.incidents || []; data.requests = data.requests || []; data.bricks = data.bricks || [];
    data.assignments = data.assignments || [];

    const draftOnly = data.drafts.filter((draft) => !draft.exists);
    const items = [];
    for (const flow of data.flows) {
      const knowledge = [flow.priority || "no priority", (flow.context || []).length + " ctx", (flow.tests || []).length + " tests"].join(" · ");
      const drafted = data.drafts.some((draft) => draft.flow_id === flow.id);
      items.push({ key: "flow:" + flow.id, kind: "flow", id: flow.id, name: flow.id, priority: flow.priority, draft: false, incidents: flow.incidents, watchers: flow.watchers ? flow.watchers.active : 0,
        signals: knowledge + (drafted ? " · draft" : ""), title: flow.description || flow.id, zones: flow.stages || [], search: [flow.id, flow.description, ...(flow.entry_points || []), ...(flow.people || []).map((person) => person.name)].join(" ").toLowerCase() });
    }
    for (const draft of draftOnly) {
      const current = (draft.revisions || []).find((revision) => revision.revision === draft.current) || {};
      items.push({ key: "draft:" + draft.flow_id, kind: "draft", id: draft.flow_id, name: draft.flow_id, draft: true, incidents: 0, watchers: 0,
        signals: "draft r" + draft.current + " · " + (current.nodes || 0) + " fn · " + draft.via, title: plain(draft.description) || draft.flow_id, zones: [], search: [draft.flow_id, plain(draft.description), ...(current.from || [])].join(" ").toLowerCase() });
    }
    for (const zone of data.zones) {
      items.push({ key: "zone:" + zone.id, kind: "zone", id: zone.id, name: zone.id, draft: false, incidents: 0, watchers: 0,
        signals: [zone.criticality, zone.ceiling, (zone.files || 0) + " files"].join(" · "), title: zone.description || zone.id, zones: [], search: [zone.id, zone.description, zone.owner].join(" ").toLowerCase() });
    }
    const byKey = new Map(items.map((item) => [item.key, item]));
    const rank = { P0: 0, P1: 1, P2: 2, P3: 3 };
    const critical = { restricted: 0, critical: 1, sensitive: 2, routine: 3 };
    const zoneCriticality = new Map(data.zones.map((zone) => [zone.id, zone.criticality]));
    const bands = [
      ...["P0", "P1", "P2", "P3"].map((priority) => ({ label: "Flows · " + priority, items: items.filter((item) => item.kind === "flow" && item.priority === priority) })),
      { label: "Flows · no priority", items: items.filter((item) => item.kind === "flow" && !item.priority) },
      { label: "Drafts · waiting for review", items: items.filter((item) => item.kind === "draft") },
      { label: "Zones", items: items.filter((item) => item.kind === "zone").sort((a, b) => (critical[zoneCriticality.get(a.id)] ?? 9) - (critical[zoneCriticality.get(b.id)] ?? 9) || a.name.localeCompare(b.name)) },
    ];
    for (const band of bands) if (band.label.startsWith("Flows")) band.items.sort((a, b) => (rank[a.priority] ?? 9) - (rank[b.priority] ?? 9) || a.name.localeCompare(b.name));

    const state = { selected: null, query: "", tab: null, nodes: new Map(), lens: "human" };
    try { const saved = sessionStorage.getItem("fx-selected"); if (saved && byKey.has(saved)) state.selected = saved; } catch (error) { /* storage unavailable */ }

    // Header: what the repository's flow knowledge holds, at a glance
    const openIncidents = data.incidents.length;
    const pending = data.proposals.filter((proposal) => proposal.status === "PENDING").length;
    const active = data.watchers.filter((watcher) => watcher.active).length;
    const waitingRequests = data.requests.filter((row) => row.status === "PENDING").length;
    const stat = (label, value, alert) => h("span", { class: "fx-stat" + (alert ? " alert" : "") }, h("b", {}, String(value)), " " + label);
    const root = h("div", { class: "fx" });
    root.append(h("div", { class: "fx-head" },
      h("div", {}, h("h1", { class: "fx-title" }, "Flows"), h("div", { class: "fx-meta" }, "The repository's knowledge base for agents and people · reported by ", data.reported_by ? data.reported_by.actor : "a member", " · ", dt(data.reported_at))),
      h("div", { class: "fx-spacer" }),
      h("div", { class: "fx-stats" }, stat("flows", data.flows.length), stat("zones", data.zones.length), stat("drafts", data.drafts.length), stat("proposals", pending, pending > 0), stat("watchers", active), stat("incidents", openIncidents, openIncidents > 0), stat("requests waiting", waitingRequests)),
      h("button", { class: "fx-btn primary", type: "button", onclick: () => request("flow-trace", "Trace a new flow", { effect: "Crane follows the calls from a starting point through the repository and saves the result as a draft. The same code always gives the same trace; a code owner or admin approves it.", fields: [
        { key: "from", label: "Starting point (function, RPC handler, or path:Symbol)", hint: "api/pay.py:CheckoutService.submit", required: true },
        { key: "flow", label: "Flow id (made from the starting point when empty)", hint: "checkout" },
        { key: "depth", label: "Deepest call chain", kind: "number", value: 8 },
        { key: "description", label: "What the flow does", hint: "Pay for an order" }] }) }, "Trace a flow")));

    // The map
    const search = h("input", { type: "search", placeholder: "Search flows, zones, people, symbols…", "aria-label": "Search flows and zones", autocomplete: "off" });
    const mapBox = h("div", { class: "fx-map" });
    const canvas = h("section", { class: "fx-canvas", "aria-label": "Map of flows and zones" },
      h("div", { class: "fx-toolbar" },
        h("label", { class: "fx-search" }, h("span", { "aria-hidden": "true", style: "color:var(--muted)" }, "⌕"), search, h("kbd", {}, "/")),
        h("div", { class: "fx-legend" }, h("span", {}, h("i", { style: "background:var(--bad)" }), "P0"), h("span", {}, h("i", { style: "background:var(--warn)" }), "P1"), h("span", {}, h("i", { style: "background:var(--info)" }), "other flows"), h("span", {}, h("i", { style: "background:var(--low)" }), "zone"), h("span", {}, "◉ watchers"), h("span", {}, h("i", { style: "background:var(--bad);border-radius:50%" }), "incident")),
        h("span", { class: "fx-spacer" }),
        h("button", { class: "fx-btn ghost small", type: "button", onclick: () => pick(null) }, "Clear selection")),
      mapBox);
    root.append(canvas);
    const look = h("section", { class: "fx-look", "aria-live": "polite", "aria-label": "Lookout" });
    root.append(look);
    main.append(root);

    let geometry = null;
    function drawMap() {
      const width = Math.max(320, mapBox.clientWidth || main.clientWidth || 960);
      geometry = layout(bands, width);
      state.nodes = new Map();
      if (!items.length) {
        put(mapBox, h("div", { class: "fx-empty" }, "No flows or zones yet. Trace a flow from its starting point; once a code owner approves it, it appears here."));
        return;
      }
      const edges = svgNode("g", { class: "fx-edges" });
      const svg = svgNode("svg", { width: geometry.width, height: geometry.height, viewBox: `0 0 ${geometry.width} ${geometry.height}`, role: "group", "aria-label": "Flows and zones" },
        edges,
        geometry.labels.map((label) => svgNode("text", { class: "fx-band", x: NODE.pad, y: label.y }, label.text)),
        [...geometry.positions.values()].map((position) => node(position, state, pick)));
      state.edges = edges;
      put(mapBox, svg);
      highlight();
    }

    function related(key) {
      const item = byKey.get(key);
      if (!item) return new Set();
      if (item.kind === "flow") return new Set(item.zones.map((zone) => "zone:" + zone));
      if (item.kind === "zone") return new Set(items.filter((other) => other.kind === "flow" && other.zones.includes(item.id)).map((other) => other.key));
      return new Set();
    }

    function highlight() {
      const query = state.query.trim().toLowerCase();
      const near = state.selected ? related(state.selected) : new Set();
      for (const [key, group] of state.nodes) {
        const item = byKey.get(key);
        group.classList.toggle("selected", key === state.selected);
        group.classList.toggle("related", near.has(key));
        group.classList.toggle("dim", Boolean(query) && !item.search.includes(query) && !item.name.toLowerCase().includes(query));
      }
      // Lines only for the selection: a flow to the zones it runs through
      if (state.edges && geometry) {
        const lines = [];
        if (state.selected) {
          const from = geometry.positions.get(state.selected);
          for (const key of near) {
            const to = geometry.positions.get(key);
            if (!from || !to) continue;
            const [a, b] = from.y <= to.y ? [from, to] : [to, from];
            const x1 = a.x + NODE.w / 2, y1 = a.y + NODE.h, x2 = b.x + NODE.w / 2, y2 = b.y;
            lines.push(svgNode("path", { class: "fx-edge", d: `M${x1} ${y1} C ${x1} ${(y1 + y2) / 2}, ${x2} ${(y1 + y2) / 2}, ${x2} ${y2}` }));
          }
        }
        put(state.edges, lines);
      }
    }

    function pick(key) {
      state.selected = key;
      state.tab = null;
      try { if (key) sessionStorage.setItem("fx-selected", key); else sessionStorage.removeItem("fx-selected"); } catch (error) { /* storage unavailable */ }
      highlight();
      drawLook();
    }

    search.addEventListener("input", () => { state.query = search.value; highlight(); });
    search.addEventListener("keydown", (event) => {
      if (event.key !== "Enter") return;
      const query = search.value.trim().toLowerCase();
      const found = items.find((item) => item.name.toLowerCase() === query) || items.find((item) => item.search.includes(query) || item.name.toLowerCase().includes(query));
      if (found) { pick(found.key); const group = state.nodes.get(found.key); if (group) group.scrollIntoView({ block: "nearest", inline: "nearest" }); }
    });
    const slash = (event) => { if (event.key === "/" && document.activeElement && !["INPUT", "TEXTAREA", "SELECT"].includes(document.activeElement.tagName) && document.body.contains(search)) { event.preventDefault(); search.focus(); } };
    document.addEventListener("keydown", slash);

    // ---------------------------------------------------------------- the lookout

    function tabs(list) {
      if (!list.some(([id]) => id === state.tab)) state.tab = list[0][0];
      return h("div", { class: "fx-tabs", role: "tablist" }, list.map(([id, label, count]) => h("button", { class: "fx-tab", role: "tab", type: "button", "aria-selected": String(id === state.tab), onclick: () => { state.tab = id; drawLook(); } }, label, count !== undefined ? h("span", { class: "n" }, String(count)) : null)));
    }

    function requestRows(filter) {
      const rows = data.requests.filter(filter).slice(0, 12);
      if (!rows.length) return null;
      return h("div", { class: "fx-section" }, h("h3", {}, "Requests made here"), h("div", { class: "fx-list" }, rows.map((row) => h("div", { class: "fx-row" },
        h("div", { class: "main" }, h("span", { class: "title" }, row.op.replace(/^flow-/, "").replace(/-/g, " "), " ", h("code", {}, (row.params && (row.params.flow || row.params.proposal || row.params.watcher)) || "")), h("span", { class: "meta" }, "@" + row.requested_by + " · " + dt(row.requested_at) + (row.detail ? " · " + row.detail : ""))),
        h("div", { class: "acts" }, chip(row.status === "PENDING" && row.expires_at && row.expires_at < Date.now() ? "EXPIRED" : row.status))))));
    }

    function drawLook() {
      const key = state.selected;
      const item = key ? byKey.get(key) : null;
      if (!item) return put(look, overview());
      if (item.kind === "zone") return put(look, zoneLook(data.zones.find((zone) => zone.id === item.id)));
      if (item.kind === "draft") return put(look, draftLook(data.drafts.find((draft) => draft.flow_id === item.id)));
      return put(look, flowLook(data.flows.find((flow) => flow.id === item.id)));
    }

    // At rest: the repository-wide view (models, routing, limits, proposals, incidents, requests)
    function overview() {
      const body = h("div", { class: "fx-body" },
        h("p", { class: "fx-note", style: "margin:0" }, "Select a flow or zone on the map. Agents read the same knowledge through their hooks ('crane-request flow ID'); nothing an agent asks is ever run."),
        h("div", { class: "fx-split" },
          h("div", { class: "fx-section" }, h("h3", {}, "Model routing", h("span", { class: "fx-spacer" }), h("span", { class: "fx-note" }, "priority → tier → model")),
            h("div", { class: "fx-list" }, (data.routing || []).map((route) => h("div", { class: "fx-row" }, h("div", { class: "main" }, h("span", { class: "title" }, pill(route.priority, tone(route.priority)), " ", route.model || h("span", { class: "muted" }, "no available model")), h("span", { class: "meta" }, "tiers: " + route.tiers.join(", "))))))),
          h("div", { class: "fx-section" }, h("h3", {}, "Limits agents work within"),
            h("div", { class: "fx-grid" }, field("Watchers per agent session", String(data.limits ? data.limits.max_watchers_per_session : "—")), field("Watchers per flow", String(data.limits ? data.limits.max_watchers_per_flow : "—"))),
            h("div", {}, h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-limit", "Change the limits agents work within", { effect: "Only a person changes these. Agents start watchers by themselves only up to them.", fields: [{ key: "session", label: "Watchers one agent session may start", kind: "number", value: data.limits && data.limits.max_watchers_per_session }, { key: "per_flow", label: "Watchers per flow", kind: "number", value: data.limits && data.limits.max_watchers_per_flow }] }) }, "Change limits…")),
            h("p", { class: "fx-note", style: "margin:0" }, (data.models || []).length + " provider model(s) listed for the organization."))),
        proposalSection(() => true),
        incidentSection(() => true),
        requestRows(() => true));
      return [h("div", { class: "fx-look-head" }, h("span", { class: "fx-kind" }, "Repository"), h("h2", { class: "fx-look-title" }, repositoryName())), body];
    }

    function proposalSection(filter) {
      const rows = data.proposals.filter((proposal) => proposal.status === "PENDING" && filter(proposal));
      if (!rows.length) return null;
      return h("div", { class: "fx-section" }, h("h3", {}, "Proposals from agents", h("span", { class: "fx-spacer" }), pill(rows.length + " waiting", "accent")),
        h("div", { class: "fx-list" }, rows.map((proposal) => {
          const payload = proposal.payload || {};
          const test = proposal.kind === "test";
          return h("div", { class: "fx-row" },
            h("div", { class: "main" },
              h("span", { class: "title" }, pill(proposal.kind), " ", test ? plain(payload.name) : plain(payload.title), " ", h("span", { class: "muted" }, "for " + proposal.flow)),
              h("span", { class: "meta" }, test ? [plain(payload.path), plain(payload.description)].filter(Boolean).join(" — ") : [payload.kind ? plain(payload.kind) : "", plain(payload.value)].filter(Boolean).join(": ")),
              test && (payload.assumptions || []).length ? h("span", { class: "meta" }, "Assumes: " + payload.assumptions.map(plain).join("; ")) : null,
              test && plain(payload.mock_data) ? h("span", { class: "meta" }, "Mock data: " + plain(payload.mock_data)) : null,
              h("span", { class: "meta" }, "by " + ((proposal.proposed_by || {}).agent || "an agent") + " · session " + ((proposal.proposed_by || {}).session || "?") + " · " + dt(proposal.proposed_at))),
            h("div", { class: "acts" },
              h("button", { class: "fx-btn small primary", type: "button", onclick: () => request("flow-proposal-approve", "Approve " + proposal.proposal_id, { params: { proposal: proposal.proposal_id }, effect: test ? "Attaches the test to the flow's signed knowledge." : "Adds the context to the flow's signed knowledge.", fields: test ? [{ key: "required", label: "Require it in every release ('crane flow tests run --required')", kind: "check" }] : [] }) }, "Approve…"),
              h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-proposal-reject", "Reject " + proposal.proposal_id, { params: { proposal: proposal.proposal_id }, effect: "The agent's session is told why with its next tool call." }) }, "Reject…")));
        })));
    }

    function incidentSection(filter) {
      const rows = data.incidents.filter(filter).slice(0, 20);
      if (!rows.length) return null;
      return h("div", { class: "fx-section" }, h("h3", {}, "Incidents", h("span", { class: "fx-spacer" }), pill(rows.length, "bad")),
        h("div", { class: "fx-list" }, rows.map((incident) => h("div", { class: "fx-row" },
          h("div", { class: "main" }, h("span", { class: "title" }, pill(incident.code, "bad"), " ", incident.summary), h("span", { class: "meta" }, incident.flow + " · " + incident.raised_by + " · " + dt(incident.raised_at) + (incident.notified && incident.notified.length ? " · told " + incident.notified.join(", ") : " · nobody told yet"))),
          h("div", { class: "acts" }, h("code", { class: "muted" }, incident.incident_id))))));
    }

    function zoneLook(zone) {
      const through = data.flows.filter((flow) => (flow.stages || []).includes(zone.id));
      const head = h("div", { class: "fx-look-head" }, h("span", { class: "fx-kind" }, "Zone"), h("h2", { class: "fx-look-title" }, zone.id), pill(zone.status), pill(zone.criticality, zone.criticality === "critical" || zone.criticality === "restricted" ? "bad" : ""));
      const list = [["overview", "Overview"], ["flows", "Flows through it", through.length], ["bricks", "Bricks", (zone.bricks || []).length]];
      const nav = tabs(list);
      let body;
      if (state.tab === "flows") body = through.length ? h("div", { class: "fx-list" }, through.map((flow) => h("div", { class: "fx-row" }, h("div", { class: "main" }, h("span", { class: "title" }, flow.id, " ", flow.priority ? pill(flow.priority, tone(flow.priority)) : null), h("span", { class: "meta" }, flow.description || "")), h("div", { class: "acts" }, h("button", { class: "fx-btn small", type: "button", onclick: () => pick("flow:" + flow.id) }, "Open"))))) : h("p", { class: "fx-note" }, "No flow lists this zone among its stages.");
      else if (state.tab === "bricks") body = bricksList(zone.bricks || []);
      else body = h("div", { class: "fx-grid" }, field("Description", zone.description), field("Criticality", zone.criticality), field("Autonomy ceiling", zone.ceiling), field("Status", zone.status), field("Files covered", zone.files === null || zone.files === undefined ? "" : String(zone.files)), field("Owner", zone.owner));
      return [head, nav, h("div", { class: "fx-body" }, body)];
    }

    function bricksList(refs) {
      if (!refs.length) return h("p", { class: "fx-note" }, "No Brick is attached. Attach one on the Bricks page.");
      return h("div", { class: "fx-list" }, refs.map((ref) => {
        const brick = data.bricks.find((row) => "brick:" + row.ref === ref || row.ref === ref.replace(/^brick:/, ""));
        return h("div", { class: "fx-row" }, h("div", { class: "main" }, h("span", { class: "title" }, brick ? brick.name : ref.replace(/^(brick|stack):/, ""), " ", brick ? pill(brick.type) : pill(ref.startsWith("stack:") ? "stack" : "brick")), h("span", { class: "meta" }, (brick && brick.intent) || ref)), h("div", { class: "acts" }, brick ? chip(brick.state) : null));
      }));
    }

    function draftLook(draft) {
      const head = h("div", { class: "fx-look-head" }, h("span", { class: "fx-kind" }, "Draft flow"), h("h2", { class: "fx-look-title" }, draft.flow_id), pill("revision " + draft.current + " of " + draft.revisions.length), pill("by " + draft.via));
      return [head, h("div", { class: "fx-body" }, traceDraft(draft))];
    }

    /** The draft's current revision: its revisions, the call tree, what changed, and the actions */
    function traceDraft(draft) {
      const current = draft.revisions.find((revision) => revision.revision === draft.current) || {};
      const graph = draft.graph || { nodes: [], edges: [], unresolved: [] };
      const added = new Set(((draft.diff || {}).added_nodes) || []);
      const children = new Map();
      for (const edge of graph.edges || []) { if (!children.has(edge.from)) children.set(edge.from, []); children.get(edge.from).push(edge.to); }
      const byId = new Map((graph.nodes || []).map((entry) => [entry.id, entry]));
      const lines = [];
      const seen = new Set();
      const walk = (id, depth) => {
        const entry = byId.get(id);
        if (!entry || lines.length > 300) return;
        const again = seen.has(id);
        seen.add(id);
        lines.push(h("div", { class: added.has(id) ? "add" : "" }, "  ".repeat(depth) + (depth ? "└ " : "") + entry.symbol + " ", h("span", { class: "file" }, entry.path + ":" + entry.start_line + "-" + entry.end_line + (again ? " (above)" : ""))));
        if (!again) for (const next of children.get(id) || []) walk(next, depth + 1);
      };
      for (const entry of graph.entry_points || []) walk(entry, 0);
      const diff = draft.diff;
      return [
        h("div", { class: "fx-grid" }, field("Starts at", (current.from || []).join(", ")), field("Status", current.status), field("Functions · calls · files", [current.nodes, current.edges, current.files].join(" · ")), field("Digest", h("code", {}, String(current.digest || "").slice(0, 16))), field("Traced", current.traced_at ? dt(current.traced_at) + " by " + current.by : ""), field("Description", shared(draft.description))),
        diff ? h("p", { class: "fx-note", style: "margin:0" }, "Since the revision before: +" + diff.added_nodes.length + " / −" + diff.removed_nodes.length + " functions, +" + diff.added_edges.length + " / −" + diff.removed_edges.length + " calls (new functions in green).") : null,
        h("div", { class: "fx-tree", role: "tree", "aria-label": "Call tree" }, lines.length ? lines : h("span", { class: "muted" }, "No functions."), graph.truncated ? h("div", { class: "file" }, "… cut at 400 functions") : null),
        (graph.unresolved || []).length ? h("details", {}, h("summary", { class: "fx-note" }, graph.unresolved.length + " call(s) not followed (external, dynamic, or ambiguous)"), h("div", { class: "fx-code" }, graph.unresolved.join("\n"))) : null,
        h("div", { class: "fx-section" }, h("h3", {}, "Revisions"), h("div", { class: "fx-list" }, draft.revisions.slice().reverse().map((revision) => h("div", { class: "fx-row" },
          h("div", { class: "main" }, h("span", { class: "title" }, "r" + revision.revision + (revision.revision === draft.current ? " · under review" : "")), h("span", { class: "meta" }, revision.nodes + " functions · " + revision.edges + " calls · " + revision.status + " · " + dt(revision.traced_at) + " · " + (revision.reason || ""))),
          h("div", { class: "acts" }, revision.revision === draft.current ? null : h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-rewind", "Review revision " + revision.revision + " of " + draft.flow_id, { params: { flow: draft.flow_id, revision: revision.revision }, effect: "Makes this revision the one under review. Later revisions are kept." }) }, "Rewind here…")))))),
        h("div", { style: "display:flex;flex-wrap:wrap;gap:8px" },
          h("button", { class: "fx-btn primary", type: "button", onclick: () => request("flow-approve", "Approve " + draft.flow_id, { params: { flow: draft.flow_id }, effect: draft.exists ? "Records this trace in the flow's signed knowledge." : "Adds the flow to .crane/governance.yaml with a context pack written from its trace, and records the trace in the signed knowledge.", fields: draft.exists ? [] : [{ key: "proposed", label: "Add it as proposed (not enforced yet)", kind: "check" }] }) }, "Approve…"),
          h("button", { class: "fx-btn", type: "button", onclick: () => request("flow-retrace", "Trace " + draft.flow_id + " again", { params: { flow: draft.flow_id }, effect: "A new revision from the same starting points, after the code changed." }) }, "Retrace…"),
          h("button", { class: "fx-btn danger", type: "button", onclick: () => request("flow-discard", "Discard the draft of " + draft.flow_id, { params: { flow: draft.flow_id } }) }, "Discard…")),
      ];
    }

    function flowLook(flow) {
      const draft = data.drafts.find((candidate) => candidate.flow_id === flow.id);
      const watchers = data.watchers.filter((watcher) => watcher.flow === flow.id);
      const agents = data.assignments.filter((assignment) => (assignment.targets || []).includes("flow:" + flow.id));
      const proposals = data.proposals.filter((proposal) => proposal.flow === flow.id && proposal.status === "PENDING");
      const head = h("div", { class: "fx-look-head" }, h("span", { class: "fx-kind" }, "Flow"), h("h2", { class: "fx-look-title" }, flow.id),
        flow.priority ? pill(flow.priority, tone(flow.priority)) : pill("no priority"), pill(flow.status), flow.trace ? pill("traced r" + flow.trace.revision, "ok") : pill("not traced"), draft ? pill("draft r" + draft.current, "accent") : null,
        h("span", { class: "fx-spacer" }),
        h("div", { style: "display:inline-flex;gap:4px", role: "group", "aria-label": "Lens" }, ["human", "agent"].map((lens) => h("button", { class: "fx-btn small" + (state.lens === lens ? " primary" : " ghost"), type: "button", "aria-pressed": String(state.lens === lens), onclick: () => { state.lens = lens; drawLook(); } }, lens === "human" ? "For people" : "For agents"))));
      if (state.lens === "agent") return [head, h("div", { class: "fx-body" }, agentLens(flow, watchers))];
      const nav = tabs([["overview", "Overview"], ["trace", "Trace", draft ? "draft" : undefined], ["context", "Context", (flow.context || []).length], ["tests", "Tests", (flow.tests || []).length + (proposals.filter((proposal) => proposal.kind === "test").length ? "+" : "")], ["people", "People & agents", (flow.people || []).length + watchers.filter((watcher) => watcher.active).length], ["bricks", "Bricks", (flow.bricks || []).length]]);
      const body = h("div", { class: "fx-body" });
      if (state.tab === "trace") put(body, draft ? traceDraft(draft) : null, flow.trace ? h("div", { class: "fx-section" }, h("h3", {}, "Approved trace"), h("div", { class: "fx-grid" }, field("Starts at", flow.trace.from.join(", ")), field("Revision", String(flow.trace.revision)), field("Functions · calls", flow.trace.nodes + " · " + flow.trace.edges), field("Status", flow.trace.status), field("Approved", dt(flow.trace.approved_at) + " by " + flow.trace.approved_by), field("Digest", h("code", {}, flow.trace.digest.slice(0, 16)))), h("div", { class: "fx-code" }, flow.trace.files.join("\n"))) : h("p", { class: "fx-note" }, "No approved trace yet."),
        draft ? null : h("div", {}, h("button", { class: "fx-btn", type: "button", onclick: () => request("flow-retrace", "Trace " + flow.id + " again", { params: { flow: flow.id }, effect: "Starts a draft revision from its approved starting points. A watcher with the trace_drift check tells you when the code moved." }) }, "Retrace…")));
      else if (state.tab === "context") put(body, contextTab(flow), proposalSection((proposal) => proposal.flow === flow.id && proposal.kind === "context"));
      else if (state.tab === "tests") put(body, testsTab(flow), proposalSection((proposal) => proposal.flow === flow.id && proposal.kind === "test"));
      else if (state.tab === "people") put(body, peopleTab(flow, watchers, agents), incidentSection((incident) => incident.flow === flow.id));
      else if (state.tab === "bricks") put(body, bricksList(flow.bricks || []));
      else put(body, overviewTab(flow), requestRows((row) => row.params && row.params.flow === flow.id));
      return [head, nav, body];
    }

    function overviewTab(flow) {
      const routed = flow.routed || {};
      return [
        h("div", { class: "fx-grid" }, field("Description", flow.description), field("Starts at", (flow.entry_points || []).join(", ")), field("Criticality · ceiling", flow.criticality + " · " + flow.ceiling), field("Files covered", flow.files === null || flow.files === undefined ? "" : String(flow.files)), field("Stages", (flow.stages || []).join(" → ")), field("Owner", flow.owner)),
        h("div", { class: "fx-section" }, h("h3", {}, "Priority and model", h("span", { class: "fx-spacer" }), h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-priority", "Priority of " + flow.id, { params: { flow: flow.id }, effect: "The priority routes agents' work to the organization's model tiers. Enforced, a change from a model outside them needs a person.", fields: [{ key: "priority", label: "Priority", kind: "select", value: flow.priority || "none", options: [["P0", "P0 · critical"], ["P1", "P1 · high"], ["P2", "P2 · normal"], ["P3", "P3 · low"], ["none", "No priority"]] }, { key: "enforce_model", label: "Model routing", kind: "select", value: flow.enforce_model ? "on" : "off", options: [["off", "Guidance only"], ["on", "Enforced: other models need a person"]] }] }) }, "Change…")),
          h("div", { class: "fx-grid" }, field("Priority", flow.priority ? pill(flow.priority, tone(flow.priority)) : ""), field("Routed to", routed.model ? routed.model + " (" + routed.provider + ")" : flow.priority ? "no available model of its tiers" : ""), field("Tiers", (routed.tiers || []).join(", ")), field("Routing", flow.enforce_model ? "enforced" : "guidance only"))),
      ];
    }

    function contextTab(flow) {
      const kinds = [["query", "Query"], ["metric", "Metric"], ["latency", "Latency"], ["file", "File"], ["note", "Note"], ["link", "Link"]];
      return h("div", { class: "fx-section" }, h("h3", {}, "Context agents receive", h("span", { class: "fx-spacer" }), h("button", { class: "fx-btn small primary", type: "button", onclick: () => request("flow-context-add", "Add context to " + flow.id, { params: { flow: flow.id }, effect: "Guidance agents receive when they work in this flow. It never authorizes anything; secrets are refused.", fields: [{ key: "kind", label: "Kind", kind: "select", value: "note", options: kinds }, { key: "title", label: "Title", required: true, max: 200 }, { key: "value", label: "Value (the query, number, note, or address)", kind: "textarea" }, { key: "unit", label: "Unit (ms for latency)", max: 20 }, { key: "threshold", label: "Keep within (a budget or limit)", kind: "number" }, { key: "file", label: "Repository file (optional)", hint: "app/pay.py" }, { key: "lines", label: "Lines (A-B)", max: 20 }] }) }, "Add context…")),
        (flow.context || []).length ? h("div", { class: "fx-list" }, flow.context.map((item) => h("div", { class: "fx-row" },
          h("div", { class: "main" }, h("span", { class: "title" }, pill(item.kind, "accent"), " ", item.title), h("div", { class: item.kind === "query" ? "fx-code" : "meta" }, shared(item.value), item.unit ? " " + item.unit : "", item.threshold !== null && item.threshold !== undefined ? " · keep within " + item.threshold : ""), item.file ? h("span", { class: "meta" }, h("code", {}, item.file + (item.lines ? ":" + item.lines : ""))) : null, h("span", { class: "meta" }, "added by " + item.added_by + " · " + dt(item.added_at))),
          h("div", { class: "acts" }, h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-context-remove", "Remove " + item.title, { params: { flow: flow.id, item: item.id } }) }, "Remove…"))))) : h("p", { class: "fx-note" }, "No context yet: queries, metrics, latency budgets, files, notes, and links agents should know."));
    }

    function testsTab(flow) {
      const tests = flow.tests || [];
      const required = tests.filter((test) => test.required).length;
      return h("div", { class: "fx-section" }, h("h3", {}, "Tests", h("span", { class: "fx-spacer" }), pill(required + " required in every release", required ? "accent" : "")),
        tests.length ? h("div", { class: "fx-list" }, tests.map((test) => h("div", { class: "fx-row" },
          h("div", { class: "main" },
            h("span", { class: "title" }, test.name, " ", test.required ? pill("required", "accent") : null, " ", test.last_result ? pill(test.last_result, test.last_result === "PASSED" ? "ok" : test.last_result === "FAILED" ? "bad" : "") : pill("not run")),
            h("span", { class: "meta" }, h("code", {}, test.path), test.exists ? "" : " · missing"),
            h("span", { class: "meta" }, shared(test.description)),
            (test.assumptions || []).length ? h("span", { class: "meta" }, "Assumes: ", test.assumptions.map(plain).join("; ")) : null,
            plain(test.mock_data) ? h("span", { class: "meta" }, "Mock data: ", shared(test.mock_data)) : null,
            h("span", { class: "meta" }, "written by " + test.proposed_by + " · approved by " + test.approved_by + " · " + dt(test.approved_at))),
          h("div", { class: "acts" }, h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-test-require", (test.required ? "Stop requiring " : "Require ") + test.name, { params: { flow: flow.id, test: test.id, required: !test.required }, effect: test.required ? "Releases no longer fail on it." : "'crane flow tests run --required' fails while it fails, so a release step stops." }) }, test.required ? "Not required…" : "Require…"))))) : h("p", { class: "fx-note" }, "No tests yet. Agents propose them with their assumptions and mock data ('crane-request test …'); a person approves each one here."));
    }

    function peopleTab(flow, watchers, agents) {
      const active = watchers.filter((watcher) => watcher.active);
      const limit = (flow.watchers || {}).limit || 0;
      return [
        h("div", { class: "fx-section" }, h("h3", {}, "Designated people", h("span", { class: "fx-spacer" }), h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-person-add", "Designate a person on " + flow.id, { params: { flow: flow.id }, effect: "They are told when a watcher raises an incident (Slack, and the Jira issue that names them).", fields: [{ key: "name", label: "Name", required: true, max: 120 }, { key: "role", label: "Role", kind: "select", value: "owner", options: [["author", "Author (wrote the code)"], ["owner", "Owner"], ["reviewer", "Reviewer"], ["oncall", "On call"]] }, { key: "github", label: "GitHub login" }, { key: "jira", label: "Jira account or issue (PAY-12)" }, { key: "slack", label: "Slack member id (U…)" }] }) }, "Designate…")),
          (flow.people || []).length ? h("div", { class: "fx-list" }, flow.people.map((person) => h("div", { class: "fx-row" }, h("div", { class: "main" }, h("span", { class: "title" }, person.name, " ", pill(person.role)), h("span", { class: "meta" }, [person.github ? "GitHub @" + person.github : "", person.jira ? "Jira " + person.jira : "", person.slack ? "Slack " + person.slack : "", "from " + person.source].filter(Boolean).join(" · "))), h("div", { class: "acts" }, h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-person-remove", "Remove " + person.name, { params: { flow: flow.id, person: person.id } }) }, "Remove…"))))) : h("p", { class: "fx-note" }, "Nobody is designated. 'crane flow people suggest " + flow.id + "' lists the Git authors of its files.")),
        h("div", { class: "fx-section" }, h("h3", {}, "Watchers (monitoring subagents)", h("span", { class: "fx-spacer" }), h("button", { class: "fx-btn small primary", type: "button", disabled: active.length >= limit ? "" : undefined, title: active.length >= limit ? "At the limit; raise it first" : undefined, onclick: () => request("flow-watch-add", "Start a watcher on " + flow.id, { params: { flow: flow.id }, effect: "A read-only watcher runs Crane's checks of this flow without a model. It takes on the flow's Bricks and ceiling, and raises an incident to the designated people.", fields: [{ key: "purpose", label: "Why it watches", required: true }, { key: "trace_drift", label: "The call graph drifts from the approved trace", kind: "check", value: true }, { key: "required_tests", label: "A required test fails or goes missing", kind: "check", value: true }, { key: "denials", label: "Agents are denied repeatedly", kind: "check", value: true }], shape: (params) => { const checks = ["trace_drift", "required_tests", "denials"].filter((check) => params[check]); for (const check of ["trace_drift", "required_tests", "denials"]) delete params[check]; return checks; } }) }, "Start a watcher…"),
          h("button", { class: "fx-btn small", type: "button", onclick: () => request("flow-limit", "Watcher limit of " + flow.id, { params: { flow: flow.id }, effect: "Only a person changes it; a flow cannot go above the repository's limit.", fields: [{ key: "watchers", label: "Watchers on this flow", kind: "number", value: limit }] }) }, "Limit…")),
          h("div", { class: "fx-grid" }, field("Active", active.length + " of " + limit + " allowed"), field("Started by agents", String(active.filter((watcher) => watcher.created_by === "agent").length))),
          h("div", { class: "fx-meter", role: "img", "aria-label": active.length + " of " + limit + " watchers" }, h("i", { style: "width:" + (limit ? Math.min(100, (active.length / limit) * 100) : 0) + "%" })),
          watchers.length ? h("div", { class: "fx-list" }, watchers.map((watcher) => h("div", { class: "fx-row" },
            h("div", { class: "main" }, h("span", { class: "title" }, watcher.purpose, " ", watcher.active ? pill("read-only", "ok") : pill("stopped"), (watcher.firing || []).map((code) => pill(code, "bad"))), h("span", { class: "meta" }, (watcher.created_by === "agent" ? "started by " + watcher.parent.agent + " (session " + watcher.parent.session + (watcher.parent.model ? ", " + watcher.parent.model : "") + ")" : "started by " + watcher.created_by) + " · " + dt(watcher.created_at)), h("span", { class: "meta" }, "checks " + watcher.checks.join(", ") + " · inherits " + (watcher.inherited.bricks.length ? watcher.inherited.bricks.join(", ") : "no Bricks") + ", ceiling " + watcher.inherited.ceiling + (watcher.last_run ? " · last ran " + dt(watcher.last_run) : ""))),
            h("div", { class: "acts" }, watcher.active ? h("button", { class: "fx-btn small danger", type: "button", onclick: () => request("flow-watch-stop", "Stop " + watcher.watcher_id, { params: { watcher: watcher.watcher_id } }) }, "Stop…") : null)))) : h("p", { class: "fx-note" }, "No watcher yet. Agents start one with 'crane-request watch --flow " + flow.id + "' up to the limit.")),
        h("div", { class: "fx-section" }, h("h3", {}, "Agents assigned"), agents.length ? h("div", { class: "fx-list" }, agents.map((agent) => h("div", { class: "fx-row" }, h("div", { class: "main" }, h("span", { class: "title" }, agent.agent), h("span", { class: "meta" }, (agent.targets || []).join(", "))), h("div", { class: "acts" }, chip(agent.assignment_status), chip(agent.runtime_status))))) : h("p", { class: "fx-note" }, "No agent is assigned to this flow (assign one on the Bricks page).")),
      ];
    }

    /** What an agent receives about the flow, and what it may ask */
    function agentLens(flow, watchers) {
      const lines = ["Flow " + flow.id + ": " + (flow.description || "no description") + " (criticality " + flow.criticality + ", autonomy ceiling " + flow.ceiling + ", " + flow.status + ")", "Starts at: " + (flow.entry_points || []).join(", ")];
      if (flow.priority) lines.push("Priority " + flow.priority + ": model tiers " + ((flow.routed || {}).tiers || []).join(", ") + ((flow.routed || {}).model ? " (routed to " + flow.routed.model + " by " + flow.routed.provider + ")" : ""));
      if (flow.trace) lines.push("Approved trace (revision " + flow.trace.revision + ", " + flow.trace.status + "): " + flow.trace.nodes + " functions, " + flow.trace.edges + " calls in " + flow.trace.files.length + " files");
      if ((flow.people || []).length) lines.push("People: " + flow.people.map((person) => person.name + " (" + person.role + ")").join("; "));
      for (const item of flow.context || []) lines.push("Context [" + item.kind + "] " + item.title + ": " + plain(item.value) + (item.unit ? " " + item.unit : "") + (item.threshold !== null && item.threshold !== undefined ? " (keep within " + item.threshold + ")" : ""));
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
        h("p", { class: "fx-note", style: "margin:0" }, "What an agent receives when it asks for this flow (and, shorter, with each tool call that touches it). Rebuilt here from the last report; the machine's answer is the authority."),
        h("div", { class: "fx-code" }, lines.join("\n")),
        h("div", { class: "fx-section" }, h("h3", {}, "What an agent may ask"), h("div", { class: "fx-list" }, asks.map((ask) => h("div", { class: "fx-row" }, h("div", { class: "main" }, h("code", {}, ask)), h("div", { class: "acts" }, h("button", { class: "fx-btn small ghost", type: "button", onclick: copy(ask) }, "Copy")))))),
        h("p", { class: "fx-note", style: "margin:0" }, "The hook answers these and stops the command: nothing runs. Traces, tests, and context are drafts and proposals a person approves; watchers are read-only and capped by the limits only a person changes."),
      ];
    }

    drawMap();
    drawLook();
    const resize = () => { if (!document.body.contains(root)) { window.removeEventListener("resize", resize); document.removeEventListener("keydown", slash); return; } drawMap(); };
    window.addEventListener("resize", resize);
  }

  Object.assign(window.FOXX_PAGES, { flows: flowsView });
})();
