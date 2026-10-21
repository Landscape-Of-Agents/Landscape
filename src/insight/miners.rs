// The miners of Insight. Each one counts over the read model (only what the viewer may see) and
// reports a pattern only when its count reaches the minimum support; a smaller candidate is
// counted as suppressed and never shown, so no pattern can single out one record. Every pattern
// states its numerator, its denominator, and what could not be judged, and outcome patterns are
// co-occurrences compared with all work, never causes. The miners are deterministic: the same
// store, viewer, and version give the same patterns in the same order.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::read::ReadModel;
use super::{
    Counts, Destination, Pattern, PatternKind, LEARN_SUPPORT, MAX_CITED, MIN_SUPPORT,
    OUTCOME_MARGIN_PERCENT,
};
use crate::capture::broker::run::ExecutionMode;
use crate::capture::format::record::{EvidenceRecord, EvidenceType, Payload};
use crate::capture::ingest::episode::{EpisodeOutcome, WorkEpisode};
use crate::capture::ingest::pack::{EvidencePack, InvestigationStatus, SourceState};
use crate::capture::Truth;

/** What the miners found
 * Fields
    - patterns: Vec<Pattern> - patterns, by kind and subject
    - suppressed: usize - candidates below the minimum support
*/
#[derive(Debug, Default)]
pub(crate) struct Mined {
    pub(crate) patterns: Vec<Pattern>,
    pub(crate) suppressed: usize,
}

/** A zone and the version an investigation saw, None when the subject is not a zone */
type Zone = Option<(String, Option<String>)>;

/** The zone an investigation was about, or its repository when it named no zone */
type Subject = (String, Zone);

/** Run every miner
 * Input
    - model: &ReadModel - what the viewer may read
    - served: &BTreeSet<String> - question classes a recipe serves
 * Output
    - Mined
*/
pub(crate) fn mine(model: &ReadModel, served: &BTreeSet<String>) -> Mined {
    let mut mined = Mined::default();
    evidence_gaps(model, &mut mined);
    questions(model, served, &mut mined);
    trust(model, &mut mined);
    outcomes(model, &mut mined);
    work(model, &mut mined);
    mined.patterns.sort_by(|a, b| {
        (a.kind, &a.subject, &a.pattern_id).cmp(&(b.kind, &b.subject, &b.pattern_id))
    });
    mined
}

/** What a pack was about: each of its zones (with the version it saw), or its repository
 * Input
    - pack: &EvidencePack - pack
 * Output
    - Vec<Subject>
*/
fn subjects(pack: &EvidencePack) -> Vec<Subject> {
    if pack.scope.zones.is_empty() {
        let repository = pack.scope.repository.clone().unwrap_or_default();
        return vec![(format!("repository {repository}"), None)];
    }
    pack.scope
        .zones
        .iter()
        .map(|zone| {
            (
                format!("zone {}", zone.id),
                Some((zone.id.clone(), zone.version.clone())),
            )
        })
        .collect()
}

/** Group the concluded packs by subject; a zone keeps the version its latest pack saw
 * Input
    - model: &ReadModel - read model
 * Output
    - BTreeMap<String, (Zone, Vec<&EvidencePack>)>
*/
fn by_subject<'a>(model: &ReadModel<'a>) -> BTreeMap<String, (Zone, Vec<&'a EvidencePack>)> {
    let mut groups: BTreeMap<String, (Zone, Vec<&EvidencePack>)> = BTreeMap::new();
    let mut packs = model
        .packs
        .iter()
        .copied()
        .filter(|pack| ReadModel::concluded(pack))
        .collect::<Vec<_>>();
    packs.sort_by_key(|pack| (pack.created_at, pack.pack_id.clone()));
    for pack in packs {
        for (subject, zone) in subjects(pack) {
            let entry = groups.entry(subject).or_default();
            if zone.is_some() {
                entry.0 = zone;
            }
            entry.1.push(pack);
        }
    }
    groups
}

/** The visible evidence of some packs, optionally of one type, deduplicated and bounded
 * Input
    - model: &ReadModel - read model
    - packs: &[&EvidencePack] - packs
    - evidence_type: Option<EvidenceType> - type, None for every type
 * Output
    - Vec<String>
*/
fn cited(
    model: &ReadModel,
    packs: &[&EvidencePack],
    evidence_type: Option<EvidenceType>,
) -> Vec<String> {
    packs
        .iter()
        .flat_map(|pack| model.pack_evidence(pack, evidence_type))
        .map(|record| record.evidence_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(MAX_CITED)
        .collect()
}

/** The pack and episode ids of some packs
 * Input
    - packs: &[&EvidencePack] - packs
 * Output
    - (Vec<String>, Vec<String>)
*/
fn ids(packs: &[&EvidencePack]) -> (Vec<String>, Vec<String>) {
    let packs_ids = packs
        .iter()
        .map(|pack| pack.pack_id.clone())
        .collect::<BTreeSet<_>>();
    let episodes = packs
        .iter()
        .filter_map(|pack| pack.episode_id.clone())
        .collect::<BTreeSet<_>>();
    (
        packs_ids.into_iter().collect(),
        episodes.into_iter().collect(),
    )
}

/** Evidence the investigations of a zone (or a repository) keep lacking, with the reason the
 * packs gave (Empty, Unavailable, Denied, NotQueried, ParseFailed): each reason points to a
 * different repair, so each is its own pattern
 * Input
    - model: &ReadModel - read model
    - mined: &mut Mined - results
 * Output
    - None
*/
fn evidence_gaps(model: &ReadModel, mined: &mut Mined) {
    for (subject, (zone, packs)) in by_subject(model) {
        let mut gaps: BTreeMap<(EvidenceType, SourceState), Vec<&EvidencePack>> = BTreeMap::new();
        for pack in &packs {
            for missing in &pack.missing_evidence {
                let lacking = gaps
                    .entry((missing.evidence_type, missing.state))
                    .or_default();
                if !lacking.iter().any(|other| other.pack_id == pack.pack_id) {
                    lacking.push(pack);
                }
            }
        }
        for ((evidence_type, state), lacking) in gaps {
            if lacking.len() < MIN_SUPPORT {
                mined.suppressed += 1;
                continue;
            }
            let mut pattern = Pattern::new(
                PatternKind::EvidenceGap,
                "evidence-gaps",
                format!("{subject}: {evidence_type:?}"),
                format!(
                    "{} of {} investigations of {subject} lacked {evidence_type:?} evidence ({state:?})",
                    lacking.len(),
                    packs.len()
                ),
                Counts {
                    matching: lacking.len(),
                    total: packs.len(),
                    unknown: 0,
                },
            );
            (pattern.packs, pattern.episodes) = ids(&lacking);
            pattern.evidence = cited(model, &lacking, None);
            if let (Some((zone, version)), false) = (&zone, pattern.evidence.is_empty()) {
                pattern.destination = Destination::Context {
                    zone: zone.clone(),
                    version: version.clone(),
                };
            }
            mined.patterns.push(pattern);
        }
    }
}

/** Questions no recipe serves: how often each class was asked, how often it was explored, and how
 * people judged the explorations. Enough explorations judged sufficient make the class a
 * candidate for a learned recipe
 * Input
    - model: &ReadModel - read model
    - served: &BTreeSet<String> - classes a recipe serves (they need no learned recipe)
    - mined: &mut Mined - results
 * Output
    - None
*/
fn questions(model: &ReadModel, served: &BTreeSet<String>, mined: &mut Mined) {
    let mut classes: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for audit in &model.audits {
        let class = audit.request.investigation_class.as_str();
        let unserved = audit
            .recipe
            .as_ref()
            .is_none_or(|recipe| recipe.is_exploration());
        if audit.mode == ExecutionMode::Fresh && unserved && !served.contains(class) {
            classes.entry(class).or_default().push(*audit);
        }
    }
    for (class, asked) in classes {
        let explorations = asked
            .iter()
            .filter_map(|audit| audit.pack_id.as_deref())
            .filter_map(|id| model.pack(id))
            .filter(|pack| pack.status == InvestigationStatus::Exploratory)
            .collect::<Vec<_>>();
        let judged = |truth: Truth| {
            explorations
                .iter()
                .copied()
                .filter(|pack| model.judged(pack) == truth)
                .collect::<Vec<_>>()
        };
        let (sufficient, lacking, unjudged) = (
            judged(Truth::Yes),
            judged(Truth::No),
            judged(Truth::Unknown),
        );
        if asked.len() < MIN_SUPPORT && sufficient.len() < LEARN_SUPPORT {
            mined.suppressed += 1;
            continue;
        }
        let mut pattern = Pattern::new(
            PatternKind::Question,
            "questions",
            format!("class {class}"),
            format!(
                "'{class}' was asked {} times and no recipe serves it; {} explorations ran: {} judged sufficient, {} not, {} not judged",
                asked.len(),
                explorations.len(),
                sufficient.len(),
                lacking.len(),
                unjudged.len()
            ),
            Counts {
                matching: sufficient.len(),
                total: explorations.len(),
                unknown: unjudged.len(),
            },
        );
        (pattern.packs, pattern.episodes) = ids(&sufficient);
        pattern.evidence = cited(model, &sufficient, None);
        if sufficient.len() >= LEARN_SUPPORT {
            pattern.destination = Destination::Recipe {
                class: class.to_string(),
            };
        }
        mined.patterns.push(pattern);
    }
}

/** The known outcome of an episode
 * Input
    - episode: &WorkEpisode - episode
 * Output
    - Truth - Yes when it succeeded (or was partially verified), No when it failed, Unknown
      otherwise (not recorded, unknown, or abandoned)
*/
fn outcome(episode: &WorkEpisode) -> Truth {
    match episode.outcome {
        Some(EpisodeOutcome::Succeeded | EpisodeOutcome::PartiallyVerified) => Truth::Yes,
        Some(EpisodeOutcome::Failed) => Truth::No,
        _ => Truth::Unknown,
    }
}

/** How often the investigations of a zone were sufficient, beside the outcomes of its work. A zone
 * whose investigations were all sufficient and whose work never failed is a candidate for more
 * agent autonomy, which only the owner or an admin can grant
 * Input
    - model: &ReadModel - read model
    - mined: &mut Mined - results
 * Output
    - None
*/
fn trust(model: &ReadModel, mined: &mut Mined) {
    for (subject, (zone, packs)) in by_subject(model) {
        if zone.is_none() {
            continue;
        }
        if packs.len() < MIN_SUPPORT {
            mined.suppressed += 1;
            continue;
        }
        let sufficient = packs
            .iter()
            .filter(|pack| pack.status == InvestigationStatus::Sufficient)
            .count();
        let episodes = packs
            .iter()
            .filter_map(|pack| pack.episode_id.as_ref())
            .filter_map(|id| model.episodes.get(id))
            .map(|episode| (episode.episode_id.clone(), outcome(episode)))
            .collect::<BTreeMap<_, _>>();
        let count = |truth: Truth| episodes.values().filter(|value| **value == truth).count();
        let (succeeded, failed, unknown) =
            (count(Truth::Yes), count(Truth::No), count(Truth::Unknown));
        let mut pattern = Pattern::new(
            PatternKind::Trust,
            "trust",
            subject.clone(),
            format!(
                "{sufficient} of {} investigations of {subject} were sufficient; of its {} work episodes {succeeded} succeeded, {failed} failed, and {unknown} have no known outcome",
                packs.len(),
                episodes.len()
            ),
            Counts {
                matching: sufficient,
                total: packs.len(),
                unknown: 0,
            },
        );
        (pattern.packs, pattern.episodes) = ids(&packs);
        pattern.evidence = cited(model, &packs, None);
        if sufficient == packs.len() && failed == 0 && succeeded >= MIN_SUPPORT {
            pattern.destination = Destination::Governance;
        }
        mined.patterns.push(pattern);
    }
}

/** The top directories of the files a change touched
 * Input
    - record: &EvidenceRecord - code change
 * Output
    - BTreeSet<String> such as "migrations/"
*/
fn directories(record: &EvidenceRecord) -> BTreeSet<String> {
    match &record.payload {
        Some(Payload::Code { files, .. }) => files
            .iter()
            .filter_map(|file| file.path.split_once('/'))
            .map(|(top, _)| format!("{top}/"))
            .collect(),
        _ => BTreeSet::new(),
    }
}

/** What the work touching one feature showed
 * Fields
    - zone: Zone - the zone, when the feature is one
    - episodes: BTreeMap<String, Truth> - its work episodes with their outcome
    - evidence: BTreeSet<String> - evidence of its changes and outcomes
*/
#[derive(Default)]
struct Feature {
    zone: Zone,
    episodes: BTreeMap<String, Truth>,
    evidence: BTreeSet<String>,
}

/** Features (a zone, or a top directory changed) whose work fails clearly more often than work
 * overall. Only episodes with a known outcome are compared; the others are counted beside them
 * Input
    - model: &ReadModel - read model
    - mined: &mut Mined - results
 * Output
    - None
*/
fn outcomes(model: &ReadModel, mined: &mut Mined) {
    let mut features: BTreeMap<String, Feature> = BTreeMap::new();
    let mut all = BTreeMap::new();
    for episode in model.episodes.values() {
        let truth = outcome(episode);
        all.insert(episode.episode_id.clone(), truth);
        let packs = model.episode_packs(episode);
        let mut found: BTreeMap<String, (Zone, BTreeSet<String>)> = BTreeMap::new();
        for zone in &episode.zones {
            found.insert(
                format!("zone {}", zone.id),
                (
                    Some((zone.id.clone(), zone.version.clone())),
                    BTreeSet::new(),
                ),
            );
        }
        for pack in &packs {
            for change in model.pack_evidence(pack, Some(EvidenceType::CodeChange)) {
                for directory in directories(change) {
                    found
                        .entry(directory)
                        .or_default()
                        .1
                        .insert(change.evidence_id.clone());
                }
            }
        }
        let outcome_evidence = episode
            .outcome_evidence
            .iter()
            .filter(|id| model.evidence(id).is_some())
            .cloned()
            .collect::<Vec<_>>();
        for (feature, (zone, evidence)) in found {
            let entry = features.entry(feature).or_default();
            if zone.is_some() {
                entry.zone = zone;
            }
            entry.episodes.insert(episode.episode_id.clone(), truth);
            entry.evidence.extend(evidence);
            entry.evidence.extend(outcome_evidence.iter().cloned());
        }
    }
    let count = |outcomes: &BTreeMap<String, Truth>, truth: Truth| {
        outcomes.values().filter(|value| **value == truth).count()
    };
    let failed_all = count(&all, Truth::No);
    let known_all = failed_all + count(&all, Truth::Yes);
    if known_all == 0 {
        return;
    }
    for (
        feature,
        Feature {
            zone,
            episodes,
            evidence,
        },
    ) in features
    {
        let failed = count(&episodes, Truth::No);
        let known = failed + count(&episodes, Truth::Yes);
        let unknown = count(&episodes, Truth::Unknown);
        if known < MIN_SUPPORT {
            mined.suppressed += 1;
            continue;
        }
        if failed < 2 || !fails_clearly_more(failed, known, failed_all, known_all) {
            continue;
        }
        let mut pattern = Pattern::new(
            PatternKind::Outcome,
            "outcomes",
            feature.clone(),
            format!(
                "work touching {feature} failed {failed} of {known} times with a known outcome, against {failed_all} of {known_all} for all work; {unknown} more have no known outcome (a co-occurrence, not a cause)"
            ),
            Counts {
                matching: failed,
                total: known,
                unknown,
            },
        );
        pattern.baseline = Some(Counts {
            matching: failed_all,
            total: known_all,
            unknown: count(&all, Truth::Unknown),
        });
        pattern.episodes = episodes.keys().cloned().collect();
        pattern.packs = pattern
            .episodes
            .iter()
            .filter_map(|id| model.episodes.get(id))
            .flat_map(|episode| model.episode_packs(episode))
            .map(|pack| pack.pack_id.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        pattern.evidence = evidence.into_iter().take(MAX_CITED).collect();
        if let (Some((zone, version)), false) = (zone, pattern.evidence.is_empty()) {
            pattern.destination = Destination::Context { zone, version };
        }
        mined.patterns.push(pattern);
    }
}

/** Report whether a feature's work fails clearly more often than all work: its failure rate is at
 * least the overall rate plus OUTCOME_MARGIN_PERCENT points (whole-number arithmetic, so the
 * comparison is exact)
 * Input
    - failed: usize - the feature's failed episodes
    - known: usize - the feature's episodes with a known outcome (at least 1)
    - failed_all: usize - all failed episodes
    - known_all: usize - all episodes with a known outcome (at least 1)
 * Output
    - bool
*/
pub(crate) fn fails_clearly_more(
    failed: usize,
    known: usize,
    failed_all: usize,
    known_all: usize,
) -> bool {
    // failed / known >= failed_all / known_all + margin / 100
    100 * failed * known_all
        >= 100 * failed_all * known + OUTCOME_MARGIN_PERCENT * known * known_all
}

/** The tool an agent tool call used: the event's own "tool" field, the tool of its payload, or
 * its action's tool
 * Input
    - record: &EvidenceRecord - tool call
 * Output
    - Option<String>
*/
fn tool_name(record: &EvidenceRecord) -> Option<String> {
    let Some(Payload::Structured { fields }) = &record.payload else {
        return None;
    };
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_string);
    text(fields.get("tool"))
        .or_else(|| {
            text(
                fields
                    .get("payload")
                    .and_then(|payload| payload.get("tool")),
            )
        })
        .or_else(|| {
            text(
                fields
                    .get("action")
                    .and_then(|action| action.get("action_tool")),
            )
        })
        .filter(|name| !name.is_empty())
}

/** Agent tool sequences (three tools in a row) that recur across work episodes: a procedure
 * people's agents keep repeating is a candidate capability, compiled from one of the episodes
 * Input
    - model: &ReadModel - read model
    - mined: &mut Mined - results
 * Output
    - None
*/
fn work(model: &ReadModel, mined: &mut Mined) {
    let mut sequences: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    let mut with_tools = 0;
    for episode in model.episodes.values() {
        let mut calls = BTreeMap::new();
        for pack in model.episode_packs(episode) {
            for call in model.pack_evidence(pack, Some(EvidenceType::ToolCall)) {
                calls.insert(
                    (
                        call.observed_at.unwrap_or_default(),
                        call.evidence_id.clone(),
                    ),
                    call,
                );
            }
        }
        let mut tools: Vec<String> = Vec::new();
        for call in calls.values() {
            if let Some(name) = tool_name(call) {
                if tools.last() != Some(&name) {
                    tools.push(name);
                }
            }
        }
        if tools.is_empty() {
            continue;
        }
        with_tools += 1;
        let cited = calls
            .values()
            .map(|call| call.evidence_id.clone())
            .collect::<Vec<_>>();
        for window in tools.windows(3) {
            let entry = sequences.entry(window.join(" > ")).or_default();
            entry.0.insert(episode.episode_id.clone());
            entry.1.extend(cited.iter().cloned());
        }
    }
    for (sequence, (episodes, evidence)) in sequences {
        if episodes.len() < MIN_SUPPORT {
            mined.suppressed += 1;
            continue;
        }
        let succeeded = episodes
            .iter()
            .filter_map(|id| model.episodes.get(id))
            .filter(|episode| outcome(episode) == Truth::Yes)
            .map(|episode| episode.episode_id.clone())
            .collect::<Vec<_>>();
        let mut pattern = Pattern::new(
            PatternKind::Work,
            "work",
            sequence.clone(),
            format!(
                "the agent tool sequence {sequence} recurs in {} of {with_tools} work episodes with tool calls; {} of them succeeded",
                episodes.len(),
                succeeded.len()
            ),
            Counts {
                matching: episodes.len(),
                total: with_tools,
                unknown: 0,
            },
        );
        pattern.episodes = episodes.iter().cloned().collect();
        pattern.evidence = evidence.into_iter().take(MAX_CITED).collect();
        // A capability is compiled from one episode whose work is known to have succeeded
        if let Some(episode) = succeeded.first() {
            pattern.destination = Destination::Capability {
                episode: episode.clone(),
            };
        }
        mined.patterns.push(pattern);
    }
}
