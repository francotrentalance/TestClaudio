use holdem_solver::postflop::{PostflopConfig, PostflopGame, tree_stats};
use std::time::Instant;
fn main() {
    let threads: usize = std::env::args().nth(1).map(|s| s.parse().unwrap()).unwrap_or(4);
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().unwrap();
    let cfg = PostflopConfig { board: "Ks7d2c9h".into(), pot: 12.0, stack: 91.5, ..Default::default() };
    let st = tree_stats(&cfg).unwrap();
    println!("{st:?} solve MB {:.0}", st.solve_bytes() as f64 / 1e6);
    let mut g = PostflopGame::new(cfg).unwrap();
    let t = Instant::now();
    let n: usize = std::env::var("N").ok().and_then(|s| s.parse().ok()).unwrap_or(100);
    for _ in 0..n { g.iterate(); }
    let el = t.elapsed();
    let te = Instant::now();
    let e = g.exploitability_pct();
    println!("threads {threads}: {:.1} ms/iter, expl {e:.3}% (calc {:?})", el.as_secs_f64() * 1000.0 / n as f64, te.elapsed());
}
