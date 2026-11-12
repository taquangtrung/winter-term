//! The menus, drawn by misty: the menubar's open menu dropped from its title
//! (or the hamburger button), with its submenus, and the right-click context
//! menu at the pointer. While one is open it takes every pointer input and
//! the keys it moves by (the arrows, Enter, Escape); a choice runs its
//! command, and a press outside puts the menu away.

use misty::TitleBarHit;
use misty::{EventCx, Frame, Input, MenuResponse, Widget};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// App: the menus misty draws
// ========================================================================

impl App {
    /// Draw the open menus over `frame`.
    pub(in crate::window) fn draw_menus(&mut self, frame: &mut Frame) {
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&crate::render::Theme::dark()),
        };
        let area = frame.bounds();
        if let Some(menu) = &mut self.chrome.menus.dropdown {
            menu.draw(frame, area, &theme);
        }
        if let Some(menu) = &mut self.chrome.menus.context {
            menu.draw(frame, area, &theme);
        }
    }

    /// Hand `input` to the open menu; whether it took it. A press outside
    /// the context menu goes on to what it landed on (a right press opens
    /// the menu there again); one outside the menubar's menu goes on only
    /// to another of the bar's elements, so a press on another title opens
    /// that menu.
    pub(in crate::window) fn offer_to_menus(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        if let Some(menu) = &mut self.chrome.menus.context {
            return match menu.handle(input, cx) {
                MenuResponse::Chosen(index) => {
                    let action = self.chrome.menus.context_actions.get(index).cloned();
                    self.close_context_menu();
                    if let Some(action) = action {
                        self.run_context_action(action);
                    }
                    true
                }
                MenuResponse::Dismissed => {
                    self.close_context_menu();
                    !matches!(input, Input::PointerButton(_))
                }
                MenuResponse::Handled => true,
                MenuResponse::Ignored => false,
            };
        }
        let Some(menu) = &mut self.chrome.menus.dropdown else {
            return false;
        };
        match menu.handle(input, cx) {
            MenuResponse::Chosen(command) => {
                self.close_menu();
                let focused = self.layout().focused();
                self.run_command(command, focused);
                true
            }
            MenuResponse::Dismissed => {
                let passes = match input {
                    Input::PointerButton(button) => {
                        let (x, y) = (button.position.x, button.position.y);
                        let hit = self.title_bar_hit(x, y);
                        let own = hit == Some(TitleBarHit::MenuButton)
                            || self.chrome.menus.open.map(TitleBarHit::MenuTitle) == hit;
                        y < self.top_chrome_height() && !own
                    }
                    _ => false,
                };
                self.close_menu();
                !passes
            }
            MenuResponse::Handled => true,
            MenuResponse::Ignored => false,
        }
    }
}
