// Read models shared by the task and session commands and the hosted Foxx dashboard: sessions,
// tasks, agents, and tool calls with their metrics, built from the local runtime stores or, on the
// service, from the uploaded records.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::governance::workspace::Workspace;
use crate::platform::render::iso_time;
use crate::telemetry::events::Event;
use crate::telemetry::identity::{SessionRecord, TaskRecord};
use crate::telemetry::ledger::{Entry, Ledger};
use crate::telemetry::metrics::{self, Metric};
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
    pub(crate) fn session_events(&self, session: &str) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|event| event.scope.foxx_session_id.as_deref() == Some(session))
            .collect()
    }

    /** Events of one task
     * Input
        - task: &str - canonical task
     * Output
        - Vec<&Event>
    */
    pub(crate) fn task_events(&self, task: &str) -> Vec<&Event> {
        self.events
            .iter()
            .filter(|event| event.scope.foxx_task_id.as_deref() == Some(task))
            .collect()
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

/** Convert metrics into a name-keyed map of value, unit, and status
 * Input
    - metrics: &[Metric] - computed metrics
 * Output
    - Value
*/
pub(crate) fn metric_map(metrics: &[Metric]) -> Value {
    let mut map = serde_json::Map::new();
    for metric in metrics {
        map.insert(
            metric.name.into(),
            json!({"value": metric.value, "unit": metric.unit, "status": metric.status, "domain": metric.domain}),
        );
    }
    Value::Object(map)
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

/** Build the view of a task, with its metrics
 * Input
    - snapshot: &Snapshot - stores
    - task: &TaskRecord - task
 * Output
    - Value
*/
pub(crate) fn task_view(snapshot: &Snapshot, task: &TaskRecord) -> Value {
    let session = snapshot.session(&task.foxx_session_id);
    let events = snapshot.task_events(&task.foxx_task_id);
    let calls = events
        .iter()
        .filter_map(|event| event.scope.tool_call_id.clone())
        .collect::<BTreeSet<_>>();
    let ledger = snapshot
        .ledger
        .iter()
        .filter(|entry| entry.task.as_deref() == Some(task.foxx_task_id.as_str()))
        .collect::<Vec<_>>();
    let session_json = session
        .map(|session| serde_json::to_value(session).unwrap_or_default())
        .into_iter()
        .collect::<Vec<_>>();
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
        "metrics": metric_map(&metrics::compute(&events, &ledger, &session_json)),
    })
}

/** Build the view of a session, with its tasks, credits, and metrics
 * Input
    - snapshot: &Snapshot - stores
    - session: &SessionRecord - session
 * Output
    - Value
*/
pub(crate) fn session_view(snapshot: &Snapshot, session: &SessionRecord) -> Value {
    let events = snapshot.session_events(&session.foxx_session_id);
    let ids = BTreeSet::from([session.foxx_session_id.clone()]);
    let ledger = metrics::ledger_of(&snapshot.ledger, &ids);
    let balance = Ledger::balance_of(&snapshot.ledger, &session.foxx_session_id);
    let record = serde_json::to_value(session).unwrap_or_default();
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
        "metrics": metric_map(&metrics::compute(&events, &ledger, &[record])),
    })
}

/** Build the per-agent longitudinal view: every agent identity with its sessions and metrics
 * Input
    - snapshot: &Snapshot - stores
 * Output
    - Vec<Value>
*/
#[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
pub(crate) fn agents(snapshot: &Snapshot) -> Vec<Value> {
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
            let events = snapshot
                .events
                .iter()
                .filter(|event| event.scope.foxx_session_id.as_ref().is_some_and(|id| ids.contains(id)))
                .collect::<Vec<_>>();
            let records = sessions.iter().map(|session| serde_json::to_value(session).unwrap_or_default()).collect::<Vec<_>>();
            json!({
                "agent_id": agent,
                "provider": sessions[0].provider,
                "sessions": sessions.len(),
                "first_seen": iso_time(sessions.iter().map(|session| session.started_at).min().unwrap_or(0)),
                "last_seen": iso_time(sessions.iter().map(|session| session.last_seen_at).max().unwrap_or(0)),
                "quarantined_sessions": sessions.iter().filter(|session| format!("{:?}", session.safety) == "Quarantined").count(),
                "metrics": metric_map(&metrics::compute(&events, &metrics::ledger_of(&snapshot.ledger, &ids), &records)),
            })
        })
        .collect()
}
