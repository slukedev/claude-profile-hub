//! Draws profile icons (a colored tile with a symbol or letter) and writes them as
//! PNG, ICO and ICNS, so every platform's shortcuts can use them.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font, FontRef, OutlineCurve};
use anyhow::{Context, Result};
use serde::Deserialize;
use tiny_skia::{
    Color, FillRule, GradientStop, LineCap, LineJoin, LinearGradient, Paint, PathBuilder, Pixmap, Point, Rect,
    SpreadMode, Stroke, Transform,
};

use crate::profile::{Badge, Profile};

const GLYPHS: &str = include_str!("../assets/glyphs.json");
const FONT: &[u8] = include_bytes!("../assets/ArchivoBlack-Regular.ttf");

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
    /// Drawn in the tile color, to cut a detail out of a white shape.
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

/// Renders a square icon `dim` pixels wide.
pub fn render(dim: u32, rgb: [u8; 3], badge: &Badge) -> Option<Pixmap> {
    let mut pm = Pixmap::new(dim, dim)?;
    let d = dim as f32;
    let inset = d * 0.08;
    let tile = Rect::from_xywh(inset, inset, d - inset * 2.0, d - inset * 2.0)?;

    // Tile: the profile color, a touch lighter at the top.
    let top = lighten(rgb, 0.14);
    let shader = LinearGradient::new(
        Point::from_xy(0.0, tile.top()),
        Point::from_xy(0.0, tile.bottom()),
        vec![
            GradientStop::new(0.0, Color::from_rgba8(top[0], top[1], top[2], 255)),
            GradientStop::new(1.0, Color::from_rgba8(rgb[0], rgb[1], rgb[2], 255)),
        ],
        SpreadMode::Pad,
        Transform::identity(),
    )?;
    let tile_paint = Paint { shader, anti_alias: true, ..Paint::default() };
    pm.fill_path(&rounded_rect(tile, d * 0.22)?, &tile_paint, FillRule::Winding, Transform::identity(), None);

    // Thin inner ring.
    let ring_inset = d * 0.14;
    let inner = Rect::from_xywh(
        tile.left() + ring_inset,
        tile.top() + ring_inset,
        tile.width() - ring_inset * 2.0,
        tile.height() - ring_inset * 2.0,
    )?;
    let ring = rounded_rect(inner, inner.width() * 0.18)?;
    let stroke = Stroke { width: (d * 0.03).max(1.0), ..Stroke::default() };
    pm.stroke_path(&ring, &solid([255, 255, 255], 0.35), &stroke, Transform::identity(), None);

    match badge {
        Badge::Symbol(name) => {
            if let Some(sym) = glyphs().iter().find(|s| s.name == *name) {
                draw_symbol(&mut pm, sym, d, &tile_paint);
            }
        }
        Badge::Letter(c) => draw_letter(&mut pm, *c, d),
        Badge::None => {}
    }
    Some(pm)
}

fn lighten(rgb: [u8; 3], amount: f32) -> [u8; 3] {
    rgb.map(|c| (f32::from(c) + (255.0 - f32::from(c)) * amount).round() as u8)
}

fn solid(rgb: [u8; 3], alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(rgb[0], rgb[1], rgb[2], (alpha.clamp(0.0, 1.0) * 255.0).round() as u8);
    p.anti_alias = true;
    p
}

fn rounded_rect(r: Rect, radius: f32) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    push_rounded_rect(&mut pb, r, radius);
    pb.finish()
}

fn push_rounded_rect(pb: &mut PathBuilder, r: Rect, radius: f32) {
    let rad = radius.min(r.width() / 2.0).min(r.height() / 2.0);
    // Cubic approximation of a quarter circle.
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

fn draw_symbol(pm: &mut Pixmap, sym: &Symbol, d: f32, tile: &Paint<'_>) {
    // Glyph coordinates are fractions of the icon with the origin at the bottom left.
    let map = |x: f32, y: f32| (x * d, (1.0 - y) * d);
    for shape in &sym.shapes {
        let mut pb = PathBuilder::new();
        for part in &shape.parts {
            if let Some([cx, cy, r]) = part.circle {
                let (x, y) = map(cx, cy);
                pb.push_circle(x, y, r * d);
            } else if let Some([cx, cy, rx, ry]) = part.ellipse {
                let (x, y) = map(cx, cy);
                if let Some(rect) = Rect::from_xywh(x - rx * d, y - ry * d, rx * 2.0 * d, ry * 2.0 * d) {
                    pb.push_oval(rect);
                }
            } else if let Some([x, y, w, h, r]) = part.rrect {
                // (x, y) is the bottom-left corner in glyph space.
                let (sx, sy) = map(x, y + h);
                if let Some(rect) = Rect::from_xywh(sx, sy, w * d, h * d) {
                    push_rounded_rect(&mut pb, rect, r * d);
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
                    let n: Vec<f32> = op.iter().skip(1).filter_map(|v| v.as_f64()).map(|v| v as f32).collect();
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
        let paint = if shape.background { tile.clone() } else { solid([255, 255, 255], shape.alpha.unwrap_or(1.0)) };
        if let Some(w) = shape.stroke {
            let stroke = Stroke {
                width: (w * d).max(1.0),
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

fn draw_letter(pm: &mut Pixmap, c: char, d: f32) {
    let Ok(font) = FontRef::try_from_slice(FONT) else { return };
    let glyph = font.glyph_id(c);
    let Some(outline) = font.outline(glyph) else { return };
    // ab_glyph reports outline bounds with min.y/max.y swapped, so normalize them.
    let b = outline.bounds;
    let (x0, x1) = (b.min.x.min(b.max.x), b.min.x.max(b.max.x));
    let (y0, y1) = (b.min.y.min(b.max.y), b.min.y.max(b.max.y));
    let (w, h) = (x1 - x0, y1 - y0);
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    // Scale so the letter is 32% of the icon tall, but never wider than 40%.
    let scale = (d * 0.32 / h).min(d * 0.40 / w);
    let ox = d / 2.0 - (x0 + w / 2.0) * scale;
    let oy = d / 2.0 + (y0 + h / 2.0) * scale;
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
        let end = match curve {
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
        };
        last = Some(end);
    }
    pb.close();
    if let Some(path) = pb.finish() {
        pm.fill_path(&path, &solid([255, 255, 255], 1.0), FillRule::Winding, Transform::identity(), None);
    }
}

fn png(dim: u32, rgb: [u8; 3], badge: &Badge) -> Result<Vec<u8>> {
    render(dim, rgb, badge).context("couldn't draw the icon")?.encode_png().context("couldn't encode the icon")
}

/// An `.icns` made of PNG entries, which macOS has read since 10.7.
pub fn icns(rgb: [u8; 3], badge: &Badge) -> Result<Vec<u8>> {
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
        let data = png(*dim, rgb, badge)?;
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
pub fn ico(rgb: [u8; 3], badge: &Badge) -> Result<Vec<u8>> {
    const SIZES: &[u32] = &[16, 24, 32, 48, 64, 256];
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| png(s, rgb, badge)).collect::<Result<_>>()?;
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

/// Icon files for one profile.
#[derive(Debug, Clone)]
pub struct IconSet {
    pub png: PathBuf,
    pub ico: PathBuf,
    pub icns: PathBuf,
}

/// Writes a profile's icons into `dir`, named by their look so a changed icon gets a new
/// file name (Windows and Linux cache icons by path).
pub fn write_for(dir: &Path, profile: &Profile) -> Result<IconSet> {
    let badge = profile.badge();
    let rgb = profile.rgb();
    let mut tag = String::new();
    for b in rgb {
        let _ = write!(tag, "{b:02x}");
    }
    let stem = format!("{}-{}-{}", profile.slug(), tag, badge.as_config().to_lowercase());
    std::fs::create_dir_all(dir).with_context(|| format!("couldn't create {}", dir.display()))?;
    let set = IconSet {
        png: dir.join(format!("{stem}.png")),
        ico: dir.join(format!("{stem}.ico")),
        icns: dir.join(format!("{stem}.icns")),
    };
    if !set.png.exists() {
        std::fs::write(&set.png, png(512, rgb, &badge)?)?;
    }
    if cfg!(windows) && !set.ico.exists() {
        std::fs::write(&set.ico, ico(rgb, &badge)?)?;
    }
    if cfg!(target_os = "macos") && !set.icns.exists() {
        std::fs::write(&set.icns, icns(rgb, &badge)?)?;
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::SYMBOLS;

    #[test]
    fn glyph_file_has_every_symbol() {
        let names: Vec<&str> = glyphs().iter().map(|s| s.name.as_str()).collect();
        for s in SYMBOLS {
            assert!(names.contains(s), "missing {s}");
        }
    }

    #[test]
    fn render_draws_white_badge_over_tile() {
        let pm = render(128, [97, 89, 199], &Badge::Letter('I')).expect("render");
        let center = pm.pixel(64, 64).expect("pixel");
        let corner = pm.pixel(1, 1).expect("pixel");
        assert_eq!(corner.alpha(), 0, "outside the tile is transparent");
        assert!(center.red() > 200 && center.green() > 200, "letter is white");
    }

    #[test]
    fn every_symbol_renders_something_white() {
        for s in SYMBOLS {
            let pm = render(64, [0, 0, 0], &Badge::Symbol(s)).expect("render");
            let white = pm.pixels().iter().filter(|p| p.red() > 200).count();
            assert!(white > 40, "{s} drew {white} white pixels");
        }
    }

    #[test]
    fn icns_header_matches_length() {
        let data = icns([1, 2, 3], &Badge::None).expect("icns");
        assert_eq!(&data[..4], b"icns");
        let len = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
        assert_eq!(len, data.len());
    }

    #[test]
    fn ico_directory_points_at_pngs() {
        let data = ico([1, 2, 3], &Badge::Symbol("star")).expect("ico");
        assert_eq!(u16::from_le_bytes([data[4], data[5]]), 6);
        let offset = u32::from_le_bytes([data[18], data[19], data[20], data[21]]) as usize;
        assert_eq!(&data[offset + 1..offset + 4], b"PNG");
    }
}
