//! Discounted CFR (Brown & Sandholm 2019, alpha = 1.5, beta = 0, gamma = 2) with
//! alternating updates, vectorized over each player's hands, plus best-response
//! evaluation for exploitability.
//!
//! Values are counterfactual: `cfv[h]` is the sum over opponent hands `o` of
//! `reach_o[o] * chance * utility(h, o)`. Utilities are shifted by half the
//! starting pot so the game is zero-sum without rake: winning a pot where each
//! player has contributed `c` (half the starting pot included) is `+c`.

use rayon::prelude::*;

use crate::card::Card;
use crate::postflop::game::PostflopGame;
use crate::postflop::tree::{Action, Node, NodeKind, NONE};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Discount {
    pos: f32,
    neg: f32,
    strat: f32,
}

impl Discount {
    pub fn for_iteration(t: u32) -> Self {
        let prev = (t.saturating_sub(1)) as f64;
        let ta = prev.powf(1.5);
        Discount { pos: (ta / (ta + 1.0)) as f32, neg: 0.5, strat: ((prev / t as f64).powi(2)) as f32 }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Mode {
    Cfr(Discount),
    /// The traverser best-responds to the opponent's average strategy.
    BestResponse,
    /// Both players follow their average strategies.
    Average,
}

/// Cards dealt after the initial board along the current path.
#[derive(Clone, Copy, Debug, Default)]
pub struct Dealt {
    pub cards: [Card; 2],
    pub len: u8,
}

impl Dealt {
    pub fn push(mut self, c: Card) -> Self {
        self.cards[self.len as usize] = c;
        self.len += 1;
        self
    }

    pub fn as_slice(&self) -> &[Card] {
        &self.cards[..self.len as usize]
    }
}

pub(crate) fn regret_matching(regrets: &[f32], na: usize, nh: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; na * nh];
    let mut sum = vec![0.0f32; nh];
    for a in 0..na {
        let row = &regrets[a * nh..(a + 1) * nh];
        let o = &mut out[a * nh..(a + 1) * nh];
        for h in 0..nh {
            let r = row[h].max(0.0);
            o[h] = r;
            sum[h] += r;
        }
    }
    let uniform = 1.0 / na as f32;
    for a in 0..na {
        let o = &mut out[a * nh..(a + 1) * nh];
        for h in 0..nh {
            o[h] = if sum[h] > 0.0 { o[h] / sum[h] } else { uniform };
        }
    }
    out
}

impl PostflopGame {
    /// Runs one DCFR iteration (an update for each player).
    pub fn iterate(&mut self) {
        assert!(!self.finalized, "cannot keep solving a finalized game");
        let t = self.iteration + 1;
        let d = Discount::for_iteration(t);
        for p in 0..2 {
            self.recurse(
                0,
                p,
                &self.initial_weights[p],
                &self.initial_weights[1 - p],
                Dealt::default(),
                Mode::Cfr(d),
            );
        }
        self.iteration = t;
    }

    /// Value of `p`'s whole range under `mode`, in chips (pot share minus
    /// chips put in after the start of the spot).
    fn root_value(&self, p: usize, mode: Mode) -> f64 {
        let cfv = self.recurse(
            0,
            p,
            &self.initial_weights[p],
            &self.initial_weights[1 - p],
            Dealt::default(),
            mode,
        );
        let total: f64 = cfv.iter().zip(&self.initial_weights[p]).map(|(&v, &w)| v as f64 * w as f64).sum();
        total / self.norm + self.config.pot / 2.0
    }

    /// Expected value of each player when both play their average strategies.
    pub fn expected_values(&self) -> [f64; 2] {
        [self.root_value(0, Mode::Average), self.root_value(1, Mode::Average)]
    }

    /// Best-response value of each player against the other's average strategy.
    pub fn best_response_values(&self) -> [f64; 2] {
        [self.root_value(0, Mode::BestResponse), self.root_value(1, Mode::BestResponse)]
    }

    /// Exploitability in chips: the average gain of a best response over the
    /// current profile, `(BR_oop - EV_oop + BR_ip - EV_ip) / 2`.
    pub fn exploitability(&self) -> f64 {
        let ev = self.expected_values();
        let br = self.best_response_values();
        ((br[0] - ev[0]) + (br[1] - ev[1])) / 2.0
    }

    /// Exploitability as a percentage of the starting pot.
    pub fn exploitability_pct(&self) -> f64 {
        100.0 * self.exploitability() / self.config.pot
    }

    pub(crate) fn recurse(
        &self,
        idx: usize,
        p: usize,
        reach_p: &[f32],
        reach_o: &[f32],
        dealt: Dealt,
        mode: Mode,
    ) -> Vec<f32> {
        let node = &self.nodes[idx];
        match node.kind {
            NodeKind::Fold(f) => self.fold_value(node, f as usize, p, reach_o),
            NodeKind::Showdown => self.showdown_value(node, p, reach_o, dealt),
            NodeKind::Chance => self.chance_value(node, p, reach_p, reach_o, dealt, mode),
            NodeKind::Player(q) if q as usize == p => self.own_value(node, p, reach_p, reach_o, dealt, mode),
            NodeKind::Player(_) => self.opp_value(node, p, reach_p, reach_o, dealt, mode),
        }
    }

    fn own_value(
        &self,
        node: &Node,
        p: usize,
        reach_p: &[f32],
        reach_o: &[f32],
        dealt: Dealt,
        mode: Mode,
    ) -> Vec<f32> {
        let nh = self.hands[p].len();
        let na = node.num_children as usize;
        let first = node.first_child as usize;

        match mode {
            Mode::Cfr(d) => {
                let block = self.block(node);
                // SAFETY: this node's block is touched only by the thread recursing into it.
                let strat = regret_matching(&unsafe { self.storage.regrets(block) }, na, nh);
                let mut child_vals = Vec::with_capacity(na);
                for a in 0..na {
                    let s = &strat[a * nh..(a + 1) * nh];
                    let child_reach: Vec<f32> = reach_p.iter().zip(s).map(|(r, s)| r * s).collect();
                    child_vals.push(self.recurse(first + a, p, &child_reach, reach_o, dealt, mode));
                }
                let mut value = vec![0.0f32; nh];
                for a in 0..na {
                    let s = &strat[a * nh..(a + 1) * nh];
                    for h in 0..nh {
                        value[h] += s[h] * child_vals[a][h];
                    }
                }
                // SAFETY: as above.
                unsafe {
                    self.storage.update(block, |regrets, cum| {
                        for a in 0..na {
                            let row = &mut regrets[a * nh..(a + 1) * nh];
                            let cv = &child_vals[a];
                            for h in 0..nh {
                                let r = row[h];
                                let disc = if r >= 0.0 { d.pos } else { d.neg };
                                row[h] = r * disc + cv[h] - value[h];
                            }
                        }
                        for a in 0..na {
                            let row = &mut cum[a * nh..(a + 1) * nh];
                            let s = &strat[a * nh..(a + 1) * nh];
                            for h in 0..nh {
                                row[h] = row[h] * d.strat + reach_p[h] * s[h];
                            }
                        }
                    });
                }
                value
            }
            Mode::BestResponse => {
                let mut value = vec![f32::NEG_INFINITY; nh];
                for a in 0..na {
                    let v = self.recurse(first + a, p, reach_p, reach_o, dealt, mode);
                    for h in 0..nh {
                        value[h] = value[h].max(v[h]);
                    }
                }
                value
            }
            Mode::Average => {
                let strat = self.average_strategy(node);
                let mut value = vec![0.0f32; nh];
                for a in 0..na {
                    let s = &strat[a * nh..(a + 1) * nh];
                    let child_reach: Vec<f32> = reach_p.iter().zip(s).map(|(r, s)| r * s).collect();
                    let v = self.recurse(first + a, p, &child_reach, reach_o, dealt, mode);
                    for h in 0..nh {
                        value[h] += s[h] * v[h];
                    }
                }
                value
            }
        }
    }

    fn opp_value(
        &self,
        node: &Node,
        p: usize,
        reach_p: &[f32],
        reach_o: &[f32],
        dealt: Dealt,
        mode: Mode,
    ) -> Vec<f32> {
        let o = 1 - p;
        let no = self.hands[o].len();
        let na = node.num_children as usize;
        let first = node.first_child as usize;
        let strat = match mode {
            Mode::Cfr(_) => {
                // SAFETY: read-only access; the opponent's block is written only on its own pass.
                regret_matching(&unsafe { self.storage.regrets(self.block(node)) }, na, no)
            }
            _ => self.average_strategy(node),
        };
        let mut value = vec![0.0f32; self.hands[p].len()];
        for a in 0..na {
            let s = &strat[a * no..(a + 1) * no];
            let child_reach: Vec<f32> = reach_o.iter().zip(s).map(|(r, s)| r * s).collect();
            if child_reach.iter().all(|&r| r == 0.0) {
                continue;
            }
            let v = self.recurse(first + a, p, reach_p, &child_reach, dealt, mode);
            for (x, y) in value.iter_mut().zip(&v) {
                *x += y;
            }
        }
        value
    }

    fn chance_value(
        &self,
        node: &Node,
        p: usize,
        reach_p: &[f32],
        reach_o: &[f32],
        dealt: Dealt,
        mode: Mode,
    ) -> Vec<f32> {
        let o = 1 - p;
        let nh = self.hands[p].len();
        // Each (hand, hand) pair leaves 52 - board - 4 cards that can come.
        let deals = (52 - node.board_len as usize - 4) as f32;
        let deal = |ci: usize| {
            let Action::Deal(card) = self.nodes[ci].action else { unreachable!("chance children are deals") };
            let mut rp = reach_p.to_vec();
            for &h in &self.card_hands[p][card as usize] {
                rp[h as usize] = 0.0;
            }
            let mut ro = reach_o.to_vec();
            for &h in &self.card_hands[o][card as usize] {
                ro[h as usize] = 0.0;
            }
            let mut v = if ro.iter().all(|&r| r == 0.0) {
                vec![0.0; nh]
            } else {
                self.recurse(ci, p, &rp, &ro, dealt.push(card), mode)
            };
            for &h in &self.card_hands[p][card as usize] {
                v[h as usize] = 0.0;
            }
            v
        };
        let results: Vec<Vec<f32>> = node.children().into_par_iter().map(deal).collect();

        let mut total = vec![0.0f32; nh];
        for v in &results {
            for (t, x) in total.iter_mut().zip(v) {
                *t += x;
            }
        }
        if node.iso != NONE {
            let first = node.first_child as usize;
            for e in &self.iso_tables[node.iso as usize] {
                let v = &results[e.rep_child as usize - first];
                let map = &self.swap_maps[p][e.pair as usize];
                for (h, t) in total.iter_mut().enumerate() {
                    *t += v[map[h] as usize];
                }
            }
        }
        let inv = 1.0 / deals;
        for t in &mut total {
            *t *= inv;
        }
        total
    }

    /// Sum of opponent reach per card, and in total.
    fn card_sums(&self, o: usize, reach_o: &[f32]) -> ([f32; 52], f32) {
        let mut cs = [0.0f32; 52];
        let hands = &self.hands[o];
        if hands.is_empty() {
            return (cs, 0.0);
        }
        // Hands are ordered by their higher card, so accumulate that card in a
        // register instead of chaining dependent adds through `cs`.
        let mut cur = hands[0].0;
        let mut run = 0.0f32;
        for (&r, &(a, b)) in reach_o.iter().zip(hands) {
            if a != cur {
                cs[cur as usize] += run;
                cur = a;
                run = 0.0;
            }
            run += r;
            cs[b as usize] += r;
        }
        cs[cur as usize] += run;
        let total = reach_o.iter().sum();
        (cs, total)
    }

    fn fold_value(&self, node: &Node, folder: usize, p: usize, reach_o: &[f32]) -> Vec<f32> {
        let o = 1 - p;
        let pot = node.pot() as f64;
        let rake = self.config.rake_amount(pot) as f32;
        let u = if p == folder { -node.contrib[folder] } else { node.contrib[folder] - rake };
        let (cs, total) = self.card_sums(o, reach_o);
        self.hands[p]
            .iter()
            .zip(&self.same_hand[p])
            .map(|(&(a, b), &same)| {
                let same_r = if same == NONE { 0.0 } else { reach_o[same as usize] };
                u * (total - cs[a as usize] - cs[b as usize] + same_r)
            })
            .collect()
    }

    fn showdown_value(&self, node: &Node, p: usize, reach_o: &[f32], dealt: Dealt) -> Vec<f32> {
        let o = 1 - p;
        let c = node.contrib[0];
        let rake = self.config.rake_amount(node.pot() as f64) as f32;
        let (u_win, u_lose, u_tie) = (c - rake, -c, -rake / 2.0);
        let ev = self.river_eval(dealt.as_slice());
        let (hero, vill) = (&ev.sorted[p], &ev.sorted[o]);
        let (weaker, stronger) = (&ev.weaker[p], &ev.stronger[p]);
        let mut value = vec![0.0f32; self.hands[p].len()];

        // Ascending sweep: opponent weight strictly weaker than each hero hand,
        // minus the hands that share a card with it. Stored in `value` for now.
        let mut acc = 0.0f32;
        let mut acc_card = [0.0f32; 52];
        let mut j = 0usize;
        for (e, &end) in hero.iter().zip(weaker) {
            while j < end as usize {
                let v = &vill[j];
                let r = reach_o[v.idx as usize];
                acc += r;
                acc_card[v.cards[0] as usize] += r;
                acc_card[v.cards[1] as usize] += r;
                j += 1;
            }
            let [a, b] = e.cards;
            value[e.idx as usize] = acc - acc_card[a as usize] - acc_card[b as usize];
        }

        // Descending sweep: strictly stronger opponent hands.
        let tie_info = (rake > 0.0).then(|| self.card_sums(o, reach_o));
        let mut acc = 0.0f32;
        let mut acc_card = [0.0f32; 52];
        let mut j = vill.len();
        for (e, &start) in hero.iter().zip(stronger).rev() {
            while j > start as usize {
                j -= 1;
                let v = &vill[j];
                let r = reach_o[v.idx as usize];
                acc += r;
                acc_card[v.cards[0] as usize] += r;
                acc_card[v.cards[1] as usize] += r;
            }
            let [a, b] = e.cards;
            let h = e.idx as usize;
            let lose = acc - acc_card[a as usize] - acc_card[b as usize];
            let win = value[h];
            let mut v = u_win * win + u_lose * lose;
            if let Some((cs, total)) = &tie_info {
                // Ties pay rake too: tie weight = valid opponent hands - wins - losses.
                let same = self.same_hand[p][h];
                let same_r = if same == NONE { 0.0 } else { reach_o[same as usize] };
                let valid = total - cs[a as usize] - cs[b as usize] + same_r;
                v += u_tie * (valid - win - lose);
            }
            value[h] = v;
        }
        value
    }

    /// Current (regret-matching) strategy at a node, for diagnostics.
    pub fn current_strategy(&self, node: &Node) -> Vec<f32> {
        let NodeKind::Player(q) = node.kind else { return Vec::new() };
        if self.finalized {
            return self.average_strategy(node);
        }
        let nh = self.hands[q as usize].len();
        let na = node.num_children as usize;
        // SAFETY: called between iterations, when nothing writes the storage.
        regret_matching(&unsafe { self.storage.regrets(self.block(node)) }, na, nh)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::{card_bit, cards_mask, Rng};
    use crate::eval::eval_mask;
    use crate::postflop::PostflopConfig;

    fn brute(g: &PostflopGame, node: &Node, p: usize, reach_o: &[f32]) -> Vec<f64> {
        let o = 1 - p;
        let board = cards_mask(&g.board);
        let pot = node.pot() as f64;
        let rake = g.config.rake_amount(pot);
        g.hands[p]
            .iter()
            .map(|&(a, b)| {
                let hm = card_bit(a) | card_bit(b);
                if hm & board != 0 {
                    return 0.0;
                }
                let mut v = 0.0;
                for (oi, &(c, d)) in g.hands[o].iter().enumerate() {
                    let om = card_bit(c) | card_bit(d);
                    if om & (hm | board) != 0 {
                        continue;
                    }
                    let r = reach_o[oi] as f64;
                    let u = match node.kind {
                        NodeKind::Fold(f) if f as usize == p => -(node.contrib[p] as f64),
                        NodeKind::Fold(f) => node.contrib[f as usize] as f64 - rake,
                        _ => {
                            let (hs, os) = (eval_mask(board | hm), eval_mask(board | om));
                            let c = node.contrib[0] as f64;
                            if hs > os {
                                c - rake
                            } else if hs < os {
                                -c
                            } else {
                                -rake / 2.0
                            }
                        }
                    };
                    v += r * u;
                }
                v
            })
            .collect()
    }

    #[test]
    fn terminal_values_match_brute_force() {
        for rake in [0.0, 0.05] {
            let cfg = PostflopConfig {
                board: "Qh9h4c4d2s".into(),
                oop_range: "any".into(),
                ip_range: "22+,A2+,K9+,QT+,J9s+,T8s+,97s+,87s,76s".into(),
                rake,
                rake_cap: 3.0,
                ..PostflopConfig::default()
            };
            let g = PostflopGame::new(cfg).unwrap();
            let mut rng = Rng::new(7);
            let mut checked = [0, 0];
            for (idx, node) in g.nodes.iter().enumerate() {
                match node.kind {
                    NodeKind::Fold(_) => checked[0] += 1,
                    NodeKind::Showdown => checked[1] += 1,
                    _ => continue,
                }
                for p in 0..2 {
                    let reach_o: Vec<f32> = (0..g.num_hands(1 - p)).map(|_| rng.unit_f32()).collect();
                    let fast = g.recurse(
                        idx,
                        p,
                        &vec![1.0; g.num_hands(p)],
                        &reach_o,
                        Dealt::default(),
                        Mode::Average,
                    );
                    let slow = brute(&g, node, p, &reach_o);
                    for (h, (&f, &s)) in fast.iter().zip(&slow).enumerate() {
                        assert!(
                            (f as f64 - s).abs() < 1e-3 * (1.0 + s.abs()),
                            "rake {rake} node {idx} player {p} hand {h}: fast {f} slow {s}"
                        );
                    }
                }
            }
            assert!(checked[0] > 0 && checked[1] > 0);
        }
    }
}
