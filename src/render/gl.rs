//! The GPU side: one shader, one instanced draw call per frame, two atlases,
//! and beside them the texture of the one picture a pane shows.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

use glow::HasContext;

use super::font::{Fonts, Raster};
use super::{Canvas, Color, Icon};

pub const KIND_RECT: f32 = 0.0;
pub const KIND_MASK: f32 = 1.0;
pub const KIND_COLOR: f32 = 2.0;
pub const KIND_CROSS: f32 = 3.0;
pub const KIND_ARC: f32 = 4.0;
pub const KIND_RING: f32 = 5.0;
pub const KIND_PICTURE: f32 = 6.0;

/// Largest side of a glyph atlas. Every OpenGL 3.3 and ES 3.0 driver
/// supports at least 2048, common ones far more.
pub(super) const MAX_ATLAS: i32 = 4096;
/// Most glyphs remembered, found or not. A session that prints all of
/// Unicode would otherwise leave an entry behind for every character.
const MAX_GLYPHS: usize = 1 << 16;

/// One quad. `params` is (corner radius, border width, kind, arc corner or
/// part of a ring).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Instance {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub params: [f32; 4],
    pub color: Color,
}

#[derive(Clone, Copy, Default)]
pub struct Glyph {
    pub uv: [f32; 4],
    pub left: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
    pub advance: f32,
    pub color: bool,
}

const VERTEX: &str = r#"
layout(location = 0) in vec4 a_rect;
layout(location = 1) in vec4 a_uv;
layout(location = 2) in vec4 a_params;
layout(location = 3) in vec4 a_color;
uniform vec2 u_view;
out vec2 v_p;
out vec2 v_half;
out vec2 v_uv;
out vec4 v_color;
out vec4 v_params;
void main() {
    vec2 c = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
    vec2 pos = a_rect.xy + c * a_rect.zw;
    v_half = a_rect.zw * 0.5;
    v_p = (c - 0.5) * a_rect.zw;
    v_uv = mix(a_uv.xy, a_uv.zw, c);
    v_color = vec4(a_color.rgb * a_color.a, a_color.a);
    v_params = a_params;
    gl_Position = vec4(pos.x / u_view.x * 2.0 - 1.0, 1.0 - pos.y / u_view.y * 2.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"
in vec2 v_p;
in vec2 v_half;
in vec2 v_uv;
in vec4 v_color;
in vec4 v_params;
uniform sampler2D u_mask;
uniform sampler2D u_color;
uniform sampler2D u_picture;
out vec4 o;
void main() {
    float kind = v_params.z;
    if (kind < 0.5) {
        // Rounded rectangle, filled or (border > 0) outlined.
        float r = min(v_params.x, min(v_half.x, v_half.y));
        vec2 q = abs(v_p) - v_half + r;
        float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - r;
        float a = clamp(0.5 - d, 0.0, 1.0);
        if (v_params.y > 0.0) a *= clamp(0.5 + d + v_params.y, 0.0, 1.0);
        o = v_color * a;
    } else if (kind < 1.5) {
        o = v_color * texture(u_mask, v_uv).r;
    } else if (kind < 2.5) {
        o = texture(u_color, v_uv) * v_color.a;
    } else if (kind < 3.5) {
        // Diagonal cross, used for close buttons.
        float d = min(abs(v_p.x - v_p.y), abs(v_p.x + v_p.y)) * 0.7071 - v_params.y * 0.5;
        float b = max(abs(v_p.x), abs(v_p.y)) - v_half.x;
        o = v_color * clamp(0.5 - d, 0.0, 1.0) * clamp(0.5 - b, 0.0, 1.0);
    } else if (kind < 4.5) {
        // Rounded box-drawing corner: a quarter circle with straight ends.
        // Mirrored so that the two arms always point to +x and +y.
        float corner = v_params.w;
        vec2 s = vec2(corner < 0.5 || corner > 2.5 ? 1.0 : -1.0, corner < 1.5 ? 1.0 : -1.0);
        vec2 p = (v_p - v_uv) * s;
        float r = v_params.x;
        float d = p.x > r ? abs(p.y) : (p.y > r ? abs(p.x) : abs(length(p - vec2(r)) - r));
        o = v_color * clamp(v_params.y * 0.5 + 0.5 - d, 0.0, 1.0);
    } else if (kind < 5.5) {
        // Part of a ring, clockwise from the top: v_params.w of the whole.
        float r = min(v_half.x, v_half.y);
        float d = length(v_p) - r;
        float a = clamp(0.5 - d, 0.0, 1.0) * clamp(0.5 + d + v_params.y, 0.0, 1.0);
        float turn = atan(v_p.x, -v_p.y) / 6.2831853;
        if (turn < 0.0) turn += 1.0;
        // The end is soft over a pixel of the way around.
        if (v_params.w < 1.0) a *= clamp(0.5 + (v_params.w - turn) * 6.2831853 * r, 0.0, 1.0);
        o = v_color * a;
    } else {
        // A picture, its colors multiplied by their alpha. With cells of
        // v_params.x it lies on a ground that shows what of it is
        // transparent: a chequer in the color given, every other cell
        // lighter by v_params.w.
        vec4 picture = texture(u_picture, v_uv);
        if (v_params.x > 0.0) {
            vec2 cell = floor((v_p + v_half) / v_params.x);
            vec3 ground = v_color.rgb + mod(cell.x + cell.y, 2.0) * v_params.w;
            o = vec4(picture.rgb + ground * (1.0 - picture.a), 1.0);
        } else {
            o = picture;
        }
    }
}
"#;

/// Shelf-packed texture atlas. Storage is allocated on first use.
struct Atlas {
    tex: glow::Texture,
    channels: i32,
    size: i32,
    x: i32,
    y: i32,
    row: i32,
}

impl Atlas {
    /// Starts over. What was placed is not valid any more.
    fn clear(&mut self) {
        (self.x, self.y, self.row) = (0, 0, 0);
    }

    fn alloc(&mut self, w: i32, h: i32) -> Option<(i32, i32)> {
        if self.x + w + 1 > self.size {
            self.x = 0;
            self.y += self.row + 1;
            self.row = 0;
        }
        if self.size == 0 || w + 1 > self.size || self.y + h + 1 > self.size {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w + 1;
        self.row = self.row.max(h);
        Some(at)
    }
}

#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(self.0 ^ b as u64);
        }
    }

    fn write_u64(&mut self, v: u64) {
        let h = v.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        self.0 = h ^ (h >> 32);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub struct Renderer {
    gl: glow::Context,
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    u_view: Option<glow::UniformLocation>,
    mask: Atlas,
    color: Atlas,
    max_atlas: i32,
    /// The texture of the picture a pane shows, and the number of the
    /// decoding it holds. It is of its own, outside the atlases.
    picture: Option<(glow::Texture, u64)>,
    /// The longest side the driver takes for a texture: what it says,
    /// not cut down as for the atlases.
    pub max_side: u32,
    glyphs: HashMap<u64, Option<Glyph>, BuildHasherDefault<KeyHasher>>,
    /// How wide the signs are that were measured and have no image, and
    /// the fallback fonts they were measured with.
    advances: HashMap<u64, f32, BuildHasherDefault<KeyHasher>>,
    measured: u32,
    pub fonts: Fonts,
    /// An atlas was reset while building the frame, so earlier quads are invalid.
    stale: bool,
    /// The frame is on its last attempt and must come out whole: nothing
    /// that was handed out may be taken back now.
    settle: bool,
    /// The fallback fonts this cache has seen, see `Fonts::generation`.
    fallbacks: u32,
    /// The quads of the current frame, kept to reuse their memory.
    quads: Vec<Instance>,
    held: Vec<Instance>,
}

impl Renderer {
    pub fn new(gl: glow::Context, gles: bool, fonts: Fonts) -> Result<Self, String> {
        let header = if gles { "#version 300 es\nprecision highp float;\n" } else { "#version 330 core\n" };
        unsafe {
            let program = gl.create_program()?;
            for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
                let shader = gl.create_shader(kind)?;
                gl.shader_source(shader, &format!("{header}{source}"));
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    return Err(gl.get_shader_info_log(shader));
                }
                gl.attach_shader(program, shader);
                gl.delete_shader(shader);
            }
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                return Err(gl.get_program_info_log(program));
            }
            gl.use_program(Some(program));
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_mask").as_ref(), 0);
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_color").as_ref(), 1);
            gl.uniform_1_i32(gl.get_uniform_location(program, "u_picture").as_ref(), 2);
            let u_view = gl.get_uniform_location(program, "u_view");

            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = size_of::<Instance>() as i32;
            for (location, offset) in [(0, 0), (1, 16), (2, 32)] {
                gl.enable_vertex_attrib_array(location);
                gl.vertex_attrib_pointer_f32(location, 4, glow::FLOAT, false, stride, offset);
                gl.vertex_attrib_divisor(location, 1);
            }
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 4, glow::UNSIGNED_BYTE, true, stride, 48);
            gl.vertex_attrib_divisor(3, 1);

            gl.enable(glow::BLEND);
            gl.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);

            let atlas = |channels| -> Result<Atlas, String> {
                let tex = gl.create_texture()?;
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                }
                Ok(Atlas { tex, channels, size: 0, x: 0, y: 0, row: 0 })
            };
            let (mask, color) = (atlas(1)?, atlas(4)?);

            let max_side = gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE).max(1);
            let max_atlas = max_side.min(MAX_ATLAS);
            let mut renderer = Self {
                gl,
                program,
                vao,
                vbo,
                u_view,
                mask,
                color,
                max_atlas,
                picture: None,
                max_side: max_side as u32,
                glyphs: HashMap::default(),
                advances: HashMap::default(),
                measured: 0,
                fonts,
                stale: false,
                settle: false,
                fallbacks: 0,
                quads: Vec::new(),
                held: Vec::new(),
            };
            // Allocated up front, so the first frame is not built twice.
            renderer.allocate(false, 1024.min(max_atlas));
            Ok(renderer)
        }
    }

    /// Fallback fonts became available: characters that had no glyph are
    /// looked up again. Returns whether anything may change on screen.
    pub fn fonts_ready(&mut self) -> bool {
        // A lookup in the middle of a frame may have taken the list over
        // already, so it is not this call's own result that counts.
        self.fonts.poll();
        let ready = self.fonts.generation != self.fallbacks;
        if ready {
            self.fallbacks = self.fonts.generation;
            self.glyphs.retain(|_, glyph| glyph.is_some());
        }
        ready
    }

    /// Drops all cached glyphs, for example after a scale or font change.
    pub fn reset_glyphs(&mut self) {
        self.glyphs.clear();
        self.advances.clear();
        for atlas in [&mut self.mask, &mut self.color] {
            atlas.clear();
        }
        self.stale = true;
    }

    /// How wide a character is, for whoever sets text in rows and draws
    /// little of it. No image of it is made: a text can have more signs
    /// that differ than images are kept, and to measure such a text
    /// through `glyph` made every image anew each time, more than a
    /// second for a megabyte of them. The widths are kept, as many as
    /// images are, and all of them go when there are more or when other
    /// fonts stand in for missing signs.
    pub fn advance(&mut self, c: char, style: u8, px: f32) -> f32 {
        let key = Self::key(c, style, px);
        if let Some(glyph) = self.glyphs.get(&key) {
            return glyph.map_or(0.0, |glyph| glyph.advance);
        }
        if self.measured != self.fonts.generation || self.advances.len() >= MAX_GLYPHS {
            self.advances.clear();
            self.measured = self.fonts.generation;
        }
        if let Some(advance) = self.advances.get(&key) {
            return *advance;
        }
        let advance = self.fonts.advance(c, style, px).unwrap_or(0.0);
        // Asking may have taken the list of fallback fonts over: what
        // was measured before it is of the fonts before.
        if self.measured == self.fonts.generation {
            self.advances.insert(key, advance);
        }
        advance
    }

    fn key(c: char, style: u8, px: f32) -> u64 {
        c as u64 | (style as u64) << 24 | ((px * 4.0) as u64) << 32
    }

    pub fn glyph(&mut self, c: char, style: u8, px: f32) -> Option<Glyph> {
        let key = Self::key(c, style, px);
        if let Some(glyph) = self.glyphs.get(&key) {
            return *glyph;
        }
        // Reset before allocating this glyph. Resetting after upload would
        // cache UVs into storage whose next allocation starts over at (0, 0).
        if self.glyphs.len() >= MAX_GLYPHS && !self.settle {
            self.reset_glyphs();
        }
        let glyph = match self.fonts.rasterize(c, style, px) {
            None => None,
            Some((raster, advance)) => {
                let mut glyph = Glyph { advance, ..Glyph::default() };
                // An image that no atlas takes is left out for good, and
                // remembered so: it would be made again for every frame.
                let fits = |raster: &Raster| (raster.w.max(raster.h) as i32) < self.max_atlas;
                if let Some(raster) = raster.filter(fits) {
                    // No room for the image right now. It is left out of
                    // this frame and not remembered, so the next one tries
                    // again instead of keeping a blank.
                    let Some(uv) = self.upload(&raster) else { return Some(glyph) };
                    glyph.uv = uv;
                    glyph.left = raster.left as f32;
                    glyph.top = raster.top as f32;
                    glyph.w = raster.w as f32;
                    glyph.h = raster.h as f32;
                    glyph.color = raster.color;
                }
                Some(glyph)
            }
        };
        self.glyphs.insert(key, glyph);
        glyph
    }

    /// An icon as a square of `size` pixels, at step `step` if it is one
    /// that moves. It is kept among the glyphs, under a key no character
    /// has.
    pub fn icon(&mut self, icon: Icon, size: f32, step: u8) -> Option<Glyph> {
        let key = 1 << 22 | icon as u64 | (step as u64) << 24 | (size as u64) << 32;
        if let Some(glyph) = self.glyphs.get(&key) {
            return *glyph;
        }
        let raster = icon.rasterize(size as u32, step)?;
        // No room right now: left out of this frame and tried again.
        let uv = self.upload(&raster)?;
        let glyph = Glyph { uv, w: size, h: size, ..Glyph::default() };
        self.glyphs.insert(key, Some(glyph));
        Some(glyph)
    }

    /// Gives an atlas its texture storage, at `size` pixels square.
    fn allocate(&mut self, color: bool, size: i32) {
        let atlas = self.atlas(color);
        atlas.size = size;
        let (tex, format) =
            (atlas.tex, if atlas.channels == 1 { (glow::R8, glow::RED) } else { (glow::RGBA8, glow::RGBA) });
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            self.gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                format.0 as i32,
                size,
                size,
                0,
                format.1,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );
        }
    }

    fn upload(&mut self, raster: &Raster) -> Option<[f32; 4]> {
        let (w, h) = (raster.w as i32, raster.h as i32);
        let channels = if raster.color { 4 } else { 1 };
        if w >= self.max_atlas || h >= self.max_atlas || raster.data.len() != (w * h * channels) as usize {
            return None;
        }
        let max_atlas = self.max_atlas;
        let at = match self.atlas(raster.color).alloc(w, h) {
            Some(at) => at,
            // Starting over takes back what this frame was already given.
            // On its last attempt the frame keeps that and goes without
            // this glyph.
            None if self.settle => return None,
            None => {
                // Full, or not yet allocated: start over, larger if there is
                // room to grow. Every glyph is rasterized again on demand,
                // so an atlas at its largest size is simply refilled.
                let atlas = self.atlas(raster.color);
                let size = match atlas.size {
                    0 => 512.min(max_atlas),
                    size => (size * 2).min(max_atlas),
                };
                // The other atlas and its glyphs are left alone.
                atlas.clear();
                self.glyphs.retain(|_, glyph| glyph.is_none_or(|g| g.w == 0.0 || g.color != raster.color));
                self.stale = true;
                self.allocate(raster.color, size);
                self.atlas(raster.color).alloc(w, h)?
            }
        };
        let atlas = self.atlas(raster.color);
        let (tex, size, format) =
            (atlas.tex, atlas.size as f32, if atlas.channels == 1 { glow::RED } else { glow::RGBA });
        unsafe {
            self.gl.bind_texture(glow::TEXTURE_2D, Some(tex));
            self.gl.tex_sub_image_2d(
                glow::TEXTURE_2D,
                0,
                at.0,
                at.1,
                w,
                h,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&raster.data)),
            );
        }
        let (x, y) = (at.0 as f32, at.1 as f32);
        Some([x / size, y / size, (x + w as f32) / size, (y + h as f32) / size])
    }

    fn atlas(&mut self, color: bool) -> &mut Atlas {
        if color { &mut self.color } else { &mut self.mask }
    }

    /// The number of the decoding whose pixels are loaded as the picture.
    pub fn picture(&self) -> Option<u64> {
        self.picture.map(|(_, id)| id)
    }

    /// Loads the pixels of a picture, four bytes each and multiplied by
    /// their alpha, in the place of the picture that was loaded. Says
    /// whether they were taken: where not, the picture before stays.
    /// Never called while a frame is built.
    pub fn load_picture(&mut self, id: u64, size: (u32, u32), pixels: &[u8]) -> bool {
        let fits = size.0 > 0 && size.1 > 0 && size.0 <= self.max_side && size.1 <= self.max_side;
        if !fits || pixels.len() as u64 != size.0 as u64 * size.1 as u64 * 4 {
            return false;
        }
        unsafe {
            let gl = &self.gl;
            let Ok(texture) = gl.create_texture() else { return false };
            // What went wrong before is not this load's.
            while gl.get_error() != glow::NO_ERROR {}
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                size.0 as i32,
                size.1 as i32,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(pixels)),
            );
            // The driver may have no memory for it.
            if gl.get_error() != glow::NO_ERROR {
                gl.delete_texture(texture);
                return false;
            }
            if let Some((before, _)) = self.picture.replace((texture, id)) {
                gl.delete_texture(before);
            }
        }
        true
    }

    /// Gives up the picture that is loaded.
    pub fn drop_picture(&mut self) {
        if let Some((texture, _)) = self.picture.take() {
            unsafe { self.gl.delete_texture(texture) };
        }
    }

    /// Builds and presents one frame. `build` draws onto the canvas, and is
    /// asked again if a glyph atlas had to start over halfway through.
    /// Returns the number of quads drawn.
    pub fn frame(
        &mut self,
        width: u32,
        height: u32,
        clear: Color,
        mut build: impl FnMut(&mut Canvas),
    ) -> usize {
        let (mut quads, mut held) = (std::mem::take(&mut self.quads), std::mem::take(&mut self.held));
        for attempt in 0..3 {
            quads.clear();
            held.clear();
            self.stale = false;
            self.settle = attempt == 2;
            build(&mut Canvas { r: self, quads: &mut quads, held: &mut held, hold: false });
            if !self.stale {
                break;
            }
        }
        self.settle = false;
        self.draw(width, height, clear, &quads);
        let count = quads.len();
        super::let_go(&mut quads);
        super::let_go(&mut held);
        (self.quads, self.held) = (quads, held);
        count
    }

    fn draw(&mut self, width: u32, height: u32, clear: Color, instances: &[Instance]) {
        let bytes =
            unsafe { std::slice::from_raw_parts(instances.as_ptr() as *const u8, size_of_val(instances)) };
        let c = clear.map(|v| v as f32 / 255.0);
        unsafe {
            let gl = &self.gl;
            gl.viewport(0, 0, width as i32, height as i32);
            gl.clear_color(c[0], c[1], c[2], 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            gl.use_program(Some(self.program));
            gl.uniform_2_f32(self.u_view.as_ref(), width as f32, height as f32);
            gl.active_texture(glow::TEXTURE2);
            gl.bind_texture(glow::TEXTURE_2D, self.picture.map(|(texture, _)| texture));
            gl.active_texture(glow::TEXTURE1);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.color.tex));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(self.mask.tex));
            gl.bind_vertex_array(Some(self.vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            gl.draw_arrays_instanced(glow::TRIANGLE_STRIP, 0, 4, instances.len() as i32);
        }
    }
}
