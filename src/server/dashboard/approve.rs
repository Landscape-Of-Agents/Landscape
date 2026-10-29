// Approvals a person makes on the dashboard after GitHub confirms their role: linking a machine's
// upload key to their GitHub account (they can read the repository), confirming the repository
// owner's key (they administer the repository), and granting autonomy credits to an agent's session
// (they administer the repository). Links and owner keys start on the person's machine ('crane agent
// link', 'crane admin register'), which leaves a request with a one-time code at the service; a
// grant starts on the dashboard. Either way the person signs in with GitHub, and GitHub's answer for
// that repository decides. This module also contains the dashboard's narrow, repository-scoped
// writes for monitor-only Dashboard Agents and their incidents. Telemetry/evidence reads remain
// isolated in data.rs.

use mongodb::bson::{self, doc, Bson, Document};
use mongodb::sync::Database;
use serde_json::{json, Value};

use super::auth::{Access, Intent, Ownership};
use crate::platform::{io_error, now_millis};
use crate::telemetry::ingest::CODE_DOMAIN;
use crate::trust::crypto::digest;
use crate::trust::crypto::random_id;

/** The dashboard's narrow write access
 * Fields
    - database: Database - MongoDB (private, so only the methods below can write)
*/
pub(crate) struct Approvals {
    database: Database,
}

/** One evaluation of a dashboard agent
 * Fields
    - agent_id: &str - agent
    - observed: u64 - matching events of its headline signal
    - threshold: u64 - threshold of its headline signal
    - triggered: bool - whether any signal met its threshold
    - evaluated_at: u64 - evaluation time in milliseconds
    - signals: Value - every signal with its observation
*/
pub(crate) struct DashboardAgentEvaluation<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) observed: u64,
    pub(crate) threshold: u64,
    pub(crate) triggered: bool,
    pub(crate) evaluated_at: u64,
    pub(crate) signals: Value,
}

impl Approvals {
    /** Wrap a database
     * Input
        - database: Database - MongoDB
     * Output
        - Approvals
    */
    pub(crate) fn new(database: Database) -> Self {
        Self { database }
    }

    /** Save a validated monitor-only dashboard agent. */
    pub(crate) fn save_dashboard_agent(
        &self,
        repository: &str,
        login: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let name = Self::bounded_text(input, "name", 80)?;
        let reason = Self::bounded_text(input, "reason", 500)?;
        let state = input["state"].as_str().unwrap_or("active");
        if !matches!(state, "active" | "inactive" | "sleep" | "idle") {
            return Err("agent state must be active, inactive, sleep, or idle".into());
        }
        // Clients from before signals existed send one metric, threshold, and window
        let requested = match input["signals"].as_array() {
            Some(items) => items.clone(),
            None => vec![json!({
                "metric": input["metric"],
                "threshold": input["threshold"],
                "window_minutes": input["window_minutes"],
            })],
        };
        if requested.is_empty() || requested.len() > 10 {
            return Err("an agent needs between 1 and 10 signals".into());
        }
        let signals = requested
            .iter()
            .map(Self::dashboard_agent_signal)
            .collect::<Result<Vec<_>, _>>()?;
        let mut seen = std::collections::BTreeSet::new();
        if !signals.iter().all(|signal| {
            seen.insert((
                signal["metric"].to_string(),
                signal["window_minutes"].as_i64(),
            ))
        }) {
            return Err("each signal needs a different metric or window".into());
        }
        // The first signal is mirrored at the top level, which older readers and the
        // collection's required fields expect
        let metric = signals[0]["metric"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let threshold = signals[0]["threshold"].as_i64().unwrap_or_default();
        let window_minutes = signals[0]["window_minutes"].as_i64().unwrap_or_default();
        let assignments = input["assignments"]
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 100)
            .ok_or("assign at least one flow, zone, selection, or all events")?;
        for assignment in assignments {
            let kind = assignment["kind"].as_str().unwrap_or_default();
            let id = assignment["id"].as_str().unwrap_or_default().trim();
            if !matches!(kind, "flow" | "zone" | "selection" | "all")
                || id.is_empty()
                || id.len() > 128
                || (kind == "all" && id != "*")
            {
                return Err(
                    "each assignment needs a valid type and identifier (all uses '*')".into(),
                );
            }
        }
        let assignments = bson::to_bson(assignments).map_err(|error| error.to_string())?;
        let agent_id = match input["agent_id"].as_str() {
            Some(id)
                if (8..=64).contains(&id.len())
                    && id.bytes().all(|byte| byte.is_ascii_alphanumeric()) =>
            {
                id.to_string()
            }
            Some(_) => return Err("agent_id is invalid".into()),
            None => random_id()?,
        };
        let now = now_millis() as i64;
        let set = doc! {
            "name": &name,
            "reason": reason,
            "state": state,
            "metric": &metric,
            "threshold": threshold,
            "window_minutes": window_minutes,
            "signals": bson::to_bson(&signals).map_err(|error| error.to_string())?,
            "assignments": assignments,
            "monitoring": "METRICS_AND_CRITICAL_EVENTS",
            "updated_at": now,
        };
        let result = self
            .database
            .collection::<Document>("dashboard_agents")
            .update_one(
                doc! {"repository_id": repository, "agent_id": &agent_id},
                doc! {
                    "$set": set,
                    "$setOnInsert": {
                        "agent_id": &agent_id,
                        "repository_id": repository,
                        "created_by": login,
                        "created_at": now,
                    },
                },
            )
            .upsert(true)
            .run()
            .map_err(io_error)?;
        self.record_dashboard_agent_activity(
            repository,
            &agent_id,
            login,
            if result.upserted_id.is_some() {
                "agent_created"
            } else {
                "agent_updated"
            },
            &json!({
                "name": name,
                "state": state,
                "metric": metric,
                "threshold": threshold,
                "window_minutes": window_minutes,
                "signals": signals,
                "assignments": input["assignments"],
            }),
        )?;
        Ok(json!({"agent_id": agent_id, "name": name, "state": state, "updated_at": now}))
    }

    /** Validate one signal of a dashboard agent
     * Input
        - input: &Value - {metric, threshold, window_minutes}
     * Output
        - Result<Value, String> - the signal with only those three fields
        - Error if the metric is unknown, the threshold is outside 1..=100000, or the window is
          outside 1..=1440 minutes
    */
    fn dashboard_agent_signal(input: &Value) -> Result<Value, String> {
        let metric = input["metric"].as_str().unwrap_or_default();
        if !matches!(
            metric,
            "critical_events"
                | "denials"
                | "prompt_injections"
                | "bypass_attempts"
                | "tool_failures"
                | "event_volume"
        ) {
            return Err("agent metric is not supported".into());
        }
        let threshold = input["threshold"]
            .as_u64()
            .filter(|value| (1..=100_000).contains(value))
            .ok_or("agent threshold must be between 1 and 100000")?;
        let window_minutes = input["window_minutes"]
            .as_u64()
            .filter(|value| (1..=1440).contains(value))
            .ok_or("agent window must be between 1 and 1440 minutes")?;
        Ok(json!({"metric": metric, "threshold": threshold, "window_minutes": window_minutes}))
    }

    /** Change only an agent's monitoring state. */
    pub(crate) fn set_dashboard_agent_state(
        &self,
        repository: &str,
        agent_id: &str,
        login: &str,
        state: &str,
    ) -> Result<(), String> {
        if !matches!(state, "active" | "inactive" | "sleep" | "idle") {
            return Err("agent state must be active, inactive, sleep, or idle".into());
        }
        let result = self
            .database
            .collection::<Document>("dashboard_agents")
            .update_one(
                doc! {"repository_id": repository, "agent_id": agent_id},
                doc! {"$set": {"state": state, "updated_at": now_millis() as i64}},
            )
            .run()
            .map_err(io_error)?;
        if result.matched_count == 0 {
            return Err("dashboard agent not found".into());
        }
        self.record_dashboard_agent_activity(
            repository,
            agent_id,
            login,
            "agent_state_changed",
            &json!({"state": state}),
        )?;
        Ok(())
    }

    /** Insert an incident once per agent and evaluation window. */
    pub(crate) fn record_dashboard_incident(
        &self,
        repository: &str,
        created_by: &str,
        incident: &Value,
    ) -> Result<bool, String> {
        let agent_id = Self::bounded_text(incident, "agent_id", 64)?;
        let incident_key = Self::bounded_text(incident, "incident_key", 160)?;
        let now = now_millis() as i64;
        let incident_id = random_id()?;
        let document = doc! {
            "incident_id": &incident_id,
            "incident_key": &incident_key,
            "repository_id": repository,
            "agent_id": &agent_id,
            "agent_name": incident["agent_name"].as_str().unwrap_or("Dashboard agent"),
            "created_at": now,
            "updated_at": now,
            "status": "OPEN",
            "severity": incident["severity"].as_str().unwrap_or("HIGH"),
            "reason": Self::bounded_text(incident, "reason", 1000)?,
            "metric": incident["metric"].as_str().unwrap_or("unknown"),
            "observed": incident["observed"].as_u64().unwrap_or_default() as i64,
            "threshold": incident["threshold"].as_u64().unwrap_or_default() as i64,
            "assignments": bson::to_bson(&incident["assignments"]).map_err(|error| error.to_string())?,
            "event_ids": bson::to_bson(&incident["event_ids"]).map_err(|error| error.to_string())?,
            "three_a": bson::to_bson(&incident["three_a"]).map_err(|error| error.to_string())?,
            "created_by": created_by,
            "resolved_by": Bson::Null,
        };
        let result = self
            .database
            .collection::<Document>("dashboard_incidents")
            .update_one(
                doc! {"repository_id": repository, "incident_key": incident_key},
                doc! {"$setOnInsert": document},
            )
            .upsert(true)
            .run()
            .map_err(io_error)?;
        if result.upserted_id.is_some() {
            self.record_dashboard_agent_activity(
                repository,
                &agent_id,
                created_by,
                if incident["metric"] == "manual_review" {
                    "incident_reported"
                } else {
                    "incident_opened"
                },
                &json!({
                    "incident_id": incident_id,
                    "severity": incident["severity"],
                    "metric": incident["metric"],
                    "observed": incident["observed"],
                    "threshold": incident["threshold"],
                    "event_ids": incident["event_ids"],
                    "reason": incident["reason"],
                }),
            )?;
        }
        Ok(result.upserted_id.is_some())
    }

    /** Add one bounded 3A note and optionally resolve the incident. */
    pub(crate) fn annotate_dashboard_incident(
        &self,
        repository: &str,
        incident_id: &str,
        login: &str,
        note: &str,
        resolve: bool,
    ) -> Result<(), String> {
        let note = note.trim();
        if note.is_empty() || note.len() > 1000 {
            return Err("incident note must contain 1-1000 characters".into());
        }
        let entry = doc! {
            "at": now_millis() as i64,
            "kind": if resolve { "mitigation" } else { "human_analysis" },
            "actor": login,
            "text": note,
        };
        let mut fields = doc! {"updated_at": now_millis() as i64};
        if resolve {
            fields.insert("status", "RESOLVED");
            fields.insert("resolved_by", login);
            fields.insert("resolved_at", now_millis() as i64);
        }
        let update = doc! {
            "$push": {"three_a": entry},
            "$set": fields,
        };
        let result = self
            .database
            .collection::<Document>("dashboard_incidents")
            .update_one(
                doc! {"repository_id": repository, "incident_id": incident_id, "status": "OPEN"},
                update,
            )
            .run()
            .map_err(io_error)?;
        if result.matched_count == 0 {
            return Err("open dashboard incident not found".into());
        }
        self.record_dashboard_agent_activity(
            repository,
            "",
            login,
            if resolve {
                "incident_resolved"
            } else {
                "incident_note_added"
            },
            &json!({"incident_id": incident_id, "note": note}),
        )?;
        Ok(())
    }

    pub(crate) fn request_dashboard_quarantine(
        &self,
        repository: &str,
        login: &str,
        input: &Value,
    ) -> Result<Value, String> {
        let kind = input["kind"].as_str().unwrap_or_default();
        if !matches!(kind, "agent" | "session" | "tool") {
            return Err("quarantine kind must be agent, session, or tool".into());
        }
        let target_id = Self::bounded_text(input, "target_id", 128)?;
        if !target_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
        {
            return Err("quarantine target contains unsupported characters".into());
        }
        let scope_session_id = input["scope_session_id"]
            .as_str()
            .unwrap_or_default()
            .trim();
        if kind == "tool" {
            if scope_session_id.is_empty()
                || scope_session_id.len() > 128
                || !scope_session_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
            {
                return Err("tool quarantine requires a valid session ID scope".into());
            }
        } else if !scope_session_id.is_empty() {
            return Err("only tool quarantines may specify a session scope".into());
        }
        let reason = Self::bounded_text(input, "reason", 1000)?;
        let incident_id = input["incident_id"].as_str().unwrap_or_default().trim();
        if !incident_id.is_empty() {
            if incident_id.len() > 64 {
                return Err("incident_id is invalid".into());
            }
            if self
                .database
                .collection::<Document>("dashboard_incidents")
                .find_one(doc! {"repository_id": repository, "incident_id": incident_id})
                .run()
                .map_err(io_error)?
                .is_none()
            {
                return Err("incident_id does not belong to this repository".into());
            }
        }
        let quarantine_id = random_id()?;
        let now = now_millis() as i64;
        self.database
            .collection::<Document>("dashboard_quarantines")
            .insert_one(doc! {
                "quarantine_id": &quarantine_id,
                "repository_id": repository,
                "kind": kind,
                "target_id": &target_id,
                "scope_session_id": if scope_session_id.is_empty() { Bson::Null } else { Bson::String(scope_session_id.to_string()) },
                "incident_id": if incident_id.is_empty() { Bson::Null } else { Bson::String(incident_id.to_string()) },
                "reason": &reason,
                "requested_by": login,
                "requested_at": now,
                "state": "PENDING",
            })
            .run()
            .map_err(io_error)?;
        self.record_dashboard_agent_activity(
            repository,
            "",
            login,
            "quarantine_requested",
            &json!({"quarantine_id": quarantine_id, "incident_id": incident_id, "kind": kind, "target_id": target_id, "scope_session_id": scope_session_id, "reason": reason}),
        )?;
        Ok(json!({"quarantine_id": quarantine_id, "state": "PENDING"}))
    }

    pub(crate) fn request_dashboard_quarantine_release(
        &self,
        repository: &str,
        quarantine_id: &str,
        login: &str,
    ) -> Result<(), String> {
        let incident_id = self
            .database
            .collection::<Document>("dashboard_quarantines")
            .find_one(doc! {"repository_id": repository, "quarantine_id": quarantine_id})
            .run()
            .map_err(io_error)?
            .and_then(|record| record.get_str("incident_id").ok().map(String::from))
            .unwrap_or_default();
        let result = self
            .database
            .collection::<Document>("dashboard_quarantines")
            .update_one(
                doc! {
                    "repository_id": repository,
                    "quarantine_id": quarantine_id,
                    "state": "ACTIVE",
                    "$or": [
                        {"release_decision": {"$exists": false}},
                        {"release_decision": "REJECTED"},
                    ],
                },
                doc! {"$set": {
                    "release_requested_by": login,
                    "release_requested_at": now_millis() as i64,
                    "release_decision": "PENDING",
                }},
            )
            .run()
            .map_err(io_error)?;
        if result.matched_count == 0 {
            return Err("active quarantine not found or release already requested".into());
        }
        self.record_dashboard_agent_activity(
            repository,
            "",
            login,
            "quarantine_release_requested",
            &json!({"quarantine_id": quarantine_id, "incident_id": incident_id}),
        )
    }

    pub(crate) fn propose_dashboard_flow_context(
        &self,
        repository: &str,
        login: &str,
        incident_id: &str,
        flow_id: &str,
    ) -> Result<Value, String> {
        if flow_id.is_empty()
            || flow_id.len() > 128
            || !flow_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
        {
            return Err("flow_id is invalid".into());
        }
        let incident = self
            .database
            .collection::<Document>("dashboard_incidents")
            .find_one(doc! {"repository_id": repository, "incident_id": incident_id})
            .run()
            .map_err(io_error)?
            .ok_or("incident not found")?;
        let activities = self
            .database
            .collection::<Document>("dashboard_agent_activity")
            .find(doc! {"repository_id": repository, "details.incident_id": incident_id})
            .sort(doc! {"at": 1})
            .limit(500)
            .run()
            .map_err(io_error)?;
        let mut response_actions = Vec::new();
        for activity in activities {
            let activity = activity.map_err(io_error)?;
            response_actions.push(Bson::Document(doc! {
                "at": activity.get("at").cloned().unwrap_or(Bson::Null),
                "action": activity.get("action").cloned().unwrap_or(Bson::Null),
                "actor": activity.get("actor").cloned().unwrap_or(Bson::Null),
                "details": activity.get("details").cloned().unwrap_or(Bson::Document(Document::new())),
            }));
        }
        let context = doc! {
            "incident_reason": incident.get_str("reason").unwrap_or_default(),
            "severity": incident.get_str("severity").unwrap_or("UNKNOWN"),
            "metric": incident.get_str("metric").unwrap_or("unknown"),
            "three_a": incident.get("three_a").cloned().unwrap_or(Bson::Array(Vec::new())),
            "event_ids": incident.get("event_ids").cloned().unwrap_or(Bson::Array(Vec::new())),
            "response_actions": Bson::Array(response_actions),
        };
        let context_id = random_id()?;
        let now = now_millis() as i64;
        self.database
            .collection::<Document>("dashboard_flow_context")
            .insert_one(doc! {
                "context_id": &context_id,
                "repository_id": repository,
                "incident_id": incident_id,
                "flow_id": flow_id,
                "context": context,
                "proposed_by": login,
                "proposed_at": now,
                "status": "PENDING",
            })
            .run()
            .map_err(io_error)?;
        self.record_dashboard_agent_activity(
            repository,
            incident.get_str("agent_id").unwrap_or_default(),
            login,
            "flow_context_proposed",
            &json!({"context_id": context_id, "incident_id": incident_id, "flow_id": flow_id}),
        )?;
        Ok(json!({"context_id": context_id, "status": "PENDING"}))
    }

    pub(crate) fn approve_dashboard_action(
        &self,
        repository: &str,
        action: &str,
        record_id: &str,
        login: &str,
    ) -> Result<(), String> {
        let now = now_millis() as i64;
        let (collection, filter, fields, activity, details) = match action {
            "approve-quarantine" => (
                "dashboard_quarantines",
                doc! {"repository_id": repository, "quarantine_id": record_id, "state": "PENDING"},
                doc! {"state": "ACTIVE", "approved_by": login, "approved_at": now},
                "quarantine_activated",
                json!({"quarantine_id": record_id}),
            ),
            "reject-quarantine" => (
                "dashboard_quarantines",
                doc! {"repository_id": repository, "quarantine_id": record_id, "state": "PENDING"},
                doc! {"state": "REJECTED", "rejected_by": login, "rejected_at": now},
                "quarantine_rejected",
                json!({"quarantine_id": record_id}),
            ),
            "release-quarantine" => (
                "dashboard_quarantines",
                doc! {
                    "repository_id": repository,
                    "quarantine_id": record_id,
                    "state": "ACTIVE",
                    "release_decision": "PENDING",
                },
                doc! {
                    "state": "RELEASED",
                    "released_by": login,
                    "released_at": now,
                    "release_decision": "APPROVED",
                },
                "quarantine_released",
                json!({"quarantine_id": record_id}),
            ),
            "reject-release" => (
                "dashboard_quarantines",
                doc! {
                    "repository_id": repository,
                    "quarantine_id": record_id,
                    "state": "ACTIVE",
                    "release_decision": "PENDING",
                },
                doc! {
                    "release_decision": "REJECTED",
                    "release_reviewed_by": login,
                    "release_reviewed_at": now,
                },
                "quarantine_release_rejected",
                json!({"quarantine_id": record_id}),
            ),
            "approve-context" => (
                "dashboard_flow_context",
                doc! {"repository_id": repository, "context_id": record_id, "status": "PENDING"},
                doc! {"status": "APPROVED", "approved_by": login, "approved_at": now},
                "flow_context_approved",
                json!({"context_id": record_id}),
            ),
            "reject-context" => (
                "dashboard_flow_context",
                doc! {"repository_id": repository, "context_id": record_id, "status": "PENDING"},
                doc! {"status": "REJECTED", "rejected_by": login, "rejected_at": now},
                "flow_context_rejected",
                json!({"context_id": record_id}),
            ),
            _ => return Err("unsupported Dashboard Agent approval".into()),
        };
        let linked_record = self
            .database
            .collection::<Document>(collection)
            .find_one(filter.clone())
            .run()
            .map_err(io_error)?
            .ok_or("pending Dashboard Agent action not found")?;
        let incident_id = linked_record
            .get_str("incident_id")
            .ok()
            .map(String::from)
            .unwrap_or_default();
        let result = self
            .database
            .collection::<Document>(collection)
            .update_one(filter, doc! {"$set": fields})
            .run()
            .map_err(io_error)?;
        if result.matched_count == 0 {
            return Err("pending Dashboard Agent action not found".into());
        }
        let mut details = details;
        if let Some(object) = details.as_object_mut() {
            object.insert("incident_id".into(), json!(incident_id));
        }
        self.record_dashboard_agent_activity(repository, "", login, activity, &details)
    }

    /** Persist a monitor evaluation as both an agent metric and an auditable action. */
    pub(crate) fn record_dashboard_agent_evaluation(
        &self,
        repository: &str,
        actor: &str,
        trigger: &str,
        evaluation: DashboardAgentEvaluation<'_>,
    ) -> Result<(), String> {
        let result = self
            .database
            .collection::<Document>("dashboard_agents")
            .update_one(
                doc! {"repository_id": repository, "agent_id": evaluation.agent_id},
                doc! {
                    "$inc": {
                        "metrics.evaluations": 1_i64,
                        "metrics.matched_events": evaluation.observed as i64,
                        "metrics.threshold_hits": i64::from(evaluation.triggered),
                    },
                    "$set": {
                        "metrics.last_evaluated_at": evaluation.evaluated_at as i64,
                        "metrics.last_observed": evaluation.observed as i64,
                        "metrics.last_triggered": evaluation.triggered,
                    },
                },
            )
            .run()
            .map_err(io_error)?;
        if result.matched_count == 0 {
            return Err("dashboard agent not found during evaluation".into());
        }
        self.record_dashboard_agent_activity(
            repository,
            evaluation.agent_id,
            actor,
            "agent_evaluated",
            &json!({
                "observed": evaluation.observed,
                "threshold": evaluation.threshold,
                "triggered": evaluation.triggered,
                "signals": evaluation.signals,
                "evaluated_at": evaluation.evaluated_at,
                "trigger": trigger,
            }),
        )
    }

    /** Record a failed evaluation attempt when repository-scoped storage is still available. */
    pub(crate) fn record_dashboard_agent_evaluation_failure(
        &self,
        repository: &str,
        actor: &str,
        trigger: &str,
        error: &str,
    ) -> Result<(), String> {
        let error = error.chars().take(500).collect::<String>();
        self.record_dashboard_agent_activity(
            repository,
            "",
            actor,
            "evaluation_failed",
            &json!({"trigger": trigger, "error": error}),
        )
    }

    /** Claim a repository evaluation lease, ensuring only one service instance processes it. */
    pub(crate) fn acquire_dashboard_agent_lease(
        &self,
        repository: &str,
        owner: &str,
        now: u64,
    ) -> Result<bool, String> {
        let collection = self
            .database
            .collection::<Document>("dashboard_agent_leases");
        let until = now.saturating_add(90_000) as i64;
        let update = collection
            .update_one(
                doc! {"repository_id": repository, "lease_until": {"$lt": now as i64}},
                doc! {"$set": {"owner": owner, "lease_until": until}},
            )
            .run()
            .map_err(io_error)?;
        if update.matched_count > 0 {
            return Ok(true);
        }
        match collection
            .insert_one(doc! {
                "repository_id": repository,
                "owner": owner,
                "lease_until": until,
            })
            .run()
        {
            Ok(_) => Ok(true),
            Err(error)
                if error.to_string().contains("E11000")
                    || error
                        .to_string()
                        .to_ascii_lowercase()
                        .contains("duplicate key") =>
            {
                Ok(false)
            }
            Err(error) => Err(io_error(error)),
        }
    }

    /** Release a repository lease only when this worker owns it. */
    pub(crate) fn release_dashboard_agent_lease(
        &self,
        repository: &str,
        owner: &str,
    ) -> Result<(), String> {
        self.database
            .collection::<Document>("dashboard_agent_leases")
            .delete_one(doc! {"repository_id": repository, "owner": owner})
            .run()
            .map_err(io_error)?;
        Ok(())
    }

    fn record_dashboard_agent_activity(
        &self,
        repository: &str,
        agent_id: &str,
        actor: &str,
        action: &str,
        details: &Value,
    ) -> Result<(), String> {
        let now = now_millis();
        let details = bson::to_document(details).map_err(|error| error.to_string())?;
        self.database
            .collection::<Document>("dashboard_agent_activity")
            .insert_one(doc! {
                "activity_id": random_id()?,
                "repository_id": repository,
                "agent_id": agent_id,
                "action": action,
                "actor": actor,
                "at": now as i64,
                "details": details,
                "expires_at": bson::DateTime::from_millis(now as i64 + 30 * 86_400_000),
            })
            .run()
            .map_err(io_error)?;
        Ok(())
    }

    /** Find the pending request of an intent: unused, unexpired, and of the right kind
     * Input
        - intent: &Intent - what the person asked to approve
     * Output
        - Result<(&'static str, Document), String> collection and request
        - Error if the action is unknown or the code is unknown, used, or expired
    */
    pub(crate) fn pending(&self, intent: &Intent) -> Result<(&'static str, Document), String> {
        let collection = match intent.action.as_str() {
            "link" => "member_links",
            "owner" => "owner_keys",
            _ => return Err("unknown approval".into()),
        };
        self.database
            .collection::<Document>(collection)
            .find_one(doc! {
                "code_digest": digest(CODE_DOMAIN, &[intent.code.as_bytes()]),
                "expires_at": {"$gt": now_millis() as i64},
                "confirmed_by": {"$exists": false},
            })
            .run()
            .map_err(io_error)?
            .map(|document| (collection, document))
            .ok_or_else(|| {
                "the code is unknown, already used, or expired; run the command on your machine again".into()
            })
    }

    fn bounded_text(value: &Value, key: &str, limit: usize) -> Result<String, String> {
        let text = value[key].as_str().unwrap_or_default().trim();
        if text.is_empty() || text.len() > limit {
            return Err(format!("{key} must contain 1-{limit} characters"));
        }
        Ok(text.to_string())
    }

    /** Confirm a pending request when GitHub's answer allows it: a link needs read access to the
     * repository, an owner key needs admin access
     * Input
        - intent: &Intent - what the person asked to approve
        - login: &str - the person's GitHub login
        - access: Access - their access to the request's repository, from GitHub
     * Output
        - Result<String, String> what was confirmed, for the person
    */
    pub(crate) fn confirm(
        &self,
        intent: &Intent,
        login: &str,
        access: Access,
    ) -> Result<String, String> {
        let (collection, pending) = self.pending(intent)?;
        let remote = pending.get_str("remote").unwrap_or_default().to_string();
        let allowed = if collection == "owner_keys" {
            access.admin
        } else {
            access.read
        };
        if !allowed {
            return Err(format!(
                "GitHub does not show @{login} as {} of {remote}; nothing was confirmed",
                if collection == "owner_keys" {
                    "an admin"
                } else {
                    "having access"
                }
            ));
        }
        self.database
            .collection::<Document>(collection)
            .update_one(
                doc! {"_id": pending.get("_id").cloned().unwrap_or_default(), "confirmed_by": {"$exists": false}},
                doc! {"$set": {"confirmed_by": login, "confirmed_at": now_millis() as i64}},
            )
            .run()
            .map_err(io_error)?;
        let short = |key: &str| key.chars().take(16).collect::<String>();
        Ok(if collection == "owner_keys" {
            format!(
                "The owner key {}… of {remote} is confirmed by @{login}, an admin of the repository. Teammates joining with crane init see this confirmation.",
                short(pending.get_str("owner_key").unwrap_or_default())
            )
        } else {
            format!(
                "The machine {}… is linked to @{login} for {remote}; its activity is now shown as verified.",
                short(pending.get_str("installation").unwrap_or_default())
            )
        })
    }

    /** Record a credit grant an admin approved; the session's machine applies it to its own ledger
     * at its next sync and reports back
     * Input
        - grant: &Grant - what was granted, to which session and machine
        - login: &str - the admin's GitHub login
        - access: Access - their access to the repository, from GitHub
     * Output
        - Result<String, String> what was recorded, for the person
    */
    pub(crate) fn grant(
        &self,
        grant: &Grant,
        login: &str,
        access: Access,
    ) -> Result<String, String> {
        if !access.admin {
            return Err(format!("GitHub does not show @{login} as an admin of {}; only admins and owners can grant credits", grant.remote));
        }
        let now = now_millis();
        let grant_id = format!(
            "grt_{}",
            &digest(
                "dashboard/grant-id/v1",
                &[
                    grant.repository.as_bytes(),
                    grant.session.as_bytes(),
                    login.as_bytes(),
                    now.to_string().as_bytes()
                ]
            )[..24]
        );
        self.database
            .collection::<Document>("credit_grants")
            .insert_one(doc! {
                "grant_id": &grant_id,
                "repository_id": &grant.repository,
                "installation": &grant.installation,
                "session": &grant.session,
                "agent_id": &grant.agent,
                "amount": grant.amount as i64,
                "reason": &grant.reason,
                "approved_by": login,
                "approved_at": now as i64,
                "expires_at": (now + GRANT_LIFETIME_MILLIS) as i64,
                "status": "PENDING",
            })
            .run()
            .map_err(io_error)?;
        Ok(format!(
            "{} credits for session {} are approved by @{login} ({grant_id}). The agent's machine applies them at its next sync, within about a minute while the agent is active.",
            grant.amount, grant.session
        ))
    }
}

/** How long a resident decision waits for its machine before it lapses (one day: a decision on a
 * plan is only meaningful while that plan is current) */
const RESIDENT_DECISION_MILLIS: u64 = 24 * 60 * 60 * 1000;

/** A resident-agent decision asked for on the dashboard
 * Fields
    - repository: String - repository identity
    - remote: String - its verified remote
    - installation: String - the machine that holds the activation or assignment
    - action: String - approve, deny, answer, cancel, pause, or revoke
    - activation: String - activation (for step and activation decisions)
    - assignment: String - assignment (for pause and revoke)
    - step: String - step (approve, deny, answer)
    - plan: String - the plan digest the admin was shown
    - text: String - the answer (answer only)
    - reason: String - why
*/
pub(crate) struct ResidentDecision {
    pub(crate) repository: String,
    pub(crate) remote: String,
    pub(crate) installation: String,
    pub(crate) action: String,
    pub(crate) activation: String,
    pub(crate) assignment: String,
    pub(crate) step: String,
    pub(crate) plan: String,
    pub(crate) text: String,
    pub(crate) reason: String,
}

impl Approvals {
    /** Record a resident decision an admin made; the machine applies it at its next sync, with
     * the checks the CLI makes, and reports back
     * Input
        - decision: &ResidentDecision - what was decided
        - login: &str - the admin's GitHub login
        - access: Access - their access to the repository, from GitHub
     * Output
        - Result<String, String> what was recorded, for the person
    */
    pub(crate) fn resident_decision(
        &self,
        decision: &ResidentDecision,
        login: &str,
        access: Access,
    ) -> Result<String, String> {
        if !access.admin {
            return Err(format!("GitHub does not show @{login} as an admin of {}; only admins and owners decide for resident agents", decision.remote));
        }
        let now = now_millis();
        let decision_id = format!(
            "rsd_{}",
            &digest(
                "dashboard/resident-decision-id/v1",
                &[
                    decision.repository.as_bytes(),
                    decision.activation.as_bytes(),
                    decision.assignment.as_bytes(),
                    decision.step.as_bytes(),
                    decision.action.as_bytes(),
                    login.as_bytes(),
                    now.to_string().as_bytes()
                ]
            )[..24]
        );
        self.database
            .collection::<Document>("resident_decisions")
            .insert_one(doc! {
                "decision_id": &decision_id,
                "repository_id": &decision.repository,
                "installation": &decision.installation,
                "action": &decision.action,
                "activation": &decision.activation,
                "assignment": &decision.assignment,
                "step": &decision.step,
                "plan": &decision.plan,
                "text": &decision.text,
                "reason": &decision.reason,
                "decided_by": login,
                "decided_at": now as i64,
                "expires_at": (now + RESIDENT_DECISION_MILLIS) as i64,
                "status": "PENDING",
            })
            .run()
            .map_err(io_error)?;
        Ok(format!(
            "@{login} decided to {} ({decision_id}). The machine that holds it applies it at its next sync, with every check Crane makes there; the Resident agents page shows the result.",
            decision.action
        ))
    }
}

/** How many Brick requests one person may have waiting per repository */
const BRICK_OPEN_LIMIT: u64 = 50;

/** How long a Brick request waits for GitHub to confirm the person's role */
const BRICK_STAGE_MILLIS: u64 = 10 * 60 * 1000;

/** How long a confirmed Brick request waits for a machine before it lapses */
pub(crate) const BRICK_REQUEST_MILLIS: u64 = 14 * 24 * 60 * 60 * 1000;

impl Approvals {
    /** Keep a Brick change request until GitHub confirms the person's role at sign-in: checked
     * with the rules the machine checks again (brick/requests.rs), and unconfirmed it is only a
     * staged row that no machine ever reads, removed within the hour
     * Input
        - repository: &str - repository identity
        - login: &str - the signed-in person
        - op: &str - operation
        - params: &Value - its parameters
        - reason: &str - why
     * Output
        - Result<(String, &'static str), String> the request id and the role it needs
    */
    pub(crate) fn stage_brick_request(
        &self,
        repository: &str,
        login: &str,
        op: &str,
        params: &Value,
        reason: &str,
    ) -> Result<(String, &'static str), String> {
        let kept = crate::brick::requests::check(op, params, reason)?;
        let open = self
            .database
            .collection::<Document>("brick_requests")
            .count_documents(doc! {"repository_id": repository, "requested_by": login, "status": {"$in": ["STAGED", "PENDING"]}})
            .run()
            .map_err(io_error)?;
        if open >= BRICK_OPEN_LIMIT {
            return Err(format!("you have {open} Brick requests waiting for this repository; apply, dismiss, or withdraw some first"));
        }
        let role = crate::brick::requests::role(op)
            .ok_or_else(|| format!("'{op}' cannot be requested"))?
            .name();
        let now = now_millis();
        let request_id = format!("brq_{}{}", random_id()?, random_id()?);
        self.database
            .collection::<Document>("brick_requests")
            .insert_one(doc! {
                "request_id": &request_id,
                "repository_id": repository,
                "op": op,
                "params": serde_json::to_string(&kept).map_err(|error| error.to_string())?,
                "reason": crate::telemetry::redact::redact(reason.trim()).0,
                "role": role,
                "requested_by": login,
                "requested_at": now as i64,
                "status": "STAGED",
                "purge_at": bson::DateTime::from_millis((now + 60 * 60 * 1000) as i64),
            })
            .run()
            .map_err(io_error)?;
        Ok((request_id, role))
    }

    /** Confirm a staged Brick request with GitHub's answer for the repository: writers (push,
     * maintain, admin) may request drafts; signed changes need an admin. Confirmed, it waits for a
     * person on a machine to apply it; refused, it is kept as refused so the page can say why
     * Input
        - request_id: &str - the staged request
        - repository: &str - repository identity
        - remote: &str - its verified remote (for the message)
        - login: &str - who signed in (must be who staged it)
        - access: Access - their access to the repository, from GitHub
     * Output
        - Result<String, String> the message for the person
    */
    pub(crate) fn confirm_brick_request(
        &self,
        request_id: &str,
        repository: &str,
        remote: &str,
        login: &str,
        access: Access,
        owners: Option<&Ownership>,
    ) -> Result<String, String> {
        let now = now_millis();
        let collection = self.database.collection::<Document>("brick_requests");
        let staged = collection
            .find_one(doc! {"request_id": request_id, "repository_id": repository, "requested_by": login, "status": "STAGED",
                "requested_at": {"$gt": now.saturating_sub(BRICK_STAGE_MILLIS) as i64}})
            .run()
            .map_err(io_error)?
            .ok_or("that request was not found or waited too long for sign-in; make it again")?;
        let role = staged.get_str("role").unwrap_or("admin");
        let owned = owners.is_some_and(|owners| owners.owned);
        let allowed = match (role, owners) {
            // A code owner of the signed store may draft too
            ("writer", _) => access.write || owned,
            // CODEOWNERS cover the signed store: only its owners request signed changes
            (_, Some(_)) => owned,
            (_, None) => access.admin,
        };
        let kept = now + super::BRICK_REQUEST_KEPT_MILLIS;
        if !allowed {
            let detail = match (role, owners) {
                ("writer", _) => format!("GitHub does not show @{login} with write access to {remote}; drafts need push, maintain, or admin"),
                (_, Some(owners)) if owners.owners.is_empty() => format!("{} gives {} no owner (rule '{}'), so no one may request signed Brick changes from the dashboard; add an owner, or apply the change on a machine", owners.file, super::auth::BRICKS_PATH, owners.rule),
                (_, Some(owners)) => format!("@{login} does not own {} by {} (rule '{}': {}); signed Brick changes need one of its code owners", super::auth::BRICKS_PATH, owners.file, owners.rule, owners.owners.iter().map(|owner| format!("@{owner}")).collect::<Vec<_>>().join(", ")),
                (_, None) => format!("GitHub does not show @{login} as an admin of {remote}, and no CODEOWNERS line covers {}; signed Brick changes need an admin or owner", super::auth::BRICKS_PATH),
            };
            collection
                .update_one(
                    doc! {"request_id": request_id, "status": "STAGED"},
                    doc! {"$set": {"status": "REFUSED", "detail": &detail, "settled_at": now as i64, "purge_at": bson::DateTime::from_millis(kept as i64)}},
                )
                .run()
                .map_err(io_error)?;
            return Err(detail);
        }
        collection
            .update_one(
                doc! {"request_id": request_id, "status": "STAGED"},
                doc! {"$set": {"status": "PENDING", "confirmed_at": now as i64, "expires_at": (now + BRICK_REQUEST_MILLIS) as i64,
                    "purge_at": bson::DateTime::from_millis((kept + BRICK_REQUEST_MILLIS) as i64)}},
            )
            .run()
            .map_err(io_error)?;
        Ok(format!(
            "Request {request_id} is waiting for a machine: a person there applies it with one click in Landscape or 'crane brick inbox apply {request_id}', and Crane checks{} it there.",
            if role == "writer" { "" } else { " and signs" }
        ))
    }

    /** Withdraw a Brick request that is still staged or waiting (only its author may)
     * Input
        - repository: &str - repository identity
        - request_id: &str - the request
        - login: &str - the signed-in person
     * Output
        - Result<(), String>
    */
    pub(crate) fn cancel_brick_request(
        &self,
        repository: &str,
        request_id: &str,
        login: &str,
    ) -> Result<(), String> {
        let now = now_millis();
        let changed = self
            .database
            .collection::<Document>("brick_requests")
            .update_one(
                doc! {"request_id": request_id, "repository_id": repository, "requested_by": login, "status": {"$in": ["STAGED", "PENDING"]}},
                doc! {"$set": {"status": "CANCELLED", "settled_at": now as i64, "detail": format!("withdrawn by @{login}"),
                    "purge_at": bson::DateTime::from_millis((now + super::BRICK_REQUEST_KEPT_MILLIS) as i64)}},
            )
            .run()
            .map_err(io_error)?;
        if changed.modified_count == 0 {
            return Err("only a waiting request you made can be withdrawn".into());
        }
        Ok(())
    }
}

/** How long a grant waits for its machine before it lapses */
const GRANT_LIFETIME_MILLIS: u64 = 7 * 24 * 60 * 60 * 1000;

/** A credit grant asked for on the dashboard
 * Fields
    - repository: String - repository identity
    - remote: String - its verified remote
    - session: String - the agent session to credit
    - installation: String - the machine the session runs on
    - agent: String - the agent identity
    - amount: u64 - credits, 1 to 1000
    - reason: String - why, shown in the ledger
*/
pub(crate) struct Grant {
    pub(crate) repository: String,
    pub(crate) remote: String,
    pub(crate) session: String,
    pub(crate) installation: String,
    pub(crate) agent: String,
    pub(crate) amount: u64,
    pub(crate) reason: String,
}
