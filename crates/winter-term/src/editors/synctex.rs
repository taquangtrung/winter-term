//! SyncTeX both ways, the way magic-vscode goes: backward from a spot on a
//! page to the line of LaTeX that set it, and forward from a line of LaTeX
//! to where it was set.
//!
//! Forward, `synctex view` names the boxes a line was set in, and the PDF it
//! asks about is the one built from the document's root: a `% !TEX root`
//! comment names it, else the file declaring `\documentclass` (of several in
//! one directory, the one whose last build read the file, else the one with
//! the newest PDF). The PDF is where latexmk was told to write it
//! (`$out_dir`, else `$aux_dir`, in the project's `.latexmkrc`), else beside
//! the root. From a `.bib` file, the line searched is a `\cite` of the entry
//! under the cursor.
//!
//! `synctex edit` names a source file and a line, but only as closely as TeX
//! recorded them: often a paragraph's first line, or the `\input` line a whole
//! file came in by. The text either side of the spot narrows that down: the
//! source line holding it nearest the one SyncTeX named, and failing that, a
//! line of one of the files included near there.

use std::fs;
use std::path::{Component, Path, PathBuf};

use regex::Regex;

use crate::model::editor::{CommandRequest, PdfMark};

// ========================================================================
// Constants
// ========================================================================

/// The tag a `synctex edit` run answers under.
pub const TAG_EDIT: &str = "synctex-edit";

/// The tag a `synctex view` run answers under.
pub const TAG_VIEW: &str = "synctex-view";

/// The program SyncTeX ships as.
const PROGRAM: &str = "synctex";

/// The fields of `synctex edit`'s answer this reads.
const INPUT_FIELD: &str = "Input:";
const LINE_FIELD: &str = "Line:";

/// The fields of `synctex view`'s answer this reads: a record's page, then
/// its box's left edge and baseline, width, and height above the baseline.
const PAGE_FIELD: &str = "Page:";
const LEFT_FIELD: &str = "h:";
const BASELINE_FIELD: &str = "v:";
const WIDTH_FIELD: &str = "W:";
const HEIGHT_FIELD: &str = "H:";

/// The comment naming a document's root from a file it includes, as
/// `% !TEX root = main.tex`, matched without regard to case or spacing.
const ROOT_PATTERN: &str = r"(?i)^\s*%\s*!\s*TEX\s+root\s*=\s*(.+?)\s*$";

/// latexmk's own settings files, read in a document's directory for where
/// it writes the PDF.
const LATEXMKRC_NAMES: &[&str] = &[".latexmkrc", "latexmkrc"];

/// The settings naming latexmk's output directory, the first found winning:
/// `$out_dir = 'build';`, else `$aux_dir`, which `$out_dir` defaults to.
const OUT_DIR_PATTERNS: &[&str] = &[
    r#"\$out_dir\s*=\s*['"]([^'"]*)['"]"#,
    r#"\$aux_dir\s*=\s*['"]([^'"]*)['"]"#,
];

/// The extension of the list of files a build read, written beside its PDF.
const FLS_EXTENSION: &str = "fls";

/// What a line of a `.fls` list starts with for a file the build read.
const FLS_INPUT: &str = "INPUT ";

/// The extension of a BibTeX database.
const BIB_EXTENSION: &str = "bib";

/// The start of a BibTeX entry and its key: `@article{smith2020,`.
const BIB_ENTRY_PATTERN: &str = r"^\s*@\s*(\w+)\s*[{(]\s*([^,\s]+)\s*,";

/// A citation and the keys it cites: `\cite{a,b}`, `\citep[p.~3]{a}`, and
/// biblatex's `\parencite`, `\textcite`, and the like.
const CITE_PATTERN: &str = r"\\[A-Za-z]*cite[A-Za-z]*\*?(?:\[[^\]]*\])*\{([^}]*)\}";

/// How many `.tex` files of a project are read for a citation, at most.
const CITE_SEARCH_FILES: usize = 2000;

/// How many directories below a document the citation search goes.
const CITE_SEARCH_DEPTH: usize = 6;

/// How many directory entries the citation search looks at, at most.
const CITE_SEARCH_ENTRIES: usize = 20_000;

/// The directories a search for a citation never goes into.
const SKIPPED_DIRS: &[&str] = &[".git", "node_modules", "target", "_build", "build"];

/// The lines at a file's head searched for its root comment.
const ROOT_COMMENT_LINES: usize = 20;

/// What a document's root declares.
const DOCUMENT_CLASS: &str = "\\documentclass";

/// How many directories up from a file its root is looked for.
const ROOT_SEARCH_DEPTH: usize = 3;

/// Words of context the looser match keeps, for text found whole on no one
/// line: a line break or a macro in the source splits it there.
const SHORT_WORDS: usize = 3;

/// Lines either side of SyncTeX's line searched for a file it includes.
const INCLUDE_RADIUS: usize = 5;

/// The extensions SyncTeX's data for a PDF takes beside it, packed and not.
const DATA_EXTENSIONS: &[&str] = &["synctex.gz", "synctex"];

/// What a LaTeX comment line starts with; its text is never what was set.
const COMMENT: char = '%';

/// The extension an included name may leave off.
const TEX_EXTENSION: &str = "tex";

/// The commands bringing in another file: `\input{..}`, `\include{..}`,
/// `\subfile{..}`, and `\import{dir}{..}` with its starred and sub forms.
const INCLUDE_PATTERN: &str =
    r"\\(?:input|include|subfile|subimport\*?\{[^}]*\}|import\*?\{[^}]*\})\*?\s*\{([^}]+)\}";

/// `\input` with its name bare, TeX's own form.
const BARE_INPUT_PATTERN: &str = r"\\input\s+([^\s%{}]+)";

/// The line that sets a title block, whose text SyncTeX places on it though
/// it often comes from files included anywhere in the document.
const MAKETITLE: &str = "\\maketitle";

// ========================================================================
// Data Structures
// ========================================================================

/// A spot on a page, the way SyncTeX takes one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spot {
    /// The page, from zero.
    pub page: usize,
    /// Points from the page's left edge.
    pub x: f32,
    /// Points down from the page's top edge.
    pub y: f32,
}

/// The text either side of a spot, along its line of the page.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Context {
    /// From the spot to the line's end.
    pub after: String,
    /// From the line's start to the spot.
    pub before: String,
}

/// A line of a source file, from one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Source {
    /// The line, counting from one.
    pub line: usize,
    /// The file the line is in.
    pub path: PathBuf,
}

// ========================================================================
// Context
// ========================================================================

impl Context {
    /// Whether there is any text to narrow SyncTeX's line down with.
    pub fn is_empty(&self) -> bool {
        self.before.trim().is_empty() && self.after.trim().is_empty()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `path` is a LaTeX source, by its extension.
pub fn is_tex(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == TEX_EXTENSION)
}

/// The directory where SyncTeX data for `pdf` lies, if not beside the PDF
/// itself: latexmk's `$out_dir` or `$aux_dir`.
pub fn synctex_data_dir(pdf: &Path) -> Option<PathBuf> {
    if DATA_EXTENSIONS
        .iter()
        .any(|ext| pdf.with_extension(ext).exists())
    {
        return None;
    }
    let root = root_of(pdf);
    for ext in DATA_EXTENSIONS {
        let candidate = artifact_of(&root, ext);
        if candidate.exists() {
            return candidate.parent().map(Path::to_path_buf);
        }
    }
    None
}

/// Whether SyncTeX's data for `pdf` lies beside it or in its build directory:
/// a PDF built without it, or not by TeX at all, has no source to go back to.
pub fn has_data(pdf: &Path) -> bool {
    DATA_EXTENSIONS
        .iter()
        .any(|ext| pdf.with_extension(ext).exists())
        || synctex_data_dir(pdf).is_some()
}

/// The `synctex view` run asking where line `line`, column `column` (both
/// from one) of `source` was set in `pdf`.
pub fn view_request(source: &Path, line: usize, column: usize, pdf: &Path) -> CommandRequest {
    let pdf_dir = pdf
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let mut args = vec![
        "view".to_string(),
        "-i".to_string(),
        format!("{line}:{column}:{}", source.display()),
        "-o".to_string(),
        pdf.display().to_string(),
    ];
    if let Some(dir) = synctex_data_dir(pdf) {
        args.push("-d".to_string());
        args.push(dir.display().to_string());
    }
    CommandRequest {
        id: 0,
        args,
        cwd: pdf_dir,
        env: Vec::new(),
        program: PROGRAM.to_string(),
        stdin: None,
        tag: TAG_VIEW,
    }
}

/// The boxes `synctex view` named in `stdout`, first page first and down
/// each page: a record missing any of its fields is skipped.
pub fn parse_view(stdout: &str) -> Vec<PdfMark> {
    let mut marks = Vec::new();
    let mut page: Option<usize> = None;
    let mut fields: [Option<f32>; 4] = [None; 4];
    let mut close = |page: Option<usize>, fields: &[Option<f32>; 4]| {
        if let (Some(page), [Some(left), Some(baseline), Some(width), Some(height)]) =
            (page, *fields)
        {
            marks.push(PdfMark {
                height,
                left,
                page: page.saturating_sub(1),
                top: baseline - height,
                width,
            });
        }
    };
    for line in stdout.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix(PAGE_FIELD) {
            close(page, &fields);
            page = value.trim().parse().ok();
            fields = [None; 4];
            continue;
        }
        let names = [LEFT_FIELD, BASELINE_FIELD, WIDTH_FIELD, HEIGHT_FIELD];
        for (slot, name) in fields.iter_mut().zip(names) {
            if let Some(value) = line.strip_prefix(name) {
                *slot = value.trim().parse().ok();
            }
        }
    }
    close(page, &fields);
    marks.sort_by(|a, b| a.page.cmp(&b.page).then(a.top.total_cmp(&b.top)));
    marks
}

/// The PDF built from the document `source` belongs to, where it has been
/// built: its root's name with `.pdf`, in latexmk's output directory where
/// the project names one, else beside the root.
pub fn pdf_for(source: &Path) -> Option<PathBuf> {
    let root = root_of(source);
    let pdf = [artifact_of(&root, "pdf"), root.with_extension("pdf")]
        .into_iter()
        .find(|pdf| pdf.exists())?;
    Some(pdf.canonicalize().unwrap_or(pdf))
}

/// Where a build of `root` writes the file with extension `ext`: in
/// latexmk's output directory where the project names one, else beside it.
fn artifact_of(root: &Path, ext: &str) -> PathBuf {
    let dir = root.parent().unwrap_or(Path::new(""));
    let name = root.with_extension(ext);
    let name = name.file_name().unwrap_or_default();
    match out_dir(dir) {
        Some(out) => normalized(&dir.join(out)).join(name),
        None => dir.join(name),
    }
}

/// The output directory `dir`'s latexmk settings name, if any.
fn out_dir(dir: &Path) -> Option<String> {
    LATEXMKRC_NAMES.iter().find_map(|name| {
        let text = fs::read_to_string(dir.join(name)).ok()?;
        OUT_DIR_PATTERNS.iter().find_map(|pattern| {
            let found = Regex::new(pattern).ok()?.captures(&text)?.get(1)?.as_str().trim().to_string();
            (!found.is_empty()).then_some(found)
        })
    })
}

/// The root of the document `source` belongs to: the file its root comment
/// names, else itself where it declares the class, else a file that does
/// in its directory or one above, else itself.
pub fn root_of(source: &Path) -> PathBuf {
    let content = fs::read_to_string(source).unwrap_or_default();
    let dir = source.parent().unwrap_or(Path::new(""));
    if let Some(root) = root_comment(&content) {
        let path = normalized(&dir.join(root));
        return match path.extension() {
            Some(_) => path,
            None => path.with_extension(TEX_EXTENSION),
        };
    }
    if content.contains(DOCUMENT_CLASS) {
        return source.to_path_buf();
    }
    dir.ancestors()
        .take(ROOT_SEARCH_DEPTH + 1)
        .find_map(|dir| declaring_class(dir, source))
        .unwrap_or_else(|| source.to_path_buf())
}

/// What the `% !TEX root` comment at the head of `content` names.
fn root_comment(content: &str) -> Option<&str> {
    let pattern = Regex::new(ROOT_PATTERN).ok()?;
    content
        .lines()
        .take(ROOT_COMMENT_LINES)
        .find_map(|line| pattern.captures(line)?.get(1))
        .map(|name| name.as_str())
}

/// The `.tex` file in `dir` that declares the class and is `source`'s
/// root. Of several, the one whose last build read `source` (its `.fls`
/// lists it), else the one with the newest PDF, else the first by name.
fn declaring_class(dir: &Path, source: &Path) -> Option<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| is_tex(path))
        .filter(|path| fs::read_to_string(path).is_ok_and(|content| content.contains(DOCUMENT_CLASS)))
        .collect();
    files.sort();
    if files.len() < 2 {
        return files.pop();
    }
    let read: Vec<&PathBuf> = files.iter().filter(|root| build_read(root, source)).collect();
    if let [only] = read.as_slice() {
        return Some((*only).clone());
    }
    let pool: Vec<&PathBuf> = match read.is_empty() {
        true => files.iter().collect(),
        false => read,
    };
    let built = |root: &PathBuf| {
        let pdf = [artifact_of(root, "pdf"), root.with_extension("pdf")].into_iter().find(|pdf| pdf.exists());
        pdf.and_then(|pdf| fs::metadata(pdf).ok()?.modified().ok())
    };
    // The first by name wins a tie, as `max_by_key` would not have it.
    let newest = pool.iter().copied().rev().max_by_key(|root| built(root));
    newest.cloned()
}

/// Whether the last build of `root` read `source`, by the `.fls` list of
/// files it wrote.
fn build_read(root: &Path, source: &Path) -> bool {
    let fls = [artifact_of(root, FLS_EXTENSION), root.with_extension(FLS_EXTENSION)];
    let Some(text) = fls.iter().find_map(|path| fs::read_to_string(path).ok()) else {
        return false;
    };
    let dir = root.parent().unwrap_or(Path::new(""));
    let wanted = normalized(source);
    text.lines()
        .filter_map(|line| line.strip_prefix(FLS_INPUT))
        .any(|input| normalized(&dir.join(input.trim())) == wanted)
}

/// Whether `path` is a BibTeX database, by its extension.
pub fn is_bib(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == BIB_EXTENSION)
}

/// The key of the BibTeX entry `row` (from zero) of `lines` is in: the
/// nearest entry starting at or above it, before the next one starts.
pub fn bib_key_at(lines: &[String], row: usize) -> Option<String> {
    let pattern = Regex::new(BIB_ENTRY_PATTERN).ok()?;
    let start = lines.iter().take(row + 1).rposition(|line| pattern.is_match(line))?;
    let caps = pattern.captures(&lines[start])?;
    let kind = caps.get(1)?.as_str().to_ascii_lowercase();
    // A `@string` or `@comment` is no entry anything cites.
    if matches!(kind.as_str(), "string" | "comment" | "preamble") {
        return None;
    }
    Some(caps.get(2)?.as_str().to_string())
}

/// Where in the project of `bib` the entry `key` is first cited: the line
/// and column (both from one) in a `.tex` file under its document's
/// directory, the root itself searched first.
pub fn cite_of(bib: &Path, key: &str) -> Option<(PathBuf, usize, usize)> {
    let root = root_of(bib);
    let pattern = Regex::new(CITE_PATTERN).ok()?;
    let top = root.parent().unwrap_or(Path::new("")).to_path_buf();
    let mut files = vec![root.clone()];
    files.extend(tex_files_under(&top).into_iter().filter(|file| *file != root));
    files.into_iter().find_map(|file| {
        let text = fs::read_to_string(&file).ok()?;
        text.lines().enumerate().find_map(|(at, line)| {
            let code = line.split(COMMENT).next().unwrap_or_default();
            pattern.captures_iter(code).find_map(|caps| {
                let keys = caps.get(1)?;
                keys.as_str().split(',').any(|cited| cited.trim() == key).then(|| {
                    let column = code[..keys.start()].chars().count() + 1;
                    (file.clone(), at + 1, column)
                })
            })
        })
    })
}

/// The `.tex` files under `dir`, by name, leaving out what tools write. The
/// walk follows no link to a directory, which could lead it round a loop for
/// good, goes no deeper than a document's chapters ever sit, and looks at a
/// bounded number of entries: the search runs on the window's thread, and a
/// `.bib` in a home directory would otherwise walk all of it.
fn tex_files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut looked = 0;
    let mut dirs = vec![(dir.to_path_buf(), 0)];
    while let Some((dir, depth)) = dirs.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<(PathBuf, bool)> = entries
            .flatten()
            .map(|entry| (entry.path(), entry.file_type().is_ok_and(|kind| kind.is_dir())))
            .collect();
        paths.sort();
        for (path, is_dir) in paths {
            looked += 1;
            if looked > CITE_SEARCH_ENTRIES {
                return found;
            }
            let name = path.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default();
            if is_dir {
                if depth < CITE_SEARCH_DEPTH && !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_str()) {
                    dirs.push((path, depth + 1));
                }
            } else if is_tex(&path) {
                found.push(path);
                if found.len() >= CITE_SEARCH_FILES {
                    return found;
                }
            }
        }
    }
    found
}

/// The `synctex edit` run asking where `spot` on `pdf` came from, run beside
/// the PDF, where its `.synctex.gz` lies.
pub fn edit_request(pdf: &Path, spot: Spot) -> CommandRequest {
    let query = format!(
        "{}:{}:{}:{}",
        spot.page + 1,
        spot.x.round() as i64,
        spot.y.round() as i64,
        pdf.display()
    );
    let pdf_dir = pdf
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_default();
    let mut args = vec!["edit".to_string(), "-o".to_string(), query];
    if let Some(dir) = synctex_data_dir(pdf) {
        args.push("-d".to_string());
        args.push(dir.display().to_string());
    }
    CommandRequest {
        id: 0,
        args,
        cwd: pdf_dir,
        env: Vec::new(),
        program: PROGRAM.to_string(),
        stdin: None,
        tag: TAG_EDIT,
    }
}

/// The source `synctex edit` named in `stdout`, its path taken from `dir`
/// where it is relative: the first answer, where it gave several.
pub fn parse_edit(stdout: &str, dir: &Path) -> Option<Source> {
    let field = |name: &str| {
        stdout
            .lines()
            .find_map(|line| line.strip_prefix(name))
            .map(str::trim)
    };
    let input = field(INPUT_FIELD).filter(|input| !input.is_empty())?;
    let line: usize = field(LINE_FIELD)?.parse().ok()?;
    Some(Source {
        line: line.max(1),
        path: normalized(&dir.join(input)),
    })
}

/// The line of `content`, from one, holding the text around the spot: the
/// whole of it where some line does, else its last and first few words,
/// either way the match nearest line `near` when there are several. Comment
/// lines are passed over.
pub fn find_in(content: &str, context: &Context, near: Option<usize>) -> Option<usize> {
    let before = context.before.trim();
    let after = context.after.trim();
    if before.is_empty() && after.is_empty() {
        return None;
    }
    nearest_holding(content, before, after, near).or_else(|| {
        let short_before = last_words(before, SHORT_WORDS);
        let short_after = first_words(after, SHORT_WORDS);
        nearest_holding(content, &short_before, &short_after, near)
    })
}

/// The files `content`, kept in `dir`, includes around its line `line` (from
/// one), nearest first, then the whole document's when none are near or the
/// line sets the title. A name given without its extension is tried with
/// `.tex` first; the ones not there are for the reader to skip.
pub fn included_files(content: &str, dir: &Path, line: usize) -> Vec<PathBuf> {
    let (Ok(include), Ok(bare)) = (Regex::new(INCLUDE_PATTERN), Regex::new(BARE_INPUT_PATTERN))
    else {
        return Vec::new();
    };
    let lines: Vec<&str> = content.lines().collect();
    let target = line.saturating_sub(1);
    let patterns = [&include, &bare];
    let mut names: Vec<String> = Vec::new();
    if let Some(text) = lines.get(target) {
        names.extend(names_in(text, &patterns));
    }
    for step in 1..=INCLUDE_RADIUS {
        if let Some(text) = target.checked_sub(step).and_then(|at| lines.get(at)) {
            names.extend(names_in(text, &patterns));
        }
        if let Some(text) = lines.get(target + step) {
            names.extend(names_in(text, &patterns));
        }
    }
    let sets_title = lines
        .get(target)
        .is_some_and(|text| text.contains(MAKETITLE));
    if names.is_empty() || sets_title {
        for text in &lines {
            names.extend(names_in(text, &patterns));
        }
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for name in names {
        let path = normalized(&dir.join(&name));
        let mut tries = Vec::new();
        if path.extension().is_none_or(|ext| ext != TEX_EXTENSION) {
            tries.push(PathBuf::from(format!("{}.{TEX_EXTENSION}", path.display())));
        }
        tries.push(path);
        for found in tries {
            if !paths.contains(&found) {
                paths.push(found);
            }
        }
    }
    paths
}

/// The names the include commands on `text` bring in: none on a comment.
fn names_in(text: &str, patterns: &[&Regex]) -> Vec<String> {
    if text.trim_start().starts_with(COMMENT) {
        return Vec::new();
    }
    patterns
        .iter()
        .flat_map(|pattern| pattern.captures_iter(text))
        .filter_map(|found| found.get(1))
        .map(|name| name.as_str().trim().to_string())
        .collect()
}

/// The line, from one, nearest `near` that holds `before` or `after`.
fn nearest_holding(content: &str, before: &str, after: &str, near: Option<usize>) -> Option<usize> {
    let holds = |line: &str| {
        (!before.is_empty() && line.contains(before)) || (!after.is_empty() && line.contains(after))
    };
    let mut best: Option<(usize, usize)> = None;
    for (index, line) in content.lines().enumerate() {
        if line.trim_start().starts_with(COMMENT) || !holds(line) {
            continue;
        }
        let distance = near.map_or(0, |near| index.abs_diff(near.saturating_sub(1)));
        if best.is_none_or(|(nearest, _)| distance < nearest) {
            best = Some((distance, index));
        }
    }
    best.map(|(_, index)| index + 1)
}

/// The last `count` words of `text`, a space apart.
fn last_words(text: &str, count: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    words[words.len().saturating_sub(count)..].join(" ")
}

/// The first `count` words of `text`, a space apart.
fn first_words(text: &str, count: usize) -> String {
    text.split_whitespace()
        .take(count)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `path` with its `.` steps dropped and each `..` taking the step before
/// it, the way SyncTeX's `./paper.tex` and an `\input{../common}` read.
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir if out.file_name().is_some() => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_the_citation_search_ends_in_a_directory_that_links_back_to_itself() {
        // Following the link, the walk went round the loop for good on the
        // window's thread.
        let dir = std::env::temp_dir().join(format!("winter-cite-loop-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("chapters")).expect("dirs");
        std::os::unix::fs::symlink(&dir, dir.join("chapters/back")).expect("link");
        fs::write(dir.join("chapters/one.tex"), "x").expect("file");
        let found = tex_files_under(&dir);
        assert_eq!(found, [dir.join("chapters/one.tex")]);
        let _ = fs::remove_dir_all(&dir);
    }

    fn context(before: &str, after: &str) -> Context {
        Context {
            after: after.to_string(),
            before: before.to_string(),
        }
    }

    #[test]
    fn test_the_edit_query_counts_pages_from_one_in_whole_points() {
        let request = edit_request(
            Path::new("/work/paper.pdf"),
            Spot {
                page: 2,
                x: 120.4,
                y: 300.6,
            },
        );
        assert_eq!(request.program, "synctex");
        assert_eq!(request.args, ["edit", "-o", "3:120:301:/work/paper.pdf"]);
        assert_eq!(request.cwd, Path::new("/work"));
    }

    #[test]
    fn test_the_answer_names_the_first_input_and_line_relative_to_the_pdf() {
        let stdout = "This is SyncTeX command line utility, version 1.5\n\
                      SyncTeX result begin\n\
                      Output:/work/./paper.pdf\n\
                      Input:/work/./chapters/intro.tex\n\
                      Line:42\n\
                      Column:-1\n\
                      Input:/work/./other.tex\n\
                      Line:7\n\
                      SyncTeX result end\n";
        assert_eq!(
            parse_edit(stdout, Path::new("/work")),
            Some(Source {
                line: 42,
                path: PathBuf::from("/work/chapters/intro.tex"),
            })
        );
        let relative = "Input:./paper.tex\nLine:3\n";
        assert_eq!(
            parse_edit(relative, Path::new("/work")).map(|source| source.path),
            Some(PathBuf::from("/work/paper.tex"))
        );
        assert_eq!(
            parse_edit("SyncTeX result begin\nSyncTeX result end\n", Path::new("/")),
            None
        );
    }

    #[test]
    fn test_the_text_around_the_spot_finds_its_line_nearest_synctex_s() {
        let source = "\\section{Intro}\n\
                      % We prove the theorem here.\n\
                      We prove the theorem here.\n\
                      Unrelated text.\n\
                      We prove the theorem here.\n";
        // Both lines 3 and 5 hold it, the comment on line 2 never counts.
        let around = context("We prove", "the theorem here.");
        assert_eq!(find_in(source, &around, Some(5)), Some(5));
        assert_eq!(find_in(source, &around, Some(1)), Some(3));
        assert_eq!(find_in(source, &context("", ""), Some(1)), None);
    }

    #[test]
    fn test_text_split_across_source_lines_is_found_by_its_nearest_words() {
        // The page reads one line where the source broke it in two.
        let source = "Results\nthe method converges\nquickly in all of our tests.\n";
        let around = context(
            "the method converges quickly",
            "in all of our tests. We then",
        );
        assert_eq!(find_in(source, &around, None), Some(3));
    }

    #[test]
    fn test_the_view_query_names_the_line_column_source_and_pdf() {
        let request = view_request(
            Path::new("/w/ch/intro.tex"),
            12,
            3,
            Path::new("/w/main.pdf"),
        );
        assert_eq!(request.tag, TAG_VIEW);
        assert_eq!(
            request.args,
            ["view", "-i", "12:3:/w/ch/intro.tex", "-o", "/w/main.pdf"]
        );
    }

    #[test]
    fn test_the_view_answer_gives_each_box_down_the_pages_from_its_baseline() {
        let stdout = "SyncTeX result begin\n\
                      Output:main.pdf\n\
                      Page:2\nx:72.0\ny:300.0\nh:72.0\nv:310.5\nW:400.0\nH:10.5\n\
                      Page:1\nx:72.0\ny:100.0\nh:80.0\nv:120.0\nW:300.0\nH:12.0\n\
                      Page:1\nh:80.0\nv:90.0\n\
                      SyncTeX result end\n";
        let marks = parse_view(stdout);
        assert_eq!(
            marks.len(),
            2,
            "the record with no width or height is skipped"
        );
        assert_eq!(
            marks[0],
            PdfMark {
                height: 12.0,
                left: 80.0,
                page: 0,
                top: 108.0,
                width: 300.0,
            }
        );
        assert_eq!((marks[1].page, marks[1].top), (1, 300.0));
    }

    #[test]
    fn test_the_root_comment_names_the_root_however_it_is_spaced() {
        assert_eq!(
            root_comment("%!TEX root = ../main.tex\n"),
            Some("../main.tex")
        );
        assert_eq!(
            root_comment("% !tex Root=main\n\\section{A}\n"),
            Some("main")
        );
        assert_eq!(root_comment("\\section{A}\n"), None);
    }

    #[test]
    fn test_a_chapter_finds_the_pdf_of_the_root_declaring_the_class() {
        let dir = std::env::temp_dir().join(format!("winter-synctex-{}", std::process::id()));
        let chapters = dir.join("chapters");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&chapters).expect("temp dir");
        fs::write(dir.join("main.tex"), "\\documentclass{article}\n").expect("main");
        fs::write(chapters.join("intro.tex"), "Intro text.\n").expect("intro");
        fs::write(chapters.join("other.tex"), "% !TEX root = ../main.tex\n").expect("other");
        assert_eq!(
            pdf_for(&chapters.join("intro.tex")),
            None,
            "nothing built yet"
        );
        fs::write(dir.join("main.pdf"), "").expect("pdf");
        assert_eq!(
            pdf_for(&chapters.join("intro.tex")),
            Some(dir.join("main.pdf"))
        );
        assert_eq!(
            pdf_for(&chapters.join("other.tex")),
            Some(dir.join("main.pdf"))
        );
        assert_eq!(pdf_for(&dir.join("main.tex")), Some(dir.join("main.pdf")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_includes_near_the_line_come_first_with_tex_tried_before_the_bare_name() {
        let main = "\\documentclass{article}\n\
                    \\begin{document}\n\
                    \\input{chapters/intro}\n\
                    \\include{body.tex}\n\
                    % \\input{old}\n\
                    \\end{document}\n\
                    \\subfile{appendix}\n";
        let files = included_files(main, Path::new("/work"), 3);
        assert_eq!(
            files,
            [
                PathBuf::from("/work/chapters/intro.tex"),
                PathBuf::from("/work/chapters/intro"),
                PathBuf::from("/work/body.tex"),
                PathBuf::from("/work/appendix.tex"),
                PathBuf::from("/work/appendix"),
            ]
        );
    }

    #[test]
    fn test_a_maketitle_line_looks_through_the_whole_document() {
        let mut main = String::from("\\input{front}\n");
        main.push_str(&"text\n".repeat(20));
        main.push_str("\\maketitle\n\\input{near}\n");
        // The include beside the title comes first, and the title still
        // looks through the whole document, where its text may come from.
        let files = included_files(&main, Path::new("/w"), 22);
        assert_eq!(files[0], PathBuf::from("/w/near.tex"));
        assert!(files.contains(&PathBuf::from("/w/front.tex")));
    }

    /// A directory of `files`, removed when the test is done.
    struct Project(PathBuf);

    impl Project {
        fn new(tag: &str, files: &[(&str, &str)]) -> Self {
            let dir = std::env::temp_dir().join(format!("winter-synctex-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            for (name, text) in files {
                let path = dir.join(name);
                fs::create_dir_all(path.parent().expect("dir")).expect("dirs");
                fs::write(path, text).expect("file");
            }
            Self(dir)
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_the_pdf_is_found_where_latexmk_was_told_to_write_it() {
        let project = Project::new(
            "outdir",
            &[
                ("latexmkrc", "$aux_dir = '_aux';\n$out_dir = \"_build\";\n"),
                ("main.tex", "\\documentclass{article}\n"),
                ("sections/intro.tex", "Hello.\n"),
                ("_build/main.pdf", ""),
            ],
        );
        let dir = &project.0;
        assert_eq!(pdf_for(&dir.join("sections/intro.tex")), Some(dir.join("_build/main.pdf")));
        // Without the settings, beside the root.
        fs::remove_file(dir.join("latexmkrc")).expect("rc");
        assert_eq!(pdf_for(&dir.join("main.tex")), None, "not beside the root yet");
        fs::write(dir.join("main.pdf"), "").expect("pdf");
        assert_eq!(pdf_for(&dir.join("main.tex")), Some(dir.join("main.pdf")));
    }

    #[test]
    fn test_of_two_roots_the_one_whose_build_read_the_file_is_taken() {
        let project = Project::new(
            "two-roots",
            &[
                ("a.tex", "\\documentclass{article}\n"),
                ("b.tex", "\\documentclass{article}\n"),
                ("sections/intro.tex", "Hello.\n"),
            ],
        );
        let dir = &project.0;
        let intro = dir.join("sections/intro.tex");
        assert_eq!(root_of(&intro), dir.join("a.tex"), "the first by name, failing all else");
        fs::write(dir.join("b.fls"), "PWD /x\nINPUT ./sections/intro.tex\n").expect("fls");
        assert_eq!(root_of(&intro), dir.join("b.tex"));
    }

    #[test]
    fn test_the_bib_entry_under_the_cursor_is_the_nearest_above_it() {
        let lines: Vec<String> = ["@string{x = {y}}", "", "@inproceedings{ smith20,", "  title = {T},", "}"]
            .iter()
            .map(|line| line.to_string())
            .collect();
        assert_eq!(bib_key_at(&lines, 4).as_deref(), Some("smith20"));
        assert_eq!(bib_key_at(&lines, 1), None, "a @string is no entry");
    }
}
