# Proxy Gate

Proxy Gate is an independent Windows proxy control plane for Mihomo / Clash-style
fixed listener pools. It reads a local `sidecar.yaml`, resolves static
`dialer-proxy` chains, probes proxy health, and exposes one stable local HTTP
CONNECT endpoint per account.

The current release provides:

- Read-only discovery and parsing of the existing local proxy pool.
- Whole-pool health checks and latency ordering.
- Fixed local listeners that fail closed when the bound entry disappears,
  changes fingerprint, or becomes unavailable.
- A native Windows tray and opt-in per-user autostart.
- A thin account-manager adapter with in-app selection, probing, assignment,
  restart, and open-time fail-closed checks.

It does not modify Mihomo, update subscriptions, change the system proxy, or
fall back to a direct connection.

## How it fits

```text
Account WebView
    |
    | http://127.0.0.1:19101
    v
Proxy Gate
    |
    | http:// or socks5:// listeners from sidecar.yaml
    v
Mihomo
    |
    | static dialer-proxy chain
    v
Muse / Meta
```

The account manager combines its provider ID and account UUID into a stable
Gate account ID. It stores the resulting `http://127.0.0.1:191xx` endpoint in
the account record and keeps the Gate assignment separate from the legacy
static-pool binding format.

## Build

```powershell
cargo test --workspace
cargo build --release -p maam-proxy-gate
```

Requires Rust 1.88 or newer.

## Quick start

Create a configuration and discover the local pool:

```powershell
cargo run -p maam-proxy-gate -- init
cargo run -p maam-proxy-gate -- discover
cargo run -p maam-proxy-gate -- inspect
```

Inspect and health-check all entries:

```powershell
cargo run -p maam-proxy-gate -- list
cargo run -p maam-proxy-gate -- list --json
```

Bind an account and allocate the first free port in `19100..=19199`:

```powershell
cargo run -p maam-proxy-gate -- assign account-uuid socks5:7902
```

The command prints the stable endpoint to enter in an account manager:

```text
account-uuid    http://127.0.0.1:19100    ...7902
```

Start the gate:

```powershell
cargo run -p maam-proxy-gate -- run
```

The runtime status is written next to the configuration as
`proxy-gate-status.json`.

On Windows, start the gate with a system-tray icon:

```powershell
cargo run -p maam-proxy-gate -- tray
```

The tray menu opens the configuration, status file, or program directory and
can stop the gate. Double-clicking the tray icon opens the status file.

The tray process and an account-manager-managed Gate are intentionally not
allowed to own the same configuration simultaneously. If the adapter detects a
listener started by the tray or another process, it reports that conflict
instead of spawning a duplicate process. Stop the tray instance before changing
assignments from the account manager.

Autostart is opt-in and uses only the current user's Run key:

```powershell
cargo run -p maam-proxy-gate -- install-autostart
cargo run -p maam-proxy-gate -- uninstall-autostart
```

The installer writes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\ProxyGate`
and an ASCII/UTF-16 VBS launcher under `%LOCALAPPDATA%\ProxyGate`. The Run
entry starts `wscript.exe` in hidden mode, then launches
`proxy-gate.exe tray` with the selected configuration's absolute path. This
avoids a console window at login. Moving the executable or configuration after
installation requires running `install-autostart` again.

## Health states

| State | Meaning | Default gate behavior |
| --- | --- | --- |
| `available` | The 443 proxy tunnel and HTTP page request both pass | Allow |
| `handshake_only` | The 443 tunnel passes but the HTTP page request fails | Allow; configurable |
| `unavailable` | The 443 tunnel fails | Reject with HTTP 503 |

Set `allowHandshakeOnly` to `false` to reject the middle state.

## Binding and weekly pool updates

An assignment stores the entry ID, label, port, and a fingerprint of the full
static chain. Every health interval the gate reloads `sidecar.yaml`.

- A newly added listener appears in `inspect` and `list`.
- An existing assignment does not switch to a new entry automatically.
- A removed entry blocks the corresponding local endpoint.
- A changed node definition or any changed `dialer-proxy` hop blocks the
  endpoint until the user explicitly runs `assign` again.
- Existing tunneled connections are closed when an assignment becomes blocked.

Changing assignments while `run` is active requires restarting Proxy Gate. The
sidecar pool itself is hot-reloaded without restart.

## Configuration

```json
{
  "version": 1,
  "poolPath": "C:/path/to/sidecar.yaml",
  "listenHost": "127.0.0.1",
  "portStart": 19100,
  "portEnd": 19199,
  "healthTarget": "https://muse.ai/",
  "healthIntervalSeconds": 60,
  "allowHandshakeOnly": true,
  "assignments": []
}
```

Only `127.0.0.1` and `::1` are accepted as control endpoints. Proxy entry
listeners in `sidecar.yaml` must be loopback or wildcard and must not require
authentication.

## Current scope

Implemented:

- Read-only WorkBuddy `sidecar.yaml` discovery.
- HTTP, mixed, SOCKS, and SOCKS5 fixed listener parsing.
- Static `dialer-proxy` chain validation up to eight hops.
- Stable fingerprints and duplicate target counts.
- HTTP CONNECT and SOCKS5 upstream tunnels.
- Whole-pool health checks with bounded concurrency.
- Stable per-account local ports and fail-closed runtime checks.
- Status snapshots and Windows/Linux CI.
- Windows tray controls for configuration, status, program directory, and exit.
- Opt-in per-user autostart through `HKCU\...\Run`.
- Proxy Gate mode in the Windows account manager.
- Automatic health probing ordered as `available`, `handshake_only`,
  `unavailable`, then undetected, with latency ordering inside each group.
- Account UUID synchronization and automatic restart after assignment changes.
- Old Gate assignment removal before switching an account to a manual or
  static-pool proxy.

Remaining follow-up:

- Local control API or Windows Named Pipe.
- Hot add/remove assignments without restarting the Gate process.
- Push-style blocked-state events to the account manager.

## Boundary

Proxy Gate isolates the configured proxy route. It does not provide physical
device isolation, hide shared Windows or WebView engine characteristics, prove
that two upstream entries have different public IP addresses, or protect
traffic from another application that does not use its local endpoint.

