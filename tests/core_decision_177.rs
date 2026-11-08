//! Файл: tests/core_decision_177.rs
//! Что делает: решение 177 в ядре на настоящей SQLite во временной папке (площадок в реестре нет, моков нет).
//! Живой случай 05.11.2026, сделка 20261105131254836GAIB (mexc → bsc_dex): отказ продажи по квотеру (текст — из журнала
//! сделки) по кругу: каждое нажатие «Продать и перегнать» — новый алерт, новое уведомление и +1 к risk.errors_in_row
//! bsc_dex (3 → 6 → 9) → пауза всей площадки DEX.
//! Проверяется: (1) отказ до отправки (QuoteMismatch без факта транзакции) не считается ошибкой площадки подряд — ни у
//! автоматики, ни у человека; (2) ручная команда не считается совсем, даже сбой с фактом транзакции; (3) автоматический
//! сбой после отправки — как раньше: на пределе risk.max_errors_in_row — пауза; (4) повторный ОДИНАКОВЫЙ отказ (числа
//! другие) по записи, с которой человек запустил продажу, — одна запись (тот же id, попытка 2), без нового события алерта;
//! другой отказ — новый алерт; (5) Telegram: вернувшаяся запись правит своё сообщение, итог продажи — правка ушедшей.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify.

use std::str::FromStr;
use std::time::Instant;

use alloy_primitives::B256;
use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::manual_repeat;
use arb::core::manual_rows::alert_record;
use arb::core::risk_count;
use arb::core::rules_apply;
use arb::core::types::{
    Asset, ChainTx, Direction, ManualAction, SendMethod, StuckAlert, TxOutcome, VenueId,
};
use arb::core::view::CoreEvent;
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::notify::mirror_state::MirrorState;
use arb::notify::mirror_track::is_sale;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::json;
use tokio::sync::broadcast;

/// Отказ продажи из живого случая (текст сверки — как в журнале). Второй — тот же текст, числа в нём изменены тестом:
/// так выглядит та же сверка на следующей попытке (суммы и % другие) — проверяется, что числа не в счёт.
const MISMATCH_1: &str = "[свой сборщик: прямой] pancakeswap-infinity-clmm GAIB: перед сделкой расчёт 42559024903490846447 против квотера 42489232805231828113: расхождение 0.1642583% > 0.1% (dex.quoter_max_diff_pct)";
const MISMATCH_2: &str = "[свой сборщик: прямой] pancakeswap-infinity-clmm GAIB: перед сделкой расчёт 42561210334905114218 против квотера 42491311089112934400: расхождение 0.1645012% > 0.1% (dex.quoter_max_diff_pct)";

/// Ядро на базе во временной папке; risk.max_errors_in_row bsc_dex = 3 (как в живом случае).
async fn start(name: &str) -> Core {
    let d = std::env::temp_dir().join(format!("arb-177-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    settings
        .set(
            "risk.max_errors_in_row",
            Scope::Venue("bsc_dex".into()),
            Some(json!(3)),
            "тест",
        )
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await
}

fn dex() -> VenueId {
    VenueId("bsc_dex".into())
}

/// Отказ сверки с квотером до отправки (факта транзакции нет).
fn refused() -> GatewayError {
    GatewayError::new(ErrorCode::QuoteMismatch, Some(dex()), MISMATCH_1)
}

/// Откат свопа в сети (факт транзакции есть). Хеш, nonce и блок — из записанного ответа узла BSC
/// (tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json).
fn reverted() -> GatewayError {
    let tx = ChainTx {
        hash: B256::from_str("0x84762200f2bce717c0725ffa926d43f57313c8279d38f2c88189deeaa6391df6")
            .unwrap(),
        nonce: 0x383d,
        channels: vec![],
        method: SendMethod::Single,
        outcome: TxOutcome::Reverted,
        block: Some(0x7b7d27d),
        gas_used: None,
        gas_bnb: None,
        tip_bnb: None,
        cancel_hash: None,
        cancel_gas_bnb: None,
    };
    GatewayError::new(
        ErrorCode::SwapReverted,
        Some(dex()),
        "Своп откатился в сети",
    )
    .with_tx(tx)
}

fn errors(core: &Core) -> u64 {
    core.control()
        .errors_in_row
        .get(&dex())
        .copied()
        .unwrap_or(0)
}

#[tokio::test]
async fn refusal_before_send_and_manual_do_not_count() {
    let core = start("count").await;
    assert!(!risk_count::counts(&refused()));
    assert!(risk_count::counts(&reverted()));
    // (1) Автоматика: отказ до отправки — 4 раза подряд, счётчик не растёт, паузы нет.
    for n in 1..=4 {
        rules_apply::on_error(&core, "Продажа", &dex(), Some("d1"), &refused(), n).await;
    }
    assert_eq!(errors(&core), 0);
    assert!(core.control().venue_paused.is_empty());
    // (2) Человек: ни отказ до отправки, ни сбой с фактом транзакции не считаются (живой случай: 3 → 6 → 9 → пауза).
    for e in [refused(), reverted(), reverted(), reverted()] {
        assert!(!risk_count::is_manual());
        risk_count::manual(async {
            assert!(risk_count::is_manual());
            rules_apply::on_error(&core, "Продажа", &dex(), Some("d1"), &e, 1).await;
        })
        .await;
    }
    assert_eq!(errors(&core), 0);
    assert!(core.control().venue_paused.is_empty());
    // Внутри ручной команды — работа автоматики (остаток после частичной продажи) снова считается.
    risk_count::manual(risk_count::auto(async {
        assert!(!risk_count::is_manual());
    }))
    .await;
}

#[tokio::test]
async fn auto_failure_after_send_counts_as_before() {
    let core = start("auto").await;
    for n in 1..=2 {
        rules_apply::on_error(&core, "Продажа", &dex(), Some("d1"), &reverted(), n).await;
    }
    assert_eq!(errors(&core), 2);
    assert!(core.control().venue_paused.is_empty());
    rules_apply::on_error(&core, "Продажа", &dex(), Some("d1"), &reverted(), 3).await;
    assert_eq!(
        core.control().venue_paused.get(&dex()).map(String::as_str),
        Some("3 ошибок подряд — предел 3")
    );
    // Пауза, поставленная ошибками, снимается кнопкой «Снять паузу» (решение 177, п. 4 — без изменений).
    core.resume_venue(&dex()).await;
    assert!(core.control().venue_paused.is_empty());
    assert_eq!(errors(&core), 0);
}

/// Сделка GAIB на кошельке DEX, ждёт решения человека.
fn gaib() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "GAIB".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: dex(),
    };
    let mut x = Deal::new(
        "20261105131254836GAIB".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = dex();
    x.qty = Decimal::from_str("2434.66").unwrap();
    x.step = Step::Selling;
    x
}

/// Алерты ядра (как их видит экран «Ручное управление»), старые первыми.
fn alerts(core: &Core) -> Vec<StuckAlert> {
    let mut v: Vec<StuckAlert> = arb::core::engine::lk(&core.inner.alerts)
        .values()
        .map(|(a, _)| a.clone())
        .collect();
    v.sort_by_key(|a| a.at);
    v
}

#[tokio::test]
async fn same_refusal_updates_one_record_without_new_alert() {
    assert!(manual_repeat::same(MISMATCH_1, MISMATCH_2));
    assert!(!manual_repeat::same(
        MISMATCH_1,
        "[свой сборщик: прямой] Ни один канал не принял транзакцию"
    ));
    let core = start("repeat").await;
    let mut rx = core.subscribe();
    // Первый отказ (повторы исчерпаны) — новый алерт.
    let why = |e: &str| format!("продажа не прошла: {e}");
    arb::core::manual::raise_alert(
        &core,
        gaib(),
        None,
        why(MISMATCH_1),
        Some(MISMATCH_1.into()),
    )
    .await;
    let first = alerts(&core);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].attempts, 1);
    // Человек нажал «Продать и перегнать» с этого алерта (manual::sell_now помнит его в сделке) — тот же отказ.
    let mut d = gaib();
    d.alert_was = Some(first[0].clone());
    arb::core::manual::raise_alert(&core, d, None, why(MISMATCH_2), Some(MISMATCH_2.into())).await;
    let second = alerts(&core);
    assert_eq!(second.len(), 1, "одна запись, не новая");
    assert_eq!(second[0].id, first[0].id);
    assert_eq!(second[0].attempts, 2);
    assert_eq!(second[0].at, first[0].at);
    let rec = alert_record(&second[0], Decimal::ZERO);
    assert!(rec.text.contains("Попыток с этим отказом: 2"));
    // Событие алерта (всплывающее «новый алерт») — только у первого.
    let mut fresh = 0;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, CoreEvent::StuckAlert(_)) {
            fresh += 1;
        }
    }
    assert_eq!(fresh, 1);
    // Другой отказ — новый алерт, как раньше.
    let mut d = gaib();
    d.alert_was = Some(second[0].clone());
    let other = "[свой сборщик: прямой] Своп откатился в сети";
    arb::core::manual::raise_alert(&core, d, None, why(other), Some(other.into())).await;
    assert_eq!(alerts(&core).len(), 2);
}

#[test]
fn telegram_edits_returned_record_and_final_outcome() {
    let a = StuckAlert {
        id: "0b6f3c2e-1d4a-4f7e-9a51-3c8d2e7f1a90".into(),
        deal_id: "20261105131254836GAIB".into(),
        venue: dex(),
        asset: Asset {
            ticker: "GAIB".into(),
            bsc_contract: None,
        },
        qty: Decimal::from_str("2434.66").unwrap(),
        loss_pct: None,
        reason: Some(format!("продажа не прошла: {MISMATCH_1}")),
        at: chrono::Utc::now(),
        error: Some(MISMATCH_1.into()),
        attempts: 1,
    };
    let rec = alert_record(&a, Decimal::ZERO);
    let mut st = MirrorState::default();
    st.apply(std::slice::from_ref(&rec));
    st.mark_sent(&rec.key, rec.text.clone(), 501, Instant::now());
    // Нажали «Продать и перегнать»: запись ушла, итог команды «запущена» — сообщение ждёт итога продажи.
    let act = ManualAction::SellNowReturn {
        alert_id: a.id.clone(),
    };
    assert!(is_sale(&act));
    assert_eq!(st.apply(&[]).gone.len(), 1);
    for (r, s) in st.take_closed(&act) {
        st.keep_finished(r, s);
    }
    // Тот же отказ: запись вернулась с тем же ключом и новым текстом — правка сообщения 501, нового сообщения нет.
    let mut again = a.clone();
    again.attempts = 2;
    again.reason = Some(format!("продажа не прошла: {MISMATCH_2}"));
    let rec2 = alert_record(&again, Decimal::ZERO);
    let diff = st.apply(std::slice::from_ref(&rec2));
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(st.sent[&rec2.key].msgs, vec![501]);
    assert!(st.unsent(std::slice::from_ref(&rec2)).is_empty());
    assert_eq!(st.waiting(std::slice::from_ref(&rec2)).len(), 1);
    assert!(st.take_finished(&a.deal_id).is_empty());
    // Продано: запись ушла, итог продажи находит её сообщение — один раз.
    st.apply(&[]);
    for (r, s) in st.take_closed(&act) {
        st.keep_finished(r, s);
    }
    let fin = st.take_finished(&a.deal_id);
    assert_eq!(fin.len(), 1);
    assert_eq!(fin[0].1.msgs, vec![501]);
    assert!(st.take_finished(&a.deal_id).is_empty());
}
