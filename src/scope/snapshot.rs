// Boundary snapshots: the effective boundary per file (autonomy ceiling, criticality, covering
// zones and flows, zone and flow source blocks, preserved and target selections, and the content
// digest), plus the digests of source files nothing governs. Two snapshots are compared to find
// where the boundary widened, narrowed, or moved.
//
// Zones and flows only restrict, but they select files by path, so repository structure can still
// widen the boundary without anyone signing anything: a governed file renamed out of a zone's glob
// is governed by nothing. The diff finds that by content: a file that leaves the governed set while
// a file with the same content appears among the ungoverned ones has moved out of scope.
//
// A baseline is kept in runtime/scope/baseline.json and recorded after 'crane governance sign',
// 'crane protect', and 'crane target', so 'crane scope diff' reports what changed since the rules
// were last signed. The baseline is local evidence, not authority: it never changes a decision.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::resource::content_digest;
use super::{Boundary, Versions};
use crate::governance::workspace::Workspace;
use crate::governance::zones::{Ceiling, Criticality};
use crate::platform::files::write_atomic;
use crate::platform::git;
use crate::selection::anchors::language_for;
use crate::selection::registry::Operation as SelectionOperation;

/** Version of the snapshot layout */
pub(crate) const SNAPSHOT_VERSION: u32 = 1;

/** The boundary of one governed file
 * Fields
    - ceiling: Ceiling - file-wide autonomy ceiling (autonomous when only blocks cover it)
    - criticality: Criticality - file-wide criticality
    - block_ceiling: Option<Ceiling> - strictest ceiling of the zone and flow blocks inside it
    - entities: Vec<String> - covering zones and flows ("zone:ID", "flow:ID")
    - preserved: Vec<String> - preserved selections in it
    - targets: Vec<String> - target selections in it
    - content: Option<String> - content digest
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileScope {
    pub(crate) ceiling: Ceiling,
    pub(crate) criticality: Criticality,
    pub(crate) block_ceiling: Option<Ceiling>,
    pub(crate) entities: Vec<String>,
    pub(crate) preserved: Vec<String>,
    pub(crate) targets: Vec<String>,
    pub(crate) content: Option<String>,
}

/** The effective boundary of a repository at one moment
 * Fields
    - version: u32 - SNAPSHOT_VERSION
    - recorded_at: u64 - Unix milliseconds
    - versions: Versions - rule versions
    - integrity_ok: bool - the signed state verified
    - zone_error: Option<String> - why zones could not be resolved (changes then fail closed)
    - files: BTreeMap<String, FileScope> - governed files
    - ungoverned: BTreeMap<String, String> - source files nothing governs, with content digests
      (empty when nothing is governed at all)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub(crate) version: u32,
    pub(crate) recorded_at: u64,
    pub(crate) versions: Versions,
    pub(crate) integrity_ok: bool,
    pub(crate) zone_error: Option<String>,
    pub(crate) files: BTreeMap<String, FileScope>,
    pub(crate) ungoverned: BTreeMap<String, String>,
}

/** Capture the boundary as it is now
 * Input
    - boundary: &Boundary - the boundary
 * Output
    - Result<Snapshot, String>
    - Error if Git cannot list the files
*/
pub(crate) fn capture(boundary: &Boundary) -> Result<Snapshot, String> {
    let root = &boundary.workspace.root;
    let mut files = BTreeMap::<String, FileScope>::new();
    let blank = || FileScope {
        ceiling: Ceiling::Autonomous,
        criticality: Criticality::Routine,
        block_ceiling: None,
        entities: Vec::new(),
        preserved: Vec::new(),
        targets: Vec::new(),
        content: None,
    };
    for (path, limit) in &boundary.enforcement.limits {
        let entry = files.entry(path.clone()).or_insert_with(blank);
        if !limit.entities.is_empty() {
            entry.ceiling = limit.ceiling;
            entry.criticality = limit.criticality;
        }
        entry.entities.extend(limit.entities.iter().cloned());
        for block in &limit.scopes {
            entry.block_ceiling = Some(
                entry
                    .block_ceiling
                    .map_or(block.ceiling, |ceiling| ceiling.min(block.ceiling)),
            );
            entry.criticality = entry.criticality.max(block.criticality);
            entry.entities.extend(block.entities.iter().cloned());
        }
    }
    for record in &boundary.state.registry.selections {
        let entry = files
            .entry(record.current.span.path.clone())
            .or_insert_with(blank);
        match record.operation {
            SelectionOperation::Preserve => entry.preserved.push(record.id.clone()),
            SelectionOperation::Target => entry.targets.push(record.id.clone()),
        }
    }
    for (path, entry) in &mut files {
        entry.entities.sort();
        entry.entities.dedup();
        entry.preserved.sort();
        entry.targets.sort();
        entry.content = content_digest(&root.join(path));
    }
    let ungoverned = if files.is_empty() {
        BTreeMap::new()
    } else {
        git::list_files(root)?
            .into_iter()
            .filter(|path| !path.starts_with(".crane/") && !files.contains_key(path))
            .filter(|path| language_for(path).is_some())
            .filter_map(|path| content_digest(&root.join(&path)).map(|digest| (path, digest)))
            .collect()
    };
    Ok(Snapshot {
        version: SNAPSHOT_VERSION,
        recorded_at: crate::platform::now_millis(),
        versions: boundary.versions(),
        integrity_ok: boundary.integrity_ok(),
        zone_error: boundary.enforcement.zone_error.clone(),
        files,
        ungoverned,
    })
}

/** The path of the baseline snapshot
 * Input
    - runtime: &Path - the repository's runtime directory
 * Output
    - std::path::PathBuf
*/
fn baseline_path(runtime: &Path) -> std::path::PathBuf {
    runtime.join("scope").join("baseline.json")
}

/** Read the baseline snapshot
 * Input
    - runtime: &Path - the repository's runtime directory
 * Output
    - Result<Option<Snapshot>, String> - None when no baseline was recorded
    - Error if the baseline is unreadable or of another layout version
*/
pub(crate) fn load_baseline(runtime: &Path) -> Result<Option<Snapshot>, String> {
    let path = baseline_path(runtime);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not read {}: {error}", path.display())),
    };
    let snapshot: Snapshot = serde_json::from_str(&text).map_err(|error| {
        format!(
            "{} is not a valid boundary snapshot: {error}",
            path.display()
        )
    })?;
    if snapshot.version != SNAPSHOT_VERSION {
        return Err(format!(
            "the boundary baseline has layout version {}; record a new one with 'crane scope diff --record'",
            snapshot.version
        ));
    }
    Ok(Some(snapshot))
}

/** Record a snapshot as the baseline
 * Input
    - runtime: &Path - the repository's runtime directory
    - snapshot: &Snapshot - snapshot
 * Output
    - Result<(), String>
*/
pub(crate) fn record_baseline(runtime: &Path, snapshot: &Snapshot) -> Result<(), String> {
    let text = serde_json::to_string_pretty(snapshot).map_err(|error| error.to_string())?;
    write_atomic(&baseline_path(runtime), text.as_bytes())
}

/** Record the current boundary as the baseline after a signed rule change (best effort: a failure
 * is returned for the caller to ignore or report, and never undoes the signed change)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<(), String>
*/
pub(crate) fn record_current(workspace: &Workspace) -> Result<(), String> {
    let boundary = Boundary::load(workspace.clone())?;
    record_baseline(&boundary.runtime, &capture(&boundary)?)
}

/** Kinds of boundary change
 * Variants
    - Widened - agents may do more than before (a weaker ceiling or criticality, lost protection,
      or a file that left the governed set)
    - Narrowed - agents may do less than before
    - Changed - the boundary moved without widening (a governed file moved or was removed)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ChangeKind {
    Widened,
    Narrowed,
    Changed,
}

/** One change of the boundary
 * Fields
    - path: String - file
    - kind: ChangeKind - kind
    - detail: String - what changed
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Change {
    pub(crate) path: String,
    pub(crate) kind: ChangeKind,
    pub(crate) detail: String,
}

/** The difference between two snapshots
 * Fields
    - changes: Vec<Change> - changes, widenings first
    - notes: Vec<String> - context (re-signed rules, integrity, zone configuration)
*/
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Difference {
    pub(crate) changes: Vec<Change>,
    pub(crate) notes: Vec<String>,
}

impl Difference {
    /** Report whether the boundary widened
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn widened(&self) -> bool {
        self.changes
            .iter()
            .any(|change| change.kind == ChangeKind::Widened)
    }
}

/** Compare the restrictions of one file before and after
 * Input
    - before: &FileScope - earlier boundary
    - after: &FileScope - later boundary
 * Output
    - (Vec<String>, Vec<String>) widenings and narrowings
*/
fn compare(before: &FileScope, after: &FileScope) -> (Vec<String>, Vec<String>) {
    let mut wider = Vec::new();
    let mut narrower = Vec::new();
    let (b, a) = (before.ceiling, after.ceiling);
    if a > b {
        wider.push(format!("autonomy ceiling {} -> {}", b.name(), a.name()));
    } else if a < b {
        narrower.push(format!("autonomy ceiling {} -> {}", b.name(), a.name()));
    }
    let (b, a) = (before.criticality, after.criticality);
    if a < b {
        wider.push(format!("criticality {b:?} -> {a:?}").to_lowercase());
    } else if a > b {
        narrower.push(format!("criticality {b:?} -> {a:?}").to_lowercase());
    }
    match (before.block_ceiling, after.block_ceiling) {
        (Some(b), None) => wider.push(format!("its {} zone or flow blocks are gone", b.name())),
        (Some(b), Some(a)) if a > b => {
            wider.push(format!("block ceiling {} -> {}", b.name(), a.name()))
        }
        (Some(b), Some(a)) if a < b => {
            narrower.push(format!("block ceiling {} -> {}", b.name(), a.name()))
        }
        (None, Some(a)) => narrower.push(format!("new {} zone or flow blocks", a.name())),
        _ => {}
    }
    let lost = before
        .preserved
        .iter()
        .filter(|id| !after.preserved.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    if !lost.is_empty() {
        wider.push(format!(
            "preserved selections no longer here: {}",
            lost.join(", ")
        ));
    }
    let gained = after
        .preserved
        .iter()
        .filter(|id| !before.preserved.contains(id))
        .cloned()
        .collect::<Vec<_>>();
    if !gained.is_empty() {
        narrower.push(format!("newly preserved: {}", gained.join(", ")));
    }
    let left = before
        .entities
        .iter()
        .filter(|entity| !after.entities.contains(entity))
        .cloned()
        .collect::<Vec<_>>();
    if !left.is_empty() && wider.is_empty() {
        wider.push(format!("no longer covered by {}", left.join(", ")));
    }
    (wider, narrower)
}

/** Compare two snapshots
 * Input
    - before: &Snapshot - earlier boundary (the baseline)
    - after: &Snapshot - later boundary (now)
 * Output
    - Difference
*/
pub(crate) fn diff(before: &Snapshot, after: &Snapshot) -> Difference {
    let mut difference = Difference::default();
    if before.versions.manifest_head != after.versions.manifest_head {
        difference.notes.push(format!(
            "the rules were re-signed since the baseline (generation {} -> {}); changes they made are intentional, changes from repository structure may not be",
            before.versions.registry_generation, after.versions.registry_generation
        ));
    }
    if before.integrity_ok && !after.integrity_ok {
        difference.notes.push("the signed state no longer verifies: agent changes are denied until a person runs 'crane validate'".into());
    }
    if let Some(error) = after
        .zone_error
        .as_ref()
        .filter(|_| before.zone_error.is_none())
    {
        difference.notes.push(format!("zones and flows can no longer be resolved ({error}); agent changes are denied until fixed"));
    }
    let mut push = |path: &str, kind: ChangeKind, detail: String| {
        difference.changes.push(Change {
            path: path.into(),
            kind,
            detail,
        });
    };
    for (path, old) in &before.files {
        if let Some(new) = after.files.get(path) {
            let (wider, narrower) = compare(old, new);
            if !wider.is_empty() {
                push(path, ChangeKind::Widened, wider.join("; "));
            }
            if !narrower.is_empty() {
                push(path, ChangeKind::Narrowed, narrower.join("; "));
            }
            continue;
        }
        let moved_governed = old.content.as_ref().and_then(|digest| {
            after.files.iter().find(|(other, scope)| {
                scope.content.as_ref() == Some(digest) && !before.files.contains_key(*other)
            })
        });
        let moved_ungoverned = old.content.as_ref().and_then(|digest| {
            after
                .ungoverned
                .iter()
                .find(|(_, other)| *other == digest)
                .map(|(other, _)| other.clone())
        });
        if let Some((other, scope)) = moved_governed {
            let (wider, _) = compare(old, scope);
            if wider.is_empty() {
                push(
                    path,
                    ChangeKind::Changed,
                    format!("moved to {other}, which is governed at least as strictly"),
                );
            } else {
                push(
                    path,
                    ChangeKind::Widened,
                    format!(
                        "moved to {other}, where it is less restricted: {}",
                        wider.join("; ")
                    ),
                );
            }
        } else if let Some(other) = moved_ungoverned {
            push(
                path,
                ChangeKind::Widened,
                format!("moved to {other}, which no zone, flow, or selection governs"),
            );
        } else if after.ungoverned.contains_key(path) {
            push(
                path,
                ChangeKind::Widened,
                "no zone, flow, or selection governs it any more".into(),
            );
        } else {
            push(
                path,
                ChangeKind::Changed,
                "removed from the repository".into(),
            );
        }
    }
    let moved_targets = difference
        .changes
        .iter()
        .filter_map(|change| change.detail.strip_prefix("moved to "))
        .filter_map(|rest| rest.split(',').next())
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    for (path, scope) in &after.files {
        if !before.files.contains_key(path) && !moved_targets.contains(path) {
            let mut governed_by = scope.entities.clone();
            governed_by.extend(scope.preserved.iter().map(|id| format!("preserved {id}")));
            governed_by.extend(scope.targets.iter().map(|id| format!("target {id}")));
            difference.changes.push(Change {
                path: path.clone(),
                kind: ChangeKind::Narrowed,
                detail: format!("newly governed by {}", governed_by.join(", ")),
            });
        }
    }
    difference
        .changes
        .sort_by(|a, b| a.kind.cmp(&b.kind).then(a.path.cmp(&b.path)));
    difference
}

#[cfg(test)]
mod tests {
    use super::{diff, ChangeKind, FileScope, Snapshot, SNAPSHOT_VERSION};
    use crate::governance::zones::{Ceiling, Criticality};
    use crate::scope::Versions;
    use std::collections::BTreeMap;

    /** A governed file
     * Input
        - ceiling: Ceiling - ceiling
        - content: &str - content digest
     * Output
        - FileScope
    */
    fn governed(ceiling: Ceiling, content: &str) -> FileScope {
        FileScope {
            ceiling,
            criticality: Criticality::Critical,
            block_ceiling: None,
            entities: vec!["zone:ledger".into()],
            preserved: Vec::new(),
            targets: Vec::new(),
            content: Some(content.into()),
        }
    }

    /** A snapshot
     * Input
        - files: Vec<(&str, FileScope)> - governed files
        - ungoverned: Vec<(&str, &str)> - ungoverned files and digests
     * Output
        - Snapshot
    */
    fn snapshot(files: Vec<(&str, FileScope)>, ungoverned: Vec<(&str, &str)>) -> Snapshot {
        Snapshot {
            version: SNAPSHOT_VERSION,
            recorded_at: 0,
            versions: Versions::default(),
            integrity_ok: true,
            zone_error: None,
            files: files
                .into_iter()
                .map(|(path, scope)| (path.into(), scope))
                .collect(),
            ungoverned: ungoverned
                .into_iter()
                .map(|(path, digest)| (path.to_string(), digest.to_string()))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    /** A governed file renamed out of its zone is a widening; a weaker ceiling is a widening; a
     * stricter one a narrowing; an unchanged boundary has no changes
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn renames_out_of_scope_widen_the_boundary() {
        let before = snapshot(
            vec![("app/ledger.py", governed(Ceiling::Observe, "d1"))],
            vec![("web/app.js", "d2")],
        );
        assert!(diff(&before, &before).changes.is_empty());
        let renamed = snapshot(vec![], vec![("web/app.js", "d2"), ("app/books.py", "d1")]);
        let difference = diff(&before, &renamed);
        assert!(difference.widened());
        assert!(
            difference.changes[0]
                .detail
                .contains("moved to app/books.py"),
            "{difference:?}"
        );
        let loosened = snapshot(
            vec![("app/ledger.py", governed(Ceiling::Delegated, "d1"))],
            vec![],
        );
        assert_eq!(
            diff(&before, &loosened).changes[0].kind,
            ChangeKind::Widened
        );
        let tightened = diff(&loosened, &before);
        assert!(!tightened.widened());
        assert_eq!(tightened.changes[0].kind, ChangeKind::Narrowed);
        let moved_governed = snapshot(
            vec![("app/ledger2.py", governed(Ceiling::Observe, "d1"))],
            vec![],
        );
        let difference = diff(&before, &moved_governed);
        assert!(!difference.widened(), "{difference:?}");
        assert_eq!(difference.changes.len(), 1, "{difference:?}");
    }
}
