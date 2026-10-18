//! Hand evaluator for 5, 6 or 7 cards.
//!
//! The result is a `u32` where a larger value is a stronger hand. It packs the
//! category in bits 20..24 and up to five kicker ranks as 4-bit nibbles below it,
//! so comparing two values compares the hands.

use crate::card::{card_bit, Card};

pub const HIGH_CARD: u32 = 0;
pub const ONE_PAIR: u32 = 1;
pub const TWO_PAIR: u32 = 2;
pub const TRIPS: u32 = 3;
pub const STRAIGHT: u32 = 4;
pub const FLUSH: u32 = 5;
pub const FULL_HOUSE: u32 = 6;
pub const QUADS: u32 = 7;
pub const STRAIGHT_FLUSH: u32 = 8;

pub const CATEGORY_NAMES: [&str; 9] = [
    "high card",
    "one pair",
    "two pair",
    "three of a kind",
    "straight",
    "flush",
    "full house",
    "four of a kind",
    "straight flush",
];

/// For a 13-bit rank set: 1 + the high rank of the best straight it contains, or 0.
const fn build_straight_table() -> [u8; 8192] {
    let mut t = [0u8; 8192];
    let mut m = 0;
    while m < 8192 {
        let mut high = 12i32;
        while high >= 4 {
            let window = 0x1Fu32 << (high - 4);
            if (m as u32) & window == window {
                t[m] = high as u8 + 1;
                break;
            }
            high -= 1;
        }
        // The wheel (A-2-3-4-5) is a five-high straight.
        if t[m] == 0 && (m as u32) & 0x100F == 0x100F {
            t[m] = 3 + 1;
        }
        m += 1;
    }
    t
}

/// For a 13-bit rank set: its five highest ranks packed as nibbles, highest first.
const fn build_top5_table() -> [u32; 8192] {
    let mut t = [0u32; 8192];
    let mut m = 0;
    while m < 8192 {
        let mut packed = 0u32;
        let mut found = 0;
        let mut r = 12i32;
        while r >= 0 && found < 5 {
            if (m >> r) & 1 == 1 {
                packed |= (r as u32) << (16 - 4 * found);
                found += 1;
            }
            r -= 1;
        }
        t[m] = packed;
        m += 1;
    }
    t
}

static STRAIGHT_TABLE: [u8; 8192] = build_straight_table();
static TOP5_TABLE: [u32; 8192] = build_top5_table();

#[inline]
fn top_rank(m: u32) -> u32 {
    31 - m.leading_zeros()
}

/// Top `n` ranks of a rank set, packed into the low `4 * n` bits.
#[inline]
fn top_n(m: u32, n: u32) -> u32 {
    TOP5_TABLE[m as usize] >> (4 * (5 - n))
}

/// Evaluates the cards in `mask` (5 to 7 cards, suit-major layout from [`card_bit`]).
#[inline]
pub fn eval_mask(mask: u64) -> u32 {
    let c = (mask & 0x1FFF) as u32;
    let d = ((mask >> 16) & 0x1FFF) as u32;
    let h = ((mask >> 32) & 0x1FFF) as u32;
    let s = ((mask >> 48) & 0x1FFF) as u32;
    let ranks = c | d | h | s;

    // With at most 7 cards a flush excludes quads and full houses, so it can be
    // resolved first.
    for suited in [c, d, h, s] {
        if suited.count_ones() >= 5 {
            let st = STRAIGHT_TABLE[suited as usize] as u32;
            if st != 0 {
                return (STRAIGHT_FLUSH << 20) | ((st - 1) << 16);
            }
            return (FLUSH << 20) | top_n(suited, 5);
        }
    }

    let quads = c & d & h & s;
    if quads != 0 {
        let q = top_rank(quads);
        let kicker = top_rank(ranks & !(1 << q));
        return (QUADS << 20) | (q << 16) | (kicker << 12);
    }

    let three = (c & d & h) | (c & d & s) | (c & h & s) | (d & h & s);
    let two = (c & d) | (c & h) | (c & s) | (d & h) | (d & s) | (h & s);

    if three != 0 {
        let t = top_rank(three);
        let rest = two & !(1 << t);
        if rest != 0 {
            return (FULL_HOUSE << 20) | (t << 16) | (top_rank(rest) << 12);
        }
    }

    let st = STRAIGHT_TABLE[ranks as usize] as u32;
    if st != 0 {
        return (STRAIGHT << 20) | ((st - 1) << 16);
    }

    if three != 0 {
        let t = top_rank(three);
        return (TRIPS << 20) | (t << 16) | (top_n(ranks & !(1 << t), 2) << 8);
    }

    if two.count_ones() >= 2 {
        let p1 = top_rank(two);
        let p2 = top_rank(two & !(1 << p1));
        let kicker = top_rank(ranks & !((1 << p1) | (1 << p2)));
        return (TWO_PAIR << 20) | (p1 << 16) | (p2 << 12) | (kicker << 8);
    }

    if two != 0 {
        let p = top_rank(two);
        return (ONE_PAIR << 20) | (p << 16) | (top_n(ranks & !(1 << p), 3) << 4);
    }

    (HIGH_CARD << 20) | top_n(ranks, 5)
}

pub fn eval_cards(cards: &[Card]) -> u32 {
    debug_assert!((5..=7).contains(&cards.len()));
    eval_mask(cards.iter().fold(0, |m, &c| m | card_bit(c)))
}

#[inline]
pub fn category(value: u32) -> u32 {
    value >> 20
}

pub fn describe(value: u32) -> &'static str {
    CATEGORY_NAMES[category(value) as usize]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::parse_cards;

    fn ev(s: &str) -> u32 {
        eval_cards(&parse_cards(s).unwrap())
    }

    #[test]
    fn ordering_examples() {
        assert!(ev("AsKsQsJsTs") > ev("KsQsJsTs9s"));
        assert!(ev("5s4s3s2sAs") < ev("6s5s4s3s2s"));
        assert_eq!(category(ev("5s4s3s2sAs")), STRAIGHT_FLUSH);
        assert_eq!(category(ev("5d4s3s2sAh")), STRAIGHT);
        assert!(ev("AhAdAcAs2c") > ev("KhKdKcKsAc"));
        assert!(ev("AhAdAc2s2c") > ev("KhKdKcAsAc"));
        assert!(ev("AhAdKcKsQc") > ev("AhAdKcKsJc"));
        assert!(ev("2h3h4h5h7h") > ev("AsKdQcJhTs"));
        assert!(ev("AhAd5c4s3c") < ev("AhAd6c4s3c"));
        // Best five of seven: board plays.
        assert_eq!(ev("AsKsQsJsTs2d3d"), ev("AsKsQsJsTs4c5c"));
        // Two trips make a full house with the higher trips.
        assert_eq!(category(ev("AhAdAc2s2c2hKd")), FULL_HOUSE);
        assert!(ev("AhAdAc2s2c2hKd") < ev("AhAdAcKs2c2hKd"));
        // Three pairs: best two plus the best kicker.
        assert_eq!(ev("AhAdKcKsQcQhJd"), ev("AhAdKcKsQcJh2d"));
    }

    /// Exhaustive check over all 2,598,960 five-card hands against the known
    /// category counts and the 7,462 distinct hand values.
    #[test]
    fn five_card_counts() {
        let mut counts = [0u64; 9];
        let mut distinct = std::collections::HashSet::new();
        for a in 0..52u8 {
            for b in 0..a {
                for c in 0..b {
                    for d in 0..c {
                        for e in 0..d {
                            let v = eval_cards(&[a, b, c, d, e]);
                            counts[category(v) as usize] += 1;
                            distinct.insert(v);
                        }
                    }
                }
            }
        }
        assert_eq!(counts, [1_302_540, 1_098_240, 123_552, 54_912, 10_200, 5_108, 3_744, 624, 40]);
        assert_eq!(distinct.len(), 7462);
    }

    /// Exhaustive check over all 133,784,560 seven-card hands (about a second in release).
    #[test]
    fn seven_card_counts() {
        let bits: Vec<u64> = (0..52u8).map(card_bit).collect();
        let mut counts = [0u64; 9];
        for a in 0..52 {
            for b in 0..a {
                let m2 = bits[a] | bits[b];
                for c in 0..b {
                    let m3 = m2 | bits[c];
                    for d in 0..c {
                        let m4 = m3 | bits[d];
                        for e in 0..d {
                            let m5 = m4 | bits[e];
                            for f in 0..e {
                                let m6 = m5 | bits[f];
                                for g in 0..f {
                                    counts[category(eval_mask(m6 | bits[g])) as usize] += 1;
                                }
                            }
                        }
                    }
                }
            }
        }
        assert_eq!(
            counts,
            [23_294_460, 58_627_800, 31_433_400, 6_461_620, 6_180_020, 4_047_644, 3_473_184, 224_848, 41_584]
        );
    }
}
