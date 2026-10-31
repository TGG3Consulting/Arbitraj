//! Файл: tests/bsc_dex_cycle.rs
//! Что делает: проверяет поиск замкнутых циклов DEX ↔ DEX (quote/route_paths::enumerate_cycles, решение 85) и расчёт их
//! сигнала (quote/cycle.rs) на РЕАЛЬНОЙ записи маршрута CAKE 2026-10-29 (tests/data/routes_2026-10-29, рекордер
//! record_route.js) — состояния пулов настоящие, на одном блоке 130521896; шесть пулов: CAKE-USDT ×2 (Pancake V3 +
//! Uniswap V3), CAKE-WBNB (Pancake V3), WBNB-USDT (V2), CAKE-BTCB (Uniswap V3), BTCB-WBNB (Pancake V3). Граф и
//! условные адреса — те же, что в tests/bsc_dex_route_paths.rs (запись адреса токенов не содержит; порядок сортировки
//! как у реальных адресов BSC, чтобы ориентация token0/token1 у V3 совпадала с записанным sqrtPriceX96).
//! Проверяется (на реальных записанных пулах, без моков): (1) из базовой USDT находятся циклы USDT → … → USDT —
//! двухшаговый (двумя разными пулами CAKE-USDT), трёх- и четырёхшаговые через WBNB и BTCB; оба обхода (прямой и
//! обратный) — отдельными циклами; ни один пул в цикле не используется дважды, внутренние токены не повторяются;
//! пределы длины и ширины работают. (2) Расчёт цикла: вход = проба, выход — реальной математикой пулов (комиссии,
//! проскальзывание уже внутри), валовая прибыль = выход − вход; газ (BNB) → базовую монету по курсу, чистая прибыль =
//! валовая − затраты. Для замкнутого round-trip по двум пулам одной пары прибыль ожидаемо отрицательна (комиссии двух
//! свопов) — это валидная проверка МЕХАНИКИ (решение 85: прибыль может быть отрицательной). (3) Исполнимость: все хопы
//! через tradable-пулы → исполним; хоп через не-tradable семейство → сигнал, но не исполним. (4) Маппинг сигнала в
//! read-тип ядра DexCycle (стык образца решения 74, quote/cycle_view.rs): поля переносятся один-в-один, добавляется
//! площадка; JSON-поверхность эндпоинта GET /api/venues/{id}/dex-cycles — та, что ждёт интерфейс (VenueId строкой,
//! адреса «0x…», Decimal строками, PoolRef::Contract как {Contract: «0x…»}).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{pools, quote}, core::types (DexCycle), abi_v2v3; настоящая SQLite во временной папке.

use std::collections::{BTreeMap, HashMap, HashSet};

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256, address};
use chrono::Utc;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

use arb::core::types::{Asset, Pool, PoolRef, PoolState, VenueId};
use arb::settings::scope::Scope;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::storage::raw_log::RawLog;
use arb::venues::bsc_dex::ctx::Ctx;
use arb::venues::bsc_dex::num;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::abi_v2v3::{
    IPancakeV3Pool, ITickLens, IUniswapV3Pool, IV2Pair, IV3Pool,
};
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::pools::graph;
use arb::venues::bsc_dex::pools::registry::Entry;
use arb::venues::bsc_dex::pools::ticks::{TickData, TickMap};
use arb::venues::bsc_dex::quote::cycle;
use arb::venues::bsc_dex::quote::route_calc::Leg;
use arb::venues::bsc_dex::quote::route_paths::{self, RoutePath};
use arb::venues::bsc_dex::quote::select::Usable;

// Узлы графа: порядок сортировки — как у реальных адресов BSC (CAKE < USDT < BTCB < WBNB), чтобы token0 < token1 у V3
// совпадало с записанным sqrtPriceX96. Сами адреса условные (запись их не содержит).
const CAKE: Address = address!("0x0e00000000000000000000000000000000000001");
const USDT: Address = address!("0x5500000000000000000000000000000000000002");
const BTCB: Address = address!("0x7100000000000000000000000000000000000003");
const WBNB: Address = address!("0xbb00000000000000000000000000000000000004");

/// Записанные ответы: имя вызова → байты результата.
fn load() -> HashMap<String, Vec<u8>> {
    let v: Value = serde_json::from_str(
        &std::fs::read_to_string("tests/data/routes_2026-10-29/responses.json").unwrap(),
    )
    .unwrap();
    let mut got = HashMap::new();
    for r in v["results"].as_array().unwrap() {
        if let Some(s) = r["response"]["result"].as_str() {
            got.insert(
                r["name"].as_str().unwrap().to_string(),
                alloy_primitives::hex::decode(s).unwrap(),
            );
        }
    }
    got
}

fn base_entry(
    kind: &str,
    fee: Option<u32>,
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let f = families::make(kind, kind).unwrap();
    let (t0, t1) = if coin < counter {
        (coin, counter)
    } else {
        (counter, coin)
    };
    Entry {
        family: f,
        pool: Pool {
            venue: VenueId("bsc_dex".into()),
            family: f.id.into(),
            kind: f.kind,
            reference: PoolRef::Contract(addr),
            token0: t0,
            token1: t1,
            fee_pips: fee,
            tick_spacing: None,
            bin_step: None,
            hooks: None,
            state: PoolState::V2 {
                reserve0: U256::ZERO,
                reserve1: U256::ZERO,
            },
            block: 1,
            received_at: Utc::now(),
        },
        ticker: "X".into(),
        coin,
        usdt: counter,
        key: None,
        ticks: None,
        bins: None,
        synced_block: 1,
        synced_log: None,
        valid: true,
        asleep: false,
    }
}

/// Пул V3 из записи (slot0 + liquidity + слова TickLens) как запись реестра. pancake — ABI PancakeSwap (slot0 шире).
#[allow(clippy::too_many_arguments)]
fn v3(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    pancake: bool,
    spacing: i32,
    fee: u32,
    words: (i16, i16),
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let (sqrt, tick) = if pancake {
        let s =
            IPancakeV3Pool::slot0Call::abi_decode_returns(&got[&format!("{tag} slot0")]).unwrap();
        (s.sqrtPriceX96.to::<U256>(), s.tick.as_i32())
    } else {
        let s =
            IUniswapV3Pool::slot0Call::abi_decode_returns(&got[&format!("{tag} slot0")]).unwrap();
        (s.sqrtPriceX96.to::<U256>(), s.tick.as_i32())
    };
    let liquidity =
        IV3Pool::liquidityCall::abi_decode_returns(&got[&format!("{tag} liquidity")]).unwrap();
    let mut ticks = BTreeMap::new();
    for k in words.0..=words.1 {
        for t in ITickLens::getPopulatedTicksInWordCall::abi_decode_returns(
            &got[&format!("{tag} TickLens word {k}")],
        )
        .unwrap()
        {
            ticks.insert(
                t.tick.as_i32(),
                TickData {
                    gross: t.liquidityGross,
                    net: t.liquidityNet,
                },
            );
        }
    }
    let kind = if pancake {
        "pancakeswap_v3"
    } else {
        "uniswap_v3"
    };
    let mut e = base_entry(kind, Some(fee), addr, coin, counter);
    e.pool.tick_spacing = Some(spacing);
    e.pool.state = PoolState::V3 {
        sqrt_price_x96: sqrt,
        liquidity,
        tick,
    };
    e.ticks = Some(TickMap {
        spacing,
        ticks,
        words,
    });
    e
}

/// Пул V2 из записи (getReserves). Комиссия 0,25% (fee_pips 2500).
fn v2(
    got: &HashMap<String, Vec<u8>>,
    tag: &str,
    addr: Address,
    coin: Address,
    counter: Address,
) -> Entry {
    let rs =
        IV2Pair::getReservesCall::abi_decode_returns(&got[&format!("{tag} getReserves")]).unwrap();
    let mut e = base_entry("pancakeswap_v2", Some(2500), addr, coin, counter);
    e.pool.state = PoolState::V2 {
        reserve0: U256::from(rs.reserve0),
        reserve1: U256::from(rs.reserve1),
    };
    e
}

async fn ctx() -> Ctx {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let uniq = format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    );
    let d = std::env::temp_dir().join(format!("arb-cycle-{uniq}"));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db, changes, journal.clone()).await.unwrap();
    let c = Ctx {
        raw: RawLog::start(d.join("raw"), settings.clone()),
        pace: Default::default(),
        failover: Default::default(),
        index: Default::default(),
        settings,
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        http: reqwest::Client::new(),
        journal,
    };
    let v = Scope::Venue("bsc_dex".into());
    for (n, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("dex.min_pool_liquidity_usd", json!(1)),
        ("dex.depth_band_pct", json!(5)),
        ("dex.depth_hysteresis_pct", json!(0)),
        ("dex.route_max_pools_per_edge", json!(5)),
        ("dex.route_max_hops", json!(3)),
        ("dex.route_max_paths", json!(50)),
    ] {
        c.settings
            .set(n, v.clone(), Some(val), "тест")
            .await
            .unwrap();
    }
    c
}

/// Граф из реальной записи (как в tests/bsc_dex_route_paths.rs — та же авторитетная топология).
async fn graph_and_ctx() -> (graph::Graph, Parts) {
    let got = load();
    let p = Parts::new(ctx().await);
    p.tokens.set(&[Asset {
        ticker: "USDT".into(),
        bsc_contract: Some(USDT),
    }]);
    for e in [
        v3(
            &got,
            "CAKE-USDT pcs_v3",
            true,
            50,
            2500,
            (-3, 3),
            address!("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1"),
            CAKE,
            USDT,
        ),
        v3(
            &got,
            "CAKE-USDT uni_v3",
            false,
            60,
            3000,
            (-3, 3),
            address!("0xfe4fe5b4575c036ac6d5cccfe13660020270e27a"),
            CAKE,
            USDT,
        ),
        v3(
            &got,
            "CAKE-WBNB pcs_v3",
            true,
            50,
            2500,
            (-8, -2),
            address!("0x133b3d95bad5405d14d53473671200e9342896bf"),
            CAKE,
            WBNB,
        ),
        v2(
            &got,
            "WBNB-USDT v2",
            address!("0x16b9a82891338f9ba80e2d6970fdda79d1eb0dae"),
            WBNB,
            USDT,
        ),
        v3(
            &got,
            "CAKE-BTCB uni_v3",
            false,
            200,
            10000,
            (-3, 3),
            address!("0x8e8410f4878079fc03a287a7ded4a65afb87cc06"),
            BTCB,
            CAKE,
        ),
        v3(
            &got,
            "BTCB-WBNB pcs_v3",
            true,
            10,
            500,
            (15, 21),
            address!("0x6bbc40579ad1bbd243895ca0acb086bb6300d636"),
            BTCB,
            WBNB,
        ),
    ] {
        p.registry.put(e);
    }
    let g = graph::build(&p.ctx, &p.registry, &p.tokens, 18).expect("граф строится");
    (g, p)
}

/// Последовательность токенов цикла: вход первого хопа и выходы по порядку.
fn seq(path: &RoutePath) -> Vec<Address> {
    let mut v = vec![path.steps[0].token_in];
    v.extend(path.steps.iter().map(|s| s.token_out));
    v
}

/// Нога по шагу цикла с заданным допуском (газ, без крючка) и decimals 18 — как в tests/bsc_dex_route_calc.rs
/// (расчёт строим из записанных пулов, допуск задаём явно, не дёргая проверки квотера).
fn leg_of(step: &route_paths::Step, gas: u64) -> Leg {
    Leg {
        entry: step.edge.entry.clone(),
        usable: Usable {
            gas,
            hook_pct: Decimal::ZERO,
        },
        token_in: step.token_in,
        token_out: step.token_out,
        dec_in: 18,
        dec_out: 18,
    }
}

#[tokio::test]
async fn finds_cycles_from_base() {
    let (g, _p) = graph_and_ctx().await;
    assert_eq!(g.len(), 6, "все шесть пулов записи — в графе");

    // Длина 2: только USDT → CAKE → USDT двумя разными пулами пары (оба обхода A→B и B→A).
    let c2 = route_paths::enumerate_cycles(&g, USDT, 2, 100);
    assert_eq!(
        c2.len(),
        2,
        "два двухшаговых цикла (два обхода пары пулов CAKE-USDT)"
    );
    for c in &c2 {
        assert_eq!(c.hops(), 2);
        assert_eq!(seq(c), vec![USDT, CAKE, USDT]);
        // Разные пулы на входе и выходе (один пул в цикле не дважды).
        assert_ne!(
            c.steps[0].edge.entry.pool.reference,
            c.steps[1].edge.entry.pool.reference
        );
    }

    // Длина 3: прибавляются циклы через WBNB (оба обхода, по два варианта первого пула CAKE-USDT) — всего 4.
    let c3 = route_paths::enumerate_cycles(&g, USDT, 3, 100);
    assert_eq!(c3.len(), 6, "2 двухшаговых + 4 трёхшаговых через WBNB");
    assert_eq!(c3.iter().filter(|c| c.hops() == 3).count(), 4);
    assert!(
        c3.iter().any(|c| seq(c) == vec![USDT, CAKE, WBNB, USDT]),
        "есть цикл USDT → CAKE → WBNB → USDT"
    );
    assert!(
        c3.iter().any(|c| seq(c) == vec![USDT, WBNB, CAKE, USDT]),
        "есть обратный обход USDT → WBNB → CAKE → USDT"
    );

    // Длина 4: прибавляются циклы через BTCB и WBNB (оба обхода) — всего 10.
    let c4 = route_paths::enumerate_cycles(&g, USDT, 4, 100);
    assert_eq!(c4.len(), 10, "2 + 4 + 4 (через BTCB-WBNB)");
    assert!(
        c4.iter()
            .any(|c| seq(c) == vec![USDT, CAKE, BTCB, WBNB, USDT])
    );

    // Общие свойства всех циклов: старт = финиш = база, внутренние токены и пулы не повторяются, длина в пределах.
    for c in &c4 {
        let s = seq(c);
        assert_eq!(*s.first().unwrap(), USDT);
        assert_eq!(*s.last().unwrap(), USDT);
        assert!(c.hops() >= 2 && c.hops() <= 4);
        let inner: Vec<Address> = s[1..s.len() - 1].to_vec();
        let uniq: HashSet<_> = inner.iter().collect();
        assert_eq!(
            uniq.len(),
            inner.len(),
            "внутренние токены цикла не повторяются"
        );
        let pools: HashSet<_> = c
            .steps
            .iter()
            .map(|x| x.edge.entry.pool.reference.clone())
            .collect();
        assert_eq!(pools.len(), c.steps.len(), "один пул в цикле — не дважды");
    }

    // Ширина: не больше max_cycles.
    assert_eq!(route_paths::enumerate_cycles(&g, USDT, 4, 3).len(), 3);
    // Меньше 2 хопов цикла не бывает.
    assert!(route_paths::enumerate_cycles(&g, USDT, 1, 100).is_empty());
}

#[tokio::test]
async fn evaluates_cycle_profit_model_85() {
    let (g, p) = graph_and_ctx().await;
    // Двухшаговый цикл двумя пулами V3 (без V2 — не нужен dex.v2_fee_pct): USDT → CAKE → USDT.
    let cycles = route_paths::enumerate_cycles(&g, USDT, 2, 100);
    let cyc = cycles.first().expect("есть двухшаговый цикл");

    let gas_per_hop = 150_000u64;
    let legs: Vec<Leg> = cyc.steps.iter().map(|s| leg_of(s, gas_per_hop)).collect();
    let input = Decimal::new(100, 0); // 100 USDT пробного входа

    let path = cycle::eval(&p.ctx, &legs, input).expect("цикл считается на записанных пулах");
    assert_eq!(path.coins, input, "вход цикла = проба");
    assert!(
        path.usdt > Decimal::ZERO,
        "выход цикла посчитан реальной математикой пулов"
    );
    assert_eq!(path.hops.len(), 2);
    // Round-trip по двум пулам одной пары: валовая прибыль = выход − вход (комиссии/проскальзывание уже внутри).
    let gross = path.usdt - input;
    assert!(
        gross < Decimal::ZERO,
        "замкнутый round-trip по двум пулам пары теряет на комиссиях двух свопов (валидная механика, решение 85): выход {} < вход {}",
        path.usdt,
        input
    );

    // Сигнал: газ (BNB) → базовую USDT по курсу, чистая прибыль = валовая − затраты.
    let gas_wei = U256::from(1_000_000_000u64); // 1 gwei
    let tip = Decimal::ZERO;
    let rate = Decimal::new(600, 0); // 600 USDT за 1 BNB (базовая = USDT)
    let sig = cycle::build_signal(&p.ctx, USDT, "USDT", &path, &legs, gas_wei, tip, Some(rate))
        .expect("сигнал собирается");

    assert_eq!(sig.input, input);
    assert_eq!(sig.output, path.usdt);
    assert_eq!(sig.gross_profit, gross);
    assert_eq!(sig.gas_units, 2 * gas_per_hop);
    // Газ в BNB и его перевод в базовую — ровно модель расчёта.
    let gas_bnb =
        num::to_decimal(gas_wei * U256::from(2 * gas_per_hop), num::BNB_DECIMALS).unwrap();
    assert_eq!(sig.gas_bnb, gas_bnb);
    assert_eq!(sig.bnb_cost, gas_bnb + tip);
    assert_eq!(sig.bnb_cost_in_base, Some((gas_bnb + tip) * rate));
    assert_eq!(sig.net_profit, Some(gross - (gas_bnb + tip) * rate));
    assert!(
        sig.net_profit.unwrap() < Decimal::ZERO,
        "с учётом газа прибыль тем более отрицательна"
    );

    // Оба хопа через tradable-семейства (pancakeswap_v3 / uniswap_v3) → цикл исполним как есть.
    assert!(sig.executable, "все хопы tradable → исполним");
    assert!(sig.exec_note.is_none());
    assert_eq!(sig.hops.len(), 2);
    assert!(
        sig.hops.iter().all(|h| h.fee_pct > Decimal::ZERO),
        "комиссия пула на каждом шаге"
    );

    // Без курса BNB → базовая чистой прибыли нет, видна валовая (ничего не скрыто).
    let no_rate = cycle::build_signal(&p.ctx, USDT, "USDT", &path, &legs, gas_wei, tip, None)
        .expect("сигнал собирается и без курса");
    assert_eq!(no_rate.net_profit, None);
    assert_eq!(no_rate.bnb_cost_in_base, None);
    assert_eq!(no_rate.gross_profit, gross);
}

#[tokio::test]
async fn executability_requires_tradable_hops() {
    let (g, p) = graph_and_ctx().await;
    let cycles = route_paths::enumerate_cycles(&g, USDT, 2, 100);
    let cyc = cycles.first().unwrap();
    let mut legs: Vec<Leg> = cyc.steps.iter().map(|s| leg_of(s, 150_000)).collect();

    // Исходно — все хопы tradable.
    assert!(cycle::executable(&legs).is_ok());

    // Подменяем семейство одного хопа на не-tradable (THENA Fusion — в графе, но авто-сделкой не исполняется):
    // цикл показывается сигналом, но исполнимым НЕ считается (решение 84/85).
    legs[0].entry.family = families::make("thena_fusion", "thena_fusion").unwrap();
    assert!(!legs[0].entry.family.tradable, "thena_fusion — не tradable");
    let err = cycle::executable(&legs).expect_err("хоп через не-tradable → не исполним");
    assert!(
        err.contains("thena_fusion"),
        "в причине — семейство без исполнения: {err}"
    );

    let input = Decimal::new(100, 0);
    let path = cycle::eval(&p.ctx, &legs, input).expect("расчёт не зависит от исполнимости");
    let sig = cycle::build_signal(
        &p.ctx,
        USDT,
        "USDT",
        &path,
        &legs,
        U256::from(1u64),
        Decimal::ZERO,
        Some(Decimal::ONE),
    )
    .unwrap();
    assert!(!sig.executable, "не исполним как есть");
    assert!(
        sig.exec_note.is_some(),
        "есть причина неисполнимости для карточки"
    );
}

/// Маппинг сигнала цикла (адаптер) в read-тип ядра DexCycle (стык образца решения 74, quote/cycle_view.rs): все поля
/// переносятся один-в-один, добавляется площадка; JSON-поверхность эндпоинта GET /api/venues/{id}/dex-cycles — та, что
/// ждёт интерфейс (VenueId строкой, адреса «0x…», Decimal строками, PoolRef как {Contract: «0x…»}). Данные — реальная
/// запись routes_2026-10-29, как в остальных тестах файла (без моков).
#[tokio::test]
async fn maps_cycle_signal_to_core_dex_cycle_74() {
    use arb::core::types::DexCycle;
    use arb::venues::bsc_dex::quote::cycle_view;

    let (g, p) = graph_and_ctx().await;
    let cycles = route_paths::enumerate_cycles(&g, USDT, 2, 100);
    let cyc = cycles.first().expect("есть двухшаговый цикл");
    let legs: Vec<Leg> = cyc.steps.iter().map(|s| leg_of(s, 150_000)).collect();
    let input = Decimal::new(100, 0);
    let path = cycle::eval(&p.ctx, &legs, input).unwrap();
    let gas_wei = U256::from(1_000_000_000u64);
    let rate = Decimal::new(600, 0);
    let sig = cycle::build_signal(
        &p.ctx,
        USDT,
        "USDT",
        &path,
        &legs,
        gas_wei,
        Decimal::ZERO,
        Some(rate),
    )
    .expect("сигнал собирается");

    let venue = VenueId("bsc_dex".into());
    let dc: DexCycle = cycle_view::to_dex_cycle(&venue, &sig);

    // Площадка подставлена; остальные поля — один-в-один из сигнала.
    assert_eq!(dc.venue, venue);
    assert_eq!(dc.base, sig.base);
    assert_eq!(dc.base_ticker, sig.base_ticker);
    assert_eq!(dc.input, sig.input);
    assert_eq!(dc.output, sig.output);
    assert_eq!(dc.gross_profit, sig.gross_profit);
    assert_eq!(dc.gas_units, sig.gas_units);
    assert_eq!(dc.gas_bnb, sig.gas_bnb);
    assert_eq!(dc.tip_bnb, sig.tip_bnb);
    assert_eq!(dc.bnb_cost, sig.bnb_cost);
    assert_eq!(dc.bnb_cost_in_base, sig.bnb_cost_in_base);
    assert_eq!(dc.net_profit, sig.net_profit);
    assert_eq!(dc.executable, sig.executable);
    assert_eq!(dc.exec_note, sig.exec_note);
    assert_eq!(dc.block, sig.block);
    assert_eq!(dc.hops.len(), sig.hops.len());
    for (a, b) in dc.hops.iter().zip(&sig.hops) {
        assert_eq!(a.index, b.index);
        assert_eq!(a.family, b.family);
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.pool, b.pool);
        assert_eq!(a.token_in, b.token_in);
        assert_eq!(a.token_out, b.token_out);
        assert_eq!(a.fee_pct, b.fee_pct);
        assert_eq!(a.hook_pct, b.hook_pct);
        assert_eq!(a.gas_units, b.gas_units);
    }

    // JSON-поверхность эндпоинта (что получает интерфейс) — ровно те формы, на которые рассчитан DexCycles.svelte.
    let j = serde_json::to_value(&dc).unwrap();
    assert_eq!(j["venue"], json!("bsc_dex"), "VenueId — простая строка");
    assert_eq!(j["base_ticker"], json!("USDT"));
    assert!(
        j["base"].as_str().unwrap().starts_with("0x"),
        "адрес базовой монеты — строка 0x…"
    );
    assert!(j["net_profit"].is_string(), "Decimal в JSON — строкой");
    assert_eq!(j["hops"].as_array().unwrap().len(), 2);
    assert!(
        j["hops"][0]["pool"]["Contract"].is_string(),
        "PoolRef::Contract → {{\"Contract\": \"0x…\"}}"
    );
    assert!(
        j["executable"].as_bool().unwrap(),
        "оба хопа tradable → исполним"
    );
}
