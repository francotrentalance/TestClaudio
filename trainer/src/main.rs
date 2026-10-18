//! Hold'em Trainer: a gamified quiz trainer for 6-max No-Limit Hold'em.

mod ui;

use eframe::egui;
use holdem_trainer::model::Content;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

fn main() -> eframe::Result<()> {
    let content = match Content::from_bytes(
        include_bytes!("../data/cash.bin"),
        include_bytes!("../data/pushfold.bin"),
        include_bytes!("../data/flops.bin"),
    ) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("holdem-trainer: built-in training data is damaged: {e}");
            std::process::exit(1);
        }
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Hold'em Trainer")
        .with_app_id("holdem-trainer")
        .with_inner_size([1280.0, 780.0])
        .with_min_inner_size([1000.0, 660.0]);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("Hold'em Trainer", options, Box::new(|cc| Ok(Box::new(ui::app::App::new(cc, content)))))
}
