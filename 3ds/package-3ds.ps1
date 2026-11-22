param(
    [Parameter(Mandatory)][string] $DkpRoot,
    [Parameter(Mandatory)][string] $OutputDirectory,
    [string] $RomfsLevelsDirectory
)

$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot/..").Path
$version = [regex]::Match((Get-Content "$root/Cargo.toml" -Raw), '(?m)^version = "([^"]+)"').Groups[1].Value
if (!$version) { throw 'Blotworx package version is missing.' }

$dkp = (Resolve-Path -LiteralPath $DkpRoot).Path
$gcc = Join-Path $dkp 'devkitARM/bin/arm-none-eabi-gcc.exe'
$picasso = Join-Path $dkp 'tools/bin/picasso.exe'
$smdhtool = Join-Path $dkp 'tools/bin/smdhtool.exe'
$tool3dsx = Join-Path $dkp 'tools/bin/3dsxtool.exe'
$makerom = Join-Path $dkp 'tools/bin/makerom.exe'
$specs = Join-Path $dkp 'devkitARM/arm-none-eabi/lib/3dsx.specs'

foreach ($bin in $gcc, $picasso, $smdhtool, $tool3dsx, $makerom, $specs) {
    if (!(Test-Path -LiteralPath $bin)) {
        throw "Required 3DS toolchain file is missing: $bin"
    }
}

$output = (New-Item -ItemType Directory -Force -Path $OutputDirectory).FullName
$work = (New-Item -ItemType Directory -Force -Path (Join-Path $output "3ds-build.work")).FullName
$romfs = (New-Item -ItemType Directory -Force -Path (Join-Path $work "romfs/levels")).FullName

if ($RomfsLevelsDirectory -and (Test-Path -LiteralPath $RomfsLevelsDirectory)) {
    Get-ChildItem -LiteralPath $RomfsLevelsDirectory -Filter '*.bw3d' -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $romfs $_.Name) -Force
    }
}

# 1. Assemble PICA200 vertex shader -> vshader_shbin.h
$shbin = Join-Path $work 'vshader.shbin'
& $picasso -o $shbin (Join-Path $PSScriptRoot 'vshader.pica')
if ($LASTEXITCODE -ne 0) { throw 'picasso PICA200 shader assembly failed.' }

$shBytes = [System.IO.File]::ReadAllBytes($shbin)
$hex = ($shBytes | ForEach-Object { '0x{0:x2}' -f $_ }) -join ', '
$headerText = "static const unsigned char vshader_shbin[] __attribute__((aligned(4))) = { $hex };`nstatic const unsigned int vshader_shbin_size = $($shBytes.Length);`n"
[System.IO.File]::WriteAllText((Join-Path $work 'vshader_shbin.h'), $headerText)

# 2. Generate 48x48 icon and SMDH metadata
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
& $smdhtool --create "Blotworx $version" "Epic Mickey Runtime for Nintendo 3DS (Old3DS & New3DS)" "Bloticle" $iconPath $smdhPath
if ($LASTEXITCODE -ne 0) { throw 'smdhtool failed.' }

# 3. Compile and link ARMv6K 3DS ELF
$objPath = Join-Path $work 'main.o'
$elfPath = Join-Path $work 'Blotworx.elf'
& $gcc -g -Wall -O2 -mword-relocations -ffunction-sections -march=armv6k -mtune=mpcore -mfloat-abi=hard -mtp=soft `
    -I"$work" -I"$PSScriptRoot" -I"$dkp/libctru/include" -D__3DS__ `
    -c (Join-Path $PSScriptRoot 'main.c') -o $objPath
if ($LASTEXITCODE -ne 0) { throw '3DS arm-none-eabi-gcc compilation failed.' }

& $gcc "-specs=$specs" -g -march=armv6k -mtune=mpcore -mfloat-abi=hard -mtp=soft `
    $objPath "-L$dkp/libctru/lib" -lcitro3d -lctru -lm -o $elfPath
if ($LASTEXITCODE -ne 0) { throw '3DS ELF linking failed.' }

# 4. Package .3dsx and .cia
$out3dsx = Join-Path $output "Blotworx-$version-3DS.3dsx"
$outCia  = Join-Path $output "Blotworx-$version-3DS.cia"
$romfsRoot = Join-Path $work 'romfs'

& $tool3dsx $elfPath $out3dsx "--smdh=$smdhPath" "--romfs=$romfsRoot"
if ($LASTEXITCODE -ne 0) { throw '3dsxtool packaging failed.' }

& $makerom -f cia -o $outCia -elf $elfPath -rsf (Join-Path $PSScriptRoot 'cia.rsf') -icon $smdhPath -exefslogo -ver 8
if ($LASTEXITCODE -ne 0) { throw 'makerom .cia packaging failed.' }

Remove-Item -LiteralPath $work -Recurse -Force
Get-Item -LiteralPath $out3dsx, $outCia | Select-Object Name, Length
