"use strict";
// Landscape of Agents: the "What's new" section. A resident agent's heartbeat (flat while idle, one
// activation per trigger, then idle again) and Bricks composing into stacks, plus the page's scroll
// hairline. Each scene waits while it is off screen or the tab is hidden, and shows its end state
// under reduced motion.

(() => {
  const STILL = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  const $ = (selector) => document.querySelector(selector);
  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  const html = (tag, className, inner) => { const node = document.createElement(tag); if (className) node.className = className; node.innerHTML = inner; return node; };

  // Waits that hold while nobody can see the scene
  function watch(root) {
    const state = { visible: false };
    new IntersectionObserver((entries) => { state.visible = entries.some((entry) => entry.isIntersecting); }).observe(root);
    state.beat = async (ms) => { await pause(ms); while (!state.visible || document.hidden) await pause(400); };
    return state;
  }

  // ------------------------------------------------------------------ scroll hairline
  const progress = $("#progress");
  if (progress) {
    let queued = false;
    const draw = () => {
      queued = false;
      const room = document.documentElement.scrollHeight - innerHeight;
      progress.style.transform = "scaleX(" + (room > 0 ? Math.min(1, scrollY / room) : 0) + ")";
    };
    addEventListener("scroll", () => { if (!queued) { queued = true; requestAnimationFrame(draw); } }, { passive: true });
    draw();
  }

  // ------------------------------------------------------------------ resident agent
  // Each step of one activation: the step it lights, the runtime status, and what it recorded
  const ACTIVATION = [
    { step: 0, status: "QUEUED", kind: "", mark: "▲", text: "ci.failed · workflow tests · branch main", spike: true },
    { step: 1, status: "CONTEXT_LOADING", kind: "", mark: "·", text: "context HIT · mission, flow, zone, 3 Bricks, RCA v4" },
    { step: 2, status: "INVESTIGATING", kind: "", mark: "·", text: "cause supported · stale migration lock (check passed)" },
    { step: 3, status: "PLAN_READY", kind: "", mark: "·", text: "READ_CHECK allow · NOTIFY_SLACK allow · CREATE_TICKET approval" },
    { step: 4, status: "AWAITING_APPROVAL", kind: "ask", mark: "?", text: "approved by a repository admin on DataSea", hold: 2200 },
    { step: 5, status: "EXECUTING", kind: "", mark: "·", text: "Slack notified · Jira OPS-142 opened (deduplicated by label)" },
    { step: 6, status: "VERIFYING", kind: "ok", mark: "✓", text: "VERIFY passed · ci.passed on main" },
    { step: 7, status: "COMPLETED · proven", kind: "ok", mark: "✓", text: "back to ACTIVE_IDLE · 0 model calls while idle" },
  ];
  const resident = $("#resident");
  if (resident) {
    const view = watch(resident);
    const status = $("#res-status"), steps = [...$("#res-steps").children], log = $("#res-log"), ticks = $("#res-ticks"), canvas = $("#res-pulse");
    let idle = true, idleTicks = 1204, energy = 0, spike = [];
    const setStatus = (text, kind) => { status.textContent = text; status.className = "sh-tag ml-auto" + (kind ? " " + kind : text === "ACTIVE_IDLE" ? "" : " live"); };
    const setStep = (index, kind) => steps.forEach((node, at) => { node.className = at < index ? "done" : at === index ? "on" + (kind === "ask" ? " ask" : "") : ""; });
    const addLine = (entry) => {
      for (const old of log.children) old.classList.add("old");
      log.append(html("li", entry.kind, "<i>" + entry.mark + "</i><span>" + entry.text + "</span>"));
      while (log.children.length > 5) log.firstElementChild.remove();
    };

    // The heartbeat: a flat line while idle, a spike when a trigger lands, small work while it runs
    const context = canvas.getContext("2d");
    const samples = new Array(140).fill(0);
    let last = 0;
    function frame(now) {
      requestAnimationFrame(frame);
      if (!view.visible || now - last < 45) return;
      last = now;
      const ratio = Math.min(2, devicePixelRatio || 1), width = canvas.clientWidth, height = canvas.clientHeight;
      if (canvas.width !== width * ratio) { canvas.width = width * ratio; canvas.height = height * ratio; }
      const next = spike.length ? spike.shift() : idle ? 0 : (Math.random() - 0.5) * energy;
      samples.push(next);
      samples.shift();
      context.setTransform(ratio, 0, 0, ratio, 0, 0);
      context.clearRect(0, 0, width, height);
      const middle = height / 2, step = width / (samples.length - 1);
      const gradient = context.createLinearGradient(0, 0, width, 0);
      gradient.addColorStop(0, "rgba(255,255,255,0)");
      gradient.addColorStop(1, "rgba(255,255,255,.85)");
      context.strokeStyle = gradient;
      context.lineWidth = 1.4;
      context.beginPath();
      samples.forEach((value, at) => { const y = middle - value * (height * 0.42); at ? context.lineTo(at * step, y) : context.moveTo(0, y); });
      context.stroke();
      const head = middle - samples[samples.length - 1] * (height * 0.42);
      context.fillStyle = "#fff";
      context.shadowColor = "rgba(255,255,255,.9)";
      context.shadowBlur = 10;
      context.beginPath();
      context.arc(width - 2, head, 2.4, 0, Math.PI * 2);
      context.fill();
      context.shadowBlur = 0;
    }

    async function play() {
      if (STILL) {
        for (const entry of ACTIVATION.slice(-5)) addLine(entry);
        setStep(7);
        setStatus("COMPLETED · proven", "ok");
        ticks.textContent = idleTicks.toLocaleString();
        return;
      }
      requestAnimationFrame(frame);
      for (;;) {
        // Idle: the line is flat and only the idle tick counter moves
        idle = true;
        setStatus("ACTIVE_IDLE");
        setStep(-1);
        addLine({ kind: "", mark: "·", text: "ACTIVE_IDLE · subscribed to ci.failed on main" });
        for (let tick = 0; tick < 9; tick++) { idleTicks++; ticks.textContent = idleTicks.toLocaleString(); await view.beat(420); }
        idle = false;
        for (const entry of ACTIVATION) {
          if (entry.spike) spike = [0.15, 0.95, -0.7, 0.45, -0.2, 0.1];
          energy = entry.kind === "ask" ? 0.06 : 0.32;
          setStep(entry.step, entry.kind);
          setStatus(entry.status, entry.kind);
          addLine(entry);
          await view.beat(entry.hold || 1250);
        }
        idle = true;
        await view.beat(2400);
        for (const old of log.children) old.classList.add("old");
      }
    }
    play();
  }

  // ------------------------------------------------------------------ bricks
  // The fixed types, their support level today, and what Crane does with each
  const TYPES = [
    ["Context", "delivered", "Common Context and files are delivered before the agent acts in the target."],
    ["Identity", "checked", "Reports to the agent, and records, when the acting agent is not admitted."],
    ["Scope", "enforced", "Files of the target outside the scope become read-only."],
    ["Permission", "enforced", "Without write, the target becomes read-only."],
    ["Constraint", "enforced", "Lowers the ceiling, raises the criticality, and makes protected paths read-only."],
    ["Precondition", "enforced", "While a check fails, every change in the target needs approval."],
    ["Approval", "enforced", "Write or delete approval makes the target assisted."],
    ["Isolation", "enforced", "Requiring an unavailable profile makes direct changes read-only."],
    ["Budget", "declared", "Tool calls, retries, and runtime limits, validated and delivered."],
    ["Action", "checked", "Must name a registered operation. A Brick never executes anything."],
    ["Verification", "checked", "Proven, contradicted, unknown, or not run: tests, selections, files, signed state."],
    ["Provenance", "checked", "Verified against the signed manifest and the audit chain."],
    ["Recovery", "declared", "Retries require reconciliation first."],
    ["Revocation", "checked", "Monitors report a revocation as due; a person signs it."],
    ["Monitoring", "checked", "Deterministic checks on every background sync."],
    ["Intelligence", "checked", "Denial patterns and stale context produce proposals only."],
  ];
  const STACKS = [
    { name: "Production code change", types: ["Context", "Scope", "Constraint", "Approval", "Verification"], target: "flow:checkout", effect: "guard · migrations/ read-only · writes need approval" },
    { name: "Release watch", types: ["Monitoring", "Provenance", "Revocation", "Recovery"], target: "zone:payments", effect: "monitors · checked on every sync · revocation reported when due" },
    { name: "Least privilege", types: ["Identity", "Permission", "Precondition", "Isolation"], target: "flow:refunds", effect: "guard · no write grant → target read-only" },
  ];
  const bricks = $("#bricks");
  if (bricks) {
    const view = watch(bricks);
    const grid = $("#bk-grid"), caption = $("#bk-caption"), name = $("#bk-name"), stack = $("#bk-stack"), attach = $("#bk-attach"), effect = $("#bk-effect");
    const DEFAULT = caption.textContent;
    const tiles = {};
    for (const [type, support, help] of TYPES) {
      const tile = document.createElement("button");
      tile.type = "button";
      tile.className = "bk";
      tile.setAttribute("aria-describedby", "bk-caption");
      tile.innerHTML = '<i class="bk-dot s-' + support + '" aria-hidden="true"></i><span>' + type + "</span>";
      const show = () => (caption.innerHTML = "<b>" + type + "</b> · " + support + " · " + help);
      tile.addEventListener("mouseenter", show);
      tile.addEventListener("focus", show);
      tile.addEventListener("mouseleave", () => { if (document.activeElement !== tile) caption.textContent = DEFAULT; });
      tile.addEventListener("blur", () => (caption.textContent = DEFAULT));
      const item = document.createElement("li");
      item.append(tile);
      grid.append(item);
      tiles[type] = tile;
    }
    const clear = () => {
      for (const tile of Object.values(tiles)) tile.classList.remove("on");
      stack.replaceChildren();
      attach.className = effect.className = "bk-line";
      attach.textContent = effect.textContent = "";
    };
    const finish = (item) => {
      attach.innerHTML = "attach " + item.name.toLowerCase().replace(/ /g, "-") + "@1 → " + item.target + " · <b>signed ✓</b>";
      attach.classList.add("on");
      effect.textContent = item.effect;
      effect.classList.add("on");
    };

    async function play() {
      if (STILL) {
        const item = STACKS[0];
        name.textContent = item.name;
        for (const type of item.types) { tiles[type].classList.add("on"); stack.append(html("span", "sh-badge info", type.toLowerCase() + "@1")); }
        finish(item);
        return;
      }
      for (let round = 0; ; round = (round + 1) % STACKS.length) {
        const item = STACKS[round];
        clear();
        name.textContent = item.name;
        await view.beat(700);
        for (const type of item.types) {
          tiles[type].classList.add("on");
          stack.append(html("span", "sh-badge info", type.toLowerCase() + "@1"));
          await view.beat(520);
        }
        await view.beat(400);
        finish(item);
        await view.beat(3200);
      }
    }
    play();
  }
})();
