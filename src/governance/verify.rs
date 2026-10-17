// Verification of the whole governance state, shared by validate, test, policy status, and the
// runtime hooks. It never trusts a single source: the signed state is verified first, policy
// commands must agree with the signed registry, every selection is resolved from the anchors in
// the work tree, and preserve and target commands are judged from the actual file content against
// the checkpoint baseline, never from an agent's claims. A policy passes only if every one of its
// commands passes; an unresolved selection fails closed.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::changes::{classify, judge, ChangeType, Summary};
use super::config::Config;
use super::policy::{stray_policy_files, PolicySet};
use super::state::{integrity, State};
use super::workspace::Workspace;
use super::{Finding, Severity};
use crate::platform::git;
use crate::selection::anchors::{language_for, lines, Boundary, CommentSyntax, MarkerKind};
use crate::selection::registry::{content_digest, Operation, Record, Span};
use crate::selection::tracker::{
    candidates, normalize_content, resolve, scan_findings, scan_paths, scan_repository, Resolution,
    Scan,
};

/** The result of one command
 * Variants
    - Pass - the command holds
    - Fail - the command is violated
    - Unresolved - the selection or its baseline could not be established; fails closed
    - Pending - a target that has not been changed yet, while a session is still working
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub(crate) enum Outcome {
    Pass,
    Fail,
    Unresolved,
    Pending,
}

/** The evaluation of one policy command
 * Fields
    - policy: String - policy name
    - file: String - policy file
    - line: usize - line of the command
    - operation: Operation - preserve or target
    - id: String - selection identifier
    - change_type: Option<ChangeType> - required change kind
    - outcome: Outcome - result
    - message: String - explanation
    - location: Option<Span> - where the selection is now, when resolved
    - change: Option<Summary> - classified difference from the baseline, when computed
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CommandResult {
    pub(crate) policy: String,
    pub(crate) file: String,
    pub(crate) line: usize,
    pub(crate) operation: Operation,
    pub(crate) id: String,
    pub(crate) change_type: Option<ChangeType>,
    pub(crate) outcome: Outcome,
    pub(crate) message: String,
    pub(crate) location: Option<Span>,
    pub(crate) change: Option<Summary>,
}

/** The evaluation of one policy
 * Fields
    - name: String - policy name
    - file: String - policy file
    - passed: bool - every command passed
    - commands: Vec<CommandResult> - command results in source order
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PolicyResult {
    pub(crate) name: String,
    pub(crate) file: String,
    pub(crate) passed: bool,
    pub(crate) commands: Vec<CommandResult>,
}

/** A selection whose markers now live in another file
 * Fields
    - id: String - selection identifier
    - from: String - path recorded in the registry
    - to: String - path where the markers resolve now
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Relocation {
    pub(crate) id: String,
    pub(crate) from: String,
    pub(crate) to: String,
}

/** The full verification report
 * Fields
    - generation: u64 - registry generation verified
    - findings: Vec<Finding> - integrity, consistency, and resolution problems
    - policies: Vec<PolicyResult> - command evaluations by policy
    - relocations: Vec<Relocation> - selections that moved to another file
    - resolutions: BTreeMap<String, Option<Span>> - where every selection resolves, None when
      unresolved
    - files_scanned: usize - files scanned for markers
    - passed: bool - no blocking finding and every policy passed
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Report {
    pub(crate) generation: u64,
    pub(crate) findings: Vec<Finding>,
    pub(crate) policies: Vec<PolicyResult>,
    pub(crate) relocations: Vec<Relocation>,
    pub(crate) resolutions: BTreeMap<String, Option<Span>>,
    pub(crate) files_scanned: usize,
    pub(crate) passed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) zones: Option<super::zones::Resolution>,
}

impl Report {
    /** Report whether any finding is blocking
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn has_blocking_findings(&self) -> bool {
        self.findings.iter().any(Finding::blocking)
    }

    /** List every command result that failed or is unresolved
     * Input
        - None (uses self)
     * Output
        - Vec<&CommandResult>
    */
    pub(crate) fn failures(&self) -> Vec<&CommandResult> {
        self.policies
            .iter()
            .flat_map(|policy| policy.commands.iter())
            .filter(|command| matches!(command.outcome, Outcome::Fail | Outcome::Unresolved))
            .collect()
    }
}

/** How much of the work tree to scan
 * Variants
    - Full - every file Git lists
    - Paths(Vec<String>) - only these files and the files the registry last saw selections in;
      a selection missing from them triggers a full scan, so a move is never misreported
*/
#[derive(Debug, Clone)]
pub(crate) enum ScanMode {
    Full,
    Paths(Vec<String>),
}

/** Options of an evaluation
 * Fields
    - scan: ScanMode - what to scan
    - targets_pending: bool - report unchanged targets as Pending instead of Fail (mid-session)
    - candidates: bool - search for the baseline content of selections whose markers are missing
    - governance: bool - resolve and validate zones, flows, and context packs
*/
#[derive(Debug, Clone)]
pub(crate) struct Options {
    pub(crate) scan: ScanMode,
    pub(crate) targets_pending: bool,
    pub(crate) candidates: bool,
    pub(crate) governance: bool,
}

impl Default for Options {
    /** Full scan, unchanged targets fail, candidate search on
     * Input
        - None
     * Output
        - Options
    */
    fn default() -> Self {
        Self {
            scan: ScanMode::Full,
            targets_pending: false,
            candidates: true,
            governance: true,
        }
    }
}

/** Read the baseline content of a selection from its checkpoint commit and check it against the
 * signed origin content digest
 * Input
    - workspace: &Workspace - repository
    - record: &Record - selection record
 * Output
    - Result<String, String> normalized baseline content
    - Error if the checkpoint file is unavailable or no longer matches the signed digest
*/
pub(crate) fn baseline(workspace: &Workspace, record: &Record) -> Result<String, String> {
    let bytes = git::show(
        &workspace.root,
        &record.origin.commit,
        &record.origin.span.path,
    )?
    .ok_or_else(|| {
        format!(
            "the checkpoint version of {} is not available (commit {} missing locally?)",
            record.origin.span.path, record.origin.commit
        )
    })?;
    let text = String::from_utf8(bytes).map_err(|_| {
        format!(
            "the checkpoint version of {} is not text",
            record.origin.span.path
        )
    })?;
    let content = text
        .get(record.origin.span.start_byte..record.origin.span.end_byte)
        .map(normalize_content)
        .ok_or("the checkpoint range is outside the checkpoint file")?;
    if content_digest(&content) != record.origin.content_digest {
        return Err("the checkpoint content no longer matches the signed origin digest".into());
    }
    Ok(content)
}

/** Return the comment syntax of a path, defaulting to '#' line comments
 * Input
    - path: &str - repository-relative path
 * Output
    - CommentSyntax
*/
fn syntax_of(path: &str) -> CommentSyntax {
    language_for(path).map_or(CommentSyntax::Line("#"), |language| language.syntax)
}

fn unchanged_except_nested_targets(
    workspace: &Workspace,
    parent: &Record,
    location: &crate::selection::tracker::Location,
    state: &State,
    scan: &Scan,
) -> bool {
    let Some(current_text) = scan.texts.get(&location.span.path) else {
        return false;
    };
    let mut nested = Vec::new();
    for child in state
        .registry
        .selections
        .iter()
        .filter(|child| child.operation == Operation::Target && child.id != parent.id)
    {
        let starts = scan
            .markers
            .iter()
            .filter(|(path, marker)| {
                path == &location.span.path
                    && marker.kind == MarkerKind::Selection
                    && marker.id.as_deref() == Some(child.id.as_str())
                    && marker.boundary == Some(Boundary::Start)
                    && marker.problem.is_none()
            })
            .collect::<Vec<_>>();
        let ends = scan
            .markers
            .iter()
            .filter(|(path, marker)| {
                path == &location.span.path
                    && marker.kind == MarkerKind::Selection
                    && marker.id.as_deref() == Some(child.id.as_str())
                    && marker.boundary == Some(Boundary::End)
                    && marker.problem.is_none()
            })
            .collect::<Vec<_>>();
        if let ([start], [end]) = (starts.as_slice(), ends.as_slice()) {
            if start.1.line > location.start_marker_line && end.1.line < location.end_marker_line {
                if child.origin.commit != parent.origin.commit
                    || child.origin.span.path != parent.origin.span.path
                    || child.origin.span.start_line <= parent.origin.span.start_line
                    || child.origin.span.end_line >= parent.origin.span.end_line
                {
                    return false;
                }
                nested.push((
                    start.1.line,
                    end.1.line,
                    child.origin.span.start_line,
                    child.origin.span.end_line,
                ));
            }
        }
    }
    if nested.is_empty() {
        return false;
    }
    let Ok(baseline_text) = baseline(workspace, parent) else {
        return false;
    };
    let baseline_lines = baseline_text.lines().collect::<Vec<_>>();
    let current_lines = lines(current_text);
    let marker_lines = scan
        .markers
        .iter()
        .filter(|(path, marker)| {
            path == &location.span.path && marker.kind == MarkerKind::Selection
        })
        .map(|(_, marker)| marker.line)
        .collect::<BTreeSet<_>>();
    let mut baseline_projected = String::new();
    for (index, line) in baseline_lines.iter().enumerate() {
        let absolute = parent.origin.span.start_line + index;
        if nested
            .iter()
            .any(|(_, _, start, end)| absolute >= *start && absolute <= *end)
        {
            continue;
        }
        baseline_projected.push_str(line);
        baseline_projected.push('\n');
    }
    let mut current_projected = String::new();
    for line in current_lines.iter().filter(|line| {
        line.number > location.start_marker_line && line.number < location.end_marker_line
    }) {
        if marker_lines.contains(&line.number)
            || nested
                .iter()
                .any(|(start, end, _, _)| line.number > *start && line.number < *end)
        {
            continue;
        }
        current_projected.push_str(line.text);
        current_projected.push('\n');
    }
    normalize_content(&baseline_projected) == normalize_content(&current_projected)
}

/** Check the consistency of policies, defaults, and registry: syntax errors, policy files outside
 * .crane/policies, duplicate or case-ambiguous policy names, missing defaults, commands that do
 * not match their signed record, and selections without exactly one command
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - policies: &PolicySet - parsed policies
 * Output
    - Vec<Finding>
*/
fn consistency(workspace: &Workspace, state: &State, policies: &PolicySet) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (path, error) in &policies.errors {
        findings.push(
            Finding::new("policy_syntax", Severity::High, format!("{path}: {error}"))
                .with_path(path),
        );
    }
    match stray_policy_files(workspace) {
        Ok(stray) => {
            for path in stray {
                findings.push(
                    Finding::new(
                        "policy_outside_folder",
                        Severity::High,
                        format!("{path} is a policy file outside .crane/policies; policies must be written in .crane/policies"),
                    )
                    .with_path(&path),
                );
            }
        }
        Err(error) => findings.push(Finding::new("scan_failed", Severity::High, error)),
    }
    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (file, block) in policies.blocks() {
        names
            .entry(block.name.to_ascii_lowercase())
            .or_default()
            .push(format!("{} in {}", block.name, file.path));
    }
    for places in names.values().filter(|places| places.len() > 1) {
        findings.push(Finding::new(
            "policy_duplicate",
            Severity::High,
            format!(
                "policy names must be unique (ignoring case): {}",
                places.join(", ")
            ),
        ));
    }
    findings.extend(default_findings(&state.config, state, policies));
    if !super::config::valid_mode(&state.config.autonomy.mode) {
        findings.push(Finding::new(
            "autonomy_mode_invalid",
            Severity::High,
            format!("autonomy mode '{}' in .crane/config.json is not observe, assisted, delegated, or autonomous", state.config.autonomy.mode),
        ));
    }
    for (file, block) in policies.blocks() {
        for statement in &block.statements {
            let place = format!("{}:{}", file.path, statement.line);
            match state.record(&statement.id) {
                None => findings.push(
                    Finding::new(
                        "policy_unregistered_selection",
                        Severity::Critical,
                        format!("{place}: {} is not a registered selection; markers are generated only by crane protect and crane target", statement.id),
                    )
                    .with_selection(&statement.id)
                    .with_path(&file.path),
                ),
                Some(record) => {
                    let expected = record.change_type.as_deref().map(ChangeType::parse).transpose().ok().flatten();
                    if record.operation != statement.operation
                        || expected != statement.change_type
                        || record.policy != block.name
                    {
                        findings.push(
                            Finding::new(
                                "policy_registry_mismatch",
                                Severity::Critical,
                                format!(
                                    "{place}: '{}' in policy {} does not match the signed registry ({} in policy {}); use crane add policy to move commands",
                                    statement.render(),
                                    block.name,
                                    super::policy::render_statement(record.operation, &record.id, expected),
                                    record.policy
                                ),
                            )
                            .with_selection(&statement.id)
                            .with_path(&file.path),
                        );
                    }
                }
            }
        }
    }
    for record in &state.registry.selections {
        match policies.references(&record.id).len() {
            1 => {}
            0 if !policies.errors.is_empty() => {}
            0 => findings.push(
                Finding::new(
                    "selection_unreferenced",
                    Severity::Critical,
                    format!("selection {} has no command in any policy (removed outside Crane); it belongs to policy {}", record.id, record.policy),
                )
                .with_selection(&record.id),
            ),
            count => findings.push(
                Finding::new(
                    "selection_referenced_twice",
                    Severity::High,
                    format!("selection {} is referenced by {count} commands; each selection belongs to exactly one command", record.id),
                )
                .with_selection(&record.id),
            ),
        }
    }
    findings
}

/** Check the default policy and checkpoint
 * Input
    - config: &Config - repository configuration
    - state: &State - governance state
    - policies: &PolicySet - parsed policies
 * Output
    - Vec<Finding>
*/
fn default_findings(config: &Config, state: &State, policies: &PolicySet) -> Vec<Finding> {
    let mut findings = Vec::new();
    match &config.default_policy {
        None => findings.push(Finding::new(
            "default_policy_unset",
            Severity::High,
            "no default policy is set; run 'crane --set default policy NAME'".into(),
        )),
        Some(name) if policies.find(name).is_none() && policies.errors.is_empty() => {
            findings.push(Finding::new(
                "default_policy_missing",
                Severity::High,
                format!("the default policy '{name}' does not exist in .crane/policies"),
            ))
        }
        Some(_) => {}
    }
    match &config.default_checkpoint {
        None => findings.push(Finding::new(
            "default_checkpoint_unset",
            Severity::Medium,
            "no default checkpoint yet; run 'crane checkpoint NAME'".into(),
        )),
        Some(name) if state.checkpoints.find(name).is_none() => findings.push(Finding::new(
            "default_checkpoint_missing",
            Severity::High,
            format!("the default checkpoint '{name}' does not exist"),
        )),
        Some(_) => {}
    }
    findings
}

/** Scan the work tree as the options ask, falling back to a full scan when a selection is missing
 * from a partial scan
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - mode: &ScanMode - scan mode
 * Output
    - Result<Scan, String>
*/
fn scan(workspace: &Workspace, state: &State, mode: &ScanMode) -> Result<Scan, String> {
    match mode {
        ScanMode::Full => scan_repository(workspace),
        ScanMode::Paths(paths) => {
            let mut wanted = paths.iter().cloned().collect::<BTreeSet<_>>();
            wanted.extend(
                state
                    .registry
                    .selections
                    .iter()
                    .map(|record| record.current.span.path.clone()),
            );
            let partial = scan_paths(workspace, wanted);
            let missing = state.registry.selections.iter().any(|record| {
                matches!(
                    resolve(&record.id, &partial),
                    Resolution::Unresolved {
                        code: "markers_missing",
                        ..
                    }
                )
            });
            if missing {
                scan_repository(workspace)
            } else {
                Ok(partial)
            }
        }
    }
}

/** Evaluate one command against its resolved selection
 * Input
    - workspace: &Workspace - repository
    - record: &Record - signed record
    - resolution: &Resolution - where the selection is now
    - change_type: Option<ChangeType> - required change kind
    - targets_pending: bool - report an unchanged target as Pending
 * Output
    - (Outcome, String, Option<Summary>) result, explanation, classified change
*/
fn judge_command(
    workspace: &Workspace,
    record: &Record,
    resolution: &Resolution,
    change_type: Option<ChangeType>,
    targets_pending: bool,
    state: &State,
    scan: &Scan,
) -> (Outcome, String, Option<Summary>) {
    let location = match resolution {
        Resolution::Unresolved { reason, .. } => {
            return (Outcome::Unresolved, format!("UNRESOLVED: {reason}"), None)
        }
        Resolution::Resolved(location) => location,
    };
    let where_ = format!(
        "{} lines {}-{}",
        location.span.path, location.span.start_line, location.span.end_line
    );
    let current = content_digest(&location.content);
    let baseline = baseline(workspace, record);
    let syntax = syntax_of(&location.span.path);
    match record.operation {
        Operation::Preserve => {
            if current == record.origin.content_digest {
                return (Outcome::Pass, format!("preserved ({where_})"), None);
            }
            if unchanged_except_nested_targets(workspace, record, location, state, scan) {
                return (
                    Outcome::Pass,
                    format!("preserved outside nested target selections ({where_})"),
                    None,
                );
            }
            let summary = baseline
                .as_ref()
                .ok()
                .map(|baseline| classify(baseline, &location.content, syntax));
            let detail =
                summary.map_or_else(String::new, |summary| format!(": {}", summary.describe()));
            (
                Outcome::Fail,
                format!(
                    "preserved selection changed since checkpoint {} ({where_}){detail}",
                    record.origin.checkpoint
                ),
                summary,
            )
        }
        Operation::Target => {
            let baseline = match baseline {
                Ok(baseline) => baseline,
                Err(error) => return (Outcome::Unresolved, format!("UNRESOLVED: {error}"), None),
            };
            let summary = classify(&baseline, &location.content, syntax);
            match judge(&summary, change_type) {
                Ok(()) => (
                    Outcome::Pass,
                    format!(
                        "target changed as required ({where_}): {}",
                        summary.describe()
                    ),
                    Some(summary),
                ),
                Err(_) if targets_pending && !summary.changed => (
                    Outcome::Pending,
                    format!("target not changed yet ({where_})"),
                    Some(summary),
                ),
                Err(reason) => (Outcome::Fail, format!("{reason} ({where_})"), Some(summary)),
            }
        }
    }
}

/** Verify the governance state and evaluate every policy
 * Input
    - workspace: &Workspace - repository
    - options: &Options - scan mode and target handling
 * Output
    - Result<(Report, State), String> the report and the state it was computed from
    - Error only when .crane cannot be read at all (malformed files become findings)
*/
pub(crate) fn evaluate(
    workspace: &Workspace,
    options: &Options,
) -> Result<(Report, State), String> {
    let mut findings = Vec::new();
    let state = match State::load(workspace) {
        Ok(state) => state,
        Err(error) => {
            findings.push(Finding::new(
                "governance_unreadable",
                Severity::Critical,
                error,
            ));
            let report = Report {
                generation: 0,
                findings,
                policies: Vec::new(),
                relocations: Vec::new(),
                resolutions: BTreeMap::new(),
                files_scanned: 0,
                passed: false,
                zones: None,
            };
            return Ok((report, State::default()));
        }
    };
    let trust = workspace.trust()?;
    findings.extend(integrity(workspace, &state, &trust));
    let policies = PolicySet::load(workspace)?;
    findings.extend(consistency(workspace, &state, &policies));
    // Selections created before context was required still work, but are reported
    for record in state.registry.selections.iter().filter(|record| {
        record
            .context
            .as_deref()
            .is_none_or(|text| text.trim().is_empty())
    }) {
        findings.push(
            Finding::new(
                "selection_context_missing",
                Severity::Medium,
                format!(
                    "selection {} has no context for agents; create it again with 'crane {} FILE context TEXT ...' to add it",
                    record.id,
                    if record.operation == crate::selection::registry::Operation::Preserve { "protect" } else { "target" }
                ),
            )
            .with_selection(&record.id),
        );
    }
    let mut zones = None;
    if options.governance {
        match super::zones::load(workspace) {
            Ok(governance) => {
                let resolution = super::zones::resolve(workspace, &governance, &state, &policies)?;
                findings.extend(resolution.findings.iter().cloned());
                zones = Some(resolution);
            }
            Err(error) => findings.push(Finding::new(
                "governance_yaml_invalid",
                Severity::High,
                error,
            )),
        }
    }
    let scanned = scan(workspace, &state, &options.scan)?;
    let registered = state
        .registry
        .selections
        .iter()
        .map(|record| record.id.clone())
        .collect::<BTreeSet<_>>();
    findings.extend(scan_findings(&scanned, &registered));
    let invalid = findings
        .iter()
        .filter(|finding| finding.code == "selection_record_invalid")
        .filter_map(|finding| finding.selection.clone())
        .collect::<BTreeSet<_>>();
    let mut resolutions = BTreeMap::new();
    let mut relocations = Vec::new();
    let mut resolved = BTreeMap::new();
    for record in &state.registry.selections {
        let resolution = resolve(&record.id, &scanned);
        match &resolution {
            Resolution::Resolved(location) => {
                if location.span.path != record.current.span.path {
                    relocations.push(Relocation {
                        id: record.id.clone(),
                        from: record.current.span.path.clone(),
                        to: location.span.path.clone(),
                    });
                }
                resolutions.insert(record.id.clone(), Some(location.span.clone()));
            }
            Resolution::Unresolved { code, reason } => {
                let mut message = format!("selection {} is UNRESOLVED: {reason}", record.id);
                if options.candidates && *code == "markers_missing" {
                    let found = candidates(workspace, record);
                    if !found.is_empty() {
                        message.push_str(&format!(
                            "; its baseline content appears at {} but is never rebound automatically",
                            found.join(", ")
                        ));
                    }
                }
                findings.push(
                    Finding::new("selection_unresolved", Severity::High, message)
                        .with_selection(&record.id),
                );
                resolutions.insert(record.id.clone(), None);
            }
        }
        resolved.insert(record.id.clone(), resolution);
    }
    let mut results = Vec::new();
    for (file, block) in policies.blocks() {
        let mut commands = Vec::new();
        for statement in &block.statements {
            let (outcome, message, location, change) = match state.record(&statement.id) {
                None => (
                    Outcome::Fail,
                    format!("{} is not a registered selection", statement.id),
                    None,
                    None,
                ),
                Some(_) if invalid.contains(&statement.id) => (
                    Outcome::Unresolved,
                    "UNRESOLVED: the selection record failed signature verification".to_string(),
                    None,
                    None,
                ),
                Some(record) => {
                    let resolution = &resolved[&record.id];
                    let (outcome, message, change) = judge_command(
                        workspace,
                        record,
                        resolution,
                        statement.change_type,
                        options.targets_pending,
                        &state,
                        &scanned,
                    );
                    let location = match resolution {
                        Resolution::Resolved(location) => Some(location.span.clone()),
                        Resolution::Unresolved { .. } => None,
                    };
                    (outcome, message, location, change)
                }
            };
            commands.push(CommandResult {
                policy: block.name.clone(),
                file: file.path.clone(),
                line: statement.line,
                operation: statement.operation,
                id: statement.id.clone(),
                change_type: statement.change_type,
                outcome,
                message,
                location,
                change,
            });
        }
        results.push(PolicyResult {
            name: block.name.clone(),
            file: file.path.clone(),
            passed: commands
                .iter()
                .all(|command| matches!(command.outcome, Outcome::Pass | Outcome::Pending)),
            commands,
        });
    }
    let blocking = findings.iter().any(Finding::blocking);
    let report = Report {
        generation: state.generation(),
        passed: !blocking && results.iter().all(|policy| policy.passed),
        findings,
        policies: results,
        relocations,
        resolutions,
        files_scanned: scanned.files,
        zones,
    };
    Ok((report, state))
}
