// The command families through the real binary: the shared result envelope and its stable exit
// codes (0 ok or passed, 3 denied, failed, or blocked, 4 approval required, 5 unknown or
// unresolved), the fixture-based rule tests, a skill drafted from a template and checked by every
// family, a runbook that refuses an unpublished skill and self-activation, the selection aliases
// beside the legacy commands, the catalog forms of the scope commands, and the agent gate.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** Run a family command with --format json and return its exit code and envelope
 * Input
    - fixture: &Fixture - repository
    - args: &[&str] - arguments
 * Output
    - (Option<i32>, Value)
*/
fn envelope(fixture: &Fixture, args: &[&str]) -> (Option<i32>, Value) {
    let mut full = args.to_vec();
    full.extend(["--format", "json"]);
    let output = fixture.run(&full);
    let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "crane {args:?} printed no JSON ({error}):\n{}",
            text(&output)
        )
    });
    (output.status.code(), value)
}

/** The rule tests that run on fixture systems pass, and an unavailable profile is blocked (3)
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn fixture_rule_tests_pass_with_stable_exit_codes() {
    let fixture = Fixture::ready();
    for family in ["approval", "recovery", "revocation", "budget", "provenance"] {
        let (code, value) = envelope(&fixture, &[family, "test"]);
        assert_eq!(code, Some(0), "{family}: {value}");
        assert_eq!(value["status"], "passed", "{family}: {value}");
        assert_eq!(value["schema_version"], 1);
        assert!(value["request_id"].as_str().unwrap().starts_with("req_"));
        assert!(
            value["data"]["cases"]
                .as_array()
                .unwrap()
                .iter()
                .all(|case| case["status"] == "PASS"),
            "{family}: {value}"
        );
    }
    let (code, value) = envelope(&fixture, &["isolation", "test", "--profile", "sandbox"]);
    assert_eq!(
        (code, value["status"].as_str()),
        (Some(3), Some("blocked")),
        "{value}"
    );
    let (code, value) = envelope(&fixture, &["isolation", "profiles"]);
    assert_eq!(code, Some(0));
    assert_eq!(value["data"]["profiles"].as_array().unwrap().len(), 3);
    let (code, value) = envelope(
        &fixture,
        &["isolation", "test", "--profile", "typed-adapters"],
    );
    assert_eq!(code, Some(0), "{value}");
}

/** A skill drafted from a template grants nothing and is checked by every family; it cannot be
 * activated unvalidated; a runbook over it does not validate and its author cannot activate it
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn skills_and_runbooks_are_checked_end_to_end() {
    let fixture = Fixture::ready();
    let (code, created) = envelope(
        &fixture,
        &[
            "skill",
            "create",
            "--from-template",
            "read-only-lookup",
            "--name",
            "Look up a commit",
            "--department",
            "engineering",
        ],
    );
    assert_eq!(code, Some(0), "{created}");
    assert_eq!(created["refs"]["skill_ref"], "look-up-a-commit@1");
    assert_eq!(created["data"]["state"], "candidate");
    let (code, inspected) = envelope(&fixture, &["skill", "inspect", "look-up-a-commit"]);
    assert_eq!(code, Some(0), "{inspected}");
    assert_eq!(
        inspected["data"]["definition"]["applicability"]["business_domain"],
        "engineering"
    );
    // Drafted claims carry no evidence: validation fails (3), nothing is activated
    let (code, validated) = envelope(
        &fixture,
        &["skill", "validate", "look-up-a-commit@1", "--dry-run"],
    );
    assert_eq!(code, Some(3), "{validated}");
    for family in ["permission", "precondition", "constraint", "verification"] {
        let (code, value) = envelope(&fixture, &[family, "test", "--skill", "look-up-a-commit"]);
        assert_eq!(code, Some(0), "{family}: {value}");
    }
    let (_, evaluated) = envelope(
        &fixture,
        &["verification", "evaluate", "--skill", "look-up-a-commit"],
    );
    assert_eq!(evaluated["status"], "unknown", "{evaluated}");
    let (code, denied) = envelope(
        &fixture,
        &[
            "permission",
            "check",
            "--operation",
            "shell/exec",
            "--resource",
            "repo:app/payments.py",
        ],
    );
    assert_eq!(
        (code, denied["decision"].as_str()),
        (Some(3), Some("DENY")),
        "{denied}"
    );
    assert_eq!(denied["reason_codes"], json!(["UNREGISTERED_OPERATION"]));
    let (code, read) = envelope(
        &fixture,
        &[
            "permission",
            "check",
            "--operation",
            "git/git.commit",
            "--resource",
            "repo:app/payments.py",
        ],
    );
    assert_eq!(
        (code, read["decision"].as_str()),
        (Some(0), Some("ALLOW")),
        "{read}"
    );
    let (code, crm) = envelope(
        &fixture,
        &[
            "permission",
            "check",
            "--operation",
            "git/git.commit",
            "--resource",
            "crm:account-1",
        ],
    );
    assert_eq!(code, Some(3), "{crm}");
    assert!(fixture
        .fails(&[
            "skill",
            "activate",
            "look-up-a-commit@1",
            "--review-ref",
            "PR-1"
        ])
        .contains("publication_denied"));
    // A runbook over the unpublished skill
    fixture.write("bindings.json", &json!({"binding_id": "lookup", "version": 1, "capability_id": "look-up-a-commit", "capability_version": 1, "steps": [{"step_id": "read", "adapter_id": "git", "operation_id": "git.commit", "params": {"commit": {"from": "input", "name": "subject"}}}], "verifications": []}).to_string());
    let (code, runbook) = envelope(
        &fixture,
        &[
            "runbook",
            "create",
            "--from-template",
            "linear",
            "--name",
            "Commit triage",
            "--skills",
            "look-up-a-commit@1",
            "--bindings",
            "bindings.json",
        ],
    );
    assert_eq!(code, Some(0), "{runbook}");
    assert_eq!(runbook["refs"]["runbook_ref"], "commit-triage@1");
    let (code, validated) = envelope(&fixture, &["runbook", "validate", "commit-triage@1"]);
    assert_eq!(code, Some(3), "{validated}");
    assert!(
        validated["reason_codes"]
            .as_array()
            .unwrap()
            .contains(&json!("skill_not_published")),
        "{validated}"
    );
    let (code, tested) = envelope(&fixture, &["runbook", "test", "commit-triage@1"]);
    assert_eq!(code, Some(3), "{tested}");
    let refusals = tested["data"]["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|case| {
            case["case"].as_str().unwrap().contains("cannot run")
                || case["case"].as_str().unwrap().contains("refused")
        })
        .collect::<Vec<_>>();
    assert!(
        !refusals.is_empty() && refusals.iter().all(|case| case["status"] == "PASS"),
        "{tested}"
    );
    let (code, activated) = envelope(
        &fixture,
        &[
            "runbook",
            "activate",
            "commit-triage@1",
            "--review-ref",
            "PR-2",
        ],
    );
    assert_eq!(
        (code, activated["status"].as_str()),
        (Some(3), Some("denied")),
        "{activated}"
    );
    assert!(fixture
        .fails(&[
            "runbook",
            "run",
            "commit-triage@1",
            "--idempotency-key",
            "k1"
        ])
        .contains("Draft"));
    // Agents never run Crane, whatever the family
    let agent = fixture.as_agent(&["skill", "inspect", "look-up-a-commit"]);
    assert!(!agent.status.success() && text(&agent).contains("cannot be executed by an AI agent"));
}

/** The selection aliases share the legacy handlers and records; the catalog forms of the scope
 * commands resolve resources and explain operations
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn selection_aliases_and_scope_forms() {
    let fixture = Fixture::ready();
    let output = fixture.ok(&[
        "selection",
        "protect",
        "app/payments.py",
        "start-line",
        "1",
        "end-line",
        "4",
        "context",
        "Fees are audited.",
    ]);
    let id = common::selection_id(&output);
    let legacy = fixture.protect(&["app/ledger.py"]);
    let (code, inspected) = envelope(&fixture, &["selection", "inspect", &id]);
    assert_eq!(code, Some(0), "{inspected}");
    assert_eq!(inspected["data"]["record"]["operation"], "preserve");
    let (code, validated) = envelope(&fixture, &["selection", "validate", &legacy]);
    assert_eq!(code, Some(0), "{validated}");
    let (code, repair) = envelope(&fixture, &["selection", "repair", &id]);
    assert_eq!(code, Some(0), "{repair}");
    let (code, resolved) = envelope(
        &fixture,
        &["scope", "resolve", "--resource", "repo:app/payments.py"],
    );
    assert_eq!(code, Some(0), "{resolved}");
    assert_eq!(resolved["data"]["resource"]["path"], "app/payments.py");
    let (code, unresolved) = envelope(
        &fixture,
        &["scope", "resolve", "--resource", "crm:account-1"],
    );
    assert_eq!(
        (code, unresolved["status"].as_str()),
        (Some(5), Some("unresolved")),
        "{unresolved}"
    );
    let (code, explained) = envelope(
        &fixture,
        &[
            "scope",
            "explain",
            "--operation",
            "git/git.commit",
            "--resource",
            "repo:app/payments.py",
        ],
    );
    assert_eq!(
        (code, explained["decision"].as_str()),
        (Some(0), Some("ALLOW")),
        "{explained}"
    );
    // The repository forms are unchanged
    assert!(fixture
        .ok(&[
            "scope",
            "resolve",
            "write",
            "app/payments.py",
            "lines",
            "2-5"
        ])
        .contains("DENY"));
}
