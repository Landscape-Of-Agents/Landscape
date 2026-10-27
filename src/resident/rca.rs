// RCA graphs and the investigation. A graph is a versioned record of signals, symptoms, candidate
// causes, diagnostic checks, remediations, and verifications, joined by typed relations
// (observed_with, may_cause, confirmed_by, contradicted_by, mitigated_by, verified_by). Every node
// says how it is known (observed, inferred, hypothesized, confirmed, contradicted, unknown) and on
// what basis. A graph written by a person is proposed until someone reviews it; one written by an
// agent is a proposal whose assertions are hypotheses, and review never turns them into confirmed
// causes by itself.
//
// The investigation is deterministic: the signals that match the trigger, the causes they may lead
// to, and the checks that confirm or contradict each cause (evaluated now, against real state). One
// supported cause with a remediation in a reviewed graph yields that remediation's steps and its
// verifications; anything else (no signal, no evidence, two candidates, an unreviewed graph) yields
// a question or a proposal for a person, never a guess. Co-occurrence is never presented as cause.
//
// The Reasoner trait is where a model provider would plug in. None is configured in this
// repository, so the only reasoner is GraphReasoner, which makes no model call; every reasoner
// reports the model calls it made, and the runtime records them per activation.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::model::{Assignment, Change, Definition};
use super::plan::{Plan, PlanStep, StepType};
use super::trigger::{kind_matches, Trigger};
use crate::brick::model::{Author, Target};
use crate::trust::crypto::digest_json;

/** A deterministic check (closed set; nothing here runs a command or fetches a URL)
 * Variants
    - TriggerAttribute - the trigger's attribute equals a value
    - FileExists - a repository file exists (relative path, no "..")
    - SignedStateVerifies - the signed governance state verifies
    - MonitorOk - every Brick monitor of the target is OK (optionally one signal source)
    - RecentTrigger - a trigger of a kind was received within some minutes
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum CheckSpec {
    TriggerAttribute {
        key: String,
        equals: String,
    },
    FileExists {
        path: String,
    },
    SignedStateVerifies,
    MonitorOk {
        #[serde(default)]
        signal: Option<String>,
    },
    RecentTrigger {
        trigger_kind: String,
        within_minutes: u32,
    },
}

impl CheckSpec {
    /** Validate the check's parameters
     * Input
        - None (uses self)
     * Output
        - Result<(), String>
    */
    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Self::FileExists { path } => {
                if path.is_empty()
                    || path.starts_with('/')
                    || path.contains('\\')
                    || path.split('/').any(|part| part == ".." || part.is_empty())
                    || path.contains(':')
                {
                    return Err(format!(
                        "'{path}' must be a relative repository path without '..'"
                    ));
                }
                Ok(())
            }
            Self::TriggerAttribute { key, equals } => {
                if key.is_empty() || key.len() > 64 || equals.len() > 256 {
                    return Err("trigger_attribute needs a key and a bounded value".into());
                }
                Ok(())
            }
            Self::RecentTrigger {
                trigger_kind,
                within_minutes,
            } => {
                super::model::check_kind(trigger_kind)?;
                if *within_minutes == 0 || *within_minutes > 10_080 {
                    return Err("within_minutes must be 1-10080".into());
                }
                Ok(())
            }
            Self::SignedStateVerifies | Self::MonitorOk { .. } => Ok(()),
        }
    }

    /** Describe the check in words
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn text(&self) -> String {
        match self {
            Self::TriggerAttribute { key, equals } => format!("trigger attribute {key} = {equals}"),
            Self::FileExists { path } => format!("{path} exists"),
            Self::SignedStateVerifies => "the signed state verifies".into(),
            Self::MonitorOk { signal } => format!(
                "Brick monitors of the target are OK{}",
                signal
                    .as_ref()
                    .map(|signal| format!(" ({signal})"))
                    .unwrap_or_default()
            ),
            Self::RecentTrigger {
                trigger_kind,
                within_minutes,
            } => {
                format!("a {trigger_kind} trigger arrived within {within_minutes} minutes")
            }
        }
    }
}

/** The result of a check
 * Variants
    - Passed - established true
    - Failed - established false
    - Unknown - not established (the reason says why)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "result",
    content = "reason",
    rename_all = "SCREAMING_SNAKE_CASE"
)]
pub(crate) enum CheckResult {
    Passed,
    Failed,
    Unknown(String),
}

/** What checks read: the real state, supplied by the runtime (tests supply fixtures)
 * Fields
    - root: Option<std::path::PathBuf> - repository root (None: files cannot be read)
    - signed_state: Option<bool> - whether the signed state verifies (None: unknown)
    - monitors: BTreeMap<String, Vec<(String, String)>> - per target key, (signal, result) rows
    - recent: Vec<(String, u64)> - recently received trigger kinds with times
    - now: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct CheckEnv {
    pub(crate) root: Option<std::path::PathBuf>,
    pub(crate) signed_state: Option<bool>,
    pub(crate) monitors: BTreeMap<String, Vec<(String, String)>>,
    pub(crate) recent: Vec<(String, u64)>,
    pub(crate) now: u64,
}

/** Evaluate a check
 * Input
    - check: &CheckSpec - check
    - env: &CheckEnv - real state
    - trigger: &Trigger - the activation's trigger
    - target: &str - the assignment's target key
 * Output
    - CheckResult
*/
pub(crate) fn evaluate(
    check: &CheckSpec,
    env: &CheckEnv,
    trigger: &Trigger,
    target: &str,
) -> CheckResult {
    if let Err(error) = check.validate() {
        return CheckResult::Unknown(error);
    }
    let truth = |value: bool| {
        if value {
            CheckResult::Passed
        } else {
            CheckResult::Failed
        }
    };
    match check {
        CheckSpec::TriggerAttribute { key, equals } => match trigger.attributes.get(key) {
            Some(value) => truth(value == equals),
            None => CheckResult::Unknown(format!("the trigger has no attribute {key}")),
        },
        CheckSpec::FileExists { path } => match &env.root {
            Some(root) => truth(root.join(path).is_file()),
            None => CheckResult::Unknown("the repository is not readable here".into()),
        },
        CheckSpec::SignedStateVerifies => match env.signed_state {
            Some(value) => truth(value),
            None => CheckResult::Unknown("the signed state could not be read".into()),
        },
        CheckSpec::MonitorOk { signal } => match env.monitors.get(target) {
            None => CheckResult::Unknown(format!("no Brick monitor is attached to {target}")),
            Some(rows) => {
                let rows = rows
                    .iter()
                    .filter(|(name, _)| signal.as_ref().is_none_or(|signal| signal == name))
                    .collect::<Vec<_>>();
                if rows.is_empty() {
                    CheckResult::Unknown("no matching monitor".into())
                } else if rows.iter().any(|(_, result)| {
                    matches!(result.as_str(), "NOT_RUN" | "UNSUPPORTED" | "ERROR")
                }) {
                    CheckResult::Unknown("a monitor has not run or has no connector".into())
                } else {
                    truth(rows.iter().all(|(_, result)| result == "OK"))
                }
            }
        },
        CheckSpec::RecentTrigger {
            trigger_kind,
            within_minutes,
        } => {
            let since = env.now.saturating_sub(u64::from(*within_minutes) * 60_000);
            truth(
                env.recent
                    .iter()
                    .any(|(kind, at)| kind_matches(trigger_kind, kind) && *at >= since),
            )
        }
    }
}

/** Describe a check result in words
 * Input
    - result: &CheckResult - result
 * Output
    - String
*/
pub(crate) fn result_text(result: &CheckResult) -> String {
    match result {
        CheckResult::Passed => "PASSED".into(),
        CheckResult::Failed => "FAILED".into(),
        CheckResult::Unknown(reason) => format!("UNKNOWN ({reason})"),
    }
}

/** Kinds of RCA node
 * Variants
    - Signal - an observation a trigger can match
    - Symptom - an effect observed with a signal
    - Cause - a candidate cause
    - Check - a diagnostic check
    - Remediation - a response, as typed plan steps
    - Verification - a postcondition that proves a remediation worked
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum NodeKind {
    Signal,
    Symptom,
    Cause,
    Check,
    Remediation,
    Verification,
}

/** How a node's assertion is known
 * Variants
    - Observed - seen in evidence
    - Inferred - derived from observed facts
    - Hypothesized - proposed, not established
    - Confirmed - established by evidence and reviewed
    - Contradicted - evidence says otherwise
    - Unknown - not established either way
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Basis {
    Observed,
    Inferred,
    #[default]
    Hypothesized,
    Confirmed,
    Contradicted,
    Unknown,
}

/** Relations between nodes
 * Variants
    - ObservedWith - seen together (never cause)
    - MayCause - a candidate causal link
    - ConfirmedBy - a check whose pass supports a cause
    - ContradictedBy - a check whose pass contradicts a cause
    - MitigatedBy - a remediation of a cause
    - VerifiedBy - a verification of a remediation
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Relation {
    ObservedWith,
    MayCause,
    ConfirmedBy,
    ContradictedBy,
    MitigatedBy,
    VerifiedBy,
}

/** The trigger a signal matches
 * Fields
    - source: super::model::TriggerSource - source
    - kinds: Vec<String> - kinds or patterns
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SignalMatch {
    pub(crate) source: super::model::TriggerSource,
    pub(crate) kinds: Vec<String>,
}

/** One node
 * Fields
    - id: String - identifier, unique in the graph
    - kind: NodeKind - kind
    - label: String - what it says
    - status: Basis - how it is known
    - basis: String - the evidence or reasoning behind the status
    - source: String - where the assertion came from (person, activation, document)
    - matches: Option<SignalMatch> - what a signal matches
    - check: Option<CheckSpec> - the check of a check or verification node
    - steps: Vec<PlanStep> - the steps of a remediation node
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Node {
    pub(crate) id: String,
    pub(crate) kind: NodeKind,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) status: Basis,
    #[serde(default)]
    pub(crate) basis: String,
    #[serde(default)]
    pub(crate) source: String,
    #[serde(default)]
    pub(crate) matches: Option<SignalMatch>,
    #[serde(default)]
    pub(crate) check: Option<CheckSpec>,
    #[serde(default)]
    pub(crate) steps: Vec<PlanStep>,
}

/** One edge
 * Fields
    - from: String - node
    - to: String - node
    - relation: Relation - relation
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Edge {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) relation: Relation,
}

/** An RCA graph version
 * Fields
    - rca_id: String - identifier
    - version: u32 - version
    - name: String - name
    - target: String - the Flow, Zone, or Runbook it explains ("flow:ID")
    - nodes: Vec<Node> - nodes
    - edges: Vec<Edge> - edges
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Graph {
    pub(crate) rca_id: String,
    #[serde(default = "one")]
    pub(crate) version: u32,
    pub(crate) name: String,
    pub(crate) target: String,
    pub(crate) nodes: Vec<Node>,
    pub(crate) edges: Vec<Edge>,
}

/** The default version of a new graph
 * Input
    - None
 * Output
    - u32 1
*/
fn one() -> u32 {
    1
}

impl Graph {
    /** The graph's digest
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub(crate) fn digest(&self) -> String {
        digest_json("resident/rca/v1", &json!(self))[..32].to_string()
    }

    /** A node by id
     * Input
        - id: &str - node
     * Output
        - Option<&Node>
    */
    pub(crate) fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /** The nodes an edge of a relation leads to from a node
     * Input
        - from: &str - node
        - relation: Relation - relation
     * Output
        - Vec<&Node>
    */
    pub(crate) fn next(&self, from: &str, relation: Relation) -> Vec<&Node> {
        self.edges
            .iter()
            .filter(|edge| edge.from == from && edge.relation == relation)
            .filter_map(|edge| self.node(&edge.to))
            .collect()
    }
}

/** Validate a graph: unique node ids, known edge ends, relations between the right kinds, signals
 * with matches, checks and verifications with valid checks, remediation steps, and that a node
 * marked confirmed names its basis
 * Input
    - graph: &Graph - graph
 * Output
    - Vec<(String, String)> problems
*/
pub(crate) fn validate(graph: &Graph) -> Vec<(String, String)> {
    let mut problems = Vec::new();
    let mut push = |code: &str, message: String| problems.push((code.to_string(), message));
    if crate::platform::validate_name("rca", &graph.rca_id).is_err() {
        push("RCA_ID", format!("'{}' is not a valid id", graph.rca_id));
    }
    if Target::parse(&graph.target).is_err() {
        push(
            "RCA_TARGET",
            format!("'{}' is not flow:ID, zone:ID, or runbook:ID", graph.target),
        );
    }
    let mut ids = BTreeSet::new();
    for node in &graph.nodes {
        if !ids.insert(node.id.clone()) {
            push(
                "RCA_DUPLICATE_NODE",
                format!("node {} appears twice", node.id),
            );
        }
        if crate::platform::validate_name("node", &node.id).is_err() {
            push(
                "RCA_NODE_ID",
                format!("'{}' is not a valid node id", node.id),
            );
        }
        if crate::telemetry::redact::redact(&node.label).1 > 0
            || crate::telemetry::redact::redact(&node.basis).1 > 0
        {
            push(
                "RCA_SECRET",
                format!("node {} holds a secret-looking value", node.id),
            );
        }
        match node.kind {
            NodeKind::Signal if node.matches.is_none() => push(
                "RCA_SIGNAL_MATCH",
                format!("signal {} needs 'matches'", node.id),
            ),
            NodeKind::Signal => {
                for kind in &node
                    .matches
                    .as_ref()
                    .map(|m| m.kinds.clone())
                    .unwrap_or_default()
                {
                    if let Err(error) = super::model::check_kind(kind) {
                        push("RCA_SIGNAL_MATCH", format!("signal {}: {error}", node.id));
                    }
                }
            }
            NodeKind::Check | NodeKind::Verification => match &node.check {
                None => push("RCA_CHECK", format!("{} needs a check", node.id)),
                Some(check) => {
                    if let Err(error) = check.validate() {
                        push("RCA_CHECK", format!("{}: {error}", node.id));
                    }
                }
            },
            NodeKind::Remediation if node.steps.is_empty() => push(
                "RCA_REMEDIATION",
                format!("remediation {} has no step", node.id),
            ),
            _ => {}
        }
        if node.status == Basis::Confirmed && node.basis.trim().is_empty() {
            push(
                "RCA_UNSUPPORTED_CONFIRMATION",
                format!("node {} is confirmed without a basis", node.id),
            );
        }
    }
    let kind = |id: &str| graph.node(id).map(|node| node.kind);
    for edge in &graph.edges {
        let (Some(from), Some(to)) = (kind(&edge.from), kind(&edge.to)) else {
            push(
                "RCA_DANGLING_EDGE",
                format!("edge {} -> {} names a missing node", edge.from, edge.to),
            );
            continue;
        };
        use NodeKind::*;
        let fits = match edge.relation {
            Relation::ObservedWith => matches!((from, to), (Signal | Symptom, Signal | Symptom)),
            Relation::MayCause => matches!((from, to), (Signal | Symptom, Cause) | (Cause, Cause)),
            Relation::ConfirmedBy | Relation::ContradictedBy => {
                matches!((from, to), (Cause, Check))
            }
            Relation::MitigatedBy => matches!((from, to), (Cause, Remediation)),
            Relation::VerifiedBy => matches!((from, to), (Remediation, Verification)),
        };
        if !fits {
            push(
                "RCA_RELATION_KIND",
                format!(
                    "{:?} cannot join {} ({from:?}) to {} ({to:?})",
                    edge.relation, edge.from, edge.to
                ),
            );
        }
    }
    problems
}

/** Lifecycle of an RCA graph version
 * Variants
    - Proposed - written (by a person or an agent), not reviewed; investigates but never drives
      remediation by itself
    - Reviewed - a person reviewed it; its remediations may be planned
    - Rejected - a reviewer rejected it
    - Retired - superseded
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RcaState {
    Proposed,
    Reviewed,
    Rejected,
    Retired,
}

/** Who wrote a graph version
 * Variants
    - Person - a person, by file
    - Activation - an agent activation proposed it (its assertions are hypotheses)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub(crate) enum Origin {
    Person,
    Activation { activation_id: String },
}

/** One stored graph version
 * Fields
    - graph: Graph - content
    - digest: String - its digest
    - state: RcaState - lifecycle
    - origin: Origin - who wrote it
    - author: Author - the person who stored it (or the reviewer of an agent proposal)
    - self_reviewed: bool - the author reviewed it
    - changes: Vec<Change> - lifecycle history
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RcaRecord {
    pub(crate) graph: Graph,
    pub(crate) digest: String,
    pub(crate) state: RcaState,
    pub(crate) origin: Origin,
    pub(crate) author: Author,
    #[serde(default)]
    pub(crate) self_reviewed: bool,
    pub(crate) changes: Vec<Change>,
}

/** What the investigation found for one candidate cause
 * Variants
    - Supported - a confirming check passed and no contradicting check passed
    - Contradicted - a contradicting check passed
    - Unknown - no check established anything
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Verdict {
    Supported,
    Contradicted,
    Unknown,
}

/** One evaluated check
 * Fields
    - node: String - check node
    - relation: Relation - confirmed_by or contradicted_by
    - check: String - what it checked
    - result: CheckResult - result
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Observation {
    pub(crate) node: String,
    pub(crate) relation: Relation,
    pub(crate) check: String,
    pub(crate) result: CheckResult,
}

/** One candidate cause
 * Fields
    - node: String - cause node
    - label: String - its label
    - prior: Basis - what the graph says about it
    - verdict: Verdict - what this activation's evidence says
    - observations: Vec<Observation> - the checks behind the verdict
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Candidate {
    pub(crate) node: String,
    pub(crate) label: String,
    pub(crate) prior: Basis,
    pub(crate) verdict: Verdict,
    pub(crate) observations: Vec<Observation>,
}

/** The conclusion of an investigation
 * Variants
    - Identified - exactly one cause is supported by evidence
    - Ambiguous - several causes are supported, or none of the candidates is
    - InsufficientEvidence - no signal of the graph matches, or there is no graph
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Conclusion {
    Identified,
    Ambiguous,
    InsufficientEvidence,
}

/** The result of an investigation
 * Fields
    - rca: Option<String> - graph version used ("ID@N"), None without one
    - rca_state: Option<RcaState> - its state
    - signals: Vec<String> - signal nodes the trigger matched
    - candidates: Vec<Candidate> - candidate causes
    - conclusion: Conclusion - conclusion
    - cause: Option<String> - the identified cause
    - gaps: Vec<String> - what is missing
    - consulted: Vec<String> - nodes and edges consulted ("node:ID", "edge:A>B")
    - plan: Plan - the typed plan it proposes
    - reasoner: String - which reasoner produced it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Investigation {
    pub(crate) rca: Option<String>,
    pub(crate) rca_state: Option<RcaState>,
    pub(crate) signals: Vec<String>,
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) conclusion: Conclusion,
    pub(crate) cause: Option<String>,
    pub(crate) gaps: Vec<String>,
    pub(crate) consulted: Vec<String>,
    pub(crate) plan: Plan,
    pub(crate) reasoner: String,
}

/** What a reasoner receives
 * Fields
    - definition: &Definition - definition
    - assignment: &Assignment - assignment
    - trigger: &Trigger - trigger
    - rca: Option<&RcaRecord> - the assignment's graph
    - checks: &CheckEnv - real state for checks
*/
pub(crate) struct ReasonInput<'a> {
    pub(crate) definition: &'a Definition,
    pub(crate) assignment: &'a Assignment,
    pub(crate) trigger: &'a Trigger,
    pub(crate) rca: Option<&'a RcaRecord>,
    pub(crate) checks: &'a CheckEnv,
}

/** A reasoner: turns a trigger, context, and RCA graph into an investigation and a plan */
pub(crate) trait Reasoner {
    /** The reasoner's name, recorded on every investigation
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn name(&self) -> String;

    /** Investigate
     * Input
        - input: &ReasonInput - what it may use
     * Output
        - Result<Investigation, String>
    */
    fn investigate(&mut self, input: &ReasonInput) -> Result<Investigation, String>;

    /** The model calls made so far by this reasoner (0 for a deterministic one)
     * Input
        - None (uses self)
     * Output
        - u64
    */
    fn model_calls(&self) -> u64;

    /** The provider and model, when the reasoner calls one
     * Input
        - None (uses self)
     * Output
        - Option<String>
    */
    fn model(&self) -> Option<String> {
        None
    }
}

/** The deterministic reasoner: RCA traversal and checks, no model */
pub(crate) struct GraphReasoner;

impl Reasoner for GraphReasoner {
    /** Name the reasoner
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn name(&self) -> String {
        "rca-graph (deterministic, no model)".into()
    }

    /** Investigate by traversing the RCA graph (see investigate)
     * Input
        - input: &ReasonInput - what it may use
     * Output
        - Result<Investigation, String>
    */
    fn investigate(&mut self, input: &ReasonInput) -> Result<Investigation, String> {
        Ok(investigate(input, &self.name()))
    }

    /** No model calls, ever
     * Input
        - None (uses self)
     * Output
        - u64 0
    */
    fn model_calls(&self) -> u64 {
        0
    }
}

/** A step
 * Input
    - id: &str - step id
    - step_type: StepType - type
    - params: &[(&str, String)] - parameters
    - depends_on: &[&str] - dependencies
    - basis: String - why
 * Output
    - PlanStep
*/
pub(crate) fn step(
    id: &str,
    step_type: StepType,
    params: &[(&str, String)],
    depends_on: &[&str],
    basis: String,
) -> PlanStep {
    PlanStep {
        step_id: id.into(),
        step_type,
        params: params
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect(),
        check: None,
        depends_on: depends_on.iter().map(|item| item.to_string()).collect(),
        proposal: Vec::new(),
        basis,
    }
}

/** Investigate a trigger against the assignment's graph and build the plan (deterministic)
 * Input
    - input: &ReasonInput - definition, assignment, trigger, graph, and real state
    - reasoner: &str - name recorded on the investigation
 * Output
    - Investigation
*/
pub(crate) fn investigate(input: &ReasonInput, reasoner: &str) -> Investigation {
    let target = input.assignment.target.key();
    let mut consulted = Vec::new();
    let mut gaps = Vec::new();
    let Some(record) = input.rca else {
        gaps.push("the assignment has no RCA graph".to_string());
        return finish(
            input,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Conclusion::InsufficientEvidence,
            None,
            gaps,
            consulted,
            reasoner,
        );
    };
    let graph = &record.graph;
    let reference = format!("{}@{}", graph.rca_id, graph.version);
    let signals = graph
        .nodes
        .iter()
        .filter(|node| node.kind == NodeKind::Signal)
        .filter(|node| {
            node.matches.as_ref().is_some_and(|matches| {
                matches.source == input.trigger.source
                    && matches
                        .kinds
                        .iter()
                        .any(|pattern| kind_matches(pattern, &input.trigger.kind))
            })
        })
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    consulted.extend(signals.iter().map(|id| format!("node:{id}")));
    if signals.is_empty() {
        gaps.push(format!(
            "no signal of {reference} matches {} {}",
            input.trigger.source.name(),
            input.trigger.kind
        ));
        return finish(
            input,
            Some((reference, record.state)),
            Some(record),
            signals,
            Vec::new(),
            Conclusion::InsufficientEvidence,
            None,
            gaps,
            consulted,
            reasoner,
        );
    }
    // Causes reachable from the signals, directly or through symptoms observed with them
    let mut frontier = signals.clone();
    for signal in &signals {
        for symptom in graph.next(signal, Relation::ObservedWith) {
            consulted.push(format!("edge:{signal}>{}", symptom.id));
            frontier.push(symptom.id.clone());
        }
    }
    let mut causes = BTreeSet::new();
    for from in &frontier {
        for cause in graph.next(from, Relation::MayCause) {
            consulted.push(format!("edge:{from}>{}", cause.id));
            causes.insert(cause.id.clone());
        }
    }
    let mut candidates = Vec::new();
    for cause_id in &causes {
        let Some(cause) = graph.node(cause_id) else {
            continue;
        };
        consulted.push(format!("node:{cause_id}"));
        let mut observations = Vec::new();
        for relation in [Relation::ConfirmedBy, Relation::ContradictedBy] {
            for check in graph.next(cause_id, relation) {
                consulted.push(format!("edge:{cause_id}>{}", check.id));
                let Some(spec) = &check.check else { continue };
                observations.push(Observation {
                    node: check.id.clone(),
                    relation,
                    check: spec.text(),
                    result: evaluate(spec, input.checks, input.trigger, &target),
                });
            }
        }
        let passed = |relation| {
            observations
                .iter()
                .any(|item| item.relation == relation && item.result == CheckResult::Passed)
        };
        let verdict = if passed(Relation::ContradictedBy) || cause.status == Basis::Contradicted {
            Verdict::Contradicted
        } else if passed(Relation::ConfirmedBy) {
            Verdict::Supported
        } else {
            Verdict::Unknown
        };
        if observations.is_empty() {
            gaps.push(format!("cause {cause_id} has no diagnostic check"));
        }
        for item in observations
            .iter()
            .filter(|item| matches!(item.result, CheckResult::Unknown(_)))
        {
            if let CheckResult::Unknown(reason) = &item.result {
                gaps.push(format!("{} ({}): {reason}", item.node, item.check));
            }
        }
        candidates.push(Candidate {
            node: cause_id.clone(),
            label: cause.label.clone(),
            prior: cause.status,
            verdict,
            observations,
        });
    }
    let supported = candidates
        .iter()
        .filter(|candidate| candidate.verdict == Verdict::Supported)
        .collect::<Vec<_>>();
    let (conclusion, cause) = match supported.as_slice() {
        [one] => (Conclusion::Identified, Some(one.node.clone())),
        _ if candidates.is_empty() => {
            gaps.push("the matched signals lead to no candidate cause".into());
            (Conclusion::InsufficientEvidence, None)
        }
        _ => (Conclusion::Ambiguous, None),
    };
    finish(
        input,
        Some((reference, record.state)),
        Some(record),
        signals,
        candidates,
        conclusion,
        cause,
        gaps,
        consulted,
        reasoner,
    )
}

/** Build the investigation result and its plan
 * Input
    - input: &ReasonInput - inputs
    - rca: Option<(String, RcaState)> - graph reference and state
    - record: Option<&RcaRecord> - graph
    - signals, candidates, conclusion, cause, gaps, consulted - findings
    - reasoner: &str - reasoner name
 * Output
    - Investigation
*/
#[allow(clippy::too_many_arguments)]
fn finish(
    input: &ReasonInput,
    rca: Option<(String, RcaState)>,
    record: Option<&RcaRecord>,
    signals: Vec<String>,
    candidates: Vec<Candidate>,
    conclusion: Conclusion,
    cause: Option<String>,
    gaps: Vec<String>,
    mut consulted: Vec<String>,
    reasoner: &str,
) -> Investigation {
    let allowed = |step_type| input.definition.policy(step_type).is_some();
    let summary = format!(
        "{} {} on {}",
        input.trigger.source.name(),
        input.trigger.kind,
        input.assignment.target.key()
    );
    let mut steps = Vec::new();
    let reviewed = rca
        .as_ref()
        .is_some_and(|(_, state)| *state == RcaState::Reviewed);
    let remediation = match (record, &cause) {
        (Some(record), Some(cause)) => {
            let graph = &record.graph;
            let remediations = graph.next(cause, Relation::MitigatedBy);
            for node in &remediations {
                consulted.push(format!("edge:{cause}>{}", node.id));
            }
            remediations.first().map(|node| {
                let verifications = graph
                    .next(&node.id, Relation::VerifiedBy)
                    .into_iter()
                    .filter_map(|verification| {
                        verification.check.clone().map(|check| {
                            (verification.id.clone(), verification.label.clone(), check)
                        })
                    })
                    .collect::<Vec<_>>();
                (node.id.clone(), node.steps.clone(), verifications)
            })
        }
        _ => None,
    };
    match (conclusion, remediation) {
        (Conclusion::Identified, Some((node, remediation_steps, verifications))) => {
            let mut planned = remediation_steps
                .into_iter()
                .map(|mut item| {
                    item.basis = format!(
                        "remediation {node} of cause {}",
                        cause.clone().unwrap_or_default()
                    );
                    item
                })
                .collect::<Vec<_>>();
            let last = planned.last().map(|item| item.step_id.clone());
            for (index, (id, label, check)) in verifications.into_iter().enumerate() {
                let mut verify = step(
                    &format!("verify-{}", index + 1),
                    StepType::Verify,
                    &[],
                    &last.iter().map(String::as_str).collect::<Vec<_>>(),
                    format!("verification {id}: {label}"),
                );
                verify.check = Some(check);
                planned.push(verify);
            }
            if reviewed {
                steps = planned;
            } else if allowed(StepType::ProposeActionPlan) {
                let mut proposal = step(
                    "propose",
                    StepType::ProposeActionPlan,
                    &[(
                        "summary",
                        format!("{summary}: remediation {node} from a graph that is not reviewed"),
                    )],
                    &[],
                    "the graph is proposed, so its remediation needs a person".into(),
                );
                proposal.proposal = planned;
                steps.push(proposal);
            }
        }
        (Conclusion::Identified, None) | (Conclusion::Ambiguous, _) => {
            let mut proposal = step(
                "propose",
                StepType::ProposeActionPlan,
                &[(
                    "summary",
                    format!(
                        "{summary}: {}",
                        if conclusion == Conclusion::Ambiguous {
                            "several or no causes are supported"
                        } else {
                            "the cause has no remediation"
                        }
                    ),
                )],
                &[],
                "no validated deterministic path".into(),
            );
            if let Some(record) = record {
                for candidate in candidates
                    .iter()
                    .filter(|candidate| candidate.verdict != Verdict::Contradicted)
                {
                    for node in record.graph.next(&candidate.node, Relation::MitigatedBy) {
                        for item in &node.steps {
                            let mut item = item.clone();
                            item.step_id = format!("{}-{}", node.id, item.step_id);
                            item.depends_on = item
                                .depends_on
                                .iter()
                                .map(|dependency| format!("{}-{dependency}", node.id))
                                .collect();
                            item.basis = format!(
                                "remediation {} of candidate {} ({:?})",
                                node.id, candidate.node, candidate.verdict
                            );
                            proposal.proposal.push(item);
                        }
                    }
                }
            }
            if !proposal.proposal.is_empty() && allowed(StepType::ProposeActionPlan) {
                steps.push(proposal);
            } else if allowed(StepType::AskUser) {
                steps.push(step("ask", StepType::AskUser, &[("question", format!("{summary}: the evidence supports no single cause. What should be done?"))], &[], "ambiguous evidence".into()));
            }
        }
        (Conclusion::InsufficientEvidence, _) => {
            if allowed(StepType::ProposeRca) {
                steps.push(step(
                    "propose-rca",
                    StepType::ProposeRca,
                    &[("label", summary.clone())],
                    &[],
                    "no signal or cause in the graph explains this trigger".into(),
                ));
            }
            if allowed(StepType::AskUser) {
                steps.push(step(
                    "ask",
                    StepType::AskUser,
                    &[(
                        "question",
                        format!(
                            "{summary}: insufficient evidence ({}). What should be done?",
                            gaps.join("; ")
                        ),
                    )],
                    &[],
                    "insufficient evidence".into(),
                ));
            }
        }
    }
    if steps.is_empty() {
        // Nothing the definition allows can respond: record that, never improvise
        steps.push(step(
            "ask",
            StepType::AskUser,
            &[(
                "question",
                format!("{summary}: no allowed step can respond"),
            )],
            &[],
            "no allowed response".into(),
        ));
    }
    consulted.sort();
    consulted.dedup();
    Investigation {
        rca: rca.as_ref().map(|(reference, _)| reference.clone()),
        rca_state: rca.map(|(_, state)| state),
        signals,
        candidates,
        conclusion,
        cause,
        gaps,
        consulted,
        plan: Plan::new(steps),
        reasoner: reasoner.into(),
    }
}

/** The graph an activation proposes when no signal explains its trigger: the trigger as an
 * observed signal and an unknown cause, every assertion hypothesized
 * Input
    - assignment: &Assignment - assignment
    - trigger: &Trigger - trigger
    - label: &str - proposal label
    - version: u32 - next version of the proposal graph
 * Output
    - Graph
*/
pub(crate) fn proposal_graph(
    assignment: &Assignment,
    trigger: &Trigger,
    label: &str,
    version: u32,
) -> Graph {
    let source = format!("trigger {}", trigger.trigger_id);
    Graph {
        rca_id: format!("{}-proposed", assignment.assignment_id),
        version,
        name: label.chars().take(200).collect(),
        target: assignment.target.key(),
        nodes: vec![
            Node {
                id: "signal".into(),
                kind: NodeKind::Signal,
                label: format!("{} {}", trigger.source.name(), trigger.kind),
                status: Basis::Observed,
                basis: format!("received at {} ({})", trigger.received_at, trigger.origin),
                source: source.clone(),
                matches: Some(SignalMatch {
                    source: trigger.source,
                    kinds: vec![trigger.kind.clone()],
                }),
                check: None,
                steps: Vec::new(),
            },
            Node {
                id: "cause".into(),
                kind: NodeKind::Cause,
                label: "unknown cause (to be established by a person)".into(),
                status: Basis::Hypothesized,
                basis: "no evidence; proposed by an activation".into(),
                source,
                matches: None,
                check: None,
                steps: Vec::new(),
            },
        ],
        edges: vec![Edge {
            from: "signal".into(),
            to: "cause".into(),
            relation: Relation::MayCause,
        }],
    }
}

/** Record a lifecycle change on an RCA record
 * Input
    - record: &mut RcaRecord - record
    - to: RcaState - new state
    - by: &Author - who
    - reason: &str - why
    - at: u64 - Unix milliseconds
 * Output
    - None
*/
pub(crate) fn change(record: &mut RcaRecord, to: RcaState, by: &Author, reason: &str, at: u64) {
    record.state = to;
    record.changes.push(Change {
        to: crate::resident::word(&to),
        by: by.clone(),
        reason: reason.into(),
        at,
    });
}
