param(
    [Parameter(Mandatory)]
    [string]$Tag
)

$ErrorActionPreference = 'Stop'
$metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
if ($LASTEXITCODE -ne 0) { throw 'Failed to read Cargo metadata' }
$manifest = (Resolve-Path 'Cargo.toml').Path
$package = @($metadata.packages | Where-Object { $_.manifest_path -eq $manifest })
if ($package.Count -ne 1) { throw 'Expected one package in the root Cargo.toml' }
$version = $package[0].version
if ($Tag -cne "v$version") {
    throw "Release tag '$Tag' does not match Cargo.toml version '$version'; expected 'v$version'"
}
Write-Output "Validated $($package[0].name) $version against $Tag"
if ($env:GITHUB_OUTPUT) {
    [IO.File]::AppendAllText($env:GITHUB_OUTPUT, "version=$version`nname=$($package[0].name)`n")
}
