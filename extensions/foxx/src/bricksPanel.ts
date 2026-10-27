// The Bricks panel: the repository's Zones and Flows as a graph with the Bricks attached to each,
// and one place to create a Brick, attach it to a Zone or Flow, and change its two contexts: the
// Common Context every attachment shares (a change starts the Brick's next version) and the
// volatile context of one attachment (Crane's Current Context; it changes in place). Crane is the
// authority: every signing step is previewed with '--dry-run' and signed only after the person
// confirms, and whatever Crane refuses is shown in plain words with its own reasons.

import { AttachmentRow, BRICK_TEMPLATES, BRICK_TYPES, BrickList, BrickRow, draftFile, SnapshotData, snapshotMarkdown, succeeded, TypeInfo } from "./core/bricks";
import { friendlyError, OverlayItem, overlayGraph, problemLines } from "./core/workbench";
import { askReason, call, CANCELLED, confirm, OpResult, PanelDeps, refused, scratch, showMarkdown, signed, WorkbenchHost, WorkbenchPage } from "./workbench";

/** The panel's view type. */
export const BRICKS_PANEL = "foxx.bricksPanel";

/**
 * Read a text value the page sent.
 * @param value value
 * @returns text ("" when absent)
 */
const text = (value: unknown): string => (typeof value === "string" ? value : "");

/**
 * The extension side of the Bricks panel.
 * @param deps the rest of Landscape
 * @returns host
 */
export function bricksHost(deps: PanelDeps): WorkbenchHost {
  let types: TypeInfo[] = [];
  let list: BrickList | undefined;

  /** The newest Brick list, or why there is none. */
  async function load(): Promise<string | undefined> {
    const listed = await call<BrickList>(deps, ["brick", "list"]);
    list = listed.envelope?.data;
    if (list && !types.length) types = (await call<{ types: TypeInfo[] }>(deps, ["brick", "types"])).envelope?.data.types ?? [];
    return listed.error;
  }

  /**
   * The Brick an attachment pins.
   * @param row attachment
   * @returns row
   */
  const brickOf = (row: AttachmentRow): BrickRow | undefined => list?.bricks.find((brick) => `brick:${brick.ref}` === row.subject || brick.ref === row.subject);

  return {
    async state(refresh) {
      const problem = await load();
      const graph = await deps.graph(refresh);
      const items: OverlayItem[] = (list?.attachments ?? []).map((row) => ({
        id: row.attachment_id,
        label: brickOf(row)?.name ?? row.subject.replace(/^(brick|stack):/, ""),
        sub: `${row.subject.replace(/^brick:/, "")} · ${row.issues.length ? "fails closed" : row.state}`,
        target: row.target,
        tone: row.issues.length ? "bad" : row.state === "enabled" ? "ok" : "off",
      }));
      return {
        problem,
        graphError: graph.error,
        graph: overlayGraph(graph.data, items),
        targets: deps.targets().map((target) => ({ key: `${target.kind}:${target.id}`, ...target })),
        bricks: list?.bricks ?? [],
        stacks: list?.stacks ?? [],
        attachments: list?.attachments ?? [],
        store: list?.store,
        types: types.length ? types : BRICK_TYPES.map((type) => ({ type, label: type, purpose: "", support: "declared", runtime: "", fields: [] })),
        templates: BRICK_TEMPLATES,
      };
    },

    async run(op, values, progress) {
      const result = await operate(op, values, progress);
      if (result.ok && !op.startsWith("load:")) await deps.refresh();
      return result;
    },
  };

  /**
   * Run one operation of the page.
   * @param op operation
   * @param values what the person entered
   * @param progress step reporter
   * @returns result
   */
  async function operate(op: string, values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    if (op === "create") return create(values, progress);
    if (op === "attach") {
      const subject = text(values.subject), target = text(values.target);
      if (!subject || !target) return { ok: false, message: "Choose a Brick and a Zone or Flow first.", fields: { ...(subject ? {} : { subject: "Choose a Brick." }), ...(target ? {} : { target: "Choose a Zone or Flow." }) } };
      return attach(subject, target, text(values.volatile), progress);
    }
    if (op === "load:common") {
      const shown = await call<{ definition: { common_context?: { text?: string } } }>(deps, ["brick", "show", text(values.ref)]);
      if (!shown.envelope) return { ok: false, message: shown.error ?? "Crane printed no result." };
      return { ok: true, message: "", data: shown.envelope.data.definition?.common_context?.text ?? "" };
    }
    if (op === "common") return commonContext(values, progress);
    if (op === "volatile") {
      const attachment = text(values.attachment);
      if (!attachment) return { ok: false, message: "Choose the attachment whose volatile context you want to change.", fields: { attachment: "Choose an attachment." } };
      const file = scratch("current-context.md", text(values.text));
      try {
        return await signed(deps, ["brick", "context", "set", attachment, "--file", file.file], "Update the volatile context", progress);
      } finally {
        file.dispose();
      }
    }
    if (op === "detach") {
      const row = list?.attachments.find((item) => item.attachment_id === text(values.attachment));
      if (!row) return { ok: false, message: "That attachment is gone; refresh the panel." };
      const reason = await askReason(`Why detach ${row.subject} from ${row.target}?`);
      if (!reason) return CANCELLED;
      return signed(deps, ["brick", "detach", row.attachment_id, "--reason", reason], `Detach ${row.subject} from ${row.target}`, progress);
    }
    if (op === "activate") {
      const id = text(values.id);
      const reason = await askReason(`Why activate ${id}? Activation is signed with an admin key.`);
      if (!reason) return CANCELLED;
      return signed(deps, ["brick", "activate", id, "--reason", reason], `Activate ${id}`, progress);
    }
    if (op === "validate") {
      progress("Validating");
      const result = await call<{ validation?: { problems?: unknown } }>(deps, ["brick", "validate", text(values.id)]);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      const problems = problemLines(result.envelope.data?.validation?.problems);
      return succeeded(result.envelope.status) && !problems.length ? { ok: true, message: result.envelope.summary } : { ok: false, message: friendlyError(result.envelope.summary), problems };
    }
    if (op === "preview") {
      const result = await call<SnapshotData>(deps, ["brick", "context", "preview", "--targets", text(values.target)]);
      if (!result.envelope) return { ok: false, message: result.error ?? "Crane printed no result." };
      await showMarkdown(snapshotMarkdown(result.envelope.data));
      return { ok: true, message: `Opened what agents in ${text(values.target)} receive.` };
    }
    return { ok: false, message: `Landscape does not know the operation '${op}'.` };
  }

  /**
   * Attach an activated Brick to a target with an optional volatile context.
   * @param subject "ID@N" or "stack:ID@N"
   * @param target "zone:ID" or "flow:ID"
   * @param volatile the attachment's volatile context
   * @param progress step reporter
   * @returns result
   */
  async function attach(subject: string, target: string, volatile: string, progress: (text: string) => void): Promise<OpResult> {
    const args = ["brick", "attach", subject.replace(/^brick:/, ""), "--target", target];
    const file = volatile.trim() ? scratch("current-context.md", volatile) : undefined;
    if (file) args.push("--context-file", file.file);
    try {
      return await signed(deps, args, `Attach ${subject} to ${target}`, progress);
    } finally {
      file?.dispose();
    }
  }

  /**
   * Create a draft, and when asked, activate it and attach it.
   * @param values the form
   * @param progress step reporter
   * @returns result
   */
  async function create(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const type = text(values.type) as BrickRow["type"];
    let config: Record<string, unknown>;
    try {
      config = JSON.parse(text(values.config) || "{}") as Record<string, unknown>;
    } catch (error) {
      return { ok: false, message: "The configuration is not valid JSON. Check brackets, quotes, and commas.", fields: { config: (error as Error).message } };
    }
    progress("Saving the draft");
    const file = scratch("draft.json", draftFile({ type, name: text(values.name), intent: text(values.intent), context: text(values.common), config }));
    let created;
    try {
      created = await call<{ validation?: { problems?: unknown } }>(deps, ["brick", "create", "--file", file.file]);
    } finally {
      file.dispose();
    }
    if (!created.envelope) return { ok: false, message: created.error ?? "Crane printed no result." };
    if (!succeeded(created.envelope.status)) return refused(created.envelope, created.envelope.data?.validation?.problems);
    const problems = problemLines(created.envelope.data?.validation?.problems);
    const reference = created.envelope.refs.brick_ref ?? "";
    if (problems.length) return { ok: false, message: `Saved the draft ${reference}, but it does not validate yet. Fix these in the Library (Edit), then activate it.`, problems };
    if (!values.now) return { ok: true, message: `Created the draft ${reference}. Activate it from the Library to attach it.` };
    const id = reference.split("@")[0];
    const activated = await signed(deps, ["brick", "activate", id, "--reason", text(values.reason)], `Activate ${reference}`, progress);
    if (!activated.ok) return { ...activated, message: `Created the draft ${reference}; activating it did not finish. ${activated.message}` };
    const target = text(values.target);
    if (!target) return { ok: true, message: `Created and activated ${reference}.` };
    const attached = await attach(reference, target, text(values.volatile), progress);
    return attached.ok ? { ok: true, message: `Created ${reference}, activated it, and attached it to ${target}.` } : { ...attached, message: `Created and activated ${reference}; attaching it to ${target} did not finish. ${attached.message}` };
  }

  /**
   * Save a Brick's Common Context. On an activated Brick it starts the next version's draft; when
   * asked, that version is activated and the Brick's attachments are moved to it.
   * @param values the form
   * @param progress step reporter
   * @returns result
   */
  async function commonContext(values: Record<string, unknown>, progress: (text: string) => void): Promise<OpResult> {
    const brick = list?.bricks.find((row) => row.ref === text(values.ref) || (row.state === "draft" && row.brick_id === text(values.ref)));
    if (!brick) return { ok: false, message: "That Brick is gone; refresh the panel.", fields: { brick: "Choose a Brick." } };
    progress("Saving the common context");
    const file = scratch("context.md", text(values.text));
    let edited;
    try {
      edited = await call(deps, ["brick", "edit", brick.brick_id, "--context-file", file.file]);
    } finally {
      file.dispose();
    }
    if (!edited.envelope) return { ok: false, message: edited.error ?? "Crane printed no result." };
    if (!succeeded(edited.envelope.status)) return refused(edited.envelope);
    const reference = edited.envelope.refs.brick_ref ?? brick.brick_id;
    if (brick.state === "draft") return { ok: true, message: `Saved the common context of the draft ${reference}.` };
    if (!values.promote) return { ok: true, message: `Saved as ${reference}, a new draft version: ${brick.ref} stays in force until ${reference} is activated and its attachments are moved to it.` };
    const activated = await signed(deps, ["brick", "activate", brick.brick_id, "--reason", text(values.reason)], `Activate ${reference}`, progress);
    if (!activated.ok) return { ...activated, message: `Saved ${reference} as a draft; activating it did not finish. ${activated.message}` };
    const moving = (list?.attachments ?? []).filter((row) => row.subject.replace(/^brick:/, "").split("@")[0] === brick.brick_id && !row.subject.endsWith(`@${reference.split("@")[1]}`));
    if (!moving.length) return { ok: true, message: `Activated ${reference}; nothing was attached to move.` };
    if (!(await confirm(`move ${moving.length} attachment(s) to ${reference}?`, moving.map((row) => `${row.target}: ${row.subject} → ${reference}`).join("\n") + "\n\nEach move is signed; volatile contexts stay as they are.", "Move them"))) return { ok: true, message: `Activated ${reference}; the attachments still use the older version.` };
    const failed: string[] = [];
    for (const row of moving) {
      progress(`Moving ${row.target} to ${reference}`);
      const moved = await call(deps, ["brick", "upgrade", row.attachment_id, "--to", reference, "--reason", text(values.reason)]);
      if (!moved.envelope || !succeeded(moved.envelope.status)) failed.push(`${row.target}: ${moved.error ?? friendlyError(moved.envelope?.summary ?? "")}`);
    }
    return failed.length ? { ok: false, message: `Activated ${reference}, but ${failed.length} attachment(s) could not move.`, problems: failed } : { ok: true, message: `Activated ${reference} and moved ${moving.length} attachment(s) to it.` };
  }
}

/** The Bricks panel page. */
export const BRICKS_PAGE: WorkbenchPage = {
  viewType: BRICKS_PANEL,
  title: "Landscape Bricks",
  styles: ``,
  body: `<div class="shell">
<header class="top"><span class="mark" aria-hidden="true">▦</span><div class="grow"><b>Bricks</b><div class="muted small" id="summary">Loading…</div></div><button class="btn" id="refresh" title="Ask Crane again">Refresh</button></header>
<div id="notice" aria-live="polite"></div>
<div id="problem"></div>
<div class="layout">
  <section class="card"><div class="head"><b class="grow">Landscape</b><span class="muted small">Click a Zone or Flow to work on it</span></div><div class="graph" id="graph" data-item="Brick" data-glyph="◆"></div></section>
  <section class="card"><div class="head"><b class="grow" id="selected-title">Selected</b></div><div class="body" id="selected"></div></section>
</div>
<section class="card">
  <div class="tabs" role="tablist">
    <button data-tab="create" role="tab" aria-selected="true">Create a Brick</button>
    <button data-tab="attach" role="tab">Attach</button>
    <button data-tab="context" role="tab">Edit context</button>
    <button data-tab="library" role="tab">Library<span class="count" id="count-library"></span></button>
  </div>
  <div class="body">
    <form data-panel="create" id="create-form" novalidate>
      <div class="grid2">
        <div class="field" data-field="type"><label for="c-type">Kind of Brick</label><select id="c-type"></select><div class="help" id="c-type-help"></div></div>
        <div class="field" data-field="name"><label for="c-name">Name</label><input type="text" id="c-name" placeholder="Production config guard"></div>
      </div>
      <div class="field" data-field="intent"><label for="c-intent">What it solves</label><input type="text" id="c-intent" placeholder="Agents must not change production settings without a person"></div>
      <div class="field" data-field="common"><label for="c-common">Common context</label><textarea id="c-common" placeholder="What every agent should know wherever this Brick is attached…"></textarea><div class="help">Shared by every Zone and Flow this Brick is attached to. It is guidance for agents, never permission. Never put secrets here.</div></div>
      <details class="more"><summary>Configuration (optional)</summary><div class="field" data-field="config" style="margin-top:8px"><textarea class="code" id="c-config" spellcheck="false" aria-label="Configuration JSON"></textarea><div class="help">JSON for this kind; Crane checks it against the kind's schema. <a href="#" id="c-reset">Reset to the example</a></div></div></details>
      <label class="check"><input type="checkbox" id="c-now"><span class="grow">Activate and attach it now<small>Activating is signed with an admin key. Crane shows what changes before anything is signed.</small></span></label>
      <div id="c-now-fields" hidden style="margin-top:10px">
        <div class="grid2">
          <div class="field" data-field="target"><label for="c-target">Attach to</label><select id="c-target" data-targets></select><div class="help">Or click a Zone or Flow in the graph.</div></div>
          <div class="field" data-field="reason"><label for="c-reason">Why activate it?</label><input type="text" id="c-reason" placeholder="Recorded in the signed audit"></div>
        </div>
        <div class="field" data-field="volatile"><label for="c-volatile">Volatile context there <span class="muted">(optional)</span></label><textarea id="c-volatile" placeholder="What is true there right now, e.g. a release freeze until Friday…"></textarea><div class="help">Only this Zone or Flow gets it. Change it any time without a new version.</div></div>
      </div>
      <div class="row" style="margin-top:12px"><button class="btn primary" id="c-submit" type="submit">Create Brick</button><span class="muted small">It starts as a draft and changes nothing until it is activated and attached.</span></div>
    </form>

    <form data-panel="attach" id="attach-form" novalidate hidden>
      <div id="a-empty" class="empty-note" hidden>No Brick is activated yet. Create one, then activate it from the Library.</div>
      <div class="grid2">
        <div class="field" data-field="subject"><label for="a-subject">Brick</label><select id="a-subject"></select><div class="help">Only activated Bricks and Stacks can be attached.</div></div>
        <div class="field" data-field="target"><label for="a-target">Zone or Flow</label><select id="a-target" data-targets></select><div class="help">Or click one in the graph.</div></div>
      </div>
      <div class="field" data-field="volatile"><label for="a-volatile">Volatile context <span class="muted">(optional)</span></label><textarea id="a-volatile" placeholder="What agents should know about this Zone or Flow right now…"></textarea><div class="help">This attachment only. It can add guidance but never relaxes a control.</div></div>
      <div class="row"><button class="btn primary" id="a-submit" type="submit">Attach</button><span class="muted small">You see what changes before Crane signs it.</span></div>
    </form>

    <div data-panel="context" hidden>
      <div class="grid2">
        <form id="common-form" novalidate>
          <h3 style="margin:0 0 4px">Common context</h3><p class="muted small" style="margin:0 0 10px">Shared by every attachment of the Brick.</p>
          <div class="field" data-field="brick"><label for="k-brick">Brick</label><select id="k-brick"></select></div>
          <div class="field" data-field="text"><label for="k-text">Text</label><textarea id="k-text" style="min-height:120px" placeholder="Choose a Brick to load its common context"></textarea></div>
          <div id="k-promote-box" hidden>
            <div class="banner info" style="margin-bottom:8px"><span class="icon">ℹ</span><div class="grow small">This Brick is activated, so saving starts its next version as a draft. The current version stays in force until the new one is activated.</div></div>
            <label class="check"><input type="checkbox" id="k-promote"><span class="grow">Then activate the new version and move every attachment to it<small>Signed with an admin key.</small></span></label>
            <div class="field" data-field="reason" id="k-reason-box" hidden style="margin-top:8px"><label for="k-reason">Why?</label><input type="text" id="k-reason" placeholder="Recorded in the signed audit"></div>
          </div>
          <div class="row" style="margin-top:10px"><button class="btn primary" id="k-submit" type="submit">Save common context</button></div>
        </form>
        <form id="volatile-form" novalidate>
          <h3 style="margin:0 0 4px">Volatile context</h3><p class="muted small" style="margin:0 0 10px">For one attachment: what is true on that Zone or Flow right now.</p>
          <div class="field" data-field="attachment"><label for="v-attachment">Attachment</label><select id="v-attachment"></select></div>
          <div class="field" data-field="text"><label for="v-text">Text</label><textarea id="v-text" style="min-height:120px" placeholder="Choose an attachment to load its volatile context"></textarea></div>
          <div class="row" style="margin-top:10px"><button class="btn primary" id="v-submit" type="submit">Save volatile context</button><span class="muted small">Signed in place; no new version.</span></div>
        </form>
      </div>
    </div>

    <div data-panel="library" hidden id="library"></div>
  </div>
</section>
</div>`,
  script: `
let S = { graph: { nodes: [], edges: [] }, targets: [], bricks: [], stacks: [], attachments: [], types: [], templates: {} };
let configTouched = false;
let commonLoaded = "";
const graph = Graph($("graph"), (node) => { renderSelected(node); if (node && node.kind !== "item") setTarget(node.id); });
const subjectName = (subject) => { const brick = S.bricks.find((row) => "brick:" + row.ref === subject || row.ref === subject); return brick ? brick.name : subject.replace(/^(brick|stack):/, ""); };
const brickRef = (subject) => subject.replace(/^brick:/, "");
const stateTone = (state) => state === "active" ? "ok" : state === "draft" ? "warn" : state === "revoked" ? "bad" : "";
function options(select, items, placeholder) {
  const keep = select.value;
  select.innerHTML = (placeholder ? '<option value="">' + esc(placeholder) + "</option>" : "") + items.map((item) => '<option value="' + esc(item.value) + '">' + esc(item.label) + "</option>").join("");
  if (items.some((item) => item.value === keep)) select.value = keep;
}
function setTarget(key) { for (const select of document.querySelectorAll("[data-targets]")) if ([...select.options].some((option) => option.value === key)) select.value = key; }

function onState(state) {
  S = Object.assign(S, state);
  $("problem").innerHTML = state.problem ? '<div class="banner" role="alert"><span class="icon">⚠</span><div class="grow"><b>Crane could not list the Bricks</b><div>' + esc(state.problem) + "</div></div></div>" : state.graphError ? '<div class="banner info"><span class="icon">ℹ</span><div class="grow"><b>The graph is not available</b><div>' + esc(state.graphError) + "</div></div></div>" : "";
  const active = S.bricks.filter((row) => row.state === "active").length;
  $("summary").textContent = S.bricks.length + " Brick version(s) · " + active + " active · " + S.attachments.length + " attachment(s)" + (S.store ? " · signed store " + S.store.trust : "");
  $("count-library").textContent = String(S.bricks.length);
  graph.set(S.graph);
  // The kinds, with what each does and how far Crane enforces it
  const type = $("c-type");
  if (!type.options.length) {
    type.innerHTML = S.types.map((info) => '<option value="' + esc(info.type) + '">' + esc(info.label || info.type) + "</option>").join("");
    applyType();
  }
  const targets = S.targets.map((target) => ({ value: target.key, label: target.key + (target.status && target.status !== "active" ? " (" + target.status + ")" : "") }));
  for (const select of document.querySelectorAll("[data-targets]")) options(select, targets, "Choose a Zone or Flow");
  const attachable = S.bricks.filter((row) => row.state === "active").map((row) => ({ value: row.ref, label: row.name + " · " + row.ref + " · " + row.type })).concat(S.stacks.filter((row) => row.state === "active").map((row) => ({ value: "stack:" + row.stack_id + "@" + row.version, label: row.name + " · Stack " + row.stack_id + "@" + row.version })));
  options($("a-subject"), attachable, "Choose a Brick");
  $("a-empty").hidden = attachable.length > 0;
  options($("k-brick"), S.bricks.filter((row) => row.state === "draft" || row.state === "active").map((row) => ({ value: row.state === "draft" ? row.brick_id : row.ref, label: row.name + " · " + (row.state === "draft" ? row.brick_id + " draft" : row.ref) })), "Choose a Brick");
  options($("v-attachment"), S.attachments.map((row) => ({ value: row.attachment_id, label: row.target + " · " + subjectName(row.subject) + " (" + brickRef(row.subject) + ")" })), "Choose an attachment");
  renderLibrary();
  const picked = graph.picked();
  renderSelected(picked ? S.graph.nodes.find((node) => node.id === picked) : null);
}
function onFocus(id) { graph.pick(id); }

function applyType() {
  const info = S.types.find((item) => item.type === $("c-type").value);
  $("c-type-help").textContent = info ? (info.purpose ? info.purpose + " " : "") + (info.support ? "Crane " + (info.support === "enforced" ? "enforces it at every tool call." : info.support === "checked" ? "checks it." : info.support === "delivered" ? "delivers it to agents as context." : "records it; it is not enforced yet.") : "") : "";
  if (!configTouched) $("c-config").value = JSON.stringify(S.templates[$("c-type").value] || {}, null, 2);
}
$("c-type").addEventListener("change", applyType);
$("c-config").addEventListener("input", () => { configTouched = true; });
$("c-reset").addEventListener("click", (event) => { event.preventDefault(); configTouched = false; applyType(); });
$("c-now").addEventListener("change", () => { $("c-now-fields").hidden = !$("c-now").checked; });
$("refresh").addEventListener("click", () => { notice(); vscode.postMessage({ type: "refresh" }); });

// ---------------------------------------------------------------- the selected Zone, Flow, or attachment
function attachmentCard(row) {
  return '<div class="item"><div class="title"><b>' + esc(subjectName(row.subject)) + '</b><span class="tag">' + esc(brickRef(row.subject)) + '</span><span class="tag ' + (row.issues.length ? "bad" : row.state === "enabled" ? "ok" : "") + '">' + esc(row.issues.length ? "fails closed" : row.state) + "</span></div>" +
    (row.issues.length ? '<div class="error-text">' + esc(row.issues.join("; ")) + "</div>" : "") +
    '<div class="quote">' + (row.current_context ? esc(row.current_context) : "<i>No volatile context</i>") + "</div>" +
    '<div class="actions"><button class="btn small" data-volatile="' + esc(row.attachment_id) + '">Edit volatile context</button><button class="btn small" data-common="' + esc(brickRef(row.subject)) + '">Edit common context</button><button class="btn small danger" data-detach="' + esc(row.attachment_id) + '">Detach</button></div></div>';
}
function renderSelected(node) {
  const box = $("selected");
  if (!node) { $("selected-title").textContent = "Selected"; box.innerHTML = '<div class="empty-note">Click a Zone or Flow in the graph to see its Bricks and attach new ones.</div>'; return; }
  if (node.kind === "item") {
    const row = S.attachments.find((item) => "item:" + item.attachment_id === node.id);
    $("selected-title").textContent = "Attachment";
    box.innerHTML = row ? '<div class="muted small" style="margin-bottom:6px">on <b>' + esc(row.target) + "</b></div>" + attachmentCard(row) : '<div class="empty-note">This attachment is gone.</div>';
    return;
  }
  const rows = S.attachments.filter((row) => row.target === node.id);
  $("selected-title").textContent = (node.kind === "flow" ? "Flow " : "Zone ") + node.name;
  box.innerHTML = '<div class="row" style="margin-bottom:10px"><span class="tag ' + (node.kind === "flow" ? "fox" : "busy") + '">' + esc(node.kind) + '</span><span class="muted small grow">' + esc(node.sub) + "</span></div>" +
    (node.missing ? '<div class="banner"><span class="icon">⚠</span><div class="grow small">Bricks are attached to this ' + esc(node.kind) + ", but it is not in the signed governance. Detach them, or create the " + esc(node.kind) + " again.</div></div>" : "") +
    '<div class="list">' + (rows.length ? rows.map(attachmentCard).join("") : '<div class="empty-note">No Brick attached yet.</div>') + "</div>" +
    '<div class="row" style="margin-top:10px"><button class="btn primary" data-attach-here="' + esc(node.id) + '">Attach a Brick here</button><button class="btn" data-preview="' + esc(node.id) + '">Preview what agents receive</button></div>';
}

// ---------------------------------------------------------------- the library
function renderLibrary() {
  const groups = [["Drafts", (row) => row.state === "draft"], ["Active", (row) => row.state === "active"], ["Deprecated and revoked", (row) => row.state === "deprecated" || row.state === "revoked"]];
  const html = groups.map(([title, filter]) => {
    const rows = S.bricks.filter(filter);
    if (!rows.length) return "";
    return "<h3 style=\\"margin:4px 0 8px\\">" + esc(title) + ' <span class="muted small">' + rows.length + '</span></h3><div class="list" style="margin-bottom:14px">' + rows.map((row) => {
      const where = S.attachments.filter((item) => brickRef(item.subject) === row.ref).map((item) => item.target);
      const id = row.state === "draft" ? row.brick_id : row.ref;
      const actions = row.state === "draft"
        ? '<button class="btn small primary" data-activate="' + esc(row.brick_id) + '">Activate</button><button class="btn small" data-validate="' + esc(row.brick_id) + '">Check for problems</button><button class="btn small" data-common="' + esc(id) + '">Edit common context</button>'
        : row.state === "active" ? '<button class="btn small primary" data-attach-brick="' + esc(row.ref) + '">Attach…</button><button class="btn small" data-common="' + esc(id) + '">Edit common context</button>' : "";
      return '<div class="item"><div class="title"><b>' + esc(row.name) + '</b><span class="tag">' + esc(row.type) + '</span><span class="tag ' + stateTone(row.state) + '">' + esc(row.state) + "</span>" + (row.validation && row.validation !== "passed" ? '<span class="tag bad">' + esc(row.validation) + "</span>" : "") + '<span class="muted small mono">' + esc(id) + "</span></div>" +
        (row.intent ? '<div class="muted small" style="margin-top:4px">' + esc(row.intent) + "</div>" : "") +
        (where.length ? '<div class="small" style="margin-top:4px">Attached to ' + where.map((key) => '<a href="#" data-goto="' + esc(key) + '">' + esc(key) + "</a>").join(", ") + "</div>" : "") +
        (actions ? '<div class="actions">' + actions + "</div>" : "") + "</div>";
    }).join("") + "</div>";
  }).join("");
  $("library").innerHTML = html || '<div class="empty-note">No Bricks yet. Create the first one in "Create a Brick".</div>';
}

// ---------------------------------------------------------------- buttons anywhere on the page
document.addEventListener("click", async (event) => {
  const el = event.target.closest("[data-volatile],[data-common],[data-detach],[data-attach-here],[data-preview],[data-activate],[data-validate],[data-attach-brick],[data-goto]");
  if (!el) return;
  event.preventDefault();
  const d = el.dataset;
  if (d.volatile) { selectTab("context"); $("v-attachment").value = d.volatile; loadVolatile(); $("v-text").focus(); }
  else if (d.common) { selectTab("context"); const select = $("k-brick"); const match = [...select.options].find((option) => option.value === d.common || option.value.split("@")[0] === d.common.split("@")[0]); if (match) { select.value = match.value; loadCommon(); } }
  else if (d.detach) perform(el, "detach", { attachment: d.detach });
  else if (d.attachHere) { selectTab("attach"); setTarget(d.attachHere); $("a-subject").focus(); }
  else if (d.preview) perform(el, "preview", { target: d.preview });
  else if (d.activate) perform(el, "activate", { id: d.activate });
  else if (d.validate) perform(el, "validate", { id: d.validate });
  else if (d.attachBrick) { selectTab("attach"); $("a-subject").value = d.attachBrick; const picked = graph.picked(); if (picked && !picked.startsWith("item:")) setTarget(picked); $("a-target").focus(); }
  else if (d.goto) { graph.pick(d.goto); $("graph").scrollIntoView({ block: "nearest" }); }
});

// ---------------------------------------------------------------- the forms
$("create-form").addEventListener("submit", (event) => {
  event.preventDefault();
  const form = $("create-form");
  clearErrors(form);
  const values = { type: $("c-type").value, name: $("c-name").value.trim(), intent: $("c-intent").value.trim(), common: $("c-common").value, config: $("c-config").value, now: $("c-now").checked, target: $("c-target").value, reason: $("c-reason").value.trim(), volatile: $("c-volatile").value };
  let bad = false;
  if (!values.name) { fieldError("name", "Give the Brick a name."); bad = true; }
  try { JSON.parse(values.config || "{}"); } catch (error) { form.querySelector("details.more").open = true; fieldError("config", "Not valid JSON: " + error.message); bad = true; }
  if (values.now && !values.reason) { fieldError("reason", "Say why it is activated; it is recorded."); bad = true; }
  if (bad) { notice("bad", "Some fields need attention."); return; }
  perform($("c-submit"), "create", values, () => { for (const id of ["c-name", "c-intent", "c-common", "c-volatile", "c-reason"]) $(id).value = ""; configTouched = false; applyType(); });
});
$("attach-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("attach-form"));
  const values = { subject: $("a-subject").value, target: $("a-target").value, volatile: $("a-volatile").value };
  if (!values.subject) fieldError("subject", "Choose a Brick.");
  if (!values.target) fieldError("target", "Choose a Zone or Flow, or click one in the graph.");
  if (!values.subject || !values.target) { notice("bad", "Choose what to attach and where."); return; }
  perform($("a-submit"), "attach", values, () => { $("a-volatile").value = ""; });
});
async function loadCommon() {
  const ref = $("k-brick").value;
  const row = S.bricks.find((item) => item.ref === ref || (item.state === "draft" && item.brick_id === ref));
  $("k-promote-box").hidden = !row || row.state !== "active";
  if (!ref) { $("k-text").value = ""; return; }
  $("k-text").value = "Loading…";
  const result = await ask("load:common", { ref });
  commonLoaded = result.ok ? String(result.data || "") : "";
  $("k-text").value = commonLoaded;
  if (!result.ok) notice("bad", result.message);
}
function loadVolatile() { const row = S.attachments.find((item) => item.attachment_id === $("v-attachment").value); $("v-text").value = row ? row.current_context || "" : ""; }
$("k-brick").addEventListener("change", loadCommon);
$("v-attachment").addEventListener("change", loadVolatile);
$("k-promote").addEventListener("change", () => { $("k-reason-box").hidden = !$("k-promote").checked; });
$("common-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("common-form"));
  const values = { ref: $("k-brick").value, text: $("k-text").value, promote: !$("k-promote-box").hidden && $("k-promote").checked, reason: $("k-reason").value.trim() };
  if (!values.ref) { fieldError("brick", "Choose a Brick."); return; }
  if (values.text === commonLoaded) { notice("info", "Nothing changed in the common context."); return; }
  if (values.promote && !values.reason) { fieldError("reason", "Say why; it is recorded."); return; }
  perform($("k-submit"), "common", values, (result) => { commonLoaded = values.text; });
});
$("volatile-form").addEventListener("submit", (event) => {
  event.preventDefault();
  clearErrors($("volatile-form"));
  const values = { attachment: $("v-attachment").value, text: $("v-text").value };
  if (!values.attachment) { fieldError("attachment", "Choose an attachment."); return; }
  const row = S.attachments.find((item) => item.attachment_id === values.attachment);
  if (row && (row.current_context || "") === values.text) { notice("info", "Nothing changed in the volatile context."); return; }
  perform($("v-submit"), "volatile", values);
});
renderSelected(null);
vscode.postMessage({ type: "ready" });
`,
};
