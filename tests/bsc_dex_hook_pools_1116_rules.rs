//! Файл: tests/bsc_dex_hook_pools_1116_rules.rs
//! Что проверяет: решение «335 (записи узла)» на записи tests/data/hook_pools_2026-11-16 (пулы Infinity CL с крючком):
//! 1. Правило безопасной стороны (check/reprobe_rule.rs safe) и вычет влияния (quote/quoter.rs less_hook) на настоящих
//!    числах: RIVER — на блоке записи наш выход = квотеру до wei; влияние 0,1306 % из журнала живого бота даёт ровно
//!    живое расхождение «наш ниже квотера» (безопасная сторона); влияние, измеренное на одном блоке, — 0. CAKE
//!    0xc6104625 без влияния — наш выше квотера (опасная сторона, перемер не применяется); после влияния на одном
//!    блоке наш выход не выше квотера (less_hook округляет миллионные доли вверх).
//! 2. BLESS 0x6e398c95: откат квотера UnexpectedRevertBytes(NotEnoughLiquidity(PoolId)) разбирается
//!    quote/quoter_revert.rs (short_pool_id, from_data) и по тексту узла — пакетной пробой перед сделкой
//!    (check/hook_batch_parse.rs classify); ликвидность пула 0 на всех трёх блоках.
//! 3. Группа крючка по ключу (pools/hooks.rs info) — B у всех 37 пулов; beforeSwap есть у всех, ReturnsDelta — ни у одного.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_pools_common, arb::venues::bsc_dex::{quote::{pool_quote, quoter, quoter_revert},
//! check::{quoter_compare, reprobe_rule}, pools::{hooks, key}}.

mod hook_pools_common;

use std::str::FromStr;

use alloy_primitives::{B256, Bytes, U256};
use rust_decimal::Decimal;

use arb::core::types::HookGroup;
use arb::venues::bsc_dex::check::hook_batch_parse::{Fail, classify};
use arb::venues::bsc_dex::check::hooks_probe::impact;
use arb::venues::bsc_dex::check::quoter_compare::{Cmp, Nums};
use arb::venues::bsc_dex::check::reprobe_rule::safe;
use arb::venues::bsc_dex::pools::hooks::info;
use arb::venues::bsc_dex::pools::key::{InfinityPoolKeyData, PoolKey};
use arb::venues::bsc_dex::quote::pool_quote::swap_fee;
use arb::venues::bsc_dex::quote::quoter::{diff_pct, less_hook};
use arb::venues::bsc_dex::quote::quoter_revert::{from_data, short_pool_id};
use hook_pools_common::{Rec, load, one_step, short, signed, usdt};

/// Пауза с цифрами на настоящих числах (допуск — из журнала живого бота, dex.quoter_max_diff_pct 0,11).
fn pause(local: U256, remote: U256) -> Cmp {
    Cmp::Pause {
        block: 0,
        why: String::new(),
        nums: Some(Nums {
            local,
            remote,
            pct: diff_pct(local, remote).expect("%"),
            limit: Decimal::from_str("0.11").expect("допуск"),
        }),
    }
}

/// Наш шаг и ответ квотера без from на первом блоке записи для пула и стороны «покупка 20 USDT» (вход — USDT).
fn buy20(rec: &Rec, prefix: &str) -> (U256, U256) {
    let usdt = usdt(rec);
    let twenty = U256::from(20u64) * U256::from(10u64).pow(U256::from(18u64));
    let first = rec.quotes.iter().map(|q| q.block).min().expect("блок");
    let q = rec
        .quotes
        .iter()
        .find(|q| {
            let p = &rec.pools[&q.pool];
            let token_in = if q.zero_for_one {
                p.currency0
            } else {
                p.currency1
            };
            short(&q.pool) == prefix
                && q.block == first
                && q.from.is_none()
                && q.amount == twenty
                && token_in == usdt
        })
        .expect("вызов квотера");
    let s = &rec.pools[&q.pool].at[&q.block];
    let fee = swap_fee(s.protocol_fee, s.lp_fee, q.zero_for_one);
    (
        one_step(s, q.zero_for_one, q.amount, fee).0,
        q.out.clone().expect("ответ"),
    )
}

#[test]
fn safe_side_on_real_numbers() {
    let rec = load();
    // RIVER: на H наш выход = квотеру до wei; влияние 0,1306 % (журнал живого бота) даёт «наш ниже» — безопасно.
    let (ours, remote) = buy20(&rec, "0x35c76576");
    assert_eq!(ours, remote);
    let pct = Decimal::from_str("0.1306").expect("%");
    let local = less_hook(ours, pct).expect("вычет");
    let first = pause(local, remote);
    assert!(safe(&first));
    let d = diff_pct(local, remote).expect("%");
    assert!(
        (d - pct).abs() < Decimal::from_str("0.00001").expect("ε"),
        "{d}"
    );
    // Живые числа журнала (RIVER, блок 133963327, 7 знаков): квотер 31.0747533 → наш 31.0341697 = вычет 1306 ppm.
    let live = less_hook(U256::from(310_747_533u64), pct).expect("вычет");
    assert!(
        signed(U256::from(310_341_697u64), live).abs() <= 1,
        "{live}"
    );
    // Влияние, измеренное на одном блоке (как check/hooks_probe.rs measure_plan), — 0: расхождения нет.
    assert_eq!(
        diff_pct(less_hook(ours, Decimal::ZERO).expect("0"), remote),
        Some(Decimal::ZERO)
    );
    // CAKE 0xc610: без влияния наш выход выше квотера — опасная сторона, перемер (а) не применяется.
    let (ours, remote) = buy20(&rec, "0xc6104625");
    assert!(ours > remote);
    assert!(!safe(&pause(ours, remote)));
    // Влияние на одном блоке (check/hooks_probe.rs impact) закрывает расхождение: наш выход за вычетом (less_hook,
    // миллионные доли вверх) не выше квотера и отстаёт меньше чем на 1 ppm.
    let hook = impact(ours, remote).expect("влияние");
    let local = less_hook(ours, hook).expect("вычет");
    assert!(local <= remote, "{local} > {remote}");
    assert!(diff_pct(local, remote).expect("%") < Decimal::from_str("0.0001").expect("ε"));
}

#[test]
fn bless_quoter_revert_is_not_enough_liquidity() {
    let rec = load();
    let (id, p) = rec
        .pools
        .iter()
        .find(|(id, _)| short(id) == "0x6e398c95")
        .expect("BLESS");
    assert!(p.at.values().all(|s| s.liquidity == 0), "ликвидность 0");
    let mut n = 0;
    for q in rec.quotes.iter().filter(|q| &q.pool == id) {
        let data: Bytes = q.out.clone().expect_err("откат").parse().expect("hex");
        assert_eq!(
            short_pool_id(&data),
            Some(B256::from_str(id).expect("PoolId"))
        );
        assert_eq!(from_data(&data), Some("квотер: NotEnoughLiquidity"));
        // Пакетная проба перед сделкой (check/hook_batch_parse.rs classify) разбирает тот же откат по тексту узла
        // (пачка JSON-RPC: «execution reverted: 0x…»).
        let text = q.message.clone().expect("текст отказа");
        assert_eq!(
            classify(&text),
            Fail::Short(B256::from_str(id).expect("PoolId"))
        );
        n += 1;
    }
    assert_eq!(n, 4 * 9);
}

#[test]
fn all_hooks_group_b() {
    let rec = load();
    assert_eq!(rec.pools.len(), 37);
    for (id, p) in &rec.pools {
        let h = info(&PoolKey::Infinity(InfinityPoolKeyData {
            currency0: p.currency0,
            currency1: p.currency1,
            hooks: p.hooks,
            manager: p.manager,
            fee: p.fee,
            parameters: p.parameters,
        }));
        assert_eq!(h.group, HookGroup::B, "{id}");
        assert!(
            !h.before_swap_returns_delta && !h.after_swap_returns_delta,
            "{id}"
        );
        assert!(h.before_swap, "{id}");
    }
}
