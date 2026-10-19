// Work Capture commands: 'crane capture demo' runs the deterministic end-to-end demonstration from
// fixtures; 'crane capture sources' lists this repository's live evidence sources (Git, DataSea,
// Landscape, Crane) with their health; 'crane capture investigate' collects a bounded Evidence Pack
// about a commit (and optionally a task) from those sources and keeps it in the trust directory's
// runtime folder.

use std::collections::BTreeSet;

use serde_json::json;

use super::args::Args;
use crate::capture::broker::recipe::{Budget, RecipeRef};
use crate::capture::broker::run::{EvidenceQueryBroker, EvidenceQueryRequest, KnownRefs};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::pack::{EvidencePack, PackLimits};
use crate::capture::resources::connector::ResourceConnector;
use crate::capture::resources::local::{
    CraneConnector, DataSeaConnector, GitConnector, LandscapeConnector,
};
use crate::capture::resources::registry::{
    Capabilities, Operation, ResourceDefinition, ResourceRegistry, ResourceScope, ResourceStatus,
    ResourceType,
};
use crate::capture::resources::scope::TimeWindow;
use crate::capture::store::CaptureStore;
use crate::capture::{demo, AccessPolicy, Actor, ActorKind, Clock, Sensitivity};
use crate::governance::workspace::Workspace;
use crate::platform::{actor, git};
use crate::telemetry::events::EventStore;

/** Dispatch a capture command
 * Input
    - args: &[String] - arguments after "capture"
 * Output
    - Result<(), String>
*/
pub(crate) fn capture(args: &[String]) -> Result<(), String> {
    let usage = "usage: crane capture demo [--json] | crane capture sources [--json] | crane capture investigate [commit SHA] [task TASK_ID] [--json]";
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(&args[1..], &[], &["json"])?),
        Some("sources") => sources(&Args::parse(&args[1..], &[], &["json"])?),
        Some("investigate") => {
            investigate(&Args::parse(&args[1..], &["commit", "task"], &["json"])?)
        }
        _ => Err(usage.into()),
    }
}

/** Run the demonstration and print its pack
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capture demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    println!("Work Capture demonstration (fixtures; no live service was contacted)\n");
    print_pack(&report.pack);
    if let Some(replay) = &report.replay {
        println!(
            "\nReplay of {}: {:?}",
            replay.original_query, replay.fidelity
        );
        for difference in &replay.differences {
            println!("  - {difference}");
        }
    }
    println!("\nContributions:");
    for contribution in &report.contributions {
        println!(
            "  {} {:?} {:?} by {} ({:?}, {:?})",
            contribution.contribution_id,
            contribution.contribution_type,
            contribution.namespace,
            contribution.contributor_id,
            contribution.status,
            contribution.locality
        );
    }
    if let Some(episode) = &report.episode {
        println!(
            "\nEpisode {}: outcome {:?}, {} feedback entries",
            episode.episode_id,
            episode.outcome,
            episode.feedback.len()
        );
    }
    Ok(())
}

/** The repository label used to scope local evidence
 * Input
    - workspace: &Workspace - repository
 * Output
    - String
*/
pub(super) fn repository_label(workspace: &Workspace) -> String {
    workspace
        .remote
        .clone()
        .unwrap_or_else(|| format!("local:{}", &workspace.repository_id[..16]))
}

/** A live source: id, resource type, connector type, evidence types, filters, and connector */
type LocalSource = (
    &'static str,
    ResourceType,
    &'static str,
    Vec<EvidenceType>,
    Vec<&'static str>,
    Box<dyn ResourceConnector>,
);

/** Register and connect this repository's live sources
 * Input
    - workspace: &Workspace - repository
    - clock: Clock - clock
 * Output
    - Result<EvidenceQueryBroker, String>
*/
fn local_broker(workspace: &Workspace, clock: Clock) -> Result<EvidenceQueryBroker, String> {
    let repository = repository_label(workspace);
    let runtime = workspace.trust()?.runtime();
    let store = CaptureStore::open(&runtime.join("capture"))?;
    let mut registry = ResourceRegistry::default();
    let sources: [LocalSource; 4] = [
        (
            "git",
            ResourceType::Git,
            "git",
            vec![EvidenceType::CodeChange],
            vec!["commit", "path"],
            Box::new(GitConnector {
                root: workspace.root.clone(),
            }),
        ),
        (
            "datasea",
            ResourceType::AgentExecution,
            "datasea",
            vec![
                EvidenceType::ToolCall,
                EvidenceType::AgentSession,
                EvidenceType::AgentEvent,
            ],
            vec!["session", "task", "event_type"],
            Box::new(DataSeaConnector {
                store: EventStore::open(&runtime),
            }),
        ),
        (
            "landscape",
            ResourceType::Landscape,
            "landscape",
            vec![EvidenceType::GraphState],
            vec!["kind", "id"],
            Box::new(LandscapeConnector {
                workspace: workspace.clone(),
            }),
        ),
        (
            "crane",
            ResourceType::Crane,
            "crane",
            vec![EvidenceType::Policy, EvidenceType::Validation],
            vec!["kind", "name"],
            Box::new(CraneConnector {
                workspace: workspace.clone(),
            }),
        ),
    ];
    let mut connectors = Vec::new();
    for (id, resource_type, connector_type, evidence_types, filters, connector) in sources {
        registry.register(
            ResourceDefinition {
                resource_id: id.into(),
                resource_type,
                connector_type: connector_type.into(),
                capabilities: Capabilities {
                    operations: vec![
                        Operation::Discover,
                        Operation::Fetch,
                        Operation::Query,
                        Operation::Metadata,
                        Operation::Health,
                    ],
                    evidence_types,
                    filters: filters.into_iter().map(str::to_string).collect(),
                },
                scope: ResourceScope {
                    repositories: vec![repository.clone()],
                    ..ResourceScope::default()
                },
                auth_reference: None,
                configuration: Default::default(),
                configuration_version: 1,
                status: ResourceStatus::Active,
                created_at: 0,
                updated_at: 0,
            },
            clock.now(),
        )?;
        connectors.push((id, connector));
    }
    let mut broker = EvidenceQueryBroker::new(registry, store, clock)?;
    for (id, connector) in connectors {
        broker.connect(id, connector)?;
    }
    Ok(broker)
}

/** List the live sources with their health
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn sources(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capture sources [--json]")?;
    let workspace = Workspace::locate()?;
    let broker = local_broker(&workspace, Clock::system())?;
    let rows = broker
        .registry
        .definitions
        .values()
        .map(|definition| {
            let health = broker
                .connectors
                .get(&definition.resource_id)
                .map(|connector| connector.health_check());
            let discovered = broker
                .connectors
                .get(&definition.resource_id)
                .map(|connector| connector.discover());
            json!({
                "resource_id": definition.resource_id,
                "resource_type": definition.resource_type,
                "connector_type": definition.connector_type,
                "evidence_types": definition.capabilities.evidence_types,
                "available": health.as_ref().map(|health| health.available),
                "detail": health.map(|health| health.detail),
                "objects": discovered.as_ref().map(|outcome| outcome.records.len()),
                "snapshot": discovered.and_then(|outcome| outcome.snapshot),
            })
        })
        .collect::<Vec<_>>();
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|error| error.to_string())?
        );
    } else {
        for row in rows {
            println!(
                "{:<10} {:<9} {}",
                row["resource_id"].as_str().unwrap_or_default(),
                if row["available"].as_bool() == Some(true) {
                    "AVAILABLE"
                } else {
                    "UNAVAILABLE"
                },
                row["detail"].as_str().unwrap_or_default()
            );
        }
        println!("GitHub pull requests, CI, deployments, logs, and metrics have no live connector yet; 'crane capture demo' exercises them from fixtures.");
    }
    Ok(())
}

/** Investigate a commit (and optionally a task) from the live sources and keep the pack
 * Input
    - args: &Args - commit, task, and flags
 * Output
    - Result<(), String>
*/
fn investigate(args: &Args) -> Result<(), String> {
    args.expect_positional(
        0,
        "crane capture investigate [commit SHA] [task TASK_ID] [--json]",
    )?;
    let workspace = Workspace::locate()?;
    let revision = args.value("commit").unwrap_or_else(|| "HEAD".into());
    let commit = git::git_in(
        &workspace.root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ],
    )
    .map_err(|_| format!("'{revision}' is not a commit of this repository"))?;
    let repository = repository_label(&workspace);
    let mut broker = local_broker(&workspace, Clock::system())?;
    let policy = AccessPolicy {
        actor: Actor::new(&actor(), ActorKind::Human),
        resources: broker.registry.definitions.keys().cloned().collect(),
        repositories: BTreeSet::from([repository.clone()]),
        environments: ["local", "ci", "hosted"]
            .iter()
            .map(|name| name.to_string())
            .collect(),
        evidence_types: None,
        max_sensitivity: Sensitivity::Confidential,
        may_review: false,
    };
    let task = args.value("task");
    let request = EvidenceQueryRequest {
        episode_id: None,
        task_id: task.clone(),
        question: format!(
            "What changed in {} and under which governance?",
            &commit[..commit.len().min(12)]
        ),
        investigation_class: "local-change".into(),
        recipe: Some(RecipeRef {
            id: "local-change".into(),
            version: Some(1),
        }),
        plan: None,
        known: KnownRefs {
            repository: Some(repository),
            commits: vec![commit],
            ..KnownRefs::default()
        },
        resources: Vec::new(),
        required_evidence: Vec::new(),
        evidence_types: Vec::new(),
        time_window: TimeWindow::default(),
        budget: Budget::default(),
        freshness_ms: None,
        pack_limits: PackLimits::default(),
    };
    let result = broker.investigate(request, &policy)?;
    broker.store.save()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({"pack": result.pack, "audit": result.audit}))
                .map_err(|error| error.to_string())?
        );
    } else {
        print_pack(&result.pack);
        println!(
            "\nKept in {}",
            workspace.trust()?.runtime().join("capture").display()
        );
    }
    Ok(())
}

/** Print a pack summary
 * Input
    - pack: &EvidencePack - pack
 * Output
    - None
*/
fn print_pack(pack: &EvidencePack) {
    println!("Evidence Pack {} ({:?})", pack.pack_id, pack.status);
    println!("  investigation: {}", pack.investigation);
    println!(
        "  recipe: {} v{} (query {}, stop: {:?})",
        pack.retrieval.recipe_id,
        pack.retrieval.recipe_version,
        pack.retrieval.query_id,
        pack.retrieval.stop_reason
    );
    let zones = pack
        .scope
        .zones
        .iter()
        .chain(pack.scope.flows.iter())
        .map(|reference| {
            format!(
                "{}@{}",
                reference.id,
                reference
                    .version
                    .clone()
                    .unwrap_or_else(|| "unknown".into())
            )
        })
        .collect::<Vec<_>>();
    if !zones.is_empty() {
        println!("  zones and flows: {}", zones.join(", "));
    }
    println!(
        "  evidence: {} records, {} entities, {} relations, {} bytes",
        pack.evidence.len(),
        pack.entities.len(),
        pack.context_view.relations.len(),
        pack.size_bytes
    );
    println!("\nSources:");
    for source in &pack.sources {
        println!(
            "  {:<18} {:<11} {:<12} {} records",
            source.step_id,
            source.resource_id.clone().unwrap_or_else(|| "-".into()),
            format!("{:?}", source.state),
            source.records
        );
    }
    if !pack.retrieval.branches.is_empty() {
        println!("\nBranches:");
        for branch in &pack.retrieval.branches {
            println!(
                "  {:<18} {}",
                branch.step_id,
                if branch.held { "taken" } else { "skipped" }
            );
        }
    }
    println!("\nAssertions:");
    for assertion in &pack.assertions {
        println!(
            "  [{:?}] {} ({} supporting, {} contradicting)",
            assertion.status,
            assertion.statement,
            assertion.supporting.len(),
            assertion.contradicting.len()
        );
    }
    if !pack.missing_evidence.is_empty() {
        println!("\nMissing evidence:");
        for missing in &pack.missing_evidence {
            println!(
                "  {:?} ({:?}): {}",
                missing.evidence_type, missing.state, missing.requirement
            );
        }
    }
    if !pack.warnings.is_empty() {
        println!("\nWarnings:");
        for warning in &pack.warnings {
            println!("  [{:?}] {}", warning.kind, warning.message);
        }
    }
    println!(
        "\nManifest: {} entries, digest {}",
        pack.manifest.entries.len(),
        pack.manifest.digest
    );
}
