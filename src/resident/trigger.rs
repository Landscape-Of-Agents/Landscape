// Triggers: the canonical envelope every event that may wake a resident is normalized into, the
// validation that rejects malformed, stale, oversized, foreign, or secret-carrying input, the
// subscription match, and the collectors of the real sources (the local event store, the signed
// generation, Brick monitor results, and timers). Trigger content is data: it is never read as an
// instruction, and only its typed fields reach a plan.
//
// Identity: a trigger's id derives from its source and idempotency key, so the same delivery seen
// twice has the same id and deduplication (store) drops the repeat.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::model::{check_kind, Assignment, Subscription, SubscriptionScope, TriggerSource};
use crate::brick::model::Target;
use crate::capture::stable_id;
use crate::telemetry::events::Event;

/** Most attributes of one trigger */
const MAX_ATTRIBUTES: usize = 32;

/** Longest attribute value, in characters */
const MAX_VALUE: usize = 256;

/** How old a trigger may be when it arrives (30 days), in milliseconds */
const MAX_AGE_MS: u64 = 30 * super::model::DAY_MS;

/** How far in the future a trigger may claim to be (clock skew, 5 minutes), in milliseconds */
const MAX_SKEW_MS: u64 = 300_000;

/** A normalized trigger
 * Fields
    - schema_version: u32 - SCHEMA_VERSION
    - trigger_id: String - "trg_..." from source and idempotency key
    - source: TriggerSource - source
    - kind: String - dot-separated kind, such as ci.failed or crane.tool.denied
    - occurred_at: u64 - when it happened (Unix milliseconds)
    - received_at: u64 - when Crane received it
    - repository_id: String - repository it belongs to
    - targets: Vec<String> - Flow, Zone, or Runbook keys it concerns (empty: repository-wide)
    - attributes: BTreeMap<String, String> - typed, bounded, redacted attributes
    - idempotency_key: String - delivery key
    - origin: String - where it was read ("events.jsonl#42", "cli:NAME", "timer:ID")
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Trigger {
    pub(crate) schema_version: u32,
    pub(crate) trigger_id: String,
    pub(crate) source: TriggerSource,
    pub(crate) kind: String,
    pub(crate) occurred_at: u64,
    pub(crate) received_at: u64,
    pub(crate) repository_id: String,
    pub(crate) targets: Vec<String>,
    pub(crate) attributes: BTreeMap<String, String>,
    pub(crate) idempotency_key: String,
    pub(crate) origin: String,
}

/** The fields a delivered event file carries ('crane resident trigger --file')
 * Fields
    - source: TriggerSource - ci or webhook (the other sources are read by Crane itself)
    - kind: String - kind
    - occurred_at: Option<u64> - when it happened (default: now)
    - targets: Vec<String> - target keys
    - attributes: BTreeMap<String, String> - attributes
    - idempotency_key: String - delivery key (a redelivery uses the same key)
*/
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Delivered {
    pub(crate) source: TriggerSource,
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) occurred_at: Option<u64>,
    #[serde(default)]
    pub(crate) targets: Vec<String>,
    #[serde(default)]
    pub(crate) attributes: BTreeMap<String, String>,
    pub(crate) idempotency_key: String,
}

/** Normalize and validate a trigger: kind syntax, target keys, bounded attributes without control
 * characters or secrets, an identifier-like idempotency key, the repository, and its age
 * Input
    - source: TriggerSource - source
    - kind: &str - kind
    - occurred_at: u64 - when it happened
    - targets: Vec<String> - target keys
    - attributes: BTreeMap<String, String> - attributes
    - idempotency_key: &str - delivery key
    - origin: &str - where it was read
    - repository_id: &str - the repository it claims
    - expected_repository: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - Result<Trigger, String>
    - Error naming the first problem
*/
#[allow(clippy::too_many_arguments)]
pub(crate) fn normalize(
    source: TriggerSource,
    kind: &str,
    occurred_at: u64,
    targets: Vec<String>,
    attributes: BTreeMap<String, String>,
    idempotency_key: &str,
    origin: &str,
    repository_id: &str,
    expected_repository: &str,
    now: u64,
) -> Result<Trigger, String> {
    if kind.ends_with(".*") {
        return Err(format!(
            "'{kind}' is a pattern; a trigger has one exact kind"
        ));
    }
    check_kind(kind)?;
    if source.delivery().is_none() {
        return Err(format!(
            "the {} source has no connector; its triggers are not accepted",
            source.name()
        ));
    }
    if repository_id != expected_repository {
        return Err(format!(
            "the trigger belongs to repository {repository_id}, not this one ({expected_repository})"
        ));
    }
    let key_valid = !idempotency_key.is_empty()
        && idempotency_key.len() <= 128
        && idempotency_key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._:/@#-".contains(c));
    if !key_valid {
        return Err("the idempotency key must be 1-128 letters, digits, or . _ : / @ # -".into());
    }
    if occurred_at + MAX_AGE_MS < now {
        return Err("the trigger is more than 30 days old (stale)".into());
    }
    if occurred_at > now + MAX_SKEW_MS {
        return Err("the trigger claims a time in the future".into());
    }
    let mut keys = Vec::new();
    for target in &targets {
        keys.push(Target::parse(target)?.key());
    }
    keys.sort();
    keys.dedup();
    if attributes.len() > MAX_ATTRIBUTES {
        return Err(format!(
            "a trigger carries at most {MAX_ATTRIBUTES} attributes"
        ));
    }
    for (name, value) in &attributes {
        crate::platform::validate_name("attribute", &name.replace('.', "_"))?;
        if value.chars().count() > MAX_VALUE || value.chars().any(char::is_control) {
            return Err(format!(
                "attribute '{name}' is longer than {MAX_VALUE} characters or has control characters"
            ));
        }
        if crate::telemetry::redact::redact(value).1 > 0 {
            return Err(format!(
                "attribute '{name}' holds a secret-looking value; triggers never carry secrets"
            ));
        }
    }
    Ok(Trigger {
        schema_version: super::SCHEMA_VERSION,
        trigger_id: stable_id("trg", &[&source.name(), idempotency_key]),
        source,
        kind: kind.into(),
        occurred_at,
        received_at: now,
        repository_id: repository_id.into(),
        targets: keys,
        attributes,
        idempotency_key: idempotency_key.into(),
        origin: origin.into(),
    })
}

/** Normalize a delivered event file (only the ci and webhook sources are delivered this way)
 * Input
    - value: &Value - file content
    - actor: &str - the person or CI step delivering it
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - Result<Trigger, String>
*/
pub(crate) fn delivered(
    value: &Value,
    actor: &str,
    repository_id: &str,
    now: u64,
) -> Result<Trigger, String> {
    let file: Delivered = serde_json::from_value(value.clone())
        .map_err(|error| format!("the event file is not a trigger: {error}"))?;
    if !matches!(file.source, TriggerSource::Ci | TriggerSource::Webhook) {
        return Err(format!(
            "the {} source is read by Crane itself; deliver only ci or webhook triggers",
            file.source.name()
        ));
    }
    normalize(
        file.source,
        &file.kind,
        file.occurred_at.unwrap_or(now),
        file.targets,
        file.attributes,
        &file.idempotency_key,
        &format!("cli:{actor}"),
        repository_id,
        repository_id,
        now,
    )
}

/** Normalize a test fixture: the same file format as a delivery, but any source with a connector
 * (a test may simulate a Crane event, a monitor result, or a timer); fixtures never reach the
 * deduplication set
 * Input
    - value: &Value - file content
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - Result<Trigger, String>
*/
pub(crate) fn fixture(value: &Value, repository_id: &str, now: u64) -> Result<Trigger, String> {
    let file: Delivered = serde_json::from_value(value.clone())
        .map_err(|error| format!("the event file is not a trigger: {error}"))?;
    normalize(
        file.source,
        &file.kind,
        file.occurred_at.unwrap_or(now),
        file.targets,
        file.attributes,
        &file.idempotency_key,
        "test fixture",
        repository_id,
        repository_id,
        now,
    )
}

/** Check a kind against a subscription pattern ("ci.failed", or "ci.*" for every ci kind)
 * Input
    - pattern: &str - pattern
    - kind: &str - kind
 * Output
    - bool
*/
pub(crate) fn kind_matches(pattern: &str, kind: &str) -> bool {
    match pattern.strip_suffix(".*") {
        Some(prefix) => kind.starts_with(&format!("{prefix}.")),
        None => pattern == kind,
    }
}

/** Decide whether a subscription takes a trigger for an assignment's target
 * Input
    - subscription: &Subscription - subscription
    - trigger: &Trigger - trigger
    - target: &str - the assignment's target key
 * Output
    - Result<(), String> Ok when it matches, else why not
*/
pub(crate) fn subscription_matches(
    subscription: &Subscription,
    trigger: &Trigger,
    target: &str,
) -> Result<(), String> {
    if subscription.source != trigger.source {
        return Err("source differs".into());
    }
    if !subscription
        .kinds
        .iter()
        .any(|pattern| kind_matches(pattern, &trigger.kind))
    {
        return Err(format!("kind {} is not subscribed", trigger.kind));
    }
    if let Some((name, _)) = subscription
        .attributes
        .iter()
        .find(|(name, value)| trigger.attributes.get(*name) != Some(value))
    {
        return Err(format!("attribute {name} does not match"));
    }
    let concerns_target = trigger.targets.iter().any(|key| key == target);
    match (
        concerns_target,
        trigger.targets.is_empty(),
        subscription.scope,
    ) {
        (true, _, _) | (false, true, SubscriptionScope::Repository) => Ok(()),
        (false, true, SubscriptionScope::Target) => {
            Err("repository-wide trigger; the subscription takes target triggers only".into())
        }
        (false, false, _) => Err(format!(
            "the trigger concerns {}, not {target}",
            trigger.targets.join(", ")
        )),
    }
}

/** Decide whether any subscription of an assignment takes a trigger
 * Input
    - assignment: &Assignment - assignment
    - trigger: &Trigger - trigger
 * Output
    - Result<usize, String> the matching subscription's index, else the closest reason
*/
pub(crate) fn assignment_matches(
    assignment: &Assignment,
    trigger: &Trigger,
) -> Result<usize, String> {
    // A timer wakes only the assignment it belongs to
    if trigger.source == TriggerSource::Timer {
        return if trigger.attributes.get("assignment") == Some(&assignment.assignment_id) {
            Ok(usize::MAX)
        } else {
            Err("the timer belongs to another assignment".into())
        };
    }
    let target = assignment.target.key();
    let mut reason = "no subscription".to_string();
    for (index, subscription) in assignment.subscriptions.iter().enumerate() {
        match subscription_matches(subscription, trigger, &target) {
            Ok(()) => return Ok(index),
            Err(why) if subscription.source == trigger.source => reason = why,
            Err(_) => {}
        }
    }
    Err(reason)
}

/** Report whether a stored event was caused by the resident runtime itself (its tick events and
 * the commands that carry resident notes), so a resident can never wake itself in a loop
 * Input
    - event: &Event - event
 * Output
    - bool
*/
pub(crate) fn self_caused(event: &Event) -> bool {
    event.event_type.starts_with("resident.")
        || event.payload["notes"].as_array().is_some_and(|notes| {
            notes.iter().any(|note| {
                note["kind"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("RESIDENT_"))
            })
        })
}

/** The kind a stored event becomes: "crane." and its type in dot-separated lowercase
 * Input
    - event_type: &str - event type ("hook.pre_tool_use", "command.executed")
 * Output
    - String
*/
fn event_kind(event_type: &str) -> String {
    let words = event_type
        .to_ascii_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    let words = words
        .split('.')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join(".");
    format!("crane.{words}")
}

/** Triggers from stored events after a sequence number (self-caused events are skipped)
 * Input
    - events: &[Event] - the event store
    - after: u64 - last sequence already read
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - (Vec<Trigger>, u64) triggers and the new cursor
*/
pub(crate) fn from_events(
    events: &[Event],
    after: u64,
    repository_id: &str,
    now: u64,
) -> (Vec<Trigger>, u64) {
    let mut cursor = after;
    let mut triggers = Vec::new();
    for event in events.iter().filter(|event| event.sequence > after) {
        cursor = cursor.max(event.sequence);
        if self_caused(event) {
            continue;
        }
        let mut attributes = BTreeMap::new();
        attributes.insert(
            "severity".to_string(),
            crate::resident::word(&event.severity),
        );
        if let Some(decision) = &event.decision {
            attributes.insert("decision".into(), crate::resident::word(decision));
        }
        attributes.insert("domain".into(), crate::resident::word(&event.domain));
        let targets = event
            .bindings
            .zones
            .iter()
            .filter(|key| Target::parse(key).is_ok())
            .cloned()
            .collect::<Vec<_>>();
        if let Ok(trigger) = normalize(
            TriggerSource::CraneEvent,
            &event_kind(&event.event_type),
            event.occurred_at.min(now),
            targets,
            attributes,
            &format!("evt:{}", event.digest.chars().take(32).collect::<String>()),
            &format!("events.jsonl#{}", event.sequence),
            &event.repository_id,
            repository_id,
            now,
        ) {
            triggers.push(trigger);
        }
    }
    (triggers, cursor)
}

/** The trigger of a signed generation change, when the generation moved since the cursor
 * Input
    - generation: u64 - current signed generation
    - cursor: u64 - generation last seen (0: never, which records it without a trigger)
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - Option<Trigger>
*/
pub(crate) fn from_generation(
    generation: u64,
    cursor: u64,
    repository_id: &str,
    now: u64,
) -> Option<Trigger> {
    if cursor == 0 || generation == cursor {
        return None;
    }
    let attributes = BTreeMap::from([
        ("generation".to_string(), generation.to_string()),
        ("previous".to_string(), cursor.to_string()),
    ]);
    normalize(
        TriggerSource::GovernanceChange,
        "governance.signed",
        now,
        Vec::new(),
        attributes,
        &format!("gen:{generation}"),
        "signed state",
        repository_id,
        repository_id,
        now,
    )
    .ok()
}

/** Triggers from Brick monitor results that need attention and ran after the cursor
 * Input
    - monitors: &[Value] - brick::runtime::monitor_status rows
    - after: u64 - last monitor run already read (Unix milliseconds)
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - (Vec<Trigger>, u64) triggers and the new cursor
*/
pub(crate) fn from_monitors(
    monitors: &[Value],
    after: u64,
    repository_id: &str,
    now: u64,
) -> (Vec<Trigger>, u64) {
    let mut cursor = after;
    let mut triggers = Vec::new();
    for monitor in monitors {
        let Some(last_run) = monitor["last_run"].as_u64() else {
            continue;
        };
        cursor = cursor.max(last_run);
        let result = monitor["result"].as_str().unwrap_or("NOT_RUN");
        if last_run <= after || matches!(result, "OK" | "NOT_RUN") {
            continue;
        }
        let text = |key: &str| monitor[key].as_str().unwrap_or_default().to_string();
        let attributes = BTreeMap::from([
            ("brick".to_string(), text("brick")),
            ("monitor".to_string(), text("monitor")),
            ("signal".to_string(), text("source")),
            ("result".to_string(), result.to_string()),
        ]);
        if let Ok(trigger) = normalize(
            TriggerSource::BrickMonitor,
            &format!("monitor.{}", result.to_ascii_lowercase()),
            last_run.min(now),
            vec![text("target")],
            attributes,
            &format!(
                "mon:{}:{}:{}:{last_run}",
                text("target"),
                text("brick"),
                text("source")
            ),
            "brick monitors",
            repository_id,
            repository_id,
            now,
        ) {
            triggers.push(trigger);
        }
    }
    (triggers, cursor)
}

/** Triggers of the timers that fell due for active assignments
 * Input
    - assignments: &[&Assignment] - active assignments
    - fired: &BTreeMap<String, u64> - "assignment:timer" to the last slot fired
    - repository_id: &str - this repository
    - now: u64 - Unix milliseconds
 * Output
    - Vec<(String, u64, Trigger)> timer key, slot, trigger
*/
pub(crate) fn from_timers(
    assignments: &[&Assignment],
    fired: &BTreeMap<String, u64>,
    repository_id: &str,
    now: u64,
) -> Vec<(String, u64, Trigger)> {
    let mut due = Vec::new();
    for assignment in assignments {
        for timer in &assignment.timers {
            let period = u64::from(timer.every_minutes.max(5)) * 60_000;
            let slot = now / period;
            let key = format!("{}:{}", assignment.assignment_id, timer.timer_id);
            if fired.get(&key).is_some_and(|last| *last >= slot) {
                continue;
            }
            // The first evaluation only records the slot: a new timer waits one period
            if !fired.contains_key(&key) && assignment.starts_at + period > now {
                continue;
            }
            let attributes = BTreeMap::from([
                ("assignment".to_string(), assignment.assignment_id.clone()),
                ("timer".to_string(), timer.timer_id.clone()),
            ]);
            if let Ok(trigger) = normalize(
                TriggerSource::Timer,
                &format!(
                    "timer.{}",
                    timer.timer_id.to_ascii_lowercase().replace('_', "-")
                ),
                slot * period,
                vec![assignment.target.key()],
                attributes,
                &format!("tmr:{key}:{slot}"),
                &format!("timer:{key}"),
                repository_id,
                repository_id,
                now,
            ) {
                due.push((key, slot, trigger));
            }
        }
    }
    due
}
