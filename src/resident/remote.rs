// Decisions made on the DataSea dashboard, applied on the machine that holds the activation. A
// repository admin (GitHub decides, at the moment of the decision) approves, denies, or answers a
// waiting step, cancels an activation, or pauses or revokes an assignment; the service keeps the
// decision for this machine, and the next sync fetches it and applies it here through the same
// functions the CLI uses, with every check they make. The dashboard can always make things
// stricter (pause, revoke, cancel). Approvals and answers count only when the definition allows
// dashboard decisions (remote_decisions) and only for the plan digest the admin was shown; a
// decision about a plan that has changed since is refused. Resuming or activating stays a signer's
// action on a machine. Each decision is applied at most once (store.remote).

use serde_json::{json, Value};

use super::model::Lifecycle;
use super::store::ResidentStore;
use super::worker::{self, Env, Via};
use crate::brick::model::Author;
use crate::platform::notes::note;

/** How long applied decision ids are remembered (30 days), in milliseconds */
const REMEMBER_MS: u64 = 30 * super::model::DAY_MS;

/** Most decisions applied in one sync */
const MAX_DECISIONS: usize = 50;

/** Read a bounded identifier field of a decision
 * Input
    - decision: &Value - decision
    - key: &str - field
 * Output
    - String (empty when absent or not an identifier)
*/
fn field(decision: &Value, key: &str) -> String {
    decision[key]
        .as_str()
        .filter(|value| {
            value.len() <= 128
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_-.:@".contains(c))
        })
        .unwrap_or_default()
        .to_string()
}

/** Apply one decision
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - decision: &Value - {action, activation, assignment, step, plan, text, reason}
    - decider: &Author - the admin, as recorded
 * Output
    - Result<String, String> - the resulting state, or why it was refused
*/
fn apply_one(
    store: &mut ResidentStore,
    env: &mut Env,
    decision: &Value,
    decider: &Author,
) -> Result<String, String> {
    let action = field(decision, "action");
    let activation = field(decision, "activation");
    let assignment = field(decision, "assignment");
    let step = field(decision, "step");
    let plan = field(decision, "plan");
    let reason = decision["reason"]
        .as_str()
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect::<String>();
    let reason = format!(
        "on the dashboard: {}",
        if reason.trim().is_empty() {
            "no reason given"
        } else {
            reason.trim()
        }
    );
    let state = match action.as_str() {
        "approve" | "deny" => worker::approve(store, env, &activation, &step, decider, Via::Dashboard { plan: &plan }, action == "approve", &reason)?,
        "answer" => {
            let text = decision["text"].as_str().unwrap_or_default().chars().take(2000).collect::<String>();
            worker::answer(store, env, &activation, &step, &text, decider, Via::Dashboard { plan: &plan })?
        }
        "cancel" => worker::cancel(store, &activation, decider, &reason, env.now)?,
        "pause" | "revoke" => {
            let authority = (env.authority)()?;
            let to = if action == "pause" { Lifecycle::Paused } else { Lifecycle::Revoked };
            let changed = super::registry::change_assignment(store, &mut env.cache, &authority, &assignment, to, decider, &reason, env.now)?;
            store.save()?;
            return Ok(crate::resident::word(&changed.lifecycle));
        }
        other => return Err(format!("'{other}' cannot be decided on the dashboard; resuming and activating need a signer on a machine")),
    };
    Ok(crate::resident::word(&state))
}

/** Apply the dashboard decisions the service holds for this machine, each at most once
 * Input
    - store: &mut ResidentStore - store
    - env: &mut Env - environment
    - decisions: &[Value] - decisions from the service
 * Output
    - Vec<Value> - results {decision_id, status APPLIED or REJECTED, detail, state}
*/
pub(crate) fn apply(store: &mut ResidentStore, env: &mut Env, decisions: &[Value]) -> Vec<Value> {
    let now = env.now;
    store.state.remote.retain(|_, at| *at + REMEMBER_MS > now);
    let mut results = Vec::new();
    for decision in decisions.iter().take(MAX_DECISIONS) {
        let id = field(decision, "decision_id");
        if id.is_empty() {
            continue;
        }
        if store.state.remote.contains_key(&id) {
            results.push(json!({"decision_id": id, "status": "APPLIED", "detail": "already applied on this machine"}));
            continue;
        }
        let login = field(decision, "decided_by");
        let decider = Author {
            name: format!("@{login} (dashboard, GitHub admin)"),
            key: None,
        };
        let outcome = apply_one(store, env, decision, &decider);
        store.state.remote.insert(id.clone(), now);
        let (status, detail, state) = match &outcome {
            Ok(state) => (
                "APPLIED",
                format!("applied; now {state}"),
                Some(state.clone()),
            ),
            Err(error) => ("REJECTED", error.clone(), None),
        };
        note(
            "RESIDENT_DECISION_APPLIED",
            json!({"decision": id, "action": field(decision, "action"), "activation": field(decision, "activation"), "assignment": field(decision, "assignment"), "step": field(decision, "step"), "approver": decider.name, "result": status, "source": "DASHBOARD"}),
        );
        results.push(json!({"decision_id": id, "status": status, "detail": detail.chars().take(300).collect::<String>(), "state": state}));
    }
    let _ = store.save();
    results
}
