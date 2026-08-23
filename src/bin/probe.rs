//! `jplots-probe`: draw a kitty graphics test pattern, several ways, and say what was sent.
//!
//! Run it in the terminal you are diagnosing. Which frames appear names the piece of the
//! protocol that terminal rejects, because each variant adds exactly one thing to the one
//! before it and the last is what jplots itself sends.

use jplots::{kitty, probe};
use std::io::Write;

const USAGE: &str = "\
jplots-probe: a kitty graphics test pattern

    jplots-probe                 every variant
    jplots-probe -n 3            only variant 3
    jplots-probe -s 400x240      a different size
    jplots-probe --sixel         the same pattern as sixel, for terminals that prefer it
    jplots-probe --palette       sixel frames using 4..256 colours, to find a palette limit
    jplots-probe --cursor        one sixel frame per cursor strategy, each followed by a rule
    jplots-probe --raw > out.esc the bytes, for utils/decode.py (or desixel.py)

Each frame carries its own number, a ruler down two edges, colour bands, a greyscale ramp
and four corner markers. Report which numbers rendered, and for a partial one, the ruler
value it stopped at.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |names: [&str; 2]| -> Option<String> {
        args.iter()
            .position(|a| names.contains(&a.as_str()))
            .and_then(|i| args.get(i + 1).cloned())
    };
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return;
    }
    let only: Option<usize> = flag(["-n", "--variant"]).and_then(|v| v.parse().ok());
    let (w, h) = flag(["-s", "--size"])
        .and_then(|v| {
            let (a, b) = v.split_once(['x', 'X'])?;
            Some((a.parse().ok()?, b.parse().ok()?))
        })
        .unwrap_or((480u32, 300u32));
    let raw = args.iter().any(|a| a == "--raw");
    let as_sixel = args.iter().any(|a| a == "--sixel");
    let as_palette = args.iter().any(|a| a == "--palette");
    let as_cursor = args.iter().any(|a| a == "--cursor");

    let m = kitty::metrics();
    let tmux = kitty::in_tmux();
    let out = std::io::stdout();
    let mut out = out.lock();

    if !raw {
        // To stderr, so `--raw`-style redirection of the image never mixes with the notes,
        // and so this is still readable when stdout is a pipe.
        eprintln!(
            "terminal: {}x{} cells, {}x{} px{}   tmux passthrough: {}",
            m.cols,
            m.rows,
            m.xpix,
            m.ypix,
            if m.xpix == 0 { " (not reported, assuming 8x16 cells)" } else { "" },
            if tmux { "yes, needs `tmux set -g allow-passthrough all`" } else { "no" },
        );
        eprintln!("image: {w}x{h}");

        // Five frames rarely fit, and a terminal that loses images when the screen scrolls
        // then looks like a terminal that rejects the later VARIANTS. That misreading has
        // already happened once, so say it before it can happen again.
        let (_, ch) = m.cell();
        let per = (h as f64 / ch).ceil() as u16 + 3;
        let total = per * probe::variants().len() as u16;
        if only.is_none() && !as_sixel && !as_palette && !as_cursor && total > m.rows {
            eprintln!(
                "\nNOTE: {} frames need about {} rows and this terminal has {}. The screen will\n\
                 scroll, and some terminals drop an image when it does, which looks exactly like\n\
                 a variant being rejected. Run them one at a time instead:\n\
                 \n    for n in 1 2 3 4 5; do clear; jplots-probe -n $n -s {}x{}; read; done\n",
                probe::variants().len(),
                total,
                m.rows,
                w,
                (m.rows.saturating_sub(4) as f64 * ch) as u32,
            );
        }
        eprintln!();
    }

    // Where a terminal leaves the cursor after a sixel image, which nothing in the protocol
    // says and which differs enough between terminals to be worth 60px of screen to settle.
    // Each frame is followed IMMEDIATELY by a rule, so the blank between image and rule is
    // the strategy's error, in lines, readable without counting anything else.
    if as_cursor {
        use jplots::sixel::Cursor;
        // Named as `.plt.sixel_cursor` takes them, so a result here is the setting to use.
        let strategies = [
            (Cursor::Reserve, "`reserve  reserve, draw, terminal advances  (the default)"),
            (Cursor::Advance, "`advance  reserve, draw, descend ourselves  (WezTerm)"),
            (Cursor::Bare, "`bare     draw only, no reserve"),
            (Cursor::Newline, "`newline  draw, then one newline"),
        ];
        let (pw, ph) = (w.min(360), 60u32);
        for (i, (c, label)) in strategies.iter().enumerate() {
            let rgb = probe::palette_frame(pw, ph, 6, m.font_scale());
            let _ = writeln!(out, "{}  {label}", i + 1);
            let _ = out.write_all(&jplots::sixel::place(&rgb, pw, ph, m, *c));
            let _ = writeln!(out, "---- end {} ----", i + 1);
            let _ = out.flush();
        }
        if !raw {
            eprintln!(
                "\nThe blank between each image and its `---- end N ----` rule is that\n\
                 strategy's error. The one with no gap and no overlap is the right one here."
            );
        }
        return;
    }

    // Colour count, which the main pattern cannot test: it uses six, and a chart uses up to
    // 256. A terminal with fewer registers than it claims draws this probe perfectly and a
    // chart as mud.
    if as_palette {
        for n in probe::PALETTE_STEPS {
            let rgb = probe::palette_frame(w, h.min(120), n, m.font_scale());
            let bytes = jplots::sixel::encode(&rgb, w, h.min(120), m);
            if !raw {
                eprintln!("{n:>4} colours   {} bytes", bytes.len());
            }
            let _ = out.write_all(&bytes);
            let _ = out.flush();
        }
        if !raw {
            eprintln!(
                "\nA smooth ramp in every frame means the palette is fine. The first frame that\n\
                 goes flat, banded or muddy is past what this terminal really provides."
            );
        }
        return;
    }

    // Sixel has none of the kitty protocol's optional pieces to vary, so there is one frame
    // and the only question it answers is whether the terminal draws sixel at all.
    if as_sixel {
        let rgb = probe::pattern(w, h, 1, "sixel", m.font_scale());
        if !raw {
            eprintln!("1  sixel (DCS, 256-colour palette)");
        }
        let bytes = jplots::sixel::encode(&rgb, w, h, m);
        if !raw {
            eprintln!("   {} bytes on the wire", bytes.len());
        }
        let _ = out.write_all(&bytes);
        let _ = out.flush();
        return;
    }

    for (i, v) in probe::variants().iter().enumerate() {
        let n = i + 1;
        if only.is_some_and(|k| k != n) {
            continue;
        }
        let bytes = probe::frame(v, n, w, h, m, tmux);
        if !raw {
            let escapes = bytes.windows(3).filter(|s| s == b"\x1b_G").count();
            eprintln!("{n}  {} ({})", v.label, v.note);
            eprintln!("   {} escape(s), {} bytes on the wire", escapes, bytes.len());
        }
        let _ = out.write_all(&bytes);
        let _ = out.flush();
    }

    if !raw {
        eprintln!(
            "\nAll {} rendered? The protocol is fine here.\nOne missing, or short: that \
             variant's line above names what it rejected.",
            probe::variants().len()
        );
    }
}
