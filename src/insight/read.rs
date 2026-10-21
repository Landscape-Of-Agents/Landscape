// The read model of Insight: Work Capture's store as one viewer may see it. A pack, an
// investigation, or an episode is visible when its repository and environment are the viewer's to
// see; a piece of evidence is visible when Work Capture's own eligibility check would give it to
// the viewer. Everything a miner counts or cites passes through here, so a pattern is never built
// from what the viewer could not open themselves.

use std::collections::BTreeMap;

use crate::capture::broker::run::QueryAuditRecord;
use crate::capture::format::record::{EvidenceRecord, EvidenceType};
use crate::capture::ingest::episode::{PackFeedback, WorkEpisode};
use crate::capture::ingest::pack::{EvidencePack, InvestigationStatus};
use crate::capture::ingest::selection::withheld;
use crate::capture::store::CaptureStore;
use crate::capture::{AccessPolicy, Truth};

/** What one viewer may read of the store
 * Fields
    - store: &CaptureStore - the store
    - policy: &AccessPolicy - the viewer's permissions
    - packs: Vec<&EvidencePack> - visible packs, by id
    - audits: Vec<&QueryAuditRecord> - visible investigations, in order
    - episodes: BTreeMap<String, &WorkEpisode> - episodes with at least one visible pack
*/
pub(crate) struct ReadModel<'a> {
    store: &'a CaptureStore,
    policy: &'a AccessPolicy,
    pub(crate) packs: Vec<&'a EvidencePack>,
    pub(crate) audits: Vec<&'a QueryAuditRecord>,
    pub(crate) episodes: BTreeMap<String, &'a WorkEpisode>,
}

impl<'a> ReadModel<'a> {
    /** Build the read model of a viewer
     * Input
        - store: &CaptureStore - the store
        - policy: &AccessPolicy - the viewer's permissions
     * Output
        - ReadModel
    */
    pub(crate) fn new(store: &'a CaptureStore, policy: &'a AccessPolicy) -> Self {
        let in_scope = |repository: Option<&str>, environment: Option<&str>| {
            repository.is_some()
                && policy.permits_repository(repository)
                && policy.permits_environment(environment)
        };
        let packs = store
            .packs
            .values()
            .filter(|pack| {
                in_scope(
                    pack.scope.repository.as_deref(),
                    pack.scope.environment.as_deref(),
                )
            })
            .collect::<Vec<_>>();
        let audits = store
            .audits
            .iter()
            .filter(|audit| {
                in_scope(
                    audit
                        .scope
                        .repository
                        .as_deref()
                        .or(audit.request.known.repository.as_deref()),
                    audit.scope.environment.as_deref().or(audit
                        .request
                        .known
                        .environment
                        .as_deref()),
                )
            })
            .collect::<Vec<_>>();
        let episodes = packs
            .iter()
            .filter_map(|pack| pack.episode_id.as_ref())
            .filter_map(|id| store.episodes.get(id).map(|episode| (id.clone(), episode)))
            .collect();
        Self {
            store,
            policy,
            packs,
            audits,
            episodes,
        }
    }

    /** A piece of evidence, when the viewer may receive it
     * Input
        - id: &str - evidence id
     * Output
        - Option<&EvidenceRecord>
    */
    pub(crate) fn evidence(&self, id: &str) -> Option<&'a EvidenceRecord> {
        self.store
            .evidence
            .get(id)
            .filter(|record| withheld(record, self.policy).is_none())
    }

    /** The visible evidence of a pack, optionally of one type
     * Input
        - pack: &EvidencePack - pack
        - evidence_type: Option<EvidenceType> - type, None for every type
     * Output
        - Vec<&EvidenceRecord>
    */
    pub(crate) fn pack_evidence(
        &self,
        pack: &EvidencePack,
        evidence_type: Option<EvidenceType>,
    ) -> Vec<&'a EvidenceRecord> {
        pack.evidence
            .iter()
            .filter(|item| evidence_type.is_none_or(|kind| item.evidence_type == kind))
            .filter_map(|item| self.evidence(&item.evidence_id))
            .collect()
    }

    /** The visible packs of an episode
     * Input
        - episode: &WorkEpisode - episode
     * Output
        - Vec<&EvidencePack>
    */
    pub(crate) fn episode_packs(&self, episode: &WorkEpisode) -> Vec<&'a EvidencePack> {
        self.packs
            .iter()
            .copied()
            .filter(|pack| pack.episode_id.as_deref() == Some(episode.episode_id.as_str()))
            .collect()
    }

    /** A visible pack by id
     * Input
        - id: &str - pack id
     * Output
        - Option<&EvidencePack>
    */
    pub(crate) fn pack(&self, id: &str) -> Option<&'a EvidencePack> {
        self.packs.iter().copied().find(|pack| pack.pack_id == id)
    }

    /** What people said about a pack: the latest judgement of each person (a later one replaces
     * that person's earlier one; different people are counted apart)
     * Input
        - pack: &EvidencePack - pack
     * Output
        - Vec<&PackFeedback>
    */
    pub(crate) fn feedback(&self, pack: &EvidencePack) -> Vec<&'a PackFeedback> {
        let Some(episode) = pack
            .episode_id
            .as_ref()
            .and_then(|id| self.store.episodes.get(id))
        else {
            return Vec::new();
        };
        let mut latest: BTreeMap<&str, &PackFeedback> = BTreeMap::new();
        for feedback in episode
            .feedback
            .iter()
            .filter(|feedback| feedback.pack_id == pack.pack_id)
        {
            let entry = latest
                .entry(feedback.requester.id.as_str())
                .or_insert(feedback);
            if feedback.at >= entry.at {
                *entry = feedback;
            }
        }
        latest.into_values().collect()
    }

    /** How people judged a pack: Yes when someone found it sufficient and nobody found it lacking,
     * No when someone found it lacking, Unknown otherwise
     * Input
        - pack: &EvidencePack - pack
     * Output
        - Truth
    */
    pub(crate) fn judged(&self, pack: &EvidencePack) -> Truth {
        let feedback = self.feedback(pack);
        if feedback
            .iter()
            .any(|feedback| feedback.sufficient == Truth::No)
        {
            Truth::No
        } else if feedback
            .iter()
            .any(|feedback| feedback.sufficient == Truth::Yes)
        {
            Truth::Yes
        } else {
            Truth::Unknown
        }
    }

    /** Report whether a pack came from a registered recipe and reached a verdict about its
     * question (an exploration or a rejected request did not)
     * Input
        - pack: &EvidencePack - pack
     * Output
        - bool
    */
    pub(crate) fn concluded(pack: &EvidencePack) -> bool {
        !matches!(
            pack.status,
            InvestigationStatus::Exploratory | InvestigationStatus::Rejected
        )
    }
}
