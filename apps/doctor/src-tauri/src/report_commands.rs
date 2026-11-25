//! Doctor-only report commands.
//!
//! `build_report` is pure. `save_report` rebuilds the report from the same
//! request and writes it to a path the user picks in a native save dialog that
//! Rust opens. The frontend never supplies a path or file content, and the
//! dialog plugin's JavaScript commands are not granted to any window (no
//! `dialog:` permission in the capability file).

use ossiframe_desktop::ipc::IpcError;
use ossiframe_mod_linux_audit::report::{ReportError, ReportInput, build_html, build_json};
use serde::Deserialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

/// Largest accepted request, measured as serialized JSON.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportFormat {
    Json,
    Html,
}

impl ReportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Html => "html",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReportRequest {
    format: ReportFormat,
    report: ReportInput,
}

fn invalid(field: &str, message: &str) -> IpcError {
    IpcError::Validation {
        field: field.into(),
        message: message.into(),
    }
}

fn report_error(e: ReportError) -> IpcError {
    invalid("report", &e.to_string())
}

/// Checks the size, parses the request and rejects unknown fields.
fn parse_request(value: serde_json::Value) -> Result<ReportRequest, IpcError> {
    let size = serde_json::to_vec(&value)
        .map_err(|_| invalid("request", "invalid request"))?
        .len();
    if size > MAX_REQUEST_BYTES {
        return Err(invalid("request", "request is larger than 4 MiB"));
    }
    serde_json::from_value(value).map_err(|_| invalid("request", "invalid request"))
}

fn render(req: &ReportRequest, generated_at: &str) -> Result<String, IpcError> {
    Ok(match req.format {
        ReportFormat::Json => build_json(&req.report, generated_at).map_err(report_error)?,
        ReportFormat::Html => build_html(&req.report, generated_at).map_err(report_error)?,
    })
}

fn now() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".into())
}

/// `ossiframe-doctor-report-YYYYMMDD.json|html`, from a UTC date.
fn default_file_name(format: ReportFormat, generated_at: &str) -> String {
    let date: String = generated_at
        .chars()
        .filter(char::is_ascii_digit)
        .take(8)
        .collect();
    let date = if date.len() == 8 {
        date
    } else {
        "report".into()
    };
    format!("ossiframe-doctor-{date}.{}", format.extension())
}

/// Builds the report text for the preview. No I/O.
#[tauri::command]
pub async fn build_report(request: serde_json::Value) -> Result<String, IpcError> {
    let req = parse_request(request)?;
    render(&req, &now())
}

/// Rebuilds the report and saves it where the user chooses. Returns `false`
/// when the user cancels the dialog.
#[tauri::command]
pub async fn save_report(app: AppHandle, request: serde_json::Value) -> Result<bool, IpcError> {
    let req = parse_request(request)?;
    let generated_at = now();
    let text = render(&req, &generated_at)?;
    let name = default_file_name(req.format, &generated_at);
    let (label, ext) = match req.format {
        ReportFormat::Json => ("JSON report", "json"),
        ReportFormat::Html => ("HTML report", "html"),
    };
    // `blocking_save_file` waits for the dialog, which the plugin runs on the
    // main thread. It must not run on the main thread or on an async worker,
    // so it runs on a blocking-pool thread.
    tauri::async_runtime::spawn_blocking(move || {
        let chosen = app
            .dialog()
            .file()
            .set_file_name(name)
            .add_filter(label, &[ext])
            .blocking_save_file();
        let Some(chosen) = chosen else {
            return Ok(false);
        };
        let path = chosen.into_path().map_err(|_| IpcError::OperationFailed {
            message: "the chosen location is not a file path".into(),
        })?;
        std::fs::write(&path, text.as_bytes()).map_err(|e| IpcError::OperationFailed {
            message: format!("could not save the report ({})", e.kind()),
        })?;
        Ok(true)
    })
    .await
    .map_err(|_| IpcError::Internal)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn report() -> serde_json::Value {
        json!({
            "server": {"profileName": "web", "host": "198.51.100.7", "port": 22,
                       "hostname": "h", "os": null, "kernel": null},
            "scan": {"startedAt": "2025-01-01T00:00:00Z", "finishedAt": null,
                     "completedSteps": [], "stale": false},
            "steps": [],
            "redactHost": false
        })
    }

    #[test]
    fn builds_json_and_html_and_rejects_unknown_format_or_fields() {
        for f in ["json", "html"] {
            let req = parse_request(json!({"format": f, "report": report()})).unwrap();
            assert!(!render(&req, "t").unwrap().is_empty());
        }
        for bad in [
            json!({"format": "pdf", "report": report()}),
            json!({"format": "json", "report": report(), "path": "/tmp/x"}),
            json!({"format": "json", "report": report(), "content": "x"}),
            json!({"format": "json"}),
        ] {
            assert!(parse_request(bad).is_err());
        }
    }

    #[test]
    fn oversized_request_is_rejected() {
        let mut r = report();
        r["server"]["profileName"] = json!("a".repeat(MAX_REQUEST_BYTES));
        let err = parse_request(json!({"format": "json", "report": r})).unwrap_err();
        assert!(
            matches!(err, IpcError::Validation { ref message, .. } if message.contains("4 MiB"))
        );
    }

    #[test]
    fn builds_are_deterministic_for_the_same_timestamp() {
        let req = parse_request(json!({"format": "json", "report": report()})).unwrap();
        assert_eq!(render(&req, "t").unwrap(), render(&req, "t").unwrap());
    }

    #[test]
    fn default_file_name_has_extension_and_no_separators() {
        assert_eq!(
            default_file_name(ReportFormat::Html, "2025-03-04T05:06:07Z"),
            "ossiframe-doctor-20250304.html"
        );
        let n = default_file_name(ReportFormat::Json, "unknown");
        assert!(n.ends_with(".json") && !n.contains('/'));
    }
}
