use egui::{Context, Pos2, ViewportId, ViewportInfo};
use egui::viewport::ViewportIdMap;
use skia_safe::{surfaces, Canvas, Surface};

use crate::painter::Painter;

pub struct RasterizeOptions {
    pub pixels_per_point: f32,
    /// The number of frames to render before a screenshot is taken.
    /// Default is 2, so egui will be able to display windows
    pub frames_before_screenshot: usize,
}

impl Default for RasterizeOptions {
    fn default() -> Self {
        Self {
            pixels_per_point: 1.0,
            frames_before_screenshot: 2,
        }
    }
}

pub fn rasterize(
    size: (i32, i32),
    ui: impl FnMut(&Context),
    options: Option<RasterizeOptions>,
) -> Surface {
    let mut surface = surfaces::raster_n32_premul(size).expect("Failed to create surface");
    draw_onto_surface(&mut surface, ui, options);
    surface
}

pub fn draw_onto_surface(
    surface: &mut Surface,
    mut ui: impl FnMut(&Context),
    options: Option<RasterizeOptions>,
) {
    let RasterizeOptions {
        pixels_per_point,
        frames_before_screenshot,
    } = options.unwrap_or_default();
    let mut backend = EguiSkia::new();

    // Create viewport info with pixels_per_point
    let viewport_id = ViewportId::ROOT;
    let mut viewport_info = ViewportInfo::default();
    viewport_info.inner_rect = Some([
        Pos2::default(),
        Pos2::new(surface.width() as f32, surface.height() as f32),
    ].into());

    let mut viewports = ViewportIdMap::default();
    viewports.insert(viewport_id, viewport_info);

    let input = egui::RawInput {
        viewports,
        ..Default::default()
    };

    // Set pixels_per_point on the context
    backend.egui_ctx.set_pixels_per_point(pixels_per_point);

    for _ in 0..frames_before_screenshot {
        backend.run(input.clone(), &mut ui);
    }
    backend.paint(surface.canvas());
}

/// Convenience wrapper for using [`egui`] from a [`skia`] app.
pub struct EguiSkia {
    pub egui_ctx: Context,
    pub painter: Painter,

    shapes: Vec<egui::epaint::ClippedShape>,
    textures_delta: egui::TexturesDelta,
}

impl EguiSkia {
    pub fn new() -> Self {
        let painter = Painter::new();
        Self {
            egui_ctx: Default::default(),
            painter,
            shapes: Default::default(),
            textures_delta: Default::default(),
        }
    }

    /// Run egui and return the platform output.
    ///
    /// Call [`Self::paint`] later to paint.
    pub fn run(
        &mut self,
        input: egui::RawInput,
        run_ui: impl FnMut(&Context),
    ) -> egui::PlatformOutput {
        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point: _,
            viewport_output: _,
        } = self.egui_ctx.run(input, run_ui);

        self.shapes = shapes;
        self.textures_delta.append(textures_delta);

        platform_output
    }

    /// Paint the results of the last call to [`Self::run`].
    pub fn paint(&mut self, canvas: &Canvas) {
        let shapes = std::mem::take(&mut self.shapes);
        let textures_delta = std::mem::take(&mut self.textures_delta);
        let clipped_primitives = self.egui_ctx.tessellate(shapes, self.egui_ctx.pixels_per_point());
        self.painter.paint_and_update_textures(
            canvas,
            self.egui_ctx.pixels_per_point(),
            clipped_primitives,
            textures_delta,
        );
    }
}

impl Default for EguiSkia {
    fn default() -> Self {
        Self::new()
    }
}
