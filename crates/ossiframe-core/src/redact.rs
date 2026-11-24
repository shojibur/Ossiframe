//! Bounded output capture and whole-output redaction.
//!
//! Redaction is defense in depth. It replaces exact occurrences of registered
//! secrets only. The primary protection is that secrets never go into
//! command arguments, logs, SQLite or IPC responses.

/// Maximum bytes kept per output stream.
pub const OUTPUT_CAP: usize = 256 * 1024;

pub const REDACTED: &str = "[REDACTED]";

/// Collects bytes up to a fixed limit and remembers whether more arrived.
#[derive(Debug)]
pub struct BoundedBuffer {
    buf: Vec<u8>,
    cap: usize,
    overflowed: bool,
}

impl BoundedBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            buf: Vec::new(),
            cap,
            overflowed: false,
        }
    }

    pub fn push(&mut self, data: &[u8]) {
        let room = self.cap.saturating_sub(self.buf.len());
        if data.len() > room {
            self.overflowed = true;
        }
        self.buf.extend_from_slice(&data[..data.len().min(room)]);
    }

    pub fn finalize(self, secrets: &[&str]) -> (String, bool) {
        finalize_output(&self.buf, self.overflowed, secrets, self.cap)
    }
}

/// Converts captured bytes to redacted, bounded text.
///
/// If `raw_truncated` is set, the capture was cut while reading. Trailing
/// bytes that could be the start of a registered secret are dropped before
/// redaction, so a secret split by the cut cannot leak partially. Length
/// truncation happens after redaction, on a character boundary.
pub fn finalize_output(
    raw: &[u8],
    raw_truncated: bool,
    secrets: &[&str],
    cap: usize,
) -> (String, bool) {
    let mut secrets: Vec<&str> = secrets.iter().copied().filter(|s| !s.is_empty()).collect();
    // Longest first, so a secret that contains another is replaced whole.
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));

    let mut bytes = raw;
    if raw_truncated {
        bytes = &bytes[..bytes.len() - partial_secret_suffix(bytes, &secrets)];
    }

    let mut text = String::from_utf8_lossy(bytes).into_owned();
    for s in &secrets {
        if text.contains(s) {
            text = text.replace(s, REDACTED);
        }
    }

    let mut truncated = raw_truncated;
    if text.len() > cap {
        let mut end = cap;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        truncated = true;
    }
    (text, truncated)
}

/// Length of the longest suffix of `bytes` that is a proper prefix of a secret.
fn partial_secret_suffix(bytes: &[u8], secrets: &[&str]) -> usize {
    let mut longest = 0;
    for s in secrets {
        let s = s.as_bytes();
        let max = (s.len() - 1).min(bytes.len());
        for k in (longest + 1..=max).rev() {
            if bytes[bytes.len() - k..] == s[..k] {
                longest = k;
                break;
            }
        }
    }
    longest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_all_registered_secrets() {
        let (t, tr) = finalize_output(
            b"user=a pass=s3cr3t again s3cr3t tok=XYZ",
            false,
            &["s3cr3t", "XYZ", ""],
            1024,
        );
        assert_eq!(t, "user=a pass=[REDACTED] again [REDACTED] tok=[REDACTED]");
        assert!(!tr);
    }

    #[test]
    fn longer_secret_wins_over_contained_one() {
        let (t, _) = finalize_output(b"abcdef", false, &["cd", "abcdef"], 1024);
        assert_eq!(t, "[REDACTED]");
    }

    #[test]
    fn secret_cut_by_the_raw_cap_does_not_leak() {
        let mut b = BoundedBuffer::new(10);
        b.push(b"data: topsecret!");
        let (t, tr) = b.finalize(&["topsecret"]);
        assert!(tr);
        assert_eq!(t, "data: ");
        assert!(!t.contains("top"));
    }

    #[test]
    fn length_truncation_happens_after_redaction_on_char_boundary() {
        let raw = "ééé secret".as_bytes();
        let (t, tr) = finalize_output(raw, false, &["secret"], 5);
        assert!(tr);
        assert_eq!(t, "éé");
        let (t, _) = finalize_output(b"xxsecret", false, &["secret"], 6);
        assert_eq!(t, "xx[RED");
    }

    #[test]
    fn invalid_utf8_is_lossy() {
        let (t, _) = finalize_output(&[0x66, 0xff, 0x6f], false, &[], 1024);
        assert_eq!(t, "f\u{fffd}o");
    }
}
