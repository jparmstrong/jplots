//! Turning a [`Plot`] into drawing calls: axis ranges, tick selection, label formatting,
//! margins, legend placement, and the series themselves.
//!
//! Everything here is backend-agnostic (it talks only to [`Canvas`]), so the SVG and text
//! backends inherit all of it. When 3-D lands, the camera projection goes in this file
//! (data → device stops being a pair of 1-D scales and becomes a projection, and the draw
//! order gets a depth sort before it emits), and no backend changes.

use crate::canvas::{Align, Canvas, TextStyle};
use crate::{Kind, Plot, Series, TickFmt, PALETTE};

/// How solid a band is. Enough to read as a region, light enough that a grid line and a
/// second band both survive underneath it.
const BAND_ALPHA: f64 = 0.18;

// ---------------------------------------------------------------- calendar

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse: civil `(year, month, day)` from days since 1970-01-01.
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// q's epoch is 2000.01.01, not the Unix one.
const Q_EPOCH_DAYS: i64 = 10_957;

// ---------------------------------------------------------------- ticks

/// The tick positions for `[lo, hi]`, plus the step (label precision depends on it).
fn nice_ticks(lo: f64, hi: f64, want: usize) -> (Vec<f64>, f64) {
    if !lo.is_finite() || !hi.is_finite() || hi <= lo {
        return (vec![lo], 1.0);
    }
    let raw = (hi - lo) / want.max(1) as f64;
    let mag = 10f64.powf(raw.log10().floor());
    let norm = raw / mag;
    // The classic 1/2/5/10 ladder: any other step produces labels nobody reads easily.
    let step = mag
        * if norm < 1.5 {
            1.0
        } else if norm < 3.0 {
            2.0
        } else if norm < 7.0 {
            5.0
        } else {
            10.0
        };
    let first = (lo / step).ceil() * step;
    let mut out = Vec::new();
    let mut t = first;
    while t <= hi + step * 1e-9 && out.len() < 64 {
        out.push(t);
        t += step;
    }
    if out.is_empty() {
        out.push(lo);
    }
    (out, step)
}

/// Calendar-aware ticks over a day range: whole days while the span is short, then month
/// starts, then year starts. Stepping a date axis by "30 days" drifts off the calendar and
/// the labels read as noise, which is the whole reason this exists.
fn date_ticks(lo: f64, hi: f64, want: usize) -> (Vec<f64>, DateStep) {
    let (l, h) = (lo.floor() as i64, hi.ceil() as i64);
    let span = (h - l).max(1);
    let per = span / want.max(1) as i64;
    if per <= 21 {
        let step = *[1i64, 2, 5, 7, 14, 21]
            .iter()
            .find(|s| **s >= per.max(1))
            .unwrap_or(&21);
        let first = l.div_euclid(step) * step;
        let ticks = (0..)
            .map(|i| first + i * step)
            .take_while(|d| *d <= h)
            .filter(|d| *d >= l)
            .take(64)
            .map(|d| d as f64)
            .collect();
        return (ticks, DateStep::Day);
    }
    let months = span / 30;
    if months / want.max(1) as i64 <= 12 {
        let mstep = *[1i64, 2, 3, 6, 12]
            .iter()
            .find(|s| **s >= (months / want.max(1) as i64).max(1))
            .unwrap_or(&12);
        let (y0, m0, _) = civil_from_days(l + Q_EPOCH_DAYS);
        let mut ticks = Vec::new();
        let mut idx = (y0 * 12 + m0 - 1) / mstep * mstep;
        while ticks.len() < 64 {
            let d = days_from_civil(idx / 12, idx % 12 + 1, 1) - Q_EPOCH_DAYS;
            if d > h {
                break;
            }
            if d >= l {
                ticks.push(d as f64);
            }
            idx += mstep;
        }
        return (
            ticks,
            if mstep == 12 {
                DateStep::Year
            } else {
                DateStep::Month
            },
        );
    }
    let years = (span / 365).max(1);
    let (ystep, _) = nice_ticks(0.0, years as f64, want);
    let ystep = (ystep.get(1).copied().unwrap_or(1.0) as i64).max(1);
    let (y0, ..) = civil_from_days(l + Q_EPOCH_DAYS);
    let mut ticks = Vec::new();
    let mut y = y0.div_euclid(ystep) * ystep;
    while ticks.len() < 64 {
        let d = days_from_civil(y, 1, 1) - Q_EPOCH_DAYS;
        if d > h {
            break;
        }
        if d >= l {
            ticks.push(d as f64);
        }
        y += ystep;
    }
    (ticks, DateStep::Year)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DateStep {
    Day,
    Month,
    Year,
}

fn fmt_date(v: f64, step: DateStep) -> String {
    let (y, m, d) = civil_from_days(v.round() as i64 + Q_EPOCH_DAYS);
    match step {
        DateStep::Day => format!("{y:04}.{m:02}.{d:02}"),
        DateStep::Month => format!("{y:04}.{m:02}"),
        DateStep::Year => format!("{y:04}"),
    }
}

/// `hh:mm:ss[.mmm]` from milliseconds since midnight, dropping fields the step can't resolve.
fn fmt_time_ms(ms: f64, step: f64) -> String {
    let t = ms.round() as i64;
    let (h, m, s, milli) = (t / 3_600_000, t / 60_000 % 60, t / 1000 % 60, t % 1000);
    if step < 1000.0 {
        format!("{h:02}:{m:02}:{s:02}.{milli:03}")
    } else if step < 60_000.0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h:02}:{m:02}")
    }
}

/// Decimals chosen from the tick step, so a 0.25 step gets two places and a 1000 step gets
/// none, because a fixed precision is either noisy or lossy at some scale.
fn fmt_num(v: f64, step: f64) -> String {
    if !v.is_finite() {
        return String::new();
    }
    // Ticks are generated as `first + k*step`, so the one that should be zero can land a few
    // ULPs off it, and the exponent branch below then prints `-2.78e-17` where the reader
    // expects `0`. Snap relative to the step, so genuinely tiny data is left alone.
    let v = if v.abs() < step.abs() * 1e-9 { 0.0 } else { v };
    let av = v.abs();
    if av != 0.0 && !(1e-4..1e7).contains(&av) {
        return format!("{v:.2e}");
    }
    let dec = if step >= 1.0 {
        0
    } else {
        (-step.log10().floor()).clamp(0.0, 6.0) as usize
    };
    let s = format!("{v:.dec$}");
    if s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

fn fmt_tick(v: f64, fmt: TickFmt, step: f64, dstep: DateStep, cats: &[String]) -> String {
    match fmt {
        TickFmt::Num => fmt_num(v, step),
        TickFmt::Date => fmt_date(v, dstep),
        TickFmt::Time => fmt_time_ms(v, step),
        TickFmt::Timestamp => {
            let day = (v / 86_400_000_000_000.0).floor();
            if step >= 86_400_000_000_000.0 {
                fmt_date(day, dstep)
            } else {
                fmt_time_ms((v - day * 86_400_000_000_000.0) / 1e6, step / 1e6)
            }
        }
        TickFmt::Cat => cats
            .get(v.round().max(0.0) as usize)
            .cloned()
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------- ranges

/// Finite min/max over an iterator, ignoring the NaNs q nulls arrive as.
fn bounds(vals: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    let mut any = false;
    for v in vals.filter(|v| v.is_finite()) {
        lo = lo.min(v);
        hi = hi.max(v);
        any = true;
    }
    any.then_some((lo, hi))
}

/// Widen a degenerate or zero-width range so the axis has somewhere to put ticks.
fn pad_range(lo: f64, hi: f64, frac: f64) -> (f64, f64) {
    if hi - lo <= f64::EPSILON.max(lo.abs() * 1e-12) {
        let d = if lo.abs() > 0.0 { lo.abs() * 0.1 } else { 1.0 };
        return (lo - d, hi + d);
    }
    let p = (hi - lo) * frac;
    (lo - p, hi + p)
}

/// Bin raw observations into `(centre, count)` pairs plus the bin width.
fn histogram(vals: &[f64], bins: usize) -> (Vec<(f64, f64)>, f64) {
    let Some((lo, hi)) = bounds(vals.iter().copied()) else {
        return (Vec::new(), 1.0);
    };
    let n = bins.clamp(1, 200);
    let w = if hi > lo { (hi - lo) / n as f64 } else { 1.0 };
    let mut counts = vec![0f64; n];
    for v in vals.iter().copied().filter(|v| v.is_finite()) {
        let i = (((v - lo) / w).floor() as isize).clamp(0, n as isize - 1) as usize;
        counts[i] += 1.0;
    }
    (
        counts
            .into_iter()
            .enumerate()
            .map(|(i, c)| (lo + (i as f64 + 0.5) * w, c))
            .collect(),
        w,
    )
}

// ---------------------------------------------------------------- draw

/// One series as a connected line. Shared by `Kind::Line` and by the overlay pass, so a
/// fitted model is drawn by exactly the same code as the data it is fitted to.
fn polyline(
    c: &mut dyn Canvas,
    ser: &Series,
    col: crate::Rgb,
    w: f64,
    sx: &impl Fn(f64) -> f64,
    sy: &impl Fn(f64) -> f64,
) {
    for (xs, ys) in ser.x.windows(2).zip(ser.y.windows(2)) {
        if xs.iter().chain(ys).all(|v| v.is_finite()) {
            c.line((sx(xs[0]), sy(ys[0])), (sx(xs[1]), sy(ys[1])), col, w);
        }
    }
    // A one-point series would otherwise draw nothing at all. Read BOTH sides through
    // `first`: x and y need not be the same length (`.plt.xy` takes them separately), and
    // indexing y here panicked on a 1-element x with no y.
    if ser.x.len() == 1 {
        if let (Some(x), Some(y)) = (ser.x.first(), ser.y.first()) {
            if x.is_finite() && y.is_finite() {
                c.marker((sx(*x), sy(*y)), w, col);
            }
        }
    }
}

/// The segment of `y = a + b*x` that lies within `x` in `[x0,x1]` AND `y` in `[y0,y1]`, or
/// `None` if none of it does. The two ranges are separate: x is bounded by the observations,
/// so the line is not mostly extrapolation, while y is bounded by the PANEL, so a steep fit
/// stops at its own frame instead of running across the panels beside it.
fn clip_fit(a: f64, b: f64, x0: f64, x1: f64, y0: f64, y1: f64) -> Option<((f64, f64), (f64, f64))> {
    let (mut xa, mut xb) = (x0, x1);
    if b.abs() > 1e-12 {
        let (u, v) = ((y0 - a) / b, (y1 - a) / b);
        xa = xa.max(u.min(v));
        xb = xb.min(u.max(v));
    } else if a < y0 || a > y1 {
        return None; // horizontal, and outside the box
    }
    if !xa.is_finite() || !xb.is_finite() || xb <= xa {
        return None;
    }
    Some(((xa, a + b * xa), (xb, a + b * xb)))
}

/// A scatter matrix: every series against every other, the diagonal a histogram of that
/// series. Separate from [`draw`] rather than a rect-parameterised version of it, because a
/// panel wants none of a plot's furniture (no per-panel ticks, title, axis labels or legend)
/// and suppressing all of that costs more than the ~90 lines here.
///
/// Every panel shares ONE scale. The columns are in the same units by construction (this is
/// a returns matrix), and free-scaling each panel makes a tight pair and a diffuse one fill
/// their boxes identically, which is the one comparison the chart exists to support.
fn draw_matrix(p: &Plot, c: &mut dyn Canvas) {
    let (w, h) = c.size();
    let (w, h) = (w as f64, h as f64);
    let th = p.theme;
    c.clear(th.bg);
    let n = p.series.len();
    if n == 0 {
        return;
    }
    let s = if p.font > 0 {
        p.font.clamp(1, 6)
    } else {
        ((w / 900.0).round() as u32).clamp(1, 3)
    };
    let pad = 8.0 * s as f64;
    let (_, glyph_h) = c.text_size("0", s);

    let mut top = pad;
    if !p.title.is_empty() {
        let t_h = c.text_size("X", s + 1).1;
        c.text(
            (w / 2.0, top + t_h / 2.0),
            &p.title,
            th.fg,
            TextStyle::new(Align::Middle, Align::Middle, s + 1),
        );
        top += t_h + pad;
    }

    let (lo, hi) = bounds(p.series.iter().flat_map(|se| se.y.iter().copied())).unwrap_or((0.0, 1.0));
    let (v0, v1) = pad_range(lo, hi, 0.06);
    // Panels are small and the labels repeat under every one of them, so this asks for far
    // fewer ticks than a single plot of the same total width would.
    let want = ((w / n as f64) / (glyph_h * 4.5)).clamp(2.0, 5.0) as usize;
    let (ticks, step) = nice_ticks(v0, v1, want);
    let labs: Vec<String> = ticks.iter().map(|v| fmt_num(*v, step)).collect();
    let tw = labs.iter().map(|l| c.text_size(l, s).0).fold(0.0, f64::max);

    // Ticks sit on the OUTER edge only. The scale is shared, so repeating them 2N times
    // would spend most of a small panel on labels that all say the same thing.
    let tick_len = 3.0 * s as f64;
    let left = pad + glyph_h + pad + tw + tick_len + 4.0;
    let bottom = pad + glyph_h + pad + glyph_h + tick_len + 4.0;
    let gut = 3.0 * s as f64;
    let gw = (w - left - pad).max(20.0);
    let gh = (h - top - bottom).max(20.0);
    let cw = (gw - gut * (n as f64 - 1.0)) / n as f64;
    let ch = (gh - gut * (n as f64 - 1.0)) / n as f64;
    let px = |j: usize| left + j as f64 * (cw + gut);
    let py = |i: usize| top + i as f64 * (ch + gut);

    let dim = th.bg.mix(th.fg, 0.55);
    let panel_bg = th.bg.mix(th.fg, 0.05);
    let r = (0.9 * s as f64).max(1.0);
    let room = cw > c.text_size("r=0.00", s).0 + 4.0 * s as f64;

    for i in 0..n {
        for j in 0..n {
            let (x0, y0) = (px(j), py(i));
            c.rect(x0, y0, cw, ch, panel_bg);
            let sx = |v: f64| x0 + (v - v0) / (v1 - v0) * cw;
            let sy = |v: f64| y0 + ch - (v - v0) / (v1 - v0) * ch;

            if i == j {
                // The diagonal is a variable against itself: a 45 degree line, and useless.
                // Its own distribution is what the space is worth spending on.
                let (hist, bw) = histogram(&p.series[i].y, p.bins);
                let peak = hist.iter().map(|(_, k)| *k).fold(1.0, f64::max);
                let base = hist.first().map_or(0.0, |(cx, _)| cx - bw / 2.0);
                // Each boundary from the bin INDEX, so bar k's right edge and bar k+1's left
                // edge are one number rather than two float paths to it and a lost separator.
                let edge = |k: usize| sx(base + k as f64 * bw).round();
                for (k, (_, count)) in hist.iter().enumerate() {
                    let bh = count / peak * ch * 0.88;
                    c.rect(
                        edge(k),
                        y0 + ch - bh,
                        (edge(k + 1) - edge(k) - 1.0).max(1.0),
                        bh,
                        PALETTE[0],
                    );
                }
            } else {
                // Returns straddle the origin and the quadrants are the point, so the zero
                // lines are drawn before the points rather than left to be inferred.
                if v0 < 0.0 && v1 > 0.0 {
                    c.line((x0, sy(0.0)), (x0 + cw, sy(0.0)), th.grid, 1.0);
                    c.line((sx(0.0), y0), (sx(0.0), y0 + ch), th.grid, 1.0);
                }
                let (ys, xs) = (&p.series[i].y, &p.series[j].y);
                for (xv, yv) in xs.iter().zip(ys) {
                    if xv.is_finite() && yv.is_finite() {
                        c.marker((sx(*xv), sy(*yv)), r, PALETTE[0]);
                    }
                }
                if let (Some(b), Some(a)) = (
                    p.beta.get(i).and_then(|row| row.get(j)),
                    p.alpha.get(i).and_then(|row| row.get(j)),
                ) {
                    // Only across the x values that were actually observed. Drawn to the
                    // panel edges instead, the line is mostly extrapolation, and it is the
                    // most visually dominant thing in the panel.
                    let (dx0, dx1) = bounds(xs.iter().copied()).unwrap_or((v0, v1));
                    if let Some((p0, p1)) = clip_fit(*a, *b, dx0.max(v0), dx1.min(v1), v0, v1) {
                        c.line((sx(p0.0), sy(p0.1)), (sx(p1.0), sy(p1.1)), PALETTE[1], 1.4 * s as f64);
                    }
                }
                if room {
                    if let Some(rv) = p.corr.get(i).and_then(|row| row.get(j)) {
                        if rv.is_finite() {
                            c.text(
                                (x0 + 3.0 * s as f64, y0 + 3.0 * s as f64 + glyph_h / 2.0),
                                &format!("r={rv:.2}"),
                                dim,
                                TextStyle::new(Align::Start, Align::Middle, s),
                            );
                        }
                    }
                }
            }
            // The frame last, so points and bars cannot sit on top of it.
            c.line((x0, y0), (x0 + cw, y0), th.axis, 1.0);
            c.line((x0, y0 + ch), (x0 + cw, y0 + ch), th.axis, 1.0);
            c.line((x0, y0), (x0, y0 + ch), th.axis, 1.0);
            c.line((x0 + cw, y0), (x0 + cw, y0 + ch), th.axis, 1.0);
        }
    }

    // Names on the outer edge, once per row and column; the left ones rotated so a long
    // ticker does not force the grid inward.
    for k in 0..n {
        let name = &p.series[k].name;
        c.text(
            (px(k) + cw / 2.0, h - pad - glyph_h / 2.0),
            name,
            th.fg,
            TextStyle::new(Align::Middle, Align::Middle, s),
        );
        c.text(
            (pad + glyph_h / 2.0, py(k) + ch / 2.0),
            name,
            th.fg,
            TextStyle::new(Align::Middle, Align::Middle, s).rotated(),
        );
    }
    // Every panel is on the same scale, so the ticks repeat per panel down the left column
    // and along the bottom row. Running one set across the whole grid instead would put
    // "0" wherever the grid happens to be half way across, which is nowhere in particular.
    let yb = py(n - 1) + ch;
    for (v, lab) in ticks.iter().zip(&labs) {
        let f = (v - v0) / (v1 - v0);
        for k in 0..n {
            let y = py(k) + ch - f * ch;
            c.line((left - tick_len, y), (left, y), th.axis, 1.0);
            c.text(
                (left - tick_len - 4.0, y),
                lab,
                th.fg,
                TextStyle::new(Align::End, Align::Middle, s),
            );
            let x = px(k) + f * cw;
            c.line((x, yb), (x, yb + tick_len), th.axis, 1.0);
            c.text(
                (x, yb + tick_len + 2.0 + glyph_h / 2.0),
                lab,
                th.fg,
                TextStyle::new(Align::Middle, Align::Middle, s),
            );
        }
    }
}

/// The translucent region between a series' `lo` and `hi`.
///
/// Emitted as one polygon per unbroken run rather than one for the whole series: a null has
/// to break a band the way it breaks a line, or the fill bridges a gap the data honestly
/// shows. The run is walked forwards along `lo` and back along `hi`, which is what makes the
/// ring close on itself without a diagonal across the middle.
fn band(
    c: &mut dyn Canvas,
    ser: &Series,
    col: crate::Rgb,
    sx: &impl Fn(f64) -> f64,
    sy: &impl Fn(f64) -> f64,
) {
    let n = ser.lo.len().min(ser.hi.len()).min(ser.x.len());
    let ok = |i: usize| ser.lo[i].is_finite() && ser.hi[i].is_finite() && ser.x[i].is_finite();
    let mut i = 0;
    while i < n {
        if !ok(i) {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && ok(i) {
            i += 1;
        }
        // A single point has no area, and a degenerate polygon is not worth a call.
        if i - start < 2 {
            continue;
        }
        let mut pts: Vec<(f64, f64)> = (start..i).map(|k| (sx(ser.x[k]), sy(ser.lo[k]))).collect();
        pts.extend((start..i).rev().map(|k| (sx(ser.x[k]), sy(ser.hi[k]))));
        c.polygon(&pts, col, BAND_ALPHA);
    }
}

pub fn draw(p: &Plot, c: &mut dyn Canvas) {
    // A matrix is a grid of panels rather than one axis rect, and shares none of the
    // furniture below.
    if p.kind == Kind::Matrix {
        return draw_matrix(p, c);
    }
    let (w, h) = c.size();
    let (w, h) = (w as f64, h as f64);
    let th = p.theme;
    c.clear(th.bg);

    // One integer scale drives every glyph and gap, so a HiDPI plot is the same design at
    // twice the size rather than the same design with tiny text. It comes from the CALLER
    // (the terminal's cell height); the width-derived fallback is for standalone rendering
    // where there is no terminal to ask.
    let s = if p.font > 0 {
        p.font.clamp(1, 6)
    } else {
        ((w / 900.0).round() as u32).clamp(1, 3)
    };
    let pad = 8.0 * s as f64;
    let (_, glyph_h) = c.text_size("0", s);

    // The series the plot's OWN kind draws, keeping each one's original index so its colour
    // does not shift when an overlay is added. Overlays are drawn afterwards, as lines.
    let main: Vec<(usize, &Series)> = p
        .series
        .iter()
        .enumerate()
        .filter(|(_, se)| !se.overlay)
        .collect();

    // Histogram bars are derived, not given: the caller passes raw observations.
    let (hist, hist_w) = if p.kind == Kind::Hist {
        let vals: Vec<f64> = main.first().map(|(_, se)| se.y.clone()).unwrap_or_default();
        histogram(&vals, p.bins)
    } else {
        (Vec::new(), 1.0)
    };

    // ---- data ranges
    let (mut x0, mut x1, mut y0, mut y1) = match p.kind {
        Kind::Hist => {
            let (a, b) = bounds(hist.iter().map(|(x, _)| *x)).unwrap_or((0.0, 1.0));
            let (_, top) = bounds(hist.iter().map(|(_, y)| *y)).unwrap_or((0.0, 1.0));
            (a - hist_w / 2.0, b + hist_w / 2.0, 0.0, top)
        }
        Kind::Bar => {
            let n = main.first().map_or(0, |(_, se)| se.y.len());
            let (lo, hi) =
                bounds(p.series.iter().flat_map(|s| s.y.iter().copied())).unwrap_or((0.0, 1.0));
            (-0.5, n as f64 - 0.5, lo, hi)
        }
        // A bar chart on its side: the values run along x and the rows down y.
        Kind::HBar => {
            let n = main.first().map_or(0, |(_, se)| se.y.len());
            let (lo, hi) =
                bounds(p.series.iter().flat_map(|s| s.y.iter().copied())).unwrap_or((0.0, 1.0));
            (lo, hi, -0.5, n as f64 - 0.5)
        }
        _ => {
            let (a, b) =
                bounds(p.series.iter().flat_map(|s| s.x.iter().copied())).unwrap_or((0.0, 1.0));
            // A band joins the y range. Left out, a band wider than the data is clipped at
            // the axis, which is exactly when its width is the thing worth seeing.
            let (lo, hi) = bounds(p.series.iter().flat_map(|s| {
                s.y.iter().chain(s.lo.iter()).chain(s.hi.iter()).copied()
            }))
            .unwrap_or((0.0, 1.0));
            (a, b, lo, hi)
        }
    };
    if p.kind == Kind::Candle {
        // Half a slot of x headroom each side, or the first and last bodies hang over the
        // axis. Bars get this for free from their -0.5 .. n-0.5 range.
        let n = main.first().map_or(1, |(_, se)| se.x.len()).max(1) as f64;
        let slot = (x1 - x0) / n;
        let (lo, hi) = pad_range(y0, y1, 0.06);
        (x0, x1, y0, y1) = (x0 - slot / 2.0, x1 + slot / 2.0, lo, hi);
    } else if matches!(p.kind, Kind::Line | Kind::Scatter | Kind::Bands) {
        let (a, b) = pad_range(x0, x1, 0.0);
        let (lo, hi) = pad_range(y0, y1, 0.06);
        (x0, x1, y0, y1) = (a, b, lo, hi);
    } else {
        // Bars are measured FROM zero, so the axis has to contain it, and both ends need
        // headroom independently. Padding only the top leaves a negative bar running out of
        // the plot rect and over the x tick labels.
        let from_zero = |a: f64, b: f64| {
            let (mut lo, mut hi) = (a.min(0.0), b.max(0.0));
            if hi <= lo {
                hi = lo + 1.0;
            }
            let room = (hi - lo) * 0.08;
            if lo < 0.0 {
                lo -= room;
            }
            if hi > 0.0 {
                hi += room;
            }
            (lo, hi)
        };
        if p.kind == Kind::HBar {
            (x0, x1) = from_zero(x0, x1);
        } else {
            (y0, y1) = from_zero(y0, y1);
        }
    }

    // ---- ticks (before margins: the labels decide how much room the axes need)
    // An hbar's categories sit on y, one per row, top row first, subsampled by the height
    // the labels need; its x is then numeric whatever the spec's `xfmt` said.
    let hbar_rows = if p.kind == Kind::HBar {
        main.first().map_or(0, |(_, se)| se.y.len())
    } else {
        0
    };
    let xfmt = if p.kind == Kind::HBar { TickFmt::Num } else { p.xfmt };
    // A keyed hbar colours each bar by its group. Only with ONE series: several already
    // take a colour each, and two colour codes on one bar would say nothing.
    let by_group = p.kind == Kind::HBar && !p.groups.is_empty() && main.len() == 1;
    let mut group_names: Vec<&str> = Vec::new();
    let group_of: Vec<usize> = p
        .groups
        .iter()
        .map(|g| {
            group_names.iter().position(|n| *n == g).unwrap_or_else(|| {
                group_names.push(g);
                group_names.len() - 1
            })
        })
        .collect();
    let (yt, ylab): (Vec<f64>, Vec<String>) = if p.kind == Kind::HBar {
        let want = ((h * 0.8 / (glyph_h * 1.5)) as usize).max(1);
        let every = hbar_rows.div_ceil(want).max(1);
        let rows: Vec<usize> = (0..hbar_rows).step_by(every).collect();
        (
            rows.iter().map(|j| (hbar_rows - 1 - j) as f64).collect(),
            rows.iter()
                .map(|j| p.xcats.get(*j).cloned().unwrap_or_else(|| j.to_string()))
                .collect(),
        )
    } else {
        let (yt, ystep) = nice_ticks(y0, y1, (h / (glyph_h * 3.0)).clamp(2.0, 10.0) as usize);
        let ylab = yt
            .iter()
            .map(|v| fmt_tick(*v, p.yfmt, ystep, DateStep::Day, &[]))
            .collect();
        (yt, ylab)
    };
    let (xt, xstep, xdstep) = match xfmt {
        TickFmt::Cat => {
            // Subsample from the width the labels ACTUALLY need. A guess based on the
            // glyph box drops labels that would have fitted comfortably.
            let n = p.xcats.len().max(1);
            let widest = p
                .xcats
                .iter()
                .map(|l| c.text_size(l, s).0)
                .fold(glyph_h, f64::max);
            let want = ((w * 0.85 / (widest + 8.0 * s as f64)) as usize).max(1);
            let every = n.div_ceil(want).max(1);
            (
                (0..n).step_by(every).map(|i| i as f64).collect(),
                1.0,
                DateStep::Day,
            )
        }
        TickFmt::Date => {
            let (t, d) = date_ticks(x0, x1, (w / (glyph_h * 7.0)).clamp(2.0, 12.0) as usize);
            (t, 1.0, d)
        }
        _ => {
            let (t, st) = nice_ticks(x0, x1, (w / (glyph_h * 7.0)).clamp(2.0, 12.0) as usize);
            (t, st, DateStep::Day)
        }
    };
    let xlab: Vec<String> = xt
        .iter()
        .map(|v| fmt_tick(*v, xfmt, xstep, xdstep, &p.xcats))
        .collect();
    let ytw = ylab.iter().map(|l| c.text_size(l, s).0).fold(0.0, f64::max);

    // ---- plot rect
    let tick_len = 4.0 * s as f64;
    let left =
        pad + if p.ylabel.is_empty() {
            0.0
        } else {
            glyph_h + pad
        } + ytw
            + tick_len
            + 4.0;
    let bottom =
        pad + if p.xlabel.is_empty() {
            0.0
        } else {
            glyph_h + pad
        } + glyph_h
            + tick_len
            + 4.0;
    let top = pad
        + if p.title.is_empty() {
            0.0
        } else {
            c.text_size("X", s + 1).1 + pad
        };
    let right = pad + xlab.last().map_or(0.0, |l| c.text_size(l, s).0 / 2.0);
    let (px, py) = (left, top);
    let (pw, ph) = ((w - left - right).max(20.0), (h - top - bottom).max(20.0));

    let sx = |v: f64| px + (v - x0) / (x1 - x0) * pw;
    let sy = |v: f64| py + ph - (v - y0) / (y1 - y0) * ph;

    // ---- grid and ticks
    for (v, lab) in yt.iter().zip(&ylab) {
        let y = sy(*v);
        c.line((px, y), (px + pw, y), th.grid, 1.0);
        c.line((px - tick_len, y), (px, y), th.axis, 1.0);
        c.text(
            (px - tick_len - 4.0, y),
            lab,
            th.fg,
            TextStyle::new(Align::End, Align::Middle, s),
        );
    }
    for (v, lab) in xt.iter().zip(&xlab) {
        let x = sx(*v);
        c.line((x, py), (x, py + ph), th.grid, 1.0);
        c.line((x, py + ph), (x, py + ph + tick_len), th.axis, 1.0);
        c.text(
            (x, py + ph + tick_len + 4.0),
            lab,
            th.fg,
            TextStyle::new(Align::Middle, Align::Start, s),
        );
    }
    // A zero line, when zero is inside the range and isn't already the axis.
    if y0 < 0.0 && y1 > 0.0 {
        c.line((px, sy(0.0)), (px + pw, sy(0.0)), th.axis, 1.0);
    }
    if p.kind == Kind::HBar && x0 < 0.0 && x1 > 0.0 {
        c.line((sx(0.0), py), (sx(0.0), py + ph), th.axis, 1.0);
    }
    c.line((px, py), (px, py + ph), th.axis, 1.0);
    c.line((px, py + ph), (px + pw, py + ph), th.axis, 1.0);

    // ---- series
    let line_w = 1.6 * s as f64;
    #[allow(clippy::match_same_arms)]
    match p.kind {
        // Handled above, before any of this ran.
        Kind::Matrix => {}
        Kind::Line => {
            for (i, ser) in main.iter() {
                polyline(c, ser, PALETTE[i % PALETTE.len()], line_w, &sx, &sy);
            }
        }
        // Fills first, then the lines through them: a band over its own line is a smear.
        Kind::Bands => {
            for (i, ser) in main.iter() {
                let col = ser.colour.unwrap_or(PALETTE[i % PALETTE.len()]);
                band(c, ser, col, &sx, &sy);
            }
            for (i, ser) in main.iter() {
                let col = ser.colour.unwrap_or(PALETTE[i % PALETTE.len()]);
                polyline(c, ser, col, line_w, &sx, &sy);
            }
        }
        Kind::Scatter => {
            let r = (1.8 * s as f64).max(1.5);
            for (i, ser) in main.iter() {
                let col = PALETTE[i % PALETTE.len()];
                for (x, y) in ser.x.iter().zip(&ser.y) {
                    if x.is_finite() && y.is_finite() {
                        c.marker((sx(*x), sy(*y)), r, col);
                    }
                }
            }
        }
        Kind::Bar => {
            let n = main.len().max(1);
            let slot = pw / (x1 - x0);
            let bw = slot * 0.8 / n as f64;
            for (i, ser) in main.iter() {
                let col = PALETTE[i % PALETTE.len()];
                for (j, y) in ser.y.iter().enumerate() {
                    if !y.is_finite() {
                        continue;
                    }
                    let cx = sx(j as f64) - slot * 0.4 + bw * *i as f64;
                    let (top, base) = (sy(*y), sy(0.0));
                    c.rect(cx, top.min(base), bw, (base - top).abs().max(1.0), col);
                }
            }
        }
        Kind::HBar => {
            let n = main.len().max(1);
            let slot = ph / (y1 - y0);
            let bh = slot * 0.8 / n as f64;
            for (i, ser) in main.iter() {
                let col = PALETTE[i % PALETTE.len()];
                for (j, v) in ser.y.iter().enumerate() {
                    if !v.is_finite() || j >= hbar_rows {
                        continue;
                    }
                    let col = match group_of.get(j) {
                        Some(g) if by_group => PALETTE[g % PALETTE.len()],
                        _ => col,
                    };
                    // Row 0 at the top, so the chart reads in the table's own order.
                    let cy = sy((hbar_rows - 1 - j) as f64) - slot * 0.4 + bh * *i as f64;
                    let (end, base) = (sx(*v), sx(0.0));
                    c.rect(end.min(base), cy, (end - base).abs().max(1.0), bh, col);
                }
            }
        }
        Kind::Candle => {
            // Four positional series: open, high, low, close. `plt.q` arranges them by column
            // name, so anything short of four is a caller that bypassed it.
            if let [(_, o), (_, h), (_, l), (_, cl)] = &main[..] {
                let n = o.x.len().max(1);
                let bw = (pw / n as f64 * 0.7).max(1.0);
                let wick = (0.9 * s as f64).max(1.0);
                for i in 0..n {
                    let (Some(&xv), Some(&ov), Some(&hv), Some(&lv), Some(&cv)) =
                        (o.x.get(i), o.y.get(i), h.y.get(i), l.y.get(i), cl.y.get(i))
                    else {
                        continue;
                    };
                    if ![xv, ov, hv, lv, cv].iter().all(|v| v.is_finite()) {
                        continue;
                    }
                    let up = cv >= ov;
                    let col = if up { th.up } else { th.down };
                    let x = sx(xv);
                    c.line((x, sy(lv)), (x, sy(hv)), col, wick);
                    let (top, bot) = (sy(ov.max(cv)), sy(ov.min(cv)));
                    // A doji (open == close) has no body to fill, so it reads as a bare tick.
                    if (bot - top).abs() < wick {
                        c.line((x - bw / 2.0, top), (x + bw / 2.0, top), col, wick);
                    } else if up {
                        // Hollow: four edges, so the up/down cue is not colour alone.
                        let (x0b, x1b) = (x - bw / 2.0, x + bw / 2.0);
                        for (a, b) in [
                            ((x0b, top), (x1b, top)),
                            ((x0b, bot), (x1b, bot)),
                            ((x0b, top), (x0b, bot)),
                            ((x1b, top), (x1b, bot)),
                        ] {
                            c.line(a, b, col, wick);
                        }
                    } else {
                        c.rect(x - bw / 2.0, top, bw, bot - top, col);
                    }
                }
            }
        }
        Kind::Hist => {
            let col = PALETTE[0];
            // Each boundary is computed ONCE, from the bin index, so bar i's right edge and
            // bar i+1's left edge are the same number rather than two float paths to it.
            // Deriving a width instead (`slot - 1`, centred) lets the two edges round
            // independently, and that division lands an ULP light, 20.374999999999996 for
            // a slot of 21.375, so at some sizes a pair rounds together and two bars fuse
            // with no separator between them.
            let lo = hist.first().map_or(0.0, |(c, _)| c - hist_w / 2.0);
            let edge = |i: usize| sx(lo + i as f64 * hist_w).round();
            for (i, (_, count)) in hist.iter().enumerate() {
                let (left, right) = (edge(i), edge(i + 1));
                let (top, base) = (sy(*count), sy(0.0));
                // A pixel off the right edge is the separator from the next bar.
                c.rect(
                    left,
                    top,
                    (right - left - 1.0).max(1.0),
                    (base - top).max(0.0),
                    col,
                );
            }
        }
    }

    // ---- titles
    if !p.title.is_empty() {
        c.text(
            (w / 2.0, pad),
            &p.title,
            th.fg,
            TextStyle::new(Align::Middle, Align::Start, s + 1),
        );
    }
    if !p.xlabel.is_empty() {
        c.text(
            (px + pw / 2.0, h - pad),
            &p.xlabel,
            th.fg,
            TextStyle::new(Align::Middle, Align::End, s),
        );
    }
    if !p.ylabel.is_empty() {
        // Rotated and centred on the plot rect, which is where a reader looks for it. The
        // left margin already reserves a glyph-height-wide column for this label, and that
        // is exactly the width a rotated string needs.
        c.text(
            (pad, py + ph / 2.0),
            &p.ylabel,
            th.fg,
            TextStyle::new(Align::Start, Align::Middle, s).rotated(),
        );
    }

    // ---- overlays, on top of whatever the kind drew: a fitted line over its sample.
    for (i, ser) in p.series.iter().enumerate().filter(|(_, se)| se.overlay) {
        polyline(c, ser, PALETTE[i % PALETTE.len()], line_w, &sx, &sy);
    }

    // ---- legend
    let named: Vec<(usize, &Series)> = p
        .series
        .iter()
        .enumerate()
        .filter(|(_, s)| !s.name.is_empty())
        .collect();
    // What each key row shows: the series' own colour so an entry cannot disagree with what
    // was drawn, and a swatch shaped like the series rather than a hairline for everything.
    // A band's is a translucent block; a bar's a solid one; a scatter's a marker.
    let entries: Vec<(&str, crate::Rgb, Option<&Series>)> = if by_group {
        // The groups repeat (cold, warm, cold, warm) and are worth a key. Distinct on every
        // row, they only restate the labels already down the left.
        if group_names.len() < p.groups.len() {
            group_names
                .iter()
                .enumerate()
                .map(|(g, n)| (*n, PALETTE[g % PALETTE.len()], None))
                .collect()
        } else {
            Vec::new()
        }
    } else if p.kind == Kind::Candle
        || !(named.len() > 1 || (named.len() == 1 && p.series.len() > 1))
    {
        // A candlestick's four series are one instrument, not four things to tell apart,
        // and listing open/high/low/close in a key explains nothing and covers the data.
        Vec::new()
    } else {
        named
            .iter()
            .map(|(i, se)| (se.name.as_str(), se.colour.unwrap_or(PALETTE[i % PALETTE.len()]), Some(*se)))
            .collect()
    };
    if !entries.is_empty() {
        let lh = glyph_h + 4.0 * s as f64;
        let chip = 14.0 * s as f64;
        let tw = entries
            .iter()
            .map(|(n, ..)| c.text_size(n, s).0)
            .fold(0.0, f64::max);
        let bw = chip + 6.0 * s as f64 + tw + 2.0 * pad;
        let bh = lh * entries.len() as f64 + pad;
        // Put the box in whichever corner the data uses least. A fixed corner sat on top
        // of the series often enough to matter: a rising line and a top-right legend are
        // the single most common chart there is.
        let corners = [
            (px + pw - bw - pad, py + pad),
            (px + pad, py + pad),
            (px + pw - bw - pad, py + ph - bh - pad),
            (px + pad, py + ph - bh - pad),
        ];
        let occupancy = |bx: f64, by: f64| {
            p.series
                .iter()
                .flat_map(|se| se.x.iter().zip(&se.y))
                .filter(|(x, y)| x.is_finite() && y.is_finite())
                .filter(|(x, y)| {
                    let (dx, dy) = (sx(**x), sy(**y));
                    dx >= bx - pad && dx <= bx + bw + pad && dy >= by - pad && dy <= by + bh + pad
                })
                .count()
        };
        let (bx, by) = corners
            .into_iter()
            .min_by_key(|(bx, by)| occupancy(*bx, *by))
            .expect("four corners");
        c.rect(bx, by, bw, bh, th.bg.mix(th.fg, 0.08));
        for (row, (name, col, se)) in entries.iter().enumerate() {
            let col = *col;
            let cy = by + pad / 2.0 + lh * row as f64 + lh / 2.0;
            if let Some(se) = se.filter(|se| !se.lo.is_empty()) {
                c.polygon(
                    &[
                        (bx + pad, cy - lh * 0.3),
                        (bx + pad + chip, cy - lh * 0.3),
                        (bx + pad + chip, cy + lh * 0.3),
                        (bx + pad, cy + lh * 0.3),
                    ],
                    col,
                    BAND_ALPHA * 3.0,
                );
                if !se.y.is_empty() {
                    c.line((bx + pad, cy), (bx + pad + chip, cy), col, line_w);
                }
            } else {
                match if se.is_some_and(|se| se.overlay) { Kind::Line } else { p.kind } {
                    Kind::Bar | Kind::HBar | Kind::Hist => {
                        c.rect(bx + pad, cy - lh * 0.25, chip, lh * 0.5, col)
                    }
                    Kind::Scatter => {
                        c.marker((bx + pad + chip / 2.0, cy), (1.8 * s as f64).max(1.5), col)
                    }
                    _ => c.line((bx + pad, cy), (bx + pad + chip, cy), col, line_w),
                }
            }
            c.text(
                (bx + pad + chip + 6.0 * s as f64, cy),
                name,
                th.fg,
                TextStyle::new(Align::Start, Align::Middle, s),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canvas::Rgb;

    #[test]
    fn civil_roundtrip() {
        for (y, m, d) in [
            (2000, 1, 1),
            (1970, 1, 1),
            (2026, 8, 21),
            (1999, 12, 31),
            (2024, 2, 29),
        ] {
            let z = days_from_civil(y, m, d);
            assert_eq!(civil_from_days(z), (y, m, d), "{y}.{m}.{d}");
        }
        // q's epoch: date 0 is 2000.01.01.
        assert_eq!(civil_from_days(Q_EPOCH_DAYS), (2000, 1, 1));
    }

    #[test]
    fn ticks_are_round_numbers_inside_the_range() {
        let (t, step) = nice_ticks(0.0, 10.0, 5);
        assert_eq!(step, 2.0);
        assert_eq!(t, vec![0.0, 2.0, 4.0, 6.0, 8.0, 10.0]);
        for (lo, hi) in [(-3.7, 12.1), (0.0, 1e-3), (1e6, 1.0000005e6)] {
            let (t, _) = nice_ticks(lo, hi, 6);
            assert!(!t.is_empty());
            assert!(
                t.iter().all(|v| *v >= lo - 1e-9 && *v <= hi + 1e-9),
                "{lo}..{hi} -> {t:?}"
            );
        }
        // A zero-width range must still terminate rather than spin.
        let (t, _) = nice_ticks(5.0, 5.0, 6);
        assert_eq!(t.len(), 1);
    }

    /// A tick that should be zero must print as `0`. Accumulated `first + k*step` puts it a
    /// few ULPs out, and the small-value branch then rendered `-2.78e-17` on any axis that
    /// straddles zero: every signed bar chart and most histograms.
    #[test]
    fn a_tick_at_zero_prints_as_zero() {
        // "0.0" rather than "0": the decimals come from the step, so it matches the
        // -0.4/-0.3/0.1 labels sharing the axis.
        assert_eq!(fmt_num(-2.775557561562891e-17, 0.1), "0.0");
        assert_eq!(fmt_num(0.0, 0.1), "0.0");
        assert_eq!(fmt_num(-2.8e-17, 1.0), "0");
        // Real data near zero is not clobbered: the snap is relative to the step.
        assert_eq!(fmt_num(1e-17, 1e-16), "1.00e-17");
        assert_eq!(fmt_num(-0.1, 0.1), "-0.1");
    }

    #[test]
    fn date_ticks_snap_to_the_calendar() {
        // Two years of daily data: month-or-year steps, each landing on a 1st.
        let lo = (days_from_civil(2024, 1, 1) - Q_EPOCH_DAYS) as f64;
        let hi = (days_from_civil(2026, 1, 1) - Q_EPOCH_DAYS) as f64;
        let (t, step) = date_ticks(lo, hi, 6);
        assert!(step != DateStep::Day);
        assert!(!t.is_empty());
        for v in &t {
            let (_, _, d) = civil_from_days(*v as i64 + Q_EPOCH_DAYS);
            assert_eq!(d, 1, "tick {} is not a month start", fmt_date(*v, step));
        }
        // A two-week window steps in whole days instead.
        let (t, step) = date_ticks(lo, lo + 14.0, 5);
        assert_eq!(step, DateStep::Day);
        assert!(t.len() >= 3);
    }

    #[test]
    fn histogram_conserves_count() {
        let vals: Vec<f64> = (0..100).map(|i| i as f64 / 10.0).collect();
        let (bins, w) = histogram(&vals, 10);
        assert_eq!(bins.len(), 10);
        assert!(w > 0.0);
        assert_eq!(bins.iter().map(|(_, c)| c).sum::<f64>(), 100.0);
    }

    /// Every kind renders without panicking and puts ink on the canvas. The cheapest
    /// guard against an empty-series or degenerate-range crash reaching the REPL.
    #[test]
    fn every_kind_draws() {
        for kind in [Kind::Line, Kind::Scatter, Kind::Bar, Kind::HBar, Kind::Hist] {
            let p = Plot {
                kind,
                series: vec![Series {
                    name: "a".into(),
                    x: (0..50).map(|i| i as f64).collect(),
                    y: (0..50).map(|i| (i as f64 / 5.0).sin()).collect(),
                    overlay: false,
                    ..Default::default()
                }],
                title: "t".into(),
                xlabel: "x".into(),
                ylabel: "y".into(),
                ..Default::default()
            };
            let r = p.raster();
            assert!(r.bytes().iter().any(|b| *b > 60), "{kind:?} drew nothing");
        }
    }

    /// The fit segment must stay inside the panel on BOTH axes: bounded in x by the
    /// observations, so the line is not mostly extrapolation, and in y by the panel, so a
    /// steep one stops at its own frame instead of running across its neighbours.
    #[test]
    fn a_fit_never_leaves_its_panel() {
        for (a, b) in [(0.0, 1.0), (0.0, 40.0), (-5.0, -12.0), (3.0, 0.0), (0.0, -0.001)] {
            if let Some((p0, p1)) = clip_fit(a, b, -2.0, 2.0, -1.0, 1.0) {
                for (x, y) in [p0, p1] {
                    assert!((-2.0 - 1e-9..=2.0 + 1e-9).contains(&x), "x {x} for {a}+{b}x");
                    assert!((-1.0 - 1e-9..=1.0 + 1e-9).contains(&y), "y {y} for {a}+{b}x");
                }
            }
        }
        // Entirely outside on y, in both the sloped and the horizontal case.
        assert!(clip_fit(50.0, 0.0, -2.0, 2.0, -1.0, 1.0).is_none());
        assert!(clip_fit(50.0, 1.0, -2.0, 2.0, -1.0, 1.0).is_none());
        // A vertical-ish fit still yields a segment rather than NaN endpoints.
        let (p0, p1) = clip_fit(0.0, 1e9, -2.0, 2.0, -1.0, 1.0).expect("steep but present");
        assert!(p0.0.is_finite() && p1.1.is_finite());
    }

    /// A matrix draws N x N panels in one image, and none of the degenerate shapes that
    /// reach it from q (a constant column with no slope, a single row, absent fits) may
    /// panic or leave the canvas blank.
    #[test]
    fn a_matrix_draws_every_panel() {
        let col = |k: usize| Series {
            name: format!("c{k}"),
            x: Vec::new(),
            y: (0..40).map(|i| ((i * (k + 1)) % 7) as f64).collect(),
            overlay: false,
            ..Default::default()
        };
        let n = 3;
        let fit = vec![vec![1.0; n]; n];
        for (series, beta) in [
            ((0..n).map(col).collect::<Vec<_>>(), fit.clone()),
            ((0..n).map(col).collect(), Vec::new()), // fit_line:0b
            (vec![col(0), Series { y: vec![2.0; 40], ..col(1) }], vec![vec![0.0; 2]; 2]),
            (vec![Series { y: vec![1.0], ..col(0) }, col(1)], vec![vec![0.0; 2]; 2]),
        ] {
            let p = Plot {
                kind: Kind::Matrix,
                title: "m".into(),
                corr: vec![vec![0.5; series.len()]; series.len()],
                alpha: vec![vec![0.0; series.len()]; series.len()],
                beta,
                series,
                width: 400,
                height: 400,
                ..Default::default()
            };
            let r = p.raster();
            assert!(r.bytes().iter().any(|b| *b > 60), "matrix drew nothing");
        }
    }

    /// A bar chart's axis must contain the NEGATIVE extreme too. It didn't: the range was
    /// built from the maximum alone, so a negative bar was drawn below the plot rect and
    /// straight over the x tick labels. Assert no bar-coloured pixel reaches the bottom
    /// margin, which is exactly what the reader saw go wrong.
    #[test]
    fn negative_bars_stay_inside_the_plot() {
        let p = Plot {
            kind: Kind::Bar,
            series: vec![Series {
                name: "v".into(),
                x: vec![],
                y: vec![5.0, -3.0, 8.0, -6.0],
                overlay: false,
                ..Default::default()
            }],
            xcats: ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect(),
            xfmt: TickFmt::Cat,
            width: 400,
            height: 300,
            ..Default::default()
        };
        let r = p.raster();
        let bar = PALETTE[0];
        let px = |x: usize, y: usize| {
            let i = (y * 400 + x) * 3;
            Rgb(r.bytes()[i], r.bytes()[i + 1], r.bytes()[i + 2])
        };
        // The bottom rows are tick labels and the x axis label, so no series ink belongs there.
        for y in 280..300 {
            for x in 0..400 {
                assert_ne!(px(x, y), bar, "bar ink at ({x},{y}), below the plot rect");
            }
        }
        // ...and the bars really were drawn, so this isn't passing by rendering nothing.
        assert!(
            (0..300).any(|y| (0..400).any(|x| px(x, y) == bar)),
            "no bars drawn at all"
        );
    }

    /// An hbar reads like the table it came from: the first row is the TOP bar, and a bar
    /// runs from zero along x. Checked on the pixels, since the row order is the one thing
    /// a transposed bar chart can get wrong without any test noticing.
    #[test]
    fn hbar_rows_read_top_down() {
        let p = Plot {
            kind: Kind::HBar,
            series: vec![Series {
                name: "v".into(),
                x: vec![],
                y: vec![10.0, 1.0],
                overlay: false,
                ..Default::default()
            }],
            xcats: vec!["long".into(), "short".into()],
            xfmt: TickFmt::Cat,
            width: 400,
            height: 200,
            ..Default::default()
        };
        let r = p.raster();
        let bar = PALETTE[0];
        let run = |y: usize| {
            (0..400)
                .filter(|x| {
                    let i = (y * 400 + x) * 3;
                    Rgb(r.bytes()[i], r.bytes()[i + 1], r.bytes()[i + 2]) == bar
                })
                .count()
        };
        let widths: Vec<usize> = (0..200).map(run).filter(|n| *n > 0).collect();
        assert!(widths.len() > 2, "no bars drawn");
        assert!(widths[0] > 5 * widths[widths.len() - 1], "row 0 is not the long bar on top: {widths:?}");
    }

    /// A keyed hbar colours each bar by its group, so cold and warm rows come out in two
    /// palette colours rather than one.
    #[test]
    fn hbar_groups_colour_the_bars() {
        let p = Plot {
            kind: Kind::HBar,
            series: vec![Series {
                name: "v".into(),
                x: vec![],
                y: vec![5.0, 5.0, 5.0, 5.0],
                overlay: false,
                ..Default::default()
            }],
            xcats: ["a", "b", "c", "d"].iter().map(|s| s.to_string()).collect(),
            groups: ["cold", "warm", "cold", "warm"].iter().map(|s| s.to_string()).collect(),
            xfmt: TickFmt::Cat,
            width: 400,
            height: 200,
            ..Default::default()
        };
        let r = p.raster();
        let count = |col: Rgb| {
            r.bytes()
                .chunks(3)
                .filter(|px| Rgb(px[0], px[1], px[2]) == col)
                .count()
        };
        assert!(count(PALETTE[0]) > 100, "no bars in the first colour");
        assert!(count(PALETTE[1]) > 100, "no bars in the second colour");
    }

    /// A panic here takes the whole session down, so mismatched and empty series must be
    /// merely uninteresting. `.plt.xy` takes x and y as separate arguments, so nothing
    /// guarantees they are the same length.
    /// Every histogram bar must be separated from its neighbour. The bar width is the slot
    /// minus one pixel, which looks like it guarantees a gap, but both edges are rounded
    /// independently, and for some slot widths the pair rounds together and two bars fuse.
    #[test]
    fn histogram_bars_never_fuse() {
        let obs: Vec<f64> = (0..400).map(|i| ((i * 37 % 101) as f64) / 101.0).collect();
        for (w, bins) in [(900, 40), (640, 30), (820, 24), (400, 17), (1200, 53)] {
            let p = Plot {
                kind: Kind::Hist,
                series: vec![Series {
                    name: String::new(),
                    x: vec![],
                    y: obs.clone(),
                    overlay: false,
                    ..Default::default()
                }],
                width: w,
                height: 300,
                bins,
                ..Default::default()
            };
            let r = p.raster();
            let bar = PALETTE[0];
            // Scan the row just above the axis, where every bar is present.
            let y = 250usize;
            let lit: Vec<bool> = (0..w as usize)
                .map(|x| {
                    let i = (y * w as usize + x) * 3;
                    Rgb(r.bytes()[i], r.bytes()[i + 1], r.bytes()[i + 2]) == bar
                })
                .collect();
            let mut runs = Vec::new();
            let (mut start, mut n) = (0usize, 0usize);
            for (x, on) in lit.iter().enumerate() {
                if *on {
                    if n == 0 {
                        start = x;
                    }
                    n += 1;
                } else if n > 0 {
                    runs.push((start, n));
                    n = 0;
                }
            }
            if n > 0 {
                runs.push((start, n));
            }
            let widest = runs.iter().map(|(_, n)| *n).max().unwrap_or(0);
            let narrowest = runs.iter().map(|(_, n)| *n).min().unwrap_or(0);
            assert!(runs.len() > 1, "{w}x{bins}: no bars drawn");
            assert!(
                widest <= narrowest + 1,
                "{w}px/{bins} bins: a run of {widest} against a bar of {narrowest}, bars fused"
            );
        }
    }

    #[test]
    fn degenerate_inputs_do_not_panic() {
        let s = |x: Vec<f64>, y: Vec<f64>| Series {
            name: "n".into(),
            x,
            y,
            overlay: false,
            ..Default::default()
        };
        for series in [
            vec![],
            vec![Series::default()],
            vec![s(vec![1.0], vec![f64::NAN])],
            vec![s(vec![1.0, 1.0], vec![2.0, 2.0])],
            vec![s(vec![1.0], vec![])], // 1-element x, no y, used to panic
            vec![s(vec![], vec![1.0, 2.0])],
            vec![s(vec![1.0, 2.0, 3.0], vec![5.0])],
            vec![s(vec![f64::NAN, f64::NAN], vec![f64::NAN, f64::NAN])],
            vec![s(vec![f64::INFINITY], vec![f64::NEG_INFINITY])],
        ] {
            for kind in [Kind::Line, Kind::Scatter, Kind::Bar, Kind::HBar, Kind::Hist] {
                let _ = Plot {
                    kind,
                    series: series.clone(),
                    ..Default::default()
                }
                .raster();
            }
        }
    }
}
