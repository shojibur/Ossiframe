//! APT source parsing for the preflight source check. Handles the one-line
//! format (`*.list`) and deb822 (`*.sources`). Pure functions over text; the
//! preflight reads the files. Sources are never modified.
//!
//! Only hosts on an explicit allowlist pass. Anything else (a PPA, a third
//! party or provider mirror, a non-HTTP scheme, an option that disables
//! signature checks) is reported as rejected.

/// Hosts accepted as-is. Provider mirrors are added here only after
/// verification on a real server.
const ALLOWED_HOSTS: [&str; 3] = [
    "archive.ubuntu.com",
    "security.ubuntu.com",
    "ports.ubuntu.com",
];
/// Any non-empty subdomain label sequence below this (for example cloud
/// mirrors such as `eu-west-1.ec2.archive.ubuntu.com`).
const ALLOWED_SUFFIX: &str = ".archive.ubuntu.com";

const MAX_NOTE: usize = 100;

/// Enabled source hosts and the entries that must block provisioning.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Parsed {
    /// Host of every enabled source with a valid HTTP(S) URI.
    pub hosts: Vec<String>,
    /// Plain-text descriptions of entries that are not acceptable.
    pub rejected: Vec<String>,
}

impl Parsed {
    fn add_uri(&mut self, uri: &str) {
        match uri_host(uri) {
            Ok(host) => {
                if !host_allowed(&host) {
                    self.rejected
                        .push(format!("host not on the allowlist: {}", clean(&host)));
                }
                self.hosts.push(host);
            }
            Err(reason) => self.rejected.push(format!("{reason}: {}", clean(uri))),
        }
    }
}

/// True for the Ubuntu archive hosts and their subdomains.
pub fn host_allowed(host: &str) -> bool {
    if ALLOWED_HOSTS.contains(&host) {
        return true;
    }
    host.strip_suffix(ALLOWED_SUFFIX).is_some_and(|prefix| {
        !prefix.is_empty()
            && !prefix.starts_with('.')
            && !prefix.ends_with('.')
            && !prefix.contains("..")
            && prefix
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
    })
}

/// Control characters removed and length bounded, for messages.
pub fn clean(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_NOTE)
        .collect()
}

/// Lower-case host of an `http` or `https` URI. User info and port are
/// stripped, so `http://archive.ubuntu.com@evil.example/` yields
/// `evil.example`. `Err` names the problem.
pub(crate) fn uri_host(uri: &str) -> Result<String, &'static str> {
    let (scheme, rest) = uri.split_once("://").ok_or("not an http(s) URI")?;
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return Err("scheme is not http or https");
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host_port = authority.rsplit('@').next().unwrap_or("");
    if host_port.starts_with('[') {
        return Err("IP literal host");
    }
    let host = match host_port.split_once(':') {
        Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return Err("invalid port"),
        None => host_port,
    };
    if host.is_empty() {
        return Err("empty host");
    }
    Ok(host.to_ascii_lowercase())
}

fn insecure_option(options: &str) -> bool {
    let o: String = options
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    ["trusted=yes", "allow-insecure=yes", "allow-weak=yes"]
        .iter()
        .any(|bad| o.contains(bad))
}

/// Parses a one-line-format file. `Err` when a line is not valid.
pub fn parse_list(text: &str) -> Result<Parsed, String> {
    let mut out = Parsed::default();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        match words.next() {
            Some("deb" | "deb-src") => {}
            _ => return Err(format!("unrecognized source line: {}", clean(line))),
        }
        let mut next = words.next();
        if let Some(first) = next.filter(|w| w.starts_with('[')) {
            let mut options = String::from(first);
            let mut closed = first.ends_with(']');
            while !closed {
                let Some(w) = words.next() else {
                    return Err(format!("unterminated options: {}", clean(line)));
                };
                options.push(' ');
                options.push_str(w);
                closed = w.ends_with(']');
            }
            if insecure_option(&options) {
                out.rejected
                    .push(format!("signature checks disabled: {}", clean(line)));
            }
            next = words.next();
        }
        let (Some(uri), Some(_suite)) = (next, words.next()) else {
            return Err(format!("incomplete source line: {}", clean(line)));
        };
        out.add_uri(uri);
    }
    Ok(out)
}

fn truthy(v: &str) -> bool {
    matches!(v.to_ascii_lowercase().as_str(), "yes" | "true" | "1" | "on")
}

fn falsy(v: &str) -> bool {
    matches!(
        v.to_ascii_lowercase().as_str(),
        "no" | "false" | "0" | "off"
    )
}

/// Parses a deb822 file. A stanza with `Enabled: no` is skipped.
pub fn parse_deb822(text: &str) -> Result<Parsed, String> {
    let mut out = Parsed::default();
    let mut stanza: Vec<(String, String)> = Vec::new();
    for raw in text.lines().chain(std::iter::once("")) {
        if raw.trim().is_empty() {
            apply_stanza(&mut out, std::mem::take(&mut stanza))?;
        } else if raw.trim_start().starts_with('#') {
            continue;
        } else if raw.starts_with([' ', '\t']) {
            let Some((_, value)) = stanza.last_mut() else {
                return Err(format!("unexpected continuation line: {}", clean(raw)));
            };
            value.push(' ');
            value.push_str(raw.trim());
        } else {
            let Some((key, value)) = raw.split_once(':') else {
                return Err(format!("unrecognized source line: {}", clean(raw)));
            };
            stanza.push((key.trim().to_ascii_lowercase(), value.trim().to_owned()));
        }
    }
    Ok(out)
}

fn apply_stanza(out: &mut Parsed, stanza: Vec<(String, String)>) -> Result<(), String> {
    if stanza.is_empty() {
        return Ok(());
    }
    let get = |k: &str| stanza.iter().find(|(key, _)| key == k).map(|(_, v)| v);
    if get("enabled").is_some_and(|v| falsy(v)) {
        return Ok(());
    }
    let (Some(_types), Some(uris)) = (get("types"), get("uris")) else {
        return Err("source stanza without Types or URIs".to_owned());
    };
    if uris.split_whitespace().next().is_none() {
        return Err("source stanza with empty URIs".to_owned());
    }
    for field in ["trusted", "allow-insecure", "allow-weak"] {
        if get(field).is_some_and(|v| truthy(v)) {
            out.rejected
                .push(format!("signature checks disabled ({field})"));
        }
    }
    for uri in uris.split_whitespace() {
        out.add_uri(uri);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_accepts_ubuntu_archive_hosts_only() {
        for ok in [
            "archive.ubuntu.com",
            "security.ubuntu.com",
            "ports.ubuntu.com",
            "us.archive.ubuntu.com",
            "eu-west-1.ec2.archive.ubuntu.com",
        ] {
            assert!(host_allowed(ok), "{ok}");
        }
        for bad in [
            "ppa.launchpadcontent.net",
            "mirrors.example.com",
            "archive.ubuntu.com.evil.example",
            "evilarchive.ubuntu.com",
            ".archive.ubuntu.com",
            "..archive.ubuntu.com",
            "a..archive.ubuntu.com",
            "archive.ubuntu.com.",
            "ARCHIVE.ubuntu.com",
            "",
        ] {
            assert!(!host_allowed(bad), "{bad}");
        }
    }

    #[test]
    fn one_line_format_accepts_defaults_and_rejects_unknown_hosts() {
        let ok = "# comment\n\
                  deb http://archive.ubuntu.com/ubuntu noble main universe\n\
                  deb [arch=amd64 signed-by=/k.gpg] https://security.ubuntu.com/ubuntu noble-security main # c\n";
        let p = parse_list(ok).unwrap();
        assert_eq!(p.hosts, ["archive.ubuntu.com", "security.ubuntu.com"]);
        assert!(p.rejected.is_empty());

        let bad = "deb http://ppa.launchpadcontent.net/x/y/ubuntu noble main\n\
                   deb-src http://mirror.example.com:8080/ubuntu noble main\n";
        let p = parse_list(bad).unwrap();
        assert_eq!(p.rejected.len(), 2);
        assert!(p.rejected[0].contains("ppa.launchpadcontent.net"));
        assert!(p.rejected[1].contains("mirror.example.com"));
    }

    #[test]
    fn hostile_uris_are_rejected_or_resolved_to_the_real_host() {
        let p = parse_list("deb http://archive.ubuntu.com@evil.example/ubuntu noble main").unwrap();
        assert_eq!(p.hosts, ["evil.example"]);
        assert_eq!(p.rejected.len(), 1);
        for uri in [
            "cdrom:[Ubuntu 24.04]/",
            "file:/srv/repo",
            "ftp://archive.ubuntu.com/ubuntu",
            "mirror+http://archive.ubuntu.com/mirrors.txt",
            "http://[::1]/ubuntu",
            "http://archive.ubuntu.com:abc/ubuntu",
            "http:///ubuntu",
        ] {
            let p = parse_list(&format!("deb {uri} noble main")).unwrap();
            assert_eq!(p.rejected.len(), 1, "{uri}");
        }
        let p =
            parse_list("deb [trusted=yes] http://archive.ubuntu.com/ubuntu noble main").unwrap();
        assert_eq!(p.rejected.len(), 1);
        let p =
            parse_list("deb [ allow-insecure=yes ] http://archive.ubuntu.com/ubuntu noble main")
                .unwrap();
        assert_eq!(p.rejected.len(), 1);
    }

    #[test]
    fn one_line_format_rejects_unparsable_lines() {
        assert!(parse_list("garbage here").is_err());
        assert!(parse_list("deb http://archive.ubuntu.com/ubuntu").is_err());
        assert!(parse_list("deb [arch=amd64").is_err());
        assert!(parse_list("").unwrap().hosts.is_empty());
    }

    const UBUNTU_SOURCES: &str = "Types: deb\n\
        URIs: http://archive.ubuntu.com/ubuntu/\n\
        Suites: noble noble-updates noble-backports\n\
        Components: main universe restricted multiverse\n\
        Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n\
        \n\
        Types: deb\n\
        URIs: http://security.ubuntu.com/ubuntu/\n\
        Suites: noble-security\n\
        Components: main universe restricted multiverse\n\
        Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n";

    #[test]
    fn deb822_default_ubuntu_file_is_accepted() {
        let p = parse_deb822(UBUNTU_SOURCES).unwrap();
        assert_eq!(p.hosts, ["archive.ubuntu.com", "security.ubuntu.com"]);
        assert!(p.rejected.is_empty());
    }

    #[test]
    fn deb822_rejects_unknown_hosts_in_any_uri_and_skips_disabled_stanzas() {
        let text = "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\n http://evil.example/x\n\
                    Suites: noble\n\nTypes: deb\nURIs: http://other.example/\nSuites: n\nEnabled: no\n";
        let p = parse_deb822(text).unwrap();
        assert_eq!(p.rejected.len(), 1);
        assert!(p.rejected[0].contains("evil.example"));
        assert!(!p.hosts.contains(&"other.example".to_owned()));
    }

    #[test]
    fn deb822_rejects_disabled_signature_checks_and_malformed_stanzas() {
        let p = parse_deb822(
            "Types: deb\nURIs: http://archive.ubuntu.com/ubuntu\nSuites: noble\nTrusted: yes\n",
        )
        .unwrap();
        assert_eq!(p.rejected.len(), 1);
        assert!(parse_deb822("Types: deb\nSuites: noble\n").is_err());
        assert!(parse_deb822("not a field\n").is_err());
        assert!(parse_deb822(" continuation first\n").is_err());
        assert!(parse_deb822("# only a comment\n").unwrap().hosts.is_empty());
    }
}
