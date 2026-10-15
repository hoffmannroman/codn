//! Drawing primitives: one GPU pipeline, one glyph atlas.

pub mod boxdraw;
mod canvas;
pub mod font;
mod fontconfig;
mod gl;
pub mod icon;

pub use canvas::Canvas;
pub use gl::Renderer;
pub use icon::Icon;

pub type Color = [u8; 4];

pub const fn rgb(v: u32) -> Color {
    [(v >> 16) as u8, (v >> 8) as u8, v as u8, 255]
}

/// A color as the eye takes it (OKLab): how light it is, from 0 to 1,
/// and how far it lies from gray, towards red and towards yellow. The
/// colors of codn are reckoned and mixed here, not in what the screen is
/// given.
pub fn oklab(color: Color) -> [f64; 3] {
    let part = |value: u8| {
        let value = value as f64 / 255.0;
        if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
    };
    let (r, g, b) = (part(color[0]), part(color[1]), part(color[2]));
    let long = (0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b).cbrt();
    let middle = (0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b).cbrt();
    let short = (0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b).cbrt();
    [
        0.2104542553 * long + 0.7936177850 * middle - 0.0040720468 * short,
        1.9779984951 * long - 2.4285922050 * middle + 0.4505937099 * short,
        0.0259040371 * long + 0.7827717662 * middle - 0.8086757660 * short,
    ]
}

/// The color of the screen that is nearest to one said as `oklab` says it.
pub fn from_oklab([light, a, b]: [f64; 3]) -> Color {
    let long = (light + 0.3963377774 * a + 0.2158037573 * b).powi(3);
    let middle = (light - 0.1055613458 * a - 0.0638541728 * b).powi(3);
    let short = (light - 0.0894841775 * a - 1.2914855480 * b).powi(3);
    let part = |value: f64| {
        let value = value.clamp(0.0, 1.0);
        let bent = if value <= 0.0031308 { 12.92 * value } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 };
        (bent * 255.0).round() as u8
    };
    [
        part(4.0767416621 * long - 3.3077115913 * middle + 0.2309699292 * short),
        part(-1.2684380046 * long + 2.6097574011 * middle - 0.3413193965 * short),
        part(-0.0041960863 * long - 0.7034186147 * middle + 1.7076147010 * short),
        255,
    ]
}

/// A blend from `a` to `b` as the eye takes it, `t` in 0..=255: that
/// much of the way from the one to the other in lightness and in color.
/// Two colors that are equally light and equally colorful are so still
/// when both are blended with a third.
pub fn mix(a: Color, b: Color, t: u8) -> Color {
    let (from, to, part) = (oklab(a), oklab(b), t as f64 / 255.0);
    from_oklab([0, 1, 2].map(|at| from[at] + (to[at] - from[at]) * part))
}

/// A buffer that is kept between frames keeps no more than this without
/// being asked whether it is used.
const KEPT: usize = 1 << 14;

/// Gives back most of a buffer that is kept between frames and holds
/// several times what the frame used. It is kept so that a frame asks
/// for no memory, and one frame of a very large window would be paid
/// for as long as the app runs: a gigabyte for a window that another
/// program made as large as the window system allows.
pub fn let_go<T>(buffer: &mut Vec<T>) {
    if buffer.capacity() > KEPT && buffer.capacity() / 4 > buffer.len() {
        buffer.shrink_to((buffer.len() * 2).max(KEPT));
    }
}

/// A rectangle in physical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_gives_back_what_one_large_frame_made_it_hold() {
        // A frame of a very large window, and then frames as they are.
        let mut buffer: Vec<u64> = Vec::with_capacity(4_000_000);
        buffer.extend(0..3000);
        let_go(&mut buffer);
        assert!(buffer.capacity() <= 2 * KEPT, "{} kept", buffer.capacity());
        assert_eq!(buffer.len(), 3000);
        // What is used stays, and a small buffer is left alone: a frame
        // asks for no memory once its buffers have grown.
        let mut used: Vec<u64> = Vec::with_capacity(4_000_000);
        used.extend(0..1_500_000);
        let_go(&mut used);
        assert!(used.capacity() >= 4_000_000);
        let mut small: Vec<u64> = Vec::with_capacity(KEPT);
        let_go(&mut small);
        assert!(small.capacity() >= KEPT);
    }
}
