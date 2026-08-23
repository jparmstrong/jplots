//! The kitty graphics protocol: RGB pixels → APC escape sequences on stdout.
//!
//! Supported by kitty, ghostty, Konsole and WezTerm. Three details are what make it work
//! in practice rather than in theory:
//!
//! * **`q=2`.** Without it the terminal replies `OK` on stdin after every image, and those
//!   replies land in the REPL's next read.
//! * **`o=z`.** Charts are mostly flat colour, so zlib takes a 900×480 frame from ~1.3 MB
//!   to tens of kilobytes. Over ssh that is the difference between instant and visible.
//! * **tmux passthrough.** Each escape has to be wrapped in `ESC Ptmux; … ESC \` with its
//!   inner escapes doubled, and the user needs `set -g allow-passthrough all`, because `on`
//!   forwards only for a pane tmux considers visible. Transport is all this buys: tmux has
//!   no record of an image, so its next repaint of the pane erases the chart. Surviving that
//!   needs the Unicode placeholder mode, which is not implemented.

use base64::{engine::general_purpose::STANDARD, Engine};
use flate2::{write::ZlibEncoder, Compression};
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};

/// How many previously drawn charts to leave in the terminal's store.
///
/// A transmitted image lives in the terminal until something frees it, and nothing did, so a
/// long session accumulated a frame per plot forever. That is worth bounding on its own.
///
/// It is NOT worth tuning. An earlier version bounded pixel BYTES against a budget calibrated
/// to a terminal I believed was refusing images after three. It was not: they had scrolled off
/// a short window, and the tight budget bought nothing while costing the scrollback a reader
/// would actually use. No terminal is known to need any of this. A plain count is enough for
/// the leak, and it is what makes the id ring provably safe: ids cycle through [`SLOTS`], so
/// the live set has to stay below it, or a new image takes an id the terminal is still
/// showing something under.
const KEEP: usize = 32;

/// The id range this library allocates from. Ids are a shared namespace with anything else
/// drawing to the same terminal, so this starts somewhere nothing else is likely to pick.
const ID_BASE: u32 = 0x6a70_0000;

/// Ids per process. Twice [`KEEP`], so a slot is reused long after the image in it was freed
/// rather than one plot after.
const SLOTS: u32 = 64;

/// This process's slice of the id range.
///
/// A FIXED base is wrong, and wrong in a way that looks like a terminal bug: every process
/// starts its counter at zero, so two q sessions in one terminal, or two runs of the probe,
/// transmit with the same ids. Transmitting over an existing id REPLACES it, and a replaced
/// image takes its earlier placement with it, so the previous chart vanishes as the next one
/// appears. Derived from the pid rather than a counter because the collision to avoid is
/// between processes, which share nothing else.
fn id_base() -> u32 {
    static BASE: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *BASE.get_or_init(|| {
        let pid = std::process::id();
        ID_BASE + ((pid ^ (pid >> 16)) & 0xffff) * SLOTS
    })
}

/// Transmissions so far, this process. The id has to change per image or the terminal
/// replaces the previous one, and a replaced image takes its earlier placement with it: the
/// chart you scrolled back to would silently become the newest chart.
static SENT: AtomicU32 = AtomicU32::new(0);

/// What this process believes the terminal is still holding: `(id, pixel bytes)`, oldest
/// first. The terminal has no way to be asked, so this is a model of it rather than a reading.
static LIVE: std::sync::Mutex<std::collections::VecDeque<(u32, usize)>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());

/// Take an id for an image of `bytes`, and return the ids to free to make room for it.
///
/// Split out from [`encode_with`] so it can be tested against a state it owns, rather than
/// through a process-wide counter that every other test in the binary would race.
fn admit(
    live: &mut std::collections::VecDeque<(u32, usize)>,
    base: u32,
    seq: u32,
    bytes: usize,
) -> (u32, Vec<u32>) {
    let id = base + seq % SLOTS;
    let mut freed = Vec::new();
    while live.len() >= KEEP {
        if let Some((old, _)) = live.pop_front() {
            freed.push(old);
        }
    }
    live.push_back((id, bytes));
    (id, freed)
}

/// Payload bytes per APC chunk: the protocol's documented maximum.
const CHUNK: usize = 4096;

/// Terminal geometry: character cells, and the pixel size of the text area when the
/// terminal reports it (`ws_xpixel`/`ws_ypixel` are 0 on plenty of setups, notably inside
/// tmux and over some ssh paths).
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub cols: u16,
    pub rows: u16,
    pub xpix: u16,
    pub ypix: u16,
}

impl Metrics {
    /// Pixels per character cell.
    ///
    /// Getting this RIGHT is what makes a plot sharp. The image is sized in cells (`c`/`r`),
    /// so if the pixel size we render at doesn't match the cell box the terminal rescales it
    /// and the result is soft. `ws_xpixel`/`ws_ypixel` are zero inside tmux and over plenty
    /// of ssh paths, so a wrong guess there is the normal case, not the edge case. Hence
    /// the `CSI 16 t` query in [`metrics`], and 8x16 only as a last resort.
    pub fn cell(&self) -> (f64, f64) {
        let cw = if self.xpix > 0 && self.cols > 0 {
            self.xpix as f64 / self.cols as f64
        } else {
            8.0
        };
        let ch = if self.ypix > 0 && self.rows > 0 {
            self.ypix as f64 / self.rows as f64
        } else {
            16.0
        };
        (cw.max(1.0), ch.max(1.0))
    }

    /// A default plot size in pixels: most of the terminal's width, half its height.
    pub fn default_plot(&self) -> (u32, u32) {
        let (cw, ch) = self.cell();
        let w = (self.cols.max(20) as f64 * cw * 0.94) as u32;
        let h = (self.rows.max(10) as f64 * ch * 0.55) as u32;
        (w.clamp(320, 8192), h.clamp(200, 8192))
    }

    /// The integer glyph scale that makes chart labels about the size of the terminal's OWN
    /// text, the only reference the reader actually has. Keying this to the image width
    /// instead (the first thing I tried) makes a bigger plot grow its fonts, which is
    /// backwards: a wider chart wants MORE labels, not larger ones.
    pub fn font_scale(&self) -> u32 {
        let (_, ch) = self.cell();
        ((ch * 0.72 / crate::font::H as f64).round() as u32).clamp(1, 4)
    }
}

impl Default for Metrics {
    fn default() -> Metrics {
        Metrics {
            cols: 100,
            rows: 30,
            xpix: 0,
            ypix: 0,
        }
    }
}

/// Ask the terminal for its cell size with `CSI 16 t`, for when `TIOCGWINSZ` reports no
/// pixels. Answers look like `ESC [ 6 ; <height> ; <width> t`.
///
/// This reads stdin in raw mode with a 200 ms ceiling, so it is done ONCE per process and
/// cached: a plot must not cost a round-trip every time, and must not hang when the
/// terminal ignores the query (many do). Type-ahead typed during that window is consumed,
/// which is the same bargain every terminal-capability query makes.
fn query_cell_pixels() -> Option<(u16, u16)> {
    use std::io::{IsTerminal, Read, Write};
    use std::os::fd::AsRawFd;
    if !std::io::stdout().is_terminal() {
        return None;
    }
    let fd = std::io::stdin().as_raw_fd();
    let mut saved: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: both calls take a valid fd and a properly sized `termios`; a non-zero return
    // means nothing was changed, and we bail without having touched the terminal.
    if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
        return None;
    }
    let mut raw = saved;
    unsafe { libc::cfmakeraw(&mut raw) };
    raw.c_cc[libc::VMIN] = 0;
    raw.c_cc[libc::VTIME] = 2; // deciseconds
    if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
        return None;
    }
    let mut out = std::io::stdout();
    let _ = out.write_all(b"\x1b[16t");
    let _ = out.flush();
    let mut buf = [0u8; 64];
    let n = std::io::stdin().read(&mut buf).unwrap_or(0);
    // Restore before doing anything else: leaving the tty raw would wreck the REPL.
    unsafe { libc::tcsetattr(fd, libc::TCSANOW, &saved) };

    let reply = std::str::from_utf8(&buf[..n]).ok()?;
    let body = reply.split("\x1b[").find_map(|p| p.strip_prefix("6;"))?;
    let body = body.split('t').next()?;
    let (h, w) = body.split_once(';')?;
    let (h, w) = (h.trim().parse().ok()?, w.trim().parse().ok()?);
    (h > 0 && w > 0).then_some((w, h))
}

/// Query the controlling terminal. Falls back to a sane 100×30 when stdout is not a tty
/// (a piped script run, a test), so plotting never depends on being interactive.
pub fn metrics() -> Metrics {
    #[repr(C)]
    #[derive(Default)]
    struct Winsize {
        row: libc::c_ushort,
        col: libc::c_ushort,
        xpixel: libc::c_ushort,
        ypixel: libc::c_ushort,
    }
    let mut ws = Winsize::default();
    // SAFETY: TIOCGWINSZ writes exactly a `winsize` through the pointer; a failed call
    // leaves it at the zeroed default, which the `> 0` checks above already handle.
    let rc = unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) };
    if rc != 0 || ws.col == 0 {
        return Metrics::default();
    }
    let mut m = Metrics {
        cols: ws.col,
        rows: ws.row.max(1),
        xpix: ws.xpixel,
        ypix: ws.ypixel,
    };
    if m.xpix == 0 || m.ypix == 0 {
        use std::sync::OnceLock;
        static CELL: OnceLock<Option<(u16, u16)>> = OnceLock::new();
        if let Some((cw, ch)) = *CELL.get_or_init(query_cell_pixels) {
            m.xpix = cw.saturating_mul(m.cols);
            m.ypix = ch.saturating_mul(m.rows);
        }
    }
    m
}

/// Whether the terminal looks like it speaks the protocol, from the environment alone.
///
/// This is a HINT, not a handshake: `.plt.renderer` is the actual switch, so a wrong answer
/// here costs nothing today. The real detection, an `a=q` probe and a timed raw-mode read
/// (which also works through tmux and over ssh), belongs with the text fallback backend,
/// because only then is there somewhere to fall back TO.
pub fn looks_supported() -> bool {
    let e = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    if e("KITTY_WINDOW_ID").is_some()
        || e("GHOSTTY_RESOURCES_DIR").is_some()
        || e("GHOSTTY_BIN_DIR").is_some()
    {
        return true;
    }
    let term = e("TERM").unwrap_or_default();
    let prog = e("TERM_PROGRAM").unwrap_or_default().to_ascii_lowercase();
    term.contains("kitty")
        || term.contains("ghostty")
        || prog.contains("ghostty")
        || prog.contains("wezterm")
        || e("KONSOLE_VERSION").is_some()
}

/// Whether we are inside tmux, and so have to wrap every escape for passthrough.
pub fn in_tmux() -> bool {
    std::env::var("TMUX").is_ok_and(|v| !v.is_empty())
}

/// Wrap one escape sequence for tmux's passthrough (`allow-passthrough all`), doubling the
/// inner ESCs as the DCS payload requires.
fn passthrough(seq: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1bPtmux;");
    for b in seq {
        if *b == 0x1b {
            out.push(0x1b);
        }
        out.push(*b);
    }
    out.extend_from_slice(b"\x1b\\");
}

/// Which of the protocol's optional pieces to use. Terminals differ in what they accept, so
/// [`crate::probe`] varies these one at a time to find where one stops rendering; nothing
/// else has a reason to change them.
#[derive(Clone, Copy, Debug)]
pub struct Wire {
    /// `o=z`: send the pixels zlib-compressed, if that is actually smaller.
    pub compress: bool,
    /// Payload bytes per APC escape. The protocol's maximum is [`CHUNK`]; a larger value
    /// sends the whole image in one escape, which some terminals handle and others do not.
    pub chunk: usize,
    /// `c=`/`r=`: the cell box to display over. Without it the terminal uses the image's
    /// natural size, which is right when our idea of the cell size is a guess.
    pub cells: bool,
}

impl Default for Wire {
    fn default() -> Wire {
        Wire { compress: true, chunk: CHUNK, cells: true }
    }
}

/// The full byte stream that draws `rgb` (`w`×`h`, 3 bytes per pixel) at the cursor and
/// leaves the cursor on the line below the image. `tmux` is passed in rather than read
/// from the environment so the encoding stays a pure function of its inputs. The caller
/// uses [`in_tmux`].
pub fn encode(rgb: &[u8], w: u32, h: u32, m: Metrics, tmux: bool) -> Vec<u8> {
    encode_with(rgb, w, h, m, tmux, Wire::default())
}

/// [`encode`], with the optional pieces of the protocol selectable. Everything ships through
/// here, so the probe exercises the real encoder rather than a second copy of it that could
/// agree with the terminal while this one does not.
pub fn encode_with(rgb: &[u8], w: u32, h: u32, m: Metrics, tmux: bool, wire: Wire) -> Vec<u8> {
    let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
    let payload = match z.write_all(rgb).and_then(|_| z.finish()) {
        Ok(c) => c,
        Err(_) => rgb.to_vec(),
    };
    let compressed = wire.compress && payload.len() < rgb.len();
    let b64 = STANDARD.encode(if compressed { &payload } else { rgb });

    let (cw, ch) = m.cell();
    let cols = (w as f64 / cw).ceil().max(1.0) as u32;
    let rows = (h as f64 / ch).ceil().max(1.0) as u32;

    let mut out = Vec::with_capacity(b64.len() + 1024);
    let emit = |seq: Vec<u8>, out: &mut Vec<u8>| {
        if tmux {
            passthrough(&seq, out);
        } else {
            out.extend_from_slice(&seq);
        }
    };

    // Free the image from `KEEP` plots ago before adding another. `d=I` rather than `d=i`:
    // the capital frees the stored DATA as well as the placement, and the data is the part
    // that accumulates.
    let (id, freed) = {
        let seq = SENT.fetch_add(1, Ordering::Relaxed);
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        admit(&mut live, id_base(), seq, rgb.len())
    };
    for old in freed {
        emit(format!("\x1b_Ga=d,d=I,i={old},q=2;\x1b\\").into_bytes(), &mut out);
    }

    let chunks: Vec<&str> = b64
        .as_bytes()
        .chunks(wire.chunk.max(4))
        .map(|c| std::str::from_utf8(c).expect("base64 is ascii"))
        .collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        let seq = if i == 0 {
            // `C=1` keeps the cursor put so the newlines below are the only thing that
            // moves it: the protocol's own cursor handling varies between terminals.
            format!(
                "\x1b_Ga=T,f=24,s={w},v={h},i={id}{}{},C=1,q=2{}{};{chunk}\x1b\\",
                if wire.cells { format!(",c={cols}") } else { String::new() },
                if wire.cells { format!(",r={rows}") } else { String::new() },
                if compressed { ",o=z" } else { "" },
                format_args!(",m={more}"),
            )
        } else {
            format!("\x1b_Gm={more};{chunk}\x1b\\")
        };
        emit(seq.into_bytes(), &mut out);
    }
    // Reserve the rows FIRST, then draw into space that already exists. Emitting the image
    // and scrolling afterwards asks the terminal to place pixels below its own last line,
    // which is where the handling differs: some scroll, some clip, and a clipped chart looks
    // like a corrupt one. `C=1` keeps the cursor put through the image, so afterwards it only
    // has to move down past what was reserved.
    let mut framed = format!("{}\x1b[{rows}A", "\n".repeat(rows as usize)).into_bytes();
    framed.extend(out);
    framed.extend(format!("\x1b[{rows}B\r").into_bytes());
    framed
}

// ---------------------------------------------------------------- decoding

/// Every image in a captured kitty stream, as `(width, height, rgb)`.
///
/// The counterpart to [`encode`], for the pixels that leave a terminal rather than enter one:
/// a report captured to a file becomes an email. Reads what this file writes, plus what a
/// tmux passthrough wrapper does to it on the way.
pub fn decode(stream: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    // Undo the tmux wrapper first: the payload inside is our own escapes with every ESC
    // doubled, so un-doubling has to happen before anything looks for an introducer.
    let mut flat = Vec::with_capacity(stream.len());
    let mut i = 0;
    while i < stream.len() {
        if stream[i..].starts_with(b"\x1bPtmux;") {
            i += 7;
            continue;
        }
        if stream[i] == 0x1b && stream.get(i + 1) == Some(&0x1b) {
            flat.push(0x1b);
            i += 2;
            continue;
        }
        flat.push(stream[i]);
        i += 1;
    }

    let mut out = Vec::new();
    let (mut dims, mut b64) = (None, Vec::new());
    let mut i = 0;
    while let Some(start) = find(&flat[i..], b"\x1b_G").map(|p| i + p) {
        let Some(semi) = find(&flat[start..], b";").map(|p| start + p) else {
            break;
        };
        let Some(end) = find(&flat[semi..], b"\x1b\\").map(|p| semi + p) else {
            break;
        };
        let ctrl = String::from_utf8_lossy(&flat[start + 3..semi]).to_string();
        // `a=T` opens an image; anything else is a continuation chunk or a delete, and a
        // delete carries no payload so it contributes nothing either way.
        if ctrl.contains("a=T") {
            emit(&mut out, dims.take(), &mut b64);
            let kv = |k: &str| {
                ctrl.split(',')
                    .find_map(|p| p.strip_prefix(k))
                    .and_then(|v| v.parse::<u32>().ok())
            };
            if let (Some(w), Some(h)) = (kv("s="), kv("v=")) {
                dims = Some((w, h, ctrl.contains("o=z")));
            }
        }
        if dims.is_some() {
            b64.extend_from_slice(&flat[semi + 1..end]);
        }
        i = end + 2;
    }
    emit(&mut out, dims, &mut b64);

    fn emit(out: &mut Vec<(u32, u32, Vec<u8>)>, dims: Option<(u32, u32, bool)>, b64: &mut Vec<u8>) {
        let Some((w, h, zipped)) = dims else {
            b64.clear();
            return;
        };
        let payload = STANDARD.decode(std::mem::take(b64)).unwrap_or_default();
        let rgb = if zipped {
            let mut v = Vec::new();
            match ZlibDecoder::new(&payload[..]).read_to_end(&mut v) {
                Ok(_) => v,
                Err(_) => return,
            }
        } else {
            payload
        };
        if rgb.len() >= (w as usize * h as usize * 3) {
            out.push((w, h, rgb));
        }
    }
    out
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m() -> Metrics {
        Metrics {
            cols: 100,
            rows: 30,
            xpix: 800,
            ypix: 480,
        }
    }

    #[test]
    fn font_scale_tracks_the_terminal_cell_not_the_image() {
        let at = |ch: u16, rows: u16| {
            Metrics {
                cols: 100,
                rows,
                xpix: 800,
                ypix: ch * rows,
            }
            .font_scale()
        };
        assert_eq!(at(16, 30), 1, "an ordinary 16px cell wants 1x glyphs");
        assert_eq!(at(20, 30), 1);
        assert_eq!(at(32, 30), 2, "a HiDPI 32px cell wants 2x");
        assert_eq!(at(40, 30), 2);
        // Never zero, however odd the report: a 0 scale would draw invisible labels.
        assert!(
            Metrics {
                cols: 0,
                rows: 0,
                xpix: 0,
                ypix: 0
            }
            .font_scale()
                >= 1
        );
    }

    #[test]
    fn cell_size_falls_back_when_unreported() {
        assert_eq!(m().cell(), (8.0, 16.0));
        let none = Metrics {
            cols: 100,
            rows: 30,
            xpix: 0,
            ypix: 0,
        };
        assert_eq!(none.cell(), (8.0, 16.0));
        // Never zero, whatever the terminal claims: a zero divisor here would be a panic
        // in the middle of drawing.
        let silly = Metrics {
            cols: 0,
            rows: 0,
            xpix: 0,
            ypix: 0,
        };
        let (cw, ch) = silly.cell();
        assert!(cw >= 1.0 && ch >= 1.0);
        assert!(silly.default_plot().0 >= 320);
    }

    #[test]
    fn encodes_one_chunked_compressed_image() {
        // A flat image compresses hugely, which is the point of `o=z`.
        let rgb = vec![7u8; 200 * 100 * 3];
        let out = encode(&rgb, 200, 100, m(), false);
        let s = String::from_utf8_lossy(&out);
        // The stream OPENS by reserving rows, so the transmit is not at byte zero.
        assert!(s.contains("\x1b_Ga=T,f=24,s=200,v=100,"), "{:?}", &s[..40.min(s.len())]);
        assert!(s.contains(",o=z"), "flat image should compress");
        assert!(s.contains("q=2"), "responses must be suppressed");
        assert!(
            out.len() < rgb.len() / 4,
            "compression did not help: {}",
            out.len()
        );
        // Reserve, back up, draw, come down: the cursor ends below the image by construction
        // rather than wherever the terminal chose to leave it.
        let rows = (100.0f64 / m().cell().1).ceil() as u32;
        assert!(s.starts_with(&format!("{}\x1b[{rows}A", "\n".repeat(rows as usize))));
        assert!(s.ends_with(&format!("\x1b[{rows}B\r")));
    }

    /// The live set stays bounded however many charts a session draws, which is the whole
    /// point: without it every plot left a frame in the terminal for good.
    #[test]
    fn the_store_stays_bounded() {
        use std::collections::VecDeque;
        let mut live: VecDeque<(u32, usize)> = VecDeque::new();
        for seq in 0..500 {
            admit(&mut live, 1000, seq, 432_000);
            assert!(live.len() <= KEEP, "{} held after {seq}", live.len());
            assert!(!live.is_empty(), "nothing kept, not even the newest");
        }
        assert_eq!(live.len(), KEEP, "a long session should settle at the bound");
    }

    /// An id is never handed out while the terminal is still holding an image under it.
    #[test]
    fn a_reused_id_is_always_a_freed_one() {
        use std::collections::VecDeque;
        let mut live: VecDeque<(u32, usize)> = VecDeque::new();
        for seq in 0..SLOTS * 4 {
            let before: Vec<u32> = live.iter().map(|(i, _)| *i).collect();
            let (id, freed) = admit(&mut live, 1000, seq, 40_000);
            let still_live: Vec<u32> =
                before.into_iter().filter(|i| !freed.contains(i)).collect();
            assert!(!still_live.contains(&id), "seq {seq} took live id {id}");
        }
    }

    /// Encode then decode, for every combination of the wire's optional pieces. This is the
    /// property that matters for turning a captured report into an email: what comes back has
    /// to be the pixels that went in, not merely something image-shaped.
    #[test]
    fn a_stream_decodes_back_to_its_pixels() {
        let (w, h) = (37u32, 11u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i * 31 % 253) as u8).collect();
        for tmux in [false, true] {
            for wire in [
                Wire { compress: false, chunk: usize::MAX, cells: false },
                Wire { compress: false, chunk: 4096, cells: false },
                Wire { compress: true, chunk: usize::MAX, cells: true },
                Wire::default(),
            ] {
                let s = encode_with(&rgb, w, h, m(), tmux, wire);
                let got = decode(&s);
                assert_eq!(got.len(), 1, "tmux={tmux} wire={wire:?}");
                assert_eq!((got[0].0, got[0].1), (w, h));
                assert_eq!(got[0].2[..rgb.len()], rgb[..], "tmux={tmux} wire={wire:?}");
            }
        }
    }

    /// Several images in one stream come back in order, with the deletes between them
    /// contributing nothing. A report is a sequence, and the order is the report.
    #[test]
    fn a_sequence_decodes_in_order() {
        let mut stream = Vec::new();
        let sizes = [(8u32, 4u32), (12, 6), (5, 9)];
        for (i, (w, h)) in sizes.iter().enumerate() {
            let rgb = vec![(i * 40) as u8; (w * h * 3) as usize];
            stream.extend(encode(&rgb, *w, *h, m(), false));
        }
        let got = decode(&stream);
        assert_eq!(got.len(), 3);
        for (i, (w, h)) in sizes.iter().enumerate() {
            assert_eq!((got[i].0, got[i].1), (*w, *h), "image {i}");
            assert!(got[i].2.iter().all(|b| *b == (i * 40) as u8), "image {i} pixels");
        }
    }

    #[test]
    fn chunks_are_within_the_protocol_limit() {
        // Random-ish bytes defeat zlib, so this really does span many chunks.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let rgb: Vec<u8> = (0..300 * 200 * 3)
            .map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed as u8
            })
            .collect();
        let out = encode(&rgb, 300, 200, m(), false);
        let s = String::from_utf8_lossy(&out);
        let parts: Vec<&str> = s.split("\x1b_G").skip(1).collect();
        assert!(parts.len() > 1, "expected chunking");
        for p in &parts {
            let payload = p.split(';').nth(1).unwrap_or("");
            assert!(payload.trim_end_matches(['\x1b', '\\', '\n']).len() <= CHUNK);
        }
        // Exactly one terminating chunk.
        assert_eq!(s.matches("m=0").count(), 1);
    }

    #[test]
    fn tmux_wraps_every_sequence() {
        let out = encode(&vec![9u8; 60 * 40 * 3], 60, 40, m(), true);
        let s = String::from_utf8_lossy(&out);
        // The reserve comes first and is plain text, so the wrapper starts at the escape.
        assert!(
            s.contains("\x1bPtmux;"),
            "not wrapped: {:?}",
            &s[..24.min(s.len())]
        );
        // Every APC introducer must be the DOUBLED form; a bare one would end tmux's DCS
        // early and dump the payload on screen as text.
        assert_eq!(
            s.matches("\x1b\x1b_G").count(),
            s.matches("\x1b_G").count(),
            "an undoubled APC leaked through the wrapper"
        );
    }

    #[test]
    fn tmux_passthrough_doubles_escapes() {
        let mut seq = Vec::new();
        passthrough(b"\x1b_Ga=T;AA\x1b\\", &mut seq);
        let s = String::from_utf8_lossy(&seq);
        assert!(s.starts_with("\x1bPtmux;"));
        assert!(
            s.contains("\x1b\x1b_Ga=T;AA"),
            "inner ESC not doubled: {s:?}"
        );
        assert!(s.ends_with("\x1b\\"));
    }
}
