$ErrorActionPreference = 'Stop'

$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargo)) {
    throw 'Rust/Cargo bulunamadı.'
}

& $cargo build --release
if ($LASTEXITCODE -ne 0) {
    throw 'Derleme başarısız oldu.'
}

$output = Join-Path $PSScriptRoot 'outputs'
New-Item -ItemType Directory -Force -Path $output | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'target\release\spotlite-free.exe') -Destination $output -Force
Write-Host "Hazır: $output\spotlite-free.exe"
