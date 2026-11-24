//! Live checks against a disposable server. Never runs by default.
//!
//! OSSIFRAME_TEST_HOST=1.2.3.4 OSSIFRAME_TEST_USER=root \
//! OSSIFRAME_TEST_KEY=~/.ssh/id_ed25519 [OSSIFRAME_TEST_PORT=22] \
//! cargo test -p ossiframe-ssh --test live -- --ignored --nocapture

use ossiframe_core::{
    CancelToken, CommandSpec, ExecError, Executor, ExitStatus, Hostname, KeyPath, Port, SshUser,
};
use ossiframe_ssh::{AuthMethod, ConnectError, ConnectOptions, ConnectTarget, connect};
use std::time::{Duration, Instant};

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} is not set"))
}

fn cmd(p: &str, a: &[&str], secs: u64) -> CommandSpec {
    CommandSpec::new(p, a, Duration::from_secs(secs)).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a disposable server; see file header"]
async fn live_server() {
    let port = std::env::var("OSSIFRAME_TEST_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(22);
    let target = ConnectTarget {
        host: Hostname::parse(&env("OSSIFRAME_TEST_HOST")).unwrap(),
        port: Port::new(port).unwrap(),
        user: SshUser::parse(&env("OSSIFRAME_TEST_USER")).unwrap(),
    };
    let auth = AuthMethod::KeyFile {
        path: KeyPath::parse(&env("OSSIFRAME_TEST_KEY")).unwrap(),
        passphrase: None,
    };
    let opts = ConnectOptions::default();

    // 1. Unknown host key: handshake aborted before auth, fingerprint reported.
    let presented = match connect(&target, &auth, None, &opts).await {
        Err(ConnectError::HostKeyUnknown(k)) => *k,
        Err(e) => panic!("expected HostKeyUnknown, got {e:?}"),
        Ok(_) => panic!("connected without a pin"),
    };
    println!("host key: {} {}", presented.key_type, presented.sha256);

    // 2. Wrong pin: mismatch.
    let mut wrong = presented.clone();
    wrong.public_key = vec![0; 32];
    assert!(matches!(
        connect(&target, &auth, Some(wrong.into()), &opts).await,
        Err(ConnectError::HostKeyMismatch { .. })
    ));

    // 3. Correct pin: connected.
    let exec = connect(&target, &auth, Some(presented.into()), &opts)
        .await
        .expect("connect with pin");
    let cancel = CancelToken::new();

    let r = exec.run(&cmd("uname", &["-a"], 10), &cancel).await.unwrap();
    assert_eq!(r.exit, ExitStatus::Code(0));
    println!("uname -a: {}", r.stdout.trim());

    let r = exec
        .run(&cmd("ls", &["/nonexistent-ossiframe"], 10), &cancel)
        .await
        .unwrap();
    assert_ne!(r.exit, ExitStatus::Code(0));
    assert!(!r.stderr.is_empty());
    println!("ls exit {:?}, stderr: {}", r.exit, r.stderr.trim());

    let r = exec
        .run(&cmd("printf", &["%s", "it's; $(id)"], 10), &cancel)
        .await
        .unwrap();
    assert_eq!(r.stdout, "it's; $(id)");

    let t = Instant::now();
    assert_eq!(
        exec.run(&cmd("sleep", &["30"], 2), &cancel)
            .await
            .unwrap_err(),
        ExecError::Timeout
    );
    assert!(t.elapsed() < Duration::from_secs(5));

    let c2 = CancelToken::new();
    let c2b = c2.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        c2b.cancel();
    });
    assert_eq!(
        exec.run(&cmd("sleep", &["30"], 20), &c2).await.unwrap_err(),
        ExecError::Cancelled
    );

    // The session is still usable after a timeout and a cancel.
    let r = exec.run(&cmd("true", &[], 10), &cancel).await.unwrap();
    assert_eq!(r.exit, ExitStatus::Code(0));
    exec.disconnect().await;
}
