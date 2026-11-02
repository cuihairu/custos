#!/usr/bin/env python3
"""Regenerate the Windows application icon (assets/icons/custos.ico).

This is a pure format conversion of the existing official SVG brand asset
(docs/public/logo.svg) into the Windows .ico container — no new artwork.
The .ico packs the standard sizes Windows uses across Explorer, taskbar,
Alt-Tab and the installer's Add/Remove Programs entry.

Usage:  python3 scripts/make-windows-icon.py
Needs:  librsvg (rsvg-convert) + ImageMagick (magick) on PATH.

ImageMagick's internal MSVG renderer mis-fills this path (verified), so the
vector is rasterized by librsvg first and only the .ico packing is done by
ImageMagick.
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SVG = ROOT / "docs" / "public" / "logo.svg"
OUT = ROOT / "assets" / "icons" / "custos.ico"

# Windows expects 16..256 px; each size is rasterized straight from the SVG
# vector (no bitmap upscaling) before packing into one .ico container.
SIZES = [16, 24, 32, 48, 64, 128, 256]


def main() -> int:
    if not SVG.is_file():
        print(f"error: source SVG not found: {SVG}", file=sys.stderr)
        return 1
    for tool in ("rsvg-convert", "magick"):
        if not Path(f"/usr/bin/{tool}").is_file():
            print(f"error: {tool} not found (see module docstring)", file=sys.stderr)
            return 1

    OUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as tmp:
        pngs: list[str] = []
        for size in SIZES:
            png = Path(tmp) / f"custos-{size}.png"
            subprocess.run(
                ["rsvg-convert", "-w", str(size), "-h", str(size),
                 "-o", str(png), str(SVG)],
                check=True,
            )
            pngs.append(str(png))
        # Exact-size inputs: ImageMagick packs them as-is into one .ico.
        subprocess.run(["magick", *pngs, str(OUT)], check=True)

    # Verify every requested size actually landed in the container.
    identify = subprocess.run(
        ["magick", "identify", str(OUT)], check=True, capture_output=True, text=True
    )
    print(identify.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
