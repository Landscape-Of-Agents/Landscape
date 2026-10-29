# Terminology

Every term in plain words, one or two lines each. Where the code uses another name, it is in
parentheses. For how things work, follow the links in [the docs index](README.md).

**The one idea behind every term:** people sign the rules, Crane checks every agent action against
them before it happens and looks at what really changed afterwards, and nothing is called "done"
without evidence.

---

## 1. Products and surfaces

| Term | Meaning |
|---|---|
| **Crane** | The free rule layer. Signed rules for AI agents, checked before every action and verified after. |
| **Black Box** | The paid flight recorder. Every agent attempt, near miss, and what stopped it, kept tamper-evident. |
| **Evidence Receipts** | The paid proof per change. What is proven, contradicted, or unknown (and why), for every pull request. |
| **Landscape** | The whole system's name, and the VS Code extension that authors rules (`foxx`). |
| **Dashboard** | The hosted web app where a team sees agents, decisions, incidents, and approvals (served by DataSea). |
| **DataSea** | The hosted service that receives signed uploads from machines and serves the dashboard. |
| **CLI** | The `crane` command. Only people run it; agents never do. |

## 2. Rules people sign

| Term | Meaning |
|---|---|
| **Rule** | Anything in `.crane/` that limits what agents may do. Only people change rules, and only by signing. |
| **`.crane/`** | The folder in the repository that holds the rules. Agents can never write to it. |
| **Trust directory** | `~/.crane-trust`: your keys, secrets, and local records. Outside the repository, denied to agents. |
| **Signing key** | Your private key. A rule counts only when an owner or admin key signs it. |
| **Owner / admin** | The first signer of a repository, and the people the owner allows to sign. |
| **Manifest** | The signed summary of every rule file. A file that does not match it fails, and agents stop. |
| **Generation** | The rules' version number. Every signed change adds one. |
| **Checkpoint** | A commit marked as trusted, so "keep this as it was" has a fixed reference. |
| **Audit chain** | The append-only history of rule changes (`audit.jsonl`). Rewriting it breaks the chain. |
| **Selection** | Lines of code a person chose to govern, with an 8-character id such as `K7M2P9RX`. |
| **Marker** | The two comment lines Crane puts around a selection so it moves with the code. Agents cannot edit markers. |
| **Protect (preserve)** | "These lines must not change." Written `crane protect`, stored as `preserve` in the policy. |
| **Target** | "These lines must change, and in this way" (business logic, control flow, structure, or names). |
| **Policy** | A named list of protect and target rules in `.crane/policies/*.crane`, written in AgentScript. |
| **AgentScript** | The tiny rule language: `preserve ID;` and `target ID change_type logical_bn;`. |
| **Context** | Guidance attached to a rule and given to the agent when it touches that code. Guidance, never permission. |
| **Unresolved** | Crane cannot find where a selection is now. Agent changes to it are refused until a person repairs it. |
| **Zone** | An area of code (paths, files, folders, functions, or marked blocks) with its own limits. |
| **Flow** | A path through the code (an entry point and everything it calls), governed as one thing. |
| **Context pack** | Reusable guidance for zones and flows. Every flow must have one. |
| **Criticality** | How risky an area is: routine, sensitive, critical, restricted (restricted always needs a person). |
| **Autonomy ceiling** | The most an agent may do in an area: observe (look only), assisted (ask first), delegated, autonomous. |
| **Strictest wins** | Where several zones or flows cover the same code, the strictest limit applies. Rules only restrict. |
| **Proposed / active** | A zone or flow being reviewed (not enforced) or in force. |

## 3. Agents at work

| Term | Meaning |
|---|---|
| **Hook** | The program an agent tool runs before and after each action. It hands every action to Crane. |
| **Adapter (agent)** | Translates one agent tool's format (Claude Code, Codex, Copilot, Gemini) into one neutral action. |
| **Guard** | The part of Crane that decides each action from the real files, never from what the agent says. |
| **Decision** | The guard's answer: ALLOW, REQUIRE_APPROVAL, DENY, QUARANTINE, or ERROR. |
| **Rule code** | The short reason name on every decision, such as `PRESERVED_SELECTION` or `ZONE_OBSERVE`. |
| **Edit simulation** | Crane applies a proposed edit in memory first and refuses it if it would touch protected code. |
| **Shell check** | Commands are read before they run. It is a text check, so the real effect is judged afterwards. |
| **Script check** | Scripts an agent runs (`python x.py`) are read first for dangerous patterns. |
| **After-check (post-action verification)** | After each action Crane looks at what really changed on disk. |
| **Bypass** | Protected code changed without passing the guard (for example through a shell trick). The session is quarantined. |
| **Coverage gap** | An action Crane could not see or decide. It is reported, never counted as clean. |
| **Safety state** | Active, degraded (a rule was broken: changes need a person), or quarantined (reads only). |
| **Autonomy mode** | The session's own dial: observe, assisted, delegated (default), autonomous. Can only make things stricter. |
| **Credits** | A per-session budget of risky actions. When it runs out, actions need a person; credits never grant anything. |
| **Session / task** | One agent session (`ses_…`) and one piece of work within it (`tsk_…`). |
| **Stale session** | No hook activity for two hours and no proper end: the hooks may have been removed. |
| **Agent view of the boundary** | At session start, agents get the list of restricted files and their limits, instead of running Crane. |

## 4. Scope

| Term | Meaning |
|---|---|
| **Scope** | What an agent may touch, here and now, after every rule is combined. |
| **Boundary** | The scope of a whole repository: which files are limited, how, and what is always denied. |
| **Resolve** | Turn a name ("app/../pay.py", a symlink, a URL) into what it really is before deciding. |
| **Scope decision** | A recorded answer to "may this happen?", with the rule versions and file content it was made under. It goes stale when either changes. |
| **Baseline** | The boundary recorded when rules were last signed, for comparison later. |
| **Widening** | The boundary got looser without anyone signing, for example a protected file renamed out of its zone. |
| **Probe** | A simulated attack or check run against the boundary (`crane scope test`). Nothing is executed. |
| **Gap** | A limit of repository rules, reported as such: the network, files outside the repository, shell tricks. Repository scope is not a sandbox. |
| **Other worktree** | Another checkout of the same repository. Agents may not write into one from this session. |
| **Resource reference** | A typed name for something an action touches: `repo:app/x.py`, or `crm:account-17` for a business system. |

## 5. Skills

| Term | Meaning |
|---|---|
| **Skill (capability)** | A reusable, versioned procedure for one kind of task: steps, inputs, what it may do, how success is proven. It never grants itself anything. |
| **Template** | A reviewed starting point for a new skill: read-only lookup, repository change, or business record update. |
| **Draft (candidate)** | A new skill version. It grants nothing and cannot run. |
| **Validation gates** | The checks a version must pass: structure, sources, where it applies, proof, rules. |
| **Lab** | Checks a skill without running it (`crane skill test`). Unproven checks are "inconclusive", never "passed". |
| **Activate (publish)** | A person other than the author approves the exact version. Only then can it run. |
| **Retire (deprecate) / revoke** | No new use, or withdrawn everywhere. Never undone: a new version is the way back. |
| **Shared skill store** | `.crane/skills.json`: published versions, signed like the rules, so a revocation reaches every machine. |
| **Registered operation** | The only things a skill can do: typed actions such as `git/git.commit`, each with checked parameters. Never a shell command. |
| **Operation adapter** | The code behind registered operations for one system (Git today; CRM, HR, finance later). |
| **Effect class** | What an operation does: read-only, state-changing, or externally consequential. |
| **Binding set** | The reviewed map from each step of a skill to the registered operation that carries it out. |
| **Invocation** | One run of a skill (`inv_…`), with its own identity, checks, and record. |
| **Plan / plan digest** | The exact list of operations an invocation will run, and its fingerprint. |
| **Preflight** | Every condition re-checked right before an invocation runs: published, not revoked, not stale, applicable. |
| **Approval reference** | What a person approves: `inv_…@PLAN_DIGEST` or `rbr_…:STEP@DIGEST`. Change the plan and the approval no longer applies. |
| **Reconciliation** | Asking the real system whether an uncertain action happened, instead of trying it again. |
| **Idempotency key** | A key that makes a repeated request return the first result instead of acting twice. |
| **Claim** | Something a skill says it proves, such as "the tests pass". Counts only with evidence. |
| **Invariant** | A business rule the skill must keep, such as "refunds never exceed the charge". Tests alone do not prove it. |
| **Precondition** | A fact that must hold before starting. Proven by evidence, or declared by a person, never by the agent. |
| **Discovery** | Finding published skills that fit a task. Unknown is never a match. |
| **Evolution** | Spotting skills that keep failing and proposing a better version for people to review. Never applied by itself. |

## 6. Runbooks

| Term | Meaning |
|---|---|
| **Runbook** | A versioned workflow that runs skills in order. It can never give a skill more authority. |
| **Step** | One skill run inside a runbook, pinned to an exact skill version. |
| **Dependency** | A step that must finish first. |
| **Branch** | A step that runs only when an earlier step succeeded, failed, or was blocked. |
| **Approval point** | A step that waits until a person other than the one who started the run approves it. |
| **Retry** | Another attempt at a step, only when the step had no effect, within its limit (at most two). |
| **Failure policy** | What a failed step does: stop the run, or let independent steps continue. |
| **Budget** | The most operations a run may use, reserved before each step and settled from what really happened. |
| **Run** | One execution of a runbook (`rbr_…`): running, awaiting approval, needs reconciliation, succeeded, partial, failed, blocked, or cancelled. |
| **Completion** | The steps that must succeed, with proof, for the run to succeed. |

## 6a. Bricks

| Term | Meaning |
|---|---|
| **Brick** | A reusable, versioned module of one fixed kind (knowledge, identity, access, approval, limits, environment, precondition, verification, action, recovery, provenance, watch). Each kind answers one question no other kind answers. It can only tighten what agents may do. |
| **Brick type** | One of the twelve fixed kinds. People make Bricks; they never make new kinds. Nine former kinds stay readable and are rewritten by `crane brick migrate`. |
| **Credential** | A secret an Environment Brick names. Only the name is shared; the value stays in each machine's vault (`crane secret set`) and reaches only Crane-run operations. |
| **Brick Stack** | A named set of exact Brick versions for a bigger scenario. It is not a type and cannot loosen its members. |
| **Draft** | An unsigned Brick or Stack in `.crane/drafts/`. It grants nothing until an admin activates it. |
| **Attachment** | A Brick or Stack version pinned to a Flow, Zone, or Runbook, signed into `.crane/bricks.json`. |
| **Common Context** | Guidance stored with a Brick version; it travels to every target the version is attached to. |
| **Current Context** | Guidance stored on one attachment; changing it changes only that attachment. |
| **Snapshot** | The context agents receive for some targets, in a fixed order, with a digest for every source. Sent once per session while unchanged. |
| **Fail closed** | When Crane cannot honor a pin (revoked, missing), changes in that target need a person's approval instead of being allowed. |
| **Assignment** | An agent assigned to Flows and Zones. Its runtime status (idle, running, waiting, degraded) is derived, never a running model. |
| **Monitor** | A deterministic check a Brick asks for (integrity, policy tests, selections, denials), run by the background sync without a model. |

## 6b. Resident agents

| Term | Meaning |
|---|---|
| **Resident agent** | An agent assignment that stays responsible for one Flow, Zone, or Runbook for days or weeks. It uses no model and no process while idle. |
| **Definition** | A reusable, versioned description of a resident: its mission, the trigger sources and step types it may use, and its limits. Grants nothing by itself. |
| **Resident assignment** | One definition version on one target for a lifetime (at most 90 days). It acts only inside an active signed Brick assignment. |
| **Trigger** | A normalized event that may wake a resident: a Crane event, a signed rule change, a Brick monitor result, a timer, or a delivered CI or webhook event. Data, never instructions. |
| **Activation** | One bounded, persisted run of a resident for one trigger (`act_…`). It resumes after a crash or a person's answer. |
| **Context pack** (resident) | The versioned context an activation receives, cached by its dependencies (HIT, MISS, INVALIDATED). Never authority. |
| **RCA graph** | A versioned graph of signals, causes, checks, remediations, and verifications, each with how it is known. Proposed until a person reviews it. |
| **Supported cause** | A cause this activation's checks support. Not the same as confirmed. |
| **Step type** | One of twelve fixed kinds of plan step (`READ_CHECK`, `NOTIFY_SLACK`, `VERIFY`, …). One without an adapter is unavailable and never counts as done. |
| **Dashboard decision** | An approval, denial, answer, cancel, pause, or revoke a GitHub-verified repository admin makes on the dashboard. The machine applies it at its next sync with Crane's own checks. Approvals and answers count only when the definition allows it. |
| **Separation of duties** | A definition option: the person who activated an assignment cannot approve its steps. |
| **Idle** | Active with no activation running. No model call; shown as ACTIVE_IDLE, or DEGRADED when Crane would not admit it now. |

## 7. The eleven controls

The families of checks every skill, runbook, and agent action passes through (`crane <family> …`).

| Control | Question it answers |
|---|---|
| **Scope** | What may it touch? |
| **Permission** | May this operation act on this resource? |
| **Precondition** | Is what must be true before starting actually true? |
| **Constraint** | Does it stay within hard limits (size, time, environments, exclusions)? |
| **Approval** | Has the right person approved this exact action? |
| **Isolation** | What is really confined (files, processes, network, credentials), and what is not? |
| **Verification** | Is there evidence that it worked? |
| **Provenance** | Who and what produced this result, and does that record still check out? |
| **Budget** | Does it fit within the operations it may use? |
| **Recovery** | What happened when something went wrong, and what is safe to do next? |
| **Revocation** | Is its authority withdrawn? |

**Isolation profile:** a named execution path and what it confines: agent-hooks, typed-adapters,
sandbox (not available yet).

## 8. Evidence (Work Capture)

| Term | Meaning |
|---|---|
| **Work Capture** | Collects evidence about a piece of work from the systems that know: Git, CI, GitHub, the dashboard, Crane. Read-only. |
| **Source (connector)** | One system evidence comes from, with a fixed scope. Read-only by design. |
| **Evidence** | One fact from a source, with where it came from and how sensitive it is. |
| **Question class** | A kind of question, such as "why did this deployment fail?". |
| **Checklist** | A saved, versioned list of where to look for one question class: "check CI, then the commits, then the logs". Formerly called a recipe: the CLI still accepts `recipe ID` and `crane insight recipes`, and stored records and the code keep the field `recipe_id`. |
| **Exploration** | Looking around when no checklist exists yet. Its answer is never called complete. |
| **Learned checklist** | A checklist the system proposed from explorations people found useful, accepted by an owner. |
| **Evidence Pack** | The bundled answer to one question: the evidence, what it proves, and what is missing. Sealed with a digest. |
| **Proven / contradicted / unknown** | The three answers to any claim (yes, no, unknown). Unknown always comes with a reason. |
| **Unknown reasons** | Why something is unknown: empty (looked, nothing there), unavailable, denied, not_queried, parse_failed. |
| **Episode** | One piece of work from start to outcome, such as a task and its pull request. |
| **Outcome** | How an episode ended: succeeded, failed, partially verified, unknown, abandoned. |
| **Contribution** | Context someone proposes for the system to remember. Stays proposed until a person accepts it. |
| **Replay** | Run a recorded investigation again to see what changed. |

## 9. Learning (Insight)

| Term | Meaning |
|---|---|
| **Insight** | Finds repeating patterns across past work and turns them into proposals. Changes nothing itself. |
| **Pattern** | Something seen repeatedly: a recurring evidence gap, an unanswered question, a work routine. |
| **Minimum support** | A pattern needs at least 3 cases, so no single record can be singled out. |
| **Proposal** | A suggested change (a checklist, context, a rule) that waits for a person's decision. |
| **Co-occurrence** | Two things seen together. Never presented as cause. |

## 10. Records and the dashboard

| Term | Meaning |
|---|---|
| **Event** | One recorded fact (a decision, an effect, a session start), with a digest. |
| **Event chain** | Events linked by digests, so a removed or edited event is detectable. |
| **Redaction** | Secrets are removed before anything is written or uploaded. |
| **Eval** | A measured quantity, such as the denial rate. Missing data is "unmeasured", never zero. |
| **Alert rule** | A threshold on an eval that notifies Slack or WhatsApp when crossed. |
| **Compliance control** | A requirement (for example from SOC 2) and the evidence that it holds. |
| **Dashboard Agent** | A team-wide monitor on the dashboard that opens an incident when a threshold is crossed. |
| **Incident** | An event worth a person's attention, with its timeline. |
| **Remote quarantine** | An admin stops an agent from the dashboard. Its hooks deny further actions. |
| **Read-only handle** | Dashboard pages read through a connection that cannot write. |

## 11. Command results

Every family command answers with the same envelope (`--format json`).

| Status | Meaning | Exit code |
|---|---|---|
| **ok / passed** | Done, or every required check passed. | 0 |
| **denied / failed / blocked** | Not allowed, a check failed, or it cannot proceed. | 3 |
| **approval_required** | A person must approve first. | 4 |
| **unknown / unresolved** | It cannot be established. Never treated as success. | 5 |
| (usage or internal error) | Wrong arguments, or something broke. | 1 |
| (hook block) | An agent tool is told "no". | 2 |

Also in every envelope: `request_id`, `decision` (when one was made), `reason_codes`, and references
such as `skill_ref`, `run_ref`, `approval_ref`, `budget_reservation_ref`, `policy_digest`.

## 12. Words with a precise meaning

| Word | In this system it means |
|---|---|
| **Allowed** | Permitted to start. Not the same as done. |
| **Succeeded** | Finished, and its required claims are proven by evidence. |
| **Verified** | Checked against evidence, not against anyone's word. |
| **Declared** | Stated by a person or document. Not observed. |
| **Unknown** | Not established, with a reason. Never zero, never "fine". |
| **Signed** | Approved by an owner or admin key. The only way rules change. |
| **Gap** | Something the system cannot see or enforce, said out loud. |
