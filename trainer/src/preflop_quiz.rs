//! Preflop drill: a full 6-max hand where everyone else plays the solution.
//!
//! Works for the 100bb cash pack and for push/fold packs alike.

use std::sync::Arc;

use holdem_solver::card::{Card, Rng, card_bit, combo_index, combo_to_string};
use holdem_solver::eval::{describe, eval_mask};
use holdem_solver::postflop::tree::fmt_chips;
use holdem_solver::preflop::{PAction, PKind};
use holdem_solver::range::{NUM_CLASSES, class_name, combo_class};

use crate::grading::by_frequency;
use crate::model::LoadedPreflop;
use crate::quiz::{Feedback, LogEntry, SeatView, shuffled_deck, skip_obvious};

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

fn action_label(a: PAction, facing_bet: bool) -> String {
    match a {
        PAction::Fold => "Fold".into(),
        PAction::Check => "Check".into(),
        PAction::Call => if facing_bet { "Call".into() } else { "Check".into() },
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

    fn facing_bet(&self, seat: usize) -> bool {
        let node = self.node();
        let max = node.invested.iter().cloned().fold(0.0f32, f32::max);
        max > node.invested[seat] + 1e-6
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
        let facing = self.facing_bet(self.hero);
        Some(PreflopDecision {
            labels: actions.iter().map(|&a| action_label(a, facing)).collect(),
            freqs: self.pack.strategy(node, self.hero_class()),
            actions,
            category: self.category(),
        })
    }

    /// Category of the hero's current spot for stats, e.g. "RFI BTN" or "BB vs BTN open".
    pub fn category(&self) -> String {
        let pos = self.positions();
        let me = pos[self.hero];
        let stack = self.pack.pack.config.stack;
        let raises: Vec<(usize, bool)> = self
            .log_actions()
            .into_iter()
            .filter(|(_, a)| matches!(a, PAction::Raise(_) | PAction::AllIn(_)))
            .map(|(s, a)| (s, matches!(a, PAction::AllIn(_))))
            .collect();
        let push = self.pack.pack.config.pushfold;
        match (raises.len(), push) {
            (0, true) => format!("Push {me} {stack}bb"),
            (_, true) => format!("Call {me} vs {} jam", pos[raises[0].0]),
            (0, false) => format!("RFI {me}"),
            (1, false) => format!("{me} vs {} open", pos[raises[0].0]),
            (2, false) => format!("{me} vs 3-bet"),
            (_, false) if raises.last().is_some_and(|r| r.1) => format!("{me} vs all-in"),
            (_, false) => format!("{me} vs 4-bet"),
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
                PKind::Decision(q) if q as usize == self.hero => return,
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
        let facing = self.facing_bet(seat);
        let child = self.node().first_child as usize + i;
        let action = self.pack.tree.nodes[child].action;
        let label = action_label(action, facing);
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
                    descriptions: ranks.iter().map(|&(p, r)| (p, describe(r).to_string())).collect(),
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
        let grid: Vec<Vec<f32>> = (0..NUM_CLASSES).map(|c| self.pack.strategy(&node, c)).collect();
        debug_assert!(grid.iter().all(|g| g.len() == na));
        let (a, b) = self.cards[self.hero];
        let fb = Feedback {
            grade,
            chosen: i,
            actions: d.labels.clone(),
            freqs: d.freqs.clone(),
            evs: None,
            ev_loss: 0.0,
            pot: node.pot,
            category: d.category.clone(),
            spot: self.spot_text(),
            hand: format!("{} ({})", combo_to_string(combo_index(a, b)), self.hero_hand_name()),
            grid: Some((grid, self.hero_class())),
        };
        self.hero_decisions += 1;
        self.apply(self.hero, i);
        self.run_villains(rng);
        fb
    }

    /// Short description of the hero's spot, e.g. "BTN, CO opened to 2.5".
    pub fn spot_text(&self) -> String {
        let pos = self.positions();
        let acts: Vec<String> = self
            .log
            .iter()
            .filter(|e| !e.what.starts_with("Fold"))
            .map(|e| format!("{} {}", e.who, e.what.to_lowercase()))
            .collect();
        let stack = fmt_chips(self.pack.pack.config.stack as f32);
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
                    to_act: to_act == Some(p),
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
                if winners.contains(&self.hero) && winners.len() > 1 {
                    format!("All-in: you split the pot with {}.", desc(self.hero))
                } else if winners.contains(&self.hero) {
                    format!("All-in: you win with {}.", desc(self.hero))
                } else {
                    let w = winners[0];
                    format!("All-in: {} wins with {}.", pos[w], desc(w))
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holdem_solver::preflop::PreflopConfig;

    fn tiny_pack(pushfold: bool) -> Arc<LoadedPreflop> {
        let cfg = if pushfold {
            crate::gen::pushfold_config(10.0, 200_000)
        } else {
            PreflopConfig { iterations: 200_000, ..PreflopConfig::default() }
        };
        let pack = crate::gen::preflop_pack("test", cfg, |_, _| {}).unwrap();
        Arc::new(LoadedPreflop::new(pack).unwrap())
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
                let fb = h.answer(rng.below(d.actions.len() as u32) as usize, &mut rng);
                assert!(fb.grid.as_ref().unwrap().0.len() == NUM_CLASSES);
                steps += 1;
                assert!(steps < 10);
            }
            assert!(h.end.is_some());
            assert!(h.end_text().is_some());
            let seats = h.seats();
            assert_eq!(seats.len(), 6);
            assert!(seats[h.hero].cards.is_some());
        }
    }

    #[test]
    fn pushfold_hands_only_offer_jam_or_fold() {
        let pack = tiny_pack(true);
        let mut rng = Rng::new(4);
        for _ in 0..100 {
            let h = PreflopHand::deal(pack.clone(), &[3], 0, &mut rng);
            let d = h.decision().unwrap();
            assert_eq!(h.hero, 3);
            assert!(d.actions.iter().all(|a| matches!(a, PAction::Fold | PAction::AllIn(_) | PAction::Call)));
            assert!(h.category().starts_with("Push BTN") || h.category().starts_with("Call BTN"));
        }
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
    }
}
