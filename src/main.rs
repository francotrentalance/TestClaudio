use std::time::Instant;

// The solver allocates many short-lived per-node vectors; mimalloc handles that
// pattern noticeably faster than the system allocator.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use clap::{Args, Parser, Subcommand};

use holdem_solver::card::{cards_to_string, parse_cards};
use holdem_solver::display::{self, bold, dim};
use holdem_solver::equity::range_vs_range;
use holdem_solver::eval::{describe, eval_cards};
use holdem_solver::postflop::explore::{render_node, run_repl, Explorer};
use holdem_solver::postflop::{io as pio, tree_stats, PostflopConfig, PostflopGame};
use holdem_solver::preflop;
use holdem_solver::range::{class_name, Range};

#[derive(Parser)]
#[command(name = "holdem-solver", version, about = "GTO solver for 6-max No-Limit Hold'em")]
struct Cli {
    /// Disable colored output.
    #[arg(long, global = true)]
    no_color: bool,
    /// Worker threads (default: all cores).
    #[arg(long, global = true)]
    threads: Option<usize>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Solve a heads-up postflop spot (exact, no card abstraction).
    Postflop(PostflopArgs),
    /// Explore a saved postflop solution.
    View { file: String },
    /// Solve 6-max (2-6 players) preflop with MCCFR.
    Preflop(preflop::cli::PreflopArgs),
    /// Explore a saved preflop solution.
    PreflopView { file: String },
    /// Range vs range equity.
    Equity {
        range1: String,
        range2: String,
        #[arg(long, default_value = "")]
        board: String,
        /// Random boards for preflop/flop-less spots.
        #[arg(long, default_value_t = 100_000)]
        samples: u64,
    },
    /// Evaluate a 5-7 card hand.
    Eval { cards: String },
    /// Show a range on the 13x13 grid.
    Range {
        range: String,
        #[arg(long, default_value = "")]
        board: String,
    },
}

#[derive(Args)]
struct PostflopArgs {
    /// JSON config file (see `--print-config` for the format). Flags override it.
    #[arg(long)]
    config: Option<String>,
    #[arg(long)]
    board: Option<String>,
    /// OOP range, e.g. "22+,A2s+,KTo+".
    #[arg(long)]
    oop: Option<String>,
    /// IP range.
    #[arg(long)]
    ip: Option<String>,
    /// Starting pot.
    #[arg(long)]
    pot: Option<f64>,
    /// Effective stack behind.
    #[arg(long)]
    stack: Option<f64>,
    /// Bet sizes for every street and both players, e.g. "33%,75%".
    #[arg(long)]
    bet: Option<String>,
    /// Raise sizes for every street and both players, e.g. "3x" or "60%".
    #[arg(long)]
    raise: Option<String>,
    #[arg(long)]
    flop_bet: Option<String>,
    #[arg(long)]
    flop_raise: Option<String>,
    #[arg(long)]
    turn_bet: Option<String>,
    #[arg(long)]
    turn_raise: Option<String>,
    #[arg(long)]
    river_bet: Option<String>,
    #[arg(long)]
    river_raise: Option<String>,
    /// Raises allowed after the first bet on a street.
    #[arg(long)]
    max_raises: Option<u32>,
    /// Bets committing at least this fraction of the stack become all-in.
    #[arg(long)]
    allin_threshold: Option<f64>,
    /// Rake fraction (e.g. 0.05) and cap.
    #[arg(long)]
    rake: Option<f64>,
    #[arg(long)]
    rake_cap: Option<f64>,
    /// Maximum iterations.
    #[arg(long, short = 'n')]
    iterations: Option<u32>,
    /// Target exploitability in % of the pot.
    #[arg(long)]
    target: Option<f64>,
    /// Compute exploitability every N iterations.
    #[arg(long, default_value_t = 10)]
    check_every: u32,
    /// Disable suit isomorphism.
    #[arg(long)]
    no_iso: bool,
    /// Store regrets and strategies in 16 bits: half the memory.
    #[arg(long)]
    compress: bool,
    /// Save the solution to a file (open it later with `view`).
    #[arg(long)]
    save: Option<String>,
    /// Write the starting street's strategy as JSON.
    #[arg(long)]
    json: Option<String>,
    /// Open the interactive explorer after solving.
    #[arg(long)]
    explore: bool,
    /// Print the tree size and memory estimate, then exit.
    #[arg(long)]
    dry_run: bool,
    /// Print the effective config as JSON, then exit.
    #[arg(long)]
    print_config: bool,
    /// Solve even if the estimate exceeds available memory.
    #[arg(long)]
    force: bool,
}

fn main() {
    let cli = Cli::parse();
    display::init_color(cli.no_color);
    if let Some(t) = cli.threads {
        rayon::ThreadPoolBuilder::new().num_threads(t).build_global().ok();
    }
    let result = match cli.command {
        Command::Postflop(a) => postflop(a),
        Command::View { file } => view(&file),
        Command::Preflop(a) => preflop::cli::run(a),
        Command::PreflopView { file } => preflop::cli::view(&file),
        Command::Equity { range1, range2, board, samples } => equity(&range1, &range2, &board, samples),
        Command::Eval { cards } => eval(&cards),
        Command::Range { range, board } => show_range(&range, &board),
    };
    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn build_config(a: &PostflopArgs) -> Result<PostflopConfig, String> {
    let mut cfg = match &a.config {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?
        }
        None => PostflopConfig::default(),
    };
    macro_rules! set {
        ($field:expr, $val:expr) => {
            if let Some(v) = $val.clone() {
                $field = v;
            }
        };
    }
    set!(cfg.board, a.board);
    set!(cfg.oop_range, a.oop);
    set!(cfg.ip_range, a.ip);
    set!(cfg.pot, a.pot);
    set!(cfg.stack, a.stack);
    set!(cfg.max_raises, a.max_raises);
    set!(cfg.allin_threshold, a.allin_threshold);
    set!(cfg.rake, a.rake);
    set!(cfg.rake_cap, a.rake_cap);
    set!(cfg.iterations, a.iterations);
    set!(cfg.target_exploitability, a.target);
    let streets = [
        (&mut cfg.flop, &a.flop_bet, &a.flop_raise),
        (&mut cfg.turn, &a.turn_bet, &a.turn_raise),
        (&mut cfg.river, &a.river_bet, &a.river_raise),
    ];
    for (st, bet, raise) in streets {
        if let Some(b) = bet.as_ref().or(a.bet.as_ref()) {
            st.oop_bet = b.clone();
            st.ip_bet = b.clone();
        }
        if let Some(r) = raise.as_ref().or(a.raise.as_ref()) {
            st.oop_raise = r.clone();
            st.ip_raise = r.clone();
        }
    }
    if a.no_iso {
        cfg.isomorphism = false;
    }
    cfg.compress |= a.compress;
    Ok(cfg)
}

fn available_memory() -> Option<u64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let line = info.lines().find(|l| l.starts_with("MemAvailable:"))?;
    let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kb * 1024)
}

fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1e9)
}

fn postflop(a: PostflopArgs) -> Result<(), String> {
    let cfg = build_config(&a)?;
    if a.print_config {
        println!("{}", serde_json::to_string_pretty(&cfg).unwrap());
        return Ok(());
    }
    let stats = tree_stats(&cfg)?;
    let board = parse_cards(&cfg.board)?;
    println!(
        "{} {}  pot {}  stack {}  OOP {} hands, IP {} hands",
        bold("Spot:"),
        cards_to_string(&board),
        cfg.pot,
        cfg.stack,
        stats.hands[0],
        stats.hands[1]
    );
    println!(
        "{} {} nodes ({} decisions, {} chance), memory {}",
        bold("Tree:"),
        stats.nodes,
        stats.player_nodes,
        stats.chance_nodes,
        gb(stats.solve_bytes())
    );
    if a.dry_run {
        return Ok(());
    }
    let avail = available_memory().unwrap_or(u64::MAX);
    if stats.solve_bytes() > avail / 10 * 9 && !a.force {
        return Err(format!(
            "this tree needs {} but only {} is available; use --compress, fewer bet sizes, \
             a lower --max-raises, narrower ranges, or --force",
            gb(stats.solve_bytes()),
            gb(avail)
        ));
    }

    let start = Instant::now();
    let mut game = PostflopGame::new(cfg)?;
    let target = game.config.target_exploitability;
    let max_it = game.config.iterations;
    let mut expl = f64::NAN;
    for it in 1..=max_it {
        game.iterate();
        if it % a.check_every.max(1) == 0 || it == max_it {
            expl = game.exploitability_pct();
            println!(
                "iter {:>5}  exploitability {:>7.3}% of pot  {}",
                it,
                expl,
                dim(&format!("{:.1}s", start.elapsed().as_secs_f64()))
            );
            if expl <= target {
                break;
            }
        }
    }
    let ev = game.expected_values();
    println!(
        "{} {} iterations, exploitability {:.3}% of pot, {:.1}s",
        bold("Solved:"),
        game.iteration,
        expl,
        start.elapsed().as_secs_f64()
    );
    println!(
        "{} OOP {:.3}  IP {:.3}  (chips won from the {} pot)\n",
        bold("EV:"),
        ev[0],
        ev[1],
        game.config.pot
    );
    game.finalize();
    println!("{}", render_node(&Explorer::new(&game)));

    if let Some(path) = &a.json {
        let v = pio::export_json(&game, Some(expl));
        std::fs::write(path, serde_json::to_string_pretty(&v).unwrap())
            .map_err(|e| format!("{path}: {e}"))?;
        println!("wrote {path}");
    }
    if let Some(path) = &a.save {
        pio::save(&game, path)?;
        println!("saved {path} (open with: holdem-solver view {path})");
    }
    if a.explore {
        run_repl(&game);
    }
    Ok(())
}

fn view(file: &str) -> Result<(), String> {
    let game = pio::load(file)?;
    println!(
        "{} {}  pot {}  stack {}  ({} iterations)",
        bold("Loaded:"),
        game.config.board,
        game.config.pot,
        game.config.stack,
        game.iteration
    );
    run_repl(&game);
    Ok(())
}

fn equity(r1: &str, r2: &str, board: &str, samples: u64) -> Result<(), String> {
    let (a, b) = (Range::parse(r1)?, Range::parse(r2)?);
    let board = parse_cards(board)?;
    if board.len() > 5 || board.len() == 1 || board.len() == 2 {
        return Err("board must have 0, 3, 4 or 5 cards".into());
    }
    let res = range_vs_range(&a, &b, &board, samples, 1);
    let how = if res.exact { "exact".to_string() } else { format!("{} sampled boards", res.boards) };
    println!("Range 1: {:.2}%   Range 2: {:.2}%   ({how})", 100.0 * res.equity[0], 100.0 * res.equity[1]);
    Ok(())
}

fn eval(cards: &str) -> Result<(), String> {
    let cards = parse_cards(cards)?;
    if !(5..=7).contains(&cards.len()) {
        return Err("give 5 to 7 cards".into());
    }
    let v = eval_cards(&cards);
    println!("{}: {} (value {v:#x})", cards_to_string(&cards), describe(v));
    Ok(())
}

fn show_range(range: &str, board: &str) -> Result<(), String> {
    let r = Range::parse(range)?;
    let board = parse_cards(board)?;
    let weights = r.class_weights();
    println!(
        "{:.1} combos ({:.1}% of hands){}",
        r.num_combos(),
        100.0 * r.num_combos() / 1326.0,
        if board.is_empty() {
            String::new()
        } else {
            format!(
                ", {:.1} after card removal on {}",
                r.num_combos_excluding(&board),
                cards_to_string(&board)
            )
        }
    );
    print!(
        "{}",
        display::grid(|c| {
            let w = weights[c];
            if w <= 0.0 {
                if display::color_enabled() {
                    dim(&format!("{:<5}", class_name(c)))
                } else {
                    "  .  ".to_string()
                }
            } else if w >= 0.999 {
                display::paint(&format!("{:<5}", class_name(c)), display::ActionKind::Passive)
            } else {
                format!(
                    "{:<3}{:>2}",
                    &class_name(c)[..class_name(c).len().min(3)],
                    (w * 100.0).round().min(99.0) as i64
                )
            }
        })
    );
    Ok(())
}
