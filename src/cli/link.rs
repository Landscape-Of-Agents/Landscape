// Approvals checked by GitHub: 'crane agent link' links this machine's upload key to the GitHub
// account of the person who approves it on the dashboard (who must have access to the repository),
// and 'crane admin register' asks a GitHub admin of the repository to confirm the owner key, which
// a teammate's 'crane init' then shows next to the fingerprint it asks them to trust. The machine
// only leaves a request with a one-time code at the service; the decision is GitHub's, made when
// the person opens the printed address and signs in.
use crate::governance::admins;
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::platform::render::iso_time;
use crate::telemetry::ingest::{
    approval_code, owner_message, ApprovalRequest, Client, CODE_DOMAIN, OWNER_DOMAIN,
};
use crate::trust::crypto::{digest, SigningIdentity};
use crate::trust::require_human;

/** Open the service client, or explain why approvals are unavailable
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Client, String>
*/
fn client(workspace: &Workspace) -> Result<Client, String> {
    if !workspace
        .remote
        .as_deref()
        .is_some_and(|remote| remote.starts_with("github.com/"))
    {
        return Err(
            "approvals need a GitHub 'origin' remote, so GitHub can confirm who may approve".into(),
        );
    }
    Client::open(workspace)?.ok_or_else(|| {
        "uploads are off (CRANE_INGEST_URL=off), so there is no service to approve with".into()
    })
}

/** Leave an approval request at the service and return the address that approves it
 * Input
    - workspace: &Workspace - repository
    - client: &Client - service client
    - operation: &str - link or owner
    - owner: Option<&SigningIdentity> - the owner's signing key (owner registration)
 * Output
    - Result<String, String> the approval address
*/
fn request(
    workspace: &Workspace,
    client: &Client,
    operation: &str,
    owner: Option<&SigningIdentity>,
) -> Result<String, String> {
    let code = approval_code()?;
    let mut approval = ApprovalRequest {
        code: code.clone(),
        remote: workspace.remote.clone(),
        root_commits: workspace.root_commits.clone(),
        ..ApprovalRequest::default()
    };
    if let Some(owner) = owner {
        let code_digest = digest(CODE_DOMAIN, &[code.as_bytes()]);
        let message = owner_message(
            &workspace.repository_id,
            &owner.public_hex(),
            &client.installation(),
            &code_digest,
        );
        approval.owner_key = Some(owner.public_hex());
        approval.signature = Some(owner.sign(OWNER_DOMAIN, &message));
    }
    client.request_approval(operation, &approval)?;
    Ok(format!(
        "{}/auth/approve?action={operation}&code={code}",
        client.base()
    ))
}

/** crane agent link: link this machine's upload key to the GitHub account that approves it
 * Input
    - args: &[String] - none
 * Output
    - Result<(), String>
    - Error if run by an agent, uploads are off, or the service refuses
*/
pub(crate) fn link(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("usage: crane agent link".into());
    }
    require_human("crane agent link")?;
    let workspace = Workspace::discover()?;
    let client = client(&workspace)?;
    let url = request(&workspace, &client, "link", None)?;
    println!(
        "To show this machine's activity as yours on the dashboard, open this address within 10 minutes and sign in with GitHub (you need access to {}):",
        workspace.remote.as_deref().unwrap_or_default()
    );
    println!("  {url}");
    println!("The code works once. Machines nobody linked are hidden on the dashboard once a teammate is linked.");
    Ok(())
}

/** crane admin register: ask a GitHub admin of the repository to confirm the owner key
 * Input
    - workspace: &Workspace - repository
    - state: &State - governance state
    - owner: &SigningIdentity - this machine's signing key (must be the owner's)
 * Output
    - Result<String, String> the approval address
*/
pub(crate) fn register_owner(
    workspace: &Workspace,
    state: &State,
    owner: &SigningIdentity,
) -> Result<String, String> {
    if admins::owner(state).as_deref() != Some(owner.public_hex().as_str()) {
        return Err("only the repository owner's machine can register the owner key".into());
    }
    let client = client(workspace)?;
    request(workspace, &client, "owner", Some(owner))
}

/** Show what GitHub confirmed about the owner key a teammate is about to trust: a match, a loud
 * warning on a mismatch, or that nothing is confirmed. Best effort: the fingerprint comparison
 * stays the decision, and a missing service never blocks joining
 * Input
    - workspace: &Workspace - repository
    - owner: &str - the owner key in the repository's signed rules
 * Output
    - bool true when GitHub confirmed a different key (the caller should warn and default to no)
*/
pub(crate) fn show_owner_confirmation(workspace: &Workspace, owner: &str) -> bool {
    let answer = Client::open(workspace)
        .ok()
        .flatten()
        .map(|client| client.confirmed_owner());
    let result = match &answer {
        Some(Ok(answer)) => match answer["owner_key"].as_str() {
            Some(confirmed) if confirmed == owner => "MATCH",
            Some(_) => "MISMATCH",
            None => "NONE_CONFIRMED",
        },
        Some(Err(_)) => "UNAVAILABLE",
        None => "UPLOADS_OFF",
    };
    match &answer {
        Some(Ok(answer)) => crate::platform::notes::note(
            "OWNER_KEY_LOOKUP",
            serde_json::json!({"result": result, "confirmed_by": answer["confirmed_by"], "confirmed_key": answer["owner_key"]}),
        ),
        Some(Err(_)) => {
            crate::platform::notes::note("OWNER_KEY_LOOKUP", serde_json::json!({"result": result}))
        }
        None => {}
    }
    match answer {
        Some(Ok(answer)) => match answer["owner_key"].as_str() {
            Some(confirmed) if confirmed == owner => {
                println!(
                    "  GitHub check:  this owner key was confirmed by @{}, an admin of {}, on {}",
                    answer["confirmed_by"].as_str().unwrap_or("?"),
                    answer["remote"].as_str().unwrap_or("the repository"),
                    iso_time(answer["confirmed_at"].as_u64().unwrap_or(0))
                );
                false
            }
            Some(confirmed) => {
                println!("  WARNING: this is NOT the owner key a GitHub admin confirmed for this repository.");
                println!(
                    "           @{} confirmed {}… on {}.",
                    answer["confirmed_by"].as_str().unwrap_or("?"),
                    &confirmed[..confirmed.len().min(16)],
                    iso_time(answer["confirmed_at"].as_u64().unwrap_or(0))
                );
                println!("           The rules may have been replaced. Do not trust this key unless the owner tells you the key changed.");
                true
            }
            None => {
                println!("  GitHub check:  no owner key is confirmed for this repository yet (the owner can run 'crane admin register'); compare the fingerprint with the owner directly");
                false
            }
        },
        Some(Err(_)) => {
            println!("  GitHub check:  the service could not be asked; compare the fingerprint with the owner directly");
            false
        }
        None => false,
    }
}
