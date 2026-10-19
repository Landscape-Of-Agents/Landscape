// Capability Model & Registry: the second component of the Capability Compiler. It defines what a
// reusable capability is (the canonical, versioned Capability schema), accepts a traceable
// handoff from Work Capture (the CapabilityIntake contract), validates definitions through six
// separate gates, stores immutable versions, and governs their lifecycle. The stages are kept
// apart on purpose:
//
//   intake      - what work occurred, what evidence exists, why it mattered, what is uncertain
//   model       - a proposed reusable procedure and where it applies
//   validate    - which requirements explicit checks and evidence satisfy (gates 1-6)
//   lifecycle   - whether a version may stay a draft, be validated, published, or withdrawn
//   registry    - the authoritative, versioned store with append-only history
//
// It references Work Capture, Landscape, DataSea, and Crane records by identifier and version and
// never copies or rewrites them. It executes nothing: a stored procedure is a description. It
// does not compile capabilities from evidence; drafts are created explicitly (manual or fixture),
// and the "compiler" creation method is reserved for that future component.

pub(crate) mod demo;
pub(crate) mod intake;
pub(crate) mod lifecycle;
pub(crate) mod model;
pub(crate) mod query;
pub(crate) mod refs;
pub(crate) mod registry;
pub(crate) mod validate;

#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::capture::{Actor, ActorKind};

/** Machine-readable category of a capability error (the API's explicit failure conditions)
 * Variants
    - InvalidSchema - the record does not match its schema (fields, types, enumerations, structure)
    - UnsupportedSchemaVersion - the record's schema version is not supported
    - MissingSourceReference - a referenced record does not exist
    - IntegrityMismatch - a digest does not match the referenced content
    - Unauthorized - the actor may not perform the operation or read the reference
    - IncompleteEvidence - required evidence is missing or unresolved
    - UnresolvedBusinessMapping - a required business mapping is not authoritative
    - MissingDependency - a required dependency cannot be resolved
    - StaleValidation - the validation no longer matches the current references
    - InvalidTransition - the lifecycle transition is not allowed
    - PublicationDenied - publication requirements are not met
    - IdempotencyConflict - an idempotency key was reused with different content
    - Persistence - the registry could not be read or written durably
    - NotFound - the capability, version, or intake does not exist
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ErrorCode {
    InvalidSchema,
    UnsupportedSchemaVersion,
    MissingSourceReference,
    IntegrityMismatch,
    Unauthorized,
    IncompleteEvidence,
    UnresolvedBusinessMapping,
    MissingDependency,
    StaleValidation,
    InvalidTransition,
    PublicationDenied,
    IdempotencyConflict,
    Persistence,
    NotFound,
}

/** A capability error: a category and an explanation
 * Fields
    - code: ErrorCode - category
    - message: String - explanation
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CapabilityError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
}

impl CapabilityError {
    /** Build an error
     * Input
        - code: ErrorCode - category
        - message: impl Into<String> - explanation
     * Output
        - CapabilityError
    */
    pub(crate) fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for CapabilityError {
    /** Render the error as "code: message"
     * Input
        - formatter: &mut fmt::Formatter - output
     * Output
        - fmt::Result
    */
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = serde_json::to_value(self.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_string))
            .unwrap_or_default();
        write!(formatter, "{code}: {}", self.message)
    }
}

impl From<CapabilityError> for String {
    /** Convert to Crane's String error type
     * Input
        - error: CapabilityError - error
     * Output
        - String
    */
    fn from(error: CapabilityError) -> Self {
        error.to_string()
    }
}

/** Result of a capability operation */
pub(crate) type CapabilityResult<T> = Result<T, CapabilityError>;

/** What an actor may do in the registry
 * Variants
    - Contributor - submit intakes, create candidates, propose versions
    - Validator - run validation that changes lifecycle state
    - Publisher - publish, reject, deprecate, and revoke
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Role {
    Contributor,
    Validator,
    Publisher,
}

/** The roles the registry grants, held by the application (never taken from caller metadata).
 * Agents can only ever contribute: a Validator or Publisher grant to an agent has no effect
 * Fields
    - members: BTreeMap<String, BTreeSet<Role>> - actor id to roles
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Grants {
    pub(crate) members: BTreeMap<String, BTreeSet<Role>>,
}

impl Grants {
    /** Grant roles to an actor
     * Input
        - actor_id: &str - actor
        - roles: &[Role] - roles
     * Output
        - Grants
    */
    pub(crate) fn with(mut self, actor_id: &str, roles: &[Role]) -> Self {
        self.members
            .entry(actor_id.into())
            .or_default()
            .extend(roles.iter().copied());
        self
    }

    /** Check that an actor holds a role
     * Input
        - actor: &Actor - actor
        - role: Role - role
     * Output
        - CapabilityResult<()>
        - Error (Unauthorized) when the role is not granted, or an agent asks for more than
          contributing
    */
    pub(crate) fn require(&self, actor: &Actor, role: Role) -> CapabilityResult<()> {
        if actor.kind == ActorKind::Agent && role != Role::Contributor {
            return Err(CapabilityError::new(ErrorCode::Unauthorized, format!("an agent cannot act as {role:?}; only people validate, publish, and withdraw capabilities")));
        }
        if self
            .members
            .get(&actor.id)
            .is_some_and(|roles| roles.contains(&role))
        {
            Ok(())
        } else {
            Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                format!("{} does not hold the {role:?} role", actor.id),
            ))
        }
    }
}
