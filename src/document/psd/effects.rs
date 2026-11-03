//! Layer styles: the "lfx2" descriptor (and "lmfx", with several effects
//! of a kind) both ways, for the styles Atelier has.

use super::adjust::{gradient_fill, gradient_fill_items};
use super::pattern::{Patterns, fill_items, fill_of};
use super::descriptor::{Descriptor, Value, angle, color_of, enumerated, percent, pixels, rgb};
use super::{descriptor_blend_key, descriptor_blend_of};
use crate::compose::styles::{Effects, GlowFx, ShadowFx, Style};
use crate::document::BlendMode;

/// Whether an effect is on.
fn enab(d: &Descriptor) -> bool {
    d.bool(b"enab").unwrap_or(false)
}

/// Whether an effect holds settings: on, or added to the layer and turned
/// off (`present` is missing in older files).
fn held(d: &Descriptor) -> bool {
    enab(d) || d.bool(b"present").unwrap_or(true)
}

/// The effect of `class` in `fx`: its single entry, or the first enabled
/// one of its list (the first one if none is); and whether others with
/// settings had to be left out.
fn effect<'a>(fx: &'a Descriptor, class: &[u8], multi: &[u8]) -> (Option<&'a Descriptor>, bool) {
    if let Some(list) = fx.list(multi) {
        let all: Vec<&Descriptor> = list
            .iter()
            .filter_map(|v| match v {
                Value::Object(d) => Some(d),
                _ => None,
            })
            .collect();
        let kept = all.iter().copied().find(|d| enab(d)).or(all.first().copied());
        let lost = all
            .iter()
            .any(|d| held(d) && !kept.is_some_and(|k| std::ptr::eq(k, *d)));
        return (kept, lost);
    }
    (fx.object(class), false)
}

fn color4(d: &Descriptor, key: &[u8], default: [u8; 4]) -> [u8; 4] {
    d.object(key)
        .and_then(color_of)
        .map_or(default, |[r, g, b]| [r, g, b, 255])
}

fn mode(d: &Descriptor, key: &[u8], default: BlendMode) -> BlendMode {
    d.enumerated(key).map_or(default, descriptor_blend_of)
}

fn opacity(d: &Descriptor, key: &[u8], default: f32) -> f32 {
    d.num(key)
        .map_or(default, |v| (v as f32 / 100.0).clamp(0.0, 1.0))
}

/// The styles of a layer from its effects descriptor (`global`: the
/// document's light angle), the ones turned off with their settings too;
/// and whether some had to be simplified or left out.
pub fn read(fx: &Descriptor, global: f64, patterns: &Patterns) -> (Effects, bool) {
    let mut e = Effects::default();
    // All styles hidden at once: Atelier has no such switch, so they come
    // back each turned off (which were on is lost).
    let master = fx.bool(b"masterFXSwitch").unwrap_or(true);
    let hidden = std::cell::Cell::new(false);
    let on = |d: &Descriptor| {
        hidden.set(hidden.get() || (enab(d) && !master));
        enab(d) && master
    };
    let scale = fx.num(b"Scl ").unwrap_or(100.0) as f32 / 100.0;
    let px = |d: &Descriptor, key: &[u8], default: f32| {
        let v = d.num(key).map(|v| v as f32 * scale);
        v.filter(|v| v.is_finite()).unwrap_or(default)
    };
    let light = |d: &Descriptor| {
        if d.bool(b"uglg").unwrap_or(false) {
            global
        } else {
            d.num(b"lagl").unwrap_or(global)
        }
    };
    let mut simplified = false;
    let shadow = |d: &Descriptor, enabled: bool, default: ShadowFx| {
        let a = light(d).to_radians();
        // Photoshop's ranges: distance to 30000 px, size to 250.
        let dist = px(d, b"Dstn", 0.0).clamp(0.0, 30000.0);
        ShadowFx {
            enabled,
            color: color4(d, b"Clr ", default.color),
            opacity: opacity(d, b"Opct", default.opacity),
            dx: -dist * a.cos() as f32,
            dy: dist * a.sin() as f32,
            blur: px(d, b"blur", default.blur).clamp(0.0, 250.0),
            blend: mode(d, b"Md  ", default.blend),
        }
    };
    let (d, more) = effect(fx, b"DrSh", b"dropShadowMulti");
    simplified |= more;
    if let Some(d) = d {
        e.shadow = shadow(d, on(d), e.shadow);
    }
    let (d, more) = effect(fx, b"IrSh", b"innerShadowMulti");
    simplified |= more;
    if let Some(d) = d {
        e.inner_shadow = shadow(d, on(d), e.inner_shadow);
    }
    let glow = |d: &Descriptor, enabled: bool, default: GlowFx| GlowFx {
        enabled,
        color: color4(d, b"Clr ", default.color),
        opacity: opacity(d, b"Opct", default.opacity),
        size: px(d, b"blur", default.size).clamp(0.0, 250.0),
        spread: (d.num(b"Ckmt").unwrap_or(0.0) as f32 / 100.0).clamp(0.0, 1.0),
        blend: mode(d, b"Md  ", default.blend),
    };
    if let Some(d) = fx.object(b"OrGl") {
        e.outer_glow = glow(d, on(d), e.outer_glow);
        simplified |= held(d) && d.object(b"Clr ").is_none();
    }
    if let Some(d) = fx.object(b"IrGl") {
        e.inner_glow = glow(d, on(d), e.inner_glow);
        simplified |= held(d)
            && (d.object(b"Clr ").is_none() || d.enumerated(b"glwS") == Some(b"SrcC"));
    }
    let (d, more) = effect(fx, b"SoFi", b"solidFillMulti");
    simplified |= more;
    if let Some(d) = d {
        let c = &mut e.color_overlay;
        c.enabled = on(d);
        c.color = color4(d, b"Clr ", c.color);
        c.opacity = opacity(d, b"Opct", c.opacity);
        c.blend = mode(d, b"Md  ", c.blend);
    }
    let (d, more) = effect(fx, b"GrFl", b"gradientFillMulti");
    simplified |= more;
    if let Some(d) = d {
        let g = &mut e.gradient_overlay;
        match gradient_fill(d) {
            Some(fill) => {
                g.enabled = on(d);
                g.fill = fill;
                g.opacity = opacity(d, b"Opct", g.opacity);
                g.blend = mode(d, b"Md  ", g.blend);
            }
            None => simplified |= held(d),
        }
    }
    let (d, more) = effect(fx, b"FrFX", b"frameFXMulti");
    simplified |= more;
    if let Some(d) = d {
        let s = &mut e.stroke;
        s.enabled = on(d);
        s.color = color4(d, b"Clr ", s.color);
        s.opacity = opacity(d, b"Opct", s.opacity);
        s.width = px(d, b"Sz  ", s.width).clamp(0.0, 250.0);
        s.blend = mode(d, b"Md  ", s.blend);
        simplified |= held(d)
            && (d.enumerated(b"Styl").is_some_and(|p| p != b"OutF")
                || d.enumerated(b"PntT").is_some_and(|p| p != b"SClr"));
    }
    if let Some(d) = fx.object(b"ebbl") {
        let b = &mut e.bevel;
        b.enabled = on(d);
        b.depth = d
            .num(b"srgR")
            .map_or(b.depth, |v| (v as f32 / 100.0).clamp(0.0, 10.0));
        b.size = px(d, b"blur", b.size).clamp(0.0, 250.0);
        b.soften = px(d, b"Sftn", b.soften).clamp(0.0, 16.0);
        b.angle = light(d) as f32;
        b.altitude = d
            .num(b"Lald")
            .map_or(b.altitude, |v| (v as f32).clamp(0.0, 90.0));
        b.up = d.enumerated(b"bvlD") != Some(b"Out ");
        b.highlight = color4(d, b"hglC", b.highlight);
        b.highlight_opacity = opacity(d, b"hglO", b.highlight_opacity);
        b.highlight_blend = mode(d, b"hglM", b.highlight_blend);
        b.shadow = color4(d, b"sdwC", b.shadow);
        b.shadow_opacity = opacity(d, b"sdwO", b.shadow_opacity);
        b.shadow_blend = mode(d, b"sdwM", b.shadow_blend);
        simplified |= held(d) && d.enumerated(b"bvlS").is_some_and(|s| s != b"InrB");
    }
    if let Some(d) = fx.object(b"patternFill") {
        let p = &mut e.pattern_overlay;
        match fill_of(d, patterns) {
            Some(fill) => {
                p.enabled = on(d);
                p.fill = fill;
                p.opacity = opacity(d, b"Opct", p.opacity);
                p.blend = mode(d, b"Md  ", p.blend);
            }
            None => simplified |= held(d),
        }
    }
    // Satin: not in Atelier.
    simplified |= fx.object(b"ChFX").is_some_and(held);
    (e, simplified || hidden.get())
}

/// A linear contour, as Photoshop writes it.
fn linear_contour() -> Value {
    let point = |v: f64| {
        Value::Object(
            Descriptor::new(b"CrPt")
                .with(b"Hrzn", Value::Double(v))
                .with(b"Vrtc", Value::Double(v)),
        )
    };
    Value::Object(
        Descriptor::new(b"ShpC")
            .with(b"Nm  ", Value::Text("Linear".into()))
            .with(b"Crv ", Value::List(vec![point(0.0), point(255.0)])),
    )
}

fn base(class: &[u8], enabled: bool, blend: BlendMode) -> Descriptor {
    Descriptor::new(class)
        .with(b"enab", Value::Bool(enabled))
        .with(b"present", Value::Bool(true))
        .with(b"showInDialog", Value::Bool(true))
        .with(b"Md  ", enumerated(b"BlnM", descriptor_blend_key(blend)))
}

fn color3([r, g, b, _]: [u8; 4]) -> Value {
    rgb([r, g, b])
}

fn shadow_descriptor(class: &[u8], s: &ShadowFx) -> Descriptor {
    let distance = (s.dx * s.dx + s.dy * s.dy).sqrt();
    let light = (s.dy as f64).atan2(-s.dx as f64).to_degrees();
    let d = base(class, s.enabled, s.blend)
        .with(b"Clr ", color3(s.color))
        .with(b"Opct", percent(s.opacity as f64 * 100.0))
        .with(b"uglg", Value::Bool(false))
        .with(b"lagl", angle(light))
        .with(b"Dstn", pixels(distance as f64))
        .with(b"Ckmt", pixels(0.0))
        .with(b"blur", pixels(s.blur.round() as f64))
        .with(b"Nose", percent(0.0))
        .with(b"AntA", Value::Bool(false))
        .with(b"TrnS", linear_contour());
    if class == b"DrSh" {
        d.with(b"layerConceals", Value::Bool(true))
    } else {
        d
    }
}

fn glow_descriptor(class: &[u8], g: &GlowFx) -> Descriptor {
    let d = base(class, g.enabled, g.blend)
        .with(b"Clr ", color3(g.color))
        .with(b"Opct", percent(g.opacity as f64 * 100.0))
        .with(b"GlwT", enumerated(b"BETE", b"SfBL"))
        .with(b"Ckmt", pixels((g.spread * 100.0).round() as f64))
        .with(b"blur", pixels(g.size.round() as f64))
        .with(b"Nose", percent(0.0))
        .with(b"ShdN", percent(0.0))
        .with(b"AntA", Value::Bool(false))
        .with(b"TrnS", linear_contour())
        .with(b"Inpr", percent(50.0));
    if class == b"IrGl" {
        d.with(b"glwS", enumerated(b"IGSr", b"SrcE"))
    } else {
        d
    }
}

/// The effects descriptor of the styles that are on, or off with settings
/// of their own (with `enab` false); `None` if there are none.
pub fn write(e: &Effects) -> Option<Descriptor> {
    if !e.any_kept() {
        return None;
    }
    let mut fx = Descriptor::new(b"null")
        .with(b"Scl ", percent(100.0))
        .with(b"masterFXSwitch", Value::Bool(true));
    if e.kept(Style::DropShadow) {
        fx = fx.with(
            b"DrSh",
            Value::Object(shadow_descriptor(b"DrSh", &e.shadow)),
        );
    }
    if e.kept(Style::InnerShadow) {
        fx = fx.with(
            b"IrSh",
            Value::Object(shadow_descriptor(b"IrSh", &e.inner_shadow)),
        );
    }
    if e.kept(Style::OuterGlow) {
        fx = fx.with(
            b"OrGl",
            Value::Object(glow_descriptor(b"OrGl", &e.outer_glow)),
        );
    }
    if e.kept(Style::InnerGlow) {
        fx = fx.with(
            b"IrGl",
            Value::Object(glow_descriptor(b"IrGl", &e.inner_glow)),
        );
    }
    let b = &e.bevel;
    if e.kept(Style::Bevel) {
        let d = Descriptor::new(b"ebbl")
            .with(b"enab", Value::Bool(b.enabled))
            .with(b"present", Value::Bool(true))
            .with(b"showInDialog", Value::Bool(true))
            .with(
                b"hglM",
                enumerated(b"BlnM", descriptor_blend_key(b.highlight_blend)),
            )
            .with(b"hglC", color3(b.highlight))
            .with(b"hglO", percent(b.highlight_opacity as f64 * 100.0))
            .with(
                b"sdwM",
                enumerated(b"BlnM", descriptor_blend_key(b.shadow_blend)),
            )
            .with(b"sdwC", color3(b.shadow))
            .with(b"sdwO", percent(b.shadow_opacity as f64 * 100.0))
            .with(b"bvlT", enumerated(b"bvlT", b"SfBL"))
            .with(b"bvlS", enumerated(b"BESl", b"InrB"))
            .with(b"uglg", Value::Bool(false))
            .with(b"lagl", angle(b.angle as f64))
            .with(b"Lald", angle(b.altitude as f64))
            .with(b"srgR", percent((b.depth * 100.0).round() as f64))
            .with(b"blur", pixels(b.size.round() as f64))
            .with(
                b"bvlD",
                enumerated(b"BESs", if b.up { b"In  " } else { b"Out " }),
            )
            .with(b"TrnS", linear_contour())
            .with(b"antialiasGloss", Value::Bool(false))
            .with(b"Sftn", pixels(b.soften.round() as f64))
            .with(b"useShape", Value::Bool(false))
            .with(b"useTexture", Value::Bool(false));
        fx = fx.with(b"ebbl", Value::Object(d));
    }
    let c = &e.color_overlay;
    if e.kept(Style::ColorOverlay) {
        let d = base(b"SoFi", c.enabled, c.blend)
            .with(b"Clr ", color3(c.color))
            .with(b"Opct", percent(c.opacity as f64 * 100.0));
        fx = fx.with(b"SoFi", Value::Object(d));
    }
    let g = &e.gradient_overlay;
    if e.kept(Style::GradientOverlay) {
        let d = base(b"GrFl", g.enabled, g.blend).with(b"Opct", percent(g.opacity as f64 * 100.0));
        fx = fx.with(b"GrFl", Value::Object(gradient_fill_items(&g.fill, d)));
    }
    let p = &e.pattern_overlay;
    if e.kept(Style::PatternOverlay) {
        let d = base(b"patternFill", p.enabled, p.blend).with(b"Opct", percent(p.opacity as f64 * 100.0));
        fx = fx.with(b"patternFill", Value::Object(fill_items(&p.fill, d, b"Algn")));
    }
    let s = &e.stroke;
    if e.kept(Style::Stroke) {
        let d = base(b"FrFX", s.enabled, s.blend)
            .with(b"Styl", enumerated(b"FStl", b"OutF"))
            .with(b"PntT", enumerated(b"FrFl", b"SClr"))
            .with(b"Opct", percent(s.opacity as f64 * 100.0))
            .with(b"Sz  ", pixels(s.width.round() as f64))
            .with(b"Clr ", color3(s.color));
        fx = fx.with(b"FrFX", Value::Object(d));
    }
    Some(fx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compose::gradient::GradientFill;
    use crate::paint::gradient::{Preset, Shape};

    #[test]
    fn styles_round_trip() {
        let mut e = Effects {
            shadow: ShadowFx {
                enabled: true,
                color: [10, 20, 30, 255],
                opacity: 0.5,
                dx: 3.0,
                dy: 4.0,
                blur: 7.0,
                blend: BlendMode::Multiply,
            },
            ..Effects::default()
        };
        e.inner_shadow.enabled = true;
        e.outer_glow = GlowFx {
            enabled: true,
            spread: 0.2,
            size: 9.0,
            ..e.outer_glow
        };
        e.inner_glow.enabled = true;
        e.color_overlay.enabled = true;
        e.color_overlay.blend = BlendMode::Overlay;
        e.gradient_overlay.enabled = true;
        e.gradient_overlay.fill = GradientFill {
            gradient: Preset::Copper.gradient(),
            style: Shape::Radial,
            angle: 45.0,
            scale: 0.5,
            reverse: true,
        };
        e.stroke.enabled = true;
        e.stroke.width = 4.0;
        e.bevel.enabled = true;
        e.bevel.up = false;
        e.bevel.depth = 2.5;
        let fx = write(&e).unwrap();
        let mut w = super::super::bytes::Writer::default();
        fx.write(&mut w);
        let fx = Descriptor::read(&mut super::super::bytes::Reader::new(&w.data)).unwrap();
        let (back, simplified) = read(&fx, 30.0, &Patterns::new());
        assert!(!simplified);
        assert!((back.shadow.dx - 3.0).abs() < 0.01 && (back.shadow.dy - 4.0).abs() < 0.01);
        assert_eq!(
            (
                back.shadow.color,
                back.shadow.opacity,
                back.shadow.blur,
                back.shadow.blend
            ),
            (
                e.shadow.color,
                e.shadow.opacity,
                e.shadow.blur,
                e.shadow.blend
            )
        );
        assert_eq!(back.outer_glow, e.outer_glow);
        assert_eq!(back.inner_glow, e.inner_glow);
        assert_eq!(back.color_overlay, e.color_overlay);
        assert_eq!(back.stroke, e.stroke);
        assert_eq!(back.bevel, e.bevel);
        let (g, h) = (&back.gradient_overlay.fill, &e.gradient_overlay.fill);
        assert_eq!(
            (g.style, g.angle, g.scale, g.reverse),
            (h.style, h.angle, h.scale, h.reverse)
        );
        assert!(back.inner_shadow.enabled && back.gradient_overlay.enabled);
        assert!(write(&Effects::default()).is_none());
    }

    fn round_trip(fx: &Descriptor) -> Descriptor {
        let mut w = super::super::bytes::Writer::default();
        fx.write(&mut w);
        Descriptor::read(&mut super::super::bytes::Reader::new(&w.data)).unwrap()
    }

    #[test]
    fn styles_turned_off_are_written_with_enab_false() {
        let mut e = Effects::default();
        e.shadow.blur = 12.0;
        e.color_overlay.color = [1, 2, 3, 255];
        let fx = round_trip(&write(&e).expect("settings to keep"));
        assert_eq!(fx.object(b"DrSh").and_then(|d| d.bool(b"enab")), Some(false));
        assert!(fx.object(b"OrGl").is_none());
        let (back, simplified) = read(&fx, 30.0, &Patterns::new());
        assert!(!simplified);
        assert!((back.shadow.dx - 4.0).abs() < 0.01 && (back.shadow.dy - 4.0).abs() < 0.01);
        assert_eq!((back.shadow.blur, back.shadow.opacity), (12.0, e.shadow.opacity));
        assert_eq!(back.color_overlay, e.color_overlay);
        assert!(!back.any());
    }

    #[test]
    fn styles_hidden_all_at_once_come_back_off_with_their_settings() {
        let mut e = Effects::default();
        e.stroke.enabled = true;
        e.stroke.width = 7.0;
        let stroke = write(&e).unwrap().object(b"FrFX").unwrap().clone();
        let fx = Descriptor::new(b"null")
            .with(b"masterFXSwitch", Value::Bool(false))
            .with(b"FrFX", Value::Object(stroke));
        let (back, simplified) = read(&round_trip(&fx), 30.0, &Patterns::new());
        assert!(simplified, "which styles were on is lost");
        assert!(!back.stroke.enabled && back.stroke.width == 7.0);
    }

    #[test]
    fn a_list_of_effects_keeps_the_one_on_or_else_the_first() {
        let mut off = Effects::default();
        off.shadow.blur = 9.0;
        let mut on = off.clone();
        on.shadow.enabled = true;
        on.shadow.blur = 3.0;
        let entry = |e: &Effects| Value::Object(shadow_descriptor(b"DrSh", &e.shadow));
        let multi = |list: Vec<Value>| {
            Descriptor::new(b"null").with(b"dropShadowMulti", Value::List(list))
        };
        let (back, simplified) = read(
            &round_trip(&multi(vec![entry(&off), entry(&on)])),
            30.0,
            &Patterns::new(),
        );
        assert!(simplified && back.shadow.enabled && back.shadow.blur == 3.0);
        let (back, simplified) = read(&round_trip(&multi(vec![entry(&off)])), 30.0, &Patterns::new());
        assert!(!simplified && !back.shadow.enabled && back.shadow.blur == 9.0);
    }
}
