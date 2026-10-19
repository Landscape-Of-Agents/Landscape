// The deterministic fixture connector: serves a fixed list of source records with the same
// contract as a live connector, and can be told to fail in each structured way (unavailable,
// unauthorized, rate limited, timeout, partial). It stands in for sources this repository has no
// live integration with (GitHub pull requests, CI, deployments, runtime logs and metrics), so the
// whole pipeline can be exercised without credentials. It never claims to be a live source: its
// snapshots are prefixed "fixture:".

use std::cell::Cell;

use super::connector::{
    page, snapshot_of, ConnectorOutcome, ConnectorStatus, HealthReport, QuerySpec,
    ResourceConnector, SourceRecord,
};

/** How a fixture connector behaves
 * Variants
    - Normal - answer from its records
    - Fail(ConnectorStatus) - fail every operation with this status
    - Partial(usize) - return at most this many records of each answer, marked partial
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FixtureBehavior {
    Normal,
    Fail(ConnectorStatus),
    #[cfg_attr(not(test), allow(dead_code))]
    Partial(usize),
}

/** A fixture-backed connector
 * Fields
    - records: Vec<SourceRecord> - the records it serves
    - behavior: FixtureBehavior - normal or failing
    - calls: Cell<usize> - operations performed (tests check that a denied query never ran)
*/
#[derive(Debug)]
pub(crate) struct FixtureConnector {
    records: Vec<SourceRecord>,
    behavior: FixtureBehavior,
    calls: Cell<usize>,
}

impl FixtureConnector {
    /** Build a connector serving records
     * Input
        - records: Vec<SourceRecord> - records
     * Output
        - FixtureConnector
    */
    pub(crate) fn new(records: Vec<SourceRecord>) -> Self {
        Self {
            records,
            behavior: FixtureBehavior::Normal,
            calls: Cell::new(0),
        }
    }

    /** Change how the connector behaves
     * Input
        - behavior: FixtureBehavior - behavior
     * Output
        - FixtureConnector
    */
    pub(crate) fn with_behavior(mut self, behavior: FixtureBehavior) -> Self {
        self.behavior = behavior;
        self
    }

    /** Count the operations performed
     * Input
        - None (uses self)
     * Output
        - usize
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn calls(&self) -> usize {
        self.calls.get()
    }

    /** The snapshot identifier of the current records
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn snapshot(&self) -> String {
        format!("fixture:{}", snapshot_of(&self.records))
    }

    /** Apply the configured behavior to an answer
     * Input
        - outcome: ConnectorOutcome - answer from the records
     * Output
        - ConnectorOutcome
    */
    fn apply(&self, mut outcome: ConnectorOutcome) -> ConnectorOutcome {
        self.calls.set(self.calls.get() + 1);
        match self.behavior {
            FixtureBehavior::Normal => outcome,
            FixtureBehavior::Fail(status) => ConnectorOutcome::failed(
                status,
                format!("fixture configured to fail with {status:?}"),
            ),
            FixtureBehavior::Partial(keep) => {
                if outcome.records.len() > keep {
                    outcome.records.truncate(keep);
                    outcome.status = ConnectorStatus::Partial;
                    outcome.error = Some(format!("only {keep} records could be retrieved"));
                }
                outcome
            }
        }
    }
}

impl ResourceConnector for FixtureConnector {
    /** Describe the fixture's records (identifiers and types only)
     * Input
        - None (uses self)
     * Output
        - ConnectorOutcome
    */
    fn discover(&self) -> ConnectorOutcome {
        let described = self
            .records
            .iter()
            .map(|record| SourceRecord {
                content: serde_json::Value::Null,
                ..record.clone()
            })
            .collect();
        self.apply(ConnectorOutcome::ok(described, Some(self.snapshot())))
    }

    /** Return the record with an identifier, or an empty Ok answer when there is none
     * Input
        - record_id: &str - identifier
     * Output
        - ConnectorOutcome
    */
    fn fetch(&self, record_id: &str) -> ConnectorOutcome {
        let found = self
            .records
            .iter()
            .filter(|record| record.record_id == record_id)
            .take(1)
            .cloned()
            .collect();
        self.apply(ConnectorOutcome::ok(found, Some(self.snapshot())))
    }

    /** Answer a query from the records
     * Input
        - spec: &QuerySpec - query
     * Output
        - ConnectorOutcome
    */
    fn query(&self, spec: &QuerySpec) -> ConnectorOutcome {
        self.apply(page(&self.records, spec, Some(self.snapshot())))
    }

    /** Report availability: a connector configured to fail is unavailable
     * Input
        - None (uses self)
     * Output
        - HealthReport
    */
    fn health_check(&self) -> HealthReport {
        match self.behavior {
            FixtureBehavior::Fail(status) => HealthReport {
                available: false,
                detail: format!("fixture configured to fail with {status:?}"),
            },
            _ => HealthReport {
                available: true,
                detail: format!("fixture with {} records", self.records.len()),
            },
        }
    }
}
