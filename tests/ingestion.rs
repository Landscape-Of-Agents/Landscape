// The Foxx ingestion pipeline (built with --features server). An unreachable service never weakens
// enforcement, never loses local evidence, and never stops the dashboard. With a live MongoDB
// (CRANE_TEST_MONGODB_URI), a real 'crane server' receives signed uploads with no setup on the
// client, rejects forged requests, re-receives records missing from the database, and serves the
// dashboard.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

use common::{text, Fixture};
use serde_json::json;

/** An address where nothing listens */
const UNREACHABLE: &str = "http://127.0.0.1:9";

/** A running ingestion service, stopped on drop
 * Fields
    - child: Child - the crane server process
    - url: String - its base URL
*/
struct Service {
    child: Child,
    url: String,
}

impl Drop for Service {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/** Start 'crane server' on a free local port
 * Input
    - uri: &str - MongoDB connection string (given to the service only)
    - database: &str - database name
 * Output
    - Service
*/
fn service(uri: &str, database: &str) -> Service {
    let mut child = Command::new(env!("CARGO_BIN_EXE_crane"))
        .args(["server", "--listen", "127.0.0.1:0"])
        .env("CRANE_MONGODB_URI", uri)
        .env("CRANE_MONGODB_DB", database)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let address = line
        .split("listening on ")
        .nth(1)
        .and_then(|rest| rest.split(' ').next())
        .unwrap_or_else(|| panic!("the service did not start: {line}"))
        .to_string();
    Service {
        child,
        url: format!("http://{address}"),
    }
}

/** Run a hook and return its output
 * Input
    - fixture: &Fixture - repository
    - event: &str - hook event
    - payload: serde_json::Value - payload
 * Output
    - std::process::Output
*/
fn hook(fixture: &Fixture, event: &str, payload: serde_json::Value) -> std::process::Output {
    let mut child = fixture
        .command(&["agent", "hook", "--event", event, "--profile", "claude"])
        .env("CLAUDECODE", "1")
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

/** Start the dashboard for one request and fetch an API path with its token
 * Input
    - fixture: &Fixture - repository
    - path: &str - API path
 * Output
    - (String, String) the HTTP response and the dashboard's stderr
*/
fn dashboard(fixture: &Fixture, path: &str) -> (String, String) {
    let mut dashboard = fixture
        .command(&["dashboard"])
        .env("CRANE_DASHBOARD_MAX_REQUESTS", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(dashboard.stdout.as_mut().unwrap())
        .read_line(&mut line)
        .unwrap();
    let token = line.split("#token=").nth(1).unwrap().trim().to_string();
    let port = line[line.find("127.0.0.1:").unwrap() + 10..]
        .split('/')
        .next()
        .unwrap()
        .to_string();
    let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
    let output = dashboard.wait_with_output().unwrap();
    (
        response,
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/** Enforcement and local evidence are unaffected when the service is unreachable
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn service_outage_does_not_weaken_enforcement() {
    let mut fixture = Fixture::ready();
    fixture.ingest_url = UNREACHABLE.into();
    fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    assert!(hook(&fixture, "SessionStart", json!({"session_id": "m1"}))
        .status
        .success());
    let denied = hook(
        &fixture,
        "pre-tool-use",
        json!({"session_id": "m1", "tool_use_id": "t1", "cwd": fixture.work.to_string_lossy(), "tool_name": "Edit", "tool_input": {"file_path": fixture.work.join("app/payments.py").to_string_lossy(), "old_string": "0.03", "new_string": "0.3"}}),
    );
    assert_eq!(denied.status.code(), Some(2), "{}", text(&denied));
    let end = hook(&fixture, "SessionEnd", json!({"session_id": "m1"}));
    assert!(
        end.status.success(),
        "the session ends even though the service is down: {}",
        text(&end)
    );
    let session = fixture.ok(&["session", "current"]);
    assert!(
        session.contains("status: ENDED") && session.contains("actions_denied"),
        "{session}"
    );
    // The upload fails; under load the hooks' own retries may still hold the sync lock
    let failed = fixture.fails(&["agent", "sync"]);
    assert!(
        failed.contains("unreachable") || failed.contains("timed out waiting for lock"),
        "{failed}"
    );
    let (response, stderr) = dashboard(&fixture, "/api/health");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert!(response.contains("\"source\":\"local\""), "{response}");
    assert!(stderr.contains("synchronization failed"), "{stderr}");
}

/** The live pipeline (needs CRANE_TEST_MONGODB_URI, skipped otherwise): with only the service
 * address set, hooks and commands reach MongoDB through signed uploads, rollups and alerts are
 * stored, forged requests are refused, records missing from MongoDB are re-sent, and the dashboard
 * reads from the service
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn live_pipeline_reaches_mongodb_without_setup() {
    use mongodb::bson::doc;
    let Ok(uri) = std::env::var("CRANE_TEST_MONGODB_URI") else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run against a live MongoDB");
        return;
    };
    let name = format!("crane_test_{}", std::process::id());
    let running = service(&uri, &name);
    let mut fixture = Fixture::ready();
    fixture.ingest_url = running.url.clone();
    fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let call = |id: &str, command: &str| json!({"session_id": "live-1", "tool_use_id": id, "cwd": fixture.work.to_string_lossy(), "tool_name": "Bash", "tool_input": {"command": command}});
    fixture.hook("claude", "SessionStart", &json!({"session_id": "live-1"}));
    fixture.hook("claude", "pre-tool-use", &call("l1", "echo ok"));
    fixture.hook("claude", "post-tool-use", &call("l1", "echo ok"));
    fixture.hook("claude", "pre-tool-use", &call("l2", "cat .crane/map"));
    fixture.hook("claude", "SessionEnd", &json!({"session_id": "live-1"}));
    let synced = fixture.ok(&["agent", "sync"]);
    assert!(synced.contains("Synchronized"), "{synced}");
    let client = mongodb::sync::Client::with_uri_str(&uri).unwrap();
    let database = client.database(&name);
    let count = |collection: &str, filter| {
        database
            .collection::<mongodb::bson::Document>(collection)
            .count_documents(filter)
            .run()
            .unwrap()
    };
    let local_events = std::fs::read_to_string(
        std::fs::read_dir(fixture.home.join("repos"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("runtime/events.jsonl"),
    )
    .unwrap()
    .lines()
    .count() as u64;
    assert_eq!(count("events", doc! {}), local_events);
    assert_eq!(
        count(
            "events",
            doc! {"installation": {"$regex": "^[0-9a-f]{64}$"}}
        ),
        local_events,
        "every record is tagged with the installation key"
    );
    assert!(count("events", doc! {"event_type": "command.executed"}) >= 2);
    assert!(count("events", doc! {"event_type": "tool_call.verified"}) >= 1);
    assert!(count("alerts", doc! {"kind": "bypass_attempt"}) >= 1);
    assert!(count("metric_rollups", doc! {"scope": "session"}) >= 1);
    assert!(count("metric_rollups", doc! {"scope": "tool_call"}) >= 1);
    assert_eq!(count("sessions", doc! {}), 1);
    assert!(count("governance_changes", doc! {}) >= 3);
    assert_eq!(count("policy_versions", doc! {}), 1);
    // A request without a valid signature is refused
    let address = running.url.trim_start_matches("http://").to_string();
    let mut stream = std::net::TcpStream::connect(&address).unwrap();
    write!(
        stream,
        "GET /v1/repositories/{}/snapshot HTTP/1.1\r\nHost: {address}\r\nX-Crane-Installation: {}\r\nX-Crane-Timestamp: {}\r\nX-Crane-Signature: {}\r\nConnection: close\r\n\r\n",
        "a".repeat(128),
        "b".repeat(64),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis(),
        "c".repeat(128)
    )
    .unwrap();
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 401"), "{response}");
    // Let the detached syncs started by the hooks finish before simulating data loss
    std::thread::sleep(std::time::Duration::from_secs(5));
    database
        .collection::<mongodb::bson::Document>("events")
        .delete_many(doc! {"sequence": {"$gt": 3}})
        .run()
        .unwrap();
    assert!(fixture.ok(&["agent", "sync"]).contains("re-sent"));
    assert_eq!(count("events", doc! {}), local_events);
    let (response, _) = dashboard(&fixture, "/api/health");
    assert!(response.contains("\"source\":\"service\""), "{response}");
    assert!(response.contains("\"status\":\"verified\""), "{response}");
    database.drop().run().unwrap();
}

/** A deployed service (needs CRANE_TEST_INGEST_URL, skipped otherwise): a fresh installation with
 * only the address set uploads its commands, session, and tool calls, and its dashboard reads the
 * same verified event chain back from the service
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn deployed_service_round_trip() {
    let Ok(url) = std::env::var("CRANE_TEST_INGEST_URL") else {
        eprintln!("skipped: set CRANE_TEST_INGEST_URL to test a deployed ingestion service");
        return;
    };
    let mut fixture = Fixture::ready();
    fixture.ingest_url = url;
    fixture.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let call = |id: &str| json!({"session_id": "deployed-1", "tool_use_id": id, "cwd": fixture.work.to_string_lossy(), "tool_name": "Bash", "tool_input": {"command": "echo ok"}});
    fixture.hook(
        "claude",
        "SessionStart",
        &json!({"session_id": "deployed-1"}),
    );
    fixture.hook("claude", "pre-tool-use", &call("d1"));
    fixture.hook("claude", "post-tool-use", &call("d1"));
    fixture.hook("claude", "SessionEnd", &json!({"session_id": "deployed-1"}));
    // Let the detached syncs started by the hooks finish, then upload the rest
    std::thread::sleep(std::time::Duration::from_secs(5));
    let synced = fixture.ok(&["agent", "sync"]);
    assert!(synced.contains("Synchronized"), "{synced}");
    let local_events = std::fs::read_to_string(
        std::fs::read_dir(fixture.home.join("repos"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("runtime/events.jsonl"),
    )
    .unwrap()
    .lines()
    .count();
    let (response, _) = dashboard(&fixture, "/api/health");
    assert!(response.contains("\"source\":\"service\""), "{response}");
    assert!(response.contains("\"status\":\"verified\""), "{response}");
    assert!(
        response.contains(&format!("\"events\":{local_events}")),
        "the service holds every local event ({local_events}): {response}"
    );
    // A teammate's machine: same clone, its own trust directory, no repository signing key
    let owner_home = fixture.home.clone();
    fixture.home = fixture.root.path().join("teammate-trust");
    let _ = fixture.command(&["session", "."]).output().unwrap();
    let synced = fixture.ok(&["agent", "sync"]);
    assert!(synced.contains("Synchronized"), "{synced}");
    assert!(
        !fixture.home.join("keys").exists(),
        "the teammate never needed a repository signing key"
    );
    let teammate_key = std::fs::read_to_string(fixture.home.join("installation.key")).unwrap();
    let owner_key = std::fs::read_to_string(owner_home.join("installation.key")).unwrap();
    assert_ne!(
        teammate_key, owner_key,
        "each machine has its own upload key"
    );
    let teammate_events = std::fs::read_to_string(
        std::fs::read_dir(fixture.home.join("repos"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path()
            .join("runtime/events.jsonl"),
    )
    .unwrap()
    .lines()
    .count();
    assert_ne!(teammate_events, local_events);
    let (response, _) = dashboard(&fixture, "/api/health");
    assert!(response.contains("\"source\":\"service\""), "{response}");
    assert!(
        response.contains(&format!("\"events\":{teammate_events}")),
        "the teammate reads exactly its own {teammate_events} events: {response}"
    );
}
