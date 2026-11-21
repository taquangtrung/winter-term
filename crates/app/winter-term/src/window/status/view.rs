//! The status bar as misty draws it: the line Winter builds for the
//! focused tab, drawn with the notifier's message and tasks, and the
//! pointer handed to it.

use crate::render::StatusHover;
use misty::{Frame, Input, StatusBarResponse, Widget};

use super::{BELL_ID, BRANCH_ID, HOST_ID, MODE_ID, PROJECT_ID, SEGMENT_IDS};

// ========================================================================
// Constants
// ========================================================================

/// How big the status bar's and the tooltips' text is against the
/// terminal's: a step smaller, 13.5 pixels beside the default 15, so they
/// read as chrome rather than another line of output.
const SMALL_CHROME_FONT_RATIO: f32 = 13.5 / 15.0;

use crate::window::App;
use crate::window::misty_theme::misty_theme;

// ========================================================================
// App: the status bar misty draws
// ========================================================================

impl App {
    /// Draw the status bar the last frame built along the bottom of
    /// `frame`, with the notifier's message and tasks.
    pub(in crate::window) fn draw_status_bar(&mut self, frame: &mut Frame) {
        let Some(line) = &self.chrome.drawn_status else {
            return;
        };
        let theme = misty_theme(self.renderer.as_ref());
        let bounds = frame.bounds();
        let font = self.small_chrome_font();
        let bar = &mut self.chrome.status_bar;
        frame.with_font(&font, |frame| {
            bar.lend(line, &self.chrome.notifier)
                .draw(frame, bounds, theme);
        });
    }

    /// The font the status bar and the tooltips are drawn in: the
    /// chrome's, a step smaller.
    pub(in crate::window) fn small_chrome_font(&self) -> misty::Font {
        self.misty_font_at(self.config.font_size * SMALL_CHROME_FONT_RATIO)
    }

    /// Hand pointer `input` to the status bar; whether it took it. The bell
    /// opens the history, a quota segment asks its provider again, and the
    /// progress item shows the tasks as cards.
    pub(in crate::window) fn point_at_status_bar(
        &mut self,
        input: &Input,
        cx: &mut misty::EventCx<'_>,
    ) -> bool {
        let Some(line) = &self.chrome.drawn_status else {
            return false;
        };
        let answer = self
            .chrome
            .status_bar
            .lend(line, &self.chrome.notifier)
            .handle(input, cx);
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
                    Some(
                        StatusHover::Branch
                        | StatusHover::Host
                        | StatusHover::Mode
                        | StatusHover::Progress
                        | StatusHover::Project,
                    )
                    | None => {}
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
            // A response misty adds later is none the status bar acts on.
            _ => false,
        }
    }

    /// Where the status bar's part `part` was last drawn, for a tooltip
    /// hung over it.
    pub(in crate::window) fn status_part_region(
        &self,
        part: StatusHover,
    ) -> Option<crate::render::Region> {
        let id = match part {
            StatusHover::Bell => BELL_ID,
            StatusHover::Branch => BRANCH_ID,
            StatusHover::Host => HOST_ID,
            StatusHover::Mode => MODE_ID,
            StatusHover::Progress => misty::StatusSegment::PROGRESS_ID,
            StatusHover::Project => PROJECT_ID,
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

    /// The height misty draws the status bar at, in its font at the
    /// window's scale; `None` before the window opens.
    pub(in crate::window) fn status_bar_height(&self) -> Option<f32> {
        let window = self.window.as_ref()?;
        let scale_factor = window.scale_factor();
        let metrics = self.small_chrome_font().cell_metrics(scale_factor);
        Some(misty::StatusBar::height(metrics, scale_factor))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What Winter calls the status bar part named `id`.
fn status_hover(id: &str) -> Option<StatusHover> {
    match id {
        BELL_ID => Some(StatusHover::Bell),
        BRANCH_ID => Some(StatusHover::Branch),
        HOST_ID => Some(StatusHover::Host),
        MODE_ID => Some(StatusHover::Mode),
        PROJECT_ID => Some(StatusHover::Project),
        misty::StatusSegment::PROGRESS_ID => Some(StatusHover::Progress),
        _ => SEGMENT_IDS
            .iter()
            .position(|segment| *segment == id)
            .map(StatusHover::Segment),
    }
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
        assert_eq!(
            status_hover(misty::StatusSegment::PROGRESS_ID),
            Some(StatusHover::Progress)
        );
    }
}
