//! The open picker as misty's quick pick draws it over the frame: where it
//! goes, its rows' icons and colors, and its side panel, all read from the
//! picker view the frame worked out; and the pointer handed to it.

use std::collections::HashMap;

use crate::render::PickerPlacement;
use misty::Rgb;
use misty::picker::{PickerAct, RowKey};
use misty::widgets::QuickPickPlacement;
use misty::{EventCx, Frame, Input, Rect, Widget};

use super::keys::KEYS;
use crate::window::App;
use crate::window::appearance::ui_theme;

// ========================================================================
// Constants
// ========================================================================

/// The acts the hints line names.
const HINTED: [PickerAct; 3] = [
    PickerAct::ActionMenu,
    PickerAct::Save,
    PickerAct::TogglePanel,
];

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
        // A copy of the kept theme, so the picker can be borrowed from the
        // window while it is drawn in it.
        let theme = ui_theme(self.renderer.as_ref()).clone();
        let cell_width = frame.cell_metrics().width;
        let Some(quick) = self.parts.quick_pick_mut() else {
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
        // The hints line names the keys of Winter's own table; its words are
        // the short ones Winter's misty theme sets.
        for act in HINTED {
            let Some(binding) = KEYS.bindings.iter().find(|binding| binding.action == act) else {
                continue;
            };
            if let Some(key) = binding.keys.first() {
                quick.set_hint_key(act, *key);
            }
        }
        quick.set_panel(
            view.panel
                .as_ref()
                .map(|panel| panel.lines.clone())
                .unwrap_or_default(),
        );
        let names = icon_names(quick);
        crate::icons::add_to(quick.icons_mut(), &names);
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
        let Some(quick) = self.parts.quick_pick_mut() else {
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
fn runs(colors: &[(usize, crate::render::ThemeRgb)]) -> Vec<(usize, Rgb)> {
    colors
        .iter()
        .map(|&(count, color)| (count, Rgb::new(color.r, color.g, color.b)))
        .collect()
}

/// Every icon name the picker's rows and their buttons draw.
fn icon_names(
    quick: &misty::widgets::QuickPick<super::WindowOutcome, super::Preview>,
) -> Vec<String> {
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
