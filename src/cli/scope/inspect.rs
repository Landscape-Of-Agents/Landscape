// crane scope inspect: the effective boundary of the repository: whether the signed rules verify,
// whether zones and flows resolve, how enforcement reaches agents (installed hooks; no sandbox),
// what is always denied, the governed files with their ceilings, criticality, zones, flows, and
// selections, how many source files nothing governs, and the unresolved parts (stale selectors,
// unresolved selections, overlaps). With paths, each path is resolved and its read, write, and
// delete decisions are shown.

use serde_json::{json, Value};

use super::{name, print_json};
use crate::cli::args::Args;
use crate::governance::policy::PolicySet;
use crate::governance::zones;
use crate::scope::snapshot::capture;
use crate::scope::{Access, Boundary, Request};
use crate::selection::tracker::{resolve, scan_repository, Resolution};

/** Decide read, write, and delete of one path
 * Input
    - boundary: &Boundary - the boundary
    - path: &str - path as given
 * Output
    - Result<Value, String>
*/
fn path_view(boundary: &Boundary, path: &str) -> Result<Value, String> {
    let mut access = serde_json::Map::new();
    let mut resource = Value::Null;
    let mut gaps = Vec::new();
    for kind in [Access::Read, Access::Write, Access::Delete] {
        let outcome = boundary.decide(&Request {
            access: kind,
            target: path.into(),
            lines: None,
            mode: "delegated".into(),
        })?;
        resource = json!(outcome.resource);
        gaps.extend(outcome.gaps.iter().map(|gap| json!(gap)));
        access.insert(
            kind.name().into(),
            json!({"decision": outcome.decided.verdict.decision, "rules": outcome.decided.rules, "reasons": outcome.decided.verdict.reasons}),
        );
    }
    gaps.dedup();
    let relative = resource["path"].as_str().unwrap_or_default().to_string();
    let selections = boundary
        .state
        .registry
        .selections
        .iter()
        .filter(|record| record.current.span.path == relative)
        .map(|record| json!({"id": record.id, "operation": record.operation.name(), "policy": record.policy, "start_line": record.current.span.start_line, "end_line": record.current.span.end_line}))
        .collect::<Vec<_>>();
    Ok(json!({
        "path": path,
        "resource": resource,
        "limit": boundary.enforcement.limits.get(&relative),
        "selections": selections,
        "access": access,
        "gaps": gaps,
    }))
}

/** Run the command
 * Input
    - args: &[String] - arguments after "inspect"
 * Output
    - Result<(), String>
*/
pub(super) fn inspect(args: &[String]) -> Result<(), String> {
    let args = Args::parse(args, &["skill", "format", "request-id"], &["json"])?;
    if let Some(skill) = args.value("skill") {
        return super::skill::inspect(&args, &skill);
    }
    let boundary = super::boundary()?;
    let workspace = &boundary.workspace;
    let findings = zones::load(workspace)
        .and_then(|governance| {
            zones::resolve(
                workspace,
                &governance,
                &boundary.state,
                &PolicySet::load(workspace)?,
            )
        })
        .map(|resolution| resolution.findings)
        .unwrap_or_else(|error| {
            vec![crate::governance::Finding::new(
                "governance_unreadable",
                crate::governance::Severity::High,
                error,
            )]
        });
    let scan = scan_repository(workspace)?;
    let unresolved = boundary
        .state
        .registry
        .selections
        .iter()
        .filter(|record| !matches!(resolve(&record.id, &scan), Resolution::Resolved(_)))
        .map(|record| record.id.clone())
        .collect::<Vec<_>>();
    let snapshot = capture(&boundary)?;
    let hooks = crate::hooks::install::status_checks()
        .into_iter()
        .map(|(check, status, detail)| json!({"check": check, "status": status, "detail": detail}))
        .collect::<Vec<_>>();
    let paths = args
        .positional
        .iter()
        .map(|path| path_view(&boundary, path))
        .collect::<Result<Vec<_>, _>>()?;
    let always_denied = {
        let mut denied = vec![
            ".crane (the signed rules)".to_string(),
            "the Crane trust directory".into(),
            "running crane".into(),
        ];
        denied.extend(
            boundary
                .protected
                .iter()
                .map(|path| format!("hook settings {path}")),
        );
        denied
    };
    let report = json!({
        "repository": workspace.root,
        "integrity": {"verified": boundary.integrity_ok(), "findings": boundary.enforcement.integrity},
        "versions": boundary.versions(),
        "zones_resolved": boundary.enforcement.zone_error.is_none(),
        "zone_error": boundary.enforcement.zone_error,
        "enforcement": {
            "hooks": hooks,
            "sandbox": "unavailable: Crane does not confine shell commands, processes, files outside the repository, credentials, or the network (see SECURITY.md)",
            "network": "not mediated: destinations are parsed from commands, never observed",
            "default": "files no zone or flow covers may be changed by agents within policy (ceiling delegated); preserved selections and Crane metadata stay protected everywhere",
            "always_denied": always_denied,
        },
        "governed": snapshot.files,
        "ungoverned_source_files": snapshot.ungoverned.len(),
        "unresolved_selections": unresolved,
        "findings": findings,
        "paths": paths,
    });
    if crate::cli::protocol::wants_json(&args)? {
        return print_json(&report);
    }
    println!("Scope of {}", workspace.root.display());
    if boundary.integrity_ok() {
        println!(
            "  Signed rules: verified (generation {})",
            boundary.state.generation()
        );
    } else {
        println!("  Signed rules: FAILED verification; every agent change is denied until a person runs 'crane validate'");
        for finding in &boundary.enforcement.integrity {
            println!("    - {}: {}", finding.code, finding.message);
        }
    }
    match &boundary.enforcement.zone_error {
        None => println!("  Zones and flows: resolved"),
        Some(error) => {
            println!("  Zones and flows: UNRESOLVED ({error}); every agent change is denied")
        }
    }
    println!("  Enforcement:");
    for hook in &hooks {
        println!(
            "    {} {}: {}",
            hook["status"].as_str().unwrap_or_default(),
            hook["check"].as_str().unwrap_or_default(),
            hook["detail"].as_str().unwrap_or_default()
        );
    }
    println!("    sandbox: none (shell commands, processes, files outside the repository, and the network are not confined)");
    println!("  Always denied to agents: {}", always_denied.join(", "));
    println!("  Elsewhere: agents may change files within policy (ceiling delegated) unless a zone, flow, or selection restricts them");
    println!("\nGoverned files ({})", snapshot.files.len());
    for (path, scope) in &snapshot.files {
        let mut parts = if scope.entities.is_empty() {
            vec!["no zone or flow".to_string()]
        } else if scope.block_ceiling.is_some()
            && scope.ceiling == crate::governance::zones::Ceiling::Autonomous
        {
            vec![format!("{:?}", scope.criticality).to_lowercase()]
        } else {
            vec![
                format!("ceiling {}", scope.ceiling.name()),
                format!("{:?}", scope.criticality).to_lowercase(),
            ]
        };
        if let Some(ceiling) = scope.block_ceiling {
            parts.push(format!("blocks {}", ceiling.name()));
        }
        if !scope.entities.is_empty() {
            parts.push(scope.entities.join(", "));
        }
        if !scope.preserved.is_empty() {
            parts.push(format!("preserved {}", scope.preserved.join(", ")));
        }
        if !scope.targets.is_empty() {
            parts.push(format!("target {}", scope.targets.join(", ")));
        }
        println!("  {path}: {}", parts.join("; "));
    }
    if !snapshot.files.is_empty() {
        println!(
            "Source files no zone, flow, or selection governs: {}",
            snapshot.ungoverned.len()
        );
    }
    if !unresolved.is_empty() {
        println!(
            "\nUnresolved selections (agent changes to them fail closed): {}",
            unresolved.join(", ")
        );
    }
    if !findings.is_empty() {
        println!("\nFindings");
        for finding in &findings {
            println!(
                "  [{}] {}: {}",
                name(&finding.severity),
                finding.code,
                finding.message
            );
        }
    }
    for view in &paths {
        println!("\n{}", view["path"].as_str().unwrap_or_default());
        for kind in ["read", "write", "delete"] {
            let entry = &view["access"][kind];
            let reasons = entry["reasons"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            println!(
                "  {kind}: {}{}",
                entry["decision"].as_str().unwrap_or_default(),
                if reasons.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", reasons.join("; "))
                }
            );
        }
        for gap in view["gaps"].as_array().into_iter().flatten() {
            println!(
                "  not enforced: {}",
                gap["message"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(())
}
