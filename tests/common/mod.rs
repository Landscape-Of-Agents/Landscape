// Shared fixture of the integration tests: a temporary bare repository acting as the GitHub
// remote, a clone with sample sources in several languages, and an isolated Crane trust
// directory. Agent environment markers are removed so tests run as a human unless a test sets one.

#![allow(dead_code)]

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::Value;

/** Environment variables that mark an agent environment */
pub const AGENT_MARKERS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CODEX_SANDBOX",
    "CODEX_SANDBOX_NETWORK_DISABLED",
    "GEMINI_SESSION_ID",
    "CRANE_AGENT",
];

/** The sample Python payment module */
pub const PAYMENTS: &str = "def charge(amount):\n    fee = amount * 0.03\n    total = amount + fee\n    return round(total, 2)\n\n\ndef refund(amount):\n    return -amount\n";

/** A temporary repository with Crane's trust directory
 * Fields
    - root: tempfile::TempDir - holds everything
    - work: PathBuf - the clone
    - remote: PathBuf - the bare remote
    - home: PathBuf - CRANE_HOME
    - ingest_url: String - CRANE_INGEST_URL of every command ("off" unless a test sets a service)
*/
pub struct Fixture {
    pub root: tempfile::TempDir,
    pub work: PathBuf,
    pub remote: PathBuf,
    pub home: PathBuf,
    pub ingest_url: String,
}

/** Run git in a directory and assert success
 * Input
    - directory: &Path - working directory
    - args: &[&str] - git arguments
 * Output
    - String trimmed stdout
*/
pub fn git(directory: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(directory)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/** Combine stdout and stderr of an output
 * Input
    - output: &Output - process output
 * Output
    - String
*/
pub fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

impl Fixture {
    /** Create a remote with an initial commit and a clone of it
     * Input
        - None
     * Output
        - Fixture
    */
    pub fn new() -> Self {
        let root = tempfile::tempdir().expect("temporary directory");
        let remote = root.path().join("remote.git");
        let seed = root.path().join("seed");
        let work = root.path().join("work");
        let home = root.path().join("trust");
        fs::create_dir_all(&seed).unwrap();
        git(
            root.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                remote.to_str().unwrap(),
            ],
        );
        git(&seed, &["init", "-q", "-b", "main"]);
        git(&seed, &["config", "user.email", "dev@example.com"]);
        git(&seed, &["config", "user.name", "Dev"]);
        git(&seed, &["config", "core.autocrlf", "false"]);
        let files: &[(&str, &str)] = &[
            ("app/payments.py", PAYMENTS),
            ("app/ledger.py", "def post(entry):\n    return entry\n"),
            ("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n\npub fn sub(a: i32, b: i32) -> i32 {\n    a - b\n}\n"),
            ("web/app.js", "export function greet(name) {\n  return `hi ${name}`;\n}\n"),
            ("docs/guide.md", "# Guide\n\nUse the payment API carefully.\n"),
            ("data/config.json", "{\"a\": 1}\n"),
        ];
        for (path, content) in files {
            let full = seed.join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "initial"]);
        git(
            &seed,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        git(&seed, &["push", "-q", "origin", "main"]);
        git(
            root.path(),
            &[
                "-c",
                "core.autocrlf=false",
                "clone",
                "-q",
                remote.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        git(&work, &["config", "user.email", "dev@example.com"]);
        git(&work, &["config", "user.name", "Dev"]);
        git(&work, &["config", "core.autocrlf", "false"]);
        fs::create_dir_all(&home).unwrap();
        Self {
            root,
            work,
            remote,
            home,
            ingest_url: "off".into(),
        }
    }

    /** Create a fixture that is connected, initialized, and has a baseline checkpoint
     * Input
        - None
     * Output
        - Fixture
    */
    pub fn ready() -> Self {
        let fixture = Self::new();
        fixture.ok(&["repo", "--https", fixture.remote_url().as_str()]);
        fixture.ok(&["init"]);
        fixture.ok(&["checkpoint", "baseline"]);
        fixture
    }

    /** Return the remote as a URL string
     * Input
        - None (uses self)
     * Output
        - String
    */
    pub fn remote_url(&self) -> String {
        self.remote.to_string_lossy().into_owned()
    }

    /** Build a crane command in the clone, as a human, with the isolated trust directory
     * Input
        - args: &[&str] - crane arguments
     * Output
        - Command
    */
    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_crane"));
        command
            .current_dir(&self.work)
            .args(args)
            .env("CRANE_HOME", &self.home)
            .env("CRANE_ALLOW_LOCAL_REMOTE", "1")
            .env("CRANE_ALERTS_DRY_RUN", "1")
            .env("CRANE_INGEST_URL", &self.ingest_url)
            .env_remove("CRANE_TRUSTED_PUBLIC_KEYS")
            .env_remove("CRANE_SESSION_ID")
            .env_remove("CRANE_TASK_ID")
            .env_remove("CRANE_TASK_NAME")
            .env_remove("CRANE_TEST_SELECTION_IDS");
        for marker in AGENT_MARKERS {
            command.env_remove(marker);
        }
        let binary = Path::new(env!("CARGO_BIN_EXE_crane"))
            .parent()
            .unwrap()
            .to_path_buf();
        let path = std::env::var_os("PATH").unwrap_or_default();
        let mut paths = vec![binary];
        paths.extend(std::env::split_paths(&path));
        command.env("PATH", std::env::join_paths(paths).unwrap());
        command.stdin(Stdio::null());
        command
    }

    /** Run crane and return its output
     * Input
        - args: &[&str] - crane arguments
     * Output
        - Output
    */
    pub fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().expect("crane runs")
    }

    /** Run crane, assert success, and return combined output
     * Input
        - args: &[&str] - crane arguments
     * Output
        - String
    */
    pub fn ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "crane {args:?} failed:\n{}",
            text(&output)
        );
        text(&output)
    }

    /** Run crane, assert failure, and return combined output
     * Input
        - args: &[&str] - crane arguments
     * Output
        - String
    */
    pub fn fails(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            !output.status.success(),
            "crane {args:?} should fail:\n{}",
            text(&output)
        );
        text(&output)
    }

    /** Run crane as an AI agent (CLAUDECODE=1)
     * Input
        - args: &[&str] - crane arguments
     * Output
        - Output
    */
    pub fn as_agent(&self, args: &[&str]) -> Output {
        self.command(args)
            .env("CLAUDECODE", "1")
            .output()
            .expect("crane runs")
    }

    /** Run crane with stdin input
     * Input
        - args: &[&str] - crane arguments
        - input: &str - stdin content
     * Output
        - Output
    */
    pub fn with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("crane runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().expect("crane finishes")
    }

    /** Run a hook as the agent host would
     * Input
        - profile: &str - claude, codex, or generic
        - event: &str - event name
        - payload: &Value - hook JSON
     * Output
        - Output
    */
    pub fn hook(&self, profile: &str, event: &str, payload: &Value) -> Output {
        let mut child = self
            .command(&["agent", "hook", "--event", event, "--profile", profile])
            .env("CLAUDECODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("crane runs");
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        child.wait_with_output().expect("hook finishes")
    }

    /** Write a file in the clone
     * Input
        - path: &str - relative path
        - content: &str - content
     * Output
        - None
    */
    pub fn write(&self, path: &str, content: &str) {
        let full = self.work.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }

    /** Read a file in the clone
     * Input
        - path: &str - relative path
     * Output
        - String
    */
    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.work.join(path)).unwrap_or_else(|_| panic!("{path} exists"))
    }

    /** Commit everything in the clone
     * Input
        - message: &str - commit message
     * Output
        - None
    */
    pub fn commit(&self, message: &str) {
        git(&self.work, &["add", "-A"]);
        git(&self.work, &["commit", "-q", "-m", message]);
    }

    /** Protect lines of a file and return the generated selection identifier
     * Input
        - args: &[&str] - arguments after "protect"
     * Output
        - String identifier
    */
    pub fn protect(&self, args: &[&str]) -> String {
        let mut full = vec!["protect"];
        full.extend_from_slice(args);
        selection_id(&self.ok(&full))
    }

    /** Target lines of a file and return the generated selection identifier
     * Input
        - args: &[&str] - arguments after "target"
     * Output
        - String identifier
    */
    pub fn target(&self, args: &[&str]) -> String {
        let mut full = vec!["target"];
        full.extend_from_slice(args);
        selection_id(&self.ok(&full))
    }

    /** Read a JSON file of .crane
     * Input
        - name: &str - file name
     * Output
        - Value
    */
    pub fn crane_json(&self, name: &str) -> Value {
        serde_json::from_str(&self.read(&format!(".crane/{name}"))).unwrap()
    }

    /** Run a command with --json and parse its output
     * Input
        - args: &[&str] - crane arguments including --json
     * Output
        - (bool, Value) success and parsed stdout
    */
    pub fn json(&self, args: &[&str]) -> (bool, Value) {
        let output = self.run(args);
        let value = serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
            panic!(
                "crane {args:?} printed no JSON ({error}):\n{}",
                text(&output)
            )
        });
        (output.status.success(), value)
    }
}

/** Extract the selection identifier from protect or target output
 * Input
    - output: &str - command output
 * Output
    - String identifier
*/
pub fn selection_id(output: &str) -> String {
    let start = output
        .find("as selection ")
        .expect("output names the selection")
        + "as selection ".len();
    output[start..start + 8].to_string()
}
