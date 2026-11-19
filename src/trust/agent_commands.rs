// The operator switch that lets AI agents run read-only Crane commands inside their own sessions.
//
// Off is the default and the answer to every doubt. The switch is set on the operator dashboard
// ('crane operator serve', server/operator/), which only the operator can sign in to. The dashboard
// answers machines with a policy signed by the operator key (Ed25519); a machine trusts it only
// when the signature verifies against the operator public key compiled into this build
// (CRANE_OPERATOR_PUBLIC_KEY at build time; debug builds also read it at run time, for tests), and
// only until it expires (a quarter of an hour after it was issued). The background sync refreshes
// it and keeps it in the trust directory, where it is verified again every time it is read, so a
// copied, edited, or replayed file grants nothing beyond its own expiry.
//
// Customers cannot turn it on: they hold no operator key, and pointing CRANE_OPERATOR_URL at
// another service only yields policies that fail verification. They can always turn it off on
// their own machine with CRANE_AGENT_COMMANDS=off.
//
// Even when it is on, only the commands in READ_ONLY may run, as one plain invocation: everything
// that signs, changes rules, keys, hooks, secrets, or integrations stays human-only, and commit()
// and every mutating command keep their own require_human check.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::platform::files::write_atomic;
use crate::platform::now_millis;
use crate::trust::crypto::verify_signature;

/** Signature domain of the operator's agent-command policy */
pub(crate) const POLICY_DOMAIN: &str = "crane/operator/agent-commands/v1";

/** How long a signed policy stays valid after it was issued, in milliseconds */
pub(crate) const POLICY_LIFETIME_MILLIS: u64 = 15 * 60 * 1000;

/** How far a policy's issue time may lie in the future (clock skew), in milliseconds */
const CLOCK_SKEW_MILLIS: u64 = 5 * 60 * 1000;

/** The operator dashboard machines read the policy from, unless the build or CRANE_OPERATOR_URL
 * names another */
pub(crate) const DEFAULT_OPERATOR_URL: &str = "https://dashboard.agentslandscape.cloud";

/** The operator's decision, as signed
 * Fields
    - enabled: bool - agents may run read-only Crane commands
    - issued_at: u64 - Unix milliseconds the service signed it
    - expires_at: u64 - Unix milliseconds after which it grants nothing
    - changed_by: String - GitHub login of the operator who last changed it
    - changed_at: u64 - Unix milliseconds of that change
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Policy {
    pub(crate) enabled: bool,
    pub(crate) issued_at: u64,
    pub(crate) expires_at: u64,
    pub(crate) changed_by: String,
    pub(crate) changed_at: u64,
}

impl Policy {
    /** The exact bytes the operator key signs
     * Input
        - None (uses self)
     * Output
        - Vec<u8>
    */
    pub(crate) fn message(&self) -> Vec<u8> {
        format!(
            "{}\n{}\n{}\n{}\n{}",
            self.enabled, self.issued_at, self.expires_at, self.changed_by, self.changed_at
        )
        .into_bytes()
    }
}

/** A policy with the operator's signature, as the dashboard serves it and the cache keeps it
 * Fields
    - policy: Policy - the decision
    - public_key: String - the key that signed it (hex), informational: verification uses the key
      compiled into the build, never this one
    - signature: String - Ed25519 signature over policy.message() under POLICY_DOMAIN
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Signed {
    pub(crate) policy: Policy,
    pub(crate) public_key: String,
    pub(crate) signature: String,
}

/** The operator public key this build trusts, None when the build has none (then agents never
 * run Crane commands)
 * Input
    - None
 * Output
    - Option<String> 64 hex characters
*/
pub(crate) fn operator_public_key() -> Option<String> {
    let compiled = option_env!("CRANE_OPERATOR_PUBLIC_KEY").map(str::to_string);
    // Debug builds (the test suite) may name the key at run time; release builds never do, so a
    // customer cannot substitute a key of their own
    let runtime = if cfg!(debug_assertions) {
        std::env::var("CRANE_OPERATOR_PUBLIC_KEY").ok()
    } else {
        None
    };
    runtime
        .or(compiled)
        .map(|key| key.trim().to_ascii_lowercase())
        .filter(|key| key.len() == 64 && key.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/** The operator dashboard address: CRANE_OPERATOR_URL, else CRANE_DEFAULT_OPERATOR_URL from the
 * build, else DEFAULT_OPERATOR_URL; None when set to off
 * Input
    - None
 * Output
    - Option<String> without a trailing slash
*/
pub(crate) fn operator_url() -> Option<String> {
    let value = std::env::var("CRANE_OPERATOR_URL").unwrap_or_else(|_| {
        option_env!("CRANE_DEFAULT_OPERATOR_URL")
            .filter(|url| !url.trim().is_empty())
            .unwrap_or(DEFAULT_OPERATOR_URL)
            .to_string()
    });
    let value = value.trim().trim_end_matches('/').to_string();
    match value.to_ascii_lowercase().as_str() {
        "" | "off" | "0" | "false" | "none" => None,
        _ => Some(value),
    }
}

/** Report whether this machine's person turned agent commands off (CRANE_AGENT_COMMANDS=off);
 * nothing a customer sets can turn them on
 * Input
    - None
 * Output
    - bool
*/
pub(crate) fn locally_off() -> bool {
    std::env::var("CRANE_AGENT_COMMANDS").is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no" | "none"
        )
    })
}

/** Report whether this machine fetches the operator policy at all: the build trusts an operator
 * key, the address is not off, and the person did not turn agent commands off
 * Input
    - None
 * Output
    - bool
*/
pub(crate) fn fetches() -> bool {
    operator_public_key().is_some() && operator_url().is_some() && !locally_off()
}

/** Check a signed policy against a key and the time
 * Input
    - signed: &Signed - the policy and its signature
    - public_key: &str - the trusted operator key
    - now: u64 - Unix milliseconds
 * Output
    - Result<(), String>
    - Error if the signature does not verify, it expired, or it claims to be issued in the future
      or for longer than POLICY_LIFETIME_MILLIS
*/
pub(crate) fn verify(signed: &Signed, public_key: &str, now: u64) -> Result<(), String> {
    let policy = &signed.policy;
    verify_signature(
        public_key,
        POLICY_DOMAIN,
        &policy.message(),
        &signed.signature,
    )
    .map_err(|_| "the operator policy is not signed by the operator key".to_string())?;
    if policy.issued_at > now.saturating_add(CLOCK_SKEW_MILLIS) {
        return Err("the operator policy claims to be issued in the future".into());
    }
    if policy.expires_at <= policy.issued_at
        || policy.expires_at - policy.issued_at > POLICY_LIFETIME_MILLIS
    {
        return Err("the operator policy claims an invalid lifetime".into());
    }
    if policy.expires_at <= now {
        return Err("the operator policy expired".into());
    }
    Ok(())
}

/** Where this machine keeps the last verified policy (outside every repository)
 * Input
    - None
 * Output
    - Result<std::path::PathBuf, String>
*/
fn cache_path() -> Result<std::path::PathBuf, String> {
    Ok(super::crane_home()?
        .join("operator")
        .join("agent-commands.json"))
}

/** Read the cached policy and verify it now
 * Input
    - None
 * Output
    - Result<Signed, String> - the policy, or why it grants nothing (no key in this build, no
      cache, unreadable, forged, expired)
*/
pub(crate) fn cached() -> Result<Signed, String> {
    let key = operator_public_key().ok_or("this build trusts no operator key")?;
    let text = std::fs::read_to_string(cache_path()?)
        .map_err(|_| "no operator policy has been received on this machine yet".to_string())?;
    let signed: Signed = serde_json::from_str(&text)
        .map_err(|_| "the cached operator policy is unreadable".to_string())?;
    verify(&signed, &key, now_millis())?;
    Ok(signed)
}

/** Report whether agents may run read-only Crane commands on this machine right now: the person
 * did not turn it off, and a verified, unexpired operator policy says on
 * Input
    - None
 * Output
    - bool
*/
pub(crate) fn enabled_now() -> bool {
    !locally_off() && cached().is_ok_and(|signed| signed.policy.enabled)
}

/** Fetch the operator policy and keep it when it verifies. A failure keeps the cached one, which
 * expires on its own; a verified "off" replaces it at once
 * Input
    - None
 * Output
    - Result<Signed, String> the policy kept
    - Error if the build trusts no key, the address is off, the dashboard cannot be reached, or its
      answer does not verify
*/
pub(crate) fn refresh() -> Result<Signed, String> {
    let key = operator_public_key().ok_or("this build trusts no operator key")?;
    let base = operator_url().ok_or("the operator address is off (CRANE_OPERATOR_URL)")?;
    if !base.starts_with("https://")
        && !["http://127.0.0.1", "http://localhost", "http://[::1]"]
            .iter()
            .any(|prefix| base.starts_with(prefix))
    {
        return Err(format!("the operator address must use https:// ({base})"));
    }
    let answer: Value = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .get(&format!("{base}/v1/agent-commands"))
        .set("User-Agent", concat!("crane/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|error| format!("the operator dashboard could not be reached: {error}"))?
        .into_json()
        .map_err(|error| format!("the operator dashboard sent an unreadable answer: {error}"))?;
    let signed: Signed = serde_json::from_value(answer)
        .map_err(|error| format!("the operator dashboard sent an unreadable policy: {error}"))?;
    verify(&signed, &key, now_millis())?;
    let text = serde_json::to_string_pretty(&signed).map_err(crate::platform::io_error)?;
    write_atomic(&cache_path()?, text.as_bytes())?;
    Ok(signed)
}

/** The commands an agent may run while the switch is on. A pattern matches the start of the
 * command's words; "*" is any one word, and "$" means only options (words starting with '-') may
 * follow. Every command here only reads: none signs, changes rules, keys, hooks, secrets,
 * integrations, or drafts, and none runs a program a person has not signed */
pub(crate) const READ_ONLY: &[&[&str]] = &[
    &[],
    &["help", "$"],
    &["--help", "$"],
    &["-h", "$"],
    &["--version", "$"],
    &["-V", "$"],
    &["validate"],
    &["test", "."],
    &["parse", "file"],
    &["policy", "*", "status"],
    &["policy-context", "*", "--view"],
    &["repo", "status"],
    &["repo", "--view"],
    &["governance", "graph"],
    &["admin", "list"],
    &["session", "current"],
    &["session", "."],
    &["task", "current"],
    &["task", "."],
    &["scope", "inspect"],
    &["scope", "resolve"],
    &["scope", "explain"],
    &["scope", "diff"],
    &["scope", "test"],
    &["scope", "audit"],
    &["selection", "inspect"],
    &["selection", "validate"],
    &["brick", "types"],
    &["brick", "list"],
    &["brick", "show"],
    &["brick", "history"],
    &["brick", "agents"],
    &["brick", "context", "preview"],
    &["brick", "context", "status"],
    &["brick", "monitor", "status"],
    &["brick", "inbox", "$"],
    &["brick", "inbox", "list"],
    &["flow", "list"],
    &["flow", "show"],
    &["flow", "draft"],
    &["flow", "incidents"],
    &["flow", "proposals", "$"],
    &["flow", "proposals", "list"],
    &["flow", "models", "$"],
    &["flow", "models", "list"],
    &["flow", "subagent", "list"],
    &["flow", "subagent", "catalog"],
    &["flow", "scenarios", "show"],
    &["resident", "catalog"],
    &["resident", "list"],
    &["resident", "show"],
    &["resident", "status"],
    &["resident", "context"],
    &["resident", "activations"],
    &["resident", "activation"],
    &["resident", "definition", "list"],
    &["resident", "definition", "show"],
    &["resident", "rca", "list"],
    &["resident", "rca", "show"],
    &["alert", "list"],
    &["alert", "log"],
    &["alert", "metrics"],
    &["integrate", "--status"],
    &["capture", "sources"],
    &["capture", "verify"],
    &["insight", "reports"],
    &["insight", "checklists"],
    &["capability", "list"],
    &["capability", "inspect"],
    &["capability", "versions"],
    &["capability", "stale"],
    &["skill", "inspect"],
    &["runbook", "inspect"],
    &["runbook", "status"],
    &["permission", "inspect"],
    &["permission", "check"],
    &["precondition", "inspect"],
    &["constraint", "inspect"],
    &["approval", "status"],
    &["isolation", "profiles"],
    &["isolation", "inspect"],
    &["verification", "inspect"],
    &["provenance", "inspect"],
    &["provenance", "verify"],
    &["budget", "inspect"],
    &["recovery", "inspect"],
    &["revocation", "inspect"],
];

/** Options that make an otherwise read-only command write or run something */
const WRITING_OPTIONS: &[&str] = &["--record", "--allow-commands", "--apply", "--fix"];

/** Report whether words are only options: each starts with '-', and the shared options that take
 * a value (--format, --request-id) are followed by it
 * Input
    - words: &[&str] - the remaining words
 * Output
    - bool
*/
fn only_options(words: &[&str]) -> bool {
    let mut rest = words.iter();
    while let Some(word) = rest.next() {
        if !word.starts_with('-') {
            return false;
        }
        if matches!(*word, "--format" | "--request-id") && rest.next().is_none() {
            return false;
        }
    }
    true
}

/** Report whether a command (the words after "crane") is read-only
 * Input
    - words: &[&str] - command words
 * Output
    - bool
*/
pub(crate) fn read_only(words: &[&str]) -> bool {
    if words
        .iter()
        .any(|word| WRITING_OPTIONS.contains(&word.split('=').next().unwrap_or_default()))
    {
        return false;
    }
    READ_ONLY.iter().any(|pattern| {
        let mut index = 0;
        for part in pattern.iter() {
            if *part == "$" {
                return only_options(&words[index..]);
            }
            match words.get(index) {
                Some(word) if *part == "*" || part == word => index += 1,
                _ => return false,
            }
        }
        // An empty pattern is bare 'crane' (the help): nothing may follow it
        !pattern.is_empty() || words.is_empty()
    })
}

/** Decide whether an agent may run these command words now: read-only and the switch on
 * Input
    - words: &[&str] - command words after "crane"
 * Output
    - Result<(), String> - the reason when it may not
*/
pub(crate) fn agent_may_run(words: &[&str]) -> Result<(), String> {
    if !read_only(words) {
        return Err("only a human runs Crane commands that change anything".into());
    }
    if locally_off() {
        return Err("agent commands are turned off on this machine (CRANE_AGENT_COMMANDS)".into());
    }
    match cached() {
        Ok(signed) if signed.policy.enabled => Ok(()),
        Ok(_) => Err("the operator has not allowed agents to run Crane commands".into()),
        Err(reason) => Err(format!(
            "agents may run read-only Crane commands only while the operator allows it ({reason})"
        )),
    }
}

/** For the hook: the words of a shell command that is exactly one plain Crane invocation (no
 * pipes, chains, redirections, substitutions, quotes, or environment assignments), None otherwise
 * Input
    - command: &str - the shell command
 * Output
    - Option<Vec<String>> the words after the program
*/
pub(crate) fn plain_invocation(command: &str) -> Option<Vec<String>> {
    let command = command.trim();
    if command.is_empty()
        || command
            .chars()
            .any(|character| ";|&<>`$(){}\"'\n\r*?~!#%^".contains(character))
    {
        return None;
    }
    let mut words = command.split_whitespace();
    let program = words.next()?.to_ascii_lowercase();
    let name = program.rsplit(['/', '\\']).next().unwrap_or_default();
    if name != "crane" && name != "crane.exe" {
        return None;
    }
    Some(words.map(str::to_string).collect())
}

/** For the hook: whether an agent's shell command may run because it is one plain, read-only
 * Crane invocation and the switch is on
 * Input
    - command: &str - the shell command
 * Output
    - bool
*/
pub(crate) fn shell_allowed(command: &str) -> bool {
    plain_invocation(command).is_some_and(|words| {
        let words = words.iter().map(String::as_str).collect::<Vec<_>>();
        agent_may_run(&words).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::{plain_invocation, read_only, verify, Policy, Signed, POLICY_DOMAIN};
    use crate::trust::crypto::SigningIdentity;

    /** Sign a policy with a key
     * Input
        - key: &SigningIdentity - operator key
        - enabled: bool - the switch
        - issued_at: u64 - issue time
        - lifetime: u64 - milliseconds
     * Output
        - Signed
    */
    fn signed(key: &SigningIdentity, enabled: bool, issued_at: u64, lifetime: u64) -> Signed {
        let policy = Policy {
            enabled,
            issued_at,
            expires_at: issued_at + lifetime,
            changed_by: "operator".into(),
            changed_at: issued_at,
        };
        Signed {
            signature: key.sign(POLICY_DOMAIN, &policy.message()),
            public_key: key.public_hex(),
            policy,
        }
    }

    /** Only the operator's signature, within its lifetime, makes a policy count
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn only_a_fresh_operator_signature_counts() {
        let operator = SigningIdentity::generate().unwrap();
        let customer = SigningIdentity::generate().unwrap();
        let now = 1_000_000_000;
        let good = signed(&operator, true, now, 60_000);
        assert!(verify(&good, &operator.public_hex(), now).is_ok());
        // Signed by anyone else
        let forged = signed(&customer, true, now, 60_000);
        assert!(verify(&forged, &operator.public_hex(), now).is_err());
        // Edited after signing
        let mut edited = good.clone();
        edited.policy.expires_at += 1;
        assert!(verify(&edited, &operator.public_hex(), now).is_err());
        // Expired, from the future, or claiming a long life
        assert!(verify(&good, &operator.public_hex(), now + 60_000).is_err());
        let future = signed(&operator, true, now + 10 * 60_000, 60_000);
        assert!(verify(&future, &operator.public_hex(), now).is_err());
        let long = signed(&operator, true, now, 24 * 60 * 60_000);
        assert!(verify(&long, &operator.public_hex(), now).is_err());
    }

    /** Read-only commands match; anything that writes, signs, or runs does not
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn only_read_only_commands_match() {
        for allowed in [
            vec![],
            vec!["validate"],
            vec!["validate", "--json"],
            vec!["test", "."],
            vec!["policy", "payments", "status"],
            vec!["scope", "resolve", "write", "app.py", "--json"],
            vec!["scope", "diff"],
            vec!["brick", "inbox"],
            vec!["brick", "inbox", "--format", "json"],
            vec!["flow", "show", "checkout"],
            vec!["flow", "models"],
            vec!["resident", "status"],
            vec!["--version"],
        ] {
            assert!(read_only(&allowed), "{allowed:?}");
        }
        for refused in [
            vec!["init"],
            vec!["protect", "a.py", "start-line", "1"],
            vec!["governance", "sign"],
            vec!["scope", "diff", "--record"],
            vec!["brick", "inbox", "apply", "brq_1"],
            vec!["brick", "activate", "x", "--reason", "r"],
            vec!["brick", "test", "x"],
            vec!["flow", "models", "add", "--provider", "p"],
            vec!["flow", "approve", "checkout"],
            vec!["agent", "install", "--profile", "claude"],
            vec!["agent", "hook", "--event", "pre-tool-use"],
            vec!["secret", "list"],
            vec!["integrate", "slack"],
            vec!["alert", "add", "x"],
            vec!["admin", "add", "KEY"],
            vec!["resident", "trigger", "--file", "f"],
            vec!["data", "clear"],
            vec!["help", "extra"],
        ] {
            assert!(!read_only(&refused), "{refused:?}");
        }
    }

    /** Only one plain Crane invocation is a candidate in a shell command
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn only_plain_invocations_are_candidates() {
        assert_eq!(
            plain_invocation("crane validate --json"),
            Some(vec!["validate".into(), "--json".into()])
        );
        assert_eq!(
            plain_invocation(r"C:\tools\crane.exe scope inspect"),
            Some(vec!["scope".into(), "inspect".into()])
        );
        for refused in [
            "crane validate && rm -rf .",
            "crane validate; crane init",
            "crane validate | tee out",
            "crane validate > out",
            "X=1 crane validate",
            "crane $(echo init)",
            "crane 'init'",
            "bash -c crane validate",
            "cargo run -- validate",
            "",
        ] {
            assert!(plain_invocation(refused).is_none(), "{refused}");
        }
    }
}
