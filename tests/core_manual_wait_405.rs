//! Файл: tests/core_manual_wait_405.rs
//! Что делает: решение 405 (t191, ответ Тиграна 18.11 на ревью Н3, вариант «в») — ручная кнопка при идущих сделках
//! монеты ждёт их окончания (core/manual_wait.rs, manual_wait_run.rs) на настоящем ядре и SQLite во временной папке
//! (площадок в реестре нет, моков нет; монета и направление — записи CAKE, tests/size_common):
//! - «Продать сейчас» остановленной сделки при идущей сделке монеты — ответ «ждёт окончания сделок по CAKE: D1»,
//!   ожидание на экране (Core::manual — waits), новые сделки, расчёты и действия с монетой на паузе; идущая
//!   кончилась — та же команда выполняется заново (журнал core.manual_wait «выполняю», дальше — её обычный итог:
//!   площадки нет — отказ, сделка снова остановлена), пауза снята;
//! - держащая сделка застряла — сразу отказ с причиной, паузы нет; застряла во время ожидания — ожидание кончается
//!   с причиной;
//! - «Отменить ожидание» — действие не выполняется, пауза снята; вторая кнопка по той же монете — «уже ждёт».
//!
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, tests/size_common.

mod size_common;

use std::path::PathBuf;
use std::time::Duration as StdDuration;

use arb::core::Core;
use arb::core::coin_key::CoinKey;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::types::{CommandSource, ManualAction, ManualCommand, VenueId};
use arb::core::{coin_ride, manual_wait};
use arb::gateway::contract::VenueKind;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use chrono::Utc;
use serde_json::json;
use size_common::d;
use tokio::sync::broadcast;

/// Ядро на базе во временной папке; bingx, mexc — биржи, bsc_dex — DEX. Возвращает ядро и журнал.
async fn start(name: &str) -> (Core, Journal) {
    let dir: PathBuf =
        std::env::temp_dir().join(format!("arb-405mw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let db = Db::open(&dir.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings: Store = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault: arb::secrets::Vault::locked(dir.join("secrets.enc")),
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw: Registry::new(),
        settings,
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: dir.clone(),
    })
    .await;
    *core.inner.kinds.write().unwrap() = vec![
        (VenueId("bingx".into()), VenueKind::Cex),
        (VenueId("mexc".into()), VenueKind::Cex),
        (VenueId("bsc_dex".into()), VenueKind::Dex),
    ];
    (core, journal)
}

/// Направление CAKE A → B (записи tests/size_common).
fn dir_of(a: &str, b: &str) -> arb::core::types::Direction {
    let mut x = size_common::calc::direction();
    x.buy_on = VenueId(a.into());
    x.sell_on = VenueId(b.into());
    x
}

/// Сделка CAKE на шаге `step` с монетами `qty` на площадке покупки. Ошибок нет.
fn deal(id: &str, step: Step, qty: &str) -> Deal {
    let mut x = Deal::new(id.into(), dir_of("bingx", "mexc"), json!({}), d("1"));
    x.step = step;
    x.invested = d("40");
    x.qty = d(qty);
    x
}

/// Идущая сделка монеты так, как её ставит исполнитель (admit → учёт → board). Ошибок нет.
async fn riding(core: &Core, id: &str, step: Step) {
    let x = deal(id, step, "10");
    coin_ride::admit(core, &x.direction).unwrap();
    arb::core::ledger::save(core, &x).await;
    coin_ride::board(core, &x.direction, id, None);
}

/// Команда человека из интерфейса.
fn cmd(action: ManualAction) -> ManualCommand {
    ManualCommand {
        action,
        source: CommandSource::Ui,
        by: None,
        at: Utc::now(),
    }
}

/// Ждёт строку журнала core.manual_wait с `part`. Тест падает по сроку.
async fn logged(j: &Journal, part: &str) {
    for _ in 0..400 {
        let hit = j
            .list(500, None)
            .await
            .unwrap()
            .iter()
            .any(|e| e.kind == "core.manual_wait" && e.message.contains(part));
        if hit {
            return;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    panic!("нет строки журнала «{part}»");
}

#[tokio::test]
async fn button_waits_for_running_deal_then_runs() {
    let (core, j) = start("run").await;
    let key = CoinKey::of(&dir_of("bingx", "mexc").asset);
    arb::core::ledger::save(&core, &deal("H", Step::Halted, "5")).await;
    riding(&core, "D1", Step::Transfer).await;
    let sell = ManualAction::SellHalted {
        deal_id: "H".into(),
    };
    let text = core.command(cmd(sell.clone())).await.unwrap();
    assert!(text.contains("ждёт окончания сделок по CAKE: D1"), "{text}");
    let waits = manual_wait::list(&core);
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].what, "Продать сейчас");
    assert_eq!(core.manual()["waits"].as_array().unwrap().len(), 1);
    // Пауза новых сделок монеты: вход исполнителя, очередь расчёта, другое ручное действие.
    let other = dir_of("mexc", "bsc_dex");
    let why = coin_ride::admit(&core, &other).unwrap_err();
    assert!(
        why.contains("на паузе: ручное действие «Продать сейчас»"),
        "{why}"
    );
    assert!(coin_ride::blocks_new(&core, &other));
    assert!(!coin_ride::take(&core, &key));
    let again = core.command(cmd(sell)).await.unwrap_err();
    assert!(
        again.contains("уже ждёт ручное действие «Продать сейчас»"),
        "{again}"
    );
    let h = core.inner.deals.lock().unwrap().get("H").cloned().unwrap();
    assert_eq!(
        h.step,
        Step::Halted,
        "до выполнения — остановлена, как была"
    );
    // Идущая кончилась — команда выполняется заново (площадок нет — её обычный отказ), пауза снята.
    coin_ride::release(&core, &key, Some("D1"));
    logged(&j, "сделки по CAKE закончились — выполняю «Продать сейчас»").await;
    for _ in 0..400 {
        if manual_wait::list(&core).is_empty() && !core.inner.active.lock().unwrap().contains(&key)
        {
            break;
        }
        tokio::time::sleep(StdDuration::from_millis(5)).await;
    }
    assert!(manual_wait::list(&core).is_empty());
    assert!(!coin_ride::blocks_new(&core, &other));
}

#[tokio::test]
async fn stuck_holder_refuses_at_once_and_while_waiting() {
    let (core, j) = start("stuck").await;
    let key = CoinKey::of(&dir_of("bingx", "mexc").asset);
    arb::core::ledger::save(&core, &deal("H", Step::Halted, "5")).await;
    riding(&core, "D1", Step::Stuck).await;
    let back = ManualAction::SellHalted {
        deal_id: "H".into(),
    };
    let why = core.command(cmd(back.clone())).await.unwrap_err();
    assert!(why.contains("монету держит сделка D1 (шаг: "), "{why}");
    assert!(why.contains("окончания не будет"), "{why}");
    assert!(manual_wait::list(&core).is_empty(), "паузы нет");
    // Во время ожидания держащая ушла с пути — ожидание кончается с причиной, пауза снята.
    coin_ride::release(&core, &key, Some("D1"));
    riding(&core, "D2", Step::Selling).await;
    assert!(core.command(cmd(back)).await.is_ok());
    let stuck = deal("D2", Step::Stuck, "10");
    arb::core::ledger::save(&core, &stuck).await;
    logged(&j, "ожидание прервано: монету держит сделка D2").await;
    assert!(manual_wait::list(&core).is_empty());
}

#[tokio::test]
async fn cancel_wait_does_not_run_action() {
    let (core, j) = start("cancel").await;
    let key = CoinKey::of(&dir_of("bingx", "mexc").asset);
    arb::core::ledger::save(&core, &deal("H", Step::Halted, "5")).await;
    riding(&core, "D1", Step::Selling).await;
    core.command(cmd(ManualAction::SellHalted {
        deal_id: "H".into(),
    }))
    .await
    .unwrap();
    let id = manual_wait::list(&core)[0].id.clone();
    let ok = core
        .command(cmd(ManualAction::CancelManualWait { id: id.clone() }))
        .await
        .unwrap();
    assert!(ok.contains("отменено"), "{ok}");
    logged(&j, "ожидание отменено — действие не выполнено").await;
    assert!(manual_wait::list(&core).is_empty());
    // Пауза снята; идущая сделка — прежний отказ входа (одна сделка на монету без докупки).
    let why = coin_ride::admit(&core, &dir_of("mexc", "bsc_dex")).unwrap_err();
    assert!(why.starts_with("по монете идёт другая сделка D1"), "{why}");
    let h = core.inner.deals.lock().unwrap().get("H").cloned().unwrap();
    assert_eq!(h.step, Step::Halted);
    assert!(
        core.command(cmd(ManualAction::CancelManualWait { id }))
            .await
            .is_err()
    );
    coin_ride::release(&core, &key, Some("D1"));
}

/// Ревью 4, Б1: обычный конец держащей сделки — ledger::finish сохраняет «закрыта» (будит ожидание) раньше, чем задача
/// снимает запись в идущих: ожидание НЕ кончается отказом, ждёт снятия записи и выполняет команду.
#[tokio::test]
async fn normal_finish_then_release_keeps_waiting() {
    let (core, j) = start("finish").await;
    let key = CoinKey::of(&dir_of("bingx", "mexc").asset);
    arb::core::ledger::save(&core, &deal("H", Step::Halted, "5")).await;
    riding(&core, "D1", Step::Selling).await;
    core.command(cmd(ManualAction::SellHalted {
        deal_id: "H".into(),
    }))
    .await
    .unwrap();
    // Реальный порядок: finish(Closed) → сохранение будит ожидание → только потом release (ride_guard).
    let mut d1 = core.inner.deals.lock().unwrap().get("D1").cloned().unwrap();
    d1.qty = d("0");
    arb::core::ledger::finish(&core, &mut d1, Step::Closed, "продано").await;
    tokio::time::sleep(StdDuration::from_millis(50)).await;
    assert_eq!(
        manual_wait::list(&core).len(),
        1,
        "закрытая — не застрявшая"
    );
    let rows = j.list(500, None).await.unwrap();
    assert!(
        !rows
            .iter()
            .any(|e| e.kind == "core.manual_wait" && e.message.contains("прервано")),
        "отказа «окончания не будет» нет"
    );
    coin_ride::release(&core, &key, Some("D1"));
    logged(&j, "сделки по CAKE закончились — выполняю «Продать сейчас»").await;
}

/// Ревью 4, Н3: сделки кончились, повтор команды идёт — пауза держится, пока повтор не возьмёт монету (take внутри
/// той же команды снимает ожидание); посторонний take и вход сделки в это время — отказ.
#[tokio::test]
async fn pause_holds_until_retry_takes_coin() {
    let (core, _j) = start("hold").await;
    let key = CoinKey::of(&dir_of("bingx", "mexc").asset);
    arb::core::ledger::save(&core, &deal("H", Step::Halted, "5")).await;
    // Монету держит замок целиком (вход сделки / другое действие) — задача ожидания ждёт его снятия.
    core.inner.active.lock().unwrap().insert(key.clone());
    let sell = ManualAction::SellHalted {
        deal_id: "H".into(),
    };
    core.command(cmd(sell.clone())).await.unwrap();
    // «Повтор идёт» (задача ожидания ставит его, когда монета свободна); замок снимается без пробуждения ожидания,
    // чтобы задача не опередила проверку — пауза и её снятие проверяются через claim.
    assert_eq!(
        manual_wait::start_retry(&core, &key).as_deref(),
        Some("CAKE")
    );
    let other = dir_of("mexc", "bsc_dex");
    core.inner.active.lock().unwrap().remove(&key);
    assert!(
        coin_ride::admit(&core, &other)
            .unwrap_err()
            .contains("на паузе")
    );
    assert!(!coin_ride::take(&core, &key), "посторонний take — пауза");
    // Другая команда (другое действие) — не повтор.
    let wrong = manual_wait::scope(
        cmd(ManualAction::ReturnToBot {
            holding_id: "x".into(),
        }),
        async { coin_ride::take(&core, &key) },
    )
    .await;
    assert!(!wrong);
    // Повтор той же команды берёт монету — ожидание снято вместе с take.
    let took = manual_wait::scope(cmd(sell), async { coin_ride::take(&core, &key) }).await;
    assert!(took);
    assert!(manual_wait::list(&core).is_empty());
    assert!(!manual_wait::paused(&core, &key));
    coin_ride::free_active(&core, &key);
}
