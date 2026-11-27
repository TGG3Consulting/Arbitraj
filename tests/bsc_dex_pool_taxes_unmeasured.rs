//! Файл: tests/bsc_dex_pool_taxes_unmeasured.rs
//! Что делает: ревью решения 280 — пул вида v2_fork_executor, где пара отклонила пробную покупку («налоги пула не
//! измерены», quote/v2_pair_probe.rs buy_unmeasured), НЕ помеха окружения (check/pool_taxes.rs outcome): вердикт по
//! монете выносится по остальным пулам как раньше (откат покупки в них — «не прошла», а не «не проверено»); пул нового
//! вида единственный — «не проверено» с его причиной; без таких пулов итог буква в букву прежний. Причины — настоящие
//! тексты: buy_unmeasured / buy_revert на записанном откате пары («DAOSwap: K», tests/data/v2_fork_executor_2026-11-15).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{check::{pool_taxes, probe_calls, state, taxes}, quote::v2_pair_probe},
//! tests/route_common.

mod route_common;

use rust_decimal::Decimal;
use serde_json::Value;

use arb::core::types::PoolRef;
use arb::venues::bsc_dex::check::pool_taxes::outcome;
use arb::venues::bsc_dex::check::probe_calls;
use arb::venues::bsc_dex::check::simulate::CallResult;
use arb::venues::bsc_dex::check::state::PoolTaxes;
use arb::venues::bsc_dex::check::taxes::Outcome;
use arb::venues::bsc_dex::pools::families;
use arb::venues::bsc_dex::quote::v2_pair_probe::buy_unmeasured;

use route_common::WBNB_USDT_V2;

/// Причины: (пул нового вида «не измерен», откат покупки в обычном пуле — вердикт). Ошибок нет.
async fn reasons() -> (String, String) {
    let p = route_common::parts().await;
    let path = "tests/data/v2_fork_executor_2026-11-15/pairs.json";
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let r: CallResult =
        serde_json::from_value(rec["dex"]["dao_swap"]["below_1pip_last"].clone()).unwrap();
    let mut e = p
        .quoting()
        .reg
        .get(&PoolRef::Contract(WBNB_USDT_V2))
        .unwrap();
    e.family = families::make("dao_swap", "v2_fork_executor").unwrap();
    let un = buy_unmeasured(&e, 5, 3, &r).expect("пара отклонила обмен — свойство пула");
    let failed = probe_calls::buy_revert(&p.ctx, 5, &r, 3).expect("вердикт по монете");
    (un, failed)
}

/// Налоги пула с удачной покупкой и неизмеренной продажей.
fn bought_only() -> PoolTaxes {
    PoolTaxes {
        one_share_only:false,
        buy_pct: Decimal::ZERO,
        sell_pct: None,
        transfer_pct: Decimal::ZERO,
    }
}

#[tokio::test]
async fn unmeasured_pool_is_not_interference() {
    let (un, failed) = reasons().await;
    let none: Vec<String> = Vec::new();
    // Ханипот: покупка откатилась в обычном пуле, пул нового вида не измерен — «не прошла» (раньше — Wait).
    match outcome(
        &[],
        false,
        &[],
        (vec![failed.clone()], vec![], vec![]),
        std::slice::from_ref(&un),
    ) {
        Outcome::Failed(w) => assert!(w.contains(&failed), "{w}"),
        o => panic!("ожидался вердикт «не прошла», а {o:?}"),
    }
    // Продажа откатилась во всех измеренных пулах без помех — «не прошла» и при неизмеренном пуле нового вида.
    let miss = vec![(true, "Biswap: продажа откатилась".to_string())];
    assert!(matches!(
        outcome(
            &[bought_only()],
            false,
            &miss,
            (vec![], vec![], vec![]),
            std::slice::from_ref(&un)
        ),
        Outcome::Failed(_)
    ));
    // Пул нового вида единственный — «не проверено» с его причиной (монету пробовать больше негде).
    match outcome(
        &[],
        false,
        &[],
        (vec![], vec![], vec![]),
        std::slice::from_ref(&un),
    ) {
        Outcome::Wait(w) => assert!(w.contains("налоги пула не измерены"), "{w}"),
        o => panic!("ожидалось «не проверено», а {o:?}"),
    }
    // Помеха окружения у другого пула — вердикта по монете нет; покупка в пуле откатилась — Doubt (ревью t178, Б1).
    assert!(matches!(
        outcome(
            &[],
            false,
            &[],
            (vec![failed.clone()], vec!["узел".into()], vec![]),
            &[un]
        ),
        Outcome::Doubt(_)
    ));
    // Без пулов «не измерены» — как раньше.
    assert_eq!(
        outcome(
            &[],
            false,
            &[],
            (vec![failed.clone()], vec![], vec![]),
            &none
        ),
        Outcome::Failed(failed)
    );
    assert!(matches!(
        outcome(
            &[bought_only()],
            false,
            &miss,
            (vec![], vec!["узел".into()], vec![]),
            &none
        ),
        Outcome::Doubt(_)
    ));
}

/// Откат перевода монеты из пары («DAOSwap: TRANSFER_FAILED» — строка require из кода пары, запись
/// v2_executor_balance_2026-11-15) — механика монеты: buy_unmeasured молчит, голосует buy_revert как у всех.
#[tokio::test]
async fn coin_transfer_revert_votes_like_others() {
    use alloy::sol_types::{Revert, SolError};
    let p = route_common::parts().await;
    let path = "tests/data/v2_executor_balance_2026-11-15/sims.json";
    let rec: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let reason = rec["dex"]["dao_swap"]["pair_code_strings"]["transfer_failed"][0]
        .as_str()
        .unwrap()
        .to_string();
    let r = CallResult {
        status: alloy_primitives::U64::ZERO,
        return_data: Revert { reason }.abi_encode().into(),
        gas_used: alloy_primitives::U64::ZERO,
        error: None,
        logs: Vec::new(),
    };
    let mut e = p
        .quoting()
        .reg
        .get(&PoolRef::Contract(WBNB_USDT_V2))
        .unwrap();
    e.family = families::make("dao_swap", "v2_fork_executor").unwrap();
    assert!(buy_unmeasured(&e, 5, 3, &r).is_none());
    assert!(
        probe_calls::buy_revert(&p.ctx, 5, &r, 3).is_ok(),
        "вердикт по монете"
    );
}
