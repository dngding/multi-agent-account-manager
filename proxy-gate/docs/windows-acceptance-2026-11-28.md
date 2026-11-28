# Windows acceptance - 2026-11-28

## Scope

This acceptance validates the independent Proxy Gate against:

- A real local `sidecar.yaml`.
- A synthetic SOCKS5 upstream with deterministic health responses.
- A real Windows x86-64 `proxy-gate.exe`.
- HTTP CONNECT from a loopback client.
- Fingerprint drift and active-connection interruption.
- A native Windows tray and its context menu.
- Temporary HKCU autostart installation, hidden launcher validation, and
  cleanup.
- The account-manager adapter's `inspect` / `assign` / `snapshot` /
  `unassign` contract.

It does not validate a production airport subscription, a production Mihomo
process, or a logged-in Muse account.

## Artifact

```text
proxy-gate.exe
size:   5,637,076 bytes
sha256: 5420D7C80E16CEB4A6B340B99E3B4BB4DF4A0CE63D6C97E033B46EA473B45A7B
```

## Real pool parsing

Source:

```text
C:/Users/<local-user>/WorkBuddy/2026-08-30-22-02-16/
telegram-panel/flclash-sidecar/sidecar.yaml
```

Result:

```text
entries:  210
revision: 83a9860c902b2ab84c04fc98cb0bc9d448daa863ecf724328886b37b7281b19b
first:    socks5:7891
last:     socks5:8180
```

At the time of the test, no Mihomo listeners were running, so the real pool
health pass correctly classified all entries as unavailable with
"代理服务器当前无法连接". No direct fallback was attempted.

## End-to-end synthetic pass

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke_phase3.ps1
```

Result:

```text
PASS initial=socks5:37982 drift=503 active_connection=closed
```

Verified behavior:

1. Proxy Gate read a synthetic `sidecar.yaml`.
2. It health-checked the SOCKS5 upstream as available.
3. The account assignment received stable endpoint `127.0.0.1:19100`.
4. HTTP CONNECT through Proxy Gate reached the synthetic upstream.
5. `ping -> pong` byte forwarding succeeded.
6. Changing the static node definition changed the chain fingerprint.
7. A new CONNECT request received HTTP 503.
8. The already-open tunnel was closed by the drift event.

## Windows tray

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke_phase4.ps1
```

Result:

```text
PASS tray_window=created menu=opened status=created process=running
```

The tray created its hidden native window, started the Gateway, published
`proxy-gate-status.json`, opened the right-click menu, and exited cleanly.

## Autostart

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\smoke_autostart.ps1
```

Result:

```text
PASS autostart=installed hidden_launcher=verified cleanup=verified
```

The test preserved and restored any existing HKCU value. No HKLM, Task
Scheduler, Windows service, firewall, or system proxy setting was changed.

## Account-manager adapter

The independent adapter harness runs four tests:

```text
account-ID provider scoping:          pass
proxy type parsing:                   pass
health ordering:                      pass
inspect / assign / snapshot / unassign: pass
```

The Windows account-manager target also passes `cargo check`, produces the
unit-test binary, and passes Clippy after exempting two pre-existing repository
warnings unrelated to Proxy Gate. The frontend passes `npm run build`.

## Automated checks

```text
cargo fmt --all --check: pass
cargo test --workspace: pass
proxy-core tests:         7/7
proxy-gate tests:         7/7
adapter harness tests:    4/4
Linux build:              pass
Windows x86-64 build:     pass
```

The full Windows account-manager IDE/installer is produced by the repository's
Windows packaging workflow. The loopback proxy runtime and its Windows binary
are fully built and exercised here; a logged-in production Muse session and a
live airport subscription remain environment-dependent acceptance steps.

