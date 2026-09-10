use eframe::egui;

pub const TRAY_SIZE: u32 = 32;
pub const TRAY_RGBA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/icon-32.rgba"));

pub fn window() -> egui::IconData {
    egui::IconData {
        rgba: include_bytes!(concat!(env!("OUT_DIR"), "/icon-256.rgba")).to_vec(),
        width: 256,
        height: 256,
    }
}

pub fn texture(ctx: &egui::Context) -> egui::TextureHandle {
    ctx.load_texture(
        "relayhop-brand",
        egui::ColorImage::from_rgba_unmultiplied(
            [128, 128],
            include_bytes!(concat!(env!("OUT_DIR"), "/icon-128.rgba")),
        ),
        egui::TextureOptions::LINEAR,
    )
}

#[cfg(target_os = "linux")]
pub fn argb() -> Vec<u8> {
    TRAY_RGBA
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[3], p[0], p[1], p[2]])
        .collect()
}
