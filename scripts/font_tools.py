#!/usr/bin/env python3
"""Fetch the pinned font tooling: a venv with fontTools + brotli, Cubic 11 and Silkscreen.

Everything the font scripts need is pinned here and in
scripts/tools/requirements.txt, so a fresh machine gets the same bytes:
the venv and the font are fetched on first use into
${XDG_CACHE_HOME:-~/.cache}/pad-to-kbm-tools/ and reused after that.

    python3 scripts/font_tools.py python   # prints the venv's python
    python3 scripts/font_tools.py font     # prints the source font path
    python3 scripts/font_tools.py silk     # prints the Silkscreen source path

scripts/build-font.py and scripts/verify/font-coverage.py call
reexec_in_venv() themselves; run them with plain python3.
"""
import hashlib, os, pathlib, subprocess, sys, urllib.request

ROOT = pathlib.Path(__file__).resolve().parents[1]
REQUIREMENTS = ROOT / "scripts" / "tools" / "requirements.txt"

# Cubic 11 v1.500 (OFL; licence shipped as rust/web/fonts/OFL.txt).
FONT_VERSION = "v1.500"
FONT_URL = f"https://raw.githubusercontent.com/ACh-K/Cubic-11/{FONT_VERSION}/fonts/ttf/Cubic_11.ttf"
FONT_SHA256 = "0193f5f033612496df6b45ee92ac3b335bc6a5a24ff95da55ca87b33e57dcf62"

# Silkscreen (OFL; licence shipped as rust/web/fonts/OFL-Silkscreen.txt): the 8 px
# pixel face of the keyboard keys and the title bar. Pinned google/fonts commit.
SILK_COMMIT = "7e8d490955e9d2629cd3095e8dadd6fafe3c16b8"
SILK_URL = f"https://raw.githubusercontent.com/google/fonts/{SILK_COMMIT}/ofl/silkscreen/Silkscreen-Regular.ttf"
SILK_SHA256 = "c845473330b94c2079ce9af01c51ac8ba2d99c24f4d14c039843bbb8e642ebd8"


def tools_dir() -> pathlib.Path:
    base = os.environ.get("XDG_CACHE_HOME") or os.path.expanduser("~/.cache")
    return pathlib.Path(base) / "pad-to-kbm-tools"


def font_path() -> pathlib.Path:
    return tools_dir() / "fonts" / f"Cubic_11-{FONT_VERSION}.ttf"


def silk_path() -> pathlib.Path:
    return tools_dir() / "fonts" / f"Silkscreen-{SILK_COMMIT[:7]}.ttf"


def _sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def ensure_font(url: str = FONT_URL, sha256: str = FONT_SHA256, target: pathlib.Path = None) -> pathlib.Path:
    target = target or font_path()
    if target.exists() and _sha256(target) == sha256:
        return target
    target.parent.mkdir(parents=True, exist_ok=True)
    part = target.with_name(target.name + ".part")
    try:
        with urllib.request.urlopen(url, timeout=60) as r:
            part.write_bytes(r.read())
        got = _sha256(part)
        if got != sha256:
            raise RuntimeError(f"sha256 mismatch for {url}: got {got}, want {sha256}")
        os.replace(part, target)
    finally:
        part.unlink(missing_ok=True)
    return target


def ensure_silk() -> pathlib.Path:
    return ensure_font(SILK_URL, SILK_SHA256, silk_path())


def ensure_python() -> pathlib.Path:
    venv = tools_dir() / "venv"
    py = venv / "bin" / "python"
    stamp = venv / ".requirements.sha256"
    want = _sha256(REQUIREMENTS)
    if py.exists() and stamp.exists() and stamp.read_text().strip() == want:
        return py
    if not py.exists():
        subprocess.run([sys.executable, "-m", "venv", str(venv)], check=True, stdout=sys.stderr)
    subprocess.run([str(py), "-m", "pip", "install", "--quiet", "-r", str(REQUIREMENTS)],
                   check=True, stdout=sys.stderr)
    stamp.write_text(want + "\n")
    return py


def reexec_in_venv(pinned: bool = True) -> None:
    """Re-run the calling script under the pinned venv unless it already runs there.

    pinned=True (build-font): any other interpreter is left, since another fontTools
    version could subset differently. pinned=False (read-only checks): an interpreter
    that already imports fontTools + brotli is kept.
    """
    venv = tools_dir() / "venv"
    if pathlib.Path(sys.prefix).resolve() == venv.resolve():
        return
    if not pinned:
        try:
            import fontTools, brotli  # noqa: F401
            return
        except ImportError:
            pass
    py = ensure_python()
    os.execv(str(py), [str(py), sys.argv[0], *sys.argv[1:]])


def main(argv) -> int:
    if argv == ["python"]:
        print(ensure_python())
    elif argv == ["font"]:
        print(ensure_font())
    elif argv == ["silk"]:
        print(ensure_silk())
    else:
        print(__doc__.strip(), file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
