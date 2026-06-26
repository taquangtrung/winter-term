//! Where a terminal's commands run: this machine, a host reached over ssh, or
//! a container, read from the command line of the program in the foreground.

// ========================================================================
// Constants
// ========================================================================

/// The ssh options that take a value as the next argument, so the value is
/// never read as the destination.
const SSH_VALUE_FLAGS: &str = "BbcDEeFIiJLlmOoPpQRSWw";

/// The scheme of ssh's URI form of a destination, `ssh://user@host:port`.
const SSH_SCHEME: &str = "ssh://";

/// The programs that run containers, whose command lines read alike.
const CONTAINER_PROGRAMS: [&str; 2] = ["docker", "podman"];

/// The container subcommands that put a shell inside a running container,
/// naming it as their first argument.
const CONTAINER_ENTRY_COMMANDS: [&str; 2] = ["attach", "exec"];

/// The subcommand grouping the container commands (`docker container exec`).
const CONTAINER_GROUP: &str = "container";

/// The subcommand grouping the compose commands, whose `exec` and `run` name
/// a service rather than a container.
const COMPOSE_GROUP: &str = "compose";

/// The compose subcommands that put a shell inside a service.
const COMPOSE_ENTRY_COMMANDS: [&str; 2] = ["exec", "run"];

/// The container options, at any level, that take a value as the next
/// argument, so the value is never read as a subcommand or a name.
const CONTAINER_VALUE_FLAGS: [&str; 18] = [
    "--config",
    "--context",
    "--detach-keys",
    "--env",
    "--env-file",
    "--file",
    "--host",
    "--log-level",
    "--project-name",
    "--user",
    "--workdir",
    "-H",
    "-c",
    "-e",
    "-f",
    "-p",
    "-u",
    "-w",
];

// ========================================================================
// Data Structures
// ========================================================================

/// Where a terminal's commands run, by the name the status bar shows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Host {
    /// Inside a container: the container's or the service's name, or the
    /// program's own when the command line names neither.
    Container(String),
    /// This machine.
    Local,
    /// A host reached over ssh, by the name it was given.
    Ssh(String),
}

// ========================================================================
// Free functions
// ========================================================================

/// Where the program run by `argv` puts its terminal: a remote host for
/// `ssh`, a container for `docker exec` and its kin, and this machine for
/// anything else.
pub fn host_of_command(argv: &[String]) -> Host {
    let Some((program, args)) = argv.split_first() else {
        return Host::Local;
    };
    let name = program.rsplit('/').next().unwrap_or(program);
    if name == "ssh" {
        return ssh_destination(args).map_or(Host::Local, Host::Ssh);
    }
    if CONTAINER_PROGRAMS.contains(&name) {
        return container_of(args).map_or(Host::Local, |container| {
            Host::Container(container.unwrap_or_else(|| name.to_string()))
        });
    }
    Host::Local
}

/// The host an ssh command line connects to, without its user: the first
/// argument that is not an option or an option's value.
fn ssh_destination(args: &[String]) -> Option<String> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args.next().map(|arg| bare_host(arg));
        }
        let Some(flags) = arg.strip_prefix('-') else {
            return Some(bare_host(arg));
        };
        // Flags bundle (`-tt`, `-p22`): the first one taking a value takes
        // the rest of the argument, or the next one when nothing is left.
        let value_at = flags.find(|flag| SSH_VALUE_FLAGS.contains(flag));
        if value_at.is_some_and(|at| at + 1 == flags.len()) {
            args.next();
        }
    }
    None
}

/// `destination` without its scheme, user, and port: `ssh://me@box:22` is
/// `box`.
fn bare_host(destination: &str) -> String {
    let uri = destination.strip_prefix(SSH_SCHEME);
    let destination = uri.unwrap_or(destination);
    let host = destination.rsplit('@').next().unwrap_or(destination);
    // Only the URI form carries a port; a bare destination's colon is part
    // of the name ssh is given.
    match uri {
        Some(_) => host.split(':').next().unwrap_or(host).to_string(),
        None => host.to_string(),
    }
}

/// What a container command line enters: `Some` with the container's or
/// service's name when it names one, `Some(None)` for a container it does
/// not name (`docker run`), and `None` when it enters none at all.
fn container_of(args: &[String]) -> Option<Option<String>> {
    let mut words = positionals(args);
    let mut command = words.next()?;
    if command == CONTAINER_GROUP {
        command = words.next()?;
    }
    if command == COMPOSE_GROUP {
        let command = words.next()?;
        return COMPOSE_ENTRY_COMMANDS
            .contains(&command)
            .then(|| words.next().map(str::to_string));
    }
    if command == "run" {
        return Some(named(args));
    }
    CONTAINER_ENTRY_COMMANDS
        .contains(&command)
        .then(|| words.next().map(str::to_string))
}

/// The arguments that are neither options nor options' values, in order.
fn positionals(args: &[String]) -> impl Iterator<Item = &str> {
    let mut args = args.iter().map(String::as_str);
    std::iter::from_fn(move || loop {
        let arg = args.next()?;
        if !arg.starts_with('-') {
            return Some(arg);
        }
        if CONTAINER_VALUE_FLAGS.contains(&arg) {
            args.next();
        }
    })
}

/// The name a `docker run` gives its container with `--name`, if any.
fn named(args: &[String]) -> Option<String> {
    args.iter()
        .enumerate()
        .find_map(|(at, arg)| match arg.strip_prefix("--name=") {
            Some(name) => Some(name.to_string()),
            None => (arg == "--name")
                .then(|| args.get(at + 1).cloned())
                .flatten(),
        })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn host(line: &str) -> Host {
        let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        host_of_command(&argv)
    }

    #[test]
    fn test_ssh_names_the_host_past_its_options_and_user() {
        assert_eq!(host("ssh box"), Host::Ssh("box".into()));
        assert_eq!(host("/usr/bin/ssh me@box"), Host::Ssh("box".into()));
        assert_eq!(
            host("ssh -p 2222 -i key -t box ls"),
            Host::Ssh("box".into())
        );
        assert_eq!(host("ssh -tt -p2222 box"), Host::Ssh("box".into()));
        assert_eq!(host("ssh -o Port=22 -- me@box"), Host::Ssh("box".into()));
        assert_eq!(host("ssh ssh://me@box:2222"), Host::Ssh("box".into()));
        assert_eq!(host("ssh -V"), Host::Local, "connects nowhere");
    }

    #[test]
    fn test_a_container_shell_names_the_container_past_its_options() {
        assert_eq!(
            host("docker exec -it -u root -w /app web bash"),
            Host::Container("web".into())
        );
        assert_eq!(
            host("docker --context prod container exec -e A=1 web sh"),
            Host::Container("web".into())
        );
        assert_eq!(host("podman attach db"), Host::Container("db".into()));
        assert_eq!(
            host("docker compose -f dev.yml exec api sh"),
            Host::Container("api".into())
        );
    }

    #[test]
    fn test_docker_run_is_named_by_name_or_else_by_the_program() {
        assert_eq!(
            host("docker run --rm -it --name scratch ubuntu"),
            Host::Container("scratch".into())
        );
        assert_eq!(
            host("docker run --name=scratch ubuntu"),
            Host::Container("scratch".into())
        );
        assert_eq!(
            host("docker run -it ubuntu"),
            Host::Container("docker".into())
        );
    }

    #[test]
    fn test_commands_that_enter_nothing_stay_local() {
        assert_eq!(host("docker ps"), Host::Local);
        assert_eq!(host("docker build -t x ."), Host::Local);
        assert_eq!(host("vim notes.md"), Host::Local);
        assert_eq!(host_of_command(&[]), Host::Local);
    }
}
