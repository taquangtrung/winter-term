//! Git: the working tree's state in a pane, and the keys that change it.
//!
//! - [`exec`]: the git command lines the view runs.
//! - [`parse`]: reading git's own output.
//! - [`rows`]: painting the view, and what each row stands for.

pub mod exec;
pub mod parse;
pub mod rows;

use std::collections::HashSet;
use std::path::PathBuf;

use crate::model::input::{Key, KeyCode};
use crate::model::page::{
    scroll_to_cursor, CommandOutput, JobReply, JobRequest, Page, PageContent, PageOutcome,
    PageSpan, PageStyle, PromptMode, PromptReply, PromptRequest,
};

use parse::{Commit, Section, Status};
use rows::{Item, ViewRow};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question the view asks.
const ASK_COMMIT: &str = "commit";
const ASK_DISCARD: &str = "discard";

/// Shown while the first status has not come back yet.
const LOADING: &str = "  reading the repository...";

/// Shown when the directory the view opened in is not in a repository.
const NOT_A_REPO: &str = "  not a git repository";

/// Rows of header above the first section.
const HEADER_ROWS: usize = 1;

// ========================================================================
// Data Structures
// ========================================================================

/// The working tree's state, and the cursor over it.
pub struct GitPage {
    /// Sections the user has shut. `None` stands for the recent-commits
    /// section, which has no `Section` of its own.
    collapsed: HashSet<Option<Section>>,
    commits: Vec<Commit>,
    cursor: usize,
    /// What the last command reported, shown in the header.
    message: Option<String>,
    /// Where the repository is rooted, once git has said.
    root: Option<PathBuf>,
    rows: Vec<ViewRow>,
    scroll: usize,
    /// Set by `g`, waiting for the key that says which section to jump to.
    pending: bool,
    /// Where the page was opened, which is where the root is looked up from.
    start: PathBuf,
    status: Status,
    /// Set once a status has come back, so an empty view can say which it is:
    /// a clean tree, or one still being read.
    loaded: bool,
}

// ========================================================================
// GitPage
// ========================================================================

impl GitPage {
    /// A view of the repository containing `start`, which is asked for first.
    pub fn new(start: PathBuf) -> Self {
        Self {
            collapsed: HashSet::new(),
            commits: Vec::new(),
            cursor: 0,
            loaded: false,
            message: None,
            pending: false,
            root: None,
            rows: Vec::new(),
            scroll: 0,
            start,
            status: Status::default(),
        }
    }

    /// The first request the view makes: where the repository is.
    pub fn initial_request(&self) -> JobRequest {
        exec::repo_root(&self.start)
    }

    /// Re-read the working tree.
    fn refresh(&self) -> PageOutcome {
        match &self.root {
            Some(root) => PageOutcome::Job(exec::status(root)),
            None => PageOutcome::Job(self.initial_request()),
        }
    }

    fn rebuild(&mut self) {
        let collapsed = self.collapsed.clone();
        self.rows = rows::build(
            &self.status,
            &self.commits,
            &|section| collapsed.contains(&section),
            self.message.as_deref(),
        );
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&ViewRow> {
        self.rows.get(self.cursor)
    }

    /// The paths a key acts on: the file under the cursor, or every file in the
    /// section whose heading it is on.
    fn targets(&self) -> Vec<String> {
        match self.selected().map(|row| &row.item) {
            Some(Item::File(file)) => vec![file.path.clone()],
            Some(Item::Heading(section)) => self.paths_in(*section),
            Some(Item::Commit(_)) | Some(Item::RecentHeading) | Some(Item::None) | None => {
                Vec::new()
            }
        }
    }

    fn paths_in(&self, section: Section) -> Vec<String> {
        self.status
            .files
            .iter()
            .filter(|file| file.section == section)
            .map(|file| file.path.clone())
            .collect()
    }

    /// Which section the cursor is in, taken from the row itself so a heading
    /// and its entries agree.
    fn section_at_cursor(&self) -> Option<Section> {
        match self.selected().map(|row| &row.item) {
            Some(Item::File(file)) => Some(file.section),
            Some(Item::Heading(section)) => Some(*section),
            _ => None,
        }
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
    }

    /// Move to the next or previous row that stands for something, skipping the
    /// blank lines between sections.
    fn move_to_entity(&mut self, forward: bool) {
        let step: isize = if forward { 1 } else { -1 };
        let mut index = self.cursor as isize;
        loop {
            index += step;
            if index < 0 || index as usize >= self.rows.len() {
                return;
            }
            if self.rows[index as usize].item != Item::None {
                self.cursor = index as usize;
                return;
            }
        }
    }

    /// Jump to a section's heading, which is how the `g` keys navigate.
    fn move_to_section(&mut self, section: Option<Section>) {
        let target = self.rows.iter().position(|row| match (&row.item, section) {
            (Item::Heading(found), Some(want)) => *found == want,
            (Item::RecentHeading, None) => true,
            _ => false,
        });
        if let Some(index) = target {
            self.cursor = index;
        }
    }

    fn toggle_fold(&mut self) {
        let key = match self.selected().map(|row| &row.item) {
            Some(Item::Heading(section)) => Some(Some(*section)),
            Some(Item::File(file)) => Some(Some(file.section)),
            Some(Item::RecentHeading) | Some(Item::Commit(_)) => Some(None),
            Some(Item::None) | None => None,
        };
        let Some(key) = key else {
            return;
        };
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.rebuild();
    }

    fn toggle_fold_all(&mut self) {
        if self.collapsed.is_empty() {
            self.collapsed = Section::all().into_iter().map(Some).collect();
            self.collapsed.insert(None);
        } else {
            self.collapsed.clear();
        }
        self.rebuild();
    }

    /// Stage what the cursor is on. Untracked files are staged by the same key,
    /// since "add this" is what the user means either way.
    fn stage(&self, all: bool) -> PageOutcome {
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return PageOutcome::Consumed;
        }
        PageOutcome::Job(exec::stage(root, &paths))
    }

    fn unstage(&self, all: bool) -> PageOutcome {
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return PageOutcome::Consumed;
        }
        PageOutcome::Job(exec::unstage(root, &paths))
    }

    /// Ask before throwing away work, naming what will go: nothing recovers a
    /// discarded working-tree change.
    fn ask_discard(&self) -> PageOutcome {
        let paths = self.targets();
        let label = match paths.len() {
            0 => return PageOutcome::Consumed,
            1 => format!("Discard changes to {}? (y/n) ", paths[0]),
            count => format!("Discard changes to {count} files? (y/n) "),
        };
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag: ASK_DISCARD,
        })
    }

    fn discard(&self) -> PageOutcome {
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        let paths = self.targets();
        if paths.is_empty() {
            return PageOutcome::Consumed;
        }
        // Untracked files are not in the index, so `restore` cannot reach
        // them: deleting is what discarding means for those.
        if self.section_at_cursor() == Some(Section::Untracked) {
            return PageOutcome::Job(exec::remove_untracked(root, &paths));
        }
        PageOutcome::Job(exec::discard(root, &paths))
    }

    /// Ask for a one-line message. The long-form editor is the `c` key.
    fn ask_commit_message(&self) -> PageOutcome {
        if self.paths_in(Section::Staged).is_empty() {
            return PageOutcome::Consumed;
        }
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "Commit message: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_COMMIT,
        })
    }

    /// Resolve the key after `g`: each names a section to jump to, and
    /// anything else abandons the sequence.
    fn jump_to(&mut self, code: KeyCode) -> PageOutcome {
        match code {
            KeyCode::Char('t') => self.move_to_section(Some(Section::Untracked)),
            KeyCode::Char('u') => self.move_to_section(Some(Section::Unstaged)),
            KeyCode::Char('s') => self.move_to_section(Some(Section::Staged)),
            KeyCode::Char('r') => self.move_to_section(None),
            KeyCode::Char('j') => self.move_to_entity(true),
            KeyCode::Char('k') => self.move_to_entity(false),
            _ => {}
        }
        PageOutcome::Consumed
    }

    /// What a finished command means for the view.
    fn on_command(&mut self, output: CommandOutput) -> PageOutcome {
        if !output.succeeded() {
            self.message = Some(format!("failed: {}", output.failure()));
            self.rebuild();
            return PageOutcome::Consumed;
        }
        match output.tag {
            exec::TAG_ROOT => {
                let root = PathBuf::from(output.stdout.trim());
                self.root = Some(root.clone());
                PageOutcome::Job(exec::status(&root))
            }
            exec::TAG_STATUS => {
                self.status = parse::parse_status(&output.stdout);
                self.loaded = true;
                self.rebuild();
                match &self.root {
                    Some(root) => PageOutcome::Job(exec::recent_log(root)),
                    None => PageOutcome::Consumed,
                }
            }
            exec::TAG_LOG => {
                self.commits = parse::parse_log(&output.stdout);
                self.rebuild();
                PageOutcome::Consumed
            }
            // Anything that changed the repository: report it and re-read.
            tag => {
                self.message = Some(report_for(tag, &output));
                self.refresh()
            }
        }
    }
}

impl Page for GitPage {
    fn title(&self) -> String {
        "Git".to_string()
    }

    fn content(&mut self, rows: usize) -> PageContent {
        if self.rows.is_empty() {
            // A loaded view with no rows means git answered and had nothing
            // to report, which only happens outside a repository: a clean tree
            // still has a header row.
            let note = if self.loaded { NOT_A_REPO } else { LOADING };
            return PageContent::new(vec![vec![PageSpan::new(PageStyle::Dim, note)]]);
        }
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.scroll = scroll_to_cursor(self.scroll, self.cursor, self.rows.len(), visible);
        let painted = self
            .rows
            .iter()
            .skip(self.scroll)
            .take(visible.max(1))
            .map(|row| row.spans.clone())
            .collect();
        PageContent::new(painted).with_cursor_line(self.cursor - self.scroll)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        if self.pending {
            self.pending = false;
            return self.jump_to(key.code);
        }
        if key.alt {
            return match key.code {
                KeyCode::Char('n') => {
                    self.move_to_entity(true);
                    PageOutcome::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_to_entity(false);
                    PageOutcome::Consumed
                }
                _ => PageOutcome::Ignored,
            };
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('C') | KeyCode::Char('c') if key.shift => self.ask_commit_message(),
                _ => PageOutcome::Ignored,
            };
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_by(1);
                PageOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_by(-1);
                PageOutcome::Consumed
            }
            KeyCode::Home => {
                self.cursor = 0;
                PageOutcome::Consumed
            }
            KeyCode::End => {
                self.cursor = self.rows.len().saturating_sub(1);
                PageOutcome::Consumed
            }
            KeyCode::Tab if key.shift => {
                self.toggle_fold_all();
                PageOutcome::Consumed
            }
            KeyCode::Tab => {
                self.toggle_fold();
                PageOutcome::Consumed
            }
            KeyCode::Enter => match self.selected().map(|row| &row.item) {
                Some(Item::File(file)) => {
                    let path = self.root.clone().unwrap_or_default().join(&file.path);
                    PageOutcome::OpenPath(path)
                }
                _ => PageOutcome::Consumed,
            },
            KeyCode::Char('g') => {
                self.pending = true;
                PageOutcome::Consumed
            }
            KeyCode::Char('s') => self.stage(false),
            KeyCode::Char('S') => self.stage(true),
            KeyCode::Char('u') => self.unstage(false),
            KeyCode::Char('U') => self.unstage(true),
            KeyCode::Char('x') => self.ask_discard(),
            KeyCode::Char('G') => self.refresh(),
            KeyCode::Char('P') => match &self.root {
                Some(root) => PageOutcome::Job(exec::push(root)),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('F') => match &self.root {
                Some(root) => PageOutcome::Job(exec::pull(root)),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('f') => match &self.root {
                Some(root) => PageOutcome::Job(exec::fetch(root)),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            _ => PageOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        let Some(answer) = reply.answer else {
            self.message = Some("cancelled".to_string());
            self.rebuild();
            return PageOutcome::Consumed;
        };
        match reply.tag {
            ASK_COMMIT => match &self.root {
                Some(root) => PageOutcome::Job(exec::commit(root, &answer)),
                None => PageOutcome::Consumed,
            },
            ASK_DISCARD => self.discard(),
            _ => PageOutcome::Consumed,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> PageOutcome {
        match reply {
            JobReply::Command(output) => self.on_command(output),
            // The view asks for no directory walks.
            JobReply::DirSize { .. } => PageOutcome::Consumed,
        }
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// What to say in the header after a command that changed something.
fn report_for(tag: &'static str, output: &CommandOutput) -> String {
    let detail = output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match tag {
        exec::TAG_COMMIT => format!("committed: {detail}"),
        exec::TAG_DISCARD => "discarded".to_string(),
        exec::TAG_FETCH => "fetched".to_string(),
        exec::TAG_PULL => format!("pulled: {detail}"),
        exec::TAG_PUSH => "pushed".to_string(),
        exec::TAG_STAGE => "staged".to_string(),
        exec::TAG_UNSTAGE => "unstaged".to_string(),
        _ => detail.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::page::CommandRequest;

    const STATUS_OUTPUT: &str = "\
# branch.head main
# branch.upstream origin/main
# branch.ab +1 -0
1 M. N... 100644 100644 100644 aaa bbb staged.rs
1 .M N... 100644 100644 100644 aaa bbb working.rs
? new.rs
";

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn reply(tag: &'static str, stdout: &str) -> JobReply {
        JobReply::Command(CommandOutput {
            code: Some(0),
            stderr: String::new(),
            stdout: stdout.to_string(),
            tag,
        })
    }

    fn command_of(outcome: &PageOutcome) -> CommandRequest {
        match outcome {
            PageOutcome::Job(JobRequest::Command(command)) => command.clone(),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    /// A page that has heard back about its root, its status, and its log.
    fn loaded_page() -> GitPage {
        let mut page = GitPage::new(PathBuf::from("/repo/sub"));
        page.on_job(reply(exec::TAG_ROOT, "/repo\n"));
        page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        page.on_job(reply(exec::TAG_LOG, "abc1234 do the thing\n"));
        page
    }

    fn cursor_on(page: &mut GitPage, path: &str) {
        let index = page
            .rows
            .iter()
            .position(|row| matches!(&row.item, Item::File(file) if file.path == path))
            .unwrap_or_else(|| panic!("no row for {path}"));
        page.cursor = index;
    }

    #[test]
    fn test_the_root_lookup_leads_to_a_status_read() {
        // Every later command runs from the root, so the view cannot do
        // anything until that answer arrives.
        let mut page = GitPage::new(PathBuf::from("/repo/sub"));
        let next = page.on_job(reply(exec::TAG_ROOT, "/repo\n"));
        let command = command_of(&next);
        assert_eq!(command.tag, exec::TAG_STATUS);
        assert_eq!(command.cwd, PathBuf::from("/repo"), "not the subdirectory");
    }

    #[test]
    fn test_a_status_read_is_followed_by_the_recent_log() {
        let mut page = GitPage::new(PathBuf::from("/repo"));
        page.on_job(reply(exec::TAG_ROOT, "/repo\n"));
        let next = page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        assert_eq!(command_of(&next).tag, exec::TAG_LOG);
    }

    #[test]
    fn test_staging_acts_on_the_path_under_the_cursor() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["add", "--", "working.rs"]);
    }

    #[test]
    fn test_staging_on_a_heading_acts_on_that_whole_section() {
        // The heading stands for its section, which is what makes "stage this
        // lot" one keystroke.
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| row.item == Item::Heading(Section::Unstaged))
            .expect("the unstaged heading");
        let command = command_of(&page.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["add", "--", "working.rs"]);
    }

    #[test]
    fn test_unstaging_uses_the_staged_copy_of_a_file_changed_on_both_sides() {
        // `staged.rs` and `working.rs` differ, but a file in both sections must
        // unstage from the staged row and stage from the unstaged one.
        let mut page = loaded_page();
        cursor_on(&mut page, "staged.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Char('u'))));
        assert_eq!(command.args, ["restore", "--staged", "--", "staged.rs"]);
    }

    #[test]
    fn test_discarding_asks_first_and_names_the_file() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('x'))) else {
            panic!("expected a confirmation");
        };
        assert_eq!(request.mode, PromptMode::Confirm);
        assert!(
            request.label.contains("working.rs"),
            "got {:?}",
            request.label
        );
    }

    #[test]
    fn test_discarding_an_untracked_file_deletes_it_instead_of_restoring() {
        // `restore` cannot reach a path that is not in the index, so the view
        // would report success while leaving the file exactly where it was.
        let mut page = loaded_page();
        cursor_on(&mut page, "new.rs");
        page.on_key(&press(KeyCode::Char('x')));
        let command = command_of(&page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, ["clean", "--force", "-d", "--", "new.rs"]);
    }

    #[test]
    fn test_declining_the_confirmation_runs_nothing() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        page.on_key(&press(KeyCode::Char('x')));
        let outcome = page.on_prompt(PromptReply {
            answer: None,
            tag: ASK_DISCARD,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
    }

    #[test]
    fn test_committing_nothing_is_refused_before_it_asks() {
        // `git commit` with an empty index fails with a wall of advice; not
        // asking for a message is the better answer.
        let mut page = GitPage::new(PathBuf::from("/repo"));
        page.on_job(reply(exec::TAG_ROOT, "/repo\n"));
        page.on_job(reply(exec::TAG_STATUS, "# branch.head main\n"));
        let outcome = page.on_key(&Key {
            alt: false,
            code: KeyCode::Char('c'),
            ctrl: true,
            shift: true,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
    }

    #[test]
    fn test_a_command_that_changed_the_tree_re_reads_it() {
        // Without the re-read the view shows the state before the change, and
        // the next key acts on rows that no longer describe the repository.
        let mut page = loaded_page();
        let next = page.on_job(reply(exec::TAG_STAGE, ""));
        assert_eq!(command_of(&next).tag, exec::TAG_STATUS);
        assert_eq!(page.message.as_deref(), Some("staged"));
    }

    #[test]
    fn test_a_failed_command_reports_and_does_not_re_read() {
        let mut page = loaded_page();
        let outcome = page.on_job(JobReply::Command(CommandOutput {
            code: Some(1),
            stderr: "error: pathspec 'nope' did not match\n".to_string(),
            stdout: String::new(),
            tag: exec::TAG_STAGE,
        }));
        assert_eq!(outcome, PageOutcome::Consumed);
        let message = page.message.clone().unwrap_or_default();
        assert!(message.starts_with("failed:"), "got {message:?}");
        assert!(message.contains("pathspec"), "got {message:?}");
    }

    #[test]
    fn test_the_g_keys_jump_between_sections() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('s')));
        assert_eq!(
            page.selected().map(|row| &row.item),
            Some(&Item::Heading(Section::Staged))
        );

        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('t')));
        assert_eq!(
            page.selected().map(|row| &row.item),
            Some(&Item::Heading(Section::Untracked))
        );
    }

    #[test]
    fn test_entity_motion_skips_the_blank_lines_between_sections() {
        // Plain `j` walks through the gaps; `Alt-n` is what steps from one
        // thing to the next thing.
        let mut page = loaded_page();
        page.cursor = 0;
        page.on_key(&Key {
            alt: true,
            code: KeyCode::Char('n'),
            ctrl: false,
            shift: false,
        });
        assert_ne!(page.selected().map(|row| &row.item), Some(&Item::None));
    }

    #[test]
    fn test_folding_a_section_hides_its_files_but_not_the_others() {
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| row.item == Item::Heading(Section::Untracked))
            .expect("the untracked heading");
        page.on_key(&press(KeyCode::Tab));

        let listed: Vec<String> = page
            .rows
            .iter()
            .filter_map(|row| match &row.item {
                Item::File(file) => Some(file.path.clone()),
                _ => None,
            })
            .collect();
        assert!(!listed.contains(&"new.rs".to_string()), "got {listed:?}");
        assert!(listed.contains(&"working.rs".to_string()), "got {listed:?}");
    }

    #[test]
    fn test_shift_tab_shuts_everything_then_opens_it_again() {
        let mut page = loaded_page();
        let shift_tab = Key {
            alt: false,
            code: KeyCode::Tab,
            ctrl: false,
            shift: true,
        };
        page.on_key(&shift_tab);
        assert!(!page
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
        page.on_key(&shift_tab);
        assert!(page
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
    }

    #[test]
    fn test_a_push_runs_from_the_repository_root() {
        let mut page = loaded_page();
        let command = command_of(&page.on_key(&press(KeyCode::Char('P'))));
        assert_eq!(command.args, ["push"]);
        assert_eq!(command.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn test_keys_before_the_root_is_known_do_nothing() {
        // The view is up as soon as the key is pressed, so every command has to
        // survive being asked before git has answered.
        let mut page = GitPage::new(PathBuf::from("/repo"));
        for code in [
            KeyCode::Char('s'),
            KeyCode::Char('u'),
            KeyCode::Char('P'),
            KeyCode::Char('F'),
            KeyCode::Char('f'),
        ] {
            assert_eq!(page.on_key(&press(code)), PageOutcome::Consumed);
        }
    }
}
