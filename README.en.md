# PadToKBM 手把轉鍵鼠

[繁體中文](README.md) | **English**

**Turn a gamepad into a keyboard and mouse: play keyboard-and-mouse-only games with a controller.**

<p align="center">
  <img src="docs/images/readme/board.png" width="820"
       alt="The key-binding board: the controller in the middle, every button's slot joined to it by a red line, a keyboard, a mouse and macro tiles below">
</p>
<p align="center"><sub>The key-binding board: drag a key, mouse button or macro tile onto a button's slot to bind it. The pressed button and the key it sends light up.</sub></p>

PadToKBM (formerly DualSense Mapper) is a small Windows app: one ~11 MB `PadToKBM.exe`, no installer, no bundled driver.
It supports **DualSense (PS5), Switch Pro and Switch 2 Pro**, each with its own set of bindings.
It was first written so my wife could play MapleStory Worlds Artale with a controller on a laptop.

The app's interface is in Traditional Chinese; the labels quoted below (「…」) are what you see on screen.
For the current version see [CHANGELOG.md](CHANGELOG.md).

---

## What it solves

| Problem | What PadToKBM does |
|---|---|
| The game only takes keyboard and mouse | Controller buttons send keys, mouse buttons and the wheel; the DualSense touchpad can move the mouse cursor |
| Editing a config file to bind keys is a pain | Drag: a tile onto a slot binds it, a slot onto another swaps them, a slot dragged out is cleared; undo / redo |
| Not enough buttons | **Quick pages** (快捷頁): while L2 / ZL (or R2 / ZR) is held, the D-pad and the four face buttons become another set of 8 keys |
| Rapid fire or a combo takes many presses | **Macros** (巨集): record presses and releases, optionally looping; every step's delay is a min–max range |
| Afraid of looking like a script or a cheat | Every key timing is randomized, no fixed rhythm; only ordinary Windows key input, no injection, no game changes |
| Stuck keys | Keys held when the controller drops, the app closes or crashes are released |
| Sometimes you want the game to use the controller itself | Top right, per controller: 「關｜鍵鼠｜手把」 (off / keyboard-mouse / controller). 手把 means the app sends nothing and the game uses the controller (the Switch 2 Pro becomes an Xbox controller) |

---

## Three steps

**1. Download** `PadToKBM.exe` from the [latest release](https://github.com/Luotee/pad-to-kbm/releases/latest) into a folder you can write to (for example `Downloads\PadToKBM\`).

**2. Pair the controller (Bluetooth)**

| Controller | How to pair |
|---|---|
| DualSense | Hold PS and Create until the light bar flashes, then add it in Windows "Bluetooth & devices" |
| Switch Pro | Hold the sync button on top until the lights run, then add it in Windows "Bluetooth & devices" |
| Switch 2 Pro | Do **not** pair it in Windows. With PadToKBM running, hold the small SYNC button the first time; afterwards any button reconnects |

**3. Double-click `PadToKBM.exe`**: once the window is open, press buttons; the keys go to the program in the foreground.
The first run writes a config file `padtokbm.json` next to the exe; the whole folder can be moved to another PC.

---

## A look around

<table>
  <tr>
    <td width="50%"><img src="docs/images/readme/quickslots.png" alt="Switch 2 Pro with ZL held: the quick page's F3 slot and the keyboard's F3 light up"></td>
    <td width="50%"><img src="docs/images/readme/capture.png" alt="The Switch 2 Pro − slot turns blue while it waits for a key"></td>
  </tr>
  <tr>
    <td><sub><b>Quick pages.</b> While ZL is held, the D-pad and face buttons use this page (here ← sends F3). Each trigger switches between 快捷頁 (page) and 單鍵 (one key).</sub></td>
    <td><sub><b>Click to bind.</b> Click a slot, then press a key or a mouse button (middle, right, wheel), or click a tile below. Esc cancels.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/images/readme/macro-editor.png" alt="Macro editor: pressed / released keycaps with a min–max delay under each step"></td>
    <td><img src="docs/images/readme/settings.png" alt="Settings window, 一般 (General) tab: config file path, open buttons, event log switch and the last six events"></td>
  </tr>
  <tr>
    <td><sub><b>Macros.</b> Click keys on the keyboard to add steps, drag to reorder, drag out to delete.</sub></td>
    <td><sub><b>Settings</b> (gear): config file and recent events (each names the button or macro that sent it), stick calibration, touchpad, key timing; applied with 儲存 (Save).</sub></td>
  </tr>
</table>

<sub>Screenshots use a demo config and can be regenerated with <code>scripts/readme-shots.mjs</code>.</sub>

---

## FAQ

### Will it be flagged as a cheat?

It only turns controller buttons into keyboard keys, the same as pressing the keys yourself, using ordinary Windows key input (`SendInput`): no injection, no changes to game files, and it brings no driver of its own.
So it does not look like a script, the hold time (`min_press_ms`), the spread of simultaneous presses (`tick_jitter_ms`) and every macro step's delay are random ranges and cannot be set to a fixed value.
To check for yourself, turn on 「記錄每個按鍵事件」 (log every key event) in Settings: every key sent is written with its time to `padtokbm-events.log` next to the config (past 1 MB it becomes `.log.1` and a new file starts).

### The Switch 2 Pro is connected but the game does nothing?

Look at 「關｜鍵鼠｜手把」 at the top right (on the Switch 2 Pro the third segment reads 「Xbox」):

- **鍵鼠** (keyboard-mouse, the default): buttons send keys and mouse input; nothing else needed.
- **Xbox**: PadToKBM turns it into a virtual Xbox controller for the game, which needs
  [ViGEmBus 1.22.0](https://github.com/nefarius/ViGEmBus/releases/tag/v1.22.0). If this PC does not have it, picking Xbox shows a prompt;
  「安裝 ViGEmBus」 (Install ViGEmBus) downloads the official installer, checks the file and runs it (it asks for administrator rights).
- **關** (off): this controller sends nothing for now; restarting the app brings it back.

In any mode but 鍵鼠 the bindings below are locked; switch back to 鍵鼠 to edit them.

Windows 11 is recommended (Windows 10's Bluetooth limits it to about 17 Hz).

### Where is the config? Can I edit it directly?

`padtokbm.json` next to the exe. Settings → 一般 (General) has 「用記事本開啟」 (Open in Notepad); save the file and the app applies it. The top of the file lists the valid key names.

### Upgrading from DualSense Mapper (v3.0.0 or earlier): do I lose my settings?

No. Put `PadToKBM.exe` in the old folder and delete the old `dualsense-mapper.exe`; on the first run the old `dualsense-mapper.json`
is renamed to `padtokbm.json`, and a backup `dualsense-mapper.json.bak` is kept.

### Can I turn the controller off when I'm not using it?

Yes, click the power button next to the controller's name. The pairing is kept; press PS / HOME to reconnect.
To let the game use the controller directly (no key mapping) pick 手把; to stop it sending anything for a while pick 關.

### USB, DualSense Edge, Xbox controllers or macOS?

Not yet: Bluetooth only, Windows only. A macOS build is planned; until then macOS can use the old Python program in [`legacy-python/`](legacy-python/README.md) (DualSense only).

---

## For developers

<details>
<summary><b>Build, test, layout</b></summary>

| Folder | Contents |
|---|---|
| `rust/` | The app (Rust + Tauri); the frontend is in `rust/web/` |
| `scripts/` | `verify.sh` (pre-release verification), screenshots, font and icon generators |
| `legacy-python/` | The May 2025 Python prototype, frozen |

```bash
cd rust
cargo test                               # a Linux dev host needs pkg-config + libudev-dev
WEBVIEW2_STATIC=true cargo xwin build --release \
  --target x86_64-pc-windows-msvc --features gui   # → PadToKBM.exe
cd .. && scripts/verify.sh               # four layers: linux / windows-tests / gui-smoke / web
```

Command-line flags (mainly for debugging): `--config PATH`, `--validate`, `--dry-run` (print the keys it would send, press nothing), `--list-buttons`, `--verbose`, `--cli` (the old console mode), `--no-pause`.

Every controller's `buttons` table in the config must list button ids 0–32; which button each id is: see `label` in [`rust/config.example.json`](rust/config.example.json).
Contribution rules and the design rules that must not break: [CLAUDE.md](CLAUDE.md).

</details>
