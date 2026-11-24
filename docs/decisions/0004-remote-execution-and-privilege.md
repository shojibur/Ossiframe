# ADR-0004: Remote execution and privileged operations

Status: Accepted (2026-11-24), with open questions.

## Decision
- All remote work goes through an `Executor` taking a `CommandSpec`: program, arguments, privilege (`User`, `RunAs(user)`, `Root`), timeout, optional stdin, mutating flag.
- Core quotes each argument (POSIX single-quote escaping) and builds one command line. Modules never concatenate shell strings.
- Privileged commands use `sudo -n` so they fail instead of hanging on a password prompt.
- Secrets travel over stdin, never in arguments. Known secrets are redacted from output.
- Read-only behavior (Doctor) comes from exposing only typed IPC operations built on predefined commands, not from command metadata. Trusted compiled code is not sandboxed.
- S01 implements only program, arguments, quoting, timeout, cancellation, exit status, bounded output and structured errors. Privilege modes, operation records and locks are implemented in S04.
- Operation record: ID, server ID, type, status, timestamps, sanitized output, exit code, error.
- States: Pending, Running, Verifying, Succeeded, Failed, Cancelled, UnknownOutcome.
- An interruption during Running results in UnknownOutcome. Never retry it automatically. Do not assume a disconnect stopped the remote process.
- Mutating operations on the same server or app are serialized with a lock. Steps are idempotent where practical; critical changes are verified.
- Deploy uses a dedicated management user with passwordless sudo after bootstrap. This is effectively root and documented as such.

## Rationale
One execution path makes quoting, privilege, redaction and logging consistent. A small state model covers real needs without a workflow engine.

## Open questions (resolve in S04)
- Durable execution for long provisioning steps: `systemd-run` transient units with status files, or another approach. Investigate when provisioning is built.
- Remote lock (`flock` on the server) in addition to the local lock, for multiple desktops.
- Whether Deploy offers to disable root and password SSH login after bootstrap.
