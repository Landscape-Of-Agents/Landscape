// The command line. Only the commands of the Crane command set exist; every other word is an
// unknown command. 'crane agent hook' is the internal entry point the installed agent hooks call
// (it is how every pre and post hook passes through Crane) and is refused when an agent runs it.

mod agent;
mod args;
mod audit;
mod governance;
mod link;
mod repo;
mod runtime;

use std::env;

/** Dispatch the command line to its handler
 * Input
    - None (arguments are read from the process environment)
 * Output
    - Result<(), String>
    - Error if the command is unknown or its handler fails
*/
pub(crate) fn run() -> Result<(), String> {
    let arguments = env::args().skip(1).collect::<Vec<_>>();
    let started = crate::platform::now_millis();
    let result = dispatch(&arguments);
    audit::record(&arguments, &result, started);
    result
}

/** Match the arguments to the handler of their command
 * Input
    - arguments: &[String] - arguments after "crane"
 * Output
    - Result<(), String>
    - Error if the command is unknown or its handler fails
*/
fn dispatch(arguments: &[String]) -> Result<(), String> {
    let words = arguments.iter().map(String::as_str).collect::<Vec<_>>();
    let rest = |count: usize| arguments[count.min(arguments.len())..].to_vec();
    match words.as_slice() {
        [] | ["help" | "--help" | "-h", ..] => {
            print!("{HELP}");
            Ok(())
        }
        ["--version" | "-V", ..] => {
            println!("crane {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["init", ..] => governance::init(&rest(1)),
        ["checkpoint", ..] => governance::checkpoint(&rest(1)),
        ["protect", ..] => governance::select(crate::selection::registry::Operation::Preserve, &rest(1)),
        ["target", ..] => governance::select(crate::selection::registry::Operation::Target, &rest(1)),
        ["--set", "default", "policy", ..] => governance::set_default_policy(&rest(3)),
        ["--set", "default", "checkpoint", ..] => governance::set_default_checkpoint(&rest(3)),
        ["--set", ..] => Err("usage: crane --set default policy NAME | crane --set default checkpoint NAME".into()),
        ["parse", "file", ..] => governance::parse(&rest(2)),
        ["parse", ..] => Err("usage: crane parse file FILE".into()),
        ["validate", ..] => governance::validate(&rest(1)),
        ["test", ..] => governance::test(&rest(1)),
        ["add", "policy-context", "file", ..] => governance::add_context(&rest(3)),
        ["add", "policy", ..] => governance::move_marker(&rest(2)),
        ["add", ..] => Err("usage: crane add policy-context file FILE [policy NAME] | crane add policy NAME marker MARKER".into()),
        ["policy-context", ..] => governance::view_context(&rest(1)),
        ["create", "policy", ..] => governance::create_policy(&rest(2)),
        ["create", ..] => Err("usage: crane create policy NAME FILENAME".into()),
        ["policy", _, "status", ..] => governance::policy_status(&words[1..2].iter().map(std::string::ToString::to_string).chain(rest(3)).collect::<Vec<_>>()),
        ["policy", ..] => Err("usage: crane policy NAME status".into()),
        ["repo", ..] => repo::repo(&rest(1)),
        ["github", ..] => repo::github(&rest(1)),
        ["task", ..] => runtime::task(&rest(1)),
        ["session", ..] => runtime::session(&rest(1)),
        ["agent", ..] => agent::run(&rest(1)),
        ["integrate", ..] => repo::integrate(&rest(1)),
        ["admin", ..] => governance::admin(&rest(1)),
        ["governance", ..] => governance::governance(&rest(1)),
        ["foxx", "event", ..] => audit::foxx_event(&rest(2)),
        #[cfg(feature = "server")]
        ["server", ..] => crate::server::run(&rest(1)),
        [command, ..] => Err(format!("unknown command '{command}'; run 'crane help'")),
    }
}

/** The command reference printed by crane help */
const HELP: &str = r#"Crane: signed selection contracts and runtime enforcement for AI coding agents

Basic
  crane --version
  crane init
  crane checkpoint [NAME]
  crane protect FILE [policy NAME] [checkpoint NAME] [start-line N] [end-line N] [name LABEL]
  crane target FILE [policy NAME] [checkpoint NAME] [start-line N] [end-line N] [name LABEL]
               [change_type logical_bn|logical_cn|logical_sn|semantic]
  crane --set default policy NAME
  crane --set default checkpoint NAME
  crane parse file FILE
  crane validate [--json]
  crane test . [--json]
  crane add policy-context file FILE [policy NAME]
  crane policy-context NAME --view

Repository
  crane repo --ssh|--https|--gh-cli REMOTE [rf] [codeowners] [contributors] [ci-cd]
             [gh-actions] [gh-branches] [gh-insights]
  crane github
  crane github days DAYS
  crane repo status
  crane repo --view
  crane repo del

Policies
  crane create policy NAME FILENAME
  crane add policy NAME marker MARKER
  crane policy NAME status

Tasks and sessions
  crane task current [TASK_ID]
  crane task .
  crane session current [SESSION_ID]
  crane session .


Zones and flows (.crane/governance.yaml)
  crane governance sign

Admins
  crane admin register          (owner: a GitHub admin confirms the owner key on the dashboard)
  crane admin list
  crane admin add PUBLIC_KEY [name NAME]
  crane admin del PUBLIC_KEY

Agent
  crane agent link              (link this machine's upload key to your GitHub account)
  crane agent install --profile claude|codex
  crane agent uninstall --profile claude|codex
  crane agent hooks --profile claude|codex

Integrations
  crane integrate jira|whatsapp|slack|github

Selections are created only by protect and target: Crane inserts language-appropriate anchor
comments around the lines (the whole file when no lines are given), generates the 8-character
marker, signs the registry record, and adds 'preserve MARKER;' or 'target MARKER;' to the policy.
checkpoint, protect, target, the defaults, policy changes, repo and github changes, integrations,
admin changes, governance sign, and agent uninstall cannot be executed by an AI agent; the installed hooks also deny them, and
deny every agent change to .crane (including .crane/map), even when approval is requested.
"#;
