// The hook runtime: 'crane agent hook --event EVENT --profile PROFILE', run by the agent host for
// every lifecycle event. It establishes identity, decides tool calls before they run, verifies
// their actual effects afterwards, accounts autonomy credits, reconciles at stop, attests at the
// end, and records every step as a security event. Whenever a decision or verification cannot be
// obtained the adapter answers fail-closed, never as an allow or a clean result.

mod session;
mod tools;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::governance::state::State;
use crate::governance::verify::{baseline, Outcome, Report};
use crate::governance::workspace::Workspace;
use crate::hooks::action::AgentAction;
use crate::hooks::adapter::{adapter, AgentAdapter, AgentKind, HookEvent, ProviderEvent};
use crate::platform::files::{write_atomic, Lock};
use crate::platform::{io_error, now_millis};
use crate::selection::registry::Operation as SelectionOperation;
use crate::telemetry::events::{Bindings, Event, Measurement, Scope, Source};
use crate::telemetry::identity::{Observed, SessionRecord, TaskRecord};
use crate::telemetry::ledger::{Kind, Ledger, Operation as LedgerOperation};
use crate::telemetry::model::{Decision, Domain, Enforcement, Severity, Telemetry};
use crate::telemetry::Stores;
use crate::telemetry::{alerts, sync};
use crate::trust::crypto::digest;

/** Largest hook payload read from stdin, in bytes */
const MAX_PAYLOAD: u64 = 16 * 1024 * 1024;

/** Largest baseline excerpt included in repair instructions, in bytes */
const MAX_RESTORE_EXCERPT: usize = 4096;

/** Phrases in a prompt that suggest an instruction-integrity attack on the agent or on Crane */
const INJECTION_PHRASES: &[&str] = &[
    "ignore previous instructions",
    "ignore all previous",
    "disregard the above",
    "disregard previous",
    "disable crane",
    "bypass crane",
    "uninstall crane",
    "edit .crane",
    "modify .crane",
    "delete .crane",
    "remove the hook",
    "disable the hook",
    "jailbreak",
    "reveal your system prompt",
];

/** What the pre-tool hook decided for a tool call, kept for the post-tool hook
 * Fields
    - at: u64 - Unix milliseconds of the decision
    - decision: Decision - authority decision
    - cost: u64 - credits the action costs
    - reserved: u64 - credits reserved
    - operation: String - read, write, execute, or other
    - tool: String - tool name
    - selections: Vec<String> - selections touched
    - before: Option<Tree> - the changed files before a shell or other tool ran, so its effect can
      be attributed to it afterwards
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CallRecord {
    at: u64,
    decision: Decision,
    cost: u64,
    reserved: u64,
    operation: String,
    tool: String,
    selections: Vec<String>,
    #[serde(default)]
    before: Option<Tree>,
}

/** The files that differ from HEAD, each with its size and modification time ("missing" when
 * deleted): enough to see that a file changed without reading it */
pub(super) type Tree = BTreeMap<String, String>;

/** Take a Tree of the work tree, leaving out .crane (its own signatures cover it)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Tree, String>
*/
pub(super) fn tree(workspace: &Workspace) -> Result<Tree, String> {
    Ok(crate::platform::git::changed_files(&workspace.root)?
        .into_iter()
        .filter(|path| !path.starts_with(".crane/"))
        .map(|path| {
            let state = fs::metadata(workspace.root.join(&path)).map_or_else(
                |_| "missing".to_string(),
                |metadata| {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |time| time.as_nanos());
                    format!("{}:{modified}", metadata.len())
                },
            );
            (path, state)
        })
        .collect())
}

/** List the files whose state differs between two Trees
 * Input
    - before: &Tree - earlier
    - after: &Tree - later
 * Output
    - BTreeSet<String>
*/
pub(super) fn tree_changes(before: &Tree, after: &Tree) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|path| before.get(*path) != after.get(*path))
        .cloned()
        .collect()
}

/** Per-session correlation state of tool calls
 * Fields
    - calls: BTreeMap<String, CallRecord> - pre-tool decisions by tool call
    - known_violations: BTreeSet<String> - violations already reported in this session
    - baseline: Option<Tree> - the changed files when the session started
    - attributed: BTreeSet<String> - files changed by the session's recorded tool calls
    - unattributed: BTreeSet<String> - files already reported as changed outside them
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Calls {
    calls: BTreeMap<String, CallRecord>,
    known_violations: BTreeSet<String>,
    #[serde(default)]
    baseline: Option<Tree>,
    #[serde(default)]
    attributed: BTreeSet<String>,
    #[serde(default)]
    unattributed: BTreeSet<String>,
}

/** Everything a hook invocation works with
 * Fields
    - adapter: Box<dyn AgentAdapter> - provider adapter
    - event: HookEvent - lifecycle event
    - provider: ProviderEvent - translated payload
    - workspace: Workspace - repository
    - state: State - governance state
    - stores: Stores - runtime stores
    - trust_home: Option<PathBuf> - the trust directory
    - started: Instant - when the hook started
    - hook_event_id: String - identifier of this hook invocation
*/
struct Hook {
    adapter: Box<dyn AgentAdapter>,
    event: HookEvent,
    provider: ProviderEvent,
    workspace: Workspace,
    state: State,
    stores: Stores,
    trust_home: Option<PathBuf>,
    started: Instant,
    hook_event_id: String,
}

/** Read the hook payload from stdin (empty input is Null); malformed JSON on a tool event is an
 * error, so the tool call is denied
 * Input
    - event: HookEvent - lifecycle event
 * Output
    - Result<Value, String>
*/
fn read_payload(event: HookEvent) -> Result<Value, String> {
    let stdin = std::io::stdin();
    if stdin.is_terminal() {
        return Ok(Value::Null);
    }
    let mut input = String::new();
    stdin
        .lock()
        .take(MAX_PAYLOAD)
        .read_to_string(&mut input)
        .map_err(|error| format!("could not read the hook payload: {error}"))?;
    if input.trim().is_empty() {
        return if event.has_action() {
            Err("the hook payload is empty, so the tool call cannot be checked".into())
        } else {
            Ok(Value::Null)
        };
    }
    serde_json::from_str(&input)
        .map_err(|error| format!("the hook payload is not valid JSON: {error}"))
}

/** Run one hook invocation: translate the payload and handle the event, answering fail-closed
 * through the adapter when anything goes wrong
 * Input
    - kind: AgentKind - provider profile
    - event: HookEvent - lifecycle event
 * Output
    - Result<(), String>
    - blocking error (outcome::block) when the provider must block
*/
pub(crate) fn run(kind: AgentKind, event: HookEvent) -> Result<(), String> {
    let started = Instant::now();
    let adapter = adapter(kind);
    let payload = match read_payload(event) {
        Ok(payload) => payload,
        Err(error) => return adapter.respond_failure(event, &error, false),
    };
    let provider = adapter.translate(event, &payload);
    let stop_hook_active = provider.stop_hook_active;
    let setup = (|| -> Result<(Workspace, State, Stores, Option<PathBuf>), String> {
        let workspace = Workspace::locate()?;
        let trust = workspace.trust()?;
        let state = State::load(&workspace)?;
        Ok((
            workspace,
            state,
            Stores::open(&trust.runtime()),
            Some(trust.home),
        ))
    })();
    let (workspace, state, stores, trust_home) = match setup {
        Ok(parts) => parts,
        Err(error) => return adapter.respond_failure(event, &error, stop_hook_active),
    };
    let hook_event_id = format!(
        "hk_{}",
        &digest(
            "hook/invocation/v1",
            &[
                event.name().as_bytes(),
                &now_millis().to_be_bytes(),
                &std::process::id().to_be_bytes()
            ]
        )[..24]
    );
    let hook = Hook {
        adapter,
        event,
        provider,
        workspace,
        state,
        stores,
        trust_home,
        started,
        hook_event_id,
    };
    match hook.handle() {
        Ok(()) => Ok(()),
        Err(error) if super::outcome::blocked(&error).is_some() => Err(error),
        Err(error) => {
            let _ = hook.record_failure(&error);
            hook.adapter
                .respond_failure(event, &error, stop_hook_active)
        }
    }
}

impl Hook {
    /** Handle the event
     * Input
        - None (uses self)
     * Output
        - Result<(), String>
    */
    fn handle(&self) -> Result<(), String> {
        let mut session = self.session()?;
        let result = match self.event {
            HookEvent::SessionStart => self.session_start(&mut session),
            HookEvent::UserPromptSubmit => self.prompt(&mut session),
            HookEvent::PreToolUse | HookEvent::PermissionRequest => self.pre_tool(&mut session),
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => self.post_tool(&mut session),
            HookEvent::Stop => self.stop(&mut session),
            HookEvent::SessionEnd => self.session_end(&mut session),
        };
        // A tool call is complete after post-tool use, a session after stop or session end
        match self.event {
            HookEvent::PostToolUse | HookEvent::PostToolUseFailure => {
                sync::trigger(&self.workspace, "POST_TOOL")
            }
            HookEvent::Stop => sync::trigger(&self.workspace, "STOP"),
            HookEvent::SessionEnd => sync::trigger(&self.workspace, "SESSION_END"),
            _ => sync::trigger_if_due(&self.workspace, &self.stores),
        }
        result
    }

    /** Find or create the session, and make sure it has a current task
     * Input
        - None (uses self)
     * Output
        - Result<SessionRecord, String>
    */
    fn session(&self) -> Result<SessionRecord, String> {
        let _lock = self.stores.identity.lock()?;
        let observed = Observed {
            provider: self.adapter.kind().name().into(),
            provider_session_id: self.provider.session.clone(),
            model: self.provider.model.clone(),
            cwd: self.provider.cwd.clone(),
            autonomy_mode: self.state.config.autonomy.mode.clone(),
            registry_generation: self.state.generation(),
        };
        let (mut session, created) = self.stores.identity.resolve_session(
            &self.workspace.repository_id,
            &observed,
            now_millis(),
        );
        if session.current_task.is_none() {
            self.stores.identity.start_task(
                &mut session,
                self.provider.task.clone(),
                None,
                now_millis(),
            )?;
        }
        self.stores.identity.save_session(&session)?;
        if created {
            self.emit(
                &session,
                self.base_event(
                    "session.created",
                    Domain::IdentityProvenance,
                    &session,
                    "created",
                ),
                None,
            )?;
        }
        Ok(session)
    }

    /** Reload a session, change it, and save it under the identity lock, so concurrent hooks of
     * one session never lose an update
     * Input
        - session: &mut SessionRecord - session (refreshed with the saved result)
        - change: impl FnOnce(&mut SessionRecord) - the change
     * Output
        - Result<(), String>
    */
    fn update_session(
        &self,
        session: &mut SessionRecord,
        change: impl FnOnce(&mut SessionRecord),
    ) -> Result<(), String> {
        let _lock = self.stores.identity.lock()?;
        let mut fresh = self
            .stores
            .identity
            .session(&session.foxx_session_id)
            .unwrap_or_else(|| session.clone());
        change(&mut fresh);
        fresh.last_seen_at = now_millis();
        self.stores.identity.save_session(&fresh)?;
        *session = fresh;
        Ok(())
    }

    /** Return the current task of a session
     * Input
        - session: &SessionRecord - session
     * Output
        - Option<TaskRecord>
    */
    fn task(&self, session: &SessionRecord) -> Option<TaskRecord> {
        session
            .current_task
            .as_ref()
            .and_then(|id| self.stores.identity.task(id))
    }

    /** Return the tool call identifier: the provider's tool_use_id, or one derived from the
     * session and the action digest (marked inferred in the event)
     * Input
        - session: &SessionRecord - session
        - action: &AgentAction - tool call
     * Output
        - (String, bool) identifier and whether the provider supplied it
    */
    fn tool_call_id(&self, session: &SessionRecord, action: &AgentAction) -> (String, bool) {
        match &self.provider.tool_call_id {
            Some(id) => (id.clone(), true),
            None => (
                format!(
                    "call_{}",
                    &digest(
                        "hook/tool-call/v1",
                        &[session.foxx_session_id.as_bytes(), action.digest.as_bytes()]
                    )[..24]
                ),
                false,
            ),
        }
    }

    /** Build an event with the session's identity and governance bindings
     * Input
        - event_type: &str - typed name
        - domain: Domain - security domain
        - session: &SessionRecord - session
        - key: &str - idempotency suffix
     * Output
        - Event
    */
    fn base_event(
        &self,
        event_type: &str,
        domain: Domain,
        session: &SessionRecord,
        key: &str,
    ) -> Event {
        let mut event = Event::new(
            event_type,
            domain,
            &self.workspace.repository_id,
            &format!("{}:{event_type}:{key}", session.foxx_session_id),
        );
        event.source = Source::ProviderAdapter;
        let task = self.task(session);
        event.scope = Scope {
            foxx_session_id: Some(session.foxx_session_id.clone()),
            foxx_task_id: task.as_ref().map(|task| task.foxx_task_id.clone()),
            external_task_id: task.as_ref().and_then(|task| task.external_task_id.clone()),
            provider: Some(session.provider.clone()),
            provider_session_id: session.provider_session_id.clone(),
            provider_turn_id: self.provider.turn.clone(),
            provider_task_id: task.as_ref().and_then(|task| task.provider_task_id.clone()),
            agent_instance_id: Some(session.agent_instance_id.clone()),
            model: session.model.clone(),
            tool_call_id: None,
            hook_event_id: Some(self.hook_event_id.clone()),
            trace_id: Some(session.foxx_session_id.clone()),
            span_id: Some(self.hook_event_id.clone()),
            parent_event_id: None,
        };
        event.bindings = Bindings {
            registry_generation: self.state.generation(),
            registry_head: self
                .state
                .registry
                .manifest
                .as_ref()
                .map(|manifest| manifest.head.clone()),
            policies: Vec::new(),
            selections: Vec::new(),
            checkpoint: self.state.config.default_checkpoint.clone(),
            autonomy_mode: Some(session.autonomy_mode.clone()),
            safety: Some(format!("{:?}", session.safety).to_uppercase()),
            zones: Vec::new(),
        };
        event.measurements.push(Measurement::observed(
            "hook_latency_ms",
            self.started.elapsed().as_millis() as f64,
            "ms",
            "hook process wall clock",
        ));
        event.payload = json!({
            "hook_event": self.event.name(),
            "identity_provenance": session.provenance,
            "user": self.workspace.user,
            "email": self.workspace.email,
        });
        // How the repository identity was found, reported once per hook process
        static REPORTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if self.workspace.identity_cache != "HIT"
            && !REPORTED.swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            event.payload["identity_cache"] = json!(self.workspace.identity_cache);
        }
        event
    }

    /** Store an event and raise an alert for it when it is serious enough
     * Input
        - session: &SessionRecord - session
        - event: Event - event to store
        - alert: Option<&str> - alert kind to raise
     * Output
        - Result<(), String>
    */
    fn emit(
        &self,
        session: &SessionRecord,
        event: Event,
        alert: Option<&str>,
    ) -> Result<(), String> {
        let mut event = event;
        if let Some(kind) = alert {
            event.payload["alert"] = json!(kind);
        }
        let stored = self.stores.events.append(event)?;
        if let (Some(kind), Some(event)) = (alert, stored) {
            alerts::raise(&self.workspace, &self.stores, kind, session, &event);
        }
        Ok(())
    }

    /** Record an internal failure of the hook as a coverage gap
     * Input
        - error: &str - what failed
     * Output
        - Result<(), String>
    */
    fn record_failure(&self, error: &str) -> Result<(), String> {
        let mut event = Event::new(
            "hook.failed",
            Domain::AuditCompliance,
            &self.workspace.repository_id,
            &format!("{}:failed", self.hook_event_id),
        );
        event.severity = Severity::High;
        event.decision = Some(Decision::Error);
        event.enforcement = Enforcement::CoverageGap;
        event.telemetry = Telemetry::Lost;
        event.reasons.push(error.to_string());
        event.scope.hook_event_id = Some(self.hook_event_id.clone());
        event.scope.provider_session_id = self.provider.session.clone();
        event.scope.provider = Some(self.adapter.kind().name().into());
        event.payload = json!({"hook_event": self.event.name(), "fail_closed": true});
        self.stores.events.append(event).map(|_| ())
    }

    /** Path of the per-session tool call correlation file
     * Input
        - session: &SessionRecord - session
     * Output
        - PathBuf
    */
    fn calls_path(&self, session: &SessionRecord) -> PathBuf {
        self.stores
            .directory
            .join("calls")
            .join(format!("{}.json", session.foxx_session_id))
    }

    /** Load, change, and save the correlation state under its lock
     * Input
        - session: &SessionRecord - session
        - change: impl FnOnce(&mut Calls) -> T - the change
     * Output
        - Result<T, String>
    */
    fn with_calls<T>(
        &self,
        session: &SessionRecord,
        change: impl FnOnce(&mut Calls) -> T,
    ) -> Result<T, String> {
        let path = self.calls_path(session);
        let _lock = Lock::acquire(&path.with_extension("lock"))?;
        let mut calls: Calls = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let result = change(&mut calls);
        write_atomic(
            &path,
            serde_json::to_string(&calls).map_err(io_error)?.as_bytes(),
        )?;
        Ok(result)
    }

    /** Available credits of a session
     * Input
        - session: &SessionRecord - session
     * Output
        - Result<u64, String>
    */
    fn available(&self, session: &SessionRecord) -> Result<u64, String> {
        Ok(Ledger::balance_of(&self.stores.ledger.entries()?, &session.foxx_session_id).available)
    }

    /** Apply a ledger operation for the session
     * Input
        - session: &SessionRecord - session
        - tool_call_id: Option<&str> - tool call
        - kind: Kind - operation
        - amount: u64 - amount
        - reason: &str - why
        - key: &str - idempotency key suffix
     * Output
        - Result<u64, String> amount applied
    */
    fn ledger(
        &self,
        session: &SessionRecord,
        tool_call_id: Option<&str>,
        kind: Kind,
        amount: u64,
        reason: &str,
        key: &str,
    ) -> Result<u64, String> {
        let task = session.current_task.clone();
        let (entry, _) = self.stores.ledger.apply(LedgerOperation {
            session: &session.foxx_session_id,
            task: task.as_deref(),
            tool_call_id,
            kind,
            amount,
            reason,
            registry_generation: self.state.generation(),
            idempotency_key: &format!("{}:{key}", session.foxx_session_id),
        })?;
        Ok(entry.amount)
    }
}

/** Collect the violations in a report, keyed so the same violation is recognized across tool
 * calls: failing or unresolved commands by selection, and blocking findings by code and subject
 * Input
    - report: &Report - verification report
 * Output
    - BTreeMap<String, String> key to explanation
*/
fn violations(report: &Report) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for policy in &report.policies {
        for command in &policy.commands {
            let failing = match command.outcome {
                Outcome::Fail => command.operation == SelectionOperation::Preserve,
                Outcome::Unresolved => true,
                Outcome::Pass | Outcome::Pending => false,
            };
            if failing {
                found.insert(
                    format!("selection:{}", command.id),
                    format!(
                        "{} {} ({}): {}",
                        command.operation.name(),
                        command.id,
                        policy.name,
                        command.message
                    ),
                );
            }
        }
    }
    for finding in report.findings.iter().filter(|finding| finding.blocking()) {
        let subject = finding
            .selection
            .clone()
            .or(finding.path.clone())
            .unwrap_or_default();
        found
            .entry(format!("finding:{}:{subject}", finding.code))
            .or_insert_with(|| finding.message.clone());
    }
    found
}

/** Explain how to repair violations: for a changed preserved selection, the baseline content to
 * restore between its markers (from the checkpoint); for marker problems, how to restore markers
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - report: &Report - verification report
    - keys: &[String] - violation keys to explain
    - violations: &BTreeMap<String, String> - all violations
 * Output
    - String
*/
fn repair_instructions(
    workspace: &Workspace,
    state: &State,
    report: &Report,
    keys: &[String],
    violations: &BTreeMap<String, String>,
) -> String {
    let mut text = String::new();
    for key in keys {
        text.push_str(&format!(
            "- {}\n",
            violations.get(key).cloned().unwrap_or_default()
        ));
        let Some(id) = key.strip_prefix("selection:") else {
            continue;
        };
        let Some(record) = state.record(id) else {
            continue;
        };
        let resolved = report.resolutions.get(id).cloned().flatten();
        if record.operation == SelectionOperation::Preserve {
            if let (Ok(content), Some(span)) = (baseline(workspace, record), resolved) {
                let excerpt: String = content.chars().take(MAX_RESTORE_EXCERPT).collect();
                text.push_str(&format!(
                    "  Restore the content between the markers of {id} in {} to exactly this (from checkpoint {}):\n{excerpt}{}\n",
                    span.path,
                    record.origin.checkpoint,
                    if excerpt.len() < content.len() { "\n  [truncated]" } else { "" }
                ));
                continue;
            }
        }
        text.push_str(&format!(
            "  Selection {id} must be enclosed again by its exact markers in {} (do not create, copy, or move markers); a human resolves it with crane validate if it cannot be restored.\n",
            record.current.span.path
        ));
    }
    text.push_str(&format!(
        "Verification report:\n{}\n",
        serde_json::to_string(&json!({"findings": report.findings, "policies": report.policies}))
            .unwrap_or_default()
    ));
    text
}
