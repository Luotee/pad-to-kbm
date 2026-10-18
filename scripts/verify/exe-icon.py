#!/usr/bin/env python3
"""exe-icon.py EXE ICO — fail unless every image in ICO is embedded in EXE.

tauri_build does not tell cargo to watch rust/icons/, so a changed icon.ico
can leave the old icon resource in an incrementally rebuilt exe (v3.3.0).
ICO entries are PNGs (scripts/ico.mjs) and the resource keeps them as is.
"""
import struct
import sys


def main(exe_path, ico_path):
    exe = open(exe_path, 'rb').read()
    ico = open(ico_path, 'rb').read()
    count = struct.unpack('<H', ico[4:6])[0]
    missing = []
    for i in range(count):
        w, _, _, _, _, _, size, off = struct.unpack('<BBBBHHII', ico[6 + 16 * i:22 + 16 * i])
        if ico[off:off + size] not in exe:
            missing.append(str(w or 256))
    if missing:
        print('exe-icon: %s lacks the %s px image(s) of %s (stale build?)' % (exe_path, '/'.join(missing), ico_path))
        return 1
    print('exe-icon: ok (%d images)' % count)
    return 0


if __name__ == '__main__':
    sys.exit(main(sys.argv[1], sys.argv[2]))
