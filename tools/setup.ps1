<#
.SYNOPSIS
    Prépare un clone neuf pour la compilation (toolchain Rust x86_64-pc-windows-gnu).

.DESCRIPTION
    Les crates windows récentes (raw-dylib) exigent un dlltool : llvm-ar (composant rustup
    llvm-tools) renommé en tools\dlltool.exe fait l'affaire. On lie avec rust-lld, car ld (binutils)
    lit mal les import libs au format LLVM. Les chemins de ces outils sont propres au poste : ce
    script écrit .cargo\config.toml (non versionné) d'après la toolchain active. Avec la toolchain
    MSVC (Build Tools C++ installés), il n'y a rien à faire.

.EXAMPLE
    tools\setup.ps1
    puis : cargo build --release
#>
param(
    # Réécrit .cargo\config.toml s'il existe déjà avec un autre contenu.
    [switch]$Force
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

$target = 'x86_64-pc-windows-gnu'
$sysroot = (rustc --print sysroot).Trim()
if ($LASTEXITCODE -ne 0 -or -not $sysroot) { throw 'rustc introuvable : installer Rust (https://rustup.rs).' }
$host_triple = ((rustc -vV) | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
if ($host_triple -ne $target) {
    Write-Host "Toolchain $host_triple : rien à préparer (ce script ne sert qu'à $target)."
    return
}

$bin = Join-Path $sysroot "lib\rustlib\$target\bin"
$llvmAr = Join-Path $bin 'llvm-ar.exe'
if (-not (Test-Path -LiteralPath $llvmAr)) {
    Write-Host 'Ajout du composant rustup llvm-tools…'
    rustup component add llvm-tools
    if ($LASTEXITCODE -ne 0) { throw 'rustup component add llvm-tools a échoué.' }
}
$lld = Join-Path $bin 'rust-lld.exe'
foreach ($tool in $llvmAr, $lld) {
    if (-not (Test-Path -LiteralPath $tool)) { throw "Introuvable : $tool" }
}

$dlltool = Join-Path $repo 'tools\dlltool.exe'
if (-not (Test-Path -LiteralPath $dlltool) -or (Get-FileHash $dlltool).Hash -ne (Get-FileHash $llvmAr).Hash) {
    Copy-Item -LiteralPath $llvmAr -Destination $dlltool -Force
    Write-Host "tools\dlltool.exe copié depuis $llvmAr"
}

$slash = { param($p) $p.Replace('\', '/') }
$config = @"
# Écrit par tools\setup.ps1 (chemins propres à ce poste, non versionné).
# La toolchain $target a besoin d'un dlltool pour les crates ``windows-link``
# (raw-dylib). Celui livré par rustup exige un assembleur absent ; on utilise donc llvm-ar
# (composant rustup ``llvm-tools``) renommé en dlltool.exe, et on lie avec rust-lld car ld (binutils)
# lit mal les import libs au format LLVM. Voir README.md > Compilation.
[target.$target]
linker = "$(& $slash $lld)"
rustflags = [
  "-C", "linker-flavor=ld.lld",
  "-C", "link-self-contained=yes",
  "-C", "dlltool=$(& $slash $dlltool)",
]
"@
$configPath = Join-Path $repo '.cargo\config.toml'
$current = if (Test-Path -LiteralPath $configPath) { [IO.File]::ReadAllText($configPath) } else { $null }
if ($current -eq $config) {
    Write-Host '.cargo\config.toml déjà à jour.'
} elseif ($null -ne $current -and -not $Force) {
    throw ".cargo\config.toml existe déjà avec un autre contenu : relancer avec -Force pour le réécrire."
} else {
    New-Item -ItemType Directory -Force (Split-Path $configPath) | Out-Null
    [IO.File]::WriteAllText($configPath, $config, (New-Object Text.UTF8Encoding $false))
    Write-Host ".cargo\config.toml écrit."
}
Write-Host 'Prêt : cargo build --release'
