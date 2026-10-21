// Context contributions: structured proposals of context for a zone, flow, entity, or work
// episode. Unstructured agent text is never appended to trusted context. A contribution carries a
// namespace (agent-proposed, observed, human-approved, system-derived) that classifies it, but
// trust comes from the checks here, not from the label: agents may only propose in the
// agent-proposed namespace, "observed" needs evidence that exists, "human-approved" is reached only
// through review by a different, authorized human, and nobody approves their own contribution.
// Constraint and policy proposals never change a policy: weakening one is refused outright, and
// any accepted proposal still has to go through Crane's signed policy workflow.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::capture::{
    stable_id, AccessPolicy, Actor, ActorKind, CaptureError, CaptureResult, ErrorCode,
    SCHEMA_VERSION,
};
use crate::telemetry::events::redact_value;

/** Largest serialized contribution content, in bytes */
const MAX_CONTENT_BYTES: usize = 16 * 1024;

/** What a contribution offers
 * Variants
    - Observation - something observed, with evidence
    - Hypothesis - a possible explanation, unverified
    - Summary - a summary of evidence
    - Question - an open question
    - ProcedureProposal - a proposed procedure
    - RelationshipProposal - a proposed relation between entities
    - ConstraintProposal - a proposed constraint or policy change
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContributionType {
    Observation,
    Hypothesis,
    Summary,
    Question,
    ProcedureProposal,
    RelationshipProposal,
    ConstraintProposal,
}

/** Classification of a contribution (not a trust mechanism)
 * Variants
    - AgentProposed - proposed by an agent
    - Observed - an observation backed by existing evidence
    - HumanApproved - accepted by an authorized human reviewer
    - SystemDerived - derived deterministically by Work Capture
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Namespace {
    AgentProposed,
    Observed,
    HumanApproved,
    SystemDerived,
}

/** Lifecycle of a contribution
 * Variants
    - Proposed - awaiting review
    - Accepted - accepted by a reviewer
    - Rejected - rejected by a reviewer
    - Superseded - replaced by a newer contribution
    - Expired - past its validity
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ContributionStatus {
    Proposed,
    Accepted,
    Rejected,
    Superseded,
    Expired,
}

/** Whether a contribution is about this task only or about the system
 * Variants
    - TaskLocal - context of one work episode
    - Reusable - proposed system context for a zone, flow, or entity
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Locality {
    TaskLocal,
    Reusable,
}

/** What a contribution can target
 * Variants
    - Zone - a Landscape zone (existing id)
    - Flow - a Landscape flow (existing id)
    - Entity - an entity of the evidence graph (entity id)
    - WorkEpisode - a work episode
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetKind {
    Zone,
    Flow,
    Entity,
    WorkEpisode,
}

/** A contribution target
 * Fields
    - kind: TargetKind - kind
    - id: String - zone id, flow id, entity id, or episode id
    - version: Option<String> - version of the target it applies to, None when unknown
*/
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct TargetRef {
    pub(crate) kind: TargetKind,
    pub(crate) id: String,
    pub(crate) version: Option<String>,
}

/** A review decision
 * Fields
    - reviewer: Actor - reviewer
    - decision: ContributionStatus - accepted or rejected
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Review {
    pub(crate) reviewer: Actor,
    pub(crate) decision: ContributionStatus,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** A context contribution
 * Fields
    - contribution_id: String - stable identifier
    - schema_version: u32 - capture::SCHEMA_VERSION
    - target_ref: TargetRef - what it is about
    - contributor_id: String - who proposed it
    - contributor_type: ActorKind - human, agent, or system
    - episode_id: Option<String> - work episode it came from
    - contribution_type: ContributionType - what it offers
    - namespace: Namespace - classification
    - content: Value - structured payload (redacted)
    - evidence_refs: Vec<String> - supporting evidence
    - locality: Locality - task-local or reusable
    - valid_from: u64 - Unix milliseconds
    - valid_until: Option<u64> - expiry, None when open
    - status: ContributionStatus - lifecycle
    - reviews: Vec<Review> - review history
    - superseded_by: Option<String> - replacement
    - requires_crane_workflow: bool - true for constraint proposals: acceptance never changes a
      policy; a human must make the change through Crane
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContextContribution {
    pub(crate) contribution_id: String,
    pub(crate) schema_version: u32,
    pub(crate) target_ref: TargetRef,
    pub(crate) contributor_id: String,
    pub(crate) contributor_type: ActorKind,
    pub(crate) episode_id: Option<String>,
    pub(crate) contribution_type: ContributionType,
    pub(crate) namespace: Namespace,
    pub(crate) content: Value,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) locality: Locality,
    pub(crate) valid_from: u64,
    pub(crate) valid_until: Option<u64>,
    pub(crate) status: ContributionStatus,
    pub(crate) reviews: Vec<Review>,
    pub(crate) superseded_by: Option<String>,
    pub(crate) requires_crane_workflow: bool,
    pub(crate) created_at: u64,
}

/** A proposal submitted by an actor
 * Fields
    - target_ref: TargetRef - target
    - episode_id: Option<String> - work episode
    - contribution_type: ContributionType - type
    - namespace: Namespace - requested classification
    - content: Value - structured payload
    - evidence_refs: Vec<String> - supporting evidence
    - locality: Locality - task-local or reusable
    - valid_until: Option<u64> - expiry
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Proposal {
    pub(crate) target_ref: TargetRef,
    pub(crate) episode_id: Option<String>,
    pub(crate) contribution_type: ContributionType,
    pub(crate) namespace: Namespace,
    pub(crate) content: Value,
    pub(crate) evidence_refs: Vec<String>,
    pub(crate) locality: Locality,
    pub(crate) valid_until: Option<u64>,
}

/** Report whether a lifecycle transition is allowed
 * Input
    - from: ContributionStatus - current status
    - to: ContributionStatus - requested status
 * Output
    - bool
*/
#[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
pub(crate) fn transition_allowed(from: ContributionStatus, to: ContributionStatus) -> bool {
    use ContributionStatus::{Accepted, Expired, Proposed, Rejected, Superseded};
    matches!(
        (from, to),
        (Proposed, Accepted | Rejected | Superseded | Expired) | (Accepted, Superseded | Expired)
    )
}

/** Build a contribution error
 * Input
    - code: ErrorCode - category
    - message: &str - explanation
 * Output
    - CaptureError
*/
fn refuse(code: ErrorCode, message: &str) -> CaptureError {
    CaptureError::new(code, message)
}

/** Report whether a constraint proposal could weaken a policy. Only effects that can only make a
 * policy stricter or clearer (tighten, add, clarify) are allowed here; anything else, or no stated
 * effect, is treated as weakening. This check is advisory: an accepted proposal still goes through
 * Crane's signed policy workflow, which is the authority
 * Input
    - content: &Value - proposal content
 * Output
    - bool
*/
fn weakens_policy(content: &Value) -> bool {
    content
        .get("effect")
        .and_then(Value::as_str)
        .is_none_or(|effect| {
            !matches!(
                effect.to_ascii_lowercase().as_str(),
                "tighten" | "add" | "clarify"
            )
        })
}

/** Every contribution, by identifier
 * Fields
    - contributions: BTreeMap<String, ContextContribution> - contributions
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContributionLedger {
    pub(crate) contributions: BTreeMap<String, ContextContribution>,
}

impl ContributionLedger {
    /** Validate and record a proposal; it always starts as "proposed"
     * Input
        - actor: &Actor - proposing actor
        - proposal: Proposal - proposal
        - evidence_exists: &dyn Fn(&str) -> bool - whether an evidence record exists
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<String> contribution id
        - Error when the namespace is not the actor's to claim, evidence is missing, the content is
          empty or too large, or a constraint proposal would weaken a policy
    */
    pub(crate) fn propose(
        &mut self,
        actor: &Actor,
        mut proposal: Proposal,
        evidence_exists: &dyn Fn(&str) -> bool,
        now: u64,
    ) -> CaptureResult<String> {
        let allowed_namespace = match actor.kind {
            ActorKind::Agent => proposal.namespace == Namespace::AgentProposed,
            ActorKind::Human => proposal.namespace == Namespace::Observed,
            ActorKind::System => proposal.namespace == Namespace::SystemDerived,
        };
        if !allowed_namespace {
            return Err(refuse(
                ErrorCode::Unauthorized,
                "this actor cannot claim that namespace: agents propose in agent-proposed, humans in observed, the system in system-derived; human-approved is reached only by review",
            ));
        }
        if proposal.namespace == Namespace::Observed
            && proposal.contribution_type != ContributionType::Observation
        {
            return Err(refuse(
                ErrorCode::Unauthorized,
                "only observations can be classified as observed",
            ));
        }
        let needs_evidence = matches!(
            proposal.namespace,
            Namespace::Observed | Namespace::SystemDerived
        ) || matches!(
            proposal.contribution_type,
            ContributionType::Observation | ContributionType::RelationshipProposal
        );
        if needs_evidence && proposal.evidence_refs.is_empty() {
            return Err(refuse(
                ErrorCode::EvidenceRequired,
                "this contribution needs supporting evidence",
            ));
        }
        if let Some(missing) = proposal
            .evidence_refs
            .iter()
            .find(|id| !evidence_exists(id))
        {
            return Err(CaptureError::new(
                ErrorCode::EvidenceRequired,
                format!("evidence '{missing}' does not exist"),
            ));
        }
        if proposal.contribution_type == ContributionType::ConstraintProposal
            && proposal
                .content
                .get("policy")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err(refuse(
                ErrorCode::InvalidTransition,
                "a constraint proposal must name the policy it concerns (content.policy) and its effect (content.effect)",
            ));
        }
        if proposal.contribution_type == ContributionType::ConstraintProposal
            && weakens_policy(&proposal.content)
        {
            return Err(refuse(
                ErrorCode::PolicyChange,
                "a contribution cannot relax or remove a policy; policy changes go through Crane's signed policy workflow",
            ));
        }
        if proposal.locality == Locality::Reusable
            && proposal.target_ref.kind == TargetKind::WorkEpisode
        {
            return Err(refuse(
                ErrorCode::InvalidTransition,
                "a work episode can only receive task-local context",
            ));
        }
        redact_value(&mut proposal.content);
        let size = serde_json::to_string(&proposal.content).map_or(0, |text| text.len());
        if proposal.content.is_null() || size > MAX_CONTENT_BYTES {
            return Err(refuse(
                ErrorCode::InvalidTransition,
                "contribution content must be present and at most 16 KiB",
            ));
        }
        let created = now.to_string();
        let content = proposal.content.to_string();
        let id = stable_id(
            "ctb",
            &[&actor.id, &proposal.target_ref.id, &content, &created],
        );
        let contribution = ContextContribution {
            contribution_id: id.clone(),
            schema_version: SCHEMA_VERSION,
            target_ref: proposal.target_ref,
            contributor_id: actor.id.clone(),
            contributor_type: actor.kind,
            episode_id: proposal.episode_id,
            requires_crane_workflow: proposal.contribution_type
                == ContributionType::ConstraintProposal,
            contribution_type: proposal.contribution_type,
            namespace: proposal.namespace,
            content: proposal.content,
            evidence_refs: proposal.evidence_refs,
            locality: proposal.locality,
            valid_from: now,
            valid_until: proposal.valid_until,
            status: ContributionStatus::Proposed,
            reviews: Vec::new(),
            superseded_by: None,
            created_at: now,
        };
        self.contributions.insert(id.clone(), contribution);
        Ok(id)
    }

    /** Accept or reject a contribution. Only an authorized human who is not the contributor may
     * decide; accepting needs supporting evidence for observations, relationship and constraint
     * proposals; a policy-weakening proposal is never accepted. Acceptance classifies the
     * contribution as human-approved but never turns a hypothesis into an observation, and an
     * accepted constraint proposal still requires Crane's workflow
     * Input
        - reviewer: &AccessPolicy - reviewer and their permissions
        - id: &str - contribution
        - decision: ContributionStatus - Accepted or Rejected
        - reason: &str - why
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error for unauthorized reviewers, self-approval, invalid transitions, or missing evidence
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn review(
        &mut self,
        reviewer: &AccessPolicy,
        id: &str,
        decision: ContributionStatus,
        reason: &str,
        now: u64,
    ) -> CaptureResult<()> {
        let contribution = self.contributions.get_mut(id).ok_or_else(|| {
            CaptureError::new(ErrorCode::NotFound, format!("no contribution '{id}'"))
        })?;
        if reviewer.actor.kind != ActorKind::Human || !reviewer.may_review {
            return Err(refuse(
                ErrorCode::Unauthorized,
                "only an authorized human reviewer can accept or reject contributions",
            ));
        }
        if reviewer.actor.id == contribution.contributor_id {
            return Err(refuse(
                ErrorCode::SelfApproval,
                "a contributor cannot review their own contribution",
            ));
        }
        if !matches!(
            decision,
            ContributionStatus::Accepted | ContributionStatus::Rejected
        ) || !transition_allowed(contribution.status, decision)
        {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                format!(
                    "cannot move a {:?} contribution to {decision:?}",
                    contribution.status
                ),
            ));
        }
        if decision == ContributionStatus::Accepted {
            if contribution.contribution_type == ContributionType::ConstraintProposal
                && weakens_policy(&contribution.content)
            {
                return Err(refuse(
                    ErrorCode::PolicyChange,
                    "a policy-weakening proposal cannot be accepted here",
                ));
            }
            let needs_evidence = matches!(
                contribution.contribution_type,
                ContributionType::Observation
                    | ContributionType::RelationshipProposal
                    | ContributionType::ConstraintProposal
            );
            if needs_evidence && contribution.evidence_refs.is_empty() {
                return Err(refuse(
                    ErrorCode::EvidenceRequired,
                    "accepting this contribution needs supporting evidence",
                ));
            }
            contribution.namespace = Namespace::HumanApproved;
        }
        contribution.status = decision;
        contribution.reviews.push(Review {
            reviewer: reviewer.actor.clone(),
            decision,
            reason: reason.into(),
            at: now,
        });
        Ok(())
    }

    /** Mark a contribution superseded by a newer one about the same target; only its contributor
     * or an authorized human reviewer may do so
     * Input
        - actor: &AccessPolicy - acting actor
        - old: &str - contribution replaced
        - new: &str - replacement
     * Output
        - CaptureResult<()>
        - Error for unknown contributions, other targets, or invalid transitions
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn supersede(
        &mut self,
        actor: &AccessPolicy,
        old: &str,
        new: &str,
    ) -> CaptureResult<()> {
        let target = self
            .contributions
            .get(new)
            .map(|contribution| contribution.target_ref.clone())
            .ok_or_else(|| {
                CaptureError::new(ErrorCode::NotFound, format!("no contribution '{new}'"))
            })?;
        let contribution = self.contributions.get_mut(old).ok_or_else(|| {
            CaptureError::new(ErrorCode::NotFound, format!("no contribution '{old}'"))
        })?;
        let reviewer = actor.actor.kind == ActorKind::Human && actor.may_review;
        if actor.actor.id != contribution.contributor_id && !reviewer {
            return Err(refuse(
                ErrorCode::Unauthorized,
                "only the contributor or a reviewer can supersede a contribution",
            ));
        }
        if contribution.target_ref.kind != target.kind
            || contribution.target_ref.id != target.id
            || !transition_allowed(contribution.status, ContributionStatus::Superseded)
        {
            return Err(refuse(
                ErrorCode::InvalidTransition,
                "a contribution can only be superseded by one about the same target",
            ));
        }
        contribution.status = ContributionStatus::Superseded;
        contribution.superseded_by = Some(new.into());
        Ok(())
    }

    /** Expire proposed and accepted contributions past their validity
     * Input
        - now: u64 - Unix milliseconds
     * Output
        - usize contributions expired
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn expire(&mut self, now: u64) -> usize {
        let mut count = 0;
        for contribution in self.contributions.values_mut() {
            if contribution.valid_until.is_some_and(|until| until < now)
                && transition_allowed(contribution.status, ContributionStatus::Expired)
            {
                contribution.status = ContributionStatus::Expired;
                count += 1;
            }
        }
        count
    }
}
