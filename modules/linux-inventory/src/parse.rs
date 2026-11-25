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

/// Most filesystems kept from `df`.
pub const MAX_FILESYSTEMS: usize = 50;

fn is_snap_mount(mount: &str) -> bool {
    mount == "/snap"
        || mount.starts_with("/snap/")
        || mount == "/var/lib/snapd/snap"
        || mount.starts_with("/var/lib/snapd/snap/")
}

/// Parses `df -P -B1` output into meaningful filesystems. Zero-size entries
/// and snap mounts are dropped, and unparsable lines are skipped. Returns at
/// most [`MAX_FILESYSTEMS`] entries and whether more were dropped. `None` when
/// the header is missing.
pub fn df_all(s: &str) -> Option<(Vec<DiskInfo>, bool)> {
    let mut lines = bounded(s).lines();
    if !lines.next()?.starts_with("Filesystem") {
        return None;
    }
    let mut out = Vec::new();
    let mut truncated = false;
    for line in lines {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // Find "<total> <used> <avail> <NN%>"; the mount point is the rest
        // and may contain spaces.
        let Some(pct) = (3..cols.len()).find(|&i| {
            cols[i].ends_with('%') && cols[i - 3..i].iter().all(|c| c.parse::<u64>().is_ok())
        }) else {
            continue;
        };
        let (Ok(total), Ok(used), Ok(avail)) = (
            cols[pct - 3].parse::<u64>(),
            cols[pct - 2].parse::<u64>(),
            cols[pct - 1].parse::<u64>(),
        ) else {
            continue;
        };
        let Some(mount) = field(&cols[pct + 1..].join(" ")) else {
            continue;
        };
        if total == 0 || is_snap_mount(&mount) {
            continue;
        }
        if out.len() >= MAX_FILESYSTEMS {
            truncated = true;
            break;
        }
        out.push(DiskInfo {
            mount,
            total_bytes: total,
            used_bytes: used,
            available_bytes: avail,
        });
    }
    Some((out, truncated))
}

/// Returns (SwapTotal, SwapFree) in kB. `None` if neither is present.
pub fn swap(s: &str) -> Option<(Option<u64>, Option<u64>)> {
    let mut total = None;
    let mut free = None;
    for line in bounded(s).lines() {
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let num = v
            .split_whitespace()
            .next()
            .and_then(|n| n.parse::<u64>().ok());
        match k.trim() {
            "SwapTotal" => total = num,
            "SwapFree" => free = num,
            _ => {}
        }
    }
    if total.is_none() && free.is_none() {
        None
    } else {
        Some((total, free))
    }
}

/// Aggregate CPU counters from the first `cpu` line of `/proc/stat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuSample {
    pub total: u64,
    pub idle: u64,
}

/// Parses the aggregate `cpu ` line. `total` is user..steal (guest time is
/// already part of user/nice); `idle` is idle + iowait.
pub fn proc_stat_cpu(s: &str) -> Option<CpuSample> {
    let line = bounded(s).lines().find(|l| l.starts_with("cpu "))?;
    let nums: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|n| n.parse().ok())
        .collect::<Option<_>>()?;
    if nums.len() < 4 {
        return None;
    }
    let total = nums.iter().try_fold(0u64, |a, n| a.checked_add(*n))?;
    let idle = nums[3].checked_add(nums.get(4).copied().unwrap_or(0))?;
    Some(CpuSample { total, idle })
}

/// CPU busy percent (0 to 100, one decimal) between two samples. `None` when
/// the counters did not advance or went backwards.
pub fn cpu_usage(a: CpuSample, b: CpuSample) -> Option<f64> {
    let dt = b.total.checked_sub(a.total).filter(|d| *d > 0)?;
    let di = b.idle.checked_sub(a.idle)?;
    let busy = dt.checked_sub(di)?;
    Some((busy as f64 / dt as f64 * 1000.0).round() / 10.0)
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

    #[test]
    fn df_all_filters_and_bounds() {
        let df = "Filesystem 1-blocks Used Available Capacity Mounted on\n\
/dev/vda1 100 40 60 40% /\n\
/dev/loop0 128 128 0 100% /snap/core/1\n\
none 0 0 0 - /sys/fs/x\n\
/dev/vdb 50 10 40 20% /mnt/my disk\n\
bad line here\n";
        let (v, t) = df_all(df).unwrap();
        assert!(!t);
        let mounts: Vec<_> = v.iter().map(|d| d.mount.as_str()).collect();
        assert_eq!(mounts, ["/", "/mnt/my disk"]);
        let mut many = String::from("Filesystem 1-blocks Used Available Capacity Mounted on\n");
        for i in 0..60 {
            many.push_str(&format!("/dev/x{i} 10 1 9 10% /m{i}\n"));
        }
        let (v, t) = df_all(&many).unwrap();
        assert_eq!(v.len(), MAX_FILESYSTEMS);
        assert!(t);
        assert!(df_all("").is_none() && df_all("garbage\n1 2 3 4% /").is_none());
    }

    #[test]
    fn swap_parsing() {
        assert_eq!(
            swap("MemTotal: 1 kB\nSwapTotal: 2097148 kB\nSwapFree: 2000000 kB\n"),
            Some((Some(2097148), Some(2000000)))
        );
        assert_eq!(
            swap("SwapTotal: 0 kB\nSwapFree: 0 kB"),
            Some((Some(0), Some(0)))
        );
        assert!(swap("MemTotal: 1 kB\nSwapTotal: x kB").is_none());
        assert!(swap("").is_none() && swap("::::").is_none());
    }

    #[test]
    fn cpu_delta() {
        let a = proc_stat_cpu("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(
            a,
            CpuSample {
                total: 1000,
                idle: 800
            }
        );
        let b = proc_stat_cpu("cpu  150 0 150 750 150 0 0 0 0 0\n").unwrap();
        // delta total 200, idle 100 -> 50% busy
        assert_eq!(cpu_usage(a, b), Some(50.0));
        assert_eq!(cpu_usage(a, a), None, "zero delta");
        assert_eq!(cpu_usage(b, a), None, "counters went backwards");
        assert!(proc_stat_cpu("").is_none());
        assert!(proc_stat_cpu("cpu0 1 2 3 4\n").is_none());
        assert!(proc_stat_cpu("cpu  1 2 x 4\n").is_none());
        assert!(proc_stat_cpu("cpu  1 2 3\n").is_none());
        assert!(proc_stat_cpu("cpu  18446744073709551615 1 0 0\n").is_none());
    }
}
