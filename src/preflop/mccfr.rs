//! External-sampling Monte Carlo CFR for the preflop game.
//!
//! Each iteration deals real cards to every seat (so card removal is exact),
//! draws a few random boards shared by all showdowns of that iteration, and
//! traverses the tree once per seat: the traverser explores all of its
//! actions, everyone else samples one from their current strategy. Regrets and
//! cumulative strategies live in shared atomics updated lock-free by all
//! threads (as in Pluribus). Linear CFR is approximated by periodically
//! discounting everything by `k / (k + 1)` early in the run.
//!
//! Information sets are (decision node, 169-class starting hand).

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

use rayon::prelude::*;

use crate::card::{card_bit, combo_index, Card, Rng};
use crate::eval::eval_mask;
use crate::preflop::config::{PreflopConfig, MAX_PLAYERS};
use crate::preflop::tree::{PKind, PNode, PreflopTree};
use crate::range::{combo_class, NUM_CLASSES};

const MAX_ACTIONS: usize = 8;

#[inline]
fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Relaxed))
}

/// Racy add (lost updates are tolerated, as in Hogwild-style MCCFR).
#[inline]
fn add(a: &AtomicU32, x: f32) {
    a.store((load(a) + x).to_bits(), Relaxed);
}

pub struct PreflopSolver {
    pub cfg: PreflopConfig,
    pub tree: PreflopTree,
    regrets: Vec<AtomicU32>,
    strategy: Vec<AtomicU32>,
    pub iterations: AtomicU64,
}

/// Cards for one iteration plus cached showdown shares per set of players.
struct Deal {
    class: [usize; MAX_PLAYERS],
    /// Hand values on each sampled board, per seat.
    ranks: Vec<[u32; MAX_PLAYERS]>,
    raw: Vec<Option<[f32; MAX_PLAYERS]>>,
    realized: Vec<Option<[f32; MAX_PLAYERS]>>,
}

impl Deal {
    fn new(cfg: &PreflopConfig, rng: &mut Rng) -> Self {
        let n = cfg.players;
        let mut deck: [Card; 52] = std::array::from_fn(|i| i as Card);
        // Partial Fisher-Yates for the hole cards.
        for i in 0..2 * n {
            let j = i + rng.below((52 - i) as u32) as usize;
            deck.swap(i, j);
        }
        let mut class = [0usize; MAX_PLAYERS];
        let mut hand_mask = [0u64; MAX_PLAYERS];
        for p in 0..n {
            let (a, b) = (deck[2 * p], deck[2 * p + 1]);
            class[p] = combo_class(combo_index(a, b));
            hand_mask[p] = card_bit(a) | card_bit(b);
        }
        let rest = &mut deck[2 * n..];
        let mut ranks = Vec::with_capacity(cfg.equity_boards);
        for _ in 0..cfg.equity_boards {
            let len = rest.len();
            let mut board = 0u64;
            for i in 0..5 {
                let j = i + rng.below((len - i) as u32) as usize;
                rest.swap(i, j);
                board |= card_bit(rest[i]);
            }
            let mut r = [0u32; MAX_PLAYERS];
            for p in 0..n {
                r[p] = eval_mask(board | hand_mask[p]);
            }
            ranks.push(r);
        }
        Deal { class, ranks, raw: vec![None; 1 << n], realized: vec![None; 1 << n] }
    }

    /// Average pot share of each seat in `active` at showdown.
    fn raw_equity(&mut self, active: u8) -> [f32; MAX_PLAYERS] {
        if let Some(e) = self.raw[active as usize] {
            return e;
        }
        let mut eq = [0.0f32; MAX_PLAYERS];
        for r in &self.ranks {
            let mut best = 0;
            let mut winners = 0u8;
            for p in 0..MAX_PLAYERS {
                if active >> p & 1 == 0 {
                    continue;
                }
                if r[p] > best {
                    best = r[p];
                    winners = 1 << p;
                } else if r[p] == best {
                    winners |= 1 << p;
                }
            }
            let share = 1.0 / winners.count_ones() as f32;
            for p in 0..MAX_PLAYERS {
                if winners >> p & 1 == 1 {
                    eq[p] += share;
                }
            }
        }
        let inv = 1.0 / self.ranks.len() as f32;
        eq.iter_mut().for_each(|x| *x *= inv);
        self.raw[active as usize] = Some(eq);
        eq
    }

    /// Pot shares under the realization model for a flop terminal.
    fn realized_equity(&mut self, node: &PNode, cfg: &PreflopConfig) -> [f32; MAX_PLAYERS] {
        let active = node.active;
        // Positional shares depend only on who is in; hand-dependent ones also
        // on the flop's stack-to-pot ratio, so they are not cached.
        if cfg.eqr.is_none() {
            if let Some(e) = self.realized[active as usize] {
                return e;
            }
        }
        let exponent = cfg.realization_exponent as f32;
        let raw = self.raw_equity(active);
        let mut w = [0.0f32; MAX_PLAYERS];
        let mut sum = 0.0;
        for p in 0..MAX_PLAYERS {
            if active >> p & 1 == 1 {
                let r = match &cfg.eqr {
                    Some(m) => m.factor(node.eqr_mix, node.position[p], self.class[p]),
                    None => node.realization[p],
                };
                let e = if exponent == 1.0 { raw[p] } else { raw[p].powf(exponent) };
                w[p] = r * e;
                sum += w[p];
            }
        }
        if sum > 0.0 {
            w.iter_mut().for_each(|x| *x /= sum);
        } else {
            w = raw;
        }
        if cfg.eqr.is_none() {
            self.realized[active as usize] = Some(w);
        }
        w
    }
}

#[inline]
fn regret_matching(r: &[AtomicU32], out: &mut [f32]) {
    let mut sum = 0.0;
    for (o, a) in out.iter_mut().zip(r) {
        *o = load(a).max(0.0);
        sum += *o;
    }
    if sum > 0.0 {
        out.iter_mut().for_each(|x| *x /= sum);
    } else {
        let u = 1.0 / out.len() as f32;
        out.iter_mut().for_each(|x| *x = u);
    }
}

impl PreflopSolver {
    pub fn new(cfg: PreflopConfig) -> Result<Self, String> {
        let tree = PreflopTree::build(&cfg)?;
        if tree.nodes.iter().any(|n| n.num_children as usize > MAX_ACTIONS) {
            return Err("too many actions at a node".into());
        }
        let regrets = (0..tree.storage_len).map(|_| AtomicU32::new(0)).collect();
        let strategy = (0..tree.storage_len).map(|_| AtomicU32::new(0)).collect();
        Ok(PreflopSolver { cfg, tree, regrets, strategy, iterations: AtomicU64::new(0) })
    }

    /// Rebuilds a solver from a config and saved average strategy (for viewing).
    pub fn with_strategy(cfg: PreflopConfig, strategy: Vec<f32>, iterations: u64) -> Result<Self, String> {
        let mut s = Self::new(cfg)?;
        if strategy.len() != s.tree.storage_len {
            return Err("saved strategy does not match the tree".into());
        }
        s.strategy = strategy.into_iter().map(|x| AtomicU32::new(x.to_bits())).collect();
        s.iterations.store(iterations, Relaxed);
        Ok(s)
    }

    /// Runs `count` iterations spread over the rayon pool.
    pub fn run(&self, count: u64, seed: u64) {
        let threads = rayon::current_num_threads().max(1) as u64;
        let per = count.div_ceil(threads);
        (0..threads).into_par_iter().for_each(|t| {
            let mut rng = Rng::new(seed ^ (t + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let todo = per.min(count.saturating_sub(t * per));
            for _ in 0..todo {
                self.iterate(&mut rng);
            }
        });
        self.iterations.fetch_add(count, Relaxed);
    }

    fn iterate(&self, rng: &mut Rng) {
        let mut deal = Deal::new(&self.cfg, rng);
        for p in 0..self.cfg.players {
            self.traverse(0, p, &mut deal, rng);
        }
    }

    fn payoff(&self, node: &PNode, p: usize, deal: &mut Deal) -> f32 {
        if node.active >> p & 1 == 0 {
            return -node.invested[p];
        }
        let share = match node.kind {
            PKind::Fold(_) => return node.pot - node.invested[p],
            PKind::AllIn => deal.raw_equity(node.active)[p],
            PKind::Flop => deal.realized_equity(node, &self.cfg)[p],
            PKind::Decision(_) => unreachable!(),
        };
        let rake = self.cfg.rake_amount(node.pot as f64) as f32;
        share * (node.pot - rake) - node.invested[p]
    }

    fn traverse(&self, idx: usize, p: usize, deal: &mut Deal, rng: &mut Rng) -> f32 {
        let node = &self.tree.nodes[idx];
        let PKind::Decision(q) = node.kind else {
            return self.payoff(node, p, deal);
        };
        let q = q as usize;
        let na = node.num_children as usize;
        let base = node.storage as usize + deal.class[q] * na;
        let mut sigma = [0.0f32; MAX_ACTIONS];
        regret_matching(&self.regrets[base..base + na], &mut sigma[..na]);
        let first = node.first_child as usize;
        if q == p {
            let mut vals = [0.0f32; MAX_ACTIONS];
            let mut ev = 0.0;
            for a in 0..na {
                vals[a] = self.traverse(first + a, p, deal, rng);
                ev += sigma[a] * vals[a];
            }
            for a in 0..na {
                add(&self.regrets[base + a], vals[a] - ev);
            }
            ev
        } else {
            for a in 0..na {
                add(&self.strategy[base + a], sigma[a]);
            }
            let u = rng.unit_f32();
            let mut acc = 0.0;
            let mut pick = na - 1;
            for (a, &s) in sigma[..na].iter().enumerate() {
                acc += s;
                if u < acc {
                    pick = a;
                    break;
                }
            }
            self.traverse(first + pick, p, deal, rng)
        }
    }

    /// Multiplies all regrets and cumulative strategies by `factor` (linear-CFR discounting).
    pub fn discount(&self, factor: f32) {
        self.regrets.par_iter().chain(self.strategy.par_iter()).for_each(|a| {
            a.store((load(a) * factor).to_bits(), Relaxed);
        });
    }

    /// Average strategy of a decision node for one hand class.
    pub fn average(&self, node: &PNode, class: usize) -> Vec<f32> {
        let na = node.num_children as usize;
        let base = node.storage as usize + class * na;
        let raw: Vec<f32> = self.strategy[base..base + na].iter().map(load).collect();
        let sum: f32 = raw.iter().sum();
        if sum > 0.0 {
            raw.iter().map(|x| x / sum).collect()
        } else {
            vec![1.0 / na as f32; na]
        }
    }

    /// Normalized average strategies of all decision nodes (for saving).
    pub fn strategy_vec(&self) -> Vec<f32> {
        let mut out: Vec<f32> = self.strategy.iter().map(load).collect();
        for node in &self.tree.nodes {
            if let PKind::Decision(_) = node.kind {
                let na = node.num_children as usize;
                for c in 0..NUM_CLASSES {
                    let base = node.storage as usize + c * na;
                    let avg = self.average(node, c);
                    out[base..base + na].copy_from_slice(&avg);
                }
            }
        }
        out
    }
}
