// The upload pipeline (built with --features server). An unreachable service never weakens
// enforcement or loses local evidence. With a live MongoDB (CRANE_TEST_MONGODB_URI), a real
// 'crane server' receives signed uploads with no setup on the client, stores each machine's
// repository status (events, ledger, and alerts stay on the machine), rejects forged requests,
// and enforces its limits. With CRANE_TEST_INGEST_URL, the same uploads reach a deployed service.

mod common;
mod support;

use std::io::Write;
use std::process::Stdio;

use common::{text, Fixture};
use serde_json::json;

/** An address where nothing listens */
const UNREACHABLE: &str = "http://127.0.0.1:9";

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

/** Count the events this machine holds locally
 * Input
    - fixture: &Fixture - repository (on the current machine)
 * Output
    - u64
*/
fn local_events(fixture: &Fixture) -> u64 {
    std::fs::read_to_string(
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
    .count() as u64
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
        session.contains("status: ENDED") && session.contains("denied:"),
        "{session}"
    );
    // The upload fails; under load the hooks' own retries may still hold the sync lock
    let failed = fixture.fails(&["agent", "sync"]);
    assert!(
        failed.contains("unreachable") || failed.contains("timed out waiting for lock"),
        "{failed}"
    );
    if failed.contains("unreachable") {
        assert!(
            failed.contains("(request "),
            "failures name their correlation id: {failed}"
        );
    }
    assert!(
        local_events(&fixture) >= 4,
        "the evidence stays on the machine"
    );
}

/** The live pipeline (needs CRANE_TEST_MONGODB_URI): with only the service address set, hooks and
 * commands reach the service through signed uploads; the repository and its status (flows and
 * Bricks) are stored and the remote is verified, while events, the ledger, governance changes,
 * and alerts never leave the machine; forged requests are refused
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn live_pipeline_reaches_mongodb_without_setup() {
    use mongodb::bson::doc;
    let Some(uri) = support::mongodb() else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run against a live MongoDB");
        return;
    };
    let name = format!("crane_test_{}", std::process::id());
    let running = support::service(&uri, &name, &[]);
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
    assert!(synced.contains("Sent this repository's status"), "{synced}");
    // Let the detached syncs started by the hooks finish
    std::thread::sleep(std::time::Duration::from_secs(5));
    let database = mongodb::sync::Client::with_uri_str(&uri)
        .unwrap()
        .database(&name);
    let count = |collection: &str, filter| {
        database
            .collection::<mongodb::bson::Document>(collection)
            .count_documents(filter)
            .run()
            .unwrap()
    };
    for stream in ["events", "ledger", "audit", "alerts"] {
        assert_eq!(
            count("ingestion_checkpoints", doc! {"stream": stream}),
            0,
            "{stream} stay on the machine"
        );
    }
    assert!(local_events(&fixture) >= 4, "the evidence is kept locally");
    let names = database.list_collection_names().run().unwrap();
    for retired in [
        "events",
        "sessions",
        "tasks",
        "agents",
        "alerts",
        "autonomy_ledger",
        "governance_changes",
        "selection_registry",
        "policy_versions",
        "chain_heads",
        "service_events",
    ] {
        assert!(
            !names.iter().any(|found| found == retired),
            "{retired} is not stored: {names:?}"
        );
    }
    assert_eq!(
        count(
            "ingestion_checkpoints",
            doc! {"installation": {"$regex": "^[0-9a-f]{64}$"}}
        ),
        count("ingestion_checkpoints", doc! {}),
        "every record is tagged with the machine's upload key"
    );
    assert_eq!(
        count("repositories", doc! {"verified": true}),
        1,
        "the upload proved its remote"
    );
    let status = database
        .collection::<mongodb::bson::Document>("repository_status")
        .find_one(doc! {})
        .run()
        .unwrap()
        .expect("the repository status was uploaded");
    assert!(status.contains_key("flows"), "the Flows section reads it");
    assert!(status.contains_key("bricks"), "the Flows section reads it");
    for local in [
        "governance",
        "integrity",
        "compliance_inputs",
        "integrations",
        "alerting",
        "residents",
    ] {
        assert!(!status.contains_key(local), "{local} stays on the machine");
    }
    // A request without a valid signature is refused
    let forged = support::request(
        "GET",
        &format!(
            "{}/v1/repositories/{}/positions",
            running.url,
            "a".repeat(128)
        ),
        &[
            ("X-Crane-Installation", "b".repeat(64).as_str()),
            ("X-Crane-Timestamp", "1"),
            ("X-Crane-Signature", "c".repeat(128).as_str()),
        ],
    );
    assert_eq!(forged.status, 401, "{}", forged.body);
    let id = forged.header("x-request-id").unwrap().to_string();
    assert_eq!(
        forged.json()["request_id"],
        id.as_str(),
        "errors name their request"
    );
    assert!(running
        .log_line(&format!("\"request_id\":\"{id}\""))
        .is_some());
    database.drop().run().unwrap();
}

/** The schema 14 migration (needs CRANE_TEST_MONGODB_URI): a database written by an earlier
 * service keeps every machine's upload positions (the chain head, events that arrived expired,
 * stored events, ledger entries, governance changes, and uploaded alerts) and loses the telemetry
 * and dashboard collections, while approvals, Brick requests, and the session key stay
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn migration_keeps_positions_and_drops_telemetry() {
    use mongodb::bson::{doc, Document};
    let Some(uri) = support::mongodb() else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run against a live MongoDB");
        return;
    };
    let name = format!("crane_migrate_{}", std::process::id());
    let database = mongodb::sync::Client::with_uri_str(&uri)
        .unwrap()
        .database(&name);
    let insert = |collection: &str, documents: Vec<Document>| {
        database
            .collection::<Document>(collection)
            .insert_many(documents)
            .run()
            .unwrap();
    };
    let repository = "r".repeat(128);
    let scope =
        |installation: &str| doc! {"repository_id": &repository, "installation": installation};
    let with = |installation: &str, extra: Document| {
        let mut document = scope(installation);
        document.extend(extra);
        document
    };
    let (a, b) = ("a".repeat(64), "b".repeat(64));
    insert(
        "schema_migrations",
        vec![doc! {"version": 13_i64, "applied_at": 1_i64}],
    );
    insert(
        "chain_heads",
        vec![
            with(
                &a,
                doc! {"sequence": 7_i64, "digest": "d", "expired_through": 9_i64},
            ),
            with(&b, doc! {"sequence": 4_i64, "digest": "d"}),
        ],
    );
    insert(
        "events",
        vec![
            with(&a, doc! {"sequence": 5_i64}),
            with(&b, doc! {"sequence": 6_i64}),
        ],
    );
    insert(
        "autonomy_ledger",
        vec![
            with(&a, doc! {"seq": 2_i64}),
            with(&a, doc! {"seq": 11_i64}),
        ],
    );
    insert("governance_changes", vec![with(&b, doc! {"seq": 3_i64})]);
    insert(
        "alerts",
        vec![
            with(&a, doc! {"alert_id": "1"}),
            with(&a, doc! {"alert_id": "2"}),
            with(&a, doc! {"alert_id": "3", "source": "service"}),
        ],
    );
    insert(
        "ingestion_checkpoints",
        vec![with(&a, doc! {"stream": "alerts", "count": 1_i64})],
    );
    for retired in [
        "sessions",
        "service_events",
        "credit_grants",
        "dashboard_agents",
    ] {
        insert(retired, vec![with(&a, doc! {"x": 1})]);
    }
    insert(
        "brick_requests",
        vec![
            doc! {"request_id": "brq_kept", "repository_id": &repository, "op": "flow-discard", "requested_by": "alice", "status": "PENDING"},
        ],
    );
    insert(
        "service_settings",
        vec![
            doc! {"name": "session_key", "seed": "5".repeat(64)},
            doc! {"name": "retention", "retention_ms": 1_i64},
        ],
    );
    let _running = support::service(&uri, &name, &[]);
    let position = |installation: &str, stream: &str, field: &str| {
        let mut filter = scope(installation);
        filter.insert("stream", stream);
        database
            .collection::<Document>("ingestion_checkpoints")
            .find_one(filter)
            .run()
            .unwrap()
            .and_then(|document| document.get_i64(field).ok())
    };
    assert_eq!(
        position(&a, "events", "seq"),
        Some(9),
        "events that arrived expired"
    );
    assert_eq!(
        position(&b, "events", "seq"),
        Some(6),
        "stored events past the head"
    );
    assert_eq!(position(&a, "ledger", "seq"), Some(11));
    assert_eq!(position(&b, "audit", "seq"), Some(3));
    assert_eq!(
        position(&a, "alerts", "count"),
        Some(2),
        "uploaded alerts only"
    );
    let names = database.list_collection_names().run().unwrap();
    for retired in [
        "chain_heads",
        "events",
        "autonomy_ledger",
        "governance_changes",
        "alerts",
        "sessions",
        "service_events",
        "credit_grants",
        "dashboard_agents",
    ] {
        assert!(
            !names.iter().any(|found| found == retired),
            "{retired}: {names:?}"
        );
    }
    let settings = database.collection::<Document>("service_settings");
    assert!(settings
        .find_one(doc! {"name": "retention"})
        .run()
        .unwrap()
        .is_none());
    assert!(settings
        .find_one(doc! {"name": "session_key"})
        .run()
        .unwrap()
        .is_some());
    assert!(database
        .collection::<Document>("brick_requests")
        .find_one(doc! {"request_id": "brq_kept"})
        .run()
        .unwrap()
        .is_some());
    assert!(database
        .collection::<Document>("schema_migrations")
        .find_one(doc! {"version": 14_i64})
        .run()
        .unwrap()
        .is_some());
    database.drop().run().unwrap();
}

/** A deployed service (needs CRANE_TEST_INGEST_URL): a fresh machine with only the address set
 * sends its repository status, and a teammate's machine without a repository signing key sends
 * with its own upload key
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
    // Let the detached syncs started by the hooks finish
    std::thread::sleep(std::time::Duration::from_secs(5));
    let synced = fixture.ok(&["agent", "sync"]);
    assert!(synced.contains("Sent this repository's status"), "{synced}");
    // A teammate's machine: same clone, its own trust directory, no repository signing key
    let owner_home = fixture.home.clone();
    fixture.home = fixture.root.path().join("teammate-trust");
    let _ = fixture.command(&["session", "."]).output().unwrap();
    let synced = fixture.ok(&["agent", "sync"]);
    assert!(synced.contains("Sent this repository's status"), "{synced}");
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
}

/** The service's limits (needs CRANE_TEST_MONGODB_URI): a rate per installation answers 429 and
 * a full database answers 507; the machine's records stay on it (the batch-size limit, 413, only
 * concerns released versions that still upload records)
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn upload_limits_protect_the_database() {
    let Some(uri) = support::mongodb() else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run against a live MongoDB");
        return;
    };
    let mut fixture = Fixture::ready();
    let cases: [(&str, &str, &str); 2] = [
        ("CRANE_SERVER_REQUESTS_PER_MINUTE", "3", "HTTP 429"),
        ("CRANE_SERVER_MAX_STORAGE_MB", "1", "HTTP 507"),
    ];
    for (index, (variable, value, expected)) in cases.iter().enumerate() {
        let name = format!("crane_limits_{}_{index}", std::process::id());
        // A database larger than 1 MB, so the storage limit applies
        if *variable == "CRANE_SERVER_MAX_STORAGE_MB" {
            let client = mongodb::sync::Client::with_uri_str(&uri).unwrap();
            let filler = (0..300)
                .map(|n| mongodb::bson::doc! {"n": n, "pad": "x".repeat(8_000)})
                .collect::<Vec<_>>();
            client
                .database(&name)
                .collection::<mongodb::bson::Document>("filler")
                .insert_many(filler)
                .run()
                .unwrap();
        }
        let limited = support::service(&uri, &name, &[(variable, value)]);
        fixture.ingest_url = limited.url.clone();
        // Each manual sync is one request: the rate limit answers within a few of them
        let mut answers = Vec::new();
        for _ in 0..5 {
            let output = fixture.command(&["agent", "sync"]).output().unwrap();
            answers.push(text(&output));
            if !output.status.success() {
                break;
            }
        }
        let refused = answers.last().unwrap();
        assert!(
            refused.contains(expected),
            "{variable}={value}: {answers:?}"
        );
        drop(limited);
        mongodb::sync::Client::with_uri_str(&uri)
            .unwrap()
            .database(&name)
            .drop()
            .run()
            .unwrap();
    }
    assert!(fixture.ok(&["session", "."]).contains("sessions"));
}
