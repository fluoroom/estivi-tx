# Estivi

Estivi runs an SSTV channel the way a station runs a schedule. One window, no browser: a grid of programs and common blocks, the picture list of the block you are on, graphics on the block, the program, and the channel, a live view and the next transmission, an instant transmission, and activations that answer what the radio hears. It is a single executable for Linux and Windows (Rust + Slint).

The channel is one frequency and one timezone, UTC or local. That timezone drives the clock, the grid, the block ends, and the times written into export names. The offset that shifts every start belongs to the whole project and does not move the written end. The interval does not belong to the project. Each common block has its own, in whole minutes. A program has one per weekday: Monday can be 14:00–14:30 every 6 minutes and Saturday 22:00–22:30 every 10, and changing one day leaves the others. Zero means one start in that block. Turning a weekday on copies the hours and interval from a day that already has them, and from then on that day can be changed on its own. A program repeats on the weekdays you mark, until a date or forever. A common block is one date and does not repeat. Pictures belong to that occurrence. A program day is stored once it has a picture, its own graphics, or its own SSTV mode, and it becomes a prediction again when all three are gone. An empty common block stays until you delete the card.

## What a transmission contains

A turn that fits entirely before the next start and before the block end goes out. One that does not fit is skipped, and so are pictures past the number of turns. The block end releases the PTT and cuts the audio together, just before the written end. The offset moves the starts and leaves that end where you wrote it. The silence after the last start of a block does not have to satisfy the minimum pause. More than 90 seconds late, the turn is skipped.

Order of one grid turn, each piece only when it is enabled and has something to send:

1. VOX burst, if the project option is on (800 ms, in front of every transmitting action, not a row you can reorder).
2. Channel CW preamble.
3. AX.25 info frame.
4. Welcome, only on the first turn of a program block. If that turn is dropped, the welcome is dropped with it.
5. Pre-bumpers, then the picture, then post-bumpers. Each SSTV piece has its VIS in front. A mode AX.25 frame can sit in front of each one (`ESTIVI MODE R36`, `… BUMPER PRE`, `… WELCOME`, `… BYE`).
6. Bye, only on the last turn. If that turn is dropped, the bye is dropped with it.
7. AX.25 info again, then the channel CW postamble.

Bumpers do not consume a turn. A disabled picture is skipped and does not consume one either. A single turn that is both first and last carries the welcome and the bye.

There is no FSK ID. The digital identification is AX.25 and, if you leave it on, CW. AX.25 is always 1200 baud (1200/2200 Hz). The source call is the station callsign (`CALL-N` sets the SSID).

## The window

The grid is the center: month, week, or day, with Today and the period kept to the left of that row. You can move back as far as you want and forward three years from today. A program card takes its color from the program hue. A common block is a neutral gray. A disabled program or a disabled block is a lighter gray and does not keep the hue. A filled, enabled card is opaque; a disabled one is dimmer; an empty one is the lightest and still occupies the hour, so nothing else can overlap it. The selected card has a yellow border. Clicking empty grid clears the selection and hides the timeline. Touching edges (one ends when the next starts) is not an overlap. If a drag, an edit, or a new card would cross another, none of the change is applied and the message names the card in the way.

Programs are a separate window. A new program has no days and no hours until you set them there: each weekday can be on or off, and each day has its own start, end, and interval. The grid draws a card from that schedule. The card does not store the day or the hours, and its panel does not edit them. An empty card is a prediction until it has a picture, its own graphics, or its own SSTV mode. Double-click an empty hour to create a common block on that date only. Nothing is created until you confirm. The dialog starts the interval at 6 minutes and proposes an end one interval after the click; you can lengthen the end, and changing the interval rewrites that proposal from the start. Dragging a common card changes only that card. Dragging a program card does not change the hours. A common block can be deleted. A program is deleted only after you choose what happens to its blocks: drop them, keep the ones with pictures as common blocks, or also keep the empty days up to a date you pick (three years at most). Turning a weekday off asks the same question for that day's future blocks. Past blocks stay.

“Transmit this date” turns that occurrence off. On a program card the other days stay as they are. On a common block the card stays on the grid, in gray. Start grid and Stop grid arm and disarm the schedule. Cut aborts the transmission that is on the air. Undo puts the channel back to the last load or save and does not write the file.

The block panel lists the pictures and the bumper links. Click a row to select it; the picture-mode menu follows that picture. Each transmission shows its pieces and its total: CW, AX.25, VIS, welcome, the picture, bye, and the silence until the next start. Those time rows are not draggable. The AX.25 mode row and the VIS row travel with their picture, and clicking one selects that picture. Link up, link down, and unlink keep the bumper invariant: pre-bumpers sit immediately above their picture, post-bumpers immediately below, and a bumper does not point at a bumper. Dragging a row, or the up and down buttons, moves a whole group together and moves a bumper only within its own side. The timeline under the grid sizes each transmission by its duration, shows the picture, and marks the start, the end, and the interval. A yellow playhead appears when now is inside the block. Clicking a clip selects that picture.

Live shows the picture that is going out, filling from the top as the audio plays. VIS, CW, and the other non-picture pieces stay black. Next is the soonest transmission that has not started, with its name, the clock time, and the file name. It redraws when that transmission changes, or when the block, program, or channel graphics that paint on it change. It does not stay on a block that is already on the air, and it does not stay on a block that a time edit pushed behind another.

Marking a bumper picks the mode from the file: Robot 12 color when some pixel has a channel difference above 8, Robot 8 black and white otherwise. An unreadable file stays Robot 12. You can change the mode afterwards. Unlinking leaves the mode and makes the row a normal picture.

Instant transmission has its own tab. It works while the grid is armed and idle. While something holds the PTT the transmit control is disabled and says why. It is not cut by a block end. You can start from a picture of the selected block, with that picture's bumpers and, when it is the first or last enabled picture, the welcome or the bye.

Activations have their own tab. They are not objects on the grid. A trigger is CW or DTMF, and any one match runs the activation. Actions run in order while that activation holds the PTT: CW, AX.25, DTMF, a local command, a picture, a random picture from a pool, pause, resume, or an API answer. Pause and resume do not key the radio and do not cut a transmission already on the air. A second match while that activation is queued or running does not enqueue another copy.

Graphics are edited per level. The block is painted first, then the program, then the channel, each on the frame the previous step left. A later step does not refill the background. The automatic base is never a bumper: the block editor uses the first normal image of that block, the program editor uses the first normal image of that program's next block, and the channel editor uses the next image the channel will send. The editor can also stand the graphics on color bars, a grid, or a gray ramp. Text, image, and QR layers sit on that frame. A QR can be text (with the same `{fields}`), a command, or an API. If the QR cannot be built, that layer is skipped and the rest of the frame goes out. Fields, also valid in text: `{block}`, `{range}`, `{days}`, `{daysleft}`, `{next}`.

The themes are Gray (the default), Light, Lavender, Pastel, Subte, and Subte light. Block hues stay their own colors. The interface is English or Spanish, and the first launch follows the system language when it is one of those two. Errors, warnings, and other system text stay in English.

## Two files

| File | What it stores | Where |
|---|---|---|
| `estivi.toml` | This computer: audio, serial port, PTT, CAT, language, theme, and the NTP offset | Next to the executable when that folder is writable, otherwise `~/.config/estivi/` or `%APPDATA%\estivi\` |
| `*.estivi` | The channel, format 2 | Wherever you save it. Pictures inside the project folder are stored with a relative path |
| `*-registro.csv` | One line per transmission. The last columns are the block id, the program id, and whether the block end cut the audio (`cortado` is `0` or `1`) | Next to the project |

An old format 1 file is rejected with a clear error and is not rewritten. There is no converter. Format 2 is the file.

The clock of the operating system is not changed. On the first launch Estivi offers to follow `pool.ntp.org` and store the offset (every 6 hours, editable under Equipment). If you refuse, it does not ask again. If the server fails later, the last offset is kept and transmission is not blocked. The grid, the PTT cutoff, “now” for a folder import, and an export range all use the system clock plus that offset.

`.ttf` / `.otf` fonts in a `fuentes/` folder next to the project or the executable show up in the graphics editor, besides the bundled Noto fonts.

## Import and export

Import is a button, not a drop. A folder named `block` plus a number (`block01`, `block1`, `BLOCK02`, case does not matter) fills the Nth upcoming occurrence of the selected program, counting empty days. `block02` alone fills the second and leaves the first alone. Inside the folder the order is natural (`2` before `10`). A number past the remaining days is reported and skipped. The folder does not create new days. A file whose name ends in `-bp` before the extension is the post-bumper of the previous picture; `-bn` is the pre-bumper of the next. `foto-bp-2.png` is not a bumper. A `-bp` with nothing before it, or a `-bn` with nothing after it, is added as a normal picture and reported.

Export is not saving the project. You pick a range and one of five shapes: one file, each transmission and each silence, each piece, slices of N seconds, or one file per block. The files add up to the range. The time in the name is when to press play, in the channel timezone (`-0300` or `Z`). The index comes first so alphabetical order is play order. The encoder in this build is FLAC. MP3 is not included because the usual encoder is LGPL and linking it here does not match the license of the program. AAC is not included because there is no encoder that ships without an external tool. Sample rates are 11025, 22050, 44100, and 48000. The audio is synthesized at the rate you pick. Leaving FLAC at 44100 shows a warning every time: any other setting introduces slight visual changes in the decoded image. It does not guarantee a perfect transmission, but it is still usable.

## SSTV modes

Martin M1–M4, Scottie S1, S2 and DX, Robot 24, 36 and 72, Robot 12 color, Robot B/W 8, 12 and 24, and PD 50, 90, 120, 160, 180, 240 and 290. Every SSTV piece carries VIS. Scottie keeps its 9 ms sync glued to the VIS.

The mode of a picture is its own, otherwise the block's, otherwise the program's, otherwise the channel's. The picture, block, and program menus start with Inherited. The welcome and the bye use their own mode, otherwise the program's, otherwise the channel's. They do not take the block's mode.

## Build

Stable Rust 1.85 or newer.

```sh
cargo build --release
# target/release/estivi  (estivi.exe on Windows)
```

- **Linux**: ALSA and fontconfig (`alsa-lib` and `fontconfig` on Arch; `libasound2-dev` and `libfontconfig1-dev` on Debian or Ubuntu).
- **Windows**: build on Windows with the MSVC toolchain. Release builds do not open a console.
- The UI draws in software (about 50 MB of RAM, no GPU driver). Equipment can switch the renderer only by the setting that was already in the config (`gpu_render`); the default stays the CPU renderer.

```sh
cargo run -p estivi-core --example render -- ejemplo/canal-demo.estivi cuadro.png [audio.wav] [programa] [modo]
cargo run -p estivi-core --example dispositivos
```

`ejemplo/canal-demo.estivi` is a format 2 channel (callsign `LU1AQY`) with two programs, three stored blocks, two activations, a lower third, a QR, and a hidden weather layer. The pictures it names are not in the repository.

## Verification

`cargo test -p estivi-core --lib`: line timings, VIS, pixel edges, Martin M1 round-trip, CW and DTMF (including a DTMF tone round-trip), the calendar (turns, overlap, days left, the three-year limit, DST), AX.25 frames, bumper linking and `block02` import, export durations for the five shapes, and one short FLAC file.

Decoded with QSSTV 9 over real audio, except where noted:

| Mode | Result |
|---|---|
| Martin M1 | OK, transmitted live from the app |
| Martin M2 · Scottie S1, S2, DX | OK |
| Robot 24, 36, 72 · Robot B/W 8, 12 | OK |
| PD 50, 120, 290 | OK |
| PD 90, 160, 180, 240 | Same code as the tested PD modes; not decoded separately |
| Martin M3, M4 · Robot B/W 24 | QSSTV does not support them; they follow the slowrx table and are still untested on the air |
| Robot 12 color | Duration is tested (12.0 s of picture after the VIS). QSSTV and slowrx do not transmit it; slowrx only has Robot 12 black and white, so this mode was not decoded there |

The color range default is full range, which is what QSSTV decodes as a normal picture. Studio range remains an option and comes out washed on QSSTV.

Serial PTT and CAT (`rigctld`) are implemented and have not been tried on a radio. RTS and DTR are chosen apart: off, positive (high while keyed), or negative (high at rest). On Linux, the `front` PCM of some cards goes through a software mixer: if the level is very low, pick the `plughw` device or raise that control.

The window process was started on a display with `ejemplo/canal-demo.estivi`. It opened the default audio device and stayed up until it was stopped. The click-through — create a program in its window and see the cards, reject an overlap, mark a bumper, read the turn total, disable a program, export a short FLAC, and confirm that instant transmit is refused while the grid holds the PTT — was not clicked through in this run.

## Licenses

The bundled Noto fonts (`crates/core/assets/fonts`) are SIL Open Font License 1.1 (`OFL.txt`). Slint can be used under GPLv3 or under its royalty-free desktop license, which requires showing the “Made with Slint” attribution. FLAC is written with `flacenc` (Apache-2.0). QR codes use the `qrcode` crate.
