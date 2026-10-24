# Estivi

Schedule an SSTV channel the way you would a TV station: a **grid** of time blocks, **programs** (the pictures that go on air), **channel graphics** on top (lower thirds, clock, serial number, logo, data from an API), and a **transmission log**. It runs on Windows and Linux as a single light executable (Rust + Slint, no browser) and does not need the internet.

- **Transmit** SSTV: Martin M1–M4, Scottie S1/S2/DX, Robot 24/36/72, Robot B/W 8/12/24, and PD 50/90/120/160/180/240/290. With VIS, optional VOX preamble, FSK ID, and CW ID (optional preamble before VIS and ID after the picture).
- **Listen** for CW (fixed speed) and DTMF, for remote **triggers**: “if I hear `*1#`, transmit”, “if I hear `QRV SSTV` in CW, transmit”, pause or resume the grid.
- **PTT** by VOX, serial port (RTS/DTR, normal or inverted), or CAT via Hamlib (`rigctld`), which can also set the frequency of each block.

## Two different files

| File | What it stores | Where |
|---|---|---|
| `estivi.toml` (config) | Things that belong to **this PC**: audio devices, serial port, PTT, CAT, clock calibration | Next to the executable if that folder is writable (portable install on a USB stick); otherwise `~/.config/estivi/` or `%APPDATA%\estivi\` |
| `*.estivi` (project) | Things that belong to the **channel**: SSTV mode, station, programs, grid, graphics, identification, listen, and triggers | Wherever you save it. Images inside the project folder are stored with a relative path, so the whole folder travels together |
| `*-registro.csv` | One line per transmission (start, end, serial, mode, program, frequency, origin, result) | Next to the project |

`.ttf`/`.otf` fonts you drop in a `fuentes/` folder next to the project or the executable show up in the graphics editor (in addition to the bundled Noto fonts).

## Main window

- **On-air bar**: status (STANDBY / ON AIR / PAUSED), next transmission with countdown, project mode, “Grid on air” switch, *Transmit now* and *Cut*.
- **Programs**: groups of pictures with sequential or random-without-repeat rotation. Each program can override CW and activations; the grid owns the schedule. Double-click to rename; ▶ transmits it now.
- **Grid**: blocks with days, time (once, or every N minutes between two hours, including across midnight), a fixed program or rotation, and an optional frequency. The day timeline sits above. A project-wide **offset** (seconds) delays every start so several channels can share a frequency: keep the 14:00 block, set +3:00, and this station keys at 14:03 while the other occupies 14:00–14:03.
- **Monitors**: what is on air (with the scan line advancing) and an exact preview of what goes next.
- **Log** and **Listen** (input level, received CW text and DTMF sequences).

The **minimum pause** between transmissions (Project → Transmission) protects the radio: if a block would start earlier, it waits until the pause ends. Transmissions that slip more than 90 s (for example after the PC was suspended) are skipped instead of going out late.

Light and dark themes live in the window header. **Utils** opens a cheat sheet of every SSTV mode (size, color space, duration, VIS).

## SSTV mode

The project mode is the default. A program can set its own default for its pictures, and a single picture can override that. Inheritance is **picture → program → project**.

Files can be dropped from the file manager onto the in-app drop zones, or chosen with the native file picker.

## Channel graphics

A separate window (**Graphics**). The program picture is the base layer and is fitted to each mode’s size: crop to fill, fit with bars, or stretch. On top, **text** and **image** layers (PNG with transparency) can be dragged, resized from the corners (with snap to center and edges), and tuned in the inspector. Positions are stored as ratios, so the same graphics work from 160×120 (Robot 24) to 800×616 (PD 290).

Text layers accept fields in braces:

| Field | Shows |
|---|---|
| `{call}` `{op}` `{qth}` `{loc}` | Station data |
| `{utc}` `{date}` `{local}` | UTC time, UTC date, local time. With a format: `{utc:%d/%m %H:%M}` |
| `{serial}` `{serial:4}` | Transmission number (zero-padded: `0042`) |
| `{mode}` `{program}` `{freq}` | Mode, program name, block frequency |
| `{cw}` `{cwcall}` `{dtmf}` | Last CW text, last callsign heard in CW, last DTMF sequence |
| `{api}` | Response from a URL (optional JSON field, e.g. `current.temperature_2m`), refreshed every N seconds in the background |

For readability in noise, each text can have an outline, shadow, and background box, and it shrinks on its own if it does not fit.

## Build

Needs stable Rust (1.85 or newer).

```sh
cargo build --release
# the binary is target/release/estivi (estivi.exe on Windows)
```

- **Linux**: ALSA and fontconfig development packages (`alsa-lib` and `fontconfig` on Arch; `libasound2-dev libfontconfig1-dev` on Debian/Ubuntu).
- **Windows**: build on Windows with the MSVC toolchain. Release builds do not open a console.
- The UI draws in software (about 50 MB of RAM and ~3 % CPU at idle, no GPU driver required). Equipment → General can switch to the GPU (OpenGL) if you prefer.

Command-line tools (useful for testing without a radio):

```sh
# final frame of a program to PNG and the full transmission to WAV (decode with QSSTV/MMSSTV)
cargo run -p estivi-core --example render -- ejemplo/canal-demo.estivi frame.png audio.wav 0 PD120
# audio devices (the id goes in estivi.toml) and serial ports
cargo run -p estivi-core --example dispositivos
```

`ejemplo/canal-demo.estivi` is a sample channel (fictitious callsign `N0CALL`) with three programs, three blocks, a lower third, clock/serial, logo, and a weather API layer (hidden).

## Verification status

Automated tests (`cargo test --workspace`): line timings of every mode against published values, VIS and parity, pixel edges with no accumulated drift, Martin M1 frequency round-trip, CW and DTMF decoders with noise, the grid (including midnight wrap), minimum pause and late transmissions, rotation, and file formats.

Decoded with **QSSTV 9** over real audio (ALSA loopback), correct image and resolution:

| Mode | Result |
|---|---|
| Martin M1 | OK, transmitted live from the app (engine → audio → QSSTV) |
| Martin M2 · Scottie S1, S2, DX | OK |
| Robot 24, 36, 72 · Robot B/W 8, 12 | OK |
| PD 50, 120, 290 | OK |
| PD 90, 160, 180, 240 | Same code as the tested PD modes; not decoded separately |
| Martin M3, M4 · Robot B/W 24 | QSSTV does not support them: they follow the slowrx table, still untested |

- Robot/PD color scale: QSSTV decodes luminance as **full range** (measured with bars: 0→2 and 255→255; studio range came out 18 and 237, a washed-out picture). That is why it is the default; studio range remains an option under Project → Transmission.
- FSK ID follows the MMSSTV format (same as pySSTV); still needs confirmation with MMSSTV.
- Serial PTT and CAT (rigctld) are implemented but not tested on real hardware.
- On Linux, the `front` PCM of some cards goes through an ALSA software mixer volume: if the level is very low, pick the device’s `plughw` variant or raise that control.

## Licenses

The bundled Noto fonts (`crates/core/assets/fonts`) are SIL Open Font License 1.1 (`OFL.txt`). Slint can be used under GPLv3 or under its royalty-free desktop license, which requires showing the “Made with Slint” attribution: pick one before distributing.
