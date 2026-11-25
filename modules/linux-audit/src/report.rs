//! Report builder. Pure functions: a typed, bounded [`ReportInput`] goes in,
//! a JSON string or one self-contained HTML string comes out. No I/O.
//!
//! The input is validated here, not in the caller. Only the fields written
//! below are emitted; the input types deny unknown fields, so a username or key
//! path cannot be passed in by accident. Output depends only on the input and
//! `generated_at`, so a preview and an export differ only in `generatedAt`.
//!
//! The report contains text read from the server. It is not checked for
//! sensitive data, and the report says so.

use crate::OPERATIONS;
use crate::model::{Category, Outcome, ScanStatus, Severity, Summary};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fmt;

/// Version of the JSON report layout.
pub const SCHEMA_VERSION: u32 = 1;

pub const MAX_STEPS: usize = 8;
pub const MAX_CHECKS_PER_STEP: usize = 100;
pub const MAX_EVIDENCE_LINES: usize = 10;
pub const MAX_EVIDENCE_CHARS: usize = 200;
pub const MAX_LIMITATIONS_PER_CHECK: usize = 20;
/// Serialized size of one step's `evidence` value.
pub const MAX_STEP_EVIDENCE_BYTES: usize = 512 * 1024;
const MAX_SHORT: usize = 255;
const MAX_TEXT: usize = 2000;
const MAX_LIMITATION: usize = 500;
const MAX_TIME: usize = 40;
const MAX_FILESYSTEMS: usize = 50;
const MAX_SOCKETS: usize = 100;
const MAX_PACKAGES: usize = 100;
const MAX_EVIDENCE_LIMITATIONS: usize = 20;

const CSP: &str = "default-src 'none'; style-src 'unsafe-inline'";

const CONTENTS_NOTE: &str = "This report contains text read from the server: hostname, \
    operating system, service and package names and versions, ports, process names, \
    configuration file names and evidence lines. Ossiframe does not check that text for \
    sensitive data. Review the report before you share it.";
const REDACTED_NOTE: &str = "The connection address was omitted. Hostnames, service names, \
    package versions, ports, process names and file paths are still present. The report is \
    not anonymized.";
const NOT_REDACTED_NOTE: &str = "The connection address is included in this report.";

/// Why a report could not be built. Never contains input text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportError {
    TooManySteps,
    UnknownStep,
    DuplicateStep,
    CompletedStepNotPresent,
    TooManyChecks,
    FieldTooLong(&'static str),
    FieldEmpty(&'static str),
    TooManyItems(&'static str),
    InvalidRuleId,
    SeverityMismatch,
    EvidenceTooLarge,
}

impl fmt::Display for ReportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManySteps => write!(f, "too many steps"),
            Self::UnknownStep => write!(f, "unknown step"),
            Self::DuplicateStep => write!(f, "duplicate step"),
            Self::CompletedStepNotPresent => write!(f, "completed step has no result"),
            Self::TooManyChecks => write!(f, "too many checks in a step"),
            Self::FieldTooLong(n) => write!(f, "{n} is too long"),
            Self::FieldEmpty(n) => write!(f, "{n} is empty"),
            Self::TooManyItems(n) => write!(f, "too many items in {n}"),
            Self::InvalidRuleId => write!(f, "invalid rule id"),
            Self::SeverityMismatch => write!(f, "severity is set only on findings"),
            Self::EvidenceTooLarge => write!(f, "step evidence is too large"),
        }
    }
}

impl std::error::Error for ReportError {}

/// The server as the user configured and observed it. There is no username
/// and no key path.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ServerIdentity {
    pub profile_name: String,
    /// The configured connection address. Omitted when `redact_host` is set.
    pub host: String,
    pub port: u16,
    pub hostname: Option<String>,
    pub os: Option<String>,
    pub kernel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScanInput {
    pub started_at: String,
    /// `None` when the scan did not finish.
    pub finished_at: Option<String>,
    /// Steps that ran to the end. Scan status is derived from this.
    pub completed_steps: Vec<String>,
    /// The connection was lost; results show their age.
    pub stale: bool,
}

/// One check result as received from the frontend.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckInput {
    pub rule_id: String,
    pub title: String,
    pub category: Category,
    pub outcome: Outcome,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub resource: Option<String>,
    pub evidence: Vec<String>,
    pub explanation: String,
    #[serde(default)]
    pub recommendation: Option<String>,
    pub observed_at: String,
    pub limitations: Vec<String>,
    pub inferred: bool,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StepInput {
    pub step: String,
    pub observed_at: String,
    /// Typed per step. Only known keys are read; the rest is dropped.
    pub evidence: Value,
    pub checks: Vec<CheckInput>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReportInput {
    pub server: ServerIdentity,
    pub scan: ScanInput,
    pub steps: Vec<StepInput>,
    pub redact_host: bool,
}

// ---------------------------------------------------------------------------
// Validation

fn check_len(s: &str, max: usize, name: &'static str) -> Result<(), ReportError> {
    if s.chars().count() > max {
        Err(ReportError::FieldTooLong(name))
    } else {
        Ok(())
    }
}

fn check_opt(s: &Option<String>, max: usize, name: &'static str) -> Result<(), ReportError> {
    s.as_deref().map_or(Ok(()), |v| check_len(v, max, name))
}

fn known_step(id: &str) -> bool {
    OPERATIONS.iter().any(|o| o.id == id)
}

/// Checks size and count caps, step ids and field consistency.
pub fn validate(input: &ReportInput) -> Result<(), ReportError> {
    let s = &input.server;
    if s.profile_name.trim().is_empty() {
        return Err(ReportError::FieldEmpty("profileName"));
    }
    check_len(&s.profile_name, 100, "profileName")?;
    check_len(&s.host, MAX_SHORT, "host")?;
    check_opt(&s.hostname, MAX_SHORT, "hostname")?;
    check_opt(&s.os, MAX_SHORT, "os")?;
    check_opt(&s.kernel, MAX_SHORT, "kernel")?;

    let scan = &input.scan;
    check_len(&scan.started_at, MAX_TIME, "startedAt")?;
    check_opt(&scan.finished_at, MAX_TIME, "finishedAt")?;
    if scan.completed_steps.len() > MAX_STEPS {
        return Err(ReportError::TooManyItems("completedSteps"));
    }

    if input.steps.len() > MAX_STEPS {
        return Err(ReportError::TooManySteps);
    }
    for (i, step) in input.steps.iter().enumerate() {
        if !known_step(&step.step) {
            return Err(ReportError::UnknownStep);
        }
        if input.steps[..i].iter().any(|p| p.step == step.step) {
            return Err(ReportError::DuplicateStep);
        }
        validate_step(step)?;
    }
    for (i, id) in scan.completed_steps.iter().enumerate() {
        if !known_step(id) {
            return Err(ReportError::UnknownStep);
        }
        if scan.completed_steps[..i].contains(id) {
            return Err(ReportError::DuplicateStep);
        }
        if !input.steps.iter().any(|s| &s.step == id) {
            return Err(ReportError::CompletedStepNotPresent);
        }
    }
    Ok(())
}

fn validate_step(step: &StepInput) -> Result<(), ReportError> {
    check_len(&step.observed_at, MAX_TIME, "observedAt")?;
    if step.checks.len() > MAX_CHECKS_PER_STEP {
        return Err(ReportError::TooManyChecks);
    }
    match serde_json::to_string(&step.evidence) {
        Ok(s) if s.len() <= MAX_STEP_EVIDENCE_BYTES => {}
        _ => return Err(ReportError::EvidenceTooLarge),
    }
    for c in &step.checks {
        if c.rule_id.is_empty()
            || c.rule_id.len() > 64
            || !c
                .rule_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
        {
            return Err(ReportError::InvalidRuleId);
        }
        check_len(&c.title, 200, "title")?;
        check_opt(&c.resource, MAX_EVIDENCE_CHARS, "resource")?;
        check_len(&c.explanation, MAX_TEXT, "explanation")?;
        check_opt(&c.recommendation, MAX_TEXT, "recommendation")?;
        check_len(&c.observed_at, MAX_TIME, "observedAt")?;
        if c.evidence.len() > MAX_EVIDENCE_LINES {
            return Err(ReportError::TooManyItems("evidence"));
        }
        for l in &c.evidence {
            check_len(l, MAX_EVIDENCE_CHARS, "evidence line")?;
        }
        if c.limitations.len() > MAX_LIMITATIONS_PER_CHECK {
            return Err(ReportError::TooManyItems("limitations"));
        }
        for l in &c.limitations {
            check_len(l, MAX_LIMITATION, "limitation")?;
        }
        if c.severity.is_some() != (c.outcome == Outcome::Finding) {
            return Err(ReportError::SeverityMismatch);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared model

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Critical => 0,
        Severity::High => 1,
        Severity::Medium => 2,
        Severity::Low => 3,
        Severity::Info => 4,
    }
}

const SEVERITIES: [Severity; 5] = [
    Severity::Critical,
    Severity::High,
    Severity::Medium,
    Severity::Low,
    Severity::Info,
];

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Critical => "Critical",
        Severity::High => "High",
        Severity::Medium => "Medium",
        Severity::Low => "Low",
        Severity::Info => "Info",
    }
}

fn outcome_label(o: Outcome) -> &'static str {
    match o {
        Outcome::Passed => "Passed",
        Outcome::Finding => "Finding",
        Outcome::Unavailable => "Unavailable",
        Outcome::Incomplete => "Incomplete",
        Outcome::NotApplicable => "Not applicable",
        Outcome::Error => "Error",
    }
}

fn category_label(c: Category) -> &'static str {
    match c {
        Category::Resources => "Resources",
        Category::Services => "Services",
        Category::Network => "Network",
        Category::Security => "Security",
        Category::Updates => "Updates",
    }
}

/// A check with the step it came from.
type Row<'a> = (&'a str, &'a CheckInput);

/// Derived data shared by both formats.
struct Model<'a> {
    rows: Vec<Row<'a>>,
    findings: Vec<Row<'a>>,
    not_assessed: Vec<Row<'a>>,
    passed: Vec<Row<'a>>,
    summary: Summary,
    status: ScanStatus,
    missing_steps: Vec<&'static str>,
    limitations: Vec<&'a str>,
    infra: Infra,
}

impl<'a> Model<'a> {
    fn new(input: &'a ReportInput) -> Self {
        let mut steps: Vec<&StepInput> = input.steps.iter().collect();
        steps.sort_by_key(|s| OPERATIONS.iter().position(|o| o.id == s.step));
        let rows: Vec<Row> = steps
            .iter()
            .flat_map(|s| s.checks.iter().map(move |c| (s.step.as_str(), c)))
            .collect();

        let mut findings: Vec<Row> = rows
            .iter()
            .filter(|(_, c)| c.outcome == Outcome::Finding)
            .copied()
            .collect();
        findings.sort_by_key(|(_, c)| c.severity.map_or(u8::MAX, severity_rank));
        let not_assessed = rows
            .iter()
            .filter(|(_, c)| {
                matches!(
                    c.outcome,
                    Outcome::Unavailable | Outcome::Incomplete | Outcome::Error
                )
            })
            .copied()
            .collect();
        let passed = rows
            .iter()
            .filter(|(_, c)| matches!(c.outcome, Outcome::Passed | Outcome::NotApplicable))
            .copied()
            .collect();

        let mut summary = Summary::default();
        for (_, c) in &rows {
            summary.add(c.outcome, c.severity);
        }

        let expected: Vec<&str> = OPERATIONS.iter().map(|o| o.id).collect();
        let done: Vec<&str> = input
            .scan
            .completed_steps
            .iter()
            .map(String::as_str)
            .collect();
        let status = ScanStatus::of(&expected, &done);
        let missing_steps = OPERATIONS
            .iter()
            .map(|o| o.id)
            .filter(|id| !done.contains(id))
            .collect();

        let mut limitations: Vec<&str> = Vec::new();
        for (_, c) in &rows {
            for l in &c.limitations {
                if !limitations.contains(&l.as_str()) {
                    limitations.push(l);
                }
            }
        }
        let infra = Infra::from_steps(&input.server, &steps);
        Self {
            rows,
            findings,
            not_assessed,
            passed,
            summary,
            status,
            missing_steps,
            limitations,
            infra,
        }
    }

    /// Check limitations first, then evidence limitations, without repeats.
    fn all_limitations(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for l in self
            .limitations
            .iter()
            .map(|s| (*s).to_owned())
            .chain(self.infra.limitations.iter().cloned())
        {
            if !out.contains(&l) {
                out.push(l);
            }
        }
        out
    }
}

fn status_str(s: ScanStatus) -> &'static str {
    match s {
        ScanStatus::Complete => "complete",
        ScanStatus::Incomplete => "incomplete",
    }
}

// ---------------------------------------------------------------------------
// Infrastructure projection. Reads known keys with type checks and bounds.

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Resources {
    memory_total_kb: Option<u64>,
    memory_available_kb: Option<u64>,
    swap_total_kb: Option<u64>,
    swap_free_kb: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Filesystem {
    mount: String,
    total_bytes: u64,
    used_bytes: u64,
    available_bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServiceCounts {
    unit_count: u64,
    failed_count: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Socket {
    protocol: String,
    address: String,
    port: u64,
    binding: String,
    process: Option<String>,
}

#[derive(Debug, Serialize)]
struct Package {
    name: String,
    version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct Infra {
    os: Option<String>,
    kernel: Option<String>,
    resources: Option<Resources>,
    filesystems: Option<Vec<Filesystem>>,
    filesystems_truncated: bool,
    services: Option<ServiceCounts>,
    /// Sockets on database ports only, as collected by the network step.
    database_port_sockets: Option<Vec<Socket>>,
    packages: Option<Vec<Package>>,
    #[serde(skip)]
    limitations: Vec<String>,
}

fn clip(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_EVIDENCE_CHARS)
        .collect()
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(clip)
}

fn num(v: &Value, key: &str) -> Option<u64> {
    v.get(key).and_then(Value::as_u64)
}

fn items<'a>(v: &'a Value, key: &str, max: usize) -> Vec<&'a Value> {
    v.get(key)
        .and_then(Value::as_array)
        .map(|a| a.iter().take(max).collect())
        .unwrap_or_default()
}

impl Infra {
    fn from_steps(server: &ServerIdentity, steps: &[&StepInput]) -> Self {
        let ev = |id: &str| steps.iter().find(|s| s.step == id).map(|s| &s.evidence);
        let mut limitations = Vec::new();

        let resources_ev = ev("resources");
        let resources = resources_ev.map(|e| Resources {
            memory_total_kb: num(e, "memoryTotalKb"),
            memory_available_kb: num(e, "memoryAvailableKb"),
            swap_total_kb: num(e, "swapTotalKb"),
            swap_free_kb: num(e, "swapFreeKb"),
        });
        let filesystems = resources_ev.map(|e| {
            items(e, "filesystems", MAX_FILESYSTEMS)
                .into_iter()
                .filter_map(|f| {
                    Some(Filesystem {
                        mount: text(f, "mount")?,
                        total_bytes: num(f, "totalBytes")?,
                        used_bytes: num(f, "usedBytes")?,
                        available_bytes: num(f, "availableBytes")?,
                    })
                })
                .collect()
        });
        let filesystems_truncated = resources_ev
            .and_then(|e| e.get("filesystemsTruncated"))
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let services = ev("services").and_then(|e| {
            Some(ServiceCounts {
                unit_count: num(e, "unitCount")?,
                failed_count: num(e, "failedCount")?,
            })
        });

        let database_port_sockets = ev("network").map(|e| {
            items(e, "databasePortSockets", MAX_SOCKETS)
                .into_iter()
                .filter_map(|s| {
                    Some(Socket {
                        protocol: text(s, "protocol")?,
                        address: text(s, "address")?,
                        port: num(s, "port")?,
                        binding: text(s, "binding")?,
                        process: s.get("process").and_then(|p| text(p, "name")),
                    })
                })
                .collect()
        });

        let software = ev("software");
        let packages = software.map(|e| {
            items(e, "packages", MAX_PACKAGES)
                .into_iter()
                .filter_map(|p| {
                    Some(Package {
                        name: text(p, "name")?,
                        version: text(p, "version")?,
                    })
                })
                .collect()
        });
        for id in ["software", "ssh"] {
            if let Some(e) = ev(id) {
                for l in items(e, "limitations", MAX_EVIDENCE_LIMITATIONS) {
                    if let Some(l) = l.as_str() {
                        let l: String = l.chars().take(MAX_LIMITATION).collect();
                        if !limitations.contains(&l) {
                            limitations.push(l);
                        }
                    }
                }
            }
        }

        Self {
            os: server.os.as_deref().map(clip),
            kernel: server.kernel.as_deref().map(clip),
            resources,
            filesystems,
            filesystems_truncated,
            services,
            database_port_sockets,
            packages,
            limitations,
        }
    }
}

// ---------------------------------------------------------------------------
// JSON

fn result_json(step: &str, c: &CheckInput) -> Value {
    let mut v = json!({
        "step": step,
        "ruleId": c.rule_id,
        "title": c.title,
        "category": c.category,
        "outcome": c.outcome,
        "resource": c.resource,
        "evidence": c.evidence,
        "explanation": c.explanation,
        "recommendation": c.recommendation,
        "observedAt": c.observed_at,
        "limitations": c.limitations,
        "inferred": c.inferred,
    });
    if let (Some(sev), Some(obj)) = (c.severity, v.as_object_mut()) {
        obj.insert("severity".to_owned(), json!(sev));
    }
    v
}

fn server_json(input: &ReportInput) -> Value {
    let s = &input.server;
    let mut v = json!({
        "profileName": s.profile_name,
        "port": s.port,
        "hostname": s.hostname,
        "os": s.os,
        "hostRedacted": input.redact_host,
    });
    if let (false, Some(obj)) = (input.redact_host, v.as_object_mut()) {
        obj.insert("host".to_owned(), json!(s.host));
    }
    v
}

/// Builds the JSON report (`schemaVersion` 1). Fails only on invalid input.
pub fn build_json(input: &ReportInput, generated_at: &str) -> Result<String, ReportError> {
    validate(input)?;
    let m = Model::new(input);
    let results: Vec<Value> = m.rows.iter().map(|(s, c)| result_json(s, c)).collect();
    let findings: Vec<Value> = m.findings.iter().map(|(s, c)| result_json(s, c)).collect();
    let unavailable: Vec<Value> = m
        .not_assessed
        .iter()
        .map(|(s, c)| result_json(s, c))
        .collect();
    let report = json!({
        "schemaVersion": SCHEMA_VERSION,
        "generatedAt": generated_at,
        "notes": {
            "contents": CONTENTS_NOTE,
            "redaction": if input.redact_host { REDACTED_NOTE } else { NOT_REDACTED_NOTE },
        },
        "server": server_json(input),
        "scan": {
            "startedAt": input.scan.started_at,
            "finishedAt": input.scan.finished_at,
            "status": m.status,
            "coverage": m.summary.coverage,
            "missingSteps": m.missing_steps,
            "stale": input.scan.stale,
        },
        "infrastructure": m.infra,
        "summary": {
            "outcomes": m.summary.outcomes,
            "severities": m.summary.severities,
        },
        "results": results,
        "findings": findings,
        "unavailable": unavailable,
        "limitations": m.all_limitations(),
    });
    serde_json::to_string_pretty(&report).map_err(|_| ReportError::EvidenceTooLarge)
}

// ---------------------------------------------------------------------------
// HTML

/// The only place a dynamic value is turned into HTML text. Control
/// characters are dropped.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font:14px/1.5 system-ui,sans-serif;max-width:60rem;margin:2rem auto;\
padding:0 1rem;color:#1a1a1a}h1,h2,h3{line-height:1.25}h2{margin-top:2rem;border-bottom:1px solid #ccc;\
padding-bottom:.25rem}table{border-collapse:collapse;width:100%;margin:.5rem 0}th,td{border:1px solid #ccc;\
padding:.25rem .5rem;text-align:left;vertical-align:top}th{background:#f3f3f3}\
article{border:1px solid #ccc;border-radius:4px;padding:.5rem 1rem;margin:.75rem 0}\
.note{background:#fff8e1;border:1px solid #e0c060;padding:.5rem 1rem}\
.badge{display:inline-block;padding:0 .4rem;border:1px solid #666;border-radius:3px;font-size:12px}\
code{font-family:ui-monospace,monospace;font-size:13px;word-break:break-all}";

fn dl(out: &mut String, rows: &[(&str, Option<String>)]) {
    out.push_str("<table>");
    for (k, v) in rows {
        out.push_str(&format!(
            "<tr><th>{}</th><td>{}</td></tr>",
            esc(k),
            esc(v.as_deref().unwrap_or("not collected"))
        ));
    }
    out.push_str("</table>");
}

fn list(out: &mut String, items: &[String]) {
    out.push_str("<ul>");
    for i in items {
        out.push_str(&format!("<li>{}</li>", esc(i)));
    }
    out.push_str("</ul>");
}

fn check_html(out: &mut String, step: &str, c: &CheckInput) {
    let label = match c.severity {
        Some(s) => format!("{} finding", severity_label(s)),
        None => outcome_label(c.outcome).to_owned(),
    };
    out.push_str(&format!(
        "<article><h4>{} <span class=\"badge\">{}</span></h4>",
        esc(&c.title),
        esc(&label)
    ));
    let mut meta = format!(
        "Step: {}. Rule: {}. Category: {}. Observed: {}.",
        step,
        c.rule_id,
        category_label(c.category),
        c.observed_at
    );
    if let Some(r) = &c.resource {
        meta.push_str(&format!(" Resource: {r}."));
    }
    out.push_str(&format!("<p><small>{}</small></p>", esc(&meta)));
    if c.inferred {
        out.push_str("<p><span class=\"badge\">Inferred</span> This conclusion goes beyond observed facts.</p>");
    }
    out.push_str(&format!("<p>{}</p>", esc(&c.explanation)));
    if !c.evidence.is_empty() {
        out.push_str("<p>Evidence:</p><ul>");
        for l in &c.evidence {
            out.push_str(&format!("<li><code>{}</code></li>", esc(l)));
        }
        out.push_str("</ul>");
    }
    if let Some(r) = &c.recommendation {
        out.push_str(&format!("<p>Recommendation: {}</p>", esc(r)));
    }
    if !c.limitations.is_empty() {
        out.push_str("<p>Limitations:</p>");
        list(out, &c.limitations);
    }
    out.push_str("</article>");
}

fn kb(v: Option<u64>) -> Option<String> {
    v.map(|n| format!("{n} KiB"))
}

/// Builds one self-contained HTML file: no script, no remote assets, a
/// restrictive CSP meta tag. Fails only on invalid input.
pub fn build_html(input: &ReportInput, generated_at: &str) -> Result<String, ReportError> {
    validate(input)?;
    let m = Model::new(input);
    let s = &input.server;
    let mut o = String::new();

    o.push_str(&format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">\
<title>{}</title><style>{}</style></head><body>",
        esc(CSP),
        esc(&format!("Ossiframe Doctor report: {}", s.profile_name)),
        STYLE
    ));
    o.push_str(&format!(
        "<h1>Ossiframe Doctor report</h1><p>Server: {}. Generated at {}. Schema version {}.</p>",
        esc(&s.profile_name),
        esc(generated_at),
        SCHEMA_VERSION
    ));
    o.push_str(&format!(
        "<p class=\"note\">{}</p><p class=\"note\">{}</p>",
        esc(CONTENTS_NOTE),
        esc(if input.redact_host {
            REDACTED_NOTE
        } else {
            NOT_REDACTED_NOTE
        })
    ));

    // Server
    o.push_str("<h2>Server</h2>");
    let mut rows = vec![
        ("Profile", Some(s.profile_name.clone())),
        ("Port", Some(s.port.to_string())),
        ("Hostname", s.hostname.clone()),
        ("Operating system", s.os.clone()),
        ("Kernel", s.kernel.clone()),
    ];
    if !input.redact_host {
        rows.insert(1, ("Connection address", Some(s.host.clone())));
    }
    dl(&mut o, &rows);

    // Summary
    let c = &m.summary.outcomes;
    let v = &m.summary.severities;
    o.push_str("<h2>Summary</h2><h3>Findings by severity</h3><table><tr>");
    for sev in SEVERITIES {
        o.push_str(&format!("<th>{}</th>", esc(severity_label(sev))));
    }
    o.push_str("</tr><tr>");
    for n in [v.critical, v.high, v.medium, v.low, v.info] {
        o.push_str(&format!("<td>{n}</td>"));
    }
    o.push_str("</tr></table><h3>Results by outcome</h3><table><tr>");
    let outcome_cols = [
        (Outcome::Passed, c.passed),
        (Outcome::Finding, c.finding),
        (Outcome::Unavailable, c.unavailable),
        (Outcome::Incomplete, c.incomplete),
        (Outcome::NotApplicable, c.not_applicable),
        (Outcome::Error, c.error),
    ];
    for (oc, _) in outcome_cols {
        o.push_str(&format!("<th>{}</th>", esc(outcome_label(oc))));
    }
    o.push_str("</tr><tr>");
    for (_, n) in outcome_cols {
        o.push_str(&format!("<td>{n}</td>"));
    }
    o.push_str("</tr></table><p>There is no security score.</p>");

    // Scan status vs coverage
    let cov = &m.summary.coverage;
    o.push_str("<h2>Scan status and coverage</h2><p>Scan status says whether every step ran to the end. Coverage says how many checks were actually assessed. A complete scan can still contain checks that were not assessed.</p>");
    dl(
        &mut o,
        &[
            ("Scan status", Some(status_str(m.status).to_owned())),
            ("Started", Some(input.scan.started_at.clone())),
            (
                "Finished",
                Some(
                    input
                        .scan
                        .finished_at
                        .clone()
                        .unwrap_or_else(|| "did not finish".to_owned()),
                ),
            ),
            (
                "Connection lost, results may be stale",
                Some(if input.scan.stale { "yes" } else { "no" }.to_owned()),
            ),
            (
                "Assessed (passed or finding)",
                Some(cov.assessed.to_string()),
            ),
            (
                "Not assessed (unavailable, incomplete or error)",
                Some(cov.not_assessed.to_string()),
            ),
            ("Not applicable", Some(cov.not_applicable.to_string())),
            (
                "Steps that did not complete",
                Some(if m.missing_steps.is_empty() {
                    "none".to_owned()
                } else {
                    m.missing_steps.join(", ")
                }),
            ),
        ],
    );

    // Findings
    o.push_str("<h2>Findings</h2>");
    if m.findings.is_empty() {
        o.push_str("<p>No findings in the checks that were assessed. This is not a statement about checks that were not assessed.</p>");
    }
    for sev in SEVERITIES {
        let group: Vec<&Row> = m
            .findings
            .iter()
            .filter(|(_, c)| c.severity == Some(sev))
            .collect();
        if group.is_empty() {
            continue;
        }
        o.push_str(&format!(
            "<h3>{} ({})</h3>",
            esc(severity_label(sev)),
            group.len()
        ));
        for (step, c) in group {
            check_html(&mut o, step, c);
        }
    }

    // Unavailable / incomplete
    o.push_str("<h2>Unavailable, incomplete and failed checks</h2>");
    if m.not_assessed.is_empty() && m.missing_steps.is_empty() {
        o.push_str("<p>None.</p>");
    }
    if !m.missing_steps.is_empty() {
        o.push_str("<p>Steps with no result in this report:</p>");
        let missing: Vec<String> = m.missing_steps.iter().map(|s| (*s).to_owned()).collect();
        list(&mut o, &missing);
    }
    for (step, c) in &m.not_assessed {
        check_html(&mut o, step, c);
    }

    // Passed
    o.push_str("<h2>Passed and not applicable checks</h2>");
    if m.passed.is_empty() {
        o.push_str("<p>None.</p>");
    } else {
        o.push_str("<table><tr><th>Check</th><th>Rule</th><th>Outcome</th></tr>");
        for (_, c) in &m.passed {
            o.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&c.title),
                esc(&c.rule_id),
                esc(outcome_label(c.outcome))
            ));
        }
        o.push_str("</table>");
    }

    infra_html(&mut o, &m.infra);

    // Limitations
    o.push_str("<h2>Limitations</h2>");
    let lims = m.all_limitations();
    if lims.is_empty() {
        o.push_str("<p>None recorded.</p>");
    } else {
        list(&mut o, &lims);
    }
    o.push_str("</body></html>");
    Ok(o)
}

fn infra_html(o: &mut String, i: &Infra) {
    o.push_str("<h2>Infrastructure summary</h2>");
    if let Some(r) = &i.resources {
        o.push_str("<h3>Memory</h3>");
        dl(
            o,
            &[
                ("Memory total", kb(r.memory_total_kb)),
                ("Memory available", kb(r.memory_available_kb)),
                ("Swap total", kb(r.swap_total_kb)),
                ("Swap free", kb(r.swap_free_kb)),
            ],
        );
    }
    if let Some(fs) = &i.filesystems {
        o.push_str("<h3>Filesystems</h3>");
        if fs.is_empty() {
            o.push_str("<p>None reported.</p>");
        } else {
            o.push_str("<table><tr><th>Mount</th><th>Total bytes</th><th>Used bytes</th><th>Available bytes</th></tr>");
            for f in fs {
                o.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&f.mount),
                    f.total_bytes,
                    f.used_bytes,
                    f.available_bytes
                ));
            }
            o.push_str("</table>");
        }
        if i.filesystems_truncated {
            o.push_str("<p>The filesystem list was truncated.</p>");
        }
    }
    if let Some(s) = &i.services {
        o.push_str("<h3>Services</h3>");
        dl(
            o,
            &[
                ("Units seen", Some(s.unit_count.to_string())),
                ("Failed units", Some(s.failed_count.to_string())),
            ],
        );
    }
    if let Some(socks) = &i.database_port_sockets {
        o.push_str("<h3>Listening sockets on database ports</h3>");
        if socks.is_empty() {
            o.push_str("<p>None seen.</p>");
        } else {
            o.push_str("<table><tr><th>Protocol</th><th>Address</th><th>Port</th><th>Binding</th><th>Process</th></tr>");
            for s in socks {
                o.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                    esc(&s.protocol),
                    esc(&s.address),
                    s.port,
                    esc(&s.binding),
                    esc(s.process.as_deref().unwrap_or("not visible"))
                ));
            }
            o.push_str("</table>");
        }
    }
    if let Some(p) = &i.packages {
        o.push_str("<h3>Server packages known to dpkg</h3>");
        if p.is_empty() {
            o.push_str("<p>None detected. Software installed outside dpkg is not detected.</p>");
        } else {
            o.push_str("<table><tr><th>Package</th><th>Version</th></tr>");
            for x in p {
                o.push_str(&format!(
                    "<tr><td>{}</td><td>{}</td></tr>",
                    esc(&x.name),
                    esc(&x.version)
                ));
            }
            o.push_str("</table>");
        }
    }
    if i.resources.is_none()
        && i.filesystems.is_none()
        && i.services.is_none()
        && i.database_port_sockets.is_none()
        && i.packages.is_none()
    {
        o.push_str("<p>No infrastructure data was collected.</p>");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "2025-01-01T00:00:00Z";
    const HOST: &str = "203.0.113.77";

    fn check(outcome: Outcome, severity: Option<Severity>) -> CheckInput {
        CheckInput {
            rule_id: "t.rule".to_owned(),
            title: "Title".to_owned(),
            category: Category::Security,
            outcome,
            severity,
            resource: None,
            evidence: vec!["line".to_owned()],
            explanation: "why".to_owned(),
            recommendation: None,
            observed_at: T.to_owned(),
            limitations: vec!["only global config".to_owned()],
            inferred: false,
        }
    }

    fn step(id: &str, checks: Vec<CheckInput>, evidence: Value) -> StepInput {
        StepInput {
            step: id.to_owned(),
            observed_at: T.to_owned(),
            evidence,
            checks,
        }
    }

    fn input() -> ReportInput {
        ReportInput {
            server: ServerIdentity {
                profile_name: "prod".to_owned(),
                host: HOST.to_owned(),
                port: 22,
                hostname: Some("web-1".to_owned()),
                os: Some("Ubuntu 24.04".to_owned()),
                kernel: Some("6.8.0".to_owned()),
            },
            scan: ScanInput {
                started_at: T.to_owned(),
                finished_at: Some(T.to_owned()),
                completed_steps: vec!["ssh".to_owned()],
                stale: false,
            },
            steps: vec![
                step(
                    "ssh",
                    vec![
                        check(Outcome::Finding, Some(Severity::High)),
                        check(Outcome::Unavailable, None),
                        check(Outcome::Passed, None),
                    ],
                    json!({ "limitations": ["Reads files on disk only."] }),
                ),
                step(
                    "resources",
                    vec![],
                    json!({
                        "memoryTotalKb": 1000,
                        "filesystems": [{"mount": "/", "totalBytes": 10, "usedBytes": 4,
                                         "availableBytes": 6, "secret": "x"}],
                        "unknownKey": "dropped",
                    }),
                ),
            ],
            redact_host: false,
        }
    }

    #[test]
    fn html_escapes_script_quotes_and_ampersands_in_every_dynamic_field() {
        let mut i = input();
        let evil = "<script>alert(\"x\")</script> & 'q'";
        i.server.profile_name = evil.to_owned();
        i.server.hostname = Some(evil.to_owned());
        i.server.os = Some(evil.to_owned());
        let c = &mut i.steps[0].checks[0];
        c.title = evil.to_owned();
        c.evidence = vec![evil.to_owned()];
        c.recommendation = Some(evil.to_owned());
        c.limitations = vec![evil.to_owned()];
        i.steps[1].evidence = json!({ "filesystems": [{"mount": evil, "totalBytes": 1, "usedBytes": 1, "availableBytes": 0}] });
        let html = build_html(&i, T).unwrap();
        assert!(!html.contains("<script"));
        assert!(!html.contains("alert(\"x\")"));
        assert!(
            html.contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;q&#39;")
        );
        assert!(!html.contains("http://") && !html.contains("https://"));
        assert!(
            html.contains(
                "content=\"default-src &#39;none&#39;; style-src &#39;unsafe-inline&#39;\""
            )
        );
        assert_eq!(esc("a\u{1b}[0m\nb"), "a[0mb");
    }

    #[test]
    fn host_redaction_removes_only_the_connection_address() {
        let mut i = input();
        i.redact_host = true;
        let json = build_json(&i, T).unwrap();
        let html = build_html(&i, T).unwrap();
        assert!(!json.contains(HOST) && !html.contains(HOST));
        let v: Value = serde_json::from_str(&json).unwrap();
        assert!(v["server"].get("host").is_none());
        assert_eq!(v["server"]["hostRedacted"], true);
        assert_eq!(v["server"]["hostname"], "web-1");
        assert!(html.contains("web-1"));
        assert!(html.contains("not anonymized"));
        // Without redaction the address is present.
        i.redact_host = false;
        let v: Value = serde_json::from_str(&build_json(&i, T).unwrap()).unwrap();
        assert_eq!(v["server"]["host"], HOST);
    }

    #[test]
    fn unknown_fields_are_rejected_including_username_and_key_path() {
        let good = serde_json::to_value(json!({
            "server": {"profileName": "p", "host": "h", "port": 22,
                       "hostname": null, "os": null, "kernel": null},
            "scan": {"startedAt": T, "finishedAt": null, "completedSteps": [], "stale": false},
            "steps": [],
            "redactHost": false,
        }))
        .unwrap();
        assert!(serde_json::from_value::<ReportInput>(good.clone()).is_ok());
        for (path, key) in [
            ("server", "username"),
            ("server", "keyPath"),
            ("scan", "extra"),
        ] {
            let mut bad = good.clone();
            bad[path][key] = json!("x");
            assert!(serde_json::from_value::<ReportInput>(bad).is_err(), "{key}");
        }
        let mut bad = good.clone();
        bad["extra"] = json!(1);
        assert!(serde_json::from_value::<ReportInput>(bad).is_err());
        let check_bad = json!({"ruleId": "a", "title": "t", "category": "security",
            "outcome": "passed", "evidence": [], "explanation": "e", "observedAt": T,
            "limitations": [], "inferred": false, "password": "x"});
        assert!(serde_json::from_value::<CheckInput>(check_bad).is_err());
    }

    #[test]
    fn caps_and_known_step_ids_are_enforced() {
        let mut i = input();
        i.steps[0].step = "shell".to_owned();
        assert_eq!(validate(&i), Err(ReportError::UnknownStep));

        let mut i = input();
        i.steps[0].checks = (0..101).map(|_| check(Outcome::Passed, None)).collect();
        assert_eq!(validate(&i), Err(ReportError::TooManyChecks));

        let mut i = input();
        i.steps.push(i.steps[0].clone());
        assert_eq!(validate(&i), Err(ReportError::DuplicateStep));

        let mut i = input();
        i.steps = OPERATIONS
            .iter()
            .map(|o| step(o.id, vec![], Value::Null))
            .chain([step("ssh", vec![], Value::Null)])
            .collect();
        assert_eq!(validate(&i), Err(ReportError::TooManySteps));

        let mut i = input();
        i.steps[0].checks[0].evidence = vec!["x".repeat(201)];
        assert!(matches!(validate(&i), Err(ReportError::FieldTooLong(_))));

        let mut i = input();
        i.steps[0].checks[0].evidence = vec!["x".to_owned(); 11];
        assert!(matches!(validate(&i), Err(ReportError::TooManyItems(_))));

        let mut i = input();
        i.steps[1].evidence = json!({ "blob": "x".repeat(MAX_STEP_EVIDENCE_BYTES) });
        assert_eq!(validate(&i), Err(ReportError::EvidenceTooLarge));

        let mut i = input();
        i.steps[0].checks[1].severity = Some(Severity::Low);
        assert_eq!(validate(&i), Err(ReportError::SeverityMismatch));

        let mut i = input();
        i.scan.completed_steps = vec!["firewall".to_owned()];
        assert_eq!(validate(&i), Err(ReportError::CompletedStepNotPresent));

        assert!(validate(&input()).is_ok());
        assert!(
            build_json(
                &{
                    let mut i = input();
                    i.steps[0].step = "x".into();
                    i
                },
                T
            )
            .is_err()
        );
    }

    #[test]
    fn json_has_the_schema_fields_and_keeps_status_and_coverage_separate() {
        let v: Value = serde_json::from_str(&build_json(&input(), "G").unwrap()).unwrap();
        for k in [
            "schemaVersion",
            "generatedAt",
            "server",
            "scan",
            "infrastructure",
            "summary",
            "results",
            "findings",
            "unavailable",
            "limitations",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert_eq!(v["schemaVersion"], 1);
        assert_eq!(v["generatedAt"], "G");
        assert_eq!(v["scan"]["status"], "incomplete");
        assert_eq!(v["scan"]["startedAt"], T);
        assert_eq!(v["scan"]["coverage"]["assessed"], 2);
        assert_eq!(v["scan"]["coverage"]["notAssessed"], 1);
        assert_eq!(v["scan"]["missingSteps"].as_array().unwrap().len(), 7);
        assert!(
            v["scan"]["missingSteps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|s| s != "ssh")
        );
        assert_eq!(v["summary"]["severities"]["high"], 1);
        assert_eq!(v["findings"].as_array().unwrap().len(), 1);
        assert_eq!(v["unavailable"].as_array().unwrap().len(), 1);
        assert_eq!(v["results"].as_array().unwrap().len(), 3);
        assert_eq!(v["results"][0]["severity"], "high");
        assert!(v["results"][1].get("severity").is_none());
        // Deduplicated limitations include check and evidence limitations.
        let lims = v["limitations"].as_array().unwrap();
        assert_eq!(lims.len(), 2);
        // Only known evidence keys reach the report.
        assert_eq!(v["infrastructure"]["resources"]["memoryTotalKb"], 1000);
        let fs = &v["infrastructure"]["filesystems"][0];
        assert_eq!(fs["mount"], "/");
        assert!(fs.get("secret").is_none());
        let all = v.to_string();
        assert!(!all.contains("unknownKey") && !all.contains("username"));
        assert!(all.contains("not anonymized") || all.contains("included in this report"));
        assert!(
            v["notes"]["contents"]
                .as_str()
                .unwrap()
                .contains("Review the report")
        );
    }

    #[test]
    fn complete_scan_is_reported_complete_even_with_unassessed_checks() {
        let mut i = input();
        i.steps = OPERATIONS
            .iter()
            .map(|o| step(o.id, vec![check(Outcome::Unavailable, None)], Value::Null))
            .collect();
        i.scan.completed_steps = OPERATIONS.iter().map(|o| o.id.to_owned()).collect();
        let v: Value = serde_json::from_str(&build_json(&i, T).unwrap()).unwrap();
        assert_eq!(v["scan"]["status"], "complete");
        assert_eq!(v["scan"]["coverage"]["assessed"], 0);
        assert_eq!(v["scan"]["coverage"]["notAssessed"], 8);
        assert!(v["scan"]["missingSteps"].as_array().unwrap().is_empty());
    }

    #[test]
    fn two_builds_differ_only_in_generated_at_and_step_order_is_canonical() {
        let i = input();
        let mut reversed = input();
        reversed.steps.reverse();
        let a = build_json(&i, "A").unwrap();
        let b = build_json(&reversed, "B").unwrap();
        assert_eq!(a.replace("\"A\"", "\"B\""), b);
        assert_eq!(build_json(&i, "A").unwrap(), a);
        let ha = build_html(&i, "AAAA").unwrap();
        let hb = build_html(&reversed, "BBBB").unwrap();
        assert_eq!(ha.replace("AAAA", "BBBB"), hb);
    }

    #[test]
    fn secret_like_text_outside_emitted_fields_is_not_written() {
        let mut i = input();
        i.steps[1].evidence = json!({
            "password": "hunter2-secret",
            "filesystems": [{"mount": "/", "totalBytes": 1, "usedBytes": 1,
                             "availableBytes": 0, "token": "hunter2-secret"}],
        });
        let json = build_json(&i, T).unwrap();
        let html = build_html(&i, T).unwrap();
        assert!(!json.contains("hunter2-secret") && !html.contains("hunter2-secret"));
    }

    #[test]
    fn html_has_the_required_sections_in_order() {
        let html = build_html(&input(), T).unwrap();
        let mut at = 0;
        for h in [
            "<h2>Server</h2>",
            "<h2>Summary</h2>",
            "<h2>Scan status and coverage</h2>",
            "<h2>Findings</h2>",
            "<h2>Unavailable, incomplete and failed checks</h2>",
            "<h2>Infrastructure summary</h2>",
            "<h2>Limitations</h2>",
        ] {
            let p = html[at..].find(h).unwrap_or_else(|| panic!("{h}"));
            at += p;
        }
        assert!(!html.contains("<script") && !html.contains("<link"));
    }
}
