//! Result model: one typed shape for every check, step and summary.
//! There is no security score.

use serde::{Deserialize, Serialize};

/// Evidence lines per check.
pub const MAX_EVIDENCE_LINES: usize = 10;
/// Characters per evidence line.
pub const MAX_EVIDENCE_CHARS: usize = 200;
/// Check results per step.
pub const MAX_CHECKS_PER_STEP: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Outcome {
    Passed,
    Finding,
    /// No usable evidence (permission denied, file missing).
    Unavailable,
    /// Some evidence, but truncated, skipped or limited.
    Incomplete,
    NotApplicable,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    Resources,
    Services,
    Network,
    Security,
    Updates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub rule_id: &'static str,
    pub title: String,
    pub category: Category,
    pub outcome: Outcome,
    /// Set only when `outcome` is `finding`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub severity: Option<Severity>,
    pub resource: Option<String>,
    pub evidence: Vec<String>,
    pub explanation: String,
    /// Advice text only. Never executed.
    pub recommendation: Option<String>,
    pub observed_at: String,
    pub limitations: Vec<String>,
    /// True when the conclusion goes beyond observed facts.
    pub inferred: bool,
}

impl CheckResult {
    /// A result that is not a finding. Use [`CheckResult::finding`] for findings.
    pub fn new(
        rule_id: &'static str,
        category: Category,
        outcome: Outcome,
        title: impl Into<String>,
        explanation: impl Into<String>,
        observed_at: &str,
    ) -> Self {
        debug_assert!(outcome != Outcome::Finding, "use CheckResult::finding");
        Self {
            rule_id,
            title: title.into(),
            category,
            outcome,
            severity: None,
            resource: None,
            evidence: Vec::new(),
            explanation: explanation.into(),
            recommendation: None,
            observed_at: observed_at.to_owned(),
            limitations: Vec::new(),
            inferred: false,
        }
    }

    pub fn finding(
        rule_id: &'static str,
        category: Category,
        severity: Severity,
        title: impl Into<String>,
        explanation: impl Into<String>,
        observed_at: &str,
    ) -> Self {
        let mut c = Self::new(
            rule_id,
            category,
            Outcome::Passed,
            title,
            explanation,
            observed_at,
        );
        c.outcome = Outcome::Finding;
        c.severity = Some(severity);
        c
    }

    pub fn resource(mut self, resource: impl AsRef<str>) -> Self {
        self.resource = Some(clean_line(resource.as_ref()));
        self
    }

    pub fn recommendation(mut self, text: impl Into<String>) -> Self {
        self.recommendation = Some(text.into());
        self
    }

    pub fn limitation(mut self, text: impl Into<String>) -> Self {
        self.limitations.push(text.into());
        self
    }

    pub fn inferred(mut self) -> Self {
        self.inferred = true;
        self
    }

    /// Sets the evidence. Control characters are removed, lines are cut to
    /// [`MAX_EVIDENCE_CHARS`] and at most [`MAX_EVIDENCE_LINES`] are kept. A
    /// limitation is added when anything was cut.
    pub fn evidence<I, S>(mut self, lines: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut cut = false;
        self.evidence.clear();
        for line in lines {
            if self.evidence.len() == MAX_EVIDENCE_LINES {
                cut = true;
                break;
            }
            let raw = line.as_ref();
            let cleaned = clean_line(raw);
            if cleaned.chars().count() < raw.chars().filter(|c| !c.is_control()).count() {
                cut = true;
            }
            self.evidence.push(cleaned);
        }
        if cut {
            self.limitations.push("Evidence was shortened.".to_owned());
        }
        self
    }
}

/// Removes control characters and cuts to [`MAX_EVIDENCE_CHARS`] characters.
fn clean_line(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .take(MAX_EVIDENCE_CHARS)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepResult {
    /// Operation id of the step.
    pub step: String,
    pub observed_at: String,
    /// Typed per step. `null` until the step collects evidence.
    pub evidence: serde_json::Value,
    pub checks: Vec<CheckResult>,
}

impl StepResult {
    /// Keeps at most [`MAX_CHECKS_PER_STEP`] checks. When more were produced,
    /// the last slot becomes an `incomplete` result so the cut is visible.
    pub fn new(
        step: &str,
        observed_at: &str,
        evidence: serde_json::Value,
        mut checks: Vec<CheckResult>,
    ) -> Self {
        if checks.len() > MAX_CHECKS_PER_STEP {
            let category = checks[0].category;
            checks.truncate(MAX_CHECKS_PER_STEP - 1);
            checks.push(CheckResult::new(
                "step.check-limit",
                category,
                Outcome::Incomplete,
                "Check limit reached",
                format!(
                    "More than {MAX_CHECKS_PER_STEP} results were produced; the rest are not shown."
                ),
                observed_at,
            ));
        }
        Self {
            step: step.to_owned(),
            observed_at: observed_at.to_owned(),
            evidence,
            checks,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutcomeCounts {
    pub passed: u32,
    pub finding: u32,
    pub unavailable: u32,
    pub incomplete: u32,
    pub not_applicable: u32,
    pub error: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeverityCounts {
    pub critical: u32,
    pub high: u32,
    pub medium: u32,
    pub low: u32,
    pub info: u32,
}

/// How much was actually assessed. Separate from [`ScanStatus`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    /// `passed` + `finding`.
    pub assessed: u32,
    /// `unavailable` + `incomplete` + `error`.
    pub not_assessed: u32,
    pub not_applicable: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub outcomes: OutcomeCounts,
    pub severities: SeverityCounts,
    pub coverage: Coverage,
}

impl Summary {
    pub fn from_checks<'a>(checks: impl IntoIterator<Item = &'a CheckResult>) -> Self {
        let mut s = Summary::default();
        for c in checks {
            s.add(c.outcome, c.severity);
        }
        s
    }

    /// Counts one result. The severity counts only for `finding`.
    pub fn add(&mut self, outcome: Outcome, severity: Option<Severity>) {
        let o = &mut self.outcomes;
        let cov = &mut self.coverage;
        match outcome {
            Outcome::Passed => {
                o.passed += 1;
                cov.assessed += 1;
            }
            Outcome::Finding => {
                o.finding += 1;
                cov.assessed += 1;
            }
            Outcome::Unavailable => {
                o.unavailable += 1;
                cov.not_assessed += 1;
            }
            Outcome::Incomplete => {
                o.incomplete += 1;
                cov.not_assessed += 1;
            }
            Outcome::Error => {
                o.error += 1;
                cov.not_assessed += 1;
            }
            Outcome::NotApplicable => {
                o.not_applicable += 1;
                cov.not_applicable += 1;
            }
        }
        if let (Outcome::Finding, Some(sev)) = (outcome, severity) {
            let v = &mut self.severities;
            match sev {
                Severity::Critical => v.critical += 1,
                Severity::High => v.high += 1,
                Severity::Medium => v.medium += 1,
                Severity::Low => v.low += 1,
                Severity::Info => v.info += 1,
            }
        }
    }
}

/// Whether every step ran to the end. Says nothing about coverage: a
/// `complete` scan can still contain unassessed checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ScanStatus {
    Complete,
    Incomplete,
}

impl ScanStatus {
    /// `Complete` only when every expected step is in `completed`.
    pub fn of(expected: &[&str], completed: &[&str]) -> Self {
        if expected.iter().all(|s| completed.contains(s)) {
            ScanStatus::Complete
        } else {
            ScanStatus::Incomplete
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: &str = "2025-01-01T00:00:00Z";

    fn check(outcome: Outcome) -> CheckResult {
        CheckResult::new("t.rule", Category::Security, outcome, "t", "e", T)
    }

    #[test]
    fn serializes_camel_case_and_omits_severity_unless_finding() {
        let v = serde_json::to_value(check(Outcome::NotApplicable)).unwrap();
        assert_eq!(v["ruleId"], "t.rule");
        assert_eq!(v["outcome"], "notApplicable");
        assert_eq!(v["observedAt"], T);
        assert!(v.get("severity").is_none());
        assert_eq!(v["inferred"], false);

        let f = CheckResult::finding("t.rule", Category::Network, Severity::High, "t", "e", T);
        let v = serde_json::to_value(f).unwrap();
        assert_eq!(v["outcome"], "finding");
        assert_eq!(v["severity"], "high");
        assert_eq!(v["category"], "network");
    }

    #[test]
    fn summary_counts_outcomes_severities_and_coverage() {
        let checks = vec![
            check(Outcome::Passed),
            check(Outcome::Passed),
            CheckResult::finding("a", Category::Network, Severity::Low, "t", "e", T),
            CheckResult::finding("b", Category::Network, Severity::High, "t", "e", T),
            check(Outcome::Unavailable),
            check(Outcome::Incomplete),
            check(Outcome::Error),
            check(Outcome::NotApplicable),
        ];
        let s = Summary::from_checks(&checks);
        assert_eq!(s.outcomes.passed, 2);
        assert_eq!(s.outcomes.finding, 2);
        assert_eq!(s.outcomes.unavailable, 1);
        assert_eq!(s.outcomes.incomplete, 1);
        assert_eq!(s.outcomes.error, 1);
        assert_eq!(s.outcomes.not_applicable, 1);
        assert_eq!((s.severities.low, s.severities.high), (1, 1));
        assert_eq!(
            s.coverage,
            Coverage {
                assessed: 4,
                not_assessed: 3,
                not_applicable: 1
            }
        );
    }

    #[test]
    fn scan_status_is_separate_from_coverage() {
        let expected = ["a", "b"];
        assert_eq!(ScanStatus::of(&expected, &["a", "b"]), ScanStatus::Complete);
        assert_eq!(ScanStatus::of(&expected, &["a"]), ScanStatus::Incomplete);
        // A complete scan with an unassessed check is still not a clean result.
        let s = Summary::from_checks(&[check(Outcome::Unavailable)]);
        assert_eq!(s.coverage.assessed, 0);
        assert_eq!(s.coverage.not_assessed, 1);
    }

    #[test]
    fn evidence_is_limited_and_stripped() {
        let long = "x".repeat(500);
        let lines: Vec<String> = (0..15).map(|i| format!("line {i}\u{1b}[31m\n")).collect();
        let c = check(Outcome::Passed).evidence(lines);
        assert_eq!(c.evidence.len(), MAX_EVIDENCE_LINES);
        assert!(c.evidence.iter().all(|l| !l.chars().any(char::is_control)));
        assert_eq!(c.evidence[0], "line 0[31m");
        assert!(!c.limitations.is_empty());

        let c = check(Outcome::Passed).evidence([long]);
        assert_eq!(c.evidence[0].chars().count(), MAX_EVIDENCE_CHARS);
        assert!(!c.limitations.is_empty());

        let c = check(Outcome::Passed).evidence(["short"]);
        assert!(c.limitations.is_empty());
    }

    #[test]
    fn step_keeps_at_most_100_checks_and_marks_the_cut_incomplete() {
        let checks: Vec<CheckResult> = (0..150).map(|_| check(Outcome::Passed)).collect();
        let step = StepResult::new("ssh", T, serde_json::Value::Null, checks);
        assert_eq!(step.checks.len(), MAX_CHECKS_PER_STEP);
        assert_eq!(step.checks[99].outcome, Outcome::Incomplete);
        assert_eq!(step.checks[98].outcome, Outcome::Passed);
    }
}
