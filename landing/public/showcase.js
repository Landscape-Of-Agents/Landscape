"use strict";
// The product showcase: Crane (the CLI), Landscape (the editor extension), and DataSea (the
// dashboard), each drawn as a 3D object, a stack of glass layers at different depths that sways
// and leans with the pointer. Each scene is a script of beats with the products' real output and
// layout in the page's grey palette. Scenes advance on their own; they wait while the stage is off
// screen, the tab is hidden, or the viewer paused them, and under reduced motion each one shows its
// finished state without moving.
(() => {
  const stage = document.getElementById("show-stage");
  if (!stage) return;
  const STILL = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const screen = document.getElementById("show-screen");
  const tabs = [...document.querySelectorAll(".show-tab")];
  const stepNode = document.getElementById("show-step");
  const sayNode = document.getElementById("show-say");
  const pauseButton = document.getElementById("show-pause");
  const describe = document.getElementById("show-describe");
  const LOGO = "logo.png?v=3";
  const SVG = "http://www.w3.org/2000/svg";
  const TICK = 40;
  let visible = false;
  let paused = false;
  let run = 0;
  new IntersectionObserver((entries) => { visible = entries.some((entry) => entry.isIntersecting); }, { threshold: 0.2 }).observe(stage);

  class Stopped extends Error {}
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const random = (low, high) => low + Math.random() * (high - low);
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

  // A player for one run of one scene: waits that hold while nobody watches, the progress bar,
  // typing, terminal lines, and the caption. Every call throws Stopped once another run started.
  function player(token, bar, total, steps) {
    let elapsed = 0;
    const live = () => { if (token !== run) throw new Stopped(); };
    const q = (selector) => screen.querySelector(selector);
    const wait = async (ms) => {
      live();
      if (STILL) return;
      let left = ms;
      while (left > 0) {
        const slice = Math.min(TICK, left);
        await sleep(slice);
        live();
        if (!visible || document.hidden || paused) continue;
        left -= slice;
        elapsed += slice;
        bar.style.width = Math.min(97, (elapsed / total) * 100) + "%";
      }
    };
    const type = async (node, text, speed = 34) => {
      if (STILL) { node.textContent += text; return; }
      for (const character of text) {
        node.textContent += character;
        await wait(speed * (0.55 + Math.random() * 0.9));
      }
    };
    const say = (step, text) => {
      live();
      stepNode.textContent = step + "/" + steps;
      sayNode.textContent = text;
      sayNode.classList.remove("show-say");
      void sayNode.offsetWidth;
      sayNode.classList.add("show-say");
    };
    // A terminal line; it scrolls the terminal like a real one
    const line = (term, className, text) => {
      live();
      const node = term.appendChild(make("div", className, text));
      term.scrollTop = term.scrollHeight;
      return node;
    };
    const lines = async (term, items, gap = 150) => {
      for (const item of items) {
        const [className, text] = Array.isArray(item) ? item : ["", item];
        line(term, className, text);
        await wait(gap);
      }
    };
    // A typed command with a caret, then a beat before its output
    const command = async (term, text, speed = 24) => {
      const node = line(term, "cmd", "");
      const typed = node.appendChild(make("span"));
      const caret = node.appendChild(make("span", "sh-caret"));
      await wait(350);
      await type(typed, text, speed);
      term.scrollTop = term.scrollHeight;
      await wait(300);
      caret.remove();
    };
    const flash = (node, className = "flash") => {
      node.classList.remove(className);
      void node.offsetWidth;
      node.classList.add(className);
    };
    return { q, wait, type, say, line, lines, command, flash };
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
    id: "cli",
    steps: 5,
    duration: 36000,
    describe: "Crane as a 3D object: three terminals over one shared payments.py. You run crane protect on the fee lines and a preserve zone appears in the file; a teammate runs crane target on the refund lines and a target zone appears; CI runs crane validate and crane test, which fails while the target is unchanged. An agent's edit of the fee is denied and its refund change lands, and crane test then passes.",
    html: () => `
  <div class="ly ly-file" data-file style="--z:0px">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">app/payments.py</span><small>acme/payments · one codebase</small><span class="sh-tag ml-auto" data-filetag>3 terminals attached</span></div>
    <div class="sh-code" data-code>
      ${row(`${k("from")} decimal ${k("import")} Decimal`)}
      ${row("")}
      ${row(`${k("def")} ${f("fee")}(amount):`, "fee")}
      ${row(`    ${k("return")} amount * Decimal(${s('"0.03"')})<span data-ghost></span>`, "fee")}
      ${row("")}
      ${row(`${k("def")} ${f("refund")}(order):`, "refund")}
      ${row(`    ${k("return")} order.total<span data-refund></span>`, "refund")}
      ${row("")}
      ${row(`${k("def")} ${f("charge")}(card, amount):`)}
      ${row(`    ${k("return")} gateway.capture(card, amount + fee(amount))`)}
    </div>
    <div class="sh-foot" data-foot><b>No selections yet</b></div>
  </div>
  ${terminal(1, "ly-t1", "you", "main", 70, 0.15)}
  ${terminal(2, "ly-t2", "ana", "feature/refunds", 125, 0.3)}
  ${terminal(3, "ly-t3", "ci", "github actions", 180, 0.45)}`,
    async play(p) {
      const code = p.q("[data-code]");
      const term = (n) => p.q(`[data-term="${n}"]`);
      const layers = [1, 2, 3].map((n) => p.q(`[data-ly="${n}"]`));
      const file = p.q("[data-file]");
      const hot = (n) => layers.forEach((node, index) => node.classList.toggle("hot", index + 1 === n));
      const foot = (html) => { p.q("[data-foot]").innerHTML = html; };
      const renumber = () => { let number = 0; for (const node of code.querySelectorAll(".sh-line > i")) node.textContent = String(++number); };
      // Anchor comments around the lines of a selection, and its band
      const seal = (name, kind, id, lens) => {
        const lines = [...code.querySelectorAll(".sh-line." + name)];
        lines[0].insertAdjacentHTML("beforebegin", `<div class="sh-lens ${kind} grow">${lens}</div>` + row(`# @crane:selection:${id}:start`, `anchor ${kind} grow`));
        lines.at(-1).insertAdjacentHTML("afterend", row(`# @crane:selection:${id}:end`, `anchor ${kind} grow`));
        for (const node of code.querySelectorAll(".sh-line." + name)) node.classList.add(kind);
        renumber();
        p.flash(file, "hot");
      };
      renumber();

      p.say(1, "Three terminals share one codebase. You protect the fee");
      hot(1);
      await p.command(term(1), 'crane protect app/payments.py context "Fees are audited; keep the rate." start-line 3 end-line 4');
      await p.lines(term(1), [
        "Protected app/payments.py lines 3-4 as selection K7M2P9RX (policy payments, checkpoint baseline)",
        ["dim", "  added to policy payments: preserve K7M2P9RX;"],
      ]);
      seal("fee", "preserve", "K7M2P9RX", "◆ K7M2P9RX · preserve · from you on main");
      foot('<b><span class="text-green-300">⛨ 1 preserve</span> · K7M2P9RX sealed (sha-512 9f2c…e1a7)</b>');
      await p.wait(1300);

      p.say(2, "A teammate targets the refund the task must change");
      hot(2);
      await p.command(term(2), 'crane target app/payments.py context "Subtract what was refunded." start-line 8 end-line 9 change_type logical_bn');
      await p.lines(term(2), [
        "Targeted app/payments.py lines 8-9 as selection 4QZ8M2LD (policy payments, checkpoint baseline)",
        ["dim", "  added to policy payments: target 4QZ8M2LD change_type logical_bn;"],
      ]);
      seal("refund", "target", "4QZ8M2LD", "◆ 4QZ8M2LD · target · logical_bn · from ana on feature/refunds");
      file.classList.remove("hot");
      foot('<b><span class="text-green-300">⛨ 1 preserve</span> · <span class="text-violet-300">◎ 1 target</span> · policy payments</b>');
      await p.wait(1300);

      p.say(3, "CI validates the rules, and the test waits for the target");
      hot(3);
      await p.command(term(3), "crane validate");
      await p.lines(term(3), ["Crane validate: 1 policies, 2 commands, 2 selections, 1 checkpoints, registry generation 6 (38 files scanned)", ["ok", "Crane validate: PASS"]], 220);
      await p.command(term(3), "crane test .");
      await p.lines(term(3), [
        "  PASS preserve K7M2P9RX: preserved (app/payments.py lines 4-5)",
        ["bad", "  FAIL target 4QZ8M2LD change_type logical_bn: target not changed yet (app/payments.py lines 9-10)"],
        ["bad", "Crane test: FAIL"],
      ], 220);
      await p.wait(1300);

      p.say(4, "An agent edits the codebase: the preserve holds, the target changes");
      hot(0);
      const tag = p.q("[data-filetag]");
      tag.textContent = "claude-code editing";
      tag.classList.add("live");
      file.classList.add("hot");
      await p.wait(700);
      const ghost = p.q("[data-ghost]");
      ghost.className = "sh-ghost";
      ghost.textContent = 'Decimal("0.025")';
      await p.wait(700);
      for (const node of code.querySelectorAll(".sh-line.preserve")) node.classList.add("deny");
      file.classList.replace("hot", "alarm");
      foot('<b class="text-red-300">✕ claude-code Edit denied before it ran · PRESERVED_SELECTION K7M2P9RX</b>');
      await p.wait(2000);
      ghost.className = "";
      ghost.textContent = "";
      for (const node of code.querySelectorAll(".deny")) node.classList.remove("deny");
      file.classList.replace("alarm", "hot");
      await p.wait(500);
      const refund = p.q("[data-refund]");
      refund.className = "sh-typed";
      await p.type(refund, " - order.refunded", 55);
      refund.className = "";
      for (const node of code.querySelectorAll(".sh-line.target")) node.classList.add("done");
      foot('<b class="text-green-300">✓ claude-code changed 4QZ8M2LD · logical_bn</b>');
      tag.textContent = "3 terminals attached";
      tag.classList.remove("live");
      file.classList.remove("hot");
      await p.wait(1200);

      p.say(5, "The same test now passes on every machine");
      hot(3);
      await p.command(term(3), "crane test .");
      await p.lines(term(3), [
        "  PASS preserve K7M2P9RX: preserved (app/payments.py lines 4-5)",
        "  PASS target 4QZ8M2LD change_type logical_bn: target changed as required (app/payments.py lines 9-10)",
        ["ok", "Crane test: PASS"],
      ], 240);
      foot('<b class="text-green-300">✓ policy payments · 2 of 2 rules pass</b>');
      hot(0);
    },
  };

  // ---------------------------------------------------------------- Landscape: the governance graph
  const NODES = {
    billing: { x: 115, y: 112, kind: "zone", label: "billing", sub: "ZONE · CRITICAL" },
    ledger: { x: 320, y: 112, kind: "zone", label: "ledger", sub: "ZONE · HIGH" },
    refunds: { x: 525, y: 112, kind: "zone", label: "refunds", sub: "ZONE · HIGH" },
    K7M2P9RX: { x: 180, y: 252, kind: "preserve", label: "K7M2P9RX", sub: "PRESERVE · FEE RATE", in: "billing" },
    "4QZ8M2LD": { x: 460, y: 252, kind: "target", label: "4QZ8M2LD", sub: "TARGET · LOGICAL_BN", in: "refunds" },
  };
  const EXT = {
    id: "ext",
    steps: 5,
    duration: 30000,
    describe: "Landscape as a 3D object: the governance graph in VS Code. Zones and selections appear; billing, ledger, and refunds are connected in order into the flow checkout; a connection from refunds back to billing would form a cycle, so it is rejected and removed and the zones are deselected; then context is written and signed for the flow, and agents light up the zones they work in.",
    html: () => `
  <div class="ly ly-vs" style="--z:0px">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">Landscape graph</span><small>acme · Visual Studio Code</small><img src="${LOGO}" alt="" class="ml-auto h-4 w-4"></div>
    <div class="sh-toolbar"><span class="sh-tool">◇ Fit</span><span class="sh-tool" data-connect>⇢ Connect zones</span><span class="sh-tool">⛨ Selections</span><span class="ml-auto font-mono text-[10px] text-ink-3" data-hint>Click zones in execution order</span></div>
    <svg class="sh-graph" viewBox="0 0 640 320" preserveAspectRatio="xMidYMid meet" data-graph>
      <defs><marker id="sh-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M0 1 9 5 0 9z" fill="rgba(255,255,255,.75)"/></marker></defs>
      <g data-links></g><g data-edges></g><g data-nodes></g><g data-pulses></g>
    </svg>
  </div>
  <div class="ly ly-gstat sh-gstat" style="--z:95px;--d:.25s"><span class="sh-label">Governance graph</span><b data-gstat>0 zones · 0 flows · 0 selections</b><div><span class="sh-badge ok" data-acyclic>acyclic</span><span class="sh-badge info" data-ctxstat>no context yet</span></div></div>
  <div class="ly ly-ctx later" data-ctx style="--z:40px">
    <div class="sh-bar"><img src="${LOGO}" alt="" class="h-4 w-4"><span class="sh-bar-title">Context · flow checkout</span><span class="sh-tag ml-auto">3 zones</span></div>
    <div class="sh-ctx-body">
      <span class="text-[10.5px] text-ink-2">Given to the agent whenever a tool call touches this flow</span>
      <div class="sh-field" data-text></div>
      <ol class="sh-plan" data-plan><li>Sign the context into governance.yaml</li><li>Attach it to every zone of the flow</li><li>Send it with each tool call in the flow</li></ol>
      <div class="flex items-center gap-2 pt-1"><span class="font-mono text-[10px] text-ink-3">Ctrl ↵</span><span class="sh-btn" data-sign>Sign context</span></div>
    </div>
  </div>`,
    async play(p) {
      const graph = p.q("[data-graph]");
      const nodes = {};
      for (const [id, node] of Object.entries(NODES)) {
        const outer = p.q("[data-nodes]").appendChild(svg("g", { transform: `translate(${node.x} ${node.y})` }));
        const group = outer.appendChild(svg("g", { class: "node " + node.kind }));
        group.innerHTML = `<rect class="ring" x="-72" y="-28" width="144" height="56" rx="15"/><rect class="box" x="-64" y="-21" width="128" height="42" rx="10"/><text class="t" y="-1">${node.label}</text><text class="sub" y="13">${node.sub}</text><g class="agent" transform="translate(44 -27)"><rect x="-26" y="-8" width="52" height="15" rx="7.5"/><text y="2.5">${node.kind === "zone" ? "claude-code" : ""}</text></g>`;
        nodes[id] = group;
      }
      const status = () => {
        const zones = ["billing", "ledger", "refunds"].filter((id) => nodes[id].classList.contains("on")).length;
        const selections = ["K7M2P9RX", "4QZ8M2LD"].filter((id) => nodes[id].classList.contains("on")).length;
        p.q("[data-gstat]").textContent = `${zones} zones · ${edges.length ? 1 : 0} flows · ${selections} selections`;
      };
      const edges = [];
      // A path between two nodes: zone to zone in a row, or an arc back over the top
      const route = (from, to) => {
        const a = NODES[from], b = NODES[to];
        if (b.x > a.x) return `M${a.x + 64} ${a.y} C${a.x + 110} ${a.y} ${b.x - 110} ${b.y} ${b.x - 66} ${b.y}`;
        return `M${a.x} ${a.y - 22} C${a.x} ${a.y - 100} ${b.x} ${b.y - 100} ${b.x} ${b.y - 24}`;
      };
      // Draw a line on as if pen-plotted; undraw it the same way
      const draw = async (path, on = true, ms = 950) => {
        const length = path.getTotalLength();
        path.style.strokeDasharray = String(length);
        path.style.strokeDashoffset = String(on ? length : 0);
        void path.getBoundingClientRect();
        path.style.strokeDashoffset = String(on ? 0 : length);
        await p.wait(ms);
        if (on) { path.style.strokeDasharray = ""; path.style.strokeDashoffset = ""; }
      };
      const connect = async (from, to, label, className = "") => {
        const path = p.q("[data-edges]").appendChild(svg("path", { class: "edge " + className, d: route(from, to), "marker-end": "url(#sh-arrow)", id: "sh-e-" + from + "-" + to }));
        const a = NODES[from], b = NODES[to];
        const text = p.q("[data-edges]").appendChild(svg("text", { class: "elabel " + className, x: (a.x + b.x) / 2, y: b.x > a.x ? a.y - 12 : a.y - 88 }));
        text.textContent = label;
        if (STILL) path.style.opacity = "1";
        await draw(path);
        text.classList.add("on");
        return { path, text };
      };
      const pick = async (id, ms = 550) => { nodes[id].classList.add("pick"); await p.wait(ms); };
      const unpick = () => { for (const node of Object.values(nodes)) node.classList.remove("pick"); };

      p.say(1, "Zones and selections appear as a graph");
      for (const id of ["billing", "ledger", "refunds"]) { nodes[id].classList.add("on"); status(); await p.wait(380); }
      for (const id of ["K7M2P9RX", "4QZ8M2LD"]) {
        nodes[id].classList.add("on");
        const a = NODES[id], b = NODES[NODES[id].in];
        const link = p.q("[data-links]").appendChild(svg("path", { class: "link", d: `M${a.x} ${a.y - 21} C${a.x} ${a.y - 70} ${b.x} ${b.y + 70} ${b.x} ${b.y + 21}` }));
        link.style.opacity = "0";
        status();
        await p.wait(300);
        link.style.transition = "opacity .6s";
        link.style.opacity = "1";
        await p.wait(300);
      }
      await p.wait(700);

      p.say(2, "Connect zones in execution order to create the flow checkout");
      p.q("[data-connect]").classList.add("on");
      await pick("billing");
      await pick("ledger", 200);
      edges.push(await connect("billing", "ledger", "checkout · 1"));
      status();
      await pick("refunds", 200);
      edges.push(await connect("ledger", "refunds", "checkout · 2"));
      status();
      await p.wait(900);

      p.say(3, "A connection back to billing would form a cycle, so it is rejected and removed");
      unpick();
      await p.wait(300);
      await pick("refunds", 350);
      await pick("billing", 250);
      const cycle = await connect("refunds", "billing", "✕ cycle · refunds → billing", "bad");
      const acyclic = p.q("[data-acyclic]");
      acyclic.className = "sh-badge bad";
      acyclic.textContent = "1 cyclic connection rejected";
      p.q("[data-hint]").textContent = "Governance refuses flows that form a cycle";
      await p.wait(1500);
      cycle.text.classList.remove("on");
      await draw(cycle.path, false, 800);
      cycle.path.remove();
      cycle.text.remove();
      unpick();
      p.q("[data-connect]").classList.remove("on");
      acyclic.className = "sh-badge ok";
      acyclic.textContent = "acyclic";
      p.q("[data-hint]").textContent = "Connections deselected";
      await p.wait(1000);

      p.say(4, "Add context to the flow; agents receive it on every tool call there");
      const panel = p.q("[data-ctx]");
      panel.classList.add("on");
      panel.style.setProperty("--z", "150px");
      await p.wait(900);
      const text = p.q("[data-text]");
      text.classList.add("focus");
      await p.type(text, "Charge before posting to the ledger. Never refund an order twice.", 32);
      text.classList.remove("focus");
      await p.wait(300);
      const sign = p.q("[data-sign]");
      sign.classList.add("down", "busy");
      await p.wait(160);
      sign.classList.remove("down");
      for (const step of p.q("[data-plan]").children) { step.classList.add("done"); await p.wait(330); }
      sign.classList.remove("busy");
      sign.textContent = "✓ Signed";
      for (const edge of edges) { edge.path.classList.add("signed"); edge.text.classList.add("signed"); edge.text.textContent = edge.text.textContent.replace("checkout", "checkout ✓"); }
      p.q("[data-ctxstat]").className = "sh-badge ok";
      p.q("[data-ctxstat]").textContent = "context signed";
      await p.wait(1200);

      p.say(5, "Agents light up the flows they work in");
      panel.classList.remove("on");
      panel.style.setProperty("--z", "40px");
      nodes.billing.classList.add("hot");
      p.q("[data-hint]").textContent = "1 agent active on checkout";
      if (!STILL) {
        for (const [index, edge] of edges.entries()) {
          const dot = p.q("[data-pulses]").appendChild(svg("circle", { class: "pulse", r: 3.2 }));
          const motion = dot.appendChild(svg("animateMotion", { dur: "1.8s", begin: index * 0.9 + "s", repeatCount: "indefinite" }));
          motion.appendChild(svg("mpath", { href: "#" + edge.path.id }));
        }
      }
      await p.wait(1600);
      nodes.billing.classList.remove("hot");
      nodes.ledger.classList.add("hot");
      await p.wait(1800);
    },
  };

  // ---------------------------------------------------------------- DataSea: metrics, logs, and traces
  const RAIL = ['<path d="M4 13h6V4H4zM14 20h6v-9h-6zM4 20h6v-3H4zM14 4v3h6V4z"/>', '<circle cx="11" cy="11" r="6"/><path d="m20 20-4.5-4.5"/>', '<path d="M5 6h14M5 10h14M5 14h9M5 18h6"/>', '<path d="M4 6h8M8 12h10M6 18h8"/>', '<circle cx="6" cy="12" r="2"/><circle cx="18" cy="6" r="2"/><circle cx="18" cy="18" r="2"/><path d="M8 12h4l4-5M12 12l4 5"/>', '<path d="M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z"/>'];
  const LOGS = [
    ["F05", "TOOL_CALL_ALLOWED", "codex · BASH"],
    ["F05", "TOOL_CALL_ALLOWED", "copilot · READ"],
    ["F06", "TOOL_CALL_RESULT", "claude-code · EDIT"],
    ["F05", "TOOL_CALL_APPROVAL_REQUIRED", "gemini · WRITE"],
    ["F05", "TOOL_CALL_ALLOWED", "codex · EDIT"],
    ["F02", "COMMAND_COMPLETED", "maya · crane test"],
    ["F05", "TOOL_CALL_ALLOWED", "claude-code · READ"],
  ];
  const TRACES = {
    allowed: { title: "copilot · Read billing/ledger.py", total: 257, spans: [["PreToolUse hook", 0, 22, "#d4d4d8"], ["evaluate policies", 4, 14, "#71717a", true], ["tool Read", 22, 180, "#a1a1aa"], ["PostToolUse verify", 202, 30, "#86efac"], ["event uploaded", 232, 25, "#52525b"]] },
    bash: { title: "codex · Bash pytest -q", total: 1840, spans: [["PreToolUse hook", 0, 31, "#d4d4d8"], ["evaluate policies", 6, 20, "#71717a", true], ["tool Bash", 31, 1720, "#a1a1aa"], ["PostToolUse verify", 1751, 52, "#86efac"], ["event uploaded", 1803, 37, "#52525b"]] },
    denied: { title: "claude-code · Edit app/payments.py", total: 63, spans: [["PreToolUse hook", 0, 41, "#d4d4d8"], ["load signed state", 2, 9, "#71717a", true], ["context pack", 11, 8, "#71717a", true], ["evaluate policy payments", 19, 17, "#71717a", true], ["DENY · PRESERVED_SELECTION", 36, 5, "#f87171", true], ["event uploaded", 41, 22, "#52525b"]] },
  };
  const SEA = {
    id: "sea",
    steps: 4,
    duration: 24000,
    describe: "DataSea as a 3D object: the dashboard's metrics and decisions chart at the back, with a live log, a trace waterfall, and a hook latency tile floating in front. Logs stream in from every machine, a tool call's trace draws span by span, and a denied edit is logged, traced, and counted.",
    html: () => `
  <div class="ly ly-dash" style="--z:0px">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">DataSea</span><small>acme › payments › Overview</small><span class="ml-auto hidden font-mono text-[10px] text-ink-3 sm:inline">1d</span><span class="sh-tag live">Live</span></div>
    <div class="sh-dash">
      <div class="sh-rail"><img src="${LOGO}" alt="">${RAIL.map((path, index) => `<span class="${index ? "" : "on"}">${ICON(path)}</span>`).join("")}</div>
      <div class="sh-main">
        <div class="sh-kpis">
          <div class="sh-kpi"><small>Tool calls</small><b data-kpi="calls">1,284</b></div>
          <div class="sh-kpi bad"><small>Denied</small><b data-kpi="denied">37</b></div>
          <div class="sh-kpi good"><small>Confirmed bypasses</small><b>0</b></div>
          <div class="sh-kpi"><small>Tokens</small><b data-kpi="tokens">412k</b></div>
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
    <div class="sh-bar"><span class="sh-bar-title">Trace</span><small data-trace-title></small><span class="sh-tag ml-auto" data-trace-total></span></div>
    <div class="sh-spans" data-spans></div>
  </div>
  <div class="ly ly-metric sh-metric" data-metric style="--z:200px;--d:.5s"><span class="sh-label">Hook decision · p95</span><b data-p95>38<small>ms</small></b><svg viewBox="0 0 100 30" preserveAspectRatio="none"><polyline data-spark fill="none" stroke="rgba(255,255,255,.7)" stroke-width="1.4" vector-effect="non-scaling-stroke"/></svg><p><span>bypasses</span><span class="text-green-300">0 confirmed</span></p></div>`,
    async play(p) {
      const allowed = Array.from({ length: 30 }, (_, index) => 55 + 25 * Math.sin(index / 3) + random(-6, 6));
      const denied = Array.from({ length: 30 }, () => random(4, 12));
      const latency = Array.from({ length: 24 }, () => random(28, 44));
      const curve = (points) => {
        const step = 400 / (points.length - 1);
        return points.map((value, index) => `${index ? "L" : "M"}${(index * step).toFixed(1)} ${(118 - value).toFixed(1)}`).join(" ");
      };
      const chart = () => {
        p.q("[data-line]").setAttribute("d", curve(allowed));
        p.q("[data-area]").setAttribute("d", curve(allowed) + " L400 120 L0 120 Z");
        p.q("[data-line-bad]").setAttribute("d", curve(denied));
        p.q("[data-area-bad]").setAttribute("d", curve(denied) + " L400 120 L0 120 Z");
        p.q("[data-spark]").setAttribute("points", latency.map((value, index) => `${(index * 100) / (latency.length - 1)},${30 - (value - 20) * 0.9}`).join(" "));
      };
      const totals = { calls: 1284, denied: 37, tokens: 412 };
      const kpis = () => {
        p.q('[data-kpi="calls"]').textContent = totals.calls.toLocaleString("en-US");
        p.q('[data-kpi="denied"]').textContent = String(totals.denied);
        p.q('[data-kpi="tokens"]').textContent = totals.tokens + "k";
      };
      let clock = 14 * 3600 + 2 * 60 + 11;
      const time = () => { clock += Math.round(random(2, 8)); return [clock / 3600, (clock / 60) % 60, clock % 60].map((part) => String(Math.floor(part)).padStart(2, "0")).join(":"); };
      const log = ([flow, event, who], className = "", fresh = true) => {
        const node = make("div", "sh-log " + className + (fresh ? " new" : ""));
        node.append(make("span", "", time()), make("span", "", flow), make("span", "", event), make("span", "", who));
        const list = p.q("[data-logs]");
        list.prepend(node);
        while (list.children.length > 9) list.lastChild.remove();
      };
      // One more decision: the chart moves on, the totals count it, the latency tile samples it
      const tick = (bad) => {
        allowed.shift(); allowed.push(55 + 25 * Math.sin(Date.now() / 900) + random(-6, 6));
        denied.shift(); denied.push(bad ? random(26, 34) : random(4, 12));
        latency.shift(); latency.push(random(28, 44));
        totals.calls += Math.round(random(1, 4));
        totals.tokens += Math.round(random(1, 3));
        if (bad) totals.denied += 1;
        chart(); kpis();
      };
      const trace = async (name) => {
        const { title, total, spans } = TRACES[name];
        p.q("[data-trace-title]").textContent = title;
        p.q("[data-trace-total]").textContent = total + " ms";
        const list = p.q("[data-spans]");
        list.replaceChildren();
        for (const [label, start, length, color, sub] of spans) {
          const span = list.appendChild(make("div", "sh-span" + (sub ? " sub" : "")));
          const track = make("s");
          const bar = track.appendChild(make("u"));
          bar.style.left = (start / total) * 100 + "%";
          bar.style.background = color;
          span.append(make("span", "", label), track, make("em", "", length + " ms"));
          void bar.offsetWidth;
          bar.style.width = Math.max(1.5, (length / total) * 100) + "%";
          await p.wait(320);
        }
      };
      for (const entry of LOGS.slice(0, 5)) log(entry, "", false);
      chart(); kpis();

      p.say(1, "Metrics and decisions from every teammate's agents, live");
      await trace("allowed");
      for (let index = 0; index < 3; index += 1) { await p.wait(700); tick(false); }

      p.say(2, "Logs stream in as agents work");
      for (const entry of LOGS.slice(5)) { await p.wait(650); log(entry); tick(false); }
      await p.wait(500);

      p.say(3, "Traces show each tool call, span by span");
      await trace("bash");
      await p.wait(1500);

      p.say(4, "A denied edit is logged, traced, and counted");
      log(["F05", "TOOL_CALL_DENIED_RULE", "claude-code · EDIT"], "bad");
      tick(true);
      p.flash(p.q('[data-kpi="denied"]').parentElement);
      p.q("[data-trace]").classList.add("alarm");
      await trace("denied");
      p.flash(p.q("[data-metric]"));
      await p.wait(1800);
      p.q("[data-trace]").classList.remove("alarm");
      for (const entry of LOGS.slice(0, 3)) { await p.wait(800); log(entry); tick(false); }
      await p.wait(800);
    },
  };

  // ---------------------------------------------------------------- the stage
  const SCENES = [CLI, EXT, SEA];
  let rig = null;
  async function show(index) {
    const token = ++run;
    const scene = SCENES[index];
    tabs.forEach((tab, position) => {
      tab.setAttribute("aria-selected", String(position === index));
      tab.tabIndex = position === index ? 0 : -1;
      tab.querySelector(".show-progress b").style.width = "0%";
    });
    stage.setAttribute("aria-labelledby", tabs[index].id);
    describe.textContent = scene.describe;
    screen.innerHTML = '<div class="obj"><div class="obj-floor"></div><div class="obj-sway"><div class="obj-rig">' + scene.html() + "</div></div></div>";
    rig = screen.querySelector(".obj-rig");
    const bar = tabs[index].querySelector(".show-progress b");
    const play = player(token, bar, scene.duration, scene.steps);
    try {
      await play.wait(900);
      await scene.play(play);
      await play.wait(2800);
    } catch (error) {
      if (error instanceof Stopped) return;
      throw error;
    }
    bar.style.width = "100%";
    if (!STILL) show((index + 1) % SCENES.length);
  }

  // The object leans toward a mouse pointer over the stage
  if (!STILL) {
    stage.addEventListener("pointermove", (event) => {
      if (!rig || event.pointerType !== "mouse") return;
      const box = stage.getBoundingClientRect();
      const x = ((event.clientX - box.left) / box.width) * 2 - 1;
      const y = ((event.clientY - box.top) / box.height) * 2 - 1;
      rig.style.setProperty("--ry", (-15 + x * 7).toFixed(2) + "deg");
      rig.style.setProperty("--rx", (17 - y * 6).toFixed(2) + "deg");
    });
    stage.addEventListener("pointerleave", () => { if (rig) { rig.style.removeProperty("--rx"); rig.style.removeProperty("--ry"); } });
  }

  const setPaused = (value) => {
    paused = value;
    pauseButton.setAttribute("aria-pressed", String(paused));
    pauseButton.setAttribute("aria-label", paused ? "Play the showcase" : "Pause the showcase");
    pauseButton.querySelector("path").setAttribute("d", paused ? "M8 5v14l11-7z" : "M7 5h3.5v14H7zM13.5 5H17v14h-3.5z");
  };
  if (STILL) pauseButton.hidden = true;
  pauseButton.addEventListener("click", () => setPaused(!paused));
  tabs.forEach((tab, index) => {
    tab.addEventListener("click", () => { setPaused(false); show(index); });
    tab.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      if (!step) return;
      const next = (index + step + tabs.length) % tabs.length;
      tabs[next].focus();
      setPaused(false);
      show(next);
    });
  });
  // Links to #cli, #extension, or #dashboard open that scene
  const fromHash = () => tabs.findIndex((tab) => "#" + tab.id === location.hash);
  window.addEventListener("hashchange", () => { const index = fromHash(); if (index >= 0) show(index); });
  show(Math.max(0, fromHash()));
})();
