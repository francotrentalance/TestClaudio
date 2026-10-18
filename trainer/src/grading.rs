//! Grading a decision against the solution, and the XP it earns.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Grade {
    Perfect,
    Good,
    Inaccuracy,
    Mistake,
    Blunder,
}

pub const GRADES: [Grade; 5] = [Grade::Perfect, Grade::Good, Grade::Inaccuracy, Grade::Mistake, Grade::Blunder];

impl Grade {
    pub fn label(self) -> &'static str {
        match self {
            Grade::Perfect => "Perfect",
            Grade::Good => "Good",
            Grade::Inaccuracy => "Inaccuracy",
            Grade::Mistake => "Mistake",
            Grade::Blunder => "Blunder",
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// Base XP before the streak bonus.
    pub fn xp(self) -> u32 {
        match self {
            Grade::Perfect => 10,
            Grade::Good => 7,
            Grade::Inaccuracy => 3,
            Grade::Mistake | Grade::Blunder => 0,
        }
    }

    /// Whether the decision counts as correct (keeps the streak, counts toward accuracy).
    pub fn is_correct(self) -> bool {
        matches!(self, Grade::Perfect | Grade::Good)
    }

    pub fn breaks_streak(self) -> bool {
        matches!(self, Grade::Mistake | Grade::Blunder)
    }
}

/// Grades by how often the solution takes the chosen action with this hand.
/// Used preflop, where the solution gives frequencies.
pub fn by_frequency(freqs: &[f32], chosen: usize) -> Grade {
    let f = freqs[chosen];
    let best = freqs.iter().cloned().fold(0.0f32, f32::max);
    if f >= 0.5 || f >= 0.9 * best {
        Grade::Perfect
    } else if f >= 0.2 {
        Grade::Good
    } else if f >= 0.05 {
        Grade::Inaccuracy
    } else if best >= 0.9 && f < 0.02 {
        Grade::Blunder
    } else {
        Grade::Mistake
    }
}

/// Grades by EV lost against the best action, as a percentage of the pot.
/// Actions the solution itself mixes in at least 25% of the time are never
/// worse than "Good" (at equilibrium mixed actions have equal EV), unless
/// `trust_freqs` is false: off the solution's path the hand's frequencies are
/// untrained noise, while its EVs stay valid.
pub fn by_ev(evs: &[f32], freqs: &[f32], chosen: usize, pot: f32, trust_freqs: bool) -> (Grade, f32) {
    let best = evs.iter().cloned().filter(|x| x.is_finite()).fold(f32::NEG_INFINITY, f32::max);
    let loss = if evs[chosen].is_finite() && best.is_finite() { (best - evs[chosen]).max(0.0) } else { 0.0 };
    let pct = if pot > 0.0 { 100.0 * loss / pot } else { 0.0 };
    let mut grade = if pct <= 1.0 {
        Grade::Perfect
    } else if pct <= 3.0 {
        Grade::Good
    } else if pct <= 8.0 {
        Grade::Inaccuracy
    } else if pct <= 20.0 {
        Grade::Mistake
    } else {
        Grade::Blunder
    };
    if trust_freqs && freqs[chosen] >= 0.25 && grade > Grade::Good {
        grade = Grade::Good;
    }
    (grade, loss)
}

/// XP for a decision given the current streak (one extra XP per 5 in a row, up to +5).
pub fn xp_for(grade: Grade, streak_before: u32) -> u32 {
    let base = grade.xp();
    if base == 0 {
        return 0;
    }
    base + (streak_before / 5).min(5)
}

/// Total XP needed to reach `level` (level 1 needs none).
pub fn xp_for_level(level: u32) -> u64 {
    let l = level.max(1) as u64;
    25 * l * (l - 1)
}

pub fn level_for_xp(xp: u64) -> u32 {
    let mut level = 1;
    while xp_for_level(level + 1) <= xp {
        level += 1;
    }
    level
}

pub fn title_for_level(level: u32) -> &'static str {
    match level {
        0..=2 => "Rookie",
        3..=5 => "Regular",
        6..=9 => "Grinder",
        10..=14 => "Shark",
        15..=19 => "Crusher",
        20..=29 => "Solver Mind",
        _ => "GTO Legend",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_grades() {
        assert_eq!(by_frequency(&[0.0, 1.0], 1), Grade::Perfect);
        assert_eq!(by_frequency(&[0.0, 1.0], 0), Grade::Blunder);
        assert_eq!(by_frequency(&[0.6, 0.4], 1), Grade::Good);
        assert_eq!(by_frequency(&[0.45, 0.45, 0.1], 1), Grade::Perfect);
        assert_eq!(by_frequency(&[0.9, 0.1], 1), Grade::Inaccuracy);
        assert_eq!(by_frequency(&[0.85, 0.12, 0.03], 2), Grade::Mistake);
    }

    #[test]
    fn ev_grades() {
        // Pot 10: losing 0.05 is Perfect, 0.25 Good, 0.5 Inaccuracy, 1.5 Mistake, 3 Blunder.
        let freqs = [1.0, 0.0];
        assert_eq!(by_ev(&[5.0, 4.95], &freqs, 1, 10.0, true).0, Grade::Perfect);
        assert_eq!(by_ev(&[5.0, 4.75], &freqs, 1, 10.0, true).0, Grade::Good);
        assert_eq!(by_ev(&[5.0, 4.5], &freqs, 1, 10.0, true).0, Grade::Inaccuracy);
        assert_eq!(by_ev(&[5.0, 3.5], &freqs, 1, 10.0, true).0, Grade::Mistake);
        assert_eq!(by_ev(&[5.0, 2.0], &freqs, 1, 10.0, true).0, Grade::Blunder);
        // A mixed action is at worst Good even if the sampled EVs differ a bit.
        assert_eq!(by_ev(&[5.0, 4.0], &[0.6, 0.4], 1, 10.0, true).0, Grade::Good);
        // Off the solution's path the frequencies are noise: EV alone decides.
        assert_eq!(by_ev(&[5.0, 4.0], &[0.6, 0.4], 1, 10.0, false).0, Grade::Mistake);
        let (_, loss) = by_ev(&[5.0, 3.5], &freqs, 1, 10.0, true);
        assert!((loss - 1.5).abs() < 1e-6);
    }

    #[test]
    fn levels() {
        assert_eq!(level_for_xp(0), 1);
        assert_eq!(level_for_xp(49), 1);
        assert_eq!(level_for_xp(50), 2);
        assert_eq!(level_for_xp(150), 3);
        assert_eq!(xp_for_level(10), 2250);
        assert_eq!(xp_for(Grade::Perfect, 0), 10);
        assert_eq!(xp_for(Grade::Perfect, 12), 12);
        assert_eq!(xp_for(Grade::Perfect, 100), 15);
        assert_eq!(xp_for(Grade::Mistake, 100), 0);
    }
}
