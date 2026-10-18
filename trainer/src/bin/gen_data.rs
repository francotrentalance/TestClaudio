//! Generates the content shipped with the app.
//!
//!   gen-data cash     <iterations> <out.bin>
//!   gen-data pushfold <iterations> <out.bin>
//!   gen-data flops    <kind> <out.bin> [board ...]   (resumable: skips boards already in out.bin)

use std::time::Instant;

use holdem_trainer::gen::{self, FLOPS};
use holdem_trainer::model::{FlopLibrary, PreflopPack, decode, encode};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub const PUSHFOLD_STACKS: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 17.0, 20.0];

fn write(path: &str, bytes: &[u8]) {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, bytes).expect("write");
    std::fs::rename(&tmp, path).expect("rename");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = Instant::now();
    match args.first().map(String::as_str) {
        Some("cash") => {
            let iterations: u64 = args[1].parse().expect("iterations");
            let pack = gen::preflop_pack("6-max cash, 100bb", gen::cash_config(iterations), |done, total| {
                if done % 10_000_000 < 200_000 {
                    eprintln!("cash: {done}/{total} ({:.0}s)", start.elapsed().as_secs_f64());
                }
            })
            .expect("solve");
            write(&args[2], &encode(&pack));
            eprintln!("cash: wrote {} ({} bytes)", args[2], pack.strategy.len());
        }
        Some("pushfold") => {
            let iterations: u64 = args[1].parse().expect("iterations");
            let mut packs: Vec<PreflopPack> = Vec::new();
            for &stack in PUSHFOLD_STACKS {
                let pack = gen::preflop_pack(&format!("Push/fold {stack}bb"), gen::pushfold_config(stack, iterations), |_, _| {})
                    .expect("solve");
                eprintln!("pushfold: {stack}bb done ({:.0}s)", start.elapsed().as_secs_f64());
                packs.push(pack);
            }
            write(&args[2], &encode(&packs));
        }
        Some("flops") => {
            let kind = &args[1];
            let out = &args[2];
            let boards: Vec<String> =
                if args.len() > 3 { args[3..].to_vec() } else { FLOPS.iter().map(|s| s.to_string()).collect() };
            let mut lib: FlopLibrary =
                std::fs::read(out).ok().and_then(|b| decode(&b).ok()).unwrap_or_default();
            for board in boards {
                let cards = holdem_solver::card::parse_cards(&board).expect("board");
                if lib.spots.iter().any(|s| &s.kind == kind && s.board == cards) {
                    eprintln!("{kind} {board}: already done");
                    continue;
                }
                let t = Instant::now();
                let mut last = String::new();
                let spot = gen::flop_spot(kind, &board, |m| last = m.to_string()).expect("solve");
                eprintln!(
                    "{kind} {board}: {last}; {} nodes, {} lines, {:.0}s",
                    spot.nodes.len(),
                    spot.lines.len(),
                    t.elapsed().as_secs_f64()
                );
                lib.spots.push(spot);
                write(out, &encode(&lib));
            }
        }
        _ => {
            eprintln!("usage: gen-data cash|pushfold <iterations> <out> | flops <kind> <out> [boards]");
            std::process::exit(2);
        }
    }
}
