//! Image: one picture, drawn by Winter itself over the pane.
//!
//! The tool reads the file and keeps how it is shown, fitted or zoomed and
//! where it is scrolled to; the host decodes it and draws it on the GPU.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::{Key, KeyCode};
use crate::model::paint::{Frame, IconKind};
use crate::model::path::abbreviate_home;
use crate::model::picture::{Content, Picture, PictureView};
use crate::model::tool::{menu_groups, Notice, OpenTarget, Tool, ToolMenuItem, ToolOutcome, ToolStatus};

// ========================================================================
// Constants
// ========================================================================

/// Each extension the viewer draws, lowercase, and whether it is vectors.
const IMAGE_KINDS: [(&str, bool); 8] = [
    ("bmp", false),
    ("gif", false),
    ("ico", false),
    ("jpeg", false),
    ("jpg", false),
    ("png", false),
    ("svg", true),
    ("webp", false),
];

/// Pane pixels one `j`/`k` or `h`/`l` scrolls.
const STEP_PX: f32 = 48.0;

/// Fraction of the pane `C-d` and `C-u` move.
const HALF_SCREEN: f32 = 0.5;

/// Fraction of the pane the page keys move, short of a whole screen so the
/// band at the edge stays in view.
const FULL_SCREEN: f32 = 0.9;

/// The viewer's keys: scrolling round an image zoomed past the pane, and the
/// zoom itself.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Pan(0.0, 1.0),
        keys: &["j", "<down>"],
        label: "Scroll down",
    },
    Binding {
        action: Act::Pan(0.0, -1.0),
        keys: &["k", "<up>"],
        label: "Scroll up",
    },
    Binding {
        action: Act::Pan(-1.0, 0.0),
        keys: &["h", "<left>"],
        label: "Scroll left",
    },
    Binding {
        action: Act::Pan(1.0, 0.0),
        keys: &["l", "<right>"],
        label: "Scroll right",
    },
    Binding {
        action: Act::Screens(FULL_SCREEN),
        keys: &["SPC", "<next>", "C-f"],
        label: "Scroll down a screen",
    },
    Binding {
        action: Act::Screens(-FULL_SCREEN),
        keys: &["b", "<prior>", "C-b"],
        label: "Scroll up a screen",
    },
    Binding {
        action: Act::Screens(HALF_SCREEN),
        keys: &["C-d"],
        label: "Scroll down half a screen",
    },
    Binding {
        action: Act::Screens(-HALF_SCREEN),
        keys: &["C-u"],
        label: "Scroll up half a screen",
    },
    Binding {
        action: Act::Zoom(1),
        keys: &["+", "S-="],
        label: "Zoom in",
    },
    Binding {
        action: Act::Zoom(-1),
        keys: &["-"],
        label: "Zoom out",
    },
    Binding {
        action: Act::Fit,
        keys: &["="],
        label: "Fit the image to the pane",
    },
    Binding {
        action: Act::ActualSize,
        keys: &["1"],
        label: "Show the image at its own size",
    },
    Binding {
        action: Act::Position,
        keys: &["C-g"],
        label: "Say what the image is and how it is drawn",
    },
];

/// The menu, a group at a time: the zoom, then what the image is. Each
/// entry is named shorter than the help list names its key.
const MENU: [&[(Act, &str)]; 2] = [
    &[
        (Act::Zoom(1), "Zoom In"),
        (Act::Zoom(-1), "Zoom Out"),
        (Act::Fit, "Fit to Pane"),
        (Act::ActualSize, "Actual Size"),
    ],
    &[(Act::Position, "Image Info")],
];

/// Hands each image read a key of its own, so the host tells a file read
/// again from the one it already decoded.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the viewer's own does.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    /// Show the image at its own size.
    ActualSize,
    /// Fit the image to the pane.
    Fit,
    /// Scroll by these many steps across and down.
    Pan(f32, f32),
    /// Say what the image is and how it is drawn.
    Position,
    /// Scroll down by this fraction of the pane.
    Screens(f32),
    /// Zoom by this many steps.
    Zoom(i32),
}

/// A tab showing one image file.
pub struct ImageTool {
    /// The size the host decoded the image at, and the pane it last drew it
    /// in, once it has.
    geometry: Option<Geometry>,
    notice: Option<Notice>,
    path: PathBuf,
    /// The file's contents, or `None` for one that could not be read.
    picture: Option<Picture>,
}

/// The sizes a zoom or a scroll is reckoned from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    natural: (f32, f32),
    pane: (f32, f32),
}

// ========================================================================
// ImageTool
// ========================================================================

impl ImageTool {
    /// A tab showing `path`, read now.
    pub fn new(path: PathBuf) -> Self {
        let (picture, notice) = match std::fs::read(&path) {
            Ok(bytes) => (Some(picture_of(&path, bytes)), None),
            Err(e) => (
                None,
                Some(Notice::error(format!("{}: {e}", path.display()))),
            ),
        };
        Self {
            geometry: None,
            notice,
            path,
            picture,
        }
    }

    /// Whether `path` is a picture this viewer can draw.
    pub fn handles(path: &Path) -> bool {
        kind_of(path).is_some()
    }

    /// Whether `path` is a picture written as text, which the editor opens
    /// too and so stays the one a file opens in unless asked otherwise.
    pub fn is_markup(path: &Path) -> bool {
        kind_of(path) == Some(true)
    }

    fn on_act(&mut self, act: Act) -> ToolOutcome {
        if act == Act::Position {
            self.notice = Some(Notice::info(self.description()));
            return ToolOutcome::Consumed;
        }
        let (Some(picture), Some(at)) = (self.picture.as_mut(), self.geometry) else {
            return ToolOutcome::Consumed;
        };
        let vector = picture.content.is_vector();
        let view = &mut picture.view;
        match act {
            Act::ActualSize => view.actual_size(),
            Act::Fit => view.fit(),
            Act::Pan(across, down) => view.pan(
                (across * STEP_PX, down * STEP_PX),
                at.natural,
                at.pane,
                vector,
            ),
            Act::Screens(fraction) => {
                view.pan((0.0, fraction * at.pane.1), at.natural, at.pane, vector)
            }
            Act::Zoom(steps) => view.zoom_by(steps, at.natural, at.pane, vector),
            Act::Position => {}
        }
        ToolOutcome::Consumed
    }

    fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.path.to_string_lossy().to_string())
    }

    /// The image's size, `640×480`, once it has been decoded.
    fn size(&self) -> Option<String> {
        let (width, height) = self.geometry?.natural;
        Some(format!("{}×{}", width.round(), height.round()))
    }

    /// How the image is drawn: `fit 42%`, or `150%` once zoomed by hand.
    fn scale(&self) -> Option<String> {
        let picture = self.picture.as_ref()?;
        let at = self.geometry?;
        let scale = picture
            .view
            .scale(at.natural, at.pane, picture.content.is_vector());
        let percent = (scale * 100.0).round();
        Some(match picture.view.zoom {
            None => format!("fit {percent}%"),
            Some(_) => format!("{percent}%"),
        })
    }

    /// The file, its size, and its scale, as `C-g` says them.
    fn description(&self) -> String {
        [Some(self.file_name()), self.size(), self.scale()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl Tool for ImageTool {
    fn context_items(&self) -> Vec<ToolMenuItem> {
        menu_groups(MENU.iter().map(|group| {
            group
                .iter()
                .filter_map(|&(act, label)| {
                    table::menu_entry(KEYS, act).map(|(key, _)| ToolMenuItem::new(key, label))
                })
                .collect()
        }))
    }

    fn title(&self) -> String {
        self.file_name()
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::File {
            name: self.file_name(),
        })
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        // Nothing: the picture the host draws covers every row.
        Frame::new(Vec::new())
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn status(&self) -> ToolStatus {
        ToolStatus {
            mode: None,
            segments: [self.size(), self.scale()].into_iter().flatten().collect(),
        }
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        // The space bar may come as the character it types.
        let key = &match key.code {
            KeyCode::Char(' ') => Key {
                code: KeyCode::Space,
                ..*key
            },
            _ => *key,
        };
        match table::find(KEYS, key) {
            Some(act) => self.on_act(act),
            None => ToolOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(KEYS)
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        // The wheel reports rows the way the grid counts them, positive
        // upwards, which is the opposite of how far the image moves.
        self.on_act(Act::Pan(0.0, -lines as f32))
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.path))
    }

    fn file_reference(&self) -> Option<String> {
        Some(abbreviate_home(&self.path))
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        Some(OpenTarget::file(self.path.clone()))
    }

    fn picture(&self) -> Option<Picture> {
        self.picture.clone()
    }

    fn set_picture_geometry(&mut self, natural: (f32, f32), pane: (f32, f32)) {
        self.geometry = Some(Geometry { natural, pane });
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Whether the image at `path` is vectors, by its extension, or `None` for
/// a file the viewer does not draw.
fn kind_of(path: &Path) -> Option<bool> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    IMAGE_KINDS
        .iter()
        .find(|(known, _)| *known == extension)
        .map(|(_, vector)| *vector)
}

/// The picture of the file at `path`, read as `bytes`, fitted to the pane.
fn picture_of(path: &Path, bytes: Vec<u8>) -> Picture {
    let bytes: Arc<[u8]> = bytes.into();
    let source = match kind_of(path) {
        Some(true) => Content::Vector(bytes),
        Some(false) | None => Content::Raster(bytes),
    };
    Picture {
        key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
        content: source,
        view: PictureView::fitted(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    fn tool_with(path: &str) -> ImageTool {
        ImageTool {
            geometry: None,
            notice: None,
            path: PathBuf::from(path),
            picture: Some(picture_of(Path::new(path), b"pixels".to_vec())),
        }
    }

    #[test]
    fn test_every_menu_entry_is_a_key_the_viewer_answers() {
        let tool = tool_with("/tmp/a.png");
        let items: Vec<ToolMenuItem> = tool
            .context_items()
            .into_iter()
            .filter(|item| !item.is_separator())
            .collect();
        assert_eq!(items.len(), 5);
        for item in items {
            let mut probe = tool_with("/tmp/a.png");
            assert_ne!(probe.on_key(&item.keys[0]), ToolOutcome::Ignored, "{}", item.label);
        }
    }

    #[test]
    fn test_an_image_is_known_by_its_extension_in_any_case() {
        assert!(ImageTool::handles(Path::new("/shots/Screen.PNG")));
        assert!(ImageTool::handles(Path::new("logo.svg")));
        assert!(!ImageTool::handles(Path::new("notes.txt")));
        assert!(!ImageTool::handles(Path::new("png")), "a name, not a type");
        assert!(ImageTool::is_markup(Path::new("logo.SVG")));
        assert!(!ImageTool::is_markup(Path::new("photo.jpg")));
    }

    #[test]
    fn test_an_svg_is_drawn_from_vectors_and_a_photo_from_pixels() {
        let svg = tool_with("logo.svg").picture().expect("a picture");
        assert!(svg.content.is_vector());
        let photo = tool_with("a.JPG").picture().expect("a picture");
        assert!(!photo.content.is_vector());
    }

    #[test]
    fn test_a_zoom_waits_for_the_sizes_and_then_scales_from_the_fit() {
        let mut tool = tool_with("a.png");
        assert_eq!(
            tool.on_key(&press(KeyCode::Char('+'))),
            ToolOutcome::Consumed
        );
        assert_eq!(
            tool.picture().expect("a picture").view,
            PictureView::fitted(),
            "nothing to reckon from before the first frame"
        );
        tool.set_picture_geometry((1600.0, 1200.0), (800.0, 600.0));
        tool.on_key(&press(KeyCode::Char('+')));
        assert_eq!(tool.picture().expect("a picture").view.zoom, Some(0.625));
        assert_eq!(
            tool.on_key(&press(KeyCode::Char('q'))),
            ToolOutcome::Ignored
        );
    }

    #[test]
    fn test_the_status_shows_the_size_and_scale() {
        let mut tool = tool_with("a.png");
        assert!(tool.status().segments.is_empty(), "nothing known yet");
        tool.set_picture_geometry((1600.0, 1200.0), (800.0, 600.0));
        assert_eq!(tool.status().segments, vec!["1600×1200", "fit 50%"]);
        tool.on_key(&press(KeyCode::Char('1')));
        assert_eq!(tool.status().segments, vec!["1600×1200", "100%"]);
    }

    #[test]
    fn test_a_file_that_cannot_be_read_says_so() {
        let mut tool = ImageTool::new(PathBuf::from("/no/such/dir/a.png"));
        assert!(tool.picture().is_none());
        assert!(tool.take_notice().is_some());
    }
}
