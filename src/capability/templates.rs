// Skill templates: reviewed starting points for 'crane skill create' (and the future dashboard
// form). A template fills a capability definition with safe defaults: typed steps, restrictive risk
// and review levels, required verification claims marked only declared, and no evidence. The draft
// is stored as a candidate, which grants nothing: validation, the lab, review, and publication by
// another person still decide whether it can ever run. Templates are department-neutral; the
// department is recorded as the business domain.

use serde_json::{json, Value};

use super::model::{
    parse_definition, CapabilityDefinition, CAPABILITY_SCHEMA_VERSION, PROCEDURE_VERSION,
};
use crate::capture::Actor;

/** A template
 * Fields
    - id: &'static str - template id
    - summary: &'static str - what it is for
*/
pub(crate) struct Template {
    pub(crate) id: &'static str,
    pub(crate) summary: &'static str,
}

/** The available templates */
pub(crate) const TEMPLATES: &[Template] = &[
    Template {
        id: "read-only-lookup",
        summary: "look something up and report it; changes nothing (low risk, peer review)",
    },
    Template {
        id: "repository-change",
        summary: "change code, run the tests, and verify the result (medium risk, owner review)",
    },
    Template {
        id: "business-record-update",
        summary: "update a record in a business system after a person approves (high risk, owner review; needs a registered adapter for that system)",
    },
];

/** A step description
 * Input
    - id: &str - step id
    - kind: &str - step kind
    - description: &str - what it does
    - verifies: &[&str] - claims it checks
 * Output
    - Value
*/
fn step(id: &str, kind: &str, description: &str, verifies: &[&str]) -> Value {
    json!({"step_id": id, "kind": kind, "description": description, "inputs": ["subject"], "outputs": [], "preconditions": [], "verifies": verifies})
}

/** A required claim, only declared
 * Input
    - id: &str - claim id
    - kind: &str - claim kind
    - statement: &str - the claim
    - expected: &str - what verifies it
 * Output
    - Value
*/
fn claim(id: &str, kind: &str, statement: &str, expected: &str) -> Value {
    json!({"claim_id": id, "kind": kind, "statement": statement, "expected_result": expected, "required": true, "status": "declared", "evidence_refs": [], "assertion_refs": []})
}

/** Build a draft definition from a template
 * Input
    - template: &str - template id
    - capability_id: &str - new skill id
    - name: &str - name
    - department: Option<&str> - owning department
    - author: &Actor - author
    - now: u64 - Unix milliseconds
 * Output
    - Result<CapabilityDefinition, String>
    - Error for an unknown template or an invalid id
*/
pub(crate) fn draft(
    template: &str,
    capability_id: &str,
    name: &str,
    department: Option<&str>,
    author: &Actor,
    now: u64,
) -> Result<CapabilityDefinition, String> {
    crate::platform::validate_name("skill", capability_id)?;
    let (purpose, task_class, risk, review, steps, claims, side_effects, invariants) =
        match template {
            "read-only-lookup" => (
                "Look up a record and report what it says",
                "lookup",
                "low",
                "peer",
                vec![step(
                    "read",
                    "query",
                    "Read the record named by the subject",
                    &["record-read"],
                )],
                vec![claim(
                    "record-read",
                    "technical",
                    "The record was read from its system of record",
                    "The record's content",
                )],
                json!([]),
                json!([]),
            ),
            "repository-change" => (
                "Change code in the repository and prove the change works",
                "code-change",
                "medium",
                "owner",
                vec![
                    step("inspect", "query", "Read the code the subject names", &[]),
                    step("change", "change", "Apply the reviewed change", &[]),
                    step(
                        "test",
                        "test",
                        "Run the tests that cover the change",
                        &["tests-pass"],
                    ),
                    step(
                        "verify",
                        "verify",
                        "Confirm the change and the tests through independent evidence",
                        &["change-present"],
                    ),
                ],
                vec![
                    claim(
                        "tests-pass",
                        "technical",
                        "The tests covering the change pass",
                        "A passing test report for the commit",
                    ),
                    claim(
                        "change-present",
                        "acceptance",
                        "The intended change is in the repository",
                        "The commit contains the change",
                    ),
                ],
                json!([{"system": "repository", "description": "a commit", "reversible": true, "risk": "medium"}]),
                json!([]),
            ),
            "business-record-update" => (
                "Update a record in a business system after a person approves",
                "record-update",
                "high",
                "owner",
                vec![
                    step("read", "query", "Read the current record", &[]),
                    step(
                        "approve",
                        "approval",
                        "A person approves the exact update",
                        &[],
                    ),
                    step(
                        "update",
                        "change",
                        "Update the record through the system's registered adapter",
                        &[],
                    ),
                    step(
                        "confirm",
                        "verify",
                        "Read the record back from the system of record",
                        &["record-updated"],
                    ),
                ],
                vec![claim(
                    "record-updated",
                    "acceptance",
                    "The system of record shows the update",
                    "The record read back after the update",
                )],
                json!([{"system": "business system", "description": "an updated record", "reversible": null, "risk": "high"}]),
                json!([{"invariant_id": "policy-respected", "statement": "The update follows the department's policy", "authority": null, "mapping": "unmapped", "required": true}]),
            ),
            other => {
                return Err(format!(
                    "unknown template '{other}'; templates:\n{}",
                    TEMPLATES
                        .iter()
                        .map(|template| format!("  {}: {}", template.id, template.summary))
                        .collect::<Vec<_>>()
                        .join("\n")
                ))
            }
        };
    let checkpoints = claims
        .iter()
        .filter_map(|claim| claim["claim_id"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
    let subject = json!([{"name": "subject", "kind": "identifier", "required": true, "description": "What the skill works on"}]);
    let definition = json!({
        "capability_id": capability_id, "version": 1, "schema_version": CAPABILITY_SCHEMA_VERSION,
        "name": name, "description": purpose, "owner_ref": department.map(|department| format!("team:{department}")), "previous_version_ref": null,
        "provenance": {"creation_method": "manual", "created_by": author, "created_at": now, "source_intake_refs": [], "source_episode_refs": [], "source_evidence_pack_refs": [], "change_reason": format!("created from template {template}")},
        "applicability": {"purpose": purpose, "task_classes": [task_class], "environments": [], "business_domain": department,
            "preconditions": [], "required_state": [], "exclusions": [], "limitations": ["drafted from a template; complete it before validation"],
            "inputs": subject, "outputs": [], "assumptions": [], "risk": risk, "review_level": review},
        "dependencies": [],
        "execution": {"procedure_version": PROCEDURE_VERSION, "inputs": subject, "preconditions": [], "steps": steps, "effects": [], "state_transitions": [],
            "postconditions": checkpoints, "outputs": [], "failure_conditions": ["any required claim is not verified"], "checkpoints": checkpoints,
            "recovery": ["reconcile uncertain effects before any retry"], "side_effects": side_effects},
        "verification": {"method": "independent-evidence", "method_version": "1", "scope_environments": [], "claims": claims, "invariants": invariants,
            "negative_cases": [], "limitations": [], "unresolved_assumptions": []},
        "known_failure_cases": [], "proposed_improvements": []
    });
    parse_definition(&definition).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::{draft, TEMPLATES};
    use crate::capture::{Actor, ActorKind};

    /** Every template drafts a parseable definition with only declared claims and no evidence;
     * unknown templates and invalid ids are refused
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn templates_draft_definitions_that_grant_nothing() {
        let author = Actor::new("Dev", ActorKind::Human);
        for template in TEMPLATES {
            let definition = draft(
                template.id,
                "my-skill",
                "My skill",
                Some("sales"),
                &author,
                1,
            )
            .unwrap();
            assert_eq!(
                definition.applicability.business_domain.as_deref(),
                Some("sales")
            );
            assert!(definition
                .verification
                .claims
                .iter()
                .all(|claim| claim.evidence_refs.is_empty()));
            assert!(!definition.provenance.evidence_backed());
        }
        assert!(draft("shell-script", "x", "x", None, &author, 1).is_err());
        assert!(draft("read-only-lookup", "1bad", "x", None, &author, 1).is_err());
    }
}
