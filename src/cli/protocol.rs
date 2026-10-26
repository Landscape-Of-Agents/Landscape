// The shared result model of the command families (scope, permission, precondition, constraint,
// approval, isolation, verification, provenance, budget, recovery, revocation, selection, skill,
// runbook). Every command builds one Response; '--format json' (or '--json') prints it as is, and
// the human view is rendered from the same object, never from a second decision path.
//
// Exit codes are stable:
//   0 - ok or passed
//   1 - usage or internal error (the existing convention of every Crane command)
//   3 - denied, failed, or blocked
//   4 - a person's approval is required
//   5 - unknown or unresolved (never silently treated as success)
// A non-zero result is returned as an error carrying a marker that main() turns into the code,
// the same way hooks::outcome carries the hooks' blocking exit code 2.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::Value;

use super::args::Args;
use crate::telemetry::model::Decision;

/** Version of the response layout */
pub(crate) const RESPONSE_SCHEMA_VERSION: u32 = 1;

/** Marker at the start of an error that carries an exit code */
const EXIT_PREFIX: &str = "CRANE_EXIT:";

/** Keys every family command accepts besides its own */
pub(crate) const COMMON_KEYS: &[&str] = &["format", "request-id"];

/** Flags every family command accepts besides its own */
pub(crate) const COMMON_FLAGS: &[&str] = &["json"];

/** The outcome class of a command
 * Variants
    - Ok - done, or nothing to object to
    - Passed - every required check passed
    - Failed - a required check failed
    - Denied - the requested action is not allowed
    - Blocked - it cannot proceed until something changes (revoked, stale, missing)
    - ApprovalRequired - a person must approve first
    - Unknown - the outcome cannot be established
    - Unresolved - required facts are missing or ambiguous
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Status {
    Ok,
    Passed,
    Failed,
    Denied,
    Blocked,
    ApprovalRequired,
    Unknown,
    Unresolved,
}

impl Status {
    /** The process exit code of the status
     * Input
        - None (uses self)
     * Output
        - i32
    */
    pub(crate) fn exit_code(self) -> i32 {
        match self {
            Self::Ok | Self::Passed => 0,
            Self::Failed | Self::Denied | Self::Blocked => 3,
            Self::ApprovalRequired => 4,
            Self::Unknown | Self::Unresolved => 5,
        }
    }

    /** The status a decision implies
     * Input
        - decision: Decision - authority decision
     * Output
        - Status
    */
    pub(crate) fn of_decision(decision: Decision) -> Self {
        match decision {
            Decision::Allow => Self::Ok,
            Decision::RequireApproval => Self::ApprovalRequired,
            Decision::Deny | Decision::Quarantine => Self::Denied,
            Decision::Error => Self::Unknown,
        }
    }
}

/** The result of a family command
 * Fields
    - schema_version: u32 - RESPONSE_SCHEMA_VERSION
    - command: String - family and command, such as "skill.inspect"
    - request_id: String - given with --request-id, or derived
    - status: Status - outcome class
    - decision: Option<Decision> - authority decision, where one was made
    - reason_codes: Vec<String> - machine-readable reasons
    - summary: String - one line for people
    - refs: BTreeMap<String, String> - references (skill_ref, run_ref, policy_digest, ...); only
      those that apply, never fabricated
    - data: Value - the command's typed payload
    - created_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, Serialize)]
pub(crate) struct Response {
    pub(crate) schema_version: u32,
    pub(crate) command: String,
    pub(crate) request_id: String,
    pub(crate) status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decision: Option<Decision>,
    pub(crate) reason_codes: Vec<String>,
    pub(crate) summary: String,
    pub(crate) refs: BTreeMap<String, String>,
    pub(crate) data: Value,
    pub(crate) created_at: u64,
}

impl Response {
    /** Start a response
     * Input
        - command: &str - "family.command"
        - status: Status - outcome class
        - summary: impl Into<String> - one line for people
     * Output
        - Response
    */
    pub(crate) fn new(command: &str, status: Status, summary: impl Into<String>) -> Self {
        let created_at = crate::platform::now_millis();
        Self {
            schema_version: RESPONSE_SCHEMA_VERSION,
            command: command.into(),
            request_id: format!(
                "req_{}",
                &crate::trust::crypto::digest(
                    "cli/request/v1",
                    &[
                        command.as_bytes(),
                        &created_at.to_be_bytes(),
                        &std::process::id().to_be_bytes()
                    ]
                )[..20]
            ),
            status,
            decision: None,
            reason_codes: Vec::new(),
            summary: summary.into(),
            refs: BTreeMap::new(),
            data: Value::Null,
            created_at,
        }
    }

    /** Set the decision (and the status it implies)
     * Input
        - decision: Decision - decision
     * Output
        - Response
    */
    pub(crate) fn decision(mut self, decision: Decision) -> Self {
        self.decision = Some(decision);
        self.status = Status::of_decision(decision);
        self
    }

    /** Add reason codes
     * Input
        - codes: impl IntoIterator<Item = impl Into<String>> - codes
     * Output
        - Response
    */
    pub(crate) fn reasons(mut self, codes: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.reason_codes.extend(codes.into_iter().map(Into::into));
        self
    }

    /** Add a reference
     * Input
        - name: &str - reference name, such as "skill_ref"
        - value: impl Into<String> - value
     * Output
        - Response
    */
    pub(crate) fn reference(mut self, name: &str, value: impl Into<String>) -> Self {
        self.refs.insert(name.into(), value.into());
        self
    }

    /** Set the payload
     * Input
        - data: Value - payload
     * Output
        - Response
    */
    pub(crate) fn data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }
}

/** Report whether JSON output was asked for (--json or --format json)
 * Input
    - args: &Args - parsed arguments
 * Output
    - Result<bool, String>
    - Error for a format other than text or json
*/
pub(crate) fn wants_json(args: &Args) -> Result<bool, String> {
    match args.value("format").as_deref() {
        None => Ok(args.flag("json")),
        Some("json") => Ok(true),
        Some("text") => Ok(args.flag("json")),
        Some(other) => Err(format!("format must be text or json, got '{other}'")),
    }
}

/** Parse a family command's arguments with the common keys and flags added
 * Input
    - args: &[String] - arguments after the command words
    - keys: &[&str] - the command's own keys
    - flags: &[&str] - the command's own flags
 * Output
    - Result<Args, String>
*/
pub(crate) fn parse(args: &[String], keys: &[&str], flags: &[&str]) -> Result<Args, String> {
    let keys = keys.iter().chain(COMMON_KEYS).copied().collect::<Vec<_>>();
    let flags = flags
        .iter()
        .chain(COMMON_FLAGS)
        .copied()
        .collect::<Vec<_>>();
    Args::parse(args, &keys, &flags)
}

/** Print a response (JSON, or the human view with optional detail lines) and turn a non-zero
 * status into the error that carries its exit code
 * Input
    - args: &Args - parsed arguments (format and request id)
    - response: Response - result
    - detail: &[String] - extra human lines below the summary
 * Output
    - Result<(), String>
*/
pub(crate) fn finish(args: &Args, mut response: Response, detail: &[String]) -> Result<(), String> {
    if let Some(id) = args.value("request-id") {
        response.request_id = id;
    }
    // Each reason once, in the order it was first given
    let mut seen = std::collections::BTreeSet::new();
    response
        .reason_codes
        .retain(|code| seen.insert(code.clone()));
    if wants_json(args)? {
        println!(
            "{}",
            serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?
        );
    } else {
        let decision = response
            .decision
            .map(|decision| format!(" {}", name(&decision)))
            .unwrap_or_default();
        println!(
            "[{}{decision}] {}",
            name(&response.status),
            response.summary
        );
        for line in detail {
            println!("{line}");
        }
        if !response.reason_codes.is_empty() {
            println!("reasons: {}", response.reason_codes.join(", "));
        }
    }
    match response.status.exit_code() {
        0 => Ok(()),
        code => Err(format!("{EXIT_PREFIX}{code}:{}", response.summary)),
    }
}

/** Read the exit code and message of an error that carries one
 * Input
    - error: &str - error message
 * Output
    - Option<(i32, &str)>
*/
pub(crate) fn exit(error: &str) -> Option<(i32, &str)> {
    let rest = error.strip_prefix(EXIT_PREFIX)?;
    let (code, message) = rest.split_once(':')?;
    Some((code.parse().ok()?, message))
}

/** The serialized name of an enumeration value
 * Input
    - value: &impl Serialize - value
 * Output
    - String
*/
pub(crate) fn name(value: &impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/** Split "ID@VERSION" (or "ID@vVERSION", or "ID") into the id and an optional version
 * Input
    - reference: &str - reference
 * Output
    - Result<(String, Option<u32>), String>
*/
pub(crate) fn versioned(reference: &str) -> Result<(String, Option<u32>), String> {
    match reference.split_once('@') {
        None if !reference.is_empty() => Ok((reference.into(), None)),
        Some((id, version)) if !id.is_empty() => version
            .trim_start_matches('v')
            .parse::<u32>()
            .ok()
            .filter(|version| *version > 0)
            .map(|version| (id.to_string(), Some(version)))
            .ok_or_else(|| {
                format!("'{reference}' must be ID or ID@VERSION with a positive version")
            }),
        _ => Err(format!("'{reference}' must be ID or ID@VERSION")),
    }
}

#[cfg(test)]
mod tests {
    use super::{exit, versioned, Response, Status};
    use crate::telemetry::model::Decision;

    /** Exit codes are stable, carried through the error marker, and versions parse
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn exit_codes_and_references() {
        assert_eq!(Status::Passed.exit_code(), 0);
        assert_eq!(Status::Denied.exit_code(), 3);
        assert_eq!(Status::ApprovalRequired.exit_code(), 4);
        assert_eq!(Status::Unknown.exit_code(), 5);
        let response = Response::new("permission.check", Status::Ok, "x").decision(Decision::Deny);
        assert_eq!(response.status, Status::Denied);
        assert_eq!(
            exit("CRANE_EXIT:4:needs approval"),
            Some((4, "needs approval"))
        );
        assert_eq!(exit("plain error"), None);
        assert_eq!(versioned("refund@3").unwrap(), ("refund".into(), Some(3)));
        assert_eq!(versioned("refund@v2").unwrap(), ("refund".into(), Some(2)));
        assert_eq!(versioned("refund").unwrap(), ("refund".into(), None));
        assert!(versioned("refund@0").is_err());
        assert!(versioned("@1").is_err());
    }
}
