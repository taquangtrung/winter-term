//! The status bar: its background and prompt caret quads, and its one
//! shaped line (where commands run, the mode, a prompt or the live search, a
//! transient notice, then at the right edge its segments and the bell).

use std::sync::Arc;

use glyphon::{Attrs, BufferLine, Metrics, Shaping};

use crate::image::ImagePlacement;
use crate::renderer::background::{quad_vertices, BgVertex};
use crate::renderer::glyphs::{base_family, effective_bold_weight, parse_weight};
use crate::renderer::raster::paint::{buffer_width, ui_family};
use crate::renderer::raster::STATUS_ICON_TEXTURE_IDS;
use crate::renderer::{FrameHits, GpuRenderer, StatusBar, StatusHover, StatusSegment, THIN_SPACE};

// ========================================================================
// Constants
// ========================================================================

/// The status bar's glyph for a project: git icon (Nerd Font `dev-git` \u{e702}).
const PROJECT_ICON: char = '\u{e702}';

/// The status bar's glyph for a version control branch: codicon `git-branch`.
const BRANCH_ICON: char = '\u{ea68}';

/// What stands between two segments on the status bar.
const STATUS_GAP: &str = "  ";

/// Pixels left empty after the status bar's right text, at its right edge.
const RIGHT_MARGIN_PX: f32 = 12.0;

/// The fewest pixels kept between the status bar's left text and its right.
const MIN_GAP_PX: f32 = 16.0;

/// The cells the editor's icon is drawn over, kept blank in the text: the
/// artwork is square and a cell is about half as wide as it is tall.
const ICON_SLOT: &str = "  ";

/// What stands between the right-hand segments of the status bar.
const SEGMENT_SEPARATOR: &str = "  ";

/// What stands between an icon's slot and the text it heads.
const ICON_GAP: &str = " ";

/// The right side is laid out in half cells, the width of a [`THIN_SPACE`].
const HALVES_PER_CELL: usize = 2;

/// How much the status bar font size and line height scale down relative
/// to the terminal grid's font metrics (13.5px statusline on default 15px terminal).
pub(in crate::renderer) const STATUS_FONT_RATIO: f32 = 13.5 / 15.0;

/// Smallest allowed status bar font size in logical pixels.
const MIN_STATUS_FONT_SIZE: f32 = 9.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Where the status bar's line put the parts drawn or clicked over it.
pub(in crate::renderer) struct StatusColumns {
    /// Caret position in surface pixels `(x, width)` when a prompt is active.
    pub(in crate::renderer) cursor: Option<(f32, f32)>,
    /// Each icon shown, with its pixel X coordinate on the bar, in order from the left.
    pub(in crate::renderer) icons: Vec<(f32, Arc<[u8]>)>,
    /// Each part of the right side shown, with its span `(part, left_px, right_px)`.
    pub(in crate::renderer) parts: Vec<(StatusHover, f32, f32)>,
    /// The X coordinate where the right-hand buffer begins.
    pub(in crate::renderer) right_x: f32,
}

/// One half of the project segment as the line spells it: its text, the
/// icon drawn before it when there is one, the glyph standing in when there
/// is not, and how it is styled.
struct SegmentPart<'a, 's> {
    attrs: Attrs<'a>,
    fallback: char,
    fallback_attrs: Attrs<'a>,
    icon: Option<&'s Arc<[u8]>>,
    text: &'s str,
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
// SegmentPart Implementations
// ========================================================================

impl<'a, 's> SegmentPart<'a, 's> {
    /// The project's name, behind the project glyph when it has no icon.
    fn project(
        text: &'s str,
        icon: Option<&'s Arc<[u8]>>,
        attrs: Attrs<'a>,
        fallback_attrs: Attrs<'a>,
    ) -> Self {
        Self {
            attrs,
            fallback: PROJECT_ICON,
            fallback_attrs,
            icon,
            text,
        }
    }

    /// The branch's name, behind the branch glyph when it has no icon.
    fn branch(
        text: &'s str,
        icon: Option<&'s Arc<[u8]>>,
        attrs: Attrs<'a>,
        fallback_attrs: Attrs<'a>,
    ) -> Self {
        Self {
            attrs,
            fallback: BRANCH_ICON,
            fallback_attrs,
            icon,
            text,
        }
    }
}

// ========================================================================
// GpuRenderer: status bar
// ========================================================================

impl GpuRenderer {
    /// Append the status bar's background band, below its top edge, which is
    /// drawn as the bottom border of the panes resting on it (see
    /// `background::divider`).
    pub(in crate::renderer) fn push_status_bar_quads(
        &self,
        status_top: f32,
        (surface_w, surface_h): (f32, f32),
        out: &mut Vec<BgVertex>,
    ) {
        out.extend_from_slice(&quad_vertices(
            0.0,
            status_top + 1.0,
            surface_w,
            status_top + crate::status_bar_height_px(self.fonts.cell_height),
            self.theme.background.as_linear(),
            surface_w,
            surface_h,
        ));
    }

    /// Append the prompt caret quad if one was shaped.
    pub(in crate::renderer) fn push_status_cursor_quad(
        &self,
        columns: &StatusColumns,
        status_top: f32,
        (surface_w, surface_h): (f32, f32),
        out: &mut Vec<BgVertex>,
    ) {
        if let Some((cursor_x, cursor_w)) = columns.cursor {
            let (top, bottom) = crate::status_bar_content_px(status_top, self.fonts.cell_height);
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
        self.hits.status_hits = columns
            .parts
            .iter()
            .map(|&(part, left, right)| StatusHit {
                left,
                part,
                right,
                top: status_top,
            })
            .collect();
        let Some(hit) = status
            .hovered
            .and_then(|hovered| self.hits.status_hits.iter().find(|hit| hit.part == hovered))
        else {
            return;
        };
        let pad = 4.0;
        let (top, bottom) = crate::status_bar_content_px(status_top, self.fonts.cell_height);
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

    /// Shape the status bar's lines: the left side in `left_buffer` (mode,
    /// host, project, branch, search/prompt/notice) and the right side in
    /// `right_buffer` (segments, bell), using the proportional UI font.
    pub(in crate::renderer) fn shape_status_bar(
        &mut self,
        status: &StatusBar,
        family: Option<&str>,
        surface_width: f32,
        left_buffer: &mut glyphon::Buffer,
        right_buffer: &mut glyphon::Buffer,
    ) -> StatusColumns {
        let metrics = status_metrics(self.fonts.font_size, self.fonts.line_height);
        left_buffer.set_metrics(metrics);
        right_buffer.set_metrics(metrics);

        let font_family = ui_family(&self.font_system);
        let icon_family = base_family(family);
        let accent_attrs = Attrs::new()
            .family(font_family)
            .weight(effective_bold_weight(
                self.fonts.bold_weight.as_deref(),
                true,
            ))
            .color(status.accent.to_glyphon());
        let text_color = self.theme.foreground;
        let muted_attrs = Attrs::new()
            .family(font_family)
            .weight(parse_weight(
                self.fonts.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ))
            .color(text_color.to_glyphon());
        let info_attrs = Attrs::new()
            .family(font_family)
            .weight(effective_bold_weight(
                self.fonts.bold_weight.as_deref(),
                true,
            ))
            .color(self.theme.ansi[4].to_glyphon());
        let not_found_attrs = muted_attrs.clone().color(self.theme.ansi[3].to_glyphon());

        let project_attrs = Attrs::new()
            .family(font_family)
            .weight(parse_weight(
                self.fonts.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ))
            .color(self.theme.foreground.to_glyphon());
        let project_icon_attrs = project_attrs.clone().family(icon_family);
        let branch_attrs = Attrs::new()
            .family(font_family)
            .weight(parse_weight(
                self.fonts.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ))
            .color(self.theme.foreground.to_glyphon());
        let branch_icon_attrs = branch_attrs.clone().family(icon_family);

        let mut left_text = String::new();
        let mut left_spans = Vec::new();
        let mut left_svg_icons: Vec<(usize, Arc<[u8]>)> = Vec::new();

        let mut mode_range = None;
        if !status.mode.is_empty() {
            let mode_start = left_text.len();
            left_text.push_str(&status.mode);
            let mode_end = left_text.len();
            left_spans.push((mode_start..mode_end, accent_attrs.clone()));
            mode_range = Some((mode_start, mode_end));
        }

        let host = host_segment(&status.host);
        if !host.is_empty() {
            if !left_text.is_empty() {
                left_text.push_str(STATUS_GAP);
            }
            let host_start = left_text.len();
            left_text.push_str(&host);
            let host_end = left_text.len();
            if let Some(first_char) = host.chars().next() {
                if first_char == '\u{eb3a}' || first_char == '\u{f0868}' {
                    let icon_len = first_char.len_utf8();
                    let host_icon_attrs = info_attrs.clone().family(icon_family);
                    left_spans.push((host_start..host_start + icon_len, host_icon_attrs));
                    if host_start + icon_len < host_end {
                        left_spans.push((host_start + icon_len..host_end, info_attrs.clone()));
                    }
                } else {
                    left_spans.push((host_start..host_end, info_attrs.clone()));
                }
            }
        }

        let has_project_or_branch = !status.project.is_empty() || !status.branch.is_empty();
        if has_project_or_branch {
            let mut project_svg_indices = Vec::new();
            push_project_segment(
                &mut left_text,
                &mut left_spans,
                &mut project_svg_indices,
                SegmentPart::project(
                    &status.project,
                    status.project_icon.as_ref(),
                    project_attrs,
                    project_icon_attrs,
                ),
                SegmentPart::branch(
                    &status.branch,
                    status.branch_icon.as_ref(),
                    branch_attrs,
                    branch_icon_attrs,
                ),
            );
            for (icon_half_cell, svg) in project_svg_indices {
                let char_idx = icon_half_cell / HALVES_PER_CELL;
                let byte_offset = left_text.char_indices().nth(char_idx).map_or(left_text.len(), |(b, _)| b);
                left_svg_icons.push((byte_offset, svg));
            }
        }

        let mut prompt_caret_byte: Option<(usize, usize)> = None;

        if let Some(ref prompt) = status.prompt {
            if !left_text.is_empty() {
                if !left_text.ends_with(' ') {
                    left_text.push(' ');
                }
                left_text.push(' ');
            }

            let label_start = left_text.len();
            left_text.push_str(&prompt.label);
            let label_end = left_text.len();
            left_spans.push((label_start..label_end, accent_attrs.clone()));

            let value_chars: Vec<char> = prompt.value.chars().collect();
            let caret = prompt.caret.min(value_chars.len());

            if caret > 0 {
                let before_str: String = value_chars[..caret].iter().collect();
                let b_start = left_text.len();
                left_text.push_str(&before_str);
                let b_end = left_text.len();
                left_spans.push((b_start..b_end, info_attrs.clone()));
            }

            let cursor_text_attrs = Attrs::new()
                .family(font_family)
                .weight(effective_bold_weight(
                    self.fonts.bold_weight.as_deref(),
                    true,
                ))
                .color(self.theme.background.to_glyphon());

            if caret < value_chars.len() {
                let c = value_chars[caret];
                let c_start = left_text.len();
                left_text.push(c);
                let c_end = left_text.len();
                left_spans.push((c_start..c_end, cursor_text_attrs));
                prompt_caret_byte = Some((c_start, c_end));

                if caret + 1 < value_chars.len() {
                    let after_str: String = value_chars[caret + 1..].iter().collect();
                    let a_start = left_text.len();
                    left_text.push_str(&after_str);
                    let a_end = left_text.len();
                    left_spans.push((a_start..a_end, info_attrs.clone()));
                }
            } else {
                let s_start = left_text.len();
                left_text.push(' ');
                let s_end = left_text.len();
                left_spans.push((s_start..s_end, cursor_text_attrs));
                prompt_caret_byte = Some((s_start, s_end));
            }

            if let Some(ref search) = status.search {
                if search.match_total > 0 {
                    let count_start = left_text.len();
                    left_text.push_str(&format!(
                        "  [{}/{}]",
                        search.match_index, search.match_total
                    ));
                    let count_end = left_text.len();
                    left_spans.push((count_start..count_end, muted_attrs.clone()));
                } else if !prompt.value.is_empty() {
                    let none_start = left_text.len();
                    left_text.push_str("  [no matches]");
                    let none_end = left_text.len();
                    left_spans.push((none_start..none_end, not_found_attrs.clone()));
                }
            }
        } else if let Some(ref search) = status.search {
            if !left_text.is_empty() {
                if !left_text.ends_with(' ') {
                    left_text.push(' ');
                }
                left_text.push(' ');
            }

            let query_start = left_text.len();
            left_text.push(if search.reverse { '?' } else { '/' });
            left_text.push_str(&search.query);
            let query_end = left_text.len();
            left_spans.push((query_start..query_end, muted_attrs.clone()));

            if search.match_total > 0 {
                let count_start = left_text.len();
                left_text.push_str(&format!(
                    "  [{}/{}]",
                    search.match_index, search.match_total
                ));
                let count_end = left_text.len();
                left_spans.push((count_start..count_end, muted_attrs.clone()));
            } else if !search.query.is_empty() {
                let none_start = left_text.len();
                left_text.push_str("  [no matches]");
                let none_end = left_text.len();
                left_spans.push((none_start..none_end, not_found_attrs.clone()));
            }
        } else if let Some(ref notice) = status.notice {
            if !left_text.is_empty() {
                if !left_text.ends_with(' ') {
                    left_text.push(' ');
                }
                left_text.push(' ');
            }

            let notice_start = left_text.len();
            left_text.push_str(&notice.text);
            let notice_end = left_text.len();
            left_spans.push((notice_start..notice_end, muted_attrs.clone()));
        }

        let default_attrs = Attrs::new()
            .family(font_family)
            .color(text_color.to_glyphon());
        let mut left_attrs_list = glyphon::AttrsList::new(&default_attrs);
        for (range, attrs) in left_spans {
            left_attrs_list.add_span(range, &attrs);
        }

        let ending = glyphon::cosmic_text::LineEnding::default();
        if left_buffer.lines.is_empty() {
            left_buffer.lines.push(BufferLine::new(
                &left_text,
                ending,
                left_attrs_list,
                Shaping::Advanced,
            ));
        } else {
            left_buffer.lines[0].set_text(&left_text, ending, left_attrs_list);
        }
        left_buffer.lines.truncate(1);
        left_buffer.shape_until_scroll(&mut self.font_system, false);

        let mut cursor = None;
        if let Some((c_start, c_end)) = prompt_caret_byte {
            if let Some(run) = left_buffer.layout_runs().next() {
                let mut found_glyph = None;
                for glyph in run.glyphs.iter() {
                    if glyph.start >= c_start && glyph.end <= c_end {
                        found_glyph = Some((glyph.x, glyph.w.max(8.0)));
                        break;
                    }
                }
                cursor = found_glyph.or(Some((run.line_w, 8.0)));
            }
        }

        let mut icons = Vec::new();
        if let Some(run) = left_buffer.layout_runs().next() {
            for (byte_offset, svg) in left_svg_icons {
                for glyph in run.glyphs.iter() {
                    if glyph.start == byte_offset {
                        icons.push((glyph.x, svg));
                        break;
                    }
                }
            }
        }

        let mut parts_shown = Vec::new();
        if let Some((m_start, m_end)) = mode_range {
            if let Some(run) = left_buffer.layout_runs().next() {
                let mut start_x = None;
                let mut end_x = None;
                for glyph in run.glyphs.iter() {
                    if glyph.start >= m_start && glyph.end <= m_end {
                        if start_x.is_none() {
                            start_x = Some(glyph.x);
                        }
                        end_x = Some(glyph.x + glyph.w);
                    }
                }
                if let (Some(sx), Some(ex)) = (start_x, end_x) {
                    parts_shown.push((StatusHover::Mode, 0.0, ex.max(sx + self.fonts.cell_width)));
                }
            }
        }

        let parts = &status.segments;
        let right: Vec<String> = parts.iter().map(|part| rendered(part, false)).collect();
        let bell = status.bell.as_ref();

        let left_w = buffer_width(left_buffer);
        let avail_right_w = (surface_width - left_w - MIN_GAP_PX - RIGHT_MARGIN_PX).max(0.0);
        let mut right_x = surface_width;

        if !right.is_empty() || bell.is_some() {
            let est_char_w = (self.fonts.cell_width * STATUS_FONT_RATIO * 0.5).max(1.0);
            let room_halves = (avail_right_w / est_char_w).floor() as usize;
            let right_texts = fitting(right, parts, room_halves);
            let kept = kept(&right_texts, parts, room_halves);

            let mut right_text = String::new();
            let mut right_spans = Vec::new();
            let mut right_ranges = Vec::new();
            let mut right_svg_icons = Vec::new();

            for (i, &idx) in kept.iter().enumerate() {
                if i > 0 {
                    right_text.push_str(SEGMENT_SEPARATOR);
                }
                let seg_start = right_text.len();
                if let Some(icon) = &parts[idx].icon {
                    right_svg_icons.push((right_text.len(), Arc::clone(icon)));
                    let text = &right_texts[idx];
                    let slot_len = if text.len() > ICON_SLOT.len() {
                        ICON_SLOT.len() + ICON_GAP.len()
                    } else {
                        ICON_SLOT.len()
                    };
                    let icon_attrs = muted_attrs.clone().family(icon_family);
                    right_spans.push((seg_start..seg_start + slot_len, icon_attrs));
                }
                right_text.push_str(&right_texts[idx]);
                let seg_end = right_text.len();
                right_ranges.push((StatusHover::Segment(idx), seg_start, seg_end));
            }

            let bell_range = if let Some(bell) = bell {
                if !right_text.is_empty() {
                    right_text.push_str(SEGMENT_SEPARATOR);
                }
                let bell_start = right_text.len();
                right_text.push_str(&bell.text);
                let bell_end = right_text.len();
                let bell_attrs = muted_attrs.clone();
                if let Some(first_char) = bell.text.chars().next() {
                    if first_char == '\u{eaa2}' || first_char == '\u{eb9a}' {
                        let icon_len = first_char.len_utf8();
                        let bell_icon_attrs = bell_attrs.clone().family(icon_family);
                        right_spans.push((bell_start..bell_start + icon_len, bell_icon_attrs));
                        if bell_start + icon_len < bell_end {
                            right_spans.push((bell_start + icon_len..bell_end, bell_attrs));
                        }
                    } else {
                        right_spans.push((bell_start..bell_end, bell_attrs));
                    }
                }
                Some((StatusHover::Bell, bell_start, bell_end))
            } else {
                None
            };

            let default_right_attrs = muted_attrs.clone();
            let mut right_attrs_list = glyphon::AttrsList::new(&default_right_attrs);
            for (range, attrs) in right_spans {
                right_attrs_list.add_span(range, &attrs);
            }

            if right_buffer.lines.is_empty() {
                right_buffer.lines.push(BufferLine::new(
                    &right_text,
                    ending,
                    right_attrs_list,
                    Shaping::Advanced,
                ));
            } else {
                right_buffer.lines[0].set_text(&right_text, ending, right_attrs_list);
            }
            right_buffer.lines.truncate(1);
            right_buffer.shape_until_scroll(&mut self.font_system, false);

            let right_w = buffer_width(right_buffer);
            right_x = (surface_width - right_w - RIGHT_MARGIN_PX).max(left_w + MIN_GAP_PX);

            if let Some(run) = right_buffer.layout_runs().next() {
                for (hover_kind, start_byte, end_byte) in right_ranges {
                    let mut start_x = None;
                    let mut end_x = None;
                    for glyph in run.glyphs.iter() {
                        if glyph.start >= start_byte && glyph.end <= end_byte {
                            if start_x.is_none() {
                                start_x = Some(glyph.x);
                            }
                            end_x = Some(glyph.x + glyph.w);
                        }
                    }
                    if let (Some(sx), Some(ex)) = (start_x, end_x) {
                        parts_shown.push((hover_kind, right_x + sx, right_x + ex));
                    }
                }

                if let Some((hover_kind, start_byte, end_byte)) = bell_range {
                    let mut start_x = None;
                    let mut end_x = None;
                    for glyph in run.glyphs.iter() {
                        if glyph.start >= start_byte && glyph.end <= end_byte {
                            if start_x.is_none() {
                                start_x = Some(glyph.x);
                            }
                            end_x = Some(glyph.x + glyph.w);
                        }
                    }
                    if let (Some(sx), Some(ex)) = (start_x, end_x) {
                        parts_shown.push((hover_kind, right_x + sx, right_x + ex));
                    }
                }

                for (byte_offset, svg) in right_svg_icons {
                    for glyph in run.glyphs.iter() {
                        if glyph.start == byte_offset {
                            icons.push((right_x + glyph.x, svg));
                            break;
                        }
                    }
                }
            }
        } else {
            right_buffer.lines.clear();
        }

        StatusColumns {
            cursor,
            icons,
            parts: parts_shown,
            right_x,
        }
    }

    /// Icon `svg` over the slot starting at pixel `x_px` of the bar whose top
    /// edge is `status_top`, in texture slot `slot` (counted from the bar's
    /// left), uploaded again only when the artwork or its size changes there.
    /// `None` when the artwork does not rasterize or the slots have run out.
    pub(in crate::renderer) fn status_icon_placement(
        &mut self,
        slot: usize,
        svg: &Arc<[u8]>,
        x_px: f32,
        status_top: f32,
    ) -> Option<ImagePlacement> {
        let id = *STATUS_ICON_TEXTURE_IDS.get(slot)?;
        let (top, bottom) = crate::status_bar_content_px(status_top, self.fonts.cell_height);
        let status_font_size = (self.fonts.font_size * STATUS_FONT_RATIO).round();
        let size = (status_font_size.max(1.0)) as u32;
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
        let status_cell_w = self.fonts.cell_width * STATUS_FONT_RATIO;
        let slot_w = (status_cell_w * 2.0).round().max(size as f32);
        Some(ImagePlacement {
            alpha: 1.0,
            height: size as f32,
            id,
            u_max: 1.0,
            u_min: 0.0,
            v_max: 1.0,
            v_min: 0.0,
            width: size as f32,
            x: (x_px + (slot_w - size as f32) / 2.0).round(),
            y: (top + (bottom - top - size as f32) / 2.0).round(),
        })
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Font metrics for the status bar's line, scaled down from the terminal's
/// font size and line height.
impl GpuRenderer {
    /// Where the bar whose top edge is `status_top` draws `buffer`'s line
    /// from: so the font's whole height, from its ascent down to its
    /// descent, sits in the middle of the bar below its border, as VS Code
    /// centres its status bar's text. A glyph reaching below the baseline,
    /// as `|` does, stays clear of the bar's foot that way. Measured on the
    /// font the line's first letter or digit was shaped in, since an icon
    /// ahead of it may be another font's.
    pub(in crate::renderer) fn status_text_top(&mut self, buffer: &glyphon::Buffer, status_top: f32) -> f32 {
        let (band_top, band_bottom) = crate::status_bar_content_px(status_top, self.fonts.cell_height);
        let line_height = buffer.metrics().line_height;
        let line_box = (band_top + (band_bottom - band_top - line_height) / 2.0).round();
        let Some(run) = buffer.layout_runs().next() else {
            return line_box;
        };
        let letter = run
            .glyphs
            .iter()
            .find(|glyph| run.text.get(glyph.start..glyph.end).is_some_and(|text| text.chars().any(char::is_alphanumeric)))
            .or_else(|| run.glyphs.first());
        let Some(glyph) = letter else {
            return line_box;
        };
        let Some(font) = self.font_system.get_font(glyph.font_id, glyph.font_weight) else {
            return line_box;
        };
        let metrics = font.as_swash().metrics(&[]).scale(glyph.font_size);
        if metrics.ascent + metrics.descent <= 0.0 {
            return line_box;
        }
        centered_text_top(status_top, self.fonts.cell_height, (metrics.ascent, metrics.descent), run.line_y)
    }
}

/// Where a line whose baseline sits `line_y` below its top is drawn from,
/// so a font reaching `ascent` above its baseline and `descent` below it
/// stands in the middle of the row of contents of the bar whose top edge
/// is `status_top`, in a window whose cells are `cell_height` tall.
///
/// A whole pixel, and the baseline offset taken rounded: the text renderer
/// truncates the top and rounds the offset, so a fractional top drew the
/// letters up to a pixel above where they were meant to stand.
fn centered_text_top(status_top: f32, cell_height: f32, (ascent, descent): (f32, f32), line_y: f32) -> f32 {
    let (contents_top, contents_bottom) = crate::status_bar_content_px(status_top, cell_height);
    let middle = (contents_top + contents_bottom) / 2.0;
    let baseline = (middle + (ascent - descent) / 2.0).round();
    baseline - line_y.round()
}

pub(in crate::renderer) fn status_metrics(font_size: f32, line_height: f32) -> Metrics {
    let status_font_size = ((font_size * STATUS_FONT_RATIO * 2.0).round() / 2.0).max(MIN_STATUS_FONT_SIZE);
    let status_line_height = (line_height * STATUS_FONT_RATIO).round().max(status_font_size);
    Metrics::new(status_font_size, status_line_height)
}

/// Append the project name and VC branch to the status bar text and spans.
fn push_project_segment<'a>(
    status_text: &mut String,
    spans: &mut Vec<(std::ops::Range<usize>, Attrs<'a>)>,
    icons: &mut Vec<(usize, Arc<[u8]>)>,
    project: SegmentPart<'a, '_>,
    branch: SegmentPart<'a, '_>,
) {
    if project.text.is_empty() && branch.text.is_empty() {
        return;
    }
    status_text.push(' ');
    let had_project = !project.text.is_empty();
    if had_project {
        push_segment_part(status_text, spans, icons, project);
    }
    if !branch.text.is_empty() {
        if had_project {
            status_text.push(' ');
        }
        push_segment_part(status_text, spans, icons, branch);
    }
    status_text.push(' ');
}

/// Append one half of the project segment: its icon, or its glyph where it
/// has none, then its text, styled as one span.
fn push_segment_part<'a>(
    status_text: &mut String,
    spans: &mut Vec<(std::ops::Range<usize>, Attrs<'a>)>,
    icons: &mut Vec<(usize, Arc<[u8]>)>,
    part: SegmentPart<'a, '_>,
) {
    match part.icon {
        Some(svg) => {
            let icon_col = status_text.chars().count() * HALVES_PER_CELL;
            icons.push((icon_col, Arc::clone(svg)));
            let slot_start = status_text.len().saturating_sub(1);
            status_text.push_str(ICON_SLOT);
            status_text.push_str(ICON_GAP);
            let slot_end = status_text.len();
            spans.push((slot_start..slot_end, part.fallback_attrs));
            let text_start = status_text.len();
            status_text.push_str(part.text);
            spans.push((text_start..status_text.len(), part.attrs));
        }
        None => {
            let icon_start = status_text.len().saturating_sub(1);
            status_text.push(part.fallback);
            status_text.push(' ');
            let icon_end = status_text.len();
            spans.push((icon_start..icon_end, part.fallback_attrs));
            let text_start = status_text.len();
            status_text.push_str(part.text);
            spans.push((text_start..status_text.len(), part.attrs));
        }
    }
}

/// The host segment as it stands at the bar's left edge, padded on both
/// sides; empty when there is no host to show.
fn host_segment(host: &str) -> String {
    match host.is_empty() {
        true => String::new(),
        false => format!(" {host} "),
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
#[cfg(test)]
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
#[cfg(test)]
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
// FrameHits
// ========================================================================

impl FrameHits {
    /// The pane toolbar button at the surface point `(x, y)`, as the bars were
    /// last drawn: whose bar it is, and what the button is told back as.
    pub fn toolbar_button_at(&self, x: f32, y: f32) -> Option<(u64, u16)> {
        self.toolbar_hits
            .iter()
            .filter(|(_, (top, bottom), _)| y >= *top && y < *bottom)
            .find_map(|(owner, _, hits)| {
                hits.iter()
                    .find(|hit| x >= hit.left && x < hit.right)
                    .map(|hit| (*owner, hit.button))
            })
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
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_text_stands_on_a_whole_pixel_with_the_whole_line_centred() {
        // The bar's row of contents, as at 15 px with 20 px cells; Noto
        // Sans's ascent and descent at 13.5 px; the baseline
        // offset fractional, as the shaper gives it.
        let (status_top, cell_height, (ascent, descent), line_y) = (100.0, 20.0, (14.43, 3.96), 14.238);
        let top = centered_text_top(status_top, cell_height, (ascent, descent), line_y);
        assert_eq!(top.fract(), 0.0, "the renderer would truncate a fraction away");
        // Where the renderer puts the baseline: the top as is, the offset rounded.
        let baseline = top + line_y.round();
        let line_middle = baseline - (ascent - descent) / 2.0;
        let (contents_top, contents_bottom) = crate::status_bar_content_px(status_top, cell_height);
        let bar_middle = (contents_top + contents_bottom) / 2.0;
        assert!((line_middle - bar_middle).abs() <= 0.5, "the line centred: {line_middle} vs {bar_middle}");
    }

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
        let parts = [
            segment("12:5", 3),
            segment("17% 30%", 1),
            segment("1% 1%", 2),
        ];
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
        let editor = StatusSegment {
            icon: Some(logo),
            priority: 0,
            short: None,
            text: String::new(),
        };
        assert_eq!(rendered(&quota, false), "   42% 14:05");
        assert_eq!(rendered(&quota, true), "   42%");
        assert_eq!(
            rendered(&editor, false),
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
                left: 0.0,
                part: StatusHover::Mode,
                right: 8.0,
                top: 100.0,
            },
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
        assert_eq!(hit_at(&hits, 4.0, 105.0), Some(StatusHover::Mode));
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

    #[test]
    fn test_push_project_segment() {
        let p_attrs = Attrs::new();
        let b_attrs = Attrs::new();
        let dummy_svg: Arc<[u8]> = Arc::from(&b"<svg/>"[..]);

        // Empty both
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut icons = Vec::new();
        push_project_segment(
            &mut text,
            &mut spans,
            &mut icons,
            SegmentPart::project("", None, p_attrs.clone(), p_attrs.clone()),
            SegmentPart::branch("", None, b_attrs.clone(), b_attrs.clone()),
        );
        assert_eq!(text, "");
        assert_eq!(spans.len(), 0);
        assert_eq!(icons.len(), 0);

        // Project only (codicon)
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut icons = Vec::new();
        push_project_segment(
            &mut text,
            &mut spans,
            &mut icons,
            SegmentPart::project("my-proj", None, p_attrs.clone(), p_attrs.clone()),
            SegmentPart::branch("", None, b_attrs.clone(), b_attrs.clone()),
        );
        assert_eq!(text, format!(" {PROJECT_ICON} my-proj "));
        assert_eq!(spans.len(), 2);
        assert_eq!(icons.len(), 0);

        // Project only (SVG)
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut icons = Vec::new();
        push_project_segment(
            &mut text,
            &mut spans,
            &mut icons,
            SegmentPart::project("my-proj", Some(&dummy_svg), p_attrs.clone(), p_attrs.clone()),
            SegmentPart::branch("", None, b_attrs.clone(), b_attrs.clone()),
        );
        assert_eq!(text, format!(" {ICON_SLOT}{ICON_GAP}my-proj "));
        assert_eq!(spans.len(), 2);
        assert_eq!(icons.len(), 1);
        assert_eq!(icons[0].0, 2); // 1 leading space * 2 half cells = 2

        // Branch only (codicon)
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut icons = Vec::new();
        push_project_segment(
            &mut text,
            &mut spans,
            &mut icons,
            SegmentPart::project("", None, p_attrs.clone(), p_attrs.clone()),
            SegmentPart::branch("main", None, b_attrs.clone(), b_attrs.clone()),
        );
        assert_eq!(text, format!(" {BRANCH_ICON} main "));
        assert_eq!(spans.len(), 2);
        assert_eq!(icons.len(), 0);

        // Project and branch (codicons)
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut icons = Vec::new();
        push_project_segment(
            &mut text,
            &mut spans,
            &mut icons,
            SegmentPart::project("my-proj", None, p_attrs.clone(), p_attrs.clone()),
            SegmentPart::branch("main", None, b_attrs.clone(), b_attrs.clone()),
        );
        assert_eq!(text, format!(" {PROJECT_ICON} my-proj {BRANCH_ICON} main "));
        assert_eq!(spans.len(), 4);
        assert_eq!(icons.len(), 0);

        // Project and branch (both SVG)
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut icons = Vec::new();
        push_project_segment(
            &mut text,
            &mut spans,
            &mut icons,
            SegmentPart::project("my-proj", Some(&dummy_svg), p_attrs.clone(), p_attrs.clone()),
            SegmentPart::branch("main", Some(&dummy_svg), b_attrs.clone(), b_attrs),
        );
        assert_eq!(
            text,
            format!(" {ICON_SLOT}{ICON_GAP}my-proj {ICON_SLOT}{ICON_GAP}main ")
        );
        assert_eq!(spans.len(), 4);
        assert_eq!(icons.len(), 2);
    }

    #[test]
    fn test_status_metrics_scales_down_from_terminal_grid() {
        let metrics = status_metrics(15.0, 20.0);
        assert!(metrics.font_size < 15.0 && metrics.line_height < 20.0);

        let small = status_metrics(8.0, 10.0);
        assert_eq!(small.font_size, MIN_STATUS_FONT_SIZE);
        assert!(small.line_height >= small.font_size);
    }
}

