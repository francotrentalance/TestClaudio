//! Offline generation of the training content (see `src/bin/gen_data.rs`).

use holdem_solver::card::{Card, parse_cards};
use holdem_solver::postflop::explore::Explorer;
use holdem_solver::postflop::{Action, NodeKind, PostflopConfig, PostflopGame, StreetSizes};
use holdem_solver::preflop::cli::solve as solve_preflop_iterations;
use holdem_solver::preflop::{PKind, PreflopConfig, PreflopSolver};
use holdem_solver::range::NUM_CLASSES;

use crate::model::{Act, ActKind, FlopLine, FlopNode, FlopSpot, Next, PreflopPack, quantize};

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

pub fn cash_config(iterations: u64) -> PreflopConfig {
    PreflopConfig { iterations, ..PreflopConfig::default() }
}

/// Push/fold at `stack` big blinds with a 12.5% ante, as in most tournaments.
pub fn pushfold_config(stack: f64, iterations: u64) -> PreflopConfig {
    PreflopConfig { stack, ante: 0.125, pushfold: true, iterations, ..PreflopConfig::default() }
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

/// Solves one flop and extracts the training data.
pub fn flop_spot(kind: &str, board: &str, mut log: impl FnMut(&str)) -> Result<FlopSpot, String> {
    let config = spot_config(kind, board)?;
    let mut game = PostflopGame::new(config.clone())?;
    let mut expl = f64::INFINITY;
    for it in 1..=config.iterations {
        game.iterate();
        if it % 10 == 0 || it == config.iterations {
            expl = game.exploitability_pct();
            log(&format!("{board}: iteration {it}, exploitability {expl:.3}%"));
            if expl <= config.target_exploitability {
                break;
            }
        }
    }
    game.finalize();
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
