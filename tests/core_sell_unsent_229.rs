//! Файл: tests/core_sell_unsent_229.rs
//! Что делает: решение 229 на настоящей SQLite во временной папке (ядро без площадок в реестре, моков нет).
//! Живой случай 08.11.2026, сделка JCT (продажа на bsc_dex): «Транзакция не попала в блок за send.max_wait_blocks,
//! tx … — nonce занят отменой … — ждём решения человека (алерт)» (текст — из хронологии сделки; хеши в разборе не
//! сохранились — здесь нулевые); затем кнопка «Продать сейчас»: проба откатилась TooLittleReceived до отправки —
//! снова человеку.
//! Проверяется: (1) TxNotIncluded с отменой, занявшей nonce в блоке (факт транзакции: исход NotIncluded и хеш отмены —
//! так их ставит адаптер, venues/bsc_dex/exec/chain_tx.rs), — повтор; без подтверждённой отмены и «исход неизвестен» —
//! человеку; (2) кнопка человека: отказ до отправки — повтор, прочее — человеку; (3) цепочка решения 229 при
//! исчерпании — продажа здесь по порогу один раз, затем алерт; (4) повтор метит цепочку, причина входа в режим
//! застрявшей монеты — «не попала в блок и отменена».
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::gateway, arb::settings, arb::storage, arb::notify, arb::secrets.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use alloy_primitives::B256;
use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::sell::BUTTON;
use arb::core::sell_unsent::{self, Retry};
use arb::core::stuck_reason::StuckReason;
use arb::core::types::{Asset, ChainTx, Direction, SendMethod, TxOutcome, VenueId};
use arb::gateway::errors::{ErrorCode, GatewayError};
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Текст отказа из хронологии сделки JCT (как записан, хеши обрезаны).
const JCT: &str = "[свой сборщик: прямой] Транзакция не попала в блок за send.max_wait_blocks, tx … — nonce занят отменой …";

/// Текст отказа пробы после кнопки человека (JCT 08.11).
const PROBE: &str = "[свой сборщик: прямой] проба свопа откатилась: TooLittleReceived";

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-229-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе в папке; возвращает ядро и настройки.
async fn start(d: &Path) -> (Core, Store) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(d.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings: settings.clone(),
        journal,
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings)
}

/// Задаёт общую настройку.
async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Сделка JCT: монеты на кошельке DEX, идёт продажа.
fn jct() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "JCT".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("mexc".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new("20261108JCT".into(), dir, json!(null), Decimal::ZERO);
    x.at = VenueId("bsc_dex".into());
    x.qty = Decimal::from_str("1000").unwrap();
    x.invested = Decimal::from(10);
    x.step = Step::Selling;
    x
}

/// Ошибка шлюза «не попала в блок» с фактом транзакции; outcome и отмена — как их ставит адаптер.
fn not_included(code: ErrorCode, outcome: TxOutcome, cancel: Option<B256>) -> OpError {
    let tx = ChainTx {
        hash: B256::ZERO,
        nonce: 7,
        channels: vec![],
        method: SendMethod::Single,
        outcome,
        block: None,
        gas_used: None,
        gas_bnb: None,
        tip_bnb: None,
        cancel_hash: cancel,
        cancel_gas_bnb: None,
    };
    let g = GatewayError::new(code, Some(VenueId("bsc_dex".into())), JCT).with_tx(tx);
    OpError::from(&g)
}

/// Отказ пробы до отправки (SwapReverted без факта транзакции).
fn probe() -> OpError {
    OpError::from(&GatewayError::new(
        ErrorCode::SwapReverted,
        Some(VenueId("bsc_dex".into())),
        PROBE,
    ))
}

#[tokio::test]
async fn cancel_confirmed_and_button_presend_retry() {
    let d = dir("decide");
    let (core, s) = start(&d).await;
    set(&s, "stuck.unsent_retries_max", json!(2)).await;
    let deal = jct();
    let one = Some(B256::repeat_byte(1));
    let cancelled = not_included(ErrorCode::TxNotIncluded, TxOutcome::NotIncluded, one);
    assert!(cancelled.sent && cancelled.cancel_confirmed());
    // (1) Отмена подтверждена квитанцией — исход известен: повтор без паузы.
    assert_eq!(
        sell_unsent::decide(&core, &deal, &cancelled, "выгодно"),
        Retry::Go { pause_ms: 0 }
    );
    // Без отмены в блоке — как раньше, человеку; «исход неизвестен» — тем более.
    let bare = not_included(ErrorCode::TxNotIncluded, TxOutcome::NotIncluded, None);
    assert!(!bare.cancel_confirmed());
    assert_eq!(
        sell_unsent::decide(&core, &deal, &bare, "выгодно"),
        Retry::No
    );
    let unknown = not_included(ErrorCode::OutcomeUnknown, TxOutcome::Replaced, None);
    assert_eq!(
        sell_unsent::decide(&core, &deal, &unknown, "выгодно"),
        Retry::No
    );
    // (2) Кнопка человека: отказ до отправки — повтор; ушедшая в сеть — человеку, как раньше.
    assert_eq!(
        sell_unsent::decide(&core, &deal, &probe(), BUTTON),
        Retry::Go { pause_ms: 0 }
    );
    assert_eq!(
        sell_unsent::decide(&core, &deal, &cancelled, BUTTON),
        Retry::No
    );
    // (4) Причина входа в режим застрявшей монеты.
    assert!(matches!(StuckReason::of_sell(&cancelled), StuckReason::Cancelled(t) if t == JCT));
}

#[tokio::test]
async fn chain_229_sells_here_once_then_alerts() {
    let d = dir("here");
    let (core, s) = start(&d).await;
    set(&s, "stuck.unsent_retries_max", json!(1)).await;
    // (3) Исчерпание: продажа здесь у любой цепочки (Тигран 08.11), не только у помеченной случаем 229.
    let mut x = jct();
    x.unsent_retries = 1;
    assert_eq!(
        sell_unsent::decide(&core, &x, &probe(), "выгодно"),
        Retry::Here { max: 1 }
    );
    assert_eq!(
        sell_unsent::decide(&core, &x, &probe(), BUTTON),
        Retry::Here { max: 1 }
    );
    x.unsent_here = true;
    assert_eq!(
        sell_unsent::decide(&core, &x, &probe(), "выгодно"),
        Retry::Here { max: 1 }
    );
    // Продажа здесь: порог stuck.relay_max_loss_pct не задан — алерт с причиной; счётчик выше предела.
    sell_unsent::here(&core, x, &probe(), 1).await;
    let y = core
        .deals()
        .into_iter()
        .find(|d| d.id == "20261108JCT")
        .unwrap();
    assert_eq!(y.step, Step::WaitingHuman, "{:?}", y.timeline);
    assert_eq!(y.unsent_retries, 2);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t
                .contains("продажа здесь по порогу stuck.relay_max_loss_pct (решение 229)")),
        "{:?}",
        y.timeline
    );
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("не задана настройка stuck.relay_max_loss_pct")),
        "{:?}",
        y.timeline
    );
    // Второй раз продажи здесь нет — алерт.
    assert_eq!(
        sell_unsent::decide(&core, &y, &probe(), BUTTON),
        Retry::Exhausted { max: 1 }
    );
}

#[tokio::test]
async fn retry_marks_chain() {
    let d = dir("mark");
    let (core, s) = start(&d).await;
    set(&s, "stuck.unsent_retries_max", json!(1)).await;
    let one = Some(B256::repeat_byte(1));
    let e = not_included(ErrorCode::TxNotIncluded, TxOutcome::NotIncluded, one);
    // stuck.mode не задан — сделка остановлена после записи повтора: путь пройден, метка в учёте.
    sell_unsent::retry(&core, jct(), &e, "выгодно", 0).await;
    let y = core
        .deals()
        .into_iter()
        .find(|d| d.id == "20261108JCT")
        .unwrap();
    assert!(y.unsent_here && y.unsent_retries == 1);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t
                .contains("продажа не попала в блок, nonce занят отменой (исход известен)")),
        "{:?}",
        y.timeline
    );
}
