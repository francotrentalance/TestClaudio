//! Preflop action tree for 2-6 players with equal stacks.
//!
//! Seats are numbered in preflop action order (seat 0 is UTG at 6-max, the SB
//! heads-up). Terminals are folds, all-in showdowns and pots that see a flop.

use crate::preflop::config::{MAX_PLAYERS, PreflopConfig};
use crate::range::NUM_CLASSES;

const EPS: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PAction {
    None,
    Fold,
    Check,
    Call,
    /// Raise to the given total (bb).
    Raise(f32),
    AllIn(f32),
}

impl PAction {
    pub fn label(&self) -> String {
        match *self {
            PAction::None => "root".into(),
            PAction::Fold => "Fold".into(),
            PAction::Check => "Check".into(),
            PAction::Call => "Call".into(),
            PAction::Raise(x) => format!("Raise {}", crate::postflop::tree::fmt_chips(x)),
            PAction::AllIn(x) => format!("All-in {}", crate::postflop::tree::fmt_chips(x)),
        }
    }

    pub fn code(&self) -> &'static str {
        match self {
            PAction::Fold => "F",
            PAction::Check => "X",
            PAction::Call => "C",
            PAction::Raise(_) => "R",
            PAction::AllIn(_) => "A",
            PAction::None => "?",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PKind {
    /// Decision of the given seat.
    Decision(u8),
    /// Everyone else folded; the given seat wins.
    Fold(u8),
    /// All remaining players are all-in: showdown on a random board.
    AllIn,
    /// Betting closed with stacks behind: valued with the realization model.
    Flop,
}

#[derive(Clone, Debug)]
pub struct PNode {
    pub kind: PKind,
    pub action: PAction,
    pub parent: u32,
    pub first_child: u32,
    pub num_children: u32,
    /// Decision nodes: offset of the `169 x actions` block in the storage.
    pub storage: u32,
    /// Bitmask of players still in the hand.
    pub active: u8,
    pub pot: f32,
    /// Chips each player has put in, antes included.
    pub invested: [f32; MAX_PLAYERS],
    /// Flop terminals: realization factor of each active player.
    pub realization: [f32; MAX_PLAYERS],
}

impl PNode {
    pub fn children(&self) -> std::ops::Range<usize> {
        self.first_child as usize..(self.first_child + self.num_children) as usize
    }
}

#[derive(Clone)]
struct St {
    contrib: [f64; MAX_PLAYERS],
    folded: [bool; MAX_PLAYERS],
    needs: [bool; MAX_PLAYERS],
    voluntary: [bool; MAX_PLAYERS],
    to_act: usize,
    level: u32,
    raise_to: f64,
    last_inc: f64,
    last_raiser: Option<usize>,
    callers: u32,
    cold_callers: u32,
    limpers: u32,
}

pub struct PreflopTree {
    pub nodes: Vec<PNode>,
    pub storage_len: usize,
    pub decisions: usize,
}

struct Builder<'a> {
    cfg: &'a PreflopConfig,
    nodes: Vec<PNode>,
    storage_len: usize,
    decisions: usize,
}

impl PreflopTree {
    pub fn build(cfg: &PreflopConfig) -> Result<Self, String> {
        cfg.validate()?;
        let n = cfg.players;
        let mut contrib = [0.0; MAX_PLAYERS];
        contrib[cfg.sb_seat()] = cfg.sb;
        contrib[cfg.bb_seat()] = cfg.bb;
        let mut needs = [false; MAX_PLAYERS];
        needs[..n].iter_mut().for_each(|x| *x = true);
        let mut folded = [true; MAX_PLAYERS];
        folded[..n].iter_mut().for_each(|x| *x = false);
        let root = St {
            contrib,
            folded,
            needs,
            voluntary: [false; MAX_PLAYERS],
            to_act: 0,
            level: 0,
            raise_to: cfg.bb,
            last_inc: cfg.bb,
            last_raiser: None,
            callers: 0,
            cold_callers: 0,
            limpers: 0,
        };
        let mut b = Builder { cfg, nodes: Vec::new(), storage_len: 0, decisions: 0 };
        b.nodes.push(placeholder());
        b.place(0, u32::MAX, root, PAction::None);
        if b.nodes.len() > u32::MAX as usize / 2 {
            return Err("preflop tree too large".into());
        }
        Ok(PreflopTree { nodes: b.nodes, storage_len: b.storage_len, decisions: b.decisions })
    }
}

impl Builder<'_> {
    fn is_allin(&self, st: &St, p: usize) -> bool {
        st.contrib[p] >= self.cfg.stack - EPS
    }

    fn place(&mut self, slot: usize, parent: u32, st: St, action: PAction) {
        let cfg = self.cfg;
        let n = cfg.players;
        let mut active = 0u8;
        for p in 0..n {
            if !st.folded[p] {
                active |= 1 << p;
            }
        }
        let invested = {
            let mut v = [0.0f32; MAX_PLAYERS];
            for p in 0..n {
                v[p] = (st.contrib[p] + cfg.ante) as f32;
            }
            v
        };
        let pot = invested.iter().sum::<f32>();
        let mut node = PNode {
            kind: PKind::Flop,
            action,
            parent,
            first_child: 0,
            num_children: 0,
            storage: 0,
            active,
            pot,
            invested,
            realization: [0.0; MAX_PLAYERS],
        };

        if active.count_ones() == 1 {
            node.kind = PKind::Fold(active.trailing_zeros() as u8);
            self.nodes[slot] = node;
            return;
        }
        let next = (0..n)
            .map(|k| (st.to_act + k) % n)
            .find(|&p| st.needs[p] && !st.folded[p] && !self.is_allin(&st, p));
        let Some(p) = next else {
            if st.raise_to >= cfg.stack - EPS {
                node.kind = PKind::AllIn;
            } else {
                node.kind = PKind::Flop;
                node.realization = self.realization(active);
            }
            self.nodes[slot] = node;
            return;
        };

        let mut st = st;
        st.to_act = p;
        let actions = self.actions(&st);
        let first = self.nodes.len();
        node.kind = PKind::Decision(p as u8);
        node.first_child = first as u32;
        node.num_children = actions.len() as u32;
        node.storage = self.storage_len as u32;
        self.storage_len += NUM_CLASSES * actions.len();
        self.decisions += 1;
        self.nodes[slot] = node;
        self.nodes.extend((0..actions.len()).map(|_| placeholder()));
        for (i, &a) in actions.iter().enumerate() {
            let child = self.apply(&st, a);
            self.place(first + i, slot as u32, child, a);
        }
    }

    fn realization(&self, active: u8) -> [f32; MAX_PLAYERS] {
        let cfg = self.cfg;
        let mut seats: Vec<usize> = (0..cfg.players).filter(|&p| active >> p & 1 == 1).collect();
        seats.sort_by_key(|&p| cfg.postflop_rank(p));
        let k = seats.len();
        let mut r = [0.0f32; MAX_PLAYERS];
        for (i, &p) in seats.iter().enumerate() {
            let t = if k > 1 { i as f64 / (k - 1) as f64 } else { 1.0 };
            r[p] = (cfg.realization_oop + (1.0 - cfg.realization_oop) * t) as f32;
        }
        r
    }

    fn actions(&self, st: &St) -> Vec<PAction> {
        let cfg = self.cfg;
        let p = st.to_act;
        let to_call = st.raise_to - st.contrib[p];
        let left = cfg.stack - st.contrib[p];
        let mut acts = Vec::new();
        if to_call > EPS {
            acts.push(PAction::Fold);
            let call_ok = if st.level == 0 {
                !cfg.pushfold && if p == cfg.sb_seat() { cfg.sb_limp } else { cfg.limp }
            } else if st.level == 1 {
                st.voluntary[p] || p == cfg.bb_seat() || st.cold_callers < cfg.max_cold_callers
            } else {
                st.voluntary[p] || cfg.cold_call_3bet
            };
            // Facing an all-in, calling is always allowed.
            if call_ok || st.raise_to >= cfg.stack - EPS {
                acts.push(PAction::Call);
            }
        } else {
            acts.push(PAction::Check);
        }

        let others_live = (0..cfg.players).any(|q| q != p && !st.folded[q] && !self.is_allin(st, q));
        if left > to_call + EPS && others_live {
            let max_to = cfg.stack;
            if cfg.pushfold {
                if st.level == 0 {
                    acts.push(PAction::AllIn(max_to as f32));
                }
            } else if st.level < cfg.max_raises {
                let mut to = match st.level {
                    0 => {
                        (if p == cfg.sb_seat() { cfg.sb_open } else { cfg.open })
                            + cfg.limp_raise_add * st.limpers as f64
                    }
                    1 => {
                        let raiser = st.last_raiser.unwrap_or(p);
                        let ip = cfg.postflop_rank(p) > cfg.postflop_rank(raiser);
                        let m = if ip { cfg.threebet_ip } else { cfg.threebet_oop };
                        st.raise_to * (m + cfg.caller_add * st.callers as f64)
                    }
                    _ => st.raise_to * (cfg.fourbet + cfg.caller_add * st.callers as f64),
                };
                to = to.max(st.raise_to + st.last_inc);
                if to >= cfg.allin_threshold * cfg.stack || to >= max_to - EPS {
                    acts.push(PAction::AllIn(max_to as f32));
                } else {
                    acts.push(PAction::Raise(to as f32));
                    if cfg.stack <= cfg.jam_ratio * to {
                        acts.push(PAction::AllIn(max_to as f32));
                    }
                }
            } else {
                acts.push(PAction::AllIn(max_to as f32));
            }
        }
        acts
    }

    fn apply(&self, st: &St, a: PAction) -> St {
        let cfg = self.cfg;
        let p = st.to_act;
        let mut s = st.clone();
        s.needs[p] = false;
        match a {
            PAction::Fold => s.folded[p] = true,
            PAction::Check => {}
            PAction::Call => {
                let was_voluntary = st.voluntary[p];
                s.contrib[p] = st.raise_to.min(cfg.stack);
                s.voluntary[p] = true;
                if st.level == 0 {
                    s.limpers += 1;
                } else {
                    s.callers += 1;
                    if st.level == 1 && !was_voluntary && p != cfg.bb_seat() {
                        s.cold_callers += 1;
                    }
                }
            }
            PAction::Raise(to) | PAction::AllIn(to) => {
                let to = to as f64;
                let inc = to - st.raise_to;
                s.last_inc = inc.max(st.last_inc);
                s.contrib[p] = to;
                s.raise_to = to;
                s.level += 1;
                s.last_raiser = Some(p);
                s.callers = 0;
                s.voluntary[p] = true;
                for q in 0..cfg.players {
                    if q != p && !s.folded[q] && s.contrib[q] < cfg.stack - EPS {
                        s.needs[q] = true;
                    }
                }
            }
            PAction::None => unreachable!(),
        }
        s.to_act = (p + 1) % cfg.players;
        s
    }
}

fn placeholder() -> PNode {
    PNode {
        kind: PKind::Flop,
        action: PAction::None,
        parent: u32::MAX,
        first_child: 0,
        num_children: 0,
        storage: 0,
        active: 0,
        pot: 0.0,
        invested: [0.0; MAX_PLAYERS],
        realization: [0.0; MAX_PLAYERS],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(cfg: &PreflopConfig) -> (usize, usize) {
        let t = PreflopTree::build(cfg).unwrap();
        (t.nodes.len(), t.decisions)
    }

    #[test]
    fn heads_up_pushfold_tree() {
        let cfg = PreflopConfig { players: 2, stack: 10.0, pushfold: true, ..Default::default() };
        let t = PreflopTree::build(&cfg).unwrap();
        // SB: fold | jam -> BB: fold | call.
        assert_eq!(t.decisions, 2);
        let root = &t.nodes[0];
        assert_eq!(root.kind, PKind::Decision(0));
        let acts: Vec<PAction> = root.children().map(|c| t.nodes[c].action).collect();
        assert_eq!(acts, vec![PAction::Fold, PAction::AllIn(10.0)]);
        let jam = &t.nodes[root.first_child as usize + 1];
        assert_eq!(jam.kind, PKind::Decision(1));
        let call = &t.nodes[jam.first_child as usize + 1];
        assert_eq!(call.kind, PKind::AllIn);
        assert_eq!(call.pot, 20.0);
        let fold = &t.nodes[jam.first_child as usize];
        assert_eq!(fold.kind, PKind::Fold(0));
        assert_eq!(fold.invested[1], 1.0);
    }

    #[test]
    fn six_max_trees_are_reasonable() {
        let (nodes, dec) = count(&PreflopConfig::default());
        assert!(dec > 100 && nodes < 2_000_000, "{nodes} nodes, {dec} decisions");
        let (_, dec_pf) = count(&PreflopConfig { pushfold: true, stack: 10.0, ..Default::default() });
        assert!(dec_pf < 400, "{dec_pf}");
    }

    #[test]
    fn walk_to_bb_is_a_fold_terminal() {
        let cfg = PreflopConfig::default();
        let t = PreflopTree::build(&cfg).unwrap();
        let mut idx = 0;
        for _ in 0..5 {
            let n = &t.nodes[idx];
            idx = n.first_child as usize; // fold is always the first action
            assert_eq!(t.nodes[idx].action, PAction::Fold);
        }
        assert_eq!(t.nodes[idx].kind, PKind::Fold(5));
        assert_eq!(t.nodes[idx].pot, 1.5);
    }
}
