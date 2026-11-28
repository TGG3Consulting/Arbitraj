//! Файл: tests/bsc_dex_tax_late_pools.rs
//! Что делает: живой бот 28.11.2026 (base_dex, сборка e9907a2f): справочник индексатора пуст (код поставщика 10006),
//! первый проход налогов — все монеты «ждут (нет пула)» (Why::NoPool), пулы приходят позже от поиска по фабрикам
//! (index.factory_scan, «Новый пул» ×2946) и дочитываются, пока цикл налогов занят проверкой. Сигнал «пул готов»
//! (pools/progress.rs — Notify::notify_waiters) без ждущего теряется; шаг цикла (check/tax_wait.rs due_now) всё равно
//! выдаёт ждущую монету, у которой пул уже есть. Промежуточные монеты (WETH, cbBTC) цикл налогов проверяет тем же
//! списком, что и монеты (pools/market.rs with_mids), — для решения шага разницы нет.
//! Пул — живая запись AAVE Uniswap V3 0x8e3ecc0b… (tests/data/depth_2026-10-25, tests/depth_common).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, tests/depth_common, arb::venues::bsc_dex::{parts, check::{tax_wait, tax_deep,
//! tax_pass}}.

mod depth_common;
mod native_common;

use std::time::Duration;

use alloy_primitives::Address;

use arb::venues::bsc_dex::check::tax_deep::has_deep;
use arb::venues::bsc_dex::check::tax_pass::Why;
use arb::venues::bsc_dex::check::tax_wait::{due, due_now};
use arb::venues::bsc_dex::parts::Parts;

use depth_common::{aave_v3, rec};

#[tokio::test]
async fn late_pool_wakes_waiting_coin_even_if_signal_was_lost() {
    let ctx = native_common::ctx("tax-late-pools").await;
    let p = Parts::new(ctx);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let pools = p.pools(tx);
    let (e, _, _) = aave_v3(&rec());
    let coin = e.coin;
    // Первый проход: пулов нет — ждёт «нет пула»; вторая монета ждёт сбой узла (повтор — tax.retry_s, не здесь).
    let other = alloy_primitives::address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
    let waiting = vec![
        (coin, "AAVE".to_string(), Why::NoPool),
        (other, "CAKE".to_string(), Why::Node),
    ];
    let pending = |a: Address| pools.loading.pending(&pools, a);
    let deep = |a: Address| has_deep(&pools, &a);
    assert!(
        due_now(&waiting, pending, deep).is_empty(),
        "пула ещё нет — проверять нечего"
    );

    // Поиск по фабрикам нашёл пул, пачка его прочитала (pools/session.rs: запись в реестр, затем «пул готов»), а цикл
    // налогов в это время проверял другие монеты — сигнал некому принять.
    p.registry.put(e);
    pools.signals.pool_ready();
    let late = pools.signals.ready.notified();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), late)
            .await
            .is_err(),
        "notify_waiters без ждущего не хранится — следующее ожидание сигнала его не увидит"
    );
    // Прежний шаг цикла (без сигнала) монету «нет пула» не выдавал — она ждала полного прохода tax.recheck_interval_s.
    assert!(due(&waiting, false, pending, deep).is_empty());
    // Шаг цикла теперь смотрит реестр сам: пул есть — монета на проверку сразу; сбой узла — по-прежнему по tax.retry_s.
    assert_eq!(
        due_now(&waiting, pending, deep),
        vec![(coin, "AAVE".to_string())]
    );
}
