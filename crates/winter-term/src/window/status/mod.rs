//! The status bar, built for the focused tab: where commands run and the mode
//! on its left, then the segments `status-bar { segments ... }` names on its
//! right, each from its own source.
//!
//! A new segment is a [`StatusSegmentKind`] with its `settings.kdl` name, and
//! an arm in [`segments_of`] saying where its text comes from. The AI quota
//! segments' providers and API key live in [`quota`]; an extension's
//! segments say what the extension last set them to.

pub(in crate::window) mod quota;

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use winter_render::{StatusBar, StatusNotice, StatusPrompt, StatusSearch, StatusSegment, Theme};

use crate::config::{StatusBarConfig, StatusSegmentKind};
use crate::model::editor::EditorStatus;
use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::model::textedit::modal::EditMode;
use crate::quota::{describe, describe_compact, Provider, Window};
use crate::terminal::session::Host;

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

// ========================================================================
// Data Structures
// ========================================================================

/// Where the right side's segments get what they say.
struct SegmentSources<'a> {
    /// Each provider with a quota to show.
    quotas: Vec<(Provider, &'a [Window])>,
    /// The editor that has the keyboard, if an editor does.
    editor: Option<FocusedEditor>,
    /// What each extension segment says, by its full id.
    extensions: &'a HashMap<&'static str, String>,
}

/// What the focused editor gives the status bar: the glyph for its kind, and
/// where its keys stand.
struct FocusedEditor {
    status: EditorStatus,
}

// ========================================================================
// App: status bar
// ========================================================================

impl App {
    /// The status bar for tab `focused`, whose terminal is in `mode` (`None`
    /// for an editor).
    pub(in crate::window) fn build_status_bar(
        &self,
        mode: Option<Mode>,
        theme: &Theme,
        search: Option<StatusSearch>,
        notice: Option<StatusNotice>,
        focused: &TabId,
    ) -> StatusBar {
        let editor = self.open_tabs.editor(focused).map(|slot| FocusedEditor {
            status: slot.editor.status(),
        });
        let host = self
            .open_tabs
            .terminal(focused)
            .map_or(Host::Local, |terminal| terminal.host());
        let editor_mode = editor.as_ref().and_then(|editor| editor.status.mode);
        let cwd = self
            .open_tabs
            .terminal(focused)
            .and_then(|terminal| terminal.cwd().map(PathBuf::from))
            .or_else(|| {
                self.open_tabs
                    .editor(focused)
                    .and_then(|slot| slot.editor.cwd())
            });
        let (project, branch) = cwd
            .as_deref()
            .map(discover_project_and_branch)
            .unwrap_or_default();
        // A search being typed is shown in the find popup; the bar keeps
        // only how far it has got.
        let mut status = status_bar(
            mode,
            editor_mode,
            theme,
            None,
            search,
            notice,
            &self.config.status_bar,
        );
        let sources = SegmentSources {
            quotas: self
                .quota
                .as_ref()
                .map(|monitor| monitor.shown().collect())
                .unwrap_or_default(),
            editor,
            extensions: self.host.extensions.segments(),
        };
        status.host = host_label(&host);
        status.project_icon = (!project.is_empty())
            .then(|| crate::icons::named_svg(PROJECT_ICON))
            .flatten();
        status.project = project;
        status.branch = branch.unwrap_or_default();
        status.bell = Some(self.chrome.status_bell());
        status.hovered = self.chrome.status_hover;
        status.segments = segments_of(
            &self.config.status_bar.segments,
            &self.config.status_bar.priority,
            &sources,
        );
        if self.macro_state.is_recording() {
            status.mode = format!(" [REC]{}", status.mode);
        }
        status
    }

    /// Tooltip text for segment `idx` in the status bar, if any.
    pub(in crate::window) fn status_segment_tooltip(&self, idx: usize) -> Option<String> {
        let focused = self.layout().focused();
        let editor = self.open_tabs.editor(&focused).map(|slot| FocusedEditor {
            status: slot.editor.status(),
        });
        let sources = SegmentSources {
            quotas: self
                .quota
                .as_ref()
                .map(|monitor| monitor.shown().collect())
                .unwrap_or_default(),
            editor,
            extensions: self.host.extensions.segments(),
        };
        let segments = segments_of(
            &self.config.status_bar.segments,
            &self.config.status_bar.priority,
            &sources,
        );
        let segment = segments.get(idx)?;
        if segment.text.is_empty() {
            return None;
        }
        let clean = segment.text.replace(winter_render::THIN_SPACE, " ");
        Some(clean)
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
/// no mode label). The letters and colors are shared, so an editor in Insert
/// reads as a terminal in Insert does.
fn status_bar(
    mode: Option<Mode>,
    editor_mode: Option<EditMode>,
    theme: &Theme,
    prompt: Option<StatusPrompt>,
    search: Option<StatusSearch>,
    notice: Option<StatusNotice>,
    config: &StatusBarConfig,
) -> StatusBar {
    let (mode_symbol, accent) = match (mode, editor_mode) {
        (Some(Mode::Insert), _) | (None, Some(EditMode::Insert)) => ("▲", theme.ansi[12]),
        (Some(Mode::Normal), _) | (None, Some(EditMode::Normal)) => ("●", theme.ansi[4]),
        (Some(Mode::Visual), _) | (None, Some(EditMode::Visual)) => ("❖", theme.ansi[6]),
        (None, Some(EditMode::Replace)) => ("↻", theme.ansi[12]),
        (None, None) => ("", theme.foreground),
    };
    let mode_label = if config.show_mode && prompt.is_none() && !mode_symbol.is_empty() {
        format!(" {mode_symbol}")
    } else {
        String::new()
    };
    StatusBar {
        accent,
        bell: None,
        branch: String::new(),
        branch_icon: None,
        host: String::new(),
        hovered: None,
        mode: mode_label,
        notice,
        project: String::new(),
        project_icon: None,
        prompt,
        search,
        segments: Vec::new(),
    }
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

/// The right side's segments for `kinds`, in their order. A kind with
/// nothing to say (no quota heard, no editor focused) shows nothing. Each
/// holds on by its place in `priority`, the first listed longest; with none
/// listed they all rank alike and the bar drops from the left.
fn segments_of(
    kinds: &[StatusSegmentKind],
    priority: &[StatusSegmentKind],
    sources: &SegmentSources,
) -> Vec<StatusSegment> {
    kinds
        .iter()
        .flat_map(|kind| {
            let rank = priority
                .iter()
                .position(|listed| listed == kind)
                .map_or(0, |at| (priority.len() - at) as u32);
            let segments = match kind {
                StatusSegmentKind::Antigravity => quota_segment(sources, Provider::Antigravity),
                StatusSegmentKind::Claude => quota_segment(sources, Provider::Claude),
                StatusSegmentKind::Editor => editor_segments(sources),
                StatusSegmentKind::Extension(id) => extension_segment(sources, id),
                StatusSegmentKind::Zai => quota_segment(sources, Provider::Zai),
            };
            segments.into_iter().map(move |segment| StatusSegment {
                priority: rank,
                ..segment
            })
        })
        .collect()
}

/// The providers whose quotas `kinds` show, which are the only ones worth
/// asking.
pub(in crate::window) fn quota_providers(kinds: &[StatusSegmentKind]) -> BTreeSet<Provider> {
    kinds.iter().filter_map(|&kind| provider_of(kind)).collect()
}

/// The provider whose quota segment `kind` is, if it is one.
fn provider_of(kind: StatusSegmentKind) -> Option<Provider> {
    match kind {
        StatusSegmentKind::Antigravity => Some(Provider::Antigravity),
        StatusSegmentKind::Claude => Some(Provider::Claude),
        StatusSegmentKind::Editor | StatusSegmentKind::Extension(_) => None,
        StatusSegmentKind::Zai => Some(Provider::Zai),
    }
}

/// `provider`'s quota under its logo, once one has been heard.
fn quota_segment(sources: &SegmentSources, provider: Provider) -> Vec<StatusSegment> {
    let segment = sources
        .quotas
        .iter()
        .find(|(shown, _)| *shown == provider)
        .and_then(|(_, windows)| {
            Some(StatusSegment {
                icon: Some(crate::icons::named_svg(provider.icon_name())?),
                priority: 0,
                short: describe_compact(windows),
                text: describe(windows)?,
            })
        });
    segment.into_iter().collect()
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
        .map(|text| StatusSegment {
            icon: None,
            priority: 0,
            short: None,
            text: text.clone(),
        })
        .collect()
}

/// What the extension segment `id` was last told to say, if anything.
fn extension_segment(sources: &SegmentSources, id: &str) -> Vec<StatusSegment> {
    sources
        .extensions
        .get(id)
        .map(|text| StatusSegment {
            icon: None,
            priority: 0,
            short: None,
            text: text.clone(),
        })
        .into_iter()
        .collect()
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

    let project_name = if let Some(home) = crate::paths::home_dir() {
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
    use winter_render::NoticeKind;

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

    fn bar(mode: Option<Mode>, editor_mode: Option<EditMode>, cfg: &StatusBarConfig) -> StatusBar {
        status_bar(mode, editor_mode, &Theme::dark(), None, None, None, cfg)
    }

    #[test]
    fn test_status_bar_labels_each_mode() {
        let cfg = shown_mode();
        assert_eq!(bar(Some(Mode::Insert), None, &cfg).mode, " ▲");
        assert_eq!(bar(Some(Mode::Normal), None, &cfg).mode, " ●");
        assert_eq!(bar(Some(Mode::Visual), None, &cfg).mode, " ❖");
        assert_eq!(bar(None, Some(EditMode::Replace), &cfg).mode, " ↻");
        assert_ne!(
            bar(Some(Mode::Insert), None, &cfg).accent,
            bar(Some(Mode::Normal), None, &cfg).accent
        );
    }

    #[test]
    fn test_a_focused_editor_puts_its_own_mode_in_the_bar() {
        // An editor in Insert reads as a terminal in Insert does, and an
        // editor with no mode of its own shows no mode label.
        let cfg = shown_mode();
        let insert = bar(None, Some(EditMode::Insert), &cfg);
        assert_eq!(insert.mode, " ▲");
        assert_eq!(insert.accent, bar(Some(Mode::Insert), None, &cfg).accent);
        assert_eq!(bar(None, None, &cfg).mode, "");
    }

    #[test]
    fn test_status_bar_show_mode_toggle_hides_mode_label() {
        let shown = StatusBarConfig::default();
        let hidden = StatusBarConfig {
            show_mode: false,
            ..StatusBarConfig::default()
        };
        assert_eq!(bar(Some(Mode::Normal), None, &hidden).mode, "");
        assert_eq!(bar(Some(Mode::Normal), None, &shown).mode, " ●");
    }

    #[test]
    fn test_status_bar_prompt_suppresses_mode_label() {
        let prompt = StatusPrompt {
            caret: 3,
            label: "/".to_string(),
            value: "foo".to_string(),
        };
        let theme = Theme::dark();
        let cfg = shown_mode();
        let bar = status_bar(
            Some(Mode::Normal),
            None,
            &theme,
            Some(prompt),
            None,
            None,
            &cfg,
        );
        assert_eq!(bar.mode, "", "prompt suppresses mode label");
        assert!(bar.prompt.is_some());
    }

    #[test]
    fn test_status_bar_carries_the_search_and_the_notice_it_is_given() {
        let theme = Theme::dark();
        let cfg = StatusBarConfig::default();
        let search = StatusSearch {
            query: "foo".to_string(),
            match_index: 1,
            match_total: 3,
            reverse: false,
        };
        let notice = StatusNotice {
            kind: NoticeKind::Info,
            text: "oops".to_string(),
        };
        let bar = status_bar(
            Some(Mode::Normal),
            None,
            &theme,
            None,
            Some(search),
            Some(notice),
            &cfg,
        );
        assert!(bar.search.is_some());
        assert_eq!(bar.notice.map(|n| n.text), Some("oops".to_string()));
        assert!(
            status_bar(Some(Mode::Normal), None, &theme, None, None, None, &cfg)
                .search
                .is_none()
        );
    }

    #[test]
    fn test_segments_follow_the_configured_order_and_skip_what_has_nothing_to_say() {
        let windows = [Window {
            resets_at: None,
            used_percent: 42.0,
            weekly: false,
        }];
        let sources = SegmentSources {
            quotas: vec![(Provider::Zai, &windows[..])],
            editor: Some(FocusedEditor {
                status: EditorStatus {
                    mode: None,
                    segments: vec!["12:5".to_string(), "340 lines".to_string()],
                },
            }),
            extensions: &HashMap::from([("probe.said", "said".to_string())]),
        };
        let kinds = [
            StatusSegmentKind::Editor,
            StatusSegmentKind::Claude,
            StatusSegmentKind::Zai,
            StatusSegmentKind::Extension("probe.said"),
            StatusSegmentKind::Extension("probe.quiet"),
        ];
        let texts: Vec<String> = segments_of(&kinds, &[], &sources)
            .into_iter()
            .map(|segment| segment.text)
            .collect();
        assert_eq!(
            texts,
            ["12:5", "340 lines", "42%\u{2009}--", "said"],
            "the editor's facts apart and no icon, no Claude quota heard, \
             an extension's segment as told and one told nothing silent"
        );
        assert!(segments_of(&[], &[], &sources).is_empty(), "nothing named");
        let ranked = segments_of(&kinds, &[StatusSegmentKind::Editor], &sources);
        let ranks: Vec<u32> = ranked.iter().map(|segment| segment.priority).collect();
        assert_eq!(
            ranks,
            [1, 1, 0, 0],
            "the listed editor outranks the unlisted quota"
        );
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
