//! A postflop spot instantiated for solving: hands, tree, showdown tables and storage.

use std::cell::UnsafeCell;

use rayon::prelude::*;

use crate::card::{
    Card, NUM_COMBOS, cards_mask, combo_cards, combo_index, combo_mask, parse_cards,
};
use crate::eval::eval_mask;
use crate::postflop::config::PostflopConfig;
use crate::postflop::tree::{
    IsoEntry, NONE, Node, TreeBuilder, board_invariant, pair_suits, swap_card,
};
use crate::range::Range;

/// A flat `f32` buffer that solver threads write into concurrently.
///
/// Safety contract: every player node owns a disjoint `actions x hands` slice,
/// and parallel recursion only ever splits at chance nodes, whose children are
/// disjoint subtrees. So no two threads touch the same slice at the same time.
pub(crate) struct SharedBuf(UnsafeCell<Vec<f32>>);

unsafe impl Sync for SharedBuf {}

impl SharedBuf {
    pub fn zeros(len: usize) -> Self {
        SharedBuf(UnsafeCell::new(vec![0.0; len]))
    }

    pub fn from_vec(v: Vec<f32>) -> Self {
        SharedBuf(UnsafeCell::new(v))
    }

    pub fn len(&self) -> usize {
        unsafe { (*self.0.get()).len() }
    }

    /// # Safety
    /// The caller must hold the only live reference to this range (see the type docs).
    #[allow(clippy::mut_from_ref)]
    pub unsafe fn slice_mut(&self, off: usize, len: usize) -> &mut [f32] {
        unsafe { &mut (&mut *self.0.get())[off..off + len] }
    }

    /// # Safety
    /// No thread may be writing this range concurrently.
    pub unsafe fn slice(&self, off: usize, len: usize) -> &[f32] {
        unsafe { &(&*self.0.get())[off..off + len] }
    }

    pub fn into_vec(self) -> Vec<f32> {
        self.0.into_inner()
    }
}

/// One hand in a [`RiverEval`] list; the cards are inlined for the showdown sweep.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct SortedHand {
    pub strength: u32,
    pub idx: u32,
    pub cards: [u8; 2],
}

/// Hands of both players sorted by strength on one complete river board.
pub struct RiverEval {
    /// Ascending by strength, only hands that do not conflict with the board.
    pub sorted: [Vec<SortedHand>; 2],
    /// For each entry of `sorted[p]`: number of opponent entries strictly weaker.
    pub weaker: [Vec<u32>; 2],
    /// For each entry of `sorted[p]`: index of the first opponent entry strictly stronger.
    pub stronger: [Vec<u32>; 2],
}

pub struct PostflopGame {
    pub config: PostflopConfig,
    pub board: Vec<Card>,
    /// Hole cards of every hand in each player's range (non-zero weight, no board conflict).
    pub hands: [Vec<(Card, Card)>; 2],
    pub initial_weights: [Vec<f32>; 2],
    /// Combo index -> hand index for each player, or [`NONE`].
    pub hand_index: [Vec<u32>; 2],
    /// Hand index -> index of the identical combo in the other player's list, or [`NONE`].
    pub same_hand: [Vec<u32>; 2],
    /// Card -> hand indices containing that card.
    pub card_hands: [Vec<Vec<u32>>; 2],
    /// Suit pair -> permutation of hand indices (empty when the swap is not a symmetry).
    pub swap_maps: [Vec<Vec<u32>>; 2],
    pub nodes: Vec<Node>,
    pub iso_tables: Vec<Vec<IsoEntry>>,
    river_slot: Vec<u32>,
    river_evals: Vec<RiverEval>,
    pub(crate) regrets: SharedBuf,
    pub(crate) strategy: SharedBuf,
    pub storage_len: usize,
    pub iteration: u32,
    /// Total weight of all non-conflicting (OOP hand, IP hand) pairs.
    pub norm: f64,
    /// True once `strategy` holds the normalized average strategy and regrets are dropped.
    pub finalized: bool,
}

/// Memory and size figures for a tree, available before allocating storage.
#[derive(Clone, Copy, Debug)]
pub struct TreeStats {
    pub nodes: usize,
    pub player_nodes: usize,
    pub chance_nodes: usize,
    pub storage_floats: u64,
    pub hands: [usize; 2],
}

impl TreeStats {
    /// Bytes needed to solve (regrets + cumulative strategy) plus the node array.
    pub fn solve_bytes(&self) -> u64 {
        self.storage_floats * 8 + (self.nodes * std::mem::size_of::<Node>()) as u64
    }
}

struct Setup {
    board: Vec<Card>,
    hands: [Vec<(Card, Card)>; 2],
    weights: [Vec<f32>; 2],
    hand_index: [Vec<u32>; 2],
    sym_pairs: [bool; 6],
}

fn setup(config: &PostflopConfig) -> Result<Setup, String> {
    let board = parse_cards(&config.board)?;
    if !(3..=5).contains(&board.len()) {
        return Err(format!("board must have 3 to 5 cards, got {}", board.len()));
    }
    if config.pot <= 0.0 || config.stack < 0.0 {
        return Err("pot must be positive and stack non-negative".into());
    }
    let ranges = [Range::parse(&config.oop_range)?, Range::parse(&config.ip_range)?];
    let board_mask = cards_mask(&board);
    let mut hands: [Vec<(Card, Card)>; 2] = [Vec::new(), Vec::new()];
    let mut weights: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    let mut hand_index = [vec![NONE; NUM_COMBOS], vec![NONE; NUM_COMBOS]];
    for p in 0..2 {
        for c in 0..NUM_COMBOS {
            let w = ranges[p].weights[c];
            if w > 0.0 && combo_mask(c) & board_mask == 0 {
                hand_index[p][c] = hands[p].len() as u32;
                hands[p].push(combo_cards(c));
                weights[p].push(w);
            }
        }
        if hands[p].is_empty() {
            return Err(format!("{} range is empty on this board", ["OOP", "IP"][p]));
        }
    }
    let mut sym_pairs = [false; 6];
    for (pair, sym) in sym_pairs.iter_mut().enumerate() {
        *sym = board_invariant(&board, pair)
            && ranges.iter().all(|r| {
                (0..NUM_COMBOS).all(|c| {
                    let (a, b) = combo_cards(c);
                    let s = combo_index(swap_card(a, pair), swap_card(b, pair));
                    r.weights[c] == r.weights[s]
                })
            });
    }
    Ok(Setup { board, hands, weights, hand_index, sym_pairs })
}

/// Builds the tree without allocating storage, to report its size.
pub fn tree_stats(config: &PostflopConfig) -> Result<TreeStats, String> {
    let s = setup(config)?;
    let num_hands = [s.hands[0].len(), s.hands[1].len()];
    let mut tb = TreeBuilder::new(config, num_hands, s.sym_pairs)?;
    tb.build(&s.board);
    Ok(stats_of(&tb.nodes, tb.storage_len, num_hands))
}

fn stats_of(nodes: &[Node], storage: u64, hands: [usize; 2]) -> TreeStats {
    use crate::postflop::tree::NodeKind;
    TreeStats {
        nodes: nodes.len(),
        player_nodes: nodes.iter().filter(|n| matches!(n.kind, NodeKind::Player(_))).count(),
        chance_nodes: nodes.iter().filter(|n| matches!(n.kind, NodeKind::Chance)).count(),
        storage_floats: storage,
        hands,
    }
}

impl PostflopGame {
    /// Builds the game and allocates solver storage.
    pub fn new(config: PostflopConfig) -> Result<Self, String> {
        Self::build(config, None)
    }

    /// Rebuilds a game from its config and attaches a saved average strategy.
    pub fn with_strategy(config: PostflopConfig, strategy: Vec<f32>) -> Result<Self, String> {
        Self::build(config, Some(strategy))
    }

    fn build(config: PostflopConfig, strategy: Option<Vec<f32>>) -> Result<Self, String> {
        let Setup { board, hands, weights, hand_index, sym_pairs } = setup(&config)?;
        let num_hands = [hands[0].len(), hands[1].len()];
        let mut tb = TreeBuilder::new(&config, num_hands, sym_pairs)?;
        tb.build(&board);
        let storage_len = tb.storage_len as usize;
        let nodes = std::mem::take(&mut tb.nodes);
        let iso_tables = std::mem::take(&mut tb.iso_tables);
        drop(tb);

        let same_hand = [0, 1].map(|p| {
            hands[p]
                .iter()
                .map(|&(a, b)| hand_index[1 - p][combo_index(a, b)])
                .collect::<Vec<u32>>()
        });
        let card_hands = [0, 1].map(|p| {
            let mut v = vec![Vec::new(); 52];
            for (i, &(a, b)) in hands[p].iter().enumerate() {
                v[a as usize].push(i as u32);
                v[b as usize].push(i as u32);
            }
            v
        });
        let swap_maps = [0, 1].map(|p| {
            (0..6)
                .map(|pair| {
                    if !sym_pairs[pair] {
                        return Vec::new();
                    }
                    hands[p]
                        .iter()
                        .map(|&(a, b)| {
                            hand_index[p][combo_index(swap_card(a, pair), swap_card(b, pair))]
                        })
                        .collect()
                })
                .collect::<Vec<Vec<u32>>>()
        });
        debug_assert!(swap_maps.iter().flatten().flatten().all(|&h| h != NONE));

        let (river_slot, river_evals) = build_river_evals(&board, &hands);

        let mut norm = 0.0f64;
        for (h, &(a, b)) in hands[0].iter().enumerate() {
            let m = crate::card::card_bit(a) | crate::card::card_bit(b);
            let opp: f64 = hands[1]
                .iter()
                .zip(&weights[1])
                .filter(|&(&(c, d), _)| (crate::card::card_bit(c) | crate::card::card_bit(d)) & m == 0)
                .map(|(_, &w)| w as f64)
                .sum();
            norm += weights[0][h] as f64 * opp;
        }
        if norm <= 0.0 {
            return Err("the ranges have no non-conflicting hand pairs".into());
        }

        let (regrets, strategy, finalized) = match strategy {
            Some(s) => {
                if s.len() != storage_len {
                    return Err(format!(
                        "saved strategy has {} entries but the tree needs {storage_len}",
                        s.len()
                    ));
                }
                (SharedBuf::zeros(0), SharedBuf::from_vec(s), true)
            }
            None => (SharedBuf::zeros(storage_len), SharedBuf::zeros(storage_len), false),
        };

        Ok(PostflopGame {
            config,
            board,
            hands,
            initial_weights: weights,
            hand_index,
            same_hand,
            card_hands,
            swap_maps,
            nodes,
            iso_tables,
            river_slot,
            river_evals,
            regrets,
            strategy,
            storage_len,
            iteration: 0,
            norm,
            finalized,
        })
    }

    pub fn stats(&self) -> TreeStats {
        stats_of(&self.nodes, self.storage_len as u64, [self.hands[0].len(), self.hands[1].len()])
    }

    pub fn num_hands(&self, p: usize) -> usize {
        self.hands[p].len()
    }

    /// Showdown table for the board formed by the initial board plus `dealt`.
    pub fn river_eval(&self, dealt: &[Card]) -> &RiverEval {
        &self.river_evals[self.river_slot[river_key(dealt)] as usize]
    }

    /// Average strategy of a player node as `actions x hands`, normalized per hand.
    pub fn average_strategy(&self, node: &Node) -> Vec<f32> {
        let crate::postflop::tree::NodeKind::Player(p) = node.kind else {
            return Vec::new();
        };
        let nh = self.hands[p as usize].len();
        let na = node.num_children as usize;
        let raw = unsafe { self.strategy.slice(node.storage as usize, na * nh) };
        if self.finalized {
            return raw.to_vec();
        }
        normalize_strategy(raw, na, nh)
    }

    /// Replaces cumulative strategies with normalized averages and frees regrets.
    pub fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        let nodes = &self.nodes;
        let hands = &self.hands;
        let mut strat = std::mem::replace(&mut self.strategy, SharedBuf::zeros(0)).into_vec();
        // Player nodes own contiguous, disjoint blocks in node order.
        let mut blocks: Vec<(usize, usize, usize)> = nodes
            .iter()
            .filter_map(|n| match n.kind {
                crate::postflop::tree::NodeKind::Player(p) => {
                    Some((n.storage as usize, n.num_children as usize, hands[p as usize].len()))
                }
                _ => None,
            })
            .collect();
        blocks.sort_unstable();
        let mut rest: &mut [f32] = &mut strat;
        let mut pos = 0;
        let mut slices = Vec::with_capacity(blocks.len());
        for &(off, na, nh) in &blocks {
            let (_, tail) = rest.split_at_mut(off - pos);
            let (blk, tail) = tail.split_at_mut(na * nh);
            slices.push((blk, na, nh));
            rest = tail;
            pos = off + na * nh;
        }
        slices.into_par_iter().for_each(|(blk, na, nh)| {
            let norm = normalize_strategy(blk, na, nh);
            blk.copy_from_slice(&norm);
        });
        self.strategy = SharedBuf::from_vec(strat);
        self.regrets = SharedBuf::zeros(0);
        self.finalized = true;
    }

    /// Normalized average strategies for all player nodes (for saving).
    pub fn strategy_vec(&mut self) -> Vec<f32> {
        self.finalize();
        unsafe { self.strategy.slice(0, self.strategy.len()).to_vec() }
    }
}

pub(crate) fn normalize_strategy(raw: &[f32], na: usize, nh: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; na * nh];
    for h in 0..nh {
        let sum: f32 = (0..na).map(|a| raw[a * nh + h]).sum();
        if sum > 0.0 {
            for a in 0..na {
                out[a * nh + h] = raw[a * nh + h] / sum;
            }
        } else {
            for a in 0..na {
                out[a * nh + h] = 1.0 / na as f32;
            }
        }
    }
    out
}

fn river_key(dealt: &[Card]) -> usize {
    match dealt.len() {
        0 => 0,
        1 => dealt[0] as usize,
        _ => {
            let (a, b) = (dealt[0].min(dealt[1]) as usize, dealt[0].max(dealt[1]) as usize);
            a * 52 + b
        }
    }
}

fn build_river_evals(board: &[Card], hands: &[Vec<(Card, Card)>; 2]) -> (Vec<u32>, Vec<RiverEval>) {
    let base = cards_mask(board);
    let deck: Vec<Card> = (0..52u8).filter(|c| !board.contains(c)).collect();
    let boards: Vec<Vec<Card>> = match board.len() {
        5 => vec![vec![]],
        4 => deck.iter().map(|&c| vec![c]).collect(),
        _ => {
            let mut v = Vec::new();
            for i in 0..deck.len() {
                for j in i + 1..deck.len() {
                    v.push(vec![deck[i], deck[j]]);
                }
            }
            v
        }
    };
    let evals: Vec<RiverEval> = boards
        .par_iter()
        .map(|dealt| {
            let mask = base | cards_mask(dealt);
            let sorted = [0, 1].map(|p| {
                let mut v: Vec<SortedHand> = hands[p]
                    .iter()
                    .enumerate()
                    .filter_map(|(i, &(a, b))| {
                        let hm = crate::card::card_bit(a) | crate::card::card_bit(b);
                        (hm & mask == 0).then(|| SortedHand {
                            strength: eval_mask(mask | hm),
                            idx: i as u32,
                            cards: [a, b],
                        })
                    })
                    .collect();
                v.sort_unstable();
                v
            });
            let bounds = |p: usize| {
                let (hero, vill) = (&sorted[p], &sorted[1 - p]);
                let weaker = hero.iter().map(|e| vill.partition_point(|v| v.strength < e.strength) as u32).collect();
                let stronger = hero.iter().map(|e| vill.partition_point(|v| v.strength <= e.strength) as u32).collect();
                (weaker, stronger)
            };
            let (w0, s0) = bounds(0);
            let (w1, s1) = bounds(1);
            RiverEval { sorted, weaker: [w0, w1], stronger: [s0, s1] }
        })
        .collect();
    let mut slot = vec![NONE; 52 * 52];
    for (i, dealt) in boards.iter().enumerate() {
        slot[river_key(dealt)] = i as u32;
    }
    (slot, evals)
}

/// Suits of a pair, re-exported for the explorer.
pub fn swap_pair_suits(pair: usize) -> (u8, u8) {
    pair_suits(pair)
}
