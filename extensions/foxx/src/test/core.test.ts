// Unit tests of the editor-independent parts of Foxx (node --test).

import assert from "node:assert/strict";
import { test } from "node:test";

import { parse } from "yaml";

import { selectionArgs, selectionIdFrom } from "../core/crane";
import { ACTIONS, search } from "../core/actions";
import { activate, addContext, addFlow, addZone, attachContextPack, attachToFlow, attachToZone, contextPackOf, entities } from "../core/governance";
import { parsePolicies } from "../core/policies";
import { checkName, confirmation, ownerKey, repoArgs } from "../core/setup";

test("parses policies and commands", () => {
  const policies = parsePolicies(".crane/policies/p.crane", "# c\npolicy default {\n}\npolicy Payments {\n    preserve K7M2P9RX; // keep\n    target ABCD2345 change_type logical_bn;\n}\n");
  assert.equal(policies.length, 2);
  assert.equal(policies[1].name, "Payments");
  assert.deepEqual(policies[1].commands.map((command) => [command.operation, command.id, command.changeType]), [["preserve", "K7M2P9RX", undefined], ["target", "ABCD2345", "logical_bn"]]);
});

test("builds protect and target arguments", () => {
  assert.deepEqual(selectionArgs("preserve", "a.py", 3, 9, { policy: "p" }), ["protect", "a.py", "start-line", "3", "end-line", "9", "policy", "p"]);
  assert.deepEqual(selectionArgs("target", "a.py", 1, 1, { changeType: "semantic", name: "n" }), ["target", "a.py", "start-line", "1", "end-line", "1", "change_type", "semantic", "name", "n"]);
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
