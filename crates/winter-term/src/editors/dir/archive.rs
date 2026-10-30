//! Archives from a listing, as dired makes and opens them: `Z` extracts an
//! archive where it stands, or compresses a file (`gzip`) or a directory
//! (`.tar.gz`) beside itself; `c` puts the marked entries into one archive
//! named by its kind (`.zip`, `.tar.gz`, `.tar.xz`, `.7z`, ...). Each is the
//! archiver of its kind, run off the window's thread, one after another.

use std::path::Path;

use crate::model::editor::{CommandOutput, CommandRequest, EditorOutcome, JobRequest, Notice, PromptMode, PromptRequest};

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
        if targets.iter().any(|path| crate::remote::parse(path).is_some()) {
            self.notice = Some(Notice::error("archives: only entries on this machine"));
            return EditorOutcome::Consumed;
        }
        self.archive_queue = targets.iter().filter_map(|path| toggle_command(path)).collect();
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
            label: "Compress to (.zip, .tar.gz, .tar.xz, .7z ...): ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_COMPRESS,
        })
    }

    /// Put the targets asked about into the archive `name`.
    pub(super) fn compress_to(&mut self, name: &str) -> EditorOutcome {
        let targets = std::mem::take(&mut self.pending_archive);
        let name = name.trim();
        let Some(kind) = kind_of(name) else {
            self.notice = Some(Notice::error(format!("archives: {name} is no kind of archive this makes")));
            return EditorOutcome::Consumed;
        };
        let names: Vec<String> = targets
            .iter()
            .map(|path| path.strip_prefix(&self.root).unwrap_or(path).to_string_lossy().into_owned())
            .collect();
        match pack_command(kind, name, &names, &self.root) {
            Some(command) => {
                self.archive_queue = vec![command];
                self.next_archive()
            }
            None => {
                self.notice = Some(Notice::error(format!("archives: {name} holds one file; name a .zip or a .tar.gz")));
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
    KINDS.iter().find(|(ending, _)| lower.ends_with(ending)).map(|(_, kind)| *kind)
}

/// `path`'s own name.
fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |name| name.to_string_lossy().into_owned())
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
        Some(Kind::Tar | Kind::TarGz | Kind::TarXz | Kind::TarBz2 | Kind::TarZst) => {
            command("tar", vec!["--keep-old-files".into(), "-xf".into(), whole, "-C".into(), into], &dir)
        }
        // Each extractor is told to leave a file that is already there: `Z` is
        // one key, and an archive unpacked over edited work would replace it.
        Some(Kind::Zip) => command("unzip", vec!["-n".into(), whole, "-d".into(), into], &dir),
        Some(Kind::SevenZip) => command("7z", vec!["x".into(), "-aos".into(), format!("-o{into}"), whole], &dir),
        Some(Kind::Rar) => command("unrar", vec!["x".into(), "-o-".into(), whole, format!("{into}/")], &dir),
        Some(Kind::Gz) => command("gunzip", vec!["-k".into(), whole], &dir),
        Some(Kind::Xz) => command("unxz", vec!["-k".into(), whole], &dir),
        Some(Kind::Bz2) => command("bunzip2", vec!["-k".into(), whole], &dir),
        Some(Kind::Zst) => command("unzstd", vec!["-k".into(), whole], &dir),
        None if path.is_dir() => command("tar", vec!["-czf".into(), format!("{name}.tar.gz"), operand(&name)], &dir),
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
        assert_eq!((extract.program.as_str(), extract.args.clone()), ("tar", vec!["--keep-old-files".into(), "-xf".into(), "/work/data.tar.gz".into(), "-C".into(), "/work".into()]));
        let zip = toggle_command(&PathBuf::from("/work/a.zip")).expect("a command");
        assert_eq!(zip.program, "unzip");
        let file = toggle_command(&PathBuf::from("/work/notes.txt")).expect("a command");
        assert_eq!((file.program.as_str(), file.args[0].as_str()), ("gzip", "-k"));
    }

    #[test]
    fn test_a_name_starting_with_a_dash_is_never_read_as_an_option() {
        // `tar -czf out.tar.gz --checkpoint-action=exec=sh x.sh` runs x.sh.
        let hostile = "--checkpoint-action=exec=sh x.sh".to_string();
        for kind in [Kind::TarGz, Kind::Zip, Kind::SevenZip] {
            let request = pack_command(kind, "out.tar.gz", std::slice::from_ref(&hostile), Path::new("/work"))
                .expect("a command");
            assert!(request.args.iter().all(|arg| arg != &hostile), "{kind:?}: {:?}", request.args);
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
        assert_eq!(std::fs::read_to_string(tree.join("src/a.txt")).expect("reads"), "edited\n");
        let _ = std::fs::remove_dir_all(&tree);
    }

    #[test]
    fn test_an_archive_s_name_says_which_archiver_packs_it() {
        let names = vec!["a".to_string(), "b/c".to_string()];
        let xz = pack_command(kind_of("out.tar.xz").expect("a kind"), "out.tar.xz", &names, Path::new("/w")).expect("packs");
        assert_eq!(xz.args, ["-cJf", "out.tar.xz", "a", "b/c"]);
        let zip = pack_command(Kind::Zip, "out.zip", &names, Path::new("/w")).expect("packs");
        assert_eq!((zip.program.as_str(), zip.args[0].as_str()), ("zip", "-r"));
        assert!(pack_command(Kind::Gz, "out.gz", &names, Path::new("/w")).is_none(), "one file only");
        assert_eq!(kind_of("notes.txt"), None);
    }
}
