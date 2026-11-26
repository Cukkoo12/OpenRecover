#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

fn build_app_icon() -> Arc<eframe::egui::IconData> {
    let size = 32usize;
    let mut rgba = vec![0u8; size * size * 4];
    let center = (size as f32 - 1.0) * 0.5;
    let radius = 14.5f32;

    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            // Rounded square SDF for a clean modern app badge
            let qx = dx.abs() - 10.5;
            let qy = dy.abs() - 10.5;
            let box_dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - 4.0;

            let idx = (y * size + x) * 4;
            if box_dist <= 0.5 {
                let aa = (0.5 - box_dist).clamp(0.0, 1.0);
                // Warm tangerine -> deep burnt orange gradient
                let t = ((x + y) as f32 / ((size * 2) as f32)).clamp(0.0, 1.0);
                let mut r = (249.0 - t * 25.0) as u8;
                let mut g = (115.0 - t * 40.0) as u8;
                let mut b = (22.0 - t * 10.0) as u8;

                // Inner crisp recovery ring & warm white center dot
                if (dist - 6.8).abs() < 1.35 && !(dx > 1.0 && dy < -1.0) {
                    r = 255;
                    g = 255;
                    b = 255;
                } else if dist < 2.3 && dist <= radius {
                    r = 255;
                    g = 237;
                    b = 213;
                }

                rgba[idx] = r;
                rgba[idx + 1] = g;
                rgba[idx + 2] = b;
                rgba[idx + 3] = (aa * 255.0) as u8;
            }
        }
    }

    Arc::new(eframe::egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    })
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("OpenRecover")
            .with_inner_size([1180.0, 740.0])
            .with_min_inner_size([880.0, 560.0])
            .with_icon(build_app_icon()),
        ..Default::default()
    };

    eframe::run_native(
        "OpenRecover",
        options,
        Box::new(|cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(openrecover::ui::OpenRecoverApp::new(cc)))
        }),
    )
}
