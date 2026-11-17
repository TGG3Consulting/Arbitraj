//! Файл: tests/bsc_dex_hook_ladder_review3_1117.rs
//! Что проверяет: ревью 3 решения «335 (лесенка влияния)» на досъёмке узла 17.11 (tests/data/hook_pools_ticks_2026-11-17,
//! блок 134000292; GAIB 0x5c1b: влияние покупки 0 / 0,099 / 0,674 % на 1 / 20 / 100 USDT):
//! 1. Н-а — точка пробы перед сделкой (check/hook_store.rs fold_head) не продвигает блок записанного замера: полный
//!    замер цикла на блоке между ними принимается (put_head); fold_head, который ничего не пишет, не сбрасывает
//!    эпоху котировки (решение 352, Checks::generation).
//! 2. Б4 — отказ квотера на точке (check/hook_fold.rs judge_side): эта точка и большие суммы стороны не измерены,
//!    меньшие работают; не осталось точек — «не прошла» только сторона; пул целиком — обе (check/hooks_probe.rs
//!    settled). Текст отказа — квотер записи 16.11 (BLESS 0x6e398c95, NotEnoughLiquidity) — в tests/
//!    bsc_dex_hook_pools_1116_rules.rs; здесь — правило на лесенке GAIB.
//!
//! Ревью 4 (Н8): тесты Drive (Б3, Б5) перенесены в tests/bsc_dex_hook_drive_logic_1117.rs — это чистая логика
//! переходов на условных числах (ответа квотера на промежуточных суммах в записи нет), а не тест «на записи».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_ticks_common, arb::venues::bsc_dex::check::{hook_fold, hook_store, hooks_probe, state}.

mod hook_ticks_common;

use std::str::FromStr;

use alloy_primitives::{Address, B256};
use rust_decimal::Decimal;

use arb::core::types::{
    Asset, CheckState, HookGroup, HookImpact, HookStep, PoolCheck, PoolRef, VenueId,
};
use arb::venues::bsc_dex::check::hook_fold::judge_side;
use arb::venues::bsc_dex::check::hook_store::{fold_head, put_head};
use arb::venues::bsc_dex::check::hooks_probe::{Plan, Point, impact, settled};
use arb::venues::bsc_dex::check::state::Checks;
use hook_ticks_common::{Pool, load, map, ours, pools, quotes};

/// GAIB: запись пула, лесенка покупки (1 / 20 / 100 USDT), направление покупки, ключ.
fn gaib() -> (Pool, Vec<HookStep>, bool, PoolRef) {
    let all = load();
    let ps = pools(&all);
    let qs = quotes(&all, &ps);
    let (id, p) = ps
        .into_iter()
        .find(|(id, _)| id.starts_with("0x5c1b4d20"))
        .expect("GAIB");
    let m = map(&p);
    let mut buy: Vec<HookStep> = qs
        .iter()
        .filter(|q| q.id == id && q.buy)
        .map(|q| HookStep {
            amount_in: q.amount,
            usd: Decimal::from(q.usd),
            pct: impact(ours(&p, &m, q.z, true, q.amount, p.lp).amount_out, q.remote)
                .expect("влияние"),
            coins: None,
            quoter_out: Some(q.remote),
        })
        .collect();
    buy.sort_by_key(|s| s.amount_in);
    let z = qs.iter().find(|q| q.id == id && q.buy).expect("покупка").z;
    let r = PoolRef::Singleton {
        manager: Address::ZERO,
        id: B256::from_str(&id).expect("PoolId"),
    };
    (p, buy, z, r)
}

fn check(i: HookImpact) -> PoolCheck {
    PoolCheck {
        venue: VenueId("bsc_dex".into()),
        family: "pancakeswap-infinity-clmm".into(),
        pool: PoolRef::Contract(Address::ZERO),
        asset: Asset {
            ticker: "GAIB".into(),
            bsc_contract: None,
        },
        group: HookGroup::B,
        impact: CheckState::Known(i),
        checked_at: None,
    }
}

#[test]
fn na_trade_point_keeps_cycle_block() {
    let (_, buy, _, r) = gaib();
    let c = Checks::default();
    let cycle = check(HookImpact {
        buy_pct: buy[1].pct,
        buy: buy[..2].to_vec().into(),
        ..Default::default()
    });
    assert!(put_head(&c, &r, 10, &cycle, None));
    // Ничего не пишет — эпоха котировки та же.
    let g = c.generation();
    assert!(!fold_head(&c, &r, 12, |_| None));
    assert_eq!(c.generation(), g);
    // Точка сделки на блоке 12 — итог с ней, блок замера остаётся 10.
    let with = check(HookImpact {
        buy_pct: buy[2].pct,
        buy: buy.clone().into(),
        ..Default::default()
    });
    assert!(fold_head(&c, &r, 12, |_| Some(with.clone())));
    assert_eq!(c.hook_mark(&r).0, Some(10));
    // Полный замер цикла на блоке 11 (между ними) — принимается.
    assert!(put_head(&c, &r, 11, &cycle, None));
    assert_eq!(c.hook_mark(&r).0, Some(11));
}

#[test]
fn b4_refusal_cuts_point_and_above_only() {
    let (_, buy, _, _) = gaib();
    let why = "в пуле не хватает ликвидности для пробы".to_string();
    let (kept, fail) = judge_side(buy.clone(), &[(buy[2].amount_in, why.clone())], true);
    assert_eq!((kept, fail), (buy[..2].to_vec(), None));
    let (kept, fail) = judge_side(buy.clone(), &[(buy[0].amount_in, why.clone())], true);
    assert!(kept.is_empty() && fail.is_some());
    let point = |s: &HookStep| Point {
        usd: s.usd,
        amount_in: s.amount_in,
    };
    let plan = Plan {
        buy: buy.iter().map(point).collect(),
        sell: vec![point(&buy[0])],
    };
    let one = HookImpact {
        sell: vec![buy[0].clone()].into(),
        buy_fail: Some(why.clone()),
        ..Default::default()
    };
    assert!(
        matches!(settled(Ok(Ok(one)), &plan), Ok(Ok(_))),
        "продажа работает — пул не выпадает"
    );
    let both = HookImpact {
        buy_fail: Some(why.clone()),
        sell_fail: Some(why),
        ..Default::default()
    };
    assert!(matches!(settled(Ok(Ok(both)), &plan), Ok(Err(_))));
}
