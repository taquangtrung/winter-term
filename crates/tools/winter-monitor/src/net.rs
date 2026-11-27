//! Listing the machine's sockets and the process holding each: the tables
//! Linux keeps under `/proc/net` matched to each process's descriptors,
//! `lsof` elsewhere on Unix, and `netstat -ano` on Windows. Shared by the
//! Ports monitor and a process's details.

use std::collections::HashMap;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};

use winter_model::monitor::{Protocol, Socket};

use crate::run::run_text;

// ========================================================================
// Constants
// ========================================================================

/// Where Linux keeps what it knows about processes and sockets.
const PROC_DIR: &str = "/proc";

/// The socket tables under `/proc/net`, each with its protocol.
const PROC_TABLES: [(&str, Protocol); 4] = [
    ("tcp", Protocol::Tcp),
    ("tcp6", Protocol::Tcp),
    ("udp", Protocol::Udp),
    ("udp6", Protocol::Udp),
];

/// How a descriptor's link names the socket it is: `socket:[inode]`.
const SOCKET_LINK: (&str, char) = ("socket:[", ']');

/// The TCP states the kernel numbers, in its order from one.
const TCP_STATES: [&str; 11] = [
    "ESTABLISHED",
    "SYN_SENT",
    "SYN_RECV",
    "FIN_WAIT1",
    "FIN_WAIT2",
    "TIME_WAIT",
    "CLOSE",
    "CLOSE_WAIT",
    "LAST_ACK",
    "LISTEN",
    "CLOSING",
];

/// The state a listening TCP socket is in, however a system spells it.
const LISTEN: &str = "LISTEN";
const WINDOWS_LISTEN: &str = "LISTENING";

// ========================================================================
// Functions
// ========================================================================

/// Every socket on the machine with the process holding it, or why they
/// could not be listed.
pub fn sockets() -> Result<Vec<Socket>, String> {
    if cfg!(windows) {
        let text = run_text("netstat", &["-ano"])?;
        return Ok(parse_netstat_ano(&text));
    }
    if cfg!(target_os = "linux") {
        let mut sockets = proc_sockets()?;
        let owners = socket_owners();
        for (inode, socket) in &mut sockets {
            if let Some((pid, program)) = owners.get(inode) {
                socket.pid = Some(*pid);
                socket.program = Some(program.clone());
            }
        }
        return Ok(sockets.into_iter().map(|(_, socket)| socket).collect());
    }
    let text = run_text("lsof", &["-nP", "-iTCP", "-iUDP", "-FpcPnT"])?;
    Ok(parse_lsof(&text))
}

/// The sockets process `pid` holds, or why they could not be listed.
pub fn sockets_of(pid: u32) -> Result<Vec<Socket>, String> {
    if !cfg!(target_os = "linux") {
        return Ok(sockets()?
            .into_iter()
            .filter(|socket| socket.pid == Some(pid))
            .collect());
    }
    let held = socket_inodes(pid)?;
    Ok(proc_sockets()?
        .into_iter()
        .filter(|(inode, _)| held.contains(inode))
        .map(|(_, socket)| socket)
        .collect())
}

/// Every socket in Linux's tables, each with its inode, the key to the
/// process holding it.
fn proc_sockets() -> Result<Vec<(u64, Socket)>, String> {
    let mut sockets = Vec::new();
    let mut read_any = false;
    for (table, protocol) in PROC_TABLES {
        let Ok(text) = fs::read_to_string(format!("{PROC_DIR}/net/{table}")) else {
            continue;
        };
        read_any = true;
        sockets.extend(parse_proc_net(&text, protocol));
    }
    match read_any {
        true => Ok(sockets),
        false => Err(format!(
            "cannot read the socket tables under {PROC_DIR}/net"
        )),
    }
}

/// Each socket inode a process holds, with its id and name, by reading every
/// process's descriptors. Another user's processes cannot be read, and
/// their sockets keep no owner.
fn socket_owners() -> HashMap<u64, (u32, String)> {
    let mut owners = HashMap::new();
    let Ok(listing) = fs::read_dir(PROC_DIR) else {
        return owners;
    };
    for entry in listing.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(inodes) = socket_inodes(pid) else {
            continue;
        };
        if inodes.is_empty() {
            continue;
        }
        let name = fs::read_to_string(format!("{PROC_DIR}/{pid}/comm"))
            .map(|name| name.trim().to_string())
            .unwrap_or_default();
        for inode in inodes {
            owners.entry(inode).or_insert_with(|| (pid, name.clone()));
        }
    }
    owners
}

/// The inodes of the sockets process `pid` has open.
fn socket_inodes(pid: u32) -> Result<Vec<u64>, String> {
    let dir = format!("{PROC_DIR}/{pid}/fd");
    let listing = fs::read_dir(&dir).map_err(|e| format!("cannot read {dir}: {e}"))?;
    Ok(listing
        .flatten()
        .filter_map(|entry| fs::read_link(entry.path()).ok())
        .filter_map(|link| socket_inode(&link.to_string_lossy()))
        .collect())
}

/// The inode in a descriptor link naming a socket, or `None` for another.
pub fn socket_inode(link: &str) -> Option<u64> {
    link.strip_prefix(SOCKET_LINK.0)?
        .strip_suffix(SOCKET_LINK.1)?
        .parse()
        .ok()
}

/// The sockets of one `/proc/net` table, each with its inode.
pub fn parse_proc_net(raw: &str, protocol: Protocol) -> Vec<(u64, Socket)> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let (local, port) = proc_address(fields.get(1)?)?;
            let (remote, remote_port) = proc_address(fields.get(2)?)?;
            let state_code = usize::from_str_radix(fields.get(3)?, 16).ok()?;
            let inode = fields.get(9)?.parse().ok()?;
            let connected = remote_port != 0;
            let state = match protocol {
                Protocol::Tcp => TCP_STATES
                    .get(state_code.checked_sub(1)?)
                    .copied()
                    .unwrap_or_default()
                    .to_string(),
                Protocol::Udp => String::new(),
            };
            let listening = match protocol {
                Protocol::Tcp => state == LISTEN,
                Protocol::Udp => !connected,
            };
            Some((
                inode,
                Socket {
                    listening,
                    local,
                    pid: None,
                    port,
                    program: None,
                    protocol,
                    remote: connected.then_some(remote),
                    state,
                },
            ))
        })
        .collect()
}

/// A `/proc/net` address (`0100007F:0BB8`) as `127.0.0.1:3000`, with its
/// port. The address is in the kernel's byte order, a 32-bit word at a time.
fn proc_address(field: &str) -> Option<(String, u16)> {
    let (address, port) = field.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let words: Vec<u32> = (0..address.len() / 8)
        .map(|at| u32::from_str_radix(&address[at * 8..at * 8 + 8], 16).map(u32::swap_bytes))
        .collect::<Result<_, _>>()
        .ok()?;
    let shown = match words.as_slice() {
        [word] => Ipv4Addr::from(*word).to_string(),
        [a, b, c, d] => {
            let bytes: Vec<u8> = [a, b, c, d]
                .iter()
                .flat_map(|word| word.to_be_bytes())
                .collect();
            let bytes: [u8; 16] = bytes.try_into().ok()?;
            format!("[{}]", Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    Some((format!("{shown}:{port}"), port))
}

/// The sockets `lsof -F pcPnT` lists: a `p` line opens a process, `c` names
/// it, and each `f`-led group under it is a socket with its `P` protocol,
/// `n` addresses, and `T` state.
pub fn parse_lsof(raw: &str) -> Vec<Socket> {
    let mut sockets = Vec::new();
    let (mut pid, mut program) = (None, None);
    for line in raw.lines() {
        let Some(tag) = line.chars().next() else {
            continue;
        };
        let value = &line[tag.len_utf8()..];
        match tag {
            'p' => {
                pid = value.parse().ok();
                program = None;
            }
            'c' => program = Some(value.to_string()),
            'P' => {
                let protocol = match value {
                    "UDP" => Protocol::Udp,
                    _ => Protocol::Tcp,
                };
                sockets.push(Socket {
                    listening: protocol == Protocol::Udp,
                    local: String::new(),
                    pid,
                    port: 0,
                    program: program.clone(),
                    protocol,
                    remote: None,
                    state: String::new(),
                });
            }
            'n' => {
                let Some(socket) = sockets.last_mut() else {
                    continue;
                };
                let (local, remote) = match value.split_once("->") {
                    Some((local, remote)) => (local, Some(remote.to_string())),
                    None => (value, None),
                };
                socket.port = port_of(local);
                socket.local = local.to_string();
                socket.listening &= remote.is_none();
                socket.remote = remote;
            }
            'T' => {
                if let (Some(socket), Some(state)) = (sockets.last_mut(), value.strip_prefix("ST="))
                {
                    socket.listening = state == LISTEN;
                    socket.state = state.to_string();
                }
            }
            _ => {}
        }
    }
    sockets
}

/// The sockets `netstat -ano` lists on Windows: protocol, local, remote, a
/// state for TCP alone, and the process id.
pub fn parse_netstat_ano(raw: &str) -> Vec<Socket> {
    raw.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            let protocol = match *fields.first()? {
                "TCP" => Protocol::Tcp,
                "UDP" => Protocol::Udp,
                _ => return None,
            };
            let (state, pid) = match (protocol, fields.len()) {
                (Protocol::Tcp, 5) => (fields[3].to_string(), fields[4]),
                (Protocol::Udp, 4) => (String::new(), fields[3]),
                _ => return None,
            };
            let local = fields[1].to_string();
            let remote = Some(fields[2])
                .filter(|remote| !matches!(*remote, "*:*" | "0.0.0.0:0" | "[::]:0"))
                .map(str::to_string);
            let listening = match protocol {
                Protocol::Tcp => state == WINDOWS_LISTEN,
                Protocol::Udp => remote.is_none(),
            };
            Some(Socket {
                listening,
                port: port_of(&local),
                local,
                pid: pid.parse().ok(),
                program: None,
                protocol,
                remote,
                state: match state.as_str() {
                    WINDOWS_LISTEN => LISTEN.to_string(),
                    _ => state,
                },
            })
        })
        .collect()
}

/// The port after an address's last colon, nought for `*` or none.
fn port_of(address: &str) -> u16 {
    address
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse().ok())
        .unwrap_or(0)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_proc_net_reads_addresses_in_kernel_order_and_states_by_number() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 4242 1
   1: 0100007F:0BB8 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 4243 1
";
        let sockets = parse_proc_net(tcp, Protocol::Tcp);
        let (inode, listener) = &sockets[0];
        assert_eq!(*inode, 4242);
        assert_eq!(listener.local, "127.0.0.1:3000");
        assert!(listener.listening && listener.remote.is_none());
        let (_, client) = &sockets[1];
        assert_eq!(client.state, "ESTABLISHED");
        assert_eq!(client.remote.as_deref(), Some("127.0.0.1:54321"));
        assert!(!client.listening);
    }

    #[test]
    fn test_ipv6_addresses_and_unbound_udp_read_as_listening() {
        let udp6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops
  0: 00000000000000000000000001000000:14E9 00000000000000000000000000000000:0000 07 00000000:00000000 00:00000000 00000000   100        0 777 2 0 0
";
        let (_, socket) = &parse_proc_net(udp6, Protocol::Udp)[0];
        assert_eq!(socket.local, "[::1]:5353");
        assert_eq!(socket.port, 5353);
        assert!(
            socket.listening,
            "a UDP socket bound to no peer waits for any"
        );
    }

    #[test]
    fn test_a_socket_descriptor_link_gives_its_inode_and_others_none() {
        assert_eq!(socket_inode("socket:[4242]"), Some(4242));
        assert_eq!(socket_inode("pipe:[4242]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }

    #[test]
    fn test_lsof_groups_sockets_under_their_process() {
        let raw = "p812\ncnode\nf23\nPTCP\nn127.0.0.1:3000\nTST=LISTEN\nf24\nPTCP\nn127.0.0.1:3000->127.0.0.1:50000\nTST=ESTABLISHED\np90\ncmDNSResponder\nf5\nPUDP\nn*:5353\n";
        let sockets = parse_lsof(raw);
        assert_eq!(sockets.len(), 3);
        assert_eq!(sockets[0].pid, Some(812));
        assert_eq!(sockets[0].program.as_deref(), Some("node"));
        assert!(sockets[0].listening && sockets[0].port == 3000);
        assert!(!sockets[1].listening);
        assert_eq!(sockets[1].remote.as_deref(), Some("127.0.0.1:50000"));
        assert_eq!(sockets[2].program.as_deref(), Some("mDNSResponder"));
        assert!(sockets[2].listening);
    }

    #[test]
    fn test_netstat_ano_reads_tcp_with_a_state_and_udp_without() {
        let raw = "
Active Connections

  Proto  Local Address          Foreign Address        State           PID
  TCP    0.0.0.0:135            0.0.0.0:0              LISTENING       1012
  TCP    10.0.0.5:50123         140.82.112.4:443       ESTABLISHED     6400
  UDP    0.0.0.0:5353           *:*                                    2210
";
        let sockets = parse_netstat_ano(raw);
        assert_eq!(sockets.len(), 3);
        assert!(sockets[0].listening && sockets[0].state == LISTEN);
        assert_eq!(sockets[0].pid, Some(1012));
        assert_eq!(sockets[1].remote.as_deref(), Some("140.82.112.4:443"));
        assert!(sockets[2].listening && sockets[2].port == 5353);
    }
}
