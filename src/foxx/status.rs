// The repository status a machine reports to the Foxx dashboard. Most of the dashboard is built from
// uploaded events, but some views describe this machine's working tree and configuration, which
// the service cannot see: policy results against the current files, governance integrity, the
// compliance inputs that come from local verification, the GitHub connection, and the configured
// integrations (non-secret settings only). The upload sync collects this report at most every two
// minutes, or sooner when the registry generation changes, and the service keeps the latest one
// per machine and repository.

use serde_json::{json, Value};

use crate::github;
use crate::governance::state::{integrity, State};
use crate::governance::verify::{evaluate, Options, Outcome};
use crate::governance::workspace::Workspace;
use crate::hooks::adapter::AgentKind;
use crate::hooks::install;
use crate::integrations;
use crate::platform::now_millis;
use crate::platform::render::iso_time;
use crate::selection::registry::Operation;
use crate::telemetry::alerts;
use crate::telemetry::compliance::Inputs;

/** Collect this machine's status of a repository
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value {collected_at, crane_version, registry_generation, integrity, governance,
      compliance_inputs, repo, integrations}
*/
pub(crate) fn collect(workspace: &Workspace) -> Value {
    let state = State::load(workspace);
    let findings = match (&state, workspace.trust()) {
        (Ok(state), Ok(trust)) => integrity(workspace, state, &trust),
        (Err(error), _) => vec![unreadable(error.clone())],
        (_, Err(error)) => vec![unreadable(error)],
    };
    json!({
        "collected_at": now_millis(),
        "crane_version": env!("CARGO_PKG_VERSION"),
        "registry_generation": state.as_ref().ok().map(State::generation),
        "integrity": {
            "ok": !findings.iter().any(crate::governance::Finding::blocking),
            "findings": findings,
        },
        "governance": governance(workspace).unwrap_or_else(|error| json!({"error": error})),
        "compliance_inputs": compliance_inputs(workspace),
        "repo": repo(workspace),
        "integrations": integrations::configured(workspace),
    })
}

/** A finding for governance that cannot be read
 * Input
    - message: String - why
 * Output
    - Finding
*/
fn unreadable(message: String) -> crate::governance::Finding {
    crate::governance::Finding::new(
        "governance_unreadable",
        crate::governance::Severity::Critical,
        message,
    )
}

/** The governance view: policies with their results, selections with their resolution,
 * checkpoints, defaults, context bindings, findings, zones and flows, and the audit trail
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Value, String>
*/
fn governance(workspace: &Workspace) -> Result<Value, String> {
    let (report, state) = evaluate(workspace, &Options::default())?;
    let selections = state
        .registry
        .selections
        .iter()
        .map(|record| {
            json!({
                "id": record.id,
                "name": record.name,
                "operation": record.operation,
                "change_type": record.change_type,
                "policy": record.policy,
                "language": record.language,
                "status": record.status,
                "generation": record.generation,
                "origin": {"checkpoint": record.origin.checkpoint, "commit": record.origin.commit, "path": record.origin.span.path, "lines": [record.origin.span.start_line, record.origin.span.end_line], "binding_digest": record.origin.binding_digest},
                "resolved": report.resolutions.get(&record.id).cloned().flatten(),
                "created_at": iso_time(record.created_at * 1000),
                "created_by": record.created_by,
            })
        })
        .collect::<Vec<_>>();
    let contexts = state
        .context
        .bindings
        .iter()
        .map(|binding| {
            let current = std::fs::read(workspace.root.join(&binding.path))
                .ok()
                .map(|bytes| crate::trust::crypto::sha512_hex(&bytes));
            json!({"policy": binding.policy, "path": binding.path, "state": match current { None => "missing", Some(digest) if digest == binding.digest => "current", Some(_) => "stale" }})
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "generation": report.generation,
        "public_key": state.registry.manifest.as_ref().map(|manifest| manifest.public_key.clone()),
        "admins": state.config.admins,
        "defaults": {"policy": state.config.default_policy, "checkpoint": state.config.default_checkpoint},
        "autonomy": state.config.autonomy,
        "checkpoints": state.checkpoints.checkpoints.iter().map(|checkpoint| json!({"name": checkpoint.name, "commit": checkpoint.commit, "branch": checkpoint.branch, "created_at": iso_time(checkpoint.created_at * 1000), "created_by": checkpoint.created_by})).collect::<Vec<_>>(),
        "policies": report.policies,
        "selections": selections,
        "contexts": contexts,
        "findings": report.findings,
        "relocations": report.relocations,
        "passed": report.passed,
        "governance_config": crate::governance::zones::load(workspace).ok(),
        "zones_and_flows": report.zones,
        "audit": state.audit.iter().rev().take(50).map(|entry| json!({"seq": entry.seq, "at": iso_time(entry.at * 1000), "actor": entry.actor, "action": entry.action, "detail": entry.detail})).collect::<Vec<_>>(),
    }))
}

/** The compliance inputs that come from local verification (the service adds the event chain and
 * ledger results from the uploaded records)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Inputs
*/
fn compliance_inputs(workspace: &Workspace) -> Inputs {
    let mut inputs = Inputs {
        alerts_configured: alerts::configured(workspace),
        hooks_valid: [AgentKind::Claude, AgentKind::Codex]
            .into_iter()
            .any(|kind| install::validate(kind).is_ok_and(|report| report["valid"] == true)),
        ..Inputs::default()
    };
    if let Ok((report, state)) = evaluate(
        workspace,
        &Options {
            candidates: false,
            ..Options::default()
        },
    ) {
        let tally = |operation: Operation| {
            let commands = report
                .policies
                .iter()
                .flat_map(|policy| policy.commands.iter())
                .filter(|command| command.operation == operation)
                .collect::<Vec<_>>();
            Some((
                commands
                    .iter()
                    .filter(|command| command.outcome == Outcome::Pass)
                    .count(),
                commands.len(),
            ))
        };
        inputs.preserve = tally(Operation::Preserve);
        inputs.target = tally(Operation::Target);
        if let Ok(trust) = workspace.trust() {
            let findings = integrity(workspace, &state, &trust);
            inputs.integrity_findings =
                findings.iter().filter(|finding| finding.blocking()).count();
            inputs.audit_verified = !findings
                .iter()
                .any(|finding| finding.code == "audit_tampered");
            inputs.anchor = trust.anchor().ok().flatten().is_some();
        }
    }
    inputs
}

/** The repository connection without secrets
 * Input
    - workspace: &Workspace - repository
 * Output
    - Value
*/
fn repo(workspace: &Workspace) -> Value {
    let connection = github::load(workspace).ok().flatten();
    json!({
        "connection": connection.map(|connection| json!({"method": connection.method.name(), "repository": connection.normalized, "privileges": connection.privileges, "connected_at": iso_time(connection.connected_at * 1000)})),
        "github_grant_active": github::active_grant().is_some(),
    })
}
