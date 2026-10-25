//! What the canvas shows, checked with a software GPU: it keeps the textures
//! (and fails like wgpu on one that is too large or an update outside one)
//! and draws the meshes.

use std::sync::{Arc, Mutex};

use egui::epaint::{Primitive, Vertex};
use egui::{ImageData, TextureFilter, TextureId};
use egui_kittest::Harness;
use image::{Rgba, RgbaImage};

use super::*;

/// Textures as a GPU holds them.
#[derive(Default)]
struct SoftGpu {
    textures: HashMap<TextureId, (Arc<ColorImage>, TextureOptions)>,
    /// Largest texture side (the fonts' texture is exempt).
    max_side: Option<usize>,
}

struct SoftRenderer(Arc<Mutex<SoftGpu>>);

impl egui_kittest::TestRenderer for SoftRenderer {
    fn handle_delta(&mut self, delta: &mut egui::TexturesDelta) {
        let mut gpu = self.0.lock().unwrap();
        let max = gpu.max_side;
        for (id, deltas) in delta.set.drain() {
            for d in deltas {
                let ImageData::Color(image) = d.image;
                let [w, h] = image.size;
                match d.pos {
                    None => {
                        assert!(w > 0 && h > 0, "zero-sized texture");
                        if id != TextureId::default()
                            && let Some(max) = max
                        {
                            assert!(w <= max && h <= max, "texture {w}×{h} over the limit {max}");
                        }
                        gpu.textures.insert(id, (image, d.options));
                    }
                    Some([x, y]) => {
                        let (texture, _) = gpu.textures.get_mut(&id).expect("texture allocated");
                        let [tw, th] = texture.size;
                        assert!(x + w <= tw && y + h <= th, "update outside the texture");
                        let texture = Arc::make_mut(texture);
                        for row in 0..h {
                            let to = (y + row) * tw + x;
                            texture.pixels[to..to + w]
                                .copy_from_slice(&image.pixels[row * w..(row + 1) * w]);
                        }
                    }
                }
            }
        }
        for id in delta.free.drain() {
            gpu.textures.remove(&id);
        }
    }

    fn render(
        &mut self,
        ctx: &egui::Context,
        output: &egui::FullOutput,
    ) -> Result<RgbaImage, String> {
        let ppp = output.pixels_per_point;
        let screen = ctx.content_rect().size() * ppp;
        let (w, h) = (screen.x.round() as usize, screen.y.round() as usize);
        let mut out = vec![[0.0f32; 4]; w * h];
        let mut drawn = vec![false; w * h];
        let gpu = self.0.lock().unwrap();
        for prim in ctx.tessellate(output.shapes.clone(), ppp) {
            let Primitive::Mesh(mesh) = &prim.primitive else {
                continue;
            };
            let (texture, options) = gpu
                .textures
                .get(&mesh.texture_id)
                .ok_or("missing texture")?;
            let clip = prim.clip_rect;
            let (cx0, cy0) = ((clip.min.x * ppp).max(0.0), (clip.min.y * ppp).max(0.0));
            let (cx1, cy1) = (
                (clip.max.x * ppp).min(w as f32),
                (clip.max.y * ppp).min(h as f32),
            );
            // Each pixel once per mesh (the two triangles of a quad share an edge).
            let mut touched = Vec::new();
            for tri in mesh.indices.chunks_exact(3) {
                let v: [Vertex; 3] = [0, 1, 2].map(|k| mesh.vertices[tri[k] as usize]);
                let p = v.map(|v| v.pos * ppp);
                let area = edge(p[0], p[1], p[2]);
                if area == 0.0 {
                    continue;
                }
                let [tw, th] = texture.size.map(|s| s as f32);
                let uv_area = edge(v[0].uv, v[1].uv, v[2].uv) * tw * th;
                let filter = if uv_area.abs() > area.abs() * 1.001 {
                    options.minification
                } else {
                    options.magnification
                };
                let x0 = p
                    .iter()
                    .map(|p| p.x)
                    .fold(f32::MAX, f32::min)
                    .max(cx0)
                    .floor() as usize;
                let x1 = p
                    .iter()
                    .map(|p| p.x)
                    .fold(f32::MIN, f32::max)
                    .min(cx1)
                    .ceil() as usize;
                let y0 = p
                    .iter()
                    .map(|p| p.y)
                    .fold(f32::MAX, f32::min)
                    .max(cy0)
                    .floor() as usize;
                let y1 = p
                    .iter()
                    .map(|p| p.y)
                    .fold(f32::MIN, f32::max)
                    .min(cy1)
                    .ceil() as usize;
                for y in y0..y1.min(h) {
                    for x in x0..x1.min(w) {
                        let c = pos2(x as f32 + 0.5, y as f32 + 0.5);
                        if c.x < cx0 || c.x >= cx1 || c.y < cy0 || c.y >= cy1 || drawn[y * w + x] {
                            continue;
                        }
                        let b = [
                            edge(p[1], p[2], c),
                            edge(p[2], p[0], c),
                            edge(p[0], p[1], c),
                        ]
                        .map(|e| e / area);
                        if b.iter().any(|&b| b < 0.0) {
                            continue;
                        }
                        drawn[y * w + x] = true;
                        touched.push(y * w + x);
                        let uv = (0..3).fold(Vec2::ZERO, |s, k| s + v[k].uv.to_vec2() * b[k]);
                        let texel = sample(texture, filter, uv);
                        let src: [f32; 4] = std::array::from_fn(|ch| {
                            let tint: f32 = (0..3).map(|k| v[k].color[ch] as f32 * b[k]).sum();
                            tint / 255.0 * texel[ch]
                        });
                        let dst = &mut out[y * w + x];
                        for ch in 0..4 {
                            dst[ch] = src[ch] + dst[ch] * (1.0 - src[3]);
                        }
                    }
                }
            }
            for i in touched {
                drawn[i] = false;
            }
        }
        let raw = out
            .iter()
            .flat_map(|p| p.map(|c| (c * 255.0).round().clamp(0.0, 255.0) as u8))
            .collect();
        Ok(RgbaImage::from_raw(w as u32, h as u32, raw).expect("size"))
    }
}

fn edge(a: Pos2, b: Pos2, p: Pos2) -> f32 {
    (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x)
}

/// Premultiplied color (0..1) of a texture at `uv`, clamped to its edges.
fn sample(texture: &ColorImage, filter: TextureFilter, uv: Vec2) -> [f32; 4] {
    let [w, h] = texture.size;
    let texel = |x: i64, y: i64| {
        let (x, y) = (
            x.clamp(0, w as i64 - 1) as usize,
            y.clamp(0, h as i64 - 1) as usize,
        );
        texture.pixels[y * w + x]
            .to_array()
            .map(|c| c as f32 / 255.0)
    };
    let (x, y) = (uv.x * w as f32, uv.y * h as f32);
    match filter {
        TextureFilter::Nearest => texel(x.floor() as i64, y.floor() as i64),
        TextureFilter::Linear => {
            let (x, y) = (x - 0.5, y - 0.5);
            let (fx, fy) = (x - x.floor(), y - y.floor());
            let (x, y) = (x.floor() as i64, y.floor() as i64);
            let (a, b, c, d) = (
                texel(x, y),
                texel(x + 1, y),
                texel(x, y + 1),
                texel(x + 1, y + 1),
            );
            std::array::from_fn(|ch| {
                let top = a[ch] + (b[ch] - a[ch]) * fx;
                let bottom = c[ch] + (d[ch] - c[ch]) * fx;
                top + (bottom - top) * fy
            })
        }
    }
}

/// The app with `doc`, on a software GPU whose largest texture side is
/// `max_side` (egui then splits images by that size too).
fn app_with(doc: Document, tool: Tool, max_side: usize) -> Harness<'static, AtelierApp> {
    let gpu = Arc::new(Mutex::new(SoftGpu::default()));
    let mut h = Harness::builder()
        .with_size([1100.0, 750.0])
        .renderer(SoftRenderer(Arc::clone(&gpu)))
        .build_eframe(|cc| AtelierApp::new(cc, None));
    h.input_mut().max_texture_side = Some(max_side);
    gpu.lock().unwrap().max_side = Some(max_side);
    let app = h.state_mut();
    app.doc = Some(doc);
    app.tool = tool;
    // Actual pixels: the probed pixels are sampled exactly.
    (app.zoom, app.pan, app.fit_pending) = (1.0, Vec2::ZERO, None);
    h.run_steps(3);
    h
}

/// Color shown on screen at canvas pixel (x, y).
fn shown(shot: &RgbaImage, h: &Harness<'_, AtelierApp>, x: u32, y: u32) -> [u8; 3] {
    let view = h.state().last_view.expect("canvas shown");
    let ppp = h.ctx.pixels_per_point();
    let p = view.to_screen(pos2(x as f32 + 0.5, y as f32 + 0.5)) * ppp;
    let [r, g, b, _] = shot.get_pixel(p.x as u32, p.y as u32).0;
    [r, g, b]
}

fn solid(w: u32, h: u32, c: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba(c))
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(&a, b)| a.abs_diff(b) <= 2)
}

/// Large images are shown as tiles (the GPU never gets a texture over its
/// limit), whole, without seams.
#[test]
fn large_images_are_shown_in_tiles() {
    let img = RgbaImage::from_fn(2300, 2100, |x, y| Rgba([x as u8, y as u8, 200, 255]));
    let mut h = app_with(Document::new(img.clone(), None), Tool::Hand, 2048);
    // Around the corner where 4 tiles meet.
    h.state_mut().pan = vec2(1150.0 - 2048.0, 1050.0 - 2048.0);
    h.run_steps(1);
    let shot = h.render().unwrap();
    for (x, y) in [
        (2047, 2047),
        (2048, 2048),
        (2047, 2048),
        (2048, 2047),
        (2100, 2000),
    ] {
        let [r, g, b, _] = img.get_pixel(x, y).0;
        assert_eq!(shown(&shot, &h, x, y), [r, g, b], "pixel {x}, {y}");
    }
}

/// The brush grows a layer smaller than the canvas: it shows whole, not
/// stretched.
#[test]
fn painting_a_layer_smaller_than_the_canvas() {
    let mut doc = Document::new(solid(400, 300, [255, 255, 255, 255]), None);
    doc.add_layer("small".into(), solid(50, 50, [0, 0, 255, 255]), (10, 10));
    let mut h = app_with(doc, Tool::Brush, 2048);
    let view = h.state().last_view.unwrap();
    let at = view.to_screen(pos2(300.0, 200.0));
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.hover_at(at + vec2(5.0, 5.0));
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: at + vec2(5.0, 5.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.run_steps(2);
    let shot = h.render().unwrap();
    assert_eq!(shown(&shot, &h, 30, 30), [0, 0, 255]);
    assert_eq!(shown(&shot, &h, 200, 100), [255, 255, 255]);
    assert!(shown(&shot, &h, 300, 200)[0] < 60);
}

/// A 0 × 0 image (some decoders accept one) does not crash the display.
#[test]
fn zero_sized_document() {
    let mut h = app_with(Document::new(RgbaImage::new(0, 0), None), Tool::Brush, 2048);
    h.run_steps(2);
    h.render().unwrap();
}

/// Document with a Multiply layer over everything: white background, a
/// green layer (active), a gray Multiply layer.
fn multiply_doc() -> Document {
    let mut doc = Document::new(solid(400, 300, [255, 255, 255, 255]), None);
    doc.add_layer("B".into(), solid(400, 300, [0, 200, 0, 255]), (0, 0));
    doc.add_layer("M".into(), solid(400, 300, [128, 128, 128, 255]), (0, 0));
    doc.layers[2].blend = BlendMode::Multiply;
    doc.active = 1;
    doc
}

/// The composite follows what changed while no layer was in a blend mode
/// (it showed old pixels: only the last brush stroke was redone).
#[test]
fn composite_follows_changes_made_while_the_blend_layer_was_hidden() {
    let mut h = app_with(multiply_doc(), Tool::Brush, 2048);
    h.state_mut().doc.as_mut().unwrap().layers[2].visible = false;
    h.run_steps(2);
    {
        // A brush stroke on B, then another change of all of B.
        let app = h.state_mut();
        let layer = &mut app.doc.as_mut().unwrap().layers[1];
        for y in 0..10 {
            for x in 0..10 {
                layer.pixels_mut().put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        app.stroke_dirty = Some(DirtyRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10,
        });
    }
    h.run_steps(2);
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.layers[1].replace_pixels(solid(400, 300, [200, 0, 0, 255]));
    h.run_steps(2);
    h.state_mut().doc.as_mut().unwrap().layers[2].visible = true;
    h.run_steps(2);
    let shot = h.render().unwrap();
    assert!(near(shown(&shot, &h, 200, 150), [100, 0, 0]));
    assert!(near(shown(&shot, &h, 5, 5), [100, 0, 0]));
}

/// Moving a layer under a Multiply layer: the display matches the result
/// on every frame (only the area it leaves and enters is composited again).
#[test]
fn composite_follows_a_moved_layer() {
    // Zoomed out too: the reduced copies follow.
    for zoom in [1.0, 0.25] {
        let mut doc = multiply_doc();
        // Under the Multiply layer.
        doc.add_layer("small".into(), solid(40, 40, [0, 0, 255, 255]), (20, 20));
        let mut h = app_with(doc, Tool::Hand, 2048);
        h.state_mut().zoom = zoom;
        h.run_steps(1);
        for step in 1..4 {
            let doc = h.state_mut().doc.as_mut().unwrap();
            doc.layers[2].offset = (20 + 50 * step, 20 + 30 * step);
            // The large green layer too: its whole area changes.
            doc.layers[1].offset = (step, 0);
            h.run_steps(1);
            let shot = h.render().unwrap();
            let flat = h.state().doc.as_ref().unwrap().flatten();
            let mut probes = vec![(30, 30), (75, 55), (25 + 50 * step, 25 + 30 * step)];
            if zoom == 1.0 {
                probes.extend([(0, 0), (399, 299)]);
            }
            for (x, y) in probes {
                let [r, g, b, _] = flat.get_pixel(x as u32, y as u32).0;
                let at = shown(&shot, &h, x as u32, y as u32);
                assert!(
                    near(at, [r, g, b]),
                    "zoom {zoom}, step {step}: {x}, {y}: {at:?}"
                );
            }
        }
    }
}

/// Painting zoomed out: the reduced copy shown follows the brush.
#[test]
fn painting_zoomed_out() {
    let mut h = app_with(
        Document::new(solid(400, 300, [255, 255, 255, 255]), None),
        Tool::Brush,
        2048,
    );
    (h.state_mut().zoom, h.state_mut().brush_size) = (0.25, 40.0);
    h.run_steps(1);
    let at = h.state().last_view.unwrap().to_screen(pos2(201.0, 151.0));
    h.hover_at(at);
    h.run_steps(1);
    h.drag_at(at);
    h.run_steps(1);
    h.hover_at(at + vec2(2.0, 0.0));
    h.run_steps(1);
    let shot = h.render().unwrap();
    assert!(shown(&shot, &h, 202, 150)[0] < 60);
    assert_eq!(shown(&shot, &h, 100, 100), [255, 255, 255]);
}

/// The adjustment preview shows on the canvas when a layer uses a blend
/// mode too (only the layer's thumbnail changed).
#[test]
fn adjustment_preview_with_a_blend_layer() {
    let mut h = app_with(multiply_doc(), Tool::Hand, 2048);
    h.state_mut().run(Action::Adjust(AdjustKind::HueSaturation));
    if let Some(Adjustment::HueSaturation(hs)) =
        h.state_mut().adjust.as_mut().map(|d| &mut d.adjustment)
    {
        hs.lightness = -1.0;
    }
    // The preview is computed on another thread.
    let start = std::time::Instant::now();
    loop {
        h.run_steps(1);
        let shot = h.render().unwrap();
        if near(shown(&shot, &h, 200, 150), [0, 0, 0]) {
            break;
        }
        assert!(start.elapsed().as_secs() < 30, "no preview shown");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.state_mut().adjust = None;
    h.state_mut().end_layer_preview();
    h.run_steps(2);
    let shot = h.render().unwrap();
    assert!(near(shown(&shot, &h, 200, 150), [0, 100, 0]));
}

/// An element being transformed is previewed in its place in the stack,
/// with its effects, under the layers above it (in their blend modes).
#[test]
fn transform_preview_keeps_its_place_and_effects() {
    let mut doc = Document::new(solid(400, 300, [255, 255, 255, 255]), None);
    doc.add_layer("A".into(), solid(40, 40, [255, 0, 0, 255]), (20, 20));
    doc.layers[1].effects.stroke.enabled = true;
    doc.layers[1].effects.stroke.color = [0, 0, 255, 255];
    doc.layers[1].effects.stroke.width = 4.0;
    doc.add_layer("M".into(), solid(400, 300, [128, 128, 128, 255]), (0, 0));
    doc.layers[2].blend = BlendMode::Multiply;
    doc.add_layer("C".into(), solid(20, 20, [0, 255, 0, 255]), (120, 120));
    doc.active = 1;
    let mut h = app_with(doc, Tool::Transform, 2048);
    h.state_mut().transform_rect = Some(Rect::from_min_size(pos2(110.0, 110.0), vec2(40.0, 40.0)));
    h.run_steps(2);
    let shot = h.render().unwrap();
    // In its place: under M (multiplied by its gray) and under C.
    // (Away from the frame's handles.)
    assert!(near(shown(&shot, &h, 115, 125), [128, 0, 0]));
    assert_eq!(shown(&shot, &h, 130, 130), [0, 255, 0]);
    // Its stroke moved with it; its old place shows what was under it.
    assert!(near(shown(&shot, &h, 108, 120), [0, 0, 128]));
    assert!(near(shown(&shot, &h, 30, 30), [128, 128, 128]));
}

/// At 100 %, one image pixel is one physical pixel, on the pixel grid,
/// whatever the display scaling and the image's size.
#[test]
fn actual_pixels_on_the_pixel_grid() {
    for ppp in [1.0, 1.25, 1.5] {
        let mut h = Harness::builder()
            .with_size([800.0, 600.0])
            .with_pixels_per_point(ppp)
            .build_eframe(|cc| AtelierApp::new(cc, None));
        h.state_mut().doc = Some(Document::new(solid(301, 151, [0, 0, 0, 255]), None));
        h.state_mut().run(Action::ActualSize);
        h.state_mut().pan = vec2(0.3, 0.7);
        h.run_steps(2);
        let view = h.state().last_view.unwrap();
        assert!((view.zoom * ppp - 1.0).abs() < 1e-5, "{ppp}");
        let origin = view.origin * ppp;
        assert!(
            (origin.x - origin.x.round()).abs() < 1e-3,
            "{ppp}: {origin:?}"
        );
        assert!(
            (origin.y - origin.y.round()).abs() < 1e-3,
            "{ppp}: {origin:?}"
        );
    }
}

/// Zoomed out, an image shows the average of its pixels (from a reduced
/// copy), not a few of them: no shimmering noise or moiré.
#[test]
fn zoomed_out_images_are_filtered() {
    let noise = |x: u32, y: u32| {
        let v = x
            .wrapping_mul(374_761_393)
            .wrapping_add(y.wrapping_mul(668_265_263));
        let v = (v ^ (v >> 13)).wrapping_mul(1_274_126_177);
        (v ^ (v >> 16)) & 1 == 1
    };
    let img = RgbaImage::from_fn(512, 512, |x, y| {
        let c = if noise(x, y) { 255 } else { 0 };
        Rgba([c, c, c, 255])
    });
    let mut h = app_with(Document::new(img, None), Tool::Hand, 2048);
    h.state_mut().zoom = 0.125;
    h.run_steps(2);
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let corner = view.to_screen(pos2(64.0, 64.0));
    let values: Vec<f32> = (0..40)
        .flat_map(|y| (0..40).map(move |x| (x, y)))
        .map(|(x, y)| shot.get_pixel(corner.x as u32 + x, corner.y as u32 + y).0[0] as f32)
        .collect();
    let mean = values.iter().sum::<f32>() / values.len() as f32;
    let spread =
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32).sqrt();
    assert!(
        (mean - 127.5).abs() < 12.0 && spread < 30.0,
        "mean {mean}, spread {spread}"
    );
}

/// Noisy mask selection: far too many segments for shapes.
fn noisy_selection(w: u32, h: u32) -> Selection {
    let mask = image::GrayImage::from_fn(w, h, |x, y| {
        let v = x
            .wrapping_mul(374_761_393)
            .wrapping_add(y.wrapping_mul(668_265_263));
        let v = (v ^ (v >> 13)).wrapping_mul(1_274_126_177);
        image::Luma([((v ^ (v >> 16)) & 1) as u8 * 255])
    });
    Selection::from_mask(&mask, (0, 0)).unwrap()
}

/// A huge outline is drawn as an image of the screen (as shapes, it
/// overflowed the GPU's buffers), and moving the selection does not compute
/// it again.
#[test]
fn huge_selection_outline() {
    let mut h = app_with(
        Document::new(solid(300, 200, [90, 90, 90, 255]), None),
        Tool::Select,
        2048,
    );
    h.state_mut().selection = Some(noisy_selection(300, 200));
    h.run_steps(1);
    let vertices: usize = h
        .ctx
        .tessellate(h.output().shapes.clone(), 1.0)
        .iter()
        .map(|p| match &p.primitive {
            Primitive::Mesh(m) => m.vertices.len(),
            Primitive::Callback(_) => 0,
        })
        .sum();
    let edges = h.state().selection_outline.as_ref().unwrap().edges.len();
    assert!(edges > render::MAX_OUTLINE_SHAPES, "{edges} edges");
    assert!(vertices < edges, "{edges} edges, {vertices} vertices");
    let edges = |h: &Harness<'_, AtelierApp>| {
        h.state()
            .selection_outline
            .as_ref()
            .map(|o| o.edges.as_ptr())
    };
    let before = edges(&h);
    let sel = h.state_mut().selection.take().unwrap();
    h.state_mut().selection = Some(sel.translate(7, 3));
    h.run_steps(1);
    assert_eq!(edges(&h), before);
    let shot = h.render().unwrap();
    // Dashes: black and white pixels along the outline.
    let view = h.state().last_view.unwrap();
    let p = view.to_screen(pos2(57.0, 43.0));
    let around: Vec<[u8; 4]> = (0..8)
        .map(|d| shot.get_pixel(p.x as u32 + d, p.y as u32).0)
        .collect();
    assert!(around.iter().any(|c| c[..3] == [0, 0, 0]), "{around:?}");
    assert!(
        around.iter().any(|c| c[..3] == [255, 255, 255]),
        "{around:?}"
    );
}

/// A speckled selection on a large image: its outline is found on another
/// thread and, far too detailed, drawn from the coverage (dashes on its
/// edges), without keeping its millions of segments.
#[test]
fn speckled_selection_outline_is_drawn_from_its_coverage() {
    let mut h = app_with(
        Document::new(solid(2400, 1800, [90, 90, 90, 255]), None),
        Tool::Select,
        2048,
    );
    h.state_mut().selection = Some(noisy_selection(2400, 1800));
    let start = std::time::Instant::now();
    loop {
        h.run_steps(1);
        let o = h.state().selection_outline.as_ref();
        if o.is_some_and(|o| o.dense) {
            break;
        }
        assert!(start.elapsed().as_secs() < 30, "outline never found");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    h.run_steps(1);
    assert!(
        h.state()
            .selection_outline
            .as_ref()
            .unwrap()
            .edges
            .is_empty()
    );
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let p = view.to_screen(pos2(1200.0, 900.0));
    let around: Vec<[u8; 4]> = (0..16)
        .map(|d| shot.get_pixel(p.x as u32 + d, p.y as u32).0)
        .collect();
    assert!(around.iter().any(|c| c[..3] == [0, 0, 0]), "{around:?}");
    assert!(
        around.iter().any(|c| c[..3] == [255, 255, 255]),
        "{around:?}"
    );
}

/// Groups, a clipped layer, a mask, an adjustment, a fill layer and styles:
/// the display shows what the export gives, zoomed in or out, and follows
/// a change of the adjustment.
#[test]
fn display_matches_the_export_with_every_kind_of_layer() {
    use crate::document::{Fill, NewMask};
    use crate::pixel_ops::HueSaturation;
    let make = || {
        let mut doc = Document::new(solid(400, 300, [240, 240, 240, 255]), None);
        doc.add_layer("base".into(), solid(200, 150, [200, 40, 40, 255]), (40, 40));
        doc.layers[1].effects.stroke.enabled = true;
        doc.add_layer(
            "clip".into(),
            RgbaImage::from_fn(400, 300, |x, _| Rgba([20, 120, (x % 256) as u8, 255])),
            (0, 0),
        );
        doc.toggle_clipping();
        doc.layers[2].blend = BlendMode::Overlay;
        let hue = Adjustment::HueSaturation(HueSaturation {
            hue: 90.0,
            ..Default::default()
        });
        doc.add_adjustment_layer(hue, None);
        doc.group_layers(&[1, 2, 3]);
        doc.add_fill_layer(Fill::Solid([30, 60, 200]), None);
        doc.active_layer_mut().blend = BlendMode::Screen;
        doc.active_layer_mut().opacity = 0.5;
        let sel: Selection = PixelRect {
            x: 200,
            y: 0,
            w: 200,
            h: 300,
        }
        .into();
        doc.delete_mask();
        doc.add_mask(NewMask::RevealSelection, Some(&sel));
        doc
    };
    for zoom in [1.0, 0.25] {
        let mut h = app_with(make(), Tool::Hand, 2048);
        h.state_mut().zoom = zoom;
        h.run_steps(2);
        let check = |h: &mut Harness<'_, AtelierApp>| {
            let shot = h.render().unwrap();
            let flat = h.state().doc.as_ref().unwrap().flatten();
            for (x, y) in [
                (10, 10),
                (60, 60),
                (150, 100),
                (230, 120),
                (300, 200),
                (390, 290),
            ] {
                let [r, g, b, _] = flat.get_pixel(x, y).0;
                let at = shown(&shot, h, x, y);
                assert!(
                    near(at, [r, g, b]),
                    "zoom {zoom}: {x}, {y}: {at:?} / {:?}",
                    [r, g, b]
                );
            }
        };
        check(&mut h);
        // A change of the adjustment's setting shows.
        let doc = h.state_mut().doc.as_mut().unwrap();
        let adjusted = doc
            .layers
            .iter()
            .position(|l| matches!(l.kind, LayerKind::Adjustment(_)));
        doc.layers[adjusted.unwrap()].kind = LayerKind::Adjustment(Adjustment::Invert);
        h.run_steps(2);
        check(&mut h);
    }
}

/// Alt + click on a mask: the canvas shows it alone, in gray.
#[test]
fn mask_view_shows_the_mask() {
    use crate::document::NewMask;
    let mut doc = Document::new(solid(400, 300, [200, 30, 30, 255]), None);
    let sel: Selection = PixelRect {
        x: 0,
        y: 0,
        w: 100,
        h: 300,
    }
    .into();
    doc.add_mask(NewMask::RevealSelection, Some(&sel));
    let mut h = app_with(doc, Tool::Hand, 2048);
    h.state_mut().mask_view = true;
    h.run_steps(2);
    let shot = h.render().unwrap();
    assert_eq!(shown(&shot, &h, 50, 50), [255, 255, 255]);
    assert_eq!(shown(&shot, &h, 200, 50), [0, 0, 0]);
}

/// A brush stroke below a Multiply layer, zoomed out to 50 % (composited
/// at the level reduced twice): only the painted parts of the tiles are
/// composited again, from the snapshots of the layers below, and they
/// match the result.
#[test]
fn stroke_under_a_blend_layer_at_half_size() {
    let mut h = app_with(multiply_doc(), Tool::Brush, 2048);
    (h.state_mut().zoom, h.state_mut().brush_size) = (0.5, 30.0);
    h.state_mut().primary = Color32::from_rgb(0, 0, 255);
    h.run_steps(2);
    let view = h.state().last_view.unwrap();
    let (from, to) = (pos2(60.0, 150.0), pos2(340.0, 160.0));
    h.hover_at(view.to_screen(from));
    h.run_steps(1);
    h.drag_at(view.to_screen(from));
    h.run_steps(1);
    for i in 1..=8 {
        h.hover_at(view.to_screen(from.lerp(to, i as f32 / 8.0)));
        // The frame after shows the dabs of this one.
        h.run_steps(2);
        let shot = h.render().unwrap();
        let flat = h.state().doc.as_ref().unwrap().flatten();
        // Behind the pointer (away from the brush outline), and outside.
        let behind = (60 + 35 * i - 40).max(60);
        for (x, y) in [(60, 150), (behind, 152), (100, 100), (300, 250)] {
            // A pixel shown is the average of a block of 2 × 2.
            let (x0, y0) = (x & !1, y & !1);
            let block =
                [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(dx, dy)| flat.get_pixel(x0 + dx, y0 + dy).0);
            let [r, g, b] =
                [0, 1, 2].map(|c| (block.iter().map(|p| p[c] as u32).sum::<u32>() / 4) as u8);
            let at = shown(&shot, &h, x, y);
            assert!(
                near(at, [r, g, b]),
                "frame {i}: {x}, {y}: {at:?} vs {:?}",
                [r, g, b]
            );
        }
    }
}

/// Zooming out across levels on a large image: the next frame still shows
/// the image (the former tiles, scaled) while the new level is prepared,
/// then the new level everywhere.
#[test]
fn zooming_across_levels_never_shows_an_empty_canvas() {
    let color = [40, 120, 200, 255];
    let mut h = app_with(
        Document::new(solid(2600, 2000, color), None),
        Tool::Hand,
        2048,
    );
    let view = h.state().last_view.unwrap();
    let rect = h.ctx.content_rect();
    let middle = view.to_image(rect.center());
    h.state_mut().zoom = 0.2;
    h.run_steps(1);
    let shot = h.render().unwrap();
    let (x, y) = (middle.x as u32, middle.y as u32);
    assert!(near(shown(&shot, &h, x, y), [40, 120, 200]), "at once");
    let start = std::time::Instant::now();
    loop {
        h.run_steps(1);
        let shot = h.render().unwrap();
        if [(10, 10), (2590, 1990), (x, y)]
            .iter()
            .all(|&(x, y)| near(shown(&shot, &h, x, y), [40, 120, 200]))
        {
            break;
        }
        assert!(start.elapsed().as_secs() < 60, "the new level never came");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Paints `color` over 30 × 30 pixels at (x, y) of the active layer, as a
/// brush stroke tells the view, then draws a frame.
fn dab(h: &mut Harness<'static, AtelierApp>, (x, y): (u32, u32), color: [u8; 4]) {
    let app = h.state_mut();
    let layer = app.doc.as_mut().unwrap().active_layer_mut();
    let pixels = layer.pixels_mut();
    for py in y..y + 30 {
        for px in x..x + 30 {
            pixels.put_pixel(px, py, Rgba(color));
        }
    }
    let r = crate::paint::DirtyRect { x, y, w: 30, h: 30 };
    app.stroke_dirty = Some(app.stroke_dirty.map_or(r, |d| d.union(r)));
    h.run_steps(1);
}

/// Runs frames until the view's reductions are made.
fn until_reduced(h: &mut Harness<'static, AtelierApp>) {
    let start = std::time::Instant::now();
    while h.state().level_pending() {
        assert!(start.elapsed().as_secs() < 60, "the level never came");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
    h.run_steps(1);
}

fn same_reductions(h: &mut Harness<'static, AtelierApp>, what: &str) {
    let (kept, fresh) = h.state_mut().reductions_against_fresh().expect("made");
    let differ = kept
        .pixels()
        .zip(fresh.pixels())
        .filter(|(a, b)| a.0.iter().zip(b.0).any(|(a, b)| a.abs_diff(b) > 2))
        .count();
    assert_eq!(differ, 0, "{what}: pixels differ from fresh reductions");
}

/// Paint where the view composited nothing (out of view) still reaches
/// its reduced copies of the layers: a stroke never keeps a gap there,
/// even once in view.
#[test]
fn paint_out_of_view_reaches_the_reduced_copies() {
    let mut h = app_with(
        Document::new(solid(4000, 1600, [255, 255, 255, 255]), None),
        Tool::Brush,
        2048,
    );
    h.state_mut().zoom = 0.5;
    h.run_steps(1);
    until_reduced(&mut h);
    let view = h.state().last_view.unwrap();
    let window = h.ctx.content_rect();
    let out = [(100, 800), (3870, 800)]
        .into_iter()
        .find(|&(x, y)| !window.contains(view.to_screen(pos2(x as f32, y as f32))))
        .expect("part of the canvas out of view");
    dab(&mut h, out, [200, 0, 0, 255]);
    // Then in view: the reductions are redone in part.
    dab(&mut h, (2000, 800), [0, 0, 200, 255]);
    same_reductions(&mut h, "after paint out of view");
}

/// Paint while a new level's reductions are made on a worker thread also
/// reaches them.
#[test]
fn paint_during_a_level_change_reaches_the_new_reductions() {
    let mut doc = Document::new(solid(4000, 3000, [255, 255, 255, 255]), None);
    for k in 0..3 {
        doc.add_layer(format!("l{k}"), solid(4000, 3000, [0, 90, 0, 40]), (0, 0));
    }
    let mut h = app_with(doc, Tool::Brush, 2048);
    h.state_mut().zoom = 0.4;
    h.run_steps(1);
    until_reduced(&mut h);
    h.state_mut().zoom = 0.2;
    h.run_steps(1);
    let meanwhile = h.state().level_pending();
    dab(&mut h, (2000, 1500), [200, 0, 0, 255]);
    dab(&mut h, (2100, 1500), [200, 0, 0, 255]);
    until_reduced(&mut h);
    dab(&mut h, (1000, 1000), [0, 0, 200, 255]);
    same_reductions(&mut h, "after paint during a level change");
    assert!(meanwhile, "the level was made on a worker thread");
}

/// A change of the whole view (a layer's opacity) is never shown as a
/// patchwork of tiles from before and after it.
#[test]
fn a_change_of_the_whole_view_shows_at_once() {
    let mut doc = Document::new(solid(3000, 2000, [255, 255, 255, 255]), None);
    doc.add_layer("red".into(), solid(3000, 2000, [255, 0, 0, 255]), (0, 0));
    for k in 0..12 {
        doc.add_layer(format!("l{k}"), solid(3000, 2000, [0, 0, 255, 8]), (0, 0));
    }
    let mut h = app_with(doc, Tool::Hand, 2048);
    h.state_mut().zoom = 0.5;
    h.run_steps(1);
    until_reduced(&mut h);
    for _ in 0..20 {
        h.run_steps(1);
    }
    let view = h.state().last_view.unwrap();
    let window = h.ctx.content_rect().shrink(80.0);
    // A point in each tile shown.
    let points: Vec<(u32, u32)> = (0..3000)
        .step_by(512)
        .flat_map(|x| (0..2000).step_by(512).map(move |y| (x + 256, y + 256)))
        .filter(|&(x, y)| window.contains(view.to_screen(pos2(x as f32, y as f32))))
        .collect();
    let shot = h.render().unwrap();
    // Those where the canvas shows (not under a panel).
    let flat = h.state().doc.as_ref().unwrap().flatten();
    let points: Vec<(u32, u32)> = points
        .into_iter()
        .filter(|&(x, y)| {
            let [r, g, b, _] = flat.get_pixel(x, y).0;
            near(shown(&shot, &h, x, y), [r, g, b])
        })
        .collect();
    assert!(points.len() >= 4, "{points:?}");
    let before: Vec<[u8; 3]> = points
        .iter()
        .map(|&(x, y)| shown(&shot, &h, x, y))
        .collect();
    h.state_mut().doc.as_mut().unwrap().layers[1].opacity = 0.0;
    // A frame composites a few tiles only (a slow computer).
    super::render::TEST_TILE_BUDGET.set(Some(std::time::Duration::ZERO));
    let mut seen_generation = false;
    let start = std::time::Instant::now();
    loop {
        assert!(start.elapsed().as_secs() < 60, "the change never showed");
        h.run_steps(1);
        let shot = h.render().unwrap();
        let now: Vec<bool> = points
            .iter()
            .zip(&before)
            .map(|(&(x, y), b)| near(shown(&shot, &h, x, y), *b))
            .collect();
        assert!(
            now.iter().all(|&same| same) || now.iter().all(|&same| !same),
            "a patchwork: {now:?}"
        );
        seen_generation |= h.state().generation_pending();
        if !now[0] && !h.state().generation_pending() {
            break;
        }
    }
    super::render::TEST_TILE_BUDGET.set(None);
    assert!(seen_generation, "composited over several frames");
}

/// The histogram of what a Levels layer adjusts is made on a worker
/// thread, and not again when only the Levels layer itself changes.
#[test]
fn a_levels_layer_histogram_is_made_once_off_the_ui_thread() {
    use crate::pixel_ops::Adjustment;
    let mut doc = Document::new(solid(400, 300, [200, 50, 50, 255]), None);
    doc.add_adjustment_layer(Adjustment::Levels(Default::default()), None);
    let mut h = app_with(doc, Tool::Hand, 2048);
    let ctx = h.ctx.clone();
    let levels = Adjustment::Levels(Default::default());
    let ready = |h: &mut Harness<'static, AtelierApp>| {
        let start = std::time::Instant::now();
        loop {
            let found = h.state_mut().below_histogram(&ctx, &levels);
            if let Some(found) = found
                && !h.state().below_histogram.busy()
            {
                return found;
            }
            assert!(start.elapsed().as_secs() < 60, "no histogram");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    let first = ready(&mut h);
    assert_eq!(first[1][200], 400 * 300, "red of the layer below");
    // The Levels layer's own settings: the same histogram, at once.
    if let LayerKind::Adjustment(Adjustment::Levels(l)) =
        &mut h.state_mut().doc.as_mut().unwrap().active_layer_mut().kind
    {
        l.rgb.gamma = 1.6;
    }
    assert_eq!(h.state_mut().below_histogram(&ctx, &levels), Some(first));
    assert!(!h.state().below_histogram.busy());
    // The layer below changed: made again.
    let doc = h.state_mut().doc.as_mut().unwrap();
    doc.layers[0]
        .pixels_mut()
        .put_pixel(0, 0, Rgba([10, 10, 10, 255]));
    let _ = h.state_mut().below_histogram(&ctx, &levels);
    let next = ready(&mut h);
    assert_eq!(next[1][200], 400 * 300 - 1);
}

/// Opening the Channels panel on a large image makes its first
/// reductions on a worker thread; the thumbnails come once they are made.
#[test]
fn channels_thumbnails_are_first_reduced_off_the_ui_thread() {
    let mut doc = Document::new(solid(4000, 3000, [255, 255, 255, 255]), None);
    doc.add_layer("a".into(), solid(4000, 3000, [0, 90, 0, 120]), (0, 0));
    let mut h = app_with(doc, Tool::Hand, 2048);
    h.state_mut().channels.open = true;
    h.run_steps(1);
    assert_eq!(h.state().channel_thumbs_state(), (false, true));
    let start = std::time::Instant::now();
    while h.state().channel_thumbs_state() != (true, false) {
        assert!(start.elapsed().as_secs() < 60, "no thumbnails");
        std::thread::sleep(std::time::Duration::from_millis(5));
        h.run_steps(1);
    }
}

/// A new document's view (opened, or a tab whose view wasn't kept) makes
/// its reductions on a worker thread rather than freezing the window.
#[test]
fn a_new_documents_view_is_reduced_off_the_ui_thread() {
    let mut h = app_with(
        Document::new(solid(400, 300, [255, 255, 255, 255]), None),
        Tool::Hand,
        2048,
    );
    let mut big = Document::new(solid(5000, 4000, [20, 200, 20, 255]), None);
    big.add_layer("a".into(), solid(5000, 4000, [0, 0, 90, 60]), (0, 0));
    h.state_mut().add_tab(big);
    h.state_mut().zoom = 0.1;
    h.state_mut().fit_pending = None;
    h.run_steps(1);
    assert!(h.state().level_pending(), "made on a worker thread");
    until_reduced(&mut h);
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let middle = view.to_image(h.ctx.content_rect().center());
    let at = shown(&shot, &h, middle.x as u32, middle.y as u32);
    assert!(at[1] > 100 && at[0] < 60, "the image shows: {at:?}");
}

/// Panning while a change of the whole view is composited over several
/// frames: the tiles coming into view show at once, never holes.
#[test]
fn tiles_panned_into_view_during_a_generation_show_at_once() {
    let mut doc = Document::new(solid(3000, 2000, [255, 255, 255, 255]), None);
    doc.add_layer("red".into(), solid(3000, 2000, [255, 0, 0, 255]), (0, 0));
    for k in 0..12 {
        doc.add_layer(format!("l{k}"), solid(3000, 2000, [0, 0, 255, 8]), (0, 0));
    }
    let mut h = app_with(doc, Tool::Hand, 2048);
    h.state_mut().zoom = 0.5;
    h.run_steps(1);
    until_reduced(&mut h);
    for _ in 0..20 {
        h.run_steps(1);
    }
    h.state_mut().doc.as_mut().unwrap().layers[1].opacity = 0.0;
    // A tile per frame (a slow computer).
    super::render::TEST_TILE_BUDGET.set(Some(std::time::Duration::ZERO));
    super::render::TEST_THREADS.set(Some(1));
    h.run_steps(1);
    let started = h.state().generation_pending();
    let mut panned = 0;
    while h.state().generation_pending() && panned < 6 {
        h.state_mut().pan -= vec2(150.0, 100.0);
        h.run_steps(1);
        panned += 1;
        let holes = super::render::TEST_HOLES.with(std::cell::Cell::get);
        assert_eq!(holes, 0, "empty tiles after pan {panned}");
    }
    super::render::TEST_TILE_BUDGET.set(None);
    super::render::TEST_THREADS.set(None);
    assert!(started && panned >= 3, "a generation ran ({panned} frames)");
    // Once done, the view shows the document as it is.
    h.state_mut().pan = Vec2::ZERO;
    let start = std::time::Instant::now();
    while h.state().generation_pending() {
        assert!(start.elapsed().as_secs() < 60, "never done");
        h.run_steps(1);
    }
    for _ in 0..3 {
        h.run_steps(1);
    }
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let p = view.to_image(h.ctx.content_rect().center());
    let flat = h.state().doc.as_ref().unwrap().flatten();
    let [r, g, b, _] = flat.get_pixel(p.x as u32, p.y as u32).0;
    assert!(near(shown(&shot, &h, p.x as u32, p.y as u32), [r, g, b]));
}

/// A pan during a generation uncovering many tiles: those nearest the
/// focus are composited within the frame's budget, the others show
/// coarsely meanwhile (never holes), and come over the next frames.
#[test]
fn a_pan_during_a_generation_keeps_to_the_budget() {
    let mut doc = Document::new(solid(3000, 2000, [255, 255, 255, 255]), None);
    doc.add_layer("red".into(), solid(3000, 2000, [255, 0, 0, 255]), (0, 0));
    for k in 0..12 {
        doc.add_layer(format!("l{k}"), solid(3000, 2000, [0, 0, 255, 8]), (0, 0));
    }
    let mut h = app_with(doc, Tool::Hand, 2048);
    h.state_mut().zoom = 1.0;
    h.run_steps(1);
    until_reduced(&mut h);
    for _ in 0..20 {
        h.run_steps(1);
    }
    h.state_mut().doc.as_mut().unwrap().layers[1].opacity = 0.0;
    super::render::TEST_TILE_BUDGET.set(Some(std::time::Duration::ZERO));
    super::render::TEST_THREADS.set(Some(1));
    h.run_steps(1);
    let started = h.state().generation_pending();
    // Far, at once: many tiles uncovered.
    super::render::TEST_UNCOVERED.set((0, 0));
    h.state_mut().pan -= vec2(1000.0, 600.0);
    h.run_steps(1);
    let (uncovered, made) = super::render::TEST_UNCOVERED.get();
    let holes = super::render::TEST_HOLES.with(std::cell::Cell::get);
    let after = h.state().view_tiles();
    for _ in 0..4 {
        h.run_steps(1);
        assert_eq!(super::render::TEST_HOLES.with(std::cell::Cell::get), 0);
    }
    let later = h.state().view_tiles();
    super::render::TEST_TILE_BUDGET.set(None);
    super::render::TEST_THREADS.set(None);
    assert!(started, "a generation ran");
    assert!(uncovered >= 4 && made == 1, "{made} of {uncovered} tiles in one frame");
    assert_eq!(holes, 0, "empty tiles");
    assert!(later >= after + 3, "the uncovered tiles come: {after} → {later}");
}

/// A large document whose masks are feathered, shown for the first time:
/// neither its coarse view nor the view once reduced feathers them at full
/// size on the UI thread (the level job's feathered images are shared).
#[test]
fn a_new_documents_feathered_masks_are_not_feathered_on_the_ui_thread() {
    let mut h = app_with(
        Document::new(solid(200, 100, [0, 0, 0, 255]), None),
        Tool::Hand,
        2048,
    );
    h.state_mut().zoom = 0.3;
    h.run_steps(2);
    let mut doc = Document::new(solid(3000, 2000, [255, 255, 255, 255]), None);
    for k in 0..3 {
        doc.add_layer(format!("l{k}"), solid(3000, 2000, [0, 160, 0, 255]), (0, 0));
    }
    let mut mask = image::GrayImage::from_pixel(3000, 2000, image::Luma([0]));
    for y in 400..1600 {
        for x in 500..2500 {
            mask.put_pixel(x, y, image::Luma([255]));
        }
    }
    let mut mask = crate::document::Mask::new(mask, (0, 0), 0, (0, 0));
    mask.feather = 12.0;
    doc.layers[3].mask = Some(mask);
    h.state_mut().doc = Some(doc);
    h.state_mut().pan = Vec2::ZERO;
    crate::document::TEST_BIG_FEATHERS.set(0);
    h.run_steps(1);
    assert!(h.state().level_pending(), "reduced on a worker thread");
    assert_eq!(crate::document::TEST_BIG_FEATHERS.get(), 0, "coarse view");
    until_reduced(&mut h);
    for _ in 0..5 {
        h.run_steps(1);
    }
    assert_eq!(crate::document::TEST_BIG_FEATHERS.get(), 0, "once reduced");
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let p = view.to_image(h.ctx.content_rect().center());
    if (600.0..2400.0).contains(&p.x) && (500.0..1500.0).contains(&p.y) {
        assert!(near(shown(&shot, &h, p.x as u32, p.y as u32), [0, 160, 0]));
    }
}

/// A large document shown for the first time: a coarse view of it while
/// its reductions are made, not an empty canvas.
#[test]
fn a_new_document_shows_coarsely_while_reduced() {
    let mut h = app_with(
        Document::new(solid(200, 100, [0, 0, 0, 255]), None),
        Tool::Hand,
        2048,
    );
    h.state_mut().zoom = 0.3;
    h.run_steps(2);
    let mut doc = Document::new(solid(3000, 2000, [255, 255, 255, 255]), None);
    for k in 0..3 {
        doc.add_layer(format!("l{k}"), solid(3000, 2000, [0, 160, 0, 255]), (0, 0));
    }
    h.state_mut().doc = Some(doc);
    h.state_mut().pan = Vec2::ZERO;
    h.run_steps(1);
    assert!(h.state().level_pending(), "reduced on a worker thread");
    assert_eq!(
        super::render::TEST_HOLES.with(std::cell::Cell::get),
        0,
        "no empty tiles"
    );
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let p = view.to_image(h.ctx.content_rect().center());
    if (0.0..3000.0).contains(&p.x) && (0.0..2000.0).contains(&p.y) {
        assert!(near(shown(&shot, &h, p.x as u32, p.y as u32), [0, 160, 0]));
    }
    until_reduced(&mut h);
}

/// The view of a tab left during a generation keeps none (its reductions
/// would stay pinned), and shows the change once back.
#[test]
fn a_tab_left_during_a_generation_drops_it() {
    let mut doc = Document::new(solid(3000, 2000, [255, 255, 255, 255]), None);
    doc.add_layer("red".into(), solid(3000, 2000, [255, 0, 0, 255]), (0, 0));
    for k in 0..12 {
        doc.add_layer(format!("l{k}"), solid(3000, 2000, [0, 0, 255, 8]), (0, 0));
    }
    let mut h = app_with(doc, Tool::Hand, 2048);
    h.state_mut().zoom = 0.5;
    h.run_steps(1);
    until_reduced(&mut h);
    for _ in 0..20 {
        h.run_steps(1);
    }
    h.state_mut().doc.as_mut().unwrap().layers[1].opacity = 0.0;
    super::render::TEST_TILE_BUDGET.set(Some(std::time::Duration::ZERO));
    h.run_steps(1);
    super::render::TEST_TILE_BUDGET.set(None);
    assert!(h.state().generation_pending(), "a generation ran");
    let other = Document::new(solid(300, 200, [0, 0, 0, 255]), None);
    let first = h.state_mut().doc.replace(other);
    h.run_steps(1);
    assert_eq!(h.state().kept_generations(), 0);
    h.state_mut().doc = first;
    for _ in 0..10 {
        h.run_steps(1);
    }
    let shot = h.render().unwrap();
    let view = h.state().last_view.unwrap();
    let p = view.to_image(h.ctx.content_rect().center());
    let flat = h.state().doc.as_ref().unwrap().flatten();
    let [r, g, b, _] = flat.get_pixel(p.x as u32, p.y as u32).0;
    assert!(
        near(shown(&shot, &h, p.x as u32, p.y as u32), [r, g, b]),
        "the change shows"
    );
}
