// The permission check: may this typed operation act on this resource, for this actor and skill?
// It never executes anything. In order, each rule can only make the answer stricter:
//
//   1. the operation must be registered (an unknown id is denied, never interpreted)
//   2. its adapter must be available
//   3. with a skill: the skill must be published and the operation must implement one of its steps
//   4. the resource must be in the namespace the adapter declares; repository resources are then
//      decided by the scope engine (the hooks' guard): reads for read-only operations, writes for
//      the rest, so a skill can never do in the repository what an agent tool could not; a resource
//      outside the repository, or in a namespace no adapter declares, is denied
//   5. an agent may only use read-only operations (the invocation engine's rule)
//   6. an operation that changes state needs a person's approval (the invocation engine's rule)

use serde::Serialize;

use super::adapter::AdapterRegistry;
use super::lifecycle::EffectClass;
use crate::capability::lifecycle::LifecycleState;
use crate::capability::registry::VersionRecord;
use crate::capture::{Actor, ActorKind};
use crate::scope::resource::Location;
use crate::scope::{Access, Boundary, Request};
use crate::telemetry::model::Decision;

/** The answer of a permission check
 * Fields
    - decision: Decision - ALLOW, REQUIRE_APPROVAL, DENY, or ERROR
    - reasons: Vec<(String, String)> - reason codes and explanations
    - effect: Option<EffectClass> - the operation's effect class, when registered
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Permission {
    pub(crate) decision: Decision,
    pub(crate) reasons: Vec<(String, String)>,
    pub(crate) effect: Option<EffectClass>,
}

impl Permission {
    /** Make the answer stricter with a reason
     * Input
        - decision: Decision - at least this strict
        - code: &str - reason code
        - message: String - explanation
     * Output
        - None
    */
    fn tighten(&mut self, decision: Decision, code: &str, message: String) {
        let rank = |decision: Decision| match decision {
            Decision::Allow => 0,
            Decision::RequireApproval => 1,
            Decision::Error => 2,
            Decision::Deny | Decision::Quarantine => 3,
        };
        if rank(decision) > rank(self.decision) {
            self.decision = decision;
        }
        self.reasons.push((code.into(), message));
    }
}

/** Split a resource reference into its namespace and identifier ("repo:app/x.py"; a bare path is
 * a repository path)
 * Input
    - resource: &str - reference
 * Output
    - (String, String)
*/
pub(crate) fn namespace(resource: &str) -> (String, String) {
    match resource.split_once(':') {
        Some((space, rest))
            if !space.is_empty()
                && space
                    .chars()
                    .all(|character| character.is_ascii_lowercase())
                && !rest.starts_with("//")
                && space.len() > 1 =>
        {
            let space = if space == "file" { "repo" } else { space };
            (space.to_string(), rest.to_string())
        }
        _ => ("repo".into(), resource.into()),
    }
}

/** Check a typed operation on a resource
 * Input
    - adapters: &AdapterRegistry - registered adapters
    - boundary: &Boundary - the repository's scope engine
    - actor: &Actor - who asks
    - operation: &str - "ADAPTER/OPERATION"
    - resource: &str - resource reference
    - skill: Option<&VersionRecord> - the skill it would run for
 * Output
    - Permission
*/
pub(crate) fn check(
    adapters: &AdapterRegistry,
    boundary: &Boundary,
    actor: &Actor,
    operation: &str,
    resource: &str,
    skill: Option<&VersionRecord>,
) -> Permission {
    let mut answer = Permission {
        decision: Decision::Allow,
        reasons: Vec::new(),
        effect: None,
    };
    let Some((adapter_id, operation_id)) = operation.split_once('/') else {
        answer.tighten(
            Decision::Deny,
            "OPERATION_MALFORMED",
            format!("'{operation}' must be ADAPTER/OPERATION"),
        );
        return answer;
    };
    let Some(spec) = adapters.operation(adapter_id, operation_id) else {
        answer.tighten(
            Decision::Deny,
            "UNREGISTERED_OPERATION",
            format!(
                "{operation} is not a registered operation; nothing else is ever run in its place"
            ),
        );
        return answer;
    };
    answer.effect = Some(spec.effect);
    let adapter = adapters.adapter(adapter_id);
    if !adapter.is_some_and(|adapter| adapter.healthy()) {
        answer.tighten(
            Decision::Error,
            "ADAPTER_UNAVAILABLE",
            format!("adapter {adapter_id} is not available"),
        );
    }
    if let Some(record) = skill {
        if record.state != LifecycleState::Published {
            answer.tighten(
                Decision::Deny,
                "SKILL_NOT_PUBLISHED",
                format!(
                    "{}@{} is {:?}; only a published skill acts",
                    record.definition.capability_id, record.definition.version, record.state
                ),
            );
        }
        let kinds = record
            .definition
            .execution
            .steps
            .iter()
            .map(|step| step.kind)
            .collect::<Vec<_>>();
        if !spec.step_kinds.iter().any(|kind| kinds.contains(kind)) {
            answer.tighten(
                Decision::Deny,
                "OPERATION_NOT_IN_SKILL",
                format!("{operation} implements none of the skill's steps"),
            );
        }
    }
    let (space, identifier) = namespace(resource);
    match adapter.and_then(|adapter| adapter.resource_namespace()) {
        Some(declared) if declared == space => {
            if space == "repo" {
                let access = if spec.effect == EffectClass::ReadOnly { Access::Read } else { Access::Write };
                match boundary.decide(&Request { access, target: identifier.clone(), lines: None, mode: "delegated".into() }) {
                    Ok(outcome) => {
                        if !matches!(outcome.resource.location, Location::Repository | Location::CraneMetadata | Location::HookSettings | Location::TrustDirectory) {
                            answer.tighten(Decision::Deny, "RESOURCE_OUT_OF_SCOPE", format!("{identifier} is not in the repository"));
                        }
                        let decision = outcome.decided.verdict.decision;
                        if decision != Decision::Allow {
                            for (code, reason) in outcome.decided.rules.iter().zip(outcome.decided.verdict.reasons.iter()) {
                                answer.tighten(decision, code, reason.clone());
                            }
                        }
                    }
                    Err(error) => answer.tighten(Decision::Deny, "RESOURCE_UNRESOLVED", error),
                }
            }
        }
        Some(declared) => answer.tighten(Decision::Deny, "RESOURCE_NAMESPACE_MISMATCH", format!("{adapter_id} acts on {declared} resources, not {space}")),
        None => answer.tighten(Decision::Deny, "UNSUPPORTED_RESOURCE", format!("adapter {adapter_id} declares no resource namespace, so {resource} cannot be resolved")),
    }
    if actor.kind == ActorKind::Agent && spec.effect != EffectClass::ReadOnly {
        answer.tighten(
            Decision::Deny,
            "AGENT_READ_ONLY",
            "an agent may only use read-only operations".into(),
        );
    }
    if spec.effect != EffectClass::ReadOnly {
        answer.tighten(
            Decision::RequireApproval,
            "OPERATION_CHANGES_STATE",
            format!(
                "{operation} is {:?}; a person approves its exact plan",
                spec.effect
            ),
        );
    }
    answer
}

#[cfg(test)]
mod tests {
    use super::namespace;

    /** Resource references split into namespace and identifier; bare paths and file: are the repo
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn resources_have_namespaces() {
        assert_eq!(namespace("crm:acct-17"), ("crm".into(), "acct-17".into()));
        assert_eq!(
            namespace("repo:app/x.py"),
            ("repo".into(), "app/x.py".into())
        );
        assert_eq!(
            namespace("file:app/x.py"),
            ("repo".into(), "app/x.py".into())
        );
        assert_eq!(namespace("app/x.py"), ("repo".into(), "app/x.py".into()));
        assert_eq!(
            namespace("C:/repo/x.py"),
            ("repo".into(), "C:/repo/x.py".into())
        );
    }
}
