// crane revocation revoke: revoke one typed target through the service that owns it: a skill
// version (here and in the signed shared store), a runbook version (no new runs; running runs stop
// at their next step), or an approval that was not used yet. Needs the role the owning service
// requires (publisher for skills and runbooks; the approver or a publisher for an approval).

use crate::cli::protocol::versioned;

/** Run the command
 * Input
    - args: &[String] - arguments after "revoke"
 * Output
    - Result<(), String>
*/
pub(super) fn revoke(args: &[String]) -> Result<(), String> {
    let parsed = crate::cli::protocol::parse(args, &["target", "reason"], &[])?;
    parsed.expect_positional(0, "crane revocation revoke --target skill:SKILL@N|runbook:RUNBOOK@N|approval:APPROVAL_REF --reason TEXT")?;
    let target = parsed.value("target").ok_or("'--target' is required")?;
    let reason = parsed.value("reason").ok_or("'--reason' is required")?;
    let mut forwarded = Vec::new();
    let (kind, subject) = target
        .split_once(':')
        .ok_or("'--target' must be skill:..., runbook:..., or approval:...")?;
    match kind {
        "skill" | "runbook" => {
            versioned(subject)?;
            forwarded.push(subject.to_string());
        }
        "approval" => forwarded.push(subject.to_string()),
        other => return Err(format!("unknown revocation target kind '{other}'")),
    }
    forwarded.extend(["--reason".to_string(), reason]);
    if let Some(format) = parsed.value("format") {
        forwarded.extend(["--format".to_string(), format]);
    }
    if parsed.flag("json") {
        forwarded.push("--json".into());
    }
    match kind {
        "skill" => crate::cli::skill::skill(&[vec!["revoke".to_string()], forwarded].concat()),
        "runbook" => {
            crate::cli::runbook::runbook(&[vec!["revoke".to_string()], forwarded].concat())
        }
        _ => crate::cli::approval::approval(&[vec!["revoke".to_string()], forwarded].concat()),
    }
}
