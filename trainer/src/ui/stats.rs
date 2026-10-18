//! The stats screen: totals, per-drill and per-spot accuracy, activity and mistakes.

use eframe::egui::{self, Align2, Layout, Margin, Rect, Rounding, Sense, Stroke, pos2, vec2};
use holdem_trainer::grading::{Grade, title_for_level};
use holdem_trainer::profile::{CategoryStats, Mode};

use super::app::App;
use super::home::{centered, mistake_row};
use super::theme::{self, *};
use super::widgets;

const GRADES: [Grade; 5] = [Grade::Perfect, Grade::Good, Grade::Inaccuracy, Grade::Mistake, Grade::Blunder];

fn grade_bar(ui: &mut egui::Ui, stats: &CategoryStats, width: f32, height: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let p = ui.painter();
    let r = Rounding::same(height / 2.0);
    p.rect_filled(rect, r, BG);
    let total = stats.decisions.max(1) as f32;
    let mut x = rect.left();
    let clip = p.with_clip_rect(rect);
    // Clip the segments to a rounded track by painting the track's corners last.
    for g in GRADES {
        let w = rect.width() * stats.grades[g.index()] as f32 / total;
        if w > 0.0 {
            clip.rect_filled(Rect::from_min_max(pos2(x, rect.top()), pos2(x + w, rect.bottom())), Rounding::ZERO, theme::grade_color(g));
        }
        x += w;
    }
    p.rect_stroke(rect, r, Stroke::new(2.0_f32, PANEL));
    let tip: Vec<String> = GRADES
        .iter()
        .map(|g| format!("{}: {}", g.label(), stats.grades[g.index()]))
        .collect();
    resp.on_hover_text(tip.join("\n"))
}

fn accuracy_text(s: &CategoryStats) -> String {
    s.accuracy().map(|a| format!("{:.0}%", 100.0 * a)).unwrap_or_else(|| "–".into())
}

pub fn show(app: &mut App, ctx: &egui::Context) {
    let filter_id = egui::Id::new("stats_filter");
    let mut filter: u8 = ctx.data(|d| d.get_temp(filter_id)).unwrap_or(0);
    egui::CentralPanel::default().frame(egui::Frame::none().fill(BG).inner_margin(Margin::symmetric(24.0, 20.0))).show(ctx, |ui| {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            centered(ui, 1120.0, |ui| {
                let p = &app.profile;
                ui.label(egui::RichText::new("Your stats").font(theme::bold(26.0)).color(TEXT));
                ui.add_space(8.0);

                let modes = [Mode::Cash, Mode::PushFold, Mode::Postflop].map(|m| (m, p.mode_stats(m)));
                let mut all = CategoryStats::default();
                for (_, s) in &modes {
                    all.decisions += s.decisions;
                    for g in 0..5 {
                        all.grades[g] += s.grades[g];
                    }
                }
                let w = ui.available_width();
                let gap = 14.0;
                let tile_w = (w - 4.0 * gap) / 5.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    let level = p.level();
                    widgets::stat_tile(ui, "Level", &level.to_string(), title_for_level(level), GOLD, tile_w);
                    widgets::stat_tile(ui, "Total XP", &p.xp.to_string(), "", GOLD, tile_w);
                    widgets::stat_tile(ui, "Decisions", &p.decisions.to_string(), &format!("{} hands", p.hands), TEXT, tile_w);
                    widgets::stat_tile(ui, "Accuracy", &accuracy_text(&all), "Perfect + Good", TEAL, tile_w);
                    widgets::stat_tile(ui, "Best streak", &p.best_streak.to_string(), &format!("now {}", p.streak), FLAME, tile_w);
                });
                ui.add_space(14.0);

                let left_w = (w - gap) * 0.42;
                let right_w = w - gap - left_w;
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.vertical(|ui| {
                        ui.set_width(left_w);
                        widgets::panel_frame(PANEL).show(ui, |ui| {
                            ui.set_width(left_w - 32.0);
                            widgets::section_title(ui, "By drill");
                            ui.add_space(4.0);
                            for (m, s) in &modes {
                                ui.horizontal(|ui| {
                                    ui.label(egui::RichText::new(m.label()).font(theme::semibold(14.5)).color(TEXT));
                                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                                        ui.label(egui::RichText::new(accuracy_text(s)).font(theme::bold(15.0)).color(TEAL));
                                        let extra = if *m == Mode::Postflop && s.decisions > 0 {
                                            format!("{} decisions · {:.2} bb lost avg", s.decisions, s.ev_loss / s.decisions as f64)
                                        } else {
                                            format!("{} decisions", s.decisions)
                                        };
                                        ui.label(egui::RichText::new(extra).font(theme::font(12.5)).color(TEXT_DIM));
                                    });
                                });
                                grade_bar(ui, s, left_w - 32.0, 12.0);
                                ui.add_space(8.0);
                            }
                            ui.horizontal_wrapped(|ui| {
                                for g in GRADES {
                                    let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                    ui.painter().circle_filled(r.center(), 5.0, theme::grade_color(g));
                                    ui.label(egui::RichText::new(g.label()).font(theme::font(12.0)).color(TEXT_DIM));
                                }
                            });
                        });
                        ui.add_space(gap);
                        widgets::panel_frame(PANEL).show(ui, |ui| {
                            ui.set_width(left_w - 32.0);
                            widgets::section_title(ui, "Last 14 days");
                            ui.add_space(6.0);
                            activity_chart(ui, app, left_w - 32.0);
                        });
                    });
                    ui.vertical(|ui| {
                        ui.set_width(right_w);
                        widgets::panel_frame(PANEL).show(ui, |ui| {
                            ui.set_width(right_w - 32.0);
                            categories(ui, app, right_w - 32.0);
                        });
                    });
                });

                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Mistakes to review").font(theme::semibold(20.0)).color(TEXT));
                    ui.add_space(12.0);
                    for (k, label) in ["All", "Cash", "Push / Fold", "Postflop"].iter().enumerate() {
                        if ui.selectable_label(filter == k as u8, *label).clicked() {
                            filter = k as u8;
                        }
                    }
                });
                ui.add_space(6.0);
                let want = match filter {
                    1 => Some(Mode::Cash),
                    2 => Some(Mode::PushFold),
                    3 => Some(Mode::Postflop),
                    _ => None,
                };
                let list: Vec<_> = app.profile.mistakes.iter().filter(|m| want.map_or(true, |w| m.mode == w)).take(60).cloned().collect();
                widgets::panel_frame(PANEL).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    if list.is_empty() {
                        ui.label(egui::RichText::new("Nothing here yet.").font(theme::font(14.0)).color(TEXT_DIM));
                    }
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
            });
        });
    });
    ctx.data_mut(|d| d.insert_temp(filter_id, filter));
}

fn activity_chart(ui: &mut egui::Ui, app: &App, width: f32) {
    let days: Vec<(String, u32)> = (0..14)
        .rev()
        .map(|k| {
            let d = chrono::Local::now().date_naive() - chrono::Duration::days(k);
            let key = d.format("%Y-%m-%d").to_string();
            (d.format("%a").to_string(), app.profile.daily.get(&key).copied().unwrap_or(0))
        })
        .collect();
    let goal = app.profile.settings.daily_goal.max(1);
    let max = days.iter().map(|d| d.1).max().unwrap_or(0).max(goal) as f32;
    let h = 130.0;
    let (rect, _) = ui.allocate_exact_size(vec2(width, h + 20.0), Sense::hover());
    let p = ui.painter();
    let slot = rect.width() / days.len() as f32;
    let chart_bottom = rect.top() + h;
    let goal_y = chart_bottom - h * goal as f32 / max;
    for (k, (label, n)) in days.iter().enumerate() {
        let x = rect.left() + slot * (k as f32 + 0.5);
        let bh = (h * *n as f32 / max).max(if *n > 0 { 3.0 } else { 0.0 });
        let bar = Rect::from_min_max(pos2(x - slot * 0.32, chart_bottom - bh), pos2(x + slot * 0.32, chart_bottom));
        let color = if *n >= goal { TEAL } else { theme::mix(TEAL, PANEL, 0.55) };
        p.rect_filled(Rect::from_min_max(pos2(bar.left(), rect.top()), bar.max), Rounding::same(4.0), BG);
        if bh > 0.0 {
            p.rect_filled(bar, Rounding::same(4.0), color);
        }
        let today = k == days.len() - 1;
        p.text(pos2(x, chart_bottom + 10.0), Align2::CENTER_CENTER, &label[..1], theme::font(11.0), if today { TEXT } else { TEXT_FAINT });
    }
    p.hline(rect.x_range(), goal_y, Stroke::new(1.0_f32, theme::mix(GOLD, PANEL, 0.3)));
    p.text(pos2(rect.left() + 2.0, goal_y - 8.0), Align2::LEFT_CENTER, format!("goal {goal}"), theme::font(11.0), GOLD);
}

fn categories(ui: &mut egui::Ui, app: &App, width: f32) {
    widgets::section_title(ui, "By spot");
    let mut cats: Vec<(&String, &CategoryStats)> = app.profile.categories.iter().collect();
    if cats.is_empty() {
        ui.label(egui::RichText::new("Play a few hands to see how you do in each spot.").font(theme::font(14.0)).color(TEXT_DIM));
        return;
    }
    // Weakest spots with enough data first, the rest by volume.
    let leak = |s: &CategoryStats| if s.decisions >= 10 { s.accuracy().unwrap_or(1.0) } else { 2.0 };
    cats.sort_by(|a, b| leak(a.1).partial_cmp(&leak(b.1)).unwrap_or(std::cmp::Ordering::Equal).then(b.1.decisions.cmp(&a.1.decisions)));
    ui.label(egui::RichText::new("Weakest spots first (with at least 10 decisions).").font(theme::font(12.0)).color(TEXT_FAINT));
    ui.add_space(4.0);
    let name_w = width * 0.44;
    let bar_w = width * 0.30;
    for (name, s) in cats.iter().take(16) {
        ui.horizontal(|ui| {
            ui.allocate_ui(vec2(name_w, 22.0), |ui| {
                ui.set_width(name_w);
                ui.label(egui::RichText::new(name.as_str()).font(theme::medium(13.5)).color(TEXT));
            });
            ui.vertical(|ui| {
                ui.add_space(5.0);
                grade_bar(ui, s, bar_w, 10.0);
            });
            ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                let acc = s.accuracy().unwrap_or(0.0);
                let col = if s.decisions < 10 {
                    TEXT_DIM
                } else if acc >= 0.8 {
                    theme::grade_color(Grade::Perfect)
                } else if acc >= 0.6 {
                    GOLD
                } else {
                    theme::grade_color(Grade::Mistake)
                };
                ui.label(egui::RichText::new(accuracy_text(s)).font(theme::bold(14.0)).color(col));
                ui.label(egui::RichText::new(format!("{}", s.decisions)).font(theme::font(12.5)).color(TEXT_FAINT));
            });
        });
    }
    if cats.len() > 16 {
        ui.label(egui::RichText::new(format!("…and {} more", cats.len() - 16)).font(theme::font(12.0)).color(TEXT_FAINT));
    }
}
