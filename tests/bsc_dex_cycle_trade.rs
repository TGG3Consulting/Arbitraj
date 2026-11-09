//! Файл: tests/bsc_dex_cycle_trade.rs
//! Что делает: исполнение цикла DEX ↔ DEX (решение 124) на РЕАЛЬНОЙ записи пулов CAKE 2026-10-29 (tests/route_common,
//! блок 130521896) — без сети и без моков: (1) ноги цикла по сигналу ядра DexCycle берутся из реестра по ссылкам пулов
//! (quote/cycle_legs.rs), разорванный / незамкнутый цикл, чужой пул и нативный BNB — отказ; (2) вызов исполнителя
//! ArbExecutor для цикла: tokenIn = tokenOut = базовая, вход — весь у кошелька, все доли 10000 bps, minOut — граница
//! ядра (contracts/ArbExecutor.sol: при tokenIn = tokenOut прямой вход выключен, received = весь выход); (3) выбор
//! формы: исполнитель → бандл (все каналы бандловые) → отказ с причинами обеих; (4) граница ядра у бандла — на
//! последнем хопе последнего участка; (5) курсы по пулам записи; (6) порог чистой прибыли после симуляции; (7) запрос
//! свопа «базовая → базовая» в формате ядра; (8) память причин ядра (core/cycle_state.rs).
//! Факт сделки по живой квитанции — тест с #[ignore] (записи нет, что записать — tests/README.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{quote::{cycle_legs, cycle_cost}, exec::{cycle_steps, cycle_gate,
//! cycle_bundle, executor}}, arb::core::cycle_state, tests/route_common.

mod route_common;

use alloy_primitives::{Address, U256, address};
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{CycleTradeRequest, DexCycle, DexCycleHop, PoolRef, Side, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::exec::bundle::BundleTx;
use arb::venues::bsc_dex::exec::cycle_gate::{self, Form};
use arb::venues::bsc_dex::exec::executor::{abi::ArbExecutor, call};
use arb::venues::bsc_dex::exec::{cycle_bundle, cycle_steps};
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::quote::cycle_cost::{self, Rates};
use arb::venues::bsc_dex::quote::{cycle, cycle_legs, cycle_opt};

use alloy::sol_types::SolCall;
use route_common::{CAKE, CAKE_USDT_PCS, CAKE_USDT_UNI, CAKE_WBNB_PCS, USDT, WBNB, WBNB_USDT_V2};

const EXECUTOR: Address = address!("0x00000000000000000000000000000000000000ee");
const WALLET: Address = address!("0x00000000000000000000000000000000000000aa");

/// n × 10^18.
fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(1_000_000_000_000_000_000u128)
}

/// Настройка площадки bsc_dex.
async fn set(p: &Parts, name: &str, v: serde_json::Value) {
    let s = Scope::Venue("bsc_dex".into());
    p.ctx.settings.set(name, s, Some(v), "тест").await.unwrap();
}

/// Окружение записи + срок, допуск, разрешения, отдельный Universal Router Uniswap (два участка у бандла).
async fn parts() -> Parts {
    let p = route_common::parts().await;
    for (n, v) in [
        ("exec.dex_slippage_pct", json!("0.5")),
        ("dex.deadline_s", json!(60)),
        ("dex.approve_unlimited", json!(false)),
        ("dex.permit2_expiration_s", json!(3600)),
        ("dex.native_wrapped", json!(WBNB.to_string())),
    ] {
        set(&p, n, v).await;
    }
    let uni = Scope::Dex("uniswap_v3".into());
    for n in ["dex.universal_router", "dex.permit2"] {
        let a = json!("0x00000000000000000000000000000000000000bb");
        p.ctx
            .settings
            .set(n, uni.clone(), Some(a), "тест")
            .await
            .unwrap();
    }
    p
}

/// Двухшаговый цикл USDT → CAKE (PancakeSwap V3) → USDT (Uniswap V3) — сигнал ядра, как его отдаёт dex_cycles.
fn signal(p: &Parts) -> DexCycle {
    let q = p.quoting();
    let hop = |i: u32, pool: Address, tin: Address, tout: Address| {
        let e = q.reg.get(&PoolRef::Contract(pool)).expect("пул в реестре");
        DexCycleHop {
            index: i,
            family: e.family.id.to_string(),
            kind: e.pool.kind,
            pool: e.pool.reference.clone(),
            token_in: tin,
            token_out: tout,
            fee_pct: Decimal::ZERO,
            hook_pct: Decimal::ZERO,
            gas_units: 150_000,
        }
    };
    DexCycle {
        venue: VenueId("bsc_dex".into()),
        base: USDT,
        base_ticker: "USDT".into(),
        input: Decimal::new(100, 0),
        output: Decimal::new(100, 0),
        gross_profit: Decimal::ZERO,
        gas_units: 300_000,
        gas_bnb: Decimal::ZERO,
        tip_bnb: Decimal::ZERO,
        bnb_cost: Decimal::ZERO,
        bnb_cost_in_base: None,
        net_profit: None,
        hops: vec![
            hop(1, CAKE_USDT_PCS, USDT, CAKE),
            hop(2, CAKE_USDT_UNI, CAKE, USDT),
        ],
        executable: true,
        exec_note: None,
        block: 130_521_896,
    }
}

#[tokio::test]
async fn legs_follow_signal_hops_and_reject_broken_cycles() {
    let p = parts().await;
    let q = p.quoting();
    let c = signal(&p);
    let legs = cycle_legs::now(&q, &c)
        .await
        .expect("ноги цикла по реестру");
    assert_eq!(legs.len(), 2);
    assert_eq!(
        legs[0].entry.pool.reference,
        PoolRef::Contract(CAKE_USDT_PCS)
    );
    assert_eq!((legs[1].token_in, legs[1].token_out), (CAKE, USDT));

    let mut open = c.clone();
    open.hops.pop();
    let e = cycle_legs::now(&q, &open)
        .await
        .err()
        .expect("незамкнутый цикл");
    assert_eq!(e.code, ErrorCode::NoRoute);

    let mut alien = c.clone();
    alien.hops[1].pool = PoolRef::Contract(EXECUTOR);
    let e = cycle_legs::now(&q, &alien)
        .await
        .err()
        .expect("пула нет в реестре");
    assert!(e.message.contains("не в реестре"), "{}", e.message);

    let mut native = c.clone();
    native.hops[0].token_out = Address::ZERO;
    native.hops[1].token_in = Address::ZERO;
    let e = cycle_legs::now(&q, &native)
        .await
        .err()
        .expect("нативный BNB внутри цикла");
    assert!(e.message.contains("нативный BNB"), "{}", e.message);
}

#[tokio::test]
async fn executor_call_for_cycle_has_same_token_in_and_out() {
    let p = parts().await;
    let q = p.quoting();
    let legs = cycle_legs::now(&q, &signal(&p)).await.unwrap();
    let input = Decimal::new(100, 0);
    let min_out = Decimal::new(1005, 1);
    let (steps, expected) = cycle_steps::exec_steps(&q.ctx, &legs, input, min_out).unwrap();
    assert_eq!(expected, cycle::eval(&q.ctx, &legs, input).unwrap().usdt);
    let c = call::build(&q.ctx, &[steps], EXECUTOR, 1_000_000).expect("вызов execute");
    assert_eq!(
        (c.token_in, c.token_out),
        (USDT, USDT),
        "цикл: вход = выход"
    );
    assert_eq!(c.amount_in, e18(100));
    assert_eq!(
        c.min_out,
        U256::from(1005u64) * U256::from(10u64).pow(U256::from(17u64))
    );
    let view: Vec<(Address, Address, u16)> = c
        .steps
        .iter()
        .map(|s| (s.tokenIn, s.tokenOut, s.shareBps))
        .collect();
    assert_eq!(view, vec![(USDT, CAKE, 10_000), (CAKE, USDT, 10_000)]);
    let back = ArbExecutor::executeCall::abi_decode(&c.data()).unwrap();
    assert_eq!(
        (back.tokenIn, back.tokenOut, back.minOut),
        (USDT, USDT, c.min_out)
    );
}

#[tokio::test]
async fn form_prefers_executor_then_bundle_then_refuses() {
    let p = parts().await;
    let q = p.quoting();
    let legs = cycle_legs::now(&q, &signal(&p)).await.unwrap();
    let e = cycle_gate::form(&q.ctx, &legs).expect_err("нет исполнителя и каналов");
    assert_eq!(e.code, ErrorCode::NoRoute);
    assert!(
        e.message.contains("не развёрнут исполнитель"),
        "{}",
        e.message
    );
    assert!(e.message.contains("бандл"), "{}", e.message);

    route_common::channel(&p, "club48", "bundle").await;
    assert_eq!(cycle_gate::form(&q.ctx, &legs).unwrap(), Form::Bundle);

    route_common::executor(&p, &EXECUTOR.to_string()).await;
    assert_eq!(
        cycle_gate::form(&q.ctx, &legs).unwrap(),
        Form::Executor(EXECUTOR)
    );
}

#[tokio::test]
async fn bundle_floor_sits_on_last_hop_of_last_segment() {
    let p = parts().await;
    let q = p.quoting();
    let legs = cycle_legs::now(&q, &signal(&p)).await.unwrap();
    let input = Decimal::new(100, 0);
    let plain = cycle_bundle::build(&q.ctx, &legs, input, WALLET, 7, 1_000_000, true)
        .await
        .unwrap();
    let floor = e18(1000);
    let at = (WALLET, 7, 1_000_000, true);
    let a = cycle_bundle::build_floor(&q.ctx, &legs, (input, floor), at)
        .await
        .unwrap();
    let mins = |b: &cycle_bundle::CycleAssembly| -> Vec<Vec<U256>> {
        b.bundle
            .txs
            .iter()
            .filter_map(|t| match t {
                BundleTx::Swap { legs_min, .. } => Some(legs_min.clone()),
                _ => None,
            })
            .collect()
    };
    let (m0, m1) = (mins(&plain), mins(&a));
    assert_eq!(m1.len(), 2, "два участка (два роутера)");
    assert_eq!(m1[0], m0[0], "первый участок — без границы");
    assert_eq!(
        *m1[1].last().unwrap(),
        floor,
        "граница ядра — последнему хопу"
    );
}

#[tokio::test]
async fn rates_come_from_recorded_pools() {
    let p = parts().await;
    let q = p.quoting();
    let r = cycle_cost::rates(&q, USDT).await.expect("курсы USDT");
    assert_eq!(r.base_usdt, Decimal::ONE);
    assert!(r.bnb_usdt > Decimal::ZERO);
    assert_eq!(r.bnb_in_base, r.bnb_usdt);
    let w = cycle_cost::rates(&q, WBNB).await.expect("курсы WBNB");
    assert_eq!((w.bnb_in_base, w.base_usdt), (Decimal::ONE, w.bnb_usdt));
}

#[tokio::test]
async fn net_after_simulation_must_clear_threshold() {
    let p = parts().await;
    let r = Rates {
        base_usdt: Decimal::ONE,
        bnb_usdt: Decimal::new(600, 0),
        bnb_in_base: Decimal::new(600, 0),
    };
    let gwei = U256::from(1_000_000_000u64);
    let io = (Decimal::new(101, 0), Decimal::new(100, 0));
    let e = cycle_steps::net_check(&p.ctx, io, (300_000, gwei), &r)
        .await
        .expect_err("порог не задан");
    assert_eq!(e.code, ErrorCode::NotConfigured);
    set(&p, "dex_dex.min_profit_usd", json!("0.5")).await;
    // Газ 300 000 × 1 gwei = 0,0003 BNB × 600 = 0,18 USDT; чистая 1 − 0,18 = 0,82.
    let net = cycle_steps::net_check(&p.ctx, io, (300_000, gwei), &r)
        .await
        .unwrap();
    assert_eq!(net, Decimal::new(82, 2));
    set(&p, "dex_dex.min_profit_usd", json!("1")).await;
    let e = cycle_steps::net_check(&p.ctx, io, (300_000, gwei), &r)
        .await
        .expect_err("ниже порога");
    assert_eq!(e.code, ErrorCode::QuoteMismatch);
}

#[tokio::test]
async fn trade_request_is_base_for_base_at_boundary() {
    let p = parts().await;
    let req = CycleTradeRequest {
        venue: VenueId("bsc_dex".into()),
        deal_id: "t".into(),
        cycle: signal(&p),
        input: Decimal::new(200, 0),
        min_out: Decimal::new(201, 0),
    };
    let t = cycle_steps::trade_request(&req);
    assert_eq!((t.side, t.qty), (Side::Sell, Decimal::new(200, 0)));
    assert_eq!(t.asset.bsc_contract, Some(USDT));
    assert_eq!(t.worst_price, Decimal::new(201, 0) / Decimal::new(200, 0));
}

#[tokio::test]
async fn core_remembers_reason_once_per_cycle() {
    let p = parts().await;
    let st = arb::core::cycle_state::CycleState::default();
    let c = signal(&p);
    let k = arb::core::cycle_state::key(&c);
    let mut other = c.clone();
    other.hops.reverse();
    assert_ne!(
        k,
        arb::core::cycle_state::key(&other),
        "обход в другую сторону — свой ключ"
    );
    assert!(st.fresh(&k, "нет бюджета"));
    assert!(
        !st.fresh(&k, "нет бюджета"),
        "та же причина — не повторяется"
    );
    assert!(st.fresh(&k, "ниже порога"));
    st.retain("bsc_dex", &Default::default());
    assert!(
        st.fresh(&k, "ниже порога"),
        "цикл ушёл из сигналов — память стёрта"
    );
}

/// Факт цикла по живой квитанции (решение 124): вход = Transfer базовой из кошелька, выход = Transfer базовой на
/// кошелёк. Записи нет — первая живая сделка цикла (что записать — tests/README.md, «Цикл DEX ↔ DEX: квитанция»).
#[test]
#[ignore = "нет записи квитанции живого цикла — tests/README.md"]
fn fact_from_recorded_cycle_receipt() {
    let path = "tests/data/cycle_exec_2026-11/receipt.json";
    let raw = std::fs::read_to_string(path).expect("запись квитанции цикла");
    let r: arb::venues::bsc_dex::send::receipt::Receipt = serde_json::from_str(&raw).unwrap();
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/cycle_exec_2026-11/meta.json").unwrap(),
    )
    .unwrap();
    let wallet: Address = meta["wallet"].as_str().unwrap().parse().unwrap();
    let base: Address = meta["base"].as_str().unwrap().parse().unwrap();
    let spent = arb::venues::bsc_dex::exec::fill::moved(&r, base, Some(wallet), None);
    let got = arb::venues::bsc_dex::exec::fill::moved(&r, base, None, Some(wallet));
    assert_eq!(spent.to_string(), meta["input_units"].as_str().unwrap());
    assert_eq!(got.to_string(), meta["output_units"].as_str().unwrap());
}

/// Цикл с базовой WBNB: WBNB → USDT (PancakeSwap V2) → CAKE (PancakeSwap V3) → WBNB (PancakeSwap V3).
fn wbnb_signal(p: &Parts) -> DexCycle {
    let mut c = signal(p);
    let q = p.quoting();
    let hop = |i: u32, pool: Address, tin: Address, tout: Address| {
        let e = q.reg.get(&PoolRef::Contract(pool)).expect("пул в реестре");
        DexCycleHop {
            index: i,
            family: e.family.id.to_string(),
            kind: e.pool.kind,
            pool: e.pool.reference.clone(),
            token_in: tin,
            token_out: tout,
            fee_pct: Decimal::ZERO,
            hook_pct: Decimal::ZERO,
            gas_units: 150_000,
        }
    };
    c.base = WBNB;
    c.base_ticker = "WBNB".into();
    c.input = Decimal::ONE;
    c.hops = vec![
        hop(1, WBNB_USDT_V2, WBNB, USDT),
        hop(2, CAKE_USDT_PCS, USDT, CAKE),
        hop(3, CAKE_WBNB_PCS, CAKE, WBNB),
    ];
    c
}

#[tokio::test]
async fn wbnb_base_cycle_end_to_end_without_network() {
    let p = parts().await;
    route_common::executor(&p, &EXECUTOR.to_string()).await;
    let q = p.quoting();
    let c = wbnb_signal(&p);
    let legs = cycle_legs::now(&q, &c).await.expect("ноги цикла WBNB");
    assert_eq!(
        cycle_gate::form(&q.ctx, &legs).unwrap(),
        Form::Executor(EXECUTOR)
    );
    let r = cycle_cost::rates(&q, WBNB).await.unwrap();
    assert_eq!(r.bnb_in_base, Decimal::ONE, "газ BNB = базовая WBNB 1:1");
    let input = Decimal::ONE;
    let out = cycle::eval(&q.ctx, &legs, input).unwrap().usdt;
    let (steps, expected) = cycle_steps::exec_steps(&q.ctx, &legs, input, out).unwrap();
    assert_eq!(expected, out);
    let call = call::build(&q.ctx, &[steps], EXECUTOR, 1_000_000).unwrap();
    assert_eq!((call.token_in, call.token_out), (WBNB, WBNB));
    // Порог в USDT: 1 USDT = 1 / курс WBNB в базовой; выход, равный входу, его не проходит.
    set(&p, "dex_dex.min_profit_usd", json!("1")).await;
    let e = cycle_steps::net_check(&p.ctx, (input, input), (0, U256::ZERO), &r)
        .await
        .unwrap_err();
    assert!(e.message.contains("USDT"), "{}", e.message);
    let gain = Decimal::new(2, 0) / r.base_usdt;
    let net = cycle_steps::net_check(&p.ctx, (input + gain, input), (0, U256::ZERO), &r)
        .await
        .unwrap();
    assert_eq!(net, gain);
}

#[tokio::test]
async fn signal_input_is_optimum_within_probe_bound() {
    let p = parts().await;
    let q = p.quoting();
    let legs = cycle_legs::now(&q, &signal(&p)).await.unwrap();
    let cap = Decimal::new(1000, 0);
    let fixed = cycle::eval(&q.ctx, &legs, cap).unwrap();
    // size.refine_steps не задана — как раньше: сигнал на самой границе.
    let same = cycle_opt::best(&q.ctx, &legs, cap).unwrap();
    assert_eq!(same.coins, fixed.coins);
    p.ctx
        .settings
        .set("size.refine_steps", Scope::Global, Some(json!(40)), "тест")
        .await
        .unwrap();
    let best = cycle_opt::best(&q.ctx, &legs, cap).unwrap();
    assert!(best.coins <= cap && best.coins > Decimal::ZERO);
    let gain = |x: &arb::venues::bsc_dex::quote::path::Path| x.usdt - x.coins;
    assert!(gain(&best) >= gain(&fixed), "оптимум не хуже границы");
    for x in [
        Decimal::new(1, 0),
        Decimal::new(100, 0),
        Decimal::new(500, 0),
    ] {
        let pt = cycle::eval(&q.ctx, &legs, x).unwrap();
        assert!(gain(&best) >= gain(&pt), "оптимум не хуже входа {x}");
    }
}
