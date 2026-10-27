<#
.SYNOPSIS
    Construit l'application Android d'Eyrie (APK signé), et l'installe sur le téléphone branché si -Install.

.DESCRIPTION
    Sans Gradle ni Android Studio :
    1. la bibliothèque native (crate eyrie-mobile, Slint) est compilée pour Android avec le NDK ;
    2. les ressources (manifeste, icône adaptative tirée du nid d'Eyrie) sont liées par aapt2 ;
    3. la bibliothèque est ajoutée à l'APK, aligné (zipalign) puis signé (apksigner).

    Prérequis (sans droits d'administrateur) : le SDK Android dans %LOCALAPPDATA%\Android\Sdk (ANDROID_HOME) avec
    platforms;android-36, build-tools;36.1.0, platform-tools et un NDK (ndk;28.x), un JDK 17
    (%LOCALAPPDATA%\Android\jdk-17 ou JAVA_HOME), et la cible Rust aarch64-linux-android
    (rustup target add aarch64-linux-android). Les licences du SDK s'acceptent une fois :
    sdkmanager --licenses.

    La clé de signature est celle de débogage d'Android (%USERPROFILE%\.android\debug.keystore), créée au besoin :
    les versions suivantes s'installent par-dessus sans perdre l'appairage.

.EXAMPLE
    tools\android.ps1 -Release -Install
#>
param(
    [switch]$Release,
    # Installer sur le téléphone branché (débogage USB) ou l'émulateur (adb).
    [switch]$Install,
    # x86_64 pour l'émulateur ; arm64-v8a pour les téléphones.
    [ValidateSet('arm64-v8a', 'x86_64')]
    [string]$Abi = 'arm64-v8a'
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent

$sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
if (-not (Test-Path -LiteralPath $sdk)) { throw "SDK Android introuvable : $sdk (voir l'aide du script)." }
if (-not $env:JAVA_HOME) {
    $jdk = Join-Path $env:LOCALAPPDATA 'Android\jdk-17'
    if (Test-Path -LiteralPath $jdk) { $env:JAVA_HOME = $jdk }
}
if (-not $env:JAVA_HOME) { throw 'JDK introuvable : indiquez JAVA_HOME (JDK 17).' }
$env:ANDROID_HOME = $sdk
$env:ANDROID_SDK_ROOT = $sdk
$env:PATH = "$env:JAVA_HOME\bin;$env:PATH"

$ndk = Get-ChildItem (Join-Path $sdk 'ndk') -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1
if (-not $ndk) { throw "NDK introuvable dans $sdk\ndk (sdkmanager --install `"ndk;28.2.13676358`")." }
$env:ANDROID_NDK_ROOT = $ndk.FullName
$env:ANDROID_NDK_HOME = $ndk.FullName
$tools = Get-ChildItem (Join-Path $sdk 'build-tools') -Directory | Sort-Object { [version]($_.Name -replace '-.*', '') } -Descending | Select-Object -First 1
$platform = Get-ChildItem (Join-Path $sdk 'platforms') -Directory | Where-Object { Test-Path (Join-Path $_.FullName 'android.jar') } |
    Sort-Object { [int]($_.Name -replace '^android-(\d+).*$', '$1') } -Descending | Select-Object -First 1
if (-not $tools -or -not $platform) { throw 'build-tools ou platforms manquants dans le SDK.' }
$androidJar = Join-Path $platform.FullName 'android.jar'

# --- 1. La bibliothèque native ---
$target = if ($Abi -eq 'arm64-v8a') { 'aarch64-linux-android' } else { 'x86_64-linux-android' }
$api = 26
$llvm = Join-Path $ndk.FullName 'toolchains\llvm\prebuilt\windows-x86_64\bin'
$clang = Join-Path $llvm "$target$api-clang.cmd"
$envTarget = $target.ToUpper().Replace('-', '_')
Set-Item "env:CARGO_TARGET_${envTarget}_LINKER" $clang
Set-Item "env:CC_$($target.Replace('-', '_'))" $clang
Set-Item "env:AR_$($target.Replace('-', '_'))" (Join-Path $llvm 'llvm-ar.exe')
$kind = if ($Release) { 'release' } else { 'debug' }
$cargoArgs = @('build', '-p', 'eyrie-mobile', '--lib', '--target', $target, '-j', '4')
if ($Release) { $cargoArgs += '--release' }
Push-Location $repo
try {
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build a échoué (code $LASTEXITCODE)." }
} finally {
    Pop-Location
}
$so = Join-Path $repo "target\$target\$kind\libeyrie_mobile.so"

# --- 2. Les ressources ---
$work = Join-Path $repo "target\android\$kind"
Remove-Item $work -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force "$work\res", "$work\flat", "$work\lib\$Abi" | Out-Null
Copy-Item (Join-Path $repo 'crates\eyrie-mobile\android\res\*') "$work\res" -Recurse
# Les icônes : le nid d'Eyrie, dessiné par l'application de bureau (même code que dans l'application).
Push-Location $repo
try {
    & cargo run -q -p eyrie-mobile --bin eyrie-mobile -- --icones "$work\res"
    if ($LASTEXITCODE -ne 0) { throw 'icônes : échec.' }
} finally {
    Pop-Location
}
$aapt2 = Join-Path $tools.FullName 'aapt2.exe'
& $aapt2 compile --dir "$work\res" -o "$work\flat\res.zip"
if ($LASTEXITCODE -ne 0) { throw 'aapt2 compile a échoué.' }
$unsigned = "$work\eyrie-unsigned.apk"
& $aapt2 link -o $unsigned -I $androidJar --manifest (Join-Path $repo 'crates\eyrie-mobile\android\AndroidManifest.xml') `
    --min-sdk-version $api --target-sdk-version 35 "$work\flat\res.zip"
if ($LASTEXITCODE -ne 0) { throw 'aapt2 link a échoué.' }

# --- 3. La bibliothèque dans l'APK, alignement, signature ---
Copy-Item $so "$work\lib\$Abi\libeyrie_mobile.so"
Add-Type -AssemblyName System.IO.Compression, System.IO.Compression.FileSystem
$zip = [System.IO.Compression.ZipFile]::Open($unsigned, 'Update')
try {
    [void][System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, "$work\lib\$Abi\libeyrie_mobile.so", "lib/$Abi/libeyrie_mobile.so")
} finally {
    $zip.Dispose()
}
$aligned = "$work\eyrie-aligned.apk"
& (Join-Path $tools.FullName 'zipalign.exe') -f -p 4 $unsigned $aligned
if ($LASTEXITCODE -ne 0) { throw 'zipalign a échoué.' }

$ks = Join-Path $env:USERPROFILE '.android\debug.keystore'
if (-not (Test-Path -LiteralPath $ks)) {
    New-Item -ItemType Directory -Force (Split-Path $ks) | Out-Null
    & keytool -genkeypair -v -keystore $ks -storepass android -keypass android -alias androiddebugkey `
        -keyalg RSA -keysize 2048 -validity 10000 -dname 'CN=Android Debug,O=Android,C=US' | Out-Null
    if ($LASTEXITCODE -ne 0) { throw 'keytool a échoué.' }
}
$apk = Join-Path $repo "target\android\eyrie-$kind.apk"
& (Join-Path $tools.FullName 'apksigner.bat') sign --ks $ks --ks-pass pass:android --ks-key-alias androiddebugkey `
    --key-pass pass:android --out $apk $aligned
if ($LASTEXITCODE -ne 0) { throw 'apksigner a échoué.' }
Write-Host "APK : $apk ($([math]::Round((Get-Item $apk).Length / 1MB, 1)) Mo)"

if ($Install) {
    $adb = Join-Path $sdk 'platform-tools\adb.exe'
    & $adb install -r $apk
    if ($LASTEXITCODE -ne 0) { throw 'adb install a échoué (téléphone branché, débogage USB autorisé ?).' }
    & $adb shell am start -n 'io.github.aubinched.eyrie/android.app.NativeActivity' | Out-Null
}
