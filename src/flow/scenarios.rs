// Test scenarios of a flow, written by fixed rules from what the repository knows about it, for a
// person to review. No model is called: the same knowledge always gives the same scenarios.
//
// - every starting point gets a smoke test, and a mocked test whose mocks are the functions its
//   kept call graph reaches in other files;
// - a flow through two or more zones gets an end-to-end integration test through them;
// - context becomes checks: a latency budget a performance test, a metric a threshold test, a query
//   a data integration test, a link an HTTP integration test, a file a regression test, a note a
//   contract test;
// - every Brick attached to the flow or its zones gets a negative test (the action it forbids is
//   blocked), and a critical or restricted zone a contract test at its boundary;
// - an observe or assisted ceiling gets a negative test (agents' changes wait for a person).
//
// A scenario already among the flow's tests (same title) is left out. A person edits the values
// (URLs, parameters, JSON bodies, expectations), accepts each scenario or all of them, and saves
// them to .crane/scenarios/ID.json, which travels with the repository; generating again keeps the
// saved edits of a scenario with the same id.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::model::FlowBook;
use crate::governance::workspace::Workspace;
use crate::platform::files::write_atomic;
use crate::platform::io_error;

/** The kinds of scenario */
pub(crate) const TYPES: &[&str] = &[
    "smoke",
    "mocked",
    "integration",
    "performance",
    "negative",
    "contract",
    "regression",
];

/** Most scenarios one file holds */
const MAX_SCENARIOS: usize = 200;

/** A request a scenario makes
 * Fields
    - method: String - CALL (a function), GET, POST, QUERY, ...
    - url: String - the address or the function
    - params: Map<String, Value> - parameters
    - body: Value - the JSON body (null when none)
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Request {
    pub(crate) method: String,
    pub(crate) url: String,
    pub(crate) params: Map<String, Value>,
    pub(crate) body: Value,
}

/** What a scenario expects
 * Fields
    - outcome: String - success, blocked, or within
    - status: Option<u16> - an HTTP status
    - latency_ms: Option<f64> - the most time allowed
    - assertions: Vec<String> - what must hold
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Expect {
    pub(crate) outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) latency_ms: Option<f64>,
    pub(crate) assertions: Vec<String>,
}

/** One test scenario
 * Fields
    - id: String - stable id ("sc_…", from its kind and source)
    - kind: String - one of TYPES
    - title: String - one line
    - description: String - what it checks and why
    - source: String - the knowledge it came from (entry:, context:, brick:, zone:, flow:)
    - request: Request - what it calls
    - mocks: Vec<String> - functions replaced with mocks
    - expect: Expect - what it expects
    - accepted: bool - a person accepted it
    - reviewed_by: Option<String> - who accepted it
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Scenario {
    pub(crate) id: String,
    #[serde(rename = "type")]
    pub(crate) kind: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) source: String,
    pub(crate) request: Request,
    pub(crate) mocks: Vec<String>,
    pub(crate) expect: Expect,
    pub(crate) accepted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reviewed_by: Option<String>,
}

/** The scenarios file of a flow
 * Fields
    - flow: String - flow
    - saved_by: String - who saved it
    - saved_at: u64 - Unix milliseconds
    - scenarios: Vec<Scenario> - the scenarios
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ScenarioFile {
    pub(crate) flow: String,
    pub(crate) saved_by: String,
    pub(crate) saved_at: u64,
    pub(crate) scenarios: Vec<Scenario>,
}

/** A stable scenario id from its kind and source
 * Input
    - kind: &str - kind
    - source: &str - source
 * Output
    - String
*/
fn id_of(kind: &str, source: &str) -> String {
    let digest = crate::trust::crypto::digest_json("flows/scenario/v1", &json!([kind, source]));
    format!("sc_{}", &digest[..12])
}

/** Build one scenario
 * Input
    - kind, source, title, description - see Scenario
    - request: Request - what it calls
    - expect: Expect - what it expects
 * Output
    - Scenario
*/
fn scenario(
    kind: &str,
    source: &str,
    title: String,
    description: String,
    request: Request,
    expect: Expect,
) -> Scenario {
    Scenario {
        id: id_of(kind, source),
        kind: kind.into(),
        title,
        description,
        source: source.into(),
        request,
        expect,
        ..Scenario::default()
    }
}

/** A function call request
 * Input
    - entry: &str - the starting point
 * Output
    - Request
*/
fn call(entry: &str) -> Request {
    Request {
        method: "CALL".into(),
        url: entry.into(),
        ..Request::default()
    }
}

/** The file of a flow's saved scenarios
 * Input
    - workspace: &Workspace - repository
    - flow: &str - flow (validated as a name)
 * Output
    - Result<PathBuf, String>
*/
pub(crate) fn file(workspace: &Workspace, flow: &str) -> Result<PathBuf, String> {
    crate::platform::validate_name("Flow", flow)?;
    Ok(workspace
        .crane
        .join("scenarios")
        .join(format!("{flow}.json")))
}

/** Read a flow's saved scenarios
 * Input
    - workspace: &Workspace - repository
    - flow: &str - flow
 * Output
    - Result<Option<ScenarioFile>, String>
*/
pub(crate) fn load(workspace: &Workspace, flow: &str) -> Result<Option<ScenarioFile>, String> {
    let path = file(workspace, flow)?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|error| format!("{} is malformed: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("could not read {}: {error}", path.display())),
    }
}

/** Write the scenarios of a flow from its knowledge, keeping the saved edits of scenarios with the
 * same id (and the saved ones the rules no longer write)
 * Input
    - workspace: &Workspace - repository
    - book: &FlowBook - flow knowledge
    - flow: &str - flow
 * Output
    - Result<Vec<Scenario>, String>
    - Error when the flow is not declared
*/
pub(crate) fn generate(
    workspace: &Workspace,
    book: &FlowBook,
    flow: &str,
) -> Result<Vec<Scenario>, String> {
    let governance = crate::governance::zones::load(workspace)?;
    let declared = governance
        .flows
        .iter()
        .find(|candidate| candidate.id == flow)
        .ok_or_else(|| format!("{flow} is not a flow of .crane/governance.yaml"))?;
    let knowledge = book.flows.get(flow).cloned().unwrap_or_default();
    let stages = declared
        .stages
        .iter()
        .map(|stage| stage.id.clone())
        .collect::<Vec<_>>();
    let mut written = Vec::new();

    // Starting points: smoke and mocked
    let graph = knowledge
        .trace
        .as_ref()
        .and_then(|approved| super::trace::load_approved(workspace, flow, &approved.digest));
    for entry in &declared.entry_points {
        written.push(scenario(
            "smoke",
            &format!("entry:{entry}"),
            format!("{entry} answers without an error"),
            format!("The flow's starting point runs once with typical input and returns without raising. A first check that {flow} is alive."),
            call(entry),
            Expect { outcome: "success".into(), assertions: vec!["no exception or error result".into()], ..Expect::default() },
        ));
        let file_of = entry.split(':').next().unwrap_or_default();
        let mocks = graph
            .as_ref()
            .map(|graph| {
                graph
                    .nodes
                    .iter()
                    .filter(|node| node.depth > 0 && !file_of.is_empty() && node.path != file_of)
                    .map(|node| format!("{}:{}", node.path, node.symbol))
                    .take(12)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut mocked = scenario(
            "mocked",
            &format!("entry:{entry}:mocked"),
            format!("{entry} with its dependencies mocked"),
            if mocks.is_empty() {
                format!("Run {entry} with the services it calls replaced by mocks. Approve a trace of {flow} so the mocks are listed from its call graph.")
            } else {
                format!("Run {entry} with the {} function(s) its approved trace reaches in other files replaced by mocks, and check the calls it makes to them.", mocks.len())
            },
            call(entry),
            Expect {
                outcome: "success".into(),
                assertions: vec!["each mock is called with the expected arguments".into()],
                ..Expect::default()
            },
        );
        mocked.mocks = mocks;
        written.push(mocked);
    }

    // Zones the flow runs through: end to end
    if stages.len() >= 2 {
        written.push(scenario(
            "integration",
            &format!("flow:{flow}:stages"),
            format!("{flow} end to end through {}", stages.join(" → ")),
            "Drive the flow from its starting point through every zone in order, with real services where a test environment has them.".to_string(),
            declared.entry_points.first().map(|entry| call(entry)).unwrap_or_default(),
            Expect { outcome: "success".into(), assertions: stages.iter().map(|stage| format!("zone {stage} is reached")).collect(), ..Expect::default() },
        ));
    }

    // Context: each kind becomes a check
    for item in &knowledge.context {
        let source = format!("context:{}", item.id);
        let threshold = item.threshold;
        let entry = declared.entry_points.first().cloned().unwrap_or_default();
        let made = match item.kind.name() {
            "latency" => scenario(
                "performance",
                &source,
                format!("{}: {} stays within {}{}", item.title, entry, threshold.map(|value| value.to_string()).unwrap_or_else(|| item.value.clone()), item.unit.clone().unwrap_or_else(|| "ms".into())),
                format!("Call {entry} repeatedly and measure; the budget comes from the flow's context '{}'.", item.title),
                Request { params: Map::from_iter([("iterations".to_string(), json!(100)), ("percentile".to_string(), json!(95))]), ..call(&entry) },
                Expect { outcome: "within".into(), latency_ms: threshold.or_else(|| item.value.trim().parse().ok()), assertions: vec![format!("p95 at most the budget of '{}'", item.title)], ..Expect::default() },
            ),
            "metric" => scenario(
                "regression",
                &source,
                format!("{} stays within its limit", item.title),
                format!("After the flow runs, the metric '{}' ({}) stays within {}.", item.title, item.value, threshold.map(|value| value.to_string()).unwrap_or_else(|| "its stated value".into())),
                call(&entry),
                Expect { outcome: "within".into(), assertions: vec![format!("{} ≤ {}", item.title, threshold.map(|value| value.to_string()).unwrap_or_else(|| item.value.clone()))], ..Expect::default() },
            ),
            "query" => scenario(
                "integration",
                &source,
                format!("Data check: {}", item.title),
                "Run the flow, then run the query from its context against a test database and check the rows it returns.".into(),
                Request { method: "QUERY".into(), body: json!({"query": item.value}), ..Request::default() },
                Expect { outcome: "success".into(), assertions: vec!["the query returns the expected rows".into()], ..Expect::default() },
            ),
            "link" => scenario(
                "integration",
                &source,
                format!("{} answers", item.title),
                format!("Call the address from the flow's context '{}' and check it answers.", item.title),
                Request { method: "GET".into(), url: item.value.clone(), ..Request::default() },
                Expect { outcome: "success".into(), status: Some(200), assertions: vec!["the response is not an error page".into()], ..Expect::default() },
            ),
            "file" => scenario(
                "regression",
                &source,
                format!("Behavior of {} stays the same", item.file.clone().unwrap_or_else(|| item.title.clone())),
                format!("Pin the behavior of {}{} that the flow relies on ('{}').", item.file.clone().unwrap_or_default(), item.lines.as_ref().map(|lines| format!(" lines {lines}")).unwrap_or_default(), item.title),
                call(&entry),
                Expect { outcome: "success".into(), assertions: vec!["output matches the recorded output".into()], ..Expect::default() },
            ),
            _ => scenario(
                "contract",
                &source,
                format!("Holds: {}", item.title),
                item.value.chars().take(400).collect(),
                call(&entry),
                Expect { outcome: "success".into(), assertions: vec![item.title.clone()], ..Expect::default() },
            ),
        };
        written.push(made);
    }

    // Bricks of the flow and its zones: what they forbid is blocked
    let mut targets = BTreeSet::from([format!("flow:{flow}")]);
    targets.extend(stages.iter().map(|stage| format!("zone:{stage}")));
    if let Some(store) = crate::governance::state::State::load(workspace)
        .ok()
        .and_then(|state| crate::brick::store::BrickStore::of(&state).ok())
    {
        for attachment in store
            .attachments
            .iter()
            .filter(|attachment| targets.contains(&attachment.target.key()))
        {
            let crate::brick::model::Subject::Brick { brick_id, version } = &attachment.subject
            else {
                continue;
            };
            let Some(entry) = store
                .bricks
                .iter()
                .find(|entry| entry.brick.brick_id == *brick_id && entry.brick.version == *version)
            else {
                continue;
            };
            let brick = &entry.brick;
            let kind = serde_json::to_value(brick.brick_type)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_default();
            written.push(scenario(
                "negative",
                &format!("brick:{brick_id}@{version}:{}", attachment.target.key()),
                format!("{} blocks what it forbids in {}", brick.name, attachment.target.key()),
                format!("An agent action that breaks the {kind} Brick '{}' in {} is denied by Crane. {}", brick.name, attachment.target.key(), if brick.intent.is_empty() { brick.description.clone() } else { brick.intent.clone() }),
                Request { method: "AGENT_ACTION".into(), url: attachment.target.key(), params: Map::from_iter([("brick".to_string(), json!(format!("{brick_id}@{version}")))]), body: brick.config.clone() },
                Expect { outcome: "blocked".into(), assertions: vec![format!("Crane denies the action under {}", brick.name)], ..Expect::default() },
            ));
        }
    }

    // Sensitive zones and a strict ceiling
    for zone in governance
        .zones
        .iter()
        .filter(|zone| stages.contains(&zone.id))
    {
        let criticality = serde_json::to_value(zone.criticality)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        if criticality == "critical" || criticality == "restricted" {
            written.push(scenario(
                "contract",
                &format!("zone:{}", zone.id),
                format!("Calls into {} keep its contract", zone.id),
                format!("{} is {criticality}: the inputs the flow passes into it and the outputs it gets back keep their agreed shape.", zone.id),
                Request { method: "CALL".into(), url: format!("zone:{}", zone.id), ..Request::default() },
                Expect { outcome: "success".into(), assertions: vec!["inputs and outputs match the agreed schema".into()], ..Expect::default() },
            ));
        }
    }
    let ceiling = declared.autonomy_ceiling.name();
    if ceiling == "observe" || ceiling == "assisted" {
        written.push(scenario(
            "negative",
            &format!("flow:{flow}:ceiling"),
            format!("Agents' changes in {flow} wait for a person"),
            format!(
                "The flow's ceiling is {ceiling}: an agent's edit in its files is {}.",
                if ceiling == "observe" {
                    "denied"
                } else {
                    "held for approval"
                }
            ),
            Request {
                method: "AGENT_ACTION".into(),
                url: format!("flow:{flow}"),
                params: Map::from_iter([("action".to_string(), json!("edit"))]),
                ..Request::default()
            },
            Expect {
                outcome: "blocked".into(),
                assertions: vec![format!("Crane enforces the {ceiling} ceiling")],
                ..Expect::default()
            },
        ));
    }

    // Leave out what the flow's tests already cover, keep saved edits
    let existing = knowledge
        .tests
        .iter()
        .map(|test| test.name.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    written.retain(|scenario| !existing.contains(&scenario.title.to_ascii_lowercase()));
    let saved = load(workspace, flow)?
        .map(|file| file.scenarios)
        .unwrap_or_default();
    let mut merged = written
        .into_iter()
        .map(|fresh| {
            saved
                .iter()
                .find(|kept| kept.id == fresh.id)
                .cloned()
                .unwrap_or(fresh)
        })
        .collect::<Vec<_>>();
    for kept in saved {
        if !merged.iter().any(|scenario| scenario.id == kept.id) {
            merged.push(kept);
        }
    }
    Ok(merged)
}

/** Check scenarios a person edited before they are saved
 * Input
    - scenarios: &[Scenario] - scenarios
 * Output
    - Result<(), String>
*/
pub(crate) fn validate(scenarios: &[Scenario]) -> Result<(), String> {
    if scenarios.len() > MAX_SCENARIOS {
        return Err(format!("at most {MAX_SCENARIOS} scenarios are saved"));
    }
    let mut ids = BTreeSet::new();
    for scenario in scenarios {
        if scenario.id.is_empty() || !ids.insert(scenario.id.clone()) {
            return Err(format!(
                "scenario ids must be present and unique ('{}')",
                scenario.id
            ));
        }
        if !TYPES.contains(&scenario.kind.as_str()) {
            return Err(format!(
                "scenario {} has type '{}'; use {}",
                scenario.id,
                scenario.kind,
                TYPES.join(", ")
            ));
        }
        if scenario.title.trim().is_empty() {
            return Err(format!("scenario {} needs a title", scenario.id));
        }
        let text = serde_json::to_string(scenario).map_err(io_error)?;
        if crate::telemetry::redact::redact(&text).1 > 0 {
            return Err(format!(
                "scenario {} looks like it holds a secret; use a placeholder",
                scenario.id
            ));
        }
    }
    Ok(())
}

/** Save a flow's scenarios to .crane/scenarios/ID.json
 * Input
    - workspace: &Workspace - repository
    - flow: &str - flow
    - scenarios: Vec<Scenario> - scenarios (accepted ones get the saver as reviewer when unset)
 * Output
    - Result<ScenarioFile, String>
*/
pub(crate) fn save(
    workspace: &Workspace,
    flow: &str,
    mut scenarios: Vec<Scenario>,
) -> Result<ScenarioFile, String> {
    validate(&scenarios)?;
    let by = crate::platform::actor();
    for scenario in scenarios
        .iter_mut()
        .filter(|scenario| scenario.accepted && scenario.reviewed_by.is_none())
    {
        scenario.reviewed_by = Some(by.clone());
    }
    let saved = ScenarioFile {
        flow: flow.to_string(),
        saved_by: by,
        saved_at: crate::platform::now_millis(),
        scenarios,
    };
    let path = file(workspace, flow)?;
    fs::create_dir_all(workspace.crane.join("scenarios")).map_err(io_error)?;
    let text = serde_json::to_string_pretty(&saved).map_err(io_error)? + "\n";
    write_atomic(&path, text.as_bytes())?;
    Ok(saved)
}

/** The scenarios as plain text, for reading or attaching
 * Input
    - flow: &str - flow
    - scenarios: &[Scenario] - scenarios
 * Output
    - String
*/
pub(crate) fn to_text(flow: &str, scenarios: &[Scenario]) -> String {
    let mut out = format!(
        "Test scenarios of flow {flow} ({} total, {} accepted)\n\n",
        scenarios.len(),
        scenarios
            .iter()
            .filter(|scenario| scenario.accepted)
            .count()
    );
    for (index, scenario) in scenarios.iter().enumerate() {
        out.push_str(&format!(
            "{}. [{}] {}{}\n",
            index + 1,
            scenario.kind,
            scenario.title,
            if scenario.accepted { " (accepted)" } else { "" }
        ));
        out.push_str(&format!(
            "   id: {} · from: {}\n",
            scenario.id, scenario.source
        ));
        if !scenario.description.is_empty() {
            out.push_str(&format!("   {}\n", scenario.description));
        }
        let request = &scenario.request;
        if !request.method.is_empty() || !request.url.is_empty() {
            out.push_str(&format!("   request: {} {}\n", request.method, request.url));
        }
        if !request.params.is_empty() {
            out.push_str(&format!(
                "   params: {}\n",
                Value::Object(request.params.clone())
            ));
        }
        if !request.body.is_null() {
            out.push_str(&format!("   body: {}\n", request.body));
        }
        if !scenario.mocks.is_empty() {
            out.push_str(&format!("   mocks: {}\n", scenario.mocks.join(", ")));
        }
        let expect = &scenario.expect;
        out.push_str(&format!(
            "   expect: {}{}{}\n",
            expect.outcome,
            expect
                .status
                .map(|status| format!(", HTTP {status}"))
                .unwrap_or_default(),
            expect
                .latency_ms
                .map(|ms| format!(", within {ms} ms"))
                .unwrap_or_default()
        ));
        for assertion in &expect.assertions {
            out.push_str(&format!("   - {assertion}\n"));
        }
        out.push('\n');
    }
    out
}
