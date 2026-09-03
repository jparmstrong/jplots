#!/usr/bin/env python3
"""Render the examples and save the README's images.

    utils/gallery.py ~/.kx/bin/q        # -> docs/img/*.png

The images in the README are not screenshots of a terminal. They are decoded straight from
the escape stream the examples emit, so they are exactly the pixels a terminal would be sent.
That also means they cannot drift from the renderer: `make images` regenerates them, and a
change that alters a chart shows up as a changed PNG in the diff.

The source is `examples/` rather than a private corpus, so every image is one a reader can
reproduce by running the example. The index-to-name maps below pin WHICH chart is which; if
an example gains or loses a chart this fails loudly rather than silently renaming the set.
"""
import os
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from decode import frames, write_png  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "docs", "img")

# script -> {frame index: image name}. Indices not listed are rendered and discarded. The
# examples exist to be read, not to be exactly the gallery.
WANT = {
    "examples/demo.q": {
        1: "line",
        2: "bar",
        4: "bar-grouped",
        5: "hbar",
        6: "hist",
        7: "scatter",
        8: "scatter-fit",
        9: "matrix",
        10: "clusters",
        11: "bands",
    },
    "examples/candlestick.q": {
        0: "candle",
    },
}


def render(host: str, script: str) -> list:
    env = dict(os.environ,
               JPLOTS_LIB=os.path.join(ROOT, "target", "release", "libjplots"))
    # `kitty` explicitly: the demo defaults to sixel now, and these images are decoded from
    # the kitty stream. The pixels are identical either way, so this is about which decoder
    # reads them, not about what the charts look like.
    proc = subprocess.run(
        [host, script, "-q", "kitty"],
        cwd=ROOT, env=env, stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=180,
    )
    if proc.returncode != 0:
        raise SystemExit(f"{script}: exit {proc.returncode}\n{proc.stderr.decode()[:800]}")
    got = list(frames(proc.stdout))
    if not got:
        raise SystemExit(f"{script}: produced no images\n{proc.stderr.decode()[:800]}")
    return got


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    host = sys.argv[1]
    os.makedirs(OUT, exist_ok=True)
    for script, want in WANT.items():
        got = render(host, script)
        if max(want) >= len(got):
            raise SystemExit(
                f"{script} drew {len(got)} charts; the map expects at least {max(want) + 1}. "
                "Update WANT in utils/gallery.py.")
        for i, name in sorted(want.items()):
            w, h, rgb = got[i]
            path = os.path.join(OUT, f"{name}.png")
            write_png(rgb, w, h, path)
            print(f"docs/img/{name}.png  {w}x{h}  {os.path.getsize(path) // 1024}k")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
