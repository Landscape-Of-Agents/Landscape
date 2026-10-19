// Invocation commands, under the capability group. 'plan' builds and preflights a plan without
// executing anything; 'approve' binds an approval to an exact plan digest; 'run' rechecks and
// executes an approved (or approval-free) plan; 'reconcile' resolves uncertain effects through the
// adapters' reconciliation; 'abandon' stops an invocation and records its remaining effects;
// 'inspect' and 'list' read the ledger; 'demo' runs the deterministic fixture demonstration. In a
// repository, the registered adapters are the read-only Git adapter only (no external system is
// assumed), no transaction runtime is available, lifecycle events go to DataSea's event store, and
// the ledger lives in the trust directory's runtime/invocation folder. Outcomes are printed as
// recorded; nothing is reported as successful before its verification.

use std::collections::BTreeMap;
use std::rc::Rc;

use super::args::Args;
use super::capability::{read_json, Context};
use crate::capability::refs::Sources;
use crate::capability::Role;
use crate::capability_lab::store::LabStore;
use crate::capture::resources::local::GitConnector;
use crate::compiler::assemble::CompilationReport;
use crate::discovery::report::DiscoveryStore;
use crate::invocation::adapter::{AdapterRegistry, GitReadAdapter, UnavailableRuntime};
use crate::invocation::demo;
use crate::invocation::engine::{DataSeaSink, Engine, InvocationRecord, InvocationRequest};
use crate::invocation::ledger::Ledger;
use crate::invocation::plan::{BindingSet, PlanLimits};
use crate::platform::files::read_lines;
use crate::telemetry::events::EventStore;

/** The command reference of 'crane capability invoke' */
const USAGE: &str = "usage: crane capability invoke demo [--json]
       crane capability invoke plan ID version N digest DIGEST bindings FILE key KEY [inputs FILE]
                     [environment ENV] [repository REPO] [task TASK] [preconditions ID,ID]
                     [discovery REPORT_ID] [compilation REPORT_ID] [--execute] [--json]
       crane capability invoke approve INVOCATION_ID plan-digest DIGEST [--json]
       crane capability invoke run|inspect|reconcile INVOCATION_ID [--json]
       crane capability invoke abandon INVOCATION_ID reason TEXT [--json]
       crane capability invoke list [--json]";

/** Dispatch an invoke command
 * Input
    - args: &[String] - arguments after "invoke"
 * Output
    - Result<(), String>
*/
pub(crate) fn invoke(args: &[String]) -> Result<(), String> {
    let rest = || args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("demo") => demo_command(&Args::parse(rest(), &[], &["json"])?),
        Some("plan") => plan(&Args::parse(
            rest(),
            &[
                "version",
                "digest",
                "bindings",
                "key",
                "inputs",
                "environment",
                "repository",
                "task",
                "preconditions",
                "discovery",
                "compilation",
            ],
            &["execute", "json"],
        )?),
        Some(command @ ("approve" | "run" | "inspect" | "reconcile" | "abandon")) => act(
            command,
            &Args::parse(rest(), &["plan-digest", "reason"], &["json"])?,
        ),
        Some("list") => list(&Args::parse(rest(), &[], &["json"])?),
        _ => Err(USAGE.into()),
    }
}

/** Print a record summary
 * Input
    - record: &InvocationRecord - record
 * Output
    - None
*/
fn print_record(record: &InvocationRecord) {
    println!(
        "{} {} v{} (digest {}): {:?}",
        record.invocation_id, record.capability_id, record.version, record.digest, record.state
    );
    if let Some(plan) = &record.plan {
        println!(
            "  plan {} with {} operation(s); approval {}",
            plan.plan_digest,
            plan.operations.len(),
            if record.approval.required {
                format!(
                    "required ({:?}): {}",
                    record.approval.level,
                    record.approval.reasons.join("; ")
                )
            } else {
                "not required".into()
            }
        );
    }
    for check in record.preflight.iter().filter(|check| {
        check.blocks() || check.status != crate::invocation::preflight::PreflightStatus::Passed
    }) {
        println!(
            "  preflight {} {:?}: {}",
            check.check_id, check.status, check.detail
        );
    }
    for operation in &record.operations {
        println!(
            "  op {} {}.{} {:?}, effect {:?}, attempts {}",
            operation.step_id,
            operation.adapter_id,
            operation.operation_id,
            operation.status,
            operation.effect_status,
            operation.attempts
        );
    }
    for verification in &record.verifications {
        println!(
            "  verify {} {:?} via {}",
            verification.claim_id, verification.result, verification.method
        );
    }
    for effect in &record.unresolved_effects {
        println!("  unresolved effect: {effect}");
    }
    for failure in &record.failures {
        println!("  - {failure}");
    }
}

/** Print a record as text or JSON
 * Input
    - record: &InvocationRecord - record
    - json: bool - JSON output
 * Output
    - Result<(), String>
*/
fn show(record: &InvocationRecord, json: bool) -> Result<(), String> {
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(record).map_err(|error| error.to_string())?
        );
    } else {
        print_record(record);
    }
    Ok(())
}

/** Run the demonstration
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn demo_command(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability invoke demo [--json]")?;
    let report = demo::run()?;
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    println!("Capability invocation demonstration (fixture systems; no credentials)\n");
    print_record(&report.succeeded);
    println!("  stored record verified: {}; repeated request returned the same invocation: {} (deployment.get dispatched {} time)", report.ledger_verified, report.repeat_returns_same, report.dispatches_after_repeat);
    println!(
        "\nChanged digest: {:?}; stale dependency: {:?}; revoked: {:?}",
        report.changed_digest, report.stale, report.revoked
    );
    println!(
        "Unsupported operation: {:?} with {} dispatches ({})",
        report.unsupported.0,
        report.unsupported.2,
        report.unsupported.1.join("; ")
    );
    println!("Failed verification: {:?}\n", report.failed_verification);
    print_record(&report.reconciled);
    println!("  deploy.release dispatched {} time: the timeout after its effect was reconciled, not retried", report.release_dispatches);
    Ok(())
}

/** What a repository invocation command needs
 * Fields
    - context: Context - registry, capture store, versions, actor, access
    - ledger: Ledger - invocation ledger
    - adapters: AdapterRegistry - the read-only Git adapter
    - events: DataSeaSink - DataSea event sink
    - lab, discovery, compilation - verified reports
*/
struct Local {
    context: Context,
    ledger: Ledger,
    adapters: AdapterRegistry,
    events: DataSeaSink,
    lab: Vec<crate::capability_lab::report::LabReport>,
    discovery: Vec<crate::discovery::report::DiscoveryReport>,
    compilation: Vec<CompilationReport>,
}

impl Local {
    /** Open the repository's invocation context
     * Input
        - None
     * Output
        - Result<Local, String>
    */
    fn open() -> Result<Self, String> {
        let context = Context::open()?;
        let ledger = Ledger::open(&context.runtime.join("invocation"))?;
        let adapters = AdapterRegistry::default().with(Rc::new(GitReadAdapter {
            connector: GitConnector {
                root: context.workspace.root.clone(),
            },
        }));
        let events = DataSeaSink {
            store: EventStore::open(&context.runtime),
            repository_id: context.workspace.repository_id.clone(),
        };
        let lab = LabStore::open(&context.runtime).load()?;
        let discovery = DiscoveryStore::open(&context.runtime).load()?;
        let compilation = read_lines(&context.runtime.join("compiler").join("reports.jsonl"))?
            .into_iter()
            .map(|value| {
                serde_json::from_value::<CompilationReport>(value)
                    .map_err(|error| format!("a compilation report is malformed: {error}"))
            })
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .filter(CompilationReport::verify_digest)
            .collect();
        Ok(Self {
            context,
            ledger,
            adapters,
            events,
            lab,
            discovery,
            compilation,
        })
    }
}

/** Run a closure with an engine over the local context
 * Input
    - local: &mut Local - local context
    - work: impl FnOnce(&mut Engine) -> Result<InvocationRecord, String> - work
 * Output
    - Result<InvocationRecord, String>
*/
fn with_engine(
    local: &mut Local,
    work: impl FnOnce(&mut Engine) -> Result<InvocationRecord, String>,
) -> Result<InvocationRecord, String> {
    let clock = crate::capture::Clock::system();
    let runtime = UnavailableRuntime;
    let context = &mut local.context;
    let mut engine = Engine {
        registry: &mut context.registry,
        sources: Sources {
            capture: &context.capture,
            current: &context.current,
            reader: &context.reader,
        },
        lab_reports: &local.lab,
        discovery_reports: &local.discovery,
        compilation_reports: &local.compilation,
        adapters: &local.adapters,
        runtime: &runtime,
        ledger: &mut local.ledger,
        events: &mut local.events,
        clock: &clock,
    };
    work(&mut engine)
}

/** Build (and with --execute, run when no approval is needed) a plan
 * Input
    - args: &Args - request fields
 * Output
    - Result<(), String>
*/
fn plan(args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        "crane capability invoke plan ID version N digest DIGEST bindings FILE key KEY ...",
    )?;
    let required = |key: &str| {
        args.value(key)
            .ok_or_else(|| format!("'{key}' is required\n{USAGE}"))
    };
    let bindings: BindingSet = serde_json::from_value(read_json(&required("bindings")?)?)
        .map_err(|error| format!("bindings: {error}"))?;
    let inputs: BTreeMap<String, String> = match args.value("inputs") {
        Some(path) => {
            serde_json::from_value(read_json(&path)?).map_err(|error| format!("inputs: {error}"))?
        }
        None => BTreeMap::new(),
    };
    let request = InvocationRequest {
        capability_id: args.positional[0].clone(),
        version: required("version")?
            .parse()
            .map_err(|_| "version must be a positive integer")?,
        digest: required("digest")?,
        task_id: args.value("task"),
        episode_id: None,
        environment: args.value("environment"),
        repository: args.value("repository"),
        confirmed_preconditions: args
            .value("preconditions")
            .map(|ids| {
                ids.split(',')
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        inputs,
        bindings,
        discovery_report: args.value("discovery"),
        compilation_report: args.value("compilation"),
        idempotency_key: required("key")?,
        execute: args.flag("execute"),
        limits: PlanLimits::default(),
    };
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let record = with_engine(&mut local, |engine| {
        engine.submit(&actor, request).map_err(String::from)
    })?;
    show(&record, args.flag("json"))
}

/** Approve, run, inspect, reconcile, or abandon an invocation
 * Input
    - command: &str - command
    - args: &Args - invocation id and options
 * Output
    - Result<(), String>
*/
fn act(command: &str, args: &Args) -> Result<(), String> {
    args.expect_positional(
        1,
        &format!("crane capability invoke {command} INVOCATION_ID"),
    )?;
    let id = args.positional[0].clone();
    let mut local = Local::open()?;
    let actor = local.context.actor.clone();
    let record = match command {
        "inspect" => {
            let record = local.ledger.get(&id)?;
            let reviewer = local
                .context
                .registry
                .grants
                .require(&actor, Role::Validator)
                .is_ok();
            if record.requester.id != actor.id && !reviewer {
                return Err("only the requester or a validator may inspect this invocation".into());
            }
            record
        }
        "approve" => {
            let digest = args
                .value("plan-digest")
                .ok_or("'plan-digest' is required")?;
            with_engine(&mut local, |engine| {
                engine.approve(&actor, &id, &digest).map_err(String::from)
            })?
        }
        "run" => with_engine(&mut local, |engine| {
            engine.run(&actor, &id).map_err(String::from)
        })?,
        "reconcile" => with_engine(&mut local, |engine| {
            engine.reconcile(&actor, &id).map_err(String::from)
        })?,
        _ => {
            let reason = args.value("reason").ok_or("'reason' is required")?;
            with_engine(&mut local, |engine| {
                engine.abandon(&actor, &id, &reason).map_err(String::from)
            })?
        }
    };
    show(&record, args.flag("json"))
}

/** List the invocations the actor may see (their own; all for validators)
 * Input
    - args: &Args - flags
 * Output
    - Result<(), String>
*/
fn list(args: &Args) -> Result<(), String> {
    args.expect_positional(0, "crane capability invoke list [--json]")?;
    let local = Local::open()?;
    let actor = &local.context.actor;
    let reviewer = local
        .context
        .registry
        .grants
        .require(actor, Role::Validator)
        .is_ok();
    let records = local.ledger.state.records.values().filter(|record| reviewer || record.requester.id == actor.id).map(|record| serde_json::json!({"invocation_id": record.invocation_id, "capability_id": record.capability_id, "version": record.version, "state": record.state, "updated_at": record.updated_at})).collect::<Vec<_>>();
    if args.flag("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&records).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if records.is_empty() {
        println!("No invocation.");
    }
    for record in records {
        println!(
            "{} {} v{}: {}",
            record["invocation_id"].as_str().unwrap_or_default(),
            record["capability_id"].as_str().unwrap_or_default(),
            record["version"],
            record["state"].as_str().unwrap_or_default()
        );
    }
    Ok(())
}
