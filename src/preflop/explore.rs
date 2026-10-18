//! Preflop charts, interactive explorer, saving and export.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};

use serde_json::{Value, json};

use crate::display::{self, ActionKind, bold, dim, paint, pct1};
use crate::postflop::tree::fmt_chips;
use crate::preflop::config::PreflopConfig;
use crate::preflop::mccfr::PreflopSolver;
use crate::preflop::tree::{PAction, PKind, PNode};
use crate::range::{NUM_CLASSES, Range, class_name, class_num_combos};

#[derive(Clone)]
pub struct PFrame {
    pub node: usize,
    /// Probability that each seat holds each class given its own actions so far.
    pub reach: Vec<[f32; NUM_CLASSES]>,
    pub line: Vec<String>,
}

pub struct PExplorer<'a> {
    pub solver: &'a PreflopSolver,
    pub frames: Vec<PFrame>,
}

pub fn action_kinds(actions: &[PAction]) -> Vec<ActionKind> {
    let mut level = 0;
    actions
        .iter()
        .map(|a| match a {
            PAction::Fold => ActionKind::Fold,
            PAction::Check | PAction::Call => ActionKind::Passive,
            PAction::AllIn(_) => ActionKind::Aggressive { level: 0, allin: true },
            _ => {
                level += 1;
                ActionKind::Aggressive { level: level - 1, allin: false }
            }
        })
        .collect()
}

impl<'a> PExplorer<'a> {
    pub fn new(solver: &'a PreflopSolver) -> Self {
        let root = PFrame { node: 0, reach: vec![[1.0; NUM_CLASSES]; solver.cfg.players], line: Vec::new() };
        PExplorer { solver, frames: vec![root] }
    }

    pub fn frame(&self) -> &PFrame {
        self.frames.last().unwrap()
    }

    pub fn node(&self) -> &PNode {
        &self.solver.tree.nodes[self.frame().node]
    }

    pub fn names(&self) -> Vec<&'static str> {
        self.solver.cfg.position_names()
    }

    pub fn actions(&self) -> Vec<PAction> {
        self.node().children().map(|c| self.solver.tree.nodes[c].action).collect()
    }

    pub fn take_action(&mut self, i: usize) {
        let node = self.node().clone();
        let PKind::Decision(q) = node.kind else { return };
        let q = q as usize;
        let mut f = self.frame().clone();
        for c in 0..NUM_CLASSES {
            f.reach[q][c] *= self.solver.average(&node, c)[i];
        }
        let child = node.first_child as usize + i;
        f.node = child;
        f.line.push(format!("{} {}", self.names()[q], self.solver.tree.nodes[child].action.label()));
        self.frames.push(f);
    }

    pub fn apply(&mut self, token: &str) -> Result<(), String> {
        if !matches!(self.node().kind, PKind::Decision(_)) {
            return Err("terminal node; 'u' to go back".into());
        }
        let actions = self.actions();
        let t = token.to_ascii_lowercase();
        let idx = if let Ok(n) = t.parse::<usize>() {
            (n >= 1 && n <= actions.len()).then(|| n - 1)
        } else {
            actions.iter().position(|a| match a {
                PAction::Fold => t == "f",
                PAction::Check => t == "x" || t == "k",
                PAction::Call => t == "c",
                PAction::Raise(_) => t == "r",
                PAction::AllIn(_) => t == "a" || t == "j",
                PAction::None => false,
            })
        };
        let i = idx.ok_or_else(|| format!("no action '{token}' here"))?;
        self.take_action(i);
        Ok(())
    }

    /// Follows the folds to `seat`'s first decision (its raise-first-in spot).
    pub fn goto_first_in(&mut self, seat: usize) -> bool {
        self.frames.truncate(1);
        loop {
            match self.node().kind {
                PKind::Decision(q) if q as usize == seat => return true,
                PKind::Decision(_) => {
                    let Some(i) = self.actions().iter().position(|a| *a == PAction::Fold) else { return false };
                    self.take_action(i);
                }
                _ => return false,
            }
        }
    }

    /// Per-class strategy at the current decision, and combo-weighted overall frequencies.
    pub fn frequencies(&self) -> (Vec<Option<Vec<f32>>>, Vec<f64>) {
        let node = self.node();
        let PKind::Decision(q) = node.kind else { return (Vec::new(), Vec::new()) };
        let reach = &self.frame().reach[q as usize];
        let na = node.num_children as usize;
        let mut overall = vec![0.0f64; na];
        let mut total = 0.0;
        let per: Vec<Option<Vec<f32>>> = (0..NUM_CLASSES)
            .map(|c| {
                if reach[c] < 1e-4 {
                    return None;
                }
                let s = self.solver.average(node, c);
                let w = reach[c] as f64 * class_num_combos(c) as f64;
                for a in 0..na {
                    overall[a] += w * s[a] as f64;
                }
                total += w;
                Some(s)
            })
            .collect();
        if total > 0.0 {
            overall.iter_mut().for_each(|x| *x /= total);
        }
        (per, overall)
    }

    pub fn range_of(&self, seat: usize) -> Range {
        let r = &self.frame().reach[seat];
        let w: Vec<f32> = r.iter().map(|&x| if x < 0.005 { 0.0 } else { (x * 1000.0).round() / 1000.0 }).collect();
        Range::from_class_weights(&w)
    }

    pub fn range_combos(&self, seat: usize) -> f64 {
        let r = &self.frame().reach[seat];
        (0..NUM_CLASSES).map(|c| r[c] as f64 * class_num_combos(c) as f64).sum()
    }
}

pub fn render(ex: &PExplorer) -> String {
    let node = ex.node();
    let names = ex.names();
    let f = ex.frame();
    let mut s = String::new();
    let line = if f.line.is_empty() { "(start)".to_string() } else { f.line.join(" > ") };
    s.push_str(&format!("{} {}\n{} {}\n", bold("Line:"), line, bold("Pot:"), fmt_chips(node.pot)));
    match node.kind {
        PKind::Decision(q) => {
            let q = q as usize;
            let actions = ex.actions();
            let kinds = action_kinds(&actions);
            let (per, overall) = ex.frequencies();
            s.push_str(&format!(
                "{} to act ({:.0} combos, {:.1}% of hands)\n",
                bold(names[q]),
                ex.range_combos(q),
                100.0 * ex.range_combos(q) / 1326.0
            ));
            for (i, a) in actions.iter().enumerate() {
                let tag = paint(&format!(" {} ", a.code()), kinds[i]);
                s.push_str(&format!("  [{}] {} {:<14} {:>6}\n", i + 1, tag, a.label(), pct1(overall[i])));
            }
            s.push('\n');
            s.push_str(&display::grid(|c| match &per[c] {
                None => dim(&format!("{:<6}", class_name(c))),
                Some(fr) => {
                    let (best, v) = fr
                        .iter()
                        .enumerate()
                        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                        .map(|(i, &v)| (i, v))
                        .unwrap();
                    paint(&format!("{:<2}{:>3} ", actions[best].code(), (v * 100.0).round() as i64), kinds[best])
                }
            }));
        }
        PKind::Fold(w) => s.push_str(&format!("Terminal: {} wins the pot.\n", names[w as usize])),
        PKind::AllIn => s.push_str("Terminal: all-in showdown.\n"),
        PKind::Flop => {
            let players: Vec<&str> = (0..ex.solver.cfg.players)
                .filter(|&p| node.active >> p & 1 == 1)
                .map(|p| names[p])
                .collect();
            s.push_str(&format!("Terminal: flop, {} see it. 'spot' prints the postflop command.\n", players.join(", ")));
        }
    }
    s
}

pub fn render_action_grid(ex: &PExplorer, i: usize) -> Result<String, String> {
    let actions = ex.actions();
    if i >= actions.len() {
        return Err("no such action".into());
    }
    let (per, overall) = ex.frequencies();
    let kinds = action_kinds(&actions);
    let mut s = format!("{} ({} overall)\n", actions[i].label(), pct1(overall[i]));
    s.push_str(&display::grid(|c| match &per[c] {
        None => dim("  .  "),
        Some(fr) => {
            let txt = format!("{:>4} ", (fr[i] * 100.0).round() as i64);
            if fr[i] >= 0.5 { paint(&txt, kinds[i]) } else { txt }
        }
    }));
    Ok(s)
}

pub fn render_range(ex: &PExplorer, seat: usize) -> String {
    let r = &ex.frame().reach[seat];
    let mut s = format!(
        "{} range here: {:.0} combos ({:.1}%)\n",
        ex.names()[seat],
        ex.range_combos(seat),
        100.0 * ex.range_combos(seat) / 1326.0
    );
    s.push_str(&display::grid(|c| {
        if r[c] < 0.005 {
            dim("  .  ")
        } else {
            let txt = format!("{:>4} ", (r[c] * 100.0).round() as i64);
            if r[c] >= 0.5 { paint(&txt, ActionKind::Passive) } else { txt }
        }
    }));
    s.push_str(&format!("{}\n", ex.range_of(seat).to_range_string()));
    s
}

/// The postflop command for a heads-up flop terminal.
pub fn spot_command(ex: &PExplorer) -> Result<String, String> {
    let node = ex.node();
    if node.kind != PKind::Flop || node.active.count_ones() != 2 {
        return Err("go to a heads-up flop terminal first (both players called)".into());
    }
    let cfg = &ex.solver.cfg;
    let mut seats: Vec<usize> = (0..cfg.players).filter(|&p| node.active >> p & 1 == 1).collect();
    seats.sort_by_key(|&p| cfg.postflop_rank(p));
    let (oop, ip) = (seats[0], seats[1]);
    let behind = cfg.stack - (node.invested[oop] as f64 - cfg.ante);
    Ok(format!(
        "holdem-solver postflop --board <FLOP> --pot {} --stack {} \\\n  --oop \"{}\" \\\n  --ip \"{}\"\n{}",
        fmt_chips(node.pot),
        fmt_chips(behind as f32),
        ex.range_of(oop).to_range_string(),
        ex.range_of(ip).to_range_string(),
        dim(&format!("(OOP = {}, IP = {})", ex.names()[oop], ex.names()[ip]))
    ))
}

/// Raise-first-in frequency of every seat that can open (combo-weighted).
pub fn rfi_summary(solver: &PreflopSolver) -> Vec<(&'static str, f64)> {
    let mut ex = PExplorer::new(solver);
    let names = solver.cfg.position_names();
    let mut out = Vec::new();
    for seat in 0..solver.cfg.players - 1 {
        if !ex.goto_first_in(seat) {
            continue;
        }
        let (_, overall) = ex.frequencies();
        let actions = ex.actions();
        let open: f64 = actions
            .iter()
            .zip(&overall)
            .filter(|(a, _)| !matches!(a, PAction::Fold))
            .map(|(_, &f)| f)
            .sum();
        out.push((names[seat], open));
    }
    out
}

const HELP: &str = "\
Commands:
  <n> | f x c r a        take an action by number or code
  u                      undo                     root    back to the start
  open <POS>             jump to POS's raise-first-in spot (e.g. 'open BTN')
  grid <n>               frequency grid of action n
  range [POS]            range of POS (default: player to act) with its range string
  spot                   at a heads-up flop terminal: print the postflop solver command
  help                   this help                q       quit";

pub fn run_repl(solver: &PreflopSolver) {
    let mut ex = PExplorer::new(solver);
    println!("{}", render(&ex));
    println!("{}", dim("type 'help' for commands"));
    let names = solver.cfg.position_names();
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
        let seat_arg = arg.and_then(|a| names.iter().position(|n| n.eq_ignore_ascii_case(a)));
        match cmd.to_ascii_lowercase().as_str() {
            "q" | "quit" | "exit" => break,
            "help" | "h" | "?" => println!("{HELP}"),
            "u" | "back" | "undo" => {
                if ex.frames.len() > 1 {
                    ex.frames.pop();
                }
                println!("{}", render(&ex));
            }
            "root" => {
                ex.frames.truncate(1);
                println!("{}", render(&ex));
            }
            "open" => match seat_arg {
                Some(seat) if ex.goto_first_in(seat) => println!("{}", render(&ex)),
                _ => println!("usage: open <{}>", names[..names.len() - 1].join("|")),
            },
            "grid" => match arg.and_then(|a| a.parse::<usize>().ok()) {
                Some(n) if n >= 1 => match render_action_grid(&ex, n - 1) {
                    Ok(s) => println!("{s}"),
                    Err(e) => println!("{e}"),
                },
                _ => println!("usage: grid <action number>"),
            },
            "range" => {
                let seat = seat_arg.or(match ex.node().kind {
                    PKind::Decision(q) => Some(q as usize),
                    _ => None,
                });
                match seat {
                    Some(s) => println!("{}", render_range(&ex, s)),
                    None => println!("usage: range <POS>"),
                }
            }
            "spot" => match spot_command(&ex) {
                Ok(s) => println!("{s}"),
                Err(e) => println!("{e}"),
            },
            _ => match ex.apply(cmd) {
                Ok(()) => println!("{}", render(&ex)),
                Err(e) => println!("{e}"),
            },
        }
    }
}

const MAGIC: &[u8; 8] = b"HSPRE001";

pub fn save(solver: &PreflopSolver, path: &str) -> Result<(), String> {
    let io = |e: std::io::Error| format!("{path}: {e}");
    let cfg = serde_json::to_vec(&solver.cfg).map_err(|e| e.to_string())?;
    let strat = solver.strategy_vec();
    let mut w = BufWriter::new(File::create(path).map_err(io)?);
    w.write_all(MAGIC).map_err(io)?;
    w.write_all(&(cfg.len() as u64).to_le_bytes()).map_err(io)?;
    w.write_all(&cfg).map_err(io)?;
    w.write_all(&solver.iterations.load(std::sync::atomic::Ordering::Relaxed).to_le_bytes()).map_err(io)?;
    w.write_all(&(strat.len() as u64).to_le_bytes()).map_err(io)?;
    let bytes: Vec<u8> = strat.iter().flat_map(|x| x.to_le_bytes()).collect();
    w.write_all(&bytes).map_err(io)?;
    w.flush().map_err(io)
}

pub fn load(path: &str) -> Result<PreflopSolver, String> {
    let io = |e: std::io::Error| format!("{path}: {e}");
    let mut r = BufReader::new(File::open(path).map_err(io)?);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic).map_err(io)?;
    if &magic != MAGIC {
        return Err(format!("{path} is not a preflop solution file"));
    }
    let mut b8 = [0u8; 8];
    r.read_exact(&mut b8).map_err(io)?;
    let mut cfg = vec![0u8; u64::from_le_bytes(b8) as usize];
    r.read_exact(&mut cfg).map_err(io)?;
    let cfg: PreflopConfig = serde_json::from_slice(&cfg).map_err(|e| e.to_string())?;
    r.read_exact(&mut b8).map_err(io)?;
    let iterations = u64::from_le_bytes(b8);
    r.read_exact(&mut b8).map_err(io)?;
    let mut bytes = vec![0u8; u64::from_le_bytes(b8) as usize * 4];
    r.read_exact(&mut bytes).map_err(io)?;
    let strat = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    PreflopSolver::with_strategy(cfg, strat, iterations)
}

/// Every decision node with its line and per-class strategy.
pub fn export_json(solver: &PreflopSolver) -> Value {
    let names = solver.cfg.position_names();
    let tree = &solver.tree;
    let mut nodes = Vec::new();
    let mut stack: Vec<(usize, Vec<String>)> = vec![(0, Vec::new())];
    while let Some((idx, line)) = stack.pop() {
        let node = &tree.nodes[idx];
        let PKind::Decision(q) = node.kind else { continue };
        let actions: Vec<String> = node.children().map(|c| tree.nodes[c].action.label()).collect();
        let mut strategy = serde_json::Map::new();
        for c in 0..NUM_CLASSES {
            let s: Vec<f64> = solver.average(node, c).iter().map(|&x| (x as f64 * 1000.0).round() / 1000.0).collect();
            strategy.insert(class_name(c), json!(s));
        }
        nodes.push(json!({
            "line": line,
            "player": names[q as usize],
            "pot": node.pot,
            "actions": actions,
            "strategy": strategy,
        }));
        for c in node.children().rev() {
            let mut l = line.clone();
            l.push(format!("{} {}", names[q as usize], tree.nodes[c].action.label()));
            stack.push((c, l));
        }
    }
    json!({ "config": solver.cfg, "iterations": solver.iterations.load(std::sync::atomic::Ordering::Relaxed), "nodes": nodes })
}
