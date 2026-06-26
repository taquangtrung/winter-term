//! Remote places as URIs (RFC 3986), the way the desktop file managers
//! name them:
//!
//! - `sudo:///etc/hosts`: local root, reached through `sudo` (`sudo:/x` is
//!   read too, and so is `admin:`, the name GVfs and KIO give it).
//! - `ssh://[user@]host[:port]/path`, with `sftp://` read as the same and an
//!   IPv6 host in brackets.
//! - `docker://[user@]container/path`.
//!
//! `ssh://` and `docker://` with nothing after them name the list of hosts
//! and containers there are to go to.
//!
//! What is not plain ASCII punctuation-free text is percent-encoded when a
//! URI is printed, except characters beyond ASCII, which an IRI (RFC 3987)
//! keeps readable.

use super::{Container, Kind, Method, Place, Remote, SshHost};

// ========================================================================
// Constants
// ========================================================================

pub(super) const SUDO: &str = "sudo";
pub(super) const DOCKER: &str = "docker";
pub(super) const SSH: &str = "ssh";

/// Read as `ssh`: the files are the same, whichever protocol a file
/// manager would have reached them with.
const SFTP: &str = "sftp";

/// Read as `sudo`: what GNOME Files and Dolphin call root, and what Winter
/// wrote before, so a path saved then still opens.
const ADMIN: &str = "admin";

/// What opens a URI's authority, after the scheme's `:`.
const AUTHORITY: &str = "//";

/// The one host a `sudo:` URI may name, which is this machine.
const LOCALHOST: &str = "localhost";

/// ASCII punctuation a path keeps as it is: RFC 3986's unreserved and
/// sub-delimiter characters, and the `:`, `@`, and `/` a path may hold.
const PATH_SAFE: &str = "-._~!$&'()*+,;=:@/";

/// Characters a host or user name may hold beyond letters and digits.
const NAME_PUNCTUATION: [char; 3] = ['.', '_', '-'];

/// Characters an IPv6 literal may hold beyond hex digits: the colons, a
/// dotted IPv4 tail, and a zone index.
const IPV6_PUNCTUATION: [char; 3] = [':', '.', '%'];

// ========================================================================
// Functions
// ========================================================================

/// What `text` names, or `None` where it is not a remote URI Winter knows.
pub(super) fn parse(text: &str) -> Option<Place> {
    let (scheme, rest) = text.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    // A query or a fragment means something no file has.
    if rest.contains(['?', '#']) {
        return None;
    }
    if scheme == SUDO || scheme == ADMIN {
        return parse_sudo(rest);
    }
    let kind = match scheme.as_str() {
        DOCKER => Kind::Docker,
        SSH | SFTP => Kind::Ssh,
        _ => return None,
    };
    let rest = rest.strip_prefix(AUTHORITY)?;
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let path = decode(path)?;
    if authority.is_empty() {
        return matches!(path.as_str(), "" | "/").then_some(Place::Targets(kind));
    }
    let method = match kind {
        Kind::Docker => Method::Docker(parse_container(authority)?),
        Kind::Ssh => Method::Ssh(parse_ssh_host(authority)?),
    };
    Some(Place::Remote(Remote::new(method, &path)))
}

/// The rest of a `sudo:` URI: `///path`, `//localhost/path`, or `/path`.
fn parse_sudo(rest: &str) -> Option<Place> {
    let path = match rest.strip_prefix(AUTHORITY) {
        Some(rest) => {
            let (host, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
            if !host.is_empty() && !host.eq_ignore_ascii_case(LOCALHOST) {
                return None;
            }
            path
        }
        None if rest.starts_with('/') => rest,
        None => return None,
    };
    Some(Place::Remote(Remote::new(Method::Sudo, &decode(path)?)))
}

/// `[user@]name`.
fn parse_container(authority: &str) -> Option<Container> {
    let (user, name) = split_user(authority)?;
    let name = decode(name)?;
    is_name(&name).then_some(Container { name, user })
}

/// `[user@]host[:port]`, the host a name or a bracketed IPv6 literal.
fn parse_ssh_host(authority: &str) -> Option<SshHost> {
    let (user, host_port) = split_user(authority)?;
    let (host, port) = match host_port.strip_prefix('[') {
        Some(bracketed) => {
            let (host, after) = bracketed.split_once(']')?;
            if !is_ipv6(host) {
                return None;
            }
            (host.to_string(), after.strip_prefix(':'))
        }
        None => {
            let (host, port) = match host_port.split_once(':') {
                Some((host, port)) => (host, Some(port)),
                None => (host_port, None),
            };
            let host = decode(host)?;
            if !is_name(&host) {
                return None;
            }
            (host, port)
        }
    };
    let port = match port {
        Some(port) => Some(port.parse::<u16>().ok().filter(|&port| port > 0)?),
        None => None,
    };
    Some(SshHost { host, port, user })
}

/// The user an authority names before `@`, and the rest. A password in
/// it is refused rather than carried around in a path.
fn split_user(authority: &str) -> Option<(Option<String>, &str)> {
    let Some((user, rest)) = authority.rsplit_once('@') else {
        return Some((None, authority));
    };
    let user = decode(user)?;
    is_name(&user).then_some((Some(user), rest))
}

/// `remote` as a URI.
pub(super) fn format(remote: &Remote) -> String {
    format!(
        "{}{}",
        authority_of(&remote.method),
        encode_path(&remote.path)
    )
}

/// The scheme and authority of a URI reaching `method`: everything before
/// the path, such as `ssh://me@host:2222`.
pub(super) fn authority_of(method: &Method) -> String {
    match method {
        Method::Sudo => format!("{SUDO}:{AUTHORITY}"),
        Method::Docker(container) => format!(
            "{DOCKER}:{AUTHORITY}{}{}",
            user_part(container.user.as_deref()),
            container.name
        ),
        Method::Ssh(host) => {
            let name = match host.host.contains(':') {
                true => format!("[{}]", host.host),
                false => host.host.clone(),
            };
            let port = host.port.map(|port| format!(":{port}")).unwrap_or_default();
            format!(
                "{SSH}:{AUTHORITY}{}{name}{port}",
                user_part(host.user.as_deref())
            )
        }
    }
}

/// The URI naming every target of `kind`.
pub(super) fn targets_uri(kind: Kind) -> String {
    let scheme = match kind {
        Kind::Docker => DOCKER,
        Kind::Ssh => SSH,
    };
    format!("{scheme}:{AUTHORITY}")
}

/// `user@`, or nothing.
fn user_part(user: Option<&str>) -> String {
    user.map(|user| format!("{user}@")).unwrap_or_default()
}

/// `path` with what a URI path cannot hold percent-encoded.
fn encode_path(path: &str) -> String {
    let mut encoded = String::with_capacity(path.len());
    for c in path.chars() {
        let keep = !c.is_ascii() || c.is_ascii_alphanumeric() || PATH_SAFE.contains(c);
        match keep {
            true => encoded.push(c),
            false => encoded.push_str(&format!("%{:02X}", c as u32)),
        }
    }
    encoded
}

/// `text` with its `%XX` escapes decoded, or `None` for a malformed escape
/// or bytes that are not UTF-8.
fn decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            b'%' => {
                let hex = text.get(at + 1..at + 3)?;
                decoded.push(u8::from_str_radix(hex, 16).ok()?);
                at += 3;
            }
            byte => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(decoded).ok()
}

/// Letters, digits, `.`, `_`, `-`, at least one, not starting with `-`: a
/// name reaches `docker` and `ssh` as an argument, where one starting with
/// `-` would be read as an option.
pub(super) fn is_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || NAME_PUNCTUATION.contains(&c))
}

/// Whether `host` can be the inside of an IPv6 literal's brackets.
fn is_ipv6(host: &str) -> bool {
    host.contains(':')
        && host
            .chars()
            .all(|c| c.is_ascii_hexdigit() || IPV6_PUNCTUATION.contains(&c))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(text: &str) -> Remote {
        match parse(text) {
            Some(Place::Remote(remote)) => remote,
            other => panic!("{text} is not a remote file: {other:?}"),
        }
    }

    #[test]
    fn test_each_scheme_prints_back_the_uri_it_was_read_from() {
        for uri in [
            "sudo:///etc/hosts",
            "docker://web/etc/nginx/nginx.conf",
            "docker://www-data@web/var/www",
            "ssh://deploy@example.com:2222/srv/app",
            "ssh://[::1]:22/etc",
            "ssh://host/café%20menu.txt",
        ] {
            assert_eq!(format(&remote(uri)), uri);
        }
    }

    #[test]
    fn test_the_other_spellings_read_as_the_canonical_one() {
        assert_eq!(format(&remote("sudo:/etc/hosts")), "sudo:///etc/hosts");
        assert_eq!(format(&remote("admin:///etc/hosts")), "sudo:///etc/hosts");
        assert_eq!(format(&remote("admin://localhost/etc")), "sudo:///etc");
        assert_eq!(format(&remote("sftp://host/x")), "ssh://host/x");
        assert_eq!(format(&remote("SSH://host")), "ssh://host/");
        let spaced = remote("ssh://host/a%20b/%25");
        assert_eq!(spaced.path, "/a b/%", "escapes decode into the path");
    }

    #[test]
    fn test_a_bare_scheme_names_its_list_of_targets() {
        assert_eq!(parse("docker://"), Some(Place::Targets(Kind::Docker)));
        assert_eq!(parse("ssh:///"), Some(Place::Targets(Kind::Ssh)));
        assert_eq!(
            parse(&targets_uri(Kind::Ssh)),
            Some(Place::Targets(Kind::Ssh))
        );
    }

    #[test]
    fn test_what_is_not_a_safe_remote_file_is_refused() {
        for text in [
            "/etc/hosts",
            "C:\\Users\\me",
            "https://example.com/x",
            "sudo://otherhost/etc",
            "sudo:relative",
            "ssh:/host/x",
            "ssh://-oProxyCommand=x/",
            "ssh://user:secret@host/",
            "ssh://host:0/",
            "ssh://host:99999/",
            "ssh://host/x?y",
            "ssh://host/x#y",
            "ssh://[not-ipv6]/",
            "ssh://host/%zz",
            "docker://-it/etc",
            "docker://web:22/etc",
            "docker:///etc",
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
    }
}
