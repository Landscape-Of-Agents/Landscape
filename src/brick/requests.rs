// Brick change requests made on the DataSea dashboard. The dashboard holds no signing key, so it
// never changes Bricks itself: it keeps a request (an operation, its parameters, and a reason) that
// a person applies on a machine, with one click in Landscape or 'crane brick inbox apply', where
// Crane runs the same 'crane brick' command a person would type, with every check and signature it
// makes there. This module is the one list of what may be requested, used twice: the service
// checks a request before it keeps it (and which GitHub role it needs), and the machine checks it
// again before it builds the command, so a request the service let through is never trusted.
// Drafts grant nothing, so anyone who can write to the repository may request them; the rest are
// signed and need a repository admin (or a code owner of the signed file). Flow requests ('flow-…')
// run 'crane flow' commands the same way. Text that looks like it holds a secret is refused.

use serde_json::{json, Map, Value};

use super::types::BrickType;

/** The longest reason (characters) */
pub(crate) const REASON_LIMIT: usize = 300;

/** The longest context text (bytes) */
pub(crate) const TEXT_LIMIT: usize = 64 * 1024;

/** The largest draft (bytes of JSON) */
pub(crate) const DRAFT_LIMIT: usize = 96 * 1024;

/** The GitHub role a request needs
 * Variants
    - Writer - drafts and migration (unsigned files that grant nothing): push, maintain, or admin
    - Admin - signed changes: repository admin
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    Writer,
    Admin,
}

impl Role {
    /** The role as a word
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
    pub(crate) fn name(self) -> &'static str {
        match self {
            Role::Writer => "writer",
            Role::Admin => "admin",
        }
    }
}

/** What may be requested: (operation, role, what it does) */
pub(crate) const OPERATIONS: &[(&str, Role, &str)] = &[
    ("create", Role::Writer, "Create a draft"),
    (
        "edit",
        Role::Writer,
        "Edit a draft (or start the next version)",
    ),
    (
        "migrate",
        Role::Writer,
        "Draft the new kinds for Bricks of former kinds",
    ),
    ("stack-create", Role::Writer, "Draft a Stack"),
    (
        "import",
        Role::Writer,
        "Draft a Brick from the organization's catalog",
    ),
    (
        "activate",
        Role::Admin,
        "Activate a draft (sign it as a version)",
    ),
    ("deprecate", Role::Admin, "Deprecate a version"),
    ("revoke", Role::Admin, "Revoke a version"),
    ("attach", Role::Admin, "Attach to a Zone or Flow"),
    ("detach", Role::Admin, "Detach"),
    (
        "upgrade",
        Role::Admin,
        "Move an attachment to another version",
    ),
    (
        "context-set",
        Role::Admin,
        "Replace an attachment's Current Context",
    ),
    ("assign", Role::Admin, "Assign an agent to Zones or Flows"),
    ("unassign", Role::Admin, "Withdraw an agent's assignment"),
    ("stack-activate", Role::Admin, "Activate a Stack draft"),
    ("stack-revoke", Role::Admin, "Revoke a Stack version"),
    (
        "publish",
        Role::Admin,
        "Publish a version to the organization's catalog",
    ),
    // Flows (crane flow): drafts and watchers need write access; signed knowledge needs an admin
    (
        "flow-trace",
        Role::Writer,
        "Trace a flow from a starting point into a draft",
    ),
    (
        "flow-retrace",
        Role::Writer,
        "Trace a flow again (a new draft revision)",
    ),
    (
        "flow-rewind",
        Role::Writer,
        "Make an earlier draft revision current",
    ),
    (
        "flow-watch-add",
        Role::Writer,
        "Start a read-only watcher on a flow",
    ),
    ("flow-watch-stop", Role::Writer, "Stop a watcher"),
    (
        "flow-watch-edit",
        Role::Admin,
        "Change a watcher's purpose or checks",
    ),
    (
        "flow-approve",
        Role::Admin,
        "Approve a traced flow into the knowledge base",
    ),
    ("flow-discard", Role::Admin, "Discard a trace draft"),
    (
        "flow-priority",
        Role::Admin,
        "Set a flow's priority and model routing",
    ),
    ("flow-context-add", Role::Admin, "Add context to a flow"),
    (
        "flow-context-remove",
        Role::Admin,
        "Remove context from a flow",
    ),
    (
        "flow-person-add",
        Role::Admin,
        "Designate a person on a flow",
    ),
    (
        "flow-person-remove",
        Role::Admin,
        "Remove a designated person",
    ),
    (
        "flow-test-require",
        Role::Admin,
        "Require a flow's test in every release (or not)",
    ),
    (
        "flow-proposal-approve",
        Role::Admin,
        "Approve an agent's proposal",
    ),
    (
        "flow-proposal-reject",
        Role::Admin,
        "Reject an agent's proposal",
    ),
    (
        "flow-limit",
        Role::Admin,
        "Change the watcher limits agents work within",
    ),
];

/** The role an operation needs
 * Input
    - op: &str - operation
 * Output
    - Option<Role>, None when it cannot be requested
*/
pub(crate) fn role(op: &str) -> Option<Role> {
    OPERATIONS
        .iter()
        .find(|(name, _, _)| *name == op)
        .map(|(_, role, _)| *role)
}

/** Refuse text that looks like it holds a secret
 * Input
    - what: &str - what the text is, for the message
    - text: &str - text
 * Output
    - Result<(), String>
*/
fn no_secret(what: &str, text: &str) -> Result<(), String> {
    if crate::telemetry::redact::redact(text).1 > 0 {
        return Err(format!(
            "{what} looks like it holds a secret; keep secrets in the machine's vault ('crane secret set') and name them instead"
        ));
    }
    Ok(())
}

/** A name of a Brick, Stack, attachment, agent, Zone, or Flow
 * Input
    - what: &str - what it names
    - text: &str - name
 * Output
    - Result<String, String>
*/
fn name(what: &str, text: &str) -> Result<String, String> {
    crate::platform::validate_name(what, text)?;
    Ok(text.to_string())
}

/** An exact version, "ID@N" (with "stack:" when stacks is true and it is given)
 * Input
    - what: &str - what it names
    - text: &str - reference
    - stacks: bool - a Stack may be named
 * Output
    - Result<String, String>
*/
fn version(what: &str, text: &str, stacks: bool) -> Result<String, String> {
    let bare = match text.strip_prefix("stack:") {
        Some(rest) if stacks => rest,
        Some(_) => return Err(format!("{what} must be a Brick version, ID@N")),
        None => text,
    };
    let (id, number) = bare
        .split_once('@')
        .ok_or_else(|| format!("{what} must name an exact version, ID@N"))?;
    name(what, id)?;
    if number.is_empty() || number.len() > 6 || !number.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("{what} must name an exact version, ID@N"));
    }
    Ok(text.to_string())
}

/** A Zone, Flow, or runbook ("zone:ID", "flow:ID", "runbook:ID")
 * Input
    - text: &str - target
 * Output
    - Result<String, String>
*/
fn target(text: &str) -> Result<String, String> {
    let (kind, id) = text
        .split_once(':')
        .filter(|(kind, _)| ["zone", "flow", "runbook"].contains(kind))
        .ok_or_else(|| format!("'{text}' is not a target (zone:ID, flow:ID, or runbook:ID)"))?;
    name(kind, id)?;
    Ok(text.to_string())
}

/** Check a draft: only the fields 'crane brick create --file' reads, a current kind, bounded size
 * Input
    - draft: &Value - the draft
    - editing: bool - an edit (the type may be left out)
 * Output
    - Result<Value, String> the draft with only those fields
*/
fn draft(draft: &Value, editing: bool) -> Result<Value, String> {
    let object = draft.as_object().ok_or("the draft must be a JSON object")?;
    let mut kept = Map::new();
    for (key, value) in object {
        match key.as_str() {
            "type" => {
                let text = value.as_str().ok_or("type must be text")?;
                BrickType::parse(text)?;
            }
            "brick_id" => {
                name("Brick", value.as_str().ok_or("brick_id must be text")?)?;
            }
            "name" | "description" | "intent" | "owner" => {
                if !value.is_string() && !value.is_null() {
                    return Err(format!("{key} must be text"));
                }
            }
            "departments" | "dependencies" => {
                if !value
                    .as_array()
                    .is_some_and(|items| items.iter().all(Value::is_string))
                {
                    return Err(format!("{key} must be a list of text"));
                }
            }
            "config" => {
                if !value.is_object() {
                    return Err("config must be a JSON object".into());
                }
            }
            "common_context" => {
                let text = value["text"].as_str().unwrap_or_default();
                if text.len() > TEXT_LIMIT {
                    return Err(format!(
                        "the common context is longer than {TEXT_LIMIT} bytes"
                    ));
                }
                if !matches!(
                    value["sensitivity"].as_str(),
                    None | Some("internal" | "restricted")
                ) {
                    return Err("sensitivity is internal or restricted".into());
                }
            }
            other => return Err(format!("a draft has no field '{other}'")),
        }
        kept.insert(key.clone(), value.clone());
    }
    if !editing && !kept.contains_key("type") {
        return Err("a new draft needs its type".into());
    }
    if !editing
        && kept
            .get("name")
            .and_then(Value::as_str)
            .is_none_or(|text| text.trim().is_empty())
    {
        return Err("a new draft needs a name".into());
    }
    let kept = Value::Object(kept);
    let text = serde_json::to_string(&kept).map_err(|error| error.to_string())?;
    if text.len() > DRAFT_LIMIT {
        return Err(format!("the draft is larger than {DRAFT_LIMIT} bytes"));
    }
    no_secret("the draft", &text)?;
    Ok(kept)
}

/** Check a request and keep only what its operation reads
 * Input
    - op: &str - operation (see OPERATIONS)
    - params: &Value - its parameters
    - reason: &str - why (required)
 * Output
    - Result<Value, String> the parameters to keep
*/
pub(crate) fn check(op: &str, params: &Value, reason: &str) -> Result<Value, String> {
    if role(op).is_none() {
        return Err(format!("'{op}' cannot be requested"));
    }
    let reason = reason.trim();
    if reason.is_empty() || reason.chars().count() > REASON_LIMIT {
        return Err(format!(
            "say why in 1 to {REASON_LIMIT} characters; it is recorded with the change"
        ));
    }
    no_secret("the reason", reason)?;
    let text = |key: &str| params[key].as_str().unwrap_or_default().trim().to_string();
    let context = |key: &str| -> Result<String, String> {
        let value = params[key].as_str().unwrap_or_default().to_string();
        if value.len() > TEXT_LIMIT {
            return Err(format!("the context is longer than {TEXT_LIMIT} bytes"));
        }
        no_secret("the context", &value)?;
        Ok(value)
    };
    Ok(match op {
        "create" => json!({"draft": draft(&params["draft"], false)?}),
        "edit" => {
            json!({"brick_id": name("Brick", &text("brick_id"))?, "draft": draft(&params["draft"], true)?})
        }
        "migrate" => json!({}),
        "import" => {
            let (id, version) = super::share::reference(&text("ref"))?;
            let as_id = text("as");
            if !as_id.is_empty() {
                name("Brick", &as_id)?;
            }
            json!({"ref": format!("org:{id}@{version}"), "as": as_id})
        }
        "activate" => json!({"brick_id": name("Brick", &text("brick_id"))?}),
        "deprecate" | "revoke" | "publish" => {
            json!({"ref": version("the version", &text("ref"), false)?})
        }
        "attach" => {
            json!({"subject": version("the Brick or Stack", &text("subject"), true)?, "target": target(&text("target"))?, "context": context("context")?})
        }
        "detach" => json!({"attachment": name("attachment", &text("attachment"))?}),
        "upgrade" => {
            json!({"attachment": name("attachment", &text("attachment"))?, "to": version("the new version", &text("to"), true)?})
        }
        "context-set" => {
            json!({"attachment": name("attachment", &text("attachment"))?, "text": context("text")?})
        }
        "assign" => {
            let targets = params["targets"]
                .as_array()
                .filter(|items| !items.is_empty() && items.len() <= 32)
                .ok_or("name 1 to 32 targets")?
                .iter()
                .map(|item| target(item.as_str().unwrap_or_default()))
                .collect::<Result<Vec<_>, _>>()?;
            json!({"agent": name("agent", &text("agent"))?, "targets": targets, "paused": params["paused"].as_bool().unwrap_or(false)})
        }
        "unassign" => json!({"agent": name("agent", &text("agent"))?}),
        "stack-create" => {
            let stack_name = text("name");
            if stack_name.is_empty() || stack_name.chars().count() > 120 {
                return Err("a Stack needs a name of up to 120 characters".into());
            }
            let members = params["members"]
                .as_array()
                .filter(|items| items.len() >= 2 && items.len() <= 32)
                .ok_or("a Stack combines 2 to 32 Brick versions")?
                .iter()
                .map(|item| version("a member", item.as_str().unwrap_or_default(), false))
                .collect::<Result<Vec<_>, _>>()?;
            let id = text("id");
            if !id.is_empty() {
                name("Stack", &id)?;
            }
            let purpose = text("purpose");
            no_secret("the purpose", &purpose)?;
            json!({"name": stack_name, "members": members, "id": id, "purpose": purpose.chars().take(REASON_LIMIT).collect::<String>()})
        }
        "stack-activate" => json!({"stack_id": name("Stack", &text("stack_id"))?}),
        "stack-revoke" => {
            json!({"ref": version("the Stack version", text("ref").trim_start_matches("stack:"), false)?})
        }
        flow if flow.starts_with("flow-") => flow_check(flow, params)?,
        _ => unreachable!("every operation is listed"),
    })
}

/** The 'crane brick' command that carries out a request, after checking it again
 * Input
    - op: &str - operation
    - params: &Value - its parameters
    - reason: &str - the reason recorded with the request
    - origin: &str - who asked and the request id ("@login on DataSea, brq_…"), added to the reason
    - file: &str - a file the command may read (a draft or context text)
 * Output
    - Result<(Vec<String>, Option<String>), String> arguments after "crane", and what to write
      to the file first (None when the command reads no file)
*/
pub(crate) fn command(
    op: &str,
    params: &Value,
    reason: &str,
    origin: &str,
    file: &str,
) -> Result<(Vec<String>, Option<String>), String> {
    let params = check(op, params, reason)?;
    let text = |key: &str| params[key].as_str().unwrap_or_default().to_string();
    let why = format!("{} (requested by {origin})", reason.trim());
    let owned = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_string())
            .collect::<Vec<_>>()
    };
    let mut written = None;
    let mut args = match op {
        "create" => {
            written =
                Some(serde_json::to_string_pretty(&params["draft"]).map_err(|e| e.to_string())?);
            owned(&["brick", "create", "--file", file])
        }
        "edit" => {
            written =
                Some(serde_json::to_string_pretty(&params["draft"]).map_err(|e| e.to_string())?);
            owned(&["brick", "edit", &text("brick_id"), "--file", file])
        }
        "migrate" => owned(&["brick", "migrate"]),
        "import" => {
            let mut words = owned(&["brick", "import", &text("ref")]);
            if !text("as").is_empty() {
                words.extend(owned(&["--as", &text("as")]));
            }
            words
        }
        "activate" => owned(&["brick", "activate", &text("brick_id"), "--reason", &why]),
        "deprecate" | "revoke" | "publish" => owned(&["brick", op, &text("ref"), "--reason", &why]),
        "attach" => {
            let mut words = owned(&[
                "brick",
                "attach",
                &text("subject"),
                "--target",
                &text("target"),
            ]);
            if !text("context").is_empty() {
                written = Some(text("context"));
                words.extend(owned(&["--context-file", file]));
            }
            words
        }
        "detach" => owned(&["brick", "detach", &text("attachment"), "--reason", &why]),
        "upgrade" => owned(&[
            "brick",
            "upgrade",
            &text("attachment"),
            "--to",
            &text("to"),
            "--reason",
            &why,
        ]),
        "context-set" => {
            written = Some(text("text"));
            owned(&[
                "brick",
                "context",
                "set",
                &text("attachment"),
                "--file",
                file,
            ])
        }
        "assign" => {
            let targets = params["targets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            let mut words = owned(&["brick", "assign", &text("agent"), "--targets", &targets]);
            if params["paused"].as_bool() == Some(true) {
                words.push("--paused".into());
            }
            words
        }
        "unassign" => owned(&["brick", "unassign", &text("agent"), "--reason", &why]),
        "stack-create" => {
            let members = params["members"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            let mut words = owned(&[
                "brick",
                "stack",
                "create",
                "--name",
                &text("name"),
                "--members",
                &members,
            ]);
            if !text("id").is_empty() {
                words.extend(owned(&["--id", &text("id")]));
            }
            if !text("purpose").is_empty() {
                words.extend(owned(&["--purpose", &text("purpose")]));
            }
            words
        }
        "stack-activate" => owned(&[
            "brick",
            "stack",
            "activate",
            &text("stack_id"),
            "--reason",
            &why,
        ]),
        "stack-revoke" => owned(&["brick", "stack", "revoke", &text("ref"), "--reason", &why]),
        flow if flow.starts_with("flow-") => flow_command(flow, &params, &why),
        _ => unreachable!("check() refuses the rest"),
    };
    args.extend(owned(&["--format", "json"]));
    Ok((args, written))
}

/** A whole number of a request within bounds
 * Input
    - params: &Value - parameters
    - key: &str - field
    - low, high: u64 - bounds
 * Output
    - Result<Option<u64>, String> (None when absent)
*/
fn bounded(params: &Value, key: &str, low: u64, high: u64) -> Result<Option<u64>, String> {
    match &params[key] {
        Value::Null => Ok(None),
        Value::String(text) if text.trim().is_empty() => Ok(None),
        value => value
            .as_u64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
            .filter(|number| (low..=high).contains(number))
            .map(Some)
            .ok_or_else(|| format!("{key} is a whole number from {low} to {high}")),
    }
}

/** A short text of a request: trimmed, at most a length, no secret
 * Input
    - params: &Value - parameters
    - key: &str - field
    - limit: usize - most characters
    - required: bool - it may not be empty
 * Output
    - Result<String, String>
*/
fn short(params: &Value, key: &str, limit: usize, required: bool) -> Result<String, String> {
    let text = params[key].as_str().unwrap_or_default().trim().to_string();
    if required && text.is_empty() {
        return Err(format!("{key} is required"));
    }
    if text.chars().count() > limit {
        return Err(format!("{key} is longer than {limit} characters"));
    }
    no_secret(key, &text)?;
    Ok(text)
}

/** Check a flow request and keep only what its command reads
 * Input
    - op: &str - a flow-… operation
    - params: &Value - parameters
 * Output
    - Result<Value, String>
*/
fn flow_check(op: &str, params: &Value) -> Result<Value, String> {
    let flow = || name("Flow", params["flow"].as_str().unwrap_or_default());
    let handle = |key: &str| -> Result<String, String> {
        let text = params[key].as_str().unwrap_or_default().trim().to_string();
        if text.len() > 120
            || text.chars().any(|c| c.is_whitespace() || c.is_control())
            || text.starts_with('-')
        {
            return Err(format!("{key} is one handle or key without spaces"));
        }
        Ok(text)
    };
    Ok(match op {
        "flow-trace" => {
            let depth = bounded(params, "depth", 1, crate::flow::trace::MAX_DEPTH as u64)?;
            let from = crate::flow::trace::check_starts(
                &[params["from"].as_str().unwrap_or_default().to_string()],
                depth.unwrap_or(crate::flow::trace::DEFAULT_DEPTH as u64) as usize,
            )?;
            let id = params["flow"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !id.is_empty() {
                name("Flow", &id)?;
            }
            json!({"from": from.join(","), "flow": id, "depth": depth, "description": short(params, "description", REASON_LIMIT, false)?})
        }
        "flow-retrace" | "flow-approve" | "flow-discard" => {
            json!({"flow": flow()?, "proposed": params["proposed"].as_bool().unwrap_or(false)})
        }
        "flow-rewind" => {
            json!({"flow": flow()?, "revision": bounded(params, "revision", 1, 1_000_000)?.ok_or("name the revision")?})
        }
        "flow-priority" => {
            let priority = params["priority"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string();
            if priority != "none" {
                crate::flow::model::Priority::parse(&priority)?;
            }
            let enforce = params["enforce_model"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if !["", "on", "off"].contains(&enforce.as_str()) {
                return Err("enforce_model is on or off".into());
            }
            json!({"flow": flow()?, "priority": priority.to_ascii_uppercase().replace("NONE", "none"), "enforce_model": enforce})
        }
        "flow-context-add" => {
            let kind = crate::flow::model::ContextKind::parse(
                params["kind"].as_str().unwrap_or_default(),
            )?;
            let value = params["value"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string();
            if value.len() > TEXT_LIMIT {
                return Err(format!("the value is longer than {TEXT_LIMIT} bytes"));
            }
            no_secret("the value", &value)?;
            let file = params["file"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .replace('\\', "/");
            if !file.is_empty()
                && (file.starts_with('/')
                    || file.contains(':')
                    || file.split('/').any(|part| part == "..")
                    || file.starts_with(".crane/")
                    || file.starts_with('-'))
            {
                return Err(format!("'{file}' is not a file of the repository"));
            }
            if value.is_empty() && file.is_empty() {
                return Err("give a value or a file".into());
            }
            let threshold = match &params["threshold"] {
                Value::Null => None,
                Value::String(text) if text.trim().is_empty() => None,
                other => Some(
                    other
                        .as_f64()
                        .or_else(|| other.as_str().and_then(|text| text.trim().parse().ok()))
                        .ok_or("the threshold is a number")?,
                ),
            };
            let lines = short(params, "lines", 20, false)?;
            if !lines.is_empty() && !lines.chars().all(|c| c.is_ascii_digit() || c == '-') {
                return Err("lines is A-B".into());
            }
            json!({"flow": flow()?, "kind": kind.name(), "title": short(params, "title", 200, true)?, "value": value, "unit": short(params, "unit", 20, false)?, "threshold": threshold, "file": file, "lines": lines})
        }
        "flow-context-remove" => {
            json!({"flow": flow()?, "item": name("item", params["item"].as_str().unwrap_or_default())?})
        }
        "flow-person-add" => {
            crate::flow::model::Role::parse(params["role"].as_str().unwrap_or_default())?;
            json!({"flow": flow()?, "name": short(params, "name", 120, true)?, "role": params["role"].as_str().unwrap_or_default().trim().to_ascii_lowercase(), "github": handle("github")?, "jira": handle("jira")?, "slack": handle("slack")?})
        }
        "flow-person-remove" => {
            json!({"flow": flow()?, "person": name("person", params["person"].as_str().unwrap_or_default())?})
        }
        "flow-test-require" => {
            json!({"flow": flow()?, "test": name("test", params["test"].as_str().unwrap_or_default())?, "required": params["required"].as_bool().unwrap_or(true)})
        }
        "flow-proposal-approve" | "flow-proposal-reject" => {
            let proposal = name("proposal", params["proposal"].as_str().unwrap_or_default())?;
            if !proposal.starts_with("fpr_") {
                return Err("name a flow proposal (fpr_…)".into());
            }
            json!({"proposal": proposal, "required": params["required"].as_bool().unwrap_or(false)})
        }
        "flow-limit" => {
            let id = params["flow"]
                .as_str()
                .unwrap_or_default()
                .trim()
                .to_string();
            if !id.is_empty() {
                name("Flow", &id)?;
            }
            let watchers = bounded(params, "watchers", 0, 50)?;
            let session = bounded(params, "session", 0, 50)?;
            let per_flow = bounded(params, "per_flow", 0, 50)?;
            if watchers.is_some() && id.is_empty() {
                return Err("a flow's watcher limit needs the flow".into());
            }
            if watchers.is_none() && session.is_none() && per_flow.is_none() {
                return Err("change at least one limit".into());
            }
            json!({"flow": id, "watchers": watchers, "session": session, "per_flow": per_flow})
        }
        "flow-watch-add" => {
            let checks = params["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            if let Some(bad) = checks
                .iter()
                .find(|check| !crate::flow::watch::CHECKS.contains(&check.as_str()))
            {
                return Err(format!(
                    "check '{bad}' is one of {}",
                    crate::flow::watch::CHECKS.join(", ")
                ));
            }
            let subtype = crate::flow::subagent::parse_subtype(
                params["subtype"].as_str().unwrap_or_default(),
            )?;
            json!({"flow": flow()?, "purpose": short(params, "purpose", REASON_LIMIT, true)?, "checks": checks, "subtype": subtype, "config": subagent_config(params)?})
        }
        "flow-watch-stop" => {
            json!({"watcher": name("watcher", params["watcher"].as_str().unwrap_or_default())?})
        }
        "flow-watch-edit" => {
            let checks = params["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>();
            if let Some(bad) = checks
                .iter()
                .find(|check| !crate::flow::watch::CHECKS.contains(&check.as_str()))
            {
                return Err(format!(
                    "check '{bad}' is one of {}",
                    crate::flow::watch::CHECKS.join(", ")
                ));
            }
            let config = subagent_config(params)?;
            let purpose = short(params, "purpose", REASON_LIMIT, false)?;
            if checks.is_empty() && config.is_null() && purpose.is_empty() {
                return Err("change the purpose, the checks, or the configuration".into());
            }
            json!({
                "watcher": name("watcher", params["watcher"].as_str().unwrap_or_default())?,
                "purpose": purpose,
                "checks": checks,
                "config": config,
            })
        }
        _ => return Err(format!("'{op}' cannot be requested")),
    })
}

/** A subagent configuration given with a request: a JSON object (or JSON text of one), merged on
 * the machine into the current configuration, where its bounds are checked
 * Input
    - params: &Value - request parameters
 * Output
    - Result<Value, String> - the object, or null when none was given
*/
fn subagent_config(params: &Value) -> Result<Value, String> {
    let config = match &params["config"] {
        Value::Null => return Ok(Value::Null),
        Value::String(text) if text.trim().is_empty() => return Ok(Value::Null),
        Value::String(text) => serde_json::from_str(text)
            .map_err(|error| format!("the configuration is not valid JSON: {error}"))?,
        other => other.clone(),
    };
    if !config.is_object() {
        return Err("the configuration must be a JSON object".into());
    }
    if serde_json::to_vec(&config).map_or(0, |bytes| bytes.len()) > 16 * 1024 {
        return Err("the configuration is larger than 16 KB".into());
    }
    if crate::telemetry::redact::redact(&config.to_string()).1 > 0 {
        return Err(
            "the configuration looks like it holds a secret; keep credentials in 'crane integrate'"
                .into(),
        );
    }
    Ok(config)
}

/** The 'crane flow' command of a checked flow request
 * Input
    - op: &str - a flow-… operation
    - params: &Value - checked parameters
    - why: &str - the reason with who asked
 * Output
    - Vec<String> arguments after "crane"
*/
fn flow_command(op: &str, params: &Value, why: &str) -> Vec<String> {
    let text = |key: &str| params[key].as_str().unwrap_or_default().to_string();
    let words = |list: &[&str]| {
        list.iter()
            .map(|word| (*word).to_string())
            .collect::<Vec<_>>()
    };
    let mut args = match op {
        "flow-trace" => words(&["flow", "trace", "--from", &text("from")]),
        "flow-retrace" => words(&["flow", "retrace", &text("flow"), "--reason", why]),
        "flow-rewind" => words(&[
            "flow",
            "rewind",
            &text("flow"),
            "--to",
            &params["revision"].to_string(),
        ]),
        "flow-approve" => words(&["flow", "approve", &text("flow"), "--reason", why]),
        "flow-discard" => words(&["flow", "discard", &text("flow"), "--reason", why]),
        "flow-priority" => words(&["flow", "priority", &text("flow"), &text("priority")]),
        "flow-context-add" => words(&[
            "flow",
            "context",
            "add",
            &text("flow"),
            "--kind",
            &text("kind"),
            "--title",
            &text("title"),
        ]),
        "flow-context-remove" => {
            words(&["flow", "context", "remove", &text("flow"), &text("item")])
        }
        "flow-person-add" => words(&[
            "flow",
            "person",
            "add",
            &text("flow"),
            "--name",
            &text("name"),
            "--role",
            &text("role"),
        ]),
        "flow-person-remove" => {
            words(&["flow", "person", "remove", &text("flow"), &text("person")])
        }
        "flow-test-require" => words(&["flow", "test", "require", &text("flow"), &text("test")]),
        "flow-proposal-approve" => words(&["flow", "proposals", "approve", &text("proposal")]),
        "flow-proposal-reject" => words(&[
            "flow",
            "proposals",
            "reject",
            &text("proposal"),
            "--reason",
            why,
        ]),
        "flow-limit" => words(&["flow", "limit"]),
        "flow-watch-add" => words(&[
            "flow",
            "watch",
            "add",
            &text("flow"),
            "--purpose",
            &text("purpose"),
        ]),
        "flow-watch-stop" => words(&["flow", "watch", "stop", &text("watcher")]),
        "flow-watch-edit" => words(&["flow", "watch", "edit", &text("watcher")]),
        _ => Vec::new(),
    };
    if op == "flow-limit" && !text("flow").is_empty() {
        args.insert(2, text("flow"));
    }
    let mut option = |key: &str, flag: &str| {
        let value = match &params[key] {
            Value::String(text) if !text.is_empty() => Some(text.clone()),
            Value::Number(number) => Some(number.to_string()),
            _ => None,
        };
        if let Some(value) = value {
            args.push(flag.to_string());
            args.push(value);
        }
    };
    match op {
        "flow-trace" => {
            option("flow", "--flow");
            option("depth", "--depth");
            option("description", "--description");
        }
        "flow-priority" => option("enforce_model", "--enforce-model"),
        "flow-context-add" => {
            option("value", "--value");
            option("unit", "--unit");
            option("threshold", "--threshold");
            option("file", "--file");
            option("lines", "--lines");
        }
        "flow-person-add" => {
            option("github", "--github");
            option("jira", "--jira");
            option("slack", "--slack");
        }
        "flow-limit" => {
            option("watchers", "--watchers");
            option("session", "--session");
            option("per_flow", "--flow");
        }
        "flow-watch-add" | "flow-watch-edit" => {
            if op == "flow-watch-add" {
                option("subtype", "--type");
            } else {
                option("purpose", "--purpose");
            }
            let checks = params["checks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(",");
            if !checks.is_empty() {
                args.push("--checks".into());
                args.push(checks);
            }
            if params["config"].is_object() {
                args.push("--config".into());
                args.push(params["config"].to_string());
            }
        }
        _ => {}
    }
    if op == "flow-approve" && params["proposed"].as_bool() == Some(true) {
        args.push("--proposed".into());
    }
    if op == "flow-test-require" && params["required"].as_bool() == Some(false) {
        args.push("--off".into());
    }
    if op == "flow-proposal-approve" && params["required"].as_bool() == Some(true) {
        args.push("--required".into());
    }
    args
}
