//! Archives from a listing, as dired makes and opens them: `Z` extracts an
//! archive where it stands, or compresses a file (`gzip`) or a directory
//! (`.tar.gz`) beside itself; `c` puts the marked entries into one archive
//! named by its kind (`.zip`, `.tar.gz`, `.tar.xz`, `.7z`, ...). Each is the
//! archiver of its kind, run off the window's thread, one after another.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use winter_model::editor::{
    CommandOutput, CommandRequest, EditorOutcome, JobRequest, Notice, PromptMode, PromptRequest,
};

use super::entry::{Entry, EntryKind, Meta};
use super::source::{Listing, ReadPlan};
use super::tree::Row;
use super::DirEditor;

// ========================================================================
// Constants
// ========================================================================

/// The question the archive's name answers.
pub(super) const ASK_COMPRESS: &str = "compress-to";

/// Which job an archiver runs as.
pub(super) const TAG_ARCHIVE: &str = "archive";

/// What a new archive is named when nothing better suggests itself.
const DEFAULT_ARCHIVE: &str = "archive.tar.gz";

/// What each kind of archive or compressed file ends in, the longest
/// first so `.tar.gz` is not read as `.gz`.
const KINDS: &[(&str, Kind)] = &[
    (".tar.gz", Kind::TarGz),
    (".tar.xz", Kind::TarXz),
    (".tar.bz2", Kind::TarBz2),
    (".tar.zst", Kind::TarZst),
    (".tgz", Kind::TarGz),
    (".txz", Kind::TarXz),
    (".tbz2", Kind::TarBz2),
    (".tar", Kind::Tar),
    (".zip", Kind::Zip),
    (".jar", Kind::Zip),
    (".7z", Kind::SevenZip),
    (".rar", Kind::Rar),
    (".gz", Kind::Gz),
    (".xz", Kind::Xz),
    (".bz2", Kind::Bz2),
    (".zst", Kind::Zst),
];

// ========================================================================
// Data Structures
// ========================================================================

/// A kind of archive or compressed file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Bz2,
    Gz,
    Rar,
    SevenZip,
    Tar,
    TarBz2,
    TarGz,
    TarXz,
    TarZst,
    Xz,
    Zip,
    Zst,
}

impl Kind {
    /// Whether an archive of this kind can be browsed in-place as a directory.
    pub(super) fn is_browseable(&self) -> bool {
        matches!(
            self,
            Kind::Tar
                | Kind::TarGz
                | Kind::TarXz
                | Kind::TarBz2
                | Kind::TarZst
                | Kind::Zip
                | Kind::SevenZip
                | Kind::Rar
        )
    }
}

// ========================================================================
// DirEditor: archives
// ========================================================================

impl DirEditor {
    /// `Z`: extract each target that is an archive where it stands, and
    /// compress each that is not beside itself.
    pub(super) fn toggle_archive(&mut self) -> EditorOutcome {
        let targets = self.targets();
        if targets.is_empty() {
            return EditorOutcome::Consumed;
        }
        if targets
            .iter()
            .any(|path| winter_base::remote::parse(path).is_some())
        {
            self.notice = Some(Notice::error("archives: only entries on this machine"));
            return EditorOutcome::Consumed;
        }
        self.archive_queue = targets
            .iter()
            .filter_map(|path| toggle_command(path))
            .collect();
        self.next_archive()
    }

    /// `c`: ask what to call the archive the targets go into.
    pub(super) fn ask_compress(&mut self) -> EditorOutcome {
        let targets = self.targets();
        let suggested = match targets.as_slice() {
            [only] => format!("{}.tar.gz", name_of(only)),
            _ => DEFAULT_ARCHIVE.to_string(),
        };
        self.pending_archive = targets;
        EditorOutcome::Prompt(PromptRequest {
            initial: suggested,
            label: "Compress to (.zip, .tar.gz, .tar.xz, .7z …): ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_COMPRESS,
            title: None,
        })
    }

    /// Put the targets asked about into the archive `name`.
    pub(super) fn compress_to(&mut self, name: &str) -> EditorOutcome {
        let targets = std::mem::take(&mut self.pending_archive);
        let name = name.trim();
        let Some(kind) = kind_of(name) else {
            self.notice = Some(Notice::error(format!(
                "archives: {name} is no kind of archive this makes"
            )));
            return EditorOutcome::Consumed;
        };
        let names: Vec<String> = targets
            .iter()
            .map(|path| {
                path.strip_prefix(&self.root)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        match pack_command(kind, name, &names, &self.root) {
            Some(command) => {
                self.archive_queue = vec![command];
                self.next_archive()
            }
            None => {
                self.notice = Some(Notice::error(format!(
                    "archives: {name} holds one file; name a .zip or a .tar.gz"
                )));
                EditorOutcome::Consumed
            }
        }
    }

    /// Run the next archiver waiting, if one is.
    fn next_archive(&mut self) -> EditorOutcome {
        match self.archive_queue.first().cloned() {
            Some(command) => {
                self.archive_queue.remove(0);
                EditorOutcome::Job(JobRequest::Command(command))
            }
            None => {
                self.marks.clear();
                self.reload();
                EditorOutcome::Consumed
            }
        }
    }

    /// An archiver finished: say how, and run the next.
    pub(super) fn archive_done(&mut self, output: &CommandOutput) -> EditorOutcome {
        if !output.succeeded() {
            self.archive_queue.clear();
            self.notice = Some(Notice::error(format!("archives: {}", output.failure())));
            self.reload();
            return EditorOutcome::Consumed;
        }
        if self.archive_queue.is_empty() {
            self.notice = Some(Notice::info("done"));
        }
        self.next_archive()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The kind `name` names, by its ending.
fn kind_of(name: &str) -> Option<Kind> {
    let lower = name.to_ascii_lowercase();
    KINDS
        .iter()
        .find(|(ending, _)| lower.ends_with(ending))
        .map(|(_, kind)| *kind)
}

/// `path`'s own name.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// Whether `path` is an archive file that can be browsed as a directory.
pub(super) fn is_browseable_archive(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    kind_of(name).is_some_and(|k| k.is_browseable())
}

/// Find if `path` is an archive or sits within an archive.
/// Returns `(archive_file_path, relative_subpath_inside_archive)`.
pub(super) fn find_archive_in_path(path: &Path) -> Option<(PathBuf, String)> {
    let mut current = path.to_path_buf();
    let mut parts = Vec::new();
    loop {
        if is_browseable_archive(&current) {
            parts.reverse();
            let sub_path = parts.join("/");
            return Some((current, sub_path));
        }
        let file_name = current.file_name()?.to_string_lossy().into_owned();
        parts.push(file_name);
        current = current.parent()?.to_path_buf();
    }
}

/// List every file and directory relative path in `archive`.
pub(super) fn list_archive_entries(archive: &Path) -> Result<Vec<String>, String> {
    let name = name_of(archive);
    let kind = kind_of(&name).ok_or_else(|| "not an archive".to_string())?;
    let output = match kind {
        Kind::Zip => std::process::Command::new("unzip")
            .args(["-Z1", "--"])
            .arg(archive)
            .output(),
        Kind::Tar | Kind::TarGz | Kind::TarXz | Kind::TarBz2 | Kind::TarZst => {
            std::process::Command::new("tar")
                .args(["-tf"])
                .arg(archive)
                .output()
        }
        Kind::SevenZip => std::process::Command::new("7z")
            .args(["l", "-ba", "-slt"])
            .arg(archive)
            .output(),
        Kind::Rar => std::process::Command::new("unrar")
            .args(["lb"])
            .arg(archive)
            .output(),
        _ => return Err("unsupported archive format".to_string()),
    };
    let output = output.map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let entries = if kind == Kind::SevenZip {
        parse_7z_entries(&text)
    } else {
        parse_plain_entries(&text)
    };
    Ok(entries)
}

fn parse_plain_entries(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let normalized = trimmed
            .trim_start_matches("./")
            .trim_start_matches('/')
            .trim_end_matches('/');
        if !normalized.is_empty() {
            entries.push(normalized.to_string());
        }
    }
    entries
}

fn parse_7z_entries(text: &str) -> Vec<String> {
    let mut entries = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("Path = ") {
            let trimmed = path.trim();
            let normalized = trimmed
                .trim_start_matches("./")
                .trim_start_matches('/')
                .trim_end_matches('/');
            if !normalized.is_empty() {
                entries.push(normalized.to_string());
            }
        }
    }
    if !entries.is_empty() && (entries[0].ends_with(".7z") || entries[0].ends_with(".zip")) {
        entries.remove(0);
    }
    entries
}

/// Read the listing of an archive at `sub_path`, building a virtual tree.
pub(super) fn read_archive_listing(
    archive_file: &Path,
    sub_path: &str,
    plan: &ReadPlan,
) -> Listing {
    let entries = match list_archive_entries(archive_file) {
        Ok(e) => e,
        Err(_) => {
            return Listing {
                folds: plan.folds.clone(),
                rows: Vec::new(),
                truncated: false,
            };
        }
    };

    let mut rows = Vec::new();
    collect_archive_rows(archive_file, &entries, sub_path, 0, plan, &mut rows);

    Listing {
        folds: plan.folds.clone(),
        rows,
        truncated: false,
    }
}

fn collect_archive_rows(
    archive_file: &Path,
    all_entries: &[String],
    current_subpath: &str,
    depth: usize,
    plan: &ReadPlan,
    out: &mut Vec<Row>,
) {
    let prefix = if current_subpath.is_empty() {
        String::new()
    } else {
        format!("{current_subpath}/")
    };

    let mut children: BTreeMap<String, bool> = BTreeMap::new();
    for entry in all_entries {
        if !entry.starts_with(&prefix) {
            continue;
        }
        let remainder = &entry[prefix.len()..];
        if remainder.is_empty() {
            continue;
        }
        if let Some(slash_idx) = remainder.find('/') {
            let dir_name = &remainder[..slash_idx];
            children.insert(dir_name.to_string(), true);
        } else {
            children.entry(remainder.to_string()).or_insert(false);
        }
    }

    let mut dirs: Vec<String> = children
        .iter()
        .filter(|(_, &is_d)| is_d)
        .map(|(n, _)| n.clone())
        .collect();
    let mut files: Vec<String> = children
        .iter()
        .filter(|(_, &is_d)| !is_d)
        .map(|(n, _)| n.clone())
        .collect();

    if !plan.show_hidden {
        dirs.retain(|n| !n.starts_with('.'));
        files.retain(|n| !n.starts_with('.'));
    }

    for dir_name in dirs {
        let child_rel_path = if current_subpath.is_empty() {
            dir_name.clone()
        } else {
            format!("{current_subpath}/{dir_name}")
        };
        let full_path = archive_file.join(&child_rel_path);
        let expanded = plan.folds.is_expanded(&full_path);
        out.push(Row {
            depth,
            entry: Entry::new(EntryKind::Dir, Meta::default(), dir_name, full_path.clone()),
            expanded,
        });

        if expanded {
            collect_archive_rows(
                archive_file,
                all_entries,
                &child_rel_path,
                depth + 1,
                plan,
                out,
            );
        }
    }

    for file_name in files {
        let child_rel_path = if current_subpath.is_empty() {
            file_name.clone()
        } else {
            format!("{current_subpath}/{file_name}")
        };
        let full_path = archive_file.join(&child_rel_path);
        out.push(Row {
            depth,
            entry: Entry::new(EntryKind::File, Meta::default(), file_name, full_path),
            expanded: false,
        });
    }
}

/// Extract a single file from `archive` to a temporary file on disk for viewing.
pub(super) fn extract_single_file_to_temp(archive: &Path, rel_path: &str) -> Option<PathBuf> {
    let name = name_of(archive);
    let kind = kind_of(&name)?;
    let temp_dir = std::env::temp_dir().join("winter-term-archive");
    let _ = std::fs::create_dir_all(&temp_dir);
    let file_name = Path::new(rel_path).file_name()?.to_string_lossy();
    let temp_file = temp_dir.join(format!("{}-{}", std::process::id(), file_name));

    let output = match kind {
        Kind::Zip => std::process::Command::new("unzip")
            .args(["-p", "--"])
            .arg(archive)
            .arg(rel_path)
            .output(),
        Kind::Tar | Kind::TarGz | Kind::TarXz | Kind::TarBz2 | Kind::TarZst => {
            std::process::Command::new("tar")
                .args(["-xf"])
                .arg(archive)
                .arg(rel_path)
                .arg("-O")
                .output()
        }
        Kind::SevenZip => std::process::Command::new("7z")
            .args(["e", "-so"])
            .arg(archive)
            .arg(rel_path)
            .output(),
        Kind::Rar => std::process::Command::new("unrar")
            .args(["p", "-inul"])
            .arg(archive)
            .arg(rel_path)
            .output(),
        _ => return None,
    };

    let output = output.ok()?;
    if output.status.success() {
        std::fs::write(&temp_file, output.stdout).ok()?;
        Some(temp_file)
    } else {
        None
    }
}

/// `program` run with `args` in `cwd`.
fn command(program: &str, args: Vec<String>, cwd: &Path) -> CommandRequest {
    CommandRequest {
        id: 0,
        args,
        cwd: cwd.to_path_buf(),
        env: Vec::new(),
        program: program.to_string(),
        stdin: None,
        tag: TAG_ARCHIVE,
    }
}

/// What `Z` runs on `path`: extracting it where it stands if it is an
/// archive, else compressing it beside itself.
fn toggle_command(path: &Path) -> Option<CommandRequest> {
    let dir = path.parent()?.to_path_buf();
    let name = name_of(path);
    let whole = path.to_string_lossy().into_owned();
    let into = dir.to_string_lossy().into_owned();
    let request = match kind_of(&name) {
        Some(Kind::Tar | Kind::TarGz | Kind::TarXz | Kind::TarBz2 | Kind::TarZst) => command(
            "tar",
            vec![
                "--keep-old-files".into(),
                "-xf".into(),
                whole,
                "-C".into(),
                into,
            ],
            &dir,
        ),
        // Each extractor is told to leave a file that is already there: `Z` is
        // one key, and an archive unpacked over edited work would replace it.
        Some(Kind::Zip) => command("unzip", vec!["-n".into(), whole, "-d".into(), into], &dir),
        Some(Kind::SevenZip) => command(
            "7z",
            vec!["x".into(), "-aos".into(), format!("-o{into}"), whole],
            &dir,
        ),
        Some(Kind::Rar) => command(
            "unrar",
            vec!["x".into(), "-o-".into(), whole, format!("{into}/")],
            &dir,
        ),
        Some(Kind::Gz) => command("gunzip", vec!["-k".into(), whole], &dir),
        Some(Kind::Xz) => command("unxz", vec!["-k".into(), whole], &dir),
        Some(Kind::Bz2) => command("bunzip2", vec!["-k".into(), whole], &dir),
        Some(Kind::Zst) => command("unzstd", vec!["-k".into(), whole], &dir),
        None if path.is_dir() => command(
            "tar",
            vec!["-czf".into(), format!("{name}.tar.gz"), operand(&name)],
            &dir,
        ),
        None => command("gzip", vec!["-k".into(), whole], &dir),
    };
    Some(request)
}

/// What puts `names` (relative to `cwd`) into the archive `archive` of
/// `kind`; `None` for a kind that holds one file only.
fn pack_command(kind: Kind, archive: &str, names: &[String], cwd: &Path) -> Option<CommandRequest> {
    let archive = &operand(archive);
    let names: Vec<String> = names.iter().map(|name| operand(name)).collect();
    let names = &names;
    let tar = |flag: &str| {
        let mut args = vec![flag.to_string(), archive.to_string()];
        args.extend(names.iter().cloned());
        command("tar", args, cwd)
    };
    let request = match kind {
        Kind::Tar => tar("-cf"),
        Kind::TarGz => tar("-czf"),
        Kind::TarXz => tar("-cJf"),
        Kind::TarBz2 => tar("-cjf"),
        Kind::TarZst => {
            let mut args = vec!["--zstd".to_string(), "-cf".to_string(), archive.to_string()];
            args.extend(names.iter().cloned());
            command("tar", args, cwd)
        }
        Kind::Zip => {
            let mut args = vec!["-r".to_string(), archive.to_string()];
            args.extend(names.iter().cloned());
            command("zip", args, cwd)
        }
        Kind::SevenZip => {
            let mut args = vec!["a".to_string(), archive.to_string()];
            args.extend(names.iter().cloned());
            command("7z", args, cwd)
        }
        Kind::Rar | Kind::Gz | Kind::Xz | Kind::Bz2 | Kind::Zst => return None,
    };
    Some(request)
}

/// `name`, relative to the directory a command runs in, written so no tool
/// can read it as an option: a file named `--checkpoint-action=exec=...`
/// handed to tar as it is runs whatever it names.
fn operand(name: &str) -> String {
    match name.starts_with('-') {
        true => format!("./{name}"),
        false => name.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_an_archive_is_extracted_where_it_stands_and_a_file_compressed_beside_itself() {
        let extract = toggle_command(&PathBuf::from("/work/data.tar.gz")).expect("a command");
        assert_eq!(
            (extract.program.as_str(), extract.args.clone()),
            (
                "tar",
                vec![
                    "--keep-old-files".into(),
                    "-xf".into(),
                    "/work/data.tar.gz".into(),
                    "-C".into(),
                    "/work".into()
                ]
            )
        );
        let zip = toggle_command(&PathBuf::from("/work/a.zip")).expect("a command");
        assert_eq!(zip.program, "unzip");
        let file = toggle_command(&PathBuf::from("/work/notes.txt")).expect("a command");
        assert_eq!(
            (file.program.as_str(), file.args[0].as_str()),
            ("gzip", "-k")
        );
    }

    #[test]
    fn test_a_name_starting_with_a_dash_is_never_read_as_an_option() {
        // `tar -czf out.tar.gz --checkpoint-action=exec=sh x.sh` runs x.sh.
        let hostile = "--checkpoint-action=exec=sh x.sh".to_string();
        for kind in [Kind::TarGz, Kind::Zip, Kind::SevenZip] {
            let request = pack_command(
                kind,
                "out.tar.gz",
                std::slice::from_ref(&hostile),
                Path::new("/work"),
            )
            .expect("a command");
            assert!(
                request.args.iter().all(|arg| arg != &hostile),
                "{kind:?}: {:?}",
                request.args
            );
            assert!(request.args.contains(&format!("./{hostile}")), "{kind:?}");
        }
    }

    #[test]
    fn test_extracting_never_overwrites_a_file_already_there() {
        // `Z` is a single key; an archive unpacked over edited work
        // replaces it with what the archive held.
        let tree = std::env::temp_dir().join(format!("winter-archive-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tree);
        std::fs::create_dir_all(tree.join("src")).expect("dir");
        std::fs::write(tree.join("src/a.txt"), "old\n").expect("file");
        let packed = std::process::Command::new("tar")
            .args(["-czf", "src.tar.gz", "src"])
            .current_dir(&tree)
            .status()
            .expect("tar runs");
        assert!(packed.success());
        std::fs::write(tree.join("src/a.txt"), "edited\n").expect("edit");
        let request = toggle_command(&tree.join("src.tar.gz")).expect("a command");
        let _ = std::process::Command::new(&request.program)
            .args(&request.args)
            .current_dir(&request.cwd)
            .stderr(std::process::Stdio::null())
            .status()
            .expect("tar runs");
        assert_eq!(
            std::fs::read_to_string(tree.join("src/a.txt")).expect("reads"),
            "edited\n"
        );
        let _ = std::fs::remove_dir_all(&tree);
    }

    #[test]
    fn test_an_archive_s_name_says_which_archiver_packs_it() {
        let names = vec!["a".to_string(), "b/c".to_string()];
        let xz = pack_command(
            kind_of("out.tar.xz").expect("a kind"),
            "out.tar.xz",
            &names,
            Path::new("/w"),
        )
        .expect("packs");
        assert_eq!(xz.args, ["-cJf", "out.tar.xz", "a", "b/c"]);
        let zip = pack_command(Kind::Zip, "out.zip", &names, Path::new("/w")).expect("packs");
        assert_eq!((zip.program.as_str(), zip.args[0].as_str()), ("zip", "-r"));
        assert!(
            pack_command(Kind::Gz, "out.gz", &names, Path::new("/w")).is_none(),
            "one file only"
        );
        assert_eq!(kind_of("notes.txt"), None);
    }

    #[test]
    fn test_archive_in_place_browsing_and_extraction() {
        use super::super::source::Opening;
        use super::super::tree::Folds;

        let tree =
            std::env::temp_dir().join(format!("winter-archive-browse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tree);
        std::fs::create_dir_all(tree.join("src")).expect("dir");
        std::fs::write(tree.join("src/lib.rs"), "pub fn hello() {}\n").expect("file");
        std::fs::write(tree.join("src/main.rs"), "fn main() {}\n").expect("file");
        std::fs::write(tree.join("README.md"), "# Test\n").expect("file");

        let archive_path = tree.join("bundle.tar.gz");
        let packed = std::process::Command::new("tar")
            .args(["-czf", "bundle.tar.gz", "src", "README.md"])
            .current_dir(&tree)
            .status()
            .expect("tar runs");
        assert!(packed.success());

        assert!(is_browseable_archive(&archive_path));
        let (found_archive, sub_path) = find_archive_in_path(&archive_path).expect("finds archive");
        assert_eq!(found_archive, archive_path);
        assert_eq!(sub_path, "");

        let nested_file = archive_path.join("src").join("lib.rs");
        let (found_archive2, sub_path2) =
            find_archive_in_path(&nested_file).expect("finds archive in nested path");
        assert_eq!(found_archive2, archive_path);
        assert_eq!(sub_path2, "src/lib.rs");

        let mut folds = Folds::new();
        let plan = ReadPlan {
            folds: folds.clone(),
            opening: Opening::AsFolded,
            root: archive_path.clone(),
            show_hidden: true,
            sort: super::super::listing::SortKey::Name,
        };

        // Root listing of archive
        let listing = read_archive_listing(&archive_path, "", &plan);
        let names: Vec<&str> = listing.rows.iter().map(|r| r.entry.name.as_str()).collect();
        assert_eq!(names, ["src", "README.md"]);
        assert!(listing.rows[0].entry.is_dir());
        assert!(!listing.rows[0].expanded);
        assert!(!listing.rows[1].entry.is_dir());

        // Expanded "src" folder
        folds.expand(&archive_path.join("src"));
        let plan_expanded = ReadPlan {
            folds,
            opening: Opening::AsFolded,
            root: archive_path.clone(),
            show_hidden: true,
            sort: super::super::listing::SortKey::Name,
        };
        let listing_expanded = read_archive_listing(&archive_path, "", &plan_expanded);
        let expanded_names: Vec<(&str, usize)> = listing_expanded
            .rows
            .iter()
            .map(|r| (r.entry.name.as_str(), r.depth))
            .collect();
        assert_eq!(
            expanded_names,
            [("src", 0), ("lib.rs", 1), ("main.rs", 1), ("README.md", 0)]
        );

        // Single file extraction
        let extracted = extract_single_file_to_temp(&archive_path, "src/lib.rs").expect("extracts");
        assert_eq!(
            std::fs::read_to_string(&extracted).expect("reads"),
            "pub fn hello() {}\n"
        );

        let _ = std::fs::remove_file(extracted);
        let _ = std::fs::remove_dir_all(&tree);
    }
}
