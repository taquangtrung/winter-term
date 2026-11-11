//! Long work under way as the notifier's progress tasks: a program an editor
//! runs long enough to mention, one task for each tab it runs for, and the
//! language servers' work. The status bar names the oldest; a click on it
//! shows them as cards, where Cancel stops a tab's programs as its stop key
//! does.

use std::collections::HashMap;
use std::time::Instant;

use misty::notify::Progress;
use misty::ProgressId;
use winter_model::layout::TabId;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// Who the language servers' task is said to come from.
const LSP_SOURCE: &str = "LSP";

/// What the language servers' task is doing.
const LSP_TITLE: &str = "Working";

// ========================================================================
// Data Structures
// ========================================================================

/// The notifier's tasks standing for Winter's long work.
#[derive(Debug, Default)]
pub(in crate::window) struct ProgressTasks {
    /// The task of each tab with a long program running, by tab.
    jobs: HashMap<TabId, ProgressId>,
    /// The language servers' task, while one works.
    lsp: Option<ProgressId>,
}

// ========================================================================
// App: progress
// ========================================================================

impl App {
    /// Bring the notifier's tasks in step with the long work under way:
    /// start a task for each tab whose program has run long enough, give it
    /// the program's latest step, and finish the tasks whose work ended.
    /// Whether anything changed.
    pub(in crate::window) fn sync_progress(&mut self) -> bool {
        let now = Instant::now();
        let long = self.host.jobs.long_jobs();
        let mut changed = false;
        let ended: Vec<TabId> = self
            .progress
            .jobs
            .keys()
            .filter(|tab| !long.iter().any(|(running, _)| running == *tab))
            .copied()
            .collect();
        for tab in ended {
            if let Some(task) = self.progress.jobs.remove(&tab) {
                changed |= self.chrome.notifier.finish(task, None, now, 0);
            }
        }
        for (tab, label) in long {
            let task = match self.progress.jobs.get(&tab) {
                Some(task) => *task,
                None => {
                    let source = self.tab_full_name(tab);
                    let progress = Progress::new(label).from_source(source).cancellable();
                    let task = self.chrome.notifier.start(progress, now);
                    self.progress.jobs.insert(tab, task);
                    changed = true;
                    task
                }
            };
            let step = self.host.jobs.progress_for(tab);
            let before = self.task_step(task);
            if before != step {
                self.chrome.notifier.set_step(task, step);
                changed = true;
            }
        }
        changed | self.sync_lsp_progress(now)
    }

    /// Bring the language servers' task in step with what they say they
    /// are doing. Whether anything changed.
    fn sync_lsp_progress(&mut self, now: Instant) -> bool {
        let busy = self.host.lsp.busy();
        match (busy, self.progress.lsp) {
            (Some(step), Some(task)) => {
                let changed = self.task_step(task).as_deref() != Some(step.as_str());
                if changed {
                    self.chrome.notifier.set_step(task, Some(step));
                }
                changed
            }
            (Some(step), None) => {
                let progress = Progress::new(LSP_TITLE).from_source(LSP_SOURCE);
                let task = self.chrome.notifier.start(progress, now);
                self.chrome.notifier.set_step(task, Some(step));
                self.progress.lsp = Some(task);
                true
            }
            (None, Some(task)) => {
                self.progress.lsp = None;
                self.chrome.notifier.finish(task, None, now, 0)
            }
            (None, None) => false,
        }
    }

    /// The step task `task` is on, as the notifier has it.
    fn task_step(&self, task: ProgressId) -> Option<String> {
        self.chrome
            .notifier
            .tasks()
            .iter()
            .find(|running| running.id == task)
            .and_then(|running| running.step.clone())
    }

    /// Cancel was pressed on task `task`'s card: stop the programs of the
    /// tab it stands for.
    pub(in crate::window) fn cancel_task(&mut self, task: ProgressId) {
        let tab = self
            .progress
            .jobs
            .iter()
            .find(|(_, running)| **running == task)
            .map(|(tab, _)| *tab);
        if let Some(tab) = tab {
            self.stop_tasks(tab);
            self.dirty = true;
        }
    }
}
