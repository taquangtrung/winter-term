//! The status bar: its background and prompt caret quads, and its one
//! shaped line (where commands run, the mode, a prompt or the live search, a
//! transient notice, then at the right edge its segments and the bell).

use std::sync::Arc;

use glyphon::{Attrs, BufferLine, Shaping};

use crate::image::ImagePlacement;
use crate::renderer::background::{quad_vertices, BgVertex};
use crate::renderer::glyphs::{base_family, effective_bold_weight, parse_weight};
use crate::renderer::raster::{ICON_HEIGHT_RATIO, STATUS_ICON_TEXTURE_IDS};
use crate::renderer::{GpuRenderer, NoticeKind, StatusBar, StatusHover, StatusSegment, THIN_SPACE};

// ========================================================================
// Constants
// ========================================================================

/// What stands on either side of the host segment at the bar's left edge.
const HOST_PADDING: &str = " ";

/// Cells left empty after the status bar's last glyph, at its right edge.
const RIGHT_MARGIN: usize = 1;

/// The fewest cells kept between the status bar's left text and its right.
const RIGHT_GAP: usize = 2;

/// The cells the tool's icon is drawn over, kept blank in the text: the
/// artwork is square and a cell is about half as wide as it is tall.
const ICON_SLOT: &str = "  ";

/// What stands between the right-hand segments of the status bar.
const SEGMENT_SEPARATOR: &str = "  ";

/// What stands between an icon's slot and the text it heads.
const ICON_GAP: &str = " ";

/// The right side is laid out in half cells, the width of a [`THIN_SPACE`].
const HALVES_PER_CELL: usize = 2;

/// How far, in cells, a hovered part's background reaches past its text on
/// either side: half the gap between two parts, so neighbors never touch.
const HOVER_PAD_CELLS: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// Where the status bar's line put the parts drawn or clicked over it, in
/// cells from its left edge.
pub(in crate::renderer) struct StatusColumns {
    /// Each icon shown, with the first half cell of the slot it is drawn
    /// over, in order from the left.
    pub(in crate::renderer) icons: Vec<(usize, Arc<[u8]>)>,
    /// Each part of the right side shown, with its span `(first, past the
    /// last)` in half cells: the segments that fit, then the bell.
    pub(in crate::renderer) parts: Vec<(StatusHover, usize, usize)>,
}

/// Where one part of the status bar's right side was drawn, in surface
/// pixels, running from `top` to the bottom edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::renderer) struct StatusHit {
    pub(in crate::renderer) left: f32,
    pub(in crate::renderer) part: StatusHover,
    pub(in crate::renderer) right: f32,
    pub(in crate::renderer) top: f32,
}

// ========================================================================
// GpuRenderer: status bar
// ========================================================================

impl GpuRenderer {
    /// Append the status bar's border hairline and background band.
    pub(in crate::renderer) fn push_status_bar_quads(
        &self,
        status: &StatusBar,
        status_top: f32,
        (surface_w, surface_h): (f32, f32),
        out: &mut Vec<BgVertex>,
    ) {
        out.extend_from_slice(&quad_vertices(
            0.0,
            status_top,
            surface_w,
            status_top + 1.0,
            self.theme.status_bar_border.as_linear(),
            surface_w,
            surface_h,
        ));

        out.extend_from_slice(&quad_vertices(
            0.0,
            status_top + 1.0,
            surface_w,
            status_top + crate::STATUS_BAR_HEIGHT * self.cell_height,
            self.theme.background.as_linear(),
            surface_w,
            surface_h,
        ));

        if let Some(ref prompt) = status.prompt {
            let mode_cols = if status.mode.is_empty() {
                0
            } else {
                status.mode.chars().count() + 5
            };
            let prefix_cols = host_segment(&status.host).chars().count() + mode_cols;
            let caret_col = prefix_cols + prompt.label.chars().count() + prompt.caret;
            let cursor_x = caret_col as f32 * self.cell_width;
            let cursor_w = self.cell_width;
            let (top, bottom) = crate::status_bar_content_px(status_top, self.cell_height);
            out.extend_from_slice(&quad_vertices(
                cursor_x,
                top,
                (cursor_x + cursor_w).min(surface_w),
                bottom,
                self.theme.cursor_bg.as_linear(),
                surface_w,
                surface_h,
            ));
        }
    }

    /// Whether the surface point `(x, y)` is on the status bar's bell, as it
    /// was last drawn.
    pub fn status_bell_at(&self, x: f32, y: f32) -> bool {
        self.status_hover_at(x, y) == Some(StatusHover::Bell)
    }

    /// The part of the status bar's right side at the surface point `(x, y)`,
    /// as it was last drawn.
    pub fn status_hover_at(&self, x: f32, y: f32) -> Option<StatusHover> {
        hit_at(&self.status_hits, x, y)
    }

    /// Note where the parts in `columns` landed on the bar whose top edge is
    /// `status_top`, and append the hover background of the one under the
    /// pointer, if any, over the bar's own.
    pub(in crate::renderer) fn push_status_hover_quad(
        &mut self,
        status: &StatusBar,
        columns: &StatusColumns,
        status_top: f32,
        (surface_w, surface_h): (f32, f32),
        out: &mut Vec<BgVertex>,
    ) {
        let cw = self.cell_width / HALVES_PER_CELL as f32;
        self.status_hits = columns
            .parts
            .iter()
            .map(|&(part, first, past)| StatusHit {
                left: first as f32 * cw,
                part,
                right: past as f32 * cw,
                top: status_top,
            })
            .collect();
        let Some(hit) = status
            .hovered
            .and_then(|hovered| self.status_hits.iter().find(|hit| hit.part == hovered))
        else {
            return;
        };
        let pad = HOVER_PAD_CELLS * self.cell_width;
        let (top, bottom) = crate::status_bar_content_px(status_top, self.cell_height);
        out.extend_from_slice(&quad_vertices(
            (hit.left - pad).max(0.0),
            top,
            (hit.right + pad).min(surface_w),
            bottom,
            self.theme.menu_hover_bg.as_linear(),
            surface_w,
            surface_h,
        ));
    }

    /// Shape the status bar's line: the mode label, then the live search
    /// query and any transient notice that follow it. Returns where the bell
    /// and the icons' slots landed.
    pub(in crate::renderer) fn shape_status_bar(
        &mut self,
        status: &StatusBar,
        family: Option<&str>,
        surface_width: f32,
        buffer: &mut glyphon::Buffer,
    ) -> StatusColumns {
        let mut parts_shown = Vec::new();
        let mut icons = Vec::new();
        // Build status bar text segments dynamically
        let mut status_text = String::new();
        let mut spans = Vec::new();

        // Font attributes
        let accent_attrs = Attrs::new()
            .family(base_family(family))
            .weight(effective_bold_weight(
                self.bold_weight.as_deref(),
                self.font_has_bold,
            ))
            .color(status.accent.to_glyphon());
        // The bar's own text is in the theme's text color.
        let text_color = self.theme.foreground;
        let muted_attrs = Attrs::new()
            .family(base_family(family))
            .weight(parse_weight(
                self.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ))
            .color(text_color.to_glyphon());
        let error_attrs = Attrs::new()
            .family(base_family(family))
            .weight(effective_bold_weight(
                self.bold_weight.as_deref(),
                self.font_has_bold,
            ))
            .color(self.theme.ansi[1].to_glyphon());
        let info_attrs = Attrs::new()
            .family(base_family(family))
            .weight(effective_bold_weight(
                self.bold_weight.as_deref(),
                self.font_has_bold,
            ))
            .color(self.theme.ansi[4].to_glyphon());
        let warning_attrs = Attrs::new()
            .family(base_family(family))
            .weight(effective_bold_weight(
                self.bold_weight.as_deref(),
                self.font_has_bold,
            ))
            .color(self.theme.ansi[3].to_glyphon());
        // A search that finds nothing: the warning hue, at the bar's weight.
        let not_found_attrs = muted_attrs.clone().color(self.theme.ansi[3].to_glyphon());

        // Where commands run leads the bar, ahead of everything else.
        let host = host_segment(&status.host);
        if !host.is_empty() {
            status_text.push_str(&host);
            spans.push((0..status_text.len(), info_attrs.clone()));
        }

        if let Some(ref prompt) = status.prompt {
            if !status.mode.is_empty() {
                let mode_start = status_text.len();
                status_text.push_str(&status.mode);
                let mode_end = status_text.len();
                spans.push((mode_start..mode_end, accent_attrs.clone()));

                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));
            }

            let label_start = status_text.len();
            status_text.push_str(&prompt.label);
            let label_end = status_text.len();
            spans.push((label_start..label_end, accent_attrs.clone()));

            let value_chars: Vec<char> = prompt.value.chars().collect();
            let caret = prompt.caret.min(value_chars.len());

            if caret > 0 {
                let before_str: String = value_chars[..caret].iter().collect();
                let b_start = status_text.len();
                status_text.push_str(&before_str);
                let b_end = status_text.len();
                spans.push((b_start..b_end, info_attrs.clone()));
            }

            let cursor_text_attrs = Attrs::new()
                .family(base_family(family))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(self.theme.background.to_glyphon());

            if caret < value_chars.len() {
                let c = value_chars[caret];
                let c_start = status_text.len();
                status_text.push(c);
                let c_end = status_text.len();
                spans.push((c_start..c_end, cursor_text_attrs));

                if caret + 1 < value_chars.len() {
                    let after_str: String = value_chars[caret + 1..].iter().collect();
                    let a_start = status_text.len();
                    status_text.push_str(&after_str);
                    let a_end = status_text.len();
                    spans.push((a_start..a_end, info_attrs.clone()));
                }
            } else {
                let s_start = status_text.len();
                status_text.push(' ');
                let s_end = status_text.len();
                spans.push((s_start..s_end, cursor_text_attrs));
            }

            if let Some(ref search) = status.search {
                if search.match_total > 0 {
                    let count_start = status_text.len();
                    status_text.push_str(&format!(
                        "  [{}/{}]",
                        search.match_index, search.match_total
                    ));
                    let count_end = status_text.len();
                    spans.push((count_start..count_end, muted_attrs.clone()));
                } else if !prompt.value.is_empty() {
                    let none_start = status_text.len();
                    status_text.push_str("  [no matches]");
                    let none_end = status_text.len();
                    spans.push((none_start..none_end, not_found_attrs.clone()));
                }
            }
        } else {
            // Mode label (e.g. Normal, Insert, Block)
            if !status.mode.is_empty() {
                let mode_start = status_text.len();
                status_text.push_str(&status.mode);
                let mode_end = status_text.len();
                spans.push((mode_start..mode_end, accent_attrs.clone()));
            }

            // The live `/` search query follows the mode label while a search
            // is active, showing the query text and its match position; hidden
            // once the search is cancelled (`status.search` is `None`).
            if let Some(ref search) = status.search {
                if !status_text.is_empty() {
                    let sep_start = status_text.len();
                    status_text.push_str("  •  ");
                    let sep_end = status_text.len();
                    spans.push((sep_start..sep_end, muted_attrs.clone()));
                } else {
                    status_text.push(' ');
                }

                // How far the search has got, in the bar's own color and
                // weight; only a search that finds nothing stands out.
                let query_start = status_text.len();
                status_text.push(if search.reverse { '?' } else { '/' });
                status_text.push_str(&search.query);
                let query_end = status_text.len();
                spans.push((query_start..query_end, muted_attrs.clone()));

                if search.match_total > 0 {
                    let count_start = status_text.len();
                    status_text.push_str(&format!(
                        "  [{}/{}]",
                        search.match_index, search.match_total
                    ));
                    let count_end = status_text.len();
                    spans.push((count_start..count_end, muted_attrs.clone()));
                } else if !search.query.is_empty() {
                    let none_start = status_text.len();
                    status_text.push_str("  [no matches]");
                    let none_end = status_text.len();
                    spans.push((none_start..none_end, not_found_attrs.clone()));
                }
            }

            // A transient notice follows the mode label, in the bar's own
            // color and weight whatever its kind: the bell at the right edge
            // is what says an error or a warning came in.
            if let Some(ref notice) = status.notice {
                if !status_text.is_empty() {
                    let sep_start = status_text.len();
                    status_text.push_str("  •  ");
                    let sep_end = status_text.len();
                    spans.push((sep_start..sep_end, muted_attrs.clone()));
                } else {
                    status_text.push(' ');
                }

                let notice_start = status_text.len();
                status_text.push_str(&notice.text);
                let notice_end = status_text.len();
                spans.push((notice_start..notice_end, muted_attrs.clone()));
            }
        }

        // The segments, then the notifications glyph one cell in from the
        // right edge: the line is monospace, so padding with spaces to the
        // column count lands them there. Segments that would run into the
        // left side are dropped lowest priority first (see [`kept`]); the
        // notifications glyph always stays.
        let parts = &status.segments;
        // The right side is laid out in half cells, since a segment's
        // [`THIN_SPACE`] takes one; the left side's text has none.
        let right: Vec<String> = parts.iter().map(|part| rendered(part, false)).collect();
        let bell = status.bell.as_ref();
        let mut thin = Vec::new();
        if !right.is_empty() || bell.is_some() {
            let columns = (surface_width / self.cell_width).floor() as usize;
            let total = columns * HALVES_PER_CELL;
            let used = status_text.chars().count() * HALVES_PER_CELL;
            let gap = halves(SEGMENT_SEPARATOR);
            let bell_width = bell.map_or(0, |bell| halves(&bell.text) + gap);
            let edge = (RIGHT_MARGIN + RIGHT_GAP) * HALVES_PER_CELL;
            let room = total.saturating_sub(used + edge + bell_width);
            let right = fitting(right, parts, room);
            let kept = kept(&right, parts, room);
            let texts: Vec<String> = kept.iter().map(|&index| right[index].clone()).collect();
            let shown = texts.join(SEGMENT_SEPARATOR);
            let shown_width = halves(&shown);
            let bell_gap = match shown.is_empty() {
                true => 0,
                false => gap,
            };
            let tail = shown_width + bell.map_or(0, |bell| bell_gap + halves(&bell.text));
            // Padding in whole spaces, and a thin one when the tail ends on a
            // half cell, so the bell still sits flush with the right margin.
            let padding = total.saturating_sub(used + tail + RIGHT_MARGIN * HALVES_PER_CELL);
            status_text.extend(std::iter::repeat_n(' ', padding / HALVES_PER_CELL));
            if !padding.is_multiple_of(HALVES_PER_CELL) {
                push_thinned(&mut status_text, &THIN_SPACE.to_string(), &mut thin);
            }
            let right_start = status_text.len();
            push_thinned(&mut status_text, &shown, &mut thin);
            spans.push((right_start..status_text.len(), muted_attrs.clone()));
            let start = used + padding;
            parts_shown = segment_spans(&kept, start, &texts);
            for (&index, (_, at, _)) in kept.iter().zip(&parts_shown) {
                if let Some(icon) = &parts[index].icon {
                    icons.push((*at, Arc::clone(icon)));
                }
            }
            if let Some(bell) = bell {
                if !shown.is_empty() {
                    status_text.push_str(SEGMENT_SEPARATOR);
                }
                status_text.push_str(&bell.text);
                let bell_start = status_text.len() - bell.text.len();
                let first = start + shown_width + bell_gap;
                parts_shown.push((StatusHover::Bell, first, first + halves(&bell.text)));
                let bell_attrs = match bell.kind {
                    Some(NoticeKind::Error) => error_attrs.clone(),
                    Some(NoticeKind::Warning) => warning_attrs.clone(),
                    Some(NoticeKind::Info) => info_attrs.clone(),
                    Some(NoticeKind::Progress) | None => muted_attrs.clone(),
                };
                spans.push((bell_start..status_text.len(), bell_attrs));
            }
        }

        // Apply attributes to the text buffer line
        let default_attrs = Attrs::new()
            .family(base_family(family))
            .color(text_color.to_glyphon());
        let mut attrs_list = glyphon::AttrsList::new(&default_attrs);
        for (range, attrs) in spans {
            attrs_list.add_span(range, &attrs);
        }
        // A thin space is an ordinary one, a whole cell in a monospace font,
        // drawn half a cell narrower: exactly half a cell, whatever the font.
        let tighten = -(self.cell_width / HALVES_PER_CELL as f32) / self.font_size;
        let thin_attrs = muted_attrs.clone().letter_spacing(tighten);
        for at in thin {
            attrs_list.add_span(at..at + 1, &thin_attrs);
        }

        let ending = glyphon::cosmic_text::LineEnding::default();
        if buffer.lines.is_empty() {
            buffer.lines.push(BufferLine::new(
                &status_text,
                ending,
                attrs_list,
                Shaping::Advanced,
            ));
        } else {
            buffer.lines[0].set_text(&status_text, ending, attrs_list);
        }
        buffer.lines.truncate(1);
        buffer.shape_until_scroll(&mut self.font_system, false);
        StatusColumns {
            icons,
            parts: parts_shown,
        }
    }

    /// Icon `svg` over the slot starting at cell `col` of the bar whose top
    /// edge is `status_top`, in texture slot `slot` (counted from the bar's
    /// left), uploaded again only when the artwork or its size changes there.
    /// `None` when the artwork does not rasterize or the slots have run out.
    pub(in crate::renderer) fn status_icon_placement(
        &mut self,
        slot: usize,
        svg: &Arc<[u8]>,
        col: usize,
        status_top: f32,
    ) -> Option<ImagePlacement> {
        let id = *STATUS_ICON_TEXTURE_IDS.get(slot)?;
        let (top, bottom) = crate::status_bar_content_px(status_top, self.cell_height);
        let size = ((self.cell_height * ICON_HEIGHT_RATIO).round() - 2.0 * crate::STATUS_BAR_PAD_Y)
            .max(1.0) as u32;
        let current = self.status_icon_uploads[slot]
            .as_ref()
            .is_some_and(|(art, px)| Arc::ptr_eq(art, svg) && *px == size);
        if !current {
            self.status_icon_uploads[slot] = None;
            if !self.upload_svg_in_box(id, svg, size, size) {
                return None;
            }
            self.status_icon_uploads[slot] = Some((Arc::clone(svg), size));
        }
        let slot_w = ICON_SLOT.chars().count() as f32 * self.cell_width;
        let half = self.cell_width / HALVES_PER_CELL as f32;
        Some(ImagePlacement {
            alpha: 1.0,
            height: size as f32,
            id,
            u_max: 1.0,
            u_min: 0.0,
            v_max: 1.0,
            v_min: 0.0,
            width: size as f32,
            x: (col as f32 * half + (slot_w - size as f32) / 2.0).round(),
            y: (top + (bottom - top - size as f32) / 2.0).round(),
        })
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The host segment as it stands at the bar's left edge, padded on both
/// sides; empty when there is no host to show.
fn host_segment(host: &str) -> String {
    match host.is_empty() {
        true => String::new(),
        false => format!("{HOST_PADDING}{host}{HOST_PADDING}"),
    }
}

/// The cells `segment` takes: the blank slot its icon is drawn over, then
/// its text, or its short text when `short` and it has one.
fn rendered(segment: &StatusSegment, short: bool) -> String {
    let text = match (short, &segment.short) {
        (true, Some(text)) => text,
        _ => &segment.text,
    };
    match (&segment.icon, text.is_empty()) {
        (None, _) => text.clone(),
        (Some(_), true) => ICON_SLOT.to_string(),
        (Some(_), false) => format!("{ICON_SLOT}{ICON_GAP}{text}"),
    }
}

/// How many half cells `text` takes: two for each character, one for each
/// [`THIN_SPACE`].
fn halves(text: &str) -> usize {
    text.chars()
        .map(|c| match c {
            THIN_SPACE => 1,
            _ => HALVES_PER_CELL,
        })
        .sum()
}

/// Append `text` to `out` with each [`THIN_SPACE`] as an ordinary space,
/// noting in `thin` the byte it is at, for the span that narrows it.
fn push_thinned(out: &mut String, text: &str, thin: &mut Vec<usize>) {
    for c in text.chars() {
        match c {
            THIN_SPACE => {
                thin.push(out.len());
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
}

/// Where each of `shown` lands when the first starts at half cell `col`:
/// its segment's index (from `indices`) and its span `(first half cell, past
/// the last)`, the segments standing [`SEGMENT_SEPARATOR`] apart.
fn segment_spans(
    indices: &[usize],
    col: usize,
    shown: &[String],
) -> Vec<(StatusHover, usize, usize)> {
    let gap = halves(SEGMENT_SEPARATOR);
    let mut at = col;
    indices
        .iter()
        .zip(shown)
        .map(|(&index, text)| {
            let width = halves(text);
            let span = (StatusHover::Segment(index), at, at + width);
            at += width + gap;
            span
        })
        .collect()
}

/// The part among `hits` at the surface point `(x, y)`.
fn hit_at(hits: &[StatusHit], x: f32, y: f32) -> Option<StatusHover> {
    hits.iter()
        .find(|hit| y >= hit.top && x >= hit.left && x < hit.right)
        .map(|hit| hit.part)
}

/// The right side's texts for `room` half cells: `full` when every segment fits
/// as it is, else every segment in its short form, so the bar shows them all
/// shortened before it drops any (see [`kept`]).
fn fitting(full: Vec<String>, parts: &[StatusSegment], room: usize) -> Vec<String> {
    if kept(&full, parts, room).len() == full.len() {
        return full;
    }
    parts.iter().map(|part| rendered(part, true)).collect()
}

/// The indices of `texts` (one per segment of `parts`) still shown in
/// `room` half cells, in order: segments are dropped until the rest, joined,
/// fits, the lowest priority first and among equals the left-most, so with
/// no priorities set the one nearest the edge holds on longest.
fn kept(texts: &[String], parts: &[StatusSegment], room: usize) -> Vec<usize> {
    let mut kept: Vec<usize> = (0..texts.len()).collect();
    let width = |kept: &[usize]| {
        let shown: Vec<&str> = kept.iter().map(|&index| texts[index].as_str()).collect();
        halves(&shown.join(SEGMENT_SEPARATOR))
    };
    while !kept.is_empty() && width(&kept) > room {
        let weakest = kept
            .iter()
            .enumerate()
            .min_by_key(|&(_, &index)| (parts[index].priority, index))
            .map(|(at, _)| at)
            .unwrap_or(0);
        kept.remove(weakest);
    }
    kept
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_right_side_drops_its_first_segments_when_short_of_room() {
        // With no priorities set, a long count further left is the first to
        // go when the notice needs the room.
        let texts = ["340 lines", "12:5", "42%"].map(String::from);
        let parts = texts.clone().map(|text| segment(&text, 0));
        assert_eq!(kept(&texts, &parts, 160), [0, 1, 2]);
        assert_eq!(kept(&texts, &parts, 18), [1, 2]);
        assert_eq!(kept(&texts, &parts, 0), [] as [usize; 0]);
    }

    #[test]
    fn test_the_lowest_priority_is_dropped_first_wherever_it_sits() {
        // The position on the left holds on while the quota to its right,
        // ranked lower, gives way.
        let texts = ["12:5", "17% 30%", "1% 1%"].map(String::from);
        let parts = [segment("12:5", 3), segment("17% 30%", 1), segment("1% 1%", 2)];
        assert_eq!(kept(&texts, &parts, 30), [0, 2]);
        assert_eq!(kept(&texts, &parts, 8), [0]);
    }

    fn segment(text: &str, priority: u32) -> StatusSegment {
        StatusSegment {
            icon: None,
            priority,
            short: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn test_a_segment_reads_as_its_icon_s_slot_then_its_text() {
        let logo: Arc<[u8]> = Arc::from(&b"<svg/>"[..]);
        let quota = StatusSegment {
            icon: Some(Arc::clone(&logo)),
            priority: 0,
            short: Some("42%".to_string()),
            text: "42% 14:05".to_string(),
        };
        let tool = StatusSegment {
            icon: Some(logo),
            priority: 0,
            short: None,
            text: String::new(),
        };
        assert_eq!(rendered(&quota, false), "   42% 14:05");
        assert_eq!(rendered(&quota, true), "   42%");
        assert_eq!(
            rendered(&tool, false),
            ICON_SLOT,
            "a bare icon is just its slot"
        );
    }

    #[test]
    fn test_a_bar_short_of_room_shortens_every_segment_before_dropping_one() {
        // Regression: two quotas side by side did not fit a narrow window,
        // and the first was dropped whole while a shorter form would have
        // left room for both.
        let quota = |text: &str, short: &str| StatusSegment {
            icon: None,
            priority: 0,
            short: Some(short.to_string()),
            text: text.to_string(),
        };
        let parts = [
            quota("17% 18:00 | 30% Tue 03:00", "17% 30%"),
            quota("1% 22:08 | 1% Fri 15:42", "1% 1%"),
        ];
        let full: Vec<String> = parts.iter().map(|part| rendered(part, false)).collect();
        assert_eq!(
            fitting(full.clone(), &parts, 160),
            full,
            "room for all as they are"
        );
        let narrow = fitting(full, &parts, 40);
        assert_eq!(narrow, ["17% 30%", "1% 1%"]);
        assert_eq!(kept(&narrow, &parts, 40), [0, 1], "both still shown");
    }

    #[test]
    fn test_each_shown_segment_keeps_its_own_index_and_cells() {
        // Segments dropped for room still count: the first shown is
        // segment 1 here, so a hover lights the one under the pointer rather
        // than the one before it.
        let shown = ["12:5", "340 lines"].map(String::from);
        let spans = segment_spans(&[1, 2], 100, &shown);
        assert_eq!(
            spans,
            [
                (StatusHover::Segment(1), 100, 108),
                (StatusHover::Segment(2), 112, 130),
            ],
            "the separator's cells belong to neither"
        );
    }

    #[test]
    fn test_the_pointer_finds_the_part_it_is_on_and_none_between_them() {
        let hits = [
            StatusHit {
                left: 10.0,
                part: StatusHover::Segment(0),
                right: 40.0,
                top: 100.0,
            },
            StatusHit {
                left: 60.0,
                part: StatusHover::Bell,
                right: 70.0,
                top: 100.0,
            },
        ];
        assert_eq!(hit_at(&hits, 20.0, 105.0), Some(StatusHover::Segment(0)));
        assert_eq!(hit_at(&hits, 65.0, 105.0), Some(StatusHover::Bell));
        assert_eq!(hit_at(&hits, 50.0, 105.0), None, "between two parts");
        assert_eq!(hit_at(&hits, 20.0, 90.0), None, "above the bar");
    }

    #[test]
    fn test_a_thin_space_takes_half_a_cell_and_is_drawn_as_an_ordinary_one() {
        assert_eq!(halves("17%\u{2009}18:00"), 17, "eight cells and a half");
        let mut out = String::from("ab");
        let mut thin = Vec::new();
        push_thinned(&mut out, "1%\u{2009}5%", &mut thin);
        assert_eq!(out, "ab1% 5%");
        assert_eq!(thin, [4], "the byte the narrowed space is at");
    }
}
