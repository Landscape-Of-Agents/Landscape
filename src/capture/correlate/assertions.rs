// Assertions: propositions about entities, each with the evidence that supports or contradicts
// it, the derivation that produced it (direct observation, deterministic rule, agent hypothesis,
// or human review), and lineage to its inputs. Rules here are deterministic and versioned. Copies
// of one artifact count once (independence is by content hash). Disagreeing evidence is kept on
// both sides and the assertion stays unresolved: no winner is picked. Nothing is inferred from
// timing, and an agent hypothesis stays "proposed" whatever it cites.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::model::{entity_id, EntityKind, NodeRef};
use crate::capture::format::record::{EvidenceRecord, EvidenceType, RecordScope};
use crate::capture::{stable_id, Actor, Truth, SCHEMA_VERSION};

/** Version of the deterministic derivation rules */
pub(crate) const RULES_VERSION: &str = "1";

/** Lifecycle of an assertion
 * Variants
    - Proposed - a hypothesis, not evaluated against evidence
    - Supported - the evidence agrees with the claim
    - Contradicted - the evidence disagrees with the claim (reserved: no rule produces it; disagreeing
      evidence makes an assertion Unresolved with both sides kept)
    - Unresolved - no evidence, or evidence on both sides
    - Superseded - replaced by a newer assertion (reserved: a successor is a new assertion with its
      own id; marking the older one would change the stored copy that capability intake compares
      with the copy inside an Evidence Pack, so every earlier pack would read as changed)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AssertionStatus {
    Proposed,
    Supported,
    Contradicted,
    Unresolved,
    Superseded,
}

/** How an assertion was produced
 * Variants
    - DirectObservation - a source states it
    - Deterministic - a versioned rule derived it
    - AgentHypothesis - an agent proposed it (unverified)
    - Reviewed - an authorized human reviewed it
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DerivationMethod {
    DirectObservation,
    Deterministic,
    AgentHypothesis,
    Reviewed,
}

/** The derivation of an assertion
 * Fields
    - method: DerivationMethod - how
    - rule: String - rule or producer name
    - version: String - rule version
    - inputs: Vec<NodeRef> - every input considered (evidence and assertions)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Derivation {
    pub(crate) method: DerivationMethod,
    pub(crate) rule: String,
    pub(crate) version: String,
    pub(crate) inputs: Vec<NodeRef>,
}

/** A structured proposition: subject, predicate, object
 * Fields
    - subject: String - entity id
    - subject_kind: EntityKind - its kind
    - subject_key: String - its natural key
    - predicate: String - property such as "ci_passed"
    - object: Value - value, a Truth for boolean propositions
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Claim {
    pub(crate) subject: String,
    pub(crate) subject_kind: EntityKind,
    pub(crate) subject_key: String,
    pub(crate) predicate: String,
    pub(crate) object: Value,
}

/** Optional confidence metadata; it is not a calibrated probability unless its interpretation
 * says so, and it never authorizes anything
 * Fields
    - value: f64 - score
    - producer: String - who produced it
    - method_version: String - model or method version
    - inputs: Vec<NodeRef> - inputs it was computed from
    - interpretation: String - how to read the score
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Confidence {
    pub(crate) value: f64,
    pub(crate) producer: String,
    pub(crate) method_version: String,
    pub(crate) inputs: Vec<NodeRef>,
    pub(crate) interpretation: String,
}

/** An assertion
 * Fields
    - assertion_id: String - stable identifier
    - schema_version: u32 - capture::SCHEMA_VERSION
    - claim: Claim - the proposition
    - statement: String - the proposition in words
    - supporting: Vec<String> - evidence supporting the claim
    - contradicting: Vec<String> - evidence contradicting it
    - independent_support: usize - distinct artifacts among the supporting evidence
    - independent_contradiction: usize - distinct artifacts among the contradicting evidence
    - derived_from: Vec<String> - assertions it was derived from
    - derivation: Derivation - how it was produced
    - status: AssertionStatus - lifecycle
    - scope: RecordScope - scope
    - valid_from: Option<u64> - start of validity, None when unknown
    - valid_to: Option<u64> - end of validity, None when open
    - confidence: Option<Confidence> - optional confidence metadata
    - created_at: u64 - Unix milliseconds
    - created_by: Actor - producer
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Assertion {
    pub(crate) assertion_id: String,
    pub(crate) schema_version: u32,
    pub(crate) claim: Claim,
    pub(crate) statement: String,
    pub(crate) supporting: Vec<String>,
    pub(crate) contradicting: Vec<String>,
    pub(crate) independent_support: usize,
    pub(crate) independent_contradiction: usize,
    pub(crate) derived_from: Vec<String>,
    pub(crate) derivation: Derivation,
    pub(crate) status: AssertionStatus,
    pub(crate) scope: RecordScope,
    pub(crate) valid_from: Option<u64>,
    pub(crate) valid_to: Option<u64>,
    pub(crate) confidence: Option<Confidence>,
    pub(crate) created_at: u64,
    pub(crate) created_by: Actor,
}

impl Assertion {
    /** The claim's object as a truth value
     * Input
        - None (uses self)
     * Output
        - Truth, Unknown when the object is not a truth value
    */
    pub(crate) fn truth(&self) -> Truth {
        serde_json::from_value(self.claim.object.clone()).unwrap_or(Truth::Unknown)
    }

    /** Report whether evidence sits on both sides of the claim
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn conflicted(&self) -> bool {
        !self.supporting.is_empty() && !self.contradicting.is_empty()
    }
}

/** One piece of evidence speaking to a proposition
 * Fields
    - evidence_id: String - evidence
    - value: Truth - what it says (Unknown when it says nothing usable)
    - artifact_hash: Option<String> - content hash of its artifact, for independence
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) evidence_id: String,
    pub(crate) value: Truth,
    pub(crate) artifact_hash: Option<String>,
}

/** Count distinct artifacts among observations
 * Input
    - observations: &[&Observation] - observations
 * Output
    - usize
*/
fn independent(observations: &[&Observation]) -> usize {
    observations
        .iter()
        .map(|observation| {
            observation
                .artifact_hash
                .clone()
                .unwrap_or_else(|| observation.evidence_id.clone())
        })
        .collect::<BTreeSet<_>>()
        .len()
}

/** Build a deterministic assertion about a boolean proposition from observations. The claim is
 * stated positively ("yes"): evidence saying yes supports it and evidence saying no contradicts
 * it. When only "no" evidence exists the claim is restated as "no" and supported. Evidence on both
 * sides leaves it unresolved; no evidence leaves it unresolved with object "unknown"
 * Input
    - subject: (EntityKind, &str) - kind and natural key of the subject
    - predicate: &str - property
    - observations: &[Observation] - evidence considered (Unknown values are inputs only)
    - scope: &RecordScope - scope
    - created_at: u64 - Unix milliseconds
 * Output
    - Assertion
*/
pub(crate) fn from_observations(
    subject: (EntityKind, &str),
    predicate: &str,
    observations: &[Observation],
    scope: &RecordScope,
    created_at: u64,
) -> Assertion {
    let yes = observations
        .iter()
        .filter(|observation| observation.value == Truth::Yes)
        .collect::<Vec<_>>();
    let no = observations
        .iter()
        .filter(|observation| observation.value == Truth::No)
        .collect::<Vec<_>>();
    let ids = |list: &[&Observation]| {
        list.iter()
            .map(|observation| observation.evidence_id.clone())
            .collect::<Vec<_>>()
    };
    let (object, supporting, contradicting, status) = match (yes.is_empty(), no.is_empty()) {
        (false, true) => (
            Truth::Yes,
            yes.clone(),
            Vec::new(),
            AssertionStatus::Supported,
        ),
        (true, false) => (
            Truth::No,
            no.clone(),
            Vec::new(),
            AssertionStatus::Supported,
        ),
        (false, false) => (
            Truth::Yes,
            yes.clone(),
            no.clone(),
            AssertionStatus::Unresolved,
        ),
        (true, true) => (
            Truth::Unknown,
            Vec::new(),
            Vec::new(),
            AssertionStatus::Unresolved,
        ),
    };
    let inputs = observations
        .iter()
        .map(|observation| NodeRef::Evidence(observation.evidence_id.clone()))
        .collect::<Vec<_>>();
    let (kind, key) = subject;
    let input_ids = inputs.iter().map(NodeRef::id).collect::<Vec<_>>();
    let mut parts = vec![predicate, key, RULES_VERSION];
    parts.extend(input_ids);
    Assertion {
        assertion_id: stable_id("asr", &parts),
        schema_version: SCHEMA_VERSION,
        claim: Claim {
            subject: entity_id(kind, key),
            subject_kind: kind,
            subject_key: key.into(),
            predicate: predicate.into(),
            object: json!(object),
        },
        statement: format!(
            "{predicate} for {kind:?} {key}: {}",
            json!(object).as_str().unwrap_or("unknown")
        ),
        independent_support: independent(&supporting),
        independent_contradiction: independent(&contradicting),
        supporting: ids(&supporting),
        contradicting: ids(&contradicting),
        derived_from: Vec::new(),
        derivation: Derivation {
            method: DerivationMethod::Deterministic,
            rule: predicate.into(),
            version: RULES_VERSION.into(),
            inputs,
        },
        status,
        scope: scope.clone(),
        valid_from: None,
        valid_to: None,
        confidence: None,
        created_at,
        created_by: Actor::system(),
    }
}

/** Read a status-like field as a truth value
 * Input
    - value: Option<String> - field value
    - yes: &[&str] - values meaning yes
    - no: &[&str] - values meaning no
 * Output
    - Truth, Unknown for missing or unrecognized values
*/
fn status_truth(value: Option<String>, yes: &[&str], no: &[&str]) -> Truth {
    match value.map(|value| value.to_ascii_lowercase()) {
        Some(value) if yes.contains(&value.as_str()) => Truth::Yes,
        Some(value) if no.contains(&value.as_str()) => Truth::No,
        _ => Truth::Unknown,
    }
}

/** What a test report says: failures mean no, zero failures with passing tests mean yes, and
 * missing or malformed counts mean unknown
 * Input
    - record: &EvidenceRecord - test report
 * Output
    - Truth
*/
fn tests_truth(record: &EvidenceRecord) -> Truth {
    let count = |name: &str| {
        record
            .field(name)
            .and_then(|value| value.parse::<u64>().ok())
    };
    match (count("passed"), count("failed")) {
        (_, Some(failed)) if failed > 0 => Truth::No,
        (Some(passed), Some(0)) if passed > 0 => Truth::Yes,
        _ => Truth::Unknown,
    }
}

/** The subjects of an investigation the rules assess
 * Fields
    - task: Option<String> - task key
    - commits: Vec<String> - commits
    - deployments: Vec<String> - deployments
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct Subjects {
    pub(crate) task: Option<String>,
    pub(crate) commits: Vec<String>,
    pub(crate) deployments: Vec<String>,
}

/** Apply the deterministic rules: ci_passed and tests_passed per commit, deployment_succeeded per
 * deployment, and change_verified for the task, derived from those assertions (lineage kept in
 * derived_from). A subject without evidence gets an unresolved assertion, never a false one
 * Input
    - records: &[&EvidenceRecord] - evidence of the investigation
    - hashes: &BTreeMap<String, String> - artifact id to content hash
    - subjects: &Subjects - what to assess
    - scope: &RecordScope - scope
    - created_at: u64 - Unix milliseconds
 * Output
    - Vec<Assertion>
*/
pub(crate) fn derive(
    records: &[&EvidenceRecord],
    hashes: &BTreeMap<String, String>,
    subjects: &Subjects,
    scope: &RecordScope,
    created_at: u64,
) -> Vec<Assertion> {
    let observe = |record: &EvidenceRecord, value: Truth| Observation {
        evidence_id: record.evidence_id.clone(),
        value,
        artifact_hash: hashes.get(&record.source_ref.artifact_id).cloned(),
    };
    let mut derived = Vec::new();
    for commit in &subjects.commits {
        let about = |kind: EvidenceType| {
            records.iter().filter(move |record| {
                record.evidence_type == kind
                    && record.refs.commit.as_deref() == Some(commit.as_str())
            })
        };
        let ci = about(EvidenceType::CiRun)
            .map(|record| {
                observe(
                    record,
                    status_truth(
                        record.field("status"),
                        &["success", "passed"],
                        &["failure", "failed"],
                    ),
                )
            })
            .collect::<Vec<_>>();
        derived.push(from_observations(
            (EntityKind::Commit, commit),
            "ci_passed",
            &ci,
            scope,
            created_at,
        ));
        let tests = about(EvidenceType::TestReport)
            .map(|record| observe(record, tests_truth(record)))
            .collect::<Vec<_>>();
        derived.push(from_observations(
            (EntityKind::Commit, commit),
            "tests_passed",
            &tests,
            scope,
            created_at,
        ));
    }
    for deployment in &subjects.deployments {
        let observations = records
            .iter()
            .filter(|record| {
                record.refs.deployment.as_deref() == Some(deployment.as_str())
                    || (record.evidence_type == EvidenceType::Deployment
                        && record.source_ref.record_id == *deployment)
            })
            .filter_map(|record| match record.evidence_type {
                EvidenceType::Deployment => Some(observe(
                    record,
                    status_truth(
                        record.field("status"),
                        &["succeeded", "success"],
                        &["failed", "failure", "rolled_back"],
                    ),
                )),
                EvidenceType::Verification => Some(observe(
                    record,
                    status_truth(record.field("healthy"), &["true"], &["false"]),
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        derived.push(from_observations(
            (EntityKind::Deployment, deployment),
            "deployment_succeeded",
            &observations,
            scope,
            created_at,
        ));
    }
    if let Some(task) = &subjects.task {
        derived.push(change_verified(task, &derived, scope, created_at));
    }
    // Derived after change_verified on purpose: whether the policies hold is reported beside the
    // verification of the change, never folded into it, so an unavailable or failing validation
    // report does not change what CI, tests, and the deployment established
    if let Some(repository) = &scope.repository {
        let validations = records
            .iter()
            .filter(|record| record.evidence_type == EvidenceType::Validation)
            .map(|record| {
                observe(
                    record,
                    status_truth(record.field("passed"), &["true"], &["false"]),
                )
            })
            .collect::<Vec<_>>();
        if !validations.is_empty() {
            derived.push(from_observations(
                (EntityKind::Repository, repository),
                "policies_hold",
                &validations,
                scope,
                created_at,
            ));
        }
    }
    derived
}

/** The evidence types the deterministic rules read; every other type an investigation requires is
 * context only (collected and shown, but no rule concludes from it) */
#[cfg_attr(not(test), allow(dead_code))] // checked against the built-in recipes by the acceptance tests
pub(crate) const RULE_INPUTS: &[EvidenceType] = &[
    EvidenceType::CiRun,
    EvidenceType::TestReport,
    EvidenceType::Deployment,
    EvidenceType::Verification,
    EvidenceType::Validation,
    EvidenceType::CodeChange,
    EvidenceType::GraphState,
];

/** Evidence types required by built-in recipes that no rule concludes from, by decision: context
 * for people and later components (what the task asked, which change it was, what the logs and
 * metrics showed, what the agent did) */
#[cfg_attr(not(test), allow(dead_code))] // checked against the built-in recipes by the acceptance tests
pub(crate) const CONTEXT_ONLY: &[EvidenceType] = &[
    EvidenceType::Task,
    EvidenceType::PullRequest,
    EvidenceType::RuntimeLog,
    EvidenceType::Metric,
    EvidenceType::ToolCall,
];

/** State which governed zones and flows a commit's changed files belong to (an informational
 * assertion: it is supported by the commit's change evidence and lists every zone and flow with the
 * version in force, for example "zone:refunds@v3")
 * Input
    - commit: &str - commit
    - touched: &[String] - zones and flows, as "zone:ID@VERSION" or "flow:ID@VERSION"
    - supporting: Vec<String> - the change evidence of the commit
    - scope: &RecordScope - scope
    - created_at: u64 - Unix milliseconds
 * Output
    - Assertion
*/
pub(crate) fn governance_touched(
    commit: &str,
    touched: &[String],
    supporting: Vec<String>,
    scope: &RecordScope,
    created_at: u64,
) -> Assertion {
    let mut parts = vec!["governed_zone_touched", commit, RULES_VERSION];
    parts.extend(touched.iter().map(String::as_str));
    parts.extend(supporting.iter().map(String::as_str));
    Assertion {
        assertion_id: stable_id("asr", &parts),
        schema_version: SCHEMA_VERSION,
        claim: Claim {
            subject: entity_id(EntityKind::Commit, commit),
            subject_kind: EntityKind::Commit,
            subject_key: commit.into(),
            predicate: "governed_zone_touched".into(),
            object: json!(touched),
        },
        statement: format!(
            "governed_zone_touched for Commit {commit}: {}",
            touched.join(", ")
        ),
        independent_support: supporting.len(),
        independent_contradiction: 0,
        derivation: Derivation {
            method: DerivationMethod::Deterministic,
            rule: "governed_zone_touched".into(),
            version: RULES_VERSION.into(),
            inputs: supporting.iter().cloned().map(NodeRef::Evidence).collect(),
        },
        supporting,
        contradicting: Vec::new(),
        derived_from: Vec::new(),
        status: AssertionStatus::Supported,
        scope: scope.clone(),
        valid_from: None,
        valid_to: None,
        confidence: None,
        created_at,
        created_by: Actor::system(),
    }
}

/** Derive whether a task's change is verified from the per-commit and per-deployment assertions:
 * yes when every input is supported yes, no when a supported input says no, unknown otherwise
 * Input
    - task: &str - task key
    - inputs: &[Assertion] - assertions it is derived from
    - scope: &RecordScope - scope
    - created_at: u64 - Unix milliseconds
 * Output
    - Assertion
*/
fn change_verified(
    task: &str,
    inputs: &[Assertion],
    scope: &RecordScope,
    created_at: u64,
) -> Assertion {
    let settled = |assertion: &&Assertion| assertion.status == AssertionStatus::Supported;
    let any_no = inputs
        .iter()
        .filter(settled)
        .any(|assertion| assertion.truth() == Truth::No);
    let all_yes = !inputs.is_empty()
        && inputs
            .iter()
            .all(|assertion| settled(&assertion) && assertion.truth() == Truth::Yes);
    let (object, status) = if any_no {
        (Truth::No, AssertionStatus::Supported)
    } else if all_yes {
        (Truth::Yes, AssertionStatus::Supported)
    } else {
        (Truth::Unknown, AssertionStatus::Unresolved)
    };
    let derived_from = inputs
        .iter()
        .map(|assertion| assertion.assertion_id.clone())
        .collect::<Vec<_>>();
    let mut parts = vec!["change_verified", task, RULES_VERSION];
    parts.extend(derived_from.iter().map(String::as_str));
    Assertion {
        assertion_id: stable_id("asr", &parts),
        schema_version: SCHEMA_VERSION,
        claim: Claim {
            subject: entity_id(EntityKind::Task, task),
            subject_kind: EntityKind::Task,
            subject_key: task.into(),
            predicate: "change_verified".into(),
            object: json!(object),
        },
        statement: format!(
            "change_verified for Task {task}: {}",
            json!(object).as_str().unwrap_or("unknown")
        ),
        supporting: Vec::new(),
        contradicting: Vec::new(),
        independent_support: 0,
        independent_contradiction: 0,
        derivation: Derivation {
            method: DerivationMethod::Deterministic,
            rule: "change_verified".into(),
            version: RULES_VERSION.into(),
            inputs: derived_from
                .iter()
                .map(|id| NodeRef::Assertion(id.clone()))
                .collect(),
        },
        derived_from,
        status,
        scope: scope.clone(),
        valid_from: None,
        valid_to: None,
        confidence: None,
        created_at,
        created_by: Actor::system(),
    }
}

/** Record an agent's hypothesis: it stays proposed (unverified) whatever evidence it cites
 * Input
    - actor: &Actor - proposing actor
    - claim: Claim - proposition
    - statement: &str - proposition in words
    - cited: Vec<String> - evidence the agent cites
    - confidence: Option<Confidence> - the agent's own confidence metadata
    - created_at: u64 - Unix milliseconds
 * Output
    - Assertion
*/
#[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
pub(crate) fn hypothesis(
    actor: &Actor,
    claim: Claim,
    statement: &str,
    cited: Vec<String>,
    confidence: Option<Confidence>,
    created_at: u64,
) -> Assertion {
    let created = created_at.to_string();
    Assertion {
        assertion_id: stable_id(
            "asr",
            &[
                &actor.id,
                &claim.subject,
                &claim.predicate,
                statement,
                &created,
            ],
        ),
        schema_version: SCHEMA_VERSION,
        statement: statement.into(),
        supporting: Vec::new(),
        contradicting: Vec::new(),
        independent_support: 0,
        independent_contradiction: 0,
        derived_from: Vec::new(),
        derivation: Derivation {
            method: DerivationMethod::AgentHypothesis,
            rule: format!("agent:{}", actor.id),
            version: "unverified".into(),
            inputs: cited.into_iter().map(NodeRef::Evidence).collect(),
        },
        status: AssertionStatus::Proposed,
        scope: RecordScope::default(),
        valid_from: None,
        valid_to: None,
        confidence,
        created_at,
        created_by: actor.clone(),
        claim,
    }
}

/** The lineage of an assertion down to its original evidence
 * Fields
    - assertion_id: String - assertion traced
    - chain: Vec<String> - assertions visited, starting with it
    - evidence: Vec<String> - original evidence reached (supporting, contradicting, and inputs)
    - complete: bool - false when the depth limit stopped the trace or an input was missing
*/
#[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Trace {
    pub(crate) assertion_id: String,
    pub(crate) chain: Vec<String>,
    pub(crate) evidence: Vec<String>,
    pub(crate) complete: bool,
}

/** Trace an assertion through the assertions it was derived from to the original evidence,
 * within a depth limit
 * Input
    - assertion_id: &str - assertion
    - assertions: &BTreeMap<String, Assertion> - every assertion
    - max_depth: usize - deepest derivation followed
 * Output
    - Trace
*/
#[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
pub(crate) fn trace(
    assertion_id: &str,
    assertions: &BTreeMap<String, Assertion>,
    max_depth: usize,
) -> Trace {
    let mut result = Trace {
        assertion_id: assertion_id.into(),
        chain: Vec::new(),
        evidence: Vec::new(),
        complete: true,
    };
    let mut frontier = vec![(assertion_id.to_string(), 0usize)];
    let mut seen = BTreeSet::new();
    let mut evidence = BTreeSet::new();
    while let Some((id, depth)) = frontier.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let Some(assertion) = assertions.get(&id) else {
            result.complete = false;
            continue;
        };
        result.chain.push(id.clone());
        evidence.extend(assertion.supporting.iter().cloned());
        evidence.extend(assertion.contradicting.iter().cloned());
        evidence.extend(
            assertion
                .derivation
                .inputs
                .iter()
                .filter_map(|input| match input {
                    NodeRef::Evidence(id) => Some(id.clone()),
                    _ => None,
                }),
        );
        if depth >= max_depth && !assertion.derived_from.is_empty() {
            result.complete = false;
            continue;
        }
        frontier.extend(
            assertion
                .derived_from
                .iter()
                .map(|parent| (parent.clone(), depth + 1)),
        );
    }
    result.evidence = evidence.into_iter().collect();
    result
}
