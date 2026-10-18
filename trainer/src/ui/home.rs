//! The home screen: progress, daily goal and the three drills.

use eframe::egui::{self, Align2, Color32, Layout, Margin, Rect, Rounding, Sense, Stroke, pos2, vec2};
use holdem_trainer::grading::{title_for_level, xp_for_level};
use holdem_trainer::profile::Mode;

use super::app::{App, Screen};
use super::theme::{self, *};
use super::widgets::{self, draw_card, draw_chip};

/// Centers a column of at most `max` width in the available space.
pub fn centered(ui: &mut egui::Ui, max: f32, add: impl FnOnce(&mut egui::Ui)) {
    let avail = ui.available_width();
    let w = avail.min(max);
    ui.horizontal(|ui| {
        ui.add_space(((avail - w) / 2.0).max(0.0));
        ui.vertical(|ui| {
            ui.set_width(w);
            add(ui);
        });
    });
}

/// Short windows (1366x768 laptops, the 660px minimum) get a tighter layout so
/// the drills' Train buttons stay above the fold.
fn compact(ctx: &egui::Context) -> bool {
    ctx.screen_rect().height() < 760.0
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let mut start: Option<Mode> = None;
    let tight = compact(ctx);
    let margin = Margin::symmetric(24.0, if tight { 12.0 } else { 20.0 });
    egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(margin)).show(ctx, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            centered(ui, 1120.0, |ui| {
                hero_row(ui, app);
                ui.add_space(if tight { 12.0 } else { 22.0 });
                ui.label(egui::RichText::new("Choose a drill").font(theme::semibold(20.0)).color(TEXT));
                ui.add_space(6.0);
                let w = ui.available_width();
                let gap = 16.0;
                let card_w = (w - 2.0 * gap) / 3.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for mode in [Mode::Cash, Mode::PushFold, Mode::Postflop] {
                        if mode_card(ui, app, mode, card_w) {
                            start = Some(mode);
                        }
                    }
                });
                ui.add_space(22.0);
                mistakes(ui, app);
            });
        });
    });
    if let Some(mode) = start {
        // The running drill's card resumes it instead of starting over.
        if app.session.as_ref().is_some_and(|s| s.mode == mode) {
            app.screen = Screen::Train;
        } else {
            app.start(mode);
        }
    }
}

fn hero_row(ui: &mut egui::Ui, app: &App) {
    let p = &app.profile;
    let level = p.level();
    let lo = xp_for_level(level);
    let hi = xp_for_level(level + 1);
    let frac = (p.xp - lo) as f32 / (hi - lo).max(1) as f32;
    let w = ui.available_width();
    let goal_w = 300.0;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        // Level card.
        let level_card = widgets::panel_frame(PANEL).show(ui, |ui| {
            ui.set_width(w - goal_w - 16.0 - 32.0);
            ui.set_height(176.0);
            ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(86.0, 86.0), Sense::hover());
                let c = r.center();
                ui.painter().circle_filled(c, 40.0, theme::mix(PANEL, GOLD, 0.16));
                ui.painter().circle_stroke(c, 40.0, Stroke::new(3.0_f32, GOLD));
                ui.painter().text(c - vec2(0.0, 9.0), Align2::CENTER_CENTER, level.to_string(), theme::bold(32.0), GOLD);
                ui.painter().text(c + vec2(0.0, 19.0), Align2::CENTER_CENTER, "LEVEL", theme::semibold(10.5), TEXT_DIM);
                ui.add_space(8.0);
                ui.vertical(|ui| {
                    let greeting = if p.decisions == 0 { "Welcome to the tables" } else { "Welcome back" };
                    ui.label(egui::RichText::new(greeting).font(theme::font(14.0)).color(TEXT_DIM));
                    ui.label(egui::RichText::new(title_for_level(level)).font(theme::bold(28.0)).color(TEXT));
                    ui.add_space(2.0);
                    let bar_w = ui.available_width().min(460.0);
                    widgets::progress_bar(ui, frac, GOLD, bar_w, 12.0);
                    ui.label(
                        egui::RichText::new(format!("{} XP  ·  {} XP to level {}", p.xp, hi - p.xp, level + 1))
                            .font(theme::font(12.5))
                            .color(TEXT_DIM),
                    );
                });
            });
            ui.add_space(8.0);
            let all = [Mode::Cash, Mode::PushFold, Mode::Postflop].map(|m| p.mode_stats(m));
            let decisions: u32 = all.iter().map(|s| s.decisions).sum();
            let correct: u32 = all.iter().map(|s| s.grades[0] + s.grades[1]).sum();
            let acc = if decisions > 0 { format!("{:.0}%", 100.0 * correct as f32 / decisions as f32) } else { "–".into() };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 30.0;
                for (label, value, color) in [
                    ("Decisions", p.decisions.to_string(), TEXT),
                    ("Accuracy", acc, TEAL),
                    ("Streak", format!("🔥 {}", p.streak), if p.streak > 0 { FLAME } else { TEXT_FAINT }),
                    ("Best streak", p.best_streak.to_string(), TEXT),
                    ("Day streak", match p.day_streak() { 1 => "1 day".to_string(), n => format!("{n} days") }, TEXT),
                ] {
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new(value).font(theme::bold(18.0)).color(color));
                        ui.label(egui::RichText::new(label).font(theme::font(12.0)).color(TEXT_FAINT));
                    });
                }
            });
            });
        });
        // Daily goal card, as tall as the level card.
        let row_h = level_card.response.rect.height() - 32.0;
        widgets::panel_frame(PANEL).show(ui, |ui| {
            ui.set_width(goal_w - 32.0);
            ui.set_height(row_h);
            let goal = p.settings.daily_goal.max(1);
            let today = p.today_count();
            ui.horizontal(|ui| {
                ui.add_space(0.0);
                let (r, _) = ui.allocate_exact_size(vec2(118.0, row_h), Sense::hover());
                let done = today >= goal;
                let color = if done { Color32::from_rgb(61, 220, 132) } else { TEAL };
                widgets::ring(ui.painter(), r.center(), 50.0, 10.0, today as f32 / goal as f32, color);
                ui.painter().text(r.center() - vec2(0.0, 8.0), Align2::CENTER_CENTER, today.to_string(), theme::bold(28.0), TEXT);
                ui.painter().text(r.center() + vec2(0.0, 18.0), Align2::CENTER_CENTER, format!("of {goal}"), theme::font(12.5), TEXT_DIM);
                ui.vertical(|ui| {
                    ui.add_space(((row_h - 88.0) / 2.0).max(0.0));
                    ui.label(egui::RichText::new("Daily goal").font(theme::semibold(17.0)).color(TEXT));
                    let msg = if done { "Done for today. Nice work!".to_string() } else { format!("{} decisions to go", goal - today) };
                    ui.label(egui::RichText::new(msg).font(theme::font(13.0)).color(TEXT_DIM));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new("Change it in Settings").font(theme::font(11.5)).color(TEXT_FAINT));
                });
            });
        });
    });
}

fn mode_info(app: &App, mode: Mode) -> (&'static str, String, Color32) {
    match mode {
        Mode::Cash => (
            "Preflop Cash",
            "6-max, 100bb deep. Opens, 3-bets, 4-bets and calls from every seat; opponents play the solution.".into(),
            Color32::from_rgb(214, 52, 60),
        ),
        Mode::PushFold => {
            let stacks = app.content.pushfold_stacks();
            let range = match (stacks.first(), stacks.last()) {
                (Some(a), Some(b)) => format!("{}–{}bb", a, b),
                _ => "short".into(),
            };
            (
                "Push / Fold",
                format!("Tournament short stacks ({range}, with antes). Shove or fold, and call or fold against a shove."),
                Color32::from_rgb(140, 60, 190),
            )
        }
        Mode::Postflop => (
            "Postflop",
            format!(
                "BTN vs BB single-raised and 3-bet pots on {} flop{}. Turn and river are solved live for the exact board.",
                app.content.flops.spots.len(),
                if app.content.flops.spots.len() == 1 { "" } else { "s" }
            ),
            Color32::from_rgb(40, 120, 220),
        ),
    }
}

fn draw_mode_art(p: &egui::Painter, r: Rect, mode: Mode, accent: Color32) {
    let c = r.center();
    match mode {
        Mode::Cash => {
            let h = r.height() * 0.78;
            let w = h * 0.71;
            let a = Rect::from_center_size(c + vec2(-w * 0.32, 4.0), vec2(w, h));
            let b = Rect::from_center_size(c + vec2(w * 0.32, -2.0), vec2(w, h));
            draw_card(p, a, Some(holdem_solver::card::make_card(12, 3)), true, 1.0);
            draw_card(p, b, Some(holdem_solver::card::make_card(11, 2)), true, 1.0);
        }
        Mode::PushFold => {
            // Drawn for a 98px tall banner; scaled down in the compact one.
            let s = (r.height() / 98.0).min(1.0);
            for k in 0..5 {
                draw_chip(p, c + vec2(-22.0, 22.0 - 7.0 * k as f32) * s, 18.0 * s, accent);
            }
            for k in 0..3 {
                draw_chip(p, c + vec2(22.0, 22.0 - 7.0 * k as f32) * s, 18.0 * s, Color32::from_rgb(30, 30, 36));
            }
        }
        Mode::Postflop => {
            let h = r.height() * 0.56;
            let w = h * 0.71;
            let cards = [(12, 0), (9, 1), (4, 2)];
            for (k, &(rank, suit)) in cards.iter().enumerate() {
                let x = (k as f32 - 1.0) * (w + 5.0);
                draw_card(p, Rect::from_center_size(c + vec2(x, 0.0), vec2(w, h)), Some(holdem_solver::card::make_card(rank, suit)), true, 1.0);
            }
        }
    }
}

/// Returns true when the drill should start (or resume). The whole card is clickable.
fn mode_card(ui: &mut egui::Ui, app: &App, mode: Mode, width: f32) -> bool {
    let (title, desc, accent) = mode_info(app, mode);
    let stats = app.profile.mode_stats(mode);
    let resume = app.session.as_ref().is_some_and(|s| s.mode == mode);
    let tight = compact(ui.ctx());
    let banner_h = if tight { 56.0 } else { 118.0 };
    let mut go = false;
    let frame = widgets::panel_frame(PANEL).inner_margin(Margin::same(0.0));
    // The card's click sense sits below its widgets, so the Train button keeps its
    // own hover; the labels must not take clicks for text selection.
    let card = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
        ui.style_mut().interaction.selectable_labels = false;
        frame.show(ui, |ui| {
        ui.set_width(width);
        ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        // Art banner.
        let (banner, _) = ui.allocate_exact_size(vec2(width, banner_h), Sense::hover());
        let p = ui.painter();
        let top = Rounding { nw: 14.0, ne: 14.0, sw: 0.0, se: 0.0 };
        p.rect_filled(banner, top, theme::mix(FELT_DARK, accent, 0.18));
        p.rect_filled(Rect::from_min_max(pos2(banner.left(), banner.bottom() - 3.0), banner.max), Rounding::ZERO, accent);
        draw_mode_art(p, banner.shrink(10.0), mode, accent);
        egui::Frame::none().inner_margin(Margin::same(16.0)).show(ui, |ui| {
            ui.set_width(width - 32.0);
            ui.spacing_mut().item_spacing.y = if tight { 5.0 } else { 8.0 };
            ui.label(egui::RichText::new(title).font(theme::bold(21.0)).color(TEXT));
            ui.add_space(2.0);
            ui.allocate_ui_with_layout(vec2(width - 32.0, 62.0), Layout::top_down(egui::Align::Min), |ui| {
                ui.set_min_size(vec2(width - 32.0, 62.0));
                ui.add(egui::Label::new(egui::RichText::new(desc).font(theme::font(13.5)).color(TEXT_DIM)).wrap());
            });
            ui.add_space(if tight { 2.0 } else { 6.0 });
            ui.horizontal(|ui| {
                let acc = stats.accuracy().map(|a| format!("{:.0}%", 100.0 * a)).unwrap_or_else(|| "–".into());
                ui.label(egui::RichText::new(format!("{} decisions", stats.decisions)).font(theme::medium(13.0)).color(TEXT_DIM));
                ui.label(egui::RichText::new("·").color(TEXT_FAINT));
                ui.label(egui::RichText::new(format!("{acc} accuracy")).font(theme::medium(13.0)).color(TEAL));
            });
            ui.add_space(if tight { 4.0 } else { 10.0 });
            if widgets::primary_button(ui, if resume { "Resume" } else { "Train" }, width - 32.0).clicked() {
                go = true;
            }
        });
        });
        })
    });
    let card = card.response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if card.clicked() {
        go = true;
    }
    if card.contains_pointer() {
        ui.painter().rect_stroke(card.rect, Rounding::same(14.0), Stroke::new(1.5_f32, theme::mix(BORDER, accent, 0.8)));
    }
    go
}

fn mistakes(ui: &mut egui::Ui, app: &mut App) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Recent mistakes").font(theme::semibold(20.0)).color(TEXT));
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            if !app.profile.mistakes.is_empty() && widgets::soft_button(ui, "See all in Stats", TEAL).clicked() {
                app.screen = Screen::Stats;
            }
        });
    });
    ui.add_space(6.0);
    if app.profile.mistakes.is_empty() {
        widgets::panel_frame(PANEL).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                egui::RichText::new("No mistakes yet. The spots you get wrong will show up here, with the solution, so you can review them.")
                    .font(theme::font(14.0))
                    .color(TEXT_DIM),
            );
        });
        return;
    }
    let list: Vec<_> = app.profile.mistakes.iter().take(5).cloned().collect();
    widgets::panel_frame(PANEL).show(ui, |ui| {
        ui.set_width(ui.available_width());
        for (k, m) in list.iter().enumerate() {
            if k > 0 {
                ui.add_space(2.0);
                let y = ui.cursor().top();
                ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0_f32, BORDER));
                ui.add_space(6.0);
            }
            mistake_row(ui, m);
        }
    });
}

pub fn mistake_row(ui: &mut egui::Ui, m: &holdem_trainer::profile::MistakeRecord) {
    ui.horizontal(|ui| {
        let color = theme::grade_color(m.grade);
        widgets::pill(ui, m.grade.label(), theme::mix(PANEL, color, 0.35), color);
        ui.label(egui::RichText::new(&m.hand).font(theme::bold(15.0)).color(TEXT));
        ui.label(egui::RichText::new(&m.spot).font(theme::font(13.5)).color(TEXT_DIM));
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(&m.date).font(theme::font(12.0)).color(TEXT_FAINT));
        });
    });
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(format!("You: {}", m.chosen)).font(theme::font(13.0)).color(theme::grade_color(m.grade)));
        ui.label(egui::RichText::new("→").color(TEXT_FAINT));
        ui.label(egui::RichText::new(format!("Solution: {}", m.solution)).font(theme::font(13.0)).color(TEAL));
    });
}
