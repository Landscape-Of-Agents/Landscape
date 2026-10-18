"use strict";
// Agent Landscape: the latest Crane release and download totals (read from GitHub on every visit,
// so new releases appear without redeploying), a visit counter, install snippets, the CLI
// reference, the feedback form, and the page's motion (all of it off under reduced motion).

const REPO = "Landscape-Of-Agents/Landscape";
const RELEASES = "https://github.com/" + REPO + "/releases";
// Free, public counter (abacus.jasoncameron.dev): one hit per browser session
const VISITS = "https://abacus.jasoncameron.dev";
const VISITS_KEY = "agent-landscape/visits";
const STILL = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

const COMMANDS = [
  ["Basics", "crane --version", "Print the installed Crane version."],
  ["Basics", "crane help", "Print the full command reference with every option."],
  ["Basics", "crane init", "Create the repository's signed rules and this machine's signing key (owner), or join them after confirming the owner key (teammate)."],
  ["Basics", "crane checkpoint baseline", "Record the current commit as a trusted checkpoint. Selections and later checks are compared against it."],
  ["Selections", "crane protect FILE start-line N end-line N", "Preserve lines: agents may not change them. Crane inserts anchor comments, signs the selection, and adds it to the policy. Options: policy NAME, checkpoint NAME, name LABEL."],
  ["Selections", "crane target FILE start-line N end-line N change_type logical_bn", "Target lines: the task passes only once the agent changes them, optionally by kind: logical_bn, logical_cn, logical_sn, or semantic."],
  ["Selections", "crane --set default policy NAME", "Use this policy for new selections when none is given."],
  ["Selections", "crane --set default checkpoint NAME", "Use this checkpoint for new selections when none is given."],
  ["Checks", "crane validate --json", "Verify integrity, signatures, policy syntax, markers, zones, and flows."],
  ["Checks", "crane test . --json", "Run every preserve and target rule. A policy passes only if all its rules pass."],
  ["Checks", "crane parse file FILE", "Check the syntax of one policy file."],
  ["Policies", "crane create policy NAME FILENAME", "Create a policy in .crane/policies/FILENAME.crane."],
  ["Policies", "crane add policy NAME marker MARKER", "Move a selection's rule into another policy."],
  ["Policies", "crane policy NAME status", "Run one policy end to end, with an enforcement self-test and a check of the hooks and CI."],
  ["Policies", "crane add policy-context file FILE policy NAME", "Attach a text file as guidance agents receive with the policy."],
  ["Policies", "crane policy-context NAME --view", "Show the context files bound to a policy."],
  ["Zones", "crane governance sign", "Approve .crane/governance.yaml (zones, flows, context packs) with an admin key. Refused while flows form a cycle."],
  ["Zones", "crane governance graph --json", "Zones, flows in stage order, and selections as an acyclic graph, with the agents active in each."],
  ["Repository", "crane repo --https https://github.com/OWNER/REPO", "Connect this clone to its GitHub repository (or --ssh, --gh-cli), with optional privileges."],
  ["Repository", "crane repo status", "Check the live connection: CONNECTED, NOT_CONNECTED, or FAILURE."],
  ["Repository", "crane repo --view", "Show the privileges granted to Crane and the GitHub grant."],
  ["Repository", "crane repo del", "Delete the repository connection."],
  ["Repository", "crane repo rebind OLD_REMOTE", "After a transfer or rename: prove the move and sign the rules again for the new remote."],
  ["Repository", "crane github", "Sign in with gh and grant read access for this terminal session."],
  ["Repository", "crane github days DAYS", "Grant read access through gh for a number of days."],
  ["Repository", "crane data clear", "Permanently clear this repository's hosted data. CODEOWNERS only."],
  ["Sessions", "crane task current", "Show the current task of the most recently active agent session."],
  ["Sessions", "crane task .", "List the tasks of the current session."],
  ["Sessions", "crane session current", "Show the current agent session."],
  ["Sessions", "crane session .", "List every recorded agent session."],
  ["Agents", "crane agent install --profile claude", "Install Crane's hooks for claude, codex, copilot, or gemini."],
  ["Agents", "crane agent uninstall --profile claude", "Remove Crane's hooks for an agent. Refused inside an agent session."],
  ["Agents", "crane agent hooks --profile claude", "Validate an agent's installed hooks."],
  ["Agents", "crane agent link", "Link this machine's upload key to your GitHub account."],
  ["Admins", "crane admin register", "Owner: ask a GitHub admin to confirm the owner key on the dashboard."],
  ["Admins", "crane admin list", "Show the owner, the admins, and this machine's role."],
  ["Admins", "crane admin add PUBLIC_KEY name NAME", "Owner: let another key sign governance changes."],
  ["Admins", "crane admin del PUBLIC_KEY", "Owner: revoke an admin key."],
  ["Alerts", "crane integrate slack", "Set up an integration: jira, whatsapp, slack, or github."],
  ["Alerts", "crane alert add NAME metric METRIC above N window 15m", "Add an alert rule on a metric, with channels, severity, cooldown, and filters."],
  ["Alerts", "crane alert list --json", "List the alert rules with their state."],
  ["Alerts", "crane alert check --json", "Evaluate every rule now and deliver new notifications."],
  ["Alerts", "crane alert test NAME", "Send a test notification (also remove, enable, or disable NAME)."],
  ["Alerts", "crane alert log --json", "Show the history of firings, resolutions, and deliveries."],
  ["Alerts", "crane alert metrics", "List the metrics an alert rule can use."],
].map(([group, command, help]) => ({ group, command, help }));

const $ = (selector, root) => (root || document).querySelector(selector);
const $$ = (selector, root) => [...(root || document).querySelectorAll(selector)];
const make = (tag, attributes, ...children) => {
  const node = document.createElement(tag);
  for (const [key, value] of Object.entries(attributes || {})) {
    if (key.startsWith("on")) node.addEventListener(key.slice(2), value);
    else if (value !== false && value != null) node.setAttribute(key, value === true ? "" : value);
  }
  node.append(...children.filter((child) => child != null));
  return node;
};
const store = (kind) => ({
  get(key) { try { return JSON.parse(window[kind].getItem(key)); } catch { return null; } },
  set(key, value) { try { window[kind].setItem(key, JSON.stringify(value)); } catch { /* blocked storage */ } },
});
const session = store("sessionStorage");
const COPY_ICON = '<svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/></svg>';
const CHECK_ICON = '<svg viewBox="0 0 24 24" class="h-4 w-4" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><path d="m5 12 5 5L20 7"/></svg>';

// ------------------------------------------------------------------ toast and copy
let toastTimer;
function toast(text) {
  const node = $("#toast");
  node.textContent = text;
  node.classList.remove("opacity-0", "translate-y-4");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => node.classList.add("opacity-0", "translate-y-4"), 1600);
}
async function copy(text, button) {
  try { await navigator.clipboard.writeText(text); } catch {
    const area = make("textarea", { class: "fixed opacity-0", "aria-hidden": "true" });
    area.value = text;
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
  toast("Copied");
  if (button) { const before = button.innerHTML; button.innerHTML = CHECK_ICON; setTimeout(() => (button.innerHTML = before), 1200); }
}
for (const button of $$("[data-copy]")) button.addEventListener("click", () => copy(document.getElementById(button.dataset.copy).textContent, button));

// ------------------------------------------------------------------ motion
// Sections fade up once; cards light up under the pointer
const seen = new IntersectionObserver((entries) => {
  for (const entry of entries) if (entry.isIntersecting) { entry.target.classList.add("seen"); seen.unobserve(entry.target); }
}, { rootMargin: "0px 0px -10% 0px" });
for (const node of $$(".reveal")) seen.observe(node);
for (const card of $$(".spotlight")) {
  card.addEventListener("pointermove", (event) => {
    const box = card.getBoundingClientRect();
    card.style.setProperty("--x", event.clientX - box.left + "px");
    card.style.setProperty("--y", event.clientY - box.top + "px");
  });
}

// AgentScript at work: the agent tries to change the preserved fee (denied, with its context),
// rewrites the targeted refund (passes), and crane test reports both rules passing; then again.
// While the hero is off screen or the tab is hidden it waits; under reduced motion it shows the
// end state.
const scene = {
  root: $("#scene"),
  cursor: $("#scene-cursor"),
  toast: $("#scene-deny"),
  agent: $("#scene-agent"),
  result: $("#scene-result"),
  preserve: $("#block-preserve"),
  target: $("#block-target"),
  rulePreserve: $("#rule-preserve"),
  ruleTarget: $("#rule-target"),
  fee: $("#fee-rate"),
  refund: $("#refund-code"),
};
const REFUND = "order.total - order.refunded";
let sceneVisible = true;
new IntersectionObserver((entries) => { sceneVisible = entries.some((entry) => entry.isIntersecting); }).observe(scene.root);
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
// Waits that hold while nobody can see the scene
async function beat(ms) { await pause(ms); while (!sceneVisible || document.hidden) await pause(400); }
// Put the cursor at the end of a line of code
// Positions come from the screen, relative to the code area the cursor and the message live in
const sceneHost = () => scene.cursor.parentElement.getBoundingClientRect();
function cursorTo(line, offset) {
  const host = sceneHost();
  const text = (line.lastElementChild || line).getBoundingClientRect();
  const row = line.getBoundingClientRect();
  const x = Math.min(host.width - 70, text.right - host.left + (offset || 2));
  scene.cursor.style.transform = "translate(" + x + "px," + (row.top - host.top + row.height / 2 - 9) + "px)";
}
function sceneReset() {
  for (const node of [scene.preserve, scene.target, scene.rulePreserve, scene.ruleTarget]) node.classList.remove("deny", "pass");
  scene.toast.classList.remove("on");
  scene.result.classList.remove("on");
  scene.cursor.classList.remove("on");
  scene.fee.textContent = "0.03";
  scene.fee.className = "";
  scene.refund.textContent = "order.total";
  scene.refund.className = "";
  scene.agent.textContent = "claude-code is working";
}
function sceneEnd() {
  sceneReset();
  scene.rulePreserve.classList.add("deny");
  scene.preserve.classList.add("deny");
  scene.refund.textContent = REFUND;
  scene.ruleTarget.classList.add("pass");
  scene.target.classList.add("pass");
  scene.result.classList.add("on");
}
async function playScene() {
  if (STILL) { sceneEnd(); return; }
  const feeLine = $("#line-fee"), refundLine = $("#line-refund");
  for (;;) {
    sceneReset();
    await beat(700);
    // 1. The agent goes for the preserved fee
    cursorTo(feeLine);
    scene.cursor.classList.add("on");
    scene.agent.textContent = "claude-code · Edit app/payments.py";
    await beat(1100);
    scene.fee.className = "scene-struck";
    await beat(500);
    // 2. Crane denies it before it runs, and gives the agent the selection's context
    scene.preserve.classList.add("deny");
    scene.rulePreserve.classList.add("deny");
    scene.toast.style.top = feeLine.getBoundingClientRect().bottom - sceneHost().top + 6 + "px";
    scene.toast.classList.add("on");
    await beat(2600);
    scene.toast.classList.remove("on");
    scene.fee.className = "";
    // 3. The agent rewrites the targeted refund instead
    cursorTo(refundLine);
    await beat(1000);
    scene.refund.textContent = "";
    scene.refund.className = "scene-typed";
    for (const character of REFUND) {
      scene.refund.textContent += character;
      cursorTo(refundLine, 3);
      await pause(28 + Math.random() * 30);
    }
    await beat(500);
    scene.refund.className = "";
    scene.target.classList.add("pass");
    scene.ruleTarget.classList.add("pass");
    scene.cursor.classList.remove("on");
    // 4. crane test agrees
    await beat(700);
    scene.agent.textContent = "claude-code is done";
    scene.result.classList.add("on");
    await beat(3800);
  }
}
playScene();

// ------------------------------------------------------------------ product mockups
// Each mockup moves only while it is on screen and the tab is visible; under reduced motion it
// shows a finished still.
function onScreen(node) {
  const state = { visible: false };
  new IntersectionObserver((entries) => { state.visible = entries.some((entry) => entry.isIntersecting); }).observe(node);
  return () => state.visible && !document.hidden;
}
async function whenShown(shown) { while (!shown()) await pause(400); }

// Crane: validate, test, then a hook denies an agent's edit, typed out line by line
const CRANE_SCRIPT = [
  ["cmd", "crane validate"],
  ["ok", "manifest signature", "ed25519 · owner 7f3a…c21e"],
  ["ok", "selections sealed", "4 · sha-512"],
  ["ok", "governance graph", "acyclic · 3 zones · 2 flows"],
  ["pass", "PASS", "integrity verified in 41 ms"],
  ["cmd", "crane test ."],
  ["ok", "preserve K7M2P9RX", "untouched"],
  ["ok", "target 4QZ8M2LD", "logical_bn changed"],
  ["pass", "PASS", "payments · 2/2 rules"],
  ["hook", "claude-code", "Edit app/payments.py:2"],
  ["deny", "DENIED", "K7M2P9RX · \"Fees are audited; keep the rate.\""],
];
function craneLine([kind, label, detail]) {
  const span = (className, text) => make("span", { class: className }, text);
  if (kind === "cmd") return make("div", { class: "term-line" }, span("text-ink-3", "$"), span("text-ink", ""));
  if (kind === "ok") return make("div", { class: "term-line" }, span("text-green-300", "✓"), span("label", label), span("detail", detail));
  if (kind === "pass") return make("div", { class: "term-line" }, span("term-badge bg-green-400/15 text-green-300", label), span("detail text-ink-2", detail));
  if (kind === "hook") return make("div", { class: "term-line mt-1" }, span("text-violet-300", "◆ hook"), span("text-ink-2", label), span("detail", detail));
  return make("div", { class: "term-line" }, span("term-badge bg-red-400/15 text-red-300", label), span("detail text-red-200/80", detail));
}
async function playCrane() {
  const term = $("#crane-term");
  if (STILL) { for (const step of CRANE_SCRIPT) { const line = craneLine(step); if (step[0] === "cmd") line.lastChild.textContent = step[1]; term.append(line); } return; }
  const shown = onScreen(term);
  for (;;) {
    term.replaceChildren();
    for (const step of CRANE_SCRIPT) {
      await whenShown(shown);
      const line = term.appendChild(craneLine(step));
      if (step[0] === "cmd") {
        const typed = line.lastChild;
        const caret = line.appendChild(make("span", { class: "scene-caret inline-block h-3.5 translate-y-0.5" }));
        await pause(500);
        for (const letter of step[1]) { typed.textContent += letter; await pause(45 + Math.random() * 50); }
        await pause(350);
        caret.remove();
      } else {
        await pause(step[0] === "pass" || step[0] === "hook" ? 520 : 260);
      }
    }
    await pause(4200);
  }
}
playCrane();

// Landscape: the agent moves between zones; its edges and zone light up
function playLandscape() {
  const graph = $("#land-graph");
  const label = $("#land-agent");
  const zones = ["billing", "ledger"];
  const AGENTS = ["claude-code", "codex", "copilot"];
  let turn = 0;
  const show = () => {
    const zone = zones[turn % zones.length];
    for (const node of $$("[data-zone]", graph)) node.classList.toggle("hot", node.dataset.zone === zone);
    label.lastChild.textContent = `${AGENTS[turn % AGENTS.length]} · ${zone}`;
  };
  show();
  if (STILL) { graph.pauseAnimations?.(); return; }
  const shown = onScreen(graph);
  setInterval(() => { if (shown()) { turn += 1; show(); } }, 3200);
}
playLandscape();

// Data Sea: KPIs, sparklines, the decisions chart, and an activity feed that ticks like live data
function playSea() {
  const root = $("#dashboard");
  const random = (low, high) => low + Math.random() * (high - low);
  // A random walk inside [low, high], so the data drifts instead of jumping
  const walk = (last, low, high) => Math.min(high, Math.max(low, last + random(-1, 1) * (high - low) * 0.18));
  const series = (count, low, high) => { const points = [random(low, high)]; while (points.length < count) points.push(walk(points.at(-1), low, high)); return points; };
  const RANGES = { decisions: [30, 90], denied: [5, 30], incidents: [0, 12], tokens: [40, 95] };
  const allowed = series(24, 40, 95);
  const denied = series(24, 4, 22);
  const sparks = Object.fromEntries(Object.entries(RANGES).map(([name, [low, high]]) => [name, series(14, low, high)]));
  const totals = { decisions: 1284, denied: 37, incidents: 2, tokens: 412 };
  // A smooth path through the points, scaled into a width × height box (top is the maximum)
  const curve = (points, width, height, max) => {
    const step = width / (points.length - 1);
    const xy = points.map((value, index) => [index * step, height - (value / max) * (height - 6) - 2]);
    return xy.reduce((path, [x, y], index) => {
      if (!index) return `M${x} ${y}`;
      const [px, py] = xy[index - 1];
      return `${path} C${px + step / 2} ${py} ${x - step / 2} ${y} ${x} ${y}`;
    }, "");
  };
  const drawChart = () => {
    for (const [name, points] of [["allowed", allowed], ["denied", denied]]) {
      const line = curve(points, 400, 130, 100);
      $(`#sea-${name}-line`).setAttribute("d", line);
      $(`#sea-${name}-area`).setAttribute("d", `${line} L400 130 L0 130 Z`);
    }
  };
  const drawSparks = () => {
    for (const [name, points] of Object.entries(sparks)) {
      const max = Math.max(...points, 1);
      $(`#spark-${name}`).setAttribute("points", points.map((value, index) => `${(index * 80) / (points.length - 1)},${20 - (value / max) * 17}`).join(" "));
    }
  };
  const drawTotals = () => {
    $("#kpi-decisions").textContent = totals.decisions.toLocaleString("en-US");
    $("#kpi-denied").textContent = totals.denied;
    $("#kpi-incidents").textContent = totals.incidents;
    $("#kpi-tokens").textContent = totals.tokens + "k";
  };
  const EVENTS = [
    ["claude-code", "#f97316", "Edit app/payments.py:2", "DENIED", "bg-red-400/15 text-red-300"],
    ["codex", "#60a5fa", "Bash pytest -q", "ALLOWED", "bg-green-400/15 text-green-300"],
    ["copilot", "#c4b5fd", "Read billing/ledger.py", "CONTEXT", "bg-violet-400/15 text-violet-300"],
    ["claude-code", "#f97316", "Edit app/refunds.py:4", "ALLOWED", "bg-green-400/15 text-green-300"],
    ["gemini", "#34d399", "Write .crane/policy", "DENIED", "bg-red-400/15 text-red-300"],
    ["codex", "#60a5fa", "Edit ledger/post.py:18", "ALLOWED", "bg-green-400/15 text-green-300"],
  ];
  const feed = $("#sea-feed");
  let next = 0;
  const addEvent = () => {
    const [agent, color, what, verdict, tone] = EVENTS[next++ % EVENTS.length];
    feed.prepend(make("li", { class: "feed-row" },
      make("span", { class: "who" }, make("i", { style: `background:${color}` }), agent),
      make("span", { class: `term-badge ${tone}` }, verdict),
      make("span", { class: "what" }, what)));
    while (feed.children.length > 4) feed.lastChild.remove();
    return verdict;
  };
  for (let index = 0; index < 4; index += 1) addEvent();
  drawChart(); drawSparks(); drawTotals();
  if (STILL) return;
  const shown = onScreen(root);
  setInterval(() => {
    if (!shown()) return;
    const verdict = addEvent();
    totals.decisions += Math.round(random(2, 9));
    if (verdict === "DENIED") totals.denied += 1;
    totals.tokens += Math.round(random(1, 4));
    allowed.shift(); allowed.push(walk(allowed.at(-1), 40, 95));
    denied.shift(); denied.push(verdict === "DENIED" ? Math.max(denied.at(-1), 18) + random(0, 6) : walk(denied.at(-1), 4, 22));
    for (const [name, points] of Object.entries(sparks)) { points.shift(); points.push(walk(points.at(-1), ...RANGES[name])); }
    drawChart(); drawSparks(); drawTotals();
  }, 2200);
}
playSea();

// ------------------------------------------------------------------ download and install
const TARGETS = { linux: "x86_64-unknown-linux-gnu", macos: "aarch64-apple-darwin", windows: "x86_64-pc-windows-msvc" };
const INSTALL = {
  linux: (tag) => `curl -LO ${RELEASES}/download/${tag}/crane-${tag}-x86_64-unknown-linux-gnu.tar.gz
tar -xzf crane-${tag}-x86_64-unknown-linux-gnu.tar.gz
sudo mv crane-${tag}-x86_64-unknown-linux-gnu/crane /usr/local/bin/
crane --version`,
  macos: (tag) => `# Apple silicon; use x86_64-apple-darwin on Intel
curl -LO ${RELEASES}/download/${tag}/crane-${tag}-aarch64-apple-darwin.tar.gz
tar -xzf crane-${tag}-aarch64-apple-darwin.tar.gz
xattr -d com.apple.quarantine crane-${tag}-aarch64-apple-darwin/crane
sudo mv crane-${tag}-aarch64-apple-darwin/crane /usr/local/bin/`,
  windows: (tag) => `# PowerShell
Invoke-WebRequest ${RELEASES}/download/${tag}/crane-${tag}-x86_64-pc-windows-msvc.zip -OutFile crane.zip
Expand-Archive crane.zip -DestinationPath "$env:LOCALAPPDATA\\Crane"
$bin = "$env:LOCALAPPDATA\\Crane\\crane-${tag}-x86_64-pc-windows-msvc"
[Environment]::SetEnvironmentVariable("Path", "$env:Path;$bin", "User")`,
};
const NAMES = { linux: "Linux", macos: "macOS", windows: "Windows" };
let latest = null;
function detect() {
  const agent = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent || "";
  return /win/i.test(agent) ? "windows" : /mac|iphone|ipad/i.test(agent) ? "macos" : "linux";
}
let os = detect();
function formatSize(bytes) { return bytes >= 1048576 ? (bytes / 1048576).toFixed(1) + " MB" : Math.max(1, Math.round(bytes / 1024)) + " KB"; }
function showOs(next) {
  os = next;
  for (const tab of $$("#os-tabs [role=tab]")) { const on = tab.dataset.os === os; tab.setAttribute("aria-selected", String(on)); tab.tabIndex = on ? 0 : -1; }
  const tag = latest ? latest.tag_name : "vX.Y.Z";
  $("#install-code").textContent = INSTALL[os](tag);
  $("#install-caption").textContent = "Install on " + NAMES[os];
  const asset = latest && latest.assets.find((item) => item.name.includes(TARGETS[os]) && !item.name.endsWith(".sha256"));
  const button = $("#download-button");
  button.href = asset ? asset.browser_download_url : RELEASES + "/latest";
  $("#download-label").textContent = "Download for " + NAMES[os] + (asset ? " · " + formatSize(asset.size) : "");
}
$$("#os-tabs [role=tab]").forEach((tab, index, tabs) => {
  tab.addEventListener("click", () => showOs(tab.dataset.os));
  tab.addEventListener("keydown", (event) => {
    const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
    if (!step) return;
    const next = tabs[(index + step + tabs.length) % tabs.length];
    showOs(next.dataset.os);
    next.focus();
  });
});
showOs(os);

// ------------------------------------------------------------------ numbers
const MILESTONES = [10, 25, 50, 100, 250, 500, 1000, 2500, 5000, 10000, 25000, 50000, 100000, 250000, 500000, 1000000];
const compact = new Intl.NumberFormat(undefined, { notation: "compact", maximumFractionDigits: 1 });
// Numbers count up once the band is on screen; data arriving later animates then
let statsVisible = false;
function setStat(name, value) {
  const node = $(`[data-stat="${name}"]`);
  if (!node || !Number.isFinite(value)) return;
  node.dataset.value = String(value);
  const milestone = $(`[data-milestone="${name}"]`);
  const crossed = MILESTONES.filter((mark) => value >= mark).pop();
  if (milestone && crossed) milestone.textContent = "Crossed " + compact.format(crossed) + (name === "downloads" ? " downloads" : " visits");
  if (statsVisible) countUp(node);
}
function countUp(node) {
  const target = Number(node.dataset.value);
  if (!Number.isFinite(target)) return;
  const show = (value) => (node.textContent = value >= 10000 ? compact.format(value) : Math.round(value).toLocaleString());
  if (STILL) return show(target);
  const start = performance.now();
  const from = Number(node.dataset.shown || 0);
  const step = (now) => {
    const progress = Math.min(1, (now - start) / 1400);
    const value = from + (target - from) * (1 - Math.pow(1 - progress, 3));
    show(value);
    node.dataset.shown = String(value);
    if (progress < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}
new IntersectionObserver((entries, observer) => {
  if (!entries.some((entry) => entry.isIntersecting)) return;
  statsVisible = true;
  for (const node of $$("[data-stat]")) countUp(node);
  observer.disconnect();
}, { threshold: 0.3 }).observe($("#stats"));

async function loadReleases() {
  // The cache is per repository, so a move to another repository never shows the old releases
  const CACHE_KEY = "releases:" + REPO;
  const cached = session.get(CACHE_KEY);
  let releases = cached && Date.now() - cached.at < 10 * 60 * 1000 ? cached.releases : null;
  if (!releases) {
    try {
      const response = await fetch("https://api.github.com/repos/" + REPO + "/releases?per_page=100", { headers: { Accept: "application/vnd.github+json" } });
      if (!response.ok) throw new Error(String(response.status));
      // Keep only what the page uses, so the cache stays small
      releases = (await response.json()).map((release) => ({
        tag_name: release.tag_name, html_url: release.html_url, published_at: release.published_at, draft: release.draft, prerelease: release.prerelease,
        assets: release.assets.map((asset) => ({ name: asset.name, size: asset.size, download_count: asset.download_count, browser_download_url: asset.browser_download_url })),
      }));
      session.set(CACHE_KEY, { at: Date.now(), releases });
    } catch {
      releases = cached && cached.releases;
    }
  }
  if (!releases || !releases.length) { $("#release-meta").textContent = "Binaries for Linux, Windows, and macOS."; return; }
  // The newest stable release that carries binaries (a release whose build failed has none)
  const stable = releases.filter((release) => !release.draft && !release.prerelease);
  latest = stable.find((release) => release.assets.some((asset) => !asset.name.endsWith(".sha256"))) || stable[0] || releases[0];
  const date = latest.published_at ? new Date(latest.published_at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "";
  $("#release-meta").textContent = "Crane " + latest.tag_name + (date ? " · released " + date : "");
  $("#release-chip").textContent = "Crane " + latest.tag_name + " is out";
  $("#release-page").href = latest.html_url;
  showOs(os);
  const published = releases.filter((release) => !release.draft);
  setStat("releases", published.length);
  setStat("downloads", published.reduce((sum, release) => sum + release.assets.filter((asset) => !asset.name.endsWith(".sha256")).reduce((total, asset) => total + asset.download_count, 0), 0));
}
async function loadVisits() {
  // A visit counts once per browser session; later page views, and local previews, only read it
  const local = /^(localhost|127\.0\.0\.1|\[::1\])$/.test(location.hostname);
  const counted = local || session.get("visit-counted");
  try {
    const response = await fetch(VISITS + (counted ? "/get/" : "/hit/") + VISITS_KEY);
    // The counter does not exist until the first visit is counted
    const { value } = response.status === 404 ? { value: 0 } : response.ok ? await response.json() : Promise.reject(new Error(String(response.status)));
    session.set("visit-counted", true);
    setStat("visits", value);
  } catch {
    const node = $('[data-stat="visits"]');
    node.textContent = "—";
    $('[data-milestone="visits"]').textContent = "counter unavailable";
  }
}
loadReleases();
loadVisits();

// ------------------------------------------------------------------ commands
const tooltip = $("#tooltip");
let tipOwner = null;
function showTip(button, item) {
  tooltip.replaceChildren(make("b", { class: "mb-1 block break-all font-mono text-xs font-normal text-ink" }, item.command), document.createTextNode(item.help));
  tooltip.hidden = false;
  const box = button.getBoundingClientRect();
  const left = Math.min(window.innerWidth - tooltip.offsetWidth - 12, Math.max(12, box.right - tooltip.offsetWidth));
  const top = box.top - tooltip.offsetHeight - 8 < 8 ? box.bottom + 8 : box.top - tooltip.offsetHeight - 8;
  tooltip.style.left = left + "px";
  tooltip.style.top = top + "px";
  if (tipOwner && tipOwner !== button) tipOwner.setAttribute("aria-expanded", "false");
  tipOwner = button;
  button.setAttribute("aria-expanded", "true");
}
function hideTip() { tooltip.hidden = true; if (tipOwner) tipOwner.setAttribute("aria-expanded", "false"); tipOwner = null; }
document.addEventListener("keydown", (event) => { if (event.key === "Escape") hideTip(); });
window.addEventListener("scroll", hideTip, { passive: true });

const GROUPS = [...new Set(COMMANDS.map((item) => item.group))];
let group = "Basics";
function renderGroups() {
  $("#command-groups").replaceChildren(...GROUPS.map((name) => make("button", {
    type: "button",
    "aria-pressed": String(name === group),
    class: "shrink-0 rounded-full border px-3 py-1 text-xs transition " + (name === group ? "border-white/25 bg-white/10 text-ink" : "border-white/[0.08] text-ink-3 hover:text-ink"),
    onclick: () => { group = name; $("#command-search").value = ""; renderGroups(); renderCommands(); },
  }, name)));
}
function renderCommands() {
  const query = $("#command-search").value.trim().toLowerCase();
  // Searching looks through every area; otherwise one area is shown
  const shown = COMMANDS.filter((item) => (query ? (item.command + " " + item.help).toLowerCase().includes(query) : item.group === group));
  $("#command-groups").classList.toggle("opacity-40", Boolean(query));
  $("#command-list").replaceChildren(...(shown.length ? shown.map((item) => {
    const help = make("button", { type: "button", class: "icon-btn font-medium", "aria-label": "What " + item.command + " does", "aria-expanded": "false", "aria-describedby": "tooltip" }, "?");
    help.addEventListener("mouseenter", () => showTip(help, item));
    help.addEventListener("focus", () => showTip(help, item));
    help.addEventListener("click", () => (tipOwner === help ? hideTip() : showTip(help, item)));
    help.addEventListener("mouseleave", () => { if (document.activeElement !== help) hideTip(); });
    help.addEventListener("blur", hideTip);
    const button = make("button", { type: "button", class: "icon-btn", "aria-label": "Copy " + item.command });
    button.innerHTML = COPY_ICON;
    button.addEventListener("click", () => copy(item.command, button));
    return make("li", { class: "flex items-center gap-1 rounded-lg px-2.5 py-1.5 transition hover:bg-white/[0.03]" },
      make("code", { class: "min-w-0 flex-1 break-words text-[13px] text-ink" }, item.command), help, button);
  }) : [make("li", { class: "px-3 py-6 text-center text-sm text-ink-3" }, "No command matches.")]));
}
$("#command-search").addEventListener("input", renderCommands);
renderGroups();
renderCommands();

// ------------------------------------------------------------------ feedback
// The form posts to FormSubmit, which emails it with the images attached, then returns here
const MAX_IMAGES_BYTES = 5 * 1024 * 1024;
const form = $("#feedback-form");
$("#feedback-next").value = location.origin + location.pathname + "?sent=1#feedback";
if (new URLSearchParams(location.search).get("sent") === "1") {
  $("#feedback-sent").hidden = false;
  history.replaceState(null, "", location.pathname + "#feedback");
}
form.addEventListener("submit", (event) => {
  const error = $("#feedback-error");
  const fail = (text) => { event.preventDefault(); error.textContent = text; error.hidden = false; };
  error.hidden = true;
  const images = [...$("#feedback-images").files];
  if (images.some((file) => !file.type.startsWith("image/"))) return fail("Only images can be attached.");
  if (images.reduce((sum, file) => sum + file.size, 0) > MAX_IMAGES_BYTES) return fail("The images are over 5 MB in total; attach fewer or smaller ones.");
  const links = form.elements.links.value.split(/\s+/).filter(Boolean);
  const bad = links.find((link) => { try { return !/^https?:$/.test(new URL(link).protocol); } catch { return true; } });
  if (bad) return fail("This is not a web link: " + bad);
  const button = $("button[type=submit]", form);
  button.disabled = true;
  button.textContent = "Sending…";
});

// ------------------------------------------------------------------ navigation
const menu = $("#menu");
const mobile = $("#mobile-links");
const setMenu = (open) => { mobile.hidden = !open; menu.setAttribute("aria-expanded", String(open)); menu.setAttribute("aria-label", open ? "Close the menu" : "Open the menu"); };
menu.addEventListener("click", () => setMenu(mobile.hidden));
mobile.addEventListener("click", (event) => { if (event.target.closest("a")) setMenu(false); });
document.addEventListener("keydown", (event) => { if (event.key === "Escape" && !mobile.hidden) { setMenu(false); menu.focus(); } });
$("#year").textContent = String(new Date().getFullYear());
