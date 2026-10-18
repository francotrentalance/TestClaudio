//! Benchmark: iterations of a small flop spot. `cargo run --release --example bench_flop [threads] [iters]`
use holdem_solver::postflop::{tree_stats, PostflopConfig, PostflopGame, StreetSizes};
use std::time::Instant;
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
fn main() {
    let mut args = std::env::args().skip(1);
    let compress = std::env::var("COMPRESS").is_ok();
    let threads: usize = args.next().map(|s| s.parse().unwrap()).unwrap_or(4);
    let iters: u32 = args.next().map(|s| s.parse().unwrap()).unwrap_or(10);
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().unwrap();
    let cfg = PostflopConfig {
        board: "Qs8d4c".into(),
        oop_range: "22+,A2s+,K9s+,Q9s+,J9s+,T8s+,98s,87s,76s,ATo+,KJo+,QJo".into(),
        ip_range: "22+,A2s+,K5s+,Q8s+,J8s+,T7s+,97s+,86s+,75s+,65s,54s,A9o+,KTo+,QTo+,JTo".into(),
        pot: 6.0,
        stack: 97.0,
        flop: StreetSizes::symmetric("33%", "3x"),
        turn: StreetSizes::symmetric("75%", "3x"),
        river: StreetSizes::symmetric("75%", "3x"),
        max_raises: 1,
        compress,
        ..Default::default()
    };
    let st = tree_stats(&cfg).unwrap();
    println!("{} nodes, hands {:?}, {:.2} GB", st.nodes, st.hands, st.solve_bytes() as f64 / 1e9);
    let mut g = PostflopGame::new(cfg).unwrap();
    g.iterate(); // warm up (page faults)
    let t = Instant::now();
    for _ in 0..iters {
        g.iterate();
    }
    println!("threads {threads}: {:.0} ms/iter", t.elapsed().as_secs_f64() * 1000.0 / iters as f64);
}
