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
    /// Postflop model with hand-dependent realization measured from postflop
    /// solves. When set, it replaces `realization_oop`: pot shares are
    /// proportional to `eqr(position, SPR, hand) * equity^exponent`.
    pub eqr: Option<EqrModel>,
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
            eqr: None,
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
        if let Some(m) = &self.eqr {
            m.validate()?;
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
        if self.players == 2 {
            0
        } else {
            self.players - 2
        }
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
        if seat >= n - 2 {
            seat - (n - 2)
        } else {
            seat + 2
        }
    }

    pub fn rake_amount(&self, pot: f64) -> f64 {
        let r = pot * self.rake;
        if self.rake_cap > 0.0 {
            r.min(self.rake_cap)
        } else {
            r
        }
    }
}

/// Equity realization (EQR) of each starting hand, measured from postflop
/// solves: the share of the pot a hand wins on average divided by its raw
/// equity share. Measured at two stack-to-pot ratios (a single-raised pot
/// and a 3-bet pot) for the player first and last to act postflop.
///
/// For a flop with stack-to-pot ratio `s` the factor is taken from the deep
/// table when `s >= deep_spr`, interpolated in `ln s` between the two tables
/// down to `shallow_spr`, and from there toward 1 (no realization edge) at
/// `s <= 1`, where little play is left after the flop. Players between the
/// first and the last to act interpolate by position.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EqrModel {
    pub deep_spr: f64,
    pub shallow_spr: f64,
    /// 169 values each, indexed by starting-hand class.
    pub deep_oop: Vec<f32>,
    pub deep_ip: Vec<f32>,
    pub shallow_oop: Vec<f32>,
    pub shallow_ip: Vec<f32>,
}

impl EqrModel {
    pub fn validate(&self) -> Result<(), String> {
        if !(self.shallow_spr > 1.0 && self.deep_spr > self.shallow_spr) {
            return Err("eqr: need 1 < shallow_spr < deep_spr".into());
        }
        for t in [&self.deep_oop, &self.deep_ip, &self.shallow_oop, &self.shallow_ip] {
            if t.len() != crate::range::NUM_CLASSES || t.iter().any(|&x| !(x > 0.0 && x.is_finite())) {
                return Err("eqr: each table needs 169 positive values".into());
            }
        }
        Ok(())
    }

    /// Weights of the deep table, the shallow table and plain equity (factor 1)
    /// at stack-to-pot ratio `spr`.
    pub fn mix(&self, spr: f64) -> [f32; 3] {
        let (deep, shallow) = (self.deep_spr, self.shallow_spr);
        if spr >= deep {
            [1.0, 0.0, 0.0]
        } else if spr >= shallow {
            let u = ((spr.ln() - shallow.ln()) / (deep.ln() - shallow.ln())) as f32;
            [u, 1.0 - u, 0.0]
        } else if spr > 1.0 {
            let u = (spr.ln() / shallow.ln()) as f32;
            [0.0, u, 1.0 - u]
        } else {
            [0.0, 0.0, 1.0]
        }
    }

    /// Realization factor of `class` for a player at position `t` (0 = first
    /// to act postflop, 1 = last) on a flop whose [`EqrModel::mix`] is `mix`.
    #[inline]
    pub fn factor(&self, mix: [f32; 3], t: f32, class: usize) -> f32 {
        let deep = self.deep_oop[class] + (self.deep_ip[class] - self.deep_oop[class]) * t;
        let shallow = self.shallow_oop[class] + (self.shallow_ip[class] - self.shallow_oop[class]) * t;
        mix[0] * deep + mix[1] * shallow + mix[2]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> EqrModel {
        let n = crate::range::NUM_CLASSES;
        EqrModel {
            deep_spr: 16.0,
            shallow_spr: 4.0,
            deep_oop: vec![0.8; n],
            deep_ip: vec![1.2; n],
            shallow_oop: vec![0.9; n],
            shallow_ip: vec![1.1; n],
        }
    }

    #[test]
    fn eqr_mix_is_continuous_and_sums_to_one() {
        let m = model();
        assert_eq!(m.mix(20.0), [1.0, 0.0, 0.0]);
        assert_eq!(m.mix(0.5), [0.0, 0.0, 1.0]);
        let s = m.mix(4.0);
        assert!((s[1] - 1.0).abs() < 1e-6, "{s:?}");
        let s = m.mix(8.0); // halfway between 4 and 16 in ln(spr)
        assert!((s[0] - 0.5).abs() < 1e-5 && (s[1] - 0.5).abs() < 1e-5, "{s:?}");
        let s = m.mix(2.0); // halfway between 1 and 4
        assert!((s[1] - 0.5).abs() < 1e-5 && (s[2] - 0.5).abs() < 1e-5, "{s:?}");
        for spr in [0.3, 1.0, 1.5, 3.9, 4.1, 10.0, 15.9, 16.1, 40.0] {
            let s = m.mix(spr);
            assert!((s.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn eqr_factor_interpolates_position_and_depth() {
        let m = model();
        assert!((m.factor([1.0, 0.0, 0.0], 0.0, 5) - 0.8).abs() < 1e-6);
        assert!((m.factor([1.0, 0.0, 0.0], 1.0, 5) - 1.2).abs() < 1e-6);
        assert!((m.factor([0.0, 1.0, 0.0], 0.5, 5) - 1.0).abs() < 1e-6);
        assert!((m.factor([0.0, 0.0, 1.0], 0.0, 5) - 1.0).abs() < 1e-6);
        assert!(m.validate().is_ok());
        assert!(EqrModel { deep_spr: 3.0, ..model() }.validate().is_err());
    }
}
