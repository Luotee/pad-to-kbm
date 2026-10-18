#!/usr/bin/env bash
# scripts/verify.sh — release self-verification for PadToKBM.
#
# Layers (run in this order):
#   linux          cargo test (all suites) on the Linux dev host
#   windows-tests  cargo xwin test --no-run for x86_64-pc-windows-msvc, then
#                  every test exe runs natively on Windows via WSL interop
#   gui-smoke      GUI exe build, launched alone from a fresh %LOCALAPPDATA%\Temp
#                  dir: window up, first-run config written, config hot-edit
#                  (valid + invalid) does not kill the process
#   web            headless Chromium click-through of rust/web/ with a mocked
#                  window.__TAURI__ (scripts/verify/web-smoke.mjs)
#
# Usage:
#   scripts/verify.sh                 all layers
#   scripts/verify.sh --quick         linux + windows-tests only
#   scripts/verify.sh --skip-gui      everything except gui-smoke
#   scripts/verify.sh --skip-web      everything except web
#   scripts/verify.sh --only <layer>  one layer
#
# Logs: rust/target/verify/<layer>.log. Exit status is non-zero if any layer
# FAILs. A missing tool turns its layer into SKIP with a hint (except cargo
# for the linux layer, which is a FAIL).

set -uo pipefail

# Machine-local toolchain env (cargo, libudev, clang-cl / llvm-lib paths).
# Read from the first of these that exists (the second is the pre-rename path).
for VERIFY_ENV_FILE in "$HOME/.config/pad-to-kbm/verify-env.sh" \
                       "$HOME/.config/dualsense-mapper/verify-env.sh"; do
    [ -f "$VERIFY_ENV_FILE" ] && break
done
if [ -f "$VERIFY_ENV_FILE" ]; then
    # shellcheck source=/dev/null
    . "$VERIFY_ENV_FILE"
fi

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUST_DIR="$REPO_ROOT/rust"
VERIFY_DIR="$RUST_DIR/target/verify"
WIN_TARGET="x86_64-pc-windows-msvc"
GUI_EXE_NAME="PadToKBM.exe"
# Pre-v3.1.0 name: same Tauri identifier, so a running copy swallows our launch.
LEGACY_EXE_NAME="dualsense-mapper.exe"
CONFIG_NAME="padtokbm.json"
LEGACY_CONFIG_NAME="dualsense-mapper.json"
GUI_STARTUP_TIMEOUT_S=8
GUI_HOTEDIT_WAIT_S=3
PLAYWRIGHT_CORE_VER="1.63.0"

ALL_LAYERS=(linux windows-tests gui-smoke web)
declare -A SELECTED=()
declare -A STATUS=()
declare -A DETAIL=()
declare -A DURATION=()

# gui-smoke state, read by the cleanup trap.
GUI_PID=""
GUI_TMP_LINUX=""

# ─── Helpers ─────────────────────────────────────────────────────────────────

usage() {
    sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
}

log_path() {
    echo "$VERIFY_DIR/$1.log"
}

have() {
    command -v "$1" >/dev/null 2>&1
}

record() {
    # record <layer> <PASS|FAIL|SKIP> <detail>
    STATUS[$1]="$2"
    DETAIL[$1]="$3"
}

is_wsl() {
    have cmd.exe && have powershell.exe && have wslpath
}

# Run a cmd.exe command from a Windows-side cwd (cmd.exe warns about UNC
# cwds otherwise) and strip the CRs from its output.
win_cmd() {
    (cd /mnt/c && cmd.exe /c "$@" 2>/dev/null) | tr -d '\r'
}

# Run a Windows exe directly (no cmd.exe re-quoting), same cwd / CR handling.
win_run() {
    (cd /mnt/c && "$@" 2>/dev/null) | tr -d '\r'
}

win_ps() {
    (cd /mnt/c && powershell.exe -NoProfile -NonInteractive -Command "$1" 2>/dev/null) | tr -d '\r'
}

# Echo "<passed> <failed> <result-lines>" summed over every libtest
# "test result:" line in a file.
sum_test_results() {
    local file="$1"
    local passed=0 failed=0 lines=0 p f
    while IFS= read -r line; do
        p="$(echo "$line" | sed -E 's/.* ([0-9]+) passed;.*/\1/')"
        f="$(echo "$line" | sed -E 's/.* ([0-9]+) failed;.*/\1/')"
        passed=$((passed + p))
        failed=$((failed + f))
        lines=$((lines + 1))
    done < <(tr -d '\r' < "$file" | grep -E '^test result: ')
    echo "$passed $failed $lines"
}

# Echo the first missing tool among the arguments, or nothing.
first_missing() {
    local tool
    for tool in "$@"; do
        if ! have "$tool"; then
            echo "$tool"
            return
        fi
    done
}

# Preflight for the cross-compile layers. Echo a hint on failure.
preflight_xwin() {
    local missing
    if ! is_wsl; then
        echo "not on WSL (no cmd.exe / powershell.exe / wslpath) — Windows layers need WSL interop"
        return 1
    fi
    missing="$(first_missing cargo cargo-xwin clang-cl llvm-lib python3)"
    if [ -n "$missing" ]; then
        echo "missing $missing — install it or set its PATH in $VERIFY_ENV_FILE (cargo-xwin: cargo install cargo-xwin)"
        return 1
    fi
    return 0
}

# ─── Layer: linux ────────────────────────────────────────────────────────────

layer_linux() {
    local log rc totals passed failed suites
    log="$(log_path linux)"
    if ! have cargo; then
        record linux FAIL "cargo not found (rustup, or set PATH in $VERIFY_ENV_FILE)"
        return
    fi
    if ! have pkg-config || ! pkg-config --exists libudev; then
        record linux SKIP "libudev not found by pkg-config — sudo apt install -y pkg-config libudev-dev"
        return
    fi

    (cd "$RUST_DIR" && cargo test) >"$log" 2>&1
    rc=$?
    totals="$(sum_test_results "$log")"
    read -r passed failed suites <<<"$totals"

    if [ "$rc" -eq 0 ] && [ "$failed" -eq 0 ] && [ "$passed" -gt 0 ]; then
        record linux PASS "$passed passed, 0 failed ($suites suites)"
    else
        record linux FAIL "cargo test rc=$rc; $passed passed, $failed failed ($suites suites)"
    fi
}

# ─── Layer: windows-tests ────────────────────────────────────────────────────

list_test_exes() {
    # stdin: cargo --message-format=json. stdout: one test executable per line.
    python3 -c '
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line.startswith("{"):
        continue
    msg = json.loads(line)
    if msg.get("reason") != "compiler-artifact":
        continue
    if not msg.get("profile", {}).get("test"):
        continue
    if msg.get("executable"):
        print(msg["executable"])
'
}

layer_windows_tests() {
    local log json hint rc exe exe_out totals p f n
    local total_passed=0 total_failed=0 exe_count=0 bad_exes=0
    log="$(log_path windows-tests)"
    json="$VERIFY_DIR/windows-tests.cargo.json"

    if ! hint="$(preflight_xwin)"; then
        record windows-tests SKIP "$hint"
        return
    fi

    (cd "$RUST_DIR" && cargo xwin test --no-run --release --target "$WIN_TARGET" \
        --message-format=json) >"$json" 2>"$log"
    rc=$?
    if [ "$rc" -ne 0 ]; then
        record windows-tests FAIL "cargo xwin test --no-run rc=$rc"
        return
    fi

    while IFS= read -r exe; do
        exe_count=$((exe_count + 1))
        exe_out="$VERIFY_DIR/windows-tests.$(basename "$exe").out"
        {
            echo "=== $exe"
        } >>"$log"
        # </dev/null: the exe would otherwise eat this loop's stdin.
        # cwd = rust/: compile-time CARGO_MANIFEST_DIR paths resolve through
        # the \\wsl.localhost UNC cwd.
        (cd "$RUST_DIR" && "$exe" </dev/null) 2>&1 | tr -d '\r' >"$exe_out"
        rc=${PIPESTATUS[0]}
        cat "$exe_out" >>"$log"
        totals="$(sum_test_results "$exe_out")"
        read -r p f n <<<"$totals"
        total_passed=$((total_passed + p))
        total_failed=$((total_failed + f))
        if [ "$rc" -ne 0 ] || [ "$n" -eq 0 ]; then
            bad_exes=$((bad_exes + 1))
            echo "!!! $(basename "$exe") rc=$rc result-lines=$n" >>"$log"
        fi
    done < <(list_test_exes <"$json")

    if [ "$exe_count" -eq 0 ]; then
        record windows-tests FAIL "no test executables in cargo output"
    elif [ "$bad_exes" -eq 0 ] && [ "$total_failed" -eq 0 ]; then
        record windows-tests PASS "$total_passed passed, 0 failed ($exe_count exes on Windows)"
    else
        record windows-tests FAIL "$total_passed passed, $total_failed failed, $bad_exes/$exe_count exes non-zero"
    fi
}

# ─── Layer: gui-smoke ────────────────────────────────────────────────────────

gui_alive() {
    win_run tasklist.exe /FI "PID eq $1" /NH | grep -i "$GUI_EXE_NAME" >/dev/null
}

gui_cleanup() {
    local i
    if [ -n "$GUI_PID" ]; then
        # Only ever kill the PID we launched (and its WebView2 children).
        win_run taskkill.exe /PID "$GUI_PID" /T /F >/dev/null
        GUI_PID=""
    fi
    if [ -n "$GUI_TMP_LINUX" ] && [ -d "$GUI_TMP_LINUX" ]; then
        # The exe image stays locked for a moment after the kill.
        for i in 1 2 3 4 5 6 7 8 9 10; do
            rm -rf "$GUI_TMP_LINUX" 2>/dev/null && break
            sleep 0.5
            : "$i"
        done
        GUI_TMP_LINUX=""
    fi
}

# Assert the auto-generated config parses and has the first-run keys.
gui_check_config() {
    python3 - "$1" <<'PY'
import json, sys
doc = json.load(open(sys.argv[1], encoding="utf-8"))
missing = [k for k in ("_help", "pads", "mapping_enabled") if k not in doc]
missing += ["pads." + p for p in ("dualsense", "ns_pro", "ns2_pro") if p not in doc.get("pads", {})]
if missing:
    print("missing keys: " + ", ".join(missing))
    sys.exit(1)
if doc.get("version") != 2:
    print("config version %r, expected 2" % doc.get("version"))
    sys.exit(1)
PY
}

# Rewrite the config in place (truncate + write, no rename) with one
# mapping_enabled key changed.
gui_set_mapping_enabled() {
    python3 - "$1" "$2" "$3" <<'PY'
import json, sys
path, slug, value = sys.argv[1], sys.argv[2], sys.argv[3] == "true"
with open(path, encoding="utf-8") as f:
    doc = json.load(f)
doc.setdefault("mapping_enabled", {})[slug] = value
with open(path, "w", encoding="utf-8") as f:
    json.dump(doc, f, ensure_ascii=False, indent=2)
PY
}

# Launch the exe in a fresh temp dir that holds a pre-v3.1.0 dualsense-mapper.json
# (the example config with one binding changed). Expect it renamed to
# padtokbm.json byte for byte, a .bak copy beside it, the old name gone.
gui_legacy_adoption() {
    local exe="$1" localappdata="$2" stamp win_dir seed deadline
    stamp="$(date +%Y%m%d-%H%M%S)-$$-legacy"
    win_dir="$localappdata\\Temp\\dsm-verify-$stamp"
    GUI_TMP_LINUX="$(wslpath -u "$win_dir")"
    mkdir -p "$GUI_TMP_LINUX"
    cp "$exe" "$GUI_TMP_LINUX/"
    seed="$GUI_TMP_LINUX/$LEGACY_CONFIG_NAME.seed"
    python3 - "$RUST_DIR/config.example.json" "$seed" <<'PY'
import json, sys
cfg = json.load(open(sys.argv[1], encoding="utf-8"))
cfg["pads"]["dualsense"]["buttons"]["0"]["value"] = "F9"
json.dump(cfg, open(sys.argv[2], "w", encoding="utf-8"), ensure_ascii=False, indent=2)
PY
    cp "$seed" "$GUI_TMP_LINUX/$LEGACY_CONFIG_NAME"
    # The first launch was just killed; until its process is gone the
    # single-instance plugin would make this one hand over and exit.
    deadline=$((SECONDS + 10))
    while [ "$SECONDS" -lt "$deadline" ] &&
          win_run tasklist.exe /FI "IMAGENAME eq $GUI_EXE_NAME" /NH | grep -i "$GUI_EXE_NAME" >/dev/null; do
        sleep 0.5
    done
    GUI_PID="$(win_ps "(Start-Process -FilePath '$win_dir\\$GUI_EXE_NAME' -ArgumentList '--dry-run' -WorkingDirectory '$win_dir' -PassThru).Id")"
    if ! [[ "$GUI_PID" =~ ^[0-9]+$ ]]; then
        GUI_PID=""; gui_cleanup; LEGACY_MSG="could not launch the exe"; return 1
    fi
    deadline=$((SECONDS + GUI_STARTUP_TIMEOUT_S))
    while [ "$SECONDS" -lt "$deadline" ] && [ ! -f "$GUI_TMP_LINUX/$CONFIG_NAME" ]; do
        sleep 0.5
    done
    sleep 1
    if [ ! -f "$GUI_TMP_LINUX/$CONFIG_NAME" ]; then
        gui_cleanup; LEGACY_MSG="$CONFIG_NAME not created from $LEGACY_CONFIG_NAME"; return 1
    fi
    if ! cmp -s "$seed" "$GUI_TMP_LINUX/$CONFIG_NAME"; then
        gui_cleanup; LEGACY_MSG="$CONFIG_NAME differs from the old $LEGACY_CONFIG_NAME"; return 1
    fi
    if [ ! -f "$GUI_TMP_LINUX/$LEGACY_CONFIG_NAME.bak" ] || [ -f "$GUI_TMP_LINUX/$LEGACY_CONFIG_NAME" ]; then
        gui_cleanup; LEGACY_MSG="expected $LEGACY_CONFIG_NAME.bak and no $LEGACY_CONFIG_NAME"; return 1
    fi
    if ! gui_alive "$GUI_PID"; then
        GUI_PID=""; gui_cleanup; LEGACY_MSG="process exited after adopting the old config"; return 1
    fi
    gui_cleanup
    LEGACY_MSG="old $LEGACY_CONFIG_NAME adopted"
}

layer_gui_smoke() {
    local log hint rc exe running localappdata stamp win_dir win_exe cfg
    local title="" deadline msg
    log="$(log_path gui-smoke)"
    : >"$log"

    if ! hint="$(preflight_xwin)"; then
        record gui-smoke SKIP "$hint"
        return
    fi
    local missing
    missing="$(first_missing tasklist.exe taskkill.exe)"
    if [ -n "$missing" ]; then
        record gui-smoke SKIP "missing $missing (Windows System32 not on PATH?)"
        return
    fi

    echo "=== build" >>"$log"
    (cd "$RUST_DIR" && WEBVIEW2_STATIC=true cargo xwin build --release \
        --target "$WIN_TARGET" --features gui) >>"$log" 2>&1
    rc=$?
    exe="$RUST_DIR/target/$WIN_TARGET/release/$GUI_EXE_NAME"
    if [ "$rc" -ne 0 ] || [ ! -f "$exe" ]; then
        record gui-smoke FAIL "GUI build rc=$rc"
        return
    fi

    local name
    for name in "$GUI_EXE_NAME" "$LEGACY_EXE_NAME"; do
        running="$(win_run tasklist.exe /FI "IMAGENAME eq $name" /NH)"
        if echo "$running" | grep -i "$name" >/dev/null; then
            echo "$running" >>"$log"
            record gui-smoke SKIP "built OK; $name already running on Windows — single-instance would just focus it (close it and re-run)"
            return
        fi
    done

    localappdata="$(win_cmd echo %LOCALAPPDATA%)"
    stamp="$(date +%Y%m%d-%H%M%S)-$$"
    win_dir="$localappdata\\Temp\\dsm-verify-$stamp"
    GUI_TMP_LINUX="$(wslpath -u "$win_dir")"
    mkdir -p "$GUI_TMP_LINUX"
    cp "$exe" "$GUI_TMP_LINUX/"
    win_exe="$win_dir\\$GUI_EXE_NAME"
    cfg="$GUI_TMP_LINUX/$CONFIG_NAME"
    echo "=== launch $win_exe (cwd $win_dir)" >>"$log"

    # --dry-run: the window, config and watcher paths are identical, but a
    # paired controller on this desktop cannot type into the user's apps.
    GUI_PID="$(win_ps "(Start-Process -FilePath '$win_exe' -ArgumentList '--dry-run' -WorkingDirectory '$win_dir' -PassThru).Id")"
    if ! [[ "$GUI_PID" =~ ^[0-9]+$ ]]; then
        echo "Start-Process output: $GUI_PID" >>"$log"
        GUI_PID=""
        gui_cleanup
        record gui-smoke FAIL "could not launch the exe (see log)"
        return
    fi
    echo "pid $GUI_PID" >>"$log"

    deadline=$((SECONDS + GUI_STARTUP_TIMEOUT_S))
    while [ "$SECONDS" -lt "$deadline" ]; do
        if ! gui_alive "$GUI_PID"; then
            break
        fi
        title="$(win_ps "(Get-Process -Id $GUI_PID -ErrorAction SilentlyContinue).MainWindowTitle")"
        if [ -n "$title" ] && [ -f "$cfg" ]; then
            break
        fi
        sleep 0.5
    done
    echo "window title: '$title'" >>"$log"

    if ! gui_alive "$GUI_PID"; then
        GUI_PID=""
        gui_cleanup
        record gui-smoke FAIL "process exited within ${GUI_STARTUP_TIMEOUT_S}s of launch"
        return
    fi
    if [ -z "$title" ]; then
        gui_cleanup
        record gui-smoke FAIL "no top-level window with a title within ${GUI_STARTUP_TIMEOUT_S}s"
        return
    fi
    if [ ! -f "$cfg" ]; then
        gui_cleanup
        record gui-smoke FAIL "$CONFIG_NAME not auto-generated next to the exe"
        return
    fi
    if ! msg="$(gui_check_config "$cfg" 2>&1)"; then
        echo "config check: $msg" >>"$log"
        gui_cleanup
        record gui-smoke FAIL "auto-generated config: $msg"
        return
    fi

    echo "=== hot-edit mapping_enabled.dualsense=false" >>"$log"
    gui_set_mapping_enabled "$cfg" dualsense false >>"$log" 2>&1
    sleep "$GUI_HOTEDIT_WAIT_S"
    if ! gui_alive "$GUI_PID"; then
        GUI_PID=""
        gui_cleanup
        record gui-smoke FAIL "process died after a valid config hot-edit"
        return
    fi

    echo "=== hot-edit invalid slug mapping_enabled.ps5=false" >>"$log"
    gui_set_mapping_enabled "$cfg" ps5 false >>"$log" 2>&1
    sleep "$GUI_HOTEDIT_WAIT_S"
    if ! gui_alive "$GUI_PID"; then
        GUI_PID=""
        gui_cleanup
        record gui-smoke FAIL "process died after an invalid config hot-edit (watcher must reject, not crash)"
        return
    fi

    gui_cleanup

    # Upgrade from a pre-v3.1.0 folder: the old config must be adopted as is.
    # (runs in this shell, not $(...), so the exit trap still sees GUI_PID)
    LEGACY_MSG=""
    echo "=== launch with a pre-v3.1.0 $LEGACY_CONFIG_NAME" >>"$log"
    if ! gui_legacy_adoption "$exe" "$localappdata" >>"$log" 2>&1; then
        echo "legacy adoption: $LEGACY_MSG" >>"$log"
        record gui-smoke FAIL "legacy config: $LEGACY_MSG"
        return
    fi
    record gui-smoke PASS "window '$title'; first-run config OK; survived valid + invalid hot-edit; $LEGACY_MSG"
}

# ─── Layer: web ──────────────────────────────────────────────────────────────

find_node() {
    local candidate major
    for candidate in "$HOME"/.nvm/versions/node/v22*/bin/node; do
        if [ -x "$candidate" ]; then
            echo "$candidate"
            return 0
        fi
    done
    if have node; then
        major="$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null)"
        if [ -n "$major" ] && [ "$major" -ge 18 ]; then
            command -v node
            return 0
        fi
    fi
    return 1
}

find_chrome() {
    local candidate found=""
    for candidate in "$HOME"/.cache/ms-playwright/chromium-*/chrome-linux64/chrome; do
        if [ -x "$candidate" ]; then
            found="$candidate"
        fi
    done
    if [ -z "$found" ]; then
        return 1
    fi
    echo "$found"
}

layer_web() {
    local log node_bin chrome node_dir npm_bin version rc summary
    log="$(log_path web)"
    : >"$log"

    if ! node_bin="$(find_node)"; then
        record web SKIP "node >= 18 not found (expected ~/.nvm/versions/node/v22*)"
        return
    fi
    npm_bin="$(dirname "$node_bin")/npm"
    if ! chrome="$(find_chrome)"; then
        record web SKIP "no cached Chromium — npx playwright@$PLAYWRIGHT_CORE_VER install chromium"
        return
    fi

    node_dir="$VERIFY_DIR/node"
    if [ ! -f "$node_dir/node_modules/playwright-core/package.json" ]; then
        mkdir -p "$node_dir"
        if [ ! -f "$node_dir/package.json" ]; then
            echo '{ "name": "dsm-verify-node", "private": true }' >"$node_dir/package.json"
        fi
        echo "=== npm install playwright-core@$PLAYWRIGHT_CORE_VER" >>"$log"
        (cd "$node_dir" && PATH="$(dirname "$node_bin"):$PATH" \
            PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 "$npm_bin" install --no-audit --no-fund \
            "playwright-core@$PLAYWRIGHT_CORE_VER") >>"$log" 2>&1
        rc=$?
        if [ "$rc" -ne 0 ]; then
            record web SKIP "npm install playwright-core failed (offline?) — see log"
            return
        fi
    fi

    # Pure-module unit tests (rust/web/tests, scripts/tests: *.test.mjs) first.
    echo "=== node --test rust/web/tests scripts/tests" >>"$log"
    "$node_bin" --test "$RUST_DIR"/web/tests/*.test.mjs "$REPO_ROOT"/scripts/tests/*.test.mjs >>"$log" 2>&1
    rc=$?
    units="$(grep -E '^# (pass|fail) ' "$log" | tail -2 | tr '\n' ' ' | sed 's/# //g')"
    if [ "$rc" -ne 0 ]; then
        record web FAIL "unit tests: ${units:-rc=$rc}"
        return
    fi

    version="$(sed -n -E 's/^version = "([^"]+)"/\1/p' "$RUST_DIR/Cargo.toml" | head -1)"
    echo "=== web-smoke (node $node_bin, chrome $chrome)" >>"$log"
    NODE_PATH="$node_dir/node_modules" "$node_bin" "$REPO_ROOT/scripts/verify/web-smoke.mjs" \
        --web-dir "$RUST_DIR/web" \
        --config "$RUST_DIR/config.example.json" \
        --out-dir "$VERIFY_DIR" \
        --chrome "$chrome" \
        --version "$version" >>"$log" 2>&1
    rc=$?
    summary="$(grep -E '^web-smoke: ' "$log" | tail -1 | sed 's/^web-smoke: //')"
    if [ "$rc" -ne 0 ]; then
        record web FAIL "${summary:-web-smoke rc=$rc}"
        return
    fi

    # Font tooling unit tests (scripts/font_tools.py: pinned venv + font fetch).
    echo "=== python3 -m unittest scripts/tests/test_font_tools.py" >>"$log"
    if ! python3 -m unittest "$REPO_ROOT/scripts/tests/test_font_tools.py" >>"$log" 2>&1; then
        record web FAIL "font_tools unit tests — see log"
        return
    fi

    # Glyph coverage of the bundled Cubic 11 + Silkscreen (spec 4.4 / 5.6). The pinned
    # venv is fetched on first use; $DSM_FONT_PY overrides it.
    local font_py font
    echo "=== font tooling (scripts/font_tools.py python)" >>"$log"
    font_py="${DSM_FONT_PY:-$(python3 "$REPO_ROOT/scripts/font_tools.py" python 2>>"$log")}"
    if [ -n "$font_py" ] && [ -x "$font_py" ] && "$font_py" -c 'import fontTools, brotli' 2>/dev/null; then
        echo "=== font-coverage ($font_py)" >>"$log"
        if "$font_py" "$REPO_ROOT/scripts/verify/font-coverage.py" >>"$log" 2>&1; then
            font="font ok"
        else
            record web FAIL "font coverage: $(grep -E '^(missing|drawn|cubic11|silkscreen)' "$log" | tail -1)"
            return
        fi
    else
        font="font SKIP (could not set up the pinned venv, offline? — see log)"
    fi
    record web PASS "units ${units}; ${summary:-ok}; ${font}"
}

# ─── Driver ──────────────────────────────────────────────────────────────────

run_layer() {
    local layer="$1" start
    start=$SECONDS
    echo "[verify] $layer ..."
    case "$layer" in
        linux)         layer_linux ;;
        windows-tests) layer_windows_tests ;;
        gui-smoke)     layer_gui_smoke ;;
        web)           layer_web ;;
    esac
    DURATION[$layer]=$((SECONDS - start))
    echo "[verify] $layer: ${STATUS[$layer]} — ${DETAIL[$layer]}"
}

print_summary() {
    local layer any_fail=0
    echo
    echo "== PadToKBM verify summary ($(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo '?'))"
    printf '%-14s %-5s %6s  %s\n' LAYER RESULT TIME DETAIL
    for layer in "${ALL_LAYERS[@]}"; do
        printf '%-14s %-5s %5ss  %s\n' "$layer" "${STATUS[$layer]}" \
            "${DURATION[$layer]:-0}" "${DETAIL[$layer]}"
    done
    for layer in "${ALL_LAYERS[@]}"; do
        if [ "${STATUS[$layer]}" = FAIL ]; then
            any_fail=1
            echo "  log ($layer): $(log_path "$layer")"
        fi
    done
    return "$any_fail"
}

on_signal() {
    gui_cleanup
    exit 130
}

main() {
    local layer only="" skip_reason=""
    for layer in "${ALL_LAYERS[@]}"; do
        SELECTED[$layer]=1
    done

    while [ $# -gt 0 ]; do
        case "$1" in
            --quick)
                SELECTED[gui-smoke]=0
                SELECTED[web]=0
                skip_reason="--quick"
                ;;
            --skip-gui) SELECTED[gui-smoke]=0 ;;
            --skip-web) SELECTED[web]=0 ;;
            --only)
                only="${2:-}"
                if [ -z "$only" ] || [ -z "${SELECTED[$only]+set}" ]; then
                    echo "unknown layer '$only' (layers: ${ALL_LAYERS[*]})" >&2
                    exit 2
                fi
                shift
                ;;
            -h|--help) usage; exit 0 ;;
            *) echo "unknown option $1" >&2; usage >&2; exit 2 ;;
        esac
        shift
    done
    if [ -n "$only" ]; then
        for layer in "${ALL_LAYERS[@]}"; do
            SELECTED[$layer]=0
        done
        SELECTED[$only]=1
        skip_reason="--only $only"
    fi

    mkdir -p "$VERIFY_DIR"
    trap gui_cleanup EXIT
    trap on_signal INT TERM

    for layer in "${ALL_LAYERS[@]}"; do
        if [ "${SELECTED[$layer]}" = 1 ]; then
            run_layer "$layer"
        else
            record "$layer" SKIP "not selected (${skip_reason:-flag})"
        fi
    done

    print_summary
}

main "$@"
