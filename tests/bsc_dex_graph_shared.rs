//! Файл: tests/bsc_dex_graph_shared.rs
//! Что делает: решение 223 — доли с ОБЩИМ пулом считаются по порядку исполнения на последовательных состояниях пула
//! (quote/graph_seq.rs, quote/pool_after.rs) на РЕАЛЬНОЙ записи CAKE 2026-10-29 (tests/route_common, блок 130521896) и
//! записи Algebra Integral 2026-11-06:
//! (а) две доли через один пул V2 (WBNB-USDT) — выход каждой до wei равен ручному getAmountOut на резервах после прошлой
//!     доли (reserveIn + in, reserveOut − out — UniswapV2Pair._update); вторая доля меньше, чем на исходных резервах;
//!     то же для долей CAKE→WBNB→USDT и CAKE→BTCB→WBNB→USDT, делящих последний хоп V2;
//! (б) две доли через один пул Pancake V3 (CAKE-USDT): первая — ровно как в одиночку; сумма двух ≈ одному обмену суммы
//!     (CL путь-независим до округлений шагов); ликвидность после обмена = ликвидность до ± liquidityNet пересечённых
//!     тиков карты; тик = getTickAtSqrtRatio(цена);
//! (в) доля без общих пулов — ровно route_calc::eval в одиночку;
//! (г) общий пул вида без расчёта состояния после обмена (Algebra Integral) — маршрут не считается (NoRoute).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::{graph_seq, pool_after, route_calc, graph_legs, select},
//! math::{swap_cl, tick_math}, pools}, tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256};
use rust_decimal::Decimal;

use arb::core::types::{PoolRef, PoolState, Side, TokenTaxes};
use arb::venues::bsc_dex::math::tick_math;
use arb::venues::bsc_dex::num;
use arb::venues::bsc_dex::quote::Quoting;
use arb::venues::bsc_dex::quote::route_calc::{self, Leg};
use arb::venues::bsc_dex::quote::select::{self, Usable};
use arb::venues::bsc_dex::quote::{graph_legs, graph_seq, pool_after};

use route_common::{
    BTCB, BTCB_WBNB_PCS, CAKE, CAKE_BTCB_UNI, CAKE_USDT_PCS, CAKE_USDT_UNI, CAKE_WBNB_PCS, USDT,
    WBNB, WBNB_USDT_V2,
};

fn zero() -> TokenTaxes {
    TokenTaxes {
        buy_pct: Decimal::ZERO,
        sell_pct: Decimal::ZERO,
        transfer_pct: Decimal::ZERO,
    }
}

fn units(x: u64) -> U256 {
    U256::from(x) * U256::from(10u64).pow(U256::from(18u8))
}

/// Нога через записанный пул `addr` от `tin` к `tout` (допуск — по стороне шага, как graph_legs).
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

/// getAmountOut пары V2 руками (комиссия 0,25% записи — 9975/10000; PancakeLibrary). Ошибок нет.
fn v2_out(a: U256, r_in: U256, r_out: U256) -> U256 {
    let f = a * U256::from(9975u32);
    f * r_out / (r_in * U256::from(10000u32) + f)
}

fn v2_reserves(l: &Leg) -> (U256, U256) {
    match l.entry.pool.state {
        PoolState::V2 { reserve0, reserve1 } => (reserve0, reserve1),
        _ => panic!("не V2"),
    }
}

#[tokio::test]
async fn v2_two_shares_match_hand_math_to_wei() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let l = leg(&q, WBNB_USDT_V2, WBNB, USDT);
    // token0 = USDT (0x55…) < WBNB (0xbb…): продаём WBNB — вход reserve1.
    let (r_usdt, r_wbnb) = v2_reserves(&l);
    let (q1, q2) = (units(40), units(60));
    let o1 = v2_out(q1, r_wbnb, r_usdt);
    let o2 = v2_out(q2, r_wbnb + q1, r_usdt - o1);
    let alone2 = v2_out(q2, r_wbnb, r_usdt);
    let d = |u: U256| num::to_decimal(u, 18).unwrap();
    let shares = [
        (std::slice::from_ref(&l), d(q1)),
        (std::slice::from_ref(&l), d(q2)),
    ];
    let paths = graph_seq::eval(&q.ctx, &shares, Side::Sell, &t).expect("доли считаются");
    println!(
        "V2: доля1 {} доля2 {} (в одиночку доля2 была бы {})",
        d(o1),
        d(o2),
        d(alone2)
    );
    assert_eq!(paths[0].hops[0].swapped.amount_out, o1);
    assert_eq!(
        paths[1].hops[0].swapped.amount_out, o2,
        "вторая доля — на резервах после первой, до wei"
    );
    assert_eq!(paths[0].usdt + paths[1].usdt, d(o1) + d(o2));
    assert!(o2 < alone2, "общий пул — не независимая ликвидность");
}

#[tokio::test]
async fn shared_last_hop_v2_second_share_on_moved_reserves() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let a = vec![
        leg(&q, CAKE_WBNB_PCS, CAKE, WBNB),
        leg(&q, WBNB_USDT_V2, WBNB, USDT),
    ];
    let b = vec![
        leg(&q, CAKE_BTCB_UNI, CAKE, BTCB),
        leg(&q, BTCB_WBNB_PCS, BTCB, WBNB),
        leg(&q, WBNB_USDT_V2, WBNB, USDT),
    ];
    let (qa, qb) = (Decimal::from(3000u32), Decimal::from(2u32));
    let paths = graph_seq::eval(&q.ctx, &[(&a, qa), (&b, qb)], Side::Sell, &t).expect("доли");
    // Доля A — как в одиночку (первая в порядке исполнения).
    let alone_a = route_calc::eval(&q.ctx, &a, Side::Sell, qa, Decimal::ONE).unwrap();
    assert_eq!(paths[0].usdt, alone_a.usdt);
    // Доля B — на резервах V2 после A, руками: reserveIn + вход A, reserveOut − выход A.
    let h = &paths[0].hops[1].swapped;
    let (r_usdt, r_wbnb) = v2_reserves(&a[1]);
    let mut b_moved = b.clone();
    b_moved[2].entry.pool.state = PoolState::V2 {
        reserve0: r_usdt - h.amount_out,
        reserve1: r_wbnb + h.amount_in,
    };
    let want_b = route_calc::eval(&q.ctx, &b_moved, Side::Sell, qb, Decimal::ONE).unwrap();
    let alone_b = route_calc::eval(&q.ctx, &b, Side::Sell, qb, Decimal::ONE).unwrap();
    println!(
        "общий хоп V2: B после A {} (в одиночку {})",
        want_b.usdt, alone_b.usdt
    );
    assert_eq!(paths[1].usdt, want_b.usdt, "до wei");
    assert_eq!(paths[1].hops[2].swapped, want_b.hops[2].swapped);
    assert!(paths[1].usdt < alone_b.usdt);
}

#[tokio::test]
async fn v3_shared_pool_sequential_state() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let l = leg(&q, CAKE_USDT_PCS, CAKE, USDT);
    let (q1, q2) = (Decimal::from(20_000u32), Decimal::from(30_000u32));
    let one = std::slice::from_ref(&l);
    let paths = graph_seq::eval(&q.ctx, &[(one, q1), (one, q2)], Side::Sell, &t).expect("доли V3");
    let alone1 = route_calc::eval(&q.ctx, one, Side::Sell, q1, Decimal::ONE).unwrap();
    let alone2 = route_calc::eval(&q.ctx, one, Side::Sell, q2, Decimal::ONE).unwrap();
    let both = route_calc::eval(&q.ctx, one, Side::Sell, q1 + q2, Decimal::ONE).unwrap();
    assert_eq!(
        paths[0].hops[0].swapped, alone1.hops[0].swapped,
        "первая доля — как в одиночку"
    );
    let seq = paths[0].hops[0].swapped.amount_out + paths[1].hops[0].swapped.amount_out;
    let one_swap = both.hops[0].swapped.amount_out;
    let diff = if seq > one_swap {
        seq - one_swap
    } else {
        one_swap - seq
    };
    println!(
        "V3: доля1 {} доля2 {} сумма {} | один обмен суммы {} | разница {diff} wei | доля2 в одиночку {}",
        paths[0].usdt,
        paths[1].usdt,
        paths[0].usdt + paths[1].usdt,
        both.usdt,
        alone2.usdt
    );
    // CL путь-независим: две доли подряд = один обмен суммы с точностью до округлений шагов (единицы wei).
    assert!(diff <= U256::from(10u8), "разница {diff} wei");
    assert!(paths[1].usdt < alone2.usdt);
    // Состояние после первой доли: ликвидность — по пересечённым тикам карты, тик — по цене.
    let units1 = num::to_units(q1, 18).unwrap();
    let (_, st) = pool_after::apply(&q.ctx, &l.entry, CAKE, units1).unwrap();
    let (
        PoolState::V3 {
            sqrt_price_x96: s0,
            liquidity: l0,
            tick: t0,
        },
        PoolState::V3 {
            sqrt_price_x96: s1,
            liquidity: l1,
            tick: t1,
        },
    ) = (l.entry.pool.state.clone(), st)
    else {
        panic!("V3");
    };
    assert!(s1 < s0, "CAKE = token0, продажа — цена вниз");
    let map = l.entry.ticks.as_ref().unwrap();
    let mut liq = l0 as i128;
    let mut crossed = 0;
    if t1 < t0 {
        // Цена вниз: пересечён тик t (t1 < t ≤ t0) — ликвидность −= liquidityNet(t) (PancakeV3Pool: при zeroForOne знак
        // liquidityNet обратный).
        for (_, d) in map.ticks.range((t1 + 1)..=t0) {
            liq -= d.net;
            crossed += 1;
        }
    }
    println!("V3: тик {t0} → {t1}, пересечено инициализированных тиков {crossed}, L {l0} → {l1}");
    assert_eq!(
        liq, l1 as i128,
        "ликвидность после — по liquidityNet пересечённых тиков"
    );
    let at = tick_math::tick_at_sqrt_ratio(s1).unwrap();
    assert!(t1 == at || (tick_math::sqrt_ratio_at_tick(t1 + 1).unwrap() == s1 && t1 + 1 == at));
}

#[tokio::test]
async fn share_without_common_pool_unchanged() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let a = vec![leg(&q, CAKE_USDT_PCS, CAKE, USDT)];
    let b = vec![leg(&q, CAKE_USDT_UNI, CAKE, USDT)];
    let c = vec![
        leg(&q, CAKE_WBNB_PCS, CAKE, WBNB),
        leg(&q, WBNB_USDT_V2, WBNB, USDT),
    ];
    let qs = [
        Decimal::from(7000u32),
        Decimal::from(3000u32),
        Decimal::from(2000u32),
    ];
    let paths = graph_seq::eval(
        &q.ctx,
        &[(&a, qs[0]), (&b, qs[1]), (&c, qs[2])],
        Side::Sell,
        &t,
    )
    .unwrap();
    for (legs, (qty, got)) in [&a, &b, &c].into_iter().zip(qs.iter().zip(&paths)) {
        let alone = route_calc::eval(&q.ctx, legs, Side::Sell, *qty, Decimal::ONE).unwrap();
        assert_eq!(got.usdt, alone.usdt);
        assert_eq!(got.coins, alone.coins);
    }
}

#[tokio::test]
async fn shared_pool_without_after_state_refused() {
    let p = route_common::parts().await;
    let q = p.quoting();
    let t = zero();
    let e = algebra_entry();
    assert!(!pool_after::appliable(&e));
    let l = Leg {
        token_in: e.pool.token0,
        token_out: e.pool.token1,
        entry: e,
        usable: Usable {
            gas: 0,
            hook_pct: Decimal::ZERO,
            tax_pct: Decimal::ZERO,
        },
        dec_in: 18,
        dec_out: 18,
    };
    let one = std::slice::from_ref(&l);
    let err = graph_seq::eval(
        &q.ctx,
        &[(one, Decimal::ONE), (one, Decimal::ONE)],
        Side::Sell,
        &t,
    )
    .expect_err("две доли через Algebra — не считаются");
    println!("{}", err.message);
    assert!(err.message.contains("решение 223"));
    // Одна доля через тот же пул — считается как раньше (ограничение только на общий пул).
    assert!(graph_seq::shared(&[one]).is_empty());
}

/// Тик из второго слова globalState (int24 в слове ABI, дополнение до двух). Ошибок нет.
fn tick_of(gs: &str) -> i32 {
    let h = gs.trim_start_matches("0x");
    let w = U256::from_str_radix(&h[64..128], 16).unwrap();
    if w.bit(255) {
        -(((!w).wrapping_add(U256::from(1u8))).to::<u64>() as i64) as i32
    } else {
        w.to::<u64>() as i32
    }
}

/// Пул Algebra Integral (thena-v3 BTCB/WBNB) из записи 2026-11-06 (tests/data/algebra_integral_2026-11-06/probe.json).
fn algebra_entry() -> arb::venues::bsc_dex::pools::registry::Entry {
    use arb::core::types::{Pool, VenueId};
    use arb::venues::bsc_dex::pools::families;
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/data/algebra_integral_2026-11-06/probe.json"
    );
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let pool = &v["pools"][0];
    let word = |raw: &str, i: usize| {
        let h = raw.trim_start_matches("0x");
        U256::from_str_radix(&h[64 * i..64 * (i + 1)], 16).unwrap()
    };
    let gs = pool["calls"]["globalState"]["raw"].as_str().unwrap();
    let addr = |s: &str| s.parse::<Address>().unwrap();
    let f = families::make("algebra_integral", "algebra_integral").unwrap();
    let liq = word(pool["calls"]["liquidity"]["raw"].as_str().unwrap(), 0).to::<u128>();
    let fee = word(pool["calls"]["fee"]["raw"].as_str().unwrap(), 0).to::<u32>();
    arb::venues::bsc_dex::pools::registry::Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr(pool["pool"].as_str().unwrap())),
            token0: addr(pool["token_decimals"]["token0"]["addr"].as_str().unwrap()),
            token1: addr(pool["token_decimals"]["token1"]["addr"].as_str().unwrap()),
            fee_pips: None,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::Algebra {
                sqrt_price_x96: word(gs, 0),
                tick: tick_of(gs),
                liquidity: liq,
                fee,
            },
            block: v["block"].as_u64().unwrap_or(0),
            received_at: chrono::Utc::now(),
        },
        ticker: "BTCB".into(),
        coin: addr(pool["token_decimals"]["token0"]["addr"].as_str().unwrap()),
        usdt: addr(pool["token_decimals"]["token1"]["addr"].as_str().unwrap()),
        key: None,
        ticks: None,
        bins: None,
        synced_block: 0,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}
