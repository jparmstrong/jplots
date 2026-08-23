#!/usr/bin/env python3
"""Decode a captured kitty-graphics stream back into images.

Rendering is the one thing tests cannot check by assertion, so the way to review a change is
to look at what the terminal was actually sent:

    q demo.q -q > out.esc
    utils/decode.py out.esc frame        # -> frame0.png, frame1.png, ...

This reverses the whole wire format (tmux passthrough, chunking, base64, zlib), so what it
writes is exactly the pixels the terminal receives, not a re-render that might differ.
"""
import re
import struct
import sys
import zlib
import base64


def frames(data: bytes):
    """Every image in the stream, as (width, height, rgb_bytes)."""
    # tmux passthrough: strip the DCS wrapper, then un-double the inner escapes.
    data = re.sub(rb"\x1bPtmux;", b"", data).replace(b"\x1b\x1b", b"\x1b")
    out, cur, dims = [], [], None
    for m in re.finditer(rb"\x1b_G([^;]*);([^\x1b]*)\x1b\\", data):
        ctrl, payload = m.group(1).decode(), m.group(2)
        if "a=T" in ctrl:                      # a new image begins
            if cur:
                out.append((*dims, b"".join(cur)))
                cur = []
            kv = dict(p.split("=") for p in ctrl.split(",") if "=" in p)
            dims = (int(kv["s"]), int(kv["v"]), "o" in kv)
        cur.append(payload)
    if cur:
        out.append((*dims, b"".join(cur)))

    for w, h, compressed, b64 in out:
        rgb = base64.b64decode(b64)
        if compressed:
            rgb = zlib.decompress(rgb)
        if len(rgb) != w * h * 3:
            raise ValueError(f"frame is {len(rgb)} bytes, expected {w * h * 3}")
        yield w, h, rgb


def write_png(rgb: bytes, w: int, h: int, path: str) -> None:
    """A minimal PNG writer. No dependency, and the format is trivial for raw RGB."""
    raw = b"".join(b"\x00" + rgb[y * w * 3:(y + 1) * w * 3] for y in range(h))

    def chunk(tag: bytes, body: bytes) -> bytes:
        return (struct.pack(">I", len(body)) + tag + body
                + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF))

    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n"
                + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(raw, 6))
                + chunk(b"IEND", b""))


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    stem = sys.argv[2] if len(sys.argv) > 2 else "frame"
    data = open(sys.argv[1], "rb").read()
    n = 0
    for i, (w, h, rgb) in enumerate(frames(data)):
        write_png(rgb, w, h, f"{stem}{i}.png")
        print(f"{stem}{i}.png  {w}x{h}")
        n += 1
    if not n:
        print("no kitty images found in the stream", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
