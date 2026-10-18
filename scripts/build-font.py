#!/usr/bin/env python3
"""Build the bundled web fonts from the pinned sources (scripts/font_tools.py).

rust/web/fonts/cubic11.woff2 is the whole of Cubic 11 (OFL): the UI text and
whatever the user types as a macro name are drawn in the same pixel face
(v3.3.0; it used to be a subset of the UI text, so macro names fell back to
the system font). rust/web/fonts/silkscreen.woff2 is Silkscreen (OFL), the
8 px face of the keyboard keys and the title bar.

Run `python3 scripts/build-font.py`; it re-runs itself in the pinned venv
(scripts/font_tools.py). scripts/verify/font-coverage.py checks the result.
"""
import pathlib, sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
WEB = ROOT / "rust" / "web"
OUT = WEB / "fonts" / "cubic11.woff2"
SILK_OUT = WEB / "fonts" / "silkscreen.woff2"
sys.path.insert(0, str(ROOT / "scripts"))
import font_tools  # noqa: E402


def to_woff2(src, out):
    from fontTools.ttLib import TTFont
    # Keep the source head.modified: a rebuild is byte-identical.
    font = TTFont(src, recalcTimestamp=False)
    font.flavor = "woff2"
    out.parent.mkdir(parents=True, exist_ok=True)
    font.save(out)
    print(f"{out.relative_to(ROOT)}: {out.stat().st_size} bytes, {len(font.getBestCmap())} characters")


def main():
    to_woff2(font_tools.ensure_font(), OUT)
    to_woff2(font_tools.ensure_silk(), SILK_OUT)
    return 0


if __name__ == "__main__":
    font_tools.reexec_in_venv()
    sys.exit(main())
