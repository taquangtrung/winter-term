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

/// How many percent a whole piece of a language server's work is.
const FULL_PERCENT: f32 = 100.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The notifier's tasks standing for Winter's long work.
#[derive(Debug, Default)]
pub(in crate::window) struct ProgressTasks {
    /// The task of each tab with a long program running, by tab.
    jobs: HashMap<TabId, ProgressId>,
    /// The language servers' task while one works, and the work it is.
    lsp: Option<(ProgressId, String)>,
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

    /// Bring the language servers' task in step with what the busiest says
    /// it is doing: its work as the title, what it is on as the step, and
    /// how far it has come. A new piece of work is a new task. Whether
    /// anything changed.
    fn sync_lsp_progress(&mut self, now: Instant) -> bool {
        let busy = self.host.lsp.busy();
        let running = self.progress.lsp.as_ref().and_then(|(task, title)| {
            let same = busy.as_ref().is_some_and(|busy| busy.work.title == *title);
            same.then_some(*task)
        });
        let ended = self
            .progress
            .lsp
            .as_ref()
            .filter(|_| running.is_none())
            .map(|(task, _)| *task);
        let mut changed = false;
        if let Some(task) = ended {
            self.progress.lsp = None;
            changed |= self.chrome.notifier.finish(task, None, now, 0);
        }
        let Some(busy) = busy else {
            return changed;
        };
        let task = match running {
            Some(task) => task,
            None => {
                let progress = Progress::new(busy.work.title.clone()).from_source(busy.server);
                let task = self.chrome.notifier.start(progress, now);
                self.progress.lsp = Some((task, busy.work.title.clone()));
                changed = true;
                task
            }
        };
        if self.task_step(task) != busy.work.message {
            self.chrome.notifier.set_step(task, busy.work.message);
            changed = true;
        }
        let fraction = busy
            .work
            .percentage
            .map(|percent| f32::from(percent) / FULL_PERCENT);
        if self.task_fraction(task) != fraction {
            self.chrome.notifier.set_fraction(task, fraction);
            changed = true;
        }
        changed
    }

    /// How far task `task` has come, as the notifier has it.
    fn task_fraction(&self, task: ProgressId) -> Option<f32> {
        self.chrome
            .notifier
            .tasks()
            .iter()
            .find(|running| running.id == task)
            .and_then(|running| running.fraction)
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
