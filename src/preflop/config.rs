//! Preflop game configuration.

use serde::{Deserialize, Serialize};

pub const MAX_PLAYERS: usize = 6;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PreflopConfig {
    /// Number of players, 2 to 6 (6 = UTG, HJ, CO, BTN, SB, BB).
    pub players: usize,
    /// Effective stack in big blinds (all players start equal).
    pub stack: f64,
    pub sb: f64,
    pub bb: f64,
    /// Ante posted by every player (dead money).
    pub ante: f64,
    /// Push/fold game: the first player in can only fold or go all-in.
    pub pushfold: bool,
    /// Open raise size (raise-to, in bb) for every position but the SB.
    pub open: f64,
    pub sb_open: f64,
    /// Big blinds added to an isolation raise per limper.
    pub limp_raise_add: f64,
    /// 3-bet raise-to as a multiple of the open, when in position against the opener.
    pub threebet_ip: f64,
    /// 3-bet multiple when out of position (blinds, or squeezing from in front).
    pub threebet_oop: f64,
    /// 4-bet (and later sized raises) as a multiple of the raise faced.
    pub fourbet: f64,
    /// Extra multiple of the raise faced added per caller in between (squeeze sizing).
    pub caller_add: f64,
    /// Number of sized raises (open, 3-bet, 4-bet...). Beyond this only all-in is allowed.
    pub max_raises: u32,
    /// A sized raise to at least this fraction of the stack becomes an all-in.
    pub allin_threshold: f64,
    /// Offer all-in next to a sized raise when the stack is at most this many times the raise.
    pub jam_ratio: f64,
    /// Allow open-limping from UTG..BTN.
    pub limp: bool,
    /// Allow the SB to complete.
    pub sb_limp: bool,
    /// Allow calling a 3-bet (or more) cold, without money already in voluntarily.
    pub cold_call_3bet: bool,
    /// Maximum number of cold callers of an open; later players may only fold or raise.
    pub max_cold_callers: u32,
    /// Postflop model: relative equity realization of the first player to act
    /// postflop (the last to act realizes 1.0; positions in between interpolate).
    pub realization_oop: f64,
    /// Postflop model: pot shares are proportional to `realization * equity^exponent`.
    /// Exponents above 1 make strong hands over-realize and weak hands under-realize.
    pub realization_exponent: f64,
    /// Rake fraction and cap (bb), taken from pots that see a flop.
    pub rake: f64,
    pub rake_cap: f64,
    pub iterations: u64,
    /// Random boards drawn per iteration to estimate showdown equity.
    pub equity_boards: usize,
    /// Iterations between linear-CFR discounts (applied during the first 40% of the run).
    pub discount_interval: u64,
    pub seed: u64,
}

impl Default for PreflopConfig {
    fn default() -> Self {
        PreflopConfig {
            players: 6,
            stack: 100.0,
            sb: 0.5,
            bb: 1.0,
            ante: 0.0,
            pushfold: false,
            open: 2.5,
            sb_open: 3.0,
            limp_raise_add: 1.0,
            threebet_ip: 3.0,
            threebet_oop: 4.0,
            fourbet: 2.3,
            caller_add: 1.0,
            max_raises: 3,
            allin_threshold: 0.4,
            jam_ratio: 4.0,
            limp: false,
            sb_limp: false,
            cold_call_3bet: false,
            max_cold_callers: 2,
            realization_oop: 0.75,
            realization_exponent: 1.4,
            rake: 0.0,
            rake_cap: 0.0,
            iterations: 20_000_000,
            equity_boards: 12,
            discount_interval: 200_000,
            seed: 1,
        }
    }
}

impl PreflopConfig {
    pub fn validate(&self) -> Result<(), String> {
        if !(2..=MAX_PLAYERS).contains(&self.players) {
            return Err(format!("players must be 2..={MAX_PLAYERS}"));
        }
        if self.stack <= self.bb {
            return Err("stack must be larger than the big blind".into());
        }
        if self.equity_boards == 0 {
            return Err("equity_boards must be at least 1".into());
        }
        if self.realization_oop <= 0.0 || self.realization_exponent <= 0.0 {
            return Err("realization parameters must be positive".into());
        }
        Ok(())
    }

    /// Position names in preflop action order.
    pub fn position_names(&self) -> Vec<&'static str> {
        let all = ["UTG", "HJ", "CO", "BTN", "SB", "BB"];
        match self.players {
            2 => vec!["SB", "BB"],
            n => all[6 - n..].to_vec(),
        }
    }

    pub fn sb_seat(&self) -> usize {
        if self.players == 2 { 0 } else { self.players - 2 }
    }

    pub fn bb_seat(&self) -> usize {
        self.players - 1
    }

    /// Order of acting postflop (0 = first). Heads-up the SB has the button.
    pub fn postflop_rank(&self, seat: usize) -> usize {
        let n = self.players;
        if n == 2 {
            return 1 - seat;
        }
        if seat >= n - 2 { seat - (n - 2) } else { seat + 2 }
    }

    pub fn rake_amount(&self, pot: f64) -> f64 {
        let r = pot * self.rake;
        if self.rake_cap > 0.0 { r.min(self.rake_cap) } else { r }
    }
}
