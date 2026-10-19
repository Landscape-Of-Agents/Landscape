// Capability evolution through the crane binary: the deterministic demonstration, and the
// commands in a real repository, where the analysis reads the invocation ledger without changing
// it, unknown records are refused, a tampered evolution store fails closed, and agents are
// refused.

mod common;

use std::fs;

use common::{text, Fixture};
use serde_json::json;

/** The demonstration reports all fifteen cases as expected, deterministically
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_matches_every_case() {
    let fixture = Fixture::new();
    let (ok, report) = fixture.json(&["evolution", "demo", "--json"]);
    assert!(ok, "{report}");
    assert_eq!(report["all_matched"], json!(true));
    assert_eq!(report["cases"].as_array().unwrap().len(), 15);
    assert!(report["notice"].as_str().unwrap().starts_with("SYNTHETIC"));
    let (_, again) = fixture.json(&["evolution", "demo", "--json"]);
    assert_eq!(again, report);
    let help = fixture.ok(&["help"]);
    assert!(help.contains("crane evolution analyze"));
    assert!(fixture
        .fails(&["evolution"])
        .contains("usage: crane evolution"));
}

/** In a repository: analysis reads the ledger read-only, reports a blocked invocation as no
 * trial, creates no candidate, refuses unknown records, and fails closed on a tampered store
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn analyzes_a_repository_read_only() {
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
    fixture.json(&[
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
    let runtime = fs::read_dir(fixture.home.join("repos"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("runtime");
    let ledger = fs::read_to_string(runtime.join("invocation").join("ledger.json")).unwrap();
    let (ok, analysis) = fixture.json(&["evolution", "analyze", "local-change", "--json"]);
    assert!(ok, "{analysis}");
    let report = &analysis["report"];
    assert_eq!(report["coverage"]["records_visible"], json!(1));
    assert_eq!(
        report["coverage"]["independent"],
        json!(0),
        "a blocked invocation is not a trial"
    );
    assert_eq!(
        report["outcome_counts"]["blocked_before_execution"],
        json!(1)
    );
    assert!(analysis["signals"]
        .as_array()
        .unwrap()
        .iter()
        .all(|signal| signal["kind"] != json!("repeated_failure")));
    assert_eq!(
        fs::read_to_string(runtime.join("invocation").join("ledger.json")).unwrap(),
        ledger,
        "the ledger is read only"
    );
    fixture.fails(&["capability", "inspect", "local-change", "version", "2"]);
    let (_, again) = fixture.json(&["evolution", "analyze", "local-change", "--json"]);
    assert_eq!(again["reused"], json!(true));
    assert_eq!(again["report"]["report_id"], report["report_id"]);
    let (ok, drift) = fixture.json(&["evolution", "drift", "local-change", "--json"]);
    assert!(ok, "{drift}");
    assert!(drift["findings"].is_array());
    let (_, proposals) = fixture.json(&["evolution", "proposals", "--json"]);
    assert_eq!(proposals, json!([]));
    assert!(fixture
        .fails(&["evolution", "propose", "sig_missing"])
        .contains("not_found"));
    assert!(fixture
        .fails(&["evolution", "regression", "run", "prp_missing"])
        .contains("not_found"));
    assert!(fixture
        .fails(&["evolution", "analyze", "no-such-capability"])
        .contains("not_found"));
    let store = runtime.join("evolution").join("state.json");
    let saved = fs::read_to_string(&store).unwrap();
    fs::write(
        &store,
        saved.replacen("\"records_visible\": 1", "\"records_visible\": 7", 1),
    )
    .unwrap();
    assert!(fixture
        .fails(&["evolution", "signals"])
        .contains("integrity_mismatch"));
    let agent = fixture.as_agent(&["evolution", "demo"]);
    assert!(!agent.status.success() && text(&agent).contains("cannot be executed by an AI agent"));
}
