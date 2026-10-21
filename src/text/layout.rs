//! Laying out (cosmic-text shapes and wraps each paragraph; lines, indents,
//! leading and spacing are placed here) and rendering (glyph outlines
//! filled through the layer's transformation).

use std::ops::Range;
use std::sync::Arc;

use cosmic_text::skrifa::{self, MetadataProvider as _};
use cosmic_text::{
    Align, Attrs, AttrsOwned, Buffer, CacheKey, CacheKeyFlags, Command, Family, FeatureTag,
    FontFeatures, FontSystem, LayoutLine, Metrics, Shaping, Stretch, Style, SwashCache,
    UnderlineStyle, Weight, Wrap, fontdb,
};
use image::RgbaImage;

use super::raster::Raster;
use super::{
    AntiAlias, CharStyle, IDENTITY, Kerning, ParagraphStyle, SMALL_CAPS, TextAlign, TextData,
    displayed,
};
use crate::document::MAX_DIMENSION;
use crate::pixel_ops::{over, threads};

/// A style of a font family (Regular, Bold Italic, Condensed…).
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    /// Its name in the font.
    pub name: String,
    pub weight: u16,
    pub italic: bool,
    pub stretch: u16,
}

/// A line of the laid out text, in text coordinates (y down).
#[derive(Clone, Debug)]
pub struct Line {
    pub top: f32,
    pub bottom: f32,
    /// Horizontal extent.
    pub x0: f32,
    pub x1: f32,
    /// Byte range of the text it shows.
    pub start: usize,
    pub end: usize,
    /// Caret positions: (byte index, x), by index.
    stops: Vec<(usize, f32)>,
    /// Lines past the bottom of a paragraph box are hidden.
    pub shown: bool,
}

#[derive(Clone, Debug)]
struct Glyph {
    font_id: fontdb::ID,
    glyph_id: u16,
    size: f32,
    weight: Weight,
    flags: CacheKeyFlags,
    /// Pen position on the baseline.
    x: f32,
    y: f32,
    /// Vertical stretch of the outline (vertical over horizontal scale).
    stretch_y: f32,
    color: [u8; 4],
    faux_bold: bool,
}

/// Underline or strikethrough: a band of the text.
#[derive(Clone, Debug)]
struct Band {
    x0: f32,
    x1: f32,
    y: f32,
    thickness: f32,
    color: [u8; 4],
}

/// A text laid out: lines (for the caret and the selection) and glyphs.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub lines: Vec<Line>,
    glyphs: Vec<Glyph>,
    bands: Vec<Band>,
    /// Paragraph text that doesn't fit in its box.
    pub overflow: bool,
    /// Box of the lines (paragraph text: the box), text coordinates:
    /// [x0, y0, x1, y1].
    pub extent: [f32; 4],
}

impl Layout {
    /// Line showing the caret at byte `i`: at a line wrap, the next line.
    pub fn line_of(&self, i: usize) -> usize {
        let mut found = 0;
        for (k, line) in self.lines.iter().enumerate() {
            if line.start <= i {
                found = k;
            }
            if i < line.end || (line.start <= i && i <= line.end && self.next_starts_later(k, i)) {
                return k;
            }
        }
        found
    }

    fn next_starts_later(&self, k: usize, i: usize) -> bool {
        self.lines.get(k + 1).is_none_or(|n| n.start > i)
    }

    /// Caret at byte `i`: x, top and bottom.
    pub fn caret(&self, i: usize) -> (f32, f32, f32) {
        let Some(line) = self.lines.get(self.line_of(i)) else {
            return (0.0, 0.0, 0.0);
        };
        (line.x_at(i), line.top, line.bottom)
    }

    /// Byte index nearest to the point (text coordinates).
    pub fn hit(&self, x: f32, y: f32) -> usize {
        let mut shown = self.lines.iter().filter(|l| l.shown);
        let line = shown
            .clone()
            .find(|l| y < l.bottom)
            .or_else(|| shown.next_back())
            .or(self.lines.first());
        line.map_or(0, |l| l.nearest(x))
    }

    /// From byte `i`, the nearest index on the line above (`up`) or below,
    /// at about `x`.
    pub fn vertical(&self, i: usize, x: f32, up: bool) -> Option<usize> {
        let k = self.line_of(i);
        let k = if up { k.checked_sub(1)? } else { k + 1 };
        Some(self.lines.get(k)?.nearest(x))
    }

    /// Start and end of the line showing byte `i` (Home, End).
    pub fn line_bounds(&self, i: usize) -> Range<usize> {
        self.lines
            .get(self.line_of(i))
            .map_or(i..i, |l| l.start..l.end)
    }

    /// Highlight of the bytes `range`: one rectangle per line,
    /// [x0, top, x1, bottom].
    pub fn selection(&self, range: Range<usize>) -> Vec<[f32; 4]> {
        let mut out = Vec::new();
        for line in self.lines.iter().filter(|l| l.shown) {
            if range.end < line.start || range.start > line.end {
                continue;
            }
            let (a, b) = (range.start.max(line.start), range.end.min(line.end));
            let x0 = line.x_at(a);
            let mut x1 = line.x_at(b);
            // The line break is selected too: a sliver past the end.
            if range.end > line.end && range.start <= line.end {
                x1 += (line.bottom - line.top) * 0.25;
            }
            if x1 > x0 {
                out.push([x0, line.top, x1, line.bottom]);
            }
        }
        out
    }
}

impl Line {
    fn x_at(&self, i: usize) -> f32 {
        let k = self.stops.partition_point(|&(j, _)| j <= i);
        self.stops
            .get(k.saturating_sub(1))
            .map_or(self.x0, |&(_, x)| x)
    }

    fn nearest(&self, x: f32) -> usize {
        self.stops
            .iter()
            .min_by(|a, b| (a.1 - x).abs().total_cmp(&(b.1 - x).abs()))
            .map_or(self.start, |&(i, _)| i)
    }
}

/// A rendered text: its image and where the text's origin lies in it.
pub struct Rendered {
    pub image: RgbaImage,
    pub anchor: (f32, f32),
}

/// System fonts and glyph cache. Loading the fonts takes a while: only one
/// is created, on first use.
pub struct TextEngine {
    fonts: FontSystem,
    cache: SwashCache,
    families: Vec<String>,
    /// Last layout, reused while the text doesn't change (the caret is drawn
    /// every frame).
    last: Option<(LayoutKey, Arc<Layout>)>,
}

/// What a layout depends on.
#[derive(PartialEq)]
struct LayoutKey {
    text: String,
    runs: Vec<super::Run>,
    paragraphs: Vec<ParagraphStyle>,
    frame: Option<(f32, f32)>,
}

impl LayoutKey {
    fn of(data: &TextData) -> Self {
        Self {
            text: data.text.clone(),
            runs: data.runs.clone(),
            paragraphs: data.paragraphs.clone(),
            frame: data.frame,
        }
    }
}

impl TextEngine {
    pub fn new() -> Self {
        let fonts = FontSystem::new();
        let mut families: Vec<String> = fonts
            .db()
            .faces()
            .filter_map(|f| f.families.first().map(|(name, _)| name.clone()))
            .collect();
        families.sort_by_key(|f| f.to_lowercase());
        families.dedup();
        Self {
            fonts,
            cache: SwashCache::new(),
            families,
            last: None,
        }
    }

    /// Available font families, sorted.
    pub fn families(&self) -> &[String] {
        &self.families
    }

    /// The styles of `family` (empty: the default font), lightest and
    /// narrowest first, upright before italic.
    pub fn faces(&self, family: &str) -> Vec<Face> {
        let db = self.fonts.db();
        let name = if family.is_empty() {
            db.family_name(&Family::SansSerif).to_owned()
        } else {
            family.to_owned()
        };
        let mut faces: Vec<Face> = db
            .faces()
            .filter(|f| f.families.iter().any(|(n, _)| *n == name))
            .map(|f| {
                let style_name = db
                    .with_face_data(f.id, |data, index| {
                        let font = skrifa::FontRef::from_index(data, index).ok()?;
                        [
                            skrifa::string::StringId::TYPOGRAPHIC_SUBFAMILY_NAME,
                            skrifa::string::StringId::SUBFAMILY_NAME,
                        ]
                        .into_iter()
                        .find_map(|id| font.localized_strings(id).english_or_first())
                        .map(|s| s.to_string())
                    })
                    .flatten();
                Face {
                    name: style_name.unwrap_or_else(|| f.weight.0.to_string()),
                    weight: f.weight.0,
                    italic: f.style != Style::Normal,
                    stretch: f.stretch.to_number(),
                }
            })
            .collect();
        faces.sort_by_key(|f| (f.stretch, f.weight, f.italic));
        faces.dedup_by(|a, b| (a.weight, a.italic, a.stretch) == (b.weight, b.italic, b.stretch));
        faces
    }

    /// Lays out the text (the last layout is kept).
    pub fn layout(&mut self, data: &TextData) -> Arc<Layout> {
        let key = LayoutKey::of(data);
        if let Some((k, layout)) = &self.last
            && *k == key
        {
            return Arc::clone(layout);
        }
        let layout = Arc::new(self.build(data));
        self.last = Some((key, Arc::clone(&layout)));
        layout
    }

    fn build(&mut self, data: &TextData) -> Layout {
        let mut out = Layout::default();
        let ranges = data.paragraph_ranges();
        let box_width = data.frame.map(|f| f.0);
        let mut baseline: Option<f32> = None;
        let mut space_after = 0.0;
        // Style table: one per run, in order.
        let styles: Vec<&CharStyle> = data.runs.iter().map(|r| &r.style).collect();
        let run_starts: Vec<usize> = data
            .runs
            .iter()
            .scan(0, |s, r| {
                let start = *s;
                *s += r.len;
                Some(start)
            })
            .collect();
        for (p, range) in ranges.iter().enumerate() {
            let para = data.paragraphs.get(p).copied().unwrap_or_default();
            let para_text = Paragraph::new(data, range.clone(), &run_starts);
            let lines = self.paragraph_lines(&para_text, &styles, &para, box_width);
            for (j, (line, offset)) in lines.into_iter().enumerate() {
                let first = j == 0;
                let lh = line
                    .line_height_opt
                    .unwrap_or_else(|| data.style_at(range.start).line_height());
                let (ascent, descent) = if line.glyphs.is_empty() {
                    self.empty_metrics(data.style_at(range.start))
                } else {
                    (line.max_ascent, line.max_descent)
                };
                let base = match baseline {
                    None if data.frame.is_some() => ascent,
                    None => 0.0,
                    Some(b) if first => b + lh + space_after + para.space_before,
                    Some(b) => b + lh,
                };
                baseline = Some(base);
                let indent = para.indent_left + if first { para.indent_first } else { 0.0 };
                let x0 = match (data.frame, para.align) {
                    // An empty line of a box: the caret where typing goes.
                    (Some((w, _)), align) if line.glyphs.is_empty() => {
                        let room = (w - indent - para.indent_right).max(0.0);
                        indent
                            + match align {
                                TextAlign::Center => room / 2.0,
                                TextAlign::Right => room,
                                TextAlign::Left | TextAlign::Justify => 0.0,
                            }
                    }
                    (Some(_), _) | (None, TextAlign::Left | TextAlign::Justify) => indent,
                    (None, TextAlign::Center) => (indent - para.indent_right - line.w) / 2.0,
                    (None, TextAlign::Right) => -line.w - para.indent_right,
                };
                let shown = data.frame.is_none_or(|(_, h)| base + descent <= h + 0.5)
                    && out.lines.last().is_none_or(|l| l.shown);
                out.overflow |= !shown;
                self.place_line(
                    &mut out, &para_text, &styles, &line, offset, x0, base, ascent, descent, shown,
                );
            }
            space_after = para.space_after;
        }
        out.extent = match data.frame {
            Some((w, h)) => [0.0, 0.0, w, h],
            None => out
                .lines
                .iter()
                .map(|l| [l.x0, l.top, l.x1, l.bottom])
                .reduce(|a, b| {
                    [
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ]
                })
                .unwrap_or_default(),
        };
        out
    }

    /// Lines of a paragraph, with the byte offset of their glyphs in its
    /// laid out text (after a first line indented differently).
    fn paragraph_lines(
        &mut self,
        para: &Paragraph,
        styles: &[&CharStyle],
        style: &ParagraphStyle,
        box_width: Option<f32>,
    ) -> Vec<(LayoutLine, usize)> {
        let width = box_width.map(|w| (w - style.indent_left - style.indent_right).max(1.0));
        let align = match (box_width, style.align) {
            (None, _) | (_, TextAlign::Left) => Align::Left,
            (_, TextAlign::Center) => Align::Center,
            (_, TextAlign::Right) => Align::Right,
            (_, TextAlign::Justify) => Align::Justified,
        };
        let first_width = width.map(|w| (w - style.indent_first).max(1.0));
        let mut lines = self.shape(para, 0, styles, first_width, align);
        if lines.is_empty() {
            lines.push(LayoutLine {
                w: 0.0,
                max_ascent: 0.0,
                max_descent: 0.0,
                line_height_opt: None,
                glyphs: Vec::new(),
                decorations: Vec::new(),
            });
        }
        if width == first_width || lines.len() < 2 {
            return lines.into_iter().map(|l| (l, 0)).collect();
        }
        // The other lines wrap at the full width.
        let split = lines[1].glyphs.iter().map(|g| g.start).min().unwrap_or(0);
        let mut out = vec![(lines.into_iter().next().expect("two lines"), 0)];
        out.extend(
            self.shape(para, split, styles, width, align)
                .into_iter()
                .map(|l| (l, split)),
        );
        out
    }

    /// Shapes and wraps the paragraph's text from byte `from` (of its laid
    /// out text).
    fn shape(
        &mut self,
        para: &Paragraph,
        from: usize,
        styles: &[&CharStyle],
        width: Option<f32>,
        align: Align,
    ) -> Vec<LayoutLine> {
        let attrs: Vec<(Range<usize>, AttrsOwned)> = para
            .spans
            .iter()
            .filter(|(r, ..)| r.end > from)
            .map(|(r, s, small)| {
                let range = r.start.max(from) - from..r.end - from;
                (range, AttrsOwned::new(&attrs_of(styles[*s], *s, *small)))
            })
            .collect();
        let style = styles[para.first_style];
        let mut buffer = Buffer::new_empty(Metrics::new(
            style.size.max(0.1),
            style.line_height().max(0.1),
        ));
        buffer.set_wrap(if width.is_some() {
            Wrap::WordOrGlyph
        } else {
            Wrap::None
        });
        buffer.set_size(width, None);
        let default = attrs_of(style, para.first_style, false);
        let text = &para.text[from..];
        buffer.set_rich_text(
            attrs.iter().map(|(r, a)| (&text[r.clone()], a.as_attrs())),
            &default,
            Shaping::Advanced,
            Some(align),
        );
        buffer
            .line_layout(&mut self.fonts, 0)
            .map(<[LayoutLine]>::to_vec)
            .unwrap_or_default()
    }

    /// Ascent and descent of an empty line in this style.
    fn empty_metrics(&mut self, style: &CharStyle) -> (f32, f32) {
        let mut buffer = Buffer::new_empty(Metrics::new(
            style.size.max(0.1),
            style.line_height().max(0.1),
        ));
        let attrs = attrs_of(style, 0, false);
        buffer.set_rich_text([(" ", attrs.clone())], &attrs, Shaping::Advanced, None);
        buffer
            .line_layout(&mut self.fonts, 0)
            .and_then(|l| l.first())
            .map_or((style.size * 0.8, style.size * 0.2), |l| {
                (l.max_ascent, l.max_descent)
            })
    }

    #[allow(clippy::too_many_arguments)]
    fn place_line(
        &mut self,
        out: &mut Layout,
        para: &Paragraph,
        styles: &[&CharStyle],
        line: &LayoutLine,
        offset: usize,
        x0: f32,
        baseline: f32,
        ascent: f32,
        descent: f32,
        shown: bool,
    ) {
        let source = |b: usize| para.map[(b + offset).min(para.map.len() - 1)];
        let mut stops = Vec::new();
        let (mut left, mut x1) = (f32::MAX, f32::MIN);
        let mut end = None::<usize>;
        for g in &line.glyphs {
            let cluster = &para.text[g.start + offset..g.end + offset];
            let n = cluster.chars().count().max(1);
            for (k, (b, _)) in cluster.char_indices().enumerate() {
                let x = x0 + g.x + g.w * k as f32 / n as f32;
                stops.push((source(g.start + b), x));
            }
            left = left.min(x0 + g.x);
            x1 = x1.max(x0 + g.x + g.w);
            end = end.max(Some(source(g.end)));
            let style = styles[g.metadata];
            if shown {
                out.glyphs.push(Glyph {
                    font_id: g.font_id,
                    glyph_id: g.glyph_id,
                    size: g.font_size,
                    weight: g.font_weight,
                    flags: g.cache_key_flags,
                    x: x0 + g.x + g.font_size * g.x_offset,
                    y: baseline + g.y - g.font_size * g.y_offset - style.baseline_shift,
                    stretch_y: style.vertical_scale / style.horizontal_scale,
                    color: style.color,
                    faux_bold: style.faux_bold,
                });
            }
        }
        if line.glyphs.is_empty() {
            (left, x1) = (x0, x0);
        }
        let end = end.unwrap_or(para.range.start);
        stops.push((end, x1));
        stops.sort_by_key(|s| s.0);
        stops.dedup_by_key(|s| s.0);
        if shown {
            for d in &line.decorations {
                let Some(g) = line.glyphs.get(d.glyph_range.start) else {
                    continue;
                };
                let last = &line.glyphs[d.glyph_range.end.max(1) - 1];
                let style = styles[g.metadata];
                let td = &d.data.text_decoration;
                let x = (x0 + g.x, x0 + last.x + last.w);
                let y = baseline - style.baseline_shift;
                let band = |m: cosmic_text::DecorationMetrics| Band {
                    x0: x.0,
                    x1: x.1,
                    y: y - m.offset * d.font_size,
                    thickness: (m.thickness * d.font_size).max(1.0),
                    color: style.color,
                };
                if td.underline != UnderlineStyle::None {
                    out.bands.push(band(d.data.underline_metrics));
                }
                if td.strikethrough {
                    out.bands.push(band(d.data.strikethrough_metrics));
                }
            }
        }
        out.lines.push(Line {
            top: baseline - ascent,
            bottom: baseline + descent,
            x0: left,
            x1,
            start: stops.first().map_or(para.range.start, |s| s.0),
            end,
            stops,
            shown,
        });
    }

    /// Renders the text into a transparent image. The text's origin lands
    /// at the returned anchor, whose fractional part is the one of
    /// `data.anchor` (the text doesn't move when the layer is re-rendered).
    pub fn render(&mut self, data: &TextData) -> Rendered {
        let layout = self.layout(data);
        let m = data.transform;
        let map = move |x: f32, y: f32| (m[0] * x + m[1] * y, m[2] * x + m[3] * y);
        let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        let hint = data.antialias != AntiAlias::Smooth && m == IDENTITY;

        // Shapes relative to the anchor.
        let mut shapes: Vec<Shape> = Vec::with_capacity(layout.glyphs.len() + layout.bands.len());
        for g in &layout.glyphs {
            let flags = if hint && g.stretch_y == 1.0 {
                g.flags
            } else {
                g.flags | CacheKeyFlags::DISABLE_HINTING
            };
            let (key, ..) =
                CacheKey::new(g.font_id, g.glyph_id, g.size, (0.0, 0.0), g.weight, flags);
            let Some(commands) = self.cache.get_outline_commands(&mut self.fonts, key) else {
                continue;
            };
            let place = |x: f32, y: f32| map(g.x + x, g.y - y * g.stretch_y);
            let path: Vec<Seg> = commands
                .iter()
                .map(|c| match *c {
                    Command::MoveTo(p) => Seg::Move(place(p.x, p.y)),
                    Command::LineTo(p) => Seg::Line(place(p.x, p.y)),
                    Command::QuadTo(c, p) => Seg::Quad(place(c.x, c.y), place(p.x, p.y)),
                    Command::CurveTo(a, b, p) => {
                        Seg::Cubic(place(a.x, a.y), place(b.x, b.y), place(p.x, p.y))
                    }
                    Command::Close => Seg::Close,
                })
                .collect();
            if path.is_empty() {
                continue;
            }
            let bold = if g.faux_bold {
                g.size * 0.025 * scale
            } else {
                0.0
            };
            shapes.push(Shape::new(path, g.color, bold));
        }
        for b in &layout.bands {
            let (t0, t1) = (b.y - b.thickness / 2.0, b.y + b.thickness / 2.0);
            let path = vec![
                Seg::Move(map(b.x0, t0)),
                Seg::Line(map(b.x1, t0)),
                Seg::Line(map(b.x1, t1)),
                Seg::Line(map(b.x0, t1)),
                Seg::Close,
            ];
            shapes.push(Shape::new(path, b.color, 0.0));
        }

        // The image holds the shapes and the text's box.
        let [ex0, ey0, ex1, ey1] = layout.extent;
        let mut bounds = [ex0, ey0, ex1, ey1, ex0, ey1, ex1, ey0]
            .chunks(2)
            .map(|c| map(c[0], c[1]))
            .fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, (x, y)| {
                [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
            });
        for s in &shapes {
            bounds = [
                bounds[0].min(s.bounds[0]),
                bounds[1].min(s.bounds[1]),
                bounds[2].max(s.bounds[2]),
                bounds[3].max(s.bounds[3]),
            ];
        }
        const PAD: f32 = 2.0;
        let frac = (data.anchor.0.rem_euclid(1.0), data.anchor.1.rem_euclid(1.0));
        let anchor = (
            frac.0 + (PAD - bounds[0] - frac.0).ceil(),
            frac.1 + (PAD - bounds[1] - frac.1).ceil(),
        );
        let w = ((bounds[2] + anchor.0 + PAD).ceil().max(1.0) as u32).min(MAX_DIMENSION);
        let h = ((bounds[3] + anchor.1 + PAD).ceil().max(1.0) as u32).min(MAX_DIMENSION);
        let mut image = RgbaImage::new(w, h);
        // Outlines filled on all cores, then painted in order.
        let aliased = data.antialias == AntiAlias::None;
        let chunk = shapes.len().div_ceil(threads()).max(16);
        let coverages: Vec<Option<Coverage>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = shapes
                .chunks(chunk)
                .map(|part| {
                    scope.spawn(move || {
                        part.iter()
                            .map(|s| s.coverage((w, h), anchor, aliased))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter()
                .flat_map(|j| j.join().expect("text rendering thread"))
                .collect()
        });
        for (s, c) in shapes.iter().zip(coverages) {
            if let Some(c) = c {
                c.paint(&mut image, s.color);
            }
        }
        Rendered { image, anchor }
    }
}

/// A paragraph as laid out: its text after case changes, by spans of one
/// style, and where each of its bytes comes from.
struct Paragraph {
    range: Range<usize>,
    text: String,
    /// (bytes of `text`, style index, faux small capitals).
    spans: Vec<(Range<usize>, usize, bool)>,
    /// Source byte of each byte of `text`, and of its end.
    map: Vec<usize>,
    first_style: usize,
}

impl Paragraph {
    fn new(data: &TextData, range: Range<usize>, run_starts: &[usize]) -> Self {
        let mut out = Self {
            range: range.clone(),
            text: String::new(),
            spans: Vec::new(),
            map: Vec::new(),
            first_style: run_starts
                .partition_point(|&s| s <= range.start)
                .saturating_sub(1)
                .min(data.runs.len() - 1),
        };
        // An empty last run is the style to type with.
        while out.first_style > 0 && data.runs[out.first_style].len == 0 {
            out.first_style -= 1;
        }
        for (i, c) in data.text[range.clone()].char_indices() {
            let at = range.start + i;
            let s = run_starts
                .partition_point(|&start| start <= at)
                .saturating_sub(1);
            let (shown, small) = displayed(c, &data.runs[s].style);
            let start = out.text.len();
            out.text.push(shown);
            out.map.resize(out.text.len(), at);
            match out.spans.last_mut() {
                Some((r, style, sm)) if *style == s && *sm == small => r.end = out.text.len(),
                _ => out.spans.push((start..out.text.len(), s, small)),
            }
        }
        out.map.push(range.end);
        out
    }
}

fn attrs_of(style: &CharStyle, index: usize, small_caps: bool) -> Attrs<'_> {
    let family = if style.family.is_empty() {
        Family::SansSerif
    } else {
        Family::Name(&style.family)
    };
    let size = style.size * style.horizontal_scale * if small_caps { SMALL_CAPS } else { 1.0 };
    let mut attrs = Attrs::new()
        .family(family)
        .weight(Weight(style.weight))
        .style(if style.italic {
            Style::Italic
        } else {
            Style::Normal
        })
        .stretch(stretch(style.stretch))
        .metrics(Metrics::new(size.max(0.1), style.line_height().max(0.1)))
        .metadata(index);
    if style.tracking != 0.0 {
        attrs = attrs.letter_spacing(style.tracking / 1000.0);
    }
    if style.kerning == Kerning::Zero {
        let mut features = FontFeatures::new();
        features.disable(FeatureTag::KERNING);
        attrs = attrs.font_features(features);
    }
    if style.faux_italic {
        attrs = attrs.cache_key_flags(CacheKeyFlags::FAKE_ITALIC);
    }
    if style.underline {
        attrs = attrs.underline(UnderlineStyle::Single);
    }
    if style.strikethrough {
        attrs = attrs.strikethrough();
    }
    attrs
}

fn stretch(n: u16) -> Stretch {
    match n {
        1 => Stretch::UltraCondensed,
        2 => Stretch::ExtraCondensed,
        3 => Stretch::Condensed,
        4 => Stretch::SemiCondensed,
        6 => Stretch::SemiExpanded,
        7 => Stretch::Expanded,
        8 => Stretch::ExtraExpanded,
        9 => Stretch::UltraExpanded,
        _ => Stretch::Normal,
    }
}

#[derive(Clone, Copy)]
enum Seg {
    Move((f32, f32)),
    Line((f32, f32)),
    Quad((f32, f32), (f32, f32)),
    Cubic((f32, f32), (f32, f32), (f32, f32)),
    Close,
}

/// A filled outline, relative to the anchor, y down.
struct Shape {
    path: Vec<Seg>,
    color: [u8; 4],
    /// Faux bold: the outline grown by this many pixels.
    bold: f32,
    bounds: [f32; 4],
}

impl Shape {
    fn new(path: Vec<Seg>, color: [u8; 4], bold: f32) -> Self {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let mut add = |(x, y): (f32, f32)| {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        };
        for s in &path {
            match *s {
                Seg::Move(p) | Seg::Line(p) => add(p),
                Seg::Quad(c, p) => {
                    add(c);
                    add(p);
                }
                Seg::Cubic(c1, c2, p) => {
                    add(c1);
                    add(c2);
                    add(p);
                }
                Seg::Close => {}
            }
        }
        let bounds = [b[0] - bold, b[1] - bold, b[2] + bold, b[3] + bold];
        Self {
            path,
            color,
            bold,
            bounds,
        }
    }

    /// Its coverage in an image of `size`: top-left corner, width and
    /// values (0–255) of its box.
    fn coverage(
        &self,
        (iw, ih): (u32, u32),
        anchor: (f32, f32),
        aliased: bool,
    ) -> Option<Coverage> {
        let x0 = (self.bounds[0] + anchor.0).floor().max(0.0);
        let y0 = (self.bounds[1] + anchor.1).floor().max(0.0);
        let x1 = (self.bounds[2] + anchor.0).ceil().min(iw as f32);
        let y1 = (self.bounds[3] + anchor.1).ceil().min(ih as f32);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let (w, h) = ((x1 - x0) as usize, (y1 - y0) as usize);
        // Faux bold: the union of copies around a circle.
        let offsets: Vec<(f32, f32)> = if self.bold > 0.0 {
            let n = 12;
            std::iter::once((0.0, 0.0))
                .chain((0..n).map(|k| {
                    let a = k as f32 * std::f32::consts::TAU / n as f32;
                    (a.cos() * self.bold, a.sin() * self.bold)
                }))
                .collect()
        } else {
            vec![(0.0, 0.0)]
        };
        let mut coverage = vec![0.0f32; w * h];
        for (dx, dy) in offsets {
            let o = (anchor.0 - x0 + dx, anchor.1 - y0 + dy);
            let at = |(x, y): (f32, f32)| (x + o.0, y + o.1);
            let mut r = Raster::new(w, h);
            for s in &self.path {
                match *s {
                    Seg::Move(p) => r.move_to(at(p)),
                    Seg::Line(p) => r.line_to(at(p)),
                    Seg::Quad(c, p) => r.quad_to(at(c), at(p)),
                    Seg::Cubic(a, b, p) => r.cubic_to(at(a), at(b), at(p)),
                    Seg::Close => r.close(),
                }
            }
            for (c, v) in coverage.iter_mut().zip(r.coverage()) {
                *c = c.max(v);
            }
        }
        let alpha = self.color[3] as f32;
        let values = coverage
            .into_iter()
            .map(|c| {
                let c = if aliased { (c >= 0.5) as u8 as f32 } else { c };
                (alpha * c).round() as u8
            })
            .collect();
        Some(Coverage {
            x: x0 as u32,
            y: y0 as u32,
            w,
            values,
        })
    }
}

struct Coverage {
    x: u32,
    y: u32,
    w: usize,
    values: Vec<u8>,
}

impl Coverage {
    fn paint(&self, image: &mut RgbaImage, [r, g, b, _]: [u8; 4]) {
        for (i, &a) in self.values.iter().enumerate() {
            if a > 0 {
                let (x, y) = (self.x + (i % self.w) as u32, self.y + (i / self.w) as u32);
                let dst = image.get_pixel_mut(x, y);
                dst.0 = over(dst.0, [r, g, b, a]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::TextAlign;

    /// An engine, with a font whose metrics are known when the machine has
    /// it (no fonts at all: `None`, the test is skipped).
    fn engine() -> Option<(TextEngine, CharStyle)> {
        let engine = TextEngine::new();
        if engine.families().is_empty() {
            return None;
        }
        let family = ["DejaVu Sans", "Liberation Sans", "Arial"]
            .into_iter()
            .find(|f| engine.families().iter().any(|e| e == f))
            .unwrap_or_default();
        let style = CharStyle {
            family: family.into(),
            size: 20.0,
            ..Default::default()
        };
        Some((engine, style))
    }

    const LONG: &str = "The quick brown fox jumps over the lazy dog again and again";

    fn boxed(text: &str, style: &CharStyle, w: f32, h: f32, align: TextAlign) -> TextData {
        let mut d = TextData::plain(text, style.clone());
        d.frame = Some((w, h));
        for p in &mut d.paragraphs {
            p.align = align;
        }
        d
    }

    #[test]
    fn paragraph_text_wraps_in_its_box() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let d = boxed(LONG, &style, 200.0, 1000.0, TextAlign::Left);
        let l = e.layout(&d);
        assert!(l.lines.len() >= 3, "{} lines", l.lines.len());
        assert!(l.lines.iter().all(|line| line.x1 <= 200.5 && line.shown));
        assert!(!l.overflow);
        assert_eq!(l.extent, [0.0, 0.0, 200.0, 1000.0]);
        // Every character is on a line, in order.
        assert_eq!(l.lines[0].start, 0);
        assert_eq!(l.lines.last().unwrap().end, LONG.len());
        // A short box hides the lines that don't fit: the text overflows.
        let l = e.layout(&boxed(LONG, &style, 200.0, 30.0, TextAlign::Left));
        assert!(l.overflow);
        assert_eq!(l.lines.iter().filter(|l| l.shown).count(), 1);
        // Point text doesn't wrap.
        let l = e.layout(&TextData::plain(LONG, style));
        assert_eq!(l.lines.len(), 1);
    }

    #[test]
    fn leading_is_the_distance_between_baselines() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let tops = |e: &mut TextEngine, style: &CharStyle| -> Vec<f32> {
            let l = e.layout(&TextData::plain("Ab\nCd\n\nEf", style.clone()));
            l.lines.iter().map(|l| l.top).collect()
        };
        let auto = tops(&mut e, &style);
        assert_eq!(auto.len(), 4, "an empty line too");
        for pair in auto.windows(2) {
            assert!((pair[1] - pair[0] - 24.0).abs() < 0.01, "auto: 120 % of 20");
        }
        let set = tops(
            &mut e,
            &CharStyle {
                leading: Some(50.0),
                ..style.clone()
            },
        );
        assert!((set[1] - set[0] - 50.0).abs() < 0.01);
        // Point text: the first baseline is at the origin.
        let l = e.layout(&TextData::plain("Ab", style));
        assert!(l.lines[0].top < -10.0 && l.lines[0].bottom > 0.0);
    }

    #[test]
    fn tracking_and_kerning_change_the_width() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let width = |e: &mut TextEngine, text: &str, style: CharStyle| {
            let l = e.layout(&TextData::plain(text, style));
            l.lines[0].x1 - l.lines[0].x0
        };
        let plain = width(&mut e, "abcdef", style.clone());
        let tracked = width(
            &mut e,
            "abcdef",
            CharStyle {
                tracking: 100.0,
                ..style.clone()
            },
        );
        // 100/1000 em of 20 px between letters.
        let added = tracked - plain;
        assert!((10.0..=12.5).contains(&added), "added {added}");
        let kerned = width(&mut e, "AVAV", style.clone());
        let unkerned = width(
            &mut e,
            "AVAV",
            CharStyle {
                kerning: Kerning::Zero,
                ..style.clone()
            },
        );
        assert!(unkerned >= kerned, "{unkerned} < {kerned}");
        // Horizontal scale widens the letters.
        let wide = width(
            &mut e,
            "abcdef",
            CharStyle {
                horizontal_scale: 2.0,
                ..style
            },
        );
        assert!((wide - 2.0 * plain).abs() < 1.0);
    }

    #[test]
    fn paragraphs_align_in_their_box_or_around_the_origin() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let line = |e: &mut TextEngine, d: &TextData| {
            let l = e.layout(d);
            (l.lines[0].x0, l.lines[0].x1)
        };
        let (x0, _) = line(
            &mut e,
            &boxed("Hello", &style, 300.0, 100.0, TextAlign::Left),
        );
        assert!(x0.abs() < 0.5);
        let (x0, x1) = line(
            &mut e,
            &boxed("Hello", &style, 300.0, 100.0, TextAlign::Center),
        );
        assert!(((x0 + x1) / 2.0 - 150.0).abs() < 0.5);
        let (_, x1) = line(
            &mut e,
            &boxed("Hello", &style, 300.0, 100.0, TextAlign::Right),
        );
        assert!((x1 - 300.0).abs() < 0.5);
        // Point text: around its origin.
        let mut d = TextData::plain("Hello", style.clone());
        d.paragraphs[0].align = TextAlign::Center;
        let (x0, x1) = line(&mut e, &d);
        assert!((x0 + x1).abs() < 0.5);
        d.paragraphs[0].align = TextAlign::Right;
        assert!(line(&mut e, &d).1.abs() < 0.5);

        // Justified: every line reaches the right edge but the last one.
        let l = e.layout(&boxed(LONG, &style, 200.0, 1000.0, TextAlign::Justify));
        let (last, full) = l.lines.split_last().unwrap();
        assert!(!full.is_empty());
        for line in full {
            assert!((line.x1 - 200.0).abs() < 1.0, "{}", line.x1);
        }
        assert!(last.x1 < 199.0);
    }

    #[test]
    fn indents_and_paragraph_spacing() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let mut d = boxed(LONG, &style, 250.0, 1000.0, TextAlign::Left);
        d.paragraphs[0].indent_left = 20.0;
        d.paragraphs[0].indent_first = 30.0;
        let l = e.layout(&d);
        assert!((l.lines[0].x0 - 50.0).abs() < 0.01);
        assert!((l.lines[1].x0 - 20.0).abs() < 0.01);
        assert!(l.lines.iter().all(|l| l.x1 <= 250.5));

        let mut d = TextData::plain("Ab\nCd", style);
        let plain = e.layout(&d).lines[1].top;
        d.paragraphs[0].space_after = 7.0;
        d.paragraphs[1].space_before = 5.0;
        let spaced = e.layout(&d).lines[1].top;
        assert!((spaced - plain - 12.0).abs() < 0.01);
    }

    #[test]
    fn caret_hit_testing_and_selection() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let l = e.layout(&TextData::plain("ab\ncd", style));
        let (x0, top, bottom) = l.caret(0);
        assert!(x0.abs() < 0.01 && top < 0.0 && bottom > 0.0);
        assert!(l.caret(1).0 > x0 && l.caret(2).0 > l.caret(1).0);
        assert_eq!(l.line_of(2), 0, "end of the first line");
        assert_eq!(l.line_of(3), 1);
        assert_eq!(l.hit(1000.0, 0.0), 2);
        assert_eq!(l.hit(-50.0, 1000.0), 3);
        assert_eq!(l.vertical(1, l.caret(1).0, false), Some(4));
        assert_eq!(l.vertical(4, l.caret(4).0, true), Some(1));
        assert_eq!(l.vertical(4, 0.0, false), None);
        assert_eq!(l.line_bounds(4), 3..5);
        let rects = l.selection(1..4);
        assert_eq!(rects.len(), 2);
        assert!(
            rects[0][2] > l.caret(2).0,
            "the line break is shown selected"
        );
    }

    #[test]
    fn caps_keep_caret_positions_in_the_text() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let lower = e.layout(&TextData::plain("abß", style.clone()));
        let caps = e.layout(&TextData::plain(
            "abß",
            CharStyle {
                all_caps: true,
                ..style.clone()
            },
        ));
        assert!(caps.lines[0].x1 > lower.lines[0].x1);
        assert_eq!(caps.lines[0].end, "abß".len());
        assert_eq!(caps.hit(1000.0, 0.0), "abß".len());
        let small = e.layout(&TextData::plain(
            "ab",
            CharStyle {
                small_caps: true,
                ..style
            },
        ));
        let (x, upper) = (small.lines[0].x1, caps.caret(2).0);
        assert!(x > 0.6 * upper && x < upper, "smaller capitals");
    }

    fn opaque(img: &RgbaImage) -> usize {
        img.pixels().filter(|p| p.0[3] > 0).count()
    }

    #[test]
    fn renders_styles_runs_and_antialiasing() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let mut d = TextData::plain("HH", style.clone());
        d.restyle(1..2, |s| s.color = [255, 0, 0, 255]);
        let img = e.render(&d).image;
        let black = img.pixels().filter(|p| p.0 == [0, 0, 0, 255]).count();
        let red = img.pixels().filter(|p| p.0 == [255, 0, 0, 255]).count();
        assert!(black > 20 && red > 20, "{black} {red}");
        // Without anti-aliasing: only opaque or empty pixels.
        d.antialias = AntiAlias::None;
        assert!(
            e.render(&d)
                .image
                .pixels()
                .all(|p| p.0[3] == 0 || p.0[3] == 255)
        );
        d.antialias = AntiAlias::Smooth;
        let smooth = e.render(&d).image;
        assert!(smooth.pixels().any(|p| p.0[3] > 0 && p.0[3] < 255));
        // Underline, faux bold: more ink.
        for change in [
            (|s: &mut CharStyle| s.underline = true) as fn(&mut CharStyle),
            |s| s.strikethrough = true,
            |s| s.faux_bold = true,
        ] {
            let mut s = style.clone();
            change(&mut s);
            let more = e.render(&TextData::plain("HH", s)).image;
            assert!(opaque(&more) > opaque(&img) + 10);
        }
    }

    #[test]
    fn rendering_follows_the_transformation_and_keeps_the_anchor() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let mut d = TextData::plain("Hello world", style);
        d.anchor = (0.25, 0.75);
        let flat = e.render(&d);
        assert!(flat.image.width() > 2 * flat.image.height());
        assert_eq!(flat.anchor.0.fract(), 0.25);
        assert_eq!(flat.anchor.1.fract(), 0.75);
        // The origin is on the baseline: most of the ink above it.
        let above = flat
            .image
            .enumerate_pixels()
            .filter(|(_, y, p)| p.0[3] > 0 && (*y as f32) < flat.anchor.1)
            .count();
        assert!(above * 2 > opaque(&flat.image));
        // Turned a quarter: taller than wide, about as much ink.
        d.transform = [0.0, -1.0, 1.0, 0.0];
        let turned = e.render(&d);
        assert!(turned.image.height() > 2 * turned.image.width());
        let (a, b) = (opaque(&flat.image) as f32, opaque(&turned.image) as f32);
        assert!((a - b).abs() / a < 0.15, "{a} vs {b}");
    }

    #[test]
    fn empty_texts_have_a_caret_line() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let d = TextData::new(style, Default::default());
        let l = e.layout(&d);
        assert_eq!(l.lines.len(), 1);
        let (_, top, bottom) = l.caret(0);
        assert!(bottom - top > 15.0);
        let r = e.render(&d);
        assert!(r.image.width() >= 1 && opaque(&r.image) == 0);
    }

    #[test]
    fn families_have_their_faces() {
        let Some((e, style)) = engine() else { return };
        let faces = e.faces(&style.family);
        assert!(!faces.is_empty());
        if style.family == "DejaVu Sans" {
            assert!(
                faces
                    .iter()
                    .any(|f| f.weight == 700 && !f.italic && f.name == "Bold")
            );
        }
    }

    /// `cargo test --release text_render_speed -- --ignored --nocapture`
    #[test]
    #[ignore = "timing, in release"]
    fn text_render_speed() {
        let Some((mut e, style)) = engine() else {
            return;
        };
        let text = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. ".repeat(40);
        let mut d = TextData::plain(
            &text,
            CharStyle {
                size: 48.0,
                ..style
            },
        );
        d.frame = Some((2000.0, 3000.0));
        for _ in 0..3 {
            let start = std::time::Instant::now();
            d.insert(0, "x", &d.style_at(0).clone());
            let lines = e.layout(&d).lines.len();
            let layout = start.elapsed();
            e.render(&d);
            println!(
                "{} characters, {lines} lines: layout {layout:?}, rendering {:?}",
                d.text.len(),
                start.elapsed()
            );
        }
    }
}
