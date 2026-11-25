# ADR-0004: Remote execution and privileged operations

Status: Accepted (2026-11-24). S04 notes below are design facts; live behaviour is pending verification (S04 task 8).

## Decision
- All remote work goes through an `Executor` taking a `CommandSpec`: program, arguments, privilege, timeout. S04 implements `Privilege::{User, Root}`. `RunAs(user)`, stdin and a mutating flag are not implemented; stdin arrives in S05 when file content needs it.
- Core quotes each argument (POSIX single-quote escaping) and builds one command line. Modules never concatenate shell strings.
- Privileged commands use `'/usr/bin/sudo' '-n' '--'` with an absolute program path, so they fail instead of hanging on a password prompt. Passwordless sudo only; no sudo password in S04.
- Secrets travel over stdin, never in arguments. Known secrets are redacted from output.
- Read-only behavior (Doctor) comes from exposing only typed IPC operations built on predefined commands, not from command metadata. Trusted compiled code is not sandboxed.
- S01 implements only program, arguments, quoting, timeout, cancellation, exit status, bounded output and structured errors. Privilege, operation records and the remote lock are implemented in S04.
- Operation record: ID, server ID, type, status, timestamps, sanitized output, exit code, error.
- States: Pending, Running, Verifying, Succeeded, Failed, Cancelled, UnknownOutcome.
- An interruption during Running results in UnknownOutcome. Never retry it automatically. Do not assume a disconnect stopped the remote process.
- Mutating operations on the same server or app are serialized with a lock. Steps are idempotent where practical; critical changes are verified.
- Deploy uses a dedicated management user with passwordless sudo after bootstrap. This is effectively root and documented as such.

## Rationale
One execution path makes quoting, privilege, redaction and logging consistent. A small state model covers real needs without a workflow engine.

## S04 resolutions
- **Durable execution:** long apt steps run as transient systemd units (`sudo -n /usr/bin/systemd-run --unit=ossiframe-op-<operation UUID>-<index> --service-type=exec --remain-after-exit`), polled with `systemctl show`, with a bounded journal tail stored. The unit name is stored before launch. An SSH loss, deadline or crash gives `UnknownOutcome`; nothing is relaunched and nothing stops apt, dpkg or the unit. Pending verification: that `--remain-after-exit` keeps a finished unit readable, and the `NEEDRESTART_SUSPEND` effect.
- **Remote lock:** the transient unit `ossiframe-provision-lock` instead of `flock`. systemd refuses a duplicate name, so no lock file is needed. It is cooperative and cleared by a reboot. Pending verification: duplicate-unit refusal on a real server. A user-gated explicit release exists for stale holders.
- **Unknown outcome:** read-only reconcile settles only what evidence proves. A missing unit alone is never proof. A user "mark resolved" action closes the rest as `Failed` after backend evidence shows apt/dpkg idle.
- **Read-only enforcement:** `ReadOnly` rejects `Privilege::Root` on `run_operation`. It is not a sandbox (see `docs/security.md`).

## Open questions
- Whether Deploy offers to disable root and password SSH login after bootstrap (not part of S04; no SSH or firewall change is made).
- Sudo with a password, and stdin for secrets or file content: deferred to S05.
