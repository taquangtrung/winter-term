//! The window-title template and the variables it expands.

// ========================================================================
// Window title
// ========================================================================

/// The window title template used when `window-title-template` is unset (or set
/// to empty). `{{ title }}` is the active tab's resolved title, so the default
/// keeps the long-standing `"Winter - <title>"` look.
pub(crate) const DEFAULT_WINDOW_TITLE_TEMPLATE: &str = "Winter - {{ title }}";
/// Values the `window-title-template` placeholders expand to, describing the
/// app running in the active pane. Fields default to empty when the running
/// program provides none (e.g. `app_name` while sitting at a shell prompt).
pub struct WindowTitleVars {
    /// The active tab's resolved title — the OSC 0/2 title set by the running
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
}
/// Expand the `{{ name }}` placeholders of a window-title template
/// (`window-title-template`). Surrounding whitespace inside the braces is
/// allowed (`{{title}}`, `{{ title }}`); an unknown name is left as literal
/// text so a typo stays visible instead of silently vanishing.
pub fn expand_window_title_template(template: &str, vars: &WindowTitleVars) -> String {
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
            "app_name" => Some(&vars.app_name),
            "pane_title" => Some(&vars.pane_title),
            "cwd" => Some(&vars.cwd),
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
    use super::*;

    #[test]
    fn test_expand_window_title_template() {
        let vars = WindowTitleVars {
            title: "Terminal 1".into(),
            app_name: "butterfly".into(),
            pane_title: "butterfly — notes.md".into(),
            cwd: "~/N/notes".into(),
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
    }
}
