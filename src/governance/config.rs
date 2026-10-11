// .crane/config.json: repository defaults (policy and checkpoint), the autonomy-credit model, and
// the optional smoke-test commands of 'crane test .'. Changed only through Crane commands, and
// covered by the signed registry manifest.

use super::admins::AdminStatement;
use serde::{Deserialize, Serialize};

/** Version of the configuration layout */
pub(crate) const CONFIG_FORMAT: u64 = 1;

/** Name of the policy created by init */
pub(crate) const DEFAULT_POLICY: &str = "default";

/** Repository configuration
 * Fields
    - format: u64 - layout version
    - default_policy: Option<String> - policy used when a command names none
    - default_checkpoint: Option<String> - checkpoint used when a command names none
    - autonomy: Autonomy - credit and safety model for agent sessions
    - tests: Vec<TestCommand> - commands 'crane test .' runs after the policies
    - admins: Option<AdminStatement> - the owner's signed admin list (absent until the owner adds
      an admin, and then not written, so existing repositories keep their signed digests)
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Config {
    pub(crate) format: u64,
    pub(crate) default_policy: Option<String>,
    pub(crate) default_checkpoint: Option<String>,
    pub(crate) autonomy: Autonomy,
    pub(crate) tests: Vec<TestCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) admins: Option<AdminStatement>,
}

impl Default for Config {
    /** Build the configuration init writes: the default policy, no default checkpoint yet, the
     * default autonomy model, and no test commands
     * Input
        - None
     * Output
        - Config
    */
    fn default() -> Self {
        Self {
            format: CONFIG_FORMAT,
            default_policy: Some(DEFAULT_POLICY.into()),
            default_checkpoint: None,
            autonomy: Autonomy::default(),
            tests: Vec::new(),
            admins: None,
        }
    }
}

/** The autonomy model of agent sessions: credits are risk-bearing authority, separate from
 * tokens, cost, and compute; a balance never grants a permission or overrides a denial
 * Fields
    - mode: String - observe, assisted, delegated, or autonomous
    - session_credits: u64 - credits granted when a session starts
    - low_credit_threshold: u64 - remaining credits that raise a low-autonomy alert
    - credit_ttl_seconds: u64 - lifetime of a grant; unused credits then expire
    - quarantine_after_bypass_attempts: u64 - bypass attempts that quarantine a session
    - costs: Costs - credits each kind of action costs
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Autonomy {
    pub(crate) mode: String,
    pub(crate) session_credits: u64,
    pub(crate) low_credit_threshold: u64,
    pub(crate) credit_ttl_seconds: u64,
    pub(crate) quarantine_after_bypass_attempts: u64,
    pub(crate) costs: Costs,
}

impl Default for Autonomy {
    /** Build the default autonomy model: delegated mode, 100 credits for 8 hours, an alert at 20,
     * quarantine after 3 bypass attempts
     * Input
        - None
     * Output
        - Autonomy
    */
    fn default() -> Self {
        Self {
            mode: "delegated".into(),
            session_credits: 100,
            low_credit_threshold: 20,
            credit_ttl_seconds: 8 * 60 * 60,
            quarantine_after_bypass_attempts: 3,
            costs: Costs::default(),
        }
    }
}

/** Credits charged per action, by operation and risk factor
 * Fields
    - read: u64 - reading files or searching
    - write: u64 - writing a file
    - write_selection_file: u64 - extra for writing a file that holds selections
    - execute: u64 - running a shell command (effects unknown until it runs)
    - network: u64 - extra for an action that reaches the network
    - other: u64 - any other tool (MCP servers and new built-ins)
    - violation_penalty: u64 - credits revoked when a violation is detected after the effect
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Costs {
    pub(crate) read: u64,
    pub(crate) write: u64,
    pub(crate) write_selection_file: u64,
    pub(crate) execute: u64,
    pub(crate) network: u64,
    pub(crate) other: u64,
    pub(crate) violation_penalty: u64,
}

impl Default for Costs {
    /** Build the default cost model
     * Input
        - None
     * Output
        - Costs
    */
    fn default() -> Self {
        Self {
            read: 0,
            write: 2,
            write_selection_file: 3,
            execute: 3,
            network: 5,
            other: 2,
            violation_penalty: 10,
        }
    }
}

/** One smoke-test command run by 'crane test .'
 * Fields
    - name: String - label in the report
    - command: String - shell command, run at the repository top level
    - timeout_seconds: u64 - time limit, 600 when omitted
*/
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TestCommand {
    pub(crate) name: String,
    pub(crate) command: String,
    #[serde(default = "default_timeout")]
    pub(crate) timeout_seconds: u64,
}

/** Return the default test timeout
 * Input
    - None
 * Output
    - u64 seconds (600)
*/
fn default_timeout() -> u64 {
    600
}

/** Check that an autonomy mode name is valid
 * Input
    - mode: &str - candidate mode
 * Output
    - bool
*/
pub(crate) fn valid_mode(mode: &str) -> bool {
    matches!(mode, "observe" | "assisted" | "delegated" | "autonomous")
}
