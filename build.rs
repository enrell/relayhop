use std::{env, fs, path::PathBuf};

use image::{GenericImageView, imageops::FilterType};

fn main() {
    println!("cargo:rerun-if-changed=assets/relayhop.png");
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let source = image::open("assets/relayhop.png").expect("read app icon PNG");
    assert_eq!(
        source.dimensions().0,
        source.dimensions().1,
        "app icon must be square"
    );
    let mut ico = ico::IconDir::new(ico::ResourceType::Icon);
    for size in [16, 24, 32, 48, 64, 128, 256] {
        let resized = source.resize_exact(size, size, FilterType::Lanczos3);
        let rgba = resized.into_rgba8().into_raw();
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
