//! Live, read-only preflight against a server the user names. Never runs by default.
//!
//! OSSIFRAME_LIVE_HOST=1.2.3.4 OSSIFRAME_LIVE_USER=ubuntu \
//! OSSIFRAME_LIVE_KEY=~/.ssh/key.pem OSSIFRAME_LIVE_FINGERPRINT=SHA256:... \
//! cargo test -p ossiframe-mod-ubuntu-provisioning --test live_preflight -- --ignored --nocapture
//!
//! Connects through `ossiframe-ssh`, accepts only the host key whose SHA-256
//! fingerprint equals the one given, and runs `Provisioner::preflight_for`
//! with the default selection and no prior operation. Every command line is
//! printed. Any privileged command other than the sudo probe is refused.

use async_trait::async_trait;
use ossiframe_core::{
    CancelToken, CommandSpec, ExecError, ExecResult, Executor, Hostname, KeyPath, Port, Privilege,
    Provisioner, SshUser,
};
use ossiframe_mod_ubuntu_provisioning::UbuntuProvisioning;
use ossiframe_ssh::{AuthMethod, ConnectError, ConnectOptions, ConnectTarget, connect};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

const PROBE: &str = "'/usr/bin/sudo' '-n' '--' '/usr/bin/true'";

struct Logged<'a>(&'a dyn Executor);

#[async_trait]
impl Executor for Logged<'_> {
    async fn run(&self, spec: &CommandSpec, cancel: &CancelToken) -> Result<ExecResult, ExecError> {
        let line = spec.to_command_line();
        if spec.privilege() == Privilege::Root && line != PROBE {
            eprintln!("REFUSED privileged command: {line}");
            return Err(ExecError::PrivilegeDenied);
        }
        eprintln!("CMD {line}");
        let result = self.0.run(spec, cancel).await;
        // Show the raw result of the checks whose verdict is a blocker or warning.
        let shown = matches!(spec.program(), "/usr/bin/dpkg" | "/usr/bin/pgrep")
            || line.contains("'is-active'");
        if let (true, Ok(r)) = (shown, &result) {
            let cut = |s: &str| s.chars().take(400).collect::<String>();
            eprintln!(
                "  -> exit {:?} stdout {:?} stderr {:?}",
                r.exit,
                cut(&r.stdout),
                cut(&r.stderr)
            );
        }
        result
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a server the user names; see file header"]
async fn live_preflight_read_only() {
    let target = ConnectTarget {
        host: Hostname::parse(&env("OSSIFRAME_LIVE_HOST")).unwrap(),
        port: Port::new(22).unwrap(),
        user: SshUser::parse(&env("OSSIFRAME_LIVE_USER")).unwrap(),
    };
    let auth = AuthMethod::KeyFile {
        path: KeyPath::parse(&env("OSSIFRAME_LIVE_KEY")).unwrap(),
        passphrase: None,
    };
    let opts = ConnectOptions::default();
    let expected = env("OSSIFRAME_LIVE_FINGERPRINT");

    let presented = match connect(&target, &auth, None, &opts).await {
        Err(ConnectError::HostKeyUnknown(k)) => *k,
        Err(e) => panic!("expected HostKeyUnknown, got {e:?}"),
        Ok(_) => panic!("connected without a pin"),
    };
    assert_eq!(presented.sha256, expected, "host key fingerprint differs");
    println!("host key {} {}", presented.key_type, presented.sha256);

    let exec = connect(&target, &auth, Some(presented.into()), &opts)
        .await
        .expect("connect with pin");
    let report = UbuntuProvisioning
        .preflight_for(
            &Logged(&exec),
            &CancelToken::new(),
            &default_selection(),
            &[],
        )
        .await
        .expect("preflight");
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    exec.disconnect().await;
}

fn default_selection() -> Vec<String> {
    ["git", "nginx", "php", "php-fpm", "composer", "mysql"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect()
}
