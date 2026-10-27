<#
.SYNOPSIS
    Compile le compagnon Tailscale d'Eyrie (eyrie-tailnet.exe), à côté d'eyrie.exe.

.DESCRIPTION
    Le compagnon (dossier compagnon\) est écrit en Go avec tsnet : il fait d'Eyrie un appareil du tailnet
    personnel, sans toucher au client Tailscale du PC, et relaie le téléphone vers le pont mobile d'Eyrie.
    Go est cherché dans -Go, dans le PATH, puis dans %LOCALAPPDATA%\Programs\go (archive officielle
    décompressée, sans droits d'administrateur). -Release le place dans target\release, sinon dans
    target\debug. -Test lance d'abord ses essais.

.EXAMPLE
    tools\compagnon.ps1 -Release
#>
param(
    # Compagnon de la version release (sinon debug).
    [switch]$Release,
    # Lancer les essais du compagnon avant de le compiler.
    [switch]$Test,
    # Chemin de go.exe, s'il n'est ni dans le PATH ni dans %LOCALAPPDATA%\Programs\go.
    [string]$Go
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

if (-not $Go) { $Go = (Get-Command go -ErrorAction SilentlyContinue).Source }
if (-not $Go) {
    $local = Join-Path $env:LOCALAPPDATA 'Programs\go\bin\go.exe'
    if (Test-Path -LiteralPath $local) { $Go = $local }
}
if (-not $Go) {
    throw "Go introuvable : télécharger l'archive Windows sur https://go.dev/dl/ et la décompresser dans $env:LOCALAPPDATA\Programs."
}

$kind = if ($Release) { 'release' } else { 'debug' }
$out = Join-Path $repo "target\$kind\eyrie-tailnet.exe"
New-Item -ItemType Directory -Force (Split-Path $out) | Out-Null

# Jamais de téléchargement d'une autre version de Go en douce ; chemins du poste retirés du binaire.
$env:GOTOOLCHAIN = 'local'
Push-Location (Join-Path $repo 'compagnon')
try {
    if ($Test) {
        & $Go test ./...
        if ($LASTEXITCODE -ne 0) { throw "go test a échoué (code $LASTEXITCODE)." }
    }
    & $Go build -trimpath -ldflags '-s -w' -o $out .
    if ($LASTEXITCODE -ne 0) { throw "go build a échoué (code $LASTEXITCODE)." }
} finally {
    Pop-Location
}
Write-Host "Compagnon : $out"
