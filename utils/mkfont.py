import gzip, struct, sys

PCF_METRICS, PCF_BITMAPS, PCF_BDF_ENCODINGS = 4, 8, 32
COMPRESSED_METRICS = 0x100

class R:
    def __init__(s, b, msb_byte): s.b, s.p, s.m = b, 0, msb_byte
    def u32(s):
        f = '>I' if s.m else '<I'; v = struct.unpack_from(f, s.b, s.p)[0]; s.p += 4; return v
    def i16(s):
        f = '>h' if s.m else '<h'; v = struct.unpack_from(f, s.b, s.p)[0]; s.p += 2; return v
    def u16(s):
        f = '>H' if s.m else '<H'; v = struct.unpack_from(f, s.b, s.p)[0]; s.p += 2; return v
    def u8(s):
        v = s.b[s.p]; s.p += 1; return v

def tables(buf):
    assert buf[:4] == b'\x01fcp', buf[:4]
    n = struct.unpack_from('<I', buf, 4)[0]
    out = {}
    for i in range(n):
        t, fmt, size, off = struct.unpack_from('<IIII', buf, 8 + 16*i)
        out[t] = (fmt, size, off)
    return out

def read_table(buf, off):
    fmt = struct.unpack_from('<I', buf, off)[0]     # format is ALWAYS little-endian
    msb_byte = bool(fmt & 4)
    r = R(buf, msb_byte); r.p = off + 4
    return fmt, r

def parse(path):
    buf = gzip.open(path, 'rb').read()
    tb = tables(buf)

    # --- metrics
    fmt, r = read_table(buf, tb[PCF_METRICS][2])
    if fmt & COMPRESSED_METRICS:
        n = r.i16()
        metrics = [tuple(r.u8() - 0x80 for _ in range(5)) for _ in range(n)]
    else:
        n = r.u32()
        metrics = [(r.i16(), r.i16(), r.i16(), r.i16(), r.i16(), r.u16())[:5] for _ in range(n)]

    # --- bitmaps
    fmt, r = read_table(buf, tb[PCF_BITMAPS][2])
    glyph_count = r.u32()
    offsets = [r.u32() for _ in range(glyph_count)]
    sizes = [r.u32() for _ in range(4)]
    data_off = r.p
    data = buf[data_off:data_off + sizes[fmt & 3]]
    pad = 1 << (fmt & 3)                 # row padding in bytes
    msb_bit = bool(fmt & 8)

    # --- encodings
    fmt, r = read_table(buf, tb[PCF_BDF_ENCODINGS][2])
    min2, max2, min1, max1, default = (r.u16(), r.u16(), r.u16(), r.u16(), r.u16())
    idx = {}
    for b1 in range(min1, max1 + 1):
        for b2 in range(min2, max2 + 1):
            g = r.u16()
            if g != 0xFFFF:
                idx[(b1 << 8 | b2) if max1 else b2] = g
    return metrics, offsets, data, pad, msb_bit, idx

def glyph_rows(gi, metrics, offsets, data, pad, msb_bit, height):
    lsb, rsb, width, asc, desc = metrics[gi]
    w = rsb - lsb
    h = asc + desc
    rowbytes = ((w + 8*pad - 1) // (8*pad)) * pad
    base = offsets[gi]
    rows = []
    for y in range(h):
        val = 0
        for bx in range(rowbytes):
            byte = data[base + y*rowbytes + bx]
            if not msb_bit:
                byte = int(f'{byte:08b}'[::-1], 2)
            val = (val << 8) | byte
        val >>= max(0, rowbytes*8 - w)
        rows.append((val, w, lsb, asc))
    return rows, width, h

if __name__ == '__main__':
    path, lo, hi = sys.argv[1], 32, 126
    metrics, offsets, data, pad, msb_bit, idx = parse(path)
    # font box from the space/`M` glyph
    gi = idx[ord('M')]
    rows, adv, h = glyph_rows(gi, metrics, offsets, data, pad, msb_bit, 0)
    print(f'// advance={adv} height={h} pad={pad} msb_bit={msb_bit} glyphs={len(idx)}')
    for ch in ('M', 'g', '0', '.'):
        gi = idx[ord(ch)]
        rows, adv, h = glyph_rows(gi, metrics, offsets, data, pad, msb_bit, 0)
        print(f'--- {ch!r} adv={adv} h={h}')
        for val, w, lsb, asc in rows:
            print('   ' + ''.join('#' if val >> (w-1-x) & 1 else '.' for x in range(w)))

def emit(path, out):
    metrics, offsets, data, pad, msb_bit, idx = parse(path)
    lines = []
    for code in range(32, 127):
        gi = idx.get(code)
        if gi is None:
            lines.append((code, [0]*13)); continue
        rows, adv, h = glyph_rows(gi, metrics, offsets, data, pad, msb_bit, 0)
        cell = []
        for val, w, lsb, asc in rows:
            r = 0
            for x in range(w):
                if val >> (w-1-x) & 1:
                    px = lsb + x
                    if 0 <= px < 6:
                        r |= 1 << (5 - px)
            cell.append(r)
        assert len(cell) == 13, (code, len(cell))
        lines.append((code, cell))
    with open(out, 'w') as f:
        f.write('''//! 6x13 bitmap glyphs for ASCII 32..126, extracted once from the X11 `misc-fixed`
//! 6x13 font (`/usr/share/fonts/X11/misc/6x13-ISO8859-1.pcf.gz`), which is public
//! domain. Committed as data so the raster canvas needs no font stack at runtime:
//! no fontconfig, no TTF parser, nothing to cross-compile. Crisp at chart-label
//! sizes and integer-scalable for HiDPI.
//!
//! One byte per row, 13 rows per glyph, bit 5 = leftmost pixel.
//! Regenerate with `utils/mkfont.py` if the box ever changes.

/// Glyph cell width in pixels.
pub const W: u32 = 6;
/// Glyph cell height in pixels.
pub const H: u32 = 13;

/// Rows for `ch`, or all-blank when it is outside ASCII 32..126.
pub fn glyph(ch: char) -> &'static [u8; H as usize] {
    let c = ch as usize;
    if (32..127).contains(&c) {
        &GLYPHS[c - 32]
    } else {
        &GLYPHS[0]
    }
}

#[rustfmt::skip]
static GLYPHS: [[u8; H as usize]; 95] = [
''')
        for code, cell in lines:
            ch = chr(code)
            label = {"\\\\": "backslash"}.get(ch, ch)
            body = ','.join(f'0x{b:02x}' for b in cell)
            f.write(f'    [{body}], // {code} {label!r}\n')
        f.write('];\n')
    print('wrote', out)

emit('/usr/share/fonts/X11/misc/6x13-ISO8859-1.pcf.gz', sys.argv[2] if len(sys.argv)>2 else '/dev/stdout')
