// The resource registry: which evidence sources exist, which registered connector implements
// each one, what they may be asked, and where they may look. Registration is declarative and
// validated: only connector types compiled into Crane are accepted, configuration keys are those
// the connector declares (no command, script, or URL template is ever executed), credentials are
// references to a provider ("env:NAME"), never values, and every scope is explicit (no wildcards).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::capture::format::record::EvidenceType;
use crate::capture::{CaptureError, CaptureResult, ErrorCode};
use crate::telemetry::redact::redact;

/** The kind of system a resource is
 * Variants
    - Tracker - tasks, tickets, acceptance criteria, approvals
    - Github - repositories, pull requests, reviews, diffs
    - Git - a local Git repository
    - Ci - CI/CD jobs and test reports
    - Deployment - deployments and post-change verification
    - AgentExecution - DataSea agent sessions, tool calls, decisions
    - Logs - logs and stack traces
    - Metrics - metrics and time series
    - InternalApi - internal operational tools and APIs
    - Landscape - Landscape zones, flows, and system context
    - Crane - Crane/AgentScript policies, contracts, and validation
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceType {
    Tracker,
    Github,
    Git,
    Ci,
    Deployment,
    AgentExecution,
    Logs,
    Metrics,
    InternalApi,
    Landscape,
    Crane,
}

/** Health and administrative status of a resource
 * Variants
    - Active - may be queried
    - Disabled - switched off by an administrator
    - Unhealthy - its last health check failed
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ResourceStatus {
    Active,
    Disabled,
    Unhealthy,
}

/** A retrieval operation of the connector contract
 * Variants
    - Discover - list available objects and operations
    - Fetch - retrieve one known record
    - Query - retrieve records by validated filters and limits
    - Metadata - retrieve identifiers, timestamps, and revisions only
    - Health - report availability
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Operation {
    Discover,
    Fetch,
    Query,
    Metadata,
    Health,
}

/** What a resource can be asked
 * Fields
    - operations: Vec<Operation> - supported operations
    - evidence_types: Vec<EvidenceType> - evidence it provides
    - filters: Vec<String> - filter keys its queries accept
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Capabilities {
    pub(crate) operations: Vec<Operation>,
    pub(crate) evidence_types: Vec<EvidenceType>,
    pub(crate) filters: Vec<String>,
}

/** Where a resource may look; every list is explicit and none may hold a wildcard
 * Fields
    - repositories: Vec<String> - repositories
    - projects: Vec<String> - projects
    - environments: Vec<String> - environments
    - resources: Vec<String> - other named resources
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResourceScope {
    pub(crate) repositories: Vec<String>,
    pub(crate) projects: Vec<String>,
    pub(crate) environments: Vec<String>,
    pub(crate) resources: Vec<String>,
}

/** A registered evidence source
 * Fields
    - resource_id: String - stable identifier
    - resource_type: ResourceType - kind of system
    - connector_type: String - registered connector implementation
    - capabilities: Capabilities - supported operations, evidence, and filters
    - scope: ResourceScope - where it may look
    - auth_reference: Option<String> - credential provider reference ("env:NAME"), never a secret
    - configuration: BTreeMap<String, String> - connector settings the connector declares
    - configuration_version: u32 - configuration version
    - status: ResourceStatus - active, disabled, or unhealthy
    - created_at: u64 - Unix milliseconds
    - updated_at: u64 - Unix milliseconds
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResourceDefinition {
    pub(crate) resource_id: String,
    pub(crate) resource_type: ResourceType,
    pub(crate) connector_type: String,
    pub(crate) capabilities: Capabilities,
    pub(crate) scope: ResourceScope,
    pub(crate) auth_reference: Option<String>,
    pub(crate) configuration: BTreeMap<String, String>,
    pub(crate) configuration_version: u32,
    pub(crate) status: ResourceStatus,
    pub(crate) created_at: u64,
    pub(crate) updated_at: u64,
}

/** A connector implementation compiled into Crane
 * Fields
    - name: &'static str - connector type
    - resource_types: &'static [ResourceType] - systems it can serve
    - configuration_keys: &'static [&'static str] - settings it accepts
    - filters: &'static [&'static str] - query filters it understands
*/
#[derive(Debug, Clone, Copy)]
pub(crate) struct ConnectorSpec {
    pub(crate) name: &'static str,
    pub(crate) resource_types: &'static [ResourceType],
    pub(crate) configuration_keys: &'static [&'static str],
    pub(crate) filters: &'static [&'static str],
}

/** Every connector type that can be registered */
pub(crate) const CONNECTORS: &[ConnectorSpec] = &[
    ConnectorSpec {
        name: "fixture",
        resource_types: &[
            ResourceType::Tracker,
            ResourceType::Github,
            ResourceType::Ci,
            ResourceType::Deployment,
            ResourceType::AgentExecution,
            ResourceType::Logs,
            ResourceType::Metrics,
            ResourceType::InternalApi,
            ResourceType::Landscape,
            ResourceType::Crane,
        ],
        configuration_keys: &["fixture"],
        filters: &[
            "task",
            "commit",
            "pull_request",
            "deployment",
            "environment",
            "session",
            "status",
            "kind",
        ],
    },
    ConnectorSpec {
        name: "git",
        resource_types: &[ResourceType::Git],
        configuration_keys: &[],
        filters: &["commit", "path"],
    },
    ConnectorSpec {
        name: "datasea",
        resource_types: &[ResourceType::AgentExecution],
        configuration_keys: &[],
        filters: &["session", "task", "event_type"],
    },
    ConnectorSpec {
        name: "landscape",
        resource_types: &[ResourceType::Landscape],
        configuration_keys: &[],
        filters: &["kind", "id"],
    },
    ConnectorSpec {
        name: "crane",
        resource_types: &[ResourceType::Crane],
        configuration_keys: &[],
        filters: &["kind", "name"],
    },
];

/** Find a registered connector type
 * Input
    - name: &str - connector type
 * Output
    - Option<&'static ConnectorSpec>
*/
pub(crate) fn connector_spec(name: &str) -> Option<&'static ConnectorSpec> {
    CONNECTORS.iter().find(|spec| spec.name == name)
}

/** Check an identifier: 1-64 lowercase letters, digits, '.', '_' or '-', starting with a letter
 * Input
    - value: &str - identifier
 * Output
    - bool
*/
pub(crate) fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.starts_with(|character: char| character.is_ascii_lowercase())
        && value.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '.' | '_' | '-')
        })
}

/** Build a registration error
 * Input
    - message: String - explanation
 * Output
    - CaptureError
*/
fn invalid(message: String) -> CaptureError {
    CaptureError::new(ErrorCode::InvalidRegistration, message)
}

/** Validate a definition against its connector type
 * Input
    - definition: &ResourceDefinition - candidate
 * Output
    - CaptureResult<()>
    - Error if the connector is unknown or any field is invalid
*/
pub(crate) fn validate(definition: &ResourceDefinition) -> CaptureResult<()> {
    if !valid_identifier(&definition.resource_id) {
        return Err(invalid(format!(
            "invalid resource id '{}'",
            definition.resource_id
        )));
    }
    let spec = connector_spec(&definition.connector_type).ok_or_else(|| {
        CaptureError::new(
            ErrorCode::UnknownConnector,
            format!(
                "connector type '{}' is not registered; known: {}",
                definition.connector_type,
                CONNECTORS
                    .iter()
                    .map(|spec| spec.name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )
    })?;
    if !spec.resource_types.contains(&definition.resource_type) {
        return Err(invalid(format!(
            "connector '{}' cannot serve resource type {:?}",
            spec.name, definition.resource_type
        )));
    }
    if definition.capabilities.operations.is_empty()
        || definition.capabilities.evidence_types.is_empty()
    {
        return Err(invalid(format!(
            "resource '{}' must declare its operations and evidence types",
            definition.resource_id
        )));
    }
    if let Some(filter) = definition
        .capabilities
        .filters
        .iter()
        .find(|filter| !spec.filters.contains(&filter.as_str()))
    {
        return Err(invalid(format!(
            "connector '{}' does not support filter '{filter}'",
            spec.name
        )));
    }
    for (key, value) in &definition.configuration {
        if !spec.configuration_keys.contains(&key.as_str()) {
            return Err(invalid(format!(
                "connector '{}' does not accept configuration '{key}'",
                spec.name
            )));
        }
        if value.len() > 512 || redact(value).1 > 0 {
            return Err(invalid(format!(
                "configuration '{key}' is too long or contains a secret; use auth_reference"
            )));
        }
    }
    if let Some(reference) = &definition.auth_reference {
        let well_formed = reference.split_once(':').is_some_and(|(provider, name)| {
            matches!(provider, "env" | "keychain" | "integration")
                && !name.is_empty()
                && name.len() <= 128
                && name
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "_-.".contains(character))
        });
        if !well_formed || redact(reference).1 > 0 {
            return Err(invalid(
                "auth_reference must name a credential provider (env:NAME, keychain:NAME, integration:NAME), never a secret".into(),
            ));
        }
    }
    let scope = &definition.scope;
    let lists = [
        &scope.repositories,
        &scope.projects,
        &scope.environments,
        &scope.resources,
    ];
    if lists.iter().all(|list| list.is_empty()) {
        return Err(invalid(format!(
            "resource '{}' needs an explicit scope",
            definition.resource_id
        )));
    }
    if lists
        .iter()
        .flat_map(|list| list.iter())
        .any(|item| item.is_empty() || item.contains('*') || item.contains('?'))
    {
        return Err(invalid(
            "scopes cannot be empty or contain wildcards".into(),
        ));
    }
    Ok(())
}

/** The registered resources
 * Fields
    - definitions: BTreeMap<String, ResourceDefinition> - by resource id
*/
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ResourceRegistry {
    pub(crate) definitions: BTreeMap<String, ResourceDefinition>,
}

impl ResourceRegistry {
    /** Register or update a resource; an update keeps the creation time and must raise the
     * configuration version when the configuration changes
     * Input
        - definition: ResourceDefinition - resource
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error if validation fails
    */
    pub(crate) fn register(
        &mut self,
        mut definition: ResourceDefinition,
        now: u64,
    ) -> CaptureResult<()> {
        validate(&definition)?;
        if let Some(existing) = self.definitions.get(&definition.resource_id) {
            if existing.configuration != definition.configuration
                && definition.configuration_version <= existing.configuration_version
            {
                return Err(invalid(format!(
                    "resource '{}' changed its configuration without raising configuration_version",
                    definition.resource_id
                )));
            }
            definition.created_at = existing.created_at;
        } else {
            definition.created_at = now;
        }
        definition.updated_at = now;
        self.definitions
            .insert(definition.resource_id.clone(), definition);
        Ok(())
    }

    /** Find a resource
     * Input
        - resource_id: &str - identifier
     * Output
        - Option<&ResourceDefinition>
    */
    pub(crate) fn get(&self, resource_id: &str) -> Option<&ResourceDefinition> {
        self.definitions.get(resource_id)
    }

    /** Change a resource's status (disable it, or mark it unhealthy after a failed check)
     * Input
        - resource_id: &str - identifier
        - status: ResourceStatus - new status
        - now: u64 - Unix milliseconds
     * Output
        - CaptureResult<()>
        - Error if the resource is not registered
    */
    #[cfg_attr(not(test), allow(dead_code))] // extension point for Capability Compiler components; exercised by the acceptance tests
    pub(crate) fn set_status(
        &mut self,
        resource_id: &str,
        status: ResourceStatus,
        now: u64,
    ) -> CaptureResult<()> {
        let definition = self.definitions.get_mut(resource_id).ok_or_else(|| {
            CaptureError::new(ErrorCode::NotFound, format!("no resource '{resource_id}'"))
        })?;
        definition.status = status;
        definition.updated_at = now;
        Ok(())
    }

    /** List the resources of a type, in identifier order
     * Input
        - resource_type: ResourceType - kind of system
     * Output
        - Vec<&ResourceDefinition>
    */
    pub(crate) fn of_type(&self, resource_type: ResourceType) -> Vec<&ResourceDefinition> {
        self.definitions
            .values()
            .filter(|definition| definition.resource_type == resource_type)
            .collect()
    }
}
