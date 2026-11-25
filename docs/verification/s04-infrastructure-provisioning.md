# S04 Infrastructure Provisioning: verification (draft)

Status: draft. Task 9 completes this file. Live verification of mutation (tasks 8.2 and later) is not done.

## 8.1 Read-only preflight (Doctor server)

Server: the existing 512 MB Lightsail Ubuntu server at `<server-ip>`, user `ubuntu`, key `<key-path>`. The host key was pinned earlier; the ed25519 fingerprint `SHA256:qobMUOP//FiL3rW2mD42syTV5iK5BzW0Dw9Rylibswk` was confirmed by the user in the Lightsail console. The server was never provisioned and nothing was installed.

### How it was run

The Deploy GUI was not driven. The same code path ran headlessly through the ignored test `modules/ubuntu-provisioning/tests/live_preflight.rs`. It connects through `ossiframe-ssh`, accepts only the host key whose fingerprint equals the expected one, and calls `Provisioner::preflight_for` with the default selection (git, nginx, php, php-fpm, composer, mysql; Redis unselected) and no prior operation. It refuses any privileged command except the exact `sudo -n -- /usr/bin/true` probe. No plan was created and nothing was approved. This did not go through the `provision_preflight` Tauri command, so the IPC layer and the Deploy UI remain unverified on a real server.

Commands issued (all read-only; the sudo probe is the only privileged one): `hostname`, `cat /etc/os-release`, `uname -r`, `uname -m`, `cat /proc/uptime`, `nproc`, `lscpu`, `cat /proc/meminfo`, `df -P ...`, `cat /proc/stat` (twice), `dpkg-query -W` (inventory and planned packages), `systemctl list-units` and `list-unit-files`, `ss -H -tulnp`, `id -u`, `sudo -n -- true`, `apt-cache policy` for the planned packages, `dpkg --audit`, `pgrep -x -l`, `systemctl is-active apt-daily*.service`, `test -f /var/run/reboot-required`, `find` on the nginx and apache sites-enabled directories and `/etc/apt/sources.list.d`, `systemctl list-units 'ossiframe-op-*' 'ossiframe-provision-lock*'`, and `cat` of `/etc/apt/sources.list` and `/etc/apt/sources.list.d/ubuntu.sources`.

### Observed report

| Field | Observed |
| --- | --- |
| OS | ubuntu 24.04 |
| Sudo | passwordless |
| Server class | fresh |
| Total memory | 412 MB |
| Free disk | 15860 MB |
| Package sources | `ap-southeast-1.ec2.archive.ubuntu.com`, `security.ubuntu.com` |
| Sources allowlist verdict | no source blocker (the regional mirror matches `*.archive.ubuntu.com`) |
| Nginx sites | none |
| Preinstalled planned packages | `git` 1:2.43.0-1ubuntu7.4. All other planned packages absent (nginx, mysql-server, composer, php8.3-*, redis-server). |

Conflicts, first run (before the fix below):

| Id | Severity | Cause |
| --- | --- | --- |
| `dpkg-broken` | blocker | False positive. See "Bug found". |
| `memory-low` | blocker | MySQL needs at least 900 MB; the server has 412 MB. Expected. |
| `apt-busy` | warning | `pgrep` found `unattended-upgr` running (`apt-daily*.service` were inactive/failed). Plausible for a freshly running Ubuntu server. |

Conflicts, after the fix:

| Id | Severity | Cause |
| --- | --- | --- |
| `dpkg-evidence` | blocker | `dpkg --audit` exits 2 for user `ubuntu`: "unable to check lock file for dpkg database directory /var/lib/dpkg: Permission denied". The dpkg health cannot be checked without root, so the evidence is missing and the preflight fails closed. |
| `memory-low` | blocker | As above. |
| `apt-busy` | warning | As above. |

`can_provision` is false. That matches the expectation for RAM. The dpkg blocker was not expected.

### Bug found and fixed

`dpkg --audit` printed nothing on stdout and a permission error on stderr (exit 2). The preflight treated any stderr output as a broken dpkg and raised `dpkg-broken`, which tells the user to repair packages that are not broken. Fix in `modules/ubuntu-provisioning/src/preflight.rs`: a non-zero exit with no stdout is missing evidence (`dpkg-evidence`), not `dpkg-broken`. Stdout output is still broken. Unit test row added to the evidence table in `preflight_tests.rs`.

### Open finding (needs a decision)

Running `dpkg --audit` unprivileged fails on this server, so the preflight as specified (the sudo probe is the only privileged command) can never report dpkg health as good for a non-root user, and every preflight of a standard Ubuntu server will carry a `dpkg-evidence` blocker. Options were: run `dpkg --audit` through `sudo -n` (a second privileged preflight command that changes the spec), or derive dpkg health without root from `dpkg-query` package states. Decision taken: derive it from an unprivileged `dpkg-query -W` listing (blocks on `H`, `U`, `F`, `W`, `t` and the `R` error flag; non-zero exit, timeout or truncated output stays `dpkg-evidence`). Implemented in the preflight, recheck and reconcile code; the sudo probe remains the only privileged preflight command. It does not cover the missing-metadata checks of `dpkg --audit`. Not live-verified yet; that happens in task 8.3.

### Not observed

Listener ownership (`ss` output) and the unit scan produced no conflicts. The raw outputs were not recorded beyond the values above.
