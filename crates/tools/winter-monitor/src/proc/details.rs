//! Looking into one process: what it has open, its sockets, the libraries it
//! loaded, its memory by what each part maps, its environment, and its
//! threads. Linux tells all of it under `/proc/<pid>`; elsewhere on Unix
//! `lsof` tells the open files, sockets, and libraries.

use std::collections::HashMap;
use std::fs;

use winter_model::monitor::{FileKind, MemoryRegion, OpenFile, ProcessDetails, ThreadInfo};

use crate::net;
use crate::run::run_text;

use super::parse::parse_proc_stat;

// ========================================================================
// Constants
// ========================================================================

/// Where Linux keeps what it knows about processes.
const PROC_DIR: &str = "/proc";

/// What a part says where the system has no way to tell it.
const NOT_HERE: &str = "not available on this system";

/// What a mapping with no name is grouped as.
const ANONYMOUS: &str = "[anon]";

/// How a shared library's file name reads: `libc.so.6`, `libm.so`.
const LIBRARY_MARK: &str = ".so";

/// The descriptor-link prefixes of what is not a file.
const PIPE_LINK: &str = "pipe:";
const SOCKET_LINK: &str = "socket:";

// ========================================================================
// Functions
// ========================================================================

/// What process `pid` has and holds, each part read on its own.
pub fn details(pid: u32) -> ProcessDetails {
    if cfg!(target_os = "linux") {
        return linux_details(pid);
    }
    if cfg!(windows) {
        return ProcessDetails {
            sockets: net::sockets_of(pid),
            ..ProcessDetails::unread(pid, NOT_HERE)
        };
    }
    let listed = run_text("lsof", &["-nP", "-p", &pid.to_string(), "-Fftn"]);
    let (files, libraries) = match listed {
        Ok(text) => {
            let (files, libraries) = parse_lsof_files(&text);
            (Ok(files), Ok(libraries))
        }
        Err(why) => (Err(why.clone()), Err(why)),
    };
    ProcessDetails {
        files,
        libraries,
        sockets: net::sockets_of(pid),
        ..ProcessDetails::unread(pid, NOT_HERE)
    }
}

/// Every part from `/proc/<pid>`.
fn linux_details(pid: u32) -> ProcessDetails {
    let dir = format!("{PROC_DIR}/{pid}");
    let read = |name: &str| {
        fs::read_to_string(format!("{dir}/{name}"))
            .map_err(|e| denied(&format!("{dir}/{name}"), &e))
    };
    let maps = read("maps");
    ProcessDetails {
        environment: fs::read(format!("{dir}/environ"))
            .map(|bytes| parse_environ(&String::from_utf8_lossy(&bytes)))
            .map_err(|e| denied(&format!("{dir}/environ"), &e)),
        files: open_files(&dir),
        libraries: maps.as_deref().map(parse_libraries).map_err(Clone::clone),
        memory: read("smaps")
            .map(|text| parse_smaps(&text))
            .or_else(|_| maps.as_deref().map(parse_smaps).map_err(Clone::clone)),
        pid,
        sockets: net::sockets_of(pid),
        threads: threads(&dir),
    }
}

/// What `dir`'s process has open, by descriptor.
fn open_files(dir: &str) -> Result<Vec<OpenFile>, String> {
    let fds = format!("{dir}/fd");
    let listing = fs::read_dir(&fds).map_err(|e| denied(&fds, &e))?;
    let mut files: Vec<OpenFile> = listing
        .flatten()
        .filter_map(|entry| {
            let fd = entry.file_name().to_str()?.parse().ok()?;
            let target = fs::read_link(entry.path()).ok()?;
            Some(open_file(Some(fd), &target.to_string_lossy()))
        })
        .collect();
    files.sort_by_key(|file| file.fd);
    Ok(files)
}

/// The threads of `dir`'s process, by id.
fn threads(dir: &str) -> Result<Vec<ThreadInfo>, String> {
    let tasks = format!("{dir}/task");
    let listing = fs::read_dir(&tasks).map_err(|e| denied(&tasks, &e))?;
    let mut threads: Vec<ThreadInfo> = listing
        .flatten()
        .filter_map(|entry| {
            let tid = entry.file_name().to_str()?.parse().ok()?;
            let stat = parse_proc_stat(&fs::read_to_string(entry.path().join("stat")).ok()?)?;
            Some(ThreadInfo {
                name: stat.name,
                state: stat.state,
                tid,
            })
        })
        .collect();
    threads.sort_by_key(|thread| thread.tid);
    Ok(threads)
}

/// What an open descriptor is, from what its link names.
pub fn open_file(fd: Option<u32>, target: &str) -> OpenFile {
    let kind = if target.starts_with('/') {
        FileKind::File
    } else if target.starts_with(SOCKET_LINK) {
        FileKind::Socket
    } else if target.starts_with(PIPE_LINK) {
        FileKind::Pipe
    } else {
        FileKind::Other
    };
    OpenFile {
        fd,
        kind,
        target: target.to_string(),
    }
}

/// `environ`'s variables, NUL between each, in the order they were set.
pub fn parse_environ(raw: &str) -> Vec<(String, String)> {
    raw.split('\0')
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

/// The shared libraries a `maps` file names, each once, in load order.
pub fn parse_libraries(maps: &str) -> Vec<String> {
    let mut seen = Vec::new();
    for path in maps.lines().filter_map(mapping_path) {
        let file = path.rsplit('/').next().unwrap_or(path);
        let is_library = file.ends_with(LIBRARY_MARK) || file.contains(".so.");
        if is_library && !seen.iter().any(|known: &String| known == path) {
            seen.push(path.to_string());
        }
    }
    seen
}

/// A `smaps` file, or a `maps` file for want of one, as the memory each
/// source maps, the most resident first (the largest, where nothing says
/// what is resident).
pub fn parse_smaps(raw: &str) -> Vec<MemoryRegion> {
    let mut regions: Vec<MemoryRegion> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut current = None;
    for line in raw.lines() {
        if let Some(size_kb) = mapping_size_kb(line) {
            let name = mapping_path(line).unwrap_or(ANONYMOUS).to_string();
            let at = *index.entry(name.clone()).or_insert_with(|| {
                regions.push(MemoryRegion {
                    mappings: 0,
                    name,
                    resident_kb: None,
                    size_kb: 0,
                });
                regions.len() - 1
            });
            regions[at].mappings += 1;
            regions[at].size_kb += size_kb;
            current = Some(at);
        } else if let (Some(at), Some(rss)) = (current, line.strip_prefix("Rss:")) {
            let kb: u64 = rss
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .unwrap_or(0);
            *regions[at].resident_kb.get_or_insert(0) += kb;
        }
    }
    regions.sort_by(|a, b| {
        b.resident_kb
            .cmp(&a.resident_kb)
            .then(b.size_kb.cmp(&a.size_kb))
    });
    regions
}

/// The open files and the shared libraries `lsof -F ftn` lists: an `f` line
/// opens an entry with its descriptor (`txt` and `mem` for what is mapped
/// rather than opened), `t` gives its type, and `n` its name.
pub fn parse_lsof_files(raw: &str) -> (Vec<OpenFile>, Vec<String>) {
    let (mut files, mut libraries) = (Vec::new(), Vec::new());
    let mut fd: Option<&str> = None;
    let mut kind = "";
    for line in raw.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let value = &line[tag.len_utf8()..];
        match tag {
            'f' => {
                fd = Some(value);
                kind = "";
            }
            't' => kind = value,
            'n' => match fd {
                Some("txt" | "mem") => {
                    if value.contains(LIBRARY_MARK) && !libraries.iter().any(|l| l == value) {
                        libraries.push(value.to_string());
                    }
                }
                Some(number) => {
                    let mut file = open_file(number.parse().ok(), value);
                    file.kind = match kind {
                        "IPv4" | "IPv6" | "unix" => FileKind::Socket,
                        "PIPE" | "FIFO" => FileKind::Pipe,
                        _ => file.kind,
                    };
                    files.push(file);
                }
                None => {}
            },
            _ => {}
        }
    }
    (files, libraries)
}

/// The path a `maps` or `smaps` mapping line ends with, `[heap]` and the
/// like included, or `None` for an anonymous mapping or another line.
fn mapping_path(line: &str) -> Option<&str> {
    mapping_size_kb(line)?;
    let mut rest = line;
    for _ in 0..5 {
        rest = rest.trim_start();
        rest = &rest[rest.find(char::is_whitespace)?..];
    }
    Some(rest.trim()).filter(|path| !path.is_empty())
}

/// The kilobytes a mapping line's address range spans, or `None` for a line
/// that is not one (`Rss: 4 kB`).
fn mapping_size_kb(line: &str) -> Option<u64> {
    let range = line.split_whitespace().next()?;
    let (start, end) = range.split_once('-')?;
    let start = u64::from_str_radix(start, 16).ok()?;
    let end = u64::from_str_radix(end, 16).ok()?;
    Some(end.saturating_sub(start) / 1024)
}

/// Why `path` would not read, in a word the page can show.
fn denied(path: &str, error: &std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "not allowed: the process belongs to another user".to_string()
        }
        std::io::ErrorKind::NotFound => "the process has ended".to_string(),
        _ => format!("cannot read {path}: {error}"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SMAPS: &str = "\
55d0c0000000-55d0c0002000 r--p 00000000 103:02 1234   /usr/bin/cat
Rss:                   8 kB
55d0c0002000-55d0c0006000 r-xp 00002000 103:02 1234   /usr/bin/cat
Rss:                  12 kB
55d0c1000000-55d0c1400000 rw-p 00000000 00:00 0      [heap]
Rss:                1024 kB
7f0000000000-7f0000100000 rw-p 00000000 00:00 0
Rss:                 512 kB
7f0000100000-7f0000200000 r-xp 00000000 103:02 99    /usr/lib/libc.so.6
Rss:                 600 kB
";

    #[test]
    fn test_memory_groups_mappings_by_source_most_resident_first() {
        let regions = parse_smaps(SMAPS);
        let names: Vec<&str> = regions.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            ["[heap]", "/usr/lib/libc.so.6", "[anon]", "/usr/bin/cat"]
        );
        let cat = &regions[3];
        assert_eq!(
            (cat.mappings, cat.size_kb, cat.resident_kb),
            (2, 24, Some(20))
        );
    }

    #[test]
    fn test_a_maps_file_gives_sizes_without_what_is_resident() {
        let maps: String = SMAPS
            .lines()
            .filter(|l| !l.starts_with("Rss"))
            .collect::<Vec<_>>()
            .join("\n");
        let regions = parse_smaps(&maps);
        assert!(regions.iter().all(|region| region.resident_kb.is_none()));
        assert_eq!(regions[0].name, "[heap]", "the largest first: {regions:?}");
    }

    #[test]
    fn test_libraries_are_the_shared_objects_mapped_each_once() {
        let maps = format!(
            "{SMAPS}7f0000200000-7f0000300000 r--p 00000000 103:02 99 /usr/lib/libc.so.6\n"
        );
        assert_eq!(parse_libraries(&maps), ["/usr/lib/libc.so.6"]);
    }

    #[test]
    fn test_descriptor_links_are_sorted_into_files_pipes_and_sockets() {
        assert_eq!(open_file(Some(0), "/dev/pts/3").kind, FileKind::File);
        assert_eq!(open_file(Some(1), "pipe:[123]").kind, FileKind::Pipe);
        assert_eq!(open_file(Some(2), "socket:[456]").kind, FileKind::Socket);
        assert_eq!(
            open_file(Some(3), "anon_inode:[eventfd]").kind,
            FileKind::Other
        );
    }

    #[test]
    fn test_environ_splits_on_nul_and_keeps_an_equals_sign_in_a_value() {
        let env = parse_environ("HOME=/home/me\0OPTS=a=b\0\0");
        assert_eq!(
            env,
            [
                ("HOME".to_string(), "/home/me".to_string()),
                ("OPTS".to_string(), "a=b".to_string())
            ]
        );
    }

    #[test]
    fn test_lsof_splits_mapped_libraries_from_open_descriptors() {
        let raw = "p42\nftxt\ntREG\nn/usr/bin/node\nfmem\ntREG\nn/usr/lib/libssl.so.3\nf0\ntCHR\nn/dev/null\nf7\ntIPv4\nn127.0.0.1:3000\nf8\ntPIPE\nn->0x1234\n";
        let (files, libraries) = parse_lsof_files(raw);
        assert_eq!(libraries, ["/usr/lib/libssl.so.3"]);
        let kinds: Vec<FileKind> = files.iter().map(|file| file.kind).collect();
        assert_eq!(kinds, [FileKind::File, FileKind::Socket, FileKind::Pipe]);
        assert_eq!(files[1].fd, Some(7));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_this_process_reads_its_own_files_libraries_memory_and_threads() {
        // The one check against the live /proc: a column read from the
        // wrong field gives plausible but empty parts.
        let details = details(std::process::id());
        assert!(!details.files.unwrap().is_empty());
        assert!(!details.memory.unwrap().is_empty());
        assert!(!details.threads.unwrap().is_empty());
        assert!(
            details
                .environment
                .unwrap()
                .iter()
                .any(|(name, _)| name == "PATH")
        );
    }
}
