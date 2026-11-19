// 'crane operator': the operator's own commands. 'keygen' creates the operator key pair (the seed
// goes to the operator dashboard's CRANE_OPERATOR_SIGNING_KEY, the public key into official builds
// as CRANE_OPERATOR_PUBLIC_KEY); 'status' shows what this machine believes about the agent-command
// switch, and '--refresh' asks the dashboard now; 'serve' runs the operator dashboard (feature
// "server"). Agents may run none of these.

use crate::trust::agent_commands;
use crate::trust::crypto::SigningIdentity;
use crate::trust::require_human;

/** How to use 'crane operator' */
const USAGE: &str = "usage:
  crane operator keygen                 (a new operator key pair; keep the seed secret)
  crane operator status [--refresh]     (this machine's view of the agent-command switch)
  crane operator serve [--listen ADDR]  (the operator dashboard; needs the server feature)";

/** Run 'crane operator'
 * Input
    - args: &[String] - arguments after "operator"
 * Output
    - Result<(), String>
*/
pub(crate) fn operator(args: &[String]) -> Result<(), String> {
    let words = args.iter().map(String::as_str).collect::<Vec<_>>();
    match words.as_slice() {
        ["keygen"] => keygen(),
        ["status"] => status(false),
        ["status", "--refresh"] => status(true),
        #[cfg(feature = "server")]
        ["serve", ..] => {
            require_human("crane operator serve")?;
            crate::server::operator::run(&args[1..])
        }
        _ => Err(USAGE.into()),
    }
}

/** Create and print a new operator key pair
 * Input
    - None
 * Output
    - Result<(), String>
*/
fn keygen() -> Result<(), String> {
    require_human("crane operator keygen")?;
    let key = SigningIdentity::generate()?;
    println!("Operator key pair (keep the signing key secret; it is shown only now):");
    println!("  CRANE_OPERATOR_SIGNING_KEY={}", key.seed_hex());
    println!("  CRANE_OPERATOR_PUBLIC_KEY={}", key.public_hex());
    println!();
    println!("Set CRANE_OPERATOR_SIGNING_KEY as a secret on the operator dashboard service.");
    println!(
        "Build official Crane releases with CRANE_OPERATOR_PUBLIC_KEY set (a repository variable),"
    );
    println!("so machines trust only answers signed by this key.");
    Ok(())
}

/** Show this machine's view of the agent-command switch
 * Input
    - refresh: bool - ask the operator dashboard first
 * Output
    - Result<(), String>
*/
fn status(refresh: bool) -> Result<(), String> {
    require_human("crane operator status")?;
    println!(
        "operator key in this build: {}",
        agent_commands::operator_public_key()
            .unwrap_or_else(|| "none (agents never run Crane commands)".into())
    );
    println!(
        "operator dashboard: {}",
        agent_commands::operator_url().unwrap_or_else(|| "off".into())
    );
    if agent_commands::locally_off() {
        println!("this machine: turned off (CRANE_AGENT_COMMANDS)");
    }
    if refresh {
        match agent_commands::refresh() {
            Ok(_) => println!("refreshed: the dashboard's answer verifies"),
            Err(error) => println!("not refreshed: {error}"),
        }
    }
    match agent_commands::cached() {
        Ok(signed) => {
            let policy = signed.policy;
            println!(
                "switch: {} (set by {} at {}; this answer is valid until {})",
                if policy.enabled { "ON" } else { "OFF" },
                if policy.changed_by.is_empty() {
                    "nobody yet"
                } else {
                    &policy.changed_by
                },
                crate::platform::render::iso_time(policy.changed_at),
                crate::platform::render::iso_time(policy.expires_at)
            );
        }
        Err(reason) => println!("switch: OFF ({reason})"),
    }
    println!(
        "agents may run read-only commands now: {}",
        if agent_commands::enabled_now() {
            "yes"
        } else {
            "no"
        }
    );
    Ok(())
}
