// The operator page: shows the agent-command switch and changes it. Every change goes to
// POST api/agent-commands with the X-Crane-Operator header; the service checks the operator's
// GitHub login on every request. Addresses are relative, so the page works at / (its own service)
// and at /operator/ (inside DataSea).
"use strict";

const element = (id) => document.getElementById(id);
const show = (id, visible) => element(id).classList.toggle("hidden", !visible);
let current = null;

function when(millis) {
  return millis ? new Date(millis).toLocaleString() : "never";
}

function render(data) {
  current = data;
  show("signed-out", false);
  show("unavailable", false);
  element("operator").classList.remove("hidden");
  element("who").textContent = "@" + data.login + " · ";
  const out = document.createElement("a");
  out.href = "auth/logout";
  out.textContent = "Sign out";
  out.style.color = "inherit";
  element("who").appendChild(out);
  const on = data.agent_commands.enabled;
  const state = element("state");
  state.textContent = on ? "ON" : "OFF";
  state.className = "state " + (on ? "on" : "off");
  const toggle = element("toggle");
  toggle.textContent = on ? "Turn off" : "Turn on";
  toggle.disabled = false;
  element("lifetime").textContent = data.lifetime_minutes;
  element("changed").textContent = data.agent_commands.changed_by
    ? "Last changed by @" + data.agent_commands.changed_by + " on " + when(data.agent_commands.changed_at) + "."
    : "Never changed: it has been off since the start.";
  element("key").textContent = data.public_key;
  const commands = element("commands");
  commands.replaceChildren(...data.commands.map((command) => {
    const chip = document.createElement("span");
    chip.className = "chip mono";
    chip.textContent = command;
    return chip;
  }));
  const audit = element("audit");
  if (!data.audit.length) return;
  audit.replaceChildren(...data.audit.map((entry) => {
    const row = document.createElement("tr");
    for (const text of [when(entry.at), "@" + entry.login, (entry.from ? "on" : "off") + " → " + (entry.to ? "on" : "off"), entry.reason]) {
      const cell = document.createElement("td");
      cell.textContent = text;
      row.appendChild(cell);
    }
    return row;
  }));
}

async function load() {
  const params = new URLSearchParams(location.search);
  if (params.has("refused")) {
    show("refused", true);
    history.replaceState(null, "", location.pathname);
  }
  let response;
  try {
    response = await fetch("api/state", { credentials: "same-origin" });
  } catch (error) {
    show("unavailable", true);
    element("unavailable-text").textContent = "The operator dashboard could not be reached.";
    return;
  }
  if (response.status === 401 || response.status === 403) {
    element("operator").classList.add("hidden");
    show("signed-out", true);
    // Signed in, but not as an operator (inside DataSea a DataSea session exists)
    if (response.status === 403) show("refused", true);
    return;
  }
  const data = await response.json().catch(() => ({}));
  if (!response.ok) {
    show("unavailable", true);
    element("unavailable-text").textContent = data.error || "The operator dashboard answered " + response.status + ".";
    return;
  }
  render(data);
}

async function change() {
  if (!current) return;
  const enabled = !current.agent_commands.enabled;
  const reason = element("reason").value.trim();
  const message = element("message");
  if (!reason) {
    message.textContent = "Give a reason first.";
    element("reason").focus();
    return;
  }
  if (!confirm((enabled ? "Allow" : "Stop") + " AI agents running read-only Crane commands on every machine?")) return;
  element("toggle").disabled = true;
  message.textContent = "Saving…";
  const response = await fetch("api/agent-commands", {
    method: "POST",
    credentials: "same-origin",
    headers: { "Content-Type": "application/json", "X-Crane-Operator": "1" },
    body: JSON.stringify({ enabled, reason }),
  }).catch(() => null);
  const data = response ? await response.json().catch(() => ({})) : {};
  if (!response || !response.ok) {
    message.textContent = (data && data.error) || "The change was not saved.";
    element("toggle").disabled = false;
    return;
  }
  element("reason").value = "";
  message.textContent = "Saved. Machines follow at their next sync.";
  render(data);
}

element("sign-in").addEventListener("click", () => { location.href = "auth/login"; });
element("toggle").addEventListener("click", change);
load();
