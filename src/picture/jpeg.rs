//! JPEG, the plain kind: Huffman coding, eight bits, one scan or one
//! scan for each component. Read as a stream, one entropy-coded block
//! after the next, and each block that lies in the part that is wanted
//! is made into samples at a half, a quarter or an eighth of its size
//! where the result is that much smaller than the part. Only what the
//! part holds is kept: the samples of its blocks, a plane for each
//! component, and then the result.
//!
//! What is read here comes from anywhere. As in `png.rs` nothing in
//! this file indexes or reckons in a way that can fail: the lints below
//! hold it to that, and `cargo clippy` is what checks them. Floating
//! point is not a side effect in that sense, and a cast from it
//! saturates.

#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use std::io::Read;

use super::png::{Picture, fit};
use super::wire::{PIXELS_MAX, SIDE_MAX, Wish};
use crate::core::picture::{Decoded, Notes, Refused};

/// In pieces of how many bytes the file is read.
const PIECE: usize = 64 << 10;
/// What the planes, the result and its copy may take together. A picture
/// that needs more for the size that was asked is made smaller still.
const WORK: u64 = 128 << 20;
/// How many segments a file may have before the data.
const SEGMENTS: u32 = 65_536;
/// The places of a block in the order they come in, as `zigzag[k]`.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7,
    14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39,
    46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// The file, a piece at a time, with the markers it holds found.
struct Input<R> {
    from: R,
    piece: Vec<u8>,
    at: usize,
    len: usize,
    done: bool,
}

impl<R: Read> Input<R> {
    fn new(from: R) -> Self {
        Self { from, piece: vec![0; PIECE], at: 0, len: 0, done: false }
    }

    fn byte(&mut self) -> Option<u8> {
        if self.at >= self.len {
            if self.done {
                return None;
            }
            self.at = 0;
            self.len = 0;
            loop {
                match self.from.read(&mut self.piece) {
                    Ok(0) => {
                        self.done = true;
                        return None;
                    }
                    Ok(read) => {
                        self.len = read;
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => {
                        self.done = true;
                        return None;
                    }
                }
            }
        }
        let byte = self.piece.get(self.at).copied();
        self.at = self.at.saturating_add(1);
        byte
    }

    /// The next marker, going over whatever lies before it: bytes that
    /// are not part of one, stuffing, fill bytes, and restart markers
    /// where no data is being read.
    fn marker(&mut self) -> Option<u8> {
        loop {
            while self.byte()? != 0xFF {}
            let mut next = self.byte()?;
            while next == 0xFF {
                next = self.byte()?;
            }
            if next != 0 && !(0xD0..=0xD7).contains(&next) {
                return Some(next);
            }
        }
    }

    fn u16(&mut self) -> Option<u16> {
        let (high, low) = (self.byte()?, self.byte()?);
        Some(u16::from_be_bytes([high, low]))
    }

    /// The body of a segment, after its marker, without its length.
    fn segment(&mut self, into: &mut Vec<u8>) -> Option<()> {
        let length = usize::from(self.u16()?).checked_sub(2)?;
        into.clear();
        for _ in 0..length {
            into.push(self.byte()?);
        }
        Some(())
    }
}

/// A table for the codes of one kind of symbol, as the file names it.
struct Huffman {
    /// By the first nine bits: the length of the code and its symbol,
    /// or none where the code is longer.
    quick: Vec<u16>,
    /// For each length, the largest code and the first one, and where
    /// its symbols begin.
    largest: [i32; 17],
    first: [i32; 17],
    place: [i32; 17],
    symbols: Vec<u8>,
}

impl Huffman {
    fn new(counts: &[u8], symbols: &[u8]) -> Option<Self> {
        let total: usize = counts.iter().map(|count| usize::from(*count)).sum();
        if total == 0 || total > 256 || symbols.len() < total {
            return None;
        }
        let mut table = Self {
            quick: vec![0; 512],
            largest: [-1; 17],
            first: [0; 17],
            place: [0; 17],
            symbols: symbols.get(..total)?.to_vec(),
        };
        let (mut code, mut place) = (0i32, 0i32);
        for (length, count) in (1usize..=16).zip(counts) {
            let count = i32::from(*count);
            *table.first.get_mut(length)? = code;
            *table.place.get_mut(length)? = place;
            let end = code.checked_add(count)?;
            // More codes of a length than there is room for, or as many:
            // the last of them would be all ones, which no code may be.
            let room = 1i32.checked_shl(u32::try_from(length).ok()?)?;
            if end > room || (count > 0 && end == room) {
                return None;
            }
            if count > 0 {
                *table.largest.get_mut(length)? = end.checked_sub(1)?;
            }
            if length <= 9 {
                for at in 0..count {
                    let symbol = *table.symbols.get(usize::try_from(place.checked_add(at)?).ok()?)?;
                    let low = code
                        .checked_add(at)?
                        .checked_shl(u32::try_from(9usize.checked_sub(length)?).ok()?)?;
                    let fill = 1i32.checked_shl(u32::try_from(9usize.checked_sub(length)?).ok()?)?;
                    for slot in low..low.checked_add(fill)? {
                        let entry = (u16::try_from(length).ok()?.wrapping_shl(8)) | u16::from(symbol);
                        *table.quick.get_mut(usize::try_from(slot).ok()?)? = entry;
                    }
                }
            }
            place = place.checked_add(count)?;
            code = end.checked_mul(2)?;
        }
        Some(table)
    }

    /// The symbol that begins `look`, 16 bits of the data, and how
    /// long its code is.
    fn symbol(&self, look: u32) -> Option<(u8, u32)> {
        let entry = self.quick.get(usize::try_from(look.wrapping_shr(7)).ok()?).copied().unwrap_or(0);
        if entry != 0 {
            return Some(((entry & 0xFF) as u8, u32::from(entry.wrapping_shr(8))));
        }
        for length in 10..=16u32 {
            let code = i32::try_from(look.checked_shr(16u32.checked_sub(length)?)?).ok()?;
            let at = usize::try_from(length).ok()?;
            let largest = *self.largest.get(at)?;
            if largest >= 0 && code <= largest {
                let index = self.place.get(at)?.checked_add(code.checked_sub(*self.first.get(at)?)?)?;
                return Some((*self.symbols.get(usize::try_from(index).ok()?)?, length));
            }
        }
        None
    }
}

/// The bits of the entropy-coded data, with the stuffing taken out. At
/// a marker, and past the end of the file, what comes is zeros, and
/// `starved` says that some of them were used.
struct Bits {
    acc: u64,
    count: u32,
    /// How many of the bits at the end of `acc` are not the file's.
    padded: u32,
    starved: bool,
    marker: Option<u8>,
}

impl Bits {
    fn new() -> Self {
        Self { acc: 0, count: 0, padded: 0, starved: false, marker: None }
    }

    fn fill<R: Read>(&mut self, input: &mut Input<R>) {
        while self.count <= 56 {
            let byte = if self.marker.is_some() {
                None
            } else {
                match input.byte() {
                    Some(0xFF) => {
                        let mut next = input.byte();
                        while next == Some(0xFF) {
                            next = input.byte();
                        }
                        match next {
                            Some(0) => Some(0xFF),
                            other => {
                                // A marker, or the end of the file.
                                self.marker = other;
                                None
                            }
                        }
                    }
                    other => other,
                }
            };
            self.acc = self.acc.wrapping_shl(8) | u64::from(byte.unwrap_or(0));
            self.count = self.count.saturating_add(8);
            if byte.is_none() {
                self.padded = self.padded.saturating_add(8);
            }
        }
    }

    fn peek<R: Read>(&mut self, input: &mut Input<R>, bits: u32) -> u32 {
        if self.count < bits {
            self.fill(input);
        }
        let shift = self.count.saturating_sub(bits);
        ((self.acc >> shift) & 1u64.wrapping_shl(bits).wrapping_sub(1)) as u32
    }

    fn drop(&mut self, bits: u32) {
        self.count = self.count.saturating_sub(bits);
        if self.count < self.padded {
            self.starved = true;
            self.padded = self.count;
        }
    }

    fn take<R: Read>(&mut self, input: &mut Input<R>, bits: u32) -> u32 {
        if bits == 0 {
            return 0;
        }
        let value = self.peek(input, bits);
        self.drop(bits);
        value
    }

    /// The restart marker `expected` is next: the bits that remain are
    /// padding. Returns whether it was that one.
    fn restart<R: Read>(&mut self, input: &mut Input<R>, expected: u8) -> bool {
        self.acc = 0;
        self.count = 0;
        self.padded = 0;
        let found = match self.marker.take() {
            Some(marker) => Some(marker),
            None => input.marker_raw(),
        };
        match found {
            Some(marker) if marker == expected => true,
            other => {
                self.marker = other;
                false
            }
        }
    }
}

impl<R: Read> Input<R> {
    /// The marker that is next, where nothing but padding may lie
    /// before it: one that is not at once is data that was not read.
    fn marker_raw(&mut self) -> Option<u8> {
        let mut byte = self.byte()?;
        let mut skipped = 0u32;
        while byte != 0xFF {
            skipped = skipped.saturating_add(1);
            if skipped > 64 {
                return None;
            }
            byte = self.byte()?;
        }
        let mut next = self.byte()?;
        while next == 0xFF {
            next = self.byte()?;
        }
        Some(next)
    }
}

#[derive(Clone, Copy, Debug)]
struct Component {
    id: u8,
    h: u32,
    v: u32,
    quant: usize,
}

/// The head of a frame.
struct Frame {
    w: u32,
    h: u32,
    components: Vec<Component>,
    h_max: u32,
    v_max: u32,
}

impl Frame {
    /// How many blocks the component has across and down in a scan of
    /// its own, and how many a scan of all of them takes, padded to
    /// whole units of the picture.
    fn blocks(&self, component: &Component, whole: bool) -> (u32, u32) {
        let (w, h) = self.samples(component);
        if whole {
            let across = self.w.div_ceil(self.h_max.saturating_mul(8).max(1)).saturating_mul(component.h);
            let down = self.h.div_ceil(self.v_max.saturating_mul(8).max(1)).saturating_mul(component.v);
            (across, down)
        } else {
            (w.div_ceil(8), h.div_ceil(8))
        }
    }

    /// The size of a component in samples, before it is made smaller.
    fn samples(&self, component: &Component) -> (u32, u32) {
        let scaled = |length: u32, by: u32, of: u32| {
            u32::try_from(u64::from(length).saturating_mul(u64::from(by)).div_ceil(u64::from(of.max(1))))
                .unwrap_or(u32::MAX)
        };
        (scaled(self.w, component.h, self.h_max), scaled(self.h, component.v, self.v_max))
    }
}

/// The samples of the blocks of one component that lie in the part.
struct Plane {
    /// The first block that is kept, and how many across and down.
    first: (u32, u32),
    blocks: (u32, u32),
    /// The samples of one side of a block: 8, 4, 2 or 1.
    side: usize,
    /// The sample at row `y` and column `x` lies at `y * stride + x`.
    stride: usize,
    samples: Vec<u8>,
}

impl Plane {
    fn put(&mut self, block: (u32, u32), from: &[u8]) {
        let (Some(x), Some(y)) = (block.0.checked_sub(self.first.0), block.1.checked_sub(self.first.1))
        else {
            return;
        };
        if x >= self.blocks.0 || y >= self.blocks.1 || self.side == 0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let left = x.saturating_mul(self.side);
        let top = y.saturating_mul(self.side);
        for (row, line) in from.chunks_exact(self.side).enumerate() {
            let start = top.saturating_add(row).saturating_mul(self.stride).saturating_add(left);
            if let Some(into) = self.samples.get_mut(start..start.saturating_add(self.side)) {
                into.copy_from_slice(line);
            }
        }
    }

    /// The sample at a place of the plane as a whole, held to its
    /// edges, which are where the blocks that are kept end.
    fn at(&self, x: i64, y: i64) -> f32 {
        let side = i64::try_from(self.side).unwrap_or(1);
        let ranges = |first: u32, count: u32| {
            let from = i64::from(first).saturating_mul(side);
            (from, from.saturating_add(i64::from(count).saturating_mul(side)).saturating_sub(1))
        };
        let (x0, x1) = ranges(self.first.0, self.blocks.0);
        let (y0, y1) = ranges(self.first.1, self.blocks.1);
        let x = x.clamp(x0, x1.max(x0)).saturating_sub(x0);
        let y = y.clamp(y0, y1.max(y0)).saturating_sub(y0);
        let at = usize::try_from(y)
            .unwrap_or(0)
            .saturating_mul(self.stride)
            .saturating_add(usize::try_from(x).unwrap_or(0));
        self.samples.get(at).map_or(128.0, |sample| f32::from(*sample))
    }
}

/// An inverse DCT for blocks made to `n` samples a side, by the first
/// `n` frequencies each way.
struct Idct {
    n: usize,
    /// `table[x][u]`: how much of frequency `u` is in sample `x`.
    table: [[f32; 8]; 8],
    scale: f32,
}

impl Idct {
    fn new(n: usize) -> Self {
        let mut table = [[0.0f32; 8]; 8];
        let nf = n.max(1) as f32;
        for (x, line) in table.iter_mut().enumerate().take(n) {
            for (u, cell) in line.iter_mut().enumerate().take(n) {
                let alpha = if u == 0 { (1.0 / nf).sqrt() } else { (2.0 / nf).sqrt() };
                let angle = (2.0 * x as f32 + 1.0) * u as f32 * std::f32::consts::PI / (2.0 * nf);
                *cell = alpha * angle.cos();
            }
        }
        Self { n, table, scale: nf / 8.0 }
    }

    /// Makes `out`, `n` by `n` samples, of 64 coefficients in the order
    /// of a block, already multiplied by their quantization.
    fn run(&self, coefficients: &[f32; 64], out: &mut [u8]) {
        let n = self.n;
        if coefficients.iter().skip(1).all(|coefficient| *coefficient == 0.0) {
            let flat = (coefficients.first().copied().unwrap_or(0.0) / 8.0 + 128.0).round();
            out.iter_mut().for_each(|sample| *sample = flat.clamp(0.0, 255.0) as u8);
            return;
        }
        // Rows of frequencies to rows of samples.
        let mut across = [[0.0f32; 8]; 8];
        for (row, spread) in coefficients.as_chunks::<8>().0.iter().zip(across.iter_mut()).take(n) {
            if row.iter().take(n).all(|coefficient| *coefficient == 0.0) {
                continue;
            }
            for (at, sample) in self.table.iter().zip(spread.iter_mut()).take(n) {
                *sample = at.iter().zip(row).take(n).map(|(a, b)| a * b).sum();
            }
        }
        for (at, line) in self.table.iter().zip(out.chunks_exact_mut(n.max(1))).take(n) {
            for (x, sample) in line.iter_mut().enumerate() {
                let total: f32 = at
                    .iter()
                    .zip(across.iter())
                    .take(n)
                    .map(|(a, row)| a * row.get(x).copied().unwrap_or(0.0))
                    .sum();
                *sample = (total * self.scale + 128.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Which way the camera held the picture, by what the EXIF segment
/// says, as the places a pixel goes to are told in `turn`.
fn orientation(exif: &[u8]) -> Option<u8> {
    let tiff = exif.strip_prefix(b"Exif\0\0")?;
    let little = match tiff.get(..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |at: usize| {
        let bytes: [u8; 2] = tiff.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little { u16::from_le_bytes(bytes) } else { u16::from_be_bytes(bytes) })
    };
    let u32_at = |at: usize| {
        let bytes: [u8; 4] = tiff.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little { u32::from_le_bytes(bytes) } else { u32::from_be_bytes(bytes) })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = usize::try_from(u32_at(4)?).ok()?;
    let entries = usize::from(u16_at(ifd)?);
    for entry in 0..entries.min(256) {
        let at = ifd.checked_add(2)?.checked_add(entry.checked_mul(12)?)?;
        if u16_at(at)? == 0x0112 {
            let value = u16_at(at.checked_add(8)?)?;
            return u8::try_from(value).ok().filter(|value| (1..=8).contains(value));
        }
    }
    None
}

/// Where the pixel at `(x, y)` of a picture of `w` by `h` as it is
/// stored lies when it is turned upright, and the size it then has.
fn turn(orientation: u8, (x, y): (u32, u32), (w, h): (u32, u32)) -> (u32, u32) {
    let (xr, yr) = (w.saturating_sub(1).saturating_sub(x), h.saturating_sub(1).saturating_sub(y));
    match orientation {
        2 => (xr, y),
        3 => (xr, yr),
        4 => (x, yr),
        5 => (y, x),
        6 => (yr, x),
        7 => (yr, xr),
        8 => (y, xr),
        _ => (x, y),
    }
}

/// The inverse of `turn`: where in the picture as it is stored the
/// pixel at `(u, v)` of the upright one comes from. `(w, h)` is the
/// stored size.
fn unturn(orientation: u8, (u, v): (u32, u32), (w, h): (u32, u32)) -> (u32, u32) {
    let (ur, vr) = (h.saturating_sub(1).saturating_sub(u), w.saturating_sub(1).saturating_sub(v));
    match orientation {
        2 => (w.saturating_sub(1).saturating_sub(u), v),
        3 => (w.saturating_sub(1).saturating_sub(u), h.saturating_sub(1).saturating_sub(v)),
        4 => (u, h.saturating_sub(1).saturating_sub(v)),
        5 => (v, u),
        6 => (v, ur),
        7 => (vr, ur),
        8 => (vr, u),
        _ => (u, v),
    }
}

/// How the colors of the components are to be read.
#[derive(Clone, Copy, PartialEq)]
enum Colors {
    Gray,
    YCbCr,
    Rgb,
}

/// What the scans are decoded into, planned from the head of the file
/// and what was asked.
struct Plan {
    /// The orientation, the upright part and its size, and the size of
    /// the result as it is stored, before it is turned.
    orientation: u8,
    part: [u32; 4],
    size: (u32, u32),
    stored: (u32, u32),
    /// The part as it is stored, and by how much it is made smaller.
    cut: [u32; 4],
    reduce: u32,
    planes: Vec<Plane>,
    smaller: bool,
}

/// How a block is made into samples at each reduction.
fn side_of(reduce: u32) -> usize {
    match reduce {
        2 => 4,
        4 => 2,
        8 => 1,
        _ => 8,
    }
}

fn zeroed(count: u64, of: u8) -> Result<Vec<u8>, Refused> {
    let count = usize::try_from(count).map_err(|_| Refused::Memory)?;
    let mut all = Vec::new();
    all.try_reserve_exact(count).map_err(|_| Refused::Memory)?;
    all.resize(count, of);
    Ok(all)
}

fn plan(frame: &Frame, orientation: u8, wish: &Wish) -> Result<Plan, Refused> {
    let stored = (frame.w, frame.h);
    if u64::from(stored.0).saturating_mul(u64::from(stored.1)) > PIXELS_MAX
        || stored.0 > SIDE_MAX
        || stored.1 > SIDE_MAX
    {
        return Err(Refused::Large);
    }
    let upright = if orientation >= 5 { (stored.1, stored.0) } else { stored };
    let part = wish.part_of(upright);
    let asked = fit((part[2], part[3]), wish);
    // The part as it lies in the file: the box of its two corners.
    let corner = |u: u32, v: u32| unturn(orientation, (u, v), stored);
    let (a, b) = (
        corner(part[0], part[1]),
        corner(
            part[0].saturating_add(part[2]).saturating_sub(1),
            part[1].saturating_add(part[3]).saturating_sub(1),
        ),
    );
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let cut = [
        x0,
        y0,
        a.0.max(b.0).saturating_sub(x0).saturating_add(1),
        a.1.max(b.1).saturating_sub(y0).saturating_add(1),
    ];
    let swap = orientation >= 5;
    let mut size = asked;
    loop {
        let stored_size = if swap { (size.1, size.0) } else { size };
        // The largest reduction that leaves at least as many samples as
        // the result has.
        let reduce = [8u32, 4, 2, 1]
            .into_iter()
            .find(|by| {
                cut[2].checked_div(*by).unwrap_or(0) >= stored_size.0
                    && cut[3].checked_div(*by).unwrap_or(0) >= stored_size.1
            })
            .unwrap_or(1);
        let side = side_of(reduce);
        let mut planes = Vec::new();
        let mut bytes = u64::from(stored_size.0).saturating_mul(u64::from(stored_size.1)).saturating_mul(4);
        for component in &frame.components {
            let (across, down) = frame.blocks(component, true);
            let samples = frame.samples(component);
            // The blocks that hold the samples of the part, with the one
            // beside it each way that a smooth way of making a chroma
            // plane larger looks at. That neighbor is one of the picture
            // as it is made smaller, so it is reckoned there: a sample
            // of that picture is `reduce` of the file.
            let range = |from: u32, length: u32, by: u32, of: u32, limit: u32| {
                let of = u64::from(of.max(1));
                let low = u64::from(from.checked_div(reduce).unwrap_or(0))
                    .saturating_mul(u64::from(by))
                    .checked_div(of)
                    .unwrap_or(0)
                    .saturating_sub(1);
                let high = u64::from(from.saturating_add(length).div_ceil(reduce))
                    .saturating_mul(u64::from(by))
                    .div_ceil(of);
                let per = (side as u64).max(1);
                let first = u32::try_from(low.checked_div(per).unwrap_or(0)).unwrap_or(0);
                let end =
                    u32::try_from(high.checked_div(per).unwrap_or(0).saturating_add(1)).unwrap_or(u32::MAX);
                let last = limit.div_ceil(8).min(end);
                (first.min(last), last)
            };
            let (bx0, bx1) = range(cut[0], cut[2], component.h, frame.h_max, samples.0);
            let (by0, by1) = range(cut[1], cut[3], component.v, frame.v_max, samples.1);
            let blocks = (bx1.saturating_sub(bx0).min(across), by1.saturating_sub(by0).min(down));
            let stride = (blocks.0 as usize).saturating_mul(side);
            let count = u64::from(blocks.0)
                .saturating_mul(u64::from(blocks.1))
                .saturating_mul((side as u64).saturating_mul(side as u64));
            bytes = bytes.saturating_add(count);
            planes.push(Plane { first: (bx0, by0), blocks, side, stride, samples: Vec::new() });
            if bytes > WORK {
                break;
            }
        }
        if bytes <= WORK {
            for (plane, _) in planes.iter_mut().zip(&frame.components) {
                let count = (plane.blocks.0 as u64)
                    .saturating_mul(u64::from(plane.blocks.1))
                    .saturating_mul((plane.side as u64).saturating_mul(plane.side as u64));
                plane.samples = zeroed(count, 128)?;
            }
            return Ok(Plan {
                orientation,
                part,
                size,
                stored: stored_size,
                cut,
                reduce,
                planes,
                smaller: size != asked,
            });
        }
        if size == (1, 1) {
            return Err(Refused::Large);
        }
        size = (size.0.div_ceil(2), size.1.div_ceil(2));
    }
}

/// What is read of the head of a file before its first scan, and the
/// tables it names.
struct Head {
    frame: Option<Frame>,
    quant: [Option<[u16; 64]>; 4],
    dc: [Option<Huffman>; 4],
    ac: [Option<Huffman>; 4],
    restart: u32,
    colors: Option<Colors>,
    orientation: u8,
    notes: Notes,
}

/// Reads a JPEG from `from` and makes of it the picture the wish asks
/// for.
pub fn decode(from: impl Read, wish: &Wish) -> Result<Picture, Refused> {
    let mut input = Input::new(from);
    // Begins with SOI, and then a marker.
    if input.byte() != Some(0xFF) || input.byte() != Some(0xD8) {
        return Err(Refused::Format);
    }
    let mut head = Head {
        frame: None,
        quant: [None; 4],
        dc: [None, None, None, None],
        ac: [None, None, None, None],
        restart: 0,
        colors: None,
        orientation: 1,
        notes: Notes::default(),
    };
    let mut body = Vec::new();
    let mut segments = 0u32;
    let mut state: Option<Scans> = None;
    // The marker that ended a scan and was read as data.
    let mut held: Option<u8> = None;
    let mut early = false;
    let mut damaged = false;
    loop {
        let marker = match held.take() {
            Some(marker) => marker,
            None => match input.marker() {
                Some(marker) => marker,
                None => {
                    early = true;
                    break;
                }
            },
        };
        segments = segments.saturating_add(1);
        if segments > SEGMENTS {
            return Err(Refused::Damaged);
        }
        match marker {
            0xD9 => break,
            0xD8 | 0x01 | 0xD0..=0xD7 => {}
            // Sequential, Huffman, eight bits or twelve.
            0xC0 | 0xC1 => {
                if head.frame.is_some() || state.is_some() {
                    return Err(Refused::Damaged);
                }
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                head.frame = Some(read_frame(&body)?);
            }
            0xC2 => return Err(Refused::Progressive),
            // Lossless, hierarchical, arithmetic.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xCC | 0xDE | 0xDF => {
                return Err(Refused::Coding);
            }
            0xC4 => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                read_tables(&body, &mut head)?;
            }
            0xDB => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                read_quant(&body, &mut head)?;
            }
            0xDD => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                let [high, low] = body.first_chunk::<2>().copied().ok_or(Refused::Damaged)?;
                head.restart = u32::from(u16::from_be_bytes([high, low]));
            }
            0xDA => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                let frame = head.frame.as_ref().ok_or(Refused::Damaged)?;
                if state.is_none() {
                    let colors = colors_of(&head, frame)?;
                    let planned = plan(frame, head.orientation, wish)?;
                    state = Some(Scans {
                        planned,
                        colors,
                        covered: vec![false; frame.components.len()],
                        begun: vec![false; frame.components.len()],
                        blocks: 0,
                    });
                }
                let Some(scans) = state.as_mut() else { return Err(Refused::Damaged) };
                let ended = scan(&mut input, &body, &head, scans)?;
                held = ended.marker;
                early |= ended.early;
                damaged |= ended.damaged;
                // Everything that was asked for is there. A file that
                // was read to the end of its data is whole only where
                // a marker follows: one that stops there is one that
                // is still being written.
                if scans.covered.iter().all(|done| *done) {
                    if !ended.stopped && !ended.early && !ended.damaged && held.is_none() {
                        early |= input.marker().is_none();
                    }
                    break;
                }
                if ended.early || ended.damaged {
                    // What is left of the file is not data of a scan
                    // that can be found again.
                    if held.is_none() {
                        break;
                    }
                }
            }
            0xE1 => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                if head.frame.is_none()
                    && let Some(found) = orientation(&body)
                {
                    head.orientation = found;
                }
            }
            0xE2 => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                if body.starts_with(b"ICC_PROFILE\0") {
                    head.notes.add(Notes::UNAPPLIED);
                }
            }
            0xEE => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
                if body.starts_with(b"Adobe") {
                    // Zero is what is stored as RGB, and else it is
                    // YCbCr or, with four components, YCCK.
                    head.colors = Some(if body.get(11) == Some(&0) { Colors::Rgb } else { Colors::YCbCr });
                }
            }
            _ => {
                input.segment(&mut body).ok_or(Refused::Damaged)?;
            }
        }
    }
    let Some(scans) = state else { return Err(Refused::Damaged) };
    if scans.blocks == 0 {
        return Err(Refused::Damaged);
    }
    let incomplete = scans.covered.iter().any(|done| !*done);
    let mut notes = head.notes;
    if damaged {
        notes.add(Notes::DAMAGED);
    } else if early || incomplete {
        notes.add(Notes::EARLY);
    }
    let Some(frame) = head.frame.as_ref() else { return Err(Refused::Damaged) };
    let pixels = finish(frame, &scans)?;
    if scans.planned.smaller {
        notes.add(Notes::SMALLER);
    }
    let picture = if scans.planned.orientation >= 5 { (frame.h, frame.w) } else { (frame.w, frame.h) };
    let plan = &scans.planned;
    Ok(Picture { decoded: Decoded { picture, part: plan.part, size: plan.size, notes }, pixels })
}

/// What is known while the scans are read.
struct Scans {
    planned: Plan,
    colors: Colors,
    /// Whether every block of a component that is wanted was read, and
    /// whether a scan of it was begun.
    covered: Vec<bool>,
    begun: Vec<bool>,
    /// How many blocks were read at all.
    blocks: u64,
}

fn colors_of(head: &Head, frame: &Frame) -> Result<Colors, Refused> {
    match frame.components.len() {
        1 => Ok(Colors::Gray),
        3 => {
            let ids: Vec<u8> = frame.components.iter().map(|component| component.id).collect();
            if head.colors == Some(Colors::Rgb) || ids == *b"RGB" {
                Ok(Colors::Rgb)
            } else {
                Ok(Colors::YCbCr)
            }
        }
        _ => Err(Refused::Colors),
    }
}

fn read_frame(body: &[u8]) -> Result<Frame, Refused> {
    let (precision, h, w, count) = match body {
        [precision, h1, h0, w1, w0, count, ..] => {
            (*precision, u16::from_be_bytes([*h1, *h0]), u16::from_be_bytes([*w1, *w0]), *count)
        }
        _ => return Err(Refused::Damaged),
    };
    if precision != 8 {
        return Err(Refused::Coding);
    }
    if w == 0 || h == 0 || count == 0 {
        return Err(Refused::Damaged);
    }
    if count > 4 {
        return Err(Refused::Colors);
    }
    let listed = body.get(6..).ok_or(Refused::Damaged)?;
    let mut components = Vec::new();
    for entry in listed.as_chunks::<3>().0.iter().take(usize::from(count)) {
        let [id, sampling, quant] = entry;
        let (hs, vs) = (u32::from(sampling.wrapping_shr(4)), u32::from(sampling & 15));
        if !(1..=4).contains(&hs) || !(1..=4).contains(&vs) || *quant > 3 {
            return Err(Refused::Damaged);
        }
        components.push(Component { id: *id, h: hs, v: vs, quant: usize::from(*quant) });
    }
    if components.len() != usize::from(count) {
        return Err(Refused::Damaged);
    }
    let h_max = components.iter().map(|component| component.h).max().unwrap_or(1);
    let v_max = components.iter().map(|component| component.v).max().unwrap_or(1);
    Ok(Frame { w: u32::from(w), h: u32::from(h), components, h_max, v_max })
}

fn read_quant(mut body: &[u8], head: &mut Head) -> Result<(), Refused> {
    while let Some((&info, rest)) = body.split_first() {
        let (wide, id) = (info.wrapping_shr(4), usize::from(info & 15));
        if wide > 1 || id > 3 {
            return Err(Refused::Damaged);
        }
        let size = if wide == 1 { 128 } else { 64 };
        let (table, rest) = rest.split_at_checked(size).ok_or(Refused::Damaged)?;
        let mut quant = [1u16; 64];
        for (step, value) in quant.iter_mut().zip(if wide == 1 {
            table.as_chunks::<2>().0.iter().map(|pair| u16::from_be_bytes(*pair)).collect::<Vec<_>>()
        } else {
            table.iter().map(|byte| u16::from(*byte)).collect::<Vec<_>>()
        }) {
            *step = value.max(1);
        }
        *head.quant.get_mut(id).ok_or(Refused::Damaged)? = Some(quant);
        body = rest;
    }
    Ok(())
}

fn read_tables(mut body: &[u8], head: &mut Head) -> Result<(), Refused> {
    while let Some((&info, rest)) = body.split_first() {
        let (class, id) = (info.wrapping_shr(4), usize::from(info & 15));
        if class > 1 || id > 3 {
            return Err(Refused::Damaged);
        }
        let (counts, rest) = rest.split_at_checked(16).ok_or(Refused::Damaged)?;
        let total: usize = counts.iter().map(|count| usize::from(*count)).sum();
        let (symbols, rest) = rest.split_at_checked(total).ok_or(Refused::Damaged)?;
        let table = Huffman::new(counts, symbols).ok_or(Refused::Damaged)?;
        let slot = if class == 0 { head.dc.get_mut(id) } else { head.ac.get_mut(id) };
        *slot.ok_or(Refused::Damaged)? = Some(table);
        body = rest;
    }
    Ok(())
}

/// How a scan ended.
struct Ended {
    /// The marker that was met in its data, if one was.
    marker: Option<u8>,
    early: bool,
    damaged: bool,
    /// It was not read to its end, because nothing below was wanted.
    stopped: bool,
}

/// The component of a scan as it is read.
struct InScan {
    index: usize,
    dc: usize,
    ac: usize,
    previous: i32,
}

/// Reads one scan, into the planes.
fn scan<R: Read>(
    input: &mut Input<R>,
    body: &[u8],
    head: &Head,
    scans: &mut Scans,
) -> Result<Ended, Refused> {
    let frame = head.frame.as_ref().ok_or(Refused::Damaged)?;
    let (&count, rest) = body.split_first().ok_or(Refused::Damaged)?;
    let count = usize::from(count);
    if count == 0 || count > 4 {
        return Err(Refused::Damaged);
    }
    let (listed, rest) = rest.split_at_checked(count.saturating_mul(2)).ok_or(Refused::Damaged)?;
    let (spectral, _) = rest.split_at_checked(3).ok_or(Refused::Damaged)?;
    // Sequential: all of the block, with nothing refined.
    if spectral != [0, 63, 0] {
        return Err(Refused::Damaged);
    }
    let mut members = Vec::new();
    for pair in listed.as_chunks::<2>().0 {
        let [id, tables] = pair;
        let index =
            frame.components.iter().position(|component| component.id == *id).ok_or(Refused::Damaged)?;
        if members.iter().any(|member: &InScan| member.index == index) {
            return Err(Refused::Damaged);
        }
        let (dc, ac) = (usize::from(tables.wrapping_shr(4)), usize::from(tables & 15));
        if dc > 3 || ac > 3 {
            return Err(Refused::Damaged);
        }
        members.push(InScan { index, dc, ac, previous: 0 });
    }
    // A component has one scan. A file that names one again would be
    // read for as long as it goes on: it is damaged, and what was read
    // is what there is.
    if members.iter().any(|member| scans.begun.get(member.index).copied().unwrap_or(true)) {
        return Ok(Ended { marker: None, early: false, damaged: true, stopped: false });
    }
    for member in &members {
        if let Some(begun) = scans.begun.get_mut(member.index) {
            *begun = true;
        }
    }
    let whole = members.len() > 1;
    // What a unit of a scan of several components holds at the most.
    let held: u32 = members
        .iter()
        .filter_map(|member| frame.components.get(member.index))
        .map(|component| component.h.saturating_mul(component.v))
        .sum();
    if whole && held > 10 {
        return Err(Refused::Damaged);
    }
    // The units of the scan: the blocks that stand together in one
    // unit of the picture, or the blocks of one component in order.
    let (across, down) = match members.first() {
        Some(first) if !whole => {
            let component = frame.components.get(first.index).ok_or(Refused::Damaged)?;
            frame.blocks(component, false)
        }
        _ => (
            frame.w.div_ceil(frame.h_max.saturating_mul(8).max(1)),
            frame.h.div_ceil(frame.v_max.saturating_mul(8).max(1)),
        ),
    };
    let block_units = u64::from(across).saturating_mul(u64::from(down));
    // The decoding tables of each member and its quantization are held
    // as they are now: a file may redefine them between scans.
    let mut quants = Vec::new();
    for member in &members {
        let component = frame.components.get(member.index).ok_or(Refused::Damaged)?;
        let quant = head.quant.get(component.quant).and_then(|quant| *quant).ok_or(Refused::Damaged)?;
        quants.push(quant);
        head.dc.get(member.dc).and_then(Option::as_ref).ok_or(Refused::Damaged)?;
        head.ac.get(member.ac).and_then(Option::as_ref).ok_or(Refused::Damaged)?;
    }
    // Up to which unit row anything that is wanted lies.
    let last_row = {
        let wanted = |plane: &Plane, component: &Component| {
            let bottom = plane.first.1.saturating_add(plane.blocks.1);
            if whole { bottom.div_ceil(component.v.max(1)) } else { bottom }
        };
        members
            .iter()
            .filter_map(|member| {
                let plane = scans.planned.planes.get(member.index)?;
                let component = frame.components.get(member.index)?;
                Some(wanted(plane, component))
            })
            .max()
            .unwrap_or(0)
    };
    let idct = Idct::new(side_of(scans.planned.reduce));
    let side = idct.n;
    let mut bits = Bits::new();
    let mut ended = Ended { marker: None, early: false, damaged: false, stopped: false };
    let mut coefficients = [0.0f32; 64];
    let mut samples = vec![0u8; side.saturating_mul(side)];
    let mut unit = 0u64;
    let mut stopped = false;
    let mut restarts = 0u32;
    'rows: for row in 0..down {
        if row >= last_row {
            // Nothing below is wanted.
            stopped = true;
            break;
        }
        for column in 0..across {
            if head.restart > 0 && unit > 0 && unit.checked_rem(u64::from(head.restart)) == Some(0) {
                // They count from 0 to 7 and around.
                let expected = 0xD0u8.saturating_add((restarts & 7) as u8);
                restarts = restarts.wrapping_add(1);
                if !bits.restart(input, expected) {
                    // Another marker, or data, where it should be is
                    // damage; the end of the file is an end.
                    ended.early = bits.marker.is_none() && input.done;
                    ended.damaged = !ended.early;
                    break 'rows;
                }
                members.iter_mut().for_each(|member| member.previous = 0);
            }
            for (member, quant) in members.iter_mut().zip(&quants) {
                let component = *frame.components.get(member.index).ok_or(Refused::Damaged)?;
                let (dc, ac) = (
                    head.dc.get(member.dc).and_then(Option::as_ref).ok_or(Refused::Damaged)?,
                    head.ac.get(member.ac).and_then(Option::as_ref).ok_or(Refused::Damaged)?,
                );
                let (h, v) = if whole { (component.h, component.v) } else { (1, 1) };
                for by in 0..v {
                    for bx in 0..h {
                        if !block(input, &mut bits, dc, ac, &mut member.previous, quant, &mut coefficients) {
                            // The data ran out at the end of the
                            // file: it ends early. A code that is
                            // none, and a marker in the middle of a
                            // block, are damage.
                            ended.early = bits.starved && bits.marker.is_none();
                            ended.damaged = !ended.early;
                            break 'rows;
                        }
                        scans.blocks = scans.blocks.saturating_add(1);
                        let place = if whole {
                            (
                                column.saturating_mul(component.h).saturating_add(bx),
                                row.saturating_mul(component.v).saturating_add(by),
                            )
                        } else {
                            (column, row)
                        };
                        let Some(plane) = scans.planned.planes.get_mut(member.index) else { continue };
                        let inside = place.0 >= plane.first.0
                            && place.0 < plane.first.0.saturating_add(plane.blocks.0)
                            && place.1 >= plane.first.1
                            && place.1 < plane.first.1.saturating_add(plane.blocks.1);
                        if inside {
                            idct.run(&coefficients, &mut samples);
                            plane.put(place, &samples);
                        }
                    }
                }
            }
            unit = unit.saturating_add(1);
        }
    }
    if stopped || unit >= block_units {
        for member in &members {
            if let Some(done) = scans.covered.get_mut(member.index) {
                *done = true;
            }
        }
    }
    // The bits are read ahead: a marker that was met there is the next
    // one of the file, also where the scan stopped above it.
    ended.marker = bits.marker;
    ended.stopped = stopped;
    Ok(ended)
}

/// Reads one block into `out`, dequantized, in the order of a block.
/// Returns whether it was read: not where a code is none, or the data
/// had run out.
fn block<R: Read>(
    input: &mut Input<R>,
    bits: &mut Bits,
    dc: &Huffman,
    ac: &Huffman,
    previous: &mut i32,
    quant: &[u16; 64],
    out: &mut [f32; 64],
) -> bool {
    out.fill(0.0);
    let extend = |value: u32, size: u32| -> i32 {
        let value = value as i32;
        if size == 0 {
            0
        } else if value < 1i32.wrapping_shl(size.saturating_sub(1)) {
            value.wrapping_sub(1i32.wrapping_shl(size).wrapping_sub(1))
        } else {
            value
        }
    };
    let look = bits.peek(input, 16);
    let Some((size, length)) = dc.symbol(look) else { return false };
    bits.drop(length);
    if size > 11 {
        return false;
    }
    let value = bits.take(input, u32::from(size));
    *previous = previous.wrapping_add(extend(value, u32::from(size)));
    let step = f32::from(quant.first().copied().unwrap_or(1));
    if let Some(first) = out.first_mut() {
        *first = *previous as f32 * step;
    }
    let mut k = 1usize;
    while k < 64 {
        let look = bits.peek(input, 16);
        let Some((symbol, length)) = ac.symbol(look) else { return false };
        bits.drop(length);
        let (run, size) = (usize::from(symbol.wrapping_shr(4)), u32::from(symbol & 15));
        if size == 0 {
            match run {
                15 => {
                    k = k.saturating_add(16);
                    continue;
                }
                0 => break,
                // Runs of ends are of a progressive file.
                _ => return false,
            }
        }
        // More bits than a value of eight bits has.
        if size > 10 {
            return false;
        }
        k = k.saturating_add(run);
        if k > 63 {
            return false;
        }
        let value = extend(bits.take(input, size), size);
        let place = ZIGZAG.get(k).map_or(0, |place| usize::from(*place));
        let step = f32::from(quant.get(k).copied().unwrap_or(1));
        if let Some(slot) = out.get_mut(place) {
            *slot = value as f32 * step;
        }
        k = k.saturating_add(1);
    }
    !bits.starved
}

/// Makes the result of the planes: the colors of every sample of the
/// part, each result pixel the mean of what falls into it, and then
/// turned upright.
fn finish(frame: &Frame, scans: &Scans) -> Result<Vec<u8>, Refused> {
    let planned = &scans.planned;
    let reduce = planned.reduce.max(1);
    let (sw, sh) = (planned.stored.0 as usize, planned.stored.1 as usize);
    // The part in samples of the reduced picture.
    let (x0, y0) =
        (planned.cut[0].checked_div(reduce).unwrap_or(0), planned.cut[1].checked_div(reduce).unwrap_or(0));
    let x1 = planned.cut[0].saturating_add(planned.cut[2]).div_ceil(reduce);
    let y1 = planned.cut[1].saturating_add(planned.cut[3]).div_ceil(reduce);
    let (cw, ch) = (x1.saturating_sub(x0).max(1), y1.saturating_sub(y0).max(1));
    let field = |at: u32, length: u32, into: usize| -> usize {
        let at = u64::from(at).saturating_mul(into as u64).checked_div(u64::from(length.max(1))).unwrap_or(0);
        (at as usize).min(into.saturating_sub(1))
    };
    let columns: Vec<usize> = (0..cw).map(|x| field(x, cw, sw)).collect();
    let mut per_column = vec![0u32; sw];
    columns.iter().for_each(|column| {
        if let Some(count) = per_column.get_mut(*column) {
            *count = count.saturating_add(1);
        }
    });
    // The result, upright: each pixel is written to the place the
    // orientation says, and no second picture is made to turn it.
    let mut upright = zeroed((sw as u64).saturating_mul(sh as u64).saturating_mul(4), 255)?;
    let across = if planned.orientation >= 5 { sh } else { sw };
    let (orientation, stored_size) = (planned.orientation, planned.stored);
    let mut sums = vec![[0u32; 3]; sw];
    let mut current: Option<usize> = None;
    let mut rows_in = 0u32;
    let settle = |sums: &mut Vec<[u32; 3]>, row: usize, rows_in: u32, upright: &mut [u8]| {
        for (column, (sum, per)) in sums.iter().zip(&per_column).enumerate() {
            let count = per.saturating_mul(rows_in).max(1);
            let (u, v) = turn(orientation, (column as u32, row as u32), stored_size);
            let at = (v as usize).saturating_mul(across).saturating_add(u as usize).saturating_mul(4);
            let Some(pixel) = upright.get_mut(at..at.saturating_add(3)) else { continue };
            for (channel, total) in pixel.iter_mut().zip(sum) {
                *channel =
                    total.saturating_add(count.wrapping_shr(1)).checked_div(count).unwrap_or(0).min(255)
                        as u8;
            }
        }
        sums.iter_mut().for_each(|sum| *sum = [0; 3]);
    };
    let mut line = vec![[0u8; 3]; cw as usize];
    let mut samples = vec![[0.0f32; 3]; cw as usize];
    for y in y0..y0.saturating_add(ch) {
        let row = field(y.saturating_sub(y0), ch, sh);
        if current.is_some_and(|before| before != row) {
            if let Some(before) = current {
                settle(&mut sums, before, rows_in, &mut upright);
            }
            rows_in = 0;
        }
        current = Some(row);
        rows_in = rows_in.saturating_add(1);
        colors_of_row(frame, scans, x0, y, &mut samples, &mut line);
        for (pixel, column) in line.iter().zip(&columns) {
            if let Some(sum) = sums.get_mut(*column) {
                for (total, channel) in sum.iter_mut().zip(pixel) {
                    *total = total.saturating_add(u32::from(*channel));
                }
            }
        }
    }
    if let Some(before) = current {
        settle(&mut sums, before, rows_in, &mut upright);
    }
    Ok(upright)
}

/// The colors of the row `y` of the reduced picture, from the column
/// `x0`, for as many as `line` has. `rows` is room for the samples of
/// as many, made once for all rows.
fn colors_of_row(frame: &Frame, scans: &Scans, x0: u32, y: u32, rows: &mut [[f32; 3]], line: &mut [[u8; 3]]) {
    let reduce = scans.planned.reduce.max(1);
    for (index, component) in frame.components.iter().enumerate() {
        let Some(plane) = scans.planned.planes.get(index) else { continue };
        // Where a sample of the picture lies in this component, as the
        // centers of both are placed: smooth between two neighbors where
        // the component has fewer samples.
        let (sx, sy) =
            (component.h as f32 / frame.h_max.max(1) as f32, component.v as f32 / frame.v_max.max(1) as f32);
        let cy = (y as f32 + 0.5) * sy - 0.5;
        let (y_low, y_frac) = (cy.floor(), cy - cy.floor());
        let samples = frame.samples(component);
        let limit = |length: u32| i64::from(length.div_ceil(reduce)).saturating_sub(1);
        let (max_x, max_y) = (limit(samples.0), limit(samples.1));
        for (at, out) in rows.iter_mut().enumerate() {
            let x = x0.saturating_add(at as u32);
            let cx = (x as f32 + 0.5) * sx - 0.5;
            let (x_low, x_frac) = (cx.floor(), cx - cx.floor());
            let sample = |dx: i64, dy: i64| {
                let px = (x_low as i64).saturating_add(dx).clamp(0, max_x);
                let py = (y_low as i64).saturating_add(dy).clamp(0, max_y);
                plane.at(px, py)
            };
            let value = if (sx - 1.0).abs() < f32::EPSILON && (sy - 1.0).abs() < f32::EPSILON {
                sample(0, 0)
            } else {
                let top = sample(0, 0) * (1.0 - x_frac) + sample(1, 0) * x_frac;
                let bottom = sample(0, 1) * (1.0 - x_frac) + sample(1, 1) * x_frac;
                top * (1.0 - y_frac) + bottom * y_frac
            };
            if let Some(slot) = out.get_mut(index) {
                *slot = value;
            }
        }
    }
    for (pixel, [first, second, third]) in line.iter_mut().zip(rows.iter().copied()) {
        let to_byte = |value: f32| value.round().clamp(0.0, 255.0) as u8;
        *pixel = match scans.colors {
            Colors::Gray => [to_byte(first); 3],
            Colors::Rgb => [to_byte(first), to_byte(second), to_byte(third)],
            Colors::YCbCr => {
                let (cb, cr) = (second - 128.0, third - 128.0);
                [
                    to_byte(first + 1.402 * cr),
                    to_byte(first - 0.344_136 * cb - 0.714_136 * cr),
                    to_byte(first + 1.772 * cb),
                ]
            }
        };
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::core::picture::Format;
    use crate::picture::wire::WHOLE;

    /// Writes bits, most significant first, with the stuffing.
    struct Writer {
        out: Vec<u8>,
        acc: u32,
        count: u32,
    }

    impl Writer {
        fn put(&mut self, code: u32, length: u32) {
            for at in (0..length).rev() {
                self.acc = (self.acc << 1) | ((code >> at) & 1);
                self.count += 1;
                if self.count == 8 {
                    self.out.push(self.acc as u8);
                    if self.acc as u8 == 0xFF {
                        self.out.push(0);
                    }
                    (self.acc, self.count) = (0, 0);
                }
            }
        }

        fn pad(&mut self) {
            while self.count != 0 {
                self.put(1, 1);
            }
        }
    }

    /// A table as a file names it: how many codes of each length, and the
    /// symbols in the order of the codes.
    #[derive(Clone)]
    struct Spec {
        counts: [u8; 16],
        symbols: Vec<u8>,
    }

    impl Spec {
        /// Every symbol with a code of one length.
        fn flat(count: u8, length: usize) -> Self {
            let mut counts = [0; 16];
            counts[length - 1] = count;
            Self { counts, symbols: (0..count).collect() }
        }

        /// Sixteen short codes, and the rest long ones, which the quick
        /// table of a decoder does not hold.
        fn skewed(short: usize, long: u8, of: u8) -> Self {
            let mut counts = [0; 16];
            counts[short - 1] = of;
            counts[10] = long;
            Self { counts, symbols: (0..u16::from(of) + u16::from(long)).map(|n| n as u8).collect() }
        }

        /// The code and its length for every symbol, canonically.
        fn codes(&self) -> Vec<(u8, u32, u32)> {
            let (mut code, mut at, mut all) = (0u32, 0usize, Vec::new());
            for (length, count) in self.counts.iter().enumerate() {
                for _ in 0..*count {
                    all.push((self.symbols[at], code, length as u32 + 1));
                    code += 1;
                    at += 1;
                }
                code <<= 1;
            }
            all
        }

        fn segment(&self, class: u8, id: u8) -> Vec<u8> {
            [vec![class << 4 | id], self.counts.to_vec(), self.symbols.clone()].concat()
        }
    }

    #[derive(Clone)]
    struct Setup {
        w: u32,
        h: u32,
        /// Id, horizontal and vertical sampling of each component.
        components: Vec<(u8, u32, u32)>,
        restart: u32,
        interleaved: bool,
        skewed: bool,
        /// Segments between SOI and the tables, marker and body.
        extra: Vec<(u8, Vec<u8>)>,
        marker: u8,
        precision: u8,
        /// The symbol that ends a block: zero, unless a test wants a
        /// file that is wrong there.
        eob: u8,
    }

    impl Setup {
        fn new(w: u32, h: u32, components: &[(u8, u32, u32)]) -> Self {
            Self {
                w,
                h,
                components: components.to_vec(),
                restart: 0,
                interleaved: true,
                skewed: false,
                extra: Vec::new(),
                marker: 0xC0,
                precision: 8,
                eob: 0,
            }
        }
    }

    fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
        let length = (body.len() + 2) as u16;
        [&[0xFF, marker][..], &length.to_be_bytes(), body].concat()
    }

    /// A JPEG of these planes, each at the size of its component and
    /// with every quantization step one, so that what comes back is
    /// what went in, within the rounding.
    fn encode(setup: &Setup, planes: &[Vec<u8>]) -> Vec<u8> {
        let h_max = setup.components.iter().map(|c| c.1).max().unwrap();
        let v_max = setup.components.iter().map(|c| c.2).max().unwrap();
        let (dc, ac) = if setup.skewed {
            (Spec::skewed(5, 12, 0), Spec::skewed(5, 239, 16))
        } else {
            (Spec::flat(12, 4), Spec::flat(255, 8))
        };
        // A skewed table for DC has all its codes long.
        let dc = if setup.skewed {
            let mut counts = [0; 16];
            counts[10] = 12;
            Spec { counts, symbols: (0..12).collect() }
        } else {
            dc
        };
        let (dc_codes, ac_codes) = (dc.codes(), ac.codes());
        let code_of = |codes: &[(u8, u32, u32)], symbol: u8| {
            let (_, code, length) = *codes.iter().find(|(s, ..)| *s == symbol).unwrap();
            (code, length)
        };
        let mut file = vec![0xFF, 0xD8];
        for (marker, body) in &setup.extra {
            file.extend(segment(*marker, body));
        }
        file.extend(segment(0xDB, &[vec![0], vec![1; 64]].concat()));
        let mut frame = vec![setup.precision];
        frame.extend((setup.h as u16).to_be_bytes());
        frame.extend((setup.w as u16).to_be_bytes());
        frame.push(setup.components.len() as u8);
        for (id, h, v) in &setup.components {
            frame.extend([*id, (*h << 4 | *v) as u8, 0]);
        }
        file.extend(segment(setup.marker, &frame));
        file.extend(segment(0xC4, &dc.segment(0, 0)));
        file.extend(segment(0xC4, &ac.segment(1, 0)));
        if setup.restart > 0 {
            file.extend(segment(0xDD, &(setup.restart as u16).to_be_bytes()));
        }
        // The forward transform of the block at (bx, by) of a component,
        // the samples held at the edges where the plane ends.
        let block = |index: usize, bx: u32, by: u32| -> [i32; 64] {
            let (_, h, v) = setup.components[index];
            let cw = (setup.w * h).div_ceil(h_max);
            let ch = (setup.h * v).div_ceil(v_max);
            let plane = &planes[index];
            let sample =
                |x: u32, y: u32| f64::from(plane[(y.min(ch - 1) * cw + x.min(cw - 1)) as usize]) - 128.0;
            let mut out = [0i32; 64];
            for (k, slot) in out.iter_mut().enumerate() {
                let (u, w) = ((k % 8) as f64, (k / 8) as f64);
                let c = |n: f64| if n == 0.0 { 1.0 / 2f64.sqrt() } else { 1.0 };
                let mut sum = 0.0;
                for y in 0..8u32 {
                    for x in 0..8u32 {
                        sum += sample(bx * 8 + x, by * 8 + y)
                            * ((2.0 * f64::from(x) + 1.0) * u * std::f64::consts::PI / 16.0).cos()
                            * ((2.0 * f64::from(y) + 1.0) * w * std::f64::consts::PI / 16.0).cos();
                    }
                }
                *slot = (sum * c(u) * c(w) / 4.0).round() as i32;
            }
            out
        };
        let write_block = |writer: &mut Writer, coefficients: &[i32; 64], previous: &mut i32| {
            let size = |value: i32| 32 - value.unsigned_abs().leading_zeros();
            let bits = |value: i32, size: u32| -> u32 {
                if value < 0 { (value - 1) as u32 & ((1 << size) - 1) } else { value as u32 }
            };
            let diff = coefficients[0] - *previous;
            *previous = coefficients[0];
            let s = size(diff);
            let (code, length) = code_of(&dc_codes, s as u8);
            writer.put(code, length);
            writer.put(bits(diff, s), s);
            let mut run = 0;
            for k in 1..64 {
                let value = coefficients[ZIGZAG[k] as usize];
                if value == 0 {
                    run += 1;
                    continue;
                }
                while run > 15 {
                    let (code, length) = code_of(&ac_codes, 0xF0);
                    writer.put(code, length);
                    run -= 16;
                }
                let s = size(value);
                let (code, length) = code_of(&ac_codes, (run << 4 | s) as u8);
                writer.put(code, length);
                writer.put(bits(value, s), s);
                run = 0;
            }
            if run > 0 {
                let (code, length) = code_of(&ac_codes, setup.eob);
                writer.put(code, length);
            }
        };
        let scan_header = |members: &[usize]| {
            let mut body = vec![members.len() as u8];
            for member in members {
                body.extend([setup.components[*member].0, 0]);
            }
            body.extend([0, 63, 0]);
            segment(0xDA, &body)
        };
        let run = |file: &mut Vec<u8>, members: &[usize], whole: bool| {
            file.extend(scan_header(members));
            let mut writer = Writer { out: Vec::new(), acc: 0, count: 0 };
            let mut previous = vec![0i32; members.len()];
            let (across, down) = if whole {
                (setup.w.div_ceil(8 * h_max), setup.h.div_ceil(8 * v_max))
            } else {
                let (_, h, v) = setup.components[members[0]];
                ((setup.w * h).div_ceil(h_max).div_ceil(8), (setup.h * v).div_ceil(v_max).div_ceil(8))
            };
            let mut unit = 0u32;
            let mut restarts = 0u8;
            for row in 0..down {
                for column in 0..across {
                    if setup.restart > 0 && unit > 0 && unit.is_multiple_of(setup.restart) {
                        writer.pad();
                        writer.out.extend([0xFF, 0xD0 + (restarts & 7)]);
                        restarts += 1;
                        previous.iter_mut().for_each(|p| *p = 0);
                    }
                    for (slot, member) in members.iter().enumerate() {
                        let (_, h, v) = setup.components[*member];
                        let (h, v) = if whole { (h, v) } else { (1, 1) };
                        for by in 0..v {
                            for bx in 0..h {
                                let at = if whole { (column * h + bx, row * v + by) } else { (column, row) };
                                let coefficients = block(*member, at.0, at.1);
                                write_block(&mut writer, &coefficients, &mut previous[slot]);
                            }
                        }
                    }
                    unit += 1;
                }
            }
            writer.pad();
            file.extend(writer.out);
        };
        let all: Vec<usize> = (0..setup.components.len()).collect();
        if setup.interleaved || all.len() == 1 {
            run(&mut file, &all, all.len() > 1);
        } else {
            for member in &all {
                run(&mut file, &[*member], false);
            }
        }
        file.extend([0xFF, 0xD9]);
        file
    }

    /// A smooth picture of three channels.
    fn picture(w: u32, h: u32) -> Vec<[u8; 3]> {
        (0..h)
            .flat_map(|y| (0..w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let (x, y) = (f64::from(x), f64::from(y));
                let channel = |phase: f64, scale: f64| {
                    (128.0 + 90.0 * (x / scale + phase).sin() * (y / (scale + 3.0) - phase).cos()) as u8
                };
                [channel(0.0, 7.0), channel(1.3, 9.0), channel(2.6, 11.0)]
            })
            .collect()
    }

    /// The planes for a setup: the colors made into Y, Cb and Cr and
    /// those that are sampled less often made the mean of what they
    /// stand for.
    fn planes_of(setup: &Setup, rgb: &[[u8; 3]]) -> Vec<Vec<u8>> {
        let (w, h) = (setup.w as usize, setup.h as usize);
        let h_max = setup.components.iter().map(|c| c.1).max().unwrap() as usize;
        let v_max = setup.components.iter().map(|c| c.2).max().unwrap() as usize;
        let gray = setup.components.len() == 1;
        let channel = |index: usize, at: usize| -> f64 {
            let [r, g, b] = rgb[at].map(f64::from);
            match (gray, index) {
                (true, _) => r,
                (_, 0) => 0.299 * r + 0.587 * g + 0.114 * b,
                (_, 1) => 128.0 - 0.168_736 * r - 0.331_264 * g + 0.5 * b,
                _ => 128.0 + 0.5 * r - 0.418_688 * g - 0.081_312 * b,
            }
        };
        setup
            .components
            .iter()
            .enumerate()
            .map(|(index, (_, ch, cv))| {
                let (fx, fy) = (h_max / *ch as usize, v_max / *cv as usize);
                let (cw, chh) = ((w * *ch as usize).div_ceil(h_max), (h * *cv as usize).div_ceil(v_max));
                (0..chh)
                    .flat_map(|y| (0..cw).map(move |x| (x, y)))
                    .map(|(x, y)| {
                        let mut sum = 0.0;
                        let mut count = 0.0;
                        for dy in 0..fy {
                            for dx in 0..fx {
                                let (sx, sy) = ((x * fx + dx).min(w - 1), (y * fy + dy).min(h - 1));
                                sum += channel(index, sy * w + sx);
                                count += 1.0;
                            }
                        }
                        (sum / count).round().clamp(0.0, 255.0) as u8
                    })
                    .collect()
            })
            .collect()
    }

    fn wish(room: (u32, u32), part: [u32; 4]) -> Wish {
        Wish { format: Format::Jpeg, room, side: 4096, whole: 0, part }
    }

    fn whole() -> Wish {
        Wish { whole: WHOLE, ..wish((16, 16), [0; 4]) }
    }

    fn seen(bytes: &[u8], wish: &Wish) -> Picture {
        decode(bytes, wish).unwrap_or_else(|reason| panic!("turned away: {reason:?}"))
    }

    fn rgb_of(picture: &Picture) -> Vec<[u8; 3]> {
        picture.pixels.as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]]).collect()
    }

    fn worst(a: &[[u8; 3]], b: &[[u8; 3]]) -> u8 {
        assert_eq!(a.len(), b.len());
        a.iter().zip(b).flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| a.abs_diff(*b))).max().unwrap_or(0)
    }

    const YCC: [(u8, u32, u32); 3] = [(1, 1, 1), (2, 1, 1), (3, 1, 1)];

    fn color(setup: &Setup) -> (Vec<[u8; 3]>, Vec<u8>) {
        let rgb = picture(setup.w, setup.h);
        let planes = planes_of(setup, &rgb);
        (rgb, encode(setup, &planes))
    }

    #[test]
    fn a_gray_picture_comes_back_as_it_went_in() {
        // Not a multiple of eight either way.
        let setup = Setup::new(13, 11, &[(1, 1, 1)]);
        let (rgb, bytes) = color(&setup);
        let got = seen(&bytes, &whole());
        assert_eq!((got.decoded.picture, got.decoded.size), ((13, 11), (13, 11)));
        assert_eq!(got.decoded.part, [0, 0, 13, 11]);
        assert!(got.pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 255 && p[0] == p[1] && p[1] == p[2]));
        let want: Vec<[u8; 3]> = rgb.iter().map(|[r, ..]| [*r; 3]).collect();
        assert!(worst(&rgb_of(&got), &want) <= 1);
        assert_eq!(got.decoded.notes, Notes(0));
    }

    #[test]
    fn colors_come_back_for_every_way_of_sampling() {
        for (name, components) in [
            ("4:4:4", vec![(1, 1, 1), (2, 1, 1), (3, 1, 1)]),
            ("4:2:2", vec![(1, 2, 1), (2, 1, 1), (3, 1, 1)]),
            ("4:2:0", vec![(1, 2, 2), (2, 1, 1), (3, 1, 1)]),
            ("4:4:0", vec![(1, 1, 2), (2, 1, 1), (3, 1, 1)]),
            ("4:1:1", vec![(1, 4, 1), (2, 1, 1), (3, 1, 1)]),
        ] {
            let setup = Setup::new(45, 37, &components);
            let (rgb, bytes) = color(&setup);
            let got = seen(&bytes, &whole());
            assert_eq!(got.decoded.size, (45, 37), "{name}");
            let error = worst(&rgb_of(&got), &rgb);
            // The mean of what a chroma sample stands for, made larger
            // smoothly, is near a picture that does not change fast.
            assert!(error <= if name == "4:4:4" { 3 } else { 14 }, "{name}: {error}");
        }
    }

    #[test]
    fn the_way_the_data_is_laid_out_does_not_change_the_picture() {
        let setup = Setup::new(40, 34, &[(1, 2, 2), (2, 1, 1), (3, 1, 1)]);
        let (_, plain) = color(&setup);
        let base = rgb_of(&seen(&plain, &whole()));
        // Restart markers at every interval, also one that does not
        // divide the units.
        for restart in [1, 2, 3, 7] {
            let (_, bytes) = color(&Setup { restart, ..setup.clone() });
            assert_eq!(rgb_of(&seen(&bytes, &whole())), base, "restart {restart}");
        }
        // One scan for each component.
        let (_, bytes) = color(&Setup { interleaved: false, ..setup.clone() });
        assert_eq!(rgb_of(&seen(&bytes, &whole())), base);
        let (_, bytes) = color(&Setup { interleaved: false, restart: 5, ..setup.clone() });
        assert_eq!(rgb_of(&seen(&bytes, &whole())), base);
        // Codes the quick table of the decoder does not hold.
        let (_, bytes) = color(&Setup { skewed: true, ..setup.clone() });
        assert_eq!(rgb_of(&seen(&bytes, &whole())), base);
        // The other kind that is not progressive.
        let (_, bytes) = color(&Setup { marker: 0xC1, ..setup });
        assert_eq!(rgb_of(&seen(&bytes, &whole())), base);
    }

    /// The segment that holds an orientation, in either order of bytes.
    fn exif(orientation: u16, little: bool) -> (u8, Vec<u8>) {
        let word = |value: u16| if little { value.to_le_bytes() } else { value.to_be_bytes() };
        let long = |value: u32| if little { value.to_le_bytes() } else { value.to_be_bytes() };
        let body = [
            &b"Exif\0\0"[..],
            if little { b"II" } else { b"MM" },
            &word(42),
            &long(8),
            &word(2),
            // A tag that is none of ours, and the orientation.
            &[word(0x010F)[..].to_vec(), word(2).to_vec(), long(1).to_vec(), vec![b'x', 0, 0, 0]].concat(),
            &[
                word(0x0112)[..].to_vec(),
                word(3).to_vec(),
                long(1).to_vec(),
                word(orientation).to_vec(),
                vec![0, 0],
            ]
            .concat(),
            &long(0),
        ]
        .concat();
        (0xE1, body)
    }

    #[test]
    fn a_picture_is_turned_upright_by_its_orientation() {
        let setup = Setup::new(29, 18, &YCC);
        let (_, plain) = color(&setup);
        let base = seen(&plain, &whole());
        for orientation in 1..=8u16 {
            for little in [true, false] {
                let with = Setup { extra: vec![exif(orientation, little)], ..setup.clone() };
                let (_, bytes) = color(&with);
                let got = seen(&bytes, &whole());
                let swap = orientation >= 5;
                let size = if swap { (18, 29) } else { (29, 18) };
                assert_eq!((got.decoded.picture, got.decoded.size), (size, size), "{orientation}");
                let mut want = vec![0u8; base.pixels.len()];
                for y in 0..18u32 {
                    for x in 0..29u32 {
                        let (u, v) = turn(orientation as u8, (x, y), (29, 18));
                        let from = ((y * 29 + x) * 4) as usize;
                        let to = ((v * size.0 + u) * 4) as usize;
                        want[to..to + 4].copy_from_slice(&base.pixels[from..from + 4]);
                    }
                }
                assert_eq!(got.pixels, want, "orientation {orientation}");
            }
        }
        // What is no orientation is none.
        for value in [0u16, 9, 300] {
            let (_, bytes) = color(&Setup { extra: vec![exif(value, true)], ..setup.clone() });
            assert_eq!(seen(&bytes, &whole()).pixels, base.pixels);
        }
        // `unturn` is the way back.
        for orientation in 1..=8u8 {
            let upright = if orientation >= 5 { (18, 29) } else { (29, 18) };
            for (x, y) in [(0, 0), (28, 17), (5, 11), (28, 0)] {
                let (u, v) = turn(orientation, (x, y), (29, 18));
                assert!(u < upright.0 && v < upright.1);
                assert_eq!(unturn(orientation, (u, v), (29, 18)), (x, y), "{orientation}");
            }
        }
    }

    #[test]
    fn a_part_is_the_pixels_of_it_at_its_own_size_also_where_it_is_turned() {
        let setup = Setup::new(64, 48, &[(1, 2, 2), (2, 1, 1), (3, 1, 1)]);
        for orientation in [1u16, 3, 6, 8, 5] {
            let with = Setup { extra: vec![exif(orientation, true)], ..setup.clone() };
            let (_, bytes) = color(&with);
            let full = seen(&bytes, &wish((100, 100), [0; 4]));
            let (w, h) = full.decoded.picture;
            let part = [5, 7, 20.min(w - 5), 10.min(h - 7)];
            let got = seen(&bytes, &wish((100, 100), part));
            assert_eq!(got.decoded.part, part, "{orientation}");
            assert_eq!(got.decoded.size, (part[2], part[3]));
            let mut want = Vec::new();
            for y in part[1]..part[1] + part[3] {
                let from = ((y * w + part[0]) * 4) as usize;
                want.extend_from_slice(&full.pixels[from..from + (part[2] * 4) as usize]);
            }
            assert_eq!(got.pixels, want, "orientation {orientation}");
        }
        // Cut to the picture, and the whole where there is nothing.
        let (_, bytes) = color(&setup);
        assert_eq!(seen(&bytes, &wish((100, 100), [60, 40, 20, 20])).decoded.part, [60, 40, 4, 8]);
        assert_eq!(seen(&bytes, &wish((100, 100), [64, 0, 5, 5])).decoded.part, [0, 0, 64, 48]);
    }

    #[test]
    fn a_picture_made_smaller_is_near_the_mean_of_what_falls_into_a_field() {
        let setup = Setup::new(128, 96, &YCC);
        let (_, bytes) = color(&setup);
        let full = rgb_of(&seen(&bytes, &wish((200, 200), [0; 4])));
        // A half, a quarter and an eighth are each reached by a room of
        // about that size, and a size in between by a mean of the rest.
        for (room, step) in [((64, 48), 2usize), ((32, 24), 4), ((16, 12), 8), ((40, 30), 0)] {
            let got = seen(&bytes, &wish(room, [0; 4]));
            assert_eq!(got.decoded.size, room);
            if step == 0 {
                continue;
            }
            let small = rgb_of(&got);
            let want: Vec<[u8; 3]> = (0..room.1 as usize)
                .flat_map(|y| (0..room.0 as usize).map(move |x| (x, y)))
                .map(|(x, y)| {
                    let mut sum = [0u32; 3];
                    for dy in 0..step {
                        for dx in 0..step {
                            let at = (y * step + dy) * 128 + x * step + dx;
                            (0..3).for_each(|c| sum[c] += u32::from(full[at][c]));
                        }
                    }
                    sum.map(|s| ((s + (step * step / 2) as u32) / (step * step) as u32) as u8)
                })
                .collect();
            // An eighth is the mean itself. A half and a quarter keep
            // the low frequencies of a block as they are, which is a
            // little sharper than a mean, as other decoders have it.
            let most = if step == 8 { 2 } else { 8 };
            assert!(worst(&small, &want) <= most, "{room:?}: {}", worst(&small, &want));
        }
    }

    /// A picture of one color in every component, so that a block is a
    /// byte or two of data.
    fn flat(setup: &Setup, colors: [u8; 3]) -> Vec<u8> {
        let rgb = vec![colors; (setup.w * setup.h) as usize];
        encode(setup, &planes_of(setup, &rgb))
    }

    #[test]
    fn a_part_of_a_file_with_a_scan_for_each_component_has_all_of_them() {
        // The bits are read ahead, in a small file as far as the marker of
        // the scan that follows, and a scan that stopped below what was
        // wanted threw that marker away. The component after it was never
        // read. Two blocks to a scan: four bytes, which are read at once.
        let setup = Setup { interleaved: false, ..Setup::new(8, 16, &YCC) };
        let bytes = flat(&setup, [80, 40, 160]);
        let full = seen(&bytes, &whole());
        let part = seen(&bytes, &wish((100, 100), [0, 0, 8, 1]));
        assert_eq!(part.pixels, full.pixels[..32]);
        assert_eq!(part.decoded.notes, Notes(0));
        assert!(worst(&rgb_of(&part), &[[80, 40, 160]; 8]) <= 2);
    }

    #[test]
    fn a_scan_of_one_component_is_not_held_to_what_a_unit_of_all_may_hold() {
        // Ten blocks is the most a unit of a scan with several components
        // has, and it was held against the frame, also where every scan has
        // one.
        let sampled = [(1, 4, 4), (2, 1, 1), (3, 1, 1)];
        let apart = Setup { interleaved: false, ..Setup::new(40, 36, &sampled) };
        let (rgb, bytes) = color(&apart);
        let got = seen(&bytes, &whole());
        assert_eq!(got.decoded.size, (40, 36));
        assert!(worst(&rgb_of(&got), &rgb) <= 40, "{}", worst(&rgb_of(&got), &rgb));
        // In one scan that is eighteen blocks to a unit, which there is
        // none of.
        let (_, together) = color(&Setup::new(40, 36, &sampled));
        assert_eq!(decode(&together[..], &whole()).err(), Some(Refused::Damaged));
    }

    #[test]
    fn a_part_made_smaller_has_the_samples_beside_it_that_its_chroma_looks_at() {
        // One sample of the file was kept around a part, and made smaller
        // the neighbor that is looked at is one of the smaller picture,
        // eight of the file away.
        let frame = Frame {
            w: 256,
            h: 128,
            components: vec![
                Component { id: b'R', h: 2, v: 1, quant: 0 },
                Component { id: b'G', h: 1, v: 1, quant: 0 },
                Component { id: b'B', h: 1, v: 1, quant: 0 },
            ],
            h_max: 2,
            v_max: 1,
        };
        let mut planned = plan(&frame, 1, &wish((16, 16), [34, 0, 128, 128])).unwrap();
        assert_eq!(planned.reduce, 8);
        for (index, plane) in planned.planes.iter_mut().enumerate() {
            for y in plane.first.1..plane.first.1 + plane.blocks.1 {
                for x in plane.first.0..plane.first.0 + plane.blocks.0 {
                    let value = if index == 1 { (30 + 30 * x).min(240) as u8 } else { 128 };
                    plane.put((x, y), &[value]);
                }
            }
        }
        let scans =
            Scans { planned, colors: Colors::Rgb, covered: vec![true; 3], begun: vec![true; 3], blocks: 1 };
        let mut row = [[0; 3]; 1];
        colors_of_row(&frame, &scans, 4, 0, &mut [[0.0; 3]; 1], &mut row);
        // The second component at (4 + .5) / 2 - .5 = 1.75: between its
        // samples 1 and 2, which are 60 and 90.
        assert_eq!(row[0], [128, 83, 128]);
    }

    #[test]
    fn what_a_file_may_not_hold_is_said_and_not_shown_as_whole() {
        let setup = Setup { restart: 1, ..Setup::new(24, 16, &YCC) };
        let (_, bytes) = color(&setup);
        assert_eq!(seen(&bytes, &whole()).decoded.notes, Notes(0));
        let data = bytes.windows(2).position(|pair| pair == [0xFF, 0xDA]).unwrap() + 14;
        // Without the marker that ends it, a file is one that ends early.
        let open = &bytes[..bytes.len() - 2];
        assert!(seen(open, &whole()).decoded.notes.has(Notes::EARLY));
        // A part of it does not read that far, and says nothing of it.
        assert_eq!(seen(open, &wish((100, 100), [0, 0, 8, 2])).decoded.notes, Notes(0));
        // A marker in the middle of a block is damage, not an end.
        let mut marked = bytes.clone();
        let middle = data + (bytes.len() - data) / 2;
        marked[middle..middle + 2].copy_from_slice(&[0xFF, 0xD9]);
        let got = seen(&marked, &whole()).decoded.notes;
        assert!(got.has(Notes::DAMAGED) && !got.has(Notes::EARLY), "{got:?}");
        // The restart markers count up: another one is damage.
        let mut counted = bytes.clone();
        let first = counted.windows(2).position(|pair| pair == [0xFF, 0xD0]).unwrap();
        counted[first + 1] = 0xD7;
        assert!(seen(&counted, &whole()).decoded.notes.has(Notes::DAMAGED));
        // Only one symbol ends a block.
        let (_, wrong) = color(&Setup { eob: 0x10, ..Setup::new(24, 16, &YCC) });
        assert_eq!(decode(&wrong[..], &whole()).err(), Some(Refused::Damaged));
        // A table whose last code is all ones, which no code may be.
        let mut counts = [0u8; 16];
        counts[0] = 2;
        assert!(Huffman::new(&counts, &[1, 0]).is_none());
        counts[0] = 1;
        assert!(Huffman::new(&counts, &[1]).is_some());
        // A hierarchical file begins with a frame like any other, after
        // a segment that says what it is.
        let hierarchical =
            Setup { extra: vec![(0xDE, vec![8, 0, 16, 0, 24, 1, 1, 0x11, 0])], ..setup.clone() };
        assert_eq!(decode(&color(&hierarchical).1[..], &whole()).err(), Some(Refused::Coding));
        // Five components are a number of colors that is not shown.
        let mut frame = vec![8, 0, 8, 0, 8, 5];
        (1..=5).for_each(|id| frame.extend([id, 0x11, 0]));
        assert!(matches!(read_frame(&frame), Err(Refused::Colors)));
    }

    #[test]
    fn a_component_that_comes_in_a_second_scan_ends_the_reading() {
        // A file could name one component in scan after scan, each read in
        // full, until the time of the process was up. A component has one
        // scan.
        let setup = Setup { interleaved: false, ..Setup::new(24, 16, &YCC) };
        let (_, bytes) = color(&setup);
        let starts: Vec<usize> = bytes
            .windows(2)
            .enumerate()
            .filter(|(_, pair)| *pair == [0xFF, 0xDA])
            .map(|(at, _)| at)
            .collect();
        assert_eq!(starts.len(), 3);
        // The scan of the first, again, before the two that follow.
        let again = [&bytes[..starts[1]], &bytes[starts[0]..starts[1]], &bytes[starts[1]..]].concat();
        let got = seen(&again, &whole());
        assert!(got.decoded.notes.has(Notes::DAMAGED), "{:?}", got.decoded.notes);
        // What was read before it is there.
        let sound = seen(&bytes, &whole());
        let luma = |picture: &Picture| -> Vec<u8> { rgb_of(picture).iter().map(|[_, g, _]| *g).collect() };
        assert_eq!(luma(&got).len(), luma(&sound).len());
    }

    #[test]
    fn a_turned_part_costs_no_more_memory_than_one_that_is_not() {
        // The result was made as it is stored and then again upright, twice
        // its size, and a large part of a turned picture was made smaller
        // for it.
        let frame = Frame {
            w: 8192,
            h: 8192,
            components: vec![
                Component { id: 1, h: 2, v: 2, quant: 0 },
                Component { id: 2, h: 1, v: 1, quant: 0 },
                Component { id: 3, h: 1, v: 1, quant: 0 },
            ],
            h_max: 2,
            v_max: 2,
        };
        for orientation in [1, 6] {
            let planned = plan(&frame, orientation, &wish((4096, 4096), [0, 0, 4096, 4096])).unwrap();
            assert_eq!((planned.size, planned.smaller), ((4096, 4096), false), "{orientation}");
        }
    }

    #[test]
    fn a_file_that_ends_early_shows_what_is_there() {
        let setup = Setup::new(40, 40, &[(1, 2, 2), (2, 1, 1), (3, 1, 1)]);
        let (rgb, bytes) = color(&setup);
        let whole_picture = seen(&bytes, &whole());
        let mut early = 0;
        // Ended at every place: an answer, or a reason, and no more.
        for length in (0..bytes.len()).step_by(3) {
            match decode(&bytes[..length], &whole()) {
                Ok(picture) => {
                    assert_eq!(picture.pixels.len(), 40 * 40 * 4);
                    if picture.decoded.notes.partial() {
                        early += 1;
                    }
                }
                Err(reason) => {
                    assert!(matches!(reason, Refused::Damaged | Refused::Format), "{length}: {reason:?}");
                }
            }
        }
        assert!(early > 10, "{early}");
        // Cut in the middle of the data: the top of the picture is
        // there, the rest is gray, and it is said.
        let cut = seen(&bytes[..bytes.len() * 2 / 3], &whole());
        assert!(cut.decoded.notes.has(Notes::EARLY));
        assert_eq!(&cut.pixels[..40 * 4 * 8], &whole_picture.pixels[..40 * 4 * 8]);
        assert!(worst(&rgb_of(&whole_picture), &rgb) <= 14);
        // Whole, it is not said to end early.
        assert!(!whole_picture.decoded.notes.partial());
    }

    #[test]
    fn what_is_not_shown_is_turned_away_by_name() {
        let setup = Setup::new(16, 16, &YCC);
        let (_, bytes) = color(&setup);
        let changed = |from: &[u8], to: &[u8]| {
            let mut bytes = bytes.clone();
            let at = bytes.windows(from.len()).position(|w| w == from).unwrap();
            bytes[at..at + from.len()].copy_from_slice(to);
            bytes
        };
        let why = |bytes: &[u8]| decode(bytes, &whole()).err();
        // The marker of the frame, found after the tables.
        let (_, progressive) = color(&Setup { marker: 0xC2, ..setup.clone() });
        assert_eq!(why(&progressive), Some(Refused::Progressive));
        for marker in [0xC3, 0xC5, 0xC9, 0xCA, 0xCF] {
            let (_, bytes) = color(&Setup { marker, ..setup.clone() });
            assert_eq!(why(&bytes), Some(Refused::Coding), "{marker:x}");
        }
        let (_, twelve) = color(&Setup { precision: 12, ..setup.clone() });
        assert_eq!(why(&twelve), Some(Refused::Coding));
        // Four components, and two.
        let four = Setup::new(16, 16, &[(1, 1, 1), (2, 1, 1), (3, 1, 1), (4, 1, 1)]);
        let planes = vec![vec![100; 256]; 4];
        assert_eq!(why(&encode(&four, &planes)), Some(Refused::Colors));
        let two = Setup::new(16, 16, &[(1, 1, 1), (2, 1, 1)]);
        assert_eq!(why(&encode(&two, &planes[..2])), Some(Refused::Colors));
        // Not a JPEG, no frame, no tables for the scan.
        assert_eq!(why(b"GIF89a and so on"), Some(Refused::Format));
        assert_eq!(why(&[0xFF, 0xD8, 0xFF, 0xD9]), Some(Refused::Damaged));
        let no_tables = changed(&[0xFF, 0xC4], &[0xFF, 0xFE]);
        assert_eq!(why(&no_tables), Some(Refused::Damaged));
        // A picture too large for what is decoded, said before anything
        // is asked for.
        let mut large = bytes.clone();
        let at = large.windows(2).position(|w| w == [0xFF, 0xC0]).unwrap();
        large[at + 5..at + 9].copy_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(why(&large), Some(Refused::Large));
        let mut empty = bytes.clone();
        empty[at + 5..at + 9].copy_from_slice(&[0, 0, 0, 16]);
        assert_eq!(why(&empty), Some(Refused::Damaged));
    }

    #[test]
    fn what_a_file_says_of_its_profile_and_of_its_colors_is_noted() {
        let setup = Setup::new(16, 16, &YCC);
        let (_, plain) = color(&setup);
        assert_eq!(seen(&plain, &whole()).decoded.notes, Notes(0));
        let icc =
            Setup { extra: vec![(0xE2, [&b"ICC_PROFILE\0"[..], &[1, 1, 0, 0]].concat())], ..setup.clone() };
        let (_, bytes) = color(&icc);
        assert_eq!(seen(&bytes, &whole()).decoded.notes, Notes(Notes::UNAPPLIED));
        // Colors that are stored as red, green and blue.
        let adobe = [&b"Adobe"[..], &[0, 100, 0, 0, 0, 0, 0]].concat();
        let rgb_setup = Setup { extra: vec![(0xEE, adobe)], ..setup.clone() };
        let rgb = picture(16, 16);
        let planes: Vec<Vec<u8>> = (0..3).map(|c| rgb.iter().map(|pixel| pixel[c]).collect()).collect();
        let got = seen(&encode(&rgb_setup, &planes), &whole());
        assert!(worst(&rgb_of(&got), &rgb) <= 1);
    }

    #[test]
    fn the_inverse_transform_matches_the_formula_at_every_size() {
        // A block of frequencies that is not flat.
        let mut coefficients = [0.0f32; 64];
        for (k, slot) in coefficients.iter_mut().enumerate() {
            *slot = (((k * 37 + 11) % 23) as f32 - 11.0) * if k < 20 { 9.0 } else { 0.0 };
        }
        for n in [8usize, 4, 2, 1] {
            let idct = Idct::new(n);
            let mut out = vec![0u8; n * n];
            idct.run(&coefficients, &mut out);
            // The formula for a transform of n points on the first n
            // frequencies, as a mean of 8 by 8 is made smaller by 8/n.
            for y in 0..n {
                for x in 0..n {
                    let mut sum = 0.0f64;
                    for v in 0..n {
                        for u in 0..n {
                            let alpha = |f: usize| {
                                if f == 0 { (1.0 / n as f64).sqrt() } else { (2.0 / n as f64).sqrt() }
                            };
                            sum += alpha(u)
                                * alpha(v)
                                * f64::from(coefficients[v * 8 + u])
                                * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / (2.0 * n as f64))
                                    .cos()
                                * ((2 * y + 1) as f64 * v as f64 * std::f64::consts::PI / (2.0 * n as f64))
                                    .cos();
                        }
                    }
                    let want = (sum * n as f64 / 8.0 + 128.0).round().clamp(0.0, 255.0) as u8;
                    assert!(out[y * n + x].abs_diff(want) <= 1, "n {n} at {x},{y}");
                }
            }
        }
        // Only a first value: flat.
        let mut flat = [0.0f32; 64];
        flat[0] = 80.0;
        let mut out = vec![0u8; 64];
        Idct::new(8).run(&flat, &mut out);
        assert!(out.iter().all(|sample| *sample == 138));
    }

    #[test]
    fn a_table_of_codes_is_held_against_the_room_there_is_and_both_ways_of_looking_agree() {
        // More codes of a length than that length has.
        let mut counts = [0u8; 16];
        counts[0] = 3;
        assert!(Huffman::new(&counts, &[0, 1, 2]).is_none());
        assert!(Huffman::new(&[0; 16], &[]).is_none(), "no symbols");
        assert!(Huffman::new(&counts, &[0]).is_none(), "fewer symbols than counts");
        let spec = Spec::skewed(5, 239, 16);
        let table = Huffman::new(&spec.counts, &spec.symbols).unwrap();
        for (symbol, code, length) in spec.codes() {
            // The code, then whatever follows it.
            for tail in [0u32, 0xFFFF] {
                let look = (code << (16 - length)) | (tail & ((1 << (16 - length)) - 1));
                assert_eq!(table.symbol(look), Some((symbol, length)), "{symbol}");
            }
        }
        // The all-ones code is none.
        assert_eq!(table.symbol(0xFFFF), None);
    }

    /// Numbers that look like chance and are the same every time.
    fn chance(state: &mut u64) -> usize {
        *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (*state >> 33) as usize
    }

    #[test]
    fn a_file_changed_at_random_places_ends_in_an_answer() {
        let setup = Setup { restart: 3, ..Setup::new(33, 27, &[(1, 2, 2), (2, 1, 1), (3, 1, 1)]) };
        let (_, bytes) = color(&setup);
        let mut state = 7u64;
        let wishes = [whole(), wish((9, 7), [0; 4]), wish((100, 100), [5, 5, 20, 12])];
        for round in 0..400 {
            let mut broken = bytes.clone();
            for _ in 0..=(round % 4) {
                let at = chance(&mut state) % broken.len();
                broken[at] = chance(&mut state) as u8;
            }
            if round % 5 == 0 {
                broken.truncate(chance(&mut state) % broken.len());
            }
            for wish in &wishes {
                if let Ok(picture) = decode(&broken[..], wish) {
                    let (w, h) = picture.decoded.size;
                    assert_eq!(picture.pixels.len(), (w * h * 4) as usize);
                }
            }
        }
    }
}
