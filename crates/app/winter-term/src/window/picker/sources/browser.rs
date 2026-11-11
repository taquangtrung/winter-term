//! The file browser: one directory at a time, its entries filtered by what
//! is typed. `Tab` or `→` steps into the selected directory, `←` or `C-h`
//! out of it, and a `/` typed after a path goes there (`/` alone to the root,
//! `~/` home). `Enter` opens a file, or lists a directory in a Dir editor:
//! on `./`, the directory being browsed. A query that names no entry is
//! offered as a new file after the matches, made empty when chosen.
//!
//! A browser opened to choose (see [`Chooser`]) hands back the file chosen
//! in place of opening it: `Enter` on a directory steps into it.
//!
//! A remote place (a host over SSH, a container) is browsed the same way,
//! by its path; its directories are read on a thread of their own, so a slow
//! host never holds the window.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use misty::picker::{
    Action, ActionId, Batch, Feed, MatchOn, Matching, Resume, Row, RowKey, Source, SourceKey,
};
use winter_base::remote::{self, Kind, Place, Remote, RemoteError};
use winter_base::wake::wake_window;
use winter_model::editor::OpenTarget;
use winter_model::input::table::{find, Binding, KeyTable};
use winter_model::paint::IconKind;
use winter_model::path::{abbreviate_home, clean_path, expand_home};

use crate::window::input::misty::winter_key;
use crate::window::picker::sources::IconRow;
use crate::window::picker::Preview;
use crate::window::picker::WindowOutcome;

// ========================================================================
// Constants
// ========================================================================

/// The row that opens the directory browsed, and what it says at its right.
const CURRENT_LABEL: &str = "./";
const CURRENT_HINT: &str = "open here";

/// The row that leads up, and what it says at its right.
const PARENT_LABEL: &str = "../";
const PARENT_HINT: &str = "up";

/// The one action of a row.
const OPEN: ActionId = ActionId("open");

/// The row offering the query as a new file, its action, and what it says at
/// its right.
const NEW_FILE_ROW: RowKey = RowKey(u64::MAX);
const CREATE: ActionId = ActionId("create");
const NEW_FILE_HINT: &str = "new file";

/// Said when a remote file cannot be made before `sudo` has its password.
const NEEDS_PASSWORD: &str = "sudo needs its password before the file can be made";

/// The reading thread's name.
const THREAD_NAME: &str = "picker-browser";

/// Said when a directory cannot be read.
const UNREADABLE: &str = "this directory cannot be read";

/// The browser's own keys, block `picker.browser`.
pub(in crate::window) const KEYS: KeyTable<BrowserAct> = KeyTable {
    block: "picker.browser",
    bindings: &[
        Binding {
            action: BrowserAct::StepIn,
            id: "step_in",
            keys: &["TAB", "<right>"],
            label: "Step Into the Selected Directory",
        },
        Binding {
            action: BrowserAct::StepOut,
            id: "step_out",
            keys: &["<left>", "C-h"],
            label: "Step Out to the Parent Directory",
        },
        Binding {
            action: BrowserAct::Up,
            id: "up",
            keys: &["RET"],
            label: "Step Up From ../",
        },
    ],
    configurable: true,
    title: "Picker: Browser",
};

// ========================================================================
// Data Structures
// ========================================================================

/// What a browser key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum BrowserAct {
    StepIn,
    StepOut,
    Up,
}

/// One directory, browsed.
pub(in crate::window) struct BrowserSource {
    /// What it was opened to choose, in place of opening what is chosen.
    chooser: Option<Chooser>,
    dir: PathBuf,
    /// The entries listed, by row, filled by the reading thread.
    items: Arc<Mutex<Vec<DirItem>>>,
    /// The entry to start on, once its directory is read: the file the
    /// browser was opened at.
    select: Option<PathBuf>,
}

/// What a browser opened to choose a file does with the choice, and what it
/// is headed by.
pub(in crate::window) struct Chooser {
    /// What choosing a file does.
    pub(in crate::window) file: Choose,
    pub(in crate::window) title: String,
}

/// What a choice becomes.
pub(in crate::window) type Choose = Box<dyn Fn(PathBuf) -> WindowOutcome + Send>;

/// One entry of a directory, as the browser lists it.
#[derive(Clone, Debug, Eq, PartialEq)]
struct DirItem {
    /// Whether it is a directory, somewhere to go rather than to open.
    is_dir: bool,
    /// What the row says.
    label: String,
    /// Its full path.
    path: PathBuf,
}

// ========================================================================
// BrowserSource
// ========================================================================

impl BrowserSource {
    /// The browser at `dir`.
    pub(in crate::window) fn new(dir: PathBuf) -> Self {
        Self {
            chooser: None,
            dir,
            items: Arc::default(),
            select: None,
        }
    }

    /// The browser at `place`: in it when it is a directory, else in the
    /// directory holding it, started on it.
    pub(in crate::window) fn at(place: PathBuf) -> Self {
        if is_dir(&place) {
            return Self::new(place);
        }
        let Some(dir) = parent_of(&place) else {
            return Self::new(place);
        };
        Self {
            select: Some(place),
            ..Self::new(dir)
        }
    }

    /// The browser at `dir`, opened to choose as `chooser` says.
    #[cfg(feature = "wasm-extensions")]
    pub(in crate::window) fn choosing(dir: PathBuf, chooser: Chooser) -> Self {
        Self {
            chooser: Some(chooser),
            ..Self::new(dir)
        }
    }

    fn item(&self, row: RowKey) -> Option<DirItem> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.get(usize::try_from(row.0).ok()?).cloned()
    }

    /// Go to `dir`, its entries to be read and the query cleared.
    fn go(&mut self, dir: PathBuf) -> Option<SourceKey<WindowOutcome>> {
        self.dir = dir;
        Some(SourceKey::Reset)
    }

    /// The directory a `/` typed after `query` goes to: the root for an
    /// empty query, home for `~`, the query as a path against the directory
    /// shown when that is one, else the selected row when it is one.
    fn typed_dir(&self, query: &str, row: Option<RowKey>) -> Option<PathBuf> {
        let candidate = match query {
            "" => root_of(&self.dir),
            query => {
                let typed = join(&self.dir, query);
                match is_dir(&typed) {
                    true => typed,
                    false => self.item(row?)?.path,
                }
            }
        };
        is_dir(&candidate).then_some(candidate)
    }

    /// The new file `query` names, against the directory shown: `None` for
    /// no name, a directory's (ended with `/`), a list of hosts, or a name
    /// an entry already has.
    fn new_file_path(&self, query: &str) -> Option<PathBuf> {
        let ends_dir = query.ends_with(std::path::is_separator);
        if query.trim().is_empty() || ends_dir {
            return None;
        }
        if matches!(remote::parse(&self.dir), Some(Place::Targets(_))) {
            return None;
        }
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        let named = |item: &DirItem| item.label.trim_end_matches('/') == query;
        if items.iter().any(named) {
            return None;
        }
        let path = join(&self.dir, query);
        let local = remote::parse(&path).is_none();
        (!(local && path.exists())).then_some(path)
    }
}

impl Source<WindowOutcome, Preview> for BrowserSource {
    fn id(&self) -> &'static str {
        "browser"
    }

    fn title(&self) -> String {
        match &self.chooser {
            Some(chooser) => format!("{}: {}", chooser.title, abbreviate_home(&self.dir)),
            None => abbreviate_home(&self.dir),
        }
    }

    fn placeholder(&self) -> String {
        "Name, or a path ending in a slash".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::Words(MatchOn::LABEL)
    }

    fn resume(&self) -> Resume {
        Resume::Reread
    }

    fn feed(&mut self, _query: &str) -> Feed {
        let (send, batches) = mpsc::channel();
        let (dir, kept) = (self.dir.clone(), Arc::clone(&self.items));
        let select = self.select.take();
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_string())
            .spawn(move || {
                let batch = match listing(&dir) {
                    Some(items) => {
                        let rows = items
                            .iter()
                            .zip(0..)
                            .map(|(item, at)| item_row(RowKey(at), item));
                        let rows = rows.collect();
                        let start = select
                            .and_then(|place| items.iter().position(|item| item.path == place))
                            .map(|at| RowKey(at as u64));
                        *kept.lock().unwrap_or_else(PoisonError::into_inner) = items;
                        Batch {
                            done: true,
                            rows,
                            select: start,
                            ..Batch::default()
                        }
                    }
                    None => Batch {
                        done: true,
                        error: Some(UNREADABLE.to_string()),
                        ..Batch::default()
                    },
                };
                let _ = send.send(batch);
                wake_window();
            });
        match spawned {
            Ok(_) => Feed::Stream(batches),
            Err(_) => Feed::Failed(UNREADABLE.to_string()),
        }
    }

    fn query_row(&self, query: &str) -> Option<Row> {
        if self.chooser.is_some() {
            return None;
        }
        let name = self
            .new_file_path(query)?
            .file_name()?
            .to_string_lossy()
            .into_owned();
        let row = Row::new(NEW_FILE_ROW, query).with_icon_kind(IconKind::File { name });
        Some(row.with_hint(NEW_FILE_HINT))
    }

    fn actions(&self, row: RowKey) -> Vec<Action> {
        let (id, title) = match (row == NEW_FILE_ROW, &self.chooser) {
            (true, _) => (CREATE, "Create File"),
            (false, Some(_)) => (OPEN, "Choose"),
            (false, None) => (OPEN, "Open"),
        };
        vec![Action {
            button: None,
            id,
            title: title.to_string(),
        }]
    }

    fn run(&mut self, _action: ActionId, row: RowKey, query: &str) -> Option<WindowOutcome> {
        if row == NEW_FILE_ROW {
            return Some(WindowOutcome::CreateFile(self.new_file_path(query)?));
        }
        let item = self.item(row)?;
        if let Some(chooser) = &self.chooser {
            return (!item.is_dir).then(|| (chooser.file)(item.path));
        }
        match item.is_dir {
            true => Some(WindowOutcome::OpenDir(item.path)),
            // Over the focused pane, as every other way into a file does
            // it, so closing it puts the shell back.
            false => Some(WindowOutcome::OpenFile(OpenTarget::file(item.path))),
        }
    }

    fn key(
        &mut self,
        key: &misty::Key,
        row: Option<RowKey>,
        query: &str,
    ) -> Option<SourceKey<WindowOutcome>> {
        let key = &winter_key(*key);
        let item = row.and_then(|row| self.item(row));
        match find(&KEYS, key) {
            // `./` is where the browser already is.
            Some(BrowserAct::StepIn) => {
                let dir = item
                    .filter(|item| item.is_dir && item.label != CURRENT_LABEL)?
                    .path;
                self.go(dir)
            }
            Some(BrowserAct::StepOut) => {
                let up = parent_of(&self.dir)?;
                self.go(up)
            }
            // `Enter` on `../` steps up, the browser staying open; one
            // choosing steps into any directory but the one it is in.
            Some(BrowserAct::Up) => {
                let choosing = self.chooser.is_some();
                let steps = |item: &DirItem| match choosing {
                    true => item.is_dir && item.label != CURRENT_LABEL,
                    false => item.label == PARENT_LABEL,
                };
                let to = item.filter(steps)?;
                self.go(to.path)
            }
            None => {
                let typed = key.typed_char().is_some_and(std::path::is_separator);
                let dir = typed.then(|| self.typed_dir(query, row)).flatten()?;
                self.go(dir)
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Make `path` an empty file, with the directories it is in, to open it new;
/// a file there already is kept as it is.
pub(in crate::window) fn create_file(path: &Path) -> io::Result<()> {
    if let Some(at) = remote::remote_of(path) {
        if remote::stat(&at).is_some() {
            return Ok(());
        }
        return remote::write(&at, &[]).map_err(|e| match e {
            RemoteError::Failed(reason) => io::Error::other(reason),
            RemoteError::NeedsPassword => io::Error::other(NEEDS_PASSWORD),
        });
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e),
    }
}

/// The rows of `dir`: `./` first where it can be opened in Dir (anywhere but
/// a list of hosts or containers), `../` where there is a parent to reach,
/// then directories, then files, each by name, with the hidden ones after
/// the rest of their kind. `None` when it cannot be read.
fn listing(dir: &Path) -> Option<Vec<DirItem>> {
    let mut items = read_dir(dir)?;
    items.sort_by_key(|item| {
        (
            !item.is_dir,
            item.label.starts_with('.'),
            item.label.to_lowercase(),
        )
    });
    let mut listed = Vec::new();
    if !matches!(remote::parse(dir), Some(Place::Targets(_))) {
        listed.push(DirItem {
            is_dir: true,
            label: CURRENT_LABEL.to_string(),
            path: dir.to_path_buf(),
        });
    }
    if let Some(parent) = parent_of(dir) {
        listed.push(DirItem {
            is_dir: true,
            label: PARENT_LABEL.to_string(),
            path: parent,
        });
    }
    listed.extend(items);
    Some(listed)
}

/// An entry's row: its name, a directory's ended with `/`, with its icon;
/// `./` and `../` say where they lead.
fn item_row(key: RowKey, item: &DirItem) -> Row {
    // `./` stands for where the browser is, not for a folder of its own
    // name, so it takes the plain folder icon.
    let name = match item.label.as_str() {
        CURRENT_LABEL => String::new(),
        _ => item
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
    };
    let icon = match item.is_dir {
        true => IconKind::Dir {
            expanded: false,
            name,
        },
        false => IconKind::File { name },
    };
    let row = Row::new(key, item.label.clone()).with_icon_kind(icon);
    match item.label.as_str() {
        CURRENT_LABEL => row.with_hint(CURRENT_HINT),
        PARENT_LABEL => row.with_hint(PARENT_HINT),
        _ => row,
    }
}

/// The entries of `dir`, local or remote.
fn read_dir(dir: &Path) -> Option<Vec<DirItem>> {
    match remote::parse(dir) {
        Some(Place::Remote(at)) => return remote_items(&at),
        Some(Place::Targets(kind)) => return Some(target_items(kind)),
        None => {}
    }
    let items = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| {
            let is_dir = entry.metadata().is_ok_and(|meta| meta.is_dir());
            dir_item(
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
                is_dir,
            )
        })
        .collect();
    Some(items)
}

fn dir_item(name: String, path: PathBuf, is_dir: bool) -> DirItem {
    let label = match is_dir {
        true => format!("{name}/"),
        false => name,
    };
    DirItem {
        is_dir,
        label,
        path,
    }
}

fn remote_items(dir: &Remote) -> Option<Vec<DirItem>> {
    let entries = remote::list(dir).ok()?;
    let items = entries
        .into_iter()
        .map(|entry| {
            let path = dir.child(&entry.name).to_path();
            dir_item(entry.name, path, entry.is_dir)
        })
        .collect();
    Some(items)
}

fn target_items(kind: Kind) -> Vec<DirItem> {
    remote::targets(kind)
        .into_iter()
        .map(|name| {
            let path = Remote::target_root(kind, &name).to_path();
            dir_item(name, path, true)
        })
        .collect()
}

/// Whether `path` is a directory that can be browsed.
fn is_dir(path: &Path) -> bool {
    match remote::parse(path) {
        Some(Place::Remote(at)) => remote::is_dir(&at),
        Some(Place::Targets(_)) => true,
        None => path.is_dir(),
    }
}

/// The directory `..` leads to from `dir`, or `None` at the top.
fn parent_of(dir: &Path) -> Option<PathBuf> {
    match remote::parse(dir) {
        Some(Place::Remote(at)) => match at.parent() {
            Some(up) => Some(up.to_path()),
            None => at.kind().map(remote::targets_path),
        },
        Some(Place::Targets(_)) => None,
        None => dir.parent().map(Path::to_path_buf),
    }
}

/// The root of the filesystem `dir` is on, where a `/` typed alone goes.
fn root_of(dir: &Path) -> PathBuf {
    match remote::remote_of(dir) {
        Some(at) => at.at("/").to_path(),
        None => dir.ancestors().last().unwrap_or(dir).to_path_buf(),
    }
}

/// Where `query`, typed as a path, leads from `dir`.
fn join(dir: &Path, query: &str) -> PathBuf {
    if remote::parse(Path::new(query)).is_some() {
        return PathBuf::from(query);
    }
    match remote::remote_of(dir) {
        Some(at) if !query.starts_with('~') => at.join(query).to_path(),
        Some(_) | None => clean_path(&dir.join(expand_home(query))),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::input::{Key, KeyCode};

    use crate::window::input::misty::misty_key;

    use super::*;

    fn tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("winter-browser-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).expect("temp tree");
        fs::create_dir_all(root.join(".hidden")).expect("temp tree");
        fs::write(root.join("b.rs"), "").expect("temp file");
        fs::write(root.join("a.rs"), "").expect("temp file");
        root
    }

    #[test]
    fn test_a_listing_leads_here_and_up_then_directories_then_files_hidden_last() {
        let root = tree();
        let labels: Vec<String> = listing(&root)
            .expect("readable")
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert_eq!(labels, ["./", "../", "src/", ".hidden/", "a.rs", "b.rs"]);
    }

    #[test]
    fn test_the_dot_row_takes_the_plain_folder_icon_and_others_their_own() {
        let root = tree();
        let items = listing(&root).unwrap();
        let icon = |at: usize| item_row(RowKey(at as u64), &items[at]).icon;
        let plain = IconKind::Dir {
            expanded: false,
            name: String::new(),
        };
        assert_eq!(icon(0), crate::icons::name_for(&plain), "`./`");
        let src = IconKind::Dir {
            expanded: false,
            name: "src".to_string(),
        };
        assert_eq!(
            icon(2),
            crate::icons::name_for(&src),
            "a folder keeps its name's icon"
        );
    }

    #[test]
    fn test_enter_on_dot_opens_the_browsed_directory_and_tab_stays() {
        let root = tree();
        let mut browser = BrowserSource::new(root.clone());
        *browser.items.lock().unwrap() = listing(&root).unwrap();
        let here = RowKey(0);
        assert_eq!(
            browser.key(&misty_key(&Key::plain(KeyCode::Enter)), Some(here), ""),
            None,
            "only `../` steps on `Enter`"
        );
        assert_eq!(
            browser.run(OPEN, here, ""),
            Some(WindowOutcome::OpenDir(root.clone()))
        );
        assert_eq!(
            browser.key(&misty_key(&Key::plain(KeyCode::Tab)), Some(here), ""),
            None,
            "`./` is not stepped into"
        );
        assert_eq!(browser.dir, root);
    }

    #[test]
    fn test_tab_steps_into_a_directory_and_left_steps_out() {
        let root = tree();
        let mut browser = BrowserSource::new(root.clone());
        *browser.items.lock().unwrap() = listing(&root).unwrap();
        let src = RowKey(2);
        assert_eq!(
            browser.key(&misty_key(&Key::plain(KeyCode::Tab)), Some(src), ""),
            Some(SourceKey::Reset)
        );
        assert_eq!(browser.dir, root.join("src"));
        assert_eq!(
            browser.key(&misty_key(&Key::plain(KeyCode::Left)), None, ""),
            Some(SourceKey::Reset)
        );
        assert_eq!(browser.dir, root);
        let file = RowKey(4);
        assert_eq!(
            browser.key(&misty_key(&Key::plain(KeyCode::Tab)), Some(file), ""),
            None,
            "a file is not stepped into"
        );
    }

    #[test]
    fn test_the_browser_opened_at_a_file_starts_on_it() {
        let root = tree();
        let mut browser = BrowserSource::at(root.join("b.rs"));
        assert_eq!(browser.dir, root);
        let Feed::Stream(batches) = browser.feed("") else {
            panic!("read on a thread");
        };
        let batch = batches.recv().expect("the listing");
        let start = batch.select.expect("a row to start on");
        assert_eq!(batch.rows[start.0 as usize].label, "b.rs");
    }

    #[test]
    fn test_a_query_naming_no_entry_is_offered_as_a_new_file() {
        let root = tree();
        let mut browser = BrowserSource::new(root.clone());
        *browser.items.lock().unwrap() = listing(&root).unwrap();
        let offered = |query: &str| browser.query_row(query).map(|row| row.label);
        assert_eq!(offered("a.r").as_deref(), Some("a.r"), "a part of a name");
        assert_eq!(offered("a.rs"), None, "a file's whole name");
        assert_eq!(offered("src"), None, "a directory's, without its `/`");
        assert_eq!(offered("new/"), None, "a directory to make");
        assert_eq!(offered("src/b.rs").as_deref(), Some("src/b.rs"));
        assert_eq!(offered("../"), None);
        assert_eq!(offered(" "), None);
        assert_eq!(
            browser.run(CREATE, NEW_FILE_ROW, "src/b.rs"),
            Some(WindowOutcome::CreateFile(root.join("src/b.rs")))
        );
    }

    #[test]
    fn test_creating_a_file_makes_its_directories_and_keeps_one_there() {
        let root = std::env::temp_dir().join(format!("winter-create-{}", std::process::id()));
        let made = root.join("deep/new.rs");
        create_file(&made).expect("made");
        assert_eq!(fs::read_to_string(&made).expect("there"), "");
        fs::write(&made, "kept").expect("written");
        create_file(&made).expect("already there");
        assert_eq!(fs::read_to_string(&made).expect("there"), "kept");
        fs::remove_dir_all(root).expect("cleaned");
    }

    #[cfg(feature = "wasm-extensions")]
    #[test]
    fn test_a_browser_choosing_hands_back_its_choice_and_steps_into_directories() {
        let root = tree();
        let chooser = Chooser {
            file: Box::new(WindowOutcome::OpenDir),
            title: "Choose".to_string(),
        };
        let mut browser = BrowserSource::choosing(root.clone(), chooser);
        *browser.items.lock().unwrap() = listing(&root).unwrap();
        let (here, src, file) = (RowKey(0), RowKey(2), RowKey(4));
        assert_eq!(
            browser.run(OPEN, file, ""),
            Some(WindowOutcome::OpenDir(root.join("a.rs"))),
            "the file chosen, handed back"
        );
        assert_eq!(
            browser.run(OPEN, here, ""),
            None,
            "a directory is not chosen"
        );
        assert_eq!(browser.query_row("new.rs"), None, "nothing is made");
        assert_eq!(
            browser.key(&misty_key(&Key::plain(KeyCode::Enter)), Some(src), ""),
            Some(SourceKey::Reset)
        );
        assert_eq!(browser.dir, root.join("src"));
    }

    #[test]
    fn test_a_slash_after_a_typed_path_goes_there() {
        let root = tree();
        let mut browser = BrowserSource::new(root.clone());
        let slash = misty_key(&Key::plain(KeyCode::Char('/')));
        assert_eq!(browser.key(&slash, None, "src"), Some(SourceKey::Reset));
        assert_eq!(browser.dir, root.join("src"));
        assert_eq!(
            browser.key(&slash, None, "nowhere"),
            None,
            "a path that is not one filters"
        );
    }
}
