//! Файл: tests/bsc_dex_hook_ladder_review3_1117.rs
//! Что проверяет: ревью 3 решения «335 (лесенка влияния)» на досъёмке узла 17.11 (tests/data/hook_pools_ticks_2026-11-17,
//! блок 134000292; GAIB 0x5c1b: влияние покупки 0 / 0,099 / 0,674 % на 1 / 20 / 100 USDT):
//! 1. Б3 — цикл пробы на пути сделки (exec/hook_drive.rs Drive, TradeHooks — та же логика, что у exec/hook_trade.rs)
//!    без hooks.impact_points: покупка «с конца» (точный выход) через пул GAIB на ~60 USDT при влиянии «как до
//!    лесенки» (одно число — влияние на 20 USDT, check.probe_usdt) не отклоняется: проба на оценке → вход вырос →
//!    проба с подъёмом → вход покрыт фактом сделки за две пробы; итог влияния — факт записи. Расчёт пути — наш обмен
//!    по карте тиков записи (math/swap_cl.rs) с HookCurve::settle; ответ квотера на сумме пробы — факт записи на
//!    ближайшей записанной сумме не меньше (допущение «влияние не убывает с суммой» — то же, на котором стоит лесенка;
//!    квотера на промежуточных суммах в записи нет).
//! 2. Н-а — точка пробы перед сделкой (check/hook_store.rs fold_head) не продвигает блок записанного замера: полный
//!    замер цикла на блоке между ними принимается (put_head); fold_head, который ничего не пишет, не сбрасывает
//!    эпоху котировки (решение 352, Checks::generation).
//! 3. Б4 — отказ квотера на точке (check/hook_fold.rs judge_side): эта точка и большие суммы стороны не измерены,
//!    меньшие работают; не осталось точек — «не прошла» только сторона; пул целиком — обе (check/hooks_probe.rs
//!    settled). Текст отказа — квотер записи 16.11 (BLESS 0x6e398c95, NotEnoughLiquidity) — в tests/
//!    bsc_dex_hook_pools_1116_rules.rs; здесь — правило на лесенке GAIB.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/hook_ticks_common, arb::venues::bsc_dex::{exec::hook_drive, check::{hook_fold, hook_store,
//! hooks_probe, state}, quote::hook_curve}.

mod hook_ticks_common;

use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::{Address, B256, U256};
use rust_decimal::Decimal;

use arb::core::types::{
    Asset, CheckState, HookGroup, HookImpact, HookStep, PoolCheck, PoolRef, VenueId,
};
use arb::venues::bsc_dex::check::hook_fold::judge_side;
use arb::venues::bsc_dex::check::hook_store::{fold_head, put_head};
use arb::venues::bsc_dex::check::hooks_probe::{Plan, Point, impact, settled};
use arb::venues::bsc_dex::check::state::Checks;
use arb::venues::bsc_dex::exec::hook_drive::{Drive, Fresh, Next, TradeHooks, Want};
use arb::venues::bsc_dex::quote::hook_curve::HookCurve;
use hook_ticks_common::{Pool, load, map, ours, pools, quotes};

fn keep(pct: Decimal) -> Decimal {
    Decimal::ONE - pct / Decimal::ONE_HUNDRED
}

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

#[test]
fn b3_exact_out_without_points_not_lost() {
    let (p, buy, z, r) = gaib();
    let m = map(&p);
    let flat = buy[1].pct;
    // Монеты, которые наш обмен без крючка даёт за ~60 USDT (середина между точками записи 20 и 100).
    let sixty = (buy[1].amount_in + buy[2].amount_in) / U256::from(2u64);
    let qty = Decimal::from_str(&ours(&p, &m, z, true, sixty, p.lp).amount_out.to_string())
        .expect("монеты");
    let want = |th: &TradeHooks| -> Want {
        let (steps, flat2, _) = th.curve(&r, true, &[], flat);
        let c = HookCurve {
            steps: (!steps.is_empty()).then(|| Arc::new(steps.clone())),
            limit: None,
            open: true,
        };
        let (pct, x) = c
            .settle(
                flat2,
                |pct| {
                    let out = U256::from_str(&(qty / keep(pct)).ceil().normalize().to_string())
                        .map_err(|e| e.to_string())?;
                    let s = ours(&p, &m, z, false, out, p.lp);
                    Ok::<_, String>((s.amount_in, s.amount_in))
                },
                |w| w,
            )
            .expect("расчёт");
        Want {
            pool: r.clone(),
            buy: true,
            x,
            applied: pct,
            steps,
        }
    };
    let (mut drive, mut th) = (Drive::new(None), TradeHooks::default());
    let mut w = want(&th);
    let fin = loop {
        match drive.next(&mut th, std::slice::from_ref(&w)) {
            Next::Done => break w,
            Next::Eval => {}
            Next::Probe(need) => {
                for n in need {
                    let rec = buy
                        .iter()
                        .find(|s| s.amount_in >= n.x)
                        .expect("запись покрывает сумму пробы");
                    th.fresh.push(Fresh {
                        pool: r.clone(),
                        buy: true,
                        step: HookStep {
                            amount_in: n.x,
                            usd: Decimal::ZERO,
                            pct: rec.pct,
                            quoter_out: None,
                        },
                        est: n.applied,
                    });
                }
            }
            Next::Fail(why) => panic!("сделка потеряна: {why}"),
        }
        w = want(&th);
    };
    assert_eq!(drive.probes(), 2, "проба на оценке и проба с подъёмом");
    assert!(fin.x <= buy[2].amount_in);
    assert_eq!(
        fin.applied, buy[2].pct,
        "итог — факт записи на 100 USDT, не одно число на 20 USDT"
    );
    assert!(fin.applied > flat);
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
        buy: buy[..2].to_vec(),
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
        buy: buy.clone(),
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
        sell: vec![buy[0].clone()],
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
