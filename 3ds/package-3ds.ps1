param(
    [Parameter(Mandatory)][string] $DkpRoot,
    [Parameter(Mandatory)][string] $OutputDirectory,
    [string] $PrebuiltElf,
    [switch] $Release
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot/..").Path
$version = [regex]::Match((Get-Content "$root/Cargo.toml" -Raw), '(?m)^version = "([^"]+)"').Groups[1].Value
if (!$version) { throw 'Blotworx package version is missing.' }

$dkp = (Resolve-Path -LiteralPath $DkpRoot).Path
$gcc = Join-Path $dkp 'devkitARM/bin/arm-none-eabi-gcc.exe'
$gxx = Join-Path $dkp 'devkitARM/bin/arm-none-eabi-g++.exe'
$ar  = Join-Path $dkp 'devkitARM/bin/arm-none-eabi-ar.exe'
$picasso = Join-Path $dkp 'tools/bin/picasso.exe'
$smdhtool = Join-Path $dkp 'tools/bin/smdhtool.exe'
$tool3dsx = Join-Path $dkp 'tools/bin/3dsxtool.exe'
$makerom = Join-Path $dkp 'tools/bin/makerom.exe'

foreach ($bin in $gcc, $gxx, $ar, $picasso, $smdhtool, $tool3dsx, $makerom) {
    if (!(Test-Path -LiteralPath $bin)) {
        throw "Required 3DS toolchain file is missing: $bin"
    }
}

$output = (New-Item -ItemType Directory -Force -Path $OutputDirectory).FullName
$work = (New-Item -ItemType Directory -Force -Path (Join-Path $output "3ds-build.work")).FullName

# 1. Generate 48x48 icon and SMDH metadata
$iconPath = Join-Path $work 'icon.png'
python -c "
import zlib, struct, sys
w, h = 48, 48
raw = bytearray()
for y in range(h):
    raw.append(0)
    for x in range(w):
        dx, dy = x - 24, y - 27
        elx, ely = x - 13, y - 14
        erx, ery = x - 35, y - 14
        if (dx*dx + dy*dy <= 121) or (elx*elx + ely*ely <= 49) or (erx*erx + ery*ery <= 49):
            raw.extend((45, 165, 255) if x < 24 else (55, 235, 85))
        elif 2 <= x < 46 and 2 <= y < 46:
            raw.extend((24, 34, 58))
        else:
            raw.extend((70, 130, 210))
def chunk(tag, data):
    return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)
png = b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)) + chunk(b'IDAT', zlib.compress(bytes(raw), 9)) + chunk(b'IEND', b'')
open(sys.argv[1], 'wb').write(png)
" $iconPath
if ($LASTEXITCODE -ne 0) { throw 'Failed to generate 3DS icon.png.' }

$smdhPath = Join-Path $work 'Blotworx.smdh'
& $smdhtool --create "Blotworx $version" "Native Rust Blotworx Engine for Nintendo 3DS" "Bloticle" $iconPath $smdhPath
if ($LASTEXITCODE -ne 0) { throw 'smdhtool failed.' }

# 2. Build native Rust armv6k-nintendo-3ds Blotworx Engine ELF (unless -PrebuiltElf provided)
if ($PrebuiltElf -and (Test-Path -LiteralPath $PrebuiltElf)) {
    $elfPath = (Resolve-Path -LiteralPath $PrebuiltElf).Path
} else {
    $env:DEVKITPRO = $dkp
    $env:DEVKITARM = Join-Path $dkp 'devkitARM'
    $env:PATH = "$(Join-Path $dkp 'devkitARM/bin');$(Join-Path $dkp 'tools/bin');$env:PATH"
    $env:CARGO_TARGET_ARMV6K_NINTENDO_3DS_LINKER = $gcc
    $env:CC_armv6k_nintendo_3ds = $gcc
    $env:CXX_armv6k_nintendo_3ds = $gxx
    $env:AR_armv6k_nintendo_3ds = $ar
    $env:CFLAGS_armv6k_nintendo_3ds = "-march=armv6k -mtune=mpcore -mfloat-abi=hard -mtp=soft"
    $env:CXXFLAGS_armv6k_nintendo_3ds = "-march=armv6k -mtune=mpcore -mfloat-abi=hard -mtp=soft"
    $env:LUA_INC = Join-Path $dkp 'portlibs/3ds/include/lua5.1'
    $env:LUA_LIB = Join-Path $dkp 'portlibs/3ds/lib'
    $env:LUA_LIB_NAME = "lua5.1"
    $env:LUA_LINK = "static"
    $libctru = (Join-Path $dkp 'libctru/lib') -replace '\\','/'
    $portlibs = (Join-Path $dkp 'portlibs/3ds/lib') -replace '\\','/'
    $sep = [char]0x1f
    $rustFlagsList = @(
        "-C", "panic=abort",
        "-L", "native=$libctru",
        "-L", "native=$portlibs",
        "-C", "link-arg=-L$libctru",
        "-C", "link-arg=-L$portlibs",
        "-C", "link-arg=-lcitro3d",
        "-C", "link-arg=-llua5.1",
        "-C", "link-arg=-lctru",
        "-C", "link-arg=-lstdc++"
    )
    # Note: do not pass -C strip=symbols here; 3dsx.specs uses --emit-relocs, which GNU ld
    # rejects when combined with --strip-all. 3dsxtool and makerom strip symbols when packaging.
    $env:CARGO_ENCODED_RUSTFLAGS = $rustFlagsList -join $sep

    Push-Location $root
    try {
        $cargoArgs = @('+nightly', 'build')
        if ($Release) { $cargoArgs += '--release' }
        $cargoArgs += @('--no-default-features', '--bin', 'blotworx', '-Z', 'build-std=std,panic_abort', '--target', 'armv6k-nintendo-3ds')
        & cargo @cargoArgs
        if ($LASTEXITCODE -ne 0) { throw 'Rust armv6k-nintendo-3ds build failed.' }
    } finally {
        Pop-Location
    }
    $profileDir = if ($Release) { 'release' } else { 'debug' }
    $elfPath = Join-Path $root "target/armv6k-nintendo-3ds/$profileDir/blotworx.elf"
}

# 3. Package .3dsx and .cia
$out3dsx = Join-Path $output "Blotworx-$version-3DS.3dsx"
$outCia  = Join-Path $output "Blotworx-$version-3DS.cia"

& $tool3dsx $elfPath $out3dsx "--smdh=$smdhPath"
if ($LASTEXITCODE -ne 0) { throw '3dsxtool packaging failed.' }

& $makerom -f cia -o $outCia -elf $elfPath -rsf (Join-Path $PSScriptRoot 'cia.rsf') -icon $smdhPath -exefslogo -ver 8
if ($LASTEXITCODE -ne 0) { throw 'makerom .cia packaging failed.' }

Remove-Item -LiteralPath $work -Recurse -Force
Get-Item -LiteralPath $out3dsx, $outCia | Select-Object Name, Length
