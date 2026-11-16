//! Файл: tests/bsc_dex_hook_pools_1116_b1.rs
//! Что проверяет: решение «335 (В1)» на записи узла tests/data/hook_pools_2026-11-16 (пулы Infinity CL с крючком):
//! 1. Влияние крючка, измеренное на ОДНОМ блоке (как check/hooks_probe.rs: наш расчёт по состоянию блока против
//!    квотера на нём, формула hooks_probe::impact), — 0 у 30 пулов (28 с комиссией 67, CAKE 0xaf12, BTC; все вызовы
//!    первого блока записи без from), и сверка с вычетом этого влияния (quote/quoter.rs less_hook, diff_pct) проходит
//!    при допуске живого бота 0,11 %.
//! 2. Ложное влияние «память против latest» воспроизводится на BASED (цена менялась между блоками записи): расчёт по
//!    состоянию первого блока (память) против квотера следующего блока даёт влияние разного знака на сторонах
//!    (покупка −0,103 %, продажа +0,103 %); отрицательное обрезается, положительное вычитается при сверке на
//!    следующем блоке, где наш расчёт = квотеру до wei, — наш выход оказывается ниже квотера ровно на ложное влияние
//!    (безопасная сторона, check/reprobe_rule.rs safe). Влияние на одном блоке там же — 0. На этой записи ложное
//!    влияние одного блока (0,103 %) меньше допуска 0,11 % — паузу оно бы не дало; запись показывает механизм, а не
//!    живые 0,11–0,71 %.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_pools_common, arb::venues::bsc_dex::{check::{hooks_probe, quoter_compare,
//! reprobe_rule}, quote::{pool_quote, quoter}}.

mod hook_pools_common;

use std::str::FromStr;

use alloy_primitives::U256;
use rust_decimal::Decimal;

use arb::venues::bsc_dex::check::hooks_probe::impact;
use arb::venues::bsc_dex::check::quoter_compare::{Cmp, Nums};
use arb::venues::bsc_dex::check::reprobe_rule::safe;
use arb::venues::bsc_dex::quote::pool_quote::swap_fee;
use arb::venues::bsc_dex::quote::quoter::{diff_pct, less_hook};
use hook_pools_common::{State, load, one_step, short, usdt};

/// Допуск сверки живого бота (dex.quoter_max_diff_pct = 0,11 % — журнал 16.11, docs/notes t156).
fn limit() -> Decimal {
    Decimal::from_str("0.11").expect("допуск")
}

/// Наш выход по состоянию `s` на сумму и сторону вызова. Ошибок нет.
fn ours(s: &State, z: bool, amount: U256) -> U256 {
    one_step(s, z, amount, swap_fee(s.protocol_fee, s.lp_fee, z)).0
}

#[test]
fn same_block_impact_zero_and_check_passes() {
    let rec = load();
    let first = rec.quotes.iter().map(|q| q.block).min().expect("блок");
    let eps = Decimal::from_str("0.000000001").expect("ε");
    let mut zero = 0;
    for (id, p) in &rec.pools {
        let s = &p.at[&first];
        let calls: Vec<_> = rec
            .quotes
            .iter()
            .filter(|q| &q.pool == id && q.block == first && q.from.is_none())
            .filter_map(|q| q.out.clone().ok().map(|r| (q, r)))
            .collect();
        if calls.is_empty() {
            continue;
        }
        let all_zero = calls.iter().all(|(q, r)| {
            impact(ours(s, q.zero_for_one, q.amount), *r).is_some_and(|i| i.abs() < eps)
        });
        if !all_zero {
            continue;
        }
        zero += 1;
        for (q, r) in &calls {
            let o = ours(s, q.zero_for_one, q.amount);
            let i = impact(o, *r).expect("влияние").max(Decimal::ZERO);
            let local = less_hook(o, i).expect("вычет");
            assert!(local <= *r, "{}: {local} > {r}", short(id));
            assert!(diff_pct(local, *r).expect("%") <= limit(), "{}", short(id));
        }
    }
    assert_eq!(zero, 30, "пулов с влиянием 0 на одном блоке");
}

#[test]
fn memory_vs_latest_false_impact_based() {
    let rec = load();
    let usdt = usdt(&rec);
    let (id, p) = rec
        .pools
        .iter()
        .find(|(id, _)| short(id) == "0xe85bc115")
        .expect("BASED");
    let blocks: Vec<u64> = p.at.keys().copied().collect();
    let (mem, net) = (&p.at[&blocks[0]], &p.at[&blocks[1]]);
    assert_ne!(mem, net, "цена BASED менялась между блоками");
    let (mut buy, mut sell) = (Vec::new(), Vec::new());
    for q in rec
        .quotes
        .iter()
        .filter(|q| &q.pool == id && q.block == blocks[1] && q.from.is_none())
    {
        let r = q.out.clone().expect("ответ");
        // На одном блоке — влияние 0 (наш расчёт = квотеру до wei).
        let on_block = ours(net, q.zero_for_one, q.amount);
        assert_eq!(on_block, r);
        // Память (первый блок) против квотера следующего блока — ложное «влияние крючка».
        let false_pct = impact(ours(mem, q.zero_for_one, q.amount), r).expect("влияние");
        let token_in = if q.zero_for_one {
            p.currency0
        } else {
            p.currency1
        };
        if token_in == usdt {
            buy.push(false_pct);
        } else {
            sell.push(false_pct);
            // Сверка на следующем блоке с этим (положительным) влиянием: наш ниже квотера ровно на него.
            let local = less_hook(on_block, false_pct).expect("вычет");
            let d = diff_pct(local, r).expect("%");
            assert!(
                (d - false_pct).abs() < Decimal::from_str("0.0002").expect("ε"),
                "{d} {false_pct}"
            );
            let pause = Cmp::Pause {
                block: blocks[1],
                why: String::new(),
                nums: Some(Nums {
                    local,
                    remote: r,
                    pct: d,
                    limit: limit(),
                }),
            };
            assert!(safe(&pause), "наш ниже квотера — безопасная сторона");
        }
    }
    assert_eq!((buy.len(), sell.len()), (2, 2));
    assert!(buy.iter().all(|i| *i < Decimal::ZERO), "покупка: {buy:?}");
    assert!(sell.iter().all(|i| *i > Decimal::ZERO), "продажа: {sell:?}");
    let big = sell.iter().copied().max().expect("продажа");
    assert!(
        big > Decimal::from_str("0.1").expect("0,1") && big < limit(),
        "{big}"
    );
}
