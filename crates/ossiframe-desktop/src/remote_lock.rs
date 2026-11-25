//! Remote exclusion lock for provisioning operations.
//!
//! The lock is the transient systemd unit `ossiframe-provision-lock`. systemd
//! refuses to create a second unit with an existing name, so acquisition needs
//! no lock file. The unit belongs to PID 1 and outlives the SSH session and
//! the desktop app. Ownership is recorded in the unit description.
//!
//! The lock is cooperative: it only excludes other Ossiframe instances that
//! use the same unit name. It does not stop a person or another tool from
//! running apt (apt and dpkg keep their own locks). A reboot clears it.
//!
//! The description read back from a server is untrusted. It is parsed strictly
//! and is used only for display and for comparing ownership.
use ossiframe_core::command::CommandSpec;
use ossiframe_core::executor::{CancelToken, ExecError, ExecResult, Executor};
use std::time::Duration;
use uuid::Uuid;

/// Unit name, without the `.service` suffix systemd adds.
pub const LOCK_UNIT: &str = "ossiframe-provision-lock";
const LOCK_UNIT_FULL: &str = "ossiframe-provision-lock.service";
/// Fixed application id written into every record this app creates.
pub const APP_ID: &str = "dev.ossiframe.deploy";

const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const SLEEP: &str = "/usr/bin/sleep";
const CMD_TIMEOUT: Duration = Duration::from_secs(20);
const RECORD_PREFIX: &str = "ossiframe-lock";
const MAX_RECORD_LEN: usize = 256;
const MAX_SHOW_LEN: usize = 4096;
const DISPLAY_LEN: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LockError {
    #[error("invalid lock record: {0}")]
    InvalidRecord(&'static str),
    #[error("command failed: {0}")]
    Exec(#[from] ExecError),
    #[error("the lock state could not be classified: {0}")]
    Unclassified(String),
    #[error("the lock is not owned by this operation")]
    NotOwner(Box<HolderInfo>),
    #[error("stale release refused: {0:?}")]
    StaleRefused(StaleRefusal),
    #[error("the lock holder changed since it was inspected")]
    HolderChanged,
    #[error("stopping the lock holder failed")]
    StopFailed,
}

/// Parsed ownership record. Every field is validated; none is trusted beyond
/// display and equality comparison.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnerRecord {
    operation_id: String,
    app: String,
    desktop: String,
    started: String,
}

impl OwnerRecord {
    /// Builds a record for this app from validated parts.
    pub fn new(operation_id: &str, desktop: &str, started: &str) -> Result<Self, LockError> {
        Self::from_parts(operation_id, APP_ID, desktop, started)
    }

    /// Same as [`OwnerRecord::new`] with the current UTC time.
    pub fn new_now(operation_id: &str, desktop: &str) -> Result<Self, LockError> {
        use time::format_description::well_known::Rfc3339;
        let now = time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .map_err(|_| LockError::InvalidRecord("clock"))?;
        Self::new(operation_id, desktop, &now)
    }

    fn from_parts(op: &str, app: &str, desktop: &str, started: &str) -> Result<Self, LockError> {
        if !valid_uuid(op) {
            return Err(LockError::InvalidRecord("operation id"));
        }
        if !valid_app(app) {
            return Err(LockError::InvalidRecord("app id"));
        }
        if !valid_desktop(desktop) {
            return Err(LockError::InvalidRecord("desktop id"));
        }
        if !valid_rfc3339(started) {
            return Err(LockError::InvalidRecord("start time"));
        }
        Ok(Self {
            operation_id: op.to_owned(),
            app: app.to_owned(),
            desktop: desktop.to_owned(),
            started: started.to_owned(),
        })
    }

    /// The description text stored on the unit.
    pub fn render(&self) -> String {
        format!(
            "{RECORD_PREFIX} op={} app={} desktop={} started={}",
            self.operation_id, self.app, self.desktop, self.started
        )
    }

    /// Strict parse: bounded, ASCII, fixed field order, single spaces.
    pub fn parse(text: &str) -> Result<Self, LockError> {
        if text.len() > MAX_RECORD_LEN || !text.is_ascii() {
            return Err(LockError::InvalidRecord("size or encoding"));
        }
        let parts: Vec<&str> = text.split(' ').collect();
        if parts.len() != 5 || parts[0] != RECORD_PREFIX {
            return Err(LockError::InvalidRecord("shape"));
        }
        let field = |part: &str, key: &str| -> Result<String, LockError> {
            part.strip_prefix(key)
                .map(str::to_owned)
                .ok_or(LockError::InvalidRecord("field order"))
        };
        Self::from_parts(
            &field(parts[1], "op=")?,
            &field(parts[2], "app=")?,
            &field(parts[3], "desktop=")?,
            &field(parts[4], "started=")?,
        )
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn desktop(&self) -> &str {
        &self.desktop
    }

    pub fn started(&self) -> &str {
        &self.started
    }

    /// True when `other` names the same operation, app and desktop. The start
    /// time is not compared.
    pub fn same_owner(&self, other: &OwnerRecord) -> bool {
        self.operation_id == other.operation_id
            && self.app == other.app
            && self.desktop == other.desktop
    }
}

fn valid_uuid(s: &str) -> bool {
    s.len() == 36
        && Uuid::parse_str(s)
            .map(|u| u.hyphenated().to_string() == s)
            .unwrap_or(false)
}

fn valid_app(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-')
}

fn valid_desktop(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `YYYY-MM-DDTHH:MM:SS[.f{1,9}](Z|+hh:mm|-hh:mm)`, with range checks.
fn valid_rfc3339(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() < 20 || b.len() > 35 {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    let num = |r: std::ops::Range<usize>| s[r].parse::<u32>().unwrap_or(u32::MAX);
    if !(digits(0..4)
        && b[4] == b'-'
        && digits(5..7)
        && b[7] == b'-'
        && digits(8..10)
        && b[10] == b'T'
        && digits(11..13)
        && b[13] == b':'
        && digits(14..16)
        && b[16] == b':'
        && digits(17..19))
    {
        return false;
    }
    if !(1..=12).contains(&num(5..7))
        || !(1..=31).contains(&num(8..10))
        || num(11..13) > 23
        || num(14..16) > 59
        || num(17..19) > 60
    {
        return false;
    }
    let mut i = 19;
    if b[i] == b'.' {
        let start = i + 1;
        i = start;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if !(1..=9).contains(&(i - start)) {
            return false;
        }
    }
    match &b[i..] {
        [b'Z'] => true,
        [b'+' | b'-', ..] if b.len() - i == 6 => {
            digits(i + 1..i + 3)
                && b[i + 3] == b':'
                && digits(i + 4..i + 6)
                && num(i + 1..i + 3) <= 23
                && num(i + 4..i + 6) <= 59
        }
        _ => false,
    }
}

/// What `systemctl show` reported for the lock unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HolderInfo {
    /// Parsed record, or `None` when the description is not one of ours.
    pub owner: Option<OwnerRecord>,
    /// Bounded printable-ASCII copy of the description, for display only.
    pub description: String,
    pub active_state: String,
    pub sub_state: String,
}

impl HolderInfo {
    /// True when the record names exactly this owner.
    pub fn is_owned_by(&self, record: &OwnerRecord) -> bool {
        self.owner.as_ref().is_some_and(|o| o.same_owner(record))
    }
}

/// Current state of the lock unit on the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockState {
    Free,
    Held(HolderInfo),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireOutcome {
    /// This call started the holder.
    Acquired,
    /// The holder already belongs to the same operation (for example after a
    /// reconnect). Nothing was started.
    AlreadyOwned(HolderInfo),
    /// Another owner, or an unreadable holder, has the lock. Nothing was
    /// changed.
    Busy(HolderInfo),
    /// The start failed and no holder exists (no sudo, no systemd-run).
    /// The engine treats this as a blocker.
    Unavailable(String),
}

fn sanitize(s: &str) -> String {
    s.chars()
        .take(DISPLAY_LEN)
        .map(|c| {
            if c.is_ascii_graphic() || c == ' ' {
                c
            } else {
                '?'
            }
        })
        .collect()
}

fn short_state(s: &str) -> String {
    let ok =
        !s.is_empty() && s.len() <= 32 && s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    if ok {
        s.to_owned()
    } else {
        "unknown".to_owned()
    }
}

/// Parses `systemctl show --property=Description,ActiveState,SubState,LoadState`.
/// Returns `None` when the unit is not loaded.
pub fn parse_show(output: &str) -> Result<Option<HolderInfo>, LockError> {
    if output.len() > MAX_SHOW_LEN {
        return Err(LockError::Unclassified("output too large".into()));
    }
    let (mut desc, mut active, mut sub, mut load) = (None, None, None, None);
    for line in output.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k {
                "Description" => desc = Some(v),
                "ActiveState" => active = Some(v),
                "SubState" => sub = Some(v),
                "LoadState" => load = Some(v),
                _ => {}
            }
        }
    }
    match load {
        Some("not-found") => Ok(None),
        Some("loaded") => {
            let description =
                desc.ok_or_else(|| LockError::Unclassified("no description".into()))?;
            Ok(Some(HolderInfo {
                owner: OwnerRecord::parse(description).ok(),
                description: sanitize(description),
                active_state: short_state(active.unwrap_or("")),
                sub_state: short_state(sub.unwrap_or("")),
            }))
        }
        _ => Err(LockError::Unclassified("unexpected LoadState".into())),
    }
}

// ---- commands -----------------------------------------------------------

fn root(program: &str, args: &[&str]) -> CommandSpec {
    CommandSpec::privileged(program, args, CMD_TIMEOUT).expect("fixed lock command is valid")
}

/// `sudo -n systemd-run` that starts the holder unit.
pub fn acquire_command(record: &OwnerRecord) -> CommandSpec {
    let unit = format!("--unit={LOCK_UNIT}");
    let description = format!("--description={}", record.render());
    root(
        SYSTEMD_RUN,
        &[
            &unit,
            "--service-type=exec",
            "--quiet",
            &description,
            "--",
            SLEEP,
            "infinity",
        ],
    )
}

/// Unprivileged read of the holder state.
pub fn show_command() -> CommandSpec {
    CommandSpec::new(
        SYSTEMCTL,
        &[
            "show",
            "--property=Description,ActiveState,SubState,LoadState",
            LOCK_UNIT_FULL,
        ],
        CMD_TIMEOUT,
    )
    .expect("fixed lock command is valid")
}

fn stop_command() -> CommandSpec {
    root(SYSTEMCTL, &["stop", LOCK_UNIT_FULL])
}

fn reset_failed_command() -> CommandSpec {
    root(SYSTEMCTL, &["reset-failed", LOCK_UNIT_FULL])
}

// ---- operations ---------------------------------------------------------

async fn run(
    exec: &dyn Executor,
    spec: &CommandSpec,
    cancel: &CancelToken,
) -> Result<ExecResult, LockError> {
    Ok(exec.run(spec, cancel).await?)
}

/// Reads the lock unit. A transport failure is an error, never `Free`.
pub async fn inspect(exec: &dyn Executor, cancel: &CancelToken) -> Result<LockState, LockError> {
    let r = run(exec, &show_command(), cancel).await?;
    if !r.exit.success() {
        return Err(LockError::Unclassified("systemctl show failed".into()));
    }
    Ok(match parse_show(&r.stdout)? {
        None => LockState::Free,
        Some(h) => LockState::Held(h),
    })
}

/// Tries to start the holder. A transport error (timeout, lost connection)
/// is returned as `Err`: the unit may or may not exist. Call [`inspect`]
/// after reconnecting to find out; it never starts anything.
pub async fn acquire(
    exec: &dyn Executor,
    cancel: &CancelToken,
    record: &OwnerRecord,
) -> Result<AcquireOutcome, LockError> {
    let started = run(exec, &acquire_command(record), cancel).await?;
    if started.exit.success() {
        return Ok(AcquireOutcome::Acquired);
    }
    match inspect(exec, cancel).await? {
        LockState::Held(h) if h.is_owned_by(record) => Ok(AcquireOutcome::AlreadyOwned(h)),
        LockState::Held(h) => Ok(AcquireOutcome::Busy(h)),
        LockState::Free => Ok(AcquireOutcome::Unavailable(
            "the lock unit could not be started (systemd-run or sudo unavailable)".into(),
        )),
    }
}

/// Releases the lock only when the record names `record`'s owner. Any other
/// holder is left untouched and returned in `NotOwner`. Releasing a free lock
/// is a no-op.
pub async fn release_own(
    exec: &dyn Executor,
    cancel: &CancelToken,
    record: &OwnerRecord,
) -> Result<(), LockError> {
    match inspect(exec, cancel).await? {
        LockState::Free => Ok(()),
        LockState::Held(h) if h.is_owned_by(record) => stop_holder(exec, cancel).await,
        LockState::Held(h) => Err(LockError::NotOwner(Box::new(h))),
    }
}

async fn stop_holder(exec: &dyn Executor, cancel: &CancelToken) -> Result<(), LockError> {
    let stopped = run(exec, &stop_command(), cancel).await?;
    // reset-failed only clears a failed unit; failure here is not an error.
    let _ = run(exec, &reset_failed_command(), cancel).await;
    if stopped.exit.success() {
        Ok(())
    } else {
        Err(LockError::StopFailed)
    }
}

/// One input of the stale-release decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    Clear,
    Present,
    /// Could not be determined. Never counts as clear.
    Unknown,
}

/// The backend's own fresh evidence for a stale release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaleEvidence {
    /// No active `ossiframe-op-*` step unit.
    pub step_units: Evidence,
    /// No apt or dpkg process.
    pub apt_processes: Evidence,
    /// apt and dpkg lock files are free.
    pub lock_files: Evidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleRefusal {
    /// The holder belongs to a non-terminal local operation (including
    /// UnknownOutcome). That is resolved through mark-resolved.
    LocalOperationActive,
    StepUnits,
    AptProcesses,
    LockFiles,
}

/// Pure gate for the explicit stale-release action.
pub fn check_stale_release(
    local_operation_active: bool,
    e: &StaleEvidence,
) -> Result<(), StaleRefusal> {
    if local_operation_active {
        return Err(StaleRefusal::LocalOperationActive);
    }
    if e.step_units != Evidence::Clear {
        return Err(StaleRefusal::StepUnits);
    }
    if e.apt_processes != Evidence::Clear {
        return Err(StaleRefusal::AptProcesses);
    }
    if e.lock_files != Evidence::Clear {
        return Err(StaleRefusal::LockFiles);
    }
    Ok(())
}

/// Releases a holder the user has seen (`seen_description`) after the gate
/// passes. The holder is read again first; if its description differs from
/// what the user saw, nothing is stopped. The caller records the decision and
/// evidence in `operation_events`.
pub async fn release_stale(
    exec: &dyn Executor,
    cancel: &CancelToken,
    seen_description: &str,
    local_operation_active: bool,
    evidence: &StaleEvidence,
) -> Result<(), LockError> {
    check_stale_release(local_operation_active, evidence).map_err(LockError::StaleRefused)?;
    match inspect(exec, cancel).await? {
        LockState::Free => Ok(()),
        LockState::Held(h) if h.description == sanitize(seen_description) => {
            stop_holder(exec, cancel).await
        }
        LockState::Held(_) => Err(LockError::HolderChanged),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ossiframe_core::command::Privilege;
    use ossiframe_core::executor::ExitStatus;
    use std::sync::Mutex;

    const OP: &str = "0b0c5a5e-1d8e-4a54-9a43-0d1c2f3a4b5c";
    const OTHER_OP: &str = "11111111-2222-4333-8444-555555555555";
    const DESKTOP: &str = "0123456789abcdef0123456789abcdef";
    const STARTED: &str = "2026-01-02T03:04:05Z";

    fn rec(op: &str) -> OwnerRecord {
        OwnerRecord::new(op, DESKTOP, STARTED).unwrap()
    }

    fn out(code: i32, stdout: &str) -> Result<ExecResult, ExecError> {
        Ok(ExecResult {
            exit: ExitStatus::Code(code),
            stdout: stdout.into(),
            stderr: String::new(),
            truncated: false,
        })
    }

    fn show_out(desc: &str, active: &str) -> String {
        format!("Description={desc}\nActiveState={active}\nSubState=running\nLoadState=loaded\n")
    }

    const NOT_FOUND: &str = "Description=ossiframe-provision-lock.service\nActiveState=inactive\nSubState=dead\nLoadState=not-found\n";

    /// Fake that answers by subcommand and logs every command line.
    struct Fake {
        log: Mutex<Vec<String>>,
        start: Result<ExecResult, ExecError>,
        show: Mutex<Vec<Result<ExecResult, ExecError>>>,
    }

    impl Fake {
        fn new(
            start: Result<ExecResult, ExecError>,
            show: Vec<Result<ExecResult, ExecError>>,
        ) -> Self {
            Self {
                log: Mutex::new(vec![]),
                start,
                show: Mutex::new(show),
            }
        }
        fn lines(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
        fn mutating(&self) -> usize {
            self.lines().iter().filter(|l| l.contains("sudo")).count()
        }
    }

    #[async_trait]
    impl Executor for Fake {
        async fn run(&self, spec: &CommandSpec, _: &CancelToken) -> Result<ExecResult, ExecError> {
            self.log.lock().unwrap().push(spec.to_command_line());
            match spec.args().first().map(String::as_str) {
                Some("show") => {
                    let mut q = self.show.lock().unwrap();
                    if q.len() > 1 {
                        q.remove(0)
                    } else {
                        q[0].clone()
                    }
                }
                Some("stop") | Some("reset-failed") => out(0, ""),
                _ => self.start.clone(),
            }
        }
    }

    #[test]
    fn record_round_trip_and_rendering() {
        let r = rec(OP);
        let text = r.render();
        assert_eq!(
            text,
            format!("ossiframe-lock op={OP} app={APP_ID} desktop={DESKTOP} started={STARTED}")
        );
        assert_eq!(OwnerRecord::parse(&text).unwrap(), r);
        assert!(OwnerRecord::new_now(OP, DESKTOP).is_ok());
        let frac = "2026-01-02T03:04:05.123456+02:00";
        assert!(OwnerRecord::new(OP, DESKTOP, frac).is_ok());
    }

    #[test]
    fn hostile_or_malformed_records_are_rejected() {
        let good = rec(OP).render();
        let bad = [
            good.replace(OP, "not-a-uuid"),
            good.replace(OP, &OP.to_uppercase()),
            good.replace(DESKTOP, "xyz"),
            good.replace(STARTED, "yesterday"),
            good.replace(STARTED, "2026-13-02T03:04:05Z"),
            format!("{good} extra=1"),
            good.replace(' ', "  "),
            good.replace("op=", "OP="),
            good.replace("ossiframe-lock", "other"),
            good.replace(APP_ID, "App;rm"),
            good.replace(STARTED, "2026-01-02T03:04:05Z\n"),
            format!("{good}\u{e9}"),
            "x".repeat(10_000),
            String::new(),
        ];
        for b in bad {
            assert!(OwnerRecord::parse(&b).is_err(), "accepted: {b:?}");
        }
        // Building from hostile parts is refused too.
        assert!(OwnerRecord::new("a b", DESKTOP, STARTED).is_err());
        assert!(OwnerRecord::new(OP, DESKTOP, "2026-01-02T03:04:05Z %n").is_err());
    }

    #[test]
    fn show_parsing_is_bounded_and_display_is_sanitized() {
        assert_eq!(parse_show(NOT_FOUND).unwrap(), None);
        let h = parse_show(&show_out(&rec(OP).render(), "active"))
            .unwrap()
            .unwrap();
        assert!(h.is_owned_by(&rec(OP)));
        assert_eq!(h.active_state, "active");
        let hostile = parse_show(&show_out("evil\u{1b}[31m desc", "ACTIVE;x"))
            .unwrap()
            .unwrap();
        assert!(hostile.owner.is_none());
        assert!(!hostile.description.contains('\u{1b}'));
        assert_eq!(hostile.active_state, "unknown");
        assert!(parse_show(&"a".repeat(MAX_SHOW_LEN + 1)).is_err());
        assert!(parse_show("LoadState=masked\nDescription=x\n").is_err());
    }

    #[test]
    fn commands_use_absolute_paths_fixed_argv_and_no_shell() {
        let r = rec(OP);
        let a = acquire_command(&r);
        assert_eq!(a.privilege(), Privilege::Root);
        assert_eq!(a.program(), "/usr/bin/systemd-run");
        let desc = format!("--description={}", r.render());
        assert_eq!(
            a.args(),
            [
                "--unit=ossiframe-provision-lock",
                "--service-type=exec",
                "--quiet",
                desc.as_str(),
                "--",
                "/usr/bin/sleep",
                "infinity"
            ]
        );
        let line = a.to_command_line();
        assert!(line.starts_with("'/usr/bin/sudo' '-n' '--' '/usr/bin/systemd-run' "));
        let s = show_command();
        assert_eq!(s.privilege(), Privilege::User);
        assert_eq!(s.program(), "/usr/bin/systemctl");
        for c in [stop_command(), reset_failed_command()] {
            assert_eq!(c.privilege(), Privilege::Root);
            assert!(c.program().starts_with('/'));
            assert_eq!(c.args().last().unwrap(), "ossiframe-provision-lock.service");
        }
    }

    #[tokio::test]
    async fn acquire_success_runs_one_start_command() {
        let f = Fake::new(out(0, ""), vec![out(0, NOT_FOUND)]);
        let r = acquire(&f, &CancelToken::new(), &rec(OP)).await.unwrap();
        assert_eq!(r, AcquireOutcome::Acquired);
        assert_eq!(f.lines().len(), 1);
    }

    #[tokio::test]
    async fn already_held_by_other_owner_is_busy_and_nothing_is_removed() {
        let other = rec(OTHER_OP).render();
        let f = Fake::new(out(1, ""), vec![out(0, &show_out(&other, "active"))]);
        match acquire(&f, &CancelToken::new(), &rec(OP)).await.unwrap() {
            AcquireOutcome::Busy(h) => assert_eq!(h.owner.unwrap().operation_id(), OTHER_OP),
            other => panic!("{other:?}"),
        }
        // Only the failed start; no stop or reset-failed.
        assert_eq!(f.mutating(), 1);
        assert!(f.lines().iter().all(|l| !l.contains("'stop'")));
    }

    #[tokio::test]
    async fn own_lock_is_redetected_after_reconnect() {
        let mine = rec(OP).render();
        // Lost connection during start: unknown, surfaced as an error.
        let f = Fake::new(
            Err(ExecError::ConnectionLost),
            vec![out(0, &show_out(&mine, "active"))],
        );
        let e = acquire(&f, &CancelToken::new(), &rec(OP))
            .await
            .unwrap_err();
        assert_eq!(e, LockError::Exec(ExecError::ConnectionLost));
        // After reconnecting, a read-only inspect finds the holder is ours.
        match inspect(&f, &CancelToken::new()).await.unwrap() {
            LockState::Held(h) => assert!(h.is_owned_by(&rec(OP))),
            s => panic!("{s:?}"),
        }
        // A retried start that fails because it already exists reports it.
        let g = Fake::new(out(1, ""), vec![out(0, &show_out(&mine, "active"))]);
        assert!(matches!(
            acquire(&g, &CancelToken::new(), &rec(OP)).await.unwrap(),
            AcquireOutcome::AlreadyOwned(_)
        ));
    }

    #[tokio::test]
    async fn unavailable_and_unreadable_results_are_classified() {
        let f = Fake::new(out(1, ""), vec![out(0, NOT_FOUND)]);
        assert!(matches!(
            acquire(&f, &CancelToken::new(), &rec(OP)).await.unwrap(),
            AcquireOutcome::Unavailable(_)
        ));
        let g = Fake::new(out(1, ""), vec![out(1, "")]);
        assert!(matches!(
            acquire(&g, &CancelToken::new(), &rec(OP))
                .await
                .unwrap_err(),
            LockError::Unclassified(_)
        ));
        // A holder with a foreign description is busy with no owner.
        let h = Fake::new(
            out(1, ""),
            vec![out(0, &show_out("something else", "active"))],
        );
        match acquire(&h, &CancelToken::new(), &rec(OP)).await.unwrap() {
            AcquireOutcome::Busy(i) => assert!(i.owner.is_none()),
            o => panic!("{o:?}"),
        }
    }

    #[tokio::test]
    async fn release_only_when_owned() {
        let mine = rec(OP).render();
        let f = Fake::new(out(0, ""), vec![out(0, &show_out(&mine, "active"))]);
        release_own(&f, &CancelToken::new(), &rec(OP))
            .await
            .unwrap();
        let l = f.lines();
        assert!(l.iter().any(|x| x.contains("'stop'")));
        assert!(l.iter().any(|x| x.contains("'reset-failed'")));

        let theirs = rec(OTHER_OP).render();
        let g = Fake::new(out(0, ""), vec![out(0, &show_out(&theirs, "active"))]);
        let e = release_own(&g, &CancelToken::new(), &rec(OP))
            .await
            .unwrap_err();
        assert!(matches!(e, LockError::NotOwner(_)));
        assert_eq!(g.mutating(), 0);

        // Same operation id from a different desktop is foreign.
        let other_desktop = OwnerRecord::new(OP, &"f".repeat(32), STARTED)
            .unwrap()
            .render();
        let h = Fake::new(
            out(0, ""),
            vec![out(0, &show_out(&other_desktop, "active"))],
        );
        assert!(
            release_own(&h, &CancelToken::new(), &rec(OP))
                .await
                .is_err()
        );
        assert_eq!(h.mutating(), 0);

        // Free lock: nothing to do.
        let free = Fake::new(out(0, ""), vec![out(0, NOT_FOUND)]);
        release_own(&free, &CancelToken::new(), &rec(OP))
            .await
            .unwrap();
        assert_eq!(free.mutating(), 0);
    }

    fn clear() -> StaleEvidence {
        StaleEvidence {
            step_units: Evidence::Clear,
            apt_processes: Evidence::Clear,
            lock_files: Evidence::Clear,
        }
    }

    #[test]
    fn stale_gate_requires_every_evidence_input_clear() {
        assert_eq!(check_stale_release(false, &clear()), Ok(()));
        assert_eq!(
            check_stale_release(true, &clear()),
            Err(StaleRefusal::LocalOperationActive)
        );
        for bad in [Evidence::Present, Evidence::Unknown] {
            let mut e = clear();
            e.step_units = bad;
            assert_eq!(check_stale_release(false, &e), Err(StaleRefusal::StepUnits));
            let mut e = clear();
            e.apt_processes = bad;
            assert_eq!(
                check_stale_release(false, &e),
                Err(StaleRefusal::AptProcesses)
            );
            let mut e = clear();
            e.lock_files = bad;
            assert_eq!(check_stale_release(false, &e), Err(StaleRefusal::LockFiles));
        }
    }

    #[tokio::test]
    async fn stale_release_stops_only_the_holder_the_user_saw() {
        let theirs = rec(OTHER_OP).render();
        let f = Fake::new(out(0, ""), vec![out(0, &show_out(&theirs, "active"))]);
        release_stale(&f, &CancelToken::new(), &theirs, false, &clear())
            .await
            .unwrap();
        assert!(f.lines().iter().any(|x| x.contains("'stop'")));

        // Holder replaced after the user looked: refuse.
        let g = Fake::new(out(0, ""), vec![out(0, &show_out(&theirs, "active"))]);
        let e = release_stale(
            &g,
            &CancelToken::new(),
            "ossiframe-lock op=old",
            false,
            &clear(),
        )
        .await
        .unwrap_err();
        assert_eq!(e, LockError::HolderChanged);
        assert_eq!(g.mutating(), 0);

        // Missing evidence: no command at all.
        let mut ev = clear();
        ev.apt_processes = Evidence::Unknown;
        let h = Fake::new(out(0, ""), vec![out(0, &show_out(&theirs, "active"))]);
        let e = release_stale(&h, &CancelToken::new(), &theirs, false, &ev)
            .await
            .unwrap_err();
        assert_eq!(e, LockError::StaleRefused(StaleRefusal::AptProcesses));
        assert!(h.lines().is_empty());
    }
}
