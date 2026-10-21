// From a pattern to a proposal, always through a review step that already exists. A question
// pattern whose explorations people judged sufficient becomes a recipe proposal: the steps that
// found evidence in those explorations, requiring what every one of them found plus what the
// feedback said was still needed. A zone pattern becomes system-derived context for the zone, which
// the contribution ledger keeps proposed until a person reviews it. Other patterns name where a
// person can act (compile a capability, decide a zone's autonomy) and propose nothing themselves.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::json;

use super::{Destination, Pattern, MINERS_VERSION};
use crate::capture::broker::proposals::{ProposalStatus, RecipeProposal};
use crate::capture::broker::recipe::{
    Budget, EvidenceRecipe, Predicate, RecipeStep, RequiredEvidence, StoppingRules,
};
use crate::capture::broker::run::{EvidenceQueryBroker, StepState};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::contributions::{
    ContributionType, Locality, Namespace, Proposal, TargetKind, TargetRef,
};
use crate::capture::{Actor, ActorKind, CaptureError, CaptureResult, ErrorCode};

/** Most records a step of a learned recipe retrieves (as the built-in recipes) */
const LEARNED_STEP_LIMIT: usize = 50;

/** What a pattern was turned into
 * Variants
    - Recipe(String) - a recipe proposal id
    - Context(String) - a contribution id
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Proposed {
    Recipe(String),
    Context(String),
}

/** The actor Insight proposes as: the system, so a person can review what it proposes
 * Input
    - None
 * Output
    - Actor
*/
pub(crate) fn insight_actor() -> Actor {
    Actor::new("crane-insight", ActorKind::System)
}

/** Turn a pattern into a proposal (kept in the broker's store, not yet saved)
 * Input
    - broker: &mut EvidenceQueryBroker - broker with its store and RecipeBook
    - pattern: &Pattern - pattern
    - now: u64 - Unix milliseconds
 * Output
    - CaptureResult<Proposed>
    - Error when the pattern leads to a person rather than a proposal, or the proposal is refused
*/
pub(crate) fn propose(
    broker: &mut EvidenceQueryBroker,
    pattern: &Pattern,
    now: u64,
) -> CaptureResult<Proposed> {
    match &pattern.destination {
        Destination::Recipe { class } => {
            let recipe = learn_recipe(broker, pattern, class)?;
            let proposal = RecipeProposal::new(
                &insight_actor(),
                recipe,
                &pattern.statement,
                pattern.packs.clone(),
                Some(pattern.pattern_id.clone()),
                now,
            )?;
            broker.store.propose_recipe(proposal).map(Proposed::Recipe)
        }
        Destination::Context { zone, version } => {
            let proposal = Proposal {
                target_ref: TargetRef {
                    kind: TargetKind::Zone,
                    id: zone.clone(),
                    version: version.clone(),
                },
                episode_id: None,
                contribution_type: ContributionType::Observation,
                namespace: Namespace::SystemDerived,
                content: json!({
                    "insight": pattern.statement,
                    "pattern_id": pattern.pattern_id,
                    "miner": pattern.miner,
                    "miners_version": MINERS_VERSION,
                    "counts": pattern.counts,
                    "baseline": pattern.baseline,
                }),
                evidence_refs: pattern.evidence.clone(),
                locality: Locality::Reusable,
                valid_until: None,
            };
            broker
                .store
                .propose(&insight_actor(), proposal, now)
                .map(Proposed::Context)
        }
        Destination::Capability { episode } => Err(CaptureError::new(
            ErrorCode::InvalidTransition,
            format!("this pattern is a capability candidate: compile it from work episode {episode} with 'crane compile episode', which then goes through the lab and the registry's gates"),
        )),
        Destination::Governance => Err(CaptureError::new(
            ErrorCode::InvalidTransition,
            "this pattern is for the repository owner or an admin to decide (agent permissions for the zone); Insight proposes no permission change",
        )),
        Destination::Report => Err(CaptureError::new(
            ErrorCode::InvalidTransition,
            "this pattern is for people to read and act on; it leads to no proposal",
        )),
    }
}

/** A recipe identifier for a question class: "learned-" and the class, in identifier characters
 * Input
    - class: &str - question class
 * Output
    - String
*/
fn learned_id(class: &str) -> String {
    let slug = class
        .to_ascii_lowercase()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    format!("learned-{slug}").chars().take(64).collect()
}

/** Learn a recipe from the explorations a question pattern rests on: every step that found
 * evidence in at least one of them (with the step's own filters), requiring the evidence types
 * every one of them found and those the feedback said were still needed. The version follows the
 * versions already proposed or published under the same id
 * Input
    - broker: &EvidenceQueryBroker - broker (store and RecipeBook)
    - pattern: &Pattern - question pattern
    - class: &str - question class
 * Output
    - CaptureResult<EvidenceRecipe>
    - Error when the explorations found nothing to learn
*/
fn learn_recipe(
    broker: &EvidenceQueryBroker,
    pattern: &Pattern,
    class: &str,
) -> CaptureResult<EvidenceRecipe> {
    let store = &broker.store;
    let mut steps: BTreeMap<String, RecipeStep> = BTreeMap::new();
    let mut found_in_every: Option<BTreeSet<EvidenceType>> = None;
    let mut still_needed = BTreeSet::new();
    for pack in pattern.packs.iter().filter_map(|id| store.packs.get(id)) {
        let Some(audit) = store.audit(&pack.retrieval.query_id) else {
            continue;
        };
        let Some(explored) = audit
            .recipe
            .as_ref()
            .filter(|recipe| recipe.is_exploration())
        else {
            continue;
        };
        for record in &audit.steps {
            if record.state != StepState::Executed || record.record_ids.is_empty() {
                continue;
            }
            if let Some(step) = explored
                .steps
                .iter()
                .find(|step| step.step_id == record.step_id)
            {
                steps
                    .entry(step.step_id.clone())
                    .or_insert_with(|| RecipeStep {
                        limit: LEARNED_STEP_LIMIT,
                        ..step.clone()
                    });
            }
        }
        let found = pack
            .evidence
            .iter()
            .map(|item| item.evidence_type)
            .collect::<BTreeSet<_>>();
        found_in_every = Some(match found_in_every {
            Some(every) => every.intersection(&found).copied().collect(),
            None => found,
        });
        if let Some(episode) = pack
            .episode_id
            .as_ref()
            .and_then(|id| store.episodes.get(id))
        {
            for feedback in episode
                .feedback
                .iter()
                .filter(|feedback| feedback.pack_id == pack.pack_id)
            {
                still_needed.extend(feedback.additional_evidence.iter().copied());
            }
        }
    }
    if steps.is_empty() {
        return Err(CaptureError::new(
            ErrorCode::InvalidRecipe,
            format!("the explorations of '{class}' found no evidence to learn a recipe from"),
        ));
    }
    let starting_conditions = steps
        .values()
        .flat_map(|step| step.filters.values())
        .filter_map(|template| {
            template
                .strip_prefix('{')
                .and_then(|rest| rest.strip_suffix('}'))
        })
        .map(str::to_string)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut required = found_in_every
        .unwrap_or_default()
        .into_iter()
        .map(|evidence_type| RequiredEvidence {
            evidence_type,
            description: "found by every exploration people judged sufficient".into(),
            when: Predicate::Always,
        })
        .collect::<Vec<_>>();
    for evidence_type in still_needed {
        if !required
            .iter()
            .any(|item| item.evidence_type == evidence_type)
        {
            required.push(RequiredEvidence {
                evidence_type,
                description: "still needed, according to the people who judged the explorations"
                    .into(),
                when: Predicate::Always,
            });
        }
    }
    let recipe_id = learned_id(class);
    let published = broker
        .recipes
        .all()
        .filter(|recipe| recipe.recipe_id == recipe_id)
        .map(|recipe| recipe.version);
    let proposed = store
        .recipe_proposals
        .values()
        .filter(|proposal| {
            proposal.recipe.recipe_id == recipe_id && proposal.status != ProposalStatus::Rejected
        })
        .map(|proposal| proposal.recipe.version);
    let version = published.chain(proposed).max().unwrap_or(0) + 1;
    Ok(EvidenceRecipe {
        recipe_id,
        version,
        description: format!(
            "Learned from {} explorations of '{class}' that people judged sufficient",
            pattern.packs.len()
        ),
        investigation_classes: vec![class.to_string()],
        starting_conditions,
        steps: steps.into_values().collect(),
        required,
        stopping: StoppingRules {
            stop_when_satisfied: false,
        },
        limits: Budget {
            max_steps: 24,
            max_records: 500,
            max_payload_bytes: 2 * 1024 * 1024,
            max_time_ms: 60_000,
        },
    })
}
