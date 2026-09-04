//! Shell resolution and the small OSC/process helpers a pane needs.

use base64::Engine;

// ========================================================================
// Shell resolution and OSC helpers
// ========================================================================

/// Resolve a bare shell name (e.g. `zsh`, `fish`) to a full path by searching
/// `$PATH`. Returns the input unchanged when it already contains a path
/// separator or when no match is found. This lets users write `shell-linux
/// "zsh"` instead of `shell-linux "/usr/bin/zsh"` in their config.
///
/// `portable_pty`'s own `search_path` runs inside `spawn_command` and can
/// silently fall through in some graphical environments where `PATH` is
/// narrower than the user's login environment. Resolving here, at spawn time,
/// uses the process's live `$PATH` and surfaces the failure early.
#[cfg(unix)]
pub(super) fn resolve_shell(name: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    if name.contains('/') {
        return name.to_string();
    }
    std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .map(|dir| std::path::PathBuf::from(dir).join(name))
        .find(|p| {
            p.is_file()
                && std::fs::metadata(p)
                    .map(|m| m.permissions().mode() & 0o111 != 0)
                    .unwrap_or(false)
        })
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_else(|| name.to_string())
}
#[cfg(not(unix))]
pub(super) fn resolve_shell(name: &str) -> String {
    name.to_string()
}
/// Clipboard text an OSC 52 read response may carry, in bytes. Larger
/// clipboards answer with an empty payload: the response is written straight
/// into the PTY (whose kernel buffer is ~64 KiB), so an unbounded reply could
/// block the GUI thread on a tool that isn't reading.
pub(super) const OSC52_READ_MAX_BYTES: usize = 64 * 1024;
/// The reply to an `OSC 52 ; c ; ?` query: the clipboard's text base64-
/// encoded as `OSC 52 ; c ; <data> ST`. Text over [`OSC52_READ_MAX_BYTES`]
/// answers with an empty payload — xterm's documented "refused" form — so a
/// querying tool never waits on data that will not come.
pub(crate) fn osc52_read_response(text: &str) -> Vec<u8> {
    let payload = if text.len() <= OSC52_READ_MAX_BYTES {
        base64::engine::general_purpose::STANDARD.encode(text)
    } else {
        String::new()
    };
    let mut response = format!("\x1b]52;c;{payload}").into_bytes();
    response.extend_from_slice(b"\x1b\\");
    response
}
/// Whether `url`'s scheme is on the safe-open allowlist (`http`, `https`,
/// `mailto`). Rejects `file://`, `javascript:`, custom app schemes, and
/// anything else that could invoke an unexpected OS handler on Ctrl+click.
pub(super) fn is_safe_url_scheme(url: &str) -> bool {
    let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
    matches!(scheme.as_str(), "http" | "https" | "mailto")
}
#[cfg(target_os = "linux")]
pub(super) fn parse_foreground_process(stat_content: &str) -> Option<i32> {
    let rparen = stat_content.rfind(')')?;
    let fields: Vec<&str> = stat_content[rparen + 1..].split_whitespace().collect();
    if fields.len() > 5 {
        let pgrp: i32 = fields[2].parse().ok()?;
        let tpgid: i32 = fields[5].parse().ok()?;
        if tpgid > 0 && pgrp != tpgid {
            return Some(tpgid);
        }
    }
    None
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_osc52_read_response_encodes_caps_and_terminates_with_st() {
        // The reply rides the same OSC 52 grammar: base64 payload, ST
        // terminator. Oversized text answers with the empty "refused"
        // payload so a tool never waits on data that will not come.
        assert_eq!(
            osc52_read_response("hello"),
            b"\x1b]52;c;aGVsbG8=\x1b\\".to_vec()
        );
        assert_eq!(
            osc52_read_response(""),
            b"\x1b]52;c;\x1b\\".to_vec(),
            "an empty clipboard is still a reply"
        );
        let oversized = "x".repeat(OSC52_READ_MAX_BYTES + 1);
        assert_eq!(
            osc52_read_response(&oversized),
            b"\x1b]52;c;\x1b\\".to_vec(),
            "oversized clipboards are refused, not truncated"
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn test_parse_foreground_process() {
        // Active foreground process: pgrp = 12345, tpgid = 12346
        let stat_active = "12345 (bash) S 12344 12345 12345 34816 12346";
        assert_eq!(parse_foreground_process(stat_active), Some(12346));

        // Idle shell: pgrp = 12345, tpgid = 12345
        let stat_idle = "12345 (bash) S 12344 12345 12345 34816 12345";
        assert_eq!(parse_foreground_process(stat_idle), None);

        // Invalid fields
        let stat_invalid = "12345 (bash) S 12344";
        assert_eq!(parse_foreground_process(stat_invalid), None);
    }
    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_absolute_path_unchanged() {
        assert_eq!(resolve_shell("/bin/zsh"), "/bin/zsh");
        assert_eq!(resolve_shell("/usr/bin/fish"), "/usr/bin/fish");
    }
    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_bare_bash_finds_executable() {
        let resolved = resolve_shell("bash");
        assert!(
            resolved.contains('/'),
            "expected bash to resolve to full path, got: {resolved}"
        );
        assert!(
            std::path::Path::new(&resolved).is_file(),
            "resolved path does not exist: {resolved}"
        );
    }
    #[cfg(unix)]
    #[test]
    fn test_resolve_shell_unknown_name_returns_as_is() {
        let name = "this-shell-does-not-exist-12345";
        assert_eq!(resolve_shell(name), name);
    }
}
