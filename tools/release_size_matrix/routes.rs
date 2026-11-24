//! Synthetic and deterministic common market callback for every historical optimizer.
use super::{Decimal, Market, Replay, Sample, Scenario, d, native_params, passes, rank, search};
use serde_json::json;
use std::time::Instant;

#[derive(Clone)]
struct Band {
    price: Decimal,
    end_qty: Decimal,
    prior_value: Decimal,
}

#[derive(Clone)]
pub(super) struct Ladder(Vec<Band>);

impl Ladder {
    fn new(levels: impl IntoIterator<Item = (Decimal, Decimal)>) -> Self {
        let mut end_qty = Decimal::ZERO;
        let mut value = Decimal::ZERO;
        Self(
            levels
                .into_iter()
                .map(|(price, qty)| {
                    end_qty += qty;
                    let band = Band {
                        price,
                        end_qty,
                        prior_value: value,
                    };
                    value += qty * price;
                    band
                })
                .collect(),
        )
    }

    pub(super) fn price(&self, q: Decimal) -> Option<(Decimal, Decimal, Decimal)> {
        let i = self.0.partition_point(|b| b.end_qty < q);
        let band = self.0.get(i)?;
        let start = if i == 0 {
            Decimal::ZERO
        } else {
            self.0[i - 1].end_qty
        };
        Some((
            band.prior_value + (q - start) * band.price,
            q * band.price,
            Decimal::ZERO,
        ))
    }

    pub(super) fn cheapest(&self) -> Decimal {
        self.0.iter().map(|b| b.price).min().unwrap()
    }

    pub(super) fn breaks(&self) -> Vec<Decimal> {
        self.0.iter().map(|b| b.end_qty).collect()
    }
}

/// Reserves are denominated in the two adjacent assets on one directional pool.
/// An exact-out buy unwinds the hops backwards before it spends the quote.
#[derive(Clone)]
struct Hop {
    input: Decimal,
    output: Decimal,
    fee_keep: Decimal,
}
impl Hop {
    fn exact_in(&self, x: Decimal) -> Decimal {
        let x = x * self.fee_keep;
        self.output * x / (self.input + x)
    }

    fn exact_out(&self, y: Decimal) -> Option<Decimal> {
        (y < self.output).then(|| self.input * y / ((self.output - y) * self.fee_keep))
    }
}

/// Split legs use independent pools, and always start and end with the same assets.
/// Intermediate assets may differ across legs, so they are never added together.
#[derive(Clone)]
struct Route {
    legs: Vec<(Decimal, Vec<Hop>)>,
    gas: Decimal,
}

impl Route {
    fn price(&self, qty: Decimal, buy: bool) -> Option<Decimal> {
        let mut total = Decimal::ZERO;
        for (share, hops) in &self.legs {
            let mut value = qty * share;
            if buy {
                for hop in hops.iter().rev() {
                    value = hop.exact_out(value)?;
                }
            } else {
                for hop in hops {
                    value = hop.exact_in(value);
                }
            }
            total += value;
        }
        Some(total)
    }

    fn buy_lower_bound(&self) -> Decimal {
        self.legs
            .iter()
            .map(|(share, hops)| {
                *share * hops.iter().map(|h| h.input / h.output).product::<Decimal>()
            })
            .sum()
    }
}

#[derive(Clone)]
pub(super) struct Router(Vec<Route>);

impl Router {
    pub(super) fn cheapest_buy(&self) -> Decimal {
        self.0.iter().map(Route::buy_lower_bound).min().unwrap()
    }

    pub(super) fn price(
        &self,
        q: Decimal,
        buy: bool,
        budget: Decimal,
    ) -> Option<(Decimal, Decimal, Decimal)> {
        self.0
            .iter()
            .filter_map(|route| {
                let amount = route.price(q, buy)?;
                (!buy || amount <= budget).then_some((amount, route.gas))
            })
            .max_by_key(|&(amount, gas)| {
                if buy {
                    (-(amount + gas), -amount)
                } else {
                    (amount - gas, amount)
                }
            })
            .map(|(amount, gas)| (amount, amount, gas))
    }
}

struct Seed(u64);
impl Seed {
    fn choose(&mut self, n: u64) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x % n
    }

    fn units(&mut self, base: u64, width: u64, decimals: u32) -> Decimal {
        Decimal::new((base + self.choose(width)) as i64, decimals)
    }
}

fn hops(rng: &mut Seed, count: usize, total_rate: Decimal, reserve_scale: Decimal) -> Vec<Hop> {
    // Aligned midrates for a path, with individual pools at independent liquidity and fee tiers.
    let rates: Vec<_> = (1..count).map(|_| rng.units(960, 81, 3)).collect();
    let first = total_rate / rates.iter().copied().product::<Decimal>();
    std::iter::once(first)
        .chain(rates)
        .map(|rate| {
            let input = rng.units(180, 21_000, 0) * reserve_scale;
            Hop {
                input,
                output: input * rate,
                fee_keep: Decimal::ONE - rng.units(1, 51, 4),
            }
        })
        .collect()
}

fn router(
    rng: &mut Seed,
    count: usize,
    split: bool,
    spot: Decimal,
    buy: bool,
    scale: Decimal,
) -> Market {
    let gas_per_hop = rng.units(2, 25, 3);
    let rate = if buy { Decimal::ONE / spot } else { spot };
    let mut legs = Vec::with_capacity(2);
    let first_share = if split {
        rng.units(25, 51, 2)
    } else {
        Decimal::ONE
    };
    legs.push((first_share, hops(rng, count, rate, scale)));
    let mut gas = gas_per_hop * Decimal::from(count);
    if split {
        let second_count = if count == 2 { 3 } else { 2 };
        let second_rate = rate * rng.units(995, 11, 3);
        legs.push((
            Decimal::ONE - first_share,
            hops(rng, second_count, second_rate, scale),
        ));
        gas += gas_per_hop * Decimal::from(second_count);
    }
    let main = Route { legs, gas };
    let alternative_rate = rate * rng.units(970, 55, 3);
    let alternative = Route {
        legs: vec![(Decimal::ONE, hops(rng, 1, alternative_rate, scale))],
        gas: gas_per_hop,
    };
    let alternatives = if split || count > 1 {
        vec![main, alternative]
    } else {
        vec![main]
    };
    Market::Router(Router(alternatives))
}

fn ladder(rng: &mut Seed, spot: Decimal, buy: bool, dense: bool, budget: Decimal) -> Market {
    let count = if dense {
        115 + rng.choose(86)
    } else {
        5 + rng.choose(19)
    };
    let mut price = spot;
    let qty_scale = (budget / Decimal::from(count)).max(d("0.25"));
    let bins = (0..count).map(|i| {
        if i > 0 {
            let gap = if dense {
                rng.units(0, 13, 5)
            } else if i % 3 == 0 {
                rng.units(1, 80, 4)
            } else {
                rng.units(1, 18, 4)
            };
            price *= if buy {
                Decimal::ONE + gap
            } else {
                Decimal::ONE - gap
            };
        }
        let band_qty = qty_scale * rng.units(1, 25, 1);
        (price, band_qty)
    });
    Market::Ladder(Ladder::new(bins))
}

const TOPOLOGIES: [(&str, usize, bool, usize, bool); 8] = [
    ("cex-cex", 0, false, 0, false),
    ("cex-dex-direct", 0, false, 1, false),
    ("cex-dex-2hop", 0, false, 2, false),
    ("cex-dex-3hop", 0, false, 3, false),
    ("cex-dex-split", 0, false, 2, true),
    ("dex-2hop-cex", 2, false, 0, false),
    ("dex-split-cex", 3, true, 0, false),
    ("dex-split-dex-3hop", 2, true, 3, false),
];
const PER_TOPOLOGY: usize = 640;
const CAPS: [u32; 2] = [300, 5000];

fn scenario(topology: usize, trial: usize) -> (Scenario, (Decimal, Decimal), u64, bool) {
    let seed = 923_050u64 + (topology * PER_TOPOLOGY + trial) as u64 * 2_654_435_761;
    let mut rng = Seed(seed);
    let (label, buy_hops, buy_split, sell_hops, sell_split) = TOPOLOGIES[topology];
    if topology == 0 && trial == 0 {
        // 360 distinct boundaries on each side deliberately exceed 300 probes.
        let asks = Ladder::new((0..360).map(|i| (Decimal::ONE + Decimal::new(i, 6), d("3.25"))));
        let bids = Ladder::new((0..360).map(|i| (d("1.03") - Decimal::new(i, 6), d("3.5"))));
        return (
            Scenario {
                label,
                buy: Market::Ladder(asks),
                sell: Market::Ladder(bids),
                withdraw: d("0.25"),
                arrive_keep: Decimal::ONE,
                fixed: d("0.1"),
                fee_buy: d("0.001"),
                fee_sell: d("0.001"),
                min_sale: d("5"),
                max_sale: d("100000"),
                step: d("0.25"),
                narrow: false,
                budget: d("1000"),
            },
            (d("0.1"), d("0.3")),
            seed,
            true,
        );
    }
    let dense = trial % 11 == 0;
    let budget = if dense {
        rng.units(350, 651, 0)
    } else {
        [10, 25, 50, 120, 350, 850, 1000][rng.choose(7) as usize].into()
    };
    let spot = rng.units(650, 1151, 3);
    let sell_spot = spot * rng.units(986, 135, 3);
    let depth = rng.units(2, 30, 1);
    let buy = if buy_hops == 0 {
        ladder(&mut rng, spot, true, dense, budget)
    } else {
        router(&mut rng, buy_hops, buy_split, spot, true, depth)
    };
    let sell = if sell_hops == 0 {
        ladder(&mut rng, sell_spot, false, dense, budget)
    } else {
        router(&mut rng, sell_hops, sell_split, sell_spot, false, depth)
    };
    let step = if dense {
        d("0.25")
    } else {
        [d("0.5"), Decimal::ONE, d("2")][rng.choose(3) as usize]
    };
    let fee_buy = if buy_hops == 0 {
        rng.units(3, 20, 4)
    } else {
        Decimal::ZERO
    };
    let fee_sell = if sell_hops == 0 {
        rng.units(3, 20, 4)
    } else {
        Decimal::ZERO
    };
    let withdraw = if trial % 9 == 0 {
        rng.units(12, 110, 1)
    } else {
        rng.units(0, 12, 1)
    };
    let arrive_keep = if trial % 7 == 0 {
        rng.units(960, 35, 3)
    } else {
        Decimal::ONE
    };
    let fixed = rng.units(1, 41, 2);
    let min_sale = if sell_hops == 0 {
        rng.units(2, 15, 0)
    } else {
        Decimal::ZERO
    };
    let max_sale = if sell_hops == 0 && trial % 5 == 0 {
        budget * rng.units(30, 60, 2)
    } else {
        d("100000")
    };
    let floor = (rng.units(1, 30, 2), rng.units(1, 210, 2));
    (
        Scenario {
            label,
            buy,
            sell,
            withdraw,
            arrive_keep,
            fixed,
            fee_buy,
            fee_sell,
            min_sale,
            max_sale,
            step,
            narrow: false,
            budget,
        },
        floor,
        seed,
        dense,
    )
}

#[tokio::test]
async fn release_size_matrix_routes() {
    for topology in 0..TOPOLOGIES.len() {
        for trial in 0..PER_TOPOLOGY {
            let (s, floor, seed, dense) = scenario(topology, trial);
            // All fees are nonnegative. For each route, the pre-fee weighted spot is a
            // lower bound on the cost of each purchased coin, independent of native search.
            let guess = s.budget / s.buy.cheapest_buy();
            let steps = u32::try_from((guess / s.step).ceil()).unwrap() + 1;
            assert!(
                steps <= 10_000,
                "{seed}: fixture exceeded exact-oracle envelope"
            );
            let mut best = None;
            let mut feasible = 0;
            for n in 1..=steps {
                if let Ok(p) = s.point(s.step * Decimal::from(n)) {
                    feasible += 1;
                    if passes(&p, floor) && best.as_ref().is_none_or(|b| rank(&p) > rank(b)) {
                        best = Some(p);
                    }
                }
            }
            for max_evals in CAPS {
                let p = native_params(&s, guess, floor, max_evals);
                let mut replay = Replay {
                    scenario: &s,
                    evaluated: Vec::new(),
                };
                let start = Instant::now();
                let actual = search(&mut replay, &p).await;
                let elapsed = start.elapsed().as_micros();
                let found = actual.as_ref().ok();
                let picked = found.map(|x| &x.best);
                if let Some(p) = picked {
                    let verified = s.point(p.qty).unwrap();
                    assert!(p.invested <= s.budget && (p.qty % s.step).is_zero());
                    assert_eq!((p.profit, p.invested), (verified.profit, verified.invested));
                }
                let pass = picked.filter(|p| passes(p, floor));
                let sampled_pass = replay
                    .evaluated
                    .iter()
                    .filter_map(|(_, p)| p.as_ref().ok())
                    .any(|p| passes(p, floor));
                let cap_hit = replay.evaluated.len() >= max_evals as usize;
                if topology == 0 && trial == 0 && max_evals == 5000 {
                    assert!(
                        replay.evaluated.len() > 300,
                        "dense stress did not exercise the extra search budget"
                    );
                }
                let error = actual.as_ref().err().map(|e| format!("{e:?}"));
                let cap_stop = cap_hit
                    && error
                        .as_deref()
                        .is_some_and(|e| e.starts_with("Stop(") && e.contains("size.max_evals"));
                let class = match (best.as_ref(), pass) {
                    (None, None) => "no_feasible_profit",
                    (None, Some(_)) => panic!("{seed}: search beat a supposedly exhaustive oracle"),
                    (Some(_), None) if cap_stop => "hard_evaluation_cap_stop",
                    (Some(_), None) if sampled_pass => "passing_point_evaluated_not_selected",
                    (Some(_), None) if cap_hit => "evaluation_cap_before_passing_point",
                    (Some(_), None) if picked.is_none() => "search_error_with_oracle_opportunity",
                    (Some(_), None) => "passing_point_not_evaluated",
                    (Some(o), Some(p)) if o.profit > p.profit => "suboptimal_profit",
                    (Some(o), Some(p)) if o.invested < p.invested => "equal_profit_more_capital",
                    _ => "matched",
                };
                let pnl = |p: Option<&Sample>| p.map_or(Decimal::ZERO, |x| x.profit);
                println!(
                    "SIZE_MATRIX {}",
                    json!({
                        "scenario": format!("routes-{}-{trial:04}", s.label),
                        "topology": s.label, "scenario_seed": seed, "dense_book": dense,
                        "model": "synthetic_shared_curve",
                        "budget_usdt": s.budget, "threshold": "seeded", "eval_limit": max_evals,
                        "reason": class, "min_usdt": floor.0, "min_pct": floor.1,
                        "oracle_points": steps, "oracle_feasible_points": feasible,
                        "oracle_pass": best.is_some(), "search_pass": pass.is_some(),
                        "oracle_qty": best.as_ref().map(|x| x.qty),
                        "oracle_profit_usdt": pnl(best.as_ref()), "search_profit_usdt": pnl(pass),
                        "model_gap_usdt": pnl(best.as_ref()) - pnl(pass),
                        "selected_qty": picked.map(|p| p.qty),
                        "selected_invested_usdt": picked.map(|p| p.invested),
                        "selected_raw_profit_usdt": picked.map(|p| p.profit),
                        "sampled_passing_point": sampled_pass, "evaluated": replay.evaluated.len(),
                        "cap_hit": cap_hit, "hard_cap_stop": cap_stop,
                        "used_more_than_300": replay.evaluated.len() > 300,
                        "search_max_qty": found.map(|f| f.choice.max_qty), "search_us": elapsed,
                        "native_error": error,
                        "threshold_placement": super::THRESHOLD_PLACEMENT
                    })
                );
            }
        }
    }
}
