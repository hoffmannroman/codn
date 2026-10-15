//! Box-drawing and block characters, drawn as geometry.
//!
//! Font glyphs for these rarely meet at the cell edges, which leaves gaps in
//! frames and tables. Lines built from rectangles always join.

use super::{Canvas, Color, Rect};

/// Arms of U+2500..U+254F as two bits each for up, right, down, left:
/// 0 none, 1 light, 2 heavy. Dashed lines are drawn solid.
const LINES: [u8; 80] = [
    0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88, 0x11, 0x22, 0x44, 0x88, 0x14, 0x24, 0x18, 0x28, 0x05,
    0x06, 0x09, 0x0a, 0x50, 0x60, 0x90, 0xa0, 0x41, 0x42, 0x81, 0x82, 0x54, 0x64, 0x94, 0x58, 0x98, 0xa4,
    0x68, 0xa8, 0x45, 0x46, 0x85, 0x49, 0x89, 0x86, 0x4a, 0x8a, 0x15, 0x16, 0x25, 0x26, 0x19, 0x1a, 0x29,
    0x2a, 0x51, 0x52, 0x61, 0x62, 0x91, 0x92, 0xa1, 0xa2, 0x55, 0x56, 0x65, 0x66, 0x95, 0x59, 0x99, 0x96,
    0xa5, 0x5a, 0x69, 0xa6, 0x6a, 0x9a, 0xa9, 0xaa, 0x11, 0x22, 0x44, 0x88,
];
/// The same for the half lines U+2574..U+257F.
const HALVES: [u8; 12] = [0x01, 0x40, 0x10, 0x04, 0x02, 0x80, 0x20, 0x08, 0x21, 0x48, 0x12, 0x84];

/// Draws `c` into `cell` if it is a character this module knows.
pub fn draw(cv: &mut Canvas, c: char, cell: Rect, light: f32, color: Color) -> bool {
    let code = c as u32;
    let arms = match code {
        0x2500..=0x254f => LINES[(code - 0x2500) as usize],
        0x2574..=0x257f => HALVES[(code - 0x2574) as usize],
        0x256d..=0x2570 => {
            let (cx, cy) = (center(cell.x, cell.w, light), center(cell.y, cell.h, light));
            let offset = (cx + light / 2.0 - cell.x - cell.w / 2.0, cy + light / 2.0 - cell.y - cell.h / 2.0);
            let radius = (cell.w.min(cell.h) / 2.0).floor();
            cv.arc(cell, radius, light, (code - 0x256d) as u8, offset, color);
            return true;
        }
        0x2580..=0x259f => return block(cv, code, cell, color),
        _ => return false,
    };
    let [up, right, down, left] =
        [arms >> 6, arms >> 4 & 3, arms >> 2 & 3, arms & 3].map(|w| w as f32 * light);
    // The arms overlap in a center box as thick as the thickest crossing arm,
    // so corners come out square.
    let (vw, hw) = (up.max(down).max(light), left.max(right).max(light));
    let (vx, hy) = (center(cell.x, cell.w, vw), center(cell.y, cell.h, hw));
    if up > 0.0 {
        cv.rect(Rect::new(center(cell.x, cell.w, up), cell.y, up, hy + hw - cell.y), color);
    }
    if down > 0.0 {
        cv.rect(Rect::new(center(cell.x, cell.w, down), hy, down, cell.bottom() - hy), color);
    }
    if left > 0.0 {
        cv.rect(Rect::new(cell.x, center(cell.y, cell.h, left), vx + vw - cell.x, left), color);
    }
    if right > 0.0 {
        cv.rect(Rect::new(vx, center(cell.y, cell.h, right), cell.right() - vx, right), color);
    }
    true
}

/// Start of a line of `width` centered in a cell, on the pixel grid.
fn center(start: f32, size: f32, width: f32) -> f32 {
    start + ((size - width) / 2.0).floor()
}

fn block(cv: &mut Canvas, code: u32, cell: Rect, color: Color) -> bool {
    let Rect { x, y, w, h } = cell;
    let eighth = |n: u32, size: f32| (size * n as f32 / 8.0).round();
    let (hw, hh) = ((w / 2.0).round(), (h / 2.0).round());
    let quadrant = |cv: &mut Canvas, bits: u8| {
        // Bits: upper left, upper right, lower left, lower right.
        for (bit, qx, qy, qw, qh) in [
            (8, x, y, hw, hh),
            (4, x + hw, y, w - hw, hh),
            (2, x, y + hh, hw, h - hh),
            (1, x + hw, y + hh, w - hw, h - hh),
        ] {
            if bits & bit != 0 {
                cv.rect(Rect::new(qx, qy, qw, qh), color);
            }
        }
    };
    match code {
        0x2580 => cv.rect(Rect::new(x, y, w, hh), color),
        // The lower half is what the upper half leaves, so the two tile.
        0x2584 => cv.rect(Rect::new(x, y + hh, w, h - hh), color),
        0x2581..=0x2588 => {
            let part = eighth(code - 0x2580, h);
            cv.rect(Rect::new(x, y + h - part, w, part), color);
        }
        0x2589..=0x258f => cv.rect(Rect::new(x, y, eighth(0x2590 - code, w), h), color),
        0x2590 => cv.rect(Rect::new(x + hw, y, w - hw, h), color),
        0x2591..=0x2593 => {
            let shade = [64, 128, 191][(code - 0x2591) as usize];
            cv.rect(cell, [color[0], color[1], color[2], shade]);
        }
        0x2594 => cv.rect(Rect::new(x, y, w, eighth(1, h)), color),
        0x2595 => cv.rect(Rect::new(x + w - eighth(1, w), y, eighth(1, w), h), color),
        _ => quadrant(cv, [2, 1, 8, 11, 9, 14, 13, 4, 6, 7][(code - 0x2596) as usize]),
    }
    true
}
