//! Postflop drill: play a heads-up hand against the solution.
//!
//! The flop is played from the stored solution in the [`FlopLibrary`]. When the
//! flop betting closes, the turn (and river) are re-solved in a background
//! thread from the ranges the flop line arrives with, which takes a second or
//! two. Suits are shown through a random permutation so each stored flop
//! appears in many suit variants.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use holdem_solver::card::{Card, Rng, card_bit, card_to_string, combo_index, combo_to_string, permute_card};
use holdem_solver::eval::{describe_long, eval_mask};
use holdem_solver::postflop::explore::{Explorer, Frame};
use holdem_solver::postflop::tree::fmt_chips;
use holdem_solver::postflop::{Action, NodeKind, PostflopGame};
use holdem_solver::range::Range;

use crate::gen::{spot_positions, turn_config};
use crate::grading::by_ev;
use crate::model::{Act, ActKind, FlopLibrary, FlopSpot, Next};
use crate::quiz::{Feedback, LogEntry, MIN_REACH, SeatView, sample_weighted, skip_obvious};

pub enum Stage {
    Flop(u32),
    /// Waiting for the turn subgame to be solved.
    Solving(Receiver<Result<PostflopGame, String>>),
    /// Playing the turn and river from the solved subgame.
    Later { game: Box<PostflopGame>, frames: Vec<Frame> },
    Over(PostEnd),
}

#[derive(Clone, Debug)]
pub enum PostEnd {
    Fold { folder: usize },
    Showdown { winner: Option<usize>, desc: [String; 2] },
    /// The hero took a line the solution never takes; there is nothing to play against.
    OffTree,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct PostDecision {
    pub labels: Vec<String>,
    pub kinds: Vec<ActKind>,
    pub freqs: Vec<f32>,
    pub evs: Vec<f32>,
    pub pot: f32,
    pub street: &'static str,
    pub category: String,
}

pub struct PostflopHand {
    pub lib: Arc<FlopLibrary>,
    pub spot_idx: usize,
    /// Canonical suit -> displayed suit.
    pub perm: [u8; 4],
    /// 0 = out of position, 1 = in position.
    pub hero: usize,
    /// Canonical hole cards, `[OOP, IP]`.
    pub cards: [(Card, Card); 2],
    /// Canonical board.
    pub board: Vec<Card>,
    pub log: Vec<LogEntry>,
    pub stage: Stage,
    pub hero_decisions: u32,
    /// EV the hero has given up so far this hand (big blinds).
    pub ev_lost: f32,
    /// Product of the solution's frequencies for the hero's hand of the actions
    /// the hero took so far on this solve (flop, or turn subgame). Below
    /// [`MIN_REACH`] the solution never brings the hand here, so its stored
    /// frequencies are untrained noise and only the EVs mean something.
    hero_reach: f32,
    contrib: [f32; 2],
    street_start: [f32; 2],
    last_action: [Option<String>; 2],
    deck: Vec<Card>,
    cached: Option<PostDecision>,
}

fn kind_of(a: Action) -> ActKind {
    match a {
        Action::Fold => ActKind::Fold,
        Action::Check => ActKind::Check,
        Action::Call => ActKind::Call,
        Action::Bet(_) => ActKind::Bet,
        Action::Raise(_) => ActKind::Raise,
        Action::AllIn(_) => ActKind::AllIn,
        _ => ActKind::Check,
    }
}

fn to_of(a: Action) -> f32 {
    match a {
        Action::Bet(x) | Action::Raise(x) | Action::AllIn(x) => x,
        _ => 0.0,
    }
}

impl PostflopHand {
    /// Deals a new hand on a random stored flop of one of `kinds`. `role` is
    /// "oop", "ip" or anything else for random.
    pub fn deal(lib: Arc<FlopLibrary>, kinds: &[String], role: &str, difficulty: u8, rng: &mut Rng) -> Option<PostflopHand> {
        let candidates: Vec<usize> =
            (0..lib.spots.len()).filter(|&i| kinds.is_empty() || kinds.contains(&lib.spots[i].kind)).collect();
        if candidates.is_empty() {
            return None;
        }
        let mut fallback = None;
        for attempt in 0..200 {
            let spot_idx = candidates[rng.below(candidates.len() as u32) as usize];
            let hero = match role {
                "oop" => 0,
                "ip" => 1,
                _ => rng.below(2) as usize,
            };
            let Some(mut hand) = Self::new_hand(lib.clone(), spot_idx, hero, rng) else { continue };
            hand.advance(rng);
            let skip = match hand.decision() {
                Some(d) => {
                    let fold = d.kinds.iter().position(|k| *k == ActKind::Fold);
                    attempt < 150 && skip_obvious(&d.freqs, fold, difficulty, rng)
                }
                None => true,
            };
            if skip {
                if hand.decision().is_some() {
                    fallback.get_or_insert(hand);
                }
                continue;
            }
            return Some(hand);
        }
        fallback
    }

    fn new_hand(lib: Arc<FlopLibrary>, spot_idx: usize, hero: usize, rng: &mut Rng) -> Option<PostflopHand> {
        let spot = &lib.spots[spot_idx];
        let mut perm = [0u8, 1, 2, 3];
        for i in (1..4).rev() {
            let j = rng.below(i as u32 + 1) as usize;
            perm.swap(i, j);
        }
        let h0 = sample_weighted(&spot.weights[hero], rng)?;
        let hero_cards = spot.hands[hero][h0];
        let blocked = card_bit(hero_cards.0) | card_bit(hero_cards.1);
        let v = 1 - hero;
        let vw: Vec<f32> = spot.hands[v]
            .iter()
            .zip(&spot.weights[v])
            .map(|(&(a, b), &w)| if (card_bit(a) | card_bit(b)) & blocked == 0 { w } else { 0.0 })
            .collect();
        let villain_cards = spot.hands[v][sample_weighted(&vw, rng)?];
        let mut cards = [hero_cards; 2];
        cards[v] = villain_cards;
        let used: Vec<Card> = spot.board.iter().copied().chain([hero_cards.0, hero_cards.1, villain_cards.0, villain_cards.1]).collect();
        let deck: Vec<Card> = (0..52u8).filter(|c| !used.contains(c)).collect();
        let half = spot.config.pot as f32 / 2.0;
        Some(PostflopHand {
            board: spot.board.clone(),
            lib: lib.clone(),
            spot_idx,
            perm,
            hero,
            cards,
            log: Vec::new(),
            stage: Stage::Flop(0),
            hero_decisions: 0,
            ev_lost: 0.0,
            hero_reach: 1.0,
            contrib: [half, half],
            street_start: [half, half],
            last_action: [None, None],
            deck,
            cached: None,
        })
    }

    pub fn spot(&self) -> &FlopSpot {
        &self.lib.spots[self.spot_idx]
    }

    pub fn positions(&self) -> [&'static str; 2] {
        spot_positions(&self.spot().kind)
    }

    pub fn show(&self, c: Card) -> Card {
        permute_card(c, &self.perm)
    }

    pub fn display_board(&self) -> Vec<Card> {
        self.board.iter().map(|&c| self.show(c)).collect()
    }

    pub fn display_cards(&self, p: usize) -> [Card; 2] {
        let (a, b) = self.cards[p];
        [self.show(a), self.show(b)]
    }

    pub fn street(&self) -> &'static str {
        match self.board.len() {
            3 => "Flop",
            4 => "Turn",
            _ => "River",
        }
    }

    pub fn pot(&self) -> f32 {
        self.contrib[0] + self.contrib[1]
    }

    pub fn is_solving(&self) -> bool {
        matches!(self.stage, Stage::Solving(_))
    }

    pub fn is_over(&self) -> bool {
        matches!(self.stage, Stage::Over(_))
    }

    fn label(&self, kind: ActKind, to: f32, player: usize) -> String {
        let pot = self.pot();
        let start = self.street_start[player];
        let mine = self.contrib[player] - start;
        match kind {
            ActKind::Fold => "Fold".into(),
            ActKind::Check => "Check".into(),
            ActKind::Call => {
                let other = self.contrib[1 - player] - self.contrib[player];
                format!("Call {}", fmt_chips(other))
            }
            ActKind::Bet => {
                let amount = to - mine;
                format!("Bet {} ({:.0}%)", fmt_chips(amount), 100.0 * amount / pot)
            }
            ActKind::Raise => format!("Raise to {}", fmt_chips(to)),
            ActKind::AllIn => format!("All-in {}", fmt_chips(to)),
        }
    }

    /// Lets the villain act, deals cards and starts solves until the hero must
    /// decide, a solve is pending, or the hand is over.
    fn advance(&mut self, rng: &mut Rng) {
        self.cached = None;
        loop {
            match &self.stage {
                Stage::Flop(n) => {
                    let node = self.spot().nodes[*n as usize].clone();
                    let q = node.player as usize;
                    if q == self.hero {
                        return;
                    }
                    let (a, b) = self.cards[q];
                    let h = self.spot().hand_index(q, a, b).expect("villain hand in range");
                    let i = sample_weighted(&self.spot().node_strategy(&node, h), rng).unwrap_or(0);
                    self.apply_flop(&node.actions, &node.next, q, i, rng);
                }
                Stage::Solving(_) | Stage::Over(_) => return,
                Stage::Later { .. } => {
                    if !self.step_later(rng) {
                        return;
                    }
                }
            }
        }
    }

    fn apply_flop(&mut self, actions: &[Act], next: &[Next], q: usize, i: usize, rng: &mut Rng) {
        let label = self.label(actions[i].kind, actions[i].to, q);
        self.push_log(q, label);
        match &next[i] {
            Next::Node(m) => {
                self.contrib = self.spot().nodes[*m as usize].contrib;
                self.stage = Stage::Flop(*m);
            }
            Next::Fold { folder, contrib } => {
                self.contrib = *contrib;
                self.stage = Stage::Over(PostEnd::Fold { folder: *folder as usize });
            }
            Next::Line(l) => {
                let line = self.spot().lines[*l as usize].clone();
                self.contrib = line.contrib;
                let hero_reach: f32 = line.reach[self.hero].iter().sum();
                let hero_total: f32 = self.spot().weights[self.hero].iter().sum();
                if line.all_in {
                    while self.board.len() < 5 {
                        let c = self.draw(rng);
                        self.board.push(c);
                    }
                    self.showdown();
                } else if hero_reach <= 1e-6 * hero_total {
                    self.stage = Stage::Over(PostEnd::OffTree);
                } else {
                    let turn = self.draw(rng);
                    self.board.push(turn);
                    self.new_street();
                    self.stage = Stage::Solving(self.spawn_turn_solve(&line, turn));
                }
            }
        }
    }

    fn draw(&mut self, rng: &mut Rng) -> Card {
        let i = rng.below(self.deck.len() as u32) as usize;
        self.deck.swap_remove(i)
    }

    fn new_street(&mut self) {
        self.street_start = self.contrib;
        self.last_action = [None, None];
        let c = self.show(*self.board.last().unwrap());
        self.log.push(LogEntry { street: self.street().into(), who: "Dealer".into(), what: card_to_string(c), hero: false });
    }

    fn push_log(&mut self, q: usize, what: String) {
        let who = self.positions()[q].to_string();
        self.last_action[q] = Some(what.clone());
        self.log.push(LogEntry { street: self.street().into(), who, what, hero: q == self.hero });
    }

    fn spawn_turn_solve(&self, line: &crate::model::FlopLine, turn: Card) -> Receiver<Result<PostflopGame, String>> {
        let spot = self.spot();
        let cfg = turn_config(spot, line, turn);
        let mut ranges = [Range::default(), Range::default()];
        for p in 0..2 {
            for (h, &(a, b)) in spot.hands[p].iter().enumerate() {
                ranges[p].weights[combo_index(a, b)] = line.reach[p][h];
            }
            // Keep the actual hands in the subgame even if the solution would
            // (almost) never hold them here.
            let (a, b) = self.cards[p];
            let ci = combo_index(a, b);
            ranges[p].weights[ci] = ranges[p].weights[ci].max(1e-5);
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let result = (|| {
                let target = cfg.target_exploitability;
                let max_it = cfg.iterations;
                let mut g = PostflopGame::with_ranges(cfg, ranges)?;
                for it in 1..=max_it {
                    g.iterate();
                    if it % 10 == 0 && g.exploitability_pct() <= target {
                        break;
                    }
                }
                g.finalize();
                Ok(g)
            })();
            let _ = tx.send(result);
        });
        rx
    }

    /// Checks whether a pending turn solve finished. Returns true if the state changed.
    pub fn poll(&mut self, rng: &mut Rng) -> bool {
        let Stage::Solving(rx) = &self.stage else { return false };
        match rx.try_recv() {
            Ok(Ok(game)) => {
                let frames = Explorer::new(&game).frames;
                self.stage = Stage::Later { game: Box::new(game), frames };
                // The subgame keeps the hero's hand in range (at a tiny weight if
                // need be), and its strategy at the turn converges whatever that
                // weight, so the hero is back on the solution's path.
                self.hero_reach = 1.0;
                self.advance(rng);
                true
            }
            Ok(Err(e)) => {
                self.stage = Stage::Over(PostEnd::Error(e));
                true
            }
            Err(TryRecvError::Empty) => false,
            Err(TryRecvError::Disconnected) => {
                self.stage = Stage::Over(PostEnd::Error("the turn solver stopped unexpectedly".into()));
                true
            }
        }
    }

    /// One automatic step on the turn/river. Returns false when the hero must act or the hand ended.
    fn step_later(&mut self, rng: &mut Rng) -> bool {
        let node = {
            let Stage::Later { game, frames } = &self.stage else { return false };
            Explorer { game, frames: frames.clone() }.node().clone()
        };
        match node.kind {
            NodeKind::Player(q) if q as usize == self.hero => false,
            NodeKind::Player(q) => {
                let q = q as usize;
                let (label, new_frames) = {
                    let Stage::Later { game, frames } = &self.stage else { return false };
                    let mut ex = Explorer { game, frames: frames.clone() };
                    let (a, b) = self.cards[q];
                    let h = ex.hand_of(q, a, b).expect("villain hand in subgame");
                    let nh = game.num_hands(q);
                    let strat = ex.strategy();
                    let freqs: Vec<f32> = (0..node.num_children as usize).map(|i| strat[i * nh + h]).collect();
                    let i = sample_weighted(&freqs, rng).unwrap_or(0);
                    let act = ex.child_actions()[i];
                    ex.take_action(i);
                    (self.label(kind_of(act), to_of(act), q), ex.frames)
                };
                self.push_log(q, label);
                self.set_later(new_frames);
                true
            }
            NodeKind::Chance => {
                let c = self.draw(rng);
                let new_frames = {
                    let Stage::Later { game, frames } = &self.stage else { return false };
                    let mut ex = Explorer { game, frames: frames.clone() };
                    ex.deal(c).expect("dealable card");
                    ex.frames
                };
                self.board.push(c);
                self.new_street();
                self.set_later(new_frames);
                true
            }
            NodeKind::Fold(f) => {
                self.contrib = node.contrib;
                self.stage = Stage::Over(PostEnd::Fold { folder: f as usize });
                false
            }
            NodeKind::Showdown => {
                self.contrib = node.contrib;
                self.showdown();
                false
            }
        }
    }

    fn set_later(&mut self, new_frames: Vec<Frame>) {
        if let Stage::Later { game, frames } = &mut self.stage {
            *frames = new_frames;
            let ex = Explorer { game, frames: frames.clone() };
            self.contrib = ex.node().contrib;
        }
    }

    fn showdown(&mut self) {
        let bmask = self.board.iter().fold(0u64, |m, &c| m | card_bit(c));
        let ranks: Vec<u32> = (0..2)
            .map(|p| {
                let (a, b) = self.cards[p];
                eval_mask(bmask | card_bit(a) | card_bit(b))
            })
            .collect();
        let winner = match ranks[0].cmp(&ranks[1]) {
            std::cmp::Ordering::Greater => Some(0),
            std::cmp::Ordering::Less => Some(1),
            std::cmp::Ordering::Equal => None,
        };
        self.stage = Stage::Over(PostEnd::Showdown {
            winner,
            desc: [describe_long(ranks[0]), describe_long(ranks[1])],
        });
    }

    pub fn category(&self) -> String {
        let short = match self.spot().kind.as_str() {
            crate::gen::SRP => "SRP",
            crate::gen::THREE_BET => "3BP",
            _ => "Spot",
        };
        format!("{short} {} {}", self.street().to_lowercase(), self.positions()[self.hero])
    }

    /// The hero's current decision (computed once per decision).
    pub fn decision(&mut self) -> Option<PostDecision> {
        if self.cached.is_none() {
            self.cached = self.compute_decision();
        }
        self.cached.clone()
    }

    fn compute_decision(&self) -> Option<PostDecision> {
        let (a, b) = self.cards[self.hero];
        let (kinds, tos, freqs, evs) = match &self.stage {
            Stage::Flop(n) => {
                let node = &self.spot().nodes[*n as usize];
                if node.player as usize != self.hero {
                    return None;
                }
                let h = self.spot().hand_index(self.hero, a, b)?;
                (
                    node.actions.iter().map(|x| x.kind).collect::<Vec<_>>(),
                    node.actions.iter().map(|x| x.to).collect::<Vec<_>>(),
                    self.spot().node_strategy(node, h),
                    self.spot().node_evs(node, h),
                )
            }
            Stage::Later { game, frames } => {
                let ex = Explorer { game, frames: frames.clone() };
                let NodeKind::Player(q) = ex.node().kind else { return None };
                if q as usize != self.hero {
                    return None;
                }
                let h = ex.hand_of(self.hero, a, b)?;
                let nh = game.num_hands(self.hero);
                let strat = ex.strategy();
                let acts = ex.child_actions();
                let all_evs = ex.action_evs();
                (
                    acts.iter().map(|&x| kind_of(x)).collect(),
                    acts.iter().map(|&x| to_of(x)).collect(),
                    (0..acts.len()).map(|i| strat[i * nh + h]).collect(),
                    all_evs.iter().map(|v| v[h]).collect(),
                )
            }
            _ => return None,
        };
        Some(PostDecision {
            labels: kinds.iter().zip(&tos).map(|(&k, &t)| self.label(k, t, self.hero)).collect(),
            kinds,
            freqs,
            evs,
            pot: self.pot(),
            street: self.street(),
            category: self.category(),
        })
    }

    pub fn answer(&mut self, i: usize, rng: &mut Rng) -> Feedback {
        let d = self.decision().expect("hero to act");
        // Grade with the reach into this decision, then follow the chosen action.
        let freqs_trusted = self.hero_reach >= MIN_REACH;
        let (grade, loss) = by_ev(&d.evs, &d.freqs, i, d.pot, freqs_trusted);
        self.hero_reach *= d.freqs[i];
        self.ev_lost += loss;
        self.hero_decisions += 1;
        let (a, b) = self.cards[self.hero];
        let fb = Feedback {
            grade,
            chosen: i,
            actions: d.labels.clone(),
            freqs: d.freqs.clone(),
            freqs_trusted,
            evs: Some(d.evs.clone()),
            ev_loss: loss,
            pot: d.pot,
            category: d.category.clone(),
            spot: self.spot_text(),
            hand: combo_to_string(combo_index(self.show(a), self.show(b))),
            grid: None,
        };
        match &self.stage {
            Stage::Flop(n) => {
                let node = self.spot().nodes[*n as usize].clone();
                self.apply_flop(&node.actions, &node.next, self.hero, i, rng);
            }
            Stage::Later { game, frames } => {
                let mut ex = Explorer { game, frames: frames.clone() };
                let act = ex.child_actions()[i];
                ex.take_action(i);
                let new_frames = ex.frames;
                let label = self.label(kind_of(act), to_of(act), self.hero);
                self.push_log(self.hero, label);
                self.set_later(new_frames);
            }
            Stage::Solving(_) | Stage::Over(_) => {}
        }
        self.advance(rng);
        fb
    }

    pub fn spot_text(&self) -> String {
        let board: String = self.display_board().iter().map(|&c| card_to_string(c)).collect();
        format!("{}, {} {}", crate::gen::spot_label(&self.spot().kind), self.street().to_lowercase(), board)
    }

    pub fn seats(&self) -> [SeatView; 2] {
        let pos = self.positions();
        let half = self.spot().config.pot as f32 / 2.0;
        let stack = self.spot().config.stack as f32;
        let over = self.is_over();
        let showdown = matches!(self.stage, Stage::Over(PostEnd::Showdown { .. }));
        let to_act = match &self.stage {
            Stage::Flop(n) => Some(self.spot().nodes[*n as usize].player as usize),
            Stage::Later { game, frames } => {
                let ex = Explorer { game, frames: frames.clone() };
                match ex.node().kind {
                    NodeKind::Player(q) => Some(q as usize),
                    _ => None,
                }
            }
            _ => None,
        };
        let folded = |p: usize| matches!(self.stage, Stage::Over(PostEnd::Fold { folder }) if folder == p);
        [0, 1].map(|p| SeatView {
            name: pos[p].to_string(),
            stack: stack - (self.contrib[p] - half),
            bet: self.contrib[p] - self.street_start[p],
            folded: folded(p),
            all_in: stack - (self.contrib[p] - half) <= 1e-3,
            is_hero: p == self.hero,
            // BTN (in position) has the button in both spot families.
            is_button: p == 1,
            to_act: !over && to_act == Some(p),
            cards: (p == self.hero || showdown || (over && !folded(p))).then(|| self.display_cards(p)),
            has_cards: !folded(p),
            last_action: self.last_action[p].clone(),
        })
    }

    pub fn end_text(&self) -> Option<String> {
        let Stage::Over(end) = &self.stage else { return None };
        let pos = self.positions();
        let villain = pos[1 - self.hero];
        let pot = fmt_chips(self.pot());
        // After a fold the uncalled part of the last bet goes back to the bettor.
        let won = fmt_chips(2.0 * self.contrib[0].min(self.contrib[1]));
        Some(match end {
            PostEnd::Fold { folder } if *folder == self.hero => format!("You fold. {villain} wins {won}bb."),
            PostEnd::Fold { .. } => format!("{villain} folds. You win {won}bb."),
            PostEnd::Showdown { winner: None, desc } => format!("Split pot: you both have {}.", desc[self.hero]),
            PostEnd::Showdown { winner: Some(w), desc } => {
                let (mine, theirs) = (&desc[self.hero], &desc[1 - self.hero]);
                // The same description with different values: the kicker decided.
                match (*w == self.hero, mine == theirs) {
                    (true, false) => format!("You win {pot}bb with {mine} against {theirs}."),
                    (true, true) => format!("You win {pot}bb: you both have {mine}, and your kicker is better."),
                    (false, false) => format!("{villain} wins {pot}bb with {theirs}; you have {mine}."),
                    (false, true) => format!("{villain} wins {pot}bb: you both have {mine}, and {villain}'s kicker is better."),
                }
            }
            PostEnd::OffTree => "The solution never plays this line, so there is no strategy to continue against. Hand over.".into(),
            PostEnd::Error(e) => format!("Could not continue the hand: {e}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::FlopNode;

    /// Solved once and shared by the tests.
    fn library() -> Arc<FlopLibrary> {
        static LIB: std::sync::OnceLock<Arc<FlopLibrary>> = std::sync::OnceLock::new();
        LIB.get_or_init(|| {
            // A small, quick spot: narrow 3-bet pot ranges on a river-like dry flop.
            let spot = crate::gen::flop_spot(crate::gen::THREE_BET, "Ks7d2c", |_| {}).unwrap();
            Arc::new(FlopLibrary { spots: vec![spot] })
        })
        .clone()
    }

    #[test]
    fn end_texts_name_the_hands_and_the_pot_won() {
        let mut h = PostflopHand::new_hand(library(), 0, 0, &mut Rng::new(1)).unwrap();
        assert_eq!(h.positions(), ["BB", "BTN"]);
        // BTN bets 15.38 into 20.5 and the hero folds: the uncalled bet goes back.
        h.contrib = [10.25, 25.63];
        h.stage = Stage::Over(PostEnd::Fold { folder: 0 });
        assert_eq!(h.end_text().unwrap(), "You fold. BTN wins 20.5bb.");
        h.contrib = [20.5, 20.5];
        let desc = |a: &str, b: &str| [a.to_string(), b.to_string()];
        h.stage = Stage::Over(PostEnd::Showdown { winner: Some(1), desc: desc("a pair of kings", "a pair of kings") });
        assert_eq!(h.end_text().unwrap(), "BTN wins 41bb: you both have a pair of kings, and BTN's kicker is better.");
        h.stage = Stage::Over(PostEnd::Showdown { winner: Some(0), desc: desc("kings full of sevens", "a flush, ace high") });
        assert_eq!(h.end_text().unwrap(), "You win 41bb with kings full of sevens against a flush, ace high.");
        h.stage = Stage::Over(PostEnd::Showdown { winner: Some(1), desc: desc("two pair, kings and sevens", "three nines") });
        assert_eq!(h.end_text().unwrap(), "BTN wins 41bb with three nines; you have two pair, kings and sevens.");
    }

    #[test]
    fn off_path_actions_are_graded_by_ev_only() {
        let lib = library();
        let spot = &lib.spots[0];
        let freq = |n: &FlopNode, a: usize, h: usize| spot.node_strategy(n, h)[a];
        let root = &spot.nodes[0];
        assert_eq!(root.player, 0);
        let check = root.actions.iter().position(|a| a.kind == ActKind::Check).unwrap();
        let Next::Node(m) = root.next[check] else { panic!("a check leads to the IP decision") };
        let after = &spot.nodes[m as usize];
        let bet = after.actions.iter().position(|a| a.kind == ActKind::Bet).unwrap();
        // OOP holds a hand the solution never checks; IP the hand that bets most after a check.
        let in_range = |p: usize| (0..spot.hands[p].len()).filter(move |&i| spot.weights[p][i] > 0.0);
        let hero = in_range(0).min_by(|&a, &b| freq(root, check, a).total_cmp(&freq(root, check, b))).unwrap();
        assert!(freq(root, check, hero) < MIN_REACH);
        let (a, b) = spot.hands[0][hero];
        let blocked = card_bit(a) | card_bit(b);
        let villain = in_range(1)
            .filter(|&i| (card_bit(spot.hands[1][i].0) | card_bit(spot.hands[1][i].1)) & blocked == 0)
            .max_by(|&x, &y| freq(after, bet, x).total_cmp(&freq(after, bet, y)))
            .unwrap();
        let mut rng = Rng::new(7);
        for _ in 0..20 {
            let mut h = PostflopHand::new_hand(lib.clone(), 0, 0, &mut rng).unwrap();
            h.cards = [spot.hands[0][hero], spot.hands[1][villain]];
            let used: Vec<Card> = spot.board.iter().copied().chain([h.cards[0].0, h.cards[0].1, h.cards[1].0, h.cards[1].1]).collect();
            h.deck = (0..52u8).filter(|c| !used.contains(c)).collect();
            let fb = h.answer(check, &mut rng);
            assert!(fb.freqs_trusted, "a first decision is on the solution's path");
            // The villain must bet so the hero decides again on the flop.
            let Stage::Flop(n) = h.stage else { continue };
            let node = spot.nodes[n as usize].clone();
            let d = h.decision().expect("hero faces the bet");
            // The most frequent stored action that does not start a turn solve.
            let j = (0..d.freqs.len())
                .filter(|&j| !matches!(node.next[j], Next::Line(_)))
                .max_by(|&x, &y| d.freqs[x].total_cmp(&d.freqs[y]))
                .unwrap();
            let fb = h.answer(j, &mut rng);
            assert!(!fb.freqs_trusted, "the hero's hand never gets here in the solution");
            assert_eq!(fb.grade, by_ev(&d.evs, &d.freqs, j, d.pot, false).0, "graded by EV alone");
            return;
        }
        panic!("the villain never bet in 20 attempts (bet frequency {})", freq(after, bet, villain));
    }

    #[test]
    fn plays_hands_to_the_end() {
        let lib = library();
        let mut rng = Rng::new(5);
        let mut saw_turn = false;
        let mut saw_showdown = false;
        for k in 0..12 {
            let role = if k % 2 == 0 { "oop" } else { "ip" };
            let mut h = PostflopHand::deal(lib.clone(), &[], role, 0, &mut rng).unwrap();
            assert_eq!(h.hero, if k % 2 == 0 { 0 } else { 1 });
            let mut guard = 0;
            while !h.is_over() {
                guard += 1;
                assert!(guard < 10_000, "hand stuck");
                if h.is_solving() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    h.poll(&mut rng);
                    continue;
                }
                let d = h.decision().expect("hero decision");
                assert!((d.freqs.iter().sum::<f32>() - 1.0).abs() < 0.02);
                assert!(d.evs.iter().all(|e| e.is_finite()), "{:?}", d.evs);
                if d.street != "Flop" {
                    saw_turn = true;
                }
                // Mostly follow the solution so hands get deep.
                let i = d.freqs.iter().enumerate().max_by(|a, b| a.1.partial_cmp(b.1).unwrap()).unwrap().0;
                let fb = h.answer(i, &mut rng);
                assert!(fb.grade.is_correct(), "playing the most frequent action must grade well: {fb:?}");
            }
            if matches!(h.stage, Stage::Over(PostEnd::Showdown { .. })) {
                saw_showdown = true;
                assert_eq!(h.board.len(), 5);
            }
            assert!(h.end_text().is_some());
        }
        assert!(saw_turn, "some hands should reach the turn");
        assert!(saw_showdown, "some hands should reach showdown");
    }
}
