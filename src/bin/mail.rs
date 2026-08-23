//! `jplots-mail`: turn a captured terminal session into an email.
//!
//! A Unix filter, so it works for any command rather than only for q:
//!
//! ```text
//! report.q -q kitty | jplots-mail | sendmail -t
//! ```
//!
//! Text runs become `<pre>` blocks, which is what a fixed-width table needs to survive; sixel
//! and kitty images become PNG parts in the same order they were drawn; and lines the script
//! printed as `EMAIL-SUBJECT:` and friends become headers rather than body text, so the
//! sending script decides the subject and the recipients from its own results.

use jplots::{kitty, png, sixel};
use std::collections::HashMap;
use std::io::{Read, Write};

const USAGE: &str = "\
jplots-mail: a captured terminal session as an email

    report.q -q kitty | jplots-mail | sendmail -t

    --to ADDR --from ADDR --subject S   defaults, overridden by EMAIL-* lines
    --data-uri                          one self-contained file, no CID parts
    --html                              the HTML body alone, with no message headers
    --title TEXT                        heading above the body
    --template FILE                     a Jinja2 body template; see templates/mail.html.j2

The input may name its own headers by printing lines like

    EMAIL-TO: ops@example.com
    EMAIL-SUBJECT: [BAD] 1 error

which are removed from the body. TO, FROM, SUBJECT, CC, BCC and REPLY-TO are recognised.
Capture with the kitty renderer where possible: it carries truecolour, where sixel has
already been reduced to a 256-colour palette.
";

/// The header names a script may set, and the header each becomes.
const MARKERS: [(&str, &str); 6] = [
    ("EMAIL-TO", "To"),
    ("EMAIL-FROM", "From"),
    ("EMAIL-SUBJECT", "Subject"),
    ("EMAIL-CC", "Cc"),
    ("EMAIL-BCC", "Bcc"),
    ("EMAIL-REPLY-TO", "Reply-To"),
];

/// A run of the input: either text, or an image already decoded to pixels.
enum Part {
    Text(String),
    Image(u32, u32, Vec<u8>),
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return;
    }
    let flag = |name: &str| -> Option<String> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
    };
    let data_uri = args.iter().any(|a| a == "--data-uri");
    let body_only = args.iter().any(|a| a == "--html");

    let mut input = Vec::new();
    if std::io::stdin().read_to_end(&mut input).is_err() {
        eprintln!("jplots-mail: could not read stdin");
        std::process::exit(1);
    }

    let (parts, mut headers) = split(&input);
    for (marker, header) in MARKERS {
        let cli = match marker {
            "EMAIL-TO" => flag("--to"),
            "EMAIL-FROM" => flag("--from"),
            "EMAIL-SUBJECT" => flag("--subject"),
            _ => None,
        };
        // A marker in the stream wins: it is the one that saw the results.
        if let Some(v) = cli {
            headers.entry(header.to_string()).or_insert(v);
        }
    }
    // A missing recipient is a warning, not a refusal: the caller may be piping to something
    // that supplies its own envelope, and a report that fails to print is worse than one with
    // no To: line.
    if !body_only && !headers.contains_key("To") {
        eprintln!("jplots-mail: no recipient (print `EMAIL-TO: addr`, or pass --to)");
    }

    let template = match flag("--template") {
        Some(path) => match std::fs::read_to_string(&path) {
            Ok(t) => Some(t),
            Err(e) => {
                eprintln!("jplots-mail: cannot read template {path}: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    let images = parts.iter().filter(|p| matches!(p, Part::Image(..))).count();
    let html = match body(
        &parts,
        flag("--title").as_deref(),
        &headers,
        data_uri,
        template.as_deref(),
    ) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("jplots-mail: template: {e}");
            std::process::exit(2);
        }
    };

    let out = std::io::stdout();
    let mut out = out.lock();
    let _ = if body_only {
        out.write_all(html.as_bytes())
    } else if data_uri || images == 0 {
        write_simple(&mut out, &headers, &html)
    } else {
        write_related(&mut out, &headers, &html, &parts)
    };
}

/// Undo tmux passthrough, so the stream that follows has one meaning per escape.
///
/// tmux wraps each escape as `ESC Ptmux; … ESC \` with every ESC inside DOUBLED. A scan for
/// the first `ESC \` therefore finds the doubled terminator INSIDE the payload and cuts the
/// image in half, and what is left decodes as nothing. Undoing the wrapper first is what makes
/// a later split unambiguous: skip a doubled ESC as a pair, and a lone one ends the wrapper.
fn untmux(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if !input[i..].starts_with(b"\x1bPtmux;") {
            out.push(input[i]);
            i += 1;
            continue;
        }
        i += 7;
        while i < input.len() {
            if input[i] == 0x1b {
                match input.get(i + 1) {
                    Some(0x1b) => {
                        out.push(0x1b);
                        i += 2;
                        continue;
                    }
                    Some(b'\\') => {
                        i += 2;
                        break;
                    }
                    _ => {}
                }
            }
            out.push(input[i]);
            i += 1;
        }
    }
    out
}

/// Split the stream into ordered parts, lifting the `EMAIL-*` lines out of the text as it goes.
///
/// One image may span several escapes (the kitty protocol chunks at 4096 bytes), so a run of
/// them separated by nothing is taken together and handed to the decoder whole.
fn split(input: &[u8]) -> (Vec<Part>, HashMap<String, String>) {
    let input = untmux(input);
    let mut parts = Vec::new();
    let mut headers = HashMap::new();
    let (mut cut, mut i) = (0usize, 0usize);

    while i + 1 < input.len() {
        let kitty = input[i] == 0x1b && input[i + 1] == b'_';
        let sixel = input[i] == 0x1b && input[i + 1] == b'P';
        if !(kitty || sixel) {
            i += 1;
            continue;
        }
        // Extend over consecutive escapes of the same kind, ignoring whitespace between them.
        let mut end = i;
        while let Some(p) = find(&input[end..], b"\x1b\\") {
            end += p + 2;
            let next = input[end..]
                .iter()
                .position(|b| !b.is_ascii_whitespace())
                .map_or(input.len(), |p| end + p);
            let more = if kitty {
                input[next..].starts_with(b"\x1b_")
            } else {
                input[next..].starts_with(b"\x1bP")
            };
            if !more {
                break;
            }
            end = next;
        }
        let decoded = if sixel {
            sixel::decode(&input[i..end])
        } else {
            kitty::decode(&input[i..end])
        };
        if decoded.is_empty() {
            i += 2;
            continue;
        }
        push_text(&mut parts, &mut headers, &input[cut..i]);
        for (w, h, rgb) in decoded {
            parts.push(Part::Image(w, h, rgb));
        }
        i = end;
        cut = end;
    }
    push_text(&mut parts, &mut headers, &input[cut..]);
    (parts, headers)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Add a text run, taking any `EMAIL-*` lines into the headers instead of the body.
fn push_text(parts: &mut Vec<Part>, headers: &mut HashMap<String, String>, raw: &[u8]) {
    let text = strip_ansi(&String::from_utf8_lossy(raw));
    let mut kept = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if let Some((marker, header)) = MARKERS
            .iter()
            .find(|(m, _)| trimmed.trim_start().starts_with(&format!("{m}:")))
        {
            let v = trimmed.trim_start()[marker.len() + 1..].trim();
            headers.insert((*header).to_string(), v.to_string());
            continue;
        }
        kept.push_str(line);
    }
    if !kept.trim().is_empty() {
        parts.push(Part::Text(kept));
    }
}

/// Drop ANSI control sequences. A terminal capture is full of them and an email is not a
/// terminal: without this the body fills with `[32m`.
fn strip_ansi(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == 0x1b {
            // CSI and OSC run to a final byte; anything else is a two-byte escape.
            let rest = &b[i + 1..];
            i += match rest.first() {
                Some(b'[') => 2 + rest[1..].iter().position(|c| (0x40..=0x7e).contains(c)).map_or(0, |p| p + 1),
                Some(b']') => {
                    2 + rest[1..].iter().position(|c| *c == 0x07 || *c == 0x1b).map_or(0, |p| p + 1)
                }
                _ => 2,
            };
            continue;
        }
        if b[i] == b'\r' {
            i += 1;
            continue;
        }
        out.push(b[i] as char);
        i += 1;
    }
    out
}

/// The default body template, so the binary works with no files beside it.
const DEFAULT_TEMPLATE: &str = include_str!("../../templates/mail.html.j2");

/// Render the body. The template sees the session as an ordered list of parts, each already
/// carrying the `src` it needs, so it never has to know whether the images went out as CID
/// parts or as data URIs.
fn body(
    parts: &[Part],
    title: Option<&str>,
    headers: &HashMap<String, String>,
    data_uri: bool,
    template: Option<&str>,
) -> Result<String, minijinja::Error> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use minijinja::value::Value;
    use minijinja::{context, Environment};

    let mut items = Vec::new();
    let mut n = 0;
    for p in parts {
        match p {
            Part::Text(t) => items.push(context! {
                kind => "text",
                text => t.trim_matches('\n'),
            }),
            Part::Image(w, h, rgb) => {
                let src = if data_uri {
                    format!("data:image/png;base64,{}", STANDARD.encode(png::encode(rgb, *w, *h)))
                } else {
                    format!("cid:chart{n}@jplots")
                };
                items.push(context! {
                    kind => "image",
                    // Marked safe because WE built it. Autoescape would turn `image/png`
                    // into `image&#x2f;png` and mangle every `/` in the base64, which breaks
                    // a data URI silently and leaves a CID one working: the sort of bug that
                    // ships. Text runs stay escaped, which is where the danger actually is.
                    src => Value::from_safe_string(src),
                    width => w, height => h,
                    alt => format!("chart {}", n + 1),
                });
                n += 1;
            }
        }
    }

    // Autoescape on: a text run is arbitrary command output, and the one thing an email body
    // must not do is let it close a tag.
    let mut env = Environment::new();
    env.set_auto_escape_callback(|_| minijinja::AutoEscape::Html);
    env.add_template("body", template.unwrap_or(DEFAULT_TEMPLATE))?;
    let lower: HashMap<String, &String> = headers
        .iter()
        .map(|(k, v)| (k.to_lowercase().replace('-', "_"), v))
        .collect();
    env.get_template("body")?
        .render(context! { title => title, headers => lower, parts => items })
}

fn write_headers(out: &mut impl Write, headers: &HashMap<String, String>) -> std::io::Result<()> {
    for (k, v) in [
        ("From", headers.get("From")),
        ("To", headers.get("To")),
        ("Cc", headers.get("Cc")),
        ("Bcc", headers.get("Bcc")),
        ("Reply-To", headers.get("Reply-To")),
        ("Subject", headers.get("Subject")),
    ] {
        if let Some(v) = v {
            writeln!(out, "{k}: {v}")?;
        }
    }
    writeln!(out, "MIME-Version: 1.0")
}

fn write_simple(
    out: &mut impl Write,
    headers: &HashMap<String, String>,
    html: &str,
) -> std::io::Result<()> {
    write_headers(out, headers)?;
    writeln!(out, "Content-Type: text/html; charset=utf-8\n")?;
    out.write_all(html.as_bytes())
}

/// `multipart/related` with the images as CID parts. Outlook will not load a `data:` image,
/// so this is the form that reaches an ops mailbox rather than the one that is simpler.
fn write_related(
    out: &mut impl Write,
    headers: &HashMap<String, String>,
    html: &str,
    parts: &[Part],
) -> std::io::Result<()> {
    use base64::{engine::general_purpose::STANDARD, Engine};
    // Fixed rather than random: a boundary only has to not occur in the body, and base64 and
    // our HTML cannot contain this. Determinism also makes the output testable.
    let b = "----=_jplots_related_boundary";
    write_headers(out, headers)?;
    writeln!(out, "Content-Type: multipart/related; type=\"text/html\"; boundary=\"{b}\"\n")?;
    writeln!(out, "--{b}")?;
    writeln!(out, "Content-Type: text/html; charset=utf-8\n")?;
    out.write_all(html.as_bytes())?;

    let mut n = 0;
    for p in parts {
        let Part::Image(w, h, rgb) = p else { continue };
        let b64 = STANDARD.encode(png::encode(rgb, *w, *h));
        writeln!(out, "\n--{b}")?;
        writeln!(out, "Content-Type: image/png")?;
        writeln!(out, "Content-Transfer-Encoding: base64")?;
        writeln!(out, "Content-ID: <chart{n}@jplots>")?;
        writeln!(out, "Content-Disposition: inline; filename=\"chart{n}.png\"\n")?;
        // 76 characters, which is what RFC 2045 asks of base64 in a MIME part.
        for line in b64.as_bytes().chunks(76) {
            out.write_all(line)?;
            writeln!(out)?;
        }
        n += 1;
    }
    writeln!(out, "\n--{b}--")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tmux doubles every ESC inside its wrapper, so a scan for the first `ESC \` lands on
    /// the doubled terminator INSIDE the payload. That truncated every image in a captured
    /// tmux session and the whole report arrived text-only.
    #[test]
    fn untmux_unwraps_without_cutting_the_payload() {
        let wrapped = b"before\x1bPtmux;\x1b\x1b_Ga=T;AAAA\x1b\x1b\\\x1b\\after";
        assert_eq!(untmux(wrapped), b"before\x1b_Ga=T;AAAA\x1b\\after");
        // Not wrapped at all: unchanged, including a bare escape in the text.
        let plain = b"plain \x1b_Gm=0;QQ\x1b\\ text";
        assert_eq!(untmux(plain), plain);
    }

    /// A run of escapes with nothing between them is ONE image: the kitty protocol chunks at
    /// 4096 bytes, so a chart is routinely several. Splitting per escape would emit an image
    /// per chunk and decode none of them.
    #[test]
    fn a_chunked_image_is_one_part() {
        let m = jplots::kitty::Metrics { cols: 80, rows: 24, xpix: 640, ypix: 384 };
        let (w, h) = (60u32, 40u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i % 251) as u8).collect();
        let mut stream = b"heading\n".to_vec();
        stream.extend(jplots::kitty::encode(&rgb, w, h, m, false));
        stream.extend(b"trailer\n");

        let (parts, _) = split(&stream);
        let imgs: Vec<_> = parts
            .iter()
            .filter_map(|p| match p {
                Part::Image(w, h, _) => Some((*w, *h)),
                _ => None,
            })
            .collect();
        assert_eq!(imgs, [(w, h)], "one image, not one per chunk");
        assert_eq!(parts.len(), 3, "text, image, text");
    }

    /// Markers become headers and leave the body. A report that printed its own subject must
    /// not also show that line to the reader.
    #[test]
    fn markers_become_headers() {
        let input = b"EMAIL-TO: ops@x.com\nEMAIL-SUBJECT: [BAD] 2 errors\nreal body\n";
        let (parts, headers) = split(input);
        assert_eq!(headers.get("To").map(String::as_str), Some("ops@x.com"));
        assert_eq!(headers.get("Subject").map(String::as_str), Some("[BAD] 2 errors"));
        match &parts[..] {
            [Part::Text(t)] => assert_eq!(t.trim(), "real body"),
            other => panic!("expected one text part, got {}", other.len()),
        }
    }

    /// ANSI has to go, or the body fills with `[32m`. A terminal capture is full of it.
    #[test]
    fn ansi_is_stripped() {
        assert_eq!(strip_ansi("\x1b[32mgreen\x1b[0m and \x1b[1;31mred\x1b[m"), "green and red");
        assert_eq!(strip_ansi("title\x1b]0;window\x07done"), "titledone");
        assert_eq!(strip_ansi("carriage\rreturn"), "carriagereturn");
    }
}
