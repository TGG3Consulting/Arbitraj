//! Файл: tests/bsc_dex_ladder_counter_306.rs
//! Что делает: решение 306 — суммы лесенки цен пула (dex.pmm_ladder_usd) во встречном токене пары не с USDT
//! (pools/ladder_counter.rs) на РЕАЛЬНОЙ записи routes_2026-10-29 (блок 130521896; пул WBNB — USDT V2 0x16b9…0dae с
//! записанными резервами — tests/route_common): (1) dex.pmm_ladder_counter не задана / «нет» — пара с WBNB без
//! лесенки, пара с USDT — суммы 1 в 1 как раньше (решение 297); (2) «да» — каждая ступень = rate::from_usdt той же
//! суммы USDT в WBNB (тот же механизм, что пересчёт пробы), по возрастанию; (3) у встречного без пула «X — USDT» в
//! реестре (BTCB записи) — лесенки нет; (4) пул «X — USDT» спит — курса нет; (5) iZiSwap / Biswap V3 без USDT в паре
//! перечитываются каждым блоком только при «да» и заданной dex.pmm_ladder_usd; (6, ревью) абсолютное значение
//! ступеней — независимо, формулой x·y=k с комиссией пары 0,25 % (fee_pips 2500 записи route_common) от резервов
//! getReserves из responses.json записи; (7, ревью) обрезка лесенки на ступени, на которую глубины пулов «X — USDT» не
//! хватает: X — CAKE записи (пулы CAKE — USDT PancakeSwap V3 и Uniswap V3 с прочитанными словами тиков), лесенка из
//! одной точки и пустая. Адреса токенов записи условные (как во всех тестах route_common), состояния пулов — настоящие.
//! Чтение лесенки квотером с узла для пары не с USDT и decimals ≠ 18 записью не покрыты (docs/TASKS.md, решение 306).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::pools::{ladder_counter, discover, families}, quote::rate, tests/route_common.

mod route_common;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, U256};
use serde_json::{Value, json};

use arb::core::types::PoolRef;
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::parts::Parts;
use arb::venues::bsc_dex::pools::abi_v2v3::IV2Pair;
use arb::venues::bsc_dex::pools::discover::Found;
use arb::venues::bsc_dex::pools::{families, ladder_counter};
use arb::venues::bsc_dex::quote::rate;

use route_common::{BTCB, CAKE, USDT, WBNB, WBNB_USDT_V2};

/// id DEX вида Biswap V3 (область настроек «DEX»).
const DEX: &str = "biswap-v3-1";

async fn set(p: &Parts, name: &str, v: Option<Value>) {
    p.ctx
        .settings
        .set(name, Scope::Dex(DEX.into()), v, "тест")
        .await
        .unwrap();
}

/// Выход WBNB на вход `usdt_in` USDT в пару WBNB — USDT V2 записи — независимо от кода бота: резервы — сырой ответ
/// getReserves из tests/data/routes_2026-10-29/responses.json, формула getAmountOut пары Uniswap V2 / PancakeSwap V2
/// (amountIn·(10000 − 25)·reserveOut / (reserveIn·10000 + amountIn·(10000 − 25))), комиссия 0,25 % — fee_pips 2500
/// пула в route_common. USDT — token0 (адрес меньше, как у реальных токенов). Ошибок нет (паника теста).
fn v2_record_out(usdt_in: U256) -> U256 {
    let got = route_common::load("tests/data/routes_2026-10-29");
    let rs =
        IV2Pair::getReservesCall::abi_decode_returns(&got["WBNB-USDT v2 getReserves"]).unwrap();
    assert!(USDT < WBNB, "USDT — token0 пары");
    let (r_in, r_out) = (U256::from(rs.reserve0), U256::from(rs.reserve1));
    let fee_num = U256::from(10_000u32 - 25);
    let with_fee = usdt_in * fee_num;
    with_fee * r_out / (r_in * U256::from(10_000u32) + with_fee)
}

/// 10^18 (decimals токенов записи — 18, route_common parts_from).
fn e18(n: u64) -> U256 {
    U256::from(n) * U256::from(10u64).pow(U256::from(18u8))
}

#[tokio::test]
async fn ladder_in_counter_token_by_rate_306() {
    let p = route_common::parts().await;
    let (ctx, tokens, reg) = (&p.ctx, &p.tokens, &p.registry);
    set(
        &p,
        "dex.pmm_ladder_usd",
        Some(json!(["10", "50", "100", "800"])),
    )
    .await;

    // (1) Не задана — пара с WBNB без лесенки, пара с USDT — как раньше.
    let usdt = ladder_counter::amounts(ctx, tokens, reg, DEX, USDT)
        .await
        .unwrap();
    assert_eq!(usdt, vec![e18(10), e18(50), e18(100), e18(800)]);
    assert!(
        ladder_counter::amounts(ctx, tokens, reg, DEX, WBNB)
            .await
            .unwrap()
            .is_empty()
    );
    set(&p, ladder_counter::SETTING, Some(json!(false))).await;
    assert!(
        ladder_counter::amounts(ctx, tokens, reg, DEX, WBNB)
            .await
            .unwrap()
            .is_empty()
    );

    // (2) «Да» — ступени в WBNB = rate::from_usdt той же суммы, по возрастанию.
    set(&p, ladder_counter::SETTING, Some(json!(true))).await;
    let got = ladder_counter::amounts(ctx, tokens, reg, DEX, WBNB)
        .await
        .unwrap();
    assert_eq!(got.len(), 4, "все ступени посчитаны: {got:?}");
    for (x, usd) in got.iter().zip([10u64, 50, 100, 800]) {
        let want = rate::from_usdt(ctx, reg, USDT, WBNB, e18(usd)).unwrap();
        assert_eq!(*x, want, "ступень {usd} USDT");
        // Независимо: формула пары от сырых резервов записи (допуск — единица округления деления).
        let own = v2_record_out(e18(usd));
        let diff = if *x > own { *x - own } else { own - *x };
        assert!(
            diff <= U256::from(1u8),
            "ступень {usd} USDT: {x} против {own}"
        );
    }
    assert!(got.windows(2).all(|w| w[0] < w[1]));
    // С USDT — по-прежнему 1 в 1.
    assert_eq!(
        ladder_counter::amounts(ctx, tokens, reg, DEX, USDT)
            .await
            .unwrap(),
        usdt
    );

    // (3) Нет пула «BTCB — USDT» в записи — курса нет, лесенки нет.
    assert!(reg.by_pair(BTCB, USDT).is_empty());
    assert!(
        ladder_counter::amounts(ctx, tokens, reg, DEX, BTCB)
            .await
            .unwrap()
            .is_empty()
    );

    // (4) Пул «WBNB — USDT» спит — в курс не идёт.
    reg.update(&PoolRef::Contract(WBNB_USDT_V2), |e| e.asleep = true);
    assert!(
        ladder_counter::amounts(ctx, tokens, reg, DEX, WBNB)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn izi_without_usdt_each_block_only_when_enabled_306() {
    let p = route_common::parts().await;
    let fam = families::make(DEX, "biswap_v3").expect("вид");
    let found = |t0: Address, t1: Address| Found {
        family: fam,
        reference: PoolRef::Contract(Address::ZERO),
        token0: t0,
        token1: t1,
        coin: t0,
        fee_pips: None,
        tick_spacing: None,
        bin_step: None,
        key: None,
        hooks: None,
    };
    let x = found(BTCB, WBNB);
    assert!(
        !ladder_counter::each_block(&p.ctx, &x, Some(USDT)),
        "не задана — как раньше"
    );
    set(&p, ladder_counter::SETTING, Some(json!(true))).await;
    assert!(
        !ladder_counter::each_block(&p.ctx, &x, Some(USDT)),
        "нет dex.pmm_ladder_usd — лесенки нет"
    );
    set(&p, "dex.pmm_ladder_usd", Some(json!(["10", "50"]))).await;
    assert!(ladder_counter::each_block(&p.ctx, &x, Some(USDT)));
    assert!(
        !ladder_counter::each_block(&p.ctx, &x, None),
        "USDT площадки не задан"
    );
    assert!(
        !ladder_counter::each_block(&p.ctx, &found(USDT, WBNB), Some(USDT)),
        "пара с USDT — правило per_block::each_block, не это"
    );
}

/// Ревью 306: обрезка лесенки. X — CAKE записи: пулы «CAKE — USDT» (PancakeSwap V3 и Uniswap V3) прочитаны словами
/// тиков ±3 (route_common), вход 10^12 USDT ни одним из них целиком не считается (complete = false — rate::best_in его
/// не берёт; предусловие проверяется). Ступени 10 и 10^12 USDT — лесенка из одной точки (10 USDT); одна ступень 10^12 —
/// лесенки нет.
#[tokio::test]
async fn ladder_cut_where_counter_pool_depth_ends_306() {
    let p = route_common::parts().await;
    let (ctx, tokens, reg) = (&p.ctx, &p.tokens, &p.registry);
    set(&p, ladder_counter::SETTING, Some(json!(true))).await;
    let huge = e18(1_000_000_000_000);
    assert!(
        !reg.by_pair(CAKE, USDT).is_empty(),
        "пулы CAKE — USDT в записи"
    );
    assert!(
        rate::from_usdt(ctx, reg, USDT, CAKE, huge).is_err(),
        "предусловие: 10^12 USDT глубже прочитанных пулов CAKE — USDT"
    );
    let ten = rate::from_usdt(ctx, reg, USDT, CAKE, e18(10)).unwrap();

    set(
        &p,
        "dex.pmm_ladder_usd",
        Some(json!(["10", "1000000000000"])),
    )
    .await;
    let got = ladder_counter::amounts(ctx, tokens, reg, DEX, CAKE)
        .await
        .unwrap();
    assert_eq!(got, vec![ten], "лесенка из одной точки — край на 10^12");

    set(&p, "dex.pmm_ladder_usd", Some(json!(["1000000000000"]))).await;
    assert!(
        ladder_counter::amounts(ctx, tokens, reg, DEX, CAKE)
            .await
            .unwrap()
            .is_empty(),
        "первой ступени не хватает глубины — лесенки нет"
    );
}
