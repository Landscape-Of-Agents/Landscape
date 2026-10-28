// The one validation of Bricks and Stacks. The CLI, activation, the extension (through the CLI),
// and the dashboard (through the reported status) all see these results; nothing re-implements
// them. Validation is deterministic and never repairs anything.

use std::path::Path;
use std::rc::Rc;

use super::config::{self, problem, Problem};
use super::model::{Brick, BrickRef, Stack, Validation, ValidationStatus, VersionState};
use super::store::BrickStore;
use crate::capture::resources::local::GitConnector;
use crate::invocation::adapter::{AdapterRegistry, GitReadAdapter};

/** Largest Common or Current Context, in bytes */
pub(crate) const MAX_CONTEXT_BYTES: usize = 16 * 1024;

/** The registered operations of a repository, as (adapter, operation) pairs: the same registry
 * 'crane skill invoke' uses
 * Input
    - root: &Path - repository root
 * Output
    - Vec<(String, String)>
*/
pub(crate) fn registered_operations(root: &Path) -> Vec<(String, String)> {
    AdapterRegistry::default()
        .with(Rc::new(GitReadAdapter {
            connector: GitConnector {
                root: root.to_path_buf(),
            },
        }))
        .operations()
        .into_iter()
        .map(|spec| (spec.adapter_id, spec.operation_id))
        // Resident agents' steps are operations Crane runs for a target too
        .chain(
            crate::resident::plan::StepType::ALL
                .iter()
                .map(|step| ("resident".to_string(), step.name())),
        )
        .collect()
}

/** The isolation profiles and whether each is available
 * Input
    - None
 * Output
    - Vec<(String, bool)>
*/
pub(crate) fn isolation_profiles() -> Vec<(String, bool)> {
    crate::invocation::isolation::profiles()
        .into_iter()
        .map(|profile| (profile.id.to_string(), profile.available))
        .collect()
}

/** Check a context text: size and secrets (a context that holds a secret is refused, never
 * silently redacted, so nobody believes it was delivered as written)
 * Input
    - field: &str - which context
    - text: &str - text
 * Output
    - Vec<Problem>
*/
pub(crate) fn context_problems(field: &str, text: &str) -> Vec<Problem> {
    let mut problems = Vec::new();
    if text.len() > MAX_CONTEXT_BYTES {
        problems.push(problem(
            "CONTEXT_TOO_LARGE",
            format!(
                "{field} is {} bytes; at most {MAX_CONTEXT_BYTES}",
                text.len()
            ),
        ));
    }
    let (_, secrets) = crate::telemetry::redact::redact(text);
    if secrets > 0 {
        problems.push(problem(
            "CONTEXT_HOLDS_SECRET",
            format!("{field} looks like it holds {secrets} secret(s); context is delivered to agents and must never carry credentials"),
        ));
    }
    problems
}

/** Validate a Brick definition against its schema, the registry of operations, the isolation
 * profiles, and the store (dependencies)
 * Input
    - brick: &Brick - definition
    - store: &BrickStore - signed store
    - root: &Path - repository root
 * Output
    - Validation (status passed, failed, or blocked; problems; the digest validated)
*/
pub(crate) fn brick(brick: &Brick, store: &BrickStore, root: &Path) -> Validation {
    let mut problems = Vec::new();
    if let Err(error) = crate::platform::validate_name("Brick", &brick.brick_id) {
        problems.push(problem("ID_INVALID", error));
    }
    if brick.version == 0 {
        problems.push(problem("VERSION_INVALID", "versions start at 1"));
    }
    if brick.name.trim().is_empty() {
        problems.push(problem("NAME_MISSING", "a Brick needs a name"));
    }
    if brick.intent.trim().is_empty() {
        problems.push(problem(
            "INTENT_MISSING",
            "state the outcome it solves in plain words (intent)",
        ));
    }
    if brick.brick_type.is_legacy() {
        problems.push(problem(
            "TYPE_REPLACED",
            format!(
                "{} is a former kind; 'crane brick migrate' rewrites it as {}",
                brick.brick_type.name(),
                brick
                    .brick_type
                    .successors()
                    .iter()
                    .map(|kind| kind.name())
                    .collect::<Vec<_>>()
                    .join(" and ")
            ),
        ));
    }
    let mut blocked = false;
    match config::parse(brick.brick_type, &brick.config) {
        Err(error) => problems.push(error),
        Ok(parsed) => {
            let operations = registered_operations(root);
            let profiles = isolation_profiles();
            for config in &parsed {
                let found = config::validate(config, &operations, &profiles);
                blocked |= found
                    .iter()
                    .any(|item| item.code == "ISOLATION_UNAVAILABLE");
                problems.extend(found);
                if let config::Config::Knowledge(knowledge) = config {
                    let empty = knowledge.files.is_empty()
                        && knowledge.glossary.is_empty()
                        && knowledge.references.is_empty();
                    if empty && brick.common_context.text.trim().is_empty() {
                        problems.push(problem(
                            "CONTEXT_EMPTY",
                            "a knowledge Brick needs common context text, files, glossary terms, or references",
                        ));
                    }
                }
            }
        }
    }
    problems.extend(context_problems(
        "common_context",
        &brick.common_context.text,
    ));
    for dependency in &brick.dependencies {
        problems.extend(dependency_problem(dependency, store, &brick.brick_id));
    }
    let status = if problems.is_empty() {
        ValidationStatus::Passed
    } else if blocked && problems.len() == 1 {
        ValidationStatus::Blocked
    } else {
        ValidationStatus::Failed
    };
    Validation {
        status,
        at: crate::platform::now_millis(),
        problems,
        digest: brick.digest(),
    }
}

/** Check one dependency or Stack member: activated, active, not the Brick itself
 * Input
    - reference: &BrickRef - exact version
    - store: &BrickStore - signed store
    - own_id: &str - the depending Brick or Stack id
 * Output
    - Option<Problem>
*/
fn dependency_problem(reference: &BrickRef, store: &BrickStore, own_id: &str) -> Option<Problem> {
    if reference.brick_id == own_id {
        return Some(problem(
            "DEPENDENCY_CYCLE",
            format!("{reference} refers to itself"),
        ));
    }
    match store.brick(reference) {
        None => Some(problem(
            "DEPENDENCY_UNRESOLVED",
            format!("{reference} is not an activated Brick version"),
        )),
        Some(entry) if entry.state == VersionState::Revoked => Some(problem(
            "DEPENDENCY_REVOKED",
            format!("{reference} is revoked"),
        )),
        Some(entry) if entry.state == VersionState::Deprecated => Some(problem(
            "DEPENDENCY_DEPRECATED",
            format!("{reference} is deprecated; pin a newer version"),
        )),
        Some(_) => None,
    }
}

/** Validate a Stack: exact, active, distinct members (one per Brick id), their dependencies
 * present in the Stack, and at most one isolation profile
 * Input
    - stack: &Stack - definition
    - store: &BrickStore - signed store
 * Output
    - Validation
*/
pub(crate) fn stack(stack: &Stack, store: &BrickStore) -> Validation {
    let mut problems = Vec::new();
    if let Err(error) = crate::platform::validate_name("Stack", &stack.stack_id) {
        problems.push(problem("ID_INVALID", error));
    }
    if stack.name.trim().is_empty() {
        problems.push(problem("NAME_MISSING", "a Stack needs a name"));
    }
    if stack.members.is_empty() {
        problems.push(problem("STACK_EMPTY", "a Stack needs at least one member"));
    }
    let mut seen = std::collections::BTreeSet::new();
    for member in &stack.members {
        if !seen.insert(member.brick_id.clone()) {
            problems.push(problem(
                "STACK_DUPLICATE_MEMBER",
                format!(
                    "{} appears more than once; pin one version",
                    member.brick_id
                ),
            ));
        }
        problems.extend(dependency_problem(member, store, &stack.stack_id));
    }
    for member in &stack.members {
        if let Some(entry) = store.brick(member) {
            for dependency in &entry.brick.dependencies {
                if !stack.members.contains(dependency) {
                    problems.push(problem(
                        "STACK_DEPENDENCY_MISSING",
                        format!("{member} needs {dependency}, which the Stack does not include"),
                    ));
                }
            }
        }
    }
    let profiles = stack
        .members
        .iter()
        .filter_map(|member| store.brick(member))
        .filter_map(|entry| config::parse(entry.brick.brick_type, &entry.brick.config).ok())
        .flatten()
        .filter_map(|config| match config {
            config::Config::Environment(environment) => Some(environment.profile),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    if profiles.len() > 1 {
        problems.push(problem(
            "STACK_ISOLATION_CONFLICT",
            format!(
                "members require different isolation profiles ({}); keep one",
                profiles.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }
    Validation {
        status: if problems.is_empty() {
            ValidationStatus::Passed
        } else {
            ValidationStatus::Failed
        },
        at: crate::platform::now_millis(),
        problems,
        digest: stack.digest(),
    }
}
