//! Looking words up in a GoldenDict dictionary, as magic-vscode looks them
//! up: an ABBYY Lingvo DSL file compressed with dictzip (`.dsl.dz`), read
//! where GoldenDict keeps its content. The file's headwords are read once,
//! on a thread of their own, into where each entry starts and ends; an
//! entry is then read by inflating only the chunks it spans, and its markup
//! is set out as plain lines.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;

use flate2::read::GzDecoder;
use flate2::{Decompress, FlushDecompress};

// ========================================================================
// Constants
// ========================================================================

/// Where Winter looks for dictionaries of its own, in its configuration
/// directory: a folder of them, or a link to wherever they are kept.
const OWN_DICTIONARIES: &str = "dictionaries";

/// GoldenDict's content directory in a home directory.
const GOLDENDICT_CONTENT: &str = ".local/share/goldendict/content";

/// The system-wide content directory.
const SYSTEM_CONTENT_DIR: &str = "/usr/share/goldendict/content";

/// What a dictionary file's name ends in.
const DSL_DZ: &str = ".dsl.dz";

/// The gzip header flags read past to the data.
const FEXTRA: u8 = 0x04;
const FNAME: u8 = 0x08;
const FCOMMENT: u8 = 0x10;
const FHCRC: u8 = 0x02;

/// How far a nested part of an entry (`[m2]`) is indented per level.
const MARGIN: &str = "  ";

/// The endings a word is tried without, where it is not a headword itself.
const ENDINGS: &[&str] = &["ies", "es", "s", "ied", "ed", "ing", "ly", "er", "est"];

// ========================================================================
// Data Structures
// ========================================================================

/// A dictionary being read, or read.
#[derive(Clone)]
enum State {
    Failed(String),
    Loading,
    Ready(Arc<Dictionary>),
}

/// A dictionary's headwords and where each entry is.
pub(crate) struct Dictionary {
    /// Each lowercased headword's entry, as the uncompressed bytes it spans.
    entries: HashMap<String, (usize, usize)>,
    /// Whether the text is UTF-16 (little-endian) rather than UTF-8.
    utf16: bool,
    /// The dictzip chunks.
    zip: Chunks,
    /// The headword as the dictionary writes it, by its lowercased form.
    headwords: HashMap<String, String>,
    /// What the dictionary calls itself.
    name: String,
}

/// A dictzip file's chunk table: where the compressed data starts, how
/// long each chunk is uncompressed, and each chunk's compressed size.
struct Chunks {
    chunk_len: usize,
    data_start: u64,
    path: PathBuf,
    sizes: Vec<u16>,
}

/// One entry, set out: its headword and its lines.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    /// The dictionary it is from.
    pub(crate) dictionary: String,
    pub(crate) headword: String,
    pub(crate) lines: Vec<String>,
}

/// What looking a word up came to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Lookup {
    /// The word's entry.
    Found(Entry),
    /// The dictionary is still being read; ask again in a moment.
    Loading,
    /// No dictionary could be read, and why.
    Missing(String),
    /// The dictionary has no entry for the word.
    NotFound,
}

// ========================================================================
// Functions
// ========================================================================

/// Look `word` up in the dictionary, starting to read it on first use.
pub(crate) fn look_up(word: &str) -> Lookup {
    static STATE: OnceLock<Mutex<Option<State>>> = OnceLock::new();
    let state = STATE.get_or_init(|| Mutex::new(None));
    let Ok(mut slot) = state.lock() else {
        return Lookup::Missing("the dictionary could not be read".to_string());
    };
    let current = match slot.clone() {
        Some(current) => current,
        None => {
            let Some(path) = find_dictionary() else {
                let missing = "no GoldenDict .dsl.dz dictionary found (looked in ~/Setup/GoldenDict, ~/Workspace/data/dictionaries, ~/.local/share/goldendict/content)";
                *slot = Some(State::Failed(missing.to_string()));
                return Lookup::Missing(missing.to_string());
            };
            *slot = Some(State::Loading);
            let _ = thread::Builder::new().name("winter dictionary".into()).spawn(move || {
                let read = Dictionary::open(&path).map_or_else(State::Failed, |dictionary| State::Ready(Arc::new(dictionary)));
                if let Some(state) = STATE.get() {
                    if let Ok(mut slot) = state.lock() {
                        *slot = Some(read);
                    }
                }
            });
            State::Loading
        }
    };
    drop(slot);
    match current {
        State::Loading => Lookup::Loading,
        State::Failed(why) => Lookup::Missing(why),
        State::Ready(dictionary) => match dictionary.entry(word) {
            Some(entry) => Lookup::Found(entry),
            None => Lookup::NotFound,
        },
    }
}

/// The largest dictionary in Winter's own dictionary folder or GoldenDict's
/// content directories. Each of those may itself be a link; inside them, a
/// link to a directory is not followed, so a loop of links cannot hold the
/// search for good.
fn find_dictionary() -> Option<PathBuf> {
    let mut dirs = vec![crate::config::config_dir().join(OWN_DICTIONARIES)];
    dirs.extend(crate::paths::home_dir().map(|home| home.join(GOLDENDICT_CONTENT)));
    dirs.push(PathBuf::from(SYSTEM_CONTENT_DIR));
    let mut found: Vec<(u64, PathBuf)> = Vec::new();
    for dir in dirs {
        let mut stack = vec![dir];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    stack.push(path);
                } else if path.to_string_lossy().ends_with(DSL_DZ) {
                    let size = entry.metadata().map_or(0, |meta| meta.len());
                    found.push((size, path));
                }
            }
        }
    }
    found.into_iter().max_by_key(|(size, _)| *size).map(|(_, path)| path)
}

// ========================================================================
// Dictionary
// ========================================================================

impl Dictionary {
    /// Read the dictionary at `path`: its chunk table, then every headword
    /// through one pass over the whole text.
    fn open(path: &Path) -> Result<Self, String> {
        let zip = Chunks::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let file = File::open(path).map_err(|e| e.to_string())?;
        let mut text = Vec::new();
        GzDecoder::new(BufReader::new(file))
            .read_to_end(&mut text)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let utf16 = text.starts_with(&[0xff, 0xfe]);
        let mut entries = HashMap::new();
        let mut headwords = HashMap::new();
        let mut name = path.file_name().map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        // The headwords waiting for their entry, which starts at the first
        // indented line after them.
        let mut waiting: Vec<String> = Vec::new();
        let mut body_start = None;
        for (start, end) in line_spans(&text, utf16) {
            let line = decode(&text[start..end], utf16);
            let line = line.trim_end_matches(['\r', '\n']).trim_start_matches('\u{feff}');
            if line.starts_with('#') && body_start.is_none() && waiting.is_empty() {
                if let Some(value) = line.strip_prefix("#NAME") {
                    name = value.trim().trim_matches('"').to_string();
                }
                continue;
            }
            let indented = line.starts_with([' ', '\t']);
            match (indented, line.trim().is_empty()) {
                (true, false) => {
                    if body_start.is_none() {
                        body_start = Some(start);
                    }
                }
                (false, false) => {
                    if let Some(from) = body_start.take() {
                        for headword in waiting.drain(..) {
                            entries.entry(headword.to_lowercase()).or_insert((from, start));
                            headwords.entry(headword.to_lowercase()).or_insert(headword);
                        }
                    }
                    waiting.push(clean_headword(line));
                }
                _ => {}
            }
        }
        if let Some(from) = body_start {
            for headword in waiting.drain(..) {
                entries.entry(headword.to_lowercase()).or_insert((from, text.len()));
                headwords.entry(headword.to_lowercase()).or_insert(headword);
            }
        }
        Ok(Self {
            entries,
            headwords,
            name,
            utf16,
            zip,
        })
    }

    /// `word`'s entry: as it is, else lowercased, else without a common
    /// ending.
    fn entry(&self, word: &str) -> Option<Entry> {
        let lower = word.trim().to_lowercase();
        let candidates = std::iter::once(lower.clone()).chain(ENDINGS.iter().filter_map(|ending| {
            let stem = lower.strip_suffix(ending)?;
            (stem.len() >= 2).then(|| match *ending {
                "ies" | "ied" => format!("{stem}y"),
                _ => stem.to_string(),
            })
        }));
        for candidate in candidates {
            let Some(&(start, end)) = self.entries.get(&candidate) else {
                continue;
            };
            let bytes = self.zip.read(start, end).ok()?;
            let body = decode(&bytes, self.utf16);
            let headword = self.headwords.get(&candidate).cloned().unwrap_or(candidate);
            return Some(Entry {
                dictionary: self.name.clone(),
                lines: render(&body, &headword),
                headword,
            });
        }
        None
    }
}

// ========================================================================
// Chunks
// ========================================================================

impl Chunks {
    /// Read the chunk table out of the gzip header of the file at `path`.
    fn open(path: &Path) -> Result<Self, String> {
        let mut file = File::open(path).map_err(|e| e.to_string())?;
        let mut header = vec![0; 1 << 16];
        let read = file.read(&mut header).map_err(|e| e.to_string())?;
        header.truncate(read);
        if header.len() < 12 || header[0] != 0x1f || header[1] != 0x8b {
            return Err("not a dictzip file".to_string());
        }
        let flags = header[3];
        if flags & FEXTRA == 0 {
            return Err("gzip, but no dictzip chunk table".to_string());
        }
        let u16_at = |at: usize| -> Option<u16> { Some(u16::from_le_bytes([*header.get(at)?, *header.get(at + 1)?])) };
        let xlen = u16_at(10).ok_or("a short header")? as usize;
        let mut at = 12;
        let extra_end = at + xlen;
        let mut table = None;
        while at + 4 <= extra_end {
            let length = u16_at(at + 2).ok_or("a short header")? as usize;
            if header.get(at) == Some(&b'R') && header.get(at + 1) == Some(&b'A') {
                let chunk_len = u16_at(at + 6).ok_or("a short table")? as usize;
                let count = u16_at(at + 8).ok_or("a short table")? as usize;
                let sizes = (0..count)
                    .map(|chunk| u16_at(at + 10 + 2 * chunk))
                    .collect::<Option<Vec<u16>>>()
                    .ok_or("a short table")?;
                table = Some((chunk_len, sizes));
            }
            at += 4 + length;
        }
        let (chunk_len, sizes) = table.ok_or("gzip, but no dictzip chunk table")?;
        // A chunk length of zero would divide by zero at every lookup.
        if chunk_len == 0 {
            return Err("a dictzip chunk table with chunks of no length".to_string());
        }
        let mut at = extra_end;
        for flag in [FNAME, FCOMMENT] {
            if flags & flag != 0 {
                let rest = header.get(at..).ok_or("a short header")?;
                at += rest.iter().position(|byte| *byte == 0).ok_or("a short header")? + 1;
            }
        }
        if flags & FHCRC != 0 {
            at += 2;
        }
        Ok(Self {
            chunk_len,
            data_start: at as u64,
            path: path.to_path_buf(),
            sizes,
        })
    }

    /// The uncompressed bytes from `start` to `end`, inflating only the
    /// chunks they fall in.
    fn read(&self, start: usize, end: usize) -> std::io::Result<Vec<u8>> {
        let first = start / self.chunk_len;
        let last = (end.saturating_sub(1)) / self.chunk_len;
        let mut offset = self.data_start + self.sizes[..first.min(self.sizes.len())].iter().map(|size| *size as u64).sum::<u64>();
        let mut file = File::open(&self.path)?;
        let mut out = Vec::new();
        for chunk in first..=last.min(self.sizes.len().saturating_sub(1)) {
            let size = self.sizes[chunk] as usize;
            let mut compressed = vec![0; size];
            file.seek(SeekFrom::Start(offset))?;
            file.read_exact(&mut compressed)?;
            offset += size as u64;
            // Each chunk is flushed apart, so it inflates on its own.
            let mut inflated = vec![0; self.chunk_len];
            let mut inflater = Decompress::new(false);
            inflater
                .decompress(&compressed, &mut inflated, FlushDecompress::Sync)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            inflated.truncate(inflater.total_out() as usize);
            out.extend(inflated);
        }
        let from = start - first * self.chunk_len;
        let to = (end - first * self.chunk_len).min(out.len());
        Ok(out[from.min(to)..to].to_vec())
    }
}

// ========================================================================
// Text
// ========================================================================

/// Where each line of `text` starts and ends, the newline included, in
/// bytes, reading UTF-16 two bytes at a time.
fn line_spans(text: &[u8], utf16: bool) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let step = if utf16 { 2 } else { 1 };
    let mut at = 0;
    while at + step <= text.len() {
        let newline = match utf16 {
            true => text[at] == b'\n' && text[at + 1] == 0,
            false => text[at] == b'\n',
        };
        at += step;
        if newline {
            spans.push((start, at));
            start = at;
        }
    }
    if start < text.len() {
        spans.push((start, text.len()));
    }
    spans
}

/// `bytes` as text.
fn decode(bytes: &[u8], utf16: bool) -> String {
    match utf16 {
        true => {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|pair| u16::from_le_bytes([pair[0], pair[1]])).collect();
            String::from_utf16_lossy(&units)
        }
        false => String::from_utf8_lossy(bytes).into_owned(),
    }
}

/// A headword line as the word it names: escapes undone, and the parts in
/// braces (left out of the search form) taken out.
fn clean_headword(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.trim().chars();
    let mut braced = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.extend(chars.next()),
            '{' => braced += 1,
            '}' => braced = braced.saturating_sub(1),
            c if braced == 0 => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

/// An entry's DSL body as plain lines: tags taken out, a nested part
/// indented by its margin, sounds and pictures left out, a cross-reference
/// left as its word, `~` read as the headword.
fn render(body: &str, headword: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for raw in body.lines() {
        let raw = raw.trim_end_matches('\r');
        if raw.trim().is_empty() {
            continue;
        }
        let mut margin = 0usize;
        let mut out = String::new();
        let chars: Vec<char> = raw.trim_start().chars().collect();
        let mut at = 0;
        let mut media = false;
        while at < chars.len() {
            match chars[at] {
                '\\' if at + 1 < chars.len() => {
                    if !media {
                        out.push(chars[at + 1]);
                    }
                    at += 2;
                }
                '[' => {
                    let close = chars[at..].iter().position(|c| *c == ']').map(|offset| at + offset);
                    let Some(close) = close else {
                        out.push('[');
                        at += 1;
                        continue;
                    };
                    let tag: String = chars[at + 1..close].iter().collect();
                    match tag.as_str() {
                        "s" => media = true,
                        "/s" => media = false,
                        tag if tag.starts_with('m') && tag[1..].chars().all(|c| c.is_ascii_digit()) => {
                            margin = tag[1..].parse().unwrap_or(1);
                        }
                        _ => {}
                    }
                    at = close + 1;
                }
                '{' if chars.get(at + 1) == Some(&'{') => {
                    // A comment for the dictionary's editors.
                    let rest: String = chars[at..].iter().collect();
                    let skip = rest.find("}}").map_or(rest.chars().count(), |end| rest[..end + 2].chars().count());
                    at += skip;
                }
                '~' if !media => {
                    out.push_str(headword);
                    at += 1;
                }
                c => {
                    if !media {
                        out.push(c);
                    }
                    at += 1;
                }
            }
        }
        // A cross-reference keeps the word it points at.
        let out = out.replace("<<", "").replace(">>", "");
        let text = out.trim();
        if !text.is_empty() {
            lines.push(format!("{}{text}", MARGIN.repeat(margin.saturating_sub(1))));
        }
    }
    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_dictzip_table_with_chunks_of_no_length_is_refused_not_divided_by() {
        // A chunk length of zero in a malformed header panicked on the
        // first lookup, dividing by it.
        let mut header = vec![0x1f, 0x8b, 8, FEXTRA, 0, 0, 0, 0, 0, 0];
        let table: Vec<u8> = vec![b'R', b'A', 6, 0, 1, 0, 0, 0, 1, 0, 0, 0];
        header.extend((table.len() as u16).to_le_bytes());
        header.extend(table);
        let path = std::env::temp_dir().join(format!("winter-dictzip-{}.dz", std::process::id()));
        std::fs::write(&path, &header).expect("file");
        assert!(Chunks::open(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_an_entry_s_markup_comes_out_as_plain_indented_lines() {
        let body = "\t[m1][b]noun[/b] [p]n[/p][/m]\n\t[m2]1. [trn]a fruit[/trn][/m]\n\t[m3][ex][i]an ~ a day[/i][/ex] [s]apple.wav[/s]{{note}}[/m]\n";
        assert_eq!(render(body, "apple"), ["noun n", "  1. a fruit", "    an apple a day"]);
    }

    #[test]
    fn test_a_headword_s_escapes_are_undone_and_its_braced_parts_left_out() {
        assert_eq!(clean_headword("rock\\'n\\'roll"), "rock'n'roll");
        assert_eq!(clean_headword("colo{u}r"), "color");
    }

    #[test]
    fn test_lines_are_found_two_bytes_at_a_time_in_utf16() {
        let text: Vec<u8> = "a\nbc\n".encode_utf16().flat_map(|unit| unit.to_le_bytes()).collect();
        assert_eq!(line_spans(&text, true), [(0, 4), (4, 10)]);
        assert_eq!(decode(&text[4..8], true), "bc");
    }

    /// The real dictionary, where one is installed: ignored, as it reads
    /// the whole file. `cargo test -- --ignored` runs it.
    #[test]
    #[ignore]
    fn test_the_installed_dictionary_knows_a_common_word() {
        let path = find_dictionary().expect("a dictionary installed");
        let dictionary = Dictionary::open(&path).expect("reads");
        let entry = dictionary.entry("apples").expect("apple, from apples");
        assert_eq!(entry.headword.to_lowercase(), "apple");
        assert!(!entry.lines.is_empty());
    }
}
