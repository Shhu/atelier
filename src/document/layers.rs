//! Structure of the layer stack: groups, clipping masks, adjustment and fill
//! layers, moves and merges.
//!
//! The stack stays a flat list from the bottom up. A group is a layer whose
//! own layers are just below it, one level deeper ([`Layer::depth`]): a
//! layer is at most one level deeper than the layer above it when that one
//! is a group, and as deep otherwise.

use std::ops::Range;

use image::RgbaImage;

use super::{Document, Fill, Layer, LayerKind, MergeError, NewMask, PixelRect, Selection};
use crate::compose::{self, BlendMode, FullSize, Options};
use crate::i18n::t;
use crate::pixel_ops::Adjustment;

impl Document {
    /// Layers moving with layer `i`: itself, and its own if it is a group.
    pub fn block(&self, i: usize) -> Range<usize> {
        let d = self.layers[i].depth;
        let mut j = i;
        if self.layers[i].is_group() {
            while j > 0 && self.layers[j - 1].depth > d {
                j -= 1;
            }
        }
        j..i + 1
    }

    /// The group layer `i` is in.
    pub fn parent(&self, i: usize) -> Option<usize> {
        let d = self.layers.get(i)?.depth;
        (i + 1..self.layers.len()).find(|&j| self.layers[j].depth < d)
    }

    /// Shown: it and its groups are visible.
    pub fn shown(&self, i: usize) -> bool {
        let mut at = Some(i);
        while let Some(j) = at {
            if !self.layers[j].visible {
                return false;
            }
            at = self.parent(j);
        }
        true
    }

    /// Whether a group above layer `i` is closed in the Layers panel.
    pub fn folded(&self, i: usize) -> bool {
        let mut at = self.parent(i);
        while let Some(g) = at {
            if matches!(self.layers[g].kind, LayerKind::Group { expanded: false }) {
                return true;
            }
            at = self.parent(g);
        }
        false
    }

    /// Keeps the nesting coherent (see the module's documentation).
    pub(super) fn fix_depths(&mut self) {
        let mut max = 0;
        for l in self.layers.iter_mut().rev() {
            l.depth = l.depth.min(max);
            max = l.depth + u32::from(l.is_group());
        }
    }

    /// Puts a new layer above the active layer, in its group (at the top of
    /// an open group that is active; in the clipping group of a clipped
    /// layer), and makes it active.
    pub(super) fn insert_layer(&mut self, mut layer: Layer) {
        let (at, depth, clipped) = match self.layers.get(self.active) {
            None => (0, 0, false),
            Some(a) if matches!(a.kind, LayerKind::Group { expanded: true }) => {
                (self.active, a.depth + 1, false)
            }
            Some(a) => (self.active + 1, a.depth, a.clipped && !layer.is_group()),
        };
        layer.depth = depth;
        layer.clipped = clipped;
        self.layers.insert(at, layer);
        self.active = at;
    }

    /// `make(n)` for the first number n from 1 that no layer is named yet.
    pub(super) fn unused_name(&self, make: impl Fn(u32) -> String) -> String {
        (1..=self.layers.len() as u32 + 1)
            .map(&make)
            .find(|n| self.layers.iter().all(|l| l.name != *n))
            .unwrap_or_else(|| make(1))
    }

    /// Name of a copy of a layer named `name`, like Photoshop: "Logo
    /// copy", then "Logo copy 2"… (a copy's copy too), one no layer has.
    pub fn copy_name(&self, name: &str) -> String {
        let suffix = t!("doc-layer-copy-name", name = "");
        // "Logo copy 3" → "Logo copy" → "Logo".
        let numbered = name
            .rsplit_once(' ')
            .filter(|(_, n)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
            .map_or(name, |(s, _)| s);
        let stem = numbered
            .strip_suffix(suffix.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| name.strip_suffix(suffix.as_str()).filter(|s| !s.is_empty()))
            .unwrap_or(name);
        let first = t!("doc-layer-copy-name", name = stem);
        let used = |n: &str| self.layers.iter().any(|l| l.name == n);
        if !used(&first) {
            return first;
        }
        (2..=self.layers.len() as u32 + 2)
            .map(|n| t!("tools5-layer-copy-numbered", name = stem, number = n))
            .find(|n| !used(n))
            .unwrap_or(first)
    }

    /// A layer without pixels of `kind`.
    fn empty_layer(&mut self, name: String, kind: LayerKind) -> Layer {
        let mut layer = self.make_layer(name, RgbaImage::new(0, 0), (0, 0));
        layer.kind = kind;
        layer
    }

    /// New adjustment layer above the active one, with a mask (from the
    /// selection, if any), like Photoshop.
    pub fn add_adjustment_layer(&mut self, adjustment: Adjustment, selection: Option<&Selection>) {
        let title = adjustment.title();
        let name = self.unused_name(|n| {
            t!(
                "core-layer-numbered-name",
                name = title.as_str(),
                number = n
            )
        });
        let layer = self.empty_layer(name, LayerKind::Adjustment(adjustment));
        let step = t!("xchg-hist-new-adjustment-layer", name = title.as_str());
        self.add_without_pixels(step, layer, selection);
    }

    /// New fill layer (solid color or gradient) above the active one.
    pub fn add_fill_layer(&mut self, fill: Fill, selection: Option<&Selection>) {
        let name = match fill {
            Fill::Solid(_) => self.unused_name(|n| t!("core-solid-fill-name", number = n)),
            Fill::Gradient(_) => self.unused_name(|n| t!("core-gradient-fill-name", number = n)),
        };
        let layer = self.empty_layer(name, LayerKind::Fill(fill));
        self.add_without_pixels(t!("core-menu-new-fill-layer"), layer, selection);
    }

    fn add_without_pixels(&mut self, step: String, layer: Layer, selection: Option<&Selection>) {
        self.checkpoint(step);
        self.insert_layer(layer);
        // The mask is part of the same step.
        let kind = if selection.is_some() {
            NewMask::RevealSelection
        } else {
            NewMask::RevealAll
        };
        self.put_mask(kind, selection);
    }

    /// New empty group above the active layer.
    pub fn add_group(&mut self) {
        let name = self.unused_name(|n| t!("core-group-name", number = n));
        let mut group = self.empty_layer(name, LayerKind::Group { expanded: true });
        group.blend = BlendMode::PassThrough;
        self.checkpoint(t!("core-menu-new-group"));
        self.insert_layer(group);
    }

    /// Group Layers (Ctrl+G): the layers `indices` (and the layers of the
    /// groups among them) go into a new group, where the topmost of them
    /// was. Refused if one of them is locked.
    pub fn group_layers(&mut self, indices: &[usize]) -> bool {
        let mut blocks: Vec<Range<usize>> = indices
            .iter()
            .filter(|&&i| i < self.layers.len())
            .map(|&i| self.block(i))
            .collect();
        blocks.sort_by_key(|r| r.start);
        // Layers inside another selected group go with it.
        let mut tops: Vec<Range<usize>> = Vec::new();
        for r in blocks {
            match tops.last_mut() {
                Some(t) if r.start < t.end => t.end = t.end.max(r.end),
                _ => tops.push(r),
            }
        }
        let Some(top) = tops.last().cloned() else {
            return false;
        };
        if tops
            .iter()
            .flat_map(|r| r.clone())
            .any(|i| self.layers[i].locked)
            || tops.iter().any(|r| self.nests_artboard(r, 1))
        {
            return false;
        }
        self.checkpoint(t!("core-menu-group-layers"));
        let depth = self.layers[top.end - 1].depth;
        let mut moved = Vec::new();
        for r in tops.iter().rev() {
            let shift = depth as i64 + 1 - self.layers[r.end - 1].depth as i64;
            let mut part: Vec<Layer> = self.layers.drain(r.clone()).collect();
            for l in &mut part {
                l.depth = (l.depth as i64 + shift).max(0) as u32;
            }
            moved.splice(0..0, part);
        }
        let below: usize = tops[..tops.len() - 1].iter().map(|r| r.len()).sum();
        let at = top.start - below;
        let name = self.unused_name(|n| t!("core-group-name", number = n));
        let mut group = self.empty_layer(name, LayerKind::Group { expanded: true });
        (group.depth, group.blend) = (depth, BlendMode::PassThrough);
        moved.push(group);
        let n = moved.len();
        self.layers.splice(at..at, moved);
        self.active = at + n - 1;
        self.fix_depths();
        true
    }

    /// Ungroup Layers (Ctrl+Shift+G): the active group goes, its layers
    /// stay in its place.
    pub fn ungroup(&mut self) -> bool {
        let g = self.active;
        if !self.layers[g].is_group() || self.layers[g].locked {
            return false;
        }
        self.checkpoint(t!("core-menu-ungroup-layers"));
        let r = self.block(g);
        for l in &mut self.layers[r.start..g] {
            l.depth -= 1;
        }
        self.layers.remove(g);
        self.active = g.saturating_sub(1).max(r.start).min(self.layers.len() - 1);
        if self.layers.is_empty() {
            return true;
        }
        if r.start == g {
            // An empty group: the layer below it becomes active.
            self.active = g.saturating_sub(1).min(self.layers.len() - 1);
        }
        true
    }

    /// Create / Release Clipping Mask (Ctrl+Alt+G) on the active layer.
    /// Releasing a clipped layer releases those clipped above it too.
    pub fn toggle_clipping(&mut self) -> bool {
        self.set_clipping(self.active, !self.layers[self.active].clipped)
    }

    /// Clips layer `i` to the layer below it (Alt + click between them), or
    /// releases it.
    pub fn set_clipping(&mut self, i: usize, clip: bool) -> bool {
        let l = &self.layers[i];
        if l.clipped == clip || l.is_group() {
            return false;
        }
        // A layer below it in the same group to clip to.
        if clip && (i == 0 || self.layers[i - 1].depth < l.depth) {
            return false;
        }
        self.checkpoint(if clip {
            t!("core-menu-create-clipping")
        } else {
            t!("core-menu-release-clipping")
        });
        let d = self.layers[i].depth;
        self.layers[i].clipped = clip;
        if !clip {
            for l in self.layers[i + 1..].iter_mut().take_while(|l| l.depth >= d) {
                if l.depth == d {
                    if !l.clipped {
                        break;
                    }
                    l.clipped = false;
                }
            }
        }
        true
    }

    /// Moves a block of layers so that it starts at `to` in the list
    /// without it, its top layer at `depth`. It becomes active.
    pub(super) fn move_block(&mut self, r: Range<usize>, to: usize, depth: u32) {
        let shift = depth as i64 - self.layers[r.end - 1].depth as i64;
        let mut block: Vec<Layer> = self.layers.drain(r).collect();
        for l in &mut block {
            l.depth = (l.depth as i64 + shift).max(0) as u32;
        }
        let n = block.len();
        let to = to.min(self.layers.len());
        self.layers.splice(to..to, block);
        self.active = to + n - 1;
        self.fix_depths();
    }

    /// Moves the active layer (a group: with its layers) one step up or
    /// down, like Photoshop: into an open group it meets, over a closed
    /// one, out of its group at the end of it.
    pub fn move_active(&mut self, up: bool) -> bool {
        let r = self.block(self.active);
        let d = self.layers[self.active].depth;
        let n = self.layers.len();
        // An artboard goes over groups, never into them.
        let artboard = self.nests_artboard(&r, 1);
        let enters =
            |kind: &LayerKind| matches!(kind, LayerKind::Group { expanded: true }) && !artboard;
        if up {
            let e = r.end;
            if e >= n {
                return false;
            }
            self.checkpoint(t!("xchg-hist-layer-order"));
            if self.layers[e].depth < d {
                // Top of its group: out of it, just above it.
                let len = r.len();
                self.move_block(r, e + 1 - len, d - 1);
                return true;
            }
            let g = (e..n).find(|&j| self.layers[j].depth == d).unwrap_or(e);
            let len = r.len();
            if enters(&self.layers[g].kind) {
                let top = r.end - 1;
                self.move_block(r, top + 1 - len, d + 1);
            } else {
                self.move_block(r, g + 1 - len, d);
            }
            return true;
        }
        let s = r.start;
        if s == 0 {
            return false;
        }
        self.checkpoint(t!("xchg-hist-layer-order"));
        let b = s - 1;
        if self.layers[b].depth < d {
            // Bottom of its group: out of it, just below it.
            self.move_block(r, s, d - 1);
            return true;
        }
        if enters(&self.layers[b].kind) {
            self.move_block(r, b, d + 1);
        } else {
            let below = self.block(b);
            self.move_block(r, below.start, d);
        }
        true
    }

    /// Bring to Front / Send to Back (Ctrl+Shift+] / [): to the top or the
    /// bottom of the active layer's group.
    pub fn move_active_to_end(&mut self, front: bool) -> bool {
        let r = self.block(self.active);
        let d = self.layers[self.active].depth;
        let (first, end) = match self.parent(self.active) {
            Some(p) => (self.block(p).start, p),
            None => (0, self.layers.len()),
        };
        let to = if front { end - r.len() } else { first };
        if to == r.start {
            return false;
        }
        self.checkpoint(t!("xchg-hist-layer-order"));
        self.move_block(r, to, d);
        true
    }

    /// Drop of a dragged layer (Layers panel): its block goes just below
    /// the layer `above` (inside it if it is an open group), or to the very
    /// top. Returns whether something moved.
    pub fn drop_layer(&mut self, from: usize, above: Option<usize>) -> bool {
        let r = self.block(from);
        let (to, depth) = match above {
            None => (self.layers.len(), 0),
            Some(a) if r.contains(&a) => return false,
            Some(a) => {
                let l = &self.layers[a];
                if matches!(l.kind, LayerKind::Group { expanded: true }) {
                    (a, l.depth + 1)
                } else {
                    (self.block(a).start, l.depth)
                }
            }
        };
        let at = if to > r.start { to - r.len() } else { to };
        if (at == r.start && depth == self.layers[from].depth) || self.nests_artboard(&r, depth) {
            return false;
        }
        self.checkpoint(t!("xchg-hist-layer-order"));
        self.move_block(r, at, depth);
        true
    }

    /// Drop of a dragged layer onto group `g` (Layers panel): its block
    /// goes into it, at its top, even when the group is closed. Returns
    /// whether something moved.
    pub fn drop_into(&mut self, from: usize, g: usize) -> bool {
        let r = self.block(from);
        if !self.layers.get(g).is_some_and(Layer::is_group) || r.contains(&g) {
            return false;
        }
        // Already its top layer.
        if (r.end == g && self.layers[from].depth == self.layers[g].depth + 1)
            || self.nests_artboard(&r, 1)
        {
            return false;
        }
        self.checkpoint(t!("xchg-hist-layer-order"));
        let depth = self.layers[g].depth + 1;
        let at = if r.start < g { g - r.len() } else { g };
        self.move_block(r, at, depth);
        true
    }

    /// Links the layers `indices` (they then move together), or unlinks
    /// them if they already are all linked together (the chain button).
    pub fn toggle_link(&mut self, indices: &[usize]) -> bool {
        let links: Vec<Option<u64>> = indices.iter().map(|&i| self.layers[i].link).collect();
        let together = links
            .first()
            .is_some_and(|l| l.is_some() && links.iter().all(|k| k == l));
        if indices.len() < 2 && !together {
            return false;
        }
        self.checkpoint(if together {
            t!("xchg-hist-unlink-layers")
        } else {
            t!("xchg-hist-link-layers")
        });
        let link = (!together).then(|| {
            self.next_id += 1;
            self.next_id
        });
        for &i in indices {
            self.layers[i].link = link;
        }
        true
    }

    /// Layers moving with layer `i`: its own (a group's), those linked to
    /// it, and theirs.
    pub fn moving_with(&self, i: usize) -> Vec<usize> {
        let link = self.layers[i].link;
        let mut out: Vec<usize> = (0..self.layers.len())
            .filter(|&j| j == i || (link.is_some() && self.layers[j].link == link))
            .flat_map(|j| self.block(j))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

    /// Copy of the active layer (a group: with its layers) above it, with
    /// all its properties (a text stays editable).
    pub fn duplicate_active(&mut self) {
        let r = self.block(self.active);
        self.checkpoint(t!("menu-layer-duplicate"));
        let copies: Vec<Layer> = self.layers[r.clone()]
            .iter()
            .map(|l| {
                self.next_id += 1;
                Layer {
                    id: self.next_id,
                    ..l.clone()
                }
            })
            .collect();
        let n = copies.len();
        self.layers.splice(r.end..r.end, copies);
        self.active = r.end + n - 1;
        let name = self.copy_name(&self.layers[self.active].name);
        self.layers[self.active].name = name;
    }

    /// Deletes the active layer (a group: with its layers); at least one
    /// layer always remains.
    pub fn delete_active(&mut self) -> bool {
        let r = self.block(self.active);
        if r.len() >= self.layers.len() {
            return false;
        }
        self.checkpoint(t!("menu-layer-delete"));
        let start = r.start;
        self.layers.drain(r);
        self.active = start.min(self.layers.len() - 1);
        self.fix_depths();
        true
    }

    /// Deletes layers `indices` (a group with its layers) in one step.
    /// Refused (`false`) when that would leave no layer.
    pub fn delete_layers(&mut self, indices: &[usize]) -> bool {
        let mut gone = vec![false; self.layers.len()];
        for &i in indices.iter().filter(|&&i| i < self.layers.len()) {
            for j in self.block(i) {
                gone[j] = true;
            }
        }
        let count = gone.iter().filter(|&&g| g).count();
        if count == 0 || count >= self.layers.len() {
            return false;
        }
        // Named for one layer or several (a group counts as one).
        let mut chosen: Vec<usize> = indices
            .iter()
            .copied()
            .filter(|&i| i < self.layers.len())
            .collect();
        chosen.sort_unstable();
        chosen.dedup();
        let inside = |i: usize| chosen.iter().any(|&g| g != i && self.block(g).contains(&i));
        let several = chosen.iter().filter(|&&i| !inside(i)).count() > 1;
        self.checkpoint(match several {
            true => t!("hist5-delete-layers"),
            false => t!("menu-layer-delete"),
        });
        let lowest = gone.iter().position(|&g| g).unwrap_or(0);
        let mut k = 0;
        self.layers.retain(|_| {
            k += 1;
            !gone[k - 1]
        });
        self.active = lowest.min(self.layers.len() - 1);
        self.fix_depths();
        true
    }

    /// Composite of `layers` (a stack on their own) over `region`.
    fn render(&self, layers: &[Layer], region: PixelRect) -> RgbaImage {
        let plan = compose::plan(layers, self.full_rect(), &mut FullSize, &Options::default());
        plan.render(region)
    }

    /// Box of what layers `indices` show on their own (the canvas for those
    /// reaching everywhere). Refused if too large.
    fn merge_region(
        &self,
        indices: impl IntoIterator<Item = usize>,
    ) -> Result<PixelRect, MergeError> {
        let mut region: Option<PixelRect> = None;
        for i in indices {
            let l = &self.layers[i];
            let e = match l.kind {
                LayerKind::Pixels => match (l.content_bounds(), l.extent(false)) {
                    (Some(c), Some(e)) => Some(intersect_or(e, c)),
                    _ => None,
                },
                LayerKind::Group { .. } | LayerKind::Adjustment(_) => None,
                LayerKind::Fill(_) => Some(l.extent(false).unwrap_or(self.full_rect())),
            };
            if let Some(e) = e {
                region = Some(region.map_or(e, |r| r.union(e)));
            }
        }
        let region = region.unwrap_or_else(|| self.full_rect());
        self.fits(region)
            .then_some(region)
            .ok_or(MergeError::TooLarge)
    }

    /// Merge Down (Ctrl+E), like Photoshop: the active layer goes into the
    /// layer below (an adjustment: applied to it); the result keeps the
    /// name, opacity and blend mode of the lower layer; styles and masks
    /// are applied. On a group: Merge Group. Refused if either is hidden or
    /// locked, or if the layer below has no pixels.
    pub fn merge_down(&mut self) -> Result<(), MergeError> {
        let upper = self.active;
        if self.layers[upper].is_group() {
            return self.merge_group();
        }
        let d = self.layers[upper].depth;
        if upper == 0 || self.layers[upper - 1].depth != d {
            return Err(MergeError::Nothing);
        }
        let lower = upper - 1;
        let (below, above) = (&self.layers[lower], &self.layers[upper]);
        if !below.visible || !above.visible || !below.has_pixels() {
            return Err(MergeError::Nothing);
        }
        if below.locked || above.locked {
            return Err(MergeError::Locked);
        }
        let region = self.merge_region([lower, upper])?;
        let base = Layer {
            blend: BlendMode::Normal,
            opacity: 1.0,
            clipped: false,
            depth: 0,
            ..below.clone()
        };
        let top = Layer {
            depth: 0,
            // Clipped to the lower layer: atop it, alone.
            clipped: above.clipped,
            ..above.clone()
        };
        let image = self.render(&[base, top], region);
        self.checkpoint(t!("menu-layer-merge-down"));
        self.layers.remove(upper);
        let merged = &mut self.layers[lower];
        merged.replace_pixels(image);
        merged.offset = (region.x, region.y);
        merged.effects = Default::default();
        merged.mask = None;
        merged.fill = 1.0;
        self.active = lower;
        Ok(())
    }

    /// Merge Group: the active group becomes one pixel layer (its blend
    /// mode kept, unless Pass Through).
    pub fn merge_group(&mut self) -> Result<(), MergeError> {
        let g = self.active;
        let r = self.block(g);
        if self.layers[r.clone()].iter().any(|l| l.locked) {
            return Err(MergeError::Locked);
        }
        let group = self.layers[g].clone();
        let region = self.merge_region(r.start..g)?;
        let mut part: Vec<Layer> = self.layers[r.clone()].to_vec();
        let base = group.depth;
        for l in &mut part {
            l.depth -= base;
        }
        let top = part.last_mut().expect("the group");
        top.visible = true;
        top.clipped = false;
        let image = self.render(&part, region);
        let mut merged = self.make_layer(group.name.clone(), image, (region.x, region.y));
        merged.depth = group.depth;
        merged.blend = match group.blend {
            BlendMode::PassThrough => BlendMode::Normal,
            m => m,
        };
        merged.visible = group.visible;
        self.checkpoint(t!("core-menu-merge-group"));
        let start = r.start;
        self.layers.splice(r, [merged]);
        self.active = start;
        Ok(())
    }

    /// Merges the visible layers into one, in place of the lowest of them
    /// (Ctrl+Shift+E). Hidden layers are kept as they are (out of the
    /// merged groups). Refused, like Merge Down, if one of them is locked.
    pub fn merge_visible(&mut self) -> Result<(), MergeError> {
        let visible: Vec<usize> = (0..self.layers.len()).filter(|&i| self.shown(i)).collect();
        let [first, _, ..] = visible[..] else {
            return Err(MergeError::Nothing);
        };
        if visible.iter().any(|&i| self.layers[i].locked) {
            return Err(MergeError::Locked);
        }
        let region = self.merge_region(visible.iter().copied())?;
        let image = self.render(&self.layers, region);
        let name = self.layers[first].name.clone();
        self.checkpoint(t!("menu-layer-merge-visible"));
        let merged = self.make_layer(name, image, (region.x, region.y));
        for &i in visible.iter().rev() {
            self.layers.remove(i);
        }
        self.layers.insert(first, merged);
        self.layers[first].depth = 0;
        self.active = first;
        self.fix_depths();
        Ok(())
    }
}

/// `a ∩ b`, or an empty box at `a` if they don't meet.
fn intersect_or(a: PixelRect, b: PixelRect) -> PixelRect {
    super::intersect(a, b).unwrap_or(PixelRect { w: 0, h: 0, ..a })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];

    /// Name and depth of each layer ("G" for the groups).
    fn names(doc: &Document) -> Vec<(String, u32)> {
        let name = |l: &Layer| {
            if l.is_group() {
                "G".into()
            } else {
                l.name.clone()
            }
        };
        doc.layers.iter().map(|l| (name(l), l.depth)).collect()
    }

    /// Background, then A, B, C (C active).
    fn abc() -> Document {
        let mut doc = Document::new(RgbaImage::from_pixel(4, 4, Rgba([255; 4])), None);
        for (name, c) in [("A", RED), ("B", BLUE), ("C", RED)] {
            doc.add_layer(name.into(), RgbaImage::from_pixel(1, 1, Rgba(c)), (0, 0));
        }
        doc
    }

    #[test]
    fn grouping_and_ungrouping() {
        let mut doc = abc();
        assert!(doc.group_layers(&[1, 3]));
        assert_eq!(
            names(&doc),
            [
                ("Background".into(), 0),
                ("B".into(), 0),
                ("A".into(), 1),
                ("C".into(), 1),
                ("G".into(), 0)
            ]
        );
        assert_eq!(doc.active, 4);
        assert_eq!(doc.block(4), 2..5);
        assert_eq!(doc.parent(2), Some(4));
        assert_eq!(doc.layers[4].blend, BlendMode::PassThrough);
        // A new layer in an open group goes into it, at the top.
        doc.add_empty_layer();
        assert_eq!((doc.active, doc.layers[doc.active].depth), (4, 1));
        // Hiding the group hides its layers.
        doc.layers[5].visible = false;
        assert!(!doc.shown(2));
        assert_eq!(doc.flatten().get_pixel(0, 0).0, BLUE);
        doc.layers[5].visible = true;
        doc.active = 5;
        assert!(doc.ungroup());
        assert!(doc.layers.iter().all(|l| l.depth == 0));
        assert_eq!(doc.layers.len(), 5);
        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 6);
        // Deleting a group deletes its layers; duplicating copies them.
        doc.active = 5;
        doc.duplicate_active();
        assert_eq!(doc.layers.len(), 10);
        assert!(doc.delete_active());
        assert_eq!(doc.layers.len(), 6);
    }

    #[test]
    fn moving_through_groups() {
        let mut doc = abc();
        doc.group_layers(&[2]); // B in a group, between A and C.
        assert_eq!(names(&doc)[2..4], [("B".into(), 1), ("G".into(), 0)]);
        doc.active = 1; // A, below the open group: up goes into it.
        assert!(doc.move_active(true));
        assert_eq!(
            names(&doc)[1..4],
            [("A".into(), 1), ("B".into(), 1), ("G".into(), 0)]
        );
        // Up again: over B, then out of the group.
        assert!(doc.move_active(true));
        assert!(doc.move_active(true));
        assert_eq!(
            names(&doc)[1..4],
            [("B".into(), 1), ("G".into(), 0), ("A".into(), 0)]
        );
        // A closed group is jumped over whole.
        if let LayerKind::Group { expanded } = &mut doc.layers[2].kind {
            *expanded = false;
        }
        assert!(doc.move_active(false));
        assert_eq!(
            names(&doc)[1..4],
            [("A".into(), 0), ("B".into(), 1), ("G".into(), 0)]
        );
        // The group moves with its layers.
        doc.active = 3;
        assert!(doc.move_active_to_end(false));
        assert_eq!(names(&doc)[..2], [("B".into(), 1), ("G".into(), 0)]);
        assert!(doc.drop_layer(1, None));
        assert_eq!(names(&doc)[3..], [("B".into(), 1), ("G".into(), 0)]);
    }

    #[test]
    fn dropping_onto_a_closed_group_puts_the_layer_inside() {
        let mut doc = abc();
        doc.group_layers(&[2]); // B in a group, between A and C.
        if let LayerKind::Group { expanded } = &mut doc.layers[3].kind {
            *expanded = false;
        }
        // C, above the group, then A, below it: both go in, at its top.
        assert!(doc.drop_into(4, 3));
        assert_eq!(
            names(&doc)[1..],
            [
                ("A".into(), 0),
                ("B".into(), 1),
                ("C".into(), 1),
                ("G".into(), 0)
            ]
        );
        assert!(doc.drop_into(1, 4));
        assert_eq!(
            names(&doc)[1..],
            [
                ("B".into(), 1),
                ("C".into(), 1),
                ("A".into(), 1),
                ("G".into(), 0)
            ]
        );
        assert_eq!(doc.active, 3);
        // Already there; a group into itself: nothing.
        assert!(!doc.drop_into(3, 4));
        assert!(!doc.drop_into(4, 4));
        assert_eq!(doc.history().0.last(), Some(&"Layer Order"));
    }

    #[test]
    fn clipping_masks() {
        let mut doc = Document::new(RgbaImage::new(4, 1), None);
        let mut base = RgbaImage::new(4, 1);
        base.put_pixel(1, 0, Rgba(RED));
        doc.add_layer("base".into(), base, (0, 0));
        doc.add_layer(
            "top".into(),
            RgbaImage::from_pixel(4, 1, Rgba(BLUE)),
            (0, 0),
        );
        assert!(doc.toggle_clipping());
        let flat = doc.flatten();
        assert_eq!(flat.get_pixel(1, 0).0, BLUE);
        assert_eq!(flat.get_pixel(2, 0).0[3], 0);
        // Hiding the base hides what is clipped to it.
        doc.layers[1].visible = false;
        assert_eq!(doc.flatten().get_pixel(1, 0).0[3], 0);
        doc.layers[1].visible = true;
        // A new layer above a clipped one is clipped too; releasing the
        // lower releases both.
        doc.add_empty_layer();
        assert!(doc.active_layer().clipped);
        doc.active = 2;
        assert!(doc.toggle_clipping());
        assert!(!doc.layers[2].clipped && !doc.layers[3].clipped);
        // The bottom layer has nothing to clip to.
        doc.active = 0;
        assert!(!doc.toggle_clipping());
    }

    #[test]
    fn adjustment_layers_and_merges() {
        use crate::pixel_ops::{AdjustKind, HueSaturation};
        let mut doc = Document::new(RgbaImage::from_pixel(2, 1, Rgba(RED)), None);
        let dark = Adjustment::HueSaturation(HueSaturation {
            lightness: -1.0,
            ..Default::default()
        });
        doc.add_adjustment_layer(dark, None);
        assert!(doc.active_layer().mask.is_some() && doc.mask_targeted());
        assert_eq!(doc.flatten().get_pixel(0, 0).0, [0, 0, 0, 255]);
        // Its opacity mixes it with what is below.
        doc.active_layer_mut().opacity = 0.5;
        assert_eq!(doc.flatten().get_pixel(0, 0).0, [128, 0, 0, 255]);
        // One undo step, mask included.
        assert!(doc.undo());
        assert_eq!(doc.layers.len(), 1);
        doc.add_adjustment_layer(AdjustKind::Invert.initial(), None);
        assert_eq!(doc.flatten().get_pixel(0, 0).0, [0, 255, 255, 255]);
        // Merged down: applied to the pixels.
        assert!(doc.merge_down().is_ok());
        assert_eq!(doc.layers.len(), 1);
        assert_eq!(doc.layers[0].pixel_at(1, 0), Some([0, 255, 255, 255]));
        // Fill layer, then Merge Visible.
        doc.add_fill_layer(Fill::Solid([0, 255, 0]), None);
        assert!(doc.merge_visible().is_ok());
        assert_eq!(doc.layers[0].pixel_at(0, 0), Some([0, 255, 0, 255]));
    }

    #[test]
    fn merge_group_keeps_its_look() {
        let mut doc = abc();
        doc.group_layers(&[2, 3]);
        doc.layers[4].opacity = 0.5;
        let (before, name) = (doc.flatten(), doc.layers[4].name.clone());
        assert!(doc.merge_down().is_ok());
        assert_eq!(doc.layers.len(), 3);
        assert_eq!(doc.active_layer().name, name);
        assert_eq!(doc.flatten(), before);
    }
}
