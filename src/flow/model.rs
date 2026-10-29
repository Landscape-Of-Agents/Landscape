// The flow knowledge base: what a repository knows about each flow besides its entry points in
// .crane/governance.yaml. It lives in the signed store .crane/flows.json (covered by the manifest's
// flows_digest and written only through governance::state::commit by an admin), so agents and the
// dashboard can read it but never change it. Per flow: the approved trace (which calls make the
// flow, with its digest), typed context (queries, metrics, latency, files, notes, links), the
// task priority that routes work to a model tier, the designated people (from Git, Jira, or
// Slack), the tests approved for it (with the ones every release must pass), and how many watchers
// agents may start on it. Repository-wide: the organization's provider models with their tier, the
// routing from priority to tiers, and the hard limits on watchers agents create.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::governance::state::State;

/** Layout version of .crane/flows.json */
pub(crate) const STORE_VERSION: u32 = 1;

/** Most context items one flow holds */
pub(crate) const MAX_CONTEXT: usize = 64;

/** Most people one flow names */
pub(crate) const MAX_PEOPLE: usize = 16;

/** Most tests one flow holds */
pub(crate) const MAX_TESTS: usize = 128;

/** Longest text of one context item or test description (bytes) */
pub(crate) const MAX_TEXT: usize = 8 * 1024;

/** How urgent work on a flow is
 * Variants
    - P0 - critical: the most capable models
    - P1 - high
    - P2 - normal
    - P3 - low: the fastest, cheapest models
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Priority {
    P0,
    P1,
    P2,
    P3,
}

impl Priority {
    /** Every priority, most urgent first */
    pub(crate) const ALL: [Priority; 4] = [Self::P0, Self::P1, Self::P2, Self::P3];

    /** Parse a priority
     * Input
        - text: &str - "P0" to "P3" (any case)
     * Output
        - Result<Priority, String>
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_uppercase().as_str() {
            "P0" => Ok(Self::P0),
            "P1" => Ok(Self::P1),
            "P2" => Ok(Self::P2),
            "P3" => Ok(Self::P3),
            other => Err(format!("priority '{other}' is P0, P1, P2, or P3")),
        }
    }

    /** The priority's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::P0 => "P0",
            Self::P1 => "P1",
            Self::P2 => "P2",
            Self::P3 => "P3",
        }
    }
}

/** How capable a provider model is, as the organization ranks it
 * Variants
    - Frontier - the most capable
    - Balanced - capable and cheaper
    - Fast - quick and cheapest
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Tier {
    Frontier,
    Balanced,
    Fast,
}

impl Tier {
    /** Parse a tier
     * Input
        - text: &str - frontier, balanced, or fast
     * Output
        - Result<Tier, String>
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "frontier" => Ok(Self::Frontier),
            "balanced" => Ok(Self::Balanced),
            "fast" => Ok(Self::Fast),
            other => Err(format!("tier '{other}' is frontier, balanced, or fast")),
        }
    }

    /** The tier's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Frontier => "frontier",
            Self::Balanced => "balanced",
            Self::Fast => "fast",
        }
    }
}

/** One provider model the organization uses
 * Fields
    - provider: String - claude, codex, gemini, copilot, ...
    - model: String - the model id, such as claude-opus-5-5
    - tier: Tier - how capable it is
    - available: bool - it may be routed to now
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ModelEntry {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) tier: Tier,
    #[serde(default = "yes")]
    pub(crate) available: bool,
}

/** Serde default true */
fn yes() -> bool {
    true
}

/** The hard limits on what agents start by themselves; only a person changes them
 * Fields
    - max_watchers_per_session: u32 - watchers one agent session may start
    - max_watchers_per_flow: u32 - watchers active on one flow (a flow may set a lower number)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Limits {
    pub(crate) max_watchers_per_session: u32,
    pub(crate) max_watchers_per_flow: u32,
}

impl Default for Limits {
    /** Two watchers per session and three per flow
     * Input
        - None
     * Output
        - Limits
    */
    fn default() -> Self {
        Self {
            max_watchers_per_session: 2,
            max_watchers_per_flow: 3,
        }
    }
}

/** A kind of flow context
 * Variants
    - Query - a query that answers questions about the flow (SQL, a log or metrics query)
    - Metric - a measured value with an optional unit and threshold
    - Latency - an expected or observed latency, in milliseconds, with an optional budget
    - File - a repository file (and lines) to read before changing the flow
    - Note - a short fact
    - Link - a dashboard, runbook, or document
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ContextKind {
    Query,
    Metric,
    Latency,
    File,
    Note,
    Link,
}

impl ContextKind {
    /** Every kind */
    pub(crate) const ALL: [ContextKind; 6] = [
        Self::Query,
        Self::Metric,
        Self::Latency,
        Self::File,
        Self::Note,
        Self::Link,
    ];

    /** Parse a kind
     * Input
        - text: &str - query, metric, latency, file, note, or link
     * Output
        - Result<ContextKind, String>
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name() == text.trim().to_ascii_lowercase())
            .ok_or_else(|| {
                format!("context kind '{text}' is query, metric, latency, file, note, or link")
            })
    }

    /** The kind's name
     * Input
        - None (uses self)
     * Output
        - &'static str
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Query => "query",
            Self::Metric => "metric",
            Self::Latency => "latency",
            Self::File => "file",
            Self::Note => "note",
            Self::Link => "link",
        }
    }
}

/** One context item of a flow (guidance for agents and people; it never authorizes anything)
 * Fields
    - id: String - identifier within the flow
    - kind: ContextKind - kind
    - title: String - what it is
    - value: String - the query, value, path, note, or address
    - unit: Option<String> - unit of a metric (ms for latency)
    - threshold: Option<f64> - a limit the value should stay within (a latency budget)
    - file: Option<String> - a repository file it refers to
    - lines: Option<String> - lines of that file ("12-40")
    - added_by: String - who added it
    - added_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ContextItem {
    pub(crate) id: String,
    pub(crate) kind: ContextKind,
    pub(crate) title: String,
    pub(crate) value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) threshold: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) lines: Option<String>,
    pub(crate) added_by: String,
    pub(crate) added_at: u64,
}

/** What a person is to a flow
 * Variants
    - Author - wrote the code
    - Owner - answers for it
    - Reviewer - reviews changes to it
    - Oncall - is called on an incident
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Role {
    Author,
    Owner,
    Reviewer,
    Oncall,
}

impl Role {
    /** Parse a role
     * Input
        - text: &str - author, owner, reviewer, or oncall
     * Output
        - Result<Role, String>
    */
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "author" => Ok(Self::Author),
            "owner" => Ok(Self::Owner),
            "reviewer" => Ok(Self::Reviewer),
            "oncall" | "on-call" => Ok(Self::Oncall),
            other => Err(format!(
                "role '{other}' is author, owner, reviewer, or oncall"
            )),
        }
    }
}

/** A person designated on a flow
 * Fields
    - id: String - a short handle, unique within the flow (their GitHub login when known)
    - name: String - their name
    - role: Role - what they are to the flow
    - github: Option<String> - GitHub login
    - jira: Option<String> - Jira account or the issue that names them (ABC-123)
    - slack: Option<String> - Slack member or channel id
    - source: String - where it came from: manual, git, jira, or slack
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Person {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) github: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jira: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) slack: Option<String>,
    pub(crate) source: String,
}

/** A test approved for a flow
 * Fields
    - id: String - identifier within the flow
    - path: String - the test file
    - name: String - what it checks
    - description: String - in plain words, what it does
    - assumptions: Vec<String> - what it assumes
    - mock_data: String - the mock data it uses, described
    - command: Option<String> - how to run it (program and arguments; run only by a person or CI)
    - required: bool - every release must pass it ('crane flow tests check')
    - proposed_by: String - who wrote it (an agent session or a person)
    - approved_by: String - who approved it
    - approved_at: u64 - Unix milliseconds
    - digest: String - digest of the test file when it was approved
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FlowTest {
    pub(crate) id: String,
    pub(crate) path: String,
    pub(crate) name: String,
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) assumptions: Vec<String>,
    #[serde(default)]
    pub(crate) mock_data: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
    #[serde(default)]
    pub(crate) required: bool,
    pub(crate) proposed_by: String,
    pub(crate) approved_by: String,
    pub(crate) approved_at: u64,
    #[serde(default)]
    pub(crate) digest: String,
}

/** The trace a person approved for a flow
 * Fields
    - from: Vec<String> - the starting points traced
    - max_depth: usize - deepest call chain followed
    - revision: u32 - the draft revision approved
    - digest: String - digest of the traced graph (drift shows when it no longer matches)
    - nodes: usize - functions in the flow
    - edges: usize - calls between them
    - files: Vec<String> - files holding them
    - status: String - RESOLVED or PARTIAL
    - approved_by: String - who approved it
    - approved_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ApprovedTrace {
    pub(crate) from: Vec<String>,
    pub(crate) max_depth: usize,
    pub(crate) revision: u32,
    pub(crate) digest: String,
    pub(crate) nodes: usize,
    pub(crate) edges: usize,
    pub(crate) files: Vec<String>,
    pub(crate) status: String,
    pub(crate) approved_by: String,
    pub(crate) approved_at: u64,
}

/** What the repository knows about one flow
 * Fields
    - priority: Option<Priority> - how urgent work on it is (routes the model)
    - enforce_model: bool - a call in the flow from a model outside the routed tiers needs a person
    - people: Vec<Person> - designated people
    - context: Vec<ContextItem> - context items
    - tests: Vec<FlowTest> - approved tests
    - trace: Option<ApprovedTrace> - the approved trace
    - max_watchers: Option<u32> - watchers allowed on this flow (at most the repository limit)
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FlowKnowledge {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) priority: Option<Priority>,
    pub(crate) enforce_model: bool,
    pub(crate) people: Vec<Person>,
    pub(crate) context: Vec<ContextItem>,
    pub(crate) tests: Vec<FlowTest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) trace: Option<ApprovedTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_watchers: Option<u32>,
}

/** The content of .crane/flows.json
 * Fields
    - version: u32 - layout version
    - models: Vec<ModelEntry> - the organization's provider models
    - routing: BTreeMap<Priority, Vec<Tier>> - the tiers each priority may use, best first (a
      priority missing here uses the default routing)
    - limits: Limits - hard limits on watchers agents start
    - flows: BTreeMap<String, FlowKnowledge> - knowledge by flow id
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct FlowBook {
    pub(crate) version: u32,
    pub(crate) models: Vec<ModelEntry>,
    pub(crate) routing: BTreeMap<Priority, Vec<Tier>>,
    pub(crate) limits: Limits,
    pub(crate) flows: BTreeMap<String, FlowKnowledge>,
}

impl Default for FlowBook {
    /** An empty book
     * Input
        - None
     * Output
        - FlowBook
    */
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            models: Vec::new(),
            routing: BTreeMap::new(),
            limits: Limits::default(),
            flows: BTreeMap::new(),
        }
    }
}

impl FlowBook {
    /** Read the book from a loaded state (empty when there is none)
     * Input
        - state: &State - governance state
     * Output
        - Result<FlowBook, String>
        - Error when the store is malformed or of another layout version
    */
    pub(crate) fn of(state: &State) -> Result<Self, String> {
        let Some(value) = &state.flows else {
            return Ok(Self::default());
        };
        let book: Self = serde_json::from_value(value.clone()).map_err(|error| {
            format!(
                ".crane/{} is malformed: {error}",
                crate::governance::state::FLOWS_FILE
            )
        })?;
        if book.version != STORE_VERSION {
            return Err(format!(
                ".crane/{} has layout version {}; this Crane reads {STORE_VERSION}",
                crate::governance::state::FLOWS_FILE,
                book.version
            ));
        }
        Ok(book)
    }

    /** The book as stored
     * Input
        - None (uses self)
     * Output
        - Value
    */
    pub(crate) fn value(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }

    /** The watcher limit of a flow: its own number, never above the repository's
     * Input
        - flow: &str - flow id
     * Output
        - u32
    */
    pub(crate) fn watcher_limit(&self, flow: &str) -> u32 {
        let general = self.limits.max_watchers_per_flow;
        self.flows
            .get(flow)
            .and_then(|knowledge| knowledge.max_watchers)
            .map_or(general, |own| own.min(general))
    }
}

/** The tiers a priority may use when the book does not say: P0 frontier; P1 frontier, then
 * balanced; P2 balanced, then frontier; P3 fast, then balanced
 * Input
    - priority: Priority - priority
 * Output
    - Vec<Tier>
*/
pub(crate) fn default_routing(priority: Priority) -> Vec<Tier> {
    match priority {
        Priority::P0 => vec![Tier::Frontier],
        Priority::P1 => vec![Tier::Frontier, Tier::Balanced],
        Priority::P2 => vec![Tier::Balanced, Tier::Frontier],
        Priority::P3 => vec![Tier::Fast, Tier::Balanced],
    }
}

/** The model a priority routes to: the first available model of the first tier that has one,
 * keeping the organization's order inside a tier (deterministic)
 * Input
    - book: &FlowBook - flow knowledge
    - priority: Priority - priority
    - provider: Option<&str> - prefer this provider when it has a model of the chosen tier
 * Output
    - (Vec<Tier>, Option<&ModelEntry>) the allowed tiers and the chosen model (None when the
      organization lists no available model of those tiers)
*/
pub(crate) fn route<'a>(
    book: &'a FlowBook,
    priority: Priority,
    provider: Option<&str>,
) -> (Vec<Tier>, Option<&'a ModelEntry>) {
    let tiers = book
        .routing
        .get(&priority)
        .filter(|tiers| !tiers.is_empty())
        .cloned()
        .unwrap_or_else(|| default_routing(priority));
    for tier in &tiers {
        let of_tier = book
            .models
            .iter()
            .filter(|entry| entry.available && entry.tier == *tier)
            .collect::<Vec<_>>();
        let preferred = provider.and_then(|provider| {
            of_tier
                .iter()
                .find(|entry| entry.provider.eq_ignore_ascii_case(provider))
        });
        if let Some(chosen) = preferred.or(of_tier.first()) {
            return (tiers, Some(chosen));
        }
    }
    (tiers, None)
}

/** The tier the organization gives a model, if it lists it
 * Input
    - book: &FlowBook - flow knowledge
    - model: &str - model id as the agent reports it
 * Output
    - Option<Tier>
*/
pub(crate) fn tier_of(book: &FlowBook, model: &str) -> Option<Tier> {
    book.models
        .iter()
        .find(|entry| entry.model.eq_ignore_ascii_case(model))
        .map(|entry| entry.tier)
}
