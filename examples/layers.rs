//! Writes the layered (macOS 26) icon for one profile look as an Icon Composer `.icon`
//! folder, for checking it in Icon Composer or with the schema:
//!   cargo run --example layers claude-icon.png out-dir
use profilehub::icons::{Layered, Look, decode_image};
use profilehub::profile::Badge;

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next().and_then(|p| decode_image(&std::fs::read(p).ok()?)).expect("pass Claude's icon (PNG or ICO)");
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| "AppIcon.icon".into()));
    let layers: Layered = Look::new([97, 89, 199], Badge::Symbol("work"), Some(&src)).layers().expect("layers");
    std::fs::create_dir_all(out.join("Assets")).expect("mkdir");
    for (name, png) in layers.assets().expect("assets") {
        std::fs::write(out.join("Assets").join(name), png).expect("write");
    }
    std::fs::write(out.join("icon.json"), layers.icon_json()).expect("write");
    println!("wrote {}", out.display());
}
