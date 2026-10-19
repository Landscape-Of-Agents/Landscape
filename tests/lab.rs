// The Capability Validation Lab through the crane binary: the deterministic demonstration, and a
// lab run on a candidate compiled from a real repository's work episode.

mod common;

use common::{text, Fixture};
use serde_json::json;

/** The demonstration reports each outcome deterministically
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_reports_each_outcome() {
    let fixture = Fixture::new();
    let (ok, report) = fixture.json(&["lab", "demo", "--json"]);
    assert!(ok);
    assert_eq!(report["compiled"]["overall"], json!("inconclusive"));
    assert_eq!(report["needs_evidence"]["overall"], json!("blocked"));
    assert_eq!(report["tampered"]["overall"], json!("failed"));
    assert_eq!(report["stale"]["overall"], json!("failed"));
    assert_eq!(report["compiled"]["environment"]["executed"], json!(false));
    let (_, again) = fixture.json(&["lab", "demo", "--json"]);
    assert_eq!(again, report);
}

/** A candidate compiled from a local episode is checked against an approved plan; its blocking
 * gaps stay blocked, the report is stored and referenced, and agents cannot run the lab
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn checks_a_locally_compiled_candidate() {
    let fixture = Fixture::ready();
    let (_, investigation) = fixture.json(&["capture", "investigate", "--json"]);
    let pack = &investigation["pack"];
    let (ok, compiled) = fixture.json(&[
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
    assert!(ok, "{compiled}");
    fixture.write(
        "plan.json",
        &json!({"plan_id": "local-plan", "version": 1, "description": "Local checks", "scenarios": [
            {"scenario_id": "unknown-environment", "description": "An unknown environment never matches", "check": {"kind": "applicability", "task_class": "local-change", "environment": null, "preconditions": [], "expect": "unknown"}},
            {"scenario_id": "recovery-untested", "description": "Recovery was never exercised", "check": {"kind": "negative_case", "declares": "Recovery or rollback was not exercised"}}
        ]})
        .to_string(),
    );
    let (ok, report) = fixture.json(&["lab", "run", "local-change", "plan", "plan.json", "--json"]);
    assert!(ok, "{report}");
    assert_eq!(report["compilation"]["report_id"], compiled["report_id"]);
    assert_eq!(report["candidate_digest"], compiled["candidate"]["digest"]);
    assert_eq!(
        report["overall"],
        json!("blocked"),
        "the compiler's blocking gaps stay blocked"
    );
    for scenario in ["scenario:unknown-environment", "scenario:recovery-untested"] {
        let check = report["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["check_id"] == json!(scenario))
            .unwrap();
        assert_eq!(check["status"], json!("passed"), "{check}");
    }
    let (_, references) = fixture.json(&["lab", "reports", "local-change", "--json"]);
    assert_eq!(references[0]["report_id"], report["report_id"]);
    let (_, listed) = fixture.json(&["capability", "list", "--json"]);
    assert_eq!(
        listed[0]["versions"],
        json!([[1, "candidate"]]),
        "the lab changes no lifecycle state"
    );
    let wrong = fixture.fails(&[
        "lab",
        "run",
        "local-change",
        "plan",
        "plan.json",
        "digest",
        &"0".repeat(32),
    ]);
    assert!(
        wrong.contains("Failed") && wrong.contains("candidate-digest"),
        "{wrong}"
    );
    let refused = fixture.as_agent(&["lab", "demo"]);
    assert!(
        !refused.status.success() && text(&refused).contains("cannot be executed by an AI agent")
    );
}
