// The skill-oriented forms of the scope commands (the command catalog's syntax), next to the
// repository forms the other files keep unchanged: a skill's effective scope (the zones and flows it
// depends on, resolved to files and limits; an unresolved one is never treated as allowed), a
// resource reference resolved to one identity, a live explanation of one operation on one resource,
// the scope difference between two skill versions, and the invocation record of a skill or run.
// Every decision comes from the scope engine and the permission service.

use std::collections::BTreeSet;

use serde_json::json;

use crate::capability::model::{CapabilityDefinition, DependencyKind};
use crate::cli::args::Args;
use crate::cli::family::{reference, skill};
use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, versioned, Response, Status};
use crate::invocation::permission::{check, namespace};
use crate::scope::{rule, Access, Boundary, Request};
use crate::telemetry::events::EventStore;

/** The zone and flow dependencies of a skill as "zone:ID" and "flow:ID"
 * Input
    - definition: &CapabilityDefinition - skill
 * Output
    - BTreeSet<String>
*/
fn entities(definition: &CapabilityDefinition) -> BTreeSet<String> {
    definition
        .dependencies
        .iter()
        .filter_map(|dependency| match dependency.kind {
            DependencyKind::Zone => Some(format!("zone:{}", dependency.target_id)),
            DependencyKind::Flow => Some(format!("flow:{}", dependency.target_id)),
            _ => None,
        })
        .collect()
}

/** The zone and flow dependencies of a skill that cover nothing now
 * Input
    - boundary: &Boundary - the boundary
    - reference_text: &str - skill reference
 * Output
    - Result<Vec<String>, String>
*/
pub(super) fn unresolved(boundary: &Boundary, reference_text: &str) -> Result<Vec<String>, String> {
    let local = Local::open()?;
    let record = skill(&local, reference_text)?;
    Ok(entities(&record.definition)
        .into_iter()
        .filter(|entity| {
            !boundary.enforcement.limits.values().any(|limit| {
                limit.entities.contains(entity)
                    || limit
                        .scopes
                        .iter()
                        .any(|block| block.entities.contains(entity))
            })
        })
        .collect())
}

/** crane scope inspect --skill: the effective scope of a skill
 * Input
    - args: &Args - parsed arguments
    - reference_text: &str - skill reference
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &Args, reference_text: &str) -> Result<(), String> {
    let local = Local::open()?;
    let record = skill(&local, reference_text)?;
    let boundary = Boundary::load(local.context.workspace.clone())?;
    let mut rows = Vec::new();
    let mut unresolved = 0;
    for entity in entities(&record.definition) {
        let files = boundary
            .enforcement
            .limits
            .iter()
            .filter(|(_, limit)| limit.entities.contains(&entity) || limit.scopes.iter().any(|block| block.entities.contains(&entity)))
            .map(|(path, limit)| json!({"path": path, "ceiling": limit.ceiling, "criticality": limit.criticality}))
            .collect::<Vec<_>>();
        if files.is_empty() {
            unresolved += 1;
        }
        rows.push(json!({"entity": entity, "files": files, "resolved": !files.is_empty()}));
    }
    let detail = rows
        .iter()
        .map(|row| {
            let files = row["files"].as_array().map(Vec::len).unwrap_or(0);
            format!("  {}: {}", row["entity"].as_str().unwrap_or_default(), if files == 0 { "UNRESOLVED (no active zone or flow covers anything under it; never treated as allowed)".to_string() } else { format!("{files} governed files") })
        })
        .chain(std::iter::once("  every other resource: the repository boundary ('crane scope inspect'), and only through the skill's registered operations".to_string()))
        .collect::<Vec<_>>();
    let response = Response::new(
        "scope.inspect",
        if unresolved > 0 {
            Status::Unresolved
        } else {
            Status::Ok
        },
        format!(
            "{}: {} zone and flow dependencies, {unresolved} unresolved",
            reference(record),
            rows.len()
        ),
    )
    .reference("skill_ref", reference(record))
    .reference(
        "scope_digest",
        boundary.versions().manifest_head.unwrap_or_default(),
    )
    .data(json!({"dependencies": rows}));
    finish(args, response, &detail)
}

/** crane scope resolve --resource: one canonical identity for a resource reference
 * Input
    - args: &Args - parsed arguments
    - resource: &str - resource reference
 * Output
    - Result<(), String>
*/
pub(super) fn resolve(args: &Args, resource: &str) -> Result<(), String> {
    let (space, identifier) = namespace(resource);
    if space != "repo" {
        let local = Local::open()?;
        let declared = local.adapters.operations().iter().any(|operation| {
            local
                .adapters
                .adapter(&operation.adapter_id)
                .and_then(|adapter| adapter.resource_namespace())
                == Some(space.as_str())
        });
        let response = Response::new(
            "scope.resolve",
            Status::Unresolved,
            format!(
                "{resource}: {}",
                if declared {
                    "resolved by its adapter at execution time, never by text"
                } else {
                    "no registered adapter resolves this namespace"
                }
            ),
        )
        .reasons(if declared {
            Vec::new()
        } else {
            vec!["UNSUPPORTED_RESOURCE".to_string()]
        })
        .data(json!({"namespace": space, "identifier": identifier}));
        return finish(args, response, &[]);
    }
    let boundary = Boundary::load(crate::governance::workspace::Workspace::locate()?)?;
    let outcome = boundary.decide(&Request {
        access: Access::Read,
        target: identifier.clone(),
        lines: None,
        mode: "delegated".into(),
    })?;
    let resolved = outcome.resource.path.is_some() || outcome.resource.absolute.is_some();
    let response = Response::new(
        "scope.resolve",
        if resolved {
            Status::Ok
        } else {
            Status::Unresolved
        },
        format!(
            "{resource}: {:?}, {:?}{}",
            outcome.resource.location,
            outcome.resource.tracking,
            if outcome.resource.via_symlink {
                ", through a symlink"
            } else {
                ""
            }
        )
        .to_lowercase(),
    )
    .data(json!({"resource": outcome.resource, "gaps": outcome.gaps}));
    finish(args, response, &[])
}

/** crane scope explain --operation --resource: why an operation may or may not act on a resource,
 * with the strictest rule first
 * Input
    - args: &Args - parsed arguments
    - operation: &str - ADAPTER/OPERATION
    - resource: &str - resource reference
 * Output
    - Result<(), String>
*/
pub(super) fn explain(args: &Args, operation: &str, resource: &str) -> Result<(), String> {
    let local = Local::open()?;
    let record = args
        .value("skill")
        .map(|text| skill(&local, &text))
        .transpose()?;
    let boundary = Boundary::load(local.context.workspace.clone())?;
    let answer = check(
        &local.adapters,
        &boundary,
        &local.context.actor,
        operation,
        resource,
        record,
    );
    let codes = answer
        .reasons
        .iter()
        .map(|(code, _)| code.clone())
        .collect::<Vec<_>>();
    let messages = answer
        .reasons
        .iter()
        .map(|(_, message)| message.clone())
        .collect::<Vec<_>>();
    let detail = answer
        .reasons
        .iter()
        .map(|(code, message)| format!("  [{}] {code}: {message}", rule(code).0))
        .collect::<Vec<_>>();
    let response = Response::new(
        "scope.explain",
        Status::Ok,
        format!(
            "{operation} on {resource}{}",
            codes
                .first()
                .map(|code| format!("; deciding rule {code}"))
                .unwrap_or_default()
        ),
    )
    .decision(answer.decision)
    .reasons(codes)
    .data(json!({"reasons": messages, "effect": answer.effect}));
    finish(args, response, &detail)
}

/** crane scope diff --skill A --against B|current: the scope two skill versions ask for, gained and
 * lost; any gain is classified as an expansion, which never activates itself
 * Input
    - args: &Args - parsed arguments
    - reference_text: &str - the newer skill version
    - against: &str - the older version, or "current" (the latest published)
 * Output
    - Result<(), String>
*/
pub(super) fn diff(args: &Args, reference_text: &str, against: &str) -> Result<(), String> {
    let local = Local::open()?;
    let newer = skill(&local, reference_text)?;
    let older = if against == "current" {
        let (id, _) = versioned(reference_text)?;
        skill(&local, &id)?
    } else {
        skill(&local, against)?
    };
    let shape = |definition: &CapabilityDefinition| {
        let mut items = entities(definition);
        items.extend(
            definition
                .execution
                .steps
                .iter()
                .filter(|step| step.kind.mutating())
                .map(|step| format!("changes:{:?}", step.kind).to_lowercase()),
        );
        items.extend(
            definition
                .execution
                .side_effects
                .iter()
                .map(|effect| format!("side-effect:{}", effect.system)),
        );
        items.extend(
            definition
                .applicability
                .environments
                .iter()
                .map(|environment| format!("environment:{environment}")),
        );
        items
    };
    let (before, after) = (shape(&older.definition), shape(&newer.definition));
    let gained = after.difference(&before).cloned().collect::<Vec<_>>();
    let lost = before.difference(&after).cloned().collect::<Vec<_>>();
    let riskier = newer.definition.applicability.risk > older.definition.applicability.risk;
    let expanded = !gained.is_empty() || riskier;
    let mut detail = gained
        .iter()
        .map(|item| format!("  GAINED {item}"))
        .collect::<Vec<_>>();
    detail.extend(lost.iter().map(|item| format!("  lost   {item}")));
    if riskier {
        detail.push(format!(
            "  RISK   {:?} -> {:?}",
            older.definition.applicability.risk, newer.definition.applicability.risk
        ));
    }
    let response = Response::new(
        "scope.diff",
        if expanded { Status::Failed } else { Status::Ok },
        format!(
            "{} against {}: {}",
            reference(newer),
            reference(older),
            if expanded {
                "the scope expands; it needs review and activation by another person"
            } else {
                "no expansion"
            }
        ),
    )
    .reasons(if expanded {
        vec!["SCOPE_EXPANDED".to_string()]
    } else {
        Vec::new()
    })
    .data(json!({"gained": gained, "lost": lost, "risk_increased": riskier}));
    finish(args, response, &detail)
}

/** crane scope audit --skill|--run: the recorded invocation events of a skill or one invocation
 * Input
    - args: &Args - parsed arguments
    - since: Option<u64> - earliest event, Unix milliseconds
 * Output
    - Result<(), String>
*/
pub(super) fn audit(args: &Args, since: Option<u64>) -> Result<(), String> {
    let local = Local::open()?;
    let skill_id = args
        .value("skill")
        .map(|text| versioned(&text).map(|(id, _)| id))
        .transpose()?;
    let run = args.value("run");
    let events = EventStore::open(&local.context.runtime).read_all()?;
    let matching = events
        .iter()
        .filter(|event| event.event_type.starts_with("capability.invocation."))
        .filter(|event| since.is_none_or(|since| event.occurred_at >= since))
        .filter(|event| {
            skill_id
                .as_ref()
                .is_none_or(|id| event.payload["capability_id"].as_str() == Some(id))
        })
        .filter(|event| {
            run.as_ref()
                .is_none_or(|run| event.payload["invocation_id"].as_str() == Some(run))
        })
        .collect::<Vec<_>>();
    let mut states = std::collections::BTreeMap::<String, usize>::new();
    for event in &matching {
        *states
            .entry(
                event
                    .event_type
                    .trim_start_matches("capability.invocation.")
                    .to_string(),
            )
            .or_default() += 1;
    }
    let status = if matching.is_empty() {
        Status::Unknown
    } else {
        Status::Ok
    };
    let detail = states
        .iter()
        .map(|(state, count)| format!("  {state}: {count}"))
        .collect::<Vec<_>>();
    let response = Response::new("scope.audit", status, if matching.is_empty() { "no invocation events are recorded for this selection on this machine (absent, not clean)".to_string() } else { format!("{} invocation events", matching.len()) })
        .data(json!({"transitions": states, "events": matching.len()}));
    finish(args, response, &detail)
}

/** Parse a --since value: "7d", "24h", "30m", or a number of days
 * Input
    - value: &str - text
 * Output
    - Result<u64, String> earliest Unix milliseconds
*/
pub(super) fn since(value: &str) -> Result<u64, String> {
    let (number, unit) = match value.char_indices().last() {
        Some((index, unit)) if unit.is_ascii_alphabetic() => (&value[..index], unit),
        _ => (value, 'd'),
    };
    let amount = number
        .parse::<u64>()
        .ok()
        .filter(|amount| *amount > 0)
        .ok_or_else(|| format!("since must be like 7d, 24h, or 30m, got '{value}'"))?;
    let millis = match unit {
        'd' => 86_400_000,
        'h' => 3_600_000,
        'm' => 60_000,
        _ => return Err(format!("since must be like 7d, 24h, or 30m, got '{value}'")),
    };
    Ok(crate::platform::now_millis().saturating_sub(amount * millis))
}
