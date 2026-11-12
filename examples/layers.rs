//! Writes the layered (macOS 26) foreground for a profile look, for checking:
//!   cargo run --example layers claude-icon.png out.png
use profilehub::icons::{Look, decode_image};
use profilehub::profile::Badge;

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next().and_then(|p| decode_image(&std::fs::read(p).ok()?)).expect("pass Claude's icon (PNG or ICO)");
    let out = args.next().unwrap_or_else(|| "foreground.png".into());
    let (fill, fg) = Look::new([97, 89, 199], Badge::Symbol("work"), Some(&src)).layers().expect("layers");
    println!("fill: #{:02x}{:02x}{:02x}", fill[0], fill[1], fill[2]);
    fg.save_png(out).expect("save");
}
