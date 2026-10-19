// Live connectors over this repository's own sources. They read through the existing APIs and
// never write: Git (commits and changed files, via platform::git), DataSea (the local event store
// of agent sessions and tool calls, telemetry::events), Landscape (zones and flows in
// .crane/governance.yaml, governance::zones), and Crane (AgentScript policies and the validation
// report, governance::policy and governance::verify). Hosted GitHub, CI, and deployment APIs have
// no live connector yet; the fixture connector stands in for them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{json, Value};

use super::connector::{
    page, snapshot_of, ConnectorOutcome, ConnectorStatus, HealthReport, QuerySpec,
    ResourceConnector, SourceRecord, MAX_PAGE,
};
use crate::capture::format::record::EvidenceType;
use crate::governance::policy::PolicySet;
use crate::governance::verify::{evaluate, Options, ScanMode};
use crate::governance::workspace::Workspace;
use crate::governance::zones;
use crate::platform::git::git_in;
use crate::telemetry::events::EventStore;
use crate::trust::crypto::sha512_hex;

/** Largest diff excerpt kept from a commit, in bytes */
const DIFF_EXCERPT_BYTES: usize = 8 * 1024;

/** Build metadata from key and value pairs, skipping empty values
 * Input
    - pairs: &[(&str, Option<String>)] - keys and values
 * Output
    - BTreeMap<String, String>
*/
fn metadata(pairs: &[(&str, Option<String>)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .filter_map(|(key, value)| {
            value
                .clone()
                .filter(|value| !value.is_empty())
                .map(|value| (key.to_string(), value))
        })
        .collect()
}

/** Check that a revision argument cannot be read by Git as an option or a range
 * Input
    - value: &str - commit or reference
 * Output
    - bool
*/
fn safe_revision(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && !value.starts_with('-')
        && !value.contains("..")
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "/_.-~^".contains(character))
}

/** Commits and changed files of a local Git repository
 * Fields
    - root: PathBuf - repository root
*/
#[derive(Debug, Clone)]
pub(crate) struct GitConnector {
    pub(crate) root: PathBuf,
}

impl GitConnector {
    /** Read one commit as a source record
     * Input
        - revision: &str - commit or reference
        - with_diff: bool - include a bounded diff excerpt
     * Output
        - Result<SourceRecord, String>
        - Error if Git cannot resolve the revision
    */
    fn commit(&self, revision: &str, with_diff: bool) -> Result<SourceRecord, String> {
        let header = git_in(
            &self.root,
            &[
                "show",
                "-s",
                "--format=%H%x1f%an%x1f%at%x1f%s",
                revision,
                "--",
            ],
        )?;
        let parts = header.split('\u{1f}').collect::<Vec<_>>();
        let [sha, author, time, subject] = parts.as_slice() else {
            return Err(format!("unexpected git output for {revision}"));
        };
        let numstat = git_in(&self.root, &["show", "--numstat", "--format=", sha, "--"])?;
        let files = numstat
            .lines()
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let (added, deleted, path) = (fields.next()?, fields.next()?, fields.next()?);
                Some(json!({"path": path, "additions": added.parse::<u64>().ok(), "deletions": deleted.parse::<u64>().ok()}))
            })
            .collect::<Vec<_>>();
        let mut content =
            json!({"commit": sha, "author": author, "subject": subject, "files": files});
        if with_diff {
            let diff = git_in(&self.root, &["show", "--format=", "--patch", sha, "--"])?;
            content["diff"] = Value::String(diff.chars().take(DIFF_EXCERPT_BYTES).collect());
            content["diff_truncated"] = Value::Bool(diff.len() > DIFF_EXCERPT_BYTES);
        }
        Ok(SourceRecord {
            record_id: (*sha).to_string(),
            evidence_type: EvidenceType::CodeChange,
            media_type: "application/vnd.git.commit+json".into(),
            content,
            source_timestamp: time.parse::<u64>().ok().map(|seconds| seconds * 1000),
            source_revision: Some((*sha).to_string()),
            metadata: metadata(&[("commit", Some((*sha).to_string()))]),
        })
    }

    /** The current HEAD commit, the snapshot every answer reflects
     * Input
        - None (uses self)
     * Output
        - Option<String>
    */
    fn head(&self) -> Option<String> {
        git_in(&self.root, &["rev-parse", "HEAD"])
            .ok()
            .map(|sha| format!("git:{sha}"))
    }
}

impl ResourceConnector for GitConnector {
    /** Describe the repository: its HEAD commit
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome {
        self.fetch("HEAD")
    }

    /** Read one commit with a diff excerpt
     * Input
        - record_id: &str - commit or reference
     * Output
        - ConnectorOutcome
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome {
        if !safe_revision(record_id) {
            return ConnectorOutcome::failed(
                ConnectorStatus::InvalidQuery,
                format!("unsafe revision '{record_id}'"),
            );
        }
        match self.commit(record_id, true) {
            Ok(record) => ConnectorOutcome::ok(vec![record], self.head()),
            Err(error)
                if error.contains("unknown revision")
                    || error.contains("bad object")
                    || error.contains("ambiguous") =>
            {
                ConnectorOutcome::ok(Vec::new(), self.head())
            }
            Err(error) => ConnectorOutcome::failed(ConnectorStatus::Failed, error),
        }
    }

    /** List commits: one commit (filter "commit") or the latest ones, optionally touching a path
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome {
        let start = spec.filters.get("commit").map_or("HEAD", String::as_str);
        if !safe_revision(start) || spec.cursor.is_some() {
            return ConnectorOutcome::failed(
                ConnectorStatus::InvalidQuery,
                "unsupported revision or cursor",
            );
        }
        let count = if spec.filters.contains_key("commit") {
            1
        } else {
            spec.limit.min(MAX_PAGE)
        };
        let limit = format!("-n{count}");
        let mut args = vec!["log", limit.as_str(), "--format=%H", start, "--"];
        if let Some(path) = spec.filters.get("path") {
            args.push(path);
        }
        let listing = match git_in(&self.root, &args) {
            Ok(listing) => listing,
            Err(error) => return ConnectorOutcome::failed(ConnectorStatus::Failed, error),
        };
        let mut records = Vec::new();
        for sha in listing.lines().filter(|line| !line.is_empty()) {
            match self.commit(sha, false) {
                Ok(record) => records.push(record),
                Err(error) => {
                    let mut outcome = ConnectorOutcome::ok(records, self.head());
                    outcome.status = ConnectorStatus::Partial;
                    outcome.error = Some(error);
                    return outcome;
                }
            }
        }
        let filtered = page(
            &records,
            &QuerySpec {
                filters: BTreeMap::new(),
                ..spec.clone()
            },
            self.head(),
        );
        ConnectorOutcome {
            next_cursor: None,
            ..filtered
        }
    }

    /** Check that Git can read the repository
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport {
        match self.head() {
            Some(head) => HealthReport {
                available: true,
                detail: head,
            },
            None => HealthReport {
                available: false,
                detail: "git cannot read HEAD".into(),
            },
        }
    }
}

/** Agent sessions, tool calls, and decisions from DataSea's local event store
 * Fields
    - store: EventStore - the repository's event store
*/
#[derive(Debug, Clone)]
pub(crate) struct DataSeaConnector {
    pub(crate) store: EventStore,
}

impl DataSeaConnector {
    /** Read every event as a source record (events are already redacted by DataSea)
     * Input
        - None (uses self)
     * Output
        - Result<(Vec<SourceRecord>, Option<String>), String> records and the chain head
        - Error if the event log is unreadable
    */
    fn records(&self) -> Result<(Vec<SourceRecord>, Option<String>), String> {
        let events = self.store.read_all()?;
        let head = events
            .last()
            .map(|event| format!("datasea:{}", &event.digest[..24.min(event.digest.len())]));
        let records = events
            .into_iter()
            .map(|event| {
                let evidence_type = if event.event_type.starts_with("tool_call") {
                    EvidenceType::ToolCall
                } else if event.event_type.starts_with("session") {
                    EvidenceType::AgentSession
                } else {
                    EvidenceType::AgentEvent
                };
                let mut content = serde_json::to_value(&event).unwrap_or(Value::Null);
                if let Some(object) = content.as_object_mut() {
                    object.remove("spec");
                    object.insert("session".into(), json!(event.scope.foxx_session_id));
                    object.insert("task".into(), json!(event.scope.foxx_task_id));
                }
                SourceRecord {
                    record_id: event.event_id.clone(),
                    evidence_type,
                    media_type: "application/vnd.datasea.event+json".into(),
                    content,
                    source_timestamp: Some(event.occurred_at),
                    source_revision: Some(event.digest.chars().take(24).collect()),
                    metadata: metadata(&[
                        ("session", event.scope.foxx_session_id.clone()),
                        ("task", event.scope.foxx_task_id.clone()),
                        ("event_type", Some(event.event_type.clone())),
                    ]),
                }
            })
            .collect();
        Ok((records, head))
    }
}

impl ResourceConnector for DataSeaConnector {
    /** Describe the event log (event identifiers and types)
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome {
        match self.records() {
            Ok((records, head)) => ConnectorOutcome::ok(
                records
                    .into_iter()
                    .take(MAX_PAGE)
                    .map(|record| SourceRecord {
                        content: Value::Null,
                        ..record
                    })
                    .collect(),
                head,
            ),
            Err(error) => ConnectorOutcome::failed(ConnectorStatus::Unavailable, error),
        }
    }

    /** Return one event by identifier
     * Input
        - record_id: &str - event identifier
     * Output
        - ConnectorOutcome
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome {
        match self.records() {
            Ok((records, head)) => ConnectorOutcome::ok(
                records
                    .into_iter()
                    .filter(|record| record.record_id == record_id)
                    .take(1)
                    .collect(),
                head,
            ),
            Err(error) => ConnectorOutcome::failed(ConnectorStatus::Unavailable, error),
        }
    }

    /** Query events by session, task, or event type
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome {
        match self.records() {
            Ok((records, head)) => page(&records, spec, head),
            Err(error) => ConnectorOutcome::failed(ConnectorStatus::Unavailable, error),
        }
    }

    /** Check that the event log is readable
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport {
        match self.store.read_all() {
            Ok(events) => HealthReport {
                available: true,
                detail: format!("{} events", events.len()),
            },
            Err(error) => HealthReport {
                available: false,
                detail: error,
            },
        }
    }
}

/** Landscape zones and flows of .crane/governance.yaml, with their versions
 * Fields
    - workspace: Workspace - repository
*/
#[derive(Debug, Clone)]
pub(crate) struct LandscapeConnector {
    pub(crate) workspace: Workspace,
}

impl LandscapeConnector {
    /** Read every zone and flow as a source record
     * Input
        - None (uses self)
     * Output
        - Result<(Vec<SourceRecord>, Option<String>), String> records and the file snapshot
        - Error if governance.yaml is invalid
    */
    fn records(&self) -> Result<(Vec<SourceRecord>, Option<String>), String> {
        let governance = zones::load(&self.workspace)?;
        let snapshot = std::fs::read(self.workspace.file(zones::FILE))
            .ok()
            .map(|bytes| format!("landscape:{}", &sha512_hex(&bytes)[..24]));
        let mut records = Vec::new();
        let entries = governance
            .zones
            .iter()
            .map(|zone| {
                (
                    "zone",
                    zone.id.clone(),
                    zone.version,
                    serde_json::to_value(zone),
                )
            })
            .chain(governance.flows.iter().map(|flow| {
                (
                    "flow",
                    flow.id.clone(),
                    flow.version,
                    serde_json::to_value(flow),
                )
            }));
        for (kind, id, version, value) in entries {
            let mut content = value.map_err(|error| error.to_string())?;
            content["kind"] = json!(kind);
            content["version"] = json!(version);
            records.push(SourceRecord {
                record_id: format!("{kind}:{id}"),
                evidence_type: EvidenceType::GraphState,
                media_type: "application/vnd.landscape.entity+json".into(),
                content,
                source_timestamp: None,
                source_revision: Some(format!("v{version}")),
                metadata: metadata(&[("kind", Some(kind.into())), ("id", Some(id))]),
            });
        }
        Ok((records, snapshot))
    }
}

impl ResourceConnector for LandscapeConnector {
    /** Describe the zones and flows
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome {
        self.query(&QuerySpec {
            evidence_type: None,
            filters: BTreeMap::new(),
            since: None,
            until: None,
            limit: MAX_PAGE,
            cursor: None,
            scope: Default::default(),
        })
    }

    /** Return one zone or flow ("zone:ID" or "flow:ID")
     * Input
        - record_id: &str - identifier
     * Output
        - ConnectorOutcome
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome {
        match self.records() {
            Ok((records, snapshot)) => ConnectorOutcome::ok(
                records
                    .into_iter()
                    .filter(|record| record.record_id == record_id)
                    .collect(),
                snapshot,
            ),
            Err(error) => ConnectorOutcome::failed(ConnectorStatus::Failed, error),
        }
    }

    /** Query zones and flows by kind or id
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome {
        match self.records() {
            Ok((records, snapshot)) => page(&records, spec, snapshot),
            Err(error) => ConnectorOutcome::failed(ConnectorStatus::Failed, error),
        }
    }

    /** Check that governance.yaml is readable
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport {
        match self.records() {
            Ok((records, _)) => HealthReport {
                available: true,
                detail: format!("{} zones and flows", records.len()),
            },
            Err(error) => HealthReport {
                available: false,
                detail: error,
            },
        }
    }
}

/** Crane/AgentScript policies and the current validation report (read only; Work Capture never
 * changes a policy)
 * Fields
    - workspace: Workspace - repository
*/
#[derive(Debug, Clone)]
pub(crate) struct CraneConnector {
    pub(crate) workspace: Workspace,
}

impl CraneConnector {
    /** Read every policy as a source record
     * Input
        - None (uses self)
     * Output
        - Result<Vec<SourceRecord>, String>
        - Error if the policy directory is unreadable
    */
    fn policies(&self) -> Result<Vec<SourceRecord>, String> {
        let set = PolicySet::load(&self.workspace)?;
        Ok(set
            .blocks()
            .into_iter()
            .map(|(file, block)| {
                let version = sha512_hex(file.text.as_bytes())[..16].to_string();
                SourceRecord {
                    record_id: format!("policy:{}", block.name),
                    evidence_type: EvidenceType::Policy,
                    media_type: "application/vnd.agentscript.policy+json".into(),
                    content: json!({
                        "name": block.name,
                        "file": file.path,
                        "statements": block.statements.iter().map(|statement| statement.render()).collect::<Vec<_>>(),
                    }),
                    source_timestamp: None,
                    source_revision: Some(version),
                    metadata: metadata(&[("kind", Some("policy".into())), ("name", Some(block.name.clone()))]),
                }
            })
            .collect())
    }

    /** Run Crane's verification and summarize it as a source record
     * Input
        - None (uses self)
     * Output
        - Result<SourceRecord, String>
        - Error if verification cannot run
    */
    fn validation(&self) -> Result<SourceRecord, String> {
        let options = Options {
            scan: ScanMode::Full,
            targets_pending: false,
            candidates: false,
            governance: false,
        };
        let (report, _) = evaluate(&self.workspace, &options)?;
        Ok(SourceRecord {
            record_id: "validation".into(),
            evidence_type: EvidenceType::Validation,
            media_type: "application/vnd.crane.validation+json".into(),
            content: json!({
                "passed": report.passed,
                "generation": report.generation,
                "failures": report.failures().len(),
                "findings": report.findings.iter().map(|finding| finding.code.clone()).collect::<Vec<_>>(),
            }),
            source_timestamp: None,
            source_revision: Some(format!("generation:{}", report.generation)),
            metadata: metadata(&[
                ("kind", Some("validation".into())),
                ("name", Some("validation".into())),
            ]),
        })
    }

    /** Every record with the snapshot they reflect
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome, Partial when the validation report could not be produced
    */
    fn all(&self) -> ConnectorOutcome {
        let mut records = match self.policies() {
            Ok(records) => records,
            Err(error) => return ConnectorOutcome::failed(ConnectorStatus::Failed, error),
        };
        let validation = self.validation();
        if let Ok(record) = &validation {
            records.push(record.clone());
        }
        let snapshot = Some(format!("crane:{}", snapshot_of(&records)));
        let mut outcome = ConnectorOutcome::ok(records, snapshot);
        if let Err(error) = validation {
            outcome.status = ConnectorStatus::Partial;
            outcome.error = Some(format!("validation unavailable: {error}"));
        }
        outcome
    }
}

impl ResourceConnector for CraneConnector {
    /** Describe policies and the validation report
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome {
        self.all()
    }

    /** Return one policy ("policy:NAME") or the validation report ("validation")
     * Input
        - record_id: &str - identifier
     * Output
        - ConnectorOutcome
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome {
        let mut outcome = self.all();
        outcome
            .records
            .retain(|record| record.record_id == record_id);
        outcome
    }

    /** Query policies and validation by kind or name
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome {
        let all = self.all();
        if !all.status.usable() {
            return all;
        }
        let mut answer = page(&all.records, spec, all.snapshot);
        if all.status == ConnectorStatus::Partial {
            answer.status = ConnectorStatus::Partial;
            answer.error = all.error;
        }
        answer
    }

    /** Check that the policy directory is readable
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport {
        match self.policies() {
            Ok(records) => HealthReport {
                available: true,
                detail: format!("{} policies", records.len()),
            },
            Err(error) => HealthReport {
                available: false,
                detail: error,
            },
        }
    }
}
