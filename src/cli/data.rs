// Destructive hosted data operations, scoped to the repository the human is currently in.

use std::io::Write;
use std::process::Command;
use std::time::Duration;

use crate::governance::workspace::Workspace;
use crate::platform::process;
use crate::telemetry::ingest::Client;
use crate::trust::require_human;

pub(crate) fn data(args: &[String]) -> Result<(), String> {
    match args {
        [command] if command == "clear" => clear(),
        _ => Err("usage: crane data clear".into()),
    }
}

fn clear() -> Result<(), String> {
    require_human("crane data clear")?;
    let workspace = Workspace::discover()?;
    let remote = workspace
        .remote
        .as_deref()
        .ok_or("this repository has no GitHub remote")?;
    let parts = remote.split('/').collect::<Vec<_>>();
    if parts.len() != 3 || parts.iter().any(|part| part.is_empty()) {
        return Err("this command requires a normalized GitHub OWNER/REPOSITORY remote".into());
    }
    let slug = format!("{}/{}", parts[1], parts[2]);
    let token = process::run(
        Command::new("gh").args(["auth", "token", "--hostname", parts[0]]),
        Duration::from_secs(15),
    )?;
    if !token.success {
        return Err(format!(
            "GitHub CLI authentication is required; run 'gh auth login'{}",
            if token.stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", token.stderr.trim())
            }
        ));
    }
    let token = token.stdout.trim();
    if token.is_empty() || token.chars().any(char::is_whitespace) {
        return Err("GitHub CLI returned an invalid token".into());
    }
    println!(
        "This will permanently delete Crane's hosted records for {slug} across all repository members and installations."
    );
    println!(
        "The service will verify that your GitHub account is directly listed in the default-branch CODEOWNERS file."
    );
    print!("Type CLEAR {slug} to continue: ");
    std::io::stdout()
        .flush()
        .map_err(|error| format!("could not display confirmation prompt: {error}"))?;
    let mut confirmation = String::new();
    std::io::stdin()
        .read_line(&mut confirmation)
        .map_err(|error| format!("could not read confirmation: {error}"))?;
    if confirmation.trim() != format!("CLEAR {slug}") {
        return Err("confirmation did not match; no repository data was deleted".into());
    }
    let client = Client::open(&workspace)?
        .ok_or("hosted telemetry is disabled; set CRANE_INGEST_URL to the ingestion service")?;
    let result = client.clear_repository(remote, token)?;
    println!(
        "Cleared {} hosted records for {} (authorized as @{}).",
        result["deleted_total"].as_u64().unwrap_or_default(),
        slug,
        result["authorized_as"].as_str().unwrap_or("unknown")
    );
    Ok(())
}
