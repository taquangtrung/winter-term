//! The token inspector's card, drawn by misty: what the character under the
//! cursor is, where it is, the theme rule its color comes from, and the
//! scopes it sits in, outermost first, hung by the cursor.

use crate::render::TokenInspectView;
use misty::color::Rgb;
use misty::text::{Span, StyledText, TextColor};
use misty::theme::ThemeToken;
use misty::{Frame, InfoCard, Rect, Widget};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// How many cells a row's label takes, its value after them.
const LABEL_CELLS: usize = 10;

/// What a row with no value of its own says.
const NONE: &str = "none";

// ========================================================================
// App: the token card misty draws
// ========================================================================

impl App {
    /// Draw the token inspector's card the last frame worked out over
    /// `frame`.
    pub(in crate::window) fn draw_token_card(&mut self, frame: &mut Frame) {
        let Some(view) = &self.chrome.drawn_token_inspect else {
            return;
        };
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&crate::render::Theme::dark()),
        };
        let (x, top, bottom) = view.anchor;
        let anchor = Rect::new(x, top, frame.cell_metrics().width, bottom - top);
        let mut card = InfoCard::new(lines_of(view), anchor);
        card.draw(frame, frame.bounds(), &theme);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The card's lines for `view`: the character, its place, and its grammar;
/// the theme rule, its color in that color, and its style; then each scope.
fn lines_of(view: &TokenInspectView) -> Vec<StyledText> {
    let muted = TextColor::Token(ThemeToken::Muted);
    let plain = TextColor::Token(ThemeToken::Foreground);
    let accent = TextColor::Token(ThemeToken::Accent);
    let row = |label: &str, value: &str, color: TextColor| {
        StyledText::new()
            .with(Span::new(format!("{label:<LABEL_CELLS$}"), muted))
            .with(Span::new(value.to_owned(), color))
    };
    let heading = |title: &str| StyledText::from(Span::new(title.to_owned(), accent).bold());
    let color = view
        .foreground_hex
        .as_deref()
        .and_then(parse_hex)
        .map_or(plain, |rgb| TextColor::Rgba(misty::color::Rgba::from(rgb)));
    let mut lines = vec![
        row("char", &view.character, plain),
        row("position", &view.position, plain),
        row("language", &view.language, plain),
        heading("theme rule"),
        row("rule", view.color_rule.as_deref().unwrap_or(NONE), plain),
        row(
            "color",
            view.foreground_hex.as_deref().unwrap_or(NONE),
            color,
        ),
        row("style", view.style.as_deref().unwrap_or(NONE), plain),
        heading("scopes"),
    ];
    match view.scopes.is_empty() {
        true => lines.push(StyledText::plain(NONE, muted)),
        false => lines.extend(
            view.scopes
                .iter()
                .map(|scope| StyledText::plain(scope.as_str(), plain)),
        ),
    }
    lines
}

/// The color `#rrggbb` names.
fn parse_hex(hex: &str) -> Option<Rgb> {
    let parsed = crate::render::ThemeRgb::parse_hex(hex)?;
    Some(Rgb::new(parsed.r, parsed.g, parsed.b))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_card_lists_the_token_then_its_rule_then_each_scope() {
        let view = TokenInspectView {
            anchor: (0.0, 0.0, 20.0),
            character: "'a' (U+0061)".into(),
            color_rule: Some("keyword".into()),
            foreground_hex: Some("#c678dd".into()),
            language: "Rust".into(),
            position: "ln 1, col 1".into(),
            scopes: vec!["source.rust".into(), "keyword.control".into()],
            style: None,
        };
        let lines = lines_of(&view);
        let text = |line: &StyledText| {
            line.spans()
                .iter()
                .map(|span| span.text.as_str())
                .collect::<String>()
        };
        assert!(text(&lines[0]).ends_with("'a' (U+0061)"));
        assert!(text(&lines[6]).ends_with(NONE), "no style says so");
        assert_eq!(
            text(&lines[lines.len() - 1]),
            "keyword.control",
            "innermost last"
        );
    }
}
