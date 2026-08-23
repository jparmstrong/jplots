//! RGB pixels to sixel (DEC DCS) escape sequences: the second raster backend.
//!
//! Sixel is a palette format, six pixel rows to a band, one pass per colour per band. It is
//! older and cruder than the kitty protocol and it buys two things that protocol cannot:
//! terminals that will not speak kitty (WezTerm, xterm, foot, mlterm, Konsole, Contour), and
//! **tmux**. tmux understands a sixel image and holds it in its own screen model, so it can
//! repaint it; a kitty image passes through tmux untouched and is erased by the first repaint,
//! because tmux has no record that one is there.
//!
//! Nothing above this file changes. Sixel is raster, so it reads the same [`crate::Raster`]
//! output the kitty encoder does, and `layout` never learns that a second backend exists.
//!
//! **The palette is the whole difficulty.** Sixel allows 256 colours and charts are drawn in
//! truecolour. Measured on the nine README charts: bar, histogram and candlestick charts use
//! five to seven colours in total, and the busiest (a 4x4 scatter matrix) uses 1883, of which
//! the 256 most frequent cover 99.2% of pixels. The excess is all antialiasing fringe, so the
//! palette is simply the most frequent 256 colours and everything else takes the nearest one.
//! No median cut, no dithering: at that coverage they would be solving a problem the data
//! does not have.

use crate::canvas::Rgb;
use crate::kitty::Metrics;
use std::collections::HashMap;
use std::fmt::Write as _;

/// Sixel's palette limit. Some terminals accept more; none accept fewer.
const MAX_COLOURS: usize = 256;

/// A run shorter than this costs more as `!<count><char>` than written out.
const MIN_RUN: u32 = 4;

/// The palette, and one index per pixel.
///
/// Split from [`encode`] so the palette can be tested on its own: how well it covers an image
/// is the property that decides whether sixel output is worth looking at, and it is a
/// question about the data rather than about the escape sequence.
pub fn quantize(rgb: &[u8], w: u32, h: u32) -> (Vec<Rgb>, Vec<u8>) {
    let n = (w as usize * h as usize).min(rgb.len() / 3);
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    for i in 0..n {
        *counts.entry([rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]]).or_insert(0) += 1;
    }

    let mut by_count: Vec<([u8; 3], usize)> = counts.into_iter().collect();
    // Sort by frequency, then by value: a HashMap iterates in an arbitrary order, and an
    // arbitrary order would make the same chart encode differently from run to run.
    by_count.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    by_count.truncate(MAX_COLOURS);
    let palette: Vec<Rgb> = by_count.iter().map(|(c, _)| Rgb(c[0], c[1], c[2])).collect();

    // Colours outside the palette take the nearest one. Cached: the tail is antialiasing
    // fringe, so the same handful of blends recur along every edge in the image.
    let mut map: HashMap<[u8; 3], u8> = by_count
        .iter()
        .enumerate()
        .map(|(i, (c, _))| (*c, i as u8))
        .collect();
    let mut idx = vec![0u8; n];
    for (i, slot) in idx.iter_mut().enumerate() {
        let c = [rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]];
        *slot = *map.entry(c).or_insert_with(|| {
            palette
                .iter()
                .enumerate()
                .min_by_key(|(_, p)| {
                    let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
                    d(p.0, c[0]) + d(p.1, c[1]) + d(p.2, c[2])
                })
                .map_or(0, |(i, _)| i as u8)
        });
    }
    (palette, idx)
}

/// The full byte stream that draws `rgb` (`w` by `h`, 3 bytes per pixel) at the cursor and
/// leaves the cursor on the line below the image.
///
/// Unlike the kitty protocol there is no image id and nothing to free later: a sixel image
/// becomes part of the terminal's screen, so it scrolls away like text and needs no
/// bookkeeping from us.
///
/// Where a terminal leaves the cursor after a sixel image is not agreed on, and a terminal
/// that leaves it inside the image draws the next one on top of the last: several plots in a
/// row then show as one or two. So the position is never inferred. The rows are reserved
/// first, which is what makes any scrolling happen BEFORE the image and therefore makes a
/// saved cursor position still valid; then the cursor goes back up, the image is drawn, the
/// saved position is restored, and it moves down by exactly the height. Whatever the terminal
/// did with the cursor in between is discarded.
pub fn encode(rgb: &[u8], w: u32, h: u32, m: Metrics) -> Vec<u8> {
    place(rgb, w, h, m, Cursor::default())
}

/// How to get the cursor below a sixel image.
///
/// Terminals disagree, and not by a little: some leave the cursor where the image started and
/// some advance past it themselves, so a sequence that lands correctly on one leaves two
/// screens of blank on the other. There is no query in the protocol that answers this, so it
/// is selectable and [`crate::probe`] draws one frame per strategy for a terminal to be asked
/// the only way that works, which is by looking.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Cursor {
    /// Reserve the rows and draw into them, letting the terminal's own advance stand. The
    /// default, because advancing is what a sixel terminal is supposed to do: with scrolling
    /// enabled the cursor lands at the start of the line below the image. Reserving first is
    /// still worth doing, so an image drawn near the bottom of the screen is not asked to
    /// land below the last line.
    #[default]
    Reserve,
    /// Reserve, draw, then descend by the height ourselves. For a terminal that does NOT
    /// advance, where anything else draws each image over the last. On a terminal that does
    /// advance this triples the gap under every plot, so it is never the default.
    Advance,
    /// Draw and add nothing at all, not even the reserve.
    Bare,
    /// Draw, then one newline.
    Newline,
}

impl Cursor {
    /// Parse `.plt.sixel_cursor`, or the message to signal.
    pub fn from_name(s: &str) -> Result<Cursor, String> {
        match s {
            "" | "reserve" => Ok(Cursor::Reserve),
            "advance" => Ok(Cursor::Advance),
            "bare" => Ok(Cursor::Bare),
            "newline" => Ok(Cursor::Newline),
            other => Err(format!(
                ".plt.sixel_cursor: unknown `{other} (known: `reserve, `advance, `bare, `newline)"
            )),
        }
    }
}

/// [`encode`], with the cursor handling chosen rather than assumed.
pub fn place(rgb: &[u8], w: u32, h: u32, m: Metrics, cursor: Cursor) -> Vec<u8> {
    let rows = (h as f64 / m.cell().1).ceil().max(1.0) as u32;
    let nl = "\n".repeat(rows as usize);
    // Reserving needs the rows to EXIST. `ESC[nA` clamps at the top of the screen, so asking
    // to move up more rows than the terminal has lands the cursor somewhere other than where
    // the reserve began, and the image is drawn from there: a chart taller than the window
    // came out with two screens of blank above it. An image that does not fit will scroll
    // whatever we do, so the least bad thing is to add nothing and let the terminal handle it.
    let fits = m.rows > 0 && rows < m.rows as u32;
    let cursor = if fits { cursor } else { Cursor::Bare };
    let (before, after) = match cursor {
        Cursor::Reserve => (format!("{nl}\x1b[{rows}A"), "\r".to_string()),
        // Reserving first is what keeps the saved position valid: a scroll between save and
        // restore would move the screen out from under it.
        Cursor::Advance => (format!("{nl}\x1b[{rows}A\x1b7"), format!("\x1b8\x1b[{rows}B\r")),
        Cursor::Bare => (String::new(), "\r".to_string()),
        Cursor::Newline => (String::new(), "\r\n".to_string()),
    };
    let mut out = before.into_bytes();
    out.extend(image(rgb, w, h));
    out.extend(after.into_bytes());
    out
}

/// Just the DCS image, with no cursor handling: the part a caller might want to embed.
pub fn image(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let (palette, idx) = quantize(rgb, w, h);
    let (wu, hu) = (w as usize, h as usize);
    let mut out = String::with_capacity(wu * hu / 8 + 1024);

    // DCS, then raster attributes: 1:1 pixel aspect and the true size, so the terminal does
    // not have to infer either. Without them some terminals assume the VT340's 2:1 pixels and
    // draw everything at double height.
    out.push_str("\x1bPq");
    let _ = write!(out, "\"1;1;{w};{h}");

    for (i, c) in palette.iter().enumerate() {
        // Sixel colour components are PERCENTAGES, not 0-255. Rounding rather than truncating:
        // truncation drags every colour towards black, visibly so on a dark background.
        let pc = |v: u8| (v as u32 * 100 + 127) / 255;
        let _ = write!(out, "#{i};2;{};{};{}", pc(c.0), pc(c.1), pc(c.2));
    }

    let ncol = palette.len();
    // One row of six-bit masks per colour, reused for every band. Built in a single pass over
    // the band's pixels, so the cost is the band's area rather than area times colours.
    let mut bits = vec![0u8; ncol * wu];
    for band in 0..hu.div_ceil(6) {
        bits.iter_mut().for_each(|b| *b = 0);
        let y0 = band * 6;
        let mut present = vec![false; ncol];
        for k in 0..6usize {
            let y = y0 + k;
            if y >= hu {
                break;
            }
            for x in 0..wu {
                let ci = idx[y * wu + x] as usize;
                bits[ci * wu + x] |= 1 << k;
                present[ci] = true;
            }
        }

        let mut first = true;
        for ci in (0..ncol).filter(|c| present[*c]) {
            // `$` returns to the start of the band so the next colour overlays it. Only
            // between colours: a leading one costs nothing but a trailing one is a wasted
            // pass on every band of the image.
            if !first {
                out.push('$');
            }
            first = false;
            let _ = write!(out, "#{ci}");
            write_run(&mut out, &bits[ci * wu..ci * wu + wu]);
        }
        out.push('-');
    }
    out.push_str("\x1b\\");
    out.into_bytes()
}

/// One colour's row of six-bit masks, run-length encoded, with the trailing empty span
/// dropped. A sixel character is `?` plus the mask, so an empty span is a run of `?` and
/// there is no reason to send it when the band simply ends there.
fn write_run(out: &mut String, row: &[u8]) {
    let end = row.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    let (mut run, mut len) = (0u8, 0u32);
    for &b in &row[..end] {
        if b == run {
            len += 1;
            continue;
        }
        flush(out, run, len);
        run = b;
        len = 1;
    }
    flush(out, run, len);
}

fn flush(out: &mut String, mask: u8, len: u32) {
    if len == 0 {
        return;
    }
    let ch = (0x3f + mask) as char;
    if len >= MIN_RUN {
        let _ = write!(out, "!{len}{ch}");
    } else {
        for _ in 0..len {
            out.push(ch);
        }
    }
}

// ---------------------------------------------------------------- decoding

/// Every image in a captured sixel stream, as `(width, height, rgb)`.
///
/// Written from the format rather than by inverting [`image`] statement by statement, so a
/// mistake in one is not automatically agreed to by the other. It reads what any sixel source
/// emits, not only ours: a report may be piped from something else entirely.
pub fn decode(stream: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(start) = pos(&stream[i..], b"\x1bP").map(|p| i + p) {
        // DCS parameters, then `q`, then the data.
        let Some(q) = stream[start + 2..]
            .iter()
            .position(|b| *b == b'q')
            .filter(|n| stream[start + 2..start + 2 + n].iter().all(|c| c.is_ascii_digit() || *c == b';'))
        else {
            i = start + 2;
            continue;
        };
        let body = start + 3 + q;
        let end = pos(&stream[body..], b"\x1b\\").map_or(stream.len(), |p| body + p);
        if let Some(img) = one(&stream[body..end]) {
            out.push(img);
        }
        i = end + 2;
    }
    out
}

fn pos(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// One DCS body: raster attributes, colour definitions, and bands of six-pixel columns.
fn one(body: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    let mut palette: Vec<Rgb> = vec![Rgb(0, 0, 0); MAX_COLOURS];
    let mut px: Vec<(u32, u32, u8)> = Vec::new();
    let (mut raster, mut cur, mut x, mut band) = (None, 0usize, 0u32, 0u32);
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            b'"' => {
                let (n, args) = params(&body[i + 1..]);
                // "Pan;Pad;Ph;Pv: the aspect numerator and denominator, then the true size.
                if args.len() >= 4 {
                    raster = Some((args[2], args[3]));
                }
                i += 1 + n;
            }
            b'#' => {
                let (n, args) = params(&body[i + 1..]);
                if let Some(&c) = args.first() {
                    cur = c as usize % MAX_COLOURS;
                    // `#n;2;r;g;b` DEFINES; a bare `#n` only selects. Components are
                    // percentages, which is the one thing everyone gets wrong here.
                    if args.len() >= 5 && args[1] == 2 {
                        let pc = |v: u32| ((v.min(100) * 255 + 50) / 100) as u8;
                        palette[cur] = Rgb(pc(args[2]), pc(args[3]), pc(args[4]));
                    }
                }
                x = 0;
                i += 1 + n;
            }
            b'$' => {
                x = 0;
                i += 1;
            }
            b'-' => {
                band += 1;
                x = 0;
                i += 1;
            }
            b'!' => {
                let (n, args) = params(&body[i + 1..]);
                let count = args.first().copied().unwrap_or(0);
                if let Some(&ch) = body.get(i + 1 + n) {
                    for _ in 0..count {
                        plot(&mut px, x, band, ch, cur as u8);
                        x += 1;
                    }
                }
                i += 2 + n;
            }
            0x3f..=0x7e => {
                plot(&mut px, x, band, body[i], cur as u8);
                x += 1;
                i += 1;
            }
            _ => i += 1, // whitespace between bands, and anything unrecognised
        }
    }

    let (w, h) = raster.or_else(|| {
        let w = px.iter().map(|p| p.0).max()? + 1;
        let h = px.iter().map(|p| p.1).max()? + 1;
        Some((w, h))
    })?;
    if w == 0 || h == 0 {
        return None;
    }
    // Index 0 is the background wherever nothing was plotted, which is what a terminal shows.
    let bg = palette[0];
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for _ in 0..w * h {
        rgb.extend_from_slice(&[bg.0, bg.1, bg.2]);
    }
    for (cx, cy, ci) in px {
        if cx < w && cy < h {
            let c = palette[ci as usize];
            let o = ((cy * w + cx) * 3) as usize;
            rgb[o..o + 3].copy_from_slice(&[c.0, c.1, c.2]);
        }
    }
    Some((w, h, rgb))
}

fn plot(px: &mut Vec<(u32, u32, u8)>, x: u32, band: u32, ch: u8, colour: u8) {
    let mask = ch.wrapping_sub(0x3f);
    for bit in 0..6u32 {
        if mask & (1 << bit) != 0 {
            px.push((x, band * 6 + bit, colour));
        }
    }
}

/// The run of digits and semicolons at `s`, as its length and its numbers.
fn params(s: &[u8]) -> (usize, Vec<u32>) {
    let n = s.iter().take_while(|b| b.is_ascii_digit() || **b == b';').count();
    let args = String::from_utf8_lossy(&s[..n])
        .split(';')
        .map(|p| p.trim().parse().unwrap_or(0))
        .collect();
    (n, args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: Rgb) -> Vec<u8> {
        (0..w * h).flat_map(|_| [c.0, c.1, c.2]).collect()
    }

    /// The shape of the stream: DCS, raster attributes at the true size, palette, bands,
    /// terminator. A terminal that gets any of these wrong draws nothing at all, and there is
    /// no error to see.
    #[test]
    fn the_envelope_is_well_formed() {
        let s = String::from_utf8(image(&solid(12, 13, Rgb(255, 0, 0)), 12, 13)).unwrap();
        assert!(s.starts_with("\x1bPq\"1;1;12;13"), "{}", &s[..24.min(s.len())]);
        assert!(s.ends_with("\x1b\\"));
        assert!(s.contains("#0;2;100;0;0"), "colour is a PERCENTAGE: {s}");
        // 13 rows is three bands: two full and a partial.
        assert_eq!(s.matches('-').count(), 3, "band count: {s}");
    }

    /// Every sixel data byte has to land in the printable range the format allows. A mask of
    /// 0 is `?` and a mask of 63 is `~`; anything outside is a terminal-specific guess.
    #[test]
    fn data_bytes_stay_inside_the_sixel_range() {
        let mut px = Vec::new();
        for y in 0..30u32 {
            for x in 0..40u32 {
                let v = ((x * 7 + y * 13) % 256) as u8;
                px.extend_from_slice(&[v, 255 - v, v / 2]);
            }
        }
        let s = String::from_utf8(image(&px, 40, 30)).unwrap();
        let body = &s[s.find('#').unwrap()..s.len() - 2];
        for ch in body.chars() {
            let ok = ch.is_ascii_digit()
                || "#;$-!\"".contains(ch)
                || ('\u{3f}'..='\u{7e}').contains(&ch);
            assert!(ok, "byte {ch:?} is not legal sixel");
        }
    }

    /// The palette covers what the measurements promised: a flat chart is exact, and a busy
    /// one loses only fringe. This is the property that decides whether the output is worth
    /// looking at, so it is asserted rather than assumed.
    #[test]
    fn the_palette_covers_the_image() {
        // A chart's worth of flat colour: exact, and nowhere near the limit.
        let mut flat = Vec::new();
        for i in 0..4000 {
            let c = [Rgb(16, 18, 24), Rgb(76, 154, 255), Rgb(255, 122, 69)][i % 3];
            flat.extend_from_slice(&[c.0, c.1, c.2]);
        }
        let (pal, idx) = quantize(&flat, 100, 40);
        assert_eq!(pal.len(), 3);
        assert!(idx.iter().enumerate().all(|(i, &s)| {
            let c = &flat[i * 3..i * 3 + 3];
            pal[s as usize] == Rgb(c[0], c[1], c[2])
        }));

        // More colours than sixel allows: the palette caps, and every pixel still maps to a
        // real entry. 512 distinct triples, so the cap is genuinely exercised.
        let mut many = Vec::new();
        for i in 0..30_000u32 {
            let k = i % 512;
            many.extend_from_slice(&[(k % 256) as u8, (k / 256) as u8, 0]);
        }
        let (pal, idx) = quantize(&many, 150, 200);
        assert_eq!(pal.len(), MAX_COLOURS, "the palette must cap at the sixel limit");
        assert_eq!(idx.len(), 30_000);
        assert!(idx.iter().all(|s| (*s as usize) < pal.len()));
        // A colour outside the palette takes the NEAREST entry, not entry 0. Asserted as the
        // argmin rather than as an absolute distance: how far the nearest is depends on which
        // colours the image happens to contain, but that it IS the nearest is the contract.
        let dist = |p: Rgb, c: &[u8]| {
            let d = |a: u8, b: u8| (a as i32 - b as i32).pow(2);
            d(p.0, c[0]) + d(p.1, c[1]) + d(p.2, c[2])
        };
        for i in (0..idx.len()).step_by(97) {
            let c = &many[i * 3..i * 3 + 3];
            let got = dist(pal[idx[i] as usize], c);
            let best = pal.iter().map(|p| dist(*p, c)).min().unwrap_or(0);
            assert_eq!(got, best, "pixel {i} did not take the nearest colour");
        }
    }

    /// Each strategy emits exactly the sequence it documents. Where a terminal leaves the
    /// cursor after a sixel image is not in the protocol and terminals disagree by a whole
    /// image height, so this is a setting; what it must not do is emit something other than
    /// what the setting names, because then a user reading the probe cannot act on it.
    #[test]
    fn each_cursor_strategy_emits_what_it_claims() {
        let m = Metrics { cols: 80, rows: 24, xpix: 640, ypix: 384 };
        let h = 100u32; // 16px cells, so 7 rows
        let px = solid(20, h, Rgb(1, 2, 3));
        let seq = |c| String::from_utf8(place(&px, 20, h, m, c)).unwrap();
        let nl = "\n".repeat(7);

        // The default: reserve the space, draw, and let the terminal's own advance stand.
        let d = seq(Cursor::Reserve);
        assert!(d.starts_with(&format!("{nl}\x1b[7A\x1bP")), "{:?}", &d[..20]);
        assert!(d.ends_with("\x1b\\\r"));
        assert_eq!(seq(Cursor::default()), d, "Reserve is the default");

        // For a terminal that does not advance: save, draw, restore, descend.
        let a = seq(Cursor::Advance);
        assert!(a.starts_with(&format!("{nl}\x1b[7A\x1b7\x1bP")), "{:?}", &a[..24]);
        assert!(a.ends_with("\x1b\\\x1b8\x1b[7B\r"));

        // And the two that add nothing before the image.
        assert!(seq(Cursor::Bare).starts_with("\x1bP"));
        assert!(seq(Cursor::Bare).ends_with("\x1b\\\r"));
        assert!(seq(Cursor::Newline).ends_with("\x1b\\\r\n"));

        // A one-row image still reserves a row: `max(1)`, not zero.
        let tiny = String::from_utf8(place(&solid(4, 4, Rgb(1, 2, 3)), 4, 4, m, Cursor::Reserve))
            .unwrap();
        assert!(tiny.starts_with("\n\x1b[1A"), "{:?}", &tiny[..8]);
    }

    /// An image taller than the screen reserves nothing. The cursor-up that pairs with the
    /// reserve clamps at the top of the screen, so beyond that height the two stop cancelling
    /// and the image is drawn from wherever the clamp left the cursor: a tall chart came out
    /// with two screens of blank above it, while a short one on the same terminal was fine.
    #[test]
    fn an_image_taller_than_the_screen_reserves_nothing() {
        // 24 rows of 16px: 384px fits, 400px does not.
        let m = Metrics { cols: 80, rows: 24, xpix: 640, ypix: 384 };
        let fits = String::from_utf8(place(&solid(8, 320, Rgb(1, 2, 3)), 8, 320, m, Cursor::Reserve))
            .unwrap();
        assert!(fits.starts_with('\n'), "a 20-row image should reserve");

        let tall = String::from_utf8(place(&solid(8, 800, Rgb(1, 2, 3)), 8, 800, m, Cursor::Reserve))
            .unwrap();
        assert!(tall.starts_with("\x1bP"), "a 50-row image must not reserve on a 24-row screen");
        assert!(tall.ends_with("\x1b\\\r"));

        // The same applies to the strategy that descends afterwards: the descent is just as
        // clamped as the ascent, so it cannot be trusted either.
        let tall_adv =
            String::from_utf8(place(&solid(8, 800, Rgb(1, 2, 3)), 8, 800, m, Cursor::Advance))
                .unwrap();
        assert!(tall_adv.starts_with("\x1bP"), "advance must also stand down when it cannot fit");
    }

    /// The names `.plt.sixel_cursor` accepts, and a message for anything else.
    #[test]
    fn cursor_names_round_trip() {
        for (n, want) in [
            ("", Cursor::Reserve),
            ("reserve", Cursor::Reserve),
            ("advance", Cursor::Advance),
            ("bare", Cursor::Bare),
            ("newline", Cursor::Newline),
        ] {
            assert_eq!(Cursor::from_name(n), Ok(want), "{n}");
        }
        let e = Cursor::from_name("wezterm").unwrap_err();
        assert!(e.contains("unknown") && e.contains("advance"), "{e}");
    }

    /// Encode then decode. Colours come back within one step per channel, which is sixel's
    /// own limit rather than a fault in either side: components are stored as percentages of
    /// 0..100, so an 8-bit value cannot survive exactly.
    #[test]
    fn a_stream_decodes_back_to_its_pixels() {
        let (w, h) = (23u32, 14u32);
        let mut rgb = Vec::new();
        for i in 0..w * h {
            let v = (i % 5) as u8 * 60;
            rgb.extend_from_slice(&[v, 255 - v, 128]);
        }
        let got = decode(&image(&rgb, w, h));
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0, got[0].1), (w, h));
        let err = got[0].2.iter().zip(&rgb).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(err <= 3, "worst channel error {err}, which is more than percentage rounding");
    }

    /// Through the cursor handling as well, and several images in one stream: a report is a
    /// sequence and the order is the report. The reserve newlines and cursor escapes between
    /// images must not be mistaken for image data.
    #[test]
    fn a_sequence_decodes_in_order() {
        let m = Metrics { cols: 80, rows: 24, xpix: 640, ypix: 384 };
        let mut stream = Vec::new();
        let sizes = [(9u32, 7u32), (14, 12), (6, 18)];
        for (i, (w, h)) in sizes.iter().enumerate() {
            let c = Rgb((i as u8 + 1) * 60, 20, 200);
            stream.extend(place(&solid(*w, *h, c), *w, *h, m, Cursor::Reserve));
            stream.extend(b"some text between the plots\n");
        }
        let got = decode(&stream);
        assert_eq!(got.len(), 3, "text between images must not break the split");
        for (i, (w, h)) in sizes.iter().enumerate() {
            assert_eq!((got[i].0, got[i].1), (*w, *h), "image {i}");
            assert!(
                got[i].2[0].abs_diff((i as u8 + 1) * 60) <= 3,
                "image {i} colour"
            );
        }
    }

    /// A run of one colour costs a count, not a character each. Without this a flat 900px
    /// background line is 900 bytes per colour per band, and the stream dwarfs the pixels.
    #[test]
    fn long_runs_are_encoded_as_runs() {
        let s = String::from_utf8(image(&solid(600, 6, Rgb(0, 0, 255)), 600, 6)).unwrap();
        assert!(s.contains("!600~"), "expected one run of 600: {s}");
        assert!(s.len() < 100, "a solid band should be tiny, got {} bytes", s.len());
    }

    /// Encoding is deterministic. The palette comes out of a HashMap, whose order is
    /// arbitrary, so without an explicit tie-break the same chart encodes differently between
    /// runs and nothing downstream can be compared byte for byte.
    #[test]
    fn the_same_image_encodes_the_same_way() {
        let mut px = Vec::new();
        for i in 0..1200u32 {
            let v = (i % 17) as u8 * 15;
            px.extend_from_slice(&[v, 200, 255 - v]);
        }
        let a = image(&px, 40, 30);
        for _ in 0..8 {
            assert_eq!(image(&px, 40, 30), a);
        }
    }
}
