//! Profile icons: Claude's own icon, recolored to the profile's color, with a small white
//! badge holding a symbol or letter. Written as PNG, ICO and ICNS for every platform.
//!
//! Claude's icon is read from the Claude Desktop install on this computer (see
//! [`claude_source`]); none of Anthropic's artwork ships with this project. If it can't be
//! found, profiles get a plain rounded square in their color with the same badge.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, OutlineCurve};
use anyhow::{Context, Result};
use serde::Deserialize;
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, PremultipliedColorU8, Rect,
    Stroke, Transform,
};

use crate::profile::{Badge, Profile};

const GLYPHS: &str = include_str!("../assets/glyphs.json");
const FONT: &[u8] = include_bytes!("../assets/ArchivoBlack-Regular.ttf");
/// Size the recolored master is prepared at; every output size is drawn from it.
const MASTER: u32 = 1024;

#[derive(Deserialize)]
struct GlyphFile {
    symbols: Vec<Symbol>,
}

#[derive(Deserialize)]
struct Symbol {
    name: String,
    shapes: Vec<Shape>,
}

#[derive(Deserialize)]
struct Shape {
    #[serde(default)]
    stroke: Option<f32>,
    #[serde(default)]
    alpha: Option<f32>,
    /// Drawn in the background color, to cut a detail out of a shape.
    #[serde(default)]
    background: bool,
    parts: Vec<Part>,
}

#[derive(Deserialize)]
struct Part {
    circle: Option<[f32; 3]>,
    ellipse: Option<[f32; 4]>,
    rrect: Option<[f32; 5]>,
    star: Option<[f32; 5]>,
    d: Option<Vec<Vec<serde_json::Value>>>,
}

fn glyphs() -> &'static [Symbol] {
    static CELL: OnceLock<Vec<Symbol>> = OnceLock::new();
    CELL.get_or_init(|| {
        // The file is compiled in and covered by tests, so a parse failure is a build bug.
        serde_json::from_str::<GlyphFile>(GLYPHS).map(|g| g.symbols).unwrap_or_default()
    })
}

// ---- color math --------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct Hsv {
    h: f32,
    s: f32,
    v: f32,
}

fn to_hsv(r: f32, g: f32, b: f32) -> Hsv {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    let mut h = 0.0;
    if d > 0.0 {
        h = if mx == r {
            (g - b) / d
        } else if mx == g {
            (b - r) / d + 2.0
        } else {
            (r - g) / d + 4.0
        } / 6.0;
        if h < 0.0 {
            h += 1.0;
        }
    }
    Hsv { h, s: if mx == 0.0 { 0.0 } else { d / mx }, v: mx }
}

fn to_rgb(c: Hsv) -> (f32, f32, f32) {
    let h = (c.h - c.h.floor()) * 6.0;
    let i = h.floor() as i32 % 6;
    let f = h - h.floor();
    let (p, q, t) = (c.v * (1.0 - c.s), c.v * (1.0 - c.s * f), c.v * (1.0 - c.s * (1.0 - f)));
    match i {
        0 => (c.v, t, p),
        1 => (q, c.v, p),
        2 => (p, c.v, t),
        3 => (p, q, c.v),
        4 => (t, p, c.v),
        _ => (c.v, p, q),
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn unit(rgb: [u8; 3]) -> (f32, f32, f32) {
    (f32::from(rgb[0]) / 255.0, f32::from(rgb[1]) / 255.0, f32::from(rgb[2]) / 255.0)
}

/// Straight (not premultiplied) color and alpha, 0..=1.
fn get(p: PremultipliedColorU8) -> (f32, f32, f32, f32) {
    let a = f32::from(p.alpha()) / 255.0;
    if a <= 0.0 {
        return (0.0, 0.0, 0.0, 0.0);
    }
    let c = |v: u8| (f32::from(v) / 255.0 / a).min(1.0);
    (c(p.red()), c(p.green()), c(p.blue()), a)
}

fn set(r: f32, g: f32, b: f32, a: f32) -> PremultipliedColorU8 {
    let byte = |v: f32| (v * a * 255.0).round().clamp(0.0, 255.0) as u8;
    let alpha = (a * 255.0).round().clamp(0.0, 255.0) as u8;
    PremultipliedColorU8::from_rgba(byte(r), byte(g), byte(b), alpha).unwrap_or(PremultipliedColorU8::TRANSPARENT)
}

/// The icon's brand color: the most common strongly saturated hue.
fn dominant(pm: &Pixmap) -> Option<Hsv> {
    let mut bins = [0f32; 36];
    let mut opaque = 0usize;
    let bin = |h: f32| ((h * 36.0) as usize) % 36;
    for p in pm.pixels() {
        let (r, g, b, a) = get(*p);
        if a <= 0.5 {
            continue;
        }
        opaque += 1;
        let c = to_hsv(r, g, b);
        if c.s > 0.35 && c.v > 0.25 {
            bins[bin(c.h)] += c.s;
        }
    }
    let best = (0..36).max_by(|&a, &b| bins[a].total_cmp(&bins[b]))?;
    if bins.iter().sum::<f32>() <= opaque as f32 * 0.01 {
        return None;
    }
    let (mut x, mut y, mut s, mut v, mut n) = (0f32, 0f32, 0f32, 0f32, 0f32);
    for p in pm.pixels() {
        let (r, g, b, a) = get(*p);
        if a <= 0.5 {
            continue;
        }
        let c = to_hsv(r, g, b);
        if c.s <= 0.35 || c.v <= 0.25 {
            continue;
        }
        let dist = bin(c.h).abs_diff(best);
        if dist.min(36 - dist) > 1 {
            continue;
        }
        let angle = std::f32::consts::TAU * c.h;
        x += angle.cos();
        y += angle.sin();
        s += c.s;
        v += c.v;
        n += 1.0;
    }
    if n == 0.0 {
        return None;
    }
    let mut h = y.atan2(x) / std::f32::consts::TAU;
    if h < 0.0 {
        h += 1.0;
    }
    Some(Hsv { h, s: s / n, v: v / n })
}

/// Moves the brand color to `target` and gives everything else the target's hue at its own
/// saturation, so Claude's cream mark stays a pale tint and the shading survives.
fn recolor(pm: &mut Pixmap, target: [u8; 3]) {
    let (tr, tg, tb) = unit(target);
    let t = to_hsv(tr, tg, tb);
    let source = dominant(pm);
    for p in pm.pixels_mut() {
        let (r, g, b, a) = get(*p);
        if a <= 0.0 {
            continue;
        }
        let c = to_hsv(r, g, b);
        let out = match source {
            Some(s0) => {
                let mut d = (c.h - s0.h).abs();
                d = d.min(1.0 - d);
                let w = (1.0 - smoothstep(0.06, 0.14, d)) * smoothstep(0.15, 0.35, c.s);
                let mapped = Hsv {
                    h: t.h + (c.h - s0.h),
                    s: (c.s * t.s / s0.s.max(0.01)).min(1.0),
                    v: (c.v * t.v / s0.v.max(0.01)).min(1.0),
                };
                Hsv {
                    h: if w > 0.5 { mapped.h } else { t.h },
                    s: c.s * (1.0 - w) + mapped.s * w,
                    v: c.v * (1.0 - w) + mapped.v * w,
                }
            }
            None => Hsv { h: t.h, s: c.s.max(t.s * 0.35), v: c.v },
        };
        let (r, g, b) = to_rgb(out);
        *p = set(r, g, b, a);
    }
}

/// Bounds of the visible part of the icon.
fn opaque_bounds(pm: &Pixmap) -> Rect {
    let (w, h) = (pm.width(), pm.height());
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    let mut any = false;
    for (i, p) in pm.pixels().iter().enumerate() {
        if p.alpha() > 25 {
            let (x, y) = (i as u32 % w, i as u32 / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            any = true;
        }
    }
    if !any {
        return Rect::from_xywh(0.0, 0.0, w as f32, h as f32)
            .unwrap_or_else(|| Rect::from_xywh(0.0, 0.0, 1.0, 1.0).expect("unit rect"));
    }
    Rect::from_ltrb(x0 as f32, y0 as f32, (x1 + 1) as f32, (y1 + 1) as f32)
        .unwrap_or_else(|| Rect::from_xywh(0.0, 0.0, w as f32, h as f32).expect("full rect"))
}

/// Average color just inside the icon's corners, which is its background.
fn sample_background(pm: &Pixmap, b: Rect) -> Option<(f32, f32, f32)> {
    let (mut r, mut g, mut bl, mut n) = (0f32, 0f32, 0f32, 0f32);
    let (iw, ih) = (b.width() * 0.12, b.height() * 0.12);
    for (cx, cy) in [
        (b.left() + iw, b.top() + ih),
        (b.right() - iw, b.top() + ih),
        (b.left() + iw, b.bottom() - ih),
        (b.right() - iw, b.bottom() - ih),
    ] {
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let (x, y) = ((cx as i32 + dx).max(0) as u32, (cy as i32 + dy).max(0) as u32);
                if let Some(p) = pm.pixel(x, y) {
                    let (pr, pg, pb, a) = get(p);
                    if a > 0.9 {
                        r += pr;
                        g += pg;
                        bl += pb;
                        n += 1.0;
                    }
                }
            }
        }
    }
    (n > 0.0).then(|| (r / n, g / n, bl / n))
}

// ---- drawing -----------------------------------------------------------------------------

fn solid(rgb: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(rgb[0], rgb[1], rgb[2], (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    p.anti_alias = true;
    p
}

fn push_rounded_rect(pb: &mut PathBuilder, r: Rect, radius: f32) {
    let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    // Cubic approximation of a circular arc.
    let k = rad * 0.552_284_8;
    let (l, t, rt, b) = (r.left(), r.top(), r.right(), r.bottom());
    pb.move_to(l + rad, t);
    pb.line_to(rt - rad, t);
    pb.cubic_to(rt - rad + k, t, rt, t + rad - k, rt, t + rad);
    pb.line_to(rt, b - rad);
    pb.cubic_to(rt, b - rad + k, rt - rad + k, b, rt - rad, b);
    pb.line_to(l + rad, b);
    pb.cubic_to(l + rad - k, b, l, b - rad + k, l, b - rad);
    pb.line_to(l, t + rad);
    pb.cubic_to(l, t + rad - k, l + rad - k, t, l + rad, t);
    pb.close();
}

/// Draws a symbol. `map` places a point given in glyph units (0..1, origin bottom left);
/// `scale` turns glyph lengths into pixels.
fn draw_symbol(
    pm: &mut Pixmap,
    sym: &Symbol,
    map: &dyn Fn(f32, f32) -> (f32, f32),
    scale: f32,
    ink: &Paint<'_>,
    bg: &Paint<'_>,
) {
    for shape in &sym.shapes {
        let mut pb = PathBuilder::new();
        for part in &shape.parts {
            if let Some([cx, cy, r]) = part.circle {
                let (x, y) = map(cx, cy);
                pb.push_circle(x, y, r * scale);
            } else if let Some([cx, cy, rx, ry]) = part.ellipse {
                let (x, y) = map(cx, cy);
                if let Some(rect) = Rect::from_xywh(x - rx * scale, y - ry * scale, rx * 2.0 * scale, ry * 2.0 * scale)
                {
                    pb.push_oval(rect);
                }
            } else if let Some([x, y, w, h, r]) = part.rrect {
                // (x, y) is the bottom-left corner in glyph space.
                let (sx, sy) = map(x, y + h);
                if let Some(rect) = Rect::from_xywh(sx, sy, w * scale, h * scale) {
                    push_rounded_rect(&mut pb, rect, r * scale);
                }
            } else if let Some([cx, cy, outer, inner, n]) = part.star {
                let points = (n as usize).max(3);
                for i in 0..points * 2 {
                    let angle = std::f32::consts::FRAC_PI_2 + i as f32 * std::f32::consts::PI / points as f32;
                    let radius = if i % 2 == 0 { outer } else { inner };
                    let (x, y) = map(cx + radius * angle.cos(), cy + radius * angle.sin());
                    if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
                }
                pb.close();
            } else if let Some(ops) = &part.d {
                for op in ops {
                    let name = op.first().and_then(|v| v.as_str()).unwrap_or("");
                    let n: Vec<f32> =
                        op.iter().skip(1).filter_map(serde_json::Value::as_f64).map(|v| v as f32).collect();
                    match (name, n.as_slice()) {
                        ("M", [x, y, ..]) => {
                            let (x, y) = map(*x, *y);
                            pb.move_to(x, y);
                        }
                        ("L", [x, y, ..]) => {
                            let (x, y) = map(*x, *y);
                            pb.line_to(x, y);
                        }
                        ("C", [x1, y1, x2, y2, x, y, ..]) => {
                            let (a, b) = map(*x1, *y1);
                            let (c, e) = map(*x2, *y2);
                            let (f, g) = map(*x, *y);
                            pb.cubic_to(a, b, c, e, f, g);
                        }
                        ("Z", _) => pb.close(),
                        _ => {}
                    }
                }
            }
        }
        let Some(path) = pb.finish() else { continue };
        let mut paint = if shape.background { bg.clone() } else { ink.clone() };
        if let (Some(a), false) = (shape.alpha, shape.background) {
            // Half-tone details: blend the ink toward the background.
            paint = mix_paint(ink, bg, a);
        }
        if let Some(w) = shape.stroke {
            let stroke = Stroke {
                width: (w * scale).max(1.0),
                line_cap: LineCap::Round,
                line_join: LineJoin::Round,
                ..Stroke::default()
            };
            pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
        } else {
            pm.fill_path(&path, &paint, FillRule::EvenOdd, Transform::identity(), None);
        }
    }
}

fn mix_paint(ink: &Paint<'_>, bg: &Paint<'_>, amount: f32) -> Paint<'static> {
    let color = |p: &Paint<'_>| match &p.shader {
        tiny_skia::Shader::SolidColor(c) => [c.red(), c.green(), c.blue()],
        _ => [1.0, 1.0, 1.0],
    };
    let (a, b) = (color(ink), color(bg));
    let m = |i: usize| ((a[i] * amount + b[i] * (1.0 - amount)) * 255.0).round() as u8;
    solid([m(0), m(1), m(2)], 1.0)
}

fn draw_letter(pm: &mut Pixmap, c: char, center: (f32, f32), height: f32, max_width: f32, ink: &Paint<'_>) {
    let Ok(font) = FontRef::try_from_slice(FONT) else { return };
    let Some(outline) = font.outline(font.glyph_id(c)) else { return };
    // ab_glyph reports outline bounds with min.y/max.y swapped, so normalize them.
    let b = outline.bounds;
    let (x0, x1) = (b.min.x.min(b.max.x), b.min.x.max(b.max.x));
    let (y0, y1) = (b.min.y.min(b.max.y), b.min.y.max(b.max.y));
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let scale = (height / h).min(max_width / w);
    let ox = center.0 - (x0 + w / 2.0) * scale;
    let oy = center.1 + (y0 + h / 2.0) * scale;
    // Font units are y-up.
    let map = |p: ab_glyph::Point| (ox + p.x * scale, oy - p.y * scale);
    let mut pb = PathBuilder::new();
    let mut last: Option<ab_glyph::Point> = None;
    for curve in &outline.curves {
        let start = match curve {
            OutlineCurve::Line(p, _) | OutlineCurve::Quad(p, _, _) | OutlineCurve::Cubic(p, _, _, _) => *p,
        };
        if last != Some(start) {
            if last.is_some() {
                pb.close();
            }
            let (x, y) = map(start);
            pb.move_to(x, y);
        }
        last = Some(match curve {
            OutlineCurve::Line(_, p1) => {
                let (x, y) = map(*p1);
                pb.line_to(x, y);
                *p1
            }
            OutlineCurve::Quad(_, c, p2) => {
                let (cx, cy) = map(*c);
                let (x, y) = map(*p2);
                pb.quad_to(cx, cy, x, y);
                *p2
            }
            OutlineCurve::Cubic(_, c1, c2, p3) => {
                let (ax, ay) = map(*c1);
                let (bx, by) = map(*c2);
                let (x, y) = map(*p3);
                pb.cubic_to(ax, ay, bx, by, x, y);
                *p3
            }
        });
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, ink, FillRule::Winding, Transform::identity(), None);
    }
}

/// Ink for the badge: the profile color, darkened when it's too pale to read on white.
fn badge_ink(rgb: [u8; 3]) -> [u8; 3] {
    let (r, g, b) = unit(rgb);
    let mut c = to_hsv(r, g, b);
    if c.v > 0.8 && c.s < 0.5 {
        c.v = 0.6;
    }
    let (r, g, b) = to_rgb(c);
    [(r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8]
}

/// A white disc in the bottom-right corner of `bounds` holding the symbol or letter.
fn draw_badge(pm: &mut Pixmap, badge: &Badge, rgb: [u8; 3], bounds: Rect) {
    if *badge == Badge::None {
        return;
    }
    let dim = pm.width() as f32;
    let d = bounds.width() * 0.40;
    let cx = (dim - d / 2.0 - 2.0).min(bounds.right() - d * 0.36);
    let cy = (dim - d / 2.0 - 2.0).min(bounds.bottom() - d * 0.36);

    // Soft shadow: a few widening, fading discs, shifted down a little.
    let mut pb = PathBuilder::new();
    for i in (1..=6).rev() {
        let spread = d * 0.015 * i as f32;
        pb.push_circle(cx, cy + d * 0.035, d / 2.0 + spread);
        if let Some(path) = pb.finish() {
            pm.fill_path(&path, &solid([0, 0, 0], 0.045), FillRule::Winding, Transform::identity(), None);
        }
        pb = PathBuilder::new();
    }
    pb.push_circle(cx, cy, d / 2.0);
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &solid([255, 255, 255], 1.0), FillRule::Winding, Transform::identity(), None);
    }

    let ink = solid(badge_ink(rgb), 1.0);
    let white = solid([255, 255, 255], 1.0);
    match badge {
        Badge::Symbol(name) => {
            if let Some(sym) = glyphs().iter().find(|s| s.name == *name) {
                let k = d * 1.62;
                let map = move |x: f32, y: f32| (cx + (x - 0.5) * k, cy - (y - 0.5) * k);
                draw_symbol(pm, sym, &map, k, &ink, &white);
            }
        }
        Badge::Letter(c) => draw_letter(pm, *c, (cx, cy), d * 0.5, d * 0.62, &ink),
        Badge::None => {}
    }
}

/// Dark background: near black, tinted with the profile's color.
fn dark_fill(rgb: [u8; 3]) -> [u8; 3] {
    let base = [20.0f32, 20.0, 23.0];
    std::array::from_fn(|i| (base[i] * 0.82 + f32::from(rgb[i]) * 0.18).round() as u8)
}

/// The profile's color for a mark on a dark background: bright enough to stand out,
/// saturated enough to tell profiles apart.
fn dark_mark_color(rgb: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = unit(rgb);
    let mut c = to_hsv(r, g, b);
    c.v = c.v.max(0.88);
    if c.s > 0.15 {
        c.s = c.s.clamp(0.45, 0.85);
    }
    // Blues and purples look darker than greens at the same value; lift them toward white
    // until they're about as bright, so every profile's mark reads on the dark background.
    let (r, g, b) = to_rgb(c);
    let luma = 0.2126 * r + 0.7152 * g + 0.0722 * b;
    const MIN_LUMA: f32 = 0.55;
    if luma >= MIN_LUMA {
        return (r, g, b);
    }
    let t = (MIN_LUMA - luma) / (1.0 - luma);
    (r + (1.0 - r) * t, g + (1.0 - g) * t, b + (1.0 - b) * t)
}

/// A layered icon: what [`Layered::icon_json`] describes, as images.
pub struct Layered {
    /// Background in Default (and the base other styles start from).
    pub fill: [u8; 3],
    /// Background in Dark.
    pub dark_fill: [u8; 3],
    /// Claude's mark as in Claude's own icon, on a transparent full canvas.
    pub mark: Pixmap,
    /// The mark in the profile's color, for Dark.
    pub mark_dark: Pixmap,
    /// The badge on its own layer, if the profile has one.
    pub badge: Option<Pixmap>,
}

impl Layered {
    /// Asset file names used by [`Layered::icon_json`].
    pub const MARK: &'static str = "mark.png";
    pub const MARK_DARK: &'static str = "mark-dark.png";
    pub const BADGE: &'static str = "badge.png";

    /// The Icon Composer document (`icon.json`). Groups are listed front to back, so the
    /// badge sits above the mark and gets its own glass.
    pub fn icon_json(&self) -> String {
        let color = |c: [u8; 3]| {
            format!(
                "srgb:{:.5},{:.5},{:.5},1.00000",
                f32::from(c[0]) / 255.0,
                f32::from(c[1]) / 255.0,
                f32::from(c[2]) / 255.0
            )
        };
        let mut groups = Vec::new();
        if self.badge.is_some() {
            groups.push(serde_json::json!({ "layers": [ { "name": "badge", "image-name": Self::BADGE } ] }));
        }
        groups.push(serde_json::json!({
            "layers": [ {
                "name": "mark",
                "image-name-specializations": [
                    { "value": Self::MARK },
                    { "appearance": "dark", "value": Self::MARK_DARK }
                ]
            } ]
        }));
        let doc = serde_json::json!({
            "fill-specializations": [
                { "value": { "solid": color(self.fill) } },
                { "appearance": "dark", "value": { "solid": color(self.dark_fill) } }
            ],
            "groups": groups,
            "supported-platforms": { "squares": "shared" }
        });
        serde_json::to_string_pretty(&doc).unwrap_or_default()
    }

    /// Each asset's file name and PNG bytes.
    pub fn assets(&self) -> Result<Vec<(&'static str, Vec<u8>)>> {
        let mut out = vec![(Self::MARK, self.mark.encode_png()?), (Self::MARK_DARK, self.mark_dark.encode_png()?)];
        if let Some(b) = &self.badge {
            out.push((Self::BADGE, b.encode_png()?));
        }
        Ok(out)
    }
}

/// A profile's look, ready to draw at any size.
pub struct Look {
    rgb: [u8; 3],
    badge: Badge,
    /// Claude's icon, recolored, at [`MASTER`] size, and where it's opaque.
    master: Option<(Pixmap, Rect)>,
}

impl Look {
    pub fn new(rgb: [u8; 3], badge: Badge, source: Option<&Pixmap>) -> Self {
        let master = source.and_then(|src| {
            let mut m = Pixmap::new(MASTER, MASTER)?;
            let sx = MASTER as f32 / src.width() as f32;
            let sy = MASTER as f32 / src.height() as f32;
            let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
            m.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(sx, sy), None);
            recolor(&mut m, rgb);
            let bounds = opaque_bounds(&m);
            Some((m, bounds))
        });
        Self { rgb, badge, master }
    }

    pub fn uses_claude_icon(&self) -> bool {
        self.master.is_some()
    }

    /// Renders the icon `dim` pixels square.
    pub fn render(&self, dim: u32) -> Option<Pixmap> {
        let mut pm = Pixmap::new(dim, dim)?;
        let d = dim as f32;
        let bounds = match &self.master {
            Some((m, b)) => {
                let s = d / MASTER as f32;
                let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
                pm.draw_pixmap(0, 0, m.as_ref(), &paint, Transform::from_scale(s, s), None);
                Rect::from_ltrb(b.left() * s, b.top() * s, b.right() * s, b.bottom() * s)?
            }
            None => {
                // No Claude icon to work from: a plain rounded square in the profile color.
                let inset = d * 0.1;
                let tile = Rect::from_xywh(inset, inset, d - inset * 2.0, d - inset * 2.0)?;
                let mut pb = PathBuilder::new();
                push_rounded_rect(&mut pb, tile, d * 0.2);
                pm.fill_path(&pb.finish()?, &solid(self.rgb, 1.0), FillRule::Winding, Transform::identity(), None);
                tile
            }
        };
        draw_badge(&mut pm, &self.badge, self.rgb, bounds);
        Some(pm)
    }

    /// The icon split into layers for macOS 26's Icon Composer format. macOS draws the
    /// Default, Dark, Clear and Tinted styles from these; Dark gets its own background and
    /// mark so the profile's color stays visible on a dark icon.
    pub fn layers(&self) -> Option<Layered> {
        let (master, bounds) = self.master.as_ref()?;
        let fill = sample_background(master, *bounds)?;
        // The mark: everything that differs from the background, cropped to the icon.
        let mut cut = master.clone();
        for p in cut.pixels_mut() {
            let (r, g, b, a) = get(*p);
            let dist = ((r - fill.0).abs() + (g - fill.1).abs() + (b - fill.2).abs()) / 3.0;
            let w = smoothstep(0.04, 0.16, dist);
            if w <= 0.0 {
                *p = PremultipliedColorU8::TRANSPARENT;
                continue;
            }
            // Edge pixels are a blend of mark and background; take the background back out
            // so the mark doesn't carry a fringe of the old color onto Dark and Clear.
            let unblend = |c: f32, bg: f32| ((c - bg * (1.0 - w)) / w).clamp(0.0, 1.0);
            *p = set(unblend(r, fill.0), unblend(g, fill.1), unblend(b, fill.2), a * w);
        }
        let mut mark = Pixmap::new(MASTER, MASTER)?;
        let (sx, sy) = (MASTER as f32 / bounds.width(), MASTER as f32 / bounds.height());
        let paint = PixmapPaint { quality: FilterQuality::Bicubic, ..PixmapPaint::default() };
        let t = Transform::from_translate(-bounds.left(), -bounds.top()).post_scale(sx, sy);
        mark.draw_pixmap(0, 0, cut.as_ref(), &paint, t, None);

        // Dark: the mark in the profile's color, keeping the mark's own shading.
        let ink = dark_mark_color(self.rgb);
        let mut mark_dark = mark.clone();
        for p in mark_dark.pixels_mut() {
            let (r, g, b, a) = get(*p);
            if a > 0.0 {
                let shade = 0.78 + 0.22 * ((r + g + b) / 3.0);
                *p = set(ink.0 * shade, ink.1 * shade, ink.2 * shade, a);
            }
        }

        // The badge on its own layer, kept inside the rounded mask macOS applies.
        let badge = if self.badge == Badge::None {
            None
        } else {
            let mut pm = Pixmap::new(MASTER, MASTER)?;
            let d = MASTER as f32;
            draw_badge(&mut pm, &self.badge, self.rgb, Rect::from_ltrb(d * 0.1, d * 0.1, d * 0.9, d * 0.9)?);
            Some(pm)
        };
        let byte = |v: f32| (v * 255.0).round().clamp(0.0, 255.0) as u8;
        Some(Layered {
            fill: [byte(fill.0), byte(fill.1), byte(fill.2)],
            dark_fill: dark_fill(self.rgb),
            mark,
            mark_dark,
            badge,
        })
    }

    fn png(&self, dim: u32) -> Result<Vec<u8>> {
        self.render(dim).context("couldn't draw the icon")?.encode_png().context("couldn't encode the icon")
    }

    /// An `.icns` made of PNG entries, which macOS has read since 10.7.
    pub fn icns(&self) -> Result<Vec<u8>> {
        const ENTRIES: &[(&[u8; 4], u32)] = &[
            (b"icp4", 16),
            (b"icp5", 32),
            (b"ic11", 32),
            (b"icp6", 64),
            (b"ic12", 64),
            (b"ic07", 128),
            (b"ic08", 256),
            (b"ic13", 256),
            (b"ic09", 512),
            (b"ic14", 512),
            (b"ic10", 1024),
        ];
        let mut body = Vec::new();
        for (kind, dim) in ENTRIES {
            let data = self.png(*dim)?;
            body.extend_from_slice(*kind);
            body.extend_from_slice(&u32::try_from(data.len() + 8)?.to_be_bytes());
            body.extend_from_slice(&data);
        }
        let mut out = Vec::with_capacity(body.len() + 8);
        out.extend_from_slice(b"icns");
        out.extend_from_slice(&u32::try_from(body.len() + 8)?.to_be_bytes());
        out.extend_from_slice(&body);
        Ok(out)
    }

    /// An `.ico` with PNG-compressed images, which Windows has read since Vista.
    pub fn ico(&self) -> Result<Vec<u8>> {
        const SIZES: &[u32] = &[16, 24, 32, 48, 64, 256];
        let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| self.png(s)).collect::<Result<_>>()?;
        let mut out = Vec::new();
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&u16::try_from(SIZES.len())?.to_le_bytes());
        let mut offset = 6 + 16 * SIZES.len();
        for (dim, data) in SIZES.iter().zip(&images) {
            let byte = if *dim >= 256 { 0 } else { *dim as u8 };
            out.extend_from_slice(&[byte, byte, 0, 0]);
            out.extend_from_slice(&1u16.to_le_bytes());
            out.extend_from_slice(&32u16.to_le_bytes());
            out.extend_from_slice(&u32::try_from(data.len())?.to_le_bytes());
            out.extend_from_slice(&u32::try_from(offset)?.to_le_bytes());
            offset += data.len();
        }
        for data in &images {
            out.extend_from_slice(data);
        }
        Ok(out)
    }
}

// ---- Claude's icon -----------------------------------------------------------------------

/// Claude's icon, decoded, plus a short fingerprint so profile icons are redrawn when
/// Claude's icon changes.
pub struct Source {
    pub pixmap: Pixmap,
    pub tag: String,
}

fn fnv(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{:08x}", h as u32)
}

/// Decodes a PNG or ICO image (the largest entry of an ICO).
pub fn decode_image(bytes: &[u8]) -> Option<Pixmap> {
    if bytes.starts_with(b"\x89PNG") {
        return Pixmap::decode_png(bytes).ok();
    }
    if bytes.len() > 6 && bytes[..4] == [0, 0, 1, 0] {
        return decode_ico(bytes);
    }
    None
}

fn decode_ico(bytes: &[u8]) -> Option<Pixmap> {
    let count = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
    let le32 =
        |at: usize| -> Option<u32> { bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) };
    let mut best: Option<(u32, usize, usize)> = None;
    for i in 0..count {
        let e = 6 + i * 16;
        let w = u32::from(*bytes.get(e)?);
        let w = if w == 0 { 256 } else { w };
        let size = le32(e + 8)? as usize;
        let offset = le32(e + 12)? as usize;
        if best.is_none_or(|(bw, _, _)| w > bw) {
            best = Some((w, offset, size));
        }
    }
    let (_, offset, size) = best?;
    let data = bytes.get(offset..offset.checked_add(size)?)?;
    if data.starts_with(b"\x89PNG") {
        return Pixmap::decode_png(data).ok();
    }
    decode_bmp_dib(data)
}

/// A 32-bit BGRA DIB from an ICO entry (bottom-up rows, height doubled for the mask).
fn decode_bmp_dib(data: &[u8]) -> Option<Pixmap> {
    let le32 =
        |at: usize| -> Option<i32> { data.get(at..at + 4).map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]])) };
    let header = usize::try_from(le32(0)?).ok()?;
    let w = u32::try_from(le32(4)?).ok()?;
    let h = u32::try_from(le32(8)?).ok()? / 2;
    let bpp = u16::from_le_bytes([*data.get(14)?, *data.get(15)?]);
    if bpp != 32 || w == 0 || h == 0 {
        return None;
    }
    let mut pm = Pixmap::new(w, h)?;
    let stride = w as usize * 4;
    for y in 0..h as usize {
        let row = header + (h as usize - 1 - y) * stride;
        for x in 0..w as usize {
            let px = data.get(row + x * 4..row + x * 4 + 4)?;
            let a = f32::from(px[3]) / 255.0;
            let c = set(f32::from(px[2]) / 255.0, f32::from(px[1]) / 255.0, f32::from(px[0]) / 255.0, a);
            pm.pixels_mut()[y * w as usize + x] = c;
        }
    }
    Some(pm)
}

/// Where Claude's icon image might be, for this platform.
fn source_candidates(install: &crate::claude::Install, cache_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if cfg!(target_os = "macos") {
        // Ask macOS for the icon it shows, which also covers icons in an asset catalog.
        let png = cache_dir.join("source").join("claude-icon.png");
        let _ = std::fs::create_dir_all(cache_dir.join("source"));
        let fresh = std::fs::metadata(&png)
            .and_then(|m| m.modified())
            .ok()
            .zip(std::fs::metadata(install.app.join("Contents/Info.plist")).and_then(|m| m.modified()).ok())
            .is_some_and(|(cached, app)| cached >= app);
        if fresh || crate::platform::export_app_icon(&install.app, &png) {
            out.push(png);
        }
    }
    let dir = install.exe.parent().map(Path::to_path_buf).unwrap_or_default();
    // Squirrel (Windows) keeps app.ico beside the stub; Electron builds often ship icons
    // in resources/.
    out.push(dir.join("app.ico"));
    let mut versioned: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("app-")))
        .collect();
    versioned.sort();
    for d in versioned.iter().rev().chain(std::iter::once(&dir)) {
        for name in ["resources/icon.png", "resources/app.ico", "resources/icon.ico", "icon.png"] {
            out.push(d.join(name));
        }
    }
    if cfg!(all(unix, not(target_os = "macos"))) {
        let mut roots = vec![
            PathBuf::from("/usr/share/icons/hicolor"),
            PathBuf::from("/var/lib/flatpak/exports/share/icons/hicolor"),
        ];
        if let Some(home) = crate::claude::home() {
            roots.push(home.join(".local/share/icons/hicolor"));
            roots.push(home.join(".local/share/flatpak/exports/share/icons/hicolor"));
        }
        for root in roots {
            for size in ["1024x1024", "512x512", "256x256", "128x128"] {
                for name in ["claude-desktop", "claude", "com.anthropic.Claude"] {
                    out.push(root.join(size).join("apps").join(format!("{name}.png")));
                }
            }
        }
        out.push(PathBuf::from("/usr/share/pixmaps/claude-desktop.png"));
    }
    out
}

/// Claude's icon from the install on this computer, if it can be found and decoded.
/// `CLAUDE_PROFILE_HUB_ICON` can point at a PNG or ICO to use instead.
pub fn claude_source(install: Option<&crate::claude::Install>, cache_dir: &Path) -> Option<Source> {
    let mut candidates = Vec::new();
    if let Some(p) = std::env::var_os("CLAUDE_PROFILE_HUB_ICON") {
        candidates.push(PathBuf::from(p));
    }
    if let Some(install) = install {
        let _ = std::fs::create_dir_all(cache_dir);
        candidates.extend(source_candidates(install, cache_dir));
    }
    candidates.into_iter().find_map(|p| {
        let bytes = std::fs::read(&p).ok()?;
        let pixmap = decode_image(&bytes)?;
        Some(Source { pixmap, tag: fnv(&bytes) })
    })
}

/// Icon files for one profile.
#[derive(Debug, Clone)]
pub struct IconSet {
    pub png: PathBuf,
    pub ico: PathBuf,
    pub icns: PathBuf,
    /// macOS: a compiled layered icon (Assets.car) with Dark, Clear and Tinted styles,
    /// when Xcode 26's actool is available.
    pub car: Option<PathBuf>,
}

/// Writes a profile's icons into `dir`. File names include the look and Claude's icon
/// fingerprint, so a changed icon gets a new name (Windows and Linux cache icons by path).
pub fn write_for(dir: &Path, profile: &Profile, source: Option<&Source>) -> Result<IconSet> {
    let badge = profile.badge();
    let rgb = profile.rgb();
    let tag: String = rgb.iter().map(|b| format!("{b:02x}")).collect();
    let src_tag = source.map_or("flat", |s| s.tag.as_str());
    let stem = format!("{}-{}-{}-{}", profile.slug(), tag, badge.as_config().to_lowercase(), src_tag);
    std::fs::create_dir_all(dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    let set = IconSet {
        png: dir.join(format!("{stem}.png")),
        ico: dir.join(format!("{stem}.ico")),
        icns: dir.join(format!("{stem}.icns")),
        car: None,
    };
    // The layer layout is versioned so a change to it rebuilds cached layered icons.
    let car = dir.join(format!("{stem}-layers2.car"));
    let need_png = !set.png.exists();
    let need_ico = cfg!(windows) && !set.ico.exists();
    let need_icns = cfg!(target_os = "macos") && !set.icns.exists();
    // A failed attempt (no Xcode) is remembered, so it isn't retried on every shortcut.
    let car_failed = dir.join(format!("{stem}-layers2.nocar"));
    let need_car = cfg!(target_os = "macos") && source.is_some() && !car.exists() && !car_failed.exists();
    if need_png || need_ico || need_icns || need_car {
        let look = Look::new(rgb, badge, source.map(|s| &s.pixmap));
        if need_png {
            std::fs::write(&set.png, look.png(512)?)?;
        }
        if need_ico {
            std::fs::write(&set.ico, look.ico()?)?;
        }
        if need_icns {
            std::fs::write(&set.icns, look.icns()?)?;
        }
        if need_car {
            let built = look.layers().is_some_and(|l| crate::platform::compile_layered_icon(&l, &car));
            if !built {
                let _ = std::fs::write(&car_failed, b"");
            }
        }
    }
    let set = IconSet { car: car.exists().then_some(car), ..set };
    // Old looks of this profile are no longer referenced.
    let prefix = format!("{}-", profile.slug());
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with(&prefix) && !name.starts_with(&stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(set)
}

/// [`write_for`] with Claude's icon looked up from the context's install.
pub fn write_for_ctx(ctx: &crate::ctx::Ctx, profile: &Profile) -> Result<IconSet> {
    let dir = ctx.paths.icons_dir();
    let source = claude_source(ctx.claude().as_ref(), &dir);
    write_for(&dir, profile, source.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::SYMBOLS;

    /// A stand-in for Claude's icon: a terracotta rounded square with a cream disc.
    fn fake_claude() -> Pixmap {
        let mut pm = Pixmap::new(256, 256).expect("pixmap");
        let mut pb = PathBuilder::new();
        push_rounded_rect(&mut pb, Rect::from_xywh(16.0, 16.0, 224.0, 224.0).expect("rect"), 50.0);
        pm.fill_path(
            &pb.finish().expect("path"),
            &solid([217, 119, 87], 1.0),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        let mut pb = PathBuilder::new();
        pb.push_circle(128.0, 128.0, 60.0);
        pm.fill_path(
            &pb.finish().expect("path"),
            &solid([250, 240, 230], 1.0),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
        pm
    }

    #[test]
    fn glyph_file_has_every_symbol() {
        let names: Vec<&str> = glyphs().iter().map(|s| s.name.as_str()).collect();
        for s in SYMBOLS {
            assert!(names.contains(s), "missing {s}");
        }
    }

    #[test]
    fn dominant_finds_the_brand_hue() {
        let h = dominant(&fake_claude()).expect("hue").h;
        assert!((h - 0.04).abs() < 0.03, "hue {h}");
    }

    #[test]
    fn recolor_moves_brand_color_to_target() {
        let look = Look::new([31, 173, 189], Badge::None, Some(&fake_claude()));
        let pm = look.render(256).expect("render");
        let (r, g, b, _) = get(pm.pixel(30, 128).expect("pixel"));
        assert!(b > r && g > r, "teal-ish, got {r} {g} {b}");
    }

    #[test]
    fn recolor_keeps_pale_mark_pale() {
        let look = Look::new([31, 173, 189], Badge::None, Some(&fake_claude()));
        let pm = look.render(256).expect("render");
        let (r, g, b, _) = get(pm.pixel(128, 128).expect("pixel"));
        assert!(r > 0.8 && g > 0.8 && b > 0.8, "cream stays light, got {r} {g} {b}");
    }

    #[test]
    fn badge_sits_in_the_bottom_right() {
        let look = Look::new([97, 89, 199], Badge::Symbol("star"), Some(&fake_claude()));
        let pm = look.render(256).expect("render");
        // The disc is centered near (208, 208) with a 45px radius; the star is in the middle.
        let (r, g, b, _) = get(pm.pixel(208, 246).expect("pixel"));
        assert!(r > 0.9 && g > 0.9 && b > 0.9, "white disc edge, got {r} {g} {b}");
    }

    #[test]
    fn every_badge_renders_with_and_without_claude_icon() {
        let src = fake_claude();
        for s in SYMBOLS {
            for source in [Some(&src), None] {
                assert!(Look::new([0, 0, 0], Badge::Symbol(s), source).render(64).is_some(), "{s}");
            }
        }
    }

    #[test]
    fn layers_split_fill_from_mark() {
        let look = Look::new([31, 173, 189], Badge::None, Some(&fake_claude()));
        let l = look.layers().expect("layers");
        assert!(l.fill[2] > l.fill[0], "fill is the recolored background, got {:?}", l.fill);
        assert_eq!(l.mark.pixel(5, 5).expect("px").alpha(), 0, "background isn't in the mark layer");
        assert!(l.mark.pixel(512, 512).expect("px").alpha() > 200, "the mark is");
    }

    #[test]
    fn dark_layers_keep_the_profile_color() {
        let l = Look::new([31, 173, 189], Badge::Symbol("star"), Some(&fake_claude())).layers().expect("layers");
        let fill_sum: u32 = l.dark_fill.iter().map(|&c| u32::from(c)).sum();
        assert!(fill_sum < 150, "dark background, got {:?}", l.dark_fill);
        assert!(l.dark_fill[2] > l.dark_fill[0], "tinted teal, got {:?}", l.dark_fill);
        let (r, g, b, _) = get(l.mark_dark.pixel(512, 512).expect("px"));
        assert!(g > r + 0.2 && b > r + 0.2, "mark is teal in Dark, got {r} {g} {b}");
    }

    #[test]
    fn icon_json_puts_badge_in_front_and_specializes_dark() {
        let l = Look::new([97, 89, 199], Badge::Symbol("work"), Some(&fake_claude())).layers().expect("layers");
        let doc: serde_json::Value = serde_json::from_str(&l.icon_json()).expect("json");
        assert_eq!(doc["groups"][0]["layers"][0]["name"], "badge");
        assert_eq!(doc["groups"][1]["layers"][0]["image-name-specializations"][1]["appearance"], "dark");
        assert_eq!(doc["fill-specializations"][1]["appearance"], "dark");
        let no_badge = Look::new([97, 89, 199], Badge::None, Some(&fake_claude())).layers().expect("layers");
        let doc: serde_json::Value = serde_json::from_str(&no_badge.icon_json()).expect("json");
        assert_eq!(doc["groups"].as_array().map(Vec::len), Some(1));
    }

    #[test]
    fn icns_header_matches_length() {
        let data = Look::new([1, 2, 3], Badge::None, None).icns().expect("icns");
        assert_eq!(&data[..4], b"icns");
        let len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        assert_eq!(len, data.len());
    }

    #[test]
    fn ico_round_trips_through_the_decoder() {
        let data = Look::new([1, 2, 3], Badge::Symbol("star"), None).ico().expect("ico");
        let pm = decode_image(&data).expect("decode");
        assert_eq!(pm.width(), 256);
    }

    #[test]
    fn decode_bmp_dib_reads_bottom_up_rows() {
        // 1x2 icon: bottom row red, top row blue (stored bottom-up), plus a mask.
        let mut dib = vec![0u8; 40];
        dib[0] = 40;
        dib[4] = 1;
        dib[8] = 4; // height doubled
        dib[14] = 32;
        dib.extend_from_slice(&[0, 0, 255, 255, 255, 0, 0, 255]);
        let pm = decode_bmp_dib(&dib).expect("decode");
        assert_eq!(pm.pixel(0, 0).expect("px").blue(), 255);
        assert_eq!(pm.pixel(0, 1).expect("px").red(), 255);
    }
}
