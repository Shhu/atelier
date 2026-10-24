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

    fn color(&self, key: &str) -> Option<Color32> {
        let v: [u8; 4] = serde_json::from_value(self.0.get(key)?.clone()).ok()?;
        Some(Color32::from_rgba_unmultiplied(v[0], v[1], v[2], v[3]))
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
