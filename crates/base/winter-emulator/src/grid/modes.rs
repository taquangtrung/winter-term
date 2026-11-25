//! The terminal modes a program can set: DEC private modes (`CSI ? Pm h/l`)
//! and ANSI modes (`CSI Pm h/l`). One enum per family names every mode this
//! terminal implements, so setting, querying (DECRQM), saving (XTSAVE) and
//! dumping a mode all go through the same table instead of each keeping its
//! own list of numbers.

use super::Grid;

// ========================================================================
// Data Structures
// ========================================================================

/// A DEC private mode this terminal implements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecMode {
    /// `?1049`: the alternate screen, saving the cursor on the way in.
    AltScreen,
    /// `?1047`: the alternate screen, cleared when left.
    AltScreenClear,
    /// `?47`: xterm's oldest alternate-screen switch.
    AltScreenLegacy,
    /// `?1` (DECCKM): unmodified cursor keys send `SS3` forms (`ESC O A`)
    /// instead of `CSI` ones, which is what `kcuu1` and friends in the
    /// `xterm-256color` terminfo promise once a program enables keypad mode.
    AppCursorKeys,
    /// `?66` (DECNKM), also set by DECKPAM (`ESC =`) and reset by DECKPNM
    /// (`ESC >`): the keypad is in application mode.
    AppKeypad,
    /// `?7` (DECAWM): printing past the last column wraps to the next line.
    /// Off, the last column is overwritten in place.
    AutoWrap,
    /// `?2004`: pasted text is bracketed by `ESC [200~` / `ESC [201~`.
    BracketedPaste,
    /// `?25` (DECTCEM): the cursor is shown.
    CursorVisible,
    /// `?1004`: focus changes are reported as `ESC [I` / `ESC [O`.
    FocusEvent,
    /// `?2027`: text is laid out by grapheme cluster, so an emoji ZWJ
    /// sequence or a VS16 emoji takes the width of the one glyph it draws
    /// rather than the sum of its code points. Always on here: printing
    /// merges these clusters unconditionally, so it is reported as
    /// permanently set and a program cannot turn it off.
    GraphemeClusters,
    /// `?1003`: every pointer motion is reported, buttons down or not.
    MouseAnyEvent,
    /// `?1000`: button presses and releases are reported.
    MouseButton,
    /// `?1002`: motion with a button held is reported too.
    MouseDrag,
    /// `?1006`: mouse reports use the SGR encoding.
    MouseSgr,
    /// `?6` (DECOM): cursor addressing is relative to the scroll region.
    Origin,
    /// `?5` (DECSCNM): the whole screen is drawn in reverse video.
    ReverseVideo,
    /// `?1048`: set saves the cursor, reset restores it. An action rather
    /// than a state, so it has no value to report or save.
    SaveCursor,
    /// `?2026`: the program is part-way through a frame it wants shown whole.
    SyncOutput,
}

/// An ANSI (non-private) mode this terminal implements.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnsiMode {
    /// `4` (IRM): printing inserts, shifting the rest of the line right,
    /// instead of overwriting.
    Insert,
}

/// The value of every mode that is a plain flag. The alternate-screen modes
/// are not here: whether the alternate screen is up is the grid's own
/// buffer state, not a flag beside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ModeFlags {
    pub(super) app_cursor_keys: bool,
    pub(super) app_keypad: bool,
    pub(super) auto_wrap: bool,
    pub(super) bracketed_paste: bool,
    pub(super) cursor_visible: bool,
    pub(super) focus_event: bool,
    pub(super) insert: bool,
    pub(super) mouse_any_event: bool,
    pub(super) mouse_button: bool,
    pub(super) mouse_drag: bool,
    pub(super) mouse_sgr: bool,
    pub(super) origin: bool,
    pub(super) reverse_video: bool,
    pub(super) sync_output: bool,
}

// ========================================================================
// DecMode
// ========================================================================

impl DecMode {
    /// Every DEC private mode, for looking one up by its number.
    pub const ALL: [Self; 18] = [
        Self::AltScreen,
        Self::AltScreenClear,
        Self::AltScreenLegacy,
        Self::AppCursorKeys,
        Self::AppKeypad,
        Self::AutoWrap,
        Self::BracketedPaste,
        Self::CursorVisible,
        Self::FocusEvent,
        Self::GraphemeClusters,
        Self::MouseAnyEvent,
        Self::MouseButton,
        Self::MouseDrag,
        Self::MouseSgr,
        Self::Origin,
        Self::ReverseVideo,
        Self::SaveCursor,
        Self::SyncOutput,
    ];

    /// The mode's number in `CSI ? Pm h`.
    pub fn code(self) -> u16 {
        match self {
            Self::AltScreen => 1049,
            Self::AltScreenClear => 1047,
            Self::AltScreenLegacy => 47,
            Self::AppCursorKeys => 1,
            Self::AppKeypad => 66,
            Self::AutoWrap => 7,
            Self::BracketedPaste => 2004,
            Self::CursorVisible => 25,
            Self::FocusEvent => 1004,
            Self::GraphemeClusters => 2027,
            Self::MouseAnyEvent => 1003,
            Self::MouseButton => 1000,
            Self::MouseDrag => 1002,
            Self::MouseSgr => 1006,
            Self::Origin => 6,
            Self::ReverseVideo => 5,
            Self::SaveCursor => 1048,
            Self::SyncOutput => 2026,
        }
    }

    /// The mode numbered `code`, or `None` for one this terminal does not
    /// implement.
    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.code() == code)
    }

    /// Whether the mode holds a value a program can change (as opposed to
    /// [`Self::SaveCursor`], which performs an action each time it is set or
    /// reset, and a permanent mode, which never changes).
    pub fn is_stateful(self) -> bool {
        self != Self::SaveCursor && !self.is_permanent()
    }

    /// Whether the mode is fixed by this terminal, so setting or resetting
    /// it changes nothing (DECRQM reports it as permanently set or reset).
    pub fn is_permanent(self) -> bool {
        self == Self::GraphemeClusters
    }
}

// ========================================================================
// AnsiMode
// ========================================================================

impl AnsiMode {
    /// The mode's number in `CSI Pm h`.
    pub fn code(self) -> u16 {
        match self {
            Self::Insert => 4,
        }
    }

    /// The mode numbered `code`, or `None` for one this terminal does not
    /// implement.
    pub fn from_code(code: u16) -> Option<Self> {
        (code == Self::Insert.code()).then_some(Self::Insert)
    }
}

// ========================================================================
// ModeFlags
// ========================================================================

impl ModeFlags {
    /// Reset what DECSTR (soft terminal reset) resets: the cursor is shown,
    /// insert and origin modes end, autowrap returns, and the cursor keys go
    /// back to their `CSI` forms, and the keypad to numeric mode. Mouse, paste and focus reporting are the
    /// program's standing requests and survive, as they do in xterm.
    pub(super) fn soft_reset(&mut self) {
        let defaults = Self::default();
        self.app_cursor_keys = defaults.app_cursor_keys;
        self.app_keypad = defaults.app_keypad;
        self.auto_wrap = defaults.auto_wrap;
        self.cursor_visible = defaults.cursor_visible;
        self.insert = defaults.insert;
        self.origin = defaults.origin;
    }
}

impl Default for ModeFlags {
    fn default() -> Self {
        Self {
            app_cursor_keys: false,
            app_keypad: false,
            auto_wrap: true,
            bracketed_paste: false,
            cursor_visible: true,
            focus_event: false,
            insert: false,
            mouse_any_event: false,
            mouse_button: false,
            mouse_drag: false,
            mouse_sgr: false,
            origin: false,
            reverse_video: false,
            sync_output: false,
        }
    }
}

// ========================================================================
// Grid: modes
// ========================================================================

impl Grid {
    /// Whether DEC private mode `mode` is set. [`DecMode::SaveCursor`] holds
    /// no value and always reads as reset.
    pub fn dec_mode(&self, mode: DecMode) -> bool {
        match mode {
            DecMode::AltScreen | DecMode::AltScreenClear | DecMode::AltScreenLegacy => {
                self.is_alt_screen()
            }
            DecMode::AppCursorKeys => self.modes.app_cursor_keys,
            DecMode::AppKeypad => self.modes.app_keypad,
            DecMode::AutoWrap => self.modes.auto_wrap,
            DecMode::BracketedPaste => self.modes.bracketed_paste,
            DecMode::CursorVisible => self.modes.cursor_visible,
            DecMode::FocusEvent => self.modes.focus_event,
            DecMode::GraphemeClusters => true,
            DecMode::MouseAnyEvent => self.modes.mouse_any_event,
            DecMode::MouseButton => self.modes.mouse_button,
            DecMode::MouseDrag => self.modes.mouse_drag,
            DecMode::MouseSgr => self.modes.mouse_sgr,
            DecMode::Origin => self.modes.origin,
            DecMode::ReverseVideo => self.modes.reverse_video,
            DecMode::SaveCursor => false,
            DecMode::SyncOutput => self.modes.sync_output,
        }
    }

    /// Set (DECSET) or reset (DECRST) DEC private mode `mode`.
    pub fn set_dec_mode(&mut self, mode: DecMode, set: bool) {
        match mode {
            DecMode::AltScreen | DecMode::AltScreenClear | DecMode::AltScreenLegacy => {
                if set {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            DecMode::AppCursorKeys => self.modes.app_cursor_keys = set,
            DecMode::AppKeypad => self.modes.app_keypad = set,
            DecMode::AutoWrap => {
                self.modes.auto_wrap = set;
                // A wrap deferred under autowrap must not fire once it is off.
                if !set {
                    self.cursor.wrap_pending = false;
                }
            }
            DecMode::BracketedPaste => self.modes.bracketed_paste = set,
            DecMode::CursorVisible => self.modes.cursor_visible = set,
            DecMode::FocusEvent => self.modes.focus_event = set,
            DecMode::GraphemeClusters => {}
            DecMode::MouseAnyEvent => self.modes.mouse_any_event = set,
            DecMode::MouseButton => self.modes.mouse_button = set,
            DecMode::MouseDrag => self.modes.mouse_drag = set,
            DecMode::MouseSgr => self.modes.mouse_sgr = set,
            // DECOM: positioning becomes relative to the scroll region and the
            // mode change homes the cursor to that region's top-left.
            DecMode::Origin => {
                self.modes.origin = set;
                self.move_to(0, 0);
            }
            DecMode::ReverseVideo => self.modes.reverse_video = set,
            DecMode::SaveCursor => {
                if set {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            DecMode::SyncOutput => self.modes.sync_output = set,
        }
    }

    /// Set or reset the DEC private mode numbered `code`; a number this
    /// terminal does not implement is ignored.
    pub fn set_private_mode(&mut self, code: u16, set: bool) {
        if let Some(mode) = DecMode::from_code(code) {
            self.set_dec_mode(mode, set);
        }
    }

    /// Whether ANSI mode `mode` is set.
    pub fn ansi_mode(&self, mode: AnsiMode) -> bool {
        match mode {
            AnsiMode::Insert => self.modes.insert,
        }
    }

    /// Set (SM) or reset (RM) ANSI mode `mode`.
    pub fn set_ansi_mode(&mut self, mode: AnsiMode, set: bool) {
        match mode {
            AnsiMode::Insert => self.modes.insert = set,
        }
    }

    /// Remember the current value of each DEC private mode in `codes`
    /// (XTSAVE, `CSI ? Pm s`) for a later [`Self::restore_dec_modes`].
    /// Unknown numbers and modes with no value are skipped.
    pub fn save_dec_modes(&mut self, codes: impl IntoIterator<Item = u16>) {
        for mode in codes.into_iter().filter_map(DecMode::from_code) {
            if !mode.is_stateful() {
                continue;
            }
            let value = self.dec_mode(mode);
            match self
                .saved_modes
                .iter_mut()
                .find(|(saved, _)| *saved == mode)
            {
                Some(entry) => entry.1 = value,
                None => self.saved_modes.push((mode, value)),
            }
        }
    }

    /// Put back each DEC private mode in `codes` that [`Self::save_dec_modes`]
    /// recorded (XTRESTORE, `CSI ? Pm r`). A mode never saved is left alone.
    pub fn restore_dec_modes(&mut self, codes: impl IntoIterator<Item = u16>) {
        for mode in codes.into_iter().filter_map(DecMode::from_code) {
            let saved = self
                .saved_modes
                .iter()
                .find(|(saved, _)| *saved == mode)
                .map(|&(_, value)| value);
            if let Some(value) = saved {
                self.set_dec_mode(mode, value);
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_dec_mode_round_trips_through_its_code() {
        // A code typed twice in `code`, or a variant missing from `ALL`, would
        // make one mode unreachable from the byte stream.
        for mode in DecMode::ALL {
            assert_eq!(DecMode::from_code(mode.code()), Some(mode), "{mode:?}");
        }
        assert_eq!(DecMode::from_code(9999), None);
    }

    #[test]
    fn test_xtrestore_puts_back_the_saved_value_not_the_current_one() {
        let mut grid = Grid::new(10, 3);
        grid.set_dec_mode(DecMode::AutoWrap, false);
        grid.save_dec_modes([DecMode::AutoWrap.code()]);
        grid.set_dec_mode(DecMode::AutoWrap, true);
        grid.restore_dec_modes([DecMode::AutoWrap.code()]);
        assert!(!grid.dec_mode(DecMode::AutoWrap));
    }

    #[test]
    fn test_xtsave_of_the_alt_screen_does_not_leave_it() {
        // Regression: `CSI ? 1049 s` was read as a reset, dropping a
        // full-screen app out of its alternate screen.
        let mut grid = Grid::new(10, 3);
        grid.set_dec_mode(DecMode::AltScreen, true);
        grid.save_dec_modes([DecMode::AltScreen.code()]);
        assert!(grid.is_alt_screen());
        grid.set_dec_mode(DecMode::AltScreen, false);
        grid.restore_dec_modes([DecMode::AltScreen.code()]);
        assert!(
            grid.is_alt_screen(),
            "restore re-enters the saved alt screen"
        );
    }

    #[test]
    fn test_restoring_a_mode_never_saved_leaves_it_alone() {
        let mut grid = Grid::new(10, 3);
        grid.set_dec_mode(DecMode::CursorVisible, false);
        grid.restore_dec_modes([DecMode::CursorVisible.code()]);
        assert!(!grid.dec_mode(DecMode::CursorVisible));
    }

    #[test]
    fn test_soft_reset_keeps_the_programs_reporting_requests() {
        let mut flags = ModeFlags {
            app_cursor_keys: true,
            app_keypad: true,
            auto_wrap: false,
            bracketed_paste: true,
            cursor_visible: false,
            insert: true,
            mouse_sgr: true,
            ..ModeFlags::default()
        };
        flags.soft_reset();
        assert!(!flags.app_cursor_keys && flags.auto_wrap && flags.cursor_visible);
        assert!(!flags.app_keypad);
        assert!(!flags.insert);
        assert!(flags.bracketed_paste && flags.mouse_sgr);
    }

    #[test]
    fn test_grapheme_clusters_are_permanently_on() {
        let mut grid = Grid::new(10, 3);
        grid.set_dec_mode(DecMode::GraphemeClusters, false);
        assert!(grid.dec_mode(DecMode::GraphemeClusters));
        assert!(!DecMode::GraphemeClusters.is_stateful(), "nothing to save");
    }
}
