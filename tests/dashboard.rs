// The hosted DataSea dashboard (built with --features server; needs CRANE_TEST_MONGODB_URI, skipped
// otherwise): a landing page and the organization's sections, of which Engineering holds Flows. A
// team repository owned by the Acme organization has two members (alice's and bob's machines);
// carol has a personal repository. Viewers sign in through a stand-in for GitHub.

mod common;
mod support;

use common::Fixture;
use mongodb::bson::{doc, Document};
use serde_json::{json, Value};
use support::{get, post_json, request, sign_in, Service};

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
}

impl Drop for World {
    fn drop(&mut self) {
        // CRANE_TEST_KEEP_DATABASE=1 keeps the data, for looking at the dashboard by hand
        if std::env::var("CRANE_TEST_KEEP_DATABASE").map_or(true, |value| value != "1") {
            let _ = self.database.drop().run();
        }
    }
}

/** The upload key (public, hex) of the machine whose trust directory is home
 * Input
    - home: &std::path::Path - the machine's trust directory
 * Output
    - String
*/
fn upload_key(home: &std::path::Path) -> String {
    let seed = std::fs::read_to_string(home.join("installation.key")).unwrap();
    signing_key(&seed)
        .verifying_key()
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/** The signing key of an upload key seed
 * Input
    - seed: &str - 64 hex characters
 * Output
    - ed25519_dalek::SigningKey
*/
fn signing_key(seed: &str) -> ed25519_dalek::SigningKey {
    let seed: [u8; 32] = (0..32)
        .map(|index| u8::from_str_radix(&seed.trim()[index * 2..index * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    ed25519_dalek::SigningKey::from_bytes(&seed)
}

/** Build the world: two members working on the team repository (alice with a denied edit and a
 * verified command, bob with one command), carol on her own repository, every machine
 * synchronized, and the clones' GitHub owners set
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
    team.protect(&["app/payments.py", "start-line", "1", "end-line", "4"]);
    team.hook("claude", "SessionStart", &json!({"session_id": "dash-1"}));
    let call = |fixture: &Fixture, session: &str, id: &str, tool: &str, input: Value| json!({"session_id": session, "tool_use_id": id, "cwd": fixture.work.to_string_lossy(), "tool_name": tool, "tool_input": input});
    let edit = call(
        &team,
        "dash-1",
        "d1",
        "Edit",
        json!({"file_path": team.work.join("app/payments.py").to_string_lossy(), "old_string": "0.03", "new_string": "0.3"}),
    );
    team.hook("claude", "pre-tool-use", &edit);
    let ok = call(
        &team,
        "dash-1",
        "ok0",
        "Bash",
        json!({"command": "echo ok"}),
    );
    team.hook("claude", "pre-tool-use", &ok);
    team.hook("claude", "post-tool-use", &ok);
    team.ok(&["agent", "sync"]);
    // bob's machine on the same repository
    let alice = std::mem::replace(&mut team.home, team.root.path().join("bob"));
    // Every command starts a background sync. Before 'crane init' trusts the owner key, bob's
    // status report would fail verification, so nothing is uploaded until he has trusted it
    let service_url = std::mem::replace(&mut team.ingest_url, "off".into());
    team.ok(&["repo", "--https", team.remote_url().as_str()]);
    team.ingest_url = service_url;
    let init = team.with_input(&["init"], "y\n");
    assert!(init.status.success(), "{}", common::text(&init));
    team.hook("claude", "SessionStart", &json!({"session_id": "bob-1"}));
    let bob_call = call(&team, "bob-1", "b1", "Bash", json!({"command": "echo bob"}));
    team.hook("claude", "pre-tool-use", &bob_call);
    team.hook("claude", "post-tool-use", &bob_call);
    team.ok(&["agent", "sync"]);
    let bob = upload_key(&team.home);
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
    assert_eq!(
        repositories
            .count_documents(doc! {"repository_id": &widgets, "installation": &bob})
            .run()
            .unwrap(),
        1,
        "bob's machine uploaded"
    );
    Some(World {
        service,
        github,
        database,
        team,
        _personal: personal,
        widgets,
        notes,
        bob,
    })
}

/** Sign-in and scopes: the page and its headers, the scripts it loads (and none of the retired
 * ones), signed-out answers, the sign-in state check, a personal account that sees only its own
 * repository, organization members who see the organization's repository, a stranger who sees
 * nothing, forged and signed-out sessions
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
    assert!(
        page.body.contains("/app.js") && page.body.contains("/flows.js"),
        "{}",
        page.body
    );
    let policy = page
        .header("content-security-policy")
        .unwrap_or_default()
        .to_string();
    assert!(
        policy.contains("script-src 'self'") && policy.contains("frame-ancestors 'none'"),
        "{policy}"
    );
    assert_eq!(get(&format!("{base}/app.js"), None).status, 200);
    assert_eq!(get(&format!("{base}/flows.js"), None).status, 200);
    for retired in ["/charts.js", "/glossary.js", "/pages.js", "/govern.js"] {
        assert_eq!(
            get(&format!("{base}{retired}"), None).status,
            404,
            "{retired}"
        );
    }
    assert_eq!(get(&format!("{base}/api/me"), None).status, 401);
    assert_eq!(
        get(
            &format!("{base}/api/flows?repository={}", world.widgets),
            None
        )
        .status,
        401
    );
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
        .log_line("\"request_id\":\"trace-0001\"")
        .expect("the request is logged with its correlation id");
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
    // Sign-in started under another host (the Render address behind a custom domain) moves to
    // the public host first, so the state cookie is there when GitHub returns
    let moved = get(
        &format!("{}/auth/login?x=1", base.replace("127.0.0.1", "localhost")),
        None,
    );
    assert_eq!(moved.status, 302, "{}", moved.body);
    assert_eq!(
        moved.location.as_deref(),
        Some("http://127.0.0.1/auth/login?x=1")
    );
    assert!(moved.cookies.is_empty(), "{:?}", moved.cookies);
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
    let flows = |cookie: &str, id: &str| {
        get(&format!("{base}/api/flows?repository={id}"), Some(cookie)).status
    };
    assert_eq!(flows(&alice, &world.widgets), 200);
    assert_eq!(flows(&carol, &world.notes), 200);
    assert_eq!(
        flows(&carol, &world.widgets),
        403,
        "a personal account cannot see the organization"
    );
    assert_eq!(flows(&mallory, &world.widgets), 403);
    assert_eq!(
        flows(&alice, &world.notes),
        403,
        "an organization member cannot see a personal repository"
    );
    assert_eq!(flows(&alice, "nope"), 400);
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

/** The Flows section: the view comes from the latest status a member reported, with the flow
 * operations people may request; the retired metric routes are gone; a flow request is checked,
 * staged, confirmed by GitHub (an admin for a signed change), kept for a machine, and withdrawn by
 * its author; Brick operations of the retired Bricks page cannot be requested
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn flows_section_reads_status_and_queues_requests() {
    let Some(world) = world("flows") else {
        return;
    };
    let base = world.service.url.clone();
    let alice = sign_in(&base, &world.github, "alice");
    let view = get(
        &format!("{base}/api/flows?repository={}", world.widgets),
        Some(&alice),
    );
    assert_eq!(view.status, 200, "{}", view.body);
    let view = view.json();
    assert!(view["flows"].is_array(), "{view}");
    assert!(view["reported_by"]["key"].is_string(), "{view}");
    assert!(view["reported_at"].as_u64().unwrap() > 0, "{view}");
    let operations = view["operations"].as_array().unwrap();
    assert!(!operations.is_empty());
    assert!(
        operations.iter().all(|operation| {
            let op = operation["op"].as_str().unwrap();
            op.starts_with("flow-") || op.starts_with("integration-")
        }),
        "only flow operations: {view}"
    );
    let discard = operations
        .iter()
        .find(|operation| operation["op"] == "flow-discard")
        .unwrap();
    assert_eq!(discard["signed_file"], ".crane/flows.json");
    // The metric pages and their data are gone
    for route in [
        "overview",
        "events",
        "sessions",
        "agents",
        "governance",
        "compliance",
        "alerting",
        "grants",
        "bricks",
        "residents",
        "health",
        "members",
        "repo",
    ] {
        let answer = get(
            &format!("{base}/api/{route}?repository={}", world.widgets),
            Some(&alice),
        );
        assert_eq!(answer.status, 404, "{route}: {}", answer.body);
    }
    assert_eq!(
        get(&format!("{base}/api/accounts/acme"), Some(&alice)).status,
        404
    );
    for retired in ["/auth/grant", "/auth/resident", "/auth/agent-action"] {
        assert_eq!(
            get(&format!("{base}{retired}"), None).status,
            404,
            "{retired}"
        );
    }
    // Requests: only flow operations, checked before anything is kept
    let post = |cookie: &str, body: Value| {
        post_json(
            &format!("{base}/api/flows?repository={}", world.widgets),
            cookie,
            &body,
        )
    };
    let brick = post(
        &alice,
        json!({"action": "stage", "op": "create", "params": {}, "reason": "a draft"}),
    );
    assert_eq!(brick.status, 400, "{}", brick.body);
    let invalid = post(
        &alice,
        json!({"action": "stage", "op": "flow-discard", "params": {"flow": "bad id!"}, "reason": "old"}),
    );
    assert_eq!(invalid.status, 400, "{}", invalid.body);
    let carol = sign_in(&base, &world.github, "carol");
    assert_eq!(
        post(
            &carol,
            json!({"action": "stage", "op": "flow-discard", "params": {"flow": "checkout"}, "reason": "old"})
        )
        .status,
        403,
        "a repository the viewer cannot see"
    );
    let stage = |cookie: &str| {
        let staged = post(
            cookie,
            json!({"action": "stage", "op": "flow-discard", "params": {"flow": "checkout"}, "reason": "an old trace"}),
        );
        assert_eq!(staged.status, 200, "{}", staged.body);
        staged.json()
    };
    // bob can push but a signed change needs an admin (no CODEOWNERS file covers it)
    let bob = sign_in(&base, &world.github, "bob");
    let staged = stage(&bob);
    assert_eq!(staged["role"], "admin");
    let (ok, message) = support::approve_path(&base, "bob", staged["continue"].as_str().unwrap());
    assert!(!ok && message.contains("admin"), "{message}");
    let staged = stage(&alice);
    let (ok, message) = support::approve_path(&base, "alice", staged["continue"].as_str().unwrap());
    assert!(ok && message.contains("waiting for a machine"), "{message}");
    let id = staged["request_id"].as_str().unwrap().to_string();
    let requests = world.database.collection::<Document>("brick_requests");
    let kept = requests
        .find_one(doc! {"request_id": &id})
        .run()
        .unwrap()
        .unwrap();
    assert_eq!(kept.get_str("status").unwrap(), "PENDING");
    let view = get(
        &format!("{base}/api/flows?repository={}", world.widgets),
        Some(&alice),
    )
    .json();
    let listed = view["requests"].as_array().unwrap();
    assert!(
        listed
            .iter()
            .any(|row| row["request_id"] == id.as_str() && row["params"]["flow"] == "checkout"),
        "{view}"
    );
    assert!(
        listed.iter().any(|row| row["status"] == "REFUSED"),
        "bob's refused request stays listed: {view}"
    );
    // Only its author withdraws it
    let cancel = json!({"action": "cancel", "request_id": &id});
    assert_eq!(post(&bob, cancel.clone()).status, 404);
    let withdrawn = post(&alice, cancel);
    assert_eq!(withdrawn.status, 200, "{}", withdrawn.body);
    assert_eq!(withdrawn.json()["status"], "CANCELLED");
    assert_eq!(
        requests
            .find_one(doc! {"request_id": &id})
            .run()
            .unwrap()
            .unwrap()
            .get_str("status")
            .unwrap(),
        "CANCELLED"
    );
    assert_eq!(
        post(&alice, json!({"action": "nothing"})).status,
        400,
        "unknown actions are refused"
    );
}

/** The dashboard only reads: other methods are refused and every read leaves the database
 * unchanged; machines (or agents holding a machine's key) cannot read records back, and the
 * routes released Crane versions still call for quarantines, grants, and resident decisions find
 * nothing to apply
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
    for _ in 0..2 {
        assert_eq!(
            get(
                &format!("{base}/api/flows?repository={}", world.widgets),
                Some(&alice)
            )
            .status,
            200
        );
        assert_eq!(get(&format!("{base}/api/me"), Some(&alice)).status, 200);
        assert_eq!(get(&format!("{base}/"), Some(&alice)).status, 200);
    }
    for method in ["POST", "PUT", "DELETE", "PATCH"] {
        for path in ["/", "/api/me", "/api/bricks", "/api/dashboard-agents"] {
            let answer = request(
                method,
                &format!("{base}{path}?repository={}", world.widgets),
                &[("Cookie", &alice)],
            );
            assert_eq!(answer.status, 405, "{method} {path}");
        }
    }
    assert_eq!(before, counts(), "reading the dashboard changed nothing");
    // Only what Flows and the machines need is kept; telemetry is not stored
    let names = world.database.list_collection_names().run().unwrap();
    for retired in [
        "events",
        "sessions",
        "alerts",
        "autonomy_ledger",
        "service_events",
        "dashboard_agents",
        "credit_grants",
        "resident_decisions",
    ] {
        assert!(
            !names.iter().any(|name| name == retired),
            "{retired}: {names:?}"
        );
    }
    // A machine's own upload key can upload, but no route returns records to it
    let seed = std::fs::read_to_string(world.team.home.join("installation.key")).unwrap();
    let key = signing_key(&seed);
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
    let positions = positions.json();
    assert!(
        positions["events"].as_u64().unwrap() >= 3,
        "positions are kept, not records: {positions}"
    );
    assert_eq!(
        signed("snapshot").status,
        404,
        "there is no read-back route"
    );
    for (operation, field) in [
        ("agent-policy", "quarantines"),
        ("grants", "grants"),
        ("resident-decisions", "decisions"),
    ] {
        let answer = signed(operation);
        assert_eq!(answer.status, 200, "{operation}: {}", answer.body);
        assert_eq!(answer.json()[field], json!([]), "{operation}");
    }
    let machine = request(
        "GET",
        &format!("{base}/api/flows?repository={}", world.widgets),
        &[("X-Crane-Installation", "a".repeat(64).as_str())],
    );
    assert_eq!(
        machine.status, 401,
        "the dashboard API answers only signed-in people"
    );
}

/** The digest the service stores for an approval code
 * Input
    - code: &str - the code
 * Output
    - String
*/
fn code_digest(code: &str) -> String {
    use sha2::{Digest, Sha512};
    let domain = "foxx/approval-code/v1";
    let mut hasher = Sha512::new();
    hasher.update(b"crane-digest\0");
    hasher.update((domain.len() as u64).to_be_bytes());
    hasher.update(domain.as_bytes());
    hasher.update(1_u64.to_be_bytes());
    hasher.update((code.len() as u64).to_be_bytes());
    hasher.update(code.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/** Approvals are decided by GitHub: a machine is linked by someone with access to the repository
 * (and machines nobody linked are then left out of the Flows section), an owner key is confirmed
 * only by an admin, codes work once, and a joining teammate's crane init shows the confirmation
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn approvals_are_decided_by_github() {
    let Some(mut world) = world("approvals") else {
        return;
    };
    let base = world.service.url.clone();
    let pending = |collection: &str, code: &str, installation: &str, owner: Option<&str>| {
        let mut document = doc! {
            "repository_id": &world.widgets,
            "installation": installation,
            "code_digest": code_digest(code),
            "remote": "github.com/acme/widgets",
            "requested_at": 1_i64,
            "expires_at": i64::MAX,
        };
        if let Some(owner) = owner {
            document.insert("owner_key", owner);
        }
        world
            .database
            .collection::<Document>(collection)
            .insert_one(document)
            .run()
            .unwrap();
    };
    // bob links his machine (he can push to acme/widgets); carol cannot see the repository
    pending("member_links", "BOBLINKCODE1", &world.bob, None);
    let (ok, message) = support::approve(&base, "carol", "link", "BOBLINKCODE1");
    assert!(!ok && message.contains("does not show @carol"), "{message}");
    let (ok, message) = support::approve(&base, "bob", "link", "BOBLINKCODE1");
    assert!(ok && message.contains("linked to @bob"), "{message}");
    let (ok, message) = support::approve(&base, "bob", "link", "BOBLINKCODE1");
    assert!(
        !ok && message.contains("already used"),
        "codes work once: {message}"
    );
    assert_eq!(
        world
            .database
            .collection::<Document>("member_links")
            .find_one(doc! {"installation": &world.bob})
            .run()
            .unwrap()
            .unwrap()
            .get_str("confirmed_by")
            .unwrap(),
        "bob"
    );
    // Once a machine is linked, the Flows section shows the linked machines' reports only
    let alice = sign_in(&base, &world.github, "alice");
    let view = get(
        &format!("{base}/api/flows?repository={}", world.widgets),
        Some(&alice),
    )
    .json();
    assert_eq!(view["reported_by"]["key"], world.bob.as_str(), "{view}");
    // The owner key is confirmed only by an admin of acme/widgets
    let owner = world
        .team
        .ok(&["admin", "list"])
        .lines()
        .find_map(|line| line.strip_prefix("owner: "))
        .unwrap()
        .replace(' ', "");
    pending("owner_keys", "OWNERCODE123", "x", Some(&owner));
    let (ok, message) = support::approve(&base, "bob", "owner", "OWNERCODE123");
    assert!(!ok && message.contains("an admin"), "{message}");
    let (ok, message) = support::approve(&base, "alice", "owner", "OWNERCODE123");
    assert!(ok && message.contains("confirmed by @alice"), "{message}");
    // A teammate joining (a new machine, "dave") sees that GitHub vouches for the owner key
    let original = std::mem::replace(&mut world.team.home, world.team.root.path().join("dave"));
    let remote = world.team.remote_url();
    world.team.ok(&["repo", "--https", remote.as_str()]);
    let joined = world.team.with_input(&["init"], "n\n");
    world.team.home = original;
    let output = common::text(&joined);
    assert!(
        output.contains("this owner key was confirmed by @alice"),
        "{output}"
    );
}
