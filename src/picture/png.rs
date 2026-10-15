//! PNG, read as a stream and made smaller while it is read: the file is
//! never whole in memory, nor is what it unpacks to. Two rows of it are,
//! and the result, and with Adam7 a sum for every field of the result.
//!
//! What is read here comes from anywhere. Nothing in this file indexes or
//! reckons in a way that can fail: the lints below hold it to that, and
//! `cargo clippy` is what checks them. Deflate is `yazi`'s, called so that
//! it hands over what it unpacks piece by piece, and what `yazi` does not
//! hold is held here: how much comes out, the sum over it, and that a
//! writer that refused once refuses from then on.

#![forbid(unsafe_code)]
#![deny(clippy::indexing_slicing, clippy::arithmetic_side_effects)]

use std::io::{ErrorKind, Read, Write};

use super::wire::{PIXELS_MAX, ROOM_MIN, SIDE_MAX, WHOLE, Wish};
use crate::core::picture::{Decoded, Notes, Refused};

const SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";
/// How many chunks a file may have.
const CHUNKS: u32 = 65_536;
/// Where white, red, green and blue of a screen lie, as `cHRM` names
/// them: each across and up, in hundred-thousandths.
const SCREEN: [u32; 8] = [31_270, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000];
/// In pieces of how many bytes the file is read.
const PIECE: usize = 64 << 10;
/// What the result and the sums may take together. A picture that needs
/// more for the size that was asked is made smaller still.
const WORK: u64 = 128 << 20;
/// How many pixels of the file one field of a result gathers at the
/// most: each is 255 at the most, and their sum is kept in 32 bits.
const FIELD_MAX: u64 = 1 << 24;
/// What a field costs while sums are kept for it: four sums and a count.
const FIELD_BYTES: u64 = 20;

/// A picture as it is handed on: its pixels row by row from the top,
/// four bytes each, the colors multiplied by their alpha.
pub struct Picture {
    pub decoded: Decoded,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Gray,
    Rgb,
    Palette,
    GrayAlpha,
    Rgba,
}

/// What the head of a file says of its picture.
#[derive(Clone, Copy, Debug)]
struct Shape {
    w: u32,
    h: u32,
    kind: Kind,
    depth: u8,
    adam7: bool,
}

impl Shape {
    fn read(body: &[u8; 13]) -> Result<Self, Refused> {
        let [w0, w1, w2, w3, h0, h1, h2, h3, depth, kind, packing, filter, lace] = *body;
        let (w, h) = (u32::from_be_bytes([w0, w1, w2, w3]), u32::from_be_bytes([h0, h1, h2, h3]));
        if w == 0 || h == 0 || packing != 0 || filter != 0 || lace > 1 {
            return Err(Refused::Damaged);
        }
        if w > SIDE_MAX || h > SIDE_MAX || u64::from(w).saturating_mul(u64::from(h)) > PIXELS_MAX {
            return Err(Refused::Large);
        }
        let kind = match (kind, depth) {
            (0, 1 | 2 | 4 | 8 | 16) => Kind::Gray,
            (2, 8 | 16) => Kind::Rgb,
            (3, 1 | 2 | 4 | 8) => Kind::Palette,
            (4, 8 | 16) => Kind::GrayAlpha,
            (6, 8 | 16) => Kind::Rgba,
            _ => return Err(Refused::Kind),
        };
        Ok(Self { w, h, kind, depth, adam7: lace == 1 })
    }

    /// The bits of one pixel.
    fn bits(&self) -> u64 {
        let channels: u64 = match self.kind {
            Kind::Gray | Kind::Palette => 1,
            Kind::GrayAlpha => 2,
            Kind::Rgb => 3,
            Kind::Rgba => 4,
        };
        channels.saturating_mul(u64::from(self.depth))
    }

    /// How many bytes back a filter looks: the bytes of a pixel, and one
    /// where a pixel is less than a byte.
    fn step(&self) -> usize {
        (self.bits().div_ceil(8) as usize).clamp(1, 8)
    }

    /// The bytes of a row of so many pixels, without the one for its
    /// filter.
    fn pitch(&self, width: u32) -> usize {
        // 65,535 pixels of 64 bits at the most.
        u64::from(width).saturating_mul(self.bits()).div_ceil(8) as usize
    }
}

/// One pass over the picture: where it begins, how far its pixels lie
/// apart, the block a pixel of it stands for until a later pass comes,
/// and how many pixels and rows it has.
#[derive(Clone, Copy, Debug)]
struct Pass {
    x0: u32,
    y0: u32,
    dx: u32,
    dy: u32,
    block: (u32, u32),
    w: u32,
    h: u32,
}

/// Where a pass begins and how far its pixels lie apart, each way, and
/// the block one of them stands for.
type Lace = (u32, u32, u32, u32, (u32, u32));

/// The seven passes of Adam7.
const ADAM7: [Lace; 7] = [
    (0, 0, 8, 8, (8, 8)),
    (4, 0, 8, 8, (4, 8)),
    (0, 4, 4, 8, (4, 4)),
    (2, 0, 4, 4, (2, 4)),
    (0, 2, 2, 4, (2, 2)),
    (1, 0, 2, 2, (1, 2)),
    (0, 1, 1, 2, (1, 1)),
];

impl Shape {
    /// The passes that have pixels, in the order their rows come in. A
    /// pass without any has no rows in the data, and no byte for a filter.
    fn passes(&self) -> Vec<Pass> {
        let pass = |(x0, y0, dx, dy, block): Lace| {
            // The distances are never zero.
            let w = self.w.saturating_sub(x0).div_ceil(dx.max(1));
            let h = self.h.saturating_sub(y0).div_ceil(dy.max(1));
            (w > 0 && h > 0).then_some(Pass { x0, y0, dx, dy, block, w, h })
        };
        if self.adam7 {
            ADAM7.into_iter().filter_map(pass).collect()
        } else {
            pass((0, 0, 1, 1, (1, 1))).into_iter().collect()
        }
    }
}

/// The size a picture is decoded to for a wish: its own where it is
/// small enough to be kept whole or fits the room as it is, and else the
/// largest that fits the room and keeps its shape. No side is longer
/// than a texture takes.
pub fn fit(picture: (u32, u32), wish: &Wish) -> (u32, u32) {
    let (w, h) = (u64::from(picture.0.max(1)), u64::from(picture.1.max(1)));
    let side = u64::from(wish.side.clamp(1, SIDE_MAX));
    let whole = u64::from(wish.whole.min(WHOLE));
    let room = |room: u32| u64::from(room.max(ROOM_MIN)).min(side);
    let room =
        if w.saturating_mul(h) <= whole { (side, side) } else { (room(wish.room.0), room(wish.room.1)) };
    if w <= room.0 && h <= room.1 {
        return (w as u32, h as u32);
    }
    // The side that is the tighter of the two decides, and the other
    // follows from it.
    let scaled = |of: u64, by: u64, over: u64| of.saturating_mul(by).checked_div(over).unwrap_or(1).max(1);
    let (rw, rh) = if w.saturating_mul(room.1) >= h.saturating_mul(room.0) {
        (room.0, scaled(h, room.0, w).min(room.1))
    } else {
        (scaled(w, room.1, h).min(room.0), room.1)
    };
    (rw as u32, rh as u32)
}

/// Whether the result and its sums fit into what the process may use.
fn fits(shape: &Shape, part: [u32; 4], size: (u32, u32)) -> bool {
    let fields = u64::from(size.0).saturating_mul(u64::from(size.1));
    // A part at its own size is not the picture whole: it is kept as
    // sums for one row of it, as a reduced one is.
    let whole = part == [0, 0, shape.w, shape.h] && size == (shape.w, shape.h);
    // A part at its own size has one pixel for a field and no sums.
    let direct = size == (part[2], part[3]);
    // Without Adam7 the rows come from the top, and sums are kept for
    // one row of the result.
    let kept = if whole || direct {
        0
    } else if shape.adam7 {
        fields
    } else {
        u64::from(size.0)
    };
    fields.saturating_mul(4).saturating_add(kept.saturating_mul(FIELD_BYTES)) <= WORK
}

/// The rectangle of the picture a wish is for: the one it names, cut to
/// lie within the picture, or the whole of it where it names none or
/// none that has anything inside the picture.
fn crop(shape: &Shape, wish: &Wish) -> [u32; 4] {
    wish.part_of((shape.w, shape.h))
}

/// The size that is decoded to, and whether it is smaller than was asked
/// for because of a limit.
fn plan(shape: &Shape, part: [u32; 4], wish: &Wish) -> Result<((u32, u32), bool), Refused> {
    let picture = (part[2], part[3]);
    let asked = fit(picture, wish);
    let mut size = asked;
    while !fits(shape, part, size) && size != (1, 1) {
        size = (size.0.div_ceil(2), size.1.div_ceil(2));
    }
    // What falls into one field at the most, each way.
    let field = u64::from(picture.0.div_ceil(size.0.max(1)))
        .saturating_mul(u64::from(picture.1.div_ceil(size.1.max(1))));
    if !fits(shape, part, size) || field > FIELD_MAX {
        return Err(Refused::Large);
    }
    Ok((size, size != asked))
}

/// What `fields` has for a pixel that lies outside the part: no field.
const OUTSIDE: u32 = u32::MAX;

/// For every pixel along one side of the picture, the field of the
/// result it falls into: of the `span` pixels that begin at `from`, and
/// `OUTSIDE` for the others.
fn fields(length: u32, into: u32, from: u32, span: u32) -> Result<Vec<u32>, Refused> {
    let mut map = Vec::new();
    map.try_reserve_exact(length as usize).map_err(|_| Refused::Memory)?;
    let (span, into) = (u64::from(span.max(1)), u64::from(into));
    let (from, length) = (u64::from(from), u64::from(length.max(1)));
    map.extend((0..length).map(|at| match at.checked_sub(from).filter(|inside| *inside < span) {
        // Below `into`, for `inside` is below `span`.
        Some(inside) => {
            inside.saturating_mul(into).checked_div(span).unwrap_or(0).min(into.saturating_sub(1)) as u32
        }
        None => OUTSIDE,
    }));
    Ok(map)
}

fn zeroed<T: Clone>(count: u64, of: T) -> Result<Vec<T>, Refused> {
    let count = usize::try_from(count).map_err(|_| Refused::Memory)?;
    let mut all = Vec::new();
    all.try_reserve_exact(count).map_err(|_| Refused::Memory)?;
    all.resize(count, of);
    Ok(all)
}

/// Where the rows of the picture go.
enum Out {
    /// The picture at its own size, and how many pixels a row of it has.
    Whole { pixels: Vec<[u8; 4]>, width: usize },
    /// The picture made smaller: every pixel is added to the field it
    /// falls into, and a field is the mean of what reached it. Rows that
    /// come from the top keep sums for one row of the result, the rows
    /// of Adam7 for all of them.
    Reduced {
        pixels: Vec<[u8; 4]>,
        width: usize,
        columns: Vec<u32>,
        rows: Vec<u32>,
        sums: Vec<[u32; 4]>,
        counts: Vec<u32>,
        all: bool,
        /// The row of the result the sums are for, where they are for
        /// one.
        at: Option<u32>,
        /// A part at its own size: every field gets one pixel, which is
        /// written to it, and no sums are kept.
        direct: bool,
    },
}

impl Out {
    fn new(shape: &Shape, part: [u32; 4], size: (u32, u32)) -> Result<Self, Refused> {
        let fields_in_all = u64::from(size.0).saturating_mul(u64::from(size.1));
        let (pixels, width) = (zeroed(fields_in_all, [0u8; 4])?, (size.0 as usize).max(1));
        if part == [0, 0, shape.w, shape.h] && size == (shape.w, shape.h) {
            return Ok(Self::Whole { pixels, width });
        }
        let direct = size == (part[2], part[3]);
        let kept = match (direct, shape.adam7) {
            (true, _) => 0,
            (false, true) => fields_in_all,
            (false, false) => u64::from(size.0),
        };
        Ok(Self::Reduced {
            pixels,
            width,
            columns: fields(shape.w, size.0, part[0], part[2])?,
            rows: fields(shape.h, size.1, part[1], part[3])?,
            sums: zeroed(kept, [0u32; 4])?,
            counts: zeroed(kept, 0u32)?,
            all: shape.adam7,
            at: None,
            direct,
        })
    }

    /// Takes one row of a pass, as pixels.
    fn put(&mut self, pass: &Pass, row: u32, wide: &[[u8; 4]]) {
        let y = pass.y0.saturating_add(row.saturating_mul(pass.dy));
        // Where in the picture the pixels of the row stand.
        let xs = (pass.x0 as usize..).step_by(pass.dx.max(1) as usize);
        match self {
            Self::Whole { pixels, width } if pass.dx == 1 && pass.dy == 1 => {
                if let Some(into) = pixels.chunks_exact_mut(*width).nth(y as usize) {
                    into.iter_mut().zip(wide).for_each(|(to, from)| *to = *from);
                }
            }
            // A pixel of a pass fills its block, and a later pass writes
            // over what of it is not its own: a file that ends early
            // then shows the whole picture coarsely, and not a grid.
            Self::Whole { pixels, width } => {
                for line in pixels.chunks_exact_mut(*width).skip(y as usize).take(pass.block.1 as usize) {
                    for (pixel, x) in wide.iter().zip(xs.clone()) {
                        line.iter_mut().skip(x).take(pass.block.0 as usize).for_each(|into| *into = *pixel);
                    }
                }
            }
            Self::Reduced { width, columns, rows, sums, counts, all, at, pixels, direct } => {
                let Some(field_row) = rows.get(y as usize).copied().filter(|row| *row != OUTSIDE) else {
                    return;
                };
                // Rows from the top: the row of the result before this
                // one has all it gets.
                if !*all
                    && *at != Some(field_row)
                    && let Some(done) = at.replace(field_row)
                {
                    settle(sums, counts, pixels.chunks_exact_mut(*width).nth(done as usize));
                    sums.fill([0; 4]);
                    counts.fill(0);
                }
                // With Adam7 a pixel of a pass stands for its block here
                // too: in the fields its block reaches, until pixels of
                // their own come. A field that got some is their mean in
                // the end, whatever was written to it here.
                let below = pass.block.1.saturating_sub(1);
                let last = rows.get(y.saturating_add(below) as usize).or(rows.last()).copied().unwrap_or(0);
                let tall = last > field_row;
                if *all && pass.block != (1, 1) {
                    let reach = (pass.block.0 as usize).saturating_sub(1);
                    let lines = (last.saturating_sub(field_row) as usize).saturating_add(1);
                    for (pixel, x) in wide.iter().zip(xs.clone()) {
                        let Some(from) = columns.get(x).map(|field| *field as usize) else { continue };
                        let to = columns
                            .get(x.saturating_add(reach))
                            .or(columns.last())
                            .map_or(from, |to| *to as usize);
                        // Its own field alone gets pixels of its own.
                        if !tall && to == from {
                            continue;
                        }
                        let wide = to.saturating_sub(from).saturating_add(1);
                        for line in pixels.chunks_exact_mut(*width).skip(field_row as usize).take(lines) {
                            line.iter_mut().skip(from).take(wide).for_each(|into| *into = *pixel);
                        }
                    }
                }
                if *direct {
                    // Its own pixel is written, and a later pass writes
                    // over what was painted for a block.
                    let line = pixels.chunks_exact_mut(*width).nth(field_row as usize);
                    if let Some(line) = line {
                        for (pixel, x) in wide.iter().zip(xs) {
                            let field = columns.get(x).copied().filter(|field| *field != OUTSIDE);
                            if let Some(into) = field.and_then(|field| line.get_mut(field as usize)) {
                                *into = *pixel;
                            }
                        }
                    }
                    return;
                }
                let slot = if *all { field_row as usize } else { 0 };
                let (Some(sums), Some(counts)) =
                    (sums.chunks_exact_mut(*width).nth(slot), counts.chunks_exact_mut(*width).nth(slot))
                else {
                    return;
                };
                for (pixel, x) in wide.iter().zip(xs) {
                    let Some(field) = columns.get(x).map(|field| *field as usize) else { continue };
                    if let (Some(sum), Some(count)) = (sums.get_mut(field), counts.get_mut(field)) {
                        // Neither runs over: `plan` held what falls into
                        // one field against what 32 bits take.
                        sum.iter_mut()
                            .zip(pixel)
                            .for_each(|(sum, part)| *sum = sum.wrapping_add(u32::from(*part)));
                        *count = count.wrapping_add(1);
                    }
                }
            }
        }
    }

    /// The pixels, when no more rows come.
    fn end(self) -> Vec<[u8; 4]> {
        match self {
            Self::Whole { pixels, .. } => pixels,
            Self::Reduced { mut pixels, width, sums, counts, all, at, .. } => {
                let mut lines = pixels.chunks_exact_mut(width);
                if all {
                    let kept = sums.chunks_exact(width).zip(counts.chunks_exact(width));
                    kept.zip(lines).for_each(|((sums, counts), line)| settle(sums, counts, Some(line)));
                } else if let Some(last) = at {
                    settle(&sums, &counts, lines.nth(last as usize));
                }
                pixels
            }
        }
    }
}

/// Writes a row of the result: every field the mean of what reached it,
/// and nothing where nothing did.
fn settle(sums: &[[u32; 4]], counts: &[u32], line: Option<&mut [[u8; 4]]>) {
    let Some(line) = line else { return };
    for ((sum, count), pixel) in sums.iter().zip(counts).zip(line) {
        if *count > 0 {
            *pixel = sum.map(|sum| sum.wrapping_add(*count >> 1).checked_div(*count).unwrap_or(0) as u8);
        }
    }
}

/// The colors a file names apart from its pixels.
#[derive(Default)]
struct Colors {
    /// The palette, each entry with its alpha and multiplied by it.
    palette: Vec<[u8; 4]>,
    /// The one color that is transparent, as the samples of the file
    /// have it. Gray has it three times.
    key: Option<[u16; 3]>,
}

/// A color times its alpha, rounded.
fn times(color: u8, alpha: u8) -> u8 {
    (u32::from(color).wrapping_mul(u32::from(alpha)).wrapping_add(127) / 255) as u8
}

fn pixel(r: u8, g: u8, b: u8, a: u8) -> [u8; 4] {
    if a == 255 { [r, g, b, a] } else { [times(r, a), times(g, a), times(b, a), a] }
}

/// Sixteen bits as eight, rounded.
fn narrow(sample: u16) -> u8 {
    (u32::from(sample).wrapping_mul(255).wrapping_add(32_767) / 65_535) as u8
}

/// A sample of sixteen bits, its higher byte first.
fn wide_sample(high: u8, low: u8) -> u16 {
    u16::from_be_bytes([high, low])
}

/// Makes of the bytes of a row, its filter undone, pixels. Says whether
/// the row named a color the palette does not have.
fn expand(shape: &Shape, colors: &Colors, data: &[u8], wide: &mut [[u8; 4]]) -> bool {
    let key = colors.key;
    let into = wide.iter_mut();
    let mut strange = false;
    match (shape.kind, shape.depth) {
        (Kind::Rgba, 8) => {
            into.zip(data.as_chunks::<4>().0).for_each(|(to, &[r, g, b, a])| *to = pixel(r, g, b, a));
        }
        (Kind::Rgb, 8) => into.zip(data.as_chunks::<3>().0).for_each(|(to, &[r, g, b])| {
            let seen = key != Some([u16::from(r), u16::from(g), u16::from(b)]);
            *to = if seen { [r, g, b, 255] } else { [0; 4] };
        }),
        (Kind::GrayAlpha, 8) => {
            into.zip(data.as_chunks::<2>().0).for_each(|(to, &[v, a])| *to = pixel(v, v, v, a));
        }
        (Kind::Rgba, 16) => {
            into.zip(data.as_chunks::<8>().0).for_each(|(to, &[r, r0, g, g0, b, b0, a, a0])| {
                let narrow = |high, low| narrow(wide_sample(high, low));
                *to = pixel(narrow(r, r0), narrow(g, g0), narrow(b, b0), narrow(a, a0));
            })
        }
        (Kind::Rgb, 16) => into.zip(data.as_chunks::<6>().0).for_each(|(to, &[r, r0, g, g0, b, b0])| {
            let (r, g, b) = (wide_sample(r, r0), wide_sample(g, g0), wide_sample(b, b0));
            let seen = key != Some([r, g, b]);
            *to = if seen { [narrow(r), narrow(g), narrow(b), 255] } else { [0; 4] };
        }),
        (Kind::GrayAlpha, 16) => into.zip(data.as_chunks::<4>().0).for_each(|(to, &[v, v0, a, a0])| {
            let (v, a) = (narrow(wide_sample(v, v0)), narrow(wide_sample(a, a0)));
            *to = pixel(v, v, v, a);
        }),
        (Kind::Gray, 16) => into.zip(data.as_chunks::<2>().0).for_each(|(to, &[v, v0])| {
            let v = wide_sample(v, v0);
            let seen = key.is_none_or(|key| key.first() != Some(&v));
            let v = narrow(v);
            *to = if seen { [v, v, v, 255] } else { [0; 4] };
        }),
        // Samples of a byte or less, one to a pixel: gray, or a place in
        // the palette. They stand in a byte from its highest bits down.
        (Kind::Gray | Kind::Palette, depth) => {
            let (shifts, mask, spread): (&[u32], u8, u8) = match depth {
                1 => (&[7, 6, 5, 4, 3, 2, 1, 0], 1, 255),
                2 => (&[6, 4, 2, 0], 3, 85),
                4 => (&[4, 0], 15, 17),
                _ => (&[0], 255, 1),
            };
            let samples =
                data.iter().flat_map(|byte| shifts.iter().map(move |by| byte.wrapping_shr(*by) & mask));
            for (to, sample) in into.zip(samples) {
                *to = if shape.kind == Kind::Palette {
                    colors.palette.get(usize::from(sample)).copied().unwrap_or_else(|| {
                        strange = true;
                        [0, 0, 0, 255]
                    })
                } else if key.is_some_and(|key| key.first() == Some(&u16::from(sample))) {
                    [0; 4]
                } else {
                    let v = sample.wrapping_mul(spread);
                    [v, v, v, 255]
                };
            }
        }
        // `Shape::read` lets no other pair through.
        _ => {}
    }
    strange
}

/// Undoes the filter of a row in place. `prior` is the row before it in
/// its pass, or zeros. Says whether the filter is one there is.
fn unfilter(filter: u8, step: usize, row: &mut [u8], prior: &[u8]) -> bool {
    let step = step.clamp(1, 8);
    let mut left = [0u8; 8];
    match filter {
        0 => {}
        1 => {
            for pixel in row.chunks_mut(step) {
                for (byte, left) in pixel.iter_mut().zip(&mut left) {
                    *byte = byte.wrapping_add(*left);
                    *left = *byte;
                }
            }
        }
        2 => row.iter_mut().zip(prior).for_each(|(byte, up)| *byte = byte.wrapping_add(*up)),
        3 => {
            for (pixel, ups) in row.chunks_mut(step).zip(prior.chunks(step)) {
                for ((byte, left), up) in pixel.iter_mut().zip(&mut left).zip(ups) {
                    let mean = (u16::from(*left).wrapping_add(u16::from(*up)) >> 1) as u8;
                    *byte = byte.wrapping_add(mean);
                    *left = *byte;
                }
            }
        }
        4 => {
            let mut corner = [0u8; 8];
            for (pixel, ups) in row.chunks_mut(step).zip(prior.chunks(step)) {
                for (((byte, left), corner), up) in pixel.iter_mut().zip(&mut left).zip(&mut corner).zip(ups)
                {
                    *byte = byte.wrapping_add(paeth(*left, *up, *corner));
                    *left = *byte;
                    *corner = *up;
                }
            }
        }
        _ => return false,
    }
    true
}

/// Of the pixel to the left, the one above and the one above to the
/// left, the one nearest to left plus above less the corner.
fn paeth(left: u8, up: u8, corner: u8) -> u8 {
    let (a, b, c) = (i16::from(left), i16::from(up), i16::from(corner));
    let to_left = b.wrapping_sub(c).unsigned_abs();
    let to_up = a.wrapping_sub(c).unsigned_abs();
    let to_corner = a.wrapping_add(b).wrapping_sub(c).wrapping_sub(c).unsigned_abs();
    if to_left <= to_up && to_left <= to_corner {
        left
    } else if to_up <= to_corner {
        up
    } else {
        corner
    }
}

/// What `yazi` writes into: it gathers a row with the byte for its
/// filter, undoes the filter, makes pixels of it and hands them on.
struct Rows<'a> {
    shape: Shape,
    colors: &'a Colors,
    passes: Vec<Pass>,
    /// The pass the row that is gathered belongs to, and which of its
    /// rows it is.
    pass: usize,
    row: u32,
    /// The row that is gathered, the byte for its filter first. Empty
    /// when every row has come.
    line: Vec<u8>,
    filled: usize,
    /// The row before it in its pass, its filter undone.
    prior: Vec<u8>,
    wide: Vec<[u8; 4]>,
    out: Out,
    sum: yazi::Adler32,
    /// How many rows came whole.
    came: u64,
    /// A filter that is none, or more than the head of the file reckons:
    /// nothing is taken after it.
    faulty: bool,
    /// A color was named that the palette does not have.
    strange: bool,
}

impl<'a> Rows<'a> {
    fn new(shape: Shape, colors: &'a Colors, part: [u32; 4], size: (u32, u32)) -> Result<Self, Refused> {
        let passes = shape.passes();
        let widest = passes.iter().map(|pass| pass.w).max().unwrap_or(0);
        let pitch = shape.pitch(widest);
        let memory = |_| Refused::Memory;
        let (mut line, mut prior, mut wide) = (Vec::new(), Vec::new(), Vec::new());
        line.try_reserve_exact(pitch.saturating_add(1)).map_err(memory)?;
        prior.try_reserve_exact(pitch).map_err(memory)?;
        wide.try_reserve_exact(widest as usize).map_err(memory)?;
        let out = Out::new(&shape, part, size)?;
        let mut rows = Self {
            shape,
            colors,
            passes,
            pass: 0,
            row: 0,
            line,
            filled: 0,
            prior,
            wide,
            out,
            sum: yazi::Adler32::new(),
            came: 0,
            faulty: false,
            strange: false,
        };
        rows.begin();
        Ok(rows)
    }

    /// Makes room for the rows of the pass that is next. None of this
    /// asks for memory: the widest pass was reserved for.
    fn begin(&mut self) {
        self.row = 0;
        self.filled = 0;
        self.line.clear();
        self.prior.clear();
        self.wide.clear();
        if let Some(pass) = self.passes.get(self.pass) {
            let pitch = self.shape.pitch(pass.w);
            self.line.resize(pitch.saturating_add(1), 0);
            self.prior.resize(pitch, 0);
            self.wide.resize(pass.w as usize, [0; 4]);
        }
    }

    /// A row has come whole.
    fn take(&mut self) -> bool {
        let Some(pass) = self.passes.get(self.pass).copied() else { return false };
        self.sum.update(&self.line);
        let Some((filter, data)) = self.line.split_first_mut() else { return false };
        if !unfilter(*filter, self.shape.step(), data, &self.prior) {
            return false;
        }
        self.strange |= expand(&self.shape, self.colors, data, &mut self.wide);
        self.out.put(&pass, self.row, &self.wide);
        self.prior.iter_mut().zip(data.iter()).for_each(|(prior, byte)| *prior = *byte);
        self.came = self.came.saturating_add(1);
        self.filled = 0;
        self.row = self.row.saturating_add(1);
        if self.row >= pass.h {
            self.pass = self.pass.saturating_add(1);
            self.begin();
        }
        true
    }

    /// Whether every row of the picture came.
    fn whole(&self) -> bool {
        self.pass >= self.passes.len() && !self.faulty
    }
}

impl Write for Rows<'_> {
    #[inline]
    fn write(&mut self, mut bytes: &[u8]) -> std::io::Result<usize> {
        // `yazi` hands over most of what it unpacks one byte at a time,
        // a hundred million times for a large picture: the short way.
        if let [byte] = bytes
            && !self.faulty
            && let Some(place) = self.line.get_mut(self.filled)
        {
            *place = *byte;
            self.filled = self.filled.saturating_add(1);
            if self.filled >= self.line.len() && !self.take() {
                self.faulty = true;
                return Err(ErrorKind::InvalidData.into());
            }
            return Ok(1);
        }
        let taken = bytes.len();
        while !bytes.is_empty() {
            if self.faulty {
                return Err(ErrorKind::InvalidData.into());
            }
            let room = self.line.get_mut(self.filled..).unwrap_or_default();
            // More than the head of the file reckons: deflate can give
            // out a thousand times what went in.
            if room.is_empty() {
                self.faulty = true;
                return Err(ErrorKind::InvalidData.into());
            }
            let now = room.len().min(bytes.len());
            let (part, rest) = bytes.split_at(now);
            room.iter_mut().zip(part).for_each(|(to, from)| *to = *from);
            self.filled = self.filled.saturating_add(now);
            bytes = rest;
            if self.filled >= self.line.len() && !self.take() {
                self.faulty = true;
                return Err(ErrorKind::InvalidData.into());
            }
        }
        Ok(taken)
    }

    /// All of it is taken at once, or none of it is.
    #[inline]
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.write(bytes).map(drop)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The table for the sum over a chunk.
#[expect(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    reason = "reckoned when codn is built, with counts that stay below 256 and 8"
)]
const fn table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut at = 0;
    while at < 256 {
        let (mut value, mut bit) = (at as u32, 0);
        while bit < 8 {
            value = if value & 1 == 1 { 0xedb8_8320 ^ (value >> 1) } else { value >> 1 };
            bit += 1;
        }
        table[at] = value;
        at += 1;
    }
    table
}

static TABLE: [u32; 256] = table();

/// Takes more bytes into the sum over a chunk.
fn sum(mut sum: u32, bytes: &[u8]) -> u32 {
    for byte in bytes {
        let at = usize::from((sum as u8) ^ *byte);
        sum = TABLE.get(at).copied().unwrap_or(0) ^ (sum >> 8);
    }
    sum
}

/// The file, read piece by piece and no further than it may be.
struct Input<R> {
    from: R,
    /// The sum over the chunk that is read.
    sum: u32,
}

impl<R: Read> Input<R> {
    /// Reads into `into` and says how much of it was filled: less than
    /// all of it where the file ends before or cannot be read on, which
    /// both are a file that ends early.
    fn some(&mut self, into: &mut [u8]) -> usize {
        let mut filled = 0;
        while let Some(left) = into.get_mut(filled..).filter(|left| !left.is_empty()) {
            match self.from.read(left) {
                Ok(0) => break,
                Ok(read) => filled = filled.saturating_add(read),
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        filled.min(into.len())
    }

    /// Fills `into`, or says that the file ends before.
    fn fill(&mut self, into: &mut [u8]) -> bool {
        self.some(into) == into.len()
    }

    /// Fills `into` with bytes of the chunk that is read, or says that
    /// the file ends before.
    fn body(&mut self, into: &mut [u8]) -> bool {
        let whole = self.fill(into);
        if whole {
            self.sum = sum(self.sum, into);
        }
        whole
    }

    /// The same for the data of a picture, of which what is there is
    /// taken: how much of `into` was filled.
    fn data(&mut self, into: &mut [u8]) -> usize {
        let filled = self.some(into);
        self.sum = sum(self.sum, into.get(..filled).unwrap_or_default());
        filled
    }

    /// The length and the name of the chunk that begins here, if the
    /// file goes on that far.
    fn chunk(&mut self) -> Option<(u32, [u8; 4])> {
        let mut head = [0; 8];
        if !self.fill(&mut head) {
            return None;
        }
        let [l0, l1, l2, l3, name @ ..] = head;
        self.sum = sum(u32::MAX, &name);
        Some((u32::from_be_bytes([l0, l1, l2, l3]), name))
    }

    /// Reads the sum the file names for the chunk that was read. Whether
    /// it is the sum over it, or nothing where the file ends before.
    fn sound(&mut self) -> Option<bool> {
        let mut named = [0; 4];
        self.fill(&mut named).then(|| u32::from_be_bytes(named) == !self.sum)
    }

    /// Reads over the rest of a chunk and its sum, holding nothing of
    /// it. Whether its sum is right, or nothing where the file ends
    /// before.
    fn skip(&mut self, length: u32, piece: &mut [u8]) -> Option<bool> {
        let mut left = length as usize;
        while left > 0 {
            let now = piece.get_mut(..left.min(PIECE))?;
            if now.is_empty() || !self.body(now) {
                return None;
            }
            left = left.saturating_sub(now.len());
        }
        self.sound()
    }
}

/// Reads a PNG from `from` and makes of it the picture the wish asks
/// for.
pub fn decode(from: impl Read, wish: &Wish) -> Result<Picture, Refused> {
    let mut input = Input { from, sum: 0 };
    let mut signature = [0; 8];
    if !input.fill(&mut signature) || signature != SIGNATURE {
        return Err(Refused::Format);
    }
    // The head comes first, once, with thirteen bytes.
    let mut head = [0; 13];
    match input.chunk() {
        Some((_, name)) if name == *b"CgBI" => return Err(Refused::Apple),
        Some((13, name)) if name == *b"IHDR" && input.body(&mut head) && input.sound() == Some(true) => {}
        _ => return Err(Refused::Damaged),
    }
    let shape = Shape::read(&head)?;

    let mut piece = zeroed(PIECE as u64, 0u8)?;
    let mut notes = Notes::default();
    let mut colors = Colors::default();
    let (mut plain, mut alphas) = ([0u8; 768], [255u8; 256]);
    let mut entries = 0;
    let mut chunks = 1u32;
    // What stands before the data.
    let mut length = loop {
        // A file that ends here holds nothing to show.
        let (length, name) = input.chunk().ok_or(Refused::Damaged)?;
        chunks = chunks.saturating_add(1);
        if chunks > CHUNKS {
            return Err(Refused::Damaged);
        }
        match &name {
            b"IDAT" => break length,
            b"PLTE" => {
                let body = plain.get_mut(..length as usize).ok_or(Refused::Damaged)?;
                if entries > 0 || body.is_empty() || body.len() % 3 != 0 || !input.body(body) {
                    return Err(Refused::Damaged);
                }
                entries = body.len() / 3;
                if input.sound() != Some(true) {
                    return Err(Refused::Damaged);
                }
            }
            b"tRNS" => {
                let mut body = [0u8; 256];
                let body = body.get_mut(..length as usize).ok_or(Refused::Damaged)?;
                if !input.body(body) || input.sound() != Some(true) {
                    return Err(Refused::Damaged);
                }
                let sample = |at: usize| {
                    body.get(at..)
                        .and_then(|from| from.first_chunk::<2>())
                        .map(|[high, low]| wide_sample(*high, *low))
                };
                match (shape.kind, body.len()) {
                    // An alpha for each entry, and none for those it
                    // does not reach.
                    (Kind::Palette, count) if count <= entries => {
                        alphas.iter_mut().zip(body.iter()).for_each(|(alpha, named)| *alpha = *named);
                    }
                    (Kind::Gray, 2) => colors.key = sample(0).map(|v| [v; 3]),
                    (Kind::Rgb, 6) => {
                        colors.key = sample(0).zip(sample(2)).zip(sample(4)).map(|((r, g), b)| [r, g, b]);
                    }
                    // The kinds with alpha have none.
                    (Kind::GrayAlpha | Kind::Rgba, _) => {}
                    _ => return Err(Refused::Damaged),
                }
            }
            b"IHDR" | b"IEND" => return Err(Refused::Damaged),
            // What the file calls needed and is not known here.
            [first, ..] if first & 0x20 == 0 => return Err(Refused::Damaged),
            b"cHRM" if length == 32 => {
                let mut places = [0; 32];
                if !input.body(&mut places) {
                    return Err(Refused::Damaged);
                }
                let right = input.sound().ok_or(Refused::Damaged)?;
                // Where white and the three colors of a screen lie, as
                // most programs write it beside the rest. Others are
                // colors that would have to be made into those.
                let named = places.as_chunks::<4>().0.iter().map(|place| u32::from_be_bytes(*place));
                let screen = named.zip(SCREEN).all(|(named, screen)| named.abs_diff(screen) <= 500);
                if right && !screen {
                    notes.add(Notes::UNAPPLIED);
                }
            }
            b"gAMA" if length == 4 => {
                let mut gamma = [0; 4];
                if !input.body(&mut gamma) {
                    return Err(Refused::Damaged);
                }
                let right = input.sound().ok_or(Refused::Damaged)?;
                // What is written for the colors of a screen is 45455.
                if right && !(45_000..=46_000).contains(&u32::from_be_bytes(gamma)) {
                    notes.add(Notes::UNAPPLIED);
                }
            }
            _ => {
                let right = input.skip(length, &mut piece).ok_or(Refused::Damaged)?;
                match &name {
                    b"iCCP" if right => notes.add(Notes::UNAPPLIED),
                    b"acTL" if right => notes.add(Notes::ANIMATION),
                    _ => {}
                }
            }
        }
    };
    if shape.kind == Kind::Palette {
        if entries == 0 {
            return Err(Refused::Damaged);
        }
        colors.palette.try_reserve_exact(entries).map_err(|_| Refused::Memory)?;
        let named = plain.as_chunks::<3>().0.iter().zip(alphas).take(entries);
        colors.palette.extend(named.map(|(&[r, g, b], alpha)| pixel(r, g, b, alpha)));
    }

    let part = crop(&shape, wish);
    let (size, smaller) = plan(&shape, part, wish)?;
    if smaller {
        notes.add(Notes::SMALLER);
    }
    let mut rows = Rows::new(shape, &colors, part, size)?;
    let mut decoder = yazi::Decoder::boxed();
    decoder.set_format(yazi::Format::Zlib);
    let mut stream = decoder.stream(&mut rows);
    // How the data ended, where it did not end as it should, and the
    // chunk that follows it, where one does.
    let (mut early, mut damaged, mut following) = (false, false, None);
    'data: loop {
        let mut left = length as usize;
        while left > 0 {
            let Some(now) = piece.get_mut(..left.min(PIECE)).filter(|now| !now.is_empty()) else {
                break 'data;
            };
            // What the file has of a chunk that it does not have whole
            // is data all the same.
            let filled = input.data(now);
            let have = now.get(..filled).unwrap_or_default();
            // The writer refused, or the data is no deflate.
            if stream.write(have).is_err() {
                damaged = true;
                break 'data;
            }
            if filled < now.len() {
                early = true;
                break 'data;
            }
            left = left.saturating_sub(filled);
        }
        match input.sound() {
            Some(true) => {}
            Some(false) => {
                // The picture ends with this chunk. What it held is
                // shown: `yazi` may hold all of a small file until it
                // is told that no more comes.
                damaged = true;
                break;
            }
            None => {
                early = true;
                break;
            }
        }
        // Data goes on in the chunk that follows, and in no later one.
        match input.chunk() {
            Some((next, name)) if name == *b"IDAT" => {
                chunks = chunks.saturating_add(1);
                if chunks > CHUNKS {
                    damaged = true;
                    break;
                }
                length = next;
            }
            next => {
                following = next;
                early = next.is_none();
                break;
            }
        }
    }
    // What follows the data, up to the chunk that ends the picture:
    // nothing of it is held, but one that the file calls needed and
    // that is not known turns the file away here as before the data. A
    // file that stops before that chunk ends early.
    while let Some((length, name)) = following {
        chunks = chunks.saturating_add(1);
        match &name {
            b"IEND" => break,
            // Data that is taken up again, and what had its place
            // before the data: not taken, and not unknown.
            b"IDAT" | b"IHDR" | b"PLTE" => {}
            [first, ..] if first & 0x20 == 0 => return Err(Refused::Damaged),
            _ => {}
        }
        if chunks > CHUNKS {
            damaged = true;
            break;
        }
        if input.skip(length, &mut piece).is_none() {
            early = true;
            break;
        }
        following = input.chunk();
        early = following.is_none();
    }
    let ended = stream.finish();
    let named = std::mem::take(&mut rows.sum).finish();
    if rows.came == 0 {
        return Err(Refused::Damaged);
    }
    if damaged || rows.faulty || rows.strange {
        notes.add(Notes::DAMAGED);
    } else if !rows.whole() {
        // The data stops, in the file or in itself.
        notes.add(if early || matches!(ended, Err(yazi::Error::Underflow)) {
            Notes::EARLY
        } else {
            Notes::DAMAGED
        });
    } else {
        match ended {
            // Every row came. The file is whole only where it also says
            // that it ends: one that stops before, inside the sum over
            // its rows or after it, is one that is still being written.
            Ok((_, Some(sum))) if sum == named && !early => {}
            Ok((_, Some(sum))) if sum == named => notes.add(Notes::EARLY),
            Err(yazi::Error::Underflow) if early => notes.add(Notes::EARLY),
            _ => notes.add(Notes::DAMAGED),
        }
    }
    let pixels = rows.out.end();
    if pixels.iter().any(|[.., alpha]| *alpha != 255) {
        notes.add(Notes::ALPHA);
    }
    let pixels = pixels.into_flattened();
    Ok(Picture { decoded: Decoded { picture: (shape.w, shape.h), part, size, notes }, pixels })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::arithmetic_side_effects)]
mod tests {
    use super::*;
    use crate::core::picture::Format;

    fn chunk(name: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let named = !sum(sum(u32::MAX, name), body);
        [&(body.len() as u32).to_be_bytes()[..], name, body, &named.to_be_bytes()].concat()
    }

    fn head(w: u32, h: u32, depth: u8, kind: u8, lace: u8) -> Vec<u8> {
        chunk(b"IHDR", &[&w.to_be_bytes()[..], &h.to_be_bytes(), &[depth, kind, 0, 0, lace]].concat())
    }

    fn pack(raw: &[u8]) -> Vec<u8> {
        yazi::compress(raw, yazi::Format::Zlib, yazi::CompressionLevel::Default).unwrap()
    }

    /// A file of these chunks, and of the one that ends a picture where
    /// they do not end with it.
    fn file(chunks: &[Vec<u8>]) -> Vec<u8> {
        let end = chunk(b"IEND", &[]);
        let ended = chunks.last().is_some_and(|last| *last == end);
        [open(chunks), if ended { Vec::new() } else { end }].concat()
    }

    /// A file of these chunks and no more: one that is still written.
    fn open(chunks: &[Vec<u8>]) -> Vec<u8> {
        [&SIGNATURE[..], &chunks.concat()].concat()
    }

    /// Rows as the file has them, each with a filter that changes nothing.
    fn raw(rows: &[Vec<u8>]) -> Vec<u8> {
        rows.iter().flat_map(|row| [&[0][..], row].concat()).collect()
    }

    fn plain(w: u32, h: u32, depth: u8, kind: u8, rows: &[Vec<u8>]) -> Vec<u8> {
        file(&[head(w, h, depth, kind, 0), chunk(b"IDAT", &pack(&raw(rows))), chunk(b"IEND", &[])])
    }

    /// The rows of a picture of four bytes a pixel, in the order Adam7
    /// has them.
    fn laced(w: usize, h: usize, pixels: &[[u8; 4]]) -> Vec<Vec<u8>> {
        let mut rows = Vec::new();
        for (x0, y0, dx, dy, _) in ADAM7 {
            for y in (y0 as usize..h).step_by(dy as usize) {
                let row: Vec<u8> =
                    (x0 as usize..w).step_by(dx as usize).flat_map(|x| pixels[y * w + x]).collect();
                if !row.is_empty() {
                    rows.push(row);
                }
            }
        }
        rows
    }

    fn wish(room: (u32, u32), whole: u32) -> Wish {
        Wish { format: Format::Png, room, side: 4096, whole, part: [0; 4] }
    }

    fn whole() -> Wish {
        wish((16, 16), WHOLE)
    }

    fn seen(bytes: &[u8], wish: &Wish) -> Picture {
        decode(bytes, wish).unwrap_or_else(|reason| panic!("turned away: {reason:?}"))
    }

    fn why(bytes: &[u8]) -> Option<Refused> {
        decode(bytes, &whole()).err()
    }

    /// A picture with a color for every place, none of them alike, some
    /// with alpha.
    fn colors(w: usize, h: usize) -> Vec<[u8; 4]> {
        (0..w * h)
            .map(|at| [(at * 7) as u8, (at * 13 + 5) as u8, (at * 29 + 11) as u8, 255 - (at * 3) as u8])
            .collect()
    }

    fn multiplied(pixels: &[[u8; 4]]) -> Vec<u8> {
        let times = |c: u8, a: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
        pixels.iter().flat_map(|&[r, g, b, a]| [times(r, a), times(g, a), times(b, a), a]).collect()
    }

    #[test]
    fn the_sum_over_a_chunk_is_the_one_the_format_names() {
        assert_eq!(!sum(u32::MAX, b"123456789"), 0xcbf4_3926);
        assert_eq!(!sum(sum(u32::MAX, b"1234"), b"56789"), 0xcbf4_3926);
    }

    #[test]
    fn color_with_alpha_comes_out_multiplied_by_it() {
        let pixels = colors(3, 2);
        let rows: Vec<Vec<u8>> = pixels.chunks(3).map(|row| row.concat()).collect();
        let picture = seen(&plain(3, 2, 8, 6, &rows), &whole());
        assert_eq!(picture.decoded, Decoded::whole((3, 2), (3, 2), Notes(Notes::ALPHA)));
        assert_eq!(picture.pixels, multiplied(&pixels));
        // Without alpha nothing is to be said of it.
        let opaque = seen(&plain(1, 1, 8, 2, &[vec![1, 2, 3]]), &whole());
        assert_eq!((opaque.pixels, opaque.decoded.notes), (vec![1, 2, 3, 255], Notes(0)));
    }

    #[test]
    fn every_filter_is_undone() {
        // The same picture, each row filtered another way, against the
        // picture itself.
        let (w, h, step) = (5usize, 6usize, 4usize);
        let pixels = colors(w, h);
        let rows: Vec<Vec<u8>> = pixels.chunks(w).map(|row| row.concat()).collect();
        let mut data = Vec::new();
        for (y, row) in rows.iter().enumerate() {
            let filter = (y % 5) as u8;
            let up = |x: usize| if y > 0 { rows[y - 1][x] } else { 0 };
            data.push(filter);
            for (x, byte) in row.iter().enumerate() {
                let left = if x >= step { row[x - step] } else { 0 };
                let corner = if x >= step { up(x - step) } else { 0 };
                let guess = match filter {
                    0 => 0,
                    1 => left,
                    2 => up(x),
                    3 => ((left as u16 + up(x) as u16) / 2) as u8,
                    _ => paeth(left, up(x), corner),
                };
                data.push(byte.wrapping_sub(guess));
            }
        }
        let bytes = file(&[head(w as u32, h as u32, 8, 6, 0), chunk(b"IDAT", &pack(&data))]);
        assert_eq!(seen(&bytes, &whole()).pixels, multiplied(&pixels));
        // Paeth takes the nearest of the three, and the left one first.
        assert_eq!(
            (paeth(10, 20, 30), paeth(10, 20, 10), paeth(10, 20, 20), paeth(5, 5, 5)),
            (10, 20, 10, 5)
        );

        // A filter that is none ends the picture there: the row before
        // it is shown, and the file is said to be damaged.
        let mut wrong = raw(&rows);
        wrong[(1 + w * 4) * 2] = 5;
        let picture =
            seen(&file(&[head(w as u32, h as u32, 8, 6, 0), chunk(b"IDAT", &pack(&wrong))]), &whole());
        assert!(picture.decoded.notes.has(Notes::DAMAGED));
        assert_eq!(picture.pixels[..w * 8], multiplied(&pixels)[..w * 8]);
        assert!(picture.pixels[w * 8..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn gray_of_every_depth_is_spread_over_the_whole_range() {
        // Seven pixels in one bit: the eighth bit of the byte is none.
        let one = seen(&plain(7, 1, 1, 0, &[vec![0b1010_0110]]), &whole());
        let gray = |picture: &Picture| picture.pixels.chunks(4).map(|pixel| pixel[0]).collect::<Vec<_>>();
        assert_eq!(gray(&one), [255, 0, 255, 0, 0, 255, 255]);
        assert!(
            one.pixels.chunks(4).all(|pixel| pixel[0] == pixel[1] && pixel[1] == pixel[2] && pixel[3] == 255)
        );
        assert_eq!(
            gray(&seen(&plain(5, 1, 2, 0, &[vec![0b0001_1011, 0b1100_0000]]), &whole())),
            [0, 85, 170, 255, 255]
        );
        assert_eq!(gray(&seen(&plain(3, 1, 4, 0, &[vec![0x0f, 0x80]]), &whole())), [0, 255, 136]);
        assert_eq!(gray(&seen(&plain(2, 1, 8, 0, &[vec![7, 200]]), &whole())), [7, 200]);
        // Sixteen bits are rounded to eight, not cut.
        let wide = seen(&plain(4, 1, 16, 0, &[vec![0, 0, 0, 128, 0x80, 0x80, 0xff, 0xff]]), &whole());
        assert_eq!(gray(&wide), [0, 0, 128, 255]);
        assert_eq!((narrow(0x00ff), narrow(0x0100), narrow(0x7fff), narrow(0xfeff)), (1, 1, 127, 254));
        // Gray with alpha, in eight and in sixteen bits.
        let two = seen(&plain(1, 1, 8, 4, &[vec![200, 128]]), &whole());
        assert_eq!(two.pixels, multiplied(&[[200, 200, 200, 128]]));
        let four = seen(&plain(1, 1, 16, 4, &[vec![0xc8, 0xc8, 0x80, 0x80]]), &whole());
        assert_eq!(four.pixels, multiplied(&[[200, 200, 200, 128]]));
    }

    #[test]
    fn a_palette_gives_each_entry_its_color_and_its_alpha() {
        let palette = chunk(b"PLTE", &[10, 20, 30, 40, 50, 60, 70, 80, 90]);
        let data = chunk(b"IDAT", &pack(&raw(&[vec![0b00_01_10_00]])));
        let bytes = file(&[head(4, 1, 2, 3, 0), palette.clone(), chunk(b"tRNS", &[255, 100]), data.clone()]);
        let picture = seen(&bytes, &whole());
        // The third entry has no alpha named, and is opaque.
        assert_eq!(
            picture.pixels,
            multiplied(&[[10, 20, 30, 255], [40, 50, 60, 100], [70, 80, 90, 255], [10, 20, 30, 255]])
        );
        assert_eq!(picture.decoded.notes, Notes(Notes::ALPHA));

        // A place the palette does not have is black, and said.
        let beyond = chunk(b"IDAT", &pack(&raw(&[vec![0b00_11_00_00]])));
        let picture = seen(&file(&[head(2, 1, 2, 3, 0), palette.clone(), beyond]), &whole());
        assert_eq!(picture.pixels, [10, 20, 30, 255, 0, 0, 0, 255]);
        assert_eq!(picture.decoded.notes, Notes(Notes::DAMAGED));

        // No palette, one that three does not divide, two of them, an
        // alpha for more entries than there are, one after the data.
        assert_eq!(why(&file(&[head(4, 1, 2, 3, 0), data.clone()])), Some(Refused::Damaged));
        let odd = chunk(b"PLTE", &[1, 2, 3, 4]);
        assert_eq!(why(&file(&[head(4, 1, 2, 3, 0), odd, data.clone()])), Some(Refused::Damaged));
        let twice = file(&[head(4, 1, 2, 3, 0), palette.clone(), palette.clone(), data.clone()]);
        assert_eq!(why(&twice), Some(Refused::Damaged));
        let many = file(&[head(4, 1, 2, 3, 0), palette.clone(), chunk(b"tRNS", &[1, 2, 3, 4]), data.clone()]);
        assert_eq!(why(&many), Some(Refused::Damaged));
        assert_eq!(why(&file(&[head(4, 1, 2, 3, 0), data, palette])), Some(Refused::Damaged));
    }

    #[test]
    fn the_one_color_that_is_transparent_is_held_against_the_sample_as_it_is() {
        // Two colors of sixteen bits that are the same in eight: only
        // the one that is named goes.
        let rows = [vec![0x12, 0x34, 0, 0, 0, 0, 0x12, 0x35, 0, 0, 0, 0]];
        let key = chunk(b"tRNS", &[0x12, 0x34, 0, 0, 0, 0]);
        let bytes = file(&[head(2, 1, 16, 2, 0), key, chunk(b"IDAT", &pack(&raw(&rows)))]);
        let picture = seen(&bytes, &whole());
        assert_eq!(narrow(0x1234), narrow(0x1235));
        assert_eq!(picture.pixels, [0, 0, 0, 0, narrow(0x1235), 0, 0, 255]);
        // In eight bits, and for gray, where the sample is what counts
        // and not what it is spread to.
        let key = chunk(b"tRNS", &[0, 9, 0, 8, 0, 7]);
        let bytes = file(&[head(2, 1, 8, 2, 0), key, chunk(b"IDAT", &pack(&raw(&[vec![9, 8, 7, 9, 8, 8]])))]);
        assert_eq!(seen(&bytes, &whole()).pixels, [0, 0, 0, 0, 9, 8, 8, 255]);
        let key = chunk(b"tRNS", &[0, 2]);
        let bytes = file(&[head(4, 1, 2, 0, 0), key, chunk(b"IDAT", &pack(&raw(&[vec![0b00_01_10_11]])))]);
        assert_eq!(
            seen(&bytes, &whole()).pixels,
            [0, 0, 0, 255, 85, 85, 85, 255, 0, 0, 0, 0, 255, 255, 255, 255]
        );
        // In a kind with alpha it says nothing, and is no fault.
        let bytes = file(&[
            head(1, 1, 8, 6, 0),
            chunk(b"tRNS", &[1]),
            chunk(b"IDAT", &pack(&raw(&[vec![1, 2, 3, 255]]))),
        ]);
        assert_eq!(seen(&bytes, &whole()).decoded.notes, Notes(0));
    }

    #[test]
    fn adam7_gives_the_same_picture_at_every_size() {
        for (w, h) in (1..=9).flat_map(|w| (1..=9).map(move |h| (w, h))).chain([(33, 17)]) {
            let pixels = colors(w, h);
            let rows = laced(w, h, &pixels);
            let bytes = file(&[
                head(w as u32, h as u32, 8, 6, 1),
                chunk(b"IDAT", &pack(&raw(&rows))),
                chunk(b"IEND", &[]),
            ]);
            let picture = seen(&bytes, &whole());
            assert_eq!(picture.pixels, multiplied(&pixels), "{w} by {h}");
            assert!(!picture.decoded.notes.partial(), "{w} by {h}");
        }
    }

    #[test]
    fn a_picture_is_made_smaller_as_the_mean_of_what_falls_into_a_field() {
        // Left of the middle nothing, right of it white that is half
        // there, in fields of two by two: and a row of fields where
        // every other pixel is white.
        let (w, h) = (32usize, 32usize);
        let pixels: Vec<[u8; 4]> = (0..w * h)
            .map(|at| match (at % w, at / w) {
                (x, 0 | 1) => [255, 255, 255, if x % 2 == 0 { 255 } else { 0 }],
                (x, _) if x >= 16 => [255, 255, 255, 128],
                _ => [0, 0, 0, 0],
            })
            .collect();
        let rows: Vec<Vec<u8>> = pixels.chunks(w).map(|row| row.concat()).collect();
        let small = wish((16, 16), 0);
        let from_top = seen(&plain(32, 32, 8, 6, &rows), &small);
        assert_eq!(from_top.decoded, Decoded::whole((32, 32), (16, 16), Notes(Notes::ALPHA)));
        let field = |picture: &Picture, x: usize, y: usize| picture.pixels[(y * 16 + x) * 4..][..4].to_vec();
        // The mean is of colors multiplied by their alpha: half white
        // and half nothing is white at half, not gray.
        assert_eq!(field(&from_top, 3, 0), [128, 128, 128, 128]);
        assert_eq!(field(&from_top, 3, 5), [0, 0, 0, 0]);
        assert_eq!(field(&from_top, 12, 5), [128, 128, 128, 128]);
        // With Adam7 the pixels come scattered, and the picture is the
        // same.
        let laced = file(&[head(32, 32, 8, 6, 1), chunk(b"IDAT", &pack(&raw(&laced(w, h, &pixels))))]);
        assert_eq!(seen(&laced, &small).pixels, from_top.pixels);
        // A room it fits into leaves it as it is, also where it is not
        // wanted whole.
        assert_eq!(seen(&plain(32, 32, 8, 6, &rows), &wish((40, 32), 0)).decoded.size, (32, 32));
    }

    #[test]
    fn the_size_follows_the_room_and_keeps_the_shape() {
        let room = wish((1000, 667), WHOLE);
        // Small enough to be kept whole, whatever the room.
        assert_eq!(fit((3840, 2160), &room), (3840, 2160));
        // Larger: the tighter side decides.
        assert_eq!(fit((6238, 4162), &room), (999, 667));
        assert_eq!(fit((6000, 2000), &room), (1000, 333));
        assert_eq!(fit((2000, 6000), &room), (222, 667));
        // Never larger than it is, and never without a size.
        assert_eq!(fit((500, 300), &wish((1000, 667), 0)), (500, 300));
        assert_eq!(fit((60_000, 2), &wish((1000, 667), 0)), (1000, 1));
        // No side longer than a texture takes, also where it is whole.
        assert_eq!(fit((4000, 100), &Wish { side: 1024, ..room }), (1024, 25));
        assert_eq!(fit((6238, 4162), &Wish { side: 512, ..room }), (512, 341));
        // A room that is no room is taken for the least there is.
        assert_eq!(fit((6400, 6400), &wish((1, 1), 0)), (16, 16));
        // What the process is told to keep whole is held to its own
        // number.
        assert_eq!(fit((4000, 4000), &wish((100, 100), u32::MAX)), (100, 100));
    }

    #[test]
    fn a_file_that_ends_early_shows_what_is_there() {
        let (w, h) = (4usize, 40usize);
        let pixels = colors(w, h);
        let rows: Vec<Vec<u8>> = pixels.chunks(w).map(|row| row.concat()).collect();
        let packed = pack(&raw(&rows));
        let whole_file = file(&[head(4, 40, 8, 6, 0), chunk(b"IDAT", &packed), chunk(b"IEND", &[])]);
        assert_eq!(seen(&whole_file, &whole()).pixels, multiplied(&pixels));
        let picture = seen(&whole_file[..whole_file.len() / 2], &whole());
        assert!(picture.decoded.notes.has(Notes::EARLY) && !picture.decoded.notes.has(Notes::DAMAGED));
        let came = picture.pixels.chunks(w * 4).take_while(|row| row.iter().any(|byte| *byte != 0)).count();
        assert!((5..35).contains(&came), "{came} rows came");
        assert_eq!(picture.pixels[..came * w * 4], multiplied(&pixels)[..came * w * 4]);
        assert!(picture.pixels[came * w * 4..].iter().all(|byte| *byte == 0));

        // Cut at every length: an answer each time, and never a picture
        // that is said to be whole before the file says that it ends. With
        // every pixel there and the sum over the data missing, it was said
        // to be whole. Only the sum over the chunk that ends the file is
        // not waited for.
        let ends = whole_file.len() - 4;
        for length in 0..whole_file.len() {
            match decode(&whole_file[..length], &whole()) {
                Ok(picture) => {
                    assert_eq!(picture.decoded.notes.partial(), length < ends, "at {length}");
                    assert!(!picture.decoded.notes.has(Notes::DAMAGED), "at {length}");
                }
                Err(reason) => {
                    assert!(matches!(reason, Refused::Format | Refused::Damaged), "{reason:?} at {length}");
                    assert!(length < ends, "no picture at {length}");
                }
            }
        }
        // All the pixels and none of the sum over them: every row is
        // shown, and the file is said to end early.
        let pixels_end = whole_file.len() - 12 - 4;
        let unsummed = seen(&whole_file[..pixels_end], &whole());
        assert_eq!(unsummed.pixels, multiplied(&pixels));
        assert!(unsummed.decoded.notes.has(Notes::EARLY));

        // With Adam7 what is there is the whole picture coarsely: a
        // pixel of a pass that came stands for its block, and so no
        // place of the picture is left empty.
        let pixels = colors(8, 8);
        let packed = pack(&raw(&laced(8, 8, &pixels)));
        let coarse =
            seen(&file(&[head(8, 8, 8, 6, 1), chunk(b"IDAT", &packed[..packed.len() / 2])]), &whole());
        assert!(coarse.decoded.notes.has(Notes::EARLY));
        assert_ne!(coarse.pixels, multiplied(&pixels));
        assert_eq!(coarse.pixels[..4], multiplied(&pixels[..1]));
        assert!(coarse.pixels.chunks(4).all(|pixel| pixel[3] != 0));
    }

    #[test]
    fn adam7_that_ends_early_is_the_whole_picture_coarsely_also_made_smaller() {
        // Made smaller, a pixel of a pass stood for its one field and for
        // no block, and a file cut in its first passes was a grid of them
        // on nothing.
        let (w, h) = (32usize, 32usize);
        let pixels: Vec<[u8; 4]> = (0..w * h).map(|at| [(at % w) as u8, (at / w) as u8, 9, 255]).collect();
        let rows = laced(w, h, &pixels);
        let small = wish((16, 16), 0);
        let cut = |rows: &[Vec<u8>]| file(&[head(32, 32, 8, 6, 1), chunk(b"IDAT", &pack(&raw(rows)))]);
        // The first pass alone, four pixels by four: each stands for
        // eight by eight of the picture, which is four fields by four.
        // The data ends as data should, with rows missing: not a file
        // that ends early, but one that does not hold what it says.
        let first = seen(&cut(&rows[..4]), &small);
        assert!(first.decoded.notes.has(Notes::DAMAGED));
        for (at, field) in first.pixels.chunks(4).enumerate() {
            let (x, y) = (at % 16, at / 16);
            assert_eq!(field, [(x / 4 * 8) as u8, (y / 4 * 8) as u8, 9, 255], "the field {x} {y}");
        }
        // Half of that pass: the blocks of the rows that did not come
        // are nothing, and not what stands above them.
        let half = seen(&cut(&rows[..2]), &small);
        assert!(half.pixels.chunks(4).take(8 * 16).all(|field| field[3] == 255));
        assert!(half.pixels.chunks(4).skip(8 * 16).all(|field| field == [0; 4]));
        // All of it: every field the mean of its own four pixels, and
        // nothing of a block left in it.
        let all = seen(&cut(&rows), &small);
        let from_top: Vec<Vec<u8>> = pixels.chunks(w).map(|row| row.concat()).collect();
        assert_eq!(all.pixels, seen(&plain(32, 32, 8, 6, &from_top), &small).pixels);
        assert!(!all.decoded.notes.partial());
    }

    #[test]
    fn filters_look_back_by_the_bytes_of_a_pixel() {
        // Samples packed into a byte look back one byte, color of
        // sixteen bits with alpha eight: each row filtered another way
        // and held against the same rows without a filter.
        for (depth, kind, step, pitch) in
            [(2u8, 0u8, 1usize, 3usize), (16, 6, 8, 40), (8, 2, 3, 15), (16, 0, 2, 10)]
        {
            let (w, h) = ((pitch * 8 / (step * 8).max(1)).max(1), 6usize);
            let w = if depth == 2 { 12 } else { w };
            let rows: Vec<Vec<u8>> =
                (0..h).map(|y| (0..pitch).map(|x| (x * 37 + y * 101 + 13) as u8).collect()).collect();
            let mut data = Vec::new();
            for (y, row) in rows.iter().enumerate() {
                let filter = (y % 4 + 1) as u8;
                let up = |x: usize| if y > 0 { rows[y - 1][x] } else { 0 };
                data.push(filter);
                for (x, byte) in row.iter().enumerate() {
                    let left = if x >= step { row[x - step] } else { 0 };
                    let corner = if x >= step { up(x - step) } else { 0 };
                    let guess = match filter {
                        1 => left,
                        2 => up(x),
                        3 => ((left as u16 + up(x) as u16) / 2) as u8,
                        _ => paeth(left, up(x), corner),
                    };
                    data.push(byte.wrapping_sub(guess));
                }
            }
            let filtered = file(&[head(w as u32, h as u32, depth, kind, 0), chunk(b"IDAT", &pack(&data))]);
            let sound = seen(&plain(w as u32, h as u32, depth, kind, &rows), &whole());
            let picture = seen(&filtered, &whole());
            assert_eq!(picture.pixels, sound.pixels, "depth {depth}, kind {kind}");
            assert!(!picture.decoded.notes.partial(), "depth {depth}, kind {kind}");
        }
    }

    #[test]
    fn a_sum_that_is_wrong_is_said_or_turns_the_file_away() {
        let rows = [vec![1, 2, 3, 255, 4, 5, 6, 255], vec![7, 8, 9, 255, 10, 11, 12, 255]];
        let sound = plain(2, 2, 8, 6, &rows);
        assert_eq!(seen(&sound, &whole()).decoded.notes, Notes(0));
        // In the head: no picture.
        let mut wrong = sound.clone();
        wrong[8 + 8 + 13] ^= 1;
        assert_eq!(why(&wrong), Some(Refused::Damaged));
        // In the data: what is there, and said.
        let data = 8 + 25;
        let mut wrong = sound.clone();
        let length = u32::from_be_bytes(wrong[data..data + 4].try_into().unwrap()) as usize;
        wrong[data + 8 + length] ^= 1;
        assert_eq!(seen(&wrong, &whole()).decoded.notes, Notes(Notes::DAMAGED));
        // The sum over what was unpacked, which stands at the end of the
        // data and which `yazi` does not look at.
        let mut packed = pack(&raw(&rows));
        *packed.last_mut().unwrap() ^= 1;
        let wrong = file(&[head(2, 2, 8, 6, 0), chunk(b"IDAT", &packed)]);
        let picture = seen(&wrong, &whole());
        assert_eq!(picture.decoded.notes, Notes(Notes::DAMAGED));
        assert_eq!(picture.pixels, rows.concat());
    }

    #[test]
    fn more_data_than_the_head_reckons_is_not_taken() {
        // Three rows in a picture of two: the third is refused, and
        // with it everything after.
        let rows = [vec![1, 2, 3], vec![4, 5, 6], vec![7, 8, 9]];
        let picture = seen(&plain(1, 2, 8, 2, &rows), &whole());
        assert_eq!(picture.decoded.notes, Notes(Notes::DAMAGED));
        assert_eq!(picture.pixels, [1, 2, 3, 255, 4, 5, 6, 255]);
        // Much more: deflate gives out a thousand times what went in,
        // and none of it is held.
        let flood = file(&[head(1, 1, 8, 0, 0), chunk(b"IDAT", &pack(&vec![0; 4 << 20]))]);
        assert_eq!(seen(&flood, &whole()).pixels, [0, 0, 0, 255]);
    }

    #[test]
    fn data_in_many_chunks_is_one_stream() {
        let pixels = colors(6, 5);
        let rows: Vec<Vec<u8>> = pixels.chunks(6).map(|row| row.concat()).collect();
        let packed = pack(&raw(&rows));
        let mut chunks = vec![head(6, 5, 8, 6, 0)];
        chunks.extend(packed.chunks(1).map(|byte| chunk(b"IDAT", byte)));
        chunks.push(chunk(b"IDAT", &[]));
        chunks.push(chunk(b"IEND", &[]));
        let picture = seen(&file(&chunks), &whole());
        assert_eq!((picture.pixels, picture.decoded.notes), (multiplied(&pixels), Notes(Notes::ALPHA)));
        // Data that is taken up again after another chunk ends where it
        // first stopped.
        let (first, second) = packed.split_at(packed.len() / 2);
        let broken = file(&[
            head(6, 5, 8, 6, 0),
            chunk(b"IDAT", first),
            chunk(b"tEXt", b"a\0b"),
            chunk(b"IDAT", second),
        ]);
        assert!(seen(&broken, &whole()).decoded.notes.partial());
    }

    #[test]
    fn what_is_not_a_picture_that_is_shown_is_turned_away_by_name() {
        let data = chunk(b"IDAT", &pack(&raw(&[vec![1, 2, 3]])));
        assert_eq!(why(b"GIF89a"), Some(Refused::Format));
        assert_eq!(why(&SIGNATURE), Some(Refused::Damaged));
        assert_eq!(
            why(&file(&[chunk(b"CgBI", &[0; 4]), head(1, 1, 8, 2, 0), data.clone()])),
            Some(Refused::Apple)
        );
        // No head first, a head of another length, two heads.
        assert_eq!(why(&file(&[data.clone(), head(1, 1, 8, 2, 0)])), Some(Refused::Damaged));
        assert_eq!(why(&file(&[chunk(b"IHDR", &[0; 12]), data.clone()])), Some(Refused::Damaged));
        assert_eq!(
            why(&file(&[head(1, 1, 8, 2, 0), head(1, 1, 8, 2, 0), data.clone()])),
            Some(Refused::Damaged)
        );
        // No size, too long a side, too many pixels.
        assert_eq!(why(&file(&[head(0, 1, 8, 2, 0), data.clone()])), Some(Refused::Damaged));
        assert_eq!(why(&file(&[head(65_536, 1, 8, 2, 0), data.clone()])), Some(Refused::Large));
        assert_eq!(why(&file(&[head(u32::MAX, u32::MAX, 8, 2, 0), data.clone()])), Some(Refused::Large));
        assert_eq!(why(&file(&[head(65_535, 2049, 8, 2, 0), data.clone()])), Some(Refused::Large));
        // A kind with a depth it does not have, and a kind that is none.
        for (depth, kind) in [(4, 2), (16, 3), (2, 4), (1, 6), (3, 0), (8, 1), (8, 5), (0, 0), (32, 0)] {
            assert_eq!(
                why(&file(&[head(1, 1, depth, kind, 0), data.clone()])),
                Some(Refused::Kind),
                "{depth} {kind}"
            );
        }
        // Packed or filtered or laced in a way there is none of.
        for at in [10, 11, 12] {
            let mut wrong = [0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0];
            wrong[at] = 2;
            assert_eq!(why(&file(&[chunk(b"IHDR", &wrong), data.clone()])), Some(Refused::Damaged), "{at}");
        }
        // A chunk the file calls needed and that is not known, and one
        // it does not call so.
        assert_eq!(
            why(&file(&[head(1, 1, 8, 2, 0), chunk(b"ABCD", &[1]), data.clone()])),
            Some(Refused::Damaged)
        );
        assert_eq!(why(&file(&[head(1, 1, 8, 2, 0), chunk(b"abCD", &[1]), data.clone()])), None);
        // No data, and data that is none.
        assert_eq!(why(&file(&[head(1, 1, 8, 2, 0), chunk(b"IEND", &[])])), Some(Refused::Damaged));
        assert_eq!(why(&file(&[head(1, 1, 8, 2, 0), chunk(b"IDAT", b"nonsense")])), Some(Refused::Damaged));
    }

    /// A picture of 64 by 48 where every pixel is told by its place,
    /// plain and laced.
    fn placed() -> (Vec<[u8; 4]>, Vec<u8>, Vec<u8>) {
        let (w, h) = (64usize, 48usize);
        let pixels: Vec<[u8; 4]> = (0..w * h).map(|at| [(at % w) as u8, (at / w) as u8, 7, 255]).collect();
        let rows: Vec<Vec<u8>> = pixels.chunks(w).map(|row| row.concat()).collect();
        let plain = plain(w as u32, h as u32, 8, 6, &rows);
        let laced =
            file(&[head(w as u32, h as u32, 8, 6, 1), chunk(b"IDAT", &pack(&raw(&laced(w, h, &pixels))))]);
        (pixels, plain, laced)
    }

    #[test]
    fn a_part_of_the_picture_is_the_pixels_of_it_at_its_own_size() {
        let (pixels, plain, laced) = placed();
        let part = Wish { part: [5, 7, 20, 10], ..wish((100, 100), 0) };
        for bytes in [&plain, &laced] {
            let got = seen(bytes, &part);
            let want: Vec<[u8; 4]> =
                (7..17).flat_map(|y| pixels[y * 64 + 5..y * 64 + 25].iter().copied()).collect();
            assert_eq!(got.decoded.picture, (64, 48));
            assert_eq!((got.decoded.part, got.decoded.size), ([5, 7, 20, 10], (20, 10)));
            assert_eq!(got.pixels, multiplied(&want));
            assert!(!got.decoded.notes.partial());
        }
    }

    #[test]
    fn a_part_is_made_smaller_as_the_mean_of_what_falls_into_a_field_of_it() {
        let (pixels, plain, laced) = placed();
        // 32 by 16 into fields of two by two: each the mean of its
        // four pixels, in what is left of colors that are all opaque.
        let part = Wish { part: [4, 6, 32, 16], ..wish((16, 8), 0) };
        let mean = |fx: usize, fy: usize, at: usize| {
            let sum: usize =
                (0..4).map(|n| pixels[(6 + fy * 2 + n / 2) * 64 + 4 + fx * 2 + n % 2][at] as usize).sum();
            ((sum + 2) / 4) as u8
        };
        let want: Vec<u8> = (0..8)
            .flat_map(|fy| (0..16).flat_map(move |fx| [mean(fx, fy, 0), mean(fx, fy, 1), 7, 255]))
            .collect();
        let from_top = seen(&plain, &part);
        assert_eq!((from_top.decoded.part, from_top.decoded.size), ([4, 6, 32, 16], (16, 8)));
        assert_eq!(from_top.pixels, want);
        assert_eq!(seen(&laced, &part).pixels, want);
    }

    #[test]
    fn a_part_at_its_own_size_is_not_made_smaller_for_the_sums_it_does_not_use() {
        // With Adam7 every field kept four sums and a count also where a
        // field gets one pixel, and a part of 5.6 megapixels went over what
        // the process may use.
        let laced = Shape { w: 6238, h: 4162, kind: Kind::Rgba, depth: 8, adam7: true };
        let at = |part: [u32; 4]| Wish { part, ..wish((100, 100), 0) };
        for part in [[0, 0, 3000, 2000], [100, 100, 4000, 2800]] {
            let wish = Wish { room: (part[2], part[3]), ..at(part) };
            assert_eq!(plan(&laced, part, &wish).unwrap(), ((part[2], part[3]), false), "{part:?}");
        }
        // Made smaller, as before, it still keeps its sums.
        let wish = Wish { room: (1500, 1000), ..at([0, 0, 3000, 2000]) };
        assert_eq!(plan(&laced, [0, 0, 3000, 2000], &wish).unwrap(), ((1500, 1000), false));
    }

    #[test]
    fn a_part_at_its_own_size_allocates_no_sums() {
        // `plan` was tested and the allocation in `Out::new`, which is a
        // decision of its own, was not.
        let laced = Shape { w: 200, h: 100, kind: Kind::Rgba, depth: 8, adam7: true };
        let sums = |out: Out| match out {
            Out::Reduced { sums, counts, .. } => Some((sums.len(), counts.len())),
            Out::Whole { .. } => None,
        };
        assert_eq!(sums(Out::new(&laced, [10, 10, 120, 60], (120, 60)).unwrap()), Some((0, 0)));
        assert_eq!(sums(Out::new(&laced, [10, 10, 120, 60], (60, 30)).unwrap()), Some((1800, 1800)));
        assert!(sums(Out::new(&laced, [0, 0, 200, 100], (200, 100)).unwrap()).is_none());
    }

    #[test]
    fn a_part_is_cut_to_the_picture_and_one_outside_it_is_the_whole() {
        let (_, plain, _) = placed();
        let at = |part: [u32; 4]| seen(&plain, &Wish { part, ..wish((100, 100), 0) }).decoded;
        let cut = at([59, 43, 20, 20]);
        assert_eq!((cut.part, cut.size), ([59, 43, 5, 5], (5, 5)));
        assert_eq!(at([64, 0, 5, 5]).part, [0, 0, 64, 48], "begins at the edge");
        assert_eq!(at([0, 0, 0, 5]).part, [0, 0, 64, 48], "no width");
        assert_eq!(at([u32::MAX, 3, 5, 5]).part, [0, 0, 64, 48]);
        assert_eq!(at([3, 3, u32::MAX, u32::MAX]).part, [3, 3, 61, 45], "far more than there is");
    }

    #[test]
    fn what_a_file_says_of_its_colors_and_of_an_animation_is_noted() {
        let data = chunk(b"IDAT", &pack(&raw(&[vec![1, 2, 3]])));
        let with =
            |extra: Vec<u8>| seen(&file(&[head(1, 1, 8, 2, 0), extra, data.clone()]), &whole()).decoded.notes;
        assert_eq!(with(chunk(b"gAMA", &45_455u32.to_be_bytes())), Notes(0));
        assert_eq!(with(chunk(b"gAMA", &100_000u32.to_be_bytes())), Notes(Notes::UNAPPLIED));
        assert_eq!(with(chunk(b"iCCP", b"name\0\0packed")), Notes(Notes::UNAPPLIED));
        assert_eq!(with(chunk(b"acTL", &[0; 8])), Notes(Notes::ANIMATION));
        assert_eq!(with(chunk(b"sRGB", &[0])), Notes(0));
        // One whose sum is wrong says nothing.
        let mut wrong = chunk(b"iCCP", b"name\0\0packed");
        *wrong.last_mut().unwrap() ^= 1;
        assert_eq!(with(wrong), Notes(0));
    }

    #[test]
    fn what_follows_the_data_is_read_to_the_end_of_the_picture() {
        // Nothing behind the data was looked at, and a chunk there that the
        // file calls needed and that is not known was let through.
        let data = chunk(b"IDAT", &pack(&raw(&[vec![1, 2, 3]])));
        let with = |after: &[Vec<u8>]| open(&[&[head(1, 1, 8, 2, 0), data.clone()][..], after].concat());
        let end = chunk(b"IEND", &[]);
        assert_eq!(why(&with(&[chunk(b"ABCD", &[1]), end.clone()])), Some(Refused::Damaged));
        // One it does not call needed is read over, with nothing said.
        let sound =
            seen(&with(&[chunk(b"tEXt", b"a\0b"), chunk(b"abCD", &[1; 70_000]), end.clone()]), &whole());
        assert_eq!((sound.pixels, sound.decoded.notes), (vec![1, 2, 3, 255], Notes(0)));
        // Without its end the file ends early, wherever it stops.
        assert_eq!(seen(&with(&[]), &whole()).decoded.notes, Notes(Notes::EARLY));
        assert_eq!(seen(&with(&[chunk(b"tEXt", b"a\0b")]), &whole()).decoded.notes, Notes(Notes::EARLY));
        let cut = with(&[chunk(b"tEXt", b"a\0b"), end.clone()]);
        assert_eq!(seen(&cut[..cut.len() - 14], &whole()).decoded.notes, Notes(Notes::EARLY));
        // What follows the end is not read.
        assert_eq!(seen(&with(&[end, chunk(b"ABCD", &[1])]), &whole()).decoded.notes, Notes(0));
    }

    #[test]
    fn colors_that_are_not_those_of_a_screen_are_noted() {
        // Where the colors of a file lie was not looked at. Most programs
        // write the numbers of a screen there, and those say nothing:
        // others are said.
        let data = chunk(b"IDAT", &pack(&raw(&[vec![1, 2, 3]])));
        let named = |values: [u32; 8]| {
            let body: Vec<u8> = values.iter().flat_map(|value| value.to_be_bytes()).collect();
            seen(
                &file(&[head(1, 1, 8, 2, 0), chunk(b"cHRM", &body), data.clone(), chunk(b"IEND", &[])]),
                &whole(),
            )
            .decoded
            .notes
        };
        let screen = [31_270, 32_900, 64_000, 33_000, 30_000, 60_000, 15_000, 6_000];
        assert_eq!(named(screen), Notes(0));
        // As a program rounds them.
        assert_eq!(named([31_269, 32_899, 63_999, 33_001, 30_000, 60_000, 15_000, 5_999]), Notes(0));
        // A wider space: its green lies elsewhere.
        assert_eq!(
            named([31_270, 32_900, 68_000, 32_000, 26_500, 69_000, 15_000, 6_000]),
            Notes(Notes::UNAPPLIED)
        );
        let mut one = screen;
        one[7] = 7_000;
        assert_eq!(named(one), Notes(Notes::UNAPPLIED));
    }

    #[test]
    fn too_many_chunks_end_the_reading() {
        let mut chunks = vec![head(1, 1, 8, 2, 0)];
        chunks.extend((0..CHUNKS).map(|_| chunk(b"tEXt", &[])));
        chunks.push(chunk(b"IDAT", &pack(&raw(&[vec![1, 2, 3]]))));
        assert_eq!(why(&file(&chunks)), Some(Refused::Damaged));
    }

    #[test]
    fn a_picture_too_large_for_its_sums_is_made_smaller_still() {
        let adam7 = Shape { w: 20_000, h: 6_000, kind: Kind::Rgba, depth: 8, adam7: true };
        // 120 megapixels into a room of a whole large screen: with
        // Adam7 the sums of that would be over what the process may
        // use, and the result is halved until they are not.
        let room = Wish { side: 16_384, ..wish((6000, 3000), 0) };
        let whole = [0, 0, 20_000, 6_000];
        let (size, smaller) = plan(&adam7, whole, &room).unwrap();
        assert_eq!((fit((20_000, 6_000), &room), size, smaller), ((6000, 1800), (3000, 900), true));
        // From the top the sums are one row, and nothing is given up.
        assert_eq!(plan(&Shape { adam7: false, ..adam7 }, whole, &room).unwrap(), ((6000, 1800), false));
    }

    /// Numbers that look like chance and are the same every time.
    fn chance(state: &mut u64) -> usize {
        *state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (*state >> 33) as usize
    }

    #[test]
    fn a_file_changed_at_random_places_ends_in_an_answer() {
        let pixels = colors(9, 7);
        let rows: Vec<Vec<u8>> = pixels.chunks(9).map(|row| row.concat()).collect();
        let palette = chunk(b"PLTE", &[10, 20, 30, 40, 50, 60, 70, 80, 90, 1, 2, 3]);
        let sound = [
            plain(9, 7, 8, 6, &rows),
            file(&[
                head(9, 7, 8, 6, 1),
                chunk(b"IDAT", &pack(&raw(&laced(9, 7, &pixels)))),
                chunk(b"IEND", &[]),
            ]),
            file(&[
                head(8, 3, 2, 3, 0),
                palette,
                chunk(b"tRNS", &[1, 2]),
                chunk(b"IDAT", &pack(&raw(&vec![vec![0x1b, 0xe4]; 3]))),
            ]),
            plain(5, 2, 16, 2, &[vec![9; 30], vec![200; 30]]),
        ];
        let started = std::time::Instant::now();
        let mut state = 7;
        for round in 0..6000 {
            let mut bytes = sound[round % sound.len()].clone();
            for _ in 0..1 + chance(&mut state) % 4 {
                let at = chance(&mut state) % bytes.len();
                bytes[at] = chance(&mut state) as u8;
            }
            if chance(&mut state).is_multiple_of(5) {
                bytes.truncate(chance(&mut state) % bytes.len());
            }
            // The sums are made right again half of the time: or the
            // reading would mostly end at the first of them.
            if round % 2 == 0 {
                let mut at = 8;
                while at + 12 <= bytes.len() {
                    let length = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
                    let Some(end) = at.checked_add(8 + length).filter(|end| end + 4 <= bytes.len()) else {
                        break;
                    };
                    let named = !sum(u32::MAX, &bytes[at + 4..end]);
                    bytes[end..end + 4].copy_from_slice(&named.to_be_bytes());
                    at = end + 4;
                }
            }
            for wish in [whole(), wish((16, 16), 0)] {
                if let Ok(picture) = decode(&bytes[..], &wish) {
                    let (w, h) = picture.decoded.size;
                    assert_eq!(picture.pixels.len(), w as usize * h as usize * 4, "round {round}");
                    assert!(wish.takes(picture.decoded.size), "round {round}");
                }
            }
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(60), "it took {:?}", started.elapsed());
    }
}
