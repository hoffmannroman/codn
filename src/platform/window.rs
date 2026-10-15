//! The window and its OpenGL context.

use std::error::Error;
use std::num::NonZeroU32;

use glutin::config::{ConfigTemplateBuilder, GlConfig};
use glutin::context::{
    ContextApi, ContextAttributesBuilder, GlContext, NotCurrentGlContext, PossiblyCurrentContext, Version,
};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle};
use winit::event_loop::ActiveEventLoop;
use winit::window::{Window, WindowAttributes};

use crate::render::Renderer;
use crate::render::font::Fonts;

pub struct Gfx {
    // Dropped in this order: what draws to the window goes before it.
    pub renderer: Renderer,
    surface: Surface<WindowSurface>,
    context: PossiblyCurrentContext,
    pub window: Window,
    /// The compositor decides when a frame may be drawn (Wayland).
    pub paced: bool,
}

impl Gfx {
    pub fn new(
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        fonts: impl FnOnce() -> Result<Fonts, Box<dyn Error>>,
    ) -> Result<Self, Box<dyn Error>> {
        // Neither depth nor stencil is used, and edges are antialiased in the
        // shader, so the plainest configuration is the right one.
        let template = ConfigTemplateBuilder::new().with_depth_size(0).with_stencil_size(0);
        let (window, config) = DisplayBuilder::new().with_window_attributes(Some(attributes)).build(
            event_loop,
            template,
            |configs| {
                configs
                    .reduce(|a, b| if b.num_samples() < a.num_samples() { b } else { a })
                    .expect("glutin reports an error when no configuration matches")
            },
        )?;
        let window = window.ok_or("could not create a window")?;
        let handle = window.window_handle()?.as_raw();
        let display = config.display();

        let context =
            [ContextApi::OpenGl(Some(Version::new(3, 3))), ContextApi::Gles(Some(Version::new(3, 0)))]
                .into_iter()
                .find_map(|api| {
                    let attributes =
                        ContextAttributesBuilder::new().with_context_api(api).build(Some(handle));
                    unsafe { display.create_context(&config, &attributes).ok() }
                })
                .ok_or("this system offers neither OpenGL 3.3 nor OpenGL ES 3.0")?;
        let surface_attributes =
            window.build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())?;
        let surface = unsafe { display.create_window_surface(&config, &surface_attributes)? };
        let context = context.make_current(&surface)?;

        // Wayland paces frames through frame callbacks, a blocking swap would
        // stall the whole app while the window is hidden.
        let wayland = matches!(window.display_handle()?.as_raw(), RawDisplayHandle::Wayland(_));
        let interval = if wayland { SwapInterval::DontWait } else { SwapInterval::Wait(NonZeroU32::MIN) };
        let _ = surface.set_swap_interval(&context, interval);

        let gles = matches!(context.context_api(), ContextApi::Gles(_));
        let gl = unsafe { glow::Context::from_loader_function_cstr(|name| display.get_proc_address(name)) };
        // Only now: the fonts are resolved on a thread while the window and
        // its context are created, and waited for when both are there.
        let renderer = Renderer::new(gl, gles, fonts()?)?;
        Ok(Self { renderer, surface, context, window, paced: wayland })
    }

    pub fn resize(&self, width: u32, height: u32) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(width), NonZeroU32::new(height)) {
            self.surface.resize(&self.context, w, h);
        }
    }

    pub fn present(&self) {
        self.window.pre_present_notify();
        let _ = self.surface.swap_buffers(&self.context);
    }
}
