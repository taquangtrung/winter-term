//! Coloring source as it is read: which run of characters is a comment, a
//! string, a number, a keyword, or the name of a type, for the languages
//! named by a file's extension.
//!
//! A lexer rather than a parser, and a small one: it colors what can be told
//! from the characters themselves, gets out of the way for a file it does not
//! know, and never decides that something is wrong with the code.

use std::path::Path;

// ========================================================================
// Constants
// ========================================================================

/// How far a `'` may look for its partner before it is read as something
/// other than a quote. Rust's lifetimes and OCaml's type variables are a `'`
/// that opens nothing, and running a string from one to the end of the line
/// paints most of a file the color of a string.
const QUOTE_REACH: usize = 4;

/// The languages named by extension, and what each one spells its comments,
/// strings, keywords, and built-in types with.
const LANGUAGES: &[(&[&str], Syntax)] = &[
    (
        &["rs"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else",
                "enum", "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match",
                "mod", "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct",
                "super", "trait", "true", "type", "unsafe", "use", "where", "while",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str",
                "u8", "u16", "u32", "u64", "u128", "usize",
            ],
        },
    ),
    (
        &["py", "pyi", "pyw"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del",
                "elif", "else", "except", "finally", "for", "from", "global", "if", "import", "in",
                "is", "lambda", "match", "case", "None", "nonlocal", "not", "or", "pass", "raise",
                "return", "try", "while", "with", "yield", "False", "True", "self",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[
                "bool", "bytes", "dict", "float", "int", "list", "object", "set", "str", "tuple",
            ],
        },
    ),
    (
        &["go"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "break", "case", "chan", "const", "continue", "default", "defer", "else",
                "fallthrough", "false", "for", "func", "go", "goto", "if", "import", "interface",
                "map", "nil", "package", "range", "return", "select", "struct", "switch", "true",
                "type", "var",
            ],
            line: &["//"],
            quotes: &['"', '`', '\''],
            types: &[
                "any", "bool", "byte", "complex64", "complex128", "error", "float32", "float64",
                "int", "int8", "int16", "int32", "int64", "rune", "string", "uint", "uint8",
                "uint16", "uint32", "uint64", "uintptr",
            ],
        },
    ),
    (
        &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "as", "async", "await", "break", "case", "catch", "class", "const", "continue",
                "debugger", "default", "delete", "do", "else", "enum", "export", "extends", "false",
                "finally", "for", "from", "function", "get", "if", "implements", "import", "in",
                "instanceof", "interface", "keyof", "let", "new", "null", "of", "private",
                "protected", "public", "readonly", "return", "set", "static", "super", "switch",
                "this", "throw", "true", "try", "type", "typeof", "undefined", "var", "void",
                "while", "yield",
            ],
            line: &["//"],
            quotes: &['"', '\'', '`'],
            types: &[
                "any", "bigint", "boolean", "never", "number", "object", "string", "symbol",
                "unknown",
            ],
        },
    ),
    (
        &["c", "h", "cc", "cpp", "hpp", "cxx", "hh", "hxx", "ino", "m", "mm"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "auto", "break", "case", "catch", "class", "const", "constexpr", "continue",
                "default", "delete", "do", "else", "enum", "explicit", "extern", "false", "final",
                "for", "friend", "goto", "if", "inline", "namespace", "new", "noexcept", "nullptr",
                "operator", "override", "private", "protected", "public", "register", "return",
                "sizeof", "static", "static_cast", "struct", "switch", "template", "this", "throw",
                "true", "try", "typedef", "typename", "union", "using", "virtual", "volatile",
                "while",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "char", "double", "float", "int", "long", "short", "signed", "size_t",
                "unsigned", "void", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t",
                "uint16_t", "uint32_t", "uint64_t",
            ],
        },
    ),
    (
        &["cs"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "abstract", "as", "async", "await", "base", "break", "case", "catch", "class",
                "const", "continue", "default", "delegate", "do", "else", "enum", "event",
                "explicit", "extern", "false", "finally", "fixed", "for", "foreach", "get", "if",
                "implicit", "in", "interface", "internal", "is", "lock", "namespace", "new", "null",
                "operator", "out", "override", "params", "private", "protected", "public",
                "readonly", "record", "ref", "return", "sealed", "set", "sizeof", "static",
                "struct", "switch", "this", "throw", "true", "try", "typeof", "using", "var",
                "virtual", "void", "volatile", "when", "where", "while", "yield",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "byte", "char", "decimal", "double", "dynamic", "float", "int", "long",
                "object", "sbyte", "short", "string", "uint", "ulong", "ushort",
            ],
        },
    ),
    (
        &["java"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "abstract", "assert", "break", "case", "catch", "class", "const", "continue",
                "default", "do", "else", "enum", "extends", "false", "final", "finally", "for",
                "goto", "if", "implements", "import", "instanceof", "interface", "native", "new",
                "null", "package", "private", "protected", "public", "record", "return", "static",
                "strictfp", "super", "switch", "synchronized", "this", "throw", "throws",
                "transient", "true", "try", "var", "void", "volatile", "while", "yield",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "boolean", "byte", "char", "double", "float", "int", "long", "short",
            ],
        },
    ),
    (
        &["kt", "kts"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "as", "break", "class", "companion", "const", "continue", "data", "do", "else",
                "enum", "false", "for", "fun", "if", "import", "in", "interface", "is", "lateinit",
                "null", "object", "open", "override", "package", "private", "protected", "public",
                "return", "sealed", "super", "this", "throw", "true", "try", "typealias", "val",
                "var", "when", "while",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "Any", "Boolean", "Byte", "Char", "Double", "Float", "Int", "Long", "Nothing",
                "Short", "String", "Unit",
            ],
        },
    ),
    (
        &["scala", "sc"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "abstract", "case", "catch", "class", "def", "do", "else", "enum", "extends",
                "false", "final", "finally", "for", "given", "if", "implicit", "import", "lazy",
                "match", "new", "null", "object", "override", "package", "private", "protected",
                "return", "sealed", "super", "then", "this", "throw", "trait", "true", "try",
                "type", "val", "var", "while", "with", "yield",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "Any", "Boolean", "Byte", "Char", "Double", "Float", "Int", "Long", "Nothing",
                "Short", "String", "Unit",
            ],
        },
    ),
    (
        &["swift"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "as", "associatedtype", "break", "case", "catch", "class", "continue", "default",
                "defer", "deinit", "do", "else", "enum", "extension", "fallthrough", "false",
                "fileprivate", "for", "func", "guard", "if", "import", "in", "init", "inout",
                "internal", "is", "let", "nil", "open", "operator", "private", "protocol", "public",
                "repeat", "rethrows", "return", "self", "Self", "static", "struct", "subscript",
                "super", "switch", "throw", "throws", "true", "try", "typealias", "var", "where",
                "while",
            ],
            line: &["//"],
            quotes: &['"'],
            types: &[
                "Any", "Bool", "Character", "Double", "Float", "Int", "String", "UInt", "Void",
            ],
        },
    ),
    (
        &["dart"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "abstract", "as", "assert", "async", "await", "break", "case", "catch", "class",
                "const", "continue", "default", "do", "else", "enum", "export", "extends",
                "extension", "external", "factory", "false", "final", "finally", "for", "get", "if",
                "implements", "import", "in", "is", "late", "library", "mixin", "new", "null", "on",
                "part", "required", "rethrow", "return", "set", "static", "super", "switch", "this",
                "throw", "true", "try", "typedef", "var", "void", "while", "with", "yield",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "double", "dynamic", "int", "num", "String",
            ],
        },
    ),
    (
        &["zig"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "align", "allowzero", "and", "anyframe", "anytype", "asm", "async", "await",
                "break", "catch", "comptime", "const", "continue", "defer", "else", "enum",
                "errdefer", "error", "export", "extern", "false", "fn", "for", "if", "inline",
                "noalias", "nosuspend", "null", "or", "orelse", "packed", "pub", "resume", "return",
                "struct", "suspend", "switch", "test", "threadlocal", "true", "try", "undefined",
                "union", "unreachable", "usingnamespace", "var", "volatile", "while",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "f16", "f32", "f64", "i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32",
                "u64", "usize", "void", "anyerror",
            ],
        },
    ),
    (
        &["rb", "rake", "gemspec"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "alias", "and", "begin", "break", "case", "class", "def", "defined?", "do", "else",
                "elsif", "end", "ensure", "false", "for", "if", "in", "module", "next", "nil",
                "not", "or", "redo", "rescue", "retry", "return", "self", "super", "then", "true",
                "undef", "unless", "until", "when", "while", "yield", "require", "attr_accessor",
                "attr_reader",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["php"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "abstract", "and", "array", "as", "break", "case", "catch", "class", "clone",
                "const", "continue", "declare", "default", "do", "echo", "else", "elseif", "empty",
                "enum", "extends", "false", "final", "finally", "fn", "for", "foreach", "function",
                "global", "if", "implements", "include", "instanceof", "interface", "isset",
                "match", "namespace", "new", "null", "or", "private", "protected", "public",
                "readonly", "require", "return", "static", "switch", "this", "throw", "trait",
                "true", "try", "unset", "use", "var", "while", "yield",
            ],
            line: &["//", "#"],
            quotes: &['"', '\''],
            types: &[
                "bool", "float", "int", "iterable", "mixed", "object", "string", "void",
            ],
        },
    ),
    (
        &["lua"],
        Syntax {
            block: Some(("--[[", "]]")),
            fold_case: false,
            keywords: &[
                "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "goto",
                "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true",
                "until", "while",
            ],
            line: &["--"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["pl", "pm"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "else", "elsif", "for", "foreach", "if", "last", "local", "my", "next", "our",
                "package", "return", "sub", "unless", "until", "use", "while",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["r"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "break", "else", "FALSE", "for", "function", "if", "in", "Inf", "NA", "NaN", "next",
                "NULL", "repeat", "return", "TRUE", "while",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["jl"],
        Syntax {
            block: Some(("#=", "=#")),
            fold_case: false,
            keywords: &[
                "abstract", "begin", "break", "catch", "const", "continue", "do", "else", "elseif",
                "end", "export", "false", "finally", "for", "function", "global", "if", "import",
                "let", "local", "macro", "module", "mutable", "nothing", "quote", "return",
                "struct", "true", "try", "using", "while",
            ],
            line: &["#"],
            quotes: &['"'],
            types: &[
                "Any", "Bool", "Float64", "Int", "Int64", "String",
            ],
        },
    ),
    (
        &["ex", "exs"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "after", "and", "case", "catch", "cond", "def", "defmacro", "defmodule", "defp",
                "defstruct", "do", "else", "end", "false", "fn", "for", "if", "import", "in", "nil",
                "not", "or", "quote", "raise", "receive", "require", "rescue", "true", "try",
                "unless", "use", "when", "with",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["erl", "hrl"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "after", "and", "andalso", "begin", "case", "catch", "end", "fun", "if", "not",
                "of", "or", "orelse", "receive", "try", "when",
            ],
            line: &["%"],
            quotes: &['"'],
            types: &[],
        },
    ),
    (
        &["hs", "lhs"],
        Syntax {
            block: Some(("{-", "-}")),
            fold_case: false,
            keywords: &[
                "case", "class", "data", "default", "deriving", "do", "else", "if", "import", "in",
                "infix", "infixl", "infixr", "instance", "let", "module", "newtype", "of", "then",
                "type", "where",
            ],
            line: &["--"],
            quotes: &['"', '\''],
            types: &[
                "Bool", "Char", "Double", "Float", "Int", "Integer", "IO", "Maybe", "String",
            ],
        },
    ),
    (
        &["ml", "mli"],
        Syntax {
            block: Some(("(*", "*)")),
            fold_case: false,
            keywords: &[
                "and", "as", "begin", "class", "do", "done", "downto", "else", "end", "exception",
                "external", "false", "for", "fun", "function", "functor", "if", "in", "include",
                "let", "match", "method", "module", "mutable", "new", "object", "of", "open", "or",
                "private", "rec", "sig", "struct", "then", "to", "true", "try", "type", "val",
                "virtual", "when", "while", "with",
            ],
            line: &[],
            quotes: &['"', '\''],
            types: &[
                "bool", "char", "float", "int", "list", "option", "string", "unit",
            ],
        },
    ),
    (
        &["clj", "cljs", "cljc", "edn", "lisp", "el", "scm", "rkt"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "def", "defn", "defmacro", "defun", "defvar", "defparameter", "define", "fn",
                "lambda", "let", "let*", "if", "cond", "when", "unless", "do", "loop", "recur",
                "nil", "true", "false",
            ],
            line: &[";"],
            quotes: &['"'],
            types: &[],
        },
    ),
    (
        &["nim"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "and", "break", "case", "const", "continue", "discard", "elif", "else", "end",
                "enum", "except", "export", "finally", "for", "from", "func", "if", "import", "in",
                "include", "is", "iterator", "let", "macro", "method", "mod", "nil", "not",
                "object", "of", "or", "proc", "raise", "ref", "return", "template", "try", "tuple",
                "type", "var", "when", "while", "yield",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[
                "bool", "char", "float", "int", "seq", "string",
            ],
        },
    ),
    (
        &["sql"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: true,
            keywords: &[
                "add", "all", "alter", "and", "as", "asc", "begin", "between", "by", "case",
                "check", "column", "commit", "constraint", "create", "cross", "database", "default",
                "delete", "desc", "distinct", "drop", "else", "end", "exists", "foreign", "from",
                "full", "group", "having", "if", "in", "index", "inner", "insert", "into", "is",
                "join", "key", "left", "like", "limit", "not", "null", "offset", "on", "or",
                "order", "outer", "primary", "references", "right", "rollback", "select", "set",
                "table", "then", "union", "unique", "update", "values", "view", "when", "where",
                "with",
            ],
            line: &["--"],
            quotes: &['\'', '"'],
            types: &[
                "bigint", "boolean", "char", "date", "decimal", "float", "int", "integer",
                "numeric", "real", "serial", "text", "timestamp", "varchar",
            ],
        },
    ),
    (
        &["sh", "bash", "zsh", "ksh", "fish", "envrc"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "case", "do", "done", "elif", "else", "esac", "exit", "export", "fi", "for",
                "function", "if", "in", "local", "readonly", "return", "set", "shift", "source",
                "then", "trap", "unset", "until", "while",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["ps1", "psm1"],
        Syntax {
            block: Some(("<#", "#>")),
            fold_case: false,
            keywords: &[
                "begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end",
                "exit", "filter", "finally", "for", "foreach", "function", "if", "in", "param",
                "process", "return", "switch", "throw", "trap", "try", "until", "while",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["vim"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "autocmd", "call", "else", "elseif", "endfor", "endfunction", "endif", "endwhile",
                "for", "function", "if", "let", "map", "nnoremap", "noremap", "return", "set",
                "while",
            ],
            line: &["\""],
            quotes: &['\''],
            types: &[],
        },
    ),
    (
        &["css", "less"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "important", "media", "import", "keyframes", "supports",
            ],
            line: &[],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["scss", "sass"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "important", "media", "import", "include", "mixin", "extend", "if", "else", "each",
                "for",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["html", "htm", "xml", "xhtml", "svg", "plist", "xsd", "xsl", "vue", "svelte"],
        Syntax {
            block: Some(("<!--", "-->")),
            fold_case: false,
            keywords: &[],
            line: &[],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["tex", "sty", "cls", "bib"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[],
            line: &["%"],
            quotes: &[],
            types: &[],
        },
    ),
    (
        &["json", "geojson", "webmanifest"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "false", "null", "true",
            ],
            line: &[],
            quotes: &['"'],
            types: &[],
        },
    ),
    (
        &["jsonc", "json5"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "false", "null", "true",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["yaml", "yml"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "false", "no", "null", "off", "on", "true", "yes",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["toml", "kdl", "ini", "cfg", "conf", "properties", "env", "editorconfig", "gitconfig", "service", "desktop"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "false", "null", "true",
            ],
            line: &["#", ";", "//"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["nix"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "assert", "else", "false", "if", "import", "in", "inherit", "let", "null", "or",
                "rec", "then", "true", "with",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["tf", "tfvars", "hcl"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "data", "false", "locals", "module", "null", "output", "provider", "resource",
                "terraform", "true", "variable",
            ],
            line: &["#", "//"],
            quotes: &['"'],
            types: &[],
        },
    ),
    (
        &["proto"],
        Syntax {
            block: Some(("/*", "*/")),
            fold_case: false,
            keywords: &[
                "enum", "false", "import", "map", "message", "oneof", "option", "optional",
                "package", "repeated", "reserved", "returns", "rpc", "service", "syntax", "true",
            ],
            line: &["//"],
            quotes: &['"', '\''],
            types: &[
                "bool", "bytes", "double", "fixed32", "fixed64", "float", "int32", "int64",
                "sint32", "sint64", "string", "uint32", "uint64",
            ],
        },
    ),
    (
        &["graphql", "gql"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "enum", "extend", "fragment", "implements", "input", "interface", "mutation",
                "query", "scalar", "schema", "subscription", "type", "union",
            ],
            line: &["#"],
            quotes: &['"'],
            types: &[
                "Boolean", "Float", "ID", "Int", "String",
            ],
        },
    ),
    (
        &["dockerfile", "containerfile"],
        Syntax {
            block: None,
            fold_case: true,
            keywords: &[
                "add", "arg", "cmd", "copy", "entrypoint", "env", "expose", "from", "healthcheck",
                "label", "maintainer", "onbuild", "run", "shell", "stopsignal", "user", "volume",
                "workdir", "as",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["mk", "mak", "make"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[
                "define", "else", "endef", "endif", "export", "ifdef", "ifeq", "ifndef", "ifneq",
                "include", "override",
            ],
            line: &["#"],
            quotes: &['"', '\''],
            types: &[],
        },
    ),
    (
        &["cmake"],
        Syntax {
            block: None,
            fold_case: true,
            keywords: &[
                "add_executable", "add_library", "else", "elseif", "endforeach", "endfunction",
                "endif", "endmacro", "endwhile", "foreach", "function", "if", "include", "macro",
                "option", "project", "set", "target_link_libraries", "while",
            ],
            line: &["#"],
            quotes: &['"'],
            types: &[],
        },
    ),
    (
        &["gitignore", "dockerignore", "npmignore", "ignore"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[],
            line: &["#"],
            quotes: &[],
            types: &[],
        },
    ),
    (
        &["diff", "patch"],
        Syntax {
            block: None,
            fold_case: false,
            keywords: &[],
            line: &[],
            quotes: &[],
            types: &[],
        },
    ),
];

/// Files whose name, rather than an extension, says what they are: the
/// extension in [`LANGUAGES`] each one is read as.
const FILENAMES: &[(&str, &str)] = &[
    (".bash_profile", "sh"),
    (".bashrc", "sh"),
    (".dockerignore", "dockerignore"),
    (".editorconfig", "editorconfig"),
    (".env", "env"),
    (".envrc", "envrc"),
    (".gitattributes", "gitignore"),
    (".gitconfig", "gitconfig"),
    (".gitignore", "gitignore"),
    (".gitmodules", "gitconfig"),
    (".npmrc", "ini"),
    (".profile", "sh"),
    (".vimrc", "vim"),
    (".zprofile", "sh"),
    (".zshenv", "sh"),
    (".zshrc", "sh"),
    ("BUILD", "py"),
    ("CMakeLists.txt", "cmake"),
    ("Cargo.lock", "toml"),
    ("Containerfile", "dockerfile"),
    ("Dockerfile", "dockerfile"),
    ("GNUmakefile", "mk"),
    ("Gemfile", "rb"),
    ("Justfile", "mk"),
    ("Makefile", "mk"),
    ("Pipfile", "toml"),
    ("Rakefile", "rb"),
    ("Vagrantfile", "rb"),
    ("flake.lock", "json"),
    ("justfile", "mk"),
    ("makefile", "mk"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// One language's spelling of the things worth coloring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Syntax {
    /// What opens and closes a comment that may run past the line it starts
    /// on, for a language that has one.
    block: Option<(&'static str, &'static str)>,
    /// Whether keywords match whatever their case, as SQL's do.
    fold_case: bool,
    /// Words that are the language rather than the program.
    keywords: &'static [&'static str],
    /// What opens a comment that runs to the end of the line.
    line: &'static [&'static str],
    /// What opens, and closes, a string.
    quotes: &'static [char],
    /// Words naming a type the language already has. Anything else opening
    /// with a capital is taken for a type as well, which is the convention
    /// every language here is written in.
    types: &'static [&'static str],
}

/// What one line of a file was colored as, and what the next line inherits.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Highlighted {
    /// What the line after this one begins inside.
    pub next: LineState,
    /// What each character of the line is, one entry per character.
    pub tokens: Vec<Token>,
}

/// Whether a line begins inside something an earlier line opened.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LineState {
    /// Inside a comment that has not been closed yet.
    Comment,
    /// Ordinary code.
    #[default]
    Code,
}

/// What a character is, in the vocabulary a page paints.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Token {
    /// Prose about the code rather than the code.
    Comment,
    /// A word of the language rather than of the program.
    Keyword,
    /// Anything the lexer has nothing to say about.
    #[default]
    Normal,
    /// A literal number.
    Number,
    /// A literal string, quotes included.
    String,
    /// The name of a type.
    Type,
}

// ========================================================================
// Syntax
// ========================================================================

impl Syntax {
    /// What opens and closes a comment that may run past its own line. A line
    /// holding neither leaves the next one exactly as it found it, whatever
    /// else is written on it, which is what makes most of a file skippable
    /// when all that is wanted is what it leaves open.
    pub fn block(&self) -> Option<(&'static str, &'static str)> {
        self.block
    }

    /// What opens a comment that runs to the end of the line, for a language
    /// that has one: what commenting a line out puts in front of it.
    pub fn line_comment(&self) -> Option<&'static str> {
        self.line.first().copied()
    }

    /// Whether anything in the language can run past the line it starts on,
    /// which is the only reason a line has to know what precedes it.
    pub fn spans_lines(&self) -> bool {
        self.block.is_some()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The language `path`'s extension names, or `None` for a file whose language
/// this does not know, which is then painted as the plain text it may well be.
pub fn of_path(path: &Path) -> Option<&'static Syntax> {
    let name = path.file_name()?.to_str()?;
    let extension = match FILENAMES.iter().find(|(file, _)| *file == name) {
        Some((_, extension)) => extension.to_string(),
        None => path.extension()?.to_str()?.to_ascii_lowercase(),
    };
    LANGUAGES
        .iter()
        .find(|(names, _)| names.contains(&extension.as_str()))
        .map(|(_, syntax)| syntax)
}

/// Color one line, given what the line before it left open.
pub fn highlight(line: &[char], syntax: &Syntax, state: LineState) -> Highlighted {
    let mut tokens = vec![Token::Normal; line.len()];
    let next = scan(line, syntax, state, Some(&mut tokens));
    Highlighted { next, tokens }
}

/// What a line leaves open for the next one, without working out its colors.
/// The same walk as [`highlight`], reading rather than writing, for the lines
/// between the top of a file and the part of it being looked at: they are
/// only worth what they say about the lines below them.
pub fn next_state(line: &[char], syntax: &Syntax, state: LineState) -> LineState {
    scan(line, syntax, state, None)
}

/// Walk a line, coloring it where somewhere to put the colors is given. One
/// walk rather than two, so what a line leaves open cannot come out different
/// from what it was painted as.
fn scan(
    line: &[char],
    syntax: &Syntax,
    state: LineState,
    mut tokens: Option<&mut Vec<Token>>,
) -> LineState {
    let mut at = 0;
    let mut state = state;
    while at < line.len() {
        if state == LineState::Comment {
            let closer = syntax.block.map(|(_, close)| close).unwrap_or_default();
            match find_from(line, at, closer) {
                Some(end) => {
                    paint(
                        &mut tokens,
                        at,
                        end + closer.chars().count(),
                        Token::Comment,
                    );
                    at = end + closer.chars().count();
                    state = LineState::Code;
                }
                None => {
                    paint(&mut tokens, at, line.len(), Token::Comment);
                    return LineState::Comment;
                }
            }
            continue;
        }
        // A block opener is tried before the line markers, since in some
        // languages one starts with the other: Lua's `--[[` and `--`.
        if let Some((open, close)) = syntax.block {
            if starts_at(line, at, open) {
                let from = at + open.chars().count();
                match find_from(line, from, close) {
                    Some(end) => {
                        let stop = end + close.chars().count();
                        paint(&mut tokens, at, stop, Token::Comment);
                        at = stop;
                    }
                    None => {
                        paint(&mut tokens, at, line.len(), Token::Comment);
                        return LineState::Comment;
                    }
                }
                continue;
            }
        }
        if syntax.line.iter().any(|mark| starts_at(line, at, mark)) {
            paint(&mut tokens, at, line.len(), Token::Comment);
            break;
        }
        let c = line[at];
        if syntax.quotes.contains(&c) {
            match string_end(line, at, c) {
                Some(end) => {
                    paint(&mut tokens, at, end, Token::String);
                    at = end;
                }
                // An unterminated double quote is a string still being typed;
                // an unterminated `'` is a lifetime or an apostrophe, and is
                // left as the one ordinary character it is.
                None if c == '\'' => at += 1,
                None => {
                    paint(&mut tokens, at, line.len(), Token::String);
                    break;
                }
            }
            continue;
        }
        if c.is_ascii_digit() && !at_word_tail(line, at) {
            let end = number_end(line, at);
            paint(&mut tokens, at, end, Token::Number);
            at = end;
            continue;
        }
        if is_word_start(c) {
            let end = word_end(line, at);
            // What a word is only matters where the colors are wanted: it is
            // the one place a line's own contents are looked up in a list.
            if tokens.is_some() {
                let word: String = line[at..end].iter().collect();
                paint(&mut tokens, at, end, classify(&word, syntax));
            }
            at = end;
            continue;
        }
        at += 1;
    }
    state
}

/// What a word is: the language's own, one of its types, a name written the
/// way types are written, or nothing in particular.
fn classify(word: &str, syntax: &Syntax) -> Token {
    let keyword = match syntax.fold_case {
        true => syntax.keywords.contains(&word.to_ascii_lowercase().as_str()),
        false => syntax.keywords.contains(&word),
    };
    if keyword {
        return Token::Keyword;
    }
    if syntax.types.contains(&word) {
        return Token::Type;
    }
    let capitalized = word.chars().next().is_some_and(char::is_uppercase);
    // A run of capitals is a constant rather than a type: `MAX_UNDO` names a
    // value, and coloring it as a type says something untrue about it.
    let shouting = word
        .chars()
        .all(|c| c.is_uppercase() || c.is_ascii_digit() || c == '_');
    match capitalized && !shouting {
        true => Token::Type,
        false => Token::Normal,
    }
}

/// Where the string opened at `at` ends, one past its closing quote, or `None`
/// when the line ends first. A backslash escapes whatever follows it, so an
/// escaped quote does not close the string.
fn string_end(line: &[char], at: usize, quote: char) -> Option<usize> {
    let mut index = at + 1;
    // A `'` is only a quote when its partner is close enough to be one.
    let reach = match quote {
        '\'' => (index + QUOTE_REACH).min(line.len()),
        _ => line.len(),
    };
    while index < reach {
        match line[index] {
            '\\' => index += 2,
            c if c == quote => return Some(index + 1),
            _ => index += 1,
        }
    }
    None
}

/// Where the number starting at `at` ends. Digits, the separators and radix
/// letters a literal may carry, and one trailing type suffix: near enough for
/// every language here without parsing any of them.
fn number_end(line: &[char], at: usize) -> usize {
    let mut index = at;
    while index < line.len() {
        let c = line[index];
        let part = c.is_ascii_alphanumeric() || c == '_';
        // A dot belongs to the number only with a digit after it, so the `.`
        // of a method call ends the literal rather than joining it.
        let decimal = c == '.' && line.get(index + 1).is_some_and(char::is_ascii_digit);
        if !part && !decimal {
            break;
        }
        index += 1;
    }
    index
}

/// Where the word starting at `at` ends.
fn word_end(line: &[char], at: usize) -> usize {
    let mut index = at;
    while index < line.len() && is_word_char(line[index]) {
        index += 1;
    }
    index
}

/// Whether `at` sits inside a word rather than at its start, which is what
/// keeps the `1` of `utf8_len` from being read as a number.
fn at_word_tail(line: &[char], at: usize) -> bool {
    at > 0 && is_word_char(line[at - 1])
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `needle` is written at `at`.
fn starts_at(line: &[char], at: usize, needle: &str) -> bool {
    needle
        .chars()
        .enumerate()
        .all(|(offset, c)| line.get(at + offset) == Some(&c))
}

/// Where `needle` next appears at or after `at`.
fn find_from(line: &[char], at: usize, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    (at..line.len()).find(|&index| starts_at(line, index, needle))
}

/// Mark every character of a half-open range as one thing, where anything is
/// keeping the marks.
fn paint(tokens: &mut Option<&mut Vec<Token>>, from: usize, to: usize, token: Token) {
    let Some(tokens) = tokens else {
        return;
    };
    let to = to.min(tokens.len());
    for slot in &mut tokens[from.min(to)..to] {
        *slot = token;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The tokens of `line` in `syntax`, as one letter per character, so a
    /// test reads as the line it is about: `c`omment, `k`eyword, `n`umber,
    /// `s`tring, `t`ype, and `.` for everything else, spaces included.
    fn marks(line: &str, syntax: &Syntax, state: LineState) -> (String, LineState) {
        let chars: Vec<char> = line.chars().collect();
        let out = highlight(&chars, syntax, state);
        let text = out
            .tokens
            .iter()
            .map(|token| match token {
                Token::Comment => 'c',
                Token::Keyword => 'k',
                Token::Normal => '.',
                Token::Number => 'n',
                Token::String => 's',
                Token::Type => 't',
            })
            .collect();
        (text, out.next)
    }

    fn rust() -> &'static Syntax {
        of_path(Path::new("a.rs")).expect("rust")
    }

    #[test]
    fn test_a_comment_marker_inside_a_string_does_not_open_a_comment() {
        // Scanning for `//` without reading the string first greys out the
        // rest of every line holding a URL.
        let (marks, _) = marks(r#"let u = "http://x"; // go"#, rust(), LineState::Code);
        assert_eq!(marks, "kkk.....ssssssssss..ccccc");
    }

    #[test]
    fn test_a_block_comment_carries_to_the_lines_under_it_and_closes() {
        let (first, state) = marks("code /* open", rust(), LineState::Code);
        assert_eq!(first, ".....ccccccc");
        assert_eq!(state, LineState::Comment, "the next line is inside it");

        let (middle, state) = marks("still inside", rust(), state);
        assert_eq!(middle, "cccccccccccc");
        assert_eq!(state, LineState::Comment);

        let (last, state) = marks("done */ code", rust(), state);
        assert_eq!(last, "ccccccc.....");
        assert_eq!(state, LineState::Code, "and code follows it again");
    }

    #[test]
    fn test_a_lifetime_is_not_a_string_that_never_ends() {
        // A `'` read as a quote with no partner paints the rest of the line,
        // and most of the rest of the file, the color of a string.
        let (lifetime, state) = marks("fn f<'a>(x: &'a str) {", rust(), LineState::Code);
        assert!(
            !lifetime.contains('s'),
            "nothing here is a string, got {lifetime}"
        );
        assert_eq!(state, LineState::Code);

        // A character literal is short enough to still be one.
        let (literal, _) = marks("let c = 'x';", rust(), LineState::Code);
        assert_eq!(literal, "kkk.....sss.");
    }

    #[test]
    fn test_an_escaped_quote_does_not_close_the_string() {
        let (marks, _) = marks(r#"("a\"b") x"#, rust(), LineState::Code);
        assert_eq!(marks, ".ssssss...");
    }

    #[test]
    fn test_digits_inside_a_name_are_part_of_the_name() {
        // `utf8_len` and `i32` are words, not a word with a number in it.
        let (marks, _) = marks("utf8_len + 42", rust(), LineState::Code);
        assert_eq!(marks, "...........nn");
    }

    #[test]
    fn test_a_shouting_name_is_a_constant_rather_than_a_type() {
        // Capitalized reads as a type, but a name in full capitals is a value
        // and saying otherwise is saying something untrue about it.
        let (marks, _) = marks("MAX_UNDO = Buffer", rust(), LineState::Code);
        assert_eq!(marks, "...........tttttt");
    }

    #[test]
    fn test_a_language_is_chosen_by_extension_and_an_unknown_one_is_left_alone() {
        assert!(of_path(Path::new("src/main.rs")).is_some());
        assert!(of_path(Path::new("script.PY")).is_some(), "case is not it");
        assert!(of_path(Path::new("notes.txt")).is_none());
        assert!(of_path(Path::new("Makefile")).is_some(), "named, not extended");
        assert!(of_path(Path::new("/home/me/.bashrc")).is_some());
    }

    #[test]
    fn test_the_common_languages_and_config_files_are_all_known() {
        for name in [
            "a.java", "a.kt", "a.cs", "a.swift", "a.rb", "a.php", "a.lua", "a.hs", "a.ml",
            "a.sql", "a.yaml", "a.yml", "a.xml", "a.html", "a.css", "a.scss", "a.nix", "a.tf",
            "a.proto", "a.ps1", "a.ex", "a.zig", "a.dart", "a.scala", "a.jsonc", "Dockerfile",
            "CMakeLists.txt", ".gitignore", ".env",
        ] {
            assert!(of_path(Path::new(name)).is_some(), "{name} is colored");
        }
    }

    #[test]
    fn test_a_block_opener_that_starts_like_a_line_comment_still_opens_a_block() {
        let lua = of_path(Path::new("a.lua")).expect("lua");
        let (_, state) = marks("x = 1 --[[ open", lua, LineState::Code);
        assert_eq!(state, LineState::Comment, "the block runs on to the next line");
        let (line, state) = marks("x = 1 -- done", lua, LineState::Code);
        assert_eq!(line, "....n.ccccccc");
        assert_eq!(state, LineState::Code);
    }

    #[test]
    fn test_sql_keywords_match_whatever_their_case() {
        let sql = of_path(Path::new("q.sql")).expect("sql");
        let (marks, _) = marks("SELECT x from t", sql, LineState::Code);
        assert_eq!(marks, "kkkkkk...kkkk..");
    }

    #[test]
    fn test_a_number_ends_where_a_method_call_begins() {
        // `1.max(2)` is a number and a call, not one long number.
        let (marks, _) = marks("1.max(2) 1.5", rust(), LineState::Code);
        assert_eq!(marks, "n.....n..nnn");
    }
}
