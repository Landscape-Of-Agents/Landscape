// Brick change requests made on the DataSea dashboard. The dashboard holds no signing key, so it
// never changes Bricks itself: it keeps a request (an operation, its parameters, and a reason) that
// a person applies on a machine, with one click in Landscape or 'crane brick inbox apply', where
// Crane runs the same 'crane brick' command a person would type, with every check and signature it
// makes there. This module is the one list of what may be requested, used twice: the service
// checks a request before it keeps it (and which GitHub role it needs), and the machine checks it
// again before it builds the command, so a request the service let through is never trusted.
// Drafts grant nothing, so anyone who can write to the repository may request them; the rest are
// signed and need a repository admin. Text that looks like it holds a secret is refused.

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
        _ => unreachable!("check() refuses the rest"),
    };
    args.extend(owned(&["--format", "json"]));
    Ok((args, written))
}
