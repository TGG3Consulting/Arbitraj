//! Файл: tests/core_balance_sell.rs
//! Что делает: решение 174 — кнопка «Продать» панели «Балансы» на настоящем ядре и настоящем адаптере BingX без сети
//! (ключей нет — хранилище секретов заблокировано), записанный справочник пар BingX 2026-10-24
//! (tests/data/bingx_2026-10-24/symbols.json: CAKE-USDT minQty 2.672, minNotional 1) и записанный стакан CAKE
//! (tests/data/bingx/ws_spot_2026-10-23.jsonl). Моков нет. Проверяется:
//! (1) состояние кнопки: котируемой USDT — кнопки нет; CAKE меньше minQty — недоступна с причиной check_order;
//!     CAKE 5 — доступна; счёт фьючерсов — перевода нет; монеты без стакана — нет пары / не в подписке;
//! (2) монета в сделке (метка Я-2, остановленная сделка с монетами на бирже) — недоступна и нажатие отказано;
//! (3) «уже продаётся» — отказ, чужая метка не снимается;
//! (4) нажатие, которое падает после постановки метки (балансы счетов не читаются — ключей нет) — метки сняты,
//!     журнал core.balance_sell с тем, кто нажал;
//! (5) общий шаг перевода на торговый счёт (to_trading.rs, «Продать сейчас» по монете на фонде — случай ION 05.11):
//!     роли счетов BingX; со счёта без перевода — отказ; с фонда — запрос уходит в адаптер (здесь отказ ключей).
//! Успешную продажу и успешный перевод проверить нечем: записанных ответов ордера и перевода BingX в tests/data нет,
//! а площадка-заглушка запрещена (CLAUDE.md).
//! Кто вызывает: cargo test.
//! От чего зависит: arb::core (balance_sell, balance_sell_check, to_trading), arb::venues::bingx, SQLite.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use arb::core::Core;
use arb::core::balance_sell_check::{self as chk, IN_DEAL, SELLING};
use arb::core::coin_key::CoinKey;
use arb::core::deal::{Deal, Step};
use arb::core::deps::Deps;
use arb::core::engine::lk;
use arb::core::types::{Asset, CommandSource, Direction, ManualAction, ManualCommand, VenueId};
use arb::gateway::contract::{AccountRole, Venue};
use arb::gateway::events::MarketEvent;
use arb::gateway::registry::Registry;
use arb::notify::dispatch::Notifier;
use arb::settings::store::Store;
use arb::settings::value::Scope;
use arb::storage::db::Db;
use arb::storage::journal::Journal;
use arb::venues::bingx::depth::{self, Msg};
use arb::venues::bingx::{Bingx, symbols};
use chrono::DateTime;
use rust_decimal::Decimal;
use serde_json::{Value, json};
use tokio::sync::broadcast;

/// Время ответа symbols в сыром журнале (мс) — tests/data/bingx_2026-10-24/README.md.
const SYMBOLS_AT_MS: i64 = 1792868538956;

fn d(s: &str) -> Decimal {
    Decimal::from_str(s).unwrap()
}

fn bingx() -> VenueId {
    VenueId("bingx".into())
}

fn cake() -> Asset {
    Asset {
        ticker: "CAKE".into(),
        bsc_contract: None,
    }
}

/// Папка теста (новая).
fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("arb-balsell-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Ядро с настоящим адаптером BingX: справочник пар — записанный, стакан CAKE — записанный кадр.
async fn start(d: &Path) -> (Core, Journal, Arc<Bingx>) {
    let db = Db::open(&d.join("t.sqlite")).unwrap();
    let (changes, _) = broadcast::channel(64);
    let (jtx, _) = broadcast::channel(64);
    let journal = Journal::new(db.clone(), jtx);
    let settings = Store::load(db.clone(), changes.clone(), journal.clone())
        .await
        .unwrap();
    let v = Scope::Venue("bingx".into());
    for (n, val) in [
        ("venue.quote_asset", json!("USDT")),
        ("venue.symbols_refresh_s", json!(1_000_000_000)),
        ("venue.order_time_in_force", json!("IOC")),
    ] {
        settings.set(n, v.clone(), Some(val), "тест").await.unwrap();
    }
    let vault = arb::secrets::Vault::locked(d.join("secrets.enc"));
    let ctx = arb::venues::bingx::ctx::Ctx {
        settings: settings.clone(),
        vault: vault.clone(),
        http: reqwest::Client::new(),
        journal: journal.clone(),
        raw: arb::storage::raw_log::RawLog::start(d.to_path_buf(), settings.clone()),
        taps: Default::default(),
    };
    let bx = Arc::new(Bingx::new(ctx));
    let raw = std::fs::read_to_string("tests/data/bingx_2026-10-24/symbols.json").unwrap();
    let list: Value = serde_json::from_str(&raw).unwrap();
    bx.symbols().load(
        symbols::parse_list(&list["data"], "USDT").unwrap(),
        DateTime::from_timestamp_millis(SYMBOLS_AT_MS).unwrap(),
    );
    let gw = Registry::new();
    gw.register(bx.clone()).await;
    let notifier = Notifier {
        http: reqwest::Client::new(),
        settings: settings.clone(),
        vault,
        journal: journal.clone(),
    };
    let core = Core::start(Deps {
        gw,
        settings,
        journal: journal.clone(),
        notifier,
        db,
        changes,
        buffer: 64,
        data_dir: d.to_path_buf(),
    })
    .await;
    core.inner
        .market
        .write()
        .unwrap()
        .apply(MarketEvent::Book(cake_book()));
    (core, journal, bx)
}

/// Первый записанный стакан CAKE-USDT BingX.
fn cake_book() -> arb::core::types::OrderBook {
    let raw = std::fs::read_to_string("tests/data/bingx/ws_spot_2026-10-23.jsonl").unwrap();
    for line in raw.lines() {
        let m: Value = serde_json::from_str(line).unwrap();
        if let Msg::Depth {
            symbol,
            bids,
            asks,
            exchange_time,
        } = depth::parse(m["m"].as_str().unwrap())
            && symbol == "CAKE-USDT"
        {
            return depth::to_book(cake(), bids, asks, exchange_time);
        }
    }
    panic!("в записи нет стакана CAKE-USDT")
}

/// Нажатие «Продать» из интерфейса (тот же путь Core::command, что у остальных кнопок).
async fn press(core: &Core, account: &str, asset: &str) -> Result<String, String> {
    core.command(ManualCommand {
        action: ManualAction::SellBalance {
            venue: "bingx".into(),
            account: account.into(),
            asset: asset.into(),
        },
        source: CommandSource::Ui,
        by: Some("тест".into()),
        at: chrono::Utc::now(),
    })
    .await
}

/// Метки монеты сняты (и Я-2, и «продаётся»).
fn unmarked(core: &Core) -> bool {
    let k = CoinKey::of(&cake());
    !lk(&core.inner.active).contains(&k) && !lk(&core.inner.balance_sells).contains(&k)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn button_state_by_quote_minimum_account_and_book() {
    let (core, _, bx) = start(&dir("state")).await;
    let gw: &dyn Venue = bx.as_ref();
    assert!(
        chk::state(&core, gw, "Спот", "USDT", d("254.53"))
            .await
            .is_none()
    );
    // Меньше minQty 2.672 — та же проверка check_order (order_rules::ready), что у предпроверки продажи.
    let e = chk::state(&core, gw, "Спот", "CAKE", d("1"))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        e.contains("биржа не примет ордер") && e.contains("minQty 2.672"),
        "{e}"
    );
    assert!(
        chk::state(&core, gw, "Спот", "CAKE", d("5"))
            .await
            .unwrap()
            .is_ok()
    );
    // С фонда — перевод есть (контракт move_to_trading), проверка та же.
    assert!(
        chk::state(&core, gw, "Фонд (Funding)", "CAKE", d("5"))
            .await
            .unwrap()
            .is_ok()
    );
    let e = chk::state(&core, gw, "Фьючерсы USDT-M", "CAKE", d("5"))
        .await
        .unwrap()
        .unwrap_err();
    assert!(e.contains("перевода со счёта «Фьючерсы USDT-M»"), "{e}");
    // Стакана нет: монета не в подписке бота (пары в SpotPairs ядра тоже нет).
    let e = chk::state(&core, gw, "Спот", "TWT", d("5"))
        .await
        .unwrap()
        .unwrap_err();
    assert!(e.contains("нет пары TWT/USDT на bingx"), "{e}");
    assert_eq!(
        chk::state(&core, gw, "Спот", "CAKE", d("0"))
            .await
            .unwrap()
            .unwrap_err(),
        "свободных нет"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn coin_in_deal_is_refused() {
    let (core, journal, bx) = start(&dir("deal")).await;
    let gw: &dyn Venue = bx.as_ref();
    // Метка Я-2: сделка бота по CAKE.
    lk(&core.inner.active).insert(CoinKey::of(&cake()));
    let e = chk::state(&core, gw, "Спот", "CAKE", d("5"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(e, IN_DEAL);
    assert_eq!(press(&core, "Спот", "CAKE").await.unwrap_err(), IN_DEAL);
    // Метку сделки кнопка не трогает.
    assert!(lk(&core.inner.active).contains(&CoinKey::of(&cake())));
    lk(&core.inner.active).clear();
    // Остановленная сделка с монетами на bingx.
    let mut x = Deal::new(
        "t1CAKE".into(),
        Direction {
            asset: cake(),
            buy_on: VenueId("bsc_dex".into()),
            sell_on: bingx(),
        },
        json!(null),
        Decimal::ZERO,
    );
    x.at = bingx();
    x.qty = d("10");
    x.step = Step::Halted;
    arb::core::ledger::save(&core, &x).await;
    assert_eq!(
        press(&core, "Фонд (Funding)", "CAKE").await.unwrap_err(),
        IN_DEAL
    );
    assert!(unmarked(&core));
    let rows = journal.list(200, None).await.unwrap();
    assert!(
        rows.iter()
            .any(|r| r.kind == "core.balance_sell" && r.message.contains("(интерфейс)"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_press_while_selling_is_refused() {
    let (core, _, bx) = start(&dir("twice")).await;
    lk(&core.inner.balance_sells).insert(CoinKey::of(&cake()));
    let e = chk::state(&core, bx.as_ref(), "Спот", "CAKE", d("5"))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(e, SELLING);
    assert_eq!(press(&core, "Спот", "CAKE").await.unwrap_err(), SELLING);
    // Метка идущей продажи остаётся — её снимет та продажа.
    assert!(lk(&core.inner.balance_sells).contains(&CoinKey::of(&cake())));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failure_after_mark_removes_marks_and_logs_who() {
    let (core, journal, _) = start(&dir("fail")).await;
    // Ключей нет: свежие балансы счёта не читаются — сбой уже под меткой.
    let e = press(&core, "Спот", "CAKE").await.unwrap_err();
    assert!(e.contains("bingx: балансы"), "{e}");
    assert!(unmarked(&core), "метки сняты при ошибке (урок решения 165)");
    let rows = journal.list(200, None).await.unwrap();
    let r = rows
        .iter()
        .find(|r| r.kind == "core.balance_sell")
        .expect("запись журнала");
    assert!(
        r.message.contains("CAKE") && r.message.contains("(интерфейс)"),
        "{}",
        r.message
    );
    // Кнопка снова доступна (метки нет).
    assert!(
        press(&core, "Спот", "USDT")
            .await
            .unwrap_err()
            .contains("котируемая")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_move_to_trading_step() {
    let (_core, _, bx) = start(&dir("move")).await;
    let gw: &dyn Venue = bx.as_ref();
    assert_eq!(gw.account_role("Спот"), AccountRole::Trading);
    assert_eq!(gw.account_role("Фонд (Funding)"), AccountRole::ToTrading);
    assert_eq!(gw.account_role("Фьючерсы USDT-M"), AccountRole::Other);
    let e = arb::core::to_trading::move_from(gw, "Фьючерсы USDT-M", &cake(), d("5"))
        .await
        .unwrap_err();
    assert!(e.contains("перевода со счёта"), "{e}");
    // С фонда запрос уходит в перевод адаптера (fund → spot) — здесь отказ: ключей нет.
    let e = arb::core::to_trading::move_from(gw, "Фонд (Funding)", &cake(), d("5"))
        .await
        .unwrap_err();
    assert!(
        e.contains("«Фонд (Funding)» → торговый счёт bingx не прошёл"),
        "{e}"
    );
    // «Продать сейчас» (on_hand_sell): на торговом нет, балансы не читаются — причина, а не «нет свободных».
    let e = arb::core::to_trading::top_up(gw, &cake(), d("905432"))
        .await
        .unwrap_err();
    assert!(e.contains("bingx: баланс торгового счёта"), "{e}");
}
