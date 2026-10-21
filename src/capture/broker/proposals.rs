// Recipe proposals: recipes learned from work instead of written by hand. A proposal carries a
// complete recipe (validated like every recipe), the reason it was proposed, and the Evidence Packs
// it was learned from. It starts proposed and is decided once, by an authorized human who did not
// propose it; only an accepted proposal enters the RecipeBook, as a new version that never changes
// afterwards. Nothing learned changes how Work Capture retrieves evidence until a person accepts it.

use serde::{Deserialize, Serialize};

use super::recipe::{EvidenceRecipe, EXPLORATION_PREFIX};
use crate::capture::{
    stable_id, AccessPolicy, Actor, ActorKind, CaptureError, CaptureResult, ErrorCode,
    SCHEMA_VERSION,
};

/** Largest rationale of a proposal, in characters */
const MAX_RATIONALE_CHARS: usize = 2000;

/** Lifecycle of a recipe proposal
 * Variants
    - Proposed - awaiting review
    - Accepted - accepted by a reviewer; its recipe is in the RecipeBook
    - Rejected - rejected by a reviewer (final)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ProposalStatus {
    Proposed,
    Accepted,
    Rejected,
}

/** A review decision on a recipe proposal
 * Fields
    - reviewer: Actor - reviewer
    - decision: ProposalStatus - accepted or rejected
    - reason: String - why
    - at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecipeDecision {
    pub(crate) reviewer: Actor,
    pub(crate) decision: ProposalStatus,
    pub(crate) reason: String,
    pub(crate) at: u64,
}

/** A proposed recipe version
 * Fields
    - proposal_id: String - stable identifier
    - schema_version: u32 - capture::SCHEMA_VERSION
    - recipe: EvidenceRecipe - the complete recipe it would publish
    - recipe_digest: String - the recipe's digest
    - rationale: String - why it is proposed
    - basis: Vec<String> - Evidence Packs it was learned from
    - pattern_id: Option<String> - the insight pattern it came from, if any
    - proposed_by: Actor - proposer
    - status: ProposalStatus - lifecycle
    - reviews: Vec<RecipeDecision> - review history
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecipeProposal {
    pub(crate) proposal_id: String,
    pub(crate) schema_version: u32,
    pub(crate) recipe: EvidenceRecipe,
    pub(crate) recipe_digest: String,
    pub(crate) rationale: String,
    pub(crate) basis: Vec<String>,
    pub(crate) pattern_id: Option<String>,
    pub(crate) proposed_by: Actor,
    pub(crate) status: ProposalStatus,
    pub(crate) reviews: Vec<RecipeDecision>,
    pub(crate) created_at: u64,
}

impl RecipeProposal {
    /** Build a proposal: the recipe must be valid, serve at least one question class, and be a
     * publishable recipe (not an exploration or a plan); the rationale is bounded
     * Input
        - proposer: &Actor - proposer
        - recipe: EvidenceRecipe - recipe
        - rationale: &str - why
        - basis: Vec<String> - Evidence Packs it was learned from
        - pattern_id: Option<String> - insight pattern
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<RecipeProposal>
        - Error (InvalidRecipe) when the recipe cannot be published as proposed
    */
    pub(crate) fn new(
        proposer: &Actor,
        recipe: EvidenceRecipe,
        rationale: &str,
        basis: Vec<String>,
        pattern_id: Option<String>,
        now: u64,
    ) -> CaptureResult<Self> {
        recipe.validate()?;
        let invalid = |message: &str| Err(CaptureError::new(ErrorCode::InvalidRecipe, message));
        if recipe.investigation_classes.is_empty() {
            return invalid("a proposed recipe must serve at least one question class");
        }
        if recipe.recipe_id.starts_with(EXPLORATION_PREFIX) || recipe.recipe_id.starts_with("plan-")
        {
            return invalid("an exploration or a plan is not a publishable recipe id");
        }
        if recipe.version == 0 {
            return invalid("a published recipe version starts at 1");
        }
        let rationale = rationale
            .chars()
            .take(MAX_RATIONALE_CHARS)
            .collect::<String>();
        let digest = recipe.digest();
        Ok(Self {
            proposal_id: stable_id(
                "rcp",
                &[
                    &recipe.recipe_id,
                    &recipe.version.to_string(),
                    &digest,
                    &proposer.id,
                ],
            ),
            schema_version: SCHEMA_VERSION,
            recipe,
            recipe_digest: digest,
            rationale,
            basis,
            pattern_id,
            proposed_by: proposer.clone(),
            status: ProposalStatus::Proposed,
            reviews: Vec::new(),
            created_at: now,
        })
    }

    /** Decide a proposal: only an authorized human who did not propose it, and only once
     * Input
        - reviewer: &AccessPolicy - reviewer and their permissions
        - decision: ProposalStatus - Accepted or Rejected
        - reason: &str - why
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error for unauthorized reviewers, self-approval, or a proposal already decided
    */
    pub(crate) fn decide(
        &mut self,
        reviewer: &AccessPolicy,
        decision: ProposalStatus,
        reason: &str,
        now: u64,
    ) -> CaptureResult<()> {
        if reviewer.actor.kind != ActorKind::Human || !reviewer.may_review {
            return Err(CaptureError::new(
                ErrorCode::Unauthorized,
                "only an authorized human reviewer can accept or reject a recipe",
            ));
        }
        if reviewer.actor.id == self.proposed_by.id {
            return Err(CaptureError::new(
                ErrorCode::SelfApproval,
                "a proposer cannot review their own recipe",
            ));
        }
        if self.status != ProposalStatus::Proposed || decision == ProposalStatus::Proposed {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                format!("cannot move a {:?} proposal to {decision:?}", self.status),
            ));
        }
        if reason.trim().is_empty() {
            return Err(CaptureError::new(
                ErrorCode::InvalidTransition,
                "a review needs a reason",
            ));
        }
        self.status = decision;
        self.reviews.push(RecipeDecision {
            reviewer: reviewer.actor.clone(),
            decision,
            reason: reason.into(),
            at: now,
        });
        Ok(())
    }
}
