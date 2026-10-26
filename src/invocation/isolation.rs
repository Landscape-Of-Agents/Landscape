// Isolation profiles: what each execution path really confines, stated per control (files,
// processes, network, credentials) as enforced, partial, or not enforced. Nothing here sandboxes
// anything; it describes the boundaries that exist so that a skill needing a stronger one is
// blocked instead of being told it is isolated.
//
//   agent-hooks     - an agent tool's own actions, decided before and verified after by the hooks
//   typed-adapters  - skill operations through registered adapters: no shell operation exists,
//                     parameters are typed and bounded, secrets are refused as inputs
//   sandbox         - a confined process environment; not available in this version

use serde::Serialize;

use super::adapter::AdapterRegistry;
use crate::capability::model::CapabilityDefinition;

/** How well a control is enforced
 * Variants
    - Enforced - by the execution path itself
    - Partial - for some cases; the rest is reported as a gap
    - NotEnforced - not at all
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Control {
    Enforced,
    Partial,
    NotEnforced,
}

/** One isolation profile
 * Fields
    - id: &'static str - profile id
    - available: bool - this version provides it
    - files, processes, network, credentials: (Control, &'static str) - each control and why
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Profile {
    pub(crate) id: &'static str,
    pub(crate) available: bool,
    pub(crate) files: (Control, &'static str),
    pub(crate) processes: (Control, &'static str),
    pub(crate) network: (Control, &'static str),
    pub(crate) credentials: (Control, &'static str),
}

/** Every profile
 * Input
    - None
 * Output
    - Vec<Profile>
*/
pub(crate) fn profiles() -> Vec<Profile> {
    vec![
        Profile {
            id: "agent-hooks",
            available: true,
            files: (Control::Partial, "repository files are decided before and verified after each tool call; files outside the repository are not governed"),
            processes: (Control::NotEnforced, "shell commands and scripts are checked lexically before they run; their real effects are judged afterwards"),
            network: (Control::NotEnforced, "destinations are parsed from commands, never observed or blocked"),
            credentials: (Control::Partial, "the trust directory is denied by name; a same-user shell can still read it"),
        },
        Profile {
            id: "typed-adapters",
            available: true,
            files: (Control::Enforced, "a skill touches files only through registered operations with typed, bounded parameters"),
            processes: (Control::Enforced, "no operation runs a command, a script, or model output"),
            network: (Control::Partial, "only adapters reach external systems, each to its own system; the operating system does not restrict them"),
            credentials: (Control::Enforced, "inputs holding secrets are refused; only credential references are passed"),
        },
        Profile {
            id: "sandbox",
            available: false,
            files: (Control::NotEnforced, "not provided by this version"),
            processes: (Control::NotEnforced, "not provided by this version"),
            network: (Control::NotEnforced, "not provided by this version"),
            credentials: (Control::NotEnforced, "not provided by this version"),
        },
    ]
}

/** Find a profile
 * Input
    - id: &str - profile id
 * Output
    - Option<Profile>
*/
pub(crate) fn profile(id: &str) -> Option<Profile> {
    profiles().into_iter().find(|profile| profile.id == id)
}

/** Check that a skill can run under a profile: the profile exists and is available, and every step
 * kind has a registered operation that implements it (a step nothing can execute is never run some
 * other way)
 * Input
    - definition: &CapabilityDefinition - skill
    - adapters: &AdapterRegistry - registered adapters
    - profile_id: &str - required profile
 * Output
    - Vec<String> problems; empty when it can run under the profile
*/
pub(crate) fn check(
    definition: &CapabilityDefinition,
    adapters: &AdapterRegistry,
    profile_id: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    match profile(profile_id) {
        None => problems.push(format!("unknown isolation profile '{profile_id}'")),
        Some(profile) if !profile.available => problems.push(format!(
            "isolation profile {profile_id} is not available in this version"
        )),
        Some(_) => {}
    }
    let operations = adapters.operations();
    for step in &definition.execution.steps {
        if step.kind == crate::capability::model::StepKind::Approval {
            continue;
        }
        if !operations
            .iter()
            .any(|operation| operation.step_kinds.contains(&step.kind))
        {
            problems.push(format!(
                "step {} ({:?}) has no registered operation that can execute it",
                step.step_id, step.kind
            ));
        }
    }
    problems
}
