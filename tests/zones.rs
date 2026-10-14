// Zones, Flows, and Context Packs (.crane/governance.yaml): validation, flow discovery, autonomy
// ceilings enforced at the hook, fail-closed handling of invalid configuration, and context packs
// supplied to agents.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** The service module that calls into payments
 * Input
    - None
 * Output
    - &'static str
*/
const SERVICE: &str = "from app.payments import charge\n\n\ndef submit(order):\n    total = charge(order)\n    return audit(total)\n\n\ndef audit(total):\n    return total\n";

/** A fixture with a service module committed and checkpointed
 * Input
    - None
 * Output
    - Fixture
*/
fn fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.write("app/service.py", SERVICE);
    fixture.commit("service");
    common::git(&fixture.work, &["push", "-q", "origin", "HEAD:main"]);
    fixture.ok(&["repo", "--https", fixture.remote_url().as_str()]);
    fixture.ok(&["init"]);
    fixture.ok(&["checkpoint", "baseline"]);
    fixture
}

/** Ask the hook about a Claude Code write
 * Input
    - fixture: &Fixture - repository
    - session: &str - session id
    - call: &str - tool_use_id
    - path: &str - file
    - content: &str - new content
 * Output
    - std::process::Output
*/
fn write(
    fixture: &Fixture,
    session: &str,
    call: &str,
    path: &str,
    content: &str,
) -> std::process::Output {
    fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": session, "tool_use_id": call, "cwd": fixture.work.to_string_lossy(), "tool_name": "Write", "tool_input": {"file_path": fixture.work.join(path).to_string_lossy(), "content": content}}),
    )
}

/** Write .crane/governance.yaml and sign it, as an admin does after changing zones or flows
 * Input
    - fixture: &Fixture - repository
    - yaml: &str - file content
 * Output
    - None (panics if signing fails)
*/
fn govern(fixture: &Fixture, yaml: &str) {
    fixture.write(".crane/governance.yaml", yaml);
    assert!(fixture
        .ok(&["governance", "sign"])
        .contains("Signed .crane/governance.yaml"));
}

/** governance.yaml is signed: an edit nobody signed fails verification and stops agent changes
 * (fail closed) until an admin signs it; agents cannot sign, and broken rules are not signed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn governance_yaml_is_signed() {
    let fixture = fixture();
    let strict = "version: 1
zones:
  - id: ledger-core
    selectors:
      files: [app/ledger.py]
    autonomy_ceiling: observe
";
    govern(&fixture, strict);
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    // Someone loosens the zone without signing: verification fails and agents are stopped
    fixture.write(
        ".crane/governance.yaml",
        &strict.replace("observe", "autonomous"),
    );
    let changed = fixture.fails(&["validate"]);
    assert!(changed.contains("governance_yaml_changed"), "{changed}");
    fixture.hook("claude", "SessionStart", &json!({"session_id": "s1"}));
    let blocked = write(
        &fixture,
        "s1",
        "s-1",
        "app/ledger.py",
        "x = 1
",
    );
    assert_eq!(blocked.status.code(), Some(2), "{}", text(&blocked));
    let elsewhere = write(
        &fixture,
        "s1",
        "s-2",
        "web/app.js",
        "x
",
    );
    assert_eq!(elsewhere.status.code(), Some(2), "{}", text(&elsewhere));
    // An agent can neither run the command nor get it past the hook
    assert!(text(&fixture.as_agent(&["governance", "sign"]))
        .contains("cannot be executed by an AI agent"));
    // A broken file is refused; an admin signs a valid one and enforcement resumes
    fixture.write(
        ".crane/governance.yaml",
        "version: 1
zones:
  - id: z
    policies: [nope]
",
    );
    let refused = fixture.fails(&["governance", "sign"]);
    assert!(refused.contains("not signing"), "{refused}");
    govern(&fixture, &strict.replace("observe", "autonomous"));
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    let allowed = write(
        &fixture,
        "s1",
        "s-3",
        "app/ledger.py",
        "x = 1
",
    );
    assert!(allowed.status.success(), "{}", text(&allowed));
}

/** Zone ceilings restrict agent writes: observe denies, assisted asks, other files are allowed
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn zone_ceilings_restrict_writes() {
    let fixture = fixture();
    govern(&fixture,
        "version: 1\nzones:\n  - id: ledger-core\n    description: Ledger postings\n    selectors:\n      files: [app/ledger.py]\n    criticality: critical\n    autonomy_ceiling: observe\n  - id: payments\n    description: Payment logic\n    selectors:\n      paths: [\"app/pay*.py\"]\n    criticality: sensitive\n    autonomy_ceiling: assisted\n",
    );
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    fixture.hook("claude", "SessionStart", &json!({"session_id": "z1"}));
    let ledger = write(&fixture, "z1", "w1", "app/ledger.py", "x = 1\n");
    assert_eq!(ledger.status.code(), Some(2), "{}", text(&ledger));
    assert!(
        text(&ledger).contains("zone:ledger-core") && text(&ledger).contains("observe"),
        "{}",
        text(&ledger)
    );
    let payments = write(&fixture, "z1", "w2", "app/payments.py", "x = 2\n");
    assert!(payments.status.success(), "{}", text(&payments));
    let answer: Value = serde_json::from_slice(&payments.stdout).unwrap();
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecision"], "ask",
        "{answer}"
    );
    let other = write(&fixture, "z1", "w3", "web/app.js", "x\n");
    assert!(
        other.status.success() && other.stdout.is_empty(),
        "{}",
        text(&other)
    );
    govern(&fixture, "version: 1\nzones:\n  - id: ledger-core\n    status: proposed\n    selectors:\n      files: [app/ledger.py]\n    autonomy_ceiling: observe\n");
    let proposed = write(&fixture, "z1", "w4", "app/ledger.py", "x = 3\n");
    assert!(
        proposed.status.success(),
        "proposed zones are not enforced: {}",
        text(&proposed)
    );
}

#[test]
fn source_marked_zone_and_flow_enforce_only_their_lines() {
    let fixture = fixture();
    fixture.write(
        "app/ledger.py",
        "# @crane:zone:ledger-window:start\ndef post(entry):\n    return entry\n# @crane:zone:ledger-window:end\n",
    );
    fixture.write(
        "web/app.js",
        "export function greet(name) {\n  // @crane:flow:greeting-window:start\n  return `hi ${name}`;\n  // @crane:flow:greeting-window:end\n}\n",
    );
    govern(
        &fixture,
        "version: 1\nzones:\n  - id: ledger-window\n    autonomy_ceiling: observe\nflows:\n  - id: greeting-window\n    entry_points: [\"app/service.py:submit\"]\n    autonomy_ceiling: observe\n",
    );
    assert!(fixture.ok(&["validate"]).contains("Crane validate: PASS"));
    fixture.hook("claude", "SessionStart", &json!({"session_id": "marked"}));

    let ledger_inside = write(
        &fixture,
        "marked",
        "m-1",
        "app/ledger.py",
        "# @crane:zone:ledger-window:start\ndef post(entry):\n    return entry + 1\n# @crane:zone:ledger-window:end\n",
    );
    assert_eq!(
        ledger_inside.status.code(),
        Some(2),
        "{}",
        text(&ledger_inside)
    );
    assert!(text(&ledger_inside).contains("zone:ledger-window"));
    let ledger_outside = write(
        &fixture,
        "marked",
        "m-2",
        "app/ledger.py",
        "# @crane:zone:ledger-window:start\ndef post(entry):\n    return entry\n# @crane:zone:ledger-window:end\n# outside = True\n",
    );
    assert!(ledger_outside.status.success(), "{}", text(&ledger_outside));

    let flow_inside = write(
        &fixture,
        "marked",
        "m-3",
        "web/app.js",
        "export function greet(name) {\n  // @crane:flow:greeting-window:start\n  return `hello ${name}`;\n  // @crane:flow:greeting-window:end\n}\n",
    );
    assert_eq!(flow_inside.status.code(), Some(2), "{}", text(&flow_inside));
    assert!(text(&flow_inside).contains("flow:greeting-window"));
    let flow_outside = write(
        &fixture,
        "marked",
        "m-4",
        "web/app.js",
        "export function greet(name) {\n  // @crane:flow:greeting-window:start\n  return `hi ${name}`;\n  // @crane:flow:greeting-window:end\n}\nexport const extra = true;\n",
    );
    assert!(flow_outside.status.success(), "{}", text(&flow_outside));
}

#[test]
fn source_marker_integrity_and_resolution_fail_closed() {
    let fixture = fixture();
    let original = "# @crane:zone:ledger-window:start\ndef post(entry):\n    return entry\n# @crane:zone:ledger-window:end\n";
    fixture.write("app/ledger.py", original);
    govern(
        &fixture,
        "version: 1\nzones:\n  - id: ledger-window\n    autonomy_ceiling: observe\n",
    );
    fixture.hook(
        "claude",
        "SessionStart",
        &json!({"session_id": "marker-guard"}),
    );
    let marker_edit = write(
        &fixture,
        "marker-guard",
        "marker-edit",
        "app/ledger.py",
        &original.replace("# @crane:zone:ledger-window:start\n", ""),
    );
    assert_eq!(marker_edit.status.code(), Some(2), "{}", text(&marker_edit));
    assert!(
        text(&marker_edit).contains("markers"),
        "{}",
        text(&marker_edit)
    );
    fixture.write(
        "app/ledger.py",
        &original.replace("ledger-window", "missing-zone"),
    );
    let orphan = fixture.fails(&["validate"]);
    assert!(orphan.contains("source_marker_unresolved"), "{orphan}");

    fixture.write(
        "app/ledger.py",
        &original.replace("# @crane:zone:ledger-window:end\n", ""),
    );
    let unmatched = fixture.fails(&["validate"]);
    assert!(
        unmatched.contains("source_marker_unresolved"),
        "{unmatched}"
    );

    fixture.write("app/ledger.py", &format!("{original}{original}"));
    let duplicate = fixture.fails(&["governance", "sign"]);
    assert!(
        duplicate.contains("markers must have exactly one"),
        "{duplicate}"
    );

    fixture.write(
        "app/ledger.py",
        "# @crane:zone:ledger-window:start\n# @crane:zone:second-window:start\nx = 1\n# @crane:zone:ledger-window:end\n# @crane:zone:second-window:end\n",
    );
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: ledger-window\n    autonomy_ceiling: observe\n  - id: second-window\n    autonomy_ceiling: observe\n",
    );
    let crossed = fixture.fails(&["governance", "sign"]);
    assert!(
        crossed.contains("source markers overlap or are crossed"),
        "{crossed}"
    );

    fixture.write(
        "app/ledger.py",
        &original.replace(
            "# @crane:zone:ledger-window:end",
            "#@crane:zone:ledger-window:end",
        ),
    );
    let malformed = fixture.fails(&["validate"]);
    assert!(malformed.contains("source_marker_malformed"), "{malformed}");
}

/** Flows are discovered from their entry points across files; ambiguous or missing entry points
 * are UNRESOLVED; a flow's ceiling applies to every file it reaches
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn flows_are_discovered_and_enforced() {
    let fixture = fixture();
    govern(&fixture,
        "version: 1\nflows:\n  - id: order-submission\n    description: Order submission and charging\n    entry_points: [\"app/service.py:submit\"]\n    criticality: critical\n    autonomy_ceiling: assisted\n",
    );
    assert!(fixture.ok(&["validate"]).contains("PASS"));
    fixture.hook("claude", "SessionStart", &json!({"session_id": "f1"}));
    let reached = write(
        &fixture,
        "f1",
        "f-1",
        "app/payments.py",
        "def charge(o):\n    return o\n",
    );
    let answer: Value = serde_json::from_slice(&reached.stdout).unwrap_or(Value::Null);
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecision"],
        "ask",
        "the flow reaches charge() in app/payments.py: {}",
        text(&reached)
    );
    assert!(
        answer.to_string().contains("flow:order-submission"),
        "{answer}"
    );
    let unrelated = write(&fixture, "f1", "f-2", "src/lib.rs", "pub fn x() {}\n");
    assert!(unrelated.stdout.is_empty(), "{}", text(&unrelated));
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nflows:\n  - id: broken\n    entry_points: [\"doesNotExist\"]\n",
    );
    let output = fixture.fails(&["validate"]);
    assert!(output.contains("flow_unresolved"), "{output}");
}

/** Stale selectors warn, unknown references and invalid YAML fail validation, and invalid
 * configuration makes the hook deny changes (fail closed)
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn configuration_problems_are_reported() {
    let fixture = fixture();
    govern(
        &fixture,
        "version: 1\nzones:\n  - id: gone\n    selectors:\n      paths: [\"old/**\"]\n",
    );
    let stale = fixture.ok(&["validate"]);
    assert!(stale.contains("selector_stale"), "{stale}");
    fixture.write(".crane/governance.yaml", "version: 1\nzones:\n  - id: z\n    selectors:\n      paths: [\"app/**\"]\n    policies: [nope]\n    context_packs: [missing]\n");
    let unknown = fixture.fails(&["validate"]);
    assert!(
        unknown.contains("governance_unknown_policy")
            && unknown.contains("governance_unknown_context"),
        "{unknown}"
    );
    fixture.write(
        ".crane/governance.yaml",
        "version: 1\nzones:\n  - id: z\n  - id: z\n",
    );
    assert!(fixture
        .fails(&["validate"])
        .contains("governance_duplicate_id"));
    fixture.write(".crane/governance.yaml", "version: 1\nzonez: []\n");
    let invalid = fixture.fails(&["validate"]);
    assert!(invalid.contains("governance_yaml_invalid"), "{invalid}");
    fixture.hook("claude", "SessionStart", &json!({"session_id": "c1"}));
    let denied = write(&fixture, "c1", "c-1", "web/app.js", "x\n");
    assert_eq!(denied.status.code(), Some(2), "{}", text(&denied));
    assert!(text(&denied).contains("fail closed"), "{}", text(&denied));
}

/** Context packs referenced by active zones are composed, deduplicated, and supplied at session
 * start, with the zone descriptions
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn context_packs_are_supplied() {
    let fixture = fixture();
    govern(&fixture,
        "version: 1\nzones:\n  - id: payment-core\n    description: Core payment resources\n    selectors:\n      paths: [\"app/**\"]\n    context_packs: [payment-invariants]\nflows:\n  - id: order-submission\n    entry_points: [\"submit\"]\n    context_packs: [payment-invariants]\ncontext_packs:\n  - id: payment-invariants\n    version: 2\n    content:\n      - Payment processing must be idempotent.\n    files: [docs/guide.md]\n  - id: unused\n    version: 1\n    content: [Not referenced]\n",
    );
    let output = fixture.hook("claude", "SessionStart", &json!({"session_id": "p1"}));
    let context: Value = serde_json::from_slice(&output.stdout).unwrap();
    let text = context["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert_eq!(
        text.matches("[payment-invariants v2]").count(),
        1,
        "deduplicated: {text}"
    );
    assert!(
        text.contains("Payment processing must be idempotent."),
        "{text}"
    );
    assert!(text.contains("Use the payment API carefully."), "{text}");
    assert!(!text.contains("Not referenced"), "{text}");
    assert!(text.contains("Zone payment-core"), "{text}");
    let session = fixture.ok(&["session", "current"]);
    assert!(session.contains("pack:payment-invariants@v2"), "{session}");
}

/** The guidance of the zones and flows a tool call touches is given to the agent with that call
 * (PreToolUse additionalContext), every time; a call outside them gets none
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn guidance_is_given_with_each_tool_call() {
    let fixture = fixture();
    govern(
        &fixture,
        "version: 1
zones:
  - id: payment-core
    description: Core payment resources
    selectors:
      files: [\"app/payments.py\"]
    criticality: critical
    autonomy_ceiling: autonomous
    context_packs: [payment-invariants]
context_packs:
  - id: payment-invariants
    version: 1
    content:
      - Charges must be idempotent; never retry a capture.
",
    );
    fixture.hook("claude", "SessionStart", &json!({"session_id": "g1"}));
    let read = |call: &str, path: &str| {
        fixture.hook(
            "claude",
            "pre-tool-use",
            &json!({"session_id": "g1", "tool_use_id": call, "cwd": fixture.work.to_string_lossy(), "tool_name": "Read", "tool_input": {"file_path": fixture.work.join(path).to_string_lossy()}}),
        )
    };
    for call in ["r1", "r2"] {
        let inside = read(call, "app/payments.py");
        assert!(inside.status.success(), "{}", text(&inside));
        let answer: Value = serde_json::from_slice(&inside.stdout).unwrap();
        assert_eq!(answer["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        assert!(
            answer["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none(),
            "guidance never decides: {answer}"
        );
        let context = answer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .unwrap();
        assert!(context.contains("never retry a capture"), "{context}");
        assert!(
            context.contains("Zone payment-core (critical, autonomy ceiling autonomous)"),
            "{context}"
        );
        assert!(
            answer["systemMessage"]
                .as_str()
                .is_some_and(|message| message.contains("Crane context · zone:payment-core")),
            "every touch is visible to the user: {answer}"
        );
    }
    let outside = read("r3", "app/ledger.py");
    assert!(outside.status.success(), "{}", text(&outside));
    assert!(
        outside.stdout.is_empty(),
        "no guidance outside the zone: {}",
        text(&outside)
    );
    govern(
        &fixture,
        "version: 1\nzones:\n  - id: payment-core\n    description: Core payment resources, updated\n    selectors:\n      files: [\"app/payments.py\"]\n    criticality: critical\n    autonomy_ceiling: autonomous\n    context_packs: [payment-invariants]\ncontext_packs:\n  - id: payment-invariants\n    version: 1\n    content:\n      - Charges must be idempotent; never retry a capture.\n",
    );
    let changed = read("r4", "app/payments.py");
    assert!(changed.status.success(), "{}", text(&changed));
    let answer: Value = serde_json::from_slice(&changed.stdout).unwrap();
    assert!(
        answer["systemMessage"]
            .as_str()
            .is_some_and(|message| message.contains("updated")),
        "changed context is shown again: {answer}"
    );

    for (profile, session, payload, message_field) in [
        (
            "copilot",
            "copilot-zone",
            json!({
                "sessionId": "copilot-zone",
                "toolName": "view",
                "toolArgs": {"filePath": fixture.work.join("app/payments.py").to_string_lossy()},
                "cwd": fixture.work.to_string_lossy(),
            }),
            "progress",
        ),
        (
            "gemini",
            "gemini-zone",
            json!({
                "session_id": "gemini-zone",
                "tool_name": "read_file",
                "tool_input": {"file_path": fixture.work.join("app/payments.py").to_string_lossy()},
                "cwd": fixture.work.to_string_lossy(),
            }),
            "systemMessage",
        ),
    ] {
        let output = fixture.hook(profile, "pre-tool-use", &payload);
        assert!(output.status.success(), "{profile}: {}", text(&output));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("Crane context · zone:payment-core")
                && stdout.contains("Core payment resources, updated"),
            "{profile} must show zone context in the agent window: {stdout}"
        );
        if profile == "copilot" {
            let final_line = stdout.lines().last().unwrap();
            let answer: Value = serde_json::from_str(final_line).unwrap();
            assert_eq!(answer["permissionDecision"], "allow");
            assert_eq!(session, "copilot-zone");
            assert_eq!(message_field, "progress");
        } else {
            let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(answer["systemMessage"].is_string());
            assert_eq!(session, "gemini-zone");
            assert_eq!(message_field, "systemMessage");
        }
    }
}
