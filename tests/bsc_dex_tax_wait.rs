//! Файл: tests/bsc_dex_tax_wait.rs
//! Что делает: решения цикла налогов (check/tax_wait.rs, решение 63). Случай живого запуска 2026-10-25: у четырёх монет
//! (BEL, BROCCOLI, C98, THE) пулы дочитались последними — остальные монеты не должны их ждать. Монеты здесь — только
//! ключи: адреса CAKE и ETH и двух пулов CAKE из записи GeckoTerminal 2026-10-24.
//! - внеочередная проверка: ждущая своих пулов монета проверяется, как только её пулы дочитаны, и без сигнала;
//!   ждущая достоверного пула — только по сигналу и когда пул есть;
//! - сводка первого прохода: монета с вечно «загружающимися» пулами её не держит, «пулы ещё загружаются» —
//!   отдельным числом; сводка — один раз.
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
fn stuck_coin_does_not_hold_first_pass_summary() {
    let mut f = First::new([A, B, C, D]);
    // Проход 1: A прошла, C ждёт пула, B и D — пулы ещё загружаются.
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
        f.tally(&p1),
        Some([1, 0, 1, 0, 2]),
        "все, кроме ждущих своих пулов, проверены — сводка"
    );
    // D дочиталась и не прошла — сводка уже отправлена, второй нет; B так и висит — никого не держит.
    let p2 = Pass {
        done: vec![(D, Res::Failed)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p2), None);
}

#[test]
fn summary_waits_for_coin_that_was_never_checked() {
    let mut f = First::new([A, B]);
    // A проверена, B ещё ни разу не попадала в проход (не известно, ждёт ли она пулов) — сводки нет.
    let p1 = Pass {
        done: vec![(A, Res::Ok)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p1), None);
    let p2 = Pass {
        done: vec![(B, Res::Config)],
        ..Pass::default()
    };
    assert_eq!(f.tally(&p2), Some([1, 0, 0, 1, 0]));
}
