use std::{env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=assets/relayhop.svg");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let svg = fs::read("assets/relayhop.svg").expect("read app icon SVG");
    let tree = resvg::usvg::Tree::from_data(&svg, &resvg::usvg::Options::default())
        .expect("parse app icon SVG");
    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let mut pixmap = resvg::tiny_skia::Pixmap::new(size, size).unwrap();
        let scale = size as f32 / tree.size().width();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        // resvg stores premultiplied pixels; window/tray APIs expect straight RGBA.
        let mut rgba = pixmap.data().to_vec();
        let (chunks, _) = rgba.as_chunks_mut::<4>();
        for pixel in chunks {
            let alpha = u32::from(pixel[3]);
            if let Some(alpha) = std::num::NonZero::new(alpha) {
                for component in &mut pixel[..3] {
                    *component =
                        ((u32::from(*component) * 255 + alpha.get() / 2) / alpha).min(255) as u8;
                }
            }
        }
        fs::write(output.join(format!("icon-{size}.rgba")), &rgba).unwrap();
        ico.add_entry(
            ico::IconDirEntry::encode(&ico::IconImage::from_rgba_data(size, size, rgba)).unwrap(),
        );
    }
    let icon_path = output.join("relayhop.ico");
    ico.write(fs::File::create(&icon_path).unwrap()).unwrap();
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon(icon_path.to_str().unwrap())
            .set("ProductName", "RelayHop")
            .set("FileDescription", "RelayHop — Discord launcher")
            .compile()
            .expect("compile Windows icon resource");
    }
}
