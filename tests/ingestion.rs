// The upload pipeline (built with --features server). An unreachable service never weakens
// enforcement or loses local evidence. With a live MongoDB (CRANE_TEST_MONGODB_URI), a real
// 'crane server' receives signed uploads with no setup on the client, stores each machine's
// repository status, rejects forged requests, re-receives records missing from the database, and
// enforces its limits. With CRANE_TEST_INGEST_URL, the same uploads reach a deployed service.

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
 * commands reach MongoDB through signed uploads, alerts, and the repository status are
 * stored, the remote is verified, forged requests are refused, and records missing from MongoDB
 * are re-sent
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
    assert!(synced.contains("Synchronized"), "{synced}");
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
    let local = local_events(&fixture);
    assert_eq!(count("events", doc! {}), local);
    assert_eq!(
        count(
            "events",
            doc! {"installation": {"$regex": "^[0-9a-f]{64}$"}}
        ),
        local,
        "every record is tagged with the machine's upload key"
    );
    assert!(count("events", doc! {"event_type": "command.executed"}) >= 2);
    assert!(count("events", doc! {"event_type": "tool_call.verified"}) >= 1);
    assert!(count("alerts", doc! {"kind": "bypass_attempt"}) >= 1);
    // The service records what each upload showed (DASHBOARD.md F02, F09, F34)
    assert_eq!(
        count("service_events", doc! {"event_name": "MACHINE_FIRST_SEEN"}),
        1
    );
    assert!(count("service_events", doc! {"event_name": "SYNC_COMPLETED"}) >= 1);
    assert!(count("service_events", doc! {"event_name": "STATUS_REPORTED"}) >= 1);
    // Every stored event carries its specification events, covered by its digest
    assert!(count("events", doc! {"spec.event_name": "TOOL_CALL_ALLOWED"}) >= 1);
    assert!(count("events", doc! {"spec.event_name": "COMMAND_EXECUTED"}) >= 2);
    assert_eq!(count("sessions", doc! {}), 1);
    assert!(count("governance_changes", doc! {}) >= 3);
    assert_eq!(count("policy_versions", doc! {}), 1);
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
    assert!(status
        .get_document("governance")
        .is_ok_and(|governance| governance.contains_key("policies")));
    assert!(status.get_document("integrity").is_ok());
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
    // Let the detached syncs started by the hooks finish before simulating data loss
    std::thread::sleep(std::time::Duration::from_secs(5));
    database
        .collection::<mongodb::bson::Document>("events")
        .delete_many(doc! {"sequence": {"$gt": 3}})
        .run()
        .unwrap();
    assert!(fixture.ok(&["agent", "sync"]).contains("re-sent"));
    assert_eq!(count("events", doc! {}), local);
    let broken = doc! {"kind": "evidence_chain_broken"};
    assert_eq!(
        count("alerts", broken.clone()),
        0,
        "re-sending is not a break"
    );
    // The machine's history is rewritten (an event that does not chain) and cut (a gap)
    let file = fixture
        .home
        .join("repos")
        .read_dir()
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("runtime/events.jsonl");
    let text = std::fs::read_to_string(&file).unwrap();
    let last: serde_json::Value = serde_json::from_str(text.lines().last().unwrap()).unwrap();
    let forged = |sequence: u64, previous: &str| {
        let mut event = last.clone();
        event["event_id"] = json!(format!("evt_forged_{sequence}"));
        event["idempotency_key"] = json!(format!("forged:{sequence}"));
        event["sequence"] = json!(sequence);
        event["previous_digest"] = json!(previous);
        event["digest"] = json!(format!("{sequence:0>128}"));
        event.to_string()
    };
    let next = last["sequence"].as_u64().unwrap() + 1;
    let appended = format!(
        "{}
{}
",
        forged(next, &"0".repeat(128)),
        forged(next + 5, &format!("{next:0>128}"))
    );
    std::fs::write(&file, text + &appended).unwrap();
    fixture.ok(&["agent", "sync"]);
    assert_eq!(count("alerts", broken.clone()), 2, "a rewrite and a gap");
    let details = database
        .collection::<mongodb::bson::Document>("alerts")
        .find(broken)
        .run()
        .unwrap()
        .map(|alert| alert.unwrap().get_str("detail").unwrap().to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    assert!(
        details.contains("does not chain") && details.contains("never arrived"),
        "{details}"
    );
    // Retention: every telemetry record expires 30 days after its time; state never does
    let dated = |collection: &str| {
        database
            .collection::<mongodb::bson::Document>(collection)
            .find(doc! {})
            .run()
            .unwrap()
            .map(|document| document.unwrap())
            .all(|document| document.get_datetime("expires_at").is_ok())
    };
    for collection in ["events", "alerts", "sessions", "service_events", "agents"] {
        assert!(dated(collection), "{collection} records expire");
    }
    assert_eq!(
        count("autonomy_ledger", doc! {"expires_at": {"$exists": true}}),
        0
    );
    assert_eq!(
        count("governance_changes", doc! {"expires_at": {"$exists": true}}),
        0
    );
    let event = database
        .collection::<mongodb::bson::Document>("events")
        .find_one(doc! {})
        .run()
        .unwrap()
        .unwrap();
    let occurred = event.get_i64("occurred_at").unwrap();
    assert_eq!(
        event.get_datetime("expires_at").unwrap().timestamp_millis() - occurred,
        30 * 86_400_000
    );
    // A machine whose every event expired on the service re-sends them once: the service does not
    // store them again (they are past the retention period, here two seconds) and remembers how
    // far they went, so the next sync sends nothing again
    drop(running);
    let short = support::service(&uri, &name, &[("CRANE_MONGODB_RETENTION_DAYS", "0.00002")]);
    std::thread::sleep(std::time::Duration::from_secs(3));
    database
        .collection::<mongodb::bson::Document>("events")
        .delete_many(doc! {})
        .run()
        .unwrap();
    fixture.ingest_url = short.url.clone();
    fixture.ok(&["agent", "sync"]);
    let head = database
        .collection::<mongodb::bson::Document>("chain_heads")
        .find_one(doc! {})
        .run()
        .unwrap()
        .unwrap();
    let expired_through = head.get_i64("expired_through").unwrap() as u64;
    assert!(
        expired_through >= local,
        "every expired event was passed over: {head}"
    );
    assert!(
        count(
            "events",
            doc! {"occurred_at": {"$lt": crate_now() - 60_000}}
        ) == 0,
        "expired events are not stored again"
    );
    let again = fixture.ok(&["agent", "sync"]);
    assert!(!again.contains("re-sent"), "{again}");
    drop(short);
    // A larger cluster keeps everything: persist removes every expiry at start
    let _kept = support::service(&uri, &name, &[("CRANE_MONGODB_RETENTION_DAYS", "persist")]);
    std::thread::sleep(std::time::Duration::from_secs(2));
    assert_eq!(count("events", doc! {"expires_at": {"$exists": true}}), 0);
    assert_eq!(count("sessions", doc! {"expires_at": {"$exists": true}}), 0);
    database.drop().run().unwrap();
}

/** The current time in Unix milliseconds
 * Input
    - None
 * Output
    - i64
*/
fn crate_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/** A deployed service (needs CRANE_TEST_INGEST_URL): a fresh machine with only the address set
 * uploads everything (a second sync has nothing left to send and nothing to re-send), and a
 * teammate's machine without a repository signing key uploads with its own upload key
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
    let again = fixture.ok(&["agent", "sync"]);
    assert!(
        again.contains("Synchronized 0 events") && !again.contains("re-sent"),
        "the service holds every local record: {again}"
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
}

/** The service's limits (needs CRANE_TEST_MONGODB_URI): a rate per installation answers 429, an
 * event quota and a full database answer 507, and an oversized batch answers 413; the refused data
 * stays on the machine
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
    let cases: [(&str, &str, &str); 4] = [
        ("CRANE_SERVER_REQUESTS_PER_MINUTE", "3", "HTTP 429"),
        ("CRANE_SERVER_MAX_EVENTS_PER_INSTALLATION", "1", "HTTP 507"),
        ("CRANE_SERVER_MAX_STORAGE_MB", "1", "HTTP 507"),
        ("CRANE_SERVER_MAX_BATCH_RECORDS", "1", "HTTP 413"),
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
        let first = fixture.command(&["agent", "sync"]).output().unwrap();
        let refused = if first.status.success() {
            text(&fixture.command(&["agent", "sync"]).output().unwrap())
        } else {
            text(&first)
        };
        assert!(
            refused.contains(expected),
            "{variable}={value}: first: {} then: {refused}",
            text(&first)
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
