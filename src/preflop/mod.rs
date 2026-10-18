//! Multiplayer preflop solver (external-sampling MCCFR).

pub mod cli;
pub mod config;
pub mod explore;
pub mod mccfr;
pub mod tree;

pub use config::PreflopConfig;
pub use tree::{PAction, PKind, PNode, PreflopTree};
pub use mccfr::PreflopSolver;
