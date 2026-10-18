#!/usr/bin/env python3
"""Glyph-coverage check for the bundled fonts (spec 4.4 / 5.6).

Every non-ASCII, non-whitespace character in the shipped web text
(rust/web/**/*.html|js|css, tests excluded) must be in
rust/web/fonts/cubic11.woff2, and so must every Big5 level-1 character:
macro names are typed by the user, so the whole common set ships (v3.3.0).
The keyboard labels and the title bar use Silkscreen
(rust/web/fonts/silkscreen.woff2), which must cover printable ASCII. The characters the font lacks and the UI
draws as SVG (✕ ◀ ▶ ↖ ↗ ↙ ↘) must not appear in that text at all.

Needs fontTools + brotli: an interpreter that has them is used as is, any other
re-runs this in the pinned venv (scripts/font_tools.py). Exit 0 = ok, 1 = missing.
"""
import pathlib, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import font_tools  # noqa: E402
WEB = ROOT / "rust" / "web"
FONT = WEB / "fonts" / "cubic11.woff2"
SILK = WEB / "fonts" / "silkscreen.woff2"


def big5_level1():
    """The 5401 common hanzi of Big5 (0xA440-0xC67E) as code points."""
    out = set()
    for hi in range(0xA4, 0xC7):
        for lo in list(range(0x40, 0x7F)) + list(range(0xA1, 0xFF)):
            try:
                out.add(ord(bytes([hi, lo]).decode("big5")))
            except UnicodeDecodeError:
                pass
    return out
SVG_ONLY = "✕◀▶↖↗↙↘"


def web_text():
    out = {}
    for p in sorted(WEB.rglob("*")):
        if p.suffix not in (".html", ".js", ".css") or "tests" in p.relative_to(WEB).parts:
            continue
        out[p.relative_to(ROOT)] = p.read_text(encoding="utf-8")
    return out


def main():
    from fontTools.ttLib import TTFont
    if not FONT.exists():
        print(f"no {FONT.relative_to(ROOT)}: run scripts/build-font.py")
        return 1
    cmap = TTFont(FONT).getBestCmap()
    missing, banned = {}, {}
    for path, text in web_text().items():
        for ch in set(text):
            if ord(ch) < 0x80 or ch.isspace():
                continue
            if ch in SVG_ONLY:
                banned.setdefault(ch, path)
            elif ord(ch) not in cmap:
                missing.setdefault(ch, path)
    if banned:
        print("drawn as SVG, must not be typed: " + " ".join(f"{c} ({p})" for c, p in sorted(banned.items())))
    if missing:
        print("missing: " + " ".join(f"{c} U+{ord(c):04X} ({p})" for c, p in sorted(missing.items())))
    common = big5_level1() - set(cmap)
    if common:
        print(f"cubic11.woff2 lacks {len(common)} Big5 common characters (e.g. {chr(min(common))}): run scripts/build-font.py")
    silk_missing = []
    if not SILK.exists():
        silk_missing = ["(no file)"]
    else:
        scmap = TTFont(SILK).getBestCmap()
        silk_missing = [chr(c) for c in range(0x21, 0x7F) if c not in scmap]
    if silk_missing:
        print("silkscreen.woff2 missing: " + " ".join(silk_missing))
    if banned or missing or common or silk_missing:
        return 1
    print(f"font-coverage: ok ({len(cmap)} glyphs)")
    return 0


if __name__ == "__main__":
    font_tools.reexec_in_venv(pinned=False)
    sys.exit(main())
