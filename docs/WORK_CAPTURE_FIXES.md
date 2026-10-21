# Work Capture: Break Register and Remediation Plan

Source: [WORK_CAPTURE.md](WORK_CAPTURE.md) (Component 1, `src/capture/`).
Criticality scale: CRITICAL (security, privacy, data loss) > HIGH (silently wrong or incomplete)
> MEDIUM (visible but misleading, or fails at scale) > LOW (ergonomic, latent, unused) > NONE.

Conventions used in every entry:
- Fix: the code change.
- Check: what must be confirmed so nothing else breaks.
- Test: tests to add. Unit tests go in `src/capture/tests.rs` and use the helpers `scenario_with`,
  `resource`, `broker`, and `investigate` (tests.rs:44-102); end-to-end tests go in
  `tests/capture.rs`.
- Class guard: a test or rule that also catches future issues of the same kind.

File references such as `run.rs:661` are relative to `src/capture/` unless a longer path is given.
They describe the code as it was when this register was written; line numbers have since moved.

---

## Resolution status (2026-10-21)

Every entry below has been worked through. Unit tests went from 197 to 259 and end-to-end capture
tests from 2 to 4; `cargo fmt`, Clippy (with and without `--features server`), and the full suite
pass. Three decisions were approved along the way: several known commits produce a warning rather
than a run per commit; `policies_hold` is reported beside `change_verified`, never folded into it;
and `RULES_VERSION` stays at 1 because the existing rules did not change.

**Fixed:**
- CRITICAL: 2.1.6 (records are checked for authorization as they arrive; a record revealing a
  forbidden scope stops the investigation as Rejected; the context view shows only entities the
  actor's evidence observed), 2.13.3(1) (save re-reads and merges under the lock).
- HIGH: 2.7.1, 2.11.1(1), 2.2.10(1), 2.2.10(2) and 2.9.6, 2.2.13 (task identity), 2.3.4
  (`failed-deployment` v2; v1 unchanged), 2.3.9(1) (golden digests and store pins), 2.8.3(1)
  (`policies_hold`, `governed_zone_touched`).
- MEDIUM: 2.1.4 (`Actor::human_operator`, review rights from the machine's owner or admin key),
  2.1.8, 2.2.7, 2.3.1 (`deployment-discovery`, `pr-review`), 2.3.5, 2.4.4(2), 2.5.6, 2.7.6(1),
  2.8.3(3), 2.10.2(2), 2.11.1(2), 2.11.1(3), 2.12(1), 2.2.12 (live GitHub connector), 2.14 (replay,
  verify, contribute, review, outcome, ask, question, confirm; class, recipe, version, episode).
- LOW: 2.1.1 (older-layout test), 2.1.2, 2.1.7, 2.2.5, 2.2.8 (connector contract harness), 2.2.11,
  2.2.13 (Git pages), 2.3.1 (starting conditions), 2.3.2 (priority), 2.3.6 (mandatory steps),
  2.3.9(2), 2.3.10, 2.5.5, 2.6.2(1), 2.6.4, 2.7.3, 2.9.7, 2.10.3, 2.11.2, 2.12(2), 2.13.3(3).

**Partly fixed, with what remains:**
- 2.3.3 / 2.4.4(1): every distinct commit is learned and the unchecked ones are named; steps do not
  run once per commit (approved).
- 2.13.3(2): unchanged files are not rewritten; save still reads and merges every collection.
- 2.3.7: Git, GitHub, and fixtures honor the investigation's deadline; Crane's own verification
  cannot be interrupted (it now runs once per investigation).
- 2.2.13 (DataSea cost): the log is read once per investigation; it is not indexed.
- 2.1.5 / 2.6.3: runtime logs and tool calls are at least Confidential; there is no entropy-based
  secret detection.
- 2.6.5: more commit field names and normalized commit and environment keys; no per-connector field
  map.
- 2.2.12: GitHub pull requests, commit files, and check runs are live; deployments, logs, and metrics
  still have no live connector, and GitHub has no test-report API.
- 2.1.9: an investigation's summary and metrics travel in its command's event; no dashboard flow
  charts them yet.

**Left as they are, by design:**
- 2.2.3: a new kind of source is reviewed code.
- 2.2.4(1): no connector needs a credential (the GitHub connector uses the GitHub CLI's own login);
  a resolver belongs with the first one that does.
- 2.2.4(2): the CLI's resources are defined in code, so there is no stored configuration to version.
- 2.5.1: the question is a label; recipes and plans decide what is retrieved.
- 2.6.2(2): a webhook receiver needs signed authentication of the sending source on the service.
- 2.7.2: no producer needs a new relation word yet.
- 2.7.4: no source states how long a relation holds.
- 2.7.5: validation evidence is already observed on its repository.
- 2.8.2: counting independence by source rather than content would weaken corroboration without
  metadata about mirrors.
- Part 3: assertion `contradicted` and `superseded` stay reserved. Marking an older assertion would
  change the stored copy that capability intake compares with the copy inside a pack, so every
  earlier pack would read as changed.

---

## A. CRITICAL

- **[2.1.6] AccessPolicy: facts from forbidden records leak into the pack (primary entry; also
  2.4.2, 2.4.4(3), 2.5.4, 2.5.7, 2.8.3(2), 2.9.3; Part 4 C9; Part 5 Example 3; Part 8 #1). CRITICAL**
  - Breaks: authorization is applied at three points: the rejection gate (run.rs:661), the per-step
    checks (run.rs:810, run.rs:823), and selection (run.rs:1319). Two gaps let forbidden data
    through:
    1. The record that first reveals an environment is fetched while the environment is still
       unknown, and `permits_environment(None) == true` (mod.rs:295) lets it in.
    2. Inside `call` (run.rs:921), only the record's TYPE is re-checked (run.rs:1084). Its
       environment, repository, and sensitivity are checked only at selection.
    Before selection, that record is (a) learned into scope (`scope.learn`, run.rs:1128,
    scope.rs:290), (b) used to derive assertions (`derive` reads every record in `run.collected`,
    run.rs:1238), and (c) recorded in `scope.learned`. The pack then carries
    `scope.environment = "production"`, a learned fact citing the hidden evidence id, and the
    assertion "deployment_succeeded for dep-7731: no" with that id as support (run.rs:1418-1420).
    A staging-only contractor learns that a production deployment failed. This is an
    information-disclosure defect; the raw record itself is never shown.
  - Fix:
    1. Make eligibility a single shared function. Expose `ineligible(record, &Criteria)`
       (selection.rs:144) to the broker and call it in `call` right after `self.store.ingest(...)`,
       before `link`, `learn`, and `collected.push`. If a record is ineligible: keep it in the store
       (the store is system-level), do NOT learn from it, do NOT add it to `collected`, record a
       record-level `deny` with the exclusion reason, and increment
       `metrics.authorization_denials`.
    2. Learning gate: if `learn` would set a repository or environment the policy does not permit,
       do not set it. Treat it as a learned explicit denial: stop the remaining steps with
       `StopReason::Rejected` and the detail "evidence revealed a scope the actor may not access".
       Rejecting is safer than continuing, because even "your later steps were denied" reveals
       that the scope moved.
    3. Derive assertions only from records that passed eligibility. Move `derive` after `select`,
       or filter `run.collected` with the same criteria.
    4. Filter `scope.learned` so it never cites an evidence id that is not in the pack.
    5. Keep the Access warning ("N records were withheld"), but make sure its `refs` list does not
       reveal ids of evidence the actor may not see. Today it lists them; replace them with a
       count.
  - Check:
    - The demo must be unchanged: the engineer's policy permits everything, so
      `runs_the_end_to_end_demonstration` (tests.rs:1741), including its byte-identical rerun
      assertion, must still pass.
    - `rejects_unauthorized_queries` (tests.rs:845) and `builds_bounded_packs_with_manifests`
      (tests.rs:1422) must still pass.
    - Downstream readers of packs: capability intake (`src/capability/intake.rs`) resolves
      evidence and assertions "for the submitting reader". Confirm it does not rely on assertions
      whose evidence is absent from the pack. Discovery (`src/discovery/resolve.rs`) reads
      `scope.environment` from packs: confirm that an absent environment is treated as unknown,
      not as "none".
    - Moving `derive` after `select` changes the `assertion_ids` used to build the context view's
      `visible` set. Keep using the derived ids.
  - Test:
    - `contractor_cannot_learn_forbidden_scope`: the policy allows environment `{staging}` and
      excludes `logs`; the request names no environment. Assert that the serialized pack does not
      contain "production" anywhere; that there is no assertion with subject `dep-7731`; that
      `stop_reason == Rejected` (or that all later steps are denied, if you choose not to reject);
      and that `scope.learned` cites only evidence that is in the pack.
    - `sensitivity_ceiling_blocks_learning`: an actor with `max_sensitivity = Internal`; the log is
      Confidential. Assert that nothing derived from the log appears in the pack.
  - Class guard: write one property-style test that runs every scenario variation in tests.rs
    under several restricted policies (by environment, repository, type, and sensitivity) and
    asserts: every evidence id referenced ANYWHERE in the pack (assertions' supporting and
    contradicting lists, `scope.learned`, `context_view.evidence`, `missing_evidence` details,
    warning refs) is in `pack.evidence`. Any future consumer that uses data before the
    eligibility check then fails this test automatically.

- **[2.13.3(1)] Store race: concurrent runs lose evidence and audits (also 2.5.9; Part 4 C16;
  Part 8 #2). CRITICAL**
  - Breaks: `CaptureStore::open` (store.rs:214) takes `capture.lock`, loads everything, and
    RELEASES the lock when it returns. `save` (store.rs:288) takes the lock again and rewrites
    every JSONL file from memory. Two `crane capture investigate` runs (two terminals, or CI plus a
    developer) both load state S, add records A and B respectively, and save. The second save
    writes S+B, so A, its audit, its pack, and its relations are gone. Audit loss defeats replay
    and lineage.
  - Fix (preferred: merge on save, which avoids holding the lock for a whole investigation):
    1. In `save`: acquire the lock, RE-READ every collection from disk, merge with memory, write
       atomically, release.
    2. Merge rules (safe because ids are content-derived, mod.rs:50): artifacts, blobs, evidence,
       entities, relations, assertions, and packs are a union by id. For entities, merge
       attributes, and resolve versions by preferring evidence-derived values (see 2.7.1). Audits
       are a union by `query_id`; for the same id, keep the one with a `replay` comparison set.
       Contributions with the same id resolve by lifecycle order (Proposed < Accepted <
       Superseded or Expired; Rejected is final) so a status never regresses. Episodes keep the
       union of packs and feedback, and the outcome with the latest `ended_at`. `StoreState` takes
       the union of deliveries; for cursors, keep the furthest.
    3. Alternative: hold the lock from open to save. That is simpler but serializes all
       investigations; first check how `Lock::acquire` (`src/platform/files.rs`) behaves on
       contention (waits or fails).
  - Check:
    - `persists_and_migrates_the_store` (tests.rs:1851) still passes.
    - Merging an entity must never overwrite an evidence-derived version with a contribution's
      version (coordinate with 2.7.1).
    - Atomic writes (`write_atomic`) stay atomic per file. A crash between files must leave each
      file valid, with an at-least-once union semantics.
  - Test:
    - `concurrent_saves_keep_both`: open two stores on the same temporary directory, ingest
      different records into each, save both, reopen, and assert that both sets of evidence and
      both audits are present.
    - `contribution_status_never_regresses_on_merge`.
  - Class guard: a stress test that spawns N processes or threads, each running an investigation
    on the same store; assert that the total audits equal N and that every pack's
    `verify_manifest()` passes.

---

## B. HIGH

- **[2.7.1] Entities: a context proposal overwrites a zone or flow version, even when refused
  (also 2.7.6(2), 2.10.2(1); Part 4 C17; Part 8 #3). HIGH**
  - Breaks: `CaptureStore::propose` (store.rs:699) handles `TargetKind::Zone` and `Flow` by
    building `Entity::new(kind, id)` with `version = proposal.target_ref.version` and calling
    `graph.add_entity` (store.rs:724-734) BEFORE `contributions.propose(...)?` validates anything.
    `add_entity` (graph.rs:163) overwrites `existing.version` whenever the incoming entity has one.
    So an agent proposing for zone `refunds` with version `v99` changes the graph's zone version to
    v99, even if its proposal is refused (wrong namespace, missing evidence). `link_governance`
    then writes version scopes like `zone:refunds@v99`, and downstream drift checks (capability
    intake, evolution) compare against a forged version. Agent-supplied data has corrupted
    observed state, which breaks Rule R4.
  - Fix:
    1. In `store.propose`: validate first (`contributions.propose`), and touch the graph only after
       it succeeds.
    2. For Zone and Flow targets: look up the existing entity by `entity_id(kind, id)`. If it does
       not exist, refuse with `NotFound` (consistent with the Entity and WorkEpisode targets). If
       the proposal names a version that differs from the entity's version, keep the proposal's
       version on the contribution record only, as "proposed for version X", and never write it
       to the entity.
    3. In `add_entity`: split the behavior. Evidence-driven updates (from `link`, GraphState) may
       set the version. Every other caller may only fill a version that is missing. Add an
       explicit `observe_version(id, version, evidence_id)` used only by `link`.
  - Check:
    - The demo proposes a hypothesis for zone `refunds` with `zone_version` taken from the pack
      (demo.rs:252); it must still succeed, because the entity exists.
    - `agent_contributions_stay_proposed` (tests.rs:1135) and
      `separates_task_local_and_reusable_context` (tests.rs:1488) still pass.
    - Contributions to zones that were never observed now fail with NotFound. Confirm no caller
      (capability, compiler demos) depends on creating zones through proposals.
  - Test:
    - `refused_proposal_leaves_graph_untouched`: snapshot the counts and versions of
      `graph.entities` and `graph.relations`, make a proposal that will be refused, and assert both
      are unchanged.
    - `proposal_cannot_change_zone_version`: a valid proposal with version v99; assert the zone
      entity is still v3, and `link_governance` still yields `@v3`.
  - Class guard: a test asserting that no path reachable from a Contribution or an Agent actor
    mutates `Entity.version`, `Entity.attributes`, or any relation with status Observed or
    Declared. Implement it by running every contribution API against a frozen graph and diffing
    the graph.

- **[2.11.1(1)] Episode outcome: "Succeeded" accepts evidence ids that do not exist (Part 4 C18;
  Part 8 #4). HIGH**
  - Breaks: `WorkEpisode::record_outcome` (episode.rs:129) only checks that the evidence list is
    non-empty for Succeeded and PartiallyVerified. `["made-up"]` passes. Capability intake and
    evolution treat an episode outcome as a fact. A fabricated "Succeeded" can make a capability
    candidate look verified.
  - Fix: add a store-level `CaptureStore::record_outcome(episode_id, actor, outcome, evidence, now)`
    that checks each id exists in `self.evidence` (as `propose` does at store.rs:699). Optionally,
    also require each piece of evidence to belong to this episode: either `evidence.episode_id ==
    episode`, or the evidence appears in one of `episode.packs`. Make `WorkEpisode::record_outcome`
    private to the store module.
  - Check: callers outside capture that call `record_outcome` directly (grep `record_outcome` in
    `src/capability*`, `src/compiler`, `src/evolution`, and the demos) must pass real ids. The demo
    passes deployment and verification ids from the pack, so it stays valid.
  - Test: `outcome_requires_existing_evidence` (refuses an unknown id, accepts real ids);
    `outcome_evidence_must_belong_to_episode`.
  - Class guard: a rule that every API accepting evidence references validates them through one
    helper, `require_evidence(ids, scope)`, and a test that greps for "evidence: Vec<String>"
    parameters without that helper. Add this to the review checklist.

- **[2.3.3] Templates: only the first of several values is checked (also 2.4.3; Part 4 C4; 7.1-B;
  Part 8 #5). HIGH**
  - Breaks: `ScopeResolution::variable` (scope.rs:249) returns `commits.first()`,
    `pull_requests.first()`, `deployments.first()`, `sessions.first()`. For a PR with commits
    A, B, C, the `code-change`, `ci`, and `tests` steps run for A only. The pack can be
    `Sufficient`, with `ci_passed = Yes`, while B and C were never checked. Nothing warns.
  - Fix (two phases):
    1. Immediately: when any list-valued variable used by an executed step has more than one value,
       add a `WarningKind::Completeness` warning ("3 commits known; only 9f1c… was checked") and
       record the unchecked values in the audit.
    2. Proper fix: fan out. Run the step once per value (each call is budgeted), and build
       `Subjects.commits` from all values so `derive` produces per-commit assertions.
       `change_verified` then requires all of them.
  - Check:
    - Fan-out changes the audit: several `QueryStepRecord`s share one `step_id`. `compare` in
      replay (run.rs:1761) matches steps by `(step_id, resource_id)`. Change the key to
      `(step_id, resource_id, parameters)`, or the comparison mismatches.
    - Budget tests (`enforces_query_budgets`, tests.rs:1557) may need new expectations.
    - The demo has one commit, so its output is unchanged. Verify with the byte-identical test.
  - Test: `multi_commit_pr_checks_every_commit` (a fixture PR with two commits, CI for one
    only); assert `ci_passed` for the second is Unknown and `change_verified` is Unknown.
    `multi_value_warns` for phase 1.
  - Class guard: a unit test that loops over VARIABLES (recipe.rs:22) and asserts that each
    list-backed variable either fans out or warns. Any new list-valued variable must join the loop.

- **[2.2.10(1)] Time window drops every timeless record (also 2.2.13 Landscape and Crane; 7.1;
  Part 8 #6). HIGH**
  - Breaks: in `page` (connector.rs:313), once `since` or `until` is set, `in_window` keeps only
    records with a `source_timestamp` (connector.rs:325-331). Zones, flows, policies, and
    validation have none (local.rs:398, local.rs:530). Any investigation with a time window gets
    `Empty` for governance evidence, which reads as "no policies exist".
  - Fix: make "is this type time-bound?" a single shared definition. Promote `is_event`
    (parsers.rs:590) to `EvidenceType::is_timed()` in record.rs, and use it in three places:
    1. In the broker's `call`: when `step.evidence_type` is not timed, send `since: None, until:
       None` in the QuerySpec.
    2. In `page`: apply the window only when the spec's evidence type is timed. Defensive: a
       record without a timestamp in a timed query stays excluded.
    3. In selection, `ineligible` already skips records whose `observed_at` is None; keep that
       consistent.
  - Check: `separates_observed_and_ingestion_time` (tests.rs:385) and the connector tests
    (tests.rs:216) still pass. The freshness-warning logic (run.rs:1566) already uses the same
    type set; switch it to `is_timed()` so the definitions cannot drift apart.
  - Test: `time_window_keeps_governance_evidence`: the demo request plus a time window around the
    deployment; assert the policy and zone records are Collected and the timed records outside the
    window are excluded.
  - Class guard: a test asserting that `is_timed()` is the only place the timed/timeless list is
    written. Grep for `GraphState | EvidenceType::Policy` patterns outside it.

- **[2.2.10(2)] Cursors never followed; silent truncation (also 2.9.6; 7.2; Part 8 #7). HIGH**
  - Breaks: `call` always sends `cursor: None` (run.rs:921 onward) and only stores `next_cursor`
    (run.rs:1060). With a step limit of 50 and 120 matching CI runs, 70 are dropped, and the pack
    gives no Truncation warning.
  - Fix: in `call`, for Query steps, loop while the outcome has a `next_cursor`, the record budget
    remains, and the step budget remains. Pass the cursor back, append records, and count each page
    as one connector call in `usage.steps`. Aggregate the pages into ONE `QueryStepRecord` with a
    `pages` count, so replay keys stay stable. If the loop stops with a cursor still pending, add a
    `WarningKind::Truncation` warning naming the step and how many pages were left. Keep
    `store.set_cursor` for incremental ingestion later.
  - Check:
    - The Git connector refuses cursors and never sends one, so no change there.
    - `page` returns an offset cursor, so the loop terminates.
    - The budget semantics change (more steps used), so update `enforces_query_budgets`
      expectations.
    - `compare` must treat a changed page count as a difference only if the evidence ids differ.
  - Test: `follows_cursors_within_budget` (a fixture with 120 CI runs; assert all are ingested or a
    Truncation warning is present); `stops_paging_at_budget`.
  - Class guard: the connector contract test harness (see 2.2.12) must include "a connector that
    paginates is fully read or truncation is reported".

- **[2.2.13] DataSea: tracker key versus canonical task id gives "no agent activity" (also
  Part 4 C8; Part 5 Example 2; Part 8 #8). HIGH**
  - Breaks: the DataSea connector puts `task = foxx_task_id` (Crane's canonical `tsk_…`) in content
    and metadata (local.rs:283, local.rs:294). The tracker key (`CRANE_TASK_ID`, for example
    `PAY-142`) is a separate field, `external_task_id` (`src/telemetry/events.rs:58-59`).
    `crane capture investigate task PAY-142` therefore gets `Empty` tool calls, and the pack says
    no agent activity although the agent worked. `link` also fails to connect sessions to the
    episode, because `refs.task != context.task`.
  - Fix: in `DataSeaConnector::records`, also emit metadata `external_task` and content
    `external_task`. Implement `query` for the `task` filter so a record matches when either
    `task` or `external_task` equals the value (a custom filter before `page`, or normalize: add
    both values under one key, `task_any`). In `link` (link.rs:123), accept `refs.task ==
    context.task` OR `external_task == context.task`; extend `references` (parsers.rs:541) to
    capture `external_task`. Add `external_task` to the DataSea `ConnectorSpec` filters
    (registry.rs:150).
  - Check: the canonical id path still works (`investigates_a_local_change`, tests/capture.rs:99).
    No event exposes another repository's tasks (filters still apply within the store).
  - Test: `datasea_matches_external_task_key`: an event store fixture with `foxx_task_id = tsk_1`
    and `external_task_id = PAY-142`; query with PAY-142; assert the ToolCall is collected and
    linked EXECUTED_DURING the episode.
  - Class guard: a test that for every identity field with two forms (task, session) the
    connector matches both. Add "dual identity" to the connector contract harness.

- **[2.3.4] Predicates: not bound to the subject; no file-path predicate (also 7.1; Part 8 #9).
  HIGH**
  - Breaks:
    1. `FieldEquals{Deployment,status,failed}` holds if ANY collected deployment failed
       (recipe.rs:95). With two deployments, one failure triggers log retrieval for the other.
    2. In the story, the deployment says `migration: "false"` while the commit adds
       `migrations/0042_refund_keys.sql` and the schema API says the migration was applied. The
       `schema` step is skipped, and the evidence that best explains the KeyError is never fetched.
  - Fix:
    1. Add an optional `subject: Option<String>` to `FieldEquals` (a template such as
       "{deployment}"), so only records whose `refs.deployment` or `source_ref.record_id` equals
       the scope value count.
    2. Add `Predicate::AnyFileMatches{glob}` over CodeChange payload files (reuse
       `platform::glob::matches`).
    3. Publish `failed-deployment` v2 with `schema.when = Any[FieldEquals{…migration true…},
       AnyFileMatches{"migrations/**"}]`. Do NOT edit v1: the RecipeBook's immutability, and every
       audit that says v1, depend on it.
  - Check:
    - `executes_conditional_recipes` (tests.rs:909) asserts that schema is skipped for the failed
      scenario under v1. Keep the demo and the tests on v1, or update them deliberately when
      switching to v2.
    - `for_class("deployment-failure")` returns the latest version, so v2 becomes the default;
      confirm the CLI and demos pin v1 explicitly where byte-identical output is expected (the
      demo pins `version: Some(1)`, demo.rs:196).
    - Predicate validation (recipe.rs:128) must accept the new variant and validate the glob.
    - The JSON of `Predicate` is serialized in audits; adding variants is backward compatible
      for writing, but older readers cannot read v2 audits. Note this in the release.
  - Test: `schema_runs_when_commit_adds_migration` (v2) and `subject_bound_field_equals` (two
    deployments, one failed; logs fetched only for the failed one).
  - Class guard: a golden test pinning each built-in recipe's digest (see 2.3.9), so any edit to a
    published version fails.

- **[2.8.3(1)] Only four assertion rules; failed validation and governed-zone touches produce no
  conclusion (also Part 5 Example 2; Part 8 #10). HIGH**
  - Breaks: `derive` (assertions.rs:362) only knows `ci_passed`, `tests_passed`,
    `deployment_succeeded`, and `change_verified`. A Validation record with `passed: false`, or a
    commit writing into a critical zone, yields evidence but no assertion and no warning, so
    downstream gates see nothing wrong.
  - Fix:
    1. `policies_hold` per repository: Validation `passed` gives Yes or No.
    2. `governed_zone_touched` per commit: a File BELONGS_TO_ZONE relation exists, and the object
       lists the zones. Treat it as information, not pass or fail.
    3. Optionally, `agent_activity_observed` per task.
    4. Bump `RULES_VERSION` (assertions.rs:18). Assertion ids include the rules version, so all
       new assertions get new ids, which is intended.
  - Check:
    - The capability registry recomputes claims "with Component 2's rules" and gates map assertion
      predicates (`src/capability/validate.rs`, `src/capability/intake.rs`). Confirm unknown
      predicates are ignored or handled, not rejected.
    - The lab and evolution re-derive claims (`src/capability_lab/invariants.rs`); confirm they use
      the same rule set or tolerate new predicates.
    - `change_verified` should probably include `policies_hold`. Decide explicitly: including it
      changes `change_verified` in local-change packs from Unknown to No when validation fails.
  - Test: `failed_validation_yields_policies_hold_no`; `zone_touch_assertion_lists_zones`.
  - Class guard: a test asserting that every EvidenceType present in a built-in recipe's
    `required` list feeds at least one rule, or is explicitly marked "context only".

- **[2.4.4(1)] Learn: the first record ingested decides the commit (Part 8 #11). HIGH**
  - Breaks: `learn` pushes a commit only if `commits.is_empty()` (scope.rs:277). With two
    deployments from a discovery query, the commit of whichever was ingested first is used for
    every later step. Selection ranks by type and recency, but ingestion order is the connector's.
  - Fix: learn into the list. Push each distinct commit from PullRequest and Deployment records,
    and record each as a LearnedFact. Then the 2.3.3 fix (warn or fan out) applies. For
    deployments, combine with subject-bound predicates (2.3.4), so each commit is associated with
    its deployment.
  - Check: the demo has one deployment, so the output is unchanged. `refresh_unknown` logic is
    unchanged.
  - Test: `two_deployments_learn_both_commits_and_warn`.
  - Class guard: covered by the 2.3.3 VARIABLES loop test.

- **[2.9.6] Warnings: no Truncation warning for cursors that were not followed. HIGH**
  - Same issue and fix as 2.2.10(2).

---

## C. MEDIUM

- **[2.1.4] Actor kind is whatever the caller says. MEDIUM**
  - Breaks: `Actor.kind` is caller-supplied. The CLI sets Human (`src/cli/capture.rs:300-312`). If
    Work Capture is ever exposed through an API or library to agents, an agent can claim Human
    and review contributions or override outcomes.
  - Fix: derive the kind from Crane's signed identity (owner or admin keys, the
    `governance/admins.rs` model), plus the existing agent-environment refusal. Inside capture,
    accept an `AuthenticatedActor` type that can only be built by the CLI or service layer.
  - Check: the demos construct actors directly; give them a test-only constructor.
  - Test: an API-level test that an Agent-originated request cannot produce a Human actor.
  - Class guard: make `Actor::new` crate-private to a module that performs authentication.

- **[2.1.5] / [2.6.3] Sensitivity depends on the redactor recognizing the secret. MEDIUM**
  - Breaks: `classify` (artifact.rs:59) sets Confidential only if `redact_value` found something.
    A token in an unknown format stays Internal and is shown to actors with an Internal ceiling.
  - Fix: add a per-evidence-type sensitivity floor to `ResourceDefinition`, through a validated
    configuration key, or as a code default (RuntimeLog and ToolCall at least Confidential). The
    result is `max(declared, redaction floor, type floor)`. Extend the redaction patterns
    (`src/telemetry/redact.rs`) with high-entropy detection.
  - Check: actors with an Internal ceiling lose logs. That is intended, but confirm demo actors
    (Confidential) are unaffected, and adjust `preserves_artifacts_and_provenance` (tests.rs:511)
    if its expectations change.
  - Test: `unknown_secret_format_still_confidential_for_logs`.
  - Class guard: a fuzz test feeding random high-entropy strings through capture and asserting the
    stored blob never contains them verbatim.

- **[2.1.8] / [2.5.3] Refusals returned as errors leave no audit (Part 4 C1, C2; Part 8 #13).
  MEDIUM**
  - Breaks: "no recipe for 'X'" (run.rs:565) and an invalid plan (`from_plan` → `validate`)
    return `Err` before any audit exists, so nobody can see later that a request was refused.
  - Fix: before returning the error, push a `QueryAuditRecord` with `recipe: None`,
    `recipe_digest: None` (the fields are already Optional, run.rs:299), `stop_reason: Rejected`,
    `status: Rejected`, the error in `errors`, and `pack_id: None`. Keep returning `Err` so callers
    behave the same.
  - Check: `replay` refuses audits without a recipe (it already does: "executed no recipe"). The
    CLI output is unchanged.
  - Test: `missing_recipe_is_audited`, `invalid_plan_is_audited`.
  - Class guard: a test that every `Err` path out of `investigate` and `replay` leaves exactly one
    audit record more than before. Iterate over a table of bad requests.

- **[2.2.7] / [2.9.2] A Partial answer with zero records is reported as Empty (Part 4 C7;
  Part 8 #14). MEDIUM**
  - Breaks: `source_report` maps `Executed && record_ids.is_empty()` to `Empty` (run.rs:1485)
    without looking at `connector_status`. Crane's `all()` returns Partial when validation cannot
    run (local.rs:605); `fetch("validation")` keeps zero records, so the pack says "no validation
    exists" when the truth is "validation could not be computed". This sends people to the wrong
    fix.
  - Fix: in `source_report`, map `Executed` with zero records and `connector_status ==
    Some(Partial)` to `Unavailable`, with the detail "partial: <error>". Do NOT add a new
    SourceState variant.
  - Check: capability intake matches on `SourceState` (`src/capability/intake.rs` imports it). A
    new variant would break deserialization of stored packs, which is why the fix reuses
    `Unavailable`. Confirm intake's treatment of Unavailable is correct for this case.
  - Test: `partial_zero_records_is_unavailable` (fixture `FixtureBehavior::Partial(0)`).
  - Class guard: a table test enumerating every (StepState, ConnectorStatus, record count, parse
    outcome) combination against its expected SourceState.

- **[2.2.12] Hosted sources exist only as fixtures. MEDIUM (product gap)**
  - Breaks: GitHub PRs, CI, deployments, logs, and metrics have no live connector, so
    `failed-deployment` cannot run on real data, and the hosted code paths (pagination,
    credentials, rate limits) are only exercised by fixtures.
  - Fix: build a GitHub connector first (PRs, check runs, Actions), using the existing `gh`
    connection (`src/github/mod.rs`) and `auth_reference = integration:github`. Map HTTP 401 and
    403 to Unauthorized, 404 to Ok with zero records, 429 to RateLimited, 5xx to Unavailable.
    Return real snapshots (ETag or `updated_at`).
  - Check: it depends on the 2.2.10(2) cursor loop and on 2.2.4 credential resolution, so land
    those first.
  - Test: recorded-response tests (no network) for every status mapping.
  - Class guard: a reusable connector contract harness, `assert_connector_contract(connector,
    definition)`, run against every connector (fixture, git, datasea, landscape, crane, github).
    It checks: every declared filter key appears in metadata; failures are never Ok-and-empty;
    a snapshot is present or explicitly None; pagination is consistent; `get_metadata` strips
    content.

- **[2.2.13] DataSea reads the whole event log on every call. MEDIUM (scale)**
  - Breaks: `records()` calls `read_all` each time (local.rs:264), so cost grows with history.
  - Fix: cache the parsed events per connector instance per investigation (invalidate when the
    head digest changes), and index by task and session.
  - Check: replays in the same process must see new events. Key the cache by head digest.
  - Test: `datasea_cache_invalidates_on_new_event`.
  - Class guard: a performance smoke test with 50k events and a time ceiling.

- **[2.2.13] Crane connector runs full verification on every call; it cannot be interrupted.
  MEDIUM**
  - Breaks: `all()` runs `evaluate` (local.rs:568, local.rs:594) for every fetch and query.
    `local-change` calls it twice (policies, validation), and the broker's time budget cannot stop
    it.
  - Fix: memoize `all()` per connector instance, keyed by the registry generation or the
    governance snapshot, so a replay after a change recomputes. Pass a deadline into `evaluate`,
    or run it through `platform::process` with a time limit.
  - Check: `crane capture sources` and investigate output are unchanged.
  - Test: count `evaluate` calls per investigation (expect 1).
  - Class guard: covered by the deadline work in 2.3.7.

- **[2.3.1] Only two recipes exist. MEDIUM**
  - Breaks: every other question needs a proposed plan or fails (C1). There is no discovery recipe
    ("which deployments happened in this window?").
  - Fix: add `deployment-discovery` v1 (Query Deployment by `{environment}` within the window) and
    `pr-review` v1 (PR, commits, CI, tests, policies, validation, zones).
  - Check: `for_class` collisions (2.3.9(2)). Each new class must be unique.
  - Test: one scenario test per recipe, plus golden digests.
  - Class guard: the recipe digest golden test.

- **[2.3.5] Conditional requirements disappear when their subject was never fetched. MEDIUM**
  - Breaks: `missing` (run.rs:1161) drops requirements whose `when` is false. If the deployment
    was never fetched, "deployment failed" is false, so RuntimeLog and Metric are not required. A
    less-informed investigation looks more complete (Part 5, Example 1).
  - Fix: add `Predicate::referenced_types()`. In `missing`, if a requirement's condition
    references an evidence type with zero collected records, keep the requirement as missing with
    state NotQueried and the detail "condition could not be evaluated: no <Type> evidence".
  - Check: the demo (deployment present) is unchanged. Packs for Example 1 gain two missing items
    and stay Incomplete; nothing that used to be Sufficient becomes Incomplete unless it was
    falsely complete.
  - Test: `unevaluable_condition_keeps_requirement`.
  - Class guard: a property test asserting that adding knowledge never REMOVES a requirement (a
    monotonicity check across scenario pairs).

- **[2.3.7] Budget time is checked only between steps; no per-call deadline (Part 4 C12;
  Part 8 #16). MEDIUM**
  - Breaks: `exhausted` (run.rs:369) runs before each step. A connector that hangs (git on a huge
    repository, a network source) blocks forever.
  - Fix: pass the remaining time into each connector call (add `deadline: Instant` to `QuerySpec`,
    or a broker-level `call_with_deadline`). Git and Crane run through
    `platform::process::run_with_timeout`; hosted connectors set an HTTP timeout. On timeout,
    return `ConnectorStatus::Timeout`.
  - Check: `Box<dyn ResourceConnector>` is not `Send` (`FixtureConnector` uses `Cell`), so a
    thread-based timeout wrapper needs `Send` bounds. Prefer cooperative deadlines inside
    connectors.
  - Test: `slow_connector_times_out` (add `FixtureBehavior::Slow(ms)`).
  - Class guard: the connector contract harness asserts every connector honors a deadline.

- **[2.3.9(1)] RecipeBook: "v1" can silently change between releases (Part 8 #17). MEDIUM**
  - Breaks: the book is rebuilt from code on every run (run.rs:445). Editing `failed_deployment()`
    without bumping `version` changes what "v1" means, and replays of old audits fail the digest
    check, or worse, new audits labelled v1 mean something new.
  - Fix:
    1. A golden test pinning `failed_deployment().digest()` and `local_change().digest()` to
       constants. This is the cheapest and strongest guard.
    2. Persist `(recipe_id, version) → digest` in the store's StoreState; on broker creation,
       refuse a known version whose digest changed (InvalidRecipe: "publish a new version").
  - Check: the persisted map must merge correctly (2.13.3).
  - Test: the golden digest test itself; `changed_builtin_recipe_is_refused`.
  - Class guard: the golden test.

- **[2.4.4(2)] Learn: deployments and tasks are never learned (Part 4 C8; Part 5 Example 1;
  Part 8 #18). MEDIUM**
  - Breaks: a discovery plan finds `dep-7731`, but the scope keeps no deployments, so the caller
    must re-issue the request by hand.
  - Fix: add learning rules. A Deployment record, when `deployments` is empty or under a discovery
    class, adds its id; a Task-referencing record (PullRequest with `refs.task`) adds the task if
    unknown. Each becomes a LearnedFact.
  - Check: combine with 2.3.3 (several values) and the 2.1.6 eligibility gate, so facts are learned
    only from eligible records.
  - Test: `discovery_plan_learns_deployment`.
  - Class guard: the VARIABLES loop test covers list behavior.

- **[2.5.6] A learned environment denies environment-free steps (Part 4 C9; Part 8 #12). MEDIUM**
  - Breaks: check E (run.rs:823) runs for every step, so after `production` is learned, a
    staging-only actor's `policies` step is denied although policies carry no environment.
  - Fix: add `EvidenceType::is_environment_bound()` (Deployment, Verification, RuntimeLog, Metric,
    Configuration, CiRun, TestReport) and apply check E only when it returns true. After the 2.1.6
    fix, a forbidden learned environment rejects the run anyway, so this mainly matters for
    environments permitted later.
  - Check: do this after 2.1.6, and keep `rejects_unauthorized_queries` passing.
  - Test: `environment_check_skips_environment_free_types`.
  - Class guard: a table test of `is_environment_bound` per type.

- **[2.6.5] Parsers: unknown reference field names link to nothing; entity keys not normalized
  (Part 8 #25). MEDIUM**
  - Breaks: `references` (parsers.rs:541) tries fixed names (`commit|head_sha|sha`); a source using
    `revision` or `git_sha` produces unlinked evidence. A short sha and a full sha become two
    Commit entities (model.rs:93).
  - Fix:
    1. Per-connector field maps declared in `ConnectorSpec` (a static table mapping source fields
       to ref names), applied before the generic lookup.
    2. Normalize keys per entity kind before `entity_id`: lowercase hex for Commit; trim and
       lowercase for Environment.
  - Check: changing entity keys changes `entity_id` values, so stored graphs get different ids.
    Bump `SCHEMA_VERSION` and write a `migrate` step (store.rs:810) that re-keys, or accept
    re-ingestion. Coordinate with 2.1.1.
  - Test: `short_and_full_sha_are_one_commit`; `connector_field_map_extracts_revision`.
  - Class guard: a contract-harness check that each connector's sample records produce a
    non-empty `refs` for its declared evidence types.

- **[2.7.6(1)] Governance linking re-links every file against every zone on every run (Part 8 #22).
  MEDIUM (scale)**
  - Breaks: `link_governance` (link.rs:439) runs over all File entities and all Zone and Flow
    entities each investigation, so CPU cost grows without bound.
  - Fix: track linked pairs `(file_id, zone_id@version)` in StoreState. Link only files first seen
    in this run, plus all files when a zone or flow version changes. Relation ids are
    deterministic, so a re-link would be idempotent anyway; this only saves work.
  - Check: the relations produced must be identical to a full run.
  - Test: `incremental_governance_link_equals_full` (run full and incremental on the same store and
    compare the relation sets).
  - Class guard: a performance smoke test with 10k files and 50 zones.

- **[2.8.3(3)] No subjects means no assertions, which can read as "nothing wrong". MEDIUM**
  - Breaks: with no task, commit, or deployment known (Example 1, stage 1), `derive` returns
    nothing, and the pack has an empty assertion list with no explanation.
  - Fix: in `finish`, when `Subjects` is empty, add `WarningKind::Uncertainty`: "no subject was
    established; no conclusions were derived".
  - Check: downstream gates must not treat "no assertions" as success; capability gates already
    require specific claims.
  - Test: `no_subjects_warns`.
  - Class guard: a rule that every pack has either at least one assertion or an Uncertainty
    warning (assert it in the property test of 2.1.6).

- **[2.10.2(2)] Policy weakening judged by a keyword (Part 8 #21). MEDIUM**
  - Breaks: `weakens_policy` (contributions.rs:236) refuses effects in a deny-list (relax, remove,
    allow, disable, weaken) or a missing effect. A proposal declaring `effect: "tighten"` whose
    prose actually weakens a policy passes.
  - Fix: switch to an ALLOW-list (`tighten`, `add`, `clarify`) and require structured content for
    ConstraintProposal: `{policy, statement, effect}`. Keep `requires_crane_workflow = true`, which
    is the real safeguard; document that capture's check is advisory and Crane's signed workflow is
    authoritative.
  - Check: `refuses_self_approval_and_policy_weakening` (tests.rs:1246) still passes; existing
    stored constraint proposals with other effects remain readable.
  - Test: `unknown_effect_refused`; `tighten_requires_structured_content`.
  - Class guard: a test that any contribution type able to influence policy sets
    `requires_crane_workflow`.

- **[2.11.1(2)] Any human can overwrite any outcome (Part 8 #20). MEDIUM**
  - Breaks: `record_outcome` lets any `Human` change an outcome recorded by someone else
    (episode.rs:129), so a non-reviewer human can flip "Failed" to "Succeeded".
  - Fix: take the actor's `AccessPolicy` (not a bare Actor) and require `may_review` to change
    another actor's outcome. Store the previous outcome in an `outcome_history`, so changes are
    audited instead of overwritten.
  - Check: this changes the signature; update the demo and any compiler, lab, or evolution caller.
  - Test: `non_reviewer_cannot_override_outcome`; `outcome_history_kept`.
  - Class guard: a test that every mutation of a stored capture object leaves a history entry.

- **[2.11.1(3)] Follow-ups and replays create new episodes (Part 8 #19). MEDIUM**
  - Breaks: without a task, `episode_id` includes the start time (episode.rs:170), so each
    follow-up investigation and each `replay` (which re-runs the original request) creates a new
    episode. With a task, every retry of the same task merges into one episode.
  - Fix:
    1. In `replay`, set `request.episode_id = original.scope.episode_id` before running.
    2. Add an `episode EPI` flag to the CLI and document that conversational callers must carry
       `episode_id`.
    3. Optionally derive the untracked id from a caller-supplied conversation id instead of time.
  - Check: replay comparison is unaffected; the episode's `packs` list gains the replay pack.
  - Test: `replay_reuses_episode`; `followup_with_episode_id_groups`.
  - Class guard: a test asserting that `replay` never increases `store.episodes.len()`.

- **[2.12(1)] Replay misses sources that recovered (Part 8 #24). MEDIUM**
  - Breaks: `compare` (run.rs:1761) iterates only steps that were Executed in the original. A
    metrics source that was Unavailable then and answers now is not reported as a difference.
  - Fix: iterate over the union of steps from both audits, keyed by `(step_id, resource_id)` (or
    including parameters after 2.3.3). Report any state change in either direction.
  - Check: `distinguishes_replays_from_fresh_queries` (tests.rs:1605) and the demo's
    replay == Exact still hold.
  - Test: `replay_reports_recovered_source` (the original has a failing fixture; the replay has
    the fixture fixed).
  - Class guard: a symmetric test where swapping the original and replay yields the same set of
    differences.

- **[2.13.3(2)] Store cost: each run loads and rewrites the whole history. MEDIUM (scale, grows to
  HIGH)**
  - Breaks: `open` reads every JSONL file and `save` rewrites them all.
  - Fix: append-only segments (write only new lines; compact occasionally), persisted indexes,
    and then the MongoDB backend the server feature already has, for hosted use. Content ids make
    upserts idempotent.
  - Check: it must coexist with the 2.13.3(1) merge semantics, and migration via SCHEMA_VERSION.
  - Test: an open and save time benchmark with 100k evidence records.
  - Class guard: a CI performance budget for open plus save.

- **[2.14] CLI: one recipe reachable; no replay, contribute, review, outcome, feedback, or
  verify-pack commands (Part 8 #26). MEDIUM**
  - Breaks: `crane capture investigate` forces `local-change` (`src/cli/capture.rs:323`) and
    grants the caller everything (`src/cli/capture.rs:300-312`). The other functionalities are
    reachable only from code.
  - Fix: add `crane capture replay QUERY_ID`, `crane capture pack verify PACK_ID`,
    `crane capture contribute FILE`, `crane capture review ID accept|reject reason TEXT`,
    `crane capture outcome EPI OUTCOME evidence ID,ID`, and `class` / `recipe` / `episode` flags
    on investigate. Load a real AccessPolicy for the caller from configuration instead of granting
    everything.
  - Check: every command must respect the agent guard (no crane under agent sessions), emit
    command.executed events (telemetry spec F13), and keep the `--json` output stable.
  - Test: tests/capture.rs end-to-end per command.
  - Class guard: a CLI test asserting that every public capture functionality (the table in the
    chapter, §1.2) has a command.

---

## D. LOW

- **[2.1.1] SCHEMA_VERSION: no real migration exercised. LOW.** Fix: when version 2 lands (likely
  from 2.6.5 re-keying), add a migration test with a version 1 fixture directory. Class guard: a
  test that reads a checked-in version 1 store.
- **[2.1.2] stable_id: query ids can collide under concurrency. LOW.** Fix: add a random nonce
  (or a process id plus a counter) to query ids only; they need not be content-stable. Check:
  determinism tests use a fixed clock, so make the nonce injectable (derived from the Clock in
  tests).
- **[2.1.7] Clock built on Cell, not thread-safe. LOW (blocks scaling).** Fix: an AtomicU64.
  Check: the byte-identical demo stays identical.
- **[2.1.9] Metrics not emitted to DataSea. LOW.** Fix: emit one `capture.investigation` event per
  run with the Metrics (a new DASHBOARD.md flow entry). Check: redaction, and no evidence content
  in the event.
- **[2.2.3] New source kinds need a release. LOW (by design).** No change; land hosted connectors
  as code.
- **[2.2.4(1)] auth_reference validated but never resolved. LOW.** Fix: a resolver for `env:` and
  `integration:` (reusing `src/integrations` secrets), called by hosted connectors only. Test:
  secrets never appear in audits or packs (grep the serialized output).
- **[2.2.4(2)] configuration_version rule has no effect in the CLI (registry rebuilt each run).
  LOW.** Fix: persist the registry in StoreState, or drop the claim from the docs.
- **[2.2.5] ResourceStatus never becomes Unhealthy; health_check unused by the broker. LOW.** Fix:
  an optional pre-flight `health_check` per permitted resource, marking it Unhealthy with the
  detail and an implicit Denial "resource unhealthy". Check: don't double-call slow health checks
  (cache them).
- **[2.2.8] Filters silently fail if a connector forgets metadata keys. LOW.** Fix: the contract
  harness (2.2.12) checks that every declared filter key appears in metadata.
- **[2.2.11] Snapshots cover the whole source, so replays diverge on unrelated changes. LOW.**
  Fix: in `compare`, when the evidence ids are equal but the snapshots differ, report "source
  changed; records identical" as a separate softer verdict, or keep Diverged but say why.
- **[2.2.13] Git connector refuses cursors. LOW.** Fix: support `--skip` paging if Git queries
  ever exceed one page.
- **[2.3.1] starting_conditions validated but never enforced. LOW.** Fix: at run start, if a
  starting-condition variable is unknown, add a Completeness warning, or reject; decide per
  recipe.
- **[2.3.2] Every permitted resource of a type is asked, so budget doubles. LOW.** Fix: an
  optional `priority` on ResourceDefinition; a step asks the highest-priority available resource,
  plus others only when it fails.
- **[2.3.6] Stopping early can skip later contradicting evidence. LOW.** Fix: a `mandatory: bool`
  on RecipeStep (verification) that the stopping rule never skips. Requires a new recipe version.
- **[2.3.9(2)] for_class picks alphabetically when two recipes serve a class. LOW.** Fix: refuse
  duplicate classes in `RecipeBook::add`.
- **[2.3.10] Plans have no conditional requirements; relevance is not judged. LOW.** Fix: accept
  `RequiredEvidence` with predicates in plans. Relevance stays the caller's job, so document it.
- **[2.5.1] question is only a label. LOW (by design).** Document it; provide discovery recipes
  (2.3.1).
- **[2.5.5] resolve_versions spends budget without checking it. LOW.** Fix: check
  `run.exhausted()` before each version fetch.
- **[2.6.2(1)] A duplicate keeps its first episode_id. LOW.** Fix: keep an `episodes: Vec` of
  every episode that observed the evidence (append on duplicate), or document that selection
  ignores episodes.
- **[2.6.2(2)] No webhook receiver. LOW.** Fix: a server route that calls `ingest` with
  `IngestPath{kind:"webhook"}` (delivery dedupe exists). Needs signed source authentication.
- **[2.6.4] Payload::field reads only some shapes. LOW.** Fix: support Measurement point names and
  Text excerpt matching where predicates need them, or refuse predicates on unsupported types
  during recipe validation.
- **[2.7.2] The vocabulary lacks words such as COVERS (test → file). LOW.** Fix: add words via code
  plus a VOCABULARY_VERSION bump.
- **[2.7.3] RelationStatus::Reviewed never produced. LOW.** Fix: a review API that promotes a
  relation, recorded as a new relation with status Reviewed (never mutate the old one).
- **[2.7.4] Relation valid_from and valid_to never set. LOW.** Fix: set them for version-scoped
  governance links (valid while the zone version is current), or remove the point-in-time filter.
- **[2.7.5] Validation evidence creates no entity. LOW.** Fix: link Validation OBSERVED_ON a
  Repository and Policy entities it covers.
- **[2.8.2] Mirrors with different bytes count as independent. LOW.** Fix: an optional
  `mirror_of` in the definition, or count independence by (source system, record id) instead of
  bytes.
- **[2.9.7] Sealing leaves dropped ids in context_view.evidence. LOW.** Fix: after `seal` drops
  evidence, prune `context_view.evidence` and recompute `view_id` before the manifest is computed.
  Check: `verify_manifest` still passes.
- **[2.10.3] expire() never called. LOW.** Fix: call `contributions.expire(now)` at the start of
  every investigation (before `contributions_for`). Test: an expired contribution is not in the
  pack.
- **[2.11.2] Questions have no lifecycle and are never stored. LOW.** Fix: `ask`, `answer`, and
  `block` operations with `transition_allowed`, and store questions on creation; or remove the
  states until needed.
- **[2.12(2)] Replay compares evidence ids by order. LOW.** Fix: compare as sets.
- **[2.13.3(3)] audit() is a linear scan. LOW.** Fix: an index by query_id.
- **Part 3 declared but unused states** (AssertionStatus Contradicted and Superseded, QuestionState,
  RelationStatus Reviewed): LOW. Either implement (assertion supersession when a new revision
  changes a vote) or remove, so consumers don't build on them.

---

## E. Holds as designed (NONE)

2.1.3 Truth · 2.2.1 resource types · 2.2.6 connector contract · 2.2.9 QuerySpec validation ·
2.3.8 recipe validate and digest · 2.5.8 step and stop labels · 2.7.7 traversal and Context View ·
2.8.4 hypotheses and trace · 2.9.4 excerpts · 2.9.5 contribution separation · 2.9.8
InvestigationStatus. Keep their existing tests; they are the guarantees the fixes above rely on.

---

## F. Index: chapter Part 4, Part 7, and Part 8 numbers → sections above

- Part 4 cases: C1 and C2 → 2.1.8 · C3 none · C4 → 2.3.3 · C5, C6 none · C7 → 2.2.7 · C8 → 2.2.13
  (DataSea), 2.6.5, plus 2.4.2 for repository spelling (normalize the repository label in both the
  request and the definitions) · C9 → 2.1.6 and 2.5.6 · C10 none · C11 → 2.3.2, 2.2.10(2),
  2.2.4(1) · C12 → 2.3.7 · C13 → 2.9.7 · C14 → 2.6.3 · C15 none · C16 → 2.13.3(1) · C17 → 2.7.1 ·
  C18 → 2.11.1(1).
- Part 7.1 ambiguity: A → 2.1.8 · B → 2.3.3 · D → 2.8.2 · E → 2.6.5 · F → 2.7.1 · G → 2.12(1), and
  freshness checks that run only when `freshness_ms` is set (consider a default freshness per
  evidence type: LOW).
- Part 7.2 scaling: history → 2.13.3 · sources and slowness → 2.3.7, 2.2.13, plus parallel steps
  (needs 2.1.7 first, then a step dependency graph with results merged in recipe order so audits
  stay identical: MEDIUM, after the correctness fixes) · data per source → 2.2.10(2) · graph →
  2.7.6(1) · repositories → shard by repository (LOW) · freshness → 2.6.2(2).
- Part 8 register #1–#30 → the sections named in each entry above.

---

## G. Recommended order (so fixes don't collide)

1. 2.1.6 eligibility at ingest (CRITICAL). It touches `call`, `learn`, `derive`, and `finish`;
   do it first because 2.5.6, 2.4.4, and 2.3.3 build on it.
2. 2.13.3(1) merge-on-save (CRITICAL).
3. 2.7.1 contributions never mutate entities (HIGH).
4. 2.11.1(1) outcome evidence must exist (HIGH).
5. 2.2.10(1) timeless types exempt from the window, and 2.2.10(2) cursor loop (HIGH).
6. 2.3.3 / 2.4.4(1) multi-value warning, then fan-out (HIGH), with the replay key change from
   2.12(1).
7. 2.2.13 dual task identity (HIGH).
8. 2.3.9 golden recipe digests, then 2.3.4 predicates as failed-deployment v2 (HIGH). Never edit
   v1.
9. 2.8.3 new rules with a RULES_VERSION bump (HIGH), after checking capability, lab, and evolution
   tolerate them.
10. The MEDIUM items, then LOW.

## H. Run after every fix (no regressions elsewhere)

- `cargo fmt --all -- --check`
- `cargo clippy --locked --all-targets -- -D warnings` (and with `--features server`)
- `CRANE_INGEST_URL=off cargo test --locked`. This runs the 24 capture unit tests
  (`src/capture/tests.rs`), `tests/capture.rs`, AND the capability, compiler, lab, discovery,
  invocation, and evolution suites, which all consume Evidence Packs. Any change to pack shape,
  assertion ids, SourceState mapping, or recipe versions shows up there.
- The byte-identical demo test (tests.rs:1741-1760) must pass unless the change intentionally
  alters demo output. If it does, update the expectations in the same commit and say why.
- If any persisted shape changes (entity keys, new enum variants, new fields), bump
  SCHEMA_VERSION (mod.rs:38), add a `migrate` step (store.rs:810), and add a version 1 to
  version 2 fixture test.
- Add the three class guards once, early: (1) the pack-reference property test (2.1.6), (2) the
  golden recipe digests (2.3.9), (3) the connector contract harness (2.2.12). Together they catch
  most future issues of these kinds automatically.
