//! The open picker as misty's quick pick draws it over the frame: where it
//! goes, its rows' icons and colors, and its side panel, all read from the
//! picker view the frame worked out; and the pointer handed to it.

use std::collections::HashMap;

use misty::color::Rgb;
use misty::picker::RowKey;
use misty::text::{Span, StyledText, TextColor};
use misty::{EventCx, Frame, Input, QuickPickPlacement, Rect, Svg, Widget};
use winter_render::{PanelView, PickerPlacement};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// App: the picker drawn by misty
// ========================================================================

impl App {
    /// Draw the open picker over `frame`, placed and colored as the view
    /// the frame worked out says.
    pub(in crate::window) fn draw_picker(&mut self, frame: &mut Frame) {
        let Some(view) = self.pickers.drawn.take() else {
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        let cell_width = frame.cell_metrics().width;
        let Some(quick) = self.chrome.quick_pick_mut() else {
            return;
        };
        quick.set_placement(placement(view.placement));
        quick.set_anchor(
            view.anchor
                .map(|(x, y, height)| Rect::new(x, y, cell_width, height)),
        );
        let rows: Vec<RowKey> = quick
            .picker()
            .view()
            .rows
            .iter()
            .map(|shown| shown.row.key)
            .collect();
        let colors: HashMap<RowKey, Vec<(usize, Rgb)>> = rows
            .iter()
            .zip(&view.items)
            .filter(|(_, item)| !item.description_colors.is_empty())
            .map(|(key, item)| (*key, runs(&item.description_colors)))
            .collect();
        quick.set_description_colors(colors);
        quick.set_panel(view.panel.as_ref().map(panel_lines).unwrap_or_default());
        let names = icon_names(quick);
        add_icons(quick.icons_mut(), &names);
        let bounds = frame.bounds();
        quick.draw(frame, bounds, &theme);
    }

    /// Hand pointer `input` to the open picker, which takes every pointer
    /// input while it is open; whether one was open.
    pub(in crate::window) fn point_at_picker(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        let focused = self.layout().focused();
        let Some(quick) = self.chrome.quick_pick_mut() else {
            return false;
        };
        let reply = quick.handle(input, cx);
        self.answer_reply(reply, focused);
        true
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's placement for Winter's.
fn placement(placement: PickerPlacement) -> QuickPickPlacement {
    let rect = |(x, y, width, height): (f32, f32, f32, f32)| Rect::new(x, y, width, height);
    match placement {
        PickerPlacement::EditorBottom(pane) => QuickPickPlacement::DockedBottom(rect(pane)),
        PickerPlacement::Pane(pane) => QuickPickPlacement::CenteredIn(rect(pane)),
        PickerPlacement::Transient(card) => QuickPickPlacement::Filling(rect(card)),
        PickerPlacement::WindowCenter => QuickPickPlacement::Window,
    }
}

/// Misty's color runs for Winter's.
fn runs(colors: &[(usize, winter_render::ThemeRgb)]) -> Vec<(usize, Rgb)> {
    colors
        .iter()
        .map(|&(count, color)| (count, Rgb::new(color.r, color.g, color.b)))
        .collect()
}

/// The side panel's lines as misty draws them: each run in its color, and
/// where the query matched marked bold.
fn panel_lines(panel: &PanelView) -> Vec<StyledText> {
    panel
        .lines
        .iter()
        .map(|line| {
            let mut styled = StyledText::new();
            let mut at = 0;
            for span in &line.spans {
                let color = TextColor::Rgba(misty::color::Rgba::new(
                    span.color.r,
                    span.color.g,
                    span.color.b,
                    u8::MAX,
                ));
                // Runs of the span marked alike, each one span.
                let mut run = String::new();
                let mut run_marked = false;
                for c in span.text.chars() {
                    let marked = line.marks.iter().any(|mark| mark.contains(&at));
                    if marked != run_marked && !run.is_empty() {
                        styled.push(marked_span(std::mem::take(&mut run), color, run_marked));
                    }
                    run_marked = marked;
                    run.push(c);
                    at += 1;
                }
                if !run.is_empty() {
                    styled.push(marked_span(run, color, run_marked));
                }
            }
            styled
        })
        .collect()
}

/// A run of a panel line in `color`, bold where the query matched.
fn marked_span(text: String, color: TextColor, marked: bool) -> Span {
    match marked {
        true => Span::new(text, color).bold(),
        false => Span::new(text, color),
    }
}

/// Every icon name the picker's rows and their buttons draw.
fn icon_names(quick: &misty::QuickPick<super::WindowOutcome, super::Preview>) -> Vec<String> {
    quick
        .picker()
        .view()
        .rows
        .iter()
        .flat_map(|shown| {
            let buttons = shown
                .buttons
                .iter()
                .filter_map(|action| action.button.clone());
            shown.row.icon.clone().into_iter().chain(buttons)
        })
        .collect()
}

/// Add each of `names` that `icons` lacks, from Winter's bundled icons.
fn add_icons(icons: &mut misty::IconSet, names: &[String]) {
    for name in names {
        if icons.get(name).is_some() {
            continue;
        }
        let Some(svg) = crate::icons::named_svg(name).and_then(|bytes| Svg::parse(&bytes).ok())
        else {
            continue;
        };
        icons.insert(name.clone(), svg);
    }
}
