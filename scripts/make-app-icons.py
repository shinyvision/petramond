#!/usr/bin/env python3
"""Regenerate the desktop icon set in packaging/icons/ from the pixel-art master.

Every size is a whole-number nearest-neighbour upscale of packaging/icon.png, so
the pixels stay sharp. Needs Pillow. Run it (or `make app-icons`) after editing
the master and commit the output: the builds read these files directly.
"""

from pathlib import Path
import sys

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
MASTER = ROOT / "packaging" / "icon.png"
OUT = ROOT / "packaging" / "icons"

PNG_SIZES = [16, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 32, 48, 64, 128, 256]
ICNS_SIZES = [16, 32, 64, 128, 256, 512, 1024]


def main() -> int:
    master = Image.open(MASTER).convert("RGBA")
    if master.width != master.height:
        print(f"{MASTER} must be square, got {master.size}", file=sys.stderr)
        return 1
    for size in PNG_SIZES:
        if size % master.width:
            print(f"{size}px is not a whole multiple of the {master.width}px master", file=sys.stderr)
            return 1

    OUT.mkdir(parents=True, exist_ok=True)
    scaled = {size: master.resize((size, size), Image.NEAREST) for size in PNG_SIZES}
    for size, image in scaled.items():
        image.save(OUT / f"petramond-{size}.png", optimize=True)

    largest_ico = scaled[ICO_SIZES[-1]]
    largest_ico.save(
        OUT / "petramond.ico",
        sizes=[(s, s) for s in ICO_SIZES],
        append_images=[scaled[s] for s in ICO_SIZES[:-1]],
    )
    largest_icns = scaled[ICNS_SIZES[-1]]
    largest_icns.save(
        OUT / "petramond.icns",
        append_images=[scaled[s] for s in ICNS_SIZES[:-1]],
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
