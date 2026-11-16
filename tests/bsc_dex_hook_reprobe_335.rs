//! Файл: tests/bsc_dex_hook_reprobe_335.rs
//! Что делает: решение 335 (ревью t156) — ЧИСТАЯ ЛОГИКА правил перемера влияния крючка перед паузой сверки
//! (check/reprobe_rule.rs) и хранения влияния на блоке (check/hook_store.rs newer, stale). Числа здесь — НЕ записи
//! узла и не цифры живого бота: это условные значения для проверки ветвлений (меньше / больше, старше / новее).
//! Записанных ответов узла по пулам с крючком нет: сам перемер (eth_call квотера на блоке), проба на одном блоке и
//! повторная сверка этим тестом не проверяются — нужен тест на записях tests/data/hook_pools_2026-11-16
//! (docs/TASKS.md, решение 335).
//! Проверяется: асимметрия — перемер допустим только при расхождении в безопасную сторону (наш выход ниже квотера),
//! при нашем выходе выше квотера или без цифр — нет; перемер меняет только сверяемую сторону; повторная сверка
//! прошла — итог «прошла», осталась паузой — пометки перемера и «отрицательное», сбой — первая пауза с пометкой;
//! измерение на более старом блоке не затирает более новое; возраст влияния против предела.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{reprobe_rule, hook_store, quoter_compare}, arb::core::types.

use alloy_primitives::U256;
use chrono::{Duration, Utc};
use rust_decimal::Decimal;

use arb::core::types::{HookImpact, Side};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::venues::bsc_dex::check::hook_store::{newer, stale};
use arb::venues::bsc_dex::check::quoter_compare::{Cmp, Nums};
use arb::venues::bsc_dex::check::reprobe_rule::{NEGATIVE, merge, only_side, safe};

/// Условная пауза (не запись): наш выход `local`, квотер `remote`.
fn pause(local: u64, remote: u64) -> Cmp {
    Cmp::Pause {
        block: 1,
        why: "пауза".into(),
        nums: Some(Nums {
            local: U256::from(local),
            remote: U256::from(remote),
            pct: Decimal::ONE,
            limit: Decimal::ONE,
        }),
    }
}

fn imp(buy: i64, sell: i64) -> HookImpact {
    HookImpact {
        buy_pct: Decimal::from(buy),
        sell_pct: Decimal::from(sell),
        buy: Vec::new(),
        sell: Vec::new(),
    }
}

#[test]
fn reprobe_only_on_safe_side() {
    // Наш выход ниже квотера — безопасно, перемер допустим.
    assert!(safe(&pause(99, 100)));
    // Наш выход выше квотера — опасно, перемера нет (пауза как до решения 335).
    assert!(!safe(&pause(101, 100)));
    // Цифр нет — перемера нет.
    let no_nums = Cmp::Pause {
        block: 1,
        why: "пауза".into(),
        nums: None,
    };
    assert!(!safe(&no_nums));
    assert!(!safe(&Cmp::Ok { block: 1 }));
}

#[test]
fn reprobe_changes_only_checked_side() {
    let (old, new) = (imp(2, 3), imp(1, 9));
    assert_eq!(only_side(&old, &new, Side::Buy), imp(1, 3));
    assert_eq!(only_side(&old, &new, Side::Sell), imp(2, 9));
}

#[test]
fn merge_outcomes() {
    let i = imp(-1, 0);
    assert_eq!(
        merge(pause(99, 100), Ok(Cmp::Ok { block: 1 }), &i, Side::Sell, 1),
        Cmp::Ok { block: 1 }
    );
    let Cmp::Pause { why, .. } = merge(pause(99, 100), Ok(pause(99, 100)), &i, Side::Buy, 1) else {
        panic!("ждали паузу");
    };
    assert!(why.contains(NEGATIVE), "{why}");
    assert!(why.contains("после перемера влияния крючка"), "{why}");
    let err = GatewayError::new(ErrorCode::Timeout, None, "таймаут");
    let Cmp::Pause { why, .. } = merge(pause(99, 100), Err(err), &i, Side::Buy, 1) else {
        panic!("ждали паузу");
    };
    assert!(
        why.contains("повторная сверка после перемера не удалась"),
        "{why}"
    );
}

#[test]
fn older_block_does_not_overwrite_and_age() {
    assert!(newer(Some(10), 9));
    assert!(!newer(Some(10), 10));
    assert!(!newer(Some(10), 11));
    assert!(!newer(None, 1));
    let now = Utc::now();
    assert!(stale(Some(now - Duration::seconds(11)), now, 10));
    assert!(!stale(Some(now - Duration::seconds(10)), now, 10));
    assert!(stale(None, now, 10));
}
