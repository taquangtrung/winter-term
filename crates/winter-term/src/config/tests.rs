//! The settings' tests: what each `settings.kdl` form reads as, what is
//! warned about, and that writing settings back reads as the same.

// Tests build configs by mutating fields off `default()` for readability,
// which is clearer here than large struct literals.
#![allow(clippy::field_reassign_with_default)]

use winter_render::{Theme, ThemeRgb, SCALAR_COLORS};

use super::*;

#[test]
fn test_lsp_settings_read_back_through_a_save() {
    let text = "lsp {\n    auto-start \"rust\" \"python\"\n    servers {\n        python \"pylsp\"\n        c \"clangd\" \"--background-index\"\n    }\n}\n";
    let (config, warning) = Config::parse_checked(text);
    assert_eq!(warning, None);
    assert_eq!(config.lsp.auto_start, ["rust", "python"]);
    assert_eq!(config.lsp.servers["c"], ["clangd", "--background-index"]);
    let again = Config::parse(&config.to_kdl());
    assert_eq!(again.lsp, config.lsp);
}

#[test]
fn test_completion_settings_read_back_and_an_unknown_backend_is_named() {
    let text = "completion {\n    auto #false\n    delay-ms 120\n    min-chars 2\n    backends {\n        rust \"words\"\n        python \"snippets\" \"word\"\n    }\n}\n";
    let (config, refused) = Config::parse_checked(text);
    assert_eq!(refused, None, "a misspelled back-end does not refuse the file");
    let warning = config.settings_warning.clone();
    assert!(!config.completion.auto);
    assert_eq!(config.completion.min_chars, 2);
    assert_eq!(config.completion.delay, std::time::Duration::from_millis(120));
    assert_eq!(config.completion.backends["rust"], ["words"]);
    assert_eq!(config.completion.backends["python"], ["snippets", "word"]);
    assert!(warning.is_some_and(|w| w.contains("word") && w.contains("python")), "a misspelled back-end");
    let again = Config::parse(&config.to_kdl());
    assert_eq!(again.completion, config.completion);
}

#[test]
fn test_default_config() {
    let config = Config::default();
    assert_eq!(config.font_size, 15.0);
    assert_eq!(config.opacity, 1.0);
    assert_eq!(config.theme, ThemeSetting::Dark);
    assert!(config.keybindings.is_empty());
}

#[test]
fn test_parse_minimal_config() {
    let config = Config::parse("font-size 18\nopacity 0.9");
    assert_eq!(config.font_size, 18.0);
    assert_eq!(config.opacity, 0.9);
}

#[test]
fn test_parse_font_family() {
    let config = Config::parse("font \"FiraCode Nerd Font\"");
    assert_eq!(config.font_family.as_deref(), Some("FiraCode Nerd Font"));
}

#[test]
fn test_default_font_family_is_none() {
    assert_eq!(Config::default().font_family, None);
    assert_eq!(Config::parse("font-size 12").font_family, None);
}

#[test]
fn test_parse_scrollback_lines() {
    let config = Config::parse("scrollback-lines 50000");
    assert_eq!(config.scrollback_lines, Some(50_000));
}

#[test]
fn test_scrollback_lines_zero_is_ignored() {
    let config = Config::parse("scrollback-lines 0");
    assert_eq!(config.scrollback_lines, None);
}

#[test]
fn test_to_kdl_roundtrips_scrollback_lines() {
    let mut config = Config::default();
    config.scrollback_lines = Some(20_000);
    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.scrollback_lines, Some(20_000));
}

#[test]
fn test_parse_shell() {
    let config = Config::parse("shell \"/usr/bin/fish\"");
    assert_eq!(config.shell.as_deref(), Some("/usr/bin/fish"));
}

#[test]
fn test_default_shell_is_none() {
    assert_eq!(Config::default().shell, None);
    assert_eq!(Config::parse("font-size 12").shell, None);
}

#[test]
fn test_shell_whitespace_ignored() {
    let config = Config::parse("shell \"   \"");
    assert_eq!(config.shell, None);
}

#[test]
fn test_parse_colors_block() {
    let config = Config::parse(
        r##"
colors {
background "#1a1a2e"
foreground "#e0e0e0"
cursor-bg "#4fa2fa"
scrollbar "#2f6099"
split "#333333"
window-border "#414248"
ansi {
    black "#000000"
    red "#c22727"
}
indexed {
    "136" "#af8700"
}
}
"##,
    );
    let mut theme = Theme::dark();
    config.colors.apply(&mut theme);
    assert_eq!(theme.background, ThemeRgb::parse_hex("#1a1a2e").unwrap());
    assert_eq!(theme.foreground, ThemeRgb::parse_hex("#e0e0e0").unwrap());
    assert_eq!(theme.divider, ThemeRgb::parse_hex("#333333").unwrap());
    assert_eq!(theme.window_border, ThemeRgb::parse_hex("#414248").unwrap());
    assert_eq!(theme.ansi[1], ThemeRgb::parse_hex("#c22727").unwrap());
    assert_eq!(theme.indexed_color(136), Some((175, 135, 0)));
    // cursor and scrollbar are overridden to the configured blues.
    assert_eq!(theme.cursor_bg, ThemeRgb::parse_hex("#4fa2fa").unwrap());
    assert_eq!(theme.scrollbar, ThemeRgb::parse_hex("#2f6099").unwrap());
}

#[test]
fn test_no_colors_block_leaves_preset_untouched() {
    let config = Config::parse("theme \"dark\"");
    let mut theme = Theme::dark();
    config.colors.apply(&mut theme);
    assert_eq!(theme, Theme::dark());
}

#[test]
fn test_todo_keywords_parse_clean_and_roundtrip() {
    assert_eq!(
        Config::default().todo_keywords,
        ["TODO", "FIXME", "XXX", "HACK", "BUG"]
    );
    // Blanks and repeats are dropped, order is kept, and a name with a
    // space in it is the two words it would be read back as.
    let config = Config::parse(r#"todo-keywords "FIXME" "" "TODO" "FIXME" "NOTE WIP""#);
    assert_eq!(config.todo_keywords, ["FIXME", "TODO", "NOTE", "WIP"]);
    // A list left with nothing in it finds the defaults, not nothing.
    assert_eq!(
        Config::parse("todo-keywords").todo_keywords,
        Config::default().todo_keywords
    );
    assert_eq!(
        Config::parse(&config.to_kdl()).todo_keywords,
        config.todo_keywords
    );
}

#[test]
fn test_the_popup_input_height_is_read_kept_in_range_and_written_back() {
    assert_eq!(
        Config::default().popup_input_height,
        winter_render::DEFAULT_POPUP_INPUT_HEIGHT
    );
    assert_eq!(Config::parse("popup-input-height 2.5").popup_input_height, 2.5);
    let (least, most) = winter_render::POPUP_INPUT_HEIGHT_RANGE;
    assert_eq!(Config::parse("popup-input-height 0.2").popup_input_height, least);
    assert_eq!(Config::parse("popup-input-height 9").popup_input_height, most);
    let mut config = Config::default();
    config.popup_input_height = 2.0;
    assert_eq!(Config::parse(&config.to_kdl()).popup_input_height, 2.0);
}

#[test]
fn test_window_title_template_defaults_and_roundtrips() {
    assert_eq!(
        Config::default().window_title_template,
        "{{ name }} — Winter Term"
    );
    assert_eq!(
        Config::parse("window-title-template \"{{ app_name }} — {{ pane_title }}\"")
            .window_title_template,
        "{{ app_name }} — {{ pane_title }}".to_string()
    );
    // Unset or empty falls back to the default.
    assert_eq!(
        Config::parse("").window_title_template,
        Config::default().window_title_template
    );
    assert_eq!(
        Config::parse("window-title-template \"\"").window_title_template,
        Config::default().window_title_template
    );
    // Serializes and parses back to the same template.
    let mut config = Config::default();
    config.window_title_template = "{{ app_name }}".to_string();
    assert_eq!(
        Config::parse(&config.to_kdl()).window_title_template,
        "{{ app_name }}".to_string()
    );
}

#[test]
fn test_title_bar_style_defaults_modern_and_roundtrips() {
    assert_eq!(Config::default().title_bar_style, TitleBarStyle::Modern);
    assert_eq!(
        Config::parse("title-bar-style \"system\"").title_bar_style,
        TitleBarStyle::System
    );
    // `native` is an alias; unknown values fall back to the modern default.
    assert_eq!(
        Config::parse("title-bar-style \"native\"").title_bar_style,
        TitleBarStyle::System
    );
    assert_eq!(
        Config::parse("title-bar-style \"bogus\"").title_bar_style,
        TitleBarStyle::Modern
    );
    // Serializes and parses back to the same selection.
    let mut config = Config::default();
    config.title_bar_style = TitleBarStyle::System;
    assert_eq!(
        Config::parse(&config.to_kdl()).title_bar_style,
        TitleBarStyle::System
    );
}

#[test]
fn test_parse_theme_auto() {
    let config = Config::parse("theme \"auto\"");
    assert_eq!(config.theme, ThemeSetting::Auto);
}

#[test]
fn test_parse_theme_light() {
    let config = Config::parse("theme \"light\"");
    assert_eq!(config.theme, ThemeSetting::Light);
}

#[test]
fn test_parse_keybindings() {
    let config = Config::parse_with_keys(
        "",
        r#"
normal {
    j "focus_down"
    k "focus_up"
}
insert {
    "C-SPC" "toggle_mode"
}
"#,
    );
    assert_eq!(config.keybindings.len(), 2);
    let normal = config.keybindings.get("normal").unwrap();
    assert_eq!(normal.get("j"), Some(&"focus_down".to_string()));
    assert_eq!(normal.get("k"), Some(&"focus_up".to_string()));
    let insert = config.keybindings.get("insert").unwrap();
    assert_eq!(insert.get("C-SPC"), Some(&"toggle_mode".to_string()));
}

#[test]
fn test_opacity_clamped() {
    let config = Config::parse("opacity 0.05");
    assert_eq!(config.opacity, 0.1);
}

#[test]
fn test_parse_invalid_returns_default() {
    let config = Config::parse("this is not valid kdl {{{{");
    assert_eq!(config.font_size, 15.0);
}

#[test]
fn test_parse_with_keys_merges_separate_files() {
    let settings = "theme \"light\"\nmenu-style \"classic\"";
    let keys = r#"
normal {
j "focus_down"
}
window {
"C-w v" "split_vertical"
}
"#;
    let config = Config::parse_with_keys(settings, keys);
    // Settings come from settings.kdl.
    assert_eq!(config.theme, ThemeSetting::Light);
    assert_eq!(config.menu_style, MenuStyle::Classic);
    // Keybindings come from keybindings.kdl (top-level mode blocks).
    assert_eq!(
        config.keybindings.get("normal").and_then(|m| m.get("j")),
        Some(&"focus_down".to_string())
    );
    assert_eq!(
        config
            .keybindings
            .get("window")
            .and_then(|m| m.get("C-w v")),
        Some(&"split_vertical".to_string())
    );
}

#[test]
fn test_parse_with_empty_keys_keeps_no_bindings() {
    let config = Config::parse_with_keys("font-size 12", "");
    assert!(config.keybindings.is_empty());
}

#[test]
fn test_menu_style_defaults_to_modern_and_parses_classic() {
    assert_eq!(Config::default().menu_style, MenuStyle::Modern);
    assert_eq!(Config::parse("font-size 12").menu_style, MenuStyle::Modern);
    assert_eq!(
        Config::parse("menu-style \"classic\"").menu_style,
        MenuStyle::Classic
    );
    // An unrecognized value falls back to the modern default.
    assert_eq!(
        Config::parse("menu-style \"fancy\"").menu_style,
        MenuStyle::Modern
    );
}

#[test]
fn test_parse_cursor_block_and_synonyms() {
    let config = Config::parse(
        r#"
cursor {
insert "beam"
normal "underline"
visual "underscore"
}
"#,
    );
    // Synonyms resolve to the canonical variants; "beam"→Bar,
    // "underline"/"underscore"→Underline.
    assert_eq!(config.cursor.insert, CursorShape::Bar);
    assert_eq!(config.cursor.normal, CursorShape::Underline);
    assert_eq!(config.cursor.visual, CursorShape::Underline);
}

#[test]
fn test_window_controls_side_defaults_left_parses_right() {
    assert_eq!(Config::default().window_controls_side, ControlsSide::Left);

    let config = Config::parse("window-controls-side \"right\"");
    assert_eq!(config.window_controls_side, ControlsSide::Right);

    // Unknown values fall back to the left-side default.
    let config = Config::parse("window-controls-side \"sideways\"");
    assert_eq!(config.window_controls_side, ControlsSide::Left);
}

#[test]
fn test_cursor_defaults_to_block_for_nav_bar_for_insert() {
    let config = Config::default();
    assert_eq!(config.cursor.insert, CursorShape::Bar);
    assert_eq!(config.cursor.normal, CursorShape::Block);
    assert_eq!(config.cursor.visual, CursorShape::Block);
}

#[test]
fn test_cursor_block_roundtrips_through_kdl() {
    let mut config = Config::default();
    config.cursor.normal = CursorShape::Underline;
    config.cursor.visual = CursorShape::Bar;
    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.cursor.insert, CursorShape::Bar);
    assert_eq!(parsed.cursor.normal, CursorShape::Underline);
    assert_eq!(parsed.cursor.visual, CursorShape::Bar);
}

#[test]
fn test_to_kdl_roundtrips_scalar_settings() {
    let mut config = Config::default();
    config.theme = ThemeSetting::Light;
    config.menu_style = MenuStyle::Classic;
    config.font_family = Some("Fira Code".to_string());
    config.font_size = 18.0;
    config.opacity = 0.8;
    config.status_bar.enabled = false;

    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.theme, ThemeSetting::Light);
    assert_eq!(parsed.menu_style, MenuStyle::Classic);
    assert_eq!(parsed.font_family.as_deref(), Some("Fira Code"));
    assert_eq!(parsed.font_size, 18.0);
    assert_eq!(parsed.opacity, 0.8);
    assert!(!parsed.status_bar.enabled);
    assert!(parsed.status_bar.show_mode);
}

#[test]
fn test_to_kdl_roundtrips_shell() {
    let mut config = Config::default();
    config.shell = Some("/usr/bin/fish".to_string());
    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.shell.as_deref(), Some("/usr/bin/fish"));
}

#[test]
fn test_to_kdl_omits_shell_when_none() {
    let kdl = Config::default().to_kdl();
    assert!(!kdl.contains("shell"), "shell should not appear when None");
}

#[test]
fn test_parse_shell_linux() {
    let config = Config::parse("shell-linux \"/usr/bin/fish\"");
    assert_eq!(config.shell_linux.as_deref(), Some("/usr/bin/fish"));
    assert_eq!(config.shell_macos, None);
    assert_eq!(config.shell_windows, None);
}

#[test]
fn test_parse_shell_macos() {
    let config = Config::parse("shell-macos \"/bin/zsh\"");
    assert_eq!(config.shell_macos.as_deref(), Some("/bin/zsh"));
    assert_eq!(config.shell_linux, None);
}

#[test]
fn test_parse_shell_windows() {
    let config = Config::parse("shell-windows \"pwsh.exe\"");
    assert_eq!(config.shell_windows.as_deref(), Some("pwsh.exe"));
    assert_eq!(config.shell_linux, None);
}

#[test]
fn test_active_shell_prefers_os_specific_over_generic() {
    let mut config = Config::default();
    config.shell = Some("/bin/sh".to_string());
    #[cfg(target_os = "windows")]
    {
        config.shell_windows = Some("pwsh.exe".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        config.shell_macos = Some("/usr/local/bin/fish".to_string());
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        config.shell_linux = Some("/usr/bin/fish".to_string());
    }

    #[cfg(target_os = "windows")]
    assert_eq!(config.active_shell(), Some("pwsh.exe"));
    #[cfg(target_os = "macos")]
    assert_eq!(config.active_shell(), Some("/usr/local/bin/fish"));
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    assert_eq!(config.active_shell(), Some("/usr/bin/fish"));
}

#[test]
fn test_active_shell_falls_back_to_generic() {
    let mut config = Config::default();
    config.shell = Some("/bin/sh".to_string());
    assert_eq!(config.active_shell(), Some("/bin/sh"));
}

#[test]
fn test_active_shell_none_when_all_unset() {
    assert_eq!(Config::default().active_shell(), None);
}

#[test]
fn test_to_kdl_roundtrips_all_shell_variants() {
    let mut config = Config::default();
    config.shell_linux = Some("/usr/bin/fish".to_string());
    config.shell_macos = Some("/opt/homebrew/bin/fish".to_string());
    config.shell_windows = Some("pwsh.exe".to_string());
    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.shell_linux.as_deref(), Some("/usr/bin/fish"));
    assert_eq!(
        parsed.shell_macos.as_deref(),
        Some("/opt/homebrew/bin/fish")
    );
    assert_eq!(parsed.shell_windows.as_deref(), Some("pwsh.exe"));
    assert_eq!(parsed.shell, None);
}

#[test]
fn test_theme_setting_from_and_to_value() {
    assert_eq!(ThemeSetting::from_value("auto"), ThemeSetting::Auto);
    assert_eq!(ThemeSetting::from_value("light"), ThemeSetting::Light);
    assert_eq!(ThemeSetting::from_value("dark"), ThemeSetting::Dark);
    assert_eq!(
        ThemeSetting::from_value("dracula"),
        ThemeSetting::Named("dracula".to_string())
    );
    assert_eq!(ThemeSetting::Named("nord".to_string()).as_value(), "nord");
    assert_eq!(ThemeSetting::Auto.as_value(), "auto");
}

#[test]
fn test_parse_and_roundtrip_named_theme() {
    let config = Config::parse("theme \"dracula\"");
    assert_eq!(config.theme, ThemeSetting::Named("dracula".to_string()));
    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.theme, ThemeSetting::Named("dracula".to_string()));
}

#[test]
fn test_translucent_colors_are_laid_over_the_background_and_saved_as_written() {
    let mut config = Config::default();
    config.colors.background = ThemeRgb::parse_hex("#00000080");
    config
        .colors
        .set_scalar("selection-bg", ThemeRgb::parse_hex("#ffffff80"));
    config
        .colors
        .git
        .set("file-band", ThemeRgb::parse_hex("#ff000000"));
    config
        .colors
        .syntax
        .set("comment", ThemeRgb::parse_hex("#11223380"));

    let mut theme = Theme::dark();
    config.colors.apply(&mut theme);
    assert_eq!(theme.background_alpha, 0x80, "the window shows through");
    assert_eq!(theme.background, ThemeRgb::new(0, 0, 0));
    assert_eq!(theme.selection_bg, ThemeRgb::new(128, 128, 128));
    assert_eq!(
        theme.git.file_band,
        ThemeRgb::parse_hex("#ff000000"),
        "kept as written"
    );
    assert!(theme.selection_bg.is_opaque());
    assert_eq!(
        theme.git.hunk_band,
        Theme::dark().git.hunk_band,
        "unset keeps the preset's"
    );

    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(
        parsed.colors.scalar("selection-bg"),
        ThemeRgb::parse_hex("#ffffff80")
    );
    assert_eq!(
        parsed.colors.git.get("file-band"),
        ThemeRgb::parse_hex("#ff000000")
    );
    assert_eq!(
        parsed.colors.syntax.get("comment"),
        ThemeRgb::parse_hex("#11223380")
    );
    assert_eq!(
        theme.syntax.get("comment"),
        ThemeRgb::parse_hex("#11223380"),
        "kept as written"
    );
    assert_eq!(
        theme.syntax.get("string"),
        Theme::dark().syntax.get("string"),
        "unset keeps the preset's"
    );
}

#[test]
fn test_to_kdl_roundtrips_color_overrides() {
    let mut config = Config::default();
    config.colors.background = ThemeRgb::parse_hex("#1a1a2e");
    config
        .colors
        .set_scalar("cursor-bg", ThemeRgb::parse_hex("#52ad70"));
    config
        .colors
        .set_scalar("window-border", ThemeRgb::parse_hex("#123456"));
    config
        .colors
        .set_scalar("active-indicator", ThemeRgb::parse_hex("#e0457b"));
    config.colors.ansi[0] = ThemeRgb::parse_hex("#000000");
    config.colors.indexed = vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())];

    let parsed = Config::parse(&config.to_kdl());
    assert_eq!(parsed.colors.background, ThemeRgb::parse_hex("#1a1a2e"));
    assert_eq!(
        parsed.colors.scalar("cursor-bg"),
        ThemeRgb::parse_hex("#52ad70")
    );
    assert_eq!(
        parsed.colors.scalar("window-border"),
        ThemeRgb::parse_hex("#123456")
    );
    assert_eq!(
        parsed.colors.scalar("active-indicator"),
        ThemeRgb::parse_hex("#e0457b")
    );
    assert_eq!(parsed.colors.ansi[0], ThemeRgb::parse_hex("#000000"));
    assert!(parsed.colors.ansi[1..].iter().all(Option::is_none));
    assert_eq!(
        parsed.colors.indexed,
        vec![(136, ThemeRgb::parse_hex("#af8700").unwrap())]
    );
}

#[test]
fn test_to_kdl_without_overrides_writes_no_colors_block() {
    let config = Config::default();
    let kdl = config.to_kdl();
    assert!(!kdl.contains("colors {"), "{kdl}");
    // A pristine config still round-trips to its defaults.
    let parsed = Config::parse(&kdl);
    assert_eq!(parsed.theme, ThemeSetting::Dark);
    assert_eq!(parsed.font_size, 15.0);
}

#[test]
fn test_status_bar_show_mode_defaults_on_and_parses_off() {
    let default = Config::default().status_bar;
    assert!(default.enabled && default.show_mode);

    let config = Config::parse(
        r#"
status-bar {
show #false
}
"#,
    );
    assert!(!config.status_bar.enabled);
    assert!(config.status_bar.show_mode);

    let config = Config::parse(
        r#"
status-bar {
show #true
show-mode #false
}
"#,
    );
    assert!(config.status_bar.enabled);
    assert!(!config.status_bar.show_mode);
}

#[test]
fn test_status_bar_priority_is_read_and_written_back() {
    let (config, warning) = Config::parse_checked(
        r#"
status-bar {
segments "tool" "claude"
priority "claude" "tool"
}
"#,
    );
    assert_eq!(warning, None);
    assert_eq!(
        config.status_bar.priority,
        [StatusSegmentKind::Claude, StatusSegmentKind::Editor]
    );
    let again = Config::parse_checked(&config.to_kdl()).0;
    assert_eq!(again.status_bar.priority, config.status_bar.priority);
    assert!(Config::parse_checked("").0.status_bar.priority.is_empty());
}

#[test]
fn test_status_bar_segments_are_picked_and_ordered_by_name() {
    let (config, refused) = Config::parse_checked(
        r#"
status-bar {
segments "tool" "zai" "nope" "claude"
}
"#,
    );
    // Refusing the file over one unknown name left every later edit to it
    // unapplied on a hot reload.
    assert_eq!(refused, None, "the file applies");
    let warning = config.settings_warning.clone();
    assert_eq!(
        config.status_bar.segments,
        [
            StatusSegmentKind::Editor,
            StatusSegmentKind::Zai,
            StatusSegmentKind::Claude,
        ]
    );
    assert!(
        warning.is_some_and(|w| w.contains("nope")),
        "an unknown name is said, not silently dropped"
    );

    let one = Config::parse("status-bar {\n    segments \"claude\"\n}\n");
    assert_eq!(one.status_bar.segments, [StatusSegmentKind::Claude]);

    let none = Config::parse("status-bar {\n    segments\n}\n");
    assert!(
        none.status_bar.segments.is_empty(),
        "an empty list hides all"
    );

    let unset = Config::parse("status-bar {\n    show #true\n}\n");
    assert_eq!(unset.status_bar.segments, StatusSegmentKind::defaults());
}

#[test]
fn test_status_bar_segments_round_trip_through_settings_kdl() {
    for segments in [
        vec![StatusSegmentKind::Zai, StatusSegmentKind::Editor],
        Vec::new(),
    ] {
        let mut config = Config::default();
        config.status_bar.segments = segments.clone();
        assert_eq!(
            Config::parse(&config.to_kdl()).status_bar.segments,
            segments
        );
    }
}

#[test]
fn test_cursor_blink_parses_and_round_trips() {
    let c = Config::default();
    assert!(c.cursor.blink, "blink defaults to true");

    let no_blink = Config::parse("cursor {\n    blink #false\n}");
    assert!(!no_blink.cursor.blink);

    // Round-trip via to_kdl / parse.
    let kdl = no_blink.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(!restored.cursor.blink);
}

#[test]
fn test_clipboard_read_parses_and_round_trips() {
    let c = Config::default();
    assert!(
        !c.clipboard_read,
        "clipboard_read defaults to false (refuse)"
    );

    let enabled = Config::parse(r"clipboard-read #true");
    assert!(enabled.clipboard_read);

    let kdl = enabled.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(restored.clipboard_read);
}

#[test]
fn test_ligatures_parses_and_round_trips() {
    let c = Config::default();
    assert!(!c.ligatures, "ligatures defaults to false");

    let lig = Config::parse(r#"ligatures #true"#);
    assert!(lig.ligatures);

    let kdl = lig.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(restored.ligatures);
}

#[test]
fn test_url_underline_parses_and_round_trips() {
    let c = Config::default();
    assert!(!c.url_underline, "url_underline defaults to false");

    let underline = Config::parse(r#"url-underline #true"#);
    assert!(underline.url_underline);

    let kdl = underline.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(restored.url_underline);
}

#[test]
fn test_wrap_indent_parses_and_round_trips() {
    let c = Config::default();
    assert!(c.wrap_indent, "wrap_indent defaults to true");

    let disabled = Config::parse(r#"wrap-indent #false"#);
    assert!(!disabled.wrap_indent);

    let kdl = disabled.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(!restored.wrap_indent);
}

#[test]
fn test_scroll_margin_defaults_to_five_and_is_held_to_its_most() {
    assert_eq!(Config::default().scroll_margin, DEFAULT_SCROLL_MARGIN);
    assert_eq!(Config::parse("theme \"dark\"").scroll_margin, 5, "absent, the default");
    let none = Config::parse("scroll-margin 0");
    assert_eq!(none.scroll_margin, 0, "0 turns it off");
    assert_eq!(Config::parse(&none.to_kdl()).scroll_margin, 0, "and is written back");
    assert_eq!(Config::parse("scroll-margin 9999").scroll_margin, MAX_SCROLL_MARGIN);
}

#[test]
fn test_wrap_words_parses_and_round_trips() {
    let c = Config::default();
    assert!(c.wrap_words, "wrap_words defaults to true");

    let disabled = Config::parse(r#"wrap-words #false"#);
    assert!(!disabled.wrap_words);

    let kdl = disabled.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(!restored.wrap_words);
}

#[test]
fn test_rainbow_parens_parses_and_round_trips() {
    let c = Config::default();
    assert!(!c.rainbow_parens, "rainbow_parens defaults to false");

    let enabled = Config::parse(r#"rainbow-parens #true"#);
    assert!(enabled.rainbow_parens);

    let kdl = enabled.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(restored.rainbow_parens);
}

#[test]
fn test_sentence_highlight_parses_and_round_trips() {
    let c = Config::default();
    assert!(
        !c.sentence_highlight,
        "sentence_highlight defaults to false"
    );

    let enabled = Config::parse(r#"sentence-highlight #true"#);
    assert!(enabled.sentence_highlight);

    let kdl = enabled.to_kdl();
    let restored = Config::parse(&kdl);
    assert!(restored.sentence_highlight);
}

#[test]
fn test_prompt_edit_bindings_parse_and_round_trip() {
    assert_eq!(
        Config::default().prompt_edit_bindings,
        PromptEditBindings::Emacs
    );

    let off = Config::parse(r#"prompt-edit-bindings "none""#);
    assert_eq!(off.prompt_edit_bindings, PromptEditBindings::None);
    assert_eq!(
        Config::parse(&off.to_kdl()).prompt_edit_bindings,
        PromptEditBindings::None
    );

    // Spellings a user is likely to reach for.
    for spelling in ["none", "off", "vi"] {
        let text = format!(r#"prompt-edit-bindings "{spelling}""#);
        assert_eq!(
            Config::parse(&text).prompt_edit_bindings,
            PromptEditBindings::None,
            "{spelling} should disable prompt editing"
        );
    }
}

#[test]
fn test_unknown_prompt_edit_bindings_keeps_the_default() {
    // A typo must not silently disable a working feature.
    let c = Config::parse(r#"prompt-edit-bindings "emcas""#);
    assert_eq!(c.prompt_edit_bindings, PromptEditBindings::Emacs);
}

#[test]
fn test_parse_checked_reports_a_rejected_file() {
    let (config, error) = Config::parse_checked("this is not kdl {{{");
    assert_eq!(config.font_size, Config::default().font_size);
    let message = error.expect("a rejected file must report why");
    assert!(
        message.contains("settings.kdl"),
        "diagnostic should name the file: {message}"
    );
}

#[test]
fn test_a_syntax_color_no_scope_goes_by_is_reported_and_the_rest_kept() {
    let (config, refused) = Config::parse_checked(
        r##"colors { syntax { keyword "#010203"; entity.name.function.rust "#040506" } }"##,
    );
    assert_eq!(refused, None);
    let message = config.settings_warning.clone().expect("an unknown scope is reported");
    assert!(message.contains("entity.name.function.rust"), "{message}");
    assert_eq!(
        config.colors.syntax.get("keyword"),
        ThemeRgb::parse_hex("#010203")
    );
}

#[test]
fn test_parse_checked_reports_nothing_for_a_good_file() {
    let (_, error) = Config::parse_checked("font-size 17");
    assert!(error.is_none());
}

/// The kebab-case names of the settings the schema accepts, read out of the
/// schema's own source so the list cannot fall behind it. `keybindings` is
/// left out: it is legal here but documented to live in its own file.
fn schema_keys(schema: &str) -> Vec<String> {
    let start = schema
        .find("struct KdlConfig {")
        .expect("the schema declares KdlConfig");
    schema[start..]
        .lines()
        .skip(1)
        .take_while(|line| !line.starts_with('}'))
        .filter_map(|line| line.trim().strip_prefix("pub(crate) "))
        .filter_map(|field| field.split(':').next())
        .filter(|name| *name != "keybindings")
        .map(|name| name.replace('_', "-"))
        .collect()
}

/// Whether `text` writes `key` as a settings key, set or commented out,
/// rather than merely mentioning it in a sentence.
fn sets_key(text: &str, key: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim_start().trim_start_matches('/').trim_start();
        line.strip_prefix(key)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '{']))
    })
}

#[test]
fn test_every_setting_appears_in_the_shipped_sample() {
    // The sample's header promises every option at its default value, and
    // the installers ship it as the reference users copy from: a setting
    // added to the schema and not to the file is one nobody learns exists.
    // The keys are read from the schema source rather than listed here,
    // because a hand-kept list is the thing that fell behind in the first
    // place.
    let schema = include_str!("schema.rs");
    let sample = include_str!("../../builtin/settings.kdl");
    let missing: Vec<String> = schema_keys(schema)
        .into_iter()
        .filter(|key| !sets_key(sample, key))
        .collect();
    assert!(
        missing.is_empty(),
        "missing from builtin/settings.kdl: {missing:?}"
    );
}

#[test]
fn test_every_setting_survives_a_save() {
    // Saving rewrites the whole file from the live config, so a key `to_kdl`
    // skips is one the user loses the moment they change anything else on
    // the settings page. Optional settings are given values here, since a
    // config that never set them has nothing to write.
    let config = Config {
        font_family: Some("Fira Code".to_string()),
        font_weight: Some("regular".to_string()),
        font_weight_bold: Some("bold".to_string()),
        scrollback_lines: Some(5_000),
        shell: Some("/bin/zsh".to_string()),
        shell_linux: Some("/bin/zsh".to_string()),
        shell_macos: Some("/bin/zsh".to_string()),
        shell_windows: Some("pwsh.exe".to_string()),
        ..Config::default()
    };
    let written = config.to_kdl();
    let schema = include_str!("schema.rs");
    let dropped: Vec<String> = schema_keys(schema)
        .into_iter()
        // A color table is only written when something overrides the theme.
        .filter(|key| key != "colors")
        .filter(|key| !sets_key(&written, key))
        .collect();
    assert!(dropped.is_empty(), "not written back on save: {dropped:?}");
}

#[test]
fn test_the_settings_a_save_used_to_drop_come_back() {
    // These three were written by no one: a save rebuilt the file without
    // them, so each reverted to its default behind the user's back. Values
    // opposite to the defaults, so a dropped key cannot read as a pass.
    let mut config = Config::default();
    config.cursor.hide_in_inactive = !config.cursor.hide_in_inactive;
    config.dim_inactive = !config.dim_inactive;
    config.restore_session = !config.restore_session;

    let reloaded = Config::parse(&config.to_kdl());
    assert_eq!(reloaded.dim_inactive, config.dim_inactive);
    assert_eq!(reloaded.restore_session, config.restore_session);
    assert_eq!(
        reloaded.cursor.hide_in_inactive,
        config.cursor.hide_in_inactive
    );
}

#[test]
fn test_the_builtin_settings_give_every_setting_a_value_but_those_unset_by_default() {
    // A setting the built-in file leaves commented out is read over
    // settings that set nothing: off, zero, or empty, not its default.
    // Only the ones whose default is to have no value may be left so.
    const UNSET_BY_DEFAULT: [&str; 9] = [
        "colors",
        "font",
        "font-weight",
        "font-weight-bold",
        "scrollback-lines",
        "shell",
        "shell-linux",
        "shell-macos",
        "shell-windows",
    ];
    let schema = include_str!("schema.rs");
    let builtin = include_str!("../../builtin/settings.kdl");
    let commented: Vec<String> = schema_keys(schema)
        .into_iter()
        .filter(|key| !UNSET_BY_DEFAULT.contains(&key.as_str()))
        .filter(|key| {
            !builtin.lines().any(|line| {
                line.strip_prefix(key.as_str())
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '{']))
            })
        })
        .collect();
    assert!(commented.is_empty(), "given no value: {commented:?}");
}

#[test]
fn test_a_file_setting_one_key_of_a_block_keeps_the_builtin_rest() {
    let config = Config::parse("completion {\n    auto #false\n}\nfont-size 17");
    assert!(!config.completion.auto);
    assert_eq!(config.completion.min_chars, 3, "the built-in file's");
    assert_eq!(config.completion.delay, Config::default().completion.delay);
    assert!(config.cursor.blink, "a block the file leaves out is the built-in one");
    // A value that cannot be used falls back to the built-in one, too.
    let config = Config::parse("compile {\n    latex-command \"\"\n}");
    assert!(config.compile.latex_command.starts_with("latexmk"));
}

#[test]
fn test_shipped_sample_settings_parse_cleanly() {
    // `builtin/settings.kdl` is where every default comes from, and the
    // installers ship it as the file users copy; a key it cannot read would
    // leave Winter with no defaults at all.
    let sample = include_str!("../../builtin/settings.kdl");
    let (_, error) = Config::parse_checked(sample);
    assert!(
        error.is_none(),
        "builtin settings.kdl failed to parse: {error:?}"
    );
}

#[test]
fn test_a_font_size_the_renderer_cannot_draw_is_held_to_one_it_can() {
    // `font-size 0` used to crash every launch: startup took it as is.
    assert_eq!(Config::parse("font-size 0").font_size, 6.0);
    assert_eq!(Config::parse("font-size -3").font_size, 6.0);
    assert_eq!(Config::parse("font-size 900").font_size, 72.0);
}

#[test]
fn test_a_keybindings_file_that_does_not_parse_says_so_without_rejecting_the_settings() {
    let (config, settings_error) =
        Config::parse_with_keys_checked("font-size 17", "window {{{");
    assert_eq!(settings_error, None, "the settings still apply");
    assert_eq!(config.font_size, 17.0);
    assert!(
        config
            .keybindings_error
            .as_deref()
            .is_some_and(|e| e.starts_with("keybindings.kdl ignored")),
        "{:?}",
        config.keybindings_error
    );
}

#[test]
fn test_a_keybindings_file_that_does_not_parse_names_the_line_at_fault() {
    let keys = "window {\n    \"C+h\" \"focus_left\"\n}\neditor {{\n";
    let (config, _) = Config::parse_with_keys_checked("", keys);
    let error = config.keybindings_error.expect("the file is rejected");
    assert!(error.contains("line 4"), "{error}");
}

#[test]
fn test_every_scalar_color_is_read_applied_and_written_under_its_name() {
    // One table drives the schema, the overrides, the theme and both
    // writers: a color set in a file must come back out of each.
    for (index, color) in SCALAR_COLORS.iter().enumerate() {
        let text = format!("colors {{\n    {} \"#123456\"\n}}", color.name);
        let config = Config::parse(&text);
        let rgb = ThemeRgb::parse_hex("#123456");
        assert_eq!(config.colors.scalars[index], rgb, "{} is read", color.name);
        let mut theme = Theme::dark();
        config.colors.apply(&mut theme);
        assert_eq!(Some((color.read)(&theme)), rgb, "{} is applied", color.name);
        let written = config.colors.to_kdl().expect("a colors block");
        assert!(written.contains(color.name), "{} is written", color.name);
    }
}
