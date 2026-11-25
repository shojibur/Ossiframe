# Requirements Document

## Introduction

S05 adds application deployment and management to Ossiframe Deploy: creating applications (Laravel, generic PHP, static HTML), deploying from Git, domain and NGINX configuration, PHP-FPM pools, per-app Linux user isolation, MySQL database provisioning, safe `.env` editing, a pre-switch boot check, releases with manual rollback, and health checks. It reuses the S04 plan, digest, approval, operation engine and single per-server remote lock unchanged, adding one module crate (`app-deploy`), three small Core primitives (`LinuxUserName`, a `RunAs` privilege and a `stdin` field on `CommandSpec`), and app/release tables in the desktop store. These requirements are derived from `design.md`.

## Glossary

- **App**: A deployed application record: identity, type, domain, repository, Linux user, app root, optional database, entry file, state, current release.
- **AppType**: One of `Laravel`, `Php`, `Static`.
- **AppSlug**, **DomainName**, **GitUrl**, **BranchName**, **LinuxUserName**: Core typed values parsed and validated at construction. LinuxUserName never accepts `root`.
- **Entry_File**: A validated relative path (no `..`, no leading `/`) naming an App's entry point: Laravel `public/index.php`, Php `public/index.php` or `index.php`, Static `index.html` by default. Its parent directory is the document root.
- **App_User**: The dedicated Linux user created for one App, named `ossi-<slug>` by the App_Deploy_Module, with no login shell.
- **App_Root**: The App's home directory, `/srv/ossiframe/apps/<slug>`, owned by App_User.
- **Release**: One deployed snapshot of an App's code under `App_Root/releases/<release-id>`, with a status.
- **Current_Symlink**: The `App_Root/current` symlink, pointed at exactly one Release.
- **RunAs**: `Privilege` variant carrying a LinuxUserName, rendering `'/usr/bin/sudo' '-n' '-u' '<user>' '--' ...`. Root remains the separate `Privilege::Root`.
- **ReadOnly_Guard**: The Doctor executor wrapper that rejects privileged commands.
- **Stdin_Field**: Optional field on `CommandSpec` carrying content sent to the remote command's standard input, bounded at 1 MiB and never serialized.
- **Content_Write_Primitive**: The fixed two-command sequence (`install` from `/dev/stdin` to a temp path, then `mv -T` onto the final path) used for every managed file write.
- **Managed_Path**: A path starting with an NGINX `sites-available/ossiframe-` prefix, a PHP-FPM `pool.d/ossiframe-` prefix, or an App_Root. Defined in the App_Deploy_Module.
- **App_Deploy_Module**: Crate `modules/app-deploy` (`ossiframe-mod-app-deploy`), implementing `Provisioner` with operation kind `app-deploy`.
- **Deploy_Key**: An ed25519 SSH key pair generated on the server under an App_User's home. The private half never leaves the server.
- **Git_Known_Hosts**: The App_User's `<App_Root>/.ssh/known_hosts` file, written by Ossiframe.
- **Builtin_Git_Hosts**: github.com, gitlab.com and bitbucket.org, whose officially published SSH host keys ship as data in the App_Deploy_Module with a recorded source.
- **App_Database**: A MySQL database and user for a Laravel App, both named `ossi_<slug>`, with privileges limited to that database.
- **Db_Password**: A locally generated secret for an App_Database, sent once over stdin and stored only in the App's server-side `.env`.
- **Boot_Check**: A type-specific check of a new Release run as App_User before Current_Symlink is switched.
- **Health_Check**: A post-switch `CheckResult` run through `Provisioner::verify()`, essential or optional.
- **Create_Plan**, **Redeploy_Plan**, **Rollback_Plan**, **Remove_Plan**: The four Plan shapes `App_Deploy_Module.plan()` can produce.
- **Server_Lock**: The S04 remote lock unit `ossiframe-provision-lock` and the engine's one-unresolved-operation-per-server rule.
- **Default_Site**: The distro's `/etc/nginx/sites-enabled/default` site.
- **Doctor**: `apps/doctor`, unaffected by S05.
- **Deploy**: `apps/deploy`, the application this spec extends.
- **Disposable_Server**: The existing 2 GB Lightsail server used for S04 provisioning, reused for S05 live verification.
- **Verification_Record**: `docs/verification/s05-application-deployment.md`.

## Requirements

### Requirement 1: Application type model stays outside Core

**User Story:** As a framework developer, I want application-type knowledge confined to a module, so that Core and the engine never depend on Laravel, PHP or NGINX.

#### Acceptance Criteria

1. THE ossiframe-core crate SHALL contain no identifier, constant or type naming Laravel, PHP, NGINX, MySQL or the `ossi-` user prefix.
2. THE App_Deploy_Module SHALL define AppType as data plus step-building functions for exactly Laravel, Php and Static in S05.
3. THE App_Deploy_Module SHALL implement the existing Provisioner trait without requiring any change to its signature.
4. WHERE a future AppType is added, THE App_Deploy_Module SHALL be the only crate requiring a change.

### Requirement 2: Typed identifiers at the IPC boundary

**User Story:** As a framework developer, I want every user-supplied application identifier parsed into a typed value before use, so that no raw string reaches a remote command.

#### Acceptance Criteria

1. THE Core SHALL define AppSlug, DomainName, GitUrl and BranchName, each parsed and validated at construction.
2. IF a submitted slug, domain, repository URL, branch or Entry_File fails validation, THEN THE relevant command SHALL return a ValidationError before any server contact.
3. THE AppSlug parser SHALL accept only lowercase letters, digits and hyphens, 3 to 32 characters.
4. THE DomainName parser SHALL reject a wildcard, a URL scheme and a path component.
5. THE GitUrl parser SHALL accept only an `https://` URL or a `git@host:path` SSH form, and SHALL reject a `file://` or other non-network scheme.
6. THE App_User, App_Root, App_Database name and PHP-FPM pool name SHALL be generated from the validated AppSlug and SHALL never be accepted as direct user input.
7. THE Entry_File parser SHALL reject a path containing `..`, a leading `/` or a shell metacharacter.

### Requirement 3: LinuxUserName and RunAs privilege

**User Story:** As a framework developer, I want to run a command as a specific unprivileged Linux user through the structured command model, so that application code never runs as root or as the SSH login account.

#### Acceptance Criteria

1. THE Core SHALL define LinuxUserName, accepting only the pattern `^[a-z_][a-z0-9_-]{0,31}$` and rejecting `root`.
2. WHEN a CommandSpec with Privilege RunAs(u) is rendered, THE CommandSpec SHALL produce a command line starting with `'/usr/bin/sudo' '-n' '-u' '<u>' '--'` followed by individually quoted program and arguments.
3. IF a LinuxUserName is constructed from `root` or from a string outside the pattern, THEN THE constructor SHALL return a ValidationError and no RunAs CommandSpec SHALL be created.
4. THE ReadOnly_Guard SHALL reject any CommandSpec whose Privilege is RunAs or Root.

### Requirement 4: Stdin on CommandSpec

**User Story:** As a framework developer, I want to send file content or a secret to a remote command over standard input, so that no secret and no file content is ever passed as a command-line argument.

#### Acceptance Criteria

1. THE CommandSpec SHALL gain an optional stdin byte field, set only through a builder method.
2. WHEN a CommandSpec carries stdin content, THE Executor SHALL write that content to the remote command's standard input and then send EOF before reading output.
3. IF stdin content exceeds 1 MiB, THEN THE CommandSpec builder SHALL return a ValidationError before any network call.
4. THE to_command_line rendering SHALL NOT include stdin content in the rendered text.
5. THE CommandSpec serialization SHALL omit stdin, and THE Engine SHALL NOT record stdin content in any plan, stored step output, log or event.

### Requirement 5: Content-write primitive

**User Story:** As a server owner, I want every file Ossiframe writes to be created atomically with the right owner and permissions, so that no managed file is ever readable, writable or partially written in an unsafe state.

#### Acceptance Criteria

1. WHEN the App_Deploy_Module writes a Managed_Path, THE Engine SHALL send the content over stdin to a fixed `install -m <mode> -o <owner> -g <group> -- /dev/stdin <path>.ossiframe-tmp` command.
2. WHEN the temporary file is created, THE Engine SHALL then run a fixed `mv -T -- <path>.ossiframe-tmp <path>` command to atomically replace the target.
3. IF the install step fails, THEN THE Engine SHALL NOT run the move step and the original file at `<path>` SHALL remain unchanged.
4. IF the move step fails, THEN THE original file at `<path>` SHALL remain unchanged.
5. THE App_Deploy_Module SHALL refuse to build a write step for a path that is not a Managed_Path.
6. WHEN an NGINX site or PHP-FPM pool file is written, THE Engine SHALL validate it (`nginx -t` or `php-fpm8.3 -t`) before reloading the corresponding service, and SHALL NOT reload WHEN validation fails.
7. WHEN an existing Ossiframe-owned file is about to be overwritten, THE App_Deploy_Module SHALL confirm its content marker line is present and unaltered, and SHALL treat a missing or altered marker as drift requiring a new plan.

### Requirement 6: Application isolation

**User Story:** As a server owner, I want every application to run as its own unprivileged Linux user, so that one application's code cannot read or modify another application's files or credentials.

#### Acceptance Criteria

1. WHEN an App is created, THE Create_Plan SHALL include a step creating a system Linux user `ossi-<slug>` with no login shell and a home directory at App_Root.
2. THE App_Root directory SHALL be owned by the App_User with mode 0750.
3. WHEN a clone, build, migration, Boot_Check or artisan command is run for an App, THE Engine SHALL run it with Privilege RunAs(App_User), never Root and never as the SSH login user.
4. IF the generated Linux user name already exists on the server for a different App, THEN THE App_Deploy_Module SHALL refuse to plan the App.

### Requirement 7: PHP-FPM pool per application

**User Story:** As a server owner, I want each PHP application to have its own PHP-FPM pool and socket, so that applications are isolated at the process and socket level.

#### Acceptance Criteria

1. WHERE the AppType is Laravel or Php, THE Create_Plan SHALL include a step writing a pool file at `/etc/php/8.3/fpm/pool.d/ossiframe-<slug>.conf` with a dedicated Unix socket `/run/php/ossiframe-<slug>.sock` and `pm.max_children = 5`.
2. THE pool file SHALL set `user` and `group` to the App_User.
3. WHEN the pool file is written, THE Engine SHALL validate it with `php-fpm8.3 -t` and reload `php8.3-fpm` only on success.
4. WHERE the AppType is Static, THE Create_Plan SHALL include no PHP-FPM pool step.

### Requirement 8: NGINX site and domain configuration

**User Story:** As a server owner, I want each application's domain served through an Ossiframe-managed NGINX site that never disturbs configuration Ossiframe does not own, so that I can safely host multiple applications.

#### Acceptance Criteria

1. WHEN an App is created, THE Create_Plan SHALL include a step writing an NGINX site file at `/etc/nginx/sites-available/ossiframe-<slug>` containing a content marker line identifying it as Ossiframe-managed, with the document root set to the parent directory of the Entry_File under `current`.
2. THE site file SHALL route PHP requests to the App's own PHP-FPM socket for Laravel and Php, and SHALL contain no PHP location block for Static.
3. WHEN the site file is written, THE Engine SHALL enable it via a symlink in `sites-enabled`, validate with `nginx -t`, and reload `nginx` only on success.
4. IF the requested DomainName is already used by another Ossiframe-owned site on the same server, THEN THE App_Deploy_Module SHALL refuse the Create_Plan.
5. THE Create_Plan SHALL NOT remove or disable the Default_Site unless the user explicitly selects the option to disable it.
6. WHERE the user selects the option to disable the Default_Site, THE Create_Plan SHALL include a separate step removing the `sites-enabled/default` symlink, validated with `nginx -t` before reload.

### Requirement 9: Git access with deploy keys and pinned host keys

**User Story:** As a developer, I want to deploy from public and private Git repositories without Ossiframe storing my Git credentials and without trusting an unverified Git host key, so that repository access cannot leak or be intercepted.

#### Acceptance Criteria

1. WHEN the GitUrl is an SSH form and no Deploy_Key exists yet for the App, THE Create_Plan SHALL include a step generating an ed25519 Deploy_Key on the server under the App_User's home.
2. THE Engine SHALL NOT transmit the private half of a Deploy_Key off the server in any form.
3. THE Deploy SHALL display the public half of a Deploy_Key to the user after generation, with instructions to add it as a read-only key on the Git host.
4. WHEN cloning with a Deploy_Key, THE App_Deploy_Module SHALL run a deploy-key check before the clone step and SHALL fail the operation with an actionable message before cloning IF the check fails.
5. THE App_Deploy_Module SHALL NOT store any Git credential, token or password in SQLite.
6. WHEN a Create_Plan is re-planned after a Deploy_Key already exists for the App, THE App_Deploy_Module SHALL reuse the existing key.
7. THE App_Deploy_Module SHALL ship the officially published SSH host keys of the Builtin_Git_Hosts as data, each with its documentation source, copied from the providers' official documentation at implementation time.
8. WHEN the GitUrl is an SSH form on a Builtin_Git_Host, THE Engine SHALL write the shipped host keys into Git_Known_Hosts before any SSH Git operation.
9. IF the GitUrl is an SSH form on a host that is not a Builtin_Git_Host and the user has not confirmed that host's key fingerprints, THEN THE App_Deploy_Module SHALL return the scanned fingerprints with reason `gitHostKeyUnconfirmed` and SHALL write nothing.
10. WHEN the user confirms the fingerprints of another host, THE Engine SHALL write into Git_Known_Hosts only the scanned keys matching the confirmed fingerprints.
11. IF the scanned host key of another host differs from the confirmed key at recheck, THEN THE Engine SHALL return rePlanRequired and run nothing.
12. THE App_Deploy_Module SHALL run every Git SSH command with `StrictHostKeyChecking=yes` and the App's Git_Known_Hosts, and SHALL NOT use `accept-new` or any trust-on-first-use option.
13. THE value of `GIT_SSH_COMMAND` SHALL be built only from fixed options and paths derived from the validated App_Root, and SHALL NOT contain user-supplied text.
14. WHERE the GitUrl is an `https://` URL, THE App_Deploy_Module SHALL clone without a Deploy_Key or Git_Known_Hosts entry.

### Requirement 10: Database and database user provisioning

**User Story:** As a developer deploying a Laravel application, I want a dedicated database and user created automatically with least-privilege access, so that my application has working storage without manual MySQL administration.

#### Acceptance Criteria

1. WHERE the AppType is Laravel, THE Create_Plan SHALL include a step creating a MySQL database and user both named `ossi_<slug>`.
2. THE created database user SHALL be granted privileges only on its own database, never on `*.*` or another App's database.
3. THE Db_Password SHALL be generated locally with a CSPRNG and sent to the server only over stdin, never as a command-line argument.
4. THE Db_Password SHALL be persisted only in the App's server-side `.env` file, and SHALL NOT be written to SQLite, the OS keychain, any log or any IPC response.
5. WHERE the AppType is Php or Static, THE Create_Plan SHALL include no database step.
6. THE App_Deploy_Module SHALL build every database and user identifier in its provisioning SQL from the validated AppSlug, and SHALL NOT interpolate free-text input into SQL identifiers.
7. THE Deploy SHALL show whether an App has a database without exposing the Db_Password.

### Requirement 11: Secure environment configuration

**User Story:** As a developer, I want to edit my application's environment variables safely, so that secrets are protected in storage, in the UI and in logs.

#### Acceptance Criteria

1. WHEN the environment is read through app_get_env, THE Deploy SHALL redact the value of any key matching a secret-like pattern (`*_KEY`, `*_SECRET`, `*_PASSWORD`, `*_TOKEN`, `DB_PASSWORD`, `APP_KEY`).
2. WHEN the environment is written through app_set_env, THE Engine SHALL write the file via the Content_Write_Primitive to `App_Root/shared/.env` with mode 0600, owned by the App_User and its group.
3. THE Engine SHALL NOT log or store the contents of an `.env` write outside the server's own file.
4. THE app_set_env command SHALL go through the same plan-and-approval path as any other mutation.

### Requirement 12: Application-specific build steps

**User Story:** As a developer, I want deployment to run the right build and setup steps for my application type automatically, so that a deploy produces a working application without manual server commands.

#### Acceptance Criteria

1. WHERE the AppType is Laravel, THE build steps SHALL include, in order: clone, `composer install --no-dev --no-interaction`, linking shared `.env` and `storage`, `artisan storage:link`, `artisan config:cache`, `artisan route:cache`, `artisan view:cache`.
2. WHERE the AppType is Laravel and the user has opted in for this deploy, THE build steps SHALL include `artisan migrate --force` before the cache steps.
3. IF the user has not opted in to migrations for a given deploy, THEN THE build steps SHALL NOT include a migrate step.
4. WHERE the AppType is Php and a `composer.json` is present in the cloned repository, THE build steps SHALL include `composer install --no-dev --no-interaction`.
5. WHERE the AppType is Static, THE build steps SHALL include no build command.
6. THE App_Deploy_Module SHALL NOT offer a user-defined custom deploy step in S05.

### Requirement 13: Releases, boot check, switch and rollback

**User Story:** As a developer, I want each deploy to be checked before it goes live and switched atomically, so that a broken release never replaces a working one and a deploy can be reverted quickly.

#### Acceptance Criteria

1. WHEN a deploy (create or redeploy) runs, THE App_Deploy_Module SHALL clone into a new release directory under `App_Root/releases/` and SHALL NOT modify the Current_Symlink until the build steps and the Boot_Check succeed.
2. WHEN the build steps succeed, THE Engine SHALL run the Boot_Check as the App_User before switching the Current_Symlink: for Laravel `php <release_dir>/artisan --version`, for Php `php -l <release_dir>/<Entry_File>`, for Static a check that `<release_dir>/<Entry_File>` exists.
3. IF a build step or the Boot_Check fails, THEN THE Engine SHALL NOT switch the Current_Symlink, SHALL keep the new release directory for inspection and SHALL fail the operation with the Current_Symlink unchanged.
4. WHEN the Boot_Check succeeds, THE Engine SHALL switch the Current_Symlink to the new release using a temporary symlink plus an atomic rename.
5. THE App_Deploy_Module SHALL retain at most 5 releases per App plus the Current_Symlink's target, removing older release directories after a successful switch and never removing the Current_Symlink's target.
6. WHEN a Rollback_Plan is approved, THE Engine SHALL switch the Current_Symlink to the specified prior release without cloning or running build steps.
7. IF a Rollback_Plan names a release id that is not one of the App's own recorded releases or no longer exists on disk, THEN THE App_Deploy_Module SHALL refuse the Rollback_Plan.
8. THE App_Deploy_Module SHALL NOT reverse a database migration as part of a rollback.

### Requirement 14: Progress, logs, history and health checks

**User Story:** As a developer, I want to see deploy progress, logs, history and the result of health checks, so that I know whether my application is healthy after a deploy.

#### Acceptance Criteria

1. THE App_Deploy_Module SHALL reuse the existing Operation, OperationStep and OperationEvent records for every app-deploy mutation, with operation kind `app-deploy`.
2. WHEN the Current_Symlink has been switched, THE App_Deploy_Module SHALL run Health_Checks including an HTTP reachability check against the App's domain with the correct Host header.
3. WHERE the AppType is Laravel or Php, THE Health_Checks SHALL include a check that the App's PHP-FPM socket exists and the `php8.3-fpm` unit is active.
4. THE Operation SHALL be Succeeded only if every essential Health_Check passes, matching the S04 Succeeded rule.
5. IF an essential Health_Check fails after the switch, THEN THE Engine SHALL mark the Operation Failed, SHALL leave the Current_Symlink on the new release and SHALL NOT roll back automatically.
6. WHEN an Operation fails a Health_Check, THE Deploy SHALL offer a manual rollback as the primary recovery action.
7. THE Deploy SHALL list an App's Release history with status and timestamp.

### Requirement 15: Single remote lock per server

**User Story:** As a server owner, I want app and provisioning operations on one server to exclude each other, so that nothing mutates shared server state concurrently.

#### Acceptance Criteria

1. THE App_Deploy_Module SHALL use the Server_Lock unchanged for every app-deploy operation.
2. WHILE any operation is unresolved on a server, THE Engine SHALL refuse a new app-deploy or provisioning operation on that server with the existing `busy` reason.
3. WHEN operations are on different servers, THE Engine SHALL allow them to run independently.

### Requirement 16: Application removal

**User Story:** As a server owner, I want to remove an application's public presence without accidentally destroying its data, so that removal is safe by default and destructive only when I explicitly ask.

#### Acceptance Criteria

1. WHEN a Remove_Plan is approved with default options, THE Engine SHALL remove the App's NGINX site and PHP-FPM pool and mark the App Removed, and SHALL NOT delete the App_Root directory, the App_User or the App_Database.
2. WHERE the user selects dropDatabase, THE Remove_Plan SHALL include a step dropping the App_Database and its user.
3. WHERE the user selects deleteFiles, THE Remove_Plan SHALL include a step deleting the App_User and its home directory.
4. THE dropDatabase and deleteFiles options SHALL default to false and SHALL be presented as two separate, explicitly labeled confirmations.
5. IF a server has any App that is not Removed, THEN THE Deploy SHALL refuse to remove that server profile.

### Requirement 17: Reuse of the S04 approval and operation engine

**User Story:** As a framework developer, I want application mutations to use the same preflight, plan, digest and approval mechanism as provisioning, so that the verified S04 guarantees extend to S05 without duplicated code.

#### Acceptance Criteria

1. THE App_Deploy_Module SHALL implement the existing Provisioner trait without any change to the trait's signature.
2. THE App_Deploy_Module SHALL produce a Plan whose digest is computed the same way as an S04 Plan.
3. THE app_approve_plan command SHALL perform the same approval checks as provision_approve_plan: unknown plan, superseded, expired, digest mismatch, identity mismatch, busy.
4. THE App_Deploy_Module SHALL use the existing OperationState machine without adding a state.
5. IF the server's state has drifted since planning (base stack missing, a managed file's marker changed, App_User or App_Database missing), THEN THE Engine SHALL return rePlanRequired at approval time and SHALL run nothing.

### Requirement 18: Doctor and server scope remain unaffected

**User Story:** As a server owner, I want Doctor to remain read-only and unaffected by application deployment, so that a diagnostics-only server is never put at risk.

#### Acceptance Criteria

1. THE apps/doctor crate SHALL NOT depend on the App_Deploy_Module.
2. THE Doctor handler, build manifest and capability lists SHALL contain no app-deploy command name.
3. THE live verification for S05 SHALL use only the Disposable_Server and SHALL NOT provision or deploy to the separate Doctor server.

### Requirement 19: Live verification through the Deploy UI

**User Story:** As a maintainer, I want S05 behavior confirmed on a real server through the actual Deploy UI, so that no claim is made without evidence.

#### Acceptance Criteria

1. THE S05 verification SHALL run through the Deploy desktop UI on the Disposable_Server, driven by the user.
2. THE agent SHALL record sanitized evidence (no secret, key or private address) in the Verification_Record.
3. THE S05 implementation SHALL stop and request explicit user approval before any command that mutates the Disposable_Server is run.
4. THE Verification_Record SHALL state any check that could not be run rather than fabricating a result.
