#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "Pillow>=10",
# ]
# ///
"""Convert pictures into "stations" for the static animation.

Each picture is turned grey, cover-cropped to a square (kept towards the
top, where faces usually are), contrast-stretched and scaled to 160x160:
one pixel per 3-px grain of the static. Written as 8-bit binary PGM, which
build.rs packs into the asset pack.

Usage:
    uv run tools/tv2pgm.py photo.jpg [more.jpg ...] [-o assets/tv] [--center]

Output names keep the input stem: photo.jpg -> OUTDIR/photo.pgm.
"""

import argparse
from pathlib import Path

from PIL import Image, ImageOps

SIZE = 160


def convert(src: Path, dst: Path, centering: tuple[float, float]) -> None:
    im = ImageOps.exif_transpose(Image.open(src)).convert("L")
    im = ImageOps.fit(im, (SIZE, SIZE), Image.Resampling.LANCZOS, centering=centering)
    im = ImageOps.autocontrast(im, cutoff=1)
    dst.write_bytes(f"P5\n{SIZE} {SIZE}\n255\n".encode() + im.tobytes())


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("images", nargs="+", type=Path)
    ap.add_argument("-o", "--out", type=Path, default=Path("assets/tv"))
    ap.add_argument("--center", action="store_true", help="crop from the middle instead of near the top")
    args = ap.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    centering = (0.5, 0.5) if args.center else (0.5, 0.3)
    for src in args.images:
        dst = args.out / f"{src.stem}.pgm"
        convert(src, dst, centering)
        print(f"{src} -> {dst}")


if __name__ == "__main__":
    main()
