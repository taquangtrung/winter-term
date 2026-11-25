//! The `agent` entries of `settings.kdl`'s `agents` block: read from the
//! document by hand, as the `lsp` block's servers are, since an entry is a
//! node with children; merged over the built-in agents by name, those
//! named first; and written back as given.

use kdl::{KdlDocument, KdlNode};

use super::schema::kdl_string;
use super::{AgentKind, AgentSetting};

// ========================================================================
// Constants
// ========================================================================

/// The block the agents are listed in, and the node naming each.
pub(super) const AGENTS_BLOCK: &str = "agents";
const AGENT_NODE: &str = "agent";

/// The fields an `agent` entry may set.
const COMMAND_FIELD: &str = "command";
const RESUME_FIELD: &str = "resume";

// ========================================================================
// Functions
// ========================================================================

/// The `agent` entries of `text`'s `agents` block, and what to say of those
/// that cannot be read, which are left out. Text that is not KDL gives
/// none, for the rest of the reading to say what is wrong with it.
pub(super) fn read_agents(text: &str) -> (Vec<AgentSetting>, Vec<String>) {
    let Ok(document) = KdlDocument::parse(text) else {
        return (Vec::new(), Vec::new());
    };
    let mut settings = Vec::new();
    let mut slips = Vec::new();
    let nodes = document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == AGENTS_BLOCK)
        .flat_map(|block| block.children().map(KdlDocument::nodes).unwrap_or_default())
        .filter(|node| node.name().value() == AGENT_NODE);
    for node in nodes {
        match read_agent(node) {
            Ok(setting) => settings.push(setting),
            Err(slip) => slips.push(slip),
        }
    }
    (settings, slips)
}

/// Every agent in `base`, with `written` taken over those it names and the
/// new ones it gives added, those it names first in its order.
pub(super) fn merge_agents(base: &[AgentKind], written: &[AgentSetting]) -> Vec<AgentKind> {
    let mut agents: Vec<AgentKind> = written
        .iter()
        .map(|setting| {
            let mut agent = base
                .iter()
                .find(|agent| agent.name == setting.name)
                .cloned()
                .unwrap_or_else(|| AgentKind {
                    command: setting.name.clone(),
                    name: setting.name.clone(),
                    resume: None,
                });
            if let Some(command) = &setting.command {
                agent.command = command.clone();
            }
            if let Some(resume) = &setting.resume {
                agent.resume = Some(resume.clone());
            }
            agent
        })
        .collect();
    let named = |agent: &&AgentKind| written.iter().any(|setting| setting.name == agent.name);
    agents.extend(base.iter().filter(|agent| !named(agent)).cloned());
    agents
}

/// The `agents` block for `stop_on_quit`, `worktrees`, and `written`, or
/// nothing when all are as built in, so a file that never named an agent
/// gets no block.
pub(super) fn write_agents(
    stop_on_quit: bool,
    worktrees: Option<&str>,
    written: &[AgentSetting],
) -> String {
    if !stop_on_quit && worktrees.is_none() && written.is_empty() {
        return String::new();
    }
    let mut out = format!("{AGENTS_BLOCK} {{\n");
    if stop_on_quit {
        out.push_str("    stop-on-quit #true\n");
    }
    if let Some(worktrees) = worktrees {
        out.push_str(&format!("    worktrees {}\n", kdl_string(worktrees)));
    }
    for setting in written {
        let mut fields = String::new();
        for (field, value) in [
            (COMMAND_FIELD, &setting.command),
            (RESUME_FIELD, &setting.resume),
        ] {
            if let Some(value) = value {
                fields.push_str(&format!("        {field} {}\n", kdl_string(value)));
            }
        }
        let name = kdl_string(&setting.name);
        match fields.is_empty() {
            true => out.push_str(&format!("    {AGENT_NODE} {name}\n")),
            false => out.push_str(&format!("    {AGENT_NODE} {name} {{\n{fields}    }}\n")),
        }
    }
    out.push_str("}\n");
    out
}

/// One `agent` node read, or what to say of why it cannot be.
fn read_agent(node: &KdlNode) -> Result<AgentSetting, String> {
    let name = only_text(node).ok_or_else(name_slip)?;
    let mut setting = AgentSetting {
        name: name.clone(),
        ..AgentSetting::default()
    };
    for child in node.children().map(KdlDocument::nodes).unwrap_or_default() {
        let field = child.name().value();
        let value = only_text(child)
            .ok_or_else(|| format!("agent {name}'s {field} takes one line in quotes"))?;
        match field {
            COMMAND_FIELD => setting.command = Some(value),
            RESUME_FIELD => setting.resume = Some(value),
            _ => return Err(format!("agent {name} has no setting `{field}`")),
        }
    }
    Ok(setting)
}

/// The one piece of text `node` gives, if that is all it gives.
fn only_text(node: &KdlNode) -> Option<String> {
    match node.entries() {
        [entry] if entry.name().is_none() => entry
            .value()
            .as_string()
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string),
        _ => None,
    }
}

/// What to say of an `agent` node without its name.
fn name_slip() -> String {
    "agents' agent takes its name: agent \"name\"".to_string()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(name: &str, command: &str, resume: Option<&str>) -> AgentKind {
        AgentKind {
            command: command.to_string(),
            name: name.to_string(),
            resume: resume.map(str::to_string),
        }
    }

    #[test]
    fn test_entries_change_built_in_agents_add_new_ones_and_come_first() {
        let text = r#"agents {
    agent "aider" { command "aider --no-auto-commits"; }
    agent "claude" { resume "claude -c"; }
    agent "broken" { colour "red"; }
    agent
}"#;
        let (written, slips) = read_agents(text);
        assert_eq!(slips.len(), 2, "{slips:?}");
        let base = [
            kind("claude", "claude", Some("claude --continue")),
            kind("gemini", "gemini", None),
        ];
        let merged = merge_agents(&base, &written);
        assert_eq!(
            merged,
            [
                kind("aider", "aider --no-auto-commits", None),
                kind("claude", "claude", Some("claude -c")),
                kind("gemini", "gemini", None),
            ]
        );
    }

    #[test]
    fn test_the_block_written_reads_back_as_written_and_is_left_out_when_unset() {
        assert_eq!(write_agents(false, None, &[]), "", "nothing to say");
        let written = vec![
            AgentSetting {
                command: Some("aider".to_string()),
                name: "aider".to_string(),
                resume: None,
            },
            AgentSetting {
                name: "claude".to_string(),
                ..AgentSetting::default()
            },
        ];
        let text = write_agents(true, Some("../wt/{repo}"), &written);
        assert!(text.contains("stop-on-quit #true"), "{text}");
        assert_eq!(read_agents(&text), (written, Vec::new()), "{text}");
        assert!(text.contains("worktrees \"../wt/{repo}\""), "{text}");
    }
}
