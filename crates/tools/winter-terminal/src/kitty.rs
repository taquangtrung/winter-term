//! Kitty graphics protocol commands: the payload of `APC G <keys> ; <data> ST`,
//! read into what the terminal should do with it.
//!
//! A command is a comma-separated list of `key=value` controls, then
//! optionally `;` and base64 data. An image may arrive over several commands
//! (`m=1` on all but the last), in which case only the first carries the
//! controls. Winter keeps no image store: a transmitted image is shown inline
//! as a block, so `a=t` (transmit) and `a=T` (transmit and display) both show
//! it, while the commands that act on stored images (put, delete, animate,
//! compose) have nothing to act on and are ignored, as is anything sent by a
//! medium other than direct data: reading a file or shared memory named by
//! whatever wrote to the PTY is not something a terminal should do for it.

use std::io::Cursor;

use base64::Engine;
use image::{DynamicImage, ImageFormat, RgbImage, RgbaImage};

// ========================================================================
// Constants
// ========================================================================

/// The most base64 one chunked transfer (`m=1` chunks) gathers before it is
/// dropped. Each chunk is capped on its own; without this, a program sending
/// `m=1` chunks forever would grow the buffer without end.
pub(crate) const KITTY_MAX_TRANSFER_BYTES: usize = 64 * 1024 * 1024;

/// `f=`: what the data is. PNG is the protocol's default.
const FORMAT_JPEG: u32 = 1;
const FORMAT_PNG: u32 = 100;
const FORMAT_RGB: u32 = 24;
const FORMAT_RGBA: u32 = 32;

/// `t=`: data sent inline, in the command itself. The protocol's default.
const MEDIUM_DIRECT: char = 'd';

/// `q=`: how much the program wants to hear back. At 1 a success is not
/// answered; at 2 a failure is not either.
const QUIET_OK: u32 = 1;
const QUIET_ALL: u32 = 2;

/// The protocol's answer to a command that worked.
const REPLY_OK: &str = "OK";
/// The answers to the failures Winter can have.
const REPLY_BAD_DATA: &str = "EINVAL:the image data could not be decoded";
const REPLY_UNSUPPORTED_MEDIUM: &str = "EINVAL:only direct transmission is supported";

const ESC: char = '\x1b';

// ========================================================================
// Data Structures
// ========================================================================

/// What one finished command comes to: an image to show, an answer to send
/// back to the program, both, or neither.
#[derive(Debug, Default, Eq, PartialEq)]
pub(crate) struct KittyOutcome {
    pub(crate) image: Option<KittyImage>,
    pub(crate) reply: Option<String>,
}

/// An image ready to show, encoded in a format a block can carry.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct KittyImage {
    pub(crate) bytes: Vec<u8>,
    pub(crate) mime: &'static str,
}

/// The commands of one transfer, gathered until its last chunk arrives.
#[derive(Debug, Default)]
pub(crate) struct KittyTransfer {
    /// Base64 data gathered so far.
    data: Vec<u8>,
    /// The controls of the transfer's first chunk, which speak for all of it.
    header: Option<Header>,
}

/// The controls of a command that matter here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Header {
    action: Action,
    format: u32,
    height: u32,
    /// `i=`: the image's id, which a reply has to name. A command without
    /// one is never answered.
    id: Option<u32>,
    medium: char,
    quiet: u32,
    width: u32,
}

/// One command's controls, and whether more chunks of its transfer follow.
struct ParsedControls {
    header: Header,
    more: bool,
}

/// `a=`: what a command asks for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    /// Anything acting on a stored image: put, delete, animate, compose.
    Ignored,
    /// `a=q`: would this image load? Answered, never shown.
    Query,
    /// `a=t` or `a=T`: here is an image.
    Transmit,
}

// ========================================================================
// Action
// ========================================================================

impl Action {
    fn from_value(value: &str) -> Self {
        match value {
            "q" => Self::Query,
            "t" | "T" => Self::Transmit,
            _ => Self::Ignored,
        }
    }
}

// ========================================================================
// Header
// ========================================================================

impl Header {
    /// Read the controls of a command; anything not understood keeps the
    /// protocol's default.
    fn parse(controls: &str) -> ParsedControls {
        let mut header = Header {
            action: Action::Transmit,
            format: FORMAT_PNG,
            height: 0,
            id: None,
            medium: MEDIUM_DIRECT,
            quiet: 0,
            width: 0,
        };
        let mut more = false;
        for (key, value) in controls.split(',').filter_map(|kv| kv.split_once('=')) {
            let value = value.trim();
            let number = || value.parse::<u32>().ok();
            match key.trim() {
                "a" => header.action = Action::from_value(value),
                "f" => header.format = number().unwrap_or(FORMAT_PNG),
                "i" => header.id = number(),
                "m" => more = number() == Some(1),
                "q" => header.quiet = number().unwrap_or(0),
                "s" => header.width = number().unwrap_or(0),
                "t" => header.medium = value.chars().next().unwrap_or(MEDIUM_DIRECT),
                "v" => header.height = number().unwrap_or(0),
                _ => {}
            }
        }
        ParsedControls { header, more }
    }

    /// What the whole transfer, its base64 `data` gathered, comes to.
    fn finish(self, data: &[u8]) -> KittyOutcome {
        let built = match (self.action, self.medium) {
            (Action::Ignored, _) => return KittyOutcome::default(),
            (_, MEDIUM_DIRECT) => self.build(data).ok_or(REPLY_BAD_DATA),
            (_, _) => Err(REPLY_UNSUPPORTED_MEDIUM),
        };
        let reply = match &built {
            Ok(_) => self.reply(REPLY_OK, QUIET_OK),
            Err(problem) => self.reply(problem, QUIET_ALL),
        };
        let image = match self.action {
            Action::Transmit => built.ok(),
            Action::Ignored | Action::Query => None,
        };
        KittyOutcome { image, reply }
    }

    /// The answer `message`, unless the command has no id to answer or asked,
    /// at `quiet` or above, not to hear it.
    fn reply(&self, message: &str, silenced_at: u32) -> Option<String> {
        let id = self.id?;
        (self.quiet < silenced_at).then(|| format!("{ESC}_Gi={id};{message}{ESC}\\"))
    }

    /// The image the base64 `data` holds, or `None` when it holds none.
    fn build(&self, data: &[u8]) -> Option<KittyImage> {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(data)
            .ok()
            .filter(|decoded| !decoded.is_empty())?;
        let (mime, bytes) = match self.format {
            FORMAT_PNG => ("image/png", decoded),
            FORMAT_JPEG => ("image/jpeg", decoded),
            FORMAT_RGBA => (
                "image/png",
                encode_png(DynamicImage::ImageRgba8(RgbaImage::from_raw(
                    self.width,
                    self.height,
                    decoded,
                )?))?,
            ),
            FORMAT_RGB => (
                "image/png",
                encode_png(DynamicImage::ImageRgb8(RgbImage::from_raw(
                    self.width,
                    self.height,
                    decoded,
                )?))?,
            ),
            _ => return None,
        };
        let is_readable = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .ok()
            .and_then(|reader| reader.into_dimensions().ok())
            .is_some_and(|(w, h)| w > 0 && h > 0);
        is_readable.then_some(KittyImage { bytes, mime })
    }
}

// ========================================================================
// KittyTransfer
// ========================================================================

impl KittyTransfer {
    /// Take one command's payload (what follows the `G`) and say what it comes
    /// to. A chunk that is not the last of its transfer comes to nothing yet.
    pub(crate) fn accept(&mut self, payload: &[u8]) -> KittyOutcome {
        let Ok(text) = std::str::from_utf8(payload) else {
            return KittyOutcome::default();
        };
        let (controls, data) = text.split_once(';').unwrap_or((text, ""));
        let parsed = Header::parse(controls);
        let header = *self.header.get_or_insert(parsed.header);
        if self.data.len() + data.len() > KITTY_MAX_TRANSFER_BYTES {
            // Too large to be an image worth drawing: the transfer is
            // dropped, and what follows starts afresh.
            *self = Self::default();
            return KittyOutcome::default();
        }
        self.data.extend_from_slice(data.as_bytes());
        if parsed.more {
            return KittyOutcome::default();
        }
        let data = std::mem::take(&mut self.data);
        self.header = None;
        header.finish(&data)
    }
}

// ========================================================================
// Encoding
// ========================================================================

/// `image` as PNG bytes, the form every raster block is carried in. Shared
/// with the Sixel path.
pub(crate) fn encode_png(image: DynamicImage) -> Option<Vec<u8>> {
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
        .ok()?;
    Some(png)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1x1 RGB image's data, the probe programs send to ask for support.
    const ONE_PIXEL_RGB: &str = "AAAA";

    fn accept(transfer: &mut KittyTransfer, command: &str) -> KittyOutcome {
        transfer.accept(command.as_bytes())
    }

    #[test]
    fn test_a_delete_command_shows_nothing_and_says_nothing() {
        // Regression: an `a=d` with no data decoded to an empty "PNG", which
        // was emitted as a block and reserved a dozen blank rows.
        let mut transfer = KittyTransfer::default();
        assert_eq!(accept(&mut transfer, "a=d"), KittyOutcome::default());
        assert_eq!(
            accept(&mut transfer, "a=d,d=A,i=3"),
            KittyOutcome::default()
        );
    }

    #[test]
    fn test_a_support_query_is_answered_and_never_shown() {
        let mut transfer = KittyTransfer::default();
        let outcome = accept(
            &mut transfer,
            &format!("i=31,s=1,v=1,a=q,t=d,f=24;{ONE_PIXEL_RGB}"),
        );
        assert_eq!(outcome.image, None);
        assert_eq!(outcome.reply.as_deref(), Some("\x1b_Gi=31;OK\x1b\\"));
    }

    #[test]
    fn test_a_query_for_a_file_says_the_medium_is_not_supported() {
        let mut transfer = KittyTransfer::default();
        let outcome = accept(&mut transfer, "i=4,a=q,t=f;L3RtcC94");
        assert_eq!(outcome.image, None);
        let reply = outcome.reply.expect("an id asks for an answer");
        assert!(reply.contains("EINVAL"), "{reply}");
    }

    #[test]
    fn test_quiet_suppresses_the_answers_it_names() {
        let mut transfer = KittyTransfer::default();
        let ok = format!("i=1,a=q,f=24,s=1,v=1,q=1;{ONE_PIXEL_RGB}");
        assert_eq!(accept(&mut transfer, &ok).reply, None);
        assert!(accept(&mut transfer, "i=1,a=q,q=1;!!").reply.is_some());
        assert_eq!(accept(&mut transfer, "i=1,a=q,q=2;!!").reply, None);
    }

    #[test]
    fn test_a_transmitted_image_is_shown_whatever_its_chunking() {
        let mut transfer = KittyTransfer::default();
        assert_eq!(
            accept(&mut transfer, "a=T,f=24,s=1,v=1,m=1;AA"),
            KittyOutcome::default()
        );
        let outcome = accept(&mut transfer, "m=0;AA");
        let image = outcome.image.expect("the last chunk completes the image");
        assert_eq!(image.mime, "image/png");
        // No id: nothing to answer.
        assert_eq!(outcome.reply, None);
    }

    #[test]
    fn test_data_that_is_no_image_is_not_shown() {
        let mut transfer = KittyTransfer::default();
        // Valid base64, but not a PNG.
        assert_eq!(accept(&mut transfer, "a=T;aGVsbG8=").image, None);
        // Raw pixels that do not fill the stated size.
        assert_eq!(accept(&mut transfer, "a=T,f=32,s=2,v=2;AAAA").image, None);
    }
}
