// The capability registry through the crane binary: the deterministic demonstration, and the
// intake, create, validate, publish, and revoke commands against a real repository whose Evidence
// Pack comes from 'crane capture investigate'.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** The demonstration runs deterministically and reports the blocked and the published capability
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn demo_runs_the_full_flow() {
    let fixture = Fixture::new();
    let (ok, report) = fixture.json(&["capability", "demo", "--json"]);
    assert!(ok);
    assert_eq!(report["recovery_outcome"], json!("needs_evidence"));
    assert_eq!(report["diagnostics_outcome"], json!("passed"));
    assert_eq!(report["eligible_before_change"], json!(1));
    assert_eq!(report["eligible_after_change"], Value::Null);
    let (_, again) = fixture.json(&["capability", "demo", "--json"]);
    assert_eq!(again, report);
    let output = fixture.ok(&["capability", "demo"]);
    assert!(
        output.contains("publication_denied") && output.contains("hypothesis cannot verify"),
        "{output}"
    );
}

/** A local Evidence Pack becomes an intake and a capability that validates; the author cannot
 * publish it, another person can, and the published version is signed into the shared skill store
 * (tampering with it fails validation); an admin can revoke it, in the store too; agents cannot run
 * any of it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn manages_a_capability_from_a_local_pack() {
    let fixture = Fixture::ready();
    let (ok, investigation) = fixture.json(&["capture", "investigate", "--json"]);
    assert!(ok);
    let pack = &investigation["pack"];
    let (pack_id, digest, episode) = (
        pack["pack_id"].as_str().unwrap(),
        pack["manifest"]["digest"].as_str().unwrap(),
        pack["episode_id"].as_str().unwrap(),
    );
    let code = pack["evidence"]
        .as_array()
        .unwrap()
        .iter()
        .find(|evidence| evidence["evidence_type"] == json!("code_change"))
        .unwrap()["evidence_id"]
        .as_str()
        .unwrap()
        .to_string();
    fixture.write("intent.json", &json!({"objective": null, "process_domain": null, "acceptance_criteria": [{"id": "ac-1", "statement": "The change is summarized with references", "mapping": "human_confirmed", "confirmed_by": "Dev"}], "invariants": [], "outcome_measures": [], "owner": "team:dev", "authoritative_systems": []}).to_string());
    let intake_args = [
        "capability",
        "intake",
        "episode",
        episode,
        "pack",
        pack_id,
        "digest",
        digest,
        "intent",
        "intent.json",
        "risk",
        "low",
        "profile",
        "low-risk-technical",
        "--json",
    ];
    let (ok, submitted) = fixture.json(&intake_args);
    assert!(ok, "{submitted}");
    assert_eq!(
        submitted["intake"]["status"],
        json!("ready"),
        "{}",
        submitted["intake"]["validation"]["findings"]
    );
    let intake_id = submitted["intake"]["intake_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, repeated) = fixture.json(&intake_args);
    assert_eq!(
        (
            repeated["duplicate"].clone(),
            repeated["intake"]["intake_id"].as_str()
        ),
        (json!(true), Some(intake_id.as_str()))
    );
    let wrong = fixture.fails(&[
        "capability",
        "intake",
        "episode",
        episode,
        "pack",
        pack_id,
        "digest",
        &"0".repeat(32),
    ]);
    assert!(wrong.contains("integrity_mismatch"), "{wrong}");
    let definition = json!({
        "capability_id": "change-summary", "version": 1, "schema_version": 1,
        "name": "Summarize a local change", "description": "Read-only summary of a commit with references.",
        "owner_ref": "team:dev", "previous_version_ref": null,
        "provenance": {"creation_method": "manual", "created_by": {"id": "Dev", "kind": "human"}, "created_at": 1,
            "source_intake_refs": [intake_id], "source_episode_refs": [episode], "source_evidence_pack_refs": [{"pack_id": pack_id, "manifest_digest": digest}]},
        "applicability": {"purpose": "Summarize what a commit changed", "task_classes": ["change-review"], "environments": ["local"],
            "preconditions": [], "required_state": [], "exclusions": [], "limitations": [], "inputs": [{"name": "commit", "kind": "revision", "required": true, "description": "Commit"}],
            "outputs": [], "assumptions": [], "risk": "low", "review_level": "peer"},
        "dependencies": [{"kind": "evidence_pack", "target_id": pack_id, "required": true, "constraint": {"constraint": "exact", "value": digest}, "reason": "Derived from it"}],
        "execution": {"procedure_version": 1, "inputs": [{"name": "commit", "kind": "revision", "required": true, "description": "Commit"}], "preconditions": [],
            "steps": [{"step_id": "read-commit", "kind": "query", "description": "Read the commit and its files", "inputs": ["commit"], "outputs": ["summary"], "preconditions": [], "verifies": ["files-listed"]}],
            "effects": [], "state_transitions": [], "postconditions": [], "outputs": [], "failure_conditions": [], "checkpoints": ["files-listed"], "recovery": [], "side_effects": []},
        "verification": {"method": "evidence-review", "method_version": "1", "scope_environments": ["local"],
            "claims": [{"claim_id": "files-listed", "kind": "technical", "statement": "The commit's files can be listed", "expected_result": "A file list", "required": true, "status": "verified",
                "evidence_refs": [code], "assertion_refs": [], "method": "evidence-review", "environment": "local"}],
            "invariants": [], "negative_cases": [], "limitations": [], "unresolved_assumptions": []},
        "known_failure_cases": [], "proposed_improvements": []
    });
    fixture.write("capability.json", &definition.to_string());
    let (ok, checked) = fixture.json(&[
        "capability",
        "validate",
        "file",
        "capability.json",
        "--json",
    ]);
    assert!(ok && checked["dry_run"] == json!(true), "{checked}");
    fixture.ok(&[
        "capability",
        "create",
        "capability.json",
        "profile",
        "low-risk-technical",
    ]);
    let (_, dry) = fixture.json(&[
        "capability",
        "validate",
        "change-summary",
        "--dry-run",
        "--json",
    ]);
    assert_eq!(dry["outcome"], json!("passed"), "{dry}");
    let (_, inspected) = fixture.json(&["capability", "inspect", "change-summary", "--json"]);
    assert_eq!(
        inspected["record"]["state"],
        json!("candidate"),
        "a dry run changes nothing"
    );
    let (_, recorded) = fixture.json(&["capability", "validate", "change-summary", "--json"]);
    assert_eq!(recorded["outcome"], json!("passed"));
    let (_, inspected) = fixture.json(&[
        "capability",
        "inspect",
        "change-summary",
        "task-class",
        "change-review",
        "environment",
        "local",
        "--json",
    ]);
    assert_eq!(inspected["record"]["state"], json!("validated"));
    assert_eq!(inspected["applicability"]["applicable"], json!("yes"));
    assert!(inspected["provenance"]["references"]
        .as_array()
        .unwrap()
        .iter()
        .all(|reference| reference["outcome"] == json!("resolved")));
    let denied = fixture.fails(&[
        "capability",
        "publish",
        "change-summary",
        "version",
        "1",
        "reason",
        "share it",
    ]);
    assert!(
        denied.contains("publication_denied") && denied.contains("cannot publish their own"),
        "{denied}"
    );
    // Another person publishes it, and the exact version is signed into the shared skill store
    common::git(&fixture.work, &["config", "user.name", "Reviewer"]);
    let published = fixture.ok(&[
        "capability",
        "publish",
        "change-summary",
        "version",
        "1",
        "reason",
        "share it",
        "acknowledge",
        "undeclared_governance",
    ]);
    assert!(published.contains(".crane/skills.json"), "{published}");
    let store = fixture.crane_json("skills.json");
    assert_eq!(store["skills"][0]["state"], json!("published"), "{store}");
    assert_eq!(store["skills"][0]["published_by"], json!("Reviewer"));
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    let (_, shared) = fixture.json(&["capability", "shared", "--json"]);
    assert_eq!(shared["store"], json!("verified"), "{shared}");
    // Nobody can edit the store outside Crane: it is covered by the signed manifest
    let original = fixture.read(".crane/skills.json");
    fixture.write(
        ".crane/skills.json",
        &original.replace("\"published\"", "\"deprecated\""),
    );
    assert!(fixture
        .fails(&["validate"])
        .contains("skills_store_changed"));
    let (_, tampered) = fixture.json(&["capability", "shared", "--json"]);
    assert!(tampered["store"]
        .as_str()
        .unwrap()
        .starts_with("UNVERIFIED"));
    fixture.write(".crane/skills.json", &original);
    let revoked = fixture.ok(&[
        "capability",
        "revoke",
        "change-summary",
        "reason",
        "superseded",
    ]);
    assert!(
        revoked.contains("signed into .crane/skills.json"),
        "{revoked}"
    );
    let store = fixture.crane_json("skills.json");
    assert_eq!(store["skills"][0]["state"], json!("revoked"));
    assert_eq!(store["skills"][0]["changed_by"], json!("Reviewer"));
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    let (_, listed) = fixture.json(&["capability", "list", "--json"]);
    assert_eq!(listed, json!([]));
    let (_, everything) = fixture.json(&["capability", "list", "--all", "--json"]);
    assert_eq!(everything[0]["versions"], json!([[1, "revoked"]]));
    let (_, history) = fixture.json(&["capability", "versions", "change-summary", "--json"]);
    assert_eq!(history["transitions"].as_array().unwrap().len(), 5);
    let refused = fixture.as_agent(&["capability", "list"]);
    assert!(
        !refused.status.success() && text(&refused).contains("cannot be executed by an AI agent")
    );
}
