//! Shape layers and the paths of the Paths panel: a shape's image is
//! rendered again at each change of its shape, which follows the layer's
//! moves and the canvas' geometry (rotation, flip, Image Size, Free
//! Transform) as a vector.

use super::*;
use crate::mask_ops::Affine;
use crate::vector::pt;
use crate::vector::raster::pixel_box;

/// A path saved in the Paths panel.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedPath {
    pub name: String,
    pub path: VectorPath,
}

/// One of the document's paths: the Work Path, a saved path, or a shape
/// layer's (by its id).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathRef {
    Work,
    Saved(usize),
    Layer(u64),
}

impl Layer {
    /// A text, a shape or a smart object: its pixels are the rendering of
    /// what stays editable.
    pub fn is_live(&self) -> bool {
        self.text.is_some() || self.shape.is_some() || self.smart.is_some()
    }

    /// Makes it show `shape` (in its coordinates), rendered within `limit`
    /// (canvas coordinates) when the whole of it would be too large: the
    /// image covers what the shape shows, the layer's position following
    /// so that the shape stays in place.
    pub fn set_shape(&mut self, mut shape: Shape, limit: PixelRect) {
        let mut area = shape.extent().map_or(
            PixelRect {
                x: 0,
                y: 0,
                w: 0,
                h: 0,
            },
            pixel_box,
        );
        if area.w as u64 * area.h as u64 > MAX_LAYER_PIXELS {
            let local = limit.translate(-self.offset.0, -self.offset.1);
            area = intersect(area, local).unwrap_or(PixelRect { w: 0, h: 0, ..area });
        }
        shape.translate(pt(-area.x as f32, -area.y as f32));
        self.offset = (self.offset.0 + area.x, self.offset.1 + area.y);
        let image = shape.render(PixelRect { x: 0, y: 0, ..area });
        self.image = Arc::new(image);
        self.rev = next_rev();
        self.shape = Some(Box::new(shape));
    }

    /// Its shape mapped by `a` (canvas coordinates, the layer having been
    /// at `old` before), rendered again. `false` if it has none.
    pub fn map_shape(&mut self, old: (i64, i64), a: &Affine, limit: PixelRect) -> bool {
        let Some(mut shape) = self.shape.take() else {
            return false;
        };
        shape.translate(pt(old.0 as f32, old.1 as f32));
        shape.transform(a);
        self.offset = (0, 0);
        self.set_shape(*shape, limit);
        true
    }
}

/// Sine and cosine of a clockwise turn of `degrees`, exact for the
/// quarter turns (a shape turned by 90° keeps its edges on the pixels).
pub(super) fn sin_cos(degrees: f32) -> (f32, f32) {
    let exact = |v: f32| {
        if (v - v.round()).abs() < 1e-6 {
            v.round()
        } else {
            v
        }
    };
    let (s, c) = degrees.to_radians().sin_cos();
    (exact(s), exact(c))
}

/// The map of Free Transform: `src` scaled to the frame of `place` from
/// its corner, then turned around the frame's center.
pub(super) fn placement_map(src: PixelRect, place: Placement) -> Affine {
    let Placement { at, size, angle } = place;
    let (kx, ky) = (
        size.0 as f32 / src.w.max(1) as f32,
        size.1 as f32 / src.h.max(1) as f32,
    );
    let (s, c) = sin_cos(angle);
    let (tx, ty) = (
        at.0 as f32 - src.x as f32 * kx,
        at.1 as f32 - src.y as f32 * ky,
    );
    let (cx, cy) = (
        at.0 as f32 + size.0 as f32 / 2.0,
        at.1 as f32 + size.1 as f32 / 2.0,
    );
    // Turn around the center: q → C + R (q − C), after the scaling.
    let (rx, ry) = (cx - (c * cx - s * cy), cy - (s * cx + c * cy));
    Affine([
        c * kx,
        -s * ky,
        c * tx - s * ty + rx,
        s * kx,
        c * ky,
        s * tx + c * ty + ry,
    ])
}

impl Layer {
    /// Its shape through `f` (canvas coordinates, the layer having been
    /// at `old` before), rendered again; `false` if it has none, or if `f`
    /// fails on one of its points (beyond a perspective's horizon).
    pub fn map_shape_with(
        &mut self,
        old: (i64, i64),
        f: impl Fn(crate::vector::Pt) -> Option<crate::vector::Pt>,
        limit: PixelRect,
    ) -> bool {
        let o = pt(old.0 as f32, old.1 as f32);
        let Some(shape) = self.shape.as_ref().and_then(|s| s.map_points(|p| f(p + o))) else {
            return false;
        };
        self.offset = (0, 0);
        self.set_shape(shape, limit);
        true
    }
}

impl Document {
    /// Index of the layer with id `id`.
    pub fn layer_index(&self, id: u64) -> Option<usize> {
        self.layers.iter().position(|l| l.id == id)
    }

    /// Area a shape layer's image may be limited to (the canvas, with
    /// room around it).
    pub(super) fn shape_limit(&self) -> PixelRect {
        let (w, h) = (self.canvas.0 as i64, self.canvas.1 as i64);
        PixelRect {
            x: -w,
            y: -h,
            w: self.canvas.0 * 3,
            h: self.canvas.1 * 3,
        }
    }

    /// New shape layer above the active one, showing `shape` (canvas
    /// coordinates), named after its kind ("Rectangle 1"…), as the history
    /// step `step`.
    pub fn add_shape_layer(&mut self, step: String, kind: &str, shape: Shape) {
        let name = self.unused_name(|n| t!("vector-layer-name", kind = kind, number = n));
        self.checkpoint(step);
        let mut layer = self.make_layer(name, RgbaImage::new(0, 0), (0, 0));
        layer.set_shape(shape, self.shape_limit());
        self.insert_layer(layer);
    }

    /// Replaces the shape of layer `i` (in its coordinates), without a
    /// history step.
    pub fn set_shape(&mut self, i: usize, shape: Shape) {
        let limit = self.shape_limit();
        self.layers[i].anchor_styles();
        self.layers[i].set_shape(shape, limit);
        self.mark_modified();
    }

    /// Path `r`, in canvas coordinates.
    pub fn path(&self, r: PathRef) -> Option<VectorPath> {
        match r {
            PathRef::Work => self.work_path.clone(),
            PathRef::Saved(i) => self.paths.get(i).map(|p| p.path.clone()),
            PathRef::Layer(id) => {
                let l = &self.layers[self.layer_index(id)?];
                let (x, y) = l.offset;
                Some(l.shape.as_ref()?.path.translated(pt(x as f32, y as f32)))
            }
        }
    }

    /// Replaces path `r` (canvas coordinates), without a history step. A
    /// shape edited this way is no longer a live shape.
    pub fn set_path(&mut self, r: PathRef, path: VectorPath) {
        match r {
            PathRef::Work => self.work_path = Some(path),
            PathRef::Saved(i) => {
                if let Some(p) = self.paths.get_mut(i) {
                    p.path = path;
                }
            }
            PathRef::Layer(id) => {
                let Some(i) = self.layer_index(id) else {
                    return;
                };
                let l = &self.layers[i];
                let Some(mut shape) = l.shape.as_deref().cloned() else {
                    return;
                };
                let (x, y) = l.offset;
                shape.path = path.translated(pt(-x as f32, -y as f32));
                shape.live = None;
                self.set_shape(i, shape);
            }
        }
        self.mark_modified();
    }

    /// Moves path `r` by `d` (a live shape stays one), without a history
    /// step.
    pub fn move_path(&mut self, r: PathRef, d: crate::vector::Pt) {
        if let PathRef::Layer(id) = r {
            let Some(i) = self.layer_index(id) else {
                return;
            };
            let Some(mut shape) = self.layers[i].shape.as_deref().cloned() else {
                return;
            };
            // By whole pixels, a shape rendered whole: the same image
            // elsewhere (its styles stay drawn).
            let whole = d.x.fract() == 0.0 && d.y.fract() == 0.0;
            let fits = shape
                .extent()
                .map(pixel_box)
                .is_some_and(|b| u64::from(b.w) * u64::from(b.h) <= MAX_LAYER_PIXELS);
            if whole && fits {
                let l = &mut self.layers[i];
                l.offset = (l.offset.0 + d.x as i64, l.offset.1 + d.y as i64);
                self.mark_modified();
                return;
            }
            shape.translate(d);
            self.set_shape(i, shape);
            self.layers[i].carry_styles(d);
        } else if let Some(mut path) = self.path(r) {
            path.translate(d);
            self.set_path(r, path);
        }
    }

    /// The paths follow a map of the canvas.
    pub(super) fn map_paths(&mut self, a: &Affine) {
        for p in self
            .paths
            .iter_mut()
            .map(|p| &mut p.path)
            .chain(&mut self.work_path)
        {
            p.transform(a);
        }
    }

    /// Layer › Rasterize: the active text or shape becomes plain pixels.
    pub fn rasterize_active(&mut self) -> bool {
        if !self.active_layer().is_live() {
            return false;
        }
        self.checkpoint(t!("vector-hist-rasterize"));
        let layer = self.active_layer_mut();
        (layer.text, layer.shape, layer.smart) = (None, None, None);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::{Bounds, Live, Paint, ShapeKind, ShapeStroke};
    use image::Rgba;

    fn rect_shape(x0: f32, y0: f32, x1: f32, y1: f32) -> Shape {
        let live = Live::new(ShapeKind::Rectangle, Bounds::of(pt(x0, y0), pt(x1, y1)));
        Shape::live(live, Paint::Color([255, 0, 0]), ShapeStroke::default())
    }

    /// A shape moved whole by whole pixels keeps its image (and the
    /// styles drawn for it), only its place changes; otherwise it is
    /// rendered again, the former styles staying where they were in the
    /// canvas until the new ones are drawn.
    #[test]
    fn a_shape_moved_by_whole_pixels_keeps_its_image() {
        let mut doc = Document::new(RgbaImage::from_pixel(80, 60, Rgba([255; 4])), None);
        doc.add_shape_layer(
            "Rectangle".into(),
            "Rectangle",
            rect_shape(10.0, 5.0, 30.0, 25.0),
        );
        let fx = &mut doc.active_layer_mut().effects;
        fx.stroke.enabled = true;
        fx.stroke.width = 3.0;
        let id = doc.active_layer().id;
        let drawn = doc.active_layer().styles(false).expect("styles");
        let (rev, offset) = (doc.active_layer().rev(), doc.active_layer().offset);
        doc.move_path(PathRef::Layer(id), pt(7.0, -3.0));
        let layer = doc.active_layer();
        assert_eq!(
            (layer.rev(), layer.offset),
            (rev, (offset.0 + 7, offset.1 - 3))
        );
        let kept = layer.styles_cached().expect("styles");
        assert_eq!(kept.version, drawn.version);
        assert_eq!(
            kept.exterior[0].offset, drawn.exterior[0].offset,
            "with the layer"
        );
        let path = doc.path(PathRef::Layer(id)).unwrap();
        let b = path.bounds().unwrap();
        assert_eq!((b.min.x, b.min.y), (17.0, 2.0));

        // Moved by a fraction: rendered again; the former styles move with
        // it (to the nearest pixel) until drawn again.
        let before = doc.active_layer().offset;
        let canvas = |l: &Layer, s: &crate::compose::styles::Styles| {
            (
                l.offset.0 + s.exterior[0].offset.0,
                l.offset.1 + s.exterior[0].offset.1,
            )
        };
        let at = canvas(doc.active_layer(), &kept);
        doc.move_path(PathRef::Layer(id), pt(-20.5, 0.0));
        let layer = doc.active_layer();
        assert_ne!(layer.rev(), rev);
        assert_ne!(layer.offset, before);
        let former = layer.styles_cached().expect("styles");
        assert_eq!(canvas(layer, &former), (at.0 - 21, at.1));
    }

    /// A styled shape dragged by fractions of a pixel (zoomed in), its
    /// styles held: they follow it, not left where it started.
    #[test]
    fn held_styles_follow_a_shape_dragged_by_fractions() {
        let mut doc = Document::new(RgbaImage::from_pixel(300, 60, Rgba([255; 4])), None);
        doc.add_shape_layer(
            "Rectangle".into(),
            "Rectangle",
            rect_shape(10.0, 5.0, 30.0, 25.0),
        );
        let fx = &mut doc.active_layer_mut().effects;
        fx.stroke.enabled = true;
        fx.stroke.width = 3.0;
        let id = doc.active_layer().id;
        let canvas = |doc: &Document| {
            let l = doc.active_layer();
            let s = l.styles_shown(true, true).expect("styles");
            (
                l.offset.0 + s.exterior[0].offset.0,
                l.offset.1 + s.exterior[0].offset.1,
            )
        };
        doc.active_layer().styles(false).expect("styles");
        let start = canvas(&doc);
        doc.hold_effects(true);
        for _ in 0..20 {
            doc.move_path(PathRef::Layer(id), pt(7.5, 0.25));
        }
        assert_eq!(canvas(&doc), (start.0 + 150, start.1 + 5));
        doc.hold_effects(false);
    }

    #[test]
    fn shape_layers_render_and_follow_their_layer() {
        let mut doc = Document::new(RgbaImage::from_pixel(40, 30, Rgba([255; 4])), None);
        doc.add_shape_layer(
            "Rectangle".into(),
            "Rectangle",
            rect_shape(10.0, 5.0, 20.0, 15.0),
        );
        let layer = doc.active_layer();
        assert!(layer.shape.is_some() && layer.has_pixels());
        assert_eq!(
            layer.content_bounds(),
            Some(PixelRect {
                x: 10,
                y: 5,
                w: 10,
                h: 10
            })
        );
        assert_eq!(doc.flatten().get_pixel(12, 7).0, [255, 0, 0, 255]);
        // Its path in canvas coordinates, wherever the layer moves.
        let id = layer.id;
        doc.active_layer_mut().offset.0 += 5;
        let b = doc.path(PathRef::Layer(id)).unwrap().bounds().unwrap();
        assert_eq!((b.min, b.max), (pt(15.0, 5.0), pt(25.0, 15.0)));
        // Moved as a path: still live, rendered again.
        doc.move_path(PathRef::Layer(id), pt(-15.0, 0.0));
        assert!(doc.active_layer().shape.as_ref().unwrap().live.is_some());
        assert_eq!(doc.active_layer().content_bounds().unwrap().x, 0);
        // Edited as a path: no longer live.
        let mut p = doc.path(PathRef::Layer(id)).unwrap();
        p.subpaths[0].anchors[2].p = pt(30.0, 30.0);
        doc.set_path(PathRef::Layer(id), p);
        assert!(doc.active_layer().shape.as_ref().unwrap().live.is_none());
        assert_eq!(doc.active_layer().content_bounds().unwrap().right(), 30);
        // Painting makes it pixels; so does Rasterize, as a step.
        let mut doc2 = Document::new(RgbaImage::new(10, 10), None);
        doc2.add_shape_layer("R".into(), "R", rect_shape(0.0, 0.0, 5.0, 5.0));
        doc2.active_layer_mut().pixels_mut();
        assert!(doc2.active_layer().shape.is_none());
        assert!(doc.rasterize_active());
        assert!(!doc.active_layer().is_live());
        assert!(doc.undo());
        assert!(doc.active_layer().shape.is_some());
    }

    #[test]
    fn shapes_stay_vectors_through_the_canvas_geometry() {
        let mut doc = Document::new(RgbaImage::from_pixel(40, 20, Rgba([255; 4])), None);
        doc.add_shape_layer("R".into(), "R", rect_shape(2.0, 4.0, 12.0, 8.0));
        doc.rotate_canvas(90.0);
        let s = doc.active_layer().shape.as_ref().unwrap();
        assert!(s.live.is_none(), "turned: a path");
        // Turned clockwise: (x, y) → (h − y, x).
        assert_eq!(
            doc.active_layer().content_bounds(),
            Some(PixelRect {
                x: 12,
                y: 2,
                w: 4,
                h: 10
            })
        );
        doc.flip_canvas(true);
        assert_eq!(doc.active_layer().content_bounds().unwrap().x, 4);
        assert!(doc.resize_image(40, 80, FilterType::Triangle));
        let b = doc.active_layer().content_bounds().unwrap();
        assert_eq!((b.w, b.h), (8, 20));
        // Crisp edges after scaling: no resampling of pixels.
        let l = doc.active_layer();
        assert_eq!(l.pixel_at(b.x, b.y), Some([255, 0, 0, 255]));
        assert!(l.shape.is_some());
    }

    #[test]
    fn free_transform_keeps_shapes_vectors() {
        let mut doc = Document::new(RgbaImage::new(100, 100), None);
        doc.add_shape_layer("R".into(), "R", rect_shape(10.0, 10.0, 30.0, 20.0));
        let src = doc.active_layer().content_bounds().unwrap();
        // Twice as large, moved: still a live rectangle, edges crisp.
        let place = Placement::new((40, 40), (40, 20));
        doc.transform_active(src, place, FilterType::Triangle, false);
        let l = doc.active_layer();
        let live = l.shape.as_ref().unwrap().live.unwrap();
        assert_eq!(live.kind, crate::vector::ShapeKind::Rectangle);
        assert_eq!(
            l.content_bounds(),
            Some(PixelRect {
                x: 40,
                y: 40,
                w: 40,
                h: 20
            })
        );
        assert_eq!(l.pixel_at(40, 40), Some([255, 0, 0, 255]));
        // Turned a quarter: a path, the same size.
        let src = l.content_bounds().unwrap();
        let place = Placement {
            angle: 90.0,
            ..Placement::new((40, 40), (40, 20))
        };
        doc.transform_active(src, place, FilterType::Triangle, false);
        let l = doc.active_layer();
        assert!(l.shape.as_ref().unwrap().live.is_none());
        let b = l.content_bounds().unwrap();
        assert_eq!((b.x, b.y, b.w, b.h), (50, 30, 20, 40));
    }

    #[test]
    fn paths_are_in_the_history() {
        let mut doc = Document::new(RgbaImage::new(10, 10), None);
        let path = rect_shape(1.0, 1.0, 4.0, 4.0).path;
        doc.checkpoint("Pen".into());
        doc.set_path(PathRef::Work, path.clone());
        doc.checkpoint("Save Path".into());
        doc.paths.push(SavedPath {
            name: "Path 1".into(),
            path: path.clone(),
        });
        doc.work_path = None;
        assert_eq!(doc.path(PathRef::Saved(0)), Some(path.clone()));
        assert!(doc.undo());
        assert!(doc.paths.is_empty() && doc.work_path.is_some());
        assert!(doc.undo());
        assert!(doc.work_path.is_none());
    }
}
