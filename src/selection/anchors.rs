// Source anchors for selections, zones, and flows. The comment syntax depends on the file's
// language, but marker identities only refer to their signed registry or governance entries;
// malformed, duplicated, unmatched, or unregistered markers are reported, never trusted.

/** Text every marker carries between the comment opener and the identifier */
pub(crate) const MARKER_TAG: &str = "@crane:selection:";
pub(crate) const ZONE_MARKER_TAG: &str = "@crane:zone:";
pub(crate) const FLOW_MARKER_TAG: &str = "@crane:flow:";
pub(crate) const ANY_MARKER_TAG: &str = "@crane:";

/** Comment openers that make a line a marker candidate, whatever the file's language, so a
 * marker written in the wrong syntax is reported instead of ignored */
const CANDIDATE_OPENERS: &[&str] = &["<!--", "//", "/*", "#", "--", ";", "%", "'"];

/** How a language writes a single-line comment
 * Variants
    - Line(opener) - a comment runs from the opener to the end of the line
    - Block(opener, closer) - a comment is enclosed by the opener and the closer
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommentSyntax {
    Line(&'static str),
    Block(&'static str, &'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MarkerKind {
    Selection,
    Zone,
    Flow,
}

impl MarkerKind {
    pub(crate) fn tag(self) -> &'static str {
        match self {
            Self::Selection => MARKER_TAG,
            Self::Zone => ZONE_MARKER_TAG,
            Self::Flow => FLOW_MARKER_TAG,
        }
    }
}

/** A comment adapter: the language name and its comment syntax
 * Fields
    - name: &'static str - language name recorded in the registry
    - syntax: CommentSyntax - how markers are written in it
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Language {
    pub(crate) name: &'static str,
    pub(crate) syntax: CommentSyntax,
}

/** Which end of a selection a marker delimits
 * Variants
    - Start - the line before the first selected line
    - End - the line after the last selected line
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Boundary {
    Start,
    End,
}

impl Boundary {
    /** Return the suffix written in the marker
     * Input
        - None (uses self)
     * Output
        - &'static str, "start" or "end"
    */
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::End => "end",
        }
    }
}

/** Select the comment adapter for a file from its extension or well-known file name
 * Input
    - path: &str - repository-relative path
 * Output
    - Option<Language>, None for file types without a supported comment syntax (JSON, binaries)
*/
pub(crate) fn language_for(path: &str) -> Option<Language> {
    let file = path.rsplit('/').next().unwrap_or(path);
    let lower = file.to_ascii_lowercase();
    let by_name = match lower.as_str() {
        "dockerfile" | "makefile" | "gnumakefile" | "cmakelists.txt" | "jenkinsfile"
        | "gemfile" | "rakefile" | "procfile" | ".gitignore" | ".dockerignore"
        | ".editorconfig" | ".env" => Some(("config", CommentSyntax::Line("#"))),
        _ => None,
    };
    if let Some((name, syntax)) = by_name {
        return Some(Language { name, syntax });
    }
    let extension = lower.rsplit_once('.')?.1;
    let (name, syntax) = match extension {
        "rs" => ("rust", CommentSyntax::Line("//")),
        "js" | "jsx" | "mjs" | "cjs" => ("javascript", CommentSyntax::Line("//")),
        "ts" | "tsx" | "mts" | "cts" => ("typescript", CommentSyntax::Line("//")),
        "java" => ("java", CommentSyntax::Line("//")),
        "kt" | "kts" => ("kotlin", CommentSyntax::Line("//")),
        "go" => ("go", CommentSyntax::Line("//")),
        "c" | "h" => ("c", CommentSyntax::Line("//")),
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" | "ino" => ("cpp", CommentSyntax::Line("//")),
        "cs" => ("csharp", CommentSyntax::Line("//")),
        "swift" => ("swift", CommentSyntax::Line("//")),
        "scala" | "sc" => ("scala", CommentSyntax::Line("//")),
        "dart" => ("dart", CommentSyntax::Line("//")),
        "php" => ("php", CommentSyntax::Line("//")),
        "groovy" | "gradle" => ("groovy", CommentSyntax::Line("//")),
        "proto" => ("protobuf", CommentSyntax::Line("//")),
        "zig" => ("zig", CommentSyntax::Line("//")),
        "sol" => ("solidity", CommentSyntax::Line("//")),
        "fs" | "fsx" => ("fsharp", CommentSyntax::Line("//")),
        "jsonc" => ("jsonc", CommentSyntax::Line("//")),
        "py" | "pyi" | "pyw" => ("python", CommentSyntax::Line("#")),
        "rb" => ("ruby", CommentSyntax::Line("#")),
        "sh" | "bash" | "zsh" | "fish" | "ksh" => ("shell", CommentSyntax::Line("#")),
        "ps1" | "psm1" => ("powershell", CommentSyntax::Line("#")),
        "yaml" | "yml" => ("yaml", CommentSyntax::Line("#")),
        "toml" => ("toml", CommentSyntax::Line("#")),
        "r" => ("r", CommentSyntax::Line("#")),
        "pl" | "pm" => ("perl", CommentSyntax::Line("#")),
        "ex" | "exs" => ("elixir", CommentSyntax::Line("#")),
        "nim" => ("nim", CommentSyntax::Line("#")),
        "tf" | "hcl" => ("terraform", CommentSyntax::Line("#")),
        "cmake" => ("cmake", CommentSyntax::Line("#")),
        "conf" | "cfg" | "properties" => ("config", CommentSyntax::Line("#")),
        "jl" => ("julia", CommentSyntax::Line("#")),
        "sql" => ("sql", CommentSyntax::Line("--")),
        "lua" => ("lua", CommentSyntax::Line("--")),
        "hs" => ("haskell", CommentSyntax::Line("--")),
        "elm" => ("elm", CommentSyntax::Line("--")),
        "ada" | "adb" | "ads" => ("ada", CommentSyntax::Line("--")),
        "erl" | "hrl" => ("erlang", CommentSyntax::Line("%")),
        "tex" | "sty" => ("latex", CommentSyntax::Line("%")),
        "clj" | "cljs" | "cljc" | "lisp" | "el" | "scm" | "rkt" => {
            ("lisp", CommentSyntax::Line(";"))
        }
        "ini" => ("ini", CommentSyntax::Line(";")),
        "asm" | "s" => ("assembly", CommentSyntax::Line(";")),
        "vb" | "vbs" | "bas" => ("visualbasic", CommentSyntax::Line("'")),
        "html" | "htm" | "xhtml" | "xml" | "svg" | "xaml" | "vue" | "svelte" | "md"
        | "markdown" | "csproj" | "plist" => ("markup", CommentSyntax::Block("<!--", "-->")),
        "css" | "scss" | "sass" | "less" => ("css", CommentSyntax::Block("/*", "*/")),
        _ => return None,
    };
    Some(Language { name, syntax })
}

/** Render a marker comment without indentation or line ending
 * Input
    - syntax: CommentSyntax - comment syntax of the file
    - id: &str - selection identifier
    - boundary: Boundary - start or end
 * Output
    - String such as "# @crane:selection:K7M2P9RX:start"
*/
pub(crate) fn render(syntax: CommentSyntax, id: &str, boundary: Boundary) -> String {
    render_kind(syntax, MarkerKind::Selection, id, boundary)
}

pub(crate) fn render_kind(
    syntax: CommentSyntax,
    kind: MarkerKind,
    id: &str,
    boundary: Boundary,
) -> String {
    let marker = format!("{}{id}:{}", kind.tag(), boundary.name());
    match syntax {
        CommentSyntax::Line(opener) => format!("{opener} {marker}"),
        CommentSyntax::Block(opener, closer) => format!("{opener} {marker} {closer}"),
    }
}

/** One line of a file, with its byte span
 * Fields
    - number: usize - 1-based line number
    - start: usize - byte offset of the first character
    - end: usize - byte offset just past the line ending (or the end of the text)
    - text: &'a str - the line without its line ending
*/
#[derive(Debug, Clone, Copy)]
pub(crate) struct Line<'a> {
    pub(crate) number: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) text: &'a str,
}

/** Split text into lines with byte spans, keeping CRLF and LF endings out of the line text; a
 * final line without a newline is included, an empty trailing segment is not
 * Input
    - text: &str - file content
 * Output
    - Vec<Line>
*/
pub(crate) fn lines(text: &str) -> Vec<Line<'_>> {
    let mut result = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    while start < bytes.len() {
        let newline = bytes[start..].iter().position(|byte| *byte == b'\n');
        let (content_end, end) = match newline {
            Some(offset) => (start + offset, start + offset + 1),
            None => (bytes.len(), bytes.len()),
        };
        let content_end = if content_end > start && bytes[content_end - 1] == b'\r' {
            content_end - 1
        } else {
            content_end
        };
        result.push(Line {
            number: result.len() + 1,
            start,
            end,
            text: &text[start..content_end],
        });
        start = end;
    }
    result
}

/** A marker found in a file
 * Fields
    - id: Option<String> - the identifier, None when the marker is too malformed to read one
    - boundary: Option<Boundary> - start or end, None when unreadable
    - line: usize - 1-based line number
    - start: usize - byte offset of the marker line
    - end: usize - byte offset just past the marker line ending
    - problem: Option<String> - why the marker is malformed, None when well formed
*/
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marker {
    pub(crate) kind: MarkerKind,
    pub(crate) id: Option<String>,
    pub(crate) boundary: Option<Boundary>,
    pub(crate) line: usize,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) problem: Option<String>,
}

/** Find every marker candidate in a file: lines that, after leading whitespace, start with a
 * comment opener of any supported language followed by the marker tag; each is parsed against
 * the file's own comment syntax and reported as well formed or with the problem found (wrong
 * comment syntax, unreadable identifier, unknown boundary, trailing text)
 * Input
    - path: &str - repository-relative path, selecting the comment adapter
    - text: &str - file content
 * Output
    - Vec<Marker> in line order
*/
pub(crate) fn find_markers(path: &str, text: &str) -> Vec<Marker> {
    if !text.contains(ANY_MARKER_TAG) {
        return Vec::new();
    }
    let language = language_for(path);
    lines(text)
        .into_iter()
        .filter_map(|line| {
            let trimmed = line.text.trim();
            let candidate = CANDIDATE_OPENERS.iter().any(|opener| {
                trimmed.strip_prefix(opener).is_some_and(|rest| {
                    [MARKER_TAG, ZONE_MARKER_TAG, FLOW_MARKER_TAG, ANY_MARKER_TAG]
                        .iter()
                        .any(|tag| rest.trim_start().starts_with(tag))
                })
            });
            candidate.then(|| parse_marker(language, trimmed, line))
        })
        .collect()
}

/** Parse one marker candidate line against the file's comment syntax
 * Input
    - language: Option<Language> - the file's adapter, None for unsupported file types
    - trimmed: &str - the line without surrounding whitespace
    - line: Line - the line and its span
 * Output
    - Marker, with problem set when the line is not exactly a well-formed marker
*/
fn parse_marker(language: Option<Language>, trimmed: &str, line: Line) -> Marker {
    let (kind, tag) = [
        (MarkerKind::Selection, MARKER_TAG),
        (MarkerKind::Zone, ZONE_MARKER_TAG),
        (MarkerKind::Flow, FLOW_MARKER_TAG),
    ]
    .into_iter()
    .find(|(_, tag)| trimmed.contains(tag))
    .unwrap_or((MarkerKind::Selection, ANY_MARKER_TAG));
    let recognized = tag != ANY_MARKER_TAG;
    let offset = trimmed.find(tag).unwrap_or(0);
    let after = &trimmed[offset + tag.len()..];
    let id_len = if kind == MarkerKind::Selection {
        8
    } else {
        after.find(':').unwrap_or(after.len())
    };
    let id = after.get(..id_len).map(str::to_string);
    let boundary = after.get(id_len..).and_then(|rest| {
        let rest = rest.strip_prefix(':')?;
        if rest.starts_with("start") {
            Some(Boundary::Start)
        } else if rest.starts_with("end") {
            Some(Boundary::End)
        } else {
            None
        }
    });
    let mut marker = Marker {
        kind,
        id: id.clone().filter(|id| match kind {
            MarkerKind::Selection => crate::trust::crypto::is_selection_id(id),
            MarkerKind::Zone | MarkerKind::Flow => {
                crate::platform::validate_name("marker", id).is_ok()
            }
        }),
        boundary,
        line: line.number,
        start: line.start,
        end: line.end,
        problem: None,
    };
    marker.problem = match (language, &marker.id, boundary) {
        (None, _, _) => {
            Some("this file type has no supported comment syntax for Crane markers".into())
        }
        (_, _, _) if !recognized => Some("unknown Crane marker type".into()),
        (_, None, _) => Some(format!(
            "unreadable {} identifier '{}'",
            match kind {
                MarkerKind::Selection => "selection",
                MarkerKind::Zone => "zone",
                MarkerKind::Flow => "flow",
            },
            id.unwrap_or_default()
        )),
        (_, _, None) => Some("the marker must end with ':start' or ':end'".into()),
        (Some(language), Some(id), Some(boundary)) => {
            let expected = render_kind(language.syntax, kind, id, boundary);
            (trimmed != expected).then(|| {
                format!(
                    "malformed marker; expected exactly '{expected}' for {} files",
                    language.name
                )
            })
        }
    };
    marker
}

/** Insert start and end markers around an inclusive 1-based line range, using the file's own
 * line ending and, for both markers, the indentation of the first selected line; a file without
 * a final newline gets one before the end marker
 * Input
    - text: &str - current file content
    - syntax: CommentSyntax - comment syntax of the file
    - id: &str - selection identifier
    - start_line: usize - first selected line
    - end_line: usize - last selected line
 * Output
    - Result<String, String> the new file content
    - Error if the range is empty, inverted, or outside the file
*/
pub(crate) fn insert(
    text: &str,
    syntax: CommentSyntax,
    id: &str,
    start_line: usize,
    end_line: usize,
) -> Result<String, String> {
    let all = lines(text);
    if start_line == 0 || end_line < start_line || end_line > all.len() {
        return Err(format!(
            "line range {start_line}-{end_line} is outside the file (it has {} lines)",
            all.len()
        ));
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let indentation =
        |line: &Line| line.text[..line.text.len() - line.text.trim_start().len()].to_string();
    let first = &all[start_line - 1];
    let last = &all[end_line - 1];
    let mut output = String::with_capacity(text.len() + 120);
    output.push_str(&text[..first.start]);
    output.push_str(&indentation(first));
    output.push_str(&render(syntax, id, Boundary::Start));
    output.push_str(newline);
    output.push_str(&text[first.start..last.end]);
    if !text[..last.end].ends_with('\n') {
        output.push_str(newline);
    }
    output.push_str(&indentation(first));
    output.push_str(&render(syntax, id, Boundary::End));
    output.push_str(newline);
    output.push_str(&text[last.end..]);
    Ok(output)
}

/** Normalize selected content for digesting: CRLF line endings become LF, so a checkout that
 * converts line endings does not look like a change; every other byte is kept exactly
 * Input
    - content: &str - text between the start and end markers
 * Output
    - String normalized content
*/
pub(crate) fn normalize(content: &str) -> String {
    content.replace("\r\n", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /** Check marker rendering for line and block comment languages
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn renders_language_specific_markers() {
        let python = language_for("app/pay.py").unwrap();
        assert_eq!(
            render(python.syntax, "K7M2P9RX", Boundary::Start),
            "# @crane:selection:K7M2P9RX:start"
        );
        let rust = language_for("src/lib.rs").unwrap();
        assert_eq!(
            render(rust.syntax, "K7M2P9RX", Boundary::End),
            "// @crane:selection:K7M2P9RX:end"
        );
        let html = language_for("index.html").unwrap();
        assert_eq!(
            render(html.syntax, "K7M2P9RX", Boundary::Start),
            "<!-- @crane:selection:K7M2P9RX:start -->"
        );
        assert!(language_for("data.json").is_none());
        assert_eq!(
            language_for("Dockerfile").unwrap().syntax,
            CommentSyntax::Line("#")
        );
    }

    /** Check insertion keeps indentation, line endings, and the selected bytes
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn inserts_markers_around_the_range() {
        let text = "def a():\r\n    x = 1\r\n    return x\r\n";
        let output = insert(text, CommentSyntax::Line("#"), "AAAAAAAA", 2, 3).unwrap();
        assert_eq!(
            output,
            "def a():\r\n    # @crane:selection:AAAAAAAA:start\r\n    x = 1\r\n    return x\r\n    # @crane:selection:AAAAAAAA:end\r\n"
        );
        let markers = find_markers("a.py", &output);
        assert_eq!(markers.len(), 2);
        assert!(markers.iter().all(|marker| marker.problem.is_none()));
        assert_eq!(markers[0].boundary, Some(Boundary::Start));
        let no_newline = insert("only", CommentSyntax::Line("//"), "AAAAAAAA", 1, 1).unwrap();
        assert_eq!(
            no_newline,
            "// @crane:selection:AAAAAAAA:start\nonly\n// @crane:selection:AAAAAAAA:end\n"
        );
        assert!(insert("a\n", CommentSyntax::Line("//"), "AAAAAAAA", 1, 2).is_err());
    }

    /** Check that wrong syntax, bad identifiers, and trailing text are reported
     * Input
        - None
     * Output
        - None (panics on failure)
    */
    #[test]
    fn reports_malformed_markers() {
        let text = "// @crane:selection:AAAAAAAA:start\n# @crane:selection:abc:end\n# @crane:selection:BBBBBBBB:middle\n";
        let markers = find_markers("x.py", text);
        assert_eq!(markers.len(), 3);
        assert!(markers[0]
            .problem
            .as_deref()
            .unwrap()
            .contains("malformed marker"));
        assert!(markers[1]
            .problem
            .as_deref()
            .unwrap()
            .contains("unreadable"));
        assert!(markers[2].problem.is_some());
        assert!(find_markers("x.py", "print('@crane:selection:AAAAAAAA:start')\n").is_empty());
        assert_eq!(
            find_markers("x.json", "# @crane:selection:AAAAAAAA:start\n").len(),
            1
        );
    }
}
