//! Файл: tests/bsc_dex_cycle_rate_quote_t189.rs
//! Что проверяет: решение 404, ревью t189 — курс ребра для отбора циклов (quote/cycle_rate.rs edge_rate) совпадает с
//! котировкой малой суммы в ОБЕ стороны на записанных пулах каждого вида, у которого есть запись состояния в tests/data
//! (без моков):
//! - V2 — пул CAKE/USDT PancakeSwap V2 (tests/size_common, getReserves, dex.v2_fee_pct = 0,25): математика бота
//!   (quote/pool_quote.rs swap) на входе 10^-6 резерва входа;
//! - V3 — пулы SquadSwap V3 (tests/data/squadswap_v3_2026-11-15/probe.json: slot0, liquidity, fee, слова TickLens):
//!   QuoterV2 КОНТРАКТА на самой малой записанной сумме (1 $), если на следующей (100 $) средняя цена та же в пределах
//!   допуска (эталон в линейной области); в записи есть пул Cake/WBNB, где цена slot0 стоит на границе тика и в сторону
//!   token0 → token1 за ней пустой участок: квотер 0,0021799 против спота 0,0027119 — курс ребра обязан совпасть с
//!   квотером (прогон 70f97160 показал расхождение 24 % у курса по споту);
//! - Algebra — пулы THENA V3 / DexSwap Integral (tests/data/algebra_integral_2026-11-06, tests/algebra_integral_common):
//!   ответы квотера записи (used, out) по тому же правилу линейной области; заодно — исправление Н4: steps::fee_pct у
//!   Algebra больше не ошибка;
//! - Solidly стабильный и волатильный — пары usdfi, veplus, magicfox (tests/data/solidly_fee_2026-11-14, комиссия по
//!   правилу DEX — tests/solidly273_common): getAmountOut КОНТРАКТА на входе 10^-3 резерва; эталоны с входом или выходом
//!   меньше 1000 единиц не берутся — их собственное округление (≥ 0,1 %) не проверяет курс с допуском 0,2 %;
//! - StableSwap — пары Nomiswap Stable (tests/data/nomiswap_stable_2026-11-15/probe.json): getAmountOut КОНТРАКТА на
//!   входе 1 токен (только где это ≤ 10^-3 баланса входа).
//!
//! Допуск — относительный, больше влияния входа эталона на цену: 0,2 % у Solidly (волатильный пул на 10^-3 резерва
//! сдвигает цену на ~0,1 %), 0,1 % у V3 / Algebra / StableSwap, 0,01 % у V2.
//! Не покрыты (нет готовой записи состояния без узла): DODO (чтение пула — только с узла, tests/bsc_dex_dodo_dead_pools —
//! поддельный узел на записи), Infinity Bin (бины — из логов, tests/bsc_dex_settle_records_357).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::{cycle_rate, pool_quote}, pools::{families, graph, load_nomi,
//! load_batch_v2v3}}, tests/{size_common, algebra_integral_common, solidly273_common}.

mod algebra_integral_common;
mod size_common;
mod solidly273_common;

use alloy_primitives::{Address, Bytes, U256, address};
use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::{PoolKind, PoolState};
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::graph::Edge;
use arb::venues::bsc_dex::pools::load_batch_v2v3::v3_ticks;
use arb::venues::bsc_dex::pools::load_nomi;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::TickMap;
use arb::venues::bsc_dex::quote::cycle_rate::edge_rate;
use arb::venues::bsc_dex::quote::pool_quote::swap;
use size_common::{CAKE, Rec, USDT};

/// Условные адреса токенов пулов записей без адресов (порядок token0 < token1 — как у записанной цены).
const T0: Address = address!("0x0000000000000000000000000000000000000a00");
const T1: Address = address!("0x0000000000000000000000000000000000000b00");

/// Отношение out/in как Decimal (минимальные единицы). Ошибок нет (тест падает).
fn ratio(out: U256, x: U256) -> Decimal {
    let d = |v: U256| Decimal::from_i128_with_scale(v.to::<i128>(), 0);
    d(out) / d(x)
}

/// Относительное расхождение |a − b| / b не больше tol. Принимает: курс ребра, котировку, допуск, что проверяем.
fn near(rate: Decimal, quote: Decimal, tol: Decimal, what: &str) {
    assert!(quote > Decimal::ZERO, "{what}: котировка 0");
    let diff = (rate - quote).abs() / quote;
    assert!(
        diff <= tol,
        "{what}: курс ребра {rate}, котировка {quote}, расхождение {diff} > {tol}"
    );
}

/// Ребро графа с записью пула (монета — token0, встречный — token1). Ошибок нет.
fn edge(e: Entry) -> Edge {
    let (a, b) = (e.coin, e.usdt);
    Edge { entry: e, a, b }
}

/// Запись пула вида kind (семейство id) на основе записи CAKE, с токенами t0/t1 (монета — t0). Ошибок нет.
fn entry(
    base: &Entry,
    id: &str,
    state: PoolState,
    fee: Option<u32>,
    t: (Address, Address),
) -> Entry {
    let f = families::make(id, id).expect("вид пула");
    let mut e = base.clone();
    e.family = f;
    e.pool.family = f.id.into();
    e.pool.kind = f.kind;
    e.pool.state = state;
    e.pool.fee_pips = fee;
    e.pool.token0 = t.0;
    e.pool.token1 = t.1;
    e.coin = t.0;
    e.usdt = t.1;
    e.ticks = None;
    e
}

/// Курс ребра и котировка бота на входе x в обе стороны. Принимает: окружение, ребро, вход по стороне (токен → x),
/// допуск, что проверяем. Возвращает: число сравнений. Ошибок нет (тест падает).
fn both_by_bot(ctx: &Ctx, ed: &Edge, xs: [(Address, U256); 2], tol: Decimal, what: &str) -> usize {
    let mut n = 0;
    for (tin, x) in xs {
        if x.is_zero() {
            continue;
        }
        let s = match swap(ctx, &ed.entry, tin, true, x) {
            Ok(s) if s.complete && !s.amount_out.is_zero() => s,
            _ => continue,
        };
        let r = edge_rate(ctx, ed, tin).expect("у пула записи есть курс");
        near(r, ratio(s.amount_out, x), tol, what);
        n += 1;
    }
    n
}

#[tokio::test]
async fn v2_rate_matches_small_quote() {
    let rec = Rec::load("rate-t189-v2").await;
    let PoolState::V2 { reserve0, reserve1 } = rec.pool.pool.state else {
        panic!("V2")
    };
    let ed = edge(rec.pool.clone());
    let m = U256::from(1_000_000u64);
    // token0 CAKE, token1 USDT (size_common).
    let xs = [(CAKE, reserve0 / m), (USDT, reserve1 / m)];
    let n = both_by_bot(&rec.ctx, &ed, xs, Decimal::new(1, 4), "V2");
    assert_eq!(n, 2, "обе стороны V2");
}

/// Слово `i` ответа.
fn word(raw: &str, i: usize) -> U256 {
    let h = raw.trim_start_matches("0x");
    U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
}

/// Слово `i` как int24.
fn int24(raw: &str, i: usize) -> i32 {
    let w = word(raw, i);
    if w.bit(255) {
        -((!w).wrapping_add(U256::from(1u8)).to::<u64>() as i32)
    } else {
        w.to::<u64>() as i32
    }
}

/// JSON файла записи.
fn json(path: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Эталон «малой суммы» из ответов квотера контракта по стороне: (вход, выход) самой малой суммы, если средняя цена на
/// следующей сумме отличается не больше чем на tol (эталон в линейной области: курс не зависит от суммы); иначе None —
/// записанная малая сумма уже сдвигает цену, сравнивать курс ребра с ней нельзя. Ошибок нет.
fn linear_ref(mut pts: Vec<(U256, U256)>, tol: Decimal) -> Option<Decimal> {
    pts.retain(|(x, o)| !x.is_zero() && !o.is_zero());
    pts.sort();
    let a = ratio(pts.first()?.1, pts.first()?.0);
    let b = ratio(pts.get(1)?.1, pts.get(1)?.0);
    ((a - b).abs() / a <= tol).then_some(a)
}

#[tokio::test]
async fn v3_rate_matches_contract_quoter() {
    let rec = Rec::load("rate-t189-v3").await;
    let d = json("tests/data/squadswap_v3_2026-11-15/probe.json");
    let tol = Decimal::new(1, 3);
    let (mut n, mut gap) = (0, false);
    for p in d["pools"].as_array().unwrap() {
        let c = &p["calls"];
        let slot0 = c["slot0"].as_str().unwrap();
        let (sqrt, tick) = (word(slot0, 0), int24(slot0, 1));
        let liq = word(c["liquidity"].as_str().unwrap(), 0).to::<u128>();
        let fee = word(c["fee"].as_str().unwrap(), 0).to::<u32>();
        let spacing = word(c["tickSpacing"].as_str().unwrap(), 0).to::<u64>() as i32;
        let words = p["words"].as_array().unwrap();
        let ans: Vec<Bytes> = words
            .iter()
            .map(|w| {
                Bytes::from(
                    alloy_primitives::hex::decode(
                        w["raw"].as_str().unwrap().trim_start_matches("0x"),
                    )
                    .unwrap(),
                )
            })
            .collect();
        let first = words.first().unwrap()["word"].as_i64().unwrap() as i16;
        let last = words.last().unwrap()["word"].as_i64().unwrap() as i16;
        let state = PoolState::V3 {
            sqrt_price_x96: sqrt,
            liquidity: liq,
            tick,
        };
        let mut e = entry(&rec.pool, "pancakeswap_v3", state, Some(fee), (T0, T1));
        e.pool.tick_spacing = Some(spacing);
        e.ticks = Some(TickMap {
            spacing,
            ticks: v3_ticks(&ans).expect("тики").into(),
            words: (first, last),
        });
        let ed = edge(e);
        for zfo in [true, false] {
            let pts: Vec<(U256, U256)> = p["quoter"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|q| q["zeroForOne"].as_bool() == Some(zfo))
                .map(|q| {
                    let a = U256::from_str_radix(q["amountIn"].as_str().unwrap(), 10).unwrap();
                    (a, word(q["quoterV2_defaultFee"].as_str().unwrap(), 0))
                })
                .collect();
            let Some(want) = linear_ref(pts, tol) else {
                continue;
            };
            let tin = if zfo { T0 } else { T1 };
            let r = edge_rate(&rec.ctx, &ed, tin).expect("у пула записи есть курс");
            near(r, want, tol, "V3 (квотер контракта)");
            // Пул Cake/WBNB записи: цена slot0 на границе тика, в сторону token0 → token1 — пустой участок; спот × (1 −
            // комиссия) отличается от квотера больше чем на 10 % — курс ребра обязан идти не по споту.
            let spot = Decimal::from_i128_with_scale((sqrt * sqrt >> 128).to::<i128>(), 0)
                / Decimal::from_i128_with_scale(1i128 << 64, 0);
            if zfo && (spot - want).abs() / want > Decimal::new(1, 1) {
                gap = true;
            }
            n += 1;
        }
    }
    assert!(n >= 4, "сравнений V3 с квотером контракта: {n}");
    assert!(
        gap,
        "в записи есть пул с пустым участком за границей тика (Cake/WBNB)"
    );
}

#[tokio::test]
async fn algebra_rate_matches_contract_quoter() {
    let rec = Rec::load("rate-t189-alg").await;
    let d = algebra_integral_common::probe();
    let tol = Decimal::new(1, 3);
    let mut n = 0;
    for p in d["pools"].as_array().unwrap() {
        let (sqrt, tick, liq, fee, map) = algebra_integral_common::pool_state(p);
        // Ликвидность текущего диапазона 0 — курса ребра нет по правилу (виртуальный резерв 0, cycle_rate.rs).
        if liq == 0 {
            continue;
        }
        let state = PoolState::Algebra {
            sqrt_price_x96: sqrt,
            tick,
            liquidity: liq,
            fee,
        };
        let mut e = entry(&rec.pool, "thena_fusion", state, None, (T0, T1));
        e.ticks = Some(map);
        let ed = edge(e);
        for zfo in [true, false] {
            // Ответ виртуального квотера записи — (used, out); откат («__err») — пропуск.
            let pts: Vec<(U256, U256)> = p["quoter"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|q| q["zeroForOne"].as_bool() == Some(zfo))
                .filter_map(|q| {
                    let raw = q["raw"].as_str()?;
                    raw.starts_with("0x").then(|| (word(raw, 0), word(raw, 1)))
                })
                .collect();
            let Some(want) = linear_ref(pts, tol) else {
                continue;
            };
            let tin = if zfo { T0 } else { T1 };
            let r = edge_rate(&rec.ctx, &ed, tin).expect("у пула записи есть курс");
            near(r, want, tol, "Algebra (квотер записи)");
            n += 1;
        }
    }
    assert!(n >= 2, "сравнений Algebra с квотером: {n}");
}

#[tokio::test]
async fn solidly_rate_matches_contract() {
    let rec = Rec::load("rate-t189-sol").await;
    let s = solidly273_common::load("state.json");
    let u = solidly273_common::u;
    let (mut stable, mut volatile) = (0, 0);
    for (dex, d, p, x, tin, out) in solidly273_common::samples(&s) {
        let m = &p["metadata"];
        let t0: Address = m["t0"].as_str().unwrap().parse().unwrap();
        let t1: Address = m["t1"].as_str().unwrap().parse().unwrap();
        let tin: Address = tin.parse().unwrap();
        let r_in = if tin == t0 { u(&m["r0"]) } else { u(&m["r1"]) };
        // Только малый вход (≤ 1/500 резерва входа; в записи — 10^-3 и 3,3·10^-2). И эталон, чьё округление до целых
        // единиц само не хуже половины допуска: вход и выход контракта ≥ 2/допуск единиц (пары 18/6 с резервом 0,68 USDT:
        // вход 680 или выход 558 единиц — точность эталона 0,15–0,18 %, им курс на 0,2 % не проверить).
        let tol = Decimal::new(2, 3);
        let min_units = U256::from(1000u32);
        if x.is_zero() || x * U256::from(500u32) > r_in || x < min_units || out < min_units {
            continue;
        }
        let st = m["st"].as_bool().unwrap();
        let state = PoolState::Solidly {
            reserve0: u(&m["r0"]),
            reserve1: u(&m["r1"]),
            stable: st,
            decimals0: u(&m["dec0"]),
            decimals1: u(&m["dec1"]),
            fee: solidly273_common::fee_of(&dex, &d, &p),
            gate: Default::default(),
        };
        let e = entry(&rec.pool, "solidly_v1", state, None, (t0, t1));
        let r = edge_rate(&rec.ctx, &edge(e), tin).expect("у пары записи есть курс");
        let what = format!("Solidly {dex} {}", if st { "stable" } else { "volatile" });
        near(r, ratio(out, x), tol, &what);
        if st {
            stable += 1;
        } else {
            volatile += 1;
        }
    }
    assert!(
        stable >= 4 && volatile >= 4,
        "стабильных {stable}, волатильных {volatile}"
    );
}

#[tokio::test]
async fn stableswap_rate_matches_contract() {
    let rec = Rec::load("rate-t189-stable").await;
    let d = json("tests/data/nomiswap_stable_2026-11-15/probe.json");
    let addr = |v: &Value| -> Address { v.as_str().unwrap().parse().unwrap() };
    let s32 = |v: &Value| -> u32 { v.as_str().unwrap().parse().unwrap() };
    let s128 = |v: &Value| -> u128 { v.as_str().unwrap().parse().unwrap() };
    let mut n = 0;
    for p in d["pairs"].as_array().unwrap() {
        let (t0, t1) = (addr(&p["token0"]), addr(&p["token1"]));
        let r = p["getReserves"].as_str().unwrap();
        let (r0, r1) = (word(r, 0), word(r, 1));
        let state = load_nomi::state(
            t0,
            (t0, t1),
            (r0, r1),
            s32(&p["swapFee"]),
            U256::from_str_radix(p["getA"].as_str().unwrap(), 10).unwrap(),
            (
                s128(&p["token0PrecisionMultiplier"]),
                s128(&p["token1PrecisionMultiplier"]),
            ),
        )
        .expect("состояние");
        let e = entry(&rec.pool, "nomiswap_stable", state, None, (t0, t1));
        assert_eq!(e.pool.kind, PoolKind::StableSwap);
        let ed = edge(e);
        for g in p["getAmountOut"].as_array().unwrap() {
            let side = g[0].as_u64().unwrap();
            let x = U256::from_str_radix(g[1].as_str().unwrap(), 10).unwrap();
            let out = U256::from_str_radix(g[2].as_str().unwrap(), 10).unwrap();
            let (tin, r_in) = if side == 0 { (t0, r0) } else { (t1, r1) };
            if x.is_zero() || x * U256::from(1000u32) > r_in {
                continue;
            }
            let rate = edge_rate(&rec.ctx, &ed, tin).expect("у пары записи есть курс");
            near(
                rate,
                ratio(out, x),
                Decimal::new(1, 3),
                "StableSwap Nomiswap",
            );
            n += 1;
        }
    }
    assert!(n >= 2, "сравнений StableSwap: {n}");
}
