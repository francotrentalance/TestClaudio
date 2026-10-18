//! Custom-drawn widgets: cards, chips, buttons, bars, grids.

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Response, Rounding, Sense, Shape, Stroke, Ui, Vec2, pos2, vec2};
use holdem_solver::card::{Card, RANK_CHARS, rank, suit};
use holdem_solver::range::{NUM_CLASSES, class_name};

use super::theme::{self, *};

/// What kind of action a button or bar represents (for its color).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ActStyle {
    Fold,
    Passive,
    /// Bet or raise; 0 is the smallest size at the decision.
    Aggressive(usize),
    AllIn,
}

pub fn act_color(s: ActStyle) -> Color32 {
    match s {
        ActStyle::Fold => FOLD,
        ActStyle::Passive => PASSIVE,
        ActStyle::Aggressive(0) => BET_S,
        ActStyle::Aggressive(_) => BET_L,
        ActStyle::AllIn => ALLIN,
    }
}

// ---------------------------------------------------------------------------
// Cards

fn suit_color(s: u8, four_color: bool) -> Color32 {
    match (s, four_color) {
        (2, _) => RED_SUIT,
        (1, true) => BLUE_SUIT,
        (1, false) => RED_SUIT,
        (0, true) => GREEN_SUIT,
        _ => BLACK_SUIT,
    }
}

/// Draws a suit symbol centered at `c` with half-height `s` (0=c, 1=d, 2=h, 3=s).
pub fn draw_suit(p: &egui::Painter, c: Pos2, s: f32, which: u8, color: Color32) {
    let at = |x: f32, y: f32| pos2(c.x + x * s, c.y + y * s);
    let tri = |a: Pos2, b: Pos2, d: Pos2| Shape::convex_polygon(vec![a, b, d], color, Stroke::NONE);
    match which {
        1 => {
            p.add(Shape::convex_polygon(vec![at(0.0, -1.0), at(0.72, 0.0), at(0.0, 1.0), at(-0.72, 0.0)], color, Stroke::NONE));
        }
        2 => {
            p.circle_filled(at(-0.4, -0.3), 0.43 * s, color);
            p.circle_filled(at(0.4, -0.3), 0.43 * s, color);
            p.add(tri(at(-0.81, -0.12), at(0.81, -0.12), at(0.0, 0.92)));
        }
        3 => {
            p.circle_filled(at(-0.4, 0.18), 0.4 * s, color);
            p.circle_filled(at(0.4, 0.18), 0.4 * s, color);
            p.add(tri(at(-0.78, 0.02), at(0.78, 0.02), at(0.0, -0.98)));
            p.add(tri(at(0.0, 0.2), at(-0.3, 0.98), at(0.3, 0.98)));
        }
        _ => {
            p.circle_filled(at(0.0, -0.45), 0.36 * s, color);
            p.circle_filled(at(-0.43, 0.14), 0.36 * s, color);
            p.circle_filled(at(0.43, 0.14), 0.36 * s, color);
            p.circle_filled(at(0.0, 0.05), 0.24 * s, color);
            p.add(tri(at(0.0, 0.05), at(-0.3, 0.98), at(0.3, 0.98)));
        }
    }
}

pub fn rank_text(c: Card) -> String {
    let r = RANK_CHARS[rank(c) as usize] as char;
    if r == 'T' { "10".into() } else { r.to_string() }
}

/// Draws a card face (or its back when `card` is None) filling `rect`.
pub fn draw_card(p: &egui::Painter, rect: Rect, card: Option<Card>, four_color: bool, alpha: f32) {
    let h = rect.height();
    let round = Rounding::same(h * 0.09);
    let fade = |c: Color32| c.gamma_multiply(alpha);
    // Soft shadow.
    p.rect_filled(rect.translate(vec2(0.0, h * 0.035)), round, fade(Color32::from_black_alpha(90)));
    match card {
        Some(c) => {
            p.rect_filled(rect, round, fade(Color32::from_rgb(250, 249, 245)));
            p.rect_stroke(rect, round, Stroke::new(1.0_f32, fade(Color32::from_rgb(205, 205, 200))));
            let col = fade(suit_color(suit(c), four_color));
            let rt = rank_text(c);
            let rank_size = h * if rt.len() > 1 { 0.3 } else { 0.34 };
            p.text(rect.left_top() + vec2(h * 0.07, h * 0.02), Align2::LEFT_TOP, rt, theme::bold(rank_size), col);
            draw_suit(p, rect.left_top() + vec2(h * 0.15, h * 0.47), h * 0.085, suit(c), col);
            draw_suit(p, rect.center() + vec2(h * 0.08, h * 0.14), h * 0.25, suit(c), col);
        }
        None => {
            p.rect_filled(rect, round, fade(Color32::from_rgb(34, 66, 128)));
            let inner = rect.shrink(h * 0.07);
            p.rect_stroke(inner, Rounding::same(h * 0.05), Stroke::new(1.5_f32, fade(Color32::from_rgb(120, 160, 230))));
            let step = h * 0.13;
            let mut x = inner.left() - inner.height();
            while x < inner.right() {
                let a = pos2(x, inner.bottom());
                let b = pos2(x + inner.height(), inner.top());
                let clip = p.with_clip_rect(inner);
                clip.line_segment([a, b], Stroke::new(1.0_f32, fade(Color32::from_rgba_unmultiplied(140, 180, 250, 70))));
                clip.line_segment([pos2(a.x, inner.top()), pos2(b.x, inner.bottom())], Stroke::new(1.0_f32, fade(Color32::from_rgba_unmultiplied(140, 180, 250, 70))));
                x += step;
            }
            p.circle_filled(rect.center(), h * 0.12, fade(GOLD));
            draw_suit(p, rect.center(), h * 0.07, 3, fade(Color32::from_rgb(34, 66, 128)));
        }
    }
}

/// Allocates and draws a card of height `h` in the layout.
pub fn card(ui: &mut Ui, card: Option<Card>, h: f32, four_color: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(h * 0.71, h), Sense::hover());
    draw_card(ui.painter(), rect, card, four_color, 1.0);
    resp
}

// ---------------------------------------------------------------------------
// Chips

pub fn chip_color(amount: f32) -> Color32 {
    if amount >= 50.0 {
        Color32::from_rgb(30, 30, 36)
    } else if amount >= 10.0 {
        Color32::from_rgb(140, 60, 190)
    } else if amount >= 2.0 {
        Color32::from_rgb(214, 52, 60)
    } else {
        Color32::from_rgb(40, 120, 220)
    }
}

pub fn draw_chip(p: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    p.circle_filled(c + vec2(0.0, r * 0.18), r, Color32::from_black_alpha(80));
    p.circle_filled(c, r, color);
    for k in 0..6 {
        let a = k as f32 * std::f32::consts::TAU / 6.0;
        let q = c + vec2(a.cos(), a.sin()) * r * 0.78;
        p.circle_filled(q, r * 0.17, Color32::from_rgb(245, 245, 245));
    }
    p.circle_filled(c, r * 0.55, theme::mix(color, Color32::WHITE, 0.25));
    p.circle_stroke(c, r * 0.55, Stroke::new(1.0_f32, Color32::from_white_alpha(140)));
}

/// A small chip stack with an amount label under/next to it.
pub fn draw_bet(p: &egui::Painter, c: Pos2, amount: f32) {
    let r = 9.0;
    let n = ((amount.log2() + 1.0).clamp(1.0, 4.0)) as i32;
    for k in 0..n {
        draw_chip(p, c + vec2(0.0, -(k as f32) * 4.0), r, chip_color(amount));
    }
    let label = holdem_trainer::quiz::fmt_bb(amount);
    let galley = p.layout_no_wrap(label, theme::semibold(13.0), TEXT);
    let pill = Rect::from_center_size(c + vec2(0.0, r + 11.0), galley.size() + vec2(12.0, 2.0));
    p.rect_filled(pill, Rounding::same(8.0), Color32::from_black_alpha(150));
    p.galley(pill.center() - galley.size() / 2.0, galley, TEXT);
}

// ---------------------------------------------------------------------------
// Buttons, pills, bars

/// A big colored action button. `key` is the keyboard shortcut hint.
pub fn action_button(ui: &mut Ui, label: &str, key: &str, color: Color32, width: f32, enabled: bool) -> Response {
    let height = 56.0;
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), sense);
    let hover = ui.ctx().animate_bool_with_time(resp.id, enabled && resp.hovered(), 0.12);
    let press = enabled && resp.is_pointer_button_down_on();
    let lift = if press { 1.0 } else { -2.0 * hover };
    let r = rect.translate(vec2(0.0, lift));
    let base = if enabled { color } else { theme::mix(color, PANEL, 0.7) };
    let p = ui.painter();
    p.rect_filled(rect.translate(vec2(0.0, 3.0)), Rounding::same(12.0), theme::mix(base, Color32::BLACK, 0.45));
    p.rect_filled(r, Rounding::same(12.0), theme::mix(base, Color32::WHITE, 0.1 * hover));
    p.hline(r.x_range().shrink(10.0), r.top() + 1.0, Stroke::new(1.0_f32, theme::mix(base, Color32::WHITE, 0.3)));
    let light = 0.299 * base.r() as f32 + 0.587 * base.g() as f32 + 0.114 * base.b() as f32 > 165.0;
    let text_col = match (enabled, light) {
        (true, true) => Color32::from_rgb(38, 28, 8),
        (true, false) => Color32::WHITE,
        (false, _) => Color32::from_white_alpha(110),
    };
    p.text(r.center() - vec2(0.0, 6.0), Align2::CENTER_CENTER, label, theme::semibold(16.0), text_col);
    if !key.is_empty() {
        p.text(r.center() + vec2(0.0, 13.0), Align2::CENTER_CENTER, key, theme::font(11.5), text_col.gamma_multiply(0.8));
    }
    if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    }
}

/// A secondary button with a flat, outlined look.
pub fn soft_button(ui: &mut Ui, label: &str, accent: Color32) -> Response {
    let font = theme::medium(15.0);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, TEXT);
    let size = galley.size() + vec2(28.0, 16.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.12);
    let p = ui.painter();
    p.rect_filled(rect, Rounding::same(10.0), theme::mix(PANEL_2, accent, 0.18 + 0.2 * hover));
    p.rect_stroke(rect, Rounding::same(10.0), Stroke::new(1.0_f32, theme::mix(BORDER, accent, 0.6)));
    p.galley(rect.center() - galley.size() / 2.0, galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A large primary button (gold).
pub fn primary_button(ui: &mut Ui, label: &str, width: f32) -> Response {
    action_button(ui, label, "", GOLD, width, true)
}

pub fn pill(ui: &mut Ui, text: &str, fill: Color32, color: Color32) -> Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), theme::semibold(12.5), color);
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(16.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, Rounding::same(rect.height() / 2.0), fill);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
    resp
}

pub fn progress_bar(ui: &mut Ui, frac: f32, color: Color32, width: f32, height: f32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    draw_progress(ui.painter(), rect, frac, color);
    resp
}

pub fn draw_progress(p: &egui::Painter, rect: Rect, frac: f32, color: Color32) {
    let r = Rounding::same(rect.height() / 2.0);
    p.rect_filled(rect, r, Color32::from_rgb(9, 15, 26));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let w = (rect.width() * f).max(rect.height());
        let fill = Rect::from_min_size(rect.min, vec2(w, rect.height()));
        p.rect_filled(fill, r, color);
        let shine = Rect::from_min_size(fill.min, vec2(fill.width(), fill.height() * 0.45));
        p.rect_filled(shine, r, Color32::from_white_alpha(12));
    }
}

/// A circular progress ring with text in the middle.
pub fn ring(p: &egui::Painter, c: Pos2, radius: f32, thickness: f32, frac: f32, color: Color32) {
    p.circle_stroke(c, radius, Stroke::new(thickness, Color32::from_rgb(9, 15, 26)));
    let f = frac.clamp(0.0, 1.0);
    if f > 0.0 {
        let n = (64.0 * f).ceil().max(2.0) as usize;
        let start = -std::f32::consts::FRAC_PI_2;
        let pts: Vec<Pos2> = (0..=n)
            .map(|k| {
                let a = start + std::f32::consts::TAU * f * k as f32 / n as f32;
                c + vec2(a.cos(), a.sin()) * radius
            })
            .collect();
        p.add(Shape::line(pts, Stroke::new(thickness, color)));
    }
}

/// A rounded card-like panel.
pub fn panel_frame(fill: Color32) -> egui::Frame {
    egui::Frame::none()
        .fill(fill)
        .rounding(Rounding::same(14.0))
        .stroke(Stroke::new(1.0_f32, BORDER))
        .inner_margin(egui::Margin::same(16.0))
}

pub fn stat_tile(ui: &mut Ui, title: &str, value: &str, sub: &str, accent: Color32, width: f32) {
    panel_frame(PANEL).show(ui, |ui| {
        ui.set_width(width - 34.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(egui::RichText::new(title).font(theme::font(12.5)).color(TEXT_DIM));
            ui.label(egui::RichText::new(value).font(theme::bold(28.0)).color(accent));
            ui.label(egui::RichText::new(if sub.is_empty() { " " } else { sub }).font(theme::font(12.5)).color(TEXT_FAINT));
        });
    });
}

/// Horizontal frequency bars for each action, the chosen one outlined.
pub fn freq_bars(ui: &mut Ui, labels: &[String], freqs: &[f32], styles: &[ActStyle], chosen: Option<usize>, evs: Option<&[f32]>, width: f32) {
    for i in 0..labels.len() {
        let (rect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
        let p = ui.painter();
        let color = act_color(styles[i]);
        let bar = Rect::from_min_size(rect.min + vec2(0.0, 4.0), vec2(rect.width(), 22.0));
        p.rect_filled(bar, Rounding::same(6.0), Color32::from_rgb(9, 15, 26));
        let w = bar.width() * freqs[i].clamp(0.0, 1.0);
        if w > 0.5 {
            p.rect_filled(Rect::from_min_size(bar.min, vec2(w, bar.height())), Rounding::same(6.0), theme::mix(color, BG, 0.15));
        }
        if chosen == Some(i) {
            p.rect_stroke(bar.expand(1.5), Rounding::same(7.0), Stroke::new(2.0_f32, Color32::WHITE));
        }
        let mut text = labels[i].clone();
        if chosen == Some(i) {
            text = format!("{text}  (you)");
        }
        p.text(bar.left_center() + vec2(8.0, 0.0), Align2::LEFT_CENTER, text, theme::medium(13.0), TEXT);
        let right = match evs {
            Some(e) if e[i].is_finite() => format!("{:.0}%  ·  EV {:+.2}", 100.0 * freqs[i], e[i]),
            _ => format!("{:.0}%", 100.0 * freqs[i]),
        };
        p.text(bar.right_center() - vec2(8.0, 0.0), Align2::RIGHT_CENTER, right, theme::semibold(13.0), TEXT);
    }
}

/// The 13x13 hand grid colored by each class's action mix.
pub fn range_grid(ui: &mut Ui, grid: &[Vec<f32>], styles: &[ActStyle], highlight: Option<usize>, cell: f32) -> Response {
    let size = vec2(13.0 * cell, 13.0 * cell);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect.expand(2.0), Rounding::same(6.0), Color32::from_rgb(9, 15, 26));
    for class in 0..NUM_CLASSES {
        let (row, col) = (class / 13, class % 13);
        let r = Rect::from_min_size(rect.min + vec2(col as f32 * cell, row as f32 * cell), Vec2::splat(cell)).shrink(0.6);
        let mix = &grid[class];
        let mut x = r.left();
        // Paint the aggressive actions first (left), like most solvers.
        let order: Vec<usize> = (0..mix.len()).rev().collect();
        for &a in &order {
            let w = r.width() * mix[a];
            if w > 0.2 {
                p.rect_filled(Rect::from_min_max(pos2(x, r.top()), pos2((x + w).min(r.right()), r.bottom())), Rounding::ZERO, act_color(styles[a]));
            }
            x += w;
        }
        let name = class_name(class);
        p.text(r.center(), Align2::CENTER_CENTER, name, theme::semibold(cell * 0.3), Color32::from_white_alpha(225));
    }
    if let Some(h) = highlight {
        let (row, col) = (h / 13, h % 13);
        let r = Rect::from_min_size(rect.min + vec2(col as f32 * cell, row as f32 * cell), Vec2::splat(cell));
        p.rect_stroke(r.expand(1.0), Rounding::same(3.0), Stroke::new(3.0_f32, Color32::WHITE));
        p.rect_stroke(r.expand(3.0), Rounding::same(4.0), Stroke::new(1.5_f32, GOLD));
    }
    resp
}

pub fn section_title(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).font(theme::semibold(17.0)).color(TEXT));
}

