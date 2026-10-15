// Cursor mode: a vim cursor that lives in the document's text rather than in
// the window over it.
//
// pdf.js gives a page's text as absolutely-positioned glyph runs with no
// shared line box, so there is nothing for a browser's own word and line
// boundaries to work from. This builds its own model instead: the page cut
// into blocks along the empty bands that run through it, so a two-column
// paper reads down one column and then down the other rather than across the
// gutter; each block's runs clustered into lines by how close their baselines
// are, joined with a synthetic space wherever the gap between two runs is
// wide enough to have been one; and each character given a box, mapped
// through pdf.js's own viewport, so the cursor has somewhere to sit on a
// rotated page as well as an upright one.
//
// Everything here takes an already-resolved motion and repeat count. The vim
// grammar that produces them, the counts, prefixes, and operators, lives in
// the host's page, so the two cannot end up disagreeing about what `3yw`
// means.

// ========================================================================
// Constants
// ========================================================================

/// Vim's `iskeyword`, Unicode-aware. Word motions split a line into runs of
/// these, runs of other visible characters, and the spaces between.
const WORD_CHAR = /[\p{L}\p{N}_]/u;

/// How close two glyph runs' baselines have to be, relative to their height,
/// to count as the same line. A superscript sits far enough off the baseline
/// to need the floor as well as the ratio.
const LINE_Y_RATIO = 0.45;
const LINE_Y_FLOOR = 3;

/// How wide a gap between two runs has to be, relative to the run's height,
/// before it reads as a space. Kerning and ligature splits leave gaps well
/// under this; a real space leaves one well over.
const ITEM_GAP_RATIO = 0.15;

/// How much bigger than the page's usual line gap a gap has to be to read as
/// a paragraph break. A PDF has no structure to ask, so `{` and `}` work off
/// the median gap of the page they are on.
const PARAGRAPH_GAP_MULTIPLIER = 1.6;

/// The motions an operator takes the landing character of, and the finds it
/// does. Vim's own division into inclusive and exclusive: `ye` takes the last
/// letter of the word, while `yw` stops before the next word starts, even
/// though both land in the same place.
///
/// This lives beside the motions rather than in the host's grammar because it
/// is only ever needed where the range is worked out, and because `;` and `,`
/// have to read it off whichever find they are repeating.
const INCLUSIVE_MOTIONS = new Set(['docEnd', 'lineEnd', 'wordEnd']);
const INCLUSIVE_FINDS = new Set(['f', 't']);

/// Ends a sentence, for `(` and `)`, when a space or the line's end follows.
const SENTENCE_END = /[.!?]/;

/// Smallest the cursor is drawn, in CSS pixels, so it stays visible over a
/// footnote's type.
const MIN_CARET_HEIGHT = 8;

/// How tall an empty band across a block has to be, as a multiple of the
/// page's usual glyph height, to cut it into a block above and one below. Set
/// well clear of the gap a paragraph leaves: cutting there would interleave
/// two columns a paragraph at a time.
const ROW_GAP_RATIO = 1.5;

/// How wide an empty band down a block has to be, by the same measure, to cut
/// it into a block left and a block right. A two-column paper leaves a gutter
/// of about two glyph heights; a bullet or an indent leaves about one, so the
/// line between them is drawn low enough that a tight gutter still reads as
/// one.
const COLUMN_GAP_RATIO = 1.4;

/// How much of a block's text each side of a column cut has to carry for that
/// cut to be a gutter. A list's bullets line up in a column of their own by
/// every other measure, and this is what tells them from a column of prose.
const COLUMN_SHARE = 0.2;

/// How many times a page is cut into smaller blocks before its layout is
/// taken as it stands. Four is a banner over a two-column body over a
/// footnote, which is as deep as a page of prose goes.
const MAX_CUT_DEPTH = 4;

/// The classes the boxes drawn over the page carry, which `viewer.html`
/// styles: what a selection covers, and what a search landed on.
const MATCH_CLASS = 'winter-caret-match';
const SELECTION_CLASS = 'winter-caret-selection';

// ========================================================================
// State
// ========================================================================

/// What the host supplies: the document, where its pages are on screen, and
/// the channel back. Set by `attach`.
let host = null;

/// Per-page text models, keyed by page number. Every box in one is in the
/// page's own PDF units, so a zoom costs a redraw rather than a rebuild.
const models = new Map();

/// Everything about where the cursor is and what it has selected.
const caret = {
  /// The other end of a visual selection, or `null` when there is none.
  anchor: null,
  /// The column a run of `j`/`k` is trying to hold, so a short line passed
  /// through does not narrow the cursor permanently.
  column: null,
  /// The last `f`/`F`/`t`/`T`, for `;` and `,` to repeat.
  find: null,
  /// What a search last landed on, as `{ page, line, char, length }`, drawn
  /// under the cursor until the cursor moves off it.
  match: null,
  /// `{ page, line, char }`, or `null` while cursor mode is off.
  pos: null,
  /// `null`, `'char'`, or `'line'`.
  visual: null,
};

/// The text `/` or `?` last looked for and which way it went, for `n` and `N`
/// to do again. Kept across leaving and re-entering cursor mode, the way vim
/// keeps a search across windows.
let lastSearch = null;

/// The element drawn as the cursor, and the ones drawn under what is
/// selected or what a search found.
let caretEl = null;
let overlayEls = [];

// ========================================================================
// Setup
// ========================================================================

/// Give the caret what it needs from the viewer: the pdf.js document, the
/// frame element each page is drawn in, the current scale, and the channel to
/// post a yank back on.
export function attach(callbacks) {
  host = callbacks;
}

/// Draw the cursor again after the pages moved or changed size under it.
export function invalidate() {
  render();
}

// ========================================================================
// The page text model
// ========================================================================

/// The text of page `number` as lines of characters, built once and kept.
///
/// Returns `null` past either end of the document, which is what stops a
/// motion walking off it.
async function pageModel(number) {
  if (number < 1 || number > host.pageCount()) return null;
  const cached = models.get(number);
  if (cached) return cached;

  const page = await host.getPage(number);
  const content = await page.getTextContent();
  const items = content.items
    .filter((item) => item.str && item.str.length > 0)
    .map((item) => ({
      height: Math.abs(item.transform[3]) || item.height || 1,
      str: item.str,
      width: item.width,
      x: item.transform[4],
      y: item.transform[5],
    }));

  const model = {
    breaks: new Set(),
    lines: buildLines(items),
    number,
    // The page's own transform, taken once at scale 1: every box is drawn
    // through it, so a rotated page's text is drawn where pdf.js drew it
    // rather than where an upright page would have put it.
    viewport: page.getViewport({ scale: 1 }),
  };
  model.breaks = paragraphBreaks(model.lines);
  models.set(number, model);
  return model;
}

/// The page's lines in reading order: block by block down the page, and line
/// by line down each block.
function buildLines(items) {
  const lines = [];
  for (const block of blocks(items, 0)) lines.push(...blockLines(block));
  return lines;
}

/// Cut `items` into the blocks a reader would take them in, along the empty
/// bands that run right across them.
///
/// A band across the page comes first, which is what puts a banner ahead of
/// the body and a footnote after it; only where no band crosses is a band
/// down the page looked for, which is the gutter of a two-column paper. The
/// two alternate until nothing else cuts or the page has been cut as deep as
/// a page of prose goes.
function blocks(items, depth) {
  if (items.length < 2 || depth >= MAX_CUT_DEPTH) return [items];
  const size = medianHeight(items);

  const rows = cut(items, (item) => [item.y, item.y + item.height], size * ROW_GAP_RATIO);
  if (rows) {
    // Down the page: PDF units count up from its foot, so the last band is
    // the topmost one.
    return rows.reverse().flatMap((row) => blocks(row, depth + 1));
  }

  const columns = cut(items, (item) => [item.x, item.x + item.width], size * COLUMN_GAP_RATIO);
  if (columns && columns.every((column) => charCount(column) >= charCount(items) * COLUMN_SHARE)) {
    return columns.flatMap((column) => blocks(column, depth + 1));
  }
  return [items];
}

/// Split `items` along every empty band of at least `least`, measured over
/// the interval `span` takes off each item. `null` when no band is that wide,
/// which is what says this block is not cut that way.
///
/// The parts come back in ascending order of the measured axis.
function cut(items, span, least) {
  const spans = items.map(span).sort((a, b) => a[0] - b[0]);
  const edges = [];
  let reach = spans[0][1];
  for (const [start, end] of spans) {
    if (start - reach >= least) edges.push((reach + start) / 2);
    reach = Math.max(reach, end);
  }
  if (edges.length === 0) return null;

  const parts = edges.map(() => []).concat([[]]);
  for (const item of items) {
    const [start] = span(item);
    const index = edges.findIndex((edge) => start < edge);
    parts[index < 0 ? edges.length : index].push(item);
  }
  return parts.filter((part) => part.length > 0);
}

/// Cluster one block's glyph runs into lines by baseline, in reading order.
function blockLines(items) {
  const sorted = [...items].sort((a, b) => (Math.abs(b.y - a.y) > LINE_Y_FLOOR ? b.y - a.y : a.x - b.x));

  const lines = [];
  for (const item of sorted) {
    const tolerance = Math.max(LINE_Y_FLOOR, item.height * LINE_Y_RATIO);
    const line = lines.find((candidate) => Math.abs(candidate.y - item.y) <= tolerance);
    if (line) {
      line.items.push(item);
      line.y = (line.y + item.y) / 2;
    } else {
      lines.push({ items: [item], y: item.y });
    }
  }

  for (const line of lines) {
    line.items.sort((a, b) => a.x - b.x);
    Object.assign(line, layOutChars(line.items));
  }
  return lines;
}

/// The height of the block's middle glyph run, which is what every band is
/// measured against: a page sets its own idea of how wide a gap is wide.
function medianHeight(items) {
  const heights = items.map((item) => item.height).sort((a, b) => a - b);
  return heights[Math.floor(heights.length / 2)] || 1;
}

/// How many characters a block holds, for the share a column has to carry.
function charCount(items) {
  return items.reduce((total, item) => total + item.str.length, 0);
}

/// Turn a line's runs into one string and one box per character, inserting a
/// space wherever the runs were far enough apart to have had one.
function layOutChars(items) {
  const chars = [];
  let text = '';
  items.forEach((item, index) => {
    const charWidth = item.width / Math.max(item.str.length, 1);
    if (index > 0) {
      const previous = items[index - 1];
      const gap = item.x - (previous.x + previous.width);
      if (gap > previous.height * ITEM_GAP_RATIO) {
        text += ' ';
        chars.push({ h: previous.height, w: gap, x: previous.x + previous.width, y: previous.y });
      }
    }
    for (let i = 0; i < item.str.length; i += 1) {
      text += item.str[i];
      chars.push({ h: item.height, w: charWidth, x: item.x + charWidth * i, y: item.y });
    }
  });
  return { chars, height: items[0] ? items[0].height : MIN_CARET_HEIGHT, text };
}

/// The lines that start a paragraph, found from how much wider their gap is
/// than the page's usual one. A PDF carries no structure to ask instead.
function paragraphBreaks(lines) {
  const breaks = new Set();
  if (lines.length < 2) return breaks;
  const gaps = [];
  for (let i = 1; i < lines.length; i += 1) {
    gaps.push(lines[i - 1].y - lines[i].y);
  }
  const positive = gaps.filter((gap) => gap > 0).sort((a, b) => a - b);
  if (positive.length === 0) return breaks;
  const median = positive[Math.floor(positive.length / 2)] || 1;
  for (let i = 1; i < lines.length; i += 1) {
    // A gap that runs backwards is the step from the foot of one block to the
    // head of the next, which starts a paragraph however wide it is.
    if (gaps[i - 1] <= 0 || gaps[i - 1] > median * PARAGRAPH_GAP_MULTIPLIER) breaks.add(i);
  }
  return breaks;
}

// ========================================================================
// Position helpers
// ========================================================================

function classOf(ch) {
  if (ch === undefined || /\s/.test(ch)) return 'space';
  return WORD_CHAR.test(ch) ? 'word' : 'punct';
}

async function lineAt(pos) {
  const model = await pageModel(pos.page);
  return model ? model.lines[pos.line] : undefined;
}

/// Order two positions the way the document reads.
function compare(a, b) {
  if (a.page !== b.page) return a.page - b.page;
  if (a.line !== b.line) return a.line - b.line;
  return a.char - b.char;
}

/// The line after `pos`, crossing into the next page when there is none left
/// on this one. `null` at the end of the document.
///
/// A page carrying no text of its own is stepped over rather than stopped at:
/// a scan or a plate in the middle of a document would otherwise be the end
/// of every motion and of every search that reached it.
async function nextLine(pos) {
  const model = await pageModel(pos.page);
  if (!model) return null;
  if (pos.line + 1 < model.lines.length) return { char: 0, line: pos.line + 1, page: pos.page };
  for (let number = pos.page + 1; number <= host.pageCount(); number += 1) {
    const following = await pageModel(number);
    if (following && following.lines.length > 0) return { char: 0, line: 0, page: number };
  }
  return null;
}

/// The line before `pos`, crossing back a page when there is none, and over
/// the pages with no text on them for the same reason.
async function previousLine(pos) {
  if (pos.line > 0) return { char: 0, line: pos.line - 1, page: pos.page };
  for (let number = pos.page - 1; number >= 1; number -= 1) {
    const preceding = await pageModel(number);
    if (preceding && preceding.lines.length > 0) {
      return { char: 0, line: preceding.lines.length - 1, page: number };
    }
  }
  return null;
}

/// The first position in the document that has any text on it.
async function documentStart() {
  for (let number = 1; number <= host.pageCount(); number += 1) {
    const model = await pageModel(number);
    if (model && model.lines.length > 0) return { char: 0, line: 0, page: number };
  }
  return null;
}

async function documentEnd() {
  for (let number = host.pageCount(); number >= 1; number -= 1) {
    const model = await pageModel(number);
    if (model && model.lines.length > 0) {
      const line = model.lines.length - 1;
      return { char: Math.max(model.lines[line].text.length - 1, 0), line, page: number };
    }
  }
  return null;
}

/// The first position on page `number`, for `{count}G` in cursor mode.
async function pageStart(number) {
  const model = await pageModel(Math.min(Math.max(number, 1), host.pageCount()));
  if (!model || model.lines.length === 0) return null;
  return { char: 0, line: 0, page: model.number };
}

// ========================================================================
// Motions
// ========================================================================

/// One step of `motion` from `pos`, or `null` when the document ends first.
async function step(pos, motion) {
  const line = await lineAt(pos);
  if (!line) return null;
  const last = Math.max(line.text.length - 1, 0);

  switch (motion) {
    case 'left':
      return pos.char > 0 ? { ...pos, char: pos.char - 1 } : null;
    case 'right':
      return pos.char < last ? { ...pos, char: pos.char + 1 } : null;
    case 'down':
    case 'up': {
      const target = motion === 'down' ? await nextLine(pos) : await previousLine(pos);
      if (!target) return null;
      const into = await lineAt(target);
      const want = caret.column === null ? pos.char : caret.column;
      const width = into ? Math.max(into.text.length - 1, 0) : 0;
      return { ...target, char: Math.min(want, width) };
    }
    case 'lineStart':
      return { ...pos, char: 0 };
    case 'lineEnd':
      return { ...pos, char: last };
    case 'lineFirstNonBlank': {
      const index = line.text.search(/\S/);
      return { ...pos, char: index < 0 ? 0 : index };
    }
    case 'wordForward':
      return wordForward(pos, line);
    case 'wordBackward':
      return wordBackward(pos, line);
    case 'wordEnd':
      return wordEnd(pos, line);
    case 'paragraphForward':
    case 'paragraphBackward':
      return paragraph(pos, motion === 'paragraphForward' ? 1 : -1);
    case 'sentenceForward':
    case 'sentenceBackward':
      return sentence(pos, line, motion === 'sentenceForward' ? 1 : -1);
    default:
      return null;
  }
}

/// Vim's `w`: the start of the next run of like characters, crossing lines.
async function wordForward(pos, line) {
  let at = pos;
  let text = line.text;
  const started = classOf(text[at.char]);
  // Off the end of what the cursor is in the middle of.
  if (started !== 'space') {
    while (at.char < text.length && classOf(text[at.char]) === started) at = { ...at, char: at.char + 1 };
  }
  // Then over the space that follows it, which may run to the next line.
  for (;;) {
    if (at.char >= text.length) {
      const next = await nextLine(at);
      if (!next) return null;
      const into = await lineAt(next);
      if (!into) return null;
      at = next;
      text = into.text;
      if (text.length === 0) continue;
      if (classOf(text[0]) !== 'space') return at;
      continue;
    }
    if (classOf(text[at.char]) !== 'space') return at;
    at = { ...at, char: at.char + 1 };
  }
}

/// Vim's `b`: back to the start of this run, or of the one before it.
async function wordBackward(pos, line) {
  let at = pos;
  let text = line.text;
  for (;;) {
    if (at.char === 0) {
      const previous = await previousLine(at);
      if (!previous) return null;
      const into = await lineAt(previous);
      if (!into) return null;
      at = { ...previous, char: into.text.length };
      text = into.text;
      if (text.length === 0) continue;
      continue;
    }
    at = { ...at, char: at.char - 1 };
    if (classOf(text[at.char]) === 'space') continue;
    const kind = classOf(text[at.char]);
    while (at.char > 0 && classOf(text[at.char - 1]) === kind) at = { ...at, char: at.char - 1 };
    return at;
  }
}

/// Vim's `e`: forward to the last character of this run, or of the next one.
async function wordEnd(pos, line) {
  let at = pos;
  let text = line.text;
  for (;;) {
    at = { ...at, char: at.char + 1 };
    if (at.char >= text.length) {
      const next = await nextLine(at);
      if (!next) return null;
      const into = await lineAt(next);
      if (!into) return null;
      at = { ...next, char: -1 };
      text = into.text;
      continue;
    }
    if (classOf(text[at.char]) === 'space') continue;
    const kind = classOf(text[at.char]);
    while (at.char + 1 < text.length && classOf(text[at.char + 1]) === kind) {
      at = { ...at, char: at.char + 1 };
    }
    return at;
  }
}

/// `{` and `}`: the next line that starts a paragraph, by the page's own
/// spacing.
async function paragraph(pos, direction) {
  let at = pos;
  for (;;) {
    const target = direction > 0 ? await nextLine(at) : await previousLine(at);
    if (!target) return null;
    at = target;
    const model = await pageModel(at.page);
    if (!model) return null;
    if (at.line === 0 || model.breaks.has(at.line)) return at;
  }
}

/// `(` and `)`: the character after the next sentence end, within the line.
async function sentence(pos, line, direction) {
  const text = line.text;
  const ends = [];
  for (let i = 0; i < text.length; i += 1) {
    const following = text[i + 1];
    if (SENTENCE_END.test(text[i]) && (following === undefined || /\s/.test(following))) {
      const start = text.slice(i + 1).search(/\S/);
      if (start >= 0) ends.push(i + 1 + start);
    }
  }
  const forward = ends.find((index) => index > pos.char);
  const backward = [...ends].reverse().find((index) => index < pos.char);
  const target = direction > 0 ? forward : backward;
  if (target !== undefined) return { ...pos, char: target };
  // No sentence break left on this line, so fall back to the line itself.
  return step(pos, direction > 0 ? 'down' : 'up');
}

/// `H`, `M`, and `L`: the line nearest an edge of what is on screen. Never
/// scrolls, which is what makes them different from `zt`/`zz`/`zb`.
async function viewportLine(where) {
  const top = host.scrollTop();
  const height = host.viewportHeight();
  const want = where === 'viewTop' ? top : where === 'viewBottom' ? top + height : top + height / 2;

  let best = null;
  let bestDistance = Infinity;
  for (const number of host.visiblePages()) {
    const model = await pageModel(number);
    if (!model) continue;
    for (let index = 0; index < model.lines.length; index += 1) {
      const rect = lineRect(number, model.lines[index]);
      if (!rect) continue;
      const distance = Math.abs(rect.top + rect.height / 2 - want);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = { char: 0, line: index, page: number };
      }
    }
  }
  return best;
}

/// `f`, `F`, `t`, and `T`: to a character on the cursor's own line, which is
/// where vim keeps them too.
async function findOnLine(pos, kind, ch, count) {
  const line = await lineAt(pos);
  if (!line) return null;
  const text = line.text;
  const forward = kind === 'f' || kind === 't';
  let at = pos.char;
  for (let remaining = count; remaining > 0; remaining -= 1) {
    let found = -1;
    if (forward) {
      for (let i = at + 1; i < text.length; i += 1) {
        if (text[i] === ch) {
          found = i;
          break;
        }
      }
    } else {
      for (let i = at - 1; i >= 0; i -= 1) {
        if (text[i] === ch) {
          found = i;
          break;
        }
      }
    }
    if (found < 0) return null;
    at = found;
  }
  if (kind === 't') return { ...pos, char: Math.max(at - 1, 0) };
  if (kind === 'T') return { ...pos, char: Math.min(at + 1, Math.max(text.length - 1, 0)) };
  return { ...pos, char: at };
}

/// Where the next match of `pattern` lies from `from`, going `forward` or
/// back and wrapping around the document once, as `{ page, line, char,
/// length }`. `null` when the text is nowhere in the document.
///
/// A pattern typed in lower case matches either case, and one typed with a
/// capital in it matches exactly. That is the rule the editor searches its
/// files by, and it is what makes looking for a word find it however it was
/// written while looking for a name finds the name.
async function nextMatch(from, forward, pattern) {
  const folded = !/\p{Lu}/u.test(pattern);
  const needle = folded ? pattern.toLowerCase() : pattern;
  if (needle.length === 0) return null;

  const start = `${from.page}:${from.line}`;
  let at = { ...from };
  // The line the search sets off from is searched only past the cursor to
  // begin with, and whole again when the wrap comes back around to it, so a
  // match behind the cursor on its own line is found last rather than never.
  let bounded = true;
  for (;;) {
    const line = await lineAt(at);
    if (line) {
      const text = folded ? line.text.toLowerCase() : line.text;
      const found = bounded
        ? boundedIndex(text, needle, from.char, forward)
        : wholeIndex(text, needle, forward);
      if (found >= 0) {
        return { char: found, length: needle.length, line: at.line, page: at.page };
      }
    }
    if (!bounded && `${at.page}:${at.line}` === start) return null;
    const next = forward ? await nextLine(at) : await previousLine(at);
    at = next || (forward ? await documentStart() : await documentEnd());
    if (!at) return null;
    bounded = false;
  }
}

/// The first match on a line, or the last one when the search runs backwards.
function wholeIndex(text, needle, forward) {
  return forward ? text.indexOf(needle) : text.lastIndexOf(needle);
}

/// The same, on the line the search set off from, where only what lies past
/// the cursor counts.
function boundedIndex(text, needle, char, forward) {
  if (forward) return text.indexOf(needle, char + 1);
  return char > 0 ? text.lastIndexOf(needle, char - 1) : -1;
}

// ========================================================================
// Text between two positions
// ========================================================================

/// The document's text from `from` to `to`, as lines.
///
/// `inclusive` says whether the last character of the range is part of it,
/// which after the two ends are put in document order always means the later
/// one. Linewise takes whole lines however far into them the ends sat.
async function textBetween(from, to, linewise, inclusive) {
  const [start, end] = compare(from, to) <= 0 ? [from, to] : [to, from];
  const parts = [];
  let at = { ...start, char: linewise ? 0 : start.char };

  for (;;) {
    const line = await lineAt(at);
    if (!line) break;
    const isLast = at.page === end.page && at.line === end.line;
    const stop = inclusive ? end.char + 1 : end.char;
    const until = linewise || !isLast ? line.text.length : Math.min(Math.max(stop, 0), line.text.length);
    parts.push(line.text.slice(at.char, until));
    if (isLast) break;
    const next = await nextLine(at);
    if (!next) break;
    at = next;
  }
  return parts.join('\n');
}

// ========================================================================
// Drawing
// ========================================================================

/// One character's box in the page's own pixels, or `null` when the page is
/// not on screen to draw over.
function charRect(page, line, index) {
  const box = line.chars[Math.min(index, line.chars.length - 1)];
  return box ? boxRect(page, box) : null;
}

/// The whole line's box, for the viewport motions and for scrolling.
function lineRect(page, line) {
  if (line.chars.length === 0) return null;
  const first = boxRect(page, line.chars[0]);
  const last = boxRect(page, line.chars[line.chars.length - 1]);
  return first && last ? union(first, last) : null;
}

/// One box of a page's text, in the pixels of the frame that page is drawn
/// in.
///
/// Mapped through pdf.js's own viewport rather than flipped by hand, so the
/// cursor sits on the glyphs of a page the document asked to be turned as
/// squarely as it does on an upright one.
function boxRect(page, box) {
  const frame = host.frameFor(page);
  const model = models.get(page);
  if (!frame || !model) return null;
  const scale = host.scale();
  const [x1, y1] = model.viewport.convertToViewportPoint(box.x, box.y);
  const [x2, y2] = model.viewport.convertToViewportPoint(box.x + box.w, box.y + box.h);
  return {
    height: Math.max(Math.abs(y2 - y1) * scale, MIN_CARET_HEIGHT),
    left: frame.offsetLeft + Math.min(x1, x2) * scale,
    top: frame.offsetTop + Math.min(y1, y2) * scale,
    width: Math.max(Math.abs(x2 - x1) * scale, 1),
  };
}

/// The smallest box holding both, which is a line's own when they are the
/// boxes of its first and last characters.
function union(first, last) {
  const left = Math.min(first.left, last.left);
  const top = Math.min(first.top, last.top);
  return {
    height: Math.max(first.top + first.height, last.top + last.height) - top,
    left,
    top,
    width: Math.max(first.left + first.width, last.left + last.width) - left,
  };
}

/// Draw the cursor, and under it the selection or the match a search found.
async function render() {
  clearOverlays();
  if (!caret.pos) {
    if (caretEl) caretEl.style.display = 'none';
    return;
  }
  const line = await lineAt(caret.pos);
  if (!line) return;
  await renderMatch();

  if (!caretEl) {
    caretEl = document.createElement('div');
    caretEl.id = 'winter-caret';
    document.body.append(caretEl);
  }
  const rect = charRect(caret.pos.page, line, caret.pos.char);
  if (!rect) {
    caretEl.style.display = 'none';
    return;
  }
  Object.assign(caretEl.style, {
    display: 'block',
    height: `${rect.height}px`,
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${rect.width}px`,
  });

  if (caret.visual && caret.anchor) await renderSelection();
}

/// A box per line the selection covers, drawn under the text.
async function renderSelection() {
  const [start, end] = compare(caret.anchor, caret.pos) <= 0
    ? [caret.anchor, caret.pos]
    : [caret.pos, caret.anchor];

  let at = { ...start };
  for (;;) {
    const line = await lineAt(at);
    if (!line) break;
    const isLast = at.page === end.page && at.line === end.line;
    const from = caret.visual === 'line' || at.page !== start.page || at.line !== start.line ? 0 : start.char;
    const to = caret.visual === 'line' || !isLast ? Math.max(line.chars.length - 1, 0) : end.char;
    paintBox(at.page, line, from, to, SELECTION_CLASS);
    if (isLast) break;
    const next = await nextLine(at);
    if (!next) break;
    at = next;
  }
}

/// The run of characters a search landed on, drawn whole: the cursor sits on
/// its first letter, which on its own says little about what was found.
async function renderMatch() {
  if (!caret.match) return;
  const line = await lineAt(caret.match);
  if (!line) return;
  const last = caret.match.char + caret.match.length - 1;
  paintBox(caret.match.page, line, caret.match.char, last, MATCH_CLASS);
}

/// Draw one box over the page, from one character of a line to another.
function paintBox(page, line, from, to, className) {
  const first = charRect(page, line, from);
  const last = charRect(page, line, to);
  if (!first || !last) return;
  const rect = union(first, last);
  const element = document.createElement('div');
  element.className = className;
  Object.assign(element.style, {
    height: `${rect.height}px`,
    left: `${rect.left}px`,
    top: `${rect.top}px`,
    width: `${rect.width}px`,
  });
  document.body.append(element);
  overlayEls.push(element);
}

function clearOverlays() {
  for (const element of overlayEls) element.remove();
  overlayEls = [];
}

/// Bring the cursor into view, moving as little as vim does: nothing at all
/// while it is already on screen.
async function scrollCaretIntoView() {
  if (!caret.pos) return;
  const line = await lineAt(caret.pos);
  if (!line) return;
  const rect = lineRect(caret.pos.page, line);
  if (!rect) return;
  const top = host.scrollTop();
  const height = host.viewportHeight();
  if (rect.top < top) host.scrollTo(rect.top);
  else if (rect.top + rect.height > top + height) host.scrollTo(rect.top + rect.height - height);
}

// ========================================================================
// The commands the host's page drives
// ========================================================================

/// Put the cursor in the text, at the first line on screen.
export async function enter() {
  if (caret.pos) return;
  const at = (await viewportLine('viewTop')) || (await documentStart());
  if (!at) {
    host.post({ error: 'this document has no text to put a cursor in' });
    return;
  }
  caret.pos = at;
  caret.column = null;
  await finish(false);
}

/// Take the cursor out of the text, dropping any selection with it.
export async function leave() {
  caret.pos = null;
  caret.anchor = null;
  caret.match = null;
  caret.visual = null;
  caret.column = null;
  await finish(false);
}

/// Run `motion` `count` times, moving the cursor or, under an operator,
/// yanking what it passes over.
export async function motion(operator, name, count) {
  if (!caret.pos) return;
  caret.match = null;
  const from = { ...caret.pos };
  let at = { ...caret.pos };

  for (let remaining = count; remaining > 0; remaining -= 1) {
    const next = await resolve(at, name, count);
    if (!next) break;
    at = next;
    // An absolute motion means the same thing however many times it is
    // asked for, so a count on one is the count it carries, not a repeat.
    if (name === 'docStart' || name === 'docEnd' || name === 'page') break;
  }

  // The column a run of j/k holds is the one last asked for, not the one a
  // short line in the middle of the run happened to allow.
  caret.column = name === 'down' || name === 'up' ? (caret.column === null ? from.char : caret.column) : null;

  if (operator === 'yank') {
    await yankRange(from, at, false, INCLUSIVE_MOTIONS.has(name));
    return;
  }
  caret.pos = at;
  await finish(true);
}

/// The motions that do not step, resolved in one go.
async function resolve(at, name, count) {
  switch (name) {
    case 'docStart':
      return documentStart();
    case 'docEnd':
      return documentEnd();
    case 'page':
      return pageStart(count);
    case 'viewTop':
    case 'viewMiddle':
    case 'viewBottom':
      return viewportLine(name);
    default:
      return step(at, name);
  }
}

/// `f`, `F`, `t`, `T`, under an operator or not.
export async function find(operator, kind, ch, count) {
  if (!caret.pos) return;
  caret.match = null;
  caret.find = { ch, kind };
  const from = { ...caret.pos };
  const at = await findOnLine(from, kind, ch, count);
  if (!at) return;
  if (operator === 'yank') {
    await yankRange(from, at, false, INCLUSIVE_FINDS.has(kind));
    return;
  }
  caret.pos = at;
  caret.column = null;
  await finish(true);
}

/// `/` and `?`: look for `pattern`, which `n` and `N` then do again.
export async function search(operator, pattern, forward, count) {
  if (!pattern) return;
  lastSearch = { forward, pattern };
  await runSearch(operator, forward, count);
}

/// `n` and `N`: the last search again, the same way or the opposite one.
export async function searchStep(operator, reverse, count) {
  if (!lastSearch) return;
  await runSearch(operator, lastSearch.forward !== reverse, count);
}

/// Walk `count` matches from where the reader is and put the cursor on the
/// last of them, with what was found drawn under it.
///
/// A search made without a cursor in the text leaves one there, the way `i`
/// would: a document scrolled to a match with nothing marking which words
/// matched would be half an answer.
async function runSearch(operator, forward, count) {
  const from = caret.pos
    ? { ...caret.pos }
    : (await viewportLine('viewTop')) || (await documentStart());
  if (!from) {
    host.post({ error: 'this document has no text to search' });
    return;
  }

  let at = from;
  for (let remaining = count; remaining > 0; remaining -= 1) {
    const found = await nextMatch(at, forward, lastSearch.pattern);
    if (!found) {
      host.post({ error: `\`${lastSearch.pattern}\` is not in this document` });
      return;
    }
    at = found;
  }

  if (operator === 'yank') {
    // Vim's own `/` under an operator: up to what was found, not over it.
    await yankRange(from, at, false, false);
    return;
  }
  caret.match = { char: at.char, length: at.length, line: at.line, page: at.page };
  caret.pos = { char: at.char, line: at.line, page: at.page };
  caret.column = null;
  await finish(true);
}

/// `;` and `,`: the last find again, the same way or the opposite one.
export async function repeatFind(operator, reverse, count) {
  if (!caret.find) return;
  const opposite = { F: 'f', T: 't', f: 'F', t: 'T' };
  const kind = reverse ? opposite[caret.find.kind] : caret.find.kind;
  await find(operator, kind, caret.find.ch, count);
}

/// `v` and `V`, and pressing either again to drop the selection.
export async function visual(kind) {
  if (!caret.pos) return;
  if (caret.visual === kind) {
    caret.visual = null;
    caret.anchor = null;
  } else {
    caret.visual = kind;
    caret.anchor = caret.anchor || { ...caret.pos };
  }
  await finish(false);
}

/// `o`: put the cursor on the other end of the selection.
export async function swapEnds() {
  if (!caret.visual || !caret.anchor) return;
  const other = caret.anchor;
  caret.anchor = { ...caret.pos };
  caret.pos = other;
  await finish(true);
}

/// `y` in visual mode: copy what is selected and drop the selection.
export async function yankSelection() {
  if (!caret.visual || !caret.anchor || !caret.pos) return;
  // A selection always covers both of its own ends, whatever the motion that
  // grew it would have done under an operator.
  await yankRange(caret.anchor, caret.pos, caret.visual === 'line', true);
}

/// `yy` and `Y`: whole lines from the cursor's own.
export async function yankLines(count) {
  if (!caret.pos) return;
  let end = { ...caret.pos };
  for (let remaining = count - 1; remaining > 0; remaining -= 1) {
    const next = await nextLine(end);
    if (!next) break;
    end = next;
  }
  await yankRange(caret.pos, end, true, true);
}

/// `zt`, `zz`, `zb` in cursor mode: the window moves so the cursor's own line
/// lands at an edge, and the cursor itself stays put.
export async function placeLine(where) {
  if (!caret.pos) return;
  const line = await lineAt(caret.pos);
  if (!line) return;
  const rect = lineRect(caret.pos.page, line);
  if (!rect) return;
  const height = host.viewportHeight();
  const offsets = { bottom: height - rect.height, center: (height - rect.height) / 2, top: 0 };
  host.scrollTo(rect.top - (offsets[where] ?? 0));
  await finish(false);
}

/// Copy the text between two positions, tell the host, and show what was
/// taken by leaving the selection up for the frame it is reported on.
async function yankRange(from, to, linewise, inclusive) {
  const text = await textBetween(from, to, linewise, inclusive);
  caret.visual = null;
  caret.anchor = null;
  await finish(false);
  host.post({ yank: text });
}

/// Redraw, report, and scroll the cursor back into view when it moved.
async function finish(follow) {
  if (follow) await scrollCaretIntoView();
  await render();
  host.post({
    caret: caret.pos ? { line: caret.pos.line + 1, page: caret.pos.page } : null,
    mode: caret.pos ? caret.visual || 'cursor' : 'normal',
  });
}

/// Keep the cursor drawn where its text is after the page moves under it.
export async function reposition() {
  if (caret.pos) await render();
}
