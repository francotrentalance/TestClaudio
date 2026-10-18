# holdem-solver

A game-theory-optimal (GTO) solver for **6-max No-Limit Hold'em**, written in Rust.

It has two engines, the same split commercial tools use:

| Engine | What it solves | Algorithm | Exact? |
|---|---|---|---|
| `postflop` | One heads-up postflop spot: two ranges, a flop/turn/river, pot, stacks, bet sizes | Discounted CFR over the full card tree, vectorized over hands | **Yes** for the given betting tree. No card abstraction. Exploitability is measured exactly. |
| `preflop` | The whole 2–6 player preflop game: opens, iso-raises, 3-bets, 4-bets, jams, optional limps and cold calls | External-sampling Monte Carlo CFR (Pluribus-style), multithreaded | All-in and fold outcomes are exact. **Pots that see a flop use an equity-realization model**, which is an approximation (see [Limitations](#limitations)). Push/fold mode is fully exact. |

You go from one to the other: solve preflop, walk to a flop terminal (e.g. *BTN opens, BB calls*), and `spot` prints the postflop command for that spot with both ranges filled in.

> Nobody solves full 6-max NLHE exactly; it is far too large. Pluribus, PioSolver, GTO Wizard and MonkerSolver all rely on abstraction or subgame solving. This project follows the same approach and states plainly where it approximates.

---

## Build

Requires a recent Rust toolchain (edition 2024, Rust ≥ 1.85).

```sh
cargo build --release
./target/release/holdem-solver --help
```

`.cargo/config.toml` builds with `-C target-cpu=native` (AVX2/AVX-512 where available). Delete that file if you need a binary that runs on other machines.

Run the tests (all in release mode; about 15 s):

```sh
cargo test --release
```

---

## Quick start

### 1. 6-max preflop, 100bb

```sh
holdem-solver preflop --save sixmax.bin            # 20M iterations, ~35 s on 4 cores
holdem-solver preflop-view sixmax.bin              # explore it
```

Prints raise-first-in charts for every position, e.g. (13×13 grid, `R` = raise, `F` = fold, number = frequency %):

```
UTG to act (1326 combos, 100.0% of hands)
  [1]  F  Fold            83.6%
  [2]  R  Raise 2.5       16.4%
```

Useful options: `--players 2..6`, `--stack 40`, `--ante 0.1`, `--open 2.3`, `--sb-limp`, `--limp`, `--cold-call-3bet`, `--rake 0.05 --rake-cap 3`, `-n <iterations>`, `--config configs/preflop_6max_100bb.json`.

### 2. Push/fold (short stacks, exact)

```sh
holdem-solver preflop --pushfold --stack 10 --ante 0.1
```

### 3. From a preflop line to a postflop solve

In `preflop-view`:

```
> open BTN        # jump to BTN's raise-first-in spot
> r               # BTN raises
> f               # SB folds
> c               # BB calls  -> flop terminal
> spot
holdem-solver postflop --board <FLOP> --pot 5.5 --stack 97.5 \
  --oop "<BB calling range>" \
  --ip "<BTN opening range>"
```

### 4. Postflop

```sh
# Defaults: BTN vs BB single-raised pot on Ks7d2c with illustrative ranges.
holdem-solver postflop --explore

holdem-solver postflop --board Qs8d4c --pot 6 --stack 97 \
  --oop "22+,A2s+,K9s+,Q9s+,J9s+,T8s+,98s,87s,76s,ATo+,KJo+,QJo" \
  --ip  "22+,A2s+,K5s+,Q8s+,J8s+,T7s+,97s+,86s+,75s+,65s,54s,A9o+,KTo+,QTo+,JTo" \
  --flop-bet "33%,75%" --turn-bet 75% --river-bet "75%,a" --raise 3x \
  --target 0.3 --save qs8d4c.bin --json qs8d4c.json

holdem-solver view qs8d4c.bin
holdem-solver postflop --config configs/postflop_bb_vs_btn_3bet_pot.json --explore
```

Check memory before a big solve: `--dry-run` prints the tree size and memory needed; `--compress` halves it.

### 5. Utilities

```sh
holdem-solver equity "AA,KK" "AKs,QQ" --board Ts7d2c   # exact on flops, sampled preflop
holdem-solver range "22+,A2s+,KTo+"                     # 13x13 grid + combo count
holdem-solver eval AsKsQsJsTs9d2c                       # straight flush
```

---

## Postflop solver

### Bet sizes

Sizes are comma-separated lists:

| Syntax | Meaning |
|---|---|
| `33%` | Bet a third of the pot. For raises: raise *on top of the call* by 33% of the pot after calling (PioSolver convention). |
| `3x` | Raise to 3× the bet faced (raises only). |
| `a` | All-in. |

`--bet`/`--raise` set every street for both players. `--flop-bet`, `--turn-raise`, ... set one street. A config file can set each player separately (`oop_bet`, `ip_raise`, …) and `oop_donk` (sizes OOP may lead with when IP was the last aggressor; `""` disables donk bets).

Other tree controls (flags or config fields):

* `max_raises` (default 1): raises allowed after the first bet on a street.
* `allin_threshold` (0.67): a bet or raise committing at least this share of the remaining stack becomes all-in.
* `add_allin_spr` (1.5): also offer all-in when the stack behind is at most 1.5 pots.
* `rake`, `rake_cap`: rake taken from every pot (no-flop-no-drop doesn't apply postflop).
* `isomorphism` (on): suit-symmetric turn/river cards are solved once and mirrored. Results are identical, with less time and memory.
* `compress` (off): 16-bit regrets and strategies. Half the memory, about 13% slower, exploitability essentially unchanged (see tests).

A full config is written by `holdem-solver postflop --print-config`; see `configs/`.

### Explorer commands (`--explore`, `view`)

```
<n> | x c f a b<amt> r<amt>  take an action (number or token, e.g. b3.3)
<card>                       deal the turn/river card, e.g. Kh
u / root                     undo / back to the root
grid <n>                     frequency grid of action n
hands [filter]               per-hand strategy, EV and equity ('hands AK', 'hands AsKd')
ev [oop|ip], eq [oop|ip]     EV / equity grid
range [oop|ip]               reach-weighted range at this node
```

EV is in chips: the expected share of the pot won minus chips still to be invested, so at the root OOP EV + IP EV = pot (without rake).

### Output

* `--save file.bin`: the full solution (config + average strategy of every node). Open it with `view`.
* `--json file.json`: every decision node of the starting street with per-combo strategy, EV and equity.

### How it works

* **Discounted CFR** (Brown & Sandholm 2019; α = 1.5, β = 0, γ = 2) with alternating updates. Every node update processes all of a player's hands at once (vectors of ~1,000 combos).
* **Exact card removal** everywhere. Showdowns are evaluated in O(n) per node with one sorted sweep per direction and per-card blocker corrections, not O(n²).
* **Chance nodes** enumerate every turn and river card. Work is parallelized across turn cards with rayon.
* **Exploitability** = the average gain of a best response for each player against the other's average strategy, reported as % of the starting pot. 0.3–0.5% is a typical "solved" target.

---

## Preflop solver

### The game

Seats act in order (UTG, HJ, CO, BTN, SB, BB at 6-max). Equal stacks. Configurable:

* opens (`open`, `sb_open`), iso-raises over limpers (`limp_raise_add`)
* 3-bet multiple in position / out of position (`threebet_ip` 3.0, `threebet_oop` 4.0), squeeze sizing (`caller_add`)
* 4-bet multiple (`fourbet` 2.3); `max_raises` sized raises (default 3: open, 3-bet, 4-bet), after which only all-in
* `allin_threshold`, `jam_ratio` (when a jam is offered next to a sized raise)
* `limp`, `sb_limp`, `cold_call_3bet`, `max_cold_callers` (limits multiway trees; the BB can always call)
* `ante`, `rake`, `rake_cap` (rake only on pots that see a flop)

The default 6-max 100bb tree has 8,732 nodes; with all limps enabled it has 76k.

### The algorithm

External-sampling MCCFR. Each iteration deals real hole cards to all seats (so card removal between players is exact), draws `equity_boards` random boards shared by every showdown in that iteration, and traverses once per seat. Regrets live in shared atomic arrays updated lock-free by all threads (as in Pluribus); linear-CFR weighting is applied by periodic discounting over the first 40% of the run. Information sets are (decision node × 169 starting-hand classes).

### The postflop model (the approximation)

A pot that ends preflop with stacks behind is valued without playing postflop:

```
share_i  ∝  realization_i × equity_i ^ exponent
```

* `equity_i` is the all-in equity of the players' actual cards (Monte Carlo).
* `realization` grows linearly from `realization_oop` (default 0.75) for the first player to act postflop to 1.0 for the last.
* `realization_exponent` (default 1.4) lets strong hands over-realize and weak hands under-realize.

The defaults were *chosen* so that opening frequencies land near those published by full solvers (100bb, no rake: UTG ≈ 16%, HJ ≈ 20%, CO ≈ 27%, BTN ≈ 41%, SB ≈ 40%). They are not derived. The results are sensitive to them: BTN's opening range moves from ~35% to ~52% across plausible values. All-in and fold outcomes don't depend on the model, so push/fold solutions are exact.

### Explorer commands (`--explore`, `preflop-view`)

```
<n> | f x c r a     take an action
open <POS>          jump to POS's raise-first-in spot
grid <n>            frequency grid of action n
range [POS]         range of POS at this node, with a range string for the postflop solver
spot                at a heads-up flop terminal: print the postflop command
u / root            undo / back to the start
```

`--json` exports every decision node with per-class strategies.

---

## Validation

All checks are in the test suite (`cargo test --release`):

* **Evaluator**: exhaustive over all 2,598,960 five-card hands (exact category counts and the 7,462 distinct ranks) and all 133,784,560 seven-card hands (exact category counts).
* **Postflop vs closed-form theory**: a polarized river (nuts + air vs a bluff-catcher). With a pot-sized bet the solver bluffs 1 combo per 2 value combos and the bluff-catcher calls 50%. With a half-pot bet it is 1:3 and 67%, exactly as the indifference equations require.
* **Showdown/fold values** match an O(n²) brute force on every terminal, with and without rake.
* **Isomorphism** gives the same first iterations, EVs and exploitability as the unreduced tree.
* **Compressed storage** reaches the same exploitability (0.069% vs 0.052% of pot after 300 iterations on a turn spot) and EVs.
* **Save/load** round-trips strategies exactly.
* **Preflop MCCFR vs an independent exact solver**: heads-up 10bb push/fold solved separately by CFR+ on a 169×169 equity matrix. Exact: SB pushes 58.1%, BB calls 37.3% (the known Nash values). MCCFR: 58.3% / 37.4%, with per-hand differences under 3%.

## Performance (4-core Xeon VM, measured)

| Spot | Tree | Memory | Time |
|---|---|---|---|
| Turn, BTN vs BB ranges (~530 hands each), default sizes | 4.6k nodes | 18 MB | to 0.3% pot in ~1 s |
| Flop, ~250–325 hands per player, 1 size per street | 0.94M nodes | 2.0 GB (1.0 GB compressed) | ~0.75 s/iteration |
| Flop, default BTN vs BB (~550 hands each), Ks7d2c | 0.89M nodes | 3.65 GB (1.85 GB compressed) | to 0.49% pot in 271 s (160 iterations, ~1.7 s each) |
| Preflop 6-max 100bb | 8.7k nodes | 12 MB | 20M iterations in ~35 s (≈ 580k it/s) |

Solving time scales roughly with tree memory. On the default flop, BB checks 96% to the BTN and the BTN's EV is 3.34 of the 5.5bb pot (61%), in line with published solver results for K72 rainbow.

## Limitations

* **Preflop non-all-in pots are approximated** by the realization model above. It ignores playability (suited connectors under-realize relative to real solvers, and BB 3-bets small pairs more than full solvers do). Treat preflop charts outside push/fold as a strong baseline, not ground truth.
* **Postflop is heads-up only.** Multiway flops (3+ players) aren't solved postflop.
* **Fixed betting trees.** As with every solver, the result is optimal only for the sizes you allow.
* Memory grows with (bet sizes × raises × range sizes). Use `--dry-run`, `--compress`, fewer sizes or narrower ranges for big flop trees.
* Average-strategy convergence in multiplayer MCCFR has no Nash guarantee (it does in the 2-player zero-sum postflop solver); the push/fold test shows it converges in practice.

## Layout

```
src/card.rs         cards, combos, masks, RNG
src/eval.rs         5-7 card evaluator
src/range.rs        range parsing, 169 classes
src/equity.rs       range vs range equity
src/postflop/       config, tree builder, game setup, DCFR solver, storage, explorer, save/export
src/preflop/        config, tree builder, MCCFR solver, explorer, save/export, CLI
tests/              solver correctness tests
configs/            example configs
examples/           benchmarks
```
