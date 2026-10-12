// The hosted Foxx dashboard (built with --features server; needs CRANE_TEST_MONGODB_URI, skipped
// otherwise). A team repository owned by the Acme organization has two members (alice's and bob's
// machines); carol has a personal repository. Viewers sign in through a stand-in for GitHub.

mod common;
mod support;

use common::Fixture;
use mongodb::bson::{doc, Document};
use serde_json::{json, Value};
use support::{get, request, sign_in, Service};

/** The world the dashboard tests look at
 * Fields
    - service: Service - the running service
    - github: String - stand-in GitHub URL
    - database: mongodb::sync::Database - the test database
    - team: Fixture - the Acme repository (alice's machine; bob's trust directory is "bob")
    - _personal: Fixture - carol's personal repository (kept alive)
    - widgets: String - the team repository identity
    - notes: String - the personal repository identity
    - bob: String - bob's member key
    - keep: String - the preserved selection
*/
struct World {
    service: Service,
    github: String,
    database: mongodb::sync::Database,
    team: Fixture,
    _personal: Fixture,
    widgets: String,
    notes: String,
    bob: String,
    keep: String,
}

impl Drop for World {
    fn drop(&mut self) {
        // CRANE_TEST_KEEP_DATABASE=1 keeps the data, for looking at the dashboard by hand
        if std::env::var("CRANE_TEST_KEEP_DATABASE").map_or(true, |value| value != "1") {
            let _ = self.database.drop().run();
        }
    }
}

/** Build the world: two members working on the team repository (alice with a Slack integration,
 * a denied edit, and three verified commands; bob with one command), carol on her own repository,
 * every machine synchronized, and the clones' GitHub owners set
 * Input
    - name: &str - database name suffix
 * Output
    - Option<World>, None when no MongoDB is configured
*/
fn world(name: &str) -> Option<World> {
    let Some(uri) = support::mongodb() else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run the dashboard tests");
        return None;
    };
    let github = support::fake_github();
    let database_name = format!("crane_dash_{name}_{}", std::process::id());
    let settings = support::sign_in_settings(&github);
    let variables = settings
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect::<Vec<_>>();
    let service = support::service(&uri, &database_name, &variables);
    let mut team = Fixture::ready();
    team.ingest_url = service.url.clone();
    let keep = team.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    let slack = team.with_input(
        &["integrate", "slack"],
        "1\nhttps://hooks.slack.com/services/T0/B0/TOPSECRETVALUE\n\n2\n\ny\n",
    );
    assert!(slack.status.success(), "{}", common::text(&slack));
    team.hook(
        "claude",
        "SessionStart",
        &json!({"session_id": "dash-1", "model": "m"}),
    );
    let call = |fixture: &Fixture, session: &str, id: &str, tool: &str, input: Value| json!({"session_id": session, "tool_use_id": id, "cwd": fixture.work.to_string_lossy(), "tool_name": tool, "tool_input": input});
    let edit = call(
        &team,
        "dash-1",
        "d1",
        "Edit",
        json!({"file_path": team.work.join("app/payments.py").to_string_lossy(), "old_string": "0.03", "new_string": "0.3"}),
    );
    team.hook("claude", "pre-tool-use", &edit);
    for index in 0..3 {
        let payload = call(
            &team,
            "dash-1",
            &format!("ok{index}"),
            "Bash",
            json!({"command": "echo ok"}),
        );
        team.hook("claude", "pre-tool-use", &payload);
        team.hook("claude", "post-tool-use", &payload);
    }
    team.ok(&["agent", "sync"]);
    // bob's machine on the same repository
    let alice = std::mem::replace(&mut team.home, team.root.path().join("bob"));
    team.ok(&["repo", "--https", team.remote_url().as_str()]);
    team.with_input(&["init"], "y\n");
    team.hook("claude", "SessionStart", &json!({"session_id": "bob-1"}));
    let bob_call = call(&team, "bob-1", "b1", "Bash", json!({"command": "echo bob"}));
    team.hook("claude", "pre-tool-use", &bob_call);
    team.hook("claude", "post-tool-use", &bob_call);
    team.ok(&["agent", "sync"]);
    team.home = alice;
    // carol's personal repository
    let mut personal = Fixture::ready();
    personal.ingest_url = service.url.clone();
    personal.hook("claude", "SessionStart", &json!({"session_id": "carol-1"}));
    personal.ok(&["agent", "sync"]);
    // Let background syncs started by the hooks finish
    std::thread::sleep(std::time::Duration::from_secs(5));
    team.ok(&["agent", "sync"]);
    let database = mongodb::sync::Client::with_uri_str(&uri)
        .unwrap()
        .database(&database_name);
    let repositories = database.collection::<Document>("repositories");
    let ids = repositories
        .distinct("repository_id", doc! {})
        .run()
        .unwrap()
        .into_iter()
        .filter_map(|id| id.as_str().map(String::from))
        .collect::<Vec<_>>();
    assert_eq!(ids.len(), 2, "two repositories: {ids:?}");
    let members = |id: &str| {
        repositories
            .count_documents(doc! {"repository_id": id})
            .run()
            .unwrap()
    };
    let (widgets, notes) = if members(&ids[0]) == 2 {
        (ids[0].clone(), ids[1].clone())
    } else {
        (ids[1].clone(), ids[0].clone())
    };
    // The clones' remotes are local mirrors; give them GitHub owners
    for (id, remote) in [
        (&widgets, "github.com/acme/widgets"),
        (&notes, "github.com/carol/notes"),
    ] {
        repositories
            .update_many(
                doc! {"repository_id": id},
                doc! {"$set": {"remote": remote}},
            )
            .run()
            .unwrap();
    }
    let bob = database
        .collection::<Document>("sessions")
        .find_one(doc! {"provider_session_id": "bob-1"})
        .run()
        .unwrap()
        .expect("bob's session was uploaded")
        .get_str("installation")
        .unwrap()
        .to_string();
    Some(World {
        service,
        github,
        database,
        team,
        _personal: personal,
        widgets,
        notes,
        bob,
        keep,
    })
}

/** Sign-in and scopes: the page and its headers, signed-out answers, the sign-in state check, a
 * personal account that sees only its own repository, organization members who see the
 * organization's repository, a stranger who sees nothing, forged and signed-out sessions
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn sign_in_scopes_personal_and_organization_accounts() {
    let Some(world) = world("scopes") else {
        return;
    };
    let base = world.service.url.clone();
    let page = get(&format!("{base}/"), None);
    assert_eq!(page.status, 200);
    assert!(page.body.contains("/app.js"), "{}", page.body);
    let policy = page
        .header("content-security-policy")
        .unwrap_or_default()
        .to_string();
    assert!(
        policy.contains("script-src 'self'") && policy.contains("frame-ancestors 'none'"),
        "{policy}"
    );
    assert_eq!(get(&format!("{base}/app.js"), None).status, 200);
    assert_eq!(get(&format!("{base}/api/me"), None).status, 401);
    // The dashboard's /api/health needs a signed-in viewer; the service's /health does not
    assert_eq!(get(&format!("{base}/api/health"), None).status, 401);
    assert_eq!(get(&format!("{base}/health"), None).json()["status"], "ok");
    // The logo and the tab icon are served as cacheable images
    for asset in ["/logo.png", "/favicon.png"] {
        let image = get(&format!("{base}{asset}"), None);
        assert_eq!(image.status, 200, "{asset}");
        assert_eq!(image.header("content-type"), Some("image/png"));
        assert!(image
            .header("cache-control")
            .unwrap_or_default()
            .contains("max-age"));
    }
    // Every response carries a correlation id; a caller's id is echoed and logged without the
    // query string or the session cookie
    assert_eq!(page.header("x-request-id").map(str::len), Some(16));
    let traced = request(
        "GET",
        &format!("{base}/api/me?secret-query=1"),
        &[
            ("X-Request-Id", "trace-0001"),
            ("Cookie", "foxx_session=not-a-session"),
        ],
    );
    assert_eq!(traced.header("x-request-id"), Some("trace-0001"));
    let line = world
        .service
        .logs()
        .lines()
        .find(|line| line.contains("\"request_id\":\"trace-0001\""))
        .expect("the request is logged with its correlation id")
        .to_string();
    let entry: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(entry["path"], "/api/me");
    assert_eq!(entry["status"], 401);
    assert_eq!(entry["component"], "http");
    assert!(
        !line.contains("secret-query") && !line.contains("not-a-session"),
        "{line}"
    );
    let start = get(&format!("{base}/auth/login"), None);
    let oauth = start.cookies[0].split(';').next().unwrap().to_string();
    let mismatch = get(
        &format!("{base}/auth/callback?code=alice&state=wrong"),
        Some(&oauth),
    );
    assert_eq!(mismatch.status, 400);
    let alice = sign_in(&base, &world.github, "alice");
    let me = get(&format!("{base}/api/me"), Some(&alice)).json();
    assert_eq!(me["login"], "alice");
    assert_eq!(
        me["accounts"][0],
        json!({"name": "alice", "kind": "personal"})
    );
    assert_eq!(
        me["accounts"][1],
        json!({"name": "acme", "kind": "organization"})
    );
    let visible = me["repositories"].as_array().unwrap();
    assert_eq!(visible.len(), 1, "{me}");
    assert_eq!(visible[0]["id"], world.widgets.as_str());
    assert_eq!(visible[0]["account"], "acme");
    let carol = sign_in(&base, &world.github, "carol");
    let me = get(&format!("{base}/api/me"), Some(&carol)).json();
    assert_eq!(me["accounts"].as_array().unwrap().len(), 1);
    assert_eq!(me["repositories"][0]["id"], world.notes.as_str());
    assert_eq!(me["repositories"].as_array().unwrap().len(), 1);
    let mallory = sign_in(&base, &world.github, "mallory");
    assert!(
        get(&format!("{base}/api/me"), Some(&mallory)).json()["repositories"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let summary = get(&format!("{base}/api/accounts/acme"), Some(&alice)).json();
    assert_eq!(summary["repositories"][0]["members"], 2, "{summary}");
    assert!(summary["repositories"][0]["denied"].as_u64().unwrap() >= 1);
    assert_eq!(
        get(&format!("{base}/api/accounts/acme"), Some(&carol)).status,
        403
    );
    let overview = |cookie: &str, id: &str| {
        get(
            &format!("{base}/api/overview?repository={id}"),
            Some(cookie),
        )
        .status
    };
    assert_eq!(overview(&alice, &world.widgets), 200);
    assert_eq!(overview(&carol, &world.notes), 200);
    assert_eq!(
        overview(&carol, &world.widgets),
        403,
        "a personal account cannot see the organization"
    );
    assert_eq!(overview(&mallory, &world.widgets), 403);
    assert_eq!(
        overview(&alice, &world.notes),
        403,
        "an organization member cannot see a personal repository"
    );
    // Change the signature's last character (always to a different one)
    let last = if alice.ends_with('0') { '1' } else { '0' };
    let forged = format!("{}{last}", &alice[..alice.len() - 1]);
    assert_ne!(forged, alice);
    assert_eq!(get(&format!("{base}/api/me"), Some(&forged)).status, 401);
    let out = get(&format!("{base}/auth/logout"), Some(&alice));
    assert_eq!(out.status, 302);
    assert!(out
        .cookies
        .iter()
        .any(|cookie| cookie.starts_with("foxx_session=;") && cookie.contains("Max-Age=0")));
}

/** Every view of the old local dashboard, now for a team: governance from the status reports,
 * events with filters and pagination and their member, the action graph, metrics, compliance,
 * reports and evidence bundles, integrations without secrets, members, per-member chains, and the
 * member filter
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn dashboard_shows_every_view_for_a_team() {
    let Some(world) = world("views") else {
        return;
    };
    let base = world.service.url.clone();
    let alice = sign_in(&base, &world.github, "alice");
    let api = |route: &str| {
        let separator = if route.contains('?') { '&' } else { '?' };
        let answer = get(
            &format!("{base}/api/{route}{separator}repository={}", world.widgets),
            Some(&alice),
        );
        assert_eq!(answer.status, 200, "{route}: {}", answer.body);
        answer
    };
    let members = api("members").json();
    let list = members["members"].as_array().unwrap();
    assert_eq!(list.len(), 2);
    assert!(
        list.iter()
            .all(|member| member["verified"] == true && member["status_reported_at"].is_u64()),
        "{members}"
    );
    let governance = api("governance").json();
    assert_eq!(governance["selections"][0]["id"], world.keep.as_str());
    assert!(governance["selections"][0]["resolved"].is_object());
    assert_eq!(governance["policies"][0]["name"], "default");
    assert!(governance["reported_by"]["key"].is_string());
    let page = api("events?limit=2").json();
    assert_eq!(page["events"].as_array().unwrap().len(), 2);
    let cursor = page["next_cursor"].as_u64().expect("more events");
    let next = api(&format!("events?limit=2&cursor={cursor}")).json();
    assert_ne!(next["events"][0]["event_id"], page["events"][0]["event_id"]);
    let denied = api("events?decision=DENY").json();
    assert_eq!(denied["events"].as_array().unwrap().len(), 1, "{denied}");
    assert_eq!(denied["events"][0]["enforcement"], "PREVENTED");
    assert!(denied["events"][0]["member"]["key"].as_str().unwrap() != world.bob);
    let sessions = api("sessions").json();
    assert_eq!(
        sessions["sessions"].as_array().unwrap().len(),
        2,
        "both members' sessions"
    );
    let session = sessions["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["member"]["key"] != world.bob.as_str())
        .unwrap()["session"]["foxx_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    let detail = api(&format!("sessions/{session}")).json();
    assert!(detail["member"]["key"].is_string());
    let graph = api(&format!("graph/{session}")).json();
    assert!(graph["edges"]
        .as_array()
        .unwrap()
        .iter()
        .any(|edge| edge["kind"] == "verified_by"));
    assert!(graph["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|node| node["flags"]
            .as_array()
            .unwrap()
            .iter()
            .any(|flag| flag == "denied")));
    let metrics = api(&format!("metrics?scope=session&id={session}")).json();
    assert!(metrics["metrics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|metric| metric["name"] == "prevented_actions" && metric["value"] == 1));
    let compliance = api("compliance").json();
    assert!(compliance["disclaimer"]
        .as_str()
        .unwrap()
        .contains("not a certification"));
    let outcome = |id: &str| {
        compliance["controls"]
            .as_array()
            .unwrap()
            .iter()
            .find(|control| control["control"]["id"] == id)
            .unwrap()["outcome"]
            .clone()
    };
    assert_eq!(outcome("CR-01"), "SATISFIED", "{compliance}");
    assert_eq!(outcome("CR-10"), "UNSUPPORTED");
    let report = api(&format!("reports/session/{session}")).json();
    assert!(report["limitations"].as_array().unwrap().len() >= 3);
    assert_eq!(
        report["integrity"]["event_chain"]["status"], "verified",
        "{}",
        report["integrity"]
    );
    let bundle = api(&format!("evidence/session/{session}"));
    assert!(bundle
        .header("content-disposition")
        .unwrap_or_default()
        .contains("crane-evidence-"));
    let bundle: Value = serde_json::from_str(&bundle.body).unwrap();
    assert_eq!(bundle["manifest"]["parts"].as_array().unwrap().len(), 3);
    assert_eq!(bundle["bundle_digest"].as_str().unwrap().len(), 128);
    let integrations = api("integrations").json();
    assert!(
        !integrations.to_string().contains("TOPSECRETVALUE"),
        "secrets never leave the machine"
    );
    assert_eq!(integrations["integrations"][0]["name"], "slack");
    let health = api("health").json();
    let chains = health["event_chain"]["members"].as_array().unwrap();
    assert_eq!(chains.len(), 2);
    assert!(
        chains
            .iter()
            .all(|chain| chain["event_chain"]["status"] == "verified"),
        "{health}"
    );
    for route in [
        "overview",
        "agents",
        "tasks",
        "ledger",
        "metrics",
        "definitions",
        "alerts",
        "repo",
    ] {
        api(route);
    }
    let overview = api("overview").json();
    assert_eq!(overview["members"], 2);
    assert_eq!(
        overview["integrity"]["ok"], true,
        "{}",
        overview["integrity"]
    );
    // The member filter shows one teammate
    let only_bob = api(&format!("sessions?members={}", world.bob)).json();
    let rows = only_bob["sessions"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["member"]["key"], world.bob.as_str());
    let bob_events = api(&format!("events?limit=500&members={}", world.bob)).json();
    assert!(bob_events["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["member"]["key"] == world.bob.as_str()));
    assert!(
        api(&format!("events?decision=DENY&members={}", world.bob)).json()["events"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        get(
            &format!("{base}/api/nothing?repository={}", world.widgets),
            Some(&alice)
        )
        .status,
        404
    );
}

/** The dashboard only reads: other methods are refused, every route leaves the database unchanged,
 * and machines (or agents holding a machine's key) cannot read records back from the service
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn dashboard_is_read_only_and_machines_cannot_read() {
    use ed25519_dalek::Signer;
    use sha2::Digest;
    let Some(world) = world("readonly") else {
        return;
    };
    let base = world.service.url.clone();
    let alice = sign_in(&base, &world.github, "alice");
    let counts = || {
        let mut names = world.database.list_collection_names().run().unwrap();
        names.sort();
        names
            .into_iter()
            .map(|name| {
                let count = world
                    .database
                    .collection::<Document>(&name)
                    .count_documents(doc! {})
                    .run()
                    .unwrap();
                (name, count)
            })
            .collect::<Vec<_>>()
    };
    let before = counts();
    let session = get(
        &format!("{base}/api/sessions?repository={}", world.widgets),
        Some(&alice),
    )
    .json()["sessions"][0]["session"]["foxx_session_id"]
        .as_str()
        .unwrap()
        .to_string();
    for route in [
        "overview",
        "members",
        "agents",
        "sessions",
        "tasks",
        "events",
        "governance",
        "ledger",
        "metrics",
        "definitions",
        "compliance",
        "alerts",
        "integrations",
        "repo",
        "health",
    ]
    .iter()
    .map(|route| route.to_string())
    .chain([
        format!("sessions/{session}"),
        format!("graph/{session}"),
        format!("reports/session/{session}"),
        format!("evidence/session/{session}"),
    ]) {
        let answer = get(
            &format!("{base}/api/{route}?repository={}", world.widgets),
            Some(&alice),
        );
        assert_eq!(answer.status, 200, "{route}: {}", answer.body);
    }
    get(&format!("{base}/api/me"), Some(&alice));
    get(&format!("{base}/api/accounts/acme"), Some(&alice));
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        for path in ["/", "/api/overview", "/api/me"] {
            let answer = request(
                method,
                &format!("{base}{path}?repository={}", world.widgets),
                &[("Cookie", &alice)],
            );
            assert_eq!(answer.status, 405, "{method} {path}");
        }
    }
    assert_eq!(before, counts(), "reading the dashboard changed nothing");
    // A machine's own upload key can upload, but no route returns records to it
    let seed = std::fs::read_to_string(world.team.home.join("installation.key")).unwrap();
    let seed: [u8; 32] = (0..32)
        .map(|index| u8::from_str_radix(&seed.trim()[index * 2..index * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let signed = |operation: &str| {
        let path = format!("/v1/repositories/{}/{operation}", world.widgets);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let message = format!(
            "GET\n{path}\n{timestamp}\n{}",
            hex(&sha2::Sha512::digest(b""))
        );
        let mut bytes = b"crane-signature\0foxx/ingest/request/v1\0".to_vec();
        bytes.extend_from_slice(message.as_bytes());
        let public = hex(key.verifying_key().as_bytes());
        let signature = hex(&key.sign(&bytes).to_bytes());
        let timestamp = timestamp.to_string();
        request(
            "GET",
            &format!("{base}{path}"),
            &[
                ("X-Crane-Installation", public.as_str()),
                ("X-Crane-Timestamp", timestamp.as_str()),
                ("X-Crane-Signature", signature.as_str()),
            ],
        )
    };
    let positions = signed("positions");
    assert_eq!(
        positions.status, 200,
        "a correctly signed request is accepted: {}",
        positions.body
    );
    assert!(
        positions.json()["events"].is_u64(),
        "positions are counts, not records"
    );
    assert_eq!(
        signed("snapshot").status,
        404,
        "there is no read-back route"
    );
    let machine = request(
        "GET",
        &format!("{base}/api/overview?repository={}", world.widgets),
        &[("X-Crane-Installation", "a".repeat(64).as_str())],
    );
    assert_eq!(
        machine.status, 401,
        "the dashboard API answers only signed-in people"
    );
}
