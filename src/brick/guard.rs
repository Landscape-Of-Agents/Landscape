// The Brick rules decided on every agent tool call, after the guard's own decision. What only the
// call reveals is decided here: who is calling (Identity), what is read and which operation each
// change is (Access), what kind of change it is (Approval), how much the session has done (Limits),
// what the call reaches and reveals (Environment), gates that change without a file changing
// (Precondition), operations agents must leave to Crane (Action), failures in a row (Recovery), and
// a firing Watch (pause). Every rule can only make the decision stricter: a finding is Deny or
// RequireApproval, never Allow, so Bricks still cannot widen what an agent may do.
//
// A call belongs to the targets of the files it reads, changes, or names in a command (a new file
// to the targets of the files beside it). Network and hidden-variable rules also apply in the
// targets the session already worked in, and repository-wide commands (commit, push, package
// installs, deployments) to every target whose Bricks name them: those calls name no file.

use std::collections::{BTreeMap, BTreeSet};

use super::compose::{controls, CommitGate, Controls};
use super::config::{ChangeClass, Escalation, Mode};
use super::store::BrickStore;
use super::usage::{Counted, Daily, TargetUsage, Usage};
use crate::governance::state::State;
use crate::governance::workspace::Workspace;
use crate::governance::zones::Limits;
use crate::hooks::action::Operation;
use crate::platform::glob;
use crate::telemetry::model::Decision;

/** Direct operations an Action Brick can keep from agents, with the command words that do them */
pub(crate) const DIRECT: &[(&str, &str, &[&[&str]])] = &[
    ("git", "git.commit", &[&["git", "commit"]]),
    ("git", "git.push", &[&["git", "push"]]),
    ("git", "git.tag", &[&["git", "tag"]]),
    ("git", "git.merge", &[&["git", "merge"]]),
    ("git", "git.rebase", &[&["git", "rebase"]]),
    ("git", "git.reset", &[&["git", "reset"]]),
    ("github", "github.pr_create", &[&["gh", "pr", "create"]]),
    ("github", "github.pr_merge", &[&["gh", "pr", "merge"]]),
    (
        "github",
        "github.release_create",
        &[&["gh", "release", "create"]],
    ),
    (
        "github",
        "github.workflow_run",
        &[&["gh", "workflow", "run"]],
    ),
    ("packages", "packages.install", INSTALLS),
    (
        "kubernetes",
        "kubectl.apply",
        &[&["kubectl", "apply"], &["kubectl", "delete"]],
    ),
    (
        "terraform",
        "terraform.apply",
        &[&["terraform", "apply"], &["terraform", "destroy"]],
    ),
    ("docker", "docker.push", &[&["docker", "push"]]),
];

/** Commands that install or add packages */
const INSTALLS: &[&[&str]] = &[
    &["npm", "install"],
    &["npm", "i"],
    &["npm", "add"],
    &["pnpm", "add"],
    &["pnpm", "install"],
    &["yarn", "add"],
    &["pip", "install"],
    &["pip3", "install"],
    &["uv", "add"],
    &["uv", "pip"],
    &["poetry", "add"],
    &["pipx", "install"],
    &["cargo", "add"],
    &["cargo", "install"],
    &["go", "get"],
    &["go", "install"],
    &["gem", "install"],
    &["bundle", "add"],
    &["composer", "require"],
    &["apt", "install"],
    &["apt-get", "install"],
    &["brew", "install"],
    &["choco", "install"],
    &["winget", "install"],
];

/** Dependency manifests and lock files */
const MANIFESTS: &[&str] = &[
    "package.json",
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "npm-shrinkwrap.json",
    "Cargo.toml",
    "Cargo.lock",
    "pyproject.toml",
    "poetry.lock",
    "Pipfile",
    "Pipfile.lock",
    "uv.lock",
    "go.mod",
    "go.sum",
    "Gemfile",
    "Gemfile.lock",
    "composer.json",
    "composer.lock",
    "pom.xml",
    "build.gradle",
    "build.gradle.kts",
    "packages.config",
];

/** The Brick rules of every target with attachments
 * Fields
    - targets: BTreeMap<String, Controls> - controls by target key (only targets with call rules)
    - paused: BTreeSet<String> - targets a firing Watch pauses
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct Rules {
    pub(crate) targets: BTreeMap<String, Controls>,
    pub(crate) paused: BTreeSet<String>,
}

impl Rules {
    /** Load the rules from the signed store (empty when it does not parse: integrity already stops
     * every change then)
     * Input
        - workspace: &Workspace - repository
        - state: &State - governance state
        - runtime: Option<&std::path::Path> - runtime directory (firing watches)
     * Output
        - Rules
    */
    pub(crate) fn load(
        workspace: &Workspace,
        state: &State,
        runtime: Option<&std::path::Path>,
    ) -> Self {
        let Ok(store) = BrickStore::of(state) else {
            return Self::default();
        };
        let keys = store
            .attachments
            .iter()
            .map(|attachment| attachment.target.key())
            .collect::<BTreeSet<_>>();
        let mut rules = Self::default();
        for key in keys {
            let combined = controls(&store, &key, &workspace.root);
            if combined.decides_calls() {
                rules.targets.insert(key, combined);
            }
        }
        if let Some(runtime) = runtime.filter(|_| {
            rules
                .targets
                .values()
                .any(|controls| !controls.watch_pause.is_empty())
        }) {
            for (target, brick) in super::runtime::firing(runtime) {
                if rules
                    .targets
                    .get(&target)
                    .is_some_and(|controls| controls.watch_pause.contains(&brick))
                {
                    rules.paused.insert(target);
                }
            }
        }
        rules
    }
}

/** One file a call changes
 * Fields
    - path: String - repository-relative file
    - existed: bool - the file exists now
    - deleted: bool - the change deletes or empties it
    - lines: u64 - lines changed
*/
#[derive(Debug, Clone)]
pub(crate) struct Change {
    pub(crate) path: String,
    pub(crate) existed: bool,
    pub(crate) deleted: bool,
    pub(crate) lines: u64,
}

/** A call as the Brick rules see it
 * Fields
    - operation: Operation - kind of call
    - reads: Vec<String> - repository files it reads
    - changes: Vec<Change> - repository files it changes
    - named: Vec<String> - repository files its command names
    - command: Option<&'a str> - shell command
    - arguments: &'a [String] - every string argument
    - network: &'a [String] - hosts it reaches
    - agent: Option<&'a str> - provider (claude, codex, ...)
    - agent_id: Option<&'a str> - the agent's id
    - model: Option<&'a str> - model
    - started_at: Option<u64> - session start, Unix milliseconds
    - usage: Option<&'a Usage> - what the session did so far
    - daily: Option<&'a Daily> - what every session did today
    - now: u64 - Unix milliseconds
*/
pub(crate) struct Call<'a> {
    pub(crate) operation: Operation,
    pub(crate) reads: Vec<String>,
    pub(crate) changes: Vec<Change>,
    pub(crate) named: Vec<String>,
    pub(crate) command: Option<&'a str>,
    pub(crate) arguments: &'a [String],
    pub(crate) network: &'a [String],
    pub(crate) agent: Option<&'a str>,
    pub(crate) agent_id: Option<&'a str>,
    pub(crate) model: Option<&'a str>,
    pub(crate) started_at: Option<u64>,
    pub(crate) usage: Option<&'a Usage>,
    pub(crate) daily: Option<&'a Daily>,
    pub(crate) now: u64,
}

/** A stricter decision a Brick asks for
 * Fields
    - decision: Decision - Deny or RequireApproval
    - reason: String - why
    - rule: &'static str - rule code
    - brick: String - Brick reference
*/
#[derive(Debug, Clone)]
pub(crate) struct Finding {
    pub(crate) decision: Decision,
    pub(crate) reason: String,
    pub(crate) rule: &'static str,
    pub(crate) brick: String,
}

/** What the rules decided about a call
 * Fields
    - findings: Vec<Finding> - stricter decisions
    - counted: Counted - what the call counts in the session's usage
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct Outcome {
    pub(crate) findings: Vec<Finding>,
    pub(crate) counted: Counted,
}

/** The targets a file belongs to: its own, or (a new file) those of the files beside it
 * Input
    - limits: &Limits - per-file limits
    - file: &str - repository-relative file
 * Output
    - Vec<String>
*/
pub(crate) fn targets_of(limits: &Limits, file: &str) -> Vec<String> {
    if let Some(limit) = limits.get(file) {
        return limit.entities.clone();
    }
    let Some((directory, _)) = file.rsplit_once('/') else {
        return Vec::new();
    };
    limits
        .iter()
        .filter(|(path, _)| {
            path.rsplit_once('/')
                .is_some_and(|(parent, _)| parent == directory)
        })
        .flat_map(|(_, limit)| limit.entities.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/** Report whether the command's words contain a sequence (as consecutive words)
 * Input
    - words: &[String] - lowercase command words
    - sequence: &[&str] - words
 * Output
    - bool
*/
fn has(words: &[String], sequence: &[&str]) -> bool {
    !sequence.is_empty()
        && words.windows(sequence.len()).any(|window| {
            window
                .iter()
                .zip(sequence)
                .all(|(word, wanted)| word == wanted)
        })
}

/** Options a tool takes before its subcommand that also take a value, by tool; a tool not listed
 * here is left as written */
fn options_with_values(tool: &str) -> Option<&'static [&'static str]> {
    Some(match tool {
        "git" => &[
            "-c",
            "--git-dir",
            "--work-tree",
            "--namespace",
            "--config-env",
            "--super-prefix",
        ],
        "gh" => &["-r", "--repo"],
        "kubectl" => &[
            "-n",
            "--namespace",
            "--context",
            "--kubeconfig",
            "--cluster",
            "--user",
            "-s",
            "--server",
        ],
        "docker" => &[
            "-c",
            "--context",
            "-h",
            "--host",
            "--config",
            "-l",
            "--log-level",
        ],
        "terraform" | "brew" | "choco" | "winget" | "gem" | "pipx" | "apt" | "apt-get" => {
            &["-c", "-o", "-t"]
        }
        "npm" | "pnpm" | "yarn" => &[
            "-c",
            "--prefix",
            "--dir",
            "--cwd",
            "--filter",
            "-f",
            "-w",
            "--workspace",
        ],
        "pip" | "pip3" => &[
            "--python",
            "--proxy",
            "--log",
            "--cache-dir",
            "--retries",
            "--timeout",
        ],
        "uv" => &[
            "--directory",
            "--project",
            "--python",
            "-p",
            "--config-file",
        ],
        "poetry" => &["-c", "--directory", "-p", "--project"],
        "cargo" => &["--config", "-z", "--color"],
        "go" => &["-c"],
        "bundle" | "composer" => &["-d", "--working-dir"],
        _ => return None,
    })
}

/** Rewrite command words so the same operation reads the same however it is spelled: a program by
 * its path or with .exe is its name, options between a tool and its subcommand are dropped
 * ('git -C x -c a=b push' is 'git push'), and 'python -m pip' is 'pip'. Words are text: a script, an
 * alias, or a Makefile can still do the operation without naming it
 * Input
    - words: &[String] - lowercase command words
 * Output
    - Vec<String>
*/
fn normalized(words: &[String]) -> Vec<String> {
    let mut output = Vec::with_capacity(words.len());
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        index += 1;
        let name = word.rsplit('/').next().unwrap_or(word);
        let name = name.strip_suffix(".exe").unwrap_or(name);
        if (name.starts_with("python") || name == "py")
            && words.get(index).is_some_and(|next| next == "-m")
        {
            index += 1;
            continue;
        }
        let Some(valued) = options_with_values(name) else {
            output.push(word.clone());
            continue;
        };
        output.push(name.to_string());
        while let Some(next) = words.get(index) {
            let toolchain = name == "cargo" && next.starts_with('+');
            if !next.starts_with('-') && !toolchain {
                break;
            }
            index += 1;
            if valued.contains(&next.as_str()) {
                index += 1;
            }
        }
    }
    output
}

/** A command as the shell would read it after the tricks that hide a word from plain matching:
 * line continuations, quotes and backslashes inside a word (g"i"t, gi\\t), and variables set in
 * the same command (G=git; $G push). None when the command holds no such thing
 * Input
    - command: &str - shell command
 * Output
    - Option<String>
*/
fn unmasked(command: &str) -> Option<String> {
    let mut text = command
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace(['"', '\'', '\\'], "");
    let assigned = text
        .split(|c: char| c.is_whitespace() || ";&|".contains(c))
        .filter_map(|word| word.split_once('='))
        .filter(|(name, value)| {
            !value.is_empty()
                && !name.is_empty()
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                && !name.starts_with(|c: char| c.is_ascii_digit())
        })
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    for (name, value) in assigned {
        text = text
            .replace(&format!("${{{name}}}"), &value)
            .replace(&format!("${name}"), &value);
    }
    (text != command).then_some(text)
}

/** What an agent's commit message lacks for a Provenance gate, reading the message from the command
 * itself. A message the command does not show (an editor, -F, --no-edit) counts as lacking: the
 * person it goes to can look
 * Input
    - command: &str - shell command
    - gate: &CommitGate - the gate
 * Output
    - Vec<String>
*/
fn commit_lacks(command: &str, gate: &CommitGate) -> Vec<String> {
    let lower = command.to_ascii_lowercase();
    let mut lacks = Vec::new();
    if let Some(key) = &gate.trailer {
        let key = key.to_ascii_lowercase();
        let colon = format!("{key}:");
        let written = lower.match_indices(&colon).any(|(start, _)| {
            lower[start + colon.len()..]
                .trim_start_matches([' ', '\t'])
                .starts_with(|c: char| c.is_ascii_alphanumeric())
        });
        let flag = lower.match_indices("--trailer").any(|(start, _)| {
            lower[start + "--trailer".len()..]
                .trim_start_matches([' ', '=', '"', '\''])
                .starts_with(&key)
        });
        if !written && !flag {
            lacks.push(format!("the trailer '{key}: VALUE'"));
        }
    }
    if let Some(pattern) = &gate.issue_pattern {
        let named = command
            .split(|c: char| c.is_whitespace() || "\"'()[]{},;:.".contains(c))
            .any(|word| !word.is_empty() && glob::matches(pattern, word));
        if !named {
            lacks.push(format!("an issue matching {pattern}"));
        }
    }
    lacks
}

/** The approval classes a changed file falls into
 * Input
    - change: &Change - changed file
 * Output
    - Vec<ChangeClass>
*/
pub(crate) fn file_classes(change: &Change) -> Vec<ChangeClass> {
    let mut classes = vec![if change.deleted {
        ChangeClass::Delete
    } else if change.existed {
        ChangeClass::Update
    } else {
        ChangeClass::Create
    }];
    if !change.deleted {
        classes.push(ChangeClass::Write);
    }
    let lower = change.path.to_ascii_lowercase();
    let name = change.path.rsplit('/').next().unwrap_or(&change.path);
    let manifest = MANIFESTS.contains(&name)
        || (name.starts_with("requirements") && name.ends_with(".txt"))
        || name.ends_with(".csproj");
    if manifest {
        classes.push(ChangeClass::DependencyChange);
    } else {
        let extension = name
            .rsplit_once('.')
            .map(|(_, extension)| extension.to_ascii_lowercase())
            .unwrap_or_default();
        let config = [
            "yaml",
            "yml",
            "toml",
            "ini",
            "cfg",
            "conf",
            "properties",
            "env",
        ]
        .contains(&extension.as_str())
            || name.starts_with(".env")
            || (extension == "json" && (name.contains("config") || name.contains("settings")))
            || name.eq_ignore_ascii_case("dockerfile")
            || name.starts_with("docker-compose");
        if config {
            classes.push(ChangeClass::ConfigChange);
        }
    }
    if lower
        .split('/')
        .any(|segment| segment == "migrations" || segment == "migrate")
    {
        classes.push(ChangeClass::MigrationChange);
    }
    let lower_name = name.to_ascii_lowercase();
    if lower.split('/').any(|segment| {
        segment == "test" || segment == "tests" || segment == "__tests__" || segment == "spec"
    }) || lower_name.contains(".test.")
        || lower_name.contains(".spec.")
        || lower_name.contains("_test.")
        || lower_name.starts_with("test_")
    {
        classes.push(ChangeClass::TestChange);
    }
    classes
}

/** The approval classes a command falls into
 * Input
    - words: &[String] - lowercase command words
 * Output
    - Vec<ChangeClass>
*/
pub(crate) fn command_classes(words: &[String]) -> Vec<ChangeClass> {
    let mut classes = vec![ChangeClass::Execute];
    if has(words, &["git", "commit"]) {
        classes.push(ChangeClass::GitCommit);
    }
    if has(words, &["git", "push"]) {
        classes.push(ChangeClass::GitPush);
    }
    if INSTALLS.iter().any(|sequence| has(words, sequence)) {
        classes.push(ChangeClass::PackageInstall);
    }
    classes
}

/** Report whether a host is matched by a pattern ("api.example.com" or "*.example.com")
 * Input
    - pattern: &str - pattern
    - host: &str - host
 * Output
    - bool
*/
fn host_matches(pattern: &str, host: &str) -> bool {
    let (pattern, host) = (pattern.to_ascii_lowercase(), host.to_ascii_lowercase());
    match pattern.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => pattern == host,
    }
}

/** The host of a network destination ("https://api.example.com/x" or "api.example.com")
 * Input
    - destination: &str - destination
 * Output
    - String
*/
fn host_of(destination: &str) -> String {
    let rest = destination
        .split_once("://")
        .map_or(destination, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let authority = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    authority.split(':').next().unwrap_or(authority).to_string()
}

/** Report whether a command prints or reads the whole environment: a dump as one part of a
 * pipeline, chain, or redirection ('env | cat', 'set > out'), or code that takes all of it
 * ('dict(os.environ)', 'node -p process.env', /proc/self/environ)
 * Input
    - command: &str - shell command
 * Output
    - bool
*/
fn dumps_environment(command: &str) -> bool {
    let lower = command.to_ascii_lowercase();
    for part in lower.split(|c: char| "|;&\n<>()`".contains(c)) {
        let words = part
            .split_whitespace()
            .map(|word| word.trim_matches(|c| c == '"' || c == '\''))
            .collect::<Vec<_>>();
        let Some((first, rest)) = words.split_first() else {
            continue;
        };
        let name = first.rsplit(['/', '\\']).next().unwrap_or(first);
        let name = name.strip_suffix(".exe").unwrap_or(name);
        let dump = match name {
            // 'env A=1 tool' runs a tool, and 'printenv NAME' reads one variable
            "env" => rest
                .iter()
                .all(|word| word.starts_with('-') || word.contains('=')),
            "printenv" => rest.iter().all(|word| word.starts_with('-')),
            "set" => rest.is_empty(),
            "export" | "declare" | "typeset" => {
                rest.is_empty() || rest.iter().all(|word| matches!(*word, "-p" | "-x" | "-px"))
            }
            "compgen" => rest.contains(&"-e"),
            "get-childitem" | "gci" | "dir" | "ls" => rest
                .iter()
                .any(|word| word.trim_end_matches(['/', '\\']) == "env:"),
            _ => false,
        };
        if dump {
            return true;
        }
    }
    if [
        "/environ",
        "getenvironmentvariables",
        "system.getenv()",
        "env.to_h",
    ]
    .iter()
    .any(|text| lower.contains(text))
    {
        return true;
    }
    // The whole object, not one variable of it: process.env.X and os.environ["X"] name theirs
    [("process.env", ".["), ("os.environ", "[")]
        .iter()
        .any(|(object, one)| {
            lower.match_indices(object).any(|(start, _)| {
                let after = &lower[start + object.len()..];
                let next = after.chars().next();
                match next {
                    Some(c) if c.is_ascii_alphanumeric() || c == '_' => false,
                    Some(c) if one.contains(c) => false,
                    Some('.') => !after.starts_with(".get(") && !after.starts_with(".getenv("),
                    _ => true,
                }
            })
        })
}

/** The hidden variable a command or its arguments reveal, if any: a read of a variable matching a
 * pattern ($NAME, ${NAME}, %NAME%, $env:NAME, process.env.NAME, getenv/environ with the name), or a
 * dump of the whole environment
 * Input
    - texts: &[&str] - command and arguments
    - command: Option<&str> - the shell command
    - patterns: &[String] - hidden variable patterns
 * Output
    - Option<String>
*/
fn reveals(texts: &[&str], command: Option<&str>, patterns: &[String]) -> Option<String> {
    if patterns.is_empty() {
        return None;
    }
    if command.is_some_and(dumps_environment) {
        return Some("the whole environment".into());
    }
    for text in texts {
        let markers = ["$", "%", "getenv", "environ", "printenv", "env:", "env."];
        if !markers.iter().any(|marker| text.contains(marker)) {
            continue;
        }
        let mut name = String::new();
        for character in text.chars().chain(std::iter::once(' ')) {
            if character.is_ascii_alphanumeric() || character == '_' {
                name.push(character);
                continue;
            }
            if !name.is_empty() && patterns.iter().any(|pattern| glob::matches(pattern, &name)) {
                return Some(name);
            }
            name.clear();
        }
    }
    None
}

/** Programs that read the files named to them */
const READERS: &[&str] = &[
    "cat",
    "less",
    "more",
    "head",
    "tail",
    "type",
    "base64",
    "xxd",
    "od",
    "strings",
    "grep",
    "rg",
    "sed",
    "awk",
    "cp",
    "mv",
    "tar",
    "zip",
    "scp",
    "rsync",
    "source",
    "get-content",
    "gc",
];

/** Programs that read whole folders */
const FOLDER_READERS: &[&str] = &["grep", "rg", "tar", "zip", "cp", "rsync", "scp"];

/** Report whether command words reach a file without naming it: a pattern matching it, or a folder
 * holding it given to a program that reads whole folders
 * Input
    - words: &[String] - lowercase command words
    - file: &str - repository-relative file
 * Output
    - bool
*/
fn reaches_without_naming(words: &[String], file: &str) -> bool {
    let file = file.to_ascii_lowercase();
    let folder_reader = words
        .iter()
        .any(|word| FOLDER_READERS.contains(&word.rsplit('/').next().unwrap_or(word)));
    words.iter().any(|word| {
        let word = word.trim_start_matches("./");
        if word.contains(['*', '?']) {
            return glob::matches(word, &file);
        }
        if word == file {
            return true;
        }
        let folder = word.trim_end_matches('/');
        folder_reader && !folder.is_empty() && file.starts_with(&format!("{folder}/"))
    })
}

/** Report whether a command works out a path while it runs: command substitution, eval, or a
 * variable handed to a program that reads files. Substitutions that only read the date, the
 * current folder, or a Git revision are fine
 * Input
    - command: &str - shell command
    - words: &[String] - lowercase command words
 * Output
    - bool
*/
fn computes_a_path(command: &str, words: &[String]) -> bool {
    let lower = command.to_ascii_lowercase();
    let harmless = ["date", "pwd", "whoami", "git rev-parse", "git branch"];
    let substitution = lower.match_indices("$(").any(|(start, _)| {
        let inside = lower[start + 2..].trim_start();
        !harmless.iter().any(|safe| inside.starts_with(safe))
    });
    substitution
        || lower.contains('`')
        || words.iter().any(|word| word == "eval")
        || words.windows(2).any(|pair| {
            READERS.contains(&pair[0].rsplit('/').next().unwrap_or(&pair[0]))
                && pair[1].starts_with('$')
        })
}

/** Turn a mode into a finding
 * Input
    - mode: Mode - mode
    - reason: String - why
    - rule: &'static str - rule code
    - brick: String - Brick reference
 * Output
    - Option<Finding>
*/
fn by_mode(mode: Mode, reason: String, rule: &'static str, brick: String) -> Option<Finding> {
    match mode {
        Mode::Auto => None,
        Mode::Ask => Some(Finding {
            decision: Decision::RequireApproval,
            reason,
            rule,
            brick,
        }),
        Mode::Deny => Some(Finding {
            decision: Decision::Deny,
            reason,
            rule,
            brick,
        }),
    }
}

/** Decide a call against the Brick rules
 * Input
    - rules: &Rules - Brick rules
    - limits: &Limits - per-file limits (target membership)
    - workspace: &Workspace - repository (gates)
    - call: &Call - the call
 * Output
    - Outcome
*/
pub(crate) fn decide(
    rules: &Rules,
    limits: &Limits,
    workspace: &Workspace,
    call: &Call,
) -> Outcome {
    let mut outcome = Outcome::default();
    let spelled = call
        .command
        .map(|command| {
            let mut found = crate::hooks::shell::words(command);
            found.extend(
                unmasked(command)
                    .map(|text| crate::hooks::shell::words(&text))
                    .unwrap_or_default(),
            );
            found
        })
        .unwrap_or_default()
        .into_iter()
        .map(|word| word.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let words = normalized(&spelled);
    let of = |files: &[String]| {
        files
            .iter()
            .flat_map(|file| {
                targets_of(limits, file)
                    .into_iter()
                    .map(move |target| (target, file.clone()))
            })
            .collect::<Vec<_>>()
    };
    let reads = of(&call.reads);
    let named = of(&call.named);
    let changed = call
        .changes
        .iter()
        .flat_map(|change| {
            targets_of(limits, &change.path)
                .into_iter()
                .map(move |target| (target, change.clone()))
        })
        .collect::<Vec<_>>();
    let touched = reads
        .iter()
        .map(|(target, _)| target.clone())
        .chain(named.iter().map(|(target, _)| target.clone()))
        .chain(changed.iter().map(|(target, _)| target.clone()))
        .collect::<BTreeSet<_>>();
    let worked = call
        .usage
        .map(|usage| usage.targets.keys().cloned().collect::<BTreeSet<_>>())
        .unwrap_or_default();
    let changes = matches!(call.operation, Operation::Write | Operation::Execute);
    let command_classes = if call.operation == Operation::Execute {
        command_classes(&words)
    } else {
        Vec::new()
    };
    let mut push = |finding: Option<Finding>| outcome.findings.extend(finding);
    for target in &touched {
        let Some(controls) = rules.targets.get(target) else {
            continue;
        };
        let used = call
            .usage
            .and_then(|usage| usage.targets.get(target))
            .cloned()
            .unwrap_or_default();
        // Identity: who is calling, whatever the call does (not decided when no agent calls)
        for rule in controls.identities.iter().filter(|_| call.agent.is_some()) {
            let agent_ok = rule.agents.iter().any(|allowed| {
                call.agent
                    .is_some_and(|agent| allowed.eq_ignore_ascii_case(agent))
                    || call.agent_id.is_some_and(|id| allowed == id)
            });
            let model_ok = rule.models.is_empty()
                || call.model.is_some_and(|model| {
                    rule.models
                        .iter()
                        .any(|pattern| glob::matches(pattern, model))
                });
            if !(agent_ok && model_ok) {
                let who = format!(
                    "{}{}",
                    call.agent.unwrap_or("an unknown agent"),
                    call.model
                        .map(|model| format!(" ({model})"))
                        .unwrap_or_default()
                );
                push(by_mode(
                    rule.on_other,
                    format!(
                        "{target} admits only {}{}; {who} is not admitted",
                        rule.agents.join(", "),
                        if rule.models.is_empty() {
                            String::new()
                        } else {
                            format!(" on models {}", rule.models.join(", "))
                        }
                    ),
                    "BRICK_IDENTITY",
                    rule.source.clone(),
                ));
            }
        }
        // Access: unreadable files
        for (_, file) in reads
            .iter()
            .chain(&named)
            .filter(|(owner, _)| owner == target)
        {
            if controls
                .deny_read
                .iter()
                .any(|pattern| glob::matches(pattern, file))
            {
                push(Some(Finding {
                    decision: Decision::Deny,
                    reason: format!("{file} may not be read in {target}"),
                    rule: "BRICK_DENY_READ",
                    brick: controls.origin(
                        controls
                            .deny_read
                            .iter()
                            .filter(|pattern| glob::matches(pattern, file))
                            .map(|pattern| format!("deny_read:{pattern}")),
                    ),
                }));
            }
        }
        // Access: the mode of each change, and Approval: the kinds of change
        for (_, change) in changed.iter().filter(|(owner, _)| owner == target) {
            let classes = file_classes(change);
            if let Some(mode) = controls.mode(classes[0]) {
                push(by_mode(
                    mode,
                    format!(
                        "{} of {} in {target} is {}",
                        super::config::serial(&classes[0]),
                        change.path,
                        super::config::serial(&mode)
                    ),
                    "BRICK_ACCESS_MODE",
                    controls.origin([Controls::mode_key(classes[0], mode)]),
                ));
            }
            for rule in controls
                .approvals
                .iter()
                .filter(|rule| rule.paths.contains(&change.path))
            {
                let large = rule
                    .min_lines_changed
                    .is_some_and(|size| change.lines > u64::from(size));
                if let Some(class) = rule.classes.iter().find(|class| {
                    classes.contains(class) || (**class == ChangeClass::LargeChange && large)
                }) {
                    push(Some(Finding {
                        decision: Decision::RequireApproval,
                        reason: format!(
                            "{} of {} in {target} needs a person{}",
                            super::config::serial(class).replace('_', " "),
                            change.path,
                            if rule.reason.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", rule.reason)
                            }
                        ),
                        rule: "BRICK_APPROVAL",
                        brick: rule.source.clone(),
                    }));
                }
            }
        }
        if call.operation == Operation::Execute && named.iter().any(|(owner, _)| owner == target) {
            if let Some(mode) = controls.mode(ChangeClass::Execute) {
                push(by_mode(
                    mode,
                    format!(
                        "commands on {target}'s files are {}",
                        super::config::serial(&mode)
                    ),
                    "BRICK_ACCESS_MODE",
                    controls.origin([Controls::mode_key(ChangeClass::Execute, mode)]),
                ));
            }
            for rule in &controls.approvals {
                if let Some(class) = rule
                    .classes
                    .iter()
                    .find(|class| command_classes.contains(class))
                {
                    push(Some(Finding {
                        decision: Decision::RequireApproval,
                        reason: format!(
                            "{} in {target} needs a person{}",
                            super::config::serial(class).replace('_', " "),
                            if rule.reason.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", rule.reason)
                            }
                        ),
                        rule: "BRICK_APPROVAL",
                        brick: rule.source.clone(),
                    }));
                }
            }
        }
        // Limits: what the session has done in the target, with this call
        for cap in &controls.caps {
            let limits = &cap.limits;
            let mut files = used.files.clone();
            files.extend(
                changed
                    .iter()
                    .filter(|(owner, change)| owner == target && cap.paths.contains(&change.path))
                    .map(|(_, change)| change.path.clone()),
            );
            let counted_lines = used.lines
                + changed
                    .iter()
                    .filter(|(owner, change)| owner == target && cap.paths.contains(&change.path))
                    .map(|(_, change)| change.lines)
                    .sum::<u64>();
            let minutes = call
                .started_at
                .map(|start| call.now.saturating_sub(start) / 60_000);
            let checks = [
                ("tool calls", Some(used.calls + 1), limits.max_tool_calls),
                (
                    "minutes in the session",
                    minutes,
                    limits.max_runtime_minutes,
                ),
                (
                    "files changed",
                    Some(files.len() as u64),
                    limits.max_files_changed,
                ),
                (
                    "lines changed",
                    Some(counted_lines),
                    limits.max_lines_changed,
                ),
                (
                    "commands",
                    Some(used.commands + u64::from(call.operation == Operation::Execute)),
                    limits.max_commands,
                ),
                (
                    "network calls",
                    Some(used.network + u64::from(!call.network.is_empty())),
                    limits.max_network_calls,
                ),
            ];
            let exhausted = checks.iter().find(
                |(_, value, cap)| matches!((value, cap), (Some(value), Some(cap)) if value > cap),
            );
            let denied_out = limits.max_denials.is_some_and(|cap| used.denials >= cap);
            // The same counts across every session today: a new session does not reset them
            let today = call
                .daily
                .and_then(|daily| daily.targets.get(target))
                .cloned()
                .unwrap_or_else(TargetUsage::default);
            let mut files_today = today.files.clone();
            files_today.extend(
                changed
                    .iter()
                    .filter(|(owner, _)| owner == target)
                    .map(|(_, change)| change.path.clone()),
            );
            let lines_today = today.lines
                + changed
                    .iter()
                    .filter(|(owner, _)| owner == target)
                    .map(|(_, change)| change.lines)
                    .sum::<u64>();
            let daily_exhausted = [
                ("tool calls", today.calls + 1, limits.max_tool_calls_per_day),
                (
                    "files changed",
                    files_today.len() as u64,
                    limits.max_files_changed_per_day,
                ),
                (
                    "lines changed",
                    lines_today,
                    limits.max_lines_changed_per_day,
                ),
                (
                    "commands",
                    today.commands + u64::from(call.operation == Operation::Execute),
                    limits.max_commands_per_day,
                ),
                (
                    "network calls",
                    today.network + u64::from(!call.network.is_empty()),
                    limits.max_network_calls_per_day,
                ),
            ]
            .into_iter()
            .find_map(|(what, value, cap)| {
                cap.filter(|cap| value > *cap).map(|cap| (what, value, cap))
            });
            if let Some((what, Some(value), Some(cap_value))) = exhausted {
                push(by_mode(
                    if limits.on_exhausted == Escalation::Escalate {
                        Mode::Ask
                    } else {
                        Mode::Deny
                    },
                    format!(
                        "{target} allows {cap_value} {what} per session; this call makes {value}"
                    ),
                    "BRICK_LIMIT",
                    cap.source.clone(),
                ));
            } else if denied_out {
                push(by_mode(
                    if limits.on_exhausted == Escalation::Escalate {
                        Mode::Ask
                    } else {
                        Mode::Deny
                    },
                    format!(
                        "{target} stops a session after {} denied calls",
                        used.denials
                    ),
                    "BRICK_LIMIT",
                    cap.source.clone(),
                ));
            } else if let Some((what, value, cap_value)) = daily_exhausted {
                push(by_mode(
                    if limits.on_exhausted == Escalation::Escalate {
                        Mode::Ask
                    } else {
                        Mode::Deny
                    },
                    format!(
                        "{target} allows {cap_value} {what} a day across sessions; this call makes {value}"
                    ),
                    "BRICK_LIMIT",
                    cap.source.clone(),
                ));
            }
        }
        if changes {
            // Precondition gates that a changed file would not reveal
            for gate in &controls.gates {
                if super::runtime::gate_holds(workspace, &gate.check, call.now) == Some(false) {
                    push(by_mode(
                        gate.on_fail,
                        format!(
                            "precondition failing in {target}: {}",
                            super::compose::check_text(&gate.check)
                        ),
                        "BRICK_PRECONDITION",
                        gate.source.clone(),
                    ));
                }
            }
            // Environment: variables the work needs
            for name in &controls.require_env {
                if std::env::var_os(name).is_none_or(|value| value.is_empty()) {
                    push(Some(Finding {
                        decision: Decision::RequireApproval,
                        reason: format!(
                            "{target} needs the environment variable {name}, which is not set"
                        ),
                        rule: "BRICK_ENV_MISSING",
                        brick: controls.origin([format!("require_env:{name}")]),
                    }));
                }
            }
            // Recovery: failures in a row
            if controls
                .max_consecutive_failures
                .is_some_and(|limit| used.failures_in_row >= limit)
            {
                push(Some(Finding {
                    decision: Decision::RequireApproval,
                    reason: format!(
                        "{} tool calls failed in a row in {target}; a person decides how to go on",
                        used.failures_in_row
                    ),
                    rule: "BRICK_RECOVERY_PAUSE",
                    brick: controls.origin(
                        controls
                            .max_consecutive_failures
                            .map(|limit| format!("recovery:{limit}")),
                    ),
                }));
            }
            // Watch: a firing signal pauses the target
            if rules.paused.contains(target) {
                push(Some(Finding {
                    decision: Decision::RequireApproval,
                    reason: format!(
                        "a watch on {target} is firing; changes need a person until it clears"
                    ),
                    rule: "BRICK_WATCH_PAUSE",
                    brick: controls.watch_pause.join(", "),
                }));
            }
        }
    }
    // Rules of the targets the session works in: network reach and hidden variables
    let texts = call
        .command
        .into_iter()
        .chain(call.arguments.iter().map(String::as_str))
        .collect::<Vec<_>>();
    for target in touched.iter().chain(worked.iter()).collect::<BTreeSet<_>>() {
        let Some(controls) = rules.targets.get(target) else {
            continue;
        };
        if !call.network.is_empty() {
            if let Some(mode) = controls.mode(ChangeClass::Network) {
                push(by_mode(
                    mode,
                    format!(
                        "network calls while working in {target} are {}",
                        super::config::serial(&mode)
                    ),
                    "BRICK_ACCESS_MODE",
                    controls.origin([Controls::mode_key(ChangeClass::Network, mode)]),
                ));
            }
            for destination in call.network {
                let host = host_of(destination);
                if controls
                    .network_deny
                    .iter()
                    .any(|pattern| host_matches(pattern, &host))
                {
                    push(Some(Finding {
                        decision: Decision::Deny,
                        reason: format!("{host} may not be reached while working in {target}"),
                        rule: "BRICK_NETWORK",
                        brick: controls.origin(
                            controls
                                .network_deny
                                .iter()
                                .filter(|pattern| host_matches(pattern, &host))
                                .map(|pattern| format!("network_deny:{pattern}")),
                        ),
                    }));
                } else if controls
                    .network_allow
                    .iter()
                    .any(|allowed| !allowed.iter().any(|pattern| host_matches(pattern, &host)))
                {
                    push(Some(Finding {
                        decision: Decision::Deny,
                        reason: format!("{host} is not an allowed host while working in {target}"),
                        rule: "BRICK_NETWORK",
                        brick: controls.origin(
                            controls
                                .network_allow
                                .iter()
                                .enumerate()
                                .filter(|(_, allowed)| {
                                    !allowed.iter().any(|pattern| host_matches(pattern, &host))
                                })
                                .map(|(index, _)| format!("network_allow:{index}")),
                        ),
                    }));
                }
            }
        }
        if let Some(name) = reveals(&texts, call.command, &controls.hide_env) {
            push(Some(Finding {
                decision: Decision::Deny,
                reason: format!(
                    "the call would reveal {name}, which agents working in {target} may not read"
                ),
                rule: "BRICK_HIDDEN_ENV",
                brick: controls.origin(
                    controls
                        .hide_env
                        .iter()
                        .filter(|pattern| {
                            reveals(&texts, call.command, std::slice::from_ref(pattern)).is_some()
                        })
                        .map(|pattern| format!("hide_env:{pattern}")),
                ),
            }));
        }
    }
    // Repository-wide commands: commit, push, installs, and deployments name no file
    if call.operation == Operation::Execute {
        // Provenance: a commit message carrying what the target's records must show
        if has(&words, &["git", "commit"]) {
            let command = call.command.unwrap_or_default();
            let text = unmasked(command).unwrap_or_else(|| command.to_string());
            for (target, controls) in &rules.targets {
                for gate in &controls.commit_gates {
                    let lacks = commit_lacks(&text, gate);
                    if !lacks.is_empty() {
                        push(by_mode(
                            gate.mode,
                            format!(
                                "a commit needs {} in its message ({target}); write it with -m so it can be read",
                                lacks.join(" and ")
                            ),
                            "BRICK_PROVENANCE",
                            gate.source.clone(),
                        ));
                    }
                }
            }
        }
        // Unreadable files reached without being named: by a pattern, a folder, or a computed path
        for (target, controls) in rules
            .targets
            .iter()
            .filter(|(_, controls)| !controls.deny_read.is_empty())
        {
            let denied = limits
                .iter()
                .filter(|(file, limit)| {
                    limit.entities.contains(target)
                        && controls
                            .deny_read
                            .iter()
                            .any(|pattern| glob::matches(pattern, file))
                })
                .map(|(file, _)| file.as_str())
                .collect::<Vec<_>>();
            if let Some(file) = denied
                .iter()
                .find(|file| reaches_without_naming(&spelled, file))
            {
                push(Some(Finding {
                    decision: Decision::Deny,
                    reason: format!(
                        "the command reaches {file}, which may not be read in {target}"
                    ),
                    rule: "BRICK_DENY_READ",
                    brick: controls.origin(
                        controls
                            .deny_read
                            .iter()
                            .filter(|pattern| glob::matches(pattern, file))
                            .map(|pattern| format!("deny_read:{pattern}")),
                    ),
                }));
            } else if computes_a_path(call.command.unwrap_or_default(), &spelled) {
                push(Some(Finding {
                    decision: Decision::RequireApproval,
                    reason: format!(
                        "the command computes what it reads, so Crane cannot tell whether it reaches files of {target} agents may not read"
                    ),
                    rule: "BRICK_DENY_READ",
                    brick: controls.origin(
                        controls
                            .deny_read
                            .iter()
                            .map(|pattern| format!("deny_read:{pattern}")),
                    ),
                }));
            }
        }
        for (target, controls) in &rules.targets {
            if touched.contains(target) {
                continue;
            }
            for rule in &controls.approvals {
                if let Some(class) = rule.classes.iter().find(|class| {
                    matches!(
                        class,
                        ChangeClass::GitCommit | ChangeClass::GitPush | ChangeClass::PackageInstall
                    ) && command_classes.contains(class)
                }) {
                    push(Some(Finding {
                        decision: Decision::RequireApproval,
                        reason: format!(
                            "{} needs a person ({target}){}",
                            super::config::serial(class).replace('_', " "),
                            if rule.reason.is_empty() {
                                String::new()
                            } else {
                                format!(": {}", rule.reason)
                            }
                        ),
                        rule: "BRICK_APPROVAL",
                        brick: rule.source.clone(),
                    }));
                }
            }
        }
        for (target, controls) in &rules.targets {
            for (adapter, operation) in &controls.direct {
                let matched = DIRECT.iter().any(|(known_adapter, known, sequences)| {
                    known_adapter == adapter
                        && known == operation
                        && sequences.iter().any(|sequence| has(&words, sequence))
                });
                if matched {
                    push(Some(Finding { decision: Decision::Deny, reason: format!("{operation} is done through Crane for {target}, not by agents directly"), rule: "BRICK_ACTION_DIRECT", brick: controls.origin([format!("direct:{adapter}:{operation}")]) }));
                }
            }
        }
    }
    outcome.counted = Counted {
        targets: touched
            .iter()
            .filter(|target| rules.targets.contains_key(*target))
            .cloned()
            .collect(),
        command: call.operation == Operation::Execute,
        network: !call.network.is_empty(),
        denied: false,
        changes: changed
            .iter()
            .filter(|(target, _)| rules.targets.contains_key(target))
            .map(|(target, change)| (target.clone(), change.path.clone(), change.lines))
            .collect(),
    };
    outcome
}

/** Lines that differ between two texts (as multisets of lines)
 * Input
    - before: Option<&str> - current text
    - after: Option<&str> - proposed text
 * Output
    - u64
*/
pub(crate) fn lines_changed(before: Option<&str>, after: Option<&str>) -> u64 {
    let mut counts: BTreeMap<&str, i64> = BTreeMap::new();
    for line in before.unwrap_or_default().lines() {
        *counts.entry(line).or_default() += 1;
    }
    for line in after.unwrap_or_default().lines() {
        *counts.entry(line).or_default() -= 1;
    }
    counts.values().map(|count| count.unsigned_abs()).sum()
}
