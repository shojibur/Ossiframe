# Product Vision

**Ossiframe**: One foundation. Endless infrastructure tools.

## Problem

Every team that builds a tool for managing remote servers solves the same problems again: SSH authentication, host verification, credential storage, remote command execution with streaming and timeouts, recovery when a connection drops mid-operation, OS detection, and a desktop UI for all of it. These parts are security-sensitive and easy to get wrong.

## Product

Ossiframe is a local-first framework for building desktop and command-line applications that manage remote infrastructure over SSH. It has three layers:

1. **Core**: the reusable skeleton. Connections, credentials, remote execution, operations, persistence, module contracts and a shared desktop foundation.
2. **Modules**: optional, independently selectable capabilities such as Linux inventory, Ubuntu provisioning, NGINX, PHP, MySQL, Laravel and auditing.
3. **Applications**: products that combine Core and modules. The first two are **Ossiframe Deploy** (provision and deploy) and **Ossiframe Doctor** (discover, audit, diagnose).

A developer generates a new application, picks modules, writes their own module against the public SDK, and ships it without changing Core.

## Long-term vision

An open-source foundation for infrastructure management applications across operating systems, providers, runtimes and deployment environments.

## Principles (mandatory)

- Local-first. No required backend, cloud account, subscription or AI provider.
- The user owns the infrastructure. Hosted applications keep running if Ossiframe apps are closed or uninstalled.
- SSH is the only management channel in the initial release.
- Servers are general-purpose. They are never classified as "PHP servers" or "Node servers"; runtimes are installed independently and can coexist.
- Operations are transparent: the user sees what will run, what ran, and the real result.
- Recovery-oriented: an unknown outcome is reported as unknown, never as success or failure.
- Real functionality only. Mock data is allowed only in a clearly labeled demo mode or isolated UI development.
- Capabilities are labeled verified or unverified based on evidence.

## Target users

| User | Primary interest |
|---|---|
| Software developers building infra tools | Core and modules as a foundation (primary audience of the skeleton) |
| Freelancers, agencies, indie SaaS developers | Deploy as a self-hosted alternative to paid server control panels |
| DevOps and infrastructure engineers | Doctor for audits; Core for internal tools |
| System administrators | Doctor diagnostics on servers they did not build |
| Hosting providers | Custom branded control panels built on the skeleton |
| Homelab enthusiasts and open-source developers | Lightweight local tools, extensible modules |

The skeleton's primary customer is the **developer building an infrastructure application**. Deploy and Doctor prove the skeleton works and are useful on their own.

## What makes it different

- It is a framework for building infra apps, not one more control panel.
- It runs locally and talks SSH directly, with no agent or SaaS in between.
- Doctor reports separate verified findings, inferred relationships, failed checks and unsupported checks, instead of a single opaque score.
