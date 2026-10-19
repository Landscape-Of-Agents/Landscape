// Request resolution. Authorization comes first: the requester must hold a registry role, and the
// Work Capture access policy must be the requester's own. Work Capture context is read only
// through that policy: an Evidence Pack named by the request must exist, recompute to its
// manifest digest (and the digest the requester expects), and belong to a repository the
// requester may read; its environment, repository, and zone and flow versions then become
// observed context. Declared values that contradict observed ones are not resolved in either
// direction: the field stays unknown and the conflict is reported. Nothing is inferred.

use serde::{Deserialize, Serialize};

use crate::capability::refs::Sources;
use crate::capability::{CapabilityError, CapabilityResult, ErrorCode, Grants, Role};
use crate::capture::resources::scope::VersionedRef;
use crate::capture::Actor;
use crate::compiler::Basis;

/** A Work Capture context reference
 * Fields
    - pack_id: String - Evidence Pack
    - pack_digest: String - its manifest digest, as the requester expects it
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ContextRef {
    pub(crate) pack_id: String,
    pub(crate) pack_digest: String,
}

/** A discovery request
 * Fields
    - task_class: String - task class (required, exact)
    - environment: Option<String> - target environment, None when unknown
    - repository: Option<String> - repository, None when unknown
    - confirmed_preconditions: Vec<String> - precondition ids the requester confirms
    - entities: Vec<String> - entities present (matched against exclusions)
    - context: Option<ContextRef> - Work Capture context
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DiscoveryRequest {
    pub(crate) task_class: String,
    pub(crate) environment: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) confirmed_preconditions: Vec<String>,
    pub(crate) entities: Vec<String>,
    pub(crate) context: Option<ContextRef>,
}

/** One resolved context value and how it is known
 * Fields
    - field: String - field name
    - value: Option<String> - value, None when unknown
    - basis: Basis - declared (by the requester), observed (from Work Capture), or unknown
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedValue {
    pub(crate) field: String,
    pub(crate) value: Option<String>,
    pub(crate) basis: Basis,
}

/** The resolved context of a discovery
 * Fields
    - task_class: String - task class
    - environment: Option<String> - environment
    - repository: Option<String> - repository
    - zones: Vec<VersionedRef> - zone versions observed in the context
    - flows: Vec<VersionedRef> - flow versions observed in the context
    - confirmed_preconditions: Vec<String> - confirmed preconditions (declared)
    - entities: Vec<String> - entities (declared)
    - values: Vec<ResolvedValue> - every value with its basis
    - unresolved: Vec<String> - what is unknown or conflicting
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResolvedContext {
    pub(crate) task_class: String,
    pub(crate) environment: Option<String>,
    pub(crate) repository: Option<String>,
    pub(crate) zones: Vec<VersionedRef>,
    pub(crate) flows: Vec<VersionedRef>,
    pub(crate) confirmed_preconditions: Vec<String>,
    pub(crate) entities: Vec<String>,
    pub(crate) values: Vec<ResolvedValue>,
    pub(crate) unresolved: Vec<String>,
}

/** Combine a declared and an observed value: equal or one-sided values resolve; conflicting
 * values stay unknown
 * Input
    - field: &str - field name
    - declared: Option<String> - requester's value
    - observed: Option<String> - Work Capture's value
    - context: &mut ResolvedContext - context to record into
 * Output
    - Option<String>
*/
fn combine(
    field: &str,
    declared: Option<String>,
    observed: Option<String>,
    context: &mut ResolvedContext,
) -> Option<String> {
    let (value, basis) = match (declared, observed) {
        (Some(declared), Some(observed)) if declared != observed => {
            context.unresolved.push(format!(
                "{field}: the request says {declared} but the Work Capture context says {observed}"
            ));
            (None, Basis::Unknown)
        }
        (_, Some(observed)) => (Some(observed), Basis::Observed),
        (Some(declared), None) => (Some(declared), Basis::Declared),
        (None, None) => {
            context.unresolved.push(format!("{field} is unknown"));
            (None, Basis::Unknown)
        }
    };
    context.values.push(ResolvedValue {
        field: field.into(),
        value: value.clone(),
        basis,
    });
    value
}

/** Resolve a request after authorizing the requester
 * Input
    - request: &DiscoveryRequest - request
    - grants: &Grants - registry roles
    - sources: &Sources - Work Capture records and the requester's access policy
    - actor: &Actor - requester
 * Output
    - CapabilityResult<ResolvedContext>
    - Error (Unauthorized, MissingSourceReference, IntegrityMismatch, InvalidSchema) when refused
*/
pub(crate) fn resolve(
    request: &DiscoveryRequest,
    grants: &Grants,
    sources: &Sources,
    actor: &Actor,
) -> CapabilityResult<ResolvedContext> {
    grants.require(actor, Role::Contributor)?;
    if sources.reader.actor != *actor {
        return Err(CapabilityError::new(
            ErrorCode::Unauthorized,
            "the access policy belongs to another actor",
        ));
    }
    let task_class = request.task_class.trim();
    if task_class.is_empty()
        || task_class.contains('*')
        || matches!(task_class.to_ascii_lowercase().as_str(), "any" | "all")
    {
        return Err(CapabilityError::new(
            ErrorCode::InvalidSchema,
            "the task class must be explicit",
        ));
    }
    let mut context = ResolvedContext {
        task_class: task_class.into(),
        confirmed_preconditions: request.confirmed_preconditions.clone(),
        entities: request.entities.clone(),
        ..ResolvedContext::default()
    };
    let mut observed_environment = None;
    let mut observed_repository = None;
    if let Some(reference) = &request.context {
        let pack = sources
            .capture
            .packs
            .get(&reference.pack_id)
            .ok_or_else(|| {
                CapabilityError::new(
                    ErrorCode::MissingSourceReference,
                    format!("Evidence Pack {} does not exist", reference.pack_id),
                )
            })?;
        if pack.manifest.digest != reference.pack_digest {
            return Err(CapabilityError::new(
                ErrorCode::IntegrityMismatch,
                format!(
                    "expected pack digest {} but the pack's is {}",
                    reference.pack_digest, pack.manifest.digest
                ),
            ));
        }
        pack.verify_manifest()
            .map_err(|error| CapabilityError::new(ErrorCode::IntegrityMismatch, error))?;
        if !sources
            .reader
            .permits_repository(pack.scope.repository.as_deref())
        {
            return Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                "the requester may not read this Work Capture context",
            ));
        }
        observed_environment = pack.scope.environment.clone();
        observed_repository = pack.scope.repository.clone();
        context.zones.clone_from(&pack.scope.zones);
        context.flows.clone_from(&pack.scope.flows);
        for reference in context
            .zones
            .iter()
            .chain(&context.flows)
            .filter(|reference| reference.version.is_none())
        {
            context
                .unresolved
                .push(format!("the version of {} is unknown", reference.id));
        }
    }
    context.environment = combine(
        "environment",
        request.environment.clone(),
        observed_environment,
        &mut context,
    );
    context.repository = combine(
        "repository",
        request.repository.clone(),
        observed_repository,
        &mut context,
    );
    if let Some(repository) = &context.repository {
        if !sources.reader.permits_repository(Some(repository)) {
            return Err(CapabilityError::new(
                ErrorCode::Unauthorized,
                format!("the requester may not see {repository}"),
            ));
        }
    }
    context.values.push(ResolvedValue {
        field: "task_class".into(),
        value: Some(context.task_class.clone()),
        basis: Basis::Declared,
    });
    Ok(context)
}
