// Queries over specification events (DASHBOARD.md), shared by the hosted dashboard and the alert
// rules on a member's machine. A query is a time range, free text, and any number of clauses; each
// clause names an attribute by its path ("severity.severity_status", "flow", "eval.latency_ms",
// "decision.decision_reasons"), an operator, and values. Clauses combine with AND, except clauses
// that share a group number, which combine with OR (so "(severity is HIGH or tool is BASH) and
// flow is F05" is two clauses in group 1 and one without a group). The values of one clause
// combine with OR (or, for negative operators, none may match). A list attribute matches when any
// of its items does.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/** The facets the dashboard offers as shortcuts, and the attribute each one reads */
pub(crate) const FACETS: [(&str, &str); 12] = [
    ("severity", "severity.severity_status"),
    ("flow", "flow"),
    ("decision", "decision.decision"),
    ("enforcement", "decision.enforcement"),
    ("alert", "severity.alert_kind"),
    ("origin", "origin"),
    ("agent", "agent.agent_type"),
    ("source", "action.action_source"),
    ("event", "event_name"),
    ("response", "response.response_status"),
    ("tool", "action.action_tool"),
    ("member", "member_key"),
];

/** How a clause compares an attribute with its values
 * Variants
    - Is - equals one of the values (case-insensitive)
    - Not - equals none of the values (an absent attribute matches)
    - Contains - contains one of the values (case-insensitive)
    - NotContains - contains none of the values
    - Prefix - starts with one of the values
    - Gt, Gte, Lt, Lte - numeric comparison with the first value
    - Exists - present and not empty
    - Missing - absent, null, or empty
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Op {
    Is,
    Not,
    Contains,
    NotContains,
    Prefix,
    Gt,
    Gte,
    Lt,
    Lte,
    Exists,
    Missing,
}

/** One condition on one attribute
 * Fields
    - path: String - attribute path, dot separated
    - op: Op - comparison
    - values: Vec<String> - values to compare with (none for exists and missing)
    - group: Option<u32> - clauses with the same group combine with OR; none stands alone
*/
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Clause {
    pub(crate) path: String,
    pub(crate) op: Op,
    #[serde(default)]
    pub(crate) values: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) group: Option<u32>,
}

/** A query over events
 * Fields
    - since, until: Option<u64> - time range in Unix milliseconds, inclusive
    - text: Option<String> - free text, matched anywhere in the event (case-insensitive)
    - clauses: Vec<Clause> - conditions, all of which must hold
*/
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Query {
    #[serde(default)]
    pub(crate) since: Option<u64>,
    #[serde(default)]
    pub(crate) until: Option<u64>,
    #[serde(default)]
    pub(crate) text: Option<String>,
    #[serde(default)]
    pub(crate) clauses: Vec<Clause>,
}

/** Read the values of an attribute: a string, number, or boolean as one value, each item of a
 * list as one value, nothing for an absent or null attribute
 * Input
    - spec: &Value - event
    - path: &str - attribute path
 * Output
    - Vec<String>
*/
pub(crate) fn values(spec: &Value, path: &str) -> Vec<String> {
    let mut node = spec;
    for part in path.split('.') {
        node = &node[part];
    }
    let scalar = |value: &Value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Null => None,
        other => Some(other.to_string()),
    };
    match node {
        Value::Array(items) => items.iter().filter_map(scalar).collect(),
        other => scalar(other).into_iter().collect(),
    }
}

impl Clause {
    /** Report whether an event satisfies the clause
     * Input
        - spec: &Value - event
     * Output
        - bool
    */
    pub(crate) fn holds(&self, spec: &Value) -> bool {
        let found = values(spec, &self.path);
        let present = found
            .iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        let wanted = || {
            self.values
                .iter()
                .map(|value| value.trim().to_ascii_lowercase())
        };
        let lower = || {
            present
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect::<Vec<_>>()
        };
        let number = || present.iter().find_map(|value| value.parse::<f64>().ok());
        let limit = || {
            self.values
                .first()
                .and_then(|value| value.trim().parse::<f64>().ok())
        };
        let compare = |test: fn(f64, f64) -> bool| matches!((number(), limit()), (Some(a), Some(b)) if test(a, b));
        match self.op {
            Op::Is => lower()
                .iter()
                .any(|value| wanted().any(|wanted| *value == wanted)),
            Op::Not => !lower()
                .iter()
                .any(|value| wanted().any(|wanted| *value == wanted)),
            Op::Contains => lower()
                .iter()
                .any(|value| wanted().any(|wanted| value.contains(&wanted))),
            Op::NotContains => !lower()
                .iter()
                .any(|value| wanted().any(|wanted| value.contains(&wanted))),
            Op::Prefix => lower()
                .iter()
                .any(|value| wanted().any(|wanted| value.starts_with(&wanted))),
            Op::Gt => compare(|a, b| a > b),
            Op::Gte => compare(|a, b| a >= b),
            Op::Lt => compare(|a, b| a < b),
            Op::Lte => compare(|a, b| a <= b),
            Op::Exists => !present.is_empty(),
            Op::Missing => present.is_empty(),
        }
    }
}

impl Query {
    /** Read a query from request parameters: since, until, q (free text), filter (a JSON list of
     * clauses), each facet shortcut (comma-separated values, as an Is clause), and the shortcuts
     * session, task, policy, selection, and name (event name prefix)
     * Input
        - params: &BTreeMap<String, String> - parameters
     * Output
        - Result<Query, String>
        - Error if filter is not a valid list of clauses
    */
    #[cfg_attr(not(feature = "server"), allow(dead_code))] // used by the hosted dashboard
    pub(crate) fn from_params(params: &BTreeMap<String, String>) -> Result<Self, String> {
        let number = |key: &str| {
            params
                .get(key)
                .and_then(|value| value.parse::<u64>().ok())
                .filter(|value| *value > 0)
        };
        let mut clauses: Vec<Clause> =
            match params.get("filter").filter(|text| !text.trim().is_empty()) {
                Some(text) => serde_json::from_str(text)
                    .map_err(|error| format!("filter is not a valid list of clauses: {error}"))?,
                None => Vec::new(),
            };
        let list = |text: &str| {
            text.split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(String::from)
                .collect::<Vec<_>>()
        };
        for (facet, path) in FACETS {
            if let Some(text) = params.get(facet).filter(|text| !text.trim().is_empty()) {
                clauses.push(Clause {
                    path: path.into(),
                    op: Op::Is,
                    values: list(text),
                    group: None,
                });
            }
        }
        for (key, path) in [
            ("session", "session.session_id"),
            ("task", "session.task_id"),
        ] {
            if let Some(text) = params.get(key).filter(|text| !text.trim().is_empty()) {
                clauses.push(Clause {
                    path: path.into(),
                    op: Op::Is,
                    values: list(text),
                    group: None,
                });
            }
        }
        if let Some(prefix) = params
            .get("name")
            .or_else(|| params.get("type"))
            .filter(|text| !text.trim().is_empty())
        {
            clauses.push(Clause {
                path: "event_name".into(),
                op: Op::Prefix,
                values: vec![prefix.clone()],
                group: None,
            });
        }
        Ok(Self {
            since: number("since"),
            until: number("until"),
            text: params
                .get("q")
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty()),
            clauses,
        })
    }

    /** Report whether an event passes the query
     * Input
        - spec: &Value - event
        - skip: Option<&str> - an attribute path whose clauses are ignored (so a facet's own counts
          show its other values)
     * Output
        - bool
    */
    pub(crate) fn matches(&self, spec: &Value, skip: Option<&str>) -> bool {
        let at = spec["occurred_at"].as_u64().unwrap_or(0);
        if self.since.is_some_and(|since| at < since) || self.until.is_some_and(|until| at > until)
        {
            return false;
        }
        let active = self
            .clauses
            .iter()
            .filter(|clause| Some(clause.path.as_str()) != skip);
        let mut groups: BTreeMap<u32, bool> = BTreeMap::new();
        for clause in active {
            match clause.group {
                None if !clause.holds(spec) => return false,
                None => {}
                Some(group) => {
                    let held = groups.entry(group).or_insert(false);
                    *held = *held || clause.holds(spec);
                }
            }
        }
        if groups.values().any(|held| !held) {
            return false;
        }
        match &self.text {
            Some(text) => spec
                .to_string()
                .to_ascii_lowercase()
                .contains(&text.to_ascii_lowercase()),
            None => true,
        }
    }
}

/** The attribute a facet shortcut reads
 * Input
    - facet: &str - facet name
 * Output
    - Option<&'static str>
*/
pub(crate) fn facet_path(facet: &str) -> Option<&'static str> {
    FACETS
        .iter()
        .find(|(name, _)| *name == facet)
        .map(|(_, path)| *path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /** An event with the attributes the tests combine
     * Input
        - severity, flow, decision, tool: &str - attributes
        - latency: u64 - eval.latency_ms
        - reasons: &[&str] - decision.decision_reasons
        - at: u64 - occurred_at
     * Output
        - Value
    */
    fn event(
        severity: &str,
        flow: &str,
        decision: &str,
        tool: &str,
        latency: u64,
        reasons: &[&str],
        at: u64,
    ) -> Value {
        json!({
            "event_name": format!("{flow}_EVENT"), "flow": flow, "occurred_at": at, "origin": "MACHINE",
            "severity": {"severity_status": severity}, "decision": {"decision": decision, "decision_reasons": reasons},
            "action": {"action_tool": tool}, "eval": {"latency_ms": latency}, "response": {"response_statement": format!("{tool} handled")},
            "session": {"session_id": "s1"},
        })
    }

    /** Every combination of operators over every attribute kind (scalar, list, number, absent),
     * combined with AND across clauses and OR within one, with and without time and text
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn clauses_combine_in_every_way() {
        let events = [
            event(
                "HIGH",
                "F05",
                "DENY",
                "BASH",
                120,
                &["PRESERVED_SELECTION", "SCRIPT_TREE_DELETION"],
                1_000,
            ),
            event("INFO", "F05", "ALLOW", "EDIT", 40, &[], 2_000),
            event(
                "CRITICAL",
                "F06",
                "NOT_REQUIRED",
                "WRITE",
                300,
                &["MARKER_FORGED"],
                3_000,
            ),
            event("LOW", "F13", "NOT_REQUIRED", "", 0, &[], 4_000),
        ];
        let count = |clauses: Vec<Clause>| {
            let query = Query {
                clauses,
                ..Query::default()
            };
            events
                .iter()
                .filter(|spec| query.matches(spec, None))
                .count()
        };
        let clause = |path: &str, op: Op, values: &[&str]| Clause {
            path: path.into(),
            op,
            values: values.iter().map(|value| value.to_string()).collect(),
            group: None,
        };
        let grouped = |group: u32, path: &str, op: Op, values: &[&str]| Clause {
            group: Some(group),
            ..clause(path, op, values)
        };
        // One operator at a time
        assert_eq!(
            count(vec![clause(
                "severity.severity_status",
                Op::Is,
                &["high", "critical"]
            )]),
            2,
            "OR within a clause, any case"
        );
        assert_eq!(
            count(vec![clause(
                "severity.severity_status",
                Op::Not,
                &["INFO", "LOW"]
            )]),
            2
        );
        assert_eq!(
            count(vec![clause(
                "decision.decision_reasons",
                Op::Is,
                &["MARKER_FORGED"]
            )]),
            1,
            "a list matches by any item"
        );
        assert_eq!(
            count(vec![clause(
                "decision.decision_reasons",
                Op::Contains,
                &["selection"]
            )]),
            1
        );
        assert_eq!(
            count(vec![clause(
                "decision.decision_reasons",
                Op::NotContains,
                &["selection"]
            )]),
            3
        );
        assert_eq!(count(vec![clause("event_name", Op::Prefix, &["f0"])]), 3);
        assert_eq!(count(vec![clause("eval.latency_ms", Op::Gt, &["100"])]), 2);
        assert_eq!(count(vec![clause("eval.latency_ms", Op::Gte, &["120"])]), 2);
        assert_eq!(count(vec![clause("eval.latency_ms", Op::Lt, &["40"])]), 1);
        assert_eq!(count(vec![clause("eval.latency_ms", Op::Lte, &["40"])]), 2);
        assert_eq!(
            count(vec![clause("action.action_tool", Op::Exists, &[])]),
            3,
            "an empty string is not present"
        );
        assert_eq!(
            count(vec![clause("action.action_tool", Op::Missing, &[])]),
            1
        );
        assert_eq!(
            count(vec![clause("other.nothing", Op::Missing, &[])]),
            4,
            "an absent attribute is missing"
        );
        assert_eq!(
            count(vec![clause("other.nothing", Op::Not, &["x"])]),
            4,
            "an absent attribute is not any value"
        );
        assert_eq!(
            count(vec![clause("eval.latency_ms", Op::Gt, &["not a number"])]),
            0,
            "a bad number matches nothing"
        );
        // AND across clauses, mixing every kind
        assert_eq!(
            count(vec![
                clause("flow", Op::Is, &["F05", "F06"]),
                clause("severity.severity_status", Op::Not, &["INFO"]),
                clause("eval.latency_ms", Op::Gte, &["120"])
            ]),
            2
        );
        assert_eq!(
            count(vec![
                clause("flow", Op::Is, &["F05"]),
                clause("decision.decision", Op::Is, &["DENY"]),
                clause("decision.decision_reasons", Op::Contains, &["tree"]),
                clause("action.action_tool", Op::Is, &["bash"])
            ]),
            1
        );
        assert_eq!(
            count(vec![
                clause("flow", Op::Is, &["F05"]),
                clause("flow", Op::Not, &["F05"])
            ]),
            0,
            "contradictions match nothing"
        );
        assert_eq!(
            count(vec![
                clause("severity.severity_status", Op::Exists, &[]),
                clause("action.action_tool", Op::Missing, &[]),
                clause("flow", Op::Prefix, &["F1"])
            ]),
            1
        );
        // OR across attributes within a group, AND between groups and with ungrouped clauses
        assert_eq!(
            count(vec![
                grouped(1, "severity.severity_status", Op::Is, &["CRITICAL"]),
                grouped(1, "action.action_tool", Op::Is, &["EDIT"])
            ]),
            2,
            "(CRITICAL or EDIT)"
        );
        assert_eq!(
            count(vec![
                grouped(1, "severity.severity_status", Op::Is, &["CRITICAL"]),
                grouped(1, "action.action_tool", Op::Is, &["EDIT"]),
                clause("flow", Op::Is, &["F05"])
            ]),
            1,
            "(CRITICAL or EDIT) and F05"
        );
        assert_eq!(
            count(vec![
                grouped(1, "flow", Op::Is, &["F13"]),
                grouped(1, "eval.latency_ms", Op::Gt, &["200"]),
                grouped(2, "severity.severity_status", Op::Is, &["LOW", "CRITICAL"]),
                grouped(2, "decision.decision", Op::Is, &["DENY"])
            ]),
            2,
            "(F13 or slow) and (LOW/CRITICAL or DENY)"
        );
        assert_eq!(
            count(vec![
                grouped(1, "flow", Op::Is, &["F99"]),
                grouped(1, "origin", Op::Not, &["MACHINE"])
            ]),
            0,
            "a group with no holding clause fails"
        );
        // Time and free text with clauses
        let query = Query {
            since: Some(1_500),
            until: Some(3_500),
            text: Some("WRITE handled".into()),
            clauses: vec![clause("severity.severity_status", Op::Is, &["CRITICAL"])],
        };
        assert_eq!(
            events
                .iter()
                .filter(|spec| query.matches(spec, None))
                .count(),
            1
        );
        let skipped = Query {
            clauses: vec![
                clause("severity.severity_status", Op::Is, &["CRITICAL"]),
                clause("flow", Op::Is, &["F05", "F06"]),
            ],
            ..Query::default()
        };
        assert_eq!(
            events
                .iter()
                .filter(|spec| skipped.matches(spec, Some("severity.severity_status")))
                .count(),
            3,
            "a skipped path is ignored"
        );
    }

    /** Parameters become clauses: shortcuts, a JSON filter, and both together
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn parameters_become_clauses() {
        let mut params = BTreeMap::new();
        params.insert("severity".to_string(), "HIGH, CRITICAL".to_string());
        params.insert("session".to_string(), "s1".to_string());
        params.insert("since".to_string(), "10".to_string());
        params.insert("filter".to_string(), r#"[{"path":"eval.latency_ms","op":"gt","values":["100"]},{"path":"origin","op":"not","values":["SERVICE"]}]"#.to_string());
        let query = Query::from_params(&params).unwrap();
        assert_eq!(query.since, Some(10));
        assert_eq!(query.clauses.len(), 4);
        assert!(query
            .clauses
            .iter()
            .any(|clause| clause.path == "severity.severity_status"
                && clause.values == ["HIGH", "CRITICAL"]));
        params.insert("filter".to_string(), "not json".to_string());
        assert!(Query::from_params(&params).is_err());
        assert_eq!(facet_path("tool"), Some("action.action_tool"));
    }
}
