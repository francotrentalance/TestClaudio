#![allow(clippy::needless_range_loop)]
//! Validates the multiplayer MCCFR against an independent exact solver on the
//! heads-up push/fold game, where the answer is a well-defined Nash equilibrium.

use holdem_solver::card::{Rng, card_bit, combo_mask};
use holdem_solver::eval::eval_mask;
use holdem_solver::preflop::{PKind, PreflopConfig, PreflopSolver};
use holdem_solver::range::{NUM_CLASSES, class_num_combos, combo_class};

/// Class-vs-class equity and the number of non-conflicting combo pairs.
fn class_matrix(boards_per_pair: usize) -> (Vec<f64>, Vec<f64>) {
    let mut eq = vec![0.0f64; NUM_CLASSES * NUM_CLASSES];
    let mut w = vec![0.0f64; NUM_CLASSES * NUM_CLASSES];
    let mut rng = Rng::new(42);
    for h1 in 0..1326 {
        for h2 in 0..1326 {
            if h1 == h2 || combo_mask(h1) & combo_mask(h2) != 0 {
                continue;
            }
            let dead = combo_mask(h1) | combo_mask(h2);
            let mut won = 0.0;
            for _ in 0..boards_per_pair {
                let mut b = 0u64;
                let mut n = 0;
                while n < 5 {
                    let c = rng.below(52) as u8;
                    let bit = card_bit(c);
                    if (b | dead) & bit == 0 {
                        b |= bit;
                        n += 1;
                    }
                }
                let (x, y) = (eval_mask(b | combo_mask(h1)), eval_mask(b | combo_mask(h2)));
                won += if x > y { 1.0 } else if x == y { 0.5 } else { 0.0 };
            }
            let k = combo_class(h1) * NUM_CLASSES + combo_class(h2);
            eq[k] += won / boards_per_pair as f64;
            w[k] += 1.0;
        }
    }
    for k in 0..eq.len() {
        if w[k] > 0.0 {
            eq[k] /= w[k];
        }
    }
    (eq, w)
}

/// CFR+ on the class-level push/fold game. Returns (SB push, BB call) per class.
fn exact_pushfold(stack: f64, eq: &[f64], w: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = NUM_CLASSES;
    let (mut rs, mut rb) = (vec![[0.0f64; 2]; n], vec![[0.0f64; 2]; n]);
    let (mut ss, mut sb) = (vec![[0.0f64; 2]; n], vec![[0.0f64; 2]; n]);
    let strat = |r: &[f64; 2]| {
        let (a, b) = (r[0].max(0.0), r[1].max(0.0));
        if a + b > 0.0 { [a / (a + b), b / (a + b)] } else { [0.5, 0.5] }
    };
    for t in 1..=4000 {
        let x: Vec<[f64; 2]> = rs.iter().map(strat).collect();
        let y: Vec<[f64; 2]> = rb.iter().map(strat).collect();
        // SB: action 1 = push. Values per class, weighted by matchup counts.
        for a in 0..n {
            let (mut v_push, mut tot) = (0.0, 0.0);
            for b in 0..n {
                let k = a * n + b;
                if w[k] == 0.0 {
                    continue;
                }
                let call = y[b][1];
                v_push += w[k] * ((1.0 - call) * 1.0 + call * (2.0 * stack * eq[k] - stack));
                tot += w[k];
            }
            let v_push = v_push / tot;
            let v_fold = -0.5;
            let v = x[a][0] * v_fold + x[a][1] * v_push;
            rs[a][0] = (rs[a][0] + v_fold - v).max(0.0);
            rs[a][1] = (rs[a][1] + v_push - v).max(0.0);
            ss[a][0] += t as f64 * x[a][0];
            ss[a][1] += t as f64 * x[a][1];
        }
        for b in 0..n {
            let (mut v_call, mut v_fold, mut tot) = (0.0, 0.0, 0.0);
            for a in 0..n {
                let k = a * n + b;
                if w[k] == 0.0 {
                    continue;
                }
                let reach = w[k] * x[a][1];
                v_call += reach * ((1.0 - eq[k]) * 2.0 * stack - stack);
                v_fold -= reach;
                tot += w[k];
            }
            let (v_call, v_fold) = (v_call / tot, v_fold / tot);
            let v = y[b][0] * v_fold + y[b][1] * v_call;
            rb[b][0] = (rb[b][0] + v_fold - v).max(0.0);
            rb[b][1] = (rb[b][1] + v_call - v).max(0.0);
            sb[b][0] += t as f64 * y[b][0];
            sb[b][1] += t as f64 * y[b][1];
        }
    }
    let norm = |s: &[[f64; 2]]| s.iter().map(|v| v[1] / (v[0] + v[1])).collect::<Vec<f64>>();
    (norm(&ss), norm(&sb))
}

fn combo_weighted(freq: &[f64]) -> f64 {
    let total: f64 = (0..NUM_CLASSES).map(|c| class_num_combos(c) as f64 * freq[c]).sum();
    total / 1326.0
}

#[test]
fn heads_up_pushfold_matches_exact_solution() {
    let stack = 10.0;
    let (eq, w) = class_matrix(48);
    let (push, call) = exact_pushfold(stack, &eq, &w);
    let (push_pct, call_pct) = (combo_weighted(&push), combo_weighted(&call));
    // Sanity: the known 10bb heads-up Nash ranges are roughly 58% / 37%.
    assert!((0.52..0.64).contains(&push_pct), "exact push {push_pct}");
    assert!((0.32..0.42).contains(&call_pct), "exact call {call_pct}");

    let cfg = PreflopConfig { players: 2, stack, pushfold: true, equity_boards: 4, ..Default::default() };
    let solver = PreflopSolver::new(cfg).unwrap();
    let mut k = 1.0f32;
    for _ in 0..20 {
        solver.run(200_000, k as u64);
        solver.discount(k / (k + 1.0));
        k += 1.0;
    }
    solver.run(4_000_000, 999);

    let root = &solver.tree.nodes[0];
    let jam = &solver.tree.nodes[root.first_child as usize + 1];
    assert_eq!(jam.kind, PKind::Decision(1));
    let m_push: Vec<f64> = (0..NUM_CLASSES).map(|c| solver.average(root, c)[1] as f64).collect();
    let m_call: Vec<f64> = (0..NUM_CLASSES).map(|c| solver.average(jam, c)[1] as f64).collect();
    let (mp, mc) = (combo_weighted(&m_push), combo_weighted(&m_call));
    println!("exact push {push_pct:.4} call {call_pct:.4} | mccfr push {mp:.4} call {mc:.4}");
    assert!((mp - push_pct).abs() < 0.015, "push {mp} vs exact {push_pct}");
    assert!((mc - call_pct).abs() < 0.015, "call {mc} vs exact {call_pct}");
    // Hand by hand, combo-weighted mean absolute difference.
    let diff = |a: &[f64], b: &[f64]| {
        (0..NUM_CLASSES).map(|c| class_num_combos(c) as f64 * (a[c] - b[c]).abs()).sum::<f64>() / 1326.0
    };
    assert!(diff(&m_push, &push) < 0.03, "push per-hand diff {}", diff(&m_push, &push));
    assert!(diff(&m_call, &call) < 0.03, "call per-hand diff {}", diff(&m_call, &call));
}
