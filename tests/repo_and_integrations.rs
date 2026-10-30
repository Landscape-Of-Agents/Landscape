// Repository connection commands, the GitHub login grant limits, and interactive integrations
// whose secrets never enter the repository.

mod common;

use common::{text, Fixture};

/** The repository connection moves through NOT_CONNECTED, CONNECTED, and back, records
 * privileges, and can be rewritten with new privileges
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn repo_connection_lifecycle() {
    let fixture = Fixture::new();
    assert_eq!(fixture.ok(&["repo", "status"]).trim(), "NOT_CONNECTED");
    assert!(fixture.ok(&["repo", "--view"]).contains("NOT_CONNECTED"));
    let remote = fixture.remote_url();
    let connected = fixture.ok(&["repo", "--https", &remote, "rf", "gh-actions"]);
    assert!(connected.contains("CONNECTED"), "{connected}");
    assert_eq!(fixture.ok(&["repo", "status"]).trim(), "CONNECTED");
    let view = fixture.ok(&["repo", "--view"]);
    assert!(view.contains("[x] rf"), "{view}");
    assert!(view.contains("[x] gh-actions"), "{view}");
    assert!(view.contains("[ ] gh-insights"), "{view}");
    fixture.ok(&["repo", "--ssh", &remote, "gh-insights"]);
    let view = fixture.ok(&["repo", "--view"]);
    assert!(
        view.contains("[x] gh-insights") && view.contains("[ ] rf"),
        "privileges are rewritten: {view}"
    );
    assert!(view.contains("method: ssh"), "{view}");
    fixture.ok(&["repo", "del"]);
    assert_eq!(fixture.ok(&["repo", "status"]).trim(), "NOT_CONNECTED");
    assert!(
        !fixture.work.join(".crane").exists(),
        "connecting writes nothing into the repository"
    );
}

/** Wrong remotes, unknown privileges, non-GitHub hosts, and broken connections are reported
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn repo_connection_errors() {
    let fixture = Fixture::new();
    let other = fixture.root.path().join("other.git");
    common::git(
        fixture.root.path(),
        &["init", "-q", "--bare", other.to_str().unwrap()],
    );
    let wrong = fixture.fails(&["repo", "--https", other.to_str().unwrap()]);
    assert!(wrong.contains("origin"), "{wrong}");
    let privilege = fixture.fails(&["repo", "--https", &fixture.remote_url(), "everything"]);
    assert!(privilege.contains("unknown privilege"), "{privilege}");
    let gitlab = fixture.fails(&["repo", "--https", "https://gitlab.com/acme/shop"]);
    assert!(gitlab.contains("not a GitHub host"), "{gitlab}");
    let method = fixture.fails(&["repo", "--gh-cli", "not-a-slug"]);
    assert!(method.contains("OWNER/REPOSITORY"), "{method}");
    let local = fixture
        .command(&["repo", "--https", &fixture.remote_url()])
        .env_remove("CRANE_ALLOW_LOCAL_REMOTE")
        .output()
        .unwrap();
    assert!(
        !local.status.success() && text(&local).contains("not GitHub repositories"),
        "{}",
        text(&local)
    );
    fixture.ok(&["repo", "--https", &fixture.remote_url()]);
    std::fs::rename(&fixture.remote, fixture.root.path().join("gone.git")).unwrap();
    let status = fixture.fails(&["repo", "status"]);
    assert!(status.contains("FAILURE"), "{status}");
    assert!(fixture.fails(&["repo", "connect"]).contains("usage"));
}

/** crane github days accepts 1 to 180 days only
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn github_grant_limits() {
    let fixture = Fixture::new();
    for days in ["0", "181", "-5", "many"] {
        let output = fixture.fails(&["github", "days", days]);
        assert!(output.contains("DAYS"), "{days}: {output}");
    }
    assert!(fixture.fails(&["github", "weeks", "2"]).contains("usage"));
    let agent = fixture.as_agent(&["github"]);
    assert!(!agent.status.success() && text(&agent).contains("cannot be executed by an AI agent"));
}

/** Integrations ask for each service's parameters, validate them, and keep secrets in the trust
 * directory only
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn integrations_store_secrets_outside_the_repository() {
    let fixture = Fixture::ready();
    let slack = fixture.with_input(
        &["integrate", "slack"],
        "1\nhttps://hooks.slack.com/services/T000/B000/XYZSECRET\n\n2\n\ny\n",
    );
    assert!(slack.status.success(), "{}", text(&slack));
    assert!(text(&slack).contains("Saved the slack integration"));
    let jira = fixture.with_input(
        &["integrate", "jira"],
        "1\nhttps://acme.atlassian.net\n1\ndev@acme.com\njira-token-SECRET\nPAY\n2\n1\n\ny\n",
    );
    assert!(jira.status.success(), "{}", text(&jira));
    let whatsapp = fixture.with_input(
        &["integrate", "whatsapp"],
        "1\n1234567890\n9876543210\nEAAG-whatsapp-SECRET\n+15551234567, +447700900123\n1\ncrane_alert\nen_US\n\n\ny\n",
    );
    assert!(whatsapp.status.success(), "{}", text(&whatsapp));
    let github = fixture.with_input(
        &["integrate", "github"],
        "2\nacme/shop\n\ngithub_pat_SECRETSECRETSECRET\n1\ny\n",
    );
    assert!(github.status.success(), "{}", text(&github));
    let invalid = fixture.with_input(&["integrate", "jira"], "1\nhttp://jira.local\n");
    assert!(
        !invalid.status.success() && text(&invalid).contains("https"),
        "{}",
        text(&invalid)
    );
    let cancelled = fixture.with_input(
        &["integrate", "slack"],
        "1\nhttps://hooks.slack.com/services/T/B/C\n\n2\n\nn\n",
    );
    assert!(!cancelled.status.success() && text(&cancelled).contains("cancelled"));
    for secret in [
        "XYZSECRET",
        "jira-token-SECRET",
        "EAAG-whatsapp-SECRET",
        "github_pat_SECRET",
    ] {
        let found = walk(&fixture.work).into_iter().any(|path| {
            std::fs::read_to_string(&path).is_ok_and(|content| content.contains(secret))
        });
        assert!(!found, "{secret} must not be written into the repository");
        let in_trust = walk(&fixture.home).into_iter().any(|path| {
            std::fs::read_to_string(&path).is_ok_and(|content| content.contains(secret))
        });
        assert!(in_trust, "{secret} is stored in the trust directory");
    }
    assert!(fixture
        .fails(&["integrate", "teams"])
        .contains("unknown integration"));
}

/** List every file under a directory, skipping .git
 * Input
    - root: &std::path::Path - directory
 * Output
    - Vec<PathBuf>
*/
fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}

/** A repository that moved (transferred to an organization, or renamed) is rebound: the move
 * blocks agents until the owner proves it with the old remote, after which every record, the
 * manifest, and this machine's trust data belong to the new remote; a wrong old remote and an
 * agent are refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn moved_repository_is_rebound_by_its_owner() {
    let fixture = Fixture::ready();
    let selection = fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "2"]);
    // An owner-signed admin list is bound to the repository too
    let teammate = "c".repeat(64);
    assert!(fixture
        .ok(&["admin", "add", &teammate, "name", "teammate"])
        .contains("Added admin"));
    let old_remote = fixture.remote_url();
    let new_remote = fixture.root.path().join("moved.git");
    common::git(
        fixture.root.path(),
        &[
            "clone",
            "-q",
            "--bare",
            &old_remote,
            new_remote.to_str().unwrap(),
        ],
    );
    common::git(
        &fixture.work,
        &["remote", "set-url", "origin", new_remote.to_str().unwrap()],
    );
    let moved = fixture.fails(&["validate"]);
    assert!(moved.contains("registry_wrong_repository"), "{moved}");
    // An agent cannot rebind, and a remote the state was not signed for is refused
    let agent = fixture.as_agent(&["repo", "rebind", &old_remote]);
    assert!(!agent.status.success(), "{}", text(&agent));
    let wrong = fixture.fails(&["repo", "rebind", "https://github.com/someone/else"]);
    assert!(wrong.contains("was not signed for"), "{wrong}");
    // The owner proves the move with the old remote and signs again
    let rebound = fixture.ok(&["repo", "rebind", &old_remote]);
    assert!(
        rebound.contains("1 selection(s), the admin list, and the manifest signed again"),
        "{rebound}"
    );
    assert!(
        fixture.ok(&["validate"]).contains("PASS"),
        "the rebound state verifies"
    );
    assert!(
        fixture.ok(&["test", "."]).contains("PASS"),
        "the selection still resolves"
    );
    let registry = fixture.crane_json("registry.json");
    let record = registry["selections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|record| record["id"] == selection.as_str())
        .unwrap();
    assert_eq!(
        record["repository_id"],
        registry["manifest"]["repository_id"]
    );
    assert!(
        fixture.ok(&["admin", "list"]).contains("teammate"),
        "the re-signed admin list verifies"
    );
    // Running it again (as a teammate would) only carries trust data over
    let again = fixture.ok(&["repo", "rebind", &old_remote]);
    assert!(again.contains("already belongs to"), "{again}");
    assert!(fixture.ok(&["validate"]).contains("PASS"));
}

/** Answer HTTP requests on a local port with one status, for as many requests as come
 * Input
    - status: &'static str - status line, such as "200 OK"
 * Output
    - u16 the port
*/
fn local_service(status: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut buffer = [0u8; 2048];
            let _ = std::io::Read::read(&mut stream, &mut buffer);
            let _ = std::io::Write::write_all(
                &mut stream,
                format!("HTTP/1.1 {status}\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                    .as_bytes(),
            );
        }
    });
    port
}

/** An integration set without prompts keeps an address only after it answers, keeps nothing when
 * one does not, takes credentials only on standard input, and reports what is still missing
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn integrations_keep_addresses_only_after_they_answer() {
    let fixture = Fixture::ready();
    let up = local_service("200 OK");
    let missing = local_service("404 Not Found");
    // A path with nothing at it is refused, and nothing is saved
    let refused = fixture.run(&[
        "integrate",
        "grafana",
        "--settings",
        &format!("{{\"url\":\"http://127.0.0.1:{missing}/grafana\"}}"),
    ]);
    assert!(!refused.status.success());
    assert!(text(&refused).contains("HTTP 404"), "{}", text(&refused));
    assert!(
        text(&refused).contains("nothing was saved"),
        "{}",
        text(&refused)
    );
    // An address that does not answer at all is refused too
    let silent = fixture.run(&[
        "integrate",
        "grafana",
        "--settings",
        "{\"url\":\"http://127.0.0.1:1\"}",
    ]);
    assert!(!silent.status.success(), "{}", text(&silent));
    let status = |fixture: &Fixture| {
        serde_json::from_str::<serde_json::Value>(&fixture.ok(&["integrate", "--status", "--json"]))
            .unwrap()["data"]["integrations"]["grafana"]
            .clone()
    };
    assert_eq!(status(&fixture)["configured"], false, "nothing was kept");
    // An address that answers is kept; the credential is still missing
    let kept: serde_json::Value = serde_json::from_str(&fixture.ok(&[
        "integrate",
        "grafana",
        "--settings",
        &format!("{{\"url\":\"http://127.0.0.1:{up}\"}}"),
        "--json",
    ]))
    .unwrap();
    assert!(
        kept["data"]["checked"][0]
            .as_str()
            .unwrap()
            .contains("answered HTTP 200"),
        "{kept}"
    );
    assert_eq!(kept["data"]["missing_secrets"][0]["key"], "token", "{kept}");
    // Credentials come on standard input, never as arguments, and connect it
    let connected = fixture.with_input(
        &["integrate", "grafana", "--secrets-stdin", "--json"],
        "{\"token\":\"glsa_test\"}",
    );
    assert!(connected.status.success(), "{}", text(&connected));
    let answer: serde_json::Value = serde_json::from_slice(&connected.stdout).unwrap();
    assert!(
        answer["data"]["missing_secrets"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{answer}"
    );
    assert_eq!(status(&fixture)["connected"], true);
    assert!(
        status(&fixture).get("token").is_none(),
        "credentials are never reported"
    );
    // A credential is refused among the settings
    let leaked = fixture.run(&[
        "integrate",
        "grafana",
        "--settings",
        "{\"token\":\"glsa_x\"}",
    ]);
    assert!(
        text(&leaked).contains("is a credential"),
        "{}",
        text(&leaked)
    );
}
