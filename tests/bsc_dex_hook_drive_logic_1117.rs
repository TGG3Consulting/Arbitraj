//! Файл: tests/bsc_dex_hook_drive_logic_1117.rs
//! Что проверяет: ЧИСТАЯ ЛОГИКА переходов Drive (exec/hook_drive.rs) — круги пробы крючков на пути сделки. Это не тест
//! «на записи» (ревью 4 лесенки, Н8): ответа квотера на промежуточных суммах (~60 USDT) в записях узла нет, поэтому
//! ответ «квотера» здесь — условная ступенчатая зависимость (влияние 0,099 % до 20 условных USDT и 0,674 % до 100 —
//! числа как у GAIB на 20 / 100 USDT в записи 17.11, но значения между ними — модель, не факт). Расчёт входа — тоже
//! модель: точный выход «с конца», вход = база / (1 − влияние/100). Что дозаписать на сервере, чтобы перевести Б3 на
//! запись, — docs/TASKS.md (квотер GAIB на промежуточных суммах).
//! 1. Б3 (ревью 3): без лесенки хранилища покупка «с конца» не теряется — проба на оценке → вход вырос → проба с
//!    подъёмом → покрыто фактом за две пробы; итог — факт (0,674 %), не одно число (0,099 %).
//! 2. Б5 (ревью 4): Drive конечен — сценарий ревью (лесенка хранилища 45 → 0,30 %, 100 → 0,70 %, пул сдвинулся).
//! 3. Н4 (ревью 4): кругов пробы — два (сам способ) без hooks.trade_probe_rounds_max и при значении меньше двух;
//!    больше двух — ровно заданное число; дальше — отказ сделке с причиной.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::{exec::hook_drive, quote::hook_curve}, arb::core::types.

use std::str::FromStr;
use std::sync::Arc;

use alloy_primitives::{B256, U256};
use rust_decimal::Decimal;

use arb::core::types::{HookStep, PoolRef};
use arb::venues::bsc_dex::exec::hook_drive::{Drive, Fresh, Next, TradeHooks, Want};
use arb::venues::bsc_dex::quote::hook_curve::HookCurve;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).expect("число")
}

fn pool() -> PoolRef {
    PoolRef::Singleton {
        manager: alloy_primitives::Address::ZERO,
        id: B256::ZERO,
    }
}

/// Условная единица «USDT» модели — 10^6 минимальных единиц.
fn usd(v: Decimal) -> U256 {
    U256::from_str(&(v * d("1000000")).ceil().normalize().to_string()).expect("единицы")
}

fn step(x: U256, pct: Decimal) -> HookStep {
    HookStep {
        amount_in: x,
        usd: Decimal::ZERO,
        pct,
        coins: None,
        quoter_out: None,
    }
}

/// Модель квотера (условная ступенька, см. шапку): влияние на входе x.
fn quoter(x: U256) -> Decimal {
    if x <= usd(d("20")) {
        d("0.099")
    } else {
        d("0.674")
    }
}

#[test]
fn b3_exact_out_two_probes_model() {
    let r = pool();
    let flat = d("0.099");
    let base = d("60");
    let want = |th: &TradeHooks| -> Want {
        let (steps, flat2, _) = th.curve(&r, true, &[], flat);
        let c = HookCurve {
            steps: (!steps.is_empty()).then(|| Arc::new(steps.clone())),
            limit: None,
            open: true,
            margin: None,
        };
        let (pct, x) = c
            .settle(
                flat2,
                |pct| {
                    let x = usd(base / (Decimal::ONE - pct / Decimal::ONE_HUNDRED));
                    Ok::<_, String>((x, x))
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
                    th.fresh.push(Fresh {
                        pool: r.clone(),
                        buy: true,
                        step: step(n.x, quoter(n.x)),
                        est: n.applied,
                    });
                }
            }
            Next::Fail(why) => panic!("сделка потеряна: {why}"),
        }
        w = want(&th);
    };
    assert_eq!(drive.probes(), 2, "проба на оценке и проба с подъёмом");
    assert!(fin.x <= usd(d("100")));
    assert_eq!(fin.applied, d("0.674"), "итог — факт пробы, не одно число");
}

/// Ревью 4 (Б5): старый Drive чередовал «подъём → покрыто → пересчёт → не покрыто → подъём» без счёта кругов; новый —
/// после подъёма на том же числе проб пробует непокрытый вход сразу. Число шагов теста ограничено.
#[test]
fn b5_drive_is_finite() {
    let r = pool();
    let at = |c: u64| usd(Decimal::from(c) / Decimal::ONE_HUNDRED);
    let stored = vec![step(at(4500), d("0.30")), step(at(10000), d("0.70"))];
    let (mut drive, mut th) = (Drive::new(None), TradeHooks::default());
    let mut done = false;
    for _ in 0..20 {
        let (steps, applied, fact) = th.curve(&r, true, &stored, d("0.70"));
        let lifted = !th.fresh.is_empty() && !fact;
        let x = match (th.fresh.len(), lifted) {
            (0, _) => at(4523),
            (_, true) => at(4502),
            (_, false) => at(4498),
        };
        let w = Want {
            pool: r.clone(),
            buy: true,
            x,
            applied,
            steps,
        };
        match drive.next(&mut th, std::slice::from_ref(&w)) {
            Next::Done => {
                done = true;
                break;
            }
            Next::Eval => {}
            Next::Probe(need) => {
                for n in need {
                    th.fresh.push(Fresh {
                        pool: r.clone(),
                        buy: true,
                        step: step(n.x, d("0.40")),
                        est: n.applied,
                    });
                }
            }
            Next::Fail(why) => panic!("сделка потеряна: {why}"),
        }
    }
    assert!(done, "Drive не завершился за 20 шагов");
    assert_eq!(drive.probes(), 2);
}

/// Сколько проб сделает Drive, если вход каждый раз вырастает за все точки сделки, и с какой причиной откажет.
fn rounds(limit: Option<u64>) -> (u64, String) {
    let r = pool();
    let (mut drive, mut th) = (Drive::new(limit), TradeHooks::default());
    for k in 1..100u64 {
        let w = Want {
            pool: r.clone(),
            buy: true,
            x: U256::from(k * 1000),
            applied: Decimal::ZERO,
            steps: Vec::new(),
        };
        match drive.next(&mut th, std::slice::from_ref(&w)) {
            Next::Fail(why) => return (drive.probes(), why),
            Next::Done => panic!("непокрытый вход не может быть итогом"),
            Next::Eval => {}
            Next::Probe(need) => {
                for n in need {
                    th.fresh.push(Fresh {
                        pool: r.clone(),
                        buy: true,
                        step: step(n.x, Decimal::ONE),
                        est: n.applied,
                    });
                }
            }
        }
    }
    panic!("Drive не остановился");
}

#[test]
fn n4_rounds_not_from_ladder_points() {
    assert_eq!(rounds(None).0, 2, "не задана — ровно два круга");
    assert_eq!(rounds(Some(1)).0, 2, "меньше двух — два (сам способ)");
    assert_eq!(rounds(Some(3)).0, 3);
    let (_, why) = rounds(None);
    assert!(why.contains("hooks.trade_probe_rounds_max"), "{why}");
}
