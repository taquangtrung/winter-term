//! The Todo view, a sidebar's: the project's TODO comments, under the file
//! each is in, every file open at first. Opening a comment shows its line
//! in the main area; `t` opens the Todo page there. It looks only in sight,
//! once each time it comes into sight.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use misty::{Element, HostedView};

use winter_model::editor::{EditorOutcome, JobReply, JobRequest, OpenTarget, SearchRequest};

use super::{ListAct, ListRow, Mark, PanelList, PanelView, list_view};
use crate::editors::todo::{Todo, todo_of, tone};

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Todo";

/// What it says while it looks, and with nothing found.
const SEARCHING: &str = "Looking for comments…";
const NOTHING_FOUND: &str = "Nothing to do";

/// What opens the Todo page in the main area.
const TODO_COMMAND: &str = "todo";

// ========================================================================
// Data Structures
// ========================================================================

/// The project's TODO comments.
#[derive(Debug)]
pub(in crate::window) struct TodoView {
    /// The files closed, by path.
    closed: BTreeSet<PathBuf>,
    /// The comments found, a file at a time, in line order.
    found: Vec<Todo>,
    /// Whether the view is in sight, the only time it looks.
    shown: bool,
    /// The words searched for.
    keywords: Vec<String>,
    list: PanelList,
    /// The project searched.
    root: PathBuf,
    /// Whether a search is out.
    searching: bool,
}

// ========================================================================
// TodoView Implementations
// ========================================================================

impl TodoView {
    /// The comments of the project at `root` holding one of `keywords`,
    /// looked for once it comes into sight.
    pub(in crate::window) fn new(root: PathBuf, keywords: &[String]) -> Self {
        Self {
            closed: BTreeSet::new(),
            found: Vec::new(),
            shown: false,
            keywords: keywords.to_vec(),
            list: PanelList::default(),
            root,
            searching: false,
        }
    }

    /// Show the project at `root` instead: what that asks of the window
    /// while in sight.
    pub(in crate::window) fn set_root(&mut self, root: PathBuf) -> EditorOutcome {
        if root == self.root {
            return EditorOutcome::Consumed;
        }
        self.root = root;
        self.found.clear();
        self.closed.clear();
        self.list.set_rows(Vec::new());
        match self.shown {
            true => self.search(),
            false => EditorOutcome::Consumed,
        }
    }

    /// Ask the window to look for the comments, unless it is looking now.
    fn search(&mut self) -> EditorOutcome {
        if self.searching || self.keywords.is_empty() {
            return EditorOutcome::Consumed;
        }
        self.searching = true;
        EditorOutcome::Job(JobRequest::Search(SearchRequest {
            root: self.root.clone(),
            words: self.keywords.clone(),
        }))
    }

    /// List what was found: each file, its comments under it while it is
    /// open.
    fn list_found(&mut self) {
        let mut rows = Vec::new();
        let mut file: Option<&Path> = None;
        for (key, todo) in self.found.iter().enumerate() {
            let open = !self.closed.contains(&todo.path);
            if file != Some(todo.path.as_path()) {
                file = Some(todo.path.as_path());
                let held = self
                    .found
                    .iter()
                    .filter(|other| other.path == todo.path)
                    .count();
                rows.push(ListRow {
                    detail: Some(held.to_string()),
                    fold: Some(open),
                    heading: true,
                    key,
                    label: file_name(&todo.path),
                    tooltip: Some(relative(&self.root, &todo.path)),
                    ..ListRow::default()
                });
            }
            if open {
                rows.push(self.todo_row(key, todo));
            }
        }
        self.list.set_rows(rows);
    }

    /// The row of `todo`, the `key`th found.
    fn todo_row(&self, key: usize, todo: &Todo) -> ListRow {
        let keyword = &self.keywords[todo.keyword];
        ListRow {
            depth: 1,
            detail: Some(todo.line.to_string()),
            key,
            label: todo.words.clone(),
            mark: Some(Mark {
                color: tone(keyword),
                meaning: format!("A {keyword} comment"),
                text: keyword.clone(),
            }),
            tooltip: Some(format!(
                "Line {} of {}",
                todo.line,
                relative(&self.root, &todo.path)
            )),
            ..ListRow::default()
        }
    }

    /// Open or close the file the cursor is on, or the file of the comment
    /// it is on.
    fn fold(&mut self, open: bool) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let (key, heading) = (row.key, row.heading);
        let Some(path) = self.found.get(key).map(|todo| todo.path.clone()) else {
            return;
        };
        match open {
            true => self.closed.remove(&path),
            false => self.closed.insert(path.clone()),
        };
        self.list_found();
        let first = self
            .found
            .iter()
            .position(|todo| todo.path == path)
            .unwrap_or(key);
        match (open, heading) {
            (false, _) => self.list.select_key(first, true),
            (true, _) => self.list.select_key(key, heading),
        }
    }
}

impl PanelView for TodoView {
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
            ListAct::Open => match self.list.selected_row() {
                Some(row) if row.heading => {
                    let open = row.fold == Some(false);
                    self.fold(open);
                }
                Some(row) => {
                    if let Some(todo) = self.found.get(row.key) {
                        return EditorOutcome::OpenPath(OpenTarget::at_line(
                            todo.path.clone(),
                            todo.line,
                        ));
                    }
                }
                None => {}
            },
            ListAct::Expand => self.fold(true),
            ListAct::Collapse => self.fold(false),
            ListAct::Refresh => return self.search(),
            ListAct::OpenTool => return EditorOutcome::RunAction(TODO_COMMAND.to_string()),
            ListAct::Down | ListAct::First | ListAct::Last | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        if let JobReply::Search(found) = reply {
            self.searching = false;
            self.found = found
                .hits
                .iter()
                .filter_map(|hit| todo_of(hit, &self.keywords))
                .collect();
            self.list_found();
        }
        EditorOutcome::Consumed
    }

    /// Looks once each time it comes into sight.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        match shown {
            true => self.search(),
            false => EditorOutcome::Consumed,
        }
    }
}

impl HostedView for TodoView {
    fn view(&self) -> Element<'_, Self> {
        let empty = match self.searching {
            true => SEARCHING,
            false => NOTHING_FOUND,
        };
        list_view(self, empty)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The last part of `path`.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// `path` from `root`, or whole where it is outside.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::editor::{SearchHit, SearchResult};

    use super::*;

    fn hit(path: &str, line: usize, text: &str) -> SearchHit {
        SearchHit {
            line,
            path: PathBuf::from("/repo").join(path),
            text: text.to_string(),
        }
    }

    #[test]
    fn test_comments_sit_under_their_file_which_closes_and_opens() {
        let mut view = TodoView::new(PathBuf::from("/repo"), &["TODO".to_string()]);
        view.on_job(JobReply::Search(SearchResult {
            hits: vec![
                hit("a.rs", 3, "// TODO: one"),
                hit("a.rs", 9, "// TODO: two"),
                hit("b.rs", 1, "// TODO: three"),
            ],
        }));
        let labels = |view: &TodoView| -> Vec<String> {
            view.list.rows.iter().map(|row| row.label.clone()).collect()
        };
        assert_eq!(labels(&view), ["a.rs", "one", "two", "b.rs", "three"]);
        view.act(ListAct::Collapse);
        assert_eq!(labels(&view), ["a.rs", "b.rs", "three"], "a.rs closed");
        assert_eq!(
            view.list.selected_row().map(|row| row.label.as_str()),
            Some("a.rs")
        );
        let opened = view.act(ListAct::Open);
        assert!(matches!(opened, EditorOutcome::Consumed));
        assert_eq!(labels(&view).len(), 5, "opened again");
    }
}
