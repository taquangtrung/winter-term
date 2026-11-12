//! The title bar, drawn by misty: the app's name, the hamburger button or
//! the classic menubar's titles, and, in the borderless style, the window
//! buttons on the side the settings put them, the resize border, and the
//! empty bar that moves the window. Also the tooltip misty hangs under
//! whatever the pointer rests on, in the bar or anywhere else.

use misty::{
    CellMetrics, EventCx, Frame, Input, Point, Rect, TitleBar, TitleBarHit, TitleBarResponse,
    Widget, WindowButtons,
};
use winter_render::{ControlsSide, MenuStyle};

use crate::config::TitleBarStyle;
use crate::window::menus::menu_defs;
use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What the title bar reads: the app's name, whatever the focused pane
/// shows, which its breadcrumb row names.
const TITLE_BAR_LABEL: &str = "Winter Term";

// ========================================================================
// Data Structures
// ========================================================================

/// What the settings make of the title bar: rebuilt when they change.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) struct TitleBarShape {
    /// Where the window buttons go, or that the system draws its own.
    buttons: WindowButtons,
    /// The hamburger button or the classic menubar.
    menu_style: MenuStyle,
}

// ========================================================================
// App: the title bar misty draws
// ========================================================================

impl App {
    /// Draw the title bar along the top of `frame`, built anew when the
    /// settings have changed its shape, and the tooltip over everything.
    pub(in crate::window) fn draw_title_bar(&mut self, frame: &mut Frame) {
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        let shape = self.title_bar_shape();
        if self.chrome.title_bar.as_ref().map(|(had, _)| *had) != Some(shape) {
            self.chrome.title_bar = Some((shape, title_bar_of(shape)));
        }
        let open = self.chrome.menus.open;
        if let Some((_, bar)) = &mut self.chrome.title_bar {
            bar.set_open_menu(open);
            bar.draw(frame, frame.bounds(), &theme);
        }
    }

    /// Draw the tooltip for what the pointer rests on over `frame`, or put
    /// it away when it rests on nothing that has one, a menu is open, or a
    /// tab is dragged.
    pub(in crate::window) fn draw_tooltip(&mut self, frame: &mut Frame) {
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        // An open menu covers what the pointer rested on, and a tab dragged
        // off its strip has left it.
        let covered = self.chrome.menus.dropdown.is_some()
            || self.chrome.menus.context.is_some()
            || self.chrome.tab_drag.is_some();
        match self.hover_tooltip().filter(|_| !covered) {
            Some((text, x, y)) => {
                let cell_h = frame.cell_metrics().height;
                let anchor = self
                    .tooltip_anchor()
                    .map_or(Rect::new(x, y, 1.0, cell_h), |at| {
                        Rect::new(at.x, at.y, at.w, at.h)
                    });
                self.chrome.tooltip.hover(anchor, text);
            }
            None => {
                self.chrome.tooltip.leave();
            }
        }
        self.chrome.tooltip.draw(frame, frame.bounds(), &theme);
    }

    /// Offer `input` to the title bar; whether it took it. A press at the
    /// window's edge stays Winter's (which keeps the scrollbar beside it),
    /// and so does every press while a dialog is up, which
    /// [`Self::manage_window_at`] answers. The pointer moving is the bar's
    /// to light its buttons by, and still Winter's too.
    pub(in crate::window) fn offer_to_title_bar(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        if let Input::PointerButton(button) = input {
            let (x, y) = (button.position.x, button.position.y);
            if button.pressed
                && (self.chrome.dialog_open() || self.edge_resize_direction(x, y).is_some())
            {
                return false;
            }
        }
        let Some((_, bar)) = &mut self.chrome.title_bar else {
            return false;
        };
        let answer = bar.handle(input, cx);
        let moved = matches!(input, Input::PointerMoved(_) | Input::PointerLeft);
        match answer {
            // Drained after the input into the shared quit path.
            TitleBarResponse::Close => self.runtime.exit.requested = true,
            TitleBarResponse::OpenMenu => self.toggle_menu(0),
            TitleBarResponse::OpenMenuTitle(index) => {
                if self.chrome.menus.open != Some(index) {
                    self.toggle_menu(index);
                }
                return true;
            }
            TitleBarResponse::Handled | TitleBarResponse::Tabs(_) => {}
            TitleBarResponse::Ignored => return false,
        }
        self.dirty = true;
        !moved
    }

    /// How tall the title bar is, in the font of the last frame (Winter's
    /// own cells before the first).
    pub(in crate::window) fn title_bar_height(&self) -> f32 {
        let metrics = self
            .window
            .as_ref()
            .and_then(|window| window.cell_metrics());
        let metrics = metrics.or_else(|| {
            let (width, height) = self.renderer.as_ref()?.cell_size();
            Some(CellMetrics {
                baseline: height,
                height,
                width,
            })
        });
        metrics.map_or(0.0, TitleBar::height)
    }

    /// What the title bar has at `(x, y)` as it was last drawn, or `None`
    /// off it.
    pub(in crate::window) fn title_bar_hit(&self, x: f32, y: f32) -> Option<TitleBarHit> {
        let (_, bar) = self.chrome.title_bar.as_ref()?;
        bar.hit_test(Point::new(x, y))
    }

    /// Where the open menu drops from: its title, or the hamburger button.
    pub(in crate::window) fn menu_anchor(&self, index: usize) -> Option<Rect> {
        let (_, bar) = self.chrome.title_bar.as_ref()?;
        match self.config.menu_style {
            MenuStyle::Classic => bar.menu_title(index),
            MenuStyle::Modern => bar.menu_button(),
        }
    }

    /// The title bar the settings ask for.
    fn title_bar_shape(&self) -> TitleBarShape {
        let buttons = match (
            self.config.title_bar_style,
            self.config.window_controls_side,
        ) {
            (TitleBarStyle::System, _) => WindowButtons::None,
            (TitleBarStyle::Modern, ControlsSide::Left) => WindowButtons::Left,
            (TitleBarStyle::Modern, ControlsSide::Right) => WindowButtons::Right,
        };
        TitleBarShape {
            buttons,
            menu_style: self.config.menu_style,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's title bar of `shape`.
fn title_bar_of(shape: TitleBarShape) -> TitleBar {
    let bar = TitleBar::new(TITLE_BAR_LABEL).with_window_buttons(shape.buttons);
    match shape.menu_style {
        MenuStyle::Classic => {
            let titles = menu_defs(MenuStyle::Classic)
                .iter()
                .map(|menu| menu.title.to_string())
                .collect();
            bar.with_menus(titles)
        }
        MenuStyle::Modern => bar.with_menu_button(),
    }
}
