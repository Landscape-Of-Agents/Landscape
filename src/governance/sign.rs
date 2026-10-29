// Signing .crane/governance.yaml: the checks 'crane governance sign' makes before an admin covers
// the file with a new signed manifest (no blocking findings, so rules the hooks could not enforce
// are never approved), shared by every command that changes the file (approving a traced flow).

use serde_json::json;

use super::policy::PolicySet;
use super::state::{commit, State, GOVERNANCE_SIGNED};
use super::workspace::Workspace;
use super::zones;

/** Check .crane/governance.yaml and sign it (with any other change of the signed state made in the
 * same generation): the file must have no blocking findings (unknown references, duplicate
 * identifiers, unresolved flows, stage cycles), so rules the hooks could not enforce are never
 * approved
 * Input
    - workspace: &Workspace - repository
    - command: &str - the command signing (audited)
    - detail: serde_json::Value - what else changed, for the audit entry
    - extra: impl FnOnce(&mut State) -> Result<(), String> - another change in the same generation
 * Output
    - Result<(State, zones::Governance, Vec<String>), String> the signed
      state, the governance signed, and its zone and flow source markers
    - Error when it is run by an agent or a machine that is not an admin, or the file does not pass
*/
pub(crate) fn sign_with(
    workspace: &Workspace,
    command: &str,
    detail: serde_json::Value,
    extra: impl FnOnce(&mut State) -> Result<(), String>,
) -> Result<(State, zones::Governance, Vec<String>), String> {
    // Refuse a file the hooks could not enforce, so broken rules are never approved
    let governance = zones::load(workspace)?;
    let resolution = zones::resolve(
        workspace,
        &governance,
        &State::load(workspace)?,
        &PolicySet::load(workspace)?,
    )?;
    // Signing is what approves added, moved, or removed zone and flow markers
    let blocking = resolution
        .findings
        .iter()
        .filter(|finding| finding.blocking())
        .filter(|finding| {
            !matches!(
                finding.code.as_str(),
                "source_marker_unsigned" | "source_marker_removed"
            )
        })
        .map(|finding| finding.message.clone())
        .collect::<Vec<_>>();
    let markers = resolution.source_markers.clone();
    if !blocking.is_empty() {
        return Err(format!(
            "not signing .crane/governance.yaml: {}",
            blocking.join("; ")
        ));
    }
    let state = commit(
        workspace,
        command,
        GOVERNANCE_SIGNED,
        {
            let mut signed = json!({
                "zones": governance.zones.len(),
                "flows": governance.flows.len(),
                "context_packs": governance.context_packs.len(),
                "source_markers": markers,
            });
            if !detail.is_null() {
                signed["change"] = detail;
            }
            signed
        },
        |state, _| {
            extra(state)?;
            // Recorded only once there are markers, so repositories without them keep their digest
            if !markers.is_empty() || state.config.source_markers.is_some() {
                state.config.source_markers = Some(markers.clone());
            }
            Ok(())
        },
    )?;
    Ok((state, governance, markers))
}
