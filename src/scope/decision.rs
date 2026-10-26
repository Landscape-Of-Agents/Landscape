// Recorded scope decisions. Every 'crane scope resolve' answer is appended to
// runtime/scope/decisions.jsonl with the request, the resolved resource and its content digest,
// the rule versions, the decision, its rule codes and reasons, and the gaps. Records are
// hash-chained, so an edited or removed record is detected when the log is read.
//
// A recorded decision stays valid only while everything it was made from is unchanged: the signed
// rules, governance.yaml, the integrity of the state, and the content of the resource. validity()
// re-establishes that against the present and names what changed; the hooks never reuse a recorded
// decision, they decide every tool call again.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::resource::{content_digest, Location, Resource};
use super::{rule, Boundary, Gap, Outcome, Request, Versions};
use crate::platform::files::{append_line, read_lines, Lock};
use crate::telemetry::events::{Event, EventStore};
use crate::telemetry::model::Decision;
use crate::trust::crypto::digest_json;

/** One recorded scope decision
 * Fields
    - decision_id: String - identifier ("scd_...")
    - recorded_at: u64 - Unix milliseconds
    - actor: String - who asked
    - request: Request - the question
    - resource: Resource - the resolved target
    - versions: Versions - rule versions it was decided under
    - integrity_ok: bool - the signed state verified when it was decided
    - decision: Decision - the guard's decision
    - rules: Vec<String> - rule codes, in order
    - reasons: Vec<String> - explanations
    - zones: Vec<String> - zones and flows covering the target
    - selections: Vec<String> - selections it touches
    - policies: Vec<String> - their policies
    - gaps: Vec<Gap> - what Crane cannot establish or enforce for it
    - previous_digest: String - digest of the previous record ("" for the first)
    - digest: String - digest of this record without this field
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScopeDecision {
    pub(crate) decision_id: String,
    pub(crate) recorded_at: u64,
    pub(crate) actor: String,
    pub(crate) request: Request,
    pub(crate) resource: Resource,
    pub(crate) versions: Versions,
    pub(crate) integrity_ok: bool,
    pub(crate) decision: Decision,
    pub(crate) rules: Vec<String>,
    pub(crate) reasons: Vec<String>,
    pub(crate) zones: Vec<String>,
    pub(crate) selections: Vec<String>,
    pub(crate) policies: Vec<String>,
    pub(crate) gaps: Vec<Gap>,
    pub(crate) previous_digest: String,
    pub(crate) digest: String,
}

impl ScopeDecision {
    /** Build an unrecorded decision from an outcome
     * Input
        - boundary: &Boundary - the boundary it was decided against
        - request: &Request - question
        - outcome: &Outcome - answer
        - actor: &str - who asked
        - at: u64 - Unix milliseconds
     * Output
        - ScopeDecision (identifier and digests are set when recorded)
    */
    pub(crate) fn new(
        boundary: &Boundary,
        request: &Request,
        outcome: &Outcome,
        actor: &str,
        at: u64,
    ) -> Self {
        Self {
            decision_id: String::new(),
            recorded_at: at,
            actor: actor.into(),
            request: request.clone(),
            resource: outcome.resource.clone(),
            versions: boundary.versions(),
            integrity_ok: boundary.integrity_ok(),
            decision: outcome.decided.verdict.decision,
            rules: outcome
                .decided
                .rules
                .iter()
                .map(|code| (*code).to_string())
                .collect(),
            reasons: outcome.decided.verdict.reasons.clone(),
            zones: outcome.decided.zones.clone(),
            selections: outcome.decided.selections.clone(),
            policies: outcome.decided.policies.clone(),
            gaps: outcome.gaps.clone(),
            previous_digest: String::new(),
            digest: String::new(),
        }
    }

    /** The digest of the record without its digest field
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn compute_digest(&self) -> String {
        let mut content = self.clone();
        content.digest = String::new();
        digest_json("scope/decision/v1", &json!(content))
    }
}

/** The append-only log of scope decisions
 * Fields
    - path: PathBuf - runtime/scope/decisions.jsonl
*/
pub(crate) struct DecisionLog {
    path: PathBuf,
}

impl DecisionLog {
    /** Open the log of a runtime directory
     * Input
        - runtime: &Path - the repository's runtime directory
     * Output
        - DecisionLog
    */
    pub(crate) fn open(runtime: &Path) -> Self {
        Self {
            path: runtime.join("scope").join("decisions.jsonl"),
        }
    }

    /** Read every record, verifying the chain
     * Input
        - None (uses self)
     * Output
        - Result<Vec<ScopeDecision>, String>
        - Error if a record is malformed, altered, or out of chain
    */
    pub(crate) fn read_all(&self) -> Result<Vec<ScopeDecision>, String> {
        let mut previous = String::new();
        let mut records = Vec::new();
        for (index, value) in read_lines(&self.path)?.into_iter().enumerate() {
            let record: ScopeDecision = serde_json::from_value(value)
                .map_err(|error| format!("scope decision {} is malformed: {error}", index + 1))?;
            if record.previous_digest != previous || record.compute_digest() != record.digest {
                return Err(format!(
                    "the scope decision log was altered at record {} ({}); recorded decisions cannot be trusted",
                    index + 1,
                    record.decision_id
                ));
            }
            previous = record.digest.clone();
            records.push(record);
        }
        Ok(records)
    }

    /** Append a decision, assigning its identifier and chaining it to the last record
     * Input
        - decision: ScopeDecision - unrecorded decision
     * Output
        - Result<ScopeDecision, String> - the recorded decision
    */
    pub(crate) fn append(&self, mut decision: ScopeDecision) -> Result<ScopeDecision, String> {
        let _lock = Lock::acquire(&self.path.with_extension("lock"))?;
        decision.previous_digest = self
            .read_all()?
            .last()
            .map(|last| last.digest.clone())
            .unwrap_or_default();
        decision.decision_id = format!(
            "scd_{}",
            &digest_json(
                "scope/decision/id/v1",
                &json!([
                    decision.recorded_at,
                    decision.request,
                    decision.resource,
                    decision.previous_digest
                ])
            )[..20]
        );
        decision.digest = decision.compute_digest();
        append_line(
            &self.path,
            &serde_json::to_string(&decision).map_err(|error| error.to_string())?,
        )?;
        Ok(decision)
    }

    /** Find a recorded decision
     * Input
        - decision_id: &str - identifier
     * Output
        - Result<Option<ScopeDecision>, String>
    */
    pub(crate) fn find(&self, decision_id: &str) -> Result<Option<ScopeDecision>, String> {
        Ok(self
            .read_all()?
            .into_iter()
            .find(|record| record.decision_id == decision_id))
    }
}

/** Check whether a recorded decision still describes the present
 * Input
    - decision: &ScopeDecision - recorded decision
    - boundary: &Boundary - the boundary now
 * Output
    - Vec<String> what changed since; empty when the decision still applies
*/
pub(crate) fn validity(decision: &ScopeDecision, boundary: &Boundary) -> Vec<String> {
    let mut changes = decision.versions.changes(&boundary.versions());
    if decision.integrity_ok != boundary.integrity_ok() {
        changes.push(format!(
            "the signed state {} verification",
            if boundary.integrity_ok() {
                "now passes"
            } else {
                "now fails"
            }
        ));
    }
    if matches!(
        decision.resource.location,
        Location::Repository | Location::OutsideRepository | Location::OtherWorktree
    ) {
        let now = decision
            .resource
            .absolute
            .as_deref()
            .map(Path::new)
            .filter(|path| path.is_file())
            .and_then(content_digest);
        if now != decision.resource.content_digest {
            changes.push(match (&decision.resource.content_digest, &now) {
                (None, Some(_)) => "the file was created".into(),
                (Some(_), None) => "the file was removed".into(),
                _ => "the file's content changed".into(),
            });
        }
    }
    changes
}

/** A decision explained: the recorded scope decision or a hook decision event, described the
 * same way
 * Fields
    - source: String - "scope decision" or "hook decision"
    - id: String - decision id or tool call id
    - decision: Decision - the decision
    - rules: Vec<Value> - each rule code with its domain and meaning
    - reasons: Vec<String> - recorded explanations
    - zones, selections, policies: Vec<String> - governance involved
    - resources: Vec<Value> - resources named
    - registry_generation: u64 - rules generation it was decided under
    - changed_since: Vec<String> - what changed since (empty: it still applies)
    - gaps: Vec<Gap> - what Crane could not establish (scope decisions)
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Explanation {
    pub(crate) source: String,
    pub(crate) id: String,
    pub(crate) decision: Decision,
    pub(crate) rules: Vec<Value>,
    pub(crate) reasons: Vec<String>,
    pub(crate) zones: Vec<String>,
    pub(crate) selections: Vec<String>,
    pub(crate) policies: Vec<String>,
    pub(crate) resources: Vec<Value>,
    pub(crate) registry_generation: u64,
    pub(crate) changed_since: Vec<String>,
    pub(crate) gaps: Vec<Gap>,
}

/** Describe rule codes with their domain and meaning
 * Input
    - codes: &[String] - rule codes
 * Output
    - Vec<Value>
*/
fn described(codes: &[String]) -> Vec<Value> {
    codes
        .iter()
        .map(|code| {
            let (domain, meaning) = rule(code);
            json!({"code": code, "domain": domain, "meaning": meaning})
        })
        .collect()
}

/** Explain a recorded scope decision, or a hook decision by its tool call id
 * Input
    - boundary: &Boundary - the boundary now
    - id: &str - "scd_..." decision id, or a tool call id from the hook events
 * Output
    - Result<Explanation, String>
    - Error if nothing with that id was recorded, or the record cannot be trusted
*/
pub(crate) fn explain(boundary: &Boundary, id: &str) -> Result<Explanation, String> {
    if id.starts_with("scd_") {
        let decision = DecisionLog::open(&boundary.runtime)
            .find(id)?
            .ok_or_else(|| format!("no scope decision {id} was recorded on this machine"))?;
        return Ok(Explanation {
            source: "scope decision".into(),
            id: id.into(),
            decision: decision.decision,
            rules: described(&decision.rules),
            reasons: decision.reasons.clone(),
            zones: decision.zones.clone(),
            selections: decision.selections.clone(),
            policies: decision.policies.clone(),
            resources: vec![json!(decision.resource)],
            registry_generation: decision.versions.registry_generation,
            changed_since: validity(&decision, boundary),
            gaps: decision.gaps.clone(),
        });
    }
    let events = EventStore::open(&boundary.runtime).read_all()?;
    let event: &Event = events
        .iter()
        .rev()
        .find(|event| {
            event.event_type == "tool_call.decided"
                && event.scope.tool_call_id.as_deref() == Some(id)
        })
        .ok_or_else(|| {
            format!("no scope decision or hook decision with id {id} was recorded on this machine")
        })?;
    let rules = event.payload["rules"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|code| code.as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let current = boundary.state.generation();
    let mut changed_since = Vec::new();
    if event.bindings.registry_generation != current {
        changed_since.push(format!(
            "the signed rules changed (generation {} -> {current})",
            event.bindings.registry_generation
        ));
    }
    Ok(Explanation {
        source: "hook decision".into(),
        id: id.into(),
        decision: event.decision.unwrap_or(Decision::Error),
        rules: described(&rules),
        reasons: event.reasons.clone(),
        zones: event.bindings.zones.clone(),
        selections: event.bindings.selections.clone(),
        policies: event.bindings.policies.clone(),
        resources: event
            .resources
            .iter()
            .map(|resource| json!(resource))
            .collect(),
        registry_generation: event.bindings.registry_generation,
        changed_since,
        gaps: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{DecisionLog, ScopeDecision};
    use crate::scope::resource::{Location, Resource, Tracking};
    use crate::scope::{Access, Request, Versions};
    use crate::telemetry::model::Decision;

    /** A record for the tests
     * Input
        - at: u64 - time
     * Output
        - ScopeDecision
    */
    fn record(at: u64) -> ScopeDecision {
        ScopeDecision {
            decision_id: String::new(),
            recorded_at: at,
            actor: "dev".into(),
            request: Request {
                access: Access::Write,
                target: "app/pay.py".into(),
                lines: None,
                mode: "delegated".into(),
            },
            resource: Resource {
                requested: "app/pay.py".into(),
                location: Location::Repository,
                path: Some("app/pay.py".into()),
                absolute: None,
                via_symlink: false,
                tracking: Tracking::Tracked,
                content_digest: Some("abc".into()),
            },
            versions: Versions::default(),
            integrity_ok: true,
            decision: Decision::Deny,
            rules: vec!["PRESERVED_SELECTION".into()],
            reasons: vec!["preserved".into()],
            zones: Vec::new(),
            selections: Vec::new(),
            policies: Vec::new(),
            gaps: Vec::new(),
            previous_digest: String::new(),
            digest: String::new(),
        }
    }

    /** Records chain, are found by id, and an altered record is detected
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn decisions_chain_and_tampering_is_detected() {
        let directory = tempfile::tempdir().unwrap();
        let log = DecisionLog::open(directory.path());
        let first = log.append(record(1)).unwrap();
        let second = log.append(record(2)).unwrap();
        assert!(first.decision_id.starts_with("scd_"));
        assert_eq!(second.previous_digest, first.digest);
        assert_eq!(log.find(&second.decision_id).unwrap(), Some(second));
        let path = directory.path().join("scope").join("decisions.jsonl");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replacen("\"DENY\"", "\"ALLOW\"", 1)).unwrap();
        assert!(log.read_all().unwrap_err().contains("altered"));
    }
}
