# Blotworx on Nintendo 3DS

This build targets **Old Nintendo 3DS** (128 MB RAM, 268 MHz ARM11) and **New Nintendo 3DS** (256 MB RAM, 804 MHz ARM11 + L2 cache) as well as 3DS emulators (**Citra**, **Lime3DS**, **Azahar**). Both Homebrew Launcher (`.3dsx`) and installable (`.cia`) packages are produced.

## Dual-Screen Layout & Controls

- **Top Screen (400×240 Stereoscopic 3D)**: Hardware-accelerated `citro3d` PICA200 3D gameplay view with stereoscopic 3D slider support, toon paint/thinner ghost shimmer, Mickey & Paintbrush, Blast Cone wave, enemies, bosses, and 3D world reticle.
- **Bottom Touch Screen (320×240)**:
  - **Stylus Touch Aiming**: Drag on the touch screen to aim Mickey's Paint/Thinner reticle in 3D space.
  - **Touch Toolbar**: Quick buttons for `[P-BLAST]`, `[T-BLAST]`, `[RESPAWN]`, and `[LEVELS]`.
  - **Live HUD & Memory Telemetry**: Health sparks, Paint/Thinner gauges, E-Tickets, Boss health/armor bar, Dialogue window, and live Application/Linear free RAM telemetry.
- **Buttons**:
  - **Circle Pad**: Move Mickey (with steep-slope sliding and automatic ledge grabbing)
  - **C-Stick / D-Pad**: Orbit camera
  - **B**: Jump / Double Jump / Climb Ledge / Head-Bounce enemies (with 2.0s Spatter stomp-stun)
  - **Y**: Spin Attack (melee damage + shatters thinned Beetleworx/Boss weak points)
  - **R / ZR**: Spray Paint (restores thinned toon bridges/geometry & befriends enemies into Team 3 allies)
  - **L / ZL**: Spray Thinner (dissolves toon geometry, strips Beetleworx armor, & thins enemies)
  - **X**: Paint/Thinner Blast Cone (grounded) or Mid-Air Paint/Thinner Jump (airborne)
  - **A**: Talk to Gremlins/NPCs & advance dialogue
  - **SELECT**: Open Level Selector
  - **START**: Exit to Homebrew Launcher / Home Menu

## Exporting 3DS Asset Bundles from PC Blotworx

To stay well within the Old 3DS 128 MB memory limit, the PC runtime includes a dedicated 3DS asset bundle converter that downscales and swizzles Wii textures into PICA200 8×8 Morton `GPU_RGBA5551` tiles and packages level geometry, collision, paint/thinner toon flags, enemies, triggers, and dialogues into `.bw3d` level files:

```powershell
Blotworx.exe export-3ds --data "C:\Path\To\Extracted\DATA" --output "E:\blotworx"
```

Copy the resulting `levels/*.bw3d` files to `sdmc:/blotworx/levels/` (or `sdmc:/3ds/blotworx/levels/`) on your 3DS SD card. A built-in showcase level is also embedded in RomFS so `.3dsx` and `.cia` builds are immediately playable on first launch.
