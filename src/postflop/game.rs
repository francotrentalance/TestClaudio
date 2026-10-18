//! A postflop spot instantiated for solving: hands, tree, showdown tables and storage.

use rayon::prelude::*;

use crate::card::{cards_mask, combo_cards, combo_index, combo_mask, parse_cards, Card, NUM_COMBOS};
use crate::eval::eval_mask;
use crate::postflop::config::PostflopConfig;
use crate::postflop::storage::{bytes_per_entry, Block, Storage};
use crate::postflop::tree::{board_invariant, pair_suits, swap_card, IsoEntry, Node, TreeBuilder, NONE};
use crate::range::Range;

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
    pub(crate) storage: Storage,
    pub storage_len: usize,
    pub iteration: u32,
    /// Total weight of all non-conflicting (OOP hand, IP hand) pairs.
    pub norm: f64,
    /// True once regrets are dropped and only the average strategy remains.
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
    pub compressed: bool,
}

impl TreeStats {
    /// Bytes needed to solve (regrets + cumulative strategy) plus the node array.
    pub fn solve_bytes(&self) -> u64 {
        self.storage_floats * bytes_per_entry(self.compressed)
            + (self.nodes * std::mem::size_of::<Node>()) as u64
    }
}

struct Setup {
    board: Vec<Card>,
    hands: [Vec<(Card, Card)>; 2],
    weights: [Vec<f32>; 2],
    hand_index: [Vec<u32>; 2],
    sym_pairs: [bool; 6],
}

fn setup(config: &PostflopConfig, ranges: Option<&[Range; 2]>) -> Result<Setup, String> {
    let board = parse_cards(&config.board)?;
    if !(3..=5).contains(&board.len()) {
        return Err(format!("board must have 3 to 5 cards, got {}", board.len()));
    }
    if config.pot <= 0.0 || config.stack < 0.0 {
        return Err("pot must be positive and stack non-negative".into());
    }
    let ranges = match ranges {
        Some(r) => r.clone(),
        None => [Range::parse(&config.oop_range)?, Range::parse(&config.ip_range)?],
    };
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
    let s = setup(config, None)?;
    let num_hands = [s.hands[0].len(), s.hands[1].len()];
    let mut tb = TreeBuilder::new(config, num_hands, s.sym_pairs)?;
    tb.build(&s.board);
    Ok(stats_of(&tb.nodes, tb.storage_len, num_hands, config.compress))
}

fn stats_of(nodes: &[Node], storage: u64, hands: [usize; 2], compressed: bool) -> TreeStats {
    use crate::postflop::tree::NodeKind;
    TreeStats {
        nodes: nodes.len(),
        player_nodes: nodes.iter().filter(|n| matches!(n.kind, NodeKind::Player(_))).count(),
        chance_nodes: nodes.iter().filter(|n| matches!(n.kind, NodeKind::Chance)).count(),
        storage_floats: storage,
        hands,
        compressed,
    }
}

impl PostflopGame {
    /// Builds the game and allocates solver storage.
    pub fn new(config: PostflopConfig) -> Result<Self, String> {
        Self::build(config, None, None)
    }

    /// Like [`PostflopGame::new`], with explicit per-combo range weights
    /// (`[OOP, IP]`) instead of the config's range strings. Used to solve a
    /// subgame whose ranges come from an earlier street's solution.
    pub fn with_ranges(config: PostflopConfig, ranges: [Range; 2]) -> Result<Self, String> {
        Self::build(config, None, Some(ranges))
    }

    /// Rebuilds a game from its config and attaches a saved average strategy.
    pub fn with_strategy(config: PostflopConfig, strategy: Vec<f32>) -> Result<Self, String> {
        Self::build(config, Some(strategy), None)
    }

    fn build(config: PostflopConfig, strategy: Option<Vec<f32>>, ranges: Option<[Range; 2]>) -> Result<Self, String> {
        let Setup { board, hands, weights, hand_index, sym_pairs } = setup(&config, ranges.as_ref())?;
        let num_hands = [hands[0].len(), hands[1].len()];
        let mut tb = TreeBuilder::new(&config, num_hands, sym_pairs)?;
        tb.build(&board);
        let storage_len = tb.storage_len as usize;
        let player_nodes = tb.player_nodes as usize;
        let nodes = std::mem::take(&mut tb.nodes);
        let iso_tables = std::mem::take(&mut tb.iso_tables);
        drop(tb);

        let same_hand = [0, 1].map(|p| {
            hands[p].iter().map(|&(a, b)| hand_index[1 - p][combo_index(a, b)]).collect::<Vec<u32>>()
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
                        .map(|&(a, b)| hand_index[p][combo_index(swap_card(a, pair), swap_card(b, pair))])
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

        let (storage, finalized) = match strategy {
            Some(st) => {
                if st.len() != storage_len {
                    return Err(format!(
                        "saved strategy has {} entries but the tree needs {storage_len}",
                        st.len()
                    ));
                }
                (Storage::from_average(st), true)
            }
            None => (Storage::new(storage_len, player_nodes, config.compress), false),
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
            storage,
            storage_len,
            iteration: 0,
            norm,
            finalized,
        })
    }

    pub fn stats(&self) -> TreeStats {
        stats_of(
            &self.nodes,
            self.storage_len as u64,
            [self.hands[0].len(), self.hands[1].len()],
            self.config.compress,
        )
    }

    pub fn num_hands(&self, p: usize) -> usize {
        self.hands[p].len()
    }

    /// Showdown table for the board formed by the initial board plus `dealt`.
    pub fn river_eval(&self, dealt: &[Card]) -> &RiverEval {
        &self.river_evals[self.river_slot[river_key(dealt)] as usize]
    }

    /// Storage block of a player node.
    pub(crate) fn block(&self, node: &Node) -> Block {
        let crate::postflop::tree::NodeKind::Player(p) = node.kind else { panic!("not a player node") };
        Block {
            off: node.storage as usize,
            len: node.num_children as usize * self.hands[p as usize].len(),
            pidx: node.pidx as usize,
        }
    }

    /// Average strategy of a player node as `actions x hands`, normalized per hand.
    pub fn average_strategy(&self, node: &Node) -> Vec<f32> {
        let crate::postflop::tree::NodeKind::Player(p) = node.kind else {
            return Vec::new();
        };
        let nh = self.hands[p as usize].len();
        // SAFETY: average strategies are read only while no update of this node runs
        // (between iterations, or by the non-updating player during an iteration).
        unsafe { self.storage.average(self.block(node), node.num_children as usize, nh) }
    }

    fn player_blocks(&self) -> Vec<(Block, usize, usize)> {
        self.nodes
            .iter()
            .filter_map(|n| match n.kind {
                crate::postflop::tree::NodeKind::Player(p) => {
                    Some((self.block(n), n.num_children as usize, self.hands[p as usize].len()))
                }
                _ => None,
            })
            .collect()
    }

    /// Drops regrets, keeping only what is needed to read average strategies.
    pub fn finalize(&mut self) {
        if self.finalized {
            return;
        }
        let blocks = self.player_blocks();
        self.storage.finalize(&blocks);
        self.finalized = true;
    }

    /// Streams the normalized average strategy of every player node in storage
    /// order (the layout [`PostflopGame::with_strategy`] expects).
    pub fn for_each_average_block(&self, mut f: impl FnMut(&[f32])) {
        let mut blocks = self.player_blocks();
        blocks.sort_unstable_by_key(|(b, _, _)| b.off);
        for (b, na, nh) in blocks {
            // SAFETY: called between iterations.
            f(&unsafe { self.storage.average(b, na, nh) });
        }
    }
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
                let weaker =
                    hero.iter().map(|e| vill.partition_point(|v| v.strength < e.strength) as u32).collect();
                let stronger =
                    hero.iter().map(|e| vill.partition_point(|v| v.strength <= e.strength) as u32).collect();
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
