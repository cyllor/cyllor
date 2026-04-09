param(
    [string]$Arch = "aarch64",
    [int]$TimeoutSec = 120
)

$ErrorActionPreference = "Stop"

if (-not (Get-Command make -ErrorAction SilentlyContinue)) {
    Write-Error "make not found in PATH"
    exit 2
}

$root = Split-Path -Parent $PSScriptRoot
$logDir = Join-Path $root "target"
if (-not (Test-Path $logDir)) {
    New-Item -ItemType Directory -Path $logDir | Out-Null
}
$logPath = Join-Path $logDir "xfce-smoke-$Arch.log"
if (Test-Path $logPath) {
    Remove-Item $logPath -Force
}

Write-Host "[xfce-smoke] Running make ARCH=$Arch run-rootfs ..."
$proc = Start-Process `
    -FilePath "make" `
    -ArgumentList @("ARCH=$Arch", "run-rootfs") `
    -WorkingDirectory $root `
    -RedirectStandardOutput $logPath `
    -RedirectStandardError $logPath `
    -PassThru

$deadline = (Get-Date).AddSeconds($TimeoutSec)
while (-not $proc.HasExited -and (Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
}

if (-not $proc.HasExited) {
    Write-Warning "[xfce-smoke] Timeout hit (${TimeoutSec}s), killing QEMU/make"
    Stop-Process -Id $proc.Id -Force
}

$log = if (Test-Path $logPath) { Get-Content -Raw $logPath } else { "" }

function Assert-Pattern([string]$Pattern, [string]$Message) {
    if ($log -notmatch $Pattern) {
        Write-Error $Message
        exit 1
    }
}

Assert-Pattern "ext4 root filesystem mounted" "rootfs mount marker missing"
Assert-Pattern "Init process: '/usr/bin/(startxfce4|xfce4-session|weston)'" "init desktop/compositor spawn marker missing"

$unimpl = ([regex]::Matches($log, "Unimplemented syscall")).Count
if ($unimpl -gt 0) {
    Write-Warning "[xfce-smoke] Found $unimpl unimplemented syscall log lines"
}

Write-Host "[xfce-smoke] PASS. log: $logPath"
exit 0
