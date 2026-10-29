// What agents receive about a flow: its knowledge as text (for 'crane-request flow ID' and, in a
// shorter form, with each tool call that touches the flow), and the one rule the knowledge adds to
// decisions: a flow whose priority routes to model tiers, with enforce_model on, holds a call from
// a model outside those tiers for a person. That rule only tightens; everything else here is
// guidance and never authorizes anything.

use std::collections::BTreeSet;

use super::model::{route, tier_of, FlowBook, Priority};
use crate::governance::zones::Governance;

/** Longest text of one context item shown with a tool call (characters) */
const ITEM_SHOWN: usize = 400;

/** Cut a text to a length, marking the cut
 * Input
    - text: &str - text
    - limit: usize - most characters
 * Output
    - String
*/
fn cut(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(limit).collect::<String>())
    }
}

/** The routing line of a flow
 * Input
    - book: &FlowBook - flow knowledge
    - priority: Priority - its priority
    - model: Option<&str> - the model working now
 * Output
    - String
*/
fn routing_line(book: &FlowBook, priority: Priority, model: Option<&str>) -> String {
    let (tiers, chosen) = route(book, priority, None);
    let tiers_text = tiers
        .iter()
        .map(|tier| tier.name())
        .collect::<Vec<_>>()
        .join(", ");
    let current = model.map(|model| match tier_of(book, model) {
        Some(tier) if tiers.contains(&tier) => {
            format!("; {model} is {} tier, allowed", tier.name())
        }
        Some(tier) => format!("; {model} is {} tier, not one of these", tier.name()),
        None => format!("; {model} is not in the organization's model list"),
    });
    format!(
        "Priority {}: model tiers {tiers_text}{}{}",
        priority.name(),
        chosen
            .map(|entry| format!(" (routed to {} by {})", entry.model, entry.provider))
            .unwrap_or_default(),
        current.unwrap_or_default()
    )
}

/** The full knowledge of one flow as text
 * Input
    - runtime: &std::path::Path - runtime directory (watchers, incidents)
    - book: &FlowBook - flow knowledge
    - governance: &Governance - zones and flows
    - id: &str - flow id
    - model: Option<&str> - the asking agent's model
 * Output
    - String
*/
pub(crate) fn text(
    runtime: &std::path::Path,
    book: &FlowBook,
    governance: &Governance,
    id: &str,
    model: Option<&str>,
) -> String {
    let mut lines = Vec::new();
    let declared = governance.flows.iter().find(|flow| flow.id == id);
    if let Some(flow) = declared {
        lines.push(format!(
            "Flow {id}: {} (criticality {}, autonomy ceiling {}, {})",
            if flow.description.is_empty() {
                "no description"
            } else {
                &flow.description
            },
            serde_json::to_value(flow.criticality)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
            flow.autonomy_ceiling.name(),
            serde_json::to_value(flow.status)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default(),
        ));
        lines.push(format!("Starts at: {}", flow.entry_points.join(", ")));
    }
    let Some(knowledge) = book.flows.get(id) else {
        lines.push(
            "No knowledge recorded for this flow yet (no trace, context, people, or tests).".into(),
        );
        return lines.join("\n");
    };
    if let Some(priority) = knowledge.priority {
        lines.push(routing_line(book, priority, model));
    }
    if let Some(trace) = &knowledge.trace {
        lines.push(format!(
            "Approved trace (revision {}, {}): {} functions, {} calls in {} files: {}",
            trace.revision,
            trace.status,
            trace.nodes,
            trace.edges,
            trace.files.len(),
            trace
                .files
                .iter()
                .take(12)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !knowledge.people.is_empty() {
        lines.push(format!(
            "People: {}",
            knowledge
                .people
                .iter()
                .map(|person| format!(
                    "{} ({}{}{})",
                    person.name,
                    serde_json::to_value(person.role)
                        .ok()
                        .and_then(|value| value.as_str().map(str::to_string))
                        .unwrap_or_default(),
                    person
                        .jira
                        .as_ref()
                        .map(|jira| format!(", Jira {jira}"))
                        .unwrap_or_default(),
                    person
                        .slack
                        .as_ref()
                        .map(|slack| format!(", Slack {slack}"))
                        .unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    for item in &knowledge.context {
        lines.push(format!(
            "Context [{}] {}: {}{}{}{}",
            item.kind.name(),
            item.title,
            cut(&item.value, ITEM_SHOWN),
            item.unit
                .as_ref()
                .map(|unit| format!(" {unit}"))
                .unwrap_or_default(),
            item.threshold
                .map(|threshold| format!(" (keep within {threshold})"))
                .unwrap_or_default(),
            item.file
                .as_ref()
                .map(|file| format!(
                    " — {file}{}",
                    item.lines
                        .as_ref()
                        .map(|lines| format!(":{lines}"))
                        .unwrap_or_default()
                ))
                .unwrap_or_default()
        ));
    }
    let required = knowledge
        .tests
        .iter()
        .filter(|test| test.required)
        .collect::<Vec<_>>();
    if !knowledge.tests.is_empty() {
        lines.push(format!(
            "Tests: {} approved, {} required for every release: {}",
            knowledge.tests.len(),
            required.len(),
            knowledge
                .tests
                .iter()
                .map(|test| format!(
                    "{}{} ({})",
                    test.path,
                    if test.required { " [required]" } else { "" },
                    test.name
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    let watchers = super::watch::watchers(runtime)
        .into_iter()
        .filter(|watcher| watcher.active && watcher.flow == id)
        .collect::<Vec<_>>();
    lines.push(format!(
        "Watchers: {} active of {} allowed",
        watchers.len(),
        book.watcher_limit(id)
    ));
    let open = super::watch::incidents(runtime)
        .into_iter()
        .filter(|incident| incident.flow == id)
        .take(3)
        .map(|incident| {
            format!(
                "{} {}: {}",
                incident.incident_id, incident.code, incident.summary
            )
        })
        .collect::<Vec<_>>();
    if !open.is_empty() {
        lines.push(format!("Recent incidents: {}", open.join("; ")));
    }
    lines
        .push("This is guidance; it never authorizes anything. Crane decides every action.".into());
    lines.join("\n")
}

/** The short knowledge of the flows a tool call touches, sent with the call
 * Input
    - book: &FlowBook - flow knowledge
    - flows: &BTreeSet<String> - touched flow ids
    - model: Option<&str> - the agent's model
 * Output
    - Option<String> (None when none of them has knowledge)
*/
pub(crate) fn guidance(
    book: &FlowBook,
    flows: &BTreeSet<String>,
    model: Option<&str>,
) -> Option<String> {
    let mut parts = Vec::new();
    for id in flows {
        let Some(knowledge) = book.flows.get(id) else {
            continue;
        };
        let mut lines = vec![format!("Flow knowledge · {id}")];
        if let Some(priority) = knowledge.priority {
            lines.push(routing_line(book, priority, model));
        }
        if !knowledge.people.is_empty() {
            lines.push(format!(
                "Designated: {}",
                knowledge
                    .people
                    .iter()
                    .map(|person| person.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        for item in knowledge.context.iter().take(8) {
            lines.push(format!(
                "[{}] {}: {}",
                item.kind.name(),
                item.title,
                cut(&item.value, 200)
            ));
        }
        let required = knowledge
            .tests
            .iter()
            .filter(|test| test.required)
            .map(|test| test.path.clone())
            .collect::<Vec<_>>();
        if !required.is_empty() {
            lines.push(format!(
                "Required tests (every release): {}",
                required.join(", ")
            ));
        }
        if lines.len() > 1 {
            parts.push(lines.join("\n"));
        }
    }
    (!parts.is_empty()).then(|| {
        format!(
            "{}\nAsk for more with 'crane-request flow ID' (guidance only, never authorization).",
            parts.join("\n")
        )
    })
}

/** The flows among touched entities whose model routing a call breaks: enforce_model is on and
 * the agent's model is not in the routed tiers (an unknown model counts as outside)
 * Input
    - book: &FlowBook - flow knowledge
    - flows: &BTreeSet<String> - touched flow ids
    - model: Option<&str> - the agent's model (None: not reported)
 * Output
    - Vec<String> reasons, one per flow
*/
pub(crate) fn model_findings(
    book: &FlowBook,
    flows: &BTreeSet<String>,
    model: Option<&str>,
) -> Vec<String> {
    let mut reasons = Vec::new();
    for id in flows {
        let Some(knowledge) = book
            .flows
            .get(id)
            .filter(|knowledge| knowledge.enforce_model)
        else {
            continue;
        };
        let Some(priority) = knowledge.priority else {
            continue;
        };
        let (tiers, _) = route(book, priority, None);
        let tier = model.and_then(|model| tier_of(book, model));
        if !tier.is_some_and(|tier| tiers.contains(&tier)) {
            reasons.push(format!(
                "flow {id} is {} and takes models of the {} tier(s); {} — a person must approve this change",
                priority.name(),
                tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>().join(", "),
                match (model, tier) {
                    (None, _) => "the agent reported no model".to_string(),
                    (Some(model), None) => format!("{model} is not in the organization's model list"),
                    (Some(model), Some(tier)) => format!("{model} is {} tier", tier.name()),
                }
            ));
        }
    }
    reasons
}
