// The AgentScript policy language. Policies live in .crane/policies/*.crane; a file holds any
// number of blocks, every command sits inside a block, and every command ends with ';':
//
//     policy payments {
//         preserve K7M2P9RX;
//         target ABCD2345 change_type logical_bn;
//     }
//
// The keyword 'policy' is lowercase, a policy name may use letters of either case, and comments
// start with '#' or '//'. Commands reference selections only by their 8-character identifier; the
// signed registry decides what the identifier selects. A policy passes only if every command in it
// passes. init creates the policy named default in default.crane.

use std::fs;
use std::path::Path;

use super::changes::ChangeType;
use super::workspace::Workspace;
use crate::platform::git;
use crate::selection::registry::Operation;
use crate::trust::crypto::is_selection_id;

/** File name of the default policy file */
pub(crate) const DEFAULT_FILE: &str = "default";

/** File extension of policy files */
pub(crate) const EXTENSION: &str = "crane";

/** One command inside a policy
 * Fields
    - operation: Operation - preserve or target
    - id: String - selection identifier
    - change_type: Option<ChangeType> - required change kind (target only)
    - line: usize - 1-based line of the command keyword
    - start: usize - byte offset of the command keyword
    - end: usize - byte offset just past the terminating ';'
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Statement {
    pub(crate) operation: Operation,
    pub(crate) id: String,
    pub(crate) change_type: Option<ChangeType>,
    pub(crate) line: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl Statement {
    /** Render the command in policy syntax, with its ';'
     * Input
        - None (uses self)
     * Output
        - String such as "target ABCD2345 change_type logical_bn;"
    */
    pub(crate) fn render(&self) -> String {
        render_statement(self.operation, &self.id, self.change_type)
    }
}

/** Render a command in policy syntax
 * Input
    - operation: Operation - preserve or target
    - id: &str - selection identifier
    - change_type: Option<ChangeType> - required change kind
 * Output
    - String ending with ';'
*/
pub(crate) fn render_statement(
    operation: Operation,
    id: &str,
    change_type: Option<ChangeType>,
) -> String {
    match change_type {
        Some(change_type) => format!(
            "{} {id} change_type {};",
            operation.name(),
            change_type.name()
        ),
        None => format!("{} {id};", operation.name()),
    }
}

/** One policy block
 * Fields
    - name: String - policy name
    - line: usize - 1-based line of the 'policy' keyword
    - statements: Vec<Statement> - commands in source order
    - close: usize - byte offset of the closing '}'
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Block {
    pub(crate) name: String,
    pub(crate) line: usize,
    pub(crate) statements: Vec<Statement>,
    pub(crate) close: usize,
}

/** A parsed policy file
 * Fields
    - path: String - repository-relative path
    - text: String - file content
    - blocks: Vec<Block> - policies in source order
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PolicyFile {
    pub(crate) path: String,
    pub(crate) text: String,
    pub(crate) blocks: Vec<Block>,
}

/** A syntax error with its position
 * Fields
    - line: usize - 1-based line
    - column: usize - 1-based column
    - message: String - what is wrong
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) message: String,
}

impl std::fmt::Display for SyntaxError {
    /** Format the error as "line L, column C: message"
     * Input
        - formatter: &mut Formatter - output
     * Output
        - std::fmt::Result
    */
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "line {}, column {}: {}",
            self.line, self.column, self.message
        )
    }
}

/** One token of the policy language
 * Fields
    - text: &'a str - token text ("{", "}", ";", or a word)
    - start: usize - byte offset
    - line: usize - 1-based line
    - column: usize - 1-based column
*/
#[derive(Debug, Clone, Copy)]
struct Token<'a> {
    text: &'a str,
    start: usize,
    line: usize,
    column: usize,
}

/** Split policy text into tokens, skipping whitespace and '#' or '//' comments; a word is a run
 * of letters, digits, '_' and '-'; any other character is a syntax error
 * Input
    - text: &str - file content
 * Output
    - Result<Vec<Token>, SyntaxError>
*/
fn tokenize(text: &str) -> Result<Vec<Token<'_>>, SyntaxError> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let (mut index, mut line, mut line_start) = (0, 1, 0);
    while index < bytes.len() {
        let byte = bytes[index];
        let column = index - line_start + 1;
        if byte == b'\n' {
            line += 1;
            line_start = index + 1;
            index += 1;
        } else if byte.is_ascii_whitespace() {
            index += 1;
        } else if byte == b'#' || (byte == b'/' && bytes.get(index + 1) == Some(&b'/')) {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if matches!(byte, b'{' | b'}' | b';') {
            tokens.push(Token {
                text: &text[index..index + 1],
                start: index,
                line,
                column,
            });
            index += 1;
        } else if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric()
                    || bytes[index] == b'_'
                    || bytes[index] == b'-')
            {
                index += 1;
            }
            tokens.push(Token {
                text: &text[start..index],
                start,
                line,
                column,
            });
        } else {
            let character = text[index..].chars().next().unwrap_or('?');
            return Err(SyntaxError {
                line,
                column,
                message: format!("unexpected character '{character}'"),
            });
        }
    }
    Ok(tokens)
}

/** Build a syntax error at a token, or at the end of the file when there is none
 * Input
    - token: Option<&Token> - offending token
    - text: &str - file content, for the end position
    - message: String - what is wrong
 * Output
    - SyntaxError
*/
fn error_at(token: Option<&Token>, text: &str, message: String) -> SyntaxError {
    match token {
        Some(token) => SyntaxError {
            line: token.line,
            column: token.column,
            message,
        },
        None => SyntaxError {
            line: text.lines().count().max(1),
            column: text.lines().last().map_or(1, |line| line.len() + 1),
            message,
        },
    }
}

/** Check a policy name: a letter followed by letters, digits, '_' or '-', at most 64 characters
 * Input
    - name: &str - candidate
 * Output
    - bool
*/
pub(crate) fn valid_policy_name(name: &str) -> bool {
    crate::platform::validate_name("policy", name).is_ok()
}

/** Parse a policy file
 * Input
    - path: &str - repository-relative path, kept in the result
    - text: &str - file content
 * Output
    - Result<PolicyFile, SyntaxError>
    - Error at the first problem: content outside a policy block, a misspelled or uppercase
      'policy' keyword, an invalid name, a missing '{', an unknown command, an invalid selection
      identifier, an option a command does not take, a missing ';', or an unclosed block
*/
pub(crate) fn parse(path: &str, text: &str) -> Result<PolicyFile, SyntaxError> {
    // A small hand-written parser over tokens: a file is a sequence of 'policy NAME { COMMAND; ...
    // }' blocks. Every error points at the line and column of the offending token, with a hint for
    // common mistakes
    let tokens = tokenize(text)?;
    let mut blocks = Vec::new();
    let mut index = 0;
    // One policy block per iteration: the keyword, a valid name and an opening brace
    while index < tokens.len() {
        let keyword = &tokens[index];
        if keyword.text != "policy" {
            let message = if keyword.text.eq_ignore_ascii_case("policy") {
                "the keyword 'policy' must be written in lowercase".to_string()
            } else if matches!(keyword.text, "preserve" | "target") {
                "commands must be written inside a policy block: policy NAME { ... }".to_string()
            } else {
                format!("expected 'policy', found '{}'", keyword.text)
            };
            return Err(error_at(Some(keyword), text, message));
        }
        let name = tokens
            .get(index + 1)
            .filter(|token| !matches!(token.text, "{" | "}" | ";"))
            .ok_or_else(|| {
                error_at(
                    tokens.get(index + 1),
                    text,
                    "expected a policy name after 'policy'".into(),
                )
            })?;
        if !valid_policy_name(name.text) {
            return Err(error_at(
                Some(name),
                text,
                format!("invalid policy name '{}'; use letters, digits, '_' or '-', starting with a letter", name.text),
            ));
        }
        let open = tokens.get(index + 2);
        if open.map(|token| token.text) != Some("{") {
            return Err(error_at(
                open,
                text,
                format!("expected '{{' after 'policy {}'", name.text),
            ));
        }
        index += 3;
        // Commands until the closing brace: 'preserve ID;' or 'target ID [change_type TYPE];'
        let mut statements = Vec::new();
        let close = loop {
            let Some(token) = tokens.get(index) else {
                return Err(error_at(
                    None,
                    text,
                    format!("policy '{}' is not closed with '}}'", name.text),
                ));
            };
            if token.text == "}" {
                break token.start;
            }
            let operation = match token.text {
                "preserve" => Operation::Preserve,
                "target" => Operation::Target,
                "policy" => {
                    return Err(error_at(
                        Some(token),
                        text,
                        format!(
                            "policy '{}' is not closed with '}}' before the next policy",
                            name.text
                        ),
                    ))
                }
                other => {
                    return Err(error_at(
                        Some(token),
                        text,
                        format!("unknown command '{other}'; expected 'preserve' or 'target'"),
                    ))
                }
            };
            let id = tokens.get(index + 1).ok_or_else(|| {
                error_at(
                    None,
                    text,
                    format!("'{}' needs a selection marker", token.text),
                )
            })?;
            if !is_selection_id(id.text) {
                return Err(error_at(
                    Some(id),
                    text,
                    format!(
                        "invalid selection marker '{}'; expected 8 characters A-Z or 0-9 (generated by crane protect or crane target)",
                        id.text
                    ),
                ));
            }
            index += 2;
            // Options until the semicolon; only target commands take a change_type, and only once
            let mut change_type = None;
            loop {
                let Some(next) = tokens.get(index) else {
                    return Err(error_at(None, text, "command must end with ';'".into()));
                };
                match next.text {
                    ";" => break,
                    "change_type" if operation == Operation::Target && change_type.is_none() => {
                        let value = tokens.get(index + 1).ok_or_else(|| {
                            error_at(None, text, "change_type needs a value".into())
                        })?;
                        change_type = Some(
                            ChangeType::parse(value.text)
                                .map_err(|message| error_at(Some(value), text, message))?,
                        );
                        index += 2;
                    }
                    "change_type" if operation == Operation::Preserve => {
                        return Err(error_at(
                            Some(next),
                            text,
                            "change_type applies only to target commands".into(),
                        ))
                    }
                    "}" | "preserve" | "target" | "policy" => {
                        return Err(error_at(
                            Some(next),
                            text,
                            "command must end with ';'".into(),
                        ))
                    }
                    other => {
                        return Err(error_at(
                            Some(next),
                            text,
                            format!("unexpected '{other}' in a {} command", operation.name()),
                        ))
                    }
                }
            }
            let semicolon = &tokens[index];
            statements.push(Statement {
                operation,
                id: id.text.to_string(),
                change_type,
                line: token.line,
                start: token.start,
                end: semicolon.start + 1,
            });
            index += 1;
        };
        blocks.push(Block {
            name: name.text.to_string(),
            line: keyword.line,
            statements,
            close,
        });
        index += 1;
    }
    Ok(PolicyFile {
        path: path.to_string(),
        text: text.to_string(),
        blocks,
    })
}

/** Insert a command at the end of a policy block, indented on its own line before the '}'
 * Input
    - text: &str - file content
    - block: &Block - target block (from parsing text)
    - statement: &str - rendered command with its ';'
 * Output
    - String new file content
*/
pub(crate) fn insert_statement(text: &str, block: &Block, statement: &str) -> String {
    let line_start = text[..block.close].rfind('\n').map_or(0, |index| index + 1);
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    if text[line_start..block.close].trim().is_empty() {
        format!(
            "{}    {statement}{newline}{}",
            &text[..line_start],
            &text[line_start..]
        )
    } else {
        format!(
            "{}{newline}    {statement}{newline}{}",
            &text[..block.close],
            &text[block.close..]
        )
    }
}

/** Remove a command, together with its line when nothing else is on it
 * Input
    - text: &str - file content
    - statement: &Statement - command (from parsing text)
 * Output
    - String new file content
*/
pub(crate) fn remove_statement(text: &str, statement: &Statement) -> String {
    let line_start = text[..statement.start]
        .rfind('\n')
        .map_or(0, |index| index + 1);
    let line_end = text[statement.end..]
        .find('\n')
        .map_or(text.len(), |index| statement.end + index + 1);
    let before = &text[line_start..statement.start];
    let after = text[statement.end..line_end].trim();
    if before.trim().is_empty()
        && (after.is_empty() || after.starts_with('#') || after.starts_with("//"))
    {
        format!("{}{}", &text[..line_start], &text[line_end..])
    } else {
        format!("{}{}", &text[..statement.start], &text[statement.end..])
    }
}

/** Every policy file of a repository, parsed
 * Fields
    - files: Vec<PolicyFile> - files that parsed
    - errors: Vec<(String, SyntaxError)> - files that did not, with their errors
*/
#[derive(Debug, Clone, Default)]
pub(crate) struct PolicySet {
    pub(crate) files: Vec<PolicyFile>,
    pub(crate) errors: Vec<(String, SyntaxError)>,
}

impl PolicySet {
    /** Load and parse every .crane file in .crane/policies, in path order
     * Input
        - workspace: &Workspace - repository
     * Output
        - Result<PolicySet, String>
        - Error if the directory cannot be read
    */
    pub(crate) fn load(workspace: &Workspace) -> Result<Self, String> {
        let directory = workspace.policies_dir();
        let mut set = Self::default();
        let Ok(entries) = fs::read_dir(&directory) else {
            return Ok(set);
        };
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == EXTENSION)
            })
            .collect::<Vec<_>>();
        paths.sort();
        for path in paths {
            let relative = format!(
                ".crane/policies/{}",
                path.file_name().unwrap_or_default().to_string_lossy()
            );
            let text = fs::read_to_string(&path)
                .map_err(|error| format!("could not read {relative}: {error}"))?;
            match parse(&relative, &text) {
                Ok(file) => set.files.push(file),
                Err(error) => set.errors.push((relative, error)),
            }
        }
        Ok(set)
    }

    /** Find a policy block by exact name
     * Input
        - name: &str - policy name
     * Output
        - Option<(&PolicyFile, &Block)>
    */
    pub(crate) fn find(&self, name: &str) -> Option<(&PolicyFile, &Block)> {
        self.files.iter().find_map(|file| {
            file.blocks
                .iter()
                .find(|block| block.name == name)
                .map(|block| (file, block))
        })
    }

    /** List every policy block with its file
     * Input
        - None (uses self)
     * Output
        - Vec<(&PolicyFile, &Block)>
    */
    pub(crate) fn blocks(&self) -> Vec<(&PolicyFile, &Block)> {
        self.files
            .iter()
            .flat_map(|file| file.blocks.iter().map(move |block| (file, block)))
            .collect()
    }

    /** List every command referencing a selection, with its policy
     * Input
        - id: &str - selection identifier
     * Output
        - Vec<(&PolicyFile, &Block, &Statement)>
    */
    pub(crate) fn references(&self, id: &str) -> Vec<(&PolicyFile, &Block, &Statement)> {
        self.blocks()
            .into_iter()
            .flat_map(|(file, block)| {
                block
                    .statements
                    .iter()
                    .filter(move |statement| statement.id == id)
                    .map(move |statement| (file, block, statement))
            })
            .collect()
    }
}

/** Find .crane files outside .crane/policies anywhere in the repository (policies must live in
 * the policies folder)
 * Input
    - workspace: &Workspace - repository
 * Output
    - Result<Vec<String>, String> repository-relative paths
    - Error if Git cannot list the files
*/
pub(crate) fn stray_policy_files(workspace: &Workspace) -> Result<Vec<String>, String> {
    Ok(git::list_files(&workspace.root)?
        .into_iter()
        .filter(|path| {
            Path::new(path)
                .extension()
                .is_some_and(|extension| extension == EXTENSION)
                && !path.starts_with(".crane/policies/")
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check a valid file with several policies, options, and comments
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn parses_policies_and_commands() {
        let text = "# payments\npolicy default {\n}\npolicy Payments {\n    preserve K7M2P9RX; // keep\n    target ABCD2345 change_type logical_bn;\n}\n";
        let file = parse("p.crane", text).unwrap();
        assert_eq!(file.blocks.len(), 2);
        assert_eq!(file.blocks[1].name, "Payments");
        let statements = &file.blocks[1].statements;
        assert_eq!(statements[0].operation, Operation::Preserve);
        assert_eq!(statements[1].change_type, Some(ChangeType::LogicalBn));
        assert_eq!(
            &text[statements[1].start..statements[1].end],
            "target ABCD2345 change_type logical_bn;"
        );
    }

    /** Check the errors the language reports
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn reports_syntax_errors() {
        let cases = [
            ("preserve K7M2P9RX;", "inside a policy block"),
            ("Policy a {\n}", "lowercase"),
            ("policy a {\n preserve K7M2P9RX\n}", "end with ';'"),
            (
                "policy a {\n preserve k7m2p9rx;\n}",
                "invalid selection marker",
            ),
            ("policy a {\n delete K7M2P9RX;\n}", "unknown command"),
            (
                "policy a {\n preserve K7M2P9RX change_type semantic;\n}",
                "only to target",
            ),
            (
                "policy a {\n target K7M2P9RX change_type bogus;\n}",
                "change_type",
            ),
            ("policy a {\n preserve K7M2P9RX;", "not closed"),
            ("policy 9a {\n}", "invalid policy name"),
        ];
        for (text, expected) in cases {
            let error = parse("p.crane", text).unwrap_err();
            assert!(
                error.message.contains(expected),
                "{text}: {}",
                error.message
            );
        }
    }

    /** Check that inserting and removing commands keeps the file parseable and tidy
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn edits_commands_in_place() {
        let text = "policy default {\n}\n";
        let file = parse("p.crane", text).unwrap();
        let added = insert_statement(text, &file.blocks[0], "preserve K7M2P9RX;");
        assert_eq!(added, "policy default {\n    preserve K7M2P9RX;\n}\n");
        let parsed = parse("p.crane", &added).unwrap();
        let removed = remove_statement(&added, &parsed.blocks[0].statements[0]);
        assert_eq!(removed, text);
        let inline = "policy a { }";
        let file = parse("p.crane", inline).unwrap();
        let added = insert_statement(inline, &file.blocks[0], "target ABCD2345;");
        assert_eq!(
            parse("p.crane", &added).unwrap().blocks[0].statements.len(),
            1
        );
    }
}
