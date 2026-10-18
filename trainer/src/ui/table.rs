//! The poker table: felt, seats, bets, pot and board.

use eframe::egui::{self, Align2, Color32, Pos2, Rect, Rounding, Shape, Stroke, Ui, pos2, vec2};
use holdem_solver::card::Card;
use holdem_trainer::quiz::{SeatView, fmt_bb};

use super::theme::{self, *};
use super::widgets::{draw_bet, draw_card};

pub struct TableView<'a> {
    pub seats: &'a [SeatView],
    pub board: &'a [Card],
    pub pot: f32,
    pub four_color: bool,
    /// Message in the middle of the felt (e.g. while the turn is being solved).
    pub banner: Option<String>,
    /// 0..1 fade-in of newly dealt cards.
    pub reveal: f32,
}

fn ellipse(c: Pos2, rx: f32, ry: f32, n: usize) -> Vec<Pos2> {
    (0..n)
        .map(|k| {
            let a = k as f32 * std::f32::consts::TAU / n as f32;
            pos2(c.x + rx * a.cos(), c.y + ry * a.sin())
        })
        .collect()
}

/// Seat angles (degrees, clockwise from the bottom) for `n` players.
fn slots(n: usize) -> Vec<f32> {
    match n {
        2 => vec![90.0, 270.0],
        3 => vec![90.0, 210.0, 330.0],
        _ => (0..n).map(|k| 90.0 + 360.0 * k as f32 / n as f32).collect(),
    }
}

pub fn draw_table(ui: &mut Ui, rect: Rect, view: &TableView) {
    let p = ui.painter_at(rect);
    let c = rect.center() + vec2(0.0, rect.height() * 0.02);
    let rx = rect.width() * 0.36;
    // Short windows: keep the hero's action pill (c.y + ry + 80) inside the rect.
    let ry = (rect.height() * 0.29).min(rect.height() * 0.48 - 84.0);

    // Rail, felt and an inner line.
    p.add(Shape::convex_polygon(ellipse(c + vec2(0.0, 10.0), rx + 30.0, ry + 30.0, 96), Color32::from_black_alpha(110), Stroke::NONE));
    p.add(Shape::convex_polygon(ellipse(c, rx + 26.0, ry + 26.0, 96), RAIL, Stroke::new(2.0_f32, RAIL_LIGHT)));
    p.add(Shape::convex_polygon(ellipse(c, rx, ry, 96), FELT_DARK, Stroke::NONE));
    p.add(Shape::convex_polygon(ellipse(c - vec2(0.0, ry * 0.06), rx * 0.94, ry * 0.86, 96), FELT, Stroke::NONE));
    p.add(Shape::closed_line(ellipse(c, rx * 0.78, ry * 0.7, 96), Stroke::new(1.5_f32, Color32::from_white_alpha(22))));

    // Board.
    // Narrow tables: keep the board inside the side seats' bets.
    let ch = (ry * 0.42).min(rx * 0.21).clamp(44.0, 86.0);
    let cw = ch * 0.71;
    let gap = cw * 0.12;
    let total = 5.0 * cw + 4.0 * gap;
    let board_top = c.y - ch * 0.5;
    for k in 0..5 {
        let r = Rect::from_min_size(pos2(c.x - total / 2.0 + k as f32 * (cw + gap), board_top), vec2(cw, ch));
        if let Some(&card) = view.board.get(k) {
            let alpha = if k + 1 == view.board.len() { view.reveal } else { 1.0 };
            draw_card(&p, r, Some(card), view.four_color, alpha);
        } else {
            p.rect_stroke(r, Rounding::same(ch * 0.09), Stroke::new(1.0_f32, Color32::from_white_alpha(26)));
        }
    }

    // Pot.
    let mut pot_top = board_top;
    if view.pot > 0.0 {
        let galley = p.layout_no_wrap(format!("Pot  {} bb", fmt_bb(view.pot)), theme::semibold(16.0), TEXT);
        let pill = Rect::from_center_size(pos2(c.x, board_top - 26.0), galley.size() + vec2(26.0, 8.0));
        pot_top = pill.top();
        p.rect_filled(pill, Rounding::same(pill.height() / 2.0), Color32::from_black_alpha(120));
        p.galley(pill.center() - galley.size() / 2.0, galley, TEXT);
    }

    // Seats, rotated so the hero sits at the bottom.
    let n = view.seats.len();
    let hero = view.seats.iter().position(|s| s.is_hero).unwrap_or(0);
    let angles = slots(n);
    for (k, &deg) in angles.iter().enumerate() {
        let seat = &view.seats[(hero + k) % n];
        let a = deg.to_radians();
        let dir = vec2(a.cos(), a.sin());
        let spot = c + vec2(dir.x * (rx + 34.0), dir.y * (ry + 30.0));
        let hero_h = (ry * 0.56).clamp(62.0, 92.0);
        draw_seat(&p, spot, seat, view.four_color, hero_h);

        // Bet in front of the seat; the hero's goes beside their cards, the
        // top seat's stays between its action pill and the pot (or beside the
        // nameplate when a short table leaves no room there).
        if seat.bet > 0.005 {
            let b = if seat.is_hero {
                spot + vec2(-(hero_h * 0.71 * 1.08 + 40.0), -hero_h * 0.75)
            } else if dir.y < -0.9 {
                let lo = spot.y + 26.0 + 24.0 + 21.0;
                let hi = pot_top - 33.0;
                if lo <= hi { pos2(c.x, (c.y - ry * 0.74).clamp(lo, hi)) } else { spot + vec2(-98.0, 4.0) }
            } else {
                c + vec2(dir.x * rx * 0.62, dir.y * ry * 0.58)
            };
            draw_bet(&p, b, seat.bet);
        }
        // Dealer button.
        if seat.is_button {
            // Beside the nameplate, on the side facing the middle of the table.
            let half = if seat.is_hero { 75.0 } else { 64.0 };
            let side = if dir.x > 0.3 { -1.0 } else { 1.0 };
            let d = spot + vec2(side * (half + 18.0), -14.0);
            p.circle_filled(d + vec2(0.0, 2.0), 11.0, Color32::from_black_alpha(90));
            p.circle_filled(d, 11.0, Color32::from_rgb(245, 245, 240));
            p.text(d, Align2::CENTER_CENTER, "D", theme::bold(12.0), Color32::from_rgb(30, 30, 30));
        }
    }

    if let Some(msg) = &view.banner {
        let galley = p.layout_no_wrap(msg.clone(), theme::semibold(16.0), TEXT);
        let box_ = Rect::from_center_size(c + vec2(0.0, ch * 0.95), galley.size() + vec2(30.0, 14.0));
        p.rect_filled(box_, Rounding::same(10.0), Color32::from_black_alpha(170));
        p.galley(box_.center() - galley.size() / 2.0, galley, TEXT);
    }
}

fn draw_seat(p: &egui::Painter, at: Pos2, seat: &SeatView, four_color: bool, hero_h: f32) {
    let w = if seat.is_hero { 150.0 } else { 128.0 };
    let plate = Rect::from_center_size(at, vec2(w, 52.0));
    let dim = if seat.folded { 0.45 } else { 1.0 };

    // The hero's cards stand above the nameplate; the others peek out from behind it.
    if seat.has_cards || seat.cards.is_some() {
        let (ch, base, spread) = if seat.is_hero {
            (hero_h, pos2(at.x, plate.top() - hero_h * 0.42), 1.08)
        } else {
            let ch = 50.0;
            (ch, pos2(at.x, plate.top() - ch * 0.16), 0.66)
        };
        let cw = ch * 0.71;
        for k in 0..2 {
            let off = (k as f32 - 0.5) * cw * spread;
            let r = Rect::from_center_size(base + vec2(off, 0.0), vec2(cw, ch));
            let face = seat.cards.map(|cs| cs[k]);
            draw_card(p, r, face, four_color, 1.0);
            if seat.folded {
                p.rect_filled(r, Rounding::same(ch * 0.09), Color32::from_black_alpha(150));
            }
        }
    }

    // A steady glow: the app only repaints on input, so a pulse would stutter.
    if seat.to_act {
        p.rect_filled(plate.expand(5.0), Rounding::same(16.0), theme::mix(TEAL, BG, 0.55).gamma_multiply(0.7));
    }
    let fill = if seat.is_hero { Color32::from_rgb(30, 44, 70) } else { Color32::from_rgb(22, 32, 52) };
    let fill = if seat.folded { theme::mix(fill, BG, 0.35) } else { fill };
    p.rect_filled(plate, Rounding::same(14.0), fill);
    let border = if seat.to_act {
        TEAL
    } else if seat.is_hero {
        GOLD
    } else {
        BORDER
    };
    p.rect_stroke(plate, Rounding::same(14.0), Stroke::new(if seat.is_hero || seat.to_act { 2.0_f32 } else { 1.0 }, border));
    let name = if seat.is_hero { format!("{}  (you)", seat.name) } else { seat.name.clone() };
    p.text(plate.center() - vec2(0.0, 9.0), Align2::CENTER_CENTER, name, theme::semibold(15.0), TEXT.gamma_multiply(dim));
    let stack = if seat.all_in && !seat.folded { "ALL-IN".to_string() } else { format!("{} bb", fmt_bb(seat.stack)) };
    let stack_col = if seat.all_in && !seat.folded { ALLIN } else { TEXT_DIM };
    p.text(plate.center() + vec2(0.0, 12.0), Align2::CENTER_CENTER, stack, theme::medium(13.5), stack_col.gamma_multiply(dim));

    // Last action under the plate.
    if let Some(a) = &seat.last_action {
        let col = if a.starts_with("Fold") {
            FOLD
        } else if a.starts_with("Check") || a.starts_with("Call") {
            PASSIVE
        } else if a.starts_with("All") {
            ALLIN
        } else {
            BET_S
        };
        let galley = p.layout_no_wrap(a.clone(), theme::semibold(12.5), Color32::WHITE);
        let r = Rect::from_center_size(pos2(at.x, plate.bottom() + 14.0), galley.size() + vec2(14.0, 4.0));
        p.rect_filled(r, Rounding::same(r.height() / 2.0), col.gamma_multiply(0.9));
        p.galley(r.center() - galley.size() / 2.0, galley, Color32::WHITE);
    }
}
