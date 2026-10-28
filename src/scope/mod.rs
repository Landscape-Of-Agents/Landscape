// The shared scope engine: one implementation of "what may an agent do here" for the hooks, the
// CLI ('crane scope'), and later skill invocations. It loads the trusted boundary once (the signed
// state and its integrity, zone and flow limits, selections), resolves a requested resource to what
// it really is (resource.rs), decides through hooks::enforce::Guard, and records each decision with
// the exact rule and resource versions it was made under (decision.rs), so it can be explained and
// recognized as stale later. It never decides anything itself: every decision comes from the guard,
// so the CLI and the hooks cannot disagree.
//
//   resource - normalize a target: repository path, symlinks, other worktrees, nested repositories,
//              tracked, untracked, or ignored, and the content digest that identifies it now
//   decision - the recorded, hash-chained scope decisions and their validity against the present
//   snapshot - the effective boundary per file, and the difference between two boundaries
//              (implicit widening when repository structure changes)
//   probe    - positive and adversarial simulations of the boundary; nothing is executed
//   audit    - the boundary compared with what agents attempted and what really changed
//
// Repository scope is not a sandbox: what a shell, a script, or the network does outside the
// files Crane can name is reported as a gap, never as enforced.

pub(crate) mod audit;
pub(crate) mod decision;
pub(crate) mod probe;
pub(crate) mod resource;
pub(crate) mod snapshot;

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::governance::state::{integrity, State};
use crate::governance::workspace::Workspace;
use crate::governance::zones::{cached_limits, Limits};
use crate::governance::Finding;
use crate::hooks::action::{AgentAction, FileChange, Operation, Proposed};
use crate::hooks::adapter::{adapter, AgentKind};
use crate::hooks::enforce::{Decided, Guard};
use crate::selection::anchors::find_markers;
use crate::telemetry::model::Safety;
use crate::trust::TrustStore;

/** What every decision is made from, computed the same way for the hooks and the CLI
 * Fields
    - integrity: Vec<Finding> - blocking integrity findings of the signed state (empty: verified)
    - limits: Limits - zone and flow limits by file
    - zone_error: Option<String> - why the zone configuration could not be resolved (changes then
      fail closed)
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct Enforcement {
    pub(crate) integrity: Vec<Finding>,
    pub(crate) limits: Limits,
    pub(crate) zone_error: Option<String>,
}

/** Compute the integrity and zone limits a decision depends on
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - trust: &TrustStore - trust directory
    - cache_directory: &Path - runtime directory holding the limits cache
 * Output
    - Enforcement
*/
pub(crate) fn enforcement(
    workspace: &Workspace,
    state: &State,
    trust: &TrustStore,
    cache_directory: &Path,
) -> Enforcement {
    let integrity = integrity(workspace, state, trust)
        .into_iter()
        .filter(Finding::blocking)
        .collect();
    let (limits, zone_error) = match cached_limits(workspace, state, cache_directory) {
        Ok(limits) => (limits, None),
        Err(error) => (Limits::default(), Some(error)),
    };
    Enforcement {
        integrity,
        limits,
        zone_error,
    }
}

/** The rule and configuration versions a decision was made under
 * Fields
    - registry_generation: u64 - signed registry generation
    - manifest_head: Option<String> - head of the signed manifest
    - governance_signed: Option<String> - digest of governance.yaml the manifest signs
    - governance_current: String - digest of governance.yaml as it is now
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Versions {
    pub(crate) registry_generation: u64,
    pub(crate) manifest_head: Option<String>,
    pub(crate) governance_signed: Option<String>,
    pub(crate) governance_current: String,
}

impl Versions {
    /** Read the versions of a governance state
     * Input
        - state: &State - governance state
     * Output
        - Versions
    */
    pub(crate) fn of(state: &State) -> Self {
        let manifest = state.registry.manifest.as_ref();
        Self {
            registry_generation: state.generation(),
            manifest_head: manifest.map(|manifest| manifest.head.clone()),
            governance_signed: manifest
                .map(|manifest| manifest.governance_digest.clone())
                .filter(|digest| !digest.is_empty()),
            governance_current: state.governance_digest.clone(),
        }
    }

    /** Describe how the present versions differ from these
     * Input
        - current: &Versions - versions now
     * Output
        - Vec<String> differences, empty when nothing changed
    */
    pub(crate) fn changes(&self, current: &Self) -> Vec<String> {
        let mut changes = Vec::new();
        if self.registry_generation != current.registry_generation
            || self.manifest_head != current.manifest_head
        {
            changes.push(format!(
                "the signed rules changed (generation {} -> {})",
                self.registry_generation, current.registry_generation
            ));
        }
        if self.governance_current != current.governance_current {
            changes.push("zones, flows, or context packs (governance.yaml) changed".into());
        }
        changes
    }
}

/** Kinds of access a request asks about
 * Variants
    - Read - read a file or resource
    - Write - change a file
    - Delete - remove a file
    - Execute - run a shell command
    - Network - reach a URL
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Access {
    Read,
    Write,
    Delete,
    Execute,
    Network,
}

impl Access {
    /** Every access kind, in order */
    pub(crate) const ALL: [Self; 5] = [
        Self::Read,
        Self::Write,
        Self::Delete,
        Self::Execute,
        Self::Network,
    ];

    /** Return the access kind's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Delete => "delete",
            Self::Execute => "execute",
            Self::Network => "network",
        }
    }

    /** Parse an access kind by name
     * Input
        - value: &str - name
     * Output
        - Option<Access>
    */
    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|access| access.name() == value)
    }
}

/** A question about the boundary: may an agent do this, here, now?
 * Fields
    - access: Access - kind of access
    - target: String - path, URL, or shell command as an agent would give it
    - lines: Option<(usize, usize)> - for a write, the lines it would change (None: every line)
    - mode: String - autonomy mode of the session asked about (default delegated)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Request {
    pub(crate) access: Access,
    pub(crate) target: String,
    pub(crate) lines: Option<(usize, usize)>,
    pub(crate) mode: String,
}

/** Something Crane cannot establish or enforce for a request, reported instead of assumed
 * Fields
    - code: String - machine-readable reason
    - message: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Gap {
    pub(crate) code: String,
    pub(crate) message: String,
}

/** Build a gap
 * Input
    - code: &str - reason code
    - message: &str - explanation
 * Output
    - Gap
*/
fn gap(code: &str, message: &str) -> Gap {
    Gap {
        code: code.into(),
        message: message.into(),
    }
}

/** The answer to a request
 * Fields
    - resource: resource::Resource - what the target really is
    - decided: Decided - the guard's decision
    - gaps: Vec<Gap> - what Crane cannot establish or enforce for it
*/
#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    pub(crate) resource: resource::Resource,
    pub(crate) decided: Decided,
    pub(crate) gaps: Vec<Gap>,
}

/** The trusted boundary of one repository, loaded once
 * Fields
    - workspace: Workspace - repository
    - state: State - governance state (verified or not; see enforcement.integrity)
    - enforcement: Enforcement - integrity and zone limits
    - trust_home: Option<PathBuf> - the trust directory
    - runtime: PathBuf - the repository's runtime directory in the trust directory
    - protected: Vec<&'static str> - hook settings files of every supported agent tool
    - bricks: Rules - the Brick rules decided per call (identity is not decided without an agent)
*/
pub(crate) struct Boundary {
    pub(crate) workspace: Workspace,
    pub(crate) state: State,
    pub(crate) enforcement: Enforcement,
    pub(crate) trust_home: Option<PathBuf>,
    pub(crate) runtime: PathBuf,
    pub(crate) protected: Vec<&'static str>,
    pub(crate) bricks: crate::brick::guard::Rules,
}

/** The hook settings files of every supported agent tool, which no agent may change
 * Input
    - None
 * Output
    - Vec<&'static str>
*/
fn protected_paths() -> Vec<&'static str> {
    let mut paths = [
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Copilot,
        AgentKind::Gemini,
    ]
    .into_iter()
    .flat_map(|kind| adapter(kind).protected_paths().iter().copied())
    .collect::<Vec<_>>();
    paths.sort_unstable();
    paths.dedup();
    paths
}

impl Boundary {
    /** Load the boundary of a repository
     * Input
        - workspace: Workspace - repository
     * Output
        - Result<Boundary, String>
        - Error if the trust directory or the governance state cannot be read
    */
    pub(crate) fn load(workspace: Workspace) -> Result<Self, String> {
        let trust = workspace.trust()?;
        let state = State::load(&workspace)?;
        let runtime = trust.runtime();
        let enforcement = enforcement(&workspace, &state, &trust, &runtime);
        let bricks = crate::brick::guard::Rules::load(&workspace, &state, Some(&runtime));
        Ok(Self {
            workspace,
            state,
            enforcement,
            trust_home: Some(trust.home),
            runtime,
            protected: protected_paths(),
            bricks,
        })
    }

    /** Report whether the signed state verified
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn integrity_ok(&self) -> bool {
        self.enforcement.integrity.is_empty()
    }

    /** The versions decisions are made under now
     * Input
        - None (uses self)
     * Output
        - Versions
    */
    pub(crate) fn versions(&self) -> Versions {
        Versions::of(&self.state)
    }

    /** The guard as an agent session in a mode would meet it (active safety, unlimited credits,
     * relative paths taken from the current directory)
     * Input
        - mode: &str - autonomy mode
     * Output
        - Guard
    */
    pub(crate) fn guard(&self, mode: &str) -> Guard<'_> {
        Guard {
            workspace: &self.workspace,
            state: &self.state,
            integrity_ok: self.integrity_ok(),
            protected_paths: &self.protected,
            trust_home: self.trust_home.clone(),
            cwd: std::env::current_dir().ok(),
            mode: mode.into(),
            safety: Safety::Active,
            available: None,
            costs: self.state.config.autonomy.costs.clone(),
            limits: Some(&self.enforcement.limits),
            zone_error: self.enforcement.zone_error.clone(),
            caller: crate::hooks::enforce::Caller::default(),
            bricks: Some(&self.bricks),
        }
    }

    /** Decide a request through the guard
     * Input
        - request: &Request - question
     * Output
        - Result<Outcome, String>
        - Error if the request cannot be turned into an action (lines outside the file, a network
          target that is not a URL)
    */
    pub(crate) fn decide(&self, request: &Request) -> Result<Outcome, String> {
        let guard = self.guard(&request.mode);
        let resource = resource::resolve(self, &guard, request);
        let action = self.action(&guard, request)?;
        let decided = guard.decide(&action);
        let gaps = gaps(request, &resource, &decided);
        Ok(Outcome {
            resource,
            decided,
            gaps,
        })
    }

    /** Turn a request into the provider-neutral action an agent tool would propose
     * Input
        - guard: &Guard - guard (for path resolution)
        - request: &Request - question
     * Output
        - Result<AgentAction, String>
    */
    fn action(&self, guard: &Guard, request: &Request) -> Result<AgentAction, String> {
        let mut action = AgentAction {
            tool: "crane-scope".into(),
            operation: Operation::Read,
            files: Vec::new(),
            reads: Vec::new(),
            command: None,
            arguments: Vec::new(),
            digest: String::new(),
        };
        match request.access {
            Access::Read => action.reads.push(request.target.clone()),
            Access::Network => {
                if !request.target.contains("://") {
                    return Err(format!(
                        "a network target must be a URL, got '{}'",
                        request.target
                    ));
                }
                action.reads.push(request.target.clone());
            }
            Access::Execute => {
                action.operation = Operation::Execute;
                action.command = Some(request.target.clone());
            }
            Access::Delete => {
                action.operation = Operation::Write;
                action.files.push(FileChange {
                    path: request.target.clone(),
                    proposed: Proposed::Delete,
                });
            }
            Access::Write => {
                let before = guard
                    .relative(&request.target)
                    .and_then(|path| {
                        std::fs::read(self.workspace.root.join(&path))
                            .ok()
                            .map(|bytes| (path, bytes))
                    })
                    .and_then(|(path, bytes)| {
                        String::from_utf8(bytes).ok().map(|text| (path, text))
                    });
                let content = match &before {
                    Some((path, text)) => probe_text(path, text, request.lines)?,
                    None if request.lines.is_some() => {
                        return Err(format!(
                            "{} does not exist in the repository, so it has no lines to change",
                            request.target
                        ))
                    }
                    None => PROBE_LINE.into(),
                };
                action.operation = Operation::Write;
                action.files.push(FileChange {
                    path: request.target.clone(),
                    proposed: Proposed::Content(content),
                });
            }
        }
        Ok(action)
    }
}

/** The content of a file a probe creates */
const PROBE_LINE: &str = "crane scope probe\n";

/** Simulate a change to lines of a text: every chosen line gets a trailing space, except Crane's
 * own marker lines (an edit of a marker is a separate rule, decided as such); a file without text
 * gets the probe line
 * Input
    - path: &str - repository path (for the comment syntax of markers)
    - text: &str - current text
    - lines: Option<(usize, usize)> - first and last line to change, None for every line
 * Output
    - Result<String, String>
    - Error if the lines are outside the file or in the wrong order
*/
pub(crate) fn probe_text(
    path: &str,
    text: &str,
    lines: Option<(usize, usize)>,
) -> Result<String, String> {
    if text.is_empty() {
        return match lines {
            None => Ok(PROBE_LINE.into()),
            Some(_) => Err(format!("{path} is empty, so it has no lines to change")),
        };
    }
    let count = text.split_inclusive('\n').count();
    if let Some((first, last)) = lines {
        if first == 0 || first > last || last > count {
            return Err(format!(
                "lines {first}-{last} are not within {path} (1-{count})"
            ));
        }
    }
    let markers = find_markers(path, text)
        .iter()
        .map(|marker| marker.line)
        .collect::<std::collections::BTreeSet<_>>();
    let mut changed = String::with_capacity(text.len() + count);
    for (index, line) in text.split_inclusive('\n').enumerate() {
        let number = index + 1;
        let chosen = lines.is_none_or(|(first, last)| number >= first && number <= last);
        if !chosen || markers.contains(&number) {
            changed.push_str(line);
            continue;
        }
        let body = line.trim_end_matches(['\n', '\r']);
        changed.push_str(body);
        changed.push(' ');
        changed.push_str(&line[body.len()..]);
    }
    Ok(changed)
}

/** List what Crane cannot establish or enforce for a decided request
 * Input
    - request: &Request - question
    - resource: &resource::Resource - resolved target
    - decided: &Decided - decision
 * Output
    - Vec<Gap>
*/
fn gaps(request: &Request, resource: &resource::Resource, decided: &Decided) -> Vec<Gap> {
    use resource::{Location, Tracking};
    let mut gaps = Vec::new();
    let allowed = decided.verdict.decision != crate::telemetry::model::Decision::Deny
        && decided.verdict.decision != crate::telemetry::model::Decision::Quarantine;
    if !allowed {
        return gaps;
    }
    if request.access == Access::Execute {
        gaps.push(gap("EFFECT_UNKNOWN_BEFORE_RUN", "what a shell command changes is known only after it runs: the check is lexical, and post-action verification judges the real effect (and quarantines a bypass)"));
        gaps.push(gap("SANDBOX_UNAVAILABLE", "no sandbox restricts the files, processes, or credentials the command can reach outside what Crane names"));
    }
    if request.access == Access::Network || !decided.network.is_empty() {
        gaps.push(gap("NETWORK_NOT_MEDIATED", "Crane does not mediate the network: destinations are parsed from text, never observed, and an unnamed destination cannot be stopped"));
    }
    match resource.location {
        Location::OtherWorktree => gaps.push(gap("OTHER_WORKTREE", "the path is in another worktree of this repository; Crane governs only the worktree the session runs in, so its preserved selections are not enforced there")),
        Location::OutsideRepository if matches!(request.access, Access::Write | Access::Delete) => gaps.push(gap("OUTSIDE_REPOSITORY", "the path is outside the repository; Crane does not govern it and no sandbox restricts it")),
        _ => {}
    }
    match resource.tracking {
        Tracking::Ignored => gaps.push(gap("IGNORED_FILE", "the file is ignored by Git, so no zone or flow covers it and changes to it are not judged after the fact")),
        Tracking::NestedRepository => gaps.push(gap("NESTED_REPOSITORY", "the file belongs to a nested repository or submodule, which zones, flows, and selections of this repository do not cover")),
        _ => {}
    }
    gaps
}

/** Most files listed in the agent's view of the boundary */
const AGENT_VIEW_FILES: usize = 40;

/** The read-only view of the boundary an agent receives in its session context (agents never run
 * Crane, so this is their 'crane scope inspect'): the files where active zones and flows restrict
 * changes, with their ceilings, and how actions are judged. It lists restrictions, never the limits
 * of enforcement, so it cannot serve as a map for working around them. Guidance only: every action
 * is still decided by the guard
 * Input
    - limits: &Limits - zone and flow limits by file
 * Output
    - String, empty when no zone or flow restricts anything
*/
pub(crate) fn agent_view(limits: &Limits) -> String {
    use crate::governance::zones::{Ceiling, Criticality};
    let restricted = limits
        .iter()
        .filter_map(|(path, limit)| {
            let file_wide = !limit.entities.is_empty()
                && (limit.ceiling <= Ceiling::Assisted
                    || limit.criticality == Criticality::Restricted);
            let blocks = limit
                .scopes
                .iter()
                .filter(|block| {
                    block.ceiling <= Ceiling::Assisted
                        || block.criticality == Criticality::Restricted
                })
                .map(|block| {
                    format!(
                        "lines {}-{} {}",
                        block.start_line,
                        block.end_line,
                        if block.ceiling == Ceiling::Observe {
                            "no changes"
                        } else {
                            "changes need approval"
                        }
                    )
                })
                .collect::<Vec<_>>();
            if file_wide {
                let rule = if limit.ceiling == Ceiling::Observe {
                    "no agent changes"
                } else {
                    "every change needs a person's approval"
                };
                Some(format!(
                    "  - {path}: {rule} ({})",
                    limit.entities.join(", ")
                ))
            } else if !blocks.is_empty() {
                Some(format!("  - {path}: {}", blocks.join("; ")))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if restricted.is_empty() {
        return String::new();
    }
    let mut text = String::from("Effective boundary (resolved by Crane; guidance only, every action is still decided when you make it):\n");
    for line in restricted.iter().take(AGENT_VIEW_FILES) {
        text.push_str(line);
        text.push('\n');
    }
    if restricted.len() > AGENT_VIEW_FILES {
        text.push_str(&format!(
            "  - and {} more restricted files\n",
            restricted.len() - AGENT_VIEW_FILES
        ));
    }
    text.push_str("Shell commands and scripts are checked before they run and judged again by what they actually changed; a change that went around a check quarantines the session.\n");
    text
}

/** The Crane operation domain a guard rule belongs to, and what it means in plain words
 * Input
    - code: &str - rule code (DASHBOARD.md decision_rule)
 * Output
    - (&'static str, &'static str) domain and explanation; ("unknown", ...) for a code this
      version does not know
*/
pub(crate) fn rule(code: &str) -> (&'static str, &'static str) {
    match code {
        "OTHER_WORKTREE_WRITE" => (
            "scope",
            "the file is in another worktree of this repository, which this session's rules cannot check",
        ),
        "WRITE_NAMES_NO_FILE" => ("scope", "the write did not name a file Crane could check, so it was refused"),
        "COMMAND_NOT_STATED" => ("scope", "the shell command was not stated, so it could not be checked"),
        "ZONE_CONFIG_UNRESOLVED" => ("scope", "the zones and flows could not be resolved, so changes are refused until a person fixes them"),
        "EDIT_NOT_SIMULATABLE" => ("scope", "the edit could not be applied to the file's current text, and the file holds protected selections"),
        "PRESERVED_SELECTION" => ("constraints", "the change would alter code a person marked to stay exactly as it was"),
        "SELECTION_FILE_DELETE" => ("constraints", "deleting the file would remove code people protect or target"),
        "SHELL_MODIFIES_PRESERVED_FILE" => ("constraints", "the shell command would change a file holding protected code; use the edit tools so Crane can check the change"),
        "SCRIPT_MODIFIES_PRESERVED_FILE" => ("constraints", "the script would change a file holding protected code"),
        "MARKER_CHANGED" => ("constraints", "the edit would add, remove, or alter Crane's selection markers, which only Crane commands manage"),
        "MARKER_FORGED" => ("constraints", "the edit would write Crane markers that Crane did not create"),
        "UNREGISTERED_MARKER" => ("constraints", "the file holds markers of a selection that is not in the signed registry"),
        "CRANE_METADATA_WRITE" => ("permission", "Crane's signed rules can only be changed by people through Crane"),
        "HOOK_SETTINGS_WRITE" => ("permission", "the agent tool's hook settings connect it to Crane, so agents cannot change them"),
        "CRANE_COMMAND" => ("permission", "only a person runs Crane commands"),
        "SHELL_NAMES_METADATA" => ("permission", "the shell command names Crane's rules, trust directory, or hook settings"),
        "TOOL_NAMES_METADATA" => ("permission", "the tool call names Crane's rules, trust directory, or hook settings"),
        "SCRIPT_NAMES_METADATA" => ("permission", "the script names Crane's rules, trust directory, or hook settings"),
        "SCRIPT_RUNS_CRANE" => ("permission", "the script runs Crane, which only a person may do"),
        "ZONE_OBSERVE" => ("permission", "the code is in an area where agents may only look, not change"),
        "MODE_OBSERVE" => ("permission", "the session is in observe mode, so it may not change anything"),
        "TRUST_DIRECTORY_ACCESS" => ("isolation", "the trust directory holds signing keys and secrets, which agents may not read or change"),
        "SCRIPT_SECRETS_NETWORK" => ("isolation", "the script reads credentials and uses the network"),
        "SCRIPT_DOWNLOAD_EXECUTE" => ("isolation", "the command runs code downloaded from the network"),
        "SCRIPT_DOWNLOAD_EVALUATE" => ("isolation", "the command evaluates code downloaded from the network"),
        "SCRIPT_DECODE_EXECUTE" => ("isolation", "the command runs code it decodes first, which hides what it does"),
        "ENCODED_POWERSHELL" => ("isolation", "the command runs encoded PowerShell, which hides what it does"),
        "ZONE_ASSISTED" => ("approval", "the code is in an area where every agent change needs a person's approval"),
        "ZONE_RESTRICTED" => ("approval", "the code is restricted, so every agent change needs a person's approval"),
        "MODE_ASSISTED" => ("approval", "the session is in assisted mode, so every change needs a person's approval"),
        "SAFETY_DEGRADED" => ("approval", "the session broke a rule earlier, so its changes need a person's approval"),
        "SCRIPT_HISTORY_REWRITE" => ("approval", "the command rewrites Git history, which needs a person's approval"),
        "SCRIPT_TREE_DELETION" => ("approval", "the command deletes a directory tree, which needs a person's approval"),
        "INTEGRITY_FAILED" => ("provenance", "Crane's signed rules failed verification, so changes are refused until a person checks them"),
        "CREDITS_EXHAUSTED" | "CREDITS_SHORT" => ("budgets", "the session's autonomy budget is used up, so a person must approve"),
        "SAFETY_QUARANTINED" => ("revocation", "the session is quarantined after a bypass; only reads are allowed"),
        "DASHBOARD_AGENT_QUARANTINE" => ("revocation", "an admin quarantined this agent or session from the dashboard"),
        _ => ("unknown", "a rule this version of Crane does not describe"),
    }
}

#[cfg(test)]
mod tests {
    use super::{probe_text, rule, Versions};

    /** Every rule code the guard can emit has a domain and an explanation
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn every_guard_rule_is_described() {
        let enforce = include_str!("../hooks/enforce.rs");
        let scripts = include_str!("../hooks/scripts.rs");
        let tools = include_str!("../hooks/runtime/tools.rs");
        let mut codes = std::collections::BTreeSet::new();
        for text in [enforce, scripts, tools] {
            for (index, _) in text.match_indices('"') {
                let rest = &text[index + 1..];
                let end = rest.find('"').unwrap_or(0);
                let word = &rest[..end];
                if word.len() > 4
                    && word.contains('_')
                    && word
                        .chars()
                        .all(|character| character.is_ascii_uppercase() || character == '_')
                {
                    codes.insert(word.to_string());
                }
            }
        }
        assert!(codes.contains("PRESERVED_SELECTION"), "{codes:?}");
        for code in codes {
            assert_ne!(rule(&code).0, "unknown", "rule {code} has no domain");
        }
        assert_eq!(rule("SOMETHING_NEW").0, "unknown");
    }

    /** A probe changes exactly the chosen lines, keeps line endings, and never touches markers
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn probes_change_only_chosen_lines() {
        let text = "a\r\nb\nc\n";
        assert_eq!(
            probe_text("x.py", text, Some((2, 2))).unwrap(),
            "a\r\nb \nc\n"
        );
        assert_eq!(probe_text("x.py", text, None).unwrap(), "a \r\nb \nc \n");
        assert!(probe_text("x.py", text, Some((3, 4))).is_err());
        assert!(probe_text("x.py", text, Some((2, 1))).is_err());
        assert_eq!(probe_text("x.py", "", None).unwrap(), "crane scope probe\n");
        let marked =
            "# @crane:selection:K7M2P9RX:start\nkeep = 1\n# @crane:selection:K7M2P9RX:end\n";
        assert_eq!(
            probe_text("x.py", marked, None).unwrap(),
            "# @crane:selection:K7M2P9RX:start\nkeep = 1 \n# @crane:selection:K7M2P9RX:end\n"
        );
    }

    /** The agent's view lists restricted files and blocks, never permissive ones, and nothing
     * when nothing restricts
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn agent_view_lists_only_restrictions() {
        use crate::governance::zones::{Ceiling, Criticality, Limit, Limits, ScopedLimit};
        let limit = |ceiling, entity: &str| Limit {
            ceiling,
            criticality: Criticality::Routine,
            entities: vec![entity.into()],
            scopes: Vec::new(),
            bricks: Vec::new(),
        };
        let mut limits = Limits::new();
        limits.insert("web/app.js".into(), limit(Ceiling::Delegated, "zone:web"));
        assert_eq!(super::agent_view(&limits), "");
        limits.insert(
            "app/ledger.py".into(),
            limit(Ceiling::Observe, "zone:ledger"),
        );
        limits.insert(
            "app/pay.py".into(),
            Limit {
                ceiling: Ceiling::Autonomous,
                criticality: Criticality::Routine,
                entities: Vec::new(),
                scopes: vec![ScopedLimit {
                    start_line: 3,
                    end_line: 9,
                    ceiling: Ceiling::Assisted,
                    criticality: Criticality::Routine,
                    entities: vec!["zone:fees".into()],
                }],
                bricks: Vec::new(),
            },
        );
        let view = super::agent_view(&limits);
        assert!(
            view.contains("app/ledger.py: no agent changes (zone:ledger)"),
            "{view}"
        );
        assert!(
            view.contains("app/pay.py: lines 3-9 changes need approval"),
            "{view}"
        );
        assert!(!view.contains("web/app.js"), "{view}");
    }

    /** Version differences name what changed
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn version_changes_are_named() {
        let before = Versions {
            registry_generation: 2,
            manifest_head: Some("a".into()),
            governance_signed: None,
            governance_current: "g".into(),
        };
        assert!(before.changes(&before).is_empty());
        let mut after = before.clone();
        after.registry_generation = 3;
        after.governance_current = "h".into();
        assert_eq!(before.changes(&after).len(), 2);
    }
}
