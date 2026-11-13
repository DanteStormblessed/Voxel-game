#!/usr/bin/env python3
"""Convert a KURAVOXEL_SCREENSHOT capture (binary PPM) to PNG.

usage: ppm2png.py input.ppm output.png [x,y,width,height]
"""
import struct
import sys
import zlib

src, dst = sys.argv[1], sys.argv[2]
data = open(src, "rb").read()
parts = data.split(b"\n", 3)
width, height = map(int, parts[1].split())
pixels = parts[3]
x0, y0, cw, ch = [int(v) for v in sys.argv[3].split(",")] if len(sys.argv) > 3 else [0, 0, width, height]
rows = b"".join(b"\x00" + pixels[(y * width + x0) * 3:(y * width + x0 + cw) * 3] for y in range(y0, y0 + ch))


def chunk(tag, body):
    return struct.pack(">I", len(body)) + tag + body + struct.pack(">I", zlib.crc32(tag + body))


open(dst, "wb").write(
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", cw, ch, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(rows, 6))
    + chunk(b"IEND", b"")
)
