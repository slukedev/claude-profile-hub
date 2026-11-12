//! Renders a sheet of every badge, for checking icon changes:
//!   cargo run --example icons out.png [claude-icon.png]
//! Pass Claude's icon (PNG or ICO) to see the recolored look; without it you get the
//! fallback used when Claude's icon can't be found.
use profilehub::icons::{Look, decode_image};
use profilehub::profile::{Badge, COLORS, SYMBOLS};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "icons.png".into());
    let source = args.next().and_then(|p| decode_image(&std::fs::read(p).ok()?));
    let mut badges: Vec<Badge> = SYMBOLS.iter().map(|s| Badge::Symbol(s)).collect();
    badges.extend(['A', 'W', 'Q', '7'].map(Badge::Letter));
    badges.push(Badge::None);
    let cell = 160u32;
    let cols = 6u32;
    let rows = (badges.len() as u32).div_ceil(cols);
    let mut sheet = tiny_skia::Pixmap::new(cell * cols, cell * rows).expect("sheet");
    sheet.fill(tiny_skia::Color::from_rgba8(245, 245, 245, 255));
    for (i, b) in badges.into_iter().enumerate() {
        let rgb = COLORS[i % COLORS.len()].1;
        let icon = Look::new(rgb, b, source.as_ref()).render(cell).expect("render");
        let (x, y) = ((i as u32 % cols) * cell, (i as u32 / cols) * cell);
        sheet.draw_pixmap(
            x as i32,
            y as i32,
            icon.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
    }
    sheet.save_png(&out).expect("save");
}
