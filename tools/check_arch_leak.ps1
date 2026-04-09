#!/usr/bin/env pwsh
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent $PSScriptRoot
$kernelSrc = Join-Path $root "kernel/src"

if (-not (Get-Command rg -ErrorAction SilentlyContinue)) {
    Write-Error "rg (ripgrep) is required for leak checks."
}

Write-Host "Checking arch-decoupling leaks in common code..."

# Common code can depend on crate::arch abstractions, but not concrete arch modules/types.
$leakPatterns = @(
    "crate::arch::AddressSpace",
    "crate::arch::PageFlags",
    "crate::arch::aarch64",
    "crate::arch::x86_64"
)

$leaks = @()
foreach ($pattern in $leakPatterns) {
    $matches = rg $pattern $kernelSrc -g "!kernel/src/arch/**" 2>$null
    if ($LASTEXITCODE -eq 0 -and $matches) {
        $leaks += $matches
    }
}

if ($leaks.Count -gt 0) {
    Write-Host ""
    Write-Host "Found architecture-coupling leaks:" -ForegroundColor Red
    $leaks | Sort-Object -Unique | ForEach-Object { Write-Host $_ }
    exit 1
}

Write-Host "No concrete arch leaks found outside arch/." -ForegroundColor Green
exit 0
