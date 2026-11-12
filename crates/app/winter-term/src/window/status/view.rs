//! The status bar as misty draws it: the bar Winter builds for the focused
//! tab, turned into misty's [`StatusLine`] (where commands run, the project
//! and branch, the mode, the search or the prompt, the segments, the bell),
//! drawn with the notifier's message and tasks, and the pointer handed to
//! it.

use crate::render::{StatusBar, StatusHover, ThemeRgb};
use misty::text::{Span, StyledText, TextColor};
use misty::{Frame, Input, StatusBarResponse, StatusLine, StatusSegment, Svg, Widget};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What stands before a branch with no icon of its own: the codicon
/// `git-branch`.
const BRANCH_GLYPH: char = '\u{ea68}';

/// The id of the bell segment.
const BELL_ID: &str = "winter.status.bell";

/// The id of the mode segment.
const MODE_ID: &str = "winter.status.mode";

/// The ids of the right side's segments, by their index: misty names a
/// clickable segment by a static id.
const SEGMENT_IDS: [&str; 16] = [
    "winter.status.0",
    "winter.status.1",
    "winter.status.2",
    "winter.status.3",
    "winter.status.4",
    "winter.status.5",
    "winter.status.6",
    "winter.status.7",
    "winter.status.8",
    "winter.status.9",
    "winter.status.10",
    "winter.status.11",
    "winter.status.12",
    "winter.status.13",
    "winter.status.14",
    "winter.status.15",
];

// ========================================================================
// App: the status bar misty draws
// ========================================================================

impl App {
    /// Draw the status bar the last frame built along the bottom of
    /// `frame`, with the notifier's message and tasks.
    pub(in crate::window) fn draw_status_bar(&mut self, frame: &mut Frame) {
        let Some(view) = self.chrome.drawn_status.take() else {
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&crate::render::Theme::dark()),
        };
        let line = self.status_line(&view);
        let bounds = frame.bounds();
        self.chrome
            .status_bar
            .lend(&line, &self.chrome.notifier)
            .draw(frame, bounds, &theme);
        self.chrome.drawn_status = Some(view);
    }

    /// Hand pointer `input` to the status bar; whether it took it. The bell
    /// opens the history, a quota segment asks its provider again, and the
    /// progress item shows the tasks as cards.
    pub(in crate::window) fn point_at_status_bar(
        &mut self,
        input: &Input,
        cx: &mut misty::EventCx<'_>,
    ) -> bool {
        let Some(view) = self.chrome.drawn_status.take() else {
            return false;
        };
        let line = self.status_line(&view);
        let answer = self
            .chrome
            .status_bar
            .lend(&line, &self.chrome.notifier)
            .handle(input, cx);
        self.chrome.drawn_status = Some(view);
        let hover = self.chrome.status_bar.hovered().and_then(status_hover);
        if hover != self.chrome.status_hover {
            self.chrome.status_hover = hover;
            self.dirty = true;
        }
        match answer {
            StatusBarResponse::Clicked(id) => {
                match status_hover(id) {
                    Some(StatusHover::Bell) => self.open_notifications(),
                    Some(StatusHover::Segment(index)) => self.refresh_quota_segment(index),
                    Some(StatusHover::Mode) | None => {}
                }
                true
            }
            StatusBarResponse::Handled => true,
            StatusBarResponse::Ignored => false,
            StatusBarResponse::Progress => {
                self.chrome.notifier.show_task_cards();
                self.dirty = true;
                true
            }
        }
    }

    /// Misty's line for `view`, the bar Winter built, its icons put in the
    /// bar's set as they come.
    fn status_line(&mut self, view: &StatusBar) -> StatusLine {
        let primary = match &view.search {
            Some(search) => {
                let prefix = if search.reverse { '?' } else { '/' };
                let count = match search.match_total {
                    0 => String::new(),
                    total => format!("  {}/{total}", search.match_index),
                };
                StyledText::plain(
                    format!("{prefix}{}{count}", search.query),
                    TextColor::Token(misty::theme::ThemeToken::Foreground),
                )
            }
            None => StyledText::new(),
        };
        let mut line = StatusLine::new(primary);
        if let Some(prompt) = &view.prompt {
            line = line.with_prompt(prompt.label.as_str(), prompt.value.as_str(), prompt.caret);
        }
        if !view.host.is_empty() {
            line = line.with_left(StatusSegment::new(quiet(&view.host)));
        }
        if !view.mode.is_empty() {
            let mode = StyledText::from(Span::new(view.mode.trim(), color(view.accent)).bold());
            line = line.with_left(StatusSegment::new(mode).with_id(MODE_ID));
        }
        if !view.project.is_empty() {
            let project = self.segment(&view.project, view.project_icon.as_ref());
            line = line.with_left(project);
        }
        if !view.branch.is_empty() {
            let branch = match &view.branch_icon {
                Some(icon) => self.segment(&view.branch, Some(icon)),
                None => StatusSegment::new(quiet(&format!("{BRANCH_GLYPH} {}", view.branch))),
            };
            line = line.with_left(branch);
        }
        for (index, segment) in view.segments.iter().enumerate() {
            let text = segment.text.replace(crate::render::THIN_SPACE, " ");
            let mut shown = self.segment(&text, segment.icon.as_ref());
            shown = shown.with_priority(segment.priority);
            if let Some(short) = &segment.short {
                shown = shown.with_short(quiet(&short.replace(crate::render::THIN_SPACE, " ")));
            }
            if let Some(id) = SEGMENT_IDS.get(index) {
                shown = shown.with_id(id);
            }
            line = line.with_segment(shown);
        }
        if let Some(bell) = &view.bell {
            line = line.with_segment(StatusSegment::new(quiet(&bell.text)).with_id(BELL_ID));
        }
        line
    }

    /// A segment saying `text`, after `icon` when it has one.
    fn segment(&mut self, text: &str, icon: Option<&std::sync::Arc<[u8]>>) -> StatusSegment {
        let segment = StatusSegment::new(quiet(text));
        let Some(bytes) = icon else {
            return segment;
        };
        // Winter's icons are shared, one copy each, so where the copy lives
        // names it.
        let name = format!("winter.icon.{:p}", bytes.as_ptr());
        let icons = self.chrome.status_bar.icons_mut();
        if icons.get(&name).is_none() {
            if let Ok(svg) = Svg::parse(bytes) {
                icons.insert_colored(name.clone(), svg);
            }
        }
        segment.with_icon(name)
    }

    /// Where the status bar's part `part` was last drawn, for a tooltip
    /// hung over it.
    pub(in crate::window) fn status_part_region(
        &self,
        part: StatusHover,
    ) -> Option<crate::render::Region> {
        let id = match part {
            StatusHover::Bell => BELL_ID,
            StatusHover::Mode => MODE_ID,
            StatusHover::Segment(index) => SEGMENT_IDS.get(index)?,
        };
        let rect = self.chrome.status_bar.segment_rect(id)?;
        Some(crate::render::Region {
            h: rect.height,
            w: rect.width,
            x: rect.x,
            y: rect.y,
        })
    }

    /// The height misty draws the status bar at, in the font and scale of
    /// the last frame; `None` before the first.
    pub(in crate::window) fn status_bar_height(&self) -> Option<f32> {
        let window = self.window.as_ref()?;
        let metrics = window.cell_metrics()?;
        Some(misty::StatusBar::height(metrics, window.scale_factor()))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What Winter calls the status bar part named `id`.
fn status_hover(id: &str) -> Option<StatusHover> {
    match id {
        BELL_ID => Some(StatusHover::Bell),
        MODE_ID => Some(StatusHover::Mode),
        _ => SEGMENT_IDS
            .iter()
            .position(|segment| *segment == id)
            .map(StatusHover::Segment),
    }
}

/// `text` in the muted color, as the bar's quieter parts are.
fn quiet(text: &str) -> StyledText {
    StyledText::plain(text, TextColor::Token(misty::theme::ThemeToken::Muted))
}

/// Misty's text color for Winter's `color`.
fn color(color: ThemeRgb) -> TextColor {
    TextColor::Rgba(misty::color::Rgba::new(color.r, color.g, color.b, u8::MAX))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_each_part_is_known_back_by_its_id() {
        assert_eq!(status_hover(BELL_ID), Some(StatusHover::Bell));
        assert_eq!(status_hover(MODE_ID), Some(StatusHover::Mode));
        assert_eq!(status_hover(SEGMENT_IDS[3]), Some(StatusHover::Segment(3)));
        assert_eq!(status_hover("misty.status-bar.progress"), None);
    }
}
