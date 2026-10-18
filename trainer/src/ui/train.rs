//! The training screen: the table, the action buttons and the solution panel.

use eframe::egui::{self, Align, Align2, Layout, Margin, Rect, Rounding, Sense, Stroke, pos2, vec2};
use holdem_solver::card::{Card, parse_card};
use holdem_trainer::grading::Grade;
use holdem_trainer::profile::Mode;
use holdem_trainer::quiz::{Feedback, LogEntry, MIN_REACH, SeatView, fmt_bb};

use super::app::{App, Hand, Screen, Session};
use super::table::{TableView, draw_table};
use super::theme::{self, *};
use super::widgets::{self, ActStyle, act_color};

const SIDE_WIDTH: f32 = 392.0;

/// Colors for a list of action labels ("Fold", "Call 2", "Raise to 7.5", "All-in 100", ...).
pub fn styles_for(labels: &[String]) -> Vec<ActStyle> {
    let mut sizes = 0;
    labels
        .iter()
        .map(|l| {
            if l.starts_with("Fold") {
                ActStyle::Fold
            } else if l.starts_with("Check") || l.starts_with("Call") {
                ActStyle::Passive
            } else if l.starts_with("All-in") {
                ActStyle::AllIn
            } else {
                sizes += 1;
                ActStyle::Aggressive(sizes - 1)
            }
        })
        .collect()
}

/// The hero's current choices, if it is the hero's turn and no feedback is open.
pub fn current_labels(s: &mut Session) -> Option<Vec<String>> {
    if s.feedback.is_some() {
        return None;
    }
    match &mut s.hand {
        Hand::Pre(h) => h.decision().map(|d| d.labels),
        Hand::Post(h) => h.decision().map(|d| d.labels),
    }
}

pub fn current_styles(s: &mut Session) -> Option<Vec<ActStyle>> {
    current_labels(s).map(|l| styles_for(&l))
}

fn shortcut(i: usize, style: ActStyle) -> String {
    let letter = match style {
        ActStyle::Fold => "F",
        ActStyle::Passive => "C",
        ActStyle::Aggressive(0) => "R",
        ActStyle::AllIn => "A",
        ActStyle::Aggressive(_) => "",
    };
    if letter.is_empty() { format!("{}", i + 1) } else { format!("{}  ·  {letter}", i + 1) }
}

/// Everything the screen shows about the current hand, copied out of the session.
struct View {
    seats: Vec<SeatView>,
    board: Vec<Card>,
    pot: f32,
    title: String,
    spot: String,
    story: Option<String>,
    hero_cards: [Card; 2],
    hero_hand: String,
    end: Option<String>,
    solving: bool,
    over: bool,
    log: Vec<LogEntry>,
}

fn view_of(s: &Session) -> View {
    // While the feedback is open the header keeps describing the spot that was
    // answered, not the actions taken after it.
    let answered = s.feedback.as_ref().map(|a| a.fb.spot.clone());
    match &s.hand {
        Hand::Pre(h) => {
            let cfg = &h.pack.pack.config;
            let stack = fmt_bb(h.pack.nominal_stack() as f32);
            let title = match s.mode {
                Mode::PushFold => format!("Push / Fold  ·  {stack}bb  ·  ante {}bb", cfg.ante),
                _ => format!("Preflop Cash  ·  {stack}bb deep"),
            };
            let (a, b) = h.cards[h.hero];
            View {
                seats: h.seats(),
                board: h.board(),
                pot: h.pot(),
                title,
                spot: answered.unwrap_or_else(|| h.spot_text()),
                story: None,
                hero_cards: [a, b],
                hero_hand: h.hero_hand_name(),
                end: h.end_text(),
                solving: false,
                over: h.end.is_some(),
                log: h.log.clone(),
            }
        }
        Hand::Post(h) => {
            let kind = &h.spot().kind;
            let cards = h.display_cards(h.hero);
            View {
                seats: h.seats().to_vec(),
                board: h.display_board(),
                pot: h.pot(),
                title: format!("Postflop  ·  {}", holdem_trainer::gen::spot_label(kind)),
                spot: answered.unwrap_or_else(|| h.spot_text()),
                story: Some(holdem_trainer::gen::spot_story(kind).to_string()),
                hero_cards: cards,
                hero_hand: format!("{} {}", holdem_solver::card::card_to_string(cards[0]), holdem_solver::card::card_to_string(cards[1])),
                end: h.end_text(),
                solving: h.is_solving(),
                over: h.is_over(),
                log: h.log.clone(),
            }
        }
    }
}

pub fn show(app: &mut App, ctx: &egui::Context, now: f64) {
    if app.session.is_none() {
        app.screen = Screen::Home;
        super::home::show(app, ctx);
        return;
    }
    let four = app.profile.settings.four_color_deck;
    let animations = app.profile.settings.animations;
    let s = app.session.as_mut().unwrap();
    let labels = current_labels(s);
    let view = view_of(s);
    if view.board.len() != s.board_len {
        s.board_len = view.board.len();
        s.changed_at = now;
    }
    let reveal = if animations { ((now - s.changed_at) / 0.35).clamp(0.0, 1.0) as f32 } else { 1.0 };
    if reveal < 1.0 {
        ctx.request_repaint();
    }

    let mut action: Option<usize> = None;
    let mut cont = false;
    let mut end_session = false;

    egui::SidePanel::right("train_side")
        .exact_width(SIDE_WIDTH)
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::none().fill(BG).inner_margin(Margin { left: 4.0, right: 18.0, top: 16.0, bottom: 16.0 }))
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                let s = app.session.as_ref().unwrap();
                session_card(ui, s);
                ui.add_space(10.0);
                if let Some(ans) = &s.feedback {
                    feedback_card(ui, ans, &view, now, animations);
                } else {
                    spot_card(ui, &view, four);
                }
                ui.add_space(10.0);
                log_card(ui, &view, four);
            });
        });

    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(BG).inner_margin(Margin { left: 18.0, right: 14.0, top: 14.0, bottom: 16.0 }))
        .show(ctx, |ui| {
            let s = app.session.as_ref().unwrap();
            // Header: what is being trained and a way out.
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(egui::RichText::new(&view.title).font(theme::semibold(18.0)).color(TEXT));
                    ui.label(egui::RichText::new(&view.spot).font(theme::font(13.5)).color(TEXT_DIM));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::soft_button(ui, "End session", TEXT_FAINT).clicked() {
                        end_session = true;
                    }
                });
            });
            if let Some(err) = &s.error {
                ui.label(egui::RichText::new(err).color(theme::grade_color(Grade::Blunder)));
            }

            let full = ui.available_rect_before_wrap();
            let actions_h = 96.0;
            let table_rect = Rect::from_min_max(full.min, pos2(full.max.x, full.max.y - actions_h));
            let banner = view.solving.then(|| "Dealing the next card…  solving the subgame".to_string());
            draw_table(ui, table_rect, &TableView { seats: &view.seats, board: &view.board, pot: view.pot, four_color: four, banner, reveal });
            ui.allocate_rect(table_rect, Sense::hover());

            let bar = Rect::from_min_max(pos2(full.min.x, full.max.y - actions_h + 14.0), full.max);
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(bar), |ui| {
                action_bar(ui, s, labels.as_deref(), &view, &mut action, &mut cont);
            });
        });

    if view.solving {
        ctx.request_repaint_after(std::time::Duration::from_millis(60));
    }
    if end_session {
        app.session = None;
        app.screen = Screen::Home;
        return;
    }
    if let Some(i) = action {
        app.answer(i, now);
    } else if cont {
        app.continue_hand();
    }
}

fn centered_row(ui: &mut egui::Ui, width: f32, add: impl FnOnce(&mut egui::Ui)) {
    let avail = ui.available_width();
    ui.horizontal(|ui| {
        ui.add_space(((avail - width) / 2.0).max(0.0));
        add(ui);
    });
}

fn action_bar(ui: &mut egui::Ui, s: &Session, labels: Option<&[String]>, view: &View, action: &mut Option<usize>, cont: &mut bool) {
    let gap = 12.0;
    if s.feedback.is_some() {
        let label = if view.over { "Next hand" } else { "Continue" };
        centered_row(ui, 300.0, |ui| {
            if widgets::action_button(ui, label, "Space", GOLD, 300.0, true).clicked() {
                *cont = true;
            }
        });
        return;
    }
    if let Some(labels) = labels {
        let styles = styles_for(labels);
        let n = labels.len() as f32;
        let w = ((ui.available_width() - gap * (n - 1.0)) / n).min(190.0);
        let total = w * n + gap * (n - 1.0);
        centered_row(ui, total, |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, label) in labels.iter().enumerate() {
                if widgets::action_button(ui, label, &shortcut(i, styles[i]), act_color(styles[i]), w, true).clicked() {
                    *action = Some(i);
                }
            }
        });
        return;
    }
    if view.solving {
        centered_row(ui, 330.0, |ui| {
            ui.add(egui::Spinner::new().size(22.0).color(TEAL));
            ui.label(egui::RichText::new("Solving the next street…").font(theme::medium(16.0)).color(TEXT_DIM));
        });
        return;
    }
    if view.over {
        centered_row(ui, 300.0, |ui| {
            if widgets::action_button(ui, "Next hand", "Space", GOLD, 300.0, true).clicked() {
                *cont = true;
            }
        });
    }
}

fn session_card(ui: &mut egui::Ui, s: &Session) {
    widgets::panel_frame(PANEL).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("This session").font(theme::semibold(14.0)).color(TEXT_DIM));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                widgets::pill(ui, s.mode.label(), theme::mix(PANEL_2, TEAL, 0.3), TEXT);
            });
        });
        ui.add_space(4.0);
        let acc = if s.decisions > 0 { format!("{:.0}%", 100.0 * s.correct as f32 / s.decisions as f32) } else { "–".into() };
        let mut items = vec![
            ("Hands", s.hands.to_string(), TEXT),
            ("Decisions", s.decisions.to_string(), TEXT),
            ("Accuracy", acc, TEAL),
            ("XP", format!("+{}", s.xp), GOLD),
        ];
        if s.mode == Mode::Postflop {
            // The unit goes in the caption: "12.34 bb" would not fit the column.
            items[1] = ("EV lost (bb)", fmt_bb(s.ev_lost), FLAME);
        }
        ui.columns(items.len(), |cols| {
            for (col, (title, value, color)) in cols.iter_mut().zip(items) {
                col.add(egui::Label::new(egui::RichText::new(value).font(theme::bold(20.0)).color(color)).extend());
                col.label(egui::RichText::new(title).font(theme::font(12.0)).color(TEXT_FAINT));
            }
        });
    });
}

fn mini_cards(ui: &mut egui::Ui, cards: &[Card], h: f32, four: bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for &c in cards {
            widgets::card(ui, Some(c), h, four);
        }
    });
}

fn spot_card(ui: &mut egui::Ui, view: &View, four: bool) {
    widgets::panel_frame(PANEL).show(ui, |ui| {
        ui.set_width(ui.available_width());
        widgets::section_title(ui, "Your spot");
        ui.add_space(2.0);
        if let Some(story) = &view.story {
            ui.label(egui::RichText::new(story).font(theme::font(13.5)).color(TEXT_DIM));
        }
        ui.label(egui::RichText::new(&view.spot).font(theme::font(13.5)).color(TEXT_DIM));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            mini_cards(ui, &view.hero_cards, 58.0, four);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(&view.hero_hand).font(theme::bold(22.0)).color(TEXT));
                ui.label(egui::RichText::new("your hand").font(theme::font(12.0)).color(TEXT_FAINT));
            });
        });
        ui.add_space(8.0);
        let hint = if view.solving {
            "The solver is working out the next street for this exact board."
        } else if view.over {
            "Hand over."
        } else {
            "Pick the play you think the solution makes. Keys: 1-6, F, C, R, A."
        };
        ui.label(egui::RichText::new(hint).font(theme::font(12.5)).color(TEXT_FAINT));
        if let Some(end) = &view.end {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(end).font(theme::medium(14.0)).color(TEXT));
        }
    });
}

fn verdict_text(fb: &Feedback) -> String {
    // Off the solution's line the frequencies mean nothing: speak in EV only.
    if let (false, Some(evs)) = (fb.freqs_trusted, &fb.evs) {
        let best = (0..evs.len()).max_by(|&a, &b| evs[a].partial_cmp(&evs[b]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(0);
        return match fb.grade {
            Grade::Perfect => "Spot on: your play keeps all the EV.".into(),
            Grade::Good => "Close enough: you give up almost nothing.".into(),
            _ => format!("{} keeps the most EV here.", fb.actions[best]),
        };
    }
    let best = fb
        .freqs
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let f = fb.freqs[fb.chosen];
    let best_label = &fb.actions[best];
    match fb.grade {
        Grade::Perfect if fb.evs.is_some() => "Spot on: your play keeps all the EV.".into(),
        Grade::Perfect => format!("Spot on: the solution plays it {:.0}% of the time.", 100.0 * f),
        Grade::Good if fb.evs.is_some() => "Close enough: you give up almost nothing.".into(),
        Grade::Good => format!("A fine mix: the solution plays it {:.0}% of the time.", 100.0 * f),
        _ if f < 0.005 => format!("The solution never plays this here. It prefers {best_label}."),
        _ => format!("The solution only plays this {:.0}% of the time and prefers {best_label}.", 100.0 * f),
    }
}

fn feedback_card(ui: &mut egui::Ui, ans: &super::app::Answered, view: &View, now: f64, animations: bool) {
    let fb = &ans.fb;
    let color = theme::grade_color(fb.grade);
    let age = if animations { (now - ans.at) as f32 } else { 10.0 };
    if age < 1.0 {
        ui.ctx().request_repaint();
    }
    let pop = 1.0 + 0.25 * (1.0 - (age / 0.25).min(1.0));
    widgets::panel_frame(theme::mix(PANEL, color, 0.08)).stroke(Stroke::new(1.5_f32, theme::mix(BORDER, color, 0.6))).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(52.0, 52.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 24.0 * pop.min(1.15), theme::mix(PANEL, color, 0.25));
            ui.painter().circle_stroke(r.center(), 24.0 * pop.min(1.15), Stroke::new(2.0_f32, color));
            ui.painter().text(r.center(), Align2::CENTER_CENTER, theme::grade_icon(fb.grade), theme::bold(19.0 * pop), color);
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(fb.grade.label()).font(theme::bold(24.0 * pop.min(1.1))).color(color));
                ui.horizontal_wrapped(|ui| {
                    let shown = (ans.award.xp as f32 * (age / 0.6).min(1.0)).round() as u32;
                    ui.label(egui::RichText::new(format!("+{shown} XP")).font(theme::semibold(15.0)).color(GOLD));
                    // One extra XP per 5 in a row (grading::xp_for).
                    let bonus = ans.award.xp.saturating_sub(fb.grade.xp());
                    if bonus > 0 {
                        ui.label(egui::RichText::new(format!("incl. +{bonus} streak bonus")).font(theme::font(12.0)).color(TEXT_DIM));
                    }
                    let streak = ans.award.streak;
                    if fb.grade.is_correct() && streak > 1 {
                        ui.label(egui::RichText::new(format!("🔥 {streak} in a row")).font(theme::semibold(14.0)).color(FLAME));
                    } else if fb.grade.breaks_streak() && ans.award.streak_before > 0 {
                        ui.label(egui::RichText::new(format!("streak of {} lost", ans.award.streak_before)).font(theme::font(13.0)).color(FLAME));
                    }
                });
            });
        });
        ui.add_space(4.0);
        ui.label(egui::RichText::new(&fb.spot).font(theme::font(12.5)).color(TEXT_DIM));
        ui.label(egui::RichText::new(verdict_text(fb)).font(theme::font(13.5)).color(TEXT));
        if fb.evs.is_some() && fb.ev_loss > 0.005 {
            let pct = if fb.pot > 0.0 { 100.0 * fb.ev_loss / fb.pot } else { 0.0 };
            ui.label(
                egui::RichText::new(format!("EV lost: {} bb ({:.1}% of the pot)", fmt_bb(fb.ev_loss), pct))
                    .font(theme::medium(13.5))
                    .color(theme::mix(color, TEXT, 0.3)),
            );
        }
        // The hand's result, before the long range grid so it stays in view.
        if let Some(end) = &view.end {
            ui.add_space(4.0);
            ui.label(egui::RichText::new(end).font(theme::semibold(14.0)).color(GOLD));
        }
        ui.add_space(8.0);
        ui.label(egui::RichText::new(format!("Solution with {}", fb.hand)).font(theme::semibold(14.5)).color(TEXT));
        if !fb.freqs_trusted {
            ui.label(egui::RichText::new("Your hand is not in the solution's range here: graded on EV only.").font(theme::font(12.5)).color(GOLD));
        }
        ui.add_space(2.0);
        let styles = styles_for(&fb.actions);
        let w = ui.available_width();
        widgets::freq_bars(ui, &fb.actions, &fb.freqs, &styles, Some(fb.chosen), fb.evs.as_deref(), w);
        if fb.evs.is_some() {
            ui.label(egui::RichText::new("EV in big blinds, from this point of the hand.").font(theme::font(11.5)).color(TEXT_FAINT));
        }

        if let Some(grid) = &fb.grid {
            ui.add_space(10.0);
            ui.label(egui::RichText::new(format!("Whole range: {}", fb.category)).font(theme::semibold(14.5)).color(TEXT));
            ui.add_space(2.0);
            let cell = ((ui.available_width() - 4.0) / 13.0).floor().min(27.0);
            let resp = widgets::range_grid(ui, grid, &styles, cell);
            if let Some(pos) = resp.hover_pos() {
                let rel = pos - resp.rect.min;
                let (col, row) = ((rel.x / cell) as usize, (rel.y / cell) as usize);
                if col < 13 && row < 13 {
                    let c = row * 13 + col;
                    let name = holdem_solver::range::class_name(c);
                    let reach = grid.reach[c];
                    let tip = if reach < MIN_REACH {
                        format!("{name}\nNot in range: the solution never plays it to this spot.")
                    } else {
                        let mix: Vec<String> = fb
                            .actions
                            .iter()
                            .zip(&grid.strategy[c])
                            .filter(|(_, &f)| f >= 0.005)
                            .map(|(a, f)| format!("{a}: {:.0}%", 100.0 * f))
                            .collect();
                        let gets_here = if reach < 0.995 { format!("\nGets here {:.0}% of the time", 100.0 * reach) } else { String::new() };
                        format!("{name}{gets_here}\n{}", mix.join("\n"))
                    };
                    resp.on_hover_text(tip);
                }
            }
            ui.horizontal_wrapped(|ui| {
                for (label, st) in fb.actions.iter().zip(&styles) {
                    let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                    ui.painter().rect_filled(r, Rounding::same(3.0), act_color(*st));
                    ui.label(egui::RichText::new(label).font(theme::font(12.0)).color(TEXT_DIM));
                }
                if grid.reach.iter().any(|&r| r < MIN_REACH) {
                    let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                    ui.painter().rect_filled(r, Rounding::same(3.0), OUT_OF_RANGE);
                    ui.label(egui::RichText::new("Not in range").font(theme::font(12.0)).color(TEXT_DIM));
                }
            });
            if grid.reach.iter().any(|&r| r < 0.995) {
                ui.label(
                    egui::RichText::new("Colored height: how often the solution plays each hand to this spot.").font(theme::font(11.5)).color(TEXT_FAINT),
                );
            }
        }
    });
}

fn log_card(ui: &mut egui::Ui, view: &View, four: bool) {
    if view.log.is_empty() {
        return;
    }
    widgets::panel_frame(PANEL).show(ui, |ui| {
        ui.set_width(ui.available_width());
        // One-line rows: the theme's 30px interact height would make each 38px tall.
        ui.spacing_mut().interact_size.y = 20.0;
        ui.spacing_mut().item_spacing.y = 3.0;
        widgets::section_title(ui, "Hand history");
        ui.add_space(2.0);
        let mut street = String::new();
        for e in &view.log {
            if e.street != street {
                street = e.street.clone();
                ui.add_space(2.0);
                ui.label(egui::RichText::new(street.to_uppercase()).font(theme::semibold(11.5)).color(TEXT_FAINT));
            }
            ui.horizontal(|ui| {
                if e.who == "Dealer" {
                    ui.label(egui::RichText::new("Dealt").font(theme::font(13.0)).color(TEXT_DIM));
                    if let Ok(c) = parse_card(&e.what) {
                        mini_cards(ui, &[c], 24.0, four);
                    }
                    return;
                }
                let who_col = if e.hero { GOLD } else { TEXT_DIM };
                let who = if e.hero { format!("{} (you)", e.who) } else { e.who.clone() };
                ui.label(egui::RichText::new(who).font(theme::semibold(13.0)).color(who_col));
                let col = match styles_for(std::slice::from_ref(&e.what))[0] {
                    ActStyle::Fold => TEXT_FAINT,
                    st => theme::mix(act_color(st), TEXT, 0.35),
                };
                ui.label(egui::RichText::new(&e.what).font(theme::font(13.0)).color(col));
            });
        }
    });
}
