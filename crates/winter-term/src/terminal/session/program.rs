//! What to call the program in a terminal's foreground, from its command
//! line: the program itself, or the one a wrapper (`sudo`, `env`) runs, or
//! the script an interpreter (`python`, `node`) runs, which is what the
//! command was typed to do.

// ========================================================================
// Constants
// ========================================================================

/// The programs that run another named after their own options, with the
/// options of theirs that take a value as the next argument.
const WRAPPERS: &[(&str, &[&str])] = &[
    ("doas", &["-C", "-u"]),
    ("env", &["-C", "-S", "-u", "--chdir", "--split-string", "--unset"]),
    ("nice", &["-n", "--adjustment"]),
    ("nohup", &[]),
    ("sudo", &["-C", "-D", "-g", "-h", "-p", "-R", "-r", "-T", "-t", "-U", "-u"]),
    ("time", &["-f", "-o", "--format", "--output"]),
];

/// The interpreters named better by the script they run. A Python is any
/// program whose name starts with this (`python3`, `python3.12`).
const PYTHON: &str = "python";

/// The other interpreters named better by the script they run.
const INTERPRETERS: &[&str] = &["bash", "node", "perl", "ruby", "sh", "zsh"];

/// The interpreter option naming a module to run (`python -m http.server`).
const MODULE_FLAG: &str = "-m";

/// The interpreter options running code given inline, which has no name.
const INLINE_FLAGS: &[&str] = &["-c", "-e"];

/// What ends a wrapper's own options, the command following it.
const END_OF_OPTIONS: &str = "--";

// ========================================================================
// Functions
// ========================================================================

/// The name to show for a process run as `argv`: past any wrapper, the
/// program, or the script or module an interpreter runs, by the last part
/// of its path; the `-` a login shell is started with dropped. Else `comm`,
/// which the kernel cuts to 15 characters, for a process with no command
/// line or one that wrote its own over it (`sshd: ann`).
#[cfg(target_os = "linux")]
pub(super) fn program_name(argv: &[String], comm: &str) -> Option<String> {
    let fallback = || Some(comm.trim()).filter(|name| !name.is_empty()).map(str::to_string);
    let command = unwrapped(argv);
    let Some(first) = command.first() else {
        return fallback();
    };
    let name = program_of(first);
    if name.is_empty() || name.contains(' ') {
        return fallback();
    }
    let interprets = name.starts_with(PYTHON) || INTERPRETERS.contains(&name);
    match interprets {
        true => Some(script_of(&command[1..]).unwrap_or(name).to_string()),
        false => Some(name.to_string()),
    }
}

/// `argv` from the command its wrappers run on (`sudo -u ann vim x` from
/// `vim`), through any number of them. A wrapper with nothing to run (`sudo
/// -i`) is the command itself.
pub(super) fn unwrapped(argv: &[String]) -> &[String] {
    let mut at = 0;
    while let Some(first) = argv.get(at) {
        let name = program_of(first);
        let Some((_, valued)) = WRAPPERS.iter().find(|(wrapper, _)| *wrapper == name) else {
            break;
        };
        let command = past_wrapper_options(argv, at + 1, name, valued);
        if command >= argv.len() {
            break;
        }
        at = command;
    }
    &argv[at..]
}

/// The program a command line's first argument names: the last part of its
/// path, without the `-` a login shell is started with.
fn program_of(first: &str) -> &str {
    base_name(first).trim_start_matches('-')
}

/// Where the command wrapper `wrapper` runs starts in `argv`, its options
/// from `from` on passed over: `valued` ones with the value after them, and
/// `env`'s `NAME=value` settings.
fn past_wrapper_options(argv: &[String], from: usize, wrapper: &str, valued: &[&str]) -> usize {
    let mut at = from;
    while let Some(arg) = argv.get(at) {
        if arg == END_OF_OPTIONS {
            return at + 1;
        }
        let setting = wrapper == "env" && !arg.starts_with('-') && arg.contains('=');
        if !setting && !arg.starts_with('-') {
            break;
        }
        at += 1 + usize::from(valued.contains(&arg.as_str()));
    }
    at
}

/// The script or module an interpreter's arguments `args` run, by its
/// name: `None` for code given inline, or none at all (an interactive one).
#[cfg(target_os = "linux")]
fn script_of(args: &[String]) -> Option<&str> {
    let mut args = args.iter().map(String::as_str);
    while let Some(arg) = args.next() {
        if arg == MODULE_FLAG {
            return args.next();
        }
        if INLINE_FLAGS.contains(&arg) {
            return None;
        }
        if !arg.starts_with('-') {
            return Some(base_name(arg));
        }
    }
    None
}

/// The last part of `path`.
fn base_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_command_line_is_read_from_the_command_its_wrappers_run() {
        let argv = |line: &str| line.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(unwrapped(&argv("sudo -u ann ssh box")), argv("ssh box"));
        assert_eq!(unwrapped(&argv("env A=1 nice -n 5 make all")), argv("make all"));
        assert_eq!(unwrapped(&argv("sudo -i")), argv("sudo -i"), "nothing to run but itself");
        assert_eq!(unwrapped(&argv("vim notes")), argv("vim notes"));
    }

    #[cfg(target_os = "linux")]
    fn name(line: &str, comm: &str) -> Option<String> {
        let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        program_name(&argv, comm)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_process_is_named_by_its_whole_program_not_the_cut_comm() {
        assert_eq!(name("/usr/lib/rust-analyzer-proc-macro-srv --flag", "rust-analyzer-p\n").as_deref(), Some("rust-analyzer-proc-macro-srv"));
        assert_eq!(name("-zsh", "zsh\n").as_deref(), Some("zsh"), "a login shell's dash dropped");
        assert_eq!(program_name(&["sshd: ann".to_string()], "sshd\n").as_deref(), Some("sshd"), "a rewritten command line");
        assert_eq!(name("", "kworker\n").as_deref(), Some("kworker"), "no command line");
        assert_eq!(name("", ""), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_a_wrapper_is_named_by_the_command_it_runs() {
        assert_eq!(name("sudo vim /etc/hosts", "sudo").as_deref(), Some("vim"));
        assert_eq!(name("sudo -u ann -E vim notes", "sudo").as_deref(), Some("vim"), "an option's value is no command");
        assert_eq!(name("env FOO=1 BAR=2 cargo run", "env").as_deref(), Some("cargo"));
        assert_eq!(name("nice -n 10 time make", "nice").as_deref(), Some("make"), "wrappers inside wrappers");
        assert_eq!(name("sudo -- ls -l", "sudo").as_deref(), Some("ls"));
        assert_eq!(name("sudo -i", "sudo").as_deref(), Some("sudo"), "nothing to run but itself");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_an_interpreter_is_named_by_the_script_it_runs() {
        assert_eq!(name("python3 manage.py runserver", "python3").as_deref(), Some("manage.py"));
        assert_eq!(name("/usr/bin/python3.12 -u -m http.server 8000", "python3.12").as_deref(), Some("http.server"));
        assert_eq!(name("node /usr/lib/node_modules/npm/bin/npm-cli.js run dev", "node").as_deref(), Some("npm-cli.js"));
        assert_eq!(name("bash ./deploy.sh", "bash").as_deref(), Some("deploy.sh"));
        assert_eq!(name("python3 -c print(1)", "python3").as_deref(), Some("python3"), "inline code has no name");
        assert_eq!(name("python3", "python3").as_deref(), Some("python3"), "an interactive one");
        assert_eq!(name("sudo python3 app.py", "sudo").as_deref(), Some("app.py"));
    }
}
