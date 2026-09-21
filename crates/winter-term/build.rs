//! Embeds the Windows executable icon, packs the bundled SVG icon set, and
//! stamps the version with the commit id on builds between releases.

use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use flate2::write::DeflateEncoder;
use flate2::Compression;
#[cfg(windows)]
use winresource::WindowsResource;

// Constants

// Only Windows PE binaries carry an icon resource; other platforms ship the
// icon separately (see assets/icons and crates/winter-term/Cargo.toml's
// package.metadata.deb).
#[cfg(windows)]
const ICON_PATH: &str = "assets/icons/winter-terminal.ico";

/// Icon sets packed into the binary, each a directory of `.svg` files under
/// `assets/icons/`. A name is the file's stem, prefixed by its set.
const ICON_SETS: [&str; 2] = ["file", "git"];

/// Name of the packed icon payload written into `OUT_DIR`.
const BLOB_NAME: &str = "icons.blob";

/// Name of the generated icon index written into `OUT_DIR`.
const INDEX_NAME: &str = "icons_index.rs";

/// Vendored pdf.js build packed into the binary, from the `pdfjs-dist` npm
/// package's `legacy/build` directory (Apache-2.0, see
/// `assets/pdfjs/LICENSE`). The legacy build is the one that targets older
/// browser engines, which is what the system WebView can turn out to be.
///
/// To refresh: download `pdfjs-dist@<version>`, copy these two files out of
/// `legacy/build`, copy `standard_fonts/` beside them, and update
/// `PDFJS_VERSION`.
const PDFJS_FILES: [&str; 2] = ["pdf.min.mjs", "pdf.worker.min.mjs"];

/// Directory of the vendored standard-font pack, under `assets/pdfjs`, and
/// the extensions worth packing out of it. The two licences beside the fonts
/// stay in the repository for attribution; nothing ever fetches them, so the
/// binary does not carry them.
const PDFJS_FONT_DIR: &str = "standard_fonts";
const PDFJS_FONT_EXTENSIONS: [&str; 2] = ["pfb", "ttf"];

/// Name of the packed pdf.js payload written into `OUT_DIR`.
const PDFJS_BLOB_NAME: &str = "pdfjs.blob";

/// Name of the generated pdf.js index written into `OUT_DIR`.
const PDFJS_INDEX_NAME: &str = "pdfjs_index.rs";

/// The `pdfjs-dist` release `assets/pdfjs` was taken from.
const PDFJS_VERSION: &str = "5.4.624";

/// Compile-time variable carrying the version `winter --version` prints.
/// Not `WINTER_VERSION`: the Makefile already exports that name for the
/// installer, and it would read as the plain release number.
const BUILD_VERSION_ENV: &str = "WINTER_BUILD_VERSION";

/// Prefix of a release tag, as `release.yml` triggers on (`v0.1.0`).
const RELEASE_TAG_PREFIX: &str = "v";

fn main() {
    #[cfg(windows)]
    WindowsResource::new()
        .set_icon(ICON_PATH)
        .compile()
        .expect("failed to embed Windows icon resource");

    pack_icons();
    pack_pdfjs();
    stamp_version();
}

/// Export the build's version: the crate version when HEAD is that
/// release's tag, or the crate version with the commit id as semver build
/// metadata (`0.1.0+7744bde`) when HEAD is anything else.
///
/// A build outside a git checkout (a release tarball, a crates.io source)
/// gets the bare crate version, since the only such sources are releases.
fn stamp_version() {
    let release = env::var("CARGO_PKG_VERSION").expect("cargo sets CARGO_PKG_VERSION");
    let version = match git(&["rev-parse", "--short", "HEAD"]) {
        Some(commit) if !head_is_release(&release) => format!("{release}+{commit}"),
        _ => release,
    };
    println!("cargo:rustc-env={BUILD_VERSION_ENV}={version}");
    watch_git_head();
}

/// Whether HEAD carries the tag of release `version`.
fn head_is_release(version: &str) -> bool {
    let tag = format!("{RELEASE_TAG_PREFIX}{version}");
    git(&["tag", "--points-at", "HEAD"]).is_some_and(|tags| tags.lines().any(|t| t == tag))
}

/// Rerun this script when HEAD moves or a tag is added, so a new commit or
/// a fresh release tag changes the stamped version without a clean build.
fn watch_git_head() {
    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]) else {
        return;
    };
    let git_dir = Path::new(&git_dir);
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    println!("cargo:rerun-if-changed={}", git_dir.join("packed-refs").display());
    println!("cargo:rerun-if-changed={}", git_dir.join("refs/tags").display());
    if let Some(branch) = git(&["symbolic-ref", "-q", "HEAD"]) {
        println!("cargo:rerun-if-changed={}", git_dir.join(branch).display());
    }
}

/// Trimmed stdout of a git command, or `None` when git is missing, the
/// source is not a checkout, or the command fails.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    Some(text.trim().to_owned())
}

/// Deflate every bundled SVG into one payload, and write the sorted
/// name-to-slice index beside it.
///
/// Each icon is compressed on its own rather than the set being compressed as a
/// whole, so drawing one icon inflates only that icon (a kilobyte or two)
/// instead of unpacking eight megabytes to reach it. The payload is generated
/// rather than checked in so the repository keeps the readable SVGs, while the
/// binary carries them at about a fifth of the size.
fn pack_icons() {
    let assets = Path::new("assets/icons");
    let mut files: Vec<(String, PathBuf)> = Vec::new();

    for set in ICON_SETS {
        let dir = assets.join(set);
        println!("cargo:rerun-if-changed={}", dir.display());
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
            .map(|entry| entry.expect("a readable directory entry").path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "svg"))
            .collect();
        entries.sort();

        for path in entries {
            let stem = path
                .file_stem()
                .expect("an .svg path has a stem")
                .to_string_lossy()
                .to_string();
            files.push((format!("{set}/{stem}"), path));
        }
    }

    write_pack(&files, BLOB_NAME, INDEX_NAME, "ICON_INDEX");
}

/// Deflate the vendored pdf.js build into its own payload, the same way the
/// icons are packed and for the same reason: the readable (if minified)
/// sources stay in the repository, and the binary carries them at about a
/// third of the size. The viewer serves them straight out of the blob over a
/// custom protocol, so a PDF opens with no network access at all.
fn pack_pdfjs() {
    let dir = Path::new("assets/pdfjs");
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rustc-env=PDFJS_VERSION={PDFJS_VERSION}");

    let fonts = dir.join(PDFJS_FONT_DIR);
    println!("cargo:rerun-if-changed={}", fonts.display());

    let mut files: Vec<(String, PathBuf)> = PDFJS_FILES
        .iter()
        .map(|name| ((*name).to_string(), dir.join(name)))
        .collect();
    files.extend(font_pack(&fonts));

    write_pack(&files, PDFJS_BLOB_NAME, PDFJS_INDEX_NAME, "PDFJS_INDEX");
}

/// Every font of the standard-font pack, named the way pdf.js asks for it.
///
/// The viewer hands the library this directory as a URL and the library
/// appends the file name of whichever base font a PDF named without embedding,
/// so a document that assumes Helvetica is on the machine still draws the
/// right shapes on a machine that has no fonts at all.
fn font_pack(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|entry| entry.expect("a readable directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|ext| PDFJS_FONT_EXTENSIONS.iter().any(|want| ext == *want))
        })
        .collect();
    entries.sort();

    entries
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .expect("a font path has a file name")
                .to_string_lossy()
                .to_string();
            (format!("{PDFJS_FONT_DIR}/{name}"), path)
        })
        .collect()
}

/// Deflate every named file into one payload and write it to `OUT_DIR`,
/// beside a `static <static_name>` index of where each one landed, sorted by
/// name so the runtime can binary-search it.
fn write_pack(files: &[(String, PathBuf)], blob_name: &str, index_name: &str, static_name: &str) {
    let out_dir =
        PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR is set for build scripts"));

    let mut blob: Vec<u8> = Vec::new();
    let mut index: Vec<(String, usize, usize, usize)> = Vec::new();

    for (name, path) in files {
        let raw = fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&raw)
            .expect("deflating into a Vec cannot fail");
        let packed = encoder.finish().expect("deflating into a Vec cannot fail");
        index.push((name.clone(), blob.len(), packed.len(), raw.len()));
        blob.extend_from_slice(&packed);
    }

    index.sort_by(|a, b| a.0.cmp(&b.0));

    let mut source = format!(
        "// Generated by build.rs. Each entry is (name, offset, packed length, \
         inflated length), sorted by name.\n\
         pub(crate) static {static_name}: &[(&str, u32, u32, u32)] = &[\n"
    );
    for (name, offset, packed, raw) in &index {
        source.push_str(&format!("    ({name:?}, {offset}, {packed}, {raw}),\n"));
    }
    source.push_str("];\n");

    fs::write(out_dir.join(blob_name), &blob).expect("writing the packed payload");
    fs::write(out_dir.join(index_name), source).expect("writing the pack index");
}
