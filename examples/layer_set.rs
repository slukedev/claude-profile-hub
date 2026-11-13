//! Writes the layered-icon pieces for a few looks, for previewing macOS 26 styles:
//!   cargo run --example layer_set claude-icon.png out-dir
//! For each look: <name>-mark.png, <name>-mark-dark.png, <name>-badge.png, and the
//! Default and Dark fill colors on stdout.
use profilehub::icons::{Look, decode_image};
use profilehub::profile::{Badge, parse_color};

fn main() {
    let mut args = std::env::args().skip(1);
    let src = args.next().and_then(|p| decode_image(&std::fs::read(p).ok()?)).expect("pass Claude's icon");
    let out = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let looks = [
        ("work", "indigo", Badge::Symbol("work")),
        ("personal", "sage", Badge::Symbol("personal")),
        ("acme", "teal", Badge::Letter('A')),
        ("side", "violet", Badge::Symbol("code")),
    ];
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    for (name, color, badge) in looks {
        let l = Look::new(parse_color(color).expect("color"), badge, Some(&src)).layers().expect("layers");
        l.mark.save_png(out.join(format!("{name}-mark.png"))).expect("save");
        l.mark_dark.save_png(out.join(format!("{name}-mark-dark.png"))).expect("save");
        if let Some(b) = &l.badge {
            b.save_png(out.join(format!("{name}-badge.png"))).expect("save");
        }
        println!("{name} {} {}", hex(l.fill), hex(l.dark_fill));
    }
}
