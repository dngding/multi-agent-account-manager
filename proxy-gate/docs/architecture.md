# Architecture

## Data plane

Each assigned account receives a stable loopback HTTP CONNECT endpoint. The
listener reads the CONNECT authority, verifies that the assignment is currently
allowed, opens an HTTP CONNECT or SOCKS5 tunnel to the configured Mihomo
listener, and then copies bytes in both directions.

Proxy Gate never accepts credentials and never opens an unproxied fallback
socket. A blocked assignment returns HTTP 503 before a target connection is
attempted.

## Pool parsing

The core crate reads one local YAML document and validates:

- Listener protocol is HTTP, mixed, SOCKS, or SOCKS5.
- Listener host is loopback or a supported wildcard that is normalized to
  loopback.
- Listener authentication is empty.
- Listener target is a static entry in `proxies`.
- Every `dialer-proxy` hop is a static entry.
- The chain has no special target, missing entry, cycle, or more than eight
  hops.

The fingerprint covers the complete node definitions and hop order. Listener
names and ports identify entries; duplicate targets are counted separately.

## Health

Each entry receives two independent checks:

1. An HTTP CONNECT or SOCKS5 tunnel to target port 443.
2. An HTTP page request through target port 80.

The page check follows the existing account manager convention and is not a
TLS certificate or response-content validation. Results are explicitly named
`available`, `handshake_only`, and `unavailable`.

## Runtime state

The daemon reloads `sidecar.yaml` on each health interval and compares every
assignment with the latest entry fingerprint. A blocked state increments a
watch generation. Active byte-copy tasks subscribe to that generation and drop
both sockets when it changes.

The current release starts listeners from the configuration loaded at startup.
Adding or removing assignments requires a restart; pool updates do not.

## Windows process lifecycle

The Windows binary provides three process-lifecycle commands:

- `run` keeps the gateway in the foreground and exits on Ctrl+C.
- `tray` starts the same gateway and hosts a native Win32 notification icon.
- `install-autostart` / `uninstall-autostart` manage a single per-user Run
  entry.

The tray uses a hidden message-only-style native window and the Windows shell
notification API. It does not embed a browser, webview, Tauri runtime, or a
second service manager. The menu opens the selected configuration, generated
runtime status, or executable directory, and exposes a clean exit command.

Autostart is never enabled implicitly. The installer records an absolute
configuration path, writes a UTF-16 VBS launcher under `%LOCALAPPDATA%`, and
registers only `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ProxyGate`.
The launcher invokes `proxy-gate.exe tray` with a hidden window style. No HKLM,
Task Scheduler, Windows service, firewall, or system-proxy setting is changed.

## Account-manager integration

The Windows account manager uses a thin adapter instead of importing the proxy
runtime into its main process. The adapter invokes the versioned CLI contracts:

- `inspect --json` for the current entry set and fingerprints.
- `list --json` for automatic health probing and latency ordering.
- `assign` / `unassign` for provider-scoped account UUIDs.
- `run` for the account-manager-owned Gate process.

Assignment changes restart only the Gate process. Opening an account still
performs the existing proxy route preflight, so an unavailable Gate endpoint,
missing listener, or failed upstream blocks the WebView before navigation.
When the account switches back to a manual or static-pool proxy, the adapter
removes the old Gate assignment first.

The adapter and Gate deliberately avoid sharing one process across two owners.
If it detects an externally managed listener, it reports the conflict instead
of starting a duplicate Gateway.

## Remaining follow-up

A future Windows Named Pipe control plane can remove the restart requirement
for assignment changes and push blocked-state events directly to the account
manager. The existing CLI and runtime-status files remain the compatibility
fallback.

