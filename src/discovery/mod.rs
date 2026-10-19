// Capability Discovery & Applicability Engine (component 5): finds published capabilities for a
// task and explains which are recommended and why the others are not. It is read-only over the
// earlier components and owns only its reports:
//
//   resolve   - the task intent plus authorized Work Capture context (an Evidence Pack read through
//               the requester's access policy, integrity-checked) -> repository, environment, zone
//               and flow versions; anything not established stays unknown
//   evaluate  - candidates come from Component 2's registry queries (no other registry); each
//               passes mandatory gates in a fixed order: published and currently eligible (not
//               revoked, deprecated, or stale, including staleness not yet marked), exact
//               applicability (Component 2's matcher: unknown is never a match, an empty
//               environment list means none), scope (zone and flow versions), dependencies, and
//               Component 4 lab reports for the exact digest (a failed report blocks automatic
//               recommendation; inconclusive is never counted as success)
//   rank      - deterministic, versioned ordering, applied only to candidates that passed every gate
//   report    - the integrity-protected discovery report and its append-only storage
//
// It never changes a definition, lifecycle state, validation, or eligibility, executes nothing,
// composes nothing, and does no semantic or fuzzy matching.

pub(crate) mod demo;
pub(crate) mod evaluate;
pub(crate) mod report;
pub(crate) mod resolve;

#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};

/** Version of the discovery rules */
pub(crate) const ENGINE_VERSION: &str = "1";

/** Version of the ranking rules */
pub(crate) const RANKING_VERSION: &str = "1";

/** Version of the discovery report layout */
pub(crate) const REPORT_SCHEMA_VERSION: u32 = 1;

/** The outcome of a discovery
 * Variants
    - Recommended - at least one candidate passed every gate
    - NoMatch - no published capability applies
    - InsufficientContext - some candidates could apply, but unknown context keeps them unresolved
    - Blocked - the request was refused, or every applicable candidate is blocked (a failed lab
      report, unresolvable required dependencies)
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DiscoveryOutcome {
    Recommended,
    NoMatch,
    InsufficientContext,
    Blocked,
}
