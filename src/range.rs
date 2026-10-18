//! Hand ranges, the 169 starting-hand classes and the 13x13 grid.
//!
//! Range strings use the usual solver syntax, comma separated, with an optional
//! `:weight` (a fraction in `0..=1`, or a percentage if larger than 1):
//!
//! ```text
//! AA, KK:0.5, AKs, AQo-ATo, 22+, A2s-A5s, KTs+, T9, AsKh, 72o:25
//! ```
//!
//! Later tokens override earlier ones, so `22+, 22:0.5` gives deuces half weight.

use crate::card::{
    card_bit, combo_index, make_card, parse_cards, parse_rank, rank, suit, Card, NUM_COMBOS, RANK_CHARS,
};

pub const NUM_CLASSES: usize = 169;

/// Grid index of a starting-hand class. The grid is the standard 13x13 layout
/// with `AA` top-left, suited hands above the diagonal and offsuit below.
#[inline]
pub const fn class_index(r1: u8, r2: u8, suited: bool) -> usize {
    let (hi, lo) = if r1 >= r2 { (r1, r2) } else { (r2, r1) };
    let (row, col) = if suited { (12 - hi, 12 - lo) } else { (12 - lo, 12 - hi) };
    row as usize * 13 + col as usize
}

/// `(high rank, low rank, suited)` of a class index.
pub fn class_ranks(idx: usize) -> (u8, u8, bool) {
    let (row, col) = ((idx / 13) as u8, (idx % 13) as u8);
    if row < col {
        (12 - row, 12 - col, true)
    } else {
        (12 - col, 12 - row, false)
    }
}

pub fn class_name(idx: usize) -> String {
    let (hi, lo, suited) = class_ranks(idx);
    let mut s = String::new();
    s.push(RANK_CHARS[hi as usize] as char);
    s.push(RANK_CHARS[lo as usize] as char);
    if hi != lo {
        s.push(if suited { 's' } else { 'o' });
    }
    s
}

pub fn class_num_combos(idx: usize) -> usize {
    match class_ranks(idx) {
        (h, l, _) if h == l => 6,
        (_, _, true) => 4,
        _ => 12,
    }
}

const fn build_combo_class_table() -> [u8; NUM_COMBOS] {
    let mut table = [0u8; NUM_COMBOS];
    let mut a = 1u8;
    while a < 52 {
        let mut b = 0u8;
        while b < a {
            let suited = suit(a) == suit(b) && rank(a) != rank(b);
            table[combo_index(a, b)] = class_index(rank(a), rank(b), suited) as u8;
            b += 1;
        }
        a += 1;
    }
    table
}

static COMBO_CLASS: [u8; NUM_COMBOS] = build_combo_class_table();

/// The starting-hand class of a combo.
#[inline]
pub fn combo_class(combo: usize) -> usize {
    COMBO_CLASS[combo] as usize
}

pub fn class_combos(idx: usize) -> Vec<usize> {
    let (hi, lo, suited) = class_ranks(idx);
    let mut out = Vec::new();
    for s1 in 0..4 {
        for s2 in 0..4 {
            let (a, b) = (make_card(hi, s1), make_card(lo, s2));
            if a == b || (hi == lo && s1 >= s2) {
                continue;
            }
            if hi != lo && (s1 == s2) != suited {
                continue;
            }
            out.push(combo_index(a, b));
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Range {
    /// Weight of every combo, indexed by [`combo_index`].
    pub weights: Vec<f32>,
}

impl Default for Range {
    fn default() -> Self {
        Range { weights: vec![0.0; NUM_COMBOS] }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Pair,
    Suited,
    Offsuit,
    Both,
}

#[derive(Clone, Copy, Debug)]
struct Pattern {
    hi: u8,
    lo: u8,
    kind: Kind,
}

fn parse_pattern(s: &str) -> Result<Pattern, String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() < 2 || chars.len() > 3 {
        return Err(format!("invalid hand '{s}'"));
    }
    let r1 = parse_rank(chars[0]).ok_or_else(|| format!("invalid rank in '{s}'"))?;
    let r2 = parse_rank(chars[1]).ok_or_else(|| format!("invalid rank in '{s}'"))?;
    let (hi, lo) = if r1 >= r2 { (r1, r2) } else { (r2, r1) };
    let kind = match (hi == lo, chars.get(2).map(|c| c.to_ascii_lowercase())) {
        (true, None) => Kind::Pair,
        (true, Some(_)) => return Err(format!("pairs cannot be suited/offsuit: '{s}'")),
        (false, None) => Kind::Both,
        (false, Some('s')) => Kind::Suited,
        (false, Some('o')) => Kind::Offsuit,
        _ => return Err(format!("invalid hand '{s}'")),
    };
    Ok(Pattern { hi, lo, kind })
}

fn pattern_combos(p: Pattern, out: &mut Vec<usize>) {
    let kinds: &[bool] = match p.kind {
        Kind::Pair => &[false],
        Kind::Suited => &[true],
        Kind::Offsuit => &[false],
        Kind::Both => &[true, false],
    };
    for &suited in kinds {
        out.extend(class_combos(class_index(p.hi, p.lo, suited)));
    }
}

fn parse_token(tok: &str) -> Result<Vec<usize>, String> {
    let lower = tok.to_ascii_lowercase();
    if matches!(lower.as_str(), "any" | "all" | "random" | "100%") {
        return Ok((0..NUM_COMBOS).collect());
    }
    let mut out = Vec::new();
    let chars: Vec<char> = tok.chars().collect();
    // Specific combo such as "AsKh".
    if chars.len() == 4
        && chars[1].is_ascii_alphabetic()
        && chars[3].is_ascii_alphabetic()
        && crate::card::parse_suit(chars[1]).is_some()
        && crate::card::parse_suit(chars[3]).is_some()
    {
        let cards = parse_cards(tok)?;
        out.push(combo_index(cards[0], cards[1]));
        return Ok(out);
    }
    if let Some((a, b)) = tok.split_once('-') {
        let (a, b) = (parse_pattern(a)?, parse_pattern(b)?);
        if a.kind != b.kind {
            return Err(format!("mismatched hand types in '{tok}'"));
        }
        if a.kind == Kind::Pair {
            let (lo, hi) = (a.hi.min(b.hi), a.hi.max(b.hi));
            for r in lo..=hi {
                pattern_combos(Pattern { hi: r, lo: r, kind: Kind::Pair }, &mut out);
            }
        } else {
            if a.hi != b.hi {
                return Err(format!("range '{tok}' must share the high card"));
            }
            let (lo, hi) = (a.lo.min(b.lo), a.lo.max(b.lo));
            for k in lo..=hi {
                pattern_combos(Pattern { hi: a.hi, lo: k, kind: a.kind }, &mut out);
            }
        }
        return Ok(out);
    }
    if let Some(base) = tok.strip_suffix('+') {
        let p = parse_pattern(base)?;
        if p.kind == Kind::Pair {
            for r in p.hi..=12 {
                pattern_combos(Pattern { hi: r, lo: r, kind: Kind::Pair }, &mut out);
            }
        } else {
            for k in p.lo..p.hi {
                pattern_combos(Pattern { hi: p.hi, lo: k, kind: p.kind }, &mut out);
            }
        }
        return Ok(out);
    }
    pattern_combos(parse_pattern(tok)?, &mut out);
    Ok(out)
}

fn parse_weight(s: &str) -> Result<f32, String> {
    let s = s.trim().trim_end_matches('%');
    let w: f32 = s.parse().map_err(|_| format!("invalid weight '{s}'"))?;
    let w = if w > 1.0 { w / 100.0 } else { w };
    if !(0.0..=1.0).contains(&w) {
        return Err(format!("weight out of range '{s}'"));
    }
    Ok(w)
}

impl Range {
    pub fn parse(s: &str) -> Result<Range, String> {
        let mut range = Range::default();
        for raw in s.split([',', '\n', ';']) {
            let tok: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
            if tok.is_empty() {
                continue;
            }
            let (body, weight) = match tok.split_once(':') {
                Some((b, w)) => (b.to_string(), parse_weight(w)?),
                None => (tok.clone(), 1.0),
            };
            for combo in parse_token(&body)? {
                range.weights[combo] = weight;
            }
        }
        Ok(range)
    }

    pub fn full() -> Range {
        Range { weights: vec![1.0; NUM_COMBOS] }
    }

    /// Builds a range from per-class weights.
    pub fn from_class_weights(class_weights: &[f32]) -> Range {
        let mut range = Range::default();
        for (combo, w) in range.weights.iter_mut().enumerate() {
            *w = class_weights[combo_class(combo)];
        }
        range
    }

    /// Average weight of each class's combos.
    pub fn class_weights(&self) -> Vec<f32> {
        let mut sum = vec![0.0f32; NUM_CLASSES];
        for (combo, &w) in self.weights.iter().enumerate() {
            sum[combo_class(combo)] += w;
        }
        (0..NUM_CLASSES).map(|c| sum[c] / class_num_combos(c) as f32).collect()
    }

    /// Total weighted number of combos (out of 1326).
    pub fn num_combos(&self) -> f64 {
        self.weights.iter().map(|&w| w as f64).sum()
    }

    /// Weighted number of combos that do not conflict with `dead` cards.
    pub fn num_combos_excluding(&self, dead: &[Card]) -> f64 {
        let dead_mask = dead.iter().fold(0, |m, &c| m | card_bit(c));
        self.weights
            .iter()
            .enumerate()
            .filter(|&(i, _)| crate::card::combo_mask(i) & dead_mask == 0)
            .map(|(_, &w)| w as f64)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.weights.iter().all(|&w| w <= 0.0)
    }

    /// Compact string form: whole classes where all combos share a weight,
    /// otherwise individual combos.
    pub fn to_range_string(&self) -> String {
        let mut parts = Vec::new();
        for class in 0..NUM_CLASSES {
            let combos = class_combos(class);
            let w0 = self.weights[combos[0]];
            let uniform = combos.iter().all(|&c| (self.weights[c] - w0).abs() < 1e-6);
            if uniform {
                if w0 > 0.0 {
                    parts.push(fmt_weighted(&class_name(class), w0));
                }
            } else {
                for &c in &combos {
                    if self.weights[c] > 0.0 {
                        parts.push(fmt_weighted(&crate::card::combo_to_string(c), self.weights[c]));
                    }
                }
            }
        }
        parts.join(",")
    }
}

fn fmt_weighted(name: &str, w: f32) -> String {
    if (w - 1.0).abs() < 1e-6 {
        name.to_string()
    } else {
        let s = format!("{:.3}", w);
        format!("{name}:{}", s.trim_end_matches('0').trim_end_matches('.'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn class_layout() {
        assert_eq!(class_name(0), "AA");
        assert_eq!(class_name(1), "AKs");
        assert_eq!(class_name(13), "AKo");
        assert_eq!(class_name(168), "22");
        let total: usize = (0..NUM_CLASSES).map(class_num_combos).sum();
        assert_eq!(total, NUM_COMBOS);
        for c in 0..NUM_CLASSES {
            let combos = class_combos(c);
            assert_eq!(combos.len(), class_num_combos(c));
            assert!(combos.iter().all(|&x| combo_class(x) == c));
            let (h, l, s) = class_ranks(c);
            assert_eq!(class_index(h, l, s), c);
        }
    }

    #[test]
    fn parse_ranges() {
        let r = Range::parse("22+").unwrap();
        assert_eq!(r.num_combos(), 78.0);
        let r = Range::parse("ATs+").unwrap();
        assert_eq!(r.num_combos(), 16.0);
        let r = Range::parse("AT+").unwrap();
        assert_eq!(r.num_combos(), 64.0);
        let r = Range::parse("A2s-A5s, KQo").unwrap();
        assert_eq!(r.num_combos(), 28.0);
        let r = Range::parse("22-44").unwrap();
        assert_eq!(r.num_combos(), 18.0);
        let r = Range::parse("AsKh, AA:0.5").unwrap();
        assert_eq!(r.num_combos(), 4.0);
        let r = Range::parse("KK:50").unwrap();
        assert_eq!(r.num_combos(), 3.0);
        let r = Range::parse("any").unwrap();
        assert_eq!(r.num_combos(), 1326.0);
        let r = Range::parse("22+, 22:0").unwrap();
        assert_eq!(r.num_combos(), 72.0);
        assert!(Range::parse("AAs").is_err());
        assert!(Range::parse("AKs-QJs").is_err());
        assert!(Range::parse("XY").is_err());
    }

    #[test]
    fn roundtrip_string() {
        let r = Range::parse("AA,KK:0.5,AKs,AsKh,T9o:0.25").unwrap();
        let back = Range::parse(&r.to_range_string()).unwrap();
        assert_eq!(r, back);
    }
}
