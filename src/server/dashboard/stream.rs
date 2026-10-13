// The dashboard's event stream (DASHBOARD.md): every specification event of a repository and its
// selected members, in time order, with the member each came from. It joins the three producers:
// machine events (stored with their specification events, or projected now for older records),
// service events (service_events), and events derived here from other records (the credit ledger,
// alerts, the governance audit trail, stale and revived sessions, inferred permission outcomes, and
// ledgers that do not reconcile).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::foxx::views::Snapshot;
use crate::telemetry::spec;

/** Minutes without hook activity after which a session counts as stale */
fn stale_minutes() -> u64 {
    std::env::var("CRANE_STALE_SESSION_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(120)
        .max(1)
}

/** What the stream is built from
 * Fields
    - repository: &'a str - repository identity
    - remote: &'a str - verified remote
    - snapshot: &'a Snapshot - the members' records
    - event_member: &'a BTreeMap<String, String> - event to member
    - session_member: &'a BTreeMap<String, String> - session to member
    - attested: &'a BTreeSet<String> - sessions with a signed end
    - audit: &'a [Value] - governance audit entries, with their installation
    - service: &'a [Value] - service events
    - chains: &'a [Value] - per member chain and ledger verification
*/
pub(crate) struct Sources<'a> {
    pub(crate) repository: &'a str,
    pub(crate) remote: &'a str,
    pub(crate) snapshot: &'a Snapshot,
    pub(crate) event_member: &'a BTreeMap<String, String>,
    pub(crate) session_member: &'a BTreeMap<String, String>,
    pub(crate) attested: &'a BTreeSet<String>,
    pub(crate) audit: &'a [Value],
    pub(crate) service: &'a [Value],
    pub(crate) chains: &'a [Value],
}

/** Mark an event with its member and the repository's remote
 * Input
    - value: &mut Value - event with envelope
    - member: Option<&String> - member key
    - remote: &str - verified remote
 * Output
    - None
*/
fn finish(value: &mut Value, member: Option<&String>, remote: &str) {
    value["member_key"] = json!(member);
    if value["repository"].is_object() {
        value["repository"]["repo_remote"] = json!(remote);
        value["repository"]["repo_remote_verified"] = json!(true);
    }
}

/** Build the stream
 * Input
    - sources: &Sources - records
    - now: u64 - Unix milliseconds (for staleness)
 * Output
    - Vec<Value> every specification event, oldest first
*/
pub(crate) fn build(sources: &Sources, now: u64) -> Vec<Value> {
    let mut all = Vec::new();
    for event in &sources.snapshot.events {
        let member = sources.event_member.get(&event.event_id);
        let specs = if event.spec.is_empty() {
            spec::project(event)
        } else {
            event.spec.clone()
        };
        for (index, value) in specs.iter().enumerate() {
            let mut value = spec::envelope(event, value, index);
            finish(&mut value, member, sources.remote);
            all.push(value);
        }
    }
    for entry in &sources.snapshot.ledger {
        if let Some(mut value) = spec::from_ledger(entry, sources.repository) {
            finish(
                &mut value,
                sources.session_member.get(&entry.session),
                sources.remote,
            );
            all.push(value);
        }
    }
    for alert in &sources.snapshot.alerts {
        let member = alert["installation"].as_str().map(String::from);
        for mut value in spec::from_alert(alert, sources.repository) {
            finish(&mut value, member.as_ref(), sources.remote);
            all.push(value);
        }
    }
    for entry in sources.audit {
        let member = entry["installation"].as_str().map(String::from);
        let mut value = spec::from_audit(entry, sources.repository);
        finish(&mut value, member.as_ref(), sources.remote);
        all.push(value);
    }
    for event in sources.service {
        let mut value = event.clone();
        let member = event["installation"].as_str().map(String::from);
        if let Some(object) = value.as_object_mut() {
            object.remove("installation");
            object.remove("repository_id");
            object.remove("expires_at");
        }
        finish(&mut value, member.as_ref(), sources.remote);
        all.push(value);
    }
    // Stale and revived sessions, from the gaps in each session's activity
    let limit = stale_minutes() * 60_000;
    for session in &sources.snapshot.sessions {
        let record = serde_json::to_value(session).unwrap_or_default();
        let member = sources.session_member.get(&session.foxx_session_id);
        let mut times = all
            .iter()
            .filter(|value| {
                value["session"]["session_id"].as_str() == Some(session.foxx_session_id.as_str())
                    && value["origin"] == "MACHINE"
            })
            .filter_map(|value| value["occurred_at"].as_u64())
            .collect::<Vec<_>>();
        times.sort_unstable();
        for pair in times.windows(2) {
            if pair[1].saturating_sub(pair[0]) > limit {
                let mut value = spec::staleness(
                    &record,
                    sources.repository,
                    true,
                    pair[1],
                    (pair[1] - pair[0]) / 60_000,
                );
                finish(&mut value, member, sources.remote);
                all.push(value);
            }
        }
        if session.ended_at.is_none()
            && !sources.attested.contains(&session.foxx_session_id)
            && now.saturating_sub(session.last_seen_at) > limit
        {
            let mut value = spec::staleness(
                &record,
                sources.repository,
                false,
                session.last_seen_at + limit,
                now.saturating_sub(session.last_seen_at) / 60_000,
            );
            finish(&mut value, member, sources.remote);
            all.push(value);
        }
    }
    // What the person chose in a permission dialog, from what happened next
    let reported = all
        .iter()
        .filter(|value| value["flow"] == "F06")
        .filter_map(|value| {
            value["action"]["action_id"]
                .as_str()
                .map(|id| (id.to_string(), value["occurred_at"].as_u64().unwrap_or(0)))
        })
        .collect::<BTreeMap<_, _>>();
    let closed = all
        .iter()
        .filter(|value| value["event_name"] == "SESSION_CLOSED")
        .filter_map(|value| {
            value["session"]["session_id"]
                .as_str()
                .map(|id| (id.to_string(), value["occurred_at"].as_u64().unwrap_or(0)))
        })
        .collect::<BTreeMap<_, _>>();
    let outcomes = all
        .iter()
        .filter(|value| value["event_name"] == "PERMISSION_LEFT_TO_PERSON")
        .filter_map(|value| {
            let call = value["action"]["action_id"].as_str().unwrap_or_default();
            let session = value["session"]["session_id"].as_str().unwrap_or_default();
            let (ran, at) = match (reported.get(call), closed.get(session)) {
                (Some(at), _) => (Some(true), *at),
                (None, Some(at)) => (Some(false), *at),
                _ => return None,
            };
            let mut outcome = spec::permission_outcome(value, ran, at);
            outcome["member_key"] = value["member_key"].clone();
            Some(outcome)
        })
        .collect::<Vec<_>>();
    all.extend(outcomes);
    for chain in sources.chains {
        if chain["ledger_reconciles"] == false {
            let member = chain["member"].as_str().unwrap_or_default().to_string();
            let entries = sources.snapshot.ledger.len();
            let mut value = spec::ledger_failed(sources.repository, &member, entries, now);
            finish(&mut value, Some(&member), sources.remote);
            all.push(value);
        }
    }
    all.sort_by_key(|value| value["occurred_at"].as_u64().unwrap_or(0));
    all
}
