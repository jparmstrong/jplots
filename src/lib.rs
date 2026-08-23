//! Terminal plots for q over the kitty graphics protocol.
//!
//! The renderer knows nothing about any q runtime: it takes plain `f64` series and returns
//! terminal escape bytes. Everything host-specific lives in [`kapi`], which reads kdb's `K`
//! objects, so kdb+ is served through `2:` and a Rust host that links this crate directly
//! can build a [`Plot`] itself and skip `kapi` entirely.
//!
//! ```text
//! q                q/plt.q builds one spec dict from a table/dict/vector
//!    |
//! kapi             K -> Plot                       (kdb+ via 2:, or a Rust host directly)
//!    |
//! layout           ticks, margins, legend, projection   <- written once
//!    |
//! Canvas trait     line . polygon . marker . text
//!    +- Raster  -> kitty graphics protocol   (here)
//!    +- Svg     -> text/html for Jupyter     (next)
//! ```

pub mod canvas;
pub mod font;
#[cfg(feature = "kapi")]
pub mod kapi;
pub mod kitty;
pub mod layout;
pub mod probe;
pub mod png;
pub mod raster;
pub mod sixel;

pub use canvas::{Align, Canvas, Rgb};

/// The q front end, `q/plt.q`, embedded so a Rust host can evaluate it straight from the
/// library instead of shipping a copy that drifts. kdb+ loads the same file from disk.
pub const PLT_Q: &str = include_str!("../q/plt.q");
pub use raster::Raster;

/// What to draw. `Hist` is the odd one: its series carries RAW observations and the bins
/// are computed at layout time, because the bin edges depend on the axis range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Line,
    Scatter,
    Bar,
    Hist,
    /// OHLC candlesticks. The series are positional (open, high, low, close), which the q
    /// side arranges by column name, so the renderer never has to guess.
    Candle,
    /// A scatter matrix: every series against every other in an N×N grid, the diagonal a
    /// histogram of that series. The pairing is derived here, like `Hist`'s bins are, so the
    /// spec stays one series per variable instead of N² panels.
    Matrix,
    /// Lines, and translucent regions between a `lo` and a `hi`. A series with those is a
    /// band; one without is a plain line, so the two mix in a single chart. `.plt.bands`.
    Bands,
}

/// How an axis renders its tick values. q temporals arrive as their underlying integer in
/// q's epoch (dates = days since 2000.01.01, timestamps = nanoseconds since the same), so
/// the formatting has to happen here rather than being baked into strings by the caller:
/// tick POSITIONS are only known once the axis range is laid out.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TickFmt {
    #[default]
    Num,
    Date,
    /// Milliseconds since midnight (q `time`).
    Time,
    /// Nanoseconds since 2000.01.01 (q `timestamp`).
    Timestamp,
    /// Positions are 0,1,2… into [`Plot::xcats`].
    Cat,
}

#[derive(Clone, Debug, Default)]
pub struct Series {
    pub name: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    /// Draw this one as a line ON TOP of the plot's own kind instead of in it: a fitted
    /// model over the sample it was fitted to. It still joins the axis ranges and the key.
    /// Kept out of the kind's own pass because that pass is not per-series: an extra series
    /// narrows every bar's slot, and a candlestick is exactly four series, positionally.
    pub overlay: bool,
    /// A translucent region between these, for [`Kind::Bands`]. A series is a band when it
    /// has them, so no per-series kind is needed: `y` is then the line THROUGH the band and
    /// may be empty, which is a band with no centre.
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    /// An explicit colour, from `.plt.bands`' style dict. `None` takes the palette in order.
    pub colour: Option<Rgb>,
}

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Rgb,
    pub fg: Rgb,
    pub grid: Rgb,
    pub axis: Rgb,
    /// Candlestick bodies. Colour alone would be a poor cue, so an up bar is drawn hollow
    /// and a down bar filled. That is the traditional convention, and it survives a
    /// colour-blind reader or a monochrome terminal.
    pub up: Rgb,
    pub down: Rgb,
}

impl Theme {
    /// Terminals are usually dark, and a white chart in a dark terminal is a flashbang.
    pub fn dark() -> Theme {
        Theme {
            bg: Rgb(0x14, 0x16, 0x1c),
            fg: Rgb(0xc6, 0xcc, 0xd6),
            grid: Rgb(0x2a, 0x2f, 0x3b),
            axis: Rgb(0x55, 0x5d, 0x6d),
            up: Rgb(0x3e, 0xcf, 0x8e),
            down: Rgb(0xff, 0x5c, 0x8a),
        }
    }

    pub fn light() -> Theme {
        Theme {
            bg: Rgb(0xff, 0xff, 0xff),
            fg: Rgb(0x22, 0x26, 0x2c),
            grid: Rgb(0xe2, 0xe6, 0xea),
            axis: Rgb(0x9a, 0xa2, 0xac),
            up: Rgb(0x18, 0x9e, 0x63),
            down: Rgb(0xd6, 0x2f, 0x5e),
        }
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::dark()
    }
}

/// Eight hues that stay distinguishable on both themes and in the common forms of colour
/// blindness: no red/green pair carries meaning on its own.
pub const PALETTE: [Rgb; 8] = [
    Rgb(0x4e, 0x9b, 0xff),
    Rgb(0xff, 0x7a, 0x45),
    Rgb(0x3e, 0xcf, 0x8e),
    Rgb(0xff, 0x5c, 0x8a),
    Rgb(0xb9, 0x8c, 0xff),
    Rgb(0xff, 0xd1, 0x66),
    Rgb(0x4e, 0xcd, 0xc4),
    Rgb(0x9c, 0xa6, 0xb4),
];

/// The renderer `.plt.renderer` selects. `Sixel` is the default.
///
/// Both are raster and both read the same [`Raster`], so the choice is only which escape
/// sequence carries the pixels. Shared, and returning the error text as well as the value, so
/// the `2:` entry point and any other host bridge answer a caller identically: a backend
/// added to one and missed by the other is the failure this pattern already prevented once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Backend {
    /// APC escapes. kitty, ghostty, iTerm2, Konsole. Not tmux: it forwards them and then
    /// erases the image on its next repaint, having no record that one is there.
    Kitty,
    /// DCS escapes, a 256-colour palette. WezTerm, xterm, foot, mlterm, Contour, and tmux,
    /// which understands a sixel image well enough to repaint it.
    Sixel,
}

impl Backend {
    /// Parse `.plt.renderer`, or the message to signal.
    pub fn from_name(s: &str) -> Result<Backend, String> {
        match s {
            // Sixel is the default: it is what the widest set of terminals draws, and the
            // one notable exception, kitty itself, is also the one that reads `kitty`.
            "" | "sixel" => Ok(Backend::Sixel),
            "kitty" => Ok(Backend::Kitty),
            // Named in the settings table and not written yet. Saying so beats drawing
            // nothing, and beats spraying escapes at a terminal that cannot read them.
            "svg" => Err(".plt.renderer: `svg is not built yet (`sixel and `kitty so far)".into()),
            other => Err(format!(
                ".plt.renderer: unknown renderer `{other} (known: `sixel, `kitty)"
            )),
        }
    }
}

impl Plot {
    /// The escape bytes that draw this plot, without touching the terminal.
    pub fn encode(&self, backend: Backend) -> Vec<u8> {
        let (w, h) = (self.width.max(64), self.height.max(48));
        let r = self.raster();
        match backend {
            Backend::Kitty => kitty::encode(r.bytes(), w, h, kitty::metrics(), kitty::in_tmux()),
            Backend::Sixel => sixel::place(r.bytes(), w, h, kitty::metrics(), self.sixel_cursor),
        }
    }
}

impl Kind {
    /// Parse the `kind` a spec carries. Shared so a host bridge cannot drift from the `2:`
    /// entry point. A chart type added to one and missed by the other happened exactly once.
    pub fn from_name(s: &str) -> Option<Kind> {
        Some(match s {
            "" | "line" => Kind::Line,
            "scatter" => Kind::Scatter,
            "bar" => Kind::Bar,
            "hist" => Kind::Hist,
            "candle" => Kind::Candle,
            "matrix" => Kind::Matrix,
            "bands" => Kind::Bands,
            _ => return None,
        })
    }
}

impl TickFmt {
    pub fn from_name(s: &str) -> Option<TickFmt> {
        Some(match s {
            "" | "num" => TickFmt::Num,
            "date" => TickFmt::Date,
            "time" => TickFmt::Time,
            "timestamp" => TickFmt::Timestamp,
            "cat" => TickFmt::Cat,
            _ => return None,
        })
    }
}

impl Theme {
    /// `light`, or the dark default for anything else.
    pub fn from_name(s: &str) -> Theme {
        if s == "light" {
            Theme::light()
        } else {
            Theme::dark()
        }
    }
}

/// The pixel size and glyph scale a plot will use, from the terminal and whatever the caller
/// pinned. Shared by every host bridge: the clamps are fiddly enough that two copies would
/// drift, and a wrong one shows up as a blurry chart rather than an error.
///
/// `size` is `(width, height)` in pixels, either may be absent; `scale` multiplies both size
/// and font; `font` pins the glyph scale. Anything out of range falls back to the terminal.
pub fn resolve_geometry(size: &[f64], scale: Option<f64>, font: Option<f64>) -> (u32, u32, u32) {
    let m = kitty::metrics();
    let (dw, dh) = m.default_plot();
    let scale = scale
        .filter(|v| v.is_finite() && (0.25..=8.0).contains(v))
        .unwrap_or(1.0);
    let font = font
        .filter(|v| (1.0..=6.0).contains(v))
        .map_or_else(|| m.font_scale(), |v| v as u32);
    let px = |v: Option<f64>, dflt: u32, min: f64| {
        (v.filter(|v| *v >= min).unwrap_or(dflt as f64) * scale).clamp(min, 8192.0) as u32
    };
    (
        px(size.first().copied(), dw, 64.0),
        px(size.get(1).copied(), dh, 48.0),
        ((font as f64 * scale).round() as u32).clamp(1, 6),
    )
}

#[derive(Clone, Debug)]
pub struct Plot {
    pub kind: Kind,
    pub series: Vec<Series>,
    pub title: String,
    pub xlabel: String,
    pub ylabel: String,
    pub width: u32,
    pub height: u32,
    pub xfmt: TickFmt,
    pub yfmt: TickFmt,
    /// Where to leave the cursor after a sixel image. Nothing in the protocol says, and
    /// terminals disagree by a whole image height, so it is a setting rather than a guess.
    pub sixel_cursor: sixel::Cursor,
    /// Categorical x labels, indexed by position. `Bar` uses these.
    pub xcats: Vec<String>,
    /// `Matrix` only: N×N least-squares fits, indexed `[row][col]`, giving the line in panel
    /// (row, col), which draws series `row` against series `col`. Computed in q, where the
    /// caller also gets them back as a table, rather than twice in two places. Empty leaves
    /// the panels bare, which is what `fit_line:0b` asks for.
    pub beta: Vec<Vec<f64>>,
    pub alpha: Vec<Vec<f64>>,
    /// N×N correlation, for the corner of each panel. Sent even when the line is not, since
    /// it is the summary rather than the drawing.
    pub corr: Vec<Vec<f64>>,
    pub bins: usize,
    pub theme: Theme,
    /// Integer glyph scale. `0` derives one from the image width, which is only a guess:
    /// the caller should pass the terminal's, via [`kitty::Metrics::font_scale`].
    pub font: u32,
}

impl Default for Plot {
    fn default() -> Plot {
        Plot {
            kind: Kind::Line,
            series: Vec::new(),
            title: String::new(),
            xlabel: String::new(),
            ylabel: String::new(),
            width: 900,
            height: 480,
            xfmt: TickFmt::Num,
            yfmt: TickFmt::Num,
            sixel_cursor: sixel::Cursor::default(),
            xcats: Vec::new(),
            beta: Vec::new(),
            alpha: Vec::new(),
            corr: Vec::new(),
            bins: 30,
            theme: Theme::dark(),
            font: 0,
        }
    }
}

impl Plot {
    /// Render to an RGB raster. The kitty encoder takes it from here.
    pub fn raster(&self) -> Raster {
        let mut r = Raster::new(self.width.max(64), self.height.max(48));
        layout::draw(self, &mut r);
        r
    }
}
