//! The mux server managed agents run under (see
//! `docs/design/agent-workspace.md`, "Lifecycle"): one is started when none
//! answers, and one of another version of Winter, which this window cannot
//! speak with, is restarted once the user says so. The new server brings
//! its sessions back, typing each agent's resume line where the old one
//! kept it; the agents of the repository the user is in are opened again,
//! and the line typed where it was not kept.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use winter_model::layout::TabId;
use winter_mux::client::{MuxClient, Probe};
use winter_mux::server::{default_socket_path, server_pid};

use super::super::App;
use super::super::editor::closing::PendingClose;
use super::super::notice::WindowNotice;
use super::land::Asked;
use super::record::Agent;
use super::{Begun, Done, Halt, MUX_WAIT, Start, keep_resume_line, list_agents, mux_sessions};

// ========================================================================
// Constants
// ========================================================================

/// How long a mux server started here has to start listening, and one
/// asked to end has to stop answering.
const SERVER_WAIT: Duration = Duration::from_secs(3);

/// How often the server is looked for meanwhile.
const SERVER_POLL: Duration = Duration::from_millis(50);

// ========================================================================
// App: the agents' mux server
// ========================================================================

impl App {
    /// The mux server answering is of another version of Winter, as `why`
    /// says: ask whether to restart it. Only the user can tell whether an
    /// agent under it is at work, since this window cannot ask the server.
    pub(in crate::window) fn ask_to_restart_server(&mut self, why: String) {
        self.runtime.agents.asked = Some(Asked::Restart(self.focused_start_dir()));
        let question = format!(
            "The mux server is from another version of Winter ({why}), so no agent can run \
             under it. Restart it? Every mux session ends, an agent at work too, and this \
             repository's agents start again with their resume lines."
        );
        self.ask_yes_no("Mux Server", "Restart", question, PendingClose::Agent);
    }

    /// End the mux server, start this version's, and open the agents of the
    /// repository `dir` is in again.
    pub(in crate::window) fn restart_mux_server(&mut self, dir: PathBuf) {
        self.on_agent_thread(move |inbox| {
            end_mux_server()?;
            // What the new server types itself, read before it starts.
            let resumed = winter_mux::persist::load_defs()
                .sessions
                .into_iter()
                .filter(|def| def.resume.is_some())
                .map(|def| def.name)
                .collect();
            ensure_mux_server(inbox)?;
            let sessions = mux_sessions().unwrap_or_default();
            Ok(Done::Resume(list_agents(&dir, &sessions)?, resumed))
        });
    }

    /// Open `agents` again after their server restarted. A session in
    /// `resumed` the server started again with its agent; another it
    /// brought back as a bare shell gets the resume line typed, and kept
    /// for the next restart; an agent with no session left starts again in
    /// its worktree.
    pub(in crate::window) fn resume_agents(
        &mut self,
        agents: Vec<Agent>,
        resumed: &[String],
        focused: TabId,
    ) {
        for agent in agents {
            let Some(kind) = self.agent_kind(&agent.kind) else {
                continue;
            };
            let line = kind.resume.unwrap_or(kind.command);
            if !agent.running {
                let start = Start {
                    branch: agent.branch,
                    how: Begun::Opened,
                    line: line.clone(),
                    resume: line,
                    worktree: agent.worktree,
                };
                self.start_agent(start, focused);
                continue;
            }
            let session = agent.session();
            self.new_mux_tab(&session);
            let tab = self.name_agent_tab(&session, &agent.branch);
            if let Some(tab) = tab.filter(|_| !resumed.contains(&session)) {
                self.type_agent_line(tab, &line);
                keep_resume_line(&session, &line);
            }
        }
        self.notify(WindowNotice::AgentDone(
            "restarted the mux server".to_string(),
        ));
        self.refresh_agents_view();
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Start the mux server agents run under when none answers, telling the
/// window its process to reap, and wait for it to listen. A server of
/// another version answers but cannot be spoken with: the window is asked
/// to restart it.
pub(super) fn ensure_mux_server(inbox: &Sender<Done>) -> Result<(), Halt> {
    let socket = default_socket_path();
    match MuxClient::probe(&socket, MUX_WAIT) {
        Probe::Absent => {}
        Probe::Mismatched(why) => return Err(Halt::Outdated(why)),
        Probe::Paired => return Ok(()),
    }
    let program = std::env::current_exe()
        .map_err(|error| format!("cannot find Winter to start the mux server: {error}"))?;
    let server = Command::new(program)
        .args(["mux", "serve"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not start the mux server: {error}"))?;
    let _ = inbox.send(Done::ServerStarted(server.id()));
    let deadline = Instant::now() + SERVER_WAIT;
    while Instant::now() < deadline {
        if MuxClient::probe(&socket, MUX_WAIT) == Probe::Paired {
            return Ok(());
        }
        thread::sleep(SERVER_POLL);
    }
    Err(Halt::Failed(
        "the mux server did not start listening".to_string(),
    ))
}

/// End the mux server serving now, by the process id it wrote on starting,
/// and wait until it no longer answers.
fn end_mux_server() -> Result<(), String> {
    let socket = default_socket_path();
    let pid = server_pid(&socket).ok_or_else(|| {
        "the mux server is too old to say which process it is: end its `winter mux serve` \
         yourself, then try again"
            .to_string()
    })?;
    signal_server(pid)?;
    let deadline = Instant::now() + SERVER_WAIT;
    while MuxClient::probe(&socket, MUX_WAIT) != Probe::Absent {
        if Instant::now() >= deadline {
            return Err("the mux server did not end".to_string());
        }
        thread::sleep(SERVER_POLL);
    }
    Ok(())
}

/// Ask the server process `pid` to end. It runs in Winter's process group,
/// so it alone is signalled.
#[cfg(unix)]
fn signal_server(pid: u32) -> Result<(), String> {
    let pid = libc::pid_t::try_from(pid).map_err(|_| format!("{pid} names no process"))?;
    winter_base::process::signal_process(pid, libc::SIGTERM);
    Ok(())
}

/// Winter ends no process by its id here.
#[cfg(not(unix))]
fn signal_server(_pid: u32) -> Result<(), String> {
    Err("Winter cannot end the mux server here: end it yourself, then try again".to_string())
}
