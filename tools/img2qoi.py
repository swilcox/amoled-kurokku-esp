#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = [
#     "Pillow>=10",
#     "pillow-heif>=0.16",
# ]
# ///
"""Convert images to 480x480 QOI for the AMOLED Kurokku display.

Each image is scaled to cover the square screen and center-cropped (use
--fit to letterbox on black instead), then written as QOI — the format the
firmware streams from HTTP or the SD card.

Usage:
    uv run tools/img2qoi.py photo.jpg [more.png ...] [-o OUTDIR] [--fit]
    # e.g. fill a card:  uv run tools/img2qoi.py ~/Pictures/*.jpg -o /Volumes/SD/images

Output names keep the input stem: photo.jpg -> OUTDIR/photo.qoi.
"""

from __future__ import annotations

import argparse
import struct
import sys
from pathlib import Path

from PIL import Image, ImageOps

try:  # HEIC/HEIF (iPhone photos), if available
    import pillow_heif

    pillow_heif.register_heif_opener()
except ImportError:
    pass

SIZE = 480


def qoi_encode(img: Image.Image) -> bytes:
    """Minimal QOI encoder (RGB, 3 channels). Spec: qoiformat.org."""
    w, h = img.size
    px = img.convert("RGB").tobytes()
    out = bytearray(b"qoif" + struct.pack(">IIBB", w, h, 3, 0))
    index = [(0, 0, 0, 0)] * 64
    prev = (0, 0, 0, 255)
    run = 0
    n = w * h
    for i in range(n):
        cur = (px[3 * i], px[3 * i + 1], px[3 * i + 2], 255)
        if cur == prev:
            run += 1
            if run == 62 or i == n - 1:
                out.append(0xC0 | (run - 1))
                run = 0
            continue
        if run:
            out.append(0xC0 | (run - 1))
            run = 0
        h_ = (cur[0] * 3 + cur[1] * 5 + cur[2] * 7 + cur[3] * 11) % 64
        if index[h_] == cur:
            out.append(h_)
        else:
            index[h_] = cur
            dr = (cur[0] - prev[0] + 128) % 256 - 128
            dg = (cur[1] - prev[1] + 128) % 256 - 128
            db = (cur[2] - prev[2] + 128) % 256 - 128
            dr_dg, db_dg = dr - dg, db - dg
            if -2 <= dr <= 1 and -2 <= dg <= 1 and -2 <= db <= 1:
                out.append(0x40 | (dr + 2) << 4 | (dg + 2) << 2 | (db + 2))
            elif -32 <= dg <= 31 and -8 <= dr_dg <= 7 and -8 <= db_dg <= 7:
                out += bytes([0x80 | (dg + 32), (dr_dg + 8) << 4 | (db_dg + 8)])
            else:
                out += bytes([0xFE, *cur[:3]])
        prev = cur
    out += b"\x00" * 7 + b"\x01"
    return bytes(out)


def convert(src: Path, fit: bool) -> Image.Image:
    img = ImageOps.exif_transpose(Image.open(src)).convert("RGB")
    if fit:
        img.thumbnail((SIZE, SIZE), Image.LANCZOS)
        canvas = Image.new("RGB", (SIZE, SIZE))
        canvas.paste(img, ((SIZE - img.width) // 2, (SIZE - img.height) // 2))
        return canvas
    return ImageOps.fit(img, (SIZE, SIZE), Image.LANCZOS)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("images", nargs="+", type=Path)
    ap.add_argument("-o", "--outdir", type=Path, default=Path("."))
    ap.add_argument("--fit", action="store_true", help="letterbox instead of cropping")
    args = ap.parse_args()
    args.outdir.mkdir(parents=True, exist_ok=True)
    for src in args.images:
        dst = args.outdir / (src.stem + ".qoi")
        try:
            data = qoi_encode(convert(src, args.fit))
        except Exception as e:  # keep going through a batch
            print(f"skip {src}: {e}", file=sys.stderr)
            continue
        dst.write_bytes(data)
        print(f"{src} -> {dst} ({len(data) // 1024} KB)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
