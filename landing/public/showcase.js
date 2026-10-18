"use strict";
// The product showcase: one rule followed through Crane (the CLI), Landscape (the editor extension),
// and DataSea (the dashboard). Each scene is a script of beats played by a virtual pointer and
// keyboard, with the products' real output and layout in the page's grey palette. A camera zooms in
// on the action and back out, and callouts name what just happened. Scenes advance on their own;
// they wait while the stage is off screen, the tab is hidden, or the viewer paused them, and under
// reduced motion each one shows its finished state without moving.
(() => {
  const stage = document.getElementById("show-stage");
  if (!stage) return;
  const STILL = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const screen = document.getElementById("show-screen");
  const pointer = document.getElementById("show-pointer");
  const tabs = [...document.querySelectorAll(".show-tab")];
  const stepNode = document.getElementById("show-step");
  const sayNode = document.getElementById("show-say");
  const pauseButton = document.getElementById("show-pause");
  const describe = document.getElementById("show-describe");
  const LOGO = "logo.png?v=3";
  const CONTEXT = "Fees are audited; keep the rate.";
  const TICK = 40;
  let visible = false;
  let paused = false;
  let run = 0;
  new IntersectionObserver((entries) => { visible = entries.some((entry) => entry.isIntersecting); }, { threshold: 0.2 }).observe(stage);

  class Stopped extends Error {}
  const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const clamp = (value, low, high) => Math.min(high, Math.max(low, value));
  const make = (tag, className, text) => {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  };

  // A player for one run of one scene: waits that hold while nobody watches, the progress bar, the
  // camera, the pointer, callouts, typing, and the caption. Every call throws Stopped once another
  // run has started.
  function player(token, camera, bar, total, steps) {
    let elapsed = 0;
    const live = () => { if (token !== run) throw new Stopped(); };
    const q = (selector) => camera.querySelector(selector);
    const resolve = (target) => (typeof target === "string" ? q(target) : target);
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
    // A point of a node in the camera's own (unzoomed) coordinates, read even mid-zoom
    const local = (node, fx, fy) => {
      const matrix = getComputedStyle(camera).transform;
      const zoom = matrix && matrix !== "none" ? new DOMMatrix(matrix).a : 1;
      const frame = camera.getBoundingClientRect();
      const box = node.getBoundingClientRect();
      return { x: (box.left - frame.left + box.width * fx) / zoom, y: (box.top - frame.top + box.height * fy) / zoom, shown: box.width > 0 };
    };
    // Zoom the camera on a node (narrow stages zoom half as much); wide() pulls back out
    const focus = async (target, zoom = 1.25, ms = 1100) => {
      live();
      if (STILL) return;
      const node = resolve(target);
      const at = node && local(node, 0.5, 0.5);
      if (!at || !at.shown) return;
      const width = camera.offsetWidth, height = camera.offsetHeight;
      const scale = width < 640 ? 1 + (zoom - 1) / 2 : zoom;
      const x = clamp(width / 2 - at.x * scale, width - width * scale, 0);
      const y = clamp(height / 2 - at.y * scale, height - height * scale, 0);
      camera.style.transform = "translate(" + x + "px," + y + "px) scale(" + scale + ")";
      await wait(ms);
    };
    const wide = async (ms = 1000) => {
      live();
      if (STILL) return;
      camera.style.transform = "";
      await wait(ms);
    };
    // Move the pointer to a point of a node (fractions of its box); hidden nodes hide the pointer
    const point = async (target, fx = 0.5, fy = 0.5, ms = 850) => {
      live();
      if (STILL) return;
      const node = resolve(target);
      const at = node && local(node, fx, fy);
      if (!at || !at.shown) { pointer.classList.remove("on"); return; }
      pointer.style.transform = "translate(" + at.x + "px," + at.y + "px)";
      pointer.classList.add("on");
      await wait(ms);
    };
    const click = async () => {
      live();
      if (STILL) return;
      pointer.classList.remove("tap");
      void pointer.offsetWidth;
      pointer.classList.add("tap", "down");
      await wait(160);
      pointer.classList.remove("down");
      await wait(120);
    };
    const hide = () => pointer.classList.remove("on");
    // A callout beside a node: a glass label naming what just happened
    const note = (target, text, side = "right", tone = "") => {
      live();
      const node = resolve(target);
      const at = node && local(node, { right: 1, left: 0, above: 0.5, below: 0.5 }[side], { right: 0.5, left: 0.5, above: 0, below: 1 }[side]);
      if (!at || !at.shown) return null;
      const label = camera.appendChild(make("div", "sh-note " + tone, text));
      const width = label.offsetWidth, height = label.offsetHeight;
      const gap = 12;
      let x = side === "right" ? at.x + gap : side === "left" ? at.x - width - gap : at.x - width / 2;
      let y = side === "above" ? at.y - height - gap : side === "below" ? at.y + gap : at.y - height / 2;
      x = clamp(x, 8, camera.offsetWidth - width - 8);
      y = clamp(y, 8, camera.offsetHeight - height - 8);
      label.style.left = x + "px";
      label.style.top = y + "px";
      return label;
    };
    const unnote = (label) => { if (label) { label.classList.add("out"); setTimeout(() => label.remove(), 400); } };
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
    const lines = async (term, items, gap = 120) => {
      for (const item of items) {
        const [className, text] = Array.isArray(item) ? item : ["", item];
        line(term, className, text);
        await wait(gap);
      }
    };
    // A typed command with a caret, then a beat before its output
    const command = async (term, text, speed = 26) => {
      const node = line(term, "cmd", "");
      const typed = node.appendChild(make("span"));
      const caret = node.appendChild(make("span", "sh-caret"));
      await wait(380);
      await type(typed, text, speed);
      await wait(320);
      caret.remove();
      return node;
    };
    const flash = (node, className = "flash") => {
      node.classList.remove(className);
      void node.offsetWidth;
      node.classList.add(className);
    };
    return { q, wait, focus, wide, point, click, hide, note, unnote, type, say, line, lines, command, flash };
  }

  // ---------------------------------------------------------------- Crane: the CLI
  const CLI = {
    id: "cli",
    steps: 5,
    duration: 40000,
    describe: "Crane in a terminal: a human protects the fee lines of app/payments.py with crane protect, crane validate passes, and crane agent install puts Crane's hooks into Claude Code. When the agent tries to change the fee, the hook denies the edit with the rule's context; the agent changes the targeted refund instead, and crane test passes.",
    html: () => `
<div class="sh-scene sh-cli">
  <div class="sh-win sh-you front" data-win="you">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="sh-bar-title">~/acme/payments · zsh</span><span class="sh-tag ml-auto">you</span></div>
    <div class="sh-term" data-term="you"></div>
  </div>
  <div class="sh-receipts" data-receipts></div>
  <div class="sh-win sh-agent back" data-win="agent">
    <div class="sh-bar"><span class="dots"><i></i><i></i><i></i></span><span class="font-mono text-[12px] text-ink">✻</span><span class="sh-bar-title">Claude Code · ~/acme/payments</span><span class="sh-tag ml-auto" data-agent>idle</span></div>
    <div class="sh-term" data-term="agent"></div>
  </div>
</div>`,
    async play(p) {
      const you = p.q('[data-term="you"]');
      const agent = p.q('[data-term="agent"]');
      const front = (name) => {
        for (const node of [p.q('[data-win="you"]'), p.q('[data-win="agent"]')]) {
          const on = node.dataset.win === name;
          node.classList.toggle("front", on);
          node.classList.toggle("back", !on);
        }
      };
      // What Crane now holds, stacked beside the terminals
      const receipt = (glyph, title, detail, tone = "") => {
        const node = make("div", "sh-receipt " + tone);
        node.append(make("i", "", glyph), make("b", "", title), make("span", "", detail));
        p.q("[data-receipts]").append(node);
        return node;
      };
      const status = (text, live) => { const tag = p.q("[data-agent]"); tag.textContent = text; tag.classList.toggle("live", Boolean(live)); };

      p.say(1, "A human protects the fee rule from the terminal");
      await p.focus('[data-win="you"]', 1.18, 900);
      await p.command(you, `crane protect app/payments.py context "${CONTEXT}" start-line 3 end-line 4 policy payments`, 22);
      await p.lines(you, [
        ["text-ink", "Protected app/payments.py lines 3-4 as selection K7M2P9RX (policy payments, checkpoint baseline)"],
        "  marker:         @crane:selection:K7M2P9RX:start / :end (in python comment syntax)",
        "  origin digest:  9f2c41d7…e1a7 (SHA-512)",
        "  added to policy payments: preserve K7M2P9RX;",
      ], 160);
      await p.wide(700);
      receipt("⛨", "Selection sealed", "K7M2P9RX · sha-512 9f2c…e1a7");
      await p.wait(900);

      p.say(2, "Crane verifies integrity, signatures, and markers");
      await p.command(you, "crane validate");
      await p.lines(you, [
        "Crane validate: 1 policies, 2 commands, 2 selections, 1 checkpoints, registry generation 5 (38 files scanned)",
        ["text-green-300", "Crane validate: PASS"],
      ], 240);
      receipt("§", "Policy signed", "payments · ed25519 · generation 5");
      await p.wait(900);

      p.say(3, "Crane's hooks go into the agent");
      await p.command(you, "crane agent install --profile claude");
      await p.lines(you, [
        "Installed Crane Claude Code hooks for PreToolUse, PostToolUse, Stop",
        "Every pre- and post-tool hook now passes through Crane; check them with 'crane agent hooks --profile claude'.",
      ], 220);
      receipt("⚓", "Hooks installed", "Claude Code · every tool call checked");
      await p.wait(1000);

      p.say(4, "The agent goes for the fee, and the hook denies it with the rule's context");
      front("agent");
      status("working", true);
      await p.focus('[data-win="agent"]', 1.22, 1000);
      const ask = p.line(agent, "ask", "");
      await p.type(ask, "make the checkout fee 2.5%", 40);
      await p.wait(700);
      const update = p.line(agent, "act", "Update(app/payments.py)");
      await p.wait(1000);
      update.classList.add("bad");
      p.line(agent, "out err", "PreToolUse:Edit hook error: Crane denied this tool call: the edit would change preserved selection K7M2P9RX (app/payments.py lines 4-5, policy payments); preserved code must stay as it was at checkpoint baseline");
      p.flash(p.q('[data-win="agent"]'), "deny");
      await p.wait(2200);
      p.line(agent, "act", `The fee rate is preserved: "${CONTEXT}" I'll leave fee() as it is. The refund fix is this task's target, so I'll make that change instead.`);
      await p.wait(1600);
      p.line(agent, "act ok", "Update(app/payments.py)");
      await p.wait(500);
      p.line(agent, "out", "Updated app/payments.py with 1 addition and 1 removal");
      status("done");
      await p.wide(800);
      receipt("✕", "Edit denied before it ran", "PRESERVED_SELECTION · context sent", "bad");
      await p.wait(1000);

      p.say(5, "crane test agrees: the preserve held and the target changed");
      front("you");
      await p.focus('[data-win="you"]', 1.12, 900);
      await p.command(you, "crane test .");
      await p.lines(you, [
        "PASS policy payments (.crane/policies/payments.crane, 2 commands)",
        "  PASS preserve K7M2P9RX: preserved (app/payments.py lines 4-5)",
        "  PASS target 4QZ8M2LD change_type logical_bn: target changed as required (app/payments.py lines 9-10)",
        ["text-green-300", "Crane test: PASS"],
      ], 260);
      await p.wide(800);
      receipt("✓", "crane test passed", "payments · 2 of 2 rules", "ok");
    },
  };

  // ---------------------------------------------------------------- Landscape: the extension
  const k = (text) => `<span class="sh-k">${text}</span>`;
  const f = (text) => `<span class="sh-f">${text}</span>`;
  const s = (text) => `<span class="sh-s">${text}</span>`;
  const row = (html, className = "") => `<div class="sh-line ${className}"><i></i><span>${html}</span></div>`;
  const ICON = (path) => `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round">${path}</svg>`;
  const EXT = {
    id: "ext",
    steps: 5,
    duration: 30000,
    describe: "Landscape in VS Code: the fee lines are selected and Ctrl Alt F opens the Landscape panel beside the code. Preserve is chosen, a name and context are typed, and Landscape has Crane insert anchor comments and seal the selection; the sidebar counts it. When an agent then tries to edit the fee, the lines turn red and Crane's denial appears with the context.",
    html: () => `
<div class="sh-scene">
  <div class="sh-vs">
    <div class="sh-vs-title"><span class="dots"><i></i><i></i><i></i></span>payments.py · acme · Visual Studio Code</div>
    <div class="sh-vs-body">
      <div class="sh-activity">
        <span>${ICON('<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>')}</span>
        <span>${ICON('<circle cx="11" cy="11" r="6"/><path d="m20 20-4.5-4.5"/>')}</span>
        <span>${ICON('<circle cx="6" cy="6" r="2"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="8" r="2"/><path d="M6 8v8M18 10c0 4-6 3-11 6"/>')}</span>
        <span class="on"><img src="${LOGO}" alt=""></span>
        <span>${ICON('<rect x="4" y="4" width="7" height="7" rx="1"/><rect x="13" y="13" width="7" height="7" rx="1"/><rect x="4" y="13" width="7" height="7" rx="1"/><path d="M16.5 4v6M13.5 7h6"/>')}</span>
      </div>
      <div class="sh-side" data-side>
        <div class="sh-side-head">Landscape: Overview</div>
        <div class="sh-side-body">
          <div class="sh-live" data-live><div class="flex items-center gap-1.5"><span class="text-ink">✦</span><span class="truncate"><b class="font-medium text-ink">claude-code</b> is working · Edit</span><span class="ml-auto font-mono text-[9px] text-ink-3">now</span></div><mark>preserve fee rate<br>${CONTEXT}</mark></div>
          <div class="sh-stats">
            <div><b class="ok" data-stat="pass">1</b><small>Passing</small></div><div><b>0</b><small>Failing</small></div><div><b>0</b><small>Unresolved</small></div>
            <div><b>1</b><small>Policies</small></div><div><b>2</b><small>Zones</small></div><div><b>1</b><small>Flows</small></div>
          </div>
          <h5>Selections <span data-count>1 of 1</span></h5>
          <div class="grid gap-0.5" data-selections>
            <div class="sh-item"><i>◎</i><span><b>refund fix</b><small>app/payments.py:7-8 · payments</small></span><span class="sh-badge ok">PASS</span></div>
          </div>
          <h5>Zones and flows <span>2 of 2</span></h5>
          <div class="grid gap-0.5">
            <div class="sh-item"><i>▦</i><span><b>billing</b><small>Card charging and refunds</small></span><span class="sh-badge info">active</span></div>
            <div class="sh-item"><i>⇢</i><span><b>checkout</b><small>src/app.py:checkout and its calls</small></span><span class="sh-badge info">active</span></div>
          </div>
          <h5>Activity</h5>
          <ul class="sh-log" data-log><li><span>09:41</span><span class="text-green-300">●</span><span>Validate · PASS</span></li></ul>
        </div>
      </div>
      <div class="sh-editor">
        <div class="sh-tabs"><span class="on">payments.py</span><span>governance.yaml</span><span class="hidden sm:flex">payments.crane</span></div>
        <div class="sh-crumbs">app <span>›</span> payments.py <span>›</span> ƒ fee</div>
        <div class="sh-code" data-code>
          ${row(`${k("from")} decimal ${k("import")} Decimal`)}
          ${row("")}
          ${row(`${k("def")} ${f("fee")}(amount):`)}
          ${row(`    ${k("return")} amount * Decimal(${s('"0.03"')})<span data-ghost></span>`)}
          ${row("")}
          <div class="sh-lens target">◆ 4QZ8M2LD · target · logical_bn · refund fix</div>
          ${row("# @crane:selection:4QZ8M2LD:start", "anchor target")}
          ${row(`${k("def")} ${f("refund")}(order):`, "target")}
          ${row(`    ${k("return")} order.total`, "target")}
          ${row("# @crane:selection:4QZ8M2LD:end", "anchor target")}
          ${row("")}
          ${row(`${k("def")} ${f("charge")}(card, amount):`)}
          ${row(`    ${k("return")} gateway.capture(card, amount + fee(amount))`)}
        </div>
        <div class="sh-keys" data-keys><kbd>Ctrl</kbd><kbd>Alt</kbd><kbd>F</kbd></div>
        <div class="sh-notify" data-notify><b>✕ Crane denied claude-code's edit</b> · preserve K7M2P9RX<span class="mt-1 block text-ink-3">Context: ${CONTEXT}</span></div>
        <div class="sh-panel" data-panel>
          <div class="sh-panel-head"><img src="${LOGO}" alt=""><span><b>Landscape</b><small>1 policy · 1 selection · 3 zones and flows</small></span><span class="sh-badge ok ml-auto">Crane 0.4</span></div>
          <div class="sh-panel-body">
            <div class="sh-ctx"><span class="sh-label">Context</span><div class="mt-1 flex flex-wrap items-center gap-1"><code>app/payments.py:3-4</code><span class="sh-badge info">ƒ fee</span><span class="sh-badge info">zone billing</span></div></div>
            <div class="sh-chips"><span data-chip>Preserve</span><span>Target</span><span>Zone</span><span>Flow</span><span>Context</span><span>Checkpoint</span></div>
            <div class="sh-form" data-form>
              <h6>Rule</h6>
              <div class="grid grid-cols-[minmax(0,1fr)_minmax(0,1.3fr)] gap-1.5"><span class="grid gap-1"><span class="sh-field-label">Policy</span><span class="sh-field flex justify-between"><span>payments</span><span class="text-ink-3">▾</span></span></span><span class="grid gap-1"><span class="sh-field-label">Name</span><span class="sh-field" data-name></span></span></div>
              <h6 class="mt-1">Agent guidance</h6>
              <span class="sh-field-label">Context (required)</span><div class="sh-field area" data-context></div>
              <h6 class="mt-1">Landscape will</h6>
              <ol class="sh-plan" data-plan><li>Save the file if it has unsaved changes</li><li>Protect app/payments.py:3-4 in policy payments</li><li>Crane inserts anchor comments around the lines</li><li>Agents are then denied any edit that changes them</li></ol>
            </div>
          </div>
          <div class="sh-panel-foot"><span class="sh-btn">← Back</span><kbd>Ctrl ↵</kbd><span class="sh-btn primary" data-go>Preserve</span></div>
        </div>
      </div>
    </div>
  </div>
</div>`,
    async play(p) {
      const code = p.q("[data-code]");
      const renumber = () => { let number = 0; for (const node of code.querySelectorAll(".sh-line > i")) node.textContent = String(++number); };
      renumber();
      const rows = () => [...code.querySelectorAll(".sh-line")];
      const [, , feeHead, feeBody] = rows();
      const field = async (selector, text) => {
        const node = p.q(selector);
        await p.point(node, 0.15, 0.5, 700);
        await p.click();
        node.classList.add("focus");
        await p.type(node, text, 42);
        await p.wait(250);
        node.classList.remove("focus");
      };
      const log = (time, tone, text) => {
        const item = make("li", "new");
        item.append(make("span", "", time), make("span", tone, "●"), make("span", "", text));
        p.q("[data-log]").prepend(item);
      };

      p.say(1, "Select the fee lines in the editor");
      await p.focus(feeBody, 1.3, 1000);
      await p.point(feeHead, 0.06, 0.5, 800);
      await p.click();
      feeHead.classList.add("sel");
      await p.point(feeBody, 0.62, 0.5, 600);
      feeBody.classList.add("sel");
      await p.wait(500);

      p.say(2, "Press Ctrl Alt F: Landscape opens beside the code");
      p.hide();
      p.q("[data-keys]").classList.add("on");
      await p.wait(1100);
      p.q("[data-keys]").classList.remove("on");
      p.q("[data-panel]").classList.add("on");
      await p.focus("[data-panel]", 1.22, 1000);

      p.say(3, "Choose Preserve, then tell agents why these lines matter");
      await p.point("[data-chip]", 0.5, 0.55, 800);
      await p.click();
      p.q("[data-chip]").classList.add("on");
      p.q("[data-form]").classList.add("on");
      await p.wait(400);
      await field("[data-name]", "fee rate");
      await field("[data-context]", CONTEXT);
      await p.wait(400);

      p.say(4, "Landscape has Crane insert the anchors and seal the selection");
      const go = p.q("[data-go]");
      await p.point(go, 0.5, 0.55, 800);
      await p.click();
      go.classList.add("busy");
      for (const step of p.q("[data-plan]").children) { step.classList.add("done"); await p.wait(380); }
      await p.wait(300);
      p.q("[data-panel]").classList.remove("on");
      p.hide();
      feeHead.classList.remove("sel");
      feeBody.classList.remove("sel");
      await p.wide(800);
      feeHead.insertAdjacentHTML("beforebegin", '<div class="sh-lens preserve grow" data-lens>◆ K7M2P9RX · preserved · fee rate · signed by owner</div>' + row("# @crane:selection:K7M2P9RX:start", "anchor preserve grow"));
      feeBody.insertAdjacentHTML("afterend", row("# @crane:selection:K7M2P9RX:end", "anchor preserve grow"));
      feeHead.classList.add("preserve");
      feeBody.classList.add("preserve");
      renumber();
      p.q("[data-selections]").insertAdjacentHTML("afterbegin", '<div class="sh-item new" data-new><i>⛨</i><span><b>fee rate</b><small>app/payments.py:4-5 · payments</small></span><span class="sh-badge ok">PASS</span></div>');
      p.q("[data-count]").textContent = "2 of 2";
      p.q('[data-stat="pass"]').textContent = "2";
      log("09:42", "text-green-300", "Preserve · selection K7M2P9RX in payments");
      await p.wait(500);
      const anchors = p.note(feeBody, "Anchor comments move with the code", "right");
      const counted = p.note("[data-new]", "Sealed and counted", "right", "ok");
      await p.wait(2200);
      p.unnote(anchors);
      p.unnote(counted);

      p.say(5, "An agent edits the fee: Crane denies it, and the agent sees why");
      p.q("[data-live]").classList.add("on");
      await p.focus(feeBody, 1.22, 1000);
      const ghost = p.q("[data-ghost]");
      ghost.className = "sh-ghost";
      ghost.textContent = 'Decimal("0.025")';
      await p.wait(900);
      for (const node of code.querySelectorAll(".preserve.sh-line")) node.classList.add("deny");
      const denied = p.note(feeBody, "Denied before it ran", "below", "bad");
      log("09:43", "text-red-300", "Denied · claude-code Edit app/payments.py:5");
      await p.wait(1800);
      p.unnote(denied);
      await p.wide(700);
      p.q("[data-notify]").classList.add("on");
      await p.wait(1600);
      ghost.className = "";
      ghost.textContent = "";
      for (const node of code.querySelectorAll(".deny")) node.classList.remove("deny");
    },
  };

  // ---------------------------------------------------------------- DataSea: the dashboard
  const NAV = [
    ["Monitor", [["Overview", '<path d="M4 13h6V4H4zM14 20h6v-9h-6zM4 20h6v-3H4zM14 4v3h6V4z"/>', "on"], ["Event explorer", '<circle cx="11" cy="11" r="6"/><path d="m20 20-4.5-4.5"/>'], ["Timeline", '<path d="M4 6h10M4 12h16M4 18h7"/>']]],
    ["AI agents", [["Agents", '<circle cx="12" cy="8" r="3.5"/><path d="M5 20c1-4 4-6 7-6s6 2 7 6"/>'], ["Tool calls", '<path d="M14 6l4 4-8 8H6v-4z"/>'], ["Sessions", '<rect x="4" y="5" width="16" height="14" rx="2"/><path d="M4 9h16"/>']]],
    ["Investigate", [["Logs", '<path d="M5 6h14M5 10h14M5 14h9M5 18h6"/>'], ["Traces", '<path d="M4 6h8M8 12h10M6 18h8"/>'], ["Topology", '<circle cx="6" cy="12" r="2"/><circle cx="18" cy="6" r="2"/><circle cx="18" cy="18" r="2"/><path d="M8 12h4l4-5M12 12l4 5"/>']]],
    ["Operate", [["Alerting", '<path d="M6 16V11a6 6 0 1 1 12 0v5l2 2H4zM10 20h4"/>', "", '<em class="hot" data-alerts>1</em>']]],
    ["Govern", [["Audit trail", '<path d="M12 3l8 4v5c0 5-3.5 8-8 9-4.5-1-8-4-8-9V7z"/>'], ["Governance", '<path d="M4 7h16M4 12h16M4 17h10"/>']]],
  ];
  const AGENTS = [["claude-code", "#e4e4e7"], ["codex", "#a1a1aa"], ["copilot", "#71717a"], ["gemini", "#52525b"]];
  const DECISION_TONE = { ALLOW: "ok", DENY: "bad", REQUIRE_APPROVAL: "ask", NOT_REQUIRED: "info" };
  const STREAM = [
    ["codex", "BASH", "TOOL_CALL_ALLOWED", "ALLOW"],
    ["copilot", "READ", "TOOL_CALL_ALLOWED", "ALLOW"],
    ["claude-code", "EDIT", "TOOL_CALL_RESULT", "NOT_REQUIRED"],
    ["gemini", "WRITE", "TOOL_CALL_APPROVAL_REQUIRED", "REQUIRE_APPROVAL"],
    ["codex", "EDIT", "TOOL_CALL_ALLOWED", "ALLOW"],
    ["claude-code", "READ", "TOOL_CALL_ALLOWED", "ALLOW"],
  ];
  const DENIED = ["claude-code", "EDIT", "TOOL_CALL_DENIED_RULE", "DENY"];
  const SEA = {
    id: "sea",
    steps: 4,
    duration: 24000,
    describe: "DataSea, the dashboard: the overview of acme/payments shows tool calls, denials, bypasses, and tokens, decisions per hour, and each agent's share, while an event stream fills from every machine. A denied edit by claude-code arrives; opening it shows its attribute groups, the rule PRESERVED_SELECTION, and its place in the verified hash chain.",
    html: () => `
<div class="sh-scene">
  <div class="sh-sea">
    <nav class="sh-rail">
      <div class="sh-brand"><img src="${LOGO}" alt=""><span><b>DataSea</b><small>AI agent observability · Crane</small></span></div>
      <div class="sh-rail-search">⌕ Find a page or zone</div>
      ${NAV.map(([group, items]) => `<h6>${group}</h6>` + items.map(([label, path, on, extra]) => `<a class="${on || ""}">${ICON(path)}<span>${label}</span>${extra || ""}</a>`).join("")).join("")}
      <div class="sh-rail-foot"><span class="sh-avatar">M</span><span class="truncate">maya · acme</span></div>
    </nav>
    <div class="sh-main">
      <div class="sh-top">
        <span class="text-ink-3">☰</span>
        <span class="crumbs">acme <span>›</span> payments <span>›</span> <b>Overview</b></span>
        <span class="sh-search">⌕ <span>Search pages, agents, tools, sessions, flows…</span><kbd>Ctrl K</kbd></span>
        <span class="sh-time"><span>15m</span><span>1h</span><span>4h</span><span class="on">1d</span><span>7d</span><span>30d</span><span>All</span></span>
        <span class="sh-tag live ml-auto sm:ml-0">Live</span>
      </div>
      <div class="sh-body">
        <div class="sh-hero"><span><b>acme/payments</b><small>All 4 members · last 24 hours · no filter</small></span><span class="sh-tag ml-auto">3 agents active</span></div>
        <div class="sh-kpis">
          <div class="sh-kpi"><small>Tool calls</small><b data-kpi="calls">1,284</b></div>
          <div class="sh-kpi bad"><small>Denied</small><b data-kpi="denied">37</b></div>
          <div class="sh-kpi good"><small>Confirmed bypasses</small><b>0</b></div>
          <div class="sh-kpi"><small>Approvals</small><b data-kpi="asks">9</b></div>
          <div class="sh-kpi"><small>Tokens</small><b data-kpi="tokens">412k</b></div>
        </div>
        <div class="sh-panels">
          <div class="sh-pnl"><div class="sh-pnl-head">Decisions per hour<span><i class="bg-white/60"></i>allowed</span><span><i class="bg-red-400"></i>denied</span></div><svg class="sh-chart" viewBox="0 0 480 104" preserveAspectRatio="none" data-chart><defs><linearGradient id="sh-bar" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="#fff" stop-opacity=".42"/><stop offset="1" stop-color="#fff" stop-opacity=".1"/></linearGradient></defs><g stroke="rgba(255,255,255,.05)"><path d="M0 26H480M0 52H480M0 78H480"/></g></svg></div>
          <div class="sh-pnl side"><div class="sh-pnl-head">By agent<span>calls · denied</span></div><div class="sh-agents" data-agents>${AGENTS.map(([name, color]) => `<div><span class="truncate">${name}</span><s><u style="background:${color};width:0"></u><u style="width:0"></u></s><em>0</em></div>`).join("")}</div></div>
        </div>
        <div class="sh-stream" data-stream><div class="sh-row head"><i class="!bg-transparent"></i><span>Time</span><span>Event</span><span>Decision</span><span>Tool</span><span>Agent</span></div></div>
      </div>
      <aside class="sh-drawer" data-drawer>
        <header><span class="min-w-0 flex-1"><small>Event · F05 · tool call decided</small><b>TOOL_CALL_DENIED_RULE</b></span><span class="text-ink-3">✕</span></header>
        <section><h6>Action</h6><dl class="sh-kv"><dt>action_type</dt><dd>TOOL_CALL</dd><dt>action_source</dt><dd>HOOK</dd><dt>action_tool</dt><dd>EDIT</dd><dt>action_user</dt><dd>maya</dd></dl></section>
        <section><h6>Decision</h6><dl class="sh-kv"><dt>decision</dt><dd><span class="sh-badge bad">DENY</span></dd><dt>decision_rule</dt><dd>PRESERVED_SELECTION</dd><dt>enforcement</dt><dd>PREVENTED</dd><dt>reason</dt><dd>the edit would change preserved selection K7M2P9RX (app/payments.py lines 4-5, policy payments)</dd></dl></section>
        <section><h6>Severity · bindings</h6><dl class="sh-kv"><dt>severity_status</dt><dd class="text-red-300">HIGH</dd><dt>selection</dt><dd>K7M2P9RX · preserve</dd><dt>zone</dt><dd>billing</dd></dl></section>
        <section data-chain><h6>Evidence</h6><dl class="sh-kv"><dt>sequence</dt><dd>4,812</dd><dt>previous_digest</dt><dd>c07a…51d2</dd><dt>digest</dt><dd>3be1…94af <span class="sh-badge ok" data-verified>✓ chain verified</span></dd></dl></section>
      </aside>
    </div>
  </div>
</div>`,
    async play(p) {
      const random = (low, high) => low + Math.random() * (high - low);
      const chart = p.q("[data-chart]");
      const SVG = "http://www.w3.org/2000/svg";
      const hours = Array.from({ length: 24 }, (_, index) => ({ allowed: 30 + 40 * Math.abs(Math.sin(index / 3.2)) + random(0, 14), denied: random(1, 6) }));
      hours[23].allowed = 44;
      const bars = hours.map((_, index) => {
        const allowed = chart.appendChild(document.createElementNS(SVG, "rect"));
        const denied = chart.appendChild(document.createElementNS(SVG, "rect"));
        for (const bar of [allowed, denied]) { bar.setAttribute("x", String(index * 20 + 3)); bar.setAttribute("width", "14"); bar.setAttribute("rx", "2"); bar.setAttribute("y", "104"); bar.setAttribute("height", "0"); }
        allowed.setAttribute("fill", index === 23 ? "rgba(255,255,255,.85)" : "url(#sh-bar)");
        denied.setAttribute("fill", "#f87171");
        return [allowed, denied];
      });
      const draw = () => hours.forEach((hour, index) => {
        const [allowed, denied] = bars[index];
        const a = hour.allowed, d = hour.denied * 2.2;
        allowed.setAttribute("y", String(104 - a - d)); allowed.setAttribute("height", String(a));
        denied.setAttribute("y", String(104 - d)); denied.setAttribute("height", String(d));
      });
      const counts = { "claude-code": [412, 21], codex: [356, 9], copilot: [301, 4], gemini: [215, 3] };
      const drawAgents = () => {
        const rows = p.q("[data-agents]").children;
        AGENTS.forEach(([name], index) => {
          const [calls, denied] = counts[name];
          const [allowedBar, deniedBar] = rows[index].querySelectorAll("u");
          allowedBar.style.width = (calls / 450) * 100 - (denied / 450) * 100 * 3 + "%";
          deniedBar.style.width = (denied / 450) * 100 * 3 + "%";
          rows[index].querySelector("em").textContent = String(calls);
        });
      };
      const totals = { calls: 1284, denied: 37, asks: 9, tokens: 412 };
      const drawTotals = () => {
        p.q('[data-kpi="calls"]').textContent = totals.calls.toLocaleString("en-US");
        p.q('[data-kpi="denied"]').textContent = String(totals.denied);
        p.q('[data-kpi="asks"]').textContent = String(totals.asks);
        p.q('[data-kpi="tokens"]').textContent = totals.tokens + "k";
      };
      const stream = p.q("[data-stream]");
      let clock = 14 * 3600 + 2 * 60 + 11;
      const time = () => { clock += Math.round(random(2, 9)); return [clock / 3600, (clock / 60) % 60, clock % 60].map((part) => String(Math.floor(part)).padStart(2, "0")).join(":"); };
      const add = ([agent, tool, event, decision], fresh = true) => {
        const node = make("div", "sh-row" + (fresh ? " new" : "") + (decision === "DENY" ? " hot" : ""));
        node.append(make("i", decision === "DENY" ? "bad" : decision === "REQUIRE_APPROVAL" ? "ask" : ""), make("span", "text-ink-3", time()), make("span", "text-ink", event));
        const badge = make("span");
        badge.append(make("span", "sh-badge " + DECISION_TONE[decision], decision === "REQUIRE_APPROVAL" ? "APPROVAL" : decision === "NOT_REQUIRED" ? "RESULT" : decision));
        node.append(badge, make("span", "", tool), make("span", "", agent));
        stream.children[0].after(node);
        while (stream.children.length > 9) stream.lastChild.remove();
        return node;
      };
      // One more call: the stream, the totals, the newest bar, and the agent's bar move together
      const tick = (entry) => {
        const node = add(entry);
        totals.calls += 1;
        totals.tokens += Math.round(random(1, 3));
        if (entry[3] === "REQUIRE_APPROVAL") totals.asks += 1;
        hours[23].allowed = Math.min(78, hours[23].allowed + random(1.5, 4));
        counts[entry[0]][0] += 1;
        draw(); drawAgents(); drawTotals();
        return node;
      };
      for (const entry of STREAM.slice().reverse()) add(entry, false);
      await p.wait(50);
      draw(); drawAgents(); drawTotals();

      p.say(1, "Tool calls from every teammate's agents stream in");
      for (const entry of STREAM.slice(0, 4)) { await p.wait(850); tick(entry); }
      await p.wait(600);

      p.say(2, "A denial arrives with its rule and severity");
      await p.focus("[data-stream]", 1.2, 900);
      const denied = tick(DENIED);
      totals.denied += 1;
      counts["claude-code"][1] += 1;
      hours[23].denied += 3;
      draw(); drawAgents(); drawTotals();
      p.flash(p.q('[data-kpi="denied"]').parentElement);
      await p.wait(500);
      const label = p.note(denied, "Denied before it ran · claude-code", "above", "bad");
      await p.wait(1700);

      p.say(3, "Open it: the decision, its reason, and what it was bound to");
      await p.point(denied, 0.4, 0.5, 900);
      await p.click();
      p.unnote(label);
      denied.classList.add("pick");
      p.q("[data-drawer]").classList.add("on");
      p.hide();
      await p.wait(400);
      await p.focus("[data-drawer]", 1.3, 1100);
      await p.wait(1600);

      p.say(4, "Hash-chained evidence of what each agent did, what was denied, and why");
      await p.focus("[data-chain]", 1.35, 900);
      p.flash(p.q("[data-verified]"));
      const chain = p.note("[data-chain]", "Hash-chained · ed25519-signed", "above", "ok");
      await p.wait(2000);
      p.unnote(chain);
      await p.wide(900);
      for (const entry of STREAM.slice(4)) { await p.wait(1000); tick(entry); }
      await p.wait(1000);
    },
  };

  // ---------------------------------------------------------------- the stage
  const SCENES = [CLI, EXT, SEA];
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
    // The pointer lives inside the camera so it zooms with it; keep it while the scene is replaced
    stage.append(pointer);
    pointer.classList.remove("on", "tap", "down");
    screen.innerHTML = '<div class="sh-camera">' + scene.html() + "</div>";
    const camera = screen.firstElementChild;
    camera.append(pointer);
    const bar = tabs[index].querySelector(".show-progress b");
    const play = player(token, camera, bar, scene.duration, scene.steps);
    try {
      await play.wait(500);
      await scene.play(play);
      await play.wait(2600);
    } catch (error) {
      if (error instanceof Stopped) return;
      throw error;
    }
    bar.style.width = "100%";
    if (!STILL) show((index + 1) % SCENES.length);
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
