// The live GitHub connector: pull requests and the files a commit changed (resource type Github),
// and the check runs of a commit (resource type Ci), read through the GitHub CLI ('gh api', the
// same CLI 'crane github' connects with, CRANE_GH to override it) and never written. It holds no
// credential itself: gh keeps its own authentication. GitHub has no general test-report API, so
// the connector does not claim test reports (an investigation then says no source provides them,
// instead of "none exist"). Every gh call runs within the investigation's deadline, and every
// failure keeps its kind: a missing object is an empty answer, 401 and 403 are unauthorized, rate
// limits are rate limited, 5xx is unavailable, and running out of time is a timeout.

use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::connector::{
    ConnectorOutcome, ConnectorStatus, HealthReport, QuerySpec, ResourceConnector, SourceRecord,
};
use crate::capture::format::record::EvidenceType;

/** Time allowed for one gh call when no investigation deadline applies */
const DEFAULT_CALL: Duration = Duration::from_secs(30);

/** What one GitHub API call answered
 * Variants
    - Found(Value) - the JSON body
    - Missing - the object does not exist (HTTP 404)
    - Failed(ConnectorStatus, String) - the call failed, with its kind and explanation
*/
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ApiAnswer {
    Found(Value),
    Missing,
    Failed(ConnectorStatus, String),
}

/** Something that answers GitHub API paths: gh in practice, recorded answers in tests */
pub(crate) trait GitHubApi {
    /** Answer one GET of an API path
     * Input
        - path: &str - API path, for example "repos/acme/payments/pulls/418"
        - deadline: Option<Instant> - when the investigation's time budget runs out
     * Output
        - ApiAnswer
    */
    fn get(&self, path: &str, deadline: Option<Instant>) -> ApiAnswer;
}

/** The GitHub CLI as the API */
pub(crate) struct GhCli;

impl GitHubApi for GhCli {
    /** Run 'gh api PATH' and classify the result
     * Input
        - path: &str - API path
        - deadline: Option<Instant> - deadline
     * Output
        - ApiAnswer
    */
    fn get(&self, path: &str, deadline: Option<Instant>) -> ApiAnswer {
        let budget = match deadline {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => DEFAULT_CALL,
        };
        if budget.is_zero() {
            return ApiAnswer::Failed(
                ConnectorStatus::Timeout,
                "no time left in the investigation's budget".into(),
            );
        }
        let binary = std::env::var("CRANE_GH").unwrap_or_else(|_| "gh".into());
        let mut command = Command::new(binary);
        command.args(["api", "-H", "Accept: application/vnd.github+json", path]);
        match crate::platform::process::run(&mut command, budget) {
            Err(error) => ApiAnswer::Failed(
                ConnectorStatus::Unavailable,
                format!("the GitHub CLI could not run: {error}"),
            ),
            Ok(output) if output.timed_out => ApiAnswer::Failed(
                ConnectorStatus::Timeout,
                "GitHub did not answer within the time budget".into(),
            ),
            Ok(output) if output.success => match serde_json::from_str(&output.stdout) {
                Ok(value) => ApiAnswer::Found(value),
                Err(error) => ApiAnswer::Failed(
                    ConnectorStatus::Failed,
                    format!("GitHub answered with something that is not JSON: {error}"),
                ),
            },
            Ok(output) => classify(&format!("{}\n{}", output.stderr, output.stdout)),
        }
    }
}

/** Classify a failed GitHub call from what gh printed
 * Input
    - message: &str - gh's output
 * Output
    - ApiAnswer
*/
pub(crate) fn classify(message: &str) -> ApiAnswer {
    let lower = message.to_ascii_lowercase();
    let text = message
        .trim()
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    if lower.contains("http 404") || lower.contains("not found") {
        ApiAnswer::Missing
    } else if lower.contains("rate limit") || lower.contains("http 429") {
        ApiAnswer::Failed(ConnectorStatus::RateLimited, text)
    } else if lower.contains("http 401")
        || lower.contains("http 403")
        || lower.contains("gh auth login")
        || lower.contains("authentication")
    {
        ApiAnswer::Failed(ConnectorStatus::Unauthorized, text)
    } else if lower.contains("http 5") {
        ApiAnswer::Failed(ConnectorStatus::Unavailable, text)
    } else {
        ApiAnswer::Failed(ConnectorStatus::Failed, text)
    }
}

/** Parse an RFC 3339 UTC timestamp ("2026-10-21T10:00:00Z") into Unix milliseconds
 * Input
    - text: &str - timestamp
 * Output
    - Option<u64>, None when it is not one
*/
pub(crate) fn unix_millis(text: &str) -> Option<u64> {
    let text = text.trim().strip_suffix('Z')?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let time = time.split('.').next()?;
    let mut time = time.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm)
    let shifted = if month <= 2 { year - 1 } else { year };
    let era = shifted.div_euclid(400);
    let of_era = shifted - era * 400;
    let month_index = (month + 9) % 12;
    let of_year = (153 * month_index + 2) / 5 + day - 1;
    let of_cycle = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    let days = era * 146_097 + of_cycle - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds).ok().map(|seconds| seconds * 1000)
}

/** The live GitHub connector of one repository
 * Fields
    - repository: String - OWNER/NAME
    - label: String - the repository label evidence is scoped to (github.com/OWNER/NAME)
    - api: Box<dyn GitHubApi> - how API paths are answered
*/
pub(crate) struct GitHubConnector {
    repository: String,
    label: String,
    api: Box<dyn GitHubApi>,
}

impl std::fmt::Debug for GitHubConnector {
    /** Show the repository (the API answerer is not shown)
     * Input
        - formatter: &mut std::fmt::Formatter - output
     * Output
        - std::fmt::Result
    */
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GitHubConnector")
            .field("repository", &self.repository)
            .finish()
    }
}

impl GitHubConnector {
    /** A connector for a repository label such as "github.com/OWNER/NAME"
     * Input
        - label: &str - normalized remote
        - api: Box<dyn GitHubApi> - API answerer (GhCli for the live one)
     * Output
        - Option<GitHubConnector>, None when the label is not a GitHub repository
    */
    pub(crate) fn for_remote(label: &str, api: Box<dyn GitHubApi>) -> Option<Self> {
        let mut parts = label.splitn(3, '/');
        let (host, owner, name) = (parts.next()?, parts.next()?, parts.next()?);
        let hosts = std::env::var("CRANE_GITHUB_HOSTS").unwrap_or_default();
        let known = host == "github.com" || hosts.split(',').any(|known| known.trim() == host);
        let safe = |part: &str| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "-_.".contains(character))
        };
        (known && safe(owner) && safe(name)).then(|| Self {
            repository: format!("{owner}/{name}"),
            label: label.to_string(),
            api,
        })
    }

    /** A pull request as a source record
     * Input
        - number: &str - pull request number
        - deadline: Option<Instant> - deadline
     * Output
        - ConnectorOutcome
    */
    fn pull_request(&self, number: &str, deadline: Option<Instant>) -> ConnectorOutcome {
        if number.is_empty() || !number.chars().all(|character| character.is_ascii_digit()) {
            return ConnectorOutcome::failed(
                ConnectorStatus::InvalidQuery,
                format!("'{number}' is not a pull request number"),
            );
        }
        let path = format!("repos/{}/pulls/{number}", self.repository);
        let body = match self.api.get(&path, deadline) {
            ApiAnswer::Found(body) => body,
            ApiAnswer::Missing => return ConnectorOutcome::ok(Vec::new(), None),
            ApiAnswer::Failed(status, error) => return ConnectorOutcome::failed(status, error),
        };
        let text = |pointer: &str| {
            body.pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let head = text("/head/sha");
        let updated = text("/updated_at");
        let record = SourceRecord {
            record_id: number.to_string(),
            evidence_type: EvidenceType::PullRequest,
            media_type: "application/vnd.github.pull+json".into(),
            content: json!({
                "number": number,
                "title": text("/title"),
                "branch": text("/head/ref"),
                "head_sha": head,
                "state": text("/state"),
                "merged": body.get("merged").and_then(Value::as_bool),
                "author": text("/user/login"),
                "repository": self.label,
            }),
            source_timestamp: updated.as_deref().and_then(unix_millis),
            source_revision: updated.clone(),
            metadata: [
                Some(("pull_request".to_string(), number.to_string())),
                head.clone().map(|sha| ("commit".to_string(), sha)),
            ]
            .into_iter()
            .flatten()
            .collect(),
        };
        let snapshot = Some(format!(
            "github:pr:{number}@{}",
            updated.unwrap_or_else(|| "unknown".into())
        ));
        ConnectorOutcome::ok(vec![record], snapshot)
    }

    /** The files a commit changed, as a source record
     * Input
        - sha: &str - commit
        - deadline: Option<Instant> - deadline
     * Output
        - ConnectorOutcome
    */
    fn commit(&self, sha: &str, deadline: Option<Instant>) -> ConnectorOutcome {
        let path = format!("repos/{}/commits/{sha}", self.repository);
        let body = match self.api.get(&path, deadline) {
            ApiAnswer::Found(body) => body,
            ApiAnswer::Missing => return ConnectorOutcome::ok(Vec::new(), None),
            ApiAnswer::Failed(status, error) => return ConnectorOutcome::failed(status, error),
        };
        let full = body
            .get("sha")
            .and_then(Value::as_str)
            .unwrap_or(sha)
            .to_string();
        let files = body
            .get("files")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|file| {
                json!({
                    "path": file.get("filename"),
                    "additions": file.get("additions"),
                    "deletions": file.get("deletions"),
                })
            })
            .collect::<Vec<_>>();
        let committed = body
            .pointer("/commit/committer/date")
            .and_then(Value::as_str)
            .and_then(unix_millis);
        let record = SourceRecord {
            record_id: full.clone(),
            evidence_type: EvidenceType::CodeChange,
            media_type: "application/vnd.github.commit+json".into(),
            content: json!({
                "commit": full,
                "subject": body.pointer("/commit/message").and_then(Value::as_str).and_then(|message| message.lines().next()),
                "files": files,
                "repository": self.label,
            }),
            source_timestamp: committed,
            source_revision: Some(full.clone()),
            metadata: [("commit".to_string(), sha.to_string())]
                .into_iter()
                .collect(),
        };
        // A commit never changes: its snapshot is the commit itself
        ConnectorOutcome::ok(vec![record], Some(format!("github:{full}")))
    }

    /** The check runs of a commit, as CI run records (a run's result is its conclusion, or its
     * status while it has not concluded)
     * Input
        - sha: &str - commit
        - limit: usize - most runs
        - deadline: Option<Instant> - deadline
     * Output
        - ConnectorOutcome
    */
    fn check_runs(&self, sha: &str, limit: usize, deadline: Option<Instant>) -> ConnectorOutcome {
        let path = format!(
            "repos/{}/commits/{sha}/check-runs?per_page={}",
            self.repository,
            limit.clamp(1, 100)
        );
        let body = match self.api.get(&path, deadline) {
            ApiAnswer::Found(body) => body,
            ApiAnswer::Missing => return ConnectorOutcome::ok(Vec::new(), None),
            ApiAnswer::Failed(status, error) => return ConnectorOutcome::failed(status, error),
        };
        let runs = body
            .get("check_runs")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut latest = String::new();
        let records = runs
            .iter()
            .take(limit)
            .map(|run| {
                let text = |key: &str| run.get(key).and_then(Value::as_str).map(str::to_string);
                let id = run
                    .get("id")
                    .map(|id| id.to_string().trim_matches('"').to_string())
                    .unwrap_or_default();
                let finished = text("completed_at").or_else(|| text("started_at"));
                if let Some(at) = &finished {
                    if *at > latest {
                        latest.clone_from(at);
                    }
                }
                SourceRecord {
                    record_id: format!("check-{id}"),
                    evidence_type: EvidenceType::CiRun,
                    media_type: "application/vnd.github.check-run+json".into(),
                    content: json!({
                        "ci_run": format!("check-{id}"),
                        "name": text("name"),
                        "commit": text("head_sha").unwrap_or_else(|| sha.to_string()),
                        "status": text("conclusion").or_else(|| text("status")),
                        "repository": self.label,
                    }),
                    source_timestamp: finished.as_deref().and_then(unix_millis),
                    source_revision: text("status"),
                    metadata: [("commit".to_string(), sha.to_string())]
                        .into_iter()
                        .collect(),
                }
            })
            .collect::<Vec<_>>();
        let mut outcome =
            ConnectorOutcome::ok(records, Some(format!("github:checks:{sha}@{latest}")));
        let total = body.get("total_count").and_then(Value::as_u64).unwrap_or(0);
        if total as usize > outcome.records.len() {
            outcome.status = ConnectorStatus::Partial;
            outcome.error = Some(format!(
                "{total} check runs exist; {} were read",
                outcome.records.len()
            ));
        }
        outcome
    }

    /** Answer a query: a pull request, a commit's files, or a commit's check runs
     * Input
        - spec: &QuerySpec - query
        - deadline: Option<Instant> - deadline
     * Output
        - ConnectorOutcome
    */
    fn answer(&self, spec: &QuerySpec, deadline: Option<Instant>) -> ConnectorOutcome {
        let commit = spec.filters.get("commit");
        let pull_request = spec.filters.get("pull_request");
        match (spec.evidence_type, commit, pull_request) {
            (Some(EvidenceType::PullRequest), _, Some(number)) => {
                self.pull_request(number, deadline)
            }
            (Some(EvidenceType::CodeChange), Some(sha), _) if safe_sha(sha) => {
                self.commit(sha, deadline)
            }
            (Some(EvidenceType::CiRun), Some(sha), _) if safe_sha(sha) => {
                self.check_runs(sha, spec.limit, deadline)
            }
            _ => ConnectorOutcome::failed(
                ConnectorStatus::InvalidQuery,
                "GitHub answers pull requests by number, and a commit's files or check runs by commit",
            ),
        }
    }
}

/** Check that a commit is a plain hexadecimal sha, so it cannot change the API path
 * Input
    - sha: &str - commit
 * Output
    - bool
*/
fn safe_sha(sha: &str) -> bool {
    (4..=64).contains(&sha.len()) && sha.chars().all(|character| character.is_ascii_hexdigit())
}

impl ResourceConnector for GitHubConnector {
    /** Describe the source: no listing (GitHub is asked by number or commit)
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome {
        ConnectorOutcome::ok(Vec::new(), None)
    }

    /** Fetch a pull request by number
     * Input
        - record_id: &str - pull request number
     * Output
        - ConnectorOutcome
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome {
        self.pull_request(record_id, None)
    }

    /** Answer a query (see answer)
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome {
        self.answer(spec, None)
    }

    /** Fetch a pull request within the investigation's deadline
     * Input
        - record_id: &str - pull request number
        - deadline: Instant - deadline
     * Output
        - ConnectorOutcome
    */
    fn fetch_within(&self, record_id: &str, deadline: Instant) -> ConnectorOutcome {
        self.pull_request(record_id, Some(deadline))
    }

    /** Answer a query within the investigation's deadline
     * Input
        - spec: &QuerySpec - query
        - deadline: Instant - deadline
     * Output
        - ConnectorOutcome
    */
    fn query_within(&self, spec: &QuerySpec, deadline: Instant) -> ConnectorOutcome {
        self.answer(spec, Some(deadline))
    }

    /** Check that GitHub answers for the repository
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport {
        match self.api.get(&format!("repos/{}", self.repository), None) {
            ApiAnswer::Found(_) => HealthReport {
                available: true,
                detail: format!("GitHub answers for {}", self.repository),
            },
            ApiAnswer::Missing => HealthReport {
                available: false,
                detail: format!("GitHub does not know {}", self.repository),
            },
            ApiAnswer::Failed(status, error) => HealthReport {
                available: false,
                detail: format!("{status:?}: {error}"),
            },
        }
    }
}
