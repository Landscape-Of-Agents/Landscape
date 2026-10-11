// The team dashboard (/team): one view of every installation's activity in a repository, for the
// people who own it on GitHub. Viewers sign in with GitHub (OAuth, scope read:org); the service
// keeps no GitHub token, only a signed session cookie with the viewer's login and organizations
// for eight hours. A repository is shown to a viewer when its verified remote is
// github.com/OWNER/NAME and OWNER is the viewer or one of the viewer's organizations. A remote is
// verified when the uploading installation proved it by sending root commits that, with the
// remote, reproduce the repository identity, so nobody can attach their uploads to someone else's
// GitHub repository. Names shown next to installations are self-reported Git user names.
//
// Configuration (service environment): CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET of a
// GitHub OAuth app whose callback URL is CRANE_PUBLIC_URL/team/callback, CRANE_PUBLIC_URL, and
// optionally CRANE_SESSION_KEY (64 hex characters; without it sessions end when the service
// restarts). CRANE_GITHUB_URL and CRANE_GITHUB_API_URL point at GitHub (changed only by tests).

use std::collections::BTreeMap;
use std::time::Duration;

use mongodb::sync::Database;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiny_http::Request;

use super::store::{self, TeamView};
use super::Reply;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::trust::crypto::{
    hex, is_digest, random_bytes, unhex, verify_signature, SigningIdentity,
};

/** Signature domain of session cookies */
const SESSION_DOMAIN: &str = "foxx/team/session/v1";

/** Lifetime of a session */
const SESSION_MILLIS: u64 = 8 * 60 * 60 * 1000;

/** Team dashboard settings
 * Fields
    - client_id, client_secret: String - the GitHub OAuth app
    - public_url: String - the service's public address
    - github_url, api_url: String - GitHub's web and API addresses
    - key: SigningIdentity - signs session cookies
    - secure: bool - cookies are marked Secure (the public address is HTTPS)
    - agent: ureq::Agent - HTTP client for GitHub
*/
pub(super) struct Team {
    client_id: String,
    client_secret: String,
    public_url: String,
    github_url: String,
    api_url: String,
    key: SigningIdentity,
    secure: bool,
    agent: ureq::Agent,
}

/** A signed-in viewer
 * Fields
    - login: String - GitHub login
    - orgs: Vec<String> - GitHub organizations (lowercase)
    - expires: u64 - Unix milliseconds
*/
#[derive(Debug, Serialize, Deserialize)]
struct Viewer {
    login: String,
    orgs: Vec<String>,
    expires: u64,
}

impl Team {
    /** Read the settings; None when no OAuth app is configured
     * Input
        - None
     * Output
        - Result<Option<Team>, String>
        - Error if the settings are incomplete or the session key is malformed
    */
    pub(super) fn from_environment() -> Result<Option<Self>, String> {
        let variable = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        let Some(client_id) = variable("CRANE_GITHUB_CLIENT_ID") else {
            return Ok(None);
        };
        let client_secret = variable("CRANE_GITHUB_CLIENT_SECRET")
            .ok_or("set CRANE_GITHUB_CLIENT_SECRET for the team dashboard")?;
        let public_url = variable("CRANE_PUBLIC_URL")
            .ok_or("set CRANE_PUBLIC_URL (the service's https address) for the team dashboard")?
            .trim_end_matches('/')
            .to_string();
        let key = match variable("CRANE_SESSION_KEY") {
            Some(seed) => SigningIdentity::from_seed_hex(&seed)
                .map_err(|error| format!("CRANE_SESSION_KEY: {error}"))?,
            None => SigningIdentity::generate()?,
        };
        Ok(Some(Self {
            client_id,
            client_secret,
            secure: public_url.starts_with("https://"),
            public_url,
            github_url: variable("CRANE_GITHUB_URL")
                .unwrap_or_else(|| "https://github.com".into())
                .trim_end_matches('/')
                .to_string(),
            api_url: variable("CRANE_GITHUB_API_URL")
                .unwrap_or_else(|| "https://api.github.com".into())
                .trim_end_matches('/')
                .to_string(),
            key,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(15))
                .build(),
        }))
    }

    /** Build a cookie header value
     * Input
        - name: &str - cookie name
        - value: &str - cookie value
        - max_age: u64 - seconds (0 deletes the cookie)
     * Output
        - String
    */
    fn cookie(&self, name: &str, value: &str, max_age: u64) -> String {
        format!(
            "{name}={value}; Path=/team; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
            if self.secure { "; Secure" } else { "" }
        )
    }

    /** Read and check the viewer's session cookie
     * Input
        - request: &Request - request
     * Output
        - Option<Viewer>, None when absent, forged, or expired
    */
    fn viewer(&self, request: &Request) -> Option<Viewer> {
        let value = cookie(request, "crane_session")?;
        let (payload, signature) = value.split_once('.')?;
        let payload = unhex(payload).ok()?;
        verify_signature(&self.key.public_hex(), SESSION_DOMAIN, &payload, signature).ok()?;
        let viewer: Viewer = serde_json::from_slice(&payload).ok()?;
        (viewer.expires > now_millis()).then_some(viewer)
    }

    /** Sign a session cookie for a viewer
     * Input
        - viewer: &Viewer - viewer
     * Output
        - String cookie value
    */
    fn session(&self, viewer: &Viewer) -> String {
        let payload = serde_json::to_vec(viewer).unwrap_or_default();
        format!(
            "{}.{}",
            hex(&payload),
            self.key.sign(SESSION_DOMAIN, &payload)
        )
    }

    /** Exchange an OAuth code for the viewer's login and organizations
     * Input
        - code: &str - OAuth code
     * Output
        - Result<Viewer, String>
    */
    fn sign_in(&self, code: &str) -> Result<Viewer, String> {
        let failed = |error: ureq::Error| format!("GitHub sign-in failed: {error}");
        let token: Value = self
            .agent
            .post(&format!("{}/login/oauth/access_token", self.github_url))
            .set("Accept", "application/json")
            .send_form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("code", code),
                (
                    "redirect_uri",
                    &format!("{}/team/callback", self.public_url),
                ),
            ])
            .map_err(failed)?
            .into_json()
            .map_err(|error| error.to_string())?;
        let access = token["access_token"]
            .as_str()
            .ok_or("GitHub did not grant access (the sign-in may have expired); try again")?;
        let get = |path: &str| -> Result<Value, String> {
            self.agent
                .get(&format!("{}{path}", self.api_url))
                .set("Authorization", &format!("Bearer {access}"))
                .set("Accept", "application/vnd.github+json")
                .set("User-Agent", concat!("crane/", env!("CARGO_PKG_VERSION")))
                .call()
                .map_err(failed)?
                .into_json()
                .map_err(|error| error.to_string())
        };
        let login = get("/user")?["login"]
            .as_str()
            .ok_or("GitHub returned no login")?
            .to_ascii_lowercase();
        let orgs = get("/user/orgs?per_page=100")?
            .as_array()
            .map(|orgs| {
                orgs.iter()
                    .filter_map(|org| org["login"].as_str())
                    .map(str::to_ascii_lowercase)
                    .collect()
            })
            .unwrap_or_default();
        Ok(Viewer {
            login,
            orgs,
            expires: now_millis() + SESSION_MILLIS,
        })
    }
}

/** Read a cookie
 * Input
    - request: &Request - request
    - name: &str - cookie name
 * Output
    - Option<String>
*/
fn cookie(request: &Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .filter(|header| header.field.equiv("Cookie"))
        .flat_map(|header| header.value.as_str().split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value.to_string())
}

/** Decode %XX escapes and + in a query value
 * Input
    - text: &str - encoded text
 * Output
    - String
*/
fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => match u8::from_str_radix(text.get(index + 1..index + 3).unwrap_or(""), 16) {
                Ok(byte) => {
                    out.push(byte);
                    index += 3;
                    continue;
                }
                Err(_) => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            byte => out.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/** Percent-encode a query value
 * Input
    - text: &str - text
 * Output
    - String
*/
fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

/** Split a URL into its path and query parameters
 * Input
    - url: &str - request URL
 * Output
    - (String, BTreeMap<String, String>)
*/
fn split(url: &str) -> (String, BTreeMap<String, String>) {
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let parameters = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(key, value)| (decode(key), decode(value)))
        .collect();
    (path.to_string(), parameters)
}

/** Escape text for HTML
 * Input
    - text: &str - text
 * Output
    - String
*/
fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/** Wrap page content in the dashboard layout
 * Input
    - title: &str - page title
    - body: &str - HTML content (already escaped)
 * Output
    - String
*/
fn page(title: &str, body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{} · Foxx team</title><style>
:root{{--bg:#f7f7f5;--fg:#1c1c1a;--muted:#6b6b66;--line:#dcdcd6;--card:#fff;--accent:#9a4a12;--bad:#a1261b}}
@media (prefers-color-scheme: dark){{:root{{--bg:#151514;--fg:#ececea;--muted:#a3a39c;--line:#33332f;--card:#1e1e1c;--accent:#e08a4a;--bad:#ef7a6e}}}}
body{{margin:0;background:var(--bg);color:var(--fg);font:15px/1.5 system-ui,sans-serif}}main{{max-width:1100px;margin:0 auto;padding:24px 16px}}
h1{{font-size:22px;margin:0 0 4px}}h2{{font-size:17px;margin:28px 0 8px}}a{{color:var(--accent)}}.muted{{color:var(--muted)}}
table{{width:100%;border-collapse:collapse;background:var(--card);border:1px solid var(--line)}}th,td{{text-align:left;padding:7px 10px;border-bottom:1px solid var(--line);vertical-align:top}}
th{{font-size:13px;color:var(--muted);font-weight:600}}.scroll{{overflow-x:auto}}.bad{{color:var(--bad);font-weight:600}}code{{font-size:13px}}
.button{{display:inline-block;padding:9px 16px;border:1px solid var(--line);background:var(--card);border-radius:6px;text-decoration:none;color:var(--fg)}}
</style></head><body><main>{body}</main></body></html>"#,
        escape(title)
    )
}

/** Report whether a viewer may see a repository with this verified remote
 * Input
    - viewer: &Viewer - viewer
    - remote: &str - verified normalized remote, such as github.com/acme/widgets
 * Output
    - bool
*/
fn allowed(viewer: &Viewer, remote: &str) -> bool {
    let Some(owner) = remote
        .strip_prefix("github.com/")
        .and_then(|rest| rest.split('/').next())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    owner == viewer.login || viewer.orgs.contains(&owner)
}

/** Route a /team request
 * Input
    - team: Option<&Team> - settings, None when not configured
    - request: &Request - request
    - database: &Database - database
 * Output
    - Reply
*/
pub(super) fn handle(team: Option<&Team>, request: &Request, database: &Database) -> Reply {
    let Some(team) = team else {
        return Reply::html(
            503,
            page(
                "Not configured",
                "<h1>Team dashboard</h1><p>The team dashboard is not configured on this service. The operator sets CRANE_GITHUB_CLIENT_ID, CRANE_GITHUB_CLIENT_SECRET, and CRANE_PUBLIC_URL.</p>",
            ),
        );
    };
    let (path, query) = split(request.url());
    let result = match path.as_str() {
        "/team" | "/team/" => Ok(index(team, request, database)),
        "/team/login" => Ok(login(team)),
        "/team/callback" => callback(team, request, &query),
        "/team/logout" => Ok(Reply::redirect("/team")
            .with_header("Set-Cookie", &team.cookie("crane_session", "", 0))),
        "/team/repository" => Ok(repository(team, request, database, &query)),
        _ => Ok(Reply::html(404, page("Not found", "<h1>Not found</h1>"))),
    };
    result.unwrap_or_else(|error| {
        Reply::html(
            400,
            page(
                "Sign-in failed",
                &format!(
                    "<h1>Sign-in failed</h1><p>{}</p><p><a href=\"/team\">Back</a></p>",
                    escape(&error)
                ),
            ),
        )
    })
}

/** Start GitHub sign-in with a random state bound to a short-lived cookie
 * Input
    - team: &Team - settings
 * Output
    - Reply redirect to GitHub
*/
fn login(team: &Team) -> Reply {
    let state = random_bytes(16)
        .map(|bytes| hex(&bytes))
        .unwrap_or_default();
    let location = format!(
        "{}/login/oauth/authorize?client_id={}&redirect_uri={}&scope=read%3Aorg&state={state}",
        team.github_url,
        encode(&team.client_id),
        encode(&format!("{}/team/callback", team.public_url))
    );
    Reply::redirect(&location).with_header("Set-Cookie", &team.cookie("crane_oauth", &state, 600))
}

/** Finish GitHub sign-in: check the state, exchange the code, and set the session cookie
 * Input
    - team: &Team - settings
    - request: &Request - request
    - query: &BTreeMap<String, String> - code and state
 * Output
    - Result<Reply, String>
*/
fn callback(
    team: &Team,
    request: &Request,
    query: &BTreeMap<String, String>,
) -> Result<Reply, String> {
    let expected = cookie(request, "crane_oauth").filter(|state| !state.is_empty());
    if expected.is_none() || query.get("state") != expected.as_ref() {
        return Err("the sign-in request did not match this browser; start again".into());
    }
    let code = query.get("code").ok_or("GitHub returned no code")?;
    let viewer = team.sign_in(code)?;
    Ok(Reply::redirect("/team")
        .with_header(
            "Set-Cookie",
            &team.cookie(
                "crane_session",
                &team.session(&viewer),
                SESSION_MILLIS / 1000,
            ),
        )
        .with_header("Set-Cookie", &team.cookie("crane_oauth", "", 0)))
}

/** The repositories a viewer may see, or the sign-in page
 * Input
    - team: &Team - settings
    - request: &Request - request
    - database: &Database - database
 * Output
    - Reply
*/
fn index(team: &Team, request: &Request, database: &Database) -> Reply {
    let Some(viewer) = team.viewer(request) else {
        return Reply::html(
            200,
            page(
                "Sign in",
                "<h1>Foxx team dashboard</h1><p class=\"muted\">See every teammate's agent activity, alerts, and metrics for repositories you or your GitHub organizations own.</p><p><a class=\"button\" href=\"/team/login\">Sign in with GitHub</a></p>",
            ),
        );
    };
    let repositories = match store::verified_repositories(database) {
        Ok(repositories) => repositories,
        Err(_) => {
            return Reply::html(
                503,
                page("Unavailable", "<h1>The database is unavailable</h1>"),
            )
        }
    };
    let rows = repositories
        .iter()
        .filter(|(_, remote)| allowed(&viewer, remote))
        .map(|(id, remote)| {
            format!(
                "<tr><td><a href=\"/team/repository?id={id}\">{}</a></td><td><code>{}…</code></td></tr>",
                escape(remote),
                &id[..16]
            )
        })
        .collect::<String>();
    let list = if rows.is_empty() {
        "<p>No repositories yet. A repository appears here once a teammate's Crane uploads from a clone whose origin is a GitHub repository owned by you or one of your organizations.</p>".to_string()
    } else {
        format!("<div class=\"scroll\"><table><tr><th>Repository</th><th>Identity</th></tr>{rows}</table></div>")
    };
    Reply::html(
        200,
        page(
            "Repositories",
            &format!(
                "<h1>Repositories</h1><p class=\"muted\">Signed in as {} · organizations: {} · <a href=\"/team/logout\">Sign out</a></p>{list}<p class=\"muted\">An organization is listed only when it allows this app (GitHub organization settings, third-party access).</p>",
                escape(&viewer.login),
                escape(&if viewer.orgs.is_empty() { "none".to_string() } else { viewer.orgs.join(", ") })
            ),
        ),
    )
}

/** Format a cell value
 * Input
    - value: &Value - JSON value
 * Output
    - String escaped text
*/
fn cell(value: &Value) -> String {
    let text = match value {
        Value::String(text) => text.clone(),
        Value::Null => "–".into(),
        other => other.to_string(),
    };
    escape(&text.chars().take(120).collect::<String>())
}

/** Format Unix milliseconds stored in a JSON value
 * Input
    - value: &Value - number
    - scale: u64 - 1 for milliseconds, 1000 for seconds
 * Output
    - String
*/
fn time(value: &Value, scale: u64) -> String {
    value
        .as_u64()
        .or_else(|| value.as_f64().map(|number| number as u64))
        .map_or_else(|| "–".into(), |number| iso_time(number * scale))
}

/** One repository's team view
 * Input
    - team: &Team - settings
    - request: &Request - request
    - database: &Database - database
    - query: &BTreeMap<String, String> - id
 * Output
    - Reply
*/
fn repository(
    team: &Team,
    request: &Request,
    database: &Database,
    query: &BTreeMap<String, String>,
) -> Reply {
    let Some(viewer) = team.viewer(request) else {
        return Reply::redirect("/team");
    };
    let id = query.get("id").cloned().unwrap_or_default();
    let forbidden = || {
        Reply::html(
            403,
            page(
                "Not allowed",
                "<h1>Not allowed</h1><p>This repository is not owned by your GitHub account or one of your organizations.</p><p><a href=\"/team\">Back</a></p>",
            ),
        )
    };
    if !is_digest(&id) {
        return forbidden();
    }
    let remote = match store::verified_remote(database, &id) {
        Ok(Some(remote)) if allowed(&viewer, &remote) => remote,
        Ok(_) => return forbidden(),
        Err(_) => {
            return Reply::html(
                503,
                page("Unavailable", "<h1>The database is unavailable</h1>"),
            )
        }
    };
    match store::team_view(database, &id) {
        Ok(view) => Reply::html(200, page(&remote, &render(&remote, &viewer, &view))),
        Err(_) => Reply::html(
            503,
            page("Unavailable", "<h1>The database is unavailable</h1>"),
        ),
    }
}

/** Render a repository's team view
 * Input
    - remote: &str - repository remote
    - viewer: &Viewer - viewer
    - view: &TeamView - data
 * Output
    - String HTML
*/
fn render(remote: &str, viewer: &Viewer, view: &TeamView) -> String {
    let name = |installation: &Value| {
        let key = installation.as_str().unwrap_or_default();
        let actor = view
            .members
            .iter()
            .find(|member| member["installation"] == *installation)
            .and_then(|member| member["actor"].as_str())
            .unwrap_or("unknown");
        format!(
            "{} <span class=\"muted\"><code>{}</code></span>",
            escape(actor),
            escape(&key[..key.len().min(8)])
        )
    };
    let members = view
        .members
        .iter()
        .map(|member| {
            let stats = view
                .activity
                .iter()
                .find(|row| row["_id"] == member["installation"])
                .cloned()
                .unwrap_or(Value::Null);
            let denied = stats["denied"].as_u64().unwrap_or(0);
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td{}>{denied}</td><td>{}</td></tr>",
                name(&member["installation"]),
                cell(&stats["events"]),
                cell(&stats["tool_calls"]),
                cell(&stats["sessions"]),
                if denied > 0 { " class=\"bad\"" } else { "" },
                time(&stats["last"], 1),
            )
        })
        .collect::<String>();
    let sessions = view
        .sessions
        .iter()
        .map(|session| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                name(&session["installation"]),
                cell(&session["provider"]),
                time(&session["started_at"], 1),
                if session["ended_at"].is_null() {
                    "active".to_string()
                } else {
                    format!("ended {}", time(&session["ended_at"], 1))
                },
                cell(&session["bypass_attempts"]),
                cell(&session["violations"]),
            )
        })
        .collect::<String>();
    let alerts = view
        .alerts
        .iter()
        .map(|alert| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                time(&alert["at"], 1000),
                cell(&alert["severity"]),
                cell(&alert["kind"]),
                cell(&alert["title"]),
                name(&alert["installation"]),
            )
        })
        .collect::<String>();
    let metrics = view
        .rollups
        .iter()
        .map(|rollup| {
            let rows = rollup["metrics"]
                .as_array()
                .map(|metrics| {
                    metrics
                        .iter()
                        .map(|metric| {
                            format!(
                                "<tr><td>{}</td><td>{}</td><td class=\"muted\">{}</td></tr>",
                                cell(&metric["name"]),
                                cell(&metric["value"]),
                                cell(&metric["status"])
                            )
                        })
                        .collect::<String>()
                })
                .unwrap_or_default();
            format!(
                "<details><summary>{}</summary><div class=\"scroll\"><table><tr><th>Metric</th><th>Value</th><th>How it is known</th></tr>{rows}</table></div></details>",
                name(&rollup["installation"])
            )
        })
        .collect::<String>();
    let table = |headers: &[&str], rows: String, empty: &str| {
        if rows.is_empty() {
            format!("<p class=\"muted\">{empty}</p>")
        } else {
            format!(
                "<div class=\"scroll\"><table><tr>{}</tr>{rows}</table></div>",
                headers
                    .iter()
                    .map(|header| format!("<th>{header}</th>"))
                    .collect::<String>()
            )
        }
    };
    format!(
        "<p class=\"muted\"><a href=\"/team\">Repositories</a> · signed in as {} · <a href=\"/team/logout\">Sign out</a></p><h1>{}</h1><p class=\"muted\">Names are self-reported Git user names; the short code identifies each machine's upload key.</p>\
         <h2>Team members</h2>{}<h2>Recent alerts</h2>{}<h2>Recent sessions</h2>{}<h2>Metrics by member</h2>{}",
        escape(&viewer.login),
        escape(remote),
        table(&["Member", "Events", "Tool calls verified", "Sessions", "Denied", "Last activity"], members, "No uploads yet."),
        table(&["Time", "Severity", "Kind", "Alert", "Member"], alerts, "No alerts."),
        table(&["Member", "Agent", "Started", "Status", "Bypass attempts", "Violations"], sessions, "No sessions yet."),
        if metrics.is_empty() { "<p class=\"muted\">No metrics yet.</p>".to_string() } else { metrics },
    )
}

#[cfg(test)]
mod tests {
    use super::{allowed, decode, escape, Viewer};

    /** Check owner matching, escaping, and query decoding
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn viewers_see_only_their_owners() {
        let viewer = Viewer {
            login: "alice".into(),
            orgs: vec!["acme".into()],
            expires: 0,
        };
        assert!(allowed(&viewer, "github.com/Acme/widgets"));
        assert!(allowed(&viewer, "github.com/alice/notes"));
        assert!(!allowed(&viewer, "github.com/other/widgets"));
        assert!(!allowed(&viewer, "gitlab.com/acme/widgets"));
        assert!(!allowed(&viewer, "local:/tmp/acme"));
        assert_eq!(escape("<a href='x'>"), "&lt;a href=&#39;x&#39;&gt;");
        assert_eq!(decode("a%2Fb+c"), "a/b c");
    }
}
