// Evidence selection: eligibility first, ranking second. A record is eligible only when its
// source resource, repository, environment, evidence type, and sensitivity are permitted to the
// requesting actor, it belongs to the investigation's scope and time window, and the record budget
// still has room. Ranking (replaceable behind the Ranker interface) only orders eligible records;
// relevance never overrides authorization or scope.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::capture::format::record::{EvidenceRecord, EvidenceType, ParseStatus};
use crate::capture::resources::scope::TimeWindow;
use crate::capture::AccessPolicy;

/** Why a record was not selected
 * Variants
    - Unauthorized - the actor may not see its resource, repository, environment, or type
    - TooSensitive - above the actor's sensitivity ceiling
    - OutOfScope - another repository or episode than the investigation's
    - TypeNotRequested - an evidence type the request did not ask for
    - OutsideTimeWindow - observed outside the investigation's time window
    - OverBudget - eligible, but the record budget was full
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ExclusionReason {
    Unauthorized,
    TooSensitive,
    OutOfScope,
    TypeNotRequested,
    OutsideTimeWindow,
    OverBudget,
}

/** A record that was not selected
 * Fields
    - evidence_id: String - record
    - reason: ExclusionReason - why
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Exclusion {
    pub(crate) evidence_id: String,
    pub(crate) reason: ExclusionReason,
}

/** The eligibility criteria of a selection
 * Fields
    - policy: &'a AccessPolicy - requesting actor's permissions
    - repository: Option<&'a str> - investigation repository
    - episode_id: Option<&'a str> - investigation episode (records of other episodes are out)
    - types: &'a [EvidenceType] - requested types, empty for any
    - window: &'a TimeWindow - time window
    - max_records: usize - record budget
*/
#[derive(Debug, Clone, Copy)]
pub(crate) struct Criteria<'a> {
    pub(crate) policy: &'a AccessPolicy,
    pub(crate) repository: Option<&'a str>,
    pub(crate) episode_id: Option<&'a str>,
    pub(crate) types: &'a [EvidenceType],
    pub(crate) window: &'a TimeWindow,
    pub(crate) max_records: usize,
}

/** A selection
 * Fields
    - selected: Vec<String> - selected records, best first
    - excluded: Vec<Exclusion> - records left out, with reasons
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Selection {
    pub(crate) selected: Vec<String>,
    pub(crate) excluded: Vec<Exclusion>,
}

/** Orders eligible records (a replaceable strategy) */
pub(crate) trait Ranker {
    /** The strategy's name, recorded with the pack
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str;

    /** Score a record; higher ranks first
     * Input
        - record: &EvidenceRecord - record
     * Output
        - i64
    */
    fn score(&self, record: &EvidenceRecord) -> i64;
}

/** Deterministic ranking: delivery outcomes before code and context, parsed before partial, newer
 * before older; ties broken by evidence id */
pub(crate) struct DeterministicRanker;

impl Ranker for DeterministicRanker {
    /** Name
     * Input
        - None
     * Output
        - &'static str
    */
    fn name(&self) -> &'static str {
        "deterministic-v1"
    }

    /** Score by type priority, parse status, and recency
     * Input
        - record: &EvidenceRecord - record
     * Output
        - i64
    */
    fn score(&self, record: &EvidenceRecord) -> i64 {
        let priority: i64 = match record.evidence_type {
            EvidenceType::Deployment | EvidenceType::Verification => 9,
            EvidenceType::CiRun | EvidenceType::TestReport => 8,
            EvidenceType::RuntimeLog | EvidenceType::Metric => 7,
            EvidenceType::PullRequest | EvidenceType::CodeChange => 6,
            EvidenceType::Task => 5,
            EvidenceType::ToolCall | EvidenceType::AgentSession | EvidenceType::AgentEvent => 4,
            EvidenceType::Configuration | EvidenceType::Validation => 3,
            EvidenceType::GraphState | EvidenceType::Policy => 2,
        };
        let parsed: i64 = match record.parse_status {
            ParseStatus::Parsed => 2,
            ParseStatus::PartiallyParsed => 1,
            ParseStatus::Failed => 0,
        };
        let recency = (record.observed_at.unwrap_or(0) / 1000).min(9_999_999_999) as i64;
        priority * 100_000_000_000_000 + parsed * 10_000_000_000_000 + recency
    }
}

/** Check one record's eligibility
 * Input
    - record: &EvidenceRecord - record
    - criteria: &Criteria - criteria
 * Output
    - Option<ExclusionReason>, None when eligible
*/
fn ineligible(record: &EvidenceRecord, criteria: &Criteria) -> Option<ExclusionReason> {
    let policy = criteria.policy;
    if !policy.permits_resource(&record.source_ref.resource_id)
        || !policy.permits_type(record.evidence_type)
        || !policy.permits_environment(record.scope.environment.as_deref())
        || record
            .scope
            .repository
            .as_deref()
            .is_some_and(|repository| !policy.permits_repository(Some(repository)))
    {
        return Some(ExclusionReason::Unauthorized);
    }
    if record.sensitivity > policy.max_sensitivity {
        return Some(ExclusionReason::TooSensitive);
    }
    let other_repository = matches!((record.scope.repository.as_deref(), criteria.repository), (Some(found), Some(wanted)) if found != wanted);
    let other_episode = matches!((record.episode_id.as_deref(), criteria.episode_id), (Some(found), Some(wanted)) if found != wanted);
    if other_repository || other_episode {
        return Some(ExclusionReason::OutOfScope);
    }
    if !criteria.types.is_empty() && !criteria.types.contains(&record.evidence_type) {
        return Some(ExclusionReason::TypeNotRequested);
    }
    if let Some(at) = record.observed_at {
        let window = criteria.window;
        if window.since.is_some_and(|since| at < since)
            || window.until.is_some_and(|until| at > until)
        {
            return Some(ExclusionReason::OutsideTimeWindow);
        }
    }
    None
}

/** Select records: drop ineligible ones, rank the rest, and cut at the record budget
 * Input
    - records: &[&EvidenceRecord] - candidates
    - criteria: &Criteria - eligibility criteria
    - ranker: &dyn Ranker - ranking strategy
 * Output
    - Selection
*/
pub(crate) fn select(
    records: &[&EvidenceRecord],
    criteria: &Criteria,
    ranker: &dyn Ranker,
) -> Selection {
    let mut selection = Selection::default();
    let mut seen = BTreeSet::new();
    let mut eligible = Vec::new();
    for record in records {
        if !seen.insert(record.evidence_id.clone()) {
            continue;
        }
        match ineligible(record, criteria) {
            Some(reason) => selection.excluded.push(Exclusion {
                evidence_id: record.evidence_id.clone(),
                reason,
            }),
            None => eligible.push(*record),
        }
    }
    eligible.sort_by(|a, b| {
        ranker
            .score(b)
            .cmp(&ranker.score(a))
            .then_with(|| a.evidence_id.cmp(&b.evidence_id))
    });
    for (index, record) in eligible.into_iter().enumerate() {
        if index < criteria.max_records {
            selection.selected.push(record.evidence_id.clone());
        } else {
            selection.excluded.push(Exclusion {
                evidence_id: record.evidence_id.clone(),
                reason: ExclusionReason::OverBudget,
            });
        }
    }
    selection
}
