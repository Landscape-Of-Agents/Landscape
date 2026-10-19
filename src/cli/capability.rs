// Capability commands. The registry lives in the trust directory's runtime/capability folder and
// reads Work Capture records from runtime/capture. Roles are decided here, by Crane, never by the
// caller: whoever runs Crane (always a person; agents are refused by the CLI gate) may submit
// intakes, store candidates, and validate; publishing, rejecting, deprecating, and revoking need
// this machine's signing key to be the repository owner's or a signed admin's. Validation of a
// file, and validation with --dry-run, change nothing; publication and withdrawal are separate
// commands that always require a reason.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::args::Args;
use crate::capability::intake::{EnterpriseIntent, IntakeRequest, INTAKE_SCHEMA_VERSION};
use crate::capability::lifecycle::{LifecycleState, PublicationRequest};
use crate::capability::model::{parse_definition, CapabilityDefinition, RiskLevel};
use crate::capability::query::{
    self, ApplicabilityDecision, ApplicabilityMatcher, ApplicabilityQuery, ExactMatcher,
    RegistryQuery,
};
use crate::capability::refs::{CurrentVersions, Sources};
use crate::capability::registry::CapabilityRegistry;
use crate::capability::validate::{self, select_profile, Severity, ValidationRun};
use crate::capability::{demo, Grants, Role};
use crate::capture::store::CaptureStore;
use crate::capture::{AccessPolicy, Actor, ActorKind, Sensitivity};
use crate::governance::admins::signers;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::platform::{actor, now_millis};

/** The command reference of 'crane capability' */
const USAGE: &str = "usage: crane capability demo [--json]
       crane capability intake episode EPISODE pack PACK digest DIGEST [intent FILE] [risk low|medium|high|critical] [profile NAME] [--json]
       crane capability create FILE [profile NAME] [--json]
       crane capability propose FILE [profile NAME] [--json]
       crane capability validate (ID [version N] [--dry-run] | file FILE) [--json]
       crane capability inspect ID [version N] [task-class CLASS environment ENV [preconditions ID,ID]] [--json]
       crane capability list [state STATE] [task-class CLASS] [environment ENV] [--all] [--json]
       crane capability versions ID [--json]
       crane capability publish ID version N reason TEXT [acknowledge CODE,CODE] [--json]
       crane capability deprecate|revoke ID [version N] reason TEXT
       crane capability reject ID version N reason TEXT
       crane capability stale [--json]
       crane capability discover demo|task-class CLASS ...|reports [--json]
       crane capability invoke demo|plan|approve|run|inspect|list|reconcile|abandon ...";

/** Dispatch a capability command
 * Input
    - args: &[String] - arguments after "capability"
 * Output
    - Result<(), String>
*/
pub(crate) fn capability(args: &[String]) -> Result<(), String> {
    let rest = || args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(rest(), &[], &["json"])?),
        Some("intake") => intake(&Args::parse(
            rest(),
            &["episode", "pack", "digest", "intent", "risk", "profile"],
            &["json"],
        )?),
        Some(command @ ("create" | "propose")) => {
            store(command, &Args::parse(rest(), &["profile"], &["json"])?)
        }
        Some("validate") => validate_command(&Args::parse(
            rest(),
            &["version", "file"],
            &["dry-run", "json"],
        )?),
        Some("inspect") => inspect(&Args::parse(
            rest(),
            &["version", "task-class", "environment", "preconditions"],
            &["json"],
        )?),
        Some("list") => list(&Args::parse(
            rest(),
            &["state", "task-class", "environment"],
            &["all", "json"],
        )?),
        Some("versions") => versions(&Args::parse(rest(), &[], &["json"])?),
        Some("publish") => publish(&Args::parse(
            rest(),
            &["version", "reason", "acknowledge"],
            &["json"],
        )?),
        Some(command @ ("deprecate" | "revoke" | "reject")) => {
            withdraw(command, &Args::parse(rest(), &["version", "reason"], &[])?)
        }
        Some("discover") => super::discover::discover_command(rest()),
        Some("invoke") => super::invoke::invoke(rest()),
        Some("stale") => stale(&Args::parse(rest(), &[], &["json"])?),
        _ => Err(USAGE.into()),
    }
}

/** Print a value as pretty JSON
 * Input
    - value: &impl serde::Serialize - value
 * Output
    - Result<(), String>
*/
fn print_json(value: &impl serde::Serialize) -> Result<(), String> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?
    );
    Ok(())
}

/** Everything a repository command needs: the registry, the capture store, current versions, the
 * actor, and the actor's evidence access
 * Fields
    - registry: CapabilityRegistry - registry
    - capture: CaptureStore - Work Capture records
    - current: CurrentVersions - current Landscape and Crane versions
    - actor: Actor - the person running Crane
    - reader: AccessPolicy - their Work Capture access
    - runtime: PathBuf - the repository's runtime folder in the trust directory
    - workspace: Workspace - the repository
*/
pub(super) struct Context {
    pub(super) registry: CapabilityRegistry,
    pub(super) capture: CaptureStore,
    pub(super) current: CurrentVersions,
    pub(super) actor: Actor,
    pub(super) reader: AccessPolicy,
    pub(super) runtime: PathBuf,
    pub(super) workspace: Workspace,
}

impl Context {
    /** Open the repository's registry and sources, granting roles by repository authority
     * Input
        - None
     * Output
        - Result<Context, String>
    */
    pub(super) fn open() -> Result<Self, String> {
        let workspace = Workspace::locate()?;
        let trust = workspace.trust()?;
        let runtime = trust.runtime();
        let actor = Actor::new(&actor(), ActorKind::Human);
        let mut roles = vec![Role::Contributor, Role::Validator];
        if is_admin(&workspace, trust.signing_public_key()?) {
            roles.push(Role::Publisher);
        }
        let grants = Grants::default().with(&actor.id, &roles);
        let repository = super::capture::repository_label(&workspace);
        let reader = AccessPolicy {
            actor: actor.clone(),
            resources: ["git", "datasea", "landscape", "crane"]
                .iter()
                .map(|id| id.to_string())
                .collect(),
            repositories: BTreeSet::from([repository]),
            environments: ["local", "ci", "hosted"]
                .iter()
                .map(|name| name.to_string())
                .collect(),
            evidence_types: None,
            max_sensitivity: Sensitivity::Confidential,
            may_review: false,
        };
        Ok(Self {
            registry: CapabilityRegistry::open(&runtime.join("capability"), grants)?,
            capture: CaptureStore::open(&runtime.join("capture"))?,
            current: CurrentVersions::from_workspace(&workspace),
            actor,
            reader,
            runtime,
            workspace,
        })
    }

    /** The sources for validation and tracing
     * Input
        - None (uses self)
     * Output
        - Sources
    */
    fn sources(&self) -> Sources<'_> {
        Sources {
            capture: &self.capture,
            current: &self.current,
            reader: &self.reader,
        }
    }
}

/** Report whether this machine's key is the repository owner's or a signed admin's
 * Input
    - workspace: &Workspace - repository
    - key: Option<String> - this machine's public signing key
 * Output
    - bool
*/
fn is_admin(workspace: &Workspace, key: Option<String>) -> bool {
    match (key, State::load(workspace)) {
        (Some(key), Ok(state)) => signers(&state).contains(&key),
        _ => false,
    }
}

/** Read a JSON file
 * Input
    - path: &str - file
 * Output
    - Result<Value, String>
*/
pub(super) fn read_json(path: &str) -> Result<Value, String> {
    let text = std::fs::read_to_string(Path::new(path))
        .map_err(|error| format!("could not read {path}: {error}"))?;
    serde_json::from_str(&text).map_err(|error| format!("{path} is not valid JSON: {error}"))
}

/** Run the demonstration
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        return print_json(&report);
    }
    println!("Capability registry demonstration (fixtures; no live service was contacted)\n");
    println!(
        "Evidence Pack {} (digest {})",
        report.pack_id, report.pack_digest
    );
    for intake in &report.intakes {
        println!(
            "Intake {} [{}]: {}",
            intake["intake_id"].as_str().unwrap_or_default(),
            intake["profile"].as_str().unwrap_or_default(),
            intake["status"].as_str().unwrap_or_default()
        );
        for finding in intake["errors"].as_array().into_iter().flatten() {
            println!("  - {}", finding.as_str().unwrap_or_default());
        }
    }
    println!(
        "Repeated submission returned the same intake: {}",
        report.duplicate_submission
    );
    println!("\nrefund-worker-recovery v1: {:?}", report.recovery_outcome);
    for finding in &report.recovery_findings {
        println!("  - {finding}");
    }
    println!("  publication: {}", report.recovery_publication);
    println!(
        "\ndeployment-failure-diagnostics v1: {:?}, then published by a second person",
        report.diagnostics_outcome
    );
    for (version, previous, digest, state) in &report.history.versions {
        println!(
            "  v{version} (predecessor {}) {state:?} digest {digest}",
            previous.map_or("none".to_string(), |previous| format!("v{previous}"))
        );
    }
    println!(
        "  trace of v1: {} references, all resolved to Work Capture records",
        report.trace.references.len()
    );
    println!(
        "\nA claim citing an agent hypothesis is {:?}: {}",
        report.hypothesis_claim.0,
        report.hypothesis_claim.1.join("; ")
    );
    println!(
        "Latest eligible version before the zone changed: {:?}",
        report.eligible_before_change
    );
    for (id, version, reasons) in &report.stale {
        println!(
            "Stale after zone refunds moved to v4: {id} v{version}: {}",
            reasons.join("; ")
        );
    }
    println!(
        "Latest eligible version after: {:?}",
        report.eligible_after_change
    );
    Ok(())
}

/** Submit an intake
 * Input
    - args: &Args - episode, pack, digest, intent, risk, profile
 * Output
    - Result<(), String>
*/
fn intake(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability intake episode EPISODE pack PACK digest DIGEST [intent FILE] [risk RISK] [profile NAME]")?;
    let required = |key: &str| {
        args.value(key)
            .ok_or_else(|| format!("'{key}' is required\n{USAGE}"))
    };
    let intent: EnterpriseIntent = match args.value("intent") {
        Some(path) => {
            serde_json::from_value(read_json(&path)?).map_err(|error| format!("{path}: {error}"))?
        }
        None => EnterpriseIntent::default(),
    };
    let risk: RiskLevel = serde_json::from_value(json!(args
        .value("risk")
        .unwrap_or_else(|| "unknown".into())))
    .map_err(|_| "risk must be low, medium, high, critical, or unknown".to_string())?;
    let request = IntakeRequest {
        schema_version: INTAKE_SCHEMA_VERSION,
        episode_id: required("episode")?,
        pack_id: required("pack")?,
        pack_digest: required("digest")?,
        intent,
        risk,
        profile: args.value("profile"),
        idempotency_key: None,
        unresolved_assumptions: Vec::new(),
        human_interventions: Vec::new(),
    };
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let sources = Sources {
        capture: &context.capture,
        current: &context.current,
        reader: &context.reader,
    };
    let submission = context
        .registry
        .submit_intake(&request, &sources, &actor, now_millis())?;
    if args.flag("json") {
        return print_json(&submission);
    }
    let intake = &submission.intake;
    println!(
        "Intake {} {:?} under {} v{}{}",
        intake.intake_id,
        intake.status,
        intake.validation.profile_id,
        intake.validation.profile_version,
        if submission.duplicate {
            " (already submitted)"
        } else {
            ""
        }
    );
    for finding in &intake.validation.findings {
        println!(
            "  [{:?}] {}: {}",
            finding.severity, finding.code, finding.message
        );
    }
    Ok(())
}

/** Store a definition file as a new capability or a new version
 * Input
    - command: &str - create or propose
    - args: &Args - file and profile
 * Output
    - Result<(), String>
*/
fn store(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        &format!("crane capability {command} FILE [profile NAME]"),
    )?;
    let definition = parse_definition(&read_json(&args.positional[0])?)?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let profile = args.value("profile");
    let record = if command == "create" {
        context
            .registry
            .create_candidate(&actor, definition, profile.as_deref(), now_millis())?
    } else {
        context
            .registry
            .propose_version(&actor, definition, profile.as_deref(), now_millis())?
    };
    if args.flag("json") {
        return print_json(&record);
    }
    println!(
        "Stored {} v{} as a candidate under {} (digest {}); it is not validated",
        record.definition.capability_id,
        record.definition.version,
        record.profile_id,
        record.definition_digest
    );
    Ok(())
}

/** Print a validation run
 * Input
    - run: &ValidationRun - run
 * Output
    - None
*/
fn print_run(run: &ValidationRun) {
    println!(
        "{} v{}: {:?}{}",
        run.capability_id,
        run.version,
        run.outcome,
        if run.dry_run {
            " (dry run; nothing recorded)"
        } else {
            ""
        }
    );
    for gate in &run.gates {
        println!("  {:?}: {:?}", gate.gate, gate.status);
        for finding in gate
            .findings
            .iter()
            .filter(|finding| finding.severity != Severity::Info)
        {
            println!(
                "    [{:?}] {}: {}",
                finding.severity, finding.code, finding.message
            );
        }
    }
}

/** Validate a stored version (recorded unless --dry-run) or a definition file (never recorded)
 * Input
    - args: &Args - id and version, or file; flags
 * Output
    - Result<(), String>
*/
fn validate_command(args: &Args) -> Result<(), String> {
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let run = match (args.value("file"), args.positional.as_slice()) {
        (Some(path), []) => {
            let definition: CapabilityDefinition = parse_definition(&read_json(&path)?)?;
            let profile = select_profile(None, definition.applicability.risk)?;
            validate::run(
                &definition,
                &profile,
                &context.registry.state,
                &context.sources(),
                &actor,
                true,
                now_millis(),
            )
        }
        (None, [id]) => {
            let version = version_of(args, &context.registry, id)?;
            let sources = Sources {
                capture: &context.capture,
                current: &context.current,
                reader: &context.reader,
            };
            context.registry.validate(
                &actor,
                id,
                version,
                &sources,
                args.flag("dry-run"),
                now_millis(),
            )?
        }
        _ => return Err(USAGE.into()),
    };
    if args.flag("json") {
        return print_json(&run);
    }
    print_run(&run);
    Ok(())
}

/** The version a command names, or the latest
 * Input
    - args: &Args - arguments
    - registry: &CapabilityRegistry - registry
    - id: &str - capability
 * Output
    - Result<u32, String>
*/
fn version_of(args: &Args, registry: &CapabilityRegistry, id: &str) -> Result<u32, String> {
    match args.value("version") {
        Some(version) => version
            .parse()
            .map_err(|_| format!("version must be a positive integer, got '{version}'")),
        None => registry
            .state
            .versions
            .get(id)
            .and_then(|versions| versions.keys().next_back().copied())
            .ok_or_else(|| format!("capability {id} does not exist")),
    }
}

/** Inspect a version: definition, state, validation, dependencies, and provenance
 * Input
    - args: &Args - id and version
 * Output
    - Result<(), String>
*/
fn inspect(args: &Args) -> Result<(), String> {
    args.expect_positional(1, "crane capability inspect ID [version N]")?;
    let context = Context::open()?;
    let id = &args.positional[0];
    let version = version_of(args, &context.registry, id)?;
    let state = &context.registry.state;
    let record = state.record(id, version)?;
    let sources = context.sources();
    let report = json!({
        "record": record,
        "latest_run": state.latest_run(record),
        "eligible": query::eligible(record),
        "dependencies": query::dependencies(state, id, version, &sources)?,
        "provenance": query::trace(state, id, version, &sources)?,
        "applicability": applicability(args, &record.definition),
    });
    if args.flag("json") {
        return print_json(&report);
    }
    println!(
        "{} v{}: {} [{:?}] profile {}",
        id, version, record.definition.name, record.state, record.profile_id
    );
    println!(
        "  digest {} created by {} ({:?})",
        record.definition_digest,
        record.definition.provenance.created_by.id,
        record.definition.provenance.creation_method
    );
    println!("  eligible for new use: {}", query::eligible(record));
    for note in &record.stale {
        println!("  stale: {}", note.reasons.join("; "));
    }
    for dependency in report["dependencies"].as_array().into_iter().flatten() {
        println!(
            "  dependency {} {}: {}",
            dependency["dependency"]["kind"]
                .as_str()
                .unwrap_or_default(),
            dependency["dependency"]["target_id"]
                .as_str()
                .unwrap_or_default(),
            dependency["outcome"].as_str().unwrap_or_default()
        );
    }
    if let Some(decision) = applicability(args, &record.definition) {
        println!(
            "  applicable to the given situation: {:?} ({})",
            decision.applicable,
            decision.reasons.join("; ")
        );
    }
    for reference in report["provenance"]["references"]
        .as_array()
        .into_iter()
        .flatten()
    {
        println!(
            "  provenance {} {}: {}",
            reference["kind"].as_str().unwrap_or_default(),
            reference["reference"].as_str().unwrap_or_default(),
            reference["outcome"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}

/** Evaluate applicability when the command describes a situation
 * Input
    - args: &Args - task-class, environment, preconditions
    - definition: &CapabilityDefinition - definition
 * Output
    - Option<ApplicabilityDecision>, None when no situation was given
*/
fn applicability(args: &Args, definition: &CapabilityDefinition) -> Option<ApplicabilityDecision> {
    if args.value("task-class").is_none() && args.value("environment").is_none() {
        return None;
    }
    let situation = ApplicabilityQuery {
        task_class: args.value("task-class"),
        environment: args.value("environment"),
        satisfied_preconditions: args
            .value("preconditions")
            .map(|ids| ids.split(',').map(|id| id.trim().to_string()).collect())
            .unwrap_or_default(),
        entities: Vec::new(),
    };
    Some(ExactMatcher.evaluate(definition, &situation))
}

/** List capabilities
 * Input
    - args: &Args - filters
 * Output
    - Result<(), String>
*/
fn list(args: &Args) -> Result<(), String> {
    args.expect_positional(
        0,
        "crane capability list [state STATE] [task-class CLASS] [environment ENV] [--all]",
    )?;
    let context = Context::open()?;
    let state = match args.value("state") {
        Some(state) => Some(
            serde_json::from_value::<LifecycleState>(json!(state))
                .map_err(|_| format!("unknown state '{state}'"))?,
        ),
        None => None,
    };
    let filter = RegistryQuery {
        state,
        task_class: args.value("task-class"),
        environment: args.value("environment"),
        include_withdrawn: args.flag("all"),
        ..RegistryQuery::default()
    };
    let summaries = query::list(&context.registry.state, &filter);
    if args.flag("json") {
        return print_json(&summaries);
    }
    if summaries.is_empty() {
        println!("No capabilities match.");
    }
    for summary in summaries {
        let versions = summary
            .versions
            .iter()
            .map(|(version, state)| format!("v{version} {state:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        println!(
            "{} - {} [{}] eligible: {}",
            summary.capability_id,
            summary.name,
            versions,
            summary
                .latest_eligible
                .map_or("none".to_string(), |version| format!("v{version}"))
        );
    }
    Ok(())
}

/** Show a capability's version history
 * Input
    - args: &Args - id
 * Output
    - Result<(), String>
*/
fn versions(args: &Args) -> Result<(), String> {
    args.expect_positional(1, "crane capability versions ID")?;
    let context = Context::open()?;
    let history = query::history(&context.registry.state, &args.positional[0])?;
    if args.flag("json") {
        return print_json(&history);
    }
    for (version, previous, digest, state) in &history.versions {
        println!(
            "v{version} {state:?} digest {digest} predecessor {}",
            previous.map_or("none".to_string(), |previous| format!("v{previous}"))
        );
    }
    for transition in &history.transitions {
        println!(
            "  v{} {:?} -> {:?} by {}: {}",
            transition.version,
            transition.from,
            transition.to,
            transition.actor.id,
            transition.reason
        );
    }
    Ok(())
}

/** Publish a version
 * Input
    - args: &Args - id, version, reason, acknowledged warnings
 * Output
    - Result<(), String>
*/
fn publish(args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        "crane capability publish ID version N reason TEXT [acknowledge CODE,CODE]",
    )?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let request = PublicationRequest {
        capability_id: args.positional[0].clone(),
        version: args
            .value("version")
            .ok_or("'version' is required")?
            .parse()
            .map_err(|_| "version must be a positive integer")?,
        reason: args
            .value("reason")
            .filter(|reason| !reason.trim().is_empty())
            .ok_or("'reason' is required")?,
        acknowledged_warnings: args
            .value("acknowledge")
            .map(|codes| {
                codes
                    .split(',')
                    .map(|code| code.trim().to_string())
                    .filter(|code| !code.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
    };
    let gate = context.registry.publish(&actor, &request, now_millis())?;
    if args.flag("json") {
        return print_json(&gate);
    }
    println!("Published {} v{}", request.capability_id, request.version);
    Ok(())
}

/** Deprecate, revoke, or reject
 * Input
    - command: &str - deprecate, revoke, or reject
    - args: &Args - id, version, reason
 * Output
    - Result<(), String>
*/
fn withdraw(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        &format!("crane capability {command} ID [version N] reason TEXT"),
    )?;
    let to = match command {
        "deprecate" => LifecycleState::Deprecated,
        "revoke" => LifecycleState::Revoked,
        _ => LifecycleState::Rejected,
    };
    let version = args
        .value("version")
        .map(|version| {
            version
                .parse::<u32>()
                .map_err(|_| "version must be a positive integer".to_string())
        })
        .transpose()?;
    if to == LifecycleState::Rejected && version.is_none() {
        return Err("reject needs 'version N'".into());
    }
    let reason = args.value("reason").ok_or("'reason' is required")?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let changed = context.registry.withdraw(
        &actor,
        &args.positional[0],
        version,
        to,
        &reason,
        now_millis(),
    )?;
    println!(
        "{} {:?}: {}",
        args.positional[0],
        to,
        changed
            .iter()
            .map(|version| format!("v{version}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

/** Mark validations made stale by changed references
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn stale(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability stale [--json]")?;
    let mut context = Context::open()?;
    let actor = context.actor.clone();
    let sources = Sources {
        capture: &context.capture,
        current: &context.current,
        reader: &context.reader,
    };
    let marked = context
        .registry
        .mark_stale(&actor, &sources, now_millis())?;
    if args.flag("json") {
        return print_json(&marked);
    }
    if marked.is_empty() {
        println!("No validation became stale.");
    }
    for (id, version, reasons) in marked {
        println!("{id} v{version} needs revalidation: {}", reasons.join("; "));
    }
    Ok(())
}
