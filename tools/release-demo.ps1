param(
    [Parameter(Mandatory)][string] $OutputDirectory,
    [Parameter(Mandatory)][string] $Sdk,
    [Parameter(Mandatory)][string] $Jdk,
    [Parameter(Mandatory)][string] $Codec,
    [Parameter(Mandatory)][string] $CodecLicense,
    [Parameter(Mandatory)][string] $Keystore,
    [Parameter(Mandatory)][ValidateRange(1, 2100000000)][int] $AndroidVersionCode,
    [string] $DkpRoot = "C:\Users\Ryan\AppData\Local\Blotworx Publisher\3ds-sdk\opt\devkitpro",
    [string] $ReleaseNotes,
    [switch] $Publish
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot/..").Path
if ((rustc -vV) -notcontains 'host: x86_64-pc-windows-msvc') {
    throw 'Build releases on an x64 Windows MSVC Rust host.'
}
if (!$env:BLOTWORX_SIGNING_PASSWORD) { throw 'Set BLOTWORX_SIGNING_PASSWORD before building.' }
if ($Publish -and !$ReleaseNotes) { throw 'Provide a release notes file when publishing.' }
if ($Publish -and !$env:GH_TOKEN) { throw 'Set GH_TOKEN for the publishing step.' }
$notes = if ($Publish) { [string](Get-Content -LiteralPath $ReleaseNotes -Raw) }
$Sdk = (Resolve-Path -LiteralPath $Sdk).Path
$ndk = Join-Path $Sdk 'ndk/27.3.13750724/toolchains/llvm/prebuilt/windows-x86_64/bin'
$android = @{
    Sdk = $Sdk
    Jdk = (Resolve-Path -LiteralPath $Jdk).Path
    Codec = (Resolve-Path -LiteralPath $Codec).Path
    CodecLicense = (Resolve-Path -LiteralPath $CodecLicense).Path
    Keystore = (Resolve-Path -LiteralPath $Keystore).Path
    VersionCode = $AndroidVersionCode
}
$commit = git -C $root rev-parse HEAD
if ($LASTEXITCODE -ne 0) { throw 'Cannot resolve source commit.' }
if (git -C $root status --porcelain) { throw 'Build releases from a clean checkout.' }
$version = [regex]::Match((Get-Content "$root/Cargo.toml" -Raw), '(?m)^version = "([^"]+)"').Groups[1].Value
if (!$version) { throw 'Blotworx package version is missing.' }
$env:CARGO_TARGET_DIR = Join-Path $root 'target'
$output = (New-Item -ItemType Directory -Force -Path $OutputDirectory).FullName
if (Get-ChildItem -LiteralPath $output -Force) { throw 'The release output directory must be empty.' }
$apkName = "Blotworx-$version-Android-arm64.apk"
$android.Output = Join-Path $output $apkName
$android.Engine = Join-Path $env:CARGO_TARGET_DIR 'aarch64-linux-android/debug/libblotworx.so'
$env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = "$ndk/aarch64-linux-android26-clang.cmd"
$env:CC_aarch64_linux_android = "$ndk/aarch64-linux-android26-clang.cmd"
$env:CXX_aarch64_linux_android = "$ndk/aarch64-linux-android26-clang++.cmd"
$env:AR_aarch64_linux_android = "$ndk/llvm-ar.exe"

Push-Location $root
try {
    cargo build --locked --bin blotworx --target x86_64-pc-windows-msvc
    if ($LASTEXITCODE -ne 0) { throw 'Windows build failed.' }
    & "$PSScriptRoot/package-demo.ps1" -Executable "$env:CARGO_TARGET_DIR/x86_64-pc-windows-msvc/debug/blotworx.exe" -OutputDirectory $output
    # Rust 1.98's Windows host crashes optimizing Ruffle for Android.
    cargo rustc --locked --jobs 2 --config 'profile.dev.package.ruffle_core.opt-level=0' --lib --target aarch64-linux-android -- '-Clink-arg=-Wl,-z,max-page-size=16384'
    if ($LASTEXITCODE -ne 0) { throw 'Android build failed.' }
    & "$root/android/package-apk.ps1" @android
    & "$root/3ds/package-3ds.ps1" -DkpRoot $DkpRoot -OutputDirectory $output -Release
    $n3dsx = Join-Path $output "Blotworx-$version-3DS.3dsx"
    $ncia  = Join-Path $output "Blotworx-$version-3DS.cia"
    Get-FileHash -Algorithm SHA256 -LiteralPath $n3dsx,$ncia | ForEach-Object {
        '{0}  {1}' -f $_.Hash.ToLowerInvariant(), (Split-Path $_.Path -Leaf)
    } | Add-Content (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
} finally {
    Pop-Location
}
if (!$Publish) { return }
# Existing Android installations only accept updates signed by this certificate.
$signer = & "$Sdk/build-tools/35.0.0/apksigner.bat" verify --print-certs $android.Output |
    Select-String 'certificate SHA-256 digest:'
if ($LASTEXITCODE -ne 0 -or $signer.Count -ne 1 -or
    $signer.Line -notmatch 'c92044dcb05269ed29c9ccdd5688bd74f273e4ff481b3717050543757c95f55e$') {
    throw 'The Android APK is not signed with the existing release certificate.'
}
if ((git -C $root rev-parse HEAD) -ne $commit -or (git -C $root status --porcelain)) {
    throw 'Source changed during the build.'
}
$names = @("Blotworx-$version-Windows-x64.zip", 'Blotworx-Windows-x64.exe',
    'SHA256SUMS.txt', $apkName, "$apkName.sha256",
    "Blotworx-$version-3DS.3dsx", "Blotworx-$version-3DS.cia")
$artifacts = foreach ($name in $names) { Get-Item -LiteralPath (Join-Path $output $name) }

$token = $env:GH_TOKEN
$headers = @{ Authorization = "Bearer $token"; Accept = 'application/vnd.github+json'; 'X-GitHub-Api-Version' = '2022-11-28' }
$api = 'https://api.github.com/repos/Bloticle/blotworx'
$null = Invoke-RestMethod -Uri "$api/commits/$commit" -Headers $headers
$tag = "v$version"
$releases = Invoke-RestMethod -Uri "$api/releases?per_page=100" -Headers $headers
$release = $releases | Where-Object tag_name -eq $tag | Select-Object -First 1
if ($release -and (!$release.draft -or $release.target_commitish -ne $commit)) {
    throw "Release $tag already exists with a different commit or is already public."
}
foreach ($published in $releases) {
    if ($published.draft) { continue }
    foreach ($asset in $published.assets) {
        if ($asset.name -like '*-Android-arm64.apk' -and $asset.label -match '^android-version-code=(\d+)$' -and
            [int64]$Matches[1] -ge $AndroidVersionCode) {
            throw 'Android version code must be newer than every published APK.'
        }
    }
}
if (!$release) {
    $payload = @{ tag_name = $tag; target_commitish = $commit; name = "Blotworx $version"
        body = $notes; draft = $true; prerelease = $version.Contains('-') }
    $release = Invoke-RestMethod -Method Post -Uri "$api/releases" -Headers $headers -ContentType 'application/json' -Body ($payload | ConvertTo-Json)
}
$upload = $release.upload_url.Split('{')[0]
foreach ($file in $artifacts) {
    $asset = $release.assets | Where-Object name -eq $file.Name | Select-Object -First 1
    $label = if ($file.Name -eq $apkName) { "android-version-code=$AndroidVersionCode" } else { '' }
    if (!$asset) {
        $query = "name=$([uri]::EscapeDataString($file.Name))&label=$([uri]::EscapeDataString($label))"
        $asset = Invoke-RestMethod -Method Post -Uri "$($upload)?$query" -Headers $headers -ContentType 'application/octet-stream' -InFile $file.FullName -TimeoutSec 600
    }
    $digest = 'sha256:' + (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($asset.state -ne 'uploaded' -or $asset.size -ne $file.Length -or $asset.digest -ne $digest -or
        ($label -and $asset.label -ne $label)) {
        throw "Draft asset verification failed for $($file.Name)."
    }
}
Invoke-RestMethod -Method Patch -Uri "$api/releases/$($release.id)" -Headers $headers -ContentType 'application/json' -Body '{"draft":false}' |
    Select-Object tag_name, html_url
