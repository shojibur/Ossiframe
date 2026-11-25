//! The `listening-sockets` operation: TCP and UDP listeners from the fixed
//! command `ss -H -tulnp`.
//!
//! Process names and pids appear only for sockets visible to the SSH user.
//! Other sockets have `process: null`, which means "not visible", never
//! "no process".

use crate::inventory::{Probe, ProbeError, probe_strict};

use ossiframe_core::{CancelToken, ExecError, Executor};
use serde::Serialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Most sockets returned. Truncation is recorded in `truncated`.
pub const MAX_SOCKETS: usize = 300;
/// Most lines read, to bound work on hostile or huge output.
const MAX_LINES: usize = 5000;
/// Longer lines are skipped, not cut.
const MAX_LINE: usize = 2048;
const MAX_NAME: usize = 64;

const SS_ARGS: [&str; 2] = ["-H", "-tulnp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Tcp,
    Udp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    Ipv4,
    Ipv6,
    /// `*`: `ss` does not say which family.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingClass {
    Loopback,
    Wildcard,
    Specific,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SocketProcess {
    pub name: String,
    pub pid: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningSocket {
    pub protocol: Protocol,
    /// Local address without brackets and without a zone suffix; `*` stays `*`.
    pub address: String,
    pub port: u16,
    pub family: Family,
    pub binding: BindingClass,
    /// Private, unique-local or link-local address (RFC 1918, `fc00::/7`,
    /// `169.254.0.0/16`, `fe80::/10`). Always false for loopback and wildcard.
    pub private: bool,
    /// `None` when `ss` did not show a process (not visible to this user).
    pub process: Option<SocketProcess>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SocketsInfo {
    pub observed_at: String,
    /// False when the command failed; `sockets` is empty.
    pub available: bool,
    pub sockets: Vec<ListeningSocket>,
    /// True when the list was cut; rules must treat it as incomplete.
    pub truncated: bool,
    /// Non-empty output lines that were not recognized and are not listed.
    /// Rules must treat a non-zero count as incomplete.
    pub unrecognized_lines: usize,
    pub limitations: Vec<String>,
    pub probe_errors: Vec<ProbeError>,
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_link_local()
}

fn is_private_v6(ip: Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
}

/// Classifies an address string (brackets and zone already removed).
fn classify(host: &str) -> Option<(Family, BindingClass, bool)> {
    if host == "*" {
        return Some((Family::Unknown, BindingClass::Wildcard, false));
    }
    match host.parse::<IpAddr>().ok()? {
        IpAddr::V4(ip) => Some(classify_v4(ip, Family::Ipv4)),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => Some(classify_v4(v4, Family::Ipv6)),
            None if ip.is_loopback() => Some((Family::Ipv6, BindingClass::Loopback, false)),
            None if ip.is_unspecified() => Some((Family::Ipv6, BindingClass::Wildcard, false)),
            None => Some((Family::Ipv6, BindingClass::Specific, is_private_v6(ip))),
        },
    }
}

fn classify_v4(ip: Ipv4Addr, family: Family) -> (Family, BindingClass, bool) {
    if ip.is_loopback() {
        (family, BindingClass::Loopback, false)
    } else if ip.is_unspecified() {
        (family, BindingClass::Wildcard, false)
    } else {
        (family, BindingClass::Specific, is_private_v4(ip))
    }
}

/// Parses `users:(("name",pid=123,fd=4),...)`; the first entry is used.
fn parse_process(line: &str) -> Option<SocketProcess> {
    let rest = &line[line.find("users:((\"")? + "users:((\"".len()..];
    let end = rest.find("\",pid=")?;
    let name: String = rest[..end]
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_NAME)
        .collect();
    let digits: String = rest[end + "\",pid=".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .take(10)
        .collect();
    let pid = digits.parse().ok()?;
    if name.is_empty() {
        return None;
    }
    Some(SocketProcess { name, pid })
}

/// Parses one `ss -H -tulnp` line. `None` for anything unrecognized.
fn parse_line(line: &str) -> Option<ListeningSocket> {
    if line.len() > MAX_LINE {
        return None;
    }
    let mut cols = line.split_whitespace();
    let protocol = match cols.next()? {
        "tcp" => Protocol::Tcp,
        "udp" => Protocol::Udp,
        _ => return None,
    };
    // State, Recv-Q, Send-Q, then the local address.
    let local = cols.nth(3)?;
    let (host, port) = local.rsplit_once(':')?;
    let port: u16 = port.parse().ok()?;
    // Accepted forms: `addr`, `addr%zone`, `[addr]`, `[addr%zone]` and
    // `[addr]%zone` (the last is what `ss` prints for link-local IPv6).
    let address = match host.strip_prefix('[') {
        Some(inner) => {
            let (addr, after) = match inner.split_once(']') {
                Some((addr, after)) => (addr, after),
                None => return None,
            };
            if !after.is_empty() && !after.starts_with('%') {
                return None;
            }
            addr.split('%').next().unwrap_or("")
        }
        None => host.split('%').next().unwrap_or(""),
    };
    let (family, binding, private) = classify(address)?;
    Some(ListeningSocket {
        protocol,
        address: address.to_owned(),
        port,
        family,
        binding,
        private,
        process: parse_process(line),
    })
}

/// Parsed sockets, whether the cap cut the list, and the number of
/// non-empty lines that were not recognized.
struct Parsed {
    sockets: Vec<ListeningSocket>,
    truncated: bool,
    skipped: usize,
}

fn parse(s: &str) -> Parsed {
    let mut parsed = Parsed {
        sockets: Vec::new(),
        truncated: false,
        skipped: 0,
    };
    for (i, line) in s.lines().enumerate() {
        if i >= MAX_LINES {
            parsed.truncated = true;
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        match parse_line(line) {
            Some(socket) if parsed.sockets.len() < MAX_SOCKETS => parsed.sockets.push(socket),
            Some(_) => {
                parsed.truncated = true;
                break;
            }
            None => parsed.skipped += 1,
        }
    }
    parsed
}

/// Runs `ss -H -tulnp`. A failed or cut command gives `available = false`.
/// Cancel or a lost connection returns the error.
pub async fn collect(
    exec: &dyn Executor,
    cancel: &CancelToken,
    observed_at: String,
) -> Result<SocketsInfo, ExecError> {
    let output = match probe_strict(exec, "ss", &SS_ARGS, cancel).await {
        Probe::Output(o) => o,
        Probe::Failed(message) => {
            return Ok(SocketsInfo {
                observed_at,
                probe_errors: vec![ProbeError {
                    probe: "listeningSockets",
                    message,
                }],
                ..Default::default()
            });
        }
        Probe::Abort(e) => return Err(e),
    };
    let parsed = parse(&output);
    let mut limitations = Vec::new();
    if parsed.truncated {
        limitations.push(format!(
            "The socket list is incomplete: at most {MAX_SOCKETS} sockets are listed."
        ));
    }
    if parsed.skipped > 0 {
        limitations.push(format!(
            "{} line(s) of socket output were not recognized and are not listed.",
            parsed.skipped
        ));
    }
    if parsed.sockets.iter().any(|s| s.process.is_none()) {
        limitations.push(
            "Process names are shown only for sockets visible to the SSH user; \
             others are not visible."
                .to_owned(),
        );
    }
    Ok(SocketsInfo {
        observed_at,
        available: true,
        sockets: parsed.sockets,
        truncated: parsed.truncated,
        unrecognized_lines: parsed.skipped,
        limitations,
        probe_errors: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{CommandSpec, ExecResult, ExitStatus};

    fn one(line: &str) -> ListeningSocket {
        parse_line(line).unwrap_or_else(|| panic!("not parsed: {line}"))
    }

    #[test]
    fn ipv6_wildcard_with_process() {
        let s = one(
            "tcp   LISTEN 0 128 [::]:22 [::]:* users:((\"sshd\",pid=812,fd=4),(\"sshd\",pid=1,fd=5))",
        );
        assert_eq!(
            (s.protocol, s.family, s.binding),
            (Protocol::Tcp, Family::Ipv6, BindingClass::Wildcard)
        );
        assert_eq!((s.address.as_str(), s.port), ("::", 22));
        assert_eq!(
            s.process,
            Some(SocketProcess {
                name: "sshd".into(),
                pid: 812
            })
        );
    }

    #[test]
    fn star_without_process_is_wildcard_with_null_process() {
        let s = one("tcp LISTEN 0 511 *:80 *:*");
        assert_eq!(s.address, "*");
        assert_eq!(
            (s.family, s.binding, s.port),
            (Family::Unknown, BindingClass::Wildcard, 80)
        );
        assert_eq!(s.process, None);
    }

    #[test]
    fn zone_suffix_is_stripped_before_classifying() {
        let s = one(
            "udp UNCONN 0 0 127.0.0.53%lo:53 0.0.0.0:* users:((\"systemd-resolve\",pid=600,fd=13))",
        );
        assert_eq!(s.address, "127.0.0.53");
        assert_eq!(
            (s.protocol, s.binding, s.private),
            (Protocol::Udp, BindingClass::Loopback, false)
        );
        let s = one("udp UNCONN 0 0 [fe80::1%eth0]:546 [::]:*");
        assert_eq!(s.address, "fe80::1");
        assert_eq!((s.binding, s.private), (BindingClass::Specific, true));
    }

    #[test]
    fn zone_after_closing_bracket_is_accepted() {
        let s = one("udp UNCONN 0      0      [fe80::4ff:d5ff:fef0:17f]%ens5:546    [::]:*");
        assert_eq!(s.address, "fe80::4ff:d5ff:fef0:17f");
        assert_eq!((s.port, s.family), (546, Family::Ipv6));
        assert_eq!((s.binding, s.private), (BindingClass::Specific, true));
        assert!(parse_line("udp UNCONN 0 0 [fe80::1]x:546 [::]:*").is_none());
        assert!(parse_line("udp UNCONN 0 0 [fe80::1:546 [::]:*").is_none());
    }

    #[test]
    fn reference_like_output_is_fully_recognized() {
        let out = "\
udp UNCONN 0      0                          127.0.0.54:53  0.0.0.0:*
udp UNCONN 0      0                       127.0.0.53%lo:53  0.0.0.0:*
udp UNCONN 0      0                           127.0.0.1:323 0.0.0.0:*
udp UNCONN 0      0                   172.26.3.117%ens5:68  0.0.0.0:*
udp UNCONN 0      0      [fe80::4ff:d5ff:fef0:17f]%ens5:546    [::]:*
udp UNCONN 0      0                               [::1]:323    [::]:*
tcp LISTEN 0      4096                    127.0.0.53%lo:53  0.0.0.0:*
tcp LISTEN 0      4096                          0.0.0.0:22  0.0.0.0:*
tcp LISTEN 0      4096                       127.0.0.54:53  0.0.0.0:*
tcp LISTEN 0      4096                             [::]:22     [::]:*
";
        let p = parse(out);
        assert_eq!((p.sockets.len(), p.skipped, p.truncated), (10, 0, false));
    }

    #[test]
    fn udp_wildcard_ipv4() {
        let s = one("udp UNCONN 0 0 0.0.0.0:68 0.0.0.0:*");
        assert_eq!(
            (s.protocol, s.family, s.binding, s.port),
            (Protocol::Udp, Family::Ipv4, BindingClass::Wildcard, 68)
        );
    }

    #[test]
    fn loopback_private_and_public_addresses() {
        assert_eq!(
            one("tcp LISTEN 0 1 [::1]:3306 [::]:*").binding,
            BindingClass::Loopback
        );
        let lo = one("tcp LISTEN 0 1 127.0.0.1:3306 0.0.0.0:*");
        assert_eq!((lo.binding, lo.private), (BindingClass::Loopback, false));
        for (addr, private) in [
            ("10.0.0.5", true),
            ("172.16.1.1", true),
            ("172.32.0.1", false),
            ("192.168.1.2", true),
            ("169.254.1.1", true),
            ("203.0.113.9", false),
        ] {
            let s = one(&format!("tcp LISTEN 0 1 {addr}:3306 0.0.0.0:*"));
            assert_eq!(
                (s.binding, s.private),
                (BindingClass::Specific, private),
                "{addr}"
            );
        }
        assert!(one("tcp LISTEN 0 1 [fd12:3456::1]:5432 [::]:*").private);
        assert!(!one("tcp LISTEN 0 1 [2001:db8::1]:5432 [::]:*").private);
        let mapped = one("tcp LISTEN 0 1 [::ffff:127.0.0.1]:6379 [::]:*");
        assert_eq!(mapped.binding, BindingClass::Loopback);
    }

    #[test]
    fn process_name_with_spaces_and_bad_process_field() {
        let s = one("tcp LISTEN 0 1 0.0.0.0:22 0.0.0.0:* users:((\"tmux: server\",pid=7,fd=3))");
        assert_eq!(s.process.unwrap().name, "tmux: server");
        for tail in [
            "users:((\"x\",pid=,fd=3))",
            "users:((\"\",pid=5,fd=3))",
            "users:((",
        ] {
            let s = one(&format!("tcp LISTEN 0 1 0.0.0.0:22 0.0.0.0:* {tail}"));
            assert_eq!(s.process, None, "{tail}");
        }
    }

    #[test]
    fn malformed_lines_are_skipped() {
        for bad in [
            "",
            "garbage",
            "tcp LISTEN 0 128",
            "tcp LISTEN 0 128 noport 0.0.0.0:*",
            "tcp LISTEN 0 128 1.2.3.4:99999 0.0.0.0:*",
            "tcp LISTEN 0 128 not-an-ip:22 0.0.0.0:*",
            "raw UNCONN 0 0 *:1 *:*",
        ] {
            assert!(parse_line(bad).is_none(), "{bad}");
        }
        assert!(
            parse_line(&format!(
                "tcp LISTEN 0 1 1.2.3.4:22 {}",
                "x".repeat(MAX_LINE)
            ))
            .is_none()
        );
        let p = parse("tcp LISTEN 0 1 1.2.3.4:22 0.0.0.0:*\nbad line\n\n");
        assert_eq!((p.sockets.len(), p.skipped, p.truncated), (1, 1, false));
    }

    #[test]
    fn list_is_capped_and_truncation_is_recorded() {
        let out: String = (0..MAX_SOCKETS + 2)
            .map(|i| format!("tcp LISTEN 0 1 0.0.0.0:{} 0.0.0.0:*\n", i + 1))
            .collect();
        let p = parse(&out);
        assert_eq!(p.sockets.len(), MAX_SOCKETS);
        assert!(p.truncated);
        let exact: String = (0..MAX_SOCKETS)
            .map(|i| format!("tcp LISTEN 0 1 0.0.0.0:{} 0.0.0.0:*\n", i + 1))
            .collect();
        assert!(!parse(&exact).truncated);
    }

    struct Fake(&'static str, i32);

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            assert_eq!(spec.program(), "ss");
            assert_eq!(spec.args(), ["-H", "-tulnp"]);
            Ok(ExecResult {
                exit: ExitStatus::Code(self.1),
                stdout: self.0.into(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn collect_lists_sockets_and_notes_invisible_processes() {
        let cancel = CancelToken::new();
        let out = "tcp LISTEN 0 1 0.0.0.0:22 0.0.0.0:* users:((\"sshd\",pid=1,fd=3))\n\
                   tcp LISTEN 0 1 127.0.0.1:3306 0.0.0.0:*\n";
        let info = collect(&Fake(out, 0), &cancel, "t".into()).await.unwrap();
        assert!(info.available && !info.truncated && info.sockets.len() == 2);
        assert_eq!(info.unrecognized_lines, 0);
        assert!(info.sockets[1].process.is_none());
        assert_eq!(info.limitations.len(), 1);

        let failed = collect(&Fake("", 1), &cancel, "t".into()).await.unwrap();
        assert!(!failed.available && failed.sockets.is_empty());
        assert_eq!(failed.probe_errors[0].probe, "listeningSockets");
    }
}
