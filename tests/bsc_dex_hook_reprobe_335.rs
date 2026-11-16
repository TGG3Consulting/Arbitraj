//! Файл: tests/bsc_dex_hook_reprobe_335.rs
//! Что делает: решение 335 (задача t156) — итог сверки пула с крючком после перемера влияния крючка на том же блоке
//! (check/quoter_reprobe.rs merge) на цифрах живого бота 16.11.2026 (сборка 8fe83aa1, журнал dex.quoter_pause):
//! пул RIVER pancakeswap-infinity-clmm, PoolId 0x35c765765132d9ba94063a26d5eab98e9f005354b614d0e3f58ba1e7dc08a679,
//! блок 133963327, покупка — наш расчёт 31.0341697, квотер 31.0747533 (0,1306 %), допуск dex.quoter_max_diff_pct
//! 0,11 %. Числа журнала — с 7 знаками после запятой, поэтому здесь они в единицах 1e-7 (отношение то же).
//! Проверяется: расхождение даёт паузу; если перемер на блоке покажет отрицательное влияние (квотер с крючком больше
//! нашего расчёта без крючка), обрезка до 0 (quote/quoter.rs less_hook, решение 261) оставляет паузу и причина
//! помечена «влияние отрицательное»; повторная сверка прошла — пауза снимается; сбой повторной сверки — пауза прежняя
//! с пометкой. Сам перемер (eth_call квотера на блоке) здесь не проверяется: записанных ответов узла по пулам с
//! крючком нет — их список в docs/notes/t156-hook-pools-quoter-pause-2026-11-16.md (промпт серверному ИИ).
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::{check, quote}, arb::settings.

mod native_common;

use std::str::FromStr;

use alloy_primitives::U256;
use rust_decimal::Decimal;
use serde_json::json;

use arb::core::types::{HookImpact, Side};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::settings::scope::Scope;
use arb::venues::bsc_dex::check::quoter_compare::{Cmp, judged};
use arb::venues::bsc_dex::check::quoter_reprobe::{NEGATIVE, merge, side_pct};
use arb::venues::bsc_dex::quote::quoter;

/// Живой случай RIVER (единицы 1e-7 монеты): наш расчёт и квотер.
const OURS: u64 = 310_341_697;
const QUOTER: u64 = 310_747_533;
/// Блок живого случая.
const BLOCK: u64 = 133_963_327;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

/// Окружение с допуском сверки 0.11% (как на живом боте).
async fn ctx(tag: &str) -> arb::venues::bsc_dex::ctx::Ctx {
    let c = native_common::ctx(tag).await;
    c.settings
        .set(
            "dex.quoter_max_diff_pct",
            Scope::Venue("bsc_dex".into()),
            Some(json!("0.11")),
            "тест",
        )
        .await
        .unwrap();
    c
}

#[tokio::test]
async fn negative_impact_keeps_pause_with_mark() {
    let c = ctx("t335-neg").await;
    let (ours, remote) = (U256::from(OURS), U256::from(QUOTER));
    // Первая сверка: наш расчёт (уже за вычетом влияния) меньше квотера на 0,1306% > 0,11% — пауза.
    let first = judged(
        quoter::judge(&c, ours, remote, Decimal::ZERO).unwrap(),
        BLOCK,
    );
    assert!(matches!(first, Cmp::Pause { .. }));
    // Перемер на том же блоке дал бы отрицательное влияние покупки (−0,1306%): less_hook его не прибавляет —
    // повторная сверка снова расходится, пауза остаётся, причина помечена.
    let neg = HookImpact {
        buy_pct: d("-0.1306"),
        sell_pct: d("0"),
    };
    assert_eq!(side_pct(&neg, Side::Buy), d("-0.1306"));
    let second = judged(quoter::judge(&c, ours, remote, neg.buy_pct).unwrap(), BLOCK);
    let res = merge(first, Ok(second), &neg, Side::Buy, BLOCK);
    let Cmp::Pause { why, nums, .. } = res else {
        panic!("ждали паузу: {res:?}");
    };
    assert!(why.contains(NEGATIVE), "{why}");
    assert!(why.contains("после перемера влияния крючка"), "{why}");
    let n = nums.unwrap();
    assert_eq!((n.local, n.remote), (ours, remote));
}

#[tokio::test]
async fn passed_recheck_lifts_pause_and_failure_keeps_it() {
    let c = ctx("t335-ok").await;
    let (ours, remote) = (U256::from(OURS), U256::from(QUOTER));
    let first = judged(
        quoter::judge(&c, ours, remote, Decimal::ZERO).unwrap(),
        BLOCK,
    );
    let i = HookImpact {
        buy_pct: d("0"),
        sell_pct: d("0"),
    };
    // Повторная сверка в допуске (квотер равен расчёту) — итог «прошла», пауза снимается.
    let ok = judged(
        quoter::judge(&c, remote, remote, Decimal::ZERO).unwrap(),
        BLOCK,
    );
    assert_eq!(
        merge(first.clone(), Ok(ok), &i, Side::Buy, BLOCK),
        Cmp::Ok { block: BLOCK }
    );
    // Повторная сверка не удалась (сбой узла) — пауза прежняя, причина дописана.
    let err = GatewayError::new(ErrorCode::Timeout, None, "таймаут");
    let res = merge(first, Err(err), &i, Side::Sell, BLOCK);
    let Cmp::Pause { why, .. } = res else {
        panic!("ждали паузу: {res:?}");
    };
    assert!(
        why.contains("повторная сверка после перемера не удалась"),
        "{why}"
    );
}
