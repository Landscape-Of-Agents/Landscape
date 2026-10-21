// Insight (component 8): what the system learns across work, not within one investigation. It
// reads Work Capture's store (packs, audits, episodes with their outcomes and feedback, evidence)
// through the viewer's access policy and finds patterns with deterministic, versioned miners:
//
//   read      - the read model: only packs, audits, episodes, and evidence the viewer may see
//   miners    - evidence gaps, unanswered questions, trust, outcome co-occurrences, and recurring
//               agent tool sequences; every pattern carries its counts, its denominator, the
//               unknowns beside them, and the episodes, packs, and evidence it rests on
//   propose   - turns a pattern into a proposal through an existing review step: a recipe
//               proposal (Work Capture's recipe ledger) or system-derived context for a zone
//               (the contribution ledger)
//   store     - the kept insight reports (runtime/insight, outside the repository)
//
// Insight writes nothing it reads. A pattern is a co-occurrence with counts, never a cause; a count
// below the minimum support is not reported, so a pattern cannot single out one hidden record; and
// nothing a pattern says changes a recipe, a context, a capability, or a policy until a person
// accepts the proposal made from it.

pub(crate) mod miners;
pub(crate) mod propose;
pub(crate) mod read;
pub(crate) mod store;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::capture::store::CaptureStore;
use crate::capture::{stable_id, AccessPolicy, Actor, SCHEMA_VERSION};
use crate::trust::crypto::digest_json;

/** Version of the miners: a report names it, so a pattern can be reproduced */
pub(crate) const MINERS_VERSION: u32 = 1;

/** The smallest count a pattern may rest on; smaller ones are suppressed, never reported */
pub(crate) const MIN_SUPPORT: usize = 3;

/** Explorations people must judge sufficient before a recipe is proposed for their question */
pub(crate) const LEARN_SUPPORT: usize = 2;

/** How far (percentage points) a feature's failure rate must exceed the overall rate */
pub(crate) const OUTCOME_MARGIN_PERCENT: usize = 20;

/** Most evidence ids a pattern cites */
pub(crate) const MAX_CITED: usize = 10;

/** What a pattern is about
 * Variants
    - EvidenceGap - evidence investigations of a zone keep lacking
    - Question - a question class no recipe serves, and how explorations of it were judged
    - Trust - how often investigations of a zone were sufficient, with the outcomes of its work
    - Outcome - a feature (zone or directory) whose work fails more often than work overall
    - Work - an agent tool sequence that recurs across work episodes
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PatternKind {
    EvidenceGap,
    Question,
    Trust,
    Outcome,
    Work,
}

/** Where a pattern can lead, always through an existing review step
 * Variants
    - Context { zone, version } - system-derived context for the zone (reviewed in the
      contribution ledger)
    - Recipe { class } - a recipe for the question class (reviewed in the recipe ledger)
    - Capability { episode } - a capability candidate compiled from the episode ('crane compile
      episode'), then the lab and the registry's gates
    - Governance - a decision for the repository owner or an admin (agent permissions for a zone)
    - Report - for people to read and act on
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "to", rename_all = "snake_case")]
pub(crate) enum Destination {
    Context {
        zone: String,
        version: Option<String>,
    },
    Recipe {
        class: String,
    },
    Capability {
        episode: String,
    },
    Governance,
    Report,
}

/** The counts a pattern rests on
 * Fields
    - matching: usize - cases showing the pattern
    - total: usize - cases considered (the denominator)
    - unknown: usize - cases that could not be judged (never counted as matching or not)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Counts {
    pub(crate) matching: usize,
    pub(crate) total: usize,
    pub(crate) unknown: usize,
}

/** A pattern found across work
 * Fields
    - pattern_id: String - stable identifier (miner, version, subject, and counts)
    - kind: PatternKind - what it is about
    - miner: String - the miner that found it
    - subject: String - the zone, class, directory, or tool sequence
    - statement: String - the pattern in words, with its counts
    - counts: Counts - the counts it rests on
    - baseline: Option<Counts> - the counts it is compared with, if any
    - episodes: Vec<String> - work episodes it rests on
    - packs: Vec<String> - Evidence Packs it rests on
    - evidence: Vec<String> - evidence it cites (only evidence the viewer may see)
    - destination: Destination - where it can lead
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pattern {
    pub(crate) pattern_id: String,
    pub(crate) kind: PatternKind,
    pub(crate) miner: String,
    pub(crate) subject: String,
    pub(crate) statement: String,
    pub(crate) counts: Counts,
    pub(crate) baseline: Option<Counts>,
    pub(crate) episodes: Vec<String>,
    pub(crate) packs: Vec<String>,
    pub(crate) evidence: Vec<String>,
    pub(crate) destination: Destination,
}

impl Pattern {
    /** Build a pattern; its identifier comes from its miner, subject, and statement
     * Input
        - kind: PatternKind - what it is about
        - miner: &str - miner
        - subject: String - subject
        - statement: String - the pattern in words
        - counts: Counts - counts
     * Output
        - Pattern
    */
    pub(crate) fn new(
        kind: PatternKind,
        miner: &str,
        subject: String,
        statement: String,
        counts: Counts,
    ) -> Self {
        Self {
            pattern_id: stable_id(
                "pat",
                &[miner, &MINERS_VERSION.to_string(), &subject, &statement],
            ),
            kind,
            miner: miner.into(),
            subject,
            statement,
            counts,
            baseline: None,
            episodes: Vec::new(),
            packs: Vec::new(),
            evidence: Vec::new(),
            destination: Destination::Report,
        }
    }
}

/** What a report considered
 * Fields
    - packs: usize - Evidence Packs the viewer may see
    - audits: usize - investigations the viewer may see
    - episodes: usize - work episodes the viewer may see
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Considered {
    pub(crate) packs: usize,
    pub(crate) audits: usize,
    pub(crate) episodes: usize,
}

/** An insight report
 * Fields
    - report_id: String - stable identifier
    - schema_version: u32 - capture::SCHEMA_VERSION
    - miners_version: u32 - MINERS_VERSION
    - viewer: Actor - whose access policy it was computed under
    - generated_at: u64 - Unix milliseconds
    - considered: Considered - what was read
    - patterns: Vec<Pattern> - patterns found, by kind and subject
    - suppressed: usize - candidate patterns below the minimum support, not reported
    - digest: String - digest of what was considered and found
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct InsightReport {
    pub(crate) report_id: String,
    pub(crate) schema_version: u32,
    pub(crate) miners_version: u32,
    pub(crate) viewer: Actor,
    pub(crate) generated_at: u64,
    pub(crate) considered: Considered,
    pub(crate) patterns: Vec<Pattern>,
    pub(crate) suppressed: usize,
    pub(crate) digest: String,
}

impl InsightReport {
    /** Check the report against its digest
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn verifies(&self) -> bool {
        report_digest(&self.considered, &self.patterns, self.suppressed) == self.digest
    }
}

/** The digest of a report's content
 * Input
    - considered: &Considered - what was read
    - patterns: &[Pattern] - patterns
    - suppressed: usize - suppressed candidates
 * Output
    - String
*/
fn report_digest(considered: &Considered, patterns: &[Pattern], suppressed: usize) -> String {
    digest_json(
        "insight/report/v1",
        &json!({
            "miners_version": MINERS_VERSION,
            "considered": considered,
            "patterns": patterns,
            "suppressed": suppressed,
        }),
    )[..32]
        .to_string()
}

/** Mine the store under a viewer's access policy
 * Input
    - store: &CaptureStore - Work Capture's store
    - policy: &AccessPolicy - the viewer's permissions
    - served: &BTreeSet<String> - question classes a recipe serves
    - now: u64 - Unix milliseconds
 * Output
    - InsightReport
*/
pub(crate) fn report(
    store: &CaptureStore,
    policy: &AccessPolicy,
    served: &BTreeSet<String>,
    now: u64,
) -> InsightReport {
    let model = read::ReadModel::new(store, policy);
    let mined = miners::mine(&model, served);
    let considered = Considered {
        packs: model.packs.len(),
        audits: model.audits.len(),
        episodes: model.episodes.len(),
    };
    let digest = report_digest(&considered, &mined.patterns, mined.suppressed);
    InsightReport {
        report_id: stable_id("inr", &[&policy.actor.id, &now.to_string(), &digest]),
        schema_version: SCHEMA_VERSION,
        miners_version: MINERS_VERSION,
        viewer: policy.actor.clone(),
        generated_at: now,
        considered,
        patterns: mined.patterns,
        suppressed: mined.suppressed,
        digest,
    }
}
