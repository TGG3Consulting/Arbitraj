//! Файл: tests/bsc_dex_tick_read_band_swap.rs
//! Что делает: решение 296 на живой записи пула PancakeSwap V3 AAVE/USDT 0x2e31285a… (tickSpacing 1; слова битмапа ±8
//! вокруг текущего — tests/data/depth_2026-10-25, рекордер tests/recorders/record_depth.js). Карта, которую бот
//! прочитал бы при dex.tick_words = 2 (±2 слова), и при полосе чтения dex.tick_read_band_pct = 10% (±6 слов) — ровно
//! записанные ответы TickLens по этим словам (tick_band_common::narrowed). Проверяется:
//! - расчёт обмена идёт дальше прежнего края: сумма, которая при ±2 словах не помещается (complete = false), при ±6
//!   получает больший выход; суммы, помещающиеся в ±2, дают тот же выход при ±6 (внутри прочитанного ничего не меняется);
//! - глубина пула в полосе dex.depth_band_pct (2%) и признак «полоса прочитана целиком» одинаковы при ±2, ±6 и ±8
//!   словах (pools/depth_book.rs считает фильтр dex.min_pool_liquidity_usd по этой глубине, quote/depth_band.rs
//!   band_depth → band_units).
//!
//! Что доказано и что нет: пул записи почти пустой — установленных тиков в словах ±8 нет, liquidity ≈ 7,96·10¹⁵,
//! balanceOf USDT = 0. Доказано: обмен БЕЗ пересечения тиков идёт дальше прежнего края ±2 слов, внутри прежней карты
//! выход тот же. «Глубина не изменилась» на этом пуле доказано тривиально (тиков нет). Пул с установленными тиками за
//! прежним краем не покрыт — нужна запись (docs/TASKS.md, решение 296 п. 2).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/tick_band_common, tests/depth_common, arb::venues::bsc_dex::{pools::load_v2v3, quote}.
//! Настройки теста: dex.tick_words = 2 (значение сервера 15.11), dex.depth_band_pct = 2, dex.tick_read_band_pct = 10.

mod depth_common;
mod native_common;
mod tick_band_common;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::PoolState;
use arb::venues::bsc_dex::pools::load_v2v3::word_range;
use arb::venues::bsc_dex::quote::depth_band::band_units;
use arb::venues::bsc_dex::quote::pool_quote;

use depth_common::rec;
use tick_band_common::{aave_ts1, ctx, narrowed, set};

#[tokio::test]
async fn swap_goes_past_old_edge_and_depth_is_unchanged() {
    let r = rec();
    let full = aave_ts1(&r);
    let PoolState::V3 { tick, .. } = full.pool.state else {
        unreachable!()
    };
    let c = ctx("swap").await;
    set(&c, "dex.tick_words", Some(json!(2))).await;
    set(&c, "dex.depth_band_pct", Some(json!(2))).await;
    let old_words = word_range(&c, tick, 1).unwrap();
    set(&c, "dex.tick_read_band_pct", Some(json!(10))).await;
    let new_words = word_range(&c, tick, 1).unwrap();
    assert_eq!(new_words.1 - new_words.0, 12, "±6 слов при 10%");
    assert_eq!(old_words.1 - old_words.0, 4, "±2 слова прежде");
    let old = narrowed(&full, old_words);
    let new = narrowed(&full, new_words);
    let m =
        |e: &arb::venues::bsc_dex::pools::registry::Entry| e.ticks.as_ref().unwrap().ticks.len();
    eprintln!(
        "тиков в карте: ±2 — {}, ±6 — {}, ±8 — {}",
        m(&old),
        m(&new),
        m(&full)
    );
    for token_in in [full.coin, full.usdt] {
        let side = if token_in == full.coin {
            "продажа AAVE"
        } else {
            "покупка AAVE"
        };
        // Сумма, на которой ±2 слова кончаются: удваиваем от 1 минимальной единицы до первого complete = false. Пул почти
        // пустой (liquidity ≈ 7,96·10¹⁵, установленных тиков в ±8 словах нет, balanceOf USDT = 0 на блоке записи) —
        // 1 целый токен (10¹⁸) за ±2 слова уже не помещается, поэтому старт с 1, а не с 10¹⁸.
        let mut amount = U256::from(1u64);
        let (mut fits, mut past) = (None, None);
        for _ in 0..128 {
            let o = pool_quote::swap(&c, &old, token_in, true, amount).unwrap();
            if o.complete {
                fits = Some((amount, o.amount_out));
            } else {
                past = Some((amount, o.amount_out));
                break;
            }
            amount *= U256::from(2);
        }
        let (a_fit, out_fit) = fits.expect("есть сумма внутри ±2 слов");
        let (a_past, out_old) = past.expect("есть сумма за краем ±2 слов");
        let n_fit = pool_quote::swap(&c, &new, token_in, true, a_fit).unwrap();
        assert!(n_fit.complete, "{side}: внутри ±2 — помещается и при ±6");
        assert_eq!(
            n_fit.amount_out, out_fit,
            "{side}: внутри прочитанного выход тот же"
        );
        let n_past = pool_quote::swap(&c, &new, token_in, true, a_past).unwrap();
        eprintln!(
            "{side}: вход {a_past}: ±2 слова — выход {out_old} (не помещается), ±6 — {} (помещается: {})",
            n_past.amount_out, n_past.complete
        );
        assert!(
            n_past.amount_out > out_old,
            "{side}: расчёт идёт дальше прежнего края"
        );
    }
    // Глубина в полосе 2% — одна и та же при любой дальности чтения, полоса прочитана целиком.
    let two = Decimal::from(2);
    let d_old = band_units(&old, two).unwrap();
    assert!(d_old.2, "полоса 2% внутри ±2 слов");
    assert_eq!(
        band_units(&new, two).unwrap(),
        d_old,
        "±6 слов — та же глубина"
    );
    assert_eq!(
        band_units(&full, two).unwrap(),
        d_old,
        "±8 слов — та же глубина"
    );
}
