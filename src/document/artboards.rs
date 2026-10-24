//! Artboards, like Photoshop's: top-level groups that are boards of their
//! own, side by side on a pasteboard. Each has a rectangle and a
//! background; its layers show only within it. The rectangle's corner is
//! the group's offset, so that whatever moves the group (Move tool, arrows,
//! canvas operations) moves the board with it; its size is its own.
//!
//! The canvas of a document with artboards is the box of all of them, kept
//! so by [`Document::fit_artboards`]: rulers, Fit on Screen, crop and
//! selections work on it as on any canvas. Layers outside every artboard
//! are on the pasteboard: the view shows them around the boards.

use std::ops::Range;
use std::path::{Path, PathBuf};

use image::RgbaImage;

use super::{BlendMode, Document, ExportFormat, FileError, Layer, LayerKind, PixelRect};
use crate::compose::{self, FullSize, Options};
use crate::i18n::t;

/// Space left between an artboard and the one added next to it.
pub const ARTBOARD_GAP: i64 = 100;

/// What an artboard shows under its layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ArtboardBackground {
    White,
    Black,
    Transparent,
    /// "Other": a color of its own.
    Custom([u8; 3]),
}

impl ArtboardBackground {
    /// Its color, `None` when transparent.
    pub fn color(self) -> Option<[u8; 3]> {
        match self {
            ArtboardBackground::White => Some([255; 3]),
            ArtboardBackground::Black => Some([0; 3]),
            ArtboardBackground::Transparent => None,
            ArtboardBackground::Custom(c) => Some(c),
        }
    }
}

/// What makes a group an artboard (its position: the group's offset).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Artboard {
    pub size: (u32, u32),
    pub background: ArtboardBackground,
}

/// Export of an artboard, like Artboards to Files.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ArtboardExport {
    /// Its background (else transparent).
    pub background: bool,
    /// What else shows over its area (other artboards, the pasteboard),
    /// rather than its content only.
    pub overlapping: bool,
}

/// Artboards to Files: where, under which names, in which format.
#[derive(Clone, Debug, PartialEq)]
pub struct ArtboardFiles {
    pub folder: PathBuf,
    /// Before each artboard's name.
    pub prefix: String,
    pub format: ExportFormat,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    pub options: ArtboardExport,
    /// Files of the same names replaced (else numbered: both kept).
    pub replace: bool,
}

/// Side of an artboard, where the "+" buttons add the next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Layer {
    /// The artboard's rectangle, if it is one.
    pub fn artboard_rect(&self) -> Option<PixelRect> {
        self.artboard.map(|a| PixelRect {
            x: self.offset.0,
            y: self.offset.1,
            w: a.size.0,
            h: a.size.1,
        })
    }
}

impl Document {
    /// The artboards, from the bottom up.
    pub fn artboards(&self) -> Vec<usize> {
        (0..self.layers.len())
            .filter(|&i| self.layers[i].artboard.is_some())
            .collect()
    }

    pub fn has_artboards(&self) -> bool {
        self.layers.iter().any(|l| l.artboard.is_some())
    }

    /// The artboard layer `i` is in (itself if it is one).
    pub fn artboard_of(&self, i: usize) -> Option<usize> {
        let mut at = Some(i);
        while let Some(j) = at {
            if self.layers[j].artboard.is_some() {
                return Some(j);
            }
            at = self.parent(j);
        }
        None
    }

    /// Topmost visible artboard at canvas point (x, y).
    pub fn artboard_at(&self, x: i64, y: i64) -> Option<usize> {
        self.artboards().into_iter().rev().find(|&i| {
            let l = &self.layers[i];
            l.visible && l.artboard_rect().is_some_and(|r| r.contains(x, y))
        })
    }

    /// Whether (x, y) shows layer `i` as far as artboards go: inside its
    /// artboard, if it is in one.
    pub(super) fn within_artboard(&self, i: usize, x: i64, y: i64) -> bool {
        self.artboard_of(i)
            .and_then(|a| self.layers[a].artboard_rect())
            .is_none_or(|r| r.contains(x, y))
    }

    /// Box of all the artboards.
    pub fn artboards_bounds(&self) -> Option<PixelRect> {
        self.layers
            .iter()
            .filter_map(Layer::artboard_rect)
            .reduce(PixelRect::union)
    }

    /// The canvas becomes the box of the artboards (part of the current
    /// history step). Returns that box, in the former canvas coordinates,
    /// if the canvas changed.
    pub fn fit_artboards(&mut self) -> Option<PixelRect> {
        let r = self.artboards_bounds()?;
        if r == self.full_rect() || r.w == 0 || r.h == 0 {
            return None;
        }
        self.fit_canvas_to(r);
        Some(r)
    }

    /// An empty artboard (a group: Pass Through, open).
    fn artboard_layer(
        &mut self,
        name: String,
        rect: PixelRect,
        background: ArtboardBackground,
    ) -> Layer {
        let mut layer = self.make_layer(name, RgbaImage::new(0, 0), (rect.x, rect.y));
        layer.kind = LayerKind::Group { expanded: true };
        layer.blend = BlendMode::PassThrough;
        layer.artboard = Some(Artboard {
            size: (rect.w.max(1), rect.h.max(1)),
            background,
        });
        layer
    }

    /// "Artboard N", the first number not used yet.
    pub fn new_artboard_name(&self) -> String {
        self.unused_name(|n| t!("artboard-name", number = n))
    }

    /// Index above the top-level block that holds the active layer.
    fn top_level_slot(&self) -> usize {
        let mut top = self.active.min(self.layers.len().saturating_sub(1));
        while let Some(p) = self.parent(top) {
            top = p;
        }
        if self.layers.is_empty() { 0 } else { top + 1 }
    }

    /// New empty artboard (Artboard tool, Layer › New › Artboard), above
    /// the active layer's artboard, as the history step `step`. It becomes
    /// active; the canvas follows the artboards.
    pub fn add_artboard(
        &mut self,
        step: String,
        name: String,
        rect: PixelRect,
        background: ArtboardBackground,
    ) {
        self.checkpoint(step);
        let layer = self.artboard_layer(name, rect, background);
        let at = self.top_level_slot();
        self.layers.insert(at, layer);
        self.active = at;
        self.fit_artboards();
    }

    /// Where an artboard of the same size as artboard `i` goes on its
    /// `side` (the "+" buttons).
    pub fn adjacent_rect(&self, i: usize, side: Side) -> Option<PixelRect> {
        let r = self.layers.get(i)?.artboard_rect()?;
        let (w, h) = (r.w as i64, r.h as i64);
        let (x, y) = match side {
            Side::Left => (r.x - w - ARTBOARD_GAP, r.y),
            Side::Right => (r.right() + ARTBOARD_GAP, r.y),
            Side::Top => (r.x, r.y - h - ARTBOARD_GAP),
            Side::Bottom => (r.x, r.bottom() + ARTBOARD_GAP),
        };
        Some(PixelRect { x, y, ..r })
    }

    /// The "+" button on `side` of artboard `i`: a new empty artboard of
    /// its size and background there, or (`copy`, Alt + click) a copy of
    /// it with its layers. The new one becomes active.
    pub fn add_adjacent_artboard(&mut self, i: usize, side: Side, copy: bool) -> bool {
        let (Some(rect), Some(board)) = (self.adjacent_rect(i, side), self.layers[i].artboard)
        else {
            return false;
        };
        if !copy {
            let name = self.new_artboard_name();
            self.add_artboard(t!("artboard-hist-new"), name, rect, board.background);
            return true;
        }
        self.checkpoint(t!("artboard-hist-duplicate"));
        let r = self.block(i);
        let (dx, dy) = (
            rect.x - self.layers[i].offset.0,
            rect.y - self.layers[i].offset.1,
        );
        let copies: Vec<Layer> = self.layers[r.clone()]
            .iter()
            .map(|l| {
                self.next_id += 1;
                let mut c = Layer {
                    id: self.next_id,
                    ..l.clone()
                };
                c.translate(dx, dy);
                c
            })
            .collect();
        let n = copies.len();
        self.layers.splice(r.end..r.end, copies);
        self.active = r.end + n - 1;
        let top = &mut self.layers[self.active];
        top.name = t!("doc-layer-copy-name", name = top.name.as_str());
        self.fit_artboards();
        true
    }

    /// Moves and resizes artboard `i` (its layers stay where they are),
    /// without a history step: the caller records one.
    pub fn set_artboard_rect(&mut self, i: usize, rect: PixelRect) {
        let layer = &mut self.layers[i];
        if let Some(a) = &mut layer.artboard {
            a.size = (rect.w.max(1), rect.h.max(1));
            layer.offset = (rect.x, rect.y);
        }
    }

    /// Moves artboard `i` with its layers, without a history step.
    pub fn move_artboard(&mut self, i: usize, dx: i64, dy: i64) {
        for j in self.block(i) {
            let l = &mut self.layers[j];
            l.offset = (l.offset.0 + dx, l.offset.1 + dy);
        }
    }

    /// Layer › New › Artboard from Group: group `g` (top-level) becomes an
    /// artboard over `rect`.
    pub fn artboard_from_group(&mut self, g: usize, rect: PixelRect) -> bool {
        let l = &self.layers[g];
        if !l.is_group() || l.artboard.is_some() || l.depth > 0 || l.locked {
            return false;
        }
        self.checkpoint(t!("artboard-hist-from-group"));
        let background = ArtboardBackground::White;
        let layer = &mut self.layers[g];
        layer.offset = (rect.x, rect.y);
        layer.artboard = Some(Artboard {
            size: (rect.w.max(1), rect.h.max(1)),
            background,
        });
        self.fit_artboards();
        true
    }

    /// Layer › New › Artboard from Layers: the layers `indices` go into a
    /// new artboard over `rect`, named `name`, at the top level.
    pub fn artboard_from_layers(
        &mut self,
        indices: &[usize],
        name: String,
        rect: PixelRect,
    ) -> bool {
        if indices.iter().any(|&i| self.artboard_of(i) == Some(i)) || !self.group_layers(indices) {
            return false;
        }
        // One step, under its own name.
        if let Some(step) = self.undo.back_mut() {
            step.name = t!("artboard-hist-from-layers");
        }
        let g = self.active;
        if self.layers[g].depth > 0 {
            // Out of the artboard it was made in.
            let r = self.block(g);
            let mut top = g;
            while let Some(p) = self.parent(top) {
                top = p;
            }
            self.move_block(r.clone(), top + 1 - r.len(), 0);
        }
        let g = self.active;
        let layer = &mut self.layers[g];
        layer.name = name;
        layer.offset = (rect.x, rect.y);
        layer.artboard = Some(Artboard {
            size: (rect.w.max(1), rect.h.max(1)),
            background: ArtboardBackground::White,
        });
        self.fit_artboards();
        true
    }

    /// Layer › Ungroup Artboards: the artboards among `indices` go, their
    /// layers stay in their place. The canvas stays as it is.
    pub fn ungroup_artboards(&mut self, indices: &[usize]) -> bool {
        let ids: Vec<u64> = indices
            .iter()
            .filter(|&&i| {
                self.layers
                    .get(i)
                    .is_some_and(|l| l.artboard.is_some() && !l.locked)
            })
            .map(|&i| self.layers[i].id)
            .collect();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint(t!("artboard-hist-ungroup"));
        for id in ids {
            let Some(g) = self.layers.iter().position(|l| l.id == id) else {
                continue;
            };
            let r = self.block(g);
            for l in &mut self.layers[r.start..g] {
                l.depth -= 1;
            }
            self.layers.remove(g);
        }
        if self.layers.is_empty() {
            let layer = self.make_layer(
                t!("doc-layer-default-name", number = 1),
                RgbaImage::new(self.canvas.0, self.canvas.1),
                (0, 0),
            );
            self.layers.push(layer);
        }
        self.active = self.active.min(self.layers.len() - 1);
        true
    }

    /// Whether a block (`r`) at `depth` would put an artboard inside a
    /// group: artboards stay at the top level.
    pub(super) fn nests_artboard(&self, r: &Range<usize>, depth: u32) -> bool {
        depth > 0 && self.layers[r.clone()].iter().any(|l| l.artboard.is_some())
    }

    /// After a move with the Move tool, like Photoshop: each moved layer
    /// (a group: with its layers) whose content now has its center on
    /// another artboard goes to the top of that one; off every artboard, it
    /// goes onto the pasteboard, just above the artboard it left. Part of
    /// the move's history step. Returns whether a layer changed artboard.
    pub fn adopt(&mut self, moved: &[u64]) -> bool {
        let active = self.layers[self.active].id;
        let mut changed = false;
        for &id in moved {
            let Some(i) = self.layers.iter().position(|l| l.id == id) else {
                continue;
            };
            // Moved along with a group that was moved too.
            let carried = std::iter::successors(self.parent(i), |&p| self.parent(p))
                .any(|p| moved.contains(&self.layers[p].id));
            if carried || self.layers[i].artboard.is_some() {
                continue;
            }
            let Some(b) = self.element_bounds(i) else {
                continue;
            };
            let (cx, cy) = (b.x + b.w as i64 / 2, b.y + b.h as i64 / 2);
            let target = self.artboard_at(cx, cy);
            let current = self.parent(i).and_then(|p| self.artboard_of(p));
            if target == current {
                continue;
            }
            let r = self.block(i);
            match (target, current) {
                (Some(a), _) => {
                    let at = if r.start < a { a - r.len() } else { a };
                    self.move_block(r, at, 1);
                }
                (None, Some(c)) => self.move_block(r.clone(), c + 1 - r.len(), 0),
                (None, None) => continue,
            }
            changed = true;
        }
        if let Some(a) = self.layers.iter().position(|l| l.id == active) {
            self.active = a;
        }
        changed
    }

    /// Artboard `i` as an image of its size.
    pub fn artboard_image(&self, i: usize, options: ArtboardExport) -> Option<RgbaImage> {
        let rect = self.layers.get(i)?.artboard_rect()?;
        let mut layers: Vec<Layer> = if options.overlapping {
            self.render_layers().into_owned()
        } else {
            self.render_layers()[self.block(i)].to_vec()
        };
        let at = if options.overlapping {
            i
        } else {
            layers.len() - 1
        };
        let board = &mut layers[at];
        board.visible = true;
        if let (Some(a), false) = (&mut board.artboard, options.background) {
            a.background = ArtboardBackground::Transparent;
        }
        let plan = compose::plan(
            &layers,
            self.full_rect(),
            &mut FullSize,
            &Options::default(),
        );
        Some(plan.render(rect))
    }

    /// Artboards to Files: the artboards `which`, one file each (the
    /// prefix and its name), to run on another thread. Returns the files
    /// written.
    pub fn export_artboards_later(
        &self,
        which: &[usize],
        files: ArtboardFiles,
    ) -> impl FnOnce() -> Result<Vec<PathBuf>, FileError> + Send + 'static {
        let render = self.artboard_images_later(which, files.options);
        let profile = self.icc_profile.clone();
        move || {
            let mut used = Vec::new();
            let mut written = Vec::new();
            for (name, image) in render() {
                let stem = artboard_file_stem(&format!("{}{name}", files.prefix), &mut used);
                let ext = files.format.extensions()[0];
                let path = artboard_path(&files.folder, &stem, ext, &mut used, files.replace);
                let profile = profile.as_deref().map(Vec::as_slice);
                super::file_io::export(&image, &path, files.format, files.quality, profile)?;
                written.push(path);
            }
            Ok(written)
        }
    }

    /// [`Self::artboard_image`] of the artboards `which` (with their names),
    /// to run on another thread: the layers are taken along (their pixels
    /// are shared, not copied).
    pub fn artboard_images_later(
        &self,
        which: &[usize],
        options: ArtboardExport,
    ) -> impl FnOnce() -> Vec<(String, RgbaImage)> + Send + 'static {
        let mut copy = Self::new(RgbaImage::new(0, 0), None);
        copy.canvas = self.canvas;
        copy.layers = self.render_layers().into_owned();
        let which = which.to_vec();
        move || {
            which
                .into_iter()
                .filter_map(|i| {
                    let image = copy.artboard_image(i, options)?;
                    Some((copy.layers[i].name.clone(), image))
                })
                .collect()
        }
    }

    /// File › New with Artboards: "Artboard 1" of `w` × `h` holding an
    /// empty "Layer 1".
    pub fn with_artboard(w: u32, h: u32, background: ArtboardBackground) -> Self {
        let mut doc = Self::new(RgbaImage::new(w, h), None);
        let rect = PixelRect { x: 0, y: 0, w, h };
        let board = doc.artboard_layer(t!("artboard-name", number = 1), rect, background);
        doc.layer_number = 1;
        let layer = &mut doc.layers[0];
        layer.name = t!("doc-layer-default-name", number = 1);
        layer.depth = 1;
        doc.layers.push(board);
        doc.active = 0;
        doc
    }
}

/// A file name Windows keeps for a device ("CON", "nul.png", "COM1"…).
pub fn windows_device(name: &str) -> bool {
    let base = name
        .split('.')
        .next()
        .unwrap_or("")
        .trim_end()
        .to_ascii_uppercase();
    matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4
            && (base.starts_with("COM") || base.starts_with("LPT"))
            && base.as_bytes()[3].is_ascii_digit())
}

/// A file name made of `name`: the characters file systems refuse
/// replaced, never hidden, empty or a Windows device name, numbered if
/// already `used` (whatever the case).
pub fn artboard_file_stem(name: &str, used: &mut Vec<String>) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    // Windows drops trailing dots and spaces; a leading dot hides a file.
    let clean = clean.trim_matches(|c: char| c.is_whitespace() || c == '.');
    let clean = match clean {
        "" => t!("artboard-name", number = 1),
        c if windows_device(c) => format!("{c}_"),
        c => c.to_string(),
    };
    let mut stem = clean.clone();
    let mut n = 2;
    while used.contains(&stem.to_lowercase()) {
        stem = format!("{clean}-{n}");
        n += 1;
    }
    used.push(stem.to_lowercase());
    stem
}

/// The file names `names` (with `prefix`) are exported to in `folder`
/// with extension `ext`, those that exist: replaced, or kept with the
/// new files numbered (see [`artboard_path`]).
pub fn existing_artboard_files(folder: &Path, prefix: &str, names: &[String], ext: &str) -> usize {
    let mut used = Vec::new();
    names
        .iter()
        .filter(|name| {
            let stem = artboard_file_stem(&format!("{prefix}{name}"), &mut used);
            folder.join(format!("{stem}.{ext}")).exists()
        })
        .count()
}

/// The file `stem`.`ext` in `folder`: replaced if `replace`, else
/// numbered while one of that name exists ("Artboard 1-2.png"). The name
/// is then `used`.
pub fn artboard_path(
    folder: &Path,
    stem: &str,
    ext: &str,
    used: &mut Vec<String>,
    replace: bool,
) -> PathBuf {
    if replace {
        let path = folder.join(format!("{stem}.{ext}"));
        used.push(path_stem(&path));
        return path;
    }
    let mut path = folder.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while path.exists() || (n > 2 && used.contains(&path_stem(&path))) {
        path = folder.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    used.push(path_stem(&path));
    path
}

fn path_stem(path: &Path) -> String {
    path.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
    const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

    fn rect(x: i64, y: i64, w: u32, h: u32) -> PixelRect {
        PixelRect { x, y, w, h }
    }

    /// Two 100 × 80 artboards side by side; a red layer in the first one
    /// that overflows it.
    fn boards() -> Document {
        let mut doc = Document::with_artboard(100, 80, ArtboardBackground::White);
        let name = doc.new_artboard_name();
        doc.add_artboard(
            "new".into(),
            name,
            rect(200, 0, 100, 80),
            ArtboardBackground::Black,
        );
        doc.active = 1;
        doc.add_layer("red".into(), RgbaImage::from_pixel(150, 40, RED), (20, 10));
        doc
    }

    #[test]
    fn layers_are_clipped_to_their_artboard() {
        let doc = boards();
        let flat = doc.flatten();
        assert_eq!(flat.dimensions(), (300, 80), "the canvas spans both");
        assert_eq!(flat.get_pixel(50, 20).0, RED.0);
        // Past the artboard's right edge: the pasteboard, transparent.
        assert_eq!(flat.get_pixel(120, 20).0, [0; 4]);
        // Its background, then the other artboard's (black).
        assert_eq!(flat.get_pixel(50, 70).0, [255; 4]);
        assert_eq!(flat.get_pixel(250, 20).0, [0, 0, 0, 255]);
        // The clipped part can't be picked.
        let red = doc.layers.iter().position(|l| l.name == "red").unwrap();
        assert_eq!(doc.pick_layer(50, 20), Some(red));
        assert_eq!(doc.pick_layer(120, 20), None);
        // Tiles at a reduced level agree.
        let plan = compose::plan(
            &doc.layers,
            doc.full_rect(),
            &mut compose::LevelCache::new(1),
            &Options::default(),
        );
        let half = plan.render(rect(0, 0, 150, 40));
        assert_eq!(half.get_pixel(25, 10).0, RED.0);
        assert_eq!(half.get_pixel(60, 10).0, [0; 4]);
        assert_eq!(half.get_pixel(125, 10).0, [0, 0, 0, 255]);
    }

    #[test]
    fn the_canvas_follows_the_artboards() {
        let mut doc = boards();
        let second = doc.artboards()[1];
        // Moved left of the first one: the origin moves with it.
        doc.set_artboard_rect(second, rect(-300, 50, 100, 80));
        let r = doc.fit_artboards().expect("the canvas changed");
        assert_eq!(r, rect(-300, 0, 400, 130));
        assert_eq!(doc.full_rect(), rect(0, 0, 400, 130));
        let first = doc.artboards()[0];
        assert_eq!(
            doc.layers[first].artboard_rect(),
            Some(rect(300, 0, 100, 80))
        );
        let red = doc.layers.iter().find(|l| l.name == "red").unwrap();
        assert_eq!(red.offset, (320, 10), "everything moved along");
        assert_eq!(doc.fit_artboards(), None);
        // A third one, then undone.
        let name = doc.new_artboard_name();
        doc.add_artboard(
            "new".into(),
            name,
            rect(0, 200, 50, 50),
            ArtboardBackground::Transparent,
        );
        assert_eq!(doc.full_rect(), rect(0, 0, 400, 250));
        doc.undo();
        assert_eq!(doc.full_rect(), rect(0, 0, 400, 130));
    }

    #[test]
    fn plus_buttons_add_or_duplicate_artboards() {
        let mut doc = boards();
        let first = doc.artboards()[0];
        assert!(doc.add_adjacent_artboard(first, Side::Bottom, false));
        let added = doc.active;
        assert_eq!(
            doc.layers[added].artboard_rect(),
            Some(rect(0, 180, 100, 80))
        );
        assert_eq!(doc.block(added).len(), 1, "empty");
        assert!(doc.add_adjacent_artboard(first, Side::Left, true));
        let copy = doc.active;
        assert_eq!(doc.layers[copy].artboard_rect().map(|r| r.x), Some(0));
        assert_eq!(doc.full_rect().w, 500);
        let layers = doc.block(copy);
        assert_eq!(layers.len(), 3, "with its layers");
        let red = &doc.layers[layers.end - 2];
        assert_eq!((red.name.as_str(), red.offset), ("red", (20, 10)));
    }

    #[test]
    fn moved_layers_change_artboard() {
        let mut doc = boards();
        let red = doc.layers.iter().position(|l| l.name == "red").unwrap();
        let id = doc.layers[red].id;
        // Its center onto the second artboard.
        doc.layers[red].offset = (180, 10);
        assert!(doc.adopt(&[id]));
        let i = doc.layers.iter().position(|l| l.id == id).unwrap();
        let second = doc.artboards()[1];
        assert_eq!(doc.artboard_of(i), Some(second));
        assert_eq!(i, second - 1, "at its top");
        assert_eq!(doc.active, i);
        // Onto the pasteboard (below both, between them).
        doc.layers[i].offset = (60, 200);
        assert!(doc.adopt(&[id]));
        let i = doc.layers.iter().position(|l| l.id == id).unwrap();
        assert_eq!((doc.artboard_of(i), doc.layers[i].depth), (None, 0));
        assert_eq!(i, doc.artboards()[1] + 1, "above the artboard it left");
        assert!(!doc.adopt(&[id]));
    }

    #[test]
    fn artboards_export_with_or_without_background_and_overlaps() {
        let mut doc = boards();
        let blue = RgbaImage::from_pixel(40, 20, BLUE);
        // On the pasteboard, over the second artboard's corner.
        let blue = doc.make_layer("blue".into(), blue, (190, 0));
        doc.layers.push(blue);
        let [first, second] = doc.artboards()[..] else {
            panic!()
        };
        let only = ArtboardExport {
            background: true,
            overlapping: false,
        };
        let a = doc.artboard_image(first, only).unwrap();
        assert_eq!(a.dimensions(), (100, 80));
        assert_eq!(a.get_pixel(50, 20).0, RED.0);
        assert_eq!(a.get_pixel(50, 70).0, [255; 4]);
        let b = doc.artboard_image(second, only).unwrap();
        assert_eq!(b.get_pixel(5, 5).0, [0, 0, 0, 255], "content only");
        let over = ArtboardExport {
            background: false,
            overlapping: true,
        };
        let b = doc.artboard_image(second, over).unwrap();
        assert_eq!(b.get_pixel(5, 5).0, BLUE.0, "with what overlaps it");
        assert_eq!(b.get_pixel(50, 50).0, [0; 4], "without its background");
        let later = doc.artboard_images_later(&[first, second], only)();
        assert_eq!(later.len(), 2);
        assert_eq!(later[1].0, doc.layers[second].name);
    }

    #[test]
    fn artboards_to_files_writes_one_file_each() {
        let doc = boards();
        let folder = std::env::temp_dir().join("atelier-artboards-to-files");
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let files = ArtboardFiles {
            folder: folder.clone(),
            prefix: "doc_".into(),
            format: ExportFormat::Png,
            quality: 90,
            options: ArtboardExport {
                background: true,
                overlapping: false,
            },
            replace: false,
        };
        let which = doc.artboards();
        let names: Vec<String> = which.iter().map(|&i| doc.layers[i].name.clone()).collect();
        assert_eq!(existing_artboard_files(&folder, "doc_", &names, "png"), 0);
        let written = doc.export_artboards_later(&which, files.clone())().unwrap();
        assert_eq!(written.len(), 2);
        assert_eq!(existing_artboard_files(&folder, "doc_", &names, "png"), 2);
        // Again: numbered, the first files kept.
        let again = doc.export_artboards_later(&which, files.clone())().unwrap();
        assert!(again.iter().all(|p| !written.contains(p) && p.exists()));
        assert!(written.iter().all(|p| p.exists()));
        // Replacing: the same files.
        let replace = ArtboardFiles {
            replace: true,
            ..files
        };
        let replaced = doc.export_artboards_later(&which, replace)().unwrap();
        assert_eq!(replaced, written);
        for (path, &i) in written.iter().zip(&which) {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("doc_") && name.ends_with(".png"), "{name}");
            let saved = image::open(path).unwrap().to_rgba8();
            let options = ArtboardExport {
                background: true,
                overlapping: false,
            };
            assert_eq!(Some(saved), doc.artboard_image(i, options));
        }
        assert_ne!(written[0], written[1], "same names numbered");
        let mut used = Vec::new();
        assert_eq!(artboard_file_stem("a/b", &mut used), "a_b");
        assert_eq!(artboard_file_stem("A_B", &mut used), "A_B-2");
        // Never hidden, empty or a device.
        let blank = artboard_file_stem("   ", &mut used);
        assert!(!blank.is_empty() && !blank.starts_with('.'));
        assert_eq!(artboard_file_stem(".hidden. ", &mut used), "hidden");
        assert_eq!(artboard_file_stem("con", &mut used), "con_");
        assert_eq!(artboard_file_stem("LPT1.txt", &mut used), "LPT1.txt_");
    }

    #[test]
    fn artboards_from_layers_and_groups_then_ungrouped() {
        let mut doc = Document::new(RgbaImage::from_pixel(200, 100, RED), None);
        doc.add_layer("a".into(), RgbaImage::from_pixel(10, 10, BLUE), (150, 50));
        assert!(doc.artboard_from_layers(&[1], "Board".into(), rect(140, 40, 30, 30)));
        assert_eq!(
            doc.undo.back().map(|s| s.name.as_str()),
            Some(t!("artboard-hist-from-layers").as_str())
        );
        assert_eq!(
            doc.full_rect(),
            rect(0, 0, 30, 30),
            "the canvas is the artboard"
        );
        assert_eq!(doc.layers[1].offset, (10, 10));
        // The former background is pasteboard content now.
        assert_eq!(doc.flatten().get_pixel(0, 0).0, [255; 4]);
        assert!(doc.ungroup_artboards(&[2]));
        assert!(!doc.has_artboards());
        assert_eq!(doc.layers.len(), 2);
        assert_eq!(doc.flatten().get_pixel(0, 0).0, RED.0);
        // An open group nested in nothing becomes an artboard.
        doc.add_group();
        let g = doc.active;
        assert!(doc.artboard_from_group(g, rect(0, 0, 20, 20)));
        assert_eq!(doc.full_rect(), rect(0, 0, 20, 20));
        // It can't go into a group.
        doc.active = 0;
        doc.add_group();
        let (group, board) = (doc.active, doc.artboards()[0]);
        assert!(!doc.drop_into(board, group));
        assert!(!doc.drop_layer(board, Some(group)));
    }
}
