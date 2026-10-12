// crane governance sign: an admin approves the current .crane/governance.yaml (zones, flows, and
// context packs) by covering it with a new signed manifest. Until then an edited file fails
// verification, and agents cannot change code, so nobody can loosen a zone or flow unnoticed.
use crate::governance::policy::PolicySet;
use crate::governance::state::{commit, State, GOVERNANCE_SIGNED};
use crate::governance::workspace::Workspace;
use crate::governance::zones::{self, Lifecycle};
use crate::trust::require_human;
use serde_json::json;

/** Run a governance command: sign
 * Input
    - args: &[String] - "sign"
 * Output
    - Result<(), String>
    - Error if run by an agent, by a machine that is not an admin, or the file is invalid or has
      blocking findings (unknown references, duplicate identifiers, unresolved flows)
*/
pub(crate) fn governance(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("sign") || args.len() != 1 {
        return Err("usage: crane governance sign".into());
    }
    require_human("crane governance sign")?;
    let workspace = Workspace::locate()?;
    // Refuse a file the hooks could not enforce, so broken rules are never approved
    let governance = zones::load(&workspace)?;
    let blocking = zones::resolve(
        &workspace,
        &governance,
        &State::load(&workspace)?,
        &PolicySet::load(&workspace)?,
    )?
    .findings
    .into_iter()
    .filter(crate::governance::Finding::blocking)
    .map(|finding| finding.message)
    .collect::<Vec<_>>();
    if !blocking.is_empty() {
        return Err(format!(
            "not signing .crane/governance.yaml: {}",
            blocking.join("; ")
        ));
    }
    let active = |status: Lifecycle| status == Lifecycle::Active;
    let zones_active = governance
        .zones
        .iter()
        .filter(|zone| active(zone.status))
        .count();
    let flows_active = governance
        .flows
        .iter()
        .filter(|flow| active(flow.status))
        .count();
    let state = commit(
        &workspace,
        "crane governance sign",
        GOVERNANCE_SIGNED,
        json!({
            "zones": governance.zones.len(),
            "flows": governance.flows.len(),
            "context_packs": governance.context_packs.len(),
        }),
        |_, _| Ok(()),
    )?;
    println!(
        "Signed .crane/governance.yaml at generation {}: {} zones ({zones_active} active), {} flows ({flows_active} active), {} context packs",
        state.generation(),
        governance.zones.len(),
        governance.flows.len(),
        governance.context_packs.len()
    );
    Ok(())
}
