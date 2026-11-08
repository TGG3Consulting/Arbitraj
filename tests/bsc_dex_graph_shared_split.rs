//! Файл: tests/bsc_dex_graph_shared_split.rs
//! Что делает: решение 223 — сплит графа (quote/graph_split.rs) с общими пулами на РЕАЛЬНОЙ записи CAKE
//! (tests/route_common, routes_2026-10-29 / 10-30):
//! (в) маршруты без общих пулов — котировка графа побайтно та же, что до правки: снимок выхода старого кода на тех же
//!     записях (tests/data/graph_shared_2026-11-08/gold_before.txt — обе записи, без учёта газа и с ним, покупка и
//!     продажа, 1…100000 CAKE: доли, пулы, доли входа, ожидаемые выходы, средняя цена);
//! (г) сценарий FLUX (живой случай 08.11: все доли начинаются одним пулом V2) на записи: доли USDT→WBNB(V2)→… делят
//!     первый пул V2. Новый сплит: итог = последовательное исполнение (доля 2 — на резервах V2 после доли 1, руками
//!     до wei); старая модель (каждая доля на исходном состоянии) занижала стоимость покупки — истинная стоимость её
//!     раскладки больше заявленной; новая раскладка не дороже истинной стоимости старой. Доля без общего пула не меняется.
//!     Четыре доли через одни и те же пулы (V2 + V3; старая модель — 30/30/20/20, как 0.3/0.3/0.3/0.1 у FLUX): старая
//!     заявляла продажу на ~0,55% лучше истинной, новая — одна доля, ровно один обмен всего объёма.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::quote::{graph_route, graph_split, graph_seq, route_calc, graph_legs, select},
//! tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{Asset, PoolRef, PoolState, Side, TokenTaxes};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::Quoting;
use arb::venues::bsc_dex::quote::graph_split::{self, Cand};
use arb::venues::bsc_dex::quote::path::Path;
use arb::venues::bsc_dex::quote::route_calc::{self, Leg};
use arb::venues::bsc_dex::quote::steps::Names;
use arb::venues::bsc_dex::quote::via::Via;
use arb::venues::bsc_dex::quote::{graph_legs, graph_route, graph_seq, select};

use route_common::{
    BTCB, BTCB_WBNB_PCS, CAKE, CAKE_BTCB_UNI, CAKE_USDT_PCS, CAKE_WBNB_PCS, USDT, WBNB,
    WBNB_USDT_V2,
};

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

fn short(r: &PoolRef) -> String {
    match r {
        PoolRef::Contract(a) => format!("{a}")[..8].to_string(),
        other => format!("{other:?}"),
    }
}

/// Строка снимка котировки графа — тот же формат, что в gold_before.txt.
async fn line(p: &Parts, side: Side, qty: u32, tag: &str) -> String {
    let q = p.quoting();
    let t = zero();
    let v = Via {
        q: &q,
        coin: CAKE,
        usdt: USDT,
        side,
        taxes: &t,
        coin_dec: 18,
        usdt_dec: 18,
    };
    let names = Names {
        usdt: (USDT, "USDT"),
        coin: (CAKE, "CAKE"),
        mids: &p.mids,
    };
    let asset = Asset {
        ticker: "CAKE".into(),
        bsc_contract: Some(CAKE),
    };
    match graph_route::quote(
        &v,
        &names,
        &asset,
        Decimal::from(qty),
        U256::from(1_000_000_000u64),
    )
    .await
    {
        Ok(quote) => {
            let r = quote.route.unwrap();
            let legs: Vec<String> = r
                .legs
                .iter()
                .map(|l| {
                    let hops: Vec<String> = l.hops.iter().map(|h| short(&h.pool)).collect();
                    format!("{}|{}|{}", hops.join(">"), l.share, l.expected_out)
                })
                .collect();
            format!(
                "GOLD {tag} {side:?} {qty} avg={} fill={} legs={}",
                quote.avg_price,
                quote.fillable_qty,
                legs.join(" ; ")
            )
        }
        Err(e) => format!("GOLD {tag} {side:?} {qty} ERR {}", e.message),
    }
}

#[tokio::test]
async fn no_shared_pools_byte_identical_to_before() {
    let mut got = Vec::new();
    for dir in [
        "tests/data/routes_2026-10-29",
        "tests/data/routes_2026-10-30",
    ] {
        for gas in [false, true] {
            let p = route_common::parts_from(dir).await;
            if gas {
                let v = Scope::Venue("bsc_dex".into());
                p.ctx
                    .settings
                    .set("dex.route_gas_aware", v.clone(), Some(json!(true)), "тест")
                    .await
                    .unwrap();
                p.ctx
                    .settings
                    .set(
                        "dex.native_wrapped",
                        v,
                        Some(json!(WBNB.to_string())),
                        "тест",
                    )
                    .await
                    .unwrap();
            }
            let tag = format!("{}{}", &dir[17..], if gas { "-gas" } else { "" });
            for side in [Side::Buy, Side::Sell] {
                for qty in [1u32, 100, 1000, 5000, 20_000, 50_000, 100_000] {
                    got.push(line(&p, side, qty, &tag).await);
                }
            }
        }
    }
    // Порядок долей в маршруте зависит от порядка перебора путей (множества в графе) и между запусками бывает разным
    // и до правки — сравнение по долям без порядка; числа каждой доли и средняя цена — побайтно.
    let canon = |l: &str| match l.split_once(" legs=") {
        Some((head, legs)) => {
            let mut v: Vec<&str> = legs.split(" ; ").collect();
            v.sort();
            format!("{head} legs={}", v.join(" ; "))
        }
        None => l.to_string(),
    };
    let mut got: Vec<String> = got.iter().map(|l| canon(l)).collect();
    got.sort();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/graph_shared_2026-11-08/gold_before.txt"
    );
    let mut want: Vec<String> = std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(canon)
        .collect();
    want.sort();
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g, w);
    }
}

fn leg(q: &Quoting, addr: Address, tin: Address, tout: Address) -> Leg {
    let e = q.reg.get(&PoolRef::Contract(addr)).expect("пул записи");
    let side = graph_legs::hop_side(&e, tin).expect("ребро");
    let usable = select::usable(q, &e, side, 18).expect("пул допущен");
    Leg {
        entry: e,
        usable,
        token_in: tin,
        token_out: tout,
        dec_in: 18,
        dec_out: 18,
    }
}

/// Покупка CAKE путями, начинающимися ОДНИМ пулом V2 USDT→WBNB (как FLUX: все доли — один первый пул), и прямым пулом
/// CAKE-USDT (без общего пула).
fn cands(q: &Quoting) -> Vec<Vec<Leg>> {
    vec![
        vec![
            leg(q, WBNB_USDT_V2, USDT, WBNB),
            leg(q, CAKE_WBNB_PCS, WBNB, CAKE),
        ],
        vec![
            leg(q, WBNB_USDT_V2, USDT, WBNB),
            leg(q, BTCB_WBNB_PCS, WBNB, BTCB),
            leg(q, CAKE_BTCB_UNI, BTCB, CAKE),
        ],
        vec![leg(q, CAKE_USDT_PCS, USDT, CAKE)],
    ]
}

/// Старая модель сплита (до решения 223, без газа): каждая доля — на исходном состоянии, независимо. Возвращает:
/// (индекс кандидата, монет доли, заявленный USDT доли). Ошибок нет.
fn old_split(
    q: &Quoting,
    c: &[Vec<Leg>],
    qty: Decimal,
    step_pct: u32,
    max: usize,
) -> Vec<(usize, Decimal, Decimal)> {
    let unit = qty * Decimal::from(step_pct) / Decimal::ONE_HUNDRED;
    let mut got: Vec<Option<(Decimal, Decimal)>> = vec![None; c.len()];
    let mut left = qty;
    while left > Decimal::ZERO {
        let step = unit.min(left);
        let used = got.iter().filter(|g| g.is_some()).count();
        let mut best: Option<(usize, Decimal, Decimal)> = None;
        for (i, legs) in c.iter().enumerate() {
            if got[i].is_none() && used >= max {
                continue;
            }
            let (qn, before) = got[i].unwrap_or((Decimal::ZERO, Decimal::ZERO));
            let Ok(p) = route_calc::eval(&q.ctx, legs, Side::Buy, qn + step, Decimal::ONE) else {
                continue;
            };
            let d = p.usdt - before;
            if best.is_none_or(|(_, _, b)| d < b) {
                best = Some((i, p.usdt, d));
            }
        }
        let Some((i, usdt, _)) = best else { break };
        let qn = got[i].map_or(Decimal::ZERO, |g| g.0) + step;
        got[i] = Some((qn, usdt));
        left -= step;
    }
    got.iter()
        .enumerate()
        .filter_map(|(i, g)| g.map(|(a, b)| (i, a, b)))
        .collect()
}

fn total(ps: &[Path]) -> Decimal {
    ps.iter().map(|p| p.usdt).sum()
}

#[tokio::test]
async fn flux_like_shared_first_v2_pool() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let all = cands(&q);
    for (set, qty) in [
        (3usize, 20_000u32),
        (3, 50_000),
        (3, 100_000),
        (2, 1000),
        (2, 5000),
        (2, 20_000),
    ] {
        let c = &all[..set];
        let qty = Decimal::from(qty);
        let mut cs: Vec<Cand> = c
            .iter()
            .map(|l| Cand::new(l.clone(), Decimal::ZERO))
            .collect();
        let new = graph_split::split(&q.ctx, &mut cs, Side::Buy, qty, &t, false).expect("сплит");
        let alloc: Vec<(usize, Decimal)> = cs
            .iter()
            .enumerate()
            .filter(|(_, x)| x.qty > Decimal::ZERO)
            .map(|(i, x)| (i, x.qty))
            .collect();
        // Итог нового сплита — ровно последовательное исполнение раскладки.
        let input: Vec<(&[Leg], Decimal)> =
            alloc.iter().map(|(i, n)| (c[*i].as_slice(), *n)).collect();
        let seq = graph_seq::eval(&q.ctx, &input, Side::Buy, &t).unwrap();
        assert_eq!(total(&new), total(&seq));
        // Руками: если обе доли через V2 открыты — вторая на резервах после первой (точный вход первой доли).
        let via_v2: Vec<usize> = (0..alloc.len()).filter(|&k| alloc[k].0 < 2).collect();
        if via_v2.len() == 2 {
            let (k1, k2) = (via_v2[0], via_v2[1]);
            let first = &new[k1].hops[0];
            let (r0, r1) = match c[0][0].entry.pool.state {
                PoolState::V2 { reserve0, reserve1 } => (reserve0, reserve1),
                _ => unreachable!(),
            };
            // token0 = USDT: вход USDT в reserve0, выход WBNB из reserve1 (getAmountOut 9975/10000).
            let a = first.swapped.amount_in;
            let f = a * U256::from(9975u32);
            let out = f * r1 / (r0 * U256::from(10000u32) + f);
            let mut moved = c[alloc[k2].0].clone();
            moved[0].entry.pool.state = PoolState::V2 {
                reserve0: r0 + a,
                reserve1: r1 - out,
            };
            let want =
                route_calc::eval(&q.ctx, &moved, Side::Buy, alloc[k2].1, Decimal::ONE).unwrap();
            assert_eq!(
                new[k2].usdt, want.usdt,
                "вторая доля через общий V2 — на резервах после первой, до wei"
            );
        }
        // Доля без общего пула (прямой CAKE-USDT) — как в одиночку.
        if let Some(k) = alloc.iter().position(|(i, _)| *i == 2) {
            assert_eq!(set, 3);
            let alone =
                route_calc::eval(&q.ctx, &c[2], Side::Buy, alloc[k].1, Decimal::ONE).unwrap();
            assert_eq!(new[k].usdt, alone.usdt);
        }
        // Старая модель: заявленная стоимость её раскладки против истинной (последовательной).
        let old = old_split(&q, c, qty, 10, 5);
        let old_claimed: Decimal = old.iter().map(|x| x.2).sum();
        let old_in: Vec<(&[Leg], Decimal)> =
            old.iter().map(|(i, n, _)| (c[*i].as_slice(), *n)).collect();
        let old_true = total(&graph_seq::eval(&q.ctx, &old_in, Side::Buy, &t).unwrap());
        let shared_old = old.iter().filter(|x| x.0 < 2).count() == 2;
        println!(
            "покупка {qty} CAKE: старая раскладка {:?} заявлено {old_claimed} истинно {old_true} (занижено на {}) | новая {:?} = {}",
            old.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(),
            old_true - old_claimed,
            alloc,
            total(&new)
        );
        if shared_old {
            assert!(
                old_true > old_claimed,
                "старая модель занижала стоимость общего пула"
            );
        }
        assert!(
            total(&new) <= old_true,
            "новая раскладка не дороже истинной стоимости старой"
        );
    }
}

/// Старая модель для любой стороны (см. old_split; продажа — больше лучше). Ошибок нет.
fn old_split_side(
    q: &Quoting,
    c: &[Vec<Leg>],
    side: Side,
    qty: Decimal,
) -> Vec<(usize, Decimal, Decimal)> {
    let unit = qty / Decimal::from(10u32);
    let mut got: Vec<Option<(Decimal, Decimal)>> = vec![None; c.len()];
    let mut left = qty;
    while left > Decimal::ZERO {
        let step = unit.min(left);
        let mut best: Option<(usize, Decimal, Decimal)> = None;
        for (i, legs) in c.iter().enumerate() {
            let (qn, before) = got[i].unwrap_or((Decimal::ZERO, Decimal::ZERO));
            let Ok(p) = route_calc::eval(&q.ctx, legs, side, qn + step, Decimal::ONE) else {
                continue;
            };
            let d = p.usdt - before;
            let wins = match side {
                Side::Buy => best.is_none_or(|(_, _, b)| d < b),
                Side::Sell => best.is_none_or(|(_, _, b)| d > b),
            };
            if wins {
                best = Some((i, p.usdt, d));
            }
        }
        let Some((i, usdt, _)) = best else { break };
        let qn = got[i].map_or(Decimal::ZERO, |g| g.0) + step;
        got[i] = Some((qn, usdt));
        left -= step;
    }
    got.iter()
        .enumerate()
        .filter_map(|(i, g)| g.map(|(a, b)| (i, a, b)))
        .collect()
}

/// Сценарий FLUX в чистом виде: четыре доли через одни и те же пулы (общий пул V2 + V3). Старая модель видела
/// четырёхкратную «независимую» ликвидность и раскладывала объём на доли (30/30/20/20 — как 0.3/0.3/0.3/0.1 у FLUX) —
/// заявленный итог лучше истинного. Новая модель: итог = последовательное исполнение; дробление через те же пулы ничего
/// не даёт (V3 путь-независим, у V2 подряд даже хуже — комиссия первой доли остаётся в резерве входа), поэтому остаётся
/// одна доля = один обмен всего объёма.
#[tokio::test]
async fn four_shares_through_same_pools_like_flux() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let buy = vec![
        leg(&q, WBNB_USDT_V2, USDT, WBNB),
        leg(&q, CAKE_WBNB_PCS, WBNB, CAKE),
    ];
    let sell = vec![
        leg(&q, CAKE_WBNB_PCS, CAKE, WBNB),
        leg(&q, WBNB_USDT_V2, WBNB, USDT),
    ];
    for (side, path, qty) in [
        (Side::Buy, &buy, 20_000u32),
        (Side::Sell, &sell, 20_000),
        (Side::Sell, &sell, 5000),
    ] {
        let qty = Decimal::from(qty);
        let c: Vec<Vec<Leg>> = (0..4).map(|_| path.clone()).collect();
        let mut cs: Vec<Cand> = c
            .iter()
            .map(|l| Cand::new(l.clone(), Decimal::ZERO))
            .collect();
        let new = graph_split::split(&q.ctx, &mut cs, side, qty, &t, false).expect("сплит");
        let alloc: Vec<(usize, Decimal)> = cs
            .iter()
            .enumerate()
            .filter(|(_, x)| x.qty > Decimal::ZERO)
            .map(|(i, x)| (i, x.qty))
            .collect();
        let input: Vec<(&[Leg], Decimal)> =
            alloc.iter().map(|(i, n)| (c[*i].as_slice(), *n)).collect();
        assert_eq!(
            total(&new),
            total(&graph_seq::eval(&q.ctx, &input, side, &t).unwrap())
        );
        let single = route_calc::eval(&q.ctx, path, side, qty, Decimal::ONE)
            .unwrap()
            .usdt;
        let old = old_split_side(&q, &c, side, qty);
        let old_claimed: Decimal = old.iter().map(|x| x.2).sum();
        let old_in: Vec<(&[Leg], Decimal)> =
            old.iter().map(|(i, n, _)| (c[*i].as_slice(), *n)).collect();
        let old_true = total(&graph_seq::eval(&q.ctx, &old_in, side, &t).unwrap());
        println!(
            "{side:?} {qty} CAKE, 4 одинаковых пути: старая раскладка {:?} заявлено {old_claimed} истинно {old_true} \
             (ошибка {}) | новая {:?} = {} | один обмен всего объёма {single}",
            old.iter().map(|x| (x.0, x.1)).collect::<Vec<_>>(),
            old_true - old_claimed,
            alloc,
            total(&new)
        );
        assert!(
            old.len() > 1,
            "старая модель раскладывала объём по «независимым» копиям пула"
        );
        match side {
            Side::Buy => {
                assert!(
                    old_claimed < old_true,
                    "старая модель занижала стоимость покупки"
                );
                assert!(total(&new) <= old_true);
            }
            Side::Sell => {
                assert!(
                    old_claimed > old_true,
                    "старая модель завышала выручку продажи (случай FLUX)"
                );
                assert!(total(&new) >= old_true);
            }
        }
        let eps = single / Decimal::from(1_000_000_000_000u64);
        assert!(
            (total(&new) - single).abs() <= eps,
            "новая сумма {} против одного обмена {single}",
            total(&new)
        );
    }
}
