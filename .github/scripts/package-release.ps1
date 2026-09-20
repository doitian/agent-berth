param(
    [Parameter(Mandatory)]
    [string]$Target,
    [Parameter(Mandatory)]
    [string]$Version,
    [string]$Name = 'agent-berth',
    [string]$Binary = 'agent-berth',
    [string]$TargetDir = 'target',
    [string]$OutputDir = 'dist'
)

$ErrorActionPreference = 'Stop'
$extension = if ($Target -like '*-windows-*') { '.exe' } else { '' }
$binaryPath = Join-Path $TargetDir "$Target/release/$Binary$extension"
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) { throw "Missing binary: $binaryPath" }
$reported = & $binaryPath --version
if ($LASTEXITCODE -ne 0 -or $reported -cne "$Binary $Version") {
    throw "Binary version '$reported' does not match '$Binary $Version'"
}
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
$suffix = if ($extension) { '.zip' } else { '.tgz' }
$archive = Join-Path $OutputDir "$Name-$Target-v$Version$suffix"
$licensePath = Join-Path $PSScriptRoot '../../LICENSE'
$noticePath = Join-Path $OutputDir 'SOURCE.txt'
[IO.File]::WriteAllText($noticePath, "$Name $Version is licensed under MPL-2.0. See LICENSE.`nSource code: https://github.com/doitian/agent-berth/tree/v$Version`n")
if ($extension) {
    Compress-Archive -LiteralPath $binaryPath, $licensePath, $noticePath -DestinationPath $archive
} else {
    tar -czf $archive -C (Split-Path $binaryPath) "$Binary$extension" -C (Resolve-Path (Split-Path $licensePath)).Path LICENSE -C (Resolve-Path $OutputDir).Path SOURCE.txt
    if ($LASTEXITCODE -ne 0) { throw 'Archive creation failed' }
}
Remove-Item -LiteralPath $noticePath
$hash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
[IO.File]::WriteAllText("$archive.sha256", "$hash  $([IO.Path]::GetFileName($archive))`n")
