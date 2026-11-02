"use strict";
// DataSea: the landing page and the organization's sections. The rail lists the sections
// (Engineering, Product, Sales, Marketing, Finance, Human Resources, Operations, Legal); a section
// lists its subsections, and Engineering holds Flows (flows.js). The landing page is shown signed
// out too; Flows needs a GitHub sign-in and a repository, chosen in the top bar. Every time is
// written as YYYY:MM:DD:HH:MM:SS:MMM. The only writes are the change requests made in Flows, which
// GitHub authorizes and a person applies on a machine, never here.

// ---------------------------------------------------------------- DOM and formatting

/** Create an element with attributes, listeners (on…), and children */
function h(tag, attributes, ...children) {
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
}
/** Create an SVG element */
function svg(tag, attributes, ...children) {
  const node = document.createElementNS("http://www.w3.org/2000/svg", tag);
  for (const [key, value] of Object.entries(attributes || {})) {
    if (key === "style") Object.assign(node.style, value);
    else if (key.startsWith("on") && typeof value === "function") node.addEventListener(key.slice(2), value);
    else if (value !== undefined && value !== null) node.setAttribute(key, value);
  }
  for (const child of children.flat(Infinity)) if (child !== null && child !== undefined && child !== false) node.append(child instanceof Node ? child : document.createTextNode(String(child)));
  return node;
}
/** Replace a node's children, skipping empty ones */
function put(node, ...children) {
  node.replaceChildren(...children.flat(Infinity).filter((child) => child !== null && child !== undefined && child !== false));
  return node;
}
const pad = (number, width) => String(number).padStart(width || 2, "0");
/** A time as YYYY:MM:DD:HH:MM:SS:MMM (local) */
function dt(millis) {
  if (millis === null || millis === undefined || millis === "") return "—";
  const value = typeof millis === "string" && !/^-?\d+(?:\.\d+)?$/.test(millis) ? Date.parse(millis) : Number(millis);
  if (!Number.isFinite(value)) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return pad(date.getFullYear(), 4) + ":" + pad(date.getMonth() + 1) + ":" + pad(date.getDate()) + ":" + pad(date.getHours()) + ":" + pad(date.getMinutes()) + ":" + pad(date.getSeconds()) + ":" + pad(date.getMilliseconds(), 3);
}
window.Foxx = Object.assign(window.Foxx || {}, { h, svg, put, dt });

/** The tone of a state shown as a chip */
const TONES = {
  ACTIVE: "ok", APPLIED: "ok", OK: "ok", SIGNED: "ok", COMPLETED: "ok", RESOLVED: "ok", verified: "ok",
  INFO: "info", QUEUED: "info", ENDED: "info",
  PENDING: "warn", PENDING_APPROVAL: "warn", DRAFT: "warn", STAGED: "warn", STALE: "warn", UNKNOWN: "warn", proposed: "warn", DISMISSED: "",
  FAILED: "bad", REFUSED: "bad", REJECTED: "bad", ERROR: "bad", TERMINATED: "bad", CANCELLED: "", REVOKED: "bad",
  QUARANTINED: "crit", CRITICAL: "crit",
};
function chip(value, title) { return value === null || value === undefined || value === "" ? h("span", { class: "muted" }, "—") : h("span", { class: "chip " + (TONES[value] || ""), title: title || undefined }, String(value)); }
function member(value) { return value ? h("span", { class: "member", title: value.key }, value.actor + " ", h("code", { class: "muted" }, value.short)) : h("span", { class: "muted" }, "—"); }

// ---------------------------------------------------------------- sections

const ICON = (d) => svg("svg", { viewBox: "0 0 24 24", fill: "none", stroke: "currentColor", "stroke-width": "1.8", "stroke-linecap": "round", "stroke-linejoin": "round", "aria-hidden": "true" }, svg("path", { d }));
const PATHS = {
  home: "M3 11l9-7 9 7M5 10v10h5v-6h4v6h5V10",
  engineering: "M8 9l-4 3 4 3M16 9l4 3-4 3M13.5 5l-3 14",
  flows: "M5 6a2 2 0 1 0 0-.01M19 18a2 2 0 1 0 0-.01M12 12a2 2 0 1 0 0-.01M7 6h3l2 4M14 13l2 3h1",
  product: "M12 3l8 4.5v9L12 21l-8-4.5v-9zM4 7.5l8 4.5 8-4.5M12 12v9",
  sales: "M4 19V9M10 19V5M16 19v-7M22 19H2",
  marketing: "M3 10v4h3l6 4V6l-6 4zM16 9a4 4 0 0 1 0 6M19 6a8 8 0 0 1 0 12",
  finance: "M12 3v18M17 7H9.5a3 3 0 0 0 0 6h5a3 3 0 0 1 0 6H6",
  people: "M9 11a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7zM2.5 20c.8-3.5 3.3-5.5 6.5-5.5s5.7 2 6.5 5.5M16 4.5a3.5 3.5 0 0 1 0 6.5M18 14.8c1.8.7 3 2.5 3.5 5.2",
  operations: "M12 8a4 4 0 1 1 0 8 4 4 0 0 1 0-8zM12 2v3M12 19v3M2 12h3M19 12h3M5 5l2 2M17 17l2 2M5 19l2-2M17 7l2-2",
  legal: "M12 3v18M7 21h10M5 7h14M5 7l-3 7a3 3 0 0 0 6 0zM19 7l-3 7a3 3 0 0 0 6 0z",
  empty: "M4 7h16v12H4zM4 7l2-3h12l2 3M9 12h6",
};
/** The organization's sections: id, name, icon, what it holds, and its subsections (id, name,
 * icon, what it shows, the page that draws it) */
const SECTIONS = [
  { id: "engineering", name: "Engineering", icon: "engineering", text: "How the software works and who keeps it running: the repository's flows, the zones they cross, their context, tests, people, and the subagents that watch them.",
    subs: [{ id: "flows", name: "Flows", icon: "flows", text: "Every flow and zone of a repository as one map, each flow's traced call graph, context, tests, people, and subagents. Every change is a request a person applies on a machine." }] },
  { id: "product", name: "Product", icon: "product", text: "What is being built and why: goals, roadmaps, and the decisions behind them.", subs: [] },
  { id: "sales", name: "Sales", icon: "sales", text: "Who is buying and how: accounts, pipeline, and the conversations that move deals.", subs: [] },
  { id: "marketing", name: "Marketing", icon: "marketing", text: "How the organization is heard: campaigns, launches, and the channels they run on.", subs: [] },
  { id: "finance", name: "Finance", icon: "finance", text: "Where the money goes: budgets, spend, and the approvals around them.", subs: [] },
  { id: "people", name: "Human Resources", icon: "people", text: "The people who do the work: hiring, onboarding, and the policies they follow.", subs: [] },
  { id: "operations", name: "Operations", icon: "operations", text: "How the organization runs day to day: processes, vendors, and incidents.", subs: [] },
  { id: "legal", name: "Legal", icon: "legal", text: "What the organization agrees to: contracts, policies, and compliance.", subs: [] },
];
const sectionOf = (id) => SECTIONS.find((section) => section.id === id);

// ---------------------------------------------------------------- state

const storage = {
  load(key, into, area) { try { Object.assign(into, JSON.parse((area || sessionStorage).getItem(key) || "{}")); } catch (error) { /* storage unavailable */ } },
  save(key, value, area) { try { (area || sessionStorage).setItem(key, JSON.stringify(value)); } catch (error) { /* storage unavailable */ } },
  read(key, fallback, area) { try { const text = (area || localStorage).getItem(key); return text ? JSON.parse(text) : fallback; } catch (error) { return fallback; } },
};
const scope = { repository: "" };
storage.load("foxx-scope", scope);
window.FOXX_REPOSITORY_ID = () => scope.repository;
let me = null;
let approval = null;
{
  const params = new URLSearchParams(location.search);
  if (params.has("approval")) {
    approval = { ok: params.get("ok") === "1", message: params.get("approval") };
    history.replaceState(null, "", location.pathname + location.hash);
  }
}

// ---------------------------------------------------------------- the API

class SignedOut extends Error {}
function scoped(path) {
  const url = new URL(path, location.origin);
  if (!url.pathname.startsWith("/api/me")) url.searchParams.set("repository", scope.repository);
  return url;
}
async function answer(response) {
  const body = await response.json().catch(() => ({ error: "invalid response" }));
  if (response.status === 401) throw new SignedOut(body.error);
  if (!response.ok) { const error = new Error(body.error || response.statusText); error.status = response.status; throw error; }
  return body;
}
async function api(path, params) {
  const url = scoped(path);
  for (const [key, value] of Object.entries(params || {})) if (value !== undefined && value !== null && value !== "") url.searchParams.set(key, value);
  return answer(await fetch(url, { credentials: "same-origin" }));
}
async function apiWrite(path, payload) {
  return answer(await fetch(scoped(path), { method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json" }, body: JSON.stringify(payload) }));
}
/** The viewer, or null when signed out; a service without sign-in answers with its reason */
async function viewer() {
  try { return { me: await api("/api/me") }; }
  catch (error) {
    if (error instanceof SignedOut) return { me: null };
    return { me: null, problem: error.message };
  }
}

// ---------------------------------------------------------------- shared views used by Flows

function needRepository(main) {
  if (scope.repository) return false;
  main.append(h("div", { class: "notice" }, me && me.repositories.length
    ? "Choose a repository in the top bar."
    : "No repository of your account or its organizations has reported yet. Once a teammate's Crane uploads from a GitHub clone, it appears here."));
  return true;
}
function repositoryName() { const repository = me && me.repositories.find((item) => item.id === scope.repository); return repository ? repository.remote.replace(/^github\.com\//, "") : "repository"; }

// ---------------------------------------------------------------- the frame

function currentRoute() {
  const [section, sub] = location.hash.replace(/^#\/?/, "").split("?")[0].split("/");
  return { section: section || "", sub: sub || "" };
}
function drawNav(route) {
  const link = (href, icon, label, active, extra) => h("a", { href, class: (active ? "active" : "") + (extra ? " " + extra : ""), "aria-current": active ? "page" : null }, ICON(PATHS[icon]), h("span", {}, label));
  put(document.getElementById("nav"),
    link("#/", "home", "Home", !route.section),
    h("div", { class: "nav-head" }, "Sections"),
    SECTIONS.map((section) => [
      link("#/" + section.id, section.icon, section.name, route.section === section.id && !route.sub),
      section.subs.map((sub) => link("#/" + section.id + "/" + sub.id, sub.icon, sub.name, route.section === section.id && route.sub === sub.id, "sub")),
    ]));
}
function drawCrumbs(route) {
  const section = sectionOf(route.section);
  const sub = section && section.subs.find((item) => item.id === route.sub);
  put(document.getElementById("crumbs"),
    section ? [sub ? h("a", { href: "#/" + section.id }, section.name) : h("b", {}, section.name), sub ? [h("span", { class: "sep" }, "›"), h("b", {}, sub.name)] : null] : h("b", {}, "DataSea"));
}
function drawViewer(problem) {
  put(document.getElementById("viewer"), me
    ? [h("div", { class: "who" }, h("span", { class: "avatar", "aria-hidden": "true" }, (me.login || "?").slice(0, 1).toUpperCase()), h("span", {}, "Signed in as ", h("b", {}, me.login))), h("a", { href: "/auth/logout" }, "Sign out")]
    : problem ? h("span", {}, "Sign-in is not available") : h("a", { href: "/auth/login" }, "Sign in with GitHub"));
}
/** The repository switcher, shown where a page reads a repository (Flows) */
function drawScope(show) {
  const holder = document.getElementById("scope");
  if (!show || !me) { holder.replaceChildren(); return; }
  if (!me.repositories.some((repository) => repository.id === scope.repository)) scope.repository = me.repositories.length ? me.repositories[0].id : "";
  storage.save("foxx-scope", scope);
  if (!me.repositories.length) { holder.replaceChildren(); return; }
  const select = h("select", { "aria-label": "Repository", onchange: (event) => { scope.repository = event.target.value; storage.save("foxx-scope", scope); render(); } },
    me.accounts.map((account) => {
      const owned = me.repositories.filter((repository) => repository.account === account.name);
      return owned.length ? h("optgroup", { label: account.name }, owned.map((repository) => h("option", { value: repository.id, selected: repository.id === scope.repository }, repository.remote.replace(/^github\.com\/[^/]+\//, "")))) : null;
    }));
  put(holder, select);
}
function signInPanel(message, problem) {
  return h("div", { class: "signin" },
    h("img", { src: "/logo.png?v=2", alt: "DataSea", width: "96", height: "96" }),
    h("h2", {}, "Sign in to continue"),
    h("p", { class: "muted" }, message),
    problem ? h("p", { class: "notice error" }, problem) : h("a", { class: "btn primary large", href: "/auth/login" }, "Sign in with GitHub"));
}

// ---------------------------------------------------------------- pages

function heroArt() {
  return svg("svg", { viewBox: "0 0 360 240", role: "img", "aria-label": "Sections joined like currents" },
    svg("path", { class: "wave c", d: "M10 190 C 80 150, 140 230, 210 185 S 320 150, 350 170" }),
    svg("path", { class: "wave b", d: "M10 140 C 70 100, 150 170, 220 130 S 310 90, 350 115" }),
    svg("path", { class: "wave a", d: "M10 85 C 80 40, 140 120, 210 75 S 310 40, 350 60" }),
    [[60, 70], [150, 92], [230, 72], [100, 128], [250, 128], [180, 186]].map(([cx, cy]) => svg("circle", { class: "node-dot", cx, cy, r: 7 })));
}
function sectionCard(section) {
  const active = section.subs.length > 0;
  return h("a", { class: "card" + (active ? "" : " quiet"), href: "#/" + section.id },
    h("span", { class: "icon" }, ICON(PATHS[section.icon])),
    h("h3", {}, section.name),
    h("p", {}, section.text),
    h("div", { class: "subs" }, active ? section.subs.map((sub) => chip(sub.name)) : h("span", { class: "chip" }, "No subsections yet")));
}
function landing(main, problem) {
  main.append(
    h("section", { class: "hero" },
      h("div", {},
        h("div", { class: "eyebrow" }, "DataSea"),
        h("h1", {}, "Your organization's knowledge, section by section."),
        h("p", {}, "DataSea gathers what each part of the organization knows in one place. Engineering starts with Flows: every flow of a repository, what it calls, and who watches it, as reported by your teammates' Crane. Every change is a request a person applies on a machine, where Crane checks and signs it."),
        h("div", { class: "actions" },
          me ? h("a", { class: "btn primary large", href: "#/engineering/flows" }, "Open Flows")
            : problem ? null : h("a", { class: "btn primary large", href: "/auth/login" }, "Sign in with GitHub"),
          h("a", { class: "btn large", href: "#/engineering" }, "Explore Engineering")),
        problem ? h("p", { class: "notice warn", style: { marginTop: "16px", marginBottom: 0 } }, problem) : null),
      h("div", { class: "hero-art" }, heroArt())),
    h("div", { class: "section-title" }, h("h2", {}, "Sections"), h("span", {}, SECTIONS.length + " sections, " + SECTIONS.reduce((total, section) => total + section.subs.length, 0) + " with content so far")),
    h("div", { class: "cards" }, SECTIONS.map(sectionCard)),
    h("div", { class: "section-title" }, h("h2", {}, "How it works")),
    h("div", { class: "steps" },
      h("div", { class: "step" }, h("h3", {}, "Machines report"), h("p", {}, "Each teammate's Crane uploads the repository's status with its own key. The repository appears once an upload proves its GitHub remote.")),
      h("div", { class: "step" }, h("h3", {}, "People review here"), h("p", {}, "Sign in with GitHub to see the repositories of your account and its organizations, and the sections they feed.")),
      h("div", { class: "step" }, h("h3", {}, "Changes happen on a machine"), h("p", {}, "A change asked for here waits for a person to apply it with one click in Landscape or 'crane brick inbox apply'. DataSea holds no signing key."))));
}
function sectionPage(main, section) {
  main.append(h("div", { class: "page-head" },
    h("span", { class: "icon" }, ICON(PATHS[section.icon])),
    h("div", {}, h("h1", {}, section.name), h("p", {}, section.text))));
  if (!section.subs.length) {
    main.append(h("div", { class: "empty" }, ICON(PATHS.empty), h("h3", {}, "No subsections yet"), h("p", {}, section.name + " has no subsections yet. They appear here as the organization's knowledge for " + section.name.toLowerCase() + " comes to DataSea.")));
    return;
  }
  main.append(h("div", { class: "cards" }, section.subs.map((sub) => h("a", { class: "card", href: "#/" + section.id + "/" + sub.id },
    h("span", { class: "icon" }, ICON(PATHS[sub.icon])), h("h3", {}, sub.name), h("p", {}, sub.text)))));
}

// ---------------------------------------------------------------- render

async function render() {
  const route = currentRoute();
  const section = sectionOf(route.section);
  const sub = section && section.subs.find((item) => item.id === route.sub);
  if ((route.section && !section) || (route.sub && !sub)) { location.replace("#/"); return; }
  drawNav(route);
  drawCrumbs(route);
  document.getElementById("app").classList.remove("open");
  const main = document.getElementById("main");
  put(main, h("p", { class: "skeleton" }, "Loading…"));
  const found = await viewer();
  me = found.me;
  drawViewer(found.problem);
  drawScope(Boolean(sub));
  put(main);
  if (approval) { main.append(h("div", { class: "notice" + (approval.ok ? " ok" : " error"), role: "status" }, approval.message)); approval = null; }
  document.title = (sub ? sub.name + " · " : section ? section.name + " · " : "") + "DataSea";
  try {
    if (!section) landing(main, found.problem);
    else if (!sub) sectionPage(main, section);
    else if (!me) main.append(signInPanel(sub.name + " reads your repositories, so it needs a GitHub sign-in. Personal accounts see their own repositories; organization members see the organization's.", found.problem));
    else await (window.FOXX_PAGES || {})[sub.id](main);
  } catch (error) {
    if (error instanceof SignedOut) put(main, signInPanel("Your session ended. Sign in again to continue."));
    else put(main, h("div", { class: "notice error" }, "Could not load: " + error.message));
  }
}

/** Apply the chosen theme (system, light, or dark) */
function applyTheme() {
  const theme = storage.read("foxx-theme", "system");
  if (theme === "system") delete document.documentElement.dataset.theme; else document.documentElement.dataset.theme = theme;
  const button = document.getElementById("theme");
  button.textContent = theme === "dark" ? "☾" : theme === "light" ? "☀" : "◐";
  button.title = "Theme: " + theme + " (click to change)";
}
applyTheme();
document.getElementById("theme").addEventListener("click", () => {
  const order = ["system", "light", "dark"];
  storage.save("foxx-theme", order[(order.indexOf(storage.read("foxx-theme", "system")) + 1) % order.length], localStorage);
  applyTheme();
});
document.getElementById("toggle").addEventListener("click", () => document.getElementById("app").classList.toggle("open"));
window.addEventListener("hashchange", render);
// flows.js loads after this script; start once every page is registered
document.addEventListener("DOMContentLoaded", render);
