// Basic commands: version, help, the reduced command set, init, checkpoints, defaults, policy
// creation, parse, and refusal of governance commands for AI agents.

mod common;

use common::{text, Fixture};

/** The version command prints the package version
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn version_and_help() {
    let fixture = Fixture::new();
    let version = fixture.ok(&["--version"]);
    assert!(version.starts_with("crane "), "{version}");
    assert!(version.trim().split(' ').nth(1).unwrap().split('.').count() == 3);
    let help = fixture.ok(&["help"]);
    for command in [
        "crane protect",
        "crane target",
        "crane checkpoint",
        "crane repo status",
        "crane integrate",
    ] {
        assert!(help.contains(command), "help lists {command}");
    }
}

/** Commands outside the command set no longer exist
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn removed_commands_are_unknown() {
    let fixture = Fixture::ready();
    for removed in [
        vec!["check"],
        vec!["status"],
        vec!["discover"],
        vec!["zones"],
        vec!["deliver", "run", "x"],
        vec!["observe"],
        vec!["context"],
        vec!["test-all"],
        vec!["autonomy", "status"],
    ] {
        let output = fixture.fails(&removed);
        assert!(output.contains("unknown command"), "{removed:?}: {output}");
    }
}

/** init requires a GitHub connection, is idempotent, and creates the default policy, signed
 * registry, map, and audit trail
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn init_requires_connection_and_is_idempotent() {
    let fixture = Fixture::new();
    let refused = fixture.fails(&["init"]);
    assert!(refused.contains("NOT_CONNECTED"), "{refused}");
    fixture.ok(&["repo", "--https", fixture.remote_url().as_str()]);
    let first = fixture.ok(&["init"]);
    assert!(
        first.contains("CONNECTED") && first.contains("generation 1"),
        "{first}"
    );
    assert_eq!(
        fixture.read(".crane/policies/default.crane"),
        "policy default {\n}\n"
    );
    for file in [
        "registry.json",
        "map",
        "audit.jsonl",
        "config.json",
        "checkpoints.json",
        "context.json",
    ] {
        assert!(
            fixture.work.join(".crane").join(file).exists(),
            "{file} exists"
        );
    }
    let registry = fixture.crane_json("registry.json");
    assert_eq!(registry["manifest"]["generation"], 1);
    assert_eq!(registry["manifest"]["head"].as_str().unwrap().len(), 128);
    let second = fixture.ok(&["init"]);
    assert!(second.contains("already initialized"), "{second}");
    assert_eq!(
        fixture.crane_json("registry.json")["manifest"]["generation"],
        1
    );
    let key = fixture.home.join("keys");
    assert!(
        key.is_dir(),
        "the signing key lives in the trust directory, outside the repository"
    );
    assert!(!fixture.read(".crane/registry.json").contains("seed"));
}

/** init fails in a repository without an origin remote
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn init_requires_a_clone() {
    let fixture = Fixture::new();
    common::git(&fixture.work, &["remote", "remove", "origin"]);
    let output = fixture.fails(&["init"]);
    assert!(output.contains("origin"), "{output}");
}

/** Checkpoints are insert-only, stored as an array, and the first becomes the default
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn checkpoints_are_insert_only() {
    let fixture = Fixture::ready();
    let checkpoints = fixture.crane_json("checkpoints.json");
    let list = checkpoints["checkpoints"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["name"], "baseline");
    assert_eq!(list[0]["commit"].as_str().unwrap().len(), 40);
    assert_eq!(
        fixture.crane_json("config.json")["default_checkpoint"],
        "baseline"
    );
    let duplicate = fixture.fails(&["checkpoint", "baseline"]);
    assert!(duplicate.contains("cannot be updated"), "{duplicate}");
    fixture.write("app/new.py", "x = 1\n");
    fixture.commit("more");
    fixture.ok(&["checkpoint", "release-1"]);
    let list = fixture.crane_json("checkpoints.json")["checkpoints"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(list.len(), 2);
    assert_ne!(list[0]["commit"], list[1]["commit"]);
    assert_eq!(list[1]["previous"], list[0]["digest"]);
    assert_eq!(
        fixture.crane_json("config.json")["default_checkpoint"],
        "baseline"
    );
    fixture.ok(&["--set", "default", "checkpoint", "release-1"]);
    assert_eq!(
        fixture.crane_json("config.json")["default_checkpoint"],
        "release-1"
    );
    let missing = fixture.fails(&["--set", "default", "checkpoint", "nope"]);
    assert!(missing.contains("does not exist"), "{missing}");
}

/** checkpoint fails when the GitHub connection fails
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn checkpoint_fails_without_connection() {
    let fixture = Fixture::ready();
    std::fs::rename(&fixture.remote, fixture.root.path().join("moved.git")).unwrap();
    let output = fixture.fails(&["checkpoint", "later"]);
    assert!(output.contains("FAILURE"), "{output}");
}

/** Governance commands cannot be executed by an AI agent
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn agents_cannot_run_governance_commands() {
    let fixture = Fixture::ready();
    for args in [
        vec!["checkpoint", "agent-made"],
        vec!["protect", "app/payments.py"],
        vec!["target", "app/payments.py"],
        vec!["--set", "default", "policy", "default"],
        vec!["create", "policy", "evil", "evil"],
        vec!["agent", "uninstall", "--profile", "claude"],
        vec!["repo", "del"],
        vec!["integrate", "slack"],
    ] {
        let output = fixture.as_agent(&args);
        assert!(
            !output.status.success(),
            "{args:?} must be refused for agents"
        );
        assert!(
            text(&output).contains("cannot be executed by an AI agent"),
            "{args:?}: {}",
            text(&output)
        );
    }
    let read_only = fixture.as_agent(&["validate"]);
    assert!(
        read_only.status.success(),
        "agents may validate: {}",
        text(&read_only)
    );
}

/** Policies are created in .crane/policies files; the FILENAME must not carry the extension and
 * names are unique; the default policy can be changed only to an existing policy
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn creates_policies_and_sets_defaults() {
    let fixture = Fixture::ready();
    let rejected = fixture.fails(&["create", "policy", "payments", "payments.crane"]);
    assert!(
        rejected.contains("must not include the .crane extension"),
        "{rejected}"
    );
    fixture.ok(&["create", "policy", "Payments", "payments"]);
    assert_eq!(
        fixture.read(".crane/policies/payments.crane"),
        "policy Payments {\n}\n"
    );
    fixture.ok(&["create", "policy", "refunds", "payments"]);
    assert!(fixture
        .read(".crane/policies/payments.crane")
        .contains("policy refunds {"));
    let duplicate = fixture.fails(&["create", "policy", "payments", "other"]);
    assert!(duplicate.contains("already exists"), "{duplicate}");
    let missing = fixture.fails(&["--set", "default", "policy", "nope"]);
    assert!(missing.contains("does not exist"), "{missing}");
    fixture.ok(&["--set", "default", "policy", "Payments"]);
    assert_eq!(
        fixture.crane_json("config.json")["default_policy"],
        "Payments"
    );
    let id = fixture.protect(&["app/ledger.py"]);
    assert!(fixture
        .read(".crane/policies/payments.crane")
        .contains(&format!("preserve {id};")));
}

/** parse file prints a tree of policies, commands, and selections, by path or unique name
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn parse_prints_a_tree() {
    let fixture = Fixture::ready();
    let id = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let tree = fixture.ok(&["parse", "file", "default"]);
    assert!(tree.contains(".crane/policies/default.crane"), "{tree}");
    assert!(tree.contains("policy default"), "{tree}");
    assert!(tree.contains(&format!("preserve {id}")), "{tree}");
    assert!(tree.contains("selection: app/payments.py"), "{tree}");
    assert!(tree.contains("checkpoint: baseline"), "{tree}");
    assert!(fixture
        .ok(&["parse", "file", ".crane/policies/default.crane"])
        .contains("policy default"));
    let missing = fixture.fails(&["parse", "file", "nope"]);
    assert!(missing.contains("no policy file"), "{missing}");
}
