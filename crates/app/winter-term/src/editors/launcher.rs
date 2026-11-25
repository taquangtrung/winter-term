//! A launcher: what a split opens in its new pane, holding the place for a
//! terminal, a file, a directory listing, or the git view, opened in the
//! directory the pane split from. Choosing one opens it in the launcher's tab.

use std::path::PathBuf;
use std::time::Instant;

use misty::layout::{Align, Justify};
use misty::view::{Extent, column, item, proportional_keycap, section};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect, Theme};

use winter_model::editor::{Editor, EditorOutcome, Launch, OpenTarget, Page, PageView};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::paint::{Frame, Row};
use winter_model::path::abbreviate_home;

// ========================================================================
// Constants
// ========================================================================

/// The editor's title.
const TITLE: &str = "New Pane";

/// The heading over the choices.
const CHOICES: &str = "Open here";

/// What each key opens, in the order the page lists them.
const KEYS: KeyTable<Launch> = KeyTable {
    block: "launcher",
    bindings: &[
        Binding {
            action: Launch::Terminal,
            id: "terminal",
            keys: &["RET", "t"],
            label: "Terminal here",
        },
        Binding {
            action: Launch::Files,
            id: "files",
            keys: &["f"],
            label: "Open file…",
        },
        Binding {
            action: Launch::Dir,
            id: "dir",
            keys: &["d"],
            label: "Directory",
        },
        Binding {
            action: Launch::Git,
            id: "git",
            keys: &["g"],
            label: "Git status",
        },
    ],
    configurable: true,
    title: "New Pane",
};

/// Every key table of this surface, for the Keys editor to list.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

/// How wide the card of choices is, in cell columns.
const CARD_COLS: f32 = 56.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A pane's placeholder until what it holds is chosen.
pub struct LauncherEditor {
    /// The page's view: where what is chosen opens, and what a click chose.
    view: PageView<Choices>,
}

/// What the launcher offers and what a click on it chose.
struct Choices {
    /// Where what is chosen opens.
    dir: PathBuf,
    /// The choice a click made, until the window takes it.
    chosen: Option<Launch>,
}

// ========================================================================
// LauncherEditor
// ========================================================================

impl LauncherEditor {
    /// The launcher for `dir`.
    pub fn new(dir: PathBuf) -> Self {
        Self {
            view: PageView::new(Choices { chosen: None, dir }),
        }
    }
}

impl Page for LauncherEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// Misty draws the page; the grid under it is empty.
    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.view.draw_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = self.view.view_input(input, cx);
        let chosen = self.view.state_mut().chosen.take();
        chosen
            .map(EditorOutcome::Launch)
            .or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = self.view.access_action(action);
        let chosen = self.view.state_mut().chosen.take();
        chosen
            .map(EditorOutcome::Launch)
            .or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        match table::find(&KEYS, key) {
            Some(launch) => EditorOutcome::Launch(launch),
            None => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn cwd(&self) -> Option<PathBuf> {
        Some(self.view.state().dir.clone())
    }
}

impl HostedView for Choices {
    /// A card headed by the directory, of what may open there, centered in
    /// the pane and no wider than it, each choice beside its key and opened by a click; a long
    /// directory cut short says itself whole in its tooltip.
    fn view(&self) -> Element<'_, Self> {
        let choices = KEYS.iter().map(|binding| {
            let launch = binding.action;
            item(binding.label)
                .trailing(proportional_keycap(binding.keys[0]))
                .on_press(move |choices: &mut Self, _| choices.chosen = Some(launch))
                .into()
        });
        let card = column([section(abbreviate_home(&self.dir))
            .description(CHOICES)
            .items(choices)
            .into()])
        // As wide as the pane up to its own width, so a narrow pane or a
        // sidebar shows the card whole rather than spilling past its edges.
        .width(Extent::Fraction(1.0))
        .max_width(Extent::Cells(CARD_COLS));
        column([card])
            .align(Align::Center)
            .justify(Justify::Center)
            .grow(1.0)
    }
}

impl Editor for LauncherEditor {
    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.view.state().dir))
    }

    /// A launcher only holds an empty pane's place: there is nothing to
    /// come back to.
    fn is_kept_when_closed(&self) -> bool {
        false
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        Some(OpenTarget::file(self.view.state().dir.clone()))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::testing::TestWindow;

    use super::*;
    use crate::editors::view_tests::{click, drawn, middle_of, text_of};

    #[test]
    fn test_every_key_spec_parses() {
        assert!(table::invalid_specs(&KEYS).is_empty());
    }

    #[test]
    fn test_the_card_names_the_directory_and_each_choice_with_its_key() {
        let mut editor = LauncherEditor::new(PathBuf::from("/test"));
        let drawn = text_of(&drawn(&mut editor, &TestWindow::new(900, 600)));
        for words in ["/test", CHOICES, "Terminal here", "RET", "Git status", "g"] {
            assert!(drawn.contains(words), "{words:?} in {drawn:?}");
        }
    }

    #[test]
    fn test_a_click_on_a_choice_launches_it_and_keys_launch_theirs() {
        let mut editor = LauncherEditor::new(PathBuf::from("/test"));
        let mut window = TestWindow::new(900, 600);
        let point = middle_of(&drawn(&mut editor, &window), "Git status g");
        let (pressed, released) = click(&mut editor, &mut window, point);
        let launched = Some(EditorOutcome::Launch(Launch::Git));
        assert!(
            [&pressed, &released].contains(&&launched),
            "{pressed:?} then {released:?}"
        );
        assert_eq!(
            editor.on_key(&Key::plain(winter_model::input::KeyCode::Char('d'))),
            EditorOutcome::Launch(Launch::Dir)
        );
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut editor = LauncherEditor::new(PathBuf::from("/test"));
        let frame = crate::editors::view_tests::drawn(&mut editor, &TestWindow::new(1000, 800));
        let unnamed = misty::testing::unnamed_controls(&frame);
        assert!(unnamed.is_empty(), "{unnamed:?}");
        let mut window = TestWindow::new(1000, 800);
        let unlit = crate::editors::view_tests::unlit(&mut editor, &mut window);
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}
