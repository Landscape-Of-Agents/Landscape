// Read models shared by the task and session commands and the hosted Data Sea dashboard: sessions,
// tasks, agents, and tool calls with their evals (DASHBOARD.md), built from the specification
// events of the local runtime stores or, on the service, of the uploaded records.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::governance::workspace::Workspace;
use crate::platform::render::iso_time;
use crate::telemetry::evals::summarize;
use crate::telemetry::events::Event;
use crate::telemetry::identity::{SessionRecord, TaskRecord};
use crate::telemetry::ledger::{Entry, Ledger};
use crate::telemetry::spec;
use crate::telemetry::Stores;

/** Everything the read models are built from
 * Fields
    - events: Vec<Event> - every event
    - ledger: Vec<Entry> - every ledger entry
    - sessions: Vec<SessionRecord> - sessions, most recent first
    - tasks: Vec<TaskRecord> - tasks, most recent first
    - alerts: Vec<Value> - alerts, newest first
*/
#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    pub(crate) events: Vec<Event>,
    pub(crate) ledger: Vec<Entry>,
    pub(crate) sessions: Vec<SessionRecord>,
    pub(crate) tasks: Vec<TaskRecord>,
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
    pub(crate) alerts: Vec<Value>,
}

impl Snapshot {
    /** Load the local runtime stores of a repository (complete and authoritative on this machine)
     * Input
        - workspace: &Workspace - repository
     * Output
        - Result<Snapshot, String>
    */
    pub(crate) fn load(workspace: &Workspace) -> Result<Self, String> {
        let stores = Stores::open(&workspace.trust()?.runtime());
        Ok(Self {
            events: stores.events.read_all()?,
            ledger: stores.ledger.entries()?,
            sessions: stores.identity.sessions(),
            tasks: stores.identity.tasks(),
            alerts: crate::telemetry::alerts::list(&stores),
        })
    }

    /** Events of one session
     * Input
        - session: &str - canonical session
     * Output
        - Vec<&Event>
    */
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
    pub(crate) fn session_events(&self, session: &str) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|event| event.scope.foxx_session_id.as_deref() == Some(session))
            .collect()
    }

    /** Every specification event of the stores: those stored with each event (projected now for
     * events stored before specifications existed) and the credit events of the ledger, in time
     * order
     * Input
        - None (uses self)
     * Output
        - Vec<Value> with envelopes
    */
    pub(crate) fn specs(&self) -> Vec<Value> {
        let repository = self
            .events
            .first()
            .map(|event| event.repository_id.clone())
            .unwrap_or_default();
        let mut all = Vec::new();
        for event in &self.events {
            let specs = if event.spec.is_empty() {
                spec::project(event)
            } else {
                event.spec.clone()
            };
            for (index, value) in specs.iter().enumerate() {
                all.push(spec::envelope(event, value, index));
            }
        }
        all.extend(
            self.ledger
                .iter()
                .filter_map(|entry| spec::from_ledger(entry, &repository)),
        );
        all.sort_by_key(|value| value["occurred_at"].as_u64().unwrap_or(0));
        all
    }

    /** Find a session by canonical or provider identifier
     * Input
        - id: &str - identifier
     * Output
        - Option<&SessionRecord>
    */
    pub(crate) fn session(&self, id: &str) -> Option<&SessionRecord> {
        self.sessions.iter().find(|session| {
            session.foxx_session_id == id || session.provider_session_id.as_deref() == Some(id)
        })
    }

    /** Find a task by canonical, external, or provider identifier
     * Input
        - id: &str - identifier
     * Output
        - Option<&TaskRecord>
    */
    pub(crate) fn task(&self, id: &str) -> Option<&TaskRecord> {
        self.tasks.iter().find(|task| {
            task.foxx_task_id == id
                || task.external_task_id.as_deref() == Some(id)
                || task.provider_task_id.as_deref() == Some(id)
        })
    }
}

/** Format optional Unix milliseconds
 * Input
    - millis: Option<u64> - time
 * Output
    - Value, null when absent
*/
fn time(millis: Option<u64>) -> Value {
    millis.map_or(Value::Null, |millis| json!(iso_time(millis)))
}

/** The specification events of one session or task
 * Input
    - specs: &[Value] - every specification event
    - group_key: (&str, &str) - ("session", "session_id") or ("session", "task_id")
    - id: &str - identifier
 * Output
    - Vec<&Value>
*/
pub(crate) fn of<'a>(specs: &'a [Value], group_key: (&str, &str), id: &str) -> Vec<&'a Value> {
    specs
        .iter()
        .filter(|spec| spec[group_key.0][group_key.1].as_str() == Some(id))
        .collect()
}

/** Build the view of a task, with its evals
 * Input
    - snapshot: &Snapshot - stores
    - task: &TaskRecord - task
 * Output
    - Value
*/
pub(crate) fn task_view(snapshot: &Snapshot, task: &TaskRecord) -> Value {
    task_view_with(snapshot, task, &snapshot.specs())
}

/** Build the view of a task from already built specification events
 * Input
    - snapshot: &Snapshot - stores
    - task: &TaskRecord - task
    - specs: &[Value] - every specification event
 * Output
    - Value
*/
pub(crate) fn task_view_with(snapshot: &Snapshot, task: &TaskRecord, specs: &[Value]) -> Value {
    let session = snapshot.session(&task.foxx_session_id);
    let events = of(specs, ("session", "task_id"), &task.foxx_task_id);
    let calls = events
        .iter()
        .filter(|spec| spec["flow"] == "F05")
        .map(|spec| {
            spec["action"]["action_id"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    json!({
        "task": {
            "foxx_task_id": task.foxx_task_id,
            "external_task_id": task.external_task_id,
            "provider_task_id": task.provider_task_id,
            "id_provenance": task.provenance,
            "name": task.name,
            "status": task.status,
            "sequence": task.sequence,
            "foxx_session_id": task.foxx_session_id,
            "agent": session.map(|session| format!("{} ({})", session.provider, session.model.clone().unwrap_or_else(|| "model not reported".into()))),
            "started_at": iso_time(task.started_at),
            "ended_at": time(task.ended_at),
            "prompt": task.prompt_chars.map(|chars| format!("{chars} characters, sha512 {}", task.prompt_digest.clone().unwrap_or_default().chars().take(16).collect::<String>())),
            "tool_calls": calls.len(),
        },
        "evals": summarize(&events),
    })
}

/** Build the view of a session, with its tasks, credits, and evals
 * Input
    - snapshot: &Snapshot - stores
    - session: &SessionRecord - session
 * Output
    - Value
*/
pub(crate) fn session_view(snapshot: &Snapshot, session: &SessionRecord) -> Value {
    session_view_with(snapshot, session, &snapshot.specs())
}

/** Build the view of a session from already built specification events
 * Input
    - snapshot: &Snapshot - stores
    - session: &SessionRecord - session
    - specs: &[Value] - every specification event
 * Output
    - Value
*/
pub(crate) fn session_view_with(
    snapshot: &Snapshot,
    session: &SessionRecord,
    specs: &[Value],
) -> Value {
    let events = of(specs, ("session", "session_id"), &session.foxx_session_id);
    let balance = Ledger::balance_of(&snapshot.ledger, &session.foxx_session_id);
    json!({
        "session": {
            "foxx_session_id": session.foxx_session_id,
            "provider": session.provider,
            "provider_session_id": session.provider_session_id,
            "id_provenance": session.provenance,
            "agent_id": session.agent_id,
            "agent_instance_id": session.agent_instance_id,
            "model": session.model,
            "status": if session.ended_at.is_some() { "ENDED" } else { "ACTIVE" },
            "started_at": iso_time(session.started_at),
            "last_seen_at": iso_time(session.last_seen_at),
            "ended_at": time(session.ended_at),
            "autonomy_mode": session.autonomy_mode,
            "safety": session.safety,
            "safety_reason": session.safety_reason,
            "bypass_attempts": session.bypass_attempts,
            "violations": session.violations,
            "current_task": session.current_task,
            "tasks": session.tasks.len(),
            "registry_generation_at_start": session.registry_generation,
            "context_supplied": session.context,
            "identity_conflicts": session.conflicts,
        },
        "credits": balance,
        "evals": summarize(&events),
    })
}

/** Build the per-agent longitudinal view: every agent identity with its sessions and evals
 * Input
    - snapshot: &Snapshot - stores
    - specs: &[Value] - every specification event
 * Output
    - Vec<Value>
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn agents(snapshot: &Snapshot, specs: &[Value]) -> Vec<Value> {
    let mut by_agent: BTreeMap<String, Vec<&SessionRecord>> = BTreeMap::new();
    for session in &snapshot.sessions {
        by_agent
            .entry(session.agent_id.clone())
            .or_default()
            .push(session);
    }
    by_agent
        .into_iter()
        .map(|(agent, sessions)| {
            let ids = sessions.iter().map(|session| session.foxx_session_id.clone()).collect::<BTreeSet<_>>();
            let events = specs
                .iter()
                .filter(|spec| spec["session"]["session_id"].as_str().is_some_and(|id| ids.contains(id)))
                .collect::<Vec<_>>();
            json!({
                "agent_id": agent,
                "provider": sessions[0].provider,
                "sessions": sessions.len(),
                "first_seen": iso_time(sessions.iter().map(|session| session.started_at).min().unwrap_or(0)),
                "last_seen": iso_time(sessions.iter().map(|session| session.last_seen_at).max().unwrap_or(0)),
                "quarantined_sessions": sessions.iter().filter(|session| format!("{:?}", session.safety) == "Quarantined").count(),
                "evals": summarize(&events),
            })
        })
        .collect()
}
