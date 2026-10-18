//! Multiplayer preflop solver (external-sampling MCCFR).

pub mod cli;
pub mod config;
pub mod explore;
pub mod mccfr;
pub mod tree;

pub use config::{EqrModel, PreflopConfig};
pub use mccfr::PreflopSolver;
pub use tree::{PAction, PKind, PNode, PreflopTree};
