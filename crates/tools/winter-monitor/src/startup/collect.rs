//! Listing what starts on its own. This is the side-effecting half of the
//! Startup monitor: on Linux it asks systemd and reads the XDG autostart
//! entries, on macOS it reads the launch agents and daemons, on Windows it
//! asks PowerShell for the startup commands, and on Unix it reads the
//! crontab and the files every new shell reads. What it reads is turned
//! into entries by the parsers here, pure text in.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use winter_model::monitor::{StartupEntry, StartupKind, StartupScope};

use crate::run::run_text;

// ========================================================================
// Constants
// ========================================================================

/// Where the desktop's autostart entries live, the user's under their home,
/// the machine's in the system's configuration.
const USER_AUTOSTART: &str = ".config/autostart";
const SYSTEM_AUTOSTART: &str = "/etc/xdg/autostart";

/// Where macOS keeps launch agents and daemons, the user's under their home.
const USER_LAUNCH_AGENTS: &str = "Library/LaunchAgents";
const SYSTEM_LAUNCHD: [&str; 2] = ["/Library/LaunchAgents", "/Library/LaunchDaemons"];

/// The files new shells read, under the user's home, with the shell each
/// is for.
const SHELL_FILES: [(&str, &str); 7] = [
    (".profile", "login shell"),
    (".bash_profile", "bash login shell"),
    (".bashrc", "bash"),
    (".zshenv", "zsh"),
    (".zprofile", "zsh login shell"),
    (".zshrc", "interactive zsh"),
    (".config/fish/config.fish", "fish"),
];

/// The extension of a desktop entry and of a property list.
const DESKTOP_EXTENSION: &str = "desktop";
const PLIST_EXTENSION: &str = "plist";

/// The unit files systemd is asked about: services, the units that run
/// something.
const SYSTEMD_TYPE: &str = "--type=service";

/// The PowerShell that lists Windows's startup commands as JSON rows of
/// `[name, command, location, user]`.
const WINDOWS_SCRIPT: &str = "$ErrorActionPreference='SilentlyContinue'; \
ConvertTo-Json -Compress -Depth 3 @(Get-CimInstance Win32_StartupCommand | \
ForEach-Object { ,@($_.Name, $_.Command, $_.Location, $_.User) })";

// ========================================================================
// Functions
// ========================================================================

/// Everything that starts on its own, or why none of it could be read.
pub fn collect() -> Result<Vec<StartupEntry>, String> {
    if cfg!(windows) {
        let text = run_text(
            "powershell.exe",
            &["-NoProfile", "-NonInteractive", "-Command", WINDOWS_SCRIPT],
        )?;
        return Ok(parse_windows_json(&text));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut entries = Vec::new();
    if cfg!(target_os = "macos") {
        if let Some(home) = &home {
            entries.extend(launchd_entries(
                &home.join(USER_LAUNCH_AGENTS),
                StartupScope::User,
            ));
        }
        for dir in SYSTEM_LAUNCHD {
            entries.extend(launchd_entries(Path::new(dir), StartupScope::System));
        }
    } else {
        entries.extend(systemd_entries(StartupScope::User));
        entries.extend(systemd_entries(StartupScope::System));
        entries.extend(autostart_entries(home.as_deref()));
    }
    if let Ok(text) = run_text("crontab", &["-l"]) {
        entries.extend(parse_crontab(&text));
    }
    if let Some(home) = &home {
        entries.extend(shell_entries(home));
    }
    Ok(entries)
}

/// The services systemd starts in `scope`: the user's enabled and disabled,
/// so one turned off can be turned on again; the machine's enabled.
fn systemd_entries(scope: StartupScope) -> Vec<StartupEntry> {
    let (scope_flag, states) = match scope {
        StartupScope::User => (Some("--user"), "--state=enabled,disabled"),
        StartupScope::System => (None, "--state=enabled"),
    };
    let args = |rest: &[&str]| -> Vec<String> {
        scope_flag
            .into_iter()
            .chain(rest.iter().copied())
            .map(str::to_string)
            .collect()
    };
    let listing = args(&[
        "list-unit-files",
        SYSTEMD_TYPE,
        states,
        "--no-legend",
        "--no-pager",
    ]);
    let listing: Vec<&str> = listing.iter().map(String::as_str).collect();
    let Ok(text) = run_text("systemctl", &listing) else {
        return Vec::new();
    };
    let units = parse_unit_files(&text);
    if units.is_empty() {
        return Vec::new();
    }
    let mut show = args(&["show", "--property=Id,ExecStart,FragmentPath", "--no-pager"]);
    show.extend(units.iter().map(|(unit, _)| unit.clone()));
    let show: Vec<&str> = show.iter().map(String::as_str).collect();
    let shown = run_text("systemctl", &show)
        .map(|text| parse_systemctl_show(&text))
        .unwrap_or_default();
    units
        .into_iter()
        .map(|(unit, enabled)| {
            let (command, source) = shown.get(&unit).cloned().unwrap_or_default();
            StartupEntry {
                command,
                enabled,
                kind: StartupKind::Systemd,
                name: unit,
                scope,
                source,
            }
        })
        .collect()
}

/// The desktop's autostart entries: the user's, and the machine's that the
/// user has not overridden with one of the same name.
fn autostart_entries(home: Option<&Path>) -> Vec<StartupEntry> {
    let mut entries: Vec<StartupEntry> = Vec::new();
    let dirs = home
        .map(|home| (home.join(USER_AUTOSTART), StartupScope::User))
        .into_iter()
        .chain([(PathBuf::from(SYSTEM_AUTOSTART), StartupScope::System)]);
    let mut seen = Vec::new();
    for (dir, scope) in dirs {
        for path in files_with(&dir, DESKTOP_EXTENSION) {
            let file = path.file_name().map(|name| name.to_owned());
            if seen.contains(&file) {
                continue;
            }
            seen.push(file);
            let Ok(text) = fs::read_to_string(&path) else {
                continue;
            };
            if let Some(mut entry) = parse_desktop_entry(&text, scope) {
                entry.source = Some(path);
                entries.push(entry);
            }
        }
    }
    entries
}

/// The launch agents or daemons in `dir`.
fn launchd_entries(dir: &Path, scope: StartupScope) -> Vec<StartupEntry> {
    files_with(dir, PLIST_EXTENSION)
        .into_iter()
        .filter_map(|path| {
            let mut entry = parse_plist(&fs::read_to_string(&path).ok()?, scope)?;
            entry.source = Some(path);
            Some(entry)
        })
        .collect()
}

/// The files under `home` a new shell reads, those there are.
fn shell_entries(home: &Path) -> Vec<StartupEntry> {
    SHELL_FILES
        .iter()
        .map(|(file, shell)| (home.join(file), *file, *shell))
        .filter(|(path, _, _)| path.is_file())
        .map(|(path, file, shell)| StartupEntry {
            command: format!("read by every new {shell}"),
            enabled: true,
            kind: StartupKind::Shell,
            name: file.to_string(),
            scope: StartupScope::User,
            source: Some(path),
        })
        .collect()
}

/// The files in `dir` with `extension`, sorted, none for a directory that
/// is not there.
fn files_with(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(listing) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = listing
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == extension))
        .collect();
    paths.sort();
    paths
}

/// `systemctl list-unit-files` rows as each unit and whether it is enabled.
pub fn parse_unit_files(raw: &str) -> Vec<(String, bool)> {
    raw.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let unit = fields.next()?;
            // A template (`foo@.service`) starts nothing until given a name.
            if unit.ends_with("@.service") {
                return None;
            }
            Some((unit.to_string(), fields.next()? == "enabled"))
        })
        .collect()
}

/// `systemctl show`'s blocks, a blank line between each, as each unit's
/// command and unit file.
pub fn parse_systemctl_show(raw: &str) -> HashMap<String, (String, Option<PathBuf>)> {
    let mut shown = HashMap::new();
    for block in raw.split("\n\n") {
        let mut id = None;
        let mut command = String::new();
        let mut source = None;
        for line in block.lines() {
            match line.split_once('=') {
                Some(("Id", value)) => id = Some(value.to_string()),
                Some(("ExecStart", value)) => command = exec_start_argv(value),
                Some(("FragmentPath", value)) if !value.is_empty() => {
                    source = Some(PathBuf::from(value));
                }
                _ => {}
            }
        }
        if let Some(id) = id {
            shown.insert(id, (command, source));
        }
    }
    shown
}

/// The command line in systemd's spelling of `ExecStart`
/// (`{ path=/usr/bin/x ; argv[]=/usr/bin/x --y ; … }`), or the value itself.
fn exec_start_argv(value: &str) -> String {
    value
        .split_once("argv[]=")
        .and_then(|(_, rest)| rest.split_once(" ;"))
        .map_or_else(
            || value.trim().to_string(),
            |(argv, _)| argv.trim().to_string(),
        )
}

/// An XDG desktop entry as what it starts, `None` for one that starts
/// nothing.
pub fn parse_desktop_entry(raw: &str, scope: StartupScope) -> Option<StartupEntry> {
    let mut fields: HashMap<&str, &str> = HashMap::new();
    let mut in_entry = false;
    for line in raw.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry && let Some((key, value)) = line.split_once('=') {
            fields.entry(key.trim()).or_insert(value.trim());
        }
    }
    let command = fields.get("Exec")?.to_string();
    let off = |key: &str| fields.get(key).is_some_and(|value| *value == "true");
    let disabled = fields
        .get("X-GNOME-Autostart-enabled")
        .is_some_and(|value| *value == "false");
    Some(StartupEntry {
        command,
        enabled: !off("Hidden") && !disabled,
        kind: StartupKind::Autostart,
        name: fields.get("Name").unwrap_or(&"").to_string(),
        scope,
        source: None,
    })
}

/// A launchd property list as what it starts: its `Label`, and its
/// `ProgramArguments` or `Program`; off where `Disabled` is true.
pub fn parse_plist(raw: &str, scope: StartupScope) -> Option<StartupEntry> {
    let label = plist_string(raw, "Label")?;
    let command = plist_array(raw, "ProgramArguments")
        .map(|args| args.join(" "))
        .or_else(|| plist_string(raw, "Program"))
        .unwrap_or_default();
    let disabled = raw
        .split_once("<key>Disabled</key>")
        .is_some_and(|(_, rest)| rest.trim_start().starts_with("<true/>"));
    Some(StartupEntry {
        command,
        enabled: !disabled,
        kind: StartupKind::Launchd,
        name: label,
        scope,
        source: None,
    })
}

/// The `<string>` after `<key>key</key>` in a property list.
fn plist_string(raw: &str, key: &str) -> Option<String> {
    let (_, rest) = raw.split_once(&format!("<key>{key}</key>"))?;
    let rest = rest.trim_start().strip_prefix("<string>")?;
    Some(rest.split_once("</string>")?.0.to_string())
}

/// The `<string>`s of the `<array>` after `<key>key</key>`.
fn plist_array(raw: &str, key: &str) -> Option<Vec<String>> {
    let (_, rest) = raw.split_once(&format!("<key>{key}</key>"))?;
    let rest = rest.trim_start().strip_prefix("<array>")?;
    let (items, _) = rest.split_once("</array>")?;
    Some(
        items
            .split("<string>")
            .filter_map(|item| Some(item.split_once("</string>")?.0.to_string()))
            .collect(),
    )
}

/// A crontab's jobs, comments and variables left out, each named by when it
/// runs.
pub fn parse_crontab(raw: &str) -> Vec<StartupEntry> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let (when, command) = match line.strip_prefix('@') {
                Some(rest) => {
                    let (word, command) = rest.split_once(char::is_whitespace)?;
                    (format!("@{word}"), command)
                }
                None => {
                    let fields: Vec<&str> = line.splitn(6, char::is_whitespace).collect();
                    if fields.len() < 6 || fields[0].contains('=') {
                        return None;
                    }
                    (fields[..5].join(" "), fields[5])
                }
            };
            Some(StartupEntry {
                command: command.trim().to_string(),
                enabled: true,
                kind: StartupKind::Cron,
                name: when,
                scope: StartupScope::User,
                source: None,
            })
        })
        .collect()
}

/// The startup commands Windows lists: those under `HKLM` or for all users
/// are the machine's.
pub fn parse_windows_json(text: &str) -> Vec<StartupEntry> {
    let Ok(value) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}').trim())
    else {
        return Vec::new();
    };
    let rows = match &value {
        Value::Array(rows) => rows.clone(),
        Value::Null => Vec::new(),
        row => vec![row.clone()],
    };
    rows.iter()
        .filter_map(|row| {
            let field = |at: usize| row[at].as_str().unwrap_or_default().to_string();
            let location = field(2);
            let machine = location.starts_with("HKLM") || field(3).contains("All Users");
            Some(StartupEntry {
                command: field(1),
                enabled: true,
                kind: StartupKind::Autostart,
                name: Some(field(0)).filter(|name| !name.is_empty())?,
                scope: match machine {
                    true => StartupScope::System,
                    false => StartupScope::User,
                },
                source: None,
            })
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unit_files_say_which_are_enabled_and_leave_templates_out() {
        let raw = "pipewire.service enabled enabled\nsyncthing.service disabled enabled\ngetty@.service enabled enabled\n";
        assert_eq!(
            parse_unit_files(raw),
            [
                ("pipewire.service".to_string(), true),
                ("syncthing.service".to_string(), false)
            ]
        );
    }

    #[test]
    fn test_systemctl_show_gives_each_unit_its_command_line_and_file() {
        let raw = "Id=pipewire.service\nExecStart={ path=/usr/bin/pipewire ; argv[]=/usr/bin/pipewire -v ; ignore_errors=no ; start_time=[n/a] }\nFragmentPath=/usr/lib/systemd/user/pipewire.service\n\nId=empty.service\nExecStart=\nFragmentPath=\n";
        let shown = parse_systemctl_show(raw);
        assert_eq!(
            shown["pipewire.service"],
            (
                "/usr/bin/pipewire -v".to_string(),
                Some(PathBuf::from("/usr/lib/systemd/user/pipewire.service"))
            )
        );
        assert_eq!(shown["empty.service"], (String::new(), None));
    }

    #[test]
    fn test_a_desktop_entry_is_off_when_hidden_or_turned_off_for_gnome() {
        let on = "[Desktop Entry]\nName=Syncthing\nExec=syncthing serve\n[Desktop Action x]\nExec=other\n";
        let entry = parse_desktop_entry(on, StartupScope::User).unwrap();
        assert_eq!(
            (entry.name.as_str(), entry.command.as_str()),
            ("Syncthing", "syncthing serve")
        );
        assert!(entry.enabled);
        let hidden = format!("{on}\n[Desktop Entry]\nHidden=true\n");
        assert!(
            !parse_desktop_entry(&hidden, StartupScope::User)
                .unwrap()
                .enabled
        );
        let gnome_off = "[Desktop Entry]\nExec=x\nX-GNOME-Autostart-enabled=false\n";
        assert!(
            !parse_desktop_entry(gnome_off, StartupScope::User)
                .unwrap()
                .enabled
        );
        assert!(
            parse_desktop_entry("[Desktop Entry]\nName=Nothing\n", StartupScope::User).is_none()
        );
    }

    #[test]
    fn test_a_plist_gives_its_label_and_arguments_and_whether_disabled() {
        let raw = "<plist><dict><key>Label</key><string>com.example.agent</string>\
                   <key>ProgramArguments</key><array><string>/usr/local/bin/agent</string>\
                   <string>--quiet</string></array><key>Disabled</key> <true/></dict></plist>";
        let entry = parse_plist(raw, StartupScope::User).unwrap();
        assert_eq!(entry.name, "com.example.agent");
        assert_eq!(entry.command, "/usr/local/bin/agent --quiet");
        assert!(!entry.enabled);
    }

    #[test]
    fn test_a_crontab_keeps_jobs_and_leaves_out_comments_and_variables() {
        let raw = "# m h dom mon dow command\nMAILTO=me@example.com\n@reboot ~/bin/sync\n*/5 * * * * /usr/bin/backup --fast\n";
        let entries = parse_crontab(raw);
        let jobs: Vec<(&str, &str)> = entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.command.as_str()))
            .collect();
        assert_eq!(
            jobs,
            [
                ("@reboot", "~/bin/sync"),
                ("*/5 * * * *", "/usr/bin/backup --fast")
            ]
        );
    }

    #[test]
    fn test_windows_startup_commands_under_hklm_are_the_machine_s() {
        let raw = r#"[["OneDrive","C:\\OneDrive.exe /background","HKU\\S-1-5-21\\Run","PC\\me"],["Teams","Teams.exe","HKLM\\SOFTWARE\\Run","Public"]]"#;
        let entries = parse_windows_json(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].scope, StartupScope::User);
        assert_eq!(entries[1].scope, StartupScope::System);
    }
}
