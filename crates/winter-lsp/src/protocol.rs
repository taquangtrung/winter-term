//! What the messages to and from a language server say, read into plain
//! values: the parameters each request is sent with, and what each answer
//! and notification holds.
//!
//! A position on the wire counts UTF-16 code units, which is what every
//! server is required to understand; the editor counts characters, and
//! [`to_units`] and [`to_chars`] convert on the line the position is on.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

// ========================================================================
// Constants
// ========================================================================

/// The characters a URI path keeps as they are; every other byte is
/// written `%XX`.
const URI_KEEPS: &[u8] = b"/-_.~!$&'()*+,;=:@";

/// The code action kinds the client says it can show.
const CODE_ACTION_KINDS: &[&str] = &[
    "quickfix",
    "refactor",
    "refactor.extract",
    "refactor.inline",
    "refactor.rewrite",
    "source",
    "source.organizeImports",
];

// ========================================================================
// Data Structures
// ========================================================================

/// A place in a document as the wire gives it: a line, and a column in
/// UTF-16 code units.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Position {
    /// The column, in UTF-16 code units.
    pub character: u32,
    /// The line, from zero.
    pub line: u32,
}

/// The stretch between two positions, the end not included.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Range {
    /// Where it ends, not included.
    pub end: Position,
    /// Where it starts.
    pub start: Position,
}

/// How bad a diagnostic says the problem is.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Severity {
    /// Wrong: the file will not build or run as it is.
    Error,
    /// Likely wrong, though it builds.
    Warning,
    /// Worth knowing, not wrong.
    Information,
    /// A suggestion, often drawn faintest of all.
    Hint,
}

/// A problem a server found in a document.
#[derive(Clone, Debug, PartialEq)]
pub struct Diagnostic {
    /// The rule or error number, where the server gives one.
    pub code: Option<String>,
    /// What the server says is wrong.
    pub message: String,
    /// Where in the document it is.
    pub range: Range,
    /// How bad it is.
    pub severity: Severity,
    /// What found it: `rustc`, `clippy`, `pyright`.
    pub source: Option<String>,
}

/// The diagnostics a server published for one document, replacing what it
/// said of it before.
#[derive(Clone, Debug, PartialEq)]
pub struct Published {
    /// Every problem in the document, none for a clean one.
    pub diagnostics: Vec<Diagnostic>,
    /// The document's file.
    pub path: PathBuf,
}

/// A place in a file: where a definition is, or a reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Location {
    /// The file.
    pub path: PathBuf,
    /// Where in it.
    pub range: Range,
}

/// Text to put in place of a range.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TextEdit {
    /// What goes in.
    pub new_text: String,
    /// What it replaces.
    pub range: Range,
}

/// Edits to several files at once, each file's in any order.
pub type FileEdits = Vec<(PathBuf, Vec<TextEdit>)>;

/// One thing a completion request offered.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionEntry {
    /// Edits elsewhere that come with it: the import a name needs.
    pub additional: Vec<TextEdit>,
    /// More about it: a type, a signature.
    pub detail: Option<String>,
    /// What the typed text is matched against.
    pub filter: String,
    /// What goes in, in snippet syntax when [`Self::snippet`] is set.
    pub insert: String,
    /// The server's kind number (`3` a function, `6` a variable).
    pub kind: Option<u64>,
    /// What the server calls it.
    pub label: String,
    /// Where on the line the text it replaces starts, where the server says.
    pub replace_from: Option<Position>,
    /// Whether what goes in is a snippet, with places to fill in, rather
    /// than plain text.
    pub snippet: bool,
    /// What it sorts by among the others.
    pub sort: String,
}

/// What a completion request answered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionList {
    /// What it offers.
    pub entries: Vec<CompletionEntry>,
    /// Whether typing on asks again, rather than narrowing these.
    pub incomplete: bool,
}

/// One fix or refactoring a server offers where the cursor is.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeAction {
    /// A command for the server to run, where it gives one rather than (or
    /// after) the edits.
    pub command: Option<Value>,
    /// The edits it makes, where it says them.
    pub edits: Option<FileEdits>,
    /// What the menu calls it.
    pub title: String,
}

// ========================================================================
// Position conversions
// ========================================================================

/// The UTF-16 column of character column `col` of `line`.
pub fn to_units(line: &str, col: usize) -> u32 {
    line.chars().take(col).map(|c| c.len_utf16() as u32).sum()
}

/// The character column of UTF-16 column `units` of `line`, the line's end
/// where it is past it.
pub fn to_chars(line: &str, units: u32) -> usize {
    let mut seen = 0;
    for (col, c) in line.chars().enumerate() {
        if seen >= units {
            return col;
        }
        seen += c.len_utf16() as u32;
    }
    line.chars().count()
}

/// The wire position of character `col` of row `row` of `lines`.
pub fn position_in(lines: &[String], row: usize, col: usize) -> Position {
    let line = lines.get(row).map_or("", String::as_str);
    Position {
        character: to_units(line, col),
        line: row as u32,
    }
}

/// The row and character column of wire position `at` in `lines`.
pub fn place_of(lines: &[String], at: Position) -> (usize, usize) {
    let row = at.line as usize;
    let line = lines.get(row).map_or("", String::as_str);
    (row, to_chars(line, at.character))
}

// ========================================================================
// URIs
// ========================================================================

/// The `file://` URI of `path`.
pub fn uri_of(path: &Path) -> String {
    let mut uri = String::from("file://");
    // The path's own bytes, not a lossy string of them: a name that is not
    // UTF-8 came back from the server as U+FFFD and never found its file.
    for &byte in path.as_os_str().as_encoded_bytes() {
        match byte.is_ascii_alphanumeric() || URI_KEEPS.contains(&byte) {
            true => uri.push(byte as char),
            false => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The path a `file://` URI names, or `None` for any other scheme.
pub fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = bytes.get(at + 1..at + 3).and_then(|pair| std::str::from_utf8(pair).ok());
        match (bytes[at], hex.and_then(|hex| u8::from_str_radix(hex, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                at += 3;
            }
            (byte, _) => {
                out.push(byte);
                at += 1;
            }
        }
    }
    Some(path_from_bytes(out))
}

/// The path whose bytes are `bytes`, as a file name on this system is any
/// bytes but `/` and NUL.
#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    use std::os::unix::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_vec(bytes))
}

/// The path whose bytes are `bytes`, read as UTF-8 where a name is text.
#[cfg(not(unix))]
fn path_from_bytes(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(&bytes).into_owned())
}

// ========================================================================
// Requests
// ========================================================================

/// What `initialize` is sent with: the project's root, and what this client
/// can do with what a server answers.
pub fn initialize_params(root: &Path) -> Value {
    let uri = uri_of(root);
    let name = root.file_name().map_or_else(|| uri.clone(), |name| name.to_string_lossy().into_owned());
    json!({
        "processId": std::process::id(),
        "clientInfo": {"name": "winter-term", "version": env!("CARGO_PKG_VERSION")},
        "rootUri": uri,
        "rootPath": root.to_string_lossy(),
        "workspaceFolders": [{"uri": uri, "name": name}],
        "capabilities": {
            "general": {"positionEncodings": ["utf-16"]},
            "window": {"workDoneProgress": true, "showMessage": {}},
            "workspace": {
                "applyEdit": true,
                "configuration": true,
                "workspaceFolders": true,
                "workspaceEdit": {"documentChanges": true},
            },
            "textDocument": {
                "synchronization": {"didSave": true, "dynamicRegistration": false},
                "completion": {
                    "completionItem": {
                        "snippetSupport": true,
                        "labelDetailsSupport": true,
                        "insertReplaceSupport": false,
                    },
                    "contextSupport": true,
                },
                "hover": {"contentFormat": ["plaintext", "markdown"]},
                "signatureHelp": {"signatureInformation": {"documentationFormat": ["plaintext"]}},
                "definition": {"linkSupport": true},
                "declaration": {"linkSupport": true},
                "typeDefinition": {"linkSupport": true},
                "implementation": {"linkSupport": true},
                "references": {},
                "rename": {"prepareSupport": false},
                "formatting": {},
                "codeAction": {
                    "codeActionLiteralSupport": {
                        "codeActionKind": {"valueSet": CODE_ACTION_KINDS},
                    },
                },
                "publishDiagnostics": {"relatedInformation": false},
            },
        },
    })
}

/// A document's identity and the place in it a request is about.
pub fn at_params(path: &Path, at: Position) -> Value {
    json!({
        "textDocument": {"uri": uri_of(path)},
        "position": {"line": at.line, "character": at.character},
    })
}

/// What `textDocument/didOpen` is sent with.
pub fn open_params(path: &Path, language: &str, version: i32, text: &str) -> Value {
    json!({
        "textDocument": {
            "uri": uri_of(path),
            "languageId": language,
            "version": version,
            "text": text,
        },
    })
}

/// What `textDocument/didChange` is sent with: the whole text again.
pub fn change_params(path: &Path, version: i32, text: &str) -> Value {
    json!({
        "textDocument": {"uri": uri_of(path), "version": version},
        "contentChanges": [{"text": text}],
    })
}

/// The document alone, for `didSave`, `didClose`, and `formatting`.
pub fn document_params(path: &Path) -> Value {
    json!({"textDocument": {"uri": uri_of(path)}})
}

/// What `textDocument/formatting` is sent with.
pub fn format_params(path: &Path, tab_size: usize, spaces: bool) -> Value {
    json!({
        "textDocument": {"uri": uri_of(path)},
        "options": {"tabSize": tab_size, "insertSpaces": spaces},
    })
}

/// What `textDocument/codeAction` is sent with: the range, and the
/// diagnostics the server said of it, which quick fixes answer.
pub fn code_action_params(path: &Path, range: Range, diagnostics: &[Diagnostic]) -> Value {
    let diagnostics: Vec<Value> = diagnostics
        .iter()
        .map(|diagnostic| {
            json!({
                "range": range_value(diagnostic.range),
                "message": diagnostic.message,
                "severity": match diagnostic.severity {
                    Severity::Error => 1,
                    Severity::Warning => 2,
                    Severity::Information => 3,
                    Severity::Hint => 4,
                },
                "source": diagnostic.source,
                "code": diagnostic.code,
            })
        })
        .collect();
    json!({
        "textDocument": {"uri": uri_of(path)},
        "range": range_value(range),
        "context": {"diagnostics": diagnostics},
    })
}

/// A range as the wire writes it.
fn range_value(range: Range) -> Value {
    json!({
        "start": {"line": range.start.line, "character": range.start.character},
        "end": {"line": range.end.line, "character": range.end.character},
    })
}

// ========================================================================
// Answers
// ========================================================================

/// A position from the wire.
fn position(value: &Value) -> Option<Position> {
    Some(Position {
        character: value.get("character")?.as_u64()? as u32,
        line: value.get("line")?.as_u64()? as u32,
    })
}

/// A range from the wire.
fn range(value: &Value) -> Option<Range> {
    Some(Range {
        end: position(value.get("end")?)?,
        start: position(value.get("start")?)?,
    })
}

/// The diagnostics `textDocument/publishDiagnostics` carries.
pub fn published(params: &Value) -> Option<Published> {
    let path = path_of(params.get("uri")?.as_str()?)?;
    let diagnostics = params
        .get("diagnostics")?
        .as_array()?
        .iter()
        .filter_map(|item| {
            let severity = match item.get("severity").and_then(Value::as_u64) {
                Some(2) => Severity::Warning,
                Some(3) => Severity::Information,
                Some(4) => Severity::Hint,
                _ => Severity::Error,
            };
            let code = item.get("code").and_then(|code| match code {
                Value::String(text) => Some(text.clone()),
                Value::Number(number) => Some(number.to_string()),
                _ => None,
            });
            Some(Diagnostic {
                code,
                message: item.get("message")?.as_str()?.to_string(),
                range: range(item.get("range")?)?,
                severity,
                source: item.get("source").and_then(Value::as_str).map(str::to_string),
            })
        })
        .collect();
    Some(Published { diagnostics, path })
}

/// The places a definition, declaration, or references request answered:
/// one location, a list of them, or a list of links.
pub fn locations(result: &Value) -> Vec<Location> {
    let one = |item: &Value| -> Option<Location> {
        let uri = item.get("uri").or_else(|| item.get("targetUri"))?.as_str()?;
        let range = item
            .get("targetSelectionRange")
            .or_else(|| item.get("range"))
            .and_then(range)?;
        Some(Location {
            path: path_of(uri)?,
            range,
        })
    };
    match result {
        Value::Array(items) => items.iter().filter_map(one).collect(),
        Value::Object(_) => one(result).into_iter().collect(),
        _ => Vec::new(),
    }
}

/// What a completion request offered, sorted the way the server asks.
pub fn completions(result: &Value) -> CompletionList {
    let (items, incomplete) = match result {
        Value::Array(items) => (items.as_slice(), false),
        Value::Object(list) => (
            list.get("items").and_then(Value::as_array).map_or(&[][..], Vec::as_slice),
            list.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false),
        ),
        _ => (&[][..], false),
    };
    let mut entries: Vec<CompletionEntry> = items.iter().filter_map(completion_entry).collect();
    entries.sort_by(|a, b| a.sort.cmp(&b.sort));
    CompletionList { entries, incomplete }
}

/// One completion item.
fn completion_entry(item: &Value) -> Option<CompletionEntry> {
    let label = item.get("label")?.as_str()?.to_string();
    let text = |key: &str| item.get(key).and_then(Value::as_str).map(str::to_string);
    let edit = item.get("textEdit");
    let edit_range = edit.and_then(|edit| edit.get("range").or_else(|| edit.get("insert")));
    let insert = edit
        .and_then(|edit| edit.get("newText"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| text("insertText"))
        .unwrap_or_else(|| label.clone());
    let details = item.get("labelDetails");
    let detail = [
        details.and_then(|d| d.get("detail")).and_then(Value::as_str),
        text("detail").as_deref().or_else(|| details.and_then(|d| d.get("description")).and_then(Value::as_str)),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .filter(|part| !part.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    Some(CompletionEntry {
        additional: item.get("additionalTextEdits").map(text_edits).unwrap_or_default(),
        detail: (!detail.is_empty()).then_some(detail),
        filter: text("filterText").unwrap_or_else(|| label.clone()),
        insert,
        kind: item.get("kind").and_then(Value::as_u64),
        replace_from: edit_range.and_then(range).map(|range| range.start),
        snippet: item.get("insertTextFormat").and_then(Value::as_u64) == Some(2),
        sort: text("sortText").unwrap_or_else(|| label.clone()),
        label,
    })
}

/// What a hover request answered, as plain lines: a code block's fences
/// dropped, its code kept.
pub fn hover_text(result: &Value) -> Option<String> {
    let contents = result.get("contents")?;
    let piece = |value: &Value| -> Option<String> {
        match value {
            Value::String(text) => Some(text.clone()),
            Value::Object(map) => map.get("value").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }
    };
    let text = match contents {
        Value::Array(items) => items.iter().filter_map(piece).collect::<Vec<_>>().join("\n\n"),
        other => piece(other)?,
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    let joined = lines.join("\n").trim().to_string();
    (!joined.is_empty()).then_some(joined)
}

/// The signature a signature help request answered, with the parameter
/// being typed marked.
pub fn signature_text(result: &Value) -> Option<String> {
    let signatures = result.get("signatures")?.as_array()?;
    let active = result.get("activeSignature").and_then(Value::as_u64).unwrap_or(0) as usize;
    let signature = signatures.get(active).or_else(|| signatures.first())?;
    let label = signature.get("label")?.as_str()?.to_string();
    let parameter = signature
        .get("activeParameter")
        .or_else(|| result.get("activeParameter"))
        .and_then(Value::as_u64)
        .and_then(|at| signature.get("parameters")?.as_array()?.get(at as usize).cloned())
        .and_then(|parameter| match parameter.get("label")? {
            Value::String(text) => Some(text.clone()),
            Value::Array(bounds) => {
                let from = bounds.first()?.as_u64()? as usize;
                let to = bounds.get(1)?.as_u64()? as usize;
                let units: Vec<u16> = label.encode_utf16().collect();
                String::from_utf16(units.get(from..to)?).ok()
            }
            _ => None,
        });
    Some(match parameter {
        Some(parameter) => format!("{label}\n→ {parameter}"),
        None => label,
    })
}

/// The edits a formatting request, or one file of a workspace edit,
/// carries.
pub fn text_edits(result: &Value) -> Vec<TextEdit> {
    result
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(TextEdit {
                        new_text: item.get("newText")?.as_str()?.to_string(),
                        range: range(item.get("range")?)?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The edits a workspace edit makes, file by file: its `documentChanges`
/// where it has them, else its `changes`. Creating, renaming, and deleting
/// files is left out.
pub fn workspace_edits(edit: &Value) -> FileEdits {
    if let Some(changes) = edit.get("documentChanges").and_then(Value::as_array) {
        return changes
            .iter()
            .filter_map(|change| {
                let uri = change.get("textDocument")?.get("uri")?.as_str()?;
                Some((path_of(uri)?, text_edits(change.get("edits")?)))
            })
            .collect();
    }
    edit.get("changes")
        .and_then(Value::as_object)
        .map(|changes| {
            changes
                .iter()
                .filter_map(|(uri, edits)| Some((path_of(uri)?, text_edits(edits))))
                .collect()
        })
        .unwrap_or_default()
}

/// The fixes and refactorings a code action request offered.
pub fn code_actions(result: &Value) -> Vec<CodeAction> {
    let Some(items) = result.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            // A bare command has its name where an action has its command.
            let command = match item.get("command") {
                Some(Value::String(_)) => Some(item.clone()),
                Some(command @ Value::Object(_)) => Some(command.clone()),
                _ => None,
            };
            Some(CodeAction {
                command,
                edits: item.get("edit").map(workspace_edits),
                title,
            })
        })
        .collect()
}

/// `lines` with `edits` made, each edit's range read against the lines as
/// they were before any of them.
pub fn apply_edits(lines: &[String], edits: &[TextEdit]) -> Vec<String> {
    // Made from the end back, so an edit never moves the text a later one
    // reads its range against; edits starting at the same place go last to
    // first, which leaves inserts there in the order the server listed them.
    let mut ordered: Vec<(usize, &TextEdit)> = edits.iter().enumerate().collect();
    ordered.sort_by_key(|(index, edit)| std::cmp::Reverse((edit.range.start, *index)));
    let mut text: Vec<String> = lines.to_vec();
    for (_, edit) in ordered {
        let (start_row, start_col) = place_of(&text, edit.range.start);
        let (end_row, end_col) = place_of(&text, edit.range.end);
        if start_row >= text.len() {
            // An edit past the end appends, as one that adds a final line does.
            let last = text.len().saturating_sub(1);
            let mut tail: Vec<String> = edit.new_text.split('\n').map(str::to_string).collect();
            if let Some(line) = text.get_mut(last) {
                line.push_str(&tail.remove(0));
            }
            text.extend(tail);
            continue;
        }
        let end_row = end_row.min(text.len() - 1);
        let head: String = text[start_row].chars().take(start_col).collect();
        let end_col = match end_row == edit.range.end.line as usize {
            true => end_col,
            false => text[end_row].chars().count(),
        };
        let tail: String = text[end_row].chars().skip(end_col).collect();
        let joined = format!("{head}{}{tail}", edit.new_text);
        let replaced: Vec<String> = joined.split('\n').map(|line| line.trim_end_matches('\r').to_string()).collect();
        text.splice(start_row..=end_row, replaced);
    }
    text
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    fn edit(from: (u32, u32), to: (u32, u32), text: &str) -> TextEdit {
        TextEdit {
            new_text: text.to_string(),
            range: Range {
                end: Position { character: to.1, line: to.0 },
                start: Position { character: from.1, line: from.0 },
            },
        }
    }

    #[test]
    fn test_a_column_counts_utf16_units_on_the_wire_and_characters_here() {
        let line = "a😀b";
        assert_eq!(to_units(line, 2), 3, "the emoji takes two units");
        assert_eq!(to_chars(line, 3), 2);
        assert_eq!(to_chars(line, 99), 3, "past the end is the end");
    }

    #[test]
    fn test_a_path_survives_a_trip_through_its_uri() {
        let path = Path::new("/home/me/my notes/ünï#1.rs");
        let uri = uri_of(path);
        assert_eq!(uri, "file:///home/me/my%20notes/%C3%BCn%C3%AF%231.rs");
        assert_eq!(path_of(&uri).as_deref(), Some(path));
        assert_eq!(path_of("untitled:1"), None);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_name_that_is_not_utf8_survives_its_uri_too() {
        // Taken as a lossy string, its odd byte went to the server as U+FFFD,
        // and what the server said of the file named one that is not there.
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/caf\xe9.rs"));
        let uri = uri_of(path);
        assert_eq!(uri, "file:///tmp/caf%E9.rs");
        assert_eq!(path_of(&uri).as_deref(), Some(path));
    }

    #[test]
    fn test_edits_apply_against_the_text_before_any_of_them() {
        let text = lines(&["fn main() {", "let x=1;", "}"]);
        let edits = [edit((1, 0), (1, 0), "    "), edit((1, 5), (1, 6), " = "), edit((2, 1), (2, 1), "\n")];
        assert_eq!(apply_edits(&text, &edits), lines(&["fn main() {", "    let x = 1;", "}", ""]));
        let joined = apply_edits(&text, &[edit((0, 11), (2, 0), " ")]);
        assert_eq!(joined, lines(&["fn main() { }"]), "an edit across lines joins them");
    }

    #[test]
    fn test_edits_at_one_place_land_in_the_order_the_server_listed_them() {
        // Applied in that order, each later insert went in before the one
        // made just ahead of it: `a` then `b` came out `ba`.
        let text = lines(&["xy"]);
        let both = [edit((0, 1), (0, 1), "a"), edit((0, 1), (0, 1), "b")];
        assert_eq!(apply_edits(&text, &both), lines(&["xaby"]));
        let insert_then_replace = [edit((0, 0), (0, 0), "a"), edit((0, 0), (0, 1), "X")];
        assert_eq!(apply_edits(&text, &insert_then_replace), lines(&["aXy"]), "the insert is kept");
    }

    #[test]
    fn test_locations_read_one_a_list_or_links() {
        let one = json!({"uri": "file:///a.rs", "range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 3}}});
        assert_eq!(locations(&one).len(), 1);
        let link = json!([{"targetUri": "file:///b.rs",
            "targetRange": {"start": {"line": 0, "character": 0}, "end": {"line": 9, "character": 0}},
            "targetSelectionRange": {"start": {"line": 4, "character": 3}, "end": {"line": 4, "character": 6}}}]);
        let found = locations(&link);
        assert_eq!(found[0].path, PathBuf::from("/b.rs"));
        assert_eq!(found[0].range.start.line, 4, "a link lands on its name, not its whole body");
        assert!(locations(&Value::Null).is_empty());
    }

    #[test]
    fn test_completions_sort_by_sort_text_and_take_the_edit_s_text_and_start() {
        let result = json!({"isIncomplete": true, "items": [
            {"label": "push(…)", "filterText": "push", "sortText": "b", "kind": 2,
             "insertTextFormat": 2, "labelDetails": {"detail": "(value)"}, "detail": "fn(&mut self, T)",
             "textEdit": {"newText": "push(${1:value})", "range": {"start": {"line": 0, "character": 4}, "end": {"line": 0, "character": 6}}}},
            {"label": "pop", "sortText": "a"},
        ]});
        let list = completions(&result);
        assert!(list.incomplete);
        assert_eq!(list.entries[0].label, "pop");
        let push = &list.entries[1];
        assert_eq!((push.filter.as_str(), push.insert.as_str()), ("push", "push(${1:value})"));
        assert!(push.snippet);
        assert_eq!(push.replace_from, Some(Position { character: 4, line: 0 }));
        assert_eq!(push.detail.as_deref(), Some("(value) fn(&mut self, T)"));
    }

    #[test]
    fn test_hover_drops_code_fences_and_keeps_the_code() {
        let result = json!({"contents": {"kind": "markdown", "value": "```rust\nfn f() -> u8\n```\n\nDoes a thing."}});
        assert_eq!(hover_text(&result).as_deref(), Some("fn f() -> u8\n\nDoes a thing."));
        assert_eq!(hover_text(&json!({"contents": []})), None);
    }

    #[test]
    fn test_a_signature_marks_the_parameter_being_typed() {
        let result = json!({"signatures": [{"label": "fn f(a: u8, b: u8)",
            "parameters": [{"label": [5, 10]}, {"label": "b: u8"}], "activeParameter": 0}]});
        assert_eq!(signature_text(&result).as_deref(), Some("fn f(a: u8, b: u8)\n→ a: u8"));
    }

    #[test]
    fn test_a_workspace_edit_reads_document_changes_or_changes() {
        let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}});
        let changes = json!({"changes": {"file:///a.rs": [{"range": range, "newText": "x"}]}});
        assert_eq!(workspace_edits(&changes)[0].0, PathBuf::from("/a.rs"));
        let documents = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///b.rs", "version": 1}, "edits": [{"range": range, "newText": "y"}]},
            {"kind": "create", "uri": "file:///c.rs"},
        ]});
        let edits = workspace_edits(&documents);
        assert_eq!(edits.len(), 1, "file operations are left out");
        assert_eq!(edits[0].1[0].new_text, "y");
    }

    #[test]
    fn test_published_diagnostics_default_to_errors_and_keep_numeric_codes() {
        let params = json!({"uri": "file:///a.rs", "diagnostics": [
            {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 4}},
             "message": "unused", "severity": 2, "code": 4, "source": "rustc"},
            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": "bad"},
        ]});
        let published = published(&params).expect("diagnostics");
        assert_eq!(published.diagnostics[0].severity, Severity::Warning);
        assert_eq!(published.diagnostics[0].code.as_deref(), Some("4"));
        assert_eq!(published.diagnostics[1].severity, Severity::Error);
    }
}
