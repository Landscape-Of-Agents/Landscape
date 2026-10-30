// crane flow: flows as a knowledge base. A person traces a flow from a starting point into a draft
// (revisions: retrace, rewind), reviews it, and approves it, which adds the flow to
// .crane/governance.yaml and records the trace in the signed flow knowledge (.crane/flows.json).
// The knowledge holds typed context (queries, metrics, latency, files, notes, links), a priority
// that routes work to the organization's model tiers, the designated people, the approved tests
// (some required in every release: 'crane flow tests run --required'), and the hard limits on the
// read-only watchers agents start. Agents ask through their hooks ('crane-request', flow/agent.rs);
// what they propose waits here for a person ('crane flow proposals'). Every change of the knowledge
// is signed with an admin key; reading and tracing are not.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::cli::args::Args;
use crate::cli::protocol::{finish, parse, Response, Status};
use crate::flow::model::{
    route, ContextItem, ContextKind, FlowBook, FlowTest, ModelEntry, Person, Priority, Role, Tier,
    MAX_CONTEXT, MAX_PEOPLE, MAX_TESTS, MAX_TEXT,
};
use crate::flow::watch::{self, Proposal, Watcher};
use crate::flow::{store, trace};
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::trust::require_human;

/** A change of the flow knowledge, decided before it is signed */
type Change = Box<dyn FnOnce(&mut FlowBook) -> Result<(), String>>;

/** The usage of 'crane flow' */
pub(crate) const USAGE: &str = "usage:
  crane flow list | show ID
  crane flow trace --from SYMBOL[,SYMBOL] [--flow ID] [--depth N] [--description TEXT]
  crane flow retrace ID [--reason TEXT] | rewind ID --to N | draft ID [--revision N]
  crane flow approve ID --reason TEXT [--proposed] | discard ID --reason TEXT
  crane flow priority ID P0|P1|P2|P3|none [--enforce-model on|off]
  crane flow context add ID --kind query|metric|latency|file|note|link --title TEXT [--value TEXT]
        [--unit U] [--threshold N] [--file PATH] [--lines A-B] [--id ITEM] | context remove ID ITEM
  crane flow person add ID --name NAME --role author|owner|reviewer|oncall [--id HANDLE]
        [--github LOGIN] [--jira KEY] [--slack ID] [--source manual|git|jira|slack] | person remove ID PERSON
  crane flow people suggest ID                (the Git authors of the flow's files)
  crane flow test add ID --file PATH --name TEXT --description TEXT [--assumptions 'A; B']
        [--mock TEXT] [--command TEXT] [--required] | test require ID TEST [--off] | test remove ID TEST
  crane flow tests run [ID] [--required]      (runs the tests' commands; fails when a required one fails)
  crane flow proposals [list] | proposals approve PROPOSAL [--required] | proposals reject PROPOSAL --reason TEXT
  crane flow watch list | watch add ID --purpose TEXT [--checks LIST] | watch stop WATCHER | watch run
        | watch edit WATCHER [--purpose TEXT] [--checks LIST]
  crane flow incidents
  crane flow limit ID --watchers N | limit --session N
  crane flow models [list] | models add --provider P --model M --tier frontier|balanced|fast
        | models remove MODEL | models available MODEL on|off | route ID
  crane flow routing P0|P1|P2|P3 TIER[,TIER] | routing P0|P1|P2|P3 default
  (agents ask through their hooks: 'crane-request help')";

/** Run 'crane flow'
 * Input
    - args: &[String] - arguments after "flow"
 * Output
    - Result<(), String>
*/
pub(crate) fn flow(args: &[String]) -> Result<(), String> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    let rest = |count: usize| args.get(count..).unwrap_or_default();
    match words.as_slice() {
        ["list", ..] => list(rest(1)),
        ["show", ..] => show(rest(1)),
        ["trace", ..] => start_trace(rest(1)),
        ["retrace", ..] => retrace(rest(1)),
        ["rewind", ..] => rewind(rest(1)),
        ["draft", ..] => draft(rest(1)),
        ["approve", ..] => approve(rest(1)),
        ["discard", ..] => discard(rest(1)),
        ["priority", ..] => priority(rest(1)),
        ["context", "add", ..] => context_add(rest(2)),
        ["context", "remove", ..] => context_remove(rest(2)),
        ["person", "add", ..] => person_add(rest(2)),
        ["person", "remove", ..] => person_remove(rest(2)),
        ["people", "suggest", ..] => people_suggest(rest(2)),
        ["test", "add", ..] => test_add(rest(2)),
        ["test", "require", ..] => test_require(rest(2)),
        ["test", "remove", ..] => test_remove(rest(2)),
        ["tests", "run", ..] => tests_run(rest(2)),
        ["proposals", "approve", ..] => proposal_decide(rest(2), true),
        ["proposals", "reject", ..] => proposal_decide(rest(2), false),
        ["proposals", ..] => proposals(
            rest(1)
                .get(usize::from(rest(1).first().map(String::as_str) == Some("list"))..)
                .unwrap_or_default(),
        ),
        ["watch", "list", ..] => watch_list(rest(2)),
        ["watch", "add", ..] => watch_add(rest(2)),
        ["watch", "edit", ..] => watch_edit(rest(2)),
        ["watch", "stop", ..] => watch_stop(rest(2)),
        ["watch", "run", ..] => watch_run(rest(2)),
        ["incidents", ..] => incidents(rest(1)),
        ["limit", ..] => limit(rest(1)),
        ["models", "add", ..] => models_add(rest(2)),
        ["models", "remove", ..] => models_remove(rest(2)),
        ["models", "available", ..] => models_available(rest(2)),
        ["models", "route", ..] => route_flow(rest(2)),
        ["models", ..] => models_list(
            rest(1)
                .get(usize::from(rest(1).first().map(String::as_str) == Some("list"))..)
                .unwrap_or_default(),
        ),
        ["routing", ..] => routing(rest(1)),
        _ => Err(USAGE.into()),
    }
}

/** The repository, its signed state, and its flow knowledge
 * Input
    - None
 * Output
    - Result<(Workspace, State, FlowBook), String>
*/
fn open() -> Result<(Workspace, State, FlowBook), String> {
    let workspace = Workspace::locate()?;
    let state = State::load(&workspace)?;
    let book = FlowBook::of(&state)?;
    Ok((workspace, state, book))
}

/** Check that a flow is declared in .crane/governance.yaml
 * Input
    - workspace: &Workspace - repository
    - id: &str - flow id
 * Output
    - Result<(), String>
*/
fn declared(workspace: &Workspace, id: &str) -> Result<(), String> {
    let governance = crate::governance::zones::load(workspace)?;
    if governance.flows.iter().any(|flow| flow.id == id) {
        Ok(())
    } else {
        Err(format!(
            "{id} is not a flow of .crane/governance.yaml; trace it first ('crane flow trace --from SYMBOL --flow {id}') and approve it"
        ))
    }
}

/** A required text option, trimmed and bounded, refused when it looks like a secret
 * Input
    - args: &Args - parsed arguments
    - key: &str - option
    - hint: &str - what to give
 * Output
    - Result<String, String>
*/
fn text(args: &Args, key: &str, hint: &str) -> Result<String, String> {
    let value = args
        .value(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("--{key} is required ({hint})"))?;
    checked(key, &value)?;
    Ok(value)
}

/** Refuse a text that is too long or looks like it holds a secret
 * Input
    - what: &str - field
    - value: &str - text
 * Output
    - Result<(), String>
*/
fn checked(what: &str, value: &str) -> Result<(), String> {
    if value.len() > MAX_TEXT {
        return Err(format!("{what} is longer than {MAX_TEXT} bytes"));
    }
    if crate::telemetry::redact::redact(value).1 > 0 {
        return Err(format!(
            "{what} looks like it holds a secret; secrets never go into flows"
        ));
    }
    Ok(())
}

/** A repository file: relative, inside it, not Crane's metadata, and existing
 * Input
    - workspace: &Workspace - repository
    - raw: &str - path
 * Output
    - Result<String, String>
*/
fn repository_file(workspace: &Workspace, raw: &str) -> Result<String, String> {
    let path = raw.trim().replace('\\', "/");
    let path = path.trim_start_matches("./").to_string();
    if path.is_empty()
        || path.starts_with('/')
        || path.contains(':')
        || path.split('/').any(|part| part == "..")
        || path.starts_with(".crane/")
    {
        return Err(format!("'{raw}' is not a file of the repository"));
    }
    if !workspace.root.join(&path).is_file() {
        return Err(format!("{path} does not exist"));
    }
    Ok(path)
}

/** An identifier within a flow: the one given, else made from a title, unique among the taken
 * Input
    - given: Option<String> - --id
    - title: &str - title to make it from
    - taken: &BTreeSet<String> - identifiers in use
 * Output
    - Result<String, String>
*/
fn item_id(given: Option<String>, title: &str, taken: &BTreeSet<String>) -> Result<String, String> {
    let base = match given {
        Some(id) => {
            crate::platform::validate_name("item", &id)?;
            if taken.contains(&id) {
                return Err(format!("{id} is taken; choose another --id"));
            }
            return Ok(id);
        }
        None => crate::flow::agent::slug(title),
    };
    let base = base.chars().take(48).collect::<String>();
    let mut id = base.clone();
    let mut number = 2;
    while taken.contains(&id) {
        id = format!("{base}-{number}");
        number += 1;
    }
    Ok(id)
}

/** Sign a change of the flow knowledge and answer with its summary
 * Input
    - args: &Args - parsed arguments (output format)
    - workspace: &Workspace - repository
    - command: &str - the command (audited)
    - name: &str - response command name
    - detail: Value - audit detail
    - summary: String - the answer
    - change: impl FnOnce(&mut FlowBook) -> Result<(), String> - the change
 * Output
    - Result<(), String>
*/
fn signed(
    args: &Args,
    workspace: &Workspace,
    command: &str,
    name: &str,
    detail: Value,
    summary: String,
    change: impl FnOnce(&mut FlowBook) -> Result<(), String>,
) -> Result<(), String> {
    let book = store::change(workspace, command, detail.clone(), change)?;
    crate::platform::notes::note(
        "FLOW_CHANGED",
        json!({"flow": detail["flow"], "change": name}),
    );
    let response = Response::new(name, Status::Ok, summary)
        .data(json!({"change": detail, "flows": book.flows.len()}));
    finish(args, response, &[])
}

/** Run 'crane flow list'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let (workspace, _, book) = open()?;
    let governance = crate::governance::zones::load(&workspace)?;
    let (drafts, _) = trace::drafts(&workspace);
    let rows = governance
        .flows
        .iter()
        .map(|flow| {
            let knowledge = book.flows.get(&flow.id).cloned().unwrap_or_default();
            json!({
                "id": flow.id, "description": flow.description, "entry_points": flow.entry_points,
                "priority": knowledge.priority.map(Priority::name), "traced": knowledge.trace.is_some(),
                "people": knowledge.people.len(), "context": knowledge.context.len(),
                "tests": knowledge.tests.len(), "required_tests": knowledge.tests.iter().filter(|test| test.required).count(),
            })
        })
        .collect::<Vec<_>>();
    let detail = rows
        .iter()
        .map(|row| {
            format!(
                "  {} [{}] {} · {} people, {} context, {} tests ({} required){}",
                row["id"].as_str().unwrap_or_default(),
                row["priority"].as_str().unwrap_or("—"),
                row["description"].as_str().unwrap_or_default(),
                row["people"],
                row["context"],
                row["tests"],
                row["required_tests"],
                if row["traced"] == true {
                    " · traced"
                } else {
                    ""
                }
            )
        })
        .chain(drafts.iter().map(|draft| {
            format!(
                "  draft {} revision {} of {} (by {})",
                draft.flow_id,
                draft.current,
                draft.revisions.len(),
                draft.requested_by
            )
        }))
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.list",
        Status::Ok,
        format!("{} flow(s), {} trace draft(s)", rows.len(), drafts.len()),
    )
    .data(json!({"flows": rows, "drafts": drafts.iter().map(|draft| json!({"flow_id": draft.flow_id, "current": draft.current, "revisions": draft.revisions.len(), "requested_by": draft.requested_by, "via": draft.via})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}

/** Run 'crane flow show ID'
 * Input
    - args: &[String] - arguments after "show"
 * Output
    - Result<(), String>
*/
fn show(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane flow show ID")?;
    let id = &args.positional[0];
    let (workspace, _, book) = open()?;
    declared(&workspace, id)?;
    let governance = crate::governance::zones::load(&workspace)?;
    let runtime = workspace.trust()?.runtime();
    let text = crate::flow::context::text(&runtime, &book, &governance, id, None);
    let response = Response::new("flow.show", Status::Ok, format!("flow {id}"))
        .data(json!({"flow": id, "knowledge": book.flows.get(id), "text": text}));
    finish(
        &args,
        response,
        &text
            .lines()
            .map(|line| format!("  {line}"))
            .collect::<Vec<_>>(),
    )
}

/** A revision's summary lines
 * Input
    - revision: &trace::Revision - revision
 * Output
    - Vec<String>
*/
fn revision_lines(revision: &trace::Revision) -> Vec<String> {
    let mut lines = vec![format!(
        "  revision {} ({}): {} functions, {} calls, {} files, digest {}",
        revision.revision,
        revision.graph.status,
        revision.graph.nodes.len(),
        revision.graph.edges.len(),
        revision.graph.files.len(),
        &revision.digest[..16.min(revision.digest.len())]
    )];
    for node in revision.graph.nodes.iter().take(40) {
        lines.push(format!(
            "    {}{} {}:{}-{}",
            "  ".repeat(node.depth.min(8)),
            node.symbol,
            node.path,
            node.start_line,
            node.end_line
        ));
    }
    if revision.graph.nodes.len() > 40 {
        lines.push(format!("    … {} more", revision.graph.nodes.len() - 40));
    }
    for reason in revision.graph.unresolved.iter().take(10) {
        lines.push(format!("    not followed: {reason}"));
    }
    lines
}

/** Run 'crane flow trace'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn start_trace(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["from", "flow", "depth", "description"], &[])?;
    args.expect_positional(
        0,
        "crane flow trace --from SYMBOL [--flow ID] [--depth N] [--description TEXT]",
    )?;
    let workspace = Workspace::locate()?;
    let from = args
        .value("from")
        .ok_or("--from is required: where the flow starts (SYMBOL, Type.SYMBOL, or path:SYMBOL)")?;
    let depth = args
        .value("depth")
        .map(|text| {
            text.parse::<usize>()
                .map_err(|_| "--depth is a number".to_string())
        })
        .transpose()?
        .unwrap_or(trace::DEFAULT_DEPTH);
    let starts = trace::check_starts(&[from], depth)?;
    let id = args
        .value("flow")
        .unwrap_or_else(|| crate::flow::agent::slug(&starts[0]));
    crate::platform::validate_name("Flow", &id)?;
    let description = args.value("description").unwrap_or_default();
    checked("--description", &description)?;
    let graph = trace::trace(&workspace, &starts, depth)?;
    if graph.status == "UNRESOLVED" {
        return Err(format!(
            "the trace could not start: {}",
            graph.unresolved.join("; ")
        ));
    }
    let draft = trace::add_revision(
        trace::load(&workspace, &id)?,
        &id,
        &description,
        starts,
        depth,
        graph,
        &crate::platform::actor(),
        "cli",
        "traced",
        crate::platform::now_millis(),
    );
    trace::save(&workspace, &draft)?;
    let revision = draft
        .current()
        .cloned()
        .ok_or("the draft has no revision")?;
    crate::platform::notes::note(
        "FLOW_TRACED",
        json!({"flow": id, "revision": revision.revision, "nodes": revision.graph.nodes.len(), "status": revision.graph.status, "via": "cli", "digest": revision.digest}),
    );
    let response = Response::new(
        "flow.trace",
        Status::Ok,
        format!(
            "draft {id} revision {}: {} functions, {} calls ({}); review it, then 'crane flow approve {id} --reason TEXT'",
            revision.revision,
            revision.graph.nodes.len(),
            revision.graph.edges.len(),
            revision.graph.status
        ),
    )
    .reference("digest", revision.digest.clone())
    .data(json!({"draft": draft.flow_id, "revision": revision}));
    finish(&args, response, &revision_lines(&revision))
}

/** Run 'crane flow retrace ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn retrace(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane flow retrace ID [--reason TEXT]")?;
    let id = &args.positional[0];
    let workspace = Workspace::locate()?;
    // A draft continues from its current revision; an approved flow is retraced from its trace
    let (from, depth, existing) = match trace::load(&workspace, id)? {
        Some(draft) => {
            let current = draft
                .current()
                .cloned()
                .ok_or("the draft has no revision")?;
            (current.from, current.max_depth, Some(draft))
        }
        None => {
            let (_, _, book) = open()?;
            let approved = book
                .flows
                .get(id)
                .and_then(|knowledge| knowledge.trace.clone())
                .ok_or_else(|| format!("{id} has no trace draft and no approved trace"))?;
            (approved.from, approved.max_depth, None)
        }
    };
    let reason = args.value("reason").unwrap_or_else(|| "retraced".into());
    checked("--reason", &reason)?;
    let previous = existing.as_ref().and_then(|draft| draft.current().cloned());
    let graph = trace::trace(&workspace, &from, depth)?;
    let changes = previous
        .as_ref()
        .map(|previous| trace::diff(&previous.graph, &graph));
    let draft = trace::add_revision(
        existing,
        id,
        "",
        from,
        depth,
        graph,
        &crate::platform::actor(),
        "cli",
        &reason,
        crate::platform::now_millis(),
    );
    trace::save(&workspace, &draft)?;
    let revision = draft
        .current()
        .cloned()
        .ok_or("the draft has no revision")?;
    crate::platform::notes::note(
        "FLOW_TRACED",
        json!({"flow": id, "revision": revision.revision, "nodes": revision.graph.nodes.len(), "status": revision.graph.status, "via": "cli", "digest": revision.digest}),
    );
    let mut detail = revision_lines(&revision);
    if let Some(changes) = &changes {
        detail.insert(
            0,
            format!(
                "  since revision {}: +{} / -{} functions, +{} / -{} calls",
                previous.as_ref().map_or(0, |previous| previous.revision),
                changes.added_nodes.len(),
                changes.removed_nodes.len(),
                changes.added_edges.len(),
                changes.removed_edges.len()
            ),
        );
    }
    let response = Response::new(
        "flow.retrace",
        Status::Ok,
        format!("draft {id} revision {}", revision.revision),
    )
    .data(json!({"draft": id, "revision": revision, "diff": changes}));
    finish(&args, response, &detail)
}

/** Run 'crane flow rewind ID --to N'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn rewind(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["to"], &[])?;
    args.expect_positional(1, "crane flow rewind ID --to N")?;
    let id = &args.positional[0];
    let to = args
        .value("to")
        .ok_or("--to N names the revision to make current")?
        .parse::<u32>()
        .map_err(|_| "--to is a revision number".to_string())?;
    let workspace = Workspace::locate()?;
    let mut draft =
        trace::load(&workspace, id)?.ok_or_else(|| format!("there is no trace draft of {id}"))?;
    trace::rewind(&mut draft, to, crate::platform::now_millis())?;
    trace::save(&workspace, &draft)?;
    crate::platform::notes::note("FLOW_REWOUND", json!({"flow": id, "revision": to}));
    let response = Response::new(
        "flow.rewind",
        Status::Ok,
        format!("draft {id} is at revision {to} again (later revisions are kept)"),
    )
    .data(json!({"draft": id, "current": to}));
    finish(&args, response, &[])
}

/** Run 'crane flow draft ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn draft(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["revision"], &[])?;
    args.expect_positional(1, "crane flow draft ID [--revision N]")?;
    let id = &args.positional[0];
    let workspace = Workspace::locate()?;
    let draft =
        trace::load(&workspace, id)?.ok_or_else(|| format!("there is no trace draft of {id}"))?;
    let wanted = args
        .value("revision")
        .map(|text| {
            text.parse::<u32>()
                .map_err(|_| "--revision is a number".to_string())
        })
        .transpose()?
        .unwrap_or(draft.current);
    let revision = draft
        .revisions
        .iter()
        .find(|revision| revision.revision == wanted)
        .cloned()
        .ok_or_else(|| format!("{id} has no revision {wanted}"))?;
    let previous = draft
        .revisions
        .iter()
        .filter(|kept| kept.revision < wanted)
        .max_by_key(|kept| kept.revision);
    let changes = previous.map(|previous| trace::diff(&previous.graph, &revision.graph));
    let response = Response::new(
        "flow.draft",
        Status::Ok,
        format!(
            "draft {id}: revision {wanted} of {} (current {})",
            draft.revisions.len(),
            draft.current
        ),
    )
    .data(json!({"draft": draft, "shown": wanted, "diff": changes}));
    finish(&args, response, &revision_lines(&revision))
}

/** Run 'crane flow approve ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn approve(args: &[String]) -> Result<(), String> {
    require_human("crane flow approve")?;
    let args = parse(args, &["reason"], &["proposed"])?;
    args.expect_positional(1, "crane flow approve ID --reason TEXT [--proposed]")?;
    let id = &args.positional[0];
    let reason = text(
        &args,
        "reason",
        "why it is approved; recorded in the signed audit",
    )?;
    let workspace = Workspace::locate()?;
    let draft =
        trace::load(&workspace, id)?.ok_or_else(|| format!("there is no trace draft of {id}"))?;
    let revision = draft
        .current()
        .cloned()
        .ok_or("the draft has no revision")?;
    let status = if args.flag("proposed") {
        "proposed"
    } else {
        "active"
    };
    let (state, added) = store::approve(&workspace, &draft, status, &reason)?;
    trace::save_approved(&workspace, id, &revision.graph)?;
    trace::remove(&workspace, id)?;
    crate::platform::notes::note(
        "FLOW_APPROVED",
        json!({"flow": id, "revision": revision.revision, "digest": revision.digest, "added": added, "nodes": revision.graph.nodes.len()}),
    );
    let response = Response::new(
        "flow.approve",
        Status::Ok,
        format!(
            "approved {id} revision {} at generation {}{}; commit .crane so the team shares it",
            revision.revision,
            state.generation(),
            if added { format!(" and added it to .crane/governance.yaml ({status})") } else { String::new() }
        ),
    )
    .reference("digest", revision.digest.clone())
    .data(json!({"flow": id, "revision": revision.revision, "added": added, "generation": state.generation()}));
    finish(&args, response, &[])
}

/** Run 'crane flow discard ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn discard(args: &[String]) -> Result<(), String> {
    require_human("crane flow discard")?;
    let args = parse(args, &["reason"], &[])?;
    args.expect_positional(1, "crane flow discard ID --reason TEXT")?;
    let id = &args.positional[0];
    let reason = text(&args, "reason", "why it is discarded")?;
    let workspace = Workspace::locate()?;
    if trace::load(&workspace, id)?.is_none() {
        return Err(format!("there is no trace draft of {id}"));
    }
    trace::remove(&workspace, id)?;
    crate::platform::notes::note("FLOW_DISCARDED", json!({"flow": id}));
    let response = Response::new(
        "flow.discard",
        Status::Ok,
        format!("discarded the trace draft of {id}: {reason}"),
    );
    finish(&args, response, &[])
}

/** Run 'crane flow priority ID P0|P1|P2|P3|none'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn priority(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["enforce-model"], &[])?;
    args.expect_positional(
        2,
        "crane flow priority ID P0|P1|P2|P3|none [--enforce-model on|off]",
    )?;
    let id = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    declared(&workspace, &id)?;
    let priority = match args.positional[1].as_str() {
        "none" => None,
        other => Some(Priority::parse(other)?),
    };
    let enforce = match args.value("enforce-model").as_deref() {
        None => None,
        Some("on") => Some(true),
        Some("off") => Some(false),
        Some(other) => return Err(format!("--enforce-model is on or off, not '{other}'")),
    };
    let summary = format!(
        "{id} is {}{}",
        priority.map_or("without a priority", Priority::name),
        match enforce {
            Some(true) => "; changes from models outside its tiers need a person",
            Some(false) => "; its routing is guidance only",
            None => "",
        }
    );
    signed(
        &args,
        &workspace,
        "crane flow priority",
        "flow.priority",
        json!({"flow": id, "priority": priority.map(Priority::name), "enforce_model": enforce}),
        summary,
        |book| {
            let knowledge = book.flows.entry(id.clone()).or_default();
            knowledge.priority = priority;
            if let Some(enforce) = enforce {
                knowledge.enforce_model = enforce;
            }
            Ok(())
        },
    )
}

/** Run 'crane flow context add ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn context_add(args: &[String]) -> Result<(), String> {
    let args = parse(
        args,
        &[
            "kind",
            "title",
            "value",
            "unit",
            "threshold",
            "file",
            "lines",
            "id",
        ],
        &[],
    )?;
    args.expect_positional(
        1,
        "crane flow context add ID --kind KIND --title TEXT [--value TEXT] ...",
    )?;
    let flow = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    declared(&workspace, &flow)?;
    let kind = ContextKind::parse(
        &args
            .value("kind")
            .ok_or("--kind is required (query, metric, latency, file, note, or link)")?,
    )?;
    let title = text(&args, "title", "what it is")?;
    let value = args.value("value").unwrap_or_default();
    checked("--value", &value)?;
    let file = args
        .value("file")
        .map(|raw| repository_file(&workspace, &raw))
        .transpose()?;
    if value.trim().is_empty() && file.is_none() {
        return Err("give --value TEXT or --file PATH".into());
    }
    let threshold = args
        .value("threshold")
        .map(|text| {
            text.parse::<f64>()
                .map_err(|_| "--threshold is a number".to_string())
        })
        .transpose()?;
    let lines = args.value("lines");
    if let Some(lines) = &lines {
        let valid = lines
            .split_once('-')
            .is_some_and(|(a, b)| a.parse::<u32>().is_ok() && b.parse::<u32>().is_ok())
            || lines.parse::<u32>().is_ok();
        if !valid {
            return Err("--lines is A-B or one line number".into());
        }
    }
    let (_, _, book) = open()?;
    let taken = book
        .flows
        .get(&flow)
        .map(|knowledge| {
            knowledge
                .context
                .iter()
                .map(|item| item.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let item = ContextItem {
        id: item_id(args.value("id"), &title, &taken)?,
        kind,
        title,
        value: value.trim().to_string(),
        unit: args
            .value("unit")
            .or_else(|| (kind == ContextKind::Latency).then(|| "ms".to_string())),
        threshold,
        file,
        lines,
        added_by: crate::platform::actor(),
        added_at: crate::platform::now_millis(),
    };
    let summary = format!(
        "added {} context '{}' ({}) to {flow}",
        kind.name(),
        item.title,
        item.id
    );
    signed(
        &args,
        &workspace,
        "crane flow context add",
        "flow.context.add",
        json!({"flow": flow, "item": item.id, "kind": kind.name()}),
        summary,
        |book| {
            let knowledge = book.flows.entry(flow.clone()).or_default();
            if knowledge.context.len() >= MAX_CONTEXT {
                return Err(format!(
                    "{flow} holds {MAX_CONTEXT} context items, the most it keeps"
                ));
            }
            knowledge.context.push(item);
            Ok(())
        },
    )
}

/** Run 'crane flow context remove ID ITEM'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn context_remove(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(2, "crane flow context remove ID ITEM")?;
    let (flow, item) = (args.positional[0].clone(), args.positional[1].clone());
    let workspace = Workspace::locate()?;
    let summary = format!("removed context {item} from {flow}");
    signed(
        &args,
        &workspace,
        "crane flow context remove",
        "flow.context.remove",
        json!({"flow": flow, "item": item}),
        summary,
        |book| {
            let knowledge = book
                .flows
                .get_mut(&flow)
                .ok_or_else(|| format!("{flow} has no knowledge"))?;
            let before = knowledge.context.len();
            knowledge.context.retain(|kept| kept.id != item);
            if knowledge.context.len() == before {
                return Err(format!("{flow} has no context item {item}"));
            }
            Ok(())
        },
    )
}

/** Run 'crane flow person add ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn person_add(args: &[String]) -> Result<(), String> {
    let args = parse(
        args,
        &["name", "role", "id", "github", "jira", "slack", "source"],
        &[],
    )?;
    args.expect_positional(1, "crane flow person add ID --name NAME --role ROLE ...")?;
    let flow = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    declared(&workspace, &flow)?;
    let name = text(&args, "name", "their name")?;
    let role = Role::parse(
        &args
            .value("role")
            .ok_or("--role is required (author, owner, reviewer, or oncall)")?,
    )?;
    let source = args.value("source").unwrap_or_else(|| "manual".into());
    if !["manual", "git", "jira", "slack"].contains(&source.as_str()) {
        return Err("--source is manual, git, jira, or slack".into());
    }
    for key in ["github", "jira", "slack"] {
        if let Some(value) = args.value(key) {
            if value.len() > 120 || value.chars().any(|c| c.is_whitespace() || c.is_control()) {
                return Err(format!("--{key} is one handle or key without spaces"));
            }
        }
    }
    let (_, _, book) = open()?;
    let taken = book
        .flows
        .get(&flow)
        .map(|knowledge| {
            knowledge
                .people
                .iter()
                .map(|person| person.id.clone())
                .collect()
        })
        .unwrap_or_default();
    let person = Person {
        id: item_id(
            args.value("id").or_else(|| args.value("github")),
            &name,
            &taken,
        )?,
        name,
        role,
        github: args.value("github"),
        jira: args.value("jira"),
        slack: args.value("slack"),
        source,
    };
    let summary = format!("designated {} on {flow}", person.name);
    signed(
        &args,
        &workspace,
        "crane flow person add",
        "flow.person.add",
        json!({"flow": flow, "person": person.id}),
        summary,
        |book| {
            let knowledge = book.flows.entry(flow.clone()).or_default();
            if knowledge.people.len() >= MAX_PEOPLE {
                return Err(format!(
                    "{flow} names {MAX_PEOPLE} people, the most it keeps"
                ));
            }
            knowledge.people.push(person);
            Ok(())
        },
    )
}

/** Run 'crane flow person remove ID PERSON'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn person_remove(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(2, "crane flow person remove ID PERSON")?;
    let (flow, person) = (args.positional[0].clone(), args.positional[1].clone());
    let workspace = Workspace::locate()?;
    let summary = format!("removed {person} from {flow}");
    signed(
        &args,
        &workspace,
        "crane flow person remove",
        "flow.person.remove",
        json!({"flow": flow, "person": person}),
        summary,
        |book| {
            let knowledge = book
                .flows
                .get_mut(&flow)
                .ok_or_else(|| format!("{flow} has no knowledge"))?;
            let before = knowledge.people.len();
            knowledge.people.retain(|kept| kept.id != person);
            if knowledge.people.len() == before {
                return Err(format!("{flow} names no {person}"));
            }
            Ok(())
        },
    )
}

/** Run 'crane flow people suggest ID': the Git authors of the flow's files, most commits first
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn people_suggest(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane flow people suggest ID")?;
    let flow = &args.positional[0];
    let (workspace, _, book) = open()?;
    let files = book
        .flows
        .get(flow)
        .and_then(|knowledge| knowledge.trace.as_ref().map(|trace| trace.files.clone()))
        .or_else(|| {
            trace::load(&workspace, flow)
                .ok()
                .flatten()
                .and_then(|draft| draft.current().map(|revision| revision.graph.files.clone()))
        })
        .ok_or_else(|| format!("{flow} has no trace yet; its files come from the trace"))?;
    let mut authors: BTreeMap<String, (String, u64)> = BTreeMap::new();
    let mut command = vec!["log", "--format=%an%x09%ae", "--"];
    command.extend(files.iter().map(String::as_str));
    let log = crate::platform::git::git_in(&workspace.root, &command).unwrap_or_default();
    for line in log.lines() {
        let (name, email) = line.split_once('\t').unwrap_or((line, ""));
        let entry = authors
            .entry(email.to_ascii_lowercase())
            .or_insert((name.to_string(), 0));
        entry.1 += 1;
    }
    let mut ranked = authors
        .into_iter()
        .map(|(email, (name, commits))| (name, email, commits))
        .collect::<Vec<_>>();
    ranked.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    let detail = ranked
        .iter()
        .take(10)
        .map(|(name, _, commits)| format!("  {name}: {commits} commit(s) · crane flow person add {flow} --name '{name}' --role author --source git"))
        .collect::<Vec<_>>();
    let response = Response::new("flow.people.suggest", Status::Ok, format!("{} author(s) of {}'s {} file(s)", ranked.len(), flow, files.len()))
        .data(json!({"flow": flow, "authors": ranked.iter().take(10).map(|(name, _, commits)| json!({"name": name, "commits": commits})).collect::<Vec<_>>()}));
    finish(&args, response, &detail)
}

/** A test from its fields
 * Input
    - workspace: &Workspace - repository
    - id: String - identifier
    - payload: &Value - {path, name, description, assumptions, mock_data, command}
    - proposed_by: String - who wrote it
    - required: bool - every release must pass it
 * Output
    - Result<FlowTest, String>
*/
fn test_of(
    workspace: &Workspace,
    id: String,
    payload: &Value,
    proposed_by: String,
    required: bool,
) -> Result<FlowTest, String> {
    let path = repository_file(workspace, payload["path"].as_str().unwrap_or_default())?;
    let digest = std::fs::read(workspace.root.join(&path))
        .map(|bytes| crate::trust::crypto::sha512_hex(&bytes))
        .map_err(|error| format!("could not read {path}: {error}"))?;
    let field = |key: &str| payload[key].as_str().unwrap_or_default().trim().to_string();
    let test = FlowTest {
        id,
        path,
        name: field("name"),
        description: field("description"),
        assumptions: payload["assumptions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        mock_data: field("mock_data"),
        command: payload["command"]
            .as_str()
            .map(str::trim)
            .filter(|command| !command.is_empty())
            .map(str::to_string),
        required,
        proposed_by,
        approved_by: crate::platform::actor(),
        approved_at: crate::platform::now_millis(),
        digest,
    };
    for (what, value) in [
        ("the name", &test.name),
        ("the description", &test.description),
        ("the mock data", &test.mock_data),
    ] {
        checked(what, value)?;
    }
    if test.name.is_empty() || test.description.is_empty() {
        return Err("a test needs a name and a description in plain words".into());
    }
    Ok(test)
}

/** Run 'crane flow test add ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn test_add(args: &[String]) -> Result<(), String> {
    let args = parse(
        args,
        &[
            "file",
            "name",
            "description",
            "assumptions",
            "mock",
            "command",
            "id",
        ],
        &["required"],
    )?;
    args.expect_positional(
        1,
        "crane flow test add ID --file PATH --name TEXT --description TEXT ...",
    )?;
    let flow = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    declared(&workspace, &flow)?;
    let (_, _, book) = open()?;
    let name = text(&args, "name", "what it checks")?;
    let taken = book
        .flows
        .get(&flow)
        .map(|knowledge| knowledge.tests.iter().map(|test| test.id.clone()).collect())
        .unwrap_or_default();
    let payload = json!({
        "path": args.value("file").ok_or("--file is required (the test file)")?,
        "name": name,
        "description": args.value("description").unwrap_or_default(),
        "assumptions": args.value("assumptions").map(|text| text.split([';', '\n']).map(str::trim).filter(|item| !item.is_empty()).map(str::to_string).collect::<Vec<_>>()).unwrap_or_default(),
        "mock_data": args.value("mock").unwrap_or_default(),
        "command": args.value("command"),
    });
    let test = test_of(
        &workspace,
        item_id(args.value("id"), &name, &taken)?,
        &payload,
        crate::platform::actor(),
        args.flag("required"),
    )?;
    let summary = format!(
        "attached test {} ({}) to {flow}{}",
        test.id,
        test.path,
        if test.required {
            "; required in every release"
        } else {
            ""
        }
    );
    signed(
        &args,
        &workspace,
        "crane flow test add",
        "flow.test.add",
        json!({"flow": flow, "test": test.id, "required": test.required}),
        summary,
        |book| {
            let knowledge = book.flows.entry(flow.clone()).or_default();
            if knowledge.tests.len() >= MAX_TESTS {
                return Err(format!("{flow} holds {MAX_TESTS} tests, the most it keeps"));
            }
            knowledge.tests.push(test);
            Ok(())
        },
    )
}

/** Run 'crane flow test require ID TEST [--off]'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn test_require(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &["off"])?;
    args.expect_positional(2, "crane flow test require ID TEST [--off]")?;
    let (flow, test) = (args.positional[0].clone(), args.positional[1].clone());
    let required = !args.flag("off");
    let workspace = Workspace::locate()?;
    let summary = format!(
        "{test} of {flow} is {}",
        if required {
            "required in every release"
        } else {
            "no longer required"
        }
    );
    signed(
        &args,
        &workspace,
        "crane flow test require",
        "flow.test.require",
        json!({"flow": flow, "test": test, "required": required}),
        summary,
        |book| {
            let kept = book
                .flows
                .get_mut(&flow)
                .and_then(|knowledge| knowledge.tests.iter_mut().find(|kept| kept.id == test))
                .ok_or_else(|| format!("{flow} has no test {test}"))?;
            kept.required = required;
            Ok(())
        },
    )
}

/** Run 'crane flow test remove ID TEST'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn test_remove(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(2, "crane flow test remove ID TEST")?;
    let (flow, test) = (args.positional[0].clone(), args.positional[1].clone());
    let workspace = Workspace::locate()?;
    let summary = format!("removed test {test} from {flow}");
    signed(
        &args,
        &workspace,
        "crane flow test remove",
        "flow.test.remove",
        json!({"flow": flow, "test": test}),
        summary,
        |book| {
            let knowledge = book
                .flows
                .get_mut(&flow)
                .ok_or_else(|| format!("{flow} has no knowledge"))?;
            let before = knowledge.tests.len();
            knowledge.tests.retain(|kept| kept.id != test);
            if knowledge.tests.len() == before {
                return Err(format!("{flow} has no test {test}"));
            }
            Ok(())
        },
    )
}

/** Run 'crane flow tests run [ID] [--required]': each test's command (never through a shell),
 * recorded for the watchers; it fails when a required test fails or cannot run
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn tests_run(args: &[String]) -> Result<(), String> {
    require_human("crane flow tests run")?;
    let args = parse(args, &["timeout"], &["required"])?;
    let (workspace, _, book) = open()?;
    let only = args.positional.first().cloned();
    let timeout = args
        .value("timeout")
        .map(|text| {
            text.parse::<u64>()
                .map_err(|_| "--timeout is seconds".to_string())
        })
        .transpose()?
        .unwrap_or(600);
    let runtime = workspace.trust()?.runtime();
    let mut results = watch::test_results(&runtime);
    let mut rows = Vec::new();
    for (flow, knowledge) in &book.flows {
        if only.as_ref().is_some_and(|only| only != flow) {
            continue;
        }
        for test in knowledge
            .tests
            .iter()
            .filter(|test| test.required || !args.flag("required"))
        {
            let now = crate::platform::now_millis();
            let (status, detail) = match test
                .command
                .as_deref()
                .map(str::split_whitespace)
                .map(|mut words| (words.next(), words.map(str::to_string).collect::<Vec<_>>()))
            {
                Some((Some(program), arguments)) if workspace.root.join(&test.path).is_file() => {
                    match crate::brick::runtime::run_program(
                        &workspace.root,
                        program,
                        &arguments,
                        timeout,
                    ) {
                        Ok(0) => ("PASSED", "exit 0".to_string()),
                        Ok(code) => ("FAILED", format!("exit {code}")),
                        Err(error) => ("FAILED", error),
                    }
                }
                Some(_) => ("FAILED", format!("{} is missing", test.path)),
                None => (
                    "NOT_RUN",
                    "no command; it is run by the project's own test runner".to_string(),
                ),
            };
            results.entry(flow.clone()).or_default().insert(
                test.id.clone(),
                json!({"status": status, "at": now, "detail": detail}),
            );
            rows.push(json!({"flow": flow, "test": test.id, "path": test.path, "required": test.required, "status": status, "detail": detail}));
        }
    }
    watch::save_test_results(&runtime, &results)?;
    let failed_required = rows
        .iter()
        .filter(|row| row["required"] == true && row["status"] != "PASSED")
        .count();
    let passed = rows.iter().filter(|row| row["status"] == "PASSED").count();
    crate::platform::notes::note(
        "FLOW_TESTS_RUN",
        json!({"flow": only, "tests": rows.len(), "passed": passed, "failed": rows.len() - passed, "required": failed_required, "result": if failed_required > 0 { "FAILED" } else { "OK" }}),
    );
    let detail = rows
        .iter()
        .map(|row| {
            format!(
                "  [{}] {} {}{} — {}",
                row["status"].as_str().unwrap_or_default(),
                row["flow"].as_str().unwrap_or_default(),
                row["path"].as_str().unwrap_or_default(),
                if row["required"] == true {
                    " (required)"
                } else {
                    ""
                },
                row["detail"].as_str().unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.tests.run",
        if failed_required > 0 {
            Status::Failed
        } else {
            Status::Passed
        },
        format!(
            "{passed} of {} test(s) passed; {failed_required} required test(s) did not",
            rows.len()
        ),
    )
    .data(json!({"results": rows}));
    finish(&args, response, &detail)
}

/** Run 'crane flow proposals'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn proposals(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &["all"])?;
    let workspace = Workspace::locate()?;
    let all = watch::proposals(&workspace);
    let shown = all
        .iter()
        .filter(|proposal| args.flag("all") || proposal.status == "PENDING")
        .collect::<Vec<_>>();
    let detail = shown
        .iter()
        .map(|proposal| {
            format!(
                "  {} [{}] {} for {} by {} ({}): {}",
                proposal.proposal_id,
                proposal.status,
                proposal.kind,
                proposal.flow,
                proposal.proposed_by.agent,
                proposal.proposed_by.session,
                proposal.payload["name"]
                    .as_str()
                    .or_else(|| proposal.payload["title"].as_str())
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.proposals",
        Status::Ok,
        format!(
            "{} proposal(s) waiting",
            all.iter()
                .filter(|proposal| proposal.status == "PENDING")
                .count()
        ),
    )
    .data(json!({"proposals": shown}));
    finish(&args, response, &detail)
}

/** Run 'crane flow proposals approve|reject PROPOSAL'
 * Input
    - args: &[String] - arguments
    - approving: bool - approve (else reject)
 * Output
    - Result<(), String>
*/
fn proposal_decide(args: &[String], approving: bool) -> Result<(), String> {
    require_human("crane flow proposals")?;
    let args = parse(args, &["reason"], &["required"])?;
    args.expect_positional(
        1,
        "crane flow proposals approve|reject PROPOSAL [--required] [--reason TEXT]",
    )?;
    let id = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    let mut proposal: Proposal = watch::proposals(&workspace)
        .into_iter()
        .find(|proposal| proposal.proposal_id == id)
        .ok_or_else(|| format!("there is no proposal {id}"))?;
    if proposal.status != "PENDING" {
        return Err(format!(
            "{id} was already {}",
            proposal.status.to_ascii_lowercase()
        ));
    }
    let now = crate::platform::now_millis();
    if !approving {
        let reason = text(
            &args,
            "reason",
            "why it is rejected; the agent's session can be told",
        )?;
        proposal.status = "REJECTED".into();
        proposal.decided_by = Some(crate::platform::actor());
        proposal.decided_at = Some(now);
        proposal.reason = Some(reason.clone());
        watch::save_proposal(&workspace, &proposal)?;
        let runtime = workspace.trust()?.runtime();
        let _ = watch::notify_session(
            &runtime,
            &proposal.proposed_by.session,
            &format!(
                "Your proposal {id} for {} was rejected: {reason}",
                proposal.flow
            ),
        );
        crate::platform::notes::note(
            "FLOW_PROPOSAL_DECIDED",
            json!({"flow": proposal.flow, "proposal": id, "kind": proposal.kind, "decision": "REJECTED", "result": "REJECTED"}),
        );
        let response = Response::new(
            "flow.proposals.reject",
            Status::Ok,
            format!("rejected {id}"),
        );
        return finish(&args, response, &[]);
    }
    declared(&workspace, &proposal.flow)?;
    let flow = proposal.flow.clone();
    let by = format!(
        "{} ({})",
        proposal.proposed_by.agent, proposal.proposed_by.session
    );
    let (_, _, book) = open()?;
    let knowledge = book.flows.get(&flow).cloned().unwrap_or_default();
    let summary;
    let detail;
    let addition: Change = if proposal.kind == "test" {
        let taken = knowledge.tests.iter().map(|test| test.id.clone()).collect();
        let name = proposal.payload["name"]
            .as_str()
            .unwrap_or("test")
            .to_string();
        let test = test_of(
            &workspace,
            item_id(None, &name, &taken)?,
            &proposal.payload,
            by,
            args.flag("required"),
        )?;
        summary = format!(
            "attached test {} ({}) to {flow}{}",
            test.id,
            test.path,
            if test.required {
                "; required in every release"
            } else {
                ""
            }
        );
        detail = json!({"flow": flow, "proposal": id, "test": test.id, "required": test.required});
        let flow = flow.clone();
        Box::new(move |book: &mut FlowBook| {
            book.flows.entry(flow).or_default().tests.push(test);
            Ok(())
        })
    } else {
        let payload = &proposal.payload;
        let title = payload["title"].as_str().unwrap_or_default().to_string();
        let taken = knowledge
            .context
            .iter()
            .map(|item| item.id.clone())
            .collect();
        let item = ContextItem {
            id: item_id(None, &title, &taken)?,
            kind: ContextKind::parse(payload["kind"].as_str().unwrap_or("note"))?,
            title,
            value: payload["value"].as_str().unwrap_or_default().to_string(),
            unit: payload["unit"].as_str().map(str::to_string),
            threshold: payload["threshold"].as_f64(),
            file: payload["file"].as_str().map(str::to_string),
            lines: payload["lines"].as_str().map(str::to_string),
            added_by: by,
            added_at: now,
        };
        checked("the value", &item.value)?;
        summary = format!(
            "added {} context '{}' to {flow}",
            item.kind.name(),
            item.title
        );
        detail = json!({"flow": flow, "proposal": id, "item": item.id});
        let flow = flow.clone();
        Box::new(move |book: &mut FlowBook| {
            book.flows.entry(flow).or_default().context.push(item);
            Ok(())
        })
    };
    store::change(
        &workspace,
        "crane flow proposals approve",
        detail.clone(),
        addition,
    )?;
    proposal.status = "APPROVED".into();
    proposal.decided_by = Some(crate::platform::actor());
    proposal.decided_at = Some(now);
    watch::save_proposal(&workspace, &proposal)?;
    crate::platform::notes::note(
        "FLOW_PROPOSAL_DECIDED",
        json!({"flow": flow, "proposal": id, "kind": proposal.kind, "decision": "APPROVED"}),
    );
    let response = Response::new(
        "flow.proposals.approve",
        Status::Ok,
        format!("{summary}; commit .crane so the team shares it"),
    )
    .data(detail);
    finish(&args, response, &[])
}

/** Run 'crane flow watch list'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn watch_list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &["all"])?;
    let workspace = Workspace::locate()?;
    let all = watch::watchers(&workspace.trust()?.runtime());
    let shown = all
        .iter()
        .filter(|watcher| args.flag("all") || watcher.active)
        .collect::<Vec<_>>();
    let detail = shown
        .iter()
        .map(|watcher| {
            format!(
                "  {} on {} ({}) by {}: {}{}",
                watcher.watcher_id,
                watcher.flow,
                watcher.checks.join(", "),
                if watcher.created_by == "agent" {
                    format!(
                        "{} session {}",
                        watcher.parent.agent, watcher.parent.session
                    )
                } else {
                    watcher.created_by.clone()
                },
                watcher.purpose,
                if watcher.firing.is_empty() {
                    String::new()
                } else {
                    format!(" · firing {}", watcher.firing.join(", "))
                }
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.watch.list",
        Status::Ok,
        format!(
            "{} active watcher(s)",
            all.iter().filter(|watcher| watcher.active).count()
        ),
    )
    .data(json!({"watchers": shown}));
    finish(&args, response, &detail)
}

/** Run 'crane flow watch add ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn watch_add(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["purpose", "checks"], &[])?;
    args.expect_positional(
        1,
        "crane flow watch add ID --purpose TEXT [--checks trace_drift,required_tests,denials]",
    )?;
    let flow = args.positional[0].clone();
    let (workspace, _, book) = open()?;
    declared(&workspace, &flow)?;
    let purpose = text(&args, "purpose", "why it watches")?;
    let checks = match args.value("checks") {
        Some(list) => watch_checks(&list)?,
        None => watch::CHECKS
            .iter()
            .map(|check| check.to_string())
            .collect(),
    };
    let runtime = workspace.trust()?.runtime();
    let inherited = watch::inherit(&workspace, &book, &flow)?;
    let watcher = Watcher {
        watcher_id: watch::new_id("wat")?,
        flow: flow.clone(),
        purpose,
        parent: Default::default(),
        created_by: crate::platform::actor(),
        created_at: crate::platform::now_millis(),
        active: true,
        stopped_by: None,
        inherited,
        checks,
        firing: Vec::new(),
        last_run: 0,
    };
    watch::update_watchers(&runtime, |all| {
        watch::may_start(&book, all, &flow, None)?;
        all.push(watcher.clone());
        Ok(())
    })?;
    crate::platform::notes::note(
        "FLOW_WATCHER_STARTED",
        json!({"flow": flow, "watcher": watcher.watcher_id, "by": "person", "checks": watcher.checks}),
    );
    let response = Response::new(
        "flow.watch.add",
        Status::Ok,
        format!("watcher {} watches {flow} read-only", watcher.watcher_id),
    )
    .data(json!({"watcher": watcher}));
    finish(&args, response, &[])
}

/** Parse a comma-separated list of watcher checks
 * Input
    - list: &str - "trace_drift,required_tests"
 * Output
    - Result<Vec<String>, String>
    - Error when a check is unknown or the list is empty
*/
fn watch_checks(list: &str) -> Result<Vec<String>, String> {
    let wanted = list
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(bad) = wanted
        .iter()
        .find(|check| !watch::CHECKS.contains(&check.as_str()))
    {
        return Err(format!(
            "check '{bad}' is one of {}",
            watch::CHECKS.join(", ")
        ));
    }
    if wanted.is_empty() {
        return Err(format!(
            "give at least one check: {}",
            watch::CHECKS.join(", ")
        ));
    }
    Ok(wanted)
}

/** Run 'crane flow watch edit WATCHER': change an active watcher's purpose or checks (it stays
 * read-only and keeps what it inherited)
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn watch_edit(args: &[String]) -> Result<(), String> {
    require_human("crane flow watch edit")?;
    let args = parse(args, &["purpose", "checks"], &[])?;
    args.expect_positional(
        1,
        "crane flow watch edit WATCHER [--purpose TEXT] [--checks trace_drift,required_tests,denials]",
    )?;
    let id = &args.positional[0];
    let purpose = args
        .value("purpose")
        .map(|purpose| purpose.trim().to_string());
    if purpose.as_deref() == Some("") {
        return Err("the purpose cannot be empty".into());
    }
    let checks = args
        .value("checks")
        .map(|list| watch_checks(&list))
        .transpose()?;
    if purpose.is_none() && checks.is_none() {
        return Err("change --purpose or --checks".into());
    }
    let workspace = Workspace::locate()?;
    let runtime = workspace.trust()?.runtime();
    let mut edited = None;
    watch::update_watchers(&runtime, |all| {
        let watcher = all
            .iter_mut()
            .find(|watcher| watcher.watcher_id == *id && watcher.active)
            .ok_or_else(|| format!("{id} is not an active watcher"))?;
        if let Some(purpose) = &purpose {
            watcher.purpose = purpose.clone();
        }
        if let Some(checks) = &checks {
            watcher.checks = checks.clone();
        }
        edited = Some(watcher.clone());
        Ok(())
    })?;
    let watcher = edited.ok_or("the watcher was not changed")?;
    crate::platform::notes::note(
        "FLOW_WATCHER_EDITED",
        json!({"flow": watcher.flow, "watcher": id, "by": "person", "checks": watcher.checks}),
    );
    let response = Response::new(
        "flow.watch.edit",
        Status::Ok,
        format!("watcher {id} changed; it stays read-only"),
    )
    .data(json!({"watcher": watcher}));
    finish(&args, response, &[])
}

/** Run 'crane flow watch stop WATCHER'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn watch_stop(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane flow watch stop WATCHER")?;
    let id = &args.positional[0];
    let workspace = Workspace::locate()?;
    let runtime = workspace.trust()?.runtime();
    watch::update_watchers(&runtime, |all| {
        let watcher = all
            .iter_mut()
            .find(|watcher| watcher.watcher_id == *id && watcher.active)
            .ok_or_else(|| format!("{id} is not an active watcher"))?;
        watcher.active = false;
        watcher.stopped_by = Some(crate::platform::actor());
        Ok(())
    })?;
    crate::platform::notes::note(
        "FLOW_WATCHER_STOPPED",
        json!({"watcher": id, "by": "person"}),
    );
    let response = Response::new(
        "flow.watch.stop",
        Status::Ok,
        format!("watcher {id} stopped"),
    );
    finish(&args, response, &[])
}

/** Run 'crane flow watch run': every active watcher's checks now
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn watch_run(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let (workspace, _, book) = open()?;
    let raised = watch::run(
        &workspace,
        &workspace.trust()?.runtime(),
        &book,
        crate::platform::now_millis(),
    )?;
    let detail = raised
        .iter()
        .map(|incident| {
            format!(
                "  {} {} on {}: {}",
                incident.incident_id, incident.code, incident.flow, incident.summary
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.watch.run",
        if raised.is_empty() {
            Status::Ok
        } else {
            Status::Failed
        },
        format!("{} incident(s) raised", raised.len()),
    )
    .data(json!({"incidents": raised}));
    finish(&args, response, &detail)
}

/** Run 'crane flow incidents'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn incidents(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let workspace = Workspace::locate()?;
    let all = watch::incidents(&workspace.trust()?.runtime());
    let detail = all
        .iter()
        .take(30)
        .map(|incident| {
            format!(
                "  {} {} on {} ({}): {}",
                incident.incident_id,
                incident.code,
                incident.flow,
                incident.raised_by,
                incident.summary
            )
        })
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.incidents",
        Status::Ok,
        format!("{} incident(s)", all.len()),
    )
    .data(json!({"incidents": all}));
    finish(&args, response, &detail)
}

/** Run 'crane flow limit ID --watchers N | limit --session N | limit --flow N'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn limit(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["watchers", "session", "flow"], &[])?;
    let workspace = Workspace::locate()?;
    let number = |key: &str| -> Result<Option<u32>, String> {
        args.value(key)
            .map(|text| {
                text.parse::<u32>()
                    .ok()
                    .filter(|number| *number <= 50)
                    .ok_or_else(|| format!("--{key} is a number from 0 to 50"))
            })
            .transpose()
    };
    let (per_flow_target, session, repository_flow) =
        (number("watchers")?, number("session")?, number("flow")?);
    let flow = args.positional.first().cloned();
    if let Some(flow) = &flow {
        declared(&workspace, flow)?;
    }
    if per_flow_target.is_none() && session.is_none() && repository_flow.is_none() {
        return Err("give --watchers N with a flow, --session N (watchers one agent session may start), or --flow N (watchers per flow)".into());
    }
    let summary = format!(
        "limits: {}{}{}",
        match (&flow, per_flow_target) {
            (Some(flow), Some(number)) => format!("{flow} allows {number} watcher(s); "),
            _ => String::new(),
        },
        session
            .map(|number| format!("an agent session may start {number}; "))
            .unwrap_or_default(),
        repository_flow
            .map(|number| format!("each flow allows at most {number}"))
            .unwrap_or_default()
    );
    signed(
        &args,
        &workspace,
        "crane flow limit",
        "flow.limit",
        json!({"flow": flow, "watchers": per_flow_target, "session": session, "per_flow": repository_flow}),
        summary,
        |book| {
            if let Some(number) = session {
                book.limits.max_watchers_per_session = number;
            }
            if let Some(number) = repository_flow {
                book.limits.max_watchers_per_flow = number;
            }
            if let Some(number) = per_flow_target {
                let flow =
                    flow.ok_or("--watchers N needs the flow: crane flow limit ID --watchers N")?;
                book.flows.entry(flow).or_default().max_watchers = Some(number);
            }
            Ok(())
        },
    )
}

/** Run 'crane flow models [list]'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn models_list(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    let (_, _, book) = open()?;
    let detail = book
        .models
        .iter()
        .map(|entry| {
            format!(
                "  {} · {} · {}{}",
                entry.model,
                entry.provider,
                entry.tier.name(),
                if entry.available {
                    ""
                } else {
                    " (unavailable)"
                }
            )
        })
        .chain(Priority::ALL.iter().map(|priority| {
            let (tiers, chosen) = route(&book, *priority, None);
            format!(
                "  {} → {} ({})",
                priority.name(),
                chosen.map_or("no model", |entry| entry.model.as_str()),
                tiers
                    .iter()
                    .map(|tier| tier.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }))
        .collect::<Vec<_>>();
    let response = Response::new(
        "flow.models",
        Status::Ok,
        format!("{} model(s)", book.models.len()),
    )
    .data(json!({"models": book.models, "routing": book.routing}));
    finish(&args, response, &detail)
}

/** Run 'crane flow models add'
 * Input
    - args: &[String] - options
 * Output
    - Result<(), String>
*/
fn models_add(args: &[String]) -> Result<(), String> {
    let args = parse(args, &["provider", "model", "tier"], &[])?;
    let workspace = Workspace::locate()?;
    let provider = text(&args, "provider", "claude, codex, gemini, copilot, ...")?;
    let model = text(&args, "model", "the model id")?;
    let tier = Tier::parse(
        &args
            .value("tier")
            .ok_or("--tier is required (frontier, balanced, or fast)")?,
    )?;
    for value in [&provider, &model] {
        if value.len() > 80 || value.chars().any(|c| c.is_whitespace()) {
            return Err("a provider and a model are one word each".into());
        }
    }
    let summary = format!("{model} ({provider}) is {} tier", tier.name());
    signed(
        &args,
        &workspace,
        "crane flow models add",
        "flow.models.add",
        json!({"flow": null, "model": model, "tier": tier.name()}),
        summary,
        |book| {
            book.models
                .retain(|entry| !entry.model.eq_ignore_ascii_case(&model));
            book.models.push(ModelEntry {
                provider,
                model,
                tier,
                available: true,
            });
            Ok(())
        },
    )
}

/** Run 'crane flow models remove MODEL'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn models_remove(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane flow models remove MODEL")?;
    let model = args.positional[0].clone();
    let workspace = Workspace::locate()?;
    let summary = format!("removed {model}");
    signed(
        &args,
        &workspace,
        "crane flow models remove",
        "flow.models.remove",
        json!({"flow": null, "model": model}),
        summary,
        |book| {
            let before = book.models.len();
            book.models
                .retain(|entry| !entry.model.eq_ignore_ascii_case(&model));
            if book.models.len() == before {
                return Err(format!("{model} is not in the list"));
            }
            Ok(())
        },
    )
}

/** Run 'crane flow models available MODEL on|off'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn models_available(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(2, "crane flow models available MODEL on|off")?;
    let model = args.positional[0].clone();
    let available = match args.positional[1].as_str() {
        "on" => true,
        "off" => false,
        other => return Err(format!("say on or off, not '{other}'")),
    };
    let workspace = Workspace::locate()?;
    let summary = format!(
        "{model} is {}",
        if available {
            "available"
        } else {
            "not routed to"
        }
    );
    signed(
        &args,
        &workspace,
        "crane flow models available",
        "flow.models.available",
        json!({"flow": null, "model": model, "available": available}),
        summary,
        |book| {
            let entry = book
                .models
                .iter_mut()
                .find(|entry| entry.model.eq_ignore_ascii_case(&model))
                .ok_or_else(|| format!("{model} is not in the list"))?;
            entry.available = available;
            Ok(())
        },
    )
}

/** Run 'crane flow models route ID'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn route_flow(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(1, "crane flow models route ID")?;
    let flow = &args.positional[0];
    let (_, _, book) = open()?;
    let priority = book
        .flows
        .get(flow)
        .and_then(|knowledge| knowledge.priority)
        .ok_or_else(|| {
            format!("{flow} has no priority; 'crane flow priority {flow} P0|P1|P2|P3'")
        })?;
    let (tiers, chosen) = route(&book, priority, None);
    let response = Response::new(
        "flow.models.route",
        Status::Ok,
        format!("{flow} is {} → {}", priority.name(), chosen.map_or("no available model of its tiers".to_string(), |entry| format!("{} ({})", entry.model, entry.provider))),
    )
    .data(json!({"flow": flow, "priority": priority.name(), "tiers": tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>(), "model": chosen}));
    finish(&args, response, &[])
}

/** Run 'crane flow routing PRIORITY TIERS|default'
 * Input
    - args: &[String] - arguments
 * Output
    - Result<(), String>
*/
fn routing(args: &[String]) -> Result<(), String> {
    let args = parse(args, &[], &[])?;
    args.expect_positional(2, "crane flow routing P0|P1|P2|P3 TIER[,TIER]|default")?;
    let priority = Priority::parse(&args.positional[0])?;
    let tiers = match args.positional[1].as_str() {
        "default" => None,
        list => Some(
            list.split(',')
                .map(Tier::parse)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };
    let workspace = Workspace::locate()?;
    let summary = format!(
        "{} routes to {}",
        priority.name(),
        tiers
            .as_ref()
            .map_or("its default tiers".to_string(), |tiers| tiers
                .iter()
                .map(|tier| tier.name())
                .collect::<Vec<_>>()
                .join(", "))
    );
    signed(
        &args,
        &workspace,
        "crane flow routing",
        "flow.routing",
        json!({"flow": null, "priority": priority.name(), "tiers": tiers.as_ref().map(|tiers| tiers.iter().map(|tier| tier.name()).collect::<Vec<_>>())}),
        summary,
        |book| {
            match tiers {
                Some(tiers) => {
                    book.routing.insert(priority, tiers);
                }
                None => {
                    book.routing.remove(&priority);
                }
            }
            Ok(())
        },
    )
}
