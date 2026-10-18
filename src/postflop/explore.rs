//! Navigating a solved postflop tree: lines, per-hand strategies, EV and equity.
//!
//! The solver only stores representative turn/river cards when suit
//! isomorphism is on. The explorer keeps the cards the user actually dealt and
//! a suit permutation into "representative space", so isomorphic cards work
//! transparently.

use std::io::{BufRead, Write};

use crate::card::{card_to_string, combo_index, parse_card, permute_card, rank, suit, Card};
use crate::display::{self, bold, dim, paint, pct1, ActionKind};
use crate::postflop::game::PostflopGame;
use crate::postflop::solver::{Dealt, Mode};
use crate::postflop::tree::{fmt_chips, pair_suits, Action, Node, NodeKind, NONE};
use crate::range::{class_name, combo_class, NUM_CLASSES};

pub const PLAYER_NAMES: [&str; 2] = ["OOP", "IP"];

#[derive(Clone)]
pub struct Frame {
    pub node: usize,
    /// Cards dealt after the starting board, in representative space.
    pub dealt: Dealt,
    /// The board as the user dealt it.
    pub board: Vec<Card>,
    /// Maps an actual suit to its representative suit.
    pub perm: [u8; 4],
    /// Reach of every hand of each player (representative space).
    pub reach: [Vec<f32>; 2],
    pub line: Vec<String>,
}

pub struct Explorer<'a> {
    pub game: &'a PostflopGame,
    pub frames: Vec<Frame>,
}

/// Per-hand figures for one player at a node, indexed like `game.hands[p]`.
pub struct HandStats {
    /// Expected value in chips (pot share minus future investment).
    pub ev: Vec<f32>,
    /// All-in equity against the opponent's range at this node.
    pub equity: Vec<f32>,
}

/// Short code of an action for grids, e.g. `X`, `C`, `F`, `B1`, `R`, `A`.
pub fn action_codes(actions: &[Action]) -> Vec<String> {
    let aggressive: Vec<usize> = actions
        .iter()
        .enumerate()
        .filter(|(_, a)| matches!(a, Action::Bet(_) | Action::Raise(_)))
        .map(|(i, _)| i)
        .collect();
    actions
        .iter()
        .enumerate()
        .map(|(i, a)| match a {
            Action::Fold => "F".into(),
            Action::Check => "X".into(),
            Action::Call => "C".into(),
            Action::AllIn(_) => "A".into(),
            Action::Bet(_) | Action::Raise(_) => {
                let letter = if matches!(a, Action::Bet(_)) { "B" } else { "R" };
                if aggressive.len() > 1 {
                    format!("{letter}{}", aggressive.iter().position(|&x| x == i).unwrap() + 1)
                } else {
                    letter.into()
                }
            }
            _ => "?".into(),
        })
        .collect()
}

pub fn action_kinds(actions: &[Action]) -> Vec<ActionKind> {
    let mut level = 0;
    actions
        .iter()
        .map(|a| match a {
            Action::Fold => ActionKind::Fold,
            Action::Check | Action::Call => ActionKind::Passive,
            Action::AllIn(_) => ActionKind::Aggressive { level: 0, allin: true },
            _ => {
                level += 1;
                ActionKind::Aggressive { level: level - 1, allin: false }
            }
        })
        .collect()
}

fn swap_suit(s: u8, pair: usize) -> u8 {
    let (a, b) = pair_suits(pair);
    if s == a {
        b
    } else if s == b {
        a
    } else {
        s
    }
}

impl<'a> Explorer<'a> {
    pub fn new(game: &'a PostflopGame) -> Self {
        let root = Frame {
            node: 0,
            dealt: Dealt::default(),
            board: game.board.clone(),
            perm: [0, 1, 2, 3],
            reach: game.initial_weights.clone(),
            line: Vec::new(),
        };
        Explorer { game, frames: vec![root] }
    }

    pub fn frame(&self) -> &Frame {
        self.frames.last().unwrap()
    }

    pub fn node(&self) -> &Node {
        &self.game.nodes[self.frame().node]
    }

    pub fn back(&mut self) -> bool {
        if self.frames.len() > 1 {
            self.frames.pop();
            true
        } else {
            false
        }
    }

    pub fn child_actions(&self) -> Vec<Action> {
        self.node().children().map(|c| self.game.nodes[c].action).collect()
    }

    /// Average strategy at the current player node (`actions x hands`).
    pub fn strategy(&self) -> Vec<f32> {
        self.game.average_strategy(self.node())
    }

    /// Applies a typed token: an action number (1-based), an action token
    /// (`x`, `c`, `f`, `a`, `b<amount>`, `r<amount>`) or a card.
    pub fn apply(&mut self, token: &str) -> Result<(), String> {
        let node = self.node().clone();
        match node.kind {
            NodeKind::Chance => {
                let card = parse_card(token)?;
                self.deal(card)
            }
            NodeKind::Player(_) => {
                let actions = self.child_actions();
                let t = token.to_ascii_lowercase();
                let idx = if let Ok(n) = t.parse::<usize>() {
                    (n >= 1 && n <= actions.len()).then(|| n - 1)
                } else {
                    match_action(&actions, &t)
                };
                let i = idx.ok_or_else(|| format!("no action '{token}' here"))?;
                self.take_action(i);
                Ok(())
            }
            _ => Err("this is a terminal node; use 'u' to go back".into()),
        }
    }

    pub fn take_action(&mut self, i: usize) {
        let node = self.node().clone();
        let NodeKind::Player(q) = node.kind else { return };
        let q = q as usize;
        let strat = self.strategy();
        let nh = self.game.num_hands(q);
        let mut f = self.frame().clone();
        for h in 0..nh {
            f.reach[q][h] *= strat[i * nh + h];
        }
        let child = node.first_child as usize + i;
        f.node = child;
        f.line.push(format!("{} {}", PLAYER_NAMES[q], self.game.nodes[child].action.label()));
        self.frames.push(f);
    }

    pub fn deal(&mut self, actual: Card) -> Result<(), String> {
        let f = self.frame().clone();
        if f.board.contains(&actual) {
            return Err(format!("{} is already on the board", card_to_string(actual)));
        }
        let node = self.node().clone();
        let mapped = permute_card(actual, &f.perm);
        let direct = node.children().find(|&c| self.game.nodes[c].action == Action::Deal(mapped));
        let (child, pair) = match direct {
            Some(c) => (c, None),
            None => {
                let table = (node.iso != NONE).then(|| &self.game.iso_tables[node.iso as usize]);
                let e = table
                    .and_then(|t| t.iter().find(|e| e.card == mapped))
                    .ok_or_else(|| format!("cannot deal {}", card_to_string(actual)))?;
                (e.rep_child as usize, Some(e.pair as usize))
            }
        };
        let Action::Deal(rep_card) = self.game.nodes[child].action else { unreachable!() };
        let mut next = f.clone();
        if let Some(pair) = pair {
            next.perm = f.perm.map(|s| swap_suit(s, pair));
            for p in 0..2 {
                let map = &self.game.swap_maps[p][pair];
                let mut r = vec![0.0; f.reach[p].len()];
                for (h, &x) in f.reach[p].iter().enumerate() {
                    r[map[h] as usize] = x;
                }
                next.reach[p] = r;
            }
        }
        for p in 0..2 {
            for &h in &self.game.card_hands[p][rep_card as usize] {
                next.reach[p][h as usize] = 0.0;
            }
        }
        next.node = child;
        next.dealt = f.dealt.push(rep_card);
        next.board.push(actual);
        next.line.push(card_to_string(actual));
        self.frames.push(next);
        Ok(())
    }

    /// Hand index (representative space) of an actual combo for player `p`.
    pub fn hand_of(&self, p: usize, a: Card, b: Card) -> Option<usize> {
        let perm = &self.frame().perm;
        let h = self.game.hand_index[p][combo_index(permute_card(a, perm), permute_card(b, perm))];
        (h != NONE).then_some(h as usize)
    }

    /// The actual combo shown for a representative hand index.
    pub fn actual_combo(&self, p: usize, h: usize) -> (Card, Card) {
        let perm = &self.frame().perm;
        let mut inv = [0u8; 4];
        for (s, &t) in perm.iter().enumerate() {
            inv[t as usize] = s as u8;
        }
        let (a, b) = self.game.hands[p][h];
        (permute_card(a, &inv), permute_card(b, &inv))
    }

    fn valid_opp(&self, p: usize) -> Vec<f32> {
        let f = self.frame();
        let o = 1 - p;
        let mut cs = [0.0f32; 52];
        let mut total = 0.0;
        for (&r, &(a, b)) in f.reach[o].iter().zip(&self.game.hands[o]) {
            cs[a as usize] += r;
            cs[b as usize] += r;
            total += r;
        }
        self.game.hands[p]
            .iter()
            .enumerate()
            .map(|(h, &(a, b))| {
                let s = self.game.same_hand[p][h];
                let same = if s == NONE { 0.0 } else { f.reach[o][s as usize] };
                total - cs[a as usize] - cs[b as usize] + same
            })
            .collect()
    }

    pub fn hand_stats(&self, p: usize) -> HandStats {
        let f = self.frame();
        let node = self.node();
        let cfv = self.game.recurse(f.node, p, &f.reach[p], &f.reach[1 - p], f.dealt, Mode::Average);
        let valid = self.valid_opp(p);
        let ev = cfv
            .iter()
            .zip(&valid)
            .map(|(&v, &w)| if w > 0.0 { v / w + node.contrib[p] } else { f32::NAN })
            .collect();
        HandStats { ev, equity: self.equity(p) }
    }

    /// All-in equity of each of `p`'s hands against the opponent's reach,
    /// averaged over every remaining runout.
    pub fn equity(&self, p: usize) -> Vec<f32> {
        let f = self.frame();
        let o = 1 - p;
        let mut used: Vec<Card> = self.game.board.clone();
        used.extend_from_slice(f.dealt.as_slice());
        let deck: Vec<Card> = (0..52u8).filter(|c| !used.contains(c)).collect();
        let mut runouts: Vec<Vec<Card>> = Vec::new();
        match 5 - used.len() {
            0 => runouts.push(vec![]),
            1 => runouts.extend(deck.iter().map(|&c| vec![c])),
            _ => {
                for i in 0..deck.len() {
                    for j in i + 1..deck.len() {
                        runouts.push(vec![deck[i], deck[j]]);
                    }
                }
            }
        }
        let nh = self.game.num_hands(p);
        let mut num = vec![0.0f64; nh];
        let mut den = vec![0.0f64; nh];
        for extra in runouts {
            let mut d = f.dealt;
            for &c in &extra {
                d = d.push(c);
            }
            let ev = self.game.river_eval(d.as_slice());
            let (hero, vill) = (&ev.sorted[p], &ev.sorted[o]);
            let mut cs = [0.0f64; 52];
            let mut total = 0.0f64;
            for v in vill {
                let r = f.reach[o][v.idx as usize] as f64;
                cs[v.cards[0] as usize] += r;
                cs[v.cards[1] as usize] += r;
                total += r;
            }
            let mut j = 0;
            let mut acc = 0.0f64;
            let mut acc_card = [0.0f64; 52];
            let mut win = vec![0.0f64; hero.len()];
            for (k, (e, &end)) in hero.iter().zip(&ev.weaker[p]).enumerate() {
                while j < end as usize {
                    let v = &vill[j];
                    let r = f.reach[o][v.idx as usize] as f64;
                    acc += r;
                    acc_card[v.cards[0] as usize] += r;
                    acc_card[v.cards[1] as usize] += r;
                    j += 1;
                }
                win[k] = acc - acc_card[e.cards[0] as usize] - acc_card[e.cards[1] as usize];
            }
            let mut j = vill.len();
            let mut acc = 0.0f64;
            let mut acc_card = [0.0f64; 52];
            for (k, (e, &start)) in hero.iter().zip(&ev.stronger[p]).enumerate().rev() {
                while j > start as usize {
                    j -= 1;
                    let v = &vill[j];
                    let r = f.reach[o][v.idx as usize] as f64;
                    acc += r;
                    acc_card[v.cards[0] as usize] += r;
                    acc_card[v.cards[1] as usize] += r;
                }
                let [a, b] = e.cards;
                let lose = acc - acc_card[a as usize] - acc_card[b as usize];
                let h = e.idx as usize;
                let s = self.game.same_hand[p][h];
                let same = if s == NONE { 0.0 } else { f.reach[o][s as usize] as f64 };
                let valid = total - cs[a as usize] - cs[b as usize] + same;
                num[h] += win[k] + 0.5 * (valid - win[k] - lose);
                den[h] += valid;
            }
        }
        num.iter().zip(&den).map(|(&n, &d)| if d > 0.0 { (n / d) as f32 } else { f32::NAN }).collect()
    }

    /// Weighted number of combos each player holds at this node.
    pub fn range_size(&self, p: usize) -> f64 {
        self.frame().reach[p].iter().map(|&r| r as f64).sum()
    }

    /// Per-class reach-weighted average of a per-hand vector.
    pub fn class_average(&self, p: usize, values: &[f32]) -> Vec<Option<f64>> {
        let reach = &self.frame().reach[p];
        let mut num = vec![0.0f64; NUM_CLASSES];
        let mut den = vec![0.0f64; NUM_CLASSES];
        for (h, &(a, b)) in self.game.hands[p].iter().enumerate() {
            let r = reach[h] as f64;
            if r > 0.0 && values[h].is_finite() {
                let c = combo_class(combo_index(a, b));
                num[c] += r * values[h] as f64;
                den[c] += r;
            }
        }
        (0..NUM_CLASSES).map(|c| (den[c] > 1e-9).then(|| num[c] / den[c])).collect()
    }

    /// Reach-weighted frequency of each action for each class, plus overall.
    pub fn class_frequencies(&self) -> (Vec<Option<Vec<f64>>>, Vec<f64>) {
        let NodeKind::Player(q) = self.node().kind else { return (Vec::new(), Vec::new()) };
        let q = q as usize;
        let strat = self.strategy();
        let na = self.node().num_children as usize;
        let nh = self.game.num_hands(q);
        let reach = &self.frame().reach[q];
        let mut num = vec![vec![0.0f64; na]; NUM_CLASSES];
        let mut den = vec![0.0f64; NUM_CLASSES];
        let mut overall = vec![0.0f64; na];
        let mut total = 0.0;
        for (h, &(a, b)) in self.game.hands[q].iter().enumerate() {
            let r = reach[h] as f64;
            if r <= 0.0 {
                continue;
            }
            let c = combo_class(combo_index(a, b));
            for i in 0..na {
                let x = r * strat[i * nh + h] as f64;
                num[c][i] += x;
                overall[i] += x;
            }
            den[c] += r;
            total += r;
        }
        let per_class = (0..NUM_CLASSES)
            .map(|c| (den[c] > 1e-9).then(|| num[c].iter().map(|x| x / den[c]).collect()))
            .collect();
        if total > 0.0 {
            overall.iter_mut().for_each(|x| *x /= total);
        }
        (per_class, overall)
    }
}

fn match_action(actions: &[Action], t: &str) -> Option<usize> {
    let find = |pred: &dyn Fn(&Action) -> bool| actions.iter().position(pred);
    match t {
        "f" | "fold" => find(&|a| *a == Action::Fold),
        "x" | "k" | "check" => find(&|a| *a == Action::Check),
        "c" | "call" => find(&|a| *a == Action::Call),
        "a" | "allin" | "all-in" | "jam" => find(&|a| matches!(a, Action::AllIn(_))),
        _ => {
            let (kind, amount) = t.split_at(1);
            let want_bet = match kind {
                "b" => true,
                "r" => false,
                _ => return None,
            };
            let candidates: Vec<(usize, f32)> = actions
                .iter()
                .enumerate()
                .filter_map(|(i, a)| match (a, want_bet) {
                    (Action::Bet(x), true) | (Action::Raise(x), false) => Some((i, *x)),
                    (Action::AllIn(x), _) => Some((i, *x)),
                    _ => None,
                })
                .collect();
            if amount.is_empty() {
                // A bare "b"/"r" picks the smallest size.
                return candidates.first().map(|&(i, _)| i);
            }
            let want: f32 = amount.parse().ok()?;
            candidates
                .iter()
                .min_by(|x, y| (x.1 - want).abs().partial_cmp(&(y.1 - want).abs()).unwrap())
                .map(|&(i, _)| i)
        }
    }
}

// ---------------------------------------------------------------------------
// Rendering

pub fn node_header(ex: &Explorer) -> String {
    let g = ex.game;
    let f = ex.frame();
    let node = ex.node();
    let half = g.config.pot as f32 / 2.0;
    let behind = |p: usize| g.config.stack as f32 - (node.contrib[p] - half);
    let board: Vec<String> = f.board.iter().map(|&c| card_to_string(c)).collect();
    let line = if f.line.is_empty() { "(root)".to_string() } else { f.line.join(" > ") };
    let mut s = String::new();
    s.push_str(&format!("{} {}\n", bold("Line:"), line));
    s.push_str(&format!(
        "{} {}   {} {}   {} {} / {}\n",
        bold("Board:"),
        board.join(" "),
        bold("Pot:"),
        fmt_chips(node.pot()),
        bold("Behind:"),
        fmt_chips(behind(0)),
        fmt_chips(behind(1)),
    ));
    s.push_str(&format!(
        "{} OOP {:.1} combos, IP {:.1} combos\n",
        bold("Ranges:"),
        ex.range_size(0),
        ex.range_size(1)
    ));
    s
}

pub fn render_node(ex: &Explorer) -> String {
    let mut s = node_header(ex);
    let node = ex.node();
    match node.kind {
        NodeKind::Player(q) => {
            let q = q as usize;
            let actions = ex.child_actions();
            let codes = action_codes(&actions);
            let kinds = action_kinds(&actions);
            let (per_class, overall) = ex.class_frequencies();
            s.push_str(&format!("{} to act:\n", bold(PLAYER_NAMES[q])));
            for (i, a) in actions.iter().enumerate() {
                let tag = paint(&format!(" {:<2} ", codes[i]), kinds[i]);
                s.push_str(&format!("  [{}] {} {:<16} {:>6}\n", i + 1, tag, a.label(), pct1(overall[i])));
            }
            s.push('\n');
            s.push_str(&display::grid(|c| match &per_class[c] {
                None => dim(&format!("{:<6}", class_name(c))),
                Some(freqs) => {
                    let (best, f) = freqs
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                        .map(|(i, &f)| (i, f))
                        .unwrap();
                    paint(&format!("{:<2}{:>3} ", codes[best], (f * 100.0).round() as i64), kinds[best])
                }
            }));
            s.push_str(&dim("cells: most frequent action and its frequency; 'grid N' shows action N\n"));
        }
        NodeKind::Chance => {
            let street = if node.board_len == 3 { "turn" } else { "river" };
            s.push_str(&format!("Chance node: deal the {street} card (e.g. 'Kh').\n"));
        }
        NodeKind::Fold(f) => {
            s.push_str(&format!("Terminal: {} folds.\n", PLAYER_NAMES[f as usize]));
        }
        NodeKind::Showdown => s.push_str("Terminal: showdown.\n"),
    }
    s
}

pub fn render_action_grid(ex: &Explorer, i: usize) -> Result<String, String> {
    let actions = ex.child_actions();
    if !matches!(ex.node().kind, NodeKind::Player(_)) || i >= actions.len() {
        return Err("no such action here".into());
    }
    let (per_class, overall) = ex.class_frequencies();
    let kinds = action_kinds(&actions);
    let mut s = format!("{} ({} overall)\n", actions[i].label(), pct1(overall[i]));
    s.push_str(&display::grid(|c| match &per_class[c] {
        None => dim("  .  "),
        Some(fr) => {
            let v = (fr[i] * 100.0).round() as i64;
            let txt = format!("{:>4} ", v);
            if fr[i] >= 0.5 {
                paint(&txt, kinds[i])
            } else {
                txt
            }
        }
    }));
    Ok(s)
}

pub fn render_value_grid(ex: &Explorer, p: usize, values: &[f32], label: &str, as_pct: bool) -> String {
    let avg = ex.class_average(p, values);
    let mut s = format!("{label} for {}\n", PLAYER_NAMES[p]);
    s.push_str(&display::grid(|c| match avg[c] {
        None => dim("   .  "),
        Some(v) if as_pct => format!("{:>5.1} ", v * 100.0),
        Some(v) => format!("{:>5.1} ", v),
    }));
    s
}

/// Table of each hand for the player to act: strategy, EV and equity.
pub fn render_hands(ex: &Explorer, filter: Option<&str>) -> String {
    let node = ex.node();
    let (p, actions) = match node.kind {
        NodeKind::Player(q) => (q as usize, ex.child_actions()),
        _ => (0, Vec::new()),
    };
    let strat = if actions.is_empty() { Vec::new() } else { ex.strategy() };
    let stats = ex.hand_stats(p);
    let nh = ex.game.num_hands(p);
    let codes = action_codes(&actions);
    let reach = &ex.frame().reach[p];
    let mut rows: Vec<(usize, usize, String)> = Vec::new();
    for h in 0..nh {
        if reach[h] <= 0.0 {
            continue;
        }
        let (a, b) = ex.actual_combo(p, h);
        let (a, b) =
            if rank(a) > rank(b) || (rank(a) == rank(b) && suit(a) > suit(b)) { (a, b) } else { (b, a) };
        let name = format!("{}{}", card_to_string(a), card_to_string(b));
        let class = combo_class(combo_index(a, b));
        if let Some(fl) = filter {
            let cn = class_name(class);
            if !name.eq_ignore_ascii_case(fl)
                && !cn.eq_ignore_ascii_case(fl)
                && !cn.to_ascii_lowercase().starts_with(&fl.to_ascii_lowercase())
            {
                continue;
            }
        }
        let mut line = format!("{:<6}{:>6.3}", name, reach[h]);
        for i in 0..actions.len() {
            line.push_str(&format!(" {:>6.1}", 100.0 * strat[i * nh + h]));
        }
        line.push_str(&format!(" {:>8.2} {:>7.1}", stats.ev[h], 100.0 * stats.equity[h]));
        rows.push((class, h, line));
    }
    rows.sort_by_key(|r| (r.0, r.1));
    let mut s = format!("{} hands ({}):\n", PLAYER_NAMES[p], rows.len());
    let mut header = format!("{:<6}{:>6}", "hand", "weight");
    for c in &codes {
        header.push_str(&format!(" {:>6}", format!("{c}%")));
    }
    header.push_str(&format!(" {:>8} {:>7}", "EV", "EQ%"));
    s.push_str(&bold(&header));
    s.push('\n');
    for (_, _, l) in rows {
        s.push_str(&l);
        s.push('\n');
    }
    s
}

const HELP: &str = "\
Commands:
  <n> | x c f a b<amt> r<amt>  take an action (by number or token, e.g. b3.3)
  <card>                       deal the turn/river card, e.g. Kh
  u                            undo the last step          root   back to the root
  grid <n>                     frequency grid of action n
  hands [filter]               per-hand strategy, EV and equity (e.g. 'hands AK', 'hands AsKd')
  ev [oop|ip]                  EV grid (chips)              eq [oop|ip]   equity grid
  range [oop|ip]               reach-weighted range grid
  help                         this help                    q      quit";

pub fn run_repl(game: &PostflopGame) {
    let mut ex = Explorer::new(game);
    println!("{}", render_node(&ex));
    println!("{}", dim("type 'help' for commands"));
    let stdin = std::io::stdin();
    loop {
        print!("> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        let Some(&cmd) = words.first() else { continue };
        let arg = words.get(1).copied();
        let who = |default: usize| match arg.map(|a| a.to_ascii_lowercase()) {
            Some(a) if a == "oop" => 0,
            Some(a) if a == "ip" => 1,
            _ => default,
        };
        let acting = match ex.node().kind {
            NodeKind::Player(q) => q as usize,
            _ => 0,
        };
        match cmd.to_ascii_lowercase().as_str() {
            "q" | "quit" | "exit" => break,
            "help" | "h" | "?" => println!("{HELP}"),
            "u" | "back" | "undo" => {
                if ex.back() {
                    println!("{}", render_node(&ex));
                }
            }
            "root" => {
                ex.frames.truncate(1);
                println!("{}", render_node(&ex));
            }
            "grid" => match arg.and_then(|a| a.parse::<usize>().ok()) {
                Some(n) if n >= 1 => match render_action_grid(&ex, n - 1) {
                    Ok(s) => println!("{s}"),
                    Err(e) => println!("{e}"),
                },
                _ => println!("usage: grid <action number>"),
            },
            "hands" => println!("{}", render_hands(&ex, arg)),
            "ev" => {
                let p = who(acting);
                let st = ex.hand_stats(p);
                println!("{}", render_value_grid(&ex, p, &st.ev, "EV (chips)", false));
            }
            "eq" => {
                let p = who(acting);
                println!("{}", render_value_grid(&ex, p, &ex.equity(p), "Equity %", true));
            }
            "range" => println!("{}", render_range(&ex, who(acting))),
            _ => match ex.apply(cmd) {
                Ok(()) => println!("{}", render_node(&ex)),
                Err(e) => println!("{e}"),
            },
        }
    }
}

/// Grid of the fraction of each class's combos still in `p`'s range.
pub fn render_range(ex: &Explorer, p: usize) -> String {
    let reach = &ex.frame().reach[p];
    let mut num = vec![0.0f64; NUM_CLASSES];
    let mut cnt = vec![0usize; NUM_CLASSES];
    for (h, &(a, b)) in ex.game.hands[p].iter().enumerate() {
        let c = combo_class(combo_index(a, b));
        num[c] += reach[h] as f64;
        cnt[c] += 1;
    }
    let mut s =
        format!("{} range ({:.1} combos), % of each hand's live combos\n", PLAYER_NAMES[p], ex.range_size(p));
    s.push_str(&display::grid(|c| {
        if cnt[c] == 0 || num[c] < 1e-6 {
            dim("  .  ")
        } else {
            format!("{:>4} ", (100.0 * num[c] / cnt[c] as f64).round() as i64)
        }
    }));
    s
}
