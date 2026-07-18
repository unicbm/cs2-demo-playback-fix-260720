$ErrorActionPreference = 'Stop'

$repoRoot = $PSScriptRoot
$packageName = 'cs2-demo-playback-fix-windows-x64'
$distRoot = Join-Path $repoRoot 'dist'
$packageRoot = Join-Path $distRoot $packageName
$zipPath = Join-Path $distRoot "$packageName.zip"

Push-Location $repoRoot
try {
    cargo test --locked
    cargo build --release --locked

    if (Test-Path -LiteralPath $packageRoot) {
        Remove-Item -LiteralPath $packageRoot -Recurse -Force
    }
    if (Test-Path -LiteralPath $zipPath) {
        Remove-Item -LiteralPath $zipPath -Force
    }

    New-Item -ItemType Directory -Path $packageRoot | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $packageRoot 'docs') | Out-Null
    Copy-Item -LiteralPath (Join-Path $repoRoot 'target\release\cs2-demo-playback-fix.exe') -Destination $packageRoot
    Copy-Item -LiteralPath (Join-Path $repoRoot 'repair-demo.bat') -Destination $packageRoot
    Copy-Item -LiteralPath (Join-Path $repoRoot 'README.md') -Destination $packageRoot
    Copy-Item -LiteralPath (Join-Path $repoRoot 'docs\TYPE138_COMPATIBILITY.md') -Destination (Join-Path $packageRoot 'docs')
    Copy-Item -LiteralPath (Join-Path $repoRoot 'LICENSE') -Destination $packageRoot
    Copy-Item -LiteralPath (Join-Path $repoRoot 'NOTICE') -Destination $packageRoot
    Copy-Item -LiteralPath (Join-Path $repoRoot 'THIRD_PARTY_NOTICES.md') -Destination $packageRoot

    Compress-Archive -LiteralPath $packageRoot -DestinationPath $zipPath -CompressionLevel Optimal
    $exeSize = (Get-Item -LiteralPath (Join-Path $packageRoot 'cs2-demo-playback-fix.exe')).Length
    Write-Host "Executable size: $exeSize bytes"
    Write-Host "Created $zipPath"
}
finally {
    Pop-Location
}
