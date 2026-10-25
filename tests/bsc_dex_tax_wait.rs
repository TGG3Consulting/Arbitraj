//! Файл: tests/bsc_dex_tax_wait.rs
//! Что делает: решения цикла налогов (check/tax_wait.rs, решение 63). Случай живого запуска 2026-10-25: у четырёх монет
//! (BEL, BROCCOLI, C98, THE) пулы дочитались последними — остальные монеты не должны их ждать. Монеты здесь — только
//! ключи: адреса CAKE и ETH и двух пулов CAKE из записи GeckoTerminal 2026-10-24.
//! - внеочередная проверка: ждущая своих пулов монета проверяется, как только её пулы дочитаны, и без сигнала;
//!   ждущая достоверного пула — только по сигналу и когда пул есть;
//! - сводка первого прохода: монета с реально читающимися пулами её держит; монета, чьи пулы застряли (ждут
//!   незаданную настройку), — не держит и идёт отдельным числом «пулы не читаются (ждут настройку)»; сводка — один раз.
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::venues::bsc_dex::check::{tax_wait, tax_pass}.

use alloy_primitives::{Address, address};

use arb::venues::bsc_dex::check::tax_pass::{Pass, Res, Why};
use arb::venues::bsc_dex::check::tax_wait::{First, due};

/// Адреса из записи GeckoTerminal 2026-10-24: CAKE, ETH и два пула CAKE как ещё две «монеты».
const A: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
const B: Address = address!("0x2170Ed0880ac9A755fd29B2688956BD959F933F8");
const C: Address = address!("0x0ed7e52944161450477ee417de9cd3a859b14fd0");
const D: Address = address!("0x7f51c8aaa6b0599abd16674e2b17fec7a9f674a1");

#[test]
fn coin_with_own_pools_read_is_checked_right_away() {
    let waiting = vec![
        (A, "A".to_string(), Why::Loading),
        (B, "B".to_string(), Why::Loading),
        (C, "C".to_string(), Why::NoPool),
        (D, "D".to_string(), Why::Node),
    ];
    // У A свои пулы дочитаны, у B — нет; у C достоверный пул появился.
    let pending = |a: Address| if a == B { 3 } else { 0 };
    let valid = |t: &str| t == "C";
    // Без сигнала — только дочитавшие свои пулы.
    assert_eq!(due(&waiting, false, pending, valid), vec![(A, "A".into())]);
    // По сигналу «пул готов» — и ждущие пула, у которых он появился; сбой узла — не здесь (tax.retry_s).
    assert_eq!(
        due(&waiting, true, pending, valid),
        vec![(A, "A".into()), (C, "C".into())]
    );
}

#[test]
fn reading_coin_holds_first_pass_summary() {
    // Замечание ревью: все монеты в «загружаются» — сводка не должна уйти сразу. Пулы B и D реально читаются.
    let mut f = First::new([A, B, C, D]);
    let reading = |_: Address| false;
    let p1 = Pass {
        waiting: vec![
            (B, "B".into(), Why::Loading),
            (C, "C".into(), Why::NoPool),
            (D, "D".into(), Why::Loading),
        ],
        done: vec![(A, Res::Ok), (C, Res::Wait)],
        ..Pass::default()
    };
    assert_eq!(
        f.tally(&p1, reading),
        None,
        "B и D читаются — держат сводку"
    );
    let p2 = Pass {
        done: vec![(D, Res::Failed)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p2, reading), None, "B ещё читается");
    let p3 = Pass {
        done: vec![(B, Res::Ok)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p3, reading), Some([2, 1, 1, 0, 0]));
    assert_eq!(
        f.tally(&Pass::default(), reading),
        None,
        "сводка — один раз"
    );
}

#[test]
fn stuck_coin_does_not_hold_first_pass_summary() {
    let mut f = First::new([A, B, C, D]);
    // B застряла (все её пулы ждут незаданную настройку), D читается.
    let stuck = |a: Address| a == B;
    let p1 = Pass {
        waiting: vec![(B, "B".into(), Why::Loading), (D, "D".into(), Why::Loading)],
        done: vec![(A, Res::Ok), (C, Res::Wait)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p1, stuck), None, "D читается — держит");
    let p2 = Pass {
        done: vec![(D, Res::Failed)],
        ..Pass::default()
    };
    assert_eq!(
        f.tally(&p2, stuck),
        Some([1, 1, 1, 0, 1]),
        "остались только застрявшие — сводка, «пулы не читаются (ждут настройку)» отдельным числом"
    );
    let p3 = Pass {
        done: vec![(B, Res::Ok)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p3, stuck), None, "второй сводки нет");
}

#[test]
fn summary_waits_for_coin_that_was_never_checked() {
    let mut f = First::new([A, B]);
    // A проверена, B ещё ни разу не попадала в проход (не известно, ждёт ли она пулов) — сводки нет.
    let p1 = Pass {
        done: vec![(A, Res::Ok)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p1, |_| false), None);
    let p2 = Pass {
        done: vec![(B, Res::Config)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p2, |_| false), Some([1, 0, 0, 1, 0]));
}
