// Capability invocation through the crane binary: the deterministic demonstration, and the
// commands in a real repository, where an unpublished candidate is blocked before anything runs,
// the refusal is recorded in the ledger and in DataSea's event log, and secrets are refused.

mod common;

use std::fs;

use common::{text, Fixture};
use serde_json::json;

/** The demonstration reports every required outcome deterministically
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_reports_every_outcome() {
    let fixture = Fixture::new();
    let (ok, report) = fixture.json(&["capability", "invoke", "demo", "--json"]);
    assert!(ok);
    assert_eq!(report["succeeded"]["state"], json!("succeeded"));
    assert_eq!(report["repeat_returns_same"], json!(true));
    for blocked in ["changed_digest", "stale", "revoked"] {
        assert_eq!(report[blocked], json!("blocked"), "{blocked}");
    }
    assert_eq!(report["unsupported"][2], json!(0));
    assert_eq!(report["reconciled"]["state"], json!("succeeded"));
    assert_eq!(report["release_dispatches"], json!(1));
    assert_eq!(report["failed_verification"], json!("failed"));
    let (_, again) = fixture.json(&["capability", "invoke", "demo", "--json"]);
    assert_eq!(again, report);
}

/** In a repository, an unpublished candidate is blocked before dispatch and recorded; run and
 * reconcile refuse it; secrets in inputs are refused; agents cannot invoke
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn blocks_unpublished_candidates_in_a_repository() {
    let fixture = Fixture::ready();
    let (_, investigation) = fixture.json(&["capture", "investigate", "--json"]);
    let pack = &investigation["pack"];
    let (_, compiled) = fixture.json(&[
        "compile",
        "episode",
        pack["episode_id"].as_str().unwrap(),
        "pack",
        pack["pack_id"].as_str().unwrap(),
        "digest",
        pack["manifest"]["digest"].as_str().unwrap(),
        "capability",
        "local-change",
        "name",
        "Make a local change",
        "purpose",
        "Change the repository and commit it",
        "task-class",
        "local-change",
        "--json",
    ]);
    let digest = compiled["candidate"]["digest"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, inspected) = fixture.json(&["capability", "inspect", "local-change", "--json"]);
    let step = inspected["record"]["definition"]["execution"]["steps"][0]["step_id"]
        .as_str()
        .unwrap()
        .to_string();
    fixture.write("bindings.json", &json!({"binding_id": "local", "version": 1, "capability_id": "local-change", "capability_version": 1, "steps": [{"step_id": step, "adapter_id": "git", "operation_id": "git.commit", "params": {"commit": {"from": "input", "name": "commit"}}}], "verifications": []}).to_string());
    fixture.write("inputs.json", &json!({"commit": "HEAD"}).to_string());
    let (ok, record) = fixture.json(&[
        "capability",
        "invoke",
        "plan",
        "local-change",
        "version",
        "1",
        "digest",
        &digest,
        "bindings",
        "bindings.json",
        "inputs",
        "inputs.json",
        "key",
        "local-1",
        "environment",
        "local",
        "--json",
    ]);
    assert!(ok, "{record}");
    assert_eq!(record["state"], json!("blocked"));
    assert!(
        record["operations"]
            .as_array()
            .unwrap()
            .iter()
            .all(|operation| operation["attempts"] == json!(0))
            || record["operations"].as_array().unwrap().is_empty()
    );
    let id = record["invocation_id"].as_str().unwrap().to_string();
    let (_, listed) = fixture.json(&["capability", "invoke", "list", "--json"]);
    assert_eq!(listed[0]["invocation_id"], json!(id));
    let (_, same) = fixture.json(&["capability", "invoke", "inspect", &id, "--json"]);
    assert_eq!(same["record_digest"], record["record_digest"]);
    let refused = fixture.fails(&["capability", "invoke", "run", &id]);
    assert!(refused.contains("invalid_transition"), "{refused}");
    assert!(fixture
        .fails(&["capability", "invoke", "reconcile", &id])
        .contains("nothing to reconcile"));
    let runtime = fs::read_dir(fixture.home.join("repos"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("runtime");
    assert!(runtime.join("invocation").join("ledger.json").exists());
    let events = fs::read_to_string(runtime.join("events.jsonl")).unwrap();
    assert!(events.contains("capability.invocation.blocked"));
    fixture.write(
        "secret.json",
        &json!({"commit": "HEAD", "token": "ghp_0123456789abcdefghijklmnopqrstuvwxyz"}).to_string(),
    );
    let secret = fixture.fails(&[
        "capability",
        "invoke",
        "plan",
        "local-change",
        "version",
        "1",
        "digest",
        &digest,
        "bindings",
        "bindings.json",
        "inputs",
        "secret.json",
        "key",
        "local-2",
        "environment",
        "local",
    ]);
    assert!(
        secret.contains("credential reference") && !secret.contains("ghp_0123456789"),
        "{secret}"
    );
    let agent = fixture.as_agent(&["capability", "invoke", "demo"]);
    assert!(!agent.status.success() && text(&agent).contains("cannot be executed by an AI agent"));
}
