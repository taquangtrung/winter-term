//! Position histories a Vim surface walks with `C-o`/`C-i` and `g;`/`g,`:
//! the jump list and the change list. Plain positions, nothing about grids,
//! so any surface that moves a cursor can keep them.

// ========================================================================
// Constants
// ========================================================================

/// How many jump origins a pane's [`JumpList`] keeps; older entries fall off
/// the front. Matches the addon-length scale vim itself uses.
const MAX_JUMP_LIST_LEN: usize = 100;

/// How many change positions a pane's [`ChangeList`] keeps; older entries
/// fall off the front, matching vim's own changelist length.
const MAX_CHANGE_LIST_LEN: usize = 100;

// ========================================================================
// Data Structures
// ========================================================================

/// A per-pane, vim-style jumplist: the absolute positions (row, col) the
/// cursor was at before each "long" jump: `gg`/`G`, `H`/`M`/`L`, `{`/`}`,
/// `%`, a confirmed search, an `f`/`t` label jump, leaving Normal for Insert,
/// oldest first. `C-o` walks back through them, `C-i`/`Tab` forward again.
#[derive(Default)]
pub(crate) struct JumpList {
    /// Jump origins, oldest first, capped at [`MAX_JUMP_LIST_LEN`].
    entries: Vec<(usize, usize)>,
    /// How far back from the newest entry the cursor currently sits; 0 means
    /// "at the live position, no jump navigation active": the state every
    /// push resets to.
    index: usize,
    /// The live position [`JumpList::older`] stepped away from, so
    /// [`JumpList::newer`] can return to it after walking all the way back,
    /// vim appends the current position the first time `C-o` is pressed.
    return_pos: Option<(usize, usize)>,
}

/// A per-pane, vim-style changelist: the absolute positions where recorded
/// changes began: Normal-mode edits (`x`, `dw`, `cs`, puts) and Insert-mode
/// typing sessions: oldest first. `g;` walks back through them
/// ([`App::change_older`](crate::window::App::change_older)), `g,` forward again ([`App::change_newer`](crate::window::App::change_newer)).
///
/// Wraps [`JumpList`]'s walk state (index and return position) but pushes
/// append-only: a changelist is a log of historical facts, so making a new
/// change while stepped back into the past keeps the entries ahead, unlike
/// the jumplist's undo-history forking. Consecutive duplicate positions
/// collapse so repeated edits at one spot (and `.`'s own replays) don't fill
/// the log with noise.
#[derive(Default)]
pub(crate) struct ChangeList(JumpList);

// ========================================================================
// JumpList Implementations
// ========================================================================

impl JumpList {
    /// Record `origin` (absolute) as the newest jump origin. Stepping back
    /// first forks away everything the cursor had stepped back past, exactly
    /// like an undo history: a new jump from the past discards the abandoned
    /// future.
    pub(crate) fn push(&mut self, origin: (usize, usize)) {
        if self.index > 0 {
            let keep = self.entries.len() - self.index;
            self.entries.truncate(keep);
            self.index = 0;
            self.return_pos = None;
        }
        self.entries.push(origin);
        if self.entries.len() > MAX_JUMP_LIST_LEN {
            self.entries.remove(0);
        }
    }

    /// Number of entries currently in the jumplist.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// The origin `C-o` lands on next, or `None` when already at the oldest
    /// entry. `live` is the current non-jumplist position, remembered so the
    /// matching `C-i` can come back to it.
    pub(crate) fn older(&mut self, live: (usize, usize)) -> Option<(usize, usize)> {
        if self.entries.is_empty() {
            return None;
        }
        if self.index == 0 {
            self.return_pos = Some(live);
        }
        if self.index < self.entries.len() {
            self.index += 1;
        }
        self.entries.get(self.entries.len() - self.index).copied()
    }

    /// The position `C-i` lands on next: the next entry forward, or the
    /// remembered live position once the walk is exhausted. `None` when
    /// already at the live position.
    pub(crate) fn newer(&mut self) -> Option<(usize, usize)> {
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        if self.index == 0 {
            self.return_pos.take()
        } else {
            self.entries.get(self.entries.len() - self.index).copied()
        }
    }

    /// Push every recorded row through a resize reflow's row remap: the
    /// positions are absolute rows into a live screen the reflow just
    /// rebuilt, and one left at its old row names the wrong line.
    pub(crate) fn remap_rows(&mut self, map: &dyn Fn(usize) -> usize) {
        for (row, _) in &mut self.entries {
            *row = map(*row);
        }
        if let Some((row, _)) = &mut self.return_pos {
            *row = map(*row);
        }
    }
}

// ========================================================================
// ChangeList Implementations
// ========================================================================

impl ChangeList {
    /// Push every recorded row through a resize reflow's row remap, like
    /// [`JumpList::remap_rows`].
    pub(crate) fn remap_rows(&mut self, map: &dyn Fn(usize) -> usize) {
        self.0.remap_rows(map);
    }

    /// Record `pos` as the newest change position, collapsing it when it
    /// matches the previous one and resetting the walk to the live position.
    pub(crate) fn push(&mut self, pos: (usize, usize)) {
        if self.0.entries.last() == Some(&pos) {
            return;
        }
        self.0.entries.push(pos);
        if self.0.entries.len() > MAX_CHANGE_LIST_LEN {
            self.0.entries.remove(0);
        }
        self.0.index = 0;
        self.0.return_pos = None;
    }

    /// The position `g;` lands on next: [`JumpList::older`]'s walk, shared
    /// with the jumplist so the two lists step identically.
    pub(crate) fn older(&mut self, live: (usize, usize)) -> Option<(usize, usize)> {
        self.0.older(live)
    }

    /// The position `g,` lands on next: [`JumpList::newer`]'s walk.
    pub(crate) fn newer(&mut self) -> Option<(usize, usize)> {
        self.0.newer()
    }
}
