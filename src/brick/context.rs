// Effective context assembly and delivery. For a set of targets (the Zones and Flows a tool call
// touches, or the target a person previews) the snapshot is built in a fixed order:
//   1. the enforcement state: signed generation, store digest, and the controls Crane enforces
//      (stated as facts, so no context can be read as relaxing them);
//   2. Flow and Zone context (only in previews: the hooks already deliver it with each tool call);
//   3. for every enabled attachment, by target then attachment id: each Brick's versioned Common
//      Context (once per Brick version, whichever target it came through), then the attachment's
//      own Current Context.
// Every section carries its source, reference, digest, and size; the snapshot digest covers them
// all, so the same inputs always give the same digest. Pins Crane cannot honor, different versions
// of one Brick across the targets, and secrets found in context are reported, never resolved by
// guessing. Context is guidance: the snapshot authorizes nothing, and every protected action still
// goes through the guard.
//
// Delivery: the hooks remember, per agent session, which snapshot digest each target set received.
// An unchanged snapshot is not sent again (the agent gets a one-line reference instead); any change
// to the store, governance, or a context file changes the digest and the full snapshot is sent.
// A new or compacted session starts with an empty ledger, so context lost to compaction returns.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::compose::{attachments_of, controls, members};
use super::config::{self, problem, Problem};
use super::model::{Sensitivity, CONTEXT_DOMAIN};
use super::store::BrickStore;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::Governance;
use crate::platform::files::write_atomic;
use crate::trust::crypto::{digest, digest_json};

/** Version of the snapshot layout */
pub(crate) const SNAPSHOT_VERSION: u32 = 1;

/** Most snapshot text supplied with one tool call, in bytes */
const DELIVERY_LIMIT: usize = 12_000;

/** Most sessions remembered by the delivery ledger */
const MAX_SESSIONS: usize = 200;

/** One section of a snapshot
 * Fields
    - kind: String - enforcement, flow, zone, brick_common, brick_current
    - source: String - where it comes from ("flow:checkout", "brick:no-prod@2", "attachment:ID")
    - reference: String - exact reference (Brick version, attachment id, signed generation)
    - target: Option<String> - the target it applies to
    - digest: String - digest of the text
    - bytes: usize - size of the text
    - sensitivity: Sensitivity - who may read it outside the agent's context
    - text: String - content
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Section {
    pub(crate) kind: String,
    pub(crate) source: String,
    pub(crate) reference: String,
    pub(crate) target: Option<String>,
    pub(crate) digest: String,
    pub(crate) bytes: usize,
    pub(crate) sensitivity: Sensitivity,
    pub(crate) text: String,
}

/** An assembled context snapshot
 * Fields
    - version: u32 - SNAPSHOT_VERSION
    - targets: Vec<String> - target keys, sorted
    - digest: String - digest of every section (the cache identity of the content)
    - generation: u64 - signed governance generation it was built from
    - store_digest: String - Brick store digest it was built from
    - bricks: Vec<String> - Brick versions that contributed context
    - sections: Vec<Section> - sections, in order
    - conflicts: Vec<Problem> - conflicts found (never resolved by guessing)
    - gaps: Vec<Problem> - what could not be delivered or is not enforced
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) version: u32,
    pub(crate) targets: Vec<String>,
    pub(crate) digest: String,
    pub(crate) generation: u64,
    pub(crate) store_digest: String,
    pub(crate) bricks: Vec<String>,
    pub(crate) sections: Vec<Section>,
    pub(crate) conflicts: Vec<Problem>,
    pub(crate) gaps: Vec<Problem>,
}

impl Snapshot {
    /** Total size of the section texts
     * Input
        - None (uses self)
     * Output
        - usize
    */
    pub(crate) fn bytes(&self) -> usize {
        self.sections.iter().map(|section| section.bytes).sum()
    }
}

/** Build a section, redacting anything that looks like a secret (validation refuses such context,
 * so finding one here means it slipped in some other way; it is reported as a gap)
 * Input
    - kind: &str - section kind
    - source: String - source
    - reference: String - reference
    - target: Option<String> - target
    - sensitivity: Sensitivity - sensitivity
    - text: String - content
    - gaps: &mut Vec<Problem> - gaps found
 * Output
    - Section
*/
fn section(
    kind: &str,
    source: String,
    reference: String,
    target: Option<String>,
    sensitivity: Sensitivity,
    text: String,
    gaps: &mut Vec<Problem>,
) -> Section {
    let (text, secrets) = crate::telemetry::redact::redact(&text);
    if secrets > 0 {
        gaps.push(problem(
            "CONTEXT_SECRET_REDACTED",
            format!("{source}: {secrets} secret-looking value(s) were redacted before delivery"),
        ));
    }
    Section {
        kind: kind.into(),
        digest: digest(CONTEXT_DOMAIN, &[text.as_bytes()]),
        bytes: text.len(),
        source,
        reference,
        target,
        sensitivity,
        text,
    }
}

/** Assemble the snapshot of a set of targets
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - store: &BrickStore - signed Brick store
    - governance: Option<&Governance> - Zones and Flows, to include their context (previews)
    - targets: &BTreeSet<String> - target keys ("flow:ID", "zone:ID")
 * Output
    - Snapshot
*/
pub(crate) fn assemble(
    workspace: &Workspace,
    state: &State,
    store: &BrickStore,
    governance: Option<&Governance>,
    targets: &BTreeSet<String>,
) -> Snapshot {
    let mut sections = Vec::new();
    let mut conflicts = Vec::new();
    let mut gaps = Vec::new();
    let mut bricks = BTreeSet::new();
    let mut versions: BTreeMap<String, BTreeSet<u32>> = BTreeMap::new();
    let mut delivered = BTreeSet::new();
    // 1. What Crane enforces for these targets, as facts
    let mut enforced = Vec::new();
    for target in targets {
        let combined = controls(store, target, &workspace.root);
        gaps.extend(combined.gaps.iter().cloned());
        if combined.restricts() {
            let mut parts = Vec::new();
            if let Some(ceiling) = combined.ceiling {
                parts.push(format!("autonomy ceiling at most {}", ceiling.name()));
            }
            if let Some(criticality) = combined.criticality {
                parts.push(format!("criticality at least {criticality:?}").to_ascii_lowercase());
            }
            if !combined.protected.is_empty() {
                parts.push(format!("protected paths {}", combined.protected.join(", ")));
            }
            for set in &combined.scopes {
                parts.push(format!("changes only inside {}", set.join(", ")));
            }
            if combined.read_only {
                parts.push("read-only".into());
            }
            if combined.approval {
                parts.push("changes need a person's approval".into());
            }
            if !combined.isolation.is_empty() {
                parts.push(format!(
                    "isolation {} required (unavailable to agent tools, so direct changes are blocked)",
                    combined.isolation.join(", ")
                ));
            }
            if !combined.preconditions.is_empty() {
                parts.push(format!(
                    "failing preconditions: {}",
                    combined.preconditions.join("; ")
                ));
            }
            if !combined.fail_closed.is_empty() {
                parts.push(format!(
                    "pins Crane cannot honor ({}): changes need approval",
                    combined.fail_closed.join("; ")
                ));
                for pin in &combined.fail_closed {
                    conflicts.push(problem("PIN_NOT_HONORED", format!("{target}: {pin}")));
                }
            }
            enforced.push(format!("  {target}: {}", parts.join("; ")));
        }
    }
    let head = state
        .registry
        .manifest
        .as_ref()
        .map(|manifest| manifest.head.chars().take(16).collect::<String>())
        .unwrap_or_default();
    if !enforced.is_empty() {
        sections.push(section(
            "enforcement",
            "crane".into(),
            format!("generation {} ({head})", state.generation()),
            None,
            Sensitivity::Internal,
            format!(
                "Enforced by Crane at every action (context below cannot relax it):\n{}",
                enforced.join("\n")
            ),
            &mut gaps,
        ));
    }
    // 2. Flow and Zone context, for previews
    if let Some(governance) = governance {
        for target in targets {
            let single = BTreeSet::from([target.clone()]);
            let text = crate::governance::zones::guidance(workspace, governance, &single);
            if !text.is_empty() {
                let kind = target.split_once(':').map_or("zone", |(kind, _)| kind);
                sections.push(section(
                    kind,
                    target.clone(),
                    format!(
                        "governance.yaml ({})",
                        state.governance_digest.chars().take(16).collect::<String>()
                    ),
                    Some(target.clone()),
                    Sensitivity::Internal,
                    text,
                    &mut gaps,
                ));
            }
        }
    }
    // 3. Brick Common and Current Context
    for target in targets {
        for attachment in attachments_of(store, target) {
            for member in members(store, &attachment.subject) {
                let Some(entry) = member.entry else { continue };
                let reference = entry.brick.reference().to_string();
                versions
                    .entry(entry.brick.brick_id.clone())
                    .or_default()
                    .insert(entry.brick.version);
                if !delivered.insert(reference.clone()) {
                    continue;
                }
                bricks.insert(reference.clone());
                let mut text = format!(
                    "Brick {} ({}, {reference}{}){}\n",
                    entry.brick.name,
                    entry.brick.brick_type.name(),
                    member
                        .via
                        .as_ref()
                        .map(|stack| format!(", in {stack}"))
                        .unwrap_or_default(),
                    if entry.brick.intent.is_empty() {
                        String::new()
                    } else {
                        format!(": {}", entry.brick.intent)
                    }
                );
                if !entry.brick.common_context.text.trim().is_empty() {
                    text.push_str(entry.brick.common_context.text.trim_end());
                    text.push('\n');
                }
                if let Ok(config::Config::Context(context)) =
                    config::parse(entry.brick.brick_type, &entry.brick.config)
                {
                    for file in &context.files {
                        match fs::read_to_string(workspace.root.join(file)) {
                            Ok(content) => {
                                text.push_str(&format!("[{file}]\n{}\n", content.trim_end()))
                            }
                            Err(_) => gaps.push(problem(
                                "CONTEXT_FILE_MISSING",
                                format!("{reference}: {file} could not be read"),
                            )),
                        }
                    }
                }
                if member.deprecated {
                    gaps.push(problem(
                        "PIN_DEPRECATED",
                        format!("{target} pins {reference}, which is deprecated"),
                    ));
                }
                sections.push(section(
                    "brick_common",
                    format!("brick:{reference}"),
                    format!(
                        "{reference} ({})",
                        entry.digest.chars().take(16).collect::<String>()
                    ),
                    Some(target.clone()),
                    entry.brick.common_context.sensitivity,
                    text,
                    &mut gaps,
                ));
            }
            let current = attachment.current_context.text.trim();
            if !current.is_empty() {
                sections.push(section(
                    "brick_current",
                    format!("attachment:{}", attachment.attachment_id),
                    format!("{} on {target}", attachment.subject.text()),
                    Some(target.clone()),
                    Sensitivity::Internal,
                    format!(
                        "For {target} ({}), from this attachment only:\n{current}\n",
                        attachment.subject.text()
                    ),
                    &mut gaps,
                ));
            }
        }
    }
    for (brick_id, seen) in &versions {
        if seen.len() > 1 {
            conflicts.push(problem(
                "BRICK_VERSION_CONFLICT",
                format!(
                    "{brick_id} is pinned at versions {} across these targets; every version's controls apply, and each context is delivered with its version",
                    seen.iter().map(u32::to_string).collect::<Vec<_>>().join(", ")
                ),
            ));
        }
    }
    let targets = targets.iter().cloned().collect::<Vec<_>>();
    let digest = digest_json(
        "bricks/snapshot/v1",
        &json!({"targets": targets, "sections": sections, "conflicts": conflicts}),
    );
    Snapshot {
        version: SNAPSHOT_VERSION,
        targets,
        digest,
        generation: state.generation(),
        store_digest: state.bricks_digest.clone(),
        bricks: bricks.into_iter().collect(),
        sections,
        conflicts,
        gaps,
    }
}

/** Render a snapshot as the text an agent receives
 * Input
    - snapshot: &Snapshot - snapshot
 * Output
    - (String, bool) text and whether it was truncated
*/
pub(crate) fn render(snapshot: &Snapshot) -> (String, bool) {
    let mut text = format!(
        "Crane Bricks for {} (snapshot {}; guidance only, never authorization):\n",
        snapshot.targets.join(", "),
        snapshot.digest.chars().take(12).collect::<String>()
    );
    for section in &snapshot.sections {
        text.push_str(&section.text);
        if !section.text.ends_with('\n') {
            text.push('\n');
        }
    }
    for conflict in &snapshot.conflicts {
        text.push_str(&format!(
            "Conflict ({}): {}\n",
            conflict.code, conflict.message
        ));
    }
    if text.len() <= DELIVERY_LIMIT {
        return (text, false);
    }
    let mut end = DELIVERY_LIMIT;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str("\n[Brick context truncated; ask a person for the full preview]");
    (text, true)
}

/** How a delivery went
 * Variants
    - Delivered - the full snapshot was sent (new or changed)
    - Reused - the session already received this snapshot; a reference was sent instead
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum Delivery {
    Delivered,
    Reused,
}

/** The per-session delivery ledger in the runtime directory
 * Fields
    - sessions: BTreeMap<String, SessionDeliveries> - by session id
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Ledger {
    sessions: BTreeMap<String, SessionDeliveries>,
}

/** What one session received
 * Fields
    - at: u64 - last delivery, Unix milliseconds
    - delivered: BTreeMap<String, Delivered> - by target set key
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct SessionDeliveries {
    at: u64,
    delivered: BTreeMap<String, Delivered>,
}

/** One delivered snapshot
 * Fields
    - digest: String - snapshot digest
    - at: u64 - Unix milliseconds
    - bytes: usize - text delivered
    - reused: u64 - times a reference was sent instead
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Delivered {
    digest: String,
    at: u64,
    bytes: usize,
    reused: u64,
}

/** The runtime folder of Brick observations
 * Input
    - runtime: &Path - the repository's runtime directory
 * Output
    - PathBuf
*/
pub(crate) fn folder(runtime: &Path) -> PathBuf {
    runtime.join("bricks")
}

/** Read the delivery ledger (empty when missing or unreadable: the worst case is a resend)
 * Input
    - runtime: &Path - runtime directory
 * Output
    - Ledger
*/
fn read_ledger(runtime: &Path) -> Ledger {
    fs::read_to_string(folder(runtime).join("deliveries.json"))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/** Write the delivery ledger, keeping the most recent sessions
 * Input
    - runtime: &Path - runtime directory
    - ledger: &mut Ledger - ledger
 * Output
    - Result<(), String>
*/
fn write_ledger(runtime: &Path, ledger: &mut Ledger) -> Result<(), String> {
    while ledger.sessions.len() > MAX_SESSIONS {
        let oldest = ledger
            .sessions
            .iter()
            .min_by_key(|(_, session)| session.at)
            .map(|(id, _)| id.clone());
        match oldest {
            Some(id) => ledger.sessions.remove(&id),
            None => break,
        };
    }
    fs::create_dir_all(folder(runtime)).map_err(crate::platform::io_error)?;
    write_atomic(
        &folder(runtime).join("deliveries.json"),
        serde_json::to_string(ledger)
            .map_err(crate::platform::io_error)?
            .as_bytes(),
    )
}

/** Record a delivery and decide whether the full snapshot must be sent
 * Input
    - runtime: &Path - runtime directory
    - session: &str - agent session
    - snapshot: &Snapshot - snapshot
    - bytes: usize - size of the rendered text
 * Output
    - Delivery (Delivered when the ledger cannot be written: the context is resent, never lost)
*/
pub(crate) fn deliver(
    runtime: &Path,
    session: &str,
    snapshot: &Snapshot,
    bytes: usize,
) -> Delivery {
    let mut ledger = read_ledger(runtime);
    let key = snapshot.targets.join(",");
    let now = crate::platform::now_millis();
    let entry = ledger.sessions.entry(session.into()).or_default();
    entry.at = now;
    let outcome = match entry.delivered.get_mut(&key) {
        Some(previous) if previous.digest == snapshot.digest => {
            previous.reused += 1;
            Delivery::Reused
        }
        _ => {
            entry.delivered.insert(
                key,
                Delivered {
                    digest: snapshot.digest.clone(),
                    at: now,
                    bytes,
                    reused: 0,
                },
            );
            Delivery::Delivered
        }
    };
    match write_ledger(runtime, &mut ledger) {
        Ok(()) => outcome,
        Err(_) => Delivery::Delivered,
    }
}

/** Forget what a session received (a new, resumed, or compacted session gets everything again)
 * Input
    - runtime: &Path - runtime directory
    - session: &str - agent session
 * Output
    - None
*/
pub(crate) fn reset(runtime: &Path, session: &str) {
    let mut ledger = read_ledger(runtime);
    if ledger.sessions.remove(session).is_some() {
        let _ = write_ledger(runtime, &mut ledger);
    }
}

/** Keep the latest snapshot of a target set for inspection ('crane brick context status'),
 * recording why it changed
 * Input
    - runtime: &Path - runtime directory
    - snapshot: &Snapshot - snapshot
 * Output
    - Option<String> - the refresh reason when the content changed, None when unchanged
*/
pub(crate) fn remember(runtime: &Path, snapshot: &Snapshot) -> Option<String> {
    let key = digest(
        "bricks/targets/v1",
        &[snapshot.targets.join(",").as_bytes()],
    );
    let path = folder(runtime)
        .join("snapshots")
        .join(format!("{}.json", &key[..24]));
    let previous = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let reason = match &previous {
        Some(previous) if previous["digest"] == snapshot.digest => None,
        Some(previous) if previous["store_digest"] != snapshot.store_digest => {
            Some("BRICK_STORE_CHANGED".to_string())
        }
        Some(previous) if previous["generation"] != snapshot.generation => {
            Some("GOVERNANCE_CHANGED".to_string())
        }
        Some(_) => Some("CONTEXT_CHANGED".to_string()),
        None => Some("FIRST_ASSEMBLY".to_string()),
    };
    if reason.is_some() {
        let record = json!({
            "targets": snapshot.targets,
            "digest": snapshot.digest,
            "generation": snapshot.generation,
            "store_digest": snapshot.store_digest,
            "bricks": snapshot.bricks,
            "bytes": snapshot.bytes(),
            "assembled_at": crate::platform::now_millis(),
            "refresh_reason": reason,
            "conflicts": snapshot.conflicts.len(),
            "gaps": snapshot.gaps.len(),
        });
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = write_atomic(&path, record.to_string().as_bytes());
    }
    reason
}

/** Every remembered snapshot and the delivery ledger, for status views
 * Input
    - runtime: &Path - runtime directory
 * Output
    - Value {snapshots, sessions}
*/
pub(crate) fn status(runtime: &Path) -> Value {
    let mut snapshots = fs::read_dir(folder(runtime).join("snapshots"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .filter_map(|text| serde_json::from_str::<Value>(&text).ok())
        .collect::<Vec<_>>();
    snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot["assembled_at"].as_u64()));
    let ledger = read_ledger(runtime);
    let sessions = ledger
        .sessions
        .iter()
        .map(|(id, session)| {
            json!({
                "session": id,
                "at": session.at,
                "deliveries": session.delivered.iter().map(|(targets, delivered)| json!({
                    "targets": targets,
                    "digest": delivered.digest,
                    "at": delivered.at,
                    "bytes": delivered.bytes,
                    "reused": delivered.reused,
                })).collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    json!({"snapshots": snapshots, "sessions": sessions})
}

/** The targets of the store's enabled attachments that are active in governance (for the session
 * index)
 * Input
    - store: &BrickStore - signed store
 * Output
    - BTreeMap<String, Vec<String>> target key to attached subject references
*/
pub(crate) fn index(store: &BrickStore) -> BTreeMap<String, Vec<String>> {
    let mut index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for attachment in store
        .attachments
        .iter()
        .filter(|attachment| attachment.state == super::model::AttachmentState::Enabled)
    {
        index
            .entry(attachment.target.key())
            .or_default()
            .push(attachment.subject.text());
    }
    index
}
