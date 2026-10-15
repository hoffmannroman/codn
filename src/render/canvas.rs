//! Collects the quads of one frame.

use super::gl::{
    Glyph, Instance, KIND_ARC, KIND_COLOR, KIND_CROSS, KIND_MASK, KIND_PICTURE, KIND_RECT, KIND_RING,
    Renderer,
};
use super::{Color, Icon, Rect};

pub struct Canvas<'a> {
    pub(super) r: &'a mut Renderer,
    pub(super) quads: &'a mut Vec<Instance>,
    /// Quads held back until `flush`, so they end up above what follows.
    pub(super) held: &'a mut Vec<Instance>,
    /// While set, new quads are held back.
    pub hold: bool,
}

impl Canvas<'_> {
    fn push(&mut self, r: Rect, uv: [f32; 4], params: [f32; 4], color: Color) {
        let quad = Instance { rect: [r.x, r.y, r.w, r.h], uv, params, color };
        if self.hold { self.held.push(quad) } else { self.quads.push(quad) }
    }

    pub fn flush(&mut self) {
        self.quads.append(self.held);
        self.hold = false;
    }

    pub fn rect(&mut self, r: Rect, color: Color) {
        self.push(r, [0.0; 4], [0.0, 0.0, KIND_RECT, 0.0], color);
    }

    pub fn rounded(&mut self, r: Rect, radius: f32, color: Color) {
        self.push(r, [0.0; 4], [radius, 0.0, KIND_RECT, 0.0], color);
    }

    pub fn outline(&mut self, r: Rect, radius: f32, width: f32, color: Color) {
        self.push(r, [0.0; 4], [radius, width, KIND_RECT, 0.0], color);
    }

    pub fn cross(&mut self, r: Rect, width: f32, color: Color) {
        self.push(r, [0.0; 4], [0.0, width, KIND_CROSS, 0.0], color);
    }

    /// A quarter circle joining two box-drawing lines. `corner` counts from
    /// "right and down" over "left and down", "left and up" to "right and up",
    /// `center` is where the lines cross, relative to the middle of `r`.
    pub fn arc(&mut self, r: Rect, radius: f32, width: f32, corner: u8, center: (f32, f32), color: Color) {
        self.push(
            r,
            [center.0, center.1, center.0, center.1],
            [radius, width, KIND_ARC, corner as f32],
            color,
        );
    }

    /// A `part` of the circle in `r`, as a line of `width`: from the top,
    /// clockwise. Nothing of nothing, and no more than the whole.
    pub fn ring(&mut self, r: Rect, width: f32, part: f32, color: Color) {
        if part > 0.0 {
            self.push(r, [0.0; 4], [0.0, width, KIND_RING, part.min(1.0)], color);
        }
    }

    /// Draws the part `uv` of the picture of the decoding `id`, as
    /// where it begins and ends in the texture from 0 to 1, into `r`,
    /// if that is the one that is loaded, and says whether it did.
    /// `ground` is what lies
    /// under it where it is transparent: a color, the width of a cell of
    /// its chequer, and by how much every other cell is lighter. Without
    /// one the picture lies on what was drawn before it.
    pub fn picture(&mut self, id: u64, r: Rect, uv: [f32; 4], ground: Option<(Color, f32, f32)>) -> bool {
        if self.r.picture() != Some(id) {
            return false;
        }
        let (color, cell, lighter) = ground.unwrap_or(([0, 0, 0, 255], 0.0, 0.0));
        self.push(r, uv, [cell, 0.0, KIND_PICTURE, lighter], color);
        true
    }

    /// Draws one character with its pen at `x` on `baseline`, returns its advance.
    pub fn glyph(&mut self, c: char, style: u8, px: f32, x: f32, baseline: f32, color: Color) -> f32 {
        let Some(g) = self.r.glyph(c, style, px) else { return 0.0 };
        self.place(&g, x, baseline, color);
        g.advance
    }

    /// Draws an icon as a square of `size` pixels with its top left at
    /// `x` and `y`.
    pub fn icon(&mut self, icon: Icon, x: f32, y: f32, size: f32, color: Color) {
        self.icon_at(icon, 0, x, y, size, color);
    }

    /// Draws an icon like `icon`: one that moves, at step `step` of the
    /// `icon::STEPS` it moves in.
    pub fn icon_at(&mut self, icon: Icon, step: u8, x: f32, y: f32, size: f32, color: Color) {
        if let Some(g) = self.r.icon(icon, size, step) {
            self.place(&g, x, y, color);
        }
    }

    fn place(&mut self, g: &Glyph, x: f32, baseline: f32, color: Color) {
        if g.w > 0.0 {
            let rect = Rect::new((x + g.left).round(), (baseline - g.top).round(), g.w, g.h);
            let kind = if g.color { KIND_COLOR } else { KIND_MASK };
            self.push(rect, g.uv, [0.0, 0.0, kind, 0.0], color);
        }
    }

    /// The quads drawn so far, on the layer that is being drawn to.
    fn layer(&mut self) -> &mut Vec<Instance> {
        if self.hold { self.held } else { self.quads }
    }

    /// Where the pen goes for a combining mark on a character that was
    /// drawn at `x` and is `advance` wide.
    pub fn mark_pen(&mut self, c: char, style: u8, px: f32, x: f32, advance: f32) -> f32 {
        let Some(g) = self.r.glyph(c, style, px) else { return x };
        x + mark_offset(g.left, g.w, advance)
    }

    pub fn text_width(&mut self, text: &str, style: u8, px: f32) -> f32 {
        text.chars().map(|c| self.r.glyph(c, style, px).map_or(0.0, |g| g.advance)).sum()
    }

    /// Draws a line of text, cut off with an ellipsis at `max_w`. Returns the width used.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        text: &str,
        style: u8,
        px: f32,
        x: f32,
        baseline: f32,
        color: Color,
        max_w: f32,
    ) -> f32 {
        // Each character is looked up once and drawn right away. Where an
        // ellipsis would still fit is remembered, and a text that turns out
        // too long is taken back to there. Nothing behind is looked at.
        let dots = self.r.glyph('…', style, px).map_or(0.0, |g| g.advance);
        let mut pen = 0.0;
        let mut cut = (0.0, self.layer().len());
        for c in text.chars() {
            let Some(g) = self.r.glyph(c, style, px) else { continue };
            if pen + g.advance > max_w {
                self.layer().truncate(cut.1);
                if dots > max_w {
                    return 0.0;
                }
                return cut.0 + self.glyph('…', style, px, x + cut.0, baseline, color);
            }
            self.place(&g, x + pen, baseline, color);
            pen += g.advance;
            if pen + dots <= max_w {
                cut = (pen, self.layer().len());
            }
        }
        pen
    }
}

/// How far behind the start of a character the pen goes for a combining
/// mark whose ink begins `left` of the pen and is `width` wide: so far
/// that the ink is in the middle of the character.
///
/// Fonts draw their marks left of the pen, to follow the character, or
/// around the pen, to be set on an anchor of the character, or in a cell
/// of their own. None of them says which, one font does it one way for
/// the acute and another way for the diaeresis, and the anchors are not
/// read here. The ink itself is what all of them agree on.
pub(crate) fn mark_offset(left: f32, width: f32, advance: f32) -> f32 {
    ((advance - width) / 2.0 - left).round()
}

#[cfg(test)]
mod tests {
    use super::mark_offset;

    #[test]
    fn a_combining_mark_stands_in_the_middle_of_its_character() {
        // The ink of marks as five fonts have it at 14 pixels, where a
        // character is 8.4 wide: from the pixel left of the pen it begins
        // at to the one it ends before.
        let fonts = [
            // Noto Sans Mono, around the pen: acute, diaeresis, grave.
            [(-1, 3), (-3, 3), (-2, 1)],
            // Hack, left of the pen for two and around it for the third.
            [(-6, -1), (-3, 3), (-7, -3)],
            // Liberation Mono, the same mixture with other numbers.
            [(-3, 0), (-2, 2), (-4, -1)],
            // Fira Code, in a cell of its own.
            [(2, 7), (2, 7), (2, 6)],
            // JetBrains Mono as a Nerd Font, left of the pen.
            [(-6, -2), (-7, -1), (-7, -3)],
        ];
        for (left, right) in fonts.into_iter().flatten() {
            let pen = mark_offset(left as f32, (right - left) as f32, 8.4);
            let middle = pen + (left + right) as f32 / 2.0;
            assert!((middle - 4.2).abs() <= 0.5, "ink {left}..{right} ends up around {middle}");
        }
    }
}
