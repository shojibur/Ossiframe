//! Pure, bounded parsers for probe output. They return `None` on input they
//! do not understand and never panic.

use super::{DiskInfo, OsInfo};

/// Inputs larger than this are cut before parsing.
const MAX_INPUT: usize = 64 * 1024;
const MAX_FIELD: usize = 256;

fn bounded(s: &str) -> &str {
    if s.len() <= MAX_INPUT {
        return s;
    }
    let mut end = MAX_INPUT;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn field(s: &str) -> Option<String> {
    let v: String = s
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_FIELD)
        .collect();
    if v.is_empty() { None } else { Some(v) }
}

pub fn first_line(s: &str) -> Option<String> {
    field(bounded(s).lines().next()?)
}

pub fn os_release(s: &str) -> Option<OsInfo> {
    let mut os = OsInfo::default();
    for line in bounded(s).lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k.trim() {
            "ID" => os.id = field(v),
            "VERSION_ID" => os.version_id = field(v),
            "PRETTY_NAME" => os.pretty_name = field(v),
            _ => {}
        }
    }
    if os == OsInfo::default() {
        None
    } else {
        Some(os)
    }
}

pub fn uptime(s: &str) -> Option<u64> {
    let first = bounded(s).split_whitespace().next()?;
    let secs: f64 = first.parse().ok()?;
    if secs.is_finite() && secs >= 0.0 && secs < u64::MAX as f64 {
        Some(secs as u64)
    } else {
        None
    }
}

pub fn cpu_count(s: &str) -> Option<u32> {
    first_line(s)?.parse().ok().filter(|n| *n > 0)
}

pub fn lscpu_model(s: &str) -> Option<String> {
    bounded(s).lines().find_map(|l| {
        l.split_once(':')
            .filter(|(k, _)| k.trim() == "Model name")
            .and_then(|(_, v)| field(v))
    })
}

/// Returns (MemTotal, MemAvailable) in kB. `None` if neither is present.
pub fn meminfo(s: &str) -> Option<(Option<u64>, Option<u64>)> {
    let mut total = None;
    let mut avail = None;
    for line in bounded(s).lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let num = v
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok());
        match k.trim() {
            "MemTotal" => total = num,
            "MemAvailable" => avail = num,
            _ => {}
        }
    }
    if total.is_none() && avail.is_none() {
        None
    } else {
        Some((total, avail))
    }
}

/// Parses `df -P -B1 /`: a header line, then one data line.
pub fn df_root(s: &str) -> Option<DiskInfo> {
    let line = bounded(s).lines().nth(1)?;
    let cols: Vec<&str> = line.split_whitespace().collect();
    if cols.len() < 6 {
        return None;
    }
    let n = cols.len();
    Some(DiskInfo {
        total_bytes: cols[n - 5].parse().ok()?,
        used_bytes: cols[n - 4].parse().ok()?,
        available_bytes: cols[n - 3].parse().ok()?,
        mount: field(cols[n - 1])?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples() {
        let os = os_release("NAME=\"Ubuntu\"\nVERSION_ID=\"24.04\"\nID=ubuntu\nPRETTY_NAME=\"Ubuntu 24.04.1 LTS\"\n").unwrap();
        assert_eq!(os.id.as_deref(), Some("ubuntu"));
        assert_eq!(os.pretty_name.as_deref(), Some("Ubuntu 24.04.1 LTS"));
        assert_eq!(uptime("350735.47 234388.90"), Some(350735));
        assert_eq!(cpu_count("4\n"), Some(4));
        assert_eq!(
            lscpu_model("Architecture: x86_64\nModel name:   AMD EPYC 7763 64-Core Processor\n")
                .as_deref(),
            Some("AMD EPYC 7763 64-Core Processor")
        );
        assert_eq!(
            meminfo("MemTotal:        4005060 kB\nMemAvailable:    3201240 kB\n"),
            Some((Some(4005060), Some(3201240)))
        );
        let d = df_root(
            "Filesystem 1-blocks Used Available Capacity Mounted on\n/dev/sda1 100 40 60 40% /\n",
        )
        .unwrap();
        assert_eq!(
            (
                d.total_bytes,
                d.used_bytes,
                d.available_bytes,
                d.mount.as_str()
            ),
            (100, 40, 60, "/")
        );
    }

    #[test]
    fn garbage_and_oversized_input() {
        let big = "x".repeat(10 * 1024 * 1024);
        for s in ["", "\n\n", "=====", "::::", "\u{0}\u{1b}[31m", big.as_str()] {
            assert!(os_release(s).is_none());
            assert!(uptime(s).is_none());
            assert!(cpu_count(s).is_none());
            assert!(lscpu_model(s).is_none());
            assert!(meminfo(s).is_none());
            assert!(df_root(s).is_none());
        }
        assert_eq!(uptime("-5 1"), None);
        assert_eq!(uptime("NaN 1"), None);
        assert_eq!(cpu_count("0"), None);
        assert_eq!(
            first_line("\u{1b}[31mred\u{7}\n").as_deref(),
            Some("[31mred")
        );
        assert!(first_line(&"é".repeat(100_000)).unwrap().chars().count() <= MAX_FIELD);
    }
}
