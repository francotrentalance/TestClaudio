//! Generates the content shipped with the app.
//!
//!   gen-data eqr      <ranges> <out.json> [flops]      measure equity realization
//!   gen-data cash     <iterations> <out.bin> [eqr.json]
//!   gen-data pushfold <iterations> <out.bin>
//!   gen-data flops    <kind> <out.bin> <ranges> [board ...]   (resumable: skips boards already in out.bin)
//!   gen-data ranges   <cash.bin>                       print the spot ranges a cash pack implies
//!
//! `<ranges>` is `illustrative` or a cash pack, whose solution gives the
//! ranges each postflop spot is reached with.

use std::time::Instant;

use holdem_solver::preflop::EqrModel;
use holdem_solver::range::{NUM_CLASSES, class_name};
use holdem_trainer::gen::{self, FLOPS, SRP, SpotRanges, THREE_BET};
use holdem_trainer::model::{FlopLibrary, LoadedPreflop, PreflopPack, decode, encode};

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub const PUSHFOLD_STACKS: &[f64] = &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0, 14.0, 15.0, 17.0, 20.0];

fn write(path: &str, bytes: &[u8]) {
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, bytes).expect("write");
    std::fs::rename(&tmp, path).expect("rename");
}

fn spot_ranges(arg: &str) -> SpotRanges {
    if arg == "illustrative" {
        return SpotRanges::illustrative().expect("ranges");
    }
    let bytes = std::fs::read(arg).unwrap_or_else(|e| panic!("{arg}: {e}"));
    let pack = LoadedPreflop::new(decode::<PreflopPack>(&bytes).expect("cash pack")).expect("cash pack");
    SpotRanges::from_cash(&pack).expect("spot ranges")
}

fn print_ranges(r: &SpotRanges) {
    for (name, [oop, ip]) in [("SRP", &r.srp), ("3BP", &r.three_bet)] {
        eprintln!("{name} OOP ({:.0} combos): {}", oop.num_combos(), oop.to_range_string());
        eprintln!("{name} IP  ({:.0} combos): {}", ip.num_combos(), ip.to_range_string());
    }
}

/// Prints the main preflop ranges of a cash pack, to sanity-check a solve.
fn report(pack: &LoadedPreflop) {
    use holdem_solver::preflop::PAction;
    use holdem_solver::range::Range;
    let tree = &pack.tree;
    let names = pack.pack.config.position_names();
    let child = |node: usize, f: &dyn Fn(PAction) -> bool| tree.nodes[node].children().find(|&c| f(tree.nodes[c].action));
    let fold = |a: PAction| a == PAction::Fold;
    let show = |label: String, node: usize| {
        let n = &tree.nodes[node];
        for (i, c) in n.children().enumerate() {
            let w: Vec<f32> = (0..NUM_CLASSES).map(|k| pack.strategy(n, k)[i]).map(|x| if x < 0.03 { 0.0 } else { x }).collect();
            let r = Range::from_class_weights(&w);
            if tree.nodes[c].action != PAction::Fold {
                eprintln!("{label} {:?} {:.1}%: {}", tree.nodes[c].action, 100.0 * r.num_combos() / 1326.0, r.to_range_string());
            }
        }
    };
    let mut node = 0;
    for seat in 0..5 {
        show(format!("RFI {}", names[seat]), node);
        node = child(node, &fold).unwrap();
    }
    // BB vs BTN open, SB vs BTN open, SB vs CO open, BTN vs CO open.
    let open = |n: usize| child(n, &|a| matches!(a, PAction::Raise(_))).unwrap();
    let path = |seats_fold: usize| (0..seats_fold).fold(0, |n, _| child(n, &fold).unwrap());
    let btn_open = open(path(3));
    show("SB vs BTN".into(), btn_open);
    show("BB vs BTN".into(), child(btn_open, &fold).unwrap());
    let co_open = open(path(2));
    show("BTN vs CO".into(), co_open);
    let sb = child(co_open, &fold).unwrap();
    show("SB vs CO".into(), sb);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = Instant::now();
    match args.first().map(String::as_str) {
        Some("eqr") => {
            let ranges = spot_ranges(&args[1]);
            print_ranges(&ranges);
            let n: usize = args.get(3).map(|s| s.parse().expect("flops")).unwrap_or(30);
            let flops = gen::sample_flops(n, 2026);
            let mut sums = Vec::new();
            for kind in [SRP, THREE_BET] {
                let mut s = gen::EqrSums::default();
                for (i, board) in flops.iter().enumerate() {
                    let t = Instant::now();
                    let mut last = String::new();
                    let expl = gen::measure_eqr(kind, board, &ranges, &mut s, |m| last = m.to_string()).expect("solve");
                    eprintln!("eqr {kind} {}/{n} {board}: {expl:.2}% ({:.0}s)", i + 1, t.elapsed().as_secs_f64());
                }
                sums.push(s);
            }
            let model = gen::eqr_model(&sums[0], &sums[1], gen::spot_spr(SRP).unwrap(), gen::spot_spr(THREE_BET).unwrap());
            model.validate().expect("model");
            std::fs::write(&args[2], serde_json::to_string_pretty(&model).unwrap()).expect("write");
            let measured = [gen::eqr_measured(&sums[0], 0), gen::eqr_measured(&sums[0], 1)];
            eprintln!("class   deep OOP  deep IP  shallow OOP  shallow IP   (* = filled in, not measured)");
            for c in 0..NUM_CLASSES {
                let mark = |m: &Option<f32>| if m.is_some() { ' ' } else { '*' };
                eprintln!(
                    "{:5} {:8.3}{} {:8.3}{} {:10.3} {:11.3}",
                    class_name(c),
                    model.deep_oop[c],
                    mark(&measured[0][c]),
                    model.deep_ip[c],
                    mark(&measured[1][c]),
                    model.shallow_oop[c],
                    model.shallow_ip[c]
                );
            }
            eprintln!("eqr: wrote {} ({:.0}s)", args[2], start.elapsed().as_secs_f64());
        }
        Some("ranges") => {
            print_ranges(&spot_ranges(&args[1]));
            let bytes = std::fs::read(&args[1]).expect("read");
            let pack = LoadedPreflop::new(decode::<PreflopPack>(&bytes).expect("cash pack")).expect("cash pack");
            report(&pack);
        }
        Some("cash") => {
            let iterations: u64 = args[1].parse().expect("iterations");
            let eqr: Option<EqrModel> = args.get(3).map(|path| {
                let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
                serde_json::from_str(&text).expect("eqr model")
            });
            let pack = gen::preflop_pack("6-max cash, 100bb", gen::cash_config(iterations, eqr), |done, total| {
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
            let ranges = spot_ranges(&args[3]);
            print_ranges(&ranges);
            let boards: Vec<String> =
                if args.len() > 4 { args[4..].to_vec() } else { FLOPS.iter().map(|s| s.to_string()).collect() };
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
                let spot = gen::flop_spot_with(kind, &board, &ranges, |m| last = m.to_string()).expect("solve");
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
            eprintln!(
                "usage: gen-data eqr <ranges> <out.json> [flops] | cash <iterations> <out> [eqr.json] | \
                 pushfold <iterations> <out> | flops <kind> <out> <ranges> [boards] | ranges <cash.bin>"
            );
            std::process::exit(2);
        }
    }
}
