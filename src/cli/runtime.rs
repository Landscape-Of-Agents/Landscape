// Runtime views: the current or a given task or session of the AI agent, every task of the current
// session, and every session. The Foxx dashboard is hosted by the ingestion service.

use serde_json::{json, Value};

use crate::foxx::views::{session_view, task_view, Snapshot};
use crate::governance::workspace::Workspace;
use crate::platform::render::yaml;

/** Print a value as YAML-like text
 * Input
    - value: &Value - value
 * Output
    - None (writes to stdout)
*/
fn print(value: &Value) {
    print!("{}", yaml(value, 0));
}

/** Show the current task (of the most recently active session) or a given task, or list the
 * tasks of the current session
 * Input
    - args: &[String] - "current [TASK_ID]" or "."
 * Output
    - Result<(), String>
*/
pub(crate) fn task(args: &[String]) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let snapshot = Snapshot::load(&workspace)?;
    match args {
        [word] if word == "current" => {
            let session = snapshot.sessions.first().ok_or("no agent session has been observed yet; install hooks with 'crane agent install --profile claude|codex'")?;
            let task = session
                .current_task
                .as_ref()
                .and_then(|id| snapshot.task(id))
                .ok_or("the current session has no task yet")?;
            print(&task_view(&snapshot, task));
            Ok(())
        }
        [word, id] if word == "current" => {
            let task = snapshot.task(id).ok_or_else(|| format!("no task '{id}'"))?;
            print(&task_view(&snapshot, task));
            Ok(())
        }
        [dot] if dot == "." => {
            let Some(session) = snapshot.sessions.first() else {
                println!("tasks: []");
                return Ok(());
            };
            let tasks = snapshot
                .tasks
                .iter()
                .filter(|task| task.foxx_session_id == session.foxx_session_id)
                .map(|task| {
                    let view = task_view(&snapshot, task);
                    let metric = |name: &str| view["metrics"][name]["value"].clone();
                    json!({
                        "foxx_task_id": task.foxx_task_id,
                        "external_task_id": task.external_task_id,
                        "name": task.name,
                        "status": task.status,
                        "started_at": view["task"]["started_at"],
                        "ended_at": view["task"]["ended_at"],
                        "tool_calls_decided": metric("tool_calls_decided"),
                        "actions_denied": metric("actions_denied"),
                        "bypass_attempts": metric("bypass_attempts"),
                        "credits_consumed": metric("credits_consumed"),
                    })
                })
                .collect::<Vec<_>>();
            print(&json!({"session": session.foxx_session_id, "tasks": tasks}));
            Ok(())
        }
        _ => Err("usage: crane task current [TASK_ID] | crane task .".into()),
    }
}

/** Show the current session or a given session, or list every session
 * Input
    - args: &[String] - "current [SESSION_ID]" or "."
 * Output
    - Result<(), String>
*/
pub(crate) fn session(args: &[String]) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let snapshot = Snapshot::load(&workspace)?;
    match args {
        [word] if word == "current" => {
            let session = snapshot.sessions.first().ok_or("no agent session has been observed yet; install hooks with 'crane agent install --profile claude|codex'")?;
            print(&session_view(&snapshot, session));
            Ok(())
        }
        [word, id] if word == "current" => {
            let session = snapshot
                .session(id)
                .ok_or_else(|| format!("no session '{id}'"))?;
            print(&session_view(&snapshot, session));
            Ok(())
        }
        [dot] if dot == "." => {
            let sessions = snapshot
                .sessions
                .iter()
                .map(|session| {
                    let view = session_view(&snapshot, session);
                    json!({
                        "foxx_session_id": session.foxx_session_id,
                        "provider": session.provider,
                        "provider_session_id": session.provider_session_id,
                        "model": session.model,
                        "status": view["session"]["status"],
                        "started_at": view["session"]["started_at"],
                        "last_seen_at": view["session"]["last_seen_at"],
                        "safety": session.safety,
                        "tasks": session.tasks.len(),
                        "credits_available": view["credits"]["available"],
                        "actions_denied": view["metrics"]["actions_denied"]["value"],
                        "bypass_attempts": view["metrics"]["bypass_attempts"]["value"],
                    })
                })
                .collect::<Vec<_>>();
            print(&json!({"sessions": sessions}));
            Ok(())
        }
        _ => Err("usage: crane session current [SESSION_ID] | crane session .".into()),
    }
}
