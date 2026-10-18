use holdem_solver::card::{combo_index, parse_cards};
use holdem_solver::postflop::{Action, NodeKind, PostflopConfig, PostflopGame, StreetSizes};

fn river_toy(bet: &str) -> PostflopConfig {
    PostflopConfig {
        board: "Kh9d5c3s2h".into(),
        // OOP holds a pure bluff-catcher: beats QJ, loses to the set.
        oop_range: "AA".into(),
        // IP is polarized: 3 combos of top set and 16 of queen-high air.
        ip_range: "KK,QJ".into(),
        pot: 10.0,
        stack: 10.0,
        river: StreetSizes {
            oop_bet: "".into(),
            oop_raise: "".into(),
            ip_bet: bet.into(),
            ip_raise: "".into(),
            oop_donk: None,
        },
        add_allin_spr: 0.0,
        iterations: 2000,
        ..PostflopConfig::default()
    }
}

/// Polarized river: with a pot-sized bet, IP bluffs 1 combo per 2 value combos
/// and OOP calls exactly half the time (MDF).
#[test]
fn river_polar_vs_bluffcatcher_matches_theory() {
    let mut g = PostflopGame::new(river_toy("100%")).unwrap();
    for _ in 0..3000 {
        g.iterate();
    }
    let expl = g.exploitability_pct();
    assert!(expl < 0.1, "exploitability {expl}% of pot");

    let root = &g.nodes[0];
    assert_eq!(root.kind, NodeKind::Player(0));
    assert_eq!(root.num_children, 1, "OOP can only check");
    let ip = &g.nodes[root.first_child as usize];
    assert_eq!(ip.kind, NodeKind::Player(1));
    let acts: Vec<Action> = ip.children().map(|c| g.nodes[c].action).collect();
    assert_eq!(acts, vec![Action::Check, Action::AllIn(10.0)]);

    let s = g.average_strategy(ip);
    let nh = g.num_hands(1);
    let kk = g.hands[1]
        .iter()
        .position(|&(a, b)| holdem_solver::card::rank(a) == 11 && holdem_solver::card::rank(b) == 11)
        .unwrap();
    assert!(s[nh + kk] > 0.99, "sets always bet, got {}", s[nh + kk]);
    let bluff_combos: f32 = g.hands[1]
        .iter()
        .enumerate()
        .filter(|(_, (a, _))| holdem_solver::card::rank(*a) != 11)
        .map(|(h, _)| s[nh + h])
        .sum();
    assert!((bluff_combos - 1.5).abs() < 0.08, "expected 1.5 bluff combos, got {bluff_combos}");

    let bet_node = &g.nodes[ip.first_child as usize + 1];
    let s = g.average_strategy(bet_node);
    let n0 = g.num_hands(0);
    let call: f32 = (0..n0).map(|h| s[n0 + h]).sum::<f32>() / n0 as f32;
    assert!((call - 0.5).abs() < 0.03, "expected 50% calls, got {call}");

    let ev = g.expected_values();
    assert!((ev[0] + ev[1] - 10.0).abs() < 1e-3, "EVs must sum to the pot: {ev:?}");
}

/// Half-pot bet: bluffs make up b/(1+2b) = 1/4 of the betting range, MDF = 2/3.
#[test]
fn river_half_pot_frequencies() {
    let mut g = PostflopGame::new(river_toy("50%")).unwrap();
    for _ in 0..3000 {
        g.iterate();
    }
    assert!(g.exploitability_pct() < 0.1);
    let ip = &g.nodes[g.nodes[0].first_child as usize];
    let s = g.average_strategy(ip);
    let nh = g.num_hands(1);
    let bluff_combos: f32 = g.hands[1]
        .iter()
        .enumerate()
        .filter(|(_, (a, _))| holdem_solver::card::rank(*a) != 11)
        .map(|(h, _)| s[nh + h])
        .sum();
    assert!((bluff_combos - 1.0).abs() < 0.06, "expected 1 bluff combo, got {bluff_combos}");
    let bet_node = &g.nodes[ip.first_child as usize + 1];
    let s = g.average_strategy(bet_node);
    let n0 = g.num_hands(0);
    let call: f32 = (0..n0).map(|h| s[n0 + h]).sum::<f32>() / n0 as f32;
    assert!((call - 2.0 / 3.0).abs() < 0.03, "expected 67% calls, got {call}");
}

#[test]
fn exploitability_decreases_on_turn() {
    let cfg = PostflopConfig {
        board: "Ks7d2c9h".into(),
        oop_range: "22+,A2s+,K9s+,QTs+,JTs,T9s,98s,ATo+,KJo+".into(),
        ip_range: "22+,A2s+,K5s+,Q8s+,J8s+,T8s+,97s+,86s+,75s+,65s,54s,A8o+,KTo+,QTo+,JTo".into(),
        pot: 20.0,
        stack: 80.0,
        ..PostflopConfig::default()
    };
    let mut g = PostflopGame::new(cfg).unwrap();
    let mut last = f64::INFINITY;
    for round in 0..4 {
        for _ in 0..50 {
            g.iterate();
        }
        let e = g.exploitability_pct();
        assert!(e >= -1e-3, "exploitability cannot be negative: {e}");
        if round > 0 {
            assert!(e < last * 1.05, "exploitability went up: {last} -> {e}");
        }
        last = e;
    }
    assert!(last < 1.0, "turn spot should be under 1% pot after 200 iterations, got {last}");
    let ev = g.expected_values();
    assert!((ev[0] + ev[1] - 20.0).abs() < 1e-2, "{ev:?}");
}

#[test]
fn isomorphism_matches_full_tree() {
    // Two-tone turn: clubs and hearts are interchangeable for the river card.
    let base = PostflopConfig {
        board: "Ks7s2d5d".into(),
        oop_range: "77+,A9s+,KTs+,QJs,AJo+".into(),
        ip_range: "55+,A5s+,K9s+,QTs+,JTs,ATo+,KQo".into(),
        pot: 10.0,
        stack: 40.0,
        ..PostflopConfig::default()
    };
    let iso = PostflopConfig { isomorphism: true, ..base.clone() };
    let full = PostflopConfig { isomorphism: false, ..base };
    let mut a = PostflopGame::new(iso).unwrap();
    let mut b = PostflopGame::new(full).unwrap();
    assert!(a.stats().nodes < b.stats().nodes);
    // The first iterations are the same computation up to float rounding. (Later
    // iterates drift apart because regret matching is discontinuous where
    // regrets tie, and equilibria are not unique per hand, so compare values.)
    for _ in 0..2 {
        a.iterate();
        b.iterate();
    }
    let sa = a.current_strategy(&a.nodes[0]);
    let sb = b.current_strategy(&b.nodes[0]);
    let max_diff = sa.iter().zip(&sb).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    assert!(max_diff < 1e-5, "root strategy differs by {max_diff}");
    for _ in 0..298 {
        a.iterate();
        b.iterate();
    }
    let (ea, eb) = (a.expected_values(), b.expected_values());
    assert!((ea[0] - eb[0]).abs() < 0.01 * 10.0, "{ea:?} vs {eb:?}");
    let (xa, xb) = (a.exploitability_pct(), b.exploitability_pct());
    assert!(xa < 0.5 && xb < 0.5, "{xa} vs {xb}");
}

#[test]
fn card_removal_in_ranges() {
    let cfg = PostflopConfig {
        board: "AsKs7d2c3h".into(),
        oop_range: "AA,KK".into(),
        ip_range: "AK".into(),
        ..PostflopConfig::default()
    };
    let g = PostflopGame::new(cfg).unwrap();
    // AA and KK lose the board card: 3 + 3 combos; AK: 3*3 = 9.
    assert_eq!(g.num_hands(0), 6);
    assert_eq!(g.num_hands(1), 9);
    let b = parse_cards("AhAd").unwrap();
    assert_ne!(g.hand_index[0][combo_index(b[0], b[1])], u32::MAX);
}

#[test]
fn compressed_storage_converges_like_full() {
    let base = PostflopConfig {
        board: "Ks7d2c9h".into(),
        oop_range: "22+,A2s+,K9s+,QTs+,JTs,T9s,98s,ATo+,KJo+".into(),
        ip_range: "22+,A2s+,K5s+,Q8s+,J8s+,T8s+,97s+,86s+,75s+,65s,54s,A8o+,KTo+,QTo+,JTo".into(),
        pot: 20.0,
        stack: 80.0,
        ..PostflopConfig::default()
    };
    let mut full = PostflopGame::new(base.clone()).unwrap();
    let mut comp = PostflopGame::new(PostflopConfig { compress: true, ..base }).unwrap();
    assert_eq!(
        comp.stats().solve_bytes() * 2 - full.stats().solve_bytes(),
        comp.stats().nodes as u64 * std::mem::size_of::<holdem_solver::postflop::Node>() as u64
    );
    for _ in 0..300 {
        full.iterate();
        comp.iterate();
    }
    let (ef, ec) = (full.exploitability_pct(), comp.exploitability_pct());
    println!("exploitability full {ef:.4}% compressed {ec:.4}%");
    assert!(ec < ef + 0.15, "compressed {ec}% vs full {ef}%");
    let (vf, vc) = (full.expected_values(), comp.expected_values());
    assert!((vf[0] - vc[0]).abs() < 0.05, "{vf:?} vs {vc:?}");
}

#[test]
fn save_and_load_roundtrip() {
    let cfg = PostflopConfig {
        board: "Kh9d5c3s2h".into(),
        oop_range: "AA,KK,QQ,AK".into(),
        ip_range: "KK,QJ,AQ".into(),
        compress: true,
        ..PostflopConfig::default()
    };
    let mut g = PostflopGame::new(cfg).unwrap();
    for _ in 0..50 {
        g.iterate();
    }
    let path = std::env::temp_dir().join(format!("hs_roundtrip_{}.bin", std::process::id()));
    let path = path.to_str().unwrap();
    holdem_solver::postflop::io::save(&g, path).unwrap();
    let loaded = holdem_solver::postflop::io::load(path).unwrap();
    std::fs::remove_file(path).ok();
    assert_eq!(loaded.iteration, 50);
    for (a, b) in g.nodes.iter().zip(&loaded.nodes) {
        if let NodeKind::Player(_) = a.kind {
            let (sa, sb) = (g.average_strategy(a), loaded.average_strategy(b));
            assert_eq!(sa, sb);
        }
    }
    let (ea, eb) = (g.expected_values(), loaded.expected_values());
    assert!((ea[0] - eb[0]).abs() < 1e-4);
}

#[test]
fn explorer_equity_matches_equity_module() {
    use holdem_solver::equity::range_vs_range;
    use holdem_solver::postflop::explore::Explorer;
    use holdem_solver::range::Range;
    let cfg = PostflopConfig {
        board: "Jh8h3c".into(),
        oop_range: "TT+,AJs+,KQs,QJs,JTs,T9s,98s,AQo+".into(),
        ip_range: "22+,A2s+,KTs+,QTs+,J9s+,T8s+,97s+,87s,76s,ATo+,KJo+".into(),
        ..PostflopConfig::default()
    };
    let g = PostflopGame::with_strategy(
        cfg.clone(),
        vec![0.0; holdem_solver::postflop::tree_stats(&cfg).unwrap().storage_floats as usize],
    )
    .unwrap();
    let ex = Explorer::new(&g);
    let eq = ex.equity(0);
    let board = parse_cards(&cfg.board).unwrap();
    let res = range_vs_range(
        &Range::parse(&cfg.oop_range).unwrap(),
        &Range::parse(&cfg.ip_range).unwrap(),
        &board,
        0,
        0,
    );
    assert!(res.exact);
    for &(combo, e, _) in &res.hands[0] {
        let h = g.hand_index[0][combo] as usize;
        assert!((eq[h] as f64 - e).abs() < 1e-4, "combo {combo}: {} vs {e}", eq[h]);
    }
}
