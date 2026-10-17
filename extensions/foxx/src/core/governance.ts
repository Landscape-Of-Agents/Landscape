// Editing .crane/governance.yaml (zones, flows, context packs) while keeping its comments and
// layout. New zones and flows are created with status "proposed", so nothing is enforced until a
// human activates it; every edit is previewed as a diff and validated by Crane before it is kept.

import { Document, isMap, isSeq, parseDocument, YAMLMap, YAMLSeq } from "yaml";

/** Criticality values. */
export const CRITICALITY = ["routine", "sensitive", "critical", "restricted"] as const;

/** Autonomy ceiling values, strictest first. */
export const CEILINGS = ["observe", "assisted", "delegated", "autonomous"] as const;

/** Zone selector kinds. */
export const SELECTOR_KINDS = ["paths", "files", "directories", "symbols", "selections"] as const;

/** A zone as the editor creates it. */
export interface ZoneInput {
  id: string;
  description: string;
  owner?: string;
  criticality: (typeof CRITICALITY)[number];
  autonomy_ceiling: (typeof CEILINGS)[number];
  selectors: Partial<Record<(typeof SELECTOR_KINDS)[number], string[]>>;
}

/** A flow as the editor creates it. */
export interface FlowInput {
  id: string;
  description: string;
  owner?: string;
  criticality: (typeof CRITICALITY)[number];
  autonomy_ceiling: (typeof CEILINGS)[number];
  entry_points: string[];
  stages?: ZoneInput[];
}

/** A summary of one entity for pickers and the tree view. */
export interface EntitySummary {
  kind: "zone" | "flow" | "context_pack";
  id: string;
  status: string;
  description: string;
}

/**
 * Check an entity identifier: a letter followed by letters, digits, '_' or '-'.
 * @param id candidate
 * @returns an error message, or undefined when valid
 */
export function checkId(id: string): string | undefined {
  return /^[A-Za-z][A-Za-z0-9_-]{0,63}$/.test(id) ? undefined : "use 1-64 letters, digits, '_' or '-', starting with a letter";
}

/**
 * Parse governance YAML, creating the version key when the text is empty.
 * @param text current file content ("" when missing)
 * @returns the document
 */
function load(text: string): Document {
  const document = parseDocument(text.trim() ? text : "version: 1\n");
  if (document.errors.length) {
    throw new Error(`governance.yaml is not valid YAML: ${document.errors[0].message}`);
  }
  if (!isMap(document.contents)) {
    throw new Error("governance.yaml must be a mapping");
  }
  return document;
}

/**
 * Return the sequence under a top-level key, creating it when missing.
 * @param document YAML document
 * @param key zones, flows, or context_packs
 * @returns the sequence
 */
function sequence(document: Document, key: string): YAMLSeq {
  const existing = document.get(key, true);
  if (isSeq(existing)) return existing;
  const created = document.createNode([]) as YAMLSeq;
  document.set(key, created);
  return created;
}

/**
 * Find an entity mapping by kind and id.
 * @param document YAML document
 * @param key zones, flows, or context_packs
 * @param id entity id
 * @returns the mapping, or undefined
 */
function find(document: Document, key: string, id: string): YAMLMap | undefined {
  const items = document.get(key, true);
  if (!isSeq(items)) return undefined;
  return items.items.find((item): item is YAMLMap => isMap(item) && item.get("id") === id);
}

/**
 * Collect every zone, flow, and context pack id.
 * @param document YAML document
 * @returns ids
 */
function allIds(document: Document): Set<string> {
  const ids = new Set<string>();
  for (const key of ["zones", "flows", "context_packs"]) {
    const items = document.get(key, true);
    if (isSeq(items)) for (const item of items.items) if (isMap(item)) ids.add(String(item.get("id")));
  }
  return ids;
}

/**
 * Add a proposed zone.
 * @param text current content
 * @param zone zone to add
 * @returns new content
 */
export function addZone(text: string, zone: ZoneInput): string {
  const error = checkId(zone.id);
  if (error) throw new Error(`zone id: ${error}`);
  const document = load(text);
  if (allIds(document).has(zone.id)) throw new Error(`'${zone.id}' is already used by a zone, flow, or context pack`);
  const selectors = Object.fromEntries(Object.entries(zone.selectors).filter(([, values]) => values && values.length));
  sequence(document, "zones").add(
    document.createNode({
      id: zone.id,
      description: zone.description,
      ...(zone.owner ? { owner: zone.owner } : {}),
      selectors,
      criticality: zone.criticality,
      autonomy_ceiling: zone.autonomy_ceiling,
      policies: [],
      context_packs: [],
      status: "proposed",
      version: 1,
    }),
  );
  return document.toString();
}

/**
 * Add a proposed flow.
 * @param text current content
 * @param flow flow to add
 * @returns new content
 */
export function addFlow(text: string, flow: FlowInput): string {
  const error = checkId(flow.id);
  if (error) throw new Error(`flow id: ${error}`);
  if (!flow.entry_points.length) throw new Error("a flow needs at least one entry point");
  const document = load(text);
  if (allIds(document).has(flow.id)) throw new Error(`'${flow.id}' is already used by a zone, flow, or context pack`);
  sequence(document, "flows").add(
    document.createNode({
      id: flow.id,
      description: flow.description,
      ...(flow.owner ? { owner: flow.owner } : {}),
      entry_points: flow.entry_points,
      ...(flow.stages?.length ? { stages: flow.stages } : {}),
      criticality: flow.criticality,
      autonomy_ceiling: flow.autonomy_ceiling,
      policies: [],
      context_packs: [],
      status: "proposed",
      version: 1,
    }),
  );
  return document.toString();
}

/**
 * Find a cycle in a directed graph (depth first, without recursion).
 * @param edges from and to pairs
 * @returns the nodes of one cycle, starting and ending with the same node, or undefined when acyclic
 */
export function findCycle(edges: [string, string][]): string[] | undefined {
  const next = new Map<string, string[]>();
  for (const [from, to] of edges) {
    next.set(from, [...(next.get(from) ?? []), to]);
    if (!next.has(to)) next.set(to, []);
  }
  // 0: not visited, 1: on the current path, 2: finished
  const color = new Map([...next.keys()].map((node) => [node, 0]));
  for (const start of next.keys()) {
    if (color.get(start) !== 0) continue;
    color.set(start, 1);
    const path: [string, number][] = [[start, 0]];
    while (path.length) {
      const top = path[path.length - 1];
      const child = (next.get(top[0]) ?? [])[top[1]];
      if (child === undefined) {
        color.set(top[0], 2);
        path.pop();
        continue;
      }
      top[1] += 1;
      if (color.get(child) === 0) {
        color.set(child, 1);
        path.push([child, 0]);
      } else if (color.get(child) === 1) {
        return [...path.slice(path.findIndex(([node]) => node === child)).map(([node]) => node), child];
      }
    }
  }
  return undefined;
}

/**
 * Check that connecting zones in an order keeps every flow's stage order acyclic, the rule Crane
 * enforces when it validates and signs governance.yaml: no zone twice in one flow, and no flows
 * that run the same zones in opposite orders.
 * @param text current content
 * @param zoneIds zones in the proposed execution order
 * @returns the cycle (zone ids, first and last equal), or undefined when the order is acyclic
 */
export function stageCycle(text: string, zoneIds: string[]): string[] | undefined {
  const document = load(text);
  const edges: [string, string][] = [];
  const flows = document.get("flows", true);
  if (isSeq(flows)) {
    for (const flow of flows.items) {
      if (!isMap(flow) || flow.get("status") === "deprecated") continue;
      const stages = flow.get("stages", true);
      const ids = isSeq(stages) ? stages.items.filter(isMap).map((stage) => String(stage.get("id"))) : [];
      for (let index = 1; index < ids.length; index += 1) edges.push([ids[index - 1], ids[index]]);
    }
  }
  for (let index = 1; index < zoneIds.length; index += 1) edges.push([zoneIds[index - 1], zoneIds[index]]);
  return findCycle(edges);
}

/**
 * Replace an ordered list of zones with one proposed flow whose stages preserve each full zone.
 * @param text current content
 * @param id flow id
 * @param description flow purpose
 * @param zoneIds zones in execution order
 * @returns new content
 */
export function connectZones(text: string, id: string, description: string, zoneIds: string[]): string {
  const error = checkId(id);
  if (error) throw new Error(`flow id: ${error}`);
  if (zoneIds.length < 2) throw new Error("choose at least two zones");
  if (new Set(zoneIds).size !== zoneIds.length) throw new Error("a zone cannot appear twice in one flow");
  const document = load(text);
  if (allIds(document).has(id)) throw new Error(`'${id}' is already used by a zone, flow, or context pack`);
  const cycle = stageCycle(text, zoneIds);
  if (cycle) throw new Error(`this order would make the flows cyclic (${cycle.join(" → ")}); keep one order for zones shared by flows`);
  const zones = document.get("zones", true);
  if (!isSeq(zones)) throw new Error("there are no zones to connect");
  const byId = new Map(zones.items.filter(isMap).map((zone) => [String(zone.get("id")), zone]));
  const stages = zoneIds.map((zoneId) => {
    const zone = byId.get(zoneId);
    if (!zone) throw new Error(`no zone '${zoneId}'`);
    return zone.toJSON() as ZoneInput & { status?: string; version?: number; policies?: string[]; context_packs?: string[] };
  });
  const criticality = [...stages.map((stage) => stage.criticality)].sort((a, b) => CRITICALITY.indexOf(b) - CRITICALITY.indexOf(a))[0];
  const autonomy = [...stages.map((stage) => stage.autonomy_ceiling)].sort((a, b) => CEILINGS.indexOf(a) - CEILINGS.indexOf(b))[0];
  const entryPoints = [...new Set(stages.flatMap((stage) => stage.selectors.symbols || []))];
  const include = [...new Set(stages.flatMap((stage) => [
    ...(stage.selectors.paths || []),
    ...(stage.selectors.files || []),
    ...(stage.selectors.directories || []).map((directory) => `${directory.replace(/\/+$/, "")}/**`),
  ]))];
  sequence(document, "flows").add(document.createNode({
    id,
    description,
    entry_points: entryPoints,
    ...(include.length ? { include } : {}),
    stages,
    criticality,
    autonomy_ceiling: autonomy,
    policies: [...new Set(stages.flatMap((stage) => stage.policies || []))],
    context_packs: [...new Set(stages.flatMap((stage) => stage.context_packs || []))],
    status: "proposed",
    version: 1,
  }));
  zones.items = zones.items.filter((item) => !isMap(item) || !zoneIds.includes(String(item.get("id"))));
  return document.toString();
}

/** Where a zone is defined: a top-level zone, or a stage of a flow at a position. */
interface Placement {
  zone: YAMLMap;
  flow?: YAMLMap;
  stages?: YAMLSeq;
  index: number;
}

/**
 * Find where a zone is defined.
 * @param document YAML document
 * @param id zone id
 * @returns its placement, or undefined
 */
function placement(document: Document, id: string): Placement | undefined {
  const zones = document.get("zones", true);
  const zone = isSeq(zones) ? zones.items.find((item): item is YAMLMap => isMap(item) && item.get("id") === id) : undefined;
  if (zone) return { zone, index: -1 };
  const flows = document.get("flows", true);
  if (!isSeq(flows)) return undefined;
  for (const flow of flows.items) {
    if (!isMap(flow)) continue;
    const stages = flow.get("stages", true);
    if (!isSeq(stages)) continue;
    const index = stages.items.findIndex((item) => isMap(item) && item.get("id") === id);
    if (index >= 0) return { zone: stages.items[index] as YAMLMap, flow, stages, index };
  }
  return undefined;
}

/**
 * An identifier not yet used by a zone, flow, or context pack.
 * @param document YAML document
 * @param base preferred identifier
 * @returns base, or base-2, base-3, …
 */
function freeId(document: Document, base: string): string {
  const used = allIds(document);
  const stem = base.replace(/[^A-Za-z0-9_-]+/g, "-").replace(/^[^A-Za-z]+/, "").slice(0, 58) || "flow";
  let candidate = stem;
  for (let index = 2; used.has(candidate); index += 1) candidate = `${stem}-${index}`;
  return candidate;
}

/**
 * The values a stage contributes to its flow: entry points from its symbols, included paths from
 * its paths, files, and directories, and its policies and context packs.
 * @param stage stage (a zone's fields)
 * @returns contributions
 */
function contributions(stage: ZoneInput & { policies?: string[]; context_packs?: string[] }) {
  const selectors = stage.selectors ?? {};
  return {
    entry_points: selectors.symbols ?? [],
    include: [...(selectors.paths ?? []), ...(selectors.files ?? []), ...(selectors.directories ?? []).map((directory) => `${directory.replace(/\/+$/, "")}/**`)],
    policies: stage.policies ?? [],
    context_packs: stage.context_packs ?? [],
  };
}

/**
 * Add a stage to a flow, at its start or end, merging what the stage contributes and keeping the
 * strictest criticality and autonomy ceiling.
 * @param document YAML document
 * @param flow flow mapping
 * @param stage the stage's fields
 * @param where start or end
 */
function absorb(document: Document, flow: YAMLMap, stage: ZoneInput & { policies?: string[]; context_packs?: string[] }, where: "start" | "end"): void {
  const existingStages = flow.get("stages", true);
  const stages: YAMLSeq = isSeq(existingStages) ? existingStages : (document.createNode([]) as YAMLSeq);
  if (!isSeq(existingStages)) flow.set("stages", stages);
  const node = document.createNode(stage);
  if (where === "end") stages.add(node);
  else stages.items.unshift(node);
  const added = contributions(stage);
  for (const key of ["entry_points", "include", "policies", "context_packs"] as const) {
    if (!added[key].length) continue;
    const existing = flow.get(key, true);
    if (isSeq(existing)) {
      const values = existing.items.map((item) => String((item as { value?: unknown }).value ?? item));
      for (const value of added[key]) if (!values.includes(value)) existing.add(value);
    } else flow.set(key, document.createNode([...new Set(added[key])]));
  }
  const criticality = String(flow.get("criticality") ?? "routine") as ZoneInput["criticality"];
  if (CRITICALITY.indexOf(stage.criticality) > CRITICALITY.indexOf(criticality)) flow.set("criticality", stage.criticality);
  const ceiling = String(flow.get("autonomy_ceiling") ?? "autonomous") as ZoneInput["autonomy_ceiling"];
  if (stage.autonomy_ceiling && CEILINGS.indexOf(stage.autonomy_ceiling) < CEILINGS.indexOf(ceiling)) flow.set("autonomy_ceiling", stage.autonomy_ceiling);
}

/**
 * Remove a mapping from a top-level sequence.
 * @param document YAML document
 * @param key zones or flows
 * @param map the mapping to remove
 */
function removeFrom(document: Document, key: string, map: YAMLMap): void {
  const items = document.get(key, true);
  if (isSeq(items)) items.items = items.items.filter((item) => item !== map);
}

/**
 * Make sure a flow carries context, as Crane requires: when none of its own or its stages' context
 * packs has guidance, give it its own pack describing the order of its zones.
 * @param document YAML document
 * @param flow flow mapping
 */
function ensureFlowContext(document: Document, flow: YAMLMap): void {
  const packs = document.get("context_packs", true);
  const withGuidance = new Set(
    (isSeq(packs) ? packs.items.filter(isMap) : [])
      .filter((pack) => {
        const content = pack.get("content", true);
        const files = pack.get("files", true);
        return (isSeq(content) && content.items.some((item) => String((item as { value?: unknown }).value ?? item).trim())) || (isSeq(files) && files.items.length > 0);
      })
      .map((pack) => String(pack.get("id"))),
  );
  const named = (map: YAMLMap) => {
    const list = map.get("context_packs", true);
    return isSeq(list) ? list.items.map((item) => String((item as { value?: unknown }).value ?? item)) : [];
  };
  const stages = flow.get("stages", true);
  const stageMaps = isSeq(stages) ? stages.items.filter(isMap) : [];
  const id = String(flow.get("id"));
  const order = stageMaps.map((stage) => String(stage.get("id")));
  const orderLine = order.length ? `Flow ${id} runs ${order.join(" → ")} in this order; a change in one stage must keep the stages after it working.` : `Flow ${id}: keep changes consistent along the whole call path.`;
  // The order line Landscape wrote follows the flow as zones join or leave; other guidance is kept
  for (const pack of isSeq(packs) ? packs.items.filter(isMap) : []) {
    if (!named(flow).includes(String(pack.get("id")))) continue;
    const content = pack.get("content", true);
    if (!isSeq(content)) continue;
    const at = content.items.findIndex((item) => String((item as { value?: unknown }).value ?? item).startsWith(`Flow ${id} runs `));
    if (at >= 0 && String((content.items[at] as { value?: unknown }).value ?? content.items[at]) !== orderLine) {
      content.items[at] = document.createNode(orderLine);
      pack.set("version", Number(pack.get("version") ?? 0) + 1);
    }
  }
  if ([...named(flow), ...stageMaps.flatMap(named)].some((pack) => withGuidance.has(pack))) return;
  const lines = [
    orderLine,
    ...stageMaps.filter((stage) => String(stage.get("description") ?? "").trim()).map((stage) => `${stage.get("id")}: ${stage.get("description")}`),
  ];
  const packId = freeId(document, contextPackOf(id));
  sequence(document, "context_packs").add(document.createNode({ id: packId, version: 1, description: `Guidance for flow ${id}`, content: lines }));
  const list = flow.get("context_packs", true);
  if (isSeq(list)) list.add(packId);
  else flow.set("context_packs", document.createNode([packId]));
}

/** What a link or unlink did, for the person. */
export interface GraphEdit {
  text: string;
  summary: string;
}

/**
 * Link one zone to the next, the way dragging a connector does: two free zones become a new flow
 * (named after them, enforced when both zones were), a zone joins the end or the start of a flow,
 * or two flows merge when the first ends where the second begins. A link must keep every flow one
 * ordered path and the stage orders acyclic.
 * @param text current content
 * @param from the zone that runs first
 * @param to the zone that runs next
 * @returns the new content and what changed
 */
export function linkZones(text: string, from: string, to: string): GraphEdit {
  if (from === to) throw new Error("a zone cannot follow itself");
  const document = load(text);
  const a = placement(document, from);
  const b = placement(document, to);
  if (!a) throw new Error(`no zone '${from}'`);
  if (!b) throw new Error(`no zone '${to}'`);
  const name = (map: YAMLMap | undefined) => String(map?.get("id"));
  if (a.flow && a.flow === b.flow) {
    if (b.index === a.index + 1) throw new Error(`${from} already leads to ${to} in flow ${name(a.flow)}`);
    throw new Error(`${from} and ${to} are already in flow ${name(a.flow)}, which runs each zone once`);
  }
  const cycle = stageCycle(text, [from, to]);
  if (cycle) throw new Error(`linking ${from} → ${to} would make a cycle (${cycle.join(" → ")})`);
  if (a.flow && a.stages && a.index !== a.stages.items.length - 1) {
    throw new Error(`${from} already leads to ${name(a.stages.items[a.index + 1] as YAMLMap)} in flow ${name(a.flow)}; a flow is one ordered path, so link from its last zone`);
  }
  if (b.flow && b.index !== 0) {
    throw new Error(`${to} already follows ${name(b.stages?.items[b.index - 1] as YAMLMap)} in flow ${name(b.flow)}; link to its first zone`);
  }
  type Stage = ZoneInput & { status?: string; policies?: string[]; context_packs?: string[] };
  let summary: string;
  if (!a.flow && !b.flow) {
    const first = a.zone.toJSON() as Stage;
    const second = b.zone.toJSON() as Stage;
    const id = freeId(document, `${from}-to-${to}`);
    const flow = document.createNode({ id, description: `${from} → ${to}`, entry_points: [], criticality: "routine", autonomy_ceiling: "autonomous", policies: [], context_packs: [], status: "proposed", version: 1 }) as YAMLMap;
    absorb(document, flow, first, "end");
    absorb(document, flow, second, "end");
    // Two enforced zones stay enforced as one flow; otherwise the flow waits for review
    flow.set("status", first.status !== "proposed" && second.status !== "proposed" ? "active" : "proposed");
    sequence(document, "flows").add(flow);
    removeFrom(document, "zones", a.zone);
    removeFrom(document, "zones", b.zone);
    summary = `flow ${id} created: ${from} → ${to}`;
  } else if (a.flow && !b.flow) {
    absorb(document, a.flow, b.zone.toJSON() as Stage, "end");
    removeFrom(document, "zones", b.zone);
    summary = `${to} added after ${from} in flow ${name(a.flow)}`;
  } else if (!a.flow && b.flow) {
    absorb(document, b.flow, a.zone.toJSON() as Stage, "start");
    removeFrom(document, "zones", a.zone);
    summary = `${from} added before ${to} in flow ${name(b.flow)}`;
  } else {
    const target = a.flow as YAMLMap;
    const merged = b.flow as YAMLMap;
    for (const stage of (b.stages as YAMLSeq).items) if (isMap(stage)) absorb(document, target, stage.toJSON() as Stage, "end");
    removeFrom(document, "flows", merged);
    summary = `flows ${name(target)} and ${name(merged)} merged at ${from} → ${to}`;
  }
  const flow = placement(document, from)?.flow as YAMLMap;
  ensureFlowContext(document, flow);
  flow.set("version", Number(flow.get("version") ?? 0) + 1);
  return { text: document.toString(), summary };
}

/**
 * Remove the link between two consecutive zones of a flow, splitting it there: each side with two
 * or more zones stays a flow (the second gets a new name), and a zone left alone becomes a
 * top-level zone again. A flow with its own entry points is kept even with one zone.
 * @param text current content
 * @param from the zone before the link
 * @param to the zone after it
 * @returns the new content and what changed
 */
export function unlinkZones(text: string, from: string, to: string): GraphEdit {
  const document = load(text);
  const a = placement(document, from);
  if (!a?.flow || !a.stages) throw new Error(`${from} is not in a flow`);
  const next = a.stages.items[a.index + 1];
  if (!isMap(next) || next.get("id") !== to) throw new Error(`${from} does not lead to ${to}`);
  const flow = a.flow;
  const flowId = String(flow.get("id"));
  const left = a.stages.items.slice(0, a.index + 1).filter(isMap);
  const right = a.stages.items.slice(a.index + 1).filter(isMap);
  type Stage = ZoneInput & { policies?: string[]; context_packs?: string[] };
  // What the moved stages brought into the flow leaves with them
  const moved = right.map((stage) => contributions(stage.toJSON() as Stage));
  for (const key of ["entry_points", "include"] as const) {
    const existing = flow.get(key, true);
    const gone = new Set(moved.flatMap((item) => item[key]));
    if (isSeq(existing)) existing.items = existing.items.filter((item) => !gone.has(String((item as { value?: unknown }).value ?? item)));
  }
  a.stages.items = left;
  const restore = (stages: YAMLMap[]) => { for (const stage of stages) sequence(document, "zones").add(document.createNode(stage.toJSON())); };
  const parts: string[] = [];
  if (right.length >= 2) {
    const id = freeId(document, `${flowId}-${to}`);
    const created = document.createNode({ id, description: `Split from ${flowId}`, entry_points: [], criticality: "routine", autonomy_ceiling: "autonomous", policies: [], context_packs: [], status: String(flow.get("status") ?? "proposed"), version: 1 }) as YAMLMap;
    for (const stage of right) absorb(document, created, stage.toJSON() as Stage, "end");
    sequence(document, "flows").add(created);
    ensureFlowContext(document, created);
    parts.push(`flow ${id} now runs ${right.map((stage) => stage.get("id")).join(" → ")}`);
  } else {
    restore(right);
    parts.push(`${to} is a zone again`);
  }
  // Entry points the flow was given itself, not brought in by the zone that remains
  const entries = flow.get("entry_points", true);
  const fromLeft = new Set(left.flatMap((stage) => contributions(stage.toJSON() as Stage).entry_points));
  const own = isSeq(entries) ? entries.items.filter((item) => !fromLeft.has(String((item as { value?: unknown }).value ?? item))) : [];
  if (left.length >= 2 || own.length) {
    ensureFlowContext(document, flow);
    flow.set("version", Number(flow.get("version") ?? 0) + 1);
    parts.unshift(`flow ${flowId} ends at ${from}`);
  } else {
    restore(left);
    removeFrom(document, "flows", flow);
    parts.unshift(`flow ${flowId} removed, ${from} is a zone again`);
  }
  return { text: document.toString(), summary: parts.join("; ") };
}

/**
 * List repository-relative source paths covered by a zone or a flow's linked stages.
 * @param text governance YAML
 * @param kind entity kind
 * @param id entity identifier
 * @returns source paths and globs
 */
export function sourcePaths(text: string, kind: "zone" | "flow", id: string): string[] {
  const document = load(text);
  const entity = find(document, kind === "zone" ? "zones" : "flows", id);
  if (!entity) throw new Error(`no ${kind} '${id}'`);
  const paths = new Set<string>();
  const collect = (value: unknown) => {
    if (!value || typeof value !== "object") return;
    const selectors = (value as { selectors?: Record<string, unknown> }).selectors;
    if (selectors) {
      for (const key of ["paths", "files", "directories", "symbols"] as const) {
        const items = selectors[key];
        if (Array.isArray(items)) for (const item of items) {
          const path = String(item).split(":")[0];
          if (path && (key !== "symbols" || path.includes("/") || path.includes("\\"))) paths.add(key === "directories" ? `${path.replace(/[\\/]+$/, "")}/**` : path);
        }
      }
    }
  };
  collect(entity.toJSON());
  if (kind === "flow") {
    const stages = entity.get("stages", true);
    if (isSeq(stages)) for (const stage of stages.items) collect(isMap(stage) ? stage.toJSON() : undefined);
    const include = entity.get("include", true);
    if (isSeq(include)) for (const item of include.items) paths.add(String((item as { value?: unknown }).value ?? item));
    const entries = entity.get("entry_points", true);
    if (isSeq(entries)) for (const item of entries.items) {
      const entry = String((item as { value?: unknown }).value ?? item).split(":")[0];
      if (entry.includes("/")) paths.add(entry);
    }
  }
  return [...paths];
}

/**
 * Append a value to a list field of an entity, creating the list, without duplicates, and bump
 * the entity's version.
 * @param map entity mapping
 * @param path field path such as ["selectors", "files"]
 * @param value value to add
 * @param document YAML document
 */
function appendUnique(map: YAMLMap, path: string[], value: string, document: Document): void {
  const existing = map.getIn(path, true);
  if (isSeq(existing)) {
    if (!existing.items.some((item) => String((item as { value?: unknown }).value ?? item) === value)) existing.add(value);
  } else {
    map.setIn(path, document.createNode([value]));
  }
  const version = Number(map.get("version") ?? 0);
  map.set("version", version + 1);
}

/**
 * Attach a resource to a zone through a selector.
 * @param text current content
 * @param zoneId zone id
 * @param kind selector kind
 * @param value path, glob, directory, symbol, or selection id
 * @returns new content
 */
export function attachToZone(text: string, zoneId: string, kind: (typeof SELECTOR_KINDS)[number], value: string): string {
  const document = load(text);
  const zone = find(document, "zones", zoneId);
  if (!zone) throw new Error(`no zone '${zoneId}'`);
  appendUnique(zone, ["selectors", kind], value, document);
  return document.toString();
}

/**
 * Attach an entry point (a symbol) or an included path to a flow.
 * @param text current content
 * @param flowId flow id
 * @param field entry_points, include, or exclude
 * @param value symbol or glob
 * @returns new content
 */
export function attachToFlow(text: string, flowId: string, field: "entry_points" | "include" | "exclude", value: string): string {
  const document = load(text);
  const flow = find(document, "flows", flowId);
  if (!flow) throw new Error(`no flow '${flowId}'`);
  appendUnique(flow, [field], value, document);
  return document.toString();
}

/**
 * Create a context pack (when missing) and attach it to a zone or flow.
 * @param text current content
 * @param kind zone or flow
 * @param id entity id
 * @param pack context pack id
 * @param content guidance lines for a new pack
 * @returns new content
 */
export function attachContextPack(text: string, kind: "zone" | "flow", id: string, pack: string, content: string[]): string {
  const error = checkId(pack);
  if (error) throw new Error(`context pack id: ${error}`);
  const document = load(text);
  const entity = find(document, kind === "zone" ? "zones" : "flows", id);
  if (!entity) throw new Error(`no ${kind} '${id}'`);
  if (!find(document, "context_packs", pack)) {
    if (allIds(document).has(pack)) throw new Error(`'${pack}' is already used by a zone or flow`);
    sequence(document, "context_packs").add(document.createNode({ id: pack, version: 1, description: "", content }));
  }
  appendUnique(entity, ["context_packs"], pack, document);
  return document.toString();
}

/**
 * Name the context pack that holds a zone's or flow's own guidance.
 * @param id entity id
 * @returns pack id ("ID-context", shortened to fit the identifier limit)
 */
export function contextPackOf(id: string): string {
  return `${id.slice(0, 56)}-context`;
}

/**
 * Add guidance lines to a zone's or flow's own context pack, creating and attaching the pack when
 * needed; existing lines are kept and repeated lines are not added twice. The agent receives the
 * pack whenever a tool call touches code the entity covers.
 * @param text current content
 * @param kind zone or flow
 * @param id entity id
 * @param lines guidance lines (blank lines are ignored)
 * @returns new content
 */
export function addContext(text: string, kind: "zone" | "flow", id: string, lines: string[]): string {
  const guidance = lines.map((line) => line.trim()).filter(Boolean);
  if (!guidance.length) throw new Error("write the context first");
  const document = load(text);
  const entity = find(document, kind === "zone" ? "zones" : "flows", id);
  if (!entity) throw new Error(`no ${kind} '${id}'`);
  const packId = contextPackOf(id);
  const pack = find(document, "context_packs", packId);
  if (pack) {
    const content = pack.get("content", true);
    const existing = isSeq(content) ? content.items.map((item) => String((item as { value?: unknown }).value ?? item)) : [];
    const added = guidance.filter((line) => !existing.includes(line));
    if (isSeq(content)) for (const line of added) content.add(line);
    else pack.set("content", document.createNode(guidance));
    pack.set("version", Number(pack.get("version") ?? 0) + 1);
  } else {
    if (allIds(document).has(packId)) throw new Error(`'${packId}' is already used by a zone or flow`);
    sequence(document, "context_packs").add(document.createNode({ id: packId, version: 1, description: `Guidance for ${kind} ${id}`, content: guidance }));
  }
  appendUnique(entity, ["context_packs"], packId, document);
  return document.toString();
}

/**
 * Activate a proposed zone or flow (an explicit, reviewable authority change).
 * @param text current content
 * @param kind zone or flow
 * @param id entity id
 * @returns new content
 */
export function activate(text: string, kind: "zone" | "flow", id: string): string {
  const document = load(text);
  const entity = find(document, kind === "zone" ? "zones" : "flows", id);
  if (!entity) throw new Error(`no ${kind} '${id}'`);
  entity.set("status", "active");
  return document.toString();
}

/**
 * List every entity for pickers and the tree view.
 * @param text current content
 * @returns summaries
 */
export function entities(text: string): EntitySummary[] {
  if (!text.trim()) return [];
  const document = load(text);
  const summaries: EntitySummary[] = [];
  for (const [key, kind] of [["zones", "zone"], ["flows", "flow"], ["context_packs", "context_pack"]] as const) {
    const items = document.get(key, true);
    if (!isSeq(items)) continue;
    for (const item of items.items) {
      if (!isMap(item)) continue;
      summaries.push({
        kind,
        id: String(item.get("id")),
        status: kind === "context_pack" ? `v${item.get("version") ?? 1}` : String(item.get("status") ?? "active"),
        description: String(item.get("description") ?? ""),
      });
    }
  }
  return summaries;
}
