# Install Ossiframe Doctor and Ossiframe Deploy

Builds are for macOS on Apple Silicon (M1 or later). Intel Macs, Linux and Windows are not built yet.

- **Ossiframe Doctor**: read-only discovery, inventory and audit of Ubuntu servers.
- **Ossiframe Deploy**: installs base software on Ubuntu servers and deploys applications. It runs commands as root on servers you approve.

Both apps are local-first: no account, no Ossiframe backend, no subscription.

## 1. Download

From the project's [GitHub Releases](https://github.com/shojibur/Ossiframe/releases), download the `.dmg` (or `.app.zip`) for the app you want, plus `SHA256SUMS`.

## 2. Verify the download

In the folder with the downloads:

```sh
shasum -a 256 -c SHA256SUMS --ignore-missing
```

Each file you downloaded must print `OK`. Do not open a file that fails.

## 3. Install

- `.dmg`: open it and drag the app to Applications.
- `.app.zip`: double-click to unzip, then move the app to Applications.

## 4. First launch

Release builds carry an ad-hoc signature, not an Apple Developer ID signature, and are not notarized. macOS therefore blocks the first launch with a warning that it cannot check the app.

To open it the first time:

1. Try to open the app once, and close the warning.
2. Open System Settings → Privacy & Security.
3. Under Security, next to the message about the blocked app, click **Open Anyway**, and confirm.

After that, the app opens normally.

Not yet observed on a release build: the exact Gatekeeper wording on current macOS, and how the keychain behaves with an ad-hoc signed build (see below). This page will be updated after the first release dry run.

## Keychain

The apps store SSH key passphrases and passwords only in your macOS login keychain. They never write secrets to their database or logs. macOS may ask you to allow access the first time an app reads a saved secret. Choose **Always Allow** only if you trust the build you verified above.

Not yet observed: whether an ad-hoc signed build asks again after each update.

## Where data lives

| App    | Local data                                            | Keychain items                 |
| ------ | ----------------------------------------------------- | ------------------------------ |
| Doctor | `~/Library/Application Support/dev.ossiframe.doctor/` | service `dev.ossiframe.doctor` |
| Deploy | `~/Library/Application Support/dev.ossiframe.deploy/` | service `dev.ossiframe.deploy` |

The data folder holds a SQLite database with server profiles and pinned host keys, plus, for Deploy, operation history and application records. It holds no secrets.

## Uninstall

1. Quit the app and delete it from Applications.
2. Delete its data folder from the table above.
3. In Keychain Access, search for the service name from the table above and delete those items.

Servers are not changed by uninstalling. Applications that Deploy set up keep running.

## Known limitations

See [known-limitations.md](known-limitations.md).
