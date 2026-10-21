// Work episodes, questions, feedback, and outcomes. An episode is the unit of work evidence is
// collected for. Its final outcome is recorded explicitly and separately from tool or CI results:
// a successful tool call is not a successful task, and "succeeded" needs evidence. Questions are
// structured requests for evidence that can be routed to the broker only within their permitted
// sources. Feedback records whether an Evidence Pack was sufficient. Nothing here publishes a
// reusable capability.

use serde::{Deserialize, Serialize};

use super::contributions::TargetRef;
use crate::capture::broker::recipe::{Budget, RecipeRef};
use crate::capture::broker::run::{EvidenceQueryRequest, KnownRefs};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::pack::PackLimits;
use crate::capture::resources::scope::{TimeWindow, VersionedRef};
use crate::capture::{
    stable_id, Actor, CaptureError, CaptureResult, ErrorCode, Truth, SCHEMA_VERSION,
};

/** The final outcome of a work episode
 * Variants
    - Succeeded - the intended outcome was verified
    - Failed - the work failed
    - PartiallyVerified - some of the intended outcome was verified
    - Unknown - the outcome could not be established
    - Abandoned - the work was stopped
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EpisodeOutcome {
    Succeeded,
    Failed,
    PartiallyVerified,
    Unknown,
    Abandoned,
}

/** Whether an Evidence Pack served its requester
 * Fields
    - pack_id: String - pack
    - requester: Actor - who used it
    - sufficient: Truth - whether it was sufficient (unknown when not said)
    - additional_evidence: Vec<EvidenceType> - evidence that was still needed
    - notes: String - remarks
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PackFeedback {
    pub(crate) pack_id: String,
    pub(crate) requester: Actor,
    pub(crate) sufficient: Truth,
    pub(crate) additional_evidence: Vec<EvidenceType>,
    pub(crate) notes: String,
    pub(crate) at: u64,
}

/** A work episode
 * Fields
    - episode_id: String - stable identifier
    - schema_version: u32 - capture::SCHEMA_VERSION
    - task_id: Option<String> - task, None when unknown
    - title: String - what the work is
    - zones: Vec<VersionedRef> - Landscape zones involved
    - flows: Vec<VersionedRef> - Landscape flows involved
    - started_at: u64 - Unix milliseconds
    - ended_at: Option<u64> - when the outcome was recorded
    - outcome: Option<EpisodeOutcome> - final outcome, None while not recorded
    - outcome_evidence: Vec<String> - evidence for the outcome
    - outcome_recorded_by: Option<Actor> - who recorded it
    - packs: Vec<String> - Evidence Packs built for it
    - feedback: Vec<PackFeedback> - feedback on those packs
    - outcome_history: Vec<OutcomeChange> - outcomes recorded before the current one, oldest first
      (a changed outcome is kept, never overwritten; absent in episodes stored before)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct WorkEpisode {
    pub(crate) episode_id: String,
    pub(crate) schema_version: u32,
    pub(crate) task_id: Option<String>,
    pub(crate) title: String,
    pub(crate) zones: Vec<VersionedRef>,
    pub(crate) flows: Vec<VersionedRef>,
    pub(crate) started_at: u64,
    pub(crate) ended_at: Option<u64>,
    pub(crate) outcome: Option<EpisodeOutcome>,
    pub(crate) outcome_evidence: Vec<String>,
    pub(crate) outcome_recorded_by: Option<Actor>,
    pub(crate) packs: Vec<String>,
    pub(crate) feedback: Vec<PackFeedback>,
    #[serde(default)]
    pub(crate) outcome_history: Vec<OutcomeChange>,
}

/** An outcome that a later recording replaced
 * Fields
    - outcome: EpisodeOutcome - the outcome
    - evidence: Vec<String> - its evidence
    - recorded_by: Actor - who recorded it
    - at: u64 - when (Unix milliseconds)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OutcomeChange {
    pub(crate) outcome: EpisodeOutcome,
    pub(crate) evidence: Vec<String>,
    pub(crate) recorded_by: Actor,
    pub(crate) at: u64,
}

impl WorkEpisode {
    /** Open an episode
     * Input
        - task_id: Option<&str> - task
        - title: &str - what the work is
        - started_at: u64 - Unix milliseconds
     * Output
        - WorkEpisode
    */
    pub(crate) fn open(task_id: Option<&str>, title: &str, started_at: u64) -> Self {
        let started = started_at.to_string();
        Self {
            episode_id: episode_id(task_id, title, &started),
            schema_version: SCHEMA_VERSION,
            task_id: task_id.map(str::to_string),
            title: title.into(),
            zones: Vec::new(),
            flows: Vec::new(),
            started_at,
            ended_at: None,
            outcome: None,
            outcome_evidence: Vec::new(),
            outcome_recorded_by: None,
            packs: Vec::new(),
            feedback: Vec::new(),
            outcome_history: Vec::new(),
        }
    }

    /** Record the final outcome; succeeded and partially verified need evidence, and a recorded
     * outcome can only be replaced by its recorder or by a human reviewer (a human with the right
     * to review); the replaced outcome is kept in outcome_history. Callers outside Work Capture go
     * through CaptureStore::record_outcome, which also checks that the cited evidence exists and
     * belongs to the episode
     * Input
        - actor: &Actor - recorder
        - may_review: bool - whether the recorder may review
        - outcome: EpisodeOutcome - outcome
        - evidence: Vec<String> - evidence for it
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error when evidence is missing or someone else's outcome would be changed by anyone but a
          human reviewer
    */
    pub(in crate::capture) fn record_outcome(
        &mut self,
        actor: &Actor,
        may_review: bool,
        outcome: EpisodeOutcome,
        evidence: Vec<String>,
        now: u64,
    ) -> CaptureResult<()> {
        if matches!(
            outcome,
            EpisodeOutcome::Succeeded | EpisodeOutcome::PartiallyVerified
        ) && evidence.is_empty()
        {
            return Err(CaptureError::new(
                ErrorCode::EvidenceRequired,
                "a verified outcome needs evidence",
            ));
        }
        if let Some(previous) = &self.outcome_recorded_by {
            let reviewer = actor.kind == crate::capture::ActorKind::Human && may_review;
            if previous.id != actor.id && !reviewer {
                return Err(CaptureError::new(
                    ErrorCode::Unauthorized,
                    "only the recorder or a human reviewer can change a recorded outcome",
                ));
            }
        }
        if let (Some(previous), Some(recorded_by)) =
            (self.outcome, self.outcome_recorded_by.clone())
        {
            self.outcome_history.push(OutcomeChange {
                outcome: previous,
                evidence: std::mem::take(&mut self.outcome_evidence),
                recorded_by,
                at: self.ended_at.unwrap_or_default(),
            });
        }
        self.outcome = Some(outcome);
        self.outcome_evidence = evidence;
        self.outcome_recorded_by = Some(actor.clone());
        self.ended_at = Some(now);
        Ok(())
    }
}

/** The stable identifier of an episode: from the task when known
 * Input
    - task_id: Option<&str> - task
    - title: &str - title (used when the task is unknown)
    - started: &str - start time (used when the task is unknown)
 * Output
    - String
*/
pub(crate) fn episode_id(task_id: Option<&str>, title: &str, started: &str) -> String {
    match task_id {
        Some(task) => stable_id("epi", &["task", task]),
        None => stable_id("epi", &["untracked", title, started]),
    }
}

/** Resolution state of a question
 * Variants
    - Open - not yet answered
    - Answered - an Evidence Pack answered it
    - Blocked - its permitted sources could not answer (unavailable or denied)
    - Unresolvable - the required evidence cannot exist or be obtained
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum QuestionState {
    Open,
    Answered,
    Blocked,
    Unresolvable,
}

/** A structured question
 * Fields
    - question_id: String - stable identifier
    - text: String - question
    - target: TargetRef - entity, flow, zone, or episode it is about
    - reason: String - why it is asked
    - relevant_evidence: Vec<String> - evidence already known
    - required_evidence: Vec<EvidenceType> - evidence that would resolve it
    - permitted_resources: Vec<String> - the only resources it may query
    - recipe: Option<RecipeRef> - recipe to use, None for an ad-hoc bounded plan
    - state: QuestionState - resolution state
    - asked_by: Actor - who asked
    - answer_pack: Option<String> - pack that answered it
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Question {
    pub(crate) question_id: String,
    pub(crate) text: String,
    pub(crate) target: TargetRef,
    pub(crate) reason: String,
    pub(crate) relevant_evidence: Vec<String>,
    pub(crate) required_evidence: Vec<EvidenceType>,
    pub(crate) permitted_resources: Vec<String>,
    pub(crate) recipe: Option<RecipeRef>,
    pub(crate) state: QuestionState,
    pub(crate) asked_by: Actor,
    pub(crate) answer_pack: Option<String>,
    pub(crate) created_at: u64,
}

impl Question {
    /** Route the question to the broker: the request is confined to the question's permitted
     * resources and evidence types, so asking grants no unrestricted retrieval
     * Input
        - episode_id: Option<&str> - work episode
        - task_id: Option<&str> - task
        - known: KnownRefs - known references
        - budget: Budget - retrieval budget
     * Output
        - CaptureResult<EvidenceQueryRequest>
        - Error when the question names no permitted resource
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn to_request(
        &self,
        episode_id: Option<&str>,
        task_id: Option<&str>,
        known: KnownRefs,
        budget: Budget,
    ) -> CaptureResult<EvidenceQueryRequest> {
        if self.permitted_resources.is_empty() {
            return Err(CaptureError::new(
                ErrorCode::InvalidQuery,
                "a question must name the resources it may query",
            ));
        }
        Ok(EvidenceQueryRequest {
            episode_id: episode_id.map(str::to_string),
            task_id: task_id.map(str::to_string),
            question: self.text.clone(),
            investigation_class: "question".into(),
            recipe: self.recipe.clone(),
            plan: None,
            known,
            resources: self.permitted_resources.clone(),
            required_evidence: self.required_evidence.clone(),
            evidence_types: self.required_evidence.clone(),
            time_window: TimeWindow::default(),
            budget,
            freshness_ms: None,
            pack_limits: PackLimits::default(),
            plan_required: Vec::new(),
        })
    }
}
