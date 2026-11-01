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

/** A Brick is destroyed only once nothing is connected to it: an attached Brick is refused, a
 * detached one leaves the store for good (the audit trail keeps it), a draft goes without signing,
 * and an agent is refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn only_a_brick_nothing_is_connected_to_can_be_destroyed() {
    let fixture = Fixture::ready();
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: payments\n    description: Payment logic\n    selectors:\n      files: [app/payments.py]\n    autonomy_ceiling: delegated\n    status: active\n",
    );
    fixture.ok(&["governance", "sign"]);
    fixture.ok(&[
        "brick",
        "create",
        "--type",
        "limits",
        "--name",
        "Guard",
        "--intent",
        "Slow down",
        "--config",
        "{\"max_ceiling\": \"observe\"}",
    ]);
    fixture.ok(&["brick", "activate", "guard", "--reason", "needed"]);
    let (code, attached) = envelope(
        &fixture,
        &["brick", "attach", "guard@1", "--target", "zone:payments"],
    );
    assert_eq!(code, Some(0), "{attached}");
    let attachment = fixture.crane_json("bricks.json")["attachments"][0]["attachment_id"]
        .as_str()
        .unwrap()
        .to_string();

    let (_, refused) = envelope(&fixture, &["brick", "destroy", "guard", "--reason", "done"]);
    assert_eq!(refused["status"], "denied", "{refused}");
    assert!(refused["data"]["connected"][0]
        .as_str()
        .unwrap()
        .contains("zone:payments"));
    assert!(fixture.read(".crane/bricks.json").contains("\"guard\""));

    assert!(
        text(&fixture.as_agent(&["brick", "destroy", "guard", "--reason", "done"]))
            .contains("cannot be executed by an AI agent")
    );
    fixture.ok(&["brick", "detach", &attachment, "--reason", "done with it"]);
    let (code, preview) = envelope(
        &fixture,
        &["brick", "destroy", "guard", "--reason", "done", "--dry-run"],
    );
    assert_eq!(code, Some(0), "{preview}");
    assert!(fixture.read(".crane/bricks.json").contains("\"guard\""));

    let (code, destroyed) = envelope(&fixture, &["brick", "destroy", "guard", "--reason", "done"]);
    assert_eq!(code, Some(0), "{destroyed}");
    assert!(!fixture.read(".crane/bricks.json").contains("\"guard\""));
    fixture.ok(&["validate"]);
    assert!(fixture
        .read(".crane/audit.jsonl")
        .contains("brick.destroyed"));
    assert!(fixture
        .fails(&["brick", "destroy", "guard", "--reason", "again"])
        .contains("not a Brick"));

    fixture.ok(&[
        "brick",
        "create",
        "--type",
        "limits",
        "--name",
        "Sketch",
        "--intent",
        "Try",
        "--config",
        "{\"max_ceiling\": \"observe\"}",
    ]);
    fixture.ok(&["brick", "destroy", "sketch", "--reason", "not needed"]);
    assert!(!fixture
        .work
        .join(".crane/drafts/bricks/sketch.json")
        .exists());
}

/** Testing an unsigned draft runs nothing until a person answers with the token of exactly what it
 * would run; a changed draft needs a new review; a shell never runs, with or without a token; and
 * once signed, the check runs like any signed Brick's
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn a_drafts_commands_run_only_after_review() {
    let fixture = Fixture::ready();
    fixture.ok(&["governance", "sign"]);
    let check = |folder: &str| {
        json!({"checks": [{"kind": "command_succeeds", "program": "git", "args": ["init", "-q", folder]}]}).to_string()
    };
    fixture.ok(&[
        "brick",
        "create",
        "--type",
        "verification",
        "--name",
        "Proof",
        "--intent",
        "Prove it",
        "--config",
        &check("first"),
    ]);
    let ran = |folder: &str| fixture.work.join(folder).exists();

    // Nothing runs; the exact command and a token come back
    let (_, tested) = envelope(&fixture, &["brick", "test", "proof"]);
    assert!(!ran("first"), "{tested}");
    let review = &tested["data"]["needs_review"];
    assert_eq!(review["checks"][0]["program"], "git", "{tested}");
    let token = review["token"].as_str().unwrap().to_string();
    assert!(tested.to_string().contains("git init -q first"), "{tested}");

    // A wrong token runs nothing
    let wrong = fixture.run(&[
        "brick",
        "test",
        "proof",
        "--allow-commands",
        "0000000000000000",
    ]);
    assert!(!wrong.status.success());
    assert!(text(&wrong).contains("does not match"), "{}", text(&wrong));
    assert!(!ran("first"));

    // Changing what it runs voids the token
    fixture.ok(&["brick", "edit", "proof", "--config", &check("second")]);
    let stale = fixture.run(&["brick", "test", "proof", "--allow-commands", &token]);
    assert!(!stale.status.success(), "{}", text(&stale));
    assert!(!ran("second"));

    // The token of what the person saw runs it
    let (_, tested) = envelope(&fixture, &["brick", "test", "proof"]);
    let token = tested["data"]["needs_review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let (_, answered) = envelope(
        &fixture,
        &["brick", "test", "proof", "--allow-commands", &token],
    );
    assert!(ran("second"), "{answered}");
    assert!(answered["data"]["needs_review"].is_null(), "{answered}");

    // A shell in a draft (a request or an imported library can supply one) never runs
    fixture.ok(&[
        "brick", "create", "--type", "verification", "--name", "Shell", "--intent", "Try",
        "--config",
        &json!({"checks": [{"kind": "command_succeeds", "program": "sh", "args": ["-c", "touch shell-ran"]}]}).to_string(),
    ]);
    let (_, tested) = envelope(&fixture, &["brick", "test", "shell"]);
    assert!(
        tested.to_string().contains("CHECK_PROGRAM_INVALID"),
        "{tested}"
    );
    let token = tested["data"]["needs_review"]["token"]
        .as_str()
        .unwrap()
        .to_string();
    let refused = fixture.run(&["brick", "test", "shell", "--allow-commands", &token]);
    assert!(!refused.status.success());
    assert!(
        text(&refused).contains("does not validate"),
        "{}",
        text(&refused)
    );
    assert!(!ran("shell-ran"));

    // A library cannot get around it: imported, it is a draft like any other
    fixture.write(
        "library.json",
        &json!({"format": "crane-brick-library/v1", "bricks": [
            {"ref": "lib@1", "state": "active", "type": "verification", "name": "Lib", "intent": "Prove",
             "config": {"checks": [{"kind": "command_succeeds", "program": "git", "args": ["init", "-q", "imported"]}]},
             "common_context": {"text": "", "sensitivity": "internal"}, "dependencies": []}
        ]}).to_string(),
    );
    fixture.ok(&["brick", "import", "library.json"]);
    let (_, tested) = envelope(&fixture, &["brick", "test", "lib"]);
    assert!(!ran("imported"), "{tested}");
    assert!(
        tested["data"]["needs_review"]["token"].is_string(),
        "{tested}"
    );

    // Signed by an admin, it runs without a token
    fixture.ok(&["brick", "edit", "proof", "--config", &check("third")]);
    fixture.ok(&["brick", "activate", "proof", "--reason", "reviewed"]);
    let (_, signed) = envelope(&fixture, &["brick", "test", "proof@1"]);
    assert!(ran("third"), "{signed}");
    assert!(signed["data"]["needs_review"].is_null(), "{signed}");
}

/** The agent hook sees the checks too: a dump, a respelled push, and a computed path are decided
 * through the real binary, and a new session does not reset a daily cap
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn the_hook_enforces_the_new_checks() {
    let fixture = Fixture::ready();
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: payments\n    description: Payment logic\n    selectors:\n      files: [app/payments.py, app/secret.txt]\n    autonomy_ceiling: delegated\n",
    );
    fixture.write("app/payments.py", "x = 1\n");
    fixture.write("app/secret.txt", "s\n");
    fixture.ok(&["governance", "sign"]);
    for (name, kind, config) in [
        ("Keep", "access", json!({"deny_read": ["app/secret.txt"]})),
        ("Hide", "environment", json!({"hide_env": ["AWS_*"]})),
        (
            "Ops",
            "action",
            json!({"operations": [{"adapter": "git", "operation": "git.push"}], "deny_direct": true}),
        ),
    ] {
        fixture.ok(&[
            "brick",
            "create",
            "--type",
            kind,
            "--name",
            name,
            "--intent",
            "Guard",
            "--config",
            &config.to_string(),
        ]);
        let id = name.to_ascii_lowercase();
        fixture.ok(&["brick", "activate", &id, "--reason", "needed"]);
        fixture.ok(&[
            "brick",
            "attach",
            &format!("{id}@1"),
            "--target",
            "zone:payments",
        ]);
    }
    let run = |session: &str, call: &str, command: &str| {
        fixture.hook(
            "claude",
            "pre-tool-use",
            &json!({"session_id": session, "tool_use_id": call, "cwd": fixture.work.to_string_lossy(), "tool_name": "Bash", "tool_input": {"command": command}}),
        )
    };
    let denied = run("h1", "c1", "git -C . -c a=b push");
    assert_eq!(denied.status.code(), Some(2), "{}", text(&denied));
    assert!(text(&denied).contains("git.push"), "{}", text(&denied));
    let glob = run("h1", "c2", "cat app/sec*");
    assert_eq!(glob.status.code(), Some(2), "{}", text(&glob));
    assert!(text(&glob).contains("may not be read"), "{}", text(&glob));
    // The session has worked in the target, so hidden variables apply
    let reading = fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": "h1", "tool_use_id": "c3", "cwd": fixture.work.to_string_lossy(), "tool_name": "Read", "tool_input": {"file_path": fixture.work.join("app/payments.py").to_string_lossy()}}),
    );
    assert!(reading.status.success(), "{}", text(&reading));
    let dump = run("h1", "c4", "env | cat");
    assert_eq!(dump.status.code(), Some(2), "{}", text(&dump));
    assert!(text(&dump).contains("whole environment"), "{}", text(&dump));
    let computed = run("h1", "c5", "cat $(echo app/secret.txt)");
    let answer: Value = serde_json::from_slice(&computed.stdout).unwrap_or(Value::Null);
    assert!(
        computed.status.code() == Some(2)
            || answer["hookSpecificOutput"]["permissionDecision"] == "ask",
        "{}",
        text(&computed)
    );

    // A daily cap: two tool calls today, whichever session makes them
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: billing\n    description: Billing\n    selectors:\n      files: [billing/a.py]\n    autonomy_ceiling: delegated\n",
    );
    fixture.write("billing/a.py", "x = 1\n");
    fixture.ok(&["governance", "sign"]);
    fixture.ok(&[
        "brick",
        "create",
        "--type",
        "limits",
        "--name",
        "Daily",
        "--intent",
        "Pace",
        "--config",
        "{\"max_tool_calls_per_day\": 2}",
    ]);
    fixture.ok(&["brick", "activate", "daily", "--reason", "needed"]);
    fixture.ok(&["brick", "attach", "daily@1", "--target", "zone:billing"]);
    let read = |session: &str, call: &str| {
        fixture.hook(
            "claude",
            "pre-tool-use",
            &json!({"session_id": session, "tool_use_id": call, "cwd": fixture.work.to_string_lossy(), "tool_name": "Read", "tool_input": {"file_path": fixture.work.join("billing/a.py").to_string_lossy()}}),
        )
    };
    assert!(read("d1", "r1").status.success());
    assert!(read("d1", "r2").status.success());
    let third = read("d2", "r3");
    assert_eq!(third.status.code(), Some(2), "{}", text(&third));
    assert!(
        text(&third).contains("a day across sessions"),
        "{}",
        text(&third)
    );
}
