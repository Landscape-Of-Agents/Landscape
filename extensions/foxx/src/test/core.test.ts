// Unit tests of the editor-independent parts of Landscape (node --test).

import assert from "node:assert/strict";
import { test } from "node:test";

import { parse } from "yaml";

import { atLeast, craneStatus, parseVersion, selectionArgs, selectionIdFrom } from "../core/crane";
import { ACTIONS, GROUPS, groupActions, search } from "../core/actions";
import { activate, addContext, addFlow, addZone, attachContextPack, attachToFlow, attachToZone, connectZones, contextPackOf, entities, findCycle, linkZones, stageCycle, unlinkZones } from "../core/governance";
import { busiest, layout } from "../core/graph";
import { contextLabel, guidanceLines, liveFresh } from "../core/live";
import { parsePolicies } from "../core/policies";
import { checkName, confirmation, ownerKey, repoArgs } from "../core/setup";

test("parses policies and commands", () => {
  const policies = parsePolicies(".crane/policies/p.crane", "# c\npolicy default {\n}\npolicy Payments {\n    preserve K7M2P9RX; // keep\n    target ABCD2345 change_type logical_bn;\n}\n");
  assert.equal(policies.length, 2);
  assert.equal(policies[1].name, "Payments");
  assert.deepEqual(policies[1].commands.map((command) => [command.operation, command.id, command.changeType]), [["preserve", "K7M2P9RX", undefined], ["target", "ABCD2345", "logical_bn"]]);
});

test("builds protect and target arguments", () => {
  assert.deepEqual(selectionArgs("preserve", "a.py", 3, 9, { context: "Keep the fee", policy: "p" }), ["protect", "a.py", "start-line", "3", "end-line", "9", "context", "Keep the fee", "policy", "p"]);
  assert.deepEqual(selectionArgs("target", "a.py", 1, 1, { context: "Rename it", changeType: "semantic", name: "n" }), ["target", "a.py", "start-line", "1", "end-line", "1", "context", "Rename it", "change_type", "semantic", "name", "n"]);
  assert.equal(selectionIdFrom("Protected a.py lines 1-4 as selection K7M2P9RX (policy default)"), "K7M2P9RX");
});

test("creates proposed zones and flows and keeps comments", () => {
  let text = "# governance\nversion: 1\n";
  text = addZone(text, { id: "payments", description: "Payments", criticality: "critical", autonomy_ceiling: "assisted", selectors: { paths: ["app/**"] } });
  text = addFlow(text, { id: "submit", description: "Submission", criticality: "sensitive", autonomy_ceiling: "delegated", entry_points: ["app/service.py:submit"] });
  assert.match(text, /# governance/);
  assert.match(text, /status: proposed/);
  assert.deepEqual(entities(text).map((entity) => [entity.kind, entity.id, entity.status]), [["zone", "payments", "proposed"], ["flow", "submit", "proposed"]]);
  assert.throws(() => addZone(text, { id: "submit", description: "", criticality: "routine", autonomy_ceiling: "observe", selectors: {} }), /already used/);
  assert.throws(() => addZone(text, { id: "9bad", description: "", criticality: "routine", autonomy_ceiling: "observe", selectors: {} }), /zone id/);
});

test("attaches selectors, entry points, and context packs without duplicates", () => {
  let text = addZone("", { id: "z", description: "", criticality: "routine", autonomy_ceiling: "delegated", selectors: {} });
  text = attachToZone(text, "z", "files", "app/a.py");
  text = attachToZone(text, "z", "files", "app/a.py");
  assert.equal((text.match(/app\/a\.py/g) ?? []).length, 1);
  text = addFlow(text, { id: "f", description: "", criticality: "routine", autonomy_ceiling: "delegated", entry_points: ["x"] });
  text = attachToFlow(text, "f", "include", "lib/**");
  text = attachContextPack(text, "zone", "z", "invariants", ["Be idempotent."]);
  text = attachContextPack(text, "flow", "f", "invariants", []);
  assert.equal(entities(text).filter((entity) => entity.kind === "context_pack").length, 1);
  assert.match(text, /Be idempotent\./);
  text = activate(text, "zone", "z");
  assert.equal(entities(text).find((entity) => entity.id === "z")?.status, "active");
  assert.throws(() => attachToZone(text, "missing", "files", "a"), /no zone/);
});

test("connects zones in order, preserving stage metadata while removing top-level zones", () => {
  let text = addZone("", { id: "first", description: "First stage", criticality: "sensitive", autonomy_ceiling: "delegated", selectors: { paths: ["src/first/**"], selections: ["ABCD1234"] } });
  text = addZone(text, { id: "second", description: "Second stage", criticality: "critical", autonomy_ceiling: "assisted", selectors: { files: ["src/second.py"], symbols: ["src/second.py:run"] } });
  text = connectZones(text, "ordered-flow", "The ordered process", ["second", "first"]);
  const parsed = parse(text);
  assert.deepEqual(parsed.zones, []);
  assert.deepEqual(parsed.flows[0].stages.map((stage: { id: string }) => stage.id), ["second", "first"]);
  assert.deepEqual(parsed.flows[0].stages[1].selectors.selections, ["ABCD1234"]);
  assert.deepEqual(parsed.flows[0].entry_points, ["src/second.py:run"]);
  assert.deepEqual(parsed.flows[0].include, ["src/second.py", "src/first/**"]);
  assert.equal(parsed.flows[0].criticality, "critical");
  assert.equal(parsed.flows[0].autonomy_ceiling, "assisted");
  assert.equal(parsed.flows[0].status, "proposed");
  assert.throws(() => connectZones(text, "cycle", "No cycle", ["first", "first"]), /cannot appear twice/);
  assert.throws(() => connectZones(text, "cycle", "Unknown zone", ["missing", "second"]), /no zone 'missing'/);
});

test("sets up a repository: remote arguments, owner key, and confirmations", () => {
  assert.deepEqual(repoArgs("https://github.com/acme/widgets.git"), ["repo", "--https", "https://github.com/acme/widgets.git"]);
  assert.deepEqual(repoArgs("git@github.com:acme/widgets.git"), ["repo", "--ssh", "git@github.com:acme/widgets.git"]);
  assert.equal(repoArgs("C:/local/mirror.git"), undefined);
  const key = Array.from({ length: 16 }, () => "ab12").join(" ");
  const joined = `This repository's rules are signed, but this machine does not trust their owner yet.
  owner key:   ${key}
Trust this owner key for this repository? (y/N): `;
  assert.equal(ownerKey(joined), key);
  assert.equal(confirmation(joined), "trust_owner");
  assert.equal(confirmation("Accept the current signed rules? (y/N): "), "accept_rules");
  assert.equal(confirmation("Crane is already initialized"), undefined);
  assert.equal(checkName("payments"), undefined);
  assert.notEqual(checkName("9lives"), undefined);
});

test("searches actions by prefix: every typed word must start a word of the action", () => {
  const ids = (query: string) => search(query, ACTIONS).map((action) => action.id);
  assert.equal(ids("").length, ACTIONS.length);
  assert.equal(ids("pres")[0], "preserve");
  assert.equal(ids("prot")[0], "preserve", "keywords match too");
  assert.equal(ids("zone")[0], "zone", "the label that starts closest to the word comes first");
  assert.ok(ids("zone").includes("zone_attach"));
  assert.ok(ids("context").includes("context") && ids("context").includes("policy_context"));
  assert.deepEqual(ids("context zone"), ["context"], "every word must match");
  assert.ok(ids("auto").includes("zone") && ids("auto").includes("flow"));
  assert.deepEqual(ids("xyz"), []);
});

test("adds context to a zone or flow in its own pack, appending without duplicates", () => {
  let text = addZone("", { id: "charge-zone", description: "", criticality: "critical", autonomy_ceiling: "assisted", selectors: { symbols: ["app/payments.py:charge"] } });
  text = addContext(text, "zone", "charge-zone", ["Charges must be idempotent.", "", "  Never retry a capture.  "]);
  text = addContext(text, "zone", "charge-zone", ["Never retry a capture.", "Log every refund."]);
  text = activate(text, "zone", "charge-zone");
  const pack = contextPackOf("charge-zone");
  assert.equal(pack, "charge-zone-context");
  assert.equal((text.match(/Never retry a capture\./g) ?? []).length, 1);
  assert.match(text, /Log every refund\./);
  const parsed = parse(text);
  assert.deepEqual(parsed.zones[0].context_packs, ["charge-zone-context"]);
  assert.deepEqual(parsed.context_packs[0].content, ["Charges must be idempotent.", "Never retry a capture.", "Log every refund."]);
  assert.equal(entities(text).find((entity) => entity.id === pack)?.status, "v2");
  assert.equal(entities(text).find((entity) => entity.id === "charge-zone")?.status, "active");
  assert.throws(() => addContext(text, "flow", "missing", ["x"]), /no flow/);
  assert.throws(() => addContext(text, "zone", "charge-zone", ["  "]), /write the context/);
});

test("checks the Crane version before running anything", () => {
  assert.equal(parseVersion("crane 0.4.10\n"), "0.4.10");
  assert.equal(atLeast("0.4.10", "0.4.8"), true, "compared as numbers, not text");
  assert.equal(atLeast("0.4.3", "0.4.8"), false);
  assert.equal(atLeast("1.0.0", "0.4.8"), true);
  const old = craneStatus("crane", { code: 0, stdout: "crane 0.4.3", stderr: "" }, "/opt/tools/crane");
  assert.equal(old.state, "outdated");
  assert.match(old.message ?? "", /0\.4\.3/);
  assert.match(old.message ?? "", /\/opt\/tools\/crane/, "says which executable is too old");
  assert.equal(craneStatus("crane", { code: 127, stdout: "", stderr: "" }).state, "missing");
  assert.equal(craneStatus("crane", { code: 0, stdout: "crane 0.4.23", stderr: "" }).state, "outdated", "protect and target take context from 0.4.24");
  const current = craneStatus("crane", { code: 0, stdout: "crane 0.4.24", stderr: "" });
  assert.equal(current.state, "ok");
  assert.equal(current.events, true);
});

test("groups actions under menu headings, keeping search rank", () => {
  const all = groupActions(ACTIONS);
  assert.deepEqual(all.map((entry) => entry.group), GROUPS.filter((group) => group !== "Setup"), "headings follow the menu order");
  assert.equal(all.flatMap((entry) => entry.actions).length, ACTIONS.length, "every action is under one heading");
  const zone = groupActions(search("zone", ACTIONS));
  assert.equal(zone[0].group, "Zones", "the best match's heading comes first");
  assert.deepEqual(zone[0].actions.map((action) => action.id), ["zone", "zone_attach"]);
  assert.ok(groupActions(search("connect", ACTIONS)).some((entry) => entry.group === "Flows" && entry.actions[0].id === "connect"));
});

test("lays out a graph in acyclic layers and drops cycle-closing edges", () => {
  const nodes = ["flow", "a", "b", "s1", "s2", "lonely"].map((id) => ({ id }));
  const edges = [
    { from: "flow", to: "a" },
    { from: "a", to: "b" },
    { from: "b", to: "s1" },
    { from: "s1", to: "s2" },
    { from: "flow", to: "a" },
    { from: "missing", to: "a" },
  ];
  const drawn = layout(nodes, edges, 100, 10);
  assert.deepEqual(["flow", "a", "b", "s1", "s2"].map((id) => drawn.placed[id].layer), [0, 1, 2, 3, 4]);
  assert.equal(drawn.placed.lonely.layer, 0);
  assert.equal(drawn.placed.s2.x, 400);
  assert.deepEqual(drawn.dropped, [{ from: "missing", to: "a" }], "the duplicate is merged, the dangling edge dropped");
  const looped = layout([{ id: "x" }, { id: "y" }, { id: "z" }], [{ from: "x", to: "y" }, { from: "y", to: "z" }, { from: "z", to: "x" }]);
  assert.equal(looped.dropped.length, 1, "one edge of the cycle is left out");
  assert.equal(new Set(Object.values(looped.placed).map((place) => place.layer)).size, 3);
});

test("keeps flow stage orders acyclic when zones are connected", () => {
  assert.equal(findCycle([["a", "b"], ["b", "c"], ["a", "c"]]), undefined);
  assert.deepEqual(findCycle([["a", "b"], ["b", "a"]]), ["a", "b", "a"]);
  let text = "version: 1\n";
  for (const id of ["cart", "payments", "ledger"]) text = addZone(text, { id, description: "", criticality: "routine", autonomy_ceiling: "delegated", selectors: { files: [`app/${id}.py`] } });
  // Another flow already runs payments before cart
  const shared = text + "flows:\n  - id: audit\n    stages:\n      - id: payments\n      - id: cart\n";
  assert.equal(stageCycle(text, ["cart", "payments"]), undefined);
  const cycle = stageCycle(shared, ["cart", "payments"]);
  assert.ok(cycle && cycle[0] === cycle[cycle.length - 1], "a cycle starts and ends with the same zone");
  assert.deepEqual(new Set(cycle), new Set(["cart", "payments"]), "the other flow runs payments before cart");
  assert.throws(() => connectZones(shared, "checkout", "", ["cart", "payments"]), /cyclic/);
  assert.match(connectZones(shared, "checkout", "", ["payments", "ledger"]), /id: checkout/, "the same order as the other flow is fine");
});

test("lists active agents first, then the most recent", () => {
  const entry = (id: string, active: boolean, last_at: number) => ({ node: "flow:" + id, kind: "flow" as const, id, calls: 1, denied: 0, last_at, active, agents: [] });
  assert.deepEqual(busiest([entry("old", false, 1), entry("live", true, 2), entry("recent", false, 9)]).map((item) => item.id), ["live", "recent", "old"]);
});

test("links zones by dragging: new flows, appends, prepends, merges, and refusals", () => {
  let text = "version: 1\n";
  for (const [id, status] of [["cart", "active"], ["pay", "active"], ["ledger", "active"], ["audit", "proposed"], ["mail", "active"]]) {
    text = addZone(text, { id, description: "", criticality: id === "pay" ? "critical" : "routine", autonomy_ceiling: id === "pay" ? "assisted" : "delegated", selectors: { files: [`app/${id}.py`] } });
    if (status === "active") text = activate(text, "zone", id);
  }
  const flows = (yaml: string) => (parse(yaml).flows ?? []).map((flow: { id: string; status: string; stages: { id: string }[] }) => `${flow.id}:${flow.status}:${flow.stages.map((stage) => stage.id).join(">")}`);
  const zones = (yaml: string) => (parse(yaml).zones ?? []).map((zone: { id: string }) => zone.id);
  let edit = linkZones(text, "cart", "pay");
  assert.deepEqual(flows(edit.text), ["cart-to-pay:active:cart>pay"], "two enforced zones become an enforced flow");
  assert.deepEqual(zones(edit.text), ["ledger", "audit", "mail"]);
  const created = parse(edit.text).flows[0];
  assert.equal(created.criticality, "critical", "the strictest criticality wins");
  assert.equal(created.autonomy_ceiling, "assisted", "the strictest ceiling wins");
  assert.deepEqual(created.include, ["app/cart.py", "app/pay.py"]);
  edit = linkZones(edit.text, "pay", "ledger");
  assert.deepEqual(flows(edit.text), ["cart-to-pay:active:cart>pay>ledger"], "a zone joins the end");
  edit = linkZones(edit.text, "mail", "cart");
  assert.deepEqual(flows(edit.text), ["cart-to-pay:active:mail>cart>pay>ledger"], "a zone joins the start");
  assert.throws(() => linkZones(edit.text, "ledger", "mail"), /already in flow/, "a flow runs each zone once");
  assert.throws(() => linkZones(edit.text, "cart", "audit"), /one ordered path/, "no branching from the middle");
  assert.throws(() => linkZones(edit.text, "audit", "pay"), /link to its first zone/);
  assert.throws(() => linkZones(edit.text, "audit", "audit"), /cannot follow itself/);
  // Every flow carries context, as Crane requires; Landscape's order line follows the flow
  const packs = parse(edit.text).context_packs as { id: string; content: string[] }[];
  const own = packs.find((pack) => pack.id === "cart-to-pay-context");
  assert.ok(own, "a flow made by linking gets its own context pack");
  assert.equal(own?.content[0], "Flow cart-to-pay runs mail → cart → pay → ledger in this order; a change in one stage must keep the stages after it working.");
  assert.deepEqual(parse(edit.text).flows[0].context_packs, ["cart-to-pay-context"]);
  let other = linkZones(text, "audit", "mail");
  assert.deepEqual(flows(other.text), ["audit-to-mail:proposed:audit>mail"], "a proposed zone keeps the flow proposed");
  other = linkZones(linkZones(other.text, "cart", "pay").text, "mail", "cart");
  assert.deepEqual(flows(other.text), ["audit-to-mail:proposed:audit>mail>cart>pay"], "two flows merge where one ends and the other starts");
});

test("unlinks zones by splitting the flow, giving lone zones back", () => {
  let text = "version: 1\n";
  for (const id of ["a", "b", "c", "d"]) text = activate(addZone(text, { id, description: "", criticality: "routine", autonomy_ceiling: "delegated", selectors: { files: [`app/${id}.py`] } }), "zone", id);
  for (const [from, to] of [["a", "b"], ["b", "c"], ["c", "d"]]) text = linkZones(text, from, to).text;
  const flows = (yaml: string) => (parse(yaml).flows ?? []).map((flow: { id: string; stages: { id: string }[]; include?: string[] }) => `${flow.id}:${flow.stages.map((stage) => stage.id).join(">")}:${(flow.include ?? []).join(",")}`);
  const zones = (yaml: string) => (parse(yaml).zones ?? []).map((zone: { id: string }) => zone.id);
  const middle = unlinkZones(text, "b", "c");
  assert.deepEqual(flows(middle.text), ["a-to-b:a>b:app/a.py,app/b.py", "a-to-b-c:c>d:app/c.py,app/d.py"], "the split keeps both halves as flows");
  const edge = unlinkZones(text, "a", "b");
  assert.deepEqual(flows(edge.text), ["a-to-b-b:b>c>d:app/b.py,app/c.py,app/d.py"]);
  assert.deepEqual(zones(edge.text), ["a"], "a lone zone becomes a zone again");
  let two = "version: 1\n";
  for (const id of ["x", "y"]) two = addZone(two, { id, description: "", criticality: "routine", autonomy_ceiling: "delegated", selectors: {} });
  const pair = unlinkZones(linkZones(two, "x", "y").text, "x", "y");
  assert.deepEqual(flows(pair.text), [], "unlinking a two-zone flow removes it");
  assert.deepEqual(zones(pair.text), ["y", "x"], "both zones are zones again");
  assert.throws(() => unlinkZones(two, "x", "y"), /not in a flow/);
});

test("shows an agent's live context only while it is fresh, with its guidance", () => {
  const context = { entity: "zone:payments", kind: "zone" as const, id: "payments", text: ["Crane guidance for the code this tool call touches (guidance only, never authorization):", "Zone payments (critical, autonomy ceiling assisted): Card charging", "  - Charges must be idempotent; never retry a capture."].join("\n"), locations: [{ path: "app/payments.py" }] };
  const live = { at: 1_000_000, agent: "agt_claude", tool: "Edit", contexts: [context] };
  assert.equal(liveFresh(live, 1_000_000 + 60_000), true);
  assert.equal(liveFresh(live, 1_000_000 + 121_000), false, "older than two minutes is gone");
  assert.equal(liveFresh({ ...live, contexts: [] }, 1_000_000), false, "nothing to show");
  assert.deepEqual(guidanceLines(context.text), ["Zone payments (critical, autonomy ceiling assisted): Card charging", "Charges must be idempotent; never retry a capture."]);
  assert.equal(contextLabel(context, "agt_claude"), "◆ agt_claude · zone payments — Charges must be idempotent; never retry a capture.");
  assert.ok(contextLabel({ ...context, text: "x".repeat(400) }).length <= 140);
});
