// crane approval: a person's decision on one exact proposed action, one file per command. An
// approval reference names what is approved: INVOCATION_ID@PLAN_DIGEST for a skill invocation, or
// RUN_ID:STEP@DIGEST for a runbook approval point. Approving binds to that digest; a changed plan
// needs a new approval. The approver is the person running Crane (never an identity flag), must not
// be the requester, and an approval never bypasses any other check.

mod approve;
mod deny;
mod request;
mod revoke;
mod status;
mod test;

/** The command reference of 'crane approval' */
const USAGE: &str = "usage: crane approval request --run INVOCATION_OR_RUN_ID [--step STEP]
       crane approval status APPROVAL_REF|INVOCATION_OR_RUN_ID
       crane approval approve APPROVAL_REF [--comment TEXT]
       crane approval deny APPROVAL_REF --reason TEXT
       crane approval revoke APPROVAL_REF --reason TEXT
       crane approval test
  (APPROVAL_REF: INVOCATION_ID@PLAN_DIGEST or RUN_ID:STEP@DIGEST; every command takes --format json)";

/** What an approval reference names
 * Variants
    - Invocation - a skill invocation and its plan digest
    - RunStep - a runbook run, a step, and the step's approval digest
*/
pub(super) enum Target {
    Invocation {
        id: String,
        digest: String,
    },
    RunStep {
        run: String,
        step: String,
        digest: String,
    },
}

/** Parse an approval reference
 * Input
    - reference: &str - APPROVAL_REF
 * Output
    - Result<Target, String>
*/
pub(super) fn target(reference: &str) -> Result<Target, String> {
    let (subject, digest) = reference
        .rsplit_once('@')
        .filter(|(subject, digest)| !subject.is_empty() && !digest.is_empty())
        .ok_or_else(|| format!("'{reference}' is not an approval reference (INVOCATION_ID@PLAN_DIGEST or RUN_ID:STEP@DIGEST); 'crane approval request' shows it"))?;
    match subject.split_once(':') {
        Some((run, step)) if run.starts_with("rbr_") => Ok(Target::RunStep {
            run: run.into(),
            step: step.into(),
            digest: digest.into(),
        }),
        None if subject.starts_with("inv_") => Ok(Target::Invocation {
            id: subject.into(),
            digest: digest.into(),
        }),
        _ => Err(format!(
            "'{reference}' names neither an invocation (inv_...) nor a runbook step (rbr_...:STEP)"
        )),
    }
}

/** Dispatch an approval command
 * Input
    - args: &[String] - arguments after "approval"
 * Output
    - Result<(), String>
*/
pub(crate) fn approval(args: &[String]) -> Result<(), String> {
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("request") => request::request(rest),
        Some("status") => status::status(rest),
        Some("approve") => approve::approve(rest),
        Some("deny") => deny::deny(rest),
        Some("revoke") => revoke::revoke(rest),
        Some("test") => test::test(rest),
        _ => Err(USAGE.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{target, Target};

    /** References name invocations or runbook steps, nothing else
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn references_parse() {
        assert!(
            matches!(target("inv_abc@d1").unwrap(), Target::Invocation { id, digest } if id == "inv_abc" && digest == "d1")
        );
        assert!(
            matches!(target("rbr_abc:refund@d2").unwrap(), Target::RunStep { run, step, digest } if run == "rbr_abc" && step == "refund" && digest == "d2")
        );
        assert!(target("inv_abc").is_err());
        assert!(target("other@d").is_err());
    }
}
