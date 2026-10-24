//! The push menu, as magic-vscode's has it: `P` opens it over where the
//! branch pushes (its push remote and its upstream, each set first where it
//! is not), anywhere else, another branch or commit, a tag, or every tag,
//! with switches (`-` then a letter) that go with whichever push is chosen.

use std::path::Path;

use crate::model::editor::{EditorOutcome, Notice};
use crate::model::input::KeyCode;

use super::exec::{self, Candidates};
use super::popup::Popup;
use super::{
    GitEditor, ASK_PUSH_ELSEWHERE, ASK_PUSH_OTHER, ASK_PUSH_OTHER_TARGET, ASK_PUSH_REMOTE,
    ASK_PUSH_TAG, ASK_PUSH_UPSTREAM,
};

// ========================================================================
// Constants
// ========================================================================

/// What a remote's name is read out of: `remote.<name>.url`.
const REMOTE_PREFIX: &str = "remote.";
const URL_SUFFIX: &str = ".url";

/// The repository-wide push remote, which a branch without its own uses.
const PUSH_DEFAULT_KEY: &str = "remote.pushdefault";

/// What a branch's own push remote is read out of: `branch.<name>.pushremote`.
const BRANCH_PREFIX: &str = "branch.";
const PUSH_REMOTE_SUFFIX: &str = ".pushremote";

/// The switches a push can take, as `(key, flag, what it does)`: the key is
/// pressed in the options menu, after `-`, to turn the flag on or off.
pub(super) const SWITCHES: [(char, &str, &str); 5] = [
    ('f', "--force-with-lease", "Force with lease"),
    ('F', "--force", "Force"),
    ('i', "--force-if-includes", "Force if includes"),
    ('h', "--no-verify", "Disable hooks"),
    ('d', "--dry-run", "Dry run"),
];

// ========================================================================
// Data Structures
// ========================================================================

/// Where the branch pushes and what it can push to, read when the menu
/// opens, and the switches turned on while it is open.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(super) struct PushMenu {
    /// The ref picked to push by `o`, while its destination is picked.
    pub(super) other: Option<String>,
    /// The remote picked for `p` while it is being set as the push remote.
    pub(super) pending_remote: Option<String>,
    /// The remote the branch pushes to, its own or the repository's.
    pub(super) push_remote: Option<String>,
    /// Every remote, by name.
    pub(super) remotes: Vec<String>,
    /// The keys of the switches turned on, as in [`SWITCHES`].
    pub(super) switches: Vec<char>,
}

// ========================================================================
// PushMenu Implementations
// ========================================================================

impl PushMenu {
    /// The menu for `branch`, read from what `git config --get-regexp`
    /// printed: `key value` lines.
    pub(super) fn parse(config: &str, branch: Option<&str>) -> Self {
        let own_key = branch.map(|branch| format!("{BRANCH_PREFIX}{branch}{PUSH_REMOTE_SUFFIX}"));
        let mut menu = Self::default();
        let mut default = None;
        for line in config.lines() {
            let Some((key, value)) = line.trim().split_once(' ') else {
                continue;
            };
            let value = value.trim().to_string();
            if Some(key) == own_key.as_deref() {
                menu.push_remote = Some(value);
            } else if key == PUSH_DEFAULT_KEY {
                default = Some(value);
            } else if let Some(name) = key
                .strip_prefix(REMOTE_PREFIX)
                .and_then(|rest| rest.strip_suffix(URL_SUFFIX))
            {
                if !menu.remotes.iter().any(|remote| remote == name) {
                    menu.remotes.push(name.to_string());
                }
            }
        }
        menu.push_remote = menu.push_remote.or(default);
        menu
    }

    /// The flags of the switches turned on, in [`SWITCHES`] order.
    pub(super) fn flags(&self) -> Vec<&'static str> {
        SWITCHES
            .iter()
            .filter(|(key, _, _)| self.switches.contains(key))
            .map(|(_, flag, _)| *flag)
            .collect()
    }

    /// Turn the switch on `key` on or off. Returns whether `key` is one.
    pub(super) fn toggle(&mut self, key: char) -> bool {
        if !SWITCHES.iter().any(|(switch, _, _)| *switch == key) {
            return false;
        }
        match self.switches.iter().position(|on| *on == key) {
            Some(at) => {
                self.switches.remove(at);
            }
            None => self.switches.push(key),
        }
        true
    }

    /// What the menu offers, as its keys and what each says, for `branch`
    /// tracking `upstream`: the push remote and the upstream by name where
    /// they are set, and the switches turned on beside `-`.
    pub(super) fn choices(
        &self,
        branch: Option<&str>,
        upstream: Option<&str>,
    ) -> Vec<(String, String)> {
        let branch = branch.unwrap_or("HEAD");
        let push_remote = match &self.push_remote {
            Some(remote) => format!("{remote}/{branch}"),
            None => "pushRemote, after setting that".to_string(),
        };
        let upstream = upstream.map_or(
            "@{upstream}, after setting that".to_string(),
            str::to_string,
        );
        let flags = self.flags();
        let options = match flags.is_empty() {
            true => "Options".to_string(),
            false => format!("Options [{}]", flags.join(" ")),
        };
        [
            ('-', options),
            ('p', push_remote),
            ('u', upstream),
            ('e', "elsewhere".to_string()),
            ('o', "another branch/commit".to_string()),
            ('T', "a tag".to_string()),
            ('t', "all tags".to_string()),
        ]
        .into_iter()
        .map(|(key, what)| (key.to_string(), what))
        .collect()
    }

    /// The options menu: each switch, marked where it is on, and the way
    /// back to the push menu.
    pub(super) fn option_choices(&self) -> Vec<(String, String)> {
        let mut choices: Vec<(String, String)> = SWITCHES
            .iter()
            .map(|(key, flag, what)| {
                let mark = match self.switches.contains(key) {
                    true => "[x]",
                    false => "[ ]",
                };
                (key.to_string(), format!("{mark} {what} ({flag})"))
            })
            .collect();
        choices.push(("RET".to_string(), "back to Pushing".to_string()));
        choices
    }

    /// `name` (`origin/feature`) as its remote and its branch there, the
    /// remote being the longest known one it starts with, since a remote's
    /// own name may hold a `/`.
    pub(super) fn split_remote<'a>(&self, name: &'a str) -> Option<(&'a str, &'a str)> {
        self.remotes
            .iter()
            .filter(|remote| {
                name.strip_prefix(remote.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
            })
            .max_by_key(|remote| remote.len())
            .map(|remote| (&name[..remote.len()], &name[remote.len() + 1..]))
            .filter(|(_, branch)| !branch.is_empty())
    }

    /// `<remote>/<name>` for every remote whose list of branches does not
    /// hold it yet, where a first push of `name` would create it: what a
    /// list of destinations offers ahead of the branches already there.
    pub(super) fn new_destinations(&self, name: &str, listed: &str) -> Vec<String> {
        self.remotes
            .iter()
            .map(|remote| format!("{remote}/{name}"))
            .filter(|target| !listed.lines().any(|line| line.trim() == target))
            .collect()
    }
}

// ========================================================================
// GitEditor: pushing
// ========================================================================

impl GitEditor {
    /// Open the push menu over what `git config` said of where the branch
    /// pushes. The switches start off, as magic-vscode's do.
    pub(super) fn open_push_menu(&mut self, config: &str) -> EditorOutcome {
        self.push = Some(PushMenu::parse(config, self.status.branch.as_deref()));
        self.open_popup(Popup::Push)
    }

    /// A key in the options menu: a switch's letter turns it on or off and
    /// keeps the menu up, `RET` (or `ESC`) goes back to the push menu.
    pub(super) fn on_push_option_key(&mut self, code: KeyCode) -> EditorOutcome {
        match code {
            KeyCode::Enter | KeyCode::Escape => self.open_popup(Popup::Push),
            KeyCode::Char(key) => {
                if let Some(menu) = self.push.as_mut() {
                    menu.toggle(key);
                }
                self.open_popup(Popup::PushOptions)
            }
            _ => self.open_popup(Popup::PushOptions),
        }
    }

    /// A key in the push menu.
    pub(super) fn on_push_choice(&mut self, root: &Path, choice: char) -> EditorOutcome {
        let Some(menu) = self.push.clone() else {
            return EditorOutcome::Consumed;
        };
        let flags = menu.flags();
        let branch = self.status.branch.clone();
        match (choice, branch) {
            ('-', _) => self.open_popup(Popup::PushOptions),
            ('o', _) => self.pick(ASK_PUSH_OTHER, "Push", Candidates::Branches),
            ('T', _) => match self.tag_remote(&menu) {
                Some(_) => self.pick(ASK_PUSH_TAG, "Push tag", Candidates::Tags),
                None => self.say_error("no upstream or push remote to push tags to"),
            },
            ('t', _) => match self.tag_remote(&menu) {
                Some(remote) => EditorOutcome::Job(exec::push_tags(root, &flags, &remote)),
                None => self.say_error("no upstream or push remote to push tags to"),
            },
            ('p' | 'u' | 'e', None) => self.say_error("no branch to push"),
            ('p', Some(branch)) => match &menu.push_remote {
                Some(remote) => EditorOutcome::Job(exec::push_to(root, &flags, remote, &branch)),
                None => self.pick(ASK_PUSH_REMOTE, "Set the push remote", Candidates::Remotes),
            },
            ('u', Some(branch)) => match self.status.upstream.clone() {
                Some(upstream) => {
                    let remote = menu.split_remote(&upstream).map_or(
                        upstream.split('/').next().unwrap_or(&upstream),
                        |(remote, _)| remote,
                    );
                    EditorOutcome::Job(exec::push_to(root, &flags, remote, &branch))
                }
                None => self.pick(
                    ASK_PUSH_UPSTREAM,
                    "Set the upstream, and push to it",
                    Candidates::RemoteBranches,
                ),
            },
            ('e', Some(branch)) => self.pick(
                ASK_PUSH_ELSEWHERE,
                &format!("Push {branch} to"),
                Candidates::RemoteBranches,
            ),
            _ => EditorOutcome::Consumed,
        }
    }

    /// The answer to one of the push menu's questions, or `None` for a
    /// question of some other menu's.
    pub(super) fn answer_push(
        &mut self,
        root: &Path,
        tag: &str,
        answer: &str,
    ) -> Option<EditorOutcome> {
        let mut menu = self.push.clone()?;
        let flags = menu.flags();
        let branch = self.status.branch.clone().unwrap_or_default();
        let outcome = match tag {
            ASK_PUSH_REMOTE => {
                menu.pending_remote = Some(answer.to_string());
                EditorOutcome::Job(exec::set_push_remote(root, &branch, answer))
            }
            ASK_PUSH_OTHER => {
                menu.other = Some(answer.to_string());
                self.push = Some(menu);
                return Some(self.pick(
                    ASK_PUSH_OTHER_TARGET,
                    &format!("Push {answer} to"),
                    Candidates::RemoteBranches,
                ));
            }
            ASK_PUSH_TAG => match self.tag_remote(&menu) {
                Some(remote) => EditorOutcome::Job(exec::push_to(root, &flags, &remote, answer)),
                None => self.say_error("no upstream or push remote to push tags to"),
            },
            ASK_PUSH_UPSTREAM | ASK_PUSH_ELSEWHERE | ASK_PUSH_OTHER_TARGET => {
                let source = match tag {
                    ASK_PUSH_OTHER_TARGET => menu.other.take().unwrap_or_default(),
                    _ => branch,
                };
                match menu.split_remote(answer) {
                    None => self.say_error(&format!("not a branch on a remote: {answer}")),
                    Some((remote, name)) if tag == ASK_PUSH_UPSTREAM => EditorOutcome::Job(
                        exec::push_set_upstream(root, &flags, remote, &source, name),
                    ),
                    Some((remote, name)) => EditorOutcome::Job(exec::push_to(
                        root,
                        &flags,
                        remote,
                        &format!("{source}:{name}"),
                    )),
                }
            }
            _ => return None,
        };
        self.push = Some(menu);
        Some(outcome)
    }

    /// The push remote just set: push there, with the menu's switches.
    pub(super) fn push_after_setting_remote(&mut self, root: &Path) -> EditorOutcome {
        let Some(menu) = self.push.as_mut() else {
            return EditorOutcome::Consumed;
        };
        let (Some(remote), Some(branch)) = (menu.pending_remote.take(), self.status.branch.clone())
        else {
            return EditorOutcome::Consumed;
        };
        menu.push_remote = Some(remote.clone());
        let flags = menu.flags();
        EditorOutcome::Job(exec::push_to(root, &flags, &remote, &branch))
    }

    /// The list a push destination is picked from: the branches already on
    /// the remotes, after the ones a first push of the branch would create.
    /// `None` for a list some other question asked for.
    pub(super) fn push_destinations(&self, tag: &str, listed: &str) -> Option<String> {
        let menu = self.push.as_ref()?;
        let source = match tag {
            ASK_PUSH_UPSTREAM | ASK_PUSH_ELSEWHERE => self.status.branch.clone()?,
            ASK_PUSH_OTHER_TARGET => {
                let other = menu.other.clone()?;
                match menu.split_remote(&other) {
                    Some((_, name)) => name.to_string(),
                    None => other,
                }
            }
            _ => return None,
        };
        let mut lines = menu.new_destinations(&source, listed);
        lines.push(listed.to_string());
        Some(lines.join("\n"))
    }

    /// Where tags are pushed: the upstream's remote, else the push remote,
    /// as magic-vscode chooses.
    fn tag_remote(&self, menu: &PushMenu) -> Option<String> {
        let upstream = self.status.upstream.as_deref().map(|upstream| {
            menu.split_remote(upstream)
                .map_or(
                    upstream.split('/').next().unwrap_or(upstream),
                    |(remote, _)| remote,
                )
                .to_string()
        });
        upstream.or_else(|| menu.push_remote.clone())
    }

    /// Say `message` as an error and keep the key.
    fn say_error(&mut self, message: &str) -> EditorOutcome {
        self.notice = Some(Notice::error(message));
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = "remote.origin.url git@x:a.git\n\
                          remote.my/fork.url git@x:b.git\n\
                          remote.pushdefault origin\n\
                          branch.main.pushremote my/fork\n";

    #[test]
    fn test_the_branch_s_own_push_remote_wins_over_the_repository_s() {
        let menu = PushMenu::parse(CONFIG, Some("main"));
        assert_eq!(menu.remotes, ["origin", "my/fork"]);
        assert_eq!(menu.push_remote.as_deref(), Some("my/fork"));
        let other = PushMenu::parse(CONFIG, Some("dev"));
        assert_eq!(other.push_remote.as_deref(), Some("origin"), "the default");
        assert_eq!(PushMenu::parse("", Some("main")).push_remote, None);
    }

    #[test]
    fn test_the_menu_names_where_each_push_goes_or_says_it_will_ask() {
        let menu = PushMenu::parse(CONFIG, Some("main"));
        let choices = menu.choices(Some("main"), None);
        let said = |key: &str| {
            choices
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, what)| what.clone())
        };
        assert_eq!(said("p").as_deref(), Some("my/fork/main"));
        assert_eq!(
            said("u").as_deref(),
            Some("@{upstream}, after setting that")
        );
        assert_eq!(said("-").as_deref(), Some("Options"));
    }

    #[test]
    fn test_switches_toggle_and_show_beside_the_options_entry() {
        let mut menu = PushMenu::default();
        assert!(menu.toggle('d'));
        assert!(menu.toggle('f'));
        assert!(!menu.toggle('z'), "not a switch");
        assert_eq!(menu.flags(), ["--force-with-lease", "--dry-run"]);
        assert_eq!(
            menu.choices(None, None)[0].1,
            "Options [--force-with-lease --dry-run]"
        );
        assert!(menu.toggle('f'));
        assert_eq!(menu.flags(), ["--dry-run"], "pressed again, off");
        assert!(menu.option_choices()[4].1.starts_with("[x] Dry run"));
    }

    #[test]
    fn test_a_destination_splits_at_the_longest_remote_it_starts_with() {
        let menu = PushMenu::parse(CONFIG, None);
        assert_eq!(menu.split_remote("my/fork/a/b"), Some(("my/fork", "a/b")));
        assert_eq!(menu.split_remote("origin/x"), Some(("origin", "x")));
        assert_eq!(menu.split_remote("nowhere/x"), None);
        assert_eq!(
            menu.new_destinations("main", "origin/main\n"),
            ["my/fork/main"],
            "only where it is not there yet"
        );
    }
}
