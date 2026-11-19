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
