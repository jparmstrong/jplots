//! An RGB pixel buffer implementing [`Canvas`], the backend behind kitty graphics.
//!
//! Antialiasing is coverage-from-distance rather than Bresenham/Wu: for each pixel near a
//! segment, alpha is `clamp(halfwidth + 0.5 - distance, 0, 1)`. It handles any line width
//! and gives round caps for free, and the cost is bounded by the segment's bounding box, so
//! which for a data polyline is a pixel or two wide, and for an axis is a 1-pixel strip.
//! Text is deliberately NOT antialiased: bitmap glyphs blitted on pixel boundaries stay
//! crisp, which is what makes small axis labels readable.

use crate::canvas::{Align, Canvas, Rgb, Rot, TextStyle};
use crate::font;

pub struct Raster {
    w: u32,
    h: u32,
    /// Row-major RGB, 3 bytes per pixel: exactly what the kitty protocol's `f=24` wants,
    /// so the buffer goes to the terminal without a repack.
    buf: Vec<u8>,
}

impl Raster {
    pub fn new(w: u32, h: u32) -> Raster {
        Raster {
            w,
            h,
            buf: vec![0; (w as usize) * (h as usize) * 3],
        }
    }

    /// The raw RGB bytes, for the kitty encoder.
    pub fn bytes(&self) -> &[u8] {
        &self.buf
    }

    fn blend(&mut self, x: i64, y: i64, c: Rgb, a: f64) {
        if a <= 0.0 || x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let a = a.min(1.0);
        let i = ((y as usize) * (self.w as usize) + x as usize) * 3;
        for (k, ch) in [c.0, c.1, c.2].into_iter().enumerate() {
            let old = self.buf[i + k] as f64;
            self.buf[i + k] = (old + (ch as f64 - old) * a).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Distance from `p` to the segment `a`-`b`.
fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 <= f64::EPSILON {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

impl Canvas for Raster {
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn clear(&mut self, c: Rgb) {
        for px in self.buf.chunks_exact_mut(3) {
            px[0] = c.0;
            px[1] = c.1;
            px[2] = c.2;
        }
    }

    fn line(&mut self, p0: (f64, f64), p1: (f64, f64), c: Rgb, width: f64) {
        let hw = (width / 2.0).max(0.35);
        // Axis-aligned strokes are a chart's furniture: candle wicks and bodies, grid
        // lines, the axes. Run through the distance field at a fractional centre, a 1px
        // stroke spreads over two pixel columns at partial coverage each and reads as
        // fuzzy. Snapped so its band starts on a whole pixel and filled as a rectangle, it
        // is crisp, and square caps suit it better than the round ones the distance field
        // gives. Diagonals keep the distance field: that is what makes a data line smooth.
        let vertical = (p0.0 - p1.0).abs() < 0.5;
        let horizontal = (p0.1 - p1.1).abs() < 0.5;
        if vertical || horizontal {
            let snap = |v: f64| (v - hw).round() + hw;
            let (bx0, bx1) = if vertical {
                let x = snap((p0.0 + p1.0) / 2.0);
                (x - hw, x + hw)
            } else {
                (p0.0.min(p1.0), p0.0.max(p1.0))
            };
            let (by0, by1) = if horizontal {
                let y = snap((p0.1 + p1.1) / 2.0);
                (y - hw, y + hw)
            } else {
                (p0.1.min(p1.1), p0.1.max(p1.1))
            };
            self.rect(bx0, by0, bx1 - bx0, by1 - by0, c);
            return;
        }
        let pad = hw + 1.0;
        let x0 = (p0.0.min(p1.0) - pad).floor() as i64;
        let x1 = (p0.0.max(p1.0) + pad).ceil() as i64;
        let y0 = (p0.1.min(p1.1) - pad).floor() as i64;
        let y1 = (p0.1.max(p1.1) + pad).ceil() as i64;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = seg_dist((x as f64 + 0.5, y as f64 + 0.5), p0, p1);
                self.blend(x, y, c, hw + 0.5 - d);
            }
        }
    }

    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: Rgb) {
        // A solid fill reads crisply only on whole pixels: a fractional edge leaves a
        // half-lit column that looks like a soft shadow down the side of a candle body or a
        // bar. Never rounds away to nothing, because a tiny value should still show.
        let (x0, y0) = (x.round(), y.round());
        let (x1, y1) = ((x + w).round().max(x0 + 1.0), (y + h).round().max(y0 + 1.0));
        self.polygon(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)], fill, 1.0);
    }

    fn polygon(&mut self, pts: &[(f64, f64)], fill: Rgb, alpha: f64) {
        if pts.len() < 3 {
            return;
        }
        let ymin = pts
            .iter()
            .fold(f64::MAX, |m, p| m.min(p.1))
            .floor()
            .max(0.0) as i64;
        let ymax =
            (pts.iter().fold(f64::MIN, |m, p| m.max(p.1)).ceil() as i64).min(self.h as i64 - 1);
        let mut xs: Vec<f64> = Vec::with_capacity(pts.len());
        for y in ymin..=ymax {
            let sy = y as f64 + 0.5;
            xs.clear();
            for i in 0..pts.len() {
                let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
                // Half-open in y so a vertex shared by two edges is counted once.
                if (a.1 <= sy) != (b.1 <= sy) {
                    xs.push(a.0 + (sy - a.1) / (b.1 - a.1) * (b.0 - a.0));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for span in xs.chunks_exact(2) {
                let (lo, hi) = (span[0], span[1]);
                for x in (lo.floor() as i64)..=(hi.ceil() as i64) {
                    // Horizontal coverage of this pixel by the span. Vertical edges of a
                    // bar stay clean instead of snapping to the pixel grid.
                    let cov = (hi.min(x as f64 + 1.0) - lo.max(x as f64)).clamp(0.0, 1.0);
                    self.blend(x, y, fill, cov * alpha.clamp(0.0, 1.0));
                }
            }
        }
    }

    fn marker(&mut self, p: (f64, f64), r: f64, c: Rgb) {
        let x0 = (p.0 - r - 1.0).floor() as i64;
        let x1 = (p.0 + r + 1.0).ceil() as i64;
        let y0 = (p.1 - r - 1.0).floor() as i64;
        let y1 = (p.1 + r + 1.0).ceil() as i64;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let d = ((x as f64 + 0.5 - p.0).powi(2) + (y as f64 + 0.5 - p.1).powi(2)).sqrt();
                self.blend(x, y, c, r + 0.5 - d);
            }
        }
    }

    fn text(&mut self, p: (f64, f64), s: &str, c: Rgb, st: TextStyle) {
        let (h, v, scale, rot) = (st.h, st.v, st.scale, st.rot);
        let (w, ht) = self.text_size(s, scale);
        // A rotated string occupies a tall box rather than a wide one, so the anchors are
        // resolved against the swapped extent, so a `Middle` centres it along the axis.
        let (tw, th) = if rot == Rot::Ccw90 { (ht, w) } else { (w, ht) };
        let ox = match h {
            Align::Start => p.0,
            Align::Middle => p.0 - tw / 2.0,
            Align::End => p.0 - tw,
        };
        let oy = match v {
            Align::Start => p.1,
            Align::Middle => p.1 - th / 2.0,
            Align::End => p.1 - th,
        };
        // Snap to the pixel grid: a half-pixel offset would smear every glyph.
        let (ox, oy) = (ox.round() as i64, oy.round() as i64);
        let sc = scale.max(1) as i64;
        let gw = font::W as i64;
        for (i, ch) in s.chars().enumerate() {
            for (row, bits) in font::glyph(ch).iter().enumerate() {
                for col in 0..font::W {
                    if bits >> (font::W - 1 - col) & 1 == 0 {
                        continue;
                    }
                    // Unrotated: characters advance right, glyph rows run down.
                    // Ccw90: characters advance UP from the bottom of the box (so the string
                    // reads bottom-to-top), each glyph's rows run right and its columns run
                    // up, which puts the tops of the letters on the left, as a y-axis label
                    // should read.
                    let (px, py) = if rot == Rot::Ccw90 {
                        (
                            ox + row as i64 * sc,
                            oy + th as i64 - i as i64 * gw * sc - (col as i64 + 1) * sc,
                        )
                    } else {
                        (
                            ox + i as i64 * gw * sc + col as i64 * sc,
                            oy + row as i64 * sc,
                        )
                    };
                    for dy in 0..sc {
                        for dx in 0..sc {
                            self.blend(px + dx, py + dy, c, 1.0);
                        }
                    }
                }
            }
        }
    }

    fn text_size(&self, s: &str, scale: u32) -> (f64, f64) {
        let sc = scale.max(1) as f64;
        (
            s.chars().count() as f64 * font::W as f64 * sc,
            font::H as f64 * sc,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The font table survived extraction: `0` and `M` have ink, space does not, and
    /// glyphs land inside the 6-pixel cell. A silently-zeroed table would draw blank
    /// axis labels and nothing else would fail.
    #[test]
    fn font_has_ink() {
        assert!(font::glyph('0').iter().any(|r| *r != 0));
        assert!(font::glyph('M').iter().any(|r| *r != 0));
        assert!(font::glyph(' ').iter().all(|r| *r == 0));
        for ch in ' '..='~' {
            assert!(
                font::glyph(ch).iter().all(|r| r & 0xc0 == 0),
                "{ch:?} exceeds 6px"
            );
        }
    }

    #[test]
    fn draws_within_bounds_and_marks_pixels() {
        let mut r = Raster::new(20, 10);
        r.clear(Rgb(0, 0, 0));
        r.line((0.0, 5.0), (19.0, 5.0), Rgb(255, 255, 255), 1.0);
        // A 1px line centred on a pixel BOUNDARY splits its coverage over the two
        // neighbouring rows: half intensity each is correct antialiasing, not a miss.
        assert!(r.bytes().iter().any(|b| *b >= 120), "line drew nothing");
        // Off-surface drawing must be clipped, not panic.
        r.line((-50.0, -50.0), (500.0, 500.0), Rgb(255, 0, 0), 3.0);
        r.marker((-5.0, -5.0), 4.0, Rgb(255, 0, 0));
        r.polygon(
            &[(-10.0, -10.0), (100.0, -10.0), (100.0, 100.0)],
            Rgb(0, 255, 0),
            1.0,
        );
        assert_eq!(r.bytes().len(), 20 * 10 * 3);
    }

    /// The pixel bounding box of everything non-black.
    fn ink(r: &Raster) -> (i64, i64, i64, i64) {
        let (w, h) = r.size();
        let (mut x0, mut y0, mut x1, mut y1) = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                if r.bytes()[((y as usize * w as usize) + x as usize) * 3] > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0, y0, x1, y1)
    }

    /// A y-axis label is rotated a quarter turn and reads bottom-to-top. Two things have to
    /// hold: the ink is TALL rather than wide, and it is exactly the transpose of the
    /// horizontal rendering. A rotation that also mirrored would still pass a "tall" check.
    #[test]
    fn rotated_text_is_the_transpose_of_horizontal() {
        let draw = |rot| {
            let mut r = Raster::new(80, 80);
            r.clear(Rgb(0, 0, 0));
            r.text(
                (10.0, 10.0),
                "price",
                Rgb(255, 255, 255),
                TextStyle {
                    h: Align::Start,
                    v: Align::Start,
                    scale: 1,
                    rot,
                },
            );
            let (x0, y0, x1, y1) = ink(&r);
            (x1 - x0, y1 - y0)
        };
        let (hw, hh) = draw(Rot::None);
        let (vw, vh) = draw(Rot::Ccw90);
        assert!(hw > hh, "horizontal text should be wide: {hw}x{hh}");
        assert!(vh > vw, "rotated text should be tall: {vw}x{vh}");
        assert_eq!(
            (hw, hh),
            (vh, vw),
            "rotation must transpose the extent exactly"
        );
    }

    /// Reading direction: the FIRST character sits at the BOTTOM, so the label reads upward.
    /// Getting this backwards renders the text upside down and no assertion about size sees it.
    #[test]
    fn rotated_text_reads_bottom_to_top() {
        let mut r = Raster::new(40, 40);
        r.clear(Rgb(0, 0, 0));
        // Two cells tall (2 chars x 6px); only the first has ink.
        r.text(
            (0.0, 0.0),
            "A ",
            Rgb(255, 255, 255),
            TextStyle::new(Align::Start, Align::Start, 1).rotated(),
        );
        let (_, y0, _, y1) = ink(&r);
        let mid = (2 * font::W as i64) / 2;
        assert!(
            y0 >= mid,
            "the first character must sit below the midpoint, got {y0}..{y1}"
        );
    }

    /// An axis-aligned stroke must land on whole pixels. Centred on a fractional
    /// coordinate it used to spread over two columns at partial coverage, which is the fuzz
    /// shows up worst on candle wicks, where every bar is a thin vertical line.
    #[test]
    fn axis_aligned_strokes_are_crisp() {
        for centre in [20.0, 20.2, 20.5, 20.7] {
            let mut r = Raster::new(40, 40);
            r.clear(Rgb(0, 0, 0));
            r.line((centre, 5.0), (centre, 35.0), Rgb(255, 255, 255), 1.0);
            // Exactly one column of full-intensity pixels, and nothing partial beside it.
            let col_max: Vec<u8> = (0..40)
                .map(|x| (0..40).map(|y| r.bytes()[(y * 40 + x) * 3]).max().unwrap())
                .collect();
            let lit: Vec<usize> = (0..40).filter(|x| col_max[*x] > 0).collect();
            assert_eq!(lit.len(), 1, "centre {centre}: lit columns {lit:?}");
            assert_eq!(col_max[lit[0]], 255, "centre {centre}: partial coverage");
        }
        // A diagonal still antialiases, which is what keeps a data line smooth.
        let mut r = Raster::new(40, 40);
        r.clear(Rgb(0, 0, 0));
        r.line((5.0, 5.0), (35.0, 30.0), Rgb(255, 255, 255), 1.0);
        assert!(
            r.bytes().iter().any(|b| *b > 0 && *b < 255),
            "a diagonal should have partial coverage"
        );
    }

    /// A filled body or bar snaps to whole pixels, so its vertical edges are one solid
    /// column rather than two half-lit ones. A tiny value still draws: rounding must not
    /// make a bar disappear.
    #[test]
    fn filled_rects_snap_to_whole_pixels() {
        let mut r = Raster::new(40, 40);
        r.clear(Rgb(0, 0, 0));
        r.rect(10.3, 10.0, 8.4, 12.0, Rgb(255, 255, 255));
        let col_max: Vec<u8> = (0..40)
            .map(|x| (0..40).map(|y| r.bytes()[(y * 40 + x) * 3]).max().unwrap())
            .collect();
        assert!(
            col_max.iter().all(|v| *v == 0 || *v == 255),
            "a snapped rect should have no partial columns: {col_max:?}"
        );
        // A sliver still shows rather than rounding to nothing.
        let mut r = Raster::new(40, 40);
        r.clear(Rgb(0, 0, 0));
        r.rect(10.0, 10.0, 0.2, 0.2, Rgb(255, 255, 255));
        assert!(r.bytes().iter().any(|b| *b > 0), "a tiny rect vanished");
    }

    /// Alpha tints rather than replaces. A band drawn over a grid line has to leave the line
    /// visible, which is the whole reason the fill is translucent and the reason pre-blending
    /// against the background would have been wrong.
    #[test]
    fn a_translucent_fill_tints_what_is_under_it() {
        let mut r = Raster::new(6, 3);
        r.clear(Rgb(0, 0, 0));
        r.rect(0.0, 1.0, 6.0, 1.0, Rgb(255, 255, 255)); // the "grid line"
        r.polygon(&[(0.0, 0.0), (6.0, 0.0), (6.0, 3.0), (0.0, 3.0)], Rgb(255, 0, 0), 0.5);
        fn px(r: &Raster, x: usize, y: usize) -> (u8, u8, u8) {
            let i = (y * 6 + x) * 3;
            (r.bytes()[i], r.bytes()[i + 1], r.bytes()[i + 2])
        }
        // Over black: half the fill. Over white: the line survives in green and blue.
        assert_eq!(px(&r, 2, 0), (128, 0, 0));
        let (_, g, b) = px(&r, 2, 1);
        assert!(g > 100 && b > 100, "the grid line was erased, not tinted: {:?}", px(&r, 2, 1));
        // Fully transparent draws nothing at all.
        r.polygon(&[(0.0, 0.0), (6.0, 0.0), (6.0, 3.0), (0.0, 3.0)], Rgb(0, 255, 0), 0.0);
        assert_eq!(px(&r, 2, 0), (128, 0, 0));
    }

    /// `#rgb` and `#rrggbb`, with or without the hash, and nothing else.
    #[test]
    fn hex_colours_parse() {
        assert_eq!(Rgb::from_hex("#4c9aff"), Some(Rgb(0x4c, 0x9a, 0xff)));
        assert_eq!(Rgb::from_hex("4c9aff"), Some(Rgb(0x4c, 0x9a, 0xff)));
        assert_eq!(Rgb::from_hex("#abc"), Some(Rgb(0xaa, 0xbb, 0xcc)));
        assert_eq!(Rgb::from_hex(" #4C9AFF "), Some(Rgb(0x4c, 0x9a, 0xff)));
        for bad in ["", "#12345", "#gggggg", "blue", "#12345678"] {
            assert_eq!(Rgb::from_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn polygon_fills_a_rect() {
        let mut r = Raster::new(10, 10);
        r.clear(Rgb(0, 0, 0));
        r.rect(2.0, 2.0, 6.0, 6.0, Rgb(255, 255, 255));
        let px = |x: usize, y: usize| r.bytes()[(y * 10 + x) * 3];
        assert_eq!(px(5, 5), 255, "inside");
        assert_eq!(px(0, 0), 0, "outside");
    }
}
