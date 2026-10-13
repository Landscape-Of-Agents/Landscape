// Shared helpers of the service tests (built with --features server): start 'crane server' on a
// free port, a stand-in for GitHub's sign-in, HTTP requests that do not follow redirects, and
// signing in to the dashboard.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::json;

/** The MongoDB used by the live tests, when CRANE_TEST_MONGODB_URI is set
 * Input
    - None
 * Output
    - Option<String>
*/
pub fn mongodb() -> Option<String> {
    std::env::var("CRANE_TEST_MONGODB_URI")
        .ok()
        .filter(|uri| !uri.is_empty())
}

/** A running ingestion service, stopped on drop
 * Fields
    - child: Child - the crane server process
    - url: String - its base URL
    - logs: Arc<Mutex<String>> - everything it wrote to stderr (its structured log)
*/
pub struct Service {
    child: Child,
    pub url: String,
    logs: Arc<Mutex<String>>,
}

impl Service {
    /** Return the service's log so far
     * Input
        - None (uses self)
     * Output
        - String JSON lines
    */
    pub fn logs(&self) -> String {
        self.logs.lock().unwrap().clone()
    }

    /** Wait for a log line containing a text: the service writes it before answering, but the
     * log reaches the test through a pipe and a reader thread, so it can arrive a moment later
     * Input
        - text: &str - text the line contains
     * Output
        - Option<String> the line, None after five seconds without it
    */
    pub fn log_line(&self, text: &str) -> Option<String> {
        let started = std::time::Instant::now();
        loop {
            if let Some(line) = self.logs().lines().find(|line| line.contains(text)) {
                return Some(line.to_string());
            }
            if started.elapsed() > std::time::Duration::from_secs(5) {
                return None;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
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
    - variables: &[(&str, &str)] - extra environment
 * Output
    - Service
*/
pub fn service(uri: &str, database: &str, variables: &[(&str, &str)]) -> Service {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crane"));
    for name in [
        "CRANE_GITHUB_CLIENT_ID",
        "CRANE_GITHUB_CLIENT_SECRET",
        "CRANE_PUBLIC_URL",
        "RENDER_EXTERNAL_URL",
    ] {
        command.env_remove(name);
    }
    for (name, value) in variables {
        command.env(name, value);
    }
    let mut child = command
        .args(["server", "--listen", "127.0.0.1:0"])
        .env("CRANE_MONGODB_URI", uri)
        .env("CRANE_MONGODB_DB", database)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let logs = Arc::new(Mutex::new(String::new()));
    let mut stderr = child.stderr.take().unwrap();
    let sink = Arc::clone(&logs);
    std::thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        while let Ok(read) = stderr.read(&mut buffer) {
            if read == 0 {
                break;
            }
            sink.lock()
                .unwrap()
                .push_str(&String::from_utf8_lossy(&buffer[..read]));
        }
    });
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
        logs,
    }
}

/** Start a stand-in for GitHub's sign-in: the OAuth code names the user; alice and bob belong to
 * the Acme organization, carol and mallory to none
 * Input
    - None
 * Output
    - String base URL
*/
pub fn fake_github() -> String {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let address = server.server_addr().to_ip().unwrap().to_string();
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let url = request.url().to_string();
            let login = request
                .headers()
                .iter()
                .find(|header| header.field.equiv("Authorization"))
                .map(|header| {
                    header
                        .value
                        .as_str()
                        .trim_start_matches("Bearer tok-")
                        .to_string()
                })
                .unwrap_or_default();
            let body = if url.starts_with("/login/oauth/access_token") {
                let mut form = String::new();
                request.as_reader().read_to_string(&mut form).unwrap();
                let code = form
                    .split('&')
                    .find_map(|pair| pair.strip_prefix("code="))
                    .unwrap_or_default()
                    .to_string();
                json!({"access_token": format!("tok-{code}")})
            } else if url == "/user" {
                json!({"login": login})
            } else if url.starts_with("/user/orgs") {
                if matches!(login.as_str(), "alice" | "bob") {
                    json!([{"login": "Acme"}])
                } else {
                    json!([])
                }
            } else if let Some(repository) = url.strip_prefix("/repos/") {
                // Roles on repositories: alice administers acme/widgets, bob can push to it,
                // carol administers carol/notes; anyone else is not shown the repository
                let role = match (repository, login.as_str()) {
                    ("acme/widgets", "alice") | ("carol/notes", "carol") => Some(true),
                    ("acme/widgets", "bob") => Some(false),
                    _ => None,
                };
                match role {
                    Some(admin) => {
                        json!({"full_name": repository, "permissions": {"admin": admin, "push": true, "pull": true}})
                    }
                    None => {
                        let _ = request.respond(
                            tiny_http::Response::from_string("{\"message\": \"Not Found\"}")
                                .with_status_code(404),
                        );
                        continue;
                    }
                }
            } else {
                json!({})
            };
            let _ = request.respond(tiny_http::Response::from_string(body.to_string()));
        }
    });
    format!("http://{address}")
}

/** The environment that turns on dashboard sign-in against the stand-in GitHub
 * Input
    - github: &str - stand-in base URL
 * Output
    - Vec<(&'static str, String)>
*/
pub fn sign_in_settings(github: &str) -> Vec<(&'static str, String)> {
    vec![
        ("CRANE_GITHUB_CLIENT_ID", "client".into()),
        ("CRANE_GITHUB_CLIENT_SECRET", "secret".into()),
        ("CRANE_PUBLIC_URL", "http://127.0.0.1".into()),
        ("CRANE_GITHUB_URL", github.into()),
        ("CRANE_GITHUB_API_URL", github.into()),
    ]
}

/** A response
 * Fields
    - status: u16 - HTTP status
    - cookies: Vec<String> - full Set-Cookie values
    - location: Option<String> - Location header
    - headers: Vec<(String, String)> - every header
    - body: String - body
*/
pub struct Fetched {
    pub status: u16,
    pub cookies: Vec<String>,
    pub location: Option<String>,
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Fetched {
    /** Parse the body as JSON
     * Input
        - None (uses self)
     * Output
        - serde_json::Value
    */
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("not JSON: {}", self.body))
    }

    /** Read a header
     * Input
        - name: &str - header name (any case)
     * Output
        - Option<&str>
    */
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/** Send a request without following redirects
 * Input
    - method: &str - HTTP method
    - url: &str - address
    - headers: &[(&str, &str)] - request headers (Cookie included)
 * Output
    - Fetched
*/
pub fn request(method: &str, url: &str, headers: &[(&str, &str)]) -> Fetched {
    let agent = ureq::AgentBuilder::new().redirects(0).build();
    let mut request = agent.request(method, url);
    for (name, value) in headers {
        request = request.set(name, value);
    }
    let response = match request.call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(error) => panic!("{method} {url}: {error}"),
    };
    Fetched {
        status: response.status(),
        cookies: response
            .all("set-cookie")
            .into_iter()
            .map(String::from)
            .collect(),
        location: response.header("location").map(String::from),
        headers: response
            .headers_names()
            .into_iter()
            .map(|name| {
                let value = response.header(&name).unwrap_or_default().to_string();
                (name, value)
            })
            .collect(),
        body: response.into_string().unwrap_or_default(),
    }
}

/** GET with an optional session cookie
 * Input
    - url: &str - address
    - cookie: Option<&str> - Cookie header value
 * Output
    - Fetched
*/
pub fn get(url: &str, cookie: Option<&str>) -> Fetched {
    match cookie {
        Some(cookie) => request("GET", url, &[("Cookie", cookie)]),
        None => request("GET", url, &[]),
    }
}

/** Sign in to the dashboard as a stand-in GitHub user
 * Input
    - base: &str - service URL
    - github: &str - stand-in GitHub URL
    - user: &str - login (also the OAuth code)
 * Output
    - String Cookie header value with the session
*/
pub fn sign_in(base: &str, github: &str, user: &str) -> String {
    let start = get(&format!("{base}/auth/login"), None);
    assert_eq!(start.status, 302);
    let location = start.location.clone().unwrap();
    assert!(location.starts_with(github), "{location}");
    assert!(location.contains("scope=read%3Aorg"), "{location}");
    let state = location.split("state=").nth(1).unwrap().to_string();
    let oauth = start
        .cookies
        .iter()
        .find(|cookie| cookie.starts_with("foxx_oauth="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();
    let done = get(
        &format!("{base}/auth/callback?code={user}&state={state}"),
        Some(&oauth),
    );
    assert_eq!(done.status, 302, "{}", done.body);
    let session = done
        .cookies
        .iter()
        .find(|cookie| cookie.starts_with("foxx_session=") && cookie.len() > 40)
        .unwrap()
        .clone();
    assert!(
        session.contains("HttpOnly") && session.contains("SameSite=Lax"),
        "{session}"
    );
    session.split(';').next().unwrap().to_string()
}

/** Approve a request on the dashboard as a stand-in GitHub user: start /auth/approve, then
 * finish the GitHub sign-in it starts
 * Input
    - base: &str - service URL
    - user: &str - login (also the OAuth code)
    - action: &str - link or owner
    - code: &str - the one-time code
 * Output
    - (bool, String) whether it was confirmed, and the message shown to the person
*/
pub fn approve(base: &str, user: &str, action: &str, code: &str) -> (bool, String) {
    approve_path(
        base,
        user,
        &format!("/auth/approve?action={action}&code={code}"),
    )
}

/** Start any approval path on the dashboard (an approval code or a credit grant) as a stand-in
 * GitHub user, and finish the GitHub sign-in it starts
 * Input
    - base: &str - service URL
    - user: &str - login (also the OAuth code)
    - path: &str - the path that starts it, with its query
 * Output
    - (bool, String) whether it was approved, and the message shown to the person
*/
pub fn approve_path(base: &str, user: &str, path: &str) -> (bool, String) {
    let start = get(&format!("{base}{path}"), None);
    assert_eq!(start.status, 302, "{}", start.body);
    let location = start.location.clone().unwrap();
    let state = location.split("state=").nth(1).unwrap().to_string();
    let cookies = start
        .cookies
        .iter()
        .map(|cookie| cookie.split(';').next().unwrap().to_string())
        .collect::<Vec<_>>()
        .join("; ");
    let done = get(
        &format!("{base}/auth/callback?code={user}&state={state}"),
        Some(&cookies),
    );
    assert_eq!(done.status, 302, "{}", done.body);
    let target = done.location.unwrap();
    let query = target.split('?').nth(1).unwrap_or_default();
    let field = |name: &str| {
        query
            .split(['&', '#'])
            .find_map(|pair| pair.strip_prefix(&format!("{name}=")))
            .unwrap_or_default()
            .to_string()
    };
    let message = percent_decode(&field("approval"));
    (field("ok") == "1", message)
}

/** Decode a percent-encoded query value
 * Input
    - text: &str - encoded text
 * Output
    - String
*/
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&text[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(if bytes[index] == b'+' {
            b' '
        } else {
            bytes[index]
        });
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
