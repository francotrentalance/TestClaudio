//! Offline generation of the training content (see `src/bin/gen_data.rs`).

use holdem_solver::card::{Card, Rng, combo_index, parse_cards};
use holdem_solver::postflop::explore::Explorer;
use holdem_solver::postflop::{Action, NodeKind, PostflopConfig, PostflopGame, StreetSizes};
use holdem_solver::preflop::cli::solve as solve_preflop_iterations;
use holdem_solver::preflop::{EqrModel, PAction, PKind, PreflopConfig, PreflopSolver};
use holdem_solver::range::{NUM_CLASSES, Range, class_ranks, combo_class};

use crate::model::{Act, ActKind, FlopLine, FlopNode, FlopSpot, LoadedPreflop, Next, PreflopPack, quantize};

/// Solves a preflop game and quantizes its average strategy.
pub fn preflop_pack(name: &str, config: PreflopConfig, mut progress: impl FnMut(u64, u64)) -> Result<PreflopPack, String> {
    let total = config.iterations;
    let solver = PreflopSolver::new(config)?;
    solve_preflop_iterations(&solver, |_, done| progress(done, total));
    let mut strategy = vec![0u8; solver.tree.storage_len];
    for node in &solver.tree.nodes {
        if let PKind::Decision(_) = node.kind {
            let na = node.num_children as usize;
            for class in 0..NUM_CLASSES {
                let base = node.storage as usize + class * na;
                strategy[base..base + na].copy_from_slice(&quantize(&solver.average(node, class)));
            }
        }
    }
    Ok(PreflopPack {
        name: name.to_string(),
        config: solver.cfg.clone(),
        strategy,
        iterations: solver.iterations.load(std::sync::atomic::Ordering::Relaxed),
    })
}

/// 6-max 100bb cash. With an [`EqrModel`] the flop is valued by measured,
/// hand-dependent equity realization (and plain equity, exponent 1).
pub fn cash_config(iterations: u64, eqr: Option<EqrModel>) -> PreflopConfig {
    match eqr {
        Some(m) => PreflopConfig { iterations, eqr: Some(m), realization_exponent: 1.0, ..PreflopConfig::default() },
        None => PreflopConfig { iterations, ..PreflopConfig::default() },
    }
}

pub const PUSHFOLD_ANTE: f64 = 0.125;

/// Push/fold with `stack` big blinds before posting, as push/fold charts
/// count it, and a 12.5% ante as in most tournaments.
pub fn pushfold_config(stack: f64, iterations: u64) -> PreflopConfig {
    PreflopConfig {
        stack: stack - PUSHFOLD_ANTE,
        ante: PUSHFOLD_ANTE,
        pushfold: true,
        iterations,
        ..PreflopConfig::default()
    }
}

pub const SRP: &str = "srp_btn_bb";
pub const THREE_BET: &str = "3bp_bb_btn";

/// BB's 3-bet range against a BTN open (illustrative, ~12%).
pub const BB_3BET_VS_BTN: &str = "TT+,AQs+,AKo,AQo:0.5,AJs:0.5,ATs:0.5,A5s-A2s,KQs,KJs:0.5,KTs:0.5,QJs:0.5,JTs:0.5,T9s:0.5,98s:0.5,87s:0.5,76s:0.5,65s:0.5";
/// BTN's flatting range against that 3-bet (illustrative).
pub const BTN_CALL_VS_3BET: &str = "JJ:0.5,TT:0.5,99-22,AQs:0.5,AJs,ATs,A9s:0.5,A5s:0.5,KQs,KJs,KTs,QJs,QTs,JTs,J9s,T9s,T8s,98s,97s,87s,76s,65s,54s,AQo,AJo:0.5,KQo:0.5";

pub fn spot_label(kind: &str) -> &'static str {
    match kind {
        SRP => "Single-raised pot: BTN vs BB",
        THREE_BET => "3-bet pot: BB vs BTN",
        _ => "Postflop spot",
    }
}

/// Preflop story shown before the flop.
pub fn spot_story(kind: &str) -> &'static str {
    match kind {
        SRP => "BTN opens to 2.5bb, SB folds, BB calls.",
        THREE_BET => "BTN opens to 2.5bb, SB folds, BB 3-bets to 10bb, BTN calls.",
        _ => "",
    }
}

/// Position names of the OOP and IP player of a spot.
pub fn spot_positions(kind: &str) -> [&'static str; 2] {
    match kind {
        SRP | THREE_BET => ["BB", "BTN"],
        _ => ["OOP", "IP"],
    }
}

/// Ranges (`[OOP, IP]`) of the postflop spots.
#[derive(Clone, Debug)]
pub struct SpotRanges {
    pub srp: [Range; 2],
    pub three_bet: [Range; 2],
}

impl SpotRanges {
    /// The illustrative ranges built into the solver and this module.
    pub fn illustrative() -> Result<SpotRanges, String> {
        Ok(SpotRanges {
            srp: [Range::parse(holdem_solver::postflop::DEFAULT_BB_RANGE)?, Range::parse(holdem_solver::postflop::DEFAULT_BTN_RANGE)?],
            three_bet: [Range::parse(BB_3BET_VS_BTN)?, Range::parse(BTN_CALL_VS_3BET)?],
        })
    }

    /// The ranges the 6-max cash solution reaches each spot with: BTN opens,
    /// SB folds, and BB calls (single-raised pot) or 3-bets and BTN calls
    /// (3-bet pot). Weights below 3% are dropped as noise.
    pub fn from_cash(pack: &LoadedPreflop) -> Result<SpotRanges, String> {
        let tree = &pack.tree;
        let child = |node: usize, pred: &dyn Fn(PAction) -> bool| -> Result<usize, String> {
            tree.nodes[node]
                .children()
                .find(|&c| pred(tree.nodes[c].action))
                .ok_or_else(|| format!("cash pack: no such action at node {node}"))
        };
        let fold = |a: PAction| a == PAction::Fold;
        let raise = |a: PAction| matches!(a, PAction::Raise(_));
        let call = |a: PAction| a == PAction::Call;
        // UTG, HJ and CO fold; BTN to act.
        let mut btn = 0;
        for _ in 0..3 {
            btn = child(btn, &fold)?;
        }
        let btn_open = child(btn, &raise)?;
        let bb = child(btn_open, &fold)?;
        let bb_call = child(bb, &call)?;
        let bb_3bet = child(bb, &raise)?;
        let btn_call = child(bb_3bet, &call)?;
        let freq = |node: usize, taken: usize| -> Vec<f32> {
            let n = &tree.nodes[node];
            let i = taken - n.first_child as usize;
            (0..NUM_CLASSES).map(|c| pack.strategy(n, c)[i]).collect()
        };
        let clean = |v: Vec<f32>| -> Range {
            Range::from_class_weights(&v.into_iter().map(|w| if w < 0.03 { 0.0 } else { w }).collect::<Vec<_>>())
        };
        let open = freq(btn, btn_open);
        let call_3bet = freq(bb_3bet, btn_call);
        let btn_vs_3bet: Vec<f32> = open.iter().zip(&call_3bet).map(|(o, c)| o * c).collect();
        Ok(SpotRanges {
            srp: [clean(freq(bb, bb_call)), clean(open)],
            three_bet: [clean(freq(bb, bb_3bet)), clean(btn_vs_3bet)],
        })
    }

    pub fn of(&self, kind: &str) -> Result<&[Range; 2], String> {
        match kind {
            SRP => Ok(&self.srp),
            THREE_BET => Ok(&self.three_bet),
            _ => Err(format!("unknown spot kind '{kind}'")),
        }
    }
}

/// Pot, stack and sizes of a spot, with the illustrative ranges.
pub fn spot_config(kind: &str, board: &str) -> Result<PostflopConfig, String> {
    let base = PostflopConfig {
        board: board.to_string(),
        iterations: 400,
        target_exploitability: 0.5,
        ..PostflopConfig::default()
    };
    Ok(match kind {
        SRP => base,
        THREE_BET => PostflopConfig {
            oop_range: BB_3BET_VS_BTN.into(),
            ip_range: BTN_CALL_VS_3BET.into(),
            pot: 20.5,
            stack: 90.0,
            flop: StreetSizes::symmetric("33%, 75%", "3x"),
            ..base
        },
        _ => return Err(format!("unknown spot kind '{kind}'")),
    })
}

/// [`spot_config`] with the given ranges.
pub fn spot_config_with(kind: &str, board: &str, ranges: &SpotRanges) -> Result<PostflopConfig, String> {
    let [oop, ip] = ranges.of(kind)?;
    Ok(PostflopConfig { oop_range: oop.to_range_string(), ip_range: ip.to_range_string(), ..spot_config(kind, board)? })
}

/// A representative spread of flop textures (suits only matter up to isomorphism).
pub const FLOPS: &[&str] = &[
    "As7d2c", "Ks7d2c", "Qs8d3c", "Js7d3c", "9s5d2c", "5s3d2c", "9s8d7c", "KsQdTc", "AhKh5c", "AsQhJh", "QsJh4h",
    "Jh9h4c", "Th9h8c", "6h5h4c", "As5h4h", "Ks8h7h", "KsKd4c", "8s8d2c", "7s7d3c", "AsAd9c", "TsTd6c", "Ks9s3s",
    "8h6h4h", "AhJh6h",
];

fn act_of(a: Action) -> Act {
    match a {
        Action::Fold => Act { kind: ActKind::Fold, to: 0.0 },
        Action::Check => Act { kind: ActKind::Check, to: 0.0 },
        Action::Call => Act { kind: ActKind::Call, to: 0.0 },
        Action::Bet(x) => Act { kind: ActKind::Bet, to: x },
        Action::Raise(x) => Act { kind: ActKind::Raise, to: x },
        Action::AllIn(x) => Act { kind: ActKind::AllIn, to: x },
        Action::None | Action::Deal(_) => unreachable!("not a player action"),
    }
}

/// Iterates until the config's target exploitability or iteration cap; returns the exploitability.
fn solve_postflop(game: &mut PostflopGame, config: &PostflopConfig, log: &mut impl FnMut(&str)) -> f64 {
    let mut expl = f64::INFINITY;
    for it in 1..=config.iterations {
        game.iterate();
        if it % 10 == 0 || it == config.iterations {
            expl = game.exploitability_pct();
            log(&format!("{}: iteration {it}, exploitability {expl:.3}%", config.board));
            if expl <= config.target_exploitability {
                break;
            }
        }
    }
    game.finalize();
    expl
}

/// Solves one flop with the illustrative ranges and extracts the training data.
pub fn flop_spot(kind: &str, board: &str, log: impl FnMut(&str)) -> Result<FlopSpot, String> {
    flop_spot_with(kind, board, &SpotRanges::illustrative()?, log)
}

/// Solves one flop with `ranges` and extracts the training data.
pub fn flop_spot_with(kind: &str, board: &str, ranges: &SpotRanges, mut log: impl FnMut(&str)) -> Result<FlopSpot, String> {
    let config = spot_config_with(kind, board, ranges)?;
    let mut game = PostflopGame::new(config.clone())?;
    let expl = solve_postflop(&mut game, &config, &mut log);
    let mut ex = Explorer::new(&game);
    let mut nodes = Vec::new();
    let mut lines = Vec::new();
    walk(&mut ex, &config, &mut nodes, &mut lines);
    Ok(FlopSpot {
        kind: kind.to_string(),
        board: parse_cards(board)?,
        hands: game.hands.clone(),
        weights: game.initial_weights.clone(),
        config,
        nodes,
        lines,
        exploitability_pct: expl,
    })
}

fn walk(ex: &mut Explorer, config: &PostflopConfig, nodes: &mut Vec<FlopNode>, lines: &mut Vec<FlopLine>) -> Next {
    let node = ex.node().clone();
    match node.kind {
        NodeKind::Fold(f) => Next::Fold { folder: f, contrib: node.contrib },
        NodeKind::Chance | NodeKind::Showdown => {
            let half = config.pot as f32 / 2.0;
            let all_in = node.contrib[0] - half >= config.stack as f32 - 1e-3;
            lines.push(FlopLine { contrib: node.contrib, all_in, reach: ex.frame().reach.clone() });
            Next::Line((lines.len() - 1) as u32)
        }
        NodeKind::Player(q) => {
            let idx = nodes.len();
            let actions: Vec<Act> = ex.child_actions().into_iter().map(act_of).collect();
            let na = actions.len();
            let nh = ex.game.num_hands(q as usize);
            let strat = ex.strategy();
            let mut strategy = vec![0u8; na * nh];
            for h in 0..nh {
                let probs: Vec<f32> = (0..na).map(|a| strat[a * nh + h]).collect();
                for (a, v) in quantize(&probs).into_iter().enumerate() {
                    strategy[a * nh + h] = v;
                }
            }
            let ev: Vec<f32> = ex.action_evs().into_iter().flatten().collect();
            nodes.push(FlopNode { player: q, contrib: node.contrib, actions, next: Vec::new(), strategy, ev });
            let mut next = Vec::with_capacity(na);
            for i in 0..na {
                ex.take_action(i);
                next.push(walk(ex, config, nodes, lines));
                ex.back();
            }
            nodes[idx].next = next;
            Next::Node(idx as u32)
        }
    }
}

/// The turn subgame after a flop line: same sizes, the line's ranges.
pub fn turn_config(spot: &FlopSpot, line: &FlopLine, turn: Card) -> PostflopConfig {
    let mut board = spot.board.clone();
    board.push(turn);
    let half = spot.config.pot as f32 / 2.0;
    let invested = line.contrib[0] - half;
    PostflopConfig {
        board: holdem_solver::card::cards_to_string(&board),
        pot: (line.contrib[0] + line.contrib[1]) as f64,
        stack: (spot.config.stack - invested as f64).max(0.0),
        iterations: 300,
        target_exploitability: 0.5,
        ..spot.config.clone()
    }
}

// ---------------------------------------------------------------------------
// Equity realization, measured from postflop solves for the preflop model.

/// Per-class totals over solved flops, per player `[OOP, IP]`: range weight,
/// weighted EV and weighted equity share of the pot at the flop.
#[derive(Clone, Debug)]
pub struct EqrSums {
    pub weight: [Vec<f64>; 2],
    pub ev: [Vec<f64>; 2],
    pub equity_pot: [Vec<f64>; 2],
}

impl Default for EqrSums {
    fn default() -> Self {
        let z = || vec![0.0; NUM_CLASSES];
        EqrSums { weight: [z(), z()], ev: [z(), z()], equity_pot: [z(), z()] }
    }
}

/// Random flops for measuring realization (fixed seed, so runs are repeatable).
pub fn sample_flops(n: usize, seed: u64) -> Vec<String> {
    let mut rng = Rng::new(seed);
    (0..n)
        .map(|_| {
            let mut cards: Vec<Card> = Vec::new();
            while cards.len() < 3 {
                let c = rng.below(52) as Card;
                if !cards.contains(&c) {
                    cards.push(c);
                }
            }
            holdem_solver::card::cards_to_string(&cards)
        })
        .collect()
}

/// Solves `kind` on `board` with `ranges` (to a looser target than the
/// training library) and adds every hand's EV and equity at the flop to `sums`.
pub fn measure_eqr(kind: &str, board: &str, ranges: &SpotRanges, sums: &mut EqrSums, mut log: impl FnMut(&str)) -> Result<f64, String> {
    let config = PostflopConfig { iterations: 200, target_exploitability: 2.0, ..spot_config(kind, board)? };
    // Every hand outside the range gets a tiny "probe" weight: too small to move
    // the equilibrium, but the solver then also works out how each of those
    // hands would play and realize there, so no class has to be guessed.
    let probe = |r: &Range| Range { weights: r.weights.iter().map(|&w| w.max(EQR_PROBE)).collect() };
    let [oop, ip] = ranges.of(kind)?;
    let mut game = PostflopGame::with_ranges(config.clone(), [probe(oop), probe(ip)])?;
    let expl = solve_postflop(&mut game, &config, &mut log);
    let ex = Explorer::new(&game);
    let pot = config.pot;
    for p in 0..2 {
        let stats = ex.hand_stats(p);
        for (h, &(a, b)) in game.hands[p].iter().enumerate() {
            let w = game.initial_weights[p][h] as f64;
            let (ev, eq) = (stats.ev[h], stats.equity[h]);
            if w <= 0.0 || !ev.is_finite() || !eq.is_finite() {
                continue;
            }
            let c = combo_class(combo_index(a, b));
            sums.weight[p][c] += w;
            sums.ev[p][c] += w * ev as f64;
            sums.equity_pot[p][c] += w * eq as f64 * pot;
        }
    }
    Ok(expl)
}

/// Range weight of hands outside a range while measuring realization.
const EQR_PROBE: f32 = 0.02;
/// Classes with less total weight than this (in combo-flops) are not measured.
const EQR_MIN_WEIGHT: f64 = 0.5;
const EQR_CLAMP: (f32, f32) = (0.2, 2.0);

/// Measured realization per class, `None` where the class was (almost) never in range.
pub fn eqr_measured(sums: &EqrSums, p: usize) -> Vec<Option<f32>> {
    (0..NUM_CLASSES)
        .map(|c| {
            (sums.weight[p][c] >= EQR_MIN_WEIGHT && sums.equity_pot[p][c] > 0.0)
                .then(|| ((sums.ev[p][c] / sums.equity_pot[p][c]) as f32).clamp(EQR_CLAMP.0, EQR_CLAMP.1))
        })
        .collect()
}

/// Fills unmeasured classes from the closest measured class of the same
/// shape (pair, suited, offsuit) by rank distance.
fn fill_nearest(measured: &[Option<f32>]) -> Vec<f32> {
    let mean = {
        let v: Vec<f32> = measured.iter().flatten().copied().collect();
        if v.is_empty() { 1.0 } else { v.iter().sum::<f32>() / v.len() as f32 }
    };
    let shape = |c: usize| {
        let (h, l, s) = class_ranks(c);
        if h == l { 0 } else if s { 1 } else { 2 }
    };
    (0..NUM_CLASSES)
        .map(|c| {
            if let Some(x) = measured[c] {
                return x;
            }
            let (h, l, _) = class_ranks(c);
            let mut best = (u32::MAX, 0.0f32, 0u32);
            for (d, m) in measured.iter().enumerate() {
                let Some(x) = *m else { continue };
                if shape(d) != shape(c) {
                    continue;
                }
                let (dh, dl, _) = class_ranks(d);
                let dist = (h as i32 - dh as i32).unsigned_abs() + (l as i32 - dl as i32).unsigned_abs();
                if dist < best.0 {
                    best = (dist, x, 1);
                } else if dist == best.0 {
                    best = (dist, best.1 + x, best.2 + 1);
                }
            }
            if best.2 == 0 { mean } else { best.1 / best.2 as f32 }
        })
        .collect()
}

/// Builds the preflop model from realization measured in single-raised
/// (`deep`) and 3-bet (`shallow`) pots. With probe weights every class is
/// normally measured; any that is not falls back to a least-squares line
/// through the classes measured in both (shallow) or its nearest measured
/// neighbour (deep).
pub fn eqr_model(deep: &EqrSums, shallow: &EqrSums, deep_spr: f64, shallow_spr: f64) -> EqrModel {
    let mut tables = Vec::new();
    for p in 0..2 {
        let d = fill_nearest(&eqr_measured(deep, p));
        let s = eqr_measured(shallow, p);
        let pairs: Vec<(f64, f64)> = (0..NUM_CLASSES).filter_map(|c| s[c].map(|x| (d[c] as f64, x as f64))).collect();
        let n = pairs.len() as f64;
        let (mx, my) = (pairs.iter().map(|q| q.0).sum::<f64>() / n.max(1.0), pairs.iter().map(|q| q.1).sum::<f64>() / n.max(1.0));
        let sxx: f64 = pairs.iter().map(|q| (q.0 - mx) * (q.0 - mx)).sum();
        let sxy: f64 = pairs.iter().map(|q| (q.0 - mx) * (q.1 - my)).sum();
        let slope = if sxx > 0.0 { (sxy / sxx).clamp(0.0, 1.5) } else { 0.0 };
        let fit = |x: f32| ((my + slope * (x as f64 - mx)) as f32).clamp(EQR_CLAMP.0, EQR_CLAMP.1);
        let s: Vec<f32> = (0..NUM_CLASSES).map(|c| s[c].unwrap_or_else(|| if pairs.is_empty() { 1.0 } else { fit(d[c]) })).collect();
        tables.push((d, s));
    }
    let (ip, oop) = (tables.pop().unwrap(), tables.pop().unwrap());
    EqrModel { deep_spr, shallow_spr, deep_oop: oop.0, deep_ip: ip.0, shallow_oop: oop.1, shallow_ip: ip.1 }
}

/// Stack-to-pot ratio at the flop of a spot.
pub fn spot_spr(kind: &str) -> Result<f64, String> {
    let c = spot_config(kind, "Ks7d2c")?;
    Ok(c.stack / c.pot)
}
