//! Types shared by the quiz engines.

use holdem_solver::card::{Card, Rng};

use crate::grading::Grade;

/// How a player is shown at the table.
#[derive(Clone, Debug)]
pub struct SeatView {
    pub name: String,
    /// Chips behind (big blinds).
    pub stack: f32,
    /// Chips in front of the player on this street.
    pub bet: f32,
    pub folded: bool,
    pub all_in: bool,
    pub is_hero: bool,
    pub is_button: bool,
    pub to_act: bool,
    /// Face-up cards (the hero's, or a villain's at showdown).
    pub cards: Option<[Card; 2]>,
    /// Whether the player still holds cards (face down if `cards` is None).
    pub has_cards: bool,
    pub last_action: Option<String>,
}

/// What the solution does with the hero's hand, and how the choice was graded.
#[derive(Clone, Debug)]
pub struct Feedback {
    pub grade: Grade,
    pub chosen: usize,
    pub actions: Vec<String>,
    pub freqs: Vec<f32>,
    /// EV of each action (big blinds), when the solution provides it.
    pub evs: Option<Vec<f32>>,
    /// EV lost against the best action (big blinds; 0 when graded by frequency).
    pub ev_loss: f32,
    pub pot: f32,
    pub category: String,
    pub spot: String,
    pub hand: String,
    /// Strategy of every starting-hand class at this decision (preflop only):
    /// `169 x actions`, with the hero's class.
    pub grid: Option<(Vec<Vec<f32>>, usize)>,
}

/// One line of the hand history.
#[derive(Clone, Debug)]
pub struct LogEntry {
    pub street: String,
    pub who: String,
    pub what: String,
    pub hero: bool,
}

pub fn shuffled_deck(rng: &mut Rng) -> Vec<Card> {
    let mut deck: Vec<Card> = (0..52u8).collect();
    for i in (1..deck.len()).rev() {
        let j = rng.below(i as u32 + 1) as usize;
        deck.swap(i, j);
    }
    deck
}

/// Picks an index with probability proportional to `weights` (None if all are zero).
pub fn sample_weighted(weights: &[f32], rng: &mut Rng) -> Option<usize> {
    let total: f64 = weights.iter().map(|&w| w.max(0.0) as f64).sum();
    if total <= 0.0 {
        return None;
    }
    let mut u = rng.unit_f32() as f64 * total;
    for (i, &w) in weights.iter().enumerate() {
        u -= w.max(0.0) as f64;
        if u < 0.0 {
            return Some(i);
        }
    }
    weights.iter().rposition(|&w| w > 0.0)
}

/// Whether to skip a decision whose solution is (nearly) pure, by difficulty:
/// easy keeps everything; normal skips most obvious folds; hard skips most pure spots.
pub fn skip_obvious(freqs: &[f32], fold_index: Option<usize>, difficulty: u8, rng: &mut Rng) -> bool {
    let best = freqs.iter().cloned().fold(0.0f32, f32::max);
    if best < 0.97 {
        return false;
    }
    let is_fold = fold_index.is_some_and(|f| freqs[f] >= 0.97);
    let p = match (difficulty, is_fold) {
        (0, _) => 0.0,
        (1, true) => 0.8,
        (1, false) => 0.3,
        (_, true) => 0.95,
        (_, false) => 0.85,
    };
    rng.unit_f32() < p
}

pub fn fmt_bb(x: f32) -> String {
    let s = format!("{:.1}", x);
    s.trim_end_matches(".0").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_sampling_follows_weights() {
        let mut rng = Rng::new(3);
        let mut counts = [0; 3];
        for _ in 0..30_000 {
            counts[sample_weighted(&[1.0, 0.0, 3.0], &mut rng).unwrap()] += 1;
        }
        assert_eq!(counts[1], 0);
        let ratio = counts[2] as f64 / counts[0] as f64;
        assert!((ratio - 3.0).abs() < 0.2, "{ratio}");
        assert_eq!(sample_weighted(&[0.0, 0.0], &mut rng), None);
    }

    #[test]
    fn deck_is_a_permutation() {
        let mut d = shuffled_deck(&mut Rng::new(1));
        d.sort();
        assert_eq!(d, (0..52u8).collect::<Vec<_>>());
    }
}
