//! Writing a [`Config`] back as `settings.kdl` text, which reads back as
//! the same settings.

use winter_render::MenuStyle;

use super::schema::{controls_side_as_value, cursor_shape_as_value, kdl_bool, kdl_string};
use super::state::config_dir;
use super::Config;

// ========================================================================
// Config
// ========================================================================

impl Config {
    /// Serialize the appearance/behavior settings as `settings.kdl` text. The
    /// output round-trips through [`Self::parse`]. Keybindings live in a separate
    /// `keybindings.kdl` and are intentionally not written here.
    pub fn to_kdl(&self) -> String {
        let menu_style = match self.menu_style {
            MenuStyle::Classic => "classic",
            MenuStyle::Modern => "modern",
        };

        let mut out = String::new();
        out.push_str(&format!("theme {}\n", kdl_string(self.theme.as_value())));
        out.push_str(&format!("scroll-margin {}\n", self.scroll_margin));
        if let Some(n) = self.scrollback_lines {
            out.push_str(&format!("scrollback-lines {n}\n"));
        }
        if let Some(shell) = &self.shell {
            out.push_str(&format!("shell {}\n", kdl_string(shell)));
        }
        if let Some(shell) = &self.shell_windows {
            out.push_str(&format!("shell-windows {}\n", kdl_string(shell)));
        }
        if let Some(shell) = &self.shell_linux {
            out.push_str(&format!("shell-linux {}\n", kdl_string(shell)));
        }
        if let Some(shell) = &self.shell_macos {
            out.push_str(&format!("shell-macos {}\n", kdl_string(shell)));
        }
        if let Some(font) = &self.font_family {
            out.push_str(&format!("font {}\n", kdl_string(font)));
        }
        out.push_str(&format!("font-size {}\n", self.font_size));
        if let Some(font_weight) = &self.font_weight {
            out.push_str(&format!("font-weight {}\n", kdl_string(font_weight)));
        }
        if let Some(bold_weight) = &self.font_weight_bold {
            out.push_str(&format!("font-weight-bold {}\n", kdl_string(bold_weight)));
        }
        out.push_str(&format!("opacity {}\n", self.opacity));
        out.push_str(&format!("menu-style {}\n", kdl_string(menu_style)));
        out.push_str(&format!(
            "palette-match-underline {}\n",
            kdl_bool(self.palette_match_underline)
        ));
        out.push_str(&format!("pane-border-width {}\n", self.pane_border_width));
        out.push_str(&format!("popup-input-height {}\n", self.popup_input_height));
        out.push_str(&format!("ligatures {}\n", kdl_bool(self.ligatures)));
        out.push_str(&format!(
            "clipboard-read {}\n",
            kdl_bool(self.clipboard_read)
        ));
        out.push_str(&format!(
            "paste-on-right-click {}\n",
            kdl_bool(self.paste_on_right_click)
        ));
        out.push_str(&format!(
            "prompt-edit-bindings {}\n",
            kdl_string(self.prompt_edit_bindings.as_value())
        ));
        out.push_str(&format!(
            "rainbow-parens {}\n",
            kdl_bool(self.rainbow_parens)
        ));
        out.push_str(&format!(
            "sentence-highlight {}\n",
            kdl_bool(self.sentence_highlight)
        ));
        out.push_str(&format!("url-underline {}\n", kdl_bool(self.url_underline)));
        out.push_str(&format!("wrap-indent {}\n", kdl_bool(self.wrap_indent)));
        out.push_str(&format!("wrap-words {}\n", kdl_bool(self.wrap_words)));
        out.push_str(&format!("dim-inactive {}\n", kdl_bool(self.dim_inactive)));
        out.push_str(&format!(
            "restore-session {}\n",
            kdl_bool(self.restore_session)
        ));
        out.push_str(&format!(
            "title-bar-style {}\n",
            kdl_string(self.title_bar_style.as_value())
        ));
        out.push_str(&format!(
            "gutter-side {}\neditor-wrap-indent {}\neditor-markup-markers {}\n",
            kdl_string(self.gutter_side.as_value()),
            kdl_string(self.editor_wrap_indent.as_value()),
            kdl_string(self.editor_markup_markers.as_value())
        ));
        out.push_str(&format!(
            "window-controls-side {}\n",
            kdl_string(controls_side_as_value(self.window_controls_side))
        ));
        out.push_str(&format!(
            "window-title-template {}\n",
            kdl_string(&self.window_title_template)
        ));
        out.push_str("todo-keywords");
        for keyword in &self.todo_keywords {
            out.push_str(&format!(" {}", kdl_string(keyword)));
        }
        out.push('\n');
        out.push_str(&self.cursor_kdl());
        out.push_str(&self.status_bar_kdl());
        out.push_str(&self.notifications_kdl());
        out.push_str(&self.compile_kdl());
        out.push_str(&self.completion_kdl());
        out.push_str(&self.lsp_kdl());
        out.push_str(&self.copilot_kdl());
        if let Some(colors) = self.colors.to_kdl() {
            out.push_str(&colors);
        }
        out
    }

    /// Write [`Self::to_kdl`] to `settings.kdl`, creating the config directory if
    /// needed. Keybindings are left untouched.
    pub fn save(&self) -> std::io::Result<()> {
        crate::persist::write_atomic(&config_dir().join("settings.kdl"), self.to_kdl())
    }

    /// The `cursor` block, always emitted so every per-mode shape round-trips.
    fn cursor_kdl(&self) -> String {
        let c = &self.cursor;
        let mut out = String::from("cursor {\n");
        out.push_str(&format!("    blink {}\n", kdl_bool(c.blink)));
        out.push_str(&format!("    insert {}\n", kdl_string(cursor_shape_as_value(c.insert))));
        out.push_str(&format!("    normal {}\n", kdl_string(cursor_shape_as_value(c.normal))));
        out.push_str(&format!("    visual {}\n", kdl_string(cursor_shape_as_value(c.visual))));
        out.push_str(&format!(
            "    hide-in-inactive {}\n",
            kdl_bool(c.hide_in_inactive)
        ));
        out.push_str("}\n");
        out
    }

    /// The `notifications` block, always emitted so every flag round-trips.
    fn notifications_kdl(&self) -> String {
        format!(
            "notifications {{\n    toasts {}\n}}\n",
            kdl_bool(self.notifications.toasts)
        )
    }

    /// The `completion` block, always emitted so every setting round-trips.
    fn completion_kdl(&self) -> String {
        let mut out = format!(
            "completion {{\n    auto {}\n    delay-ms {}\n    min-chars {}\n",
            kdl_bool(self.completion.auto),
            self.completion.delay.as_millis(),
            self.completion.min_chars
        );
        if !self.completion.backends.is_empty() {
            out.push_str("    backends {\n");
            for (language, names) in &self.completion.backends {
                let names: Vec<String> = names.iter().map(|name| kdl_string(name)).collect();
                out.push_str(&format!("        {language} {}\n", names.join(" ")));
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
        out
    }

    /// The `copilot` block, always emitted so every setting round-trips.
    fn copilot_kdl(&self) -> String {
        let copilot = &self.copilot;
        format!(
            "copilot {{\n    enable {}\n    completions {}\n    next-edit {}\n    server-path {}\n}}\n",
            kdl_bool(copilot.enable),
            kdl_bool(copilot.completions),
            kdl_bool(copilot.next_edit),
            kdl_string(&copilot.server_path)
        )
    }

    /// The `lsp` block, always emitted so every setting round-trips.
    fn lsp_kdl(&self) -> String {
        let names = |names: &[String]| -> String {
            names
                .iter()
                .map(|name| kdl_string(name))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut out = String::from("lsp {\n");
        if !self.lsp.auto_start.is_empty() {
            out.push_str(&format!("    auto-start {}\n", names(&self.lsp.auto_start)));
        }
        if !self.lsp.servers.is_empty() {
            out.push_str("    servers {\n");
            for (language, command) in &self.lsp.servers {
                out.push_str(&format!("        {language} {}\n", names(command)));
            }
            out.push_str("    }\n");
        }
        out.push_str("}\n");
        out
    }

    /// The `compile` block, always emitted so every setting round-trips.
    fn compile_kdl(&self) -> String {
        format!(
            "compile {{\n    dock {}\n    jump-to-error {}\n    latex-command {}\n    latex-on-save {}\n}}\n",
            kdl_bool(self.compile.dock),
            kdl_bool(self.compile.jump_to_error),
            kdl_string(&self.compile.latex_command),
            kdl_bool(self.compile.latex_on_save)
        )
    }

    /// The `status-bar` block, always emitted so every flag round-trips.
    fn status_bar_kdl(&self) -> String {
        let sb = &self.status_bar;
        let mut out = String::from("status-bar {\n");
        out.push_str(&format!("    show {}\n", kdl_bool(sb.enabled)));
        out.push_str(&format!("    show-mode {}\n", kdl_bool(sb.show_mode)));
        out.push_str("    segments");
        for segment in &sb.segments {
            out.push_str(&format!(" {}", kdl_string(segment.name())));
        }
        out.push('\n');
        // Left out when unset, since a bare `priority` would read as an
        // empty list, which is the same as none.
        if !sb.priority.is_empty() {
            out.push_str("    priority");
            for segment in &sb.priority {
                out.push_str(&format!(" {}", kdl_string(segment.name())));
            }
            out.push('\n');
        }
        out.push_str("}\n");
        out
    }
}
