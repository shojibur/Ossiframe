//! Bounded parser for `ss -Htln` output. Never panics on arbitrary input.

/// Input is cut at this many bytes.
pub const MAX_INPUT_BYTES: usize = 64 * 1024;
/// At most this many sockets are returned.
pub const MAX_ENTRIES: usize = 512;
/// Longer addresses make the line unparsable.
pub const MAX_ADDRESS_CHARS: usize = 128;

/// Address family of a parsed local address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    V4,
    V6,
    /// The `*` wildcard, which `ss` prints for dual-stack listeners.
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listening {
    /// Local address as printed by `ss`, without the port.
    pub address: String,
    pub port: u16,
    pub family: Family,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub sockets: Vec<Listening>,
    /// Non-empty lines that could not be parsed.
    pub skipped: usize,
    pub truncated: bool,
}

pub fn parse_ss(input: &str) -> Parsed {
    let mut out = Parsed::default();
    let mut text = input;
    if text.len() > MAX_INPUT_BYTES {
        let mut end = MAX_INPUT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text = &text[..end];
        // Drop a possibly partial last line.
        text = text.rfind('\n').map_or("", |i| &text[..i]);
        out.truncated = true;
    }
    for line in text.lines() {
        let clean: String = line.chars().filter(|c| !c.is_control()).collect();
        if clean.trim().is_empty() {
            continue;
        }
        match parse_line(&clean) {
            Some(s) => {
                if out.sockets.len() >= MAX_ENTRIES {
                    out.truncated = true;
                    break;
                }
                out.sockets.push(s);
            }
            None => out.skipped += 1,
        }
    }
    out
}

/// Columns: State Recv-Q Send-Q Local-Address:Port Peer-Address:Port.
fn parse_line(line: &str) -> Option<Listening> {
    let mut cols = line.split_whitespace();
    let state = cols.next()?;
    if state != "LISTEN" {
        return None;
    }
    let local = cols.nth(2)?;
    let (address, port) = local.rsplit_once(':')?;
    if address.is_empty() || address.chars().count() > MAX_ADDRESS_CHARS {
        return None;
    }
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = port.parse().ok()?;
    let family = if address == "*" {
        Family::Any
    } else if address.starts_with('[') || address.contains(':') {
        Family::V6
    } else {
        Family::V4
    };
    Some(Listening {
        address: address.to_owned(),
        port,
        family,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_real_forms() {
        let input = "\
LISTEN 0      4096   127.0.0.53%lo:53        0.0.0.0:*
LISTEN 0      128          0.0.0.0:22        0.0.0.0:*
LISTEN 0      128             [::]:22           [::]:*
LISTEN 0      511                *:80              *:*
LISTEN 0      4096           [::1]:631          [::]:*
";
        let p = parse_ss(input);
        assert_eq!(p.skipped, 0);
        assert!(!p.truncated);
        let got: Vec<_> = p
            .sockets
            .iter()
            .map(|s| (s.address.as_str(), s.port, s.family))
            .collect();
        assert_eq!(
            got,
            [
                ("127.0.0.53%lo", 53, Family::V4),
                ("0.0.0.0", 22, Family::V4),
                ("[::]", 22, Family::V6),
                ("*", 80, Family::Any),
                ("[::1]", 631, Family::V6),
            ]
        );
    }

    #[test]
    fn garbage_is_skipped_without_panic() {
        let input = "\nnonsense\nLISTEN 0 1\nLISTEN 0 1 host:99999 x\nLISTEN 0 1 :80 x\nLISTEN 0 1 a:b x\nESTAB 0 0 1.2.3.4:22 5.6.7.8:1\nLISTEN\u{7} 0 1 1.1.1.1:80 x\n";
        let p = parse_ss(input);
        assert_eq!(p.sockets.len(), 1, "control chars are stripped");
        assert_eq!(p.sockets[0].port, 80);
        assert_eq!(p.skipped, 6);
        let long = format!("LISTEN 0 1 {}:80 x", "a".repeat(MAX_ADDRESS_CHARS + 1));
        assert_eq!(parse_ss(&long).skipped, 1);
    }

    #[test]
    fn oversized_input_and_entry_count_are_bounded() {
        let line = "LISTEN 0 128 0.0.0.0:22 0.0.0.0:*\n";
        let big = line.repeat(MAX_INPUT_BYTES / line.len() + 100);
        let p = parse_ss(&big);
        assert!(p.truncated);
        assert!(p.sockets.len() <= MAX_ENTRIES);
        assert_eq!(p.skipped, 0, "partial last line is dropped, not counted");

        let many = "LISTEN 0 1 1.1.1.1:1 x\n".repeat(MAX_ENTRIES + 5);
        let p = parse_ss(&many);
        assert_eq!(p.sockets.len(), MAX_ENTRIES);
        assert!(p.truncated);

        let multibyte = "é".repeat(MAX_INPUT_BYTES);
        let _ = parse_ss(&multibyte);
    }
}
