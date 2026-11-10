//! A Swift file's outline: its declarations of types, extensions, and
//! functions, each found by the keyword that declares it once the
//! attributes and modifiers before it are passed, nested as their braces
//! nest. Swift's grammar marks no definitions, so it is read here. A
//! property or a local is left out; braces in strings and comments do not
//! count.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// The keywords that declare what the outline lists.
const DECLARATIONS: [&str; 11] = [
    "actor",
    "class",
    "deinit",
    "enum",
    "extension",
    "func",
    "init",
    "protocol",
    "struct",
    "subscript",
    "typealias",
];

/// The modifiers a declaration may start with.
const MODIFIERS: [&str; 20] = [
    "convenience",
    "distributed",
    "dynamic",
    "fileprivate",
    "final",
    "indirect",
    "internal",
    "lazy",
    "mutating",
    "nonisolated",
    "nonmutating",
    "open",
    "optional",
    "override",
    "package",
    "private",
    "public",
    "required",
    "static",
    "unowned",
];

// ========================================================================
// Data Structures
// ========================================================================

/// Where the scan is in the text: how deep in braces, and inside what.
#[derive(Default)]
struct Scan {
    comments: usize,
    depth: usize,
    /// Whether inside a string, and whether it is a `"""` one.
    string: Option<bool>,
}

// ========================================================================
// Functions
// ========================================================================

/// The outline of the Swift file `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline = Vec::new();
    let mut scan = Scan::default();
    // The brace depth inside each listed declaration holding the line.
    let mut open: Vec<usize> = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        let start = scan.depth;
        let in_text = scan.comments > 0 || scan.string.is_some();
        if !in_text && declares(line) {
            while open.last().is_some_and(|inside| *inside > start) {
                open.pop();
            }
            outline.push(entry(row, open.len(), line.trim()));
            open.push(start + 1);
        }
        read_line(&mut scan, line);
    }
    outline
}

/// Whether `line` declares one of [`DECLARATIONS`], after its attributes
/// and modifiers (`class` among them, before `func` or `var`).
fn declares(line: &str) -> bool {
    let mut words = line.split_whitespace().peekable();
    while let Some(word) = words.next() {
        if word.starts_with('@') {
            continue;
        }
        let base = word.split('(').next().unwrap_or(word);
        if MODIFIERS.contains(&base) {
            continue;
        }
        let modifies = words
            .peek()
            .is_some_and(|next| matches!(*next, "func" | "var" | "let" | "subscript"));
        if base == "class" && modifies {
            continue;
        }
        let keyword = base.split(['<', '{', ':']).next().unwrap_or(base);
        return DECLARATIONS.contains(&keyword);
    }
    false
}

/// Move `scan` past `line`: its braces counted where they are code.
fn read_line(scan: &mut Scan, line: &str) {
    let chars: Vec<char> = line.chars().collect();
    let mut at = 0;
    let starts = |at: usize, text: &str| {
        text.chars()
            .enumerate()
            .all(|(off, c)| chars.get(at + off) == Some(&c))
    };
    while at < chars.len() {
        if scan.comments > 0 {
            if starts(at, "*/") {
                scan.comments -= 1;
                at += 2;
            } else if starts(at, "/*") {
                scan.comments += 1;
                at += 2;
            } else {
                at += 1;
            }
            continue;
        }
        if let Some(triple) = scan.string {
            if chars[at] == '\\' {
                at += 2;
            } else if triple && starts(at, "\"\"\"") {
                scan.string = None;
                at += 3;
            } else {
                if !triple && chars[at] == '"' {
                    scan.string = None;
                }
                at += 1;
            }
            continue;
        }
        if starts(at, "//") {
            break;
        }
        if starts(at, "/*") {
            scan.comments += 1;
            at += 2;
            continue;
        }
        if starts(at, "\"\"\"") {
            scan.string = Some(true);
            at += 3;
            continue;
        }
        match chars[at] {
            '"' => scan.string = Some(false),
            '{' => scan.depth += 1,
            '}' => scan.depth = scan.depth.saturating_sub(1),
            _ => {}
        }
        at += 1;
    }
    // A single-line string never runs past its line.
    if scan.string == Some(false) {
        scan.string = None;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_declarations_nest_by_their_braces_and_properties_and_text_are_left_out() {
        let doc = lines(concat!(
            "import Foundation\n",
            "@MainActor public final class Store: ObservableObject {\n",
            "    @Published var items: [String] = []\n",
            "    let note = \"func not { a decl\"\n",
            "    /* func hidden() { */\n",
            "    init() {}\n",
            "    class func make() -> Store { Store() }\n",
            "    private func load() async throws {\n",
            "        func helper() {}\n",
            "        let text = \"\"\"\n",
            "        func inside() {\n",
            "        \"\"\"\n",
            "    }\n",
            "}\n",
            "extension Store {\n",
            "    struct Item<T>: Codable {}\n",
            "}\n",
            "protocol Loading { func load() }\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    2  @MainActor public final class Store: ObservableObject {",
                "    6    init() {}",
                "    7    class func make() -> Store { Store() }",
                "    8    private func load() async throws {",
                "    9      func helper() {}",
                "   15  extension Store {",
                "   16    struct Item<T>: Codable {}",
                "   18  protocol Loading { func load() }",
            ]
        );
    }
}
