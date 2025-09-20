use std::ops::Deref;
use std::sync::Arc;

use egui::epaint::ahash::AHashMap;
#[cfg(feature = "cpu_fix")]
use egui::epaint::Mesh16;
use egui::epaint::Primitive;
use egui::{ClippedPrimitive, ImageData, Pos2, TextureId, TexturesDelta};
use skia_safe::vertices::VertexMode;
use skia_safe::{images, scalar, surfaces, BlendMode, Canvas, ClipOp, Color, ConditionallySend, Data, Drawable, Image, ImageInfo, Paint, PictureRecorder, Point, Rect, Sendable, Vertices};
use skia_safe::canvas::AutoRestoredCanvas;

struct PaintHandle {
    paint: Paint,
    image: Image,
}

pub struct Painter {
    paints: AHashMap<TextureId, PaintHandle>,
    white_paint_workaround: Paint,
}

impl Painter {
    pub fn new() -> Painter {
        let mut white_paint_workaround = Paint::default();
        white_paint_workaround.set_color(Color::WHITE);

        Self {
            paints: AHashMap::new(),
            white_paint_workaround,
        }
    }

    pub fn paint_and_update_textures(
        &mut self,
        canvas: &Canvas,
        dpi: f32,
        primitives: Vec<ClippedPrimitive>,
        textures_delta: TexturesDelta,
    ) {
        textures_delta.set.iter().for_each(|(id, image_delta)| {
            self.set_texture(*id, image_delta);
        });

        for primitive in primitives {
            let skclip_rect = Rect::new(
                primitive.clip_rect.min.x,
                primitive.clip_rect.min.y,
                primitive.clip_rect.max.x,
                primitive.clip_rect.max.y,
            );

            match primitive.primitive {
                Primitive::Mesh(mesh) => {
                    canvas.set_matrix(skia_safe::M44::new_identity().set_scale(dpi, dpi, 1.0));
                    let arc = skia_safe::AutoCanvasRestore::guard(canvas, true);

                    #[cfg(feature = "cpu_fix")]
                    let meshes = mesh
                        .split_to_u16()
                        .into_iter()
                        .flat_map(|mesh| self.split_texture_meshes(mesh))
                        .collect::<Vec<Mesh16>>();
                    #[cfg(not(feature = "cpu_fix"))]
                    let meshes = mesh.split_to_u16();

                    for mesh in &meshes {
                        self.paint_mesh(&arc, &skclip_rect, mesh);
                    }
                }
                Primitive::Callback(data) => {
                    let callback: Arc<EguiSkiaPaintCallback> = data.callback.downcast().unwrap();
                    let rect = data.rect;

                    let skia_rect = Rect::new(
                        rect.min.x * dpi,
                        rect.min.y * dpi,
                        rect.max.x * dpi,
                        rect.max.y * dpi,
                    );

                    let mut drawable: Drawable = callback.callback.deref()(skia_rect).0.into_inner();
                    let mut arc = skia_safe::AutoCanvasRestore::guard(canvas, true);

                    arc.clip_rect(skclip_rect, ClipOp::default(), true);
                    arc.translate((rect.min.x, rect.min.y));
                    drawable.draw(&mut arc, None);
                }
            }
        }

        textures_delta.free.iter().for_each(|id| {
            self.free_texture(*id);
        });
    }

    fn set_texture(&mut self, tex_id: TextureId, image_delta: &egui::epaint::ImageDelta) {
        let delta_image = match &image_delta.image {
            ImageData::Color(color_image) => {
                images::raster_from_data(
                    &ImageInfo::new_n32_premul(
                        skia_safe::ISize::new(
                            color_image.width() as i32,
                            color_image.height() as i32,
                        ),
                        None,
                    ),
                    Data::new_copy(
                        color_image
                            .pixels
                            .iter()
                            .flat_map(|p| p.to_array())
                            .collect::<Vec<_>>()
                            .as_slice(),
                    ),
                    color_image.width() * 4,
                )
                    .unwrap()
            }
        };

        let image = match image_delta.pos {
            None => delta_image,
            Some(pos) => {
                let old_image = self.paints.remove(&tex_id).unwrap().image;

                let mut surface = surfaces::raster_n32_premul(skia_safe::ISize::new(
                    old_image.width(),
                    old_image.height(),
                ))
                    .unwrap();

                let canvas = surface.canvas();
                canvas.draw_image(&old_image, Point::new(0.0, 0.0), None);

                canvas.clip_rect(
                    Rect::new(
                        pos[0] as scalar,
                        pos[1] as scalar,
                        (pos[0] as i32 + delta_image.width()) as scalar,
                        (pos[1] as i32 + delta_image.height()) as scalar,
                    ),
                    ClipOp::default(),
                    false,
                );

                canvas.clear(Color::TRANSPARENT);
                canvas.draw_image(&delta_image, Point::new(pos[0] as f32, pos[1] as f32), None);

                surface.image_snapshot()
            }
        };

        let local_matrix = skia_safe::Matrix::scale((
            1.0 / image.width() as f32,
            1.0 / image.height() as f32,
        ));

        #[cfg(feature = "cpu_fix")]
        let sampling_options = skia_safe::SamplingOptions::new(
            skia_safe::FilterMode::Nearest,
            skia_safe::MipmapMode::None,
        );
        #[cfg(not(feature = "cpu_fix"))]
        let sampling_options = {
            use egui::TextureFilter;
            let filter_mode = match image_delta.options.magnification {
                TextureFilter::Nearest => skia_safe::FilterMode::Nearest,
                TextureFilter::Linear => skia_safe::FilterMode::Linear,
            };
            let mm_mode = match image_delta.options.minification {
                TextureFilter::Nearest => skia_safe::MipmapMode::Nearest,
                TextureFilter::Linear => skia_safe::MipmapMode::Linear,
            };
            skia_safe::SamplingOptions::new(filter_mode, mm_mode)
        };

        let tile_mode = skia_safe::TileMode::Clamp;
        let shader = image
            .to_shader((tile_mode, tile_mode), sampling_options, &local_matrix)
            .unwrap();

        let mut paint = Paint::default();
        paint.set_shader(shader);
        paint.set_color(Color::WHITE);

        self.paints.insert(tex_id, PaintHandle { paint, image });
    }

    fn free_texture(&mut self, tex_id: TextureId) {
        self.paints.remove(&tex_id);
    }

    fn paint_mesh(
        &self,
        arc: &AutoRestoredCanvas,
        skclip_rect: &Rect,
        mesh: &egui::epaint::Mesh16,
    ) {
        let texture_id = mesh.texture_id;

        let mut pos = Vec::with_capacity(mesh.vertices.len());
        let mut texs = Vec::with_capacity(mesh.vertices.len());
        let mut colors = Vec::with_capacity(mesh.vertices.len());

        mesh.vertices.iter().for_each(|v| {
            let fixed_pos = if v.pos.x.is_nan() || v.pos.y.is_nan() {
                Pos2::new(0.0, 0.0)
            } else {
                v.pos
            };

            pos.push(Point::new(fixed_pos.x, fixed_pos.y));
            texs.push(Point::new(v.uv.x, v.uv.y));

            let c = v.color;
            let c = Color::from_argb(c.a(), c.r(), c.g(), c.b());
            let mut cf = skia_safe::Color4f::from(c);
            cf.r /= cf.a;
            cf.g /= cf.a;
            cf.b /= cf.a;
            colors.push(Color::from_argb(
                c.a(),
                (cf.r * 255.0) as u8,
                (cf.g * 255.0) as u8,
                (cf.b * 255.0) as u8,
            ));
        });

        let vertices = Vertices::new_copy(
            VertexMode::Triangles,
            &pos,
            &texs,
            &colors,
            Some(
                mesh.indices
                    .iter()
                    .map(|index| *index as u16)
                    .collect::<Vec<u16>>()
                    .as_slice(),
            ),
        );

        arc.clip_rect(*skclip_rect, ClipOp::default(), true);

        #[cfg(feature = "cpu_fix")]
        let use_white_workaround = !texs
            .first()
            .map(|point| point.x != 0.0 || point.y != 0.0)
            .unwrap_or(false);
        #[cfg(not(feature = "cpu_fix"))]
        let use_white_workaround = false;

        let paint = if use_white_workaround {
            &self.white_paint_workaround
        } else {
            &self.paints[&texture_id].paint
        };

        arc.draw_vertices(&vertices, BlendMode::Modulate, paint);
    }

    #[cfg(feature = "cpu_fix")]
    fn split_texture_meshes(&self, mesh: egui::epaint::Mesh16) -> Vec<egui::epaint::Mesh16> {
        let mut is_zero = None;
        let mut meshes = Vec::new();
        meshes.push(egui::epaint::Mesh16 {
            indices: vec![],
            vertices: vec![],
            texture_id: mesh.texture_id,
        });

        for index in mesh.indices.iter() {
            let vertex = mesh.vertices.get(*index as usize).unwrap();
            let is_current_zero = vertex.uv.x == 0.0 && vertex.uv.y == 0.0;

            if is_current_zero != is_zero.unwrap_or(is_current_zero) {
                meshes.push(egui::epaint::Mesh16 {
                    indices: vec![],
                    vertices: vec![],
                    texture_id: mesh.texture_id,
                });
                is_zero = Some(is_current_zero);
            }

            if is_zero.is_none() {
                is_zero = Some(is_current_zero);
            }

            let last = meshes.last_mut().unwrap();
            last.vertices.push(vertex.clone());
            last.indices.push(last.indices.len() as u16);
        }

        meshes
    }
}

impl Default for Painter {
    fn default() -> Self {
        Self::new()
    }
}

pub struct EguiSkiaPaintCallback {
    callback: Box<dyn Fn(Rect) -> SyncSendableDrawable + Send + Sync>,
}

impl EguiSkiaPaintCallback {
    pub fn new<F: Fn(&Canvas) + Send + Sync + 'static>(callback: F) -> EguiSkiaPaintCallback {
        EguiSkiaPaintCallback {
            callback: Box::new(move |rect| {
                let mut pr = PictureRecorder::new();
                let canvas = pr.begin_recording(rect, false);
                callback(canvas);
                SyncSendableDrawable(
                    pr.finish_recording_as_drawable()
                        .unwrap()
                        .wrap_send()
                        .unwrap(),
                )
            }),
        }
    }
}

struct SyncSendableDrawable(pub Sendable<Drawable>);

unsafe impl Sync for SyncSendableDrawable {}
