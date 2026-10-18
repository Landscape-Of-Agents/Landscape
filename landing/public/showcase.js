"use strict";
// The product showcase: Crane (the CLI), Landscape (the editor extension), and DataSea (the
// dashboard), one row each, each drawn as a 3D object (a stack of glass layers at different depths
// that sways and leans with the pointer). Every scene is a 5-second timeline: a pure function of
// its time, so it can play at any speed and in either direction. It plays at 1x while its row is on
// screen, at 1.15x while the page scrolls down, and backwards while it scrolls up; at the end it
// holds, then starts again. Under reduced motion each row shows its finished state.
(() => {
  const rowsInPage = [...document.querySelectorAll(".show-row")];
  if (!rowsInPage.length) return;
  const STILL = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const DURATION = 5;
  const HOLD = 1.6;
  const LOGO = "logo.png?v=3";
  const SVG = "http://www.w3.org/2000/svg";
  const clamp = (value, low = 0, high = 1) => Math.min(high, Math.max(low, value));
  const make = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  };
  const svg = (tag, attributes) => {
    const node = document.createElementNS(SVG, tag);
    for (const [key, value] of Object.entries(attributes || {})) node.setAttribute(key, String(value));
    return node;
  };
  const ICON = (path) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${path}</svg>`;

  // A timeline: tracks that switch a class on over time ranges, type text over a span of time, or
  // run a function of the time. Drawing it at any time gives the same frame, forward or backward.
  function timeline(root) {
    const tracks = [];
    const api = {
      dirty: true,
      q: (selector) => root.querySelector(selector),
      qa: (selector) => [...root.querySelectorAll(selector)],
      // A class while the time is inside any of the ranges ([from, to) pairs; to defaults to forever)
      on(node, ranges, className = "on") {
        const list = typeof ranges === "number" ? [[ranges, Infinity]] : typeof ranges[0] === "number" ? [ranges] : ranges;
        tracks.push({ kind: "on", node, list, className, last: null });
      },
      type(node, text, from, length) { tracks.push({ kind: "type", node, text, from, length, last: -1 }); },
      each(draw) { tracks.push({ kind: "each", draw }); },
      draw(time) {
        for (const track of tracks) {
          if (track.kind === "on") {
            const on = track.list.some(([from, to = Infinity]) => time >= from && time < to);
            if (on !== track.last) { track.node.classList.toggle(track.className, on); track.last = on; api.dirty = true; }
          } else if (track.kind === "type") {
            const count = Math.round(clamp((time - track.from) / track.length) * track.text.length);
            if (count !== track.last) { track.node.textContent = track.text.slice(0, count); track.last = count; api.dirty = true; }
          } else {
            track.draw(time);
          }
        }
      },
    };
    return api;
  }

  // ---------------------------------------------------------------- Crane: three terminals, one codebase
  const k = (text) => `<span class="sh-k">${text}</span>`;
  const f = (text) => `<span class="sh-f">${text}</span>`;
  const s = (text) => `<span class="sh-s">${text}</span>`;
  const row = (html, className = "") => `<div class="sh-line ${className}"><i></i><span>${html}</span></div>`;
  // Each terminal names its layer class in full, so the style build keeps it
  const terminal = (n, layer, who, branch, depth, delay) => `
  <div class="ly ${layer}" data-ly="${n}" style="--z:${depth}px;--d:${delay}s">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">${who}</span><small>${branch}</small><span class="sh-tag ml-auto">tty${n}</span></div>
    <div class="sh-term" data-term="${n}"></div>
  </div>`;
  const CLI = {
    steps: [[0, "Three terminals share one codebase. You protect the fee"], [1.3, "A teammate targets the refund the task must change"], [2.4, "CI validates the rules; the test waits for the target"], [3.25, "An agent edits: the preserve holds, the target changes"], [4.3, "The same test now passes on every machine"]],
    html: () => `
  <div class="ly ly-file" data-file style="--z:0px">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">app/payments.py</span><small>acme/payments · one codebase</small><span class="sh-tag st ml-auto" data-tag-idle>3 terminals attached</span><span class="sh-tag live st ml-auto" data-tag-agent>claude-code editing</span></div>
    <div class="sh-code" data-code>
      ${row(`${k("from")} decimal ${k("import")} Decimal`)}
      ${row("")}
      <div class="sh-lens preserve ins" data-seal="fee">◆ K7M2P9RX · preserve · from you on main</div>
      ${row("# @crane:selection:K7M2P9RX:start", "anchor preserve ins")}
      ${row(`${k("def")} ${f("fee")}(amount):`, "fee")}
      ${row(`    ${k("return")} amount * Decimal(${s('"0.03"')})<span class="sh-ghost st" data-ghost>Decimal("0.025")</span>`, "fee")}
      ${row("# @crane:selection:K7M2P9RX:end", "anchor preserve ins")}
      ${row("")}
      <div class="sh-lens target ins" data-seal="refund">◆ 4QZ8M2LD · target · logical_bn · from ana on feature/refunds</div>
      ${row("# @crane:selection:4QZ8M2LD:start", "anchor target ins")}
      ${row(`${k("def")} ${f("refund")}(order):`, "refund")}
      ${row(`    ${k("return")} order.total<span data-refund></span>`, "refund")}
      ${row("# @crane:selection:4QZ8M2LD:end", "anchor target ins")}
      ${row("")}
      ${row(`${k("def")} ${f("charge")}(card, amount):`)}
      ${row(`    ${k("return")} gateway.capture(card, amount + fee(amount))`)}
    </div>
    <div class="sh-foot">
      <b class="st" data-foot="0">No selections yet</b>
      <b class="st" data-foot="1"><span class="text-green-300">⛨ 1 preserve</span> · K7M2P9RX sealed (sha-512 9f2c…e1a7)</b>
      <b class="st" data-foot="2"><span class="text-green-300">⛨ 1 preserve</span> · <span class="text-violet-300">◎ 1 target</span> · policy payments</b>
      <b class="st text-red-300" data-foot="3">✕ claude-code Edit denied before it ran · PRESERVED_SELECTION K7M2P9RX</b>
      <b class="st text-green-300" data-foot="4">✓ claude-code changed 4QZ8M2LD · logical_bn</b>
      <b class="st text-green-300" data-foot="5">✓ policy payments · 2 of 2 rules pass</b>
    </div>
  </div>
  ${terminal(1, "ly-t1", "you", "main", 70, 0.15)}
  ${terminal(2, "ly-t2", "ana", "feature/refunds", 125, 0.3)}
  ${terminal(3, "ly-t3", "ci", "github actions", 180, 0.45)}`,
    build(t) {
      const terms = [1, 2, 3].map((n) => t.q(`[data-term="${n}"]`));
      const code = t.q("[data-code]");
      const file = t.q("[data-file]");
      const out = (n, at, text, className = "") => t.on(terms[n - 1].appendChild(make("div", className, text)), at);
      const cmd = (n, at, length, text) => {
        const node = terms[n - 1].appendChild(make("div", "cmd"));
        const typed = node.appendChild(make("span"));
        const caret = node.appendChild(make("span", "sh-caret st"));
        t.on(node, at);
        t.type(typed, text, at + 0.04, length);
        t.on(caret, [at, at + length + 0.1]);
      };
      // A selection is sealed: its lens and anchor comments appear and its lines take the band
      const seal = (name, kind, at) => {
        const lens = t.q(`[data-seal="${name}"]`);
        t.on(lens, at);
        let node = lens.nextElementSibling;
        t.on(node, at);
        while ((node = node.nextElementSibling) && node.classList.contains(name)) t.on(node, at, kind);
        t.on(node, at);
      };
      // The footer says what the file holds now
      [[0, 1.15], [1.15, 2.3], [2.3, 3.45], [3.45, 4.2], [4.2, 4.65], [4.65]].forEach((range, index) => t.on(t.q(`[data-foot="${index}"]`), [range]));

      cmd(1, 0.1, 0.8, 'crane protect app/payments.py context "Fees are audited; keep the rate." start-line 3 end-line 4');
      out(1, 1.0, "Protected app/payments.py lines 3-4 as selection K7M2P9RX (policy payments, checkpoint baseline)");
      out(1, 1.08, "  added to policy payments: preserve K7M2P9RX;", "dim");
      seal("fee", "preserve", 1.15);

      cmd(2, 1.35, 0.75, 'crane target app/payments.py context "Subtract what was refunded." start-line 8 end-line 9 change_type logical_bn');
      out(2, 2.15, "Targeted app/payments.py lines 8-9 as selection 4QZ8M2LD (policy payments, checkpoint baseline)");
      out(2, 2.22, "  added to policy payments: target 4QZ8M2LD change_type logical_bn;", "dim");
      seal("refund", "target", 2.3);

      cmd(3, 2.45, 0.2, "crane validate");
      out(3, 2.7, "Crane validate: 1 policies, 2 commands, 2 selections, 1 checkpoints, registry generation 6 (38 files scanned)");
      out(3, 2.76, "Crane validate: PASS", "ok");
      cmd(3, 2.85, 0.12, "crane test .");
      out(3, 3.02, "  PASS preserve K7M2P9RX: preserved (app/payments.py lines 4-5)");
      out(3, 3.08, "  FAIL target 4QZ8M2LD change_type logical_bn: target not changed yet (app/payments.py lines 9-10)", "bad");
      out(3, 3.14, "Crane test: FAIL", "bad");

      // The agent: its edit of the fee is denied, its refund change lands
      t.on(t.q("[data-tag-idle]"), [[0, 3.25], [4.25]]);
      t.on(t.q("[data-tag-agent]"), [3.25, 4.25]);
      t.on(file, [3.25, 4.25], "hot");
      t.on(file, [3.45, 3.9], "alarm");
      t.on(t.q("[data-ghost]"), [3.3, 3.9]);
      for (const node of t.qa(".sh-line.fee")) t.on(node, [3.45, 3.9], "deny");
      const refund = t.q("[data-refund]");
      t.type(refund, " - order.refunded", 3.95, 0.3);
      t.on(refund, [3.95, 4.3], "sh-typed");
      for (const node of t.qa(".sh-line.refund")) t.on(node, 4.25, "done");

      cmd(3, 4.35, 0.12, "crane test .");
      out(3, 4.52, "  PASS preserve K7M2P9RX: preserved (app/payments.py lines 4-5)");
      out(3, 4.58, "  PASS target 4QZ8M2LD change_type logical_bn: target changed as required (app/payments.py lines 9-10)");
      out(3, 4.64, "Crane test: PASS", "ok");

      // Whoever is typing comes forward
      t.on(t.q('[data-ly="1"]'), [0, 1.3], "hot");
      t.on(t.q('[data-ly="2"]'), [1.3, 2.4], "hot");
      t.on(t.q('[data-ly="3"]'), [[2.4, 3.25], [4.3]], "hot");

      // After a change: number the lines in view and keep each terminal scrolled to its end
      return () => {
        let number = 0;
        for (const line of code.querySelectorAll(".sh-line")) {
          if (line.classList.contains("ins") && !line.classList.contains("on")) continue;
          line.firstElementChild.textContent = String(++number);
        }
        for (const term of terms) term.scrollTop = term.scrollHeight;
      };
    },
  };

  // ---------------------------------------------------------------- Landscape: the governance graph
  const NODES = {
    billing: { x: 115, y: 112, kind: "zone", label: "billing", sub: "ZONE · CRITICAL", at: 0.05 },
    ledger: { x: 320, y: 112, kind: "zone", label: "ledger", sub: "ZONE · HIGH", at: 0.15 },
    refunds: { x: 525, y: 112, kind: "zone", label: "refunds", sub: "ZONE · HIGH", at: 0.25 },
    K7M2P9RX: { x: 180, y: 252, kind: "preserve", label: "K7M2P9RX", sub: "PRESERVE · FEE RATE", in: "billing", at: 0.38 },
    "4QZ8M2LD": { x: 460, y: 252, kind: "target", label: "4QZ8M2LD", sub: "TARGET · LOGICAL_BN", in: "refunds", at: 0.5 },
  };
  const EXT = {
    steps: [[0, "Zones and selections appear as a graph"], [0.8, "Connect zones in execution order to create the flow checkout"], [2.0, "A connection back to billing would form a cycle: rejected and removed"], [3.0, "Sign context for the flow; agents get it on every tool call there"], [4.3, "Agents light up the flows they work in"]],
    html: () => `
  <div class="ly ly-vs" style="--z:0px">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">Landscape graph</span><small>acme · Visual Studio Code</small><img src="${LOGO}" alt="" class="ml-auto h-4 w-4"></div>
    <div class="sh-toolbar"><span class="sh-tool">◇ Fit</span><span class="sh-tool" data-connect>⇢ Connect zones</span><span class="sh-tool">⛨ Selections</span>
      <span class="ml-auto hidden truncate font-mono text-[10px] text-ink-3 sm:inline"><span class="st" data-hint="0">Click zones in execution order</span><span class="st" data-hint="1">Governance refuses flows that form a cycle</span><span class="st" data-hint="2">Connections deselected</span><span class="st" data-hint="3">1 agent active on checkout</span></span></div>
    <svg class="sh-graph" viewBox="0 0 640 320" preserveAspectRatio="xMidYMid meet" data-graph>
      <defs><marker id="sh-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 1 9 5 0 9z" fill="rgba(255,255,255,.75)"/></marker></defs>
      <g data-links></g><g data-edges></g><g data-nodes></g><g class="pulses" data-pulses></g>
    </svg>
  </div>
  <div class="ly ly-gstat sh-gstat" style="--z:95px;--d:.25s"><span class="sh-label">Governance graph</span><b data-gstat></b><div><span class="sh-badge ok st" data-acyclic>acyclic</span><span class="sh-badge bad st" data-cyclic>1 cyclic connection rejected</span><span class="sh-badge info st" data-ctx-none>no context yet</span><span class="sh-badge ok st" data-ctx-signed>context signed</span></div></div>
  <div class="ly ly-ctx later" data-ctx style="--z:40px">
    <div class="sh-bar"><img src="${LOGO}" alt="" class="h-4 w-4"><span class="sh-bar-title">Context · flow checkout</span><span class="sh-tag ml-auto">3 zones</span></div>
    <div class="sh-ctx-body">
      <span class="text-[10.5px] text-ink-2">Given to the agent whenever a tool call touches this flow</span>
      <div class="sh-field" data-text></div>
      <ol class="sh-plan"><li data-plan>Sign the context into governance.yaml</li><li data-plan>Attach it to every zone of the flow</li><li data-plan>Send it with each tool call in the flow</li></ol>
      <div class="flex items-center gap-2 pt-1"><span class="font-mono text-[10px] text-ink-3">Ctrl ↵</span><span class="sh-btn" data-sign><span class="st" data-sign-idle>Sign context</span><span class="st" data-sign-done>✓ Signed</span></span></div>
    </div>
  </div>`,
    build(t) {
      const nodes = {};
      for (const [id, node] of Object.entries(NODES)) {
        const outer = t.q("[data-nodes]").appendChild(svg("g", { transform: `translate(${node.x} ${node.y})` }));
        const group = outer.appendChild(svg("g", { class: "node " + node.kind }));
        group.innerHTML = `<rect class="ring" x="-72" y="-28" width="144" height="56" rx="15"/><rect class="box" x="-64" y="-21" width="128" height="42" rx="10"/><text class="t" y="-1">${node.label}</text><text class="sub" y="13">${node.sub}</text><g class="agent" transform="translate(44 -27)"><rect x="-26" y="-8" width="52" height="15" rx="7.5"/><text y="2.5">${node.kind === "zone" ? "claude-code" : ""}</text></g>`;
        nodes[id] = group;
        t.on(group, node.at);
        if (node.in) {
          const home = NODES[node.in];
          const link = t.q("[data-links]").appendChild(svg("path", { class: "link st", d: `M${node.x} ${node.y - 21} C${node.x} ${node.y - 70} ${home.x} ${home.y + 70} ${home.x} ${home.y + 21}` }));
          t.on(link, node.at + 0.08);
        }
      }
      // A path between two nodes: zone to zone along the row, or an arc back over the top
      const route = (from, to) => {
        const a = NODES[from], b = NODES[to];
        if (b.x > a.x) return `M${a.x + 64} ${a.y} C${a.x + 110} ${a.y} ${b.x - 110} ${b.y} ${b.x - 66} ${b.y}`;
        return `M${a.x} ${a.y - 22} C${a.x} ${a.y - 100} ${b.x} ${b.y - 100} ${b.x} ${b.y - 24}`;
      };
      // An edge that draws itself in (and out) as a function of the time
      const edge = (from, to, className, drawn) => {
        const path = t.q("[data-edges]").appendChild(svg("path", { class: "edge " + className, d: route(from, to), id: `sh-e-${from}-${to}` }));
        const length = path.getTotalLength();
        path.style.strokeDasharray = String(length);
        let last = -1;
        t.each((time) => {
          const shown = drawn(time);
          if (shown === last) return;
          last = shown;
          path.style.strokeDashoffset = String(length * (1 - shown));
          path.style.opacity = shown > 0 ? "1" : "0";
          if (shown > 0.97) path.setAttribute("marker-end", "url(#sh-arrow)");
          else path.removeAttribute("marker-end");
        });
        return path;
      };
      const label = (from, to, text, ranges, className = "") => {
        const a = NODES[from], b = NODES[to];
        const node = t.q("[data-edges]").appendChild(svg("text", { class: "elabel st " + className, x: (a.x + b.x) / 2, y: b.x > a.x ? a.y - 12 : a.y - 88 }));
        node.textContent = text;
        t.on(node, ranges);
      };
      const grow = (from, length) => (time) => clamp((time - from) / length);
      const first = edge("billing", "ledger", "", grow(1.05, 0.35));
      const second = edge("ledger", "refunds", "", grow(1.5, 0.35));
      edge("refunds", "billing", "bad", (time) => (time < 2.75 ? clamp((time - 2.2) / 0.3) : 1 - clamp((time - 2.75) / 0.2)));
      label("billing", "ledger", "checkout · 1", [1.4, 4.05]);
      label("billing", "ledger", "checkout ✓ · 1", 4.05, "signed");
      label("ledger", "refunds", "checkout · 2", [1.85, 4.05]);
      label("ledger", "refunds", "checkout ✓ · 2", 4.05, "signed");
      label("refunds", "billing", "✕ cycle · refunds → billing", [2.5, 2.75], "bad");
      for (const path of [first, second]) t.on(path, 4.05, "signed");

      // Picking zones in order, then deselecting them
      t.on(t.q("[data-connect]"), [0.8, 2.95]);
      t.on(nodes.billing, [[0.85, 2.0], [2.15, 2.75]], "pick");
      t.on(nodes.ledger, [1.0, 2.0], "pick");
      t.on(nodes.refunds, [[1.45, 2.0], [2.05, 2.75]], "pick");
      t.on(t.q("[data-acyclic]"), [[0, 2.45], [2.85]]);
      t.on(t.q("[data-cyclic]"), [2.45, 2.85]);
      [[0, 2.0], [2.0, 2.95], [2.95, 4.3], [4.3]].forEach((range, index) => t.on(t.q(`[data-hint="${index}"]`), [range]));
      const stat = t.q("[data-gstat]");
      let shown = "";
      t.each((time) => {
        const zones = ["billing", "ledger", "refunds"].filter((id) => time >= NODES[id].at).length;
        const selections = ["K7M2P9RX", "4QZ8M2LD"].filter((id) => time >= NODES[id].at).length;
        const text = `${zones} zones · ${time >= 1.4 ? 1 : 0} flows · ${selections} selections`;
        if (text !== shown) stat.textContent = shown = text;
      });

      // Context for the flow, written and signed
      t.on(t.q("[data-ctx]"), [3.0, 4.3]);
      const text = t.q("[data-text]");
      t.type(text, "Charge before posting to the ledger. Never refund an order twice.", 3.15, 0.6);
      t.on(text, [3.1, 3.8], "focus");
      t.qa("[data-plan]").forEach((step, index) => t.on(step, 3.8 + index * 0.1, "done"));
      t.on(t.q("[data-sign]"), [3.8, 4.05], "busy");
      t.on(t.q("[data-sign-idle]"), [0, 4.05]);
      t.on(t.q("[data-sign-done]"), 4.05);
      t.on(t.q("[data-ctx-none]"), [0, 4.05]);
      t.on(t.q("[data-ctx-signed]"), 4.05);

      // Agents at work on the flow
      t.on(nodes.billing, [4.3, 4.65], "hot");
      t.on(nodes.ledger, 4.65, "hot");
      t.on(t.q("[data-pulses]"), 4.3);
      if (!STILL) {
        [first, second].forEach((path, index) => {
          const dot = t.q("[data-pulses]").appendChild(svg("circle", { class: "pulse", r: 3.2 }));
          const motion = dot.appendChild(svg("animateMotion", { dur: "1.6s", begin: index * 0.8 + "s", repeatCount: "indefinite" }));
          motion.appendChild(svg("mpath", { href: "#" + path.id }));
        });
      }
    },
  };

  // ---------------------------------------------------------------- DataSea: metrics, logs, and traces
  const RAIL = ['<path d="M4 13h6V4H4zM14 20h6v-9h-6zM4 20h6v-3H4zM14 4v3h6V4z"/>', '<circle cx="11" cy="11" r="6"/><path d="m20 20-4.5-4.5"/>', '<path d="M5 6h14M5 10h14M5 14h9M5 18h6"/>', '<path d="M4 6h8M8 12h10M6 18h8"/>', '<circle cx="6" cy="12" r="2"/><circle cx="18" cy="6" r="2"/><circle cx="18" cy="18" r="2"/><path d="M8 12h4l4-5M12 12l4 5"/>', '<path d="M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z"/>'];
  // Log lines: [time in the scene, flow, event, who, tone]; the first five are there from the start
  const LOGS = [
    [-5, "F05", "TOOL_CALL_ALLOWED", "codex · BASH"], [-4, "F05", "TOOL_CALL_ALLOWED", "copilot · READ"], [-3, "F06", "TOOL_CALL_RESULT", "claude-code · EDIT"],
    [-2, "F05", "TOOL_CALL_APPROVAL_REQUIRED", "gemini · WRITE"], [-1, "F05", "TOOL_CALL_ALLOWED", "codex · EDIT"],
    [0.35, "F02", "COMMAND_COMPLETED", "maya · crane test"], [0.75, "F05", "TOOL_CALL_ALLOWED", "claude-code · READ"], [1.15, "F05", "TOOL_CALL_ALLOWED", "copilot · READ"],
    [1.55, "F06", "TOOL_CALL_RESULT", "codex · BASH"], [1.95, "F05", "TOOL_CALL_ALLOWED", "gemini · READ"], [2.35, "F05", "TOOL_CALL_ALLOWED", "codex · EDIT"],
    [3.1, "F05", "TOOL_CALL_DENIED_RULE", "claude-code · EDIT", "bad"], [3.6, "F06", "TOOL_CALL_RESULT", "copilot · READ"], [4.1, "F05", "TOOL_CALL_ALLOWED", "codex · BASH"], [4.55, "F05", "TOOL_CALL_ALLOWED", "claude-code · READ"],
  ];
  // Traces: [label, start ms, length ms, color, nested]; each draws span by span from its start
  const TRACES = [
    { at: 0.15, until: 1.55, title: "copilot · Read billing/ledger.py", total: 257, spans: [["PreToolUse hook", 0, 22, "#d4d4d8"], ["evaluate policies", 4, 14, "#71717a", true], ["tool Read", 22, 180, "#a1a1aa"], ["PostToolUse verify", 202, 30, "#86efac"], ["event uploaded", 232, 25, "#52525b"]] },
    { at: 1.6, until: 3.1, title: "codex · Bash pytest -q", total: 1840, spans: [["PreToolUse hook", 0, 31, "#d4d4d8"], ["evaluate policies", 6, 20, "#71717a", true], ["tool Bash", 31, 1720, "#a1a1aa"], ["PostToolUse verify", 1751, 52, "#86efac"], ["event uploaded", 1803, 37, "#52525b"]] },
    { at: 3.15, until: Infinity, title: "claude-code · Edit app/payments.py", total: 63, spans: [["PreToolUse hook", 0, 41, "#d4d4d8"], ["load signed state", 2, 9, "#71717a", true], ["context pack", 11, 8, "#71717a", true], ["evaluate policy payments", 19, 17, "#71717a", true], ["DENY · PRESERVED_SELECTION", 36, 5, "#f87171", true], ["event uploaded", 41, 22, "#52525b"]] },
  ];
  const DENIED_AT = 3.1;
  const SEA = {
    steps: [[0, "Metrics and decisions from every teammate's agents, live"], [1.0, "Logs stream in as agents work"], [1.55, "Traces show each tool call, span by span"], [DENIED_AT, "A denied edit is logged, traced, and counted"]],
    html: () => `
  <div class="ly ly-dash" style="--z:0px">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">DataSea</span><small>acme › payments › Overview</small><span class="ml-auto hidden font-mono text-[10px] text-ink-3 sm:inline">1d</span><span class="sh-tag live">Live</span></div>
    <div class="sh-dash">
      <div class="sh-rail"><img src="${LOGO}" alt="">${RAIL.map((path, index) => `<span class="${index ? "" : "on"}">${ICON(path)}</span>`).join("")}</div>
      <div class="sh-main">
        <div class="sh-kpis">
          <div class="sh-kpi"><small>Tool calls</small><b data-kpi="calls"></b></div>
          <div class="sh-kpi bad" data-kpi-denied><small>Denied</small><b data-kpi="denied"></b></div>
          <div class="sh-kpi good"><small>Confirmed bypasses</small><b>0</b></div>
          <div class="sh-kpi"><small>Tokens</small><b data-kpi="tokens"></b></div>
        </div>
        <div class="sh-chartbox">
          <header>Decisions over time<span><i class="bg-white/70"></i>allowed</span><span><i class="bg-red-400"></i>denied</span></header>
          <svg class="sh-chart" viewBox="0 0 400 120" preserveAspectRatio="none">
            <defs><linearGradient id="sh-area" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".22"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient><linearGradient id="sh-area-bad" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#f87171" stop-opacity=".35"/><stop offset="1" stop-color="#f87171" stop-opacity="0"/></linearGradient></defs>
            <g stroke="rgba(255,255,255,.05)"><path d="M0 30H400M0 60H400M0 90H400"/></g>
            <path data-area fill="url(#sh-area)"/><path data-line fill="none" stroke="rgba(255,255,255,.8)" stroke-width="1.6" vector-effect="non-scaling-stroke"/>
            <path data-area-bad fill="url(#sh-area-bad)"/><path data-line-bad fill="none" stroke="#f87171" stroke-width="1.6" vector-effect="non-scaling-stroke"/>
          </svg>
        </div>
      </div>
    </div>
  </div>
  <div class="ly ly-logs" style="--z:85px;--d:.2s">
    <div class="sh-bar"><span class="sh-bar-title">Logs</span><small>every machine · newest first</small><span class="sh-tag live ml-auto">streaming</span></div>
    <div class="sh-logs" data-logs></div>
  </div>
  <div class="ly ly-trace" data-trace style="--z:140px;--d:.35s">
    <div class="sh-bar"><span class="sh-bar-title">Trace</span>${TRACES.map((trace, index) => `<small class="st" data-trace-title="${index}">${trace.title}</small>`).join("")}${TRACES.map((trace, index) => `<span class="sh-tag st ml-auto" data-trace-total="${index}">${trace.total} ms</span>`).join("")}</div>
    ${TRACES.map((_, index) => `<div class="sh-spans st" data-trace-set="${index}"></div>`).join("")}
  </div>
  <div class="ly ly-metric sh-metric" data-metric style="--z:200px;--d:.5s"><span class="sh-label">Hook decision · p95</span><b>38<small>ms</small></b><svg viewBox="0 0 100 30" preserveAspectRatio="none"><polyline data-spark fill="none" stroke="rgba(255,255,255,.7)" stroke-width="1.4" vector-effect="non-scaling-stroke"/></svg><p><span>bypasses</span><span class="text-green-300">0 confirmed</span></p></div>`,
    build(t) {
      // The series move with the time, so the chart slides forward and back with it
      const allowedAt = (i) => 62 + 22 * Math.sin(i / 3.1) + 7 * Math.sin(i * 1.7);
      const deniedAt = (i) => 9 + 3.5 * Math.sin(i * 0.9) + 26 * Math.exp(-((i - 41.5) ** 2) / 0.9);
      const latencyAt = (i) => 36 + 6 * Math.sin(i * 1.3) + 3 * Math.sin(i * 3.1);
      const curve = (value, offset) => Array.from({ length: 30 }, (_, j) => `${j ? "L" : "M"}${((j * 400) / 29).toFixed(1)} ${(118 - value(j + offset)).toFixed(1)}`).join(" ");
      const parts = { line: t.q("[data-line]"), area: t.q("[data-area]"), lineBad: t.q("[data-line-bad]"), areaBad: t.q("[data-area-bad]"), spark: t.q("[data-spark]") };
      const kpis = { calls: t.q('[data-kpi="calls"]'), denied: t.q('[data-kpi="denied"]'), tokens: t.q('[data-kpi="tokens"]') };
      t.each((time) => {
        const offset = time * 4;
        const allowed = curve(allowedAt, offset), denied = curve(deniedAt, offset);
        parts.line.setAttribute("d", allowed);
        parts.area.setAttribute("d", allowed + " L400 120 L0 120 Z");
        parts.lineBad.setAttribute("d", denied);
        parts.areaBad.setAttribute("d", denied + " L400 120 L0 120 Z");
        parts.spark.setAttribute("points", Array.from({ length: 24 }, (_, j) => `${((j * 100) / 23).toFixed(1)},${(30 - (latencyAt(j + time * 3) - 20) * 0.9).toFixed(1)}`).join(" "));
        kpis.calls.textContent = (1284 + Math.floor(time * 6)).toLocaleString("en-US");
        kpis.denied.textContent = String(37 + (time >= DENIED_AT ? 1 : 0));
        kpis.tokens.textContent = 412 + Math.floor(time * 2) + "k";
      });
      t.on(t.q("[data-kpi-denied]"), [DENIED_AT, DENIED_AT + 0.6], "flash");
      t.on(t.q("[data-metric]"), [4.0, 4.5], "flash");
      t.on(t.q("[data-trace]"), [DENIED_AT, 4.2], "alarm");

      // Logs, newest first: the newest eight lines whose time has come
      const list = t.q("[data-logs]");
      const clock = (index) => { const seconds = 14 * 3600 + 2 * 60 + 11 + index * 6; return [seconds / 3600, (seconds / 60) % 60, seconds % 60].map((part) => String(Math.floor(part)).padStart(2, "0")).join(":"); };
      const lines = LOGS.map(([at, flow, event, who, tone], index) => {
        const node = make("div", "sh-log" + (tone ? " " + tone : ""));
        node.append(make("span", "", clock(index)), make("span", "", flow), make("span", "", event), make("span", "", who));
        return { at, node };
      });
      for (const line of lines.slice().reverse()) list.append(line.node);
      let count = -1;
      t.each((time) => {
        const arrived = lines.filter((line) => time >= line.at).length;
        if (arrived === count) return;
        count = arrived;
        lines.forEach((line, index) => line.node.classList.toggle("on", index < arrived && index >= arrived - 8));
      });

      // Traces, span by span
      TRACES.forEach((trace, index) => {
        t.on(t.q(`[data-trace-title="${index}"]`), [index ? trace.at - 0.05 : 0, trace.until]);
        t.on(t.q(`[data-trace-total="${index}"]`), [index ? trace.at - 0.05 : 0, trace.until]);
        t.on(t.q(`[data-trace-set="${index}"]`), [index ? trace.at - 0.05 : 0, trace.until]);
        const set = t.q(`[data-trace-set="${index}"]`);
        trace.spans.forEach(([name, start, length, color, nested], position) => {
          const span = set.appendChild(make("div", "sh-span" + (nested ? " sub" : "")));
          const track = make("s");
          const bar = track.appendChild(make("u"));
          bar.style.left = (start / trace.total) * 100 + "%";
          bar.style.background = color;
          span.append(make("span", "", name), track, make("em", "", length + " ms"));
          const width = Math.max(1.5, (length / trace.total) * 100);
          let last = -1;
          t.each((time) => {
            const grown = clamp((time - trace.at - position * 0.14) / 0.2);
            if (grown !== last) { bar.style.width = grown * width + "%"; span.style.opacity = String(0.25 + grown * 0.75); last = grown; }
          });
        });
      });
    },
  };

  // ---------------------------------------------------------------- the rows and their playback
  const SCENES = { cli: CLI, ext: EXT, sea: SEA };
  const rows = rowsInPage.map((root) => {
    const scene = SCENES[root.dataset.scene];
    const screen = root.querySelector(".show-screen");
    screen.innerHTML = '<div class="obj"><div class="obj-floor"></div><div class="obj-sway"><div class="obj-rig">' + scene.html() + "</div></div></div>";
    const line = timeline(screen);
    // One bar per step in the text column; each fills while its step plays
    const barsNode = root.querySelector(".show-bars");
    barsNode.style.setProperty("--steps", String(scene.steps.length));
    const bars = scene.steps.map(() => barsNode.appendChild(make("i")).appendChild(make("b")));
    const entry = { root, scene, line, bars, after: scene.build(line), rig: screen.querySelector(".obj-rig"), time: -1, hold: 0, visible: false, step: -1, stepNode: root.querySelector(".show-step"), sayNode: root.querySelector(".show-say"), speedNode: root.querySelector(".show-speed") };
    new IntersectionObserver((entries) => {
      entry.visible = entries.some((item) => item.isIntersecting);
      if (entry.visible) root.classList.add("seen");
    }, { threshold: 0.15 }).observe(root);
    // The object leans toward a mouse pointer over its stage
    if (!STILL) {
      const stage = root.querySelector(".show-stage");
      stage.addEventListener("pointermove", (event) => {
        if (event.pointerType !== "mouse") return;
        const box = stage.getBoundingClientRect();
        entry.rig.style.setProperty("--ry", (-15 + (((event.clientX - box.left) / box.width) * 2 - 1) * 7).toFixed(2) + "deg");
        entry.rig.style.setProperty("--rx", (17 - (((event.clientY - box.top) / box.height) * 2 - 1) * 6).toFixed(2) + "deg");
      });
      stage.addEventListener("pointerleave", () => { entry.rig.style.removeProperty("--rx"); entry.rig.style.removeProperty("--ry"); });
    }
    return entry;
  });

  const draw = (entry, time) => {
    entry.time = time;
    entry.line.draw(time);
    if (entry.line.dirty) { if (entry.after) entry.after(); entry.line.dirty = false; }
    const steps = entry.scene.steps;
    let step = 0;
    while (step + 1 < steps.length && time >= steps[step + 1][0]) step += 1;
    if (step !== entry.step) {
      entry.step = step;
      entry.stepNode.textContent = "Step " + (step + 1) + " of " + steps.length;
      entry.sayNode.textContent = steps[step][1];
      entry.sayNode.classList.remove("show-say");
      void entry.sayNode.offsetWidth;
      entry.sayNode.classList.add("show-say");
    }
    entry.bars.forEach((bar, index) => {
      const from = steps[index][0], to = index + 1 < steps.length ? steps[index + 1][0] : DURATION;
      bar.style.width = clamp((time - from) / (to - from)) * 100 + "%";
    });
  };

  if (STILL) {
    for (const entry of rows) { draw(entry, DURATION); entry.speedNode.hidden = true; }
    return;
  }

  // Scrolling down plays faster, scrolling up plays backwards; a still page plays at 1x
  let rate = 1;
  let lastY = window.scrollY;
  let settle = 0;
  window.addEventListener("scroll", () => {
    const y = window.scrollY;
    if (y === lastY) return;
    rate = y > lastY ? 1.15 : -1;
    lastY = y;
    clearTimeout(settle);
    settle = setTimeout(() => { rate = 1; }, 220);
  }, { passive: true });

  let shownRate = null;
  let previous = performance.now();
  for (const entry of rows) draw(entry, 0);
  const frame = (now) => {
    // Real time, so slow or throttled frames do not slow the scene; a long gap (a hidden tab) is cut short
    const elapsed = Math.min(0.25, (now - previous) / 1000);
    previous = now;
    if (!document.hidden) {
      for (const entry of rows) {
        if (!entry.visible) continue;
        let time = entry.time;
        if (rate > 0) {
          if (time >= DURATION) {
            entry.hold += elapsed;
            if (entry.hold >= HOLD) { time = 0; entry.hold = 0; }
          } else {
            time = Math.min(DURATION, time + elapsed * rate);
          }
        } else {
          entry.hold = 0;
          time = Math.max(0, time + elapsed * rate);
        }
        if (time !== entry.time) draw(entry, time);
      }
    }
    if (rate !== shownRate) {
      shownRate = rate;
      for (const entry of rows) {
        entry.speedNode.textContent = rate < 0 ? "◀ reverse" : rate > 1 ? "1.15×" : "1×";
        entry.speedNode.classList.toggle("fast", rate > 1);
        entry.speedNode.classList.toggle("back", rate < 0);
      }
    }
    requestAnimationFrame(frame);
  };
  requestAnimationFrame(frame);
})();
