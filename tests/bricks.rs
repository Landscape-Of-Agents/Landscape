// Bricks through the real binary: the fixed types, a draft that grants nothing, activation and
// attachment signed into .crane/bricks.json (with a dry-run preview that writes nothing), the
// guard enforcing the Brick at the hook, its context delivered once per session, tampering
// detected, revocation failing closed instead of loosening, and the agent gate.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** Run a brick command with --format json and return its exit code and envelope
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

/** Ask the hook about a Claude Code tool call
 * Input
    - fixture: &Fixture - repository
    - call: &str - tool_use_id
    - tool: &str - Write or Read
    - path: &str - file
 * Output
    - std::process::Output
*/
fn tool(fixture: &Fixture, call: &str, tool: &str, path: &str) -> std::process::Output {
    let file = fixture.work.join(path).to_string_lossy().to_string();
    let input = if tool == "Write" {
        json!({"file_path": file, "content": "x = 1\n"})
    } else {
        json!({"file_path": file})
    };
    fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": "b1", "tool_use_id": call, "cwd": fixture.work.to_string_lossy(), "tool_name": tool, "tool_input": input}),
    )
}

/** A Brick goes from draft to enforcement and context, and back out only through signed changes
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn bricks_are_drafted_activated_attached_enforced_and_revoked() {
    let fixture = Fixture::ready();
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: payments\n    description: Payment logic\n    selectors:\n      files: [app/payments.py]\n    autonomy_ceiling: delegated\n",
    );
    fixture.ok(&["governance", "sign"]);

    let (code, types) = envelope(&fixture, &["brick", "types"]);
    assert_eq!(code, Some(0), "{types}");
    assert_eq!(types["data"]["types"].as_array().unwrap().len(), 12);
    // A former kind can no longer be created; the error names the kind that replaced it
    assert!(
        text(&fixture.run(&["brick", "create", "--type", "constraint", "--name", "x"]))
            .contains("use limits and access instead")
    );
    assert_eq!(types["data"]["custom_types"], false);
    assert!(text(&fixture.run(&[
        "brick",
        "create",
        "--type",
        "data_protection",
        "--name",
        "x"
    ]))
    .contains("the set is fixed"));

    // A draft grants nothing and lives outside the signed store
    let (code, draft) = envelope(
        &fixture,
        &[
            "brick",
            "create",
            "--type",
            "limits",
            "--name",
            "Payments guard",
            "--intent",
            "Agents must not change payment code on their own",
            "--config",
            "{\"max_ceiling\": \"observe\"}",
            "--context",
            "Payment changes go through the payments team.",
        ],
    );
    assert_eq!(code, Some(0), "{draft}");
    assert_eq!(draft["refs"]["brick_ref"], "payments-guard@1");
    assert_eq!(draft["data"]["validation"]["status"], "passed");
    assert!(fixture
        .work
        .join(".crane/drafts/bricks/payments-guard.json")
        .is_file());
    assert!(!fixture.work.join(".crane/bricks.json").exists());
    assert!(
        fixture.ok(&["validate"]).contains("PASS"),
        "drafts do not disturb integrity"
    );

    // The single signer activates it (recorded as self-reviewed); the draft is consumed
    let (code, activated) = envelope(
        &fixture,
        &[
            "brick",
            "activate",
            "payments-guard",
            "--reason",
            "reviewed",
        ],
    );
    assert_eq!(code, Some(0), "{activated}");
    assert_eq!(activated["data"]["self_reviewed"], true);
    assert!(!fixture
        .work
        .join(".crane/drafts/bricks/payments-guard.json")
        .exists());
    assert!(
        fixture.ok(&["validate"]).contains("PASS"),
        "the store is covered by the manifest"
    );

    // A dry run previews exactly what the guard will restrict and signs nothing
    let store = fixture.read(".crane/bricks.json");
    let (code, preview) = envelope(
        &fixture,
        &[
            "brick",
            "attach",
            "payments-guard@1",
            "--target",
            "zone:payments",
            "--context",
            "Use the v2 gateway client.",
            "--dry-run",
        ],
    );
    assert_eq!(code, Some(0), "{preview}");
    let after = &preview["data"]["preview"]["after"]["files"];
    assert_eq!(after[0]["path"], "app/payments.py", "{preview}");
    assert_eq!(after[0]["ceiling"], "observe");
    assert_eq!(
        fixture.read(".crane/bricks.json"),
        store,
        "a dry run writes nothing"
    );
    let (code, attached) = envelope(
        &fixture,
        &[
            "brick",
            "attach",
            "payments-guard@1",
            "--target",
            "zone:payments",
            "--context",
            "Use the v2 gateway client.",
        ],
    );
    assert_eq!(code, Some(0), "{attached}");
    let unknown = fixture.run(&[
        "brick",
        "attach",
        "payments-guard@1",
        "--target",
        "zone:nowhere",
    ]);
    assert!(
        text(&unknown).contains("not a Zone or Flow"),
        "{}",
        text(&unknown)
    );

    // The agent learns about the Brick at session start; the guard enforces it before the write
    let started = fixture.hook(
        "claude",
        "SessionStart",
        &json!({"session_id": "b1", "source": "startup", "cwd": fixture.work.to_string_lossy()}),
    );
    assert!(
        text(&started).contains("zone:payments: brick:payments-guard@1"),
        "{}",
        text(&started)
    );
    let denied = tool(&fixture, "w1", "Write", "app/payments.py");
    assert_eq!(denied.status.code(), Some(2), "{}", text(&denied));
    assert!(
        text(&denied).contains("brick:payments-guard@1"),
        "{}",
        text(&denied)
    );
    let other = tool(&fixture, "w2", "Write", "web/app.js");
    assert!(
        other.status.success(),
        "files outside the target are untouched: {}",
        text(&other)
    );

    // Context arrives with the first touching call, then a reference while unchanged
    let first: Value =
        serde_json::from_slice(&tool(&fixture, "r1", "Read", "app/payments.py").stdout).unwrap();
    let context = first["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("Payment changes go through the payments team."),
        "{context}"
    );
    assert!(context.contains("Use the v2 gateway client."), "{context}");
    assert!(
        context.contains("autonomy ceiling at most observe"),
        "{context}"
    );
    let second: Value =
        serde_json::from_slice(&tool(&fixture, "r2", "Read", "app/payments.py").stdout).unwrap();
    let context = second["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(
        context.contains("unchanged since they were delivered"),
        "{context}"
    );
    assert!(
        !context.contains("Payment changes go through"),
        "not resent: {context}"
    );
    let (_, status) = envelope(&fixture, &["brick", "context", "status"]);
    assert_eq!(status["data"]["measured"]["reused"], 1, "{status}");

    // The preview shows every source with its digest
    let (code, snapshot) = envelope(
        &fixture,
        &["brick", "context", "preview", "--targets", "zone:payments"],
    );
    assert_eq!(code, Some(0), "{snapshot}");
    let sources = snapshot["data"]["snapshot"]["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|section| section["source"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        sources.contains(&"brick:payments-guard@1".to_string())
            && sources
                .iter()
                .any(|source| source.starts_with("attachment:")),
        "{sources:?}"
    );

    // An unsigned edit of the store fails integrity (and so stops agent changes)
    let original = fixture.read(".crane/bricks.json");
    fixture.write(
        ".crane/bricks.json",
        &original.replace("\"observe\"", "\"autonomous\""),
    );
    let (_, report) = fixture.json(&["validate", "--json"]);
    assert!(
        report.to_string().contains("bricks_store_changed"),
        "{report}"
    );
    fixture.write(".crane/bricks.json", &original);

    // Revoking the version fails closed: changes need approval, they are not silently allowed
    let (code, revoked) = envelope(
        &fixture,
        &[
            "brick",
            "revoke",
            "payments-guard@1",
            "--reason",
            "replaced",
        ],
    );
    assert_eq!(code, Some(0), "{revoked}");
    assert_eq!(
        revoked["data"]["pinned_attachments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let asked = tool(&fixture, "w3", "Write", "app/payments.py");
    assert!(asked.status.success(), "{}", text(&asked));
    let answer: Value = serde_json::from_slice(&asked.stdout).unwrap();
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecision"], "ask",
        "{answer}"
    );

    // History is signed, and agents cannot run Brick commands
    let (_, history) = envelope(&fixture, &["brick", "history"]);
    let actions = history["data"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["action"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(
        actions,
        ["brick.activated", "brick.attached", "brick.revoked"]
    );
    assert!(
        text(&fixture.as_agent(&["brick", "list"])).contains("cannot be executed by an AI agent")
    );
}

/** A downloaded library is imported as drafts that grant nothing (all or nothing), and a mock
 * call is decided by an attached Brick without running or recording anything
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn libraries_import_as_drafts_and_mock_calls_are_decided() {
    let fixture = Fixture::ready();
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: payments\n    description: Payment logic\n    selectors:\n      files: [app/payments.py]\n    autonomy_ceiling: delegated\n",
    );
    fixture.write("app/payments.py", "x = 1\n");
    fixture.ok(&["governance", "sign"]);
    let library = json!({
        "format": "crane-brick-library/v1",
        "bricks": [
            {"ref": "no-deletes@3", "state": "active", "type": "access", "name": "No deletes", "intent": "Agents never delete payment code",
             "config": {"delete": "deny"}, "common_context": {"text": "Deleting needs the payments team.", "sensitivity": "internal"}, "dependencies": []},
            {"ref": "push-review@1", "state": "active", "type": "approval", "name": "Push review", "intent": "A person approves pushes",
             "config": {"operations": ["git_push"], "reason": "release owners decide"}, "common_context": {"text": "", "sensitivity": "internal"}, "dependencies": []}
        ]
    });
    fixture.write("library.json", &library.to_string());
    let (code, imported) = envelope(&fixture, &["brick", "import", "library.json"]);
    assert_eq!(code, Some(0), "{imported}");
    let drafts = imported["data"]["drafts"].as_array().unwrap();
    assert_eq!(drafts.len(), 2);
    assert_eq!(
        drafts[0]["brick_ref"], "no-deletes@1",
        "versions start over here"
    );
    assert!(fixture
        .work
        .join(".crane/drafts/bricks/no-deletes.json")
        .is_file());
    assert!(
        !fixture.work.join(".crane/bricks.json").exists(),
        "an import signs nothing"
    );
    // Importing again is refused before anything is saved
    let again = fixture.run(&["brick", "import", "library.json"]);
    assert!(!again.status.success());
    assert!(text(&again).contains("already exists"), "{}", text(&again));

    // A mock call: the draft is tested as if attached; deleting is refused, a small change is not
    let (code, deleted) = envelope(
        &fixture,
        &[
            "brick",
            "test",
            "no-deletes",
            "--target",
            "zone:payments",
            "--call",
            "{\"files\": [\"app/payments.py\"], \"delete\": true}",
        ],
    );
    assert_eq!(code, Some(0), "{deleted}");
    assert_eq!(deleted["data"]["trial"]["verdict"], "deny", "{deleted}");
    assert_eq!(
        deleted["data"]["trial"]["findings"][0]["rule"],
        "BRICK_ACCESS_MODE"
    );
    let (_, changed) = envelope(
        &fixture,
        &[
            "brick",
            "test",
            "no-deletes",
            "--target",
            "zone:payments",
            "--call",
            "{\"files\": [\"app/payments.py\"], \"lines\": 2}",
        ],
    );
    assert_eq!(changed["data"]["trial"]["verdict"], "allow", "{changed}");
    let malformed = fixture.run(&[
        "brick",
        "test",
        "no-deletes",
        "--target",
        "zone:payments",
        "--call",
        "{\"files\": [\"../outside\"]}",
    ]);
    assert!(!malformed.status.success());
    assert!(
        text(&malformed).contains("not a file inside the repository"),
        "{}",
        text(&malformed)
    );
    assert_eq!(
        std::fs::read_to_string(fixture.work.join("app/payments.py")).unwrap(),
        "x = 1\n"
    );
}
