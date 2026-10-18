//! `preflop` and `preflop-view` subcommands.

use std::time::Instant;

use clap::Args;

use crate::display::{bold, dim};
use crate::preflop::config::PreflopConfig;
use crate::preflop::explore::{self, render, rfi_summary, run_repl, PExplorer};
use crate::preflop::mccfr::PreflopSolver;

#[derive(Args)]
pub struct PreflopArgs {
    /// JSON config file (see `--print-config`). Flags override it.
    #[arg(long)]
    pub config: Option<String>,
    /// Number of players (2-6).
    #[arg(long)]
    pub players: Option<usize>,
    /// Effective stack in big blinds.
    #[arg(long)]
    pub stack: Option<f64>,
    #[arg(long)]
    pub ante: Option<f64>,
    /// Push/fold game (first in can only fold or jam).
    #[arg(long)]
    pub pushfold: bool,
    /// Open size (raise-to, bb) for UTG..BTN.
    #[arg(long)]
    pub open: Option<f64>,
    #[arg(long)]
    pub sb_open: Option<f64>,
    #[arg(long)]
    pub threebet_ip: Option<f64>,
    #[arg(long)]
    pub threebet_oop: Option<f64>,
    #[arg(long)]
    pub fourbet: Option<f64>,
    /// Allow open-limping.
    #[arg(long)]
    pub limp: bool,
    /// Allow the SB to complete.
    #[arg(long)]
    pub sb_limp: bool,
    /// Allow cold-calling 3-bets.
    #[arg(long)]
    pub cold_call_3bet: bool,
    #[arg(long)]
    pub max_cold_callers: Option<u32>,
    /// Postflop realization of the first player to act (last to act = 1.0).
    #[arg(long)]
    pub realization_oop: Option<f64>,
    #[arg(long)]
    pub realization_exponent: Option<f64>,
    #[arg(long)]
    pub rake: Option<f64>,
    #[arg(long)]
    pub rake_cap: Option<f64>,
    /// Total MCCFR iterations.
    #[arg(long, short = 'n')]
    pub iterations: Option<u64>,
    /// Random boards per iteration for showdown equity.
    #[arg(long)]
    pub boards: Option<usize>,
    #[arg(long)]
    pub seed: Option<u64>,
    /// Save the solution (open with `preflop-view`).
    #[arg(long)]
    pub save: Option<String>,
    /// Export all decision nodes as JSON.
    #[arg(long)]
    pub json: Option<String>,
    /// Open the interactive explorer after solving.
    #[arg(long)]
    pub explore: bool,
    /// Print the effective config as JSON and exit.
    #[arg(long)]
    pub print_config: bool,
}

fn build_config(a: &PreflopArgs) -> Result<PreflopConfig, String> {
    let mut cfg: PreflopConfig = match &a.config {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?
        }
        None => PreflopConfig::default(),
    };
    macro_rules! set {
        ($($field:ident <- $val:expr),* $(,)?) => {
            $( if let Some(v) = $val { cfg.$field = v; } )*
        };
    }
    set!(
        players <- a.players,
        stack <- a.stack,
        ante <- a.ante,
        open <- a.open,
        sb_open <- a.sb_open,
        threebet_ip <- a.threebet_ip,
        threebet_oop <- a.threebet_oop,
        fourbet <- a.fourbet,
        max_cold_callers <- a.max_cold_callers,
        realization_oop <- a.realization_oop,
        realization_exponent <- a.realization_exponent,
        rake <- a.rake,
        rake_cap <- a.rake_cap,
        iterations <- a.iterations,
        equity_boards <- a.boards,
        seed <- a.seed,
    );
    cfg.pushfold |= a.pushfold;
    cfg.limp |= a.limp;
    cfg.sb_limp |= a.sb_limp;
    cfg.cold_call_3bet |= a.cold_call_3bet;
    cfg.validate()?;
    Ok(cfg)
}

/// Runs the configured number of iterations with linear-CFR discounting over
/// the first 40%, calling `progress` after each batch.
pub fn solve(solver: &PreflopSolver, mut progress: impl FnMut(&PreflopSolver, u64)) {
    let total = solver.cfg.iterations;
    let batch = solver.cfg.discount_interval.max(10_000);
    let discount_until = total * 2 / 5;
    let mut done = 0;
    let mut k = 1.0f32;
    while done < total {
        let n = batch.min(total - done);
        solver.run(n, solver.cfg.seed.wrapping_add(done));
        done += n;
        if done <= discount_until {
            solver.discount(k / (k + 1.0));
            k += 1.0;
        }
        progress(solver, done);
    }
}

pub fn run(a: PreflopArgs) -> Result<(), String> {
    let cfg = build_config(&a)?;
    if a.print_config {
        println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
        return Ok(());
    }
    let solver = PreflopSolver::new(cfg)?;
    let c = &solver.cfg;
    println!(
        "{} {}-handed, {}bb, blinds {}/{}{}{}",
        bold("Game:"),
        c.players,
        c.stack,
        c.sb,
        c.bb,
        if c.ante > 0.0 { format!(", ante {}", c.ante) } else { String::new() },
        if c.pushfold { ", push/fold" } else { "" }
    );
    println!(
        "{} {} nodes, {} decisions, {} iterations",
        bold("Tree:"),
        solver.tree.nodes.len(),
        solver.tree.decisions,
        c.iterations
    );
    if !c.pushfold {
        println!(
            "{}",
            dim(&format!(
                "Pots that see a flop are valued by realized equity (OOP factor {}, exponent {}); \
                 all-in pots are exact.",
                c.realization_oop, c.realization_exponent
            ))
        );
    }
    let start = Instant::now();
    let mut last_print = Instant::now();
    let total = c.iterations;
    solve(&solver, |s, done| {
        if last_print.elapsed().as_secs_f64() >= 5.0 || done == total {
            last_print = Instant::now();
            let el = start.elapsed().as_secs_f64();
            let rfi: Vec<String> =
                rfi_summary(s).iter().map(|(n, f)| format!("{n} {:.1}%", 100.0 * f)).collect();
            println!(
                "{:>6.1}% {:>12} it  {:>6.0}k it/s  open: {}",
                100.0 * done as f64 / total as f64,
                done,
                done as f64 / el / 1000.0,
                rfi.join("  ")
            );
        }
    });
    println!("{} {:.1}s\n", bold("Done in"), start.elapsed().as_secs_f64());
    print_charts(&solver);
    if let Some(path) = &a.json {
        let v = explore::export_json(&solver);
        std::fs::write(path, serde_json::to_string(&v).unwrap()).map_err(|e| format!("{path}: {e}"))?;
        println!("wrote {path}");
    }
    if let Some(path) = &a.save {
        explore::save(&solver, path)?;
        println!("saved {path} (open with: holdem-solver preflop-view {path})");
    }
    if a.explore {
        run_repl(&solver);
    }
    Ok(())
}

/// Prints the raise-first-in chart of every position.
pub fn print_charts(solver: &PreflopSolver) {
    let mut ex = PExplorer::new(solver);
    for seat in 0..solver.cfg.players - 1 {
        if ex.goto_first_in(seat) {
            println!("{}", render(&ex));
        }
    }
}

pub fn view(file: &str) -> Result<(), String> {
    let solver = explore::load(file)?;
    println!(
        "{} {}-handed {}bb ({} iterations)",
        bold("Loaded:"),
        solver.cfg.players,
        solver.cfg.stack,
        solver.iterations.load(std::sync::atomic::Ordering::Relaxed)
    );
    run_repl(&solver);
    Ok(())
}
