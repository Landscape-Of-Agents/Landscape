// Shell command analysis used by the guard and the telemetry: splitting a command into words,
// recognizing Crane invocations that change governance (everything but a read-only allow-list),
// the shell words and Git subcommands that change files, network tools, references to the .crane
// directory, and network destinations named in commands or arguments. Analysis is lexical and
// conservative: it reports what a command names, never what it would do once run.

use std::collections::BTreeSet;

/** Crane subcommands an agent may run because they only read */
pub(super) const READ_ONLY_COMMANDS: &[&[&str]] = &[
    &["--version"],
    &["-V"],
    &["help"],
    &["--help"],
    &["parse"],
    &["validate"],
    &["test"],
    &["task"],
    &["session"],
    &["repo", "status"],
    &["repo", "--view"],
    &["agent", "hooks"],
    &["admin", "list"],
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
pub(super) fn words(command: &str) -> Vec<String> {
    command
        .split(|character: char| character.is_whitespace() || "&|;()`\"'<>{}".contains(character))
        .filter(|word| !word.is_empty())
        .map(normal)
        .collect()
}

/** Find Crane invocations in a shell command that are not read-only: every Crane command that
 * changes governance, trust, connections, integrations, hooks, or starts the dashboard
 * Input
    - command: &str - command text
 * Output
    - Option<String> the offending invocation, None when every invocation only reads
*/
pub(crate) fn mutating_crane(command: &str) -> Option<String> {
    let tokens = words(command);
    for (index, token) in tokens.iter().enumerate() {
        let program = token.rsplit('/').next().unwrap_or_default();
        if program != "crane" && program != "crane.exe" {
            continue;
        }
        let rest = &tokens[index + 1..];
        // A Crane invocation inside a path (".../crane/src") is followed by nothing meaningful
        if rest.is_empty() {
            continue;
        }
        let read_only = READ_ONLY_COMMANDS.iter().any(|pattern| {
            pattern
                .iter()
                .enumerate()
                .all(|(offset, word)| rest.get(offset).map(String::as_str) == Some(*word))
        }) || (rest.first().map(String::as_str) == Some("policy")
            && rest.get(2).map(String::as_str) == Some("status"))
            || (rest.first().map(String::as_str) == Some("policy-context")
                && rest.iter().any(|word| word == "--view"));
        if !read_only {
            return Some(format!(
                "crane {}",
                rest.iter().take(3).cloned().collect::<Vec<_>>().join(" ")
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
        assert!(mutating_crane("crane protect a.py").is_some());
        assert!(mutating_crane("cd x && ./target/debug/crane.exe checkpoint v2").is_some());
        assert!(mutating_crane("crane --set default policy x").is_some());
        assert!(mutating_crane("crane agent uninstall --profile claude").is_some());
        assert!(mutating_crane("crane test . && crane validate --json").is_none());
        assert!(mutating_crane("crane policy payments status").is_none());
        assert!(mutating_crane("crane policy-context payments --view").is_none());
        assert!(mutating_crane("cargo build -p crane").is_none());
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
