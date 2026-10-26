// crane revocation inspect: whether a skill version (on this machine and in the shared store), a
// runbook version, a run, or an admin key is revoked, withdrawn, or still in force.

use serde_json::json;

use crate::capability::shared::{key, SharedView};
use crate::cli::family::{reference, runbooks, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, versioned, Response, Status};
use crate::governance::admins::signers;
use crate::governance::state::State;

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["skill", "runbook", "run", "actor"], &[])?;
    args.expect_positional(0, "crane revocation inspect --skill SKILL[@N] | --runbook RUNBOOK[@N] | --run RUN_ID | --actor PUBLIC_KEY")?;
    let local = Local::open()?;
    let (summary, data) = if let Some(reference_text) = args.value("skill") {
        let record = skill(&local, &reference_text)?;
        let shared = match &local.shared {
            SharedView::Absent => "no shared store".to_string(),
            SharedView::Unverified(reason) => {
                format!("shared store UNVERIFIED ({reason}): every invocation is blocked")
            }
            SharedView::Verified(entries) => entries
                .get(&key(
                    &record.definition.capability_id,
                    record.definition.version,
                ))
                .map_or("not in the shared store".into(), |entry| {
                    format!(
                        "{:?} in the shared store{}",
                        entry.state,
                        entry
                            .changed_by
                            .as_deref()
                            .map(|who| format!(" (by {who})"))
                            .unwrap_or_default()
                    )
                }),
        };
        (
            format!("{}: {:?} here; {shared}", reference(record), record.state),
            json!({"skill": reference(record), "state": record.state, "shared": shared, "withdrawals": record.withdrawals}),
        )
    } else if let Some(reference_text) = args.value("runbook") {
        let (id, version) = versioned(&reference_text)?;
        let store = runbooks(&local)?;
        let record = store.runbook(&id, version)?;
        (
            format!(
                "runbook {id}@{}: {:?}",
                record.definition.version, record.state
            ),
            json!({"runbook": format!("{id}@{}", record.definition.version), "state": record.state, "changes": record.changes}),
        )
    } else if let Some(run) = args.value("run") {
        let store = runbooks(&local)?;
        let record = store.run(&run)?;
        let runbook = store.runbook(&record.runbook_id, Some(record.version))?;
        (
            format!(
                "run {run}: {:?}; its runbook version is {:?}",
                record.state, runbook.state
            ),
            json!({"run": run, "state": record.state, "runbook_state": runbook.state}),
        )
    } else if let Some(key) = args.value("actor") {
        let admins = signers(&State::load(&local.context.workspace)?);
        let active = admins.contains(&key);
        (
            format!(
                "key {}: {}",
                &key[..key.len().min(16)],
                if active {
                    "an active signer (owner or admin)"
                } else {
                    "not a signer (never one, or removed with 'crane admin del')"
                }
            ),
            json!({"key": key, "signer": active}),
        )
    } else {
        return Err("give --skill, --runbook, --run, or --actor".into());
    };
    finish(
        &args,
        Response::new("revocation.inspect", Status::Ok, summary).data(data),
        &[],
    )
}
