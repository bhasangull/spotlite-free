$ErrorActionPreference = 'Stop'

$cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
if (-not (Test-Path -LiteralPath $cargo)) {
    throw 'Rust/Cargo bulunamadı.'
}

& $cargo test --locked
if ($LASTEXITCODE -ne 0) { throw 'Testler başarısız oldu.' }
$node = Join-Path $env:USERPROFILE '.cache\codex-runtimes\codex-primary-runtime\dependencies\node\bin\node.exe'
if (-not (Test-Path -LiteralPath $node)) { $node = (Get-Command node -ErrorAction Stop).Source }
& $node --test tests/playback.test.cjs tests/player-driver.test.cjs tests/control-driver.test.cjs tests/login-assist.test.cjs
if ($LASTEXITCODE -ne 0) { throw 'Oynatıcı testleri başarısız oldu.' }
& $cargo build --release --locked
if ($LASTEXITCODE -ne 0) {
    throw 'Derleme başarısız oldu.'
}

$output = Join-Path $PSScriptRoot 'outputs'
New-Item -ItemType Directory -Force -Path $output | Out-Null
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'target\release\spotlite-free.exe') -Destination (Join-Path $output 'spotlite-free-refresh.exe') -Force
Write-Host "Hazır: $output\spotlite-free-refresh.exe"
