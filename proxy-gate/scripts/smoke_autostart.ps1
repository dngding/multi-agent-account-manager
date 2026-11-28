param(
    [string]$Exe = "target\x86_64-pc-windows-gnu\release\proxy-gate.exe"
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$smoke = Join-Path $root "smoke\phase4"
$exePath = (Resolve-Path (Join-Path $root $Exe)).Path
$configPath = Join-Path $smoke "proxy-gate.json"

if (-not (Test-Path -LiteralPath $configPath)) {
    throw "Run scripts\smoke_phase4.ps1 first"
}

$runKeyPath = "Software\Microsoft\Windows\CurrentVersion\Run"
$launcherPath = Join-Path $env:LOCALAPPDATA "ProxyGate\proxy-gate-tray.vbs"
$runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runKeyPath, $true)
if ($null -eq $runKey) {
    throw "Cannot open HKCU Run key"
}

$oldValue = $runKey.GetValue("ProxyGate", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
$oldKind = if ($null -ne $oldValue) {
    $runKey.GetValueKind("ProxyGate")
} else {
    $null
}
$oldLauncher = if (Test-Path -LiteralPath $launcherPath) {
    [System.IO.File]::ReadAllBytes($launcherPath)
} else {
    $null
}
$runKey.Close()

try {
    & $exePath install-autostart --config $configPath
    if ($LASTEXITCODE -ne 0) {
        throw "install-autostart failed"
    }

    $runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runKeyPath)
    $command = $runKey.GetValue("ProxyGate", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $runKey.Close()
    if ($null -eq $command -or $command -notmatch "wscript\.exe //B //NoLogo") {
        throw "Run entry does not use the hidden launcher"
    }
    if (-not (Test-Path -LiteralPath $launcherPath)) {
        throw "Launcher file was not created"
    }
    $launcherText = Get-Content -LiteralPath $launcherPath -Encoding Unicode -Raw
    if ($launcherText -notmatch "tray --config" -or $launcherText -notmatch ", 0, False") {
        throw "Launcher does not start the tray hidden"
    }

    & $exePath uninstall-autostart
    if ($LASTEXITCODE -ne 0) {
        throw "uninstall-autostart failed"
    }
    $runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runKeyPath)
    $removed = $null -eq $runKey.GetValue("ProxyGate", $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
    $runKey.Close()
    if (-not $removed -or (Test-Path -LiteralPath $launcherPath)) {
        throw "Autostart cleanup failed"
    }

    Write-Output "PASS autostart=installed hidden_launcher=verified cleanup=verified"
}
finally {
    $runKey = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($runKeyPath, $true)
    if ($null -ne $oldValue) {
        $runKey.SetValue("ProxyGate", $oldValue, $oldKind)
    } else {
        $runKey.DeleteValue("ProxyGate", $false)
    }
    $runKey.Close()

    $launcherDirectory = Split-Path -Parent $launcherPath
    if ($null -ne $oldLauncher) {
        New-Item -ItemType Directory -Path $launcherDirectory -Force | Out-Null
        [System.IO.File]::WriteAllBytes($launcherPath, $oldLauncher)
    } elseif (Test-Path -LiteralPath $launcherPath) {
        Remove-Item -LiteralPath $launcherPath -Force
    }
}
