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
const ICON_SETS: [&str; 6] = ["apps", "brand", "file", "git", "gutter", "symbols"];

/// Icons packed from a single file under `assets/icons/` rather than a set's
/// directory, as `(packed name, file)`: the app's own icon, which the
/// installers ship from where it stands.
const LOOSE_ICONS: [(&str, &str); 1] = [("app/winter-terminal", "winter-terminal.svg")];

/// Name of the packed icon payload written into `OUT_DIR`.
const BLOB_NAME: &str = "icons.blob";

/// Name of the generated icon index written into `OUT_DIR`.
const INDEX_NAME: &str = "icons_index.rs";

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
    println!(
        "cargo:rerun-if-changed={}",
        git_dir.join("packed-refs").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        git_dir.join("refs/tags").display()
    );
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
    for (name, file) in LOOSE_ICONS {
        let path = assets.join(file);
        println!("cargo:rerun-if-changed={}", path.display());
        files.push((name.to_string(), path));
    }

    write_pack(&files, BLOB_NAME, INDEX_NAME, "ICON_INDEX");
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
