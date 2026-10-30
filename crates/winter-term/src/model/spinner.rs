//! The spinner busy work wears: braille dots circling their cell, on a busy
//! tab's strip title and on a directory whose size is being walked.

use std::time::Duration;

// ========================================================================
// Constants
// ========================================================================

/// How long one frame of the spinner shows.
pub const FRAME: Duration = Duration::from_millis(80);

/// The spinner's frames, three braille dots circling their cell.
const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

// ========================================================================
// Functions
// ========================================================================

/// The glyph the spinner shows on `frame`, which may count past the last.
pub fn glyph(frame: usize) -> char {
    FRAMES[frame % FRAMES.len()]
}

/// The frame a spinner is on `elapsed` after its epoch, so every spinner
/// counted from the same epoch turns in step. Left unwrapped; [`glyph`]
/// wraps it.
pub fn frame_at(elapsed: Duration) -> usize {
    (elapsed.as_millis() / FRAME.as_millis()) as usize
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    #[test]
    fn test_the_spinner_turns_through_every_frame_and_wraps() {
        let len = FRAMES.len();
        let turn = |step: usize| glyph(frame_at(FRAME * step as u32));
        let seen: HashSet<char> = (0..len).map(turn).collect();
        assert_eq!(seen.len(), len);
        assert_eq!(turn(len), turn(0));
    }
}
