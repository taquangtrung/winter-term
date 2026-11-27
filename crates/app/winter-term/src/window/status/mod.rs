//! The status bar, built for the focused tab: where commands run and the mode
//! on its left, then the segments `status-bar { segments ... }` names on its
//! right, each from its own source.
//!
//! A new segment is a [`StatusSegmentKind`] with its `settings.kdl` name, and
//! an arm in [`segments_of`] saying where its text comes from. An
//! extension's segments show what the extension last set them to.

mod view;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use misty::{Rgba, Span, StatusLine, StatusSegment, StyledText, TextColor, ThemeToken};
use winter_base::paths::is_local_host;
use winter_ext_api::Status;

use crate::render::{StatusHover, StatusSearch, THIN_SPACE, Theme, ThemeRgb};

use crate::config::{StatusBarConfig, StatusSegmentKind};
use winter_model::editor::EditorStatus;
use winter_model::layout::TabId;
use winter_model::modal::EditMode;
use winter_model::mode::Mode;
use winter_terminal::session::Host;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The status bar's glyph for a host reached over ssh: codicon `remote`, the
/// one VS Code marks a remote window with.
const SSH_HOST_ICON: char = '\u{eb3a}';

/// The status bar's glyph for a container: the Docker whale, in its lighter
/// Material Design cut so it sits with the outline glyphs beside it.
const CONTAINER_HOST_ICON: char = '\u{f0868}';

/// The icon drawn before the project's name: the project symbol icon.
const PROJECT_ICON: &str = "symbols/project";

/// What stands before the branch: the codicon `git-branch`.
const BRANCH_GLYPH: char = '\u{ea68}';

/// The id of the bell segment.
const BELL_ID: &str = "winter.status.bell";

/// The id of the mode segment.
const MODE_ID: &str = "winter.status.mode";

/// The names of the left side's host, project, and branch segments, for
/// their tooltips.
const HOST_ID: &str = "winter.status.host";
const PROJECT_ID: &str = "winter.status.project";
const BRANCH_ID: &str = "winter.status.branch";

/// What a macro being recorded adds to the mode segment.
const RECORDING_MARK: &str = " [REC]";

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

/// How long a directory's project and branch, read off the disk, stand
/// before they are read again: the bar is drawn every frame, each read
/// walks up the tree looking for `.git`, and a checkout shows this soon.
const PLACE_FOR: Duration = Duration::from_secs(2);

// ========================================================================
// Data Structures
// ========================================================================

/// Where the right side's segments get what they say.
struct SegmentSources<'a> {
    /// The editor that has the keyboard, if an editor does.
    editor: Option<FocusedEditor>,
    /// Each extension segment's icon, by its full id.
    extension_icons: &'a HashMap<&'static str, Arc<[u8]>>,
    /// What each extension segment shows, by its full id.
    extensions: &'a HashMap<&'static str, Status>,
}

/// What the focused editor gives the status bar: the glyph for its kind, and
/// where its keys stand.
struct FocusedEditor {
    status: EditorStatus,
}

/// Where the focused tab's commands run and what it works in: the host,
/// its directory, and that directory's project and branch.
struct FocusedPlace {
    branch: Option<String>,
    cwd: Option<PathBuf>,
    host: Host,
    project: String,
}

/// The project and branch the status bar last read off the disk, for the
/// one tab it shows: read again when the tab or its directory changes, or
/// once [`PLACE_FOR`] has passed.
#[derive(Default)]
pub(in crate::window) struct PlaceCache(RefCell<Option<ReadPlace>>);

/// A directory's project and branch as read, for which tab, and when.
struct ReadPlace {
    branch: Option<String>,
    cwd: PathBuf,
    project: String,
    read_at: Instant,
    tab: TabId,
}

// ========================================================================
// PlaceCache Implementations
// ========================================================================

impl PlaceCache {
    /// The project and branch of `cwd`, tab `tab`'s directory, as of `now`.
    fn project_and_branch(&self, tab: TabId, cwd: &Path, now: Instant) -> (String, Option<String>) {
        let mut slot = self.0.borrow_mut();
        let fresh = slot
            .as_ref()
            .filter(|read| read.tab == tab && read.cwd == cwd && now < read.read_at + PLACE_FOR);
        if let Some(read) = fresh {
            return (read.project.clone(), read.branch.clone());
        }
        let (project, branch) = discover_project_and_branch(cwd);
        *slot = Some(ReadPlace {
            branch: branch.clone(),
            cwd: cwd.to_path_buf(),
            project: project.clone(),
            read_at: now,
            tab,
        });
        (project, branch)
    }
}

// ========================================================================
// App: status bar
// ========================================================================

impl App {
    /// Where tab `focused`'s commands run and what it works in.
    fn focused_place(&self, focused: &TabId) -> FocusedPlace {
        // Read through the terminal's own cache of its processes: the bar is
        // drawn every frame, and each fresh read is a few files of `/proc`.
        let terminal = self.open_tabs.terminal(focused);
        let facts = terminal.map(|terminal| terminal.title_facts());
        let reported_host = terminal.and_then(|terminal| terminal.scrollback().cwd_host());
        let host = place_host(
            facts
                .as_ref()
                .map_or(Host::Local, |facts| facts.host.clone()),
            reported_host,
        );
        let cwd = match facts {
            Some(facts) => facts.cwd.map(PathBuf::from),
            None => self
                .open_tabs
                .page(focused)
                .and_then(|slot| slot.page.cwd()),
        };
        // A directory on another machine is not looked for on this one: the
        // same path here is some other repository, or none.
        let (project, branch) = match (&host, &cwd) {
            (Host::Local, Some(cwd)) => {
                self.parts
                    .place
                    .project_and_branch(*focused, cwd, Instant::now())
            }
            _ => Default::default(),
        };
        FocusedPlace {
            branch,
            cwd,
            host,
            project,
        }
    }

    /// The status bar for tab `focused`, whose terminal is in `mode` (`None`
    /// for an editor): where commands run, the mode, the project and branch,
    /// the search, the segments, the bell.
    pub(in crate::window) fn build_status_bar(
        &self,
        mode: Option<Mode>,
        theme: &Theme,
        search: Option<StatusSearch>,
        focused: &TabId,
    ) -> StatusLine {
        let editor = self.open_tabs.page(focused).map(|slot| FocusedEditor {
            status: slot.page.status(),
        });
        let editor_mode = editor.as_ref().and_then(|editor| editor.status.mode);
        let FocusedPlace {
            branch,
            host,
            project,
            ..
        } = self.focused_place(focused);
        let mut marks = String::new();
        if self.term.broadcast {
            marks.push_str(crate::window::terminal::broadcast::BROADCAST_MARK);
        }
        if self.input.macro_state.is_recording() {
            marks.push_str(RECORDING_MARK);
        }
        // A search being typed is shown in the find popup; the bar keeps
        // only how far it has got.
        let mut line = status_bar(
            mode,
            editor_mode,
            theme,
            &host_label(&host),
            &marks,
            None,
            search,
            &self.config.status_bar,
        );
        if !project.is_empty() {
            let icon = crate::icons::named_svg(PROJECT_ICON);
            let segment = StatusSegment::new(segment_text(&project)).with_name(PROJECT_ID);
            line = line.with_left(with_icon(segment, icon));
        }
        if let Some(branch) = branch.filter(|branch| !branch.is_empty()) {
            let text = segment_text(&format!("{BRANCH_GLYPH}  {branch}"));
            line = line.with_left(StatusSegment::new(text).with_name(BRANCH_ID));
        }
        // The running tasks' progress follows the message.
        let progress =
            StatusSegment::progress().with_details(self.config.status_bar.progress_details);
        line = line.with_after_primary(progress);
        let sources = SegmentSources {
            editor,
            extension_icons: self.host.extensions.segment_icons(),
            extensions: self.host.extensions.segments(),
        };
        let segments = segments_of(
            &self.config.status_bar.segments,
            &self.config.status_bar.priority,
            &sources,
        );
        // An extension's segment is told of a click; the rest are only
        // named, for their tooltips.
        for (index, (kind, segment)) in segments.into_iter().enumerate() {
            line = line.with_segment(match SEGMENT_IDS.get(index) {
                Some(id) if matches!(kind, StatusSegmentKind::Extension(_)) => segment.with_id(id),
                Some(id) => segment.with_name(id),
                None => segment,
            });
        }
        let bell = self.parts.status_bell();
        line.with_segment(
            StatusSegment::new(segment_text(""))
                .with_icon(bell.icon)
                .with_id(BELL_ID),
        )
    }

    /// What the left side's host, project, or branch segment means, for its
    /// tooltip: where commands run, the project and the directory worked
    /// in, the branch.
    pub(in crate::window) fn status_place_tooltip(&self, part: StatusHover) -> Option<String> {
        let place = self.focused_place(&self.layout().focused());
        match part {
            StatusHover::Host => match place.host {
                Host::Container(name) => Some(format!("Commands run in the container {name}")),
                Host::Ssh(name) => Some(format!("Commands run on {name} over SSH")),
                Host::Local => None,
            },
            StatusHover::Project => {
                let cwd = place.cwd?;
                let dir = winter_model::path::abbreviate_home(&cwd);
                Some(format!("Project {}, working in {dir}", place.project))
            }
            StatusHover::Branch => place.branch.map(|branch| format!("Git branch {branch}")),
            StatusHover::Bell
            | StatusHover::Mode
            | StatusHover::Progress
            | StatusHover::Segment(_) => None,
        }
    }

    /// What the progress item leaves out while its details are hidden: the
    /// oldest task's step, for its tooltip.
    pub(in crate::window) fn status_progress_tooltip(&self) -> Option<String> {
        if self.config.status_bar.progress_details {
            return None;
        }
        let task = self.parts.notifier.tasks().first()?;
        task.step().map(str::to_owned)
    }

    /// Tooltip text for segment `idx` in the status bar, if any.
    pub(in crate::window) fn status_segment_tooltip(&self, idx: usize) -> Option<String> {
        let (kind, segment) = self.status_segment_at(idx)?;
        let clean = plain_text(segment.text());
        if clean.is_empty() {
            return None;
        }
        match kind {
            // An editor's segments say in words what they mean, or nothing,
            // rather than repeating themselves.
            StatusSegmentKind::Editor => {
                let focused = self.layout().focused();
                let slot = self.open_tabs.page(&focused)?;
                slot.page.status_tooltip(&clean)
            }
            // An extension's says what the extension gave it, or nothing.
            StatusSegmentKind::Extension(id) => {
                self.host.extensions.segment_tip(id).map(str::to_owned)
            }
        }
    }

    /// Tell the extension whose segment is segment `idx` of the status bar's
    /// right side that it was clicked.
    pub(in crate::window) fn click_status_segment(&mut self, idx: usize) {
        if let Some((StatusSegmentKind::Extension(id), _)) = self.status_segment_at(idx) {
            self.click_extension_segment(id);
        }
    }

    /// Segment `idx` of the status bar's right side, as the bar numbers them
    /// (dropped ones counted), with the kind that said it.
    pub(in crate::window) fn status_segment_at(
        &self,
        idx: usize,
    ) -> Option<(StatusSegmentKind, StatusSegment)> {
        let focused = self.layout().focused();
        let editor = self.open_tabs.page(&focused).map(|slot| FocusedEditor {
            status: slot.page.status(),
        });
        let sources = SegmentSources {
            editor,
            extension_icons: self.host.extensions.segment_icons(),
            extensions: self.host.extensions.segments(),
        };
        segments_of(
            &self.config.status_bar.segments,
            &self.config.status_bar.priority,
            &sources,
        )
        .into_iter()
        .nth(idx)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The symbol and display name for `(mode, editor_mode)`.
pub(in crate::window) fn mode_label_and_name(
    mode: Option<Mode>,
    editor_mode: Option<EditMode>,
) -> Option<(&'static str, &'static str)> {
    match (mode, editor_mode) {
        (Some(Mode::Insert), _) | (None, Some(EditMode::Insert)) => Some(("▲", "Insert")),
        (Some(Mode::Normal), _) | (None, Some(EditMode::Normal)) => Some(("●", "Normal")),
        (Some(Mode::Visual), _) | (None, Some(EditMode::Visual)) => Some(("❖", "Visual")),
        (None, Some(EditMode::Replace)) => Some(("↻", "Replace")),
        (None, None) => None,
    }
}

/// The status bar's left side, for a terminal in `mode`, or an editor in
/// `editor_mode` (`None` for both: an editor with no mode of its own, showing
/// no mode label): `host` where commands run, then the mode after `marks`
/// (broadcasting, recording), with `prompt` (its label, what is typed, and
/// the caret) or `search` in the primary text's place. The letters and
/// colors are shared, so an editor in Insert reads as a terminal in Insert
/// does.
#[allow(clippy::too_many_arguments)]
fn status_bar(
    mode: Option<Mode>,
    editor_mode: Option<EditMode>,
    theme: &Theme,
    host: &str,
    marks: &str,
    prompt: Option<(&str, &str, usize)>,
    search: Option<StatusSearch>,
    config: &StatusBarConfig,
) -> StatusLine {
    let (mode_symbol, accent) = match (mode, editor_mode) {
        (Some(Mode::Insert), _) | (None, Some(EditMode::Insert)) => ("▲", theme.ansi[12]),
        (Some(Mode::Normal), _) | (None, Some(EditMode::Normal)) => ("●", theme.ansi[4]),
        (Some(Mode::Visual), _) | (None, Some(EditMode::Visual)) => ("❖", theme.ansi[6]),
        (None, Some(EditMode::Replace)) => ("↻", theme.ansi[12]),
        (None, None) => ("", theme.foreground),
    };
    let mode_label = if config.show_mode && prompt.is_none() && !mode_symbol.is_empty() {
        format!("{marks} {mode_symbol}")
    } else {
        marks.to_string()
    };
    let mut line = StatusLine::new(search_text(search));
    if let Some((label, text, caret)) = prompt {
        line = line.with_prompt(label, text, caret);
    }
    if !host.is_empty() {
        line = line.with_left(StatusSegment::new(segment_text(host)).with_name(HOST_ID));
    }
    if !mode_label.is_empty() {
        let text = StyledText::from(Span::new(mode_label.trim(), color(accent)));
        line = line.with_left(StatusSegment::new(text).with_name(MODE_ID));
    }
    line
}

/// What the bar says of `search`: its query after `/` (or `?` backward),
/// then which match of how many; nothing without one.
fn search_text(search: Option<StatusSearch>) -> StyledText {
    let Some(search) = search else {
        return StyledText::new();
    };
    let prefix = if search.reverse { '?' } else { '/' };
    let count = match search.match_total {
        0 => String::new(),
        total => format!("  {}/{total}", search.match_index),
    };
    StyledText::plain(
        format!("{prefix}{}{count}", search.query),
        TextColor::Token(ThemeToken::Foreground),
    )
}

/// How the status bar names where commands run: empty for this local machine
/// (so the mode indicator sits directly at the left edge), or the glyph then
/// the name for a remote host or a container.
fn host_label(host: &Host) -> String {
    match host {
        Host::Container(name) => format!("{CONTAINER_HOST_ICON} {name}"),
        Host::Local => String::new(),
        Host::Ssh(name) => format!("{SSH_HOST_ICON} {name}"),
    }
}

/// The right side's segments for `kinds`, in their order, each beside the
/// kind it is for. A kind with nothing to say (an extension's told nothing,
/// no editor focused) shows nothing. Each
/// holds on by its place in `priority`, the first listed longest; with none
/// listed they all rank alike and the bar drops from the left.
fn segments_of(
    kinds: &[StatusSegmentKind],
    priority: &[StatusSegmentKind],
    sources: &SegmentSources,
) -> Vec<(StatusSegmentKind, StatusSegment)> {
    kinds
        .iter()
        .flat_map(|kind| {
            let rank = priority
                .iter()
                .position(|listed| listed == kind)
                .map_or(0, |at| (priority.len() - at) as u32);
            let segments = match kind {
                StatusSegmentKind::Editor => editor_segments(sources),
                StatusSegmentKind::Extension(id) => extension_segment(sources, id),
            };
            segments
                .into_iter()
                .map(move |segment| (*kind, segment.with_priority(rank)))
        })
        .collect()
}

/// Each fact the focused editor reports, as a segment of its own so the bar
/// drops them one at a time when short of room. The editor's icon is left to
/// its tab, which already shows which editor is open.
fn editor_segments(sources: &SegmentSources) -> Vec<StatusSegment> {
    let Some(editor) = &sources.editor else {
        return Vec::new();
    };
    editor
        .status
        .segments
        .iter()
        .map(|text| StatusSegment::new(segment_text(&widened(text))))
        .collect()
}

/// What the extension segment `id` was last told to show, if anything:
/// after its icon, dimmed when stale, shortened as the extension said where
/// the bar is short of room.
fn extension_segment(sources: &SegmentSources, id: &str) -> Vec<StatusSegment> {
    sources
        .extensions
        .get(id)
        .map(|status| {
            let segment =
                StatusSegment::new(segment_text(&widened(&status.text))).with_stale(status.stale);
            let segment = with_icon(segment, sources.extension_icons.get(id).cloned());
            match &status.short {
                Some(short) => segment.with_short(segment_text(&widened(short))),
                None => segment,
            }
        })
        .into_iter()
        .collect()
}

/// `segment` after `icon`, drawn in its own colors, when it has one.
fn with_icon(segment: StatusSegment, icon: Option<Arc<[u8]>>) -> StatusSegment {
    match icon {
        Some(icon) => segment.with_svg_icon(icon),
        None => segment,
    }
}

/// `text` as a segment of the bar shows it, in the text color.
fn segment_text(text: &str) -> StyledText {
    StyledText::plain(text, TextColor::Token(ThemeToken::Foreground))
}

/// Misty's text color for Winter's `color`.
fn color(color: ThemeRgb) -> TextColor {
    TextColor::Rgba(Rgba::new(color.r, color.g, color.b, u8::MAX))
}

/// `text` with each [`THIN_SPACE`] a whole space, as the bar's cells draw
/// it.
fn widened(text: &str) -> String {
    text.replace(THIN_SPACE, " ")
}

/// What `text` says, without its colors.
fn plain_text(text: &StyledText) -> String {
    text.spans().iter().map(|span| span.text.as_str()).collect()
}

/// Find the project name and VC (git) branch for `dir`.
///
/// If `dir` sits inside a Git repository, returns the repository root folder's
/// name as the project name, and the current branch name (or short commit hash
/// if detached).
/// If `dir` is outside Git, returns `dir`'s folder name as the project name
/// with no branch.
pub fn discover_project_and_branch(dir: &Path) -> (String, Option<String>) {
    let mut current = Some(dir);
    while let Some(path) = current {
        let dot_git = path.join(".git");
        if dot_git.exists() {
            let project_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            let branch = resolve_git_branch(&dot_git, path);
            return (project_name, branch);
        }
        current = path.parent();
    }

    let project_name = if let Some(home) = winter_base::paths::home_dir() {
        if dir == home {
            "~".to_string()
        } else {
            dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        }
    } else {
        dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    };
    (project_name, None)
}

/// Where a terminal's commands run: `host` as its processes say, unless they
/// say this machine while its shell reports a directory on `reported`, the
/// host its last OSC 7 named, which is another one (an `ssh` they did not
/// show, run under `tmux` or `mosh`).
fn place_host(host: Host, reported: Option<&str>) -> Host {
    match (host, reported) {
        (Host::Local, Some(name)) if !is_local_host(name) => Host::Ssh(name.to_string()),
        (host, _) => host,
    }
}

/// Read the current Git branch name from `.git` (or a worktree gitdir file).
fn resolve_git_branch(dot_git: &Path, repo_root: &Path) -> Option<String> {
    let head_path = if dot_git.is_dir() {
        dot_git.join("HEAD")
    } else if dot_git.is_file() {
        let content = std::fs::read_to_string(dot_git).ok()?;
        let gitdir_str = content.trim().strip_prefix("gitdir:")?.trim();
        let gitdir_path = Path::new(gitdir_str);
        let resolved = if gitdir_path.is_relative() {
            repo_root.join(gitdir_path)
        } else {
            gitdir_path.to_path_buf()
        };
        resolved.join("HEAD")
    } else {
        return None;
    };

    let content = std::fs::read_to_string(head_path).ok()?;
    let trimmed = content.trim();
    if let Some(branch) = trimmed.strip_prefix("ref: refs/heads/") {
        Some(branch.to_string())
    } else if let Some(branch) = trimmed.strip_prefix("ref: ") {
        Some(branch.to_string())
    } else if !trimmed.is_empty() {
        let short = if trimmed.len() > 7 {
            &trimmed[..7]
        } else {
            trimmed
        };
        Some(format!("({short})"))
    } else {
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_project_icon_is_one_the_icon_set_has() {
        // A name the set lacks would draw the glyph instead, silently.
        assert!(crate::icons::named_svg(PROJECT_ICON).is_some());
    }

    fn shown_mode() -> StatusBarConfig {
        StatusBarConfig {
            show_mode: true,
            ..StatusBarConfig::default()
        }
    }

    fn bar(mode: Option<Mode>, editor_mode: Option<EditMode>, cfg: &StatusBarConfig) -> StatusLine {
        status_bar(mode, editor_mode, &Theme::dark(), "", "", None, None, cfg)
    }

    /// The mode segment's label, if `line` has one.
    fn mode(line: &StatusLine) -> Option<String> {
        mode_span(line).map(|span| span.text)
    }

    /// The color the mode segment is drawn in, if `line` has one.
    fn accent(line: &StatusLine) -> Option<TextColor> {
        mode_span(line).map(|span| span.color)
    }

    fn mode_span(line: &StatusLine) -> Option<Span> {
        let segment = line
            .left()
            .iter()
            .find(|segment| segment.id() == Some(MODE_ID))?;
        segment.text().spans().first().cloned()
    }

    #[test]
    fn test_status_bar_labels_each_mode() {
        let cfg = shown_mode();
        assert_eq!(
            mode(&bar(Some(Mode::Insert), None, &cfg)).as_deref(),
            Some("▲")
        );
        assert_eq!(
            mode(&bar(Some(Mode::Normal), None, &cfg)).as_deref(),
            Some("●")
        );
        assert_eq!(
            mode(&bar(Some(Mode::Visual), None, &cfg)).as_deref(),
            Some("❖")
        );
        assert_eq!(
            mode(&bar(None, Some(EditMode::Replace), &cfg)).as_deref(),
            Some("↻")
        );
        assert_ne!(
            accent(&bar(Some(Mode::Insert), None, &cfg)),
            accent(&bar(Some(Mode::Normal), None, &cfg))
        );
    }

    #[test]
    fn test_a_focused_editor_puts_its_own_mode_in_the_bar() {
        // An editor in Insert reads as a terminal in Insert does, and an
        // editor with no mode of its own shows no mode label.
        let cfg = shown_mode();
        let insert = bar(None, Some(EditMode::Insert), &cfg);
        assert_eq!(mode(&insert).as_deref(), Some("▲"));
        assert_eq!(
            accent(&insert),
            accent(&bar(Some(Mode::Insert), None, &cfg))
        );
        assert_eq!(mode(&bar(None, None, &cfg)), None);
    }

    #[test]
    fn test_marks_lead_the_mode_and_show_without_one() {
        let theme = Theme::dark();
        let cfg = shown_mode();
        let marked = status_bar(
            Some(Mode::Normal),
            None,
            &theme,
            "",
            " [REC]",
            None,
            None,
            &cfg,
        );
        assert_eq!(mode(&marked).as_deref(), Some("[REC] ●"));
        let unmoded = status_bar(None, None, &theme, "", " [REC]", None, None, &cfg);
        assert_eq!(mode(&unmoded).as_deref(), Some("[REC]"));
    }

    #[test]
    fn test_status_bar_show_mode_toggle_hides_mode_label() {
        let shown = StatusBarConfig::default();
        let hidden = StatusBarConfig {
            show_mode: false,
            ..StatusBarConfig::default()
        };
        assert_eq!(mode(&bar(Some(Mode::Normal), None, &hidden)), None);
        assert_eq!(
            mode(&bar(Some(Mode::Normal), None, &shown)).as_deref(),
            Some("●")
        );
    }

    #[test]
    fn test_status_bar_prompt_suppresses_mode_label() {
        let theme = Theme::dark();
        let cfg = shown_mode();
        let prompt = Some(("/", "foo", 3));
        let bar = status_bar(Some(Mode::Normal), None, &theme, "", "", prompt, None, &cfg);
        assert_eq!(mode(&bar), None, "prompt suppresses mode label");
        let prompt = bar.prompt().expect("the prompt is in the bar");
        assert_eq!(
            (prompt.label(), prompt.text(), prompt.caret()),
            ("/", "foo", 3)
        );
    }

    #[test]
    fn test_status_bar_carries_the_search_it_is_given() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig::default();
        let search = StatusSearch {
            query: "foo".to_string(),
            match_index: 1,
            match_total: 3,
            reverse: false,
        };
        let bar = status_bar(
            Some(Mode::Normal),
            None,
            &theme,
            "",
            "",
            None,
            Some(search),
            &cfg,
        );
        assert_eq!(plain_text(bar.primary()), "/foo  1/3");
        let unsearched = status_bar(Some(Mode::Normal), None, &theme, "", "", None, None, &cfg);
        assert_eq!(plain_text(unsearched.primary()), "");
    }

    #[test]
    fn test_segments_follow_the_configured_order_and_skip_what_has_nothing_to_say() {
        let extensions = HashMap::from([("probe.said", Status::new("said"))]);
        let icons = HashMap::new();
        let sources = SegmentSources {
            editor: Some(FocusedEditor {
                status: EditorStatus {
                    mode: None,
                    segments: vec!["12:5".to_string(), "340 lines".to_string()],
                },
            }),
            extension_icons: &icons,
            extensions: &extensions,
        };
        let kinds = [
            StatusSegmentKind::Editor,
            StatusSegmentKind::Extension("probe.said"),
            StatusSegmentKind::Extension("probe.quiet"),
        ];
        let texts: Vec<String> = segments_of(&kinds, &[], &sources)
            .into_iter()
            .map(|(_, segment)| plain_text(segment.text()))
            .collect();
        assert_eq!(
            texts,
            ["12:5", "340 lines", "said"],
            "the editor's facts apart and no icon, an extension's segment as told \
             and one told nothing silent"
        );
        let said_by: Vec<StatusSegmentKind> = segments_of(&kinds, &[], &sources)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        assert_eq!(
            said_by,
            [
                StatusSegmentKind::Editor,
                StatusSegmentKind::Editor,
                StatusSegmentKind::Extension("probe.said"),
            ],
            "a click on the third segment is on the extension's, not the third kind named"
        );
        assert!(segments_of(&[], &[], &sources).is_empty(), "nothing named");
        let ranked = segments_of(&kinds, &[StatusSegmentKind::Editor], &sources);
        let ranks: Vec<u32> = ranked
            .iter()
            .map(|(_, segment)| segment.priority())
            .collect();
        assert_eq!(
            ranks,
            [1, 1, 0],
            "the listed editor outranks the unlisted extension"
        );
    }

    #[test]
    fn test_an_extension_s_segment_shows_all_it_was_told() {
        // A status given in parts drew its text with the last one's tooltip.
        let status = Status::new("42%\u{2009}14:05")
            .with_short("42%")
            .with_stale(true);
        let extensions = HashMap::from([("probe.quiet", status)]);
        let icon: Arc<[u8]> = Arc::from(&b"<svg/>"[..]);
        let icons = HashMap::from([("probe.quiet", icon)]);
        let sources = SegmentSources {
            editor: None,
            extension_icons: &icons,
            extensions: &extensions,
        };
        let [segment] = <[StatusSegment; 1]>::try_from(extension_segment(&sources, "probe.quiet"))
            .unwrap_or_else(|_| panic!("one segment"));
        assert_eq!(
            plain_text(segment.text()),
            "42% 14:05",
            "a thin space widened"
        );
        assert_eq!(
            segment.short().map(plain_text).as_deref(),
            Some("42%"),
            "shortened as told"
        );
        assert!(segment.is_stale(), "dimmed when out of date");
        assert!(segment.svg_icon().is_some(), "after its icon");
    }

    #[test]
    fn test_host_label_empty_for_local_host_and_shows_icons_for_container_and_ssh() {
        assert_eq!(host_label(&Host::Local), "");
        assert_eq!(
            host_label(&Host::Container("my-docker".to_string())),
            format!("{CONTAINER_HOST_ICON} my-docker")
        );
        assert_eq!(
            host_label(&Host::Ssh("my-remote".to_string())),
            format!("{SSH_HOST_ICON} my-remote")
        );
    }

    #[test]
    fn test_a_place_is_read_off_the_disk_again_only_when_it_changes_or_goes_stale() {
        // Read every frame, the walk up the tree stalled the window on a slow
        // filesystem; read once and kept, a checkout never showed.
        let base = std::env::temp_dir().join(format!("winter-sb-cache-{}", std::process::id()));
        let repo = base.join("repo");
        let other = base.join("other");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(other.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(other.join(".git/HEAD"), "ref: refs/heads/dev\n").unwrap();
        let cache = PlaceCache::default();
        let start = Instant::now();
        let read = |tab, cwd: &Path, at| cache.project_and_branch(TabId(tab), cwd, at);
        assert_eq!(
            read(1, &repo, start),
            ("repo".to_string(), Some("main".to_string()))
        );

        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/topic\n").unwrap();
        assert_eq!(
            read(1, &repo, start).1.as_deref(),
            Some("main"),
            "kept within its time"
        );
        assert_eq!(
            read(1, &other, start).1.as_deref(),
            Some("dev"),
            "a `cd` is read at once"
        );
        assert_eq!(
            read(2, &repo, start).1.as_deref(),
            Some("topic"),
            "so is another tab"
        );
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/later\n").unwrap();
        assert_eq!(
            read(2, &repo, start + PLACE_FOR).1.as_deref(),
            Some("later"),
            "a checkout shows once the time is up"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_a_shell_reporting_another_machine_is_taken_to_be_there() {
        // Its path looked up here named a local repository the bar then showed.
        assert_eq!(
            place_host(Host::Local, Some("far-box")),
            Host::Ssh("far-box".to_string())
        );
        assert_eq!(place_host(Host::Local, Some("localhost")), Host::Local);
        assert_eq!(place_host(Host::Local, None), Host::Local);
        let container = Host::Container("db".to_string());
        assert_eq!(
            place_host(container.clone(), Some("far-box")),
            container,
            "its processes say better"
        );
    }

    #[test]
    fn test_discover_project_and_branch_git_repo_and_subdirectories() {
        let base = std::env::temp_dir().join(format!("winter-sb-git-{}", std::process::id()));
        let repo_root = base.join("my-awesome-project");
        let git_dir = repo_root.join(".git");
        let sub_dir = repo_root.join("src").join("nested");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&git_dir).unwrap();
        std::fs::create_dir_all(&sub_dir).unwrap();
        std::fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature/awesome\n").unwrap();

        // From root
        let (project, branch) = discover_project_and_branch(&repo_root);
        assert_eq!(project, "my-awesome-project");
        assert_eq!(branch, Some("feature/awesome".to_string()));

        // From nested subdirectory
        let (project_sub, branch_sub) = discover_project_and_branch(&sub_dir);
        assert_eq!(project_sub, "my-awesome-project");
        assert_eq!(branch_sub, Some("feature/awesome".to_string()));

        // Detached HEAD
        std::fs::write(
            git_dir.join("HEAD"),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4\n",
        )
        .unwrap();
        let (project_det, branch_det) = discover_project_and_branch(&repo_root);
        assert_eq!(project_det, "my-awesome-project");
        assert_eq!(branch_det, Some("(e3b0c44)".to_string()));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_discover_project_and_branch_worktree() {
        let base = std::env::temp_dir().join(format!("winter-sb-wt-{}", std::process::id()));
        let actual_git_dir = base.join("actual_git_dir");
        let worktree_dir = base.join("worktree-project");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&actual_git_dir).unwrap();
        std::fs::create_dir_all(&worktree_dir).unwrap();
        std::fs::write(actual_git_dir.join("HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::write(
            worktree_dir.join(".git"),
            format!("gitdir: {}\n", actual_git_dir.display()),
        )
        .unwrap();

        let (project, branch) = discover_project_and_branch(&worktree_dir);
        assert_eq!(project, "worktree-project");
        assert_eq!(branch, Some("main".to_string()));

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn test_discover_project_and_branch_non_git() {
        let base = std::env::temp_dir().join(format!("winter-sb-nongit-{}", std::process::id()));
        let dir = base.join("plain-folder");
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&dir).unwrap();

        let (project, branch) = discover_project_and_branch(&dir);
        assert_eq!(project, "plain-folder");
        assert_eq!(branch, None);

        let _ = std::fs::remove_dir_all(&base);
    }
}
