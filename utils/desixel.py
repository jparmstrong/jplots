#!/usr/bin/env python3
"""Decode a captured sixel stream back into images.

    q examples/demo.q -q > out.esc && utils/desixel.py out.esc frame

The counterpart to `decode.py`, which does the same for the kitty protocol. Rendering is the
one thing assertions cannot check, and a re-render is not evidence: this reverses the actual
escape sequence a terminal was sent, so what it writes is what the terminal received.

Written from the format rather than from the encoder, on purpose. A decoder that shares the
encoder's reading of the spec agrees with it about everything, including its mistakes.
"""
import re
import struct
import sys
import zlib

# ESC P <params> q <body> ESC \
DCS = re.compile(rb"\x1bP[0-9;]*q(.*?)\x1b\\", re.S)


def frames(data: bytes):
    """Every sixel image in the stream, as (width, height, rgb_bytes)."""
    # tmux passthrough, if the stream was captured through it.
    data = re.sub(rb"\x1bPtmux;", b"", data).replace(b"\x1b\x1b", b"\x1b")
    for m in DCS.finditer(data):
        yield decode(m.group(1))


def decode(body: bytes):
    palette = {}
    px = {}                       # (x, y) -> colour index
    x = y0 = 0                    # cursor, y0 being the top row of the current band
    raster = None
    i, n = 0, len(body)
    while i < n:
        c = body[i:i + 1]
        if c == b'"':             # raster attributes: "Pan;Pad;Ph;Pv
            j = i + 1
            while j < n and body[j:j + 1] in b"0123456789;":
                j += 1
            parts = body[i + 1:j].split(b";")
            if len(parts) >= 4:
                raster = (int(parts[2]), int(parts[3]))
            i = j
        elif c == b"#":           # colour: #n  (select) or #n;2;r;g;b  (define)
            j = i + 1
            while j < n and body[j:j + 1] in b"0123456789;":
                j += 1
            parts = body[i + 1:j].split(b";")
            cur = int(parts[0])
            if len(parts) >= 5 and parts[1] == b"2":
                # Components are percentages, 0..100.
                palette[cur] = tuple(
                    min(255, round(int(v) * 255 / 100)) for v in parts[2:5])
            x = 0
            i = j
        elif c == b"$":           # carriage return: back to the start of this band
            x = 0
            i += 1
        elif c == b"-":           # newline: next band
            y0 += 6
            x = 0
            i += 1
        elif c == b"!":           # run: !<count><sixel>
            j = i + 1
            while j < n and body[j:j + 1].isdigit():
                j += 1
            count = int(body[i + 1:j])
            mask = body[j] - 0x3F
            for _ in range(count):
                plot(px, x, y0, mask, cur)
                x += 1
            i = j + 1
        elif 0x3F <= body[i] <= 0x7E:
            plot(px, x, y0, body[i] - 0x3F, cur)
            x += 1
            i += 1
        else:                     # whitespace between bands, and anything unrecognised
            i += 1

    if raster:
        w, h = raster
    elif px:
        w = max(k[0] for k in px) + 1
        h = max(k[1] for k in px) + 1
    else:
        return 0, 0, b""
    # Index 0 is the background wherever nothing was plotted, which is what a terminal shows.
    bg = palette.get(0, (0, 0, 0))
    out = bytearray()
    for yy in range(h):
        for xx in range(w):
            out += bytes(palette.get(px.get((xx, yy)), bg) if (xx, yy) in px else bg)
    return w, h, bytes(out)


def plot(px, x, y0, mask, colour):
    for bit in range(6):
        if mask & (1 << bit):
            px[(x, y0 + bit)] = colour


def write_png(rgb: bytes, w: int, h: int, path: str) -> None:
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
        if not w:
            continue
        write_png(rgb, w, h, f"{stem}{i}.png")
        print(f"{stem}{i}.png  {w}x{h}")
        n += 1
    if not n:
        print("no sixel images found in the stream", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
