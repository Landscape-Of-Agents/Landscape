// crane provenance export: a minimal lineage bundle (records are already redacted) with a manifest
// digest and the verification of every link, to an approved destination only: stdout, or the
// repository's runtime folder (runtime/exports). Never an arbitrary path or URL. The export is
// recorded as an event.

use serde_json::json;

use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::platform::files::write_atomic;
use crate::telemetry::events::{Event, EventStore};
use crate::telemetry::model::Domain;
use crate::trust::crypto::digest_json;

/** Run the command
 * Input
    - args: &[String] - arguments after "export"
 * Output
    - Result<(), String>
*/
pub(super) fn export(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["run", "receipt", "destination"], &[])?;
    args.expect_positional(
        0,
        "crane provenance export --run INVOCATION_OR_RUN_ID --destination stdout|runtime",
    )?;
    let run = args
        .value("run")
        .or_else(|| args.value("receipt"))
        .ok_or("'--run' is required")?;
    let destination = args
        .value("destination")
        .ok_or("'--destination' is required (stdout or runtime)")?;
    if !matches!(destination.as_str(), "stdout" | "runtime") {
        return Err(format!(
            "'{destination}' is not an approved destination; use stdout or runtime"
        ));
    }
    let local = Local::open()?;
    let (runbook_run, invocations) = super::records(&local, &run)?;
    let checks = invocations
        .iter()
        .flat_map(|record| super::checks(&local, record))
        .collect::<Vec<_>>();
    let mut bundle = json!({
        "schema": "crane-provenance/v1",
        "run": run,
        "runbook_run": runbook_run.as_ref().map(crate::cli::family::run_view),
        "invocations": invocations.iter().map(super::lineage).collect::<Vec<_>>(),
        "checks": checks.iter().map(|(name, holds, detail)| json!({"check": name, "holds": holds, "detail": detail})).collect::<Vec<_>>(),
    });
    let manifest = digest_json("provenance/bundle/v1", &bundle)[..32].to_string();
    bundle["manifest_digest"] = json!(manifest);
    let written = if destination == "runtime" {
        let path = local
            .context
            .runtime
            .join("exports")
            .join(format!("{run}-{manifest}.json"));
        write_atomic(
            &path,
            serde_json::to_string_pretty(&bundle)
                .map_err(|error| error.to_string())?
                .as_bytes(),
        )?;
        Some(path.display().to_string())
    } else {
        None
    };
    let mut event = Event::new(
        "provenance.exported",
        Domain::AuditCompliance,
        &local.context.workspace.repository_id,
        &format!("provenance:{run}:{manifest}:{destination}"),
    );
    event.payload = json!({"run": run, "manifest_digest": manifest, "destination": destination});
    let _ = EventStore::open(&local.context.runtime).append(event);
    let broken = checks.iter().filter(|(_, holds, _)| !holds).count();
    let response = Response::new(
        "provenance.export",
        if broken == 0 {
            Status::Ok
        } else {
            Status::Failed
        },
        format!(
            "exported {run} (manifest {manifest}{}){}",
            written
                .as_deref()
                .map(|path| format!(", {path}"))
                .unwrap_or_default(),
            if broken == 0 {
                String::new()
            } else {
                format!("; {broken} links are broken")
            }
        ),
    )
    .reference("run_ref", run.clone())
    .reference("provenance_ref", manifest)
    .data(if destination == "stdout" {
        bundle
    } else {
        json!({"path": written})
    });
    finish(&args, response, &[])
}
