//! Renders a sheet of every badge, for eyeballing icon changes: cargo run --example icons <out.png>
use profilehub::icons::render;
use profilehub::profile::{Badge, COLORS, SYMBOLS};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "icons.png".into());
    let mut badges: Vec<Badge> = SYMBOLS.iter().map(|s| Badge::Symbol(s)).collect();
    badges.extend(['A', 'W', 'Q', '7'].map(Badge::Letter));
    badges.push(Badge::None);
    let cell = 160u32;
    let cols = 6u32;
    let rows = (badges.len() as u32).div_ceil(cols);
    let mut sheet = tiny_skia::Pixmap::new(cell * cols, cell * rows).expect("sheet");
    sheet.fill(tiny_skia::Color::from_rgba8(245, 245, 245, 255));
    for (i, b) in badges.iter().enumerate() {
        let rgb = COLORS[i % COLORS.len()].1;
        let icon = render(cell, rgb, b).expect("render");
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
