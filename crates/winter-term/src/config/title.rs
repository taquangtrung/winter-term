//! The window-title template and the variables it expands.

// ========================================================================
// Window title
// ========================================================================

/// What sets a title's parts apart: a part a placeholder leaves empty drops
/// out with it, so `Notifications — Winter Term` has no dangling separator.
const PART_SEPARATOR: &str = " - ";
const EM_DASH_SEPARATOR: &str = " — ";
const DOUBLE_HYPHEN_SEPARATOR: &str = " -- ";
/// Values the `window-title-template` placeholders expand to, describing the
/// app running in the active pane. Fields default to empty when the running
/// program provides none (e.g. `app_name` while sitting at a shell prompt).
pub struct WindowTitleVars {
    /// The active tab's resolved title: the OSC 0/2 title set by the running
    /// app, else its process name and cwd, else `Terminal N`. Same string the
    /// tab strip shows.
    pub title: String,
    /// Name of the foreground process in the active pane (e.g. `butterfly`).
    pub app_name: String,
    /// The OSC 0/2 title exactly as the running app set it (cleaned of a
    /// `user@host:` prefix).
    pub pane_title: String,
    /// The active pane's working directory, abbreviated like the tab strip
    /// shows it (e.g. `~/W/a/winter-term`).
    pub cwd: String,
    /// What the active tab shows, by name: the file or directory's own name,
    /// the program running in a terminal, or an editor's kind where it shows
    /// no place on disk (e.g. `main.rs`, `zsh`, `Notifications`).
    pub name: String,
    /// The directory what the tab shows is in, home written `~` (e.g.
    /// `~/Workspace/apps/winter-term/src`); empty for an editor that shows
    /// no place on disk.
    pub location: String,
    /// The whole place the tab shows, home written `~`: the location and the
    /// name together (e.g. `~/Workspace/apps/winter-term/src/main.rs`).
    pub path: String,
}
/// Expand the `{{ name }}` placeholders of a window-title template
/// (`window-title-template`). Surrounding whitespace inside the braces is
/// allowed (`{{title}}`, `{{ title }}`); an unknown name is left as literal
/// text so a typo stays visible instead of silently vanishing. A part the
/// placeholders leave empty drops out with its separator (see
/// [`PART_SEPARATOR`]).
pub fn expand_window_title_template(template: &str, vars: &WindowTitleVars) -> String {
    let expanded = expand_placeholders(template, vars);
    let sep = if template.contains(EM_DASH_SEPARATOR) {
        EM_DASH_SEPARATOR
    } else if template.contains(DOUBLE_HYPHEN_SEPARATOR) {
        DOUBLE_HYPHEN_SEPARATOR
    } else {
        PART_SEPARATOR
    };
    let parts: Vec<&str> = expanded
        .split(sep)
        .filter(|part| !part.trim().is_empty())
        .collect();
    parts.join(sep)
}

/// `template` with every placeholder it names put in.
fn expand_placeholders(template: &str, vars: &WindowTitleVars) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            // An unclosed `{{` has no placeholder; keep it literally.
            out.push_str(&rest[start..]);
            return out;
        };
        let value = match after[..end].trim() {
            "title" => Some(&vars.title),
            "app_name" | "extension" | "extension_name" | "view" => Some(&vars.app_name),
            "pane_title" => Some(&vars.pane_title),
            "cwd" => Some(&vars.cwd),
            "location" => Some(&vars.location),
            "name" => Some(&vars.name),
            "path" => Some(&vars.path),
            _ => None,
        };
        match value {
            Some(v) => out.push_str(v),
            None => {
                out.push_str("{{");
                out.push_str(&after[..end]);
                out.push_str("}}");
            }
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    out
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::config::Config;

    use super::*;

    #[test]
    fn test_expand_window_title_template() {
        let vars = WindowTitleVars {
            title: "Terminal 1".into(),
            app_name: "butterfly".into(),
            pane_title: "butterfly — notes.md".into(),
            cwd: "~/N/notes".into(),
            location: "~/Notes".into(),
            name: "notes.md".into(),
            path: "~/Notes/notes.md".into(),
        };
        // Every placeholder, with and without inner spaces.
        assert_eq!(
            expand_window_title_template("{{title}}", &vars),
            "Terminal 1"
        );
        assert_eq!(
            expand_window_title_template("{{ app_name }}: {{ pane_title }}", &vars),
            "butterfly: butterfly — notes.md"
        );
        assert_eq!(
            expand_window_title_template("{{ cwd }}", &vars),
            "~/N/notes"
        );
        // Unknown placeholders and unclosed braces stay literal.
        assert_eq!(
            expand_window_title_template("{{ bogus }}", &vars),
            "{{ bogus }}"
        );
        assert_eq!(expand_window_title_template("a {{ b", &vars), "a {{ b");
        // Text without placeholders passes through unchanged.
        assert_eq!(expand_window_title_template("Winter", &vars), "Winter");
        assert_eq!(
            expand_window_title_template(&Config::default().window_title_template, &vars),
            "notes.md — Winter Term"
        );
        assert_eq!(
            expand_window_title_template("{{ name }} - {{ location }} - Winter Term", &vars),
            "notes.md - ~/Notes - Winter Term"
        );
        assert_eq!(
            expand_window_title_template("Winter - {{ path }}", &vars),
            "Winter - ~/Notes/notes.md"
        );
    }

    #[test]
    fn test_a_part_left_empty_drops_out_with_its_separator() {
        let vars = WindowTitleVars {
            title: String::new(),
            app_name: String::new(),
            pane_title: String::new(),
            cwd: String::new(),
            location: String::new(),
            name: "Notifications".into(),
            path: String::new(),
        };
        assert_eq!(
            expand_window_title_template(&Config::default().window_title_template, &vars),
            "Notifications — Winter Term"
        );
        assert_eq!(
            expand_window_title_template("{{ name }} - {{ location }} - Winter Term", &vars),
            "Notifications - Winter Term"
        );
        let empty_vars = WindowTitleVars {
            title: String::new(),
            app_name: String::new(),
            pane_title: String::new(),
            cwd: String::new(),
            location: String::new(),
            name: String::new(),
            path: String::new(),
        };
        assert_eq!(
            expand_window_title_template(&Config::default().window_title_template, &empty_vars),
            "Winter Term"
        );
    }
}
