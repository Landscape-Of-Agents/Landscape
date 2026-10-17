// crane governance sign: an admin approves the current .crane/governance.yaml (zones, flows, and
// context packs) by covering it with a new signed manifest. Until then an edited file fails
// verification, and agents cannot change code, so nobody can loosen a zone or flow unnoticed.
use crate::governance::policy::PolicySet;
use crate::governance::state::{commit, State, GOVERNANCE_SIGNED};
use crate::governance::workspace::Workspace;
use crate::governance::zones::{self, Lifecycle};
use crate::trust::require_human;
use serde_json::json;

/** Run a governance command: sign, or graph
 * Input
    - args: &[String] - "sign", or "graph" with an optional --json
 * Output
    - Result<(), String>
    - Error if run by an agent, by a machine that is not an admin, or the file is invalid or has
      blocking findings (unknown references, duplicate identifiers, unresolved flows, stage cycles)
*/
pub(crate) fn governance(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) == Some("graph") {
        return graph(&args[1..]);
    }
    if args.first().map(String::as_str) != Some("sign") || args.len() != 1 {
        return Err("usage: crane governance sign | crane governance graph [--json]".into());
    }
    require_human("crane governance sign")?;
    let workspace = Workspace::locate()?;
    // Refuse a file the hooks could not enforce, so broken rules are never approved
    let governance = zones::load(&workspace)?;
    let resolution = zones::resolve(
        &workspace,
        &governance,
        &State::load(&workspace)?,
        &PolicySet::load(&workspace)?,
    )?;
    // Signing is what approves added, moved, or removed zone and flow markers
    let blocking = resolution
        .findings
        .iter()
        .filter(|finding| finding.blocking())
        .filter(|finding| {
            !matches!(
                finding.code.as_str(),
                "source_marker_unsigned" | "source_marker_removed"
            )
        })
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    let markers = resolution.source_markers.clone();
    if !blocking.is_empty() {
        return Err(format!(
            "not signing .crane/governance.yaml: {}",
            blocking.join("; ")
        ));
    }
    let active = |status: Lifecycle| status == Lifecycle::Active;
    let zones_active = governance
        .zones
        .iter()
        .filter(|zone| active(zone.status))
        .count();
    let flows_active = governance
        .flows
        .iter()
        .filter(|flow| active(flow.status))
        .count();
    let state = commit(
        &workspace,
        "crane governance sign",
        GOVERNANCE_SIGNED,
        json!({
            "zones": governance.zones.len(),
            "flows": governance.flows.len(),
            "context_packs": governance.context_packs.len(),
            "source_markers": markers,
        }),
        |state, _| {
            // Recorded only once there are markers, so repositories without them keep their digest
            if !markers.is_empty() || state.config.source_markers.is_some() {
                state.config.source_markers = Some(markers.clone());
            }
            Ok(())
        },
    )?;
    println!(
        "Signed .crane/governance.yaml at generation {}: {} zones ({zones_active} active), {} flows ({flows_active} active), {} context packs, {} zone/flow source blocks",
        state.generation(),
        governance.zones.len(),
        governance.flows.len(),
        governance.context_packs.len(),
        markers.len()
    );
    Ok(())
}

/** Print the governance graph: zones, flows in stage order, and the selections inside them (nested
 * selections under their parent), kept acyclic, with the agents recently active in each
 * Input
    - args: &[String] - optional --json
 * Output
    - Result<(), String>
    - Error if the governance state cannot be read
*/
fn graph(args: &[String]) -> Result<(), String> {
    let parsed = crate::cli::args::Args::parse(args, &[], &["json"])?;
    parsed.expect_positional(0, "crane governance graph [--json]")?;
    let graph = crate::governance::graph::build(&Workspace::locate()?)?;
    if parsed.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&graph).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    let nodes = graph["nodes"].as_array().cloned().unwrap_or_default();
    let count = |kind: &str| nodes.iter().filter(|node| node["kind"] == kind).count();
    println!(
        "Governance graph at generation {}: {} zones, {} flows, {} selections, {} edges ({})",
        graph["generation"],
        count("zone"),
        count("flow"),
        count("selection"),
        graph["edges"].as_array().map_or(0, Vec::len),
        if graph["acyclic"] == true {
            "acyclic"
        } else {
            "cycles left out"
        }
    );
    for edge in graph["edges"].as_array().into_iter().flatten() {
        println!(
            "  {} -[{}]-> {}",
            edge["from"].as_str().unwrap_or_default(),
            edge["kind"].as_str().unwrap_or_default(),
            edge["to"].as_str().unwrap_or_default()
        );
    }
    for edge in graph["rejected"].as_array().into_iter().flatten() {
        println!(
            "  left out: {}",
            edge["reason"].as_str().unwrap_or_default()
        );
    }
    for entry in graph["activity"].as_array().into_iter().flatten() {
        println!(
            "  {} {}: {} tool calls ({} denied) by {} agent(s){}",
            entry["kind"].as_str().unwrap_or_default(),
            entry["id"].as_str().unwrap_or_default(),
            entry["calls"],
            entry["denied"],
            entry["agents"].as_array().map_or(0, Vec::len),
            if entry["active"] == true {
                ", active now"
            } else {
                ""
            }
        );
    }
    Ok(())
}
