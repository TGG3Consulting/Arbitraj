//! Файл: tests/bsc_dex_depth_pairs.rs
//! Что делает: решение 73 — глубина в полосе у V3 и V2 на живой записи 2026-10-25:
//! - AAVE Uniswap V3 0x8e3ecc0b…: продажа монеты по всей прочитанной карте (±8 слов) отдаёт 94,3% USDT пула
//!   (balanceOf USDT пула на том же блоке — 1915,19; живой замер 25.10 днём — 2044,75): не больше баланса; остальное —
//!   за прочитанными словами и невыплаченные комиссии позиций; внутри ±2% тиков нет — глубина в полосе совпадает с
//!   формулой одного диапазона;
//! - пара V2 (IOST 0x5fef5bcc…): глубина в полосе ±b совпадает с формулой пары x·y = k — продажа y·(1 − √(1 − b)),
//!   покупка y·(√(1 + b) − 1) — без комиссии пары (чистый объём); монета — токен0 и токен1 (та же пара в зеркальном порядке).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::quote::{depth_band, pool_quote}.

mod depth_common;
mod native_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::quote::depth_band::band_units;
use arb::venues::bsc_dex::quote::pool_quote;

use depth_common::{aave_v3, closed_form, f, rec, usd, v2};

#[tokio::test]
async fn aave_v3_sell_depth_matches_pool_usdt_balance() {
    let ctx = native_common::ctx("depth-aave").await;
    let r = rec();
    let (e, p, bal) = aave_v3(&r);
    let aave = e.coin;
    let all = pool_quote::swap(&ctx, &e, aave, true, U256::from(u128::MAX)).unwrap();
    let (band_buy, band_sell, _) = band_units(&e, Decimal::from(2)).unwrap();
    eprintln!(
        "AAVE 0x8e3ecc0b… блок {}: USDT пула {} — продажа по всей карте {} USDT; ±2%: покупка {} / продажа {}",
        p["block"],
        usd(bal).round_dp(2),
        usd(all.amount_out).round_dp(2),
        usd(band_buy).round_dp(2),
        usd(band_sell).round_dp(2)
    );
    assert!(
        all.amount_out <= bal,
        "из пула не уходит больше, чем в нём USDT"
    );
    // На блоке записи в пуле 1915,19 USDT (25.10 днём было 2044,75), по карте ±8 слов бот отдаёт 1805,81 (94,3%):
    // остальное — в позициях выше прочитанных слов (AAVE — токен1, продажа двигает тик вверх; карта кончилась —
    // complete = false) и в невыплаченных комиссиях позиций, которые лежат на балансе пула, но обменом не отдаются.
    assert!(!all.complete, "карта кончилась раньше суммы");
    let share = f(all.amount_out) / f(bal);
    assert!(
        (0.94..1.0).contains(&share),
        "доля USDT пула в прочитанной карте: {share}"
    );
    assert!(band_sell <= all.amount_out);
    // Внутри ±2% тиков нет (ближайшие −48900 и −49920 при тике −49207) — формула одного диапазона.
    let PoolState::V3 { tick, .. } = e.pool.state else {
        unreachable!()
    };
    let ticks = &e.ticks.as_ref().unwrap().ticks;
    assert!(ticks.range(tick - 210..=tick + 210).next().is_none());
    let (b, s) = closed_form(&e, 2.0);
    let rel = |x: f64, y: f64| ((x - y) / y).abs();
    assert!(
        rel(f(band_buy), b) < 1e-6,
        "покупка {} против формулы {b}",
        f(band_buy)
    );
    assert!(
        rel(f(band_sell), s) < 1e-6,
        "продажа {} против формулы {s}",
        f(band_sell)
    );
}

#[test]
fn v2_band_depth_matches_pair_formula_both_orders() {
    let r = rec();
    for coin0 in [true, false] {
        let e = v2(&r, "0x5fef5bcc62164f8fb0d53d58733cc3940ddfc46c", coin0);
        let PoolState::V2 { reserve0, reserve1 } = e.pool.state else {
            unreachable!()
        };
        let y = f(if coin0 { reserve1 } else { reserve0 });
        for pct in [1u32, 2, 10] {
            let b = pct as f64 / 100.0;
            let (buy, sell, full) = band_units(&e, Decimal::from(pct)).unwrap();
            assert!(full);
            let want_sell = y * (1.0 - (1.0 - b).sqrt());
            // Без комиссии пары: чистый объём (решение Тиграна к решению 73).
            let want_buy = y * ((1.0 + b).sqrt() - 1.0);
            let rel = |x: f64, w: f64| ((x - w) / w).abs();
            assert!(
                rel(f(sell), want_sell) < 1e-6,
                "монета токен{} ±{pct}%: продажа {} против {want_sell}",
                !coin0 as u8,
                f(sell)
            );
            assert!(
                rel(f(buy), want_buy) < 1e-6,
                "монета токен{} ±{pct}%: покупка {} против {want_buy}",
                !coin0 as u8,
                f(buy)
            );
        }
    }
}
