// The shared scope engine through 'crane scope': decisions identical to the hooks' guard, resolved
// resources (traversal, metadata, ignored files, outside the repository), recorded decisions and
// their staleness, hook decisions explained, implicit widening by a rename, adversarial probes,
// and the audit of recorded attempts. Every scope command is refused to agents.

mod common;

use common::{text, Fixture};
use serde_json::{json, Value};

/** Zones of the governed fixture: the ledger only observed, the web directory assisted */
const ZONES: &str = "version: 1
zones:
  - id: ledger-core
    selectors:
      files: [app/ledger.py]
    autonomy_ceiling: observe
  - id: web
    selectors:
      directories: [web]
    autonomy_ceiling: assisted
";

/** A fixture with a preserved block (payments lines 2-5 after the markers), a target (lines
 * 10-11), and signed zones
 * Input
    - None
 * Output
    - (Fixture, String, String) fixture, preserved id, target id
*/
fn governed() -> (Fixture, String, String) {
    let fixture = Fixture::ready();
    let keep = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let change = fixture.target(&["app/payments.py", "start-line", "9", "end-line", "10"]);
    fixture.write(".crane/governance.yaml", ZONES);
    assert!(fixture
        .ok(&["governance", "sign"])
        .contains("Signed .crane/governance.yaml"));
    fixture.commit("governed");
    (fixture, keep, change)
}

/** Resolve a request and return its recorded decision
 * Input
    - fixture: &Fixture - repository
    - args: &[&str] - arguments after "scope resolve"
 * Output
    - Value
*/
fn resolve(fixture: &Fixture, args: &[&str]) -> Value {
    let mut full = vec!["scope", "resolve"];
    full.extend_from_slice(args);
    full.push("--json");
    let (ok, value) = fixture.json(&full);
    assert!(ok, "{value}");
    value
}

/** The rule codes of a decision
 * Input
    - value: &Value - recorded decision
 * Output
    - Vec<String>
*/
fn rules(value: &Value) -> Vec<String> {
    value["rules"]
        .as_array()
        .unwrap()
        .iter()
        .map(|rule| rule.as_str().unwrap().to_string())
        .collect()
}

/** The gap codes of a decision
 * Input
    - value: &Value - recorded decision
 * Output
    - Vec<String>
*/
fn gaps(value: &Value) -> Vec<String> {
    value["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gap| gap["code"].as_str().unwrap().to_string())
        .collect()
}

/** Resolve decides exactly as the guard: preserved lines and spellings of them are denied, target
 * lines allowed, zone ceilings applied, metadata and Crane commands denied, and what Crane cannot
 * enforce is reported as a gap
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn resolve_decides_like_the_hooks() {
    let (fixture, keep, _) = governed();
    let preserved = resolve(&fixture, &["write", "app/payments.py", "lines", "2-5"]);
    assert_eq!(preserved["decision"], "DENY", "{preserved}");
    // Once per selection, although its start and end markers both name it
    assert_eq!(rules(&preserved), vec!["PRESERVED_SELECTION".to_string()]);
    assert_eq!(preserved["selections"], json!([keep]));
    assert_eq!(preserved["resource"]["location"], "repository");
    assert_eq!(preserved["resource"]["tracking"], "tracked");
    assert!(preserved["decision_id"]
        .as_str()
        .unwrap()
        .starts_with("scd_"));
    let target = resolve(&fixture, &["write", "app/payments.py", "lines", "10-11"]);
    assert_eq!(target["decision"], "ALLOW", "{target}");
    let walked = resolve(
        &fixture,
        &["write", "web/../app/payments.py", "lines", "2-5"],
    );
    assert_eq!(walked["decision"], "DENY", "{walked}");
    assert_eq!(walked["resource"]["path"], "app/payments.py");
    let metadata = resolve(&fixture, &["write", ".crane/registry.json"]);
    assert_eq!(metadata["decision"], "DENY");
    assert_eq!(metadata["resource"]["location"], "crane_metadata");
    assert!(rules(&metadata).contains(&"CRANE_METADATA_WRITE".to_string()));
    let ledger = resolve(&fixture, &["write", "app/ledger.py"]);
    assert_eq!(ledger["decision"], "DENY");
    assert!(rules(&ledger).contains(&"ZONE_OBSERVE".to_string()));
    assert_eq!(ledger["zones"], json!(["zone:ledger-core"]));
    let web = resolve(&fixture, &["write", "web/app.js"]);
    assert_eq!(web["decision"], "REQUIRE_APPROVAL", "{web}");
    let observe = resolve(&fixture, &["write", "docs/guide.md", "mode", "observe"]);
    assert_eq!(observe["decision"], "DENY");
    assert!(rules(&observe).contains(&"MODE_OBSERVE".to_string()));
    let crane = resolve(&fixture, &["execute", "crane validate"]);
    assert!(rules(&crane).contains(&"CRANE_COMMAND".to_string()));
    let network = resolve(&fixture, &["execute", "curl https://example.com/x"]);
    assert_eq!(network["decision"], "ALLOW");
    assert!(gaps(&network).contains(&"NETWORK_NOT_MEDIATED".to_string()));
    assert!(gaps(&network).contains(&"EFFECT_UNKNOWN_BEFORE_RUN".to_string()));
    let outside = fixture.root.path().join("elsewhere.txt");
    let outside = resolve(&fixture, &["write", outside.to_str().unwrap()]);
    assert_eq!(outside["resource"]["location"], "outside_repository");
    assert!(gaps(&outside).contains(&"OUTSIDE_REPOSITORY".to_string()));
    fixture.write(".gitignore", "build/\n");
    fixture.write("build/out.js", "x\n");
    let ignored = resolve(&fixture, &["write", "build/out.js"]);
    assert_eq!(ignored["resource"]["tracking"], "ignored", "{ignored}");
    assert!(gaps(&ignored).contains(&"IGNORED_FILE".to_string()));
    // Malformed requests are refused, not guessed
    assert!(fixture
        .fails(&[
            "scope",
            "resolve",
            "read",
            "app/payments.py",
            "lines",
            "1-2"
        ])
        .contains("lines apply only to write"));
    assert!(fixture
        .fails(&[
            "scope",
            "resolve",
            "write",
            "app/payments.py",
            "lines",
            "90-99"
        ])
        .contains("not within"));
    assert!(fixture
        .fails(&["scope", "resolve", "network", "example.com"])
        .contains("must be a URL"));
    // Only a person runs Crane: the agent gate covers the scope commands too
    let agent = fixture.as_agent(&["scope", "inspect"]);
    assert!(!agent.status.success());
    assert!(text(&agent).contains("cannot be executed by an AI agent"));
}

/** A recorded decision is explained with its rules and domains, and stops applying when the file
 * it was made for changes
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn recorded_decisions_are_explained_and_go_stale() {
    let (fixture, _, _) = governed();
    let decision = resolve(&fixture, &["write", "app/payments.py", "lines", "2-5"]);
    let id = decision["decision_id"].as_str().unwrap();
    let (ok, explained) = fixture.json(&["scope", "explain", id, "--json"]);
    assert!(ok, "{explained}");
    assert_eq!(explained["decision"], "DENY");
    assert_eq!(explained["rules"][0]["code"], "PRESERVED_SELECTION");
    assert_eq!(explained["rules"][0]["domain"], "constraints");
    assert_eq!(explained["changed_since"], json!([]));
    assert!(fixture
        .ok(&["scope", "explain", id])
        .contains("Still applies"));
    let text = fixture.read("app/payments.py");
    fixture.write("app/payments.py", &format!("{text}\n# later\n"));
    let (_, stale) = fixture.json(&["scope", "explain", id, "--json"]);
    assert_eq!(
        stale["changed_since"],
        json!(["the file's content changed"]),
        "{stale}"
    );
    assert!(fixture
        .fails(&["scope", "explain", "scd_nothing"])
        .contains("no scope decision"));
}

/** A hook's decision is explained by its tool call id, from the event log
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn hook_decisions_are_explained() {
    let (fixture, _, _) = governed();
    fixture.hook("claude", "SessionStart", &json!({"session_id": "s1"}));
    let blocked = fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": "s1", "tool_use_id": "call-ledger", "cwd": fixture.work.to_string_lossy(), "tool_name": "Write", "tool_input": {"file_path": fixture.work.join("app/ledger.py").to_string_lossy(), "content": "x = 1\n"}}),
    );
    assert_eq!(blocked.status.code(), Some(2), "{}", text(&blocked));
    let (ok, explained) = fixture.json(&["scope", "explain", "call-ledger", "--json"]);
    assert!(ok, "{explained}");
    assert_eq!(explained["source"], "hook decision");
    assert_eq!(explained["decision"], "DENY");
    assert_eq!(explained["rules"][0]["code"], "ZONE_OBSERVE");
    assert_eq!(explained["rules"][0]["domain"], "permission");
    let (ok, audit) = fixture.json(&["scope", "audit", "--json"]);
    assert!(ok, "{audit}");
    assert_eq!(audit["decisions"]["DENY"], 1, "{audit}");
    assert_eq!(audit["domains"]["permission"], 1);
    assert_eq!(audit["chain"], "verified");
}

/** Renaming a governed file out of its zone widens the boundary without anyone signing: diff
 * reports it and fails until the change is accepted
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn renames_out_of_a_zone_are_reported_as_widening() {
    let (fixture, _, _) = governed();
    let (ok, unchanged) = fixture.json(&["scope", "diff", "--json"]);
    assert!(ok, "{unchanged}");
    assert_eq!(unchanged["changes"], json!([]), "{unchanged}");
    common::git(&fixture.work, &["mv", "app/ledger.py", "app/books.py"]);
    let (ok, widened) = fixture.json(&["scope", "diff", "--json"]);
    assert!(!ok);
    assert_eq!(widened["widened"], true, "{widened}");
    let change = &widened["changes"][0];
    assert_eq!(change["path"], "app/ledger.py");
    assert_eq!(change["kind"], "widened");
    assert!(
        change["detail"]
            .as_str()
            .unwrap()
            .contains("moved to app/books.py"),
        "{widened}"
    );
    // The file now escapes the zone: the guard allows what the zone forbade
    assert_eq!(
        resolve(&fixture, &["write", "app/books.py"])["decision"],
        "ALLOW"
    );
    fixture.ok(&["scope", "diff", "--record"]);
    let (ok, accepted) = fixture.json(&["scope", "diff", "--json"]);
    assert!(ok, "{accepted}");
    assert_eq!(accepted["changes"], json!([]));
}

/** The probes hold on a governed repository: no failure, every category exercised, and the
 * documented gaps reported as gaps
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn probes_hold_and_report_gaps() {
    let (fixture, keep, _) = governed();
    let (ok, report) = fixture.json(&["scope", "test", "--json"]);
    let probes = report["probes"].as_array().unwrap();
    let failed = probes
        .iter()
        .filter(|probe| probe["status"] == "FAIL")
        .collect::<Vec<_>>();
    assert!(ok && failed.is_empty(), "{failed:#?}");
    for category in ["metadata", "selections", "zones", "session", "gaps"] {
        assert!(
            probes.iter().any(|probe| probe["category"] == category),
            "no {category} probe"
        );
    }
    let named = |needle: &str| {
        probes
            .iter()
            .find(|probe| probe["name"].as_str().unwrap().contains(needle))
            .unwrap_or_else(|| panic!("no probe {needle}"))
    };
    assert_eq!(
        named(&format!("edit preserved {keep} through app/../"))["status"],
        "PASS"
    );
    assert_eq!(named("remove a marker")["status"], "PASS");
    assert_eq!(named("change app/ledger.py")["status"], "PASS");
    assert_eq!(named("curl https://example.com")["status"], "GAP");
    assert_eq!(named("named through a variable")["status"], "GAP");
    let summary = fixture.ok(&["scope", "test"]);
    assert!(summary.contains("0 failed"), "{summary}");
    let inspected = fixture.ok(&["scope", "inspect", "app/payments.py"]);
    assert!(inspected.contains("Governed files"), "{inspected}");
    assert!(
        inspected.contains(&format!("preserved {keep}")),
        "{inspected}"
    );
    assert!(inspected.contains("sandbox: none"), "{inspected}");
    let (ok, view) = fixture.json(&["scope", "inspect", "app/payments.py", "--json"]);
    assert!(ok);
    assert_eq!(view["paths"][0]["access"]["delete"]["decision"], "DENY");
    assert_eq!(view["governed"]["app/ledger.py"]["ceiling"], "observe");
}

/** A write into another worktree of the repository is denied (its protected code cannot be checked
 * from this session), by the hook and by resolve, and the probe for it passes
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn writes_into_other_worktrees_are_denied() {
    let (fixture, _, _) = governed();
    let other = fixture.root.path().join("other");
    common::git(
        &fixture.work,
        &[
            "worktree",
            "add",
            "-q",
            other.to_str().unwrap(),
            "-b",
            "other",
        ],
    );
    let target = other.join("app/payments.py");
    let decided = resolve(&fixture, &["write", target.to_str().unwrap()]);
    assert_eq!(decided["decision"], "DENY", "{decided}");
    assert_eq!(decided["rules"], json!(["OTHER_WORKTREE_WRITE"]));
    assert_eq!(decided["resource"]["location"], "other_worktree");
    // Elsewhere outside the repository nothing changes: allowed, reported as ungoverned
    let elsewhere = fixture.root.path().join("notes.txt");
    assert_eq!(
        resolve(&fixture, &["write", elsewhere.to_str().unwrap()])["decision"],
        "ALLOW"
    );
    fixture.hook("claude", "SessionStart", &json!({"session_id": "s1"}));
    let blocked = fixture.hook(
        "claude",
        "pre-tool-use",
        &json!({"session_id": "s1", "tool_use_id": "call-other", "cwd": fixture.work.to_string_lossy(), "tool_name": "Write", "tool_input": {"file_path": target.to_string_lossy(), "content": "x = 1\n"}}),
    );
    assert_eq!(blocked.status.code(), Some(2), "{}", text(&blocked));
    assert!(
        text(&blocked).contains("another worktree"),
        "{}",
        text(&blocked)
    );
    let (ok, report) = fixture.json(&["scope", "test", "--json"]);
    assert!(ok, "{report}");
    let probe = report["probes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|probe| probe["name"].as_str().unwrap().contains("another worktree"))
        .unwrap();
    assert_eq!(probe["status"], "PASS", "{probe}");
}

/** Agents never run Crane, so they receive the resolved boundary in their session context:
 * restricted files with their rules, and how actions are judged
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn agents_receive_the_boundary_in_their_context() {
    let (fixture, _, _) = governed();
    let started = fixture.hook("claude", "SessionStart", &json!({"session_id": "s1"}));
    assert!(started.status.success(), "{}", text(&started));
    let context = text(&started);
    assert!(context.contains("Effective boundary"), "{context}");
    assert!(
        context.contains("app/ledger.py: no agent changes (zone:ledger-core)"),
        "{context}"
    );
    assert!(
        context.contains("web/app.js: every change needs a person's approval (zone:web)"),
        "{context}"
    );
    assert!(context.contains("quarantines the session"), "{context}");
}
