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
    // Every command starts a background sync. Before 'crane init' trusts the owner key, bob's
    // status report would fail verification (an F12 DENY that races the next command), so
    // nothing is uploaded until he has trusted it
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
 * events (DASHBOARD.md) with filters and pagination and their member, the action graph, evals,
 * compliance,
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
    // bob's session stops reporting: no hook activity for hours and no signed end
    world
        .database
        .collection::<Document>("sessions")
        .update_one(
            doc! {"repository_id": &world.widgets, "installation": &world.bob},
            doc! {"$set": {"last_seen_at": 1_i64}},
        )
        .run()
        .unwrap();
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
    assert_eq!(denied["events"][0]["decision"]["enforcement"], "PREVENTED");
    assert_eq!(denied["events"][0]["flow"], "F05");
    // Every event carries the envelope and all thirteen attribute groups
    for group in [
        "action",
        "repository",
        "session",
        "agent",
        "resource",
        "response",
        "assessment",
        "decision",
        "eval",
        "severity",
        "bindings",
        "telemetry",
        "other",
    ] {
        assert!(denied["events"][0][group].is_object(), "{group}: {denied}");
    }
    assert!(
        denied["events"][0]["event_id"].is_string() && denied["events"][0]["occurred_at"].is_u64()
    );
    assert_eq!(denied["events"][0]["origin"], "MACHINE");
    let flows = api("events?flow=F05&limit=500").json();
    assert!(flows["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| event["flow"] == "F05"));
    let service = api("events?origin=SERVICE&limit=500").json();
    assert!(
        service["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["event_name"] == "SYNC_COMPLETED"),
        "the service's own events are in the stream: {service}"
    );
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
    assert_eq!(detail["evals"]["denied"], 1, "{}", detail["evals"]);
    assert_eq!(
        detail["evals"]["by_enforcement"]["PREVENTED"], 1,
        "{}",
        detail["evals"]
    );
    let definitions = api("definitions").json();
    assert_eq!(definitions["groups"].as_array().unwrap().len(), 13);
    // The graphs: events over time grouped by a facet, and the count of every facet value
    let series = api("series?by=decision").json();
    let total = series["total"].as_u64().unwrap();
    assert!(total > 0, "{series}");
    let summed = series["series"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            item["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|point| point.as_u64().unwrap())
                .sum::<u64>()
        })
        .sum::<u64>();
    assert_eq!(
        summed, total,
        "every event lands in one bucket of one series"
    );
    assert!(series["series"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["key"] == "DENY"));
    let facets = api("facets?decision=DENY").json();
    assert_eq!(facets["total"], 1, "{facets}");
    assert!(
        facets["facets"]["decision"].as_array().unwrap().len() > 1,
        "a facet ignores its own filter so its other values stay choosable: {facets}"
    );
    let several = api("events?decision=DENY,ALLOW&limit=500").json();
    assert!(several["events"]
        .as_array()
        .unwrap()
        .iter()
        .all(|event| matches!(
            event["decision"]["decision"].as_str(),
            Some("DENY" | "ALLOW")
        )));
    let text = api("events?q=PREVENTED").json();
    assert!(text["total"].as_u64().unwrap() >= 1, "{text}");
    // Any attribute, any operator, combined: the JSON filter and the shortcuts together
    let encode = |clauses: Value| {
        clauses
            .to_string()
            .replace('%', "%25")
            .replace('"', "%22")
            .replace(' ', "%20")
            .replace('{', "%7B")
            .replace('}', "%7D")
            .replace('[', "%5B")
            .replace(']', "%5D")
            .replace(',', "%2C")
            .replace(':', "%3A")
    };
    let combined = api(&format!(
        "events?limit=500&flow=F05,F06&filter={}",
        encode(json!([
            {"path": "severity.severity_status", "op": "not", "values": ["CRITICAL"]},
            {"path": "action.action_tool", "op": "exists"},
            {"path": "event_name", "op": "prefix", "values": ["TOOL_CALL"]},
        ]))
    ))
    .json();
    let list = combined["events"].as_array().unwrap();
    assert!(!list.is_empty(), "{combined}");
    assert!(list.iter().all(
        |event| ["F05", "F06"].contains(&event["flow"].as_str().unwrap())
            && event["severity"]["severity_status"] != "CRITICAL"
            && !event["action"]["action_tool"]
                .as_str()
                .unwrap_or_default()
                .is_empty()
            && event["event_name"]
                .as_str()
                .unwrap()
                .starts_with("TOOL_CALL")
    ));
    let none = api(&format!("events?filter={}", encode(json!([{"path": "flow", "op": "is", "values": ["F05"]}, {"path": "flow", "op": "not", "values": ["F05"]}])))).json();
    assert_eq!(none["total"], 0, "contradictory clauses match nothing");
    let bad = get(
        &format!(
            "{base}/api/events?repository={}&filter=%5Bnot-json",
            world.widgets
        ),
        Some(&alice),
    );
    assert_eq!(
        bad.status, 400,
        "a malformed filter is refused: {}",
        bad.body
    );
    // The attribute catalog offers every path the filter builder can use
    let catalog = api("attributes").json();
    let paths = catalog["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|attribute| attribute["path"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    for path in [
        "severity.severity_status",
        "decision.decision",
        "flow",
        "event_name",
        "action.action_tool",
        "response.response_statement",
    ] {
        assert!(
            paths.iter().any(|known| known == path),
            "{path} in {paths:?}"
        );
    }
    let digest = catalog["attributes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|attribute| attribute["path"] == "digest")
        .unwrap();
    assert!(
        digest["values"].as_array().unwrap().is_empty(),
        "identifiers are filterable by presence, not listed by value"
    );
    // The deeper views, all under the same filter
    let gantt = api("gantt?rows=session&color=decision").json();
    let bars = gantt["rows"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|row| row["bars"].as_array().unwrap().clone())
        .collect::<Vec<_>>();
    assert!(!bars.is_empty(), "{gantt}");
    assert!(bars
        .iter()
        .all(|bar| bar["end"].as_u64() >= bar["start"].as_u64()));
    assert!(bars.iter().any(|bar| bar["color"] == "DENY"), "{gantt}");
    let by_tool = api("gantt?rows=tool&color=severity").json();
    assert!(
        by_tool["rows"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| row["key"] == "BASH"),
        "{by_tool}"
    );
    let timeline = api("timeline?lanes=flow&color=severity").json();
    assert_eq!(
        timeline["points"].as_array().unwrap().len() as u64,
        timeline["total"].as_u64().unwrap()
    );
    let traces = api("traces?limit=500").json();
    let trace_list = traces["traces"].as_array().unwrap();
    let call = trace_list
        .iter()
        .find(|trace| trace["spans"].as_u64().unwrap() >= 2)
        .expect("a tool call with its decision and verification");
    let detail = api(&format!("traces/{}", call["id"].as_str().unwrap())).json();
    assert_eq!(
        detail["spans"].as_array().unwrap().len() as u64,
        call["spans"].as_u64().unwrap()
    );
    let failed = api("traces?outcome=FAILED&limit=500").json();
    assert!(failed["traces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|trace| trace["outcome"] == "FAILED"));
    assert!(
        failed["total"].as_u64().unwrap() >= 1,
        "the denied edit is a failed trace"
    );
    let long = api("traces?min_spans=2&limit=500").json();
    assert!(long["traces"]
        .as_array()
        .unwrap()
        .iter()
        .all(|trace| trace["spans"].as_u64().unwrap() >= 2));
    let topology = api("topology").json();
    assert!(
        topology["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["to"] == "2:BASH"),
        "{topology}"
    );
    let dependencies = api("dependencies").json();
    // Decisions are followed by credit settlement and verification (order only, never a cause)
    assert!(
        dependencies["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|edge| edge["from"] == "F05"
                && ["F06", "F27"].contains(&edge["to"].as_str().unwrap())),
        "{dependencies}"
    );
    assert!(dependencies["edges"]
        .as_array()
        .unwrap()
        .iter()
        .all(|edge| edge["from"] != edge["to"] && edge["count"].as_u64().unwrap() > 0));
    let histogram = api("histogram?field=events_per_session&bins=5").json();
    assert_eq!(
        histogram["bins"]
            .as_array()
            .unwrap()
            .iter()
            .map(|bin| bin["count"].as_u64().unwrap())
            .sum::<u64>(),
        histogram["count"].as_u64().unwrap()
    );
    let semantics = api("semantics").json();
    assert!(
        !semantics["groups"]["statements"]
            .as_array()
            .unwrap()
            .is_empty(),
        "{semantics}"
    );
    assert!(
        semantics["groups"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|rule| rule["value"] == "PRESERVED_SELECTION"),
        "{}",
        semantics["groups"]["rules"]
    );
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
    // Network egress is not a control (hooks cannot observe it); it is inferred observability
    assert!(compliance["controls"]
        .as_array()
        .unwrap()
        .iter()
        .all(|control| control["control"]["id"] != "CR-10"));
    assert!(compliance["inferred"][0]["meaning"]
        .as_str()
        .unwrap()
        .contains("not observed"));
    assert!(compliance["controls"]
        .as_array()
        .unwrap()
        .iter()
        .all(|control| control["control"]["fix"].is_string() && control["limitations"].is_array()));
    assert_eq!(
        outcome("CR-09"),
        "SATISFIED",
        "redaction is by design; its limit is a limitation, not a gap"
    );
    assert!(compliance["outcomes"].is_object());
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
    assert_eq!(bundle["manifest"]["parts"].as_array().unwrap().len(), 4);
    assert!(!bundle["spec_events"].as_array().unwrap().is_empty());
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
    // bob's session stopped reporting (aged before the first request, which caches the view)
    let overview = api("overview").json();
    assert_eq!(overview["stale_sessions"], 1, "{overview}");
    let rows = api(&format!("sessions?members={}", world.bob)).json();
    assert_eq!(rows["sessions"][0]["stale"], true, "{rows}");
    let alerts = api("alerts").json();
    assert!(
        alerts["alerts"].as_array().unwrap().iter().any(|alert| {
            alert["title"] == "Agent session stopped reporting"
                && alert["member"]["key"] == world.bob.as_str()
                && alert["computed"] == true
        }),
        "{alerts}"
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
    // The service's own event log (DASHBOARD.md F30) records what was viewed; nothing else changes
    let counts = || {
        let mut names = world.database.list_collection_names().run().unwrap();
        names.sort();
        names
            .into_iter()
            .filter(|name| name != "service_events")
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
    assert!(
        world
            .database
            .collection::<Document>("service_events")
            .count_documents(doc! {"event_name": "DASH_VIEW_OPENED", "action.action_user": "alice"})
            .run()
            .unwrap()
            >= 1,
        "views are recorded as service events"
    );
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
 * (and machines nobody linked are then hidden by default), an owner key is confirmed only by an
 * admin, codes work once, and a joining teammate's crane init shows the confirmation
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
    let alice = sign_in(&base, &world.github, "alice");
    let api = |route: &str| {
        let separator = if route.contains('?') { '&' } else { '?' };
        get(
            &format!("{base}/api/{route}{separator}repository={}", world.widgets),
            Some(&alice),
        )
        .json()
    };
    let members = api("members");
    let rows = members["members"].as_array().unwrap();
    let bob = rows
        .iter()
        .find(|row| row["member"]["key"] == world.bob.as_str())
        .unwrap();
    assert_eq!(bob["linked_by"], "bob", "{members}");
    let unlinked = rows
        .iter()
        .find(|row| row["member"]["key"] != world.bob.as_str())
        .unwrap();
    assert!(unlinked["linked_by"].is_null());
    assert_eq!(
        unlinked["selected"], false,
        "unlinked machines are hidden by default"
    );
    assert_eq!(api("overview")["members"], 1);
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
    let repo = get(
        &format!(
            "{base}/api/repo?repository={}&members={}",
            world.widgets, world.bob
        ),
        Some(&alice),
    )
    .json();
    assert_eq!(repo["owner"]["confirmed_by"], "alice", "{repo}");
    // A teammate joining (a new machine, "dave") sees that GitHub vouches for the owner key
    let original = std::mem::replace(&mut world.team.home, world.team.root.path().join("dave"));
    let remote = world.team.remote_url();
    world.team.ok(&["repo", "--https", remote.as_str()]);
    let joined = world.team.with_input(
        &["init"],
        "n
",
    );
    world.team.home = original;
    let output = common::text(&joined);
    assert!(
        output.contains("this owner key was confirmed by @alice"),
        "{output}"
    );
}

/** Credits granted on the dashboard: only an admin of the repository may grant, bad requests are
 * refused, and the session's machine applies the grant to its own ledger exactly once at its next
 * sync and reports back; alert rules built on that machine and their firings reach the alerting
 * view through the same sync
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn credits_and_alerting_round_trip() {
    let Some(world) = world("credits") else {
        return;
    };
    let base = world.service.url.clone();
    let session = world
        .database
        .collection::<Document>("sessions")
        .find_one(doc! {"provider_session_id": "dash-1"})
        .run()
        .unwrap()
        .unwrap()
        .get_str("foxx_session_id")
        .unwrap()
        .to_string();
    let path = |amount: &str| {
        format!(
            "/auth/grant?repository={}&session={session}&amount={amount}&reason=release%20week",
            world.widgets
        )
    };
    // Bad requests never reach GitHub
    for bad in [
        path("0"),
        path("5000"),
        format!("/auth/grant?repository=nope&session={session}&amount=5&reason=x"),
    ] {
        assert_eq!(get(&format!("{base}{bad}"), None).status, 400, "{bad}");
    }
    // bob can push but is not an admin; alice is
    let (ok, message) = support::approve_path(&base, "bob", &path("25"));
    assert!(
        !ok && message.contains("only admins and owners"),
        "{message}"
    );
    let (ok, message) = support::approve_path(&base, "alice", &path("25"));
    assert!(ok && message.contains("25 credits"), "{message}");
    let grants = world.database.collection::<Document>("credit_grants");
    assert_eq!(
        grants
            .count_documents(doc! {"status": "PENDING"})
            .run()
            .unwrap(),
        1
    );
    // The machine applies it at its next sync, once
    world.team.ok(&["agent", "sync"]);
    world.team.ok(&["agent", "sync"]);
    let grant = grants.find_one(doc! {}).run().unwrap().unwrap();
    assert_eq!(grant.get_str("status").unwrap(), "APPLIED", "{grant:?}");
    assert!(grant.get_str("ledger_entry").unwrap().starts_with("led_"));
    let entries = world
        .database
        .collection::<Document>("autonomy_ledger")
        .count_documents(doc! {"reason": {"$regex": "^granted on the dashboard by @alice"}})
        .run()
        .unwrap();
    assert_eq!(entries, 1, "applied exactly once and uploaded");
    // An alert rule on alice's machine fires (before the first read: the dashboard caches a view for
    // 20 seconds); its firing, delivery, and evaluations reach the view
    world.team.ok(&[
        "alert", "add", "denials", "metric", "denied", "at_least", "1", "window", "1d", "channels",
        "slack",
    ]);
    std::thread::sleep(std::time::Duration::from_secs(3));
    world.team.ok(&["alert", "check"]);
    world.team.ok(&["agent", "sync"]);
    let alice = sign_in(&base, &world.github, "alice");
    let api = |route: &str| {
        let separator = if route.contains('?') { '&' } else { '?' };
        let answer = get(
            &format!("{base}/api/{route}{separator}repository={}", world.widgets),
            Some(&alice),
        );
        assert_eq!(answer.status, 200, "{route}: {}", answer.body);
        answer.json()
    };
    let credits = api("grants");
    assert_eq!(credits["grants"][0]["status"], "APPLIED", "{credits}");
    assert_eq!(credits["grants"][0]["approved_by"], "alice");
    let row = credits["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["session"] == session.as_str())
        .unwrap();
    assert!(row["balance"]["granted"].as_u64().unwrap() >= 25, "{row}");
    assert_eq!(credits["ledger"].as_array().unwrap().len(), 1);
    let alerting = api("alerting");
    assert_eq!(alerting["rules"][0]["name"], "denials", "{alerting}");
    assert_eq!(alerting["rules"][0]["state"], "FIRING", "{alerting}");
    assert!(alerting["evaluations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|evaluation| evaluation["rule"] == "denials"));
    assert_eq!(alerting["firings"][0]["rule"], "denials", "{alerting}");
    assert_eq!(
        alerting["deliveries"][0]["outcome"], "DRY_RUN",
        "{alerting}"
    );
    let fired = api("events?event=ALERT_RULE_FIRED");
    assert!(
        fired["total"].as_u64().unwrap() >= 1,
        "the firing is an event of the stream (F14)"
    );
}
