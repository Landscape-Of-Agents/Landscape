// Context packs: the versioned context an activation reasons with, compiled from the agent's
// mission, the target's Flow or Zone context, the Common and Current Context of every Brick
// attached to it (brick::context::assemble, the same snapshot the hooks deliver), and the RCA graph
// version. The pack's key is the digest of every dependency (definition, assignment, signed
// generation, governance and Brick store digests, snapshot digest, RCA digest), so a change to any
// of them gives a new key. The last pack of each assignment is kept; the next activation reports a
// HIT (same key), MISS (none kept), or INVALIDATED with the dependencies that changed.
//
// The cache is an application record, not provider prompt caching, and never authority: every
// step is still decided by authority::authorize against the signed state read at that moment. The
// trigger and previous outcomes are per activation and are recorded on the activation, not here.
// A required item that cannot be loaded is named in 'missing'; the pack is then incomplete and the
// activation blocks or asks, as its definition says.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::authority::{Authority, Trust};
use super::model::{Assignment, DefinitionRecord};
use super::rca::RcaRecord;
use crate::brick::context::Snapshot;
use crate::platform::files::write_atomic;
use crate::trust::crypto::{digest, digest_json};

/** What a pack depends on
 * Fields
    - definition: String - definition digest
    - assignment: String - assignment content digest
    - generation: u64 - signed generation
    - governance: String - governance digest
    - bricks: String - Brick store digest
    - snapshot: String - Flow, Zone, and Brick context snapshot digest
    - rca: String - RCA graph digest ("" without one)
    - rca_state: String - RCA graph state ("" without one)
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Dependencies {
    pub(crate) definition: String,
    pub(crate) assignment: String,
    pub(crate) generation: u64,
    pub(crate) governance: String,
    pub(crate) bricks: String,
    pub(crate) snapshot: String,
    pub(crate) rca: String,
    pub(crate) rca_state: String,
}

impl Dependencies {
    /** The names of the dependencies that differ from another set
     * Input
        - other: &Dependencies - previous set
     * Output
        - Vec<String>
    */
    pub(crate) fn changed(&self, other: &Dependencies) -> Vec<String> {
        let mut names = Vec::new();
        let mut check = |name: &str, same: bool| {
            if !same {
                names.push(name.to_string());
            }
        };
        check("definition", self.definition == other.definition);
        check("assignment", self.assignment == other.assignment);
        check("generation", self.generation == other.generation);
        check("governance", self.governance == other.governance);
        check("bricks", self.bricks == other.bricks);
        check("context", self.snapshot == other.snapshot);
        check(
            "rca",
            self.rca == other.rca && self.rca_state == other.rca_state,
        );
        names
    }
}

/** One section of a pack (text stays on this machine; reports carry digests and sizes)
 * Fields
    - kind: String - agent, enforcement, flow, zone, brick_common, brick_current, rca
    - source: String - where it comes from
    - reference: String - exact version
    - digest: String - digest of the text
    - bytes: usize - size
    - text: String - content
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Section {
    pub(crate) kind: String,
    pub(crate) source: String,
    pub(crate) reference: String,
    pub(crate) digest: String,
    pub(crate) bytes: usize,
    pub(crate) text: String,
}

/** A compiled context pack
 * Fields
    - key: String - digest of the dependencies
    - dependencies: Dependencies - what it was built from
    - sections: Vec<Section> - sections in order
    - missing: Vec<(String, String)> - required items that could not be loaded
    - gaps: Vec<String> - snapshot gaps and conflicts (not blocking)
    - built_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Pack {
    pub(crate) key: String,
    pub(crate) dependencies: Dependencies,
    pub(crate) sections: Vec<Section>,
    pub(crate) missing: Vec<(String, String)>,
    pub(crate) gaps: Vec<String>,
    pub(crate) built_at: u64,
}

impl Pack {
    /** Report whether every required item was loaded
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/** The cache outcome of an activation's pack
 * Variants
    - Hit - the kept pack had the same key
    - Miss - no pack was kept
    - Invalidated - the kept pack's dependencies changed (which ones)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "cache",
    content = "changed",
    rename_all = "SCREAMING_SNAKE_CASE"
)]
pub(crate) enum Cache {
    Hit,
    Miss,
    Invalidated(Vec<String>),
}

/** Build a section
 * Input
    - kind: &str - kind
    - source: String - source
    - reference: String - reference
    - text: String - content (redacted)
 * Output
    - Section
*/
fn section(kind: &str, source: String, reference: String, text: String) -> Section {
    let (text, _) = crate::telemetry::redact::redact(&text);
    Section {
        kind: kind.into(),
        digest: digest("resident/context/v1", &[text.as_bytes()]),
        bytes: text.len(),
        source,
        reference,
        text,
    }
}

/** Compile the pack of an assignment
 * Input
    - authority: &Authority - Crane view
    - definition: &DefinitionRecord - definition version
    - assignment: &Assignment - assignment
    - snapshot: &Snapshot - the target's Flow, Zone, and Brick context
    - rca: Option<&RcaRecord> - the assignment's RCA graph, when found
    - now: u64 - Unix milliseconds
 * Output
    - Pack
*/
pub(crate) fn compile(
    authority: &Authority,
    definition: &DefinitionRecord,
    assignment: &Assignment,
    snapshot: &Snapshot,
    rca: Option<&RcaRecord>,
    now: u64,
) -> Pack {
    let mut sections = vec![section(
        "agent",
        format!("definition:{}@{}", definition.definition.definition_id, definition.definition.version),
        definition.digest.clone(),
        format!(
            "Resident agent {} for {} (owner {}).\nMission: {}\nThis context is guidance. Crane decides every action against the signed rules.",
            definition.definition.name,
            assignment.target.key(),
            if definition.definition.owner.is_empty() { "unnamed" } else { &definition.definition.owner },
            definition.definition.purpose
        ),
    )];
    sections.extend(snapshot.sections.iter().map(|item| Section {
        kind: item.kind.clone(),
        source: item.source.clone(),
        reference: item.reference.clone(),
        digest: item.digest.clone(),
        bytes: item.bytes,
        text: item.text.clone(),
    }));
    let mut missing = Vec::new();
    if let Trust::Unverified(reason) = &authority.trust {
        missing.push(("SIGNED_STATE_UNVERIFIED".to_string(), reason.clone()));
    }
    for gap in snapshot.gaps.iter().chain(snapshot.conflicts.iter()) {
        if matches!(
            gap.code.as_str(),
            "CONTEXT_FILE_MISSING" | "PIN_NOT_HONORED"
        ) {
            missing.push((gap.code.clone(), gap.message.clone()));
        }
    }
    for brick in &definition.definition.bricks {
        if !snapshot.bricks.iter().any(|delivered| delivered == brick) {
            missing.push((
                "BRICK_CONTEXT_MISSING".to_string(),
                format!(
                    "the definition expects {brick} on {}, but its context was not delivered",
                    assignment.target.key()
                ),
            ));
        }
    }
    match (&assignment.rca, rca) {
        (Some(reference), None) => missing.push((
            "RCA_MISSING".to_string(),
            format!("RCA graph {}@{} is not in the store", reference.rca_id, reference.version),
        )),
        (_, Some(record)) => sections.push(section(
            "rca",
            format!("rca:{}@{}", record.graph.rca_id, record.graph.version),
            format!("{} ({})", record.digest, crate::resident::word(&record.state)),
            format!(
                "RCA graph {} v{} ({}): {} nodes, {} edges. Proposed graphs and hypothesized nodes are not established facts.",
                record.graph.name,
                record.graph.version,
                crate::resident::word(&record.state),
                record.graph.nodes.len(),
                record.graph.edges.len()
            ),
        )),
        (None, None) => {}
    }
    let dependencies = Dependencies {
        definition: definition.digest.clone(),
        assignment: assignment.content_digest(),
        generation: authority.generation,
        governance: authority.governance_digest.clone(),
        bricks: authority.bricks_digest.clone(),
        snapshot: snapshot.digest.clone(),
        rca: rca.map(|record| record.digest.clone()).unwrap_or_default(),
        rca_state: rca
            .map(|record| crate::resident::word(&record.state))
            .unwrap_or_default(),
    };
    Pack {
        key: digest_json("resident/pack/v1", &json!(dependencies))[..32].to_string(),
        dependencies,
        sections,
        missing,
        gaps: snapshot
            .gaps
            .iter()
            .chain(snapshot.conflicts.iter())
            .map(|gap| format!("{}: {}", gap.code, gap.message))
            .collect(),
        built_at: now,
    }
}

/** Compare a new pack with the kept one
 * Input
    - pack: &Pack - new pack
    - kept: Option<&Pack> - the assignment's kept pack
 * Output
    - Cache
*/
pub(crate) fn compare(pack: &Pack, kept: Option<&Pack>) -> Cache {
    match kept {
        None => Cache::Miss,
        Some(kept) if kept.key == pack.key => Cache::Hit,
        Some(kept) => Cache::Invalidated(pack.dependencies.changed(&kept.dependencies)),
    }
}

/** The pack cache of a runtime directory (one file per assignment; in memory for tests)
 * Fields
    - directory: Option<PathBuf> - runtime/resident/context, None in memory
    - memory: BTreeMap<String, Pack> - packs kept in memory
*/
pub(crate) struct PackCache {
    directory: Option<PathBuf>,
    memory: BTreeMap<String, Pack>,
}

impl PackCache {
    /** An in-memory cache
     * Input
        - None
     * Output
        - PackCache
    */
    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            directory: None,
            memory: BTreeMap::new(),
        }
    }

    /** The cache of a runtime directory
     * Input
        - runtime: &Path - runtime directory
     * Output
        - PackCache
    */
    pub(crate) fn open(runtime: &Path) -> Self {
        Self {
            directory: Some(runtime.join("resident").join("context")),
            memory: BTreeMap::new(),
        }
    }

    /** The kept pack of an assignment
     * Input
        - assignment_id: &str - assignment
     * Output
        - Option<Pack>
    */
    pub(crate) fn get(&self, assignment_id: &str) -> Option<Pack> {
        match &self.directory {
            None => self.memory.get(assignment_id).cloned(),
            Some(directory) => {
                std::fs::read_to_string(directory.join(format!("{assignment_id}.json")))
                    .ok()
                    .and_then(|text| serde_json::from_str(&text).ok())
            }
        }
    }

    /** Keep a pack for an assignment
     * Input
        - assignment_id: &str - assignment
        - pack: &Pack - pack
     * Output
        - Result<(), String>
    */
    pub(crate) fn put(&mut self, assignment_id: &str, pack: &Pack) -> Result<(), String> {
        match &self.directory {
            None => {
                self.memory.insert(assignment_id.into(), pack.clone());
                Ok(())
            }
            Some(directory) => {
                std::fs::create_dir_all(directory).map_err(|error| error.to_string())?;
                let text = serde_json::to_string_pretty(pack).map_err(|error| error.to_string())?;
                write_atomic(
                    &directory.join(format!("{assignment_id}.json")),
                    text.as_bytes(),
                )
            }
        }
    }

    /** Drop the kept pack of an assignment (revocation, pause, target change)
     * Input
        - assignment_id: &str - assignment
     * Output
        - None
    */
    pub(crate) fn drop_pack(&mut self, assignment_id: &str) {
        match &self.directory {
            None => {
                self.memory.remove(assignment_id);
            }
            Some(directory) => {
                let _ = std::fs::remove_file(directory.join(format!("{assignment_id}.json")));
            }
        }
    }
}
