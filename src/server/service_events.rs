// Events the service itself records (DASHBOARD.md): dashboard sign-in and use (F00, F30),
// approvals (F02.5, F02.7), uploads as the service sees them (F02.9, F09), changes between a
// machine's status reports (F12, F18, F20, F28), and its own operation (F29). They go to the
// append-only service_events collection through this log, which can only insert there; routine ones
// (INFO, LOW, MEDIUM) expire after the retention period like routine machine events. Recording
// never fails a request: errors are dropped.

use mongodb::bson::{self, Document};
use mongodb::sync::Database;

use crate::platform::now_millis;
use crate::telemetry::spec::{envelope_detached, Spec};
use crate::trust::crypto::{hex, random_bytes};

/** The service's event log
 * Fields
    - database: Database - MongoDB (private: only `record` writes, and only to service_events)
    - retention_days: i64 - how long routine events are kept
*/
pub(crate) struct ServiceLog {
    database: Database,
    retention_days: i64,
}

impl ServiceLog {
    /** Wrap a database
     * Input
        - database: Database - MongoDB
        - retention_days: i64 - retention of routine events
     * Output
        - ServiceLog
    */
    pub(crate) fn new(database: Database, retention_days: i64) -> Self {
        Self {
            database,
            retention_days,
        }
    }

    /** Record an event
     * Input
        - spec: Spec - the event (its groups already filled)
        - repository: Option<&str> - repository it concerns, if any
        - installation: Option<&str> - machine it concerns, if any
     * Output
        - None
    */
    pub(crate) fn record(
        &self,
        mut spec: Spec,
        repository: Option<&str>,
        installation: Option<&str>,
    ) {
        if spec.get("action", "action_source") == "CLI" {
            spec.set("action", "action_source", "SERVICE");
        }
        if let Some(repository) = repository {
            spec.set("repository", "repo_id", repository);
        }
        let routine = matches!(
            spec.get("severity", "severity_status").as_str(),
            Some("INFO" | "LOW" | "MEDIUM")
        );
        let now = now_millis();
        let id = format!(
            "srv_{}",
            random_bytes(12)
                .map(|bytes| hex(&bytes))
                .unwrap_or_default()
        );
        let value = envelope_detached(spec, &id, now, "SERVICE");
        let Ok(mut document) = bson::to_document(&value) else {
            return;
        };
        document.insert("repository_id", repository.map(String::from));
        document.insert("installation", installation.map(String::from));
        if routine {
            let expiry = now as i64 + self.retention_days * 86_400_000;
            document.insert("expires_at", bson::DateTime::from_millis(expiry));
        }
        let _ = self
            .database
            .collection::<Document>("service_events")
            .insert_one(document)
            .run();
    }
}

/** Start a service-side event with its common attributes
 * Input
    - name: &str - event name
    - flow: &str - flow
    - action_type: &str - action type
    - source: &str - DASHBOARD, LINK, or SERVICE
 * Output
    - Spec
*/
pub(crate) fn event(name: &str, flow: &str, action_type: &str, source: &str) -> Spec {
    let mut spec = Spec::new(name, flow);
    spec.set("action", "action_type", action_type)
        .set("action", "action_source", source)
        .set("bindings", "third_party", ["MONGODB"])
        .set("bindings", "binding_result", "OK");
    spec
}

/** A dashboard event for a signed-in (or not yet signed-in) person
 * Input
    - name: &str - event name
    - flow: &str - flow
    - action_type: &str - action type
    - viewer: Option<&str> - GitHub login
    - repository: Option<(&str, &str)> - (identity, remote) of the chosen repository
 * Output
    - Spec
*/
pub(crate) fn dashboard(
    name: &str,
    flow: &str,
    action_type: &str,
    viewer: Option<&str>,
    repository: Option<(&str, &str)>,
) -> Spec {
    let mut spec = event(name, flow, action_type, "DASHBOARD");
    spec.set("action", "action_user", viewer)
        .set("session", "session_type", "DASHBOARD_SESSION")
        .set("bindings", "third_party", ["GITHUB", "MONGODB"]);
    if let Some((id, remote)) = repository {
        spec.set("repository", "repo_id", id)
            .set("repository", "repo_remote", remote)
            .set("repository", "repo_remote_verified", true);
    }
    spec
}
