//! The kdb+ `k.h` C ABI: `` `:libjplots 2:(`draw;1) `` and the spec-dict reader behind it.
//!
//! This is the only file that knows what a `K` is. `q/plt.q` normalises whatever you passed
//! (a table, a keyed table, a dict, a vector) into one spec dictionary, and everything
//! here turns that dictionary into a [`Plot`]. The renderer below it is host-agnostic, which
//! is what lets any host share it.
//!
//! **Hazard.** A wrong field offset or a misread type code corrupts memory rather than
//! failing a test, and it does so inside the host process. Every read below checks the type
//! code first and returns a default rather than reinterpreting bytes on trust. The layout is
//! pinned to `k.h` (KXVER>=3) and exercised against real kdb+ in `q/test.q`.

#![allow(non_camel_case_types)]

use crate::{kitty, Kind, Plot, Series, Theme, TickFmt};
use std::ffi::{c_char, CStr};

#[repr(C)]
pub struct K0 {
    pub m: i8,
    pub a: i8,
    /// Type code: negative = atom, positive = vector, 0 = general list, 99 = dict.
    pub t: i8,
    pub u: i8,
    pub r: i32,
    /// Vector length, or, for an atom, the payload reinterpreted per type.
    pub n: i64,
}
pub type K = *mut K0;

/// `sizeof(struct k0)`; vector data begins here.
const HDR: usize = 16;

// Type codes we read. `k.h` names in comments for grep-ability.
const KB: i8 = 1; // boolean
const KJ: i8 = 7; // long
const KF: i8 = 9; // float
const KC: i8 = 10; // char
const KS: i8 = 11; // symbol
const XD: i8 = 99; // dict
const NIL: i8 = 101; // generic null (`::`)

// Atom codes are the negated vector codes, but a negated const is not a valid Rust pattern,
// so the ones we match on get their own names.
const KJ_ATOM: i8 = -KJ;
const KF_ATOM: i8 = -KF;
const KS_ATOM: i8 = -KS;

// Resolved from the host executable at dlopen time (q is linked with its symbols exported).
extern "C" {
    fn ka(t: i32) -> K;
    fn krr(s: *const c_char) -> K;
}

// ---------------------------------------------------------------- reading

unsafe fn items(x: K) -> &'static [K] {
    if x.is_null() || (*x).t != 0 {
        return &[];
    }
    std::slice::from_raw_parts((x as *const u8).add(HDR) as *const K, (*x).n as usize)
}

unsafe fn floats(x: K) -> Vec<f64> {
    match (*x).t {
        KF => std::slice::from_raw_parts((x as *const u8).add(HDR) as *const f64, (*x).n as usize)
            .to_vec(),
        // A long vector is accepted so `.plt.size:800 400` needs no cast in q.
        KJ => std::slice::from_raw_parts((x as *const u8).add(HDR) as *const i64, (*x).n as usize)
            .iter()
            .map(|v| *v as f64)
            .collect(),
        KF_ATOM => vec![f64::from_bits((*x).n as u64)],
        KJ_ATOM => vec![(*x).n as f64],
        // Booleans, so `overlay` reads as the `01b` a q caller would naturally write.
        KB => std::slice::from_raw_parts((x as *const u8).add(HDR), (*x).n as usize)
            .iter()
            .map(|v| *v as f64)
            .collect(),
        _ => Vec::new(),
    }
}

unsafe fn sym(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

/// A symbol atom, a char vector, or the first of either as a list: the shapes a q caller
/// reaches for when they mean "a string".
unsafe fn text(x: K) -> String {
    match (*x).t {
        KS_ATOM => sym((*x).n as *const c_char),
        KC => String::from_utf8_lossy(std::slice::from_raw_parts(
            (x as *const u8).add(HDR),
            (*x).n as usize,
        ))
        .into_owned(),
        KS if (*x).n > 0 => sym(*((x as *const u8).add(HDR) as *const *const c_char)),
        _ => String::new(),
    }
}

/// A symbol vector or a list of char vectors. Series names and categorical tick labels
/// arrive as either, depending on whether the q side had symbols or strings.
unsafe fn strings(x: K) -> Vec<String> {
    match (*x).t {
        KS => std::slice::from_raw_parts(
            (x as *const u8).add(HDR) as *const *const c_char,
            (*x).n as usize,
        )
        .iter()
        .map(|p| sym(*p))
        .collect(),
        0 => items(x).iter().map(|i| text(*i)).collect(),
        KS_ATOM | KC => vec![text(x)],
        _ => Vec::new(),
    }
}

/// One vector, or a general list of them (multi-series `y`).
unsafe fn series_data(x: K) -> Vec<Vec<f64>> {
    if (*x).t == 0 {
        items(x).iter().map(|i| floats(*i)).collect()
    } else {
        vec![floats(x)]
    }
}

/// A dict's `(keys; values)` pair. `k.h` stores them as two `K` pointers at the data offset,
/// but the dict's own type code is 99 rather than 0, so [`items`], which insists on a general
/// list, must not be used here. Getting that wrong reads the pair as empty and every field
/// silently goes missing.
unsafe fn dict_kv(x: K) -> Option<(K, K)> {
    if x.is_null() || (*x).t != XD {
        return None;
    }
    let p = (x as *const u8).add(HDR) as *const K;
    Some((*p, *p.add(1)))
}

/// The spec dictionary's entry for `name`, or `None`.
unsafe fn field(spec: K, name: &str) -> Option<K> {
    let (keys, vals) = dict_kv(spec)?;
    let names = strings(keys);
    let vals = items(vals);
    names
        .iter()
        .position(|k| k == name)
        .and_then(|i| vals.get(i).copied())
}

unsafe fn field_text(spec: K, name: &str) -> String {
    field(spec, name).map_or_else(String::new, |v| text(v))
}

unsafe fn field_nums(spec: K, name: &str) -> Vec<f64> {
    field(spec, name).map_or_else(Vec::new, |v| floats(v))
}

// ---------------------------------------------------------------- spec -> Plot

/// Build a [`Plot`] from the spec dictionary `q/plt.q` produces.
///
/// Every field is optional and every unreadable field falls back to a default, because a
/// half-drawn chart is a better failure than a crash inside the host. The one hard error is
/// a missing `y`, which means there is nothing to draw at all.
///
/// # Safety
/// `spec` must be a live `K` dictionary owned by the caller for the duration of the call.
pub unsafe fn plot_from_spec(spec: K) -> Result<Plot, String> {
    if spec.is_null() || (*spec).t != XD {
        return Err("plt.draw: expected a spec dictionary".into());
    }
    let kind = Kind::from_name(&field_text(spec, "kind"))
        .ok_or_else(|| "plt.draw: unknown kind".to_string())?;
    let xfmt = TickFmt::from_name(&field_text(spec, "xfmt"))
        .ok_or_else(|| "plt.draw: unknown xfmt".to_string())?;
    let ys = match field(spec, "y") {
        Some(y) => series_data(y),
        None => return Err("plt.draw: spec needs a `y".into()),
    };
    // `x` is one vector shared by every series, or a list of them, one per series, for
    // clusters that each carry their own x, like close-against-vwap per instrument.
    let xs = field(spec, "x").map_or_else(Vec::new, |v| series_data(v));
    let names = field(spec, "names").map_or_else(Vec::new, |v| strings(v));
    // Per-series flag: 1b means "draw as a line over the others" rather than as `kind`.
    let over = field_nums(spec, "overlay");
    // A band's edges, one vector per series and empty for a series that is a plain line, plus
    // the explicit colours from `.plt.bands`' style dict.
    let lo = field(spec, "lo").map_or_else(Vec::new, |v| series_data(v));
    let hi = field(spec, "hi").map_or_else(Vec::new, |v| series_data(v));
    let cols = field(spec, "colours").map_or_else(Vec::new, |v| strings(v));
    let series = ys
        .into_iter()
        .enumerate()
        .map(|(i, y)| Series {
            name: names.get(i).cloned().unwrap_or_default(),
            // An absent or empty x is the row index: a one-column table has no column left
            // to be x, and the q side sends `()` for it.
            x: match xs.get(i.min(xs.len().saturating_sub(1))) {
                Some(v) if !v.is_empty() => v.clone(),
                _ => (0..y.len()).map(|j| j as f64).collect(),
            },
            y,
            overlay: over.get(i).is_some_and(|v| *v != 0.0),
            lo: lo.get(i).cloned().unwrap_or_default(),
            hi: hi.get(i).cloned().unwrap_or_default(),
            colour: cols.get(i).and_then(|s| crate::Rgb::from_hex(s)),
        })
        .collect();

    // Geometry: the terminal decides, unless the spec overrides. `plt.q` merges the
    // `.plt.*` session settings into the spec before calling, so this is the only source.
    let (width, height, font) = crate::resolve_geometry(
        &field_nums(spec, "size"),
        field_nums(spec, "scale").first().copied(),
        field_nums(spec, "font").first().copied(),
    );

    Ok(Plot {
        kind,
        series,
        title: field_text(spec, "title"),
        xlabel: field_text(spec, "xlabel"),
        ylabel: field_text(spec, "ylabel"),
        width,
        height,
        xfmt,
        yfmt: TickFmt::Num,
        xcats: field(spec, "xcats").map_or_else(Vec::new, |v| strings(v)),
        sixel_cursor: crate::sixel::Cursor::from_name(&field_text(spec, "sixel_cursor"))?,
        // N x N fits for a scatter matrix, computed in q so the caller gets the same numbers
        // back as a table. `series_data` already reads a list of vectors.
        beta: field(spec, "beta").map_or_else(Vec::new, |v| series_data(v)),
        alpha: field(spec, "alpha").map_or_else(Vec::new, |v| series_data(v)),
        corr: field(spec, "corr").map_or_else(Vec::new, |v| series_data(v)),
        groups: field(spec, "groups").map_or_else(Vec::new, |v| strings(v)),
        bins: field_nums(spec, "bins")
            .first()
            .copied()
            .filter(|f| f.is_finite() && *f > 0.0)
            .map_or(30, |f| f as usize),
        theme: Theme::from_name(&field_text(spec, "theme")),
        font,
    })
}

/// The escape-byte stream for a spec: the whole render, without touching the terminal.
/// Split out so a caller can compare the bytes rather than look at them.
///
/// # Safety
/// As [`plot_from_spec`].
pub unsafe fn encode_spec(spec: K) -> Result<Vec<u8>, String> {
    let plot = plot_from_spec(spec)?;
    Ok(plot.encode(crate::Backend::from_name(&field_text(spec, "renderer"))?))
}

// ---------------------------------------------------------------- the q entry point

/// `` .plt.draw:`:libjplots 2:(`draw;1) ``: render the spec and write it to stdout.
///
/// Returns `::`. Escape sequences are not a value anyone wants back, and returning one would
/// make `.plt.line t` echo several hundred kilobytes at the console.
///
/// Written with `write(2)` rather than Rust's buffered stdout so the bytes land immediately
/// and in order relative to whatever the host printed before them.
///
/// # Safety
/// Called by q through the `2:` ABI with a single `K` argument.
#[no_mangle]
pub unsafe extern "C" fn draw(spec: K) -> K {
    match encode_spec(spec) {
        Ok(bytes) => {
            let mut off = 0usize;
            while off < bytes.len() {
                let n = libc::write(
                    1,
                    bytes.as_ptr().add(off) as *const libc::c_void,
                    bytes.len() - off,
                );
                if n <= 0 {
                    break;
                }
                off += n as usize;
            }
            nil()
        }
        Err(e) => {
            // `krr` signals a q error from the message, which is what a q caller expects
            // (trappable with `@`/`.Q.trp`) rather than a silent no-op.
            let msg = std::ffi::CString::new(e).unwrap_or_default();
            // The string must outlive the call: q reads it after we return.
            let leaked = Box::leak(msg.into_boxed_c_str());
            krr(leaked.as_ptr())
        }
    }
}

/// `.plt.info[]`: what the library detected about the terminal, as a q dict, so a chart
/// that looks soft can be diagnosed without decoding an image.
///
/// # Safety
/// Called by q through the `2:` ABI.
#[no_mangle]
pub unsafe extern "C" fn info(_ignored: K) -> K {
    let m = kitty::metrics();
    let (cw, ch) = m.cell();
    // `width`/`height` are the size the NEXT plot will use, which is the reason to look at
    // this at all when a chart comes out soft. They were reported by one host bridge and not
    // the other until this was noticed; the keys have to match or `.plt.info[]` means
    // something different depending on where it runs.
    let (dw, dh) = m.default_plot();
    let vals = [
        m.cols as i64,
        m.rows as i64,
        m.xpix as i64,
        m.ypix as i64,
        cw as i64,
        ch as i64,
        dw as i64,
        dh as i64,
        m.font_scale() as i64,
        i64::from(kitty::in_tmux()),
        i64::from(kitty::looks_supported()),
    ];
    // Built through the same k.h allocators q uses, so the result is an ordinary q dict.
    let keys = ktn_syms(&[
        "cols", "rows", "xpix", "ypix", "cellw", "cellh", "width", "height", "font", "tmux",
        "kitty",
    ]);
    let values = ktn_longs(&vals);
    xd(keys, values)
}

/// `::`, the generic null.
unsafe fn nil() -> K {
    let r = ka(NIL as i32);
    if !r.is_null() {
        (*r).n = 0;
    }
    r
}

// The variadic k.h builders (`knk`, `k`) cannot be declared in stable Rust, but the
// fixed-arity ones can, and they are all this needs.
extern "C" {
    fn ktn(t: i32, n: i64) -> K;
    fn ss(s: *const c_char) -> *mut c_char;
    fn xD(k: K, v: K) -> K;
}

unsafe fn ktn_syms(names: &[&str]) -> K {
    let r = ktn(KS as i32, names.len() as i64);
    let slot = (r as *mut u8).add(HDR) as *mut *mut c_char;
    for (i, n) in names.iter().enumerate() {
        let c = std::ffi::CString::new(*n).unwrap_or_default();
        *slot.add(i) = ss(c.as_ptr());
    }
    r
}

unsafe fn ktn_longs(vals: &[i64]) -> K {
    let r = ktn(KJ as i32, vals.len() as i64);
    let slot = (r as *mut u8).add(HDR) as *mut i64;
    for (i, v) in vals.iter().enumerate() {
        *slot.add(i) = *v;
    }
    r
}

unsafe fn xd(k: K, v: K) -> K {
    xD(k, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The struct layout is the whole safety story: a wrong offset reinterprets bytes inside
    /// the host process. `k.h` fixes `sizeof(struct k0)` at 16 with the payload at offset 8.
    #[test]
    fn k0_layout_matches_kh() {
        assert_eq!(std::mem::size_of::<K0>(), HDR);
        assert_eq!(std::mem::align_of::<K0>(), 8);
        let k = K0 {
            m: 0,
            a: 0,
            t: 0,
            u: 0,
            r: 0,
            n: 0,
        };
        let base = &k as *const K0 as usize;
        assert_eq!(&k.t as *const i8 as usize - base, 2, "type code at byte 2");
        assert_eq!(&k.r as *const i32 as usize - base, 4, "refcount at byte 4");
        assert_eq!(&k.n as *const i64 as usize - base, 8, "payload at byte 8");
    }

    /// Reading is driven by type codes, and an unexpected one must yield a default rather
    /// than reinterpreting the bytes. These run without a q host, so they build a `K0`
    /// header directly and check the guards fire.
    #[test]
    fn wrong_type_codes_read_as_empty() {
        unsafe {
            let mut atom = K0 {
                m: 0,
                a: 0,
                t: KJ_ATOM,
                u: 0,
                r: 0,
                n: 42,
            };
            let k: K = &mut atom;
            assert_eq!(floats(k), vec![42.0], "a long atom widens");
            assert!(items(k).is_empty(), "an atom is not a general list");
            assert!(strings(k).is_empty(), "a long is not text");
            assert!(field(k, "y").is_none(), "an atom is not a dict");
            assert!(dict_kv(k).is_none(), "nor does it have a key/value pair");
        }
    }
}
