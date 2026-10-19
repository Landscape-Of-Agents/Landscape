// Capability discovery through the crane binary: the deterministic demonstration, and discovery
// in a real repository, first with an empty registry and then with an unpublished candidate.

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
    let (ok, report) = fixture.json(&["capability", "discover", "demo", "--json"]);
    assert!(ok);
    assert_eq!(report["with_context"]["outcome"], json!("recommended"));
    assert_eq!(
        report["with_context"]["recommendations"][0]["capability_id"],
        json!("refund-change-with-verified-deploy")
    );
    assert_eq!(
        report["unknown_environment"]["outcome"],
        json!("insufficient_context")
    );
    assert_eq!(report["staging"]["outcome"], json!("no_match"));
    assert_eq!(report["stale"]["outcome"], json!("blocked"));
    let (_, again) = fixture.json(&["capability", "discover", "demo", "--json"]);
    assert_eq!(again, report);
}

/** Discovery in a repository: an empty registry has no match; an unpublished candidate is never
 * recommended; reports are stored; the registry is unchanged; agents cannot discover
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn discovers_in_a_repository() {
    let fixture = Fixture::ready();
    let (_, investigation) = fixture.json(&["capture", "investigate", "--json"]);
    let pack = &investigation["pack"];
    let (pack_id, digest) = (
        pack["pack_id"].as_str().unwrap(),
        pack["manifest"]["digest"].as_str().unwrap(),
    );
    let (ok, empty) = fixture.json(&[
        "capability",
        "discover",
        "task-class",
        "local-change",
        "pack",
        pack_id,
        "digest",
        digest,
        "--json",
    ]);
    assert!(ok, "{empty}");
    assert_eq!(empty["outcome"], json!("no_match"));
    fixture.json(&[
        "compile",
        "episode",
        pack["episode_id"].as_str().unwrap(),
        "pack",
        pack_id,
        "digest",
        digest,
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
    let (_, before) = fixture.json(&["capability", "versions", "local-change", "--json"]);
    let (ok, report) = fixture.json(&[
        "capability",
        "discover",
        "task-class",
        "local-change",
        "pack",
        pack_id,
        "digest",
        digest,
        "--json",
    ]);
    assert!(ok);
    assert_eq!(report["outcome"], json!("no_match"));
    assert!(
        report["candidates"][0]["reasons"][0]
            .as_str()
            .unwrap()
            .contains("Candidate"),
        "{}",
        report["candidates"]
    );
    let (_, after) = fixture.json(&["capability", "versions", "local-change", "--json"]);
    assert_eq!(before, after, "discovery changes nothing in the registry");
    let wrong = fixture.ok(&[
        "capability",
        "discover",
        "task-class",
        "local-change",
        "pack",
        pack_id,
        "digest",
        &"0".repeat(32),
    ]);
    assert!(
        wrong.contains("Blocked") && wrong.contains("integrity_mismatch"),
        "{wrong}"
    );
    let (_, stored) = fixture.json(&["capability", "discover", "reports", "--json"]);
    assert_eq!(stored.as_array().unwrap().len(), 3);
    let refused = fixture.as_agent(&["capability", "discover", "demo"]);
    assert!(
        !refused.status.success() && text(&refused).contains("cannot be executed by an AI agent")
    );
}
