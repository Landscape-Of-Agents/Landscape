// The adapter registry. Every operation an invocation can perform is an explicitly registered,
// typed operation: an adapter id, an operation id, an effect class, a parameter schema (typed,
// bounded values; no command, script, or URL template is ever accepted), whether it is idempotent
// and reconcilable, whether it needs a durable transaction, and an output limit. Adapters dispatch,
// reconcile against their authoritative state, and report health.
//
// Provided adapters:
//   FixtureAdapter  - a deterministic, in-memory external system (read and state-changing
//                     operations) with injectable faults: timeouts before dispatch, after dispatch,
//                     and after the effect happened; failures with known and unknown effects
//   GitReadAdapter  - one read-only operation over the existing Git connector (git.commit)
//
// Durable transactions, guarantees, compensation, and recovery belong to an external transaction
// runtime that this repository does not contain; TransactionRuntime is the narrow interface to it.
// UnavailableRuntime reports it absent, so operations that require it are blocked (fail closed);
// FakeRuntime is a deterministic stand-in for tests. Compensation is recorded separately and is
// never treated as proof that the original effect did not happen.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::lifecycle::{EffectClass, EffectStatus};
use crate::capability::model::StepKind;
use crate::capture::resources::connector::ResourceConnector;
use crate::capture::resources::local::GitConnector;

/** Types of operation parameters
 * Variants
    - Identifier - 1-128 characters of letters, digits, and . _ : / @ -
    - Text - bounded text without control characters
    - Integer - a non-negative integer
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ParamKind {
    Identifier,
    Text,
    Integer,
}

/** One parameter of an operation schema
 * Fields
    - name: String - name
    - kind: ParamKind - type
    - required: bool - required
    - max_len: usize - longest value
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ParamSpec {
    pub(crate) name: String,
    pub(crate) kind: ParamKind,
    pub(crate) required: bool,
    pub(crate) max_len: usize,
}

/** A registered, typed operation
 * Fields
    - adapter_id: String - adapter
    - operation_id: String - operation
    - description: String - what it does
    - effect: EffectClass - effect class
    - step_kinds: Vec<StepKind> - procedure step kinds it may implement
    - params: Vec<ParamSpec> - parameter schema
    - idempotent: bool - repeating it with the same key has no further effect
    - reconcilable: bool - the adapter can establish whether it took effect
    - requires_transaction: bool - it must run inside a durable transaction
    - max_output_bytes: usize - largest normalized result
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OperationSpec {
    pub(crate) adapter_id: String,
    pub(crate) operation_id: String,
    pub(crate) description: String,
    pub(crate) effect: EffectClass,
    pub(crate) step_kinds: Vec<StepKind>,
    pub(crate) params: Vec<ParamSpec>,
    pub(crate) idempotent: bool,
    pub(crate) reconcilable: bool,
    pub(crate) requires_transaction: bool,
    pub(crate) max_output_bytes: usize,
}

/** Validate parameters against a schema: no unknown or missing parameters, typed bounded values
 * Input
    - spec: &OperationSpec - schema
    - params: &BTreeMap<String, String> - parameters
 * Output
    - Result<(), String> - the first problem
*/
pub(crate) fn validate_params(
    spec: &OperationSpec,
    params: &BTreeMap<String, String>,
) -> Result<(), String> {
    for name in params.keys() {
        if !spec.params.iter().any(|param| &param.name == name) {
            return Err(format!(
                "{}.{} has no parameter '{name}'",
                spec.adapter_id, spec.operation_id
            ));
        }
    }
    for param in &spec.params {
        let Some(value) = params.get(&param.name) else {
            if param.required {
                return Err(format!("parameter '{}' is required", param.name));
            }
            continue;
        };
        let valid = !value.is_empty()
            && value.len() <= param.max_len
            && match param.kind {
                ParamKind::Identifier => value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "._:/@-".contains(character)
                }),
                ParamKind::Text => !value.chars().any(char::is_control),
                ParamKind::Integer => {
                    value.len() <= 18 && value.chars().all(|character| character.is_ascii_digit())
                }
            };
        if !valid {
            return Err(format!(
                "parameter '{}' is not a valid {:?} of at most {} characters",
                param.name, param.kind, param.max_len
            ));
        }
    }
    Ok(())
}

/** The outcome of a dispatch
 * Variants
    - Completed - the adapter reported success
    - Failed - the adapter reported failure; effect says whether anything happened
    - TimedOut - no answer in time; dispatched says whether the request left the adapter
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum DispatchOutcome {
    Completed {
        result: Value,
        external_ref: Option<String>,
    },
    Failed {
        error: String,
        effect: EffectStatus,
    },
    TimedOut {
        dispatched: bool,
        external_ref: Option<String>,
    },
}

/** The outcome of a reconciliation against the authoritative state
 * Variants
    - Applied - the effect happened
    - NotApplied - the effect did not happen
    - Unavailable - the adapter cannot establish it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reconciliation", rename_all = "snake_case")]
pub(crate) enum Reconciliation {
    Applied {
        result: Value,
        external_ref: Option<String>,
    },
    NotApplied,
    Unavailable {
        reason: String,
    },
}

/** The contract of an invocation adapter */
pub(crate) trait InvocationAdapter {
    /** The adapter's identifier
     * Input
        - None (uses self)
     * Output
        - String
    */
    fn adapter_id(&self) -> String;

    /** The operations it offers
     * Input
        - None (uses self)
     * Output
        - Vec<OperationSpec>
    */
    fn operations(&self) -> Vec<OperationSpec>;

    /** Report whether it is available
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn healthy(&self) -> bool;

    /** The namespace of the resources it acts on ("repo" for the repository), which the
     * permission check resolves; None when it declares none (resources then stay unsupported)
     * Input
        - None (uses self)
     * Output
        - Option<&'static str>
    */
    fn resource_namespace(&self) -> Option<&'static str> {
        None
    }

    /** Dispatch an operation with validated parameters
     * Input
        - operation: &str - operation id
        - params: &BTreeMap<String, String> - parameters
        - key: &str - the operation's idempotency key
     * Output
        - DispatchOutcome
    */
    fn dispatch(
        &self,
        operation: &str,
        params: &BTreeMap<String, String>,
        key: &str,
    ) -> DispatchOutcome;

    /** Establish whether an operation took effect
     * Input
        - operation: &str - operation id
        - key: &str - its idempotency key
        - external_ref: Option<&str> - its external reference, when known
     * Output
        - Reconciliation
    */
    fn reconcile(&self, operation: &str, key: &str, external_ref: Option<&str>) -> Reconciliation;
}

/** The registered adapters
 * Fields
    - adapters: BTreeMap<String, Rc<dyn InvocationAdapter>> - by adapter id
*/
#[derive(Clone, Default)]
pub(crate) struct AdapterRegistry {
    adapters: BTreeMap<String, Rc<dyn InvocationAdapter>>,
}

impl AdapterRegistry {
    /** Register an adapter
     * Input
        - adapter: Rc<dyn InvocationAdapter> - adapter
     * Output
        - AdapterRegistry
    */
    pub(crate) fn with(mut self, adapter: Rc<dyn InvocationAdapter>) -> Self {
        self.adapters.insert(adapter.adapter_id(), adapter);
        self
    }

    /** Find an adapter
     * Input
        - adapter_id: &str - adapter
     * Output
        - Option<&Rc<dyn InvocationAdapter>>
    */
    pub(crate) fn adapter(&self, adapter_id: &str) -> Option<&Rc<dyn InvocationAdapter>> {
        self.adapters.get(adapter_id)
    }

    /** Find a registered operation
     * Input
        - adapter_id: &str - adapter
        - operation_id: &str - operation
     * Output
        - Option<OperationSpec>
    */
    pub(crate) fn operation(&self, adapter_id: &str, operation_id: &str) -> Option<OperationSpec> {
        self.adapters
            .get(adapter_id)?
            .operations()
            .into_iter()
            .find(|spec| spec.operation_id == operation_id)
    }

    /** Every registered operation, ordered by adapter
     * Input
        - None (uses self)
     * Output
        - Vec<OperationSpec>
    */
    pub(crate) fn operations(&self) -> Vec<OperationSpec> {
        self.adapters
            .values()
            .flat_map(|adapter| adapter.operations())
            .collect()
    }
}

/** A fault injected into a fixture operation
 * Variants
    - TimeoutBeforeDispatch - times out before the request leaves (no effect)
    - TimeoutAfterDispatch - times out after dispatch, and the effect did not happen
    - TimeoutAfterEffect - times out after the effect happened
    - Fail - fails with no effect
    - FailUnknown - fails without knowing whether the effect happened (it did not)
*/
#[cfg_attr(not(test), allow(dead_code))] // fault injection and test doubles for the fixture systems; exercised by the tests
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    TimeoutBeforeDispatch,
    TimeoutAfterDispatch,
    TimeoutAfterEffect,
    Fail,
    FailUnknown,
}

/** Mutable state of a fixture system
 * Fields
    - effects: BTreeMap<String, Value> - applied effects by idempotency key
    - calls: BTreeMap<String, usize> - dispatches per operation
    - faults: BTreeMap<String, VecDeque<Fault>> - queued faults per operation
    - reconcile_available: bool - whether reconciliation works
    - healthy: bool - whether the system is available
*/
#[derive(Debug, Default)]
struct FixtureState {
    effects: BTreeMap<String, Value>,
    calls: BTreeMap<String, usize>,
    faults: BTreeMap<String, VecDeque<Fault>>,
    reconcile_available: bool,
    healthy: bool,
}

/** A deterministic, in-memory external system
 * Fields
    - id: String - adapter id
    - specs: Vec<OperationSpec> - its operations
    - responses: BTreeMap<String, Value> - read results by operation
    - state: RefCell<FixtureState> - effects, calls, faults
*/
pub(crate) struct FixtureAdapter {
    id: String,
    specs: Vec<OperationSpec>,
    responses: BTreeMap<String, Value>,
    state: RefCell<FixtureState>,
}

/** Build an operation spec
 * Input
    - adapter: &str - adapter id
    - operation: &str - operation id
    - effect: EffectClass - effect
    - step_kinds: &[StepKind] - step kinds it may implement
    - params: &[(&str, ParamKind)] - required parameters
    - flags: (bool, bool, bool) - idempotent, reconcilable, requires a transaction
 * Output
    - OperationSpec
*/
pub(crate) fn spec(
    adapter: &str,
    operation: &str,
    effect: EffectClass,
    step_kinds: &[StepKind],
    params: &[(&str, ParamKind)],
    flags: (bool, bool, bool),
) -> OperationSpec {
    OperationSpec {
        adapter_id: adapter.into(),
        operation_id: operation.into(),
        description: format!("{adapter} {operation}"),
        effect,
        step_kinds: step_kinds.to_vec(),
        params: params
            .iter()
            .map(|(name, kind)| ParamSpec {
                name: (*name).into(),
                kind: *kind,
                required: true,
                max_len: 200,
            })
            .collect(),
        idempotent: flags.0,
        reconcilable: flags.1,
        requires_transaction: flags.2,
        max_output_bytes: 16 * 1024,
    }
}

impl FixtureAdapter {
    /** Build a fixture system
     * Input
        - id: &str - adapter id
        - specs: Vec<OperationSpec> - operations
        - responses: BTreeMap<String, Value> - read results by operation
     * Output
        - FixtureAdapter
    */
    pub(crate) fn new(
        id: &str,
        specs: Vec<OperationSpec>,
        responses: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            id: id.into(),
            specs,
            responses,
            state: RefCell::new(FixtureState {
                reconcile_available: true,
                healthy: true,
                ..FixtureState::default()
            }),
        }
    }

    /** Queue a fault for the next dispatch of an operation
     * Input
        - operation: &str - operation
        - fault: Fault - fault
     * Output
        - None
    */
    pub(crate) fn inject(&self, operation: &str, fault: Fault) {
        self.state
            .borrow_mut()
            .faults
            .entry(operation.into())
            .or_default()
            .push_back(fault);
    }

    /** Make reconciliation available or not
     * Input
        - available: bool - availability
     * Output
        - None
    */
    #[cfg_attr(not(test), allow(dead_code))] // fault injection and test doubles for the fixture systems; exercised by the tests
    pub(crate) fn set_reconcile(&self, available: bool) {
        self.state.borrow_mut().reconcile_available = available;
    }

    /** Make the system healthy or not
     * Input
        - healthy: bool - health
     * Output
        - None
    */
    #[cfg_attr(not(test), allow(dead_code))] // fault injection and test doubles for the fixture systems; exercised by the tests
    pub(crate) fn set_healthy(&self, healthy: bool) {
        self.state.borrow_mut().healthy = healthy;
    }

    /** Dispatches of an operation so far
     * Input
        - operation: &str - operation
     * Output
        - usize
    */
    pub(crate) fn calls(&self, operation: &str) -> usize {
        self.state
            .borrow()
            .calls
            .get(operation)
            .copied()
            .unwrap_or(0)
    }

    /** Applied effects
     * Input
        - None (uses self)
     * Output
        - usize
    */
    #[cfg_attr(not(test), allow(dead_code))] // fault injection and test doubles for the fixture systems; exercised by the tests
    pub(crate) fn applied(&self) -> usize {
        self.state.borrow().effects.len()
    }
}

impl InvocationAdapter for FixtureAdapter {
    /** The adapter id
     * Input
        - None
     * Output
        - String
    */
    fn adapter_id(&self) -> String {
        self.id.clone()
    }

    /** Its operations
     * Input
        - None
     * Output
        - Vec<OperationSpec>
    */
    fn operations(&self) -> Vec<OperationSpec> {
        self.specs.clone()
    }

    /** Its health
     * Input
        - None
     * Output
        - bool
    */
    fn healthy(&self) -> bool {
        self.state.borrow().healthy
    }

    /** Dispatch deterministically, honoring queued faults; state-changing operations record their
     * effect under the idempotency key (a repeated key returns the recorded result)
     * Input
        - operation: &str - operation
        - params: &BTreeMap<String, String> - parameters
        - key: &str - idempotency key
     * Output
        - DispatchOutcome
    */
    fn dispatch(
        &self,
        operation: &str,
        params: &BTreeMap<String, String>,
        key: &str,
    ) -> DispatchOutcome {
        let mut state = self.state.borrow_mut();
        *state.calls.entry(operation.into()).or_default() += 1;
        let read_only = self
            .specs
            .iter()
            .find(|spec| spec.operation_id == operation)
            .is_none_or(|spec| spec.effect == EffectClass::ReadOnly);
        let fault = state
            .faults
            .get_mut(operation)
            .and_then(VecDeque::pop_front);
        let reference = format!("fx-{key}");
        let effect = json!({"operation": operation, "params": params});
        match fault {
            Some(Fault::TimeoutBeforeDispatch) => {
                return DispatchOutcome::TimedOut {
                    dispatched: false,
                    external_ref: None,
                }
            }
            Some(Fault::TimeoutAfterDispatch) => {
                return DispatchOutcome::TimedOut {
                    dispatched: true,
                    external_ref: Some(reference),
                }
            }
            Some(Fault::TimeoutAfterEffect) => {
                if !read_only {
                    state.effects.insert(key.into(), effect);
                }
                return DispatchOutcome::TimedOut {
                    dispatched: true,
                    external_ref: Some(reference),
                };
            }
            Some(Fault::Fail) => {
                return DispatchOutcome::Failed {
                    error: format!("{operation} was refused by the fixture system"),
                    effect: if read_only {
                        EffectStatus::None
                    } else {
                        EffectStatus::NotApplied
                    },
                }
            }
            Some(Fault::FailUnknown) => {
                return DispatchOutcome::Failed {
                    error: format!("{operation} failed after dispatch"),
                    effect: if read_only {
                        EffectStatus::None
                    } else {
                        EffectStatus::Unknown
                    },
                }
            }
            None => {}
        }
        if read_only {
            let mut result = self
                .responses
                .get(operation)
                .cloned()
                .unwrap_or(Value::Null);
            if let Value::Object(map) = &mut result {
                map.insert("params".into(), json!(params));
            }
            return DispatchOutcome::Completed {
                result,
                external_ref: None,
            };
        }
        let recorded = state.effects.entry(key.into()).or_insert(effect).clone();
        DispatchOutcome::Completed {
            result: json!({"applied": true, "effect": recorded}),
            external_ref: Some(reference),
        }
    }

    /** Reconcile against the fixture's authoritative state
     * Input
        - _operation: &str - operation
        - key: &str - idempotency key
        - external_ref: Option<&str> - external reference
     * Output
        - Reconciliation
    */
    fn reconcile(&self, _operation: &str, key: &str, external_ref: Option<&str>) -> Reconciliation {
        let state = self.state.borrow();
        if !state.reconcile_available {
            return Reconciliation::Unavailable {
                reason: "the fixture system cannot be queried for this operation".into(),
            };
        }
        match state.effects.get(key) {
            Some(effect) => Reconciliation::Applied {
                result: json!({"applied": true, "effect": effect}),
                external_ref: external_ref
                    .map(str::to_string)
                    .or_else(|| Some(format!("fx-{key}"))),
            },
            None => Reconciliation::NotApplied,
        }
    }
}

/** One read-only operation over the existing Git connector: git.commit reads a commit's metadata
 * Fields
    - connector: GitConnector - existing local Git connector
*/
pub(crate) struct GitReadAdapter {
    pub(crate) connector: GitConnector,
}

impl InvocationAdapter for GitReadAdapter {
    /** The adapter id
     * Input
        - None
     * Output
        - String
    */
    fn adapter_id(&self) -> String {
        "git".into()
    }

    /** It reads the repository
     * Input
        - None
     * Output
        - Option<&'static str>
    */
    fn resource_namespace(&self) -> Option<&'static str> {
        Some("repo")
    }

    /** Its single read-only operation
     * Input
        - None
     * Output
        - Vec<OperationSpec>
    */
    fn operations(&self) -> Vec<OperationSpec> {
        vec![spec(
            "git",
            "git.commit",
            EffectClass::ReadOnly,
            &[StepKind::Query, StepKind::Diagnostic, StepKind::Verify],
            &[("commit", ParamKind::Identifier)],
            (true, false, false),
        )]
    }

    /** Health of the repository
     * Input
        - None
     * Output
        - bool
    */
    fn healthy(&self) -> bool {
        self.connector.health_check().available
    }

    /** Read a commit through the connector (its revision rules reject unsafe arguments)
     * Input
        - operation: &str - operation
        - params: &BTreeMap<String, String> - parameters
        - _key: &str - idempotency key
     * Output
        - DispatchOutcome
    */
    fn dispatch(
        &self,
        operation: &str,
        params: &BTreeMap<String, String>,
        _key: &str,
    ) -> DispatchOutcome {
        if operation != "git.commit" {
            return DispatchOutcome::Failed {
                error: format!("unknown operation {operation}"),
                effect: EffectStatus::None,
            };
        }
        let outcome = self
            .connector
            .fetch(params.get("commit").map_or("", String::as_str));
        match outcome.records.into_iter().next() {
            Some(record) if outcome.status.usable() => DispatchOutcome::Completed {
                result: json!({"commit": record.record_id, "files": record.content.get("files"), "subject": record.content.get("subject")}),
                external_ref: None,
            },
            _ => DispatchOutcome::Failed {
                error: outcome
                    .error
                    .unwrap_or_else(|| "the commit does not exist".into()),
                effect: EffectStatus::None,
            },
        }
    }

    /** Read-only operations have nothing to reconcile
     * Input
        - _operation: &str - operation
        - _key: &str - key
        - _external_ref: Option<&str> - reference
     * Output
        - Reconciliation
    */
    fn reconcile(
        &self,
        _operation: &str,
        _key: &str,
        _external_ref: Option<&str>,
    ) -> Reconciliation {
        Reconciliation::Unavailable {
            reason: "read-only operations have no effect to reconcile".into(),
        }
    }
}

/** A compensation attempt, recorded separately from the original effect
 * Fields
    - attempted: bool - whether compensation ran
    - outcome: String - what the runtime reported
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Compensation {
    pub(crate) attempted: bool,
    pub(crate) outcome: String,
}

/** The narrow interface to an external durable-transaction runtime (transaction lifecycle,
 * guarantees, compensation, recovery). The engine only asks it to run one operation and records
 * what it reports */
pub(crate) trait TransactionRuntime {
    /** Report whether a runtime is available
     * Input
        - None (uses self)
     * Output
        - bool
    */
    fn available(&self) -> bool;

    /** Run one operation inside a transaction
     * Input
        - adapter: &dyn InvocationAdapter - adapter
        - operation: &str - operation
        - params: &BTreeMap<String, String> - parameters
        - key: &str - idempotency key
     * Output
        - (DispatchOutcome, Option<Compensation>) - outcome and any compensation attempt
    */
    fn execute(
        &self,
        adapter: &dyn InvocationAdapter,
        operation: &str,
        params: &BTreeMap<String, String>,
        key: &str,
    ) -> (DispatchOutcome, Option<Compensation>);
}

/** No transaction runtime is integrated in this repository: operations that require one are
 * blocked */
pub(crate) struct UnavailableRuntime;

impl TransactionRuntime for UnavailableRuntime {
    /** Never available
     * Input
        - None
     * Output
        - bool
    */
    fn available(&self) -> bool {
        false
    }

    /** Refuse to run anything
     * Input
        - _adapter, _operation, _params, _key - unused
     * Output
        - (DispatchOutcome, Option<Compensation>)
    */
    fn execute(
        &self,
        _adapter: &dyn InvocationAdapter,
        _operation: &str,
        _params: &BTreeMap<String, String>,
        _key: &str,
    ) -> (DispatchOutcome, Option<Compensation>) {
        (
            DispatchOutcome::Failed {
                error: "no transaction runtime is available".into(),
                effect: EffectStatus::NotApplied,
            },
            None,
        )
    }
}

/** A deterministic transaction-runtime stand-in for tests: it runs the operation and, when told to,
 * reports a failure followed by a compensation attempt (the original effect then stays unknown)
 * Fields
    - fail_and_compensate: bool - whether to fail and compensate
*/
#[cfg_attr(not(test), allow(dead_code))] // fault injection and test doubles for the fixture systems; exercised by the tests
pub(crate) struct FakeRuntime {
    pub(crate) fail_and_compensate: bool,
}

impl TransactionRuntime for FakeRuntime {
    /** Always available
     * Input
        - None
     * Output
        - bool
    */
    fn available(&self) -> bool {
        true
    }

    /** Run the operation, optionally failing and compensating
     * Input
        - adapter: &dyn InvocationAdapter - adapter
        - operation: &str - operation
        - params: &BTreeMap<String, String> - parameters
        - key: &str - idempotency key
     * Output
        - (DispatchOutcome, Option<Compensation>)
    */
    fn execute(
        &self,
        adapter: &dyn InvocationAdapter,
        operation: &str,
        params: &BTreeMap<String, String>,
        key: &str,
    ) -> (DispatchOutcome, Option<Compensation>) {
        let outcome = adapter.dispatch(operation, params, key);
        if self.fail_and_compensate {
            return (
                DispatchOutcome::Failed {
                    error: "the transaction failed after the operation ran".into(),
                    effect: EffectStatus::Unknown,
                },
                Some(Compensation {
                    attempted: true,
                    outcome: "compensation reported complete".into(),
                }),
            );
        }
        (outcome, None)
    }
}
