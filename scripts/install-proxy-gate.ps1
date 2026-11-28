param(
    [string]$GateExe = "",
    [string]$PoolPath = ""
)

$ErrorActionPreference = "Stop"
$repoRoot = Split-Path -Parent $PSScriptRoot
$appData = Join-Path $env:APPDATA "com.ldjx.accountmanager"
$destination = Join-Path $appData "proxy-gate.exe"
$configPath = Join-Path $appData "proxy-gate.json"

if (-not $GateExe) {
    $candidates = @(
        (Join-Path $repoRoot "proxy-gate\target\release\proxy-gate.exe"),
        (Join-Path $repoRoot "proxy-gate\target\x86_64-pc-windows-msvc\release\proxy-gate.exe"),
        (Join-Path $repoRoot "proxy-gate\target\x86_64-pc-windows-gnu\release\proxy-gate.exe"),
        (Join-Path $repoRoot "proxy-gate\dist\proxy-gate-windows-x64\proxy-gate.exe"),
        (Join-Path (Split-Path -Parent $repoRoot) "proxy-gate\target\release\proxy-gate.exe"),
        (Join-Path (Split-Path -Parent $repoRoot) "proxy-gate\target\x86_64-pc-windows-msvc\release\proxy-gate.exe"),
        (Join-Path (Split-Path -Parent $repoRoot) "proxy-gate\target\x86_64-pc-windows-gnu\release\proxy-gate.exe"),
        (Join-Path (Split-Path -Parent $repoRoot) "proxy-gate\dist\proxy-gate-windows-x64\proxy-gate.exe")
    )
    $GateExe = $candidates | Where-Object { Test-Path -LiteralPath $_ } | Select-Object -First 1
}

if (-not $GateExe) {
    throw "proxy-gate.exe was not found. Pass -GateExe with an absolute path."
}

$gateExePath = (Resolve-Path -LiteralPath $GateExe).Path
New-Item -ItemType Directory -Path $appData -Force | Out-Null
Copy-Item -LiteralPath $gateExePath -Destination $destination -Force

if (-not (Test-Path -LiteralPath $configPath)) {
    if (-not $PoolPath) {
        $sources = @(& $destination discover | Where-Object { $_.Trim() })
        if ($sources.Count -ne 1) {
            throw "Pass -PoolPath explicitly. Auto-discovery found $($sources.Count) sidecar.yaml files."
        }
        $PoolPath = $sources[0].Trim()
    }

    $resolvedPool = (Resolve-Path -LiteralPath $PoolPath).Path
    & $destination --config $configPath inspect --pool $resolvedPool --json | Out-Null
    if ($LASTEXITCODE -ne 0) {
        throw "proxy-gate could not initialize the selected pool."
    }
}

$config = Get-Content -LiteralPath $configPath -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $config.poolPath) {
    throw "proxy-gate.json has no poolPath. Rerun this script with -PoolPath."
}

Write-Output "Proxy Gate installed: $destination"
Write-Output "Configuration: $configPath"
Write-Output "Pool: $($config.poolPath)"
Write-Output "Open the account manager and choose Proxy Gate when editing an account proxy."
