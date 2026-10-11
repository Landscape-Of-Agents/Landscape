// GitHub: read access to the repository through a fine-grained token, the gh CLI, or a GitHub App,
// for the repository commands. Verification reads the repository. GitHub receives no alerts.

use serde_json::{json, Map, Value};

use super::*;
use crate::governance::workspace::Workspace;
use crate::platform::http::request;

/** The GitHub provider */
pub(crate) struct GitHub;

impl Provider for GitHub {
    /** Return the integration's name
     * Input
        - None (uses self)
     * Output
        - &'static str "github"
    */
    fn name(&self) -> &'static str {
        "github"
    }

    /** Ask for the GitHub settings and credentials (see ask below)
     * Input
        - prompter, settings, secrets, workspace - see Provider::ask
     * Output
        - Result<(), String>
    */
    fn ask(
        &self,
        prompter: &mut Prompter,
        settings: &mut Map<String, Value>,
        secrets: &mut Map<String, Value>,
        workspace: &Workspace,
    ) -> Result<(), String> {
        ask(prompter, settings, secrets, workspace)
    }

    /** Check the saved GitHub settings against the real service (see verify below)
     * Input
        - integration: &Integration - saved settings
        - secrets: &Map<String, Value> - credentials
     * Output
        - Result<String, String> what was verified
    */
    fn verify(
        &self,
        integration: &Integration,
        secrets: &Map<String, Value>,
    ) -> Result<String, String> {
        verify(integration, secrets)
    }
}

/** Ask for GitHub parameters
 * Input
    - prompter: &mut Prompter - prompts
    - settings: &mut Map<String, Value> - non-secret settings
    - secrets: &mut Map<String, Value> - secrets
    - workspace: &Workspace - repository (its connection supplies the default repository)
 * Output
    - Result<(), String>
*/
fn ask(
    prompter: &mut Prompter,
    settings: &mut Map<String, Value>,
    secrets: &mut Map<String, Value>,
    workspace: &Workspace,
) -> Result<(), String> {
    let auth = prompter.choice(
        "Authentication",
        &[
            "GitHub CLI login (crane github)",
            "Fine-grained personal access token",
            "GitHub App installation",
        ],
        0,
    )?;
    settings.insert(
        "auth".into(),
        json!(["gh_cli", "fine_grained_pat", "github_app"][auth]),
    );
    let default_repository = crate::github::load(workspace)
        .ok()
        .flatten()
        .filter(|connection| !connection.normalized.starts_with("local:"))
        .and_then(|connection| {
            connection
                .normalized
                .split_once('/')
                .map(|(_, slug)| slug.to_string())
        });
    let repository = prompter.text(
        "Repository (OWNER/NAME)",
        default_repository.as_deref(),
        &|value| {
            let parts = value.split('/').collect::<Vec<_>>();
            if parts.len() == 2 && parts.iter().all(|part| !part.is_empty()) {
                Ok(())
            } else {
                Err("use OWNER/NAME".into())
            }
        },
    )?;
    settings.insert("repository".into(), json!(repository));
    let api = prompter.text(
        "API base URL (GitHub Enterprise: https://HOST/api/v3)",
        Some("https://api.github.com"),
        &https_url,
    )?;
    settings.insert("api_url".into(), json!(api.trim_end_matches('/')));
    match auth {
        1 => {
            secrets.insert(
                "token".into(),
                json!(prompter.secret("Fine-grained token", false, &|value| {
                    if value.starts_with("github_pat_") || value.starts_with("ghp_") {
                        Ok(())
                    } else {
                        Err("a GitHub token starts with github_pat_ or ghp_".into())
                    }
                })?),
            );
        }
        2 => {
            settings.insert(
                "app_id".into(),
                json!(prompter.text("App ID", None, &digits)?),
            );
            settings.insert(
                "installation_id".into(),
                json!(prompter.text("Installation ID", None, &digits)?),
            );
            settings.insert(
                "private_key_path".into(),
                json!(prompter.text(
                    "Path to the App private key (.pem, kept outside the repository)",
                    None,
                    &|value| {
                        let path = std::path::Path::new(value);
                        if !path.is_file() {
                            return Err("the file does not exist".into());
                        }
                        if path
                            .canonicalize()
                            .ok()
                            .zip(workspace.root.canonicalize().ok())
                            .is_some_and(|(key, root)| key.starts_with(root))
                        {
                            return Err("keep the private key outside the repository".into());
                        }
                        Ok(())
                    }
                )?),
            );
        }
        _ => {}
    }
    let features = prompter.choice(
        "What Crane may do",
        &[
            "Read only (pull requests, checks, workflows)",
            "Read and post pull-request comments with Crane reports",
            "Read and set commit statuses",
        ],
        0,
    )?;
    settings.insert(
        "features".into(),
        json!(["read", "comments", "statuses"][features]),
    );
    Ok(())
}

/** Read the repository with the configured credentials
 * Input
    - integration: &Integration - saved settings
    - secrets: &Map<String, Value> - credentials
 * Output
    - Result<String, String> what was verified
*/
fn verify(integration: &Integration, secrets: &Map<String, Value>) -> Result<String, String> {
    let repository = setting(integration, "repository")?;
    match setting(integration, "auth")?.as_str() {
        "fine_grained_pat" => {
            let (status, body) = request(
                "GET",
                &format!("{}/repos/{repository}", setting(integration, "api_url")?),
                &[
                    format!(
                        "Authorization: Bearer {}",
                        secret(integration, secrets, "token")?
                    ),
                    "Accept: application/vnd.github+json".into(),
                    "User-Agent: crane".into(),
                ],
                None,
            )?;
            if status == 200 {
                Ok(format!("the token can read {repository}"))
            } else {
                Err(failed(status, &body))
            }
        }
        "gh_cli" => {
            let output = crate::platform::process::run(
                std::process::Command::new("gh").args([
                    "api",
                    &format!("repos/{repository}"),
                    "--jq",
                    ".full_name",
                ]),
                std::time::Duration::from_secs(20),
            )?;
            if output.success {
                Ok(format!("gh can read {}", output.stdout.trim()))
            } else {
                Err(output.stderr.trim().to_string())
            }
        }
        _ => Ok(
            "GitHub App credentials are stored; Crane does not mint installation tokens in 0.4"
                .into(),
        ),
    }
}
