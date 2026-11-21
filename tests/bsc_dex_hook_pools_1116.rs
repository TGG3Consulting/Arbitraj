//! Файл: tests/bsc_dex_hook_pools_1116.rs
//! Что проверяет: решение «335 (записи узла)» — пулы PancakeSwap Infinity CL с крючком на РЕАЛЬНЫХ ответах узла BSC
//! (tests/data/hook_pools_2026-11-16, блоки 133993052…133993054; 35 пулов с паузой сверки 16.11 и 2 без паузы):
//! 1. Комиссия: обе половины protocolFee в slot0 равны; у пулов с постоянной комиссией (fee ключа ≠ 0x800000) поле
//!    fee каждого записанного события Swap = quote/pool_quote.rs swap_fee(protocolFee, lpFee), protocolFee события =
//!    половине (99 = 67 ⊕ 32, 149 = 100 ⊕ 49). Какую половину брать для направления, запись не проверяет (половины равны).
//! 2. Наш выход одним шагом SwapMath V4 (math/swap_step.rs step_v4 — шаг math/swap_cl.rs) с комиссией swap_fee по slot0
//!    на том же блоке = ответу CLQuoter ДО WEI во всех вызовах, кроме перечисленных в EXPECTED — там зафиксировано
//!    фактическое расхождение (переход тика — карты тиков в записи нет; динамическая комиссия крючка).
//! 3. Пул CAKE 0xc6104625 (динамическая комиссия, lpFee slot0 = 0): квотер = наш шаг с комиссией из события Swap (3300)
//!    до wei — крючок подменяет lpFee на обмене (lpFeeOverride, infinity-core CLHooks.sol#L145-L147), slot0 этого не
//!    показывает; без крючка наш выход ВЫШЕ квотера (опасная сторона).
//! 4. Ответ квотера не зависит от from (без from / Universal Router / исполнитель) и одинаков на блоках, где slot0 и
//!    ликвидность пула не менялись.
//!
//! Правило безопасной стороны, BLESS и группы крючков — tests/bsc_dex_hook_pools_1116_rules.rs.
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_pools_common, arb::venues::bsc_dex::{quote::pool_quote, pools::hooks}.

mod hook_pools_common;

use std::str::FromStr;

use alloy_primitives::U256;

use arb::venues::bsc_dex::pools::hooks::DYNAMIC_FEE_FLAG;
use arb::venues::bsc_dex::quote::pool_quote::swap_fee;
use hook_pools_common::{load, one_step, short, signed};

/// Фактические расхождения записи «наш шаг − квотер», wei: (PoolId, zeroForOne, сумма, разница). Все прочие — 0.
const EXPECTED: [(&str, bool, &str, i128); 13] = [
    // GAIB, динамическая комиссия: 1 USDT — до wei, 20 USDT — шаг вышел бы за тик (карты тиков в записи нет).
    (
        "0x5c1b4d20",
        true,
        "20000000000000000000",
        1229879829717215855,
    ),
    // BTCMT, динамическая комиссия: то же для 20 USDT; продажа — 1 wei.
    (
        "0x62166b8a",
        false,
        "20000000000000000000",
        222031080478205217,
    ),
    ("0x62166b8a", true, "30379068085182721835", 1),
    // CAKE 0xc528 (комиссия 100, шаг тика 1): до 17 wei; продажа 20 USDT — переход в тик с другой ликвидностью.
    ("0xc528f2c0", false, "1000000000000000000", 2),
    ("0xc528f2c0", false, "20000000000000000000", 17),
    ("0xc528f2c0", true, "623046832326064713", 3),
    (
        "0xc528f2c0",
        true,
        "12460936646521294265",
        -2253151910467637722,
    ),
    // CAKE 0xc610: крючок подменяет lpFee (п. 3) — без него наш выход выше квотера на 0,2994–0,3009 %.
    ("0xc6104625", false, "1000000000000000000", 1863928764312551),
    (
        "0xc6104625",
        false,
        "20000000000000000000",
        36911863995231473,
    ),
    ("0xc6104625", true, "621633784380807957", 2998435431190661),
    (
        "0xc6104625",
        true,
        "12432675687616159154",
        59378793306735010,
    ),
    // BTC (комиссия 100, шаг тика 1): 1 wei.
    ("0xe3a43b0d", true, "1000000000000000000", 1),
    ("0xe3a43b0d", true, "20000000000000000000", 1),
];

fn expected(pool: &str, z: bool, amount: U256) -> i128 {
    EXPECTED
        .iter()
        .find(|(p, ez, a, _)| {
            *p == short(pool) && *ez == z && U256::from_str(a).ok() == Some(amount)
        })
        .map_or(0, |e| e.3)
}

#[test]
fn fee_composition_matches_swap_events() {
    let rec = load();
    let mut checked = 0;
    for (id, p) in &rec.pools {
        for s in p.at.values() {
            assert_eq!(
                s.protocol_fee & 0xfff,
                s.protocol_fee >> 12,
                "{id}: половины protocolFee"
            );
        }
        let s = p.at.values().next().expect("slot0 на H");
        if p.fee == DYNAMIC_FEE_FLAG {
            continue;
        }
        for (fee, proto) in &p.swaps {
            assert_eq!(*proto, s.protocol_fee & 0xfff, "{id}: protocolFee события");
            assert_eq!(
                *fee,
                swap_fee(s.protocol_fee, s.lp_fee, true),
                "{id}: fee события"
            );
            checked += 1;
        }
    }
    assert!(
        checked > 100,
        "событий Swap пулов с постоянной комиссией: {checked}"
    );
}

#[test]
fn our_step_vs_quoter() {
    let rec = load();
    let (mut exact, mut fixed) = (0, 0);
    for q in &rec.quotes {
        let Ok(remote) = q.out else { continue };
        let p = &rec.pools[&q.pool];
        let s = &p.at[&q.block];
        let fee = swap_fee(s.protocol_fee, s.lp_fee, q.zero_for_one);
        let (ours, whole) = one_step(s, q.zero_for_one, q.amount, fee);
        let want = expected(&q.pool, q.zero_for_one, q.amount);
        assert_eq!(
            signed(ours, remote),
            want,
            "{} блок {} {:?}",
            q.pool,
            q.block,
            q
        );
        if want == 0 {
            assert!(whole, "{}: вся сумма вошла в шаг", q.pool);
            exact += 1;
        } else {
            fixed += 1;
        }
    }
    // 37 пулов: 2 без USDT в паре (не спрошены), BLESS — откат; 34 × 4 вызова × 3 from × 3 блока.
    assert_eq!(exact + fixed, 34 * 4 * 9);
    assert_eq!(fixed, 13 * 9);
}

#[test]
fn hook_override_explains_cake_c610() {
    let rec = load();
    let (id, p) = rec
        .pools
        .iter()
        .find(|(id, _)| short(id) == "0xc6104625")
        .expect("пул записи");
    assert_eq!(p.fee, DYNAMIC_FEE_FLAG);
    let event_fee = p.swaps.last().expect("событие Swap").0;
    let mut n = 0;
    for q in rec.quotes.iter().filter(|q| &q.pool == id) {
        let s = &p.at[&q.block];
        assert_eq!(s.lp_fee, 0, "lpFee slot0");
        let (ours, _) = one_step(s, q.zero_for_one, q.amount, event_fee);
        assert_eq!(Ok(ours), q.out, "с комиссией события {event_fee}");
        n += 1;
    }
    assert_eq!(n, 4 * 9);
}

#[test]
fn quoter_same_for_from_and_steady_blocks() {
    let rec = load();
    for a in &rec.quotes {
        for b in &rec.quotes {
            let same_call =
                a.pool == b.pool && a.zero_for_one == b.zero_for_one && a.amount == b.amount;
            if !same_call {
                continue;
            }
            let p = &rec.pools[&a.pool];
            if a.block == b.block || p.at[&a.block] == p.at[&b.block] {
                assert_eq!(
                    a.out, b.out,
                    "{} блоки {} {} from {:?} {:?}",
                    a.pool, a.block, b.block, a.from, b.from
                );
            }
        }
    }
}
