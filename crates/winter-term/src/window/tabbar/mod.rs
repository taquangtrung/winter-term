//! Top-tabbar glue: build the [`TopTabbar`] the renderer draws from the app's
//! tab/menu state, and dispatch clicks the renderer's [`hit_test`](winter_render::hit_test) resolves.
//! The menus themselves live in [`crate::window::menus`], the right-click menu in
//! [`crate::window::menus::context`]; the window title in [`title`].

mod title;

use winter_render::{ContextMenu, Menu, MenuItem, StatusHover, TabbarHit, TopTabbar};

use crate::config::TitleBarStyle;
use crate::model::layout::TabId;
use crate::terminal::session::TitleFacts;

use crate::window::menus::context::ContextAction;
use crate::window::menus::{keys_label, menu_defs, menu_item, SEPARATOR};
use crate::window::status::mode_label_and_name;
use crate::window::strip::StripHit;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What a terminal is called where nothing better names it: no program, no
/// directory, no title of its own.
const TERMINAL_NAME: &str = "Terminal";

// ========================================================================
// App: tabbar model and interaction
// ========================================================================

impl App {
    /// How a strip names one tab: a name the user gave it; for a terminal
    /// with nothing running in it, its directory's name, with as many of the
    /// directories above it as tell it from another such tab's; else as the
    /// window's title names it ([`Self::title_for_window`]).
    pub(in crate::window) fn tab_title(&self, tab: TabId) -> String {
        if self.tabs.name(tab).is_none() {
            if let Some(cwd) = self.idle_directory(tab) {
                let others: Vec<String> = self
                    .open_tabs
                    .terminals()
                    .filter(|(other, _)| **other != tab && self.tabs.name(**other).is_none())
                    .filter_map(|(other, _)| self.idle_directory(*other))
                    .collect();
                return distinct_name(&cwd, &others);
            }
        }
        self.title_for_window(tab)
    }

    /// The directory a terminal is in, while its shell waits at its prompt
    /// with no program of its own running. `None` for any other tab, a mux
    /// terminal, and one whose directory is not known.
    fn idle_directory(&self, tab: TabId) -> Option<String> {
        let terminal = self.open_tabs.terminal(&tab)?;
        if terminal.mux_session().is_some() || !terminal.is_at_prompt() {
            return None;
        }
        let facts = terminal.title_facts();
        match facts.program {
            Some(_) => None,
            None => facts.cwd,
        }
    }

    /// How the window's title names a tab: a name the user gave it, an
    /// editor's own label, else what the terminal says about itself, its
    /// directory written whole.
    pub(in crate::window) fn title_for_window(&self, focused: TabId) -> String {
        if let Some(name) = self.tabs.name(focused) {
            return name.to_string();
        }
        if let Some(slot) = self.open_tabs.editor(&focused) {
            return slot.tab_label();
        }
        let terminal = self.open_tabs.terminal(&focused);
        let facts = terminal.map(|terminal| terminal.title_facts());
        let remote = facts.as_ref().and_then(|facts| facts.host.remote_name());
        // An OSC 0/2 title is the app's own presentation of itself (butterfly,
        // for one, puts the open file's name there), so it wins over the
        // process-name/cwd heuristic below.
        if let Some(title) = self.tabs.program_title(focused).filter(|t| !t.is_empty()) {
            return clean_title(title, remote);
        }
        let (Some(terminal), Some(facts)) = (terminal, facts.as_ref()) else {
            return TERMINAL_NAME.to_string();
        };
        if let Some(session) = terminal.mux_session() {
            return format!("mux: {session}");
        }
        let place = place_of(facts, clean_cwd);
        match (facts.program.as_deref(), place) {
            (Some(program), place) => labelled(program, place),
            (None, Some(place)) => place,
            (None, None) => TERMINAL_NAME.to_string(),
        }
    }
}

/// Where a terminal's program works, for a title: the host or container it
/// reaches over ssh or `docker exec`, not the directory here it was started
/// from; else its directory, as `shorten` writes it.
fn place_of(facts: &TitleFacts, shorten: impl Fn(&str) -> String) -> Option<String> {
    match facts.host.remote_name() {
        Some(remote) => Some(remote.to_string()),
        None => facts.cwd.as_deref().map(shorten),
    }
}

/// `name: place`, or `name` alone where there is no place to name.
fn labelled(name: &str, place: Option<String>) -> String {
    match place {
        Some(place) => format!("{name}: {place}"),
        None => name.to_string(),
    }
}

/// The shortest tail of directory `cwd` that no other of `others` ends in:
/// its own name, then with its parent's before it (`a/src` beside `b/src`),
/// and so on. The root, which has no name, is `/`.
fn distinct_name(cwd: &str, others: &[String]) -> String {
    let parts = path_parts(cwd);
    if parts.is_empty() {
        return "/".to_string();
    }
    let others: Vec<Vec<&str>> = others.iter().map(|other| path_parts(other)).filter(|other| *other != parts).collect();
    let count = (1..=parts.len())
        .find(|count| others.iter().all(|other| !other.ends_with(&parts[parts.len() - count..])))
        .unwrap_or(parts.len());
    parts[parts.len() - count..].join("/")
}

/// The names of the directories along `path`, from the root down.
fn path_parts(path: &str) -> Vec<&str> {
    path.split('/').filter(|part| !part.is_empty()).collect()
}

/// `path` with a leading home directory written `~`, otherwise unchanged.
pub(in crate::window) fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) => home_relative_to(path, &home),
        Err(_) => path.to_string(),
    }
}

/// [`home_relative`] against an explicit `home`. A sibling that merely shares
/// the home directory's name as a prefix (`/home/ann2`) is not inside it.
fn home_relative_to(path: &str, home: &str) -> String {
    if home.is_empty() {
        return path.to_string();
    }
    match path.strip_prefix(home) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

pub(in crate::window) fn clean_cwd(cwd: &str) -> String {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return cwd.to_string(),
    };
    if cwd == home {
        return "~".to_string();
    }
    let suffix = match cwd.strip_prefix(&format!("{}/", home)) {
        Some(s) => s,
        None => return cwd.to_string(),
    };
    // Abbreviate every intermediate directory component to its first character,
    // keeping only the leaf component in full so a long project path stays
    // compact in the tab title while still being recognizable:
    //   ~/Workspace/apps/winter-term -> ~/W/a/winter-term
    let parts: Vec<&str> = suffix.split('/').collect();
    if parts.len() <= 1 {
        return format!("~/{}", suffix);
    }
    let (head, tail) = parts.split_at(parts.len() - 1);
    let abbrev: Vec<String> = head
        .iter()
        .map(|c| {
            c.chars()
                .next()
                .map(|ch| ch.to_string())
                .unwrap_or_default()
        })
        .collect();
    format!("~/{}/{}", abbrev.join("/"), tail[0])
}

/// `title` without the `user@host:` a shell puts before its directory in
/// the title it sets (`ann@box: ~/notes`), or with `remote: ` in its place
/// where the terminal's commands run on another host or container, so a
/// remote shell's title still says which. Only such a lead is taken off: a
/// title that merely holds an `@` and a `:` further on is the program's own.
pub(in crate::window) fn clean_title(title: &str, remote: Option<&str>) -> String {
    let Some((lead, rest)) = title.split_once(':') else {
        return title.to_string();
    };
    let is_user_at_host = lead
        .split_once('@')
        .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty() && !lead.contains(char::is_whitespace));
    match (is_user_at_host, remote) {
        (true, Some(remote)) => format!("{remote}: {}", rest.trim()),
        (true, None) => rest.trim().to_string(),
        (false, _) => title.to_string(),
    }
}

impl App {
    /// Build the tabbar model for this frame from the current tab/menu state.
    pub(in crate::window) fn build_top_tabbar(&self) -> TopTabbar {
        let menus = menu_defs(self.config.menu_style)
            .iter()
            .map(|menu| Menu {
                items: menu.items.iter().map(menu_item).collect(),
                title: menu.title.to_string(),
            })
            .collect();
        let context_menu = self.chrome.menus.context_pos.map(|(x, y)| ContextMenu {
            items: self
                .chrome.menus
                .context_actions
                .iter()
                .map(|a| MenuItem {
                    children: vec![],
                    label: match a {
                        ContextAction::Copy => "Copy".into(),
                        ContextAction::Paste => "Paste".into(),
                        ContextAction::OpenLink(_) => "Open Link".into(),
                        ContextAction::Separator => SEPARATOR.label.into(),
                        ContextAction::EditorKey(action) => action.label.clone(),
                    },
                    // An editor's entry names the keys it types, so the menu
                    // teaches them.
                    shortcut: match a {
                        ContextAction::EditorKey(action) => keys_label(&action.keys),
                        _ => String::new(),
                    },
                })
                .collect(),
            selected: self.chrome.menus.context_selected,
            x,
            y,
        });
        let url_tooltip = self.hover_tooltip();
        // A strip tab's title hangs under the tab, not the pointer.
        let tooltip_anchor = match (&self.pointer.hovered_url, self.tabs.strip_hover) {
            (None, StripHit::Tab(_)) => self.tabs.hover_anchor,
            _ => None,
        };
        TopTabbar {
            active_tab: 0,
            tabbar_hover: self.tabs.hover,
            context_menu,
            controls_side: self.config.window_controls_side,
            menu_style: self.config.menu_style,
            menus,
            open_menu: self.chrome.menus.open,
            open_submenu: self.chrome.menus.open_submenu,
            selected_item: self.chrome.menus.selected_item,
            selected_subitem: self.chrome.menus.selected_subitem,
            // Tabs live on each pane's own strip; the title bar keeps the
            // menus, the window controls, and a button opening a tab in the
            // focused pane.
            path: self.titlebar_path(),
            tabs: Vec::new(),
            tooltip_anchor,
            url_tooltip,
            window_controls: self.config.title_bar_style == TitleBarStyle::Modern,
        }
    }

    /// The label the title bar shows: the app running in the focused tab
    /// and its target (the directory, open file, or reference), with home
    /// abbreviated to `~`.
    fn titlebar_path(&self) -> String {
        let focused = self.layout().focused();
        if let Some(slot) = self.open_tabs.editor(&focused) {
            let kind = slot.kind().title();
            let target = slot.editor.address();
            return match target {
                Some(tgt) if !tgt.is_empty() => format!("{kind}: {tgt}"),
                _ => kind.to_string(),
            };
        }
        if let Some(terminal) = self.open_tabs.terminal(&focused) {
            let facts = terminal.title_facts();
            let name = match terminal.mux_session() {
                Some(session) => format!("mux: {session}"),
                None => facts.program.clone().unwrap_or_else(|| TERMINAL_NAME.to_string()),
            };
            return labelled(&name, place_of(&facts, home_relative));
        }
        TERMINAL_NAME.to_string()
    }

    /// Floating tooltip describing whatever interactive button or element the
    /// pointer is currently resting on.
    pub(in crate::window) fn hover_tooltip(&self) -> Option<(String, f32, f32)> {
        let (cx, cy) = self.pointer.cursor_pos;

        // 1. Terminal URL / Link
        if let Some(url) = &self.pointer.hovered_url {
            return Some((url.clone(), cx, cy));
        }

        // 2. Pane tab strip buttons
        match self.tabs.strip_hover {
            StripHit::Close(_) => return Some(("Close Tab".to_string(), cx, cy)),
            StripHit::NewTab(_) => return Some(("New Tab".to_string(), cx, cy)),
            StripHit::Tab(pane) => return Some((self.tab_title(pane), cx, cy)),
            StripHit::None => {}
        }

        // 3. Titlebar and window control buttons
        match self.tabs.hover {
            TabbarHit::Hamburger => return Some(("Menu".to_string(), cx, cy)),
            TabbarHit::Minimize => return Some(("Minimize".to_string(), cx, cy)),
            TabbarHit::Maximize => {
                let is_max = self.window.as_ref().is_some_and(|w| w.is_maximized());
                let label = if is_max { "Restore" } else { "Maximize" };
                return Some((label.to_string(), cx, cy));
            }
            TabbarHit::Close => return Some(("Close".to_string(), cx, cy)),
            TabbarHit::CloseTab(_) => return Some(("Close Tab".to_string(), cx, cy)),
            TabbarHit::ScrollTabsLeft => return Some(("Scroll Tabs Left".to_string(), cx, cy)),
            TabbarHit::ScrollTabsRight => return Some(("Scroll Tabs Right".to_string(), cx, cy)),
            _ => {}
        }

        // 4. Status bar buttons and indicators
        if let Some(status_hover) = self.chrome.status_hover {
            match status_hover {
                StatusHover::Mode => {
                    if let Some(mode) = self.mode_tooltip() {
                        return Some((mode, cx, cy));
                    }
                }
                StatusHover::Bell => {
                    let text = match self.chrome.notifier.log.unread() {
                        0 => "Notifications".to_string(),
                        1 => "1 unread notification".to_string(),
                        n => format!("{n} unread notifications"),
                    };
                    return Some((text, cx, cy));
                }
                StatusHover::Segment(idx) => {
                    if let Some(text) = self.status_segment_tooltip(idx) {
                        return Some((text, cx, cy));
                    }
                }
            }
        }

        // 5. Toast notification close button
        if let Some(toast) = self.toast_at(cx, cy) {
            if toast.close {
                return Some(("Dismiss".to_string(), cx, cy));
            }
        }

        // 6. Editor toolbar buttons
        if let Some((owner, button_id)) = self.pointer.hovered_toolbar_button {
            if let Some(tip) = self.toolbar_button_tooltip(owner, button_id) {
                return Some((tip, cx, cy));
            }
        }

        None
    }

    /// Tooltip text for toolbar button `button` of pane `owner`.
    fn toolbar_button_tooltip(&self, owner: u64, button: u16) -> Option<String> {
        let editor = self.open_tabs.editor(&TabId(owner))?;
        editor.editor.toolbar_tooltip(button)
    }

    /// Tooltip text for the active mode when hovered in the status bar.
    fn mode_tooltip(&self) -> Option<String> {
        let focused = self.layout().focused();
        let editor = self.open_tabs.editor(&focused).map(|slot| slot.editor.status());
        let editor_mode = editor.as_ref().and_then(|status| status.mode);
        let mode = self.open_tabs.mode(&focused);
        let (_, name) = mode_label_and_name(mode, editor_mode)?;
        Some(name.to_string())
    }

    /// Pixel height of the reserved top-tabbar band.
    pub(in crate::window) fn top_chrome_height(&self) -> f32 {
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(0.0);
        ch * self.top_chrome_rows() as f32
    }

    /// Handle a left-click at `(x, y)` against the tabbar. Returns `true` when the
    /// click was consumed (a tabbar element, or dismissing an open menu) and the
    /// caller should not treat it as a pane click.
    pub(in crate::window) fn handle_tabbar_click(&mut self, x: f32, y: f32) -> bool {
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return false;
        };
        let surface_w = self.viewport_rect().width;
        let tabbar = self.build_top_tabbar();
        let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
        let menu_was_open = self.chrome.menus.open.is_some();
        let focused = self.layout().focused();

        match hit {
            TabbarHit::Tab(_)
            | TabbarHit::CloseTab(_)
            | TabbarHit::ScrollTabsLeft
            | TabbarHit::ScrollTabsRight => {
                // The title bar draws no tabs, so nothing lands here.
            }
            TabbarHit::Hamburger => {
                if self.chrome.menus.open.is_some() {
                    self.close_menu();
                } else {
                    self.toggle_menu(0);
                }
            }
            TabbarHit::Minimize => {
                self.close_menu();
                if let Some(window) = &self.window {
                    window.set_minimized(true);
                }
            }
            TabbarHit::Maximize => {
                self.close_menu();
                if let Some(window) = &self.window {
                    window.set_maximized(!window.is_maximized());
                }
            }
            TabbarHit::Close => {
                // Drained by the mouse handler into the shared quit path.
                self.runtime.exit.requested = true;
            }
            TabbarHit::MenuTitle(i) => self.toggle_menu(i),
            TabbarHit::DropdownItem(i) => {
                if let Some(item) = self.open_menu_item(i) {
                    if item.label == "-" {
                        return true;
                    }
                    if item.children.is_empty() {
                        let command = item.command;
                        self.close_menu();
                        self.run_command(command, focused);
                    } else {
                        // A submenu parent: keep it open and expand its children.
                        self.chrome.menus.open_submenu = Some(i);
                        self.chrome.menus.selected_item = Some(i);
                        self.chrome.menus.selected_subitem = None;
                    }
                }
            }
            TabbarHit::SubmenuItem(child) => {
                if let Some(item) = self.open_submenu_item(child) {
                    if item.label != "-" {
                        let command = item.command;
                        self.close_menu();
                        self.run_command(command, focused);
                    }
                }
            }
            TabbarHit::ContextMenuItem(i) => {
                if let Some(action) = self.chrome.context_entry(i) {
                    self.close_context_menu();
                    self.run_context_action(action);
                }
            }
            TabbarHit::None => {
                if self.chrome.menus.context_pos.is_some() {
                    self.close_context_menu();
                    return true;
                }
                if menu_was_open {
                    self.close_menu();
                } else if y < self.top_chrome_height() {
                    // Empty title-bar space. In the borderless modern style this is
                    // the drag handle to move the window; either way, swallow the
                    // click so it does not select pane text.
                    if self.config.title_bar_style == TitleBarStyle::Modern {
                        if let Some(window) = &self.window {
                            let _ = window.drag_window();
                        }
                    }
                    return true;
                } else {
                    return false;
                }
            }
        }
        self.dirty = true;
        true
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_render::MenuStyle;

    use super::*;
    use crate::editors::keys::KeysEditor;
    use crate::model::editor::{Editor, EditorOutcome};
    use crate::model::input::{Key, Keymap};
    use crate::model::paint::Frame;
    use crate::window::editor::EditorKind;
    use crate::window::menus::MODERN_ITEMS;

    #[test]
    fn test_build_top_tabbar_carries_submenu_children_and_state() {
        let mut app = App::new();
        app.chrome.menus.open = Some(0);
        let layout_idx = MODERN_ITEMS
            .iter()
            .position(|it| it.label == "Layout")
            .unwrap();
        app.chrome.menus.open_submenu = Some(layout_idx);
        app.chrome.menus.selected_subitem = Some(1);
        let tabbar = app.build_top_tabbar();
        assert_eq!(tabbar.open_submenu, Some(layout_idx));
        assert_eq!(tabbar.selected_subitem, Some(1));
        assert!(tabbar.menus[0].items[layout_idx].has_children());
    }

    #[test]
    fn test_osc_title_is_used_before_process_and_cwd_fallbacks() {
        let mut app = App::new();
        let focused = app.layout().focused();
        // An app-set title (e.g. butterfly naming the open file) shows as-is,
        // even though a foreground process may also be running in the pane.
        app.tabs
            .set_program_title(focused, "butterfly — Winter Term.md".into());
        assert_eq!(app.tab_title(focused), "butterfly — Winter Term.md");

        // A shell-style "user@host: cwd" title is cleaned to its cwd part.
        app.tabs
            .set_program_title(focused, "user@host:~/notes".into());
        assert_eq!(app.tab_title(focused), "~/notes");

        // An empty title falls through to the remaining fallbacks.
        app.tabs.set_program_title(focused, String::new());
        assert_eq!(app.tab_title(focused), "Terminal");
    }

    #[test]
    fn test_an_idle_terminal_s_tab_is_named_by_its_directory_and_what_tells_it_apart() {
        let others = |paths: &[&str]| paths.iter().map(|path| path.to_string()).collect::<Vec<_>>();
        assert_eq!(distinct_name("/home/ann/dev/winter-term", &[]), "winter-term");
        assert_eq!(distinct_name("/home/ann/dev/winter-term/", &[]), "winter-term", "a trailing slash");
        assert_eq!(distinct_name("/", &[]), "/", "the root has no name but itself");
        assert_eq!(distinct_name("/x/a/src", &others(&["/x/b/src", "/y/docs"])), "a/src");
        assert_eq!(distinct_name("/p/a/src", &others(&["/q/a/src"])), "p/a/src", "as far up as it takes");
        assert_eq!(distinct_name("/x/a/src", &others(&["/x/a/src"])), "src", "the same directory twice is no clash");
    }

    #[test]
    fn test_only_a_user_at_host_lead_is_taken_off_a_title() {
        assert_eq!(clean_title("ann@box:~/notes", None), "~/notes");
        assert_eq!(clean_title("ann@box: ~/notes", None), "~/notes");
        assert_eq!(clean_title("mail me@x.com: draft", None), "mail me@x.com: draft", "an @ in the program's own title");
        assert_eq!(clean_title("vim: notes.md", None), "vim: notes.md");
        assert_eq!(clean_title("@: odd", None), "@: odd");
        // A remote shell's title keeps which host it is on.
        assert_eq!(clean_title("ann@box-17:~/x", Some("box")), "box: ~/x");
        assert_eq!(clean_title("vim: notes.md", Some("box")), "vim: notes.md");
    }

    #[test]
    fn test_home_relative_writes_only_paths_inside_home_as_tilde() {
        assert_eq!(home_relative_to("/home/ann", "/home/ann"), "~");
        assert_eq!(home_relative_to("/home/ann/dev/x", "/home/ann"), "~/dev/x");
        assert_eq!(
            home_relative_to("/home/ann2/dev", "/home/ann"),
            "/home/ann2/dev"
        );
        assert_eq!(home_relative_to("/etc", "/home/ann"), "/etc");
        assert_eq!(home_relative_to("/etc", ""), "/etc");
    }

    #[test]
    fn test_an_editor_tab_is_named_after_its_kind() {
        let mut app = App::new();
        let keys = app.show_editor(
            EditorKind::Keys,
            Box::new(KeysEditor::new(&Keymap::default(), Vec::new())),
        );
        assert_eq!(
            app.tab_title(keys),
            "Keys",
            "a title restating the kind is not doubled up"
        );

        // The icon before the title says which kind the editor is, so the
        // title is not prefixed with it.
        struct Titled(&'static str);
        impl Editor for Titled {
            fn title(&self) -> String {
                self.0.to_string()
            }
            fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
                Frame::default()
            }
            fn on_key(&mut self, _key: &Key) -> EditorOutcome {
                EditorOutcome::Consumed
            }
        }
        let dir = app.show_editor(EditorKind::Dir, Box::new(Titled("winter-term")));
        assert_eq!(app.tab_title(dir), "winter-term");
        let proc = app.show_editor(EditorKind::Proc, Box::new(Titled("firefox")));
        assert_eq!(app.tab_title(proc), "firefox");
    }

    #[test]
    fn test_close_other_tabs_leaves_only_the_kept_tab_in_its_pane() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.new_tab();
        app.new_tab();
        assert_eq!(app.layout().tabs_in_pane(first).len(), 3);

        app.close_other_tabs(first);

        assert_eq!(app.layout().tabs_in_pane(first), vec![first]);
    }

    #[test]
    fn test_a_renamed_tab_keeps_its_name_over_an_editor_label() {
        // An explicit name is the user's own act; the editor's label must not
        // overwrite it.
        let mut app = App::new();
        let keys = app.show_editor(
            EditorKind::Keys,
            Box::new(KeysEditor::new(&Keymap::default(), Vec::new())),
        );
        app.tabs.set_name(keys, Some("work".to_string()));
        assert_eq!(app.tab_title(keys), "work");
    }

    #[test]
    fn test_build_top_tabbar_carries_menus_and_no_tabs() {
        let mut app = App::new();
        app.chrome.menus.open = Some(0);
        let tabbar = app.build_top_tabbar();
        assert!(tabbar.tabs.is_empty(), "tabs live on each pane's strip");
        assert_eq!(tabbar.open_menu, Some(0));
        assert_eq!(tabbar.menu_style, MenuStyle::Modern);
    }

    #[test]
    fn test_clean_title_and_cwd() {
        assert_eq!(clean_title("user@host: /home/user", None), "/home/user");
        assert_eq!(clean_title("user@host:~/workspace", None), "~/workspace");
        assert_eq!(clean_title("user@host", None), "user@host");
        assert_eq!(clean_title("some other title", None), "some other title");

        if let Ok(home) = std::env::var("HOME") {
            assert_eq!(clean_cwd(&home), "~");
            assert_eq!(clean_cwd(&format!("{}/workspace", home)), "~/workspace");
            // Intermediate directory components are abbreviated to their first
            // character; only the leaf component is kept in full.
            assert_eq!(
                clean_cwd(&format!("{}/Workspace/apps/winter-term", home)),
                "~/W/a/winter-term"
            );
        }
    }

    #[test]
    fn test_titlebar_path_shows_app_and_target() {
        let mut app = App::new();

        // With no cwd or foreground process, the terminal shows its name.
        assert_eq!(app.titlebar_path(), "Terminal");

        // When an editor with file reference / cwd is focused:
        struct MockEditor {
            cwd: Option<std::path::PathBuf>,
            root_dir: Option<std::path::PathBuf>,
            file_ref: Option<String>,
            address: Option<String>,
        }
        impl Editor for MockEditor {
            fn title(&self) -> String {
                "sample.rs".to_string()
            }
            fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
                Frame::default()
            }
            fn on_key(&mut self, _key: &Key) -> EditorOutcome {
                EditorOutcome::Consumed
            }
            fn cwd(&self) -> Option<std::path::PathBuf> {
                self.cwd.clone()
            }
            fn root_dir(&self) -> Option<std::path::PathBuf> {
                self.root_dir.clone().or_else(|| self.cwd.clone())
            }
            fn file_reference(&self) -> Option<String> {
                self.file_ref.clone()
            }
            fn address(&self) -> Option<String> {
                self.address.clone()
            }
        }

        let _editor = app.show_editor(
            EditorKind::Text,
            Box::new(MockEditor {
                cwd: Some(std::path::PathBuf::from("/home/user/project")),
                root_dir: None,
                file_ref: Some("~/project/src/lib.rs:42".to_string()),
                address: Some("~/project/src/lib.rs".to_string()),
            }),
        );
        assert_eq!(app.titlebar_path(), "Text Editor: ~/project/src/lib.rs");

        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
        let _files = app.show_editor(
            EditorKind::Dir,
            Box::new(MockEditor {
                cwd: Some(std::path::PathBuf::from(format!("{home}/docs/subfolder"))),
                root_dir: Some(std::path::PathBuf::from(format!("{home}/docs"))),
                file_ref: Some("~/docs/subfolder/active_file.txt".to_string()),
                address: Some("~/docs".to_string()),
            }),
        );
        assert_eq!(app.titlebar_path(), "Dir: ~/docs");

        let _keys = app.show_editor(
            EditorKind::Keys,
            Box::new(MockEditor {
                cwd: None,
                root_dir: None,
                file_ref: None,
                address: None,
            }),
        );
        assert_eq!(app.titlebar_path(), "Keys");

        // The bar names the editor's address, not wherever its cursor has
        // wandered: the folder or reference it follows is ignored.
        let _git = app.show_editor(
            EditorKind::Git,
            Box::new(MockEditor {
                cwd: Some(std::path::PathBuf::from("/home/user/repo/src")),
                root_dir: None,
                file_ref: Some("~/repo/src".to_string()),
                address: Some("~/repo".to_string()),
            }),
        );
        assert_eq!(app.titlebar_path(), "Git: ~/repo");

        // An editor with no address of its own is named by its kind alone, even
        // when it has a cursor-following folder to offer.
        let _procs = app.show_editor(
            EditorKind::Proc,
            Box::new(MockEditor {
                cwd: Some(std::path::PathBuf::from("/home/user/work")),
                root_dir: None,
                file_ref: Some("~/work".to_string()),
                address: None,
            }),
        );
        assert_eq!(app.titlebar_path(), "Processes");
    }

    #[test]
    fn test_top_tabbar_shows_mode_tooltip_when_mode_is_hovered() {
        let mut app = App::new();
        struct NormalEditor;
        impl Editor for NormalEditor {
            fn title(&self) -> String {
                "mock".to_string()
            }
            fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
                Frame::default()
            }
            fn on_key(&mut self, _key: &Key) -> EditorOutcome {
                EditorOutcome::Consumed
            }
            fn status(&self) -> crate::model::editor::EditorStatus {
                crate::model::editor::EditorStatus {
                    mode: Some(crate::model::textedit::modal::EditMode::Normal),
                    segments: Vec::new(),
                }
            }
        }
        app.show_editor(EditorKind::Text, Box::new(NormalEditor));
        app.chrome.status_hover = Some(StatusHover::Mode);
        app.pointer.cursor_pos = (12.0, 500.0);
        let tabbar = app.build_top_tabbar();
        assert_eq!(
            tabbar.url_tooltip,
            Some(("Normal".to_string(), 12.0, 500.0))
        );
    }

    #[test]
    fn test_a_strip_tab_s_title_hangs_under_the_tab_not_the_pointer() {
        let mut app = App::new();
        let tab = app.layout().focused();
        let (_, strip) = app.strip_areas()[0];
        // A point inside the strip's first tab, wherever the pointer is
        // within it.
        let (x, y) = (strip.x + 6.0, strip.y + 2.0);
        assert_eq!(app.strip_hit(x, y), StripHit::Tab(tab));
        let region = app.strip_tab_region(tab, x, y).expect("the tab's place");
        assert!(region.x <= x && x < region.x + region.w);
        assert!(region.y <= y && y < region.y + region.h);
        app.pointer.cursor_pos = (x, y);
        app.tabs.strip_hover = StripHit::Tab(tab);
        app.tabs.hover_anchor = Some(region);
        let tabbar = app.build_top_tabbar();
        assert_eq!(tabbar.tooltip_anchor, Some(region));
        app.tabs.strip_hover = StripHit::NewTab(tab);
        assert_eq!(
            app.build_top_tabbar().tooltip_anchor,
            None,
            "a button's hangs under the pointer"
        );
    }

    #[test]
    fn test_hover_tooltip_for_buttons() {
        let mut app = App::new();
        app.pointer.cursor_pos = (50.0, 10.0);

        // Strip buttons
        app.tabs.strip_hover = StripHit::NewTab(TabId(0));
        assert_eq!(app.hover_tooltip(), Some(("New Tab".to_string(), 50.0, 10.0)));

        app.tabs.strip_hover = StripHit::Close(TabId(0));
        assert_eq!(app.hover_tooltip(), Some(("Close Tab".to_string(), 50.0, 10.0)));

        // Window control buttons
        app.tabs.strip_hover = StripHit::None;
        app.tabs.hover = TabbarHit::Hamburger;
        assert_eq!(app.hover_tooltip(), Some(("Menu".to_string(), 50.0, 10.0)));

        app.tabs.hover = TabbarHit::Minimize;
        assert_eq!(app.hover_tooltip(), Some(("Minimize".to_string(), 50.0, 10.0)));

        app.tabs.hover = TabbarHit::Maximize;
        assert_eq!(app.hover_tooltip(), Some(("Maximize".to_string(), 50.0, 10.0)));

        app.tabs.hover = TabbarHit::Close;
        assert_eq!(app.hover_tooltip(), Some(("Close".to_string(), 50.0, 10.0)));

        // Status bar bell
        app.tabs.hover = TabbarHit::None;
        app.chrome.status_hover = Some(StatusHover::Bell);
        assert_eq!(app.hover_tooltip(), Some(("Notifications".to_string(), 50.0, 10.0)));

        // Terminal link takes priority
        app.pointer.hovered_url = Some("https://example.com".to_string());
        assert_eq!(
            app.hover_tooltip(),
            Some(("https://example.com".to_string(), 50.0, 10.0))
        );
    }
}
