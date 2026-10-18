//! Betting-tree construction for a heads-up postflop spot.
//!
//! The game tree is stored as a flat array of [`Node`]s whose children are
//! contiguous. Each chance node (turn or river card) gets its own copy of the
//! following street's betting tree, so every player node owns a distinct slice
//! of the regret/strategy storage. With isomorphism enabled, suit-symmetric
//! cards are not expanded; [`IsoEntry`]s map them to a representative card.

use crate::card::{card_to_string, make_card, rank, suit, Card};
use crate::postflop::config::{BetSize, ParsedStreet, PostflopConfig};

pub const NONE: u32 = u32::MAX;
const EPS: f64 = 1e-9;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    /// The root has no incoming action.
    None,
    Fold,
    Check,
    Call,
    /// Bet, raise and all-in carry the player's total contribution on the street after the action.
    Bet(f32),
    Raise(f32),
    AllIn(f32),
    Deal(Card),
}

impl Action {
    pub fn label(&self) -> String {
        match *self {
            Action::None => "root".into(),
            Action::Fold => "Fold".into(),
            Action::Check => "Check".into(),
            Action::Call => "Call".into(),
            Action::Bet(a) => format!("Bet {}", fmt_chips(a)),
            Action::Raise(a) => format!("Raise {}", fmt_chips(a)),
            Action::AllIn(a) => format!("All-in {}", fmt_chips(a)),
            Action::Deal(c) => card_to_string(c),
        }
    }

    /// Short token used to type a line: `f`, `x`, `c`, `b3.3`, `r10`, `a`, or a card.
    pub fn token(&self) -> String {
        match *self {
            Action::None => String::new(),
            Action::Fold => "f".into(),
            Action::Check => "x".into(),
            Action::Call => "c".into(),
            Action::Bet(a) => format!("b{}", fmt_chips(a)),
            Action::Raise(a) => format!("r{}", fmt_chips(a)),
            Action::AllIn(_) => "a".into(),
            Action::Deal(c) => card_to_string(c),
        }
    }
}

pub fn fmt_chips(a: f32) -> String {
    let s = format!("{:.2}", a);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NodeKind {
    Player(u8),
    Chance,
    /// The given player folded.
    Fold(u8),
    Showdown,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    /// Action that leads to this node.
    pub action: Action,
    /// Board cards at this node (before the deal, for chance nodes).
    pub board_len: u8,
    pub first_child: u32,
    pub num_children: u32,
    /// Total chips each player has put in, each counting half of the starting pot.
    pub contrib: [f32; 2],
    /// Offset of this node's `actions x hands` block in the storage (player nodes).
    pub storage: u64,
    /// Index among player nodes (player nodes), used for per-node data.
    pub pidx: u32,
    /// Index into the isomorphism table (chance nodes), or [`NONE`].
    pub iso: u32,
}

impl Node {
    pub fn children(&self) -> std::ops::Range<usize> {
        self.first_child as usize..(self.first_child + self.num_children) as usize
    }

    pub fn pot(&self) -> f32 {
        self.contrib[0] + self.contrib[1]
    }
}

/// A chance outcome that was not expanded because it mirrors a representative
/// card under the suit swap `pair` (see [`suit_pair`]).
#[derive(Clone, Copy, Debug)]
pub struct IsoEntry {
    pub card: Card,
    pub rep_child: u32,
    pub pair: u8,
}

/// Index of the unordered suit pair `{a, b}` in `0..6`.
pub fn suit_pair(a: u8, b: u8) -> usize {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    match (a, b) {
        (0, 1) => 0,
        (0, 2) => 1,
        (0, 3) => 2,
        (1, 2) => 3,
        (1, 3) => 4,
        _ => 5,
    }
}

pub fn pair_suits(pair: usize) -> (u8, u8) {
    [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)][pair]
}

pub fn swap_card(c: Card, pair: usize) -> Card {
    let (a, b) = pair_suits(pair);
    let s = suit(c);
    let s2 = if s == a {
        b
    } else if s == b {
        a
    } else {
        s
    };
    make_card(rank(c), s2)
}

pub fn board_invariant(board: &[Card], pair: usize) -> bool {
    board.iter().all(|&c| board.contains(&swap_card(c, pair)))
}

#[derive(Clone)]
struct State {
    board: Vec<Card>,
    contrib: [f64; 2],
    street_contrib: [f64; 2],
    to_act: usize,
    raises: u32,
    last_inc: f64,
    prev_aggressor: Option<usize>,
    aggressor: Option<usize>,
    first_action: bool,
}

enum Next {
    Decision(State),
    Fold(usize, State),
    EndStreet(State),
}

pub struct TreeBuilder<'a> {
    cfg: &'a PostflopConfig,
    streets: Vec<ParsedStreet>,
    num_hands: [usize; 2],
    /// Suit swaps under which both ranges and the starting board are symmetric.
    sym_pairs: [bool; 6],
    pub nodes: Vec<Node>,
    pub iso_tables: Vec<Vec<IsoEntry>>,
    pub storage_len: u64,
    pub player_nodes: u32,
}

impl<'a> TreeBuilder<'a> {
    pub fn new(cfg: &'a PostflopConfig, num_hands: [usize; 2], sym_pairs: [bool; 6]) -> Result<Self, String> {
        let streets = vec![cfg.flop.parsed()?, cfg.turn.parsed()?, cfg.river.parsed()?];
        Ok(TreeBuilder {
            cfg,
            streets,
            num_hands,
            sym_pairs,
            nodes: Vec::new(),
            iso_tables: Vec::new(),
            storage_len: 0,
            player_nodes: 0,
        })
    }

    pub fn build(&mut self, board: &[Card]) {
        let half = self.cfg.pot / 2.0;
        let root = State {
            board: board.to_vec(),
            contrib: [half, half],
            street_contrib: [0.0, 0.0],
            to_act: 0,
            raises: 0,
            last_inc: 0.0,
            prev_aggressor: None,
            aggressor: None,
            first_action: true,
        };
        self.nodes.push(placeholder());
        self.place(0, Next::Decision(root), Action::None);
    }

    fn remaining(&self, st: &State, p: usize) -> f64 {
        (self.cfg.stack - (st.contrib[p] - self.cfg.pot / 2.0)).max(0.0)
    }

    fn place(&mut self, slot: usize, next: Next, action: Action) {
        match next {
            Next::Fold(folder, st) => {
                self.nodes[slot] = Node {
                    kind: NodeKind::Fold(folder as u8),
                    action,
                    board_len: st.board.len() as u8,
                    first_child: 0,
                    num_children: 0,
                    contrib: [st.contrib[0] as f32, st.contrib[1] as f32],
                    storage: 0,
                    pidx: NONE,
                    iso: NONE,
                };
            }
            Next::EndStreet(st) if st.board.len() == 5 => {
                self.nodes[slot] = Node {
                    kind: NodeKind::Showdown,
                    action,
                    board_len: 5,
                    first_child: 0,
                    num_children: 0,
                    contrib: [st.contrib[0] as f32, st.contrib[1] as f32],
                    storage: 0,
                    pidx: NONE,
                    iso: NONE,
                };
            }
            Next::EndStreet(st) => self.place_chance(slot, st, action),
            Next::Decision(st) => self.place_decision(slot, st, action),
        }
    }

    fn place_chance(&mut self, slot: usize, st: State, action: Action) {
        let valid_pairs: Vec<usize> = if self.cfg.isomorphism {
            (0..6).filter(|&p| self.sym_pairs[p] && board_invariant(&st.board, p)).collect()
        } else {
            Vec::new()
        };
        let mut reps: Vec<Card> = Vec::new();
        let mut isos: Vec<(Card, Card, usize)> = Vec::new();
        for c in 0..52u8 {
            if st.board.contains(&c) {
                continue;
            }
            // Representative: the lowest card reachable by a valid suit swap.
            let rep = valid_pairs
                .iter()
                .map(|&p| (swap_card(c, p), p))
                .filter(|&(sc, _)| sc < c)
                .min_by_key(|&(sc, _)| sc);
            match rep {
                Some((r, p)) => isos.push((c, r, p)),
                None => reps.push(c),
            }
        }

        let all_in = self.remaining(&st, 0) <= EPS || self.remaining(&st, 1) <= EPS;
        let first = self.nodes.len();
        let iso_idx = if isos.is_empty() {
            NONE
        } else {
            let table = isos
                .iter()
                .map(|&(card, rep, pair)| IsoEntry {
                    card,
                    rep_child: (first + reps.iter().position(|&x| x == rep).unwrap()) as u32,
                    pair: pair as u8,
                })
                .collect();
            self.iso_tables.push(table);
            (self.iso_tables.len() - 1) as u32
        };
        self.nodes[slot] = Node {
            kind: NodeKind::Chance,
            action,
            board_len: st.board.len() as u8,
            first_child: first as u32,
            num_children: reps.len() as u32,
            contrib: [st.contrib[0] as f32, st.contrib[1] as f32],
            storage: 0,
            pidx: NONE,
            iso: iso_idx,
        };
        self.nodes.extend((0..reps.len()).map(|_| placeholder()));
        for (i, &card) in reps.iter().enumerate() {
            let mut next = st.clone();
            next.board.push(card);
            next.street_contrib = [0.0, 0.0];
            next.to_act = 0;
            next.raises = 0;
            next.last_inc = 0.0;
            next.prev_aggressor = st.aggressor;
            next.aggressor = None;
            next.first_action = true;
            let n = if all_in { Next::EndStreet(next) } else { Next::Decision(next) };
            self.place(first + i, n, Action::Deal(card));
        }
    }

    fn place_decision(&mut self, slot: usize, st: State, action: Action) {
        let actions = self.actions(&st);
        let p = st.to_act;
        let first = self.nodes.len();
        self.nodes[slot] = Node {
            kind: NodeKind::Player(p as u8),
            action,
            board_len: st.board.len() as u8,
            first_child: first as u32,
            num_children: actions.len() as u32,
            contrib: [st.contrib[0] as f32, st.contrib[1] as f32],
            storage: self.storage_len,
            pidx: self.player_nodes,
            iso: NONE,
        };
        self.player_nodes += 1;
        self.storage_len += (actions.len() * self.num_hands[p]) as u64;
        self.nodes.extend((0..actions.len()).map(|_| placeholder()));
        for (i, &a) in actions.iter().enumerate() {
            let next = self.apply(&st, a);
            self.place(first + i, next, a);
        }
    }

    fn apply(&self, st: &State, a: Action) -> Next {
        let p = st.to_act;
        let o = 1 - p;
        let mut s = st.clone();
        s.first_action = false;
        match a {
            Action::Fold => Next::Fold(p, s),
            Action::Check => {
                if p == 0 {
                    s.to_act = 1;
                    Next::Decision(s)
                } else {
                    Next::EndStreet(s)
                }
            }
            Action::Call => {
                let call = (st.street_contrib[o] - st.street_contrib[p]).min(self.remaining(st, p));
                s.contrib[p] += call;
                s.street_contrib[p] += call;
                Next::EndStreet(s)
            }
            Action::Bet(to) | Action::Raise(to) | Action::AllIn(to) => {
                let to = to as f64;
                let to =
                    if let Action::AllIn(_) = a { st.street_contrib[p] + self.remaining(st, p) } else { to };
                s.contrib[p] += to - st.street_contrib[p];
                s.street_contrib[p] = to;
                s.last_inc = to - st.street_contrib[o];
                s.raises += 1;
                s.aggressor = Some(p);
                s.to_act = o;
                Next::Decision(s)
            }
            Action::None | Action::Deal(_) => unreachable!(),
        }
    }

    fn actions(&self, st: &State) -> Vec<Action> {
        let p = st.to_act;
        let o = 1 - p;
        let pot = st.contrib[0] + st.contrib[1];
        let rem = [self.remaining(st, 0), self.remaining(st, 1)];
        let facing = st.street_contrib[o] - st.street_contrib[p];
        let street = &self.streets[st.board.len() - 3];
        let max_to = st.street_contrib[p] + rem[p];
        let mut targets: Vec<f64> = Vec::new();
        let mut acts = Vec::new();

        if facing <= EPS {
            acts.push(Action::Check);
            if rem[p] > EPS && rem[o] > EPS {
                let sizes = match (&street.donk, p, st.first_action, st.prev_aggressor) {
                    (Some(donk), 0, true, Some(1)) => donk,
                    _ => &street.bet[p],
                };
                for s in sizes {
                    let to = match *s {
                        BetSize::Pot(f) => f * pot,
                        BetSize::AllIn => max_to,
                        BetSize::Mult(_) => continue,
                    };
                    targets.push(to);
                }
                if !sizes.is_empty() && rem[p] <= self.cfg.add_allin_spr * pot + EPS {
                    targets.push(max_to);
                }
            }
        } else {
            acts.push(Action::Fold);
            acts.push(Action::Call);
            let can_raise = st.raises <= self.cfg.max_raises;
            if can_raise && rem[p] > facing + EPS && rem[o] > EPS {
                let opp_to = st.street_contrib[o];
                let min_to = opp_to + st.last_inc;
                let pot_after_call = pot + facing;
                for s in &street.raise[p] {
                    let to = match *s {
                        BetSize::Pot(f) => opp_to + f * pot_after_call,
                        BetSize::Mult(m) => m * opp_to,
                        BetSize::AllIn => max_to,
                    };
                    targets.push(to.max(min_to));
                }
                if !street.raise[p].is_empty() && rem[p] <= self.cfg.add_allin_spr * pot_after_call + EPS {
                    targets.push(max_to);
                }
            }
        }

        let mut chosen: Vec<f64> = Vec::new();
        for to in targets {
            let mut to = to.min(max_to);
            let added = to - st.street_contrib[p];
            if added >= self.cfg.allin_threshold * rem[p] {
                to = max_to;
            }
            if to - st.street_contrib[p] <= EPS {
                continue;
            }
            if !chosen.iter().any(|&c| (c - to).abs() < 1e-6 * (1.0 + to)) {
                chosen.push(to);
            }
        }
        chosen.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for to in chosen {
            let act = if (to - max_to).abs() < 1e-9 {
                Action::AllIn(to as f32)
            } else if facing <= EPS {
                Action::Bet(to as f32)
            } else {
                Action::Raise(to as f32)
            };
            acts.push(act);
        }
        acts
    }
}

fn placeholder() -> Node {
    Node {
        kind: NodeKind::Showdown,
        action: Action::None,
        board_len: 0,
        first_child: 0,
        num_children: 0,
        contrib: [0.0, 0.0],
        storage: 0,
        pidx: NONE,
        iso: NONE,
    }
}
