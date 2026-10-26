// crane isolation test: conformance checks of a profile, run safely. agent-hooks runs the scope
// probes (failures fail; gaps stay gaps, never passes); typed-adapters checks that no registered
// operation takes a command, script, or URL, that typed identifiers refuse shell text, and that
// secrets are recognized; sandbox is not available and is blocked.

use serde_json::json;

use crate::cli::invoke::Local;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::invocation::adapter::{validate_params, ParamKind};
use crate::scope::probe::{run, ProbeStatus};
use crate::scope::Boundary;

/** Parameter names that would let an operation carry a command
 */
const COMMAND_LIKE: &[&str] = &["command", "script", "cmd", "shell", "url", "code"];

/** Run the command
 * Input
    - args: &[String] - arguments after "test"
 * Output
    - Result<(), String>
*/
pub(super) fn test(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["profile"], &[])?;
    args.expect_positional(0, "crane isolation test --profile PROFILE")?;
    let profile = args.value("profile").ok_or("'--profile' is required")?;
    let mut cases: Vec<(String, &str)> = Vec::new();
    match profile.as_str() {
        "agent-hooks" => {
            let local = Local::open()?;
            let report = run(&Boundary::load(local.context.workspace.clone())?);
            for probe in &report.probes {
                let status = match probe.status {
                    ProbeStatus::Pass => "PASS",
                    ProbeStatus::Fail => "FAIL",
                    ProbeStatus::Gap => "GAP",
                    ProbeStatus::NotRun => "NOT_RUN",
                };
                cases.push((probe.name.clone(), status));
            }
        }
        "typed-adapters" => {
            let local = Local::open()?;
            for operation in local.adapters.operations() {
                let id = format!("{}/{}", operation.adapter_id, operation.operation_id);
                let named = operation
                    .params
                    .iter()
                    .any(|param| COMMAND_LIKE.contains(&param.name.as_str()));
                cases.push((
                    format!("{id} takes no command, script, or URL"),
                    if named { "FAIL" } else { "PASS" },
                ));
                for param in operation
                    .params
                    .iter()
                    .filter(|param| param.kind == ParamKind::Identifier)
                {
                    let shell = [(param.name.clone(), "x; rm -rf /".to_string())]
                        .into_iter()
                        .collect();
                    let refused = validate_params(&operation, &shell).is_err();
                    cases.push((
                        format!("{id} refuses shell text in {}", param.name),
                        if refused { "PASS" } else { "FAIL" },
                    ));
                }
            }
            let detected =
                crate::telemetry::redact::redact("ghp_abcdefghijklmnopqrstuvwxyz0123456789").1 > 0;
            cases.push((
                "a secret passed as an input is recognized (the engine refuses it)".into(),
                if detected { "PASS" } else { "FAIL" },
            ));
        }
        "sandbox" => cases.push((
            "the sandbox profile is not available in this version".into(),
            "BLOCKED",
        )),
        other => return Err(format!("unknown isolation profile '{other}'")),
    }
    let failed = cases.iter().filter(|(_, status)| *status == "FAIL").count();
    let gaps = cases.iter().filter(|(_, status)| *status == "GAP").count();
    let status = if cases.iter().any(|(_, status)| *status == "BLOCKED") {
        Status::Blocked
    } else if failed > 0 {
        Status::Failed
    } else {
        Status::Passed
    };
    let detail = cases
        .iter()
        .map(|(name, status)| format!("  {status:<7} {name}"))
        .collect::<Vec<_>>();
    let response = Response::new("isolation.test", status, format!("profile {profile}: {} checks, {failed} failed, {gaps} gaps (not enforced, reported)", cases.len()))
        .data(json!({"profile": profile, "cases": cases.iter().map(|(name, status)| json!({"case": name, "status": status})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}
