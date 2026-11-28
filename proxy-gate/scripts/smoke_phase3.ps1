param(
    [string]$Exe = "target\x86_64-pc-windows-gnu\release\proxy-gate.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$smoke = Join-Path $root "smoke\phase3"
New-Item -ItemType Directory -Path $smoke -Force | Out-Null

$exePath = (Resolve-Path (Join-Path $root $Exe)).Path
$mockScript = Join-Path $root "scripts\mock_socks5.py"
$pythonwCommand = Get-Command pythonw.exe -ErrorAction SilentlyContinue
if ($pythonwCommand) {
    $pythonw = $pythonwCommand.Source
} else {
    $pythonCommand = Get-Command python.exe -ErrorAction SilentlyContinue
    if (-not $pythonCommand) {
        throw "Neither pythonw.exe nor python.exe was found on PATH."
    }
    $pythonw = $pythonCommand.Source
}

$mockPort = 37982
$gatePort = 19100
$mockProcess = $null
$gateProcess = $null

function Write-Utf8NoBom {
    param([string]$Path, [string]$Content)
    [IO.File]::WriteAllText($Path, $Content, [Text.UTF8Encoding]::new($false))
}

function Get-FreePortCheck {
    param([int]$Port)
    $listener = $null
    try {
        $listener = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
        $listener.Start()
    } catch {
        throw "Port $Port is already in use"
    } finally {
        if ($listener) {
            $listener.Stop()
        }
    }
}

function Write-Sidecar {
    param([string]$Password)
    $content = @"
proxies:
  - name: fixed
    type: ss
    server: mock.example.invalid
    port: 443
    cipher: aes-128-gcm
    password: $Password
listeners:
  - name: smoke
    type: socks
    listen: 127.0.0.1
    port: $mockPort
    proxy: fixed
"@
    Write-Utf8NoBom -Path (Join-Path $smoke "sidecar.yaml") -Content $content
}

function Open-GateConnection {
    $client = [System.Net.Sockets.TcpClient]::new()
    $client.ReceiveTimeout = 5000
    $client.Connect("127.0.0.1", $gatePort)
    $stream = $client.GetStream()
    $request = [Text.Encoding]::ASCII.GetBytes("CONNECT example.test:443 HTTP/1.1`r`nHost: example.test:443`r`n`r`n")
    $stream.Write($request, 0, $request.Length)
    $bytes = [Collections.Generic.List[byte]]::new()
    while ($true) {
        $value = $stream.ReadByte()
        if ($value -lt 0) {
            break
        }
        $bytes.Add([byte]$value)
        if ($bytes.Count -ge 4 -and
            $bytes[$bytes.Count - 4] -eq 13 -and
            $bytes[$bytes.Count - 3] -eq 10 -and
            $bytes[$bytes.Count - 2] -eq 13 -and
            $bytes[$bytes.Count - 1] -eq 10) {
            break
        }
    }
    return [pscustomobject]@{
        Client = $client
        Stream = $stream
        Header = [Text.Encoding]::ASCII.GetString($bytes.ToArray())
    }
}

try {
    Get-FreePortCheck -Port $mockPort
    Get-FreePortCheck -Port $gatePort
    Write-Sidecar -Password "one"

    $mockProcess = Start-Process -FilePath $pythonw -ArgumentList @($mockScript, "--port", $mockPort) -WindowStyle Hidden -PassThru
    $mockReady = $false
    $mockDeadline = (Get-Date).AddSeconds(8)
    do {
        Start-Sleep -Milliseconds 200
        $probe = [System.Net.Sockets.TcpClient]::new()
        try {
            $probe.Connect("127.0.0.1", $mockPort)
            $mockReady = $true
        } catch {
        } finally {
            $probe.Close()
        }
    } until ($mockReady -or (Get-Date) -gt $mockDeadline)
    if (-not $mockReady) {
        throw "Mock SOCKS5 proxy did not start"
    }

    $config = Join-Path $smoke "proxy-gate.json"
    $pool = Join-Path $smoke "sidecar.yaml"
    if (Test-Path -LiteralPath $config) {
        Remove-Item -LiteralPath $config -Force
    }
    & $exePath --config $config inspect --pool $pool --json | Out-Null
    $parsedConfig = Get-Content -LiteralPath $config -Raw -Encoding UTF8 | ConvertFrom-Json
    $parsedConfig.healthIntervalSeconds = 1
    Write-Utf8NoBom -Path $config -Content ($parsedConfig | ConvertTo-Json -Depth 8)
    & $exePath --config $config assign smoke-user "socks5:$mockPort" | Out-Null

    $gateOut = Join-Path $smoke "gate.out.log"
    $gateErr = Join-Path $smoke "gate.err.log"
    $gateProcess = Start-Process -FilePath $exePath -ArgumentList @("--config", $config, "run") -WindowStyle Hidden -RedirectStandardOutput $gateOut -RedirectStandardError $gateErr -PassThru

    $statusPath = Join-Path $smoke "proxy-gate-status.json"
    $deadline = (Get-Date).AddSeconds(15)
    do {
        Start-Sleep -Milliseconds 250
        $ready = Test-Path -LiteralPath $statusPath
    } until ($ready -or (Get-Date) -gt $deadline)
    if (-not $ready) {
        throw "Proxy Gate did not publish status"
    }
    $status = Get-Content -LiteralPath $statusPath -Raw -Encoding UTF8 | ConvertFrom-Json
    if (-not $status.assignments[0].allowed) {
        throw "Initial assignment was blocked: $($status.assignments[0].error)"
    }

    $first = Open-GateConnection
    if (-not $first.Header.StartsWith("HTTP/1.1 200")) {
        throw "Expected 200, got: $($first.Header)"
    }
    $ping = [Text.Encoding]::ASCII.GetBytes("ping")
    $first.Stream.Write($ping, 0, $ping.Length)
    $pong = New-Object byte[] 4
    $offset = 0
    while ($offset -lt 4) {
        $read = $first.Stream.Read($pong, $offset, 4 - $offset)
        if ($read -le 0) {
            break
        }
        $offset += $read
    }
    if ($offset -ne 4 -or [Text.Encoding]::ASCII.GetString($pong) -ne "pong") {
        throw "Tunnel echo failed: read=$offset bytes=$([BitConverter]::ToString($pong))"
    }

    $held = Open-GateConnection
    Write-Sidecar -Password "changed"
    Start-Sleep -Seconds 3

    $blocked = Open-GateConnection
    if (-not $blocked.Header.StartsWith("HTTP/1.1 503")) {
        throw "Expected 503 after fingerprint drift, got: $($blocked.Header)"
    }

    $held.Stream.ReadTimeout = 3000
    try {
        $closed = $held.Stream.ReadByte()
    } catch {
        $closed = -1
    }
    if ($closed -ne -1) {
        throw "Existing tunnel was not closed after drift"
    }

    $blocked.Client.Close()
    $held.Client.Close()
    $first.Client.Close()
    "PASS initial=$($status.assignments[0].entryId) drift=503 active_connection=closed"
}
finally {
    if ($gateProcess -and -not $gateProcess.HasExited) {
        Stop-Process -Id $gateProcess.Id -Force -ErrorAction SilentlyContinue
    }
    if ($mockProcess -and -not $mockProcess.HasExited) {
        Stop-Process -Id $mockProcess.Id -Force -ErrorAction SilentlyContinue
    }
}
