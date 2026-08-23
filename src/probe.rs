//! A test pattern for the kitty graphics protocol, and the variants that isolate a failure.
//!
//! Terminals differ in which optional pieces of the protocol they accept, and they fail
//! *silently*: the image is short, or blank, or a band of noise, with no error anywhere. This
//! sends the same picture several ways, changing one thing at a time, so the answer to "which
//! ones rendered?" names the piece the terminal rejects.
//!
//! The pattern is built to be read when it is BROKEN rather than when it works. Rulers down
//! two edges mean a truncated image can be reported as "it stopped at row 180" instead of
//! "about two thirds"; the colour bands make a horizontal crop obvious; the corner markers
//! show a crop from any side; and each frame carries its own variant number, so what you are
//! looking at is never in doubt.

use crate::canvas::{Align, Canvas, Rgb, TextStyle};
use crate::kitty::{self, Metrics, Wire};
use crate::raster::Raster;

const BG: Rgb = Rgb(0x10, 0x12, 0x18);
const FG: Rgb = Rgb(0xe6, 0xe9, 0xef);
const RED: Rgb = Rgb(0xff, 0x45, 0x45);
const GREEN: Rgb = Rgb(0x3e, 0xcf, 0x8e);
const BLUE: Rgb = Rgb(0x4c, 0x9a, 0xff);

/// One row of the variant table: what to call it, and the wire settings it uses.
pub struct Variant {
    /// Drawn INTO the frame, so it has to stay short enough to fit at the smallest size the
    /// probe is useful at. The reason it exists goes in `note`, which is only printed.
    pub label: &'static str,
    pub note: &'static str,
    pub wire: Wire,
}

/// The variants, ordered so the FIRST failure names the culprit: each one adds a single
/// piece to the one above it, and the last is exactly what jplots sends.
pub fn variants() -> Vec<Variant> {
    let base = Wire { compress: false, chunk: usize::MAX, cells: false };
    vec![
        Variant {
            label: "raw, 1 escape",
            note: "the simplest thing the protocol allows",
            wire: base,
        },
        Variant {
            label: "raw, chunked",
            note: "adds 4096-byte chunking",
            wire: Wire { chunk: 4096, ..base },
        },
        Variant {
            label: "zlib, 1 escape",
            note: "adds o=z compression, drops chunking",
            wire: Wire { compress: true, ..base },
        },
        Variant {
            label: "zlib, chunked",
            note: "both together",
            wire: Wire { compress: true, chunk: 4096, ..base },
        },
        Variant {
            label: "zlib, chunked, c/r",
            note: "adds the cell box: what jplots sends",
            wire: Wire::default(),
        },
    ]
}

/// The test pattern, `w`×`h` RGB. `n` and `label` are drawn into it so a rendered frame
/// identifies itself.
pub fn pattern(w: u32, h: u32, n: usize, label: &str, scale: u32) -> Vec<u8> {
    let mut c = Raster::new(w, h);
    let (fw, fh) = (w as f64, h as f64);
    c.clear(BG);

    // The rulers own the top and left edges; everything else keeps out of them, or the tick
    // labels sit on the artwork and neither can be read.
    let (gl, gt) = (14.0 * scale as f64 + 16.0, 9.0 * scale as f64 + 14.0);
    let (cx0, cw) = (gl, (fw - gl - 8.0).max(1.0));
    let line_h = 9.0 * scale as f64 + 4.0;

    // Three vertical bands. A horizontal crop shows up as a missing colour rather than as a
    // width you would have to measure.
    let band_y = gt + 6.0;
    let band_h = ((fh - gt) * 0.22).max(6.0);
    for (i, col) in [RED, GREEN, BLUE].into_iter().enumerate() {
        c.rect(cx0 + i as f64 * cw / 3.0, band_y, cw / 3.0, band_h, col);
    }

    // A greyscale ramp: 17 steps from black to white. Banding or a colour cast here is the
    // terminal's, not ours, because the bytes are exact multiples of 15.
    let ramp_y = band_y + band_h + 8.0;
    let ramp_h = ((fh - gt) * 0.16).max(5.0);
    for i in 0..17u32 {
        let v = (i * 15) as u8;
        c.rect(cx0 + i as f64 * cw / 17.0, ramp_y, cw / 17.0, ramp_h, Rgb(v, v, v));
    }

    // Rulers every 50px, labelled every 100. A partial image can then be reported as a
    // NUMBER, which is the difference between a useful report and "some of it".
    let mut y = 0.0;
    while y < fh {
        let long = (y as u32).is_multiple_of(100);
        c.line((0.0, y), (if long { 10.0 } else { 5.0 }, y), FG, 1.0);
        if long && y > 0.0 {
            c.text(
                (12.0, y),
                &format!("{}", y as u32),
                FG,
                TextStyle::new(Align::Start, Align::Middle, scale),
            );
        }
        y += 50.0;
    }
    let mut x = gl;
    while x < fw {
        let v = (x - gl) as u32;
        let long = v.is_multiple_of(100);
        c.line((x, 0.0), (x, if long { 10.0 } else { 5.0 }), FG, 1.0);
        if long && v > 0 {
            c.text(
                (x + 2.0, 12.0 + line_h / 2.0),
                &format!("{v}"),
                FG,
                TextStyle::new(Align::Start, Align::Middle, scale),
            );
        }
        x += 50.0;
    }

    let centre = cx0 + cw / 2.0;
    let text_y = ramp_y + ramp_h + 10.0 + line_h / 2.0;
    c.text(
        (centre, text_y),
        &format!("{n}  {label}"),
        FG,
        TextStyle::new(Align::Middle, Align::Middle, scale),
    );
    c.text(
        (centre, text_y + line_h),
        &format!("{w}x{h}"),
        FG.mix(BG, 0.35),
        TextStyle::new(Align::Middle, Align::Middle, scale),
    );

    // A 1px checkerboard. This is the one thing that catches RESCALING: at 1:1 it reads as a
    // fine texture, and if the terminal is resampling the image to fit a cell box it collapses
    // into flat grey or a moire. Everything else in the pattern survives being scaled.
    let chk_y = text_y + line_h * 2.0;
    let chk_h = (fh - chk_y - 14.0).clamp(0.0, 96.0);
    if chk_h >= 8.0 {
        let chk_w = cw.min(256.0);
        for yy in 0..chk_h as u32 {
            for xx in 0..chk_w as u32 {
                if (xx + yy) % 2 == 0 {
                    c.rect(cx0 + xx as f64, chk_y + yy as f64, 1.0, 1.0, FG);
                }
            }
        }
        c.text(
            (cx0 + chk_w + 10.0, chk_y + chk_h / 2.0),
            "1px checkerboard:",
            FG,
            TextStyle::new(Align::Start, Align::Middle, scale),
        );
        c.text(
            (cx0 + chk_w + 10.0, chk_y + chk_h / 2.0 + line_h),
            "grey = rescaled",
            FG.mix(BG, 0.35),
            TextStyle::new(Align::Start, Align::Middle, scale),
        );
    }

    // A diagonal across the content: any aspect-ratio or scaling mistake bends it off the
    // far corner, which nothing else in the pattern would show.
    c.line((cx0, gt), (fw - 1.0, fh - 1.0), Rgb(0x5a, 0x62, 0x74), 1.0);

    // Corner markers, so a crop from ANY side is visible. All four present means the whole
    // image arrived, whatever else is wrong with it.
    for (mx, my) in [(0.0, 0.0), (fw - 10.0, 0.0), (0.0, fh - 10.0), (fw - 10.0, fh - 10.0)] {
        c.rect(mx, my, 10.0, 10.0, FG);
    }

    // A 1px border LAST, so nothing overdraws it: a missing edge means a cropped image.
    c.line((0.0, 0.0), (fw - 1.0, 0.0), FG, 1.0);
    c.line((0.0, fh - 1.0), (fw - 1.0, fh - 1.0), FG, 1.0);
    c.line((0.0, 0.0), (0.0, fh - 1.0), FG, 1.0);
    c.line((fw - 1.0, 0.0), (fw - 1.0, fh - 1.0), FG, 1.0);

    c.bytes().to_vec()
}

/// A frame using exactly `n` distinct colours, for finding a terminal's palette limit.
///
/// Sixel allows 256 and terminals differ in how many colour registers they really provide. A
/// chart uses up to all of them while this probe's main pattern uses six, which is enough to
/// make a terminal look fine here and draw a chart as mud. Each frame is a smooth ramp cut
/// into `n` strips, so a limit shows as the ramp collapsing rather than as a subtle shift.
pub fn palette_frame(w: u32, h: u32, n: u32, scale: u32) -> Vec<u8> {
    let mut c = Raster::new(w, h);
    let (fw, fh) = (w as f64, h as f64);
    c.clear(BG);
    let line_h = 9.0 * scale as f64 + 4.0;
    let top = line_h * 2.0;
    let band = (fh - top - line_h * 2.0).max(4.0);
    for i in 0..n {
        // A hue sweep rather than a grey ramp: a terminal that quantises to a small palette
        // shows it as banding you cannot mistake for the picture being correct.
        let t = i as f64 / (n.max(2) - 1) as f64;
        let (r, g, b) = (
            (255.0 * (1.0 - t)) as u8,
            (255.0 * (1.0 - (t - 0.5).abs() * 2.0)) as u8,
            (255.0 * t) as u8,
        );
        c.rect(fw * i as f64 / n as f64, top, fw / n as f64 + 1.0, band, Rgb(r, g, b));
    }
    c.text(
        (fw / 2.0, line_h),
        &format!("{n} colours"),
        FG,
        TextStyle::new(Align::Middle, Align::Middle, scale),
    );
    for (mx, my) in [(0.0, 0.0), (fw - 10.0, 0.0), (0.0, fh - 10.0), (fw - 10.0, fh - 10.0)] {
        c.rect(mx, my, 10.0, 10.0, FG);
    }
    c.bytes().to_vec()
}

/// The colour counts the palette probe walks, doubling to the sixel maximum.
pub const PALETTE_STEPS: [u32; 6] = [4, 16, 64, 128, 200, 256];

/// The escape bytes for one variant, ready to write to a terminal.
pub fn frame(v: &Variant, n: usize, w: u32, h: u32, m: Metrics, tmux: bool) -> Vec<u8> {
    let rgb = pattern(w, h, n, v.label, m.font_scale());
    kitty::encode_with(&rgb, w, h, m, tmux, v.wire)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every variant produces a well-formed stream with the keys its settings imply. The
    /// terminal's behaviour cannot be asserted here; that the wire matches the request can.
    #[test]
    fn each_variant_encodes_what_it_claims() {
        let m = Metrics { cols: 80, rows: 24, xpix: 640, ypix: 384 };
        for (n, v) in variants().iter().enumerate() {
            let bytes = frame(v, n + 1, 200, 120, m, false);
            let s = String::from_utf8_lossy(&bytes);
            // From the transmit onwards: the stream opens by reserving rows for the image.
            let at = s.find("\x1b_Ga=T").unwrap_or(0);
            let head = s[at..].split(';').next().unwrap_or_default().to_string();
            assert!(head.starts_with("\x1b_Ga=T,f=24,s=200,v=120"), "{n}: {head}");
            assert_eq!(head.contains(",o=z"), v.wire.compress, "{n} compress: {head}");
            assert_eq!(head.contains(",c="), v.wire.cells, "{n} cells: {head}");
            let escapes = s.matches("\x1b_G").count();
            assert_eq!(escapes > 1, v.wire.chunk < usize::MAX, "{n} chunking: {escapes}");
            assert!(bytes.ends_with(b"\r"), "{n}: cursor not returned below the image");
        }
    }

    /// The pattern must actually contain its landmarks: a border pixel at every corner, and
    /// the three colour bands. A blank frame would otherwise pass the encoding test above.
    #[test]
    fn the_pattern_has_its_landmarks() {
        let (w, h) = (240u32, 160u32);
        let rgb = pattern(w, h, 1, "t", 1);
        let px = |x: u32, y: u32| {
            let i = ((y * w + x) * 3) as usize;
            Rgb(rgb[i], rgb[i + 1], rgb[i + 2])
        };
        for (x, y) in [(0, 0), (w - 1, 0), (0, h - 1), (w - 1, h - 1)] {
            assert_eq!(px(x, y), FG, "corner ({x},{y}) is not marked");
        }
        // The bands sit inside the left gutter, so probe them relative to it. Scan the
        // band's height rather than sampling one row: the diagonal crosses every band, and
        // a single sample happened to land exactly on it.
        let (gl, gt) = (14.0 + 16.0, 9.0 + 14.0);
        let cw = w as f64 - gl - 8.0;
        let (top, bot) = (gt + 6.0, gt + 6.0 + (h as f64 - gt) * 0.22);
        for (i, want) in [RED, GREEN, BLUE].into_iter().enumerate() {
            let x = (gl + cw * (i as f64 + 0.5) / 3.0) as u32;
            let found = (top as u32..bot as u32).any(|y| px(x, y) == want);
            assert!(found, "band {i}: no {want:?} anywhere in x={x}, y={top}..{bot}");
        }
    }
}
