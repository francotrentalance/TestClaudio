//! The settings screen.

use eframe::egui::{self, Margin};
use holdem_trainer::gen::{SRP, THREE_BET, spot_label};
use holdem_trainer::profile::{Profile, Settings};

use super::app::{App, snap_stack};
use super::home::centered;
use super::theme::{self, *};
use super::widgets;

const POSITIONS: [&str; 6] = ["UTG", "HJ", "CO", "BTN", "SB", "BB"];

fn row(ui: &mut egui::Ui, title: &str, help: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width(300.0);
            ui.label(egui::RichText::new(title).font(theme::semibold(15.0)).color(TEXT));
            if !help.is_empty() {
                ui.label(egui::RichText::new(help).font(theme::font(12.5)).color(TEXT_FAINT));
            }
        });
        ui.add_space(12.0);
        add(ui);
    });
    ui.add_space(10.0);
}

fn positions(ui: &mut egui::Ui, list: &mut Vec<String>) -> bool {
    let mut changed = false;
    for pos in POSITIONS {
        let mut on = list.iter().any(|p| p == pos);
        if ui.toggle_value(&mut on, pos).changed() {
            changed = true;
            if on {
                list.push(pos.to_string());
            } else if list.len() > 1 {
                list.retain(|p| p != pos);
            }
        }
    }
    // Keep a stable order.
    list.sort_by_key(|p| POSITIONS.iter().position(|q| q == p));
    changed
}

fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    widgets::panel_frame(PANEL).show(ui, |ui| {
        ui.set_width(ui.available_width());
        widgets::section_title(ui, title);
        ui.add_space(8.0);
        add(ui);
    });
    ui.add_space(14.0);
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let before = app.profile.settings.clone();
    let mut reset = false;
    egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(Margin::symmetric(24.0, 20.0))).show(ctx, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            centered(ui, 900.0, |ui| {
                ui.label(egui::RichText::new("Settings").font(theme::bold(26.0)).color(TEXT));
                ui.add_space(10.0);
                let stacks = app.content.pushfold_stacks();
                let s: &mut Settings = &mut app.profile.settings;

                card(ui, "Training", |ui| {
                    row(ui, "Daily goal", "Decisions per day. Meeting it keeps your day streak alive.", |ui| {
                        ui.add(egui::Slider::new(&mut s.daily_goal, 10..=300).step_by(10.0).suffix(" decisions"));
                    });
                    row(ui, "Difficulty", "Normal and Hard skip more of the obvious hands (like folding 72o).", |ui| {
                        for (k, label) in ["Easy", "Normal", "Hard"].iter().enumerate() {
                            ui.selectable_value(&mut s.difficulty, k as u8, *label);
                        }
                    });
                });

                card(ui, "Preflop Cash", |ui| {
                    row(ui, "Your seats", "The seats you are dealt into.", |ui| {
                        positions(ui, &mut s.cash_positions);
                    });
                });

                card(ui, "Push / Fold", |ui| {
                    row(ui, "Stack range", "Effective stacks in big blinds, before the ante.", |ui| {
                        if stacks.is_empty() {
                            return;
                        }
                        ui.spacing_mut().slider_width = 130.0;
                        // The sliders step through the shipped depths only, so every choice has a chart.
                        let index = |x: f64| stacks.iter().position(|&v| v == snap_stack(&stacks, x)).unwrap_or(0);
                        let last = stacks.len() - 1;
                        for (label, value) in [("from", &mut s.pushfold_min), ("to", &mut s.pushfold_max)] {
                            ui.label(label);
                            let mut i = index(*value);
                            ui.add(
                                egui::Slider::new(&mut i, 0..=last)
                                    .custom_formatter(|v, _| format!("{} bb", stacks[(v as usize).min(last)]))
                                    .custom_parser(|t| t.trim().trim_end_matches("bb").trim().parse::<f64>().ok().map(|x| index(x) as f64)),
                            );
                            *value = stacks[i.min(last)];
                        }
                    });
                    if s.pushfold_min > s.pushfold_max {
                        std::mem::swap(&mut s.pushfold_min, &mut s.pushfold_max);
                    }
                    row(ui, "Your seats", "", |ui| {
                        positions(ui, &mut s.pushfold_positions);
                    });
                });

                card(ui, "Postflop", |ui| {
                    row(ui, "Spots", "Which pots to play.", |ui| {
                        for kind in [SRP, THREE_BET] {
                            let mut on = s.postflop_spots.iter().any(|k| k == kind);
                            if ui.toggle_value(&mut on, spot_label(kind)).changed() {
                                if on {
                                    s.postflop_spots.push(kind.to_string());
                                } else if s.postflop_spots.len() > 1 {
                                    s.postflop_spots.retain(|k| k != kind);
                                }
                            }
                        }
                    });
                    row(ui, "Your seat", "Out of position is BB, in position is BTN.", |ui| {
                        for (value, label) in [("any", "Random"), ("oop", "Out of position"), ("ip", "In position")] {
                            if ui.selectable_label(s.postflop_role == value, label).clicked() {
                                s.postflop_role = value.to_string();
                            }
                        }
                    });
                });

                card(ui, "Display", |ui| {
                    row(ui, "Four-color deck", "Blue diamonds and green clubs.", |ui| {
                        ui.checkbox(&mut s.four_color_deck, "");
                    });
                    row(ui, "Animations", "", |ui| {
                        ui.checkbox(&mut s.animations, "");
                    });
                });

                card(ui, "Progress", |ui| {
                    ui.label(
                        egui::RichText::new(format!("Saved automatically in {}", Profile::path().display()))
                            .font(theme::font(12.5))
                            .color(TEXT_DIM),
                    );
                    ui.add_space(6.0);
                    if !app.confirm_reset {
                        if widgets::soft_button(ui, "Reset all progress…", theme::grade_color(holdem_trainer::grading::Grade::Blunder)).clicked() {
                            app.confirm_reset = true;
                        }
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new("Erase XP, stats and mistakes? Settings are kept.").color(TEXT));
                            if widgets::soft_button(ui, "Yes, reset", theme::grade_color(holdem_trainer::grading::Grade::Blunder)).clicked() {
                                reset = true;
                            }
                            if widgets::soft_button(ui, "Cancel", TEXT_FAINT).clicked() {
                                app.confirm_reset = false;
                            }
                        });
                    }
                    if let Some(e) = &app.save_error {
                        ui.label(egui::RichText::new(format!("Could not save: {e}")).color(theme::grade_color(holdem_trainer::grading::Grade::Blunder)));
                    }
                });

                card(ui, "About the solutions", |ui| {
                    let text = "Preflop Cash strategies come from a 6-max solve with Monte Carlo CFR in which every flop is \
                                valued with equity-realization factors measured from postflop solves. They approximate full-game \
                                play, so treat close preflop mixes as approximate. Push/fold charts are solved for each stack depth, \
                                with every player posting an ante of 12.5% of the big blind. Postflop spots are solved on each \
                                stored flop, and the turn and river are re-solved live for the board that comes. Everything is \
                                solved without rake (chip EV). Opponents always play the solution, sampled from its mixed strategy.";
                    ui.label(egui::RichText::new(text).font(theme::font(13.5)).color(TEXT_DIM));
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(format!("Hold'em Trainer {}", env!("CARGO_PKG_VERSION"))).font(theme::font(12.0)).color(TEXT_FAINT));
                });
            });
        });
    });
    if reset {
        let settings = app.profile.settings.clone();
        app.profile = Profile { settings, ..Profile::default() };
        app.confirm_reset = false;
        app.session = None;
    }
    if reset || settings_changed(&before, &app.profile.settings) {
        app.save();
    }
}

fn settings_changed(a: &Settings, b: &Settings) -> bool {
    serde_json::to_string(a).ok() != serde_json::to_string(b).ok()
}
