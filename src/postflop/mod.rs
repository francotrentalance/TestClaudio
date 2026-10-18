//! Heads-up postflop solver.
//!
//! Solves a single postflop spot exactly (no card abstraction) for a fixed
//! betting tree with Discounted CFR. In 6-max the spot comes from a preflop
//! line, e.g. BTN opens and BB calls: enter both ranges, the flop, the pot and
//! the stacks behind.

pub mod config;
pub mod explore;
pub mod game;
pub mod io;
pub mod solver;
pub mod storage;
pub mod tree;

pub use config::{BetSize, PostflopConfig, StreetSizes};
pub use game::{PostflopGame, TreeStats, tree_stats};
pub use tree::{Action, Node, NodeKind};

/// Illustrative 100bb BTN opening range (~44% of hands).
pub const DEFAULT_BTN_RANGE: &str =
    "22+,A2s+,K2s+,Q4s+,J6s+,T6s+,96s+,85s+,75s+,64s+,54s,A2o+,K8o+,Q9o+,J9o+,T8o+,98o";

/// Illustrative BB flatting range vs a 2.5bb BTN open (3-bets removed).
pub const DEFAULT_BB_RANGE: &str = "22-QQ,A2s-AQs,K2s-KQs,Q2s-QJs,J4s-JTs,T6s-T9s,96s-98s,85s-87s,74s-76s,63s-65s,52s-54s,43s,A2o-AJo,K7o-KQo,Q8o-QJo,J8o-JTo,T8o-T9o,98o,87o";
