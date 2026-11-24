use arb::core::size::{Curve, Fail, Params, Point};
use arb::core::size_search::search;
use async_trait::async_trait;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::str::FromStr;
use std::time::Instant;

include!("size_matrix_adapter.inc");
#[path = "size_matrix_routes.inc"]
mod routes;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

type Level = (Decimal, Decimal);
type Sample = Point<(Decimal, Decimal)>;

#[derive(Clone)]
enum Market {
    Book(Vec<Level>),
    Pool { coin: Decimal, usdt: Decimal },
    Ladder(routes::Ladder),
    Router(routes::Router),
}

impl Market {
    fn price(
        &self,
        qty: Decimal,
        buy: bool,
        budget: Decimal,
    ) -> Option<(Decimal, Decimal, Decimal)> {
        match self {
            Self::Book(levels) => {
                let mut open = qty;
                let mut amount = Decimal::ZERO;
                for &(price, depth) in levels {
                    let fill = open.min(depth);
                    amount += fill * price;
                    open -= fill;
                    if open.is_zero() {
                        return Some((amount, qty * price, Decimal::ZERO));
                    }
                }
                None
            }
            Self::Pool { coin, usdt } => {
                let fee = d("0.9975");
                if buy {
                    (qty < *coin).then(|| {
                        let amount = *usdt * qty / ((*coin - qty) * fee);
                        (amount, amount, Decimal::ZERO)
                    })
                } else {
                    let amount = *usdt * qty * fee / (*coin + qty * fee);
                    Some((amount, amount, Decimal::ZERO))
                }
            }
            Self::Ladder(book) => book.price(qty),
            Self::Router(router) => router.price(qty, buy, budget),
        }
    }

    fn cheapest_buy(&self) -> Decimal {
        match self {
            Self::Book(levels) => levels.iter().map(|x| x.0).min().unwrap(),
            Self::Pool { coin, usdt } => *usdt / *coin,
            Self::Ladder(book) => book.cheapest(),
            Self::Router(router) => router.cheapest_buy(),
        }
    }

    fn breaks(&self) -> Vec<Decimal> {
        if let Self::Book(levels) = self {
            return levels
                .iter()
                .scan(Decimal::ZERO, |total, &(_, qty)| {
                    *total += qty;
                    Some(*total)
                })
                .collect();
        }
        if let Self::Ladder(book) = self {
            return book.breaks();
        }
        Vec::new()
    }
}

#[derive(Clone)]
struct Scenario {
    label: &'static str,
    buy: Market,
    sell: Market,
    withdraw: Decimal,
    arrive_keep: Decimal,
    fixed: Decimal,
    fee_buy: Decimal,
    fee_sell: Decimal,
    min_sale: Decimal,
    max_sale: Decimal,
    step: Decimal,
    narrow: bool,
    budget: Decimal,
}

impl Scenario {
    fn point(&self, qty: Decimal) -> Result<Sample, Fail> {
        let arrived = ((qty - self.withdraw) * self.arrive_keep / self.step).floor() * self.step;
        if arrived <= Decimal::ZERO {
            return Err(Fail::TooSmall("withdrawal amount".into()));
        }
        let (buy, worst, buy_gas) = self
            .buy
            .price(qty, true, self.budget)
            .ok_or_else(|| Fail::TooLarge("buy depth".into()))?;
        let (sell, sell_worst, sell_gas) = self
            .sell
            .price(arrived, false, self.budget)
            .ok_or_else(|| Fail::TooLarge("sell depth".into()))?;
        if sell_worst < self.min_sale {
            return Err(Fail::TooSmall("sell minimum notional".into()));
        }
        if sell_worst > self.max_sale {
            return Err(Fail::TooLarge("sell maximum notional".into()));
        }
        let invested = worst * (Decimal::ONE + self.fee_buy);
        if invested > self.budget {
            return Err(over_budget());
        }
        let profit = if self.narrow {
            d("0.03") * qty - d("0.0001") * qty * qty - Decimal::ONE
        } else {
            sell * (Decimal::ONE - self.fee_sell)
                - buy * (Decimal::ONE + self.fee_buy)
                - self.fixed
                - buy_gas
                - sell_gas
        };
        Ok(Point {
            qty,
            invested,
            arrive: arrived,
            profit,
            data: (profit * Decimal::from(100) / buy, buy),
        })
    }
}

struct Replay<'a> {
    scenario: &'a Scenario,
    evaluated: Vec<(Decimal, Result<Sample, Fail>)>,
}

#[async_trait]
impl Curve for Replay<'_> {
    type Data = (Decimal, Decimal);

    async fn at(&mut self, qty: Decimal) -> Result<Sample, Fail> {
        let point = self.scenario.point(qty);
        self.evaluated.push((qty, point.clone()));
        point
    }

    // MATRIX_CURVE_HOOKS
}

fn levels(v: &Value) -> Vec<Level> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| (d(r[0].as_str().unwrap()), d(r[1].as_str().unwrap())))
        .collect()
}

fn scenarios() -> Vec<Scenario> {
    let snap: Value = serde_json::from_str(include_str!("size_matrix_cake.json")).unwrap();
    let book = &snap["CAKE-USDT"]["data"];
    let asks = Market::Book(levels(&book["asks"]));
    let bids = Market::Book(levels(&book["bids"]));
    let data = snap["bsc"]["reserves"]
        .as_str()
        .unwrap()
        .strip_prefix("0x")
        .unwrap();
    let reserve = |i| {
        Decimal::from(u128::from_str_radix(&data[i..i + 64], 16).unwrap())
            / d("1000000000000000000")
    };
    let pool = Market::Pool {
        coin: reserve(0),
        usdt: reserve(64),
    };
    let book_buy =
        |levels: &[(&str, &str)]| Market::Book(levels.iter().map(|(p, q)| (d(p), d(q))).collect());
    let recorded = Scenario {
        label: "cake-cex-dex",
        buy: asks,
        sell: pool.clone(),
        withdraw: d("0.01"),
        arrive_keep: Decimal::ONE,
        fixed: d("0.02"),
        fee_buy: d("0.001"),
        fee_sell: Decimal::ZERO,
        min_sale: Decimal::ONE,
        max_sale: d("10000"),
        step: d("0.01"),
        narrow: false,
        budget: Decimal::ZERO,
    };
    vec![
        recorded.clone(),
        Scenario {
            label: "cake-dex-cex",
            buy: pool,
            sell: bids,
            fee_buy: Decimal::ZERO,
            fee_sell: d("0.001"),
            ..recorded.clone()
        },
        Scenario {
            label: "cex-cex-ladder",
            buy: book_buy(&[("1", "18"), ("1.003", "35"), ("1.03", "1000")]),
            sell: book_buy(&[
                ("1.04", "16"),
                ("1.025", "42"),
                ("1.01", "120"),
                ("0.96", "2000"),
            ]),
            withdraw: d("0.25"),
            fee_sell: d("0.001"),
            min_sale: d("10"),
            step: d("0.25"),
            ..recorded.clone()
        },
        Scenario {
            label: "thin-book-fixed-fees",
            buy: book_buy(&[("1", "11"), ("1.08", "12")]),
            sell: book_buy(&[("1.02", "12"), ("0.95", "12")]),
            withdraw: Decimal::ONE,
            fee_sell: d("0.001"),
            fixed: d("0.5"),
            min_sale: d("5"),
            step: d("0.1"),
            ..recorded.clone()
        },
        Scenario {
            label: "sale-limits-withdrawal",
            buy: book_buy(&[("1", "3000")]),
            sell: book_buy(&[("1.03", "150"), ("1.015", "2000")]),
            withdraw: d("17"),
            fee_sell: d("0.001"),
            min_sale: d("20"),
            max_sale: d("80"),
            step: Decimal::ONE,
            ..recorded.clone()
        },
        Scenario {
            label: "narrow-threshold-quadratic",
            buy: book_buy(&[("1", "3000")]),
            sell: book_buy(&[("1", "3000")]),
            withdraw: Decimal::ZERO,
            fee_buy: Decimal::ZERO,
            narrow: true,
            step: Decimal::ONE,
            ..recorded
        },
    ]
}

fn passes(p: &Sample, floor: (Decimal, Decimal)) -> bool {
    p.profit >= floor.0 && p.data.0 >= floor.1
}

fn rank(p: &Sample) -> (Decimal, Decimal, Decimal) {
    (p.profit, -p.invested, -p.qty)
}

#[tokio::test]
async fn release_size_matrix() {
    for mut s in scenarios() {
        for budget in ["10", "50", "200", "850", "1000"].map(d) {
            s.budget = budget;
            // The lowest ask or pre-fee pool spot bounds any possible unit purchase cost.
            let guess = budget / s.buy.cheapest_buy();
            // One extra grid step also covers an upward Decimal division rounding boundary.
            let upper = (guess / s.step).ceil() + Decimal::ONE;
            let mut oracle = Vec::new();
            let mut qty = s.step;
            while qty <= upper * s.step {
                if let Ok(p) = s.point(qty) {
                    oracle.push(p);
                }
                qty += s.step;
            }
            for (threshold, floor) in [
                ("low", (d("0.05"), d("0.00001"))),
                ("standard", (d("0.1"), d("0.3"))),
                ("tight", (d("0.1"), d("0.997"))),
            ] {
                let best = oracle
                    .iter()
                    .filter(|p| passes(p, floor))
                    .max_by_key(|p| rank(p));
                for max_evals in [24, 300] {
                    let params = native_params(&s, guess, floor, max_evals);
                    let mut replay = Replay {
                        scenario: &s,
                        evaluated: Vec::new(),
                    };
                    let start = Instant::now();
                    let actual = search(&mut replay, &params).await;
                    let elapsed = start.elapsed().as_micros();
                    let found = actual.as_ref().ok();
                    let picked = found.map(|p| &p.best);
                    if let Some(p) = picked {
                        assert!(p.invested <= budget && (p.qty % s.step).is_zero());
                        assert_eq!(p.profit, s.point(p.qty).unwrap().profit);
                    }
                    let pass = picked.filter(|p| passes(p, floor));
                    let sampled_pass = replay
                        .evaluated
                        .iter()
                        .filter_map(|(_, p)| p.as_ref().ok())
                        .any(|p| passes(p, floor));
                    let class = match (best, pass) {
                        (None, None) => "no_feasible_profit",
                        (None, Some(_)) => panic!("search beat a supposedly exhaustive oracle"),
                        (Some(_), None) if sampled_pass => "passing_point_evaluated_not_selected",
                        (Some(_), None) if replay.evaluated.len() >= max_evals as usize => {
                            "evaluation_cap_before_passing_point"
                        }
                        (Some(_), None) if picked.is_none() => {
                            "search_error_with_oracle_opportunity"
                        }
                        (Some(_), None) => "passing_point_not_evaluated",
                        (Some(o), Some(p)) if o.profit > p.profit => "suboptimal_profit",
                        (Some(o), Some(p)) if o.invested < p.invested => {
                            "equal_profit_more_capital"
                        }
                        _ => "matched",
                    };
                    let pnl = |p: Option<&Sample>| p.map_or(Decimal::ZERO, |x| x.profit);
                    println!(
                        "SIZE_MATRIX {}",
                        json!({
                            "scenario": s.label, "budget_usdt": budget, "threshold": threshold,
                            "eval_limit": max_evals, "reason": class, "min_usdt": floor.0, "min_pct": floor.1,
                            "oracle_feasible_points": oracle.len(), "oracle_pass": best.is_some(),
                            "search_pass": pass.is_some(), "oracle_qty": best.map(|x| x.qty),
                            "oracle_profit_usdt": pnl(best), "search_profit_usdt": pnl(pass),
                            "model_gap_usdt": pnl(best) - pnl(pass), "selected_qty": picked.map(|p| p.qty),
                            "selected_invested_usdt": picked.map(|p| p.invested), "selected_raw_profit_usdt": picked.map(|p| p.profit),
                            "sampled_passing_point": sampled_pass, "evaluated": replay.evaluated.len(),
                            "search_max_qty": found.map(|f| f.choice.max_qty), "search_us": elapsed,
                            "native_error": actual.as_ref().err().map(|e| format!("{e:?}")),
                            "threshold_placement": THRESHOLD_PLACEMENT
                        })
                    );
                }
            }
        }
    }
}
