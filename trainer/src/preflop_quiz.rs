//! Preflop drill: a full 6-max hand where everyone else plays the solution.
//!
//! Works for the 100bb cash pack and for push/fold packs alike.

use std::sync::Arc;

use holdem_solver::card::{Card, Rng, card_bit, combo_index, combo_to_string};
use holdem_solver::eval::{describe_long, eval_mask};
use holdem_solver::postflop::tree::fmt_chips;
use holdem_solver::preflop::{PAction, PKind};
use holdem_solver::range::{NUM_CLASSES, class_name, combo_class};

use crate::grading::by_frequency;
use crate::model::LoadedPreflop;
use crate::quiz::{Feedback, LogEntry, MIN_REACH, RangeGrid, SeatView, shuffled_deck, skip_obvious};

#[derive(Clone, Debug)]
pub struct PreflopDecision {
    pub actions: Vec<PAction>,
    pub labels: Vec<String>,
    pub freqs: Vec<f32>,
    pub category: String,
}

#[derive(Clone, Debug)]
pub enum PreflopEnd {
    /// Everyone else folded.
    Won { winner: usize },
    /// All-in: the board ran out.
    Showdown { board: Vec<Card>, winners: Vec<usize>, descriptions: Vec<(usize, String)> },
    /// The betting closed; the hand would continue on the flop.
    SeesFlop { players: Vec<usize> },
    /// The hero reached a decision the solution never reaches with this hand
    /// (own reach below [`MIN_REACH`]), so it has no strategy to grade against.
    OffTree,
}

pub struct PreflopHand {
    pub pack: Arc<LoadedPreflop>,
    pub hero: usize,
    pub cards: Vec<(Card, Card)>,
    pub node: usize,
    pub log: Vec<LogEntry>,
    pub last_action: Vec<Option<String>>,
    pub end: Option<PreflopEnd>,
    /// Hero decisions made so far in this hand.
    pub hero_decisions: u32,
    deck: Vec<Card>,
}

/// Label of an action; `to_call` is what the actor still has to put in to call.
fn action_label(a: PAction, to_call: f32) -> String {
    match a {
        PAction::Fold => "Fold".into(),
        PAction::Check => "Check".into(),
        PAction::Call if to_call > 1e-6 => format!("Call {}", fmt_chips(to_call)),
        PAction::Call => "Check".into(),
        PAction::Raise(x) => format!("Raise to {}", fmt_chips(x)),
        PAction::AllIn(x) => format!("All-in {}", fmt_chips(x)),
        PAction::None => String::new(),
    }
}

impl PreflopHand {
    /// Deals hands until the hero (one of `hero_seats`) faces a decision that
    /// passes the difficulty filter.
    pub fn deal(pack: Arc<LoadedPreflop>, hero_seats: &[usize], difficulty: u8, rng: &mut Rng) -> PreflopHand {
        let n = pack.pack.config.players;
        let seats: Vec<usize> = if hero_seats.is_empty() { (0..n).collect() } else { hero_seats.to_vec() };
        let mut fallback = None;
        for attempt in 0..500 {
            let hero = seats[rng.below(seats.len() as u32) as usize];
            let mut deck = shuffled_deck(rng);
            let cards: Vec<(Card, Card)> = (0..n).map(|_| (deck.pop().unwrap(), deck.pop().unwrap())).collect();
            let mut hand = PreflopHand {
                pack: pack.clone(),
                hero,
                cards,
                node: 0,
                log: Vec::new(),
                last_action: vec![None; n],
                end: None,
                hero_decisions: 0,
                deck,
            };
            hand.run_villains(rng);
            let Some(d) = hand.decision() else { continue };
            let fold = d.actions.iter().position(|a| *a == PAction::Fold);
            if attempt < 400 && skip_obvious(&d.freqs, fold, difficulty, rng) {
                fallback.get_or_insert(hand);
                continue;
            }
            return hand;
        }
        fallback.expect("some hand reaches the hero")
    }

    pub fn positions(&self) -> Vec<&'static str> {
        self.pack.pack.config.position_names()
    }

    pub fn hero_class(&self) -> usize {
        let (a, b) = self.cards[self.hero];
        combo_class(combo_index(a, b))
    }

    pub fn hero_hand_name(&self) -> String {
        class_name(self.hero_class())
    }

    fn node(&self) -> &holdem_solver::preflop::PNode {
        &self.pack.tree.nodes[self.node]
    }

    fn actions_at(&self, idx: usize) -> Vec<PAction> {
        self.pack.tree.nodes[idx].children().map(|c| self.pack.tree.nodes[c].action).collect()
    }

    /// Chips `seat` still has to put in to call at the current node.
    fn to_call(&self, seat: usize) -> f32 {
        let node = self.node();
        node.invested.iter().cloned().fold(0.0f32, f32::max) - node.invested[seat]
    }

    /// The hero's current decision, if it is the hero's turn.
    pub fn decision(&self) -> Option<PreflopDecision> {
        if self.end.is_some() {
            return None;
        }
        let node = self.node();
        let PKind::Decision(q) = node.kind else { return None };
        if q as usize != self.hero {
            return None;
        }
        let actions = self.actions_at(self.node);
        let to_call = self.to_call(self.hero);
        Some(PreflopDecision {
            labels: actions.iter().map(|&a| action_label(a, to_call)).collect(),
            freqs: self.pack.strategy(node, self.hero_class()),
            actions,
            category: self.category(),
        })
    }

    /// Category of the hero's current spot for stats, e.g. "RFI BTN", "BB vs BTN open",
    /// "BB vs CO open + call" (a squeeze spot), "BTN vs SB 3-bet" or "BTN cold vs 3-bet".
    pub fn category(&self) -> String {
        let pos = self.positions();
        let me = pos[self.hero];
        let acts = self.log_actions();
        // Indices of the raises in `acts`, and the calls between two indices.
        let at: Vec<usize> = (0..acts.len()).filter(|&i| matches!(acts[i].1, PAction::Raise(_) | PAction::AllIn(_))).collect();
        let calls = |from: usize, to: usize| acts[from..to].iter().filter(|(_, a)| *a == PAction::Call).count();
        let plus = |n: usize| if n > 1 { format!(" + {n} calls") } else { " + call".to_string() };
        let by = |k: usize| pos[acts[at[k]].0];
        let hero_raised = at.iter().any(|&i| acts[i].0 == self.hero);
        // Whether the hero has already put chips in voluntarily (an earlier hero action is never a fold).
        let hero_in = acts.iter().any(|&(s, _)| s == self.hero);
        let all_in = at.last().is_some_and(|&i| matches!(acts[i].1, PAction::AllIn(_)));
        let end = acts.len();
        if self.pack.pack.config.pushfold {
            let stack = fmt_chips(self.pack.nominal_stack() as f32);
            return match at.len() {
                0 => format!("Push {me} {stack}bb"),
                _ if calls(at[0], end) > 0 => format!("Call {me} vs {} jam{}", by(0), plus(calls(at[0], end))),
                _ => format!("Call {me} vs {} jam", by(0)),
            };
        }
        match at.len() {
            0 if calls(0, end) > 0 => format!("{me} vs limp"),
            0 => format!("RFI {me}"),
            1 if hero_in => format!("{me} limp vs {} raise", by(0)),
            1 if calls(at[0], end) > 0 => format!("{me} vs {} open{}", by(0), plus(calls(at[0], end))),
            1 => format!("{me} vs {} open", by(0)),
            // The hero opened: a 3-bet, or a squeeze when someone called in between.
            2 if hero_raised && calls(at[0], at[1]) > 0 => format!("{me} vs {} squeeze", by(1)),
            2 if hero_raised => format!("{me} vs {} 3-bet", by(1)),
            2 if hero_in => format!("{me} flat vs {} squeeze", by(1)),
            2 => format!("{me} cold vs 3-bet"),
            _ if !hero_in && all_in => format!("{me} cold vs all-in"),
            _ if !hero_in => format!("{me} cold vs 4-bet"),
            _ if all_in => format!("{me} vs all-in"),
            _ => format!("{me} vs 4-bet"),
        }
    }

    fn log_actions(&self) -> Vec<(usize, PAction)> {
        // Walk up from the current node to recover the action sequence.
        let mut out = Vec::new();
        let mut idx = self.node;
        while idx != 0 {
            let node = &self.pack.tree.nodes[idx];
            let parent = node.parent as usize;
            if let PKind::Decision(q) = self.pack.tree.nodes[parent].kind {
                out.push((q as usize, node.action));
            }
            idx = parent;
        }
        out.reverse();
        out
    }

    /// Lets the other players act until the hero's turn or the end of the hand.
    fn run_villains(&mut self, rng: &mut Rng) {
        loop {
            let node = self.node().clone();
            match node.kind {
                PKind::Decision(q) if q as usize == self.hero => {
                    // After an action the solution never takes with this hand, its
                    // strategy here was never trained: end the hand instead.
                    if self.pack.own_reach(self.node, self.hero)[self.hero_class()] < MIN_REACH {
                        self.end = Some(PreflopEnd::OffTree);
                    }
                    return;
                }
                PKind::Decision(q) => {
                    let q = q as usize;
                    let (a, b) = self.cards[q];
                    let freqs = self.pack.strategy(&node, combo_class(combo_index(a, b)));
                    let i = crate::quiz::sample_weighted(&freqs, rng).unwrap_or(0);
                    self.apply(q, i);
                }
                _ => {
                    self.finish(rng);
                    return;
                }
            }
        }
    }

    fn apply(&mut self, seat: usize, i: usize) {
        let to_call = self.to_call(seat);
        let child = self.node().first_child as usize + i;
        let action = self.pack.tree.nodes[child].action;
        let label = action_label(action, to_call);
        let pos = self.positions();
        self.log.push(LogEntry { street: "Preflop".into(), who: pos[seat].into(), what: label.clone(), hero: seat == self.hero });
        self.last_action[seat] = Some(label);
        self.node = child;
    }

    fn finish(&mut self, rng: &mut Rng) {
        let node = self.node().clone();
        let n = self.pack.pack.config.players;
        let active: Vec<usize> = (0..n).filter(|&p| node.active >> p & 1 == 1).collect();
        self.end = Some(match node.kind {
            PKind::Fold(w) => PreflopEnd::Won { winner: w as usize },
            PKind::AllIn => {
                let board: Vec<Card> = (0..5).map(|_| self.deck.remove(rng.below(self.deck.len() as u32) as usize)).collect();
                let bmask = board.iter().fold(0u64, |m, &c| m | card_bit(c));
                let ranks: Vec<(usize, u32)> = active
                    .iter()
                    .map(|&p| {
                        let (a, b) = self.cards[p];
                        (p, eval_mask(bmask | card_bit(a) | card_bit(b)))
                    })
                    .collect();
                let best = ranks.iter().map(|r| r.1).max().unwrap_or(0);
                PreflopEnd::Showdown {
                    board,
                    winners: ranks.iter().filter(|r| r.1 == best).map(|r| r.0).collect(),
                    descriptions: ranks.iter().map(|&(p, r)| (p, describe_long(r))).collect(),
                }
            }
            _ => PreflopEnd::SeesFlop { players: active },
        });
    }

    /// Answers the current decision with action `i`, then lets the hand continue.
    pub fn answer(&mut self, i: usize, rng: &mut Rng) -> Feedback {
        let d = self.decision().expect("hero to act");
        let grade = by_frequency(&d.freqs, i);
        let node = self.node().clone();
        let na = node.num_children as usize;
        let grid = RangeGrid {
            strategy: (0..NUM_CLASSES).map(|c| self.pack.strategy(&node, c)).collect(),
            reach: self.pack.own_reach(self.node, self.hero),
            hero_class: self.hero_class(),
        };
        debug_assert!(grid.strategy.iter().all(|g| g.len() == na));
        let (a, b) = self.cards[self.hero];
        let fb = Feedback {
            grade,
            chosen: i,
            actions: d.labels.clone(),
            freqs: d.freqs.clone(),
            // Off-range spots end the hand before the hero gets to answer.
            freqs_trusted: true,
            evs: None,
            ev_loss: 0.0,
            pot: node.pot,
            category: d.category.clone(),
            spot: self.spot_text(),
            hand: format!("{} ({})", combo_to_string(combo_index(a, b)), self.hero_hand_name()),
            grid: Some(grid),
        };
        self.hero_decisions += 1;
        self.apply(self.hero, i);
        self.run_villains(rng);
        fb
    }

    /// Short description of the hero's spot, e.g. "BTN, CO raise to 2.5, 100bb" or
    /// "BTN, you raise to 2.5, SB raise to 11, 100bb". Other players' folds are left out.
    pub fn spot_text(&self) -> String {
        let pos = self.positions();
        let acts: Vec<String> = self
            .log
            .iter()
            .filter(|e| e.hero || !e.what.starts_with("Fold"))
            .map(|e| format!("{} {}", if e.hero { "you" } else { e.who.as_str() }, e.what.to_lowercase()))
            .collect();
        let stack = fmt_chips(self.pack.nominal_stack() as f32);
        if acts.is_empty() {
            format!("{}, folded to you, {stack}bb", pos[self.hero])
        } else {
            format!("{}, {}, {stack}bb", pos[self.hero], acts.join(", "))
        }
    }

    pub fn pot(&self) -> f32 {
        self.node().pot
    }

    pub fn seats(&self) -> Vec<SeatView> {
        let cfg = &self.pack.pack.config;
        let node = self.node();
        let pos = self.positions();
        let to_act = match node.kind {
            PKind::Decision(q) => Some(q as usize),
            _ => None,
        };
        let showdown = matches!(self.end, Some(PreflopEnd::Showdown { .. }));
        let button = if cfg.players == 2 { 0 } else { cfg.players.saturating_sub(3) };
        (0..cfg.players)
            .map(|p| {
                let bet = node.invested[p] - cfg.ante as f32;
                let folded = node.active >> p & 1 == 0;
                let (a, b) = self.cards[p];
                SeatView {
                    name: pos[p].to_string(),
                    stack: cfg.stack as f32 - bet,
                    bet,
                    folded,
                    all_in: bet >= cfg.stack as f32 - 1e-3,
                    is_hero: p == self.hero,
                    is_button: p == button,
                    to_act: self.end.is_none() && to_act == Some(p),
                    cards: (p == self.hero || (showdown && !folded)).then_some([a, b]),
                    has_cards: !folded,
                    last_action: self.last_action[p].clone(),
                }
            })
            .collect()
    }

    pub fn board(&self) -> Vec<Card> {
        match &self.end {
            Some(PreflopEnd::Showdown { board, .. }) => board.clone(),
            _ => Vec::new(),
        }
    }

    /// One-line result of the finished hand.
    pub fn end_text(&self) -> Option<String> {
        let pos = self.positions();
        Some(match self.end.as_ref()? {
            PreflopEnd::Won { winner } if *winner == self.hero => "Everyone folds: you win the pot.".into(),
            PreflopEnd::Won { winner } => format!("{} wins the pot.", pos[*winner]),
            PreflopEnd::Showdown { winners, descriptions, .. } => {
                let desc = |p: usize| descriptions.iter().find(|d| d.0 == p).map(|d| d.1.clone()).unwrap_or_default();
                let w = winners[0];
                // The hero lost (or won) on the kicker when both made the same hand;
                // without the hero in the showdown, any loser with that hand did.
                let hero_in = descriptions.iter().any(|d| d.0 == self.hero);
                let kicker = winners.len() == 1
                    && if hero_in && w != self.hero {
                        desc(self.hero) == desc(w)
                    } else {
                        descriptions.iter().any(|d| d.0 != w && d.1 == desc(w))
                    };
                let how = if kicker { format!("{} and the better kicker", desc(w)) } else { desc(w) };
                if winners.contains(&self.hero) && winners.len() > 1 {
                    format!("All-in: you split the pot with {}.", desc(self.hero))
                } else if w == self.hero {
                    format!("All-in: you win with {how}.")
                } else if winners.len() > 1 {
                    let names: Vec<&str> = winners.iter().map(|&p| pos[p]).collect();
                    format!("All-in: {} split the pot with {}.", names.join(" and "), desc(w))
                } else if hero_in {
                    format!("All-in: {} wins with {how}; you have {}.", pos[w], desc(self.hero))
                } else {
                    format!("All-in: {} wins with {how}.", pos[w])
                }
            }
            PreflopEnd::SeesFlop { players } if players.contains(&self.hero) => {
                let others: Vec<&str> = players.iter().filter(|&&p| p != self.hero).map(|&p| pos[p]).collect();
                format!("You see the flop against {}.", others.join(", "))
            }
            PreflopEnd::SeesFlop { players } => {
                let names: Vec<&str> = players.iter().map(|&p| pos[p]).collect();
                format!("{} see the flop.", names.join(" and "))
            }
            PreflopEnd::OffTree => {
                format!("The solution never plays {} this way, so it has no strategy for this spot. Hand over.", self.hero_hand_name())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holdem_solver::card::combo_cards;
    use holdem_solver::preflop::PreflopConfig;
    use holdem_solver::range::class_combos;
    use std::sync::OnceLock;

    /// Solved once and shared by the tests.
    fn tiny_pack(pushfold: bool) -> Arc<LoadedPreflop> {
        static PACKS: [OnceLock<Arc<LoadedPreflop>>; 2] = [OnceLock::new(), OnceLock::new()];
        PACKS[pushfold as usize]
            .get_or_init(|| {
                let cfg = if pushfold {
                    crate::gen::pushfold_config(10.0, 200_000)
                } else {
                    PreflopConfig { iterations: 200_000, ..PreflopConfig::default() }
                };
                let pack = crate::gen::preflop_pack("test", cfg, |_, _| {}).unwrap();
                Arc::new(LoadedPreflop::new(pack).unwrap())
            })
            .clone()
    }

    /// A hand at the root with the hero in `hero` holding a combo of `class`.
    fn fixed_hand(pack: &Arc<LoadedPreflop>, hero: usize, class: usize) -> PreflopHand {
        let n = pack.pack.config.players;
        let mine = combo_cards(class_combos(class)[0]);
        let mut deck: Vec<Card> = (0..52u8).filter(|&c| c != mine.0 && c != mine.1).collect();
        let cards = (0..n).map(|p| if p == hero { mine } else { (deck.pop().unwrap(), deck.pop().unwrap()) }).collect();
        PreflopHand { pack: pack.clone(), hero, cards, node: 0, log: Vec::new(), last_action: vec![None; n], end: None, hero_decisions: 0, deck }
    }

    /// Plays `line` from the current node, hero included: `f` fold, `c` call/check, `r` raise, `a` all-in.
    fn play(h: &mut PreflopHand, line: &str) {
        for ch in line.chars() {
            let PKind::Decision(q) = h.node().kind else { panic!("no decision left for '{ch}' in {line}") };
            let i = h
                .actions_at(h.node)
                .iter()
                .position(|a| match ch {
                    'f' => *a == PAction::Fold,
                    'c' => matches!(a, PAction::Call | PAction::Check),
                    'r' => matches!(a, PAction::Raise(_)),
                    _ => matches!(a, PAction::AllIn(_)),
                })
                .unwrap_or_else(|| panic!("'{ch}' not available in {line}"));
            h.apply(q as usize, i);
        }
    }

    #[test]
    fn hands_reach_the_hero_and_finish() {
        let pack = tiny_pack(false);
        let mut rng = Rng::new(9);
        for _ in 0..200 {
            let mut h = PreflopHand::deal(pack.clone(), &[], 0, &mut rng);
            let mut steps = 0;
            while let Some(d) = h.decision() {
                assert!((d.freqs.iter().sum::<f32>() - 1.0).abs() < 1e-3);
                assert_eq!(d.labels.len(), d.actions.len());
                let first = h.hero_decisions == 0;
                let fb = h.answer(rng.below(d.actions.len() as u32) as usize, &mut rng);
                let grid = fb.grid.as_ref().unwrap();
                assert!(grid.strategy.len() == NUM_CLASSES && grid.reach.len() == NUM_CLASSES);
                if first {
                    assert!(grid.reach.iter().all(|&r| r == 1.0), "a first decision is reached with every hand");
                } else {
                    assert!(grid.reach[grid.hero_class] >= MIN_REACH, "off-range spots end the hand instead");
                }
                assert!(fb.freqs_trusted);
                steps += 1;
                assert!(steps < 10);
            }
            assert!(h.end.is_some());
            assert!(h.end_text().is_some());
            let seats = h.seats();
            assert_eq!(seats.len(), 6);
            assert!(seats[h.hero].cards.is_some());
            assert!(seats.iter().all(|s| !s.to_act), "nobody is to act once the hand is over");
        }
    }

    #[test]
    fn off_range_hands_end_instead_of_using_untrained_strategy() {
        let pack = tiny_pack(false);
        let root = &pack.tree.nodes[0];
        let open = pack.tree.nodes[0].children().position(|c| matches!(pack.tree.nodes[c].action, PAction::Raise(_))).unwrap();
        let freq = |c: usize| pack.strategy(root, c)[open];
        let never = (0..NUM_CLASSES).min_by(|&a, &b| freq(a).partial_cmp(&freq(b)).unwrap()).unwrap();
        let always = (0..NUM_CLASSES).max_by(|&a, &b| freq(a).partial_cmp(&freq(b)).unwrap()).unwrap();
        assert!(freq(never) < MIN_REACH && freq(always) > 0.9, "{} {}", freq(never), freq(always));
        let mut rng = Rng::new(1);

        // UTG opens a hand the solution never opens and BTN 3-bets: the hand ends.
        let mut h = fixed_hand(&pack, 0, never);
        play(&mut h, "rffrff");
        h.run_villains(&mut rng);
        assert!(matches!(h.end, Some(PreflopEnd::OffTree)));
        assert!(h.decision().is_none());
        assert!(h.end_text().unwrap().contains(&class_name(never)), "{:?}", h.end_text());
        assert!(h.seats().iter().all(|s| !s.to_act));

        // The same line with a hand the solution always opens goes on, and the
        // grid's reach is each class's open frequency.
        let mut h = fixed_hand(&pack, 0, always);
        play(&mut h, "rffrff");
        h.run_villains(&mut rng);
        assert!(h.end.is_none());
        let d = h.decision().unwrap();
        assert_eq!(d.category, "UTG vs BTN 3-bet");
        let fb = h.answer(0, &mut rng);
        let grid = fb.grid.unwrap();
        for c in 0..NUM_CLASSES {
            assert!((grid.reach[c] - freq(c)).abs() < 1e-6);
        }
        assert!(grid.reach[never] < MIN_REACH);
    }

    #[test]
    fn showdown_texts_name_the_hands() {
        let pack = tiny_pack(true);
        let mut h = fixed_hand(&pack, 0, 0);
        let desc = |v: &[(usize, &str)]| v.iter().map(|&(p, d)| (p, d.to_string())).collect::<Vec<_>>();
        h.end = Some(PreflopEnd::Showdown { board: Vec::new(), winners: vec![3], descriptions: desc(&[(0, "a pair of kings"), (3, "a pair of kings")]) });
        assert_eq!(h.end_text().unwrap(), "All-in: BTN wins with a pair of kings and the better kicker; you have a pair of kings.");
        h.end = Some(PreflopEnd::Showdown { board: Vec::new(), winners: vec![0], descriptions: desc(&[(0, "a straight to the ten"), (3, "a pair of kings")]) });
        assert_eq!(h.end_text().unwrap(), "All-in: you win with a straight to the ten.");
        h.end = Some(PreflopEnd::Showdown { board: Vec::new(), winners: vec![4], descriptions: desc(&[(3, "ace high"), (4, "a pair of twos")]) });
        assert_eq!(h.end_text().unwrap(), "All-in: SB wins with a pair of twos.");
    }

    #[test]
    fn calls_show_the_amount() {
        let pack = tiny_pack(false);
        // SB facing a UTG open of 2.5 still puts in 2.
        let mut h = fixed_hand(&pack, 4, 0);
        play(&mut h, "rfff");
        let d = h.decision().unwrap();
        assert!(d.labels.contains(&"Call 2".to_string()), "{:?}", d.labels);
        assert!(d.labels.contains(&"Fold".to_string()) && d.labels.iter().any(|l| l.starts_with("Raise to ")), "{:?}", d.labels);
        // The hand history shows the amount too.
        play(&mut h, "c");
        assert_eq!(h.log.last().unwrap().what, "Call 2");
        assert!(h.log[0].what.starts_with("Raise to 2.5"));

        // Push/fold: BB facing the SB jam of a 10bb pack (9.875 behind after the ante).
        let pack = tiny_pack(true);
        let mut h = fixed_hand(&pack, 5, 0);
        play(&mut h, "ffffa");
        let d = h.decision().unwrap();
        assert_eq!(d.labels, vec!["Fold".to_string(), "Call 8.88".to_string()]);
        assert_eq!(h.log.last().unwrap().what, "All-in 9.88");
    }

    #[test]
    fn pushfold_hands_only_offer_jam_or_fold() {
        let pack = tiny_pack(true);
        assert_eq!(pack.nominal_stack(), 10.0);
        let mut rng = Rng::new(4);
        for _ in 0..100 {
            let h = PreflopHand::deal(pack.clone(), &[3], 0, &mut rng);
            let d = h.decision().unwrap();
            assert_eq!(h.hero, 3);
            assert!(d.actions.iter().all(|a| matches!(a, PAction::Fold | PAction::AllIn(_) | PAction::Call)));
            assert!(h.category().starts_with("Push BTN") || h.category().starts_with("Call BTN"));
        }
        let mut h = fixed_hand(&pack, 3, 0);
        play(&mut h, "fff");
        assert_eq!(h.category(), "Push BTN 10bb");
        assert_eq!(h.spot_text(), "BTN, folded to you, 10bb");
        let mut h = fixed_hand(&pack, 5, 0);
        play(&mut h, "ffafc");
        assert_eq!(h.category(), "Call BB vs CO jam + call");
    }

    #[test]
    fn categories_name_the_spot() {
        let pack = tiny_pack(false);
        let mut rng = Rng::new(11);
        let mut seen = std::collections::BTreeSet::new();
        for _ in 0..400 {
            let h = PreflopHand::deal(pack.clone(), &[], 0, &mut rng);
            seen.insert(h.category());
        }
        assert!(seen.contains("RFI UTG") && seen.contains("RFI BTN"), "{seen:?}");
        assert!(seen.iter().any(|c| c.contains("vs") && c.contains("open")), "{seen:?}");

        let spot = |hero: usize, line: &str| {
            let mut h = fixed_hand(&pack, hero, 0);
            play(&mut h, line);
            assert!(matches!(h.node().kind, PKind::Decision(q) if q as usize == hero), "{line}");
            (h.category(), h.spot_text())
        };
        assert_eq!(spot(5, "ffrcf").0, "BB vs CO open + call");
        assert_eq!(spot(5, "ffrff").0, "BB vs CO open");
        assert_eq!(spot(3, "rfr").0, "BTN cold vs 3-bet");
        let (cat, text) = spot(3, "fffrrf");
        assert_eq!(cat, "BTN vs SB 3-bet");
        assert_eq!(text, format!("BTN, you raise to 2.5, SB {}, 100bb", last_label(&pack, "fffrr")));
        assert_eq!(spot(2, "ffrcrf").0, "CO vs SB squeeze");
        assert_eq!(spot(3, "ffrcrff").0, "BTN flat vs SB squeeze");
    }

    /// The label of the last action of `line` from the root, lowercased.
    fn last_label(pack: &Arc<LoadedPreflop>, line: &str) -> String {
        let mut h = fixed_hand(pack, 0, 0);
        play(&mut h, line);
        h.log.last().unwrap().what.to_lowercase()
    }
}
