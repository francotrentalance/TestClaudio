//! Range-vs-range equity with exact card removal.
//!
//! Each complete board is scored with one sorted sweep per player instead of
//! pairwise comparisons, so a flop is enumerated exactly (1,081 runouts) in a
//! fraction of a second. Preflop boards are sampled.

use crate::card::{card_bit, cards_mask, combo_cards, combo_mask, Card, Rng, NUM_COMBOS};
use crate::eval::eval_mask;
use crate::range::Range;

/// Win and tie weight of each hero hand against a villain range on a complete board.
pub struct SweepTotals {
    pub win: f64,
    pub tie: f64,
    pub total: f64,
}

/// `hero` and `villain` are `(strength, combo)` lists sorted by strength,
/// containing only hands that do not conflict with the board.
pub fn sweep(hero: &[(u32, u16)], villain: &[(u32, u16)], villain_w: &[f32]) -> Vec<SweepTotals> {
    let mut out: Vec<SweepTotals> =
        hero.iter().map(|_| SweepTotals { win: 0.0, tie: 0.0, total: 0.0 }).collect();
    let mut card_total = [0.0f64; 52];
    let mut total = 0.0f64;
    for &(_, o) in villain {
        let w = villain_w[o as usize] as f64;
        let (a, b) = combo_cards(o as usize);
        card_total[a as usize] += w;
        card_total[b as usize] += w;
        total += w;
    }

    let mut acc = 0.0f64;
    let mut acc_card = [0.0f64; 52];
    let mut j = 0;
    let mut i = 0;
    while i < hero.len() {
        let s = hero[i].0;
        while j < villain.len() && villain[j].0 < s {
            let o = villain[j].1 as usize;
            let w = villain_w[o] as f64;
            let (a, b) = combo_cards(o);
            acc += w;
            acc_card[a as usize] += w;
            acc_card[b as usize] += w;
            j += 1;
        }
        while i < hero.len() && hero[i].0 == s {
            let h = hero[i].1 as usize;
            let (a, b) = combo_cards(h);
            out[i].win = acc - acc_card[a as usize] - acc_card[b as usize];
            out[i].total = total - card_total[a as usize] - card_total[b as usize] + villain_w[h] as f64;
            i += 1;
        }
    }

    let mut acc = 0.0f64;
    let mut acc_card = [0.0f64; 52];
    let mut j = villain.len();
    let mut i = hero.len();
    while i > 0 {
        let s = hero[i - 1].0;
        while j > 0 && villain[j - 1].0 > s {
            let o = villain[j - 1].1 as usize;
            let w = villain_w[o] as f64;
            let (a, b) = combo_cards(o);
            acc += w;
            acc_card[a as usize] += w;
            acc_card[b as usize] += w;
            j -= 1;
        }
        while i > 0 && hero[i - 1].0 == s {
            let h = hero[i - 1].1 as usize;
            let (a, b) = combo_cards(h);
            let lose = acc - acc_card[a as usize] - acc_card[b as usize];
            let t = &mut out[i - 1];
            t.tie = t.total - t.win - lose;
            i -= 1;
        }
    }
    out
}

pub struct EquityResult {
    /// Overall equity (win + tie/2) of each range.
    pub equity: [f64; 2],
    /// Per-combo `(combo, equity, matchup weight)` for each range.
    pub hands: [Vec<(usize, f64, f64)>; 2],
    pub boards: u64,
    pub exact: bool,
}

fn sorted_hands(range: &Range, board_mask: u64) -> Vec<(u32, u16)> {
    let mut v: Vec<(u32, u16)> = (0..NUM_COMBOS)
        .filter(|&c| range.weights[c] > 0.0 && combo_mask(c) & board_mask == 0)
        .map(|c| (eval_mask(board_mask | combo_mask(c)), c as u16))
        .collect();
    v.sort_unstable();
    v
}

/// Equity of `r1` vs `r2` on `board` (0, 3, 4 or 5 cards). Runouts are enumerated
/// exactly when at most two cards are missing; otherwise `samples` random boards are used.
pub fn range_vs_range(r1: &Range, r2: &Range, board: &[Card], samples: u64, seed: u64) -> EquityResult {
    let ranges = [r1, r2];
    let mut num = [vec![0.0f64; NUM_COMBOS], vec![0.0f64; NUM_COMBOS]];
    let mut den = [vec![0.0f64; NUM_COMBOS], vec![0.0f64; NUM_COMBOS]];
    let mut boards = 0u64;

    let mut score = |full: u64| {
        let lists = [sorted_hands(r1, full), sorted_hands(r2, full)];
        for p in 0..2 {
            let res = sweep(&lists[p], &lists[1 - p], &ranges[1 - p].weights);
            for (k, &(_, h)) in lists[p].iter().enumerate() {
                num[p][h as usize] += res[k].win + 0.5 * res[k].tie;
                den[p][h as usize] += res[k].total;
            }
        }
        boards += 1;
    };

    let base = cards_mask(board);
    let deck: Vec<Card> = (0..52u8).filter(|&c| base & card_bit(c) == 0).collect();
    let missing = 5 - board.len();
    let exact = missing <= 2;
    match missing {
        0 => score(base),
        1 => deck.iter().for_each(|&c| score(base | card_bit(c))),
        2 => {
            for i in 0..deck.len() {
                for j in 0..i {
                    score(base | card_bit(deck[i]) | card_bit(deck[j]));
                }
            }
        }
        _ => {
            let mut rng = Rng::new(seed);
            for _ in 0..samples {
                let mut m = base;
                let mut added = 0;
                while added < missing {
                    let c = deck[rng.below(deck.len() as u32) as usize];
                    if m & card_bit(c) == 0 {
                        m |= card_bit(c);
                        added += 1;
                    }
                }
                score(m);
            }
        }
    }

    let mut equity = [0.0; 2];
    let mut hands: [Vec<(usize, f64, f64)>; 2] = [Vec::new(), Vec::new()];
    for p in 0..2 {
        let (mut n, mut d) = (0.0, 0.0);
        for c in 0..NUM_COMBOS {
            let w = ranges[p].weights[c] as f64;
            if w > 0.0 && den[p][c] > 0.0 {
                n += w * num[p][c];
                d += w * den[p][c];
                hands[p].push((c, num[p][c] / den[p][c], w * den[p][c]));
            }
        }
        equity[p] = if d > 0.0 { n / d } else { 0.0 };
    }
    EquityResult { equity, hands, boards, exact }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_cards;

    fn eq(a: &str, b: &str, board: &str) -> f64 {
        let board = parse_cards(board).unwrap();
        range_vs_range(&Range::parse(a).unwrap(), &Range::parse(b).unwrap(), &board, 200_000, 1).equity[0]
    }

    #[test]
    fn known_matchups() {
        // AA vs KK preflop is ~81.9% (sampled).
        assert!((eq("AA", "KK", "") - 0.819).abs() < 0.005);
        // AKs vs QQ ~ 46%.
        assert!((eq("AKs", "QQ", "") - 0.46).abs() < 0.01);
        // Exact on the river.
        assert_eq!(eq("AsAh", "KsKh", "2c3c4d8h9s"), 1.0);
        assert_eq!(eq("AsKh", "AdKc", "2c3c4d8h9s"), 0.5);
        // Flop: set over set needs runner-runner (exactly 1 out on the turn+river of 990).
        let e = eq("KsKh", "QsQh", "Kd Qd 2c");
        assert!(e > 0.95 && e < 1.0);
        let sum = eq("AA,KK,AKs", "QQ,JJ,AQs", "Ts7d2c") + eq("QQ,JJ,AQs", "AA,KK,AKs", "Ts7d2c");
        assert!((sum - 1.0).abs() < 1e-9);
    }
}
