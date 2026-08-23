//! RGB pixels to a PNG file, for the pixels that leave the terminal rather than enter it.
//!
//! The library needs no PNG to draw a chart: a terminal takes zlib-compressed RGB directly,
//! which is why there is no image crate here. It needs one to leave the terminal, and PNG for
//! raw RGB is small enough to write than to depend on: a header, the scanlines deflated, a
//! trailer, and a CRC on each. `flate2` is already here for the kitty encoder.
//!
//! Filter type 0 on every scanline, and no interlacing. Filtering exists to make the deflate
//! that follows cheaper, and on flat chart pixels deflate does well enough unaided that the
//! extra pass is not worth its code.

use flate2::{write::ZlibEncoder, Compression, Crc};
use std::io::Write;

/// `rgb` as a PNG, 8 bits per channel, no alpha.
pub fn encode(rgb: &[u8], w: u32, h: u32) -> Vec<u8> {
    let stride = w as usize * 3;
    let mut raw = Vec::with_capacity((stride + 1) * h as usize);
    for y in 0..h as usize {
        raw.push(0); // filter: none
        let row = y * stride;
        // A short buffer pads with black rather than panicking: a caller that miscounted gets
        // a readable image and a visible mistake, not a crash inside an encoder.
        match rgb.get(row..row + stride) {
            Some(s) => raw.extend_from_slice(s),
            None => raw.resize(raw.len() + stride, 0),
        }
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
    let idat = match z.write_all(&raw).and_then(|()| z.finish()) {
        Ok(c) => c,
        Err(_) => raw,
    };

    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, truecolour, deflate, no filter, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &idat);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, tag: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(tag);
    out.extend_from_slice(body);
    let mut crc = Crc::new();
    crc.update(tag);
    crc.update(body);
    out.extend_from_slice(&crc.sum().to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    /// The envelope: signature, IHDR describing what we said, IEND last, and a CRC on every
    /// chunk that a reader will check. A wrong CRC is not a slightly wrong image, it is a
    /// file every decoder rejects.
    #[test]
    fn the_file_is_well_formed() {
        let (w, h) = (7u32, 3u32);
        let png = encode(&vec![9u8; (w * h * 3) as usize], w, h);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

        let mut tags = Vec::new();
        let mut i = 8;
        while i + 12 <= png.len() {
            let len = u32::from_be_bytes(png[i..i + 4].try_into().unwrap()) as usize;
            let tag = &png[i + 4..i + 8];
            let body = &png[i + 8..i + 8 + len];
            let mut crc = Crc::new();
            crc.update(tag);
            crc.update(body);
            let want = u32::from_be_bytes(png[i + 8 + len..i + 12 + len].try_into().unwrap());
            assert_eq!(crc.sum(), want, "bad CRC on {:?}", std::str::from_utf8(tag));
            if tag == b"IHDR" {
                assert_eq!(u32::from_be_bytes(body[..4].try_into().unwrap()), w);
                assert_eq!(u32::from_be_bytes(body[4..8].try_into().unwrap()), h);
                assert_eq!(&body[8..], &[8, 2, 0, 0, 0]);
            }
            tags.push(std::str::from_utf8(tag).unwrap().to_string());
            i += 12 + len;
        }
        assert_eq!(i, png.len(), "trailing bytes after the last chunk");
        assert_eq!(tags, ["IHDR", "IDAT", "IEND"]);
    }

    /// The pixels survive. Inflating IDAT must give back exactly the scanlines that went in,
    /// each behind its filter byte.
    #[test]
    fn the_pixels_round_trip() {
        let (w, h) = (5u32, 4u32);
        let rgb: Vec<u8> = (0..w * h * 3).map(|i| (i * 7 % 251) as u8).collect();
        let png = encode(&rgb, w, h);

        let start = png.windows(4).position(|c| c == b"IDAT").expect("IDAT") + 4;
        let len = u32::from_be_bytes(png[start - 8..start - 4].try_into().unwrap()) as usize;
        let mut raw = Vec::new();
        ZlibDecoder::new(&png[start..start + len]).read_to_end(&mut raw).unwrap();

        let stride = w as usize * 3;
        assert_eq!(raw.len(), (stride + 1) * h as usize);
        for y in 0..h as usize {
            assert_eq!(raw[y * (stride + 1)], 0, "row {y} filter");
            assert_eq!(&raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)], &rgb[y * stride..(y + 1) * stride]);
        }
    }

    /// A caller that supplies too few bytes gets a short image, not a panic inside the
    /// encoder. This runs on data that has been through two decoders already.
    #[test]
    fn a_short_buffer_does_not_panic() {
        let png = encode(&[1, 2, 3], 10, 10);
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        assert!(png.len() > 30);
    }
}
