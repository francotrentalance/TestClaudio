//! Hold'em Trainer: a gamified quiz trainer for 6-max No-Limit Hold'em.

mod ui;

use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::egui;
use holdem_trainer::model::Content;
use holdem_trainer::profile::data_dir;

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

const TITLE: &str = "Hold'em Trainer";

fn icon() -> Option<egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon-256.png")).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(egui::IconData { rgba: img.into_raw(), width, height })
}

/// Shows a message box. Started from the menu, stderr goes nowhere the user
/// sees, so this uses zenity, or a desktop notification without it.
/// `kind` is zenity's dialog type: "--error", "--info", ...
fn tell(kind: &str, text: &str) {
    eprintln!("holdem-trainer: {text}");
    let shown = Command::new("zenity").args([kind, "--no-markup", "--width=420", &format!("--title={TITLE}"), &format!("--text={text}")]).status().is_ok();
    if !shown {
        let body = text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let _ = Command::new("notify-send").args(["--app-name", TITLE, "--", TITLE, &body]).status();
    }
}

/// Appends a line to `crash.log` in the data directory and returns its path.
fn log_crash(text: &str) -> PathBuf {
    let dir = data_dir();
    let path = dir.join("crash.log");
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{} holdem-trainer {}: {text}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), env!("CARGO_PKG_VERSION"));
    }
    path
}

/// Logs a startup failure, tells the user and exits.
fn fatal(text: &str) -> ! {
    let log = log_crash(text);
    tell("--error", &format!("{text}\n\nThis was saved to {}.", log.display()));
    std::process::exit(1);
}

/// Panics are logged to `crash.log` and shown once, on top of the usual
/// message on stderr.
fn install_panic_hook() {
    static SHOWN: AtomicBool = AtomicBool::new(false);
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        default(info);
        let payload = info.payload();
        let msg = payload.downcast_ref::<&str>().copied().or_else(|| payload.downcast_ref::<String>().map(String::as_str)).unwrap_or("unknown error");
        let at = info.location().map(|l| format!(" at {}:{}", l.file(), l.line())).unwrap_or_default();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let log = log_crash(&format!("panic in thread '{thread}'{at}: {msg}"));
        // Only the main thread's panics stop the app: a turn-solver thread's
        // panic just ends that hand (PostEnd::Error).
        if thread == "main" && !SHOWN.swap(true, Ordering::Relaxed) {
            tell("--error", &format!("Hold'em Trainer stopped because of an internal error:\n\n{msg}\n\nDetails were saved to {}.", log.display()));
        }
    }));
}

/// Allows one running copy per user: each copy writes its whole profile on
/// every answer, so a second window would overwrite the first one's progress.
/// The lock goes away with the process, also after a crash.
fn lock_instance() -> Option<std::fs::File> {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(dir.join("lock")).ok()?;
    match file.try_lock() {
        Err(std::fs::TryLockError::WouldBlock) => {
            tell("--info", "Hold'em Trainer is already running. Switch to its window to keep training.");
            std::process::exit(0);
        }
        // A filesystem without locks: run anyway.
        _ => Some(file),
    }
}

fn main() {
    // On Wayland, winit (built without wayland-csd-adwaita) draws a bare
    // fallback title bar, and Muffin draws no frames for Wayland windows. Use
    // XWayland when there is one. Done first, while there is a single thread.
    if std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty()) {
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("WAYLAND_SOCKET");
    }
    install_panic_hook();
    let _lock = lock_instance();

    let content = match Content::from_bytes(
        include_bytes!("../data/cash.bin"),
        include_bytes!("../data/pushfold.bin"),
        include_bytes!("../data/flops.bin"),
    ) {
        Ok(c) => c,
        Err(e) => fatal(&format!("Hold'em Trainer cannot start: its built-in training data is damaged ({e}). Reinstalling the package should fix this.")),
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_app_id("holdem-trainer")
        .with_inner_size([1280.0, 780.0])
        .with_min_inner_size([1000.0, 660.0]);
    if let Some(icon) = icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    if let Err(e) = eframe::run_native(TITLE, options, Box::new(|cc| Ok(Box::new(ui::app::App::new(cc, content))))) {
        fatal(&format!("Hold'em Trainer could not open its window: {e}\n\nIt needs a graphical desktop with OpenGL."));
    }
}
