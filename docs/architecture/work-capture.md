# Work Capture: The Evidence Layer, Explained Line by Line

*A book chapter on [src/capture/](../../src/capture), Component 1 of the Capability Compiler.*

This chapter explains every part of Work Capture for a reader who has never seen it. Every word that has a special meaning is **defined in bold** where it first appears. Every claim points to the line of code that implements it. For every part you get the same seven answers:

| Label | Question it answers |
|---|---|
| **Definition** | What is this thing, in plain words? |
| **Code** | Where does it live? |
| **How it works** | What does it do, step by step? |
| **Why it exists** | What problem made someone build it this way? |
| **How it helps** | What does the rest of the system gain from it? |
| **What breaks** | Where does it fail today, with a concrete case? |
| **Fix and criticality** | How to repair it, and how much the failure matters |

### The criticality scale

Every "What breaks" entry carries one of these values:

| Value | Meaning | Typical consequence |
|---|---|---|
| **CRITICAL** | Security, privacy, or data-loss defect | Someone sees what they must not, or evidence is lost |
| **HIGH** | A silently wrong or silently incomplete answer | A pack looks complete or trustworthy but is not |
| **MEDIUM** | A visible but misleading answer, or a failure that appears at scale | People are sent to fix the wrong thing, or it gets slow |
| **LOW** | Ergonomics, a latent risk, or a declared feature not wired up yet | Friction, or a trap for future code |
| **NONE** | Holds as designed today | Nothing to fix |

### Resolution notes (2026-10-21)

The defects in this chapter were fixed after it was written; [WORK_CAPTURE_FIXES.md](work-capture-fixes.md)
is the full register. Every original entry is kept as it was written, so the chapter still explains
why each fix was needed. Where a defect was fixed, a **Resolved** line follows its "Fix and
criticality" entry, saying what changed, how, and the criticality that remains. **Partly resolved**
means part of the defect remains, and the line says which part. Entries with no such line were
either NONE already or were left as designed; the register gives the reason for each. Line numbers
in the original entries describe the code before the fixes; links in the Resolved lines point to the
code as it is now.

### The running story

Every part of this chapter is illustrated with the same story, the one the component's own
demonstration uses ([fixtures/failed_deployment.json](../../src/capture/fixtures/failed_deployment.json)):

> Task **PAY-142, "Make refunds idempotent"**, was given to an AI agent (session `ses_demo01`). The
> agent edited `app/refund_worker.py` and ran `pytest`. The change became **pull request 418**, head
> commit **`9f1c2ab7d4`**, touching `app/refunds.py`, `app/refund_worker.py`, and
> `migrations/0042_refund_keys.sql`. CI run `run-5521` **succeeded**. Deployment **`dep-7731`** to
> **production** **failed**, and a smoke check said **unhealthy**. A log line reads `KeyError
> 'idempotency_key' while processing job 88213 (queued before the deploy); retrying with
> password=Sup3rS3cretValue`. The metrics system is **down**. **No test report exists.** The code
> lives in Landscape **zone** `refunds` (version 3) and **flow** `refund-flow` (version 2), both
> governed by policy `refund_safety`.

An on-call engineer asks: *"Why did deployment dep-7731 of PAY-142 fail?"*

---

## Part 1. The architecture at a glance

### 1.1 What the component is for

**Definition.** Work Capture is the part of Crane that answers questions about a piece of work by
collecting **evidence** (facts recorded by systems such as Git, CI, a deployment tool, or the
agent-activity log) and packaging it into an **Evidence Pack**: a bounded, authorized,
tamper-evident answer that says three things separately:

1. what is **proven**, and by which evidence;
2. what is **contradicted**, and by which evidence;
3. what is **not known**, and *why* it is not known.

It also produces an **audit record**: a diary of every decision taken while building the pack.

**Why it exists.** When an AI agent finishes a task, people tend to trust "the agent said it is done"
or "CI is green". In the running story CI was green and production still broke. Nobody can say
whether tests ran, because there is no test report. Missing information looks the same as good news,
and that is the problem this component is built to fix.

**How it helps.** Every later component of the Capability Compiler (the registry, the compiler, the
lab, discovery, invocation, and evolution) starts from an Evidence Pack. If the pack is honest, they
can be honest. If the pack hides a gap, every component after it inherits the hidden gap.

### 1.2 The picture

```
                       EvidenceQueryRequest  +  AccessPolicy
                (question, class, known refs, budget)  (who asks, what they may see)
                                        │
 ┌──────────────────────────────────────▼───────────────────────────────────────┐
 │ BROKER  broker/run.rs                                                        │
 │  1 choose a recipe from the RecipeBook ─────────────── broker/recipe.rs      │
 │  2 resolve scope (what is this about, who may use what) resources/scope.rs   │
 │  3 rejection gate (asked for something forbidden? stop)                      │
 │  4 resolve zone and flow versions                                            │
 │  5 for each recipe step:                                                     │
 │      budget → branch → type wanted → type allowed → environment allowed      │
 │      → find resources → call each one:                                       │
 │          fill templates → validate the query → ask the connector             │
 │          → for each returned record: INGEST → LINK → LEARN                   │
 │      → stop early if everything required is present                          │
 │  6 governance linking (files → zones and flows → policies)                   │
 │  7 finish: assertions → missing evidence → selection → context view          │
 │            → contributions → warnings → seal → store                         │
 └──────┬──────────────────┬──────────────────┬───────────────────┬─────────────┘
        │                  │                  │                   │
   RESOURCES           FORMAT            CORRELATE             INGEST
   where evidence      what was said,    how facts connect,    the bounded,
   comes from          stored safely     what follows          authorized answer
   resources/          format/           correlate/            ingest/
        │                  │                  │                   │
        └──────────────────┴─────────┬────────┴───────────────────┘
                                     ▼
                      CaptureStore  store.rs
           <trust directory>/runtime/capture/*.jsonl  (outside the repository)
                                     │
                 ┌───────────────────┴───────────────────┐
                 ▼                                       ▼
           EvidencePack                          QueryAuditRecord
     (consumed by Components 2–7)          (every decision, replayable)
```

### 1.3 The five parts and why they are separate

The header comment of the component lists the four categories and the broker
([mod.rs:1-14](../../src/capture/mod.rs#L1-L14)). Each part answers one question, and each was kept
apart because merging it with another produces a specific failure:

| Part | The question it answers | Files | What would go wrong if merged |
|---|---|---|---|
| **resources** | Where can evidence come from, and may we ask? | [registry.rs](../../src/capture/resources/registry.rs), [connector.rs](../../src/capture/resources/connector.rs), [scope.rs](../../src/capture/resources/scope.rs), [local.rs](../../src/capture/resources/local.rs), [fixture.rs](../../src/capture/resources/fixture.rs) | Permission checks would be scattered through parsing code and some would be skipped |
| **format** | What exactly did the source say, and how is it stored safely? | [artifact.rs](../../src/capture/format/artifact.rs), [record.rs](../../src/capture/format/record.rs), [parsers.rs](../../src/capture/format/parsers.rs) | Raw secrets could reach the graph or the pack before redaction |
| **correlate** | How do these facts connect, and what can be concluded? | [model.rs](../../src/capture/correlate/model.rs), [graph.rs](../../src/capture/correlate/graph.rs), [link.rs](../../src/capture/correlate/link.rs), [assertions.rs](../../src/capture/correlate/assertions.rs) | Observed facts and guesses would be mixed, and nobody could tell which is which |
| **ingest** | What is the bounded, authorized answer? | [selection.rs](../../src/capture/ingest/selection.rs), [pack.rs](../../src/capture/ingest/pack.rs), [contributions.rs](../../src/capture/ingest/contributions.rs), [episode.rs](../../src/capture/ingest/episode.rs) | Relevance ranking could override authorization |
| **broker** | In which order, under which rules, within which budget? | [recipe.rs](../../src/capture/broker/recipe.rs), [run.rs](../../src/capture/broker/run.rs) | Every caller would invent its own retrieval, and nothing would be audited |

Two supporting files complete the picture: the **store** ([store.rs](../../src/capture/store.rs)), which
persists everything, and the **demo** ([demo.rs](../../src/capture/demo.rs)), which runs the story above
from fixtures.

### 1.4 Why a broker, instead of letting an agent query sources directly

**Definition.** A **broker** is a middleman that receives requests and decides, by its own rules,
what to ask of whom.

If an agent queried GitHub, CI, and logs itself:

- its permissions would be whatever its credentials happen to allow;
- nobody would know what it asked, what it skipped, or why;
- a model's choice of where to look would silently decide what the answer contains.

The broker inverts all three. A **recipe** (a fixed, versioned plan) decides what to ask. An
**access policy** decides what is allowed. A **budget** decides how much. The **audit record**
remembers all of it. The broker's own header says it *"is not a pass-through proxy"*
([run.rs:1-11](../../src/capture/broker/run.rs#L1-L11)).

### 1.5 The four rules every part protects

From [mod.rs:11-14](../../src/capture/mod.rs#L11-L14):

| Rule | Statement | Why | Where it is enforced |
|---|---|---|---|
| **R1 Read-only** | Never write to Landscape, DataSea, Crane, or Git | Investigating must never change what is being investigated | Connectors expose no write operations ([connector.rs:256](../../src/capture/resources/connector.rs#L256)) |
| **R2 Unknown stays unknown** | Missing evidence is never `false`, `0`, or "now" | Absence of evidence is not evidence of absence | `Truth` ([mod.rs:62](../../src/capture/mod.rs#L62)), parsers ([parsers.rs:1-6](../../src/capture/format/parsers.rs#L1-L6)) |
| **R3 Lineage** | Every derived claim keeps its sources | A conclusion that cannot be traced cannot be checked | `LearnedFact`, `Derivation`, relation provenance |
| **R4 Nothing is trusted by being said** | Agent text never becomes trusted context or policy by itself | Otherwise agents could rewrite the rules that govern them | Contribution ledger ([contributions.rs:269](../../src/capture/ingest/contributions.rs#L269)) |

### 1.6 One investigation from start to end (the algorithm in twelve steps)

This is the backbone of the chapter. Each step is explained in full later; here they are in order:

1. **Choose a recipe** from a proposed plan, an explicit reference, or the question's class
   ([`select_recipe`, run.rs:565](../../src/capture/broker/run.rs#L565)).
2. **Compute the budget** as the smallest of the request's, the recipe's, and the global ceiling
   ([run.rs:600](../../src/capture/broker/run.rs#L600)).
3. **Resolve scope**: copy known references, mark forbidden things, list permitted sources, list
   unknown facts ([`resolve`, scope.rs:114](../../src/capture/resources/scope.rs#L114)).
4. **Rejection gate**: if the request explicitly named something forbidden, query nothing
   ([run.rs:661](../../src/capture/broker/run.rs#L661)).
5. **Open the work episode** ([`ensure_episode`, store.rs:668](../../src/capture/store.rs#L668)).
6. **Resolve zone and flow versions** through Landscape
   ([`resolve_versions`, run.rs:691](../../src/capture/broker/run.rs#L691)).
7. **Execute every recipe step** with its gates
   ([`execute`, run.rs:757](../../src/capture/broker/run.rs#L757)).
8. **For every returned record**: ingest, link, learn ([`call`, run.rs:921](../../src/capture/broker/run.rs#L921)).
9. **Link governance**: files to zones and flows, zones and flows to policies
   ([run.rs:672](../../src/capture/broker/run.rs#L672)).
10. **Derive assertions** and **compute missing evidence**
    ([run.rs:1244](../../src/capture/broker/run.rs#L1244), [run.rs:1254](../../src/capture/broker/run.rs#L1254)).
11. **Select, view, warn, seal**: eligibility then ranking, a bounded graph view, typed warnings, a
    digest manifest ([run.rs:1319](../../src/capture/broker/run.rs#L1319)–[1451](../../src/capture/broker/run.rs#L1451)).
12. **Store** the pack and the audit, and attach the pack to the episode.

---

## Part 2. Every element, explained

### 2.1 Foundations ([mod.rs](../../src/capture/mod.rs))

These small types are used by every other file.

#### 2.1.1 `SCHEMA_VERSION`, the version of every stored shape
- **Definition.** A number stamped on every stored record, pack, audit, and on the store layout.
- **Code.** [mod.rs:38](../../src/capture/mod.rs#L38).
- **How it works.** Every structure writes `schema_version: SCHEMA_VERSION`. The store refuses a
  layout newer than it understands ([`migrate`, store.rs:810](../../src/capture/store.rs#L810)), and
  Capability intake refuses a pack with another version.
- **Why it exists.** Stored evidence outlives the code that wrote it. Without a version number, a
  future reader cannot know which shape it is reading.
- **How it helps.** Upgrades fail loudly instead of misreading old data.
- **What breaks.** There is only one version (1). Migration from "0" is a no-op, so no real
  migration path has been exercised yet.
- **Fix and criticality.** Write a migration test when version 2 lands. **LOW.**
- **Resolved (2026-10-21).** The test `older_store_layout_still_opens`
  ([tests.rs:4492](../../src/capture/tests.rs#L4492)) opens a store written before the new fields
  existed (no recipe pins, page counts, withheld counts, outcome history, or later episodes) and
  checks that they read as their defaults. Every field the fixes added is `#[serde(default)]`, so
  `SCHEMA_VERSION` stays 1. **New criticality: NONE** until version 2 lands.

#### 2.1.2 `stable_id`, identifiers computed from content
- **Definition.** An **identifier** is a short string naming one object. A **stable identifier** is
  computed by hashing the parts that define the object, with a type prefix: `evd_` evidence, `art_`
  artifact, `ent_` entity, `rel_` relation, `asr_` assertion, `ctb_` contribution, `epi_` episode,
  `qry_` query, `pck_` pack, `ctx_` context view.
- **Code.** [mod.rs:50](../../src/capture/mod.rs#L50), under the digest domain `capture/id/v1`
  ([mod.rs:41](../../src/capture/mod.rs#L41)).
- **How it works.** `stable_id("evd", [resource, record, revision, hash])` produces the same output
  for the same inputs, every time, on every machine.
- **Why it exists.** With random IDs, "is this the same thing we saw before?" requires a database
  lookup. With content IDs, it is just a computation.
- **How it helps.** Duplicates are recognized for free, replays compare by ID equality, and records
  from many machines merge without conflicts.
- **What breaks.** The query ID includes the *number of audits already stored*
  ([run.rs:600](../../src/capture/broker/run.rs#L600)). Two processes running at once can count the
  same number, and if they share the actor, question, and millisecond, they produce the same ID. That
  is rare, but it combines with the store race (§2.13.3).
- **Fix and criticality.** Add a random nonce to query IDs (they need not be content-stable).
  **LOW.**
- **Resolved (2026-10-21).** The query id now includes `Clock::nonce()`
  ([mod.rs:402](../../src/capture/mod.rs#L402)): eight random bytes under the real clock, and nothing
  under a fixed clock, so the demo and the tests stay byte-identical. Two processes can no longer
  produce the same id, and because saves now merge (§2.13.3) no audit is lost either way. **New
  criticality: NONE.**

#### 2.1.3 `Truth`, three values instead of two
- **Definition.** `Yes`, `No`, or `Unknown`. `Unknown` means "not established either way".
- **Code.** [mod.rs:62](../../src/capture/mod.rs#L62); [`from_option`, mod.rs:76](../../src/capture/mod.rs#L76)
  turns a missing value into `Unknown`.
- **How it works.** Every conclusion and every feedback answer uses `Truth` instead of `bool`.
- **Why it exists.** With a boolean, "no test report" must become `true` or `false`, and both are
  lies.
- **How it helps.** Later components (Capability gates, Discovery) refuse to treat `Unknown` as
  success. This one type carries Rule R2 through the entire pipeline.
- **What breaks.** Nothing. [`Assertion::truth`](../../src/capture/correlate/assertions.rs#L152) falls back
  to `Unknown` when the stored value cannot be read, which is the safe direction.
- **Fix and criticality.** **NONE.**

#### 2.1.4 `Actor` and `ActorKind`, who is acting
- **Definition.** An **actor** is whoever asks, proposes, or reviews. Its **kind** is `Human`,
  `Agent`, or `System`.
- **Code.** [mod.rs:185](../../src/capture/mod.rs#L185), [mod.rs:197](../../src/capture/mod.rs#L197); the
  system actor is [mod.rs:223](../../src/capture/mod.rs#L223).
- **How it works.** Rules check the kind: only humans review, agents propose only in the
  agent-proposed namespace, and the system authors deterministic conclusions.
- **Why it exists.** Trust differs by kind, so it must be a typed field and not a name.
- **How it helps.** "Who said this?" is checkable by code.
- **What breaks.** The kind is whatever the caller says. The CLI marks whoever runs it as `Human`
  ([cli/capture.rs:300-312](../../src/cli/capture.rs#L300-L312)). The guarantee that an agent cannot claim
  to be a human comes from Crane refusing to run inside agent sessions, not from this type.
- **Fix and criticality.** Derive the actor kind from Crane's signed identity (owner or admin keys)
  when the component is exposed beyond the CLI. **MEDIUM** (latent, but real once the API is
  reachable by agents).
- **Resolved (2026-10-21).** The CLI no longer builds a human actor from a name.
  It calls `Actor::human_operator` ([mod.rs:236](../../src/capture/mod.rs#L236)), which refuses
  whenever `trust::agent_environment()` detects an AI agent session, so no human actor can exist
  inside one. Review rights are no longer granted to whoever runs the command:
  [`may_review`, cli/capture.rs:87](../../src/cli/capture.rs#L87) is true only when this machine's
  signing key is the repository owner's or a signed admin's, the same authority that may change
  the rules. **New criticality: LOW.** `Actor::new` is still callable inside the crate, so a future
  network API must authenticate its callers the same way.

#### 2.1.5 `Sensitivity`, how secret something is
- **Definition.** An ordered scale: `Public < Internal < Confidential < Secret`. `Confidential` means
  a secret was found and removed. `Secret` content is never stored.
- **Code.** [mod.rs:237](../../src/capture/mod.rs#L237).
- **How it works.** Each artifact gets a level when captured (§2.6.3). A record is withheld from an
  actor when `record.sensitivity > policy.max_sensitivity`
  ([selection.rs:144](../../src/capture/ingest/selection.rs#L144)).
- **Why it exists.** An ordered scale turns "may they see this?" into a single comparison.
- **How it helps.** One field per actor covers every record.
- **What breaks.** The level depends on the redactor recognizing the secret. A secret in an
  unfamiliar format stays `Internal`.
- **Fix and criticality.** Let sources declare sensitivity for whole evidence types (for example,
  every runtime log is at least Confidential). **MEDIUM.**
- **Partly resolved (2026-10-21).** Each evidence type now has a sensitivity floor,
  `EvidenceType::sensitivity_floor` ([record.rs:117](../../src/capture/format/record.rs#L117)).
  Runtime logs and tool calls are at least Confidential whatever the redactor found, and `classify`
  takes the highest of the redaction level, the source's declaration, and that floor. A secret in
  an unfamiliar format inside a log therefore no longer reaches an actor whose ceiling is
  Internal. What remains: other evidence types still depend on the redactor, and there is no
  entropy-based secret detection. **New criticality: LOW.**

#### 2.1.6 `AccessPolicy`, what one actor may touch
- **Definition.** The list of resources, repositories, environments, evidence types, and maximum
  sensitivity one actor may use, plus whether they may review contributions.
- **Code.** [mod.rs:257](../../src/capture/mod.rs#L257), with four checks:
  [`permits_resource`](../../src/capture/mod.rs#L274), [`permits_repository`](../../src/capture/mod.rs#L284),
  [`permits_environment`](../../src/capture/mod.rs#L295), [`permits_type`](../../src/capture/mod.rs#L305).
- **How it works.** The checks run at four points: scope resolution, every step, every returned record,
  and pack selection. `permits_repository(None)` is **false**, because an unknown repository cannot be
  checked. `permits_environment(None)` is **true**, because evidence without an environment (a policy
  file) is not environment-restricted.
- **Why it exists.** The comment states it: enforced *"at execution time, never by instructions given
  to a model."*
- **How it helps.** Permission is code, not a polite request.
- **What breaks.** Two things:
  1. The first record that *reveals* an environment is fetched while the environment is still
     unknown, so `permits_environment(None) == true` lets it through.
  2. Two consumers, scope learning and assertion derivation, use that record before the final
     authorization filter runs.

  Together these leak forbidden facts into the pack (worked through in Part 5, Example 3).
- **Fix and criticality.** Run full eligibility on each record *when it is ingested* inside
  [`call`](../../src/capture/broker/run.rs#L921), before it is learned from or collected. **CRITICAL.**
- **Resolved (2026-10-21).** Authorization now runs on each record as it arrives.
  Inside [`call`](../../src/capture/broker/run.rs#L1207), right after ingestion and before linking,
  learning, or collecting, the broker applies the same check that selection uses, `withheld`
  ([selection.rs:146](../../src/capture/ingest/selection.rs#L146)).
  1. A withheld record stays in the store, but it is never learned from, never collected, and never
     used for assertions. The Access warning counts withheld records without naming their ids.
  2. If the record reveals a repository or environment the actor may not access
     (`reveals_forbidden_scope`, [selection.rs:166](../../src/capture/ingest/selection.rs#L166)), the
     investigation stops as `Rejected` (`stop_on_breach`). Rejecting is safer than continuing,
     because even "your later steps were denied" would reveal that the scope moved.
  3. The context view shows only the roots and the entities observed by evidence the actor may
     see. Relation provenance and learned facts are filtered to that evidence.

  A class-guard test, `assert_pack_is_closed`, checks that every evidence id the pack mentions
  anywhere is in the pack. **New criticality: NONE.**

#### 2.1.7 `Clock`, deterministic time
- **Definition.** Either the real clock, or a **fixed clock** that starts at a given time and
  advances one millisecond per reading.
- **Code.** [mod.rs:318](../../src/capture/mod.rs#L318), [`now`, mod.rs:351](../../src/capture/mod.rs#L351).
- **Why it exists.** The demo and tests must produce byte-identical output
  ([tests.rs:1757-1760](../../src/capture/tests.rs#L1757-L1760)).
- **How it helps.** A behavioral change shows up as a diff.
- **What breaks.** It is built on `Cell`, which cannot be shared between threads, so the broker
  cannot run steps in parallel as written.
- **Fix and criticality.** Use an atomic counter. **LOW** today, a blocker for scaling.
- **Resolved (2026-10-21).** The fixed clock is an `AtomicU64`
  ([mod.rs:339](../../src/capture/mod.rs#L339)) with a hand-written `Clone`, so a clock can be shared
  between threads. The demo output is still byte-identical. **New criticality: NONE.** Running steps
  in parallel is still future work, but the clock no longer blocks it.

#### 2.1.8 `ErrorCode` and `CaptureError`, the vocabulary of "no"
- **Definition.** A closed list of 13 refusal reasons plus a human message.
- **Code.** [mod.rs:103](../../src/capture/mod.rs#L103), [mod.rs:125](../../src/capture/mod.rs#L125).
- **Why it exists.** Callers can react to the *kind* of refusal without parsing text.
- **What breaks.** Some refusals are returned as errors and never audited: "no recipe for this
  class" ([run.rs:565](../../src/capture/broker/run.rs#L565)) and an invalid proposed plan
  ([recipe.rs:792](../../src/capture/broker/recipe.rs#L792)). Nobody can later see that someone asked
  and was refused.
- **Fix and criticality.** Write a `Rejected` audit record before returning. **MEDIUM.**
- **Resolved (2026-10-21).** `audit_refusal` ([run.rs:620](../../src/capture/broker/run.rs#L620))
  stores a `Rejected` audit, with no recipe, the error, and no pack, before `investigate` or
  `replay` returns the error. Callers still receive the same error. A refused request is now
  visible afterwards. **New criticality: NONE.**

#### 2.1.9 `Metrics`, the component's own health counters
- **Definition.** Seventeen counters per investigation: connector calls, failures, latency, records
  retrieved and normalized, duplicates, malformed records, parse failures, correlation failures,
  branches taken and skipped, traversal depth, pack size and build time, missing requirements, budget
  hits, authorization denials.
- **Code.** [mod.rs:385](../../src/capture/mod.rs#L385).
- **Why it exists.** The comment notes *"DataSea does not observe Work Capture's retrieval by
  itself"*, so the component counts itself.
- **What breaks.** The counters live only inside the audit record. They are never emitted as DataSea
  events, so the dashboard cannot chart them.
- **Fix and criticality.** Emit one summary event per investigation. **LOW.**
- **Partly resolved (2026-10-21).** Each capture command attaches the investigation's summary
  and metrics to its own `command.executed` event
  ([`note_investigation`, cli/capture.rs:135](../../src/cli/capture.rs#L135)), so DataSea receives them
  as the command's own fact. Work Capture itself still writes nothing to DataSea (Rule R1). What
  remains: no dashboard flow charts them yet. **New criticality: LOW.**

### 2.2 Resources: where evidence comes from ([resources/](../../src/capture/resources))

#### 2.2.1 Resource, resource type, connector
- **Definition.** A **resource** (an evidence source) is any system that records facts about work. A
  **resource type** is its category: `Tracker, Github, Git, Ci, Deployment, AgentExecution, Logs,
  Metrics, InternalApi, Landscape, Crane`. A **connector** is the code that knows how to talk to one
  kind of source.
- **Code.** [`ResourceType`, registry.rs:31](../../src/capture/resources/registry.rs#L31).
- **Why it exists.** Recipes ask for a *type* ("a CI source"), not a product ("GitHub Actions"), so
  the same recipe works with any CI.
- **What breaks.** Nothing in the type itself. **NONE.**

#### 2.2.2 `ResourceDefinition`, the declaration of one source
- **Definition.** A record saying "there is a source called X, of type Y, served by connector Z, able
  to answer these questions, about these repositories."
- **Code.** [registry.rs:120](../../src/capture/resources/registry.rs#L120). Its parts:

| Field | Holds | Why it exists |
|---|---|---|
| `resource_id` | `"ci"` | the name used everywhere else |
| `resource_type` | `Ci` | lets recipes match it ([run.rs:839](../../src/capture/broker/run.rs#L839)) |
| `connector_type` | `"fixture"`, `"git"`, … | which compiled-in code serves it |
| `capabilities` ([registry.rs:84](../../src/capture/resources/registry.rs#L84)) | operations, evidence types, filter keys | the only questions it may be asked |
| `scope` ([registry.rs:98](../../src/capture/resources/registry.rs#L98)) | repositories, projects, environments, resources | where it may look |
| `auth_reference` | `"env:CI_TOKEN"` | where a credential is found, never the credential itself |
| `configuration`, `configuration_version` | small settings and their version | old evidence can say which configuration produced it |
| `status` ([registry.rs:53](../../src/capture/resources/registry.rs#L53)) | `Active`, `Disabled`, `Unhealthy` | an inactive source is never asked |

#### 2.2.3 `CONNECTORS`, the list of connector types compiled into Crane
- **Definition.** Five fixed connector descriptions (`fixture`, `git`, `datasea`, `landscape`,
  `crane`). Each says which resource types it can serve, which configuration keys it accepts, and
  which filters it supports.
- **Code.** [`ConnectorSpec`, registry.rs:142](../../src/capture/resources/registry.rs#L142),
  [`CONNECTORS`, registry.rs:150](../../src/capture/resources/registry.rs#L150).
- **Why it exists.** If registration could name a script, a command, or a URL template, then
  registering a source would be the same as running arbitrary code. Only code compiled into Crane can
  serve a source.
- **How it helps.** Adding a source instance is safe configuration. Adding a new *kind* of source is a
  reviewed code change.
- **What breaks.** Every new integration (GitHub, CI, deployments) needs a Crane release.
- **Fix and criticality.** That is intended. Add the missing hosted connectors as code. **LOW**
  (design cost, not a defect).

#### 2.2.4 `validate` and `register`, the registration algorithm
- **Code.** [`validate`, registry.rs:247](../../src/capture/resources/registry.rs#L247),
  [`register`, registry.rs:365](../../src/capture/resources/registry.rs#L365).
- **How it works.** The checks run in this order, and the first failure refuses registration:
  1. The id is lowercase, starts with a letter, and is at most 64 characters
     ([`valid_identifier`, registry.rs:219](../../src/capture/resources/registry.rs#L219)).
  2. The connector type is one of the five.
  3. That connector can serve that resource type.
  4. At least one operation and one evidence type are declared.
  5. Every declared filter is supported by the connector.
  6. Every configuration key is accepted by the connector, each value is at most 512 characters,
     and no value looks like a secret. If one does, the error says "use auth_reference".
  7. The `auth_reference` is `env:`, `keychain:`, or `integration:` followed by a safe name, and is
     not itself a secret.
  8. The scope has at least one non-empty list, with no empty items and no `*` or `?`.
  9. In `register`: if the id already exists and the configuration changed, the
     `configuration_version` must be higher.
- **Why it exists.** Each check closes one hole: unsafe names, unknown code, impossible
  combinations, empty capability claims, unsupported questions, secrets in configuration, secrets
  disguised as references, unbounded scope, and silently drifting configuration.
- **What breaks.**
  1. `auth_reference` is validated but **never resolved**. No connector reads a credential through
     it yet.
  2. The CLI rebuilds the registry on every run with version 1, so the configuration-version rule
     has no effect there.
- **Fix and criticality.** Implement credential resolution with the first hosted connector, and
  persist the registry. **LOW.**

#### 2.2.5 `ResourceStatus`, the health of a source
- **Definition.** `Active`, `Disabled`, or `Unhealthy`.
- **Code.** [registry.rs:53](../../src/capture/resources/registry.rs#L53), changed by
  [`set_status`, registry.rs:410](../../src/capture/resources/registry.rs#L410).
- **How it works.** Scope resolution refuses any resource that is not `Active`
  ([scope.rs:114](../../src/capture/resources/scope.rs#L114)).
- **What breaks.** Nothing ever sets `Unhealthy`. `set_status` is only used by tests, and the broker
  never calls `health_check`. Only the `crane capture sources` command calls it
  ([cli/capture.rs:238](../../src/cli/capture.rs#L238)). So a broken source is asked anyway, fails, and
  costs budget.
- **Fix and criticality.** Optionally probe health before an investigation and mark sources
  `Unhealthy` with a reason. **LOW.**
- **Resolved (2026-10-21).** Instead of probing every source up front, which would
  double the cost of slow sources, the broker remembers which sources failed during this
  investigation. A source that did not answer is not asked again by later steps; such a step is
  recorded `Failed` with the detail "the source did not answer earlier in this investigation and was
  not asked again" ([run.rs:1108](../../src/capture/broker/run.rs#L1108)). `crane capture sources`
  still shows each source's health on demand. **New criticality: NONE.**

#### 2.2.6 The connector contract (`ResourceConnector`)
- **Definition.** A **contract** (in Rust, a *trait*) is a list of functions every connector must
  provide: `discover`, `fetch`, `query`, `get_metadata`, `health_check`.
- **Code.** [connector.rs:256](../../src/capture/resources/connector.rs#L256).
- **How it works.** `fetch` gets one object by id. `query` searches by exact filters.
  `get_metadata` is fetch with the content blanked ([connector.rs:288](../../src/capture/resources/connector.rs#L288)).
  `discover` lists objects without content.
- **Why it exists.** The broker can treat Git, DataSea, and a fixture identically.
- **What breaks.** There are no write functions, by design (Rule R1). The broker uses only `fetch`
  and `query`. **NONE.**

#### 2.2.7 `ConnectorStatus`, the label stamped on every source answer
- **Definition.** A **state** is a named condition. This one is an **outcome label**: each call's
  answer gets exactly one of `Ok`, `Partial`, `Unavailable`, `Unauthorized`, `RateLimited`,
  `InvalidQuery`, `Timeout`, `Failed`.
- **Code.** [connector.rs:34](../../src/capture/resources/connector.rs#L34); only `Ok` and `Partial`
  are usable ([`usable`, connector.rs:52](../../src/capture/resources/connector.rs#L52)).
- **Why it exists.** "Down", "not allowed", "bad question", and "nothing exists" each need a
  different human action. The contract's promise is that a failure is never reported as an empty
  success ([connector.rs:1-6](../../src/capture/resources/connector.rs#L1-L6)).
- **How it helps.** The difference survives all the way to the reason on each missing item (§2.9.2).
- **What breaks.** A `Partial` answer with zero records becomes `Empty` in the final report
  ([run.rs:1485](../../src/capture/broker/run.rs#L1485)), which reads as "nothing exists" when the truth
  is "the source could not fully answer". This happens with Crane's validation (Part 5, Example 2).
- **Fix and criticality.** Map `Partial` with zero records to `Unavailable`. **MEDIUM.**
- **Resolved (2026-10-21).** [`source_report`](../../src/capture/broker/run.rs#L1963) now maps an
  executed step whose source answered `Partial` with zero records to `Unavailable`, with the
  source's error as the detail. A step whose records were all withheld from the actor maps to
  `Denied`. No new `SourceState` was added, so stored packs and capability intake read the same
  values as before. **New criticality: NONE.**

#### 2.2.8 `SourceRecord` and `ConnectorOutcome`
- **Definition.** A **source record** is one raw fact in the source's own shape: id, evidence type,
  media type, JSON content, source timestamp, source revision, and metadata used for filtering. An
  **outcome** is status, records, next cursor, snapshot, and error.
- **Code.** [connector.rs:180](../../src/capture/resources/connector.rs#L180),
  [connector.rs:200](../../src/capture/resources/connector.rs#L200).
- **What breaks.** Filtering matches `metadata` exactly. A connector that forgets to copy a field
  into metadata cannot be filtered on it, and returns `Empty` instead of failing.
- **Fix and criticality.** Add a contract test per connector that checks every declared filter key
  is set in metadata. **LOW.**
- **Resolved (2026-10-21).** A contract harness, `assert_connector_contract`
  ([tests.rs:4390](../../src/capture/tests.rs#L4390)), runs against the fixture, DataSea, and GitHub
  connectors. It checks that an answer with records names its snapshot, that a failure is never an
  empty success, that `get_metadata` strips content, and that filtering by a declared key returns
  only records carrying that value. **New criticality: NONE.**

#### 2.2.9 `QuerySpec` and its validation
- **Definition.** A **query specification** is the exact search sent to a connector: evidence type,
  exact filters, time window, page limit, cursor, and scope (repository and environment).
- **Code.** [connector.rs:80](../../src/capture/resources/connector.rs#L80),
  [`validate`, connector.rs:100](../../src/capture/resources/connector.rs#L100).
- **How it works.** The checks run in order: the resource supports queries; it provides this type;
  `1 ≤ limit ≤ 200` ([`MAX_PAGE`](../../src/capture/resources/connector.rs#L19)); every filter key is
  declared and every value is one exact value (non-empty, no `*`/`?`, at most 256 characters); the
  window does not end before it starts; a repository-scoped resource receives a repository inside its
  scope; and an environment, if given, is inside the resource's environments.
- **Why it exists.** A connector must never receive a broader question than its definition allows.
- **What breaks.** Nothing in the checks themselves. **NONE.**

#### 2.2.10 `page`, cursors, and the time window
- **Definition.** A **page** is a slice of results. A **cursor** is a marker meaning "continue from
  here".
- **Code.** [`page`, connector.rs:313](../../src/capture/resources/connector.rs#L313).
- **How it works.**
  1. Read the cursor as an offset (a malformed cursor gives `InvalidQuery`).
  2. Keep records of the requested type.
  3. Keep records whose metadata equals every filter.
  4. If a time window is set, keep only records **with a timestamp inside it**
     ([connector.rs:325-331](../../src/capture/resources/connector.rs#L325-L331)).
  5. Skip `offset` records, take `limit`.
  6. Set `next_cursor` if more remain.
- **What breaks.** Two defects:
  1. **The time window drops timeless evidence.** Zones, flows, policies, and the validation report
     have no timestamp ([local.rs:398](../../src/capture/resources/local.rs#L398),
     [local.rs:530](../../src/capture/resources/local.rs#L530)). Any investigation with a time window
     therefore gets **no governance evidence**, reported as `Empty`.
  2. **Cursors are never followed.** The broker always sends `cursor: None` and only stores
     `next_cursor` ([run.rs:1060](../../src/capture/broker/run.rs#L1060)), so results past the step
     limit are dropped without a warning.
- **Fix and criticality.**
  1. Exempt timeless evidence types from the window. **HIGH.**
  2. Follow cursors while the record budget allows, otherwise emit a `Truncation` warning. **HIGH.**
- **Resolved (2026-10-21).** 
  1. Whether a type is tied to time is now one definition, `EvidenceType::is_timed`
     ([record.rs:102](../../src/capture/format/record.rs#L102)). [`page`](../../src/capture/resources/connector.rs#L348)
     applies the time window only to timed records, and the broker sends no window for timeless
     types, so zones, flows, policies, and validation survive a time window.
  2. Query steps follow cursors. `call` passes each `next_cursor` back while the step's and the
     investigation's budgets allow, combines the pages into one `QueryStepRecord` with a `pages`
     count (so replay keys stay stable), and adds a Truncation warning naming the step when it stops
     with a cursor still pending.

  **New criticality: NONE** for both.

#### 2.2.11 Snapshots
- **Definition.** A **snapshot** is a fingerprint of the source's state at the moment it answered.
- **Code.** [`snapshot_of`, connector.rs:368](../../src/capture/resources/connector.rs#L368). Git uses
  `git:<HEAD>`, DataSea its last event digest, Landscape the hash of `governance.yaml`.
- **Why it exists.** A **replay** (§2.12) compares snapshots to tell whether a source changed.
- **What breaks.** Snapshots cover the *whole* source. Any unrelated change (a new commit, a new
  event) makes a replay `Diverged` even when the records that matter are identical.
- **Fix and criticality.** Compare the returned evidence ids first, and treat a snapshot difference
  as "may have changed". **LOW.**
- **Resolved (2026-10-21).** Replay compares the returned evidence ids as sets. When they
  are equal but the snapshot changed, the difference says "(the source changed, but the records this
  step returned are the same)" ([`compare`, run.rs:2307](../../src/capture/broker/run.rs#L2307)), so a
  reader can tell an unrelated change from a different answer. The verdict stays `Diverged`,
  because the source's state did change. **New criticality: NONE.**

#### 2.2.12 The fixture connector
- **Definition.** A stand-in source that serves a fixed list of records and can be told to fail in
  each structured way.
- **Code.** [fixture.rs](../../src/capture/resources/fixture.rs), behaviors at
  [fixture.rs:22](../../src/capture/resources/fixture.rs#L22), applied at
  [fixture.rs:95](../../src/capture/resources/fixture.rs#L95).
- **Why it exists.** GitHub, CI, deployments, logs, and metrics have no live connector yet. Fixtures
  exercise every branch and failure deterministically, and their snapshots start with `fixture:` so
  they never pass for live data.
- **What breaks.** Confidence in the hosted-source code paths comes from fixtures only.
- **Fix and criticality.** Build live connectors for GitHub (pull requests, checks). **MEDIUM**
  (a product gap, not a defect).
- **Partly resolved (2026-10-21).** A live GitHub connector,
  [github.rs](../../src/capture/resources/github.rs), reads pull requests (PullRequest), a commit's
  files (CodeChange), and check runs (CiRun) through the GitHub CLI (`gh api`, or the program named
  by `CRANE_GH`). It uses the CLI's own login, so no credential is stored.
  [`classify`, github.rs:100](../../src/capture/resources/github.rs#L100) maps each answer to a
  connector status: 401 and 403 to Unauthorized, 404 to Ok with zero records, 429 to RateLimited,
  and 5xx to Unavailable. The CLI registers it when the repository is connected to GitHub
  (`GitHubConnector::for_remote`). Its tests use recorded responses, not the network. What remains:
  deployments, logs, and metrics still have no live connector, and GitHub has no test-report API.
  **New criticality: LOW** (a product gap).

#### 2.2.13 The live connectors ([local.rs](../../src/capture/resources/local.rs))
- **Git** ([local.rs:68](../../src/capture/resources/local.rs#L68)).
  - `safe_revision` ([local.rs:53](../../src/capture/resources/local.rs#L53)) refuses a leading `-`,
    which blocks flag injection, and refuses `..` ranges.
  - An unknown commit returns `Ok` with zero records, which honestly means "no such commit".
  - Diffs are cut to 8 KB.
  - **Breaks:** `query` refuses cursors outright. **LOW.**
  - **Resolved (2026-10-21).** Git queries page with `git log --skip`, and every Git command runs
    through `run_git` with the investigation's deadline ([local.rs:89](../../src/capture/resources/local.rs#L89)),
    answering `Timeout` when it passes. **NONE.**
- **DataSea** ([local.rs:252](../../src/capture/resources/local.rs#L252)).
  - Classifies events as `ToolCall`, `AgentSession`, or `AgentEvent` by name.
  - Filters on Crane's **canonical task id** `foxx_task_id`
    ([local.rs:283](../../src/capture/resources/local.rs#L283)), which is separate from the tracker key
    `external_task_id` ([events.rs:58-59](../../src/telemetry/events.rs#L58-L59)).
  - **Breaks:**
    1. Asking by a tracker key (`PAY-142`) finds nothing, even when the agent did the work. **HIGH.**
    2. It re-reads the whole event log on every call ([local.rs:264](../../src/capture/resources/local.rs#L264)).
       **MEDIUM** at scale.
  - **Resolved (2026-10-21), item 1.** Each event now also carries `external_task`, and a `task`
    query matches either Crane's id or the tracker key ([local.rs:535](../../src/capture/resources/local.rs#L535)).
    The linker accepts either one, so `PAY-142` finds the agent's work and connects its sessions to
    the episode. **NONE.**
  - **Partly resolved (2026-10-21), item 2.** The event log is read once per investigation (a cache
    that `begin()` clears when the next investigation starts) instead of once per call. It is not
    indexed. **LOW.**
- **Landscape** ([local.rs:386](../../src/capture/resources/local.rs#L386)).
  - Each zone and flow becomes a `GraphState` record `zone:<id>` or `flow:<id>` with revision
    `v<version>`.
  - **Breaks:** no timestamps, which triggers defect 1 of §2.2.10.
  - **Resolved (2026-10-21)** by the §2.2.10 fix: timeless types ignore the time window. **NONE.**
- **Crane** ([local.rs:518](../../src/capture/resources/local.rs#L518)).
  - Policies become `Policy` records, and the verifier result becomes one `Validation` record.
  - If verification cannot run, the answer is `Partial` ([local.rs:605](../../src/capture/resources/local.rs#L605)).
  - **Breaks:** every `fetch` and `query` runs a full verification
    ([local.rs:594](../../src/capture/resources/local.rs#L594)), so verification runs twice per
    `local-change` investigation, and the broker's time budget cannot interrupt it. **MEDIUM.**
  - **Partly resolved (2026-10-21).** Verification runs once per investigation: its result is
    cached, and `begin()` clears the cache when the next investigation starts, so a replay after a
    change recomputes it. It still cannot be interrupted by the time budget. **LOW.**

### 2.3 Recipes and the RecipeBook ([broker/recipe.rs](../../src/capture/broker/recipe.rs))

#### 2.3.1 `EvidenceRecipe`, a fixed plan for one kind of question
- **Definition.** A **recipe** is a written, versioned, unchangeable plan for one class of
  investigation. It lists:
  - the **steps** to ask;
  - the evidence that is **required** for the answer to count as sufficient;
  - when to **stop**;
  - how much it may **spend**.
- **Code.** [recipe.rs:270](../../src/capture/broker/recipe.rs#L270). Fields: `recipe_id`, `version`,
  `description`, `investigation_classes`, `starting_conditions`, `steps`, `required`, `stopping`,
  `limits`.
- **Why it exists.** The plan, not the person or model asking, decides what to retrieve. Two people
  asking the same kind of question get the same retrieval, so their packs can be compared.
- **How it helps.** Reproducible, reviewable investigations ("what does a deployment-failure
  investigation look at?" has one answer).
- **What breaks.** Only two recipes exist: `failed-deployment`
  ([recipe.rs:554](../../src/capture/broker/recipe.rs#L554)) and `local-change`
  ([recipe.rs:703](../../src/capture/broker/recipe.rs#L703)). Every other question needs a proposed plan
  (§2.3.10).
- **Fix and criticality.** Add recipes for common classes: discovery ("which deployments happened in
  this window?"), PR review, and incident. **MEDIUM.**
- **Resolved (2026-10-21).** Two recipes were added. `deployment-discovery` v1
  ([recipe.rs:906](../../src/capture/broker/recipe.rs#L906)), class `deployment-discovery`, queries the
  deployments of an environment within the window. `pr-review` v1
  ([recipe.rs:960](../../src/capture/broker/recipe.rs#L960)), class `pull-request-review`, reads the pull
  request, its commits, CI, tests, policies, validation, and zones. `failed-deployment` v2 was also
  published (§2.3.4). A test pins each recipe's digest. No incident recipe exists yet. **New
  criticality: LOW.**
- **Also unused.** `starting_conditions` is validated ([recipe.rs:336](../../src/capture/broker/recipe.rs#L336))
  but never checked at run time. A recipe that expects a deployment runs without one. **LOW.**
  **Resolved (2026-10-21):** a starting condition that is unknown when the recipe begins gets a
  Completeness warning naming it. The recipe still runs, because the gaps it leaves are reported
  anyway. **NONE.**

#### 2.3.2 `RecipeStep`, one request inside a recipe
- **Definition.** A step names a resource *type*, an operation (`Fetch` or `Query` only), an evidence
  type, a record template (for Fetch) or filter templates (for Query), a result limit, and a
  condition `when`.
- **Code.** [recipe.rs:177](../../src/capture/broker/recipe.rs#L177); built by the helper
  [`step`, recipe.rs:488](../../src/capture/broker/recipe.rs#L488).
- **Why it exists.** Naming a *type* keeps recipes portable. Restricting operations to two keeps
  recipes read-only and simple.
- **What breaks.** Every permitted resource of the named type is asked
  ([run.rs:880](../../src/capture/broker/run.rs#L880)). Two CI sources means double the budget and,
  when their bytes differ, double-counted support (§2.8.2).
- **Fix and criticality.** Allow a step to pick one resource by priority. **LOW.**
- **Resolved (2026-10-21).** `ResourceDefinition` has a `priority` (default 0, so stored
  definitions read as before). When the resources of a step's type have different priorities, the
  broker asks the best one first, and the next one only when the better one did not answer.
  Resources of equal priority are all asked, as before ([run.rs:1098](../../src/capture/broker/run.rs#L1098)).
  **New criticality: NONE.**

#### 2.3.3 Templates, `VARIABLES`, `check_template`, `fill`
- **Definition.** A **template** is either a literal (`"policy"`) or **exactly one** variable in
  braces (`"{commit}"`). There are eight variables
  ([recipe.rs:22](../../src/capture/broker/recipe.rs#L22)): `task, repository, branch, commit,
  pull_request, deployment, environment, session`.
- **Code.** [`check_template`, recipe.rs:289](../../src/capture/broker/recipe.rs#L289),
  [`fill`, recipe.rs:317](../../src/capture/broker/recipe.rs#L317).
- **How it works.** `check_template` refuses unknown variables, mixtures like `"PR-{pull_request}"`,
  empty strings, and `*`. `fill` substitutes the known value, or returns the name of the missing
  variable, and the broker then marks the step *not queried* ([run.rs:951](../../src/capture/broker/run.rs#L951)).
- **Why it exists.** String building is how queries get widened, by accident or on purpose.
- **How it helps.** An unknown fact can never become "anything".
- **What breaks.** List-valued facts use only the **first** value
  ([`variable`, scope.rs:249](../../src/capture/resources/scope.rs#L249)). For a pull request with five
  commits, CI and tests are checked for one commit, with **no warning**.
- **Fix and criticality.** Run the step once per value, or warn when more than one value exists.
  **HIGH.**
- **Partly resolved (2026-10-21).** The warning was implemented, as approved; running a step
  once per value was not. When a step uses a list-valued variable that has several values, the
  broker records which value was used and which were not (`note_unchecked`), and the pack gets a
  Completeness warning naming the values that were not checked. Every distinct commit is now
  learned (§2.4.4). The step still runs only for the first value. **New criticality: LOW**: the pack
  can still miss a commit, but it says so.

#### 2.3.4 `Predicate`, the condition of a step (a "branch")
- **Definition.** A **predicate** is a yes/no question about the evidence collected so far. A
  **branch** is a step that runs only when its predicate holds. The forms are: `Always`, `Exists{type}`,
  `Missing{type}`, `FieldEquals{type, field, value}`, `ScopeKnown{variable}`, and the combinators
  `All`, `Any`, `Not`.
- **Code.** [recipe.rs:60](../../src/capture/broker/recipe.rs#L60),
  [`holds`, recipe.rs:95](../../src/capture/broker/recipe.rs#L95), validation (depth at most 8, known
  variables) at [recipe.rs:128](../../src/capture/broker/recipe.rs#L128).
- **Why it exists.** Investigations depend on what has been found so far: fetch crash logs only if
  the deployment failed. Every decision is recorded as a `BranchDecision`
  ([run.rs:149](../../src/capture/broker/run.rs#L149)).
- **What breaks.**
  1. `FieldEquals` checks *any* collected record of the type, not the deployment under
     investigation. With two deployments collected, one failed deployment triggers logs for both.
  2. A branch trusts a single declared field. In the story, the deployment says `migration: false`
     while the commit adds `migrations/0042_refund_keys.sql`, so schema evidence is skipped. No
     predicate can look at file paths.
- **Fix and criticality.** Bind predicates to the scope subject, and add an `AnyFileMatches{glob}`
  predicate. **HIGH.**
- **Resolved (2026-10-21).** `FieldEquals` gained an optional `subject` (such as
  `{deployment}`), so only records about that deployment count. A new predicate,
  `AnyFileMatches{glob}` ([recipe.rs:82](../../src/capture/broker/recipe.rs#L82)), looks at the files a
  CodeChange touched; recipe validation refuses an empty glob. `failed-deployment` **v2**
  ([recipe.rs:858](../../src/capture/broker/recipe.rs#L858)) binds its conditions to the deployment
  under investigation, and runs the schema step when the change touches `migrations/`. **v1 was not
  edited**: its digest is pinned, and the demo still pins v1, so its output did not change.
  `for_class("deployment-failure")` now picks v2. **New criticality: NONE.**

#### 2.3.5 `RequiredEvidence`, what "complete" means
- **Definition.** An evidence type that must be present, optionally only when a predicate holds.
- **Code.** [recipe.rs:158](../../src/capture/broker/recipe.rs#L158), checked by
  [`missing`, run.rs:1161](../../src/capture/broker/run.rs#L1161).
- **How it works.** At the end, keep the requirements whose condition holds *now*, then keep those
  with no collected record of that type that parsed successfully.
- **Why it exists.** "Complete" depends on what happened. Crash logs are only required when there was
  a crash.
- **What breaks.** If the deployment itself was never fetched, "the deployment failed" is false, so
  logs and metrics become *not required*. An investigation that knows less can therefore look closer
  to complete (Part 5, Example 1).
- **Fix and criticality.** When a condition refers to an evidence type that is itself missing,
  report the requirement as `NotQueried` instead of dropping it. **MEDIUM.**
- **Resolved (2026-10-21).** `Predicate::unevaluable`
  ([recipe.rs:163](../../src/capture/broker/recipe.rs#L163)) tells whether a condition depends on
  evidence that was never collected. [`missing`](../../src/capture/broker/run.rs#L1539) keeps such a
  requirement as missing, with state `NotQueried` and a detail saying the condition could not be
  evaluated. An investigation that knows less no longer looks more complete. Some packs that used to
  read `Sufficient` now read `Incomplete`; that is the correction. **New criticality: NONE.**

#### 2.3.6 `StoppingRules`
- **Definition.** One rule today: `stop_when_satisfied`, which stops as soon as nothing required is
  missing.
- **Code.** [recipe.rs:253](../../src/capture/broker/recipe.rs#L253), applied at
  [run.rs:893](../../src/capture/broker/run.rs#L893).
- **Why it exists.** It saves cost.
- **What breaks.** Later steps become `NotQueried` even when they would have found contradicting
  evidence, such as a second health check.
- **Fix and criticality.** Allow "always run" steps (verification) that the stopping rule cannot
  skip. **LOW.**
- **Resolved (2026-10-21).** `RecipeStep.mandatory` marks a step that the stopping rule
  never skips: once the required evidence is complete, later steps become `NotQueried`, except
  mandatory ones, which still run. The field is left out of the serialized recipe when false, so the
  digests of existing recipes did not change. No built-in recipe uses it yet. **New criticality:
  NONE.**

#### 2.3.7 `Budget`, `ceiling`, `tighter`
- **Definition.** A **budget** caps connector calls, records, bytes, and time. The **ceiling** is
  the global maximum: 64 calls, 2000 records, 8 MiB, 120 s.
- **Code.** [recipe.rs:197](../../src/capture/broker/recipe.rs#L197),
  [`ceiling`, recipe.rs:211](../../src/capture/broker/recipe.rs#L211),
  [`tighter`, recipe.rs:226](../../src/capture/broker/recipe.rs#L226).
- **How it works.** The effective budget is the field-by-field minimum of the request's, the
  recipe's, and the ceiling.
- **Why it exists.** Nobody can exceed the ceiling, and a requester can only lower limits.
- **What breaks.** Time is checked only between steps
  ([`exhausted`, run.rs:369](../../src/capture/broker/run.rs#L369)), so a connector that hangs is never
  interrupted.
- **Fix and criticality.** Give each connector call a deadline. **MEDIUM.**
- **Partly resolved (2026-10-21).** Each investigation has a deadline
  ([`Investigation::deadline`, run.rs:413](../../src/capture/broker/run.rs#L413)), passed to every
  connector call through `fetch_within` and `query_within`
  ([connector.rs:321](../../src/capture/resources/connector.rs#L321)). Git runs its commands with that
  deadline and answers `Timeout` when it passes. GitHub and the fixtures honor it too, and the
  fixture's `Slow` behavior tests it. What remains: Crane's own verification cannot be interrupted,
  though it now runs only once per investigation (§2.2.13). **New criticality: LOW.**

#### 2.3.8 `validate` and `digest`
- **Code.** [`validate`, recipe.rs:336](../../src/capture/broker/recipe.rs#L336),
  [`digest`, recipe.rs:404](../../src/capture/broker/recipe.rs#L404).
- **How it works.**
  - Validation checks: a valid id; 1 to 32 steps; limits positive and within the ceiling; known
    starting conditions; unique, valid step ids; Fetch steps have a record template; only Fetch and
    Query; valid templates; `1 ≤ limit ≤ 200`; valid predicates.
  - The **digest** is a hash of the whole recipe, a fingerprint: same digest, same plan.
- **What breaks.** Nothing. **NONE.**

#### 2.3.9 `RecipeBook`, the library of recipes
- **Definition.** The **RecipeBook** is the library of every known recipe, filed first by name and
  then by version number: `recipe_id → version → recipe`.
- **Code.** [recipe.rs:414](../../src/capture/broker/recipe.rs#L414).
- **How it works.**
  - [`add`, recipe.rs:426](../../src/capture/broker/recipe.rs#L426) validates, then refuses an existing
    name and version whose content differs: *"publish a new version"*.
  - [`get`, recipe.rs:450](../../src/capture/broker/recipe.rs#L450) finds a recipe by reference, and
    returns the latest version when none is given.
  - [`for_class`, recipe.rs:464](../../src/capture/broker/recipe.rs#L464) finds the latest recipe that
    serves a question class.
  - The broker fills the book with the two built-in recipes
    ([run.rs:445](../../src/capture/broker/run.rs#L445)).
- **Why it exists.** An audit record saying "failed-deployment v1" must mean the same plan forever.
  Replay also refuses a recorded recipe whose digest does not match
  ([run.rs:512](../../src/capture/broker/run.rs#L512)).
- **What breaks.**
  1. The book is rebuilt from code on every run, so immutability only holds inside one process. A
     code change to `failed_deployment()` without a version bump silently changes "v1".
  2. When two recipes serve the same class, `for_class` picks by alphabetical name order, not by
     intent.
- **Fix and criticality.**
  1. Persist recipe digests and refuse a known version whose digest changed. **MEDIUM.**
  2. Refuse two recipes for the same class, or rank them explicitly. **LOW.**
- **Resolved (2026-10-21).** 
  1. A test pins the digest of every built-in recipe. The store also pins
     `(recipe id, version) → digest` (`pin_recipe`, [store.rs:1070](../../src/capture/store.rs#L1070)),
     and a known version whose digest changed is refused with "publish a new version".
  2. `RecipeBook::add` refuses a recipe id that serves a class another recipe id already serves.
     Versions of one recipe may share a class, and the latest one is chosen.

  **New criticality: NONE** for both.

#### 2.3.10 `from_plan`, plans proposed by a model
- **Definition.** A **plan** is a list of steps proposed by a caller (usually a model) instead of a
  registered recipe.
- **Code.** [recipe.rs:792](../../src/capture/broker/recipe.rs#L792).
- **How it works.** Wrap the steps in a recipe with tighter limits (16 calls, 200 records, 1 MiB,
  30 s), run the **same** validation, and name it `plan-<12 characters of digest>`.
- **Why it exists.** It is the only way to investigate a question no recipe covers. A model's
  suggestion gets no special trust.
- **What breaks.** Required evidence comes only from the request, so there are no conditional
  requirements. Nothing judges whether the plan is *relevant*, only whether it is *allowed*.
- **Fix and criticality.** Allow conditional requirements in plans. **LOW.**
- **Resolved (2026-10-21).** A request can carry `plan_required`: evidence required
  under a condition, validated and used like a recipe's requirements. Whether a plan is relevant
  stays the caller's job, by design. **New criticality: NONE.**

### 2.4 Scope resolution ([resources/scope.rs](../../src/capture/resources/scope.rs))

#### 2.4.1 `ScopeResolution`, what this investigation is about
- **Definition.** The **scope** is every fact identifying the subject of the investigation: task,
  episode, repository, branch, commits, pull requests, deployments, environment, sessions, zones and
  flows (with versions), and the time window. It also records which sources the actor may use
  (`permitted_resources`), which were refused and why (`denied`), which facts are still unknown
  (`unknown`), and which facts were discovered during the run (`learned`).
- **Code.** [scope.rs:83](../../src/capture/resources/scope.rs#L83); parts
  [`VersionedRef`](../../src/capture/resources/scope.rs#L21),
  [`TimeWindow`](../../src/capture/resources/scope.rs#L32),
  [`LearnedFact`](../../src/capture/resources/scope.rs#L44),
  [`Denial`](../../src/capture/resources/scope.rs#L57).
- **Why it exists.** Authorization and "what is this about?" must be settled *before* any source sees
  a question.

#### 2.4.2 `resolve`, the scope algorithm
- **Code.** [scope.rs:114](../../src/capture/resources/scope.rs#L114).
- **How it works.**
  1. Copy the known references from the request. Zones and flows start with no version.
  2. A named repository the policy forbids gets an **explicit** denial.
  3. A named environment the policy forbids gets an **explicit** denial.
  4. A named resource that is not registered gets an **explicit** denial.
  5. Every registered resource (or only the named ones, when the request names some) is refused with
     a reason if the policy does not grant it, it is not active, or it does not cover this repository
     or environment. Otherwise it is permitted.
  6. List the unknown facts ([`refresh_unknown`, scope.rs:217](../../src/capture/resources/scope.rs#L217)).
- **Explicit versus implicit.** An **explicit** denial means the request *specifically asked* for
  something forbidden, and it stops the whole investigation (§2.5.4). An **implicit** denial only
  removes one source.
- **What breaks.** A forbidden repository or environment that is **learned** mid-run is never treated
  as explicit (see §2.5.6, case C9 in Part 4, and Example 3 in Part 5).
- **Fix and criticality.** Re-run the rejection gate whenever `learn` adds a repository or
  environment. **CRITICAL** (part of the leak described in §2.1.6).
- **Resolved (2026-10-21).** By the §2.1.6 fix: a record that reveals a forbidden
  repository or environment stops the investigation as `Rejected`, the same outcome an explicit
  request gets. **New criticality: NONE.**

#### 2.4.3 `variable`, reading a fact for a template
- **Code.** [scope.rs:249](../../src/capture/resources/scope.rs#L249).
- **What breaks.** It returns the first value of a list, the defect of §2.3.3. **HIGH.**
- **Partly resolved (2026-10-21).** As in §2.3.3: it still returns the first value, but the
  other values are now named in a warning. **New criticality: LOW.**

#### 2.4.4 `learn`, discovering facts from evidence
- **Definition.** Filling scope gaps from what the evidence says, and recording which evidence said it.
- **Code.** [scope.rs:271](../../src/capture/resources/scope.rs#L271).
- **How it works.** The rules are fixed:
  - a PullRequest or Deployment teaches the commit, if none is known
    ([scope.rs:277](../../src/capture/resources/scope.rs#L277));
  - a Task teaches the pull request;
  - a Deployment teaches the environment, if none is known
    ([scope.rs:290](../../src/capture/resources/scope.rs#L290));
  - agent events teach sessions.

  Each fact is stored as a `LearnedFact` together with the evidence id that taught it.
- **Why it exists.** Real requests rarely carry every id. In the story, the commit is learned from the
  pull request's `head_sha`.
- **What breaks.**
  1. The **first** record ingested decides the commit, so with two deployments the commit is
     arbitrary. **HIGH.**
  2. Deployments and tasks are never learned, so a discovery query cannot feed what it found back
     into scope. **MEDIUM.**
  3. Facts are learned from records the actor may not see. **CRITICAL** (part of §2.1.6).
- **Fix.** Learn into lists and warn on more than one value; add deployment and task rules; learn
  only after the record passes eligibility.
- **Resolved (2026-10-21), items 2 and 3; partly resolved, item 1.**
  1. [`learn`](../../src/capture/resources/scope.rs#L289) records every distinct commit from pull
     requests and deployments, each as a learned fact. Which one a step uses is warned about
     (§2.3.3). **LOW.**
  2. Every deployment record adds its deployment, and a pull request teaches its task when none is
     known, so a discovery query feeds what it found back into scope. **NONE.**
  3. Learning happens only after the record passes authorization (§2.1.6). **NONE.**

### 2.5 The broker ([broker/run.rs](../../src/capture/broker/run.rs))

#### 2.5.1 The request: `EvidenceQueryRequest` and `KnownRefs`
- **Definition.** An **investigation request** is the question plus everything needed to answer it:
  - optional episode and task;
  - the question text and its **investigation class** (the kind of question, such as
    `deployment-failure`);
  - an explicit recipe or a proposed plan;
  - **known references** (`KnownRefs`): repository, branch, commits, pull requests, deployments,
    environment, sessions, zones, flows;
  - an optional restriction to certain resources;
  - extra required evidence and the evidence types wanted;
  - a time window, a budget, a freshness limit, and pack limits.
- **Code.** [run.rs:93](../../src/capture/broker/run.rs#L93), [run.rs:63](../../src/capture/broker/run.rs#L63).
- **Why it exists.** Everything that influences the result is in one value, and that value is copied
  into the audit record, so the result can be reproduced.
- **What breaks.** The `question` is only a **label**. Nothing reads its words. Turning a person's
  words into a class and references is left entirely to the caller (Part 5, Example 1).
- **Fix and criticality.** This is intended (a deterministic boundary). Document it, and provide
  helper recipes for discovery. **LOW.**

#### 2.5.2 The broker and the investigation in progress
- **Definition.** The **broker** ([`EvidenceQueryBroker`, run.rs:342](../../src/capture/broker/run.rs#L342))
  holds the registry, the connectors attached to resources, the RecipeBook, the store, and the clock.
  An **investigation** ([`Investigation`, run.rs:356](../../src/capture/broker/run.rs#L356)) is one run:
  its audit record being filled, the list of evidence ids collected *by this run* in order, and a
  start timer.
- **Helpers.**
  - [`note`, run.rs:393](../../src/capture/broker/run.rs#L393) records a step that called no connector.
  - [`deny`, run.rs:425](../../src/capture/broker/run.rs#L425) records a refusal.
  - [`exhausted`, run.rs:369](../../src/capture/broker/run.rs#L369) reports which limit was reached.
- **Wiring.** [`new`](../../src/capture/broker/run.rs#L445) loads the built-in recipes;
  [`connect`](../../src/capture/broker/run.rs#L470) attaches a connector to a registered resource.

#### 2.5.3 `investigate` and `select_recipe`
- **Code.** [run.rs:494](../../src/capture/broker/run.rs#L494), [run.rs:565](../../src/capture/broker/run.rs#L565).
- **How it works.** Choose a proposed plan (through `from_plan`), else the explicit recipe, else the
  recipe of the class. Then call `run` in `Fresh` mode ([`ExecutionMode`, run.rs:117](../../src/capture/broker/run.rs#L117)).
- **What breaks.** When no recipe matches, an error is returned and nothing is audited (§2.1.8).
  **MEDIUM.**
- **Resolved (2026-10-21).** The refusal is now audited (§2.1.8). **New criticality:
  NONE.**

#### 2.5.4 `run` and the rejection gate
- **Code.** [run.rs:600](../../src/capture/broker/run.rs#L600), gate at [run.rs:661](../../src/capture/broker/run.rs#L661).
- **How it works.**
  1. Compute the query id and the budget.
  2. Resolve scope.
  3. Work out the episode id.
  4. Create the investigation.
  5. Write one **Allow** decision per permitted resource and one **Deny** decision per denial
     ([`PolicyDecision`/`Effect`, run.rs:197-215](../../src/capture/broker/run.rs#L197-L215)).
  6. If any denial is explicit: `stop_reason = Rejected`, and nothing is queried.
  7. Otherwise: open the episode, resolve versions, execute, and link governance.
  8. Finish (§2.9). This always happens, even after a rejection.
- **Why the gate exists.** If you specifically asked for production and may not see it, then "here is
  what I found about production elsewhere" would leak.
- **How it helps.** A rejection still produces an audited pack with status `Rejected`.
- **What breaks.** The gate runs once, before any evidence arrives (§2.4.2). **CRITICAL** (part of
  §2.1.6).
- **Resolved (2026-10-21).** A record that reveals a forbidden scope now triggers the
  same rejection during the run (§2.1.6). **New criticality: NONE.**

#### 2.5.5 `resolve_versions`
- **Code.** [run.rs:691](../../src/capture/broker/run.rs#L691).
- **How it works.** For each zone and flow in scope, fetch `zone:<id>` or `flow:<id>` from every
  permitted Landscape resource, and copy the record's revision into the version. In the story,
  `refunds` becomes `v3` and `refund-flow` becomes `v2` ([tests.rs:1744](../../src/capture/tests.rs#L1744)).
- **Why it exists.** Later components compare "the version this pack saw" with "the version now"
  to detect stale conclusions.
- **What breaks.** These calls spend budget, but the budget is not checked before each one. **LOW.**
- **Resolved (2026-10-21).** [`resolve_versions`](../../src/capture/broker/run.rs#L889) checks
  the budget before each version fetch and stops once it is spent. **New criticality: NONE.**

#### 2.5.6 `execute`, the main loop (the step algorithm)
- **Code.** [run.rs:757](../../src/capture/broker/run.rs#L757).
- **How it works.** For each step, in recipe order:

```
A  budget reached? (run.rs:766)  → mark this and all later steps BudgetExhausted; stop
B  condition (run.rs:779)        → evaluate on THIS run's evidence only (run.rs:780);
                                    record a BranchDecision; if false → Skipped
C  type wanted by the request?   → if not → NotQueried
D  type allowed? (run.rs:810)    → if not → Deny + Denied
E  environment allowed? (run.rs:823) → if not → Deny + Denied
F  resources (run.rs:839)        → candidates = registered resources of this type that
                                    provide this evidence and support this operation;
                                    none → NotQueried; none permitted → Deny + Denied
G  for each permitted resource   → re-check budget (run.rs:880), then call()  (§2.5.7)
H  stop early? (run.rs:893)      → if satisfied → Satisfied; later steps NotQueried
```

- **Why this order.**
  - Cheap, decisive checks run before any source is touched.
  - An irrelevant step is skipped before permissions are checked, so it produces no false denial.
  - Conditions look only at this run's evidence, so the same request branches the same way no
    matter what else is in the store.
- **What breaks.** Check E runs for **every** step. Once an environment has been learned, even steps
  whose evidence has no environment (policies) are denied for an actor without that environment.
- **Fix and criticality.** Apply E only to evidence types that carry an environment. **MEDIUM.**
- **Resolved (2026-10-21).** Check E now applies only when
  `EvidenceType::is_environment_bound` ([record.rs:132](../../src/capture/format/record.rs#L132)) is
  true: deployments, verifications, runtime logs, metrics, configuration, CI runs, and test reports.
  Policies, zones, tasks, and code are no longer denied because an environment was learned. A
  forbidden learned environment rejects the run anyway (§2.1.6). **New criticality: NONE.**

#### 2.5.7 `call`, talking to one source
- **Code.** [run.rs:921](../../src/capture/broker/run.rs#L921).
- **How it works.**
  1. **Repository guard.** A repository-scoped resource with no known repository gives `NotQueried`.
  2. **Connector attached?** If not, `Failed`.
  3. **Parameters.**
     - Fetch: fill the record template ([run.rs:951](../../src/capture/broker/run.rs#L951)).
     - Query: fill the filters, then build a `QuerySpec` whose limit is the smaller of the step
       limit and the records the budget still allows.
     - An unknown variable gives `NotQueried`, naming the variable.
  4. **Validate** the query ([run.rs:1002](../../src/capture/broker/run.rs#L1002)). If invalid,
     `Failed` with status `InvalidQuery`, and the rejected spec is kept.
  5. **Ask** the connector ([run.rs:1022](../../src/capture/broker/run.rs#L1022)) and time the call.
  6. **Unusable status** ([run.rs:1053](../../src/capture/broker/run.rs#L1053)) gives `Failed`.
  7. **Store the cursor** if one was offered ([run.rs:1060](../../src/capture/broker/run.rs#L1060)).
  8. **For each record:**
     - if the record budget is full, stop;
     - if the record's type is not allowed for this actor, deny that one record
       ([run.rs:1084](../../src/capture/broker/run.rs#L1084));
     - if the byte budget would be exceeded, stop;
     - **ingest** ([run.rs:1096](../../src/capture/broker/run.rs#L1096), §2.6);
     - **link** ([run.rs:1125](../../src/capture/broker/run.rs#L1125), §2.7.5);
     - **learn** ([run.rs:1128](../../src/capture/broker/run.rs#L1128), §2.4.4);
     - add the id to `collected`.
  9. Write the step record ([`QueryStepRecord`, run.rs:173](../../src/capture/broker/run.rs#L173)).
- **What breaks.** Step 8 re-checks only the record's **type**, not its environment, repository, or
  sensitivity. Those are checked only at selection (§2.9.3), *after* learning and assertions have
  already used the record.
- **Fix and criticality.** Run [`ineligible`](../../src/capture/ingest/selection.rs#L144) here.
  **CRITICAL.**
- **Resolved (2026-10-21).** This is the §2.1.6 fix. Step 8 now runs the full
  authorization check, `withheld`, before linking, learning, or collecting each record. Step 5 now
  passes the investigation's deadline to the connector, and steps 5 to 8 repeat for each page while
  a cursor is offered and the budget allows (§2.2.10). **New criticality: NONE.**

#### 2.5.8 The step and stop states
- **`StepState`** ([run.rs:133](../../src/capture/broker/run.rs#L133)) is an outcome label on each step:

| State | Set when | Meaning for a reader |
|---|---|---|
| `Executed` | the connector answered usably | asked and answered (possibly with nothing) |
| `Skipped` | the condition was false | not relevant, by the recipe's logic |
| `NotQueried` | type not wanted, no resource, unknown value, unknown repository, or early stop | never asked |
| `Denied` | policy refused the type, environment, or every resource | not allowed to ask |
| `Failed` | no connector, invalid query, or an unusable status | asked, and the source broke |
| `BudgetExhausted` | a limit was reached first | ran out |

- **`StopReason`** ([run.rs:224](../../src/capture/broker/run.rs#L224)) records why retrieval ended:
  `Completed`, `Satisfied`, `BudgetExhausted`, or `Rejected`.
- **`BudgetUsage`** ([run.rs:239](../../src/capture/broker/run.rs#L239)) records what was spent.
- **What breaks.** Nothing in the labels themselves. **NONE.**

#### 2.5.9 `QueryAuditRecord`, the diary
- **Definition.** Every policy decision, branch, step with its exact parameters, connector status,
  snapshot and duration; the budget and its use; the stop reason, status, metrics, errors, replay
  comparison, pack id, and times.
- **Code.** [run.rs:299](../../src/capture/broker/run.rs#L299).
- **Why it exists.** Every pack must be explainable and replayable.
- **What breaks.** It is stored in the same JSONL store, so the store race (§2.13.3) can lose audits.
  **CRITICAL** (inherited from that race).
- **Resolved (2026-10-21).** Saves now merge under the lock (§2.13.3), so audits are no
  longer lost, and refusals are audited too (§2.1.8). **New criticality: NONE.**

### 2.6 Ingestion and the evidence format ([store.rs](../../src/capture/store.rs), [format/](../../src/capture/format))

#### 2.6.1 `IngestPath` and `IngestKind`
- **Definition.** An **ingestion path** says *how* a record arrived: `query:<query id>:<step id>` or
  `webhook:<delivery id>` ([store.rs:42](../../src/capture/store.rs#L42)). The **ingest kind** says what
  happened to it: `New`, `Duplicate`, or `NewRevision` ([store.rs:67](../../src/capture/store.rs#L67)).

#### 2.6.2 The ingestion algorithm
- **Code.** [`ingest`, store.rs:367](../../src/capture/store.rs#L367).
- **How it works.**
  1. A webhook delivery id already seen is a duplicate ([store.rs:377](../../src/capture/store.rs#L377)).
  2. Capture the artifact ([store.rs:382](../../src/capture/store.rs#L382), §2.6.3).
  3. An artifact id already seen is a duplicate ([store.rs:383](../../src/capture/store.rs#L383)).
  4. Normalize into an evidence record ([store.rs:391](../../src/capture/store.rs#L391), §2.6.5).
  5. If an earlier record has the same source and record id, this is a **new revision**: set
     `supersedes` and add the edge `new SUPERSEDES old`
     ([store.rs:400-431](../../src/capture/store.rs#L400-L431)).
  6. Store the blob, artifact, and evidence, and update the indexes
     ([`index`, store.rs:328](../../src/capture/store.rs#L328)): by source, type, episode, observed
     time, and artifact.
  7. For a duplicate, append the new path to the existing records' provenance
     ([`duplicate`, store.rs:451](../../src/capture/store.rs#L451)).
- **Why it exists.** Webhooks retry and sources update records. Neither may create phantom "new"
  evidence or overwrite history. "Seen by query *and* webhook" is itself provenance.
- **What breaks.**
  1. A duplicate keeps the `episode_id` of whichever episode captured it **first**. Selection ignores
     episodes for this reason ([run.rs:1309](../../src/capture/broker/run.rs#L1309)), but any other
     consumer that filters by episode will miss it. **LOW.**
  2. No webhook receiver exists outside the tests. **LOW.**
- **Resolved (2026-10-21).** Item 1: a duplicate now appends the new episode to the
  record's `observed_in`, so the record lists every episode that observed it; `episode_id` stays
  the first. **NONE.** Item 2 is left as designed: a receiver needs signed authentication of the
  sending source on the hosted service.

#### 2.6.3 `RawArtifact` and `capture`
- **Definition.** A **raw artifact** is the original content of a record, kept for audit: hashed,
  redacted, classified, and stored by reference.
- **Code.** [artifact.rs:35](../../src/capture/format/artifact.rs#L35),
  [`capture`, artifact.rs:82](../../src/capture/format/artifact.rs#L82),
  [`classify`, artifact.rs:59](../../src/capture/format/artifact.rs#L59).
- **How it works.**
  1. Hash the content exactly as the source sent it.
  2. Copy it and redact secrets, counting the redactions.
  3. Sensitivity starts at `Confidential` if anything was redacted, otherwise `Internal`, and is
     raised (never lowered) by the source's declared sensitivity.
  4. Storage: `Secret` becomes `withheld` (nothing kept); content becomes `blob:<hash>`; no content
     becomes `external:<uri>` or `none`.
- **Why it exists.**
  - Hashing the source's own bytes makes duplicate detection exact.
  - Redacting *before* storing means a secret never reaches the graph or a pack.
  - A sensitivity floor means a source can raise the level but never hide a found secret.
  - In the story, `password=Sup3rS3cretValue` is redacted and the log becomes `Confidential`
    ([tests.rs:532-538](../../src/capture/tests.rs#L532-L538)).
- **What breaks.** Redaction is pattern-based ([telemetry/redact.rs](../../src/telemetry/redact.rs)), so
  an unfamiliar secret format is stored. **MEDIUM.**
- **Partly resolved (2026-10-21).** As in §2.1.5: runtime logs and tool calls are at least
  Confidential whatever the redactor finds, but there is no entropy-based detection, so a secret
  in an unfamiliar format is still stored, redacted only if recognized. **New criticality: LOW.**

#### 2.6.4 `EvidenceType`, `PayloadKind`, `Payload`
- **Definition.**
  - An **evidence type** is one of 16 kinds of fact ([record.rs:33](../../src/capture/format/record.rs#L33)).
  - Each type maps to a **payload kind**: `Text`, `Structured`, `Code`, `Measurement`, or `Graph`
    ([`payload_kind`, record.rs:59](../../src/capture/format/record.rs#L59)).
  - A **payload** is the parsed content in that shape ([record.rs:176](../../src/capture/format/record.rs#L176)).
  - **Expected fields** list what each type should contain
    ([record.rs:76](../../src/capture/format/record.rs#L76)), for example a Deployment expects
    `environment, status, commit`.
- **Why it exists.** Five shapes cover every source, so the rest of the system reasons over five
  structures, not hundreds.
- **What breaks.** [`Payload::field`](../../src/capture/format/record.rs#L208) only reads Structured
  fields, the commit of Code, and the first node of Graph. A predicate on a field of any other shape
  is always false. **LOW.**
- **Resolved (2026-10-21).** Recipe validation now refuses a field condition on Text or
  Measurement evidence, so a predicate that could never hold is caught when the recipe is written,
  instead of being silently false at run time. **New criticality: NONE.**

#### 2.6.5 Parsers and `normalize`
- **Definition.** A **parser** turns raw content into a payload and reports what it could not
  understand ([`EvidenceParser`, parsers.rs:82](../../src/capture/format/parsers.rs#L82)). There is one
  per payload kind:
  [Text](../../src/capture/format/parsers.rs#L126), [Structured](../../src/capture/format/parsers.rs#L192),
  [Code](../../src/capture/format/parsers.rs#L269), [Measurement](../../src/capture/format/parsers.rs#L343),
  [Graph](../../src/capture/format/parsers.rs#L412).
- **Parse status** ([record.rs:119](../../src/capture/format/record.rs#L119),
  [parsers.rs:58](../../src/capture/format/parsers.rs#L58)):
  - `Parsed`: a payload with no issues;
  - `PartiallyParsed`: a payload with issues;
  - `Failed`: no payload at all.
- **`normalize`** ([parsers.rs:613](../../src/capture/format/parsers.rs#L613)):
  1. Choose the parser ([`parser_for`, parsers.rs:479](../../src/capture/format/parsers.rs#L479)).
  2. Parse. Withheld or absent content is a *failed* parse with a reason.
  3. Find the **event time** ([`observed_at`, parsers.rs:570](../../src/capture/format/parsers.rs#L570)):
     the source timestamp or a known time field. For event-like types
     ([`is_event`, parsers.rs:590](../../src/capture/format/parsers.rs#L590)), a missing time is recorded
     as an issue and is **never** replaced by "now".
  4. Extract references ([`references`, parsers.rs:541](../../src/capture/format/parsers.rs#L541)) by
     trying common names. For example, the commit is the first of `commit`, `head_sha`, `sha`.
  5. Work out the scope: the repository from the content, or the resource's single repository.
  6. Assemble the [`EvidenceRecord`](../../src/capture/format/record.rs#L338).
- **Why it exists.** A parser that fills gaps manufactures facts. These parsers only report them. A
  missing metric value is `Unobserved`, not 0.
- **What breaks.**
  1. Reference names are a fixed list. A source using `revision` or `git_sha` links to nothing and
     looks unrelated. **MEDIUM.**
  2. Entity keys are not normalized, so a short sha and a full sha become two different commits
     (§2.7.1).
- **Fix.** Per-connector field mappings, and key normalization.
- **Partly resolved (2026-10-21).** 
  1. The commit lookup also tries `commit_sha` and `git_sha`. There is no per-connector field map,
     so other names, such as `revision`, still link to nothing. **LOW.**
  2. [`entity_id`](../../src/capture/correlate/model.rs#L95) trims and lowercases Commit and
     Environment keys, so `9F1C2AB7D4` and `9f1c2ab7d4` are one commit. A short sha and a full sha
     are still two. **LOW.**

#### 2.6.6 The evidence record
- **Definition.** The normalized fact everything else reasons about.
- **Code.** [record.rs:338](../../src/capture/format/record.rs#L338).

| Field | Why it exists |
|---|---|
| `evidence_id` | stable identity |
| `schema_version` | future readers |
| `source_ref` ([record.rs:244](../../src/capture/format/record.rs#L244)) | exactly where it came from |
| `evidence_type`, `payload`, `payload_ref` | what it says, and where the original is |
| `observed_at` vs `ingested_at` | when it **happened** vs when we **learned** it; a log from Tuesday fetched today is Tuesday's evidence |
| `provenance` ([record.rs:261](../../src/capture/format/record.rs#L261)) | connector, configuration version, parser and version, every ingestion path, query id |
| `scope` ([record.rs:278](../../src/capture/format/record.rs#L278)) | repository, environment, zones, flows |
| `sensitivity` | who may see it |
| `parse_status`, `parse_issues` | how well it was understood |
| `episode_id` | the first episode that captured it |
| `refs` ([record.rs:302](../../src/capture/format/record.rs#L302)) | pointers to other objects |
| `supersedes` | the previous revision |

### 2.7 Correlation ([correlate/](../../src/capture/correlate))

#### 2.7.1 Entities and node references
- **Definition.**
  - An **entity** is a real-world object the evidence talks about: a repository, commit, file, pull
    request, task, work episode, CI run, deployment, environment, agent session, tool call, zone,
    flow, policy, service, API, table, or resource ([`EntityKind`, model.rs:26](../../src/capture/correlate/model.rs#L26)).
  - Its id comes from (kind, key) ([`entity_id`, model.rs:93](../../src/capture/correlate/model.rs#L93)).
  - A **node** is anything in the graph: an entity, evidence, an assertion, or a contribution
    ([`NodeRef`, model.rs:110](../../src/capture/correlate/model.rs#L110)).
- **Why it exists.** "Commit 9f1c…" mentioned by CI, the pull request, and the deployment must be one
  node.
- **What breaks.** [`add_entity`, graph.rs:163](../../src/capture/correlate/graph.rs#L163) **overwrites
  the version** whenever the incoming entity has one. A context proposal targeting a zone creates or
  updates that zone entity with **the version the proposer supplied**
  ([store.rs:724-734](../../src/capture/store.rs#L724-L734)), and it does so *before* the proposal is
  validated. An agent proposing for zone `refunds` with version `v99` changes the graph's zone
  version to `v99`, **even if the proposal is then refused**. Governance links then carry
  `zone:refunds@v99`.
- **Fix and criticality.** Never let a contribution write entity attributes: look up the target
  entity, and refuse when it is unknown. Separately, never overwrite a version learned from
  evidence. **HIGH.**
- **Resolved (2026-10-21).** [`CaptureStore::propose`](../../src/capture/store.rs#L938)
  validates the proposal first and touches nothing when it is refused. A zone or flow target must
  already have been observed, otherwise the answer is `NotFound`, and a proposal never writes the
  entity: the version it names stays on the contribution only.
  [`add_entity`](../../src/capture/correlate/graph.rs#L164) now only fills a missing version. A version
  can change only through evidence, by `observe_version`
  ([graph.rs:193](../../src/capture/correlate/graph.rs#L193)), which only the linker calls. **New
  criticality: NONE.**

#### 2.7.2 The controlled vocabulary of relations
- **Definition.** A **relation** is a typed arrow between two nodes, such as *commit WRITES file*. A
  **controlled vocabulary** is a **closed list of allowed words, fixed in code, each with a strict
  meaning and a strict grammar** of what may sit on each side. Data cannot add words. A relation
  with a word that is not on the list, or with the wrong kinds of endpoints, is refused.
- **Code.** The 19 words are in [`RelationType`, model.rs:144](../../src/capture/correlate/model.rs#L144).
  The grammar is [`check_endpoints`, model.rs:287](../../src/capture/correlate/model.rs#L287), and the
  version is [`VOCABULARY_VERSION`, model.rs:13](../../src/capture/correlate/model.rs#L13):

| Word | From → to | Example |
|---|---|---|
| CONTAINS, DEPENDS_ON, CALLS, READS, WRITES, TRIGGERED, DEPLOYED_TO, EXECUTED_DURING, RESULTED_IN | entity → entity | commit WRITES file |
| BELONGS_TO_ZONE / BELONGS_TO_FLOW | entity → zone / flow | file BELONGS_TO_ZONE refunds |
| GOVERNED_BY | entity → policy | zone GOVERNED_BY refund_safety |
| SUPPORTS / CONTRADICTS | evidence or assertion → assertion | CI-run evidence SUPPORTS ci_passed |
| DERIVED_FROM | assertion, contribution, or evidence → evidence or assertion | change_verified DERIVED_FROM ci_passed |
| OBSERVED_ON | evidence → entity | log OBSERVED_ON deployment |
| PROPOSED_FOR / APPLIES_TO | contribution or assertion → entity | hypothesis PROPOSED_FOR zone |
| SUPERSEDES | same kind → same kind | evidence v2 SUPERSEDES evidence v1 |

- **Why it exists.** If every source could invent words, two systems would use one word for two
  meanings, and no reasoning over the graph could be trusted.
- **How it helps.** A malformed link is refused and counted as a correlation failure. Downstream code
  can match on a known set of words.
- **What breaks.** Expressiveness: "this test covers this file" has no word. Adding one is a code
  change plus a version bump. **LOW.**

#### 2.7.3 Relation status: how a link is known
- **Definition.**
  - `Observed`: the source reported this about its own object (CI says "I ran on commit X").
  - `Declared`: one record states a link to another object, or a configuration declares it.
  - `Inferred`: a rule guessed it (a branch name contains the task key).
  - `Reviewed`: a human confirmed it.
- **Code.** [model.rs:175](../../src/capture/correlate/model.rs#L175); each relation also stores its
  method, method version, and evidence ([`RelationProvenance`, model.rs:189](../../src/capture/correlate/model.rs#L189)).
- **Why it exists.** Consumers can choose to follow only strong links. An inferred link is never
  stored as observed.
- **What breaks.** `Reviewed` is never produced, so a human cannot promote a link yet. **LOW.**
- **Resolved (2026-10-21).** `crane capture confirm RELATION_ID` (through
  `review_relation`, [store.rs:1173](../../src/capture/store.rs#L1173)) records a new relation with
  status `Reviewed` beside the original, which is never changed. Only a human with review rights
  may confirm. **New criticality: NONE.**

#### 2.7.4 `Relation` and the graph's mechanics
- **Code.** [`Relation`, model.rs:209](../../src/capture/correlate/model.rs#L209);
  [`add_relation`, graph.rs:225](../../src/capture/correlate/graph.rs#L225);
  [`relations_of`, graph.rs:265](../../src/capture/correlate/graph.rs#L265).
- **How it works.** Both endpoints must already exist, and the grammar must allow the pair. The same
  relation added twice merges its evidence lists instead of duplicating the edge. Outgoing and
  incoming indexes make neighbor lookups fast.
- **What breaks.** `valid_from` and `valid_to` exist on relations but are never set, so the
  traversal's point-in-time filter does nothing. **LOW.**

#### 2.7.5 `link`, the per-record correlation rules
- **Code.** [link.rs:123](../../src/capture/correlate/link.rs#L123).
- **How it works.** For each record, create the entities it names (repository, commit, environment),
  then apply the rule for its type:

| Evidence | Links created | Status |
|---|---|---|
| Task | Task RESULTED_IN PR (if it names one) | declared |
| PullRequest | Repo CONTAINS PR; PR CONTAINS each commit; Task RESULTED_IN PR (if named), or from the branch name | observed / declared / **inferred** |
| CodeChange | Repo CONTAINS commit; commit WRITES each file | observed |
| CiRun | commit TRIGGERED run | observed |
| Deployment | commit RESULTED_IN deployment; deployment DEPLOYED_TO environment | observed |
| ToolCall | call EXECUTED_DURING session; session EXECUTED_DURING episode (when the tasks match) | observed |
| GraphState | a Zone or Flow entity with selectors, includes, and policies | — |
| Policy | a Policy entity with its revision as version | — |

  Finally, every entity the record mentions gets `evidence OBSERVED_ON entity`.
- **Why it exists.** The file header states the rule: *nothing is linked by temporal proximity*.
  Timing coincidences are the most common false correlation in incident analysis.
- **What breaks.** `Validation` creates no entity at all, so it connects to the graph only through
  the repository. **LOW.**

#### 2.7.6 `link_governance`
- **Code.** [link.rs:439](../../src/capture/correlate/link.rs#L439).
- **How it works.** For every zone and flow entity and every file entity: a zone covers a file through
  a path glob, a file list, or a directory, and a flow covers it through `include`. Each match adds
  `file BELONGS_TO_ZONE/FLOW` (declared) with a **version scope** such as `zone:refunds@v3`. Policies
  named by a zone or flow add `GOVERNED_BY`.
- **Why it exists.** "This file is in zone refunds" is only true *for version 3* of that zone.
- **What breaks.**
  1. It runs over **every** file and **every** zone on **every** investigation, so its cost grows
     forever. **MEDIUM.**
  2. It inherits the corrupted version from §2.7.1. **HIGH.**
- **Resolved (2026-10-21).** 
  1. Governance linking now covers only what changed: files first seen in this run, plus every
     file when a zone or flow was added or changed version
     ([`link_governance_changes`, link.rs:464](../../src/capture/correlate/link.rs#L464), comparing
     `governance_state` before and after the run). Relation ids are deterministic, so the result
     equals a full re-link, and a test checks that. **NONE.**
  2. Resolved by §2.7.1. **NONE.**

#### 2.7.7 Traversal and the Context View
- **Definition.**
  - A **traversal** walks the graph from starting nodes, breadth-first (everything 1 step away,
    then everything 2 steps away, and so on).
  - A **Context View** is a traversal packaged for one question.
- **Code.** [`traverse`, graph.rs:294](../../src/capture/correlate/graph.rs#L294),
  [`context_view`, graph.rs:410](../../src/capture/correlate/graph.rs#L410), hard limits at
  [graph.rs:18-24](../../src/capture/correlate/graph.rs#L18-L24).
- **How it works.**
  1. The caller must name which relation words to follow; none is an error.
  2. Depth is clamped to 6, results to 1000, size to 1 MiB.
  3. A visibility check hides nodes. Hidden nodes are counted but never returned.
  4. Each limit that is hit is recorded ([`LimitHit`, graph.rs:74](../../src/capture/correlate/graph.rs#L74)).
  5. The view id is a stable hash of the question and its members.
- **Why it exists.** "Everything connected to production" is the whole company. Bounding the walk
  keeps its cost fixed no matter how large the graph grows.
- **What breaks.** Nothing in the walk itself. **NONE.**

### 2.8 Assertions ([correlate/assertions.rs](../../src/capture/correlate/assertions.rs))

#### 2.8.1 Assertion, claim, derivation
- **Definition.**
  - An **assertion** is a statement that can be true or false about one entity, kept together with
    the evidence on each side.
  - A **claim** is the statement itself: subject, predicate (for example `ci_passed`), and a `Truth`
    value ([assertions.rs:78](../../src/capture/correlate/assertions.rs#L78)).
  - A **derivation** records how the assertion was produced: method, rule, rule version, inputs
    ([assertions.rs:62](../../src/capture/correlate/assertions.rs#L62)).
- **States** ([`AssertionStatus`, assertions.rs:30](../../src/capture/correlate/assertions.rs#L30)):
  - `Supported`: the evidence agrees. This can be agreement on *No*.
  - `Unresolved`: there is disagreement, or there is no evidence.
  - `Proposed`: an agent's hypothesis.
  - `Contradicted` and `Superseded`: declared but **never produced** by any code.

#### 2.8.2 Observations, independence, and the decision table
- **Code.** [`independent`, assertions.rs:186](../../src/capture/correlate/assertions.rs#L186),
  [`from_observations`, assertions.rs:212](../../src/capture/correlate/assertions.rs#L212).
- **How it works.** Each piece of evidence casts a vote (`Yes`, `No`, or `Unknown`). Copies of one
  artifact count once (by content hash). Then:

| Yes votes | No votes | Result |
|---|---|---|
| some | none | Yes, Supported |
| none | some | No, Supported |
| some | some | **Unresolved**, both sides kept |
| none | none | Unknown, Unresolved |

- **Why it exists.** Picking a winner hides the disagreement, and the disagreement is often the real
  finding. Copies must not look like corroboration.
- **What breaks.** Two sources reporting the same fact with different bytes count as two independent
  votes, even if one is a mirror of the other. **LOW.**

#### 2.8.3 The four rules
- **Code.** [`derive`, assertions.rs:362](../../src/capture/correlate/assertions.rs#L362), with helpers
  [`status_truth`](../../src/capture/correlate/assertions.rs#L309) and
  [`tests_truth`](../../src/capture/correlate/assertions.rs#L324).
- **How it works.**
  - `ci_passed` per commit: from CI run status.
  - `tests_passed` per commit: failed > 0 means No; passed > 0 with 0 failed means Yes; "zero tests
    ran" is not a pass.
  - `deployment_succeeded` per deployment: from the deployment status and each verification's
    `healthy`.
  - `change_verified` per task ([assertions.rs:460](../../src/capture/correlate/assertions.rs#L460)):
    any settled input that is No gives No; all settled Yes gives Yes; otherwise Unknown.
- **In the story:** ci_passed **Yes**; tests_passed **Unknown**; deployment_succeeded **No** (two
  independent sources); change_verified **No**.
- **What breaks.**
  1. Only four rules exist. A failed policy validation, an agent touching a critical zone, or a
     missing review produce evidence but **no conclusion and no warning**. **HIGH.**
  2. Assertions are derived from **every collected record**
     ([run.rs:1238](../../src/capture/broker/run.rs#L1238)), including ones the actor will not be
     allowed to see. **CRITICAL** (part of §2.1.6).
  3. With no subjects known, there are no assertions at all, and an empty list can read as "nothing
     wrong". **MEDIUM.**
- **Fix.** Add `policies_hold` and `governed_zone_touched` rules; derive only from records that pass
  eligibility; emit an Uncertainty warning when there are no subjects.
- **Resolved (2026-10-21).** 
  1. Two rules were added. `policies_hold`, per repository, comes from the Validation record's
     `passed`; it is reported beside `change_verified` and never folded into it (approved).
     `governed_zone_touched`, per commit, lists the zones its files belong to, as information
     rather than pass or fail. `RULES_VERSION` stays 1, because the existing rules did not change,
     so existing assertion ids did not move. A class guard checks that every evidence type a
     built-in recipe requires feeds a rule (`RULE_INPUTS`) or is marked context only
     (`CONTEXT_ONLY`). **NONE.**
  2. Assertions are derived only from records the actor may see (§2.1.6). **NONE.**
  3. With no subject known, the pack gets an Uncertainty warning saying no subject was
     established, so no conclusions were derived. **NONE.**

#### 2.8.4 Hypotheses and tracing
- **Code.** [`hypothesis`, assertions.rs:538](../../src/capture/correlate/assertions.rs#L538),
  [`trace`, assertions.rs:608](../../src/capture/correlate/assertions.rs#L608).
- **How it works.** An agent's hypothesis stays `Proposed` whatever it cites. `trace` follows the
  `derived_from` links up to a depth limit, collects every piece of evidence, and reports
  `complete = false` if anything was cut off.
- **Why it exists.** Citing evidence does not make a guess true. Every conclusion must be traceable.
- **What breaks.** Nothing. **NONE.**

### 2.9 Building the pack ([run.rs:1204](../../src/capture/broker/run.rs#L1204), [ingest/](../../src/capture/ingest))

#### 2.9.1 The finish algorithm
- **How it works.**
  1. Derive assertions ([run.rs:1244](../../src/capture/broker/run.rs#L1244)).
  2. Compute the missing evidence ([run.rs:1254](../../src/capture/broker/run.rs#L1254)).
  3. Compute the status ([run.rs:1260](../../src/capture/broker/run.rs#L1260)).
  4. Turn each step into a source report.
  5. Explain each missing item.
  6. Select and rank the evidence ([run.rs:1319](../../src/capture/broker/run.rs#L1319)).
  7. Build the context view ([run.rs:1358](../../src/capture/broker/run.rs#L1358)): roots are the task,
     episode, deployments, and commits; all relation words in both directions; depth 3; at most 400
     nodes.
  8. Gather contributions ([run.rs:1522](../../src/capture/broker/run.rs#L1522)).
  9. Write warnings ([run.rs:1566](../../src/capture/broker/run.rs#L1566)).
  10. Package the excerpts, seal ([run.rs:1451](../../src/capture/broker/run.rs#L1451)), and store.

#### 2.9.2 Source reports and the reason for each missing item
- **Definition.** A **source state** ([pack.rs:65](../../src/capture/ingest/pack.rs#L65)) labels what
  happened at each source. It is mapped from the step state by
  [`source_report`, run.rs:1478](../../src/capture/broker/run.rs#L1478):

| Step result | Source state | What a person should do |
|---|---|---|
| Executed, 0 records | `Empty` | the source has nothing: go produce it (for example, run the tests) |
| Executed, nothing parsed | `ParseFailed` | fix the source's format or the parser |
| Executed, something parsed | `Collected` | nothing |
| Denied, or Unauthorized | `Denied` | grant access, or ask someone who has it |
| Failed (other reasons) | `Unavailable` | repair or retry the source |
| Skipped, NotQueried, BudgetExhausted | `NotQueried` | supply the missing id, connect a source, or raise the budget |

  For a missing item, the most informative state among its sources is chosen, in this order:
  ParseFailed, Denied, Unavailable, Empty, otherwise NotQueried
  ([`MissingEvidence`, pack.rs:102](../../src/capture/ingest/pack.rs#L102)).
- **Why it exists.** Each reason points to a different repair. A bare "missing" cannot be acted on.
- **What breaks.** `Partial` with zero records shows as `Empty` (§2.2.7). **MEDIUM.**
- **Resolved (2026-10-21).** By the §2.2.7 fix: it now shows as `Unavailable`. **New
  criticality: NONE.**

#### 2.9.3 Selection: eligibility first, ranking second
- **Code.** [`ineligible`, selection.rs:144](../../src/capture/ingest/selection.rs#L144),
  [`select`, selection.rs:187](../../src/capture/ingest/selection.rs#L187),
  [`DeterministicRanker::score`, selection.rs:116](../../src/capture/ingest/selection.rs#L116).
- **How it works.**
  1. Exclude records that are `Unauthorized`, `TooSensitive`, `OutOfScope`, `TypeNotRequested`, or
     `OutsideTimeWindow` ([`ExclusionReason`, selection.rs:26](../../src/capture/ingest/selection.rs#L26)).
  2. Rank what remains by type priority, then parse quality, then recency. Ties break on id.
  3. Keep the first `max_records`; the rest are `OverBudget`.
- **Why it exists.** Relevance must never override authorization. A model-based ranker plugged in
  later could only reorder records that are already allowed.
- **What breaks.** This is the **first** place a record's environment and repository are checked.
  Everything earlier (learning, assertions) has already used the record. **CRITICAL** (§2.1.6).
- **Resolved (2026-10-21).** Selection is no longer the first check. Records are
  checked as they arrive, with the same function (§2.1.6), and selection checks them again. **New
  criticality: NONE.**

#### 2.9.4 Excerpts and artifact references
- **Code.** [`excerpt`, pack.rs:329](../../src/capture/ingest/pack.rs#L329),
  [`pack_evidence`, pack.rs:350](../../src/capture/ingest/pack.rs#L350).
- **Why it exists.** Packs feed model contexts. Whole logs would waste tokens and leak more than
  needed. The artifact reference lets an authorized reader fetch the original.
- **What breaks.** Nothing. **NONE.**

#### 2.9.5 Contributions in the pack
- **Code.** [run.rs:1522](../../src/capture/broker/run.rs#L1522).
- **How it works.** Only `Proposed` and `Accepted` contributions are included. **Task-local** ones
  only for this exact episode; **reusable** ones only when their target entity is in the view. The two
  groups are kept apart ([`PackContributions`, pack.rs:190](../../src/capture/ingest/pack.rs#L190)).
- **Why it exists.** "For this incident only" must never turn into "for everyone working in the zone".
- **What breaks.** Nothing beyond §2.7.1. **NONE.**

#### 2.9.6 Warnings
- **Definition.** Typed notes ([`WarningKind`, pack.rs:121](../../src/capture/ingest/pack.rs#L121)):
  - **Access**: denials, withheld records, rejection;
  - **Completeness**: partial answers, missing requirements;
  - **Conflict**: evidence on both sides;
  - **Uncertainty**: could not be established;
  - **Freshness**: no event time, or older than the limit;
  - **Budget**: a limit stopped retrieval;
  - **Truncation**: a size limit cut something.
- **Why it exists.** Typed warnings can drive automated gates. A free-text note cannot.
- **What breaks.** There is no Truncation warning for cursors that were not followed (§2.2.10).
  **HIGH** (same defect as there).
- **Resolved (2026-10-21).** Cursors are followed, and a Truncation warning names any
  step that stopped with pages left (§2.2.10). Other fixes added warnings as well: values that were
  not checked (§2.3.3), unknown starting conditions (§2.3.1), no subjects (§2.8.3), and withheld
  records counted without their ids (§2.1.6). **New criticality: NONE.**

#### 2.9.7 Sealing and the manifest
- **Code.** [`seal`, pack.rs:397](../../src/capture/ingest/pack.rs#L397),
  [`manifest_entries`, pack.rs:435](../../src/capture/ingest/pack.rs#L435),
  [`verify_manifest`, pack.rs:484](../../src/capture/ingest/pack.rs#L484).
- **How it works.**
  1. While the pack is over its byte limit, drop the lowest-ranked evidence and record a Truncation
     warning.
  2. Digest every item (evidence, artifact, entity, relation, assertion, contribution).
  3. Digest the list of digests.
  4. Derive the pack id from the query id and that digest.
- **Why it exists.** Capability intake recomputes the manifest. A single changed byte is detected
  and attributed to the item that changed.
- **What breaks.** Dropped evidence ids are still listed in `context_view.evidence`. **LOW.**
- **Resolved (2026-10-21).** [`seal`](../../src/capture/ingest/pack.rs#L397) removes dropped
  evidence from the context view and recomputes `view_id` before the manifest is computed. **New
  criticality: NONE.**

#### 2.9.8 `InvestigationStatus`, the verdict
- **Code.** [pack.rs:270](../../src/capture/ingest/pack.rs#L270).
- **How it works.** `Rejected` if rejected, else `BudgetExhausted` if the budget ran out, else
  `Sufficient` if nothing is missing, else `Incomplete`.
- **Why it exists.** Running out of budget is never reported as sufficient.
- **What breaks.** Nothing. **NONE.**

### 2.10 Context contributions ([ingest/contributions.rs](../../src/capture/ingest/contributions.rs))

#### 2.10.1 Definitions
- A **context contribution** ([contributions.rs:159](../../src/capture/ingest/contributions.rs#L159)) is a
  structured proposal of knowledge about a zone, flow, entity, or episode.
- Its **type** ([contributions.rs:36](../../src/capture/ingest/contributions.rs#L36)) is one of:
  Observation, Hypothesis, Summary, Question, ProcedureProposal, RelationshipProposal,
  ConstraintProposal.
- Its **namespace** ([contributions.rs:55](../../src/capture/ingest/contributions.rs#L55)) says where the
  knowledge stands: `agent-proposed`, `observed`, `human-approved`, or `system-derived`.
- Its **locality** ([contributions.rs:87](../../src/capture/ingest/contributions.rs#L87)) is `TaskLocal`
  or `Reusable`.

#### 2.10.2 `propose`, the proposal algorithm
- **Code.** [contributions.rs:269](../../src/capture/ingest/contributions.rs#L269), called through
  [`CaptureStore::propose`, store.rs:699](../../src/capture/store.rs#L699).
- **How it works.** The proposal is refused, in this order, when:
  1. the namespace does not match the actor (agents use `agent-proposed`, humans `observed`, the
     system `system-derived`; **nobody** may propose into `human-approved`);
  2. it is in `observed` but is not an Observation;
  3. evidence is required but none is cited;
  4. a cited evidence id does not exist;
  5. a ConstraintProposal would weaken a policy
     ([`weakens_policy`, contributions.rs:236](../../src/capture/ingest/contributions.rs#L236)), which
     includes any proposal that **declares no effect at all**;
  6. reusable context targets a work episode;
  7. after redaction, the content is empty or larger than 16 KiB.

  An accepted proposal is stored as `Proposed`. Constraint proposals are flagged
  `requires_crane_workflow`.
- **Why it exists.** Without these checks, the system's memory could be written by the agents it
  governs.
- **What breaks.**
  1. The target zone or flow entity is written **before** these checks run (§2.7.1). **HIGH.**
  2. "Weakening" is judged by an `effect` keyword. A proposal whose prose weakens a policy but whose
     declared effect is "tighten" passes; only the Crane-workflow flag remains as a safeguard.
     **MEDIUM.**
- **Resolved (2026-10-21).** 
  1. By the §2.7.1 fix: validation runs first, and a proposal never writes an entity. **NONE.**
  2. [`weakens_policy`](../../src/capture/ingest/contributions.rs#L239) now uses an allow-list: only
     `tighten`, `add`, and `clarify` are accepted, and a constraint proposal must name its policy.
     Prose can still disagree with its declared effect, so the authority stays Crane's signed
     workflow (`requires_crane_workflow`). **LOW.**

#### 2.10.3 The contribution lifecycle (a true state machine)
- **Code.** [`transition_allowed`, contributions.rs:211](../../src/capture/ingest/contributions.rs#L211).

```
propose ─► PROPOSED ──review(Accepted, by another authorized human)──► ACCEPTED
              │ ──review(Rejected)──► REJECTED (final)                  │
              │ ──supersede──► SUPERSEDED (final) ◄────supersede────────┤
              └ ──expire─────► EXPIRED (final)    ◄────expire───────────┘
```

- **Guards.**
  - [`review`, contributions.rs:389](../../src/capture/ingest/contributions.rs#L389): the reviewer must be
    a human with `may_review` who is not the contributor (`SelfApproval`); weakening is refused;
    Observation, Relationship, and Constraint proposals need evidence. Acceptance sets the namespace
    to `human-approved`.
  - [`supersede`, contributions.rs:469](../../src/capture/ingest/contributions.rs#L469): only the
    contributor or a reviewer, and only by a contribution about the same target.
  - [`expire`, contributions.rs:513](../../src/capture/ingest/contributions.rs#L513): when `valid_until`
    has passed.
- **What breaks.** `expire` is never called by anything, so expired contributions stay active. **LOW.**
- **Resolved (2026-10-21).** Every investigation expires overdue contributions before
  gathering them ([run.rs:834](../../src/capture/broker/run.rs#L834)), so an expired contribution is
  not in the pack. **New criticality: NONE.**

### 2.11 Work episodes, feedback, questions ([ingest/episode.rs](../../src/capture/ingest/episode.rs))

#### 2.11.1 The work episode and its outcome
- **Definition.** A **work episode** ([episode.rs:74](../../src/capture/ingest/episode.rs#L74)) is the unit
  of work that evidence is collected for, usually one task. Its **outcome**
  ([episode.rs:30](../../src/capture/ingest/episode.rs#L30)) is Succeeded, Failed, PartiallyVerified,
  Unknown, or Abandoned.
- **Lifecycle.** `open` ([episode.rs:99](../../src/capture/ingest/episode.rs#L99)) → OPEN →
  `record_outcome` ([episode.rs:129](../../src/capture/ingest/episode.rs#L129)) → CLOSED. It can be
  re-closed by the same recorder or by any human.
- **Why it exists.** *A successful tool call is not a successful task.* `Succeeded` needs evidence.
- **What breaks.**
  1. The evidence ids passed to `record_outcome` are **never checked to exist**, so "Succeeded,
     evidence: [made-up-id]" is accepted. **HIGH.**
  2. Any human may overwrite any outcome, not only reviewers. **MEDIUM.**
  3. **Episode ids** ([episode.rs:170](../../src/capture/ingest/episode.rs#L170)): with a task, the id comes
     from the task alone, so retries of a task merge into one episode. Without a task, the id
     includes the start time, so every follow-up investigation (and every replay) creates a **new**
     episode. **MEDIUM.**
- **Fix.** Check evidence existence the way `propose` does; require `may_review` to override an
  outcome; accept an explicit episode or conversation id.
- **Resolved (2026-10-21).** 
  1. Outcomes are recorded through [`CaptureStore::record_outcome`](../../src/capture/store.rs#L1018),
     which checks that each evidence id exists and belongs to this episode (captured in it,
     observed in it, or in one of its packs). The episode's own method is private to the capture
     module. **NONE.**
  2. Changing an outcome that someone else recorded needs review rights, and the previous outcome
     is kept in `outcome_history`. **NONE.**
  3. A replay stays in its original's episode, and `crane capture investigate episode EPI`
     continues an earlier one. A caller that passes neither a task nor an episode still starts a
     new episode. **NONE.**

#### 2.11.2 Feedback and questions
- **Definition.**
  - [`PackFeedback`, episode.rs:48](../../src/capture/ingest/episode.rs#L48) records whether a pack was
    sufficient (`Truth`) and what more was needed.
  - A [`Question`, episode.rs:209](../../src/capture/ingest/episode.rs#L209) is a structured request for
    evidence. [`to_request`, episode.rs:237](../../src/capture/ingest/episode.rs#L237) refuses a question
    that names no permitted resources.
- **What breaks.** Questions have states ([episode.rs:186](../../src/capture/ingest/episode.rs#L186)) but
  **no code moves them, and nothing stores them**. **LOW.**
- **Resolved (2026-10-21).** Questions are stored and move through their states.
  `crane capture ask FILE` stores one as Open (`ask`, [store.rs:1099](../../src/capture/store.rs#L1099)),
  and `crane capture question ID answer PACK_ID|block|unresolvable|reopen` moves it
  (`set_question_state`). Open may become Answered, Blocked, or Unresolvable; Blocked may reopen or
  end; Answered and Unresolvable are final. Answering needs a kept Evidence Pack. **New criticality:
  NONE.**

### 2.12 Replay ([run.rs:512](../../src/capture/broker/run.rs#L512))
- **Definition.** A **replay** re-runs a recorded investigation, with its exact recipe and request,
  against the sources as they are now, and compares the two runs.
- **How it works.**
  1. Load the original audit.
  2. Refuse it if the stored recipe's digest no longer matches.
  3. Re-run with the **current** permissions.
  4. Compare ([`compare`, run.rs:1761](../../src/capture/broker/run.rs#L1761)) each step that was
     originally `Executed`: was it executed again, in the same state, with the same snapshot and the
     same evidence ids?
  5. Verdict ([`ReplayFidelity`, run.rs:254](../../src/capture/broker/run.rs#L254)): `Diverged` if
     anything differs, `Unverifiable` if any snapshot is missing, otherwise `Exact`.
- **Why it exists.** "Would we conclude the same today?" is the drift question. A source that cannot
  prove its state can never yield `Exact`.
- **What breaks.**
  1. Steps that originally failed are not compared, so a source that has since **recovered** goes
     unreported. **MEDIUM.**
  2. Evidence ids are compared in order, so a source returning the same records in a different order
     shows as `Diverged`. **LOW.**
- **Resolved (2026-10-21).** 
  1. [`compare`](../../src/capture/broker/run.rs#L2307) also walks the replay's executed steps, so a
     step that answers now but did not before (a recovered source, or a branch that now holds) is
     reported. **NONE.**
  2. Evidence ids are compared as sets. **NONE.**

### 2.13 The store ([store.rs](../../src/capture/store.rs))

#### 2.13.1 Layout and location
- **Definition.** JSON-lines files under `<trust directory>/runtime/capture/`: artifacts, blobs,
  evidence, entities, relations, assertions, contributions, episodes, questions, packs, and audit,
  plus `state.json` (layout version, seen webhook deliveries, source cursors;
  [`StoreState`, store.rs:95](../../src/capture/store.rs#L95)).
- **Why it exists.** Outside the repository, an agent editing files cannot forge evidence. JSONL is
  simple and diffable.

#### 2.13.2 `open`, `save`, `migrate`
- **Code.** [store.rs:214](../../src/capture/store.rs#L214), [store.rs:288](../../src/capture/store.rs#L288),
  [store.rs:810](../../src/capture/store.rs#L810).
- **How it works.** `open` takes the lock, reads everything into memory, rebuilds the indexes and the
  graph, migrates the state, and **releases the lock when it returns**. `save` takes the lock again
  and rewrites every file atomically.

#### 2.13.3 What breaks
1. **The race.** Two concurrent runs both load, both add, both save, and the second save erases the
   first one's new evidence and audits. **CRITICAL.**
2. **Cost.** Every run loads and rewrites the whole history. **MEDIUM**, growing to HIGH with use.
3. **Linear lookup.** [`audit()`, store.rs:798](../../src/capture/store.rs#L798) scans every audit.
   **LOW.**

- **Fix.** Hold the lock from open to save, or move to append-only writes; then a real database
  (the hosted service already has MongoDB). Content-based ids (§2.1.2) make that move simple.
- **Resolved (2026-10-21), items 1 and 3; partly resolved, item 2.**
  1. [`save`](../../src/capture/store.rs#L372) takes the lock, re-reads what is on disk, merges it with
     memory, and writes. Merging is by content id (`merge_keyed`): records are united, a
     contribution's status never moves backwards (`lifecycle_rank`), audits keep their order, and
     the state merges deliveries, cursors, and recipe pins. Two concurrent investigations both keep
     their evidence and audits, and a test checks it. **NONE.**
  2. `write_collection` skips files that did not change, but a save still reads and merges every
     collection. **MEDIUM** at scale.
  3. `audit()` looks up an index by query id. **NONE.**

### 2.14 The command line ([cli/capture.rs](../../src/cli/capture.rs))
- **`crane capture demo`** runs the story from fixtures ([`run`, demo.rs:252](../../src/capture/demo.rs#L252)).
- **`crane capture sources`** lists the four live sources with their health.
- **`crane capture investigate [commit SHA] [task ID]`**
  - forces recipe `local-change` ([cli/capture.rs:323](../../src/cli/capture.rs#L323));
  - grants the caller every resource and the environments `local, ci, hosted`
    ([cli/capture.rs:300-312](../../src/cli/capture.rs#L300-L312));
  - saves the result.
- **What breaks.**
  1. Only one recipe is reachable from the CLI.
  2. There is no command for replay, contributions, review, outcomes, feedback, or verifying a pack.
- **Fix and criticality.** Add those commands. **MEDIUM** (they are reachable only from code today).
- **Resolved (2026-10-21).** `investigate` takes `class`, `recipe ID [version N]`, and
  `episode EPI`, and eight commands were added: `replay`, `verify`, `contribute`, `review`,
  `outcome`, `ask`, `question`, and `confirm` ([cli/capture.rs](../../src/cli/capture.rs)). The caller's
  policy covers this repository's local sources, and review rights come from the machine's key
  (§2.1.4). Every command refuses to run inside an AI agent session. Two end-to-end tests cover the
  commands. Pack feedback is still recorded only from code. **New criticality: LOW.**

---

## Part 3. Every state, and how states change

Two kinds of state appear in this component, and mixing them up causes most confusion:

- An **outcome label** is stamped **once** on a result and never changes. Example: a step was
  `Denied`.
- A **lifecycle** is a sequence of states that one object **moves through over time**, by allowed
  **transitions**, each protected by a **guard** (a condition that must hold). Example: a
  contribution moves from `Proposed` to `Accepted`.

| Name | Kind | Values | Set or moved by | Code | Known defect |
|---|---|---|---|---|---|
| `Truth` | value | Yes, No, Unknown | rules, feedback | [mod.rs:62](../../src/capture/mod.rs#L62) | none |
| `ResourceStatus` | admin state | Active, Disabled, Unhealthy | `set_status` | [registry.rs:53](../../src/capture/resources/registry.rs#L53) | never set automatically (§2.2.5) |
| `ConnectorStatus` | label | Ok, Partial, Unavailable, Unauthorized, RateLimited, InvalidQuery, Timeout, Failed | connector | [connector.rs:34](../../src/capture/resources/connector.rs#L34) | Partial with 0 records shows as Empty |
| `StepState` | label | Executed, Skipped, NotQueried, Denied, Failed, BudgetExhausted | `execute`, `call` | [run.rs:133](../../src/capture/broker/run.rs#L133) | none |
| `StopReason` | label | Completed, Satisfied, BudgetExhausted, Rejected | `run`, `execute`, `call` | [run.rs:224](../../src/capture/broker/run.rs#L224) | none |
| `InvestigationStatus` | verdict | Sufficient, Incomplete, BudgetExhausted, Rejected | `finish` | [pack.rs:270](../../src/capture/ingest/pack.rs#L270) | none |
| `SourceState` | label | Collected, Empty, Unavailable, Denied, NotQueried, ParseFailed | `source_report` | [pack.rs:65](../../src/capture/ingest/pack.rs#L65) | see ConnectorStatus |
| `ParseStatus` | label | Parsed, PartiallyParsed, Failed | parser | [record.rs:119](../../src/capture/format/record.rs#L119) | none |
| `IngestKind` | label | New, Duplicate, NewRevision | `ingest` | [store.rs:67](../../src/capture/store.rs#L67) | none |
| `RelationStatus` | knowledge level | Observed, Declared, Inferred, Reviewed | linker | [model.rs:175](../../src/capture/correlate/model.rs#L175) | Reviewed never produced |
| `AssertionStatus` | knowledge level | Supported, Unresolved, Proposed, Contradicted, Superseded | rules, hypothesis | [assertions.rs:30](../../src/capture/correlate/assertions.rs#L30) | last two never produced |
| `ReplayFidelity` | verdict | Exact, Diverged, Unverifiable | `compare` | [run.rs:254](../../src/capture/broker/run.rs#L254) | misses recovered sources |
| `ContributionStatus` | **lifecycle** | Proposed → Accepted / Rejected / Superseded / Expired; Accepted → Superseded / Expired | propose, review, supersede, expire | [contributions.rs:211](../../src/capture/ingest/contributions.rs#L211) | expire never called |
| `Namespace` | trust label | agent-proposed, observed, system-derived → human-approved (only by review) | propose, review | [contributions.rs:55](../../src/capture/ingest/contributions.rs#L55) | none |
| Episode outcome | **lifecycle** | open → closed(outcome), re-closable | `record_outcome` | [episode.rs:129](../../src/capture/ingest/episode.rs#L129) | evidence ids not checked |
| `QuestionState` | declared lifecycle | Open, Answered, Blocked, Unresolvable | nothing yet | [episode.rs:186](../../src/capture/ingest/episode.rs#L186) | not implemented |

**After the fixes (2026-10-21).** The defects in the last column changed as follows:

- **`ResourceStatus`**: still set only by `set_status`, but a source that failed is not asked again
  in the same investigation (§2.2.5).
- **`ConnectorStatus`, `SourceState`**: Partial with zero records reads `Unavailable` (§2.2.7).
- **`RelationStatus`**: `Reviewed` is produced by `crane capture confirm` (§2.7.3).
- **`AssertionStatus`**: `Contradicted` and `Superseded` stay reserved, by design. Marking an older
  assertion would change the stored copy that capability intake compares with the copy inside a
  pack, so every earlier pack would read as changed.
- **`ReplayFidelity`**: recovered sources are reported (§2.12).
- **`ContributionStatus`**: `expire` runs at the start of every investigation (§2.10.3).
- **Episode outcome**: evidence must exist and belong to the episode; changing someone else's
  outcome needs review rights; earlier outcomes are kept (§2.11.1).
- **`QuestionState`**: a real lifecycle, Open → Answered / Blocked / Unresolvable, Blocked → Open,
  driven by `crane capture ask` and `crane capture question` (§2.11.2).

---

## Part 4. Control-flow cases: how the component responds

For each case: the **trigger**, the **path** through the code, the **response**, what **the pack**
ends up containing, and the **weak point** with its criticality.

**C1. The question fits no recipe.**
- **Trigger:** class `slow-api`, no plan.
- **Path:** [`select_recipe`](../../src/capture/broker/run.rs#L565).
- **Response:** error "no recipe for 'slow-api'". Nothing is queried.
- **Pack:** none, and **no audit**.
- **Weak point:** the refusal is invisible afterwards. **MEDIUM.**
- **Resolved (2026-10-21):** the refusal is stored as a `Rejected` audit (§2.1.8). A
  `deployment-discovery` and a `pr-review` recipe now exist (§2.3.1). **NONE.**
- **Changed later (2026-10-21, with Insight):** a class no recipe serves is no longer refused. The
  broker asks each permitted source to discover what it holds and derives a bounded exploration from
  the registry (one query per resource type and evidence type the actor may receive, filtered by the
  known scope values), validated, audited, and replayable like any recipe. The pack is
  `exploratory`, never `sufficient`. Only a named recipe that does not exist, an invalid plan, or
  having no permitted source to explore is still an audited refusal. People's feedback on
  explorations is what Insight learns recipes from (see [GUIDE.md](../surfaces/cli.md#insight-learning-from-work)).

**C2. A proposed plan is malformed.**
- **Trigger:** a filter `"9f1c*"`.
- **Path:** [`from_plan`](../../src/capture/broker/recipe.rs#L792), then
  [`check_template`](../../src/capture/broker/recipe.rs#L289) refuses `*`.
- **Response:** an error; nothing runs. Same weak point as C1. **MEDIUM.**
- **Resolved (2026-10-21):** audited, as C1. **NONE.**

**C3. A needed fact is unknown.**
- **Trigger:** the recipe needs `{deployment}` and none is known.
- **Path:** `fill` fails ([run.rs:951](../../src/capture/broker/run.rs#L951)).
- **Response:** the step is `NotQueried`: "scope value 'deployment' is unknown".
- **Pack:** the missing Deployment is `NotQueried`, with the variable named. That is actionable.
- **Weak point:** none. **NONE.**

**C4. A fact has several values.**
- **Trigger:** a pull request with commits A, B, C.
- **Path:** [`variable`](../../src/capture/resources/scope.rs#L249) returns A.
- **Response:** CI and tests are fetched for A only.
- **Pack:** looks complete. **No warning.** **HIGH.**
- **Partly resolved (2026-10-21):** every commit is learned, and a Completeness warning names B and
  C as not checked (§2.3.3). The steps still run for A only. **LOW.**

**C5. The request explicitly asks for something forbidden.**
- **Trigger:** environment `production` is named; the actor may only see `staging`.
- **Path:** an explicit denial in [`resolve`](../../src/capture/resources/scope.rs#L114), then the gate
  at [run.rs:661](../../src/capture/broker/run.rs#L661).
- **Response:** `Rejected`, zero queries.
- **Pack:** an Access warning; the run is audited. **NONE.**

**C6. A source is implicitly forbidden.**
- **Trigger:** the policy does not include `logs`.
- **Path:** [run.rs:839-879](../../src/capture/broker/run.rs#L839-L879).
- **Response:** the `deploy-logs` step is `Denied`.
- **Pack:** RuntimeLog missing with state `Denied`, and an Access warning naming the source. **NONE.**

**C7. A source is down, empty, or returns garbage.**
- **Down:** `Failed`, and the missing reason is `Unavailable`.
- **Empty:** `Executed` with zero records, and the missing reason is `Empty`.
- **JSON missing fields:** `PartiallyParsed`, which still counts as present.
- **Not JSON at all:** `Failed` parse, which does **not** satisfy a requirement, so the reason is
  `ParseFailed`.
- **Weak point:** Partial with zero records is reported as `Empty`. **MEDIUM.**
- **Resolved (2026-10-21):** it is reported as `Unavailable` (§2.2.7). **NONE.**

**C8. Wrong or mismatched identifiers (context not provided properly).**
- **Wrong deployment id:** fetch returns nothing (`Empty`); every step that uses `{deployment}` is also
  `Empty`. There is no "did you mean…". **MEDIUM.**
- **Tracker key instead of Crane's task id:** DataSea answers `Empty` even though the agent worked
  ([local.rs:283](../../src/capture/resources/local.rs#L283)). **HIGH.**
- **Repository spelled differently from the resource scope:** if the policy does not list that
  spelling, the request is `Rejected`; if the policy lists it but the resources do not, every source is
  implicitly denied ("does not cover this repository"). **MEDIUM.**
- **After the fixes (2026-10-21):** the tracker key now matches (§2.2.13), and commit and
  environment keys are trimmed and lowercased (§2.6.5). A wrong deployment id and a differently
  spelled repository behave as before. **MEDIUM** for those two.

**C9. A fact learned mid-run changes what is allowed.**
- **Trigger:** no environment is known at first; the deployment record teaches `production`; the
  actor may only see `staging`.
- **Path:** check E ([run.rs:823](../../src/capture/broker/run.rs#L823)) fails for **every later step**,
  including policies, whose evidence has no environment.
- **Response:** later steps `Denied`. But the deployment record has already been learned from and
  will be used in assertions. **CRITICAL** (see Part 5, Example 3).
- **Resolved (2026-10-21):** the deployment record is checked as it arrives. It is withheld, nothing
  is learned from it, and because it reveals a forbidden environment the investigation stops as
  `Rejected` (§2.1.6). Environment-free steps are no longer denied by a learned environment
  (§2.5.6). **NONE.**

**C10. The evidence disagrees.**
- **Trigger:** the smoke check says unhealthy, the canary says healthy.
- **Response:** `deployment_succeeded` is `Unresolved` with both sides kept; `change_verified` is
  Unknown; a Conflict warning is added ([tests.rs:1040](../../src/capture/tests.rs#L1040)). **NONE.**

**C11. Adding a new resource.**
- *A second instance of a known kind* (another CI served by an existing connector type):
  1. write a `ResourceDefinition` with an explicit scope;
  2. [`register`](../../src/capture/resources/registry.rs#L365);
  3. [`connect`](../../src/capture/broker/run.rs#L470);
  4. add it to each actor's `AccessPolicy.resources`.

  Every recipe step with a matching type, evidence, and operation now calls it automatically
  ([run.rs:839](../../src/capture/broker/run.rs#L839)); no recipe changes. **Weak point:** it is called
  *in addition to* the existing source, so budget use doubles. **LOW.**
  **Resolved (2026-10-21):** a `priority` on the definition makes the step ask the better source
  first, and the other only when it fails (§2.3.2). **NONE.**
- *A new kind of source* (a live GitHub Actions connector):
  1. add a [`ConnectorSpec`](../../src/capture/resources/registry.rs#L150);
  2. implement [`ResourceConnector`](../../src/capture/resources/connector.rs#L256): copy every filter
     key into metadata, return a real snapshot, and map HTTP 401/403 → `Unauthorized`, 429 →
     `RateLimited`, 5xx → `Unavailable`;
  3. register and connect it in [`local_broker`](../../src/cli/capture.rs).

  **Weak points:** cursors are not followed yet (§2.2.10) and credentials are not resolved yet
  (§2.2.4). **HIGH** for any real hosted source.
  **After the fixes (2026-10-21):** cursors are followed (§2.2.10), deadlines are passed to every
  call (§2.3.7), and the GitHub connector shows the pattern end to end, using the GitHub CLI's own
  login (§2.2.12). Credential resolution waits for the first connector that needs it. **LOW.**
- *A new evidence type:* add it to [`EvidenceType`](../../src/capture/format/record.rs#L33), then update
  `payload_kind`, `expected_fields`, the [link rules](../../src/capture/correlate/link.rs#L123), and the
  [ranker](../../src/capture/ingest/selection.rs#L116). Rust's exhaustive `match` forces the last two,
  so nothing is silently forgotten. **NONE.**
- *A new relation word:* add it to [`RelationType`](../../src/capture/correlate/model.rs#L144) and
  `check_endpoints`, then bump `VOCABULARY_VERSION`. **NONE.**

**C12. The budget runs out.**
- **Between steps:** remaining steps are `BudgetExhausted` and the status is `BudgetExhausted`.
- **Inside a step:** ingestion stops and the step records why.
- **Weak point:** a hanging connector is never stopped. **MEDIUM.**
- **Partly resolved (2026-10-21):** each connector call receives the investigation's deadline; Git,
  GitHub, and fixtures answer `Timeout`. Crane's own verification still cannot be interrupted
  (§2.3.7). **LOW.**

**C13. The pack is too large.**
- [`seal`](../../src/capture/ingest/pack.rs#L397) drops the lowest-ranked evidence and adds a Truncation
  warning listing it. **LOW** (the context view keeps listing the dropped ids).
- **Resolved (2026-10-21):** the view is pruned and its id recomputed (§2.9.7). **NONE.**

**C14. A secret is in the content.**
- It is redacted and the artifact becomes `Confidential`. Actors with a lower ceiling get an Access
  warning instead of the record.
- A record declared `Secret` is withheld: its parse fails with "withheld", so the gap is visible.
- **Weak point:** secret formats the redactor does not recognize. **MEDIUM.**
- **Partly resolved (2026-10-21):** runtime logs and tool calls are at least Confidential whatever
  the redactor found (§2.1.5). **LOW.**

**C15. A duplicate delivery or an updated record.**
- A retried webhook or a re-query is `Duplicate` (same evidence id, path appended).
- An updated record is `NewRevision`, with a SUPERSEDES edge. **NONE.**

**C16. Two investigations at the same time.** The second save erases the first one's new records
([store.rs:214](../../src/capture/store.rs#L214), [store.rs:288](../../src/capture/store.rs#L288)).
**CRITICAL.**
- **Resolved (2026-10-21):** each save re-reads and merges under the lock, so both runs keep their
  records (§2.13.3). **NONE.**

**C17. An agent proposes context for a zone.**
- **Expected:** the proposal is stored as `Proposed` until a different human reviews it.
- **Also happens:** the zone entity's version is overwritten with whatever the agent supplied,
  even if the proposal is refused ([store.rs:724-734](../../src/capture/store.rs#L724-L734)). **HIGH.**
- **Resolved (2026-10-21):** the proposal is validated first, its zone must already be observed, and
  it never writes the entity (§2.7.1). **NONE.**

**C18. Someone records "Succeeded" for an episode.** It is accepted with **any** evidence ids,
including ones that do not exist ([episode.rs:129](../../src/capture/ingest/episode.rs#L129)). **HIGH.**
- **Resolved (2026-10-21):** each evidence id must exist and belong to the episode, and changing
  someone else's outcome needs review rights (§2.11.1). **NONE.**

---

## Part 5. Real-world examples, traced step by step

### Example 1: the vague on-call question

**What a real person writes** (to an agent in Slack, or in a terminal):
> "The refund thing broke prod yesterday afternoon. What happened?"

**Stage 0: turning words into a request.** Work Capture does not read words. The `question` is a
label ([run.rs:93](../../src/capture/broker/run.rs#L93)). Something upstream (an agent, a bot, or a
person) must build an `EvidenceQueryRequest`. *Why it is designed this way:* if the component
interpreted text, the same sentence could retrieve different evidence on different days, and no audit
could explain why. The ambiguity is pushed to the boundary, where it is visible.

**Stage 1: the first request.** From the words, an agent can only infer:

```
investigation_class = "deployment-failure"        ("broke prod")
known.environment   = "production"                ("prod")
known.repository    = "acme/payments"
time_window         = yesterday 12:00–18:00       ("yesterday afternoon")
task, pull request, deployment, commit = unknown
```

| # | What happens | Values |
|---|---|---|
| 1 | Recipe | `for_class("deployment-failure")` gives `failed-deployment` v1 |
| 2 | Scope | `unknown = [task, branch, commit, pull_request, deployment, session]`; the episode is **new and untracked** |
| 3 | `task`, `pull-request` | `{task}` and `{pull_request}` unknown, so `NotQueried` |
| 4 | `code-change`, `ci`, `tests` | `{commit}` unknown, so `NotQueried` |
| 5 | `agent`, `deployment`, `verification` | unknown values, so `NotQueried` |
| 6 | `deploy-logs`, `runtime-metrics`, `schema` | no Deployment record was collected, so the predicates are false: `Skipped` |
| 7 | `policies` | runs, but the **time window** drops timeless records ([connector.rs:325](../../src/capture/resources/connector.rs#L325)): `Empty` |
| 8 | Assertions | no subjects known, so **none at all** |
| 9 | Missing | Task, PR, CodeChange, CiRun, TestReport, Deployment, all `NotQueried`, each naming the unknown variable. RuntimeLog and Metric are *not* required, because their condition could not hold (§2.3.5) |
| 10 | Status | `Incomplete` |

What the person sees: *"I need a deployment id: every required item is NotQueried because the
deployment, pull request, and task are unknown."* That is honest, but not yet useful.

**Stage 2: resolving the ambiguity with a discovery plan.** No recipe can *find* deployments, so the
agent proposes one step ([`from_plan`](../../src/capture/broker/recipe.rs#L792)):

```json
{ "plan": [{ "step_id": "find-deployments", "resource_type": "deployment",
             "operation": "query", "evidence_type": "deployment",
             "filters": { "environment": "{environment}" }, "limit": 20,
             "when": { "op": "always" } }],
  "required_evidence": ["deployment"] }
```

| # | What happens | Values |
|---|---|---|
| 1 | Validation | template allowed, limit within 200, id valid; the recipe becomes `plan-<digest>` with a 16 / 200 / 1 MiB / 30 s budget |
| 2 | Query | `QuerySpec{environment: production, window, limit 20}` validates against `deploy` |
| 3 | `page` | keeps deployment records with `environment=production` and a timestamp in the window, which returns **`dep-7731`** |
| 4 | Learn | commit `9f1c2ab7d4` (from the deployment). **The deployment id itself is not learned** (§2.4.4) |
| 5 | Early stop | Deployment was required and is present, so `Satisfied` |
| 6 | Pack | `Sufficient` for this plan; evidence: `dep-7731` failed, production, commit `9f1c…` |

*If two deployments failed yesterday*, both are listed, but the scope's commit is that of whichever was
ingested **first** (§2.4.4, HIGH). A careful agent must read the evidence list, not `scope.commits`.

**Stage 3: the real investigation.** The agent re-issues `failed-deployment` with
`known.deployments = ["dep-7731"]` and the **same `episode_id`** as stage 1. Without it, a third
unrelated episode is created (§2.11.1).

| Step | Result |
|---|---|
| `deployment` | `failed`; learns commit `9f1c…` and environment `production` |
| `code-change`, `ci`, `tests` | 3 files; CI `success`; tests `Empty` |
| `task`, `pull-request`, `agent` | `NotQueried`: no task, and a deployment never teaches a PR |
| `deploy-logs` | the predicate holds: the log is collected, the password is redacted, the record is `Confidential` |
| `runtime-metrics` | `Unavailable` |
| `schema` | `Skipped` (the deployment says `migration: "false"`, although the commit adds a migration; §2.3.4) |
| `verification` | smoke check `healthy: false` |
| `policies` | `Empty` if yesterday's window was kept (§2.2.10) |
| Assertions | ci_passed **Yes**; tests_passed **Unknown**; deployment_succeeded **No** (two independent sources) |
| Missing | Task, PR (`NotQueried`), TestReport (`Empty`), Metric (`Unavailable`) |

**The final answer the person gets:** *"dep-7731 failed in production. The deployment record and the
smoke check agree. The log shows `KeyError 'idempotency_key'` on a job queued before the deploy. CI
passed, but no test report exists and metrics were unreachable. I could not link this to a task or
pull request, because the deployment does not name one."* Every clause traces to an evidence id or a
missing-evidence entry.

**After the fixes (2026-10-21).**
- **Stage 1.** RuntimeLog and Metric stay required, as `NotQueried`, because their condition could
  not be evaluated (§2.3.5). The policies step keeps its governance evidence despite the time window
  (§2.2.10). The empty assertion list comes with an Uncertainty warning (§2.8.3), and the unknown
  starting condition gets a Completeness warning (§2.3.1).
- **Stage 2.** No hand-written plan is needed: class `deployment-discovery` selects the
  `deployment-discovery` recipe (§2.3.1), and the deployment id is learned into scope (§2.4.4). If
  two deployments failed, both commits are learned, and a warning names the one that was not
  checked (§2.3.3).
- **Stage 3.** `failed-deployment` v2 is the default for the class, so the schema step runs, because
  the commit touches `migrations/` (§2.3.4). From the CLI, `episode EPI` keeps the stages in one
  episode (§2.11.1). The final answer can now add what the schema evidence says about the migration.

### Example 2: a developer checks their own agent's work (live sources)

**What a real person types:**
> "What did Claude change in my last commit, and was it within our rules?"
> `crane capture investigate task PAY-142`

| # | What happens | Code | Values |
|---|---|---|---|
| 1 | Build the request | [cli/capture.rs](../../src/cli/capture.rs) | commit = `HEAD`, repository = remote label, recipe **forced** to `local-change` v1, task `PAY-142`, policy grants all four sources |
| 2 | `commit` | Git fetch | one CodeChange record (files, 8 KB diff) |
| 3 | `agent-calls` | DataSea query `task=PAY-142` ([local.rs:283](../../src/capture/resources/local.rs#L283)) | filters on Crane's **canonical** id `tsk_…`; `PAY-142` is an external key, so **0 records: `Empty`** |
| 4 | `landscape` | Landscape query | every zone and flow, as GraphState records |
| 5 | `policies` | Crane query | the policy blocks |
| 6 | `validation` | Crane fetch, which runs verification ([local.rs:568](../../src/capture/resources/local.rs#L568)) | one Validation record (`passed: true` or `false`) |
| 7 | Governance link | [link.rs:439](../../src/capture/correlate/link.rs#L439) | `fees.py BELONGS_TO_ZONE payment-core@v2`; the zone is GOVERNED_BY `payments` |
| 8 | Assertions | | ci_passed and tests_passed **Unknown** (no CI source); change_verified Unknown; **no assertion about validation** (§2.8.3) |
| 9 | Missing | | ToolCall `Empty`, detail "agent-calls: Ok" |
| 10 | Status | | `Incomplete`; saved in the trust directory |

**What the person sees:** *"Commit 3a1f… changed `app/payments/fees.py` (zone payment-core v2,
governed by `payments`). Validation passed. **No agent activity was found for this task.**"*

**Cause and effect of the misleading last line.**
- **Cause:** there are two identity systems (canonical and external, by design;
  [events.rs:58-59](../../src/telemetry/events.rs#L58-L59)), and the connector filters on only one of
  them.
- **Effect:** an `Empty` that reads as "no agent was involved".
- **Repair:** run with the `tsk_…` id (shown by `crane task current`), or make the connector match
  `external_task_id` as well (HIGH).

*Variant: the rules were edited but not signed.*
- Verification still produces a report with `passed: false`. The pack shows the record, but **no
  conclusion and no warning** reflects it (HIGH).
- If verification cannot run at all, the Crane connector answers `Partial`. `fetch("validation")`
  then returns zero records, so the missing reason reads **`Empty`**, "nothing exists", instead of
  "could not be computed" (MEDIUM). Only the extra Completeness warning tells the truth.

**After the fixes (2026-10-21).**
- **Step 3** matches the tracker key too, so the agent's tool calls are collected and linked to the
  episode (§2.2.13). The misleading last line becomes a list of the agent activity found.
- **Step 6** runs verification once per investigation (§2.2.13).
- **Step 8** adds `policies_hold` for the repository (Yes or No, from validation) and
  `governed_zone_touched` for the commit, listing `payment-core` (§2.8.3).
- When the repository is connected to GitHub, its pull request and check runs are read live
  (§2.2.12), so ci_passed can be settled.
- **Variant.** Unsigned rules now give `policies_hold = No`. When verification cannot run, the
  missing validation reads `Unavailable`, not `Empty` (§2.2.7).

### Example 3: a contractor with limited access

**Who:** an external contractor. Their policy: every source except `logs`; environment `staging`
only; maximum sensitivity `Internal`.

**What they ask:**
> "Can you check how my refund change PAY-142 is doing?"

The request carries task `PAY-142`, pull request `418`, deployment `dep-7731`, and **no environment**.

| # | What happens | Code | Values |
|---|---|---|---|
| 1 | Scope | [scope.rs:114](../../src/capture/resources/scope.rs#L114) | no environment named, so no explicit denial; not Rejected |
| 2 | Steps task → agent | | the environment is unknown and `permits_environment(None)` is true, so they run; the commit is learned |
| 3 | `deployment` | | runs; record `{environment: production, status: failed}` is ingested and linked; **learns `environment = production`** ([scope.rs:290](../../src/capture/resources/scope.rs#L290)) |
| 4 | Every later step | [run.rs:823](../../src/capture/broker/run.rs#L823) | `Denied` (environment production not permitted), **including `policies`** |
| 5 | Assertions | [run.rs:1238](../../src/capture/broker/run.rs#L1238) | derived from **all collected records**, so `deployment_succeeded(dep-7731) = No` |
| 6 | Selection | [run.rs:1319](../../src/capture/broker/run.rs#L1319) | excludes the deployment record as `Unauthorized`; Access warning "1 records were withheld" |
| 7 | Pack | [run.rs:1418-1420](../../src/capture/broker/run.rs#L1418-L1420) | **still contains** `scope.environment = "production"` with a learned fact citing the hidden record, and the assertion "deployment_succeeded for dep-7731: no" |

**What the contractor learns:** *"Your change was deployed to production and the deployment failed.
Some records were withheld."* Both facts were forbidden to them.

**Cause and effect.**
- **Cause:** permission is checked at the gate, at each step, and at selection. But the first record
  that *reveals* the environment arrives while it is still unknown, and two consumers (`learn` and
  `derive`) use that record before selection runs.
- **Effect:** a privacy leak, without the record itself ever being shown.
- **Repair (CRITICAL):**
  1. check eligibility for each record inside `call`, before learning from it or collecting it;
  2. derive assertions only from records that pass eligibility;
  3. treat a forbidden, newly learned environment like an explicit request: stop, and mark the
     investigation `Rejected`.

**After the fixes (2026-10-21).** All three repairs were made (§2.1.6). At step 3, the deployment
record is checked as it arrives. It is withheld (environment `production`), nothing is learned from
it, and because it reveals a forbidden environment the investigation stops as `Rejected`. The pack
contains no `production`, no assertion about `dep-7731`, no learned fact citing hidden evidence, and
an Access warning that counts withheld records without naming them. What the contractor learns: the
investigation was rejected because its evidence reached a scope they may not access.

---

## Part 6. How people should see it: the Evidence Receipt

### 6.1 What it is and why it exists

**Definition.** An **Evidence Receipt** is a short, fixed-format rendering of one Evidence Pack,
attached wherever a decision about the work is made: a pull-request check, a CI summary, or a deploy
approval.

**The cause it answers.** Today a CI badge is green or red. It has no color for "nobody checked", so
**absence of evidence is displayed as success**. The receipt exists to give "unknown" its own visible
mark **and a reason**, because each reason calls for a different action.

### 6.2 The receipt for the story, line by line

```
EVIDENCE RECEIPT · PAY-142 "Make refunds idempotent"          ◐ INCOMPLETE
Q: Why did deployment dep-7731 of PAY-142 fail?
Recipe failed-deployment v1 · digest 7c2e… · 13 source calls · stop: completed

VERDICTS
 ✔ ci_passed             commit 9f1c2ab7d4   YES      1 independent source
 ◌ tests_passed          commit 9f1c2ab7d4   UNKNOWN  no evidence either way
 ✘ deployment_succeeded  dep-7731            NO       2 independent sources
 ✘ change_verified       PAY-142             NO       derived from the 3 above

GAPS (required, not satisfied)
 TestReport   EMPTY        ci answered: no test report exists for this commit
 Metric       UNAVAILABLE  metrics source failed: Unavailable

DECIDED NOT TO LOOK
 schema       skipped      condition deployment.migration == "true" did not hold

GOVERNANCE TOUCHED
 zone refunds@v3 · flow refund-flow@v2 · policy refund_safety

SAFETY
 1 secret redacted in a runtime log (Confidential)

UNVERIFIED IDEAS (agent-proposed, not trusted)
 "Jobs queued before the deploy lack idempotency_key…"   cites: log-7731-1

INTEGRITY  manifest a91f… · replay: EXACT
```

| Receipt line | **Cause**: which data produces it | **Effect**: what it should make happen |
|---|---|---|
| Status `◐ INCOMPLETE` | `pack.status`, computed at [run.rs:1260](../../src/capture/broker/run.rs#L1260) | A gate rule: **pass** only if `Sufficient` and `change_verified = Yes`; **block** on any `No`; **ask a human** on any `Unknown`. Unknown deliberately does not auto-block, because gates that block on every gap get switched off |
| Recipe line | `pack.retrieval` (id, version, digest, steps, stop reason) | Anyone can reproduce the investigation. "v1" means one plan forever (§2.3.9), so a dispute is settled by replaying, not by arguing |
| ✔ / ◌ / ✘ verdicts | `pack.assertions`, built by the decision table (§2.8.2); "independent sources" is `independent_support` | ✔ from one source: decide whether one source is enough for this risk. ◌: go to GAPS. ✘ from two sources: the failure is corroborated, so move on to *why*. A **⚠ CONFLICT** line (both sides present) means a human must adjudicate |
| GAPS | `pack.missing_evidence`, with each reason chosen by priority (§2.9.2) | **Each reason routes to a different person.** `EMPTY` → the developer or agent produces the evidence. `UNAVAILABLE` → platform on-call repairs the source. `DENIED` → the access owner. `NOT QUERIED` → a Crane admin supplies the id or connects a source. `PARSE FAILED` → the integration owner |
| DECIDED NOT TO LOOK | `retrieval.branches` with `held = false` | Makes the recipe's assumptions visible. Here a human can spot that the commit *does* add a migration although the deployment says it did not |
| GOVERNANCE TOUCHED | context view entities, BELONGS_TO and GOVERNED_BY edges with version scopes (§2.7.6) | Tells reviewers the risk class, and pins the zone version in force, so later drift is detectable |
| SAFETY | artifact redactions and sensitivity (§2.6.3) | Confidence that the receipt is safe to share. It also exposes an application that logs passwords, which is a separate finding |
| UNVERIFIED IDEAS | `pack.contributions` that are Proposed and agent-proposed | A human may accept one through review (§2.10.3). From then on it travels as reusable, human-approved context for zone `refunds`. The system learns without agents writing their own memory |
| INTEGRITY | the manifest digest (§2.9.7) and the replay verdict (§2.12) | The receipt can be audited later; `DIVERGED` means the world has changed since it was issued |

### 6.3 How the receipt is produced (the mechanism)

1. **A trigger fires:** a pull request is opened or updated, or a deployment finishes. CI runs Crane
   under a human or service identity, never inside an agent session (Crane refuses agent sessions).
2. **An investigation runs.** `crane capture investigate … --json` exists today, but it only uses the
   `local-change` recipe. A deployment receipt needs the hosted connectors (§2.2.12).
3. **A renderer maps the pack's JSON to the layout above.** No new logic is needed, because every
   line is a field that already exists.
4. **The receipt is posted** as a GitHub check run (conclusion `success`, `neutral`, or `failure` by
   the gate rule), with the receipt as its summary.
5. **When new evidence arrives,** re-running produces a new pack and an updated receipt. The old one
   stays in the audit.

**After the fixes (2026-10-21).** Step 2 is no longer limited to local changes: `investigate` takes
`class` or `recipe`, and GitHub pull requests and check runs are read live. A deployment receipt
still needs live deployment, log, and metrics connectors. Under `failed-deployment` v2, the
receipt's DECIDED NOT TO LOOK line for `schema` disappears, because the step runs when the commit
touches `migrations/`. The renderer of step 3 and the check-run posting of step 4 are not built yet.

### 6.4 What receipts cause beyond one pull request

- **Blind-spot map.** For each zone, the share of receipts with gaps, by reason. *Effect:* teams see
  where agents work unverified, and invest in tests or connectors exactly there.
- **Evidence debt.** Each incomplete receipt adds debt items (zone, missing type, reason). *Effect:*
  the debt can be burned down like technical debt and trended over time.
- **Autonomy dial.** A zone whose receipts are consistently `Sufficient` has earned more agent
  autonomy. *Effect:* widening agent permissions becomes a decision backed by data.

---

## Part 7. Ambiguity and scaling

### 7.1 Seven kinds of ambiguity, and how each is handled

**Definition.** A question is **ambiguous** when more than one answer is possible and the available
information does not decide between them.

| Kind | Example | What the component does | Code | Where it still fails | Criticality |
|---|---|---|---|---|---|
| **A. What to look for** | "the refund thing broke" | No free-text reading; a class picks a recipe; plans are validated like recipes | [recipe.rs:464](../../src/capture/broker/recipe.rs#L464), [recipe.rs:792](../../src/capture/broker/recipe.rs#L792) | No recipe means an unaudited error | MEDIUM |
| **B. Which object** | "which commit?" | An unknown value makes the step `NotQueried`, never widened; learning keeps lineage | [run.rs:951](../../src/capture/broker/run.rs#L951), [scope.rs:271](../../src/capture/resources/scope.rs#L271) | Several values use the first one silently | HIGH |
| **C. Which link** | "is this PR from this task?" | Observed, declared, or inferred, stored on each edge; never linked by timing | [model.rs:175](../../src/capture/correlate/model.rs#L175) | Branch-name inference is weak, but it is labelled inferred | NONE |
| **D. Which fact is true** | healthy vs unhealthy | Both sides are kept as Unresolved; copies count once | [assertions.rs:212](../../src/capture/correlate/assertions.rs#L212) | Mirrors with different bytes count twice | LOW |
| **E. Bad data** | a field is missing | Partial parses with named issues; Unobserved instead of 0 | [parsers.rs:613](../../src/capture/format/parsers.rs#L613) | Unfamiliar reference names link to nothing | MEDIUM |
| **F. Who says so** | an agent's theory | Derivation method on every assertion; hypotheses stay Proposed; namespaces need review | [assertions.rs:538](../../src/capture/correlate/assertions.rs#L538), [contributions.rs:389](../../src/capture/ingest/contributions.rs#L389) | A proposal can still rewrite an entity's version | HIGH |
| **G. Is it still true** | an old log | Event time kept apart from ingestion time; freshness warnings; snapshots; replay | [parsers.rs:570](../../src/capture/format/parsers.rs#L570), [run.rs:1761](../../src/capture/broker/run.rs#L1761) | Freshness checks only run when a limit is set; replay misses recovered sources | MEDIUM |

**After the fixes (2026-10-21).**
- **A**: refusals are audited, and discovery and PR-review recipes exist. **LOW.**
- **B**: every value is learned, and the values not checked are named in a warning. **LOW.**
- **D**: unchanged, by design. **LOW.**
- **E**: two more commit field names, and normalized commit and environment keys; there is no
  per-connector map. **LOW.**
- **F**: a proposal can no longer change an entity. **NONE.**
- **G**: replay reports recovered sources, but freshness is still checked only when a limit is set.
  **LOW.**

**Real-time questions** ("is it healthy *right now*?"):

- The component is pull-based. It has no live stream, and nothing outside the tests feeds the
  webhook path.
- Each step's answer is pinned to its own snapshot and time. There is no single snapshot across
  sources, and none can exist, because the sources are independent. The audit records each one, so
  the component never claims a consistency it does not have.
- Time budgets apply between steps only.

So the component answers *"what was true, as observed at these recorded moments"*, and it is honest
that this is not *"what is true now"*. After the fixes, the time budget is also a deadline
passed to each connector call; only Crane's own verification cannot be interrupted.

### 7.2 Scaling: what grows, where it breaks, how to fix it

**Definition.** To **scale** means handling more of something without losing guarantees or becoming
slow. Six things grow:

| What grows | Where it breaks today | Why | Fix | Why the fix keeps the guarantees | Criticality |
|---|---|---|---|---|---|
| **History** | [`open`](../../src/capture/store.rs#L214) loads everything; [`save`](../../src/capture/store.rs#L288) rewrites everything; the lock gap loses writes | whole-file JSONL | Append-only logs with persisted indexes, then a database (the hosted service already has MongoDB) | Content-based ids (§2.1.2) make writes idempotent upserts, so many writers converge | CRITICAL (race), MEDIUM (cost) |
| **Sources and slowness** | Steps run one after another; no call deadlines; DataSea reads its whole log ([local.rs:264](../../src/capture/resources/local.rs#L264)) | sequential, synchronous design | A dependency graph of steps: a step depends on earlier evidence when its predicate looks at evidence, or when a template uses a variable `learn` can fill. Independent steps run in parallel. Add deadlines and indexes | Merge results back **in recipe order** before writing the audit or evaluating predicates, so the audit is identical to a sequential run | MEDIUM |
| **Data per source** | Cursors are never followed ([run.rs:1060](../../src/capture/broker/run.rs#L1060)) | not implemented | Follow cursors within the record budget, or warn | Budgets still cap the total | HIGH |
| **Graph size** | [`link_governance`](../../src/capture/correlate/link.rs#L439) compares every file with every zone on every run | full recompute | Re-link only files touched by this run; recompute everything only when a zone or flow version changes | Traversal is already capped (6 / 1000 / 1 MiB), so query cost does not grow | MEDIUM |
| **Repositories and teams** | One local store per machine | local-first design | Shard by repository; investigations of different episodes share only idempotent entities | Every record carries its repository, and every request carries its own access policy | LOW |
| **Freshness** | Pull-only | no receiver | Webhooks into `ingest` (delivery dedupe exists) plus cursor-based incremental pulls (cursor state exists) | Investigations read the local store first and only contact sources whose snapshot changed | MEDIUM |

The cost of one investigation is bounded by its budget, its pack limits, and the traversal caps, no
matter how much data exists. Scaling is therefore about removing the few places that touch *all*
data: store load and save, governance re-linking, and DataSea reads.

**After the fixes (2026-10-21).**
- **History**: the race is gone (each save merges under the lock), and unchanged files are not
  rewritten. Reading and merging everything remains. **MEDIUM.**
- **Sources and slowness**: connector calls have deadlines, DataSea is read once per investigation,
  and the clock is thread-safe. Steps still run one after another. **LOW.**
- **Data per source**: cursors are followed, or truncation is warned. **NONE.**
- **Graph size**: governance re-links only what changed. **NONE.**
- **Repositories and teams**: unchanged. **LOW.**
- **Freshness**: unchanged. **MEDIUM.**

---

## Part 8. The complete register of break points

| # | What breaks | Where | Fix | Criticality |
|---|---|---|---|---|
| 1 | Facts from forbidden records leak through learned scope and assertions | [run.rs:1128](../../src/capture/broker/run.rs#L1128), [run.rs:1238](../../src/capture/broker/run.rs#L1238), [scope.rs:290](../../src/capture/resources/scope.rs#L290) | Eligibility check per record at ingest; derive from eligible records only; reject on a forbidden learned environment | **CRITICAL** |
| 2 | Concurrent runs lose evidence and audits | [store.rs:214](../../src/capture/store.rs#L214), [store.rs:288](../../src/capture/store.rs#L288) | Hold the lock from open to save; append-only writes | **CRITICAL** |
| 3 | A proposal overwrites a zone or flow entity's version, even when refused | [store.rs:724-734](../../src/capture/store.rs#L724-L734), [graph.rs:163](../../src/capture/correlate/graph.rs#L163) | Never write entity attributes from contributions; validate before any graph change | **HIGH** |
| 4 | "Succeeded" outcomes accept non-existent evidence ids | [episode.rs:129](../../src/capture/ingest/episode.rs#L129) | Check that the evidence exists | **HIGH** |
| 5 | Only the first of several commits, PRs, or deployments is checked | [scope.rs:249](../../src/capture/resources/scope.rs#L249) | Iterate over values, or warn | **HIGH** |
| 6 | A time window drops all timeless governance evidence | [connector.rs:325](../../src/capture/resources/connector.rs#L325) | Exempt timeless types from the window | **HIGH** |
| 7 | Cursors are never followed; silent truncation | [run.rs:1060](../../src/capture/broker/run.rs#L1060) | Follow cursors, or add a Truncation warning | **HIGH** |
| 8 | Tracker key versus canonical task id: "no agent activity" | [local.rs:283](../../src/capture/resources/local.rs#L283) | Match `external_task_id` too | **HIGH** |
| 9 | A branch trusts one declared field; no path predicate | [recipe.rs:60](../../src/capture/broker/recipe.rs#L60) | Bind predicates to the subject; add a file-path predicate | **HIGH** |
| 10 | No conclusion for failed validation or a governed-zone touch | [assertions.rs:362](../../src/capture/correlate/assertions.rs#L362) | Add rules | **HIGH** |
| 11 | The first record ingested decides the commit | [scope.rs:277](../../src/capture/resources/scope.rs#L277) | Learn into lists and warn | **HIGH** |
| 12 | A learned environment denies environment-free steps | [run.rs:823](../../src/capture/broker/run.rs#L823) | Check only environment-bearing types | MEDIUM |
| 13 | Refusals returned as errors are not audited | [run.rs:565](../../src/capture/broker/run.rs#L565) | Write a Rejected audit | MEDIUM |
| 14 | Partial with zero records shows as Empty | [run.rs:1485](../../src/capture/broker/run.rs#L1485) | Map it to Unavailable | MEDIUM |
| 15 | Conditional requirements vanish when their subject is missing | [run.rs:1161](../../src/capture/broker/run.rs#L1161) | Report them as NotQueried | MEDIUM |
| 16 | No per-call deadline | [run.rs:369](../../src/capture/broker/run.rs#L369) | Deadlines on connector calls | MEDIUM |
| 17 | Recipe "v1" can change between releases | [recipe.rs:426](../../src/capture/broker/recipe.rs#L426) | Persist recipe digests | MEDIUM |
| 18 | Discovery results (deployments, tasks) are not learned | [scope.rs:271](../../src/capture/resources/scope.rs#L271) | Add learning rules; add a discovery recipe | MEDIUM |
| 19 | Follow-ups and replays create new episodes | [episode.rs:170](../../src/capture/ingest/episode.rs#L170) | Accept an explicit episode or conversation id | MEDIUM |
| 20 | Any human can overwrite an outcome | [episode.rs:129](../../src/capture/ingest/episode.rs#L129) | Require `may_review` | MEDIUM |
| 21 | Policy weakening is judged by a keyword | [contributions.rs:236](../../src/capture/ingest/contributions.rs#L236) | Rely on the Crane workflow; review the prose | MEDIUM |
| 22 | Governance re-linking grows forever | [link.rs:439](../../src/capture/correlate/link.rs#L439) | Incremental re-linking | MEDIUM |
| 23 | Verification runs twice per local investigation and cannot be interrupted | [local.rs:594](../../src/capture/resources/local.rs#L594) | Cache it per run; add a deadline | MEDIUM |
| 24 | Replay misses recovered sources | [run.rs:1761](../../src/capture/broker/run.rs#L1761) | Compare in both directions | MEDIUM |
| 25 | Unknown reference field names link to nothing | [parsers.rs:541](../../src/capture/format/parsers.rs#L541) | Per-connector field maps | MEDIUM |
| 26 | Only one recipe is reachable from the CLI; missing commands | [cli/capture.rs](../../src/cli/capture.rs) | Add commands | MEDIUM |
| 27 | Source health is never used | [registry.rs:410](../../src/capture/resources/registry.rs#L410) | Probe health, mark Unhealthy | LOW |
| 28 | `auth_reference` is never resolved | [registry.rs:247](../../src/capture/resources/registry.rs#L247) | Implement with the first hosted connector | LOW |
| 29 | Declared but unused: `Reviewed`, `Contradicted`, `Superseded`, the question lifecycle, `expire`, relation validity | various | Wire them up or remove them | LOW |
| 30 | Sealing leaves dropped ids in the context view | [pack.rs:397](../../src/capture/ingest/pack.rs#L397) | Prune the view after sealing | LOW |

The core ideas hold up: three-valued truth, typed gap reasons, immutable recipes, eligibility before
ranking, the controlled vocabulary, and human-only promotion of knowledge. Almost every break above
is one of two patterns:

- data is **used before its final check** (items 1, 3, 4, 12);
- one value **stands in for many** (items 5, 6, 11).

Both are local fixes that leave the architecture unchanged. Items 1 to 4 are the place to start.

### 8.1 The register after the fixes (2026-10-21)

| # | Status | How | New criticality |
|---|---|---|---|
| 1 | Resolved | Authorization per record on arrival; a revealed forbidden scope rejects the run; the view and learned facts carry only visible evidence | NONE |
| 2 | Resolved | Save re-reads and merges under the lock, by content id | NONE |
| 3 | Resolved | Validate first; target must be observed; versions change only through evidence | NONE |
| 4 | Resolved | Evidence must exist and belong to the episode | NONE |
| 5 | Partly resolved | Warning names the unchecked values; no per-value run (approved) | LOW |
| 6 | Resolved | `is_timed()`: timeless types ignore the window | NONE |
| 7 | Resolved | Cursor loop within budget; Truncation warning when pages remain | NONE |
| 8 | Resolved | DataSea matches `external_task` too | NONE |
| 9 | Resolved | Subject-bound `FieldEquals`, `AnyFileMatches`, `failed-deployment` v2 | NONE |
| 10 | Resolved | `policies_hold` and `governed_zone_touched` | NONE |
| 11 | Partly resolved | Every commit is learned; see #5 | LOW |
| 12 | Resolved | `is_environment_bound()` | NONE |
| 13 | Resolved | `audit_refusal` | NONE |
| 14 | Resolved | Partial with zero records maps to Unavailable | NONE |
| 15 | Resolved | `unevaluable` keeps the requirement as NotQueried | NONE |
| 16 | Partly resolved | Deadlines on connector calls; Crane verification is not interruptible | LOW |
| 17 | Resolved | Golden digests and store pins | NONE |
| 18 | Resolved | Deployments and tasks learned; `deployment-discovery` recipe | NONE |
| 19 | Resolved | Replay keeps the episode; `episode EPI` on the CLI | NONE |
| 20 | Resolved | Review rights needed; outcome history kept | NONE |
| 21 | Resolved | Allow-list of effects; the policy must be named | LOW |
| 22 | Resolved | Only new files and changed zones are re-linked | NONE |
| 23 | Partly resolved | Verification once per investigation; not interruptible | LOW |
| 24 | Resolved | Comparison in both directions | NONE |
| 25 | Partly resolved | More commit field names, normalized keys; no per-connector map | LOW |
| 26 | Resolved | Class, recipe, version, episode; eight new commands | LOW |
| 27 | Resolved | A failed source is not asked again in the same investigation | NONE |
| 28 | Left as designed | No connector needs a credential yet (GitHub uses the CLI's login) | LOW |
| 29 | Mostly resolved | `Reviewed`, the question lifecycle, and `expire` are wired up; `Contradicted` and `Superseded` stay reserved; relation validity is left unset | LOW |
| 30 | Resolved | The view is pruned and its id recomputed | NONE |

No CRITICAL or HIGH item remains. What is left is either a product gap (live deployment, log, and
metrics connectors), a cost at scale (the store's whole-history merge), or an approved trade-off
(checking only the first of several commits, with a warning).
