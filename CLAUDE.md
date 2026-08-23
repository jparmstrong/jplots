# jplots: working notes

Terminal plots for kdb+/q over the kitty graphics protocol. MIT, public.

## Build and test

| | |
| --- | --- |
| `make` | → `target/release/libjplots.so` |
| `make test` | cargo tests, then `tests/test.q` under kdb+ (76 assertions) |
| `make demo` | `examples/demo.q`, every chart type, in your terminal |
| `make images` | regenerate `docs/img/` from the examples |
| `make lint` | clippy, `-D warnings` |
| `cargo run --release --bin jplots-probe` | kitty protocol test pattern, one variant per wire setting |

Tests need a q. `Q ?= q`; override as `make test Q=~/.kx/bin/q`. The recipes point
`$JPLOTS_LIB` at the freshly built library, so nothing has to be installed. By hand:

```sh
JPLOTS_LIB=$PWD/target/release/libjplots q examples/demo.q -q </dev/null
```

Everything generated goes under `target/`. `docs/img/` is the exception: those are checked in,
so they are source, not artifacts.

## Layout

```
q/plt.q      argument munging -> ONE spec dict          (the public API lives here)
src/kapi.rs  K -> Plot, the only file that knows what a K is
src/layout.rs  ranges, ticks, margins, legend, panels -> device coordinates
src/canvas.rs  the Canvas trait: line / rect / polygon / marker / text
src/raster.rs  RGB pixels
src/kitty.rs   zlib, base64, APC escapes, tmux passthrough
src/sixel.rs   256-colour palette, DCS escapes    (`.plt.renderer` picks between them)
src/font.rs    a 6x13 bitmap table, generated once by utils/mkfont.py
```

**Argument munging stays in q.** `plt.q` reads a table's shape in a few lines that would be
fifty of Rust, and the API can then change without rebuilding the library. Derived quantities
(histogram bins, matrix pairings) are computed in Rust where they depend on the axis range,
except the scatter matrix's regressions, which are computed in q because the caller gets them
back as a table and they should exist once, not twice.

**Everything expensive to get right lives above `Canvas`** so a second backend inherits it and
cannot drift. Two seams are deliberate: `text` is semantic, not a blit, so an SVG backend can
emit a real element; `polygon` is in the trait although 2-D never needs more than `rect`,
because 3-D surfaces are shaded quads.

## q landmines that have actually bitten here

- **A lone `/` on a line opens a block comment to EOF.** No error: `\l` reports success and
  defines nothing. Never leave a bare `/` line in a `.q` file.
- **`and` is `min`, and evaluates both sides.** `$[...;...;...]` when either side can signal.
- **`key` of a keyed table is a TABLE**, not the key column. `(key kt)\`sym` for the labels.
- **`d k` on a key the dict lacks returns a null, and `count` of a null is 1.** Guard with
  `k in key d`, or a missing field silently becomes n nulls.
- **An indented line continues the previous one**, inside a lambda body as well as at the top
  level. `f:raze g[x]` + an indented `each til n` on the next line is one expression in q but
  binds as a projection on some hosts. Keep it on one line, or end statements with `;`.
- **`ratios` yields `x[0]` itself as its first element**, not a ratio. `1_` it.
- **`getenv` returns a CHAR VECTOR, not a symbol.** `string getenv`TERM` splits it into a
  list of one-character strings, and `like` on that is `type.
- **`like` takes ONE interior wildcard.** `"abcde" like "*b*d*"` signals `nyi in kdb+, so a
  test cannot assert two fragments of a message in one pattern.
- **`` `name!entry `` is an atom keyed to a list** and signals. A one-entry dict of dicts is
  `([name: entry])`, which is also what reads best.

## Rendering rules

- **The bitmap font is ASCII 32-126.** A long dash or an accent in a title renders as a blank
  gap. Keep example titles ASCII, because they end up in `docs/img/`.
- **Examples must be deterministic**: no `rand`, no clock. `docs/img/` is regenerated from
  them, and `utils/gallery.py` pins which frame is which by index, so a chart added or removed
  fails the run rather than quietly renaming the set.
- **A transmitted image lives in the terminal until something frees it.** Every plot used to
  leave one behind for the session; `kitty::KEEP` bounds the live set at 32 charts. A bound,
  not a tuned budget: a tighter one was once fitted to a terminal I wrongly believed was
  refusing images, and it cost scrollback for nothing. The id base is per-PROCESS: a fixed one
  makes two sessions in one terminal overwrite each other's charts, and a replaced image takes
  its earlier placement with it. Any new transmission path has to go through `encode_with`.
- **Reserve the rows before drawing, never after.** Both renderers print the image's height in
  newlines, move back up, draw, and come down. Drawing first and scrolling after asks the
  terminal to put pixels below its own last line, which is where behaviour diverges: some
  scroll, some clip, and a clipped chart reads as a corrupt one.
- **"Charts stopped appearing" is usually the WINDOW.** A 900x420 chart is fourteen rows. Ten
  of them need 140. Check the height before diagnosing a protocol.
- **Snap axis-aligned strokes and rects to whole pixels**, or thin lines come out fuzzy.
- **Derive adjacent edges from an index, once.** Computing a bar's right edge and its
  neighbour's left edge by two float paths loses the 1px separator at some widths.
- **A terminal that will not draw a plot is diagnosed with `jplots-probe`**, not by guessing:
  it varies compression, chunking and the `c`/`r` cell box one at a time through
  `kitty::encode_with`, the shipped encoder, so the frames that render name the culprit.
- `utils/decode.py` reverses the whole wire format back to PNG, so review a rendering change
  from a real q run, not from a re-render that might differ.

## Adding a chart kind

`Kind` + `Kind::from_name` in `src/lib.rs`; a draw arm in `layout.rs`; whatever spec fields it
needs in `kapi.rs`; the entry point in `q/plt.q`; assertions in `tests/test.q`; a frame in the
README and `utils/gallery.py`; then `make images`. A kind added to the renderer and missed by a
host bridge is the failure mode this ordering exists to prevent.

## Terminals

`` `sixel `` is the default renderer and reaches the most terminals. kitty and ghostty do not
implement sixel and need `.plt.renderer:`kitty`. **WezTerm is not supported**: unreliable under
both, and not worth chasing.

**Where a sixel image leaves the cursor is not in the protocol** and terminals disagree by a
whole image height, so `.plt.sixel_cursor` selects it and `jplots-probe --cursor` is how to find
the right value. Do not infer it from symptoms: I got it wrong three times before drawing all
four strategies and looking at them.

**tmux transports kitty escapes fine with `allow-passthrough all` but does not keep the image**,
repainting panes from a grid that has no record of one. It understands sixel, which is one more
reason that is the default.

## jplots-mail

A Unix filter: a captured session in, an RFC822 message out. `q report.q -q kitty |
jplots-mail | sendmail -t`. Text becomes `<pre>`, plots become CID PNG parts, `EMAIL-*` lines
become headers.

- **Un-tmux BEFORE splitting.** tmux doubles every ESC inside its wrapper, so scanning for the
  first `ESC \` finds the doubled terminator inside the payload and halves the image. This cost
  an evening: kitty captures decoded to nothing while sixel ones worked.
- **A kitty image is several escapes** (4096-byte chunks). Split per escape and you get one
  part per chunk and decode none of them.
- **Mark the image `src` safe in the template, and nothing else.** Autoescape turns `image/png`
  into `image&#x2f;png` and mangles every `/` in base64, which breaks a data URI while leaving
  CID working: a bug that ships. Text runs must stay escaped.
- The template engine is behind the `mail` feature so the library keeps its three dependencies.

## Dependencies

`base64`, `flate2`, `libc`. That is the whole list and it should stay that way: pixels go out
as zlib-compressed RGB, which the protocol takes directly, so there is no PNG encoder, and the
font is a generated table, so there is no font stack.
