//! An agent's record, rebuilt each time rather than kept: the worktrees
//! `git worktree list` gives on a branch under [`BRANCH_PREFIX`], each
//! joined with the mux session named after its branch. A worktree with a
//! live session is a running agent, one without a stopped agent, so an agent
//! made in an earlier run, or before Winter last forgot, still shows up.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// What every agent's branch starts with: `agent/claude-1`.
pub(super) const BRANCH_PREFIX: &str = "agent/";

/// Where an agent's worktree goes when `settings.kdl` says nowhere,
/// relative to the repository, `{repo}` its folder's name.
pub(super) const DEFAULT_WORKTREES: &str = "../{repo}.worktrees";

/// What stands for the repository's folder name in a worktrees setting.
const REPO_PLACEHOLDER: &str = "{repo}";

/// What a session name holds in place of a branch's `/`, which mux session
/// names may not hold.
const SESSION_SEPARATOR: &str = "-";

/// What every agent's mux session name starts with: its branch's prefix,
/// with the `/` a session name cannot hold made `-`.
pub(super) const SESSION_PREFIX: &str = "agent-";

/// The git setting, under the agent's branch's own section, that keeps the
/// branch it started from: `branch.agent/claude-1.winterBase`, which git
/// lists in lower case and drops with the branch.
pub(super) const BASE_KEY: &str = "winterBase";

/// The lines of `git worktree list --porcelain` naming a worktree and its
/// branch.
const WORKTREE_LINE: &str = "worktree ";
const BRANCH_LINE: &str = "branch refs/heads/";

// ========================================================================
// Data Structures
// ========================================================================

/// One agent, running or stopped.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Agent {
    /// The branch it started from, which it lands on; `None` for one made
    /// before Winter kept it.
    pub(in crate::window) base: Option<String>,
    /// Its branch: `agent/claude-1`.
    pub(in crate::window) branch: String,
    /// Which agent it is, by its name in `settings.kdl`: `claude`.
    pub(in crate::window) kind: String,
    /// Whether its mux session is live.
    pub(in crate::window) running: bool,
    /// Its checkout.
    pub(in crate::window) worktree: PathBuf,
}

/// A worktree as `git worktree list` gives it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Worktree {
    /// Its branch, unless it is detached.
    pub(super) branch: Option<String>,
    pub(super) path: PathBuf,
}

// ========================================================================
// Agent Implementations
// ========================================================================

impl Agent {
    /// The mux session it runs in.
    pub(in crate::window) fn session(&self) -> String {
        session_name(&self.branch)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The worktrees `porcelain`, the output of `git worktree list
/// --porcelain`, lists.
pub(super) fn parse_worktrees(porcelain: &str) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    for line in porcelain.lines() {
        if let Some(path) = line.strip_prefix(WORKTREE_LINE) {
            worktrees.push(Worktree {
                branch: None,
                path: PathBuf::from(path),
            });
        } else if let (Some(branch), Some(last)) =
            (line.strip_prefix(BRANCH_LINE), worktrees.last_mut())
        {
            last.branch = Some(branch.to_string());
        }
    }
    worktrees
}

/// The branch each agent started from, by its branch, from `listed`: what
/// `git config --get-regexp` gives for [`BASE_KEY`], one `key value` a line.
pub(super) fn parse_bases(listed: &str) -> HashMap<String, String> {
    let suffix = format!(".{}", BASE_KEY.to_ascii_lowercase());
    listed
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(' ')?;
            let branch = key.strip_prefix("branch.")?.strip_suffix(&suffix)?;
            Some((branch.to_string(), value.trim().to_string()))
        })
        .collect()
}

/// The branch the agent in mux session `session` works on: `agent/claude-1`
/// for `agent-claude-1`; `None` for a session that is no agent's.
pub(super) fn branch_of_session(session: &str) -> Option<String> {
    let rest = session.strip_prefix(SESSION_PREFIX)?;
    let branch = format!("{BRANCH_PREFIX}{rest}");
    kind_of(&branch).is_some().then_some(branch)
}

/// The agents among `worktrees`, each running when `sessions` (the mux
/// server's session names) holds its session, and with the branch `bases`
/// says it started from.
pub(super) fn agents_of(
    worktrees: &[Worktree],
    sessions: &[String],
    bases: &HashMap<String, String>,
) -> Vec<Agent> {
    worktrees
        .iter()
        .filter_map(|worktree| {
            let branch = worktree.branch.as_deref()?;
            let kind = kind_of(branch)?;
            Some(Agent {
                base: bases.get(branch).cloned(),
                branch: branch.to_string(),
                kind: kind.to_string(),
                running: sessions.contains(&session_name(branch)),
                worktree: worktree.path.clone(),
            })
        })
        .collect()
}

/// The agents' sessions among `sessions` that no agent in `agents` runs in:
/// left behind when a worktree was removed outside Winter.
pub(super) fn sessions_left(agents: &[Agent], sessions: &[String]) -> Vec<String> {
    sessions
        .iter()
        .filter(|session| branch_of_session(session).is_some())
        .filter(|session| !agents.iter().any(|agent| agent.session() == **session))
        .cloned()
        .collect()
}

/// The mux session an agent on `branch` runs in: its branch, with `/` made
/// `-`.
pub(super) fn session_name(branch: &str) -> String {
    branch.replace('/', SESSION_SEPARATOR)
}

/// Which agent `branch` is one of: `claude` for `agent/claude-1`; `None`
/// for a branch that is no agent's.
pub(super) fn kind_of(branch: &str) -> Option<&str> {
    let rest = branch.strip_prefix(BRANCH_PREFIX)?;
    let (kind, number) = rest.rsplit_once('-')?;
    let numbered = !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit());
    (numbered && !kind.is_empty()).then_some(kind)
}

/// The first branch for a new `kind` agent that no branch in `taken` is:
/// `agent/claude-1`, then `agent/claude-2`.
pub(super) fn next_branch(kind: &str, taken: &[String]) -> String {
    (1..)
        .map(|number| format!("{BRANCH_PREFIX}{kind}-{number}"))
        .find(|branch| !taken.contains(branch))
        .unwrap_or_default()
}

/// Where the worktrees of the repository at `repo` go, by `template`
/// (relative to the repository, `{repo}` its folder's name), with `..`
/// steps taken.
pub(super) fn worktrees_dir(repo: &Path, template: &str) -> PathBuf {
    let name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let place = template.replace(REPO_PLACEHOLDER, &name);
    let mut dir = PathBuf::new();
    for part in repo
        .join(winter_model::path::expand_home(&place))
        .components()
    {
        match part {
            Component::ParentDir => {
                dir.pop();
            }
            Component::CurDir => {}
            other => dir.push(other),
        }
    }
    dir
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const PORCELAIN: &str = "worktree /src/app
HEAD 1111111111111111111111111111111111111111
branch refs/heads/main

worktree /src/app.worktrees/agent/claude-1
HEAD 2222222222222222222222222222222222222222
branch refs/heads/agent/claude-1

worktree /src/app.worktrees/agent/codex-2
HEAD 3333333333333333333333333333333333333333
branch refs/heads/agent/codex-2

worktree /src/elsewhere
HEAD 4444444444444444444444444444444444444444
detached
";

    #[test]
    fn test_agents_are_the_worktrees_on_agent_branches_running_where_a_session_lives() {
        let worktrees = parse_worktrees(PORCELAIN);
        assert_eq!(worktrees.len(), 4);
        assert_eq!(worktrees[3].branch, None, "detached");
        let bases =
            parse_bases("branch.agent/claude-1.winterbase main\nbranch.main.remote origin\n");
        let agents = agents_of(
            &worktrees,
            &["agent-codex-2".to_string(), "default".to_string()],
            &bases,
        );
        assert_eq!(agents[0].base.as_deref(), Some("main"), "kept in git");
        assert_eq!(agents[1].base, None, "made before Winter kept it");
        let seen: Vec<(&str, &str, bool)> = agents
            .iter()
            .map(|agent| (agent.kind.as_str(), agent.branch.as_str(), agent.running))
            .collect();
        assert_eq!(
            seen,
            [
                ("claude", "agent/claude-1", false),
                ("codex", "agent/codex-2", true)
            ]
        );
        assert_eq!(agents[1].session(), "agent-codex-2");
        assert_eq!(
            agents[0].worktree,
            PathBuf::from("/src/app.worktrees/agent/claude-1")
        );
    }

    #[test]
    fn test_only_a_numbered_branch_under_the_prefix_is_an_agent_s() {
        assert_eq!(kind_of("agent/claude-12"), Some("claude"));
        assert_eq!(
            kind_of("agent/open-code-3"),
            Some("open-code"),
            "a dash in the name"
        );
        for branch in [
            "main",
            "agent/claude",
            "agent/claude-x",
            "agent/-1",
            "feature/a-1",
        ] {
            assert_eq!(kind_of(branch), None, "{branch}");
        }
    }

    #[test]
    fn test_a_session_names_its_agent_s_branch_and_no_other_session_does() {
        assert_eq!(
            branch_of_session("agent-claude-12").as_deref(),
            Some("agent/claude-12")
        );
        assert_eq!(branch_of_session("default"), None);
        assert_eq!(branch_of_session("agent-nonumber"), None);
    }

    #[test]
    fn test_a_new_branch_takes_the_first_number_no_branch_has() {
        assert_eq!(next_branch("claude", &[]), "agent/claude-1");
        let taken = ["agent/claude-1".to_string(), "agent/claude-3".to_string()];
        assert_eq!(next_branch("claude", &taken), "agent/claude-2");
        assert_eq!(next_branch("codex", &taken), "agent/codex-1");
    }

    #[test]
    fn test_worktrees_go_beside_the_repository_named_after_it() {
        let repo = Path::new("/src/app");
        assert_eq!(
            worktrees_dir(repo, DEFAULT_WORKTREES),
            PathBuf::from("/src/app.worktrees")
        );
        assert_eq!(
            worktrees_dir(repo, ".agents"),
            PathBuf::from("/src/app/.agents")
        );
        assert_eq!(
            worktrees_dir(repo, "/tmp/wt/{repo}"),
            PathBuf::from("/tmp/wt/app")
        );
    }
}
