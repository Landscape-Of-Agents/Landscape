// Teams: a teammate joins by trusting the owner's key with crane init, the owner adds and removes
// admins, admins govern from their own machines, a removed admin cannot restore an old admin list,
// a fork after a merge is accepted explicitly, and merge conflicts in .crane are explained.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::Fixture;
use serde_json::json;

/** Switch the fixture to another machine's trust directory, connected to the repository
 * Input
    - fixture: &mut Fixture - repository
    - name: &str - machine name
 * Output
    - PathBuf the previous trust directory
*/
fn machine(fixture: &mut Fixture, name: &str) -> PathBuf {
    let previous = std::mem::replace(&mut fixture.home, fixture.root.path().join(name));
    fixture.ok(&["repo", "--https", fixture.remote_url().as_str()]);
    previous
}

/** Join the repository as a teammate, answering the trust question
 * Input
    - fixture: &Fixture - repository (on the teammate's machine)
    - answer: &str - y or n
 * Output
    - String combined output
*/
fn join(fixture: &Fixture, answer: &str) -> String {
    common::text(&fixture.with_input(&["init"], &format!("{answer}\n")))
}

/** Read the key a join printed for this machine
 * Input
    - output: &str - output of a confirmed join
 * Output
    - String public key without spaces
*/
fn joined_key(output: &str) -> String {
    output
        .split("your key: ")
        .nth(1)
        .and_then(|rest| rest.split(" (").next())
        .unwrap_or_else(|| panic!("no key in: {output}"))
        .replace(' ', "")
}

/** Run a pre-tool-use edit hook and return its exit code
 * Input
    - fixture: &Fixture - repository
    - file: &str - repository-relative file
 * Output
    - Option<i32>
*/
fn edit(fixture: &Fixture, file: &str) -> Option<i32> {
    fixture.hook("claude", "SessionStart", &json!({"session_id": "team"}));
    fixture
        .hook(
            "claude",
            "pre-tool-use",
            &json!({"session_id": "team", "tool_use_id": format!("e-{file}"), "cwd": fixture.work.to_string_lossy(), "tool_name": "Edit", "tool_input": {"file_path": fixture.work.join(file).to_string_lossy(), "old_string": "return", "new_string": "return  "}}),
        )
        .status
        .code()
}

/** Copy a directory tree
 * Input
    - from: &Path - source
    - to: &Path - destination (replaced)
 * Output
    - None
*/
fn copy_tree(from: &Path, to: &Path) {
    let _ = fs::remove_dir_all(to);
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/** A teammate's machine blocks agent changes until the owner key is trusted; crane init shows the
 * fingerprint, a refusal changes nothing, and after confirming, enforcement works normally while
 * governance stays with the owner
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn teammate_joins_by_trusting_the_owner_key() {
    let mut fixture = Fixture::ready();
    fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let owner_list = fixture.ok(&["admin", "list"]);
    assert!(owner_list.contains("this machine: owner"), "{owner_list}");
    machine(&mut fixture, "teammate");
    assert!(fixture.fails(&["validate"]).contains("crane init"));
    assert_eq!(
        edit(&fixture, "app/ledger.py"),
        Some(2),
        "fails closed before trust"
    );
    let refused = join(&fixture, "n");
    assert!(refused.contains("owner key:"), "{refused}");
    assert!(refused.contains("was not trusted"), "{refused}");
    assert!(fixture.fails(&["validate"]).contains("crane init"));
    let joined = join(&fixture, "y");
    assert!(joined.contains("Trusted."), "{joined}");
    fixture.ok(&["validate"]);
    assert_eq!(
        edit(&fixture, "app/ledger.py"),
        Some(0),
        "unprotected code is editable"
    );
    assert_eq!(
        edit(&fixture, "app/payments.py"),
        Some(2),
        "protected code is not"
    );
    assert!(fixture
        .fails(&["protect", "app/ledger.py"])
        .contains("not an admin"));
    assert!(fixture.ok(&["init"]).contains("this machine: member"));
    let agent = fixture
        .command(&["admin", "add", &"a".repeat(64)])
        .env("CLAUDECODE", "1")
        .output()
        .unwrap();
    assert!(!agent.status.success(), "agents cannot change admins");
}

/** The owner adds an admin who then governs from their own machine; other teammates trust the
 * admin through the owner; after removal the admin can no longer govern, and a restored old admin
 * list is rejected by machines that saw the removal
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn owner_adds_and_removes_admins() {
    let mut fixture = Fixture::ready();
    let owner = machine(&mut fixture, "bob");
    let bob = joined_key(&join(&fixture, "y"));
    assert!(fixture
        .fails(&["admin", "add", &"b".repeat(64)])
        .contains("only the repository owner"));
    fixture.home = owner.clone();
    assert!(fixture
        .ok(&["admin", "add", &bob, "name", "bob"])
        .contains("Added admin"));
    assert!(fixture.ok(&["admin", "list"]).contains("bob"));
    fixture.home = fixture.root.path().join("bob");
    fixture.protect(&["app/ledger.py"]);
    fixture.ok(&["validate"]);
    let snapshot = fixture.root.path().join("snapshot");
    copy_tree(&fixture.work.join(".crane"), &snapshot);
    fixture.home = owner.clone();
    fixture.ok(&["validate"]);
    machine(&mut fixture, "carol");
    assert!(join(&fixture, "y").contains("Trusted."));
    fixture.ok(&["validate"]);
    fixture.home = owner.clone();
    assert!(fixture
        .ok(&["admin", "del", &bob])
        .contains("Removed admin"));
    fixture.home = fixture.root.path().join("carol");
    fixture.ok(&["validate"]);
    fixture.home = fixture.root.path().join("bob");
    assert!(fixture
        .fails(&["checkpoint", "after-removal"])
        .contains("not an admin"));
    // Bob puts back the .crane from when he was an admin
    copy_tree(&snapshot, &fixture.work.join(".crane"));
    fixture.home = fixture.root.path().join("carol");
    assert!(fixture.fails(&["validate"]).contains("admins_rollback"));
}

/** Two admins change the rules from the same generation; after keeping the other admin's side, the
 * first admin's machine reports a fork until crane init accepts it, then can sign on top; merge
 * conflict markers in .crane are explained
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn forks_are_accepted_explicitly_and_conflicts_explained() {
    let mut fixture = Fixture::ready();
    let owner = machine(&mut fixture, "bob");
    let bob = joined_key(&join(&fixture, "y"));
    fixture.home = owner.clone();
    fixture.ok(&["admin", "add", &bob, "name", "bob"]);
    let base = fixture.root.path().join("base");
    copy_tree(&fixture.work.join(".crane"), &base);
    fixture.ok(&["checkpoint", "owner-side"]);
    copy_tree(&base, &fixture.work.join(".crane"));
    fixture.home = fixture.root.path().join("bob");
    fixture.ok(&["checkpoint", "bob-side"]);
    fixture.home = owner;
    assert!(fixture.fails(&["validate"]).contains("registry_fork"));
    let declined = common::text(&fixture.with_input(&["init"], "n\n"));
    assert!(declined.contains("not accepted"), "{declined}");
    let accepted = common::text(&fixture.with_input(&["init"], "y\n"));
    assert!(accepted.contains("Accepted generation"), "{accepted}");
    fixture.ok(&["validate"]);
    fixture.ok(&["checkpoint", "owner-side"]);
    let config = fixture.work.join(".crane/config.json");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        format!("<<<<<<< HEAD\n{text}=======\n{text}>>>>>>> bob\n"),
    )
    .unwrap();
    assert!(fixture
        .fails(&["validate"])
        .contains("unresolved merge conflicts"));
}
