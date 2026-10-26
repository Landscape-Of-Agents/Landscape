// The command line. Only the commands of the Crane command set exist; every other word is an
// unknown command. 'crane agent hook' is the internal entry point the installed agent hooks call
// (it is how every pre and post hook passes through Crane) and is refused when an agent runs it.

mod agent;
mod alert;
mod approval;
mod args;
mod audit;
mod budget;
mod capability;
mod capture;
mod compile;
mod constraint;
mod data;
mod discover;
mod evolution;
mod family;
mod governance;
mod insight;
mod invoke;
mod isolation;
mod lab;
mod link;
mod permission;
mod precondition;
mod protocol;
mod provenance;
mod recovery;
mod repo;
mod revocation;
mod runbook;
mod runtime;
mod scope;
mod selection;
mod skill;
mod verification;

use std::env;

/** Read the exit code a family command's result carries (see protocol)
 * Input
    - error: &str - error returned by run()
 * Output
    - Option<(i32, &str)> code and message, None for an ordinary error
*/
pub(crate) fn exit_code(error: &str) -> Option<(i32, &str)> {
    protocol::exit(error)
}

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
    // Only a human runs Crane. Inside an agent session only the entry points the agent's own hooks
    // start are allowed (the hooks deny typing them from the agent's shell)
    if let Some(marker) = crate::trust::agent_environment() {
        if !matches!(words.as_slice(), ["agent", "hook", ..] | ["agent", "sync"]) {
            return Err(format!(
                "crane cannot be executed by an AI agent ({marker} is set); only a human runs Crane"
            ));
        }
    }
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
        ["data", ..] => data::data(&rest(1)),
        ["github", ..] => repo::github(&rest(1)),
        ["task", ..] => runtime::task(&rest(1)),
        ["session", ..] => runtime::session(&rest(1)),
        ["agent", ..] => agent::run(&rest(1)),
        ["integrate", ..] => repo::integrate(&rest(1)),
        ["alert", ..] => alert::alert(&rest(1)),
        ["admin", ..] => governance::admin(&rest(1)),
        ["governance", ..] => governance::governance(&rest(1)),
        ["scope", ..] => scope::scope(&rest(1)),
        ["skill", ..] => skill::skill(&rest(1)),
        ["runbook", ..] => runbook::runbook(&rest(1)),
        ["permission", ..] => permission::permission(&rest(1)),
        ["precondition", ..] => precondition::precondition(&rest(1)),
        ["constraint", ..] => constraint::constraint(&rest(1)),
        ["approval", ..] => approval::approval(&rest(1)),
        ["isolation", ..] => isolation::isolation(&rest(1)),
        ["verification", ..] => verification::verification(&rest(1)),
        ["provenance", ..] => provenance::provenance(&rest(1)),
        ["budget", ..] => budget::budget(&rest(1)),
        ["recovery", ..] => recovery::recovery(&rest(1)),
        ["revocation", ..] => revocation::revocation(&rest(1)),
        ["selection", ..] => selection::selection(&rest(1)),
        ["capture", ..] => capture::capture(&rest(1)),
        ["insight", ..] => insight::insight(&rest(1)),
        ["capability", ..] => capability::capability(&rest(1)),
        ["compile", ..] => compile::compile(&rest(1)),
        ["lab", ..] => lab::lab(&rest(1)),
        ["evolution", ..] => evolution::evolution(&rest(1)),
        ["foxx", "event", ..] => audit::foxx_event(&rest(2)),
        ["foxx", "live", ..] => audit::foxx_live(&rest(2)),
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
  crane protect FILE context TEXT [policy NAME] [checkpoint NAME] [start-line N] [end-line N]
                [name LABEL]
  crane target FILE context TEXT [policy NAME] [checkpoint NAME] [start-line N] [end-line N]
               [name LABEL] [change_type logical_bn|logical_cn|logical_sn|semantic]
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
  crane repo rebind OLD_REMOTE  (after a transfer or rename: proves the move, then re-signs)

Hosted data
  crane data clear           (CODEOWNERS only; permanently clears this repository's hosted data)

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
  crane governance graph [--json]   (zones, flows, and selections as an acyclic graph)

Scope (the effective boundary, decided by the same guard the hooks use; read-only)
  crane scope inspect [PATH ...] [--json]   (the boundary, its enforcement, and what is unresolved)
  crane scope resolve read|write|delete|execute|network TARGET [lines A-B] [mode MODE] [--json]
                                            (decide a proposed action and record the decision)
  crane scope explain DECISION_ID|TOOL_CALL_ID [--json]   (why, and whether it still applies)
  crane scope diff [--record] [--json]      (the boundary against the last signed baseline; fails
                                             when it widened)
  crane scope test [--json]                 (positive and adversarial probes; fails when one fails)
  crane scope audit [session ID] [days N] [--json]   (the boundary against recorded attempts)

Work Capture (evidence about a work episode, packaged as a bounded Evidence Pack)
  crane capture demo [--json]        (deterministic failed-deployment investigation from fixtures)
  crane capture sources [--json]     (this repository's evidence sources and their health)
  crane capture investigate [commit SHA] [task TASK_ID] [class CLASS] [recipe ID [version N]] [episode EPI] [--json]
  crane capture replay QUERY_ID [--json]   (re-run a recorded investigation; what changed)
  crane capture verify PACK_ID             (check an Evidence Pack against its manifest)
  crane capture contribute FILE [--json]   (propose context; it stays proposed until reviewed)
  crane capture review CONTRIBUTION_ID accept|reject reason TEXT   (owner and admins only)
  crane capture outcome EPISODE OUTCOME [evidence ID,ID]
  crane capture feedback PACK_ID sufficient yes|no|unknown [missing TYPE,TYPE] [notes TEXT]
  crane capture ask FILE                   (ask a question; it routes to permitted sources only)
  crane capture question QUESTION_ID answer PACK_ID|block|unresolvable|reopen
  crane capture confirm RELATION_ID        (record a reviewed relation; owner and admins only)
  (a question class no recipe serves is explored: the pack is Exploratory, never Sufficient)

Insight (patterns across the work Work Capture kept; proposals people review, nothing applied)
  crane insight report [--json]      (evidence gaps, unanswered questions, trust, outcomes, work)
  crane insight reports [--json]     (the kept reports)
  crane insight propose PATTERN_ID   (a recipe proposal, or context for a zone; stays proposed)
  crane insight recipes [--json]     (recipe proposals and their status)
  crane insight review PROPOSAL_ID accept|reject reason TEXT   (owner and admins only)

Capabilities (versioned, validated reusable procedures; publication needs an admin key)
  crane capability demo [--json]
  crane capability intake episode EPISODE pack PACK digest DIGEST [intent FILE] [risk RISK] [profile NAME]
  crane capability create|propose FILE [profile NAME]
  crane capability validate (ID [version N] [--dry-run] | file FILE)
  crane capability inspect ID [version N] | versions ID | list [state STATE] [--all]
  crane capability publish ID version N reason TEXT [acknowledge CODE,CODE]
  crane capability deprecate|revoke ID [version N] reason TEXT | reject ID version N reason TEXT
  crane capability stale
  crane capability shared [ID] [--json]   (the signed shared skill store, .crane/skills.json)

Capability compiler (evidence of one episode -> a capability candidate; never validates or publishes)
  crane compile demo [--json]
  crane compile episode EPISODE pack PACK digest DIGEST capability ID name TEXT purpose TEXT
                task-class CLASS [intent FILE] [risk RISK] [profile NAME] [--json]

Capability Validation Lab (deterministic checks of an exact candidate; nothing executed)
  crane lab demo [--json]
  crane lab run ID [version N] [digest DIGEST] plan FILE [compilation REPORT_ID] [--json]
  crane lab reports ID [version N] [--json]

Capability discovery (published, eligible capabilities for a task, with explanations; read-only)
  crane capability discover demo [--json]
  crane capability discover task-class CLASS [environment ENV] [repository REPO]
                 [preconditions ID,ID] [entities ID,ID] [pack PACK digest DIGEST] [--json]
  crane capability discover reports [--json]

Capability invocation (exact published versions, registered operations only, verified outcomes)
  crane capability invoke demo [--json]
  crane capability invoke plan ID version N digest DIGEST bindings FILE key KEY [inputs FILE]
                 [environment ENV] [repository REPO] [task TASK] [preconditions ID,ID]
                 [discovery REPORT_ID] [compilation REPORT_ID] [--execute] [--json]
  crane capability invoke approve INVOCATION_ID plan-digest DIGEST
  crane capability invoke run|inspect|reconcile INVOCATION_ID
  crane capability invoke abandon INVOCATION_ID reason TEXT
  crane capability invoke list

Capability evolution (evidence-backed revision proposals and regression checks; never publishes)
  crane evolution demo [--json]
  crane evolution analyze|drift ID [version N] [--json]
  crane evolution signals|proposals [capability ID] [status STATUS] [--json]
  crane evolution signal inspect SIGNAL_ID | signal dismiss SIGNAL_ID reason TEXT
  crane evolution propose SIGNAL_ID [changes FILE] [--json]
  crane evolution proposal inspect PROPOSAL_ID | proposal abandon PROPOSAL_ID reason TEXT
  crane evolution regression plan|run|history PROPOSAL_ID | regression inspect RUN_ID
  crane evolution prepare-review PROPOSAL_ID   (a review packet; nothing is delivered)

Skills (department-neutral names for the capability lifecycle; 'crane skill' for the details)
  crane skill create|validate|inspect|test|plan|activate|invoke|retire|revoke ...

Runbooks (exact skill versions in order, with approvals, branches, retries, and a budget)
  crane runbook create|validate|inspect|activate|plan|test|run|status|cancel|resume|revoke ...

Controls (inspect, check, and test what Crane applies automatically at every protected action)
  crane permission inspect|check|test       crane precondition inspect|validate|evaluate|test
  crane constraint inspect|validate|test    crane approval request|status|approve|deny|revoke|test
  crane isolation profiles|inspect|validate|test
  crane verification inspect|evaluate|report|test
  crane provenance inspect|verify|export|test
  crane budget inspect|evaluate|reserve|settle|release|test
  crane recovery inspect|reconcile|resume|abort|test
  crane revocation inspect|revoke|test
  crane selection protect|target|inspect|validate|repair   (protect and target as above)
  (these commands take --format json; exit codes: 0 ok or passed, 3 denied, failed, or blocked,
   4 approval required, 5 unknown or unresolved)

Admins
  crane admin register          (owner: a GitHub admin confirms the owner key on the dashboard)
  crane admin list
  crane admin add PUBLIC_KEY [name NAME]
  crane admin del PUBLIC_KEY

Agent
  crane agent link              (link this machine's upload key to your GitHub account)
  crane agent install --profile claude|codex|copilot|gemini
  crane agent uninstall --profile claude|codex|copilot|gemini
  crane agent hooks --profile claude|codex|copilot|gemini

Integrations
  crane integrate jira|whatsapp|slack|github

Alert rules (evaluated by the background sync; shown on the dashboard)
  crane alert add NAME metric METRIC above|at_least|below|at_most|equals N [window 15m]
                  [channels slack,whatsapp] [severity high] [cooldown 30]
                  [where PATH OPERATOR VALUES]... [quiet-resolve]
  crane alert list|check [--json]
  crane alert remove|enable|disable|test NAME
  crane alert log [--json]
  crane alert metrics

Selections are created only by protect and target: Crane inserts language-appropriate anchor
comments around the lines (the whole file when no lines are given), generates the 8-character
marker, signs the registry record, and adds 'preserve MARKER;' or 'target MARKER;' to the policy.
checkpoint, protect, target, the defaults, policy changes, repo and github changes, integrations,
admin changes, governance sign, and agent uninstall cannot be executed by an AI agent; the installed hooks also deny them, and
deny every agent change to .crane (including .crane/map), even when approval is requested.
"#;
