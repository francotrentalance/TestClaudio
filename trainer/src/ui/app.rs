//! Application state, navigation and the training session flow.

use std::sync::Arc;

use eframe::egui::{self, Align2, Color32, Key, Rect, Rounding, Sense, Stroke, vec2};
use holdem_solver::card::Rng;
use holdem_trainer::grading::{Grade, title_for_level, xp_for_level};
use holdem_trainer::model::Content;
use holdem_trainer::postflop_quiz::PostflopHand;
use holdem_trainer::preflop_quiz::PreflopHand;
use holdem_trainer::profile::{Award, MistakeRecord, Mode, Profile, today};
use holdem_trainer::quiz::Feedback;

use super::theme::{self, *};
use super::widgets::{self, draw_chip, draw_progress};
use super::{home, settings, stats, train};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Screen {
    Home,
    Train,
    Stats,
    Settings,
}

pub enum Hand {
    Pre(PreflopHand),
    Post(PostflopHand),
}

pub struct Answered {
    pub fb: Feedback,
    pub award: Award,
    pub at: f64,
}

pub struct Session {
    pub mode: Mode,
    pub hand: Hand,
    pub feedback: Option<Answered>,
    pub decisions: u32,
    pub correct: u32,
    pub xp: u32,
    pub ev_lost: f32,
    pub hands: u32,
    /// Time the current board/hand was last changed (for reveal animations).
    pub changed_at: f64,
    pub board_len: usize,
    pub error: Option<String>,
}

/// Headless screenshot mode used to review the UI.
pub struct Shot {
    pub path: String,
    pub screen: String,
    pub requested: bool,
    pub ready_frames: u32,
}

pub struct App {
    pub content: Content,
    pub profile: Profile,
    pub screen: Screen,
    pub rng: Rng,
    pub session: Option<Session>,
    pub level_up: Option<(u32, f64)>,
    pub goal_toast: Option<f64>,
    pub confirm_reset: bool,
    pub save_error: Option<String>,
    pub shot: Option<Shot>,
    pub frames: u64,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, content: Content) -> App {
        theme::install(&cc.egui_ctx);
        let seed = std::env::var("HOLDEM_TRAINER_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7));
        let shot = std::env::var("HOLDEM_TRAINER_SHOT").ok().map(|path| Shot {
            path,
            screen: std::env::var("HOLDEM_TRAINER_SCREEN").unwrap_or_else(|_| "home".into()),
            requested: false,
            ready_frames: 0,
        });
        let mut app = App {
            content,
            profile: Profile::load(),
            screen: Screen::Home,
            rng: Rng::new(seed),
            session: None,
            level_up: None,
            goal_toast: None,
            confirm_reset: false,
            save_error: None,
            shot,
            frames: 0,
        };
        if let Some(screen) = app.shot.as_ref().map(|s| s.screen.clone()) {
            app.setup_shot(&screen);
        }
        app
    }

    fn setup_shot(&mut self, screen: &str) {
        if std::env::var_os("HOLDEM_TRAINER_DEMO_PROFILE").is_some() {
            self.profile = demo_profile(&mut self.rng);
        }
        match screen {
            "stats" => self.screen = Screen::Stats,
            "settings" => self.screen = Screen::Settings,
            s if s.starts_with("cash") => self.start(Mode::Cash),
            s if s.starts_with("push") => self.start(Mode::PushFold),
            s if s.starts_with("post") => self.start(Mode::Postflop),
            _ => self.screen = Screen::Home,
        }
    }

    pub fn start(&mut self, mode: Mode) {
        self.screen = Screen::Train;
        let hand = self.deal(mode);
        let error = hand.is_none().then(|| "No training content for these settings.".to_string());
        let hand = hand.unwrap_or_else(|| Hand::Pre(PreflopHand::deal(self.content.cash.clone(), &[], 0, &mut self.rng)));
        self.session = Some(Session {
            mode,
            hand,
            feedback: None,
            decisions: 0,
            correct: 0,
            xp: 0,
            ev_lost: 0.0,
            hands: 1,
            changed_at: 0.0,
            board_len: 0,
            error,
        });
    }

    fn seats_from_names(names: &[&str], wanted: &[String]) -> Vec<usize> {
        names.iter().enumerate().filter(|(_, n)| wanted.iter().any(|w| w == *n)).map(|(i, _)| i).collect()
    }

    fn deal(&mut self, mode: Mode) -> Option<Hand> {
        let s = self.profile.settings.clone();
        match mode {
            Mode::Cash => {
                let pack = self.content.cash.clone();
                let seats = Self::seats_from_names(&pack.pack.config.position_names(), &s.cash_positions);
                Some(Hand::Pre(PreflopHand::deal(pack, &seats, s.difficulty, &mut self.rng)))
            }
            Mode::PushFold => {
                let packs: Vec<_> = self
                    .content
                    .pushfold
                    .iter()
                    .filter(|p| p.pack.config.stack >= s.pushfold_min - 1e-9 && p.pack.config.stack <= s.pushfold_max + 1e-9)
                    .cloned()
                    .collect();
                let packs = if packs.is_empty() { self.content.pushfold.clone() } else { packs };
                let pack = packs.get(self.rng.below(packs.len().max(1) as u32) as usize)?.clone();
                let seats = Self::seats_from_names(&pack.pack.config.position_names(), &s.pushfold_positions);
                Some(Hand::Pre(PreflopHand::deal(pack, &seats, s.difficulty, &mut self.rng)))
            }
            Mode::Postflop => PostflopHand::deal(
                Arc::clone(&self.content.flops),
                &s.postflop_spots,
                &s.postflop_role,
                s.difficulty,
                &mut self.rng,
            )
            .map(Hand::Post),
        }
    }

    pub fn next_hand(&mut self) {
        let Some(mode) = self.session.as_ref().map(|s| s.mode) else { return };
        if let Some(hand) = self.deal(mode) {
            if let Some(s) = &mut self.session {
                s.hand = hand;
                s.feedback = None;
                s.hands += 1;
                s.board_len = 0;
            }
        }
    }

    /// Answers the current decision with action `i`.
    pub fn answer(&mut self, i: usize, now: f64) {
        let Some(s) = &mut self.session else { return };
        if s.feedback.is_some() {
            return;
        }
        let fb = match &mut s.hand {
            Hand::Pre(h) => {
                if h.decision().is_none() {
                    return;
                }
                h.answer(i, &mut self.rng)
            }
            Hand::Post(h) => {
                if h.decision().is_none() {
                    return;
                }
                h.answer(i, &mut self.rng)
            }
        };
        let award = self.profile.record(s.mode, &fb.category, fb.grade, fb.ev_loss as f64);
        if matches!(fb.grade, Grade::Mistake | Grade::Blunder) {
            let solution = fb
                .actions
                .iter()
                .zip(&fb.freqs)
                .filter(|(_, &f)| f >= 0.05)
                .map(|(a, f)| format!("{a} {:.0}%", 100.0 * f))
                .collect::<Vec<_>>()
                .join(", ");
            self.profile.add_mistake(MistakeRecord {
                date: today(),
                mode: s.mode,
                category: fb.category.clone(),
                spot: fb.spot.clone(),
                hand: fb.hand.clone(),
                chosen: fb.actions[fb.chosen].clone(),
                solution,
                grade: fb.grade,
            });
        }
        s.decisions += 1;
        if fb.grade.is_correct() {
            s.correct += 1;
        }
        s.xp += award.xp;
        s.ev_lost += fb.ev_loss;
        if award.level_after > award.level_before {
            self.level_up = Some((award.level_after, now));
        }
        if award.goal_reached_now {
            self.goal_toast = Some(now);
        }
        s.feedback = Some(Answered { fb, award, at: now });
        self.save_error = self.profile.save().err().map(|e| e.to_string());
    }

    /// Closes the feedback; deals a new hand if the current one is over.
    pub fn continue_hand(&mut self) {
        let over = match &mut self.session {
            Some(s) => {
                s.feedback = None;
                match &s.hand {
                    Hand::Pre(h) => h.end.is_some(),
                    Hand::Post(h) => h.is_over(),
                }
            }
            None => return,
        };
        if over {
            self.profile.hands += 1;
            self.next_hand();
        }
    }

    fn top_bar(&mut self, ctx: &egui::Context) {
        let frame = egui::Frame::none().fill(PANEL).inner_margin(egui::Margin::symmetric(18.0, 0.0));
        egui::TopBottomPanel::top("top").exact_height(64.0).show_separator_line(false).frame(frame).show(ctx, |ui| {
            let bar = ui.max_rect();
            ui.horizontal_centered(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
                draw_chip(ui.painter(), r.center(), 16.0, Color32::from_rgb(214, 52, 60));
                ui.label(egui::RichText::new("Hold'em Trainer").font(theme::bold(20.0)).color(TEXT));
                ui.add_space(24.0);
                for (screen, label) in [(Screen::Home, "Home"), (Screen::Train, "Train"), (Screen::Stats, "Stats"), (Screen::Settings, "Settings")] {
                    if screen == Screen::Train && self.session.is_none() {
                        continue;
                    }
                    let active = self.screen == screen;
                    let text = egui::RichText::new(label).font(theme::medium(15.5)).color(if active { TEXT } else { TEXT_DIM });
                    let resp = ui.add(egui::Label::new(text).selectable(false).sense(Sense::click()));
                    if active {
                        let r = resp.rect;
                        ui.painter().rect_filled(Rect::from_min_size(egui::pos2(r.left(), r.bottom() + 4.0), vec2(r.width(), 3.0)), Rounding::same(2.0), GOLD);
                    }
                    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        self.screen = screen;
                    }
                    ui.add_space(8.0);
                }
            });
            self.header_stats(ui, bar);
        });
    }

    /// Level, XP, streak and the daily goal ring, painted at the right of the top bar.
    fn header_stats(&self, ui: &mut egui::Ui, bar: Rect) {
        let p = &self.profile;
        let painter = ui.painter();
        let cy = bar.center().y;
        let level = p.level();
        let lo = xp_for_level(level);
        let hi = xp_for_level(level + 1);
        let frac = (p.xp - lo) as f32 / (hi - lo).max(1) as f32;

        // Daily goal ring.
        let goal = p.settings.daily_goal.max(1);
        let today = p.today_count();
        let ring_c = egui::pos2(bar.right() - 20.0, cy);
        let done = today >= goal;
        widgets::ring(painter, ring_c, 16.0, 4.0, today as f32 / goal as f32, if done { theme::grade_color(Grade::Perfect) } else { TEAL });
        painter.text(ring_c, Align2::CENTER_CENTER, format!("{}", today.min(999)), theme::bold(11.0), TEXT);
        let ring_rect = Rect::from_center_size(ring_c, vec2(40.0, 40.0));
        ui.interact(ring_rect, egui::Id::new("goal_ring"), Sense::hover())
            .on_hover_text(format!("Daily goal: {today} of {goal} decisions today.\nDay streak: {} days", p.day_streak()));

        // Streak.
        let flame = if p.streak > 0 { FLAME } else { TEXT_FAINT };
        let galley = painter.layout_no_wrap(format!("🔥 {}", p.streak), theme::bold(17.0), flame);
        let streak_rect = Rect::from_min_size(egui::pos2(ring_rect.left() - 16.0 - galley.size().x, cy - galley.size().y / 2.0), galley.size());
        painter.galley(streak_rect.min, galley, flame);
        ui.interact(streak_rect, egui::Id::new("streak"), Sense::hover())
            .on_hover_text(format!("Correct answers in a row. Best: {}", p.best_streak));

        // Level and XP bar.
        let w = 190.0;
        let right = streak_rect.left() - 24.0;
        let left = right - w;
        painter.text(egui::pos2(left, cy - 13.0), Align2::LEFT_CENTER, format!("Level {level}  ·  {}", title_for_level(level)), theme::semibold(13.5), GOLD);
        draw_progress(painter, Rect::from_min_size(egui::pos2(left, cy + 1.0), vec2(w, 8.0)), frac, GOLD);
        painter.text(egui::pos2(left, cy + 19.0), Align2::LEFT_CENTER, format!("{} / {hi} XP", p.xp), theme::font(11.0), TEXT_DIM);
    }

    fn overlays(&mut self, ctx: &egui::Context, now: f64) {
        if let Some((level, at)) = self.level_up {
            let age = now - at;
            if age > 3.2 {
                self.level_up = None;
            } else {
                let a = (1.0 - ((age - 2.4).max(0.0) / 0.8)) as f32;
                let scale = (0.85 + 0.15 * (age as f32 * 6.0).min(1.0)).min(1.0);
                egui::Area::new(egui::Id::new("levelup")).anchor(Align2::CENTER_CENTER, vec2(0.0, -40.0)).interactable(false).show(ctx, |ui| {
                    let size = vec2(380.0, 170.0) * scale;
                    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                    let p = ui.painter();
                    p.rect_filled(r, Rounding::same(22.0), PANEL.gamma_multiply(0.97 * a));
                    p.rect_stroke(r, Rounding::same(22.0), Stroke::new(2.0_f32, GOLD.gamma_multiply(a)));
                    p.text(r.center() - vec2(0.0, 38.0), Align2::CENTER_CENTER, "LEVEL UP!", theme::bold(30.0), GOLD.gamma_multiply(a));
                    p.text(r.center() + vec2(0.0, 6.0), Align2::CENTER_CENTER, format!("Level {level}"), theme::bold(38.0), TEXT.gamma_multiply(a));
                    p.text(r.center() + vec2(0.0, 48.0), Align2::CENTER_CENTER, title_for_level(level), theme::semibold(17.0), TEAL.gamma_multiply(a));
                });
                ctx.request_repaint();
            }
        }
        if let Some(at) = self.goal_toast {
            let age = now - at;
            if age > 3.0 {
                self.goal_toast = None;
            } else {
                egui::Area::new(egui::Id::new("goal")).anchor(Align2::CENTER_TOP, vec2(0.0, 80.0)).interactable(false).show(ctx, |ui| {
                    widgets::panel_frame(theme::mix(PANEL, TEAL, 0.25)).show(ui, |ui| {
                        ui.label(egui::RichText::new("🎯  Daily goal reached!  Keep the day streak going tomorrow.").font(theme::semibold(16.0)));
                    });
                });
                ctx.request_repaint();
            }
        }
    }

    fn handle_shot(&mut self, ctx: &egui::Context, now: f64) {
        let Some(shot) = &mut self.shot else { return };
        // Wait until the screen is ready (e.g. a turn solve finished).
        let busy = matches!(&self.session, Some(Session { hand: Hand::Post(h), .. }) if h.is_solving());
        let wants_feedback = shot.screen.ends_with("feedback");
        if !busy {
            if wants_feedback && self.session.as_ref().is_some_and(|s| s.feedback.is_none()) {
                // Answer with the first action so the feedback panel shows.
                self.answer(0, now);
                return;
            }
            shot.ready_frames += 1;
        }
        if shot.ready_frames == 12 && !shot.requested {
            shot.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        let images: Vec<_> = ctx.input(|i| {
            i.raw.events.iter().filter_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }).collect()
        });
        if let Some(img) = images.into_iter().next() {
            let [w, h] = img.size;
            let buf: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let ok = image::save_buffer(&shot.path, &buf, w as u32, h as u32, image::ExtendedColorType::Rgba8).is_ok();
            println!("screenshot {} {}x{} {}", shot.path, w, h, if ok { "saved" } else { "FAILED" });
            std::process::exit(if ok { 0 } else { 1 });
        }
        ctx.request_repaint();
    }

    fn keyboard(&mut self, ctx: &egui::Context, now: f64) {
        if self.screen != Screen::Train {
            return;
        }
        let Some(s) = &mut self.session else { return };
        let (space, keys) = ctx.input(|i| {
            let space = i.key_pressed(Key::Space) || i.key_pressed(Key::Enter);
            let keys: Vec<Key> = [
                Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::F, Key::C, Key::X, Key::B, Key::R, Key::A,
            ]
            .into_iter()
            .filter(|k| i.key_pressed(*k))
            .collect();
            (space, keys)
        });
        if s.feedback.is_some() {
            if space {
                self.continue_hand();
            }
            return;
        }
        let Some(styles) = train::current_styles(s) else {
            if space {
                self.continue_hand();
            }
            return;
        };
        let pick = keys.iter().find_map(|k| {
            let digit = [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6].iter().position(|d| d == k);
            if let Some(d) = digit {
                return (d < styles.len()).then_some(d);
            }
            styles.iter().position(|st| match (k, st) {
                (Key::F, widgets::ActStyle::Fold) => true,
                (Key::C | Key::X, widgets::ActStyle::Passive) => true,
                (Key::B | Key::R, widgets::ActStyle::Aggressive(0)) => true,
                (Key::A, widgets::ActStyle::AllIn) => true,
                _ => false,
            })
        });
        if let Some(i) = pick {
            self.answer(i, now);
        }
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frames += 1;
        let now = ctx.input(|i| i.time);
        // Pick up finished turn solves.
        if let Some(s) = &mut self.session {
            if let Hand::Post(h) = &mut s.hand {
                if h.poll(&mut self.rng) {
                    s.changed_at = now;
                }
                if h.is_solving() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(40));
                }
            }
        }
        self.keyboard(ctx, now);
        self.top_bar(ctx);
        match self.screen {
            Screen::Home => home::show(self, ctx),
            Screen::Train => train::show(self, ctx, now),
            Screen::Stats => stats::show(self, ctx),
            Screen::Settings => settings::show(self, ctx),
        }
        self.overlays(ctx, now);
        self.handle_shot(ctx, now);
    }
}

/// A plausible profile for screenshots of the home and stats screens.
fn demo_profile(rng: &mut Rng) -> Profile {
    let mut p = Profile::default();
    let cats = [
        (Mode::Cash, "RFI BTN"),
        (Mode::Cash, "RFI UTG"),
        (Mode::Cash, "BB vs BTN open"),
        (Mode::Cash, "SB vs BTN open"),
        (Mode::Cash, "BTN vs 3-bet"),
        (Mode::PushFold, "Push SB 10bb"),
        (Mode::PushFold, "Call BB vs SB jam"),
        (Mode::Postflop, "SRP flop BB"),
        (Mode::Postflop, "SRP turn BTN"),
        (Mode::Postflop, "3BP flop BB"),
    ];
    for k in 0..640 {
        let (mode, cat) = cats[rng.below(cats.len() as u32) as usize];
        let u = rng.unit_f32();
        let g = if u < 0.55 {
            Grade::Perfect
        } else if u < 0.75 {
            Grade::Good
        } else if u < 0.86 {
            Grade::Inaccuracy
        } else if u < 0.96 {
            Grade::Mistake
        } else {
            Grade::Blunder
        };
        let loss = if mode == Mode::Postflop { rng.unit_f32() as f64 * 0.6 } else { 0.0 };
        p.record(mode, cat, g, loss);
        if matches!(g, Grade::Mistake | Grade::Blunder) && k % 3 == 0 {
            p.add_mistake(MistakeRecord {
                date: today(),
                mode,
                category: cat.into(),
                spot: format!("{cat}, 100bb"),
                hand: ["A9o", "KTs", "76s", "QJo", "55"][k % 5].into(),
                chosen: "Fold".into(),
                solution: "Raise to 2.5 100%".into(),
                grade: g,
            });
        }
    }
    p.streak = 7;
    p
}
