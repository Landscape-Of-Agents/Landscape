// The operator dashboard end to end (built with --features server, needs CRANE_TEST_MONGODB_URI):
// only a listed GitHub login gets in; the switch changes only with the operator's session, JSON,
// and the X-Crane-Operator header; machines read a policy signed by the operator key, follow it
// on refresh, and refuse a dashboard signing with another key.

mod common;
mod support;

use common::{text, Fixture};
use serde_json::{json, Value};
use support::{get, Fetched};

/** Sign in to the operator dashboard as a stand-in GitHub user
 * Input
    - base: &str - dashboard URL
    - user: &str - login (also the OAuth code)
 * Output
    - Fetched - the callback's answer
*/
fn sign_in(base: &str, user: &str) -> Fetched {
    let start = get(&format!("{base}/auth/login"), None);
    assert_eq!(start.status, 302, "{}", start.body);
    let state = start
        .location
        .clone()
        .unwrap()
        .split("state=")
        .nth(1)
        .unwrap()
        .to_string();
    let oauth = start
        .cookies
        .iter()
        .find(|cookie| cookie.starts_with("crane_operator_oauth="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    get(
        &format!("{base}/auth/callback?code={user}&state={state}"),
        Some(&oauth),
    )
}

/** The session cookie a sign-in set, if any
 * Input
    - done: &Fetched - the callback's answer
 * Output
    - Option<String> Cookie header value
*/
fn session(done: &Fetched) -> Option<String> {
    done.cookies
        .iter()
        .find(|cookie| cookie.starts_with("foxx_session=") && cookie.len() > 40)
        .map(|cookie| cookie.split(';').next().unwrap().to_string())
}

/** POST to the switch
 * Input
    - base: &str - dashboard URL
    - cookie: Option<&str> - session
    - header: bool - send X-Crane-Operator
    - body: &Value - JSON
 * Output
    - (u16, Value)
*/
fn change(base: &str, cookie: Option<&str>, header: bool, body: &Value) -> (u16, Value) {
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let mut request = agent
        .post(&format!("{base}/api/agent-commands"))
        .set("Content-Type", "application/json");
    if let Some(cookie) = cookie {
        request = request.set("Cookie", cookie);
    }
    if header {
        request = request.set("X-Crane-Operator", "1");
    }
    match request.send_string(&body.to_string()) {
        Ok(response) | Err(ureq::Error::Status(_, response)) => (
            response.status(),
            response.into_json().unwrap_or(Value::Null),
        ),
        Err(error) => panic!("POST: {error}"),
    }
}

/** Read the machine's view after refreshing from a dashboard, trusting a key
 * Input
    - fixture: &Fixture - machine
    - url: &str - dashboard URL
    - trusted: &str - operator public key the build trusts
 * Output
    - String
*/
fn refresh(fixture: &Fixture, url: &str, trusted: &str) -> String {
    text(
        &fixture
            .command(&["operator", "status", "--refresh"])
            .env("CRANE_OPERATOR_URL", url)
            .env("CRANE_OPERATOR_PUBLIC_KEY", trusted)
            .output()
            .unwrap(),
    )
}

/** Run 'crane validate' as an agent, trusting a key
 * Input
    - fixture: &Fixture - machine
    - trusted: &str - operator public key
 * Output
    - bool success
*/
fn agent_validates(fixture: &Fixture, trusted: &str) -> bool {
    fixture
        .command(&["validate"])
        .env("CLAUDECODE", "1")
        .env("CRANE_OPERATOR_PUBLIC_KEY", trusted)
        .output()
        .unwrap()
        .status
        .success()
}

/** Create an operator key pair with the real command
 * Input
    - fixture: &Fixture - machine
 * Output
    - (String, String) seed and public key
*/
fn keygen(fixture: &Fixture) -> (String, String) {
    let printed = fixture.ok(&["operator", "keygen"]);
    let value = |name: &str| {
        printed
            .lines()
            .find_map(|line| line.trim().strip_prefix(&format!("{name}=")))
            .unwrap()
            .to_string()
    };
    (
        value("CRANE_OPERATOR_SIGNING_KEY"),
        value("CRANE_OPERATOR_PUBLIC_KEY"),
    )
}

/** Sign in through DataSea's own sign-in, started from the operator dashboard
 * Input
    - base: &str - DataSea URL
    - user: &str - login (also the OAuth code)
 * Output
    - (Fetched, Option<String>) - the callback's answer and the session cookie
*/
fn sign_in_through_datasea(base: &str, user: &str) -> (Fetched, Option<String>) {
    let start = get(&format!("{base}/operator/auth/login"), None);
    assert_eq!(
        start.location.as_deref(),
        Some("/auth/login?next=/operator/")
    );
    let login = get(&format!("{base}/auth/login?next=/operator/"), None);
    assert_eq!(login.status, 302, "{}", login.body);
    let state = login
        .location
        .clone()
        .unwrap()
        .split("state=")
        .nth(1)
        .unwrap()
        .to_string();
    let cookies = login
        .cookies
        .iter()
        .filter(|cookie| cookie.starts_with("foxx_oauth=") || cookie.starts_with("foxx_next="))
        .map(|cookie| cookie.split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ");
    let done = get(
        &format!("{base}/auth/callback?code={user}&state={state}"),
        Some(&cookies),
    );
    let session = session(&done);
    (done, session)
}

/** The operator dashboard inside DataSea: at /operator/, signed in through DataSea's GitHub app,
 * open only to listed operators, with a key the service keeps across restarts, and machines follow
 * the switch from the DataSea address
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn the_operator_dashboard_runs_inside_datasea() {
    let Some(uri) = support::mongodb() else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run against a live MongoDB");
        return;
    };
    let fixture = Fixture::ready();
    let github = support::fake_github();
    let name = format!("crane_embedded_{}", std::process::id());
    let mut variables = support::sign_in_settings(&github)
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect::<Vec<_>>();
    variables.push(("CRANE_OPERATOR_LOGINS".into(), "alice".into()));
    let variables = variables
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let first = support::service(&uri, &name, &variables);
    let base = first.url.clone();

    // Its own pages, below /operator/
    assert_eq!(
        get(&format!("{base}/operator"), None).location.as_deref(),
        Some("/operator/")
    );
    assert_eq!(get(&format!("{base}/operator/"), None).status, 200);
    assert_eq!(
        get(&format!("{base}/operator/operator.js"), None).status,
        200
    );
    assert_eq!(get(&format!("{base}/operator/api/state"), None).status, 401);
    // The key is created once and kept: a restart signs with the same one
    let public = get(&format!("{base}/v1/agent-commands"), None).json()["public_key"]
        .as_str()
        .unwrap()
        .to_string();
    drop(first);
    let running = support::service(&uri, &name, &variables);
    let base = running.url.clone();
    let policy = get(&format!("{base}/v1/agent-commands"), None).json();
    assert_eq!(
        policy["public_key"],
        public.as_str(),
        "the key survives a restart"
    );
    assert_eq!(policy["policy"]["enabled"], false);

    // carol signs in to DataSea fine, but the operator dashboard refuses her
    let (done, carol) = sign_in_through_datasea(&base, "carol");
    assert_eq!(
        done.location.as_deref(),
        Some("/operator/"),
        "{}",
        done.body
    );
    let carol = carol.expect("a DataSea session");
    assert_eq!(
        get(&format!("{base}/operator/api/state"), Some(&carol)).status,
        403
    );
    assert_eq!(
        change(
            &format!("{base}/operator"),
            Some(&carol),
            true,
            &json!({"enabled": true, "reason": "x"})
        )
        .0,
        403
    );

    // alice is the operator: she turns it on, and a machine reading the DataSea address follows
    let (_, alice) = sign_in_through_datasea(&base, "alice");
    let alice = alice.expect("a session");
    assert_eq!(
        get(&format!("{base}/operator/api/state"), Some(&alice)).status,
        200
    );
    let (status, turned) = change(
        &format!("{base}/operator"),
        Some(&alice),
        true,
        &json!({"enabled": true, "reason": "embedded e2e"}),
    );
    assert_eq!(status, 200, "{turned}");
    assert!(refresh(&fixture, &base, &public).contains("switch: ON"));
    assert!(agent_validates(&fixture, &public));
    // A sign-in that did not start from the operator dashboard still returns to DataSea
    let plain = support::sign_in(&base, &github, "alice");
    assert!(!plain.is_empty());

    mongodb::sync::Client::with_uri_str(&uri)
        .unwrap()
        .database(&name)
        .drop()
        .run()
        .unwrap();
}

/** The whole operator flow against a live MongoDB and a stand-in GitHub
 * Input
    - None
 * Output
    - None (panics on failure)
*/
#[test]
fn only_the_operator_turns_agent_commands_on_and_machines_follow() {
    let Some(uri) = support::mongodb() else {
        eprintln!("skipped: set CRANE_TEST_MONGODB_URI to run against a live MongoDB");
        return;
    };
    let fixture = Fixture::ready();
    let (seed, public) = keygen(&fixture);
    let (_, other_public) = keygen(&fixture);
    let github = support::fake_github();
    let name = format!("crane_operator_{}", std::process::id());
    let mut variables = support::sign_in_settings(&github)
        .into_iter()
        .map(|(name, value)| (name.to_string(), value))
        .collect::<Vec<_>>();
    variables.push(("CRANE_OPERATOR_SIGNING_KEY".into(), seed));
    variables.push(("CRANE_OPERATOR_LOGINS".into(), "@Alice".into()));
    let variables = variables
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect::<Vec<_>>();
    let running = support::operator_service(&uri, &name, &variables);
    let base = running.url.clone();

    // The page and its script are served to anyone; the state is not
    assert_eq!(get(&format!("{base}/"), None).status, 200);
    assert_eq!(get(&format!("{base}/operator.js"), None).status, 200);
    assert_eq!(get(&format!("{base}/api/state"), None).status, 401);
    // Off from the start, and signed by the operator key
    let policy = get(&format!("{base}/v1/agent-commands"), None).json();
    assert_eq!(policy["policy"]["enabled"], false, "{policy}");
    assert_eq!(policy["public_key"], public.as_str());

    // carol is a GitHub user but not an operator: refused, no session
    let carol = sign_in(&base, "carol");
    assert_eq!(carol.location.as_deref(), Some("/?refused=1"));
    assert!(session(&carol).is_none(), "{:?}", carol.cookies);

    // alice is the operator
    let alice = session(&sign_in(&base, "alice")).expect("the operator gets a session");
    let state = get(&format!("{base}/api/state"), Some(&alice)).json();
    assert_eq!(state["login"], "alice");
    assert_eq!(state["agent_commands"]["enabled"], false);

    // Changes need the session, the header, and a reason
    let on = json!({"enabled": true, "reason": "e2e"});
    assert_eq!(change(&base, None, true, &on).0, 401);
    assert_eq!(change(&base, Some(&alice), false, &on).0, 400);
    assert_eq!(
        change(&base, Some(&alice), true, &json!({"enabled": true})).0,
        400
    );

    // The machine follows the dashboard: off first
    assert!(refresh(&fixture, &base, &public).contains("switch: OFF"));
    assert!(!agent_validates(&fixture, &public));

    // alice turns it on: the machine follows after a refresh
    let (status, turned) = change(&base, Some(&alice), true, &on);
    assert_eq!(status, 200, "{turned}");
    assert_eq!(turned["agent_commands"]["enabled"], true);
    assert_eq!(turned["audit"][0]["login"], "alice");
    assert_eq!(turned["audit"][0]["reason"], "e2e");
    let seen = refresh(&fixture, &base, &public);
    assert!(seen.contains("switch: ON"), "{seen}");
    assert!(agent_validates(&fixture, &public));

    // A build that trusts another key refuses this dashboard's answers and stays off
    let elsewhere = Fixture::ready();
    let refused = refresh(&elsewhere, &base, &other_public);
    assert!(
        refused.contains("not signed by the operator key"),
        "{refused}"
    );
    assert!(!agent_validates(&elsewhere, &other_public));

    // alice turns it off: the machine follows at once
    let (status, _) = change(
        &base,
        Some(&alice),
        true,
        &json!({"enabled": false, "reason": "done"}),
    );
    assert_eq!(status, 200);
    assert!(refresh(&fixture, &base, &public).contains("switch: OFF"));
    assert!(!agent_validates(&fixture, &public));

    mongodb::sync::Client::with_uri_str(&uri)
        .unwrap()
        .database(&name)
        .drop()
        .run()
        .unwrap();
}
