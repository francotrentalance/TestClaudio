//! Cards, combos and bitmask helpers.
//!
//! A card is a `u8` in `0..52` encoded as `rank * 4 + suit`, where rank `0` is a
//! deuce and `12` is an ace, and suits are ordered `c, d, h, s`.
//!
//! Card *masks* (`u64`) use a suit-major layout (`bit = suit * 16 + rank`) so the
//! evaluator can pull out each suit's 13-bit rank set with a shift.

pub type Card = u8;

pub const RANK_CHARS: &[u8; 13] = b"23456789TJQKA";
pub const SUIT_CHARS: &[u8; 4] = b"cdhs";
pub const NUM_CARDS: usize = 52;
pub const NUM_COMBOS: usize = 1326;

#[inline]
pub const fn rank(c: Card) -> u8 {
    c >> 2
}

#[inline]
pub const fn suit(c: Card) -> u8 {
    c & 3
}

#[inline]
pub const fn make_card(rank: u8, suit: u8) -> Card {
    rank * 4 + suit
}

#[inline]
pub const fn card_bit(c: Card) -> u64 {
    1u64 << ((suit(c) as u32) * 16 + rank(c) as u32)
}

pub fn cards_mask(cards: &[Card]) -> u64 {
    cards.iter().fold(0, |m, &c| m | card_bit(c))
}

pub fn parse_rank(ch: char) -> Option<u8> {
    let up = ch.to_ascii_uppercase() as u8;
    RANK_CHARS.iter().position(|&r| r == up).map(|r| r as u8)
}

pub fn parse_suit(ch: char) -> Option<u8> {
    let low = ch.to_ascii_lowercase() as u8;
    SUIT_CHARS.iter().position(|&s| s == low).map(|s| s as u8)
}

/// Parses a single card such as `"As"`, `"td"` or `"7H"`.
pub fn parse_card(s: &str) -> Result<Card, String> {
    let mut it = s.chars();
    match (it.next(), it.next(), it.next()) {
        (Some(r), Some(su), None) => {
            let r = parse_rank(r).ok_or_else(|| format!("invalid rank in card '{s}'"))?;
            let su = parse_suit(su).ok_or_else(|| format!("invalid suit in card '{s}'"))?;
            Ok(make_card(r, su))
        }
        _ => Err(format!("invalid card '{s}'")),
    }
}

/// Parses a list of cards. Separators (spaces, commas, dashes) are optional:
/// `"AsKd7h"`, `"As Kd 7h"` and `"As,Kd,7h"` are all accepted. Duplicates are rejected.
pub fn parse_cards(s: &str) -> Result<Vec<Card>, String> {
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace() && *c != ',' && *c != '-').collect();
    if !chars.len().is_multiple_of(2) {
        return Err(format!("invalid card list '{s}'"));
    }
    let mut cards = Vec::with_capacity(chars.len() / 2);
    let mut seen = 0u64;
    for pair in chars.chunks(2) {
        let c = parse_card(&pair.iter().collect::<String>())?;
        if seen & card_bit(c) != 0 {
            return Err(format!("duplicate card '{}' in '{s}'", card_to_string(c)));
        }
        seen |= card_bit(c);
        cards.push(c);
    }
    Ok(cards)
}

pub fn card_to_string(c: Card) -> String {
    let mut s = String::with_capacity(2);
    s.push(RANK_CHARS[rank(c) as usize] as char);
    s.push(SUIT_CHARS[suit(c) as usize] as char);
    s
}

pub fn cards_to_string(cards: &[Card]) -> String {
    cards.iter().map(|&c| card_to_string(c)).collect()
}

/// Index of the two-card combo `{a, b}` in `0..1326`, independent of order.
#[inline]
pub const fn combo_index(a: Card, b: Card) -> usize {
    let (hi, lo) = if a > b { (a as usize, b as usize) } else { (b as usize, a as usize) };
    hi * (hi - 1) / 2 + lo
}

const fn build_combo_table() -> [(Card, Card); NUM_COMBOS] {
    let mut table = [(0u8, 0u8); NUM_COMBOS];
    let mut hi = 1;
    while hi < 52 {
        let mut lo = 0;
        while lo < hi {
            table[combo_index(hi as u8, lo as u8)] = (hi as u8, lo as u8);
            lo += 1;
        }
        hi += 1;
    }
    table
}

static COMBO_TABLE: [(Card, Card); NUM_COMBOS] = build_combo_table();

/// The two cards of combo `idx`, higher card index first.
#[inline]
pub fn combo_cards(idx: usize) -> (Card, Card) {
    COMBO_TABLE[idx]
}

#[inline]
pub fn combo_mask(idx: usize) -> u64 {
    let (a, b) = COMBO_TABLE[idx];
    card_bit(a) | card_bit(b)
}

/// Human-readable combo with the higher rank first, e.g. `"AsKd"`.
pub fn combo_to_string(idx: usize) -> String {
    let (a, b) = combo_cards(idx);
    let (a, b) = if rank(a) >= rank(b) { (a, b) } else { (b, a) };
    format!("{}{}", card_to_string(a), card_to_string(b))
}

/// Applies a suit permutation (`perm[old_suit] = new_suit`) to a card.
#[inline]
pub fn permute_card(c: Card, perm: &[u8; 4]) -> Card {
    make_card(rank(c), perm[suit(c) as usize])
}

/// Small, fast, seedable PRNG (xoshiro256**), so results are reproducible
/// without pulling in a dependency.
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut z = seed;
        let mut next = || {
            // splitmix64
            z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            x ^ (x >> 31)
        };
        Rng { s: [next(), next(), next(), next()] }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    /// Uniform integer in `0..n` (Lemire's multiply-shift; bias is negligible for small `n`).
    #[inline]
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }

    /// Uniform float in `[0, 1)`.
    #[inline]
    pub fn unit_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 * (1.0 / (1u64 << 24) as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_format_roundtrip() {
        for c in 0..52u8 {
            assert_eq!(parse_card(&card_to_string(c)).unwrap(), c);
        }
        assert_eq!(parse_cards("As Kd,7h").unwrap(), parse_cards("AsKd7h").unwrap());
        assert!(parse_cards("AsAs").is_err());
        assert!(parse_cards("Ax").is_err());
    }

    #[test]
    fn combo_indexing_is_a_bijection() {
        let mut seen = vec![false; NUM_COMBOS];
        for a in 0..52u8 {
            for b in 0..a {
                let i = combo_index(a, b);
                assert_eq!(i, combo_index(b, a));
                assert!(!seen[i]);
                seen[i] = true;
                assert_eq!(combo_cards(i), (a, b));
            }
        }
        assert!(seen.iter().all(|&s| s));
    }

    #[test]
    fn masks_are_distinct() {
        let all = (0..52u8).fold(0u64, |m, c| {
            assert_eq!(m & card_bit(c), 0);
            m | card_bit(c)
        });
        assert_eq!(all.count_ones(), 52);
    }
}
