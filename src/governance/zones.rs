// Zones, Flows, and Context Packs (.crane/governance.yaml). A Zone is a persistent governance
// boundary over repository resources (paths, files, directories, symbols, selections); a Flow is
// the execution path from entry points through the functions they call; a Context Pack is
// reusable, versioned guidance. Zone and Flow identities stay distinct even when they cover the
// same code, and every entity can reference policies and context packs (many-to-many).
//
// Zones and Flows only restrict. Where several cover a resource, the strictest autonomy ceiling and
// the highest criticality apply (deterministic overlap semantics), so a change to repository
// structure or to this file can never broaden authority beyond the policies. Only entities with
// status "active" are enforced; "proposed" ones are shown for review. Context is guidance, never
// authorization.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;

use serde::{Deserialize, Serialize};

use super::flows::{self, Graph, Index};
use super::policy::PolicySet;
use super::state::State;
use super::workspace::Workspace;
use super::{Finding, Severity};
use crate::platform::{git, glob};

/** File holding the zones, flows, and context packs */
pub(crate) const FILE: &str = "governance.yaml";

/** Default depth of flow discovery */
const DEFAULT_DEPTH: usize = 8;

/** How critical a resource is
 * Variants
    - Routine - ordinary code
    - Sensitive - deserves care
    - Critical - failures are severe
    - Restricted - changes always need a human
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Criticality {
    #[default]
    Routine,
    Sensitive,
    Critical,
    Restricted,
}

/** The most autonomy an agent may have within an entity (ordered from strictest)
 * Variants
    - Observe - no changes
    - Assisted - every change needs approval
    - Delegated - changes allowed within policy
    - Autonomous - changes allowed within policy (no stricter than delegated in 0.4)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Ceiling {
    Observe,
    Assisted,
    Delegated,
    Autonomous,
}

impl Default for Ceiling {
    /** The default ceiling: delegated
     * Input
        - None
     * Output
        - Ceiling
    */
    fn default() -> Self {
        Self::Delegated
    }
}

impl Ceiling {
    /** Return the ceiling's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Observe => "observe",
            Self::Assisted => "assisted",
            Self::Delegated => "delegated",
            Self::Autonomous => "autonomous",
        }
    }
}

/** Lifecycle of an entity
 * Variants
    - Proposed - shown for review, not enforced
    - Active - enforced
    - Deprecated - kept for history, not enforced
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Lifecycle {
    Proposed,
    #[default]
    Active,
    Deprecated,
}

/** What a zone covers
 * Fields
    - paths: Vec<String> - glob patterns
    - files: Vec<String> - exact files
    - directories: Vec<String> - directories (everything below)
    - symbols: Vec<String> - functions (SYMBOL, Type.SYMBOL, or path:SYMBOL); their files are covered
    - selections: Vec<String> - selection identifiers; their files are covered
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Selectors {
    pub(crate) paths: Vec<String>,
    pub(crate) files: Vec<String>,
    pub(crate) directories: Vec<String>,
    pub(crate) symbols: Vec<String>,
    pub(crate) selections: Vec<String>,
}

/** A zone
 * Fields
    - id: String - identifier
    - description: String - purpose
    - selectors: Selectors - what it covers
    - owner: Option<String> - owner
    - criticality: Criticality - criticality
    - autonomy_ceiling: Ceiling - most autonomy allowed
    - policies: Vec<String> - policy references
    - context_packs: Vec<String> - context pack references
    - status: Lifecycle - lifecycle
    - version: u32 - version
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Zone {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) selectors: Selectors,
    pub(crate) owner: Option<String>,
    pub(crate) criticality: Criticality,
    pub(crate) autonomy_ceiling: Ceiling,
    pub(crate) policies: Vec<String>,
    pub(crate) context_packs: Vec<String>,
    pub(crate) status: Lifecycle,
    pub(crate) version: u32,
}

/** A flow
 * Fields
    - id: String - identifier
    - description: String - purpose
    - entry_points: Vec<String> - SYMBOL, Type.SYMBOL, or path:SYMBOL
    - include: Vec<String> - extra paths (globs) belonging to the flow
    - exclude: Vec<String> - paths (globs) removed from the discovered flow
    - max_depth: Option<usize> - deepest call chain followed (default 8)
    - owner: Option<String> - owner
    - criticality: Criticality - criticality
    - autonomy_ceiling: Ceiling - most autonomy allowed
    - policies: Vec<String> - policy references
    - context_packs: Vec<String> - context pack references
    - status: Lifecycle - lifecycle
    - version: u32 - version
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Flow {
    pub(crate) id: String,
    pub(crate) description: String,
    pub(crate) entry_points: Vec<String>,
    pub(crate) include: Vec<String>,
    pub(crate) exclude: Vec<String>,
    pub(crate) max_depth: Option<usize>,
    pub(crate) owner: Option<String>,
    pub(crate) criticality: Criticality,
    pub(crate) autonomy_ceiling: Ceiling,
    pub(crate) policies: Vec<String>,
    pub(crate) context_packs: Vec<String>,
    pub(crate) status: Lifecycle,
    pub(crate) version: u32,
}

/** A context pack
 * Fields
    - id: String - identifier
    - version: u32 - version
    - description: String - purpose
    - content: Vec<String> - guidance lines
    - files: Vec<String> - repository text files included as guidance
    - owner: Option<String> - owner
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ContextPack {
    pub(crate) id: String,
    pub(crate) version: u32,
    pub(crate) description: String,
    pub(crate) content: Vec<String>,
    pub(crate) files: Vec<String>,
    pub(crate) owner: Option<String>,
}

/** The content of .crane/governance.yaml
 * Fields
    - version: u32 - layout version (1)
    - zones: Vec<Zone> - zones
    - flows: Vec<Flow> - flows
    - context_packs: Vec<ContextPack> - context packs
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Governance {
    pub(crate) version: u32,
    pub(crate) zones: Vec<Zone>,
    pub(crate) flows: Vec<Flow>,
    pub(crate) context_packs: Vec<ContextPack>,
}

/** Load .crane/governance.yaml (empty when missing)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Governance, String>
    - Error if the file is unreadable or not valid YAML for the schema
*/
pub(crate) fn load(workspace: &Workspace) -> Result<Governance, String> {
    match fs::read_to_string(workspace.file(FILE)) {
        Ok(text) => serde_yaml::from_str(&text).map_err(|error| format!(".crane/{FILE}: {error}")),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(Governance::default()),
        Err(error) => Err(format!("could not read .crane/{FILE}: {error}")),
    }
}

/** A zone or flow resolved against the repository
 * Fields
    - kind: &'static str - zone or flow
    - id: String - identifier
    - status: Lifecycle - lifecycle
    - criticality: Criticality - criticality
    - ceiling: Ceiling - autonomy ceiling
    - files: BTreeSet<String> - covered files
    - graph: Option<Graph> - discovered call graph (flows)
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Resolved {
    pub(crate) kind: &'static str,
    pub(crate) id: String,
    pub(crate) status: Lifecycle,
    pub(crate) criticality: Criticality,
    pub(crate) ceiling: Ceiling,
    pub(crate) files: BTreeSet<String>,
    pub(crate) graph: Option<Graph>,
}

/** Zones and flows resolved against the repository, with findings
 * Fields
    - entities: Vec<Resolved> - resolved zones and flows
    - findings: Vec<Finding> - problems (stale selectors, conflicts, unresolved flows, ...)
    - uncovered: Vec<String> - source files covered by no active zone or flow
    - analyzed: usize - files the flow analysis understood
    - unsupported: usize - source files in languages the flow analysis does not support
*/
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Resolution {
    pub(crate) entities: Vec<Resolved>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) uncovered: Vec<String>,
    pub(crate) analyzed: usize,
    pub(crate) unsupported: usize,
}

/** Build a finding about an entity
 * Input
    - code: &str - category
    - severity: Severity - seriousness
    - message: String - explanation
 * Output
    - Finding
*/
fn finding(code: &str, severity: Severity, message: String) -> Finding {
    Finding::new(code, severity, message)
}

/** Resolve zones and flows against the repository and validate the configuration: unique and
 * distinct identifiers, references to existing policies, context packs, and selections, stale
 * selectors, unresolved or partial flows, missing context files, overlapping scopes with
 * different limits (reported; the strictest applies), and uncovered source files
 * Input
    - workspace: &Workspace - repository
    - governance: &Governance - configuration
    - state: &State - governance state (for selections)
    - policies: &PolicySet - parsed policies
 * Output
    - Result<Resolution, String>
    - Error if Git cannot list the files
*/
pub(crate) fn resolve(
    workspace: &Workspace,
    governance: &Governance,
    state: &State,
    policies: &PolicySet,
) -> Result<Resolution, String> {
    let files = git::list_files(&workspace.root)?
        .into_iter()
        .filter(|path| !path.starts_with(".crane/"))
        .collect::<Vec<_>>();
    let mut resolution = Resolution::default();
    let mut ids: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut check_id = |kind: &'static str, id: &str, findings: &mut Vec<Finding>| {
        if crate::platform::validate_name(kind, id).is_err() {
            findings.push(finding(
                "governance_invalid_id",
                Severity::High,
                format!(
                    "{kind} id '{id}' must use letters, digits, '_' or '-', starting with a letter"
                ),
            ));
        }
        if let Some(other) = ids.insert(id.to_string(), kind) {
            findings.push(finding("governance_duplicate_id", Severity::High, format!("'{id}' is used by a {other} and a {kind}; zone, flow, and context pack identities must be distinct")));
        }
    };
    let packs = governance
        .context_packs
        .iter()
        .map(|pack| pack.id.clone())
        .collect::<BTreeSet<_>>();
    for pack in &governance.context_packs {
        check_id("context pack", &pack.id, &mut resolution.findings);
        for file in &pack.files {
            if !workspace.root.join(file).is_file() {
                resolution.findings.push(finding(
                    "context_stale",
                    Severity::Medium,
                    format!(
                        "context pack {} names {file}, which does not exist",
                        pack.id
                    ),
                ));
            }
        }
    }
    let references = |kind: &str,
                      id: &str,
                      policy_refs: &[String],
                      pack_refs: &[String],
                      findings: &mut Vec<Finding>| {
        for policy in policy_refs {
            if policies.find(policy).is_none() {
                findings.push(finding(
                    "governance_unknown_policy",
                    Severity::High,
                    format!("{kind} {id} references policy {policy}, which does not exist"),
                ));
            }
        }
        for pack in pack_refs {
            if !packs.contains(pack) {
                findings.push(finding(
                    "governance_unknown_context",
                    Severity::High,
                    format!("{kind} {id} references context pack {pack}, which does not exist"),
                ));
            }
        }
    };
    let needs_index = governance
        .flows
        .iter()
        .any(|flow| flow.status != Lifecycle::Deprecated)
        || governance
            .zones
            .iter()
            .any(|zone| !zone.selectors.symbols.is_empty());
    let index = if needs_index {
        let contents = files
            .iter()
            .filter(|path| {
                flows::supported(path) || crate::selection::anchors::language_for(path).is_some()
            })
            .filter_map(|path| {
                fs::read_to_string(workspace.root.join(path))
                    .ok()
                    .map(|text| (path.clone(), text))
            });
        Index::build(contents)
    } else {
        Index::default()
    };
    resolution.analyzed = index.analyzed;
    resolution.unsupported = index.unsupported;
    for zone in &governance.zones {
        check_id("zone", &zone.id, &mut resolution.findings);
        references(
            "zone",
            &zone.id,
            &zone.policies,
            &zone.context_packs,
            &mut resolution.findings,
        );
        let mut covered = BTreeSet::new();
        let stale = |selector: String, matched: usize, findings: &mut Vec<Finding>| {
            if matched == 0 {
                findings.push(finding("selector_stale", Severity::Medium, format!("zone {} selector {selector} matches nothing (stale after a rename or deletion?); it covers nothing and grants nothing", zone.id)));
            }
        };
        for pattern in &zone.selectors.paths {
            let matched = files
                .iter()
                .filter(|path| glob::matches(pattern, path))
                .cloned()
                .collect::<Vec<_>>();
            stale(
                format!("paths: {pattern}"),
                matched.len(),
                &mut resolution.findings,
            );
            covered.extend(matched);
        }
        for file in &zone.selectors.files {
            let matched = files
                .iter()
                .filter(|path| *path == file)
                .cloned()
                .collect::<Vec<_>>();
            stale(
                format!("files: {file}"),
                matched.len(),
                &mut resolution.findings,
            );
            covered.extend(matched);
        }
        for directory in &zone.selectors.directories {
            let prefix = format!("{}/", directory.trim_end_matches('/'));
            let matched = files
                .iter()
                .filter(|path| path.starts_with(&prefix))
                .cloned()
                .collect::<Vec<_>>();
            stale(
                format!("directories: {directory}"),
                matched.len(),
                &mut resolution.findings,
            );
            covered.extend(matched);
        }
        for symbol in &zone.selectors.symbols {
            let graph = index.discover(std::slice::from_ref(symbol), 0);
            if graph.status == "UNRESOLVED" {
                resolution.findings.push(finding(
                    "selector_stale",
                    Severity::Medium,
                    format!(
                        "zone {} symbol selector: {}",
                        zone.id,
                        graph.unresolved.join("; ")
                    ),
                ));
            }
            covered.extend(graph.files);
        }
        for id in &zone.selectors.selections {
            match state.record(id) {
                Some(record) => {
                    covered.insert(record.current.span.path.clone());
                }
                None => resolution.findings.push(finding(
                    "selector_stale",
                    Severity::High,
                    format!(
                        "zone {} references selection {id}, which is not registered",
                        zone.id
                    ),
                )),
            }
        }
        resolution.entities.push(Resolved {
            kind: "zone",
            id: zone.id.clone(),
            status: zone.status,
            criticality: zone.criticality,
            ceiling: zone.autonomy_ceiling,
            files: covered,
            graph: None,
        });
    }
    for flow in &governance.flows {
        check_id("flow", &flow.id, &mut resolution.findings);
        references(
            "flow",
            &flow.id,
            &flow.policies,
            &flow.context_packs,
            &mut resolution.findings,
        );
        if flow.entry_points.is_empty() {
            resolution.findings.push(finding(
                "flow_unresolved",
                Severity::High,
                format!("flow {} has no entry points", flow.id),
            ));
        }
        let graph = index.discover(&flow.entry_points, flow.max_depth.unwrap_or(DEFAULT_DEPTH));
        let mut covered = graph.files.clone();
        for pattern in &flow.include {
            covered.extend(
                files
                    .iter()
                    .filter(|path| glob::matches(pattern, path))
                    .cloned(),
            );
        }
        covered.retain(|path| {
            !flow
                .exclude
                .iter()
                .any(|pattern| glob::matches(pattern, path))
        });
        match graph.status {
            "UNRESOLVED" => resolution.findings.push(finding(
                "flow_unresolved",
                if flow.status == Lifecycle::Active { Severity::High } else { Severity::Medium },
                format!("flow {} is UNRESOLVED: {}", flow.id, graph.unresolved.iter().take(3).cloned().collect::<Vec<_>>().join("; ")),
            )),
            "PARTIAL" => resolution.findings.push(finding(
                "flow_partial",
                Severity::Low,
                format!("flow {} is partially resolved: {} calls are external, dynamic, or ambiguous (not followed)", flow.id, graph.unresolved.len()),
            )),
            _ => {}
        }
        resolution.entities.push(Resolved {
            kind: "flow",
            id: flow.id.clone(),
            status: flow.status,
            criticality: flow.criticality,
            ceiling: flow.autonomy_ceiling,
            files: covered,
            graph: Some(graph),
        });
    }
    let mut overlaps: BTreeMap<String, Vec<&Resolved>> = BTreeMap::new();
    for entity in resolution
        .entities
        .iter()
        .filter(|entity| entity.status == Lifecycle::Active)
    {
        for file in &entity.files {
            overlaps.entry(file.clone()).or_default().push(entity);
        }
    }
    let mut reported = BTreeSet::new();
    for (file, entities) in &overlaps {
        let ceilings = entities
            .iter()
            .map(|entity| entity.ceiling)
            .collect::<BTreeSet<_>>();
        if entities.len() > 1 && ceilings.len() > 1 {
            let names = entities
                .iter()
                .map(|entity| format!("{} {}", entity.kind, entity.id))
                .collect::<Vec<_>>()
                .join(", ");
            if reported.insert(names.clone()) {
                resolution.findings.push(finding(
                    "scope_overlap",
                    Severity::Low,
                    format!("{names} overlap (e.g. {file}) with different autonomy ceilings; the strictest ({}) applies", ceilings.iter().next().map_or("", |ceiling| ceiling.name())),
                ));
            }
        }
    }
    if !governance.zones.is_empty() || !governance.flows.is_empty() {
        resolution.uncovered = files
            .iter()
            .filter(|path| {
                crate::selection::anchors::language_for(path).is_some()
                    && !overlaps.contains_key(*path)
            })
            .cloned()
            .collect();
        if !resolution.uncovered.is_empty() {
            resolution.findings.push(finding(
                "resources_uncovered",
                Severity::Low,
                format!(
                    "{} source files are covered by no active zone or flow (first: {})",
                    resolution.uncovered.len(),
                    resolution.uncovered[0]
                ),
            ));
        }
    }
    Ok(resolution)
}

/** The limits that apply to one file
 * Fields
    - ceiling: Ceiling - strictest ceiling of the active entities covering it
    - criticality: Criticality - highest criticality
    - entities: Vec<String> - "zone:ID" and "flow:ID" of the covering entities
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Limit {
    pub(crate) ceiling: Ceiling,
    pub(crate) criticality: Criticality,
    pub(crate) entities: Vec<String>,
}

/** Limits by repository-relative file */
pub(crate) type Limits = BTreeMap<String, Limit>;

/** The cached limits and the fingerprint they were computed for
 * Fields
    - fingerprint: String - digest of the configuration, registry generation, and file metadata
    - limits: Limits - limits by file
*/
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct CachedLimits {
    fingerprint: String,
    limits: Limits,
}

/** Compute the per-file limits for the hooks, reusing a cache in the runtime directory while the
 * configuration, the registry generation, and every file's size and modification time are
 * unchanged (flow discovery reads every source file, which a hook must not do on every call)
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - cache_directory: &std::path::Path - runtime directory
 * Output
    - Result<Limits, String> limits (empty without a configuration)
    - Error if the configuration is invalid
*/
pub(crate) fn cached_limits(
    workspace: &Workspace,
    state: &State,
    cache_directory: &std::path::Path,
) -> Result<Limits, String> {
    let text = match fs::read_to_string(workspace.file(FILE)) {
        Ok(text) => text,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Limits::new()),
        Err(error) => return Err(format!("could not read .crane/{FILE}: {error}")),
    };
    let mut material = format!("{text}\n{}\n", state.generation());
    for path in git::list_files(&workspace.root)? {
        if let Ok(metadata) = fs::metadata(workspace.root.join(&path)) {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |duration| duration.as_nanos());
            material.push_str(&format!("{path}\t{}\t{modified}\n", metadata.len()));
        }
    }
    let fingerprint = crate::trust::crypto::sha512_hex(material.as_bytes());
    let path = cache_directory.join("zone-limits.json");
    if let Some(cached) = fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<CachedLimits>(&text).ok())
    {
        if cached.fingerprint == fingerprint {
            return Ok(cached.limits);
        }
    }
    let governance = load(workspace)?;
    let policies = PolicySet::load(workspace)?;
    let resolution = resolve(workspace, &governance, state, &policies)?;
    let mut limits = Limits::new();
    for entity in resolution
        .entities
        .iter()
        .filter(|entity| entity.status == Lifecycle::Active)
    {
        for file in &entity.files {
            let entry = limits.entry(file.clone()).or_insert(Limit {
                ceiling: Ceiling::Autonomous,
                criticality: Criticality::Routine,
                entities: Vec::new(),
            });
            entry.ceiling = entry.ceiling.min(entity.ceiling);
            entry.criticality = entry.criticality.max(entity.criticality);
            entry
                .entities
                .push(format!("{}:{}", entity.kind, entity.id));
        }
    }
    let cached = CachedLimits {
        fingerprint,
        limits: limits.clone(),
    };
    let _ = crate::platform::files::write_atomic(
        &path,
        serde_json::to_string(&cached)
            .unwrap_or_default()
            .as_bytes(),
    );
    Ok(limits)
}

/** Compose the context packs for a session: the packs referenced by active zones and flows (and
 * every pack when there are none), deduplicated and ordered by identifier, with their file text
 * Input
    - workspace: &Workspace - repository
    - governance: &Governance - configuration
 * Output
    - Vec<(String, u32, String)> pack id, version, and text
*/
pub(crate) fn compose_context(
    workspace: &Workspace,
    governance: &Governance,
) -> Vec<(String, u32, String)> {
    let referenced = governance
        .zones
        .iter()
        .filter(|zone| zone.status == Lifecycle::Active)
        .flat_map(|zone| zone.context_packs.iter())
        .chain(
            governance
                .flows
                .iter()
                .filter(|flow| flow.status == Lifecycle::Active)
                .flat_map(|flow| flow.context_packs.iter()),
        )
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut packs = governance
        .context_packs
        .iter()
        .filter(|pack| referenced.is_empty() || referenced.contains(&pack.id))
        .collect::<Vec<_>>();
    packs.sort_by(|left, right| left.id.cmp(&right.id));
    packs
        .into_iter()
        .map(|pack| {
            let mut text = pack
                .content
                .iter()
                .map(|line| format!("- {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            for file in &pack.files {
                if let Ok(content) = fs::read_to_string(workspace.root.join(file)) {
                    text.push_str(&format!("\n[{file}]\n{}", content.trim_end()));
                }
            }
            (pack.id.clone(), pack.version, text)
        })
        .collect()
}

/** Most guidance supplied with one tool call, in bytes */
const GUIDANCE_LIMIT: usize = 8_000;

/** Compose the guidance for the active zones and flows a tool call touches: each entity's kind,
 * identifier, criticality, autonomy ceiling, and description, followed by the content (and files)
 * of its context packs. Supplied with the tool call, so the agent sees it whenever it works on
 * covered code, not only at the start of the session; it is guidance, never authorization.
 * Input
    - workspace: &Workspace - repository
    - governance: &Governance - configuration
    - entities: &BTreeSet<String> - "zone:ID" and "flow:ID" of the entities covering the files
 * Output
    - String, empty when none of them is active; at most GUIDANCE_LIMIT bytes
*/
pub(crate) fn guidance(
    workspace: &Workspace,
    governance: &Governance,
    entities: &BTreeSet<String>,
) -> String {
    let pack_text = |id: &String| {
        governance
            .context_packs
            .iter()
            .find(|pack| &pack.id == id)
            .map(|pack| {
                let mut text = pack
                    .content
                    .iter()
                    .map(|line| format!("  - {line}\n"))
                    .collect::<String>();
                for file in &pack.files {
                    if let Ok(content) = fs::read_to_string(workspace.root.join(file)) {
                        text.push_str(&format!("  [{file}]\n{}\n", content.trim_end()));
                    }
                }
                text
            })
            .unwrap_or_default()
    };
    let mut sections = Vec::new();
    let mut section = |kind: &str,
                       id: &str,
                       criticality: Criticality,
                       ceiling: Ceiling,
                       description: &str,
                       packs: &[String]| {
        let mut text = format!(
            "{kind} {id} ({}, autonomy ceiling {}){}\n",
            format!("{criticality:?}").to_ascii_lowercase(),
            ceiling.name(),
            if description.is_empty() {
                String::new()
            } else {
                format!(": {description}")
            }
        );
        for pack in packs {
            text.push_str(&pack_text(pack));
        }
        sections.push(text);
    };
    for zone in governance.zones.iter().filter(|zone| {
        zone.status == Lifecycle::Active && entities.contains(&format!("zone:{}", zone.id))
    }) {
        section(
            "Zone",
            &zone.id,
            zone.criticality,
            zone.autonomy_ceiling,
            &zone.description,
            &zone.context_packs,
        );
    }
    for flow in governance.flows.iter().filter(|flow| {
        flow.status == Lifecycle::Active && entities.contains(&format!("flow:{}", flow.id))
    }) {
        section(
            "Flow",
            &flow.id,
            flow.criticality,
            flow.autonomy_ceiling,
            &flow.description,
            &flow.context_packs,
        );
    }
    if sections.is_empty() {
        return String::new();
    }
    let mut text = format!(
        "Crane guidance for the code this tool call touches (guidance only, never authorization):\n{}",
        sections.concat()
    );
    if text.len() > GUIDANCE_LIMIT {
        let mut end = GUIDANCE_LIMIT;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[guidance truncated]");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check that the suggested YAML layout parses and unknown top-level keys are rejected
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn parses_governance_yaml() {
        let text = "version: 1\nzones:\n  - id: payment-core\n    description: Core payment resources\n    selectors:\n      paths: [\"src/payments/**\"]\n    criticality: critical\n    autonomy_ceiling: delegated\n    policies: [payment-integrity]\n    context_packs: [payment-invariants]\nflows:\n  - id: payment-submission\n    entry_points: [\"PaymentService.submitPayment\"]\n    criticality: critical\n    autonomy_ceiling: assisted\ncontext_packs:\n  - id: payment-invariants\n    version: 1\n    content:\n      - Payment processing must be idempotent.\n";
        let governance: Governance = serde_yaml::from_str(text).unwrap();
        assert_eq!(governance.zones[0].criticality, Criticality::Critical);
        assert_eq!(governance.flows[0].autonomy_ceiling, Ceiling::Assisted);
        assert_eq!(governance.zones[0].status, Lifecycle::Active);
        assert_eq!(governance.context_packs[0].content.len(), 1);
        assert!(serde_yaml::from_str::<Governance>("version: 1\nzonez: []\n").is_err());
        assert!(Ceiling::Observe < Ceiling::Assisted && Ceiling::Assisted < Ceiling::Delegated);
    }
}
