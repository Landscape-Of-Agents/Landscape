// Episode reconstruction: what happened during the work, rebuilt from the intake's references to
// Work Capture records (only records the intake could resolve for the requester, so nothing
// unreadable is used). Each item keeps its basis: tool calls, commits, CI, tests, deployments, and
// checks are observed; human interventions and the recorded outcome are declared; assertions are
// derived; unresolved and contradicted assertions and missing evidence are unknown effects.
// Items are sorted by time for reading only: order never implies that one caused another.

use serde::{Deserialize, Serialize};

use super::Basis;
use crate::capability::intake::{CapabilityIntake, EvidenceRef};
use crate::capability::refs::Sources;
use crate::capture::correlate::assertions::AssertionStatus;
use crate::capture::format::record::{EvidenceRecord, EvidenceType};
use crate::capture::Truth;

/** The note every reconstruction carries about ordering */
pub(crate) const ORDERING_NOTE: &str = "Items are ordered by observation time only; the order does not imply that one item caused another.";

/** Kinds of reconstructed items
 * Variants
    - Action - something an agent or person did (a tool call, a commit, a rollback)
    - Decision - an authority decision recorded for an action
    - Observation - a recorded result (CI, tests, deployment, checks, logs, metrics)
    - Intervention - a human intervention
    - Outcome - the recorded or derived outcome
    - UnresolvedEffect - something that could not be established
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ItemKind {
    Action,
    Decision,
    Observation,
    Intervention,
    Outcome,
    UnresolvedEffect,
}

/** One reconstructed item
 * Fields
    - kind: ItemKind - kind
    - description: String - what it is
    - basis: Basis - how it is known
    - evidence_type: Option<EvidenceType> - type of its evidence, None when not evidence-based
    - evidence_refs: Vec<String> - evidence or assertion identifiers
    - at: Option<u64> - observation time (Unix ms), None when unknown
    - environment: Option<String> - environment it was observed in
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReconstructedItem {
    pub(crate) kind: ItemKind,
    pub(crate) description: String,
    pub(crate) basis: Basis,
    pub(crate) evidence_type: Option<EvidenceType>,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) at: Option<u64>,
    pub(crate) environment: Option<String>,
}

/** A reconstructed episode
 * Fields
    - items: Vec<ReconstructedItem> - items in time order (unknown times last)
    - ordering_note: String - ORDERING_NOTE
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Reconstruction {
    pub(crate) items: Vec<ReconstructedItem>,
    pub(crate) ordering_note: String,
}

/** Describe an evidence record as an item
 * Input
    - record: &EvidenceRecord - record
 * Output
    - Vec<(ItemKind, String)> - one or two items (a tool call may carry a decision)
*/
fn describe(record: &EvidenceRecord) -> Vec<(ItemKind, String)> {
    let field = |name: &str| record.field(name);
    let id = &record.source_ref.record_id;
    match record.evidence_type {
        EvidenceType::ToolCall | EvidenceType::AgentEvent | EvidenceType::AgentSession => {
            let tool = field("tool").unwrap_or_else(|| "a tool".into());
            let target = field("command")
                .or_else(|| field("resource"))
                .map(|target| format!(" on {target}"))
                .unwrap_or_default();
            let mut items = vec![(ItemKind::Action, format!("agent used {tool}{target}"))];
            if let Some(decision) = field("decision") {
                items.push((
                    ItemKind::Decision,
                    format!("Crane decided {decision} for {tool}"),
                ));
            }
            items
        }
        EvidenceType::CodeChange => vec![(
            ItemKind::Action,
            format!(
                "committed {}",
                field("commit").unwrap_or_else(|| id.clone())
            ),
        )],
        EvidenceType::PullRequest => vec![(
            ItemKind::Action,
            format!(
                "opened pull request {}",
                field("number").unwrap_or_else(|| id.clone())
            ),
        )],
        EvidenceType::Deployment
            if field("status").is_some_and(|status| {
                status.contains("rolled_back") || status.contains("rollback")
            }) =>
        {
            vec![(ItemKind::Action, format!("rolled back deployment {id}"))]
        }
        EvidenceType::Deployment => vec![(
            ItemKind::Observation,
            format!(
                "deployment {id} to {} reported {}",
                record
                    .scope
                    .environment
                    .clone()
                    .unwrap_or_else(|| "an unknown environment".into()),
                field("status").unwrap_or_else(|| "no status".into())
            ),
        )],
        EvidenceType::CiRun => vec![(
            ItemKind::Observation,
            format!(
                "CI {} reported {}",
                field("name").unwrap_or_else(|| id.clone()),
                field("status").unwrap_or_else(|| "no status".into())
            ),
        )],
        EvidenceType::TestReport => vec![(
            ItemKind::Observation,
            format!(
                "tests reported {} passed, {} failed",
                field("passed").unwrap_or_else(|| "?".into()),
                field("failed").unwrap_or_else(|| "?".into())
            ),
        )],
        EvidenceType::Verification => vec![(
            ItemKind::Observation,
            format!(
                "check {} reported healthy={}",
                field("check").unwrap_or_else(|| id.clone()),
                field("healthy").unwrap_or_else(|| "unknown".into())
            ),
        )],
        other => vec![(ItemKind::Observation, format!("{other:?} record {id}"))],
    }
}

/** Reconstruct an episode from an intake
 * Input
    - intake: &CapabilityIntake - validated intake
    - sources: &Sources - canonical sources and the requester's access
 * Output
    - Reconstruction
*/
pub(crate) fn reconstruct(intake: &CapabilityIntake, sources: &Sources) -> Reconstruction {
    let observed = &intake.observed;
    let references: Vec<&EvidenceRef> = observed
        .code_changes
        .iter()
        .chain(&observed.tool_calls)
        .chain(&observed.ci_and_tests)
        .chain(&observed.deployments)
        .chain(&observed.runtime)
        .chain(&observed.recovery)
        .collect();
    let mut items = Vec::new();
    for reference in references {
        let (_, Some(record)) = sources.evidence(&reference.evidence_id) else {
            continue;
        };
        for (kind, description) in describe(record) {
            items.push(ReconstructedItem {
                kind,
                description,
                basis: Basis::Observed,
                evidence_type: Some(record.evidence_type),
                evidence_refs: vec![record.evidence_id.clone()],
                at: record.observed_at,
                environment: record.scope.environment.clone(),
            });
        }
    }
    for intervention in &observed.human_interventions {
        items.push(ReconstructedItem {
            kind: ItemKind::Intervention,
            description: intervention.clone(),
            basis: Basis::Declared,
            evidence_type: None,
            evidence_refs: Vec::new(),
            at: None,
            environment: None,
        });
    }
    for assertion in &observed.assertions {
        let unresolved = assertion.status != AssertionStatus::Supported
            || assertion.truth == Truth::Unknown
            || !assertion.contradicting.is_empty();
        let (kind, basis) = if unresolved {
            (ItemKind::UnresolvedEffect, Basis::Unknown)
        } else {
            (ItemKind::Outcome, Basis::Derived)
        };
        let truth = serde_json::to_value(assertion.truth)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        items.push(ReconstructedItem {
            kind,
            description: format!(
                "{} for {} is {truth} ({:?})",
                assertion.predicate, assertion.subject_key, assertion.status
            ),
            basis,
            evidence_type: None,
            evidence_refs: vec![assertion.assertion_id.clone()],
            at: None,
            environment: None,
        });
    }
    for missing in &intake.completeness.missing {
        items.push(ReconstructedItem {
            kind: ItemKind::UnresolvedEffect,
            description: format!(
                "required {:?} evidence is missing ({:?})",
                missing.evidence_type, missing.state
            ),
            basis: Basis::Unknown,
            evidence_type: Some(missing.evidence_type),
            evidence_refs: Vec::new(),
            at: None,
            environment: None,
        });
    }
    let outcome = match intake.work.outcome {
        Some(outcome) => ReconstructedItem {
            kind: ItemKind::Outcome,
            description: format!("the episode's recorded outcome is {outcome:?}"),
            basis: Basis::Declared,
            evidence_type: None,
            evidence_refs: Vec::new(),
            at: None,
            environment: None,
        },
        None => ReconstructedItem {
            kind: ItemKind::UnresolvedEffect,
            description: "the episode's final outcome was not recorded".into(),
            basis: Basis::Unknown,
            evidence_type: None,
            evidence_refs: Vec::new(),
            at: None,
            environment: None,
        },
    };
    items.push(outcome);
    items.sort_by(|a, b| {
        (a.at.is_none(), a.at, &a.evidence_refs).cmp(&(b.at.is_none(), b.at, &b.evidence_refs))
    });
    Reconstruction {
        items,
        ordering_note: ORDERING_NOTE.into(),
    }
}
