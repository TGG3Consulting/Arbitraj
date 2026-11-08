//! Файл: tests/core_sell_unsent.rs
//! Что делает: решение 154 на настоящей SQLite во временной папке (ядро без площадок в реестре, моков нет).
//! Живой случай 04.11.2026, сделка 20261104120743802ASTER (bingx → bsc_dex): 143.607219 ASTER зачислено на кошелёк,
//! продажа «свой сборщик: прямой» — «Ни один канал не принял транзакцию: … код -32000 …» (текст — из хронологии
//! сделки, тексты каналов там обрезаны), повтора не было, 4 минуты до кнопки человека, строка в хронологии — дважды.
//! Проверяется: (1) отказ всех каналов без факта транзакции → повтор с паузой stuck.reject_retry_pause_ms; с фактом
//! транзакции, кнопкой человека или без настройки — нет; (2) повтор уходит в режим застрявшей монеты и продаёт заново
//! с новой котировкой (здесь площадки нет — новая котировка честно «невозможна», сделка остановлена, но путь пройден);
//! (3) исчерпан stuck.unsent_retries_max → журнал Error core.sell_unsent_exhausted и алерт человеку, строка об отказе в
//! хронологии — одна; (5) решение 220: продажа здесь в режиме «Перегон» — порог stuck.relay_max_loss_pct (живой
//! случай BRIC: −30 %, −11 USDT по рынку без порога), не задан — алерт; нет котировки — алерт; (4) адаптер: где транзакция по ответу eth_getTransactionByHash (запись узла BSC 2026-10-25);
//! (6) ответы Тиграна 08.11 к решению 220: «вывод отклонён» (WithdrawRejected, текст Bitget 40938 из живых сделок CHEEMS,
//! AVA, RWA) не повторяется даже правилом Retry и в повторах возврата USDT; перевод не состоялся — монеты на месте:
//! продажа здесь, нет — остановка; нет котировки — ждём цену монеты до первого тика, не дольше stuck.no_quote_wait_ms.
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core, arb::settings, arb::storage, arb::notify, arb::secrets, arb::venues::bsc_dex::send.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use arb::core::Core;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::op_error::OpError;
use arb::core::sell_unsent::{self, Retry};
use arb::core::types::{Asset, Direction, VenueId};
use arb::gateway::errors::ErrorCode;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::{Journal, Level};
use arb::venues::bsc_dex::send::absent;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Текст отказа из хронологии сделки ASTER (как записан; тексты каналов в хронологии обрезаны).
const ASTER: &str = "[свой сборщик: прямой] Ни один канал не принял транзакцию: <канал>: … код -32000: …; <канал>: … код -32000: …; <канал>: … код -32000: …";

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-unsent-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро на базе в папке; возвращает ядро, настройки и журнал.
async fn start(d: &Path) -> (Core, Store, Journal) {
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
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    (core, settings, journal)
}

/// Задаёт общую настройку.
async fn set(s: &Store, name: &str, v: Value) {
    s.set(name, Scope::Global, Some(v), "тест").await.unwrap();
}

/// Сделка ASTER: монеты зачислены на кошелёк DEX, идёт продажа.
fn aster() -> Deal {
    let dir = Direction {
        asset: Asset {
            ticker: "ASTER".into(),
            bsc_contract: None,
        },
        buy_on: VenueId("bingx".into()),
        sell_on: VenueId("bsc_dex".into()),
    };
    let mut x = Deal::new(
        "20261104120743802ASTER".into(),
        dir,
        json!(null),
        Decimal::ZERO,
    );
    x.at = VenueId("bsc_dex".into());
    x.qty = Decimal::from_str("143.607219").unwrap();
    x.step = Step::Selling;
    x
}

/// Ошибка продажи: отказ всех каналов; sent — был ли факт транзакции в сети.
fn rejected(sent: bool) -> OpError {
    OpError {
        code: Some(ErrorCode::ChannelRejected),
        message: ASTER.into(),
        sent,
    }
}

/// Сделка по id из учёта ядра.
fn deal_of(core: &Core, id: &str) -> Deal {
    core.deals().into_iter().find(|d| d.id == id).unwrap()
}

#[tokio::test]
async fn decide_retries_only_unsent_channel_reject() {
    let d = dir("decide");
    let (core, s, _) = start(&d).await;
    let deal = aster();
    let e = rejected(false);
    // Как в живом случае до решения 154: повторы не настроены — алерт с причиной, почему бот сам не повторил.
    assert!(
        matches!(sell_unsent::decide(&core, &deal, &e, "выгодно"), Retry::Off(w) if w.contains("stuck.reject_retry_pause_ms"))
    );
    set(&s, "stuck.reject_retry_pause_ms", json!(1500)).await;
    assert!(
        matches!(sell_unsent::decide(&core, &deal, &e, "выгодно"), Retry::Off(w) if w.contains("stuck.unsent_retries_max"))
    );
    set(&s, "stuck.unsent_retries_max", json!(2)).await;
    assert_eq!(
        sell_unsent::decide(&core, &deal, &e, "выгодно"),
        Retry::Go { pause_ms: 1500 }
    );
    // Транзакция ушла в сеть — стартовое правило, не повторяем; кнопку человека — тоже (решение 110).
    assert_eq!(
        sell_unsent::decide(&core, &deal, &rejected(true), "выгодно"),
        Retry::No
    );
    assert_eq!(
        sell_unsent::decide(&core, &deal, &e, arb::core::sell::BUTTON),
        Retry::No
    );
    // Решение 110 не изменилось: отказ до отправки — без паузы.
    let quote = OpError {
        code: Some(ErrorCode::QuoteMismatch),
        message: "цена сборки хуже".into(),
        sent: false,
    };
    assert_eq!(
        sell_unsent::decide(&core, &deal, &quote, "выгодно"),
        Retry::Go { pause_ms: 0 }
    );
    let mut tired = aster();
    tired.unsent_retries = 2;
    assert_eq!(
        sell_unsent::decide(&core, &tired, &e, "выгодно"),
        Retry::Exhausted { max: 2 }
    );
}

#[tokio::test]
async fn retry_goes_to_new_sale_then_exhausted_alerts_once() {
    let d = dir("flow");
    let (core, s, journal) = start(&d).await;
    set(&s, "stuck.unsent_retries_max", json!(1)).await;
    set(&s, "stuck.reject_retry_pause_ms", json!(1)).await;
    set(&s, "stuck.mode", json!("wait")).await;
    set(&s, "stuck.wait_ms", json!(1)).await;
    let e = rejected(false);

    // Повтор: счётчик, запись, пауза, режим застрявшей монеты → продажа по рынку с новой котировкой.
    sell_unsent::retry(&core, aster(), &e, 1).await;
    let x = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(x.unsent_retries, 1);
    let lines: Vec<&str> = x.timeline.iter().map(|(_, t)| t.as_str()).collect();
    assert!(
        lines
            .iter()
            .any(|t| t.contains("повтор через 1 мс через режим застрявшей монеты (1/1)"))
    );
    // Решение 220: в хронологии — настоящая причина входа в режим застрявшей монеты, не «невыгодно».
    assert!(
        lines
            .iter()
            .any(|t| t.contains("Застрявшая монета (продажа не ушла в сеть:")
                && t.contains("режим wait")),
        "{lines:?}"
    );
    // Площадки в реестре нет — новая котировка невозможна: путь новой продажи пройден, сделка остановлена честно.
    assert!(lines.iter().any(|t| t.contains("продажа по рынку")));

    // Второй отказ: предел исчерпан → журнал Error и алерт человеку; строка об отказе — одна.
    let mut y = aster();
    y.unsent_retries = 1;
    let r = sell_unsent::decide(&core, &y, &e, "выгодно");
    assert_eq!(r, Retry::Exhausted { max: 1 });
    sell_unsent::alert(&core, y, &e, &r).await;
    let y = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(y.step, Step::WaitingHuman);
    let fails = y
        .timeline
        .iter()
        .filter(|(_, t)| t.contains("Ни один канал не принял"))
        .count();
    assert_eq!(
        fails, 1,
        "строка отказа в хронологии — одна: {:?}",
        y.timeline
    );
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("повторы исчерпаны") && t.contains("ждём решения человека"))
    );
    let log = journal.list(50, None).await.unwrap();
    assert!(
        log.iter()
            .any(|j| j.kind == "core.sell_unsent_exhausted" && matches!(j.level, Level::Error))
    );
}

#[test]
fn node_says_where_the_tx_is() {
    // Настоящий ответ узла BSC на eth_getTransactionByHash (вывод BNB с BingX, 2026-10-25): транзакция в блоке.
    let rec: Value = serde_json::from_str(
        &std::fs::read_to_string(
            "tests/data/native_2026-10-25/arb_bingx_bnb_withdraw_2026-10-25.json",
        )
        .unwrap(),
    )
    .unwrap();
    let tx = rec["results"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "tx")
        .unwrap()["response"]["result"]
        .clone();
    assert_eq!(absent::located(&tx).as_deref(), Some("в блоке 0x7b7d27d"));
    // Та же транзакция до блока: blockNumber = null (execution-apis transaction.yaml) — в пуле узла, повтор опасен.
    let mut pending = tx.clone();
    pending["blockNumber"] = Value::Null;
    assert_eq!(absent::located(&pending).as_deref(), Some("в пуле узла"));
    // Узел не знает транзакцию: result = null (execution-apis transaction.yaml) — точно не ушла.
    assert_eq!(absent::located(&Value::Null), None);
}

/// Решение 220, часть Б: чистое решение продажи здесь по цифрам живого случая BRIC (вложено 36.67, по рынку 25.67 —
/// −11 USDT, −30 %). Порог 20 % (совет Тиграна) — алерт; убыток 10 % — продажа по рынку; порог не задан — алерт с
/// причиной «не задана настройка»; нет котировки — алерт, убыток неизвестен.
#[test]
fn relay_sell_here_respects_loss_threshold() {
    use arb::core::relay_here::{Here, decide};
    let mut bric = aster();
    bric.invested = Decimal::from_str("36.67").unwrap();
    let d = |s: &str| Decimal::from_str(s).unwrap();
    let why = "перегон не выгоднее продажи здесь";
    match decide(&bric, why, Ok(d("20")), Ok(d("25.67"))) {
        Here::Alert { loss: Some(l), why } => {
            assert!(l > d("29.99") && l < d("30.01"), "{l}");
            assert!(
                why.contains("выше порога") && why.contains("stuck.relay_max_loss_pct"),
                "{why}"
            );
        }
        other => panic!("ждали алерт: {other:?}"),
    }
    assert!(
        matches!(decide(&bric, why, Ok(d("20")), Ok(d("33.003"))), Here::Market(t) if t.contains("не больше порога"))
    );
    assert!(matches!(
        decide(&bric, why, Err("не задано".into()), Ok(d("25.67"))),
        Here::Alert { loss: Some(_), why } if why.contains("не задана настройка stuck.relay_max_loss_pct")
    ));
    assert!(matches!(
        decide(&bric, why, Ok(d("20")), Err("нет стакана".into())),
        Here::Alert { loss: None, why } if why.contains("нет котировки: нет стакана")
    ));
}

/// Решение 220, часть Б: перегоны исчерпаны — раньше «продажа по рынку» без проверки убытка, теперь продажа здесь по
/// порогу; порог не задан — алерт человеку, сделка ждёт решения (ядро без площадок, моков нет).
#[tokio::test]
async fn relay_exhausted_without_threshold_alerts_human() {
    let d = dir("relay");
    let (core, s, _) = start(&d).await;
    set(&s, "stuck.mode", json!("relay")).await;
    set(&s, "stuck.max_relays", json!(1)).await;
    let mut x = aster();
    x.relays = 1;
    x.invested = Decimal::from(36);
    arb::core::relay::run(&core, x).await;
    let y = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(y.step, Step::WaitingHuman, "{:?}", y.timeline);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("перегонов 1 — исчерпаны")
                && t.contains("не задана настройка stuck.relay_max_loss_pct")),
        "{:?}",
        y.timeline
    );
}

/// Живой отказ Bitget 40938 (сделки CHEEMS, AVA, RWA; venues/bitget/errors.rs).
const BITGET_40938: &str = "Your account can only withdraw to addresses in the address book";

/// Решение 220 (Г1): WithdrawRejected не повторяется даже правилом Retry — журнал «вывод отклонён — не повторяем»;
/// то же правило Retry на другой код (WithdrawLocked) действует как раньше.
#[tokio::test]
async fn withdraw_rejected_is_never_retried_by_rules() {
    use arb::core::rules_apply::{self, Next};
    use arb::gateway::errors::GatewayError;
    use arb::gateway::rules::{Rule, RuleAction, RuleScope};
    let d = dir("rejected");
    let (core, _, journal) = start(&d).await;
    let v = VenueId("bitget".into());
    for event in [ErrorCode::WithdrawRejected, ErrorCode::WithdrawLocked] {
        let rule = Rule {
            id: 0,
            event,
            condition: None,
            action: RuleAction::Retry {
                attempts: 5,
                pause_ms: 1,
            },
            scope: RuleScope::All,
            enabled: true,
        };
        arb::storage::rules::create(&core.inner.deps.db, rule)
            .await
            .unwrap();
    }
    let e = GatewayError::new(ErrorCode::WithdrawRejected, Some(v.clone()), BITGET_40938);
    assert_eq!(
        rules_apply::on_error(&core, "Вывод", &v, Some("d1"), &e, 1).await,
        Next::Stop
    );
    let log = journal.list(50, None).await.unwrap();
    assert!(log.iter().any(|j| j.kind == "core.withdraw_rejected"
        && j.message.contains("вывод отклонён — не повторяем")));
    let locked = GatewayError::new(ErrorCode::WithdrawLocked, Some(v.clone()), "locked");
    assert_eq!(
        rules_apply::on_error(&core, "Вывод", &v, Some("d1"), &locked, 1).await,
        Next::Retry { pause_ms: 1 }
    );
}

/// Решение 220 (Г1): возврат USDT после WithdrawRejected не повторяется даже при заданных повторах (решение 163) —
/// сразу «USDT не вернулись» на площадке; отказ «заблокировано» (WithdrawLocked) — повтор, как раньше.
#[tokio::test]
async fn usdt_return_not_retried_after_withdraw_rejected() {
    use arb::core::usdt_return_state::UsdtReturn;
    let d = dir("usdt");
    let (core, s, journal) = start(&d).await;
    set(&s, "usdt.return_retry_s", json!(1)).await;
    set(&s, "usdt.return_retries_max", json!(3)).await;
    let x = aster();
    arb::core::ledger::save(&core, &x).await;
    let (from, to) = (VenueId("bitget".into()), VenueId("bingx".into()));
    let e = OpError {
        code: Some(ErrorCode::WithdrawRejected),
        message: BITGET_40938.into(),
        sent: false,
    };
    let amount = Decimal::from(83);
    let again = arb::core::usdt_return_retry::fail(&core, &x.id, amount, (&from, &to), 0, &e).await;
    assert!(!again, "повтора нет");
    let y = deal_of(&core, &x.id);
    assert!(
        matches!(&y.usdt_return, Some(UsdtReturn::NotReturned { at, why, .. }) if at == &from && why.contains("не повторяем")),
        "{:?}",
        y.usdt_return
    );
    let log = journal.list(50, None).await.unwrap();
    assert!(
        log.iter()
            .any(|j| j.message.contains("вывод отклонён — не повторяем"))
    );
    let locked = OpError {
        code: Some(ErrorCode::WithdrawLocked),
        message: "locked".into(),
        sent: false,
    };
    assert!(
        arb::core::usdt_return_retry::fail(&core, &x.id, amount, (&from, &to), 0, &locked).await
    );
}

/// Решение 220 (Г2): перевод не состоялся — монеты на месте → продажа здесь (количество — по балансу); нет монет или
/// баланс не получен → остановка с причиной.
#[test]
fn transfer_failed_sells_here_only_with_coins() {
    use arb::core::relay_here::{Place, place};
    let why = format!("перегон не состоялся: {BITGET_40938}");
    let q = Decimal::from_str("143.6").unwrap();
    assert_eq!(place(&why, Ok((q, None))), Place::SellHere(q, None));
    assert!(
        matches!(place(&why, Ok((Decimal::ZERO, None))), Place::Halt(t) if t.contains("монет на месте нет"))
    );
    assert!(matches!(
        place(&why, Err("на bitget нет свободных ASTER".into())),
        Place::Halt(t) if t.contains(BITGET_40938) && t.contains("нет свободных")
    ));
}

/// Решение 220 (Г2): перевод не состоялся, баланс площадки не получить (площадки в реестре нет) — сделка остановлена,
/// без режима застрявшей монеты и без повтора вывода.
#[tokio::test]
async fn transfer_failed_without_coins_halts() {
    let d = dir("tfail");
    let (core, s, _) = start(&d).await;
    set(&s, "stuck.relay_max_loss_pct", json!(20)).await;
    let why = format!("перевод не состоялся: {BITGET_40938}");
    arb::core::relay_here::transfer_failed(&core, aster(), &why).await;
    let y = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(y.step, Step::Halted, "{:?}", y.timeline);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains(BITGET_40938) && t.contains("монет на месте нет")),
        "{:?}",
        y.timeline
    );
}

/// Решение 220 (Г3): котировки нет — ждём цену монеты; первый же тик с котировкой кончает ожидание (не ждём предела);
/// без настройки — сразу; предел вышел — причина с тем, сколько ждали.
#[tokio::test]
async fn no_quote_waits_for_tick_or_timeout() {
    use arb::core::relay_here::quote_or_wait;
    use std::sync::atomic::{AtomicU32, Ordering};
    let d = dir("wait");
    let (core, _, _) = start(&d).await;
    let deal = aster();
    let calls = AtomicU32::new(0);
    // Первая котировка — нет стакана, после тика — есть.
    let c = &calls;
    let quote = move || async move {
        match c.fetch_add(1, Ordering::SeqCst) {
            0 => Err("нет стакана".to_string()),
            _ => Ok(Decimal::from(30)),
        }
    };
    let ticks = core.inner.coin_ticks.clone();
    let asset = deal.direction.asset.clone();
    tokio::spawn(async move {
        for _ in 0..50 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let _ = ticks.send(asset.clone());
        }
    });
    let t0 = std::time::Instant::now();
    assert_eq!(
        quote_or_wait(&core, &deal, Some(60_000), quote).await,
        Ok(Decimal::from(30))
    );
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(5),
        "кончилось на тике, а не на пределе"
    );
    let none = || async { Err::<Decimal, _>("нет стакана".to_string()) };
    let r = quote_or_wait(&core, &deal, None, none).await.unwrap_err();
    assert!(r.contains("stuck.no_quote_wait_ms не задана"), "{r}");
    let r = quote_or_wait(&core, &deal, Some(30), none)
        .await
        .unwrap_err();
    assert!(r.contains("ждали цену монеты 30 мс"), "{r}");
}

/// Решение 220 (Г3): продажа здесь без котировки (площадки нет) — ждали stuck.no_quote_wait_ms, затем алерт с причиной.
#[tokio::test]
async fn sell_here_without_quote_alerts_after_wait() {
    let d = dir("hwait");
    let (core, s, _) = start(&d).await;
    set(&s, "stuck.relay_max_loss_pct", json!(20)).await;
    set(&s, "stuck.no_quote_wait_ms", json!(40)).await;
    let mut x = aster();
    x.invested = Decimal::from(36);
    arb::core::relay_here::sell_here(&core, x, "перегон не выгоднее продажи здесь").await;
    let y = deal_of(&core, "20261104120743802ASTER");
    assert_eq!(y.step, Step::WaitingHuman, "{:?}", y.timeline);
    assert!(
        y.timeline
            .iter()
            .any(|(_, t)| t.contains("нет котировки") && t.contains("ждали цену монеты 40 мс")),
        "{:?}",
        y.timeline
    );
}
