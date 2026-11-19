// The operator's switch for agents running read-only Crane commands, through the real binary and
// its hooks: off by default; on only with a fresh policy signed by the operator key the build
// trusts; even then only plain read-only commands run, and a person can turn it off on their
// machine. No network is used: the test writes the cached policy the sync would keep.

mod common;

use std::io::Write;
use std::process::{Output, Stdio};

use common::{text, Fixture};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::{json, Value};

/** Signature domain of the operator policy (trust/agent_commands.rs) */
const DOMAIN: &str = "crane/operator/agent-commands/v1";

/** An operator key pair made by 'crane operator keygen'
 * Fields
    - seed: String - signing key seed (hex)
    - public: String - public key (hex)
*/
struct Key {
    seed: String,
    public: String,
}

/** Create an operator key pair with the real command
 * Input
    - fixture: &Fixture - repository
 * Output
    - Key
*/
fn keygen(fixture: &Fixture) -> Key {
    let printed = fixture.ok(&["operator", "keygen"]);
    let value = |name: &str| {
        printed
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{name}=")))
            .unwrap_or_else(|| panic!("{name} missing: {printed}"))
            .to_string()
    };
    Key {
        seed: value("CRANE_OPERATOR_SIGNING_KEY"),
        public: value("CRANE_OPERATOR_PUBLIC_KEY"),
    }
}

/** Decode hex
 * Input
    - text: &str - hex
 * Output
    - Vec<u8>
*/
fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

/** Write the cached policy the background sync would keep, signed by a key
 * Input
    - fixture: &Fixture - repository (its trust directory)
    - key: &Key - the signing key
    - enabled: bool - the switch
    - issued_ago_minutes: u64 - how long ago it was issued (16 makes it expired)
 * Output
    - None
*/
fn cache(fixture: &Fixture, key: &Key, enabled: bool, issued_ago_minutes: u64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let issued = now - issued_ago_minutes * 60_000;
    let expires = issued + 15 * 60_000;
    let message = format!("{enabled}\n{issued}\n{expires}\noperator\n{issued}");
    let mut signed = b"crane-signature\0".to_vec();
    signed.extend_from_slice(DOMAIN.as_bytes());
    signed.push(0);
    signed.extend_from_slice(message.as_bytes());
    let signing = SigningKey::from_bytes(&unhex(&key.seed).try_into().unwrap());
    let signature = signing
        .sign(&signed)
        .to_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let policy = json!({
        "policy": {"enabled": enabled, "issued_at": issued, "expires_at": expires, "changed_by": "operator", "changed_at": issued},
        "public_key": key.public,
        "signature": signature,
    });
    let path = fixture.home.join("operator").join("agent-commands.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, policy.to_string()).unwrap();
}

/** Run crane as an agent would (an agent marker set), trusting an operator key
 * Input
    - fixture: &Fixture - repository
    - trusted: &str - the operator public key this build trusts
    - extra: &[(&str, &str)] - more environment
    - args: &[&str] - arguments
 * Output
    - Output
*/
fn agent(fixture: &Fixture, trusted: &str, extra: &[(&str, &str)], args: &[&str]) -> Output {
    let mut command = fixture.command(args);
    command
        .env("CLAUDECODE", "1")
        .env("CRANE_OPERATOR_PUBLIC_KEY", trusted);
    for (name, value) in extra {
        command.env(name, value);
    }
    command.output().unwrap()
}

/** Run a Claude Code hook event, trusting an operator key
 * Input
    - fixture: &Fixture - repository
    - trusted: &str - the operator public key this build trusts
    - event: &str - hook event
    - payload: &Value - hook payload
 * Output
    - Output
*/
fn hook(fixture: &Fixture, trusted: &str, event: &str, payload: &Value) -> Output {
    let mut child = fixture
        .command(&["agent", "hook", "--event", event, "--profile", "claude"])
        .env("CLAUDECODE", "1")
        .env("CRANE_OPERATOR_PUBLIC_KEY", trusted)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

/** A Bash tool call payload
 * Input
    - fixture: &Fixture - repository
    - id: &str - tool call id
    - command: &str - the shell command
 * Output
    - Value
*/
fn bash(fixture: &Fixture, id: &str, command: &str) -> Value {
    json!({"session_id": "op-1", "tool_use_id": id, "cwd": fixture.work.to_string_lossy(), "tool_name": "Bash", "tool_input": {"command": command}})
}

/** Agents run read-only Crane commands only while a fresh, operator-signed policy says on; writes
 * stay human-only; a forged, expired, or "off" policy and a person's local off all refuse
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn agents_run_read_only_commands_only_while_the_operator_allows_it() {
    let fixture = Fixture::ready();
    let operator = keygen(&fixture);
    let customer = keygen(&fixture);
    let refused = |output: &Output| !output.status.success();

    // Off by default: no policy on this machine
    let validate = agent(&fixture, &operator.public, &[], &["validate"]);
    assert!(refused(&validate), "{}", text(&validate));
    assert!(
        text(&validate).contains("only while the operator allows it"),
        "{}",
        text(&validate)
    );

    // On, signed by the operator: read-only commands run
    cache(&fixture, &operator, true, 0);
    for args in [
        vec!["validate"],
        vec!["test", "."],
        vec!["scope", "inspect"],
        vec!["session", "."],
        vec!["governance", "graph"],
        vec!["brick", "list"],
        vec!["flow", "list"],
        vec!["--version"],
    ] {
        let output = agent(&fixture, &operator.public, &[], &args);
        assert!(output.status.success(), "{args:?}: {}", text(&output));
    }
    // Everything that changes anything stays human-only
    for args in [
        vec!["checkpoint", "agent"],
        vec![
            "protect",
            "app/payments.py",
            "start-line",
            "1",
            "end-line",
            "2",
        ],
        vec!["governance", "sign"],
        vec!["scope", "diff", "--record"],
        vec!["agent", "install", "--profile", "claude"],
        vec!["secret", "list"],
        vec!["operator", "status"],
        vec!["operator", "keygen"],
        vec!["brick", "inbox", "apply", "--all"],
    ] {
        let output = agent(&fixture, &operator.public, &[], &args);
        assert!(
            refused(&output),
            "{args:?} must be refused: {}",
            text(&output)
        );
    }
    // A person is unaffected
    fixture.ok(&["validate"]);

    // A person can turn it off on their own machine
    let off = agent(
        &fixture,
        &operator.public,
        &[("CRANE_AGENT_COMMANDS", "off")],
        &["validate"],
    );
    assert!(refused(&off), "{}", text(&off));

    // Signed by anyone but the operator: refused (a customer cannot turn it on)
    cache(&fixture, &customer, true, 0);
    let forged = agent(&fixture, &operator.public, &[], &["validate"]);
    assert!(refused(&forged), "{}", text(&forged));
    assert!(
        text(&forged).contains("not signed by the operator key"),
        "{}",
        text(&forged)
    );

    // Expired: refused
    cache(&fixture, &operator, true, 16);
    let expired = agent(&fixture, &operator.public, &[], &["validate"]);
    assert!(refused(&expired), "{}", text(&expired));
    assert!(text(&expired).contains("expired"), "{}", text(&expired));

    // The operator turned it off: refused
    cache(&fixture, &operator, false, 0);
    let disabled = agent(&fixture, &operator.public, &[], &["validate"]);
    assert!(refused(&disabled), "{}", text(&disabled));
    assert!(
        text(&disabled).contains("has not allowed agents"),
        "{}",
        text(&disabled)
    );

    // A person sees the same on their machine
    cache(&fixture, &operator, true, 0);
    let status = fixture
        .command(&["operator", "status"])
        .env("CRANE_OPERATOR_PUBLIC_KEY", &operator.public)
        .output()
        .unwrap();
    assert!(
        text(&status).contains("switch: ON")
            && text(&status).contains("read-only commands now: yes"),
        "{}",
        text(&status)
    );
}

/** The hooks let one plain read-only Crane command through while the switch is on, and deny the
 * rest (writes, chains, redirections) and everything while it is off; the session context says
 * which applies
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn hooks_follow_the_operator_switch() {
    let fixture = Fixture::ready();
    let operator = keygen(&fixture);

    // Off: the session is told not to run crane, and the hook denies it
    let start = hook(
        &fixture,
        &operator.public,
        "SessionStart",
        &json!({"session_id": "op-1"}),
    );
    assert!(start.status.success(), "{}", text(&start));
    assert!(
        text(&start).contains("do not run crane"),
        "{}",
        text(&start)
    );
    let denied = hook(
        &fixture,
        &operator.public,
        "pre-tool-use",
        &bash(&fixture, "t1", "crane validate"),
    );
    assert_eq!(denied.status.code(), Some(2), "{}", text(&denied));

    // On: the session context says so, and a plain read-only command passes
    cache(&fixture, &operator, true, 0);
    let start = hook(
        &fixture,
        &operator.public,
        "SessionStart",
        &json!({"session_id": "op-2"}),
    );
    assert!(
        text(&start).contains("Crane commands that only read are allowed"),
        "{}",
        text(&start)
    );
    for (id, command) in [
        ("t2", "crane validate"),
        ("t3", "crane scope inspect --json"),
        ("t4", "crane flow list"),
    ] {
        let allowed = hook(
            &fixture,
            &operator.public,
            "pre-tool-use",
            &bash(&fixture, id, command),
        );
        assert!(allowed.status.success(), "{command}: {}", text(&allowed));
    }
    // Still denied: writes, chains, redirections, substitutions
    for (id, command) in [
        ("t5", "crane governance sign"),
        ("t6", "crane validate && crane init"),
        ("t7", "crane validate > out.txt"),
        ("t8", "crane $(echo init)"),
        ("t9", "X=1 crane validate"),
        ("t10", "crane brick inbox apply --all"),
    ] {
        let denied = hook(
            &fixture,
            &operator.public,
            "pre-tool-use",
            &bash(&fixture, id, command),
        );
        assert_eq!(
            denied.status.code(),
            Some(2),
            "{command} must be denied: {}",
            text(&denied)
        );
    }
}
