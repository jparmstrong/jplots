//! The drawing surface every backend implements.
//!
//! Layout ([`crate::layout`]) computes device coordinates and then talks ONLY to this
//! trait, so a new backend is a new implementation and nothing else. The tick algorithm,
//! label placement, legend and series drawing are written once. Two boundaries matter:
//!
//! * `text` is a SEMANTIC call, never "blit these pixels". The raster backend renders it
//!   from the bitmap font; an SVG backend emits a `<text>` element with a real font, which
//!   only works if the string survives to that point.
//! * `polygon` is here from the start even though 2-D charts only ever need `rect`.
//!   3-D surfaces are shaded quads, so a trait without it makes 3-D a retrofit; with it,
//!   projection stays entirely inside layout and every backend gets 3-D for free.

/// 24-bit colour. The terminal is the only consumer, and both the kitty protocol (`f=24`)
/// and SVG want 8 bits per channel, so there is no reason to carry more.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// `#rrggbb` or `#rgb`, with or without the hash. `None` for anything else, so a caller
    /// can say which colour was rejected rather than silently drawing in black.
    pub fn from_hex(s: &str) -> Option<Rgb> {
        let h = s.trim().trim_start_matches('#');
        let n = |i: usize, w: usize| u8::from_str_radix(h.get(i..i + w)?, 16).ok();
        match h.len() {
            6 => Some(Rgb(n(0, 2)?, n(2, 2)?, n(4, 2)?)),
            // `#abc` is `#aabbcc`, which is what CSS means by it.
            3 => Some(Rgb(n(0, 1)? * 17, n(1, 1)? * 17, n(2, 1)? * 17)),
            _ => None,
        }
    }
}

impl Rgb {
    /// `#rrggbb`, for the SVG backend.
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }

    /// Blend towards `other` by `t` in 0..=1, for grid lines and legend chips that
    /// want a muted version of a series colour without a second palette.
    pub fn mix(self, other: Rgb, t: f64) -> Rgb {
        let f = |a: u8, b: u8| {
            (a as f64 + (b as f64 - a as f64) * t)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        Rgb(f(self.0, other.0), f(self.1, other.1), f(self.2, other.2))
    }
}

/// Text orientation. `Ccw90` reads bottom-to-top, which is where a y-axis label belongs.
/// A backend that draws real text renders this as a transform; the raster one transposes
/// the glyph blit.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Rot {
    #[default]
    None,
    Ccw90,
}

/// Where `(x, y)` sits relative to the text box.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Start,
    Middle,
    End,
}

/// How a string is placed and oriented. Grouped rather than passed positionally: the two
/// alignments are the same type and sit next to each other, so as loose arguments they are
/// silently swappable, and a swapped pair moves a label without failing anything.
#[derive(Clone, Copy, Debug)]
pub struct TextStyle {
    /// Where `p` sits horizontally within the text's DEVICE box.
    pub h: Align,
    /// …and vertically. For `Rot::Ccw90` the box is tall rather than wide, so a `Middle`
    /// here centres the rotated string along the axis it labels.
    pub v: Align,
    /// Integer multiplier, so HiDPI plots keep crisp glyph edges.
    pub scale: u32,
    pub rot: Rot,
}

impl TextStyle {
    pub fn new(h: Align, v: Align, scale: u32) -> TextStyle {
        TextStyle {
            h,
            v,
            scale,
            rot: Rot::None,
        }
    }

    /// A quarter turn anticlockwise, reading bottom-to-top, for a y-axis label.
    pub fn rotated(self) -> TextStyle {
        TextStyle {
            rot: Rot::Ccw90,
            ..self
        }
    }
}

pub trait Canvas {
    /// Device size in pixels (SVG uses the same units).
    fn size(&self) -> (u32, u32);

    /// Fill the whole surface.
    fn clear(&mut self, c: Rgb);

    /// A `width`-pixel line. Backends antialias if they can.
    fn line(&mut self, p0: (f64, f64), p1: (f64, f64), c: Rgb, width: f64);

    /// A filled convex-or-simple polygon, at `alpha` in 0..=1.
    ///
    /// Translucency is a parameter rather than a property of the colour because that is what
    /// the backends want: the raster already blends by coverage, so this multiplies into it,
    /// and SVG has `fill-opacity` as a separate attribute. Pre-blending against the background
    /// instead would ERASE a grid line under a band rather than tint it.
    fn polygon(&mut self, pts: &[(f64, f64)], fill: Rgb, alpha: f64);

    /// A filled axis-aligned rectangle.
    fn rect(&mut self, x: f64, y: f64, w: f64, h: f64, fill: Rgb) {
        self.polygon(&[(x, y), (x + w, y), (x + w, y + h), (x, y + h)], fill, 1.0);
    }

    /// A filled round marker of radius `r`.
    fn marker(&mut self, p: (f64, f64), r: f64, c: Rgb);

    /// Draw `s` anchored at `p`, placed and oriented by `style`.
    fn text(&mut self, p: (f64, f64), s: &str, c: Rgb, style: TextStyle);

    /// The `(width, height)` unrotated `text` would occupy. Layout needs this for margins,
    /// and swaps the pair itself for a rotated string.
    fn text_size(&self, s: &str, scale: u32) -> (f64, f64);
}
