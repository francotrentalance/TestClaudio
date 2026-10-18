//! Configuration of a heads-up postflop spot and its betting tree.

use serde::{Deserialize, Serialize};

/// One bet or raise size option.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BetSize {
    /// Fraction of the pot. For raises this is the raise *on top of the call*
    /// as a fraction of the pot after calling (the PioSolver convention).
    Pot(f64),
    /// Multiple of the bet being raised (`"3x"` raises a 10 bet to 30). Raises only.
    Mult(f64),
    AllIn,
}

/// Parses a size list such as `"33%, 75%, a"` or `"2.5x, 100%"`.
pub fn parse_sizes(s: &str) -> Result<Vec<BetSize>, String> {
    let mut out = Vec::new();
    for tok in s.split(|c: char| c == ',' || c.is_whitespace()) {
        let tok = tok.trim().to_ascii_lowercase();
        if tok.is_empty() {
            continue;
        }
        let size = if tok == "a" || tok == "allin" || tok == "all-in" {
            BetSize::AllIn
        } else if let Some(p) = tok.strip_suffix('%') {
            let v: f64 = p.parse().map_err(|_| format!("invalid size '{tok}'"))?;
            if v <= 0.0 {
                return Err(format!("size must be positive: '{tok}'"));
            }
            BetSize::Pot(v / 100.0)
        } else if let Some(m) = tok.strip_suffix('x') {
            let v: f64 = m.parse().map_err(|_| format!("invalid size '{tok}'"))?;
            if v <= 1.0 {
                return Err(format!("raise multiple must be > 1: '{tok}'"));
            }
            BetSize::Mult(v)
        } else {
            return Err(format!("invalid size '{tok}' (use e.g. 75%, 3x or a)"));
        };
        out.push(size);
    }
    Ok(out)
}

/// Bet and raise sizes for one street.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StreetSizes {
    pub oop_bet: String,
    pub oop_raise: String,
    pub ip_bet: String,
    pub ip_raise: String,
    /// Sizes OOP may lead with when IP was the aggressor on the previous street.
    /// `None` means the regular `oop_bet` sizes; `Some("")` disables donk bets.
    pub oop_donk: Option<String>,
}

impl Default for StreetSizes {
    fn default() -> Self {
        StreetSizes::symmetric("33%, 75%", "3x")
    }
}

impl StreetSizes {
    pub fn symmetric(bet: &str, raise: &str) -> Self {
        StreetSizes {
            oop_bet: bet.into(),
            oop_raise: raise.into(),
            ip_bet: bet.into(),
            ip_raise: raise.into(),
            oop_donk: None,
        }
    }

    pub fn parsed(&self) -> Result<ParsedStreet, String> {
        let bet = [parse_sizes(&self.oop_bet)?, parse_sizes(&self.ip_bet)?];
        for sizes in &bet {
            if sizes.iter().any(|s| matches!(s, BetSize::Mult(_))) {
                return Err("bet sizes cannot use 'x' multiples (only raises can)".into());
            }
        }
        let donk = match &self.oop_donk {
            Some(s) => Some(parse_sizes(s)?),
            None => None,
        };
        Ok(ParsedStreet { bet, raise: [parse_sizes(&self.oop_raise)?, parse_sizes(&self.ip_raise)?], donk })
    }
}

#[derive(Clone, Debug)]
pub struct ParsedStreet {
    pub bet: [Vec<BetSize>; 2],
    pub raise: [Vec<BetSize>; 2],
    pub donk: Option<Vec<BetSize>>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PostflopConfig {
    /// Flop, turn or river board, e.g. `"Ks7d2c"`.
    pub board: String,
    /// Range of the player out of position (first to act postflop).
    pub oop_range: String,
    pub ip_range: String,
    /// Pot at the start of the spot (chips, conventionally big blinds).
    pub pot: f64,
    /// Effective stack behind at the start of the spot.
    pub stack: f64,
    pub flop: StreetSizes,
    pub turn: StreetSizes,
    pub river: StreetSizes,
    /// Maximum number of raises after the first bet on each street.
    pub max_raises: u32,
    /// A bet or raise committing at least this fraction of the remaining stack becomes all-in.
    pub allin_threshold: f64,
    /// Offer an explicit all-in whenever the remaining stack is at most this many pots.
    pub add_allin_spr: f64,
    /// Rake as a fraction of the pot (e.g. 0.05), taken from every pot.
    pub rake: f64,
    /// Rake cap in chips (0 = no cap).
    pub rake_cap: f64,
    pub iterations: u32,
    /// Stop once exploitability falls below this percentage of the starting pot.
    pub target_exploitability: f64,
    /// Exploit suit symmetry at chance nodes (same result, less time and memory).
    pub isomorphism: bool,
    /// Store regrets and strategies in 16 bits (half the memory, slightly less precise).
    pub compress: bool,
}

impl Default for PostflopConfig {
    fn default() -> Self {
        PostflopConfig {
            board: "Ks7d2c".into(),
            // BTN open vs BB flat at 100bb (illustrative ranges).
            oop_range: crate::postflop::DEFAULT_BB_RANGE.into(),
            ip_range: crate::postflop::DEFAULT_BTN_RANGE.into(),
            pot: 5.5,
            stack: 97.5,
            flop: StreetSizes::symmetric("33%", "3x"),
            turn: StreetSizes::symmetric("75%", "3x"),
            river: StreetSizes::symmetric("75%", "3x"),
            max_raises: 1,
            allin_threshold: 0.67,
            add_allin_spr: 1.5,
            rake: 0.0,
            rake_cap: 0.0,
            iterations: 1000,
            target_exploitability: 0.3,
            isomorphism: true,
            compress: false,
        }
    }
}

impl PostflopConfig {
    pub fn street(&self, board_len: usize) -> &StreetSizes {
        match board_len {
            3 => &self.flop,
            4 => &self.turn,
            _ => &self.river,
        }
    }

    pub fn rake_amount(&self, pot: f64) -> f64 {
        let r = pot * self.rake;
        if self.rake_cap > 0.0 { r.min(self.rake_cap) } else { r }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parsing() {
        assert_eq!(
            parse_sizes("33%, 75% a 2.5x").unwrap(),
            vec![BetSize::Pot(0.33), BetSize::Pot(0.75), BetSize::AllIn, BetSize::Mult(2.5)]
        );
        assert!(parse_sizes("75").is_err());
        assert!(parse_sizes("0.5x").is_err());
        assert!(StreetSizes::symmetric("3x", "3x").parsed().is_err());
    }
}
