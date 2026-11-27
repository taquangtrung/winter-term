//! What the window's tab bar says of its tabs: their names, the tooltip for
//! what the pointer rests on, and the tab commands. The title bar itself is
//! misty's (see [`crate::window::frame::title_bar`]); the menus live in
//! [`crate::window::menus`], the right-click menu in
//! [`crate::window::menus::context`]; the window title in [`title`].

mod title;

use std::path::Path;

use crate::render::{Region, StatusHover};
use misty::Rect;
use misty::widgets::{TitleBarHit, TooltipPlacement, WindowButton};

use winter_model::layout::TabId;
use winter_model::path::abbreviate_home;
use winter_terminal::session::TitleFacts;

use crate::window::App;
use crate::window::editor::EditorKind;
use crate::window::editor::buffers::shown_path;
use crate::window::panels::ViewKind;
use crate::window::status::mode_label_and_name;
use crate::window::strip::StripHit;

// ========================================================================
// Constants
// ========================================================================

/// What a terminal is called where nothing better names it: no program, no
/// directory, no title of its own.
const TERMINAL_NAME: &str = "Terminal";

/// What the tooltips of a strip's buttons say, before their keys.
const CLOSE_TAB: &str = "Close Tab";
const NEW_TAB: &str = "New Tab";
const NEXT_TAB: &str = "Next Tab";
const PREVIOUS_TAB: &str = "Previous Tab";

/// What a tab's tooltip adds of a tab with unsaved changes, of one still
/// running, and of an agent waiting for the user.
const UNSAVED: &str = "unsaved changes";
const RUNNING: &str = "running";
const WAITING: &str = "waiting for you";

/// What the mode's tooltip adds of the marks beside it: a macro being
/// recorded, and typing going to every pane.
const RECORDING: &str = "recording a macro";
const BROADCASTING: &str = "typing into every pane";

/// What parts a breadcrumb's trail in its tooltip.
const CRUMB_TRAIL: &str = " \u{203a} ";

// ========================================================================
// Data Structures
// ========================================================================

/// A tooltip for what the pointer rests on: what it says, the part of the
/// window it is about, and which side of that part it shows on.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct HoverTip {
    /// The part it is about, or `None` for one that hangs off the pointer.
    pub(in crate::window) anchor: Option<Rect>,
    /// Which side of the part it shows on.
    pub(in crate::window) placement: TooltipPlacement,
    /// What it says.
    pub(in crate::window) text: String,
}

/// Which of its two names a tab is called by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NameLength {
    /// Everywhere but the tab bar: what it shows in full, its path written
    /// whole.
    Full,
    /// In the tab bar, where room is short.
    Short,
}

// ========================================================================
// App: tabbar model and interaction
// ========================================================================

impl App {
    /// A tab's short name, the one the tab bar shows: a name the user gave
    /// it; for a terminal with nothing running in it, its directory's name,
    /// with as many of the directories above it as tell it from another such
    /// tab's; else what it shows, its directory shortened.
    pub(in crate::window) fn tab_title(&self, tab: TabId) -> String {
        if self.tabs.name(tab).is_none()
            && let Some(cwd) = self.idle_directory(tab)
        {
            let others: Vec<String> = self
                .open_tabs
                .terminals()
                .filter(|(other, _)| **other != tab && self.tabs.name(**other).is_none())
                .filter_map(|(other, _)| self.idle_directory(*other))
                .collect();
            return distinct_name(&cwd, &others);
        }
        self.tab_name(tab, NameLength::Short)
    }

    /// A tab's full name, the one used everywhere but the tab bar (the window
    /// title, the tab's tooltip, the lists of tabs): an editor by the full
    /// path of what it shows, else its own label; a terminal by what it says
    /// of itself, else its program and the full path of its directory,
    /// `Terminal: ~/dev/winter-term` while nothing runs in it. A name the
    /// user gave the tab is its short name only.
    pub(in crate::window) fn tab_full_name(&self, tab: TabId) -> String {
        self.tab_name(tab, NameLength::Full)
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

    /// A tab's name at `length`: for the short one a name the user gave it,
    /// then an editor's own label (the full path it shows, for the full
    /// one), else what the terminal says about itself.
    fn tab_name(&self, focused: TabId, length: NameLength) -> String {
        if length == NameLength::Short
            && let Some(name) = self.tabs.name(focused)
        {
            return name.to_string();
        }
        if let Some(slot) = self.open_tabs.page(&focused) {
            let path = match length {
                NameLength::Full => slot.page.as_editor().and_then(shown_path),
                NameLength::Short => None,
            };
            return path.unwrap_or_else(|| slot.tab_label());
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
        terminal_name(facts, length)
    }
}

/// `label` with the keys that run it after it, as `keys`, or alone when no
/// keys do.
fn with_keys(label: &str, keys: &str) -> String {
    match keys.is_empty() {
        true => label.to_string(),
        false => format!("{label} ({keys})"),
    }
}

/// `region` as misty's rectangle.
fn region_rect(region: Region) -> Rect {
    Rect::new(region.x, region.y, region.w, region.h)
}

/// A terminal's name at `length`, from what it says of itself: its program
/// and where that works, else where its shell is. The short name shortens
/// the directory and gives an idle shell's alone; the full one writes it
/// whole, `Terminal: ~/dev/winter-term` for an idle shell.
fn terminal_name(facts: &TitleFacts, length: NameLength) -> String {
    let place = match length {
        NameLength::Full => place_of(facts, |cwd| abbreviate_home(Path::new(cwd))),
        NameLength::Short => place_of(facts, clean_cwd),
    };
    match (facts.program.as_deref(), place, length) {
        (Some(program), place, _) => labelled(program, place),
        (None, place, NameLength::Full) => labelled(TERMINAL_NAME, place),
        (None, Some(place), NameLength::Short) => place,
        (None, None, NameLength::Short) => TERMINAL_NAME.to_string(),
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
    let others: Vec<Vec<&str>> = others
        .iter()
        .map(|other| path_parts(other))
        .filter(|other| *other != parts)
        .collect();
    let count = (1..=parts.len())
        .find(|count| {
            others
                .iter()
                .all(|other| !other.ends_with(&parts[parts.len() - count..]))
        })
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
    let is_user_at_host = lead.split_once('@').is_some_and(|(user, host)| {
        !user.is_empty() && !host.is_empty() && !lead.contains(char::is_whitespace)
    });
    match (is_user_at_host, remote) {
        (true, Some(remote)) => format!("{remote}: {}", rest.trim()),
        (true, None) => rest.trim().to_string(),
        (false, _) => title.to_string(),
    }
}

impl App {
    /// What the pointer rests on has to say, and where its tooltip hangs:
    /// under a part of the window's top (a strip's tab or button, a title
    /// bar button, a toolbar button), over a part of the status bar, and
    /// off the pointer for a link, which has no part of its own.
    pub(in crate::window) fn hover_tooltip(&self) -> Option<HoverTip> {
        let (cx, cy) = self.input.pointer.cursor_pos;
        let below = |text: String, anchor: Option<Rect>| HoverTip {
            anchor,
            placement: TooltipPlacement::CenteredBelow,
            text,
        };
        if let Some(url) = &self.input.pointer.hovered_url {
            return Some(below(url.clone(), None));
        }
        let strip = || self.strip_part_rect(cx, cy);
        let keyed = |label: &str, command: &str| with_keys(label, &self.keymap.chord_hint(command));
        match self.tabs.strip_hover {
            StripHit::Close(_) => return Some(below(keyed(CLOSE_TAB, "close_tab"), strip())),
            StripHit::NewTab(_) => return Some(below(keyed(NEW_TAB, "new_terminal"), strip())),
            StripHit::NextTab => return Some(below(keyed(NEXT_TAB, "next_tab"), strip())),
            StripHit::PrevTab => return Some(below(keyed(PREVIOUS_TAB, "prev_tab"), strip())),
            StripHit::Tab(tab) => {
                return self.strip_tab_tooltip(tab).map(|text| below(text, strip()));
            }
            StripHit::None => {}
        }
        let window_button = |button: WindowButton| {
            let (_, bar) = self.parts.title_bar.as_ref()?;
            bar.button_rect(button)
        };
        let title_bar = match self.title_bar_hit(cx, cy) {
            Some(TitleBarHit::MenuButton) => {
                let rect = self
                    .parts
                    .title_bar
                    .as_ref()
                    .and_then(|(_, bar)| bar.menu_button());
                Some(("Menu", rect))
            }
            Some(TitleBarHit::Button(button @ WindowButton::Minimize)) => {
                Some(("Minimize", window_button(button)))
            }
            Some(TitleBarHit::Button(button @ WindowButton::Maximize)) => {
                let is_max = self.window.as_ref().is_some_and(|w| w.is_maximized());
                Some((
                    if is_max { "Restore" } else { "Maximize" },
                    window_button(button),
                ))
            }
            Some(TitleBarHit::Button(button @ WindowButton::Close)) => {
                Some(("Close", window_button(button)))
            }
            Some(TitleBarHit::Drag | TitleBarHit::MenuTitle(_) | TitleBarHit::Resize(_)) | None => {
                None
            }
            // A part misty adds later has no label yet.
            Some(_) => None,
        };
        if let Some((label, rect)) = title_bar {
            return Some(below(label.to_string(), rect));
        }
        if let Some(part) = self.parts.status_hover {
            let text = match part {
                StatusHover::Mode => self.mode_tooltip(),
                StatusHover::Bell => Some(match self.parts.notifier.log().unread() {
                    0 => "Notifications".to_string(),
                    1 => "1 unread notification".to_string(),
                    n => format!("{n} unread notifications"),
                }),
                StatusHover::Segment(index) => self.status_segment_tooltip(index),
                StatusHover::Progress => self.status_progress_tooltip(),
                StatusHover::Branch | StatusHover::Host | StatusHover::Project => {
                    self.status_place_tooltip(part)
                }
            };
            if let Some(text) = text {
                let anchor = self.status_part_region(part).map(region_rect);
                return Some(HoverTip {
                    anchor,
                    placement: TooltipPlacement::CenteredAbove,
                    text,
                });
            }
        }
        if let Some((owner, button)) = self.input.pointer.hovered_toolbar_button
            && let Some(tip) = self.toolbar_button_tooltip(owner, button)
        {
            return Some(below(tip, self.toolbar_button_rect(owner, button)));
        }
        let (text, anchor) = self.gutter_tooltip(cx, cy)?;
        Some(below(text, Some(anchor)))
    }

    /// The gutter row at `(x, y)` as the gutters were last drawn, and whose
    /// tab it is.
    pub(in crate::window) fn gutter_row_at(&self, x: f32, y: f32) -> Option<(TabId, usize)> {
        let point = misty::Point::new(x, y);
        let (tab, strip) = self
            .parts
            .gutter_strips
            .iter()
            .find(|(_, strip)| strip.contains(point))?;
        let cell_h = self.renderer.as_ref()?.cell_size().1.max(1.0);
        Some((*tab, ((y - strip.y) / cell_h).floor() as usize))
    }

    /// What the gutter mark at `(x, y)` says, and the row it marks: a
    /// terminal command's status, or what an editor's mark means.
    fn gutter_tooltip(&self, x: f32, y: f32) -> Option<(String, Rect)> {
        let (tab, row) = self.gutter_row_at(x, y)?;
        let (_, strip) = self.parts.gutter_strips.iter().find(|(at, _)| *at == tab)?;
        let cell_h = self.renderer.as_ref()?.cell_size().1;
        let text = match self.open_tabs.terminal(&tab) {
            Some(terminal) => crate::window::terminal::command_marks::mark_tooltip(terminal, row)?,
            None => self.open_tabs.page(&tab)?.page.gutter_tooltip(row)?,
        };
        let anchor = Rect::new(strip.x, strip.y + row as f32 * cell_h, strip.width, cell_h);
        Some((text, anchor))
    }

    /// What a strip tab's tooltip says: its full name where the strip cut
    /// it or named it shorter, and whether it has unsaved changes, is
    /// running, or holds an agent waiting for you; `None` when it would
    /// only repeat the tab.
    fn strip_tab_tooltip(&self, tab: TabId) -> Option<String> {
        // A sidebar's switcher shows only icons: its tooltip names the view.
        let kind = self.open_tabs.page(&tab).map(|slot| slot.kind());
        if let Some(EditorKind::View(view @ ViewKind::Sidebar(_))) = kind {
            let keys = self.keymap.chord_hint(view.command());
            return Some(with_keys(view.title(), &keys));
        }
        let full = self.tab_full_name(tab);
        let shown_whole = full == self.strip_title(tab) && !self.strip_title_cut(tab);
        let mut said = match shown_whole {
            true => Vec::new(),
            false => vec![full],
        };
        let modified = self
            .open_tabs
            .page(&tab)
            .and_then(|slot| slot.page.as_editor())
            .is_some_and(|editor| editor.is_dirty());
        if modified {
            said.push(UNSAVED.to_string());
        }
        if self.busy.spinning.contains(&tab) {
            said.push(RUNNING.to_string());
        }
        if self.tabs.agents.unseen(tab) {
            said.push(WAITING.to_string());
        }
        (!said.is_empty()).then(|| said.join(", "))
    }

    /// Tooltip text for toolbar button `button` of pane `owner`.
    /// A breadcrumb's is the trail up to it, which the row may have cut.
    fn toolbar_button_tooltip(&self, owner: u64, button: u16) -> Option<String> {
        let editor = self.open_tabs.page(&TabId(owner))?;
        match crate::window::layout::bands::crumb_of(button) {
            Some(crumb) => {
                let crumbs = editor.page.crumbs();
                let trail = crumbs.get(..=crumb)?;
                Some(trail.join(CRUMB_TRAIL))
            }
            None => editor.page.toolbar_tooltip(button),
        }
    }

    /// What the mode's place in the status bar means: the mode in full,
    /// and the marks beside it.
    fn mode_tooltip(&self) -> Option<String> {
        let focused = self.layout().focused();
        let editor = self.open_tabs.page(&focused).map(|slot| slot.page.status());
        let editor_mode = editor.as_ref().and_then(|status| status.mode);
        let mode = self.open_tabs.mode(&focused);
        let mut said: Vec<String> = mode_label_and_name(mode, editor_mode)
            .map(|(_, name)| format!("{name} mode"))
            .into_iter()
            .collect();
        // The marks that share the mode's place say what they mean.
        if self.input.macro_state.is_recording() {
            said.push(RECORDING.to_string());
        }
        if self.term.broadcast {
            said.push(BROADCASTING.to_string());
        }
        (!said.is_empty()).then(|| said.join(", "))
    }

    /// Pixel height of the title bar's band along the window's top.
    pub(in crate::window) fn top_bars_height(&self) -> f32 {
        self.title_bar_height()
    }

    /// A left-click at `(x, y)` on the title bar while a dialog is up: only
    /// what the window manager would do, touching nothing in the window,
    /// still happens. The minimize and maximize buttons work and empty
    /// title-bar space moves the window; its close button, menus, and the
    /// rest wait for the dialog's answer. Returns whether the click did one.
    pub(in crate::window) fn manage_window_at(&mut self, x: f32, y: f32) -> bool {
        let hit = self.title_bar_hit(x, y);
        let Some(window) = &self.window else {
            return false;
        };
        match hit {
            Some(TitleBarHit::Button(WindowButton::Minimize)) => window.set_minimized(true),
            Some(TitleBarHit::Button(WindowButton::Maximize)) => {
                window.set_maximized(!window.is_maximized())
            }
            Some(TitleBarHit::Drag) => {
                window.drag_window();
            }
            _ => return false,
        }
        true
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editors::keys::KeysEditor;
    use winter_model::editor::{Editor, EditorOutcome, Page};
    use winter_model::input::{Key, Keymap};
    use winter_model::layout::SidebarKind;
    use winter_model::paint::Frame;

    #[test]
    fn test_a_terminal_s_full_name_writes_its_directory_whole_and_its_short_one_shortens_it() {
        let facts = |program: Option<&str>| TitleFacts {
            cwd: Some("/srv/dev/winter-term".to_string()),
            host: winter_terminal::session::Host::Local,
            program: program.map(str::to_string),
        };
        let short_place = clean_cwd("/srv/dev/winter-term");
        assert_eq!(
            terminal_name(&facts(None), NameLength::Full),
            "Terminal: /srv/dev/winter-term",
            "an idle shell, named for what it is and where"
        );
        assert_eq!(terminal_name(&facts(None), NameLength::Short), short_place);
        assert_eq!(
            terminal_name(&facts(Some("cargo")), NameLength::Full),
            "cargo: /srv/dev/winter-term"
        );
        assert_eq!(
            terminal_name(&facts(Some("cargo")), NameLength::Short),
            format!("cargo: {short_place}")
        );
        let nowhere = TitleFacts {
            cwd: None,
            ..facts(None)
        };
        assert_eq!(terminal_name(&nowhere, NameLength::Full), TERMINAL_NAME);
    }

    #[test]
    fn test_an_editor_tab_s_full_name_is_the_path_it_shows_and_its_short_one_the_file_s_name() {
        let dir = std::env::temp_dir().join(format!("winter-tab-names-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("notes.txt");
        std::fs::write(&file, "text\n").expect("file");
        let mut app = App::new();
        let editor = winter_text::TextEditor::new(file.clone(), None).expect("opens");
        let tab = app.show_editor(EditorKind::Text, Box::new(editor));
        assert_eq!(app.tab_title(tab), "notes.txt");
        assert_eq!(app.tab_full_name(tab), file.display().to_string());
        app.tabs.set_name(tab, Some("work".to_string()));
        assert_eq!(app.tab_title(tab), "work", "a name given is the short name");
        assert_eq!(app.tab_full_name(tab), file.display().to_string());
        let _ = std::fs::remove_dir_all(&dir);
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
        let others = |paths: &[&str]| {
            paths
                .iter()
                .map(|path| path.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            distinct_name("/home/ann/dev/winter-term", &[]),
            "winter-term"
        );
        assert_eq!(
            distinct_name("/home/ann/dev/winter-term/", &[]),
            "winter-term",
            "a trailing slash"
        );
        assert_eq!(
            distinct_name("/", &[]),
            "/",
            "the root has no name but itself"
        );
        assert_eq!(
            distinct_name("/x/a/src", &others(&["/x/b/src", "/y/docs"])),
            "a/src"
        );
        assert_eq!(
            distinct_name("/p/a/src", &others(&["/q/a/src"])),
            "p/a/src",
            "as far up as it takes"
        );
        assert_eq!(
            distinct_name("/x/a/src", &others(&["/x/a/src"])),
            "src",
            "the same directory twice is no clash"
        );
    }

    #[test]
    fn test_only_a_user_at_host_lead_is_taken_off_a_title() {
        assert_eq!(clean_title("ann@box:~/notes", None), "~/notes");
        assert_eq!(clean_title("ann@box: ~/notes", None), "~/notes");
        assert_eq!(
            clean_title("mail me@x.com: draft", None),
            "mail me@x.com: draft",
            "an @ in the program's own title"
        );
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
        let keys = app.show_page(
            EditorKind::Keys,
            Box::new(KeysEditor::new(
                &Keymap::default(),
                Vec::new(),
                &Default::default(),
            )),
        );
        assert_eq!(
            app.tab_title(keys),
            "Keyboard Shortcuts",
            "a title restating the kind is not doubled up"
        );

        // The icon before the title says which kind the editor is, so the
        // title is not prefixed with it.
        struct Titled(&'static str);
        impl Page for Titled {
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

        impl Editor for Titled {}
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

        // The `_now` closes, as a running shell (still in its startup scripts)
        // would have the asking ones put a question instead.
        app.close_other_tabs_now(first);

        assert_eq!(app.layout().tabs_in_pane(first), vec![first]);
    }

    #[test]
    fn test_close_tabs_to_right_leaves_current_and_left_tabs() {
        let mut app = App::new();
        let t0 = app.layout().focused();
        app.new_tab();
        let t1 = app.layout().focused();
        app.new_tab();
        let t2 = app.layout().focused();
        app.new_tab();
        let t3 = app.layout().focused();
        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1, t2, t3]);

        app.close_tabs_to_right_now(t1);

        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1]);
    }

    #[test]
    fn test_close_tabs_to_left_leaves_current_and_right_tabs() {
        let mut app = App::new();
        let t0 = app.layout().focused();
        app.new_tab();
        let t1 = app.layout().focused();
        app.new_tab();
        let t2 = app.layout().focused();
        app.new_tab();
        let t3 = app.layout().focused();
        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1, t2, t3]);

        app.close_tabs_to_left_now(t2);

        assert_eq!(app.layout().tabs_in_pane(t2), vec![t2, t3]);
    }

    #[test]
    fn test_close_tabs_to_right_when_rightmost_changes_nothing() {
        let mut app = App::new();
        let t0 = app.layout().focused();
        app.new_tab();
        let t1 = app.layout().focused();
        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1]);

        app.close_tabs_to_right_now(t1);

        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1]);
    }

    #[test]
    fn test_close_tabs_to_left_when_leftmost_changes_nothing() {
        let mut app = App::new();
        let t0 = app.layout().focused();
        app.new_tab();
        let t1 = app.layout().focused();
        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1]);

        app.close_tabs_to_left_now(t0);

        assert_eq!(app.layout().tabs_in_pane(t0), vec![t0, t1]);
    }

    #[test]
    fn test_a_renamed_tab_keeps_its_name_over_an_editor_label() {
        // An explicit name is the user's own act; the editor's label must not
        // overwrite it.
        let mut app = App::new();
        let keys = app.show_page(
            EditorKind::Keys,
            Box::new(KeysEditor::new(
                &Keymap::default(),
                Vec::new(),
                &Default::default(),
            )),
        );
        app.tabs.set_name(keys, Some("work".to_string()));
        assert_eq!(app.tab_title(keys), "work");
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
    fn test_the_mode_s_tooltip_shows_when_the_mode_is_hovered() {
        let mut app = App::new();
        struct NormalEditor;
        impl Page for NormalEditor {
            fn title(&self) -> String {
                "mock".to_string()
            }

            fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
                Frame::default()
            }

            fn on_key(&mut self, _key: &Key) -> EditorOutcome {
                EditorOutcome::Consumed
            }

            fn status(&self) -> winter_model::editor::EditorStatus {
                winter_model::editor::EditorStatus {
                    mode: Some(winter_model::modal::EditMode::Normal),
                    segments: Vec::new(),
                }
            }
        }

        impl Editor for NormalEditor {}
        app.show_editor(EditorKind::Text, Box::new(NormalEditor));
        app.parts.status_hover = Some(StatusHover::Mode);
        app.input.pointer.cursor_pos = (12.0, 500.0);
        let tip = app.hover_tooltip().expect("a tooltip");
        assert_eq!(tip.text, "Normal mode");
        assert_eq!(
            tip.placement,
            TooltipPlacement::CenteredAbove,
            "over the bar"
        );
    }

    #[test]
    fn test_a_tab_s_tooltip_says_only_what_its_strip_does_not() {
        let mut app = App::new();
        let tab = app.layout().focused();
        let shown = app.strip_title(tab);
        let full = app.tab_full_name(tab);
        let said = app.strip_tab_tooltip(tab);
        match full == shown {
            true => assert_eq!(said, None, "nothing the tab does not say"),
            false => assert_eq!(said, Some(full)),
        }
        app.busy.spinning.insert(tab);
        let said = app.strip_tab_tooltip(tab).expect("a running tab says so");
        assert!(said.ends_with(RUNNING), "{said:?}");
    }

    #[test]
    fn test_a_sidebar_view_s_tooltip_names_it_and_its_keys() {
        let mut app = App::new();
        app.open_sidebar_view(SidebarKind::Git);
        let git = app.layout().focused();
        let keys = app.keymap.chord_hint("sidebar_git");
        let expected = match keys.is_empty() {
            true => "Git".to_string(),
            false => format!("Git ({keys})"),
        };
        assert_eq!(app.strip_tab_tooltip(git), Some(expected));
    }

    #[test]
    fn test_hover_tooltip_for_buttons() {
        let mut app = App::new();
        app.input.pointer.cursor_pos = (50.0, 10.0);
        let text = |app: &App| app.hover_tooltip().map(|tip| tip.text);

        // A strip's buttons name their keys where they have them.
        app.tabs.strip_hover = StripHit::NewTab(TabId(0));
        let keys = app.keymap.chord_hint("new_terminal");
        assert_eq!(text(&app), Some(with_keys(NEW_TAB, &keys)));
        assert_eq!(
            app.hover_tooltip().map(|tip| tip.placement),
            Some(TooltipPlacement::CenteredBelow),
            "under the window's top"
        );
        app.tabs.strip_hover = StripHit::None;

        app.parts.status_hover = Some(StatusHover::Bell);
        assert_eq!(text(&app), Some("Notifications".to_string()));

        // A link takes priority, and hangs off the pointer.
        app.input.pointer.hovered_url = Some("https://example.com".to_string());
        let tip = app.hover_tooltip().expect("the link's");
        assert_eq!(tip.text, "https://example.com");
        assert_eq!(tip.anchor, None);
    }

    #[test]
    fn test_keys_follow_a_label_only_where_some_run_it() {
        assert_eq!(with_keys("New Tab", "C-S-t"), "New Tab (C-S-t)");
        assert_eq!(with_keys("New Tab", ""), "New Tab");
    }
}
