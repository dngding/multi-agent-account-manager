param(
    [string]$Exe = "target\x86_64-pc-windows-gnu\release\proxy-gate.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$smoke = Join-Path $root "smoke\phase4"
New-Item -ItemType Directory -Path $smoke -Force | Out-Null

$exePath = (Resolve-Path (Join-Path $root $Exe)).Path
$sidecarPath = Join-Path $smoke "sidecar.yaml"
$configPath = Join-Path $smoke "proxy-gate.json"
$statusPath = Join-Path $smoke "proxy-gate-status.json"

@'
proxies: []
listeners: []
'@ | Set-Content -LiteralPath $sidecarPath -Encoding ASCII

$config = [ordered]@{
    version = 1
    poolPath = $sidecarPath
    listenHost = "127.0.0.1"
    portStart = 19100
    portEnd = 19199
    healthTarget = "https://muse.ai/"
    healthIntervalSeconds = 60
    allowHandshakeOnly = $true
    assignments = @()
}
$config | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $configPath -Encoding ASCII

if (Test-Path -LiteralPath $statusPath) {
    Remove-Item -LiteralPath $statusPath -Force
}

Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class ProxyGateTrayNative {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    public static extern IntPtr FindWindow(string className, string windowName);

    [DllImport("user32.dll")]
    public static extern bool PostMessage(IntPtr window, uint message, IntPtr wParam, IntPtr lParam);
}
'@

$process = Start-Process -FilePath $exePath -ArgumentList @(
    "tray",
    "--config",
    ('"' + $configPath + '"')
) -PassThru -WindowStyle Hidden

try {
    $deadline = [DateTime]::UtcNow.AddSeconds(15)
    $window = [IntPtr]::Zero
    do {
        Start-Sleep -Milliseconds 250
        $window = [ProxyGateTrayNative]::FindWindow("ProxyGate.TrayWindow", "Proxy Gate")
    } while ($window -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $deadline)

    if ($window -eq [IntPtr]::Zero) {
        throw "Tray window was not created"
    }
    if (-not (Test-Path -LiteralPath $statusPath)) {
        throw "Status snapshot was not created"
    }
    if ($process.HasExited) {
        throw "Tray process exited unexpectedly"
    }

    [void][ProxyGateTrayNative]::PostMessage(
        $window,
        (0x8000 + 1),
        [IntPtr]1,
        [IntPtr]0x0205
    )
    $menuDeadline = [DateTime]::UtcNow.AddSeconds(5)
    $menu = [IntPtr]::Zero
    do {
        Start-Sleep -Milliseconds 100
        $menu = [ProxyGateTrayNative]::FindWindow("#32768", $null)
    } while ($menu -eq [IntPtr]::Zero -and [DateTime]::UtcNow -lt $menuDeadline)
    if ($menu -eq [IntPtr]::Zero) {
        throw "Tray context menu did not open"
    }
    [void][ProxyGateTrayNative]::PostMessage($menu, 0x0100, [IntPtr]0x1B, [IntPtr]::Zero)

    Write-Output "PASS tray_window=created menu=opened status=created process=running"
}
finally {
    $menu = [ProxyGateTrayNative]::FindWindow("#32768", $null)
    if ($menu -ne [IntPtr]::Zero) {
        [void][ProxyGateTrayNative]::PostMessage($menu, 0x0100, [IntPtr]0x1B, [IntPtr]::Zero)
    }
    $window = [ProxyGateTrayNative]::FindWindow("ProxyGate.TrayWindow", "Proxy Gate")
    if ($window -ne [IntPtr]::Zero) {
        [void][ProxyGateTrayNative]::PostMessage($window, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
    }
    if (-not $process.WaitForExit(5000)) {
        Stop-Process -Id $process.Id -Force
        throw "Tray did not exit after WM_CLOSE"
    }
    if ($process.ExitCode -ne 0) {
        throw "Tray exited with code $($process.ExitCode)"
    }
}
