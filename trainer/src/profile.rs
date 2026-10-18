//! The player's progress: XP, level, streaks, daily goal, per-category stats
//! and recent mistakes. Saved as JSON in `$XDG_DATA_HOME/holdem-trainer/`
//! (normally `~/.local/share/holdem-trainer/`).

use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::grading::{Grade, level_for_xp, xp_for};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Mode {
    Cash,
    PushFold,
    Postflop,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Cash => "Preflop Cash",
            Mode::PushFold => "Push / Fold",
            Mode::Postflop => "Postflop",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CategoryStats {
    pub decisions: u32,
    /// Count per grade, indexed by `Grade::index()`.
    pub grades: [u32; 5],
    /// Sum of EV lost (big blinds), for categories graded by EV.
    pub ev_loss: f64,
}

impl CategoryStats {
    pub fn accuracy(&self) -> Option<f64> {
        (self.decisions > 0).then(|| (self.grades[0] + self.grades[1]) as f64 / self.decisions as f64)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MistakeRecord {
    /// Local date, `YYYY-MM-DD`.
    pub date: String,
    pub mode: Mode,
    pub category: String,
    /// e.g. "BTN, folded to you, 100bb".
    pub spot: String,
    /// e.g. "A9o".
    pub hand: String,
    pub chosen: String,
    /// e.g. "Raise 2.5 100%".
    pub solution: String,
    pub grade: Grade,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub daily_goal: u32,
    /// 0 = easy (every hand), 1 = normal (fewer obvious hands), 2 = hard (close decisions).
    pub difficulty: u8,
    /// Preflop positions to train, by name (UTG, HJ, CO, BTN, SB, BB).
    pub cash_positions: Vec<String>,
    pub pushfold_min: f64,
    pub pushfold_max: f64,
    pub pushfold_positions: Vec<String>,
    /// Postflop spot kinds to train.
    pub postflop_spots: Vec<String>,
    /// "any", "oop" or "ip".
    pub postflop_role: String,
    pub four_color_deck: bool,
    pub animations: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let all: Vec<String> = ["UTG", "HJ", "CO", "BTN", "SB", "BB"].iter().map(|s| s.to_string()).collect();
        Settings {
            daily_goal: 50,
            difficulty: 1,
            cash_positions: all.clone(),
            pushfold_min: 5.0,
            pushfold_max: 20.0,
            pushfold_positions: all,
            postflop_spots: vec![crate::gen::SRP.to_string(), crate::gen::THREE_BET.to_string()],
            postflop_role: "any".into(),
            four_color_deck: true,
            animations: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub version: u32,
    pub xp: u64,
    pub decisions: u32,
    pub hands: u32,
    pub streak: u32,
    pub best_streak: u32,
    /// Decisions per local date (`YYYY-MM-DD`).
    pub daily: BTreeMap<String, u32>,
    pub categories: BTreeMap<String, CategoryStats>,
    pub modes: BTreeMap<String, CategoryStats>,
    pub mistakes: VecDeque<MistakeRecord>,
    pub settings: Settings,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            version: 1,
            xp: 0,
            decisions: 0,
            hands: 0,
            streak: 0,
            best_streak: 0,
            daily: BTreeMap::new(),
            categories: BTreeMap::new(),
            modes: BTreeMap::new(),
            mistakes: VecDeque::new(),
            settings: Settings::default(),
        }
    }
}

/// What recording a decision changed, for the feedback animation.
#[derive(Clone, Copy, Debug)]
pub struct Award {
    pub xp: u32,
    pub level_before: u32,
    pub level_after: u32,
    pub streak: u32,
    pub goal_reached_now: bool,
}

pub const MAX_MISTAKES: usize = 200;

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("holdem-trainer")
}

impl Profile {
    pub fn path() -> PathBuf {
        data_dir().join("profile.json")
    }

    /// Loads the saved profile, or a fresh one. A corrupt file is kept aside
    /// as `profile.json.bad` rather than overwritten.
    pub fn load() -> Profile {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(p) => p,
                Err(_) => {
                    let _ = std::fs::rename(&path, path.with_extension("json.bad"));
                    Profile::default()
                }
            },
            Err(_) => Profile::default(),
        }
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, path)
    }

    pub fn level(&self) -> u32 {
        level_for_xp(self.xp)
    }

    pub fn today_count(&self) -> u32 {
        self.daily.get(&today()).copied().unwrap_or(0)
    }

    /// Consecutive days, ending today or yesterday, on which the daily goal was met.
    pub fn day_streak(&self) -> u32 {
        let goal = self.settings.daily_goal.max(1);
        let mut day = chrono::Local::now().date_naive();
        let key = |d: chrono::NaiveDate| d.format("%Y-%m-%d").to_string();
        // Today does not break the streak until it is over.
        if self.daily.get(&key(day)).copied().unwrap_or(0) < goal {
            day = day.pred_opt().unwrap_or(day);
        }
        let mut n = 0;
        while self.daily.get(&key(day)).copied().unwrap_or(0) >= goal {
            n += 1;
            match day.pred_opt() {
                Some(d) => day = d,
                None => break,
            }
        }
        n
    }

    /// Records one graded decision and returns what it earned.
    pub fn record(&mut self, mode: Mode, category: &str, grade: Grade, ev_loss_bb: f64) -> Award {
        let level_before = self.level();
        let xp = xp_for(grade, self.streak);
        self.xp += xp as u64;
        self.decisions += 1;
        if grade.breaks_streak() {
            self.streak = 0;
        } else if grade.is_correct() {
            self.streak += 1;
            self.best_streak = self.best_streak.max(self.streak);
        }
        let today = today();
        let goal = self.settings.daily_goal;
        let before = self.daily.get(&today).copied().unwrap_or(0);
        self.daily.insert(today, before + 1);
        for stats in [
            self.categories.entry(category.to_string()).or_default(),
            self.modes.entry(format!("{mode:?}")).or_default(),
        ] {
            stats.decisions += 1;
            stats.grades[grade.index()] += 1;
            stats.ev_loss += ev_loss_bb;
        }
        Award {
            xp,
            level_before,
            level_after: self.level(),
            streak: self.streak,
            goal_reached_now: before < goal && before + 1 >= goal,
        }
    }

    pub fn add_mistake(&mut self, record: MistakeRecord) {
        self.mistakes.push_front(record);
        self.mistakes.truncate(MAX_MISTAKES);
    }

    pub fn mode_stats(&self, mode: Mode) -> CategoryStats {
        self.modes.get(&format!("{mode:?}")).cloned().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_updates_xp_streak_and_stats() {
        let mut p = Profile::default();
        p.settings.daily_goal = 2;
        let a = p.record(Mode::Cash, "RFI BTN", Grade::Perfect, 0.0);
        assert_eq!(a.xp, 10);
        assert_eq!(p.streak, 1);
        let a = p.record(Mode::Cash, "RFI BTN", Grade::Good, 0.0);
        assert!(a.goal_reached_now);
        assert_eq!(p.streak, 2);
        p.record(Mode::Cash, "RFI BTN", Grade::Inaccuracy, 0.0);
        assert_eq!(p.streak, 2, "inaccuracies neither build nor break the streak");
        p.record(Mode::Postflop, "Flop", Grade::Blunder, 3.5);
        assert_eq!(p.streak, 0);
        assert_eq!(p.best_streak, 2);
        let c = &p.categories["RFI BTN"];
        assert_eq!(c.decisions, 3);
        assert_eq!(c.accuracy(), Some(2.0 / 3.0));
        assert_eq!(p.mode_stats(Mode::Postflop).ev_loss, 3.5);
        assert_eq!(p.today_count(), 4);
        assert_eq!(p.day_streak(), 1);
    }

    #[test]
    fn profile_json_roundtrip_and_defaults() {
        let mut p = Profile::default();
        p.record(Mode::PushFold, "Push 10bb", Grade::Perfect, 0.0);
        let text = serde_json::to_string(&p).unwrap();
        let back: Profile = serde_json::from_str(&text).unwrap();
        assert_eq!(back.xp, p.xp);
        // Older files without new fields still load.
        let old: Profile = serde_json::from_str("{\"xp\": 120}").unwrap();
        assert_eq!(old.xp, 120);
        assert_eq!(old.settings.daily_goal, 50);
    }
}
