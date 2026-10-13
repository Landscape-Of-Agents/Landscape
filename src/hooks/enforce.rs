// Pre-action authorization. Every proposed tool call is decided from the actual repository state:
// a write is simulated on the file's current text and denied when it would change a preserved
// selection, add, remove, or alter a marker, or touch Crane metadata (.crane, including
// .crane/map), the trust directory, or the provider's hook settings; such denials are final even if
// the agent asks for approval. A shell command naming protected metadata, running a governance
// command of Crane, or mutating a file that holds preserved selections is denied. Remaining shell
// and tool effects cannot be known before they run, so they are allowed only for post-action
// verification to judge. Autonomy mode, safety state, and credits are applied last, and can only
// make a decision stricter.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::action::{apply, AgentAction, FileChange, Operation, Proposed};
use super::adapter::Verdict;
use super::scripts;
use super::shell::{
    mutating_crane, names_crane_directory, network_destinations, normal, words, MUTATING_GIT,
    MUTATING_WORDS, NETWORK_WORDS,
};
use crate::governance::config::Costs;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::{Ceiling, Criticality, Limits};
use crate::platform::git;
use crate::selection::anchors::{find_markers, lines, Marker};
use crate::selection::registry::Operation as SelectionOperation;
use crate::selection::tracker::{resolve, Resolution, Scan};
use crate::telemetry::events::Resource;
use crate::telemetry::model::{Decision, Safety};

/** The outcome of deciding an action
 * Fields
    - verdict: Verdict - decision, reasons, bypass flag, and credit cost
    - resources: Vec<Resource> - resources the action names
    - selections: Vec<String> - selections it touches
    - policies: Vec<String> - policies of those selections
    - network: Vec<String> - network destinations inferred from it
    - zones: Vec<String> - zones and flows covering the files it writes
    - zone_floor: Option<(Decision, String)> - the strictest decision those zones and flows (or the
      script pre-check) require, with the reason
*/
#[derive(Debug, Clone)]
pub(crate) struct Decided {
    pub(crate) verdict: Verdict,
    pub(crate) resources: Vec<Resource>,
    pub(crate) selections: Vec<String>,
    pub(crate) policies: Vec<String>,
    pub(crate) network: Vec<String>,
    pub(crate) zones: Vec<String>,
    pub(crate) zone_floor: Option<(Decision, String)>,
}

/** Everything a decision depends on
 * Fields
    - workspace: &'a Workspace - repository
    - state: &'a State - verified-or-not governance state
    - integrity_ok: bool - the signed state verified (otherwise mutating actions fail closed)
    - protected_paths: &'a [&'a str] - provider settings files
    - trust_home: Option<PathBuf> - the trust directory
    - cwd: Option<PathBuf> - the agent's working directory, for relative paths
    - mode: String - autonomy mode
    - safety: Safety - session safety state
    - available: Option<u64> - available credits, None for unlimited
    - costs: Costs - credit model
    - limits: Option<&'a Limits> - zone and flow limits by file
    - zone_error: Option<String> - why the zone configuration could not be resolved (changes then
      fail closed)
*/
pub(crate) struct Guard<'a> {
    pub(crate) workspace: &'a Workspace,
    pub(crate) state: &'a State,
    pub(crate) integrity_ok: bool,
    pub(crate) protected_paths: &'a [&'a str],
    pub(crate) trust_home: Option<PathBuf>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) mode: String,
    pub(crate) safety: Safety,
    pub(crate) available: Option<u64>,
    pub(crate) costs: Costs,
    pub(crate) limits: Option<&'a Limits>,
    pub(crate) zone_error: Option<String>,
}

impl Guard<'_> {
    /** Resolve a tool path to a repository-relative path; None when outside the repository
     * Input
        - raw: &str - path as the tool gave it
     * Output
        - Option<String>
    */
    pub(crate) fn relative(&self, raw: &str) -> Option<String> {
        repository_path(self.workspace, self.cwd.as_deref(), raw)
    }

    /** Check whether text names protected metadata: .crane, the trust directory, or a provider
     * hook settings file
     * Input
        - text: &str - path, command, or argument
     * Output
        - bool
    */
    pub(crate) fn names_protected(&self, text: &str) -> bool {
        let text = normal(text);
        names_crane_directory(&text)
            || self.protected_paths.iter().any(|path| text.contains(path))
            || self
                .trust_home
                .as_ref()
                .is_some_and(|home| text.contains(&normal(&home.to_string_lossy())))
    }

    /** Check whether a path is inside the trust directory
     * Input
        - raw: &str - path as given
     * Output
        - bool
    */
    fn in_trust_home(&self, raw: &str) -> bool {
        let text = normal(raw);
        text.contains(".crane-trust")
            || self
                .trust_home
                .as_ref()
                .is_some_and(|home| text.contains(&normal(&home.to_string_lossy())))
    }

    /** List the files that hold preserved selections, by their last recorded path
     * Input
        - None (uses self)
     * Output
        - BTreeSet<String>
    */
    fn preserved_files(&self) -> BTreeSet<String> {
        self.state
            .registry
            .selections
            .iter()
            .filter(|record| record.operation == SelectionOperation::Preserve)
            .map(|record| record.current.span.path.clone())
            .collect()
    }

    /** Decide a proposed action
     * Input
        - action: &AgentAction - tool call
     * Output
        - Decided
    */
    pub(crate) fn decide(&self, action: &AgentAction) -> Decided {
        let mut decided = Decided {
            verdict: Verdict {
                decision: Decision::Allow,
                reasons: Vec::new(),
                bypass: false,
                cost: 0,
                context: None,
            },
            resources: Vec::new(),
            selections: Vec::new(),
            policies: Vec::new(),
            network: Vec::new(),
            zones: Vec::new(),
            zone_floor: None,
        };
        let deny = |decided: &mut Decided, reason: String, bypass: bool| {
            decided.verdict.decision = Decision::Deny;
            decided.verdict.bypass |= bypass;
            decided.verdict.reasons.push(reason);
        };
        match action.operation {
            Operation::Read => {
                decided.verdict.cost = self.costs.read;
                for read in &action.reads {
                    if self.in_trust_home(read) {
                        deny(&mut decided, format!("reading {read} is denied: the Crane trust directory holds signing keys and secrets"), true);
                    }
                    let is_url = read.contains("://");
                    decided.resources.push(Resource {
                        kind: if is_url { "url" } else { "file" }.into(),
                        id: if is_url {
                            read.split('?').next().unwrap_or_default().to_string()
                        } else {
                            self.relative(read).unwrap_or_else(|| read.clone())
                        },
                        access: if is_url { "network" } else { "read" }.into(),
                        classification: self
                            .relative(read)
                            .filter(|path| path.starts_with(".crane/"))
                            .map(|_| "metadata".into()),
                    });
                }
                decided.network = network_destinations(&action.reads);
                if !decided.network.is_empty() {
                    decided.verdict.cost += self.costs.network;
                }
            }
            Operation::Write => {
                decided.verdict.cost = self.costs.write;
                if action.files.is_empty() {
                    deny(
                        &mut decided,
                        "the write names no file Crane can check (malformed patch?)".into(),
                        false,
                    );
                }
                for change in &action.files {
                    self.decide_write(change, &mut decided);
                }
            }
            Operation::Execute => {
                decided.verdict.cost = self.costs.execute;
                match &action.command {
                    None => deny(
                        &mut decided,
                        "the shell command is not stated, so it cannot be checked".into(),
                        false,
                    ),
                    Some(command) => self.decide_command(command, &mut decided),
                }
            }
            Operation::Other => {
                decided.verdict.cost = self.costs.other;
                if let Some(argument) = action
                    .arguments
                    .iter()
                    .find(|argument| self.names_protected(argument))
                {
                    deny(
                        &mut decided,
                        format!(
                            "{} names protected Crane metadata ({argument})",
                            action.tool
                        ),
                        true,
                    );
                }
                decided.network = network_destinations(&action.arguments);
                if !decided.network.is_empty() {
                    decided.verdict.cost += self.costs.network;
                }
                decided.resources.push(Resource {
                    kind: "tool".into(),
                    id: action.tool.clone(),
                    access: "other".into(),
                    classification: None,
                });
            }
        }
        for destination in &decided.network {
            decided.resources.push(Resource {
                kind: "host".into(),
                id: destination.clone(),
                access: "network".into(),
                classification: Some("external".into()),
            });
        }
        decided.selections.sort();
        decided.selections.dedup();
        decided.policies = decided
            .selections
            .iter()
            .filter_map(|id| self.state.record(id).map(|record| record.policy.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        self.gate(action.operation, &mut decided);
        decided
    }

    /** Apply integrity, autonomy mode, safety state, and credits to a decision; each can only
     * make it stricter, and none can turn a denial into an allow
     * Input
        - operation: Operation - kind of action
        - decided: &mut Decided - decision so far
     * Output
        - None (updates decided)
    */
    fn gate(&self, operation: Operation, decided: &mut Decided) {
        if decided.verdict.decision != Decision::Allow || operation == Operation::Read {
            if decided.verdict.decision == Decision::Allow {
                self.charge(decided);
            }
            return;
        }
        let verdict = &mut decided.verdict;
        if self.safety == Safety::Quarantined {
            verdict.decision = Decision::Quarantine;
            verdict.reasons.push("the session is quarantined; only reads are allowed until a human recovers it in a new session".into());
        } else if !self.integrity_ok {
            verdict.decision = Decision::Deny;
            verdict.reasons.push("Crane's signed governance state failed verification, so changes are denied (fail closed); a human must run 'crane validate'".into());
        } else if self.mode == "observe" {
            verdict.decision = Decision::Deny;
            verdict
                .reasons
                .push("the session is in observe mode; changes are not allowed".into());
        } else if self.mode == "assisted" {
            verdict.decision = Decision::RequireApproval;
            verdict
                .reasons
                .push("the session is in assisted mode; every change needs human approval".into());
        } else if self.safety == Safety::Degraded {
            verdict.decision = Decision::RequireApproval;
            verdict.reasons.push(
                "the session is degraded after a violation; changes need human approval".into(),
            );
        } else if let Some(error) = &self.zone_error {
            verdict.decision = Decision::Deny;
            verdict.reasons.push(format!(
                "the zone and flow configuration could not be resolved ({error}); changes are denied (fail closed)"
            ));
        } else if let Some((decision, reason)) = decided.zone_floor.clone() {
            decided.verdict.decision = decision;
            decided.verdict.reasons.push(reason);
        } else {
            self.charge(decided);
        }
    }

    /** Require approval when the action costs more credits than are available
     * Input
        - decided: &mut Decided - an allowed decision
     * Output
        - None (updates decided)
    */
    fn charge(&self, decided: &mut Decided) {
        if let Some(available) = self.available {
            if decided.verdict.cost > available {
                decided.verdict.decision = Decision::RequireApproval;
                decided.verdict.reasons.push(format!(
                    "autonomy credits exhausted (available {available}, this action costs {}); a human must approve",
                    decided.verdict.cost
                ));
            }
        }
    }

    /** Decide one file of a write
     * Input
        - change: &FileChange - proposed change
        - decided: &mut Decided - decision being built
     * Output
        - None (updates decided)
    */
    fn decide_write(&self, change: &FileChange, decided: &mut Decided) {
        let deny = |decided: &mut Decided, reason: String, bypass: bool| {
            decided.verdict.decision = Decision::Deny;
            decided.verdict.bypass |= bypass;
            decided.verdict.reasons.push(reason);
        };
        if self.names_protected(&change.path) {
            deny(decided, format!("writing {} is denied: Crane metadata, the trust directory, and hook settings cannot be modified by an agent, even with approval", change.path), true);
            decided.resources.push(Resource {
                kind: "metadata".into(),
                id: change.path.clone(),
                access: "write".into(),
                classification: Some("metadata".into()),
            });
            return;
        }
        let Some(path) = self.relative(&change.path) else {
            decided.resources.push(Resource {
                kind: "file".into(),
                id: change.path.clone(),
                access: "write".into(),
                classification: Some("external".into()),
            });
            return;
        };
        if let Some(limit) = self.limits.and_then(|limits| limits.get(&path)) {
            decided.zones.extend(limit.entities.iter().cloned());
            decided.verdict.cost += match limit.criticality {
                Criticality::Routine => 0,
                Criticality::Sensitive => 1,
                Criticality::Critical => 3,
                Criticality::Restricted => 5,
            };
            let names = limit.entities.join(", ");
            let floor = match (limit.ceiling, limit.criticality) {
                (Ceiling::Observe, _) => Some((
                    Decision::Deny,
                    format!("{path} is in {names}, whose autonomy ceiling is observe; agents cannot change it"),
                )),
                (Ceiling::Assisted, _) => Some((
                    Decision::RequireApproval,
                    format!("{path} is in {names}, whose autonomy ceiling is assisted; a human must approve the change"),
                )),
                (_, Criticality::Restricted) => Some((
                    Decision::RequireApproval,
                    format!("{path} is in {names}, which is restricted; a human must approve the change"),
                )),
                _ => None,
            };
            if let Some(floor) = floor {
                let stricter = decided.zone_floor.as_ref().is_none_or(|(current, _)| {
                    floor.0 == Decision::Deny && *current != Decision::Deny
                });
                if stricter {
                    decided.zone_floor = Some(floor);
                }
            }
        }
        let full = self.workspace.root.join(&path);
        let before = fs::read(&full)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok());
        let before_markers = before
            .as_deref()
            .map(|text| find_markers(&path, text))
            .unwrap_or_default();
        let holds_selections = !before_markers.is_empty()
            || self
                .state
                .registry
                .selections
                .iter()
                .any(|record| record.current.span.path == path);
        if holds_selections {
            decided.verdict.cost += self.costs.write_selection_file;
        }
        decided.resources.push(Resource {
            kind: "file".into(),
            id: path.clone(),
            access: if change.proposed == Proposed::Delete {
                "delete"
            } else {
                "write"
            }
            .into(),
            classification: holds_selections.then(|| "selection".into()),
        });
        let after = match apply(&change.proposed, before.as_deref()) {
            Ok(after) => after,
            Err(error) if holds_selections => {
                deny(decided, format!("{path} holds Crane selections and {error}, so the edit cannot be verified (fail closed)"), false);
                return;
            }
            Err(_) => return,
        };
        let Some(after) = after else {
            if holds_selections {
                deny(
                    decided,
                    format!("deleting {path} would remove Crane selections"),
                    true,
                );
            }
            return;
        };
        if scripts::is_script(&path) {
            let preserved = self.preserved_files();
            let protected = |text: &str| self.names_protected(text);
            let risks = scripts::code_risks(&after, &path, &protected, &preserved);
            self.apply_risks(risks, true, decided);
        }
        let after_markers = find_markers(&path, &after);
        let shape = |markers: &[Marker], text: &str| {
            let all = lines(text);
            markers
                .iter()
                .map(|marker| {
                    all.get(marker.line - 1)
                        .map(|line| line.text.trim().to_string())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
        };
        let before_shape = shape(&before_markers, before.as_deref().unwrap_or_default());
        let after_shape = shape(&after_markers, &after);
        if before_shape != after_shape {
            let known = before_markers
                .iter()
                .filter_map(|marker| marker.id.clone())
                .collect::<BTreeSet<_>>();
            let forged = after_markers
                .iter()
                .filter_map(|marker| marker.id.clone())
                .filter(|id| !known.contains(id))
                .collect::<BTreeSet<_>>();
            if forged.is_empty() {
                deny(decided, format!("the edit of {path} adds, removes, or alters Crane selection markers; markers are managed only by crane protect and crane target"), true);
            } else {
                deny(
                    decided,
                    format!(
                        "the edit of {path} writes Crane markers ({}) that Crane did not generate",
                        forged.into_iter().collect::<Vec<_>>().join(", ")
                    ),
                    true,
                );
            }
            return;
        }
        let before_scan = Scan::single(&path, before.as_deref().unwrap_or_default());
        let after_scan = Scan::single(&path, &after);
        for id in before_markers.iter().filter_map(|marker| marker.id.clone()) {
            let (Resolution::Resolved(old), Resolution::Resolved(new)) =
                (resolve(&id, &before_scan), resolve(&id, &after_scan))
            else {
                continue;
            };
            if old.content == new.content {
                continue;
            }
            decided.selections.push(id.clone());
            match self.state.record(&id) {
                Some(record) if record.operation == SelectionOperation::Preserve => deny(
                    decided,
                    format!(
                        "the edit would change preserved selection {id} ({path} lines {}-{}, policy {}); preserved code must stay as it was at checkpoint {}",
                        old.span.start_line, old.span.end_line, record.policy, record.origin.checkpoint
                    ),
                    false,
                ),
                Some(_) => {}
                None => deny(decided, format!("{path} holds markers of {id}, which is not in the signed registry"), true),
            }
        }
    }

    /** Apply what the script pre-check found: denials (when the code runs) and approvals
     * Input
        - risks: Vec<Risk> - findings
        - written: bool - the code is being written, not run: every finding then needs approval
        - decided: &mut Decided - decision being built
     * Output
        - None (updates decided)
    */
    fn apply_risks(&self, risks: Vec<scripts::Risk>, written: bool, decided: &mut Decided) {
        let mut seen = BTreeSet::new();
        for risk in risks {
            if !seen.insert(risk.reason.clone()) {
                continue;
            }
            if risk.deny && !written {
                decided.verdict.decision = Decision::Deny;
                decided.verdict.bypass |= risk.bypass;
                decided.verdict.reasons.push(risk.reason);
            } else if decided.zone_floor.is_none() {
                let reason = if written {
                    format!(
                        "{} (it is being written; a human must approve it)",
                        risk.reason
                    )
                } else {
                    risk.reason
                };
                decided.zone_floor = Some((Decision::RequireApproval, reason));
            }
        }
    }

    /** Decide a shell command
     * Input
        - command: &str - command text
        - decided: &mut Decided - decision being built
     * Output
        - None (updates decided)
    */
    fn decide_command(&self, command: &str, decided: &mut Decided) {
        let deny = |decided: &mut Decided, reason: String, bypass: bool| {
            decided.verdict.decision = Decision::Deny;
            decided.verdict.bypass |= bypass;
            decided.verdict.reasons.push(reason);
        };
        decided.resources.push(Resource {
            kind: "command".into(),
            id: command.chars().take(240).collect(),
            access: "execute".into(),
            classification: None,
        });
        if self.names_protected(command) {
            deny(decided, "the command names protected Crane metadata (.crane, the trust directory, or hook settings); agents cannot read or change it through the shell".into(), true);
        }
        if let Some(invocation) = mutating_crane(command) {
            deny(decided, format!("'{invocation}' cannot be executed by an AI agent; a human runs governance commands"), true);
        }
        let tokens = words(command);
        let mutating = command.contains('>')
            || tokens.iter().any(|token| {
                MUTATING_WORDS.contains(&token.rsplit('/').next().unwrap_or_default())
            })
            || tokens
                .windows(2)
                .any(|pair| pair[0] == "git" && MUTATING_GIT.contains(&pair[1].as_str()))
            || tokens.windows(2).any(|pair| {
                matches!(pair[0].as_str(), "sed" | "perl" | "ruby") && pair[1].starts_with("-i")
            })
            || tokens
                .iter()
                .any(|token| token == "-i" || token.starts_with("--in-place"));
        if mutating {
            let lowered = normal(command);
            for path in self.preserved_files() {
                let name = path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&path)
                    .to_ascii_lowercase();
                if lowered.contains(&path.to_ascii_lowercase())
                    || tokens
                        .iter()
                        .any(|token| token.rsplit('/').next() == Some(name.as_str()))
                {
                    deny(decided, format!("the shell command would modify {path}, which holds preserved selections; use the file edit tools so Crane can check the change"), false);
                }
            }
        }
        // Script pre-check: the command line itself, and the scripts and inline code it runs
        let directory = self
            .cwd
            .clone()
            .unwrap_or_else(|| self.workspace.root.clone());
        let preserved = self.preserved_files();
        let protected = |text: &str| self.names_protected(text);
        let mut risks = scripts::command_risks(command);
        for source in scripts::sources(command, &directory) {
            if let Some((code, origin)) = scripts::read(&source) {
                risks.extend(scripts::code_risks(&code, &origin, &protected, &preserved));
            }
        }
        self.apply_risks(risks, false, decided);
        decided.network = network_destinations(&[command.to_string()]);
        if tokens
            .iter()
            .any(|token| NETWORK_WORDS.contains(&token.as_str()))
            && decided.network.is_empty()
        {
            decided.network.push("unknown-destination".into());
        }
        if !decided.network.is_empty() {
            decided.verdict.cost += self.costs.network;
        }
    }
}

/** Resolve a path named by a tool (absolute, or relative to the agent's working directory or else
 * the repository root) to a repository-relative path
 * Input
    - workspace: &Workspace - repository
    - cwd: Option<&Path> - the agent's working directory
    - raw: &str - path as the tool gave it
 * Output
    - Option<String>, None when the path lies outside the repository
*/
pub(crate) fn repository_path(
    workspace: &Workspace,
    cwd: Option<&Path>,
    raw: &str,
) -> Option<String> {
    let path = Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.unwrap_or(&workspace.root).join(path)
    };
    git::relative(&workspace.root, &absolute)
}

/** Simulate the enforcement boundary for one policy (policy status): a write changing each
 * preserved selection must be denied, a write changing each target allowed, and writes to
 * .crane/map, marker removal, agent-run governance commands, and reads of the trust directory
 * denied
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - policy: &str - policy name
 * Output
    - Vec<(String, &'static str, String)> check, status, detail
*/
pub(crate) fn self_test(
    workspace: &Workspace,
    state: &State,
    policy: &str,
) -> Vec<(String, &'static str, String)> {
    let guard = Guard {
        workspace,
        state,
        integrity_ok: true,
        protected_paths: &[
            ".claude/settings.json",
            ".claude/settings.local.json",
            ".codex/hooks.json",
        ],
        trust_home: crate::trust::crane_home().ok(),
        cwd: Some(workspace.root.clone()),
        mode: "delegated".into(),
        safety: Safety::Active,
        available: None,
        costs: Costs::default(),
        limits: None,
        zone_error: None,
    };
    let action = |operation: Operation,
                  files: Vec<FileChange>,
                  command: Option<String>,
                  reads: Vec<String>| AgentAction {
        tool: "simulation".into(),
        operation,
        files,
        reads,
        command,
        arguments: Vec::new(),
        digest: String::new(),
    };
    let mut checks = Vec::new();
    let expect = |name: String, decided: Decided, allowed: bool| {
        let passed = (decided.verdict.decision == Decision::Allow) == allowed;
        (
            name,
            if passed { "PASS" } else { "FAIL" },
            format!(
                "{} ({})",
                if decided.verdict.decision == Decision::Allow {
                    "allowed".to_string()
                } else {
                    format!("{:?}", decided.verdict.decision).to_uppercase()
                },
                if decided.verdict.reasons.is_empty() {
                    "no objection".into()
                } else {
                    decided.verdict.reasons.join("; ")
                }
            ),
        )
    };
    for record in state
        .registry
        .selections
        .iter()
        .filter(|record| record.policy == policy)
    {
        let path = record.current.span.path.clone();
        let Ok(text) = fs::read_to_string(workspace.root.join(&path)) else {
            checks.push((
                format!("hook: pre-write {}", record.id),
                "FAIL",
                format!("{path} cannot be read"),
            ));
            continue;
        };
        let markers = find_markers(&path, &text);
        let scan = Scan::single(&path, &text);
        let Resolution::Resolved(location) = resolve(&record.id, &scan) else {
            checks.push((
                format!("hook: pre-write {}", record.id),
                "FAIL",
                "the selection is unresolved".into(),
            ));
            continue;
        };
        let changed = format!(
            "{}{}crane_policy_status_probe = 1\n{}",
            &text[..location.span.start_byte],
            &text[location.span.start_byte..location.span.end_byte],
            &text[location.span.end_byte..]
        );
        let decided = guard.decide(&action(
            Operation::Write,
            vec![FileChange {
                path: path.clone(),
                proposed: Proposed::Content(changed),
            }],
            None,
            Vec::new(),
        ));
        checks.push(expect(
            format!("hook: pre-write {} {}", record.operation.name(), record.id),
            decided,
            record.operation == SelectionOperation::Target,
        ));
        let start = markers
            .iter()
            .find(|marker| marker.id.as_deref() == Some(record.id.as_str()))
            .map(|marker| marker.start);
        if let Some(start) = start {
            let end = text[start..]
                .find('\n')
                .map_or(text.len(), |offset| start + offset + 1);
            let stripped = format!("{}{}", &text[..start], &text[end..]);
            let decided = guard.decide(&action(
                Operation::Write,
                vec![FileChange {
                    path: path.clone(),
                    proposed: Proposed::Content(stripped),
                }],
                None,
                Vec::new(),
            ));
            checks.push(expect(
                format!("hook: marker removal {}", record.id),
                decided,
                false,
            ));
        }
    }
    let map = workspace
        .root
        .join(".crane")
        .join("map")
        .to_string_lossy()
        .into_owned();
    checks.push(expect(
        "hook: write .crane/map".into(),
        guard.decide(&action(
            Operation::Write,
            vec![FileChange {
                path: map,
                proposed: Proposed::Content(String::new()),
            }],
            None,
            Vec::new(),
        )),
        false,
    ));
    checks.push(expect(
        "hook: agent runs crane protect".into(),
        guard.decide(&action(
            Operation::Execute,
            Vec::new(),
            Some("crane protect src/app.py".into()),
            Vec::new(),
        )),
        false,
    ));
    checks.push(expect(
        "hook: agent runs crane validate".into(),
        guard.decide(&action(
            Operation::Execute,
            Vec::new(),
            Some("crane validate".into()),
            Vec::new(),
        )),
        true,
    ));
    if let Some(home) = &guard.trust_home {
        let key = home
            .join("keys")
            .join("repository.key")
            .to_string_lossy()
            .into_owned();
        checks.push(expect(
            "hook: pre-read of signing keys".into(),
            guard.decide(&action(Operation::Read, Vec::new(), None, vec![key])),
            false,
        ));
    }
    checks
}
