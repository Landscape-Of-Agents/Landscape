// Resolution and composition: an attachment resolves to exact Brick versions (a Stack to its
// members), and the controls of every Brick on a target combine monotonically: the lowest ceiling,
// the highest criticality, every read-only and unreadable path, every scope (a file must be inside
// all of them), the strictest mode per operation, every identity rule (a caller must satisfy all),
// every approval rule, every cap. Nothing in a Stack, a context, another attachment, or a shared
// configuration can loosen what a Brick restricts. A pin Crane cannot honor (revoked, unresolved),
// a configuration that no longer parses, or a share that does not resolve fails closed: changes in
// the target need a person's approval until an admin fixes it.
//
// The controls reach enforcement twice: brick::effect folds what is per file (ceiling, read-only,
// scope, isolation, file preconditions) into the guard's cached limits, and brick::guard decides
// what depends on the call (identity, reads, operation modes, approvals, caps, environment, direct
// operations, gates that change without a file changing).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;

use super::config::{
    self, ChangeClass, Check, Config, Escalation, LimitsConfig, Mode, Problem, Reaction,
};
use super::model::{Attachment, AttachmentState, BrickEntry, BrickRef, Subject, VersionState};
use super::store::BrickStore;
use crate::governance::zones::{Ceiling, Criticality};
use crate::platform::glob;

/** One Brick an attachment resolves to
 * Fields
    - reference: String - "ID@N" (or the Stack reference when the Stack itself is unresolved)
    - entry: Option<&BrickEntry> - the activated version, None when unresolved
    - via: Option<String> - the Stack it came through
    - issue: Option<&'static str> - PIN_UNRESOLVED, PIN_REVOKED, STACK_UNRESOLVED, STACK_REVOKED
    - deprecated: bool - pinned to a deprecated version (still applied)
*/
#[derive(Debug, Clone)]
pub(crate) struct Member<'a> {
    pub(crate) reference: String,
    pub(crate) entry: Option<&'a BrickEntry>,
    pub(crate) via: Option<String>,
    pub(crate) issue: Option<&'static str>,
    pub(crate) deprecated: bool,
}

/** Resolve a subject to its Brick versions
 * Input
    - store: &BrickStore - signed store
    - subject: &Subject - attached Brick or Stack
 * Output
    - Vec<Member>
*/
pub(crate) fn members<'a>(store: &'a BrickStore, subject: &Subject) -> Vec<Member<'a>> {
    let one = |reference: BrickRef, via: Option<String>| {
        let entry = store.brick(&reference);
        let issue = match entry.map(|entry| entry.state) {
            None => Some("PIN_UNRESOLVED"),
            Some(VersionState::Revoked) => Some("PIN_REVOKED"),
            _ => None,
        };
        Member {
            reference: reference.to_string(),
            entry: entry.filter(|entry| entry.state != VersionState::Revoked),
            via,
            issue,
            deprecated: entry.is_some_and(|entry| entry.state == VersionState::Deprecated),
        }
    };
    match subject {
        Subject::Brick { brick_id, version } => vec![one(
            BrickRef {
                brick_id: brick_id.clone(),
                version: *version,
            },
            None,
        )],
        Subject::Stack { stack_id, version } => {
            let text = subject.text();
            match store.stack(stack_id, *version) {
                None => vec![Member {
                    reference: text,
                    entry: None,
                    via: None,
                    issue: Some("STACK_UNRESOLVED"),
                    deprecated: false,
                }],
                Some(entry) if entry.state == VersionState::Revoked => vec![Member {
                    reference: text,
                    entry: None,
                    via: None,
                    issue: Some("STACK_REVOKED"),
                    deprecated: false,
                }],
                Some(entry) => entry
                    .stack
                    .members
                    .iter()
                    .map(|member| one(member.clone(), Some(text.clone())))
                    .collect(),
            }
        }
    }
}

/** The enabled attachments of a target
 * Input
    - store: &BrickStore - signed store
    - target_key: &str - "flow:ID" or "zone:ID"
 * Output
    - Vec<&Attachment>
*/
pub(crate) fn attachments_of<'a>(store: &'a BrickStore, target_key: &str) -> Vec<&'a Attachment> {
    store
        .attachments
        .iter()
        .filter(|attachment| {
            attachment.state == AttachmentState::Enabled && attachment.target.key() == target_key
        })
        .collect()
}

/** One readable configuration of a target: (Brick reference, Brick id, configuration) */
pub(crate) type Configured = (String, String, Config);

/** The current configurations of every Brick on a target, with their references
 * Input
    - store: &BrickStore - signed store
    - target_key: &str - "flow:ID" or "zone:ID"
 * Output
    - (Vec<(String, String, Config)>, Vec<String>) (reference, Brick id, configuration) of each
      readable Brick (Configured), and the pins that cannot be honored
*/
pub(crate) fn configs_of(
    store: &BrickStore,
    target_key: &str,
) -> (Vec<Configured>, Vec<(String, String)>) {
    let mut configs = Vec::new();
    let mut unhonored = Vec::new();
    for attachment in attachments_of(store, target_key) {
        for member in members(store, &attachment.subject) {
            if let Some(issue) = member.issue {
                unhonored.push((
                    member.reference.clone(),
                    format!(
                        "{} ({issue}) in attachment {}",
                        member.reference, attachment.attachment_id
                    ),
                ));
                continue;
            }
            let Some(entry) = member.entry else { continue };
            let reference = entry.brick.reference().to_string();
            match config::parse(entry.brick.brick_type, &entry.brick.config) {
                Ok(parsed) => configs.extend(
                    parsed
                        .into_iter()
                        .map(|config| (reference.clone(), entry.brick.brick_id.clone(), config)),
                ),
                // An activated configuration that no longer parses cannot be honored: fail closed
                Err(_) => unhonored.push((
                    reference.clone(),
                    format!("{reference} (CONFIG_UNREADABLE)"),
                )),
            }
        }
    }
    (configs, unhonored)
}

/** Files of a scope: in include (or every file when include is empty), outside exclude
 * Fields
    - include: Vec<String> - patterns a file must match (empty: all)
    - exclude: Vec<String> - patterns a file must not match
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct PathSet {
    pub(crate) include: Vec<String>,
    pub(crate) exclude: Vec<String>,
}

impl PathSet {
    /** Report whether a file is in the set
     * Input
        - file: &str - repository-relative file
     * Output
        - bool
    */
    pub(crate) fn contains(&self, file: &str) -> bool {
        (self.include.is_empty()
            || self
                .include
                .iter()
                .any(|pattern| glob::matches(pattern, file)))
            && !self
                .exclude
                .iter()
                .any(|pattern| glob::matches(pattern, file))
    }

    /** Report whether the set restricts anything
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn restricts(&self) -> bool {
        !self.include.is_empty() || !self.exclude.is_empty()
    }
}

/** An identity rule of the target
 * Fields
    - agents: Vec<String> - admitted providers or agent ids
    - models: Vec<String> - admitted model patterns (empty: any)
    - on_other: Mode - ask or deny for anyone else
    - source: String - Brick reference
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct IdentityRule {
    pub(crate) agents: Vec<String>,
    pub(crate) models: Vec<String>,
    pub(crate) on_other: Mode,
    pub(crate) source: String,
}

/** An approval rule of the target
 * Fields
    - classes: Vec<ChangeClass> - kinds of change
    - paths: PathSet - the files it covers
    - min_lines_changed: Option<u32> - large_change size
    - reason: String - shown to the person
    - source: String - Brick reference
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ApprovalRule {
    pub(crate) classes: Vec<ChangeClass>,
    pub(crate) paths: PathSet,
    pub(crate) min_lines_changed: Option<u32>,
    pub(crate) reason: String,
    pub(crate) source: String,
}

impl ApprovalRule {
    /** Report whether the rule holds every change of its files (write or update), which the
     * per-file limits can express; finer kinds of change are decided per call
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn holds_writes(&self) -> bool {
        self.classes
            .iter()
            .any(|class| matches!(class, ChangeClass::Write | ChangeClass::Update))
    }
}

/** A set of caps of the target
 * Fields
    - limits: LimitsConfig - caps
    - paths: PathSet - the files counted (shared from an Access Brick)
    - source: String - Brick reference
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct CapRule {
    pub(crate) limits: LimitsConfig,
    pub(crate) paths: PathSet,
    pub(crate) source: String,
}

/** A gate checked on every call (one a changed file would not reveal)
 * Fields
    - check: Check - the gate
    - on_fail: Mode - ask or deny
    - source: String - Brick reference
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct GateRule {
    pub(crate) check: Check,
    pub(crate) on_fail: Mode,
    pub(crate) source: String,
}

/** The combined controls of every Brick on a target
 * Fields
    - ceiling: Option<Ceiling> - lowest ceiling a Limits Brick sets
    - criticality: Option<Criticality> - highest criticality a Limits Brick sets
    - read_only: Vec<String> - patterns agents may read but not change
    - deny_read: Vec<String> - patterns agents may not read
    - scopes: Vec<PathSet> - scopes; a changed file must be inside every one
    - create, update, delete, execute, network: Option<Mode> - strictest mode per operation
    - identities: Vec<IdentityRule> - identity rules (a caller must satisfy each)
    - approvals: Vec<ApprovalRule> - approval rules
    - caps: Vec<CapRule> - caps per session
    - hide_env: Vec<String> - variables agents may not read
    - require_env: Vec<String> - variables that must be set
    - network_allow: Vec<Vec<String>> - host allow-lists (a host must match each)
    - network_deny: Vec<String> - hosts never reached
    - credentials: Vec<String> - credentials the target's operations use (vault names)
    - isolation: Vec<String> - required profiles that agent tool actions cannot satisfy
    - preconditions: Vec<String> - failing file preconditions that ask
    - blocked: Vec<String> - failing file preconditions that deny
    - gates: Vec<GateRule> - gates checked on every call
    - direct: Vec<(String, String)> - operations agents may not do themselves (adapter, operation)
    - operations: Vec<(String, String)> - operations Crane may run for the target (empty: no Action
      Brick)
    - max_consecutive_failures: Option<u32> - failures in a row before changes need a person
    - watch_pause: Vec<String> - Watch Bricks that pause the target while they fire
    - fail_closed: Vec<String> - pins and shares Crane cannot honor
    - sources: BTreeSet<String> - Brick references that set an enforced control
    - origins: BTreeMap<String, BTreeSet<String>> - the Brick references behind each merged control,
      by control key (see Controls::origin), so a finding names the Bricks whose rule decided it
    - gaps: Vec<Problem> - restrictions declared but not enforced
*/
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Controls {
    pub(crate) ceiling: Option<Ceiling>,
    pub(crate) criticality: Option<Criticality>,
    pub(crate) read_only: Vec<String>,
    pub(crate) deny_read: Vec<String>,
    pub(crate) scopes: Vec<PathSet>,
    pub(crate) create: Option<Mode>,
    pub(crate) update: Option<Mode>,
    pub(crate) delete: Option<Mode>,
    pub(crate) execute: Option<Mode>,
    pub(crate) network: Option<Mode>,
    pub(crate) identities: Vec<IdentityRule>,
    pub(crate) approvals: Vec<ApprovalRule>,
    pub(crate) caps: Vec<CapRule>,
    pub(crate) hide_env: Vec<String>,
    pub(crate) require_env: Vec<String>,
    pub(crate) network_allow: Vec<Vec<String>>,
    pub(crate) network_deny: Vec<String>,
    pub(crate) credentials: Vec<String>,
    pub(crate) isolation: Vec<String>,
    pub(crate) preconditions: Vec<String>,
    pub(crate) blocked: Vec<String>,
    pub(crate) gates: Vec<GateRule>,
    pub(crate) direct: Vec<(String, String)>,
    pub(crate) operations: Vec<(String, String)>,
    pub(crate) max_consecutive_failures: Option<u32>,
    pub(crate) watch_pause: Vec<String>,
    pub(crate) fail_closed: Vec<String>,
    pub(crate) sources: BTreeSet<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) origins: BTreeMap<String, BTreeSet<String>>,
    pub(crate) gaps: Vec<Problem>,
}

impl Controls {
    /** Report whether any control folded into the per-file limits is set
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn restricts(&self) -> bool {
        self.ceiling.is_some()
            || self.criticality.is_some()
            || !self.read_only.is_empty()
            || self.scopes.iter().any(PathSet::restricts)
            || !self.isolation.is_empty()
            || !self.preconditions.is_empty()
            || !self.blocked.is_empty()
            || !self.fail_closed.is_empty()
            || matches!(self.update, Some(Mode::Ask | Mode::Deny))
            || self.approvals.iter().any(ApprovalRule::holds_writes)
    }

    /** Report whether any control decided per call is set
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn decides_calls(&self) -> bool {
        !self.deny_read.is_empty()
            || [
                self.create,
                self.update,
                self.delete,
                self.execute,
                self.network,
            ]
            .iter()
            .any(Option::is_some)
            || !self.identities.is_empty()
            || !self.approvals.is_empty()
            || !self.caps.is_empty()
            || !self.hide_env.is_empty()
            || !self.require_env.is_empty()
            || !self.network_allow.is_empty()
            || !self.network_deny.is_empty()
            || !self.gates.is_empty()
            || !self.direct.is_empty()
            || self.max_consecutive_failures.is_some()
            || !self.watch_pause.is_empty()
    }

    /** The mode of an operation (None: no Brick sets one)
             * Input
                - class: ChangeClass - Create, Update, Delete, Execute, or Network
             * Output
                - Option<Mode>
            */
    /** The Bricks behind merged controls, for a finding: the references recorded for the keys
             * (every Brick that sets an enforced control when none is recorded)
             * Input
                - keys: impl IntoIterator<Item = String> - control keys: "deny_read:PATTERN",
                  "read_only:PATTERN", "mode:OPERATION:MODE" (see mode_key), "hide_env:PATTERN",
                  "require_env:NAME", "network_deny:PATTERN", "network_allow:INDEX", "recovery:LIMIT",
                  "direct:ADAPTER:OPERATION"
             * Output
                - String - references joined by ", "
            */
    pub(crate) fn origin(&self, keys: impl IntoIterator<Item = String>) -> String {
        let found = keys
            .into_iter()
            .filter_map(|key| self.origins.get(&key))
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        let chosen = if found.is_empty() {
            &self.sources
        } else {
            &found
        };
        chosen.iter().cloned().collect::<Vec<_>>().join(", ")
    }

    /** The control key of an operation's mode; the Bricks that set the strictest mode are the
     * ones recorded under the final mode's key
     * Input
        - class: ChangeClass - operation
        - mode: Mode - mode
     * Output
        - String, such as "mode:delete:deny"
    */
    pub(crate) fn mode_key(class: ChangeClass, mode: Mode) -> String {
        format!("mode:{}:{}", config::serial(&class), config::serial(&mode))
    }

    /** Record a Brick behind a merged control
     * Input
        - key: String - control key
        - reference: &str - Brick reference
     * Output
        - None
    */
    fn note(&mut self, key: String, reference: &str) {
        self.origins
            .entry(key)
            .or_default()
            .insert(reference.to_string());
    }

    pub(crate) fn mode(&self, class: ChangeClass) -> Option<Mode> {
        match class {
            ChangeClass::Create => self.create,
            ChangeClass::Update => self.update,
            ChangeClass::Delete => self.delete,
            ChangeClass::Execute => self.execute,
            ChangeClass::Network => self.network,
            _ => None,
        }
    }
}

/** Report whether a check depends only on files (so the cached limits see it change)
 * Input
    - check: &Check - check
 * Output
    - bool
*/
fn file_based(check: &Check) -> bool {
    matches!(
        check,
        Check::FileExists { .. }
            | Check::FileAbsent { .. }
            | Check::FileContains { .. }
            | Check::SignedState
    )
}

/** Evaluate a file precondition against the working tree (no Git, no policy evaluation)
 * Input
    - root: &Path - repository root
    - check: &Check - check
 * Output
    - Option<bool> - None for checks that are not file checks
*/
pub(crate) fn cheap_check(root: &Path, check: &Check) -> Option<bool> {
    match check {
        Check::FileExists { path } => Some(root.join(path).is_file()),
        Check::FileAbsent { path } => Some(!root.join(path).exists()),
        Check::FileContains { path, text } => Some(
            std::fs::read_to_string(root.join(path))
                .is_ok_and(|content| content.contains(text.as_str())),
        ),
        // Integrity failures already stop every agent change, so a precondition on it never loosens
        Check::SignedState => Some(true),
        _ => None,
    }
}

/** The stricter of two optional modes
 * Input
    - current: Option<Mode> - mode so far
    - next: Option<Mode> - another Brick's mode
 * Output
    - Option<Mode>
*/
fn stricter(current: Option<Mode>, next: Option<Mode>) -> Option<Mode> {
    match (current, next) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/** The scope an Access Brick on the same target shares, by Brick id
 * Input
    - configs: &[Configured] - the target's configurations
    - id: &str - the sharing Brick's id
 * Output
    - Option<PathSet>
*/
fn shared_scope(configs: &[Configured], id: &str) -> Option<PathSet> {
    configs
        .iter()
        .find_map(|(_, brick_id, config)| match config {
            Config::Access(access) if brick_id == id => Some(PathSet {
                include: access.include.clone(),
                exclude: access.exclude.clone(),
            }),
            _ => None,
        })
}

/** Combine the controls of every enabled attachment of a target
 * Input
    - store: &BrickStore - signed store
    - target_key: &str - "flow:ID" or "zone:ID"
    - root: &Path - repository root (file preconditions)
 * Output
    - Controls
*/
pub(crate) fn controls(store: &BrickStore, target_key: &str, root: &Path) -> Controls {
    let mut controls = Controls::default();
    let (configs, unhonored) = configs_of(store, target_key);
    for (reference, message) in unhonored {
        controls.fail_closed.push(message);
        controls.sources.insert(reference);
    }
    for (reference, _, config) in &configs {
        // A share that names no Access Brick on this target cannot be honored: fail closed
        let share = match config {
            Config::Verification(_) => None,
            other => other
                .shares_from()
                .map(|id| (id, shared_scope(&configs, id))),
        };
        if let Some((id, None)) = &share {
            controls.fail_closed.push(format!("{reference} shares the scope of '{id}', which is not an access Brick attached to {target_key} (SHARE_UNRESOLVED)"));
            controls.sources.insert(reference.clone());
            continue;
        }
        let shared = share.and_then(|(_, scope)| scope).unwrap_or_default();
        let mut enforced = true;
        match config.clone() {
            Config::Limits(limits) => {
                if let Some(ceiling) = limits.max_ceiling {
                    controls.ceiling = Some(controls.ceiling.map_or(ceiling, |c| c.min(ceiling)));
                }
                if let Some(criticality) = limits.min_criticality {
                    controls.criticality = Some(
                        controls
                            .criticality
                            .map_or(criticality, |c| c.max(criticality)),
                    );
                }
                let counts = [
                    limits.max_tool_calls,
                    limits.max_runtime_minutes,
                    limits.max_files_changed,
                    limits.max_lines_changed,
                    limits.max_commands,
                    limits.max_network_calls,
                    limits.max_denials,
                ];
                if counts.iter().any(Option::is_some) {
                    controls.caps.push(CapRule {
                        limits,
                        paths: shared,
                        source: reference.clone(),
                    });
                } else if limits.max_retries.is_some()
                    && limits.max_ceiling.is_none()
                    && limits.min_criticality.is_none()
                {
                    controls.gaps.push(config::problem("RETRIES_FOR_OPERATIONS", format!("{reference} caps retries, which apply to resident agents and skills, not to agent tool calls")));
                }
            }
            Config::Access(access) => {
                for pattern in &access.read_only {
                    controls.note(format!("read_only:{pattern}"), reference);
                }
                for pattern in &access.deny_read {
                    controls.note(format!("deny_read:{pattern}"), reference);
                }
                for class in [
                    ChangeClass::Create,
                    ChangeClass::Update,
                    ChangeClass::Delete,
                    ChangeClass::Execute,
                    ChangeClass::Network,
                ] {
                    if let Some(mode) = access.mode(class) {
                        controls.note(Controls::mode_key(class, mode), reference);
                    }
                }
                controls.read_only.extend(access.read_only.iter().cloned());
                controls.deny_read.extend(access.deny_read.iter().cloned());
                if !access.include.is_empty() || !access.exclude.is_empty() {
                    controls.scopes.push(PathSet {
                        include: access.include.clone(),
                        exclude: access.exclude.clone(),
                    });
                }
                controls.create = stricter(controls.create, access.mode(ChangeClass::Create));
                controls.update = stricter(controls.update, access.mode(ChangeClass::Update));
                controls.delete = stricter(controls.delete, access.mode(ChangeClass::Delete));
                controls.execute = stricter(controls.execute, access.mode(ChangeClass::Execute));
                controls.network = stricter(controls.network, access.mode(ChangeClass::Network));
            }
            Config::Identity(identity) => controls.identities.push(IdentityRule {
                agents: identity.agents,
                models: identity.models,
                on_other: identity.on_other,
                source: reference.clone(),
            }),
            Config::Approval(approval) => {
                let mut paths = if approval.scope_from.is_some() {
                    shared
                } else {
                    PathSet {
                        include: approval.paths.clone(),
                        exclude: Vec::new(),
                    }
                };
                paths.exclude.extend(approval.except.iter().cloned());
                controls.approvals.push(ApprovalRule {
                    classes: approval.operations,
                    paths,
                    min_lines_changed: approval.min_lines_changed,
                    reason: approval.reason,
                    source: reference.clone(),
                });
            }
            Config::Environment(environment) => {
                if environment.profile != "agent-hooks" {
                    controls.isolation.push(environment.profile.clone());
                }
                for pattern in &environment.hide_env {
                    controls.note(format!("hide_env:{pattern}"), reference);
                }
                for name in &environment.require_env {
                    controls.note(format!("require_env:{name}"), reference);
                }
                for pattern in &environment.network.deny {
                    controls.note(format!("network_deny:{pattern}"), reference);
                }
                controls.hide_env.extend(environment.hide_env);
                controls.require_env.extend(environment.require_env);
                if !environment.network.allow.is_empty() {
                    let index = controls.network_allow.len();
                    controls.note(format!("network_allow:{index}"), reference);
                    controls.network_allow.push(environment.network.allow);
                }
                controls.network_deny.extend(environment.network.deny);
                controls.credentials.extend(
                    environment
                        .credentials
                        .into_iter()
                        .map(|credential| credential.name),
                );
            }
            Config::Precondition(precondition) => {
                for check in &precondition.checks {
                    if file_based(check) {
                        if cheap_check(root, check) == Some(false) {
                            let text = format!("{reference}: {}", check_text(check));
                            if precondition.on_fail == Mode::Deny {
                                controls.blocked.push(text);
                            } else {
                                controls.preconditions.push(text);
                            }
                        }
                    } else {
                        controls.gates.push(GateRule {
                            check: check.clone(),
                            on_fail: precondition.on_fail,
                            source: reference.clone(),
                        });
                    }
                }
            }
            Config::Action(action) => {
                for rule in action.rules() {
                    controls
                        .operations
                        .push((rule.adapter.clone(), rule.operation.clone()));
                    if action.deny_direct {
                        controls.note(
                            format!("direct:{}:{}", rule.adapter, rule.operation),
                            reference,
                        );
                        controls.direct.push((rule.adapter, rule.operation));
                    }
                }
                enforced = action.deny_direct;
            }
            Config::Recovery(recovery) => match recovery.max_consecutive_failures {
                Some(limit) => {
                    controls.note(format!("recovery:{limit}"), reference);
                    controls.max_consecutive_failures = Some(
                        controls
                            .max_consecutive_failures
                            .map_or(limit, |current| current.min(limit)),
                    )
                }
                None => enforced = false,
            },
            Config::Watch(watch) => {
                if watch.on_fire.contains(&Reaction::Pause) {
                    controls.watch_pause.push(reference.clone());
                } else {
                    enforced = false;
                }
            }
            Config::Knowledge(_) | Config::Verification(_) | Config::Provenance(_) => {
                enforced = false
            }
        }
        if enforced {
            controls.sources.insert(reference.clone());
        }
    }
    if controls.execute.is_some() || controls.network.is_some() {
        controls.gaps.push(config::problem("COMMANDS_BY_FILES", "execute and network modes apply to commands and calls that name the target's files; a command naming no file cannot be attributed to a target"));
    }
    if controls
        .caps
        .iter()
        .any(|cap| cap.limits.on_exhausted == Escalation::Escalate)
    {
        controls.gaps.push(config::problem(
            "ESCALATE_BY_PROMPT",
            "an escalated cap is answered by the agent tool's own approval prompt",
        ));
    }
    controls
}

/** Describe a check in words
 * Input
    - check: &Check - check
 * Output
    - String
*/
pub(crate) fn check_text(check: &Check) -> String {
    match check {
        Check::FileExists { path } => format!("{path} exists"),
        Check::FileAbsent { path } => format!("{path} is absent"),
        Check::FileContains { path, text } => format!("{path} contains '{text}'"),
        Check::EnvSet { name } => format!("{name} is set"),
        Check::BranchIs { patterns } => format!("the branch is {}", patterns.join(" or ")),
        Check::BranchNot { patterns } => format!("the branch is not {}", patterns.join(" or ")),
        Check::GitClean => "the working tree is clean".into(),
        Check::TimeWindow {
            days,
            from,
            to,
            utc_offset_minutes,
        } => format!(
            "now is {from}-{to}{} (UTC{:+})",
            if days.is_empty() {
                String::new()
            } else {
                format!(" on {}", days.join(", "))
            },
            utc_offset_minutes / 60
        ),
        Check::CommandAvailable { name } => format!("{name} is installed"),
        Check::CredentialPresent { name } => format!("the vault holds {name}"),
        Check::SelectionsResolved => "every selection resolves".into(),
        Check::PolicyTestsPass => "every policy command passes".into(),
        Check::SignedState => "the signed state verifies".into(),
        Check::HttpOk { url, status, .. } => format!(
            "{url} answers {}",
            status.map_or("2xx".to_string(), |code| code.to_string())
        ),
        Check::JsonValue {
            path,
            pointer,
            op,
            value,
        } => format!(
            "{path}{pointer} is {} {value}",
            config::serial(op).replace('_', " ")
        ),
        Check::LocalMetric {
            metric,
            window_hours,
            max,
        } => format!(
            "{} in {window_hours} h is at most {max}",
            config::serial(metric).replace('_', " ")
        ),
        Check::CommandSucceeds { program, args, .. } => {
            format!("'{program} {}' succeeds", args.join(" "))
        }
    }
}

/** The limit the controls put on one file of the target
 * Input
    - controls: &Controls - combined controls
    - file: &str - repository-relative file
    - scope_resolved: bool - every scope matches at least one file of the target
 * Output
    - (Option<Ceiling>, Option<Criticality>, Vec<String>) ceiling and criticality to apply (None:
      no change) and the reasons
*/
pub(crate) fn file_limit(
    controls: &Controls,
    file: &str,
    scope_resolved: bool,
) -> (Option<Ceiling>, Option<Criticality>, Vec<String>) {
    let mut ceiling = controls.ceiling;
    let mut reasons = Vec::new();
    if let Some(max) = controls.ceiling {
        reasons.push(format!("a limits Brick caps the ceiling at {}", max.name()));
    }
    let mut lower = |to: Ceiling, reason: String| {
        ceiling = Some(ceiling.map_or(to, |current| current.min(to)));
        reasons.push(reason);
    };
    if controls
        .read_only
        .iter()
        .any(|pattern| glob::matches(pattern, file))
    {
        lower(Ceiling::Observe, format!("{file} is read-only"));
    }
    if controls
        .deny_read
        .iter()
        .any(|pattern| glob::matches(pattern, file))
    {
        lower(Ceiling::Observe, format!("{file} may not be read"));
    }
    // Changing an existing file is what the per-file limits decide: an update mode and an
    // approval of every write fold in; creating, deleting, and finer kinds are decided per call
    match controls.update {
        Some(Mode::Deny) => lower(
            Ceiling::Observe,
            "an access Brick does not allow changing files".into(),
        ),
        Some(Mode::Ask) => lower(
            Ceiling::Assisted,
            "an access Brick asks a person before files change".into(),
        ),
        _ => {}
    }
    if controls
        .approvals
        .iter()
        .any(|rule| rule.holds_writes() && rule.paths.contains(file))
    {
        lower(
            Ceiling::Assisted,
            "an approval Brick asks for a person".into(),
        );
    }
    if controls.scopes.iter().any(PathSet::restricts) {
        if !scope_resolved {
            lower(Ceiling::Observe, "an access Brick's scope matches no file of the target (unresolved scope never widens)".into());
        } else if !controls.scopes.iter().all(|scope| scope.contains(file)) {
            lower(
                Ceiling::Observe,
                format!("{file} is outside the access scope"),
            );
        }
    }
    if !controls.isolation.is_empty() {
        lower(
            Ceiling::Observe,
            format!(
                "isolation profile {} is required; agent tool actions run under agent-hooks",
                controls.isolation.join(", ")
            ),
        );
    }
    if !controls.blocked.is_empty() {
        lower(
            Ceiling::Observe,
            format!("precondition failing: {}", controls.blocked.join("; ")),
        );
    }
    if !controls.preconditions.is_empty() {
        lower(
            Ceiling::Assisted,
            format!(
                "precondition failing: {}",
                controls.preconditions.join("; ")
            ),
        );
    }
    if !controls.fail_closed.is_empty() {
        lower(
            Ceiling::Assisted,
            format!(
                "Bricks Crane cannot honor: {}",
                controls.fail_closed.join("; ")
            ),
        );
    }
    (ceiling, controls.criticality, reasons)
}

/** Report whether every restricting scope of the controls matches at least one of the target's
 * files
 * Input
    - controls: &Controls - controls
    - files: &BTreeSet<String> - the target's files
 * Output
    - bool
*/
pub(crate) fn scope_resolved(controls: &Controls, files: &BTreeSet<String>) -> bool {
    controls
        .scopes
        .iter()
        .filter(|scope| !scope.include.is_empty())
        .all(|scope| files.iter().any(|file| scope.contains(file)))
}
