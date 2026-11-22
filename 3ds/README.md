# Blotworx Engine on Nintendo 3DS

This is a **native Rust port of the real Blotworx Engine** compiled directly for `armv6k-nintendo-3ds` (`target_os = "horizon"`), running the same `World`, Lua 5.1 scripting (`mlua` + `liblua5.1`), Havok collision/animation decompression (`epic-mickey-lib-rs`), Box3D physics (`third_party/box3d`), skeletal animation, Paint/Thinner simulation, triggers, AI, quests, and level transitions as PC and Android, with a Nintendo 3DS `citro3d` PICA200 GPU backend (`3ds/c3d_bridge.c` + `3ds/vshader.pica`) and 3DS-specific memory optimizations.

Recommended hardware: **New Nintendo 3DS / New 2DS XL** (256 MB RAM, 804 MHz ARM11 + L2 cache) or **Azahar / Citra / Lime3DS** configured in New 3DS mode. Both Homebrew Launcher (`.3dsx`) and installable (`.cia`) packages are produced.

## Setup (Game Data)

Blotworx does **not** ship retail game files. Copy your extracted Wii Epic Mickey `DATA/` folder (containing `files/packfiles/`) onto your 3DS SD card at:

```text
sdmc:/blotworx/DATA/
```

(On Azahar/Citra, place or symlink it at `%APPDATA%\Azahar\sdmc\blotworx\DATA\`.)

## Controls & Dual-Screen Layout

- **Top Screen (400×240 Stereoscopic 3D)**: Hardware-accelerated PICA200 3D renderer with stereoscopic 3D depth slider support, two-pass opaque/transparent rendering, and 64×64 Morton-swizzled `GPU_RGBA5551` Wii textures.
- **Bottom Screen (320×240)**: Live engine telemetry (`HP`, `Paint`, `Thinner`, position, draw count, tick, RAM), stylus touch aiming, and level selector.
- **Circle Pad**: Move Mickey
- **C-Stick / D-Pad**: Orbit camera
- **B**: Jump / Double Jump
- **Y**: Spin Attack
- **R / ZR**: Paint Stream / Paint Blast
- **L / ZL**: Thinner Stream / Thinner Blast
- **Touch Screen**: Drag to aim Paint/Thinner reticle
- **SELECT**: Open/close Level Selector (`D-Pad Up/Down` + `A` to load)
- **START**: Exit to Homebrew Launcher / Home Menu

## Building

Requires Rust nightly (`rustup toolchain install nightly && rustup component add rust-src --toolchain nightly`) and devkitPro (`devkitARM`, `libctru`, `citro3d`, `3ds-liblua51`, `picasso`, `3dsxtool`, `smdhtool`, `makerom`):

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File 3ds/package-3ds.ps1 -DkpRoot C:\devkitPro -OutputDirectory dist -Release
```
