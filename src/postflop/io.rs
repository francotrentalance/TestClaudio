//! Saving and loading solutions, and JSON export.
//!
//! A saved solution holds the config (the tree is rebuilt from it) and the
//! normalized average strategy of every player node.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};

use serde_json::{Value, json};

use crate::card::card_to_string;
use crate::postflop::config::PostflopConfig;
use crate::postflop::explore::{Explorer, PLAYER_NAMES};
use crate::postflop::game::PostflopGame;
use crate::postflop::tree::NodeKind;

const MAGIC: &[u8; 8] = b"HSPOST01";

pub fn save(game: &mut PostflopGame, path: &str) -> Result<(), String> {
    let strategy = game.strategy_vec();
    let cfg = serde_json::to_vec(&game.config).map_err(|e| e.to_string())?;
    let mut w = BufWriter::new(File::create(path).map_err(|e| format!("{path}: {e}"))?);
    let io = |e: std::io::Error| e.to_string();
    w.write_all(MAGIC).map_err(io)?;
    w.write_all(&(cfg.len() as u64).to_le_bytes()).map_err(io)?;
    w.write_all(&cfg).map_err(io)?;
    w.write_all(&game.iteration.to_le_bytes()).map_err(io)?;
    w.write_all(&(strategy.len() as u64).to_le_bytes()).map_err(io)?;
    for chunk in strategy.chunks(1 << 16) {
        let bytes: Vec<u8> = chunk.iter().flat_map(|x| x.to_le_bytes()).collect();
        w.write_all(&bytes).map_err(io)?;
    }
    w.flush().map_err(io)
}

pub fn load(path: &str) -> Result<PostflopGame, String> {
    let mut r = BufReader::new(File::open(path).map_err(|e| format!("{path}: {e}"))?);
    let io = |e: std::io::Error| e.to_string();
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).map_err(io)?;
    if &magic != MAGIC {
        return Err(format!("{path} is not a postflop solution file"));
    }
    let mut u64b = [0u8; 8];
    r.read_exact(&mut u64b).map_err(io)?;
    let mut cfg = vec![0u8; u64::from_le_bytes(u64b) as usize];
    r.read_exact(&mut cfg).map_err(io)?;
    let config: PostflopConfig = serde_json::from_slice(&cfg).map_err(|e| e.to_string())?;
    let mut u32b = [0u8; 4];
    r.read_exact(&mut u32b).map_err(io)?;
    let iteration = u32::from_le_bytes(u32b);
    r.read_exact(&mut u64b).map_err(io)?;
    let n = u64::from_le_bytes(u64b) as usize;
    let mut bytes = vec![0u8; n * 4];
    r.read_exact(&mut bytes).map_err(io)?;
    let strategy: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    let mut game = PostflopGame::with_strategy(config, strategy)?;
    game.iteration = iteration;
    Ok(game)
}

/// Exports every decision node of the starting street (up to the first chance
/// node) with per-combo strategy, EV and equity.
pub fn export_json(game: &PostflopGame, exploitability_pct: Option<f64>) -> Value {
    let mut ex = Explorer::new(game);
    let mut nodes = Vec::new();
    walk(&mut ex, &mut nodes);
    let ev = game.expected_values();
    json!({
        "config": game.config,
        "iterations": game.iteration,
        "exploitability_pct_of_pot": exploitability_pct,
        "ev": { "oop": ev[0], "ip": ev[1] },
        "nodes": nodes,
    })
}

fn walk(ex: &mut Explorer, out: &mut Vec<Value>) {
    let node = ex.node().clone();
    let NodeKind::Player(q) = node.kind else { return };
    let q = q as usize;
    let actions = ex.child_actions();
    let strat = ex.strategy();
    let stats = ex.hand_stats(q);
    let (_, overall) = ex.class_frequencies();
    let nh = ex.game.num_hands(q);
    let reach = ex.frame().reach[q].clone();
    let mut hands = serde_json::Map::new();
    for h in 0..nh {
        if reach[h] <= 0.0 {
            continue;
        }
        let (a, b) = ex.actual_combo(q, h);
        let name = format!("{}{}", card_to_string(a.max(b)), card_to_string(a.min(b)));
        let s: Vec<f64> = (0..actions.len()).map(|i| round4(strat[i * nh + h] as f64)).collect();
        hands.insert(
            name,
            json!({
                "weight": round4(reach[h] as f64),
                "strategy": s,
                "ev": round4(stats.ev[h] as f64),
                "equity": round4(stats.equity[h] as f64),
            }),
        );
    }
    out.push(json!({
        "line": ex.frame().line,
        "player": PLAYER_NAMES[q],
        "pot": node.pot(),
        "actions": actions.iter().map(|a| a.label()).collect::<Vec<_>>(),
        "frequencies": overall.iter().map(|&x| round4(x)).collect::<Vec<_>>(),
        "hands": hands,
    }));
    for i in 0..actions.len() {
        ex.take_action(i);
        walk(ex, out);
        ex.back();
    }
}

fn round4(x: f64) -> f64 {
    if x.is_finite() { (x * 1e4).round() / 1e4 } else { f64::NAN }
}
