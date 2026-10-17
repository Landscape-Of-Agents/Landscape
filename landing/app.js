"use strict";
// Agent Landscape: the latest Crane release (read from GitHub on every visit, so a new release
// shows up without redeploying this page), install snippets for it, and the CLI reference.

const REPO = "VishuKalier2003/Agentscript";
const RELEASES = "https://github.com/" + REPO + "/releases";
const CACHE_KEY = "agent-landscape:release";
const CACHE_MS = 10 * 60 * 1000;

// ------------------------------------------------------------------ the CLI reference
const COMMANDS = [
  ["Basics", "crane --version", "Print the installed Crane version."],
  ["Basics", "crane help", "Print the full command reference with every option."],
  ["Basics", "crane init", "Create the repository's signed rules and this machine's signing key (owner), or join them after confirming the owner key (teammate)."],
  ["Basics", "crane checkpoint baseline", "Record the current commit as a trusted checkpoint. Selections and later checks are compared against it."],
  ["Selections", "crane protect FILE start-line N end-line N", "Preserve lines: agents may not change them. Crane inserts anchor comments, signs the selection, and adds 'preserve ID;' to the policy. Options: policy NAME, checkpoint NAME, name LABEL."],
  ["Selections", "crane target FILE start-line N end-line N change_type logical_bn", "Target lines: the task passes only once the agent changes them, optionally by kind: logical_bn (business logic), logical_cn (branches), logical_sn (structure), or semantic (names, comments)."],
  ["Selections", "crane --set default policy NAME", "Use this policy for new selections when none is given."],
  ["Selections", "crane --set default checkpoint NAME", "Use this checkpoint for new selections when none is given."],
  ["Checks", "crane validate --json", "Verify integrity, signatures, policy syntax, markers, zones, and flows. Records selections that moved to another file."],
  ["Checks", "crane test . --json", "Run every preserve and target rule of every policy. A policy passes only if all its rules pass."],
  ["Checks", "crane parse file FILE", "Check the syntax of one policy file."],
  ["Policies", "crane create policy NAME FILENAME", "Create a policy in .crane/policies/FILENAME.crane (appended when the file exists)."],
  ["Policies", "crane add policy NAME marker MARKER", "Move a selection's rule into another policy."],
  ["Policies", "crane policy NAME status", "Run one policy end to end: syntax, every rule, a simulated enforcement self-test, the installed hooks, and whether CI runs Crane."],
  ["Policies", "crane add policy-context file FILE policy NAME", "Attach a text file as guidance that agents receive with the policy."],
  ["Policies", "crane policy-context NAME --view", "Show the context files bound to a policy, with their text."],
  ["Zones and flows", "crane governance sign", "Approve the current .crane/governance.yaml (zones, flows, context packs) with an admin key. Refused while it has blocking findings, such as flows whose stages form a cycle."],
  ["Zones and flows", "crane governance graph --json", "Print zones, flows in stage order, and selections (nested ones under their parent) as an acyclic graph, with the agents recently active in each."],
  ["Repository", "crane repo --https https://github.com/OWNER/REPO", "Connect this clone to its GitHub repository (or use --ssh or --gh-cli). Optional privileges: rf, codeowners, contributors, ci-cd, gh-actions, gh-branches, gh-insights."],
  ["Repository", "crane repo status", "Check the live connection: CONNECTED, NOT_CONNECTED, or FAILURE with the error."],
  ["Repository", "crane repo --view", "Show the privileges granted to Crane and the GitHub grant."],
  ["Repository", "crane repo del", "Delete the repository connection."],
  ["Repository", "crane github", "Sign in with gh and grant Crane read access for this terminal session."],
  ["Repository", "crane github days DAYS", "Grant Crane read access through gh for a number of days."],
  ["Repository", "crane data clear", "Permanently clear this repository's hosted data. CODEOWNERS only."],
  ["Tasks and sessions", "crane task current", "Show the current task of the most recently active agent session (or pass a TASK_ID)."],
  ["Tasks and sessions", "crane task .", "List the tasks of the current session."],
  ["Tasks and sessions", "crane session current", "Show the current agent session (or pass a SESSION_ID)."],
  ["Tasks and sessions", "crane session .", "List every recorded agent session."],
  ["Agents", "crane agent install --profile claude", "Install Crane's hooks for an agent: claude, codex, copilot, or gemini. Every tool call is then decided before it runs."],
  ["Agents", "crane agent uninstall --profile claude", "Remove Crane's hooks for an agent. Refused inside an agent session."],
  ["Agents", "crane agent hooks --profile claude", "Validate an agent's installed hooks; fails when they are missing or wrong."],
  ["Agents", "crane agent link", "Link this machine's upload key to your GitHub account through the dashboard."],
  ["Admins", "crane admin register", "Owner: ask a GitHub admin to confirm the owner key on the dashboard, so teammates can verify it."],
  ["Admins", "crane admin list", "Show the owner, the admins, and this machine's role."],
  ["Admins", "crane admin add PUBLIC_KEY name NAME", "Owner: let another key sign governance changes."],
  ["Admins", "crane admin del PUBLIC_KEY", "Owner: revoke an admin key."],
  ["Integrations", "crane integrate slack", "Set up an integration interactively: jira, whatsapp, slack, or github."],
  ["Alerts", "crane alert add NAME metric METRIC above N window 15m", "Add an alert rule on a metric (above, at_least, below, at_most, equals). Options: channels slack,whatsapp, severity, cooldown, where PATH OPERATOR VALUES, quiet-resolve."],
  ["Alerts", "crane alert list --json", "List the alert rules with their current state."],
  ["Alerts", "crane alert check --json", "Evaluate every rule now and deliver notifications for new firings."],
  ["Alerts", "crane alert test NAME", "Send a test notification for a rule (also remove, enable, or disable NAME)."],
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
const storage = {
  get(key) { try { return JSON.parse(sessionStorage.getItem(key)); } catch { return null; } },
  set(key, value) { try { sessionStorage.setItem(key, JSON.stringify(value)); } catch { /* private mode */ } },
};

// ------------------------------------------------------------------ copying and the toast
let toastTimer;
function toast(text) {
  const node = $("#toast");
  node.textContent = text;
  node.classList.add("show");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => node.classList.remove("show"), 1600);
}
async function copy(text) {
  try {
    await navigator.clipboard.writeText(text);
  } catch {
    const area = make("textarea", { style: "position:fixed;opacity:0", "aria-hidden": "true" });
    area.value = text;
    document.body.append(area);
    area.select();
    document.execCommand("copy");
    area.remove();
  }
  toast("Copied to the clipboard");
}

// ------------------------------------------------------------------ the tooltip
const tooltip = $("#tooltip");
let tipOwner = null;
function showTip(button, title, text) {
  tooltip.replaceChildren(make("b", {}, title), document.createTextNode(text));
  tooltip.hidden = false;
  const box = button.getBoundingClientRect();
  const width = tooltip.offsetWidth, height = tooltip.offsetHeight;
  let left = Math.min(window.innerWidth - width - 16, Math.max(16, box.left + box.width / 2 - width / 2));
  let top = box.top - height - 10;
  if (top < 12) top = box.bottom + 10;
  tooltip.style.left = left + "px";
  tooltip.style.top = top + "px";
  if (tipOwner && tipOwner !== button) tipOwner.setAttribute("aria-expanded", "false");
  tipOwner = button;
  button.setAttribute("aria-expanded", "true");
}
function hideTip() {
  tooltip.hidden = true;
  if (tipOwner) tipOwner.setAttribute("aria-expanded", "false");
  tipOwner = null;
}
document.addEventListener("keydown", (event) => { if (event.key === "Escape") hideTip(); });
window.addEventListener("scroll", hideTip, { passive: true });

// ------------------------------------------------------------------ the command list
const COPY_ICON = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V6a2 2 0 0 1 2-2h9"/></svg>';
const GROUPS = [...new Set(COMMANDS.map((item) => item.group))];
let activeGroup = "All";
function renderCommands() {
  const query = $("#command-search").value.trim().toLowerCase();
  const shown = COMMANDS.filter((item) => (activeGroup === "All" || item.group === activeGroup) && (!query || (item.command + " " + item.help + " " + item.group).toLowerCase().includes(query)));
  const list = $("#command-list");
  list.replaceChildren(...GROUPS.map((group) => {
    const items = shown.filter((item) => item.group === group);
    if (!items.length) return null;
    return make("section", { class: "group", "aria-label": group }, make("h3", {}, group), make("ul", {}, ...items.map((item) => {
      const help = make("button", { class: "help", type: "button", "aria-label": "What " + item.command + " does", "aria-expanded": "false", "aria-describedby": "tooltip" }, "?");
      const show = () => showTip(help, item.command, item.help);
      help.addEventListener("mouseenter", show);
      help.addEventListener("focus", show);
      help.addEventListener("click", () => (tipOwner === help ? hideTip() : show()));
      help.addEventListener("mouseleave", () => { if (document.activeElement !== help) hideTip(); });
      help.addEventListener("blur", hideTip);
      const button = make("button", { class: "copy-command", type: "button", "aria-label": "Copy " + item.command, title: "Copy" });
      button.innerHTML = COPY_ICON;
      button.addEventListener("click", async () => {
        await copy(item.command);
        button.classList.add("done");
        setTimeout(() => button.classList.remove("done"), 1200);
      });
      return make("li", { class: "command glass" }, make("code", {}, item.command), help, button);
    })));
  }));
  $("#command-empty").hidden = shown.length > 0;
}
function renderFilters() {
  const holder = $("#command-filters");
  holder.replaceChildren(...["All", ...GROUPS].map((group) => make("button", {
    type: "button",
    "aria-pressed": String(group === activeGroup),
    onclick: () => { activeGroup = group; renderFilters(); renderCommands(); },
  }, group)));
}
$("#command-search").addEventListener("input", renderCommands);
renderFilters();
renderCommands();

// ------------------------------------------------------------------ install tabs and code copy
for (const tabs of $$("[data-tabs]")) {
  const buttons = $$('[role="tab"]', tabs);
  const select = (button) => {
    for (const other of buttons) {
      const on = other === button;
      other.setAttribute("aria-selected", String(on));
      other.tabIndex = on ? 0 : -1;
      document.getElementById(other.getAttribute("aria-controls")).hidden = !on;
    }
  };
  buttons.forEach((button, index) => {
    button.addEventListener("click", () => select(button));
    button.addEventListener("keydown", (event) => {
      const step = event.key === "ArrowRight" ? 1 : event.key === "ArrowLeft" ? -1 : 0;
      if (!step) return;
      const next = buttons[(index + step + buttons.length) % buttons.length];
      select(next);
      next.focus();
    });
  });
  tabs.selectTab = (id) => { const button = buttons.find((item) => item.id === id); if (button) select(button); };
}
for (const button of $$("[data-copy-pre]")) button.addEventListener("click", () => copy(button.previousElementSibling.textContent));

// ------------------------------------------------------------------ the visitor's system
function platform() {
  const agent = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || navigator.userAgent || "";
  if (/win/i.test(agent)) return "windows";
  if (/mac|iphone|ipad/i.test(agent)) return "macos";
  if (/linux|x11/i.test(agent)) return "linux";
  return "";
}
const system = platform();
if (system) {
  const tabs = $("[data-tabs]");
  if (tabs && tabs.selectTab) tabs.selectTab("tab-" + system);
  const target = { windows: "x86_64-pc-windows-msvc", linux: "x86_64-unknown-linux-gnu", macos: "aarch64-apple-darwin" }[system];
  const card = $('.download[data-target="' + target + '"]');
  if (card) card.classList.add("yours");
}

// ------------------------------------------------------------------ the latest release
const templates = $$("[data-template]").map((node) => [node, node.textContent]);
function fillTemplates(tag) { for (const [node, text] of templates) node.textContent = text.split("{tag}").join(tag); }
fillTemplates("vX.Y.Z");

function formatSize(bytes) { return bytes >= 1048576 ? (bytes / 1048576).toFixed(1) + " MB" : Math.max(1, Math.round(bytes / 1024)) + " KB"; }
function showRelease(release) {
  const tag = release.tag_name;
  const date = release.published_at ? new Date(release.published_at).toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" }) : "";
  $("#release-meta").textContent = "Crane " + tag + (date ? " · published " + date : "") + " · binaries for Linux, Windows, and macOS.";
  $("#hero-version").textContent = "Crane " + tag + " · open source";
  $("#hero-download").textContent = "Download " + tag;
  $("#all-assets").href = release.html_url || RELEASES + "/latest";
  fillTemplates(tag);
  const assets = release.assets || [];
  for (const card of $$(".download")) {
    const target = card.dataset.target;
    const archive = assets.find((asset) => asset.name.includes(target) && !asset.name.endsWith(".sha256"));
    const checksum = assets.find((asset) => asset.name.includes(target) && asset.name.endsWith(".sha256"));
    const button = $(".button", card);
    if (archive) {
      $(".file", card).textContent = archive.name + " · " + formatSize(archive.size);
      button.href = archive.browser_download_url;
      button.setAttribute("download", "");
      card.classList.remove("missing");
    } else {
      $(".file", card).textContent = "Not in this release";
      button.href = release.html_url || RELEASES + "/latest";
      card.classList.add("missing");
    }
    const sum = $(".checksum", card);
    if (checksum) sum.href = checksum.browser_download_url; else sum.remove();
  }
}
async function loadRelease() {
  const cached = storage.get(CACHE_KEY);
  if (cached && Date.now() - cached.at < CACHE_MS) { showRelease(cached.release); return; }
  try {
    const response = await fetch("https://api.github.com/repos/" + REPO + "/releases/latest", { headers: { Accept: "application/vnd.github+json" } });
    if (!response.ok) throw new Error("GitHub answered " + response.status);
    const release = await response.json();
    storage.set(CACHE_KEY, { at: Date.now(), release });
    showRelease(release);
  } catch (error) {
    if (cached) { showRelease(cached.release); return; }
    $("#release-meta").textContent = "Could not reach GitHub just now. The buttons open the latest release page instead.";
  }
}
loadRelease();

// ------------------------------------------------------------------ the navigation
const menu = $("#menu");
const links = $("#links");
const setMenu = (open) => { links.classList.toggle("open", open); menu.setAttribute("aria-expanded", String(open)); menu.setAttribute("aria-label", open ? "Close the menu" : "Open the menu"); };
menu.addEventListener("click", () => setMenu(!links.classList.contains("open")));
links.addEventListener("click", (event) => { if (event.target.closest("a")) setMenu(false); });
document.addEventListener("keydown", (event) => { if (event.key === "Escape" && links.classList.contains("open")) { setMenu(false); menu.focus(); } });
$("#year").textContent = String(new Date().getFullYear());
