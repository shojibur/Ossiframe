# Product Requirements

Detailed acceptance criteria live in each Kiro spec. This page lists product-level requirements and the full capability roadmap. The first release delivers a working subset; the rest is built progressively.

## Framework (Core, modules, generator)

- Connect over SSH with key or password auth on a configurable port. Pin host keys.
- Generate management keys locally. Only public keys go to servers.
- Store secrets in the OS keychain.
- Run remote commands through one executor with streamed output, real exit codes, timeouts, cancellation and redaction.
- Register trusted modules explicitly at compile time. Modules declare an ID, dependencies and capabilities.
- Provide a starter template and a repository-local generator. Generated projects contain only the selected modules and build outside the Ossiframe repository.
- Ship a custom module example that uses only public APIs and performs a real discovery operation.
- Core contains no application- or software-specific logic.

## Ossiframe Deploy

| Capability | First release | Later |
|---|---|---|
| Custom VPS onboarding (host, port, user, auth) | Yes | Provider APIs (DigitalOcean, Hetzner, AWS, Vultr, Linode) |
| Ubuntu 24.04 provisioning with preflight and plan | Yes | Ubuntu 26.04 once verified; Debian |
| NGINX | Yes | Redirects, aliases, WebSockets |
| PHP-FPM + Composer (distro PHP) | Yes | Multiple PHP versions and extension management |
| MySQL | Yes | PostgreSQL, Redis |
| Laravel app: dedicated user, FPM pool, vhost, database, `.env` | Yes | WordPress, generic PHP, Node.js, Python, containers |
| Git deployment (public repos), output, history, lock, health check | Yes | Private repos with deploy keys, CI/webhook triggers, atomic releases |
| Migrations with explicit approval | Yes | |
| SSL certificates, cronjobs, queue workers, scheduler | | Yes |
| Services, firewall, SSH keys, system users | | Yes |
| Monitoring, logs, configuration editing, backups and recovery | | Yes |
| Integrated terminal, SFTP | | Yes |

Servers are never tied to one runtime. Multiple runtime families and versions must be able to coexist.

## Ossiframe Doctor

- Works on any reachable Linux server, including ones never managed by Deploy.
- Read-only. No provisioning, configuration changes or automatic remediation.
- Discovers OS, hardware, resources, listening ports, services, packages, runtimes, databases and NGINX vhosts.
- Findings include evidence, severity, verification status, timestamp, recommendation and limitations.
- Failed, inaccessible and unsupported checks are reported as such, never as passing.
- JSON export first, HTML report later. No aggregate health score without a documented method.
- Later: infrastructure graph (evidence-based relationships only), certificate checks, extended rules.

## Non-functional

- Light enough for 8 GB Apple Silicon. No background processes when apps are closed.
- macOS Apple Silicon first; Windows and Linux desktop builds later, claimed only after verification.

## Non-goals

- Mandatory hosted backend, account or subscription.
- Dynamic third-party plugin loading.
- Automatic remediation in Doctor.
- Placeholder adapters for runtimes that are not implemented.
- Describing Linux-user isolation as container or VM isolation.
