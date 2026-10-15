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

use super::auth::{Access, Intent};
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

pub(crate) struct DashboardAgentEvaluation<'a> {
    pub(crate) agent_id: &'a str,
    pub(crate) observed: u64,
    pub(crate) threshold: u64,
    pub(crate) triggered: bool,
    pub(crate) evaluated_at: u64,
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
            "metric": metric,
            "threshold": threshold as i64,
            "window_minutes": window_minutes as i64,
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
                "assignments": input["assignments"],
            }),
        )?;
        Ok(json!({"agent_id": agent_id, "name": name, "state": state, "updated_at": now}))
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
