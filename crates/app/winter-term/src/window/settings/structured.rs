//! The settings page's rows for what `settings.kdl` holds as lists and
//! tables rather than single values: each platform's shell, the status
//! bar's segments, the completion back-ends of each language, the language
//! servers, and the colors set over the theme. Each row's key names what it
//! sets (`completion.backends.rust`, `lsp.server.gopls.command`,
//! `colors.syntax.comment`), and its value is read back by
//! [`App::apply_structured_setting`] the way every other row's is.

use crate::render::{ColorOverrides, ThemeRgb};
use winter_lsp::servers::Server;
use winter_model::settings_page::{ChoiceOption, SettingsField};

use crate::config::{completion_backends, status_segments_named, ServerField, StatusSegmentKind};
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The sections the rows are grouped under.
const TERMINAL_SECTION: &str = "Terminal";
const STATUS_SECTION: &str = "Status bar";
const BACKENDS_SECTION: &str = "Completion back-ends";
const SERVERS_SECTION: &str = "Language servers";
const COLORS_SECTION: &str = "Colors";

/// What each key starts with, by what it sets.
const BACKENDS_KEY: &str = "completion.backends.";
const PREFER_KEY: &str = "lsp.prefer.";
const SERVER_KEY: &str = "lsp.server.";
const COLOR_KEY: &str = "colors.";

/// The keys of the rows that stand alone.
const SHELL_WINDOWS: &str = "shell_windows";
const SHELL_LINUX: &str = "shell_linux";
const SHELL_MACOS: &str = "shell_macos";
const SEGMENTS: &str = "status.segments";
const PRIORITY: &str = "status.priority";
const AUTO_START: &str = "lsp.auto_start";
const NEW_SERVER: &str = "lsp.new_server";

/// What a server's key ends with, by the field it sets.
const COMMAND_FIELD: &str = "command";
const LANGUAGES_FIELD: &str = "languages";
const EXTENSIONS_FIELD: &str = "extensions";
const ROOTS_FIELD: &str = "roots";
const INSTALL_FIELD: &str = "install";

/// The rows the groups that join a section go after.
const SHELL_ANCHOR: &str = "shell";
const STATUS_ANCHOR: &str = "status.show_mode";
const COMPLETION_ANCHOR: &str = "completion.min_chars";

/// What separates one install command from the next in an install row.
const COMMAND_SEPARATOR: char = ';';

// ========================================================================
// App: structured settings
// ========================================================================

impl App {
    /// Put the rows for the settings held as lists and tables among
    /// `fields`: each group after the row of its section it follows, so a
    /// section stays in one piece, and the sections of their own last.
    pub(in crate::window) fn add_structured_setting_fields(&self, fields: &mut Vec<SettingsField>) {
        let groups = [
            (SHELL_ANCHOR, self.shell_fields()),
            (STATUS_ANCHOR, self.status_fields()),
            (COMPLETION_ANCHOR, self.backend_fields()),
        ];
        for (anchor, group) in groups {
            let at = fields
                .iter()
                .position(|field| field.key == anchor)
                .map_or(fields.len(), |at| at + 1);
            fields.splice(at..at, group);
        }
        fields.extend(self.server_fields());
        fields.extend(self.color_fields());
    }

    /// Put the row `key`'s new `value` in force, where it is one of these
    /// rows: `None` when it is not, and otherwise whether the value could be
    /// used.
    pub(in crate::window) fn apply_structured_setting(
        &mut self,
        key: &str,
        value: &str,
    ) -> Option<bool> {
        let names = || -> Vec<String> { value.split_whitespace().map(str::to_string).collect() };
        let text = (!value.trim().is_empty()).then(|| value.trim().to_string());
        match key {
            SHELL_WINDOWS => self.config.shell_windows = text,
            SHELL_LINUX => self.config.shell_linux = text,
            SHELL_MACOS => self.config.shell_macos = text,
            SEGMENTS => self.config.status_bar.segments = status_segments_named(names()),
            PRIORITY => self.config.status_bar.priority = status_segments_named(names()),
            AUTO_START => self.config.lsp.auto_start = names(),
            NEW_SERVER => {
                let mut words = names().into_iter();
                // Nothing typed adds nothing.
                let Some(name) = words.next() else {
                    return Some(true);
                };
                self.config.lsp.add_server(&name, words.collect());
                self.host.lsp.forget_plans();
            }
            _ => return self.apply_keyed_setting(key, value),
        }
        self.dirty = true;
        Some(true)
    }

    /// Put in force a row whose key names what it sets after its prefix.
    fn apply_keyed_setting(&mut self, key: &str, value: &str) -> Option<bool> {
        let names: Vec<String> = value.split_whitespace().map(str::to_string).collect();
        if let Some(language) = key.strip_prefix(BACKENDS_KEY) {
            match names.is_empty() {
                true => self.config.completion.backends.remove(language),
                false => self
                    .config
                    .completion
                    .backends
                    .insert(language.to_string(), names),
            };
            return Some(true);
        }
        if let Some(language) = key.strip_prefix(PREFER_KEY) {
            let serves = self
                .config
                .lsp
                .servers
                .iter()
                .any(|server| server.name == value && server.serves(language));
            if serves {
                self.config.lsp.prefer_server(value);
                self.host.lsp.forget_plans();
            }
            return Some(serves);
        }
        if let Some(color) = key.strip_prefix(COLOR_KEY) {
            let rgb = match value.trim() {
                "" => None,
                hex => Some(ThemeRgb::parse_hex(hex)?),
            };
            let known = self.config.colors.set_color(color, rgb);
            self.rebuild_theme();
            return Some(known);
        }
        let (server, field) = key.strip_prefix(SERVER_KEY)?.rsplit_once('.')?;
        let list = (!names.is_empty()).then(|| names.clone());
        match field {
            COMMAND_FIELD => self.config.lsp.set_server_command(server, list),
            LANGUAGES_FIELD => {
                self.config
                    .lsp
                    .set_server_field(server, ServerField::Languages, list)
            }
            EXTENSIONS_FIELD => {
                self.config
                    .lsp
                    .set_server_field(server, ServerField::Extensions, list)
            }
            ROOTS_FIELD => self
                .config
                .lsp
                .set_server_field(server, ServerField::Roots, list),
            INSTALL_FIELD => {
                let commands = value
                    .split(COMMAND_SEPARATOR)
                    .map(|command| command.split_whitespace().map(str::to_string).collect())
                    .filter(|command: &Vec<String>| !command.is_empty())
                    .collect();
                self.config.lsp.set_server_install(server, commands);
            }
            _ => return None,
        }
        self.host.lsp.forget_plans();
        Some(true)
    }

    /// Each platform's shell, the one this platform runs among them.
    fn shell_fields(&self) -> Vec<SettingsField> {
        let shell = |key: &str, label: &str, value: &Option<String>| {
            SettingsField::text(key, label, value.clone().unwrap_or_default())
                .in_section(TERMINAL_SECTION)
        };
        vec![
            shell(SHELL_LINUX, "Shell on Linux", &self.config.shell_linux)
                .with_note("Empty runs the login shell"),
            shell(SHELL_MACOS, "Shell on macOS", &self.config.shell_macos),
            shell(
                SHELL_WINDOWS,
                "Shell on Windows",
                &self.config.shell_windows,
            ),
        ]
    }

    /// The status bar's segments, and the order they hold on in.
    fn status_fields(&self) -> Vec<SettingsField> {
        let choices: Vec<String> = StatusSegmentKind::defaults()
            .iter()
            .map(|kind| kind.name().to_string())
            .collect();
        let named = |kinds: &[StatusSegmentKind]| -> Vec<String> {
            kinds.iter().map(|kind| kind.name().to_string()).collect()
        };
        let bar = &self.config.status_bar;
        vec![
            SettingsField::names(SEGMENTS, "Segments", &named(&bar.segments), choices.clone())
                .in_section(STATUS_SECTION)
                .with_note("The right side's segments, in order from the left"),
            SettingsField::names(PRIORITY, "Kept longest", &named(&bar.priority), choices)
                .in_section(STATUS_SECTION)
                .with_note("When the bar is short of room, the first is kept longest"),
        ]
    }

    /// The back-ends each language asks, a row a language.
    fn backend_fields(&self) -> Vec<SettingsField> {
        let choices: Vec<String> = completion_backends()
            .iter()
            .map(|name| name.to_string())
            .collect();
        winter_text::known_languages()
            .into_iter()
            .enumerate()
            .map(|(at, language)| {
                let names = self
                    .config
                    .completion
                    .backends
                    .get(language)
                    .cloned()
                    .unwrap_or_default();
                let field = SettingsField::names(
                    &format!("{BACKENDS_KEY}{language}"),
                    language,
                    &names,
                    choices.clone(),
                )
                .in_section(BACKENDS_SECTION);
                match at {
                    0 => field.with_note("In the order asked; empty asks the language's own"),
                    _ => field,
                }
            })
            .collect()
    }

    /// The languages that start their server unasked, which server serves a
    /// language several do, and each server's command, those of the user's
    /// own with their every field, then a row to add one.
    fn server_fields(&self) -> Vec<SettingsField> {
        let servers = &self.config.lsp.servers;
        let mut languages: Vec<String> = servers
            .iter()
            .flat_map(|server| server.languages.iter().cloned())
            .collect();
        languages.sort_unstable();
        languages.dedup();
        let mut fields = vec![SettingsField::names(
            AUTO_START,
            "Start on opening",
            &self.config.lsp.auto_start,
            languages.clone(),
        )
        .in_section(SERVERS_SECTION)
        .with_note("The languages whose server starts as soon as a file of theirs opens")];
        for language in &languages {
            let serving: Vec<&Server> = servers
                .iter()
                .filter(|server| server.serves(language))
                .collect();
            if serving.len() < 2 {
                continue;
            }
            let options = serving
                .iter()
                .map(|server| ChoiceOption {
                    label: server.name.clone(),
                    value: server.name.clone(),
                })
                .collect();
            fields.push(
                SettingsField::choice(
                    &format!("{PREFER_KEY}{language}"),
                    &format!("Server for {language}"),
                    options,
                    0,
                )
                .in_section(SERVERS_SECTION),
            );
        }
        for server in servers {
            fields.extend(self.fields_of_server(server));
        }
        fields.push(
            SettingsField::text(NEW_SERVER, "Add a server", String::new())
                .in_section(SERVERS_SECTION)
                .with_note("Its name, then the languages it serves; kept on Esc"),
        );
        fields
    }

    /// Server `server`'s rows: its command, and every field of a server of
    /// the user's own.
    fn fields_of_server(&self, server: &Server) -> Vec<SettingsField> {
        let key = |field: &str| format!("{SERVER_KEY}{}.{field}", server.name);
        let written = self
            .config
            .lsp
            .written
            .iter()
            .find(|setting| setting.name == server.name);
        let command = written
            .and_then(|setting| setting.command.as_ref())
            .map(|command| command.join(" "))
            .unwrap_or_default();
        let installed = match server.is_installed() {
            true => "installed",
            false => "not installed",
        };
        let note = format!(
            "Serves {}; {installed}; runs {}",
            server.languages.join(", "),
            server.command.join(" ")
        );
        let mut fields = vec![
            SettingsField::text(&key(COMMAND_FIELD), &server.name, command)
                .in_section(SERVERS_SECTION)
                .with_note(&note),
        ];
        if !self.config.lsp.is_own_server(&server.name) {
            return fields;
        }
        let list = |field: &str, label: &str, values: &[String]| {
            SettingsField::names(&key(field), label, values, Vec::new()).in_section(SERVERS_SECTION)
        };
        let install: Vec<String> = server
            .install
            .iter()
            .map(|command| command.join(" "))
            .collect();
        fields.extend([
            list(
                LANGUAGES_FIELD,
                &format!("{} languages", server.name),
                &server.languages,
            ),
            list(
                EXTENSIONS_FIELD,
                &format!("{} file extensions", server.name),
                &server.extensions,
            ),
            list(
                ROOTS_FIELD,
                &format!("{} project markers", server.name),
                &server.roots,
            ),
            SettingsField::text(
                &key(INSTALL_FIELD),
                &format!("{} install", server.name),
                install.join(&format!("{COMMAND_SEPARATOR} ")),
            )
            .in_section(SERVERS_SECTION)
            .with_note("Commands tried in order, separated by ;"),
        ]);
        fields
    }

    /// Every color set over the theme, empty for the theme's own.
    fn color_fields(&self) -> Vec<SettingsField> {
        ColorOverrides::color_keys()
            .into_iter()
            .enumerate()
            .map(|(at, color)| {
                let value = self
                    .config
                    .colors
                    .color(&color)
                    .map(ThemeRgb::to_hex)
                    .unwrap_or_default();
                let field = SettingsField::color(&format!("{COLOR_KEY}{color}"), &color, value)
                    .in_section(COLORS_SECTION);
                match at {
                    0 => field
                        .with_note("#rrggbb, or #rrggbbaa to see through; empty keeps the theme's"),
                    _ => field,
                }
            })
            .collect()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_each_kind_of_row_puts_its_value_in_force() {
        let mut app = App::new();
        assert!(app.apply_setting("colors.syntax.comment", "#112233"));
        assert_eq!(
            app.config
                .colors
                .color("syntax.comment")
                .map(ThemeRgb::to_hex),
            Some("#112233".to_string())
        );
        assert!(!app.apply_setting("colors.background", "not a color"));
        assert!(app.apply_setting("lsp.prefer.python", "basedpyright"));
        let first = app
            .config
            .lsp
            .servers
            .iter()
            .find(|server| server.serves("python"));
        assert_eq!(
            first.map(|server| server.name.as_str()),
            Some("basedpyright")
        );
        assert!(app.apply_setting("completion.backends.rust", "snippets words"));
        assert_eq!(
            app.config.completion.backends["rust"],
            ["snippets", "words"]
        );
        assert!(app.apply_setting("completion.backends.rust", ""));
        assert!(
            !app.config.completion.backends.contains_key("rust"),
            "back to its own"
        );
        assert!(app.apply_setting("status.segments", "tool nonsense"));
        assert_eq!(app.config.status_bar.segments, [StatusSegmentKind::Editor]);
        assert!(app.apply_setting("lsp.new_server", "elixir-ls elixir"));
        let fields = app.build_settings_page().fields;
        assert!(fields
            .iter()
            .any(|field| field.key == "lsp.server.elixir-ls.languages"));
    }
}
