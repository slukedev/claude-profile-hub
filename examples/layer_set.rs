//! Writes the layered-icon pieces (fill color and foreground PNG) for a few looks, for
//! previewing macOS 26 styles: cargo run --example layer_set claude-icon.png out-dir
use profilehub::icons::{Look, decode_image};
use profilehub::profile::{Badge, parse_color};

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next().and_then(|p| decode_image(&std::fs::read(p).ok()?)).expect("pass Claude's icon");
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let looks = [("work", "indigo", Badge::Symbol("work")), ("personal", "sage", Badge::Symbol("personal")), ("acme", "teal", Badge::Letter('A')), ("side", "violet", Badge::Symbol("code"))];
    for (name, color, badge) in looks {
        let (fill, fg) = Look::new(parse_color(color).expect("color"), badge, Some(&src)).layers().expect("layers");
        fg.save_png(out.join(format!("{name}.png"))).expect("save");
        println!("{name} #{:02x}{:02x}{:02x}", fill[0], fill[1], fill[2]);
    }
}
