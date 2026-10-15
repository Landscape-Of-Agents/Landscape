// Signing in to the dashboard with GitHub (OAuth, scope read:org). The service keeps no GitHub
// token: after sign-in it sets an eight-hour session cookie (HttpOnly, SameSite=Lax, Secure over
// HTTPS) holding the viewer's login and organizations, signed with a key the service keeps in its
// database. A random state bound to a short-lived cookie protects the sign-in from cross-site
// forgery. A viewer may see a repository whose verified remote is github.com/OWNER/NAME when OWNER
// is the viewer (a personal account) or one of the viewer's organizations.
//
// Only CRANE_GITHUB_CLIENT_ID and CRANE_GITHUB_CLIENT_SECRET (a GitHub OAuth app) are required. The
// public address comes from CRANE_PUBLIC_URL, else RENDER_EXTERNAL_URL (set by Render), else the
// request's Host and X-Forwarded-Proto headers; GitHub only redirects to the callback URL
// registered for the app, so a forged Host header cannot redirect a sign-in elsewhere.
// CRANE_GITHUB_URL and CRANE_GITHUB_API_URL point at GitHub (changed only by tests).

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tiny_http::Request;

use crate::platform::now_millis;
use crate::server::http::{cookie, header};
use crate::trust::crypto::{hex, unhex, verify_signature, SigningIdentity};

/** Signature domain of session cookies */
const SESSION_DOMAIN: &str = "foxx/dashboard/session/v1";

/** Signature domain of an approval intent cookie */
const INTENT_DOMAIN: &str = "foxx/dashboard/intent/v1";

/** What a person asked to approve, carried through GitHub sign-in in a signed cookie
 * Fields
    - action: String - link (a machine's upload key), owner (the owner key), or grant (credits)
    - code: String - the one-time code (empty for a grant)
    - state: String - the sign-in state it belongs to
    - grant: Option<Value> - for a grant: {repository, session, amount, reason}
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Intent {
    pub(crate) action: String,
    pub(crate) code: String,
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) grant: Option<Value>,
    #[serde(default)]
    pub(crate) dashboard_agent: Option<Value>,
}

/** The viewer's access to a repository, as GitHub reports it
 * Fields
    - admin: bool - administers it
    - read: bool - can read it (any of pull, triage, push, maintain, admin)
*/
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Access {
    pub(crate) admin: bool,
    pub(crate) read: bool,
}

/** Lifetime of a session */
pub(crate) const SESSION_MILLIS: u64 = 8 * 60 * 60 * 1000;

/** A signed-in viewer
 * Fields
    - login: String - GitHub login (lowercase)
    - orgs: Vec<String> - GitHub organizations (lowercase)
    - expires: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Viewer {
    pub(crate) login: String,
    pub(crate) orgs: Vec<String>,
    pub(crate) expires: u64,
}

impl Viewer {
    /** Report whether the viewer may see an account's repositories
     * Input
        - account: &str - GitHub user or organization
     * Output
        - bool
    */
    pub(crate) fn owns(&self, account: &str) -> bool {
        let account = account.to_ascii_lowercase();
        account == self.login || self.orgs.contains(&account)
    }

    /** Report whether the viewer may see a repository with this verified remote
     * Input
        - remote: &str - normalized remote, such as github.com/acme/widgets
     * Output
        - bool
    */
    pub(crate) fn allowed(&self, remote: &str) -> bool {
        account(remote).is_some_and(|account| self.owns(&account))
    }
}

/** Return the GitHub account that owns a remote
 * Input
    - remote: &str - normalized remote
 * Output
    - Option<String> lowercase account
*/
pub(crate) fn account(remote: &str) -> Option<String> {
    remote
        .strip_prefix("github.com/")
        .and_then(|rest| rest.split('/').next())
        .filter(|owner| !owner.is_empty())
        .map(str::to_ascii_lowercase)
}

/** Sign-in settings
 * Fields
    - client_id, client_secret: String - the GitHub OAuth app
    - github_url, api_url: String - GitHub's web and API addresses
    - key: SigningIdentity - signs session cookies
    - agent: ureq::Agent - HTTP client for GitHub
*/
pub(crate) struct Auth {
    client_id: String,
    client_secret: String,
    github_url: String,
    api_url: String,
    key: SigningIdentity,
    agent: ureq::Agent,
}

impl Auth {
    /** Read the settings; None when no OAuth app is configured
     * Input
        - session_seed: &str - the service's session key seed (64 hex characters)
     * Output
        - Result<Option<Auth>, String>
        - Error if the client secret is missing or the seed is malformed
    */
    pub(crate) fn from_environment(session_seed: &str) -> Result<Option<Self>, String> {
        let variable = |name: &str| {
            std::env::var(name)
                .ok()
                .filter(|value| !value.trim().is_empty())
        };
        let Some(client_id) = variable("CRANE_GITHUB_CLIENT_ID") else {
            return Ok(None);
        };
        let client_secret = variable("CRANE_GITHUB_CLIENT_SECRET")
            .ok_or("set CRANE_GITHUB_CLIENT_SECRET with CRANE_GITHUB_CLIENT_ID")?;
        let base = |name: &str, default: &str| {
            variable(name)
                .unwrap_or_else(|| default.into())
                .trim_end_matches('/')
                .to_string()
        };
        Ok(Some(Self {
            client_id,
            client_secret,
            github_url: base("CRANE_GITHUB_URL", "https://github.com"),
            api_url: base("CRANE_GITHUB_API_URL", "https://api.github.com"),
            key: SigningIdentity::from_seed_hex(session_seed)?,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(15))
                .build(),
        }))
    }

    /** Return the public address of the service for a request
     * Input
        - request: &Request - request
     * Output
        - String without a trailing slash
    */
    pub(crate) fn public_url(request: &Request) -> String {
        for name in ["CRANE_PUBLIC_URL", "RENDER_EXTERNAL_URL"] {
            if let Some(url) = std::env::var(name)
                .ok()
                .filter(|url| !url.trim().is_empty())
            {
                return url.trim_end_matches('/').to_string();
            }
        }
        let proto = header(request, "X-Forwarded-Proto").unwrap_or_else(|| "http".into());
        let host = header(request, "Host").unwrap_or_else(|| "localhost".into());
        format!("{proto}://{host}")
    }

    /** Build a cookie header value
     * Input
        - request: &Request - request (decides the Secure flag)
        - name: &str - cookie name
        - value: &str - cookie value
        - max_age: u64 - seconds (0 deletes the cookie)
     * Output
        - String
    */
    pub(crate) fn cookie(request: &Request, name: &str, value: &str, max_age: u64) -> String {
        format!(
            "{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{}",
            if Self::public_url(request).starts_with("https://") {
                "; Secure"
            } else {
                ""
            }
        )
    }

    /** Read and check the viewer's session cookie
     * Input
        - request: &Request - request
     * Output
        - Option<Viewer>, None when absent, forged, or expired
    */
    pub(crate) fn viewer(&self, request: &Request) -> Option<Viewer> {
        let value = cookie(request, "foxx_session")?;
        let (payload, signature) = value.split_once('.')?;
        let payload = unhex(payload).ok()?;
        verify_signature(&self.key.public_hex(), SESSION_DOMAIN, &payload, signature).ok()?;
        let viewer: Viewer = serde_json::from_slice(&payload).ok()?;
        (viewer.expires > now_millis()).then_some(viewer)
    }

    /** Sign a session cookie value for a viewer
     * Input
        - viewer: &Viewer - viewer
     * Output
        - String
    */
    pub(crate) fn session(&self, viewer: &Viewer) -> String {
        let payload = serde_json::to_vec(viewer).unwrap_or_default();
        format!(
            "{}.{}",
            hex(&payload),
            self.key.sign(SESSION_DOMAIN, &payload)
        )
    }

    /** Return GitHub's authorization address for a sign-in
     * Input
        - request: &Request - request
        - state: &str - random state
        - private: bool - also ask for repository access (needed only to check a person's role on
          a private repository during an approval; the token is used once and never kept)
     * Output
        - String
    */
    pub(crate) fn authorize_url(&self, request: &Request, state: &str, private: bool) -> String {
        format!(
            "{}/login/oauth/authorize?client_id={}&redirect_uri={}&scope={}&state={state}",
            self.github_url,
            encode(&self.client_id),
            encode(&format!("{}/auth/callback", Self::public_url(request))),
            if private {
                "read%3Aorg%20repo"
            } else {
                "read%3Aorg"
            }
        )
    }

    /** Sign an approval intent for its cookie
     * Input
        - intent: &Intent - intent
     * Output
        - String
    */
    pub(crate) fn seal(&self, intent: &Intent) -> String {
        let payload = serde_json::to_vec(intent).unwrap_or_default();
        format!(
            "{}.{}",
            hex(&payload),
            self.key.sign(INTENT_DOMAIN, &payload)
        )
    }

    /** Read and check the approval intent cookie of a request
     * Input
        - request: &Request - request
     * Output
        - Option<Intent>, None when absent or forged
    */
    pub(crate) fn intent(&self, request: &Request) -> Option<Intent> {
        let value = cookie(request, "foxx_intent")?;
        let (payload, signature) = value.split_once('.')?;
        let payload = unhex(payload).ok()?;
        verify_signature(&self.key.public_hex(), INTENT_DOMAIN, &payload, signature).ok()?;
        serde_json::from_slice(&payload).ok()
    }

    /** Ask GitHub for a person's access to a repository, with the token of their sign-in
     * Input
        - token: &str - OAuth access token (used here only, never stored)
        - remote: &str - normalized remote, github.com/OWNER/NAME
     * Output
        - Result<Access, String>; no access when GitHub does not show the repository to them
    */
    pub(crate) fn access(&self, token: &str, remote: &str) -> Result<Access, String> {
        let Some(path) = remote.strip_prefix("github.com/") else {
            return Ok(Access::default());
        };
        let answer = self
            .agent
            .get(&format!("{}/repos/{path}", self.api_url))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/vnd.github+json")
            .set("User-Agent", concat!("crane/", env!("CARGO_PKG_VERSION")))
            .call();
        let repository: Value = match answer {
            Ok(response) => response.into_json().map_err(|error| error.to_string())?,
            Err(ureq::Error::Status(404 | 403, _)) => return Ok(Access::default()),
            Err(error) => return Err(format!("GitHub could not be asked: {error}")),
        };
        let permissions = &repository["permissions"];
        let flag = |name: &str| permissions[name].as_bool().unwrap_or(false);
        Ok(Access {
            admin: flag("admin"),
            read: ["pull", "triage", "push", "maintain", "admin"]
                .iter()
                .any(|name| flag(name)),
        })
    }

    /** Exchange an OAuth code for the viewer's login and organizations, and the access token
     * (which the caller may use once for an approval and must not keep)
     * Input
        - request: &Request - request
        - code: &str - OAuth code
     * Output
        - Result<(Viewer, String), String>
    */
    pub(crate) fn sign_in(
        &self,
        request: &Request,
        code: &str,
    ) -> Result<(Viewer, String), String> {
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
                    &format!("{}/auth/callback", Self::public_url(request)),
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
        Ok((
            Viewer {
                login,
                orgs,
                expires: now_millis() + SESSION_MILLIS,
            },
            access.to_string(),
        ))
    }

    /** Verify the token's GitHub identity and a direct account entry in the repository's default
     * branch CODEOWNERS file. The token is used only for these requests and is never retained.
     */
    pub(crate) fn codeowner(&self, token: &str, remote: &str) -> Result<String, String> {
        if token.is_empty() || token.len() > 4096 || token.chars().any(char::is_whitespace) {
            return Err("a valid GitHub CLI token is required".into());
        }
        let parts = remote.split('/').collect::<Vec<_>>();
        if parts.len() != 3
            || parts[0].is_empty()
            || parts[1].is_empty()
            || parts[2].is_empty()
            || parts[1..].iter().any(|part| {
                !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
            })
        {
            return Err("the repository remote is not a normalized GitHub repository".into());
        }
        let host_allowed = parts[0] == "github.com"
            || std::env::var("CRANE_GITHUB_HOSTS")
                .unwrap_or_default()
                .split(',')
                .any(|host| host.trim().eq_ignore_ascii_case(parts[0]));
        if !host_allowed {
            return Err(format!(
                "GitHub host '{}' is not configured on this service",
                parts[0]
            ));
        }
        let repository_path = format!("{}/{}", parts[1], parts[2]);
        let api_get = |path: &str, accept: &str| -> Result<(u16, String), String> {
            let response = match self
                .agent
                .get(&format!("{}{path}", self.api_url))
                .set("Authorization", &format!("Bearer {token}"))
                .set("Accept", accept)
                .set("User-Agent", concat!("crane/", env!("CARGO_PKG_VERSION")))
                .call()
            {
                Ok(response) => response,
                Err(ureq::Error::Status(_, response)) => response,
                Err(error) => return Err(format!("GitHub authorization request failed: {error}")),
            };
            let status = response.status();
            let mut body = String::new();
            response
                .into_reader()
                .take(1_000_001)
                .read_to_string(&mut body)
                .map_err(|error| format!("could not read GitHub's response: {error}"))?;
            if body.len() > 1_000_000 {
                return Err("GitHub response exceeded the one-megabyte limit".into());
            }
            Ok((status, body))
        };
        let json_get = |path: &str| -> Result<Value, String> {
            let (status, body) = api_get(path, "application/vnd.github+json")?;
            if !(200..300).contains(&status) {
                return Err(format!("GitHub returned HTTP {status} for {path}"));
            }
            serde_json::from_str(&body)
                .map_err(|error| format!("GitHub returned invalid JSON for {path}: {error}"))
        };
        let user = json_get("/user")?;
        let login = user["login"]
            .as_str()
            .filter(|login| !login.is_empty())
            .ok_or("GitHub returned no login for this token")?
            .to_ascii_lowercase();
        let repository = json_get(&format!("/repos/{repository_path}"))?;
        let full_name = repository["full_name"]
            .as_str()
            .ok_or("GitHub returned no repository identity")?;
        if !full_name.eq_ignore_ascii_case(&repository_path) {
            return Err("GitHub repository identity did not match the requested remote".into());
        }
        let branch = repository["default_branch"]
            .as_str()
            .filter(|branch| !branch.is_empty())
            .ok_or("GitHub returned no default branch")?;
        for file in [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"] {
            let path = format!(
                "/repos/{repository_path}/contents/{file}?ref={}",
                encode(branch)
            );
            let (status, contents) = api_get(&path, "application/vnd.github.raw+json")?;
            if status == 404 {
                continue;
            }
            if !(200..300).contains(&status) {
                return Err(format!(
                    "GitHub returned HTTP {status} while reading {file}"
                ));
            }
            if codeowners_contains(&contents, &login) {
                return Ok(login);
            }
            return Err(format!(
                "GitHub login @{login} is not directly listed in the default-branch CODEOWNERS file"
            ));
        }
        Err("the repository has no CODEOWNERS file on its default branch".into())
    }
}

fn codeowners_contains(contents: &str, login: &str) -> bool {
    contents.lines().any(|line| {
        let mut fields = line
            .split('#')
            .next()
            .unwrap_or_default()
            .split_whitespace();
        fields.next();
        fields.any(|owner| {
            owner
                .strip_prefix('@')
                .is_some_and(|name| !name.contains('/') && name.eq_ignore_ascii_case(login))
        })
    })
}

/** Percent-encode a query value
 * Input
    - text: &str - text
 * Output
    - String
*/
pub(crate) fn encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{account, codeowners_contains, Viewer};

    /** Check owner matching for personal accounts and organizations
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn viewers_see_only_their_accounts() {
        let viewer = Viewer {
            login: "alice".into(),
            orgs: vec!["acme".into()],
            expires: 0,
        };
        assert!(viewer.allowed("github.com/Acme/widgets"));
        assert!(viewer.allowed("github.com/alice/notes"));
        assert!(!viewer.allowed("github.com/other/widgets"));
        assert!(!viewer.allowed("gitlab.com/acme/widgets"));
        assert!(!viewer.allowed("local:/tmp/acme"));
        assert_eq!(account("github.com/Acme/w").as_deref(), Some("acme"));
        assert!(viewer.owns("ACME") && !viewer.owns("other"));
    }

    #[test]
    fn codeowners_authorization_requires_a_direct_account_entry() {
        let contents = "* @alice\nsrc/ @acme/team\n# @bob\n";
        assert!(codeowners_contains(contents, "ALICE"));
        assert!(!codeowners_contains(contents, "team"));
        assert!(!codeowners_contains(contents, "bob"));
    }
}
