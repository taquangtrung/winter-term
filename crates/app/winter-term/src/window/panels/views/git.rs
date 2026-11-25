//! The Git view, a sidebar's: the branch, then the files git reports,
//! under Unmerged, Untracked, Unstaged, and Staged. `s` stages the file the
//! cursor is on and `u` unstages it; opening a file shows it in the main
//! area, and `t` opens the Git tool there. In sight, the repository is
//! watched as the Git tool watches it, the same watch shared, and git is
//! asked again only when something in it changes; out of sight, git is
//! asked nothing.

use std::path::PathBuf;
use std::time::Instant;

use misty::view::{Space, column, text};
use misty::{Element, HostedView, ThemeToken};

use winter_dir::vcs::GitStatus;
use winter_model::background::Background;
use winter_model::editor::{EditorOutcome, JobReply, Notice, OpenTarget};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::paint::IconKind;
use winter_vcs::exec::{self, Git, Pathspec, TAG_ROOT, TAG_STATUS};
use winter_vcs::parse::{FileStatus, Section, Status, parse_status};
use winter_vcs::watch::TreeWatch;

use super::{ListAct, ListRow, PanelList, PanelView, RowIcon, list_view};
use crate::window::notice::WindowNotice;

// ========================================================================
// Constants
// ========================================================================

/// The Git view's own keys, block `panel_git`, ahead of the panel keys.
pub(super) const KEYS: KeyTable<GitAct> = KeyTable {
    block: "panel_git",
    bindings: &[
        Binding {
            action: GitAct::Stage,
            id: "stage",
            keys: &["s"],
            label: "Stage the File",
        },
        Binding {
            action: GitAct::Unstage,
            id: "unstage",
            keys: &["u"],
            label: "Unstage the File",
        },
    ],
    configurable: true,
    title: "Git View",
};

/// What the view is called.
const TITLE: &str = "Git";

/// What it says before git answers, outside a repository, and with nothing
/// changed.
const LOOKING: &str = "Asking git…";
const NO_REPOSITORY: &str = "Not in a git repository";
const CLEAN: &str = "Nothing changed";

/// What a detached head is called in place of a branch.
const DETACHED: &str = "(detached)";

/// The tags of the view's own requests, apart from the Git tool's.
const TAG_STAGE: &str = "panel-stage";
const TAG_UNSTAGE: &str = "panel-unstage";

/// What opens the Git tool in the main area.
const GIT_COMMAND: &str = "git_status";

// ========================================================================
// Data Structures
// ========================================================================

/// What a Git view key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum GitAct {
    Stage,
    Unstage,
}

/// The project's git status.
#[derive(Debug)]
pub(in crate::window) struct GitView {
    /// Whether the view is in sight, the only time it asks git or watches.
    shown: bool,
    list: PanelList,
    notice: Option<WindowNotice>,
    /// Whether a read of the status is out, so a change meanwhile waits for
    /// its answer.
    reading: bool,
    /// What git said, once it has.
    repo: Repo,
    /// Where the view started looking for a repository.
    start: PathBuf,
    /// The repository's notifications, `None` until set up, or where the
    /// platform cannot watch it: read again only when asked, then.
    watch: Option<TreeWatch>,
    /// The watch being set up on a thread of its own.
    watch_starting: Option<Background<Option<TreeWatch>>>,
}

/// What git has said of the repository.
#[derive(Debug)]
enum Repo {
    /// Nothing yet.
    Asking,
    /// The folder is in no repository.
    Missing,
    /// The repository at `root`, and its status once git gave it.
    Found(Found),
}

/// A repository found, and its status.
#[derive(Debug)]
struct Found {
    /// Where git keeps its own files, when git said.
    git_dir: Option<PathBuf>,
    root: PathBuf,
    status: Option<Status>,
}

// ========================================================================
// GitView Implementations
// ========================================================================

impl GitView {
    /// The status of the repository `start` is in, asked for once it comes
    /// into sight.
    pub(in crate::window) fn new(start: PathBuf) -> Self {
        Self {
            shown: false,
            list: PanelList::default(),
            notice: None,
            reading: false,
            repo: Repo::Asking,
            start,
            watch: None,
            watch_starting: None,
        }
    }

    /// Show the repository `start` is in instead: what that asks of the
    /// window while in sight.
    pub(in crate::window) fn set_start(&mut self, start: PathBuf) -> EditorOutcome {
        if start == self.start {
            return EditorOutcome::Consumed;
        }
        let shown = self.shown;
        *self = Self::new(start);
        self.set_shown(shown)
    }

    /// Ask git for what it is next owed: where the repository is, then its
    /// status.
    fn ask(&mut self) -> EditorOutcome {
        match &self.repo {
            Repo::Asking | Repo::Missing => EditorOutcome::Job(exec::repo_root(&self.start)),
            Repo::Found(found) => {
                self.reading = true;
                EditorOutcome::Job(exec::status(&found.root))
            }
        }
    }

    /// Set the repository's watch up on a thread of its own, while in sight
    /// and none is running.
    fn start_watch(&mut self) {
        let unwatched = self.watch.is_none() && self.watch_starting.is_none();
        if let (true, true, Repo::Found(found)) = (self.shown, unwatched, &self.repo) {
            self.watch_starting = Some(TreeWatch::start_in_background(
                &found.root,
                found.git_dir.as_deref(),
            ));
        }
    }

    /// Take the watch over once its thread has set it up.
    fn adopt_started_watch(&mut self) {
        if let Some(starting) = self.watch_starting.take_if(|starting| starting.is_done()) {
            self.watch = starting.poll().flatten();
        }
    }

    /// The file the cursor is on.
    fn selected(&self) -> Option<&FileStatus> {
        let Repo::Found(Found {
            status: Some(status),
            ..
        }) = &self.repo
        else {
            return None;
        };
        let row = self.list.selected_row().filter(|row| !row.heading)?;
        status.files.get(row.key)
    }

    /// Ask git to stage or unstage the file the cursor is on.
    fn change(&mut self, act: GitAct) -> EditorOutcome {
        let (Repo::Found(found), Some(file)) = (&self.repo, self.selected()) else {
            return EditorOutcome::Consumed;
        };
        let path = [Pathspec::new(file.path.clone())];
        let job = match act {
            GitAct::Stage => Git::new(["add"]).paths(&path).job(&found.root, TAG_STAGE),
            GitAct::Unstage => Git::new(["restore", "--staged"])
                .paths(&path)
                .job(&found.root, TAG_UNSTAGE),
        };
        EditorOutcome::Job(job)
    }

    /// List `status`: each section with files, under its title.
    fn list_status(&mut self, status: &Status) {
        let mut rows = Vec::new();
        for section in Section::all() {
            let files: Vec<(usize, &FileStatus)> = status
                .files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.section == section)
                .collect();
            if files.is_empty() {
                continue;
            }
            rows.push(ListRow {
                detail: Some(files.len().to_string()),
                heading: true,
                key: section as usize,
                label: section.title().to_string(),
                ..ListRow::default()
            });
            rows.extend(files.into_iter().map(|(key, file)| file_row(key, file)));
        }
        self.list.set_rows(rows);
    }
}

impl PanelView for GitView {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn list(&self) -> &PanelList {
        &self.list
    }

    fn list_mut(&mut self) -> &mut PanelList {
        &mut self.list
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match act {
            ListAct::Open => match (&self.repo, self.selected()) {
                (Repo::Found(found), Some(file)) => {
                    EditorOutcome::OpenPath(OpenTarget::file(found.root.join(&file.path)))
                }
                _ => EditorOutcome::Consumed,
            },
            ListAct::Refresh => self.ask(),
            ListAct::OpenTool => EditorOutcome::RunAction(GIT_COMMAND.to_string()),
            ListAct::Collapse
            | ListAct::Down
            | ListAct::Expand
            | ListAct::First
            | ListAct::Last
            | ListAct::Up => EditorOutcome::Consumed,
        }
    }

    fn own_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        let act = table::find(&KEYS, key)?;
        Some(self.change(act))
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        let JobReply::Command(output) = reply else {
            return EditorOutcome::Consumed;
        };
        match output.tag {
            TAG_ROOT => {
                let mut lines = output.stdout.lines();
                let root = lines.next().map(PathBuf::from);
                let git_dir = lines.next().map(PathBuf::from);
                match root.filter(|_| output.succeeded()) {
                    Some(root) => {
                        self.repo = Repo::Found(Found {
                            git_dir,
                            root,
                            status: None,
                        });
                        self.start_watch();
                        return self.ask();
                    }
                    None => {
                        self.repo = Repo::Missing;
                        self.list.set_rows(Vec::new());
                    }
                }
            }
            TAG_STATUS => {
                self.reading = false;
                if output.succeeded() {
                    let status = parse_status(&output.stdout);
                    self.list_status(&status);
                    if let Repo::Found(found) = &mut self.repo {
                        found.status = Some(status);
                    }
                }
            }
            TAG_STAGE | TAG_UNSTAGE => {
                if !output.succeeded() {
                    self.notice = Some(WindowNotice::StageFailed(output.stderr.trim().to_string()));
                }
                return self.ask();
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// Never out of sight; in sight, only when the watch has news, which
    /// wakes the window itself.
    fn next_tick(&self) -> Option<Instant> {
        let now = Instant::now();
        if !self.shown || self.reading {
            return None;
        }
        match (&self.watch, &self.watch_starting) {
            (Some(watch), _) => watch.next_look(now),
            // Taken over once its thread has answered, which wakes the window.
            (None, Some(starting)) => starting.is_done().then_some(now),
            (None, None) => None,
        }
    }

    /// In sight, the watch set up and what changed meanwhile read; out of
    /// sight, the watch let go.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        if !shown {
            self.watch = None;
            self.watch_starting = None;
            return EditorOutcome::Consumed;
        }
        self.start_watch();
        match self.reading {
            true => EditorOutcome::Consumed,
            false => self.ask(),
        }
    }

    fn on_tick(&mut self) -> EditorOutcome {
        self.adopt_started_watch();
        let changed = match (self.watch.as_mut(), self.reading) {
            (Some(watch), false) => watch.changed(Instant::now()).is_some(),
            (Some(_), true) | (None, _) => false,
        };
        match changed {
            true => self.ask(),
            false => EditorOutcome::Consumed,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(Notice::from)
    }
}

impl HostedView for GitView {
    fn view(&self) -> Element<'_, Self> {
        match &self.repo {
            Repo::Asking => list_view(self, LOOKING),
            Repo::Missing => list_view(self, NO_REPOSITORY),
            Repo::Found(found) => {
                let branch = found
                    .status
                    .as_ref()
                    .and_then(|status| status.branch.as_deref())
                    .unwrap_or(DETACHED);
                let head = Element::from(text(branch).emphasized().proportional())
                    .padding_px(8.0, 4.0)
                    .tooltip(format!("Branch of {}", found.root.display()));
                column([head, list_view(self, CLEAN)]).gap(Space::Px(2.0))
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `file`, the `key`th git reported: its name, its folder on
/// hover, and its code as a colored mark.
fn file_row(key: usize, file: &FileStatus) -> ListRow {
    let name = file
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&file.path)
        .to_string();
    ListRow {
        depth: 1,
        icon: Some(RowIcon::of_path(&name, None)),
        key,
        label: name,
        mark: Some(super::Mark {
            color: code_color(file.section),
            icon: Some(IconKind::Git {
                status: GitStatus::of_change(file.section, file.code)
                    .icon_stem()
                    .to_string(),
            }),
            meaning: code_meaning(file.code, file.section).to_string(),
            text: file.code.to_string(),
        }),
        tooltip: Some(file.path.clone()),
        ..ListRow::default()
    }
}

/// The color a file's code is in, by its section.
fn code_color(section: Section) -> ThemeToken {
    match section {
        Section::Staged => ThemeToken::Accent,
        Section::Unmerged => ThemeToken::Danger,
        Section::Unstaged => ThemeToken::Warning,
        Section::Untracked => ThemeToken::Muted,
    }
}

/// What a file's code says, in words.
fn code_meaning(code: char, section: Section) -> &'static str {
    match (section, code) {
        (Section::Unmerged, _) => "In conflict",
        (Section::Untracked, _) => "Not tracked by git",
        (Section::Staged, 'A') => "Added, staged",
        (Section::Staged, 'D') => "Deleted, staged",
        (Section::Staged, 'R') => "Renamed, staged",
        (Section::Staged, _) => "Modified, staged",
        (Section::Unstaged, 'D') => "Deleted, not staged",
        (Section::Unstaged, _) => "Modified, not staged",
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::editor::CommandOutput;

    use super::*;

    fn reply(tag: &'static str, stdout: &str) -> JobReply {
        JobReply::Command(CommandOutput {
            code: Some(0),
            id: 0,
            stderr: String::new(),
            stdout: stdout.to_string(),
            tag,
        })
    }

    #[test]
    fn test_git_is_asked_nothing_out_of_sight_and_nothing_on_a_timer_in_it() {
        let mut view = GitView::new(PathBuf::from("/repo"));
        assert_eq!(view.next_tick(), None, "out of sight, nothing");
        let asked = view.set_shown(true);
        assert!(matches!(asked, EditorOutcome::Job(_)), "in sight, it asks");
        view.on_job(reply(TAG_ROOT, "/nowhere-repo\n/nowhere-repo/.git\n"));
        view.watch_starting = None;
        view.on_job(reply(TAG_STATUS, "# branch.head main\n"));
        assert_eq!(view.next_tick(), None, "no timer: only a change asks again");
        view.set_shown(false);
        assert!(
            view.watch.is_none() && view.watch_starting.is_none(),
            "let go"
        );
    }

    #[test]
    fn test_files_are_listed_under_their_sections_and_s_stages_from_the_root() {
        let mut view = GitView::new(PathBuf::from("/repo/src"));
        view.shown = true;
        let asked = view.on_job(reply(TAG_ROOT, "/repo\n/repo/.git\n"));
        assert!(
            matches!(&asked, EditorOutcome::Job(_)),
            "the status asked for once the root is known"
        );
        view.on_job(reply(
            TAG_STATUS,
            "# branch.head main\n1 .M N... 100644 100644 100644 a b src/lib.rs\n? notes.txt\n",
        ));
        let rows: Vec<(bool, String)> = view
            .list
            .rows
            .iter()
            .map(|row| (row.heading, row.label.clone()))
            .collect();
        assert_eq!(
            rows,
            [
                (true, "Untracked files".to_string()),
                (false, "notes.txt".to_string()),
                (true, "Unstaged changes".to_string()),
                (false, "lib.rs".to_string()),
            ]
        );
        view.list_mut().move_cursor(ListAct::Down);
        let staged = view.change(GitAct::Stage);
        let EditorOutcome::Job(winter_model::editor::JobRequest::Command(request)) = staged else {
            panic!("a git job: {staged:?}");
        };
        assert_eq!(request.cwd, PathBuf::from("/repo"));
        assert!(request.args.ends_with(&[
            "add".to_string(),
            "--".to_string(),
            "src/lib.rs".to_string()
        ]));
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = GitView::new(PathBuf::from("/repo"));
        view.shown = true;
        view.on_job(reply(TAG_ROOT, "/repo\n/repo/.git\n"));
        view.on_job(reply(
            TAG_STATUS,
            "# branch.head main\n1 .M N... 100644 100644 100644 a b src/lib.rs\n? notes.txt\n",
        ));
        view.watch_starting = None;
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }

    #[test]
    fn test_each_file_s_state_is_an_icon_in_one_column_at_the_right() {
        let mut view = GitView::new(PathBuf::from("/repo"));
        view.shown = true;
        view.on_job(reply(TAG_ROOT, "/repo\n/repo/.git\n"));
        view.on_job(reply(
            TAG_STATUS,
            "# branch.head main\n1 .M N... 100644 100644 100644 a b src/lib.rs\n? a-much-longer-name.txt\n",
        ));
        view.watch_starting = None;
        let frame = super::super::tests::drawn_view(view);
        let modified = super::super::tests::lefts_of(&frame, "Modified, not staged");
        let untracked = super::super::tests::lefts_of(&frame, "Not tracked by git");
        assert_eq!(
            (modified.len(), untracked.len()),
            (1, 1),
            "a state icon each"
        );
        assert_eq!(modified[0], untracked[0], "whatever the name's length");
        let files = super::super::tests::lefts_of(&frame, "File");
        assert_eq!(files.len(), 2, "a file icon each");
        assert!(
            files[0] < modified[0],
            "the file's icon before, its state after"
        );
    }
}
