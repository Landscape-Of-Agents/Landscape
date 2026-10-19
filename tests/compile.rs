// The capability compiler through the crane binary: the deterministic demonstration, and the
// compilation of a real repository's work episode (an Evidence Pack from 'crane capture
// investigate') into a candidate stored by the capability registry.

mod common;

use std::fs;

use common::{text, Fixture};
use serde_json::{json, Value};

/** The demonstration compiles the successful episode and refuses the failed one
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_compiles_and_refuses() {
    let fixture = Fixture::new();
    let (ok, report) = fixture.json(&["compile", "demo", "--json"]);
    assert!(ok);
    assert_eq!(report["success"]["outcome"], json!("candidate_generated"));
    assert_eq!(
        report["failed"]["outcome"],
        json!("insufficient_reusable_signal")
    );
    assert_eq!(report["state_after_validation"], json!("validated"));
    let (_, again) = fixture.json(&["compile", "demo", "--json"]);
    assert_eq!(again, report);
    let output = fixture.ok(&["compile", "demo"]);
    assert!(
        output.contains("CandidateGenerated") && output.contains("InsufficientReusableSignal"),
        "{output}"
    );
}

/** A local episode compiles into a stored candidate that still needs evidence (its outcome was
 * never recorded), the report is kept, and an agent cannot compile
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn compiles_a_local_episode() {
    let fixture = Fixture::ready();
    let (ok, investigation) = fixture.json(&["capture", "investigate", "--json"]);
    assert!(ok);
    let pack = &investigation["pack"];
    let args = [
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
        "risk",
        "low",
        "--json",
    ];
    let (ok, report) = fixture.json(&args);
    assert!(ok, "{report}");
    assert_eq!(
        report["outcome"],
        json!("needs_more_evidence"),
        "{}",
        report["reasons"]
    );
    assert!(report["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason.as_str().unwrap().contains("not recorded")));
    assert!(report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning.as_str().unwrap().contains("raised to medium")));
    assert_eq!(report["candidate"]["version"], json!(1));
    let (_, listed) = fixture.json(&["capability", "list", "--json"]);
    assert_eq!(listed[0]["versions"], json!([[1, "candidate"]]));
    let runtime = fs::read_dir(fixture.home.join("repos"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("runtime");
    let reports = fs::read_to_string(runtime.join("compiler").join("reports.jsonl")).unwrap();
    let stored: Value = serde_json::from_str(reports.lines().next().unwrap()).unwrap();
    assert_eq!(stored["digest"], report["digest"]);
    let (_, repeated) = fixture.json(&args);
    assert_eq!(repeated["candidate"]["reused"], json!(true));
    let refused = fixture.as_agent(&["compile", "demo"]);
    assert!(
        !refused.status.success() && text(&refused).contains("cannot be executed by an AI agent")
    );
}
