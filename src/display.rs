//! Terminal rendering helpers: 13x13 hand grids and optional ANSI colors.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::card::RANK_CHARS;
use crate::range::NUM_CLASSES;

static COLOR: AtomicBool = AtomicBool::new(false);

/// Enables colors when stdout is a terminal, unless `NO_COLOR` is set or `disable` is true.
pub fn init_color(disable: bool) {
    let on = !disable && std::env::var_os("NO_COLOR").is_none() && std::io::stdout().is_terminal();
    COLOR.store(on, Ordering::Relaxed);
}

pub fn color_enabled() -> bool {
    COLOR.load(Ordering::Relaxed)
}

/// Kind of an action for coloring.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActionKind {
    Fold,
    Passive,
    /// Aggressive action; `level` 0 is the smallest size at the node.
    Aggressive {
        level: usize,
        allin: bool,
    },
}

fn bg_code(kind: ActionKind) -> u8 {
    match kind {
        ActionKind::Fold => 25,
        ActionKind::Passive => 28,
        ActionKind::Aggressive { allin: true, .. } => 90,
        ActionKind::Aggressive { level, .. } => [172, 166, 160, 124][level.min(3)],
    }
}

pub fn paint(text: &str, kind: ActionKind) -> String {
    if color_enabled() {
        format!("\x1b[48;5;{}m\x1b[97m{}\x1b[0m", bg_code(kind), text)
    } else {
        text.to_string()
    }
}

pub fn dim(text: &str) -> String {
    if color_enabled() { format!("\x1b[2m{text}\x1b[0m") } else { text.to_string() }
}

pub fn bold(text: &str) -> String {
    if color_enabled() { format!("\x1b[1m{text}\x1b[0m") } else { text.to_string() }
}

/// Renders a 13x13 grid. `cell(class)` returns already-padded cell text.
pub fn grid(cell: impl Fn(usize) -> String) -> String {
    let mut out = String::new();
    for row in 0..13 {
        for col in 0..13 {
            let class = row * 13 + col;
            debug_assert!(class < NUM_CLASSES);
            out.push_str(&cell(class));
            out.push(' ');
        }
        out.pop();
        out.push('\n');
    }
    out
}

/// Label of a class cell padded to `width` (e.g. `"AKs  "`).
pub fn class_label(class: usize, width: usize) -> String {
    format!("{:<width$}", crate::range::class_name(class), width = width)
}

pub fn rank_char(r: usize) -> char {
    RANK_CHARS[r] as char
}

/// Percentage with no decimals, e.g. `"63%"`.
pub fn pct0(x: f64) -> String {
    format!("{:.0}%", 100.0 * x)
}

pub fn pct1(x: f64) -> String {
    format!("{:.1}%", 100.0 * x)
}
