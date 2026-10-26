// Resolution and composition: an attachment resolves to exact Brick versions (a Stack to its
// members), and the controls of every Brick on a target combine monotonically: the lowest ceiling,
// the highest criticality, every protected path, every scope (a file must be inside all of them),
// approval if any asks for it. Nothing in a Stack, a context, or another attachment can loosen what
// a Brick restricts. A pin Crane cannot honor (revoked, unresolved) fails closed: changes in the
// target need a person's approval until an admin detaches or upgrades it.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Serialize;

use super::config::{self, ActionClass, Check, Config, Problem};
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

/** The combined controls of every Brick on a target
 * Fields
    - ceiling: Option<Ceiling> - lowest ceiling a constraint sets
    - criticality: Option<Criticality> - highest criticality a constraint sets
    - protected: Vec<String> - protected path patterns
    - scopes: Vec<Vec<String>> - scope pattern sets; a file must match one pattern of each
    - read_only: bool - a permission Brick does not allow write
    - approval: bool - an approval Brick asks for write or delete approval
    - isolation: Vec<String> - required profiles that agent tool actions cannot satisfy
    - preconditions: Vec<String> - failing preconditions
    - fail_closed: Vec<String> - pins Crane cannot honor
    - sources: BTreeSet<String> - Brick references that set an enforced control
    - gaps: Vec<Problem> - restrictions declared but not enforced per file
*/
#[derive(Debug, Clone, Default, Serialize)]
pub(crate) struct Controls {
    pub(crate) ceiling: Option<Ceiling>,
    pub(crate) criticality: Option<Criticality>,
    pub(crate) protected: Vec<String>,
    pub(crate) scopes: Vec<Vec<String>>,
    pub(crate) read_only: bool,
    pub(crate) approval: bool,
    pub(crate) isolation: Vec<String>,
    pub(crate) preconditions: Vec<String>,
    pub(crate) fail_closed: Vec<String>,
    pub(crate) sources: BTreeSet<String>,
    pub(crate) gaps: Vec<Problem>,
}

impl Controls {
    /** Report whether any enforced control is set
     * Input
        - None (uses self)
     * Output
        - bool
    */
    pub(crate) fn restricts(&self) -> bool {
        self.ceiling.is_some()
            || self.criticality.is_some()
            || !self.protected.is_empty()
            || !self.scopes.is_empty()
            || self.read_only
            || self.approval
            || !self.isolation.is_empty()
            || !self.preconditions.is_empty()
            || !self.fail_closed.is_empty()
    }
}

/** Evaluate a cheap precondition check against the working tree (no Git, no policy evaluation)
 * Input
    - root: &Path - repository root
    - check: &Check - check
 * Output
    - Option<bool> - None for checks that are not evaluated at enforcement time
*/
pub(crate) fn cheap_check(root: &Path, check: &Check) -> Option<bool> {
    match check {
        Check::FileExists { path } => Some(root.join(path).is_file()),
        Check::FileAbsent { path } => Some(!root.join(path).exists()),
        // Integrity failures already stop every agent change, so a precondition on it never loosens
        Check::SignedState => Some(true),
        Check::SelectionsResolved | Check::PolicyTestsPass => None,
    }
}

/** Combine the controls of every enabled attachment of a target
 * Input
    - store: &BrickStore - signed store
    - target_key: &str - "flow:ID" or "zone:ID"
    - root: &Path - repository root (preconditions)
 * Output
    - Controls
*/
pub(crate) fn controls(store: &BrickStore, target_key: &str, root: &Path) -> Controls {
    let mut controls = Controls::default();
    for attachment in attachments_of(store, target_key) {
        for member in members(store, &attachment.subject) {
            if let Some(issue) = member.issue {
                controls.fail_closed.push(format!(
                    "{} ({issue}) in attachment {}",
                    member.reference, attachment.attachment_id
                ));
                controls.sources.insert(member.reference.clone());
                continue;
            }
            let Some(entry) = member.entry else { continue };
            let reference = entry.brick.reference().to_string();
            let Ok(parsed) = config::parse(entry.brick.brick_type, &entry.brick.config) else {
                // An activated configuration that no longer parses cannot be honored: fail closed
                controls
                    .fail_closed
                    .push(format!("{reference} (CONFIG_UNREADABLE)"));
                controls.sources.insert(reference);
                continue;
            };
            let mut enforced = true;
            match parsed {
                Config::Constraint(constraint) => {
                    if let Some(ceiling) = constraint.max_ceiling {
                        controls.ceiling =
                            Some(controls.ceiling.map_or(ceiling, |c| c.min(ceiling)));
                    }
                    if let Some(criticality) = constraint.min_criticality {
                        controls.criticality = Some(
                            controls
                                .criticality
                                .map_or(criticality, |c| c.max(criticality)),
                        );
                    }
                    controls.protected.extend(constraint.protected_paths);
                }
                Config::Scope(scope) => controls.scopes.push(scope.include),
                Config::Permission(permission) => {
                    if !permission.allow.contains(&ActionClass::Write) {
                        controls.read_only = true;
                    } else if !permission.allow.contains(&ActionClass::Delete) {
                        controls.gaps.push(config::problem("DELETE_NOT_SEPARATED", format!("{reference} allows write but not delete; the per-file limits cannot tell them apart, so deletes are not blocked by it")));
                    }
                    for class in [ActionClass::Execute, ActionClass::Network] {
                        if !permission.allow.contains(&class) {
                            controls.gaps.push(config::problem("CLASS_NOT_ENFORCED", format!("{reference} does not allow {class:?}; shell and network use is not limited per Flow or Zone, so this is delivered as guidance")));
                        }
                    }
                }
                Config::Approval(approval) => {
                    if approval
                        .operations
                        .iter()
                        .any(|class| matches!(class, ActionClass::Write | ActionClass::Delete))
                    {
                        controls.approval = true;
                    } else {
                        enforced = false;
                        controls.gaps.push(config::problem("APPROVAL_NOT_ENFORCED", format!("{reference} asks approval for execute or network, which the per-file limits cannot hold; delivered as guidance")));
                    }
                }
                Config::Isolation(isolation) => {
                    if isolation.profile != "agent-hooks" {
                        controls.isolation.push(isolation.profile);
                    } else {
                        enforced = false;
                    }
                }
                Config::Precondition(checks) => {
                    for check in &checks.checks {
                        if cheap_check(root, check) == Some(false) {
                            controls
                                .preconditions
                                .push(format!("{reference}: {}", check_text(check)));
                        }
                    }
                }
                _ => enforced = false,
            }
            if enforced {
                controls.sources.insert(reference);
            }
        }
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
        Check::SelectionsResolved => "every selection resolves".into(),
        Check::PolicyTestsPass => "every policy command passes".into(),
        Check::SignedState => "the signed state verifies".into(),
    }
}

/** The limit the controls put on one file of the target
 * Input
    - controls: &Controls - combined controls
    - file: &str - repository-relative file
    - scope_resolved: bool - every scope set matches at least one file of the target
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
        reasons.push(format!("a constraint caps the ceiling at {}", max.name()));
    }
    let mut lower = |to: Ceiling, reason: String| {
        ceiling = Some(ceiling.map_or(to, |current| current.min(to)));
        reasons.push(reason);
    };
    if controls.read_only {
        lower(
            Ceiling::Observe,
            "a permission Brick does not allow write".into(),
        );
    }
    if controls
        .protected
        .iter()
        .any(|pattern| glob::matches(pattern, file))
    {
        lower(Ceiling::Observe, format!("{file} is a protected path"));
    }
    if !controls.scopes.is_empty() {
        if !scope_resolved {
            lower(
                Ceiling::Observe,
                "a scope Brick matches no file of the target (unresolved scope never widens)"
                    .into(),
            );
        } else if !controls
            .scopes
            .iter()
            .all(|set| set.iter().any(|pattern| glob::matches(pattern, file)))
        {
            lower(
                Ceiling::Observe,
                format!("{file} is outside the Brick scope"),
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
    if controls.approval {
        lower(
            Ceiling::Assisted,
            "an approval Brick asks for a person".into(),
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
                "pins Crane cannot honor: {}",
                controls.fail_closed.join("; ")
            ),
        );
    }
    (ceiling, controls.criticality, reasons)
}

/** Report whether every scope set of the controls matches at least one of the target's files
 * Input
    - controls: &Controls - controls
    - files: &BTreeSet<String> - the target's files
 * Output
    - bool
*/
pub(crate) fn scope_resolved(controls: &Controls, files: &BTreeSet<String>) -> bool {
    controls.scopes.iter().all(|set| {
        files
            .iter()
            .any(|file| set.iter().any(|pattern| glob::matches(pattern, file)))
    })
}
