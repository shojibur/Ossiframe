//! The `services` operation: systemd service units, merged from the loaded
//! list (`list-units`) and the installed list (`list-unit-files`).
//!
//! ASSUMPTION (confirm in live verification): `--output=json` is supported by
//! `systemctl list-units` and `list-unit-files` on systemd 255 (Ubuntu 24.04),
//! with keys `unit`, `load`, `active`, `sub`, `description` and `unit_file`,
//! `state`. There is no plain-text fallback; unparsable output makes the
//! result `unavailable`.

use crate::inventory::{Probe, ProbeError, probe_strict};

use ossiframe_core::{CancelToken, ExecError, Executor};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Most services returned. Truncation is recorded in `truncated`.
pub const MAX_SERVICES: usize = 500;
/// Most rows read from either list, to bound memory.
const MAX_ROWS: usize = 5000;
/// Larger input is rejected, never cut (cut JSON does not parse).
const MAX_JSON: usize = 512 * 1024;
const MAX_TEXT: usize = 256;
const UNKNOWN: &str = "unknown";

const LIST_UNITS: [&str; 5] = [
    "list-units",
    "--type=service",
    "--all",
    "--no-pager",
    "--output=json",
];
const LIST_UNIT_FILES: [&str; 4] = [
    "list-unit-files",
    "--type=service",
    "--no-pager",
    "--output=json",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub unit: String,
    pub description: Option<String>,
    /// Present in the installed unit-file list.
    pub installed: bool,
    /// systemd load state, or `unknown` when the unit is not loaded.
    pub load: String,
    pub active: String,
    pub sub: String,
    /// systemd unit-file state (enabled, disabled, static, ...), or `unknown`.
    pub enablement: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServicesInfo {
    pub observed_at: String,
    /// False when the lists could not be read or parsed; `services` is empty.
    pub available: bool,
    pub services: Vec<Service>,
    /// True when more than [`MAX_SERVICES`] units exist; rules must treat the
    /// list as incomplete.
    pub truncated: bool,
    pub limitations: Vec<String>,
    pub probe_errors: Vec<ProbeError>,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Row {
    unit: String,
    description: Option<String>,
    load: String,
    active: String,
    sub: String,
    state: String,
}

/// Parsed rows and whether the row limit cut the list.
type Rows = (Vec<Row>, bool);

fn text(v: &Value, key: &str) -> Option<String> {
    let s: String = v
        .get(key)?
        .as_str()?
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT)
        .collect();
    if s.is_empty() { None } else { Some(s) }
}

/// A systemd state word, or `unknown` when absent or not a plain word.
fn state(v: &Value, key: &str) -> String {
    match text(v, key) {
        Some(s)
            if s.len() <= 32
                && s.chars().all(|c| {
                    c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_'
                }) =>
        {
            s
        }
        _ => UNKNOWN.to_owned(),
    }
}

fn parse_rows(s: &str, name_key: &str) -> Option<Rows> {
    if s.len() > MAX_JSON {
        return None;
    }
    let Value::Array(items) = serde_json::from_str::<Value>(s).ok()? else {
        return None;
    };
    let over = items.len() > MAX_ROWS;
    let rows = items
        .iter()
        .take(MAX_ROWS)
        .filter_map(|item| {
            let unit = text(item, name_key).filter(|u| u.ends_with(".service"))?;
            Some(Row {
                unit,
                description: text(item, "description"),
                load: state(item, "load"),
                active: state(item, "active"),
                sub: state(item, "sub"),
                state: state(item, "state"),
            })
        })
        .collect();
    Some((rows, over))
}

/// Parses `systemctl list-units --output=json`. `None` if not a JSON array.
fn parse_units(s: &str) -> Option<Rows> {
    parse_rows(s, "unit")
}

/// Parses `systemctl list-unit-files --output=json`. `None` if not a JSON array.
fn parse_unit_files(s: &str) -> Option<Rows> {
    parse_rows(s, "unit_file")
}

/// Merges by unit name, sorted by name, first entry wins. A unit that is
/// installed but not loaded keeps `unknown` load/active/sub; a loaded unit
/// without a unit file keeps `installed = false` and `unknown` enablement.
fn merge(units: Rows, files: Rows) -> (Vec<Service>, bool, Vec<String>) {
    let mut by_name: BTreeMap<String, Service> = BTreeMap::new();
    for r in units.0 {
        by_name.entry(r.unit.clone()).or_insert(Service {
            unit: r.unit,
            description: r.description,
            installed: false,
            load: r.load,
            active: r.active,
            sub: r.sub,
            enablement: UNKNOWN.to_owned(),
        });
    }
    let mut seen_files = std::collections::BTreeSet::new();
    for r in files.0 {
        if !seen_files.insert(r.unit.clone()) {
            continue;
        }
        let entry = by_name.entry(r.unit.clone()).or_insert_with(|| Service {
            unit: r.unit.clone(),
            description: None,
            installed: true,
            load: UNKNOWN.to_owned(),
            active: UNKNOWN.to_owned(),
            sub: UNKNOWN.to_owned(),
            enablement: UNKNOWN.to_owned(),
        });
        entry.installed = true;
        entry.enablement = r.state;
    }
    let mut limitations = Vec::new();
    let mut truncated = units.1 || files.1;
    let mut list: Vec<Service> = by_name.into_values().collect();
    if list.len() > MAX_SERVICES {
        list.truncate(MAX_SERVICES);
        truncated = true;
    }
    if truncated {
        limitations.push(format!(
            "The service list is incomplete: at most {MAX_SERVICES} services are listed."
        ));
    }
    (list, truncated, limitations)
}

fn unavailable(observed_at: String, probe: &'static str, message: String) -> ServicesInfo {
    ServicesInfo {
        observed_at,
        available: false,
        probe_errors: vec![ProbeError { probe, message }],
        ..Default::default()
    }
}

/// Runs both fixed `systemctl` commands and merges the results. Failed,
/// cut or unparsable output gives `available = false`. Cancel or a lost
/// connection returns the error.
pub async fn collect(
    exec: &dyn Executor,
    cancel: &CancelToken,
    observed_at: String,
) -> Result<ServicesInfo, ExecError> {
    let units = match probe_strict(exec, "systemctl", &LIST_UNITS, cancel).await {
        Probe::Output(o) => parse_units(&o),
        Probe::Failed(m) => return Ok(unavailable(observed_at, "listUnits", m)),
        Probe::Abort(e) => return Err(e),
    };
    let Some(units) = units else {
        return Ok(unavailable(
            observed_at,
            "listUnits",
            "output not recognized".into(),
        ));
    };
    let files = match probe_strict(exec, "systemctl", &LIST_UNIT_FILES, cancel).await {
        Probe::Output(o) => parse_unit_files(&o),
        Probe::Failed(m) => return Ok(unavailable(observed_at, "listUnitFiles", m)),
        Probe::Abort(e) => return Err(e),
    };
    let Some(files) = files else {
        return Ok(unavailable(
            observed_at,
            "listUnitFiles",
            "output not recognized".into(),
        ));
    };
    let (services, truncated, limitations) = merge(units, files);
    Ok(ServicesInfo {
        observed_at,
        available: true,
        services,
        truncated,
        limitations,
        probe_errors: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::{CommandSpec, ExecResult, ExitStatus};

    const UNITS: &str = r#"[
        {"unit":"ssh.service","load":"loaded","active":"active","sub":"running","description":"OpenBSD Secure Shell server"},
        {"unit":"broken.service","load":"loaded","active":"failed","sub":"failed","description":"Broken"},
        {"unit":"getty@tty1.service","load":"loaded","active":"active","sub":"running","description":"Getty"},
        {"not":"a unit"}, 7, {"unit":"x.timer","load":"loaded"}
    ]"#;
    const FILES: &str = r#"[
        {"unit_file":"ssh.service","state":"enabled","preset":"enabled"},
        {"unit_file":"broken.service","state":"disabled","preset":"enabled"},
        {"unit_file":"nginx.service","state":"disabled","preset":"enabled"}
    ]"#;

    fn merged(units: &str, files: &str) -> (Vec<Service>, bool, Vec<String>) {
        merge(
            parse_units(units).unwrap(),
            parse_unit_files(files).unwrap(),
        )
    }

    #[test]
    fn merges_loaded_and_installed_units() {
        let (list, truncated, limits) = merged(UNITS, FILES);
        assert!(!truncated && limits.is_empty());
        let names: Vec<_> = list.iter().map(|s| s.unit.as_str()).collect();
        assert_eq!(
            names,
            [
                "broken.service",
                "getty@tty1.service",
                "nginx.service",
                "ssh.service"
            ]
        );
        let ssh = &list[3];
        assert!(ssh.installed);
        assert_eq!(
            (ssh.load.as_str(), ssh.active.as_str()),
            ("loaded", "active")
        );
        assert_eq!(ssh.enablement, "enabled");
        assert_eq!(list[0].active, "failed");
        // Loaded but without a unit file (template instance): not installed.
        assert!(!list[1].installed);
        assert_eq!(list[1].enablement, "unknown");
    }

    #[test]
    fn installed_but_not_loaded_unit_keeps_unknown_states() {
        let (list, _, _) = merged(UNITS, FILES);
        let nginx = list.iter().find(|s| s.unit == "nginx.service").unwrap();
        assert!(nginx.installed);
        assert_eq!(nginx.enablement, "disabled");
        assert_eq!(
            (
                nginx.load.as_str(),
                nginx.active.as_str(),
                nginx.sub.as_str()
            ),
            ("unknown", "unknown", "unknown")
        );
        assert_eq!(nginx.description, None);
    }

    #[test]
    fn malformed_json_is_rejected() {
        for bad in [
            "",
            "not json",
            "{\"unit\":\"a.service\"}",
            "[{\"unit\":",
            "null",
        ] {
            assert!(parse_units(bad).is_none(), "{bad}");
            assert!(parse_unit_files(bad).is_none(), "{bad}");
        }
        assert!(parse_units(&" ".repeat(MAX_JSON + 1)).is_none());
        // Odd state values never leak through.
        let (rows, _) =
            parse_units(r#"[{"unit":"a.service","load":"Lo aded!","active":5}]"#).unwrap();
        assert_eq!(
            (rows[0].load.as_str(), rows[0].active.as_str()),
            ("unknown", "unknown")
        );
    }

    #[test]
    fn list_is_capped_and_truncation_is_recorded() {
        let files = format!(
            "[{}]",
            (0..MAX_SERVICES + 3)
                .map(|i| format!(r#"{{"unit_file":"s{i:04}.service","state":"enabled"}}"#))
                .collect::<Vec<_>>()
                .join(",")
        );
        let (list, truncated, limits) = merged("[]", &files);
        assert_eq!(list.len(), MAX_SERVICES);
        assert!(truncated);
        assert_eq!(limits.len(), 1);
        let (list, truncated, _) = merged("[]", "[]");
        assert!(list.is_empty() && !truncated);
    }

    struct Fake {
        units: &'static str,
        files: &'static str,
    }

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            assert_eq!(spec.program(), "systemctl");
            let out = if spec.args()[0] == "list-units" {
                self.units
            } else {
                self.files
            };
            Ok(ExecResult {
                exit: ExitStatus::Code(0),
                stdout: out.into(),
                stderr: String::new(),
                truncated: false,
            })
        }
    }

    #[tokio::test]
    async fn collect_merges_and_reports_unparsable_output_as_unavailable() {
        let cancel = CancelToken::new();
        let ok = collect(
            &Fake {
                units: UNITS,
                files: FILES,
            },
            &cancel,
            "t".into(),
        )
        .await
        .unwrap();
        assert!(ok.available && ok.services.len() == 4);

        let bad = collect(
            &Fake {
                units: UNITS,
                files: "oops",
            },
            &cancel,
            "t".into(),
        )
        .await
        .unwrap();
        assert!(!bad.available && bad.services.is_empty());
        assert_eq!(bad.probe_errors[0].probe, "listUnitFiles");
    }
}
