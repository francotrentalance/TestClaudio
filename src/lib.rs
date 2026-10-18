//! A GTO solver for 6-max No-Limit Hold'em.
//!
//! * [`postflop`]: exact heads-up postflop solver (Discounted CFR over the full
//!   card tree, no card abstraction) with exploitability measurement.
//! * [`preflop`]: multiplayer preflop solver (external-sampling MCCFR) over a
//!   configurable 2-6 player action tree.

// Index loops over hands and actions read more clearly than iterator chains here.
#![allow(clippy::needless_range_loop)]

pub mod card;
pub mod display;
pub mod equity;
pub mod eval;
pub mod postflop;
pub mod preflop;
pub mod range;
