//! Embeds the Windows executable icon into the winter binary.

#[cfg(windows)]
use winresource::WindowsResource;

// Constants

// Only Windows PE binaries carry an icon resource; other platforms ship the
// icon separately (see assets/icons and crates/winter-term/Cargo.toml's
// package.metadata.deb).
#[cfg(windows)]
const ICON_PATH: &str = "assets/icons/winter-terminal.ico";

fn main() {
    #[cfg(windows)]
    WindowsResource::new()
        .set_icon(ICON_PATH)
        .compile()
        .expect("failed to embed Windows icon resource");
}
