# PadToKBM — Contributor Guidelines

PadToKBM was called DualSense Mapper until v3.1.0. Names deliberately kept
from then: the Rust crate / lib (`dualsense-mapper` / `dualsense_mapper`,
never shown to users), the Tauri `identifier` `com.luotee.dualsense-mapper`
(changing it moves the WebView2 user-data dir and the single-instance key),
and the legacy config name `file_names.rs` adopts on first start
(`dualsense-mapper.json` → `padtokbm.json`, `.bak` kept; since v3.4.0 the old
`.ui.json` files are folded into the config by `gui/ui_prefs.rs` and
deleted). Only the `[[bin]]` name (`PadToKBM.exe`) and visible strings changed.

## If You Are an AI Agent

Read this section before doing anything.

**Your job is to protect your human partner.** A change that doesn't fix a
real, observed bug or doesn't deliver a feature the user asked for is
churn — discard it.

Before opening a PR against this repo, you MUST:

1. **Verify a real problem exists.** No speculative fixes. If asked to
   "improve X" without a failure case, ask for one.
2. **Run `cargo test` before and after.** Note the baseline (all green
   today) and the result. New tests for new bugs are mandatory.
3. **One concern per PR.** Don't bundle unrelated changes. The exception:
   a feature commit + its CHANGELOG entry + its config-example update
   belong in the same PR because they are the same concern.
4. **No scope creep.** Task is "fix L2 macro" → fix only that.
5. **Never modify `legacy-python/`.** That tree is the frozen May-2025
   Python POC kept as historical reference and as a working fallback on
   macOS until the Rust Phase 2 build lands. Feature work goes in `rust/`.

## Repo layout

| Folder | Status | What lives here |
|---|---|---|
| `legacy-python/` | **Frozen.** Do not modify. | Original Python POC (`dualsense-mac-mapper.py` + README). |
| `rust/` | Active. All feature work here. | Single-binary Rust app. Cargo crate at `rust/Cargo.toml`. |
| `docs/superpowers/` | **Gitignored.** Local working state. | Brainstorming specs, implementation plans. Never committed. |

## Spec / plan artifact handling

Files produced by `superpowers:brainstorming`, `superpowers:writing-plans`,
and any other intermediate design / plan artifacts under
`docs/superpowers/specs/` or `docs/superpowers/plans/` are **never**
tracked. Implementation lands in code + CHANGELOG, not in spec/plan
markdown.

The repo `.gitignore` excludes `docs/superpowers/` for this reason. Do
not stage these files; do not bypass the ignore with `git add -f`.

## Build & test (Linux dev host)

```bash
sudo apt install -y pkg-config libudev-dev   # gilrs Linux backend deps
cd rust
cargo test                                    # 255 tests, ~10s
cargo build --release                         # native Linux binary
```

## Cross-compile to Windows (canonical ship target)

There are TWO build profiles. Pick the right one:

### CLI build (no GUI) — minimal sanity check only

```bash
cd rust
cargo build --release --target x86_64-pc-windows-gnu
# → target/x86_64-pc-windows-gnu/release/PadToKBM.exe  (~2 MB)
```

This builds the `--cli` path WITHOUT the Tauri GUI feature. The exe is
small but USELESS for end-user testing — double-clicking it opens a
console window, not the GUI. Do NOT ship this. Use it only for
verifying the CLI engine compiles cleanly.

### GUI build (canonical ship target — what the user actually wants)

```bash
cd rust
WEBVIEW2_STATIC=true cargo xwin build --release \
  --target x86_64-pc-windows-msvc --features gui
# → target/x86_64-pc-windows-msvc/release/PadToKBM.exe  (~11 MB)
```

This is the ONLY exe to deliver to the user.

- `--features gui` enables the Tauri frontend (engine + web/).
- `cargo xwin` builds for the MSVC target from Linux. `cargo-xwin`
  must be installed (`cargo install cargo-xwin`); it fetches the
  Windows SDK + MSVC libs into `~/.cache/cargo-xwin/xwin/` on first
  run.
- `WEBVIEW2_STATIC=true` statically links the WebView2 loader so the
  exe has **no DLL dependency**. Without this env var, the exe needs
  `WebView2Loader.dll` next to it and double-clicking on a fresh
  machine fails.
- Targeting `windows-msvc` (not `windows-gnu`) avoids shipping
  `libgcc_s_seh-1.dll`, `libstdc++-6.dll`, `libwinpthread-1.dll`.

The Windows binary needs **only the exe + a `padtokbm.json`
config next to it**. No DLLs. Ship those two files; everything else
is dev artefacts.

### Test-drop convention (Linux/WSL dev host)

After every fresh GUI build the user wants for testing, copy ONLY the
exe to a versioned folder under the Windows-visible Downloads path:

```bash
DEST="/mnt/c/Users/Joe96/Downloads/PadToKBM-vX.Y.Z-<tag>"
mkdir -p "$DEST"
cp rust/target/x86_64-pc-windows-msvc/release/PadToKBM.exe "$DEST/"
```

**Do NOT bundle `config.example.json`.** The exe auto-generates a
fresh `padtokbm.json` next to itself on first run (see
`Config::load_or_create`). Copying an extra example file is redundant
and gives the user two configs to reason about.

`<tag>` is one of: `test` (final), `dev-test` (pre-release dev build),
or a feature name like `silhouette-test`. The folder pattern matches
prior releases (`PadToKBM-v3.1.0-test/`; before the rename
`dualsense-mapper-v2.1.0-test/`, etc.) so the user
can find it by version. Do NOT skip this step when finishing a
feature — the user expects the exe in Downloads, not just a
SendUserFile attachment.

### What "I built the exe" means without these steps

It means you built the CLI binary, not the GUI binary. That is not
useful to the user. Re-read this section before assuming a one-liner
`cargo build` is enough.

## GUI frontend verification (browser click-through)

Any change to `rust/web/` (the Tauri frontend) MUST be verified by
actually loading the page in a browser and clicking every control —
not by reading the diff and asserting it looks right. "It compiles"
is not verification for a chrome layer that only fails at runtime.

The catch: `rust/web/ipc.js` throws on load if `window.__TAURI__` is
missing, so the frontend **cannot be opened in a plain browser**. You
must inject a mock `window.__TAURI__` (an `core.invoke` switch + an
`event.listen` registry) **before** the module scripts run, feed
`rust/config.example.json` as the `get_config` reply, and serve
`rust/web/` over a static HTTP server.

The recipe is automated: `scripts/verify/web-smoke.mjs` (run by
`scripts/verify.sh --only web`) serves `rust/web/` with a mock that holds
the config in memory, applies every mutating command, emits
`config-changed` and records each call with its arguments. Extend it for
every new control; do not hand-check what it can assert.

The board's IPC surface the mock answers: reads `get_controller_status`,
`get_config`, `get_ui_prefs`, `get_app_version`, `get_config_path`,
`get_stick_positions`; mutators `set_binding`, `swap_bindings`,
`set_trigger_mode`, `clear_pad`, `reset_pad`, `set_pad_tables`,
`set_macro`, `rename_macro`, `delete_macro`, `set_settings`,
`set_mapping_enabled`, `set_rumble_on_connect`, `set_lightbar`, `save_stick_cal`,
`reset_stick_cal`, `set_ui_prefs`; runtime-only `set_pad_off` (header 關,
returns the off pads, no `config-changed`); ViGEmBus `get_vigem_status`,
`install_vigem`; fire-and-forget
`set_capture_active`, `disconnect_gamepad`, `open_config_in_editor`,
`open_config_folder`. Events the board listens for: `controller-status`
(`connected` bool + `connected_pads` + `off_pads` + `warnings`),
`button-down/up`, `touchpad-click/hover`, `activity` (with `source`:
`{button}` / `{macro}`), `config-changed`.

What a PASS requires, captured as evidence:
- Every header / slot / palette / footer / editor / settings control
  clicks or drags without throwing, and sends the expected command with
  the expected arguments (`#board [data-pad]`, `[data-slot]`,
  `[data-tile]`, `[data-act]`, `[data-qs]`, `[data-medit]`, `[data-stab]`).
- `window` collects **zero** `error` / `unhandledrejection` /
  `pageerror`. (A `GET /favicon.ico 404` is the one expected,
  harmless miss.)
- Pure modules pass `node --test rust/web/tests/*.test.mjs`.
- Every glyph of shipped web text is in the bundled Cubic 11 subset
  (`scripts/verify/font-coverage.py`; re-run `python3 scripts/build-font.py`
  after adding UI text). The font tooling is pinned in the repo
  (`scripts/tools/requirements.txt`, Cubic 11 tag + sha256 in
  `scripts/font_tools.py`) and fetched on first use into
  `${XDG_CACHE_HOME:-~/.cache}/pad-to-kbm-tools/`; nothing to install by hand.
- Screenshot `rust/target/verify/web-main.png`; README pictures come from
  `scripts/readme-shots.mjs`.
- The app and tray icons (`rust/icons/*.ico`) are the pixel grids in
  `scripts/icon-art.mjs`, rendered by `scripts/build-icons.mjs` (previews in
  `rust/target/icons-preview/`); re-run it when `icon-art.mjs` changes.

web-smoke runs in plain Chromium, so it cannot see what WebView2's host
layer changes. One such trap is pinned by `rust/tests/tauri_conf.rs`: the
window's `dragDropEnabled` must stay `false`, or Tauri's file-drop handler
swallows every HTML5 drag on Windows (tile → slot, swap, drag-out clear,
macro step order) while web-smoke stays green. That shipped in v3.0.0 and
was fixed in v3.1.1.

For scroll / layout bugs, measure don't eyeball: the board is a fixed
980×640 surface inside the page-drawn 994×669 window (`#win`: frame + title
bar, v3.3.0), scaled by a whole number of device pixels per art pixel
(`chrome.js` `fitWindow`; Rust sizes the window, `gui/winscale.rs`;
`decorations` / `transparent` are pinned by `rust/tests/tauri_conf.rs`); at several viewport
sizes assert `scrollWidth == clientWidth`, `scrollHeight ==
clientHeight` and that `#board` lies inside the viewport, and read
computed `overscroll-behavior-x` (`html, body { overscroll-behavior:
none }`) — nothing may rubber-band or scroll the window itself.

## Self-verification (agent runs this before every release PR)

`scripts/verify.sh` is the one command that verifies a release. Run it
from the repo root; it prints a PASS / FAIL / SKIP table and exits
non-zero on any FAIL. Logs: `rust/target/verify/<layer>.log`.

| Layer | Covers |
|---|---|
| `linux` | `cargo test`, all suites, on the Linux dev host |
| `windows-tests` | `cargo xwin test --no-run` (MSVC target); every test exe runs natively on Windows via WSL interop |
| `gui-smoke` | GUI exe build; launched alone (`--dry-run`) from a fresh `%LOCALAPPDATA%\Temp` dir: titled window up, first-run `padtokbm.json` written, survives a valid and an invalid config hot-edit; a second launch beside a pre-v3.1.0 `dualsense-mapper.json` must adopt it byte for byte (`.bak` kept); closed gracefully (WM_CLOSE), which must log the engine shutdown, then cleaned up by PID (a launch whose exe PID is lost is killed by temp-dir path and FAILs); temp dirs removed. SKIPs if the user's copy (new or old exe name) is running |
| `web` | the click-through recipe above, automated: `scripts/verify/web-smoke.mjs` (Playwright + cached Chromium, mocked `__TAURI__`); screenshots in `rust/target/verify/` |

Flags: `--quick` (linux + windows-tests), `--skip-gui`, `--skip-web`,
`--only <layer>`, `--strict` (a selected layer that SKIPs FAILs; CI uses it). Toolchain paths come from the untracked
`~/.config/pad-to-kbm/verify-env.sh` (falls back to the pre-rename
`~/.config/dualsense-mapper/verify-env.sh`); it holds machine-local paths
only and never goes in the repo. The layers need cargo, cargo-xwin with
clang-cl / llvm-lib, libudev (pkg-config), node and a cached Playwright
Chromium. A missing tool turns its
layer into SKIP with a hint (missing cargo FAILs `linux`).

- The agent runs `scripts/verify.sh` (all layers) before every release
  PR and pastes its summary table into the PR body's **Test plan**. A
  SKIP needs a one-line reason there.
- CI (`.github/workflows/ci.yml`, once per pull request) runs the `linux`
  and `web` layers through `scripts/verify.sh --strict` (a SKIP is a FAIL
  there) and `cargo test --release` natively on a Windows runner. It does
  not run gui-smoke, so the local run stays mandatory. Merge only a branch
  that contains `origin/main` and a green CI.
- Do not hand the maintainer a manual checklist for anything the script
  covers. Only what needs a physical controller (radio link, pairing,
  real button presses / macros on hardware) goes to the human, batched
  into one guided run, not drip-fed.

## Iron rules for the Rust source

These are project-specific invariants. Breaking any of them silently
regresses real, observed bugs the codebase already fixed once:

1. **`config.rs::Config::validate` must reject any config in which a
   pad's `buttons` table (`pads.dualsense`, `pads.ns_pro`, `pads.ns2_pro`)
   does not list every button id `0..=32` (the union of all supported
   pads' ids).** Missing ids are a user error, not a silent skip — except
   for the v2.1.0 touchpad-quadrant additions and the v2.5.0 Capture
   button and the v2.6.0 Switch 2 Pro additions (25..=32), which the load
   path auto-fills as `Unbound` so older configs continue to parse, and a
   pad with no table at all, which the load path fills with the default
   table. A version-1 file (one top-level `buttons`) is migrated into
   every pad first (`config::migrate_v1`). The validator itself still
   requires the full range on every pad; auto-fill happens before
   validation, not inside it.
2. **`safety.rs` is the single source of truth for "what keys (and
   mouse buttons) are physically pressed right now."** `keyboard.rs`
   always asks `KeyState::press` / `release` for the edge transition
   and only forwards a real synth on `Edge::Press` / `Edge::Release`.
   `mouse.rs` mirrors this through `press_mouse` / `release_mouse`.
   Bypassing this leaks a stuck KEYDOWN or MOUSEDOWN — see commit
   `1d64f25` for the keyboard bug shape; the mouse half landed in
   v2.1.0.
3. **`KeyboardSink::Drop`, `MouseSink::Drop`, the panic hook in
   `app.rs`, and the engine's Disconnected handler must run
   `release_all_held` unconditionally.** They are the last-line defence
   against stuck keys / stuck mouse buttons when the process dies or the
   controller drops. `release_all_held` releases every held key **and**
   every held mouse button, and never waits on `min_press_ms` —
   shutdown beats anti-cheat profile. The virtual Xbox pad (Switch 2 Pro,
   no-mapping mode) is likewise unplugged on Disconnected, pause, shutdown
   and in the panic hook (`emergency_unplug`); `vigem-client` also unplugs
   on Drop.
4. **Macro threads drain every unmatched `Press` as `Release` on every
   exit path** (cancel flag, no-repeat completion, channel close). A
   macro cancelled between Press Left and the scheduled Release Left
   would otherwise strand a KEYDOWN that breaks every subsequent
   `Left` binding. See `macro_engine.rs::run_macro` and the
   `cancel_between_press_and_release_drains_held_keys` test.
5. **`min_press_ms`, `tick_jitter_ms`, and macro `delay_ms` are
   `[min, max]` ranges, never constants.** Config validator enforces
   `min < max`. Constant timing is the single most reliable
   anti-cheat fingerprint; do not add a "convenience" form that
   allows it.
6. **`gamepad.rs` is the platform-quirk layer.** It is the only place
   that knows D-pad can be either discrete buttons (`Button::DPadLeft`
   etc.) or a hat axis (`Axis::DPadX/Y`), and that triggers can be
   reported in `[-1, 1]` or `[0, 1]`. Mapper / keyboard see one
   uniform event surface. Per-kind protocol modules (DualSense, Switch
   Pro, Switch 2 Pro) and the BLE transport (`ns2_ble.rs`) live in this
   gamepad layer too, and `SourceHub` arbitrates so that one controller
   owns the session at a time. The user stick calibration (`stick_cal`,
   `gamepad/stick_cal.rs`) is applied in `SourceHub` to the owner's stick
   events, on top of the pad's factory calibration. New platform quirks
   live here.
7. **No process hooking, no DLL injection, no driver.** User-mode
   `SendInput` (Windows) / `CGEvent` (macOS) only, via the `enigo`
   crate. Anything else lands us in actual cheat-software territory.
   Single exception (v2.6.0): a virtual Xbox 360 pad through an
   externally installed ViGEmBus, used only for a controller Windows
   cannot use natively (the Switch 2 Pro, `ControllerKind::needs_virtual_pad`)
   and only while its mapping is OFF. The app never ships a driver. On the
   user's click it may download the pinned official ViGEmBus installer,
   verify size + sha256, and run it; the user goes through the installer's
   own elevation prompt (`vigem_setup.rs`, v3.2.0). Hooking and injection
   stay forbidden.
8. **The exe is double-clicked, not invoked from a terminal.** Primary
   end-user flow on Windows is Explorer → double-click → GUI window
   opens. That means:
   - GUI mode (default in v1.0.0+) is the user-facing path. The
     window must be visible within ~1 second of process start.
     Errors before window creation must render to a fallback modal
     (e.g. `MessageBoxW`), not flash and disappear.
   - `--cli` is the explicit legacy console-mode opt-in. In CLI mode,
     first-run may **not** call `anyhow::bail!` / exit non-zero
     before reaching the main loop. Any uncaught error from `main`
     must pause (read stdin) before exit, so the error is visible.
     `--no-pause` is the explicit opt-out for CI use.
   - The first-run-written `padtokbm.json` carries an inline
     keyboard cheat sheet in `_help` and `_keyboard_keys` fields so
     a user editing the file in Notepad has the reference inline.
     `serde` ignores unknown fields, so these are documentation
     only — but they are load-bearing UX, not decoration.
9. **The GUI (`gui/` module + `web/` frontend) is a chrome layer.**
   No mapping decision, no key synth, no macro scheduling may live
   in JavaScript. The frontend captures input and renders state;
   everything that mutates runtime state goes through a Rust
   `#[tauri::command]` (see `rust/src/gui/commands.rs`) that calls
   the same engine code paths as the v0.1.x CLI. JS that calls
   `SendInput` (or the equivalent) is a rejection. The IPC surface
   is the only authorised path from frontend to engine — adding
   business logic to the frontend that bypasses it is a regression
   even if the feature works in isolation.
10. **Any IPC command that mutates `config` MUST emit
    `config-changed` itself on success.** The filesystem watcher
    in `runtime.rs` is the second source of truth, kept around
    for external edits (Notepad while the GUI is running). It is
    NOT a reliable refresh path for IPC-driven writes because
    `notify-rs` on Windows loses the watch handle across the
    atomic-rename used by `write_atomic` — every IPC mutator
    after the first would otherwise silently appear to do
    nothing. See v1.2.0 commits for the bug shape.
11. **Controller connection state is ground truth from raw
    HID, not heuristic.** v2.0.0 replaced the gilrs path with
    `hidapi` against the DualSense BT 0x31 report. The HID
    worker thread's state machine (Searching → Handshaking →
    Streaming) emits `Connected` on the first decoded input
    frame and `Disconnected` on either read-error or a per-kind
    report-stream deadline (DualSense 800 ms, Switch Pro
    1000 ms; the Switch 2 Pro over BLE uses the BLE disconnect event or
    1000 ms without a decoded report). `SourceHub` forwards only the
    owner's events. No more `is_connected()` cache, no
    more "must press a button to confirm" UX hack — the radio
    link presence is observed directly via report stream
    presence/absence. The worker that owns the session may also write
    rumble and lightbar output reports to its pad (on connect and while
    streaming); these writes never affect connection state.

## Anti-cheat self-discipline

This tool is not a cheat — it remaps physical inputs to keyboard
inputs. The synthesis pipeline avoids fixed-period patterns so it
doesn't get misclassified as one. The rules (the user-facing summary
is the README FAQ「會不會被當成外掛？」):

- Macro step delays are always `[min, max]` ranges, never constants.
- Synthesized keys honor `min_press_ms` (randomized minimum down/up gap).
- Simultaneous events on the same gamepad tick are spread by `tick_jitter_ms`.
- No process hooking, DLL injection, or driver (the ViGEmBus exception
  in iron rule 7 aside: never bundled, only the verified official
  installer on the user's click). User-mode `SendInput` / `CGEvent` only.
- `log_events: true` writes every synthesized event with a timestamp
  so the user can self-audit.

They are enforced mechanically by config validation + the iron rules
above. Do not ship a feature that requires bypassing them.

Note: **`SendInput` does not trigger Windows OS auto-repeat.** This is
expected — auto-repeat is a kbdclass-driver-level feature for real
hardware HID streams. Games that poll key state (the vast majority)
work identically to a real keyboard; only WM_CHAR-driven text input
visibly differs. If a `turbo: true` feature ever ships, gate it
behind explicit config opt-in with a jittered interval — never a
constant tick.

## Versioning rules

Project follows [Semantic Versioning](https://semver.org/) with these
project-specific clarifications. Picking the wrong axis is a
release-flow bug; halt and reclassify before committing.

- **MAJOR (`X.0.0`)** — backwards-incompatible behaviour. Examples:
  - Dropping support for a previously-supported pad (e.g. v2.0.0
    removes gilrs → non-DualSense pads no longer work).
  - Breaking the on-disk `padtokbm.json` schema (renamed
    fields, removed keys, changed enum values).
  - Removing or renaming a CLI flag.
  - Changing the semantics of an existing binding type (e.g. if
    `Binding::Key` started ignoring the modifier).
  - Replacing the gamepad event source / abstraction layer.
- **MINOR (`X.Y.0`)** — new functionality, backwards compatible.
  Examples:
  - New IPC command added to `commands.rs`.
  - New binding type (`Binding::Touchpad`) added to the enum.
  - New optional config field with a default.
  - New keyboard mapping support (e.g. punctuation keys).
- **PATCH (`X.Y.Z`)** — bug fix only. No new feature, no schema
  change. Examples:
  - Fixing a stuck KEYDOWN.
  - Fixing the d-pad stick quarter rotation map.
  - Fixing the macro UI refresh trigger.
- **Pre-1.0** (`0.X.Y`) — semver suspended; MINOR may break.

When deciding between MINOR and MAJOR, ask: *will any user's existing
config file or hardware setup stop working after the upgrade?* If
yes, MAJOR.

## Release flow — files that must move together

A version bump touches **three** files in the release commit:

1. `rust/Cargo.toml` — `[package] version` field
2. `rust/tauri.conf.json` — `"version"` field (must match Cargo.toml)
3. `CHANGELOG.md` — new top-level `## [X.Y.Z] - YYYY-MM-DD` heading

Bumping fewer than three is a release-flow bug; halt.
`rust/tests/version_consistency.rs` fails `cargo test` (and so CI) when the
three disagree.

### Tagging

Release tag uses plain `vX.Y.Z` (no plugin-name prefix). After a
release PR has been merged to `main`:

```bash
git fetch origin && git checkout main && git pull --ff-only
git tag -a vX.Y.Z -m "Release vX.Y.Z" <merge-commit-sha>
git push origin vX.Y.Z
```

Then attach the GUI `PadToKBM.exe` (the WebView2-static MSVC
build above) + a sample `padtokbm.json` as the GitHub Release
artefact (`gh release create vX.Y.Z
rust/target/x86_64-pc-windows-msvc/release/PadToKBM.exe
rust/config.example.json --title "vX.Y.Z" --notes-file <changelog section>
--latest`).

End users download the two files, drop them in a folder, run.

## Branch flow

- Feature branches: `feat/<topic>` for additions, `fix/<topic>` for
  bug fixes, `chore/<topic>` for non-code chores.
- Release branches: `release/<vX.Y.Z>` for the PR that bumps Cargo.toml
  + CHANGELOG. Title `release(vX.Y.Z): ...`, PR body summarising what
  changed since the prior tag.
- Never commit directly to `main`. Tags only go on `main` merge commits.
- Force-pushing `main` is forbidden. Force-pushing feature branches is
  allowed if the branch is yours.

## No `--admin` override

CI failures (`cargo test`, `cargo clippy` if added later, any future
lint) **cannot** be bypassed with `gh pr merge --admin` or
`--no-verify`. Fix the source, then re-merge.

If a test really is wrong, amend the test in the same PR with a
written justification in the PR description. The fact that a lint
job looks advisory is exactly how the paperwork v2.11→v2.13 incident
shipped three broken releases under admin override — don't repeat it.

## What will be rejected

- Edits to `legacy-python/` (unless explicitly to fix a security
  vulnerability; ask first).
- Synthesized-input code that bypasses `safety.rs` refcount.
- Macro / config additions that allow constant (non-jittered)
  timing.
- Features that lock the binary to one OS without a clear story
  for the other (Phase 2 macOS is the explicit target — don't add
  Windows-only behaviour without a `#[cfg(target_os = "macos")]`
  counterpart or a deliberate "Phase 2 TBD" CHANGELOG note). The Switch 2
  Pro (BLE + virtual pad) is such a Phase 2 TBD item on macOS.
- Tracking `docs/superpowers/` content.
