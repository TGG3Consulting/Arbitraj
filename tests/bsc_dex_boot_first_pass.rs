//! Файл: tests/bsc_dex_boot_first_pass.rs
//! Что делает: ревью арбитра 28.11.2026, С2 — сводки первого прохода не уходят до конца первого прохода справочника
//! подписки (directory/boot.rs). Живой бот 28.11.2026 (base_dex): справочник индексатора пуст (код поставщика 10006),
//! сводка налогов «прошло 0 … ждут 161» и сводка сверки ушли при нуле пулов, до 3025 пулов поиска по фабрикам.
//! - до начала подписки (тесты, площадка без подписки) проход окончен — как раньше;
//! - после начала: ждёт оба источника (индексатор, поиск по фабрикам) и обработки всех отправленных ими монет; окончен —
//!   «пул готов»;
//! - сводка налогов (check/tax_wait.rs First) держится, пока проход не окончен, и считает последний итог монеты;
//! - сводка сверки (check/fresh.rs settled) держится так же.
//!
//! Монеты — адреса CAKE и ETH из записи GeckoTerminal 2026-10-24 (как в tests/bsc_dex_tax_wait.rs).
//!
//! Кто вызывает: cargo test.
//! От чего зависит: tests/native_common, arb::venues::bsc_dex::{directory::boot, check::{tax_wait, tax_pass, fresh},
//! parts}.

mod native_common;

use std::time::Duration;

use alloy_primitives::{Address, address};

use arb::venues::bsc_dex::check::fresh::Fresh;
use arb::venues::bsc_dex::check::tax_pass::{Pass, Res};
use arb::venues::bsc_dex::check::tax_wait::First;
use arb::venues::bsc_dex::directory::boot::{self, Source};
use arb::venues::bsc_dex::parts::Parts;

const A: Address = address!("0x0E09FaBB73Bd3Ade0a17ECC321fD13a19e81cE82");
const B: Address = address!("0x2170Ed0880ac9A755fd29B2688956BD959F933F8");

#[tokio::test]
async fn first_pass_waits_for_directory_boot() {
    let parts = Parts::new(native_common::ctx("boot-first-pass").await);
    let (tx, _rx) = tokio::sync::mpsc::channel(16);
    let p = parts.pools(tx);
    let ctx = p.ctx.clone();
    let fresh = Fresh::default();
    // До начала подписки — как раньше: пулов нет, читать нечего — сводка сверки пора.
    assert!(boot::complete(&ctx));
    assert!(fresh.settled(&p));

    boot::start(&ctx, p.signals.ready.clone());
    assert!(!boot::complete(&ctx));
    assert!(!fresh.settled(&p), "справочник ещё не прошёл — сводки нет");
    // Индексатор кончил проход, отправив две монеты; поиск по фабрикам — ничего.
    let (dir_tx, _dir_rx) = tokio::sync::mpsc::unbounded_channel();
    boot::send(&ctx, &dir_tx, A);
    boot::send(&ctx, &dir_tx, B);
    boot::source_done(&ctx, Source::Indexer);
    boot::source_done(&ctx, Source::Factory);
    assert!(!boot::complete(&ctx), "монеты ещё не обработаны слушателем");
    boot::handled(&ctx, 1);
    assert!(!boot::complete(&ctx));
    let ready = p.signals.ready.notified();
    tokio::pin!(ready);
    ready.as_mut().enable();
    boot::handled(&ctx, 2);
    assert!(boot::complete(&ctx));
    assert!(
        tokio::time::timeout(Duration::from_millis(200), ready)
            .await
            .is_ok(),
        "окончен — «пул готов»"
    );
    assert!(fresh.settled(&p));
}

#[test]
fn tax_summary_held_until_boot_and_counts_last_result() {
    let mut f = First::new([A, B]);
    let p1 = Pass {
        done: vec![(A, Res::Wait), (B, Res::Wait)],
        ..Pass::default()
    };
    // Обе монеты проверены (пулов нет — ждут), но справочник не прошёл — сводки нет.
    assert_eq!(f.tally_when(&p1, |_| false, false), None);
    // Поздние пулы A пришли — перепроверка дала итог.
    let p2 = Pass {
        done: vec![(A, Res::Ok)],
        ..Pass::default()
    };
    assert_eq!(f.tally_when(&p2, |_| false, false), None);
    // Справочник прошёл — сводка по последнему итогу: прошла A, ждёт B.
    assert_eq!(
        f.tally_when(&Pass::default(), |_| false, true),
        Some([1, 0, 1, 0, 0])
    );
    assert_eq!(
        f.tally_when(&Pass::default(), |_| false, true),
        None,
        "сводка — один раз"
    );
}
