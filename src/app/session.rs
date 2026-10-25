//! What the tools were left at, kept from one session to the next
//! (tools.json, with the other settings): the current tool, each tool's
//! brush, the foreground and background colors, and the main tool options.
//! A missing, damaged or older file, or a value of the wrong kind, leaves
//! the defaults.

use std::ops::RangeInclusive;
use std::path::PathBuf;

use serde_json::{Map, Value, json};

use super::*;

fn settings_file() -> PathBuf {
    crate::config::dir().join("tools.json")
}

/// A tool's name in the file (stable across versions, unlike its place).
fn tool_name(tool: Tool) -> String {
    format!("{tool:?}")
}

fn tool_named(name: &str) -> Option<Tool> {
    Tool::ALL.into_iter().find(|&t| tool_name(t) == name)
}

fn rgba(c: Color32) -> Value {
    json!(c.to_srgba_unmultiplied())
}

/// The selection tools with edges of their own (Feather, Anti-alias).
const EDGE_TOOLS: [Tool; 6] = [
    Tool::Select,
    Tool::Ellipse,
    Tool::FreeLasso,
    Tool::PolyLasso,
    Tool::Lasso,
    Tool::MagicWand,
];

/// A choice from a list, by its name in the code (stable across versions,
/// unlike its place in the list).
fn name<T: std::fmt::Debug>(v: T) -> Value {
    json!(format!("{v:?}"))
}

/// The layer blend modes, as the tools' Mode lists offer them.
fn blend_modes() -> Vec<BlendMode> {
    BlendMode::GROUPS.concat()
}

const KERNINGS: [crate::text::Kerning; 2] =
    [crate::text::Kerning::Metrics, crate::text::Kerning::Zero];
const ANTI_ALIASES: [crate::text::AntiAlias; 3] = [
    crate::text::AntiAlias::None,
    crate::text::AntiAlias::Sharp,
    crate::text::AntiAlias::Smooth,
];
const ALIGNS: [crate::text::TextAlign; 4] = [
    crate::text::TextAlign::Left,
    crate::text::TextAlign::Center,
    crate::text::TextAlign::Right,
    crate::text::TextAlign::Justify,
];

/// The brush tip: Round, or the preset whose sampled tip it is.
fn tip_name(tip: &crate::paint::Tip) -> Value {
    use crate::paint::{Preset, Tip};
    match tip {
        Tip::Round => json!("Round"),
        Tip::Sampled(_) => Preset::ALL
            .into_iter()
            .find(|p| p.settings().tip == *tip)
            .map_or(json!("Round"), name),
    }
}

fn tip_named(saved: &str) -> Option<crate::paint::Tip> {
    use crate::paint::{Preset, Tip};
    if saved == "Round" {
        return Some(Tip::Round);
    }
    let preset = Preset::ALL
        .into_iter()
        .find(|p| format!("{p:?}") == saved)?;
    let tip = preset.settings().tip;
    matches!(tip, Tip::Sampled(_)).then_some(tip)
}

fn stop_color(c: crate::paint::gradient::StopColor) -> Value {
    use crate::paint::gradient::StopColor;
    match c {
        StopColor::Foreground => json!("Foreground"),
        StopColor::Background => json!("Background"),
        StopColor::Rgb(rgb) => json!(rgb),
    }
}

fn stop_color_of(v: &Value) -> Option<crate::paint::gradient::StopColor> {
    use crate::paint::gradient::StopColor;
    match v.as_str() {
        Some("Foreground") => Some(StopColor::Foreground),
        Some("Background") => Some(StopColor::Background),
        Some(_) => None,
        None => serde_json::from_value::<[u8; 3]>(v.clone())
            .ok()
            .map(StopColor::Rgb),
    }
}

fn gradient_value(g: &crate::paint::gradient::Gradient) -> Value {
    let colors: Vec<Value> = g
        .colors
        .iter()
        .map(|s| json!({"location": s.location, "midpoint": s.midpoint, "color": stop_color(s.color)}))
        .collect();
    let opacities: Vec<Value> = g
        .opacities
        .iter()
        .map(|s| json!({"location": s.location, "midpoint": s.midpoint, "opacity": s.opacity}))
        .collect();
    json!({"colors": colors, "opacities": opacities})
}

/// A saved gradient, whole (two stops of each kind at least) or not at
/// all.
fn gradient_of(s: &Saved) -> Option<crate::paint::gradient::Gradient> {
    use crate::paint::gradient::{ColorStop, Gradient, MIDPOINT_RANGE, OpacityStop};
    let colors = s
        .list("colors")
        .map(|c| {
            let c = Saved(c);
            Some(ColorStop {
                location: c.fraction("location")?,
                midpoint: c.number("midpoint", MIDPOINT_RANGE)?,
                color: stop_color_of(c.0.get("color")?)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    let opacities = s
        .list("opacities")
        .map(|o| {
            let o = Saved(o);
            Some(OpacityStop {
                location: o.fraction("location")?,
                midpoint: o.number("midpoint", MIDPOINT_RANGE)?,
                opacity: o.fraction("opacity")?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    (colors.len() >= 2 && opacities.len() >= 2).then_some(Gradient { colors, opacities })
}

/// A shape's fill or stroke paint: "Foreground" (`None`), "None" or a
/// color; a gradient isn't kept.
fn shape_paint(paint: Option<&crate::vector::Paint>) -> Option<Value> {
    use crate::vector::Paint;
    match paint {
        None => Some(json!("Foreground")),
        Some(Paint::None) => Some(json!("None")),
        Some(Paint::Color(c)) => Some(json!(c)),
        Some(Paint::Gradient(_)) => None,
    }
}

fn shape_paint_of(v: &Value) -> Option<Option<crate::vector::Paint>> {
    use crate::vector::Paint;
    match v.as_str() {
        Some("Foreground") => Some(None),
        Some("None") => Some(Some(Paint::None)),
        Some(_) => None,
        None => serde_json::from_value::<[u8; 3]>(v.clone())
            .ok()
            .map(|c| Some(Paint::Color(c))),
    }
}

/// Reads values out of the saved settings, each checked.
struct Saved<'a>(&'a Map<String, Value>);

impl Saved<'_> {
    fn number(&self, key: &str, range: RangeInclusive<f32>) -> Option<f32> {
        let v = self.0.get(key)?.as_f64()? as f32;
        v.is_finite().then(|| v.clamp(*range.start(), *range.end()))
    }

    fn fraction(&self, key: &str) -> Option<f32> {
        self.number(key, 0.0..=1.0)
    }

    fn flag(&self, key: &str) -> Option<bool> {
        self.0.get(key)?.as_bool()
    }

    /// An opaque color (the app never keeps another): its alpha, if any,
    /// is left out.
    fn color(&self, key: &str) -> Option<Color32> {
        let v: Vec<u8> = serde_json::from_value(self.0.get(key)?.clone()).ok()?;
        matches!(v.len(), 3 | 4).then(|| Color32::from_rgb(v[0], v[1], v[2]))
    }

    fn whole(&self, key: &str, range: RangeInclusive<u32>) -> Option<u32> {
        let v = self.0.get(key)?.as_u64()?;
        Some(v.clamp(u64::from(*range.start()), u64::from(*range.end())) as u32)
    }

    fn text(&self, key: &str) -> Option<String> {
        Some(self.0.get(key)?.as_str()?.to_owned())
    }

    /// One of `all`, by its name (see [`name`]).
    fn choice<T: Copy + std::fmt::Debug>(&self, key: &str, all: &[T]) -> Option<T> {
        let saved = self.0.get(key)?.as_str()?;
        all.iter().copied().find(|v| format!("{v:?}") == saved)
    }

    fn object(&self, key: &str) -> Option<Saved<'_>> {
        self.0.get(key)?.as_object().map(Saved)
    }

    fn tool(&self, key: &str) -> Option<Tool> {
        tool_named(self.0.get(key)?.as_str()?)
    }

    fn list(&self, key: &str) -> impl Iterator<Item = &Map<String, Value>> {
        let items = self.0.get(key).and_then(Value::as_array);
        items.into_iter().flatten().filter_map(Value::as_object)
    }
}

impl AtelierApp {
    /// The settings kept between sessions.
    pub(super) fn session_settings(&self) -> Value {
        let p = &self.paint;
        let r = &self.retouch;
        let brushes: Vec<Value> = self
            .tool_brushes
            .iter()
            .map(|&(tool, size, hardness)| {
                json!({"tool": tool_name(tool), "size": size, "hardness": hardness})
            })
            .collect();
        let mut groups: Vec<(usize, Tool)> =
            self.group_tool.iter().map(|(&g, &t)| (g, t)).collect();
        groups.sort_by_key(|g| g.0);
        let groups: Vec<Value> = groups
            .into_iter()
            .map(|(_, t)| json!(tool_name(t)))
            .collect();
        let edges: Vec<Value> = EDGE_TOOLS
            .into_iter()
            .map(|tool| {
                let e = self.sel.shape.edges_of(tool);
                json!({"tool": tool_name(tool), "feather": e.feather, "antialias": e.antialias})
            })
            .collect();
        let mut m = Map::new();
        let mut put = |key: &str, value: Value| {
            m.insert(key.to_owned(), value);
        };
        put("tool", json!(tool_name(self.tool)));
        put("brush_size", json!(self.brush_size));
        put("hardness", json!(self.hardness));
        put("brushes", brushes.into());
        put("groups", groups.into());
        put("foreground", rgba(self.primary));
        put("background", rgba(self.secondary));
        put("opacity", json!(self.opacity));
        put("flow", json!(p.flow));
        put("spacing", json!(p.spacing));
        put("angle", json!(p.angle));
        put("roundness", json!(p.roundness));
        put("smoothing", json!(p.smoothing));
        put("airbrush", json!(p.airbrush));
        put("pressure_size", json!(p.pressure_size));
        put("pressure_opacity", json!(p.pressure_opacity));
        put("exposure", json!(p.exposure));
        put("protect_tones", json!(p.protect_tones));
        put("saturate", json!(p.saturate));
        put("sponge_flow", json!(p.sponge_flow));
        put("vibrance", json!(p.vibrance));
        put("blur_strength", json!(p.blur_strength));
        put("sharpen_strength", json!(p.sharpen_strength));
        put("smudge_strength", json!(p.smudge_strength));
        put("sample_all", json!(p.sample_all));
        put("finger_painting", json!(p.finger_painting));
        put("gradient_opacity", json!(p.gradient.opacity));
        put("gradient_reverse", json!(p.gradient.reverse));
        put("gradient_dither", json!(p.gradient.dither));
        put("gradient_transparency", json!(p.gradient.transparency));
        put("bucket_tolerance", json!(self.bucket_tolerance));
        put("bucket_contiguous", json!(self.bucket_contiguous));
        put("bucket_opacity", json!(p.bucket.opacity));
        put("bucket_anti_alias", json!(p.bucket.anti_alias));
        put("bucket_all_layers", json!(p.bucket.all_layers));
        put("clone_opacity", json!(r.clone_opacity));
        put("clone_flow", json!(r.clone_flow));
        put("clone_aligned", json!(r.clone_aligned));
        put("heal_aligned", json!(r.heal_aligned));
        put("spot_sample_all", json!(r.spot_sample_all));
        put("pupil", json!(r.pupil));
        put("darken", json!(r.darken));
        put("wand_tolerance", json!(self.wand_tolerance));
        put("wand_contiguous", json!(self.wand_contiguous));
        put("wand_all_layers", json!(self.wand_all_layers));
        put("lasso_width", json!(self.lasso_width));
        put("lasso_frequency", json!(self.lasso_frequency));
        put("edges", edges.into());
        put("auto_select", json!(self.auto_select));
        put(
            "show_transform_controls",
            json!(self.show_transform_controls),
        );
        // The choices from lists.
        put("tip", tip_name(&p.tip));
        put("tone_range", name(p.range));
        put("gradient", gradient_value(&p.gradient.gradient));
        put("gradient_shape", name(p.gradient.shape));
        put("gradient_mode", name(p.gradient.mode));
        put("bucket_mode", name(p.bucket.mode));
        put("clone_sample", name(r.clone_sample));
        put("heal_sample", name(r.heal_sample));
        put("spot_type", name(r.spot_type));
        put("patch_mode", name(r.patch_mode));
        put("select_mode", name(self.select_mode));
        let o = &self.sel.shape;
        put("marquee_style", name(o.style));
        put("marquee_ratio", json!([o.ratio.0, o.ratio.1]));
        put("marquee_size", json!([o.size.0, o.size.1]));
        put("resample", name(self.resize_filter));
        let e = &self.export_settings;
        put(
            "export",
            json!({
                "format": name(e.format),
                "quality": e.quality,
                "resample": name(e.resample),
                "srgb": e.srgb,
                "embed_profile": e.embed_profile,
            }),
        );
        let t = &self.type_tool;
        let c = &t.style;
        put(
            "type",
            json!({
                "family": c.family,
                "weight": c.weight,
                "italic": c.italic,
                "stretch": c.stretch,
                "size": c.size,
                "leading": c.leading,
                "tracking": c.tracking,
                "kerning": name(c.kerning),
                "antialias": name(t.antialias),
                "align": name(t.paragraph.align),
            }),
        );
        let v = &self.vector;
        let mut shapes = Map::new();
        let mut keep = |key: &str, value: Value| {
            shapes.insert(key.to_owned(), value);
        };
        keep("pen_mode", name(v.pen_mode));
        keep("shape_mode", name(v.shape_mode));
        keep("auto_add", json!(v.auto_add));
        if let Some(fill) = shape_paint(v.fill.as_ref()) {
            keep("fill", fill);
        }
        if let Some(paint) = shape_paint(Some(&v.stroke.paint)) {
            keep("stroke", paint);
        }
        keep("stroke_width", json!(v.stroke.width));
        keep("stroke_align", name(v.stroke.align));
        keep("stroke_cap", name(v.stroke.cap));
        keep("stroke_join", name(v.stroke.join));
        keep("dash", json!(v.stroke.dash));
        keep("radius", json!(v.radius));
        keep("sides", json!(v.sides));
        keep("star", json!(v.star));
        keep("weight", json!(v.weight));
        keep("arrows", json!(v.arrows));
        keep("custom", name(v.custom));
        put("shapes", shapes.into());
        Value::Object(m)
    }

    /// Puts back what [`Self::session_settings`] kept. What is missing, of
    /// the wrong kind or unknown stays as it is; numbers are brought within
    /// their range.
    pub(super) fn restore_session(&mut self, settings: &Value) {
        let Some(map) = settings.as_object() else {
            return;
        };
        let s = Saved(map);
        macro_rules! set {
            ($place:expr, $value:expr) => {
                if let Some(v) = $value {
                    $place = v;
                }
            };
        }
        for b in s.list("brushes") {
            let b = Saved(b);
            if let (Some(tool), Some(size), Some(hardness)) = (
                b.tool("tool"),
                b.number("size", 1.0..=MAX_BRUSH_SIZE),
                b.fraction("hardness"),
            ) {
                self.tool_brushes.retain(|x| x.0 != tool);
                self.tool_brushes.push((tool, size, hardness));
            }
        }
        for name in map
            .get("groups")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(tool) = name.as_str().and_then(tool_named) {
                self.group_tool.insert(tool.group(), tool);
            }
        }
        if let Some(tool) = s.tool("tool") {
            self.tool = tool;
            self.group_tool.insert(tool.group(), tool);
        }
        set!(
            self.brush_size,
            s.number("brush_size", 1.0..=MAX_BRUSH_SIZE)
        );
        set!(self.hardness, s.fraction("hardness"));
        set!(self.primary, s.color("foreground"));
        set!(self.secondary, s.color("background"));
        set!(self.opacity, s.number("opacity", 0.01..=1.0));
        let p = &mut self.paint;
        set!(p.flow, s.number("flow", 0.01..=1.0));
        set!(p.spacing, s.number("spacing", 0.01..=10.0));
        set!(p.angle, s.number("angle", -180.0..=180.0));
        set!(p.roundness, s.number("roundness", 0.01..=1.0));
        set!(p.smoothing, s.fraction("smoothing"));
        set!(p.airbrush, s.flag("airbrush"));
        set!(p.pressure_size, s.flag("pressure_size"));
        set!(p.pressure_opacity, s.flag("pressure_opacity"));
        set!(p.exposure, s.number("exposure", 0.01..=1.0));
        set!(p.protect_tones, s.flag("protect_tones"));
        set!(p.saturate, s.flag("saturate"));
        set!(p.sponge_flow, s.number("sponge_flow", 0.01..=1.0));
        set!(p.vibrance, s.flag("vibrance"));
        set!(p.blur_strength, s.number("blur_strength", 0.01..=1.0));
        set!(p.sharpen_strength, s.number("sharpen_strength", 0.01..=1.0));
        set!(p.smudge_strength, s.fraction("smudge_strength"));
        set!(p.sample_all, s.flag("sample_all"));
        set!(p.finger_painting, s.flag("finger_painting"));
        set!(p.gradient.opacity, s.number("gradient_opacity", 0.01..=1.0));
        set!(p.gradient.reverse, s.flag("gradient_reverse"));
        set!(p.gradient.dither, s.flag("gradient_dither"));
        set!(p.gradient.transparency, s.flag("gradient_transparency"));
        set!(p.bucket.opacity, s.number("bucket_opacity", 0.01..=1.0));
        set!(p.bucket.anti_alias, s.flag("bucket_anti_alias"));
        set!(p.bucket.all_layers, s.flag("bucket_all_layers"));
        set!(self.bucket_tolerance, s.fraction("bucket_tolerance"));
        set!(self.bucket_contiguous, s.flag("bucket_contiguous"));
        let r = &mut self.retouch;
        set!(r.clone_opacity, s.number("clone_opacity", 0.01..=1.0));
        set!(r.clone_flow, s.number("clone_flow", 0.01..=1.0));
        set!(r.clone_aligned, s.flag("clone_aligned"));
        set!(r.heal_aligned, s.flag("heal_aligned"));
        set!(r.spot_sample_all, s.flag("spot_sample_all"));
        set!(r.pupil, s.number("pupil", 0.01..=1.0));
        set!(r.darken, s.number("darken", 0.01..=1.0));
        set!(self.wand_tolerance, s.fraction("wand_tolerance"));
        set!(self.wand_contiguous, s.flag("wand_contiguous"));
        set!(self.wand_all_layers, s.flag("wand_all_layers"));
        set!(self.lasso_width, s.number("lasso_width", 1.0..=40.0));
        set!(
            self.lasso_frequency,
            s.number("lasso_frequency", 0.0..=100.0)
        );
        for e in s.list("edges") {
            let e = Saved(e);
            let Some(tool) = e.tool("tool").filter(|t| EDGE_TOOLS.contains(t)) else {
                continue;
            };
            let edges = self.sel.shape.edges(tool);
            set!(edges.feather, e.number("feather", 0.0..=1000.0));
            set!(edges.antialias, e.flag("antialias"));
        }
        set!(self.auto_select, s.flag("auto_select"));
        set!(
            self.show_transform_controls,
            s.flag("show_transform_controls")
        );
        self.restore_choices(&s);
    }

    /// The choices from lists ([`Self::restore_session`]).
    fn restore_choices(&mut self, s: &Saved) {
        use super::retouch_tools::{PatchMode, Sample, SpotType};
        use crate::selection::SelectMode;
        macro_rules! set {
            ($place:expr, $value:expr) => {
                if let Some(v) = $value {
                    $place = v;
                }
            };
        }
        let modes = blend_modes();
        let p = &mut self.paint;
        set!(p.tip, s.text("tip").as_deref().and_then(tip_named));
        set!(
            p.range,
            s.choice("tone_range", &crate::paint::ToneRange::ALL)
        );
        set!(
            p.gradient.gradient,
            s.object("gradient").as_ref().and_then(gradient_of)
        );
        set!(
            p.gradient.shape,
            s.choice("gradient_shape", &crate::paint::gradient::Shape::ALL)
        );
        set!(p.gradient.mode, s.choice("gradient_mode", &modes));
        set!(p.bucket.mode, s.choice("bucket_mode", &modes));
        let r = &mut self.retouch;
        set!(r.clone_sample, s.choice("clone_sample", &Sample::ALL));
        set!(r.heal_sample, s.choice("heal_sample", &Sample::ALL));
        set!(
            r.spot_type,
            s.choice(
                "spot_type",
                &[SpotType::ContentAware, SpotType::ProximityMatch]
            )
        );
        set!(
            r.patch_mode,
            s.choice("patch_mode", &[PatchMode::Source, PatchMode::Destination])
        );
        set!(
            self.select_mode,
            s.choice(
                "select_mode",
                &[
                    SelectMode::Replace,
                    SelectMode::Add,
                    SelectMode::Subtract,
                    SelectMode::Intersect,
                ],
            )
        );
        let o = &mut self.sel.shape;
        set!(
            o.style,
            s.choice("marquee_style", &super::select_tools::MarqueeStyle::ALL)
        );
        if let Some(Ok([w, h])) =
            s.0.get("marquee_ratio")
                .map(|v| serde_json::from_value::<[f32; 2]>(v.clone()))
            && w.is_finite()
            && h.is_finite()
        {
            o.ratio = (w.clamp(0.001, 1000.0), h.clamp(0.001, 1000.0));
        }
        if let Some(Ok([w, h])) =
            s.0.get("marquee_size")
                .map(|v| serde_json::from_value::<[u32; 2]>(v.clone()))
        {
            o.size = (w.clamp(1, MAX_DIMENSION), h.clamp(1, MAX_DIMENSION));
        }
        set!(self.resize_filter, s.choice("resample", &ResizeFilter::ALL));
        if let Some(e) = s.object("export") {
            let x = &mut self.export_settings;
            set!(
                x.format,
                e.choice("format", &crate::document::ExportFormat::ALL)
            );
            set!(x.quality, e.whole("quality", 1..=100).map(|q| q as u8));
            set!(
                x.resample,
                e.choice(
                    "resample",
                    &[
                        crate::actions::Resample::Nearest,
                        crate::actions::Resample::Bilinear,
                        crate::actions::Resample::Bicubic,
                        crate::actions::Resample::Lanczos,
                    ],
                )
            );
            set!(x.srgb, e.flag("srgb"));
            set!(x.embed_profile, e.flag("embed_profile"));
        }
        if let Some(t) = s.object("type") {
            let tool = &mut self.type_tool;
            let c = &mut tool.style;
            set!(c.family, t.text("family"));
            set!(c.weight, t.whole("weight", 1..=1000).map(|v| v as u16));
            set!(c.italic, t.flag("italic"));
            set!(c.stretch, t.whole("stretch", 1..=9).map(|v| v as u16));
            set!(c.size, t.number("size", 0.1..=5000.0));
            if t.0.get("leading").is_some_and(Value::is_null) {
                c.leading = None;
            } else if let Some(l) = t.number("leading", 0.0..=10_000.0) {
                c.leading = Some(l);
            }
            set!(c.tracking, t.number("tracking", -1000.0..=10_000.0));
            set!(c.kerning, t.choice("kerning", &KERNINGS));
            c.sanitize();
            set!(tool.antialias, t.choice("antialias", &ANTI_ALIASES));
            set!(tool.paragraph.align, t.choice("align", &ALIGNS));
        }
        if let Some(sh) = s.object("shapes") {
            use super::vector_tools::ToolMode;
            use crate::vector::{Align, Cap, Custom, Join};
            let modes = [ToolMode::Shape, ToolMode::Path];
            let v = &mut self.vector;
            set!(v.pen_mode, sh.choice("pen_mode", &modes));
            set!(v.shape_mode, sh.choice("shape_mode", &modes));
            set!(v.auto_add, sh.flag("auto_add"));
            set!(v.fill, sh.0.get("fill").and_then(shape_paint_of));
            if let Some(Some(paint)) = sh.0.get("stroke").and_then(shape_paint_of) {
                v.stroke.paint = paint;
            }
            set!(v.stroke.width, sh.number("stroke_width", 0.0..=1000.0));
            set!(
                v.stroke.align,
                sh.choice(
                    "stroke_align",
                    &[Align::Inside, Align::Center, Align::Outside]
                )
            );
            set!(
                v.stroke.cap,
                sh.choice("stroke_cap", &[Cap::Butt, Cap::Round, Cap::Square])
            );
            set!(
                v.stroke.join,
                sh.choice("stroke_join", &[Join::Miter, Join::Round, Join::Bevel])
            );
            match sh.0.get("dash") {
                Some(Value::Null) => v.stroke.dash = None,
                Some(d) => {
                    if let Ok([a, b]) = serde_json::from_value::<[f32; 2]>(d.clone())
                        && a.is_finite()
                        && b.is_finite()
                    {
                        v.stroke.dash = Some([a.clamp(0.0, 100.0), b.clamp(0.0, 100.0)]);
                    }
                }
                None => {}
            }
            set!(v.radius, sh.number("radius", 0.0..=10_000.0));
            set!(v.sides, sh.whole("sides", 3..=100));
            set!(v.star, sh.number("star", 0.01..=1.0));
            set!(v.weight, sh.number("weight", 0.1..=1000.0));
            if let Some(Ok(arrows)) =
                sh.0.get("arrows")
                    .map(|a| serde_json::from_value::<[bool; 2]>(a.clone()))
            {
                v.arrows = arrows;
            }
            set!(v.custom, sh.choice("custom", &Custom::ALL));
        }
    }

    /// At start: the settings of the last session.
    pub(super) fn load_session(&mut self) {
        // Tests never read the user's settings.
        if cfg!(test) {
            return;
        }
        let saved = std::fs::read(settings_file())
            .ok()
            .and_then(|json| serde_json::from_slice::<Value>(&json).ok());
        if let Some(saved) = saved {
            self.restore_session(&saved);
        }
        self.session_saved = Some(self.session_settings());
    }

    /// Writes the settings when they changed, once the pointer is up (not
    /// at every step of a slider's drag).
    pub(super) fn save_session(&mut self, ctx: &egui::Context) {
        if cfg!(test) || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let now = self.session_settings();
        if self.session_saved.as_ref() == Some(&now) {
            return;
        }
        if let Ok(json) = serde_json::to_vec_pretty(&now) {
            let _ = std::fs::create_dir_all(crate::config::dir());
            // Whole or not at all: a crash midway leaves the last one.
            let temp = settings_file().with_extension("json.tmp");
            if std::fs::write(&temp, json).is_ok() {
                let _ = std::fs::rename(&temp, settings_file());
            }
        }
        self.session_saved = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;

    fn app() -> Harness<'static, AtelierApp> {
        Harness::builder()
            .with_size([1280.0, 800.0])
            .build_eframe(|cc| AtelierApp::new(cc, None))
    }

    #[test]
    fn tools_colors_and_options_come_back_in_the_next_session() {
        let mut h = app();
        let a = h.state_mut();
        a.set_tool(Tool::Brush);
        a.brush_size = 80.0;
        a.hardness = 0.25;
        a.set_tool(Tool::Eraser);
        a.brush_size = 30.0;
        a.primary = Color32::from_rgb(200, 20, 40);
        a.secondary = Color32::from_rgb(10, 200, 90);
        a.opacity = 0.5;
        a.paint.flow = 0.4;
        a.auto_select = true;
        a.show_transform_controls = true;
        a.wand_tolerance = 0.5;
        a.wand_contiguous = false;
        a.sel.shape.edges(Tool::FreeLasso).feather = 12.0;
        a.set_tool(Tool::PolyLasso);
        let saved = serde_json::to_string(&a.session_settings()).unwrap();

        let mut next = app();
        let b = next.state_mut();
        b.restore_session(&serde_json::from_str(&saved).unwrap());
        assert_eq!(b.tool, Tool::PolyLasso);
        assert_eq!(b.group_shown(Tool::Eraser.group()), Tool::Eraser);
        assert_eq!(b.primary, Color32::from_rgb(200, 20, 40));
        assert_eq!(b.secondary, Color32::from_rgb(10, 200, 90));
        assert_eq!((b.opacity, b.paint.flow), (0.5, 0.4));
        assert!(b.auto_select && b.show_transform_controls);
        assert_eq!((b.wand_tolerance, b.wand_contiguous), (0.5, false));
        assert_eq!(b.sel.shape.edges(Tool::FreeLasso).feather, 12.0);
        assert_eq!(b.sel.shape.edges(Tool::Lasso).feather, 0.0);
        // Each painting tool's brush.
        b.set_tool(Tool::Brush);
        assert_eq!((b.brush_size, b.hardness), (80.0, 0.25));
        b.set_tool(Tool::Eraser);
        assert_eq!(b.brush_size, 30.0);
    }

    #[test]
    fn the_choices_from_lists_come_back_too() {
        use super::super::retouch_tools::{PatchMode, Sample, SpotType};
        use crate::paint::gradient::{Shape, StopColor};
        use crate::paint::{Preset, ToneRange};
        let mut h = app();
        let a = h.state_mut();
        let (mut size, mut hardness) = (a.brush_size, a.hardness);
        a.paint
            .apply_preset(Preset::Chalk, &mut size, &mut hardness);
        a.paint.range = ToneRange::Highlights;
        a.paint.gradient.shape = Shape::Radial;
        a.paint.gradient.mode = BlendMode::Multiply;
        a.paint.gradient.gradient.colors[1].color = StopColor::Rgb([10, 20, 30]);
        a.paint.gradient.gradient.colors[0].location = 0.2;
        a.paint.bucket.mode = BlendMode::Screen;
        a.retouch.clone_sample = Sample::All;
        a.retouch.heal_sample = Sample::CurrentAndBelow;
        a.retouch.spot_type = SpotType::ProximityMatch;
        a.retouch.patch_mode = PatchMode::Destination;
        a.select_mode = crate::selection::SelectMode::Add;
        a.sel.shape.style = super::super::select_tools::MarqueeStyle::FixedSize;
        a.sel.shape.size = (300, 200);
        a.resize_filter = ResizeFilter::Bicubic;
        a.export_settings.format = crate::document::ExportFormat::Jpeg;
        a.export_settings.quality = 70;
        a.type_tool.style.family = "Some Serif".into();
        a.type_tool.style.size = 72.0;
        a.type_tool.style.tracking = 50.0;
        a.type_tool.antialias = crate::text::AntiAlias::Smooth;
        a.vector.fill = Some(crate::vector::Paint::Color([1, 2, 3]));
        a.vector.stroke.paint = crate::vector::Paint::Color([4, 5, 6]);
        a.vector.stroke.width = 3.0;
        a.vector.stroke.join = crate::vector::Join::Round;
        a.vector.stroke.dash = Some([2.0, 1.0]);
        a.vector.sides = 7;
        a.vector.custom = crate::vector::Custom::Heart;
        let saved = serde_json::to_string(&a.session_settings()).unwrap();

        let mut next = app();
        let b = next.state_mut();
        b.restore_session(&serde_json::from_str(&saved).unwrap());
        assert_eq!(Preset::of(&b.paint.tip, 1.0), Some(Preset::Chalk));
        assert_eq!(b.paint.range, ToneRange::Highlights);
        assert_eq!(b.paint.gradient.shape, Shape::Radial);
        assert_eq!(b.paint.gradient.mode, BlendMode::Multiply);
        assert_eq!(b.paint.gradient.gradient, a_gradient(&saved));
        assert_eq!(
            b.paint.gradient.gradient.colors[1].color,
            StopColor::Rgb([10, 20, 30])
        );
        assert_eq!(b.paint.bucket.mode, BlendMode::Screen);
        assert_eq!(b.retouch.clone_sample, Sample::All);
        assert_eq!(b.retouch.heal_sample, Sample::CurrentAndBelow);
        assert_eq!(b.retouch.spot_type, SpotType::ProximityMatch);
        assert_eq!(b.retouch.patch_mode, PatchMode::Destination);
        assert_eq!(b.select_mode, crate::selection::SelectMode::Add);
        assert_eq!(
            b.sel.shape.style,
            super::super::select_tools::MarqueeStyle::FixedSize
        );
        assert_eq!(b.sel.shape.size, (300, 200));
        assert_eq!(b.resize_filter, ResizeFilter::Bicubic);
        assert_eq!(
            b.export_settings.format,
            crate::document::ExportFormat::Jpeg
        );
        assert_eq!(b.export_settings.quality, 70);
        let t = &b.type_tool;
        assert_eq!(
            (t.style.family.as_str(), t.style.size),
            ("Some Serif", 72.0)
        );
        assert_eq!(t.style.tracking, 50.0);
        assert_eq!(t.antialias, crate::text::AntiAlias::Smooth);
        let v = &b.vector;
        assert_eq!(v.fill, Some(crate::vector::Paint::Color([1, 2, 3])));
        assert_eq!(v.stroke.paint, crate::vector::Paint::Color([4, 5, 6]));
        assert_eq!(v.stroke.width, 3.0);
        assert_eq!(v.stroke.join, crate::vector::Join::Round);
        assert_eq!(v.stroke.dash, Some([2.0, 1.0]));
        assert_eq!((v.sides, v.custom), (7, crate::vector::Custom::Heart));
    }

    /// The gradient as written in `saved`, read back.
    fn a_gradient(saved: &str) -> crate::paint::gradient::Gradient {
        let v: Value = serde_json::from_str(saved).unwrap();
        gradient_of(&Saved(v["gradient"].as_object().unwrap())).unwrap()
    }

    #[test]
    fn a_saved_foreground_is_always_opaque() {
        let mut h = app();
        let a = h.state_mut();
        a.restore_session(&json!({"foreground": [10, 20, 30, 0], "background": [1, 2, 3]}));
        assert_eq!(a.primary, Color32::from_rgb(10, 20, 30));
        assert_eq!(a.secondary, Color32::from_rgb(1, 2, 3));
        // Damaged choices: the defaults stay.
        let before = a.session_settings();
        a.restore_session(&json!({
            "tip": "Crayon",
            "tone_range": 3,
            "gradient": {"colors": [{"location": 0.0}], "opacities": []},
            "select_mode": "Sideways",
            "export": {"format": "bmp", "quality": 900},
            "type": {"size": "big", "weight": -3},
            "shapes": {"fill": "plaid", "sides": 1, "dash": "yes"},
        }));
        let after = a.session_settings();
        for key in ["tip", "tone_range", "gradient", "select_mode", "type"] {
            assert_eq!(after[key], before[key], "{key}");
        }
        // Out of range: brought within it.
        assert_eq!(after["export"]["quality"], json!(100));
        assert_eq!(after["export"]["format"], before["export"]["format"]);
        assert_eq!(after["shapes"]["sides"], json!(3));
        assert_eq!(after["shapes"]["fill"], before["shapes"]["fill"]);
    }

    #[test]
    fn a_damaged_or_older_file_leaves_the_defaults() {
        let mut h = app();
        let a = h.state_mut();
        let defaults = a.session_settings();
        for bad in [
            json!(null),
            json!([1, 2, 3]),
            json!("tools"),
            json!({"tool": "Spray Can", "brush_size": "big", "foreground": [1, 2]}),
            json!({"brushes": [{"tool": "Brush", "size": null}], "groups": 3, "edges": {}}),
            json!({"opacity": f64::MAX, "hardness": -4.0}),
        ] {
            a.restore_session(&bad);
        }
        let now = a.session_settings();
        // Out of range: brought within it.
        assert_eq!(now["opacity"], json!(1.0));
        assert_eq!(now["hardness"], json!(0.0));
        let unchanged = |key: &str| now[key] == defaults[key];
        for key in [
            "tool",
            "brush_size",
            "foreground",
            "brushes",
            "groups",
            "edges",
        ] {
            assert!(unchanged(key), "{key}");
        }
    }
}
