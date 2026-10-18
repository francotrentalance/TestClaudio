//! Precomputed training content shipped inside the app.
//!
//! * [`PreflopPack`]: a solved preflop game (6-max 100bb cash, or one push/fold
//!   stack depth) as quantized average strategies.
//! * [`FlopLibrary`]: solved flops. For each flop, the flop-street decision
//!   nodes keep every hand's strategy and the EV of every action, and each line
//!   that reaches the turn keeps both players' ranges, so the turn and river can
//!   be re-solved exactly in a second or two when a hand gets there.

use std::sync::Arc;

use holdem_solver::card::Card;
use holdem_solver::postflop::PostflopConfig;
use holdem_solver::preflop::{PKind, PNode, PreflopConfig, PreflopTree};
use holdem_solver::range::NUM_CLASSES;
use serde::{Deserialize, Serialize};

/// Probabilities to bytes summing to 255 (the largest share absorbs rounding).
pub fn quantize(probs: &[f32]) -> Vec<u8> {
    let mut q: Vec<u8> = probs.iter().map(|&p| (p.clamp(0.0, 1.0) * 255.0).round() as u8).collect();
    let sum: i32 = q.iter().map(|&x| x as i32).sum();
    if let Some(imax) = (0..probs.len()).max_by(|&a, &b| probs[a].partial_cmp(&probs[b]).unwrap()) {
        let fixed = q[imax] as i32 + (255 - sum);
        q[imax] = fixed.clamp(0, 255) as u8;
    }
    q
}

pub fn dequantize(q: &[u8]) -> Vec<f32> {
    let sum: f32 = q.iter().map(|&x| x as f32).sum();
    if sum <= 0.0 {
        return vec![1.0 / q.len().max(1) as f32; q.len()];
    }
    q.iter().map(|&x| x as f32 / sum).collect()
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PreflopPack {
    pub name: String,
    pub config: PreflopConfig,
    /// Quantized average strategy in solver layout: for each decision node,
    /// `169 classes x actions` bytes starting at the node's storage offset.
    pub strategy: Vec<u8>,
    pub iterations: u64,
}

/// A preflop pack with its rebuilt action tree.
pub struct LoadedPreflop {
    pub pack: PreflopPack,
    pub tree: PreflopTree,
}

impl LoadedPreflop {
    pub fn new(pack: PreflopPack) -> Result<Self, String> {
        let tree = PreflopTree::build(&pack.config)?;
        if tree.storage_len != pack.strategy.len() {
            return Err(format!(
                "pack '{}' has {} entries but its tree needs {}",
                pack.name,
                pack.strategy.len(),
                tree.storage_len
            ));
        }
        Ok(LoadedPreflop { pack, tree })
    }

    /// Strategy of hand class `class` at a decision node.
    pub fn strategy(&self, node: &PNode, class: usize) -> Vec<f32> {
        debug_assert!(class < NUM_CLASSES);
        let na = node.num_children as usize;
        let base = node.storage as usize + class * na;
        dequantize(&self.pack.strategy[base..base + na])
    }

    /// For every hand class: the probability that `seat`'s own earlier actions
    /// on the way to node `idx` are what the solution does with that class
    /// (the product of their frequencies; 1.0 before the seat's first decision).
    /// The solver never trains a class at a node its owner does not reach, so
    /// the stored strategy there means nothing when this is (near) zero.
    pub fn own_reach(&self, idx: usize, seat: usize) -> Vec<f32> {
        let nodes = &self.tree.nodes;
        let mut reach = vec![1.0f32; NUM_CLASSES];
        let mut idx = idx;
        while idx != 0 {
            let parent = &nodes[nodes[idx].parent as usize];
            if parent.kind == PKind::Decision(seat as u8) {
                let i = idx - parent.first_child as usize;
                for (class, r) in reach.iter_mut().enumerate() {
                    *r *= self.strategy(parent, class)[i];
                }
            }
            idx = nodes[idx].parent as usize;
        }
        reach
    }

    /// Stack depth as push/fold charts count it: chips before posting the
    /// ante (the pack's `config.stack` is what is left behind after it).
    pub fn nominal_stack(&self) -> f64 {
        self.pack.config.stack + self.pack.config.ante
    }
}

/// Kind of a postflop action, stored compactly.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum ActKind {
    Fold,
    Check,
    Call,
    Bet,
    Raise,
    AllIn,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Act {
    pub kind: ActKind,
    /// For bets, raises and all-ins: the player's total on the street after acting.
    pub to: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Next {
    /// Another flop decision (index into `FlopSpot::nodes`).
    Node(u32),
    /// The given player folded.
    Fold { folder: u8, contrib: [f32; 2] },
    /// The flop betting ended; index into `FlopSpot::lines`.
    Line(u32),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlopNode {
    pub player: u8,
    /// Chips each player has put in (each counting half the starting pot).
    pub contrib: [f32; 2],
    pub actions: Vec<Act>,
    pub next: Vec<Next>,
    /// `actions x hands` quantized strategy of the acting player.
    pub strategy: Vec<u8>,
    /// `actions x hands` EV (chips, relative to this node) of each action.
    pub ev: Vec<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlopLine {
    pub contrib: [f32; 2],
    /// Both players are all-in: the board just runs out.
    pub all_in: bool,
    /// Reach of every hand of each player at the end of the flop.
    pub reach: [Vec<f32>; 2],
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlopSpot {
    /// Spot family, e.g. "srp_btn_bb".
    pub kind: String,
    /// Full config of the solve (board, ranges, sizes); turn subgames reuse it.
    pub config: PostflopConfig,
    pub board: Vec<Card>,
    pub hands: [Vec<(Card, Card)>; 2],
    pub weights: [Vec<f32>; 2],
    pub nodes: Vec<FlopNode>,
    pub lines: Vec<FlopLine>,
    pub exploitability_pct: f64,
}

impl FlopSpot {
    pub fn hand_index(&self, p: usize, a: Card, b: Card) -> Option<usize> {
        self.hands[p].iter().position(|&(x, y)| (x == a && y == b) || (x == b && y == a))
    }

    pub fn node_strategy(&self, node: &FlopNode, hand: usize) -> Vec<f32> {
        let nh = self.hands[node.player as usize].len();
        let q: Vec<u8> = (0..node.actions.len()).map(|a| node.strategy[a * nh + hand]).collect();
        dequantize(&q)
    }

    pub fn node_evs(&self, node: &FlopNode, hand: usize) -> Vec<f32> {
        let nh = self.hands[node.player as usize].len();
        (0..node.actions.len()).map(|a| node.ev[a * nh + hand]).collect()
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct FlopLibrary {
    pub spots: Vec<FlopSpot>,
}

/// Everything the app trains on.
pub struct Content {
    pub cash: Arc<LoadedPreflop>,
    /// Push/fold packs sorted by nominal stack depth.
    pub pushfold: Vec<Arc<LoadedPreflop>>,
    pub flops: Arc<FlopLibrary>,
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    bincode::serialize(value).expect("serializable")
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    bincode::deserialize(bytes).map_err(|e| e.to_string())
}

impl Content {
    pub fn from_bytes(cash: &[u8], pushfold: &[u8], flops: &[u8]) -> Result<Self, String> {
        let cash = Arc::new(LoadedPreflop::new(decode::<PreflopPack>(cash)?)?);
        let packs: Vec<PreflopPack> = decode(pushfold)?;
        let mut pushfold: Vec<Arc<LoadedPreflop>> = packs.into_iter().map(|p| LoadedPreflop::new(p).map(Arc::new)).collect::<Result<_, _>>()?;
        pushfold.sort_by(|a, b| a.nominal_stack().partial_cmp(&b.nominal_stack()).unwrap());
        let flops = Arc::new(decode::<FlopLibrary>(flops)?);
        Ok(Content { cash, pushfold, flops })
    }

    /// Nominal stack depth of each push/fold pack, ascending.
    pub fn pushfold_stacks(&self) -> Vec<f64> {
        self.pushfold.iter().map(|p| p.nominal_stack()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_roundtrip() {
        let p = [0.7, 0.2, 0.1];
        let q = quantize(&p);
        assert_eq!(q.iter().map(|&x| x as u32).sum::<u32>(), 255);
        let d = dequantize(&q);
        for (a, b) in p.iter().zip(&d) {
            assert!((a - b).abs() < 0.005);
        }
        assert_eq!(quantize(&[1.0, 0.0]), vec![255, 0]);
        assert_eq!(dequantize(&[0, 0]), vec![0.5, 0.5]);
    }
}
