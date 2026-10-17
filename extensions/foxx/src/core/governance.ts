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
