// Flows as a knowledge base, through the real binary: a person traces a flow from a starting point
// into a draft, retraces and rewinds it, and approves it (the flow is added to governance.yaml and
// the trace signed into .crane/flows.json); knowledge is added (priority, context, people, models);
// an agent asks through its hooks with 'crane-request' (answered, never run, never a denial),
// proposes a test that a person approves, starts watchers up to the signed limit, and a tampered
// knowledge file fails verification.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** Run a flow command with --format json
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

/** Ask Crane through the agent's hook, as Claude Code would run a Bash command
 * Input
    - fixture: &Fixture - repository
    - call: &str - tool_use_id
    - command: &str - the shell command
 * Output
    - (Option<i32>, String) the hook's exit code (2: the command is stopped) and its answer
*/
fn ask(fixture: &Fixture, call: &str, command: &str) -> (Option<i32>, String) {
    let output = fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": "f1", "tool_use_id": call, "cwd": fixture.work.to_string_lossy(), "tool_name": "Bash", "tool_input": {"command": command}}),
    );
    (output.status.code(), text(&output))
}

/** A flow goes from a traced draft to signed knowledge, and agents work with it only through drafts,
 * proposals, and read-only watchers
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn flows_are_traced_approved_and_shared_with_agents() {
    let fixture = Fixture::ready();
    fixture.write(
        "app/pay.py",
        "def submit(order):\n    audit(order)\n    return charge(order)\n\ndef charge(order):\n    return send(order)\n",
    );
    fixture.write("app/net.py", "def send(order):\n    return order\n");
    fixture.write("app/log.py", "def audit(order):\n    return None\n");
    fixture.write("tests/test_pay.py", "def test_charge():\n    assert True\n");
    fixture.write(".crane/governance.yaml", "version: 1\n# Payments are critical\nzones:\n  - id: payments\n    description: Payment logic\n    selectors:\n      paths: [\"app/**\"]\n");
    fixture.commit("payment code");
    fixture.ok(&["governance", "sign"]);

    // Trace from a starting point into a draft; nothing is signed yet
    let (code, traced) = envelope(
        &fixture,
        &[
            "flow",
            "trace",
            "--from",
            "app/pay.py:submit",
            "--flow",
            "checkout",
            "--description",
            "Pay for an order",
        ],
    );
    assert_eq!(code, Some(0), "{traced}");
    assert_eq!(
        traced["data"]["revision"]["graph"]["nodes"]
            .as_array()
            .unwrap()
            .len(),
        4,
        "{traced}"
    );
    assert!(fixture
        .work
        .join(".crane/drafts/flows/checkout.json")
        .is_file());
    assert!(
        !fixture.work.join(".crane/flows.json").exists(),
        "a draft signs nothing"
    );

    // The code changes: retrace adds a revision; rewind goes back without deleting it
    fixture.write(
        "app/net.py",
        "def send(order):\n    return retry(order)\n\ndef retry(order):\n    return order\n",
    );
    let (_, retraced) = envelope(
        &fixture,
        &[
            "flow",
            "retrace",
            "checkout",
            "--reason",
            "send retries now",
        ],
    );
    assert_eq!(retraced["data"]["revision"]["revision"], 2, "{retraced}");
    assert_eq!(
        retraced["data"]["diff"]["added_nodes"],
        json!(["app/net.py:retry"])
    );
    let (code, _) = envelope(&fixture, &["flow", "rewind", "checkout", "--to", "1"]);
    assert_eq!(code, Some(0));
    let (_, shown) = envelope(&fixture, &["flow", "draft", "checkout"]);
    assert_eq!(shown["data"]["draft"]["current"], 1);
    assert_eq!(
        shown["data"]["draft"]["revisions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    envelope(&fixture, &["flow", "rewind", "checkout", "--to", "2"]);

    // Approving adds the flow to governance.yaml (comments kept) and signs the trace
    let (code, approved) = envelope(
        &fixture,
        &[
            "flow",
            "approve",
            "checkout",
            "--reason",
            "reviewed the call graph",
        ],
    );
    assert_eq!(code, Some(0), "{approved}");
    assert_eq!(approved["data"]["added"], true);
    let governance = fixture.read(".crane/governance.yaml");
    assert!(
        governance.contains("# Payments are critical") && governance.contains("app/pay.py:submit"),
        "{governance}"
    );
    assert!(!fixture
        .work
        .join(".crane/drafts/flows/checkout.json")
        .exists());
    assert!(
        fixture.ok(&["validate"]).contains("PASS"),
        "the signed state verifies"
    );

    // Knowledge: priority, context, a person, the organization's models
    for args in [
        vec![
            "flow",
            "models",
            "add",
            "--provider",
            "claude",
            "--model",
            "claude-opus-5-5",
            "--tier",
            "frontier",
        ],
        vec!["flow", "priority", "checkout", "P0"],
        vec![
            "flow",
            "context",
            "add",
            "checkout",
            "--kind",
            "latency",
            "--title",
            "Charge budget",
            "--value",
            "250",
            "--threshold",
            "300",
        ],
        vec![
            "flow",
            "context",
            "add",
            "checkout",
            "--kind",
            "query",
            "--title",
            "Failed charges",
            "--value",
            "select * from charges where status = 'failed'",
        ],
        vec![
            "flow", "person", "add", "checkout", "--name", "Ana", "--role", "owner", "--github",
            "ana", "--jira", "PAY-12",
        ],
    ] {
        let (code, answer) = envelope(&fixture, &args);
        assert_eq!(code, Some(0), "{args:?}: {answer}");
    }
    let (_, routed) = envelope(&fixture, &["flow", "models", "route", "checkout"]);
    assert_eq!(
        routed["data"]["model"]["model"], "claude-opus-5-5",
        "{routed}"
    );
    assert!(fixture
        .fails(&[
            "flow",
            "context",
            "add",
            "checkout",
            "--kind",
            "note",
            "--title",
            "Key",
            "--value",
            "AKIAABCDEFGHIJKLMNOP"
        ])
        .contains("secret"));

    // The agent asks through its hook: answered, never run, not a denial or a bypass
    let (code, reason) = ask(&fixture, "c1", "crane-request flow checkout");
    assert_eq!(code, Some(2), "the command is stopped: {reason}");
    assert!(
        reason.contains("Crane answered")
            && reason.contains("Priority P0")
            && reason.contains("Charge budget")
            && reason.contains("Ana"),
        "{reason}"
    );
    let (_, refused) = ask(&fixture, "c2", "crane-request flow checkout; rm -rf app");
    assert!(refused.contains("one command"), "{refused}");
    let (_, unknown) = ask(&fixture, "c3", "crane-request flow nothing");
    assert!(unknown.contains("not a flow"), "{unknown}");

    // The agent proposes a test; a person approves it and marks it required
    let (_, proposed) = ask(&fixture, "c4", "crane-request test --flow checkout --file tests/test_pay.py --name 'charges once' --description 'A paid order is charged exactly once' --assumptions 'the gateway answers; amounts are in cents' --mock 'order 42 for 1000 cents'");
    assert!(proposed.contains("waits for a person"), "{proposed}");
    let (_, waiting) = envelope(&fixture, &["flow", "proposals"]);
    let id = waiting["data"]["proposals"][0]["proposal_id"]
        .as_str()
        .unwrap()
        .to_string();
    let (code, decided) = envelope(
        &fixture,
        &["flow", "proposals", "approve", &id, "--required"],
    );
    assert_eq!(code, Some(0), "{decided}");
    let (_, knowledge) = envelope(&fixture, &["flow", "show", "checkout"]);
    let tests = &knowledge["data"]["knowledge"]["tests"];
    assert_eq!(tests[0]["required"], true, "{knowledge}");
    assert_eq!(tests[0]["assumptions"].as_array().unwrap().len(), 2);

    // Watchers: the agent starts them up to the signed limit (two per session by default)
    let (_, first) = ask(
        &fixture,
        "c5",
        "crane-request watch --flow checkout --purpose 'watch the charge path'",
    );
    assert!(first.contains("read-only"), "{first}");
    let (_, second) = ask(
        &fixture,
        "c6",
        "crane-request watch --flow checkout --purpose second --checks trace_drift",
    );
    assert!(second.contains("read-only"), "{second}");
    let (_, third) = ask(
        &fixture,
        "c7",
        "crane-request watch --flow checkout --purpose third",
    );
    assert!(third.contains("limit"), "{third}");
    let (_, watchers) = envelope(&fixture, &["flow", "watch", "list"]);
    assert_eq!(
        watchers["data"]["watchers"].as_array().unwrap().len(),
        2,
        "{watchers} / {first} / {second} / {third}"
    );
    assert_eq!(
        watchers["data"]["watchers"][0]["inherited"]["mode"],
        "read_only"
    );
    // Only a person raises the limit
    let (code, _) = envelope(&fixture, &["flow", "limit", "--session", "3"]);
    assert_eq!(code, Some(0));

    // The approved trace drifts after the code changed: a watcher raises one incident
    fixture.write(
        "app/log.py",
        "def audit(order):\n    return store(order)\n\ndef store(order):\n    return None\n",
    );
    // (a background sync a command started may run the watchers first; either raises it once)
    envelope(&fixture, &["flow", "watch", "run"]);
    let (_, again) = envelope(&fixture, &["flow", "watch", "run"]);
    assert!(
        again["data"]["incidents"].as_array().unwrap().is_empty(),
        "an incident is raised once while it fires"
    );
    let (_, raised) = envelope(&fixture, &["flow", "incidents"]);
    let drift = raised["data"]["incidents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|incident| incident["code"] == "TRACE_DRIFT")
        .count();
    assert_eq!(drift, 1, "{raised}");
    // The parent session hears about it with its next tool call
    let output = fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": "f1", "tool_use_id": "c8", "cwd": fixture.work.to_string_lossy(), "tool_name": "Read", "tool_input": {"file_path": fixture.work.join("app/pay.py").to_string_lossy()}}),
    );
    assert!(
        text(&output).contains("TRACE_DRIFT")
            || text(&output).contains("call graph of checkout changed"),
        "{}",
        text(&output)
    );

    // Agents still may not run Crane itself
    assert!(!fixture.as_agent(&["flow", "list"]).status.success());
    // A changed knowledge file fails verification
    let path = fixture.work.join(".crane/flows.json");
    let tampered = std::fs::read_to_string(&path)
        .unwrap()
        .replace("\"P0\"", "\"P3\"");
    std::fs::write(&path, tampered).unwrap();
    assert!(text(&fixture.run(&["validate"])).contains("flows.json"));
}
