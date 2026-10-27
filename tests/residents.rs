// Resident agents through the real binary: a definition and an RCA graph saved and activated by
// the signer, an assignment drafted (with a dry run that saves nothing), validated, and activated
// inside a signed Brick assignment; a plan-only test that executes nothing; a delivered CI trigger
// that runs to a verified outcome and back to idle, once despite redelivery; a tick that makes no
// model call; the agent gate; and a revoked Brick assignment that degrades the assignment and
// blocks its next activation.

mod common;

use std::path::PathBuf;

use common::{text, Fixture};
use serde_json::{json, Value};

/** Run a resident command with --format json and return its exit code and envelope
 * Input
    - fixture: &Fixture - repository
    - args: &[&str] - arguments after "crane"
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

/** Write a JSON file next to the working copy (outside the repository)
 * Input
    - fixture: &Fixture - repository
    - name: &str - file name
    - value: &Value - content
 * Output
    - String absolute path
*/
fn file(fixture: &Fixture, name: &str, value: &Value) -> String {
    let path: PathBuf = fixture.work.parent().unwrap().join(name);
    std::fs::write(&path, serde_json::to_string_pretty(value).unwrap()).unwrap();
    path.to_string_lossy().to_string()
}

/** A resident assignment is created, tested without effects, woken by a CI trigger, verified,
 * returned to idle, and stopped by revoking the signed Brick assignment it acts under
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn residents_wake_on_triggers_verify_and_stop_when_authority_is_revoked() {
    let fixture = Fixture::ready();
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: payments\n    description: Payment logic\n    selectors:\n      files: [app/payments.py]\n    autonomy_ceiling: delegated\n",
    );
    fixture.ok(&["governance", "sign"]);
    fixture.ok(&["brick", "assign", "claude", "--targets", "zone:payments"]);

    let definition = file(
        &fixture,
        "definition.json",
        &json!({
            "definition_id": "ci-watch",
            "name": "Payments CI watch",
            "purpose": "Respond to failed CI runs of the payments zone.",
            "owner": "payments",
            "triggers": ["ci", "webhook"],
            "actions": [{"step_type": "READ_CHECK", "auto": true}, {"step_type": "VERIFY", "auto": true}, {"step_type": "ASK_USER"}, {"step_type": "PROPOSE_RCA", "auto": true}, {"step_type": "NOTIFY_SLACK"}, {"step_type": "CREATE_TICKET"}],
            "limits": {"cooldown_minutes": 0}
        }),
    );
    let (code, saved) = envelope(
        &fixture,
        &["resident", "definition", "save", "--file", &definition],
    );
    assert_eq!(code, Some(0), "{saved}");
    assert_eq!(saved["refs"]["definition_ref"], "ci-watch@1");
    let (code, activated) = envelope(
        &fixture,
        &[
            "resident",
            "definition",
            "activate",
            "ci-watch@1",
            "--reason",
            "reviewed",
        ],
    );
    assert_eq!(code, Some(0), "{activated}");
    assert_eq!(activated["data"]["self_reviewed"], true);

    let graph = file(
        &fixture,
        "graph.json",
        &json!({
            "rca_id": "payments-ci",
            "name": "Payments CI failures",
            "target": "zone:payments",
            "nodes": [
                {"id": "failed", "kind": "signal", "label": "CI failed", "status": "observed", "matches": {"source": "ci", "kinds": ["ci.failed"]}},
                {"id": "flaky", "kind": "cause", "label": "flaky payment test", "status": "hypothesized"},
                {"id": "tests-job", "kind": "check", "label": "the tests job failed", "check": {"kind": "trigger_attribute", "key": "job", "equals": "tests"}},
                {"id": "look", "kind": "remediation", "label": "read the payment module", "steps": [{"step_id": "read", "step_type": "READ_CHECK", "check": {"kind": "file_exists", "path": "app/payments.py"}}]},
                {"id": "state", "kind": "verification", "label": "the signed state verifies", "check": {"kind": "signed_state_verifies"}}
            ],
            "edges": [
                {"from": "failed", "to": "flaky", "relation": "may_cause"},
                {"from": "flaky", "to": "tests-job", "relation": "confirmed_by"},
                {"from": "flaky", "to": "look", "relation": "mitigated_by"},
                {"from": "look", "to": "state", "relation": "verified_by"}
            ]
        }),
    );
    let (code, rca) = envelope(&fixture, &["resident", "rca", "save", "--file", &graph]);
    assert_eq!(code, Some(0), "{rca}");
    let (code, reviewed) = envelope(
        &fixture,
        &[
            "resident",
            "rca",
            "review",
            "payments-ci@1",
            "--accept",
            "--reason",
            "matches incidents",
        ],
    );
    assert_eq!(code, Some(0), "{reviewed}");

    let assignment = file(
        &fixture,
        "assignment.json",
        &json!({
            "assignment_id": "payments-ci",
            "definition_id": "ci-watch",
            "definition_version": 1,
            "agent": "claude",
            "target": "zone:payments",
            "rca": {"rca_id": "payments-ci", "version": 1},
            "subscriptions": [{"source": "ci", "kinds": ["ci.*"]}],
            "lifetime_days": 7
        }),
    );
    let (code, preview) = envelope(
        &fixture,
        &["resident", "create", "--file", &assignment, "--dry-run"],
    );
    assert_eq!(code, Some(0), "{preview}");
    let (_, listed) = envelope(&fixture, &["resident", "list"]);
    assert!(
        listed["data"]["assignments"].as_array().unwrap().is_empty(),
        "a dry run saves nothing"
    );
    let (code, created) = envelope(&fixture, &["resident", "create", "--file", &assignment]);
    assert_eq!(code, Some(0), "{created}");
    assert!(
        text(&fixture.run(&["resident", "activate", "payments-ci", "--reason", "go"]))
            .contains("validated"),
        "a draft is not activated without validation"
    );
    let (code, validated) = envelope(&fixture, &["resident", "validate", "payments-ci"]);
    assert_eq!(code, Some(0), "{validated}");
    assert!(
        validated["data"]["validation"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning[0] == "CONNECTOR_NOT_CONFIGURED"),
        "Slack is not configured: {validated}"
    );
    assert!(
        validated["data"]["validation"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|warning| warning[1]
                .as_str()
                .unwrap_or_default()
                .contains("jira is not configured")),
        "CREATE_TICKET needs Jira: {validated}"
    );
    let (code, active) = envelope(
        &fixture,
        &[
            "resident",
            "activate",
            "payments-ci",
            "--reason",
            "on call this week",
        ],
    );
    assert_eq!(code, Some(0), "{active}");

    // Only people run Crane
    assert!(!fixture.as_agent(&["resident", "list"]).status.success());

    // A plan-only test executes nothing and creates no activation
    let event = file(
        &fixture,
        "event.json",
        &json!({"source": "ci", "kind": "ci.failed", "targets": ["zone:payments"], "attributes": {"job": "tests"}, "idempotency_key": "run-41"}),
    );
    let (code, tested) = envelope(
        &fixture,
        &["resident", "test", "payments-ci", "--event", &event],
    );
    assert_eq!(code, Some(0), "{tested}");
    assert_eq!(tested["data"]["result"], "PASSED");
    let (_, activations) = envelope(&fixture, &["resident", "activations"]);
    assert!(activations["data"]["activations"]
        .as_array()
        .unwrap()
        .is_empty());

    // A delivered trigger runs to a verified outcome; a redelivery is dropped
    let (code, delivered) = envelope(&fixture, &["resident", "trigger", "--file", &event]);
    assert_eq!(code, Some(0), "{delivered}");
    let id = delivered["data"]["tick"]["ingested"]["queued"][0]
        .as_str()
        .unwrap()
        .to_string();
    let (code, shown) = envelope(&fixture, &["resident", "activation", &id]);
    assert_eq!(code, Some(0), "{shown}");
    assert_eq!(shown["data"]["state"], "COMPLETED");
    assert_eq!(shown["data"]["outcome"]["status"], "PROVEN");
    assert_eq!(shown["data"]["usage"]["model_calls"], 0);
    assert_eq!(shown["data"]["investigation"]["conclusion"], "IDENTIFIED");
    let (_, again) = envelope(&fixture, &["resident", "trigger", "--file", &event]);
    assert_eq!(again["data"]["tick"]["ingested"]["duplicates"], 1);
    let (_, listed) = envelope(&fixture, &["resident", "list"]);
    assert_eq!(listed["data"]["assignments"][0]["status"], "ACTIVE_IDLE");
    assert_eq!(listed["data"]["assignments"][0]["activations"], 1);

    // A tick reads the event store and timers without a model
    let (code, ticked) = envelope(&fixture, &["resident", "tick"]);
    assert_eq!(code, Some(0), "{ticked}");
    assert_eq!(ticked["data"]["tick"]["model_calls"], 0);
    let (_, status) = envelope(&fixture, &["resident", "status"]);
    assert_eq!(status["data"]["health"]["idle_model_calls"], 0);

    // Revoking the signed Brick assignment degrades it and blocks the next activation
    fixture.ok(&["brick", "unassign", "claude", "--reason", "rotation ended"]);
    let (_, listed) = envelope(&fixture, &["resident", "list"]);
    assert_eq!(listed["data"]["assignments"][0]["status"], "DEGRADED");
    let event = file(
        &fixture,
        "event2.json",
        &json!({"source": "ci", "kind": "ci.failed", "targets": ["zone:payments"], "attributes": {"job": "tests"}, "idempotency_key": "run-42"}),
    );
    let (_, delivered) = envelope(&fixture, &["resident", "trigger", "--file", &event]);
    let id = delivered["data"]["tick"]["ingested"]["queued"][0]
        .as_str()
        .unwrap()
        .to_string();
    let (code, blocked) = envelope(&fixture, &["resident", "activation", &id]);
    assert_eq!(code, Some(3), "{blocked}");
    assert_eq!(blocked["data"]["state"], "BLOCKED");
    assert!(blocked["data"]["outcome"]["reason"]
        .as_str()
        .unwrap()
        .contains("NO_SIGNED_ASSIGNMENT"));

    // Malformed and foreign-source deliveries are refused
    let bad = file(
        &fixture,
        "bad.json",
        &json!({"source": "governance_change", "kind": "governance.signed", "idempotency_key": "x"}),
    );
    assert!(text(&fixture.run(&["resident", "trigger", "--file", &bad])).contains("rejected"));

    // A recorded Crane event can be replayed as a plan-only test
    let (code, replayed) = envelope(
        &fixture,
        &["resident", "test", "payments-ci", "--replay-event", "1"],
    );
    assert!(matches!(code, Some(0 | 3 | 5)), "{replayed}");
    assert!(
        replayed["data"]["mode"]
            .as_str()
            .unwrap()
            .contains("nothing was executed"),
        "{replayed}"
    );

    // Duplicating makes a draft with the same definition, triggers, and graph
    let (code, copied) = envelope(
        &fixture,
        &[
            "resident",
            "duplicate",
            "payments-ci",
            "--as",
            "payments-ci-2",
        ],
    );
    assert_eq!(code, Some(0), "{copied}");
    assert_eq!(copied["data"]["assignment"]["lifecycle"], "draft");
    assert_eq!(copied["data"]["assignment"]["rca"]["rca_id"], "payments-ci");
}
