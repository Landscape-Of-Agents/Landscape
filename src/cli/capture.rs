// Work Capture commands: 'crane capture demo' runs the deterministic end-to-end demonstration from
// fixtures; 'crane capture sources' lists this repository's live evidence sources (Git, DataSea,
// Landscape, Crane) with their health; 'crane capture investigate' collects a bounded Evidence Pack
// about a commit (and optionally a task) from those sources, by a chosen recipe or question class,
// and keeps it in the trust directory's runtime folder. 'replay' re-runs a recorded investigation
// and reports what changed, 'verify' checks a pack's manifest, 'contribute' proposes context,
// 'review' accepts or rejects a contribution (owner and admins only), 'outcome' records a work
// episode's outcome with its evidence, and 'feedback' records whether a pack served the person who
// used it. A question class no recipe serves is explored: the pack is Exploratory, and feedback on
// explorations is what Insight learns recipes from.

use std::collections::BTreeSet;

use serde_json::json;

use super::args::Args;
use crate::capture::broker::recipe::{Budget, RecipeRef};
use crate::capture::broker::run::{EvidenceQueryBroker, EvidenceQueryRequest, KnownRefs};
use crate::capture::format::record::EvidenceType;
use crate::capture::ingest::pack::{EvidencePack, PackLimits};
use crate::capture::resources::connector::ResourceConnector;
use crate::capture::resources::github::{GhCli, GitHubConnector};
use crate::capture::resources::local::{
    CraneConnector, DataSeaConnector, GitConnector, LandscapeConnector,
};
use crate::capture::resources::registry::{
    Capabilities, Operation, ResourceDefinition, ResourceRegistry, ResourceScope, ResourceStatus,
    ResourceType,
};
use crate::capture::resources::scope::TimeWindow;
use crate::capture::store::CaptureStore;
use crate::capture::{demo, AccessPolicy, Actor, Clock, Sensitivity, Truth};
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
    let usage = "usage: crane capture demo [--json] | sources [--json] | investigate [commit SHA] [task TASK_ID] [class CLASS] [recipe ID [version N]] [episode EPI] [--json] | replay QUERY_ID [--json] | verify PACK_ID | contribute FILE [--json] | review CONTRIBUTION_ID accept|reject reason TEXT | outcome EPISODE OUTCOME [evidence ID,ID] | feedback PACK_ID sufficient yes|no|unknown [missing TYPE,TYPE] [notes TEXT] | ask FILE | question QUESTION_ID answer PACK_ID|block|unresolvable|reopen | confirm RELATION_ID";
    let id = |position: usize| {
        args.get(position)
            .filter(|word| !word.starts_with('-'))
            .cloned()
            .ok_or_else(|| usage.to_string())
    };
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(&args[1..], &[], &["json"])?),
        Some("sources") => sources(&Args::parse(&args[1..], &[], &["json"])?),
        Some("investigate") => investigate(&Args::parse(
            &args[1..],
            &["commit", "task", "class", "recipe", "version", "episode"],
            &["json"],
        )?),
        Some("replay") => replay(&id(1)?, &Args::parse(&args[2..], &[], &["json"])?),
        Some("verify") => verify(&id(1)?),
        Some("contribute") => contribute(&id(1)?, &Args::parse(&args[2..], &[], &["json"])?),
        Some("review") => {
            let decision = id(2)?;
            review(
                &id(1)?,
                &decision,
                &Args::parse(&args[3..], &["reason"], &[])?,
            )
        }
        Some("outcome") => outcome(
            &id(1)?,
            &id(2)?,
            &Args::parse(&args[3..], &["evidence"], &[])?,
        ),
        Some("feedback") => feedback(
            &id(1)?,
            &Args::parse(&args[2..], &["sufficient", "missing", "notes"], &[])?,
        ),
        Some("ask") => ask(&id(1)?),
        Some("question") => question(&id(1)?, &args[2..]),
        Some("confirm") => confirm(&id(1)?),
        _ => Err(usage.into()),
    }
}

/** Whether this machine may review contributions: its signing key is the repository owner's or a
 * signed admin's (the same authority that may change the rules), never what a caller claims
 * Input
    - workspace: &Workspace - repository
 * Output
    - bool
*/
fn may_review(workspace: &Workspace) -> bool {
    let Ok(trust) = workspace.trust() else {
        return false;
    };
    let Ok(Some(key)) = trust.signing_public_key() else {
        return false;
    };
    let Ok(state) = crate::governance::state::State::load(workspace) else {
        return false;
    };
    crate::governance::admins::owner(&state).as_deref() == Some(key.as_str())
        || crate::governance::admins::signers(&state).contains(&key)
}

/** The access policy of the person running Crane over this repository's local sources: every
 * local source of this repository, review rights from this machine's key
 * Input
    - workspace: &Workspace - repository
    - broker: &EvidenceQueryBroker - broker with the local sources
 * Output
    - Result<AccessPolicy, String>
    - Error inside an AI agent session
*/
pub(super) fn local_policy(
    workspace: &Workspace,
    broker: &EvidenceQueryBroker,
) -> Result<AccessPolicy, String> {
    Ok(AccessPolicy {
        actor: Actor::human_operator(&actor())?,
        resources: broker.registry.definitions.keys().cloned().collect(),
        repositories: BTreeSet::from([repository_label(workspace)]),
        environments: ["local", "ci", "hosted"]
            .iter()
            .map(|name| name.to_string())
            .collect(),
        evidence_types: None,
        max_sensitivity: Sensitivity::Confidential,
        may_review: may_review(workspace),
    })
}

/** Attach an investigation's summary and metrics to this command's event (DataSea receives them
 * with the command, as the command's own fact; Work Capture itself never writes to DataSea)
 * Input
    - result: &EvidenceQueryResult - investigation
 * Output
    - None
*/
fn note_investigation(result: &crate::capture::broker::run::EvidenceQueryResult) {
    crate::platform::notes::note(
        "CAPTURE_INVESTIGATION",
        json!({
            "query_id": result.audit.query_id,
            "pack_id": result.pack.pack_id,
            "status": result.pack.status,
            "recipe": format!("{}@v{}", result.pack.retrieval.recipe_id, result.pack.retrieval.recipe_version),
            "stop_reason": result.audit.stop_reason,
            "missing": result.pack.missing_evidence.len(),
            "metrics": result.audit.metrics,
        }),
    );
}

/** Print a value as pretty JSON
 * Input
    - value: &serde_json::Value - value
 * Output
    - Result<(), String>
*/
fn print_json(value: &serde_json::Value) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

/** Re-run a recorded investigation against the sources as they are now and report what changed
 * Input
    - query_id: &str - recorded investigation
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn replay(query_id: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capture replay QUERY_ID [--json]")?;
    let workspace = Workspace::locate()?;
    let mut broker = local_broker(&workspace, Clock::system())?;
    let policy = local_policy(&workspace, &broker)?;
    let result = broker.replay(query_id, &policy)?;
    broker.store.save()?;
    note_investigation(&result);
    if args.flag("json") {
        return print_json(&json!({"pack": result.pack, "audit": result.audit}));
    }
    if let Some(comparison) = &result.audit.replay {
        println!(
            "Replay of {}: {:?}",
            comparison.original_query, comparison.fidelity
        );
        for difference in &comparison.differences {
            println!("  - {difference}");
        }
    }
    print_pack(&result.pack);
    Ok(())
}

/** Check a kept Evidence Pack against its manifest
 * Input
    - pack_id: &str - pack
 * Output
    - Result<(), String>
    - Error when the pack is unknown or does not match its manifest
*/
fn verify(pack_id: &str) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let store = CaptureStore::open(&workspace.trust()?.runtime().join("capture"))?;
    let pack = store
        .packs
        .get(pack_id)
        .ok_or_else(|| format!("no Evidence Pack '{pack_id}' is kept here"))?;
    pack.verify_manifest()
        .map_err(|error| format!("Evidence Pack {pack_id} does not match its manifest: {error}"))?;
    println!(
        "Evidence Pack {pack_id} matches its manifest ({} entries, digest {})",
        pack.manifest.entries.len(),
        pack.manifest.digest
    );
    Ok(())
}

/** Propose context from a JSON file holding a proposal (target_ref, episode_id, contribution_type,
 * namespace, content, evidence_refs, locality, valid_until); a person proposes in the observed
 * namespace, and the proposal stays proposed until another person reviews it
 * Input
    - file: &str - proposal file
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn contribute(file: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capture contribute FILE [--json]")?;
    let workspace = Workspace::locate()?;
    let text = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
    let proposal = serde_json::from_str::<crate::capture::ingest::contributions::Proposal>(&text)
        .map_err(|error| format!("{file} is not a proposal: {error}"))?;
    let mut store = CaptureStore::open(&workspace.trust()?.runtime().join("capture"))?;
    let actor = Actor::human_operator(&actor())?;
    let id = store.propose(&actor, proposal, crate::platform::now_millis())?;
    store.save()?;
    if args.flag("json") {
        return print_json(&json!(store.contributions.contributions[&id]));
    }
    println!(
        "Proposed {id} (it stays proposed until another person with review rights accepts it)"
    );
    Ok(())
}

/** Accept or reject a contribution: only the repository owner or an admin may, and never their own
 * Input
    - id: &str - contribution
    - decision: &str - accept or reject
    - args: &Args - reason
 * Output
    - Result<(), String>
*/
fn review(id: &str, decision: &str, args: &Args) -> Result<(), String> {
    let usage = "crane capture review CONTRIBUTION_ID accept|reject reason TEXT";
    args.expect_positional(0, usage)?;
    let decision = match decision {
        "accept" => crate::capture::ingest::contributions::ContributionStatus::Accepted,
        "reject" => crate::capture::ingest::contributions::ContributionStatus::Rejected,
        _ => return Err(format!("usage: {usage}")),
    };
    let reason = args
        .value("reason")
        .ok_or_else(|| format!("usage: {usage}"))?;
    let workspace = Workspace::locate()?;
    let broker = local_broker(&workspace, Clock::system())?;
    let policy = local_policy(&workspace, &broker)?;
    let mut store = broker.store;
    store.review(
        &policy,
        id,
        decision,
        &reason,
        crate::platform::now_millis(),
    )?;
    store.save()?;
    println!("Contribution {id}: {decision:?}");
    Ok(())
}

/** Record whether an Evidence Pack served the person running Crane: sufficient yes, no, or unknown,
 * the evidence that was still needed, and a note
 * Input
    - pack_id: &str - pack
    - args: &Args - sufficient, missing (comma separated evidence types), notes
 * Output
    - Result<(), String>
*/
fn feedback(pack_id: &str, args: &Args) -> Result<(), String> {
    let usage =
        "crane capture feedback PACK_ID sufficient yes|no|unknown [missing TYPE,TYPE] [notes TEXT]";
    args.expect_positional(0, usage)?;
    let sufficient = args
        .value("sufficient")
        .and_then(|value| serde_json::from_value::<Truth>(json!(value)).ok())
        .ok_or_else(|| format!("usage: {usage}"))?;
    let missing = args
        .value("missing")
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(|item| {
                    serde_json::from_value::<EvidenceType>(json!(item))
                        .map_err(|_| format!("'{item}' is not an evidence type"))
                })
                .collect::<Result<Vec<_>, _>>()
        })
        .transpose()?
        .unwrap_or_default();
    let person = Actor::human_operator(&actor())?;
    let workspace = Workspace::locate()?;
    let mut store = CaptureStore::open(&workspace.trust()?.runtime().join("capture"))?;
    let episode = store.record_feedback(
        pack_id,
        &person,
        sufficient,
        missing,
        &args.value("notes").unwrap_or_default(),
        crate::platform::now_millis(),
    )?;
    store.save()?;
    println!(
        "Feedback on {pack_id} recorded for work episode {episode}: sufficient {sufficient:?}"
    );
    Ok(())
}

/** Ask a question from a JSON file (question_id, text, target, reason, relevant_evidence,
 * required_evidence, permitted_resources, recipe); the person running Crane is recorded as asking
 * Input
    - file: &str - question file
 * Output
    - Result<(), String>
*/
fn ask(file: &str) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let text = std::fs::read_to_string(file).map_err(|error| format!("{file}: {error}"))?;
    let mut value: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| format!("{file} is not JSON: {error}"))?;
    let asker = Actor::human_operator(&actor())?;
    if let Some(object) = value.as_object_mut() {
        object.insert("asked_by".into(), json!(asker));
        object.insert("created_at".into(), json!(crate::platform::now_millis()));
        object.insert("state".into(), json!("open"));
        object
            .entry("answer_pack")
            .or_insert(serde_json::Value::Null);
    }
    let question = serde_json::from_value::<crate::capture::ingest::episode::Question>(value)
        .map_err(|error| format!("{file} is not a question: {error}"))?;
    let mut store = CaptureStore::open(&workspace.trust()?.runtime().join("capture"))?;
    let id = store.ask(question)?;
    store.save()?;
    println!("Asked {id}");
    Ok(())
}

/** Move a question through its lifecycle
 * Input
    - id: &str - question
    - words: &[String] - answer PACK_ID, block, unresolvable, or reopen
 * Output
    - Result<(), String>
*/
fn question(id: &str, words: &[String]) -> Result<(), String> {
    use crate::capture::ingest::episode::QuestionState;
    let usage =
        "usage: crane capture question QUESTION_ID answer PACK_ID|block|unresolvable|reopen";
    let (state, pack) = match words
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["answer", pack] => (QuestionState::Answered, Some(pack.to_string())),
        ["block"] => (QuestionState::Blocked, None),
        ["unresolvable"] => (QuestionState::Unresolvable, None),
        ["reopen"] => (QuestionState::Open, None),
        _ => return Err(usage.into()),
    };
    Actor::human_operator(&actor())?;
    let workspace = Workspace::locate()?;
    let mut store = CaptureStore::open(&workspace.trust()?.runtime().join("capture"))?;
    store.set_question_state(id, state, pack.as_deref())?;
    store.save()?;
    println!("Question {id}: {state:?}");
    Ok(())
}

/** Confirm a relation as a reviewer (owner and admins only): a reviewed relation is recorded beside
 * the original
 * Input
    - id: &str - relation
 * Output
    - Result<(), String>
*/
fn confirm(id: &str) -> Result<(), String> {
    let workspace = Workspace::locate()?;
    let broker = local_broker(&workspace, Clock::system())?;
    let policy = local_policy(&workspace, &broker)?;
    let mut store = broker.store;
    let reviewed = store.review_relation(&policy, id)?;
    store.save()?;
    println!("Relation {id} confirmed as {reviewed}");
    Ok(())
}

/** Record a work episode's outcome; succeeded and partially verified need evidence of the episode
 * Input
    - episode: &str - work episode
    - outcome: &str - succeeded, failed, partially_verified, unknown, or abandoned
    - args: &Args - evidence ids (comma separated)
 * Output
    - Result<(), String>
*/
fn outcome(episode: &str, outcome: &str, args: &Args) -> Result<(), String> {
    let usage = "crane capture outcome EPISODE OUTCOME [evidence ID,ID]";
    args.expect_positional(0, usage)?;
    let outcome = serde_json::from_value::<crate::capture::ingest::episode::EpisodeOutcome>(
        json!(outcome),
    )
    .map_err(|_| format!("usage: {usage} (OUTCOME is succeeded, failed, partially_verified, unknown, or abandoned)"))?;
    let evidence = args
        .value("evidence")
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let workspace = Workspace::locate()?;
    let broker = local_broker(&workspace, Clock::system())?;
    let policy = local_policy(&workspace, &broker)?;
    let mut store = broker.store;
    store.record_outcome(
        episode,
        &policy.actor,
        policy.may_review,
        outcome,
        evidence,
        crate::platform::now_millis(),
    )?;
    store.save()?;
    println!("Work episode {episode}: {outcome:?}");
    Ok(())
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
pub(super) fn local_broker(
    workspace: &Workspace,
    clock: Clock,
) -> Result<EvidenceQueryBroker, String> {
    let repository = repository_label(workspace);
    let runtime = workspace.trust()?.runtime();
    let store = CaptureStore::open(&runtime.join("capture"))?;
    let mut registry = ResourceRegistry::default();
    let mut sources: Vec<LocalSource> = vec![
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
            Box::new(DataSeaConnector::new(EventStore::open(&runtime))),
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
            Box::new(CraneConnector::new(workspace.clone())),
        ),
    ];
    // GitHub is read only for a repository connected to it ('crane repo'), through the GitHub CLI
    if crate::github::load(workspace).ok().flatten().is_some() {
        for (id, resource_type, evidence_types) in [
            (
                "github",
                ResourceType::Github,
                vec![EvidenceType::PullRequest, EvidenceType::CodeChange],
            ),
            ("github-checks", ResourceType::Ci, vec![EvidenceType::CiRun]),
        ] {
            if let Some(connector) = GitHubConnector::for_remote(&repository, Box::new(GhCli)) {
                sources.push((
                    id,
                    resource_type,
                    "github",
                    evidence_types,
                    vec!["commit", "pull_request"],
                    Box::new(connector),
                ));
            }
        }
    }
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
                priority: 0,
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
        println!("GitHub pull requests and check runs are read when this repository is connected to GitHub ('crane repo'); deployments, logs, and metrics have no live connector yet, and 'crane capture demo' exercises them from fixtures.");
    }
    Ok(())
}

/** Investigate a commit (and optionally a task) from the live sources and keep the pack. The
 * recipe is the one named (recipe ID, optionally version N), else the latest serving the question
 * class (class CLASS), else local-change v1; episode EPI continues an earlier work episode
 * Input
    - args: &Args - commit, task, class, recipe, version, episode, and flags
 * Output
    - Result<(), String>
*/
fn investigate(args: &Args) -> Result<(), String> {
    args.expect_positional(
        0,
        "crane capture investigate [commit SHA] [task TASK_ID] [class CLASS] [recipe ID [version N]] [episode EPI] [--json]",
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
    let policy = local_policy(&workspace, &broker)?;
    let task = args.value("task");
    let version = args
        .value("version")
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| format!("version must be a number, not '{value}'"))
        })
        .transpose()?;
    let (class, recipe) = match (args.value("recipe"), args.value("class")) {
        (Some(id), class) => (
            class.unwrap_or_else(|| id.clone()),
            Some(RecipeRef { id, version }),
        ),
        (None, Some(class)) => (class, None),
        (None, None) => (
            "local-change".to_string(),
            Some(RecipeRef {
                id: "local-change".into(),
                version: Some(1),
            }),
        ),
    };
    let request = EvidenceQueryRequest {
        episode_id: args.value("episode"),
        task_id: task.clone(),
        question: format!(
            "What changed in {} and under which governance?",
            &commit[..commit.len().min(12)]
        ),
        investigation_class: class,
        recipe,
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
        plan_required: Vec::new(),
    };
    let result = broker.investigate(request, &policy)?;
    broker.store.save()?;
    note_investigation(&result);
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
